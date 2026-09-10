use std::env;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use fs4::FileExt;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::Platform;
use rattler_lock::{CondaPackageData, LockFile};
use sha2::{Digest, Sha256};

use super::bundle::gen_bundle_from_lock;
use super::diagnostic::{DiagnosticKind, ship_error};
use super::project::{
    DerivedRuntimeLock, derive_runtime_lock, package_record, project_root,
    write_generated_runtime_lock,
};
use super::sbom::{creation_timestamp, render_cyclonedx_sbom};
use super::{
    BUNDLE_ARCHIVE_FILE, BundleLayout, InstallNameConfig, InstallNameSource, RUNTIME_LOCK_FILE,
    RUNTIME_TEMPLATE_ENV, RuntimeStampConfig, RuntimeVersionSource, SHIP_STATE_DIR, ShipConfig,
    runtime_data,
};

#[derive(Debug)]
pub(crate) struct BuildOutput {
    pub(crate) binary: PathBuf,
    pub(crate) bundle: Option<PathBuf>,
    pub(crate) info: PathBuf,
    pub(crate) checksums: PathBuf,
    pub(crate) lock: PathBuf,
    pub(crate) package_list: PathBuf,
    pub(crate) sbom: PathBuf,
}

#[derive(Debug)]
struct PlannedArtifactPaths {
    stem: String,
    binary: PathBuf,
    bundle: Option<PathBuf>,
    info: PathBuf,
    checksums: PathBuf,
    lock: PathBuf,
    package_list: PathBuf,
    sbom: PathBuf,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct ArtifactChecksum {
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct ArtifactInfo {
    pub(crate) schema_version: u8,
    pub(crate) name: String,
    pub(crate) artifact_name: String,
    pub(crate) runtime_name: String,
    pub(crate) runtime_version: String,
    pub(crate) layout: String,
    pub(crate) platform: String,
    pub(crate) update: Option<runtime_data::RuntimeUpdateConfig>,
    pub(crate) binary: String,
    pub(crate) bundle: Option<String>,
    pub(crate) lock: String,
    pub(crate) package_list: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sbom: Option<String>,
    pub(crate) package_count: usize,
    pub(crate) checksums: Vec<ArtifactChecksum>,
}

#[derive(serde::Serialize)]
struct InspectReport {
    project: InspectProject,
    runtime_input: InspectRuntimeInput,
    validation: InspectValidation,
    exclusions: InspectExclusions,
    selected_platform: String,
    selected_package_count: usize,
    platform_summary: Vec<PlatformSummary>,
    packages: Vec<PackageInfo>,
}

#[derive(serde::Serialize)]
struct InspectProject {
    root: String,
    manifest: String,
    manifest_kind: String,
    lockfile: String,
    source_environment: String,
}

#[derive(serde::Serialize)]
struct InspectRuntimeInput {
    channels: Vec<String>,
    packages: Vec<String>,
    package_count: usize,
    platform_count: usize,
}

#[derive(serde::Serialize)]
struct InspectValidation {
    source_lockfile: String,
    runtime_lock_derivation: String,
}

#[derive(serde::Serialize)]
struct InspectExclusions {
    configured: Vec<String>,
    removed: Vec<String>,
    removed_count: usize,
}

#[derive(serde::Serialize)]
struct PlatformSummary {
    platform: String,
    packages: usize,
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct PackageInfo {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) build: String,
    pub(crate) url: String,
    pub(crate) sha256: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn dry_run_build_artifact(
    artifact_layout: Option<BundleLayout>,
    runtime_name: Option<String>,
    artifact_name: Option<String>,
    delegate_executable: Option<String>,
    runtime_version: Option<String>,
    target_label: Option<String>,
    platform_str: Option<String>,
    target: Option<String>,
    template: Option<PathBuf>,
    docs_url: Option<String>,
    install_scheme: Option<runtime_data::InstallScheme>,
    install_name: Option<String>,
    installer: Option<String>,
    out_dir: PathBuf,
    root_override: Option<PathBuf>,
) -> miette::Result<()> {
    if let Some(label) = target_label.as_deref() {
        validate_target_label(label)?;
    }
    if let Some(target) = target.as_deref() {
        validate_target_triple(target)?;
    }
    let root = project_root(root_override.as_deref())?;
    let platform = parse_platform(platform_str)?;

    let mut derived = derive_runtime_lock(&root)?;
    let runtime = resolve_runtime_name(runtime_name, &derived.input.config)?;
    let delegate_executable =
        resolve_delegate_executable(delegate_executable, &derived.input.config)?;
    let layout = resolve_artifact_layout(artifact_layout, &derived.input.config);
    let artifact_name = resolve_artifact_name(&runtime, artifact_name, &derived.input.config);
    validate_runtime_name(&runtime)?;
    validate_artifact_name(&artifact_name)?;
    validate_delegate_executable(&delegate_executable)?;
    derived.runtime_config.delegate_executable = Some(delegate_executable.clone());
    apply_runtime_metadata_overrides(
        &mut derived.runtime_config,
        runtime_version,
        docs_url,
        installer,
    )?;
    let install_name_source = apply_install_name_config(
        &mut derived.runtime_config,
        derived.input.config.install_name.as_ref(),
        install_name,
        &runtime,
        &derived.content,
    )?;
    validate_update_config(&derived.runtime_config, layout, install_name_source)?;
    derived.runtime_config.install_scheme =
        install_scheme.or(derived.runtime_config.install_scheme);

    let runtime_lock_path = generated_runtime_lock_path(&root);
    let packages = packages_for_platform(&derived.lock_file, &runtime_lock_path, platform)?;
    if layout.needs_bundle() {
        validate_bundle_package_hashes(&packages)?;
    }

    let template_source = source_binary_plan(template.as_deref(), target.as_deref())?;
    let out_dir = resolve_out_dir(&root, &out_dir);
    let paths = planned_artifact_paths(
        &out_dir,
        &artifact_name,
        layout,
        target_label.as_deref(),
        target.as_deref(),
    );
    print_build_dry_run(
        &root,
        &derived,
        layout,
        &runtime,
        &artifact_name,
        platform,
        target.as_deref(),
        &template_source,
        &out_dir,
        &paths,
        packages.len(),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_artifact(
    artifact_layout: Option<BundleLayout>,
    runtime_name: Option<String>,
    artifact_name: Option<String>,
    delegate_executable: Option<String>,
    runtime_version: Option<String>,
    target_label: Option<String>,
    platform_str: Option<String>,
    target: Option<String>,
    template: Option<PathBuf>,
    docs_url: Option<String>,
    install_scheme: Option<runtime_data::InstallScheme>,
    install_name: Option<String>,
    installer: Option<String>,
    out_dir: PathBuf,
    root_override: Option<PathBuf>,
) -> miette::Result<BuildOutput> {
    if let Some(label) = target_label.as_deref() {
        validate_target_label(label)?;
    }
    if let Some(target) = target.as_deref() {
        validate_target_triple(target)?;
    }
    let root = project_root(root_override.as_deref())?;
    let platform = parse_platform(platform_str)?;

    let mut derived = derive_runtime_lock(&root)?;
    let runtime = resolve_runtime_name(runtime_name, &derived.input.config)?;
    let delegate_executable =
        resolve_delegate_executable(delegate_executable, &derived.input.config)?;
    let layout = resolve_artifact_layout(artifact_layout, &derived.input.config);
    let artifact_name = resolve_artifact_name(&runtime, artifact_name, &derived.input.config);
    validate_runtime_name(&runtime)?;
    validate_artifact_name(&artifact_name)?;
    validate_delegate_executable(&delegate_executable)?;
    derived.runtime_config.delegate_executable = Some(delegate_executable);
    apply_runtime_metadata_overrides(
        &mut derived.runtime_config,
        runtime_version,
        docs_url,
        installer,
    )?;
    let install_name_source = apply_install_name_config(
        &mut derived.runtime_config,
        derived.input.config.install_name.as_ref(),
        install_name,
        &runtime,
        &derived.content,
    )?;
    validate_update_config(&derived.runtime_config, layout, install_name_source)?;
    derived.runtime_config.install_scheme =
        install_scheme.or(derived.runtime_config.install_scheme);
    let runtime_lock_path = generated_runtime_lock_path(&root);
    write_generated_runtime_lock(&runtime_lock_path, &derived.content)?;

    let generated_bundle = if layout.needs_bundle() {
        Some(gen_bundle_from_lock(
            &derived.lock_file,
            &runtime_lock_path,
            platform,
            &generated_bundle_path(&root),
        )?)
    } else {
        None
    };

    let source_binary = source_binary(template.as_deref(), target.as_deref())?;

    stage_artifacts(
        &root,
        &source_binary,
        layout,
        &runtime,
        &artifact_name,
        target_label.as_deref(),
        platform,
        target.as_deref(),
        &out_dir,
        &derived,
        generated_bundle.as_deref(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_artifact(
    artifact_layout: Option<BundleLayout>,
    runtime_name: Option<String>,
    artifact_name: Option<String>,
    delegate_executable: Option<String>,
    runtime_version: Option<String>,
    platform: Option<String>,
    out_dir: PathBuf,
    install_path: Option<PathBuf>,
    template: Option<PathBuf>,
    docs_url: Option<String>,
    install_scheme: Option<runtime_data::InstallScheme>,
    install_name: Option<String>,
    installer: Option<String>,
    root_override: Option<PathBuf>,
    args: Vec<OsString>,
) -> miette::Result<()> {
    let root = project_root(root_override.as_deref())?;
    let derived = derive_runtime_lock(&root)?;
    let runtime = resolve_runtime_name(runtime_name, &derived.input.config)?;
    let layout = resolve_artifact_layout(artifact_layout, &derived.input.config);
    let bundle_env_var = runtime_data::runtime_env_var(&runtime, "BUNDLE");
    let bundle_dir = root.join(SHIP_STATE_DIR).join("bundle");
    let output = build_artifact(
        Some(layout),
        Some(runtime),
        artifact_name,
        delegate_executable,
        runtime_version,
        None,
        platform,
        None,
        template,
        docs_url,
        install_scheme,
        install_name,
        installer,
        out_dir,
        Some(root.clone()),
    )?;

    let mut command = std::process::Command::new(&output.binary);
    command.args(args);
    if let Some(install_path) = install_path {
        command.env("CONDA_SHIP_PREFIX", install_path);
    }
    if layout == BundleLayout::External {
        command.env(bundle_env_var, bundle_dir);
    }

    let status = command
        .status()
        .into_diagnostic()
        .with_context(|| format!("failed to run {}", output.binary.display()))?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

pub(crate) fn resolve_runtime_name(
    runtime_name: Option<String>,
    config: &ShipConfig,
) -> miette::Result<String> {
    runtime_name.or_else(|| config.runtime_name.clone()).ok_or_else(|| {
        ship_error(
            DiagnosticKind::MissingRuntime,
            "runtime name is required; pass --runtime-name or set [tool.conda-ship].runtime-name",
            Some(
                "Pass --runtime-name to this command or set [tool.conda-ship].runtime-name in the source manifest."
                    .to_string(),
            ),
        )
    })
}

pub(crate) fn resolve_delegate_executable(
    delegate_executable: Option<String>,
    config: &ShipConfig,
) -> miette::Result<String> {
    delegate_executable
        .or_else(|| config.delegate_executable.clone())
        .ok_or_else(|| {
            ship_error(
                DiagnosticKind::MissingDelegate,
                "delegate executable is required; pass --delegate-executable or set [tool.conda-ship].delegate-executable",
                Some(
                    "Pass --delegate-executable to this command or set [tool.conda-ship].delegate-executable in the source manifest."
                        .to_string(),
                ),
            )
        })
}

pub(crate) fn resolve_artifact_layout(
    artifact_layout: Option<BundleLayout>,
    config: &ShipConfig,
) -> BundleLayout {
    artifact_layout
        .or(config.artifact_layout)
        .unwrap_or(BundleLayout::Online)
}

pub(crate) fn resolve_artifact_name(
    runtime: &str,
    artifact_name: Option<String>,
    config: &ShipConfig,
) -> String {
    artifact_name
        .or_else(|| config.artifact_name.clone())
        .unwrap_or_else(|| runtime.to_string())
}

pub(crate) fn inspect_artifact(
    platform_str: Option<String>,
    json: bool,
    root_override: Option<PathBuf>,
) -> miette::Result<()> {
    let root = project_root(root_override.as_deref())?;
    let derived = derive_runtime_lock(&root)?;
    let platform = parse_platform(platform_str)?;
    let runtime_lock_path = generated_runtime_lock_path(&root);

    let summaries = platform_summaries(&derived.lock_file, &runtime_lock_path)?;
    let packages = packages_for_platform(&derived.lock_file, &runtime_lock_path, platform)?;
    let package_infos = package_infos(&packages)?;

    if json {
        let inspect = inspect_report(&root, &derived, platform, summaries, package_infos);
        let mut output = serde_json::to_string_pretty(&inspect)
            .into_diagnostic()
            .context("failed to render inspect JSON")?;
        output.push('\n');
        write_stdout(|stdout| stdout.write_all(output.as_bytes()))?;
        return Ok(());
    }

    write_stdout(|stdout| {
        print_inspect_report(
            stdout,
            &root,
            &derived,
            platform,
            &summaries,
            &package_infos,
        )
    })?;
    Ok(())
}

fn write_stdout<F>(write: F) -> miette::Result<()>
where
    F: FnOnce(&mut dyn io::Write) -> io::Result<()>,
{
    let mut stdout = io::stdout().lock();
    match write(&mut stdout).and_then(|()| stdout.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error)
            .into_diagnostic()
            .context("failed to write output to stdout"),
    }
}

fn inspect_report(
    root: &Path,
    derived: &DerivedRuntimeLock,
    platform: Platform,
    summaries: Vec<PlatformSummary>,
    package_infos: Vec<PackageInfo>,
) -> InspectReport {
    InspectReport {
        project: InspectProject {
            root: root.display().to_string(),
            manifest: display_path(root, &derived.input.manifest_path),
            manifest_kind: derived.input.manifest_kind.manifest_label().to_string(),
            lockfile: display_path(root, &derived.input.lock_path),
            source_environment: derived.source_environment.clone(),
        },
        runtime_input: InspectRuntimeInput {
            channels: derived.runtime_config.channels.clone(),
            packages: derived.runtime_config.packages.clone(),
            package_count: derived.total_packages,
            platform_count: derived.platforms.len(),
        },
        validation: InspectValidation {
            source_lockfile: "ok".to_string(),
            runtime_lock_derivation: "ok".to_string(),
        },
        exclusions: InspectExclusions {
            configured: derived.runtime_config.exclude_packages.clone(),
            removed: derived.removed_excludes.clone(),
            removed_count: derived.total_excluded,
        },
        selected_platform: platform.to_string(),
        selected_package_count: package_infos.len(),
        platform_summary: summaries,
        packages: package_infos,
    }
}

fn print_inspect_report(
    writer: &mut dyn io::Write,
    root: &Path,
    derived: &DerivedRuntimeLock,
    platform: Platform,
    summaries: &[PlatformSummary],
    package_infos: &[PackageInfo],
) -> io::Result<()> {
    writeln!(writer, "Project")?;
    print_project_summary(writer, root, derived)?;
    writeln!(writer)?;
    writeln!(writer, "Runtime input")?;
    writeln!(
        writer,
        "  source environment: {}",
        derived.source_environment
    )?;
    writeln!(writer, "  platforms: {}", platform_list(&derived.platforms))?;
    writeln!(
        writer,
        "  channels: {}",
        string_list(&derived.runtime_config.channels)
    )?;
    writeln!(writer, "  packages: {}", derived.total_packages)?;
    writeln!(writer)?;
    writeln!(writer, "Validation")?;
    writeln!(writer, "  source lockfile: ok")?;
    writeln!(writer, "  runtime lock derivation: ok")?;
    writeln!(writer)?;
    writeln!(writer, "Exclusions")?;
    writeln!(
        writer,
        "  configured: {}",
        string_list(&derived.runtime_config.exclude_packages)
    )?;
    writeln!(
        writer,
        "  removed: {}",
        string_list(&derived.removed_excludes)
    )?;
    writeln!(
        writer,
        "  removed package entries: {}",
        derived.total_excluded
    )?;
    writeln!(writer)?;
    writeln!(writer, "Platforms")?;
    for summary in summaries {
        writeln!(
            writer,
            "  {}: {} packages",
            summary.platform, summary.packages
        )?;
    }
    writeln!(writer)?;
    writeln!(writer, "Selected platform: {platform}")?;
    writeln!(writer, "Packages: {}", package_infos.len())?;
    for package in package_infos {
        writeln!(
            writer,
            "  {} {} {}",
            package.name, package.version, package.build
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn print_build_dry_run(
    root: &Path,
    derived: &DerivedRuntimeLock,
    layout: BundleLayout,
    runtime: &str,
    artifact_name: &str,
    platform: Platform,
    target: Option<&str>,
    template_source: &RuntimeTemplateSource,
    out_dir: &Path,
    paths: &PlannedArtifactPaths,
    selected_package_count: usize,
) -> miette::Result<()> {
    let install_scheme = derived.runtime_config.install_scheme.unwrap_or_default();
    let install_name = derived
        .runtime_config
        .install_name
        .as_deref()
        .unwrap_or(runtime);
    let delegate_executable = derived
        .runtime_config
        .delegate_executable
        .as_deref()
        .ok_or_else(|| {
            miette::miette!("delegate executable has not been resolved before dry-run output")
        })?;
    let docs_url = derived
        .runtime_config
        .docs_url
        .clone()
        .unwrap_or_else(default_docs_url);
    let runtime_version = derived
        .runtime_config
        .runtime_version
        .as_deref()
        .ok_or_else(|| missing_runtime_version(derived.runtime_config.project_dynamic_version))?;

    write_stdout(|writer| {
        writeln!(writer, "Build dry run")?;
        writeln!(writer, "Project")?;
        print_project_summary(writer, root, derived)?;
        writeln!(writer)?;
        writeln!(writer, "Runtime")?;
        writeln!(writer, "  runtime name: {runtime}")?;
        if artifact_name != runtime {
            writeln!(writer, "  artifact name: {artifact_name}")?;
        }
        writeln!(writer, "  version: {runtime_version}")?;
        writeln!(writer, "  delegate executable: {delegate_executable}")?;
        writeln!(writer, "  artifact layout: {}", layout.as_str())?;
        writeln!(writer, "  platform: {platform}")?;
        writeln!(writer, "  target: {}", target.unwrap_or("current"))?;
        writeln!(
            writer,
            "  install scheme: {}",
            install_scheme_name(install_scheme)
        )?;
        writeln!(writer, "  install name: {install_name}")?;
        if let Some(installer) = derived.runtime_config.installer.as_deref() {
            writeln!(writer, "  installer: {installer}")?;
        }
        writeln!(writer, "  docs URL: {docs_url}")?;
        writeln!(writer, "  packages: {selected_package_count}")?;
        writeln!(writer)?;
        writeln!(writer, "Template")?;
        writeln!(writer, "  source: {}", template_source.label())?;
        writeln!(
            writer,
            "  path: {}",
            display_path(root, template_source.path())
        )?;
        writeln!(writer)?;
        writeln!(writer, "Artifacts")?;
        writeln!(
            writer,
            "  output directory: {}",
            display_path(root, out_dir)
        )?;
        writeln!(writer, "  binary: {}", display_path(root, &paths.binary))?;
        if let Some(bundle) = &paths.bundle {
            writeln!(writer, "  bundle: {}", display_path(root, bundle))?;
        }
        writeln!(writer, "  info: {}", display_path(root, &paths.info))?;
        writeln!(
            writer,
            "  runtime lock: {}",
            display_path(root, &paths.lock)
        )?;
        writeln!(
            writer,
            "  packages: {}",
            display_path(root, &paths.package_list)
        )?;
        writeln!(writer, "  sbom: {}", display_path(root, &paths.sbom))?;
        writeln!(
            writer,
            "  checksums: {}",
            display_path(root, &paths.checksums)
        )?;
        writeln!(writer)?;
        writeln!(writer, "No files written.")
    })
}

fn print_project_summary(
    writer: &mut dyn io::Write,
    root: &Path,
    derived: &DerivedRuntimeLock,
) -> io::Result<()> {
    writeln!(writer, "  root: {}", root.display())?;
    writeln!(
        writer,
        "  manifest: {}",
        display_path(root, &derived.input.manifest_path)
    )?;
    writeln!(
        writer,
        "  manifest kind: {}",
        derived.input.manifest_kind.manifest_label()
    )?;
    writeln!(
        writer,
        "  source lockfile: {}",
        display_path(root, &derived.input.lock_path)
    )
}

pub(crate) fn validate_bundle_package_hashes(packages: &[&CondaPackageData]) -> miette::Result<()> {
    let mut missing = Vec::new();
    for pkg in packages {
        let record = package_record(pkg)?;
        if record.sha256.is_none() {
            missing.push(record.name.as_normalized().to_string());
        }
    }
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        return Err(ship_error(
            DiagnosticKind::MissingSha256,
            format!(
                "cannot bundle packages without SHA256 hashes in the source lockfile: {}",
                missing.join(", ")
            ),
            Some(
                "Refresh the source lockfile with package hash metadata before building an external or embedded layout."
                    .to_string(),
            ),
        ));
    }
    Ok(())
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn string_list(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}

fn platform_list(values: &[Platform]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        let mut platforms: Vec<_> = values.iter().map(ToString::to_string).collect();
        platforms.sort();
        platforms.join(", ")
    }
}

fn install_scheme_name(scheme: runtime_data::InstallScheme) -> &'static str {
    match scheme {
        runtime_data::InstallScheme::CondaHome => "conda-home",
        runtime_data::InstallScheme::UserData => "user-data",
    }
}

fn default_docs_url() -> String {
    "https://conda-incubator.github.io/conda-ship/".to_string()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stage_artifacts(
    root: &Path,
    source_binary: &Path,
    layout: BundleLayout,
    runtime: &str,
    artifact_name: &str,
    target_label: Option<&str>,
    platform: Platform,
    target: Option<&str>,
    out_dir: &Path,
    derived: &DerivedRuntimeLock,
    generated_bundle: Option<&Path>,
) -> miette::Result<BuildOutput> {
    let out_dir = resolve_out_dir(root, out_dir);
    std::fs::create_dir_all(&out_dir)
        .into_diagnostic()
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    let paths = planned_artifact_paths(&out_dir, artifact_name, layout, target_label, target);
    let staging = create_artifact_staging_dir(&out_dir)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to create staging directory in {}",
                out_dir.display()
            )
        })?;
    let staged_paths =
        planned_artifact_paths(staging.path(), artifact_name, layout, target_label, target);
    std::fs::copy(source_binary, &staged_paths.binary)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_binary.display(),
                staged_paths.binary.display()
            )
        })?;
    stamp_runtime_data(
        &staged_paths.binary,
        layout,
        platform,
        runtime,
        artifact_name,
        derived,
        generated_bundle,
    )?;
    ad_hoc_sign_macos(&staged_paths.binary, platform)?;

    let staged_bundle = if layout == BundleLayout::External {
        let source_bundle = generated_bundle
            .ok_or_else(|| miette::miette!("external builds require a generated bundle"))?;
        let staged_bundle = staged_paths
            .bundle
            .as_ref()
            .ok_or_else(|| miette::miette!("external builds have a planned bundle path"))?;
        std::fs::copy(source_bundle, staged_bundle)
            .into_diagnostic()
            .with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source_bundle.display(),
                    staged_bundle.display()
                )
            })?;
        Some(staged_bundle.clone())
    } else {
        None
    };

    let metadata = write_artifact_metadata(
        root,
        staging.path(),
        &staged_paths.stem,
        runtime,
        artifact_name,
        layout,
        platform,
        &staged_paths.binary,
        staged_bundle.as_deref(),
        derived,
    )?;

    publish_staged_artifact_set(
        &out_dir,
        staging.path(),
        &staged_paths,
        staged_bundle.as_deref(),
        &metadata,
        &paths,
    )?;

    let bundle = paths.bundle.clone();

    eprintln!("staged {}", paths.binary.display());
    if let Some(bundle) = &bundle {
        eprintln!("staged {}", bundle.display());
    }
    eprintln!("wrote {}", paths.sbom.display());
    eprintln!("wrote {}", paths.info.display());
    eprintln!("wrote {}", paths.checksums.display());

    Ok(BuildOutput {
        binary: paths.binary,
        bundle,
        info: paths.info,
        checksums: paths.checksums,
        lock: paths.lock,
        package_list: paths.package_list,
        sbom: paths.sbom,
    })
}

fn create_artifact_staging_dir(out_dir: &Path) -> io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".conda-ship-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(out_dir)
}

