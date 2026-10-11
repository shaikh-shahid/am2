//! Ephemeral full-text index for live agents and bounded saved-session events.
//! The index lives only in memory; no transcript is added to AM2's data file.

use rusqlite::{params, Connection};

use crate::domain::{AgentKind, AgentView};
use crate::history::HistoryInventory;

const MAX_RESULTS: i64 = 80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchTarget {
    Live(String),
    Saved {
        kind: AgentKind,
        session_id: String,
        event_index: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub target: SearchTarget,
    pub kind: AgentKind,
    pub project: String,
    pub source: String,
    pub snippet: String,
    pub observed_at: Option<i64>,
}

pub struct SearchIndex {
    connection: Connection,
}

impl SearchIndex {
    pub fn new() -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "PRAGMA temp_store=MEMORY;
                 CREATE VIRTUAL TABLE documents USING fts5(
                source UNINDEXED,
                source_id UNINDEXED,
                agent_kind,
                project,
                cwd,
                body,
                event_index UNINDEXED,
                observed_at UNINDEXED,
                event_kind UNINDEXED,
                tokenize='unicode61'
            );",
            )
            .map_err(|error| format!("Search index unavailable: {error}"))?;
        Ok(Self { connection })
    }

    pub fn replace_live(&mut self, agents: &[AgentView]) -> Result<(), String> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute("DELETE FROM documents WHERE source='live'", [])
            .map_err(|error| error.to_string())?;
        for view in agents {
            let observation = &view.observation;
            let body =
                if observation.output_error.is_none() && !observation.recent_output.is_empty() {
                    format!("{}\n{}", view.activity, observation.recent_output)
                } else {
                    view.activity.clone()
                };
            transaction.execute(
                "INSERT INTO documents (source,source_id,agent_kind,project,cwd,body,event_index,observed_at,event_kind)
                 VALUES ('live',?1,?2,?3,?4,?5,'',?6,'')",
                params![observation.id, observation.kind.to_string(), observation.project,
                    observation.cwd, body, view.last_output_change],
            ).map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn replace_history(&mut self, history: &HistoryInventory) -> Result<(), String> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute("DELETE FROM documents WHERE source='saved'", [])
            .map_err(|error| error.to_string())?;
        for session in &history.sessions {
            for (event_index, event) in session.events.iter().enumerate() {
                transaction.execute(
                    "INSERT INTO documents (source,source_id,agent_kind,project,cwd,body,event_index,observed_at,event_kind)
                     VALUES ('saved',?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![session.id, session.kind.to_string(), session.project,
                        session.cwd, event.text, event_index.to_string(), session.updated_at,
                        event.kind.label()],
                ).map_err(|error| error.to_string())?;
            }
        }
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn query(&self, raw: &str) -> Result<Vec<SearchResult>, String> {
        let parsed = ParsedQuery::parse(raw);
        let has_text = !parsed.terms.is_empty();
        let expression = parsed
            .terms
            .iter()
            .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        let match_clause = if has_text {
            "documents MATCH ?1"
        } else {
            "?1 = ''"
        };
        let snippet = if has_text {
            "snippet(documents, 5, '[', ']', '…', 18)"
        } else {
            "substr(body, 1, 160)"
        };
        let order = if has_text {
            "bm25(documents), CAST(observed_at AS INTEGER) DESC"
        } else {
            "CAST(observed_at AS INTEGER) DESC"
        };
        let sql = format!(
            "SELECT source,source_id,agent_kind,project,{snippet},event_index,observed_at,event_kind
             FROM documents WHERE {match_clause}
             AND (?2 = '' OR instr(lower(project), ?2) > 0 OR instr(lower(cwd), ?2) > 0)
             AND (?3 = '' OR lower(agent_kind) = ?3)
             ORDER BY {order} LIMIT {MAX_RESULTS}"
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![expression, parsed.project, parsed.agent], |row| {
                let source: String = row.get(0)?;
                let id: String = row.get(1)?;
                let kind_text: String = row.get(2)?;
                let event_index: String = row.get(5)?;
                let event_kind: String = row.get(7)?;
                Ok((
                    source,
                    id,
                    kind_text,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    event_index,
                    row.get::<_, Option<i64>>(6)?,
                    event_kind,
                ))
            })
            .map_err(|error| format!("Search query: {error}"))?;
        let mut hits = Vec::new();
        for row in rows {
            let (source, id, kind_text, project, snippet, event_index, observed_at, event_kind) =
                row.map_err(|error| error.to_string())?;
            let Some(kind) = AgentKind::parse(&kind_text) else {
                continue;
            };
            let target = if source == "live" {
                SearchTarget::Live(id)
            } else {
                let Ok(event_index) = event_index.parse::<usize>() else {
                    continue;
                };
                SearchTarget::Saved {
                    kind,
                    session_id: id,
                    event_index,
                }
            };
            hits.push(SearchResult {
                target,
                kind,
                project,
                source: if source == "live" {
                    "Live agent".into()
                } else {
                    format!("Saved {event_kind}")
                },
                snippet: snippet.split_whitespace().collect::<Vec<_>>().join(" "),
                observed_at,
            });
        }
        Ok(hits)
    }
}

#[derive(Default)]
struct ParsedQuery {
    terms: Vec<String>,
    project: String,
    agent: String,
}

impl ParsedQuery {
    fn parse(raw: &str) -> Self {
        let mut query = Self::default();
        for token in raw.split_whitespace() {
            match token.split_once(':') {
                Some((key, value)) if key.eq_ignore_ascii_case("project") => {
                    query.project = value.to_lowercase();
                }
                Some((key, value)) if key.eq_ignore_ascii_case("agent") => {
                    query.agent = value.to_lowercase();
                }
                _ => query.terms.push(token.to_owned()),
            }
        }
        query
    }
}
