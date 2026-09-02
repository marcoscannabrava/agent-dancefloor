//! Types shared across the app, and every fixed limit in one place.

use std::path::PathBuf;

/// Upper bounds. Each read path is capped so a runaway file or a directory full
/// of stale entries cannot stall a redraw.
pub const SESSIONS_MAX: usize = 256;
pub const SUBAGENTS_MAX: usize = 64;
pub const TRANSCRIPT_TAIL_BYTES_MAX: u64 = 1024 * 1024;
pub const TRANSCRIPT_LINES_MAX: usize = 4096;
pub const PROMPT_CHARS_MAX: usize = 4000;
/// How far back to look for the last human prompt once the tail has been read,
/// and the chunk size that search steps in.
pub const PROMPT_SEARCH_BYTES_MAX: u64 = 16 * 1024 * 1024;
pub const PROMPT_SEARCH_CHUNK_BYTES: u64 = 1024 * 1024;
/// The Activity pane scrolls, so it keeps every tool call in the tail. This is
/// only a ceiling against a pathological file, not the working size.
pub const TOOL_CALLS_MAX: usize = 2048;
pub const FILES_EDITED_MAX: usize = 12;
/// A held command has to stay copyable in full, so this cap is set well past any
/// real one. The pane cuts again to whatever one row fits.
pub const TOOL_DETAIL_CHARS_MAX: usize = 8000;

/// The model id Claude Code writes on assistant messages it generated locally,
/// such as "No response requested." after an interrupt. They carry all-zero
/// usage, so they must not be read as the session's real state.
pub const MODEL_SYNTHETIC: &str = "<synthetic>";

/// Context windows Claude Code actually ships. Assistant messages record the
/// base model id (`claude-opus-5`) even on the `[1m]` variant, so the window has
/// to come from `cost-state` lines, settings, or observed usage.
pub const CONTEXT_LIMIT_STANDARD: u64 = 200_000;
pub const CONTEXT_LIMIT_LONG: u64 = 1_000_000;

/// The suffix Claude Code puts on a long-context model id.
pub const LONG_CONTEXT_SUFFIX: &str = "[1m]";

/// Declaration order is the sort order: a session waiting on the user is the
/// one that needs attention, so it sorts above everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Busy,
    Idle,
    Other,
}

impl Status {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "waiting" => Status::Waiting,
            "busy" => Status::Busy,
            "idle" => Status::Idle,
            _ => Status::Other,
        }
    }

    /// Two cells wide for a waiting session, because a circle alone is too easy
    /// to miss in a column of dots; the question mark is what carries the state.
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Waiting => "●?",
            Status::Busy => "●",
            Status::Idle => "○",
            Status::Other => "·",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Waiting => "waiting",
            Status::Busy => "busy",
            Status::Idle => "idle",
            Status::Other => "?",
        }
    }

    /// The long form, for the detail pane where there is room to say what the
    /// session is waiting on.
    pub fn description(self) -> &'static str {
        match self {
            Status::Waiting => "waiting for input",
            other => other.label(),
        }
    }
}

/// One line of `ps` output for a live session process.
#[derive(Debug, Clone)]
pub struct ProcStat {
    pub rss_kib: u64,
    pub cpu_percent: f64,
}

/// Token counts from the most recent assistant message.
#[derive(Debug, Clone, Default)]
pub struct ContextUsage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub output: u64,
}

impl ContextUsage {
    /// What the next request will carry: everything already in the window plus
    /// the reply just written into it.
    pub fn total(&self) -> u64 {
        self.input + self.cache_read + self.cache_creation + self.output
    }
}

/// Totals accumulated over the parsed tail, not the whole session. The tail is
/// capped, so these describe recent activity rather than a lifetime bill.
#[derive(Debug, Clone, Default)]
pub struct TailTotals {
    pub assistant_messages: usize,
    pub user_messages: usize,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub cache_creation_tokens: u64,
    pub web_searches: u64,
}

/// Whole-session accounting, as Claude Code records it. Unlike `TailTotals`
/// these cover every turn, not the parsed tail.
#[derive(Debug, Clone, Default)]
pub struct CostState {
    pub cost_usd: f64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub api_ms: u64,
    pub api_ms_without_retries: u64,
    pub tool_ms: u64,
    pub total_ms: u64,
    /// Billed ids keep the `[1m]` suffix, so this is the one record of the
    /// model variant the session runs. Highest cost first.
    pub models: Vec<ModelCost>,
}

