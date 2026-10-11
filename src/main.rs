use std::error::Error;
use std::time::Instant;

use clap::{Parser, Subcommand};

use am2::bridge::companion_bridge;
use am2::config::AppConfig;
use am2::domain::Runtime;
use am2::herdr::HerdrRuntime;
use am2::native::NativeRuntime;
use am2::runtime::CombinedRuntime;
use am2::store::StateStore;
use am2::ui;

#[derive(Parser)]
#[command(version, about = "Terminal dashboard for local coding agents")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Print one snapshot of running local agents.
    List {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check local tools, data path, and live discovery.
    Doctor,
    /// Show the installed version and beta update instructions.
    Update,
    /// Show or create the user preferences file.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the config file path.
    Path,
    /// Create a commented config file without overwriting an existing one.
    Init,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    if !cfg!(target_os = "macos") {
        return Err("AM2 POC currently supports macOS only".into());
    }
    let runtime = CombinedRuntime::new(HerdrRuntime, NativeRuntime);
    match cli.command {
        Some(Commands::List { json }) => {
            let agents = runtime.observe()?;
            if json {
                let rows = agents
                    .iter()
                    .map(|agent| {
                        serde_json::json!({
                            "agent": agent.kind.to_string(), "project": agent.project,
                            "state": agent.state.label(), "pid": agent.pid,
                            "cwd": agent.cwd, "source": agent.runtime.provider,
                            "target": agent.runtime.target,
                        })
                    })
                    .collect::<Vec<_>>();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else if agents.is_empty() {
                println!("No running coding agents found.");
            } else {
                println!(
                    "{:<10} {:<18} {:<10} {:<8} {:<8} CWD",
                    "AGENT", "PROJECT", "STATE", "PID", "SOURCE"
                );
                for agent in agents {
                    println!(
                        "{:<10} {:<18} {:<10} {:<8} {:<8} {}",
                        agent.kind,
                        agent.project,
                        agent.state.label(),
                        agent.pid,
                        agent.runtime.provider,
                        agent.cwd
                    );
                }
            }
        }
        Some(Commands::Doctor) => doctor(),
        Some(Commands::Update) => {
            println!("AM2 {}", env!("CARGO_PKG_VERSION"));
            if let Some(repository) = option_env!("AM2_RELEASE_REPO") {
                println!("Download a newer macOS archive from {repository}/releases");
                println!("Install all three executables from the archive together.");
            } else {
                println!("Check this project's GitHub Releases page for a newer macOS archive.");
            }
            println!("Source checkout: cargo run --release --locked");
            println!("AM2 does not update itself.");
        }
        Some(Commands::Config { action }) => {
            let path = AppConfig::default_path()?;
            match action {
                ConfigAction::Path => println!("{}", path.display()),
                ConfigAction::Init => {
                    AppConfig::init(&path)?;
                    println!("Created {}", path.display());
                }
            }
        }
        None => ui::run(runtime, AppConfig::load_default()?)?,
    }
    Ok(())
}

fn doctor() {
    println!("AM2 {} · macOS diagnostics", env!("CARGO_PKG_VERSION"));
    for (program, required) in [
        ("ps", true),
        ("lsof", true),
        ("herdr", false),
        ("codex", false),
        ("claude", false),
        ("opencode", false),
        ("pi", false),
        ("gemini", false),
        ("cursor-agent", false),
    ] {
        let found = executable_on_path(program);
        let label = if found {
            "found"
        } else if required {
            "MISSING"
        } else {
            "not installed"
        };
        println!("{program:<13} {label}");
    }
    match StateStore::default_path() {
        Ok(path) => println!("Data path     {}", path.display()),
        Err(error) => println!("Data path     unavailable: {error}"),
    }
    match StateStore::open_default() {
        Ok(_) => println!("Data store    ready"),
        Err(error) => println!("Data store    unavailable: {error}"),
    }
    show_bridge(
        "Ghostty bridge",
        "am2-ghostty-bridge",
        option_env!("AM2_GHOSTTY_BRIDGE").unwrap_or(""),
    );
    show_bridge(
        "Terminal bridge",
        "am2-terminal-bridge",
        option_env!("AM2_TERMINAL_BRIDGE").unwrap_or(""),
    );
    match AppConfig::default_path() {
        Ok(path) => {
            println!("Config path   {}", path.display());
            match AppConfig::load(&path) {
                Ok(config) => println!(
                    "Config        {} · refresh {}s · launch {}",
                    if path.exists() { "valid" } else { "defaults" },
                    config.dashboard.refresh_interval_seconds,
                    config.launch.default_terminal.label()
                ),
                Err(error) => println!("Config        INVALID · {error}"),
            }
        }
        Err(error) => println!("Config        unavailable · {error}"),
    }
    let (herdr, native) = std::thread::scope(|scope| {
        let herdr = scope.spawn(|| timed_observe(&HerdrRuntime));
        let native = scope.spawn(|| timed_observe(&NativeRuntime));
        (herdr.join().unwrap(), native.join().unwrap())
    });
    show_probe("Herdr inventory", &herdr);
    show_probe("macOS inventory", &native);
    println!(
        "Live inventory  {}",
        if herdr.0.is_ok() || native.0.is_ok() {
            "available (sources above show coverage and timing)"
        } else {
            "unavailable; check process permissions and the errors above"
        }
    );
    println!("Terminal navigation may require macOS Automation permission when first used.");
}

fn show_bridge(label: &str, name: &str, build_path: &str) {
    match companion_bridge(name, build_path) {
        Ok(path) => println!("{label:<16} ready · {}", path.display()),
        Err(error) => println!("{label:<16} unavailable · {error}"),
    }
}

fn timed_observe(runtime: &impl Runtime) -> (Result<usize, String>, u128) {
    let started = Instant::now();
    let result = runtime.observe().map(|agents| agents.len());
    (result, started.elapsed().as_millis())
}

fn show_probe(label: &str, probe: &(Result<usize, String>, u128)) {
    match &probe.0 {
        Ok(count) => println!("{label:<16} {count} agent(s) · {} ms", probe.1),
        Err(error) => println!("{label:<16} unavailable · {} ms · {error}", probe.1),
    }
}

fn executable_on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let path = dir.join(program);
            path.is_file()
        })
    })
}
