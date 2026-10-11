use am2::domain::{AgentKind, AgentObservation, AgentState, AgentView, RuntimeMetadata};
use am2::history::{HistoryEvent, HistoryEventKind, HistoryInventory, HistorySession};
use am2::search::{SearchIndex, SearchTarget};

fn live() -> AgentView {
    AgentView {
        observation: AgentObservation {
            id: "macos:101:started".into(),
            kind: AgentKind::Codex,
            project: "am2".into(),
            state: AgentState::Unknown,
            pid: 101,
            cwd: "/Projects/am2".into(),
            runtime: RuntimeMetadata {
                provider: "macOS".into(),
                target: "PID 101".into(),
                fields: vec![],
            },
            recent_output: "cargo test failed in search.rs".into(),
            output_error: None,
        },
        activity: "running tests".into(),
        last_output_change: Some(100),
    }
}

fn history() -> HistoryInventory {
    HistoryInventory {
        sessions: vec![HistorySession {
            kind: AgentKind::Claude,
            id: "exact-session".into(),
            cwd: "/Projects/backend".into(),
            project: "backend".into(),
            updated_at: 200,
            events: vec![HistoryEvent {
                timestamp: None,
                kind: HistoryEventKind::Assistant,
                text: "Fixed authentication failure in middleware".into(),
            }],
        }],
        skipped_files: 0,
    }
}

#[test]
fn searches_live_and_saved_events_with_exact_navigation_targets() {
    let mut index = SearchIndex::new().unwrap();
    index.replace_live(&[live()]).unwrap();
    index.replace_history(&history()).unwrap();
    let live_hits = index.query("cargo test").unwrap();
    assert!(live_hits
        .iter()
        .any(|hit| hit.target == SearchTarget::Live("macos:101:started".into())));
    let saved_hits = index.query("authentication failure").unwrap();
    assert!(saved_hits.iter().any(|hit| hit.target
        == SearchTarget::Saved {
            kind: AgentKind::Claude,
            session_id: "exact-session".into(),
            event_index: 0,
        }));
}

#[test]
fn project_and_agent_filters_restrict_results() {
    let mut index = SearchIndex::new().unwrap();
    index.replace_live(&[live()]).unwrap();
    index.replace_history(&history()).unwrap();
    assert!(index.query("test agent:claude").unwrap().is_empty());
    assert!(index.query("failure project:am2").unwrap().is_empty());
    assert_eq!(
        index
            .query("failure PROJECT:Back AGENT:CLAUDE")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn index_replacement_removes_stale_live_and_saved_results() {
    let mut index = SearchIndex::new().unwrap();
    index.replace_live(&[live()]).unwrap();
    index.replace_history(&history()).unwrap();
    index.replace_live(&[]).unwrap();
    index.replace_history(&HistoryInventory::default()).unwrap();
    assert!(index.query("cargo").unwrap().is_empty());
    assert!(index.query("authentication").unwrap().is_empty());
}

#[test]
fn user_query_is_escaped_before_fts_evaluation() {
    let mut index = SearchIndex::new().unwrap();
    index.replace_live(&[live()]).unwrap();
    assert!(index
        .query("\" OR DROP TABLE documents")
        .unwrap()
        .is_empty());
    assert_eq!(index.query("cargo").unwrap().len(), 1);
    for query in [
        "?",
        ":",
        "-",
        "/",
        "*",
        "\"",
        "'",
        "foo:bar",
        "src/search.rs",
    ] {
        assert!(index.query(query).is_ok(), "query should be safe: {query}");
    }
}

#[test]
#[ignore = "requires local saved sessions"]
fn local_history_scan_and_index_smoke() {
    let history = am2::history::scan_history().unwrap();
    let count = history
        .sessions
        .iter()
        .map(|session| session.events.len())
        .sum::<usize>();
    let started = std::time::Instant::now();
    let mut index = SearchIndex::new().unwrap();
    index.replace_history(&history).unwrap();
    let elapsed = started.elapsed();
    assert!(index.query("test").is_ok());
    println!("Indexed {count} bounded events in {elapsed:?}");
}
