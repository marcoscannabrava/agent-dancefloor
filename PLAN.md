# Multi-client session tracking plan

## Goal

Make `dancefloor` a read-only dashboard for every live Claude Code, Codex, and Pi coding-agent session owned by the current user. A mixed dashboard must retain the useful common views—status, working directory, model, context use, prompt, tools, files, process cost, and subagents—without inventing data that a client does not record.

The first release is complete when:

- all three clients are discovered by default and can appear together;
- a session is included only while its client process is alive (with explicit handling for ephemeral sessions that have no transcript);
- selection and caches use `(client, session_id)`, not a PID or a globally unique-looking UUID;
- each transcript is normalized into the existing `Session`/`Detail` UI model;
- missing homes, missing fields, partial writes, stale metadata, and one broken provider do not hide healthy sessions from the other providers;
- `dancefloor --once`, the TUI, tests, help, and README describe the dashboard generically;
- discovery remains local and read-only: no API calls and no writes to Claude, Codex, or Pi state.

Historical-session browsing, sending input to agents, killing processes, and exact cost conversion across providers are out of scope.

## What the clients expose

The adapter design should be based on the following current on-disk contracts, captured as sanitized fixtures before refactoring:

| Client | Live-session evidence | Transcript and metadata | Reliable status signal |
| --- | --- | --- | --- |
| Claude Code | `~/.claude/sessions/<pid>.json`, verified against a live `claude` process | `~/.claude/projects/**/<session>.jsonl` and the sibling `subagents/` directory | registry `waiting` / `busy` / `idle` |
| Codex | live `codex` process plus a matching rollout; process start, cwd, CLI arguments, and writer metadata are correlation inputs | `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`; `session_meta`, `turn_context`, lifecycle events, response items, and token events | the last `task_started`, `task_complete`, or `turn_aborted` event distinguishes busy from idle; waiting is unknown unless a durable event is found |
| Pi | live process whose title is `pi` or `pi-rpc`; process cwd/arguments are correlated with the session header | `$PI_CODING_AGENT_SESSION_DIR`, or `${PI_CODING_AGENT_DIR:-~/.pi/agent}/sessions/**/<timestamp>_<uuid>.jsonl` | the active branch's last user/tool entry versus terminal assistant entry gives busy/idle; interactive confirmation waiting is not persisted |

Codex currently also has versioned SQLite projections and `thread-writer-locks`. They are useful corroborating signals, especially for spawn edges and an in-progress turn, but they must not be the only source of truth: the filenames and schemas are internal and versioned. Prefer rollout events, degrade gracefully when the projections are absent or busy, and isolate any SQLite reader inside the Codex adapter.

Pi session files are trees, not simple linear logs. Its parser must follow `id`/`parentId` from the current leaf and honor compaction entries; otherwise abandoned branches will be reported as current activity. Pi's documented `PI_SESSION_ID` and `PI_SESSION_FILE` variables are injected into tool subprocesses rather than the main process, so they are opportunistic correlation evidence, not a required registry.

## Architecture

### 1. Make the core model client-neutral

Update `src/model.rs` first, while keeping the current Claude parser behavior covered by tests.

- Add `ClientKind::{Claude, Codex, Pi}` with stable display labels and sort order.
- Add `SessionKey { client, session_id }`; use it for selection anchors, transcript caches, and deduplication. Keep `pid` as process metadata, not identity.
- Add `client` to `Session` and rename Claude-specific presentation fields where necessary (`client_version`, `session_kind`, and `entrypoint` may remain optional strings).
- Replace `configured_model` and `long_context_models` as cross-client concepts with normalized context evidence: an optional recorded limit plus its source/confidence. Keep Claude's existing 200k/1M inference inside the Claude adapter or a Claude-specific evidence variant.
- Keep fields optional. Do not synthesize `0%`, `normal`, `auto`, or an empty model when a client did not record them.
- Retain the common `Status` values, but treat unsupported `waiting` detection as `Busy`, `Idle`, or `Other`, never as a guess presented as fact.
- Generalize `Subagent` enough to carry an optional child session ID/client and state, while preserving Claude's current metadata.

The UI should consume only normalized fields. Client-format names and JSON keys must stay in provider modules.

### 2. Take one process snapshot per refresh

Extract process handling from `src/discovery.rs` into `src/process.rs`.

