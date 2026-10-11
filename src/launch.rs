//! Start an interactive agent in a terminal and retain an exact navigation target.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::domain::AgentKind;
use crate::terminals::{self, Provider};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    Herdr,
    Terminal(Provider),
}

impl Destination {
    pub fn label(self) -> &'static str {
        match self {
            Self::Herdr => "Herdr",
            Self::Terminal(provider) => provider.label(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchRequest {
    pub kind: AgentKind,
    pub cwd: PathBuf,
    pub destination: Destination,
    pub herdr_anchor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchedAgent {
    pub kind: AgentKind,
    pub pid: u32,
    pub target: String,
    pub destination: Destination,
}

pub fn launch(request: &LaunchRequest) -> Result<LaunchedAgent, String> {
    let cwd = request
        .cwd
        .canonicalize()
        .map_err(|error| format!("Project directory: {error}"))?;
    if !cwd.is_dir() {
        return Err("Project path is not a directory".into());
    }
    let executable = find_executable(request.kind.herdr_label())
        .ok_or_else(|| format!("{} executable was not found on PATH", request.kind))?;
    let marker = unique_path("pid");
    let script = unique_path("sh");
    write_launcher(&script, &marker, &cwd, &executable)?;
    let target = match request.destination {
        Destination::Herdr => request.herdr_anchor.as_deref().map_or_else(
            || Err("Select a live Herdr pane before launching into Herdr".into()),
            |anchor| launch_herdr(anchor, &cwd, &script),
        ),
        Destination::Terminal(provider) => launch_terminal(provider, &cwd, &script),
    };
    let target = match target {
        Ok(target) => target,
        Err(error) => {
            let _ = fs::remove_file(&script);
            return Err(error);
        }
    };
    let pid = wait_for_pid(&marker);
    let _ = fs::remove_file(&script);
    let _ = fs::remove_file(&marker);
    Ok(LaunchedAgent {
        kind: request.kind,
        pid: pid?,
        target,
        destination: request.destination,
    })
}

fn find_executable(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|path| {
            path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

fn unique_path(extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "am2-launch-{}-{nanos}.{extension}",
        std::process::id()
    ))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_launcher(
    script: &Path,
    marker: &Path,
    cwd: &Path,
    executable: &Path,
) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(script)
        .map_err(|error| format!("Cannot create launch script: {error}"))?;
    file.set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let agent_command = format!("exec {}", shell_quote(&executable.to_string_lossy()));
    writeln!(
        file,
        "#!/bin/sh\ncd {} || exit 1\nprintf '%s' \"$$\" > {}\nexec \"${{SHELL:-/bin/zsh}}\" -lc {}",
        shell_quote(&cwd.to_string_lossy()),
        shell_quote(&marker.to_string_lossy()),
        shell_quote(&agent_command),
    )
    .map_err(|error| error.to_string())
}

fn wait_for_pid(marker: &Path) -> Result<u32, String> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(12) {
        if let Ok(text) = fs::read_to_string(marker) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                if pid > 0 {
                    return Ok(pid);
                }
            }
        }
        thread::sleep(Duration::from_millis(80));
    }
    Err("Terminal opened, but the agent process did not start within 12 seconds".into())
}

fn run_bounded(command: &mut Command, label: &str) -> Result<Output, String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{label}: {error}"))?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return child.wait_with_output().map_err(|error| error.to_string());
        }
        if started.elapsed() >= Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{label} timed out"));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn launch_herdr(anchor: &str, cwd: &Path, script: &Path) -> Result<String, String> {
    if anchor.is_empty() {
        return Err("Herdr launch needs a live anchor pane".into());
    }
    let split = run_herdr(&mut herdr_split_command(anchor, cwd))?;
    let pane_id = herdr_created_pane_id(&split)?;
    let command = shell_quote(&script.to_string_lossy());
    run_herdr(Command::new("herdr").args(["pane", "run", &pane_id, &command]))
        .map_err(|error| format!("{error} (new pane {pane_id})"))?;
    Ok(pane_id)
}

fn herdr_split_command(anchor: &str, cwd: &Path) -> Command {
    let mut command = Command::new("herdr");
    command
        .args(["pane", "split", anchor, "--direction", "right", "--cwd"])
        .arg(cwd)
        .arg("--focus");
    command
}

