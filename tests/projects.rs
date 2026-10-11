use std::fs;
use std::path::Path;
use std::process::Command;

use am2::domain::{AgentKind, AgentObservation, AgentState, AgentView, RuntimeMetadata};
use am2::projects::{inspect_projects, TestResult};

fn agent(id: &str, cwd: &Path, activity: &str) -> AgentView {
    AgentView {
        observation: AgentObservation {
            id: id.into(),
            kind: AgentKind::Codex,
            project: cwd.file_name().unwrap().to_string_lossy().into_owned(),
            state: AgentState::Working,
            pid: 1,
            cwd: cwd.to_string_lossy().into_owned(),
            runtime: RuntimeMetadata {
                provider: "Herdr".into(),
                target: "pane".into(),
                fields: vec![],
            },
            recent_output: String::new(),
            output_error: None,
        },
        activity: activity.into(),
        last_output_change: None,
    }
}

fn git(directory: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn groups_nested_agents_by_git_root_and_reports_changes_and_commit() {
    let root = std::env::temp_dir().join(format!("am2-projects-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    git(&root, &["init", "-q"]);
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    git(&root, &["add", "src/main.rs"]);
    git(
        &root,
        &[
            "-c",
            "user.name=AM2 Test",
            "-c",
            "user.email=am2@example.invalid",
            "commit",
            "-qm",
            "Initial project",
        ],
    );
    fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"hi\"); }\n",
    )
    .unwrap();
    fs::write(root.join("notes.txt"), "work in progress\n").unwrap();

    let projects = inspect_projects(&[
        agent("one", &root, "working"),
        agent("two", &root.join("src"), "tests passed"),
    ]);
    assert_eq!(projects.len(), 1);
    let project = &projects[0];
    assert!(project.git);
    assert_eq!(project.agents.len(), 2);
    assert_eq!(project.root, root.canonicalize().unwrap().to_string_lossy());
    assert_eq!(project.change_count, 2);
    assert!(project
        .changes
        .iter()
        .any(|file| file.path == "src/main.rs"));
    assert!(project.changes.iter().any(|file| file.path == "notes.txt"));
    assert!(project
        .latest_commit
        .as_deref()
        .unwrap()
        .contains("Initial project"));
    assert_eq!(
        project.test_evidence.as_ref().unwrap().result,
        TestResult::Passed
    );
    assert!(project.next_step.contains("Review"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn non_git_directory_is_still_visible_without_inventing_a_test_result() {
    let root = std::env::temp_dir().join(format!("am2-projects-nongit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let projects = inspect_projects(&[agent("one", &root, "activity unknown")]);
    assert_eq!(projects.len(), 1);
    assert!(!projects[0].git);
    assert!(projects[0].branch.is_none());
    assert!(projects[0].test_evidence.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn running_tests_is_not_a_pass_and_an_observed_failure_gets_priority() {
    let root = std::env::temp_dir().join(format!("am2-projects-tests-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let running = inspect_projects(&[agent("one", &root, "running tests")]);
    assert!(running[0].test_evidence.is_none());
    let observed = inspect_projects(&[
        agent("one", &root, "tests passed"),
        agent("two", &root, "tests failed"),
    ]);
    assert_eq!(
        observed[0].test_evidence.as_ref().unwrap().result,
        TestResult::Failed
    );
    assert!(observed[0].next_step.contains("failed test"));
    fs::remove_dir_all(root).unwrap();
}
