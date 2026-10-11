//! Bounded, read-only adapters for local agent session records.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::domain::AgentKind;

const MAX_FILES: usize = 120;
const MAX_FILE_BYTES: u64 = 24 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 96 * 1024 * 1024;
const MAX_EVENTS: usize = 512;
const MAX_EVENT_CHARS: usize = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryEventKind {
    User,
    Assistant,
    Tool,
    Result,
}

impl HistoryEventKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Assistant => "Agent",
            Self::Tool => "Tool",
            Self::Result => "Result",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEvent {
    pub timestamp: Option<String>,
    pub kind: HistoryEventKind,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistorySession {
    pub kind: AgentKind,
    pub id: String,
    pub cwd: String,
    pub project: String,
    pub updated_at: i64,
    pub events: Vec<HistoryEvent>,
}

#[derive(Clone, Debug, Default)]
pub struct HistoryInventory {
    pub sessions: Vec<HistorySession>,
    pub skipped_files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryHit {
    pub session_index: usize,
    pub event_index: Option<usize>,
    pub snippet: String,
    pub match_count: usize,
}

pub fn scan_history() -> Result<HistoryInventory, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let home = PathBuf::from(home);
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    scan_roots(&[
        (AgentKind::Codex, codex.join("sessions")),
        (AgentKind::Claude, home.join(".claude/projects")),
        (AgentKind::Pi, home.join(".pi/agent/sessions")),
    ])
}

pub fn scan_roots(roots: &[(AgentKind, PathBuf)]) -> Result<HistoryInventory, String> {
    let mut files = Vec::new();
    for (kind, root) in roots {
        collect_files(*kind, root, &mut files)?;
    }
    files.sort_by_key(|file| std::cmp::Reverse(file.2));
    let mut inventory = HistoryInventory::default();
    let mut used_bytes = 0_u64;
    let mut inspected_files = 0_usize;
    for (kind, path, _, len) in files {
        if inspected_files >= MAX_FILES
            || len > MAX_FILE_BYTES
            || used_bytes.saturating_add(len) > MAX_TOTAL_BYTES
        {
            inventory.skipped_files += 1;
            continue;
        }
        inspected_files += 1;
        used_bytes += len;
        match read_session(kind, &path) {
            Ok(Some(session)) => inventory.sessions.push(session),
            Ok(None) | Err(_) => inventory.skipped_files += 1,
        }
    }
    inventory
        .sessions
        .sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    Ok(inventory)
}

fn collect_files(
    kind: AgentKind,
    root: &Path,
    files: &mut Vec<(AgentKind, PathBuf, u128, u64)>,
) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| format!("{}: {error}", root.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            collect_files(kind, &entry.path(), files)?;
        } else if file_type.is_file() && entry.path().extension().is_some_and(|ext| ext == "jsonl")
        {
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |time| time.as_nanos());
            files.push((kind, entry.path(), modified, metadata.len()));
        }
    }
    Ok(())
}

fn read_session(kind: AgentKind, path: &Path) -> Result<Option<HistorySession>, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let mut id = None;
    let mut cwd = None;
    let mut events = VecDeque::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if let Some((candidate_id, candidate_cwd)) = metadata(kind, &value) {
            id = id.or(candidate_id);
            cwd = cwd.or(candidate_cwd);
        }
        for event in extract_events(kind, &value) {
            if events.len() == MAX_EVENTS {
                events.pop_front();
            }
            events.push_back(event);
        }
    }
    let Some(id) = id else { return Ok(None) };
    let cwd = cwd.unwrap_or_else(|| "?".into());
    let project = Path::new(&cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("?")
        .to_owned();
    let updated_at = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs() as i64);
    Ok(Some(HistorySession {
        kind,
        id,
        cwd,
        project,
        updated_at,
        events: events.into(),
    }))
}

fn metadata(kind: AgentKind, value: &Value) -> Option<(Option<String>, Option<String>)> {
    let (id, cwd) = match kind {
        AgentKind::Codex if value["type"] == "session_meta" => (
            value["payload"]["id"].as_str(),
            value["payload"]["cwd"].as_str(),
        ),
        AgentKind::Claude => (value["sessionId"].as_str(), value["cwd"].as_str()),
        AgentKind::Pi if value["type"] == "session" => {
            (value["id"].as_str(), value["cwd"].as_str())
        }
        _ => return None,
    };
    Some((id.map(str::to_owned), cwd.map(str::to_owned)))
}

