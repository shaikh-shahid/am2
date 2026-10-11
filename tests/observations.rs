use std::collections::HashMap;

use am2::domain::{AgentKind, AgentState, Runtime};
use am2::herdr::{observations_from_json, HerdrRuntime};

const SNAPSHOT: &str = r#"{"result":{"snapshot":{"panes":[
  {"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","terminal_id":"term_a","agent":"codex","agent_status":"working","cwd":"/old","foreground_cwd":"/fallback"},
  {"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1","terminal_id":"term_b","agent":"claude","agent_status":"idle"}
]}}}"#;

#[test]
fn verified_agents_become_runtime_neutral_observations() {
    let processes = HashMap::from([
        ("w1:p1".into(), r#"{"result":{"process_info":{"foreground_processes":[{"pid":42,"argv0":"/opt/homebrew/bin/codex","argv":["codex"],"cwd":"/Projects/am2"}]}}}"#.into()),
        ("w1:p2".into(), r#"{"result":{"process_info":{"foreground_processes":[{"pid":43,"argv0":"claude","argv":["claude"]}]}}}"#.into()),
    ]);
    let output = HashMap::from([("w1:p1".into(), "cargo test\ntest result: ok".into())]);
    let observations = observations_from_json(SNAPSHOT, &processes, &output).unwrap();

    assert_eq!(observations.len(), 2);
    let agent = &observations[0];
    assert_eq!(agent.id, "herdr:term_a:42");
    assert_eq!(agent.kind, AgentKind::Codex);
    assert_eq!(agent.project, "am2");
    assert_eq!(agent.cwd, "/Projects/am2");
    assert_eq!(agent.state, AgentState::Working);
    assert_eq!(agent.recent_output, "cargo test\ntest result: ok");
    assert_eq!(agent.runtime.provider, "Herdr");
    assert_eq!(agent.runtime.target, "w1:p1");
    assert!(agent
        .runtime
        .fields
        .iter()
        .any(|(key, value)| key == "Workspace" && value == "w1"));
    assert_eq!(observations[1].kind, AgentKind::Claude);
    assert_eq!(observations[1].id, "herdr:term_b:43");
    assert_eq!(observations[1].state, AgentState::Waiting);
}

#[test]
fn claude_agents_inventory_command_is_not_a_live_pane_agent() {
    let snapshot = r#"{"result":{"snapshot":{"panes":[{"pane_id":"w1:p15","tab_id":"w1:tM","workspace_id":"w1","terminal_id":"term_am2","cwd":"/Projects/am2"}]}}}"#;
    let processes = HashMap::from([(
        "w1:p15".into(),
        r#"{"result":{"process_info":{"foreground_processes":[{"pid":14573,"argv0":"claude","argv":["claude","agents","--json"],"cwd":"/Projects/am2"}]}}}"#.into(),
    )]);
    assert!(
        observations_from_json(snapshot, &processes, &HashMap::new())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn open_code_is_verified_from_process_and_empty_snapshot_is_clean() {
    let snapshot = r#"{"result":{"snapshot":{"panes":[{"pane_id":"p1","tab_id":"t1","workspace_id":"w1","agent":"opencode","agent_status":"working"},{"pane_id":"p2","tab_id":"t1","workspace_id":"w1","agent":"opencode"}]}}}"#;
    let processes = HashMap::from([
        ("p1".into(), r#"{"result":{"process_info":{"foreground_processes":[{"pid":71,"argv0":"/Users/me/.opencode/bin/opencode","argv":["opencode"],"cwd":"/Projects/api"}]}}}"#.into()),
        ("p2".into(), r#"{"result":{"process_info":{"foreground_processes":[{"pid":72,"argv0":"opencode","argv":["opencode","serve"],"cwd":"/Projects/api"}]}}}"#.into()),
    ]);
    let observations = observations_from_json(snapshot, &processes, &HashMap::new()).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].kind, AgentKind::OpenCode);
    assert_eq!(observations[0].project, "api");
    let empty = r#"{"result":{"snapshot":{"panes":[]}}}"#;
    assert!(
        observations_from_json(empty, &HashMap::new(), &HashMap::new())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn pi_pane_uses_herdr_status_and_can_be_focused() {
    let snapshot = r#"{"result":{"snapshot":{"panes":[{"pane_id":"w1:p3","tab_id":"w1:t1","workspace_id":"w1","agent":"pi","agent_status":"working"}]}}}"#;
    let processes = HashMap::from([("w1:p3".into(), r#"{"result":{"process_info":{"foreground_processes":[{"pid":55,"argv0":"pi","argv":["pi"],"cwd":"/Projects/pi"}]}}}"#.into())]);
    let observations = observations_from_json(snapshot, &processes, &HashMap::new()).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].kind, AgentKind::Pi);
    assert_eq!(observations[0].state, AgentState::Working);
    assert_eq!(observations[0].runtime.target, "w1:p3");
}

#[test]
fn recent_output_is_bounded_to_eighty_lines_and_sixteen_kib() {
    let processes = HashMap::from([("w1:p1".into(), r#"{"result":{"process_info":{"foreground_processes":[{"pid":42,"argv0":"codex","argv":["codex"]}]}}}"#.into())]);
    let long = (0..200)
        .map(|n| format!("line {n}: {}", "x".repeat(300)))
        .collect::<Vec<_>>()
        .join("\n");
    let output = HashMap::from([("w1:p1".into(), long)]);
    let observations = observations_from_json(SNAPSHOT, &processes, &output).unwrap();

    assert!(observations[0].recent_output.len() <= 16 * 1024);
    assert!(observations[0].recent_output.lines().count() <= 80);
    assert!(observations[0].recent_output.contains("line 199"));
    assert!(!observations[0].recent_output.contains("line 0:"));
}

#[test]
fn herdr_adapter_implements_runtime() {
    fn assert_runtime<T: Runtime>() {}
    assert_runtime::<HerdrRuntime>();
}

#[test]
fn pane_control_sequences_are_removed_before_display() {
    let processes = HashMap::from([(
        "w1:p1".into(),
        r#"{"result":{"process_info":{"foreground_processes":[{"pid":42,"argv0":"codex"}]}}}"#
            .into(),
    )]);
    let output = HashMap::from([(
        "w1:p1".into(),
        "\u{1b}[31mred\u{1b}[0m\nline\u{0}two".into(),
    )]);
    let observations = observations_from_json(SNAPSHOT, &processes, &output).unwrap();
    assert_eq!(observations[0].recent_output, "red\nlinetwo");
}
