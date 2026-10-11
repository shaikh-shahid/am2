use am2::domain::{AgentKind, Runtime};
use am2::herdr::HerdrRuntime;

/// Manual integration smoke test for a macOS host running Herdr.
#[test]
#[ignore = "requires a live Herdr session"]
fn observes_only_verified_agent_processes() {
    let agents = HerdrRuntime.observe().unwrap();
    eprintln!("observed {} agents", agents.len());
    for agent in agents {
        assert!(matches!(
            agent.kind,
            AgentKind::Codex | AgentKind::Claude | AgentKind::OpenCode
        ));
        assert!(agent.id.starts_with("herdr:"));
        assert_eq!(agent.runtime.provider, "Herdr");
        assert!(agent.recent_output.len() <= 16 * 1024);
    }
}
