//! Terminal output: upload progress and small aligned tables.
//!
//! Two progress modes exist. On an interactive terminal (and unless
//! `--no-progress`) uploads render as an [`indicatif`] `MultiProgress`: a
//! bytes/sec bar for large books, a spinner for small files. Otherwise output
//! degrades to plain log lines, which is what CI captures. All human output
//! goes to stderr; stdout is reserved for machine-readable results.

use std::io::IsTerminal;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use crate::api::{
    FileDiff, GameInfo, RevisionDetail, RevisionSummary, ShareLinkView, StatsDiff, WorkspaceInfo,
};
use crate::hash::HashedFile;

/// Files at or above this size get a byte-rate progress bar; smaller ones get a
/// spinner (their transfer is too quick for a bar to be meaningful).
const BIG_FILE: u64 = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------------

/// Drives all progress output for a command.
pub struct Reporter {
    mode: Mode,
}

enum Mode {
    /// Interactive: bars and spinners multiplexed on one terminal region.
    Fancy(MultiProgress),
    /// Non-interactive or `--no-progress`: plain log lines to stderr. Tests use
    /// this mode too (its output is captured by the test harness).
    Plain,
    /// Fully silent: no progress and no log lines. Used by the `mcp` server so a
    /// reused push/pull never writes to the process's stdout/stderr streams.
    Quiet,
}

impl Reporter {
    /// Chooses fancy output when stderr is a TTY and progress isn't disabled.
    pub fn new(no_progress: bool) -> Self {
        let fancy = !no_progress && std::io::stderr().is_terminal();
        let mode = if fancy {
            Mode::Fancy(MultiProgress::new())
        } else {
            Mode::Plain
        };
        Self { mode }
    }

    /// A reporter that emits nothing at all (for the `mcp` server).
    pub fn quiet() -> Self {
        Self { mode: Mode::Quiet }
    }

    /// Prints a block of human text, safely interleaved above any live bars.
    pub fn println(&self, msg: &str) {
        match &self.mode {
            Mode::Fancy(mp) => {
                let _ = mp.println(msg);
            }
            Mode::Plain => eprintln!("{msg}"),
            Mode::Quiet => {}
        }
    }

    /// Starts a progress handle for one file transfer (upload or download).
    pub fn start_file(&self, name: &str, size: u64, dir: Transfer) -> FileTask {
        match &self.mode {
            Mode::Fancy(mp) => {
                let pb = if size >= BIG_FILE {
                    let pb = mp.add(ProgressBar::new(size));
                    pb.set_style(bar_style());
                    pb
                } else {
                    let pb = mp.add(ProgressBar::new_spinner());
                    pb.set_style(spinner_style());
                    pb.enable_steady_tick(Duration::from_millis(100));
                    pb
                };
                pb.set_prefix(name.to_string());
                FileTask {
                    kind: TaskKind::Bar(pb),
                }
            }
            Mode::Plain => {
                eprintln!("  {} {name} ({})", dir.ing(), human_bytes(size));
                FileTask {
                    kind: TaskKind::Plain {
                        name: name.to_string(),
                        dir,
                    },
                }
            }
            Mode::Quiet => FileTask {
                kind: TaskKind::Hidden,
            },
        }
    }

    /// Starts an indeterminate spinner for a phase such as hashing.
    pub fn spinner(&self, msg: &str) -> SpinnerHandle {
        match &self.mode {
            Mode::Fancy(mp) => {
                let pb = mp.add(ProgressBar::new_spinner());
                pb.set_style(msg_spinner_style());
                pb.set_message(msg.to_string());
                pb.enable_steady_tick(Duration::from_millis(100));
                SpinnerHandle { bar: Some(pb) }
            }
            Mode::Plain => {
                eprintln!("{msg}…");
                SpinnerHandle { bar: None }
            }
            Mode::Quiet => SpinnerHandle { bar: None },
        }
    }
}

/// Direction of a file transfer, so plain-mode log lines read correctly for
/// both `push` (upload) and `pull` (download).
#[derive(Clone, Copy)]
pub enum Transfer {
    Upload,
    Download,
}

