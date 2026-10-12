use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::domain::{AgentKind, AgentObservation, AgentState, Runtime, RuntimeMetadata};

#[derive(Debug)]
pub enum HerdrError {
    Command(String),
    Json(serde_json::Error),
}

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(message) => write!(f, "Herdr: {message}"),
            Self::Json(error) => write!(f, "Invalid Herdr JSON: {error}"),
        }
    }
}

impl Error for HerdrError {}

impl From<serde_json::Error> for HerdrError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentPane {
    pub kind: AgentKind,
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub pid: u32,
    pub mode: String,
    pub cwd: String,
    pub status: String,
    pub focused: bool,
}

pub type CodexPane = AgentPane;

#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

#[derive(Deserialize)]
struct SnapshotResult {
    snapshot: Snapshot,
}

#[derive(Deserialize)]
struct Snapshot {
    panes: Vec<Pane>,
}

#[derive(Deserialize)]
struct Pane {
    pane_id: String,
    tab_id: String,
    workspace_id: String,
    terminal_id: Option<String>,
    cwd: Option<String>,
    foreground_cwd: Option<String>,
    agent: Option<String>,
    agent_status: Option<String>,
    #[serde(default)]
    focused: bool,
}

#[derive(Deserialize)]
struct ProcessInfoResult {
    process_info: ProcessInfo,
}

#[derive(Deserialize)]
struct ProcessInfo {
    #[serde(default)]
    foreground_processes: Vec<ForegroundProcess>,
}

#[derive(Deserialize)]
struct ForegroundProcess {
    pid: u32,
    name: Option<String>,
    argv0: Option<String>,
    #[serde(default)]
    argv: Vec<String>,
    cwd: Option<String>,
}

fn command_output(args: &[&str]) -> Result<String, HerdrError> {
    let capture = if args.starts_with(&["pane", "read"]) {
        CaptureMode::Tail(16 * 1024)
    } else {
        CaptureMode::Limit(4 * 1024 * 1024)
    };
    run_external_command("herdr", args, Duration::from_secs(3), capture)
}

/// Select an existing Herdr pane in an attached client. This does not raise
/// the outer terminal window; callers outside Herdr must do that separately.
pub fn focus_agent_pane(pane_id: &str) -> Result<(), HerdrError> {
    focus_agent_pane_with(pane_id, command_output)
}

fn focus_agent_pane_with(
    pane_id: &str,
    mut run: impl FnMut(&[&str]) -> Result<String, HerdrError>,
) -> Result<(), HerdrError> {
    run(&["agent", "focus", pane_id]).map(|_| ())
}

#[derive(Clone, Copy)]
enum CaptureMode {
    Limit(usize),
    Tail(usize),
}

struct Captured {
    bytes: Vec<u8>,
    exceeded: bool,
}

fn read_pipe<R: Read + Send + 'static>(
    mut pipe: R,
    capture: CaptureMode,
) -> thread::JoinHandle<io::Result<Captured>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut exceeded = false;
        let mut chunk = [0_u8; 8192];
        loop {
            let count = pipe.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            let incoming = &chunk[..count];
            match capture {
                CaptureMode::Limit(max) => {
                    let available = max.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&incoming[..incoming.len().min(available)]);
                    exceeded |= incoming.len() > available;
                }
                CaptureMode::Tail(max) => {
                    if incoming.len() >= max {
                        bytes.clear();
                        bytes.extend_from_slice(&incoming[incoming.len() - max..]);
                    } else {
                        let overflow = bytes
                            .len()
                            .saturating_add(incoming.len())
                            .saturating_sub(max);
                        if overflow > 0 {
                            bytes.drain(..overflow);
                        }
                        bytes.extend_from_slice(incoming);
                    }
                }
            }
        }
        Ok(Captured { bytes, exceeded })
    })
}

fn pipe_capture(handle: thread::JoinHandle<io::Result<Captured>>) -> Result<Captured, HerdrError> {
    handle
        .join()
        .map_err(|_| HerdrError::Command("output reader panicked".into()))?
        .map_err(|error| HerdrError::Command(format!("cannot read command output: {error}")))
}

