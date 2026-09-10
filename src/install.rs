//! Package installation from stamped lockfiles and bundles.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use miette::{Context, IntoDiagnostic};
use rattler::{
    default_cache_dir,
    install::{
        IndicatifReporter, Installer,
        link_script::{LinkScriptError, PrePostLinkResult},
    },
    package_cache::PackageCache,
};
use rattler_conda_types::{
    MatchSpec, PackageName, ParseMatchSpecOptions, Platform, PrefixRecord, RepoDataRecord,
};
use rattler_lock::LockFile;

use crate::{config, exec, policy};

static GLOBAL_MP: std::sync::LazyLock<MultiProgress> = std::sync::LazyLock::new(|| {
    let mp = MultiProgress::new();
    mp.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
    mp
});

pub(crate) fn multi_progress() -> MultiProgress {
    GLOBAL_MP.clone()
}

/// Parse a lockfile and return the current platform and records.
pub(crate) fn lockfile_records_for_current_platform(
    lock_content: &str,
) -> miette::Result<(Platform, Vec<RepoDataRecord>)> {
    let lock_file = LockFile::from_str_with_base_directory(lock_content, None)
        .into_diagnostic()
        .context("failed to parse lockfile")?;

    let env = lock_file
        .default_environment()
        .ok_or_else(|| miette::miette!("lockfile has no default environment"))?;

    let platform = Platform::current();
    let lock_platform = env
        .platforms()
        .find(|locked_platform| locked_platform.subdir() == platform)
        .ok_or_else(|| miette::miette!("lockfile has no records for platform {}", platform))?;
    let records = env
        .conda_repodata_records(lock_platform)
        .into_diagnostic()
        .context("failed to extract records from lockfile")?
        .ok_or_else(|| miette::miette!("lockfile has no records for platform {}", platform))?;

    Ok((platform, records))
}

/// Parse a lockfile and return the platform and records.
fn lockfile_records(lock_content: &str) -> miette::Result<(Platform, Vec<RepoDataRecord>)> {
    let (platform, records) = lockfile_records_for_current_platform(lock_content)?;

    eprintln!(
        "   Lockfile contains {} packages for {}",
        records.len(),
        platform
    );

    Ok((platform, records))
}

/// Install packages from a pre-solved lockfile (fast path, no solve needed).
pub async fn from_lockfile(
    prefix: &Path,
    lock_content: &str,
    reinstall: bool,
) -> miette::Result<()> {
    from_lockfile_with_specs(
        prefix,
        lock_content,
        &config::embedded_config().packages,
        reinstall,
    )
    .await
}

/// Install packages from a pre-solved lockfile with explicit requested specs.
pub(crate) async fn from_lockfile_with_specs(
    prefix: &Path,
    lock_content: &str,
    requested_specs: &[String],
    reinstall: bool,
) -> miette::Result<()> {
    let (platform, required_packages) = lockfile_records(lock_content)?;

    let match_specs = parse_specs(requested_specs)?;
    let installed = PrefixRecord::collect_from_prefix::<PrefixRecord>(prefix).into_diagnostic()?;
    let client = crate::http::download_client()?;

    run_installer(
        prefix,
        platform,
        &installed,
        &match_specs,
        client,
        required_packages,
        reinstall,
    )
    .await
}

/// Install packages from a lockfile using a local bundle directory.
///
/// Pre-populates the rattler package cache from the bundle directory, then
/// runs the normal install path. When `offline` is true, no download client
/// is configured — all packages must be present in the bundle or cache.
pub async fn from_lockfile_with_bundle(
    prefix: &Path,
    lock_content: &str,
    bundle_dir: &Path,
    offline: bool,
    reinstall: bool,
) -> miette::Result<()> {
    from_lockfile_with_bundle_and_specs(
        prefix,
        lock_content,
        &config::embedded_config().packages,
        bundle_dir,
        offline,
        reinstall,
    )
    .await
}

