#[cfg(any(target_os = "macos", target_os = "windows"))]
use assert_cmd::cargo::cargo_bin;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use rstest::rstest;
use tempfile::TempDir;

fn project_with_pypi_packages(source_environment: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("pixi.toml"),
        format!(
            r#"
[workspace]
name = "demo"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64"]

[tool.conda-ship]
runtime-name = "demo"
delegate-executable = "python"
source-environment = "{source_environment}"
"#
        ),
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("pixi.lock"),
        format!(
            r#"
version: 6
environments:
  ship:
    channels:
      - url: https://conda.anaconda.org/conda-forge
    packages:
      linux-64:
        - conda: https://conda.anaconda.org/conda-forge/linux-64/python-1.0-0.conda
  mixed:
    channels:
      - url: https://conda.anaconda.org/conda-forge
    indexes:
      - https://pypi.org/simple
    packages:
      linux-64:
        - conda: https://conda.anaconda.org/conda-forge/linux-64/python-1.0-0.conda
        - pypi: https://example.invalid/demo-1.0-py3-none-any.whl
  pypi-only:
    channels: []
    indexes:
      - https://pypi.org/simple
    packages:
      linux-64:
        - pypi: https://example.invalid/demo-1.0-py3-none-any.whl
  other-platform:
    channels:
      - url: https://conda.anaconda.org/conda-forge
    indexes:
      - https://pypi.org/simple
    packages:
      linux-64:
        - conda: https://conda.anaconda.org/conda-forge/linux-64/python-1.0-0.conda
      osx-arm64:
        - pypi: https://example.invalid/demo-1.0-py3-none-any.whl
packages:
  - conda: https://conda.anaconda.org/conda-forge/linux-64/python-1.0-0.conda
    sha256: {conda_sha256}
  - pypi: https://example.invalid/demo-1.0-py3-none-any.whl
    name: demo
    version: '1.0'
    sha256: {pypi_sha256}
"#,
            conda_sha256 = "a".repeat(64),
            pypi_sha256 = "b".repeat(64),
        ),
    )
    .unwrap();
    tmp
}

#[rstest]
#[case::mixed("mixed")]
#[case::pypi_only("pypi-only")]
#[case::other_platform("other-platform")]
fn test_cs_rejects_pypi_packages_in_selected_environment(
    #[case] source_environment: &str,
    #[values("inspect", "build")] command: &str,
) {
    let tmp = project_with_pypi_packages(source_environment);
    let original_lock = std::fs::read(tmp.path().join("pixi.lock")).unwrap();
    let mut cmd = cargo_bin_cmd!("cs");
    cmd.env("CONDA_SHIP_ERROR_FORMAT", "json").args([
        command,
        "--root",
        tmp.path().to_str().unwrap(),
        "--platform",
        "linux-64",
    ]);
    if command == "build" {
        cmd.arg("--dry-run");
    } else {
        cmd.arg("--json");
    }

    let assert = cmd.assert().failure().stdout(predicate::str::is_empty());
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).unwrap();

    assert_eq!(diagnostic["command"], command);
    assert_eq!(diagnostic["kind"], "unsupported_pypi_packages");
    assert_eq!(
        diagnostic["message"],
        format!("source environment {source_environment:?} contains unsupported PyPI packages")
    );
    assert_eq!(diagnostic["exit_code"], 1);
    let hint = diagnostic["hint"].as_str().unwrap();
    assert!(hint.contains("only conda packages"));
    assert!(hint.contains("pixi lock"));
    assert_eq!(
        std::fs::read(tmp.path().join("pixi.lock")).unwrap(),
        original_lock
    );
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2);
}

#[test]
fn test_cs_allows_pypi_packages_in_unselected_environments() {
    let tmp = project_with_pypi_packages("ship");

    let assert = cargo_bin_cmd!("cs")
        .args([
            "inspect",
            "--root",
            tmp.path().to_str().unwrap(),
            "--platform",
            "linux-64",
            "--json",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let output: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();

    assert_eq!(output["project"]["source_environment"], "ship");
    assert_eq!(
        output["runtime_input"]["packages"],
        serde_json::json!(["python"])
    );
    assert_eq!(output["runtime_input"]["package_count"], 1);
}

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