impl Transfer {
    fn ing(self) -> &'static str {
        match self {
            Transfer::Upload => "uploading",
            Transfer::Download => "downloading",
        }
    }

    fn ed(self) -> &'static str {
        match self {
            Transfer::Upload => "uploaded",
            Transfer::Download => "downloaded",
        }
    }
}

/// Progress handle for a single transfer, owned by the orchestrator so it can
/// report the terminal outcome after the transfer future resolves.
pub struct FileTask {
    kind: TaskKind,
}

enum TaskKind {
    Bar(ProgressBar),
    Plain { name: String, dir: Transfer },
    Hidden,
}

impl FileTask {
    /// A cheap, cloneable byte-counter fed into the upload stream. Clones share
    /// the same underlying bar, so `reset` on retry affects every copy. Plain
    /// and hidden modes have no bar to advance, so they hand back a no-op.
    pub fn progress(&self) -> FileProgress {
        match &self.kind {
            TaskKind::Bar(pb) => FileProgress {
                bar: Some(pb.clone()),
            },
            TaskKind::Plain { .. } | TaskKind::Hidden => FileProgress::hidden(),
        }
    }

    pub fn finish_success(&self, size: u64) {
        match &self.kind {
            TaskKind::Bar(pb) => pb.finish_with_message(format!("done ({})", human_bytes(size))),
            TaskKind::Plain { name, dir } => {
                eprintln!("  {} {name} ({})", dir.ed(), human_bytes(size))
            }
            TaskKind::Hidden => {}
        }
    }

    pub fn finish_error(&self, msg: &str) {
        match &self.kind {
            TaskKind::Bar(pb) => pb.abandon_with_message(format!("failed: {msg}")),
            TaskKind::Plain { name, .. } => eprintln!("  FAILED {name}: {msg}"),
            TaskKind::Hidden => {}
        }
    }
}

/// The byte-counter side of a [`FileTask`], passed into the streaming upload.
#[derive(Clone)]
pub struct FileProgress {
    bar: Option<ProgressBar>,
}

impl FileProgress {
    /// A no-op counter, used in plain mode (no bar to advance) and tests.
    pub fn hidden() -> Self {
        Self { bar: None }
    }

    /// Advances the bar by `n` transferred bytes.
    pub fn inc(&self, n: u64) {
        if let Some(bar) = &self.bar {
            bar.inc(n);
        }
    }

    /// Rewinds to zero before a retry so the bar doesn't overshoot.
    pub fn reset(&self) {
        if let Some(bar) = &self.bar {
            bar.set_position(0);
        }
    }
}

/// Handle for a phase spinner; call [`SpinnerHandle::finish`] when done.
pub struct SpinnerHandle {
    bar: Option<ProgressBar>,
}

impl SpinnerHandle {
    pub fn finish(self, msg: &str) {
        if let Some(bar) = self.bar {
            bar.finish_with_message(msg.to_string());
        }
    }
}

fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "  {prefix} [{bar:24.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
    )
    .unwrap_or_else(|_| ProgressStyle::default_bar())
    .progress_chars("=>-")
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("  {spinner:.green} {prefix} ({bytes})")
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
}

fn msg_spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("  {spinner:.green} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
}

// ---------------------------------------------------------------------------
// Tables and formatting
// ---------------------------------------------------------------------------

/// Pre-upload summary for a math push: file count, total size, largest file.
pub fn push_summary(files: &[HashedFile]) -> String {
    folder_summary("Math folder", files)
}

/// Pre-upload summary for a front-bundle push.
pub fn front_summary(files: &[HashedFile]) -> String {
    folder_summary("Front bundle", files)
}

/// Shared pre-upload summary: `<label>: N file(s), <size> total` + largest file.
fn folder_summary(label: &str, files: &[HashedFile]) -> String {
    let total: u64 = files.iter().map(|f| f.size).sum();
    let mut s = format!(
        "{label}: {} file(s), {} total",
        files.len(),
        human_bytes(total)
    );
    if let Some(largest) = files.iter().max_by_key(|f| f.size) {
        s.push_str(&format!(
            "\nLargest:     {} ({})",
            largest.rel_path,
            human_bytes(largest.size)
        ));
    }
    s
}

