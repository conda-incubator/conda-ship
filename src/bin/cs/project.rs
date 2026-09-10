use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{PackageName, PackageRecord, Platform};
use rattler_lock::{CondaPackageData, LockFile, LockFileBuilder, PlatformData};

use super::diagnostic::{DiagnosticKind, ship_error};
use super::{
    ProjectManifest, RuntimeStampConfig, RuntimeVersionConfig, RuntimeVersionSource, ShipConfig,
};

pub(crate) fn project_root(override_root: Option<&Path>) -> miette::Result<PathBuf> {
    if let Some(root) = override_root {
        return Ok(root.to_path_buf());
    }

    let current_dir = std::env::current_dir()
        .into_diagnostic()
        .context("failed to read current directory")?;
    find_project_root(&current_dir).ok_or_else(|| {
        ship_error(
            DiagnosticKind::MissingProjectRoot,
            "could not find project root containing conda.toml, pixi.toml, or supported pyproject.toml",
            Some("Run from a project directory or pass --root PATH.".to_string()),
        )
    })
}

pub(crate) fn find_project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|p| has_supported_manifest(p))
        .map(Path::to_path_buf)
}

pub(crate) struct DerivedRuntimeLock {
    pub(crate) input: ProjectInput,
    pub(crate) lock_file: LockFile,
    pub(crate) content: String,
    pub(crate) source_environment: String,
    pub(crate) runtime_config: RuntimeStampConfig,
    pub(crate) platforms: Vec<Platform>,
    pub(crate) total_packages: usize,
    pub(crate) total_excluded: usize,
    pub(crate) removed_excludes: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct ProjectInput {
    pub(crate) manifest_path: PathBuf,
    pub(crate) manifest_kind: ManifestKind,
    pub(crate) lock_path: PathBuf,
    pub(crate) config: ShipConfig,
    pub(crate) runtime_version: Option<String>,
    pub(crate) runtime_version_source: Option<RuntimeVersionSource>,
    pub(crate) project_dynamic_version: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManifestKind {
    CondaToml,
    PixiToml,
    CondaPyproject,
    PixiPyproject,
}

impl ManifestKind {
    pub(crate) fn manifest_label(self) -> &'static str {
        match self {
            Self::CondaToml => "conda.toml",
            Self::PixiToml => "pixi.toml",
            Self::CondaPyproject => "pyproject.toml [tool.conda]",
            Self::PixiPyproject => "pyproject.toml [tool.pixi]",
        }
    }

    fn lockfile_name(self) -> &'static str {
        match self {
            Self::CondaToml | Self::CondaPyproject => "conda.lock",
            Self::PixiToml | Self::PixiPyproject => "pixi.lock",
        }
    }

    fn lock_command(self) -> &'static str {
        match self {
            Self::CondaToml | Self::CondaPyproject => "conda workspace lock",
            Self::PixiToml | Self::PixiPyproject => "pixi lock",
        }
    }
}

