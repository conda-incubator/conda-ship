use std::path::Path;
use std::str::FromStr;

use clap::Parser;
use rattler_conda_types::{PackageName, PackageRecord, Platform, VersionWithSource};
use rattler_lock::{CondaPackageData, LockFileBuilder, PlatformData};
use rstest::rstest;
use tempfile::TempDir;

use super::artifact::{
    PackageInfo, apply_install_name_config, apply_runtime_metadata_overrides, artifact_stem,
    binary_filename, render_package_list, resolve_artifact_layout, resolve_artifact_name,
    resolve_delegate_executable, resolve_runtime_name, runtime_template_filename,
    runtime_template_from_env, source_binary, source_binary_plan, stage_artifacts,
    validate_artifact_name, validate_delegate_executable, validate_docs_url, validate_install_name,
    validate_installer, validate_package_archive_name, validate_runtime_name,
    validate_runtime_version, validate_target_label, validate_target_triple,
    validate_update_config,
};
use super::diagnostic::{DiagnosticKind, ShipDiagnostic};
use super::project::{
    DerivedRuntimeLock, ManifestKind, ProjectInput, derive_runtime_lock, discover_manifest_path,
    discover_project_input, filter_excluded, find_project_root, is_supported_pyproject_manifest,
    manifest_kind, read_condarc_file,
};
use super::sbom::{creation_timestamp, render_cyclonedx_sbom};
use super::{
    BundleLayout, Cli, Command, InstallNameConfig, InstallNameSource, InstallNameSourceConfig,
    RUNTIME_TEMPLATE_ENV, RuntimeStampConfig, RuntimeVersionConfig, RuntimeVersionSource,
    ShipConfig, runtime_data,
};

fn make_pkg(name: &str, depends: &[&str]) -> CondaPackageData {
    let mut record = PackageRecord::new(
        PackageName::new_unchecked(name),
        VersionWithSource::from_str("1.0").unwrap(),
        "0".to_string(),
    );
    record.depends = depends.iter().map(|d| d.to_string()).collect();
    CondaPackageData::from(rattler_conda_types::RepoDataRecord {
        package_record: record,
        identifier: rattler_conda_types::package::DistArchiveIdentifier::from(
            format!("{name}-1.0-0.conda")
                .parse::<rattler_conda_types::package::CondaArchiveIdentifier>()
                .unwrap(),
        ),
        url: format!("https://example.com/{name}-1.0-0.conda")
            .parse()
            .unwrap(),
        channel: Some("test".to_string()),
    })
}

fn make_sbom_pkg(name: &str, depends: &[&str], hash: u8, license: &str) -> CondaPackageData {
    let mut record = PackageRecord::new(
        PackageName::new_unchecked(name),
        VersionWithSource::from_str("1.0").unwrap(),
        "h123_0".to_string(),
    );
    record.depends = depends
        .iter()
        .map(|dependency| dependency.to_string())
        .collect();
    record.subdir = "linux-64".to_string();
    record.sha256 = Some([hash; 32].into());
    record.md5 = Some([hash; 16].into());
    record.license = Some(license.to_string());
    record.size = Some(42);
    CondaPackageData::from(rattler_conda_types::RepoDataRecord {
        package_record: record,
        identifier: rattler_conda_types::package::DistArchiveIdentifier::from(
            format!("{name}-1.0-h123_0.conda")
                .parse::<rattler_conda_types::package::CondaArchiveIdentifier>()
                .unwrap(),
        ),
        url: format!("https://conda.anaconda.org/conda-forge/linux-64/{name}-1.0-h123_0.conda")
            .parse()
            .unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge".to_string()),
    })
}

#[test]
fn test_find_project_root_finds_conda_pyproject() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("src").join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        tmp.path().join("pyproject.toml"),
        r#"
[tool.conda.workspace]
name = "demo"
channels = ["conda-forge"]
"#,
    )
    .unwrap();

    assert_eq!(find_project_root(&nested), Some(tmp.path().to_path_buf()));
}

#[test]
fn test_discover_manifest_prefers_conda_toml() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("conda.toml"), "").unwrap();
    std::fs::write(tmp.path().join("pixi.toml"), "").unwrap();
    std::fs::write(tmp.path().join("pyproject.toml"), "[tool.pixi.workspace]\n").unwrap();

    assert_eq!(
        discover_manifest_path(tmp.path()).unwrap(),
        tmp.path().join("conda.toml")
    );
}

#[test]
fn test_discover_project_input_uses_pixi_lock_for_pyproject() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("pyproject.toml"),
        r#"
[project]
version = "1.2.3"

[tool.pixi.workspace]
name = "demo"
channels = ["conda-forge"]

[tool.conda-ship]
runtime-name = "demo"
delegate-executable = "conda"
artifact-layout = "external"
source-environment = "ship"
"#,
    )
    .unwrap();
    std::fs::write(tmp.path().join("pixi.lock"), "").unwrap();

    let input = discover_project_input(tmp.path()).unwrap();

    assert_eq!(input.lock_path, tmp.path().join("pixi.lock"));
    assert_eq!(input.config.runtime_name.as_deref(), Some("demo"));
    assert_eq!(input.config.delegate_executable.as_deref(), Some("conda"));
    assert_eq!(input.config.artifact_layout, Some(BundleLayout::External));
    assert_eq!(input.config.source_environment.as_deref(), Some("ship"));
    assert_eq!(input.runtime_version.as_deref(), Some("1.2.3"));
    assert_eq!(input.runtime_version_source, None);
}

#[test]
fn test_discover_project_input_uses_conda_lock_for_conda_pyproject() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("pyproject.toml"),
        r#"
[tool.conda.workspace]
name = "demo"
channels = ["conda-forge"]

[tool.conda-ship]
runtime-name = "demo"
delegate-executable = "conda"
artifact-layout = "embedded"
source-environment = "ship"
"#,
    )
    .unwrap();
    std::fs::write(tmp.path().join("conda.lock"), "").unwrap();

    let input = discover_project_input(tmp.path()).unwrap();

    assert_eq!(input.lock_path, tmp.path().join("conda.lock"));
    assert_eq!(input.config.runtime_name.as_deref(), Some("demo"));
    assert_eq!(input.config.delegate_executable.as_deref(), Some("conda"));
    assert_eq!(input.config.artifact_layout, Some(BundleLayout::Embedded));
    assert_eq!(input.config.source_environment.as_deref(), Some("ship"));
}

#[test]
fn test_derive_runtime_lock_accepts_conda_workspaces_lock_v1_without_conda() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("conda.toml"),
        r#"
[tool.conda-ship]
source-environment = "ship"
delegate-executable = "python"
"#,
    )
    .unwrap();
    let sha256 = "a".repeat(64);
    std::fs::write(
        tmp.path().join("conda.lock"),
        format!(
            r#"
---
# conda-workspaces writes version 1 on disk but reads it as rattler-lock v6.
version: 1
environments:
  ship:
    channels:
      - url: https://conda.anaconda.org/conda-forge
    packages:
      linux-64:
        - conda: https://conda.anaconda.org/conda-forge/linux-64/python-1.0-0.conda
packages:
  - conda: https://conda.anaconda.org/conda-forge/linux-64/python-1.0-0.conda
    sha256: {sha256}
"#
        ),
    )
    .unwrap();

    let derived = derive_runtime_lock(tmp.path()).unwrap();

    assert_eq!(derived.source_environment, "ship");
    assert_eq!(derived.platforms, vec![Platform::Linux64]);
    assert_eq!(derived.runtime_config.packages, vec!["python".to_string()]);
    assert_eq!(derived.total_packages, 1);
}

