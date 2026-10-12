use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io::{self, IsTerminal};
use std::sync::mpsc::{self, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::{Frame, Terminal};

use crate::config::{AppConfig, LaunchDestination};
use crate::domain::{AgentKind, AgentObservation, AgentState, AgentView, Runtime};
use crate::evidence::derive_activity;
use crate::herdr::{focus_agent_pane, read_recent_output};
use crate::history::{scan_history, search_sessions, HistoryHit, HistoryInventory};
use crate::launch::{self, Destination, LaunchRequest, LaunchedAgent};
use crate::projects::{inspect_projects, ProjectSnapshot};
use crate::search::{SearchIndex, SearchResult, SearchTarget};
use crate::store::StateStore;
use crate::terminals::{
    focus_target as focus_terminal_target, list_all as list_terminals, list_native, matching_tty,
    parse_target, read_visible, Candidate as TerminalCandidate, Inventory as TerminalInventory,
    Provider,
};

const HISTORY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const PROJECT_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Search,
    InspectAgent(String),
    History,
    InspectHistory(usize, Option<usize>),
    Projects,
    InspectProject(String),
}

pub struct DashboardState {
    agents: Vec<AgentView>,
    agent_selected: Option<usize>,
    screen: Screen,
    error: Option<String>,
    notice: Option<String>,
    detail_scroll: u16,
    terminal_picker: Option<TerminalPicker>,
    launch_form: Option<LaunchForm>,
    launch_default: LaunchDestination,
    alias_form: Option<AliasForm>,
    aliases: HashMap<String, String>,
    terminal_links: HashMap<String, String>,
    inspection_preview: Option<InspectionPreview>,
    search_query: Option<String>,
    unified_query: String,
    unified_results: Vec<SearchResult>,
    unified_selected: Option<usize>,
    unified_error: Option<String>,
    unified_loading: bool,
    unified_origin: Option<Screen>,
    unified_detail: Option<SearchDetail>,
    history: HistoryInventory,
    history_selected: Option<usize>,
    history_hits: Vec<HistoryHit>,
    history_query: Option<String>,
    history_loaded: bool,
    history_loading: bool,
    history_error: Option<String>,
    history_return_agent: Option<String>,
    projects: Vec<ProjectSnapshot>,
    project_selected: Option<usize>,
    projects_loaded: bool,
    projects_loading: bool,
    projects_error: Option<String>,
    project_preselect_agent: Option<String>,
    project_return_agent: Option<String>,
}

struct InspectionPreview {
    agent_id: String,
    source: String,
    text: String,
}

