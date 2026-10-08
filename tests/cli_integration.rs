//! Integration tests for direct stamped-runtime delegation.
#![cfg(feature = "runtime-template")]

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin_cmd;
use fs4::FileExt;
use predicates::prelude::*;
use rattler_conda_types::Platform;
use rstest::rstest;
use tempfile::TempDir;

fn runtime() -> assert_cmd::Command {
    let mut cmd = cargo_bin_cmd!("cs-template");
    cmd.env("CONDA_SHIP_ALLOW_UNSTAMPED_TEMPLATE", "1");
    cmd
}

fn runtime_at(prefix: &Path) -> assert_cmd::Command {
    let mut cmd = runtime();
    cmd.env("CS_TEMPLATE_PREFIX", prefix);
    cmd
}

fn write_demo_runtime_metadata(prefix: &Path) {
    std::fs::write(
        prefix.join(".demo.json"),
        r#"{"schema_version":1,"display_name":"demo","install_name":"demo","metadata_file":".demo.json","version":"test","channels":[],"packages":[]}"#,
    )
    .unwrap();
}

fn build_stamped_runtime(tmp: &TempDir, delegate: &str) -> PathBuf {
    build_named_stamped_runtime(tmp, "demo", delegate)
}

fn build_named_stamped_runtime(tmp: &TempDir, runtime_name: &str, delegate: &str) -> PathBuf {
    let root = env!("CARGO_MANIFEST_DIR");
    let template = assert_cmd::cargo::cargo_bin!("cs-template");
    let out_dir = tmp.path().join("dist");

    cargo_bin_cmd!("cs")
        .env("CONDA_SHIP_TEMPLATE", template)
        .args([
            "build",
            "--root",
            root,
            "--runtime-name",
            runtime_name,
            "--delegate-executable",
            delegate,
            "--runtime-version",
            "9.8.7",
            "--out-dir",
            out_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    out_dir.join(if cfg!(windows) {
        format!("{runtime_name}.exe")
    } else {
        runtime_name.to_string()
    })
}

fn install_cs_delegate(prefix: &Path) {
    let destination = if cfg!(windows) {
        prefix.join("cs.exe")
    } else {
        prefix.join("bin").join("cs")
    };
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::copy(assert_cmd::cargo::cargo_bin!("cs"), &destination).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn hold_runtime_update_lock(prefix: &Path) -> File {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(prefix.join(".demo.update.lock"))
        .unwrap();
    FileExt::lock(&lock).unwrap();
    lock
}

fn record_installation(
    runtime: &Path,
    prefix: &Path,
    ownership: &str,
    installation: &str,
    executable: &Path,
    instruction: Option<&str>,
) -> std::process::Output {
    let mut command = Command::new(runtime);
    command
        .env("CONDA_SHIP_PREFIX", prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/record-installation")
        .env("CONDA_SHIP_INTERNAL_UPDATE_OWNERSHIP", ownership)
        .env("CONDA_SHIP_INTERNAL_UPDATE_INSTALLATION", installation)
        .env("CONDA_SHIP_INTERNAL_UPDATE_EXECUTABLE", executable);
    if let Some(instruction) = instruction {
        command.env("CONDA_SHIP_INTERNAL_UPDATE_INSTRUCTION", instruction);
    }
    command.output().unwrap()
}

#[test]
fn test_runtime_template_refuses_to_run_without_stamp() {
    cargo_bin_cmd!("cs-template")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "runtime template, not a runnable conda runtime",
        ));
}

#[rstest]
#[case(&["--help"])]
#[case(&["--version"])]
#[case(&["bootstrap"])]
#[case(&["status"])]
#[case(&["shell"])]
#[case(&["uninstall"])]
fn test_runtime_does_not_parse_delegate_arguments(#[case] args: &[&str]) {
    let tmp = TempDir::new().unwrap();
    runtime_at(tmp.path())
        .args(args)
        .assert()
        .failure()
        .stderr(predicate::str::contains("runtime has no stamped lockfile"));
}

#[test]
fn test_runtime_refuses_an_unmanaged_existing_prefix() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("conda-meta")).unwrap();

    runtime_at(tmp.path())
        .arg("--help")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unmanaged install path"));
}