#[test]
fn test_discover_project_input_accepts_project_metadata_runtime_version_source() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("pyproject.toml"),
        r#"
[project]
name = "demo"
dynamic = ["version"]

[build-system]
requires = []
build-backend = "demo_backend"

[tool.conda.workspace]
name = "demo"
channels = ["conda-forge"]

[tool.conda-ship]
runtime-name = "demo"
runtime-version = { from = "project-metadata" }
delegate-executable = "conda"
artifact-layout = "embedded"
source-environment = "ship"
"#,
    )
    .unwrap();
    std::fs::write(tmp.path().join("conda.lock"), "").unwrap();

    let input = discover_project_input(tmp.path()).unwrap();

    assert_eq!(input.runtime_version, None);
    assert_eq!(
        input.runtime_version_source,
        Some(RuntimeVersionSource::ProjectMetadata)
    );
    assert!(input.project_dynamic_version);
}

#[test]
fn test_runtime_version_config_deserializes_string_and_source() {
    let manifest: super::ProjectManifest = toml::from_str(
        r#"
[tool.conda-ship]
runtime-version = { from = "project-metadata" }
"#,
    )
    .unwrap();
    let runtime_version = manifest
        .tool
        .conda_ship
        .runtime_version
        .expect("runtime-version should deserialize");

    assert_eq!(
        runtime_version,
        RuntimeVersionConfig::Source(super::RuntimeVersionSourceConfig {
            from: RuntimeVersionSource::ProjectMetadata,
        })
    );

    let manifest: super::ProjectManifest = toml::from_str(
        r#"
[tool.conda-ship]
runtime-version = "1.2.3"
"#,
    )
    .unwrap();

    assert_eq!(
        manifest.tool.conda_ship.runtime_version,
        Some(RuntimeVersionConfig::Value("1.2.3".to_string()))
    );
}

#[test]
fn test_artifact_name_config_deserializes() {
    let manifest: super::ProjectManifest = toml::from_str(
        r#"
[tool.conda-ship]
artifact-name = "demo-cli"
"#,
    )
    .unwrap();

    assert_eq!(
        manifest.tool.conda_ship.artifact_name.as_deref(),
        Some("demo-cli")
    );
}

#[test]
fn test_full_clarity_config_keys_deserialize() {
    let manifest: super::ProjectManifest = toml::from_str(
        r#"
[tool.conda-ship]
delegate-executable = "conda"
artifact-layout = "embedded"
exclude-packages = ["conda-libmamba-solver"]
installer = "homebrew"
condarc-file = "runtime.condarc"
freeze-base = true

[tool.conda-ship.update]
channel = "https://prefix.dev/demo"
package = "demo-runtime"
build-number = 2
"#,
    )
    .unwrap();

    assert_eq!(
        manifest.tool.conda_ship.delegate_executable.as_deref(),
        Some("conda")
    );
    assert_eq!(
        manifest.tool.conda_ship.artifact_layout,
        Some(BundleLayout::Embedded)
    );
    assert_eq!(
        manifest.tool.conda_ship.exclude_packages,
        vec!["conda-libmamba-solver".to_string()]
    );
    assert_eq!(
        manifest.tool.conda_ship.installer.as_deref(),
        Some("homebrew")
    );
    assert_eq!(
        manifest.tool.conda_ship.condarc_file.as_deref(),
        Some(Path::new("runtime.condarc"))
    );
    assert!(manifest.tool.conda_ship.freeze_base);
    assert_eq!(
        manifest.tool.conda_ship.update,
        Some(runtime_data::RuntimeUpdateConfig::new(
            "https://prefix.dev/demo".to_string(),
            "demo-runtime".to_string(),
            2,
        ))
    );
}

#[test]
fn test_update_config_defaults_to_build_zero() {
    let manifest: super::ProjectManifest = toml::from_str(
        r#"
[tool.conda-ship.update]
channel = "https://conda.anaconda.org/jezdez"
package = "conda-runtime"
"#,
    )
    .unwrap();

    let update = manifest.tool.conda_ship.update.unwrap();
    assert_eq!(update.build_number, 0);
}

#[rstest]
#[case::online(BundleLayout::Online)]
#[case::embedded(BundleLayout::Embedded)]
fn test_update_config_accepts_supported_layout(#[case] layout: BundleLayout) {
    let config = RuntimeStampConfig {
        update: Some(runtime_data::RuntimeUpdateConfig::new(
            "https://conda.anaconda.org/jezdez".to_string(),
            "conda-runtime".to_string(),
            0,
        )),
        ..RuntimeStampConfig::default()
    };

    validate_update_config(&config, layout, None).unwrap();
}

#[rstest]
#[case::external_layout(
    BundleLayout::External,
    "https://prefix.dev/demo",
    "demo-runtime",
    "only for online and embedded"
)]
#[case::empty_channel(BundleLayout::Online, "", "demo-runtime", "must not be empty")]
#[case::userinfo(
    BundleLayout::Embedded,
    "https://user@prefix.dev/demo",
    "demo-runtime",
    "must not contain credentials"
)]
#[case::fragment(
    BundleLayout::Embedded,
    "https://prefix.dev/demo#release",
    "demo-runtime",
    "query or fragment"
)]
#[case::insecure_http(
    BundleLayout::Online,
    "http://packages.example.test/demo",
    "demo-runtime",
    "must use https:// or file://"
)]
#[case::invalid_package(
    BundleLayout::Online,
    "https://prefix.dev/demo",
    "Invalid Package",
    "invalid runtime update package name"
)]
fn test_update_config_rejects_invalid_policy(
    #[case] layout: BundleLayout,
    #[case] channel: &str,
    #[case] package: &str,
    #[case] expected: &str,
) {
    let config = RuntimeStampConfig {
        update: Some(runtime_data::RuntimeUpdateConfig::new(
            channel.to_string(),
            package.to_string(),
            0,
        )),
        ..RuntimeStampConfig::default()
    };

    let error = validate_update_config(&config, layout, None)
        .unwrap_err()
        .to_string();

    assert!(error.contains(expected), "{error}");
}

#[rstest]
#[case::ownership("ownership = \"direct\"")]
#[case::instruction("instruction = \"Run an installer.\"")]
fn test_update_config_rejects_installed_state(#[case] setting: &str) {
    let manifest: super::ProjectManifest = toml::from_str(&format!(
        r#"
[tool.conda-ship.update]
channel = "https://prefix.dev/demo"
package = "demo-runtime"
{setting}
"#
    ))
    .unwrap();
    let config = RuntimeStampConfig {
        update: manifest.tool.conda_ship.update,
        ..RuntimeStampConfig::default()
    };

    let error = validate_update_config(&config, BundleLayout::Online, None)
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("installed state, not build settings"),
        "{error}"
    );
}

#[test]
fn test_read_condarc_file_resolves_from_manifest_and_preserves_text() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let manifest = project.join("conda.toml");
    std::fs::write(&manifest, "").unwrap();
    let expected = "# downstream policy\nchannels:\n  - conda-forge\n";
    std::fs::write(project.join("runtime.condarc"), expected).unwrap();

    let contents = read_condarc_file(&manifest, Some(Path::new("runtime.condarc")))
        .unwrap()
        .unwrap();

    assert_eq!(contents, expected);
}