pub(crate) fn derive_runtime_lock(root: &Path) -> miette::Result<DerivedRuntimeLock> {
    let input = discover_project_input(root)?;
    let condarc = read_condarc_file(&input.manifest_path, input.config.condarc_file.as_deref())?;
    let lock_content = std::fs::read_to_string(&input.lock_path)
        .into_diagnostic()
        .with_context(|| format!("failed to read {}", input.lock_path.display()))?;

    let lock_file = parse_lock(&lock_content, &input.lock_path, input.manifest_kind)?;

    let source_environment = input.config.source_environment.as_deref().ok_or_else(|| {
        ship_error(
            DiagnosticKind::MissingSourceEnvironment,
            "source environment is required; set [tool.conda-ship].source-environment to the solved environment to ship",
            Some("Add source-environment to [tool.conda-ship] and point it at the solved conda-workspaces or pixi environment to package.".to_string()),
        )
    })?;
    let runtime_env = lock_file.environment(source_environment).ok_or_else(|| {
        ship_error(
            DiagnosticKind::SourceEnvironmentNotFound,
            format!(
                "source environment {source_environment:?} not found in {}",
                input.lock_path.display()
            ),
            Some("Add the environment to the source manifest, or update source-environment to an environment that exists in the lockfile.".to_string()),
        )
    })?;
    if runtime_env
        .pypi_packages_by_platform()
        .any(|(_, mut packages)| packages.next().is_some())
    {
        return Err(ship_error(
            DiagnosticKind::UnsupportedPypiPackages,
            format!("source environment {source_environment:?} contains unsupported PyPI packages"),
            Some(format!(
                "Use conda packages or select a source environment containing only conda packages, then run `{}` to regenerate {}.",
                input.manifest_kind.lock_command(),
                input.manifest_kind.lockfile_name()
            )),
        ));
    }

    let platform_data: Vec<_> = runtime_env
        .platforms()
        .map(|platform| PlatformData {
            name: platform.name().clone(),
            subdir: platform.subdir(),
            virtual_packages: platform.virtual_packages().to_vec(),
        })
        .collect();
    let platforms: Vec<Platform> = platform_data
        .iter()
        .map(|platform| platform.subdir)
        .collect();
    let mut builder = LockFileBuilder::new()
        .with_platforms(platform_data)
        .into_diagnostic()
        .context("failed to initialize runtime lock platforms")?;
    if !runtime_env.channels().is_empty() {
        builder.set_channels("default", runtime_env.channels().iter().cloned());
    }
    let runtime_channels = runtime_env
        .channels()
        .iter()
        .map(|channel| channel.url.clone())
        .collect();

    let mut total_packages = 0usize;
    let mut total_excluded = 0usize;
    let mut removed_excludes = HashSet::new();
    let mut resolved_package_names = HashSet::new();

    for (platform, packages) in runtime_env.conda_packages_by_platform() {
        let pkgs: Vec<_> = packages.cloned().collect();

        let filtered = if input.config.exclude_packages.is_empty() {
            pkgs
        } else {
            let (kept, removed) = filter_excluded(&pkgs, &input.config.exclude_packages)?;
            removed_excludes.extend(removed.iter().cloned());
            total_excluded += removed.len();
            kept
        };
        total_packages += filtered.len();
        for pkg in filtered {
            resolved_package_names.insert(package_record(&pkg)?.name.as_normalized().to_string());
            builder
                .add_conda_package("default", platform.name().as_str(), pkg)
                .into_diagnostic()
                .context("failed to add package to runtime lock")?;
        }
    }
    let mut runtime_packages: Vec<_> = resolved_package_names.into_iter().collect();
    runtime_packages.sort();
    let mut removed_excludes: Vec<_> = removed_excludes.into_iter().collect();
    removed_excludes.sort();

    let new_lock = builder.finish();
    let new_content = new_lock
        .render_to_string()
        .into_diagnostic()
        .context("failed to render runtime lock")?;

    Ok(DerivedRuntimeLock {
        input: input.clone(),
        lock_file: new_lock,
        content: new_content,
        source_environment: source_environment.to_string(),
        runtime_config: RuntimeStampConfig {
            channels: runtime_channels,
            packages: runtime_packages,
            exclude_packages: input.config.exclude_packages,
            delegate_executable: input.config.delegate_executable,
            runtime_version: input.runtime_version,
            runtime_version_source: input.runtime_version_source,
            project_dynamic_version: input.project_dynamic_version,
            docs_url: input.config.docs_url,
            install_scheme: input.config.install_scheme,
            install_name: None,
            installer: input.config.installer,
            condarc,
            freeze_base: input.config.freeze_base,
            update: input.config.update,
        },
        platforms,
        total_packages,
        total_excluded,
        removed_excludes,
    })
}

pub(crate) fn read_condarc_file(
    manifest_path: &Path,
    configured_path: Option<&Path>,
) -> miette::Result<Option<String>> {
    let Some(configured_path) = configured_path else {
        return Ok(None);
    };
    let path = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(configured_path);
    let contents = std::fs::read_to_string(&path)
        .into_diagnostic()
        .with_context(|| format!("failed to read condarc-file {}", path.display()))?;
    let document: serde_yaml::Value = serde_yaml::from_str(&contents)
        .into_diagnostic()
        .with_context(|| format!("failed to parse condarc-file {} as YAML", path.display()))?;
    if !document.is_mapping() {
        return Err(miette::miette!(
            "condarc-file must contain a YAML mapping: {}",
            path.display()
        ));
    }
    Ok(Some(contents))
}