- Run `ps` once per refresh and collect PID, PPID, start/elapsed time, RSS, CPU, executable/command name, and arguments.
- Classify exact client processes, including native Codex executables, the Codex Node launcher, `pi`, and `pi-rpc`; exclude shell-tool children that merely inherit client environment markers.
- Resolve cwd through `/proc/<pid>/cwd` on Linux. Add a small platform abstraction and a best-effort macOS fallback (for example `lsof`); an unavailable cwd must not discard the process.
- Confirm that a PID still has the expected executable before returning a session, protecting against stale files and PID reuse as the Claude scanner does today.
- Expose a `ProcessSnapshot` that can be constructed directly in tests. Provider tests must not depend on the developer machine's process table.

Keep process discovery bounded by `SESSIONS_MAX`, and collect stats in bulk rather than spawning once per candidate.

### 3. Introduce provider adapters

Create `src/providers/{mod.rs,claude.rs,codex.rs,pi.rs}` with a narrow common interface:

1. discover candidate live sessions from a provider home plus `ProcessSnapshot`;
2. correlate each process with at most one persistent session, or emit an ephemeral session keyed by client and PID;
3. parse/enrich a candidate into normalized `Detail`;
4. return provider-scoped warnings without failing the full refresh.

Move the existing `discovery.rs`, `transcript.rs`, `settings.rs`, and `subagents.rs` logic under the Claude adapter with minimal semantic change. Compatibility re-exports can keep the refactor reviewable, then be removed after callers and tests move.

Add `ProviderHomes`, resolved once at startup:

- Claude: `~/.claude`;
- Codex: `$CODEX_HOME`, else `~/.codex`;
- Pi agent dir: `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`;
- Pi sessions: `$PI_CODING_AGENT_SESSION_DIR`, else `<pi-agent-dir>/sessions`.

A missing provider directory is an empty provider, not a startup error. Replace `App::claude_home` with provider configuration and replace the single `scan_error` with per-provider warnings. One bad transcript or locked Codex projection must affect only that session/provider.

### 4. Add the Codex adapter

Discovery and correlation:

- enumerate live Codex processes first;
- read only plausible recent rollout headers, bounded by count and date directories, rather than recursively parsing all history on every tick;
- match `session_meta.cwd` and timestamp to process cwd/start time, honor an explicit resume/session argument if present, and perform a one-to-one match when several processes share a directory;
- use writer-lock ownership or the versioned thread projection only as corroboration;
- if an exact match is ambiguous, show a process-only Codex row with unknown transcript data rather than attaching another session's prompt;
- cache successful `(process identity -> rollout)` matches, invalidate them when the process start time changes, and allow re-correlation after resume/session switching.

Transcript normalization:

- `session_meta`: session ID, cwd, start time, CLI version, git data, source, and recorded context window;
- latest `turn_context`: model, effort, approval/sandbox mode, workspace roots, and current turn ID;
- `event_msg` lifecycle: busy/idle, turn duration where timestamps exist, compaction, and latest agent/user messages;
- `event_msg.token_count` and `token_usage_record`: newest context usage, peak, totals, and recorded model context window;
- `response_item.message`: latest real user prompt/title fallback while excluding developer/system scaffolding;
- `response_item.function_call` and `custom_tool_call`: tool name, summary/input, files touched, and MCP/skill driver where identifiable;
- tool outputs are used only to associate completion/errors and spawned children, never rendered wholesale.

Read only the bounded tail for activity and usage, with the existing backward prompt search adapted to Codex message shapes. Ignore a final partial JSONL line and retry on the next refresh.

For subagents, first use stable spawn information present in the parent rollout/tool result. If the installed Codex projection exposes `thread_spawn_edges`, enrich those children best-effort behind a schema check. A projection change must leave an empty Agents pane, not break the parent session.

### 5. Add the Pi adapter

Discovery and correlation:

- identify `pi`/`pi-rpc` processes using process title/command and reject ordinary Node children;
- honor `--session`, `--session-dir`, and continue/resume arguments when they make a file deterministic;
- otherwise match session-header cwd and creation time to process cwd/start time, using recent modification as a secondary signal and a one-to-one assignment for concurrent sessions in one directory;
- re-evaluate a cached match when another same-cwd transcript starts receiving writes, because `/resume` and runtime session switching do not change the process PID;
- show `--no-session` and unresolved processes as ephemeral Pi rows without transcript-derived details.

Transcript normalization:

- parse the header plus bounded entries and build the active parent chain before deriving any current value;
- latest `session_info`: display name; otherwise use the first active-branch user prompt, then the directory name;
- latest `model_change`, `thinking_level_change`, or assistant metadata: provider/model and reasoning level;
- assistant `usage`: input, output, cache read/write, recent totals, and costs only if a future normalized cost field is added;
- active-branch user message: last prompt;
- assistant `toolCall` blocks and `toolResult` entries: tool activity and failure state; normalize common path/command/pattern arguments using the existing tool-detail limits;
- `compaction` and `branch_summary`: active context recap and token evidence without counting abandoned history;
- infer touched files only from known write/edit tool schemas or structured compaction details.