#[derive(Debug, Clone)]
pub struct ModelCost {
    pub id: String,
    pub cost_usd: f64,
}

impl CostState {
    /// Was `model` billed against the long window? Only `cost-state` names the
    /// `[1m]` variant; assistant messages carry the base id.
    pub fn billed_long(&self, model: &str) -> bool {
        self.models
            .iter()
            .any(|billed| billed.id.strip_suffix(LONG_CONTEXT_SUFFIX) == Some(model))
    }
}

/// What is steering the newest turn, when anything is. A skill frames a whole
/// turn and an MCP tool is one call inside it, so the two never both apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Driver {
    Skill(String),
    McpTool(String),
}

impl Driver {
    pub fn label(&self) -> String {
        match self {
            Driver::Skill(name) => format!("skill {name}"),
            Driver::McpTool(name) => format!("mcp {name}"),
        }
    }
}

/// A finished turn, as Claude Code measured it.
#[derive(Debug, Clone, Copy)]
pub struct Turn {
    pub duration_ms: u64,
    pub messages: u64,
}

/// One tool the session ran. A Bash call carries both a written summary and the
/// command it summarises, and the pane shows both, so the two are kept apart.
/// Tools with no summary leave it empty and say everything in `detail`.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub summary: String,
    pub detail: String,
}

/// What the session is doing, as opposed to what it is configured as. Empty is
/// a normal state: a session that has only just started has none of this yet.
#[derive(Debug, Clone, Default)]
pub struct Activity {
    /// The recap Claude writes when the user walks away. A `/config` toggle, so
    /// most sessions never have one.
    pub recap: Option<String>,
    pub driver: Option<Driver>,
    /// The turn that just ended, not the one running now.
    pub last_turn: Option<Turn>,
    /// Oldest first, so the pane can read it back newest first. Every call in
    /// the tail is kept; the pane scrolls rather than cutting the list.
    pub tools: Vec<ToolCall>,
    /// Files the session edited, each listed once, oldest edit first.
    pub files: Vec<PathBuf>,
}

impl Activity {
    pub fn is_empty(&self) -> bool {
        self.recap.is_none()
            && self.driver.is_none()
            && self.last_turn.is_none()
            && self.tools.is_empty()
            && self.files.is_empty()
    }

    /// Newest wins, and a repeat edit moves the file up rather than doubling it.
    pub fn record_file(&mut self, path: PathBuf) {
        self.files.retain(|seen| *seen != path);
        self.files.push(path);
        if self.files.len() > FILES_EDITED_MAX {
            self.files.remove(0);
        }
    }

    pub fn record_tool(&mut self, call: ToolCall) {
        self.tools.push(call);
        if self.tools.len() > TOOL_CALLS_MAX {
            self.tools.remove(0);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub name: String,
    pub branch: String,
    pub original_branch: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub repository: String,
}

#[derive(Debug, Clone)]
pub struct Subagent {
    pub name: String,
    pub agent_type: String,
    pub description: String,
    pub spawn_depth: u64,
    pub age_secs: Option<u64>,
    pub bytes: u64,
}

/// Everything recovered from a session's transcript file.
#[derive(Debug, Clone, Default)]
pub struct Detail {
    pub transcript: Option<PathBuf>,
    pub transcript_age_secs: Option<u64>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
    pub usage: Option<ContextUsage>,
    /// Highest total seen in the tail. Drives the context-limit inference, and
    /// survives the dip that compaction puts in the latest message.
    pub usage_peak: u64,
    pub totals: TailTotals,
    /// The newest `cost-state` line. A session that has run no turn yet has
    /// written none.
    pub cost: Option<CostState>,
    pub title: Option<String>,
    pub git_branch: Option<String>,
    pub permission_mode: Option<String>,
    pub mode: Option<String>,
    pub worktree: Option<Worktree>,
    pub pull_request: Option<PullRequest>,
    pub last_prompt: Option<String>,
    pub activity: Activity,
    pub subagents: Vec<Subagent>,
    /// Set when the transcript exists but could not be read or parsed.
    pub read_error: Option<String>,
}

/// Where a session's context limit came from. Only `Assumed` is a guess, and it
/// is the only one the panels mark as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitSource {
    /// `--context-limit`, which outranks anything read from disk.
    Override,
    /// Usage already past the standard window, so the long one is a fact.
    Observed,
    /// A `cost-state` line billed this session's model at `[1m]`.
    Recorded,
    /// Settings name the long variant of the model this session runs.
    Configured,
    /// Nothing said otherwise.
    Assumed,
}

impl LimitSource {
    pub fn is_guess(self) -> bool {
        self == LimitSource::Assumed
    }
}

/// Does `configured` name the long variant of `model`? A user-wide `opus[1m]`
/// must not widen a session that switched to sonnet, so the family matters.
fn names_long_variant(configured: &str, model: &str) -> bool {
    let Some(base) = configured.strip_suffix(LONG_CONTEXT_SUFFIX) else {
        return false;
    };
    !base.is_empty() && model.to_lowercase().contains(&base.to_lowercase())
}

#[derive(Debug, Clone)]
pub struct Session {
    pub pid: u32,
    pub session_id: String,
    pub cwd: PathBuf,
    pub name: String,
    pub status: Status,
    pub version: String,
    pub kind: String,
    pub entrypoint: String,
    pub started_at_ms: i64,
    pub status_updated_at_ms: i64,
    pub proc: Option<ProcStat>,
    /// The model settings name for this session's directory. Not proof of what
    /// the session runs now, but the only clue a young session gives.
    pub configured_model: Option<String>,
    pub detail: Detail,
}

impl Session {
    /// Directory name only. Two sessions often share a repo, so the list shows
    /// this while the detail pane shows the full path.
    pub fn dir_label(&self) -> String {
        self.cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.cwd.to_string_lossy().to_string())
    }

