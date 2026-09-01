//! The model named in settings.
//!
//! A session that has not yet written a `cost-state` line gives away nothing
//! about its context window, so the configured default stands in. Claude Code
//! resolves settings nearest-first, and so does this.

use std::path::Path;

use serde_json::Value;

/// The model for a session running in `cwd`, or none if no file names one.
pub fn model_for(claude_home: &Path, cwd: &Path) -> Option<String> {
    let project = cwd.join(".claude");
    [
        project.join("settings.local.json"),
        project.join("settings.json"),
        claude_home.join("settings.json"),
    ]
    .iter()
    .find_map(|path| read_model(path))
}

/// A missing or half-written settings file is no answer, not a failure.
fn read_model(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let model = value.get("model")?.as_str()?.trim();
    if model.is_empty() {
        return None;
    }
    Some(model.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn nearest_settings_file_wins() {
        let root = std::env::temp_dir().join("dancefloor-settings-precedence");
        std::fs::remove_dir_all(&root).ok();
        let home = root.join("home");
        let cwd = root.join("repo");

        write(&home.join("settings.json"), r#"{"model":"opus[1m]"}"#);
        assert_eq!(model_for(&home, &cwd).as_deref(), Some("opus[1m]"));

        write(
            &cwd.join(".claude/settings.json"),
            r#"{"model":"claude-sonnet-5"}"#,
        );
        assert_eq!(model_for(&home, &cwd).as_deref(), Some("claude-sonnet-5"));

        write(
            &cwd.join(".claude/settings.local.json"),
            r#"{"model":"fable[1m]"}"#,
        );
        assert_eq!(model_for(&home, &cwd).as_deref(), Some("fable[1m]"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_without_a_model_is_skipped() {
        let root = std::env::temp_dir().join("dancefloor-settings-no-model");
        std::fs::remove_dir_all(&root).ok();
        let home = root.join("home");
        let cwd = root.join("repo");

        write(&home.join("settings.json"), r#"{"model":"opus[1m]"}"#);
        write(&cwd.join(".claude/settings.json"), r#"{"theme":"dark"}"#);
        write(&cwd.join(".claude/settings.local.json"), "{ truncated");

        assert_eq!(model_for(&home, &cwd).as_deref(), Some("opus[1m]"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_settings_at_all_is_not_an_error() {
        let root = std::env::temp_dir().join("dancefloor-settings-absent");
        assert_eq!(model_for(&root.join("home"), &root.join("repo")), None);
    }
}
