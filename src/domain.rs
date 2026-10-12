use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentKind {
    Codex,
    Claude,
    OpenCode,
    Pi,
    Gemini,
    Cursor,
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codex => write!(f, "Codex"),
            Self::Claude => write!(f, "Claude"),
            Self::OpenCode => write!(f, "OpenCode"),
            Self::Pi => write!(f, "Pi"),
            Self::Gemini => write!(f, "Gemini"),
            Self::Cursor => write!(f, "Cursor"),
        }
    }
}

impl AgentKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Codex" => Some(Self::Codex),
            "Claude" => Some(Self::Claude),
            "OpenCode" => Some(Self::OpenCode),
            "Pi" => Some(Self::Pi),
            "Gemini" => Some(Self::Gemini),
            "Cursor" => Some(Self::Cursor),
            _ => None,
        }
    }

    pub fn herdr_label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::Gemini => "gemini",
            Self::Cursor => "cursor-agent",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Working,
    Idle,
    Waiting,
    Finished,
    Failed,
    Unknown,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Waiting => "waiting",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub fn display_label(self) -> &'static str {
        match self {
            Self::Waiting => "waiting for input",
            _ => self.label(),
        }
    }

    pub fn compact_display_label(self) -> &'static str {
        match self {
            Self::Waiting => "needs input",
            _ => self.display_label(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeMetadata {
    pub provider: String,
    pub target: String,
    pub fields: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentObservation {
    pub id: String,
    pub kind: AgentKind,
    pub project: String,
    pub state: AgentState,
    pub pid: u32,
    pub cwd: String,
    pub runtime: RuntimeMetadata,
    pub recent_output: String,
    pub output_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentView {
    pub observation: AgentObservation,
    pub activity: String,
    pub last_output_change: Option<i64>,
}

pub trait Runtime: Send + Sync {
    fn observe(&self) -> Result<Vec<AgentObservation>, String>;
}