pub(crate) fn discover_project_input(root: &Path) -> miette::Result<ProjectInput> {
    let manifest_path = discover_manifest_path(root)?;
    let kind = manifest_kind(&manifest_path)?;

    let lock_path = root.join(kind.lockfile_name());
    if !lock_path.exists() {
        return Err(ship_error(
            DiagnosticKind::MissingLockfile,
            format!(
                "lockfile not found at {}; run `{}` first",
                lock_path.display(),
                kind.lock_command()
            ),
            Some(format!(
                "Run `{}` and commit the resulting {} before building.",
                kind.lock_command(),
                kind.lockfile_name()
            )),
        ));
    }

    let manifest = std::fs::read_to_string(&manifest_path)
        .into_diagnostic()
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest: ProjectManifest = toml::from_str(&manifest)
        .into_diagnostic()
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    let mut config = manifest.tool.conda_ship;
    let project_dynamic_version = manifest
        .project
        .dynamic
        .iter()
        .any(|field| field == "version");
    let (runtime_version, runtime_version_source) =
        resolve_manifest_runtime_version(config.runtime_version.take(), manifest.project.version);

    Ok(ProjectInput {
        manifest_path,
        manifest_kind: kind,
        lock_path,
        config,
        runtime_version,
        runtime_version_source,
        project_dynamic_version,
    })
}

pub(crate) fn discover_manifest_path(root: &Path) -> miette::Result<PathBuf> {
    if root.join("conda.toml").exists() {
        Ok(root.join("conda.toml"))
    } else if root.join("pixi.toml").exists() {
        Ok(root.join("pixi.toml"))
    } else if is_supported_pyproject_manifest(&root.join("pyproject.toml")) {
        Ok(root.join("pyproject.toml"))
    } else {
        Err(ship_error(
            DiagnosticKind::MissingManifest,
            format!(
                "could not find conda.toml, pixi.toml, or supported pyproject.toml in {}",
                root.display()
            ),
            Some(
                "Create a conda-workspaces or pixi manifest in the selected project root."
                    .to_string(),
            ),
        ))
    }
}

fn has_supported_manifest(root: &Path) -> bool {
    root.join("conda.toml").exists()
        || root.join("pixi.toml").exists()
        || is_supported_pyproject_manifest(&root.join("pyproject.toml"))
}

pub(crate) fn manifest_kind(manifest_path: &Path) -> miette::Result<ManifestKind> {
    match manifest_path.file_name().and_then(|n| n.to_str()) {
        Some("conda.toml") => Ok(ManifestKind::CondaToml),
        Some("pixi.toml") => Ok(ManifestKind::PixiToml),
        Some("pyproject.toml") => pyproject_manifest_kind(manifest_path).ok_or_else(|| {
            miette::miette!("unsupported pyproject.toml: {}", manifest_path.display())
        }),
        _ => Err(miette::miette!(
            "unsupported manifest path: {}",
            manifest_path.display()
        )),
    }
}

pub(crate) fn is_supported_pyproject_manifest(path: &Path) -> bool {
    pyproject_manifest_kind(path).is_some()
}

fn pyproject_manifest_kind(path: &Path) -> Option<ManifestKind> {
    if !path.exists() {
        return None;
    }

    let Ok(content) = std::fs::read_to_string(path) else {
        return None;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&content) else {
        return None;
    };

    if has_nonempty_toml_table(&value, &["tool", "conda", "workspace"]) {
        Some(ManifestKind::CondaPyproject)
    } else if has_nonempty_toml_table(&value, &["tool", "pixi", "workspace"]) {
        Some(ManifestKind::PixiPyproject)
    } else {
        None
    }
}

fn has_nonempty_toml_table(value: &toml::Value, path: &[&str]) -> bool {
    let Some((head, tail)) = path.split_first() else {
        return value.as_table().is_some_and(|table| !table.is_empty());
    };
    value
        .get(*head)
        .is_some_and(|nested| has_nonempty_toml_table(nested, tail))
}

fn resolve_manifest_runtime_version(
    configured: Option<RuntimeVersionConfig>,
    project_version: Option<String>,
) -> (Option<String>, Option<RuntimeVersionSource>) {
    match configured {
        Some(RuntimeVersionConfig::Value(version)) => (Some(version), None),
        Some(RuntimeVersionConfig::Source(source)) => (None, Some(source.from)),
        None => (project_version, None),
    }
}