type PreviewResult = (String, Result<(String, String), String>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchDetail {
    Live,
    Saved,
}

enum SearchCommand {
    Live(Vec<AgentView>),
    History(HistoryInventory),
    Query(u64, String),
}

struct TerminalPicker {
    agent_id: String,
    terminals: Vec<TerminalCandidate>,
    selected: Option<usize>,
}

struct AliasForm {
    session_key: String,
    text: String,
}

#[derive(Clone, Debug)]
struct LaunchForm {
    cwd: String,
    kind: AgentKind,
    destination: Destination,
    herdr_anchor: Option<String>,
    field: usize,
}

impl LaunchForm {
    fn choices() -> [AgentKind; 6] {
        [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::OpenCode,
            AgentKind::Pi,
            AgentKind::Gemini,
            AgentKind::Cursor,
        ]
    }

    fn destinations(&self) -> Vec<Destination> {
        let mut choices = Vec::new();
        if self.herdr_anchor.is_some() {
            choices.push(Destination::Herdr);
        }
        choices.extend([
            Destination::Terminal(Provider::Ghostty),
            Destination::Terminal(Provider::Terminal),
            Destination::Terminal(Provider::ITerm2),
        ]);
        choices
    }

    fn change(&mut self, forward: bool) {
        match self.field {
            1 => {
                let choices = Self::choices();
                let index = choices
                    .iter()
                    .position(|kind| *kind == self.kind)
                    .unwrap_or(0);
                self.kind = choices[if forward {
                    (index + 1) % choices.len()
                } else {
                    (index + choices.len() - 1) % choices.len()
                }];
            }
            2 => {
                let choices = self.destinations();
                let index = choices
                    .iter()
                    .position(|choice| *choice == self.destination)
                    .unwrap_or(0);
                self.destination = choices[if forward {
                    (index + 1) % choices.len()
                } else {
                    (index + choices.len() - 1) % choices.len()
                }];
            }
            _ => {}
        }
    }

    fn request(&self) -> LaunchRequest {
        LaunchRequest {
            kind: self.kind,
            cwd: self.cwd.clone().into(),
            destination: self.destination,
            herdr_anchor: self.herdr_anchor.clone(),
        }
    }
}

impl DashboardState {
    pub fn new(agents: Vec<AgentView>) -> Self {
        Self {
            agent_selected: (!agents.is_empty()).then_some(0),
            agents,
            screen: Screen::Dashboard,
            error: None,
            notice: None,
            detail_scroll: 0,
            terminal_picker: None,
            launch_form: None,
            launch_default: LaunchDestination::Auto,
            alias_form: None,
            aliases: HashMap::new(),
            terminal_links: HashMap::new(),
            inspection_preview: None,
            search_query: None,
            unified_query: String::new(),
            unified_results: Vec::new(),
            unified_selected: None,
            unified_error: None,
            unified_loading: false,
            unified_origin: None,
            unified_detail: None,
            history: HistoryInventory::default(),
            history_selected: None,
            history_hits: Vec::new(),
            history_query: None,
            history_loaded: false,
            history_loading: false,
            history_error: None,
            history_return_agent: None,
            projects: Vec::new(),
            project_selected: None,
            projects_loaded: false,
            projects_loading: false,
            projects_error: None,
            project_preselect_agent: None,
            project_return_agent: None,
        }
    }

    pub fn screen(&self) -> Screen {
        self.screen.clone()
    }

    pub fn selected_agent(&self) -> Option<&AgentView> {
        self.agent_selected
            .filter(|index| self.visible_indices().contains(index))
            .and_then(|index| self.agents.get(index))
    }

    fn visible_indices(&self) -> Vec<usize> {
        let query = self.search_query.as_deref().unwrap_or("").to_lowercase();
        let terms = query.split_whitespace().collect::<Vec<_>>();
        self.agents
            .iter()
            .enumerate()
            .filter(|(_, view)| {
                let observation = &view.observation;
                let haystack = format!(
                    "{} {} {} {} {}",
                    observation.kind,
                    observation.project,
                    observation.cwd,
                    self.session_label(observation),
                    self.location_label(observation)
                )
                .to_lowercase();
                terms.iter().all(|term| haystack.contains(term))
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub fn visible_agents(&self) -> Vec<&AgentView> {
        self.visible_indices()
            .into_iter()
            .filter_map(|index| self.agents.get(index))
            .collect()
    }

    pub fn search_query(&self) -> Option<&str> {
        self.search_query.as_deref()
    }

    pub fn history_query(&self) -> Option<&str> {
        self.history_query.as_deref()
    }

    pub fn set_projects(&mut self, projects: Vec<ProjectSnapshot>) {
        let selected_root = self.selected_project().map(|project| project.root.clone());
        let preselect_agent = self.project_preselect_agent.take();
        self.projects = projects;
        self.projects_loaded = true;
        self.projects_loading = false;
        self.projects_error = None;
        self.project_selected = preselect_agent
            .as_deref()
            .and_then(|id| {
                self.projects
                    .iter()
                    .position(|project| project.agents.iter().any(|agent| agent.id == id))
            })
            .or_else(|| {
                selected_root.and_then(|root| {
                    self.projects
                        .iter()
                        .position(|project| project.root == root)
                })
            })
            .or_else(|| (!self.projects.is_empty()).then_some(0));
        if matches!(&self.screen, Screen::InspectProject(root) if !self.projects.iter().any(|project| &project.root == root))
        {
            self.screen = Screen::Projects;
        }
    }

    pub fn selected_project(&self) -> Option<&ProjectSnapshot> {
        self.project_selected
            .and_then(|index| self.projects.get(index))
    }

    pub fn open_projects(&mut self) {
        self.project_return_agent = match &self.screen {
            Screen::InspectAgent(id) => Some(id.clone()),
            _ => None,
        };
        self.project_preselect_agent = self.selected_live_agent().map(|agent| agent.id.clone());
        self.screen = Screen::Projects;
        self.detail_scroll = 0;
        if let Some(id) = &self.project_preselect_agent {
            if let Some(index) = self
                .projects
                .iter()
                .position(|project| project.agents.iter().any(|agent| &agent.id == id))
            {
                self.project_selected = Some(index);
                self.project_preselect_agent = None;
            }
        }
    }

    pub fn set_history(&mut self, history: HistoryInventory) {
        let detail_key = match self.screen {
            Screen::InspectHistory(index, _) => self
                .history
                .sessions
                .get(index)
                .map(|session| (session.kind, session.id.clone())),
            _ => None,
        };
        let selected_key = self
            .history_selected
            .and_then(|index| self.history_hits.get(index))
            .and_then(|hit| self.history.sessions.get(hit.session_index))
            .map(|session| (session.kind, session.id.clone()));
        self.history = history;
        self.history_loaded = true;
        self.history_loading = false;
        self.history_error = None;
        self.reselect_history();
        if let Some(key) = selected_key {
            if let Some(position) = self.history_hits.iter().position(|hit| {
                self.history
                    .sessions
                    .get(hit.session_index)
                    .is_some_and(|session| {
                        (session.kind, session.id.as_str()) == (key.0, key.1.as_str())
                    })
            }) {
                self.history_selected = Some(position);
            }
        }
        if let Some(key) = detail_key {
            self.screen = self
                .history
                .sessions
                .iter()
                .position(|session| session.kind == key.0 && session.id == key.1)
                .map_or(Screen::History, |index| Screen::InspectHistory(index, None));
        }
    }

    pub fn visible_history_hits(&self) -> &[HistoryHit] {
        &self.history_hits
    }

    fn reselect_history(&mut self) {
        self.history_hits = search_sessions(
            &self.history.sessions,
            self.history_query.as_deref().unwrap_or(""),
        );
        let len = self.history_hits.len();
        self.history_selected = (len > 0).then(|| self.history_selected.unwrap_or(0).min(len - 1));
    }

    pub fn open_history(&mut self) {
        self.screen = Screen::History;
        self.detail_scroll = 0;
        self.history_return_agent = None;
        self.reselect_history();
    }

    pub fn open_exact_history_for_agent(&mut self) -> bool {
        let Some(agent) = self.selected_live_agent() else {
            return false;
        };
        let Some(session_id) = agent
            .runtime
            .fields
            .iter()
            .find(|(key, _)| key == "Session ID")
            .map(|(_, value)| value.as_str())
        else {
            return false;
        };
        let Some(index) = self
            .history
            .sessions
            .iter()
            .position(|session| session.kind == agent.kind && session.id == session_id)
        else {
            return false;
        };
        self.history_return_agent = Some(agent.id.clone());
        self.screen = Screen::InspectHistory(index, None);
        self.detail_scroll = 0;
        true
    }

    pub fn start_search(&mut self) {
        match self.screen {
            Screen::Dashboard => {
                self.search_query = Some(String::new());
                self.reselect_visible();
            }
            Screen::History => {
                self.history_query = Some(String::new());
                self.reselect_history();
            }
            _ => {}
        }
    }

    pub fn search_push(&mut self, character: char) {
        if self.screen == Screen::History {
            if let Some(query) = self.history_query.as_mut() {
                query.push(character);
                self.history_selected = Some(0);
                self.reselect_history();
            }
            return;
        }
        if let Some(query) = self.search_query.as_mut() {
            query.push(character);
            self.reselect_visible();
        }
    }

    pub fn search_backspace(&mut self) {
        if self.screen == Screen::History {
            if let Some(query) = self.history_query.as_mut() {
                query.pop();
                self.history_selected = Some(0);
                self.reselect_history();
            }
            return;
        }
        if let Some(query) = self.search_query.as_mut() {
            query.pop();
            self.reselect_visible();
        }
    }

    pub fn clear_search(&mut self) {
        if self.screen == Screen::History {
            self.history_query = None;
            self.reselect_history();
            return;
        }
        self.search_query = None;
        self.reselect_visible();
    }

    fn reselect_visible(&mut self) {
        let visible = self.visible_indices();
        if !self
            .agent_selected
            .is_some_and(|index| visible.contains(&index))
        {
            self.agent_selected = visible.first().copied();
        }
    }

    pub fn set_agents(&mut self, mut agents: Vec<AgentView>) {
        agents.sort_by(|a, b| {
            (
                a.observation.project.to_lowercase(),
                a.observation.kind.to_string(),
                session_sort_key(&a.observation),
            )
                .cmp(&(
                    b.observation.project.to_lowercase(),
                    b.observation.kind.to_string(),
                    session_sort_key(&b.observation),
                ))
        });
        let previous_id = self
            .selected_agent()
            .map(|agent| agent.observation.id.clone());
        let previous_index = self.agent_selected.unwrap_or(0);
        self.agent_selected = select_after_refresh(
            agents.len(),
            previous_index,
            previous_id.and_then(|id| agents.iter().position(|agent| agent.observation.id == id)),
        );
        self.agents = agents;
        self.reselect_visible();
    }

    fn session_label(&self, agent: &AgentObservation) -> String {
        self.aliases
            .get(&alias_key(agent))
            .cloned()
            .unwrap_or_else(|| fallback_session_label(agent))
    }

    fn location_label(&self, agent: &AgentObservation) -> String {
        location_label(agent, self.terminal_link_for(agent))
    }

    fn compact_location_label(&self, agent: &AgentObservation) -> String {
        compact_location_label(agent, self.terminal_link_for(agent))
    }

    fn terminal_link_for<'a>(&'a self, agent: &AgentObservation) -> Option<&'a str> {
        let link_key = if agent.runtime.provider == "Herdr" {
            herdr_host_key(agent)
        } else {
            agent.id.clone()
        };
        self.terminal_links.get(&link_key).map(String::as_str)
    }

    pub fn working_count(&self) -> usize {
        self.agents
            .iter()
            .filter(|agent| agent.observation.state == AgentState::Working)
            .count()
    }

    pub fn is_stale(&self) -> bool {
        self.error.is_some()
    }

    pub fn mark_error(&mut self, message: String) {
        self.error = Some(message);
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    pub fn focus_target(&self) -> Option<&str> {
        let id = self.selected_live_agent()?.id.as_str();
        self.agents
            .iter()
            .find(|agent| {
                agent.observation.id == id && agent.observation.runtime.provider == "Herdr"
            })
            .map(|agent| agent.observation.runtime.target.as_str())
    }

    fn selected_live_agent(&self) -> Option<&AgentObservation> {
        if self.is_stale() {
            return None;
        }
        let id = match &self.screen {
            Screen::Dashboard => self
                .selected_agent()
                .map(|agent| agent.observation.id.as_str()),
            Screen::InspectAgent(id) => Some(id.as_str()),
            Screen::History
            | Screen::Search
            | Screen::InspectHistory(_, _)
            | Screen::Projects
            | Screen::InspectProject(_) => None,
        }?;
        self.agents
            .iter()
            .find(|agent| agent.observation.id == id)
            .map(|view| &view.observation)
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    fn open_unified_search(&mut self) {
        self.unified_origin = Some(self.screen.clone());
        self.unified_detail = None;
        self.unified_query.clear();
        self.unified_results.clear();
        self.unified_selected = None;
        self.unified_error = None;
        self.unified_loading = false;
        self.screen = Screen::Search;
    }

    fn edit_unified_search(&mut self) {
        self.unified_results.clear();
        self.unified_selected = None;
        self.unified_error = None;
        self.unified_loading = !self.unified_query.trim().is_empty();
    }

    fn set_unified_results(&mut self, result: Result<Vec<SearchResult>, String>) {
        self.unified_loading = false;
        match result {
            Ok(results) => {
                let selected = self
                    .unified_selected
                    .and_then(|index| self.unified_results.get(index))
                    .map(|hit| hit.target.clone());
                self.unified_selected = (!results.is_empty()).then(|| {
                    selected
                        .and_then(|target| results.iter().position(|hit| hit.target == target))
                        .unwrap_or(0)
                });
                self.unified_results = results;
                self.unified_error = None;
            }
            Err(error) => {
                self.unified_results.clear();
                self.unified_selected = None;
                self.unified_error = Some(error);
            }
        }
    }

    fn inspect_unified_result(&mut self) -> bool {
        let Some(hit) = self
            .unified_selected
            .and_then(|index| self.unified_results.get(index))
        else {
            return false;
        };
        match &hit.target {
            SearchTarget::Live(id) => {
                if !self.agents.iter().any(|view| &view.observation.id == id) {
                    self.set_notice("This agent is no longer live. Refresh the search.");
                    return false;
                }
                self.screen = Screen::InspectAgent(id.clone());
                self.unified_detail = Some(SearchDetail::Live);
            }
            SearchTarget::Saved {
                kind,
                session_id,
                event_index,
            } => {
                let Some(index) = self
                    .history
                    .sessions
                    .iter()
                    .position(|session| session.kind == *kind && session.id == *session_id)
                else {
                    self.set_notice("This saved session changed. Refresh the search.");
                    return false;
                };
                self.screen = Screen::InspectHistory(index, Some(*event_index));
                self.unified_detail = Some(SearchDetail::Saved);
            }
        }
        self.detail_scroll = 0;
        true
    }

    fn open_launch_form(&mut self) {
        let herdr_anchor = self.herdr_launch_anchor();
        let cwd = match &self.screen {
            Screen::Projects | Screen::InspectProject(_) => {
                self.selected_project().map(|project| project.root.clone())
            }
            _ => self
                .selected_agent()
                .map(|agent| agent.observation.cwd.clone()),
        }
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
        let kind = self
            .selected_agent()
            .map_or(AgentKind::Codex, |agent| agent.observation.kind);
        let mut form = LaunchForm {
            cwd,
            kind,
            destination: Destination::Terminal(Provider::Ghostty),
            herdr_anchor,
            field: 0,
        };
        form.destination = select_launch_destination(self.launch_default, &form.destinations());
        self.launch_form = Some(form);
    }

    fn herdr_launch_anchor(&self) -> Option<String> {
        if self.is_stale() {
            return if std::env::var("HERDR_ENV").ok().as_deref() == Some("1") {
                std::env::var("HERDR_PANE_ID").ok()
            } else {
                None
            };
        }
        if let Some(agent) = self
            .selected_live_agent()
            .filter(|agent| agent.runtime.provider == "Herdr")
        {
            return Some(agent.runtime.target.clone());
        }
        if std::env::var("HERDR_ENV").ok().as_deref() == Some("1") {
            return std::env::var("HERDR_PANE_ID").ok();
        }
        let mut herdr = self
            .agents
            .iter()
            .filter(|view| view.observation.runtime.provider == "Herdr");
        let first = herdr.next()?;
        let workspace = first
            .observation
            .runtime
            .fields
            .iter()
            .find(|(key, _)| key == "Workspace")
            .map(|(_, value)| value.as_str());
        if herdr.all(|view| {
            view.observation
                .runtime
                .fields
                .iter()
                .find(|(key, _)| key == "Workspace")
                .map(|(_, value)| value.as_str())
                == workspace
        }) {
            Some(first.observation.runtime.target.clone())
        } else {
            None
        }
    }

    pub fn inspect(&mut self) {
        self.detail_scroll = 0;
        match self.screen {
            Screen::Search => {
                self.inspect_unified_result();
            }
            Screen::Dashboard => {
                if let Some(agent) = self.selected_agent() {
                    self.screen = Screen::InspectAgent(agent.observation.id.clone());
                }
            }
            Screen::InspectAgent(_) => {}
            Screen::History => {
                let hit = self
                    .history_selected
                    .and_then(|index| self.visible_history_hits().get(index).cloned());
                if let Some(hit) = hit {
                    self.screen = Screen::InspectHistory(hit.session_index, hit.event_index);
                }
            }
            Screen::InspectHistory(_, _) => {}
            Screen::Projects => {
                if let Some(project) = self.selected_project() {
                    self.screen = Screen::InspectProject(project.root.clone());
                }
            }
            Screen::InspectProject(_) => {}
        }
    }

    pub fn back(&mut self) {
        self.detail_scroll = 0;
        if matches!(
            (&self.screen, self.unified_detail),
            (Screen::InspectAgent(_), Some(SearchDetail::Live))
                | (Screen::InspectHistory(_, _), Some(SearchDetail::Saved))
        ) {
            self.screen = Screen::Search;
            self.unified_detail = None;
            return;
        }
        self.screen = match &self.screen {
            Screen::Search => self.unified_origin.take().unwrap_or(Screen::Dashboard),
            Screen::InspectAgent(_) => Screen::Dashboard,
            Screen::InspectHistory(_, _) => self
                .history_return_agent
                .take()
                .map_or(Screen::History, Screen::InspectAgent),
            Screen::History => Screen::Dashboard,
            Screen::InspectProject(_) => Screen::Projects,
            Screen::Projects => self
                .project_return_agent
                .take()
                .filter(|id| self.agents.iter().any(|agent| &agent.observation.id == id))
                .map_or(Screen::Dashboard, Screen::InspectAgent),
            Screen::Dashboard => Screen::Dashboard,
        };
    }

    pub fn next(&mut self) {
        match self.screen {
            Screen::Search => {
                self.unified_selected =
                    next_index(self.unified_selected, self.unified_results.len())
            }
            Screen::Dashboard => {
                let visible = self.visible_indices();
                let position = self
                    .agent_selected
                    .and_then(|selected| visible.iter().position(|index| *index == selected));
                self.agent_selected = next_index(position, visible.len())
                    .and_then(|position| visible.get(position).copied());
            }
            Screen::InspectAgent(_) => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
            }
            Screen::History => {
                self.history_selected =
                    next_index(self.history_selected, self.visible_history_hits().len());
            }
            Screen::InspectHistory(_, _) => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
            }
            Screen::Projects => {
                self.project_selected = next_index(self.project_selected, self.projects.len());
            }
            Screen::InspectProject(_) => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
            }
        }
    }

    pub fn previous(&mut self) {
        match self.screen {
            Screen::Search => {
                self.unified_selected =
                    previous_index(self.unified_selected, self.unified_results.len())
            }
            Screen::Dashboard => {
                let visible = self.visible_indices();
                let position = self
                    .agent_selected
                    .and_then(|selected| visible.iter().position(|index| *index == selected));
                self.agent_selected = previous_index(position, visible.len())
                    .and_then(|position| visible.get(position).copied());
            }
            Screen::InspectAgent(_) => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
            }
            Screen::History => {
                self.history_selected =
                    previous_index(self.history_selected, self.visible_history_hits().len());
            }
            Screen::InspectHistory(_, _) => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
            }
            Screen::Projects => {
                self.project_selected = previous_index(self.project_selected, self.projects.len());
            }
            Screen::InspectProject(_) => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
            }
        }
    }
}

fn select_after_refresh(len: usize, previous: usize, matching: Option<usize>) -> Option<usize> {
    (len > 0).then(|| matching.unwrap_or(previous.min(len - 1)))
}

fn select_launch_destination(
    preferred: LaunchDestination,
    available: &[Destination],
) -> Destination {
    let configured = match preferred {
        LaunchDestination::Auto => None,
        LaunchDestination::Herdr => Some(Destination::Herdr),
        LaunchDestination::Ghostty => Some(Destination::Terminal(Provider::Ghostty)),
        LaunchDestination::Terminal => Some(Destination::Terminal(Provider::Terminal)),
        LaunchDestination::ITerm2 => Some(Destination::Terminal(Provider::ITerm2)),
    };
    configured
        .filter(|destination| available.contains(destination))
        .unwrap_or(available[0])
}

fn next_index(current: Option<usize>, len: usize) -> Option<usize> {
    (len > 0).then(|| (current.unwrap_or(0) + 1) % len)
}

fn previous_index(current: Option<usize>, len: usize) -> Option<usize> {
    (len > 0).then(|| (current.unwrap_or(0) + len - 1) % len)
}

struct TerminalMode;

impl TerminalMode {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self)
    }
}

impl Drop for TerminalMode {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

pub fn run(runtime: impl Runtime + 'static, config: AppConfig) -> Result<(), Box<dyn Error>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("AM2 needs a terminal for the dashboard; use `am2 list` in scripts".into());
    }
    let mut store = StateStore::open_default()?;
    let (request_tx, request_rx) = mpsc::sync_channel::<()>(1);
    let (update_tx, update_rx) = mpsc::channel::<Result<Vec<AgentObservation>, String>>();
    thread::spawn(move || {
        while request_rx.recv().is_ok() {
            if update_tx.send(runtime.observe()).is_err() {
                break;
            }
        }
    });

