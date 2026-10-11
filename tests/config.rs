use std::fs;

use am2::config::{AppConfig, LaunchDestination};

fn temp_config_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "am2-config-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn missing_file_preserves_current_defaults() {
    let path = temp_config_path();
    let config = AppConfig::load(&path).unwrap();
    assert_eq!(config.dashboard.refresh_interval_seconds, 5);
    assert_eq!(config.launch.default_terminal, LaunchDestination::Auto);
}

#[test]
fn reads_preferences_and_rejects_unknown_or_invalid_values() {
    let path = temp_config_path();
    fs::write(
        &path,
        "[dashboard]\nrefresh_interval_seconds = 12\n[launch]\ndefault_terminal = 'iterm2'\n",
    )
    .unwrap();
    let config = AppConfig::load(&path).unwrap();
    assert_eq!(config.dashboard.refresh_interval_seconds, 12);
    assert_eq!(config.launch.default_terminal, LaunchDestination::ITerm2);

    fs::write(&path, "[dashboard]\nrefresh_interval_seconds = 0\n").unwrap();
    assert!(AppConfig::load(&path)
        .unwrap_err()
        .contains("refresh_interval_seconds"));

    fs::write(&path, "[launch]\ndefualt_terminal = 'ghostty'\n").unwrap();
    assert!(AppConfig::load(&path)
        .unwrap_err()
        .contains("defualt_terminal"));
    fs::remove_file(path).unwrap();
}

#[test]
fn init_creates_editable_file_without_overwriting_it() {
    let path = temp_config_path();
    AppConfig::init(&path).unwrap();
    let initial = fs::read_to_string(&path).unwrap();
    assert!(initial.contains("refresh_interval_seconds = 5"));
    assert!(initial.contains("default_terminal = \"auto\""));
    assert!(AppConfig::load(&path).is_ok());
    assert!(AppConfig::init(&path).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), initial);
    fs::remove_file(path).unwrap();
}
