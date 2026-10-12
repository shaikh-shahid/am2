# AM2 - Unified Agent Dashboard

AM2 is a terminal dashboard for **Codex, Claude, OpenCode, Pi, Gemini CLI, and Cursor Agent on macOS**. It discovers local CLI processes without Herdr. When Herdr is available, AM2 adds pane status, recent output, and precise pane navigation. AM2 can launch an agent in a new terminal surface; observation and inspection remain read-only.

### Install on macOS

Install the latest beta from [GitHub Releases](https://github.com/shaikh-shahid/am2/releases) with:

```sh
curl -fsSL https://github.com/shaikh-shahid/am2/releases/download/v0.1.2/install.sh | sh
```

The installer selects the Mac architecture, verifies the archive against `SHA256SUMS`, and installs `am2` with its two terminal helpers in `~/.local/bin`. Downloads may take a little while and currently show limited progress. If `~/.local/bin` is not on `PATH`, the installer adds it to `~/.zprofile` or `~/.bash_profile`; open a new terminal afterward. Run `am2 doctor` to verify the installation. Set `AM2_INSTALL_DIR` to choose another absolute install directory. For manual installation, download the archive for your Mac from [the v0.1.1 release](https://github.com/shaikh-shahid/am2/releases/tag/v0.1.1) and follow its `INSTALL.md`.

## Run

Start AM2 from any macOS terminal:

```sh
am2
```
OR 

```sh
cargo run
```

For development from this checkout:

```sh
cargo run -- doctor
cargo run
```

`am2 help` lists commands and options; `am2 --version` prints the installed version. `am2 doctor` checks required process tools, optional agent/Herdr CLIs, the AM2 data store, and timed Herdr/macOS inventory probes. It reports each source separately so an unavailable Herdr server does not hide a working local process scan. `am2 update` checks GitHub Releases and asks before installing a newer macOS build. Use `am2 update --check` to check without installing, or `am2 update --yes` for a noninteractive update. The updater verifies the release archive against `SHA256SUMS` and replaces `am2` and both terminal helpers in the installed binary directory. Source builds can check releases but must be updated by installing a release or rebuilding from source.

### GitHub Releases

CI runs formatting, Clippy, tests, packaging, and installer checks on Apple Silicon and Intel macOS runners for every push and pull request. Pushing a `v` tag that matches the version in `Cargo.toml` builds both release archives, writes SHA-256 checksums, and publishes a GitHub prerelease with a tag-specific `install.sh`. For example, version `0.1.1` uses tag `v0.1.1`.

### Configuration

AM2 works without a config file. Run `am2 config path` to see its location, or `am2 config init` to create a commented file without overwriting an existing one. The default path is `~/Library/Application Support/AM2/config.toml`; `AM2_CONFIG_FILE` can point to another file. Restart AM2 after editing it.

```toml
[dashboard]
refresh_interval_seconds = 5 # 1–300

[launch]
default_terminal = "auto" # auto, herdr, ghostty, terminal, iterm2
```

`auto` preserves the current choice: Herdr when AM2 has a pane anchor, otherwise Ghostty. An explicit `herdr` preference falls back to Ghostty when no Herdr anchor is available. The launch form still lets you choose another destination before starting an agent. Invalid config values stop the dashboard with a path-specific error; `am2 doctor` reports the config status. Session labels and terminal links remain in AM2's SQLite data store.

The dashboard starts scanning in the background, polls every five seconds without blocking keyboard input, and redraws when input or new data arrives. Herdr pane process queries run in bounded parallel batches, while macOS groups Codex open-file queries into one `lsof` call. Use `am2 doctor` to measure both inventory sources on your machine. On terminals at least 100 columns wide, Enter opens details beside the agent list; on narrower terminals, details use the full width. The Agents list groups by project and agent type, then sorts by session identity; state changes do not move a row. It shows a short session ID (or pane/PID fallback) and WHERE. WHERE names the Herdr pane and linked host app when available, or the linked terminal app for a standalone agent. An unlinked local process shows its TTY with `app unknown`. On narrow terminals, WHERE uses a compact label in its own column; the selected agent summary and detail view show the full location. The header counts live agents by the states AM2 can verify and shows `unknown` separately. Keys:

| Key | Action |
| --- | --- |
| `↑`/`↓` or `j`/`k` | Select an agent; scroll output in inspection |
| `Enter` | Inspect the selected agent |
| `a` | Set or clear a local label for the selected live session; exact session IDs retain the label across process restarts |
| `/` | Search live agents and saved conversations together; use arrows to select, Enter to inspect, Esc to return |
| `h` | Open saved session history; from a live detail row, open its history only when the session ID matches exactly |
| `p` | Open Projects, grouped by Git repository; Enter inspects changes and agent context |
| `n` | Start a new agent in the selected project or an entered directory |
| `o` | Jump to a Herdr pane or an exact terminal session |
| `L` | Link a standalone agent to its terminal, or a Herdr agent to the window hosting Herdr |
| `Esc` | Return to the previous view |
| `r` | Refresh now |
| `q` | Quit |

For a one-shot listing or scripts:

```sh
cargo run -- list
cargo run -- list --json
```

`am2 list` is a one-shot inventory; the TUI is the agent-oriented view. AM2 merges Herdr and local process observations by PID, preferring Herdr for agents inside its panes. If Herdr is absent or disconnected, the local inventory remains usable. For a Herdr row, `o` selects the live pane. When AM2 runs outside Herdr, press `L` on a Herdr row once and choose the terminal window hosting that Herdr workspace; subsequent `o` presses select the pane and raise that window. The host link is shared across agents in the workspace. If the host window closes, link it again.

For a standalone agent in Terminal.app or iTerm2, `o` finds the session with the agent's exact terminal TTY and focuses it when there is one match. For Ghostty, press `L` and choose the correct terminal, then press Enter or `y` to save the link (`Esc` cancels). `L` lists sessions from all three supported apps and can also override an automatic match. The picker shows each provider, title, TTY or cwd, window, and app PID. Standalone links are keyed to the live agent process identity; a restarted agent needs a new link. macOS may request Automation permission for each terminal app. If a session closes or the TTY match is ambiguous, AM2 reports the problem instead of choosing another session.

## Managed launch

Press `n` to open the launch form. Edit the project directory, use Tab to select agent and terminal, use ←/→ to change those choices, and press Enter. AM2 starts the chosen CLI in a new Herdr pane, Ghostty window, Terminal.app window, or iTerm2 window. Herdr is offered when AM2 is inside a Herdr pane or can identify a live Herdr pane to split beside. From a separate terminal window, select a Herdr agent first; if all observed Herdr agents share one workspace, AM2 can also use that workspace without a Herdr row selected. The launch form shows the anchor pane. AM2 splits it with the chosen cwd, then runs the launcher in the returned pane ID. The selected agent executable must already be on `PATH`, and the project directory must exist. AM2 records the launched process ID and the exact terminal target, links them when the live inventory sees the agent, and selects the new row. Press `o` to return to it. A launch failure or a process that never appears is reported in the status line.

The launch command starts an interactive agent through the user's noninteractive login shell without sending it a prompt. macOS may request Automation permission for a terminal app. Launch uses that app's scripting interface or Herdr's CLI; no agent plugins, hooks, or configuration changes are required.

## Activity and search

For Herdr agents, AM2 reads pane output only when you inspect a row or press `r` in inspection. It takes at most 80 recent unwrapped lines and keeps at most 16 KiB. The preview is **not** a complete conversation record. For a standalone agent in Terminal.app or iTerm2, inspection can show the current visible screen when its TTY matches exactly; press `r` to update it. This preview is not full history or a verified agent state. Ghostty does not expose terminal text through its scripting interface.

The Agents table shows an activity summary and its age. Herdr provides live state; when an exact saved-session match exists, AM2 also shows the latest recorded Codex, Claude, or Pi action. Its age comes from the session file's modification time and means *recorded write*, not confirmed current work. Inspection labels the activity source and state source separately. A saved action never changes an `unknown` live state into `working` or `waiting`. Rows without an exact saved-session match keep their limited process or reported-status evidence. The history scan starts after the first live inventory and refreshes about every 60 seconds while outside History; `r` requests an immediate refresh.

The Agents view is a live inventory. Press `/` from any main view to search live agent metadata alongside saved Codex, Claude, and Pi conversation events. Search uses an in-memory SQLite FTS5 index with ranked matches and excerpts. Type multiple words to require all of them; add `project:am2` or `agent:codex` to narrow results. Arrows select a result, Enter opens the exact live agent or saved event, and Esc returns to Search, then to the previous view. The index refreshes when live inventory or saved history changes. It is discarded when AM2 exits; transcript text is not added to the on-disk AM2 database. This remains word-based search, not semantic search. AM2 no longer creates Inbox events; Herdr status and Claude's reported status remain best-effort.

Press `h` for a separate **History** list of saved Codex, Claude, and Pi sessions. Press `/` there to open the same unified search. Press `r` to reload the history. A saved session is labeled *linked live* only when its agent kind and exact session ID match a live row; otherwise it is simply *saved*. From a live agent's detail view, `h` opens its saved conversation only for that exact match. No cwd or timestamp guesses are used.

History is read directly from local JSONL files in a background worker and is not stored in AM2's database. The scanner takes up to 120 files and 96 MiB total, skips files over 24 MiB, and retains the most recent 512 supported events per session with at most 2,000 characters per event. The view displays the last 40 events and a selected search match. It reports skipped files. These are best-effort adapters for local file formats, which may change across agent releases; they are not live status feeds or complete archives. OpenCode, Gemini, and Cursor history adapters are not included yet. Search and activity extraction do not send session content to a network service or an LLM.

## Projects

Press `p` to see live agents grouped by Git repository root. A directory outside Git remains visible as its own project. Each row shows the branch, agent count, changed-file count, and any explicit test result AM2 observed in the latest Herdr activity summary. Enter shows the latest commit, changed files, agent states, and a deterministic next step such as reviewing changes or inspecting a failed test. `Esc` returns and `r` refreshes. The Projects view refreshes about every ten seconds while open.

AM2 runs bounded, local, read-only Git commands (`rev-parse`, `branch`, `log`, `status`) in a background worker with optional Git locks disabled. It inspects up to 30 project roots, caps each Git command at three seconds and 1 MiB of output, and shows up to 60 changed paths per project while retaining the total count. It does **not** run tests or infer a pass from a clean working tree. Test evidence comes only from a recognizable result in current agent output and may become stale; a missing signal is shown as unknown. Project snapshots and Git output are not stored in SQLite.

SQLite stores agent metadata, output fingerprints, and terminal links at `~/Library/Application Support/AM2/inbox.sqlite3`. The filename remains for compatibility with earlier builds; old Inbox rows are left intact but unused. Set `AM2_DATA_DIR` to use another directory. Pane output is never stored in SQLite.

## Scope

For a Herdr row, the displayed cwd comes from the agent foreground process, then Herdr's `foreground_cwd`, then the pane's launch cwd. For a standalone row, cwd comes from macOS `lsof`, with Claude's `agents --json` cwd preferred when available. The Project column is the basename of that cwd. Standalone discovery filters live CLI executables, service subcommands, and processes without a terminal TTY, then rechecks PID and process start time. It can miss renamed/custom wrappers. Claude's supported JSON inventory can enrich an exact live terminal PID with session ID and reported state; editor or detached Claude sessions without a TTY are excluded. Codex's session ID is shown only when `lsof` finds one unique open thread lock or rollout file under `CODEX_HOME`; that exact ID also enriches the corresponding Herdr row. Other process-only rows remain `unknown`. AM2 does not infer exact Codex, OpenCode, Pi, Gemini, or Cursor session IDs from cwd or timestamps. OpenCode, Gemini, and Cursor live discovery is covered by fixture tests; no running instance of those three was available here.