    let _mode = TerminalMode::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let result = run_loop(&mut terminal, &mut store, request_tx, update_rx, config);
    terminal.show_cursor()?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &mut StateStore,
    request_tx: mpsc::SyncSender<()>,
    update_rx: mpsc::Receiver<Result<Vec<AgentObservation>, String>>,
    config: AppConfig,
) -> Result<(), Box<dyn Error>> {
    let mut state = DashboardState::new(Vec::new());
    state.launch_default = config.launch.default_terminal;
    let refresh_interval = Duration::from_secs(config.dashboard.refresh_interval_seconds);
    state.set_notice("Discovering agents…");
    let mut draw_requested = true;
    let mut refresh_now = true;
    let mut in_flight = false;
    let mut last_refresh = Instant::now();
    let (focus_tx, focus_rx) = mpsc::channel::<Result<String, String>>();
    let mut focus_in_flight = false;
    let (launch_tx, launch_rx) = mpsc::channel::<Result<LaunchedAgent, String>>();
    let mut launch_in_flight = false;
    let mut pending_launch: Option<(LaunchedAgent, Instant)> = None;
    let (link_tx, link_rx) = mpsc::channel::<(u64, String, Result<TerminalInventory, String>)>();
    let mut link_in_flight = false;
    let mut link_generation = 0_u64;
    let (preview_tx, preview_rx) = mpsc::channel::<PreviewResult>();
    let mut preview_in_flight: Option<String> = None;
    let (history_tx, history_rx) = mpsc::channel::<Result<HistoryInventory, String>>();
    let (search_tx, search_commands) = mpsc::channel::<SearchCommand>();
    let (search_results_tx, search_results_rx) =
        mpsc::channel::<(u64, Result<Vec<SearchResult>, String>)>();
    thread::spawn(move || {
        let mut index = SearchIndex::new();
        while let Ok(command) = search_commands.recv() {
            match command {
                SearchCommand::Live(agents) => {
                    if let Ok(current) = index.as_mut() {
                        if let Err(error) = current.replace_live(&agents) {
                            index = Err(error);
                        }
                    }
                }
                SearchCommand::History(history) => {
                    if let Ok(current) = index.as_mut() {
                        if let Err(error) = current.replace_history(&history) {
                            index = Err(error);
                        }
                    }
                }
                SearchCommand::Query(generation, query) => {
                    let result = index
                        .as_ref()
                        .map_err(Clone::clone)
                        .and_then(|index| index.query(&query));
                    if search_results_tx.send((generation, result)).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut search_dirty: Option<Instant> = None;
    let mut search_generation = 0_u64;
    let mut last_history_refresh: Option<Instant> = None;
    let (projects_tx, projects_rx) = mpsc::channel::<Vec<ProjectSnapshot>>();
    let mut last_project_refresh: Option<Instant> = None;

    loop {
        if !in_flight && (refresh_now || last_refresh.elapsed() >= refresh_interval) {
            match request_tx.try_send(()) {
                Ok(()) => {
                    in_flight = true;
                    refresh_now = false;
                }
                Err(TrySendError::Disconnected(_)) => {
                    state.mark_error("Refresh worker stopped".into());
                    draw_requested = true;
                    refresh_now = false;
                }
                Err(TrySendError::Full(_)) => {}
            }
        }

        match update_rx.try_recv() {
            Ok(result) => {
                draw_requested = true;
                in_flight = false;
                last_refresh = Instant::now();
                if state.notice.as_deref() == Some("Discovering agents…") {
                    state.notice = None;
                }
                match store.apply_refresh(result, unix_now()) {
                    Ok(agents) => {
                        state.set_agents(agents);
                        state.aliases.clear();
                        state.terminal_links.clear();
                        for view in &state.agents {
                            let agent = &view.observation;
                            let key = alias_key(agent);
                            if let Some(alias) = store.alias(&key)? {
                                state.aliases.insert(key, alias);
                            }
                            if let Some(link) = store.terminal_link(&agent.id)? {
                                state.terminal_links.insert(agent.id.clone(), link);
                            }
                            if agent.runtime.provider == "Herdr" {
                                let host_key = herdr_host_key(agent);
                                if let std::collections::hash_map::Entry::Vacant(entry) =
                                    state.terminal_links.entry(host_key.clone())
                                {
                                    if let Some(link) = store.terminal_link(&host_key)? {
                                        entry.insert(link);
                                    }
                                }
                            }
                        }
                        if last_history_refresh.is_none() && !state.history_loading {
                            start_history_refresh(&mut state, &history_tx, &search_tx);
                        }
                        let _ = search_tx.send(SearchCommand::Live(state.agents.clone()));
                        if state.screen == Screen::Search && !state.unified_query.trim().is_empty()
                        {
                            search_generation = search_generation.wrapping_add(1);
                            search_dirty = Some(Instant::now());
                        }
                        state.clear_error();
                        if let Some((launched, _)) = pending_launch.as_ref() {
                            if let Some(index) = state.agents.iter().position(|view| {
                                view.observation.pid == launched.pid
                                    && view.observation.kind == launched.kind
                            }) {
                                let id = state.agents[index].observation.id.clone();
                                if matches!(launched.destination, Destination::Terminal(_)) {
                                    store.link_terminal(&id, &launched.target)?;
                                    state
                                        .terminal_links
                                        .insert(id.clone(), launched.target.clone());
                                }
                                state.agent_selected = Some(index);
                                state.screen = Screen::Dashboard;
                                state.set_notice(format!(
                                    "Launched {} in {}. Press o to return.",
                                    launched.kind,
                                    launched.destination.label()
                                ));
                                pending_launch = None;
                            }
                        }
                    }
                    Err(error) => state.mark_error(error.to_string()),
                }
            }
            Err(TryRecvError::Disconnected) if in_flight => {
                draw_requested = true;
                in_flight = false;
                state.mark_error("Refresh worker stopped".into());
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
        }

        if let Ok(result) = focus_rx.try_recv() {
            draw_requested = true;
            focus_in_flight = false;
            match result {
                Ok(target) => state.set_notice(format!("Opened {target}")),
                Err(error) => state.set_notice(format!("Could not jump to agent: {error}")),
            }
        }

        if let Ok(result) = launch_rx.try_recv() {
            draw_requested = true;
            launch_in_flight = false;
            match result {
                Ok(launched) => {
                    state.set_notice(format!(
                        "{} started in {}; waiting for live inventory…",
                        launched.kind,
                        launched.destination.label()
                    ));
                    pending_launch = Some((launched, Instant::now()));
                    refresh_now = true;
                }
                Err(error) => state.set_notice(format!("Launch issue: {error}")),
            }
        }
        if pending_launch
            .as_ref()
            .is_some_and(|(_, started)| started.elapsed() >= Duration::from_secs(20))
        {
            pending_launch = None;
            state.set_notice("Agent was launched but did not appear in the live inventory. Check the new terminal.");
            draw_requested = true;
        }

        if let Ok((agent_id, result)) = preview_rx.try_recv() {
            draw_requested = true;
            if preview_in_flight.as_deref() != Some(agent_id.as_str()) {
                continue;
            }
            preview_in_flight = None;
            if matches!(&state.screen, Screen::InspectAgent(id) if id == &agent_id) {
                let (source, text) = match result {
                    Ok((source, text)) => (source, text),
                    Err(error) => ("Output unavailable".into(), error),
                };
                state.inspection_preview = Some(InspectionPreview {
                    agent_id,
                    source,
                    text,
                });
            }
        }

        if let Ok(result) = history_rx.try_recv() {
            draw_requested = true;
            state.history_loading = false;
            last_history_refresh = Some(Instant::now());
            match result {
                Ok(inventory) => {
                    state.set_history(inventory);
                    if state.screen == Screen::Search && !state.unified_query.trim().is_empty() {
                        search_generation = search_generation.wrapping_add(1);
                        search_dirty = Some(Instant::now());
                    }
                    if let Some(agent_id) = state.history_return_agent.take() {
                        state.screen = Screen::InspectAgent(agent_id);
                        if !state.open_exact_history_for_agent() {
                            state.open_history();
                            state.set_notice("No exact saved session matches this live agent.");
                        }
                    }
                }
                Err(error) => state.history_error = Some(error),
            }
        }

        if let Ok((generation, result)) = search_results_rx.try_recv() {
            draw_requested = true;
            if generation == search_generation && state.screen == Screen::Search {
                state.set_unified_results(result);
            }
        }
        if state.screen == Screen::Search
            && search_dirty.is_some_and(|since| since.elapsed() >= Duration::from_millis(120))
        {
            search_dirty = None;
            if !state.unified_query.trim().is_empty() {
                let _ = search_tx.send(SearchCommand::Query(
                    search_generation,
                    state.unified_query.clone(),
                ));
            }
        }

        if !state.history_loading
            && !matches!(state.screen, Screen::History | Screen::InspectHistory(_, _))
            && last_history_refresh.is_some_and(|last| last.elapsed() >= HISTORY_REFRESH_INTERVAL)
        {
            start_history_refresh(&mut state, &history_tx, &search_tx);
            draw_requested = true;
        }

        if let Ok(projects) = projects_rx.try_recv() {
            draw_requested = true;
            state.set_projects(projects);
            last_project_refresh = Some(Instant::now());
        }

        if matches!(state.screen, Screen::Projects | Screen::InspectProject(_))
            && !state.projects_loading
            && last_project_refresh.is_none_or(|last| last.elapsed() >= PROJECT_REFRESH_INTERVAL)
        {
            start_project_refresh(&mut state, &projects_tx);
            draw_requested = true;
        }

        if let Ok((generation, agent_id, result)) = link_rx.try_recv() {
            draw_requested = true;
            if generation != link_generation || !link_in_flight {
                continue;
            }
            link_in_flight = false;
            match result {
                Ok(inventory) => {
                    if state
                        .agents
                        .iter()
                        .any(|view| view.observation.id == agent_id)
                    {
                        let terminals = inventory.candidates;
                        if terminals.is_empty() {
                            state.set_notice(
                                "No supported terminal sessions are available to link.",
                            );
                        } else {
                            let selected = state
                                .agents
                                .iter()
                                .find(|view| view.observation.id == agent_id)
                                .and_then(|view| agent_tty(&view.observation))
                                .and_then(|tty| {
                                    matching_tty(&terminals, tty)
                                        .first()
                                        .map(|candidate| candidate.target())
                                })
                                .and_then(|target| {
                                    terminals
                                        .iter()
                                        .position(|candidate| candidate.target() == target)
                                })
                                .or(Some(0));
                            let herdr = state.agents.iter().any(|view| {
                                view.observation.id == agent_id
                                    && view.observation.runtime.provider == "Herdr"
                            });
                            let notice = if inventory.warnings.is_empty() {
                                if herdr {
                                    "Choose the terminal hosting Herdr; Enter or y saves it."
                                        .to_owned()
                                } else {
                                    "Choose a terminal; Enter or y links it to this agent."
                                        .to_owned()
                                }
                            } else {
                                format!(
                                    "Choose a terminal. Some adapters reported: {}",
                                    inventory.warnings.join(" · ")
                                )
                            };
                            state.set_notice(notice);
                            state.terminal_picker = Some(TerminalPicker {
                                agent_id,
                                selected,
                                terminals,
                            });
                        }
                    } else {
                        state.set_notice("Agent is no longer live; link cancelled.");
                    }
                }
                Err(error) => state.set_notice(format!("Could not list terminals: {error}")),
            }
        }

        if draw_requested {
            terminal.draw(|frame| render(frame, &state, unix_now()))?;
            draw_requested = false;
        }
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                draw_requested = true;
                if let Some(form) = state.launch_form.as_mut() {
                    match key.code {
                        KeyCode::Esc => {
                            state.launch_form = None;
                            state.set_notice("Launch cancelled.");
                        }
                        KeyCode::Tab | KeyCode::Down => form.field = (form.field + 1) % 3,
                        KeyCode::BackTab | KeyCode::Up => form.field = (form.field + 2) % 3,
                        KeyCode::Left => form.change(false),
                        KeyCode::Right => form.change(true),
                        KeyCode::Backspace if form.field == 0 => {
                            form.cwd.pop();
                        }
                        KeyCode::Char(character) if form.field == 0 => form.cwd.push(character),
                        KeyCode::Enter => {
                            let request = form.request();
                            state.launch_form = None;
                            launch_in_flight = true;
                            state.set_notice(format!(
                                "Launching {} in {}…",
                                request.kind,
                                request.destination.label()
                            ));
                            let reply = launch_tx.clone();
                            thread::spawn(move || {
                                let _ = reply.send(launch::launch(&request));
                            });
                        }
                        _ => {}
                    }
                    continue;
                }
                if let Some(form) = state.alias_form.as_mut() {
                    match key.code {
                        KeyCode::Esc => state.alias_form = None,
                        KeyCode::Backspace => {
                            form.text.pop();
                        }
                        KeyCode::Char(character) if !character.is_control() => {
                            if form.text.chars().count() < 32 {
                                form.text.push(character);
                            }
                        }
                        KeyCode::Enter => {
                            let form = state.alias_form.take().expect("alias form is open");
                            store.set_alias(&form.session_key, &form.text)?;
                            if form.text.trim().is_empty() {
                                state.aliases.remove(&form.session_key);
                                state.set_notice("Session label cleared.");
                            } else {
                                state
                                    .aliases
                                    .insert(form.session_key, form.text.trim().to_owned());
                                state.set_notice("Session label saved.");
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if let Some(picker) = state.terminal_picker.as_mut() {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc => {
                            state.terminal_picker = None;
                            state.set_notice("Terminal link cancelled.");
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            picker.selected = next_index(picker.selected, picker.terminals.len());
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            picker.selected =
                                previous_index(picker.selected, picker.terminals.len());
                        }
                        KeyCode::Enter | KeyCode::Char('y') => {
                            let picker = state.terminal_picker.take().expect("picker is open");
                            if let Some(terminal) = picker
                                .selected
                                .and_then(|index| picker.terminals.get(index))
                            {
                                if state
                                    .agents
                                    .iter()
                                    .any(|view| view.observation.id == picker.agent_id)
                                {
                                    let agent = state
                                        .agents
                                        .iter()
                                        .find(|view| view.observation.id == picker.agent_id)
                                        .expect("live agent was just checked");
                                    let herdr = agent.observation.runtime.provider == "Herdr";
                                    let key = if herdr {
                                        herdr_host_key(&agent.observation)
                                    } else {
                                        picker.agent_id.clone()
                                    };
                                    store.link_terminal(&key, &terminal.target())?;
                                    state.terminal_links.insert(key, terminal.target());
                                    state.set_notice(if herdr {
                                        format!(
                                            "Linked Herdr host to {} {}. Press o to open it.",
                                            terminal.provider.label(),
                                            terminal.id
                                        )
                                    } else {
                                        format!(
                                            "Linked {} {}. Press o to jump there.",
                                            terminal.provider.label(),
                                            terminal.id
                                        )
                                    });
                                } else {
                                    state.set_notice("Agent is no longer live; link cancelled.");
                                }
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if state.screen == Screen::Search {
                    match key.code {
                        KeyCode::Esc => {
                            state.back();
                            search_dirty = None;
                            search_generation = search_generation.wrapping_add(1);
                        }
                        KeyCode::Enter => {
                            if state.inspect_unified_result() {
                                start_inspection_preview(
                                    &mut state,
                                    &preview_tx,
                                    &mut preview_in_flight,
                                );
                            }
                        }
                        KeyCode::Backspace => {
                            state.unified_query.pop();
                            state.edit_unified_search();
                            search_generation = search_generation.wrapping_add(1);
                            search_dirty = Some(Instant::now());
                        }
                        KeyCode::Down => state.next(),
                        KeyCode::Up => state.previous(),
                        KeyCode::Char(character) => {
                            state.unified_query.push(character);
                            state.edit_unified_search();
                            search_generation = search_generation.wrapping_add(1);
                            search_dirty = Some(Instant::now());
                        }
                        _ => {}
                    }
                    continue;
                }
                if (state.search_query().is_some() && state.screen == Screen::Dashboard)
                    || (state.history_query().is_some() && state.screen == Screen::History)
                {
                    match key.code {
                        KeyCode::Esc => state.clear_search(),
                        KeyCode::Enter => {
                            state.inspect();
                            start_inspection_preview(
                                &mut state,
                                &preview_tx,
                                &mut preview_in_flight,
                            );
                        }
                        KeyCode::Backspace => state.search_backspace(),
                        KeyCode::Down => state.next(),
                        KeyCode::Up => state.previous(),
                        KeyCode::Char(character) => state.search_push(character),
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('a')
                        if matches!(state.screen, Screen::Dashboard | Screen::InspectAgent(_)) =>
                    {
                        if let Some(agent) = state.selected_live_agent() {
                            let session_key = alias_key(agent);
                            let text = state.aliases.get(&session_key).cloned().unwrap_or_default();
                            state.alias_form = Some(AliasForm { session_key, text });
                        }
                    }
                    KeyCode::Char('n')
                        if !launch_in_flight
                            && matches!(
                                state.screen,
                                Screen::Dashboard
                                    | Screen::InspectAgent(_)
                                    | Screen::Projects
                                    | Screen::InspectProject(_)
                            ) =>
                    {
                        state.open_launch_form()
                    }
                    KeyCode::Char('/') => {
                        search_generation = search_generation.wrapping_add(1);
                        state.open_unified_search();
                    }
                    KeyCode::Char('h')
                        if matches!(state.screen, Screen::Dashboard | Screen::InspectAgent(_)) =>
                    {
                        let exact_agent = if matches!(state.screen, Screen::InspectAgent(_)) {
                            state.selected_live_agent().map(|agent| agent.id.clone())
                        } else {
                            None
                        };
                        if state.history_loaded {
                            if exact_agent.is_some() && !state.open_exact_history_for_agent() {
                                state.set_notice("No exact saved session matches this live agent.");
                            } else if exact_agent.is_none() {
                                state.open_history();
                            }
                        } else {
                            state.open_history();
                            state.history_return_agent = exact_agent;
                            if !state.history_loading {
                                start_history_refresh(&mut state, &history_tx, &search_tx);
                            }
                        }
                    }
                    KeyCode::Char('p')
                        if matches!(state.screen, Screen::Dashboard | Screen::InspectAgent(_)) =>
                    {
                        state.open_projects();
                        if !state.projects_loading {
                            start_project_refresh(&mut state, &projects_tx);
                        }
                    }
                    KeyCode::Esc if link_in_flight => {
                        link_generation = link_generation.wrapping_add(1);
                        link_in_flight = false;
                        state.set_notice("Terminal listing cancelled.");
                    }
                    KeyCode::Esc => state.back(),
                    KeyCode::Enter => {
                        state.inspect();
                        start_inspection_preview(&mut state, &preview_tx, &mut preview_in_flight);
                    }
                    KeyCode::Char('r') => {
                        if matches!(state.screen, Screen::History | Screen::InspectHistory(_, _)) {
                            if !state.history_loading {
                                start_history_refresh(&mut state, &history_tx, &search_tx);
                            }
                        } else if matches!(
                            state.screen,
                            Screen::Projects | Screen::InspectProject(_)
                        ) {
                            if !state.projects_loading {
                                start_project_refresh(&mut state, &projects_tx);
                            }
                        } else {
                            refresh_now = true;
                            if !state.history_loading {
                                start_history_refresh(&mut state, &history_tx, &search_tx);
                            }
                            start_inspection_preview(
                                &mut state,
                                &preview_tx,
                                &mut preview_in_flight,
                            );
                        }
                    }
                    KeyCode::Char('o') if !focus_in_flight => {
                        if let Some(pane) = state.focus_target().map(str::to_owned) {
                            let inside_herdr =
                                std::env::var("HERDR_ENV").ok().as_deref() == Some("1");
                            let host = if inside_herdr {
                                None
                            } else {
                                let agent =
                                    state.selected_live_agent().expect("pane target is live");
                                store.terminal_link(&herdr_host_key(agent))?
                            };
                            if !inside_herdr && host.is_none() {
                                state.set_notice("Press L to link the terminal window hosting Herdr, then o to open this pane.");
                                continue;
                            }
                            focus_in_flight = true;
                            state.set_notice(format!("Opening Herdr pane {pane}…"));
                            let reply = focus_tx.clone();
                            thread::spawn(move || {
                                let result = focus_agent_pane(&pane)
                                    .map_err(|error| error.to_string())
                                    .and_then(|()| {
                                        if let Some(host) = host {
                                            focus_terminal_target(&host)?;
                                        }
                                        Ok(format!("Herdr pane {pane}"))
                                    });
                                let _ = reply.send(result);
                            });
                        } else if let Some(agent) = state.selected_live_agent().cloned() {
                            let saved = store.terminal_link(&agent.id)?;
                            focus_in_flight = true;
                            state.set_notice("Locating agent terminal…");
                            let reply = focus_tx.clone();
                            thread::spawn(move || {
                                let _ = reply.send(focus_agent_terminal(&agent, saved.as_deref()));
                            });
                        } else {
                            state.set_notice(
                                "No live navigation target is available for this selection.",
                            );
                        }
                    }
                    KeyCode::Char('L') if !link_in_flight => {
                        if let Some(agent) = state.selected_live_agent() {
                            if agent.runtime.provider == "macOS"
                                || agent.runtime.provider == "Herdr"
                            {
                                let agent_id = agent.id.clone();
                                let reply = link_tx.clone();
                                link_in_flight = true;
                                link_generation = link_generation.wrapping_add(1);
                                let generation = link_generation;
                                state.set_notice(if agent.runtime.provider == "Herdr" {
                                    "Loading terminal windows; choose the one hosting Herdr…"
                                } else {
                                    "Loading Ghostty, Terminal.app, and iTerm2 sessions…"
                                });
                                thread::spawn(move || {
                                    let result = list_terminals();
                                    let _ = reply.send((generation, agent_id, result));
                                });
                            }
                        } else {
                            state.set_notice("Select a live agent to link.");
                        }
                    }
                    KeyCode::Char('y') if link_in_flight => {
                        state.set_notice(
                            "Still loading terminal sessions. Choose one when the list appears.",
                        );
                    }
                    KeyCode::Down | KeyCode::Char('j') => state.next(),
                    KeyCode::Up | KeyCode::Char('k') => state.previous(),
                    _ => {}
                }
            }
        }
    }
}

fn start_history_refresh(
    state: &mut DashboardState,
    reply: &mpsc::Sender<Result<HistoryInventory, String>>,
    search: &mpsc::Sender<SearchCommand>,
) {
    state.history_loading = true;
    let reply = reply.clone();
    let search = search.clone();
    thread::spawn(move || {
        let result = scan_history();
        if let Ok(history) = &result {
            let _ = search.send(SearchCommand::History(history.clone()));
        }
        let _ = reply.send(result);
    });
}

fn start_project_refresh(state: &mut DashboardState, reply: &mpsc::Sender<Vec<ProjectSnapshot>>) {
    state.projects_loading = true;
    let agents = state.agents.clone();
    let reply = reply.clone();
    thread::spawn(move || {
        let _ = reply.send(inspect_projects(&agents));
    });
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn agent_tty(agent: &AgentObservation) -> Option<&str> {
    agent
        .runtime
        .fields
        .iter()
        .find(|(name, _)| name == "TTY")
        .map(|(_, tty)| tty.as_str())
}

fn herdr_host_key(agent: &AgentObservation) -> String {
    let workspace = agent
        .runtime
        .fields
        .iter()
        .find(|(name, _)| name == "Workspace")
        .map(|(_, value)| value.as_str())
        .unwrap_or(&agent.runtime.target);
    format!("herdr-host:{workspace}")
}

fn focus_agent_terminal(agent: &AgentObservation, saved: Option<&str>) -> Result<String, String> {
    if let Some(target) = saved {
        return focus_terminal_target(target);
    }
    let tty = agent_tty(agent).ok_or("No terminal TTY for this agent. Press L to link it.")?;
    let inventory = list_native()?;
    let matches = matching_tty(&inventory.candidates, tty);
    match matches.as_slice() {
        [candidate] => focus_terminal_target(&candidate.target()),
        [] => Err(
            "No Terminal.app or iTerm2 session matches this TTY. Press L to link another terminal."
                .into(),
        ),
        _ => Err("Multiple terminal sessions match this TTY. Press L to choose one.".into()),
    }
}

fn read_agent_visible_output(agent: &AgentObservation) -> Result<(String, String), String> {
    let tty = agent_tty(agent).ok_or("No terminal TTY is available for this agent")?;
    let candidates = list_native()?.candidates;
    let matches = matching_tty(&candidates, tty);
    let candidate = match matches.as_slice() {
        [candidate] => candidate,
        [] => return Err("No Terminal.app or iTerm2 session matches this TTY. Ghostty does not expose visible text through its scripting API.".into()),
        _ => return Err("Multiple terminal sessions match this TTY; visible output cannot be attributed safely.".into()),
    };
    Ok((
        candidate.provider.label().into(),
        read_visible(&candidate.target())?,
    ))
}

fn start_inspection_preview(
    state: &mut DashboardState,
    sender: &mpsc::Sender<PreviewResult>,
    in_flight: &mut Option<String>,
) {
    if !matches!(state.screen, Screen::InspectAgent(_)) {
        return;
    }
    let Some(agent) = state.selected_live_agent().cloned() else {
        return;
    };
    if in_flight.as_deref() == Some(agent.id.as_str()) {
        return;
    }
    state.inspection_preview = None;
    *in_flight = Some(agent.id.clone());
    let reply = sender.clone();
    thread::spawn(move || {
        let result = if agent.runtime.provider == "Herdr" {
            read_recent_output(&agent.runtime.target)
                .map(|text| ("Herdr pane".into(), text))
                .map_err(|error| error.to_string())
        } else {
            read_agent_visible_output(&agent)
        };
        let _ = reply.send((agent.id, result));
    });
}

fn elapsed_label(now: i64, then: Option<i64>) -> String {
    let Some(then) = then else {
        return "—".into();
    };
    let seconds = now.saturating_sub(then);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn metadata<'a>(agent: &'a AgentObservation, key: &str) -> Option<&'a str> {
    agent
        .runtime
        .fields
        .iter()
        .find_map(|(field, value)| (field == key && !value.is_empty()).then_some(value.as_str()))
}

fn alias_key(agent: &AgentObservation) -> String {
    if let Some(session) = metadata(agent, "Session ID") {
        format!("{}:session:{session}", agent.kind)
    } else {
        format!("{}:live:{}", agent.kind, agent.id)
    }
}

fn fallback_session_label(agent: &AgentObservation) -> String {
    if let Some(session) = metadata(agent, "Session ID") {
        format!("#{}", session.chars().take(8).collect::<String>())
    } else if agent.runtime.provider == "Herdr" {
        agent.runtime.target.clone()
    } else {
        format!("PID {}", agent.pid)
    }
}

fn session_sort_key(agent: &AgentObservation) -> String {
    metadata(agent, "Session ID")
        .map(str::to_owned)
        .unwrap_or_else(|| agent.id.clone())
}

fn location_label(agent: &AgentObservation, saved_link: Option<&str>) -> String {
    if agent.runtime.provider == "Herdr" {
        let host = saved_link
            .and_then(|link| parse_target(link).ok())
            .map(|target| format!("Herdr/{}", target.provider.label()))
            .unwrap_or_else(|| "Herdr".into());
        let tab = metadata(agent, "Tab");
        let workspace = metadata(agent, "Workspace")
            .and_then(|id| herdr_number(id, 'w'))
            .or_else(|| {
                tab.and_then(|id| id.split(':').next())
                    .and_then(|id| herdr_number(id, 'w'))
            })
            .map(|number| format!("Workspace {number}"))
            .or_else(|| metadata(agent, "Workspace").map(str::to_owned));
        let tab_label = tab
            .and_then(|id| id.rsplit(':').next())
            .and_then(|id| herdr_number(id, 't'))
            .map(|number| format!("Tab {number}"))
            .or_else(|| tab.map(str::to_owned));
        let pane_label = agent
            .runtime
            .target
            .rsplit(':')
            .next()
            .and_then(|id| herdr_number(id, 'p'))
            .map(|number| format!("Pane {number}"))
            .unwrap_or_else(|| agent.runtime.target.clone());
        let parts = workspace
            .into_iter()
            .chain(tab_label)
            .chain(Some(pane_label))
            .collect::<Vec<_>>()
            .join(" / ");
        return format!("{host} · {parts}");
    }
    let tty = metadata(agent, "TTY");
    if let Some(provider) = saved_link.and_then(|link| parse_target(link).ok()) {
        return match tty {
            Some(tty) => format!("{} · {tty}", provider.provider.label()),
            None => format!("{} · linked", provider.provider.label()),
        };
    }
    match tty {
        Some(tty) => format!("{tty} · app unknown"),
        None => "Local · app unknown".into(),
    }
}

fn compact_location_label(agent: &AgentObservation, saved_link: Option<&str>) -> String {
    let provider = saved_link
        .and_then(|link| parse_target(link).ok())
        .map(|target| match target.provider {
            Provider::Ghostty => "Ghostty",
            Provider::Terminal => "Terminal",
            Provider::ITerm2 => "iTerm2",
        });
    if agent.runtime.provider == "Herdr" {
        return match provider {
            Some(provider) => provider.into(),
            None => "Herdr".into(),
        };
    }
    let tty = metadata(agent, "TTY").map(|tty| tty.strip_prefix("ttys").unwrap_or(tty));
    match (provider, tty) {
        (Some(provider), Some(tty)) => format!("{provider}:{tty}"),
        (Some(provider), None) => provider.into(),
        (None, Some(tty)) => format!("TTY:{tty}"),
        (None, None) => "App unknown".into(),
    }
}

fn herdr_number(id: &str, prefix: char) -> Option<&str> {
    id.strip_prefix(prefix)
        .filter(|number| !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit()))
}

fn render(frame: &mut Frame, state: &DashboardState, now: i64) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(frame.area());

    let mut title = format!(
        "AM2  {} live agent{}",
        state.agents.len(),
        if state.agents.len() == 1 { "" } else { "s" },
    );
    for (status, label) in [
        (AgentState::Working, "working"),
        (AgentState::Idle, "idle"),
        (AgentState::Waiting, "needs input"),
        (AgentState::Finished, "finished"),
        (AgentState::Failed, "failed"),
        (AgentState::Unknown, "unknown"),
    ] {
        let count = state
            .agents
            .iter()
            .filter(|view| view.observation.state == status)
            .count();
        if count > 0 {
            title.push_str(&format!(" · {count} {label}"));
        }
    }
    if state.is_stale() {
        title.push_str(if state.agents.is_empty() {
            "  · inventory unavailable"
        } else {
            "  · STALE"
        });
    }
    frame.render_widget(
        Paragraph::new(title)
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Magenta)),
            ),
        areas[0],
    );

    if let Some(form) = &state.alias_form {
        frame.render_widget(
            Paragraph::new(format!(
                "Name this live session (up to 32 characters). Empty clears the name.\n\nLabel  {}_\n\nThe name follows an exact session ID when available.",
                form.text
            ))
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Green)).title("Session label")),
            areas[1],
        );
    } else if let Some(form) = &state.launch_form {
        render_launch_form(frame, form, areas[1]);
    } else if let Some(picker) = &state.terminal_picker {
        render_terminal_picker(frame, picker, areas[1]);
    } else {
        match state.screen {
            Screen::Dashboard => render_dashboard(frame, state, now, areas[1]),
            Screen::Search => render_unified_search(frame, state, now, areas[1]),
            Screen::InspectAgent(_) => {
                if areas[1].width >= 100 {
                    let columns = Layout::horizontal([
                        Constraint::Percentage(40),
                        Constraint::Percentage(60),
                    ])
                    .split(areas[1]);
                    render_compact_agents(frame, state, now, columns[0]);
                    render_inspection(frame, state, now, columns[1]);
                } else {
                    render_inspection(frame, state, now, areas[1]);
                }
            }
            Screen::History => render_history(frame, state, areas[1]),
            Screen::InspectHistory(_, _) => render_history_inspection(frame, state, areas[1]),
            Screen::Projects => render_projects(frame, state, areas[1]),
            Screen::InspectProject(_) => render_project_inspection(frame, state, now, areas[1]),
        }
    }

    frame.render_widget(
        Paragraph::new(state.notice.as_deref().unwrap_or(""))
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: true }),
        areas[2],
    );

    let footer: &str = if state.alias_form.is_some() {
        "type label  Enter save  Backspace clear  Esc cancel"
    } else if state.launch_form.is_some() {
        "type project path  Tab field  ←→ choose  Enter launch  Esc cancel"
    } else if state.terminal_picker.is_some() {
        "↑↓/jk choose terminal  Enter/y link  Esc cancel  q quit"
    } else if (state.search_query().is_some() && state.screen == Screen::Dashboard)
        || (state.history_query().is_some() && state.screen == Screen::History)
    {
        "type to search  ↑↓ select  Enter inspect  Esc clear"
    } else {
        match state.screen {
            Screen::Search => "type query  ↑↓ select  Enter inspect  Esc back  project:… agent:…",
            Screen::Dashboard => {
                "↑↓ view  a label  / search  p proj  n new  o open  L link  r refresh  q quit"
            }
            Screen::InspectAgent(_) => {
                "↑↓ scroll  a label  p project  h history  n new  o jump  L link  Esc back  q quit"
            }
            Screen::History => "↑↓/jk select  Enter inspect  / search  r reload  Esc live  q quit",
            Screen::InspectHistory(_, _) => "↑↓/jk scroll  Esc results  r reload  q quit",
            Screen::Projects => "↑↓/jk select  Enter inspect  n new  r refresh  Esc agents  q quit",
            Screen::InspectProject(_) => "↑↓/jk scroll  n new  Esc projects  r refresh  q quit",
        }
    };
    let footer = if state.is_stale() {
        footer.replace("r refresh", "r retry")
    } else {
        footer.to_owned()
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::Gray)),
        areas[3],
    );
}

