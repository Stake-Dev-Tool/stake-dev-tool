use crate::config::ServerConfig;
use crate::error::{AppError, AppResult};
use crate::types::{GameConfig, GameMode, WeightEntry};
use dashmap::DashMap;
use memmap2::Mmap;
use rand::RngCore;
use serde::Serialize;
use serde_json::value::RawValue;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::OnceCell;

pub struct BooksIndex {
    /// Decompressed books, backed by an unlinked temp file via mmap. File-backed
    /// pages stay clean, so the OS can reclaim them under memory pressure
    /// instead of pushing multi-GB buffers to swap.
    buffer: Mmap,
    /// Maps each book's `id` field to its (start, end) byte range in `buffer`.
    /// Built by scanning every line at load time — indexing by `id` rather than
    /// by line position because math-sdk writes `library[sim+1] = Book(sim)`,
    /// so line N contains id N-1 (not id N as the name might suggest).
    /// Offsets are u64: decompressed books routinely exceed 4 GiB.
    pub id_to_range: HashMap<u32, (u64, u64)>,
}

pub struct WeightSampler {
    pub entries: Vec<WeightEntry>,
    pub cum_weights: Vec<u64>,
    pub total_weight: u64,
}

pub struct ModeAssets {
    pub sampler: Arc<WeightSampler>,
    pub books: Arc<BooksIndex>,
}

struct CachedMode {
    key: String,
    bytes: u64,
    cell: Arc<OnceCell<Arc<ModeAssets>>>,
}

/// Decompressed books are huge (frequently several GiB per mode), so only the
/// most recently used modes stay cached; older entries are dropped, releasing
/// their temp-file-backed mmaps. In-flight spins keep their `Arc<ModeAssets>`
/// alive, so eviction is safe mid-request. The cache is bounded both by entry
/// count and by total decompressed bytes, since each cached mode holds its
/// decompressed size in temp-file disk space.
const MAX_CACHED_MODES: usize = 8;
const MAX_CACHED_BYTES: u64 = 40 * 1024 * 1024 * 1024;

pub struct MathEngine {
    cfg: ServerConfig,
    configs: DashMap<String, Arc<OnceCell<Arc<GameConfig>>>>,
    modes: DashMap<String, Arc<OnceCell<Arc<ModeAssets>>>>,
    mode_lru: parking_lot::Mutex<Vec<CachedMode>>,
}

impl MathEngine {
    pub fn new(cfg: ServerConfig) -> Self {
        Self {
            cfg,
            configs: DashMap::new(),
            modes: DashMap::new(),
            mode_lru: parking_lot::Mutex::new(Vec::new()),
        }
    }