#[rstest]
#[case::sequence("- conda-forge\n", "must contain a YAML mapping")]
#[case::invalid("channels: [\n", "failed to parse condarc-file")]
fn test_read_condarc_file_rejects_invalid_input(#[case] contents: &str, #[case] expected: &str) {
    let tmp = TempDir::new().unwrap();
    let manifest = tmp.path().join("conda.toml");
    std::fs::write(&manifest, "").unwrap();
    std::fs::write(tmp.path().join("runtime.condarc"), contents).unwrap();

    let error = read_condarc_file(&manifest, Some(Path::new("runtime.condarc")))
        .expect_err("invalid condarc should fail")
        .to_string();

    assert!(error.contains(expected), "{error}");
}

#[rstest]
#[case::delegate("delegate = \"conda\"", "delegate")]
#[case::layout("layout = \"embedded\"", "layout")]
#[case::exclude("exclude = [\"conda-libmamba-solver\"]", "exclude")]
#[case::install_method("install-method = \"homebrew\"", "install-method")]
fn test_old_full_clarity_config_keys_are_rejected(#[case] key: &str, #[case] expected: &str) {
    let err = match toml::from_str::<super::ProjectManifest>(&format!("[tool.conda-ship]\n{key}\n"))
    {
        Ok(_) => panic!("old key should be rejected"),
        Err(err) => err.to_string(),
    };

    assert!(err.contains(expected), "{err}");
}

#[test]
fn test_conda_pyproject_wins_over_pixi_pyproject() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("pyproject.toml"),
        r#"
[tool.conda.workspace]
name = "demo"

[tool.pixi.workspace]
name = "demo-pixi"
"#,
    )
    .unwrap();

    assert_eq!(
        manifest_kind(&tmp.path().join("pyproject.toml")).unwrap(),
        ManifestKind::CondaPyproject
    );
}

#[rstest]
#[case::empty_conda("[tool.conda]\n")]
#[case::partial_conda("[tool.conda.dependencies]\npython = \"*\"\n")]
#[case::empty_conda_workspace("[tool.conda.workspace]\n")]
#[case::invalid_conda_workspace("[tool.conda]\nworkspace = true\n")]
fn test_pixi_pyproject_wins_without_conda_workspace(#[case] conda_config: &str) {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("pyproject.toml"),
        format!(
            "{conda_config}\n[tool.pixi.workspace]\nname = \"demo\"\n\n[tool.conda-ship]\nsource-environment = \"ship\"\n"
        ),
    )
    .unwrap();
    std::fs::write(tmp.path().join("pixi.lock"), "").unwrap();

    let input = discover_project_input(tmp.path()).unwrap();

    assert_eq!(input.manifest_kind, ManifestKind::PixiPyproject);
    assert_eq!(input.lock_path, tmp.path().join("pixi.lock"));
    assert_eq!(input.config.source_environment.as_deref(), Some("ship"));
}

#[rstest]
#[case::empty_conda("[tool.conda]\n")]
#[case::empty_pixi("[tool.pixi]\n")]
#[case::partial_conda("[tool.conda.dependencies]\npython = \"*\"\n")]
#[case::partial_pixi("[tool.pixi.dependencies]\npython = \"*\"\n")]
#[case::empty_conda_workspace("[tool.conda.workspace]\n")]
#[case::empty_pixi_workspace("[tool.pixi.workspace]\n")]
#[case::invalid_conda_workspace("[tool.conda]\nworkspace = true\n")]
#[case::invalid_pixi_workspace("[tool.pixi]\nworkspace = \"demo\"\n")]
#[case::invalid_toml("[tool.conda.workspace\n")]
fn test_pyproject_requires_nonempty_workspace_table(#[case] workspace: &str) {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("pyproject.toml"), workspace).unwrap();

    assert!(!is_supported_pyproject_manifest(
        &tmp.path().join("pyproject.toml")
    ));
    assert_eq!(find_project_root(tmp.path()), None);
}

#[test]
fn test_pyproject_requires_conda_or_pixi_config() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("pyproject.toml"),
        r#"
[tool.conda-ship]
source-environment = "ship"
"#,
    )
    .unwrap();

    assert!(!is_supported_pyproject_manifest(
        &tmp.path().join("pyproject.toml")
    ));
}

#[test]
fn test_find_installed_runtime_template_uses_env_override() {
    let tmp = TempDir::new().unwrap();
    let template = tmp.path().join(runtime_template_filename());
    std::fs::write(&template, b"runtime template").unwrap();

    temp_env::with_var(RUNTIME_TEMPLATE_ENV, Some(template.as_os_str()), || {
        assert_eq!(runtime_template_from_env().unwrap(), Some(template.clone()));
    });
}

#[test]
fn test_source_binary_prefers_explicit_template() {
    let tmp = TempDir::new().unwrap();
    let template = tmp.path().join("custom-template");
    std::fs::write(&template, b"runtime template").unwrap();

    assert_eq!(source_binary(Some(&template), None).unwrap(), template);
}

#[rstest]
#[case::cross_build(Some("x86_64-unknown-linux-gnu"), "cross-builds require --template")]
#[case::installed_template(None, "runtime template not found")]
fn test_source_binary_plan_reports_missing_template(
    #[case] target: Option<&str>,
    #[case] expected: &str,
) {
    temp_env::with_var(RUNTIME_TEMPLATE_ENV, None::<&str>, || {
        let err = source_binary_plan(None, target).unwrap_err().to_string();
        assert!(err.contains(expected));
    });
}

