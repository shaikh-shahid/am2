use am2::domain::{AgentKind, AgentObservation, AgentState, Runtime, RuntimeMetadata};
use am2::runtime::CombinedRuntime;

struct Stub(Result<Vec<AgentObservation>, String>);
impl Runtime for Stub {
    fn observe(&self) -> Result<Vec<AgentObservation>, String> {
        self.0.clone()
    }
}

fn agent(pid: u32, provider: &str) -> AgentObservation {
    AgentObservation {
        id: format!("{provider}:{pid}"),
        kind: AgentKind::Codex,
        project: "am2".into(),
        state: AgentState::Unknown,
        pid,
        cwd: "/am2".into(),
        runtime: RuntimeMetadata {
            provider: provider.into(),
            target: "x".into(),
            fields: vec![],
        },
        recent_output: String::new(),
        output_error: None,
    }
}

#[test]
fn herdr_observation_wins_for_the_same_process() {
    let combined = CombinedRuntime::new(
        Stub(Ok(vec![agent(1, "Herdr")])),
        Stub(Ok(vec![agent(1, "macOS"), agent(2, "macOS")])),
    );
    let agents = combined.observe().unwrap();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].runtime.provider, "Herdr");
}

#[test]
fn native_observations_survive_missing_herdr() {
    let combined = CombinedRuntime::new(
        Stub(Err("Herdr unavailable".into())),
        Stub(Ok(vec![agent(2, "macOS")])),
    );
    assert_eq!(combined.observe().unwrap().len(), 1);
}

#[test]
fn exact_native_session_id_enriches_matching_herdr_pane() {
    let mut native = agent(1, "macOS");
    native
        .runtime
        .fields
        .push(("Session ID".into(), "codex-thread-1".into()));
    let combined = CombinedRuntime::new(Stub(Ok(vec![agent(1, "Herdr")])), Stub(Ok(vec![native])));
    let agents = combined.observe().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].runtime.provider, "Herdr");
    assert_eq!(
        agents[0]
            .runtime
            .fields
            .iter()
            .find(|(key, _)| key == "Session ID")
            .unwrap()
            .1,
        "codex-thread-1"
    );
}