fn extract_events(kind: AgentKind, value: &Value) -> Vec<HistoryEvent> {
    let timestamp = value["timestamp"].as_str().map(str::to_owned);
    let mut result = Vec::new();
    match kind {
        AgentKind::Codex if value["type"] == "response_item" => {
            let payload = &value["payload"];
            match payload["type"].as_str() {
                Some("message" | "agent_message") => {
                    let event_kind = match payload["role"].as_str() {
                        Some("user") => HistoryEventKind::User,
                        Some("assistant") => HistoryEventKind::Assistant,
                        _ => return result,
                    };
                    push_content(&mut result, event_kind, timestamp, &payload["content"]);
                }
                Some("function_call" | "custom_tool_call") => {
                    let name = payload["name"].as_str().unwrap_or("tool");
                    let args = payload["arguments"]
                        .as_str()
                        .or_else(|| payload["input"].as_str())
                        .unwrap_or("");
                    result.push(event(
                        HistoryEventKind::Tool,
                        timestamp,
                        format!("{name} {args}"),
                    ));
                }
                Some("function_call_output" | "custom_tool_call_output") => {
                    let output = &payload["output"];
                    let text = output
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| text_blocks(output).join(" "));
                    if !text.trim().is_empty() {
                        result.push(event(HistoryEventKind::Result, timestamp, text));
                    }
                }
                _ => {}
            }
        }
        AgentKind::Claude if value["type"] == "user" || value["type"] == "assistant" => {
            let event_kind = if value["type"] == "user" {
                HistoryEventKind::User
            } else {
                HistoryEventKind::Assistant
            };
            push_content(
                &mut result,
                event_kind,
                timestamp,
                &value["message"]["content"],
            );
        }
        AgentKind::Pi if value["type"] == "message" => {
            let event_kind = match value["message"]["role"].as_str() {
                Some("user") => HistoryEventKind::User,
                Some("assistant") => HistoryEventKind::Assistant,
                Some("toolResult") => HistoryEventKind::Result,
                _ => return result,
            };
            push_content(
                &mut result,
                event_kind,
                timestamp,
                &value["message"]["content"],
            );
        }
        _ => {}
    }
    result
}

fn push_content(
    events: &mut Vec<HistoryEvent>,
    kind: HistoryEventKind,
    timestamp: Option<String>,
    content: &Value,
) {
    if let Some(text) = content.as_str() {
        if !text.trim().is_empty() {
            events.push(event(kind, timestamp, text.to_owned()));
        }
        return;
    }
    let Some(blocks) = content.as_array() else {
        return;
    };
    for block in blocks {
        let block_kind = block["type"].as_str().unwrap_or("");
        match block_kind {
            "text" | "input_text" | "output_text"
                if kind != HistoryEventKind::User || block_kind != "output_text" =>
            {
                if let Some(text) = block["text"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                {
                    events.push(event(kind, timestamp.clone(), text.to_owned()));
                }
            }
            "tool_use" | "toolCall" => {
                let name = block["name"].as_str().unwrap_or("tool");
                let input = block.get("input").or_else(|| block.get("arguments"));
                let detail = input.map(compact_json).unwrap_or_default();
                events.push(event(
                    HistoryEventKind::Tool,
                    timestamp.clone(),
                    format!("{name} {detail}"),
                ));
            }
            "tool_result" => {
                let text = block["content"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| text_blocks(&block["content"]).join(" "));
                if !text.trim().is_empty() {
                    events.push(event(HistoryEventKind::Result, timestamp.clone(), text));
                }
            }
            _ => {}
        }
    }
}

fn text_blocks(value: &Value) -> Vec<String> {
    value.as_array().map_or_else(Vec::new, |items| {
        items
            .iter()
            .filter_map(|item| item["text"].as_str().map(str::to_owned))
            .collect()
    })
}

fn compact_json(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn event(kind: HistoryEventKind, timestamp: Option<String>, text: String) -> HistoryEvent {
    let text = text.chars().take(MAX_EVENT_CHARS).collect::<String>();
    HistoryEvent {
        timestamp,
        kind,
        text,
    }
}

pub fn search_sessions(sessions: &[HistorySession], query: &str) -> Vec<HistoryHit> {
    let query = query.to_lowercase();
    let terms = query.split_whitespace().collect::<Vec<_>>();
    if terms.is_empty() {
        return sessions
            .iter()
            .enumerate()
            .map(|(session_index, session)| HistoryHit {
                session_index,
                event_index: None,
                snippet: session.events.last().map_or_else(
                    || "No conversation events".into(),
                    |event| event.text.clone(),
                ),
                match_count: 0,
            })
            .collect();
    }
    let mut hits = Vec::new();
    for (session_index, session) in sessions.iter().enumerate() {
        let identity =
            format!("{} {} {}", session.kind, session.project, session.cwd).to_lowercase();
        if terms.iter().all(|term| identity.contains(term)) {
            hits.push(HistoryHit {
                session_index,
                event_index: None,
                snippet: session
                    .events
                    .last()
                    .map_or_else(|| "Session metadata".into(), |event| event.text.clone()),
                match_count: 0,
            });
            continue;
        }
        let mut first_match = None;
        let mut match_count = 0;
        for (event_index, event) in session.events.iter().enumerate() {
            let content = event.text.to_lowercase();
            if terms.iter().all(|term| content.contains(term)) {
                first_match = Some((event_index, event));
                match_count += 1;
            }
        }
        if let Some((event_index, event)) = first_match {
            hits.push(HistoryHit {
                session_index,
                event_index: Some(event_index),
                snippet: event.text.clone(),
                match_count,
            });
        }
    }
    hits
}