#[test]
fn test_empty_excludes_returns_all() {
    let packages = vec![make_pkg("a", &[]), make_pkg("b", &["a"])];
    let (filtered, removed) = filter_excluded(&packages, &[]).unwrap();
    assert!(removed.is_empty());
    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_exclude_single_leaf() {
    let packages = vec![make_pkg("a", &[]), make_pkg("b", &[])];
    let excludes = vec!["b".to_string()];
    let (filtered, removed) = filter_excluded(&packages, &excludes).unwrap();
    assert_eq!(removed, vec!["b"]);
    assert_eq!(filtered.len(), 1);
}

#[test]
fn test_exclude_with_transitive_deps() {
    let packages = vec![
        make_pkg("a", &["b"]),
        make_pkg("b", &["c"]),
        make_pkg("c", &[]),
    ];
    let excludes = vec!["a".to_string()];
    let (filtered, removed) = filter_excluded(&packages, &excludes).unwrap();
    assert_eq!(removed, vec!["a", "b", "c"]);
    assert!(filtered.is_empty());
}

#[test]
fn test_shared_dep_not_removed() {
    let packages = vec![
        make_pkg("a", &["c"]),
        make_pkg("b", &["c"]),
        make_pkg("c", &[]),
    ];
    let excludes = vec!["a".to_string()];
    let (filtered, removed) = filter_excluded(&packages, &excludes).unwrap();
    assert_eq!(removed, vec!["a"]);
    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_exclude_nonexistent_package() {
    let packages = vec![make_pkg("a", &[]), make_pkg("b", &[])];
    let excludes = vec!["nonexistent".to_string()];
    let (filtered, removed) = filter_excluded(&packages, &excludes).unwrap();
    assert!(removed.is_empty());
    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_diamond_dependency() {
    let packages = vec![
        make_pkg("a", &["c"]),
        make_pkg("b", &["c"]),
        make_pkg("c", &[]),
        make_pkg("d", &["a"]),
    ];
    let excludes = vec!["d".to_string()];
    let (filtered, removed) = filter_excluded(&packages, &excludes).unwrap();
    assert_eq!(removed, vec!["a", "d"]);
    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_multiple_simultaneous_excludes() {
    let packages = vec![
        make_pkg("a", &["shared"]),
        make_pkg("b", &["only-b"]),
        make_pkg("shared", &[]),
        make_pkg("only-b", &[]),
        make_pkg("keep", &[]),
    ];
    let excludes = vec!["a".to_string(), "b".to_string()];
    let (filtered, removed) = filter_excluded(&packages, &excludes).unwrap();
    assert_eq!(removed, vec!["a", "b", "only-b", "shared"]);
    assert_eq!(filtered.len(), 1);
}

#[rstest]
#[case::with_label(Some("linux-64"), "demo-linux-64")]
#[case::without_label(None, "demo")]
fn test_artifact_stem(#[case] target_label: Option<&str>, #[case] expected: &str) {
    assert_eq!(artifact_stem("demo", target_label), expected);
}

#[test]
fn test_resolve_artifact_name_uses_runtime_by_default() {
    assert_eq!(
        resolve_artifact_name("demo", None, &ShipConfig::default()),
        "demo"
    );
}

#[test]
fn test_resolve_artifact_name_uses_cli_artifact_name() {
    let config = ShipConfig {
        artifact_name: Some("manifest".to_string()),
        ..ShipConfig::default()
    };

    assert_eq!(
        resolve_artifact_name("demo", Some("cli".to_string()), &config),
        "cli"
    );
}

#[test]
fn test_resolve_artifact_name_uses_manifest_artifact_name() {
    let config = ShipConfig {
        artifact_name: Some("demo-cli".to_string()),
        ..ShipConfig::default()
    };

    assert_eq!(resolve_artifact_name("demo", None, &config), "demo-cli");
}

#[test]
fn test_resolve_artifact_name_is_layout_independent() {
    let config = ShipConfig {
        artifact_name: Some("demo-cli".to_string()),
        ..ShipConfig::default()
    };

    assert_eq!(
        resolve_artifact_name("demo", Some("external-cli".to_string()), &config),
        "external-cli"
    );
}

#[test]
fn test_stage_artifacts_external_uses_artifact_name_for_files() {
    let tmp = TempDir::new().unwrap();
    let source_binary = tmp.path().join("cs-template");
    let source_bundle = tmp.path().join("bundle.tar.zst");
    std::fs::write(&source_binary, b"runtime template").unwrap();
    std::fs::write(&source_bundle, b"bundle archive").unwrap();

    let platform = Platform::Linux64;
    let platform_name = platform.to_string();
    let platform_data = PlatformData {
        name: rattler_lock::PlatformName::try_from(platform_name.clone()).unwrap(),
        subdir: platform,
        virtual_packages: Vec::new(),
    };
    let mut builder = LockFileBuilder::new()
        .with_platforms(vec![platform_data])
        .unwrap();
    builder
        .add_conda_package("default", platform_name.as_str(), make_pkg("conda", &[]))
        .unwrap();
    let lock_file = builder.finish();
    let content = lock_file.render_to_string().unwrap();
    let derived = DerivedRuntimeLock {
        input: ProjectInput {
            manifest_path: tmp.path().join("conda.toml"),
            manifest_kind: ManifestKind::CondaToml,
            lock_path: tmp.path().join("conda.lock"),
            config: ShipConfig::default(),
            runtime_version: None,
            runtime_version_source: None,
            project_dynamic_version: false,
        },
        lock_file,
        content,
        source_environment: "ship".to_string(),
        runtime_config: RuntimeStampConfig {
            channels: vec!["conda-forge".to_string()],
            packages: vec!["conda".to_string()],
            delegate_executable: Some("conda".to_string()),
            runtime_version: Some("9.8.7".to_string()),
            installer: Some("homebrew".to_string()),
            condarc: Some("solver: rattler\n".to_string()),
            freeze_base: true,
            ..RuntimeStampConfig::default()
        },
        platforms: vec![platform],
        total_packages: 1,
        total_excluded: 0,
        removed_excludes: Vec::new(),
    };

    let output = stage_artifacts(
        tmp.path(),
        &source_binary,
        BundleLayout::External,
        "demo",
        "demo-cli",
        Some("linux-64"),
        platform,
        None,
        Path::new("dist"),
        &derived,
        Some(&source_bundle),
    )
    .unwrap();

    assert!(output.binary.is_file());
    let expected_binary = binary_filename("demo-cli-linux-64", None);
    assert_eq!(
        output.binary.file_name().and_then(|name| name.to_str()),
        Some(expected_binary.as_str())
    );
    let stamped = runtime_data::read_from_path(&output.binary)
        .unwrap()
        .expect("staged binary should be stamped");
    assert_eq!(stamped.header.artifact_name, "demo-cli");
    assert_eq!(stamped.header.runtime_name, "demo");
    assert_eq!(stamped.header.embedded_artifact_name, "demo-cli");
    assert_eq!(stamped.header.delegate_executable, "conda");
    assert_eq!(stamped.header.metadata_file, ".demo.json");
    assert_eq!(stamped.header.bundle_env_var, "DEMO_BUNDLE");
    assert_eq!(stamped.header.runtime_version, "9.8.7");
    assert_eq!(stamped.header.artifact_layout, "external");
    assert_eq!(stamped.header.platform, "linux-64");
    assert!(stamped.header.update.is_none());
    assert_eq!(stamped.header.installer.as_deref(), Some("homebrew"));
    assert_eq!(
        stamped.header.runtime_config.condarc.as_deref(),
        Some("solver: rattler\n")
    );
    assert!(stamped.header.runtime_config.freeze_base);
    assert_eq!(
        stamped.header.runtime_config.packages,
        vec!["conda".to_string()]
    );
    let bundle = output
        .bundle
        .expect("external layout should stage a bundle");
    assert_eq!(
        bundle.file_name().and_then(|name| name.to_str()),
        Some("demo-cli-linux-64.bundle.tar.zst")
    );
    assert!(bundle.is_file());

    let info: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&output.info).unwrap()).unwrap();
    assert_eq!(info["layout"], "external");
    assert_eq!(info["name"], "demo-cli-linux-64");
    assert_eq!(info["artifact_name"], "demo-cli");
    assert_eq!(info["runtime_name"], "demo");
    assert_eq!(info["runtime_version"], "9.8.7");
    assert!(info["update"].is_null());
    assert_eq!(info["bundle"], "demo-cli-linux-64.bundle.tar.zst");
    assert_eq!(info["sbom"], "demo-cli-linux-64.cdx.json");

    let sbom: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&output.sbom).unwrap()).unwrap();
    assert_eq!(sbom["bomFormat"], "CycloneDX");
    assert_eq!(sbom["specVersion"], "1.7");
    assert_eq!(sbom["metadata"]["component"]["name"], "demo-cli");
    assert_eq!(sbom["components"][0]["name"], "conda");

    let checksums = std::fs::read_to_string(&output.checksums).unwrap();
    assert!(checksums.contains("demo-cli-linux-64.bundle.tar.zst"));
    assert!(checksums.contains("demo-cli-linux-64.cdx.json"));
}

