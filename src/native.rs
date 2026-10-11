use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::domain::{AgentKind, AgentObservation, AgentState, Runtime, RuntimeMetadata};

pub struct NativeRuntime;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSession {
    pid: u32,
    cwd: Option<String>,
    session_id: Option<String>,
    status: Option<String>,
    kind: Option<String>,
}

impl Runtime for NativeRuntime {
    fn observe(&self) -> Result<Vec<AgentObservation>, String> {
        let ps = command_output("ps", &["-axo", "pid=,ppid=,tty=,lstart=,command="])?;
        let candidates = parse_processes(&ps);
        let claude_query = candidates
            .iter()
            .any(|candidate| candidate.kind == AgentKind::Claude)
            .then(|| thread::spawn(|| command_output("claude", &["agents", "--json"])));
        let codex_root = std::env::var_os("CODEX_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".codex"))
            });
        let codex_pids = candidates
            .iter()
            .filter(|candidate| candidate.kind == AgentKind::Codex)
            .map(|candidate| candidate.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let codex_query = codex_root.filter(|_| !codex_pids.is_empty()).map(|root| {
            thread::spawn(move || {
                command_output("lsof", &["-a", "-p", &codex_pids, "-Fn"])
                    .map(|output| codex_session_ids_from_lsof(&output, &root))
            })
        });
        let pids = candidates
            .iter()
            .map(|candidate| candidate.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let cwds = if pids.is_empty() {
            HashMap::new()
        } else {
            command_output("lsof", &["-a", "-p", &pids, "-d", "cwd", "-Fpn"])
                .map(|output| parse_lsof_cwds(&output))
                .unwrap_or_default()
        };
        let codex_ids = codex_query
            .and_then(|query| query.join().ok().and_then(Result::ok))
            .unwrap_or_default();
        // Claude's supported CLI supplies exact active session IDs and status.
        // A failed/older installation simply leaves process-backed state unknown.
        let claude = claude_query.and_then(|query| query.join().ok().and_then(Result::ok));
        let fresh = command_output("ps", &["-axo", "pid=,ppid=,tty=,lstart=,command="])?;
        let still_live: HashSet<(u32, String)> = parse_processes(&fresh)
            .into_iter()
            .map(|process| (process.pid, process.started))
            .collect();
        let live = candidates
            .into_iter()
            .filter(|process| still_live.contains(&(process.pid, process.started.clone())))
            .collect::<Vec<_>>();
        let mut observations = observations_from_candidates(&live, &cwds, claude.as_deref());
        for observation in &mut observations {
            if observation.kind == AgentKind::Codex {
                if let Some(id) = codex_ids.get(&observation.pid) {
                    observation
                        .runtime
                        .fields
                        .push(("Session ID".into(), id.clone()));
                }
            }
        }
        Ok(observations)
    }
}

fn parse_lsof_cwds(output: &str) -> HashMap<u32, String> {
    let mut cwds = HashMap::new();
    let mut pid = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok();
        } else if let (Some(pid), Some(path)) = (pid, line.strip_prefix('n')) {
            cwds.insert(pid, path.to_owned());
        }
    }
    cwds
}

fn codex_session_ids_from_lsof(output: &str, codex_root: &Path) -> HashMap<u32, String> {
    let mut files = HashMap::<u32, String>::new();
    let mut pid = None;
    for line in output.lines() {
        if let Some(value) = line
            .strip_prefix('p')
            .and_then(|value| value.parse::<u32>().ok())
        {
            pid = Some(value);
        } else if line.starts_with('n') {
            if let Some(pid) = pid {
                files.entry(pid).or_default().push_str(&format!("{line}\n"));
            }
        }
    }
    files
        .into_iter()
        .filter_map(|(pid, paths)| {
            codex_session_id_from_lsof(&paths, codex_root).map(|id| (pid, id))
        })
        .collect()
}

