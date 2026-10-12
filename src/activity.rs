use crate::domain::AgentState;

pub fn output_fingerprint(output: &str) -> u64 {
    // A stable, compact fingerprint. Pane text itself is never persisted.
    output
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
        })
}

pub fn summarize(state: AgentState, output: &str) -> String {
    let lines = output
        .lines()
        .map(strip_ansi)
        .map(|line| line.trim().to_owned())
        .filter(|line| meaningful(line))
        .collect::<Vec<_>>();

    let permission_window = match state {
        AgentState::Waiting => 3,
        AgentState::Working => 1,
        AgentState::Idle | AgentState::Finished | AgentState::Failed | AgentState::Unknown => 0,
    };
    if lines.iter().rev().take(permission_window).any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("approve this")
            || lower.contains("permission requested")
            || lower.contains("allow this command")
            || lower.contains("grant permission")
    }) {
        return "permission requested".into();
    }

    if let Some(line) = lines.last() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("test result: ok") || lower.contains("tests passed") {
            return "tests passed".into();
        }
        if lower.contains("test result: failed") || lower.contains("tests failed") {
            return "tests failed".into();
        }
        if lower.contains("finished dev profile")
            || lower.contains("finished release profile")
            || lower.contains("build succeeded")
        {
            return "build finished".into();
        }
    }

    match state {
        AgentState::Idle => return "—".into(),
        AgentState::Waiting => return "idle".into(),
        AgentState::Finished => return "finished".into(),
        AgentState::Failed => return "failed".into(),
        AgentState::Unknown if lines.is_empty() => return "activity unknown".into(),
        AgentState::Working | AgentState::Unknown => {}
    }

    if let Some(line) = lines.last() {
        if let Some(path) = edit_path(line) {
            return format!("editing {}", clip(path, 72));
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("running ") && lower.contains(" tests") {
            return "running tests".into();
        }
        if [
            "cargo test",
            "pytest",
            "npm test",
            "go test",
            "pnpm test",
            "bun test",
        ]
        .iter()
        .any(|command| lower.contains(command))
        {
            return "running tests".into();
        }
        if [
            "cargo build",
            "cargo check",
            "npm run build",
            "pnpm build",
            "compiling ",
        ]
        .iter()
        .any(|command| lower.contains(command))
        {
            return "building project".into();
        }
    }

    if let Some(line) = lines.last() {
        if state == AgentState::Working && line.eq_ignore_ascii_case("working") {
            return "no recent output".into();
        }
        return clip(line, 80);
    }

    match state {
        AgentState::Working => "no recent output".into(),
        _ => "activity unknown".into(),
    }
}

fn edit_path(line: &str) -> Option<&str> {
    ["*** Update File: ", "Editing ", "editing ", "edited "]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .filter(|path| !path.trim().is_empty())
}

fn meaningful(line: &str) -> bool {
    !line.is_empty()
        && line.chars().any(char::is_alphanumeric)
        && !line.starts_with("AM2  /")
        && !line.starts_with("──")
}

fn clip(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

pub(crate) fn strip_ansi(line: &str) -> String {
    let mut result = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else if !ch.is_control() {
            result.push(ch);
        }
    }
    result
}
