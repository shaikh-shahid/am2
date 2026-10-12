use am2::updater::{archive_name, latest_release, release_api_url};

#[test]
fn latest_release_uses_semver_and_includes_beta_prereleases() {
    let releases = r#"[
      {"tag_name":"v0.1.10","draft":false,"assets":[{"name":"am2-v0.1.10-aarch64-apple-darwin.tar.gz"}]},
      {"tag_name":"v0.2.0-beta.1","draft":false,"assets":[{"name":"am2-v0.2.0-beta.1-aarch64-apple-darwin.tar.gz"}]},
      {"tag_name":"v9.0.0","draft":true,"assets":[]},
      {"tag_name":"not-a-version","draft":false,"assets":[]}
    ]"#;

    let latest = latest_release(releases).unwrap();
    assert_eq!(latest.tag, "v0.2.0-beta.1");
    assert_eq!(latest.version.to_string(), "0.2.0-beta.1");
    assert!(latest
        .assets
        .iter()
        .any(|asset| asset == "am2-v0.2.0-beta.1-aarch64-apple-darwin.tar.gz"));
}

#[test]
fn archive_names_include_version_and_macos_architecture() {
    assert_eq!(
        archive_name("v0.1.2", "aarch64-apple-darwin"),
        "am2-v0.1.2-aarch64-apple-darwin.tar.gz"
    );
}

#[test]
fn release_repository_maps_to_github_releases_api() {
    assert_eq!(
        release_api_url("https://github.com/shaikh-shahid/am2").unwrap(),
        "https://api.github.com/repos/shaikh-shahid/am2/releases?per_page=100"
    );
}

#[test]
fn release_repository_rejects_non_github_urls_and_extra_path_components() {
    assert!(release_api_url("http://github.com/shaikh-shahid/am2").is_err());
    assert!(release_api_url("https://github.com/shaikh-shahid/am2/releases").is_err());
}

#[test]
fn release_list_without_a_published_semver_tag_is_an_error() {
    assert!(latest_release(r#"[{"tag_name":"draft","draft":true,"assets":[]}]"#).is_err());
}
