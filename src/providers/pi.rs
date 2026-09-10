use crate::model::{ClientKind, Detail, ProcessSnapshot, Session, Status, SESSIONS_MAX};
use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn is_pi(command: &str) -> bool {
    matches!(command.to_ascii_lowercase().as_str(), "pi" | "pi-rpc")
}

pub fn discover(sessions_dir: &Path, processes: &ProcessSnapshot) -> Result<Vec<Session>> {
    let files = session_files(sessions_dir);
    let mut out = Vec::new();
    for p in processes
        .processes
        .iter()
        .filter(|p| is_pi(&p.command))
        .take(SESSIONS_MAX)
    {
        let path = p
            .args
            .iter()
            .find_map(|a| {
                let x = Path::new(a);
                x.is_file().then(|| x.to_path_buf())
            })
            .or_else(|| {
                files
                    .iter()
                    .find(|f| {
                        p.cwd
                            .as_deref()
                            .map(|c| header_cwd(f).as_deref() == Some(c))
                            .unwrap_or(false)
                    })
                    .cloned()
            });
        let (id, detail, cwd) = path
            .as_deref()
            .and_then(|x| parse_session(x).ok())
            .unwrap_or_else(|| {
                (
                    format!("pid-{}", p.pid),
                    Detail::default(),
                    p.cwd.clone().unwrap_or_default(),
                )
            });
        out.push(Session {
            client: ClientKind::Pi,
            pid: p.pid,
            session_id: id,
            cwd,
            name: detail.title.clone().unwrap_or_else(|| "(Pi)".into()),
            status: if detail.last_prompt.is_some() {
                Status::Idle
            } else {
                Status::Other
            },
            version: String::new(),
            kind: String::new(),
            entrypoint: String::new(),
            started_at_ms: p.started_at_ms.unwrap_or(0),
            status_updated_at_ms: 0,
            proc: Some(p.proc_stat()),
            configured_model: None,
            detail,
        });
    }
    Ok(out)
}

fn session_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(p: &Path, o: &mut Vec<PathBuf>) {
        if o.len() >= SESSIONS_MAX * 4 {
            return;
        }
        if let Ok(es) = std::fs::read_dir(p) {
            for e in es.flatten() {
                let x = e.path();
                if x.is_dir() {
                    walk(&x, o)
                } else if x.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                    o.push(x)
                }
            }
        }
    }
    walk(root, &mut out);
    out
}
fn header_cwd(p: &Path) -> Option<PathBuf> {
    parse_session(p).ok().map(|x| x.2)
}

pub fn parse_session(path: &Path) -> Result<(String, Detail, PathBuf)> {
    let text = std::fs::read_to_string(path)?;
    let mut id: String = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut cwd = PathBuf::new();
    let mut detail = Detail {
        transcript: Some(path.to_path_buf()),
        ..Default::default()
    };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "session" | "session_info" => {
                id = v
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_owned();
                cwd = v
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or_default();
                detail.title = v
                    .get("name")
                    .or_else(|| v.get("title"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            "message" | "user" => {
                if v.get("role").and_then(Value::as_str).unwrap_or("") == "user" {
                    detail.last_prompt =
                        v.get("content").and_then(Value::as_str).map(str::to_owned);
                }
            }
            "model_change" => {
                detail.model = v.get("model").and_then(Value::as_str).map(str::to_owned)
            }
            "toolCall" => {
                if let Some(n) = v.get("name").and_then(Value::as_str) {
                    detail.activity.record_tool(crate::model::ToolCall {
                        name: n.into(),
                        summary: String::new(),
                        detail: v.to_string(),
                    })
                }
            }
            _ => {}
        }
    }
    Ok((id, detail, cwd))
}
