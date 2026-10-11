use am2::domain::{AgentKind, AgentObservation, AgentState, AgentView, RuntimeMetadata};
use am2::evidence::derive_activity;
use am2::history::{HistoryEvent, HistoryEventKind, HistoryInventory, HistorySession};

fn agent(kind: AgentKind, provider: &str, session_id: Option<&str>) -> AgentView {
    AgentView {
        observation: AgentObservation {
            id: "agent-1".into(),
            kind,
            project: "am2".into(),
            state: AgentState::Unknown,
            pid: 42,
            cwd: "/Projects/am2".into(),
            runtime: RuntimeMetadata {
                provider: provider.into(),
                target: "target".into(),
                fields: session_id
                    .map_or_else(Vec::new, |id| vec![("Session ID".into(), id.into())]),
            },
            recent_output: String::new(),
            output_error: Some("No pane output".into()),
        },
        activity: "activity unknown".into(),
        last_output_change: None,
    }
}

fn history(id: &str, kind: AgentKind) -> HistoryInventory {
    HistoryInventory {
        sessions: vec![HistorySession {
            kind,
            id: id.into(),
            cwd: "/Projects/am2".into(),
            project: "am2".into(),
            updated_at: 150,
            events: vec![HistoryEvent {
                timestamp: Some("2026-10-09T00:00:00Z".into()),
                kind: HistoryEventKind::Tool,
                text: "cargo test".into(),
            }],
        }],
        skipped_files: 0,
    }
}

#[test]
fn herdr_output_reports_pane_source_and_observed_change_age() {
    let mut view = agent(AgentKind::Codex, "Herdr", None);
    view.observation.state = AgentState::Working;
    view.observation.recent_output = "cargo test".into();
    view.observation.output_error = None;
    view.activity = "running tests".into();
    view.last_output_change = Some(100);
    let evidence = derive_activity(&view, &HistoryInventory::default());
    assert_eq!(evidence.summary, "running tests");
    assert_eq!(evidence.observed_at, Some(100));
    assert_eq!(evidence.activity_source, "Herdr pane text");
    assert_eq!(evidence.state_source, "Herdr reported status");
    assert!(!evidence.recorded);
}

#[test]
fn exact_session_id_adds_recorded_action_without_changing_live_state() {
    let view = agent(AgentKind::Claude, "macOS", Some("session-1"));
    let evidence = derive_activity(&view, &history("session-1", AgentKind::Claude));
    assert!(evidence.summary.contains("cargo test"));
    assert!(evidence.recorded);
    assert_eq!(evidence.observed_at, Some(150));
    assert_eq!(evidence.activity_source, "Claude session file");
    assert_eq!(evidence.state_source, "Claude agents --json");
    assert_eq!(view.observation.state, AgentState::Unknown);
}

#[test]
fn matching_cwd_or_different_agent_type_never_links_history() {
    let view = agent(AgentKind::Codex, "macOS", Some("session-1"));
    let mismatched = derive_activity(&view, &history("session-2", AgentKind::Codex));
    assert_eq!(mismatched.summary, "activity unknown");
    assert_eq!(mismatched.activity_source, "process only");
    assert!(!mismatched.recorded);
    let wrong_kind = derive_activity(&view, &history("session-1", AgentKind::Claude));
    assert_eq!(wrong_kind.activity_source, "process only");
}

#[test]
fn empty_pane_output_does_not_claim_pane_activity() {
    let mut view = agent(AgentKind::Pi, "Herdr", None);
    view.observation.output_error = None;
    let evidence = derive_activity(&view, &HistoryInventory::default());
    assert_eq!(evidence.activity_source, "Herdr status only");
    assert_eq!(evidence.observed_at, None);
}

#[test]
fn exact_saved_session_enriches_herdr_row_when_pane_output_is_unavailable() {
    let view = agent(AgentKind::Codex, "Herdr", Some("session-1"));
    let evidence = derive_activity(&view, &history("session-1", AgentKind::Codex));
    assert!(evidence.summary.contains("cargo test"));
    assert_eq!(evidence.activity_source, "Codex session file");
    assert_eq!(evidence.state_source, "Herdr reported status");
    assert!(evidence.recorded);
}
