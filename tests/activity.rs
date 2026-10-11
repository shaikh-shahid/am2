use am2::activity::{output_fingerprint, summarize};
use am2::domain::AgentState;

#[test]
fn working_commands_become_short_activity_labels() {
    assert_eq!(
        summarize(AgentState::Working, "$ cargo test"),
        "running tests"
    );
    assert_eq!(
        summarize(AgentState::Working, "Compiling am2 v0.1"),
        "building project"
    );
    assert_eq!(
        summarize(AgentState::Working, "*** Update File: src/main.rs"),
        "editing src/main.rs"
    );
}

#[test]
fn waiting_and_completion_are_not_described_as_still_running() {
    assert_eq!(
        summarize(AgentState::Waiting, "Approve this command? (y/n)"),
        "permission requested"
    );
    assert_eq!(
        summarize(AgentState::Waiting, "cargo test"),
        "waiting for input"
    );
    assert_eq!(
        summarize(
            AgentState::Finished,
            "cargo test\ntest result: ok. 3 passed"
        ),
        "tests passed"
    );
}

#[test]
fn fallback_uses_last_meaningful_line_and_is_bounded() {
    assert_eq!(
        summarize(
            AgentState::Working,
            "\u{1b}[31m│\u{1b}[0m\nInvestigating a compile error\n"
        ),
        "Investigating a compile error"
    );
    assert!(
        summarize(AgentState::Working, &"x".repeat(200))
            .chars()
            .count()
            <= 80
    );
    assert_eq!(summarize(AgentState::Unknown, "\n│\n"), "activity unknown");
}

#[test]
fn output_fingerprint_changes_only_when_output_changes() {
    assert_eq!(
        output_fingerprint("cargo test"),
        output_fingerprint("cargo test")
    );
    assert_ne!(
        output_fingerprint("cargo test"),
        output_fingerprint("cargo check")
    );
}

#[test]
fn old_test_command_does_not_override_newer_narrative() {
    assert_eq!(
        summarize(
            AgentState::Working,
            "cargo test\nInvestigating a compile error"
        ),
        "Investigating a compile error"
    );
}

#[test]
fn historical_permission_prompt_does_not_override_current_state() {
    assert_eq!(
        summarize(AgentState::Finished, "Approve this command? (y/n)"),
        "finished"
    );
    assert_eq!(
        summarize(
            AgentState::Working,
            "Approve this command? (y/n)\ncargo test"
        ),
        "running tests"
    );
}
