use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::Deserialize;

const FALLBACK_REPOSITORY: &str = "https://github.com/shaikh-shahid/am2";
const BINARIES: [&str; 3] = ["am2", "am2-ghostty-bridge", "am2-terminal-bridge"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub assets: Vec<String>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    draft: bool,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
}

pub fn latest_release(json: &str) -> Result<Release, String> {
    let releases: Vec<ApiRelease> = serde_json::from_str(json)
        .map_err(|error| format!("Could not read GitHub releases response: {error}"))?;
    releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let version = release
                .tag_name
                .strip_prefix('v')
                .and_then(|tag| Version::parse(tag).ok())?;
            Some(Release {
                tag: release.tag_name,
                version,
                assets: release.assets.into_iter().map(|asset| asset.name).collect(),
            })
        })
        .max_by(|left, right| left.version.cmp(&right.version))
        .ok_or_else(|| "No published AM2 release with a valid version tag was found".into())
}

pub fn archive_name(tag: &str, target: &str) -> String {
    format!("am2-{tag}-{target}.tar.gz")
}

pub fn release_api_url(repository: &str) -> Result<String, String> {
    let path = repository
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")
        .ok_or_else(|| {
            "AM2 release repository must be an https://github.com/owner/repo URL".to_owned()
        })?;
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        })
    {
        return Err("AM2 release repository URL is invalid".into());
    }
    Ok(format!(
        "https://api.github.com/repos/{}/{}/releases?per_page=100",
        parts[0], parts[1]
    ))
}

pub fn run(check_only: bool, assume_yes: bool) -> Result<(), String> {
    let repository = option_env!("AM2_RELEASE_REPO").unwrap_or(FALLBACK_REPOSITORY);
    let api_url = std::env::var("AM2_UPDATE_API_URL").unwrap_or(release_api_url(repository)?);
    let releases_json =
        curl_capture(&api_url).map_err(|error| format!("Update check failed: {error}"))?;
    let release = latest_release(&releases_json)?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| format!("AM2 has an invalid built-in version: {error}"))?;

    if release.version <= current {
        println!(
            "AM2 {} is up to date (latest release: {}).",
            current, release.tag
        );
        return Ok(());
    }

    println!(
        "AM2 {} is available (installed: {}).",
        release.version, current
    );
    println!("Release: {repository}/releases/tag/{}", release.tag);
    if check_only {
        return Ok(());
    }

    if option_env!("AM2_RELEASE_REPO").is_none() {
        println!("This source build cannot update itself. Install the release with the command in the README.");
        return Ok(());
    }
    let target = macos_target()?;
    let archive = archive_name(&release.tag, target);
    if !release.assets.iter().any(|asset| asset == &archive)
        || !release.assets.iter().any(|asset| asset == "SHA256SUMS")
    {
        return Err(format!(
            "Release {} is missing the {} archive or SHA256SUMS",
            release.tag, target
        ));
    }
    if !assume_yes && !confirm_update(&release.tag)? {
        println!("Update cancelled.");
        return Ok(());
    }

    install_release(repository, &release.tag, &archive)?;
    println!("Updated AM2 to {}.", release.version);
    Ok(())
}

fn confirm_update(tag: &str) -> Result<bool, String> {
    if !io::stdin().is_terminal() {
        println!("Run `am2 update --yes` to install without a prompt.");
        return Ok(false);
    }
    print!("Install AM2 {tag}? [y/N] ");
    io::stdout()
        .flush()
        .map_err(|error| format!("Could not write update prompt: {error}"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("Could not read update confirmation: {error}"))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn macos_target() -> Result<&'static str, String> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("aarch64-apple-darwin"),
        "x86_64" => Ok("x86_64-apple-darwin"),
        arch => Err(format!(
            "AM2 updates are not available for macOS architecture {arch}"
        )),
    }
}

fn install_release(repository: &str, tag: &str, archive: &str) -> Result<(), String> {
    let repository = repository.trim_end_matches('/');
    let base_url = format!("{repository}/releases/download/{tag}");
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the installed AM2 executable: {error}"))?;
    let install_dir = executable
        .parent()
        .ok_or_else(|| "Could not find the installed AM2 directory".to_owned())?;
    install_release_from_base(&base_url, archive, install_dir)
}

