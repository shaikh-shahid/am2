use am2::domain::{AgentKind, AgentObservation, AgentState, AgentView, RuntimeMetadata};
use am2::history::{HistoryEvent, HistoryEventKind, HistoryInventory, HistorySession};
use am2::projects::{ProjectAgent, ProjectSnapshot};
use am2::ui::{DashboardState, Screen};

fn agent(id: &str, state: AgentState) -> AgentView {
    AgentView {
        observation: AgentObservation {
            id: id.into(),
            kind: AgentKind::Codex,
            project: "am2".into(),
            state,
            pid: 42,
            cwd: "/Projects/am2".into(),
            runtime: RuntimeMetadata {
                provider: "Herdr".into(),
                target: "w1:p1".into(),
                fields: vec![("Workspace".into(), "w1".into())],
            },
            recent_output: "cargo test".into(),
            output_error: None,
        },
        activity: "running tests".into(),
        last_output_change: None,
    }
}

#[test]
fn selection_tracks_agent_identity_when_refresh_reorders_rows() {
    let mut state = DashboardState::new(vec![
        agent("a", AgentState::Working),
        agent("b", AgentState::Waiting),
    ]);
    state.next();
    assert_eq!(state.selected_agent().unwrap().observation.id, "b");
    state.set_agents(vec![
        agent("b", AgentState::Working),
        agent("c", AgentState::Working),
    ]);
    assert_eq!(state.selected_agent().unwrap().observation.id, "b");
    state.set_agents(vec![]);
    assert!(state.selected_agent().is_none());
}

#[test]
fn dashboard_keeps_project_order_when_observers_return_in_different_orders() {
    let mut alpha = agent("alpha", AgentState::Working);
    alpha.observation.project = "alpha".into();
    let mut zeta = agent("zeta", AgentState::Working);
    zeta.observation.project = "zeta".into();
    let mut state = DashboardState::new(vec![]);
    state.set_agents(vec![zeta.clone(), alpha.clone()]);
    assert_eq!(state.visible_agents()[0].observation.id, "alpha");
    state.set_agents(vec![alpha, zeta]);
    assert_eq!(state.visible_agents()[0].observation.id, "alpha");
}

#[test]
fn inspection_returns_to_live_agents() {
    let mut state = DashboardState::new(vec![agent("a", AgentState::Waiting)]);
    state.inspect();
    assert_eq!(state.screen(), Screen::InspectAgent("a".into()));
    state.back();
    assert_eq!(state.screen(), Screen::Dashboard);
}

#[test]
fn failed_refresh_marks_existing_rows_stale_without_erasing_them() {
    let mut state = DashboardState::new(vec![agent("a", AgentState::Working)]);
    state.mark_error("Herdr unavailable".into());
    assert!(state.is_stale());
    assert_eq!(state.selected_agent().unwrap().observation.id, "a");
    state.clear_error();
    assert!(!state.is_stale());
}

#[test]
fn focus_target_resolves_selected_live_agent_from_details() {
    let mut state = DashboardState::new(vec![agent("live", AgentState::Working)]);
    assert_eq!(state.focus_target(), Some("w1:p1"));
    state.inspect();
    assert_eq!(state.focus_target(), Some("w1:p1"));
    state.set_agents(vec![]);
    assert_eq!(state.focus_target(), None);
}

#[test]
fn stale_snapshot_cannot_be_used_to_focus_a_pane() {
    let mut state = DashboardState::new(vec![agent("live", AgentState::Working)]);
    state.mark_error("Herdr unavailable".into());
    assert_eq!(state.focus_target(), None);
}

#[test]
fn live_search_filters_kind_project_and_cwd_without_losing_selected_identity() {
    let mut pi = agent("pi", AgentState::Unknown);
    pi.observation.kind = AgentKind::Pi;
    pi.observation.project = "LLM_Core".into();
    pi.observation.cwd = "/Projects/LLM_Core".into();
    let mut state = DashboardState::new(vec![agent("codex", AgentState::Working), pi.clone()]);
    state.start_search();
    for ch in "llm".chars() {
        state.search_push(ch);
    }
    assert_eq!(state.visible_agents().len(), 1);
    assert_eq!(state.selected_agent().unwrap().observation.id, "pi");
    state.set_agents(vec![pi, agent("codex", AgentState::Working)]);
    assert_eq!(state.selected_agent().unwrap().observation.id, "pi");
    state.inspect();
    assert_eq!(state.screen(), Screen::InspectAgent("pi".into()));
    state.back();
    state.clear_search();
    assert_eq!(state.visible_agents().len(), 2);
}

#[test]
fn search_with_no_matches_cannot_focus_hidden_agent() {
    let mut state = DashboardState::new(vec![agent("codex", AgentState::Working)]);
    state.start_search();
    for ch in "absent".chars() {
        state.search_push(ch);
    }
    assert!(state.visible_agents().is_empty());
    assert!(state.selected_agent().is_none());
    assert_eq!(state.focus_target(), None);
    state.search_backspace();
    assert_eq!(state.search_query(), Some("absen"));
}

#[test]
fn search_matches_agent_kind_and_cwd_case_insensitively() {
    let mut pi = agent("pi", AgentState::Unknown);
    pi.observation.kind = AgentKind::Pi;
    pi.observation.cwd = "/Projects/LLM_Core".into();
    let mut state = DashboardState::new(vec![agent("codex", AgentState::Working), pi]);
    state.start_search();
    for ch in "PI CORE".chars() {
        state.search_push(ch);
    }
    assert_eq!(state.selected_agent().unwrap().observation.id, "pi");
}