/// Install packages from a lockfile and local bundle using explicit requested specs.
pub(crate) async fn from_lockfile_with_bundle_and_specs(
    prefix: &Path,
    lock_content: &str,
    requested_specs: &[String],
    bundle_dir: &Path,
    offline: bool,
    reinstall: bool,
) -> miette::Result<()> {
    let (platform, required_packages) = lockfile_records(lock_content)?;

    let bundle_index = index_bundle_dir(bundle_dir)?;
    let (matched, missing) = match_records_to_bundle(&required_packages, &bundle_index)?;

    if offline && !missing.is_empty() {
        return Err(miette::miette!(
            "offline mode: {} package(s) not found in bundle: {}",
            missing.len(),
            missing.join(", ")
        ));
    }

    eprintln!(
        "   Bundle: {}/{} packages found locally",
        matched.len(),
        required_packages.len()
    );

    let cache_dir = default_cache_dir()
        .map_err(|e| miette::miette!("could not determine cache directory: {}", e))?;
    rattler_cache::ensure_cache_dir(&cache_dir)
        .map_err(|e| miette::miette!("could not create cache directory: {}", e))?;

    let package_cache = PackageCache::new(cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR));

    let start = Instant::now();
    let cache_futures = matched.iter().map(|path| {
        let cache = &package_cache;
        async move {
            cache
                .get_or_fetch_from_path(path, None, None)
                .await
                .into_diagnostic()
                .context(format!(
                    "failed to cache package from bundle: {}",
                    policy::path_for_display(path)
                ))
        }
    });
    futures::future::try_join_all(cache_futures).await?;
    eprintln!(
        "   Cached {} packages from bundle in {:.1}s",
        matched.len(),
        start.elapsed().as_secs_f64()
    );

    let match_specs = parse_specs(requested_specs)?;
    let installed = PrefixRecord::collect_from_prefix::<PrefixRecord>(prefix).into_diagnostic()?;
    let reinstall_packages = reinstall_package_names(&required_packages, reinstall);

    let mut installer = Installer::new()
        .with_package_cache(package_cache)
        .with_target_platform(platform)
        .with_installed_packages(installed.to_vec())
        .with_reinstall_packages(reinstall_packages)
        .with_execute_link_scripts(true)
        .with_requested_specs(match_specs)
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(multi_progress())
                .finish(),
        );

    if !offline {
        installer = installer.with_download_client(crate::http::download_client()?);
    }

    let start = Instant::now();
    let result = installer
        .install(prefix, required_packages)
        .await
        .into_diagnostic()
        .context("failed to install packages")?;

    validate_post_link_script_result(result.post_link_script_result.as_ref())?;

    if result.transaction.operations.is_empty() {
        eprintln!("   {} Already up to date", console::style("✔").green());
    } else {
        eprintln!(
            "   Installed {} packages in {:.1}s",
            result.transaction.operations.len(),
            start.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

/// Install packages from a lockfile in offline mode (cache only, no bundle).
pub async fn from_lockfile_offline(
    prefix: &Path,
    lock_content: &str,
    reinstall: bool,
) -> miette::Result<()> {
    from_lockfile_offline_with_specs(
        prefix,
        lock_content,
        &config::embedded_config().packages,
        reinstall,
    )
    .await
}

/// Install packages from a lockfile in offline mode with explicit requested specs.
pub(crate) async fn from_lockfile_offline_with_specs(
    prefix: &Path,
    lock_content: &str,
    requested_specs: &[String],
    reinstall: bool,
) -> miette::Result<()> {
    let (platform, required_packages) = lockfile_records(lock_content)?;

    let cache_dir = default_cache_dir()
        .map_err(|e| miette::miette!("could not determine cache directory: {}", e))?;
    let package_cache = PackageCache::new(cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR));

    let match_specs = parse_specs(requested_specs)?;
    let installed = PrefixRecord::collect_from_prefix::<PrefixRecord>(prefix).into_diagnostic()?;
    let reinstall_packages = reinstall_package_names(&required_packages, reinstall);

    let start = Instant::now();
    let result = Installer::new()
        .with_package_cache(package_cache)
        .with_target_platform(platform)
        .with_installed_packages(installed.to_vec())
        .with_reinstall_packages(reinstall_packages)
        .with_execute_link_scripts(true)
        .with_requested_specs(match_specs)
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(multi_progress())
                .finish(),
        )
        .install(prefix, required_packages)
        .await
        .into_diagnostic()
        .context("failed to install packages (offline mode — are all packages cached?)")?;

    validate_post_link_script_result(result.post_link_script_result.as_ref())?;

    if result.transaction.operations.is_empty() {
        eprintln!("   {} Already up to date", console::style("✔").green());
    } else {
        eprintln!(
            "   Installed {} packages in {:.1}s",
            result.transaction.operations.len(),
            start.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

/// Extract the embedded bundle (if any) to a temporary directory.
///
/// Returns `Some(path)` when the current artifact contains an embedded
/// `bundle.tar.zst`. Returns `None` for standard builds without an embedded
/// bundle.
pub fn extract_embedded_bundle() -> miette::Result<Option<PathBuf>> {
    let Some(bundle) = config::embedded_bundle() else {
        return Ok(None);
    };

    let tmp_dir = tempfile::Builder::new()
        .prefix(&format!(
            "{}-bundle-",
            crate::policy::embedded_artifact_name()
        ))
        .tempdir()
        .into_diagnostic()
        .context("failed to create temp dir for embedded bundle")?;

    let verified_bundle = bundle
        .open_verified()
        .into_diagnostic()
        .context("failed to verify embedded bundle")?;
    let decoder = zstd::Decoder::new(verified_bundle)
        .into_diagnostic()
        .context("failed to decompress embedded bundle")?;
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    archive.set_preserve_ownerships(false);
    for entry in archive
        .entries()
        .into_diagnostic()
        .context("failed to read embedded bundle entries")?
    {
        let mut entry = entry
            .into_diagnostic()
            .context("failed to read bundle entry")?;
        let path = entry
            .path()
            .into_diagnostic()
            .context("failed to read bundle entry path")?
            .into_owned();
        let archive_name = bundle_archive_name_from_path(&path)?;
        let path_str = path.display().to_string();
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(miette::miette!(
                "embedded bundle contains a link entry: {}",
                path_str
            ));
        }
        if !entry_type.is_file() {
            return Err(miette::miette!(
                "unsupported entry type in embedded bundle: {}",
                path_str
            ));
        }
        let dest = tmp_dir.path().join(&archive_name);
        entry
            .unpack(&dest)
            .into_diagnostic()
            .with_context(|| format!("failed to unpack bundle entry {path_str}"))?;
    }

    eprintln!(
        "   Extracted embedded bundle ({:.1} MB) to {}",
        bundle.len() as f64 / 1_048_576.0,
        policy::path_for_display(tmp_dir.path())
    );

    let path = tmp_dir.keep();
    Ok(Some(path))
}

/// Scan a directory for `.conda` and `.tar.bz2` package archives.
///
/// Returns a map from filename to full path.
pub(crate) fn index_bundle_dir(dir: &Path) -> miette::Result<HashMap<String, PathBuf>> {
    let mut index = HashMap::new();
    let entries = std::fs::read_dir(dir).into_diagnostic().context(format!(
        "failed to read bundle directory: {}",
        policy::path_for_display(dir)
    ))?;

    for entry in entries {
        let entry = entry.into_diagnostic()?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .into_diagnostic()
            .with_context(|| {
                format!(
                    "failed to inspect bundle entry: {}",
                    policy::path_for_display(&path)
                )
            })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if validate_bundle_archive_name(&name).is_ok() {
            index.insert(name, path);
        }
    }
    Ok(index)
}

fn bundle_archive_name_from_path(path: &Path) -> miette::Result<String> {
    let mut components = path.components();
    let name = match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(name)), None) => name,
        _ => {
            return Err(miette::miette!(
                "embedded bundle entries must be flat package archive names: {}",
                path.display()
            ));
        }
    };
    let name = name.to_str().ok_or_else(|| {
        miette::miette!(
            "embedded bundle contains a non-UTF-8 package archive name: {}",
            path.display()
        )
    })?;
    validate_bundle_archive_name(name)?;
    Ok(name.to_string())
}