#[test]
fn test_runtime_bundle_env_var_rejects_missing_directory() {
    let tmp = TempDir::new().unwrap();
    runtime_at(tmp.path())
        .env("CS_TEMPLATE_BUNDLE", tmp.path().join("missing"))
        .arg("info")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "configured bundle path is not a directory",
        ));
}

#[rstest]
#[case::set_to_1("1", true)]
#[case::set_to_true("true", true)]
#[case::set_to_0("0", false)]
#[case::set_to_false("false", false)]
#[case::empty("", false)]
fn test_runtime_offline_env_var_parsing(#[case] value: &str, #[case] offline: bool) {
    let tmp = TempDir::new().unwrap();
    let expected = if offline {
        "offline bootstrap requires a stamped runtime lock"
    } else {
        "runtime has no stamped lockfile"
    };

    runtime_at(tmp.path())
        .env("CS_TEMPLATE_OFFLINE", value)
        .arg("info")
        .assert()
        .failure()
        .stderr(predicate::str::contains(expected));
}

#[rstest]
#[case::direct(false)]
#[case::build_and_run(true)]
fn test_stamped_runtime_delegates_help_to_configured_executable(#[case] run: bool) {
    let tmp = TempDir::new().unwrap();
    let prefix = tmp.path().join("prefix");
    std::fs::create_dir_all(prefix.join("conda-meta")).unwrap();
    write_demo_runtime_metadata(&prefix);
    install_cs_delegate(&prefix);

    let mut command = if run {
        let mut command = cargo_bin_cmd!("cs");
        command
            .env(
                "CONDA_SHIP_TEMPLATE",
                assert_cmd::cargo::cargo_bin!("cs-template"),
            )
            .env("DEMO_PREFIX", tmp.path())
            .args([
                "run",
                "--root",
                env!("CARGO_MANIFEST_DIR"),
                "--runtime-name",
                "demo",
                "--delegate-executable",
                "cs",
                "--runtime-version",
                "9.8.7",
            ])
            .arg("--out-dir")
            .arg(tmp.path().join("dist"))
            .arg("--install-path")
            .arg(&prefix)
            .arg("--");
        command
    } else {
        let binary = build_stamped_runtime(&tmp, "cs");
        let mut command = assert_cmd::Command::new(binary);
        command.env("DEMO_PREFIX", &prefix);
        command
    };
    command
        .env_remove("CONDA_SHIP_PREFIX")
        .env("DEMO_OFFLINE", "1")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Build ready-to-run conda runtimes",
        ));
}

#[test]
fn test_conda_ship_prefix_takes_precedence_over_runtime_specific_prefix() {
    let tmp = TempDir::new().unwrap();
    let binary = build_stamped_runtime(&tmp, "cs");
    let managed_prefix = tmp.path().join("managed-root");
    let compatibility_prefix = tmp.path().join("compatibility-root");
    std::fs::create_dir_all(managed_prefix.join("conda-meta")).unwrap();
    std::fs::create_dir_all(compatibility_prefix.join("conda-meta")).unwrap();
    write_demo_runtime_metadata(&managed_prefix);
    install_cs_delegate(&managed_prefix);

    assert_cmd::Command::new(binary)
        .env("CONDA_SHIP_PREFIX", &managed_prefix)
        .env("DEMO_PREFIX", &compatibility_prefix)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Build ready-to-run conda runtimes",
        ));
}

#[test]
fn test_runtime_named_conda_does_not_use_active_conda_prefix() {
    let tmp = TempDir::new().unwrap();
    let binary = build_named_stamped_runtime(&tmp, "conda", "cs");
    let active_prefix = tmp.path().join("active-environment");
    std::fs::create_dir_all(active_prefix.join("conda-meta")).unwrap();
    std::fs::write(
        active_prefix.join(".conda.json"),
        r#"{"schema_version":1,"display_name":"conda","install_name":"conda","metadata_file":".conda.json","version":"9.8.7","delegate_executable":"cs","channels":[],"packages":[]}"#,
    )
    .unwrap();
    install_cs_delegate(&active_prefix);

    assert_cmd::Command::new(binary)
        .env("CONDA_PREFIX", &active_prefix)
        .env("CONDA_BUNDLE", tmp.path().join("missing-bundle"))
        .arg("--help")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "configured bundle path is not a directory",
        ));
}