#[test]
fn test_stage_artifacts_embedded_uses_artifact_name_for_files() {
    let tmp = TempDir::new().unwrap();
    let source_binary = tmp.path().join("cs-template");
    let source_bundle = tmp.path().join("bundle.tar.zst");
    std::fs::write(&source_binary, b"runtime template").unwrap();
    std::fs::write(&source_bundle, b"bundle archive").unwrap();

    let platform = Platform::Linux64;
    let platform_name = platform.to_string();
    let platform_data = PlatformData {
        name: rattler_lock::PlatformName::try_from(platform_name.clone()).unwrap(),
        subdir: platform,
        virtual_packages: Vec::new(),
    };
    let mut builder = LockFileBuilder::new()
        .with_platforms(vec![platform_data])
        .unwrap();
    builder
        .add_conda_package("default", platform_name.as_str(), make_pkg("conda", &[]))
        .unwrap();
    let lock_file = builder.finish();
    let content = lock_file.render_to_string().unwrap();
    let derived = DerivedRuntimeLock {
        input: ProjectInput {
            manifest_path: tmp.path().join("conda.toml"),
            manifest_kind: ManifestKind::CondaToml,
            lock_path: tmp.path().join("conda.lock"),
            config: ShipConfig::default(),
            runtime_version: None,
            runtime_version_source: None,
            project_dynamic_version: false,
        },
        lock_file,
        content,
        source_environment: "ship".to_string(),
        runtime_config: RuntimeStampConfig {
            channels: vec!["conda-forge".to_string()],
            packages: vec!["conda".to_string()],
            delegate_executable: Some("conda".to_string()),
            runtime_version: Some("9.8.7".to_string()),
            update: Some(runtime_data::RuntimeUpdateConfig::new(
                "https://conda.anaconda.org/jezdez".to_string(),
                "conda-runtime".to_string(),
                3,
            )),
            ..RuntimeStampConfig::default()
        },
        platforms: vec![platform],
        total_packages: 1,
        total_excluded: 0,
        removed_excludes: Vec::new(),
    };

    let output = stage_artifacts(
        tmp.path(),
        &source_binary,
        BundleLayout::Embedded,
        "demo",
        "demoz",
        None,
        platform,
        None,
        Path::new("dist"),
        &derived,
        Some(&source_bundle),
    )
    .unwrap();

    assert!(output.binary.is_file());
    let expected_binary = binary_filename("demoz", None);
    assert_eq!(
        output.binary.file_name().and_then(|name| name.to_str()),
        Some(expected_binary.as_str())
    );
    assert!(output.bundle.is_none());

    let stamped = runtime_data::read_from_path(&output.binary)
        .unwrap()
        .expect("staged binary should be stamped");
    assert_eq!(stamped.header.artifact_name, "demoz");
    assert_eq!(stamped.header.runtime_name, "demo");
    assert_eq!(stamped.header.embedded_artifact_name, "demoz");
    assert_eq!(stamped.header.delegate_executable, "conda");
    assert_eq!(stamped.header.install_name, "demo");
    assert_eq!(stamped.header.metadata_file, ".demo.json");
    assert_eq!(stamped.header.bundle_env_var, "DEMO_BUNDLE");
    assert_eq!(stamped.header.artifact_layout, "embedded");
    assert_eq!(stamped.header.platform, "linux-64");
    assert!(stamped.bundle.is_some());

    let info: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&output.info).unwrap()).unwrap();
    assert_eq!(info["name"], "demoz");
    assert_eq!(info["artifact_name"], "demoz");
    assert_eq!(info["runtime_name"], "demo");
    assert_eq!(info["runtime_version"], "9.8.7");
    assert_eq!(info["layout"], "embedded");
    assert_eq!(info["binary"], expected_binary);
    assert!(info["bundle"].is_null());
    assert_eq!(info["sbom"], "demoz.cdx.json");
    assert!(output.sbom.is_file());
    assert_eq!(
        info["update"],
        serde_json::to_value(stamped.header.update).unwrap()
    );
}

#[rstest]
#[case::runtime_name("runtime name", validate_runtime_name, "conda-ship_1.0")]
#[case::artifact_name("artifact name", validate_artifact_name, "conda-ship_1.0")]
#[case::runtime_version("runtime version", validate_runtime_version, "1!2.3+local")]
#[case::docs_url("docs URL", validate_docs_url, "https://example.com/demo/")]
#[case::delegate_executable("delegate executable", validate_delegate_executable, "python3.12")]
#[case::install_name("install name", validate_install_name, "conda-express_1.0")]
fn test_artifact_component_allows_filename_safe_values(
    #[case] _kind: &str,
    #[case] validate: fn(&str) -> miette::Result<()>,
    #[case] value: &str,
) {
    validate(value).unwrap();
}

#[rstest]
#[case::runtime_name_dot(validate_runtime_name, ".", "runtime name must not be . or ..")]
#[case::runtime_name_leading_dash(
    validate_runtime_name,
    "-demo",
    "runtime name must start with an ASCII letter or digit"
)]
#[case::runtime_name_path(
    validate_runtime_name,
    "demo/tool",
    "runtime name may only contain ASCII letters, digits, dots, dashes, and underscores"
)]
#[case::runtime_name_newline(
    validate_runtime_name,
    "demo\ntool",
    "runtime name may only contain ASCII letters, digits, dots, dashes, and underscores"
)]
#[case::artifact_name_path(
    validate_artifact_name,
    "demo/tool",
    "artifact name may only contain ASCII letters, digits, dots, dashes, and underscores"
)]
#[case::target_label_path(
    validate_target_label,
    "linux/64",
    "target label may only contain ASCII letters, digits, dots, dashes, and underscores"
)]
#[case::target_triple_path(
    validate_target_triple,
    "custom/target.json",
    "target triple may only contain ASCII letters, digits, dots, dashes, and underscores"
)]
#[case::install_name_dot(validate_install_name, ".", "install name must not be . or ..")]
#[case::install_name_path(
    validate_install_name,
    "conda/express",
    "install name may only contain ASCII letters, digits, dots, dashes, and underscores"
)]
#[case::install_name_newline(
    validate_install_name,
    "express\n",
    "install name may only contain ASCII letters, digits, dots, dashes, and underscores"
)]
#[case::runtime_version_path(
    validate_runtime_version,
    "1.0/local",
    "runtime version may only contain ASCII letters, digits, dots, dashes, underscores, plus signs, and exclamation marks"
)]
#[case::docs_url_newline(
    validate_docs_url,
    "https://example.com/\nmalicious",
    "docs URL must not contain whitespace or control characters"
)]
#[case::docs_url_relative(
    validate_docs_url,
    "docs/index.html",
    "docs URL must start with https:// or http://"
)]
#[case::installer_path(
    validate_installer,
    "home/brew",
    "installer may only contain ASCII letters, digits, dots, dashes, and underscores"
)]
fn test_artifact_component_rejects_unsafe_values(
    #[case] validate: fn(&str) -> miette::Result<()>,
    #[case] value: &str,
    #[case] expected: &str,
) {
    let err = validate(value).unwrap_err().to_string();
    assert!(err.contains(expected));
}

#[test]
fn test_build_accepts_install_scheme_with_install_name() {
    let cli = Cli::try_parse_from([
        "cs",
        "build",
        "--runtime-name",
        "cx",
        "--artifact-name",
        "cxz",
        "--delegate-executable",
        "conda",
        "--runtime-version",
        "0.6.0",
        "--install-scheme",
        "user-data",
        "--install-name",
        "express",
        "--installer",
        "homebrew",
    ])
    .unwrap();

    let Command::Build {
        runtime_name,
        artifact_name,
        delegate_executable,
        runtime_version,
        install_scheme,
        install_name,
        installer,
        ..
    } = cli.command
    else {
        panic!("expected build command");
    };

    assert_eq!(runtime_name.as_deref(), Some("cx"));
    assert_eq!(artifact_name.as_deref(), Some("cxz"));
    assert_eq!(delegate_executable.as_deref(), Some("conda"));
    assert_eq!(runtime_version.as_deref(), Some("0.6.0"));
    assert_eq!(install_scheme, Some(runtime_data::InstallScheme::UserData));
    assert_eq!(install_name.as_deref(), Some("express"));
    assert_eq!(installer.as_deref(), Some("homebrew"));
}