fn validate_bundle_archive_name(name: &str) -> miette::Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        return Err(miette::miette!(
            "invalid bundle package archive name: {name:?}"
        ));
    }
    if !(name.ends_with(".conda") || name.ends_with(".tar.bz2")) {
        return Err(miette::miette!(
            "bundle package archive name must end with .conda or .tar.bz2: {name}"
        ));
    }
    Ok(())
}

/// Match lockfile records to files in a bundle index.
///
/// Returns `(matched_paths, missing_names)` where `missing_names` lists
/// packages not found in the bundle.
pub(crate) fn match_records_to_bundle(
    records: &[RepoDataRecord],
    bundle_index: &HashMap<String, PathBuf>,
) -> miette::Result<(Vec<PathBuf>, Vec<String>)> {
    let mut matched = Vec::new();
    let mut missing = Vec::new();

    for record in records {
        let filename = record
            .url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or_default()
            .to_string();

        if let Some(path) = bundle_index.get(&filename) {
            verify_bundle_package(record, path, &filename)?;
            matched.push(path.clone());
        } else {
            missing.push(filename);
        }
    }
    Ok((matched, missing))
}

fn verify_bundle_package(
    record: &RepoDataRecord,
    path: &Path,
    filename: &str,
) -> miette::Result<()> {
    let expected = record.package_record.sha256.as_ref().ok_or_else(|| {
        miette::miette!("{filename} has no SHA256 in the lockfile; refusing bundle install")
    })?;
    let (actual, _) = crate::hash::sha256_file(path)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to read bundle package: {}",
                policy::path_for_display(path)
            )
        })?;
    if actual.as_slice() != expected.as_slice() {
        return Err(miette::miette!(
            "SHA256 mismatch for bundled package {filename}: expected {}, got {}",
            crate::hash::hex(expected.as_slice()),
            crate::hash::hex(actual.as_slice())
        ));
    }
    Ok(())
}