fn install_release_from_base(
    base_url: &str,
    archive: &str,
    install_dir: &Path,
) -> Result<(), String> {
    let temporary = TempDir::create("am2-update")?;
    let archive_path = temporary.path.join(archive);
    let sums_path = temporary.path.join("SHA256SUMS");
    download(&format!("{base_url}/{archive}"), &archive_path)?;
    download(&format!("{base_url}/SHA256SUMS"), &sums_path)?;
    verify_archive(&temporary.path, archive)?;

    let unpacked = temporary.path.join("unpacked");
    fs::create_dir(&unpacked)
        .map_err(|error| format!("Could not create update staging area: {error}"))?;
    let extraction = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive_path)
        .arg("-C")
        .arg(&unpacked)
        .args(BINARIES)
        .output()
        .map_err(|error| format!("Could not run tar to unpack the update: {error}"))?;
    if !extraction.status.success() {
        return Err(format!(
            "Could not unpack update: {}",
            output_message(&extraction.stderr)
        ));
    }
    for name in BINARIES {
        let path = unpacked.join(name);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("Update archive is missing {name}: {error}"))?;
        if !metadata.file_type().is_file() {
            return Err(format!("Update archive entry {name} is not a regular file"));
        }
    }

    replace_binaries(&unpacked, install_dir)?;
    Ok(())
}

fn verify_archive(directory: &Path, archive: &str) -> Result<(), String> {
    let sums = fs::read_to_string(directory.join("SHA256SUMS"))
        .map_err(|error| format!("Could not read release checksums: {error}"))?;
    let expected = sums
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let name = fields.next()?.trim_start_matches('*');
            (name == archive).then_some(hash)
        })
        .collect::<Vec<_>>();
    if expected.len() != 1
        || expected[0].len() != 64
        || !expected[0].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!(
            "SHA256SUMS does not contain one valid checksum for {archive}"
        ));
    }
    fs::write(
        directory.join("CHECKSUM"),
        format!("{}  {archive}\n", expected[0]),
    )
    .map_err(|error| format!("Could not stage release checksum: {error}"))?;
    let check = Command::new("shasum")
        .args(["-a", "256", "-c", "CHECKSUM"])
        .current_dir(directory)
        .output()
        .map_err(|error| format!("Could not run shasum to verify the update: {error}"))?;
    if !check.status.success() {
        return Err(format!(
            "Update checksum verification failed: {}",
            output_message(&check.stdout)
        ));
    }
    Ok(())
}

fn download(url: &str, destination: &Path) -> Result<(), String> {
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--show-error",
            "--retry",
            "3",
            "--connect-timeout",
            "15",
            "--max-time",
            "1800",
            "--progress-bar",
            "--output",
        ])
        .arg(destination)
        .arg(url)
        .status()
        .map_err(|error| format!("Could not run curl to download the update: {error}"))?;
    if !status.success() {
        return Err(format!(
            "Download failed for {url} (curl exited with {status})"
        ));
    }
    Ok(())
}

fn curl_capture(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--retry",
            "2",
            "--connect-timeout",
            "15",
            "--max-time",
            "60",
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            concat!("User-Agent: AM2/", env!("CARGO_PKG_VERSION")),
        ])
        .arg(url)
        .output()
        .map_err(|error| format!("Could not run curl to check for updates: {error}"))?;
    if !output.status.success() {
        return Err(output_message(&output.stderr));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("GitHub response was not UTF-8: {error}"))
}

fn output_message(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim()
        .chars()
        .take(500)
        .collect()
}

