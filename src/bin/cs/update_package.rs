use std::fs;
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use miette::{Context, IntoDiagnostic};
use rattler_conda_types::compression_level::CompressionLevel;
use rattler_conda_types::{PackageName, Platform, VersionWithSource};

use super::artifact::ArtifactInfo;
use super::{hash, runtime_data};

const UPDATE_PACKAGE_OUTPUT_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, serde::Serialize)]
struct UpdatePackageOutput {
    schema_version: u8,
    path: String,
    filename: String,
    package_name: String,
    runtime_version: String,
    build_number: u64,
    platform: String,
    sha256: String,
    size: u64,
    payload_sha256: String,
    payload_size: u64,
}

pub(crate) fn build_update_package(
    info_path: &Path,
    binary_override: Option<&Path>,
    out_dir: Option<&Path>,
    json: bool,
) -> miette::Result<()> {
    let output = package_update(info_path, binary_override, out_dir)?;
    if json {
        let mut rendered = serde_json::to_string_pretty(&output)
            .into_diagnostic()
            .context("failed to render update package JSON")?;
        rendered.push('\n');
        write_stdout(rendered.as_bytes())
    } else {
        write_stdout(format!("{}\n", output.path).as_bytes())
    }
}

fn package_update(
    info_path: &Path,
    binary_override: Option<&Path>,
    out_dir: Option<&Path>,
) -> miette::Result<UpdatePackageOutput> {
    let info_path = absolute_path(info_path)?;
    require_regular_file(&info_path, "artifact info")?;
    let info: ArtifactInfo = serde_json::from_slice(
        &fs::read(&info_path)
            .into_diagnostic()
            .with_context(|| format!("failed to read {}", info_path.display()))?,
    )
    .into_diagnostic()
    .with_context(|| format!("failed to parse {}", info_path.display()))?;
    validate_info(&info)?;

    let binary = match binary_override {
        Some(path) => absolute_path(path)?,
        None => info_path
            .parent()
            .ok_or_else(|| miette::miette!("artifact info path has no parent directory"))?
            .join(&info.binary),
    };
    require_regular_file(&binary, "runtime executable")?;
    let snapshot_dir = tempfile::tempdir()
        .into_diagnostic()
        .context("failed to create finalized runtime snapshot directory")?;
    let snapshot = snapshot_dir.path().join("runtime.snapshot");
    fs::copy(&binary, &snapshot)
        .into_diagnostic()
        .with_context(|| format!("failed to snapshot {}", binary.display()))?;

    let (payload_digest, payload_size) = hash::sha256_file(&snapshot)
        .into_diagnostic()
        .context("failed to hash finalized runtime snapshot")?;
    let payload_sha256 = hash::hex(&payload_digest);
    if binary_override.is_none() {
        validate_recorded_binary_checksum(&info, &payload_sha256, payload_size)?;
    }

    let stamped = runtime_data::read_from_path(&snapshot)
        .into_diagnostic()
        .context("failed to inspect finalized runtime snapshot")?
        .ok_or_else(|| miette::miette!("runtime executable is not stamped by conda-ship"))?;
    validate_stamp(&info, &stamped)?;

    let update = stamped
        .header
        .update
        .as_ref()
        .ok_or_else(|| miette::miette!("runtime executable has no update configuration"))?;
    if !update.supports_direct_update() {
        return Err(miette::miette!(
            "legacy externally managed runtimes cannot be packaged for direct executable updates"
        ));
    }
    validate_channel(&update.channel)?;
    let package_name = PackageName::from_str(&update.package)
        .into_diagnostic()
        .context("invalid runtime update package name")?;
    let package_name = package_name.as_normalized().to_string();
    VersionWithSource::from_str(&stamped.header.runtime_version)
        .into_diagnostic()
        .context("runtime version is not a valid conda package version")?;
    let platform = Platform::from_str(&stamped.header.platform)
        .into_diagnostic()
        .context("runtime platform is not a known conda platform")?;
    if matches!(platform, Platform::NoArch | Platform::Unknown) {
        return Err(miette::miette!(
            "runtime update packages must use a native conda platform"
        ));
    }

    validate_plain_filename(&stamped.header.artifact_name, "runtime artifact name")?;
    let payload = if platform.is_windows() {
        format!("{}.exe", stamped.header.artifact_name)
    } else {
        format!("bin/{}", stamped.header.artifact_name)
    };
    let package_stem = format!(
        "{}-{}-{}",
        package_name, stamped.header.runtime_version, update.build_number
    );
    let filename = format!("{package_stem}.conda");
    validate_plain_filename(&filename, "update package filename")?;
    let out_dir = match out_dir {
        Some(path) => absolute_path(path)?,
        None => info_path
            .parent()
            .ok_or_else(|| miette::miette!("artifact info path has no parent directory"))?
            .to_path_buf(),
    };
    fs::create_dir_all(&out_dir)
        .into_diagnostic()
        .with_context(|| format!("failed to create {}", out_dir.display()))?;
    let output_path = out_dir.join(&filename);
    if output_path.exists() {
        return Err(miette::miette!(
            "refusing to overwrite existing update package: {}",
            output_path.display()
        ));
    }

    let package_root = tempfile::tempdir()
        .into_diagnostic()
        .context("failed to create update package staging directory")?;
    stage_package_contents(
        package_root.path(),
        &snapshot,
        &payload,
        platform,
        &package_name,
        &stamped.header.runtime_version,
        update.build_number,
        &payload_sha256,
        payload_size,
    )?;

    let paths = [
        "info/index.json".to_string(),
        "info/files".to_string(),
        "info/paths.json".to_string(),
        payload,
    ]
    .iter()
    .map(|path| package_root.path().join(path))
    .collect::<Vec<_>>();
    let mut temporary = tempfile::NamedTempFile::new_in(&out_dir)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to create temporary package in {}",
                out_dir.display()
            )
        })?;
    rattler_package_streaming::write::write_conda_package(
        temporary.as_file_mut(),
        package_root.path(),
        &paths,
        CompressionLevel::Default,
        Some(1),
        &package_stem,
        None,
        None,
    )
    .into_diagnostic()
    .context("failed to write conda update package")?;
    temporary
        .as_file()
        .sync_all()
        .into_diagnostic()
        .context("failed to sync conda update package")?;
    temporary
        .persist_noclobber(&output_path)
        .map_err(|error| error.error)
        .into_diagnostic()
        .with_context(|| format!("failed to write {}", output_path.display()))?;

    let (package_digest, package_size) = hash::sha256_file(&output_path)
        .into_diagnostic()
        .with_context(|| format!("failed to hash {}", output_path.display()))?;
    Ok(UpdatePackageOutput {
        schema_version: UPDATE_PACKAGE_OUTPUT_SCHEMA_VERSION,
        path: output_path.display().to_string(),
        filename,
        package_name,
        runtime_version: stamped.header.runtime_version,
        build_number: update.build_number,
        platform: platform.to_string(),
        sha256: hash::hex(&package_digest),
        size: package_size,
        payload_sha256,
        payload_size,
    })
}