fn validate_post_link_script_result(
    post_link_result: Option<&Result<PrePostLinkResult, LinkScriptError>>,
) -> miette::Result<()> {
    match post_link_result {
        Some(Ok(result)) if !result.failed_packages.is_empty() => Err(miette::miette!(
            "{}",
            post_link_failure_lines(result).join("\n")
        )),
        Some(Err(error)) => Err(miette::miette!(
            "failed to inspect post-link script results: {error}"
        )),
        Some(Ok(_)) | None => Ok(()),
    }
}

fn post_link_failure_lines(result: &PrePostLinkResult) -> Vec<String> {
    if result.failed_packages.is_empty() {
        return Vec::new();
    }

    let packages: Vec<_> = result
        .failed_packages
        .iter()
        .map(|package| package.as_normalized().to_string())
        .collect();
    let mut lines = vec![format!(
        "post-link scripts failed for {} package(s): {}",
        result.failed_packages.len(),
        packages.join(", ")
    )];

    for package in &result.failed_packages {
        let Some(message) = result.messages.get(package) else {
            continue;
        };
        let message = message.trim();
        if message.is_empty() {
            continue;
        }
        for line in message
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            lines.push(format!("{}: {line}", package.as_normalized()));
        }
    }

    lines
}

fn reinstall_package_names(packages: &[RepoDataRecord], reinstall: bool) -> HashSet<PackageName> {
    if !reinstall {
        return HashSet::new();
    }
    packages
        .iter()
        .map(|record| record.package_record.name.clone())
        .collect()
}

pub(crate) fn parse_specs(specs: &[String]) -> miette::Result<Vec<MatchSpec>> {
    specs
        .iter()
        .map(|s| MatchSpec::from_str(s, ParseMatchSpecOptions::default()))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()
        .context("failed to parse package specs")
}