fn replace_binaries(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::metadata(destination).map_err(|error| {
        format!(
            "Could not access AM2 install directory {}: {error}",
            destination.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!(
            "AM2 install path is not a directory: {}",
            destination.display()
        ));
    }
    let stage = TempDir::inside(destination, "am2-update-stage")?;
    let backup = stage.path.join("backup");
    let staged = stage.path.join("new");
    fs::create_dir(&backup)
        .map_err(|error| format!("Could not create update backup directory: {error}"))?;
    fs::create_dir(&staged)
        .map_err(|error| format!("Could not create update staging directory: {error}"))?;
    for name in BINARIES {
        let source_path = source.join(name);
        let metadata = fs::symlink_metadata(&source_path)
            .map_err(|error| format!("Update archive is missing {name}: {error}"))?;
        if !metadata.file_type().is_file() {
            return Err(format!("Update archive entry {name} is not a regular file"));
        }
        let staged_path = staged.join(name);
        fs::copy(&source_path, &staged_path)
            .map_err(|error| format!("Could not stage {name}: {error}"))?;
        set_executable(&staged_path)?;
    }

    let mut backed_up = Vec::new();
    let mut installed = Vec::new();
    for name in BINARIES {
        let destination_path = destination.join(name);
        match fs::symlink_metadata(&destination_path) {
            Ok(metadata) => {
                if !metadata.file_type().is_file() {
                    rollback(destination, &backup, &installed, &backed_up);
                    return Err(format!(
                        "Refusing to replace non-file at {}",
                        destination_path.display()
                    ));
                }
                if let Err(error) = fs::rename(&destination_path, backup.join(name)) {
                    rollback(destination, &backup, &installed, &backed_up);
                    return Err(format!("Could not back up {name}: {error}"));
                }
                backed_up.push(name);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                rollback(destination, &backup, &installed, &backed_up);
                return Err(format!("Could not inspect {name}: {error}"));
            }
        }
        if let Err(error) = fs::rename(staged.join(name), &destination_path) {
            rollback(destination, &backup, &installed, &backed_up);
            return Err(format!("Could not install {name}: {error}"));
        }
        installed.push(name);
    }
    Ok(())
}

fn rollback(destination: &Path, backup: &Path, installed: &[&str], backed_up: &[&str]) {
    for name in installed.iter().rev() {
        let _ = fs::remove_file(destination.join(name));
    }
    for name in backed_up.iter().rev() {
        let _ = fs::rename(backup.join(name), destination.join(name));
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("Could not inspect staged executable: {error}"))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("Could not mark staged file executable: {error}"))
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn create(prefix: &str) -> Result<Self, String> {
        Self::create_at(&std::env::temp_dir(), prefix)
    }

    fn inside(parent: &Path, prefix: &str) -> Result<Self, String> {
        Self::create_at(parent, prefix)
    }

    fn create_at(parent: &Path, prefix: &str) -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = parent.join(format!(".{prefix}-{}-{nonce}", std::process::id()));
        fs::create_dir(&path)
            .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree() -> (TempDir, PathBuf, PathBuf) {
        let root = TempDir::create("am2-updater-test").unwrap();
        let source = root.path.join("source");
        let destination = root.path.join("install");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        (root, source, destination)
    }

    #[test]
    fn replaces_the_cli_and_both_helpers_from_staged_files() {
        let (_root, source, destination) = make_tree();
        for name in BINARIES {
            fs::write(source.join(name), format!("new {name}")).unwrap();
            fs::write(destination.join(name), format!("old {name}")).unwrap();
        }

        replace_binaries(&source, &destination).unwrap();

        for name in BINARIES {
            assert_eq!(
                fs::read_to_string(destination.join(name)).unwrap(),
                format!("new {name}")
            );
        }
    }

    #[test]
    fn failed_replacement_restores_every_previously_installed_binary() {
        let (_root, source, destination) = make_tree();
        for name in BINARIES {
            fs::write(source.join(name), format!("new {name}")).unwrap();
            if name == "am2-ghostty-bridge" {
                fs::create_dir(destination.join(name)).unwrap();
            } else {
                fs::write(destination.join(name), format!("old {name}")).unwrap();
            }
        }

        assert!(replace_binaries(&source, &destination).is_err());
        assert_eq!(
            fs::read_to_string(destination.join("am2")).unwrap(),
            "old am2"
        );
        assert!(destination.join("am2-ghostty-bridge").is_dir());
        assert_eq!(
            fs::read_to_string(destination.join("am2-terminal-bridge")).unwrap(),
            "old am2-terminal-bridge"
        );
    }

    #[test]
    fn checksum_verification_rejects_an_archive_that_changed_after_download() {
        let root = TempDir::create("am2-updater-checksum-test").unwrap();
        let archive = "am2-v0.1.2-aarch64-apple-darwin.tar.gz";
        let archive_path = root.path.join(archive);
        fs::write(&archive_path, b"release archive").unwrap();
        let hash_output = Command::new("shasum")
            .args(["-a", "256", archive])
            .current_dir(&root.path)
            .output()
            .unwrap();
        assert!(hash_output.status.success());
        let hash = String::from_utf8(hash_output.stdout).unwrap();
        fs::write(root.path.join("SHA256SUMS"), hash).unwrap();
        verify_archive(&root.path, archive).unwrap();

        fs::write(&archive_path, b"modified archive").unwrap();
        assert!(verify_archive(&root.path, archive).is_err());
    }

    #[test]
    fn local_release_download_verifies_and_installs_all_three_files() {
        let (_root, source, destination) = make_tree();
        for name in BINARIES {
            fs::write(source.join(name), format!("new {name}")).unwrap();
            fs::write(destination.join(name), format!("old {name}")).unwrap();
        }
        let release_dir = source.join("release");
        fs::create_dir(&release_dir).unwrap();
        let archive = "am2-v0.1.2-aarch64-apple-darwin.tar.gz";
        let archive_path = release_dir.join(archive);
        let package = Command::new("tar")
            .args(["-czf"])
            .arg(&archive_path)
            .arg("-C")
            .arg(&source)
            .args(BINARIES)
            .status()
            .unwrap();
        assert!(package.success());
        let checksum = Command::new("shasum")
            .args(["-a", "256", archive])
            .current_dir(&release_dir)
            .output()
            .unwrap();
        assert!(checksum.status.success());
        fs::write(release_dir.join("SHA256SUMS"), checksum.stdout).unwrap();

        let base_url = format!("file://{}", release_dir.display());
        install_release_from_base(&base_url, archive, &destination).unwrap();

        for name in BINARIES {
            assert_eq!(
                fs::read_to_string(destination.join(name)).unwrap(),
                format!("new {name}")
            );
        }
    }
}