Resolve Pi model context limits from recorded/provider model metadata only when exact. Otherwise leave the limit unknown; the global user override may still provide a display value.

Core Pi has no universal subagent registry. Support known structured child-session or delegation artifacts only behind format checks and fixtures. Do not interpret arbitrary extension files as subagents.

### 6. Refresh, UI, CLI, and configuration

Refactor `App::refresh` to snapshot processes once, invoke all enabled adapters, merge by `SessionKey`, sort, and preserve selection. Cache keys must include client and process start time so PID reuse cannot retain an old transcript.

Update the presentation in one pass:

- add a compact client label/color to each list row and a `client` line in Detail;
- make empty-state and error text say “coding-agent sessions” rather than “Claude Code sessions”;
- omit unavailable Detail/Usage rows instead of showing misleading placeholders, and say when context or status is unknown;
- make `--once` include a `CLIENT` column;
- keep status sorting client-agnostic, with client/name as deterministic tie-breakers;
- retain all five tabs even when a provider cannot populate one; its empty message should state that the client did not record the data;
- update crate/module docs, `--help`, README examples, the source table, environment overrides, capability limits, and the read-only guarantee.

Optionally add a config `clients` allow-list (`["claude", "codex", "pi"]`, all by default). Provider path environment variables should be sufficient for the first release; avoid adding redundant path flags unless testing finds a real need.

## Delivery sequence

1. **Fixtures and contract tests.** Add small sanitized Claude, Codex, and Pi fixtures representing idle, busy, malformed/partial, compacted, and branched sessions. Document the observed client versions/date in fixture comments.
2. **Neutral core and process snapshot.** Add `ClientKind`, `SessionKey`, provider homes/errors, and injectable process snapshots. Move Claude behind the adapter and prove existing Claude behavior/rendering is unchanged.
3. **Codex vertical slice.** Ship discovery, correlation, parsing, mixed-list rendering, and `--once` coverage before optional subagent projection enrichment.
4. **Pi vertical slice.** Ship process correlation and active-tree parsing, then add common tools/files and guarded delegation support.
5. **Hardening and documentation.** Exercise concurrent same-cwd sessions, resume/switch behavior, stale PID/lock data, provider homes that disappear mid-refresh, large transcripts, and small terminals; then update README/help.

Each vertical slice should leave the test suite green and be independently reviewable.

## Test matrix

- provider home missing, unreadable, and containing an entry being appended;
- stale Claude registry PID and PID reused by another executable;
- live Codex/Pi process with no transcript (`--no-session` or startup race);
- two clients using the same UUID and two same-client processes using the same cwd;
- two Codex or Pi processes started close together, with deterministic one-to-one matching and an explicit ambiguous fallback;
- Codex busy -> idle -> interrupted lifecycle and context compaction;
- Pi versioned header, active branching, compaction, session rename, model switch, and tool error;
- transcript over the tail limit with the newest prompt outside the tail;
- invalid UTF-8/JSON, partial final line, disappearing file, and unavailable/locked optional SQLite projection;
- selection survives mixed-provider sorting/refresh and does not jump on PID reuse;
- all five panes render empty and populated sessions for every client at the existing six terminal sizes;
- `--once` output for no sessions, one provider, and a mixed set;
- a refresh complexity test/benchmark proving historical transcript count does not make every two-second refresh scan unbounded data.

## Compatibility and risk controls

- Treat all three local formats as external, versioned inputs: use tolerant deserialization, ignore unknown event types, and keep golden fixtures from more than one observed version as formats evolve.
- Never use transcript mtime alone as proof of liveness. A matching live process is mandatory; ambiguity produces an honest process-only row.
- Never make optional SQLite state a hard dependency for Codex correctness.
- Never follow symlinks outside configured provider roots while enumerating transcripts, and retain the existing byte/line/count caps.
- Parse files that clients may be writing without taking locks; accept only complete newline-delimited records and retry later.
- Keep model/context logic provider-specific until normalized evidence exists. Claude's fallback must not silently become the assumed context size for Codex or Pi.
- Verify Linux first, then explicitly test the process-cwd fallback on macOS before claiming equal platform support.

## Definition of done

Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and manual mixed-client smoke tests (`cargo run -- --once` plus the TUI). During the smoke test, start idle and busy examples of each installed client, including two sessions in one directory, and confirm that exiting any process removes exactly its row on the next refresh.