#[allow(clippy::too_many_arguments)]
fn stage_package_contents(
    root: &Path,
    binary: &Path,
    payload: &str,
    platform: Platform,
    package_name: &str,
    runtime_version: &str,
    build_number: u64,
    payload_sha256: &str,
    payload_size: u64,
) -> miette::Result<()> {
    let package_platform = platform
        .only_platform()
        .ok_or_else(|| miette::miette!("update package platform must be native"))?;
    let package_arch = platform
        .arch()
        .ok_or_else(|| miette::miette!("update package platform must have an architecture"))?;
    let info_dir = root.join("info");
    let payload_path = root.join(payload);
    fs::create_dir_all(&info_dir)
        .into_diagnostic()
        .context("failed to create conda package info directory")?;
    fs::create_dir_all(
        payload_path
            .parent()
            .ok_or_else(|| miette::miette!("update package payload has no parent directory"))?,
    )
    .into_diagnostic()
    .context("failed to create update package payload directory")?;
    fs::copy(binary, &payload_path)
        .into_diagnostic()
        .with_context(|| format!("failed to copy {}", binary.display()))?;

    write_json(
        &info_dir.join("index.json"),
        &serde_json::json!({
            "arch": package_arch,
            "name": package_name,
            "version": runtime_version,
            "build": build_number.to_string(),
            "build_number": build_number,
            "depends": [],
            "platform": package_platform,
            "subdir": platform.to_string(),
        }),
    )?;
    fs::write(info_dir.join("files"), format!("{payload}\n"))
        .into_diagnostic()
        .context("failed to write conda package file list")?;
    write_json(
        &info_dir.join("paths.json"),
        &serde_json::json!({
            "paths": [{
                "_path": payload,
                "path_type": "hardlink",
                "sha256": payload_sha256,
                "size_in_bytes": payload_size,
            }],
            "paths_version": 1,
        }),
    )
}