struct ArtifactPublicationLock {
    _file: File,
}

impl ArtifactPublicationLock {
    fn acquire(out_dir: &Path, stem: &str) -> miette::Result<Self> {
        let path = out_dir.join(format!(".{stem}.conda-ship.publish.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .into_diagnostic()
            .with_context(|| format!("failed to open publication lock at {}", path.display()))?;
        FileExt::lock(&file)
            .into_diagnostic()
            .with_context(|| format!("failed to acquire publication lock at {}", path.display()))?;
        Ok(Self { _file: file })
    }
}

fn publish_staged_artifact_set(
    out_dir: &Path,
    staging_dir: &Path,
    staged_paths: &PlannedArtifactPaths,
    staged_bundle: Option<&Path>,
    metadata: &ArtifactMetadataPaths,
    paths: &PlannedArtifactPaths,
) -> miette::Result<()> {
    let _publication_lock = ArtifactPublicationLock::acquire(out_dir, &paths.stem)?;
    retire_completion_marker(&paths.checksums, staging_dir)?;
    if paths.bundle.is_none() {
        retire_obsolete_bundle(&bundle_path(out_dir, &paths.stem), staging_dir)?;
    }

    if let (Some(staged), Some(published)) = (staged_bundle, paths.bundle.as_deref()) {
        publish_staged_file(staged, published)?;
    }
    publish_staged_file(&metadata.lock, &paths.lock)?;
    publish_staged_file(&metadata.package_list, &paths.package_list)?;
    publish_staged_file(&metadata.sbom, &paths.sbom)?;
    publish_staged_file(&staged_paths.binary, &paths.binary)?;
    publish_staged_file(&metadata.info, &paths.info)?;
    publish_staged_file(&metadata.checksums, &paths.checksums)
}

fn retire_obsolete_bundle(bundle: &Path, staging_dir: &Path) -> miette::Result<()> {
    let retired = staging_dir.join(".previous-bundle");
    match std::fs::symlink_metadata(bundle) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(miette::miette!(
                "obsolete external bundle is not a regular file: {}",
                bundle.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).into_diagnostic().with_context(|| {
                format!(
                    "failed to inspect the obsolete external bundle at {}",
                    bundle.display()
                )
            });
        }
    }
    match std::fs::rename(bundle, &retired) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).into_diagnostic().with_context(|| {
            format!(
                "failed to retire the obsolete external bundle at {}",
                bundle.display()
            )
        }),
    }
}

