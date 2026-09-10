//! End-to-end coverage for offline bootstrap and Fleet adaptation.
#![cfg(all(feature = "runtime-template", feature = "fleet"))]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use assert_cmd::cargo::{cargo_bin, cargo_bin_cmd};
use conda_ship::fleet::{Fleet, InstallOptions, RuntimeSpec};
use predicates::prelude::*;
use rattler_conda_types::{Platform, compression_level::CompressionLevel};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const RUNTIME_NAME: &str = "fleet-e2e";
const DELEGATE_NAME: &str = "fleet-e2e-delegate";
const PACKAGE_VERSION: &str = "1.0.0";
const PACKAGE_BUILD: &str = "0";
const CONDARC: &str = "channels: []\nchannel_priority: strict\n";

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn build_delegate_package(root: &Path) -> (PathBuf, String, u64) {
    let package_root = root.join("package");
    let info_dir = package_root.join("info");
    let payload = if cfg!(windows) {
        PathBuf::from(format!("{DELEGATE_NAME}.exe"))
    } else {
        PathBuf::from("bin").join(DELEGATE_NAME)
    };
    let payload_path = package_root.join(&payload);
    std::fs::create_dir_all(&info_dir).unwrap();
    std::fs::create_dir_all(payload_path.parent().unwrap()).unwrap();

    std::fs::copy(cargo_bin!("cs"), &payload_path).unwrap();

    let payload_bytes = std::fs::read(&payload_path).unwrap();
    let payload_sha256 = hex(Sha256::digest(&payload_bytes));
    std::fs::write(
        info_dir.join("index.json"),
        serde_json::to_vec(&serde_json::json!({
            "name": DELEGATE_NAME,
            "version": PACKAGE_VERSION,
            "build": PACKAGE_BUILD,
            "build_number": 0,
            "depends": [],
            "subdir": Platform::current().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        info_dir.join("paths.json"),
        serde_json::to_vec(&serde_json::json!({
            "paths": [{
                "_path": payload.to_string_lossy().replace('\\', "/"),
                "path_type": "hardlink",
                "sha256": payload_sha256,
                "size_in_bytes": payload_bytes.len(),
            }],
            "paths_version": 1,
        }))
        .unwrap(),
    )
    .unwrap();

    let package_stem = format!("{DELEGATE_NAME}-{PACKAGE_VERSION}-{PACKAGE_BUILD}");
    let archive = root.join(format!("{package_stem}.conda"));
    let paths = [
        info_dir.join("index.json"),
        info_dir.join("paths.json"),
        payload_path,
    ];
    rattler_package_streaming::write::write_conda_package(
        File::create(&archive).unwrap(),
        &package_root,
        &paths,
        CompressionLevel::Lowest,
        Some(1),
        &package_stem,
        None,
        None,
    )
    .unwrap();

    let archive_bytes = std::fs::read(&archive).unwrap();
    (
        archive,
        hex(Sha256::digest(&archive_bytes)),
        archive_bytes.len() as u64,
    )
}

fn write_project(root: &Path, archive: &Path, sha256: &str, size: u64) {
    std::fs::write(
        root.join("conda.toml"),
        format!(
            r#"
[tool.conda-ship]
source-environment = "default"
runtime-name = "{RUNTIME_NAME}"
runtime-version = "{PACKAGE_VERSION}"
delegate-executable = "{DELEGATE_NAME}"
artifact-layout = "online"
condarc-file = "runtime.condarc"
freeze-base = true
"#
        ),
    )
    .unwrap();
    std::fs::write(root.join("runtime.condarc"), CONDARC).unwrap();

    let platform = Platform::current();
    let url = reqwest::Url::from_file_path(archive).unwrap();
    std::fs::write(
        root.join("conda.lock"),
        format!(
            r#"---
version: 6
environments:
  default:
    channels: []
    packages:
      {platform}:
        - conda: {url}
packages:
  - conda: {url}
    sha256: {sha256}
    size: {size}
    subdir: {platform}
"#
        ),
    )
    .unwrap();
}

#[track_caller]
fn assert_policy(prefix: &Path) {
    assert_eq!(
        std::fs::read_to_string(prefix.join(".condarc")).unwrap(),
        CONDARC
    );
    assert!(prefix.join("conda-meta").join("frozen").is_file());
}

#[test]
fn test_stamped_runtime_installs_through_fleet() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    temp_env::with_var("RATTLER_CACHE_DIR", Some(cache.as_os_str()), || {
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let (archive, sha256, size) = build_delegate_package(tmp.path());
        write_project(&project, &archive, &sha256, size);

        let out_dir = tmp.path().join("dist");
        cargo_bin_cmd!("cs")
            .arg("build")
            .arg("--root")
            .arg(&project)
            .arg("--template")
            .arg(cargo_bin!("cs-template"))
            .arg("--out-dir")
            .arg(&out_dir)
            .assert()
            .success();
        let artifact = out_dir.join(format!("{RUNTIME_NAME}{}", std::env::consts::EXE_SUFFIX));

        let direct_prefix = tmp.path().join("direct-prefix");
        assert_cmd::Command::new(&artifact)
            .env("CONDA_SHIP_PREFIX", &direct_prefix)
            .env("FLEET_E2E_OFFLINE", "1")
            .arg("--help")
            .timeout(Duration::from_secs(120))
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "Build ready-to-run conda runtimes",
            ));
        assert_policy(&direct_prefix);

        let spec = RuntimeSpec::from_stamped_artifact(&artifact).unwrap();

        let installed = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(Fleet::new(tmp.path().join("fleet")).install(
                spec,
                InstallOptions {
                    offline: true,
                    ..InstallOptions::default()
                },
            ))
            .unwrap();
        assert_policy(&installed.prefix);

        let runtime_command = installed.command(DELEGATE_NAME).unwrap();
        assert_cmd::Command::new(runtime_command.executable)
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "Build ready-to-run conda runtimes",
            ));
    });
}