#[test]
fn test_build_accepts_manifest_runtime_without_cli_runtime() {
    let cli = Cli::try_parse_from(["cs", "build"]).unwrap();

    let Command::Build {
        runtime_name,
        artifact_layout,
        ..
    } = cli.command
    else {
        panic!("expected build command");
    };

    assert_eq!(runtime_name, None);
    assert_eq!(artifact_layout, None);
}

#[test]
fn test_content_addressed_install_name() {
    let literal: ShipConfig = toml::from_str("install-name = \"express\"\n").unwrap();
    let literal = literal.install_name.as_ref().unwrap();
    let default: ShipConfig =
        toml::from_str("install-name = { from = \"runtime-lock\" }\n").unwrap();
    let default = default.install_name.as_ref().unwrap();
    let explicit: ShipConfig =
        toml::from_str("install-name = { from = \"runtime-lock\", base = \"express\" }\n").unwrap();
    let explicit = explicit.install_name.as_ref().unwrap();

    let config = |version: &str| RuntimeStampConfig {
        runtime_version: Some(version.to_string()),
        ..RuntimeStampConfig::default()
    };
    let apply = |config: &mut RuntimeStampConfig, configured: &InstallNameConfig| {
        apply_install_name_config(config, Some(configured), None, "cx", "lock\n")
    };

    let mut literal_config = config("1.2.3");
    assert_eq!(apply(&mut literal_config, literal).unwrap(), None);
    assert_eq!(literal_config.install_name.as_deref(), Some("express"));

    let mut default_config = config("1.2.3");
    let source = apply(&mut default_config, default).unwrap();
    assert_eq!(source, Some(InstallNameSource::RuntimeLock));
    assert_eq!(
        default_config.install_name.as_deref(),
        Some("cx-1.2.3-d8c9f2728aa278eb")
    );

    let mut explicit_config = config("1.2.3");
    apply(&mut explicit_config, explicit).unwrap();
    assert_eq!(
        explicit_config.install_name.as_deref(),
        Some("express-1.2.3-d8c9f2728aa278eb")
    );

    let allowed_name = InstallNameConfig::Source(InstallNameSourceConfig {
        from: InstallNameSource::RuntimeLock,
        base: Some("a".repeat(109)),
    });
    let mut allowed = config("1");
    apply(&mut allowed, &allowed_name).unwrap();
    assert_eq!(allowed.install_name.unwrap().len(), 128);

    let too_long_name = InstallNameConfig::Source(InstallNameSourceConfig {
        from: InstallNameSource::RuntimeLock,
        base: Some("a".repeat(110)),
    });
    let mut too_long = config("1");
    let error = apply(&mut too_long, &too_long_name).unwrap_err();
    assert!(
        error.to_string().contains("at most 128 characters"),
        "{error}"
    );

    let mut unsafe_version = config("1.2.3+local");
    let error = apply(&mut unsafe_version, default).unwrap_err();
    assert!(
        error.to_string().contains("install name may only contain"),
        "{error}"
    );

    let mut update = config("1.2.3");
    update.update = Some(runtime_data::RuntimeUpdateConfig::new(
        "https://prefix.dev/demo".to_string(),
        "demo-runtime".to_string(),
        0,
    ));
    let source = apply(&mut update, default).unwrap();
    let error = validate_update_config(&update, BundleLayout::Online, source).unwrap_err();
    assert!(error.to_string().contains("cannot be combined"), "{error}");

    let overridden = apply_install_name_config(
        &mut update,
        Some(default),
        Some("stable-prefix".to_string()),
        "cx",
        "lock\n",
    )
    .unwrap();
    assert_eq!(update.install_name.as_deref(), Some("stable-prefix"));
    validate_update_config(&update, BundleLayout::Online, overridden).unwrap();
}

#[test]
fn test_manifest_runtime_version_is_validated() {
    let mut config = RuntimeStampConfig {
        runtime_version: Some("1.0\nmalicious".to_string()),
        ..RuntimeStampConfig::default()
    };

    let err = apply_runtime_metadata_overrides(&mut config, None, None, None)
        .expect_err("manifest runtime version should be validated");

    assert!(
        err.to_string().contains("runtime version may only contain"),
        "{err}"
    );
}

#[test]
fn test_manifest_docs_url_is_validated() {
    let mut config = RuntimeStampConfig {
        runtime_version: Some("1.0.0".to_string()),
        docs_url: Some("https://example.com/\nmalicious".to_string()),
        ..RuntimeStampConfig::default()
    };

    let err = apply_runtime_metadata_overrides(&mut config, None, None, None)
        .expect_err("manifest docs URL should be validated");

    assert!(
        err.to_string()
            .contains("docs URL must not contain whitespace"),
        "{err}"
    );
}

#[test]
fn test_runtime_version_is_required_for_build_metadata() {
    let mut config = RuntimeStampConfig::default();

    let err = apply_runtime_metadata_overrides(&mut config, None, None, None)
        .expect_err("missing runtime version should fail");

    assert!(
        err.to_string().contains("runtime version is required"),
        "{err}"
    );
}

#[test]
fn test_dynamic_project_version_mentions_project_metadata_source() {
    let mut config = RuntimeStampConfig {
        project_dynamic_version: true,
        ..RuntimeStampConfig::default()
    };

    let err = apply_runtime_metadata_overrides(&mut config, None, None, None)
        .expect_err("dynamic project version should require an explicit source");

    assert!(
        err.to_string().contains("project metadata resolution"),
        "{err}"
    );
}

#[test]
fn test_project_metadata_runtime_version_source_requires_adapter() {
    let mut config = RuntimeStampConfig {
        runtime_version_source: Some(RuntimeVersionSource::ProjectMetadata),
        project_dynamic_version: true,
        ..RuntimeStampConfig::default()
    };

    let err = apply_runtime_metadata_overrides(&mut config, None, None, None)
        .expect_err("project metadata source should require adapter resolution");

    assert!(
        err.to_string()
            .contains("must be resolved before invoking cs"),
        "{err}"
    );
    let diagnostic = err
        .downcast_ref::<ShipDiagnostic>()
        .expect("project metadata source should use a conda-ship diagnostic");
    assert_eq!(
        diagnostic.kind(),
        DiagnosticKind::ProjectMetadataRuntimeVersion
    );
}

#[test]
fn test_cli_runtime_version_overrides_project_metadata_source() {
    let mut config = RuntimeStampConfig {
        runtime_version_source: Some(RuntimeVersionSource::ProjectMetadata),
        project_dynamic_version: true,
        ..RuntimeStampConfig::default()
    };

    apply_runtime_metadata_overrides(&mut config, Some("4.5.6".to_string()), None, None).unwrap();

    assert_eq!(config.runtime_version.as_deref(), Some("4.5.6"));
    assert_eq!(config.runtime_version_source, None);
}

#[test]
fn test_resolve_runtime_name_uses_manifest_config() {
    let config = ShipConfig {
        runtime_name: Some("demo".to_string()),
        ..ShipConfig::default()
    };

    assert_eq!(resolve_runtime_name(None, &config).unwrap(), "demo");
    assert_eq!(
        resolve_runtime_name(Some("override".to_string()), &config).unwrap(),
        "override"
    );
}