#[test]
fn test_conda_ship_prefix_selects_managed_root_for_runtime_named_conda() {
    let tmp = TempDir::new().unwrap();
    let binary = build_named_stamped_runtime(&tmp, "conda", "cs");
    let managed_prefix = tmp.path().join("managed-root");
    let active_prefix = tmp.path().join("active-environment");
    std::fs::create_dir_all(managed_prefix.join("conda-meta")).unwrap();
    std::fs::write(
        managed_prefix.join(".conda.json"),
        r#"{"schema_version":1,"display_name":"conda","install_name":"conda","metadata_file":".conda.json","version":"9.8.7","channels":[],"packages":[]}"#,
    )
    .unwrap();
    install_cs_delegate(&managed_prefix);

    assert_cmd::Command::new(binary)
        .env("CONDA_SHIP_PREFIX", &managed_prefix)
        .env("CONDA_PREFIX", &active_prefix)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Build ready-to-run conda runtimes",
        ));
}

#[rstest]
#[case("status")]
#[case("shell")]
#[case("uninstall")]
fn test_stamped_runtime_has_no_private_management_subcommands(#[case] arg: &str) {
    let tmp = TempDir::new().unwrap();
    let binary = build_stamped_runtime(&tmp, "cs");
    let prefix = tmp.path().join("prefix");
    std::fs::create_dir_all(prefix.join("conda-meta")).unwrap();
    write_demo_runtime_metadata(&prefix);
    install_cs_delegate(&prefix);

    assert_cmd::Command::new(binary)
        .env("DEMO_PREFIX", &prefix)
        .arg(arg)
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

#[cfg_attr(not(feature = "online_tests"), ignore)]
#[test]
fn test_first_delegate_command_auto_bootstraps_stamped_runtime() {
    let tmp = TempDir::new().unwrap();
    let binary = build_stamped_runtime(&tmp, "conda");
    let prefix = tmp.path().join("prefix");

    assert_cmd::Command::new(binary)
        .env("DEMO_PREFIX", &prefix)
        .args([OsString::from("info"), OsString::from("--json")])
        .timeout(std::time::Duration::from_secs(300))
        .assert()
        .success();

    assert!(prefix.join("conda-meta").is_dir());
    assert!(prefix.join(".demo.json").is_file());
    assert!(!prefix.join(".condarc").exists());
    assert!(!prefix.join("conda-meta").join("frozen").exists());
}

#[test]
fn test_update_package_replaces_the_stable_runtime_from_a_file_channel() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let channel = tmp.path().join("channel");
    let platform = Platform::current().to_string();
    let subdir = channel.join(&platform);
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&subdir).unwrap();

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut manifest = std::fs::read_to_string(root.join("pixi.toml")).unwrap();
    let channel_url = reqwest::Url::from_directory_path(&channel)
        .unwrap()
        .to_string();
    manifest.push_str(&format!(
        "\n[tool.conda-ship.update]\nchannel = {channel_url:?}\npackage = \"demo-runtime\"\nbuild-number = 0\n"
    ));
    std::fs::write(project.join("pixi.toml"), manifest).unwrap();
    std::fs::copy(root.join("pixi.lock"), project.join("pixi.lock")).unwrap();

    let template = assert_cmd::cargo::cargo_bin!("cs-template");
    let first_dir = tmp.path().join("first");
    let second_dir = tmp.path().join("second");
    for (version, out_dir) in [("1.0.0", &first_dir), ("2.0.0", &second_dir)] {
        cargo_bin_cmd!("cs")
            .args([
                "build",
                "--root",
                project.to_str().unwrap(),
                "--runtime-name",
                "demo",
                "--delegate-executable",
                "cs",
                "--runtime-version",
                version,
                "--template",
                template.to_str().unwrap(),
                "--out-dir",
                out_dir.to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    let executable_name = if cfg!(windows) { "demo.exe" } else { "demo" };
    let first = first_dir.join(executable_name);
    let second = second_dir.join(executable_name);
    let info = second_dir.join("demo.info.json");
    let package_output = Command::new(assert_cmd::cargo::cargo_bin!("cs"))
        .args([
            "package-update",
            "--info",
            info.to_str().unwrap(),
            "--binary",
            second.to_str().unwrap(),
            "--out-dir",
            subdir.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        package_output.status.success(),
        "{}",
        String::from_utf8_lossy(&package_output.stderr)
    );
    let package: serde_json::Value = serde_json::from_slice(&package_output.stdout).unwrap();
    let filename = package["filename"].as_str().unwrap();
    let repodata = serde_json::json!({
        "info": {"subdir": platform},
        "packages": {},
        "packages.conda": {
            (filename): {
                "name": "demo-runtime",
                "version": package["runtime_version"],
                "build": package["build_number"].to_string(),
                "build_number": package["build_number"],
                "depends": [],
                "subdir": platform,
                "sha256": package["sha256"],
                "size": package["size"],
            }
        },
        "removed": [],
        "repodata_version": 1,
    });
    std::fs::write(
        subdir.join("repodata.json"),
        serde_json::to_vec(&repodata).unwrap(),
    )
    .unwrap();

    let prefix = tmp.path().join("prefix");
    let install_dir = tmp.path().join("bin");
    let stable = install_dir.join(executable_name);
    std::fs::create_dir_all(prefix.join("conda-meta")).unwrap();
    std::fs::create_dir_all(&install_dir).unwrap();
    std::fs::copy(&first, &stable).unwrap();
    std::fs::write(
        prefix.join(".demo.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "display_name": "demo",
            "install_name": "demo",
            "metadata_file": ".demo.json",
            "version": "1.0.0",
            "delegate_executable": "cs",
            "channels": [],
            "packages": [],
        }))
        .unwrap(),
    )
    .unwrap();
    install_cs_delegate(&prefix);

    assert_cmd::Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .arg("--help")
        .assert()
        .success();
    let recorded = record_installation(&stable, &prefix, "direct", "standalone", &stable, None);
    assert!(
        recorded.status.success(),
        "{}",
        String::from_utf8_lossy(&recorded.stderr)
    );

    let probe = |probe_prefix: &Path| {
        Command::new(&stable)
            .env("CONDA_SHIP_PREFIX", probe_prefix)
            .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/probe")
            .env("CONDA_SHIP_INTERNAL_UPDATE_OFFLINE", "1")
            .output()
            .unwrap()
    };
    let metadata_path = prefix.join(".demo.json");
    let original_metadata = std::fs::read(&metadata_path).unwrap();
    let advisory = probe(&prefix);
    assert!(
        advisory.status.success(),
        "{}",
        String::from_utf8_lossy(&advisory.stderr)
    );
    let advisory: serde_json::Value = serde_json::from_slice(&advisory.stdout).unwrap();
    assert_eq!(advisory["available"], true);
    assert_eq!(advisory["version"], "2.0.0");
    assert_eq!(advisory["source"], "file");
    assert!(advisory["cache_age_seconds"].is_null());
    assert_eq!(std::fs::read(&metadata_path).unwrap(), original_metadata);

    // A stale executable must not announce an update for a different generation.
    let mut newer_metadata: serde_json::Value = serde_json::from_slice(&original_metadata).unwrap();
    newer_metadata["version"] = serde_json::json!("2.0.0");
    let newer_metadata = serde_json::to_vec(&newer_metadata).unwrap();
    std::fs::write(&metadata_path, &newer_metadata).unwrap();
    let unknown = probe(&prefix);
    assert!(unknown.status.success());
    let unknown: serde_json::Value = serde_json::from_slice(&unknown.stdout).unwrap();
    assert!(unknown["available"].is_null());
    assert!(unknown["version"].is_null());
    assert!(unknown["source"].is_null());
    assert_eq!(std::fs::read(&metadata_path).unwrap(), newer_metadata);
    std::fs::write(&metadata_path, &original_metadata).unwrap();

    // Advisory reads must not restore interrupted metadata writes or bootstrap.
    let backup_path = metadata_path.with_extension("bak");
    std::fs::rename(&metadata_path, &backup_path).unwrap();
    assert!(!probe(&prefix).status.success());
    assert!(!metadata_path.exists());
    assert_eq!(std::fs::read(&backup_path).unwrap(), original_metadata);

    std::fs::create_dir(&metadata_path).unwrap();
    let rejected = probe(&prefix);
    assert!(
        !rejected.status.success(),
        "probe accepted a metadata directory"
    );
    assert!(rejected.stdout.is_empty());
    assert!(metadata_path.is_dir());
    assert_eq!(std::fs::read_dir(&metadata_path).unwrap().count(), 0);
    assert_eq!(std::fs::read(&backup_path).unwrap(), original_metadata);
    std::fs::remove_dir(&metadata_path).unwrap();

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&backup_path, &metadata_path).unwrap();
        let rejected = probe(&prefix);
        assert!(
            !rejected.status.success(),
            "probe followed a metadata symlink"
        );
        assert!(rejected.stdout.is_empty());
        assert_eq!(std::fs::read_link(&metadata_path).unwrap(), backup_path);
        assert_eq!(std::fs::read(&backup_path).unwrap(), original_metadata);
        std::fs::remove_file(&metadata_path).unwrap();
    }
    std::fs::rename(&backup_path, &metadata_path).unwrap();

    std::fs::write(&backup_path, &original_metadata).unwrap();
    for (field, value) in [
        ("bootstrap_state", "installing"),
        ("display_name", "another-runtime"),
    ] {
        let mut invalid: serde_json::Value = serde_json::from_slice(&original_metadata).unwrap();
        invalid[field] = serde_json::json!(value);
        let invalid_bytes = serde_json::to_vec(&invalid).unwrap();
        std::fs::write(&metadata_path, &invalid_bytes).unwrap();

        let rejected = probe(&prefix);
        assert!(!rejected.status.success(), "probe accepted invalid {field}");
        assert!(rejected.stdout.is_empty());
        assert_eq!(std::fs::read(&metadata_path).unwrap(), invalid_bytes);
        assert_eq!(std::fs::read(&backup_path).unwrap(), original_metadata);
        assert_eq!(
            std::fs::read(&stable).unwrap(),
            std::fs::read(&first).unwrap()
        );
    }
    std::fs::remove_file(&backup_path).unwrap();
    std::fs::write(&metadata_path, &original_metadata).unwrap();
    let absent_prefix = tmp.path().join("uninitialized");
    assert!(!probe(&absent_prefix).status.success());
    assert!(!absent_prefix.exists());

    let coordinator = hold_runtime_update_lock(&prefix);
    let check = Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/check")
        .env("CONDA_SHIP_INTERNAL_UPDATE_OFFLINE", "1")
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let selected: serde_json::Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(selected["version"], "2.0.0");
    let selected_sha256 = selected["sha256"].as_str().unwrap();

    let stage = Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/stage")
        .env("CONDA_SHIP_INTERNAL_UPDATE_CANDIDATE", selected_sha256)
        .env("CONDA_SHIP_INTERNAL_UPDATE_OFFLINE", "1")
        .output()
        .unwrap();
    assert!(
        stage.status.success(),
        "{}",
        String::from_utf8_lossy(&stage.stderr)
    );
    let staged: serde_json::Value = serde_json::from_slice(&stage.stdout).unwrap();
    assert_eq!(staged, serde_json::json!({"staged": true}));
    assert_eq!(
        std::fs::read(&stable).unwrap(),
        std::fs::read(&first).unwrap()
    );
    let staged_metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prefix.join(".demo.json")).unwrap()).unwrap();
    assert!(staged_metadata["update"].get("version").is_none());
    assert!(staged_metadata["update"].get("provenance").is_none());
    assert!(staged_metadata["update"].get("package-sha256").is_none());
    assert!(staged_metadata["update"].get("package_sha256").is_none());
    let pending = &staged_metadata["update"]["pending"];
    assert_eq!(pending["phase"], "staged");
    assert_eq!(pending["version"], "2.0.0");
    assert_eq!(pending["build-number"], package["build_number"]);
    assert_eq!(pending["executable_sha256"], package["payload_sha256"]);
    assert!(pending.get("candidate").is_none());
    assert!(pending.get("backup").is_none());

    // A notification between commands must leave an unapproved update intact.
    drop(coordinator);
    let staged_metadata_bytes = std::fs::read(&metadata_path).unwrap();
    let staged_path = install_dir.join(format!(
        ".{executable_name}.conda-ship-{}.new",
        &package["payload_sha256"].as_str().unwrap()[..16]
    ));
    let staged_executable = std::fs::read(&staged_path).unwrap();
    let advisory = probe(&prefix);
    assert!(advisory.status.success());
    let advisory: serde_json::Value = serde_json::from_slice(&advisory.stdout).unwrap();
    assert_eq!(advisory["sha256"], selected_sha256);
    assert_eq!(
        std::fs::read(&metadata_path).unwrap(),
        staged_metadata_bytes
    );
    assert_eq!(std::fs::read(&staged_path).unwrap(), staged_executable);
    assert_eq!(
        std::fs::read(&stable).unwrap(),
        std::fs::read(&first).unwrap()
    );
    let coordinator = hold_runtime_update_lock(&prefix);

    let record_while_pending =
        record_installation(&stable, &prefix, "direct", "standalone", &stable, None);
    assert!(!record_while_pending.status.success());
    assert!(
        String::from_utf8_lossy(&record_while_pending.stderr)
            .contains("while replacement is pending")
    );

    let recovery = Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/check")
        .env("CONDA_SHIP_INTERNAL_UPDATE_OFFLINE", "1")
        .output()
        .unwrap();
    assert!(!recovery.status.success());
    let recovery_error = String::from_utf8_lossy(&recovery.stderr);
    assert!(
        recovery_error.contains("recovery completed with DiscardedStaged")
            && recovery_error.contains("retry")
            && recovery_error.contains("the command"),
        "{recovery_error}"
    );
    assert_eq!(
        std::fs::read(&stable).unwrap(),
        std::fs::read(&first).unwrap()
    );

    let check = Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/check")
        .env("CONDA_SHIP_INTERNAL_UPDATE_OFFLINE", "1")
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let rechecked: serde_json::Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(rechecked["sha256"], selected_sha256);

    let stage = Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/stage")
        .env("CONDA_SHIP_INTERNAL_UPDATE_CANDIDATE", selected_sha256)
        .env("CONDA_SHIP_INTERNAL_UPDATE_OFFLINE", "1")
        .output()
        .unwrap();
    assert!(
        stage.status.success(),
        "{}",
        String::from_utf8_lossy(&stage.stderr)
    );

    let apply = Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/apply")
        .output()
        .unwrap();
    assert!(
        apply.status.success(),
        "{}",
        String::from_utf8_lossy(&apply.stderr)
    );
    let applied: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert!(
        applied["applied"] == true || (cfg!(windows) && applied["replacement_pending"] == true),
        "{applied}"
    );
    drop(coordinator);

    let expected = std::fs::read(&second).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::fs::read(&stable).unwrap() != expected && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(std::fs::read(&stable).unwrap(), expected);

    assert_cmd::Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .arg("--help")
        .assert()
        .success();
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prefix.join(".demo.json")).unwrap()).unwrap();
    assert_eq!(metadata["version"], "2.0.0");
    assert_eq!(metadata["update"]["ownership"], "direct");
    assert_eq!(metadata["update"]["installation"], "standalone");
    assert_eq!(metadata["update"]["build-number"], package["build_number"]);
    assert_eq!(metadata["update"]["sha256"], package["payload_sha256"]);
    assert!(metadata["update"].get("pending").is_none());
    assert!(metadata["update"].get("version").is_none());
    assert!(metadata["update"].get("provenance").is_none());
}