    /// How long the session has held its current status. This is what separates
    /// a session working on something from one wedged in `busy`.
    pub fn status_age_secs(&self, now_ms: i64) -> Option<u64> {
        if self.status_updated_at_ms <= 0 {
            return None;
        }
        let delta = now_ms - self.status_updated_at_ms;
        if delta > 0 {
            Some((delta / 1000) as u64)
        } else {
            Some(0)
        }
    }

    pub fn uptime_secs(&self, now_ms: i64) -> u64 {
        let delta = now_ms - self.started_at_ms;
        if delta > 0 {
            (delta / 1000) as u64
        } else {
            0
        }
    }

    /// The context limit for this session, and where the number came from.
    /// Strongest evidence first: measured usage cannot be argued with, a
    /// `cost-state` line is the session's own record, and settings are only the
    /// default it started from.
    pub fn context_limit(&self, override_limit: Option<u64>) -> (u64, LimitSource) {
        if let Some(limit) = override_limit {
            return (limit, LimitSource::Override);
        }
        if self.detail.usage_peak > CONTEXT_LIMIT_STANDARD {
            return (CONTEXT_LIMIT_LONG, LimitSource::Observed);
        }
        if self.recorded_long() {
            return (CONTEXT_LIMIT_LONG, LimitSource::Recorded);
        }
        if self.configured_long() {
            return (CONTEXT_LIMIT_LONG, LimitSource::Configured);
        }
        (CONTEXT_LIMIT_STANDARD, LimitSource::Assumed)
    }

    /// Was this session's current model billed against the long window?
    fn recorded_long(&self) -> bool {
        let Some(model) = self.detail.model.as_deref() else {
            return false;
        };
        self.detail
            .cost
            .as_ref()
            .is_some_and(|cost| cost.billed_long(model))
    }

    fn configured_long(&self) -> bool {
        match (self.configured_model.as_deref(), self.detail.model.as_deref()) {
            (Some(configured), Some(model)) => names_long_variant(configured, model),
            _ => false,
        }
    }

    pub fn context_ratio(&self, override_limit: Option<u64>) -> f64 {
        let used = self.detail.usage.as_ref().map(|u| u.total()).unwrap_or(0);
        let (limit, _) = self.context_limit(override_limit);
        if limit == 0 {
            return 0.0;
        }
        (used as f64 / limit as f64).clamp(0.0, 1.0)
    }
}

/// Format a token count the way the panels want it: `82k`, `1.2M`, `940`.
pub fn tokens_short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// Compact money: `$1.22`, and four places under a cent so a session that cost
/// something does not read as free.
pub fn cost_short(usd: f64) -> String {
    if usd >= 0.01 || usd == 0.0 {
        format!("${usd:.2}")
    } else {
        format!("${usd:.4}")
    }
}

