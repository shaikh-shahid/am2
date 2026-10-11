//! Read-only Git and observed test context for live agent projects.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::domain::{AgentKind, AgentState, AgentView};

const MAX_PROJECTS: usize = 30;
const MAX_CHANGED_FILES: usize = 60;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestResult {
    Passed,
    Failed,
}

impl TestResult {
    pub fn label(self) -> &'static str {
        match self {
            Self::Passed => "tests passed",
            Self::Failed => "tests failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestEvidence {
    pub result: TestResult,
    pub agent: AgentKind,
    pub observed_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectAgent {
    pub id: String,
    pub kind: AgentKind,
    pub state: AgentState,
    pub activity: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSnapshot {
    pub root: String,
    pub name: String,
    pub git: bool,
    pub branch: Option<String>,
    pub latest_commit: Option<String>,
    pub change_count: usize,
    pub changes: Vec<ChangedFile>,
    pub agents: Vec<ProjectAgent>,
    pub test_evidence: Option<TestEvidence>,
    pub next_step: String,
    pub error: Option<String>,
}

pub fn inspect_projects(agents: &[AgentView]) -> Vec<ProjectSnapshot> {
    let mut grouped: BTreeMap<String, (bool, Vec<&AgentView>)> = BTreeMap::new();
    for agent in agents {
        let cwd = &agent.observation.cwd;
        let candidate = PathBuf::from(cwd);
        let git_root = git(&candidate, &["rev-parse", "--show-toplevel"])
            .ok()
            .map(|root| root.trim().to_owned())
            .filter(|root| !root.is_empty());
        let root = git_root.clone().unwrap_or_else(|| {
            candidate
                .canonicalize()
                .unwrap_or(candidate)
                .to_string_lossy()
                .into_owned()
        });
        let entry = grouped
            .entry(root)
            .or_insert_with(|| (git_root.is_some(), Vec::new()));
        entry.1.push(agent);
    }
    grouped
        .into_iter()
        .take(MAX_PROJECTS)
        .map(|(root, (is_git, members))| inspect_group(root, is_git, members))
        .collect()
}

fn inspect_group(root: String, is_git: bool, members: Vec<&AgentView>) -> ProjectSnapshot {
    let name = Path::new(&root)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Unknown project")
        .to_owned();
    let agents = members
        .iter()
        .map(|view| ProjectAgent {
            id: view.observation.id.clone(),
            kind: view.observation.kind,
            state: view.observation.state,
            activity: view.activity.clone(),
        })
        .collect();
    let test_evidence = members
        .iter()
        .filter_map(|view| {
            let result = match view.activity.as_str() {
                "tests passed" => TestResult::Passed,
                "tests failed" => TestResult::Failed,
                _ => return None,
            };
            Some(TestEvidence {
                result,
                agent: view.observation.kind,
                observed_at: view.last_output_change,
            })
        })
        .max_by_key(|evidence| {
            (
                evidence.observed_at.unwrap_or(0),
                evidence.result == TestResult::Failed,
            )
        });
    let mut snapshot = ProjectSnapshot {
        root,
        name,
        git: is_git,
        branch: None,
        latest_commit: None,
        change_count: 0,
        changes: Vec::new(),
        agents,
        test_evidence,
        next_step: String::new(),
        error: None,
    };
    if is_git {
        let cwd = Path::new(&snapshot.root);
        snapshot.branch = git(cwd, &["branch", "--show-current"])
            .ok()
            .map(|output| clean_line(&output))
            .filter(|branch| !branch.is_empty())
            .or_else(|| Some("detached HEAD".into()));
        snapshot.latest_commit = git(cwd, &["log", "-1", "--format=%h %s"])
            .ok()
            .map(|output| clean_line(&output))
            .filter(|commit| !commit.is_empty());
        match git(
            cwd,
            &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
        ) {
            Ok(output) => {
                let (count, changes) = parse_status(&output);
                snapshot.change_count = count;
                snapshot.changes = changes;
            }
            Err(error) => snapshot.error = Some(format!("Git status unavailable: {error}")),
        }
    }
    snapshot.next_step = next_step(&snapshot);
    snapshot
}

fn next_step(project: &ProjectSnapshot) -> String {
    if project
        .test_evidence
        .as_ref()
        .is_some_and(|evidence| evidence.result == TestResult::Failed)
    {
        "Inspect failed test output in the agent pane".into()
    } else if !project.git {
        "Inspect agent output; Git status is unavailable".into()
    } else if project.error.is_some() {
        "Retry Git status before reviewing changes".into()
    } else if project.change_count > 0 && project.test_evidence.is_none() {
        "Review changes and run project tests".into()
    } else if project.change_count > 0 {
        "Review changed files before committing".into()
    } else if project.test_evidence.is_none() {
        "Check agent progress; no uncommitted changes".into()
    } else {
        "No uncommitted changes; test pass observed".into()
    }
}

fn parse_status(output: &str) -> (usize, Vec<ChangedFile>) {
    let records = output.split('\0').collect::<Vec<_>>();
    let mut index = 0;
    let mut count = 0;
    let mut changes = Vec::new();
    while let Some(record) = records
        .get(index)
        .copied()
        .filter(|record| !record.is_empty())
    {
        if record.len() < 4 || record.as_bytes()[2] != b' ' {
            break;
        }
        let status = &record[..2];
        let path = &record[3..];
        count += 1;
        if changes.len() < MAX_CHANGED_FILES {
            changes.push(ChangedFile {
                status: status.into(),
                path: clean_line(path),
            });
        }
        index += 1;
        if status.contains('R') || status.contains('C') {
            index += 1; // porcelain -z includes the original path after a rename or copy
        }
    }
    (count, changes)
}

fn clean_line(value: &str) -> String {
    value
        .chars()
        .filter(|ch| *ch == '\t' || !ch.is_control())
        .take(160)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let mut stdout = child.stdout.take().ok_or("Git stdout unavailable")?;
    let reader = thread::spawn(move || -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8192];
        let mut exceeded = false;
        loop {
            let len = stdout
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if len == 0 {
                break;
            }
            let available = MAX_OUTPUT_BYTES.saturating_sub(bytes.len());
            bytes.extend_from_slice(&buffer[..len.min(available)]);
            exceeded |= len > available;
        }
        if exceeded {
            Err("Git output exceeded 1 MiB".into())
        } else {
            Ok(bytes)
        }
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() >= GIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Git command timed out".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = reader
        .join()
        .map_err(|_| "Git output reader stopped".to_owned())??;
    if !status.success() {
        return Err(format!("Git exited with {status}"));
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}