async fn run_installer(
    prefix: &Path,
    platform: Platform,
    installed: &[PrefixRecord],
    specs: &[MatchSpec],
    client: reqwest_middleware::ClientWithMiddleware,
    packages: Vec<RepoDataRecord>,
    reinstall: bool,
) -> miette::Result<()> {
    let start = Instant::now();
    let reinstall_packages = reinstall_package_names(&packages, reinstall);
    let result = Installer::new()
        .with_download_client(client)
        .with_target_platform(platform)
        .with_installed_packages(installed.to_vec())
        .with_reinstall_packages(reinstall_packages)
        .with_execute_link_scripts(true)
        .with_requested_specs(specs.to_vec())
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(multi_progress())
                .finish(),
        )
        .install(prefix, packages)
        .await
        .into_diagnostic()
        .context("failed to install packages")?;

    validate_post_link_script_result(result.post_link_script_result.as_ref())?;

    if result.transaction.operations.is_empty() {
        eprintln!("   {} Already up to date", console::style("✔").green());
    } else {
        eprintln!(
            "   Installed {} packages in {:.1}s",
            result.transaction.operations.len(),
            start.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

pub(crate) fn wrap_spinner<T, F: FnOnce() -> T>(msg: impl Into<Cow<'static, str>>, func: F) -> T {
    let pb = multi_progress().add(ProgressBar::new_spinner());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(ProgressStyle::with_template("   {spinner:.green} {msg}").unwrap());
    pb.set_message(msg);
    let result = func();
    pb.finish_and_clear();
    result
}

pub(crate) fn compile_python_bytecode(prefix: &Path) {
    let python = exec::executable_in_prefix(prefix, "python");
    if !python.exists() {
        return;
    }

    let lib_dir = prefix.join("lib");
    let result = wrap_spinner("compiling Python bytecode", move || {
        std::process::Command::new(&python)
            .args(["-m", "compileall", "-q", "-j", "0"])
            .arg(&lib_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
    });

    match result {
        Ok(status) if status.success() => {}
        _ => {
            eprintln!(
                "   {} bytecode compilation finished with errors (non-fatal)",
                console::style("!").yellow(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::PackageName;
    use rstest::rstest;

    #[test]
    fn test_parse_specs_valid() {
        let specs = vec![
            "python >=3.12".to_string(),
            "conda >=25.1".to_string(),
            "numpy".to_string(),
        ];
        let result = parse_specs(&specs);
        assert!(result.is_ok(), "valid specs should parse successfully");
        assert_eq!(result.unwrap().len(), 3);
    }

    #[test]
    fn test_parse_specs_empty() {
        let result = parse_specs(&[]);
        assert!(result.is_ok(), "empty specs should parse successfully");
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_parse_specs_invalid() {
        let specs = vec![">=>=not_a_package!!!".to_string()];
        let result = parse_specs(&specs);
        assert!(result.is_err(), "malformed spec should fail to parse");
    }

    #[test]
    fn test_post_link_failure_lines_include_package_messages() {
        let prompt = PackageName::new_unchecked("anaconda_prompt");
        let powershell = PackageName::new_unchecked("anaconda_powershell_prompt");
        let result = PrePostLinkResult {
            messages: HashMap::from([
                (
                    prompt.clone(),
                    "\nThis package requires menuinst v2.1.1 in the base environment.\n"
                        .to_string(),
                ),
                (powershell.clone(), "".to_string()),
            ]),
            failed_packages: vec![prompt, powershell],
        };

        let lines = post_link_failure_lines(&result);

        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("post-link scripts failed for 2 package(s)"));
        assert!(lines[0].contains("anaconda_prompt"));
        assert!(lines[0].contains("anaconda_powershell_prompt"));
        assert!(lines[1].contains("anaconda_prompt"));
        assert!(lines[1].contains("menuinst v2.1.1"));

        let error = validate_post_link_script_result(Some(&Ok(result)))
            .unwrap_err()
            .to_string();
        assert!(error.contains("post-link scripts failed"));
    }

    #[test]
    fn test_post_link_result_error_is_fatal() {
        let result = Err(LinkScriptError::IoError(
            "could not read post-link messages".to_string(),
            std::io::Error::other("broken message file"),
        ));

        let error = validate_post_link_script_result(Some(&result))
            .unwrap_err()
            .to_string();

        assert!(error.contains("failed to inspect post-link script results"));
        assert!(error.contains("could not read post-link messages"));
    }

    #[rstest]
    #[case::empty(vec![], 0)]
    #[case::conda_only(vec!["foo-1.0-h1.conda"], 1)]
    #[case::tar_bz2(vec!["bar-2.0-h2.tar.bz2"], 1)]
    #[case::mixed_with_junk(vec!["a-1-h1.conda", "b-2-h2.tar.bz2", "readme.txt"], 2)]
    fn test_index_bundle_dir(#[case] files: Vec<&str>, #[case] expected_count: usize) {
        let tmp = tempfile::TempDir::new().unwrap();
        for name in &files {
            std::fs::write(tmp.path().join(name), b"").unwrap();
        }
        let index = index_bundle_dir(tmp.path()).unwrap();
        assert_eq!(index.len(), expected_count);
        for (name, path) in &index {
            assert!(name.ends_with(".conda") || name.ends_with(".tar.bz2"));
            assert!(path.exists());
        }
    }

    #[test]
    fn test_bundle_archive_name_rejects_nested_paths() {
        let err = bundle_archive_name_from_path(Path::new("nested/foo-1.0-h1.conda"))
            .unwrap_err()
            .to_string();

        assert!(err.contains("must be flat package archive names"));
    }

    #[test]
    fn test_bundle_archive_name_rejects_non_package_suffix() {
        let err = bundle_archive_name_from_path(Path::new("readme.txt"))
            .unwrap_err()
            .to_string();

        assert!(err.contains("must end with .conda or .tar.bz2"));
    }

    #[test]
    #[cfg(unix)]
    fn test_index_bundle_dir_skips_symlinks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("target.conda");
        let link = tmp.path().join("linked.conda");
        std::fs::write(&target, b"package").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let index = index_bundle_dir(tmp.path()).unwrap();

        assert!(index.contains_key("target.conda"));
        assert!(!index.contains_key("linked.conda"));
    }

    fn make_record_with_url(filename: &str, data: &[u8]) -> RepoDataRecord {
        use rattler_conda_types::{
            PackageName, VersionWithSource,
            package::{CondaArchiveIdentifier, DistArchiveIdentifier},
        };
        use std::str::FromStr;

        let record = rattler_conda_types::PackageRecord::new(
            PackageName::new_unchecked("dummy"),
            VersionWithSource::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        let mut record_json = serde_json::to_value(&record).unwrap();
        let (digest, _) = crate::hash::sha256_reader(data).unwrap();
        record_json["sha256"] = serde_json::json!(crate::hash::hex(&digest));
        let record = serde_json::from_value(record_json).unwrap();
        RepoDataRecord {
            package_record: record,
            identifier: DistArchiveIdentifier::from(
                filename.parse::<CondaArchiveIdentifier>().unwrap(),
            ),
            url: format!("https://conda.anaconda.org/conda-forge/linux-64/{filename}")
                .parse()
                .unwrap(),
            channel: Some("conda-forge".to_string()),
        }
    }

    #[test]
    fn test_recovery_reinstalls_an_otherwise_unchanged_package() {
        let desired = make_record_with_url("dummy-1.0-0.conda", b"package");
        let current = PrefixRecord::from_repodata_record(desired.clone(), Vec::new());

        let unchanged = rattler::install::Transaction::from_current_and_desired(
            vec![current.clone()],
            vec![desired.clone()],
            None,
            None,
            Platform::Linux64,
        )
        .unwrap();
        assert_eq!(unchanged.packages_to_install(), 0);

        let reinstall = reinstall_package_names(std::slice::from_ref(&desired), true);
        let recovery = rattler::install::Transaction::from_current_and_desired(
            vec![current],
            vec![desired],
            Some(&reinstall),
            None,
            Platform::Linux64,
        )
        .unwrap();

        assert_eq!(recovery.packages_to_install(), 1);
    }

    #[rstest]
    #[case::all_found(
        vec!["a-1-h1.conda", "b-2-h2.conda"],
        vec!["a-1-h1.conda", "b-2-h2.conda"],
        0
    )]
    #[case::partial(
        vec!["a-1-h1.conda"],
        vec!["a-1-h1.conda", "b-2-h2.conda"],
        1
    )]
    #[case::none_found(vec![], vec!["a-1-h1.conda"], 1)]
    fn test_match_records_to_bundle(
        #[case] bundle_files: Vec<&str>,
        #[case] record_filenames: Vec<&str>,
        #[case] expected_missing: usize,
    ) {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut bundle_index = HashMap::new();
        for name in &bundle_files {
            let path = tmp.path().join(name);
            std::fs::write(&path, b"package").unwrap();
            bundle_index.insert(name.to_string(), path);
        }
        let records: Vec<RepoDataRecord> = record_filenames
            .iter()
            .map(|f| make_record_with_url(f, b"package"))
            .collect();
        let (matched, missing) = match_records_to_bundle(&records, &bundle_index).unwrap();
        assert_eq!(
            matched.len(),
            record_filenames.len() - expected_missing,
            "matched count"
        );
        assert_eq!(missing.len(), expected_missing, "missing count");
    }

    #[test]
    fn test_match_records_to_bundle_rejects_checksum_mismatch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let filename = "a-1-h1.conda";
        let path = tmp.path().join(filename);
        std::fs::write(&path, b"tampered").unwrap();
        let mut bundle_index = HashMap::new();
        bundle_index.insert(filename.to_string(), path);
        let records = vec![make_record_with_url(filename, b"package")];

        let err = match_records_to_bundle(&records, &bundle_index).unwrap_err();
        assert!(
            err.to_string().contains("SHA256 mismatch"),
            "unexpected error: {err:?}"
        );
    }

    fn bundle_lockfile(package_bytes: &[u8]) -> String {
        let platform = Platform::current();
        let (digest, _) = crate::hash::sha256_reader(package_bytes).unwrap();
        let sha256 = crate::hash::hex(&digest);
        format!(
            "version: 6\nenvironments:\n  default:\n    channels: []\n    packages:\n      {platform}:\n        - conda: https://example.invalid/{platform}/demo-1.0-0.conda\npackages:\n  - conda: https://example.invalid/{platform}/demo-1.0-0.conda\n    sha256: {sha256}\n"
        )
    }

    #[tokio::test]
    async fn test_offline_bundle_reports_missing_packages_without_creating_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bundle = tmp.path().join("bundle");
        let prefix = tmp.path().join("prefix");
        std::fs::create_dir(&bundle).unwrap();

        let error = from_lockfile_with_bundle_and_specs(
            &prefix,
            &bundle_lockfile(b"expected package"),
            &[],
            &bundle,
            true,
            false,
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("offline mode: 1 package(s) not found in bundle")
        );
        assert!(error.to_string().contains("demo-1.0-0.conda"));
        assert!(!prefix.exists());
        assert_eq!(std::fs::read_dir(bundle).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn test_offline_bundle_rejects_tampering_before_creating_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bundle = tmp.path().join("bundle");
        let prefix = tmp.path().join("prefix");
        std::fs::create_dir(&bundle).unwrap();
        let package = bundle.join("demo-1.0-0.conda");
        std::fs::write(&package, b"tampered package").unwrap();

        let error = from_lockfile_with_bundle_and_specs(
            &prefix,
            &bundle_lockfile(b"expected package"),
            &[],
            &bundle,
            true,
            false,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("SHA256 mismatch"));
        assert!(!prefix.exists());
        assert_eq!(std::fs::read(package).unwrap(), b"tampered package");
    }
}