pub fn codex_session_id_from_lsof(output: &str, codex_root: &Path) -> Option<String> {
    let mut ids = HashSet::new();
    for path in output.lines().filter_map(|line| line.strip_prefix('n')) {
        let Ok(relative) = Path::new(path).strip_prefix(codex_root) else {
            continue;
        };
        let components = relative.components().collect::<Vec<_>>();
        let candidate =
            if components.len() == 2 && components[0].as_os_str() == "thread-writer-locks" {
                relative.file_stem().and_then(|stem| stem.to_str())
            } else if components
                .first()
                .is_some_and(|part| part.as_os_str() == "sessions")
                && relative.extension().is_some_and(|ext| ext == "jsonl")
            {
                relative
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.get(stem.len().saturating_sub(36)..))
            } else {
                None
            };
        if let Some(id) = candidate.filter(|id| valid_uuid(id)) {
            ids.insert(id.to_owned());
        }
    }
    (ids.len() == 1).then(|| ids.into_iter().next()).flatten()
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn command_output(program: &str, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{program}: {error}"))?;
    let mut pipe = child.stdout.take().expect("stdout is piped");
    let reader = thread::spawn(move || -> Result<Vec<u8>, String> {
        let mut output = Vec::new();
        let mut chunk = [0_u8; 8192];
        let mut exceeded = false;
        loop {
            let n = pipe.read(&mut chunk).map_err(|error| error.to_string())?;
            if n == 0 {
                break;
            }
            let available = (4 * 1024 * 1024_usize).saturating_sub(output.len());
            output.extend_from_slice(&chunk[..n.min(available)]);
            exceeded |= n > available;
        }
        if exceeded {
            Err("output exceeded 4 MiB".into())
        } else {
            Ok(output)
        }
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(3) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(format!("{program} timed out"));
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = reader
        .join()
        .map_err(|_| format!("{program} output reader panicked"))??;
    if !status.success() {
        return Err(format!("{program} exited with {status}"));
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

struct Process {
    pid: u32,
    ppid: Option<u32>,
    tty: String,
    started: String,
    kind: AgentKind,
}

fn parse_processes(ps: &str) -> Vec<Process> {
    ps.lines()
        .filter_map(|line| {
            let words = line.split_whitespace().collect::<Vec<_>>();
            if words.len() < 8 {
                return None;
            }
            let pid = words[0].parse().ok()?;
            let ppid = words[1].parse::<u32>().ok();
            let tty_index = if ppid.is_some() { 2 } else { 1 };
            let command_index = tty_index + 6;
            let tty = *words.get(tty_index)?;
            let started = words.get(tty_index + 1..command_index)?.join("_");
            let executable = Path::new(*words.get(command_index)?)
                .file_name()?
                .to_str()?;
            let (kind, arg_start) = match executable {
                "codex" => (AgentKind::Codex, command_index + 1),
                "claude" => (AgentKind::Claude, command_index + 1),
                "opencode" => (AgentKind::OpenCode, command_index + 1),
                "pi" => (AgentKind::Pi, command_index + 1),
                "gemini" => (AgentKind::Gemini, command_index + 1),
                "cursor-agent" => (AgentKind::Cursor, command_index + 1),
                "node" if words.len() > command_index + 1 => {
                    let script = words[command_index + 1];
                    let resolved = Path::new(script).canonicalize().ok();
                    let kind =
                        node_entrypoint_kind(script, resolved.as_deref().and_then(Path::to_str))?;
                    (kind, command_index + 2)
                }
                _ => return None,
            };
            let args = &words[arg_start..];
            if args
                .iter()
                .any(|arg| matches!(*arg, "--help" | "-h" | "--version" | "-v"))
            {
                return None;
            }
            let first = args.first().copied();
            let service = match kind {
                AgentKind::Codex => matches!(
                    first,
                    Some(
                        "app-server"
                            | "exec-server"
                            | "mcp-server"
                            | "doctor"
                            | "agents"
                            | "login"
                            | "logout"
                            | "completion"
                            | "cloud"
                            | "features"
                            | "sandbox"
                            | "apply"
                            | "mcp"
                            | "help"
                            | "update"
                    )
                ),
                AgentKind::Claude => matches!(
                    first,
                    Some(
                        "agents"
                            | "doctor"
                            | "mcp"
                            | "update"
                            | "config"
                            | "auth"
                            | "plugin"
                            | "login"
                            | "logout"
                            | "help"
                    )
                ),
                AgentKind::OpenCode => matches!(
                    first,
                    Some("session" | "mcp" | "models" | "auth" | "upgrade" | "serve" | "web")
                ),
                AgentKind::Gemini => {
                    matches!(first, Some("mcp" | "extensions" | "hooks"))
                        || args.contains(&"--list-sessions")
                }
                AgentKind::Cursor => matches!(
                    first,
                    Some(
                        "login"
                            | "logout"
                            | "mcp"
                            | "status"
                            | "whoami"
                            | "models"
                            | "about"
                            | "update"
                            | "upgrade"
                            | "create-chat"
                            | "generate-rule"
                            | "help"
                    )
                ),
                AgentKind::Pi => false,
            };
            if service || tty == "??" {
                return None;
            }
            Some(Process {
                pid,
                ppid,
                tty: tty.into(),
                started,
                kind,
            })
        })
        .collect()
}

fn node_entrypoint_kind(script: &str, resolved: Option<&str>) -> Option<AgentKind> {
    for known in [Some(script), resolved].into_iter().flatten() {
        if (known.contains("/gemini-cli/") && known.ends_with("/gemini"))
            || (known.contains("/@google/gemini-cli/dist/") && known.ends_with("/index.js"))
        {
            return Some(AgentKind::Gemini);
        }
        if known.contains("/pi-coding-agent/") && known.ends_with("/cli.js") {
            return Some(AgentKind::Pi);
        }
    }
    None
}

/// Pure conversion for fixture tests. Command arguments are used only for
/// classification; they are never copied into observations or persisted.
pub fn observations_from_processes(
    ps: &str,
    cwds: &HashMap<u32, String>,
    claude_json: Option<&str>,
) -> Vec<AgentObservation> {
    observations_from_candidates(&parse_processes(ps), cwds, claude_json)
}

fn observations_from_candidates(
    processes: &[Process],
    cwds: &HashMap<u32, String>,
    claude_json: Option<&str>,
) -> Vec<AgentObservation> {
    let sessions: HashMap<u32, ClaudeSession> = claude_json
        .and_then(|json| serde_json::from_str::<Vec<ClaudeSession>>(json).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|session| (session.pid, session))
        .collect();
    processes
        .iter()
        .filter(|process| {
            process.kind != AgentKind::Gemini
                || !processes.iter().any(|parent| {
                    process.ppid == Some(parent.pid)
                        && parent.kind == AgentKind::Gemini
                        && parent.tty == process.tty
                })
        })
        .map(|process| {
            let session = sessions.get(&process.pid);
            let cwd = session
                .and_then(|session| session.cwd.as_deref())
                .filter(|cwd| !cwd.is_empty())
                .or_else(|| cwds.get(&process.pid).map(String::as_str))
                .unwrap_or("?")
                .to_owned();
            let project = Path::new(&cwd)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("?")
                .to_owned();
            let state = match session.and_then(|session| session.status.as_deref()) {
                Some("working") => AgentState::Working,
                Some("idle" | "waiting" | "blocked") => AgentState::Waiting,
                Some("done" | "finished") => AgentState::Finished,
                Some("failed" | "error") => AgentState::Failed,
                _ => AgentState::Unknown,
            };
            let mut fields = vec![
                ("TTY".into(), process.tty.clone()),
                ("Started".into(), process.started.replace('_', " ")),
            ];
            if let Some(session) = session {
                if let Some(id) = &session.session_id {
                    fields.push(("Session ID".into(), id.clone()));
                }
                if let Some(kind) = &session.kind {
                    fields.push(("Session kind".into(), kind.clone()));
                }
            }
            AgentObservation {
                id: format!("native:{}:{}", process.pid, process.started),
                kind: process.kind,
                project,
                state,
                pid: process.pid,
                cwd,
                runtime: RuntimeMetadata {
                    provider: "macOS".into(),
                    target: format!("PID {}", process.pid),
                    fields,
                },
                recent_output: String::new(),
                output_error: Some("No pane output available".into()),
            }
        })
        .collect()
}

#[cfg(test)]
mod lsof_tests {
    use super::*;

    #[test]
    fn grouped_lsof_cwds_keep_each_path_with_its_process() {
        let output = "p10\nfcwd\nn/Projects/one space\np12\nfcwd\nn/Projects/two\n";
        let cwds = parse_lsof_cwds(output);
        assert_eq!(
            cwds.get(&10).map(String::as_str),
            Some("/Projects/one space")
        );
        assert_eq!(cwds.get(&12).map(String::as_str), Some("/Projects/two"));
    }

    #[test]
    fn grouped_codex_files_keep_session_ids_with_their_process() {
        let root = Path::new("/home/test/.codex");
        let first = "11111111-1111-1111-1111-111111111111";
        let second = "22222222-2222-2222-2222-222222222222";
        let output = format!("p10\nn/home/test/.codex/thread-writer-locks/{first}.lock\np12\nn/home/test/.codex/thread-writer-locks/{second}.lock\n");
        let ids = codex_session_ids_from_lsof(&output, root);
        assert_eq!(ids.get(&10).map(String::as_str), Some(first));
        assert_eq!(ids.get(&12).map(String::as_str), Some(second));
    }
}

#[cfg(test)]
mod node_entrypoint_tests {
    use super::*;

    #[test]
    fn gemini_symlink_to_known_cli_is_recognized() {
        assert_eq!(
            node_entrypoint_kind(
                "/opt/homebrew/bin/gemini",
                Some("/opt/homebrew/Cellar/gemini-cli/0.25.2/bin/gemini")
            ),
            Some(AgentKind::Gemini)
        );
        assert_eq!(node_entrypoint_kind("/Projects/other/gemini", None), None);
    }
}
