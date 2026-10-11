use std::fs;
use std::path::PathBuf;

use am2::domain::AgentKind;
use am2::history::{scan_history, scan_roots, search_sessions, HistoryEventKind};

fn fixture_root(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("am2-history-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn extracts_conversation_and_tool_events_from_three_agents() {
    let root = fixture_root("formats");
    let codex = root.join("codex");
    let claude = root.join("claude");
    let pi = root.join("pi");
    for directory in [&codex, &claude, &pi] {
        fs::create_dir_all(directory).unwrap();
    }
    fs::write(codex.join("a.jsonl"), concat!(
        "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-09T01:00:00Z\",\"payload\":{\"id\":\"codex-id\",\"cwd\":\"/Projects/am2\"}}\n",
        "{\"type\":\"response_item\",\"timestamp\":\"2026-10-09T01:00:01Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"fix the comet bug\"}]}}\n",
        "{\"type\":\"response_item\",\"timestamp\":\"2026-10-09T01:00:02Z\",\"payload\":{\"type\":\"function_call\",\"name\":\"shell\",\"arguments\":\"{\\\"cmd\\\":\\\"cargo test\\\"}\"}}\n",
        "{broken json\n",
        "{\"type\":\"response_item\",\"timestamp\":\"2026-10-09T01:00:03Z\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Comet fixed\"}]}}\n"
    )).unwrap();
    fs::write(claude.join("b.jsonl"), concat!(
        "{\"type\":\"user\",\"timestamp\":\"2026-10-09T02:00:00Z\",\"sessionId\":\"claude-id\",\"cwd\":\"/Projects/backend\",\"message\":{\"role\":\"user\",\"content\":\"check the worker\"}}\n",
        "{\"type\":\"assistant\",\"timestamp\":\"2026-10-09T02:00:01Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Worker reviewed\"},{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"file_path\":\"src/worker.rs\"}}]}}\n"
    )).unwrap();
    fs::write(pi.join("c.jsonl"), concat!(
        "{\"type\":\"session\",\"id\":\"pi-id\",\"cwd\":\"/Projects/mobile\",\"timestamp\":\"2026-10-09T03:00:00Z\"}\n",
        "{\"type\":\"message\",\"timestamp\":\"2026-10-09T03:00:01Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Mobile build passed\"},{\"type\":\"toolCall\",\"name\":\"bash\",\"arguments\":{\"command\":\"npm test\"}}]}}\n"
    )).unwrap();
    let inventory = scan_roots(&[
        (AgentKind::Codex, codex),
        (AgentKind::Claude, claude),
        (AgentKind::Pi, pi),
    ])
    .unwrap();
    assert_eq!(inventory.sessions.len(), 3);
    let codex = inventory
        .sessions
        .iter()
        .find(|s| s.id == "codex-id")
        .unwrap();
    assert_eq!(codex.project, "am2");
    assert_eq!(codex.events.len(), 3);
    assert_eq!(codex.events[1].kind, HistoryEventKind::Tool);
    assert!(codex.events[1].text.contains("cargo test"));
    assert_eq!(
        inventory
            .sessions
            .iter()
            .find(|s| s.id == "claude-id")
            .unwrap()
            .events
            .len(),
        3
    );
    assert_eq!(
        inventory
            .sessions
            .iter()
            .find(|s| s.id == "pi-id")
            .unwrap()
            .events
            .len(),
        2
    );
    let comet_hits = search_sessions(&inventory.sessions, "comet");
    assert_eq!(comet_hits.len(), 1);
    assert_eq!(comet_hits[0].match_count, 2);
    assert_eq!(comet_hits[0].event_index, Some(2));
    assert_eq!(
        inventory.sessions[comet_hits[0].session_index].id,
        "codex-id"
    );
    let worker_hits = search_sessions(&inventory.sessions, "worker");
    assert!(!worker_hits.is_empty());
    assert!(worker_hits
        .iter()
        .all(|hit| inventory.sessions[hit.session_index].id == "claude-id"));
    assert_eq!(
        search_sessions(&inventory.sessions, "build passed").len(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ignores_unidentified_sessions_and_does_not_guess_from_path() {
    let root = fixture_root("no-id");
    fs::write(root.join("looks-like-session.jsonl"), "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"text\":\"hello\"}]}}\n").unwrap();
    let inventory = scan_roots(&[(AgentKind::Codex, root.clone())]).unwrap();
    assert!(inventory.sessions.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skips_oversized_files_without_reading_them() {
    let root = fixture_root("large");
    let file = fs::File::create(root.join("huge.jsonl")).unwrap();
    file.set_len(25 * 1024 * 1024).unwrap();
    let inventory = scan_roots(&[(AgentKind::Codex, root.clone())]).unwrap();
    assert!(inventory.sessions.is_empty());
    assert_eq!(inventory.skipped_files, 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn caps_number_of_files_even_when_every_file_has_a_session() {
    let root = fixture_root("file-cap");
    for index in 0..121 {
        fs::write(
            root.join(format!("{index:03}.jsonl")),
            format!("{{\"type\":\"session\",\"id\":\"pi-{index}\",\"cwd\":\"/Projects/am2\"}}\n"),
        )
        .unwrap();
    }
    let inventory = scan_roots(&[(AgentKind::Pi, root.clone())]).unwrap();
    assert_eq!(inventory.sessions.len(), 120);
    assert_eq!(inventory.skipped_files, 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_files_still_count_toward_scan_limit() {
    let root = fixture_root("invalid-cap");
    fs::write(
        root.join("valid.jsonl"),
        "{\"type\":\"session\",\"id\":\"old\",\"cwd\":\"/Projects/am2\"}\n",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    for index in 0..120 {
        fs::write(root.join(format!("new-{index:03}.jsonl")), "not json\n").unwrap();
    }
    let inventory = scan_roots(&[(AgentKind::Pi, root.clone())]).unwrap();
    assert!(inventory.sessions.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "reads this machine's private local agent history"]
fn local_history_smoke() {
    let inventory = scan_history().unwrap();
    assert!(!inventory.sessions.is_empty());
    assert!(inventory
        .sessions
        .iter()
        .any(|session| !session.events.is_empty()));
    println!(
        "sessions={} skipped={}",
        inventory.sessions.len(),
        inventory.skipped_files
    );
}
