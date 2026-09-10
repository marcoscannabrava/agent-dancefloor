use crate::model::{
    ClientKind, ContextUsage, Detail, ProcessSnapshot, Session, Status, ToolCall, SESSIONS_MAX,
    TRANSCRIPT_LINES_MAX,
};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn is_codex(command: &str, args: &[String]) -> bool {
    let c = command.to_ascii_lowercase();
    c == "codex" || c == "codex.exe" || (c == "node" && args.iter().any(|a| a.contains("codex")))
}

pub fn discover(home: &Path, processes: &ProcessSnapshot) -> Result<Vec<Session>> {
    let mut out = Vec::new();
    let mut used_rollouts = HashSet::new();
    let mut used_session_ids = HashSet::new();
    for process in processes
        .processes
        .iter()
        .filter(|p| is_codex(&p.command, &p.args))
        .take(SESSIONS_MAX)
    {
        let path = explicit_rollout(home, process)
            .or_else(|| find_recent_rollout(home, process.cwd.as_deref(), &used_rollouts));
        // A Codex launcher and its runtime can both look like Codex in `ps`.
        // Once one process has claimed a rollout, do not show that same
        // persistent session a second time.
        if let Some(path) = &path {
            if !used_rollouts.insert(path.clone()) {
                continue;
            }
        }
        let (id, detail, cwd) = path
            .as_deref()
            .and_then(|p| parse_rollout(p).ok())
            .unwrap_or_else(|| {
                (
                    format!("pid-{}", process.pid),
                    Detail::default(),
                    process.cwd.clone().unwrap_or_default(),
                )
            });
        if !id.starts_with("pid-") && !used_session_ids.insert(id.clone()) {
            continue;
        }
        let status = if detail.activity.last_turn.is_some() {
            Status::Idle
        } else {
            Status::Other
        };
        out.push(Session {
            client: ClientKind::Codex,
            pid: process.pid,
            session_id: id,
            cwd,
            name: detail.title.clone().unwrap_or_else(|| "(Codex)".into()),
            status,
            version: String::new(),
            kind: String::new(),
            entrypoint: String::new(),
            started_at_ms: process.started_at_ms.unwrap_or(0),
            status_updated_at_ms: 0,
            proc: Some(process.proc_stat()),
            configured_model: None,
            detail,
        });
    }
    Ok(out)
}

fn explicit_rollout(home: &Path, p: &crate::model::ProcessInfo) -> Option<PathBuf> {
    p.args
        .iter()
        .find_map(|arg| {
            let path = Path::new(arg);
            path.is_file().then(|| path.to_path_buf())
        })
        .filter(|p| p.starts_with(home))
}

fn find_recent_rollout(
    home: &Path,
    cwd: Option<&Path>,
    used_rollouts: &HashSet<PathBuf>,
) -> Option<PathBuf> {
    let root = home.join("sessions");
    let mut candidates = Vec::new();
    for year in std::fs::read_dir(root).ok()?.flatten().take(3) {
        for month in std::fs::read_dir(year.path()).ok()?.flatten().take(12) {
            for day in std::fs::read_dir(month.path()).ok()?.flatten().take(31) {
                for entry in std::fs::read_dir(day.path()).ok()?.flatten().take(128) {
                    let p = entry.path();
                    if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                        candidates.push(p);
                    }
                }
            }
        }
    }
    candidates.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    candidates.into_iter().rev().find(|p| {
        !used_rollouts.contains(p)
            && cwd
                .map(|c| rollout_cwd(p).as_deref() == Some(c))
                .unwrap_or(true)
    })
}

fn rollout_cwd(path: &Path) -> Option<PathBuf> {
    parse_rollout(path).ok().map(|(_, _, cwd)| cwd)
}

pub fn parse_rollout(path: &Path) -> Result<(String, Detail, PathBuf)> {
    let text = std::fs::read_to_string(path)?;
    let mut id = None;
    let mut cwd = PathBuf::new();
    let mut detail = Detail {
        transcript: Some(path.to_path_buf()),
        ..Default::default()
    };
    let mut lines = text
        .lines()
        .rev()
        .take(TRANSCRIPT_LINES_MAX)
        .collect::<Vec<_>>();
    lines.reverse();
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "session_meta" {
            id = v
                .get("payload")
                .and_then(|x| x.get("id").or_else(|| x.get("session_id")))
                .and_then(Value::as_str)
                .map(str::to_owned);
            cwd = v
                .get("payload")
                .and_then(|x| x.get("cwd"))
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_default();
        }
        if kind == "turn_context" {
            detail.model = v
                .pointer("/payload/model")
                .and_then(Value::as_str)
                .map(str::to_owned);
            detail.effort = v
                .pointer("/payload/effort")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if kind == "event_msg" {
            let msg = v
                .get("payload")
                .and_then(|x| x.get("msg"))
                .unwrap_or(&Value::Null);
            match msg.get("type").and_then(Value::as_str).unwrap_or("") {
                "task_started" => detail.activity.last_turn = None,
                "task_complete" | "turn_aborted" => {
                    detail.activity.last_turn = Some(crate::model::Turn {
                        duration_ms: 0,
                        messages: 0,
                    })
                }
                _ => {}
            }
        }
        if kind == "response_item" {
            let p = v.get("payload").unwrap_or(&Value::Null);
            if p.get("type").and_then(Value::as_str) == Some("message") {
                let text = p
                    .get("content")
                    .and_then(Value::as_array)
                    .and_then(|a| {
                        a.iter()
                            .rev()
                            .find_map(|x| x.get("text").and_then(Value::as_str))
                    })
                    .map(str::to_owned);
                if text.is_some() {
                    detail.last_prompt = text;
                }
            }
            if p.get("type").and_then(Value::as_str) == Some("function_call") {
                detail.activity.record_tool(ToolCall {
                    name: p
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .into(),
                    summary: String::new(),
                    detail: p.get("arguments").map(Value::to_string).unwrap_or_default(),
                });
            }
        }
        if kind == "event_msg"
            && v.pointer("/payload/msg/type").and_then(Value::as_str) == Some("token_count")
        {
            if let Some(u) = v.pointer("/payload/msg/info/total_token_usage") {
                detail.usage = Some(ContextUsage {
                    input: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
                    output: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
                    cache_read: u
                        .get("cached_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    cache_creation: 0,
                });
            }
        }
    }
    Ok((
        id.unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into()
        }),
        detail,
        cwd,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProcessInfo, ProcessSnapshot};

    #[test]
    fn launcher_and_runtime_claiming_one_rollout_make_one_row() {
        let root = std::env::temp_dir().join(format!("dancefloor-codex-{}", std::process::id()));
        let path = root.join("sessions/2026/01/01/rollout-test.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"same-session","cwd":"/repo"}}
"#,
        )
        .unwrap();
        let process = |pid| ProcessInfo {
            pid,
            ppid: None,
            started_at_ms: None,
            elapsed_secs: None,
            rss_kib: 1,
            cpu_percent: 0.0,
            executable: "codex".into(),
            command: "codex".into(),
            args: vec![path.to_string_lossy().into_owned()],
            cwd: Some(PathBuf::from("/repo")),
        };
        let sessions = discover(
            &root,
            &ProcessSnapshot {
                processes: vec![process(1), process(2)],
            },
        )
        .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "same-session");
        std::fs::remove_dir_all(root).ok();
    }
}
