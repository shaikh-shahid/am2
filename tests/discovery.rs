use std::collections::HashMap;

use am2::herdr::codex_panes_from_json;

fn snapshot() -> &'static str {
    r#"{"result":{"snapshot":{"panes":[
        {"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"/old","foreground_cwd":"/fresh","agent":"codex","agent_status":"working","focused":true,"agent_session":{"agent":"claude","kind":"id","source":"herdr:claude","value":"wrong-agent"}},
        {"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1","cwd":"/claude","agent":"claude","agent_status":"idle"},
        {"pane_id":"w1:p3","tab_id":"w1:t2","workspace_id":"w1","cwd":"/service","agent":"codex","agent_status":"working"}
    ]}}}"#
}

fn process_json(pane_id: &str, pid: u32, argv: &[&str], cwd: &str) -> String {
    serde_json::json!({"result":{"process_info":{
        "pane_id":pane_id,"shell_pid":1,"foreground_processes":[{
            "pid":pid,"name":argv[0],"argv0":argv[0],"argv":argv,"cwd":cwd
        }]
    }}})
    .to_string()
}

#[test]
fn includes_only_live_codex_cli_and_uses_process_cwd() {
    let mut processes = HashMap::new();
    processes.insert(
        "w1:p1".into(),
        process_json("w1:p1", 42, &["codex"], "/project"),
    );
    processes.insert(
        "w1:p2".into(),
        process_json("w1:p2", 43, &["claude"], "/claude"),
    );
    processes.insert(
        "w1:p3".into(),
        process_json("w1:p3", 44, &["codex", "app-server"], "/service"),
    );

    let panes = codex_panes_from_json(snapshot(), &processes).unwrap();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].pane_id, "w1:p1");
    assert_eq!(panes[0].pid, 42);
    assert_eq!(panes[0].cwd, "/project");
    assert_eq!(panes[0].mode, "interactive");
    assert_eq!(panes[0].status, "working");
    assert!(!serde_json::to_string(&panes[0])
        .unwrap()
        .contains("wrong-agent"));
}

#[test]
fn missing_process_info_does_not_turn_stale_agent_label_into_live_session() {
    let panes = codex_panes_from_json(snapshot(), &HashMap::new()).unwrap();
    assert!(panes.is_empty());
}

#[test]
fn accepts_codex_resume_after_cwd_flag() {
    let mut processes = HashMap::new();
    processes.insert(
        "w1:p1".into(),
        process_json(
            "w1:p1",
            42,
            &["/opt/homebrew/bin/codex", "-C", "/project", "resume", "abc"],
            "/project",
        ),
    );
    let panes = codex_panes_from_json(snapshot(), &processes).unwrap();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].mode, "resume");
}