fn run_herdr(command: &mut Command) -> Result<String, String> {
    let output = run_bounded(command, "Herdr launch")?;
    if !output.status.success() {
        return Err(format!(
            "Herdr launch: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn herdr_created_pane_id(response: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(response)
        .map_err(|error| format!("Herdr returned an invalid pane response: {error}"))?;
    value
        .pointer("/result/pane/pane_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "Herdr did not return the new pane ID".into())
}

fn launch_terminal(provider: Provider, cwd: &Path, script: &Path) -> Result<String, String> {
    let command = shell_quote(&script.to_string_lossy());
    let iterm_source = if provider == Provider::ITerm2 {
        Some(iterm_launch_source()?)
    } else {
        None
    };
    let (source, args): (&str, Vec<String>) = match provider {
        Provider::Ghostty => (
            r#"on run argv
set launchDir to item 1 of argv
set launchCommand to item 2 of argv
tell application "Ghostty"
set cfg to new surface configuration
set initial working directory of cfg to launchDir
set command of cfg to launchCommand
set win to new window with configuration cfg
return id of focused terminal of selected tab of win
end tell
end run"#,
            vec![
                cwd.to_string_lossy().into_owned(),
                script.to_string_lossy().into_owned(),
            ],
        ),
        Provider::Terminal => (
            r#"on run argv
tell application "Terminal"
set newTab to do script (item 1 of argv)
activate
return tty of newTab
end tell
end run"#,
            vec![format!(
                "cd {} && exec {command}",
                shell_quote(&cwd.to_string_lossy())
            )],
        ),
        Provider::ITerm2 => (
            iterm_source.as_deref().expect("iTerm source was prepared"),
            vec![format!(
                "/bin/sh -c {}",
                shell_quote(&format!(
                    "cd {} && exec {command}",
                    shell_quote(&cwd.to_string_lossy())
                ))
            )],
        ),
    };
    let output = run_bounded(
        Command::new("osascript").arg("-e").arg(source).args(args),
        &format!("{} launch", provider.label()),
    )?;
    if !output.status.success() {
        return Err(format!(
            "{} launch: {}",
            provider.label(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if id.is_empty() {
        return Err(format!("{} did not return a terminal ID", provider.label()));
    }
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if let Ok(inventory) = terminals::list_all() {
            let matches = inventory
                .candidates
                .iter()
                .filter(|candidate| candidate.provider == provider && candidate.id == id)
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                return Ok(matches[0].target());
            }
        }
        thread::sleep(Duration::from_millis(150));
    }
    Err(format!(
        "{} created a terminal but AM2 could not verify its exact ID",
        provider.label()
    ))
}

fn iterm_launch_source() -> Result<String, String> {
    let app = [
        PathBuf::from("/Applications/iTerm.app"),
        PathBuf::from("/System/Applications/iTerm.app"),
    ]
    .into_iter()
    .chain(std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications/iTerm.app")))
    .find(|path| path.is_dir())
    .ok_or("iTerm.app was not found in Applications")?;
    // Resolve the installed bundle by path so AppleScript loads its custom
    // dictionary. The display name "iTerm2" can resolve without that dictionary,
    // causing `create window with default profile` to fail at parse time.
    let app = app
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    Ok(format!(
        "on run argv\ntell application \"{app}\"\nset newWindow to (create window with default profile command (item 1 of argv))\nactivate\nreturn id of current session of newWindow\nend tell\nend run"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn herdr_split_response_requires_exact_pane_id() {
        assert_eq!(
            herdr_created_pane_id(r#"{"result":{"pane":{"pane_id":"w1:p12"}}}"#).unwrap(),
            "w1:p12"
        );
        assert!(herdr_created_pane_id(r#"{"result":{"pane":{}}}"#).is_err());
        assert!(herdr_created_pane_id("not json").is_err());
    }

    #[test]
    fn herdr_split_targets_the_selected_pane_from_an_external_window() {
        let command = herdr_split_command("w7:p9", Path::new("/Projects/am2"));
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "pane",
                "split",
                "w7:p9",
                "--direction",
                "right",
                "--cwd",
                "/Projects/am2",
                "--focus"
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn iterm_launch_applescript_compiles_with_installed_dictionary() {
        let Ok(source) = iterm_launch_source() else {
            return;
        };
        let compiled = unique_path("scpt");
        let output = Command::new("osacompile")
            .arg("-o")
            .arg(&compiled)
            .arg("-e")
            .arg(source)
            .output()
            .unwrap();
        let _ = fs::remove_file(compiled);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn launch_script_quotes_untrusted_project_paths() {
        let script = unique_path("sh");
        let marker = unique_path("pid");
        write_launcher(
            &script,
            &marker,
            Path::new("/tmp/a'b"),
            Path::new("/tmp/agent"),
        )
        .unwrap();
        let text = fs::read_to_string(&script).unwrap();
        assert!(text.contains("cd '/tmp/a'\\''b'"));
        assert!(text.contains("/tmp/agent"));
        let _ = fs::remove_file(script);
    }

    #[test]
    fn launcher_executes_in_the_exact_project_directory_and_records_pid() {
        let root = unique_path("project");
        let cwd = root.join("a'b space");
        fs::create_dir_all(&cwd).unwrap();
        let script = root.join("start.sh");
        let marker = root.join("pid");
        let shell = root.join("test-shell.sh");
        fs::write(
            &shell,
            "#!/bin/sh\n[ \"$1\" = '-lc' ] || exit 2\nexec /bin/sh -c \"$2\"\n",
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
        write_launcher(&script, &marker, &cwd, Path::new("/bin/pwd")).unwrap();
        let output = Command::new(&script).env("SHELL", &shell).output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            cwd.to_string_lossy()
        );
        assert!(fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse::<u32>()
            .is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}
