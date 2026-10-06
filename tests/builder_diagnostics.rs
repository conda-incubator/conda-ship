#[cfg(any(
    feature = "runtime-template",
    target_os = "macos",
    target_os = "windows"
))]
use assert_cmd::cargo::cargo_bin;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use rstest::rstest;
use tempfile::TempDir;

#[cfg(feature = "runtime-template")]
#[rstest]
fn test_build_selects_explicit_inputs_without_changing_them(
    #[values("conda.toml", "pixi.toml", "pyproject.toml")] manifest_name: &str,
    #[values(1, 6)] lock_version: u8,
    #[values(false, true)] absolute_paths: bool,
    #[values(false, true)] dry_run: bool,
) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let project = root.join("project");
    let locks = root.as_path().join("locks");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&locks).unwrap();
    let manifest_path = project.join(manifest_name);
    let source_lock_path = locks.join("selected.lock");
    let manifest = r#"
[tool.conda.workspace]
name = "demo"

[tool.conda-ship]
runtime-name = "selected"
runtime-version = "1.2.3"
delegate-executable = "python"
source-environment = "missing"
condarc-file = "runtime.condarc"
"#;
    let platform = rattler_conda_types::Platform::current();
    let lock = format!(
        "version: {lock_version}\nenvironments:\n  ship:\n    channels: []\n    packages:\n      {platform}:\n        - conda: channel/{platform}/demo-1.0-0.conda\npackages:\n  - conda: channel/{platform}/demo-1.0-0.conda\n    subdir: {platform}\n    sha256: {}\n",
        "a".repeat(64),
    );
    std::fs::write(&manifest_path, manifest).unwrap();
    std::fs::write(&source_lock_path, &lock).unwrap();
    std::fs::write(project.join("runtime.condarc"), "channels: []\n").unwrap();
    if manifest_name != "conda.toml" {
        std::fs::write(project.join("conda.toml"), "invalid TOML").unwrap();
    }
    let manifest_arg = if absolute_paths {
        manifest_path.clone()
    } else {
        manifest_path
            .strip_prefix(root.as_path())
            .unwrap()
            .to_path_buf()
    };
    let lock_arg = if absolute_paths {
        source_lock_path.clone()
    } else {
        source_lock_path
            .strip_prefix(root.as_path())
            .unwrap()
            .to_path_buf()
    };
    let mut command = cargo_bin_cmd!("cs");
    command
        .current_dir(root.as_path())
        .arg("build")
        .arg("--manifest")
        .arg(manifest_arg)
        .arg("--source-lock")
        .arg(lock_arg)
        .args([
            "--source-environment",
            "ship",
            "--platform",
            platform.as_str(),
        ])
        .arg("--template")
        .arg(cargo_bin!("cs-template"));
    if dry_run {
        command.arg("--dry-run");
    }

    command.assert().success();

    assert_eq!(std::fs::read_to_string(&manifest_path).unwrap(), manifest);
    assert_eq!(std::fs::read_to_string(&source_lock_path).unwrap(), lock);
    assert!(!root.as_path().join("dist").exists());
    if dry_run {
        assert!(!project.join("dist").exists());
        assert!(!project.join("target").exists());
    } else {
        let runtime_lock =
            std::fs::read_to_string(project.join("dist/selected.runtime.lock")).unwrap();
        let lock =
            rattler_lock::LockFile::from_str_with_base_directory(&runtime_lock, None).unwrap();
        let env = lock.default_environment().unwrap();
        let (_, mut packages) = env.conda_packages_by_platform().next().unwrap();
        let package = packages.next().unwrap();
        let expected = locks.join(format!("channel/{platform}/demo-1.0-0.conda"));
        assert_eq!(
            std::path::Path::new(package.location().as_path().unwrap().as_str()),
            expected
        );
        let info: serde_json::Value = serde_json::from_slice(
            &std::fs::read(project.join("dist/selected.info.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(info["runtime_version"], "1.2.3");
        assert_eq!(info["package_count"], 1);
        assert!(project.join("dist/selected.cdx.json").is_file());
    }
}

#[cfg(feature = "runtime-template")]
fn project_with_local_package() -> TempDir {
    use sha2::Digest;

    let tmp = TempDir::new().unwrap();
    let platform = rattler_conda_types::Platform::current();
    let package = tmp.path().join("demo-1.0-0.conda");
    let contents = b"locked package contents";
    std::fs::write(&package, contents).unwrap();
    std::fs::write(
        tmp.path().join("conda.toml"),
        "[tool.conda-ship]\nruntime-name = 'selected'\nruntime-version = '1.2.3'\ndelegate-executable = 'python'\nsource-environment = 'ship'\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("source.lock"),
        format!(
            "version: 6\nenvironments:\n  ship:\n    channels: []\n    packages:\n      {platform}:\n        - conda: {location}\npackages:\n  - conda: {location}\n    subdir: {platform}\n    sha256: {sha256}\n",
            location = package.display(),
            sha256 = sha2::Sha256::digest(contents).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        ),
    )
    .unwrap();
    tmp
}

#[cfg(feature = "runtime-template")]
#[rstest]
#[case::generated_lock("target/conda-ship/runtime.lock", "online")]
#[case::generated_bundle("target/conda-ship/bundle.tar.zst", "embedded")]
#[case::bundle_directory("target/conda-ship/bundle/source.lock", "external")]
#[case::published_lock("dist/selected.runtime.lock", "online")]
#[case::published_binary("dist/selected{exe}", "online")]
#[case::published_info("dist/selected.info.json", "online")]
#[case::published_checksums("dist/selected.sha256", "online")]
#[case::published_packages("dist/selected.packages.txt", "online")]
#[case::published_sbom("dist/selected.cdx.json", "online")]
#[case::retired_bundle("dist/selected.bundle.tar.zst", "online")]
#[case::published_bundle("dist/selected.bundle.tar.zst", "external")]
fn test_build_rejects_source_lock_output_collision(
    #[case] destination: &str,
    #[case] layout: &str,
    #[values(false, true)] dry_run: bool,
) {
    let tmp = project_with_local_package();
    let manifest = tmp.path().join("conda.toml");
    let source_lock = tmp
        .path()
        .join(destination.replace("{exe}", std::env::consts::EXE_SUFFIX));
    std::fs::create_dir_all(source_lock.parent().unwrap()).unwrap();
    std::fs::rename(tmp.path().join("source.lock"), &source_lock).unwrap();
    let original_manifest = std::fs::read(&manifest).unwrap();
    let original_lock = std::fs::read(&source_lock).unwrap();
    let state_lock = tmp.path().join("target/conda-ship/runtime.lock");
    let original_state_lock = std::fs::read(&state_lock).ok();
    let mut command = cargo_bin_cmd!("cs");
    command
        .current_dir(tmp.path())
        .arg("build")
        .args(["--manifest", "conda.toml", "--source-lock"])
        .arg(&source_lock)
        .args(["--artifact-layout", layout, "--template"])
        .arg(cargo_bin!("cs-template"));
    if dry_run {
        command.arg("--dry-run");
    }

    let output = command.output().unwrap();

    assert_eq!(std::fs::read(&manifest).unwrap(), original_manifest);
    assert_eq!(std::fs::read(&source_lock).unwrap(), original_lock);
    assert_eq!(std::fs::read(&state_lock).ok(), original_state_lock);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("build output"));
}

#[cfg(feature = "runtime-template")]
#[rstest]
#[case::hard_link(false)]
#[cfg_attr(unix, case::source_symlink(true))]
fn test_build_rejects_source_input_output_alias(
    #[case] source_symlink: bool,
    #[values("conda.toml", "source.lock")] input: &str,
) {
    let tmp = project_with_local_package();
    let manifest = tmp.path().join("conda.toml");
    let source_lock = tmp.path().join("source.lock");
    let destination = tmp.path().join("target/conda-ship/runtime.lock");
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    if source_symlink {
        #[cfg(unix)]
        {
            std::fs::rename(tmp.path().join(input), &destination).unwrap();
            std::os::unix::fs::symlink(&destination, tmp.path().join(input)).unwrap();
        }
    } else {
        std::fs::hard_link(tmp.path().join(input), &destination).unwrap();
    }
    let original_manifest = std::fs::read(&manifest).unwrap();
    let original_lock = std::fs::read(&source_lock).unwrap();

    let output = cargo_bin_cmd!("cs")
        .current_dir(tmp.path())
        .args([
            "build",
            "--manifest",
            "conda.toml",
            "--source-lock",
            "source.lock",
            "--template",
        ])
        .arg(cargo_bin!("cs-template"))
        .output()
        .unwrap();

    assert_eq!(std::fs::read(&manifest).unwrap(), original_manifest);
    assert_eq!(std::fs::read(&source_lock).unwrap(), original_lock);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("build output"));
    assert!(!tmp.path().join("dist").exists());
}

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
    #[values(
        &["inspect", "--json"][..],
        &["build", "--dry-run"][..],
        &["build"][..],
        &["run"][..]
    )]
    args: &[&str],
) {
    let tmp = project_with_pypi_packages(source_environment);
    let original_lock = std::fs::read(tmp.path().join("pixi.lock")).unwrap();
    let mut cmd = cargo_bin_cmd!("cs");
    cmd.env("CONDA_SHIP_ERROR_FORMAT", "json").args(args).args([
        "--root",
        tmp.path().to_str().unwrap(),
        "--platform",
        "linux-64",
    ]);

    let assert = cmd.assert().failure().stdout(predicate::str::is_empty());
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stderr).unwrap();

    assert_eq!(diagnostic["command"], args[0]);
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