fn agent_icon(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Codex => "◆",
        AgentKind::Claude => "✦",
        AgentKind::OpenCode => "▣",
        AgentKind::Pi => "π",
        AgentKind::Gemini => "◇",
        AgentKind::Cursor => "▸",
    }
}

fn agent_color(kind: AgentKind) -> Color {
    match kind {
        AgentKind::Codex => Color::Cyan,
        AgentKind::Claude => Color::Magenta,
        AgentKind::OpenCode => Color::Blue,
        AgentKind::Pi => Color::Yellow,
        AgentKind::Gemini => Color::LightBlue,
        AgentKind::Cursor => Color::Green,
    }
}

fn render_unified_search(
    frame: &mut Frame,
    state: &DashboardState,
    now: i64,
    area: ratatui::layout::Rect,
) {
    let sections = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(5),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(format!("/ {}▏", state.unified_query)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title("Search · live agents + saved conversations"),
        ),
        sections[0],
    );
    if state.unified_query.trim().is_empty() {
        frame.render_widget(
            Paragraph::new("Type words to search current agents and saved conversation events.\nAdd project:am2 or agent:codex to narrow results. Multiple words must all match.")
                .block(Block::default().borders(Borders::ALL).title("Results"))
                .wrap(Wrap { trim: false }),
            sections[1],
        );
    } else if let Some(error) = &state.unified_error {
        frame.render_widget(
            Paragraph::new(format!("Search unavailable: {error}"))
                .block(Block::default().borders(Borders::ALL).title("Results")),
            sections[1],
        );
    } else {
        let rows = state.unified_results.iter().map(|hit| {
            let age = if matches!(hit.target, SearchTarget::Saved { .. }) {
                format!("file {}", elapsed_label(now, hit.observed_at))
            } else {
                elapsed_label(now, hit.observed_at)
            };
            ListItem::new(format!(
                "{} {} / {} · {} · {}\n  {}",
                agent_icon(hit.kind),
                hit.kind,
                hit.project,
                hit.source,
                age,
                short_text(&hit.snippet, 140)
            ))
        });
        let title = if state.unified_loading {
            "Results · searching…".into()
        } else {
            format!("Results · {} matches", state.unified_results.len())
        };
        let list = List::new(rows)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(title),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );
        let mut list_state = ListState::default();
        list_state.select(state.unified_selected);
        frame.render_stateful_widget(list, sections[1], &mut list_state);
    }
    let selected = state
        .unified_selected
        .and_then(|index| state.unified_results.get(index));
    let detail = selected.map_or_else(
        || {
            if !state.unified_loading
                && !state.unified_query.trim().is_empty()
                && state.unified_results.is_empty()
            {
                "No matches. Try fewer words or remove a project:/agent: filter.".to_owned()
            } else {
                "Select a result to open its exact live agent or saved event.".to_owned()
            }
        },
        |hit| {
            format!(
                "{} · {} · {}\n{}",
                hit.kind,
                hit.project,
                hit.source,
                short_text(&hit.snippet, 220)
            )
        },
    );
    frame.render_widget(
        Paragraph::new(detail).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title("Selected match"),
        ),
        sections[2],
    );
}

