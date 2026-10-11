use std::fs;

use am2::bridge::resolve_bridge;

fn temp_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "am2-bridge-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn bundled_helper_takes_precedence_over_build_output() {
    let dir = temp_dir();
    let executable = dir.join("am2");
    let bundled = dir.join("am2-ghostty-bridge");
    let build = dir.join("build-helper");
    fs::write(&bundled, "bundled").unwrap();
    fs::write(&build, "build").unwrap();
    assert_eq!(
        resolve_bridge(&executable, "am2-ghostty-bridge", &build).unwrap(),
        bundled
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_build_can_use_helper_in_build_directory() {
    let dir = temp_dir();
    let build = dir.join("build-helper");
    fs::write(&build, "build").unwrap();
    assert_eq!(
        resolve_bridge(&dir.join("am2"), "am2-terminal-bridge", &build).unwrap(),
        build
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn missing_companion_reports_installation_problem() {
    let dir = temp_dir();
    let error = resolve_bridge(
        &dir.join("am2"),
        "am2-terminal-bridge",
        &dir.join("missing"),
    )
    .unwrap_err();
    assert!(error.contains("am2-terminal-bridge"));
    assert!(error.contains("beside am2"));
    fs::remove_dir_all(dir).unwrap();
}
