use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub instance_pid: u32,
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub window_id: String,
    pub window_title: String,
}

pub fn list_terminals() -> Result<Vec<Candidate>, String> {
    parse_terminals(&run_bridge(&["list"])?)
}

pub fn parse_terminals(output: &str) -> Result<Vec<Candidate>, String> {
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
            Some(pid),
            Some(id),
            Some(title),
            Some(cwd),
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
        )
        else {
            return Err("Ghostty returned malformed terminal metadata".into());
        };
        let instance_pid = pid
            .parse::<u32>()
            .ok()
            .filter(|pid| *pid > 0)
            .ok_or("Ghostty returned an invalid app process ID")?;
        if !valid_terminal_id(id) || !valid_terminal_id(window_id) {
            return Err("Ghostty returned an invalid terminal ID".into());
        }
        candidates.push(Candidate {
            instance_pid,
            id: id.into(),
            title: title.into(),
            cwd: cwd.into(),
            window_id: window_id.into(),
            window_title: window_title.into(),
        });
    }
    Ok(candidates)
}

fn valid_terminal_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

pub fn link_target(candidate: &Candidate) -> String {
    format!("{}:{}", candidate.instance_pid, candidate.id)
}

pub fn focus_terminal(target: &str) -> Result<(), String> {
    let (pid, id) = parse_link_target(target)?;
    run_bridge(&["focus", &pid.to_string(), id]).map(|_| ())
}

fn parse_link_target(target: &str) -> Result<(u32, &str), String> {
    let (pid, id) = target
        .split_once(':')
        .ok_or("Old Ghostty link lacks an app process ID; press L to pair again")?;
    let pid = pid
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or("Invalid Ghostty app process ID")?;
    if !valid_terminal_id(id) {
        return Err("Invalid Ghostty terminal ID".into());
    }
    Ok((pid, id))
}

fn run_bridge(args: &[&str]) -> Result<String, String> {
    let helper = crate::bridge::companion_bridge("am2-ghostty-bridge", env!("AM2_GHOSTTY_BRIDGE"))?;
    let mut child = Command::new(helper)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot run Ghostty bridge: {error}"))?;
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
            return Err("Ghostty bridge timed out".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = stdout_reader
        .join()
        .map_err(|_| "Ghostty bridge output reader failed")?
        .map_err(|error| error.to_string())?;
    let error = stderr_reader
        .join()
        .map_err(|_| "Ghostty bridge error reader failed")?
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(if error.trim().is_empty() {
            format!("Ghostty bridge exited with {status}")
        } else {
            error.trim().to_owned()
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{link_target, parse_link_target, parse_terminals};

    #[test]
    fn parses_candidates_across_app_instances() {
        let candidates = parse_terminals("41\u{1f}A1B2\u{1f}Codex\u{1f}/Projects/am2\u{1f}W1\u{1f}AM2\u{1e}42\u{1f}C3D4\u{1f}Pi\u{1f}/Projects/pi\u{1f}W2\u{1f}Agent\u{1e}\n").unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].instance_pid, 41);
        assert_eq!(candidates[0].window_title, "AM2");
        assert_eq!(candidates[1].instance_pid, 42);
        assert_eq!(candidates[1].window_id, "W2");
        assert_eq!(link_target(&candidates[1]), "42:C3D4");
        assert_eq!(parse_link_target("42:C3D4").unwrap(), (42, "C3D4"));
        assert!(parse_link_target("C3D4").is_err());
        assert!(parse_terminals("bad\u{1f}missing\u{1e}").is_err());
    }
}