fn validate_info(info: &ArtifactInfo) -> miette::Result<()> {
    if info.schema_version != 1 {
        return Err(miette::miette!(
            "unsupported artifact info schema version: {}",
            info.schema_version
        ));
    }
    if !matches!(info.layout.as_str(), "online" | "embedded") {
        return Err(miette::miette!(
            "update packages are supported only for online and embedded artifacts"
        ));
    }
    if info.update.is_none() {
        return Err(miette::miette!(
            "artifact info has no runtime update configuration"
        ));
    }
    validate_plain_filename(&info.binary, "artifact info binary")
}

fn validate_stamp(info: &ArtifactInfo, stamped: &runtime_data::RuntimeData) -> miette::Result<()> {
    let header = &stamped.header;
    let comparisons = [
        (
            "runtime name",
            header.runtime_name.as_str(),
            info.runtime_name.as_str(),
        ),
        (
            "artifact name",
            header.artifact_name.as_str(),
            info.artifact_name.as_str(),
        ),
        (
            "runtime version",
            header.runtime_version.as_str(),
            info.runtime_version.as_str(),
        ),
        (
            "artifact layout",
            header.artifact_layout.as_str(),
            info.layout.as_str(),
        ),
        ("platform", header.platform.as_str(), info.platform.as_str()),
    ];
    for (field, stamped_value, info_value) in comparisons {
        if stamped_value != info_value {
            return Err(miette::miette!(
                "runtime executable {field} is {stamped_value:?}, expected {info_value:?}"
            ));
        }
    }
    if header.update.as_ref() != info.update.as_ref() {
        return Err(miette::miette!(
            "runtime executable update configuration does not match artifact info"
        ));
    }
    match info.layout.as_str() {
        "online" if stamped.bundle.is_some() => Err(miette::miette!(
            "online runtime executable unexpectedly contains an embedded bundle"
        )),
        "embedded" => {
            let bundle = stamped.bundle.as_ref().ok_or_else(|| {
                miette::miette!("embedded runtime executable has no embedded bundle")
            })?;
            bundle
                .verify()
                .into_diagnostic()
                .context("failed to verify embedded runtime bundle")
        }
        "online" => Ok(()),
        _ => Err(miette::miette!(
            "update packages are supported only for online and embedded artifacts"
        )),
    }
}

fn validate_recorded_binary_checksum(
    info: &ArtifactInfo,
    actual_sha256: &str,
    actual_size: u64,
) -> miette::Result<()> {
    let expected = info
        .checksums
        .iter()
        .find(|checksum| checksum.path == info.binary)
        .ok_or_else(|| {
            miette::miette!(
                "artifact info has no checksum for its recorded binary: {}",
                info.binary
            )
        })?;
    if expected.sha256 != actual_sha256 || expected.bytes != actual_size {
        return Err(miette::miette!(
            "runtime executable does not match the checksum recorded in artifact info"
        ));
    }
    Ok(())
}

fn validate_channel(value: &str) -> miette::Result<()> {
    let channel = reqwest::Url::parse(value)
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
    Ok(())
}

fn validate_plain_filename(value: &str, field: &str) -> miette::Result<()> {
    let path = Path::new(value);
    let mut components = path.components();
    if value.is_empty()
        || value.contains(['/', '\\'])
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(miette::miette!("{field} must be a plain filename"));
    }
    Ok(())
}

fn require_regular_file(path: &Path, kind: &str) -> miette::Result<()> {
    let metadata = fs::metadata(path)
        .into_diagnostic()
        .with_context(|| format!("failed to inspect {kind} at {}", path.display()))?;
    if !metadata.is_file() {
        return Err(miette::miette!(
            "{kind} is not a regular file: {}",
            path.display()
        ));
    }
    Ok(())
}

fn absolute_path(path: &Path) -> miette::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .into_diagnostic()
            .context("failed to determine current directory")?
            .join(path))
    }
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> miette::Result<()> {
    let mut rendered = serde_json::to_vec_pretty(value)
        .into_diagnostic()
        .with_context(|| format!("failed to render {}", path.display()))?;
    rendered.push(b'\n');
    fs::write(path, rendered)
        .into_diagnostic()
        .with_context(|| format!("failed to write {}", path.display()))
}