fn saved_session(kind: AgentKind, id: &str, project: &str, text: &str) -> HistorySession {
    HistorySession {
        kind,
        id: id.into(),
        cwd: format!("/Projects/{project}"),
        project: project.into(),
        updated_at: 100,
        events: vec![HistoryEvent {
            timestamp: Some("2026-10-09T00:00:00Z".into()),
            kind: HistoryEventKind::Assistant,
            text: text.into(),
        }],
    }
}

#[test]
fn history_search_finds_conversation_text_separately_from_live_agents() {
    let mut state = DashboardState::new(vec![agent("live", AgentState::Unknown)]);
    state.set_history(HistoryInventory {
        sessions: vec![
            saved_session(AgentKind::Codex, "old-codex", "am2", "fixed comet parser"),
            saved_session(AgentKind::Pi, "old-pi", "mobile", "build passed"),
        ],
        skipped_files: 0,
    });
    state.open_history();
    assert_eq!(state.screen(), Screen::History);
    state.start_search();
    for ch in "comet".chars() {
        state.search_push(ch);
    }
    assert_eq!(state.visible_history_hits().len(), 1);
    state.inspect();
    assert_eq!(state.screen(), Screen::InspectHistory(0, Some(0)));
    state.back();
    assert_eq!(state.screen(), Screen::History);
    state.back();
    assert_eq!(state.screen(), Screen::Dashboard);
    assert_eq!(state.selected_agent().unwrap().observation.id, "live");
}

#[test]
fn live_agent_opens_saved_history_only_for_exact_session_id() {
    let mut live = agent("live", AgentState::Unknown);
    live.observation.kind = AgentKind::Claude;
    let mut state = DashboardState::new(vec![live.clone()]);
    state.set_history(HistoryInventory {
        sessions: vec![saved_session(
            AgentKind::Claude,
            "claude-id",
            "am2",
            "review done",
        )],
        skipped_files: 0,
    });
    state.inspect();
    assert!(!state.open_exact_history_for_agent());
    live.observation
        .runtime
        .fields
        .push(("Session ID".into(), "claude-id".into()));
    state.set_agents(vec![live]);
    assert!(state.open_exact_history_for_agent());
    assert_eq!(state.screen(), Screen::InspectHistory(0, None));
}

#[test]
fn history_reload_does_not_show_a_different_session_in_existing_detail() {
    let mut state = DashboardState::new(vec![]);
    state.set_history(HistoryInventory {
        sessions: vec![saved_session(AgentKind::Codex, "one", "a", "first")],
        skipped_files: 0,
    });
    state.open_history();
    state.inspect();
    assert_eq!(state.screen(), Screen::InspectHistory(0, None));
    state.set_history(HistoryInventory {
        sessions: vec![saved_session(AgentKind::Codex, "two", "b", "second")],
        skipped_files: 0,
    });
    assert_eq!(state.screen(), Screen::History);
}

fn project(root: &str, agent_id: &str) -> ProjectSnapshot {
    ProjectSnapshot {
        root: root.into(),
        name: root.rsplit('/').next().unwrap().into(),
        git: true,
        branch: Some("main".into()),
        latest_commit: Some("abc123 Initial commit".into()),
        change_count: 1,
        changes: vec![],
        agents: vec![ProjectAgent {
            id: agent_id.into(),
            kind: AgentKind::Codex,
            state: AgentState::Working,
            activity: "editing src/main.rs".into(),
        }],
        test_evidence: None,
        next_step: "Review changes and run project tests".into(),
        error: None,
    }
}

#[test]
fn projects_open_from_live_agent_and_preserve_selection_after_refresh() {
    let mut state = DashboardState::new(vec![agent("a", AgentState::Working)]);
    state.set_projects(vec![
        project("/Projects/other", "other"),
        project("/Projects/am2", "a"),
    ]);
    state.open_projects();
    assert_eq!(state.screen(), Screen::Projects);
    assert_eq!(state.selected_project().unwrap().root, "/Projects/am2");
    state.inspect();
    assert_eq!(
        state.screen(),
        Screen::InspectProject("/Projects/am2".into())
    );
    state.set_projects(vec![
        project("/Projects/am2", "a"),
        project("/Projects/other", "other"),
    ]);
    assert_eq!(
        state.screen(),
        Screen::InspectProject("/Projects/am2".into())
    );
    state.back();
    assert_eq!(state.screen(), Screen::Projects);
    state.back();
    assert_eq!(state.screen(), Screen::Dashboard);
}

#[test]
fn missing_project_on_refresh_returns_to_project_list() {
    let mut state = DashboardState::new(vec![]);
    state.set_projects(vec![project("/Projects/am2", "a")]);
    state.open_projects();
    state.inspect();
    state.set_projects(vec![]);
    assert_eq!(state.screen(), Screen::Projects);
    assert!(state.selected_project().is_none());
}

#[test]
fn project_refresh_keeps_manual_selection_after_opening_from_agent() {
    let mut state = DashboardState::new(vec![agent("a", AgentState::Working)]);
    state.set_projects(vec![
        project("/Projects/am2", "a"),
        project("/Projects/other", "b"),
    ]);
    state.open_projects();
    state.next();
    assert_eq!(state.selected_project().unwrap().root, "/Projects/other");
    state.set_projects(vec![
        project("/Projects/other", "b"),
        project("/Projects/am2", "a"),
    ]);
    assert_eq!(state.selected_project().unwrap().root, "/Projects/other");
}

#[test]
fn projects_opened_from_agent_detail_return_to_that_agent() {
    let mut state = DashboardState::new(vec![agent("a", AgentState::Working)]);
    state.set_projects(vec![project("/Projects/am2", "a")]);
    state.inspect();
    state.open_projects();
    state.inspect();
    state.back();
    assert_eq!(state.screen(), Screen::Projects);
    state.back();
    assert_eq!(state.screen(), Screen::InspectAgent("a".into()));
}
