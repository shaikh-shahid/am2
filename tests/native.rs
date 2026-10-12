use std::collections::HashMap;

use am2::domain::{AgentKind, AgentState};
use am2::native::codex_session_id_from_lsof;
use am2::native::observations_from_processes;
use std::path::Path;

#[test]
fn discovers_interactive_agents_without_counting_services() {
    let ps = " 10 ttys001 Thu Oct  8 10:00:00 2026 codex\n\
              11 ?? Thu Oct  8 10:00:01 2026 codex app-server daemon\n\
              12 ttys002 Thu Oct  8 10:00:02 2026 pi\n\
              13 ttys003 Thu Oct  8 10:00:03 2026 /opt/homebrew/bin/gemini\n\
              14 ttys004 Thu Oct  8 10:00:04 2026 cursor-agent\n\
              15 ttys005 Thu Oct  8 10:00:05 2026 opencode\n\
              16 ttys006 Thu Oct  8 10:00:06 2026 claude --resume abc\n\
              17 ?? Thu Oct  8 10:00:07 2026 /Applications/Claude.app/Contents/MacOS/Claude\n";
    let cwds = HashMap::from([
        (10, "/Projects/am2".to_string()),
        (12, "/Projects/pi".to_string()),
    ]);
    let observations = observations_from_processes(ps, &cwds, None);
    assert_eq!(observations.len(), 6);
    assert_eq!(
        observations.iter().map(|x| x.kind).collect::<Vec<_>>(),
        vec![
            AgentKind::Codex,
            AgentKind::Pi,
            AgentKind::Gemini,
            AgentKind::Cursor,
            AgentKind::OpenCode,
            AgentKind::Claude
        ]
    );
    assert!(observations.iter().all(|x| x.state == AgentState::Unknown));
    assert_eq!(observations[0].project, "am2");
    assert!(observations[0].id.starts_with("native:10:"));
    assert!(observations[0].recent_output.is_empty());
    assert_eq!(observations[0].runtime.provider, "macOS");
}

#[test]
fn claude_inventory_enriches_exact_live_pid() {
    let ps = " 16 ttys006 Thu Oct  8 10:00:06 2026 claude --resume secret-prompt\n";
    let json = r#"[{"pid":16,"cwd":"/Projects/am2","sessionId":"session-abc","status":"working","kind":"interactive","name":"agent"},{"pid":99,"cwd":"/Projects/other","sessionId":"stale","status":"idle","kind":"interactive"}]"#;
    let observations = observations_from_processes(ps, &HashMap::new(), Some(json));
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].state, AgentState::Working);
    assert_eq!(observations[0].cwd, "/Projects/am2");
    assert_eq!(
        observations[0]
            .runtime
            .fields
            .iter()
            .find(|x| x.0 == "Session ID")
            .unwrap()
            .1,
        "session-abc"
    );
    assert!(!format!("{observations:?}").contains("secret-prompt"));
}

#[test]
fn claude_inventory_idle_and_blocked_statuses_stay_distinct() {
    let ps = " 16 ttys006 Thu Oct  8 10:00:06 2026 claude --resume session-abc\n";
    let idle_json = r#"[{"pid":16,"cwd":"/Projects/am2","sessionId":"session-abc","status":"idle","kind":"interactive"}]"#;
    let blocked_json = r#"[{"pid":16,"cwd":"/Projects/am2","sessionId":"session-abc","status":"blocked","kind":"interactive"}]"#;
    assert_eq!(
        observations_from_processes(ps, &HashMap::new(), Some(idle_json))[0].state,
        AgentState::Idle
    );
    assert_eq!(
        observations_from_processes(ps, &HashMap::new(), Some(blocked_json))[0].state,
        AgentState::Waiting
    );
}

#[test]
fn detached_claude_inventory_session_is_not_a_terminal_agent() {
    let ps = " 16 ?? Thu Oct  8 10:00:06 2026 claude --resume abc\n";
    let json = r#"[{"pid":16,"cwd":"/Projects/doceasy-backend-node","sessionId":"session-vscode","status":"idle","kind":"interactive"}]"#;
    let observations = observations_from_processes(ps, &HashMap::new(), Some(json));
    assert!(observations.is_empty());
}

#[test]
fn recognizes_known_node_entrypoints_without_matching_arbitrary_node_apps() {
    let ps = " 20 ttys001 Thu Oct  8 10:00:00 2026 node /opt/homebrew/Cellar/gemini-cli/0.25.2/bin/gemini\n\
              21 ttys002 Thu Oct  8 10:00:01 2026 node /opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js\n\
              22 ttys003 Thu Oct  8 10:00:02 2026 node /Projects/other/cli.js\n";
    let observations = observations_from_processes(ps, &HashMap::new(), None);
    assert_eq!(
        observations
            .iter()
            .map(|agent| agent.kind)
            .collect::<Vec<_>>(),
        vec![AgentKind::Gemini, AgentKind::Pi]
    );
}

#[test]
fn recognizes_gemini_via_an_installed_bin_symlink() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!(
        "am2-gemini-symlink-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let real = root.join("gemini-cli/0.25.2/bin/gemini");
    let link = root.join("bin/gemini");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::fs::write(&real, "").unwrap();
    symlink(&real, &link).unwrap();
    let ps = format!(
        " 88 ttys001 Thu Oct  8 10:00:00 2026 node {}\n",
        link.display()
    );
    let observations = observations_from_processes(&ps, &HashMap::new(), None);
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].kind, AgentKind::Gemini);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn recognizes_reported_terminal_app_gemini_processes() {
    let ps = "97894 97892 ttys010  Sat Oct 10 16:28:06 2026     /opt/homebrew/opt/node/bin/node /opt/homebrew/bin/gemini\n\
98047 97894 ttys010  Sat Oct 10 16:28:09 2026     /opt/homebrew/Cellar/node/25.4.0/bin/node /opt/homebrew/bin/gemini\n";
    let observations = observations_from_processes(ps, &HashMap::new(), None);
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].kind, AgentKind::Gemini);
    assert_eq!(observations[0].pid, 97894);
}

#[test]
fn codex_thread_id_requires_a_unique_open_file_under_codex_home() {
    let root = Path::new("/Users/test/.codex");
    let id = "01a111f6-08e6-7a42-be23-8d02e22d7928";
    let lock = format!("p123\nn{}/thread-writer-locks/{id}.lock\n", root.display());
    assert_eq!(codex_session_id_from_lsof(&lock, root).as_deref(), Some(id));
    let rollout = format!(
        "n{}/sessions/2026/10/09/rollout-2026-10-09T00-00-00-{id}.jsonl\n",
        root.display()
    );
    assert_eq!(
        codex_session_id_from_lsof(&rollout, root).as_deref(),
        Some(id)
    );
    let outside = format!("n/tmp/thread-writer-locks/{id}.lock\n");
    assert_eq!(codex_session_id_from_lsof(&outside, root), None);
    let ambiguous = format!(
        "{lock}n{}/thread-writer-locks/01a111f6-08e6-7a42-be23-8d02e22d7929.lock\n",
        root.display()
    );
    assert_eq!(codex_session_id_from_lsof(&ambiguous, root), None);
}
