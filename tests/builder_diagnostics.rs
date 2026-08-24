#[cfg(any(target_os = "macos", target_os = "windows"))]
use assert_cmd::cargo::cargo_bin;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_cs_emits_structured_builder_diagnostic_when_requested() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("conda.toml"),
        r#"
[tool.conda-ship]
runtime-name = "demo"
delegate-executable = "conda"
source-environment = "ship"
"#,
    )
    .unwrap();

    let assert = cargo_bin_cmd!("cs")
        .env("CONDA_SHIP_ERROR_FORMAT", "json")
        .args(["inspect", "--root", tmp.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(r#""kind":"missing_lockfile""#));

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let diagnostic: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();

    assert_eq!(diagnostic["schema_version"], 1);
    assert_eq!(diagnostic["tool"], "cs");
    assert_eq!(diagnostic["command"], "inspect");
    assert_eq!(diagnostic["kind"], "missing_lockfile");
    assert_eq!(diagnostic["exit_code"], 1);
    assert!(
        diagnostic["hint"]
            .as_str()
            .unwrap()
            .contains("conda workspace lock")
    );
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn test_builder_binary_is_not_accepted_as_runtime_template() {
    cargo_bin_cmd!("cs")
        .args([
            "build",
            "--dry-run",
            "--runtime-name",
            "builder-template",
            "--delegate-executable",
            "conda",
            "--template",
            cargo_bin!("cs").to_str().unwrap(),
            "--root",
            env!("CARGO_MANIFEST_DIR"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "conda_ship::runtime_template_incompatible",
        ));
}