fn retire_completion_marker(checksums: &Path, staging_dir: &Path) -> miette::Result<()> {
    let retired = staging_dir.join(".previous-sha256");
    match std::fs::symlink_metadata(checksums) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(miette::miette!(
                "completion marker is not a regular file: {}",
                checksums.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).into_diagnostic().with_context(|| {
                format!(
                    "failed to inspect the previous completion marker at {}",
                    checksums.display()
                )
            });
        }
    }
    match std::fs::rename(checksums, &retired) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).into_diagnostic().with_context(|| {
            format!(
                "failed to retire the previous completion marker at {}",
                checksums.display()
            )
        }),
    }
}

fn ad_hoc_sign_macos(binary: &Path, platform: Platform) -> miette::Result<()> {
    #[cfg(target_os = "macos")]
    if platform.is_osx() {
        let status = std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(binary)
            .status()
            .into_diagnostic()
            .with_context(|| format!("failed to run codesign for {}", binary.display()))?;
        if !status.success() {
            return Err(miette::miette!(
                "codesign failed for {} with status {status}",
                binary.display()
            ));
        }
        let status = std::process::Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(binary)
            .status()
            .into_diagnostic()
            .with_context(|| format!("failed to verify codesign for {}", binary.display()))?;
        if !status.success() {
            return Err(miette::miette!(
                "codesign verification failed for {} with status {status}",
                binary.display()
            ));
        }
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (binary, platform);

    Ok(())
}

fn publish_staged_file(staged: &Path, published: &Path) -> miette::Result<()> {
    tempfile::TempPath::try_from_path(staged.to_path_buf())
        .into_diagnostic()
        .with_context(|| format!("failed to prepare {} for publication", staged.display()))?
        .persist(published)
        .map_err(|error| error.error)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to publish {} as {}",
                staged.display(),
                published.display()
            )
        })
}