pub(crate) fn write_generated_runtime_lock(path: &Path, content: &str) -> miette::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .into_diagnostic()
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, content)
        .into_diagnostic()
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn parse_lock(
    lock_content: &str,
    lock_path: &Path,
    manifest_kind: ManifestKind,
) -> miette::Result<LockFile> {
    let rewritten;
    let parse_content = if matches!(
        manifest_kind,
        ManifestKind::CondaToml | ManifestKind::CondaPyproject
    ) && let Some(content) =
        conda_workspaces_lock_v1_as_rattler_v6(lock_content)?
    {
        rewritten = content;
        &rewritten
    } else {
        lock_content
    };

    LockFile::from_str_with_base_directory(parse_content, lock_path.parent())
        .into_diagnostic()
        .with_context(|| format!("failed to parse {}", lock_path.display()))
}

fn conda_workspaces_lock_v1_as_rattler_v6(lock_content: &str) -> miette::Result<Option<String>> {
    let mut document: serde_yaml::Value = serde_yaml::from_str(lock_content)
        .into_diagnostic()
        .context("failed to parse lockfile YAML")?;
    let Some(mapping) = document.as_mapping_mut() else {
        return Ok(None);
    };

    let version = serde_yaml::Value::String("version".to_string());
    let environments = serde_yaml::Value::String("environments".to_string());
    let packages = serde_yaml::Value::String("packages".to_string());
    if mapping.get(&version).and_then(serde_yaml::Value::as_u64) != Some(1)
        || !mapping.contains_key(&environments)
        || !mapping.contains_key(&packages)
    {
        return Ok(None);
    }

    mapping.insert(version, serde_yaml::Value::Number(6.into()));
    serde_yaml::to_string(&document)
        .into_diagnostic()
        .context("failed to render normalized lockfile YAML")
        .map(Some)
}

pub(crate) fn package_record(package: &CondaPackageData) -> miette::Result<&PackageRecord> {
    package
        .record()
        .ok_or_else(|| miette::miette!("conda package in lockfile is missing its package record"))
}

/// Remove explicitly excluded packages and any transitive dependencies that
/// are not required by any remaining package.
pub(crate) fn filter_excluded(
    packages: &[CondaPackageData],
    excludes: &[String],
) -> miette::Result<(Vec<CondaPackageData>, Vec<String>)> {
    let exclude_set: HashSet<&str> = excludes.iter().map(|s| s.as_str()).collect();

    let mut pkg_names = Vec::with_capacity(packages.len());
    for package in packages {
        pkg_names.push(package_record(package)?.name.as_normalized().to_string());
    }
    let name_to_idx: HashMap<&str, usize> = pkg_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();

    let n = packages.len();
    let mut reverse_deps: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    for (i, pkg) in packages.iter().enumerate() {
        for dep_str in &package_record(pkg)?.depends {
            let dep_name = PackageName::from_matchspec_str_unchecked(dep_str);
            if let Some(&dep_idx) = name_to_idx.get(dep_name.as_normalized()) {
                reverse_deps[dep_idx].insert(i);
            }
        }
    }

    let mut removed: HashSet<usize> = HashSet::new();
    let mut queue: Vec<usize> = Vec::new();
    for (i, name) in pkg_names.iter().enumerate() {
        if exclude_set.contains(name.as_str()) {
            removed.insert(i);
            queue.push(i);
        }
    }

    while let Some(pkg_idx) = queue.pop() {
        for dep_str in &package_record(&packages[pkg_idx])?.depends {
            let dep_name = PackageName::from_matchspec_str_unchecked(dep_str);
            if let Some(&dep_idx) = name_to_idx.get(dep_name.as_normalized()) {
                if removed.contains(&dep_idx) {
                    continue;
                }
                let all_dependents_removed = reverse_deps[dep_idx]
                    .iter()
                    .all(|rdep| removed.contains(rdep));
                if all_dependents_removed {
                    removed.insert(dep_idx);
                    queue.push(dep_idx);
                }
            }
        }
    }

    let mut removed_names: Vec<String> = removed.iter().map(|&i| pkg_names[i].clone()).collect();
    removed_names.sort();

    let filtered: Vec<CondaPackageData> = packages
        .iter()
        .enumerate()
        .filter(|(i, _)| !removed.contains(i))
        .map(|(_, p)| p.clone())
        .collect();

    Ok((filtered, removed_names))
}
