//! The Activity pane: what the session is doing, newest first.
//!
//! The recap leads because one sentence of prose beats twenty tool names when
//! you are scanning a fleet. Under it sit the files the session changed, then
//! the tool stream as evidence. The stream is the part that runs off the bottom
//! of the pane, so nothing shorter is allowed to sit below it.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::model::{duration_short, Activity, Session};
use crate::ui::{label_value, ACCENT, LABEL};

/// Two spaces of indent, the tool-name column, and the space after it.
const ROW_INDENT: usize = 15;

pub fn draw(frame: &mut Frame, session: &Session, area: Rect) {
    let activity = &session.detail.activity;
    let lines = if activity.is_empty() {
        vec![Line::from(Span::styled(
            "Nothing recorded in the transcript tail yet.",
            Style::new().fg(LABEL),
        ))]
    } else {
        let room = (area.width as usize).saturating_sub(ROW_INDENT);
        let mut lines = recap_lines(activity);
        lines.extend(turn_lines(activity));
        lines.extend(file_lines(session, room));
        lines.extend(tool_lines(activity, room));
        lines
    };

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn recap_lines(activity: &Activity) -> Vec<Line<'static>> {
    let Some(recap) = &activity.recap else {
        return Vec::new();
    };
    vec![Line::raw(recap.clone()), Line::raw("")]
}

/// Who is steering, and how long the turn before this one took.
fn turn_lines(activity: &Activity) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(driver) = &activity.driver {
        lines.push(label_value("driving", driver.label()));
    }
    if let Some(turn) = &activity.last_turn {
        lines.push(label_value(
            "last turn",
            format!(
                "{} · {} messages",
                duration_short(turn.duration_ms / 1000),
                turn.messages
            ),
        ));
    }
    if !lines.is_empty() {
        lines.push(Line::raw(""));
    }
    lines
}

/// Edited files, shown relative to the session's directory when they sit under
/// it. Plans and scratch files land elsewhere and keep their full path.
fn file_lines(session: &Session, room: usize) -> Vec<Line<'static>> {
    let files = &session.detail.activity.files;
    if files.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![heading("files touched")];
    for path in files.iter().rev() {
        let shown = path.strip_prefix(&session.cwd).unwrap_or(path);
        lines.push(Line::from(Span::styled(
            format!(
                "  {}",
                clip(&shown.to_string_lossy(), room + ROW_INDENT - 2)
            ),
            Style::new().fg(Color::Gray),
        )));
    }
    lines.push(Line::raw(""));
    lines
}

fn tool_lines(activity: &Activity, room: usize) -> Vec<Line<'static>> {
    if activity.tools.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![heading("recent tools")];
    for call in activity.tools.iter().rev() {
        lines.push(Line::from(vec![
            // The trailing space is not padding: a tool name wider than the
            // column would otherwise run straight into its target.
            Span::styled(format!("  {:<12} ", call.name), Style::new().bold()),
            Span::styled(clip(&call.target, room), Style::new().fg(Color::Gray)),
        ]));
    }
    lines
}

fn heading(text: &'static str) -> Line<'static> {
    Line::from(Span::styled(text, Style::new().fg(ACCENT).bold()))
}

/// One row per call, so a target that would wrap is cut instead. A wrapped tool
/// list doubles in height and stops reading as a list.
///
/// A path is identified by its end, so it loses its front; everything else
/// loses its tail.
fn clip(text: &str, room: usize) -> String {
    let length = text.chars().count();
    if room < 2 || length <= room {
        return text.to_string();
    }
    if text.starts_with('/') {
        let dropped = length - room + 1;
        return std::iter::once('…')
            .chain(text.chars().skip(dropped))
            .collect();
    }
    text.chars().take(room - 1).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::clip;

    #[test]
    fn a_path_keeps_the_end_and_prose_keeps_the_start() {
        assert_eq!(clip("/repo/src/ui/activity.rs", 12), "…activity.rs");
        assert_eq!(clip("run the whole suite", 8), "run the…");
        assert_eq!(clip("short", 12), "short");
    }
}
