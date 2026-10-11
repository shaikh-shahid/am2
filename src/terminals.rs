use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::ghostty;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Ghostty,
    Terminal,
    ITerm2,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ghostty => "Ghostty",
            Self::Terminal => "Terminal.app",
            Self::ITerm2 => "iTerm2",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Ghostty => "ghostty",
            Self::Terminal => "terminal",
            Self::ITerm2 => "iterm2",
        }
    }

    fn parse(key: &str) -> Option<Self> {
        match key {
            "ghostty" => Some(Self::Ghostty),
            "terminal" => Some(Self::Terminal),
            "iterm2" => Some(Self::ITerm2),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub provider: Provider,
    pub instance_pid: u32,
    pub id: String,
    pub tty: Option<String>,
    pub title: String,
    pub cwd: Option<String>,
    pub window_id: String,
    pub window_title: String,
}

impl Candidate {
    pub fn target(&self) -> String {
        format!("{}:{}:{}", self.provider.key(), self.instance_pid, self.id)
    }
}

pub struct Inventory {
    pub candidates: Vec<Candidate>,
    pub warnings: Vec<String>,
}

pub fn list_all() -> Result<Inventory, String> {
    let mut candidates = Vec::new();
    let mut warnings = Vec::new();
    match ghostty::list_terminals() {
        Ok(ghostty) => candidates.extend(ghostty.into_iter().map(|terminal| Candidate {
            provider: Provider::Ghostty,
            instance_pid: terminal.instance_pid,
            id: terminal.id,
            tty: None,
            title: terminal.title,
            cwd: (!terminal.cwd.is_empty()).then_some(terminal.cwd),
            window_id: terminal.window_id,
            window_title: terminal.window_title,
        })),
        Err(error) if error == "No running Ghostty app instances found" => {}
        Err(error) => warnings.push(format!("Ghostty: {error}")),
    }
    match list_native() {
        Ok(inventory) => {
            candidates.extend(inventory.candidates);
            warnings.extend(inventory.warnings);
        }
        Err(error) => warnings.push(format!("Terminal/iTerm2: {error}")),
    }
    if candidates.is_empty() && !warnings.is_empty() {
        return Err(warnings.join(" · "));
    }
    Ok(Inventory {
        candidates,
        warnings,
    })
}

pub fn list_native() -> Result<Inventory, String> {
    let output = run_bridge(&["list"])?;
    let candidates = parse_native_candidates(&output.stdout)?;
    if !output.status.success() && candidates.is_empty() {
        return Err(output.error_message());
    }
    let warnings = if output.status.success() || output.stderr.trim().is_empty() {
        Vec::new()
    } else {
        vec![output.stderr.trim().to_owned()]
    };
    Ok(Inventory {
        candidates,
        warnings,
    })
}

pub fn parse_native_candidates(output: &str) -> Result<Vec<Candidate>, String> {
    let mut candidates = Vec::new();
    for record in output
        .trim_end_matches(['\r', '\n', '\u{1e}'])
        .split('\u{1e}')
    {
        if record.is_empty() {
            continue;
        }
        let mut fields = record.split('\u{1f}');
        let (
            Some(provider),
            Some(pid),
            Some(id),
            Some(tty),
            Some(title),
            Some(window_id),
            Some(window_title),
            None,
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            return Err("Terminal adapter returned malformed metadata".into());
        };
        let provider = Provider::parse(provider)
            .filter(|provider| *provider != Provider::Ghostty)
            .ok_or("Terminal adapter returned an unknown provider")?;
        let instance_pid = pid
            .parse::<u32>()
            .ok()
            .filter(|pid| *pid > 0)
            .ok_or("Terminal adapter returned an invalid process ID")?;
        if id.is_empty() || id.chars().any(char::is_control) {
            return Err("Terminal adapter returned an invalid session ID".into());
        }
        candidates.push(Candidate {
            provider,
            instance_pid,
            id: id.into(),
            tty: (!tty.is_empty()).then(|| tty.into()),
            title: title.into(),
            cwd: None,
            window_id: window_id.into(),
            window_title: window_title.into(),
        });
    }
    Ok(candidates)
}

pub fn matching_tty<'a>(candidates: &'a [Candidate], tty: &str) -> Vec<&'a Candidate> {
    let needle = normalized_tty(tty);
    if needle.is_empty() || needle == "??" {
        return Vec::new();
    }
    candidates
        .iter()
        .filter(|candidate| {
            candidate
                .tty
                .as_deref()
                .is_some_and(|candidate_tty| normalized_tty(candidate_tty) == needle)
        })
        .collect()
}