fn planned_artifact_paths(
    out_dir: &Path,
    name: &str,
    layout: BundleLayout,
    target_label: Option<&str>,
    target: Option<&str>,
) -> PlannedArtifactPaths {
    let stem = artifact_stem(name, target_label);
    let binary = out_dir.join(binary_filename(&stem, target));
    let bundle = (layout == BundleLayout::External).then(|| bundle_path(out_dir, &stem));
    PlannedArtifactPaths {
        info: out_dir.join(format!("{stem}.info.json")),
        checksums: out_dir.join(format!("{stem}.sha256")),
        lock: out_dir.join(format!("{stem}.runtime.lock")),
        package_list: out_dir.join(format!("{stem}.packages.txt")),
        sbom: out_dir.join(format!("{stem}.cdx.json")),
        stem,
        binary,
        bundle,
    }
}

fn bundle_path(out_dir: &Path, stem: &str) -> PathBuf {
    out_dir.join(format!("{stem}.bundle.tar.zst"))
}

#[derive(Debug)]
struct ArtifactMetadataPaths {
    info: PathBuf,
    checksums: PathBuf,
    lock: PathBuf,
    package_list: PathBuf,
    sbom: PathBuf,
}

#[allow(clippy::too_many_arguments)]
fn write_artifact_metadata(
    root: &Path,
    out_dir: &Path,
    stem: &str,
    runtime: &str,
    artifact_name: &str,
    layout: BundleLayout,
    platform: Platform,
    binary: &Path,
    bundle: Option<&Path>,
    derived: &DerivedRuntimeLock,
) -> miette::Result<ArtifactMetadataPaths> {
    let runtime_lock_path = generated_runtime_lock_path(root);
    let packages = packages_for_platform(&derived.lock_file, &runtime_lock_path, platform)?;
    let package_infos = package_infos(&packages)?;

    let lock = out_dir.join(format!("{stem}.runtime.lock"));
    std::fs::write(&lock, &derived.content)
        .into_diagnostic()
        .with_context(|| format!("failed to write {}", lock.display()))?;

    let package_list = out_dir.join(format!("{stem}.packages.txt"));
    let package_list_content = render_package_list(&package_infos);
    std::fs::write(&package_list, package_list_content)
        .into_diagnostic()
        .with_context(|| format!("failed to write {}", package_list.display()))?;

    let runtime_version = derived
        .runtime_config
        .runtime_version
        .clone()
        .ok_or_else(|| missing_runtime_version(derived.runtime_config.project_dynamic_version))?;
    let binary_checksum = checksum_for_path(binary)?;
    let sbom = out_dir.join(format!("{stem}.cdx.json"));
    let sbom_content = render_cyclonedx_sbom(
        runtime,
        artifact_name,
        &runtime_version,
        layout,
        platform,
        &file_name(binary)?,
        &packages,
        creation_timestamp()?,
    )?;
    std::fs::write(&sbom, sbom_content)
        .into_diagnostic()
        .with_context(|| format!("failed to write {}", sbom.display()))?;

    let mut checksums = vec![
        binary_checksum,
        checksum_for_path(&lock)?,
        checksum_for_path(&package_list)?,
        checksum_for_path(&sbom)?,
    ];
    if let Some(bundle) = bundle {
        checksums.push(checksum_for_path(bundle)?);
    }

    let info = out_dir.join(format!("{stem}.info.json"));
    let info_doc = ArtifactInfo {
        schema_version: 1,
        name: stem.to_string(),
        artifact_name: artifact_name.to_string(),
        runtime_name: runtime.to_string(),
        runtime_version,
        layout: layout.as_str().to_string(),
        platform: platform.to_string(),
        update: derived.runtime_config.update.clone(),
        binary: file_name(binary)?,
        bundle: bundle.map(file_name).transpose()?,
        lock: file_name(&lock)?,
        package_list: file_name(&package_list)?,
        sbom: Some(file_name(&sbom)?),
        package_count: package_infos.len(),
        checksums,
    };
    let info_json = serde_json::to_string_pretty(&info_doc)
        .into_diagnostic()
        .context("failed to render info JSON")?;
    std::fs::write(&info, format!("{info_json}\n"))
        .into_diagnostic()
        .with_context(|| format!("failed to write {}", info.display()))?;

    let mut final_checksums = info_doc.checksums;
    final_checksums.push(checksum_for_path(&info)?);
    final_checksums.sort_by(|a, b| a.path.cmp(&b.path));

    let checksums = out_dir.join(format!("{stem}.sha256"));
    write_checksums(&checksums, &final_checksums)?;

    Ok(ArtifactMetadataPaths {
        info,
        checksums,
        lock,
        package_list,
        sbom,
    })
}

