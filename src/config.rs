//! User-editable preferences. Live session data and terminal links remain in SQLite.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;

const TEMPLATE: &str = "# AM2 preferences. Restart AM2 after editing this file.\n\
[dashboard]\n\
# Allowed range: 1 to 300 seconds.\n\
refresh_interval_seconds = 5\n\n\
[launch]\n\
# auto uses Herdr when available, otherwise Ghostty.\n\
# Options: auto, herdr, ghostty, terminal, iterm2.\n\
default_terminal = \"auto\"\n";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub dashboard: DashboardConfig,
    pub launch: LaunchConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardConfig {
    pub refresh_interval_seconds: u64,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            refresh_interval_seconds: 5,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchConfig {
    pub default_terminal: LaunchDestination,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LaunchDestination {
    #[default]
    Auto,
    Herdr,
    Ghostty,
    Terminal,
    ITerm2,
}

impl LaunchDestination {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Herdr => "herdr",
            Self::Ghostty => "ghostty",
            Self::Terminal => "terminal",
            Self::ITerm2 => "iterm2",
        }
    }
}

impl AppConfig {
    pub fn default_path() -> Result<PathBuf, String> {
        if let Some(path) = std::env::var_os("AM2_CONFIG_FILE") {
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
        let home = std::env::var_os("HOME")
            .ok_or_else(|| "HOME is required to locate the AM2 config file".to_owned())?;
        Ok(PathBuf::from(home).join("Library/Application Support/AM2/config.toml"))
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let config: Self =
            toml::from_str(&contents).map_err(|error| format!("{}: {error}", path.display()))?;
        if !(1..=300).contains(&config.dashboard.refresh_interval_seconds) {
            return Err(format!(
                "{}: dashboard.refresh_interval_seconds must be between 1 and 300",
                path.display()
            ));
        }
        Ok(config)
    }

    pub fn load_default() -> Result<Self, String> {
        Self::load(Self::default_path()?)
    }

    pub fn init(path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        file.write_all(TEMPLATE.as_bytes())
            .map_err(|error| format!("{}: {error}", path.display()))
    }
}