fn write_stdout(content: &[u8]) -> miette::Result<()> {
    let mut stdout = io::stdout().lock();
    match stdout.write_all(content).and_then(|()| stdout.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error)
            .into_diagnostic()
            .context("failed to write output to stdout"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::{Seek as _, SeekFrom, Write as _};

    use rattler_conda_types::package::{IndexJson, PackageFile, PathType, PathsJson};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::artifact::{ArtifactChecksum, ArtifactInfo};

    fn update_config() -> runtime_data::RuntimeUpdateConfig {
        runtime_data::RuntimeUpdateConfig::new(
            "https://prefix.dev/demo".to_string(),
            "demo-runtime".to_string(),
            3,
        )
    }

    fn write_fixture(root: &Path, layout: &str) -> (PathBuf, PathBuf) {
        let binary = root.join("demo-linux-64");
        fs::write(&binary, b"runtime executable").unwrap();
        let bundle = (layout == "embedded").then(|| {
            let path = root.join("bundle.tar.zst");
            fs::write(&path, b"embedded bundle").unwrap();
            path
        });
        let mut header = runtime_data::RuntimeDataHeader::for_name("demo");
        header.artifact_name = "demo".to_string();
        header.runtime_name = "demo".to_string();
        header.runtime_version = "1.2.3".to_string();
        header.artifact_layout = layout.to_string();
        header.platform = "linux-64".to_string();
        header.update = Some(update_config());
        runtime_data::append_to_binary(&binary, &header, bundle.as_deref()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (digest, bytes) = hash::sha256_file(&binary).unwrap();
        let info = ArtifactInfo {
            schema_version: 1,
            name: "demo-linux-64".to_string(),
            artifact_name: "demo".to_string(),
            runtime_name: "demo".to_string(),
            runtime_version: "1.2.3".to_string(),
            layout: layout.to_string(),
            platform: "linux-64".to_string(),
            update: Some(update_config()),
            binary: "demo-linux-64".to_string(),
            bundle: None,
            lock: "demo.runtime.lock".to_string(),
            package_list: "demo.packages.txt".to_string(),
            sbom: None,
            package_count: 1,
            checksums: vec![ArtifactChecksum {
                path: "demo-linux-64".to_string(),
                sha256: hash::hex(&digest),
                bytes,
            }],
        };
        let info_path = root.join("demo-linux-64.info.json");
        write_json(&info_path, &info).unwrap();
        (info_path, binary)
    }

    #[test]
    fn packages_exact_stamped_runtime_with_standard_metadata() {
        let tmp = TempDir::new().unwrap();
        let (info, binary) = write_fixture(tmp.path(), "online");
        let output_dir = tmp.path().join("packages");

        let output = package_update(&info, None, Some(&output_dir)).unwrap();

        assert_eq!(output.filename, "demo-runtime-1.2.3-3.conda");
        assert_eq!(output.package_name, "demo-runtime");
        assert_eq!(output.build_number, 3);
        let extracted = TempDir::new().unwrap();
        rattler_package_streaming::fs::extract(Path::new(&output.path), extracted.path()).unwrap();
        assert_eq!(
            fs::read(extracted.path().join("bin/demo")).unwrap(),
            fs::read(binary).unwrap()
        );
        let index = IndexJson::from_package_directory(extracted.path()).unwrap();
        assert_eq!(index.name.as_normalized(), "demo-runtime");
        assert_eq!(index.version.to_string(), "1.2.3");
        assert_eq!(index.build_number, 3);
        assert!(index.depends.is_empty());
        assert_eq!(index.platform.as_deref(), Some("linux"));
        assert_eq!(index.arch.as_deref(), Some("x86_64"));
        assert_eq!(index.subdir.as_deref(), Some("linux-64"));
        let paths = PathsJson::from_package_directory(extracted.path()).unwrap();
        assert_eq!(paths.paths_version, 1);
        let [payload] = paths.paths.as_slice() else {
            panic!("expected one package payload")
        };
        assert_eq!(payload.relative_path, Path::new("bin/demo"));
        assert_eq!(payload.path_type, PathType::HardLink);
        assert_eq!(
            payload
                .sha256
                .as_ref()
                .map(|digest| hash::hex(digest.as_slice())),
            Some(output.payload_sha256.clone())
        );
        assert_eq!(payload.size_in_bytes, Some(output.payload_size));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(extracted.path().join("bin/demo"))
                .unwrap()
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0);
        }
    }

    #[rstest]
    #[case(Platform::Linux64, "linux", "x86_64")]
    #[case(Platform::LinuxAarch64, "linux", "aarch64")]
    #[case(Platform::Osx64, "osx", "x86_64")]
    #[case(Platform::OsxArm64, "osx", "arm64")]
    #[case(Platform::Win64, "win", "x86_64")]
    #[case(Platform::WinArm64, "win", "arm64")]
    fn packages_native_platform_identity(
        #[case] platform: Platform,
        #[case] expected_platform: &str,
        #[case] expected_arch: &str,
    ) {
        let tmp = TempDir::new().unwrap();
        let package_root = tmp.path().join("package");
        let binary = tmp.path().join("runtime");
        fs::write(&binary, b"runtime executable").unwrap();

        stage_package_contents(
            &package_root,
            &binary,
            "bin/demo",
            platform,
            "demo-runtime",
            "1.2.3",
            3,
            &"0".repeat(64),
            18,
        )
        .unwrap();

        let index = IndexJson::from_package_directory(&package_root).unwrap();
        assert_eq!(index.platform.as_deref(), Some(expected_platform));
        assert_eq!(index.arch.as_deref(), Some(expected_arch));
        assert_eq!(index.subdir.as_deref(), Some(platform.as_str()));
    }

    #[test]
    fn explicit_finalized_binary_may_differ_from_build_checksum() {
        let tmp = TempDir::new().unwrap();
        let (info, binary) = write_fixture(tmp.path(), "online");
        let finalized = tmp.path().join("signed-demo");
        fs::copy(binary, &finalized).unwrap();
        let mut finalized_file = OpenOptions::new().write(true).open(&finalized).unwrap();
        finalized_file.seek(SeekFrom::Start(0)).unwrap();
        finalized_file.write_all(b"R").unwrap();

        let output =
            package_update(&info, Some(&finalized), Some(&tmp.path().join("packages"))).unwrap();

        let (digest, size) = hash::sha256_file(&finalized).unwrap();
        assert_eq!(output.payload_sha256, hash::hex(&digest));
        assert_eq!(output.payload_size, size);
        let extracted = TempDir::new().unwrap();
        rattler_package_streaming::fs::extract(Path::new(&output.path), extracted.path()).unwrap();
        assert_eq!(
            fs::read(extracted.path().join("bin/demo")).unwrap(),
            fs::read(finalized).unwrap()
        );
    }

    #[test]
    fn recorded_binary_must_match_artifact_info_checksum() {
        let tmp = TempDir::new().unwrap();
        let (info, binary) = write_fixture(tmp.path(), "online");
        let mut file = OpenOptions::new().write(true).open(binary).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"R").unwrap();

        let error = package_update(&info, None, None).unwrap_err().to_string();

        assert!(error.contains("does not match the checksum"), "{error}");
    }

    #[test]
    fn existing_update_package_is_not_overwritten() {
        let tmp = TempDir::new().unwrap();
        let (info, _) = write_fixture(tmp.path(), "online");
        let output_dir = tmp.path().join("packages");
        fs::create_dir_all(&output_dir).unwrap();
        let existing = output_dir.join("demo-runtime-1.2.3-3.conda");
        fs::write(&existing, b"existing package").unwrap();

        let error = package_update(&info, None, Some(&output_dir))
            .unwrap_err()
            .to_string();

        assert!(error.contains("refusing to overwrite"), "{error}");
        assert_eq!(fs::read(existing).unwrap(), b"existing package");
    }

    #[test]
    fn identical_inputs_produce_identical_packages() {
        let tmp = TempDir::new().unwrap();
        let (info, _) = write_fixture(tmp.path(), "online");

        let first = package_update(&info, None, Some(&tmp.path().join("first"))).unwrap();
        let second = package_update(&info, None, Some(&tmp.path().join("second"))).unwrap();

        assert_eq!(first.sha256, second.sha256);
        assert_eq!(
            fs::read(first.path).unwrap(),
            fs::read(second.path).unwrap()
        );
    }

    #[test]
    fn stamped_identity_must_match_artifact_info() {
        let tmp = TempDir::new().unwrap();
        let (info_path, _) = write_fixture(tmp.path(), "online");
        let mut info: ArtifactInfo =
            serde_json::from_slice(&fs::read(&info_path).unwrap()).unwrap();
        info.runtime_name = "other-runtime".to_string();
        write_json(&info_path, &info).unwrap();

        let error = package_update(&info_path, None, None)
            .unwrap_err()
            .to_string();

        assert!(error.contains("runtime name"), "{error}");
    }

    #[test]
    fn external_artifacts_are_rejected() {
        let tmp = TempDir::new().unwrap();
        let (info, _) = write_fixture(tmp.path(), "external");

        let error = package_update(&info, None, None).unwrap_err().to_string();

        assert!(error.contains("only for online and embedded"), "{error}");
    }

    #[test]
    fn embedded_bundle_is_preserved_inside_payload() {
        let tmp = TempDir::new().unwrap();
        let (info, binary) = write_fixture(tmp.path(), "embedded");

        let output = package_update(&info, None, Some(&tmp.path().join("packages"))).unwrap();

        let extracted = TempDir::new().unwrap();
        rattler_package_streaming::fs::extract(Path::new(&output.path), extracted.path()).unwrap();
        assert_eq!(
            fs::read(extracted.path().join("bin/demo")).unwrap(),
            fs::read(binary).unwrap()
        );
    }
}