pub(crate) fn generated_runtime_lock_path(root: &Path) -> PathBuf {
    root.join(SHIP_STATE_DIR).join(RUNTIME_LOCK_FILE)
}

pub(crate) fn generated_bundle_path(root: &Path) -> PathBuf {
    root.join(SHIP_STATE_DIR).join(BUNDLE_ARCHIVE_FILE)
}

pub(crate) fn validate_package_archive_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("archive name must not be empty");
    }
    if name == "." || name == ".." {
        return Err("archive name must not be . or ..");
    }
    if name.contains('/') || name.contains('\\') || name.chars().any(char::is_control) {
        return Err("archive name must be a plain filename");
    }
    if !(name.ends_with(".conda") || name.ends_with(".tar.bz2")) {
        return Err("archive name must end with .conda or .tar.bz2");
    }
    Ok(())
}

pub(crate) fn parse_platform(platform_str: Option<String>) -> miette::Result<Platform> {
    if let Some(ref platform) = platform_str {
        platform.parse::<Platform>().map_err(|_| {
            ship_error(
                DiagnosticKind::InvalidPlatform,
                format!("invalid platform: {platform}"),
                Some("Use a conda platform such as linux-64, osx-arm64, or win-64.".to_string()),
            )
        })
    } else {
        Ok(Platform::current())
    }
}

pub(crate) fn validate_runtime_name(runtime: &str) -> miette::Result<()> {
    validate_artifact_component("runtime name", runtime)
}

pub(crate) fn validate_artifact_name(artifact_name: &str) -> miette::Result<()> {
    validate_artifact_component("artifact name", artifact_name)
}

pub(crate) fn validate_delegate_executable(delegate_executable: &str) -> miette::Result<()> {
    validate_artifact_component("delegate executable", delegate_executable)
}

pub(crate) fn validate_target_label(label: &str) -> miette::Result<()> {
    validate_artifact_component("target label", label)
}

pub(crate) fn validate_target_triple(target: &str) -> miette::Result<()> {
    validate_artifact_component("target triple", target)
}

const MAX_CONTENT_ADDRESSED_INSTALL_NAME_LEN: usize = 128;

pub(crate) fn apply_install_name_config(
    config: &mut RuntimeStampConfig,
    configured: Option<&InstallNameConfig>,
    install_name_override: Option<String>,
    runtime: &str,
    lock_content: &str,
) -> miette::Result<Option<InstallNameSource>> {
    let (install_name, source) = match (install_name_override, configured) {
        (Some(install_name), _) => (install_name, None),
        (None, Some(InstallNameConfig::Value(install_name))) => (install_name.clone(), None),
        (None, None) => (runtime.to_string(), None),
        (None, Some(InstallNameConfig::Source(source))) => {
            let version = config
                .runtime_version
                .as_deref()
                .ok_or_else(|| missing_runtime_version(config.project_dynamic_version))?;
            let base = source.base.as_deref().unwrap_or(runtime);
            let digest = Sha256::digest(lock_content.as_bytes());
            let install_name = format!("{base}-{version}-{}", crate::hash::hex(&digest[..8]));
            if install_name.len() > MAX_CONTENT_ADDRESSED_INSTALL_NAME_LEN {
                return Err(miette::miette!(
                    "content-addressed install name must be at most {MAX_CONTENT_ADDRESSED_INSTALL_NAME_LEN} characters. Shorten install-name base or runtime-version"
                ));
            }
            (install_name, Some(source.from))
        }
    };
    validate_install_name(&install_name)?;
    config.install_name = Some(install_name);
    Ok(source)
}