    fn touch_mode_cache(&self, key: &str, cell: &Arc<OnceCell<Arc<ModeAssets>>>, bytes: u64) {
        // Keep the LRU and DashMap mutation under one lock. A request may have
        // cloned a cell just before another request evicts it; in that case it
        // may finish safely, but it must not re-add a phantom LRU entry for a
        // cell that is no longer present in `modes`.
        let mut lru = self.mode_lru.lock();
        let is_current = self
            .modes
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current.value(), cell));
        if !is_current {
            return;
        }

        lru.retain(|entry| entry.key != key);
        lru.push(CachedMode {
            key: key.to_string(),
            bytes,
            cell: Arc::clone(cell),
        });

        // Always keep at least the entry just touched, whatever its size.
        while lru.len() > 1
            && (lru.len() > MAX_CACHED_MODES
                || lru.iter().map(|entry| entry.bytes).sum::<u64>() > MAX_CACHED_BYTES)
        {
            let old = lru.remove(0);
            let removed = self
                .modes
                .remove_if(&old.key, |_, current| Arc::ptr_eq(current, &old.cell))
                .is_some();
            if removed {
                tracing::info!(
                    mode = %old.key,
                    gib = old.bytes / (1024 * 1024 * 1024),
                    "evicted books cache (LRU)"
                );
            }
        }
    }

    fn file_path(&self, game: &str, file: &str) -> PathBuf {
        let root = PathBuf::from(&self.cfg.math_dir);
        let nested = root.join(game).join(file);
        if nested.exists() {
            return nested;
        }

        let flat = root.join(file);
        if flat.exists() {
            return flat;
        }

        nested
    }

    async fn read_file(&self, game: &str, file: &str) -> AppResult<Vec<u8>> {
        let path = self.file_path(game, file);
        fs::read(&path)
            .await
            .map_err(|e| AppError::Parse(format!("read {}: {e}", path.display())))
    }

    pub async fn load_config(&self, game: &str) -> AppResult<Arc<GameConfig>> {
        let cell = self
            .configs
            .entry(game.to_string())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        cell.get_or_try_init(|| async {
            let bytes = self.read_file(game, "index.json").await?;
            let cfg: GameConfig = sonic_rs::from_slice(&bytes)
                .map_err(|e| AppError::Parse(format!("index.json: {e}")))?;
            Ok::<Arc<GameConfig>, AppError>(Arc::new(cfg))
        })
        .await
        .cloned()
    }

    pub async fn get_mode(&self, game: &str, mode_name: &str) -> AppResult<GameMode> {
        let cfg = self.load_config(game).await?;
        cfg.modes
            .iter()
            .find(|m| m.name == mode_name)
            .cloned()
            .ok_or_else(|| AppError::ModeNotFound {
                game: game.to_string(),
                mode: mode_name.to_string(),
            })
    }

    pub async fn get_mode_cost(&self, game: &str, mode_name: &str) -> AppResult<u64> {
        Ok(self
            .get_mode(game, mode_name)
            .await
            .map(|m| m.cost)
            .unwrap_or(1))
    }

    pub async fn load_assets(&self, game: &str, mode: &GameMode) -> AppResult<Arc<ModeAssets>> {
        let key = format!("{game}:{}", mode.name);
        let cell = self
            .modes
            .entry(key.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        let game = game.to_string();
        let mode = mode.clone();
        let assets = cell
            .get_or_try_init(|| async move {
                let weights_bytes = self.read_file(&game, &mode.weights).await?;

                let weights_text = String::from_utf8(weights_bytes)
                    .map_err(|e| AppError::Parse(format!("weights utf8: {e}")))?;
                let sampler = parse_weights(&weights_text)?;

                // Stream the compressed books straight from disk: at up to ~1 GiB
                // compressed, buffering the whole file first would leave a same-
                // sized hole in the allocator's large-block cache on every load.
                // Decompression and file I/O are blocking, so keep them off the
                // asynchronous request workers.
                let books_path = self.file_path(&game, &mode.events);
                let required_ids: Vec<u32> = sampler.entries.iter().map(|e| e.event_id).collect();
                let books = tokio::task::spawn_blocking(move || {
                    let books_file = std::fs::File::open(&books_path).map_err(|e| {
                        AppError::Parse(format!("read {}: {e}", books_path.display()))
                    })?;
                    decompress_and_index(
                        std::io::BufReader::with_capacity(4 << 20, books_file),
                        &required_ids,
                    )
                })
                .await
                .map_err(|e| AppError::Parse(format!("books loader task failed: {e}")))??;

                Ok::<Arc<ModeAssets>, AppError>(Arc::new(ModeAssets {
                    sampler: Arc::new(sampler),
                    books: Arc::new(books),
                }))
            })
            .await
            .cloned()?;
        self.touch_mode_cache(&key, &cell, assets.books.buffer.len() as u64);
        Ok(assets)
    }

    pub async fn preload(&self, game: &str) -> AppResult<()> {
        let cfg = self.load_config(game).await?;
        if let Some(base) = cfg.modes.iter().find(|m| m.name == "base") {
            self.load_assets(game, base).await?;
        }
        Ok(())
    }

    pub async fn play_spin(
        &self,
        game: &str,
        mode_name: &str,
        bet_amount: u64,
    ) -> AppResult<SpinResult> {
        let mode = self.get_mode(game, mode_name).await?;
        let assets = self.load_assets(game, &mode).await?;

        let pick = weighted_pick(&assets.sampler);
        self.build_result(
            &mode,
            &assets,
            pick.event_id,
            pick.payout_multiplier,
            bet_amount,
        )
    }

    /// Like `play_spin` but forces a specific event id (bypasses the RNG).
    /// Used for replay / debug "force next event" flows.
    pub async fn play_forced(
        &self,
        game: &str,
        mode_name: &str,
        bet_amount: u64,
        event_id: u32,
    ) -> AppResult<SpinResult> {
        let mode = self.get_mode(game, mode_name).await?;
        let assets = self.load_assets(game, &mode).await?;

        // Find the weight entry for this event to get the authoritative payout
        // multiplier. Weights table is small (~1k entries), a linear search is fine.
        let entry = assets
            .sampler
            .entries
            .iter()
            .find(|e| e.event_id == event_id)
            .ok_or_else(|| AppError::Parse(format!("event {event_id} not found in weights")))?;

        self.build_result(
            &mode,
            &assets,
            entry.event_id,
            entry.payout_multiplier,
            bet_amount,
        )
    }

    fn build_result(
        &self,
        mode: &GameMode,
        assets: &Arc<ModeAssets>,
        event_id: u32,
        payout_multiplier: u32,
        bet_amount: u64,
    ) -> AppResult<SpinResult> {
        let state = read_event(&assets.books, event_id)?;
        let base_bet = bet_amount / mode.cost.max(1);
        let payout = (base_bet.saturating_mul(payout_multiplier as u64)) / 100;
        Ok(SpinResult {
            event_id,
            payout_multiplier,
            payout,
            state,
        })
    }

    /// Compute notable bet ids per mode (lowest-payout / "average" winning hit
    /// / max-payout). Loads each mode's sampler — already cached after the
    /// first call — so a second call is essentially free. Used by the test
    /// view's "Notable rounds" panel.
    pub async fn game_bet_stats(&self, game: &str) -> AppResult<Vec<ModeBetStats>> {
        let cfg = self.load_config(game).await?;
        let mut out = Vec::with_capacity(cfg.modes.len());
        for mode in &cfg.modes {
            let weights_bytes = self.read_file(game, &mode.weights).await?;
            let weights_text = String::from_utf8(weights_bytes)
                .map_err(|e| AppError::Parse(format!("weights utf8: {e}")))?;
            let sampler = parse_weights(&weights_text)?;
            if let Some(stats) = compute_bet_stats(&sampler) {
                out.push(ModeBetStats {
                    mode: mode.name.clone(),
                    stats,
                });
            }
        }
        Ok(out)
    }

    /// Fetch the raw event state + payout multiplier for replay / bet-replay endpoint.
    pub async fn replay_event(
        &self,
        game: &str,
        mode_name: &str,
        event_id: u32,
    ) -> AppResult<ReplayResult> {
        let mode = self.get_mode(game, mode_name).await?;
        let assets = self.load_assets(game, &mode).await?;
        let entry = assets
            .sampler
            .entries
            .iter()
            .find(|e| e.event_id == event_id)
            .ok_or_else(|| AppError::Parse(format!("event {event_id} not found in weights")))?;
        let state = read_event(&assets.books, entry.event_id)?;
        Ok(ReplayResult {
            payout_multiplier: entry.payout_multiplier,
            cost_multiplier: mode.cost,
            state,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct NotableBet {
    #[serde(rename = "eventId")]
    pub event_id: u32,
    #[serde(rename = "payoutMultiplier")]
    pub payout_multiplier: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BetStats {
    pub zero: Vec<NotableBet>,
    pub low: Vec<NotableBet>,
    pub medium: Vec<NotableBet>,
    pub big: Vec<NotableBet>,
    pub max: Vec<NotableBet>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModeBetStats {
    pub mode: String,
    pub stats: BetStats,
}

fn notable_from(entry: &WeightEntry) -> NotableBet {
    NotableBet {
        event_id: entry.event_id,
        payout_multiplier: entry.payout_multiplier,
    }
}

fn compute_bet_stats(sampler: &WeightSampler) -> Option<BetStats> {
    if sampler.entries.is_empty() {
        return None;
    }

    let mut zeroes: Vec<&WeightEntry> = sampler
        .entries
        .iter()
        .filter(|e| e.payout_multiplier == 0)
        .collect();
    zeroes.sort_by_key(|e| (std::cmp::Reverse(e.weight), e.event_id));

    // Weighted mean of winning payoutMultipliers — represents the EV of a
    let mut winners: Vec<&WeightEntry> = sampler
        .entries
        .iter()
        .filter(|e| e.payout_multiplier > 0)
        .collect();
    winners.sort_by_key(|e| (e.payout_multiplier, e.event_id));

    Some(BetStats {
        zero: zeroes.into_iter().take(1).map(notable_from).collect(),
        low: winners.iter().take(2).map(|e| notable_from(e)).collect(),
        medium: notable_near_percentile(&winners, 1, 2, 2),
        big: notable_near_percentile(&winners, 4, 5, 2),
        max: winners
            .iter()
            .rev()
            .take(2)
            .map(|e| notable_from(e))
            .collect(),
    })
}

fn notable_near_percentile(
    sorted_winners: &[&WeightEntry],
    numerator: usize,
    denominator: usize,
    count: usize,
) -> Vec<NotableBet> {
    if sorted_winners.is_empty() || denominator == 0 {
        return Vec::new();
    }
    let target_idx = ((sorted_winners.len() - 1) * numerator) / denominator;
    let target = sorted_winners[target_idx].payout_multiplier;
    let mut entries = sorted_winners.to_vec();
    entries.sort_by_key(|e| (e.payout_multiplier.abs_diff(target), e.event_id));
    entries.truncate(count);
    entries.sort_by_key(|e| (e.payout_multiplier, e.event_id));
    entries.into_iter().map(notable_from).collect()
}

pub struct ReplayResult {
    pub payout_multiplier: u32,
    pub cost_multiplier: u64,
    pub state: Arc<RawValue>,
}

pub struct SpinResult {
    pub event_id: u32,
    pub payout_multiplier: u32,
    pub payout: u64,
    pub state: Arc<RawValue>,
}

fn parse_weights(text: &str) -> AppResult<WeightSampler> {
    let mut entries = Vec::with_capacity(1024);
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split(',');
        let event_id = it
            .next()
            .ok_or_else(|| AppError::Parse(format!("weights line {lineno}: missing eventId")))?
            .trim()
            .parse::<u32>()
            .map_err(|e| AppError::Parse(format!("weights line {lineno}: eventId: {e}")))?;
        let weight = it
            .next()
            .ok_or_else(|| AppError::Parse(format!("weights line {lineno}: missing weight")))?
            .trim()
            .parse::<u64>()
            .map_err(|e| AppError::Parse(format!("weights line {lineno}: weight: {e}")))?;
        let payout_multiplier = it
            .next()
            .ok_or_else(|| AppError::Parse(format!("weights line {lineno}: missing payout")))?
            .trim()
            .parse::<u32>()
            .map_err(|e| AppError::Parse(format!("weights line {lineno}: payout: {e}")))?;
        entries.push(WeightEntry {
            event_id,
            weight,
            payout_multiplier,
        });
    }

    let mut cum_weights = Vec::with_capacity(entries.len());
    let mut total: u64 = 0;
    for e in &entries {
        total = total
            .checked_add(e.weight)
            .ok_or_else(|| AppError::Parse("weights overflow u64".into()))?;
        cum_weights.push(total);
    }

    Ok(WeightSampler {
        entries,
        cum_weights,
        total_weight: total,
    })
}

fn decompress_and_index(
    compressed: impl std::io::Read,
    required_ids: &[u32],
) -> AppResult<BooksIndex> {
    // Stream-decompress into an unlinked temp file, then mmap it read-only.
    // The temp file has no path (already deleted); the OS frees the disk space
    // as soon as the mmap is dropped.
    let file = tempfile::tempfile().map_err(|e| AppError::Zstd(format!("temp books file: {e}")))?;

    // Fast path: math-sdk publish files are strict JSONL, so ids can be
    // harvested from line starts while the decompressed stream is being
    // written out — the multi-GiB buffer is never re-read. Trust the result
    // only if it accounts for every id the weights table can ask for;
    // otherwise (adjacent records, multi-line values…) fall back to an
    // exhaustive JSON stream scan of the mmap.
    let mut writer = IndexingWriter::new(std::io::BufWriter::with_capacity(4 << 20, file));
    zstd::stream::copy_decode(compressed, &mut writer)
        .map_err(|e| AppError::Zstd(e.to_string()))?;
    let (writer, mut id_to_range, line_index_complete) = writer.finish();
    let file = writer
        .into_inner()
        .map_err(|e| AppError::Zstd(format!("flush books: {e}")))?;
    let buffer =
        unsafe { Mmap::map(&file) }.map_err(|e| AppError::Zstd(format!("mmap books: {e}")))?;

    if !line_index_complete || !required_ids.iter().all(|id| id_to_range.contains_key(id)) {
        tracing::warn!("line-based books index incomplete; falling back to full JSON scan");
        id_to_range = index_by_json_stream(&buffer)?;
    }

    Ok(BooksIndex {
        buffer,
        id_to_range,
    })
}

/// The first bytes of a line always suffice to read its `id` field
/// (`{"id":N` with optional whitespace), so no full line is ever buffered.
const LINE_HEAD_CAP: usize = 64;

/// Write adapter that forwards the decompressed stream to `inner` while
/// building the id → byte-range index from line boundaries on the fly.
struct IndexingWriter<W: std::io::Write> {
    inner: W,
    offset: u64,
    line_start: u64,
    head: Vec<u8>,
    line_last_non_ws: Option<u8>,
    line_has_adjacent_records: bool,
    previous_byte: Option<u8>,
    line_index_complete: bool,
    id_to_range: HashMap<u32, (u64, u64)>,
}

impl<W: std::io::Write> IndexingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            offset: 0,
            line_start: 0,
            head: Vec::with_capacity(LINE_HEAD_CAP),
            line_last_non_ws: None,
            line_has_adjacent_records: false,
            previous_byte: None,
            line_index_complete: true,
            id_to_range: HashMap::with_capacity(64 * 1024),
        }
    }

    fn feed_line_segment(&mut self, segment: &[u8]) {
        let take = segment
            .len()
            .min(LINE_HEAD_CAP.saturating_sub(self.head.len()));
        self.head.extend_from_slice(&segment[..take]);

        if self.previous_byte == Some(b'}') && segment.first() == Some(&b'{') {
            self.line_has_adjacent_records = true;
        }
        if memchr::memmem::find(segment, b"}{").is_some() {
            self.line_has_adjacent_records = true;
        }
        if let Some(byte) = segment.iter().rfind(|byte| !byte.is_ascii_whitespace()) {
            self.line_last_non_ws = Some(*byte);
        }
        if let Some(byte) = segment.last() {
            self.previous_byte = Some(*byte);
        }
    }

    fn finish_line(&mut self, line_end: u64) {
        if self.line_last_non_ws.is_some() {
            if self.line_last_non_ws == Some(b'}') && !self.line_has_adjacent_records {
                if let Some(id) = read_id_field(&self.head) {
                    self.id_to_range.insert(id, (self.line_start, line_end));
                } else {
                    self.line_index_complete = false;
                }
            } else {
                self.line_index_complete = false;
            }
        }

        self.line_start = line_end + 1;
        self.head.clear();
        self.line_last_non_ws = None;
        self.line_has_adjacent_records = false;
        self.previous_byte = None;
    }

    fn feed(&mut self, buf: &[u8]) {
        let mut pos = 0usize;
        while pos < buf.len() {
            match memchr::memchr(b'\n', &buf[pos..]) {
                Some(i) => {
                    self.feed_line_segment(&buf[pos..pos + i]);
                    let nl_abs = self.offset + (pos + i) as u64;
                    self.finish_line(nl_abs);
                    pos += i + 1;
                }
                None => {
                    self.feed_line_segment(&buf[pos..]);
                    break;
                }
            }
        }
        self.offset += buf.len() as u64;
    }

    fn finish(mut self) -> (W, HashMap<u32, (u64, u64)>, bool) {
        // Trailing line without a final newline.
        if self.offset > self.line_start {
            self.finish_line(self.offset);
        }
        self.id_to_range.shrink_to_fit();
        (self.inner, self.id_to_range, self.line_index_complete)
    }
}

impl<W: std::io::Write> std::io::Write for IndexingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write_all(buf)?;
        self.feed(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn index_by_json_stream(buffer: &[u8]) -> AppResult<HashMap<u32, (u64, u64)>> {
    // Books average well above 512 bytes each (often tens of KB), so sizing the
    // map from len/512 over-allocates by orders of magnitude on multi-GiB
    // files; start modest and shrink once the real count is known.
    let mut id_to_range = HashMap::with_capacity(64 * 1024);
    let mut stream =
        serde_json::Deserializer::from_slice(buffer).into_iter::<serde::de::IgnoredAny>();
    while let Some(item) = {
        let start = stream.byte_offset();
        stream.next().map(|item| (start, item))
    } {
        let (start, item) = item;
        item.map_err(|e| AppError::Parse(format!("books json stream at byte {start}: {e}")))?;
        index_record(buffer, start, stream.byte_offset(), &mut id_to_range);
    }
    id_to_range.shrink_to_fit();
    Ok(id_to_range)
}

fn index_record(
    buffer: &[u8],
    record_start: usize,
    record_end: usize,
    id_to_range: &mut HashMap<u32, (u64, u64)>,
) {
    if record_end <= record_start {
        return;
    }
    if let Some(id) = read_id_field(&buffer[record_start..record_end]) {
        id_to_range.insert(id, (record_start as u64, record_end as u64));
    }
}

/// Pull the `id` value out of a line without parsing the (potentially huge)
/// events array. math-sdk always writes `"id"` as the first key of each book,
/// so we scan for `{"id":N` with optional whitespace. Lines that don't match
/// are skipped silently — they won't be reachable via event lookup anyway.
fn read_id_field(slice: &[u8]) -> Option<u32> {
    let i = skip_ws(slice, 0);
    if *slice.get(i)? != b'{' {
        return None;
    }
    let i = skip_ws(slice, i + 1);
    if slice.get(i..i + 4)? != b"\"id\"" {
        return None;
    }
    let i = skip_ws(slice, i + 4);
    if *slice.get(i)? != b':' {
        return None;
    }
    let start = skip_ws(slice, i + 1);
    let mut end = start;
    while slice.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if end == start {
        return None;
    }
    std::str::from_utf8(&slice[start..end]).ok()?.parse().ok()
}

fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    i
}

fn weighted_pick(sampler: &WeightSampler) -> WeightEntry {
    let mut rng = rand::thread_rng();
    let r = rng.next_u64();
    let pick = r % sampler.total_weight;
    let cw = &sampler.cum_weights;
    let mut lo = 0usize;
    let mut hi = cw.len() - 1;
    while lo < hi {
        let mid = (lo + hi) >> 1;
        if cw[mid] <= pick {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    sampler.entries[lo]
}

fn read_event(idx: &BooksIndex, event_id: u32) -> AppResult<Arc<RawValue>> {
    let &(start, end) = idx.id_to_range.get(&event_id).ok_or_else(|| {
        AppError::Parse(format!(
            "event {event_id} not found in books ({} ids indexed)",
            idx.id_to_range.len()
        ))
    })?;
    let start = usize::try_from(start)
        .map_err(|_| AppError::Parse(format!("event {event_id} start offset is too large")))?;
    let end = usize::try_from(end)
        .map_err(|_| AppError::Parse(format!("event {event_id} end offset is too large")))?;
    let slice = idx.buffer.get(start..end).ok_or_else(|| {
        AppError::Parse(format!(
            "event {event_id} range {start}..{end} exceeds books size {}",
            idx.buffer.len()
        ))
    })?;

    #[derive(serde::Deserialize)]
    struct Wrapper<'a> {
        #[serde(borrow)]
        events: Option<&'a RawValue>,
    }

    let line_str =
        std::str::from_utf8(slice).map_err(|e| AppError::Parse(format!("event utf8: {e}")))?;
    let wrapper: Wrapper =
        serde_json::from_str(line_str).map_err(|e| AppError::Parse(format!("event parse: {e}")))?;
    let raw = match wrapper.events {
        Some(events) => RawValue::from_string(events.get().to_string())
            .map_err(|e| AppError::Parse(format!("event raw: {e}")))?,
        None => RawValue::from_string(line_str.to_string())
            .map_err(|e| AppError::Parse(format!("event raw: {e}")))?,
    };
    Ok(Arc::from(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    fn compressed_books(bytes: &[u8]) -> Vec<u8> {
        zstd::encode_all(bytes, 0).expect("compress test books")
    }

    #[test]
    fn indexes_newline_delimited_books() {
        let compressed = compressed_books(
            br#"{"id":1,"events":[{"symbol":"A"}]}
{"id":2,"events":[{"symbol":"B"}]}
"#,
        );

        let books = decompress_and_index(&compressed[..], &[1, 2]).expect("index books");
        let raw = read_event(&books, 2).expect("read event");

        assert_eq!(raw.get(), r#"[{"symbol":"B"}]"#);
    }

    #[test]
    fn indexes_adjacent_books_without_newlines() {
        let compressed = compressed_books(
            br#"{"id":10,"events":[{"bonus":false}]}{"id":11,"events":[{"bonus":true}]}"#,
        );

        // Adjacent records defeat the line-based fast path; requiring id 11
        // must force the fallback JSON stream scan, which still finds it.
        let books = decompress_and_index(&compressed[..], &[10, 11]).expect("index books");
        let raw = read_event(&books, 11).expect("read event");

        assert_eq!(raw.get(), r#"[{"bonus":true}]"#);
    }

    #[test]
    fn indexes_adjacent_books_when_only_first_id_is_required() {
        let compressed = compressed_books(
            br#"{"id":10,"events":[{"bonus":false}]}{"id":11,"events":[{"bonus":true}]}"#,
        );

        let books = decompress_and_index(&compressed[..], &[10]).expect("index books");
        let raw = read_event(&books, 10).expect("read event");

        assert_eq!(raw.get(), r#"[{"bonus":false}]"#);
    }

    #[test]
    fn indexes_multiline_books_via_fallback() {
        let compressed = compressed_books(
            br#"{"id":20,
"events":[{"symbol":"A"}]}
{"id":21,
"events":[{"symbol":"B"}]}
"#,
        );

        let books = decompress_and_index(&compressed[..], &[20, 21]).expect("index books");
        let raw = read_event(&books, 21).expect("read event");

        assert_eq!(raw.get(), r#"[{"symbol":"B"}]"#);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn reads_an_event_beyond_the_four_gib_boundary() {
        let mut file = tempfile::tempfile().expect("create sparse books file");
        let record = br#"{"id":42,"events":[{"beyond":"4 GiB"}]}"#;
        let start = u32::MAX as u64 + 4096;
        let end = start + record.len() as u64;
        file.set_len(end).expect("extend sparse books file");
        file.seek(SeekFrom::Start(start))
            .expect("seek beyond 4 GiB");
        file.write_all(record).expect("write event record");
        file.flush().expect("flush event record");

        let buffer = unsafe { Mmap::map(&file) }.expect("map sparse books file");
        let books = BooksIndex {
            buffer,
            id_to_range: HashMap::from([(42, (start, end))]),
        };
        let raw = read_event(&books, 42).expect("read event beyond 4 GiB");

        assert_eq!(raw.get(), r#"[{"beyond":"4 GiB"}]"#);
    }

    fn test_engine() -> MathEngine {
        MathEngine::new(ServerConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            math_dir: ".".to_string(),
            ui_dir: None,
        })
    }

    fn insert_cache_cell(engine: &MathEngine, key: &str) -> Arc<OnceCell<Arc<ModeAssets>>> {
        let cell = Arc::new(OnceCell::new());
        engine.modes.insert(key.to_string(), Arc::clone(&cell));
        cell
    }

    #[test]
    fn lru_evicts_the_oldest_mode_by_entry_count() {
        let engine = test_engine();
        for index in 0..=MAX_CACHED_MODES {
            let key = format!("game:mode-{index}");
            let cell = insert_cache_cell(&engine, &key);
            engine.touch_mode_cache(&key, &cell, 1);
        }

        assert!(!engine.modes.contains_key("game:mode-0"));
        assert_eq!(engine.modes.len(), MAX_CACHED_MODES);
        assert_eq!(engine.mode_lru.lock().len(), MAX_CACHED_MODES);
    }

    #[test]
    fn lru_evicts_the_oldest_mode_by_decompressed_bytes() {
        let engine = test_engine();
        let bytes = MAX_CACHED_BYTES / 2 + 1;
        for index in 0..3 {
            let key = format!("game:large-mode-{index}");
            let cell = insert_cache_cell(&engine, &key);
            engine.touch_mode_cache(&key, &cell, bytes);
        }

        assert!(!engine.modes.contains_key("game:large-mode-0"));
        assert!(!engine.modes.contains_key("game:large-mode-1"));
        assert!(engine.modes.contains_key("game:large-mode-2"));
        assert_eq!(engine.mode_lru.lock().len(), 1);
    }

    #[test]
    fn stale_cache_cell_cannot_create_a_phantom_lru_entry() {
        let engine = test_engine();
        let key = "game:base";
        let stale = insert_cache_cell(&engine, key);
        engine.modes.remove(key);
        let current = insert_cache_cell(&engine, key);

        engine.touch_mode_cache(key, &stale, 10);

        assert!(engine.mode_lru.lock().is_empty());
        assert!(
            engine
                .modes
                .get(key)
                .is_some_and(|cell| Arc::ptr_eq(cell.value(), &current))
        );
    }

    #[test]
    fn notable_buckets_cover_zero_low_medium_big_and_max() {
        let sampler = WeightSampler {
            entries: vec![
                WeightEntry {
                    event_id: 1,
                    weight: 10,
                    payout_multiplier: 0,
                },
                WeightEntry {
                    event_id: 2,
                    weight: 1,
                    payout_multiplier: 10,
                },
                WeightEntry {
                    event_id: 3,
                    weight: 1,
                    payout_multiplier: 20,
                },
                WeightEntry {
                    event_id: 4,
                    weight: 1,
                    payout_multiplier: 100,
                },
                WeightEntry {
                    event_id: 5,
                    weight: 1,
                    payout_multiplier: 200,
                },
                WeightEntry {
                    event_id: 6,
                    weight: 1,
                    payout_multiplier: 500,
                },
                WeightEntry {
                    event_id: 7,
                    weight: 1,
                    payout_multiplier: 1000,
                },
            ],
            cum_weights: vec![],
            total_weight: 0,
        };

        let stats = compute_bet_stats(&sampler).expect("stats");

        assert_eq!(stats.zero.len(), 1);
        assert_eq!(stats.low.len(), 2);
        assert_eq!(stats.medium.len(), 2);
        assert_eq!(stats.big.len(), 2);
        assert_eq!(stats.max.len(), 2);
        assert_eq!(stats.zero[0].event_id, 1);
        assert_eq!(stats.low[0].event_id, 2);
        assert_eq!(stats.max[0].event_id, 7);
    }
}