/// Post-commit recap: revision number and upload/dedup byte accounting.
pub fn push_recap(
    number: i64,
    total_files: usize,
    uploaded_count: usize,
    uploaded_bytes: u64,
    total_bytes: u64,
) -> String {
    let deduped = total_bytes.saturating_sub(uploaded_bytes);
    format!(
        "Revision #{number} created.\nUploaded {uploaded_count} of {total_files} file(s), {} sent, {} deduplicated.",
        human_bytes(uploaded_bytes),
        human_bytes(deduped),
    )
}

/// Post-commit recap for a front-bundle push: bundle id + upload/dedup bytes.
pub fn front_recap(
    id: &str,
    total_files: usize,
    uploaded_count: usize,
    uploaded_bytes: u64,
    total_bytes: u64,
) -> String {
    let deduped = total_bytes.saturating_sub(uploaded_bytes);
    format!(
        "Front bundle {id} created.\nUploaded {uploaded_count} of {total_files} file(s), {} sent, {} deduplicated.",
        human_bytes(uploaded_bytes),
        human_bytes(deduped),
    )
}

/// Renders the `sdt share list` table: slug, url, revision pin, counters,
/// observed RTP, live sessions, and status.
pub fn shares_table(shares: &[ShareLinkView]) -> String {
    if shares.is_empty() {
        return "No share links yet.".to_string();
    }
    let headers = [
        "SLUG", "URL", "REV", "SESSIONS", "SPINS", "RTP", "ACTIVE", "STATUS",
    ];
    let rows: Vec<Vec<String>> = shares
        .iter()
        .map(|s| {
            vec![
                s.slug.clone(),
                s.url.clone().unwrap_or_else(|| "-".into()),
                s.revision_number
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "latest".into()),
                s.sessions_count.to_string(),
                s.spins_count.to_string(),
                s.observed_rtp
                    .map(fmt_ratio_pct)
                    .unwrap_or_else(|| "-".into()),
                s.active_sessions.to_string(),
                share_status(s),
            ]
        })
        .collect();
    render_table(&headers, &rows)
}

/// The settings recap printed after `sdt share create`.
pub fn share_settings(s: &ShareLinkView) -> String {
    let rev = s
        .revision_number
        .map(|n| format!("#{n}"))
        .unwrap_or_else(|| "latest".into());
    let bundle = s.front_bundle_id.clone().unwrap_or_else(|| "latest".into());
    let expires = s.expires_at.clone().unwrap_or_else(|| "never".into());
    let password = if s.password_protected { "yes" } else { "no" };
    format!(
        "Settings:\n  \
         revision:     {rev}\n  \
         bundle:       {bundle}\n  \
         password:     {password}\n  \
         expires:      {expires}\n  \
         max sessions: {}",
        s.max_concurrent_sessions
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into()),
    )
}

/// A share link's coarse status: revoked wins over expired wins over ok.
fn share_status(s: &ShareLinkView) -> String {
    if s.revoked_at.is_some() {
        return "revoked".into();
    }
    if let Some(exp) = &s.expires_at
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(exp)
        && dt.with_timezone(&chrono::Utc) <= chrono::Utc::now()
    {
        return "expired".into();
    }
    "ok".into()
}

/// Formats a ratio (e.g. observed RTP `0.965`) as a 2-dp percentage.
fn fmt_ratio_pct(r: f64) -> String {
    format!("{:.2}%", r * 100.0)
}

/// Renders the `sdt revisions` table.
pub fn revisions_table(revs: &[RevisionSummary]) -> String {
    if revs.is_empty() {
        return "No revisions yet.".to_string();
    }
    let headers = ["#", "AGE", "AUTHOR", "FILES", "SIZE", "STATS", "MESSAGE"];
    let rows: Vec<Vec<String>> = revs
        .iter()
        .map(|r| {
            vec![
                r.number.to_string(),
                format_age(r.created_at.as_deref()),
                r.author_display_name.clone().unwrap_or_else(|| "-".into()),
                r.files_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "-".into()),
                r.total_size
                    .map(|n| human_bytes(n.max(0) as u64))
                    .unwrap_or_else(|| "-".into()),
                stats_badge(r.stats_status.as_deref()),
                truncate(r.message.as_deref().unwrap_or(""), 48),
            ]
        })
        .collect();
    render_table(&headers, &rows)
}

