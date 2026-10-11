use am2::ghostty::parse_terminals;
use am2::store::StateStore;

#[test]
fn parses_exact_ghostty_terminal_identity() {
    let terminals =
        parse_terminals("41\u{1f}A1B2-C3D4\u{1f}Codex\u{1f}/Projects/am2\u{1f}W1\u{1f}AM2\u{1e}\n")
            .unwrap();
    assert_eq!(terminals[0].id, "A1B2-C3D4");
    assert_eq!(terminals[0].cwd, "/Projects/am2");
    assert_eq!(terminals[0].window_id, "W1");
    assert!(parse_terminals("\n").unwrap().is_empty());
}

#[test]
fn link_is_scoped_to_agent_process_identity() {
    let store = StateStore::memory().unwrap();
    store
        .link_ghostty("native:42:start-a", "41:A1B2-C3D4")
        .unwrap();
    assert_eq!(
        store.ghostty_link("native:42:start-a").unwrap().as_deref(),
        Some("41:A1B2-C3D4")
    );
    assert!(store.ghostty_link("native:42:start-b").unwrap().is_none());
}

#[test]
fn ghostty_pairing_survives_am2_restart() {
    let path = std::env::temp_dir().join(format!(
        "am2-ghostty-link-{}-{}.sqlite3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    StateStore::open(&path)
        .unwrap()
        .link_ghostty("native:42:start-a", "41:A1B2-C3D4")
        .unwrap();
    let store = StateStore::open(&path).unwrap();
    assert_eq!(
        store.ghostty_link("native:42:start-a").unwrap().as_deref(),
        Some("41:A1B2-C3D4")
    );
    assert_eq!(
        store.terminal_link("native:42:start-a").unwrap().as_deref(),
        Some("ghostty:41:A1B2-C3D4")
    );
    store
        .link_terminal("native:42:start-a", "iterm2:50:session-abc")
        .unwrap();
    assert_eq!(
        store.terminal_link("native:42:start-a").unwrap().as_deref(),
        Some("iterm2:50:session-abc")
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}