fn run_external_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
    capture: CaptureMode,
) -> Result<String, HerdrError> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| HerdrError::Command(format!("cannot run `{program}`: {error}")))?;
    let stdout_reader = read_pipe(child.stdout.take().expect("stdout is piped"), capture);
    let stderr_reader = read_pipe(
        child.stderr.take().expect("stderr is piped"),
        CaptureMode::Tail(32 * 1024),
    );
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| HerdrError::Command(format!("cannot wait for `{program}`: {error}")))?
        {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = pipe_capture(stdout_reader);
            let _ = pipe_capture(stderr_reader);
            return Err(HerdrError::Command(format!(
                "`{program} {}` timed out after {} ms",
                args.join(" "),
                timeout.as_millis()
            )));
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = pipe_capture(stdout_reader)?;
    let stderr = pipe_capture(stderr_reader)?;
    if !status.success() {
        let message = String::from_utf8_lossy(&stderr.bytes).trim().to_owned();
        return Err(HerdrError::Command(format!(
            "`{program} {}` failed: {}",
            args.join(" "),
            if message.is_empty() {
                status.to_string()
            } else {
                message
            }
        )));
    }
    if stdout.exceeded {
        return Err(HerdrError::Command(format!(
            "`{program} {}` output exceeded capture limit",
            args.join(" ")
        )));
    }
    Ok(String::from_utf8_lossy(&stdout.bytes).into_owned())
}

/// Read Herdr's live snapshot and the foreground process group of each pane.
/// A process query can race with a pane closing; that pane is omitted.
pub fn load_codex_panes() -> Result<Vec<CodexPane>, HerdrError> {
    load_codex_panes_with(command_output)
}

fn load_codex_panes_with(
    mut run: impl FnMut(&[&str]) -> Result<String, HerdrError>,
) -> Result<Vec<CodexPane>, HerdrError> {
    let (snapshot, process_by_pane) = scan_with(&mut run)?;
    codex_panes_from_json(&snapshot, &process_by_pane)
}

pub fn load_agent_panes() -> Result<Vec<AgentPane>, HerdrError> {
    let (snapshot, process_by_pane) = scan_parallel_with(command_output)?;
    agent_panes_from_json(&snapshot, &process_by_pane)
}

fn scan_with(
    mut run: impl FnMut(&[&str]) -> Result<String, HerdrError>,
) -> Result<(String, HashMap<String, String>), HerdrError> {
    let snapshot = run(&["api", "snapshot"])?;
    let parsed: Envelope<SnapshotResult> = serde_json::from_str(&snapshot)?;
    let mut process_by_pane = HashMap::new();
    let mut failed = Vec::new();
    for pane in &parsed.result.snapshot.panes {
        match run(&["pane", "process-info", "--pane", &pane.pane_id]) {
            Ok(info) => {
                process_by_pane.insert(pane.pane_id.clone(), info);
            }
            Err(error) => failed.push((pane.pane_id.clone(), error)),
        }
    }
    if !failed.is_empty() {
        // A pane can close between snapshot and process-info. Recheck before
        // treating the query failure as a connection or permission failure.
        let fresh: Envelope<SnapshotResult> = serde_json::from_str(&run(&["api", "snapshot"])?)?;
        for (id, error) in failed {
            if fresh
                .result
                .snapshot
                .panes
                .iter()
                .any(|pane| pane.pane_id == id)
            {
                return Err(HerdrError::Command(format!(
                    "process info for pane {id} failed: {error}"
                )));
            }
        }
    }
    Ok((snapshot, process_by_pane))
}