#[test]
fn test_external_manager_replacement_reconciles_the_stable_runtime() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let channel = tmp.path().join("channel");
    let platform = Platform::current().to_string();
    let subdir = channel.join(&platform);
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(
        subdir.join("repodata.json"),
        serde_json::to_vec(&serde_json::json!({
            "info": {"subdir": platform},
            "packages": {},
            "packages.conda": {},
            "removed": [],
            "repodata_version": 1,
        }))
        .unwrap(),
    )
    .unwrap();

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut manifest = std::fs::read_to_string(root.join("pixi.toml")).unwrap();
    let channel_url = reqwest::Url::from_directory_path(&channel)
        .unwrap()
        .to_string();
    manifest.push_str(&format!(
        "\n[tool.conda-ship.update]\nchannel = {channel_url:?}\npackage = \"demo-runtime\"\n"
    ));
    std::fs::write(project.join("pixi.toml"), manifest).unwrap();
    std::fs::copy(root.join("pixi.lock"), project.join("pixi.lock")).unwrap();

    let template = assert_cmd::cargo::cargo_bin!("cs-template");
    let first_dir = tmp.path().join("first");
    let second_dir = tmp.path().join("second");
    for (version, out_dir) in [("1.0.0", &first_dir), ("2.0.0", &second_dir)] {
        cargo_bin_cmd!("cs")
            .args([
                "build",
                "--root",
                project.to_str().unwrap(),
                "--runtime-name",
                "demo",
                "--delegate-executable",
                "cs",
                "--runtime-version",
                version,
                "--template",
                template.to_str().unwrap(),
                "--out-dir",
                out_dir.to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    let executable_name = if cfg!(windows) { "demo.exe" } else { "demo" };
    let first = first_dir.join(executable_name);
    let second = second_dir.join(executable_name);
    let second_info: serde_json::Value =
        serde_json::from_slice(&std::fs::read(second_dir.join("demo.info.json")).unwrap()).unwrap();
    let expected_sha256 = second_info["checksums"]
        .as_array()
        .unwrap()
        .iter()
        .find(|checksum| checksum["path"] == second_info["binary"])
        .unwrap()["sha256"]
        .as_str()
        .unwrap();

    #[cfg(unix)]
    {
        let predetection_prefix = tmp.path().join("predetection-prefix");
        let predetection_dir = tmp.path().join("predetection-bin");
        let predetection_stable = predetection_dir.join(executable_name);
        std::fs::create_dir_all(predetection_prefix.join("conda-meta")).unwrap();
        std::fs::create_dir_all(&predetection_dir).unwrap();
        std::os::unix::fs::symlink(&first, &predetection_stable).unwrap();
        std::fs::write(
            predetection_prefix.join(".demo.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "display_name": "demo",
                "install_name": "demo",
                "metadata_file": ".demo.json",
                "version": "1.0.0",
                "delegate_executable": "cs",
                "channels": [],
                "packages": [],
            }))
            .unwrap(),
        )
        .unwrap();
        install_cs_delegate(&predetection_prefix);
        assert_cmd::Command::new(&predetection_stable)
            .env("CONDA_SHIP_PREFIX", &predetection_prefix)
            .arg("--help")
            .assert()
            .success();

        std::fs::remove_file(&predetection_stable).unwrap();
        std::os::unix::fs::symlink(&second, &predetection_stable).unwrap();
        let recorded = record_installation(
            &predetection_stable,
            &predetection_prefix,
            "external",
            "package-manager",
            &predetection_stable,
            None,
        );
        assert!(
            recorded.status.success(),
            "{}",
            String::from_utf8_lossy(&recorded.stderr)
        );
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(predetection_prefix.join(".demo.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["version"], "2.0.0");
        assert_eq!(metadata["update"]["ownership"], "external");
        assert_eq!(
            metadata["update"]["executable"],
            predetection_stable.to_str().unwrap()
        );
    }

    let prefix = tmp.path().join("prefix");
    let install_dir = tmp.path().join("bin");
    let stable = install_dir.join(executable_name);
    std::fs::create_dir_all(prefix.join("conda-meta")).unwrap();
    std::fs::create_dir_all(&install_dir).unwrap();
    std::fs::copy(&first, &stable).unwrap();
    std::fs::write(
        prefix.join(".demo.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "display_name": "demo",
            "install_name": "demo",
            "metadata_file": ".demo.json",
            "version": "1.0.0",
            "delegate_executable": "cs",
            "channels": [],
            "packages": [],
        }))
        .unwrap(),
    )
    .unwrap();
    install_cs_delegate(&prefix);

    assert_cmd::Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .arg("--help")
        .assert()
        .success();
    let initial: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prefix.join(".demo.json")).unwrap()).unwrap();
    assert_eq!(initial["version"], "1.0.0");
    assert_eq!(initial["update"]["ownership"], "direct");
    assert_eq!(initial["update"]["executable"], stable.to_str().unwrap());

    let recorded = record_installation(&stable, &prefix, "external", "homebrew", &stable, None);
    assert!(
        recorded.status.success(),
        "{}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    let recorded: serde_json::Value = serde_json::from_slice(&recorded.stdout).unwrap();
    assert_eq!(recorded, serde_json::json!({"recorded": true}));

    assert_cmd::Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .arg("--help")
        .assert()
        .success();
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prefix.join(".demo.json")).unwrap()).unwrap();
    assert_eq!(persisted["update"]["ownership"], "external");
    assert_eq!(persisted["update"]["installation"], "homebrew");

    let alternate = install_dir.join(format!("alternate-{executable_name}"));
    std::fs::hard_link(&stable, &alternate).unwrap();
    for (case, ownership, installation, executable, instruction, expected) in [
        (
            "add an external instruction",
            "external",
            "homebrew",
            stable.as_path(),
            Some("Run the external package manager."),
            "instruction cannot be changed",
        ),
        (
            "change the stable path",
            "external",
            "homebrew",
            alternate.as_path(),
            None,
            "executable path cannot be changed",
        ),
        (
            "reclaim direct ownership",
            "direct",
            "standalone",
            stable.as_path(),
            None,
            "cannot become directly managed",
        ),
        (
            "change the installation kind",
            "external",
            "pipx",
            stable.as_path(),
            None,
            "changed from homebrew to pipx",
        ),
    ] {
        let rejected = record_installation(
            &stable,
            &prefix,
            ownership,
            installation,
            executable,
            instruction,
        );
        assert!(!rejected.status.success(), "{case}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(expected),
            "{case}: {}",
            String::from_utf8_lossy(&rejected.stderr)
        );
    }

    let mut tampered = persisted.clone();
    tampered["update"]["channel"] =
        serde_json::Value::String("https://different.example.test/channel".to_string());
    std::fs::write(
        prefix.join(".demo.json"),
        serde_json::to_vec(&tampered).unwrap(),
    )
    .unwrap();
    let rotate_source =
        record_installation(&stable, &prefix, "external", "homebrew", &stable, None);
    assert!(!rotate_source.status.success());
    assert!(
        String::from_utf8_lossy(&rotate_source.stderr)
            .contains("source changed outside its coordinated update")
    );
    std::fs::write(
        prefix.join(".demo.json"),
        serde_json::to_vec(&persisted).unwrap(),
    )
    .unwrap();

    let coordinator = hold_runtime_update_lock(&prefix);
    let check = Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/check")
        .env("CONDA_SHIP_INTERNAL_UPDATE_OFFLINE", "1")
        .output()
        .unwrap();
    assert!(check.status.success());
    let check: serde_json::Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(check["ownership"], "external");
    assert_eq!(check["installation"], "homebrew");
    assert!(check["instruction"].is_null());

    let stage = Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/stage")
        .env("CONDA_SHIP_INTERNAL_UPDATE_CANDIDATE", "a".repeat(64))
        .env("CONDA_SHIP_INTERNAL_UPDATE_OFFLINE", "1")
        .output()
        .unwrap();
    assert!(!stage.status.success());
    assert!(String::from_utf8_lossy(&stage.stderr).contains("managed externally"));
    drop(coordinator);

    #[cfg(unix)]
    {
        std::fs::remove_file(&stable).unwrap();
        std::os::unix::fs::symlink(&second, &stable).unwrap();
    }
    #[cfg(windows)]
    std::fs::copy(&second, &stable).unwrap();

    let coordinator = hold_runtime_update_lock(&prefix);
    let check = Command::new(&stable)
        .env("CONDA_SHIP_PREFIX", &prefix)
        .env("CONDA_SHIP_INTERNAL_UPDATE", "v1/check")
        .env("CONDA_SHIP_INTERNAL_UPDATE_OFFLINE", "1")
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let check: serde_json::Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(check["available"], false);
    drop(coordinator);

    assert_eq!(
        std::fs::read(&stable).unwrap(),
        std::fs::read(&second).unwrap()
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prefix.join(".demo.json")).unwrap()).unwrap();
    assert_eq!(metadata["version"], "2.0.0");
    assert_eq!(metadata["update"]["ownership"], "external");
    assert_eq!(metadata["update"]["installation"], "homebrew");
    assert_eq!(metadata["update"]["executable"], stable.to_str().unwrap());
    assert_eq!(metadata["update"]["channel"], channel_url);
    assert_eq!(metadata["update"]["package"], "demo-runtime");
    assert_eq!(metadata["update"]["build-number"], 0);
    assert_eq!(metadata["update"]["sha256"], expected_sha256);
    assert!(metadata["update"].get("instruction").is_none());
    assert!(metadata["update"].get("pending").is_none());
    assert!(metadata["update"].get("version").is_none());
    assert!(metadata["update"].get("provenance").is_none());
}