fn render_launch_form(frame: &mut Frame, form: &LaunchForm, area: ratatui::layout::Rect) {
    let marker = |field| if form.field == field { "▸" } else { " " };
    let destination = if form.destination == Destination::Herdr {
        format!(
            "Herdr · beside {}",
            form.herdr_anchor.as_deref().unwrap_or("unknown pane")
        )
    } else {
        form.destination.label().to_owned()
    };
    let lines = format!(
        "Start an interactive agent in a new terminal surface.\n\n{} Project   {}\n{} Agent     {}  ←→\n{} Terminal  {}  ←→\n\nAM2 will save the exact terminal target after the agent appears.",
        marker(0), form.cwd, marker(1), form.kind, marker(2), destination
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Green))
                    .title("New agent"),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_terminal_picker(frame: &mut Frame, picker: &TerminalPicker, area: ratatui::layout::Rect) {
    let window_count = picker
        .terminals
        .iter()
        .map(|terminal| (terminal.instance_pid, terminal.window_id.as_str()))
        .collect::<HashSet<_>>()
        .len();
    let rows = picker.terminals.iter().map(|terminal| {
        let title = if terminal.title.is_empty() {
            "Untitled terminal"
        } else {
            &terminal.title
        };
        let location = terminal
            .cwd
            .as_deref()
            .or(terminal.tty.as_deref())
            .unwrap_or("cwd and TTY unavailable");
        let window = if terminal.window_title.is_empty() {
            format!("Window {}", terminal.window_id)
        } else {
            format!("{} · {}", terminal.window_title, terminal.window_id)
        };
        ListItem::new(format!(
            "{} · {title} · {}\n  {location} · {window} · PID {}",
            terminal.provider.label(),
            terminal.id,
            terminal.instance_pid
        ))
    });
    let list = List::new(rows)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(format!(
                    "Choose terminal · {} sessions / {} windows",
                    picker.terminals.len(),
                    window_count
                )),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut selection = ListState::default();
    selection.select(picker.selected);
    frame.render_stateful_widget(list, area, &mut selection);
}