fn scan_parallel_with(
    run: impl Fn(&[&str]) -> Result<String, HerdrError> + Sync,
) -> Result<(String, HashMap<String, String>), HerdrError> {
    let snapshot = run(&["api", "snapshot"])?;
    let parsed: Envelope<SnapshotResult> = serde_json::from_str(&snapshot)?;
    let mut process_by_pane = HashMap::new();
    let mut failed = Vec::new();
    // Cap concurrent CLI processes so large workspaces do not flood Herdr.
    for panes in parsed.result.snapshot.panes.chunks(8) {
        let results = thread::scope(|scope| {
            let handles = panes
                .iter()
                .map(|pane| {
                    scope.spawn(|| {
                        (
                            pane.pane_id.clone(),
                            run(&["pane", "process-info", "--pane", &pane.pane_id]),
                        )
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("Herdr process query panicked"))
                .collect::<Vec<_>>()
        });
        for (id, result) in results {
            match result {
                Ok(info) => {
                    process_by_pane.insert(id, info);
                }
                Err(error) => failed.push((id, error)),
            }
        }
    }
    if !failed.is_empty() {
        let fresh: Envelope<SnapshotResult> = serde_json::from_str(&run(&["api", "snapshot"])?)?;
        for (id, error) in failed {
            if fresh
                .result
                .snapshot
                .panes
                .iter()
                .any(|pane| pane.pane_id == id)
            {
                return Err(HerdrError::Command(format!(
                    "process info for pane {id} failed: {error}"
                )));
            }
        }
    }
    Ok((snapshot, process_by_pane))
}

pub struct HerdrRuntime;

impl Runtime for HerdrRuntime {
    fn observe(&self) -> Result<Vec<AgentObservation>, String> {
        let (snapshot, processes) =
            scan_parallel_with(command_output).map_err(|error| error.to_string())?;
        observations_from_json(&snapshot, &processes, &HashMap::new())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
fn observe_with(
    mut run: impl FnMut(&[&str]) -> Result<String, HerdrError>,
) -> Result<Vec<AgentObservation>, HerdrError> {
    let (snapshot, processes) = scan_with(&mut run)?;
    observations_from_json(&snapshot, &processes, &HashMap::new())
}

/// Read one selected pane on demand. Polling every pane causes avoidable
/// traffic and can disturb a Herdr client's screen while AM2 is open.
pub fn read_recent_output(pane_id: &str) -> Result<String, HerdrError> {
    read_recent_output_with(pane_id, command_output)
}

fn read_recent_output_with(
    pane_id: &str,
    mut run: impl FnMut(&[&str]) -> Result<String, HerdrError>,
) -> Result<String, HerdrError> {
    run(&[
        "pane",
        "read",
        pane_id,
        "--source",
        "recent-unwrapped",
        "--lines",
        "80",
        "--format",
        "text",
    ])
    .map(|text| bounded_output(&text))
}

pub fn observations_from_json(
    snapshot_json: &str,
    process_by_pane: &HashMap<String, String>,
    output_by_pane: &HashMap<String, String>,
) -> Result<Vec<AgentObservation>, HerdrError> {
    let snapshot: Envelope<SnapshotResult> = serde_json::from_str(snapshot_json)?;
    let agents = agent_panes_from_json(snapshot_json, process_by_pane)?;
    let mut observations = Vec::with_capacity(agents.len());
    for pane in agents {
        let source = snapshot
            .result
            .snapshot
            .panes
            .iter()
            .find(|source| source.pane_id == pane.pane_id)
            .expect("agent pane came from the same snapshot");
        let terminal_id = source
            .terminal_id
            .as_deref()
            .unwrap_or(&pane.pane_id)
            .to_owned();
        let project = std::path::Path::new(&pane.cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(&pane.cwd)
            .to_owned();
        let state = match pane.status.as_str() {
            "working" => AgentState::Working,
            "idle" => AgentState::Idle,
            "blocked" => AgentState::Waiting,
            "done" => AgentState::Finished,
            "failed" => AgentState::Failed,
            _ => AgentState::Unknown,
        };
        let recent_output = output_by_pane
            .get(&pane.pane_id)
            .map(|text| bounded_output(text))
            .unwrap_or_default();
        observations.push(AgentObservation {
            id: format!("herdr:{terminal_id}:{}", pane.pid),
            kind: pane.kind,
            project,
            state,
            pid: pane.pid,
            cwd: pane.cwd,
            runtime: RuntimeMetadata {
                provider: "Herdr".into(),
                target: pane.pane_id,
                fields: vec![
                    ("Tab".into(), pane.tab_id),
                    ("Workspace".into(), pane.workspace_id),
                    ("Terminal".into(), terminal_id),
                    ("Mode".into(), pane.mode),
                    ("Herdr status".into(), pane.status),
                ],
            },
            recent_output,
            output_error: None,
        });
    }
    Ok(observations)
}

fn bounded_output(text: &str) -> String {
    const MAX_BYTES: usize = 16 * 1024;
    let mut lines = text
        .lines()
        .rev()
        .take(80)
        .map(crate::activity::strip_ansi)
        .collect::<Vec<_>>();
    lines.reverse();
    let joined = lines.join("\n");
    if joined.len() <= MAX_BYTES {
        return joined;
    }
    let mut start = joined.len() - MAX_BYTES;
    while !joined.is_char_boundary(start) {
        start += 1;
    }
    joined[start..].to_owned()
}

/// Pure conversion used by both the live reader and fixture tests.
pub fn codex_panes_from_json(
    snapshot_json: &str,
    process_by_pane: &HashMap<String, String>,
) -> Result<Vec<CodexPane>, HerdrError> {
    Ok(agent_panes_from_json(snapshot_json, process_by_pane)?
        .into_iter()
        .filter(|pane| pane.kind == AgentKind::Codex)
        .collect())
}

pub fn agent_panes_from_json(
    snapshot_json: &str,
    process_by_pane: &HashMap<String, String>,
) -> Result<Vec<AgentPane>, HerdrError> {
    let snapshot: Envelope<SnapshotResult> = serde_json::from_str(snapshot_json)?;
    let mut panes = Vec::new();
    for pane in snapshot.result.snapshot.panes {
        let Some(process_json) = process_by_pane.get(&pane.pane_id) else {
            continue;
        };
        let info: Envelope<ProcessInfoResult> = serde_json::from_str(process_json)?;
        let Some((process, kind, mode)) = info
            .result
            .process_info
            .foreground_processes
            .iter()
            .find_map(|process| agent_mode(process).map(|(kind, mode)| (process, kind, mode)))
        else {
            continue;
        };
        let cwd = process
            .cwd
            .as_deref()
            .filter(|value| !value.is_empty())
            .or(pane
                .foreground_cwd
                .as_deref()
                .filter(|value| !value.is_empty()))
            .or(pane.cwd.as_deref().filter(|value| !value.is_empty()))
            .unwrap_or("?")
            .to_owned();
        let status = if pane.agent.as_deref() == Some(kind.herdr_label()) {
            pane.agent_status.unwrap_or_else(|| "unknown".into())
        } else {
            "unknown".into()
        };
        panes.push(AgentPane {
            kind,
            pane_id: pane.pane_id,
            tab_id: pane.tab_id,
            workspace_id: pane.workspace_id,
            pid: process.pid,
            mode: mode.to_owned(),
            cwd,
            status,
            focused: pane.focused,
        });
    }
    panes.sort_by(|a, b| {
        (&a.workspace_id, &a.tab_id, &a.pane_id).cmp(&(&b.workspace_id, &b.tab_id, &b.pane_id))
    });
    Ok(panes)
}

fn codex_mode(process: &ForegroundProcess) -> Option<&'static str> {
    let argv0 = process
        .argv0
        .as_deref()
        .or(process.argv.first().map(String::as_str))
        .or(process.name.as_deref())?;
    let executable = argv0.rsplit('/').next().unwrap_or(argv0);
    if executable != "codex" {
        return None;
    }

    // The first positional argument is a subcommand, except that Codex also
    // accepts an initial prompt. Skip options known to take their own value.
    let mut args = process.argv.iter().skip(1);
    let mut first_positional = None;
    while let Some(arg) = args.next() {
        if arg == "--" {
            first_positional = args.next().map(String::as_str);
            break;
        }
        if matches!(
            arg.as_str(),
            "-C" | "--cd"
                | "-c"
                | "--config"
                | "-m"
                | "--model"
                | "-p"
                | "--profile"
                | "--add-dir"
                | "--remote"
        ) {
            args.next();
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        first_positional = Some(arg.as_str());
        break;
    }

    match first_positional {
        Some("resume") => Some("resume"),
        Some("exec") => Some("exec"),
        Some("fork") => Some("fork"),
        Some("review") => Some("review"),
        Some(
            "app-server" | "exec-server" | "mcp-server" | "doctor" | "agents" | "login" | "logout"
            | "completion" | "cloud" | "features" | "sandbox" | "apply" | "mcp" | "help" | "update"
            | "queue" | "archive" | "delete" | "unarchive" | "migrate-rollouts",
        ) => None,
        _ => Some("interactive"),
    }
}

fn agent_mode(process: &ForegroundProcess) -> Option<(AgentKind, &'static str)> {
    let argv0 = process
        .argv0
        .as_deref()
        .or(process.argv.first().map(String::as_str))
        .or(process.name.as_deref())?;
    match argv0.rsplit('/').next().unwrap_or(argv0) {
        "codex" => codex_mode(process).map(|mode| (AgentKind::Codex, mode)),
        "claude" => claude_mode(process).map(|mode| (AgentKind::Claude, mode)),
        "opencode" => opencode_mode(process).map(|mode| (AgentKind::OpenCode, mode)),
        "pi" => simple_mode(
            process,
            &["--help", "--version", "--list-models", "--export"],
        )
        .map(|mode| (AgentKind::Pi, mode)),
        "gemini" => simple_mode(
            process,
            &[
                "--help",
                "--version",
                "--list-sessions",
                "mcp",
                "extensions",
                "hooks",
            ],
        )
        .map(|mode| (AgentKind::Gemini, mode)),
        "cursor-agent" => simple_mode(
            process,
            &[
                "--help",
                "--version",
                "login",
                "logout",
                "mcp",
                "status",
                "whoami",
                "models",
                "about",
                "update",
                "upgrade",
                "create-chat",
                "help",
            ],
        )
        .map(|mode| (AgentKind::Cursor, mode)),
        _ => None,
    }
}

fn simple_mode(process: &ForegroundProcess, excluded: &[&str]) -> Option<&'static str> {
    if process
        .argv
        .iter()
        .skip(1)
        .any(|arg| excluded.contains(&arg.as_str()))
    {
        None
    } else {
        Some("interactive")
    }
}

fn claude_mode(process: &ForegroundProcess) -> Option<&'static str> {
    let args = process
        .argv
        .iter()
        .skip(1)
        .map(String::as_str)
        .collect::<Vec<_>>();
    if args
        .iter()
        .any(|arg| matches!(*arg, "--help" | "--version"))
    {
        return None;
    }
    let first = args.first().copied();
    if matches!(
        first,
        Some(
            "agents"
                | "doctor"
                | "mcp"
                | "update"
                | "config"
                | "auth"
                | "plugin"
                | "plugins"
                | "login"
                | "logout"
                | "help"
        )
    ) {
        return None;
    }
    if args.iter().any(|arg| matches!(*arg, "-p" | "--print")) {
        Some("print")
    } else if first == Some("-c") || args.contains(&"--continue") {
        Some("continue")
    } else if first == Some("-r") || args.contains(&"--resume") {
        Some("resume")
    } else {
        Some("interactive")
    }
}

fn opencode_mode(process: &ForegroundProcess) -> Option<&'static str> {
    let args = process
        .argv
        .iter()
        .skip(1)
        .map(String::as_str)
        .collect::<Vec<_>>();
    if args
        .iter()
        .any(|arg| matches!(*arg, "--help" | "--version"))
    {
        return None;
    }
    match args.first().copied() {
        Some("run") => Some("run"),
        Some("attach") => Some("attach"),
        Some(
            "serve" | "web" | "acp" | "mcp" | "debug" | "auth" | "agent" | "models" | "stats"
            | "export" | "import" | "github" | "session" | "db" | "completion" | "upgrade"
            | "uninstall" | "help",
        ) => None,
        _ => Some("interactive"),
    }
}

#[cfg(test)]
mod live_reader_tests {
    use super::*;

    const ONE_PANE: &str = r#"{"result":{"snapshot":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","agent":"codex","agent_status":"idle"}]}}}"#;
    const NO_PANES: &str = r#"{"result":{"snapshot":{"panes":[]}}}"#;

    #[test]
    fn focusing_uses_the_exact_pane_id_and_propagates_an_error() {
        let error = focus_agent_pane_with("w1:pF", |args| {
            assert_eq!(args, &["agent", "focus", "w1:pF"]);
            Err(HerdrError::Command("pane closed".into()))
        })
        .unwrap_err();
        assert!(error.to_string().contains("pane closed"));
    }

    #[test]
    fn a_failed_process_query_for_an_existing_pane_is_not_an_empty_list() {
        let result = load_codex_panes_with(|args| match args {
            ["api", "snapshot"] => Ok(ONE_PANE.into()),
            ["pane", "process-info", ..] => Err(HerdrError::Command("socket lost".into())),
            _ => unreachable!(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn a_pane_that_exits_during_the_scan_can_be_skipped() {
        let mut snapshots = 0;
        let result = load_codex_panes_with(|args| match args {
            ["api", "snapshot"] => {
                snapshots += 1;
                Ok(if snapshots == 1 { ONE_PANE } else { NO_PANES }.into())
            }
            ["pane", "process-info", ..] => Err(HerdrError::Command("pane gone".into())),
            _ => unreachable!(),
        })
        .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parallel_scan_queries_multiple_panes_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let snapshot = r#"{"result":{"snapshot":{"panes":[{"pane_id":"p1","tab_id":"t","workspace_id":"w"},{"pane_id":"p2","tab_id":"t","workspace_id":"w"}]}}}"#;
        let (_, processes) = scan_parallel_with(|args| match args {
            ["api", "snapshot"] => Ok(snapshot.into()),
            ["pane", "process-info", "--pane", id] => {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(format!("{{\"pane\":\"{id}\"}}"))
            }
            _ => unreachable!(),
        })
        .unwrap();
        assert_eq!(processes.len(), 2);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_hung_command_has_a_deadline() {
        let start = std::time::Instant::now();
        let result = run_external_command(
            "/bin/sh",
            &["-c", "sleep 2"],
            std::time::Duration::from_millis(80),
            CaptureMode::Limit(1024),
        );
        assert!(result.is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn periodic_observation_does_not_read_agent_scrollback() {
        let snapshot = r#"{"result":{"snapshot":{"panes":[{"pane_id":"p1","tab_id":"t1","workspace_id":"w1","agent":"codex"},{"pane_id":"p2","tab_id":"t1","workspace_id":"w1","agent":"claude"}]}}}"#;
        let codex =
            r#"{"result":{"process_info":{"foreground_processes":[{"pid":10,"argv0":"codex"}]}}}"#;
        let claude =
            r#"{"result":{"process_info":{"foreground_processes":[{"pid":11,"argv0":"claude"}]}}}"#;
        let result = observe_with(|args| match args {
            ["api", "snapshot"] => Ok(snapshot.into()),
            ["pane", "process-info", "--pane", "p1"] => Ok(codex.into()),
            ["pane", "process-info", "--pane", "p2"] => Ok(claude.into()),
            ["pane", "read", ..] => panic!("periodic observation read scrollback"),
            _ => unreachable!(),
        })
        .unwrap();
        assert!(result[0].recent_output.is_empty());
        assert_eq!(result[1].kind, AgentKind::Claude);
    }

    #[test]
    fn selected_pane_read_is_exact_and_bounded() {
        let output = read_recent_output_with("w1:p12", |args| {
            assert_eq!(
                args,
                &[
                    "pane",
                    "read",
                    "w1:p12",
                    "--source",
                    "recent-unwrapped",
                    "--lines",
                    "80",
                    "--format",
                    "text"
                ]
            );
            Ok((0..100).map(|n| format!("line {n}\n")).collect())
        })
        .unwrap();
        assert_eq!(output.lines().count(), 80);
        assert!(output.contains("line 99"));
        assert!(!output.contains("line 0\n"));
    }

    #[test]
    fn pane_output_capture_keeps_only_a_bounded_tail() {
        let output = run_external_command(
            "/bin/sh",
            &["-c", "printf 'start'; printf '%*s' 100000 ''; printf 'END'"],
            Duration::from_secs(2),
            CaptureMode::Tail(32),
        )
        .unwrap();
        assert_eq!(output.len(), 32);
        assert!(output.ends_with("END"));
    }

    #[test]
    fn oversized_structured_output_is_rejected_without_collecting_it_all() {
        let error = run_external_command(
            "/bin/sh",
            &["-c", "printf '%*s' 100000 ''"],
            Duration::from_secs(2),
            CaptureMode::Limit(1024),
        )
        .unwrap_err();
        assert!(error.to_string().contains("exceeded"));
    }
}