#[test]
fn test_resolve_delegate_executable_uses_manifest_config() {
    let config = ShipConfig {
        delegate_executable: Some("python".to_string()),
        ..ShipConfig::default()
    };

    assert_eq!(
        resolve_delegate_executable(None, &config).unwrap(),
        "python"
    );
    assert_eq!(
        resolve_delegate_executable(Some("conda".to_string()), &config).unwrap(),
        "conda"
    );
}

#[test]
fn test_resolve_artifact_layout_uses_manifest_config() {
    let config = ShipConfig {
        artifact_layout: Some(BundleLayout::Embedded),
        ..ShipConfig::default()
    };

    assert_eq!(
        resolve_artifact_layout(None, &config),
        BundleLayout::Embedded
    );
    assert_eq!(
        resolve_artifact_layout(Some(BundleLayout::External), &config),
        BundleLayout::External
    );
    assert_eq!(
        resolve_artifact_layout(None, &ShipConfig::default()),
        BundleLayout::Online
    );
}

#[test]
fn test_build_accepts_dry_run() {
    let cli = Cli::try_parse_from(["cs", "build", "--runtime-name", "demo", "--dry-run"]).unwrap();

    let Command::Build { dry_run, .. } = cli.command else {
        panic!("expected build command");
    };

    assert!(dry_run);
}

#[test]
fn test_lock_subcommand_is_not_accepted() {
    let result = Cli::try_parse_from(["cs", "lock"]);

    assert!(result.is_err(), "cs lock should not be a public command");
}

#[test]
fn test_package_update_accepts_artifact_info() {
    let cli = Cli::try_parse_from([
        "cs",
        "package-update",
        "--info",
        "dist/demo.info.json",
        "--json",
    ])
    .unwrap();

    let Command::PackageUpdate { info, json, .. } = cli.command else {
        panic!("expected package-update command")
    };
    assert_eq!(info, Path::new("dist/demo.info.json"));
    assert!(json);
}

#[test]
fn test_build_rejects_path_option() {
    let result = Cli::try_parse_from([
        "cs",
        "build",
        "--runtime-name",
        "demo",
        "--path",
        "/tmp/demo",
    ]);

    assert!(result.is_err(), "build-time --path should not be accepted");
}

#[test]
fn test_run_accepts_install_path_for_smoke_test() {
    let cli = Cli::try_parse_from([
        "cs",
        "run",
        "--runtime-name",
        "demo",
        "--install-path",
        "/tmp/demo",
        "--",
        "info",
    ])
    .unwrap();

    let Command::Run {
        install_path, args, ..
    } = cli.command
    else {
        panic!("expected run command");
    };
    assert_eq!(install_path, Some(std::path::PathBuf::from("/tmp/demo")));
    assert_eq!(args, vec![std::ffi::OsString::from("info")]);
}

#[test]
fn test_run_accepts_installer() {
    let cli = Cli::try_parse_from([
        "cs",
        "run",
        "--artifact-name",
        "demoz",
        "--installer",
        "conda-forge",
        "--",
    ])
    .unwrap();

    let Command::Run {
        artifact_name,
        installer,
        ..
    } = cli.command
    else {
        panic!("expected run command");
    };

    assert_eq!(artifact_name.as_deref(), Some("demoz"));
    assert_eq!(installer.as_deref(), Some("conda-forge"));
}

#[rstest]
#[case::windows_target(Some("x86_64-pc-windows-msvc"), "demo.exe")]
#[case::unix_target(Some("x86_64-unknown-linux-gnu"), "demo")]
fn test_binary_filename(#[case] target: Option<&str>, #[case] expected: &str) {
    assert_eq!(binary_filename("demo", target), expected);
}

#[test]
fn test_binary_filename_for_current_target() {
    let expected = if cfg!(windows) { "demo.exe" } else { "demo" };

    assert_eq!(binary_filename("demo", None), expected);
}

#[rstest]
#[case::conda("python-3.12-h123_0.conda")]
#[case::tar_bz2("python-3.12-h123_0.tar.bz2")]
fn test_package_archive_name_accepts_conda_archives(#[case] name: &str) {
    assert!(validate_package_archive_name(name).is_ok());
}

#[rstest]
#[case::parent_dir("../python-3.12-h123_0.conda")]
#[case::nested("nested/python-3.12-h123_0.conda")]
#[case::wrong_suffix("python-3.12-h123_0.zip")]
fn test_package_archive_name_rejects_invalid_archives(#[case] name: &str) {
    assert!(validate_package_archive_name(name).is_err());
}

#[test]
fn test_runtime_env_var_sanitizes_artifact_name() {
    assert_eq!(
        runtime_data::runtime_env_var("demo-tool", "BUNDLE"),
        "DEMO_TOOL_BUNDLE"
    );
}

#[test]
fn test_render_package_list_is_tab_separated() {
    let packages = vec![PackageInfo {
        name: "python".to_string(),
        version: "3.12.0".to_string(),
        build: "h123_0".to_string(),
        url: "https://example.com/python.conda".to_string(),
        sha256: Some("abc123".to_string()),
    }];

    assert_eq!(
        render_package_list(&packages),
        "name\tversion\tbuild\turl\tsha256\npython\t3.12.0\th123_0\thttps://example.com/python.conda\tabc123\n"
    );
}

#[test]
fn test_render_cyclonedx_sbom_covers_resolved_conda_graph() {
    let python = make_sbom_pkg(
        "python",
        &["conda-forge::openssl >=3", "__glibc >=2.17"],
        0xaa,
        "Python-2.0",
    );
    let openssl = make_sbom_pkg("openssl", &[], 0xbb, "Apache-2.0");
    let packages = vec![&python, &openssl];

    let content = render_cyclonedx_sbom(
        "demo",
        "demo-cli",
        "1.2.3",
        BundleLayout::Online,
        Platform::Linux64,
        "demo-cli",
        &packages,
        "2026-08-19T12:00:00Z".to_string(),
    )
    .unwrap();
    let sbom: serde_json::Value = serde_json::from_str(&content).unwrap();

    assert_eq!(sbom["bomFormat"], "CycloneDX");
    assert_eq!(sbom["specVersion"], "1.7");
    assert_eq!(sbom["metadata"]["timestamp"], "2026-08-19T12:00:00Z");
    assert_eq!(sbom["metadata"]["component"]["name"], "demo-cli");
    assert!(sbom["metadata"]["component"]["hashes"].is_null());
    assert_eq!(sbom["components"].as_array().unwrap().len(), 2);
    assert_eq!(sbom["components"][0]["name"], "openssl");
    assert_eq!(sbom["components"][1]["name"], "python");
    assert_eq!(
        sbom["components"][1]["licenses"][0]["license"]["name"],
        "Python-2.0"
    );
    assert_eq!(
        sbom["components"][1]["hashes"][0]["content"],
        "aa".repeat(32)
    );
    assert_eq!(sbom["components"][1]["hashes"][1]["alg"], "MD5");
    assert_eq!(
        sbom["components"][1]["hashes"][1]["content"],
        "aa".repeat(16)
    );
    assert!(
        sbom["components"][1]["purl"]
            .as_str()
            .unwrap()
            .starts_with("pkg:conda/python@1.0?")
    );

    let python_ref = sbom["components"][1]["bom-ref"].as_str().unwrap();
    let openssl_ref = sbom["components"][0]["bom-ref"].as_str().unwrap();
    let dependencies = sbom["dependencies"].as_array().unwrap();
    let root = dependencies
        .iter()
        .find(|dependency| dependency["ref"].as_str().unwrap().starts_with("runtime:"))
        .unwrap();
    let python_dependency = dependencies
        .iter()
        .find(|dependency| dependency["ref"] == python_ref)
        .unwrap();
    let openssl_dependency = dependencies
        .iter()
        .find(|dependency| dependency["ref"] == openssl_ref)
        .unwrap();
    assert_eq!(root["dependsOn"], serde_json::json!([python_ref]));
    assert_eq!(
        python_dependency["dependsOn"],
        serde_json::json!([openssl_ref])
    );
    assert_eq!(openssl_dependency["dependsOn"], serde_json::json!([]));
    assert_eq!(sbom["compositions"][0]["aggregate"], "incomplete");
    assert_eq!(
        sbom["compositions"][0]["assemblies"],
        serde_json::json!([sbom["metadata"]["component"]["bom-ref"]])
    );
    let omitted_edges = sbom["metadata"]["component"]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["name"] == "conda:environment:dependency-edges-omitted")
        .unwrap();
    assert_eq!(omitted_edges["value"], "1");
    assert_eq!(sbom["compositions"][1]["aggregate"], "incomplete");
    assert!(sbom["compositions"][1]["assemblies"].is_null());
    assert_eq!(
        sbom["compositions"][1]["dependencies"],
        serde_json::json!([python_ref])
    );
}