pub(crate) fn apply_runtime_metadata_overrides(
    config: &mut RuntimeStampConfig,
    runtime_version: Option<String>,
    docs_url: Option<String>,
    installer: Option<String>,
) -> miette::Result<()> {
    if let Some(runtime_version) = runtime_version {
        validate_runtime_version(&runtime_version)?;
        config.runtime_version = Some(runtime_version);
        config.runtime_version_source = None;
    } else if config.runtime_version.is_none() {
        match config.runtime_version_source.take() {
            Some(RuntimeVersionSource::ProjectMetadata) => {
                return Err(project_metadata_runtime_version_requires_adapter());
            }
            None => return Err(missing_runtime_version(config.project_dynamic_version)),
        }
    }
    if let Some(runtime_version) = config.runtime_version.as_deref() {
        validate_runtime_version(runtime_version)?;
    }
    if let Some(docs_url) = docs_url {
        validate_docs_url(&docs_url)?;
        config.docs_url = Some(docs_url);
    }
    if let Some(docs_url) = config.docs_url.as_deref() {
        validate_docs_url(docs_url)?;
    }
    if let Some(installer) = installer {
        validate_installer(&installer)?;
        config.installer = Some(installer);
    }
    validate_installer_config(config)
}

fn project_metadata_runtime_version_requires_adapter() -> miette::Report {
    ship_error(
        DiagnosticKind::ProjectMetadataRuntimeVersion,
        "runtime version from project metadata must be resolved before invoking cs",
        Some(
            "Use `conda ship build` from the Python package so the adapter can call the PEP 517 metadata hook, or pass --runtime-version explicitly."
                .to_string(),
        ),
    )
}

fn missing_runtime_version(project_dynamic_version: bool) -> miette::Report {
    if project_dynamic_version {
        ship_error(
            DiagnosticKind::MissingRuntimeVersion,
            "runtime version is required; [project] declares a dynamic version but project metadata resolution is not enabled",
            Some(
                "Pass --runtime-version, set [tool.conda-ship].runtime-version, or set runtime-version = { from = \"project-metadata\" }."
                    .to_string(),
            ),
        )
    } else {
        ship_error(
            DiagnosticKind::MissingRuntimeVersion,
            "runtime version is required",
            Some(
                "Pass --runtime-version, set [tool.conda-ship].runtime-version, or define [project].version in pyproject.toml."
                    .to_string(),
            ),
        )
    }
}

pub(crate) fn validate_update_config(
    config: &RuntimeStampConfig,
    layout: BundleLayout,
    install_name_source: Option<InstallNameSource>,
) -> miette::Result<()> {
    let Some(update) = config.update.as_ref() else {
        return Ok(());
    };
    if install_name_source.is_some() {
        return Err(miette::miette!(
            "install-name = {{ from = \"runtime-lock\" }} cannot be combined with [tool.conda-ship.update] because runtime updates require a stable install name"
        ));
    }
    if layout == BundleLayout::External {
        return Err(miette::miette!(
            "[tool.conda-ship.update] is supported only for online and embedded artifacts"
        ));
    }
    if update.channel.trim().is_empty() {
        return Err(miette::miette!("runtime update channel must not be empty"));
    }
    let channel = reqwest::Url::parse(&update.channel)
        .into_diagnostic()
        .context("runtime update channel must be an absolute URL")?;
    if !matches!(channel.scheme(), "https" | "file") {
        return Err(miette::miette!(
            "runtime update channel must use https:// or file://"
        ));
    }
    if !channel.username().is_empty() || channel.password().is_some() {
        return Err(miette::miette!(
            "runtime update channel must not contain credentials"
        ));
    }
    if channel.query().is_some() || channel.fragment().is_some() {
        return Err(miette::miette!(
            "runtime update channel must not contain a query or fragment"
        ));
    }
    if update
        .package
        .parse::<rattler_conda_types::PackageName>()
        .is_err()
    {
        return Err(miette::miette!(
            "invalid runtime update package name: {}",
            update.package
        ));
    }
    if update.has_compatibility_policy() {
        return Err(miette::miette!(
            "runtime update ownership and instruction are installed state, not build settings"
        ));
    }
    Ok(())
}

fn validate_installer_config(config: &RuntimeStampConfig) -> miette::Result<()> {
    if let Some(installer) = config.installer.as_deref() {
        validate_artifact_component("installer", installer)?;
    }
    Ok(())
}

pub(crate) fn validate_runtime_version(runtime_version: &str) -> miette::Result<()> {
    if runtime_version.is_empty() {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            "runtime version must not be empty",
            Some("Use a non-empty runtime version such as 1.0.0.".to_string()),
        ));
    }
    if !runtime_version
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            "runtime version must start with an ASCII letter or digit",
            Some("Use a package-style version such as 1.0.0 or 1.0.0+local.".to_string()),
        ));
    }
    if !runtime_version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '!'))
    {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            "runtime version may only contain ASCII letters, digits, dots, dashes, underscores, plus signs, and exclamation marks",
            Some("Remove whitespace, path separators, and shell metacharacters from the runtime version.".to_string()),
        ));
    }
    Ok(())
}

pub(crate) fn validate_docs_url(docs_url: &str) -> miette::Result<()> {
    if docs_url.is_empty() {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            "docs URL must not be empty",
            Some("Use a full documentation URL such as https://example.com/demo/.".to_string()),
        ));
    }
    if !(docs_url.starts_with("https://") || docs_url.starts_with("http://")) {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            "docs URL must start with https:// or http://",
            Some("Use a full documentation URL such as https://example.com/demo/.".to_string()),
        ));
    }
    if docs_url
        .chars()
        .any(|c| c.is_ascii_control() || c.is_whitespace())
    {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            "docs URL must not contain whitespace or control characters",
            Some(
                "Remove spaces, newlines, and terminal control characters from the docs URL."
                    .to_string(),
            ),
        ));
    }
    Ok(())
}

pub(crate) fn validate_installer(installer: &str) -> miette::Result<()> {
    validate_artifact_component("installer", installer)
}

pub(crate) fn validate_install_name(install_name: &str) -> miette::Result<()> {
    validate_artifact_component("install name", install_name)
}

fn validate_artifact_component(kind: &str, value: &str) -> miette::Result<()> {
    if value.is_empty() {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            format!("{kind} must not be empty"),
            Some(format!("Use a non-empty {kind} value.")),
        ));
    }
    if value == "." || value == ".." {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            format!("{kind} must not be . or .."),
            Some(format!("Use a filename-safe {kind} such as demo-runtime.")),
        ));
    }
    if !value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            format!("{kind} must start with an ASCII letter or digit"),
            Some(format!("Use a filename-safe {kind} such as demo-runtime.")),
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ship_error(
            DiagnosticKind::InvalidArtifactName,
            format!("{kind} may only contain ASCII letters, digits, dots, dashes, and underscores"),
            Some(format!(
                "Remove path separators, spaces, and shell metacharacters from the {kind}."
            )),
        ));
    }
    Ok(())
}

pub(crate) fn artifact_stem(name: &str, target_label: Option<&str>) -> String {
    if let Some(label) = target_label {
        format!("{name}-{label}")
    } else {
        name.to_string()
    }
}

