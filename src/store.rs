use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::activity::{output_fingerprint, summarize};
use crate::domain::{AgentObservation, AgentView};

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Database(rusqlite::Error),
    Json(serde_json::Error),
    Runtime(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "AM2 data file: {error}"),
            Self::Database(error) => write!(f, "AM2 database: {error}"),
            Self::Json(error) => write!(f, "AM2 metadata: {error}"),
            Self::Runtime(error) => write!(f, "{error}"),
        }
    }
}

impl Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub struct StateStore {
    connection: Connection,
}

impl StateStore {
    pub fn default_path() -> Result<PathBuf, StoreError> {
        if let Some(directory) = std::env::var_os("AM2_DATA_DIR") {
            return Ok(PathBuf::from(directory).join("inbox.sqlite3"));
        }
        let home = std::env::var_os("HOME")
            .ok_or_else(|| StoreError::Runtime("HOME is required for the AM2 data path".into()))?;
        Ok(PathBuf::from(home).join("Library/Application Support/AM2/inbox.sqlite3"))
    }

    pub fn open_default() -> Result<Self, StoreError> {
        let path = Self::default_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Self::open(path)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn memory() -> Result<Self, StoreError> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(connection: Connection) -> Result<Self, StoreError> {
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS observed_agents (
                agent_id TEXT PRIMARY KEY,
                state TEXT NOT NULL,
                present INTEGER NOT NULL,
                sequence INTEGER NOT NULL,
                output_hash TEXT,
                last_output_change INTEGER,
                project TEXT NOT NULL,
                pid INTEGER NOT NULL,
                cwd TEXT NOT NULL,
                runtime_json TEXT NOT NULL,
                agent_kind TEXT NOT NULL DEFAULT 'Codex'
            );
            CREATE TABLE IF NOT EXISTS ghostty_links (
                agent_id TEXT PRIMARY KEY,
                terminal_id TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS terminal_links (
                agent_id TEXT PRIMARY KEY,
                target TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS agent_aliases (
                session_key TEXT PRIMARY KEY,
                label TEXT NOT NULL
            );",
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO terminal_links (agent_id, target)
             SELECT agent_id, 'ghostty:' || terminal_id FROM ghostty_links
             WHERE instr(terminal_id, ':') > 0",
            [],
        )?;
        let column_count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('observed_agents') WHERE name='agent_kind'",
            [],
            |row| row.get(0),
        )?;
        if column_count == 0 {
            connection.execute_batch(
                "ALTER TABLE observed_agents ADD COLUMN agent_kind TEXT NOT NULL DEFAULT 'Codex'",
            )?;
        }
        Ok(Self { connection })
    }

    /// A failed observation leaves stored state untouched.
    pub fn apply_refresh(
        &mut self,
        refresh: Result<Vec<AgentObservation>, String>,
        now: i64,
    ) -> Result<Vec<AgentView>, StoreError> {
        let observations = refresh.map_err(StoreError::Runtime)?;
        let transaction = self.connection.transaction()?;
        let mut seen = HashSet::new();
        let mut views = Vec::with_capacity(observations.len());
        for observation in observations {
            seen.insert(observation.id.clone());
            let prior: Option<(Option<String>, Option<i64>)> = transaction
                .query_row(
                    "SELECT output_hash,last_output_change FROM observed_agents WHERE agent_id=?1",
                    [&observation.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let prior_hash = prior.as_ref().and_then(|value| value.0.clone());
            let hash = if observation.output_error.is_none() {
                Some(format!(
                    "{:016x}",
                    output_fingerprint(&observation.recent_output)
                ))
            } else {
                prior_hash.clone()
            };
            let last_output_change = if prior_hash.is_some() && hash != prior_hash {
                Some(now)
            } else {
                prior.and_then(|value| value.1)
            };
            transaction.execute(
                "INSERT INTO observed_agents
                 (agent_id,state,present,sequence,output_hash,last_output_change,project,pid,cwd,runtime_json,agent_kind)
                 VALUES (?1,?2,1,0,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(agent_id) DO UPDATE SET
                 state=excluded.state,present=1,
                 output_hash=excluded.output_hash,last_output_change=excluded.last_output_change,
                 project=excluded.project,pid=excluded.pid,cwd=excluded.cwd,
                 runtime_json=excluded.runtime_json,agent_kind=excluded.agent_kind",
                params![
                    observation.id,
                    observation.state.label(),
                    hash,
                    last_output_change,
                    observation.project,
                    observation.pid,
                    observation.cwd,
                    serde_json::to_string(&observation.runtime)?,
                    observation.kind.to_string(),
                ],
            )?;
            views.push(AgentView {
                activity: summarize(observation.state, &observation.recent_output),
                observation,
                last_output_change,
            });
        }
        let mut statement =
            transaction.prepare("SELECT agent_id FROM observed_agents WHERE present=1")?;
        let previously_present = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for id in previously_present {
            if !seen.contains(&id) {
                transaction.execute(
                    "UPDATE observed_agents SET present=0 WHERE agent_id=?1",
                    [id],
                )?;
            }
        }
        transaction.commit()?;
        Ok(views)
    }

    pub fn link_ghostty(&self, agent_id: &str, terminal_id: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO ghostty_links (agent_id, terminal_id) VALUES (?1, ?2)
             ON CONFLICT(agent_id) DO UPDATE SET terminal_id=excluded.terminal_id",
            params![agent_id, terminal_id],
        )?;
        Ok(())
    }

    pub fn ghostty_link(&self, agent_id: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT terminal_id FROM ghostty_links WHERE agent_id=?1",
                [agent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn link_terminal(&self, agent_id: &str, target: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO terminal_links (agent_id, target) VALUES (?1, ?2)
             ON CONFLICT(agent_id) DO UPDATE SET target=excluded.target",
            params![agent_id, target],
        )?;
        Ok(())
    }

    pub fn terminal_link(&self, agent_id: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT target FROM terminal_links WHERE agent_id=?1",
                [agent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn set_alias(&self, session_key: &str, label: &str) -> Result<(), StoreError> {
        let label = label.trim();
        if label.is_empty() {
            self.connection.execute(
                "DELETE FROM agent_aliases WHERE session_key=?1",
                [session_key],
            )?;
        } else {
            self.connection.execute(
                "INSERT INTO agent_aliases (session_key, label) VALUES (?1, ?2)
                 ON CONFLICT(session_key) DO UPDATE SET label=excluded.label",
                params![session_key, label],
            )?;
        }
        Ok(())
    }

    pub fn alias(&self, session_key: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT label FROM agent_aliases WHERE session_key=?1",
                [session_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }
}