#[test]
fn test_render_cyclonedx_sbom_marks_conditional_dependency_sets_unknown() {
    let parent = make_sbom_pkg("parent", &["optional[when=\"__win\"]"], 0xaa, "MIT");
    let optional = make_sbom_pkg("optional", &[], 0xbb, "MIT");
    let packages = vec![&parent, &optional];

    let content = render_cyclonedx_sbom(
        "demo",
        "demo",
        "1.0.0",
        BundleLayout::Online,
        Platform::Linux64,
        "demo",
        &packages,
        "2026-08-19T12:00:00Z".to_string(),
    )
    .unwrap();
    let sbom: serde_json::Value = serde_json::from_str(&content).unwrap();
    let components = sbom["components"].as_array().unwrap();
    let parent_ref = components
        .iter()
        .find(|component| component["name"] == "parent")
        .unwrap()["bom-ref"]
        .as_str()
        .unwrap();
    let parent_dependency = sbom["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|dependency| dependency["ref"] == parent_ref)
        .unwrap();

    assert_eq!(parent_dependency["dependsOn"], serde_json::json!([]));
    assert_eq!(sbom["compositions"][1]["aggregate"], "unknown");
    assert_eq!(
        sbom["compositions"][1]["dependencies"],
        serde_json::json!([parent_ref])
    );
    assert!(
        sbom["metadata"]["component"]["properties"]
            .as_array()
            .unwrap()
            .iter()
            .all(|property| property["name"] != "conda:environment:dependency-edges-omitted")
    );
}

#[test]
fn test_render_cyclonedx_sbom_sanitizes_package_sources() {
    let mut remote = make_sbom_pkg("remote", &[], 0xaa, "MIT")
        .into_binary()
        .unwrap();
    remote.location = "https://location-user:location-password@example.com/t/location-secret/conda-forge/linux-64/remote-1.0-h123_0.conda?location-query=private#location-fragment"
        .parse()
        .unwrap();
    remote.channel = Some(
        reqwest::Url::parse(
            "https://channel-user:channel-password@example.com/t/channel-secret/conda-forge?channel-query=private#channel-fragment",
        )
        .unwrap()
        .into(),
    );
    let remote = CondaPackageData::from(remote);

    let mut local = make_sbom_pkg("local", &[], 0xbb, "MIT")
        .into_binary()
        .unwrap();
    local.location = "/Users/alice/private-channel/local-1.0-h123_0.conda"
        .parse()
        .unwrap();
    local.channel = Some(
        reqwest::Url::parse("file:///Users/alice/private-channel/")
            .unwrap()
            .into(),
    );
    let local = CondaPackageData::from(local);
    let packages = vec![&remote, &local];

    let content = render_cyclonedx_sbom(
        "demo",
        "demo",
        "1.0.0",
        BundleLayout::Online,
        Platform::Linux64,
        "demo",
        &packages,
        "2026-08-19T12:00:00Z".to_string(),
    )
    .unwrap();
    let sbom: serde_json::Value = serde_json::from_str(&content).unwrap();
    let components = sbom["components"].as_array().unwrap();
    let remote = components
        .iter()
        .find(|component| component["name"] == "remote")
        .unwrap();
    let local = components
        .iter()
        .find(|component| component["name"] == "local")
        .unwrap();

    assert_eq!(
        remote["externalReferences"][0]["url"],
        "https://example.com/conda-forge/linux-64/remote-1.0-h123_0.conda"
    );
    assert_eq!(
        remote["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|property| property["name"] == "conda:package:channel")
            .unwrap()["value"],
        "https://example.com/conda-forge/"
    );
    assert!(local["externalReferences"].is_null());
    assert!(!local["purl"].as_str().unwrap().contains("channel="));
    assert!(
        !local["properties"]
            .as_array()
            .unwrap()
            .iter()
            .any(|property| property["name"] == "conda:package:channel")
    );
    for private in [
        "location-user",
        "location-password",
        "location-secret",
        "location-query",
        "location-fragment",
        "channel-user",
        "channel-password",
        "channel-secret",
        "channel-query",
        "channel-fragment",
        "/Users/alice/private-channel",
    ] {
        assert!(!content.contains(private), "SBOM exposed {private}");
    }
}

#[test]
fn test_render_cyclonedx_sbom_attaches_disconnected_cycles() {
    let root = make_sbom_pkg("root", &[], 0xaa, "MIT");
    let cycle_a = make_sbom_pkg("cycle-a", &["cycle-b"], 0xbb, "MIT");
    let cycle_b = make_sbom_pkg("cycle-b", &["cycle-a"], 0xcc, "MIT");
    let packages = vec![&root, &cycle_a, &cycle_b];

    let content = render_cyclonedx_sbom(
        "demo",
        "demo",
        "1.0.0",
        BundleLayout::Online,
        Platform::Linux64,
        "demo",
        &packages,
        "2026-08-19T12:00:00Z".to_string(),
    )
    .unwrap();
    let sbom: serde_json::Value = serde_json::from_str(&content).unwrap();
    let components = sbom["components"].as_array().unwrap();
    let component_ref = |name: &str| {
        components
            .iter()
            .find(|component| component["name"] == name)
            .unwrap()["bom-ref"]
            .as_str()
            .unwrap()
    };
    let dependencies = sbom["dependencies"].as_array().unwrap();
    let root_dependencies = dependencies
        .iter()
        .find(|dependency| dependency["ref"].as_str().unwrap().starts_with("runtime:"))
        .unwrap()["dependsOn"]
        .as_array()
        .unwrap();

    assert!(
        root_dependencies
            .iter()
            .any(|reference| reference == component_ref("root"))
    );
    assert!(
        root_dependencies
            .iter()
            .any(|reference| reference == component_ref("cycle-a"))
    );
    assert!(
        !root_dependencies
            .iter()
            .any(|reference| reference == component_ref("cycle-b"))
    );
}

#[test]
fn test_creation_timestamp_uses_source_date_epoch() {
    temp_env::with_var("SOURCE_DATE_EPOCH", Some("0"), || {
        assert_eq!(creation_timestamp().unwrap(), "1970-01-01T00:00:00Z");
    });
}