pub(crate) fn binary_filename(stem: &str, target: Option<&str>) -> String {
    if target_is_windows(target) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

#[derive(Debug)]
pub(crate) enum RuntimeTemplateSource {
    Explicit(PathBuf),
    Environment(PathBuf),
    Installed(PathBuf),
}

impl RuntimeTemplateSource {
    fn path(&self) -> &Path {
        match self {
            Self::Explicit(path) | Self::Environment(path) | Self::Installed(path) => path,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Explicit(_) => "explicit --template",
            Self::Environment(_) => "CONDA_SHIP_TEMPLATE",
            Self::Installed(_) => "installed template",
        }
    }
}

pub(crate) fn source_binary_plan(
    template: Option<&Path>,
    target: Option<&str>,
) -> miette::Result<RuntimeTemplateSource> {
    if let Some(path) = template {
        return Ok(RuntimeTemplateSource::Explicit(resolve_runtime_template(
            path,
        )?));
    }

    if let Some(path) = runtime_template_from_env()? {
        return Ok(RuntimeTemplateSource::Environment(path));
    }

    if target.is_none()
        && let Some(path) = find_installed_runtime_template()
    {
        return Ok(RuntimeTemplateSource::Installed(path));
    }

    if target.is_some() {
        return Err(ship_error(
            DiagnosticKind::CrossBuildRequiresTemplate,
            "cross-builds require --template with a prebuilt runtime template for the requested target",
            Some(
                "Pass --template PATH for the target runtime template you want to stamp."
                    .to_string(),
            ),
        ));
    }

    Err(ship_error(
        DiagnosticKind::MissingRuntimeTemplate,
        "runtime template not found; install conda-ship with its runtime template or pass --template",
        Some(
            "Install a packaged conda-ship build that includes cs-template, set CONDA_SHIP_TEMPLATE, or pass --template PATH."
                .to_string(),
        ),
    ))
}

pub(crate) fn source_binary(
    template: Option<&Path>,
    target: Option<&str>,
) -> miette::Result<PathBuf> {
    let plan = source_binary_plan(template, target)?;
    Ok(plan.path().to_path_buf())
}

pub(crate) fn runtime_template_filename() -> &'static str {
    if cfg!(windows) {
        "cs-template.exe"
    } else {
        "cs-template"
    }
}

pub(crate) fn runtime_template_from_env() -> miette::Result<Option<PathBuf>> {
    match env::var_os(RUNTIME_TEMPLATE_ENV) {
        Some(value) if !value.is_empty() => Ok(Some(resolve_runtime_template(Path::new(&value))?)),
        _ => Ok(None),
    }
}

fn find_installed_runtime_template() -> Option<PathBuf> {
    if let Ok(exe) = env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join(runtime_template_filename());
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_runtime_template(path: &Path) -> miette::Result<PathBuf> {
    let template = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .into_diagnostic()
            .context("failed to read current directory")?
            .join(path)
    };
    if !template.is_file() {
        return Err(ship_error(
            DiagnosticKind::MissingRuntimeTemplate,
            format!("runtime template not found at {}", template.display()),
            Some(
                "Check the --template path or set CONDA_SHIP_TEMPLATE to a generic cs-template binary."
                    .to_string(),
            ),
        ));
    }
    if runtime_data::read_from_path(&template)
        .into_diagnostic()
        .with_context(|| format!("failed to inspect runtime template {}", template.display()))?
        .is_some_and(|data| data.stamped)
    {
        return Err(ship_error(
            DiagnosticKind::RuntimeTemplateStamped,
            format!("runtime template is already stamped: {}", template.display()),
            Some(
                "Use the generic cs-template binary as the template, not a runtime already produced by cs build."
                    .to_string(),
            ),
        ));
    }
    ensure_runtime_template_reader(&template)?;
    Ok(template)
}

fn ensure_runtime_template_reader(template: &Path) -> miette::Result<()> {
    runtime_data::validate_runtime_template_reader(template).map_err(|error| {
        ship_error(
            DiagnosticKind::RuntimeTemplateIncompatible,
            format!(
                "runtime template is incompatible with authenticated runtime data: {}: {error}",
                template.display()
            ),
            Some(
                "Use the cs-template binary from the same conda-ship release as the builder."
                    .to_string(),
            ),
        )
    })
}

fn stamp_runtime_data(
    binary: &Path,
    layout: BundleLayout,
    platform: Platform,
    runtime: &str,
    artifact_name: &str,
    derived: &DerivedRuntimeLock,
    generated_bundle: Option<&Path>,
) -> miette::Result<()> {
    ensure_runtime_template_reader(binary)?;
    let delegate_executable = derived
        .runtime_config
        .delegate_executable
        .clone()
        .ok_or_else(|| {
            miette::miette!("delegate executable has not been resolved before stamping")
        })?;
    let header = runtime_data::RuntimeDataHeader {
        schema_version: 1,
        artifact_name: artifact_name.to_string(),
        runtime_name: runtime.to_string(),
        runtime_version: derived
            .runtime_config
            .runtime_version
            .clone()
            .ok_or_else(|| {
                missing_runtime_version(derived.runtime_config.project_dynamic_version)
            })?,
        artifact_layout: layout.as_str().to_string(),
        platform: platform.to_string(),
        embedded_artifact_name: artifact_name.to_string(),
        delegate_executable,
        install_scheme: derived.runtime_config.install_scheme.unwrap_or_default(),
        install_name: derived
            .runtime_config
            .install_name
            .clone()
            .unwrap_or_else(|| runtime.to_string()),
        metadata_file: format!(".{runtime}.json"),
        bundle_env_var: runtime_data::runtime_env_var(runtime, "BUNDLE"),
        offline_env_var: runtime_data::runtime_env_var(runtime, "OFFLINE"),
        docs_url: derived
            .runtime_config
            .docs_url
            .clone()
            .unwrap_or_else(default_docs_url),
        installer: derived.runtime_config.installer.clone(),
        update: derived.runtime_config.update.clone(),
        runtime_config: runtime_data::RuntimeConfig {
            channels: derived.runtime_config.channels.clone(),
            packages: derived.runtime_config.packages.clone(),
            condarc: derived.runtime_config.condarc.clone(),
            freeze_base: derived.runtime_config.freeze_base,
        },
        runtime_lock: derived.content.clone(),
    };

    let embedded_bundle = if layout == BundleLayout::Embedded {
        Some(
            generated_bundle
                .ok_or_else(|| miette::miette!("embedded builds require a generated bundle"))?,
        )
    } else {
        None
    };
    runtime_data::append_to_binary(binary, &header, embedded_bundle)
        .into_diagnostic()
        .with_context(|| format!("failed to stamp runtime data onto {}", binary.display()))?;
    Ok(())
}

fn target_is_windows(target: Option<&str>) -> bool {
    target
        .map(|target| target.contains("windows"))
        .unwrap_or(cfg!(windows))
}

fn resolve_out_dir(root: &Path, out_dir: &Path) -> PathBuf {
    if out_dir.is_absolute() {
        out_dir.to_path_buf()
    } else {
        root.join(out_dir)
    }
}

fn platform_summaries(
    lock_file: &LockFile,
    runtime_lock_path: &Path,
) -> miette::Result<Vec<PlatformSummary>> {
    let env = lock_file.default_environment().ok_or_else(|| {
        miette::miette!("no default environment in {}", runtime_lock_path.display())
    })?;

    let mut summaries: Vec<_> = env
        .conda_packages_by_platform()
        .map(|(platform, packages)| PlatformSummary {
            platform: platform.name().to_string(),
            packages: packages.count(),
        })
        .collect();
    summaries.sort_by(|a, b| a.platform.cmp(&b.platform));
    Ok(summaries)
}