/// Compact duration: `4d2h`, `3h12m`, `18m54s`, `41s`.
pub fn duration_short(secs: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    if secs >= DAY {
        format!("{}d{}h", secs / DAY, (secs % DAY) / HOUR)
    } else if secs >= HOUR {
        format!("{}h{}m", secs / HOUR, (secs % HOUR) / MINUTE)
    } else if secs >= MINUTE {
        format!("{}m{}s", secs / MINUTE, secs % MINUTE)
    } else {
        format!("{}s", secs)
    }
}

/// Milliseconds, kept whole under a second. Tool time is often a few hundred
/// milliseconds and must not read as nothing.
pub fn duration_ms_short(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    duration_short(ms / 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(peak: u64, used: u64) -> Session {
        Session {
            pid: 1,
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            name: "one".into(),
            status: Status::Busy,
            version: String::new(),
            kind: String::new(),
            entrypoint: String::new(),
            started_at_ms: 0,
            status_updated_at_ms: 0,
            proc: None,
            configured_model: None,
            detail: Detail {
                model: Some("claude-opus-5".into()),
                usage: Some(ContextUsage {
                    input: used,
                    ..Default::default()
                }),
                usage_peak: peak,
                ..Default::default()
            },
        }
    }

    #[test]
    fn usage_past_the_standard_window_settles_the_limit() {
        let (limit, source) = session(386_849, 386_849).context_limit(None);
        assert_eq!(limit, CONTEXT_LIMIT_LONG);
        assert_eq!(source, LimitSource::Observed);
        assert!(!source.is_guess());
    }

    #[test]
    fn nothing_known_falls_back_to_the_standard_window() {
        let (limit, source) = session(169_456, 169_456).context_limit(None);
        assert_eq!(limit, CONTEXT_LIMIT_STANDARD);
        assert_eq!(source, LimitSource::Assumed);
        assert!(source.is_guess());
    }

    fn billed(id: &str) -> CostState {
        CostState {
            cost_usd: 1.220_764,
            models: vec![ModelCost {
                id: id.into(),
                cost_usd: 1.220_764,
            }],
            ..Default::default()
        }
    }

    /// The regression: 169k of a 1M window read as 85% instead of 17%.
    #[test]
    fn a_recorded_long_model_widens_a_session_under_200k() {
        let mut session = session(169_456, 169_456);
        session.detail.cost = Some(billed("claude-opus-5[1m]"));

        let (limit, source) = session.context_limit(None);
        assert_eq!(limit, CONTEXT_LIMIT_LONG);
        assert_eq!(source, LimitSource::Recorded);
        assert!((session.context_ratio(None) - 0.169456).abs() < 1e-9);
    }

    #[test]
    fn a_recorded_model_the_session_left_is_ignored() {
        let mut session = session(169_456, 169_456);
        session.detail.cost = Some(billed("claude-fable-5[1m]"));
        assert_eq!(session.context_limit(None).1, LimitSource::Assumed);
    }

    #[test]
    fn settings_widen_only_the_family_they_name() {
        let mut session = session(100, 100);
        session.configured_model = Some("opus[1m]".into());
        assert_eq!(session.context_limit(None).1, LimitSource::Configured);

        session.detail.model = Some("claude-sonnet-5".into());
        assert_eq!(session.context_limit(None).1, LimitSource::Assumed);

        session.detail.model = Some("claude-opus-5".into());
        session.configured_model = Some("opus".into());
        assert_eq!(session.context_limit(None).1, LimitSource::Assumed);
    }

    #[test]
    fn a_cost_under_a_cent_still_shows_a_figure() {
        assert_eq!(cost_short(0.0), "$0.00");
        assert_eq!(cost_short(0.000_969), "$0.0010");
        assert_eq!(cost_short(0.009_9), "$0.0099");
        assert_eq!(cost_short(0.01), "$0.01");
        assert_eq!(cost_short(1.221_733_000_000_000_2), "$1.22");
    }

    #[test]
    fn a_duration_under_a_second_keeps_its_milliseconds() {
        assert_eq!(duration_ms_short(358), "358ms");
        assert_eq!(duration_ms_short(999), "999ms");
        assert_eq!(duration_ms_short(1_000), "1s");
        assert_eq!(duration_ms_short(132_206), "2m12s");
    }

    #[test]
    fn an_explicit_limit_outranks_every_signal() {
        let mut session = session(386_849, 386_849);
        session.configured_model = Some("opus[1m]".into());

        let (limit, source) = session.context_limit(Some(500_000));
        assert_eq!(limit, 500_000);
        assert_eq!(source, LimitSource::Override);
        assert!(!source.is_guess());
    }
}
