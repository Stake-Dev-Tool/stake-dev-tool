//! Browser-side preview engine for the Stake Dev Tool.
//!
//! Mirrors enough of the RGS contract (`/wallet/authenticate`, `/wallet/play`,
//! `/wallet/end-round`, `/wallet/balance`) so a slot game running in the
//! browser can talk to it instead of the native LGS. A service worker (or
//! direct JS shim) intercepts the HTTP calls the game makes and dispatches
//! to this crate's methods, then formats the JSON exactly the way the game
//! expects.
//!
//! Math files are fetched over HTTP at load time (one request per mode for
//! the weights CSV + the zstd-compressed books). Decompression happens in
//! WASM via `ruzstd` so we don't depend on a C library.

use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use wasm_bindgen::prelude::*;

const API_MULTIPLIER: u64 = 1_000_000;

// ============================================================
// Shared types — mirror the shapes the game expects on the wire.
// ============================================================

#[derive(Debug, Clone, Deserialize)]
struct GameMode {
    name: String,
    cost: u64,
    events: String,
    weights: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GameConfig {
    modes: Vec<GameMode>,
}

#[derive(Debug, Clone, Copy)]
struct WeightEntry {
    event_id: u32,
    weight: u64,
    payout_multiplier: u32,
}

struct WeightSampler {
    entries: Vec<WeightEntry>,
    cum_weights: Vec<u64>,
    total_weight: u64,
}

struct BooksIndex {
    buffer: Vec<u8>,
    id_to_range: HashMap<u32, (u32, u32)>,
}

struct ModeAssets {
    sampler: WeightSampler,
    books: BooksIndex,
}

/// Mirror of the manifest the desktop app writes when it uploads math files
/// as chunked Release assets. Lets us pull bytes from public CDN URLs
/// instead of via the GitHub Pages-hosted math/ folder.
#[derive(Debug, Clone, Deserialize)]
struct ChunkManifest {
    files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestFile {
    name: String,
    chunks: Vec<ManifestChunk>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestChunk {
    url: String,
}

#[derive(Debug, Clone, Serialize)]
struct Balance {
    amount: u64,
    currency: String,
}

#[derive(Debug, Clone, Serialize)]
struct Round {
    #[serde(rename = "betID")]
    bet_id: u64,
    amount: u64,
    payout: u64,
    #[serde(rename = "payoutMultiplier")]
    payout_multiplier: f64,
    active: bool,
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<String>,
    state: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
struct JurisdictionFlags {
    #[serde(rename = "socialCasino")]
    social_casino: bool,
    #[serde(rename = "disabledFullscreen")]
    disabled_fullscreen: bool,
    #[serde(rename = "disabledTurbo")]
    disabled_turbo: bool,
    #[serde(rename = "disabledSuperTurbo")]
    disabled_super_turbo: bool,
    #[serde(rename = "disabledAutoplay")]
    disabled_autoplay: bool,
    #[serde(rename = "disabledSlamstop")]
    disabled_slamstop: bool,
    #[serde(rename = "disabledSpacebar")]
    disabled_spacebar: bool,
    #[serde(rename = "disabledBuyFeature")]
    disabled_buy_feature: bool,
    #[serde(rename = "displayNetPosition")]
    display_net_position: bool,
    #[serde(rename = "displayRTP")]
    display_rtp: bool,
    #[serde(rename = "displaySessionTimer")]
    display_session_timer: bool,
    #[serde(rename = "minimumRoundDuration")]
    minimum_round_duration: u32,
}

#[derive(Debug, Clone, Serialize)]
struct AuthConfig {
    #[serde(rename = "gameID")]
    game_id: String,
    #[serde(rename = "minBet")]
    min_bet: u64,
    #[serde(rename = "maxBet")]
    max_bet: u64,
    #[serde(rename = "stepBet")]
    step_bet: u64,
    #[serde(rename = "defaultBetLevel")]
    default_bet_level: u64,
    #[serde(rename = "betLevels")]
    bet_levels: Vec<u64>,
    #[serde(rename = "betModes")]
    bet_modes: serde_json::Value,
    jurisdiction: JurisdictionFlags,
}

fn default_auth_config(game: &str) -> AuthConfig {
    AuthConfig {
        game_id: game.to_string(),
        min_bet: 10_000,
        max_bet: 2_000_000_000,
        step_bet: 10_000,
        default_bet_level: 200_000,
        bet_levels: vec![
            10_000,
            20_000,
            50_000,
            100_000,
            200_000,
            400_000,
            600_000,
            800_000,
            1_000_000,
            1_200_000,
            1_400_000,
            1_600_000,
            1_800_000,
            2_000_000,
            3_000_000,
            4_000_000,
            5_000_000,
            6_000_000,
            7_000_000,
            8_000_000,
            9_000_000,
            10_000_000,
            12_000_000,
            14_000_000,
            16_000_000,
            18_000_000,
            20_000_000,
            30_000_000,
            40_000_000,
            50_000_000,
            75_000_000,
            100_000_000,
            150_000_000,
            200_000_000,
            250_000_000,
            300_000_000,
            350_000_000,
            400_000_000,
            450_000_000,
            500_000_000,
            750_000_000,
            1_000_000_000,
            1_250_000_000,
            1_500_000_000,
            2_000_000_000,
        ],
        bet_modes: serde_json::json!({}),
        jurisdiction: JurisdictionFlags {
            social_casino: false,
            disabled_fullscreen: false,
            disabled_turbo: false,
            disabled_super_turbo: false,
            disabled_autoplay: false,
            disabled_slamstop: false,
            disabled_spacebar: false,
            disabled_buy_feature: false,
            display_net_position: false,
            display_rtp: true,
            display_session_timer: false,
            minimum_round_duration: 0,
        },
    }
}

// ============================================================
// PreviewEngine — exposed to JS via wasm-bindgen.
// ============================================================

#[wasm_bindgen]
pub struct PreviewEngine {
    game: String,
    base_url: String,
    config: Option<GameConfig>,
    modes: HashMap<String, ModeAssets>,
    balance: u64,
    currency: String,
    bet_id: u64,
    active_round: Option<Round>,
    rng: rand::rngs::StdRng,
    /// Optional chunk manifest. When `Some`, `loadMode` fetches weights +
    /// books by stitching chunks from the URLs in the manifest instead of
    /// hitting `<base_url>/<file>` directly. Lets us bypass GitHub Pages's
    /// 100 MB-per-file limit by hosting math on Release assets.
    manifest: Option<ChunkManifest>,
}

#[wasm_bindgen]
impl PreviewEngine {
    /// Construct a fresh engine. `game` is the game-slug used in the URL
    /// path the game's HTTP client targets. `base_url` is where the math
    /// files live (e.g. `https://user.github.io/.../math/<slug>/`).
    #[wasm_bindgen(constructor)]
    pub fn new(game: String, base_url: String) -> PreviewEngine {
        // Default starting balance — the Stake Engine demo defaults to
        // 10_000 units * API_MULTIPLIER, same as the native LGS.
        let initial_balance = 10_000 * API_MULTIPLIER;
        PreviewEngine {
            game,
            base_url: base_url.trim_end_matches('/').to_string(),
            config: None,
            modes: HashMap::new(),
            balance: initial_balance,
            currency: "USD".to_string(),
            bet_id: 1,
            active_round: None,
            rng: rand::rngs::StdRng::from_entropy(),
            manifest: None,
        }
    }

    /// Load a chunk manifest. Once set, `loadMode` fetches weights/books
    /// from the URLs listed there instead of `<base_url>/<file>`. Used by
    /// the publish flow to push math past Pages's 100 MB ceiling onto
    /// Release assets.
    #[wasm_bindgen(js_name = loadManifest)]
    pub async fn load_manifest(&mut self, url: String) -> Result<(), JsValue> {
        let text = fetch_text(&url).await?;
        let m: ChunkManifest =
            serde_json::from_str(&text).map_err(|e| js_err(&format!("parse manifest: {e}")))?;
        self.manifest = Some(m);
        Ok(())
    }

    /// Fetch + parse `index.json`. Must be called before any mode-specific
    /// request, but any number of times after that is a no-op.
    #[wasm_bindgen(js_name = loadConfig)]
    pub async fn load_config(&mut self) -> Result<(), JsValue> {
        if self.config.is_some() {
            return Ok(());
        }
        let url = format!("{}/index.json", self.base_url);
        let text = fetch_text(&url).await?;
        let cfg: GameConfig =
            serde_json::from_str(&text).map_err(|e| js_err(&format!("parse index.json: {e}")))?;
        self.config = Some(cfg);
        Ok(())
    }

    /// Fetch + parse the weights CSV and books .jsonl.zst for `mode_name`.
    /// Cached after the first call.
    #[wasm_bindgen(js_name = loadMode)]
    pub async fn load_mode(&mut self, mode_name: String) -> Result<(), JsValue> {
        self.load_config().await?;
        if self.modes.contains_key(&mode_name) {
            return Ok(());
        }
        let cfg = self
            .config
            .as_ref()
            .ok_or_else(|| js_err("config not loaded"))?;
        let mode = cfg
            .modes
            .iter()
            .find(|m| m.name == mode_name)
            .cloned()
            .ok_or_else(|| js_err(&format!("mode '{mode_name}' not found in index.json")))?;

        let weights_bytes = self.fetch_math_file(&mode.weights).await?;
        let books_bytes = self.fetch_math_file(&mode.events).await?;

        let weights_text =
            String::from_utf8(weights_bytes).map_err(|e| js_err(&format!("weights utf8: {e}")))?;
        let sampler = parse_weights(&weights_text)
            .map_err(|e| js_err(&format!("parse {}: {e}", mode.weights)))?;
        let books = decompress_and_index(&books_bytes)
            .map_err(|e| js_err(&format!("decompress {}: {e}", mode.events)))?;

        self.modes.insert(mode_name, ModeAssets { sampler, books });
        Ok(())
    }
}

impl PreviewEngine {
    /// Resolve a math file by name. Uses the chunk manifest when present
    /// (concatenating bytes from each chunk's API URL); otherwise falls
    /// back to a direct fetch from `<base_url>/<file>`.
    async fn fetch_math_file(&self, name: &str) -> Result<Vec<u8>, JsValue> {
        if let Some(m) = self.manifest.as_ref() {
            if let Some(f) = m.files.iter().find(|f| f.name == name) {
                let mut out = Vec::new();
                for chunk in &f.chunks {
                    // GitHub's `api.github.com/.../releases/assets/{id}`
                    // redirects to a CORS-enabled S3 URL when the request
                    // sets `Accept: application/octet-stream`. Without this
                    // header GitHub returns asset metadata as JSON instead.
                    let bytes =
                        fetch_bytes_with_accept(&chunk.url, "application/octet-stream").await?;
                    out.extend_from_slice(&bytes);
                }
                return Ok(out);
            }
            return Err(js_err(&format!(
                "manifest does not list '{name}' (manifest has {} files)",
                m.files.len()
            )));
        }
        let url = format!("{}/{}", self.base_url, name);
        fetch_bytes(&url).await
    }
}

#[wasm_bindgen]
impl PreviewEngine {
    /// Mirror of the RGS `/wallet/authenticate` response. Game expects a
    /// balance + AuthConfig.
    pub fn authenticate(&self) -> Result<JsValue, JsValue> {
        let resp = serde_json::json!({
            "balance": Balance { amount: self.balance, currency: self.currency.clone() },
            "round": serde_json::Value::Null,
            "config": default_auth_config(&self.game),
            "meta": serde_json::Value::Null,
        });
        // `Serializer::json_compatible()` emits plain JS objects/arrays
        // instead of `Map`/`Set`. Critical because the iframe shim does a
        // `JSON.stringify(...)` on the result, and `Map` serialises to `{}`,
        // which would then make the game read `undefined` for every field.
        use serde::Serialize;
        let s = serde_wasm_bindgen::Serializer::json_compatible();
        resp.serialize(&s).map_err(Into::into)
    }

    /// Mirror of `/wallet/balance`.
    pub fn balance(&self) -> Result<JsValue, JsValue> {
        let resp = serde_json::json!({
            "balance": Balance { amount: self.balance, currency: self.currency.clone() },
        });
        // `Serializer::json_compatible()` emits plain JS objects/arrays
        // instead of `Map`/`Set`. Critical because the iframe shim does a
        // `JSON.stringify(...)` on the result, and `Map` serialises to `{}`,
        // which would then make the game read `undefined` for every field.
        use serde::Serialize;
        let s = serde_wasm_bindgen::Serializer::json_compatible();
        resp.serialize(&s).map_err(Into::into)
    }

    /// Mirror of `/wallet/play`. Expects `mode` to already be loaded via
    /// `loadMode`. Picks a book with the same weighted-RNG algorithm as the
    /// native LGS and returns the standard PlayResponse shape.
    pub fn play(&mut self, mode: String, amount: u64) -> Result<JsValue, JsValue> {
        let cfg = self
            .config
            .as_ref()
            .ok_or_else(|| js_err("config not loaded — call loadConfig first"))?;
        let mode_def = cfg
            .modes
            .iter()
            .find(|m| m.name == mode)
            .cloned()
            .ok_or_else(|| js_err(&format!("mode '{mode}' not found")))?;
        let assets = self
            .modes
            .get(&mode)
            .ok_or_else(|| js_err(&format!("mode '{mode}' not loaded — call loadMode first")))?;

        // Settle a previous round that's still hanging around — the contract
        // expects /end-round before the next /play, but we mirror the native
        // LGS's defensive credit-pending-payout-on-double-play behaviour.
        if let Some(prev) = self.active_round.as_ref()
            && prev.payout > 0
        {
            self.balance = self.balance.saturating_add(prev.payout);
        }

        let total_cost = amount.saturating_mul(mode_def.cost);
        if total_cost > self.balance {
            return Err(js_err("insufficient balance"));
        }
        self.balance -= total_cost;

        let pick = weighted_pick(&assets.sampler, &mut self.rng);
        let state = read_event(&assets.books, pick.event_id)
            .map_err(|e| js_err(&format!("read event {}: {e}", pick.event_id)))?;
        let base_bet = total_cost / mode_def.cost.max(1);
        let payout = (base_bet.saturating_mul(pick.payout_multiplier as u64)) / 100;

        let round = Round {
            bet_id: {
                let id = self.bet_id;
                self.bet_id += 1;
                id
            },
            amount,
            payout,
            payout_multiplier: pick.payout_multiplier as f64 / 100.0,
            active: true,
            mode: mode.clone(),
            event: Some(pick.event_id.to_string()),
            state,
        };
        self.active_round = Some(round.clone());

        let resp = serde_json::json!({
            "balance": Balance { amount: self.balance, currency: self.currency.clone() },
            "round": round,
        });
        // `Serializer::json_compatible()` emits plain JS objects/arrays
        // instead of `Map`/`Set`. Critical because the iframe shim does a
        // `JSON.stringify(...)` on the result, and `Map` serialises to `{}`,
        // which would then make the game read `undefined` for every field.
        use serde::Serialize;
        let s = serde_wasm_bindgen::Serializer::json_compatible();
        resp.serialize(&s).map_err(Into::into)
    }

    /// Mirror of `/wallet/end-round`. Credits any pending payout and clears
    /// the active round.
    #[wasm_bindgen(js_name = endRound)]
    pub fn end_round(&mut self) -> Result<JsValue, JsValue> {
        if let Some(round) = self.active_round.take()
            && round.payout > 0
        {
            self.balance = self.balance.saturating_add(round.payout);
        }
        let resp = serde_json::json!({
            "balance": Balance { amount: self.balance, currency: self.currency.clone() },
            "round": serde_json::Value::Null,
            "config": default_auth_config(&self.game),
            "meta": serde_json::Value::Null,
        });
        // `Serializer::json_compatible()` emits plain JS objects/arrays
        // instead of `Map`/`Set`. Critical because the iframe shim does a
        // `JSON.stringify(...)` on the result, and `Map` serialises to `{}`,
        // which would then make the game read `undefined` for every field.
        use serde::Serialize;
        let s = serde_wasm_bindgen::Serializer::json_compatible();
        resp.serialize(&s).map_err(Into::into)
    }

    /// List the modes declared in `index.json`. Useful for the harness UI to
    /// pre-load every mode upfront.
    #[wasm_bindgen(js_name = listModes)]
    pub fn list_modes(&self) -> Result<JsValue, JsValue> {
        let names: Vec<&str> = self
            .config
            .as_ref()
            .map(|c| c.modes.iter().map(|m| m.name.as_str()).collect())
            .unwrap_or_default();
        use serde::Serialize;
        let s = serde_wasm_bindgen::Serializer::json_compatible();
        names.serialize(&s).map_err(Into::into)
    }
}

// ============================================================
// Internals — adapted from `lgs::math_engine` so the preview produces
// byte-identical books for a given event id.
// ============================================================

fn parse_weights(text: &str) -> Result<WeightSampler, String> {
    let mut entries = Vec::with_capacity(1024);
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split(',');
        let event_id = it
            .next()
            .ok_or_else(|| format!("line {lineno}: missing eventId"))?
            .trim()
            .parse::<u32>()
            .map_err(|e| format!("line {lineno}: eventId: {e}"))?;
        let weight = it
            .next()
            .ok_or_else(|| format!("line {lineno}: missing weight"))?
            .trim()
            .parse::<u64>()
            .map_err(|e| format!("line {lineno}: weight: {e}"))?;
        let payout_multiplier = it
            .next()
            .ok_or_else(|| format!("line {lineno}: missing payout"))?
            .trim()
            .parse::<u32>()
            .map_err(|e| format!("line {lineno}: payout: {e}"))?;
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
            .ok_or_else(|| "total weight overflow".to_string())?;
        cum_weights.push(total);
    }
    Ok(WeightSampler {
        entries,
        cum_weights,
        total_weight: total,
    })
}

fn decompress_and_index(compressed: &[u8]) -> Result<BooksIndex, String> {
    let mut decoder =
        ruzstd::StreamingDecoder::new(compressed).map_err(|e| format!("zstd init: {e}"))?;
    let mut buffer = Vec::with_capacity(compressed.len() * 4);
    decoder
        .read_to_end(&mut buffer)
        .map_err(|e| format!("zstd decode: {e}"))?;

    let mut id_to_range = HashMap::with_capacity(buffer.len() / 512 + 1);
    let mut stream =
        serde_json::Deserializer::from_slice(&buffer).into_iter::<serde::de::IgnoredAny>();
    while let Some(item) = {
        let start = stream.byte_offset();
        stream.next().map(|item| (start, item))
    } {
        let (start, item) = item;
        item.map_err(|e| format!("books json stream at byte {start}: {e}"))?;
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

fn weighted_pick(sampler: &WeightSampler, rng: &mut rand::rngs::StdRng) -> WeightEntry {
    let pick = rng.gen_range(0..sampler.total_weight);
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

fn read_event(idx: &BooksIndex, event_id: u32) -> Result<serde_json::Value, String> {
    let &(start, end) = idx
        .id_to_range
        .get(&event_id)
        .ok_or_else(|| format!("event {event_id} not found"))?;
    let slice = &idx.buffer[start as usize..end as usize];
    let line_str = std::str::from_utf8(slice).map_err(|e| format!("event {event_id} utf8: {e}"))?;

    // Same shape transformation as the native LGS: if the line is a Book
    // wrapper with an `events` array, return that array; otherwise return
    // the whole object.
    let parsed: serde_json::Value =
        serde_json::from_str(line_str).map_err(|e| format!("event {event_id} json: {e}"))?;
    let out = parsed.get("events").cloned().unwrap_or(parsed);
    Ok(out)
}

// ============================================================
// HTTP helpers
// ============================================================

async fn fetch_text(url: &str) -> Result<String, JsValue> {
    let resp = gloo_net::http::Request::get(url)
        .send()
        .await
        .map_err(|e| js_err(&format!("fetch {url}: {e}")))?;
    if !resp.ok() {
        return Err(js_err(&format!(
            "fetch {url}: {} {}",
            resp.status(),
            resp.status_text()
        )));
    }
    resp.text()
        .await
        .map_err(|e| js_err(&format!("read {url}: {e}")))
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, JsValue> {
    let resp = gloo_net::http::Request::get(url)
        .send()
        .await
        .map_err(|e| js_err(&format!("fetch {url}: {e}")))?;
    if !resp.ok() {
        return Err(js_err(&format!(
            "fetch {url}: {} {}",
            resp.status(),
            resp.status_text()
        )));
    }
    resp.binary()
        .await
        .map_err(|e| js_err(&format!("read {url}: {e}")))
}

async fn fetch_bytes_with_accept(url: &str, accept: &str) -> Result<Vec<u8>, JsValue> {
    let resp = gloo_net::http::Request::get(url)
        .header("Accept", accept)
        .send()
        .await
        .map_err(|e| js_err(&format!("fetch {url}: {e}")))?;
    if !resp.ok() {
        return Err(js_err(&format!(
            "fetch {url}: {} {}",
            resp.status(),
            resp.status_text()
        )));
    }
    resp.binary()
        .await
        .map_err(|e| js_err(&format!("read {url}: {e}")))
}

fn js_err(msg: &str) -> JsValue {
    JsValue::from_str(msg)
}
