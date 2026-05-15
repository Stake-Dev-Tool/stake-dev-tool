use crate::config::ServerConfig;
use crate::error::{AppError, AppResult};
use crate::types::{GameConfig, GameMode, WeightEntry};
use dashmap::DashMap;
use rand::RngCore;
use serde::Serialize;
use serde_json::value::RawValue;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::OnceCell;

pub struct BooksIndex {
    pub buffer: Vec<u8>,
    /// Maps each book's `id` field to its (start, end) byte range in `buffer`.
    /// Built by scanning every line at load time — indexing by `id` rather than
    /// by line position because math-sdk writes `library[sim+1] = Book(sim)`,
    /// so line N contains id N-1 (not id N as the name might suggest).
    pub id_to_range: HashMap<u32, (u32, u32)>,
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

pub struct MathEngine {
    cfg: ServerConfig,
    configs: DashMap<String, Arc<OnceCell<Arc<GameConfig>>>>,
    modes: DashMap<String, Arc<OnceCell<Arc<ModeAssets>>>>,
}

impl MathEngine {
    pub fn new(cfg: ServerConfig) -> Self {
        Self {
            cfg,
            configs: DashMap::new(),
            modes: DashMap::new(),
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
            .entry(key)
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        let game = game.to_string();
        let mode = mode.clone();
        cell.get_or_try_init(|| async move {
            let weights_bytes = self.read_file(&game, &mode.weights).await?;
            let books_bytes = self.read_file(&game, &mode.events).await?;

            let weights_text = String::from_utf8(weights_bytes)
                .map_err(|e| AppError::Parse(format!("weights utf8: {e}")))?;
            let sampler = parse_weights(&weights_text)?;

            let books = decompress_and_index(&books_bytes)?;

            Ok::<Arc<ModeAssets>, AppError>(Arc::new(ModeAssets {
                sampler: Arc::new(sampler),
                books: Arc::new(books),
            }))
        })
        .await
        .cloned()
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

fn decompress_and_index(compressed: &[u8]) -> AppResult<BooksIndex> {
    let buffer = zstd::decode_all(compressed).map_err(|e| AppError::Zstd(e.to_string()))?;

    let mut id_to_range = HashMap::with_capacity(buffer.len() / 512 + 1);
    let mut stream =
        serde_json::Deserializer::from_slice(&buffer).into_iter::<serde::de::IgnoredAny>();
    while let Some(item) = {
        let start = stream.byte_offset();
        stream.next().map(|item| (start, item))
    } {
        let (start, item) = item;
        item.map_err(|e| AppError::Parse(format!("books json stream at byte {start}: {e}")))?;
        index_record(&buffer, start, stream.byte_offset(), &mut id_to_range);
    }

    Ok(BooksIndex {
        buffer,
        id_to_range,
    })
}

fn index_record(
    buffer: &[u8],
    record_start: usize,
    record_end: usize,
    id_to_range: &mut HashMap<u32, (u32, u32)>,
) {
    if record_end <= record_start {
        return;
    }
    if let Some(id) = read_id_field(&buffer[record_start..record_end]) {
        id_to_range.insert(id, (record_start as u32, record_end as u32));
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
    let slice = &idx.buffer[start as usize..end as usize];

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

        let books = decompress_and_index(&compressed).expect("index books");
        let raw = read_event(&books, 2).expect("read event");

        assert_eq!(raw.get(), r#"[{"symbol":"B"}]"#);
    }

    #[test]
    fn indexes_adjacent_books_without_newlines() {
        let compressed = compressed_books(
            br#"{"id":10,"events":[{"bonus":false}]}{"id":11,"events":[{"bonus":true}]}"#,
        );

        let books = decompress_and_index(&compressed).expect("index books");
        let raw = read_event(&books, 11).expect("read event");

        assert_eq!(raw.get(), r#"[{"bonus":true}]"#);
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
