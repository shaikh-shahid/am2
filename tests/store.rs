use am2::domain::{AgentKind, AgentObservation, AgentState, RuntimeMetadata};
use am2::store::StateStore;

fn agent(output: &str) -> AgentObservation {
    AgentObservation {
        id: "herdr:term_a:42".into(),
        kind: AgentKind::Codex,
        project: "am2".into(),
        state: AgentState::Working,
        pid: 42,
        cwd: "/Projects/am2".into(),
        runtime: RuntimeMetadata {
            provider: "Herdr".into(),
            target: "w1:p1".into(),
            fields: vec![],
        },
        recent_output: output.into(),
        output_error: None,
    }
}

#[test]
fn tracks_output_changes_without_creating_attention_items() {
    let mut store = StateStore::memory().unwrap();
    let first = store.apply_refresh(Ok(vec![agent("old")]), 10).unwrap();
    assert_eq!(first[0].last_output_change, None);
    let changed = store.apply_refresh(Ok(vec![agent("new")]), 11).unwrap();
    assert_eq!(changed[0].last_output_change, Some(11));
}

#[test]
fn old_attention_rows_are_left_intact() {
    let path = std::env::temp_dir().join(format!("am2-store-{}.sqlite3", std::process::id()));
    let _ = std::fs::remove_file(&path);
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE inbox_items(id INTEGER PRIMARY KEY, kind TEXT); INSERT INTO inbox_items(id,kind) VALUES (7,'Waiting');").unwrap();
    }
    let mut store = StateStore::open(&path).unwrap();
    store.apply_refresh(Ok(vec![agent("work")]), 10).unwrap();
    let mut waiting = agent("prompt");
    waiting.state = AgentState::Waiting;
    store.apply_refresh(Ok(vec![waiting]), 11).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM inbox_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    drop(connection);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn session_alias_survives_store_reopen_and_can_be_cleared() {
    let path = std::env::temp_dir().join(format!(
        "am2-alias-{}-{}.sqlite3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let store = StateStore::open(&path).unwrap();
        store.set_alias("Codex:session:abc", "tests").unwrap();
        assert_eq!(
            store.alias("Codex:session:abc").unwrap().as_deref(),
            Some("tests")
        );
    }
    {
        let store = StateStore::open(&path).unwrap();
        assert_eq!(
            store.alias("Codex:session:abc").unwrap().as_deref(),
            Some("tests")
        );
        store.set_alias("Codex:session:abc", "").unwrap();
        assert!(store.alias("Codex:session:abc").unwrap().is_none());
    }
    std::fs::remove_file(path).unwrap();
}
