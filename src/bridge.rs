//! Find the macOS scripting helpers beside a release binary or in a source build.

use std::path::{Path, PathBuf};

pub fn resolve_bridge(
    executable: &Path,
    bundled_name: &str,
    build_path: &Path,
) -> Result<PathBuf, String> {
    if let Some(directory) = executable.parent() {
        let bundled = directory.join(bundled_name);
        if bundled.is_file() {
            return Ok(bundled);
        }
    }
    if build_path.is_file() {
        return Ok(build_path.to_owned());
    }
    Err(format!(
        "Missing {bundled_name} beside am2; install all files from the release archive together"
    ))
}

pub fn companion_bridge(bundled_name: &str, build_path: &str) -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Cannot locate AM2 executable: {error}"))?;
    resolve_bridge(&executable, bundled_name, Path::new(build_path))
}