fn normalized_tty(tty: &str) -> &str {
    tty.trim().strip_prefix("/dev/").unwrap_or(tty.trim())
}

#[derive(Debug, PartialEq, Eq)]
pub struct Target<'a> {
    pub provider: Provider,
    pub instance_pid: u32,
    pub id: &'a str,
}

pub fn parse_target(target: &str) -> Result<Target<'_>, String> {
    let mut parts = target.splitn(3, ':');
    let provider = parts
        .next()
        .and_then(Provider::parse)
        .ok_or("Unknown terminal provider; pair the agent again")?;
    let instance_pid = parts
        .next()
        .and_then(|pid| pid.parse::<u32>().ok())
        .filter(|pid| *pid > 0)
        .ok_or("Invalid terminal app process ID")?;
    let id = parts
        .next()
        .filter(|id| !id.is_empty() && !id.chars().any(char::is_control))
        .ok_or("Invalid terminal session ID")?;
    Ok(Target {
        provider,
        instance_pid,
        id,
    })
}

pub fn focus_target(target: &str) -> Result<String, String> {
    let target = parse_target(target)?;
    match target.provider {
        Provider::Ghostty => {
            ghostty::focus_terminal(&format!("{}:{}", target.instance_pid, target.id))?
        }
        Provider::Terminal | Provider::ITerm2 => {
            let output = run_bridge(&[
                "focus",
                target.provider.key(),
                &target.instance_pid.to_string(),
                target.id,
            ])?;
            if !output.status.success() {
                return Err(output.error_message());
            }
        }
    }
    Ok(format!("{} {}", target.provider.label(), target.id))
}

pub fn read_visible(target: &str) -> Result<String, String> {
    let target = parse_target(target)?;
    if target.provider == Provider::Ghostty {
        return Err("Ghostty does not expose terminal text through its scripting API".into());
    }
    let output = run_bridge(&[
        "read",
        target.provider.key(),
        &target.instance_pid.to_string(),
        target.id,
    ])?;
    if !output.status.success() {
        return Err(output.error_message());
    }
    Ok(bounded_visible(&output.stdout))
}

pub fn bounded_visible(text: &str) -> String {
    let lines = text.lines().rev().take(80).collect::<Vec<_>>();
    let tail = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
    let bytes = tail.as_bytes();
    if bytes.len() <= 16 * 1024 {
        return tail;
    }
    let mut start = bytes.len() - 16 * 1024;
    while !tail.is_char_boundary(start) {
        start += 1;
    }
    tail[start..].to_owned()
}

struct BridgeOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl BridgeOutput {
    fn error_message(&self) -> String {
        if self.stderr.trim().is_empty() {
            format!("Terminal adapter exited with {}", self.status)
        } else {
            self.stderr.trim().to_owned()
        }
    }
}

fn run_bridge(args: &[&str]) -> Result<BridgeOutput, String> {
    let helper =
        crate::bridge::companion_bridge("am2-terminal-bridge", env!("AM2_TERMINAL_BRIDGE"))?;
    let mut child = Command::new(helper)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot run terminal adapter: {error}"))?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stdout_reader = thread::spawn(move || {
        let mut data = String::new();
        stdout.read_to_string(&mut data).map(|_| data)
    });
    let stderr_reader = thread::spawn(move || {
        let mut data = String::new();
        stderr.read_to_string(&mut data).map(|_| data)
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("Terminal adapter timed out".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "Terminal adapter output reader failed")?
        .map_err(|error| error.to_string())?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "Terminal adapter error reader failed")?
        .map_err(|error| error.to_string())?;
    Ok(BridgeOutput {
        status,
        stdout,
        stderr,
    })
}