fn packages_for_platform<'a>(
    lock_file: &'a LockFile,
    runtime_lock_path: &Path,
    platform: Platform,
) -> miette::Result<Vec<&'a CondaPackageData>> {
    let env = lock_file.default_environment().ok_or_else(|| {
        miette::miette!("no default environment in {}", runtime_lock_path.display())
    })?;

    let packages: Vec<_> = env
        .conda_packages_by_platform()
        .filter(|(p, _)| p.subdir() == platform)
        .flat_map(|(_, pkgs)| pkgs)
        .collect();

    if packages.is_empty() {
        return Err(miette::miette!(
            "no packages for platform {platform} in {}",
            runtime_lock_path.display()
        ));
    }

    Ok(packages)
}

fn package_infos(packages: &[&CondaPackageData]) -> miette::Result<Vec<PackageInfo>> {
    let mut infos = Vec::with_capacity(packages.len());
    for pkg in packages {
        let record = package_record(pkg)?;
        infos.push(PackageInfo {
            name: record.name.as_normalized().to_string(),
            version: record.version.to_string(),
            build: record.build.to_string(),
            url: pkg
                .location()
                .as_url()
                .map(|url| url.to_string())
                .unwrap_or_default(),
            sha256: record
                .sha256
                .as_ref()
                .map(|hash| crate::hash::hex(hash.as_slice())),
        });
    }
    infos.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    Ok(infos)
}

pub(crate) fn render_package_list(packages: &[PackageInfo]) -> String {
    let mut content = String::from("name\tversion\tbuild\turl\tsha256\n");
    for package in packages {
        content.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            package.name,
            package.version,
            package.build,
            package.url,
            package.sha256.as_deref().unwrap_or("")
        ));
    }
    content
}

fn checksum_for_path(path: &Path) -> miette::Result<ArtifactChecksum> {
    let (digest, bytes) = crate::hash::sha256_file(path)
        .into_diagnostic()
        .with_context(|| format!("failed to read {} for checksum", path.display()))?;
    Ok(ArtifactChecksum {
        path: file_name(path)?,
        sha256: crate::hash::hex(&digest),
        bytes,
    })
}

fn write_checksums(path: &Path, checksums: &[ArtifactChecksum]) -> miette::Result<()> {
    let mut content = String::new();
    for checksum in checksums {
        content.push_str(&format!("{}  {}\n", checksum.sha256, checksum.path));
    }
    std::fs::write(path, content)
        .into_diagnostic()
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn file_name(path: &Path) -> miette::Result<String> {
    Ok(path
        .file_name()
        .ok_or_else(|| miette::miette!("path has no file name: {}", path.display()))?
        .to_string_lossy()
        .to_string())
}

#[cfg(test)]
mod publication_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn artifact_staging_directory_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let out_dir = tempfile::tempdir().unwrap();
        let staging = create_artifact_staging_dir(out_dir.path()).unwrap();
        let mode = staging.path().metadata().unwrap().permissions().mode() & 0o777;

        assert_eq!(mode, 0o700);
    }

    #[test]
    fn publication_lock_serializes_the_same_artifact_stem() {
        let out_dir = tempfile::tempdir().unwrap();
        let first = ArtifactPublicationLock::acquire(out_dir.path(), "demo").unwrap();
        let lock_path = out_dir.path().join(".demo.conda-ship.publish.lock");
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();

        assert!(matches!(
            FileExt::try_lock(&second),
            Err(fs4::TryLockError::WouldBlock)
        ));
        drop(first);
        FileExt::try_lock(&second).unwrap();
    }

    #[test]
    fn failed_publication_does_not_leave_old_completion_marker() {
        let out_dir = tempfile::tempdir().unwrap();
        let staging = create_artifact_staging_dir(out_dir.path()).unwrap();
        let staged_paths =
            planned_artifact_paths(staging.path(), "demo", BundleLayout::Online, None, None);
        let paths =
            planned_artifact_paths(out_dir.path(), "demo", BundleLayout::Online, None, None);
        let metadata = ArtifactMetadataPaths {
            info: staged_paths.info.clone(),
            checksums: staged_paths.checksums.clone(),
            lock: staged_paths.lock.clone(),
            package_list: staged_paths.package_list.clone(),
            sbom: staged_paths.sbom.clone(),
        };
        for (path, contents) in [
            (&staged_paths.binary, "new binary"),
            (&metadata.lock, "new lock"),
            (&metadata.package_list, "new packages"),
            (&metadata.sbom, "new sbom"),
            (&metadata.info, "new info"),
            (&metadata.checksums, "new checksums"),
            (&paths.binary, "old binary"),
            (&paths.lock, "old lock"),
            (&paths.package_list, "old packages"),
            (&paths.sbom, "old sbom"),
            (&paths.checksums, "old checksums"),
        ] {
            std::fs::write(path, contents).unwrap();
        }
        std::fs::create_dir(&paths.info).unwrap();

        let error = publish_staged_artifact_set(
            out_dir.path(),
            staging.path(),
            &staged_paths,
            None,
            &metadata,
            &paths,
        )
        .unwrap_err();

        assert!(error.to_string().contains("failed to publish"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&paths.binary).unwrap(),
            "new binary"
        );
        assert!(!paths.checksums.exists());
    }

    #[test]
    fn publication_does_not_retire_a_directory_as_completion_marker() {
        let out_dir = tempfile::tempdir().unwrap();
        let staging = create_artifact_staging_dir(out_dir.path()).unwrap();
        let checksums = out_dir.path().join("demo.sha256");
        std::fs::create_dir(&checksums).unwrap();
        let retained = checksums.join("retained.txt");
        std::fs::write(&retained, "retained").unwrap();

        let error = retire_completion_marker(&checksums, staging.path()).unwrap_err();

        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert_eq!(std::fs::read_to_string(retained).unwrap(), "retained");
    }

    #[test]
    fn publishing_nonexternal_layout_retires_previous_external_bundle() {
        let out_dir = tempfile::tempdir().unwrap();
        let staging = create_artifact_staging_dir(out_dir.path()).unwrap();
        let staged_paths =
            planned_artifact_paths(staging.path(), "demo", BundleLayout::Online, None, None);
        let paths =
            planned_artifact_paths(out_dir.path(), "demo", BundleLayout::Online, None, None);
        let metadata = ArtifactMetadataPaths {
            info: staged_paths.info.clone(),
            checksums: staged_paths.checksums.clone(),
            lock: staged_paths.lock.clone(),
            package_list: staged_paths.package_list.clone(),
            sbom: staged_paths.sbom.clone(),
        };
        for (path, contents) in [
            (&staged_paths.binary, "new binary"),
            (&metadata.lock, "new lock"),
            (&metadata.package_list, "new packages"),
            (&metadata.sbom, "new sbom"),
            (&metadata.info, "new info"),
            (&metadata.checksums, "new checksums"),
            (&paths.checksums, "old checksums"),
        ] {
            std::fs::write(path, contents).unwrap();
        }
        let obsolete_bundle = bundle_path(out_dir.path(), "demo");
        std::fs::write(&obsolete_bundle, "old bundle").unwrap();

        publish_staged_artifact_set(
            out_dir.path(),
            staging.path(),
            &staged_paths,
            None,
            &metadata,
            &paths,
        )
        .unwrap();

        assert!(!obsolete_bundle.exists());
        assert_eq!(
            std::fs::read_to_string(&paths.checksums).unwrap(),
            "new checksums"
        );
    }
}
