//! Activity provenance for live agents. Saved writes never imply a live state.

use crate::domain::{AgentKind, AgentView};
use crate::history::{HistoryEventKind, HistoryInventory};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityEvidence {
    pub summary: String,
    pub observed_at: Option<i64>,
    pub activity_source: String,
    pub state_source: String,
    pub recorded: bool,
}

pub fn derive_activity(view: &AgentView, history: &HistoryInventory) -> ActivityEvidence {
    let agent = &view.observation;
    let state_source = if agent.runtime.provider == "Herdr" {
        "Herdr reported status"
    } else if agent.kind == AgentKind::Claude
        && agent
            .runtime
            .fields
            .iter()
            .any(|(key, _)| key == "Session ID")
    {
        "Claude agents --json"
    } else {
        "process only"
    };

    let has_pane_text = agent.runtime.provider == "Herdr"
        && agent.output_error.is_none()
        && !agent.recent_output.trim().is_empty();
    if has_pane_text {
        return ActivityEvidence {
            summary: view.activity.clone(),
            observed_at: view.last_output_change,
            activity_source: "Herdr pane text".into(),
            state_source: state_source.into(),
            recorded: false,
        };
    }

    let session_id = agent
        .runtime
        .fields
        .iter()
        .find(|(key, _)| key == "Session ID")
        .map(|(_, id)| id.as_str());
    if let Some(session) = session_id.and_then(|id| {
        history
            .sessions
            .iter()
            .find(|session| session.kind == agent.kind && session.id == id)
    }) {
        if let Some(event) = session
            .events
            .iter()
            .rev()
            .find(|event| event.kind != HistoryEventKind::Result)
        {
            let summary = match event.kind {
                HistoryEventKind::Tool => format!("recorded tool: {}", compact(&event.text, 80)),
                HistoryEventKind::Assistant => format!("recorded: {}", compact(&event.text, 80)),
                HistoryEventKind::User => "user message recorded".into(),
                HistoryEventKind::Result => unreachable!(),
            };
            let source = match agent.kind {
                AgentKind::Codex => "Codex session file",
                AgentKind::Claude => "Claude session file",
                AgentKind::Pi => "Pi session file",
                AgentKind::OpenCode => "OpenCode session record",
                AgentKind::Gemini => "Gemini session record",
                AgentKind::Cursor => "Cursor session record",
            };
            return ActivityEvidence {
                summary,
                observed_at: (session.updated_at > 0).then_some(session.updated_at),
                activity_source: source.into(),
                state_source: state_source.into(),
                recorded: true,
            };
        }
    }
    ActivityEvidence {
        summary: view.activity.clone(),
        observed_at: None,
        activity_source: if agent.runtime.provider == "Herdr" {
            "Herdr status only"
        } else {
            "process only"
        }
        .into(),
        state_source: state_source.into(),
        recorded: false,
    }
}

fn compact(value: &str, limit: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}