#[test]
fn test_stamped_runtime_bootstraps_from_offline_bundle_and_reuses_installation() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let (archive, sha256, size) = build_delegate_package(tmp.path());
    write_project(&project, &archive, &sha256, size);
    let out_dir = tmp.path().join("dist");
    cargo_bin_cmd!("cs")
        .arg("build")
        .arg("--root")
        .arg(&project)
        .arg("--template")
        .arg(cargo_bin!("cs-template"))
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();

    let artifact = out_dir.join(format!("{RUNTIME_NAME}{}", std::env::consts::EXE_SUFFIX));
    let prefix = tmp.path().join("prefix");
    let cache = tmp.path().join("cache");
    let mut command = assert_cmd::Command::new(&artifact);
    command
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("FLEET_E2E_BUNDLE", tmp.path())
        .env("FLEET_E2E_OFFLINE", "1")
        .env("RATTLER_CACHE_DIR", &cache)
        .arg("--help")
        .timeout(Duration::from_secs(120));
    command.assert().success().stdout(predicate::str::contains(
        "Build ready-to-run conda runtimes",
    ));

    let delegate = if cfg!(windows) {
        prefix.join(format!("{DELEGATE_NAME}.exe"))
    } else {
        prefix.join("bin").join(DELEGATE_NAME)
    };
    assert!(delegate.is_file());
    let metadata_path = prefix.join(format!(".{RUNTIME_NAME}.json"));
    let metadata_bytes = std::fs::read(&metadata_path).unwrap();
    let metadata: serde_json::Value = serde_json::from_slice(&metadata_bytes).unwrap();
    assert_eq!(metadata["display_name"], RUNTIME_NAME);
    assert_eq!(metadata["delegate_executable"], DELEGATE_NAME);
    assert_eq!(metadata["packages"], serde_json::json!([DELEGATE_NAME]));
    let history_path = prefix.join("conda-meta").join("history");
    let history = std::fs::read(&history_path).unwrap();
    assert!(!prefix.join(".conda-ship-bootstrap.json").exists());
    assert_policy(&prefix);

    std::fs::remove_file(&archive).unwrap();
    std::fs::remove_dir_all(&cache).unwrap();

    command.assert().success().stdout(predicate::str::contains(
        "Build ready-to-run conda runtimes",
    ));
    assert_eq!(std::fs::read(metadata_path).unwrap(), metadata_bytes);
    assert_eq!(std::fs::read(history_path).unwrap(), history);
    assert!(!cache.exists());
    assert_policy(&prefix);
}