/// Renders the per-mode bet-stats table (or the reason there isn't one).
pub fn stats_table(detail: &RevisionDetail) -> String {
    let Some(stats) = &detail.stats else {
        return "No stats available.".to_string();
    };
    if stats.status == "error" {
        return format!(
            "Stats failed: {}",
            stats.error.as_deref().unwrap_or("unknown error")
        );
    }
    if stats.modes.is_empty() {
        return format!("Stats status: {} (no modes reported).", stats.status);
    }
    let headers = ["MODE", "COST", "RTP", "MAX WIN"];
    let rows: Vec<Vec<String>> = stats
        .modes
        .iter()
        .map(|m| {
            vec![
                m.mode.clone(),
                m.cost.map(fmt_num).unwrap_or_else(|| "-".into()),
                m.rtp.map(fmt_rtp).unwrap_or_else(|| "-".into()),
                m.max_win
                    .map(|x| format!("{}x", fmt_num(x)))
                    .unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    render_table(&headers, &rows)
}

/// Renders the `sdt workspaces` table.
pub fn workspaces_table(workspaces: &[WorkspaceInfo]) -> String {
    if workspaces.is_empty() {
        return "No workspaces.".to_string();
    }
    let headers = ["SLUG", "NAME", "ROLE"];
    let rows: Vec<Vec<String>> = workspaces
        .iter()
        .map(|w| {
            vec![
                w.slug.clone(),
                w.name.clone().unwrap_or_else(|| "-".into()),
                w.role.clone().unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    render_table(&headers, &rows)
}

/// Renders the `sdt games` table.
pub fn games_table(games: &[GameInfo]) -> String {
    if games.is_empty() {
        return "No games.".to_string();
    }
    let headers = ["SLUG", "NAME", "HEAD", "REVISIONS"];
    let rows: Vec<Vec<String>> = games
        .iter()
        .map(|g| {
            vec![
                g.slug.clone(),
                g.name.clone().unwrap_or_else(|| "-".into()),
                g.head_number
                    .map(|n| format!("#{n}"))
                    .unwrap_or_else(|| "-".into()),
                g.revisions_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    render_table(&headers, &rows)
}

/// True when ANSI colour should be used: stderr is a TTY and `NO_COLOR` is
/// unset. The render functions take an explicit `color` flag so tests stay
/// deterministic; commands pass this at the call site.
pub fn colors_enabled() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn paint(s: &str, code: &str, on: bool) -> String {
    if on {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// One-line file-change summary: `+added  -removed  ~changed  =unchanged`.
pub fn diff_files_summary(files: &FileDiff, color: bool) -> String {
    let added = paint(&format!("+{} added", files.added.len()), "32", color);
    let removed = paint(&format!("-{} removed", files.removed.len()), "31", color);
    let changed = paint(&format!("~{} changed", files.changed.len()), "33", color);
    let unchanged = paint(&format!("={} unchanged", files.unchanged), "2", color);
    format!("Files: {added}  {removed}  {changed}  {unchanged}")
}

/// Treats an RTP `<= 1.5` as a fraction (0.965 -> 96.5), else already a percent.
fn to_percent(r: f64) -> f64 {
    if r <= 1.5 { r * 100.0 } else { r }
}

/// Per-mode RTP diff table: before/after RTP and a signed Δ in percentage
/// points, the Δ column coloured (green up, red down) when `color` is set.
///
/// The Δ is the last column, so wrapping it in ANSI never disturbs alignment
/// (only preceding, padded columns must stay ANSI-free for widths to be right).
pub fn diff_stats_table(diff: &StatsDiff, before_num: i64, after_num: i64, color: bool) -> String {
    if diff.modes.is_empty() {
        return "No comparable mode stats (one or both revisions lack ok stats).".to_string();
    }
    let headers = [
        "MODE".to_string(),
        format!("RTP #{before_num}"),
        format!("RTP #{after_num}"),
        "Δ pp".to_string(),
    ];

    // Plain cells drive the column widths; the Δ is coloured only at print time.
    struct Row {
        cells: [String; 4],
        delta_sign: i8,
    }
    let rows: Vec<Row> = diff
        .modes
        .iter()
        .map(|m| {
            let before = m.before.as_ref().and_then(|s| s.rtp);
            let after = m.after.as_ref().and_then(|s| s.rtp);
            let before_cell = before.map(fmt_rtp).unwrap_or_else(|| "-".into());
            let after_cell = after.map(fmt_rtp).unwrap_or_else(|| "-".into());
            let (delta_cell, sign) = match (before, after) {
                (Some(b), Some(a)) => {
                    let d = to_percent(a) - to_percent(b);
                    let sign = if d > 1e-9 {
                        1
                    } else if d < -1e-9 {
                        -1
                    } else {
                        0
                    };
                    (format!("{d:+.2}pp"), sign)
                }
                _ => ("-".into(), 0),
            };
            Row {
                cells: [m.mode.clone(), before_cell, after_cell, delta_cell],
                delta_sign: sign,
            }
        })
        .collect();

    // Column widths from plain header + cell text.
    let mut widths = [0usize; 4];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.chars().count();
    }
    for row in &rows {
        for (i, cell) in row.cells.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    let mut out = String::new();
    // Header + underline (plain).
    push_diff_row(&mut out, &headers, &widths, None);
    let underline: [String; 4] = std::array::from_fn(|i| "-".repeat(widths[i]));
    push_diff_row(&mut out, &underline, &widths, None);
    for row in &rows {
        let code = match row.delta_sign {
            1 => Some("32"),
            -1 => Some("31"),
            _ => None,
        };
        let code = if color { code } else { None };
        push_diff_row(&mut out, &row.cells, &widths, code);
    }
    out.trim_end().to_string()
}

/// Emits one row of the diff table. The last cell (Δ) is not padded, so an
/// optional ANSI `color_last` never affects alignment.
fn push_diff_row(
    out: &mut String,
    cells: &[String; 4],
    widths: &[usize; 4],
    color_last: Option<&str>,
) {
    let mut line = String::new();
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            line.push_str("  ");
        }
        if i == cells.len() - 1 {
            // Last column: colour if asked, never pad.
            match color_last {
                Some(code) => line.push_str(&paint(cell, code, true)),
                None => line.push_str(cell),
            }
        } else {
            line.push_str(cell);
            let pad = widths[i].saturating_sub(cell.chars().count());
            line.push_str(&" ".repeat(pad));
        }
    }
    out.push_str(line.trim_end());
    out.push('\n');
}

fn stats_badge(status: Option<&str>) -> String {
    match status {
        Some("ok") => "ok".into(),
        Some("pending") => "pending".into(),
        Some("error") => "error".into(),
        Some(other) => other.to_string(),
        None => "-".into(),
    }
}

/// Formats a byte count with binary units (1024-based).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Turns an RFC 3339 timestamp into a coarse relative age (`5m`, `3h`, `2d`).
pub fn format_age(ts: Option<&str>) -> String {
    let Some(s) = ts else { return "-".into() };
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) else {
        return "-".into();
    };
    let secs = (chrono::Utc::now() - dt.with_timezone(&chrono::Utc)).num_seconds();
    if secs < 0 {
        "just now".into()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Formats a number as an integer when whole, else two decimals.
fn fmt_num(x: f64) -> String {
    if (x.fract()).abs() < 1e-9 {
        format!("{x:.0}")
    } else {
        format!("{x:.2}")
    }
}

/// Formats RTP as a percentage. Values <= 1.5 are treated as fractions (0.965
/// -> 96.50%); larger values are assumed to already be percentages.
fn fmt_rtp(r: f64) -> String {
    let pct = if r <= 1.5 { r * 100.0 } else { r };
    format!("{pct:.2}%")
}

/// Truncates to `max` characters, appending an ellipsis when cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Renders headers + rows as a left-aligned, two-space-gutter table.
fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }

    let header_cells: Vec<String> = headers.iter().map(|h| (*h).to_string()).collect();
    let underline: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();

    let mut out = String::new();
    push_row(&mut out, &header_cells, &widths);
    push_row(&mut out, &underline, &widths);
    for row in rows {
        push_row(&mut out, row, &widths);
    }
    out.trim_end().to_string()
}

fn push_row(out: &mut String, cells: &[String], widths: &[usize]) {
    let mut line = String::new();
    for (i, width) in widths.iter().enumerate() {
        if i > 0 {
            line.push_str("  ");
        }
        let cell = cells.get(i).map(String::as_str).unwrap_or("");
        line.push_str(cell);
        // Pad every column but the last so trailing whitespace never appears.
        if i < widths.len() - 1 {
            let pad = width.saturating_sub(cell.chars().count());
            line.push_str(&" ".repeat(pad));
        }
    }
    out.push_str(line.trim_end());
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_scales_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn rtp_handles_fraction_and_percent() {
        assert_eq!(fmt_rtp(0.965), "96.50%");
        assert_eq!(fmt_rtp(96.5), "96.50%");
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
    }

    #[test]
    fn table_aligns_columns() {
        let table = render_table(&["A", "BB"], &[vec!["xxx".into(), "y".into()]]);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines[0], "A    BB");
        assert_eq!(lines[2], "xxx  y");
    }

    fn share(slug: &str) -> ShareLinkView {
        ShareLinkView {
            id: format!("id-{slug}"),
            slug: slug.into(),
            url: Some(format!("https://{slug}.play.example.com/")),
            game: Some("g".into()),
            revision_number: None,
            front_bundle_id: None,
            password_protected: false,
            expires_at: None,
            max_concurrent_sessions: Some(25),
            revoked_at: None,
            created_at: None,
            sessions_count: 0,
            spins_count: 0,
            total_bet: 0.0,
            total_win: 0.0,
            observed_rtp: None,
            active_sessions: 0,
            extra: Default::default(),
        }
    }

    #[test]
    fn shares_table_renders_canned_json() {
        // A pinned, active link with observed RTP…
        let pinned = ShareLinkView {
            revision_number: Some(12),
            sessions_count: 4,
            spins_count: 100,
            observed_rtp: Some(0.9612),
            active_sessions: 2,
            ..share("cool-otter-42")
        };
        // …a latest-tracking link with no plays yet…
        let latest = share("brave-lynx-7");
        // …and a revoked one.
        let revoked = ShareLinkView {
            revoked_at: Some("2020-01-01T00:00:00Z".into()),
            ..share("old-fox-1")
        };

        let table = shares_table(&[pinned, latest, revoked]);
        let lines: Vec<&str> = table.lines().collect();

        // Header carries every column; the underline separates it from rows.
        for col in [
            "SLUG", "URL", "REV", "SESSIONS", "SPINS", "RTP", "ACTIVE", "STATUS",
        ] {
            assert!(lines[0].contains(col), "header missing {col}");
        }
        // Pinned row: revision number, RTP as a 2-dp percent, ok status.
        assert!(lines[2].contains("cool-otter-42"));
        assert!(lines[2].contains("12"));
        assert!(lines[2].contains("96.12%"));
        assert!(lines[2].trim_end().ends_with("ok"));
        // Latest-tracking: "latest" rev, "-" RTP.
        assert!(lines[3].contains("brave-lynx-7"));
        assert!(lines[3].contains("latest"));
        // Revoked shows its status.
        assert!(lines[4].trim_end().ends_with("revoked"));
    }

    #[test]
    fn share_status_flags_expiry() {
        let expired = ShareLinkView {
            expires_at: Some("2000-01-01T00:00:00Z".into()),
            ..share("s")
        };
        assert_eq!(share_status(&expired), "expired");
        let future = ShareLinkView {
            expires_at: Some("2999-01-01T00:00:00Z".into()),
            ..share("s")
        };
        assert_eq!(share_status(&future), "ok");
    }

    #[test]
    fn share_settings_lists_latest_defaults() {
        let text = share_settings(&share("s"));
        assert!(text.contains("revision:     latest"));
        assert!(text.contains("bundle:       latest"));
        assert!(text.contains("password:     no"));
        assert!(text.contains("expires:      never"));
        assert!(text.contains("max sessions: 25"));
    }
}
