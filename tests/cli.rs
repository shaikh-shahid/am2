use std::process::Command;

fn am2(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_am2"))
        .args(args)
        .output()
        .expect("run am2")
}

#[test]
fn help_lists_beta_maintenance_commands() {
    let output = am2(&["help"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("doctor"));
    assert!(text.contains("update"));
    assert!(text.contains("list"));
    assert!(text.contains("config"));
}

#[test]
fn config_init_creates_file_and_does_not_overwrite_it() {
    let path = std::env::temp_dir().join(format!(
        "am2-cli-config-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let first = Command::new(env!("CARGO_BIN_EXE_am2"))
        .args(["config", "init"])
        .env("AM2_CONFIG_FILE", &path)
        .output()
        .unwrap();
    assert!(first.status.success());
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("default_terminal"));
    let second = Command::new(env!("CARGO_BIN_EXE_am2"))
        .args(["config", "init"])
        .env("AM2_CONFIG_FILE", &path)
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn update_explains_the_current_release_channel_without_installing() {
    let output = am2(&["update"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(env!("CARGO_PKG_VERSION")));
    assert!(text.contains("GitHub Releases"));
    assert!(text.contains("cargo run --release --locked"));
}

#[test]
fn doctor_reports_required_tools_and_inventory_result() {
    let output = am2(&["doctor"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("macOS"));
    assert!(text.contains("ps"));
    assert!(text.contains("lsof"));
    assert!(text.contains("Config"));
    assert!(text.contains("Ghostty bridge"));
    assert!(text.contains("Terminal bridge"));
    assert!(text.contains("Live inventory"));
}
