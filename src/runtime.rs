use std::collections::HashSet;

use crate::domain::{AgentObservation, Runtime};

/// Prefer pane-backed observations because they carry state, output and focus.
pub struct CombinedRuntime<H, N> {
    herdr: H,
    native: N,
}

impl<H, N> CombinedRuntime<H, N> {
    pub fn new(herdr: H, native: N) -> Self {
        Self { herdr, native }
    }
}

impl<H: Runtime + Sync, N: Runtime + Sync> Runtime for CombinedRuntime<H, N> {
    fn observe(&self) -> Result<Vec<AgentObservation>, String> {
        let (herdr, native) = std::thread::scope(|scope| {
            let herdr = scope.spawn(|| self.herdr.observe());
            let native = scope.spawn(|| self.native.observe());
            (
                herdr.join().expect("Herdr observer panicked"),
                native.join().expect("macOS observer panicked"),
            )
        });
        match (herdr, native) {
            (Ok(mut pane_agents), Ok(native_agents)) => {
                for pane in &mut pane_agents {
                    if pane
                        .runtime
                        .fields
                        .iter()
                        .any(|(key, _)| key == "Session ID")
                    {
                        continue;
                    }
                    if let Some(id) = native_agents
                        .iter()
                        .find(|native| native.pid == pane.pid && native.kind == pane.kind)
                        .and_then(|native| {
                            native
                                .runtime
                                .fields
                                .iter()
                                .find(|(key, _)| key == "Session ID")
                        })
                        .map(|(_, id)| id.clone())
                    {
                        pane.runtime.fields.push(("Session ID".into(), id));
                    }
                }
                let mut seen: HashSet<u32> = pane_agents.iter().map(|agent| agent.pid).collect();
                pane_agents.extend(
                    native_agents
                        .into_iter()
                        .filter(|agent| seen.insert(agent.pid)),
                );
                pane_agents.sort_by(|a, b| {
                    (&a.project, a.kind.to_string(), a.pid).cmp(&(
                        &b.project,
                        b.kind.to_string(),
                        b.pid,
                    ))
                });
                Ok(pane_agents)
            }
            (Ok(agents), Err(_)) | (Err(_), Ok(agents)) => Ok(agents),
            (Err(herdr), Err(native)) => Err(format!("Herdr: {herdr}; macOS: {native}")),
        }
    }
}