fn state_symbol(state: AgentState) -> &'static str {
    match state {
        AgentState::Working => "●",
        AgentState::Idle => "○",
        AgentState::Waiting => "◉",
        AgentState::Finished => "✓",
        AgentState::Failed => "✕",
        AgentState::Unknown => "?",
    }
}

fn state_cell_label(state: AgentState) -> String {
    if state == AgentState::Waiting {
        state.compact_display_label().into()
    } else {
        format!("{} {}", state_symbol(state), state.display_label())
    }
}

fn render_compact_agents(
    frame: &mut Frame,
    state: &DashboardState,
    now: i64,
    area: ratatui::layout::Rect,
) {
    let rows = state.visible_agents().into_iter().map(|view| {
        let agent = &view.observation;
        let evidence = derive_activity(view, &state.history);
        ListItem::new(format!(
            "{} {} · {} / {}\n  {} · {} · {}",
            agent_icon(agent.kind),
            agent.kind,
            state.session_label(agent),
            agent.project,
            state_cell_label(agent.state),
            evidence.summary,
            elapsed_label(now, evidence.observed_at),
        ))
        .style(state_style(agent.state))
    });
    let list = List::new(rows)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title("Agents"),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut list_state = ListState::default();
    list_state.select(state.agent_selected.and_then(|selected| {
        state
            .visible_indices()
            .iter()
            .position(|index| *index == selected)
    }));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_dashboard(
    frame: &mut Frame,
    state: &DashboardState,
    now: i64,
    area: ratatui::layout::Rect,
) {
    let sections = Layout::vertical([Constraint::Min(6), Constraint::Length(7)]).split(area);
    let wide = area.width >= 132;
    let rows = state.visible_agents().into_iter().map(|view| {
        let observation = &view.observation;
        let evidence = derive_activity(view, &state.history);
        let agent_line = Line::from(vec![
            Span::styled(
                agent_icon(observation.kind),
                Style::default().fg(agent_color(observation.kind)),
            ),
            Span::raw(format!(
                " {} · {}",
                observation.kind,
                state.session_label(observation)
            )),
        ]);
        let mut cells = vec![
            Cell::from(agent_line),
            Cell::from(observation.project.clone()),
            Cell::from(state_cell_label(observation.state)).style(state_style(observation.state)),
            Cell::from(evidence.summary),
        ];
        if wide {
            cells.push(Cell::from(state.location_label(observation)));
            cells.push(Cell::from(elapsed_label(now, evidence.observed_at)));
        } else {
            cells.push(Cell::from(state.compact_location_label(observation)));
        }
        Row::new(cells)
    });
    let list_title = state.search_query().map_or_else(
        || "Agents".to_owned(),
        |query| {
            format!(
                "Agents  / {query}  ·  {} matches",
                state.visible_agents().len()
            )
        },
    );
    let widths = if wide {
        vec![
            Constraint::Length(24),
            Constraint::Length(16),
            Constraint::Length(13),
            Constraint::Min(16),
            Constraint::Length(48),
            Constraint::Length(5),
        ]
    } else if area.width >= 100 {
        vec![
            Constraint::Length(21),
            Constraint::Length(11),
            Constraint::Length(13),
            Constraint::Length(21),
            Constraint::Min(15),
        ]
    } else {
        vec![
            Constraint::Length(21),
            Constraint::Length(11),
            Constraint::Length(13),
            Constraint::Min(13),
            Constraint::Length(15),
        ]
    };
    let headers = if wide {
        vec![
            "AGENT · SESSION",
            "PROJECT",
            "STATE",
            "ACTIVITY",
            "LOCATION",
            "AGE",
        ]
    } else {
        vec![
            "AGENT · SESSION",
            "PROJECT",
            "STATE",
            "ACTIVITY",
            "LOCATION",
        ]
    };
    let table = Table::new(rows, widths)
        .header(
            Row::new(headers).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(list_title),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut table_state = TableState::default();
    table_state.select(state.agent_selected.and_then(|selected| {
        state
            .visible_indices()
            .iter()
            .position(|index| *index == selected)
    }));
    frame.render_stateful_widget(table, sections[0], &mut table_state);

    let selected = state.selected_agent().map_or_else(
        || {
            if let Some(error) = &state.error {
                format!("Inventory unavailable: {error}")
            } else if state.search_query().is_some() {
                "No agents match this search. Press Esc to clear it.".into()
            } else {
                "No running agents found. AM2 checks Herdr and local Codex, Claude, OpenCode, Pi, Gemini, and Cursor processes."
                    .into()
            }
        },
        |view| {
            let agent = &view.observation;
            let evidence = derive_activity(view, &state.history);
            format!(
                "{} {} · {} · {}\n{} {} · {}\n{} · {} · {}\n{} · PID {}",
                agent_icon(agent.kind),
                agent.kind,
                agent.project,
                state.session_label(agent),
                state_symbol(agent.state),
                agent.state.display_label(),
                evidence.summary,
                evidence.activity_source,
                elapsed_label(now, evidence.observed_at),
                if evidence.recorded { "recorded" } else { "observed" },
                state.location_label(agent),
                agent.pid
            )
        },
    );
    frame.render_widget(
        Paragraph::new(selected).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title("Selected agent"),
        ),
        sections[1],
    );
}

fn render_inspection(
    frame: &mut Frame,
    state: &DashboardState,
    now: i64,
    area: ratatui::layout::Rect,
) {
    let view = match &state.screen {
        Screen::InspectAgent(id) => state.agents.iter().find(|view| &view.observation.id == id),
        _ => None,
    };
    let mut metadata = Vec::new();
    let (runtime, output, output_title) = if let Some(view) = view {
        let agent = &view.observation;
        let evidence = derive_activity(view, &state.history);
        metadata.push(format!(
            "{} · {} · {}",
            agent.kind,
            agent.project,
            state.session_label(agent)
        ));
        metadata.push(format!(
            "State       {} · {}",
            agent.state.display_label(),
            evidence.state_source
        ));
        metadata.push(format!("Activity    {}", evidence.summary));
        metadata.push(format!(
            "From        {} · {} · {}",
            evidence.activity_source,
            elapsed_label(now, evidence.observed_at),
            if evidence.recorded {
                "recorded write (file mtime)"
            } else if evidence.activity_source == "Herdr pane text" {
                "pane change observed by AM2"
            } else {
                "no timestamp"
            }
        ));
        metadata.push(format!("PID         {}", agent.pid));
        metadata.push(format!("CWD         {}", agent.cwd));
        if let Some(preview) = state
            .inspection_preview
            .as_ref()
            .filter(|preview| preview.agent_id == agent.id)
        {
            (
                &agent.runtime,
                preview.text.as_str(),
                if agent.runtime.provider == "Herdr" {
                    "Recent pane output · at most 80 lines".to_owned()
                } else {
                    format!(
                        "Visible terminal text · {} · at most 80 lines",
                        preview.source
                    )
                },
            )
        } else {
            (
                &agent.runtime,
                if agent.runtime.provider == "Herdr" {
                    "Reading recent pane output…"
                } else {
                    "Reading visible terminal text…"
                },
                "Terminal preview".to_owned(),
            )
        }
    } else {
        frame.render_widget(
            Paragraph::new("Agent is no longer available. Press Esc to return.")
                .block(Block::default().borders(Borders::ALL).title("Inspect")),
            area,
        );
        return;
    };
    if let Some(view) = view {
        metadata.push(format!(
            "Runtime     {} · {} · {}",
            runtime.provider,
            runtime.target,
            state.location_label(&view.observation)
        ));
    }
    for (key, value) in &runtime.fields {
        metadata.push(format!("{key:<11} {value}"));
    }
    metadata.push(output_title);
    metadata.push(if output.is_empty() {
        "No recent output available.".into()
    } else {
        output.to_owned()
    });
    frame.render_widget(
        Paragraph::new(metadata.join("\n"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Magenta))
                    .title("Inspect agent"),
            )
            .scroll((state.detail_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn verified_live_session(state: &DashboardState, kind: AgentKind, id: &str) -> bool {
    state.agents.iter().any(|view| {
        let agent = &view.observation;
        agent.kind == kind
            && agent
                .runtime
                .fields
                .iter()
                .any(|(key, value)| key == "Session ID" && value == id)
    })
}

fn short_text(text: &str, limit: usize) -> String {
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut result = flattened.chars().take(limit).collect::<String>();
    if flattened.chars().count() > limit {
        result.push('…');
    }
    result
}

fn render_history(frame: &mut Frame, state: &DashboardState, area: ratatui::layout::Rect) {
    if state.history_loading && !state.history_loaded {
        frame.render_widget(
            Paragraph::new("Loading saved sessions…").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("History · saved sessions"),
            ),
            area,
        );
        return;
    }
    if let Some(error) = &state.history_error {
        frame.render_widget(
            Paragraph::new(format!("History unavailable: {error}\nPress r to retry."))
                .block(Block::default().borders(Borders::ALL).title("History")),
            area,
        );
        return;
    }
    let sections = Layout::vertical([Constraint::Min(8), Constraint::Length(6)]).split(area);
    let hits = state.visible_history_hits();
    let rows = hits.iter().filter_map(|hit| {
        let session = state.history.sessions.get(hit.session_index)?;
        let live = if verified_live_session(state, session.kind, &session.id) {
            " · linked live"
        } else {
            " · saved"
        };
        Some(ListItem::new(format!(
            "{} {} / {}{live}{}\n  {}",
            agent_icon(session.kind),
            session.kind,
            session.project,
            if hit.match_count > 0 {
                format!(" · {} matches", hit.match_count)
            } else {
                String::new()
            },
            short_text(&hit.snippet, 125)
        )))
    });
    let title = if let Some(query) = &state.history_query {
        format!("History · / {query} · {} sessions", hits.len())
    } else {
        format!("History · {} saved sessions", state.history.sessions.len())
    };
    let list = List::new(rows)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut list_state = ListState::default();
    list_state.select(state.history_selected);
    frame.render_stateful_widget(list, sections[0], &mut list_state);

    let selected = state
        .history_selected
        .and_then(|index| hits.get(index))
        .and_then(|hit| {
            state
                .history
                .sessions
                .get(hit.session_index)
                .map(|session| (hit, session))
        });
    let detail = if let Some((hit, session)) = selected {
        format!(
            "{} · {}\nSession ID  {}\nProject     {}\nMatch       {}",
            session.kind,
            session.project,
            session.id,
            session.cwd,
            if hit.event_index.is_some() {
                "conversation or tool event"
            } else {
                "session metadata"
            }
        )
    } else if state.history.sessions.is_empty() {
        "No saved Codex, Claude, or Pi sessions found in default locations.".into()
    } else {
        "No saved sessions match. Press Esc to clear the search.".into()
    };
    frame.render_widget(
        Paragraph::new(detail).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title(format!(
                    "Selected result · {} files skipped",
                    state.history.skipped_files
                )),
        ),
        sections[1],
    );
}

fn render_history_inspection(
    frame: &mut Frame,
    state: &DashboardState,
    area: ratatui::layout::Rect,
) {
    let Screen::InspectHistory(index, matched_event) = state.screen else {
        return;
    };
    let Some(session) = state.history.sessions.get(index) else {
        frame.render_widget(
            Paragraph::new("Saved session is no longer available. Press Esc.")
                .block(Block::default().borders(Borders::ALL).title("History")),
            area,
        );
        return;
    };
    let mut lines = vec![
        format!("{} · {} · saved session", session.kind, session.project),
        format!("Session ID  {}", session.id),
        format!("Project     {}", session.cwd),
        format!(
            "Updated     {}",
            elapsed_label(unix_now(), Some(session.updated_at))
        ),
        String::new(),
    ];
    if let Some(event) = matched_event.and_then(|event| session.events.get(event)) {
        lines.push(format!("SEARCH MATCH · {}", event.kind.label()));
        lines.push(event.text.clone());
        lines.push(String::new());
    }
    lines.push(format!(
        "RECENT TIMELINE · last {} of {} events",
        session.events.len().min(40),
        session.events.len()
    ));
    for event in session.events.iter().rev().take(40).rev() {
        let time = event
            .timestamp
            .as_deref()
            .and_then(|value| value.get(11..16))
            .unwrap_or("--:--");
        lines.push(format!(
            "{time}  {}  {}",
            event.kind.label(),
            short_text(&event.text, 320)
        ));
    }
    if session.events.is_empty() {
        lines.push("No supported conversation events in this saved session.".into());
    }
    frame.render_widget(
        Paragraph::new(lines.join("\n"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Magenta))
                    .title("Saved conversation · read only"),
            )
            .scroll((state.detail_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_projects(frame: &mut Frame, state: &DashboardState, area: ratatui::layout::Rect) {
    if state.projects_loading && !state.projects_loaded {
        frame.render_widget(
            Paragraph::new("Reading local Git status…")
                .block(Block::default().borders(Borders::ALL).title("Projects")),
            area,
        );
        return;
    }
    if let Some(error) = &state.projects_error {
        frame.render_widget(
            Paragraph::new(format!("Projects unavailable: {error}"))
                .block(Block::default().borders(Borders::ALL).title("Projects")),
            area,
        );
        return;
    }
    let sections = Layout::vertical([Constraint::Min(8), Constraint::Length(6)]).split(area);
    let rows = state.projects.iter().map(|project| {
        let git = if project.git {
            project.branch.as_deref().unwrap_or("detached")
        } else {
            "no Git repo"
        };
        let tests = project
            .test_evidence
            .as_ref()
            .map_or("no test result observed", |evidence| {
                evidence.result.label()
            });
        ListItem::new(format!(
            "▦ {} · {} · {} agent{} · {} changed\n  {} · {}",
            project.name,
            git,
            project.agents.len(),
            if project.agents.len() == 1 { "" } else { "s" },
            project.change_count,
            tests,
            short_text(&project.next_step, 90)
        ))
    });
    let list = List::new(rows)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(format!("Projects · {} live", state.projects.len())),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut list_state = ListState::default();
    list_state.select(state.project_selected);
    frame.render_stateful_widget(list, sections[0], &mut list_state);
    let selected = state.selected_project().map_or_else(
        || "No live agent projects found. Return to Agents with Esc.".into(),
        |project| {
            format!(
                "{}\nBranch     {}\nNext       {}\nEnter for changed files and agent context",
                project.root,
                project.branch.as_deref().unwrap_or("Git unavailable"),
                project.next_step,
            )
        },
    );
    frame.render_widget(
        Paragraph::new(selected).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title("Selected project"),
        ),
        sections[1],
    );
}

fn render_project_inspection(
    frame: &mut Frame,
    state: &DashboardState,
    now: i64,
    area: ratatui::layout::Rect,
) {
    let Screen::InspectProject(ref root) = state.screen else {
        return;
    };
    let Some(project) = state.projects.iter().find(|project| &project.root == root) else {
        frame.render_widget(
            Paragraph::new("Project is no longer in the live inventory. Press Esc.")
                .block(Block::default().borders(Borders::ALL).title("Project")),
            area,
        );
        return;
    };
    let mut lines = vec![
        format!("▦ {}", project.name),
        format!("Root         {}", project.root),
        format!(
            "Branch       {}",
            project.branch.as_deref().unwrap_or("Git unavailable")
        ),
        format!(
            "Last commit  {}",
            project
                .latest_commit
                .as_deref()
                .unwrap_or("No commit found")
        ),
        String::new(),
        format!("NEXT · {}", project.next_step),
        String::new(),
        format!("LIVE AGENTS · {}", project.agents.len()),
    ];
    for agent in &project.agents {
        lines.push(format!(
            "{} {} · {} · {}",
            agent_icon(agent.kind),
            agent.kind,
            agent.state.display_label(),
            agent.activity
        ));
    }
    lines.push(String::new());
    lines.push(format!("CHANGED FILES · {}", project.change_count));
    if project.git && project.change_count == 0 && project.error.is_none() {
        lines.push("Working tree clean".into());
    } else if !project.git {
        lines.push("Git status unavailable for this directory".into());
    }
    for file in &project.changes {
        lines.push(format!("{}  {}", file.status, file.path));
    }
    if project.change_count > project.changes.len() {
        lines.push(format!(
            "… {} more changed files",
            project.change_count - project.changes.len()
        ));
    }
    if let Some(error) = &project.error {
        lines.push(error.clone());
    }
    lines.push(String::new());
    lines.push("TEST EVIDENCE · observed output only; AM2 did not run tests".into());
    if let Some(evidence) = &project.test_evidence {
        lines.push(format!(
            "{} from {} · {}",
            evidence.result.label(),
            evidence.agent,
            elapsed_label(now, evidence.observed_at)
        ));
    } else {
        lines.push("No recent test result observed in agent output".into());
    }
    frame.render_widget(
        Paragraph::new(lines.join("\n"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Magenta))
                    .title("Project · read only"),
            )
            .scroll((state.detail_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn state_style(state: AgentState) -> Style {
    let color = match state {
        AgentState::Working => Color::Green,
        AgentState::Idle => Color::Gray,
        AgentState::Waiting => Color::Yellow,
        AgentState::Finished => Color::Cyan,
        AgentState::Failed => Color::Red,
        AgentState::Unknown => Color::Gray,
    };
    Style::default().fg(color)
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::domain::{AgentKind, RuntimeMetadata};
    use crate::history::{HistoryEvent, HistoryEventKind, HistorySession};
    use crate::projects::{ChangedFile, ProjectAgent, ProjectSnapshot, TestEvidence, TestResult};
    use ratatui::backend::TestBackend;

    fn screen_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn display_agent() -> AgentView {
        AgentView {
            observation: AgentObservation {
                id: "herdr:term_a:42".into(),
                kind: AgentKind::Codex,
                project: "am2".into(),
                state: AgentState::Working,
                pid: 42,
                cwd: "/Projects/am2".into(),
                runtime: RuntimeMetadata {
                    provider: "Herdr".into(),
                    target: "w1:p1".into(),
                    fields: vec![("Workspace".into(), "w1".into())],
                },
                recent_output: "cargo test\n17 tests passed".into(),
                output_error: None,
            },
            activity: "running tests".into(),
            last_output_change: Some(95),
        }
    }

    #[test]
    fn herdr_host_link_is_shared_by_agents_in_one_workspace() {
        let mut first = display_agent().observation;
        first
            .runtime
            .fields
            .iter_mut()
            .find(|(name, _)| name == "Workspace")
            .unwrap()
            .1 = "w7".into();
        let mut second = first.clone();
        second.id = "herdr:term_b:43".into();
        second.runtime.target = "w7:p9".into();
        assert_eq!(herdr_host_key(&first), "herdr-host:w7");
        assert_eq!(herdr_host_key(&first), herdr_host_key(&second));
    }

    #[test]
    fn launch_form_uses_selected_project_and_renders_choices() {
        let mut state = DashboardState::new(vec![display_agent()]);
        state.open_launch_form();
        let form = state.launch_form.as_mut().unwrap();
        assert_eq!(form.cwd, "/Projects/am2");
        assert_eq!(form.kind, AgentKind::Codex);
        assert_eq!(form.destination, Destination::Herdr);
        assert_eq!(form.request().herdr_anchor.as_deref(), Some("w1:p1"));
        form.field = 1;
        form.change(true);
        assert_eq!(form.kind, AgentKind::Claude);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("New agent"));
        assert!(text.contains("/Projects/am2"));
        assert!(text.contains("Claude"));
        assert!(text.contains("Enter launch"));
    }

    #[test]
    fn configured_launch_destination_overrides_auto_and_falls_back_without_herdr() {
        let mut state = DashboardState::new(vec![display_agent()]);
        state.launch_default = LaunchDestination::ITerm2;
        state.open_launch_form();
        assert_eq!(
            state.launch_form.as_ref().unwrap().destination,
            Destination::Terminal(Provider::ITerm2)
        );

        assert_eq!(
            select_launch_destination(
                LaunchDestination::Herdr,
                &[
                    Destination::Terminal(Provider::Ghostty),
                    Destination::Terminal(Provider::ITerm2)
                ]
            ),
            Destination::Terminal(Provider::Ghostty)
        );
    }

    #[test]
    fn unified_search_opens_exact_live_and_saved_results_then_returns_to_search() {
        let mut state = DashboardState::new(vec![display_agent()]);
        state.set_history(HistoryInventory {
            sessions: vec![HistorySession {
                kind: AgentKind::Claude,
                id: "session-42".into(),
                cwd: "/Projects/backend".into(),
                project: "backend".into(),
                updated_at: 120,
                events: vec![HistoryEvent {
                    timestamp: None,
                    kind: HistoryEventKind::Assistant,
                    text: "fixed authentication".into(),
                }],
            }],
            skipped_files: 0,
        });
        state.open_unified_search();
        state.unified_query = "auth".into();
        state.set_unified_results(Ok(vec![
            SearchResult {
                target: SearchTarget::Live("herdr:term_a:42".into()),
                kind: AgentKind::Codex,
                project: "am2".into(),
                source: "Live agent".into(),
                snippet: "running tests".into(),
                observed_at: Some(95),
            },
            SearchResult {
                target: SearchTarget::Saved {
                    kind: AgentKind::Claude,
                    session_id: "session-42".into(),
                    event_index: 0,
                },
                kind: AgentKind::Claude,
                project: "backend".into(),
                source: "Saved Agent".into(),
                snippet: "fixed authentication".into(),
                observed_at: Some(120),
            },
        ]));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 130)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Search · live agents"));
        assert!(text.contains("Live agent"));
        assert!(text.contains("Saved Agent"));
        state.unified_selected = Some(1);
        let refreshed = state.unified_results.iter().cloned().rev().collect();
        state.set_unified_results(Ok(refreshed));
        assert_eq!(state.unified_selected, Some(0));
        state.unified_selected = Some(1);
        assert!(state.inspect_unified_result());
        assert_eq!(state.screen, Screen::InspectAgent("herdr:term_a:42".into()));
        state.back();
        assert_eq!(state.screen, Screen::Search);
        state.unified_selected = Some(0);
        assert!(state.inspect_unified_result());
        assert_eq!(state.screen, Screen::InspectHistory(0, Some(0)));
        state.back();
        assert_eq!(state.screen, Screen::Search);
        state.back();
        assert_eq!(state.screen, Screen::Dashboard);
    }

    #[test]
    fn recorded_activity_shows_age_without_changing_unknown_state() {
        let mut view = display_agent();
        view.observation.kind = AgentKind::Claude;
        view.observation.state = AgentState::Unknown;
        view.observation.runtime.provider = "macOS".into();
        view.observation
            .runtime
            .fields
            .push(("Session ID".into(), "exact-1".into()));
        view.observation.recent_output.clear();
        view.observation.output_error = Some("No pane output".into());
        view.activity = "activity unknown".into();
        let mut state = DashboardState::new(vec![view]);
        state.set_history(HistoryInventory {
            sessions: vec![HistorySession {
                kind: AgentKind::Claude,
                id: "exact-1".into(),
                cwd: "/Projects/am2".into(),
                project: "am2".into(),
                updated_at: 150,
                events: vec![HistoryEvent {
                    timestamp: None,
                    kind: HistoryEventKind::Tool,
                    text: "cargo test".into(),
                }],
            }],
            skipped_files: 0,
        });
        let mut terminal = Terminal::new(TestBackend::new(100, 26)).unwrap();
        terminal.draw(|frame| render(frame, &state, 200)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("AGE"));
        assert!(text.contains("recorded tool"));
        assert!(text.contains("50s"));
        assert!(text.contains("unknown"));
        state.inspect();
        terminal.draw(|frame| render(frame, &state, 200)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Claude session file"));
        assert!(text.contains("Claude agents --json"));
        assert!(text.contains("recorded write"));
    }

    #[test]
    fn project_view_shows_changes_agents_test_evidence_and_next_step() {
        let mut state = DashboardState::new(vec![display_agent()]);
        state.set_projects(vec![ProjectSnapshot {
            root: "/Projects/am2".into(),
            name: "am2".into(),
            git: true,
            branch: Some("main".into()),
            latest_commit: Some("abc123 Add dashboard".into()),
            change_count: 1,
            changes: vec![ChangedFile {
                status: " M".into(),
                path: "src/main.rs".into(),
            }],
            agents: vec![ProjectAgent {
                id: "herdr:term_a:42".into(),
                kind: AgentKind::Codex,
                state: AgentState::Working,
                activity: "running tests".into(),
            }],
            test_evidence: Some(TestEvidence {
                result: TestResult::Passed,
                agent: AgentKind::Codex,
                observed_at: Some(95),
            }),
            next_step: "Review changed files before committing".into(),
            error: None,
        }]);
        state.open_projects();
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Projects"));
        assert!(text.contains("am2"));
        state.inspect();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("src/main.rs"));
        assert!(text.contains("tests passed"));
        assert!(text.contains("Review changed files"));
        assert!(text.contains("Add dashboard"));
    }

    #[test]
    fn history_search_renders_saved_result_and_matched_timeline_event() {
        let mut state = DashboardState::new(vec![display_agent()]);
        state.set_history(HistoryInventory {
            sessions: vec![HistorySession {
                kind: AgentKind::Pi,
                id: "pi-session-1".into(),
                cwd: "/Projects/mobile".into(),
                project: "mobile".into(),
                updated_at: 100,
                events: vec![HistoryEvent {
                    timestamp: Some("2026-10-09T12:30:00Z".into()),
                    kind: HistoryEventKind::Assistant,
                    text: "Resolved comet parser error".into(),
                }],
            }],
            skipped_files: 0,
        });
        state.open_history();
        state.start_search();
        for ch in "comet".chars() {
            state.search_push(ch);
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("History"));
        assert!(text.contains("Resolved comet parser error"));
        assert!(text.contains("saved"));
        state.inspect();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("SEARCH MATCH"));
        assert!(text.contains("RECENT TIMELINE"));
    }

    #[test]
    fn wide_inspection_keeps_agent_list_beside_details() {
        let mut state = DashboardState::new(vec![display_agent()]);
        state.inspect();
        state.inspection_preview = Some(InspectionPreview {
            agent_id: state.selected_agent().unwrap().observation.id.clone(),
            source: "Herdr pane".into(),
            text: "17 tests passed".into(),
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 28)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Agents"));
        assert!(text.contains("Inspect agent"));
        assert!(text.contains("17 tests passed"));
    }

    #[test]
    fn narrow_inspection_uses_full_width_details() {
        let mut state = DashboardState::new(vec![display_agent()]);
        state.inspect();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Inspect agent"));
        assert!(!text.contains("Agents"));
    }

    #[test]
    fn standalone_inspection_labels_visible_terminal_text() {
        let mut agent = display_agent();
        agent.observation.id = "native:42:start".into();
        agent.observation.kind = AgentKind::Pi;
        agent.observation.runtime.provider = "macOS".into();
        agent.observation.runtime.target = "PID 42".into();
        agent.observation.recent_output.clear();
        agent.observation.output_error = Some("No pane output available".into());
        let mut state = DashboardState::new(vec![agent]);
        state.inspect();
        state.inspection_preview = Some(InspectionPreview {
            agent_id: "native:42:start".into(),
            source: "iTerm2".into(),
            text: "Pi visible screen marker".into(),
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 28)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Visible terminal text"));
        assert!(text.contains("Pi visible screen marker"));
        assert!(!text.contains("No pane output available"));
    }

    #[test]
    fn dashboard_uses_compact_terminal_safe_symbols() {
        let state = DashboardState::new(vec![display_agent()]);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("AM2"));
        assert!(!text.contains("🧠"));
        assert!(!text.contains("🤖"));
        assert_eq!(agent_icon(AgentKind::Codex).chars().count(), 1);
    }

    #[test]
    fn header_counts_live_agents_and_marks_unknown_status_without_inbox() {
        let mut known = display_agent();
        known.observation.state = AgentState::Unknown;
        let state = DashboardState::new(vec![known]);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("1 live agent"));
        assert!(text.contains("1 unknown"));
        assert!(!text.contains("need attention"));
        assert!(!text.contains("0 working"));
        assert!(!text.contains("inbox"));
    }

    #[test]
    fn waiting_state_is_visible_as_needs_input_in_dashboard() {
        let mut waiting = display_agent();
        waiting.observation.state = AgentState::Waiting;
        waiting.activity = "idle".into();
        let state = DashboardState::new(vec![waiting]);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("1 needs input"));
        assert!(text.contains("needs input"));
    }

    #[test]
    fn idle_state_is_not_counted_as_needs_input_in_dashboard() {
        let mut idle = display_agent();
        idle.observation.state = AgentState::Idle;
        idle.activity = crate::activity::summarize(AgentState::Idle, "");
        let state = DashboardState::new(vec![idle]);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("1 idle"));
        assert!(text.contains("○ idle"));
        assert!(text.contains("—"));
        assert!(!text.contains("needs input"));
    }

    #[test]
    fn dashboard_search_renders_only_matching_agents() {
        let mut pi = display_agent();
        pi.observation.id = "pi".into();
        pi.observation.kind = AgentKind::Pi;
        pi.observation.project = "LLM_Core".into();
        let mut state = DashboardState::new(vec![display_agent(), pi]);
        state.start_search();
        for ch in "llm".chars() {
            state.search_push(ch);
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("/ llm"));
        assert!(text.contains("LLM_Core"));
        assert!(!text.contains("Codex"));
    }

    #[test]
    fn terminal_listing_error_is_visible_in_status_area() {
        let mut state = DashboardState::new(vec![]);
        state.set_notice("Could not list terminals: automation returned Connection Invalid while reading the terminal scripting dictionary");
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Connection Invalid"));
        assert!(text.contains("scripting dictionary"));
    }

    #[test]
    fn terminal_picker_shows_provider_identity_and_directory() {
        let mut state = DashboardState::new(vec![]);
        state.terminal_picker = Some(TerminalPicker {
            agent_id: "agent".into(),
            terminals: vec![TerminalCandidate {
                provider: crate::terminals::Provider::Ghostty,
                instance_pid: 41,
                id: "A1B2-C3D4".into(),
                tty: None,
                title: "Codex session".into(),
                cwd: Some("/Projects/am2".into()),
                window_id: "W1".into(),
                window_title: "Agent window".into(),
            }],
            selected: Some(0),
        });
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Codex session"));
        assert!(text.contains("/Projects/am2"));
        assert!(text.contains("A1B2-C3D4"));
        assert!(text.contains("Agent window"));
        assert!(text.contains("Enter/y link"));
    }

    #[test]
    fn dashboard_does_not_crowd_activity_with_last_output_column() {
        let state = DashboardState::new(vec![display_agent()]);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("ACTIVITY"));
        assert!(!text.contains("LAST OUT"));
    }

    #[test]
    fn idle_dashboard_has_no_periodic_refresh_label_and_explains_zero_panes() {
        let state = DashboardState::new(vec![]);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("No running agents found"));
        assert!(!text.contains("refreshing"));
    }

    #[test]
    fn unavailable_herdr_is_distinct_from_a_valid_empty_snapshot() {
        let mut state = DashboardState::new(vec![]);
        state.mark_error("socket unavailable".into());
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("inventory unavailable"));
        assert!(text.contains("socket unavailable"));
    }

    #[test]
    fn stale_footer_keeps_retry_and_quit_visible_at_eighty_columns() {
        let mut state = DashboardState::new(vec![]);
        state.mark_error(
            "Herdr: `herdr api snapshot` failed: Operation not permitted on the Unix socket".into(),
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("r retry"));
        assert!(text.contains("q quit"));
    }

    #[test]
    fn inspection_shows_runtime_fields_and_recent_output_at_eighty_columns() {
        let agent = AgentView {
            observation: AgentObservation {
                id: "herdr:term_a:42".into(),
                kind: AgentKind::Codex,
                project: "am2".into(),
                state: AgentState::Working,
                pid: 42,
                cwd: "/Projects/am2".into(),
                runtime: RuntimeMetadata {
                    provider: "Herdr".into(),
                    target: "w1:p1".into(),
                    fields: vec![
                        ("Tab".into(), "w1:t1".into()),
                        ("Workspace".into(), "w1".into()),
                        ("Terminal".into(), "term_a".into()),
                        ("Mode".into(), "interactive".into()),
                        ("Herdr status".into(), "working".into()),
                    ],
                },
                recent_output: "cargo test\ntest result: ok".into(),
                output_error: None,
            },
            activity: "tests passed".into(),
            last_output_change: Some(99),
        };
        let mut state = DashboardState::new(vec![agent]);
        state.inspect();
        state.inspection_preview = Some(InspectionPreview {
            agent_id: "herdr:term_a:42".into(),
            source: "Herdr pane".into(),
            text: "cargo test".into(),
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Workspace"));
        assert!(text.contains("PID"));
        assert!(text.contains("Recent pane output"));
        assert!(text.contains("cargo test"));
    }

    #[test]
    fn inspection_can_scroll_past_a_long_path_to_workspace_and_output() {
        let agent = AgentView {
            observation: AgentObservation {
                id: "herdr:term_a:42".into(),
                kind: AgentKind::Codex,
                project: "am2".into(),
                state: AgentState::Working,
                pid: 42,
                cwd: format!("/Projects/{}", "very-long-directory/".repeat(20)),
                runtime: RuntimeMetadata {
                    provider: "Herdr".into(),
                    target: "w1:p1".into(),
                    fields: vec![("Workspace".into(), "w1".into())],
                },
                recent_output: "cargo test".into(),
                output_error: None,
            },
            activity: "building project".into(),
            last_output_change: None,
        };
        let mut state = DashboardState::new(vec![agent]);
        state.inspect();
        state.inspection_preview = Some(InspectionPreview {
            agent_id: "herdr:term_a:42".into(),
            source: "Herdr pane".into(),
            text: "cargo test".into(),
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut saw_workspace = false;
        let mut saw_output = false;
        for _ in 0..40 {
            terminal.draw(|frame| render(frame, &state, 100)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            saw_workspace |= text.contains("Workspace");
            saw_output |= text.contains("cargo test");
            state.next();
        }
        assert!(saw_workspace, "workspace must be reachable by scrolling");
        assert!(saw_output, "recent output must be reachable by scrolling");
    }

    #[test]
    fn session_labels_distinguish_agents_in_one_project() {
        let mut first = display_agent();
        first
            .observation
            .runtime
            .fields
            .push(("Session ID".into(), "abcd1111-full".into()));
        let mut second = first.clone();
        second.observation.id = "second".into();
        second.observation.pid = 99;
        second.observation.runtime.target = "w1:p2".into();
        second
            .observation
            .runtime
            .fields
            .retain(|(key, _)| key != "Session ID");
        second
            .observation
            .runtime
            .fields
            .push(("Session ID".into(), "efgh2222-full".into()));
        let mut state = DashboardState::new(vec![]);
        state.set_agents(vec![second, first]);
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("#abcd1111"));
        assert!(text.contains("#efgh2222"));
        assert!(text.contains("LOCATION"));
        assert!(text.contains("Herdr"));
    }

    #[test]
    fn native_location_requires_a_saved_link_for_app_name() {
        let mut agent = display_agent().observation;
        agent.runtime.provider = "macOS".into();
        agent.runtime.fields = vec![("TTY".into(), "ttys017".into())];
        assert_eq!(location_label(&agent, None), "ttys017 · app unknown");
        assert_eq!(
            location_label(&agent, Some("iterm2:123:456")),
            "iTerm2 · ttys017"
        );
    }

    #[test]
    fn herdr_location_includes_host_when_linked() {
        let mut agent = display_agent().observation;
        agent.runtime.fields.push(("Tab".into(), "w1:t1".into()));
        assert_eq!(
            location_label(&agent, None),
            "Herdr · Workspace 1 / Tab 1 / Pane 1"
        );
        assert_eq!(
            location_label(&agent, Some("ghostty:123:window-1")),
            "Herdr/Ghostty · Workspace 1 / Tab 1 / Pane 1"
        );
    }

    #[test]
    fn narrow_dashboard_keeps_each_agent_on_one_row_with_location_column() {
        let state = DashboardState::new(vec![display_agent()]);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let lines = (0..24)
            .map(|y| {
                (0..80)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(lines.iter().any(|line| line.contains("LOCATION")));
        assert!(lines
            .iter()
            .any(|line| line.contains("Codex") && line.contains("Herdr")));
    }

    #[test]
    fn medium_dashboard_does_not_let_activity_push_location_to_the_edge() {
        let state = DashboardState::new(vec![display_agent()]);
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        let activity = text.find("ACTIVITY").unwrap();
        let location = text.find("LOCATION").unwrap();
        assert!(
            location - activity < 36,
            "headers too far apart: {location} - {activity}"
        );
    }

    #[test]
    fn wide_dashboard_explains_herdr_workspace_tab_and_pane_ids() {
        let mut agent = display_agent();
        agent
            .observation
            .runtime
            .fields
            .push(("Tab".into(), "w1:t1".into()));
        let state = DashboardState::new(vec![agent]);
        let mut terminal = Terminal::new(TestBackend::new(132, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state, 100)).unwrap();
        let text = screen_text(&terminal);
        assert!(
            text.contains("Herdr · Workspace 1 / Tab 1 / Pane 1"),
            "rendered dashboard:\n{text}"
        );
    }
}
