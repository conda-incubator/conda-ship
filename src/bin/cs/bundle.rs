use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use miette::{Context, IntoDiagnostic};
use rattler_conda_types::Platform;
use rattler_lock::{CondaPackageData, LockFile};
use sha2::Digest;

use super::artifact::{validate_bundle_package_hashes, validate_package_archive_name};
use super::tls;

pub(crate) fn gen_bundle_from_lock(
    lock_file: &LockFile,
    runtime_lock_path: &Path,
    platform: Platform,
    bundle_path: &Path,
) -> miette::Result<PathBuf> {
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
    validate_bundle_package_hashes(&packages)?;

    eprintln!("downloading {} packages for {platform}...", packages.len());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .into_diagnostic()
        .context("failed to create tokio runtime")?;

    rt.block_on(download_and_bundle(&packages, bundle_path))
        .map_err(|err| miette::miette!("failed to download bundle: {err}"))?;
    Ok(bundle_path.to_path_buf())
}

async fn download_and_bundle(
    packages: &[&CondaPackageData],
    bundle_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures::stream::{self, StreamExt};

    tls::install_default_provider();
    let client = reqwest::Client::builder()
        .user_agent(crate::http::USER_AGENT)
        .no_gzip()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .build()?;

    let bundle_parent = bundle_path
        .parent()
        .ok_or_else(|| format!("bundle path has no parent: {}", bundle_path.display()))?;
    let bundle_dir = bundle_parent.join("bundle");
    std::fs::create_dir_all(bundle_parent)?;
    if let Ok(metadata) = std::fs::symlink_metadata(&bundle_dir) {
        if metadata.file_type().is_symlink() || metadata.is_file() {
            std::fs::remove_file(&bundle_dir)?;
        } else {
            std::fs::remove_dir_all(&bundle_dir)?;
        }
    }
    std::fs::create_dir_all(&bundle_dir)?;

    let start = std::time::Instant::now();

    let download_tasks = packages.iter().map(|pkg| {
        let client = client.clone();
        let bundle_dir = bundle_dir.clone();
        async move {
            let url = pkg
                .location()
                .as_url()
                .ok_or_else(|| format!("package location is not a URL: {:?}", pkg.location()))?;
            let archive_name = url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .ok_or_else(|| format!("package URL has no archive name: {url}"))?;
            validate_package_archive_name(archive_name)
                .map_err(|e| format!("invalid package archive name from {url}: {e}"))?;

            let dest = bundle_dir.join(archive_name);
            let expected = pkg
                .record()
                .ok_or_else(|| {
                    format!("{archive_name} is missing its package record in the runtime lock")
                })?
                .sha256
                .as_ref()
                .ok_or_else(|| format!("{archive_name} has no SHA256 in the runtime lock"))?;

            if dest.exists() {
                let (actual, _) = crate::hash::sha256_file(&dest)?;
                if actual.as_slice() == expected.as_slice() {
                    return Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
                }
                eprintln!("SHA256 mismatch for {archive_name}, re-downloading");
                std::fs::remove_file(&dest)?;
            }

            let mut response = client
                .get(url.clone())
                .send()
                .await
                .map_err(|e| format!("failed to fetch {archive_name}: {e}"))?;

            let status = response.status();
            if !status.is_success() {
                return Err(format!("HTTP {status} fetching {archive_name}").into());
            }

            let tmp_dest = dest.with_file_name(format!(".{archive_name}.download"));
            let mut out = std::fs::File::create(&tmp_dest)?;
            let mut hasher = sha2::Sha256::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| format!("failed to read {archive_name}: {e}"))?
            {
                hasher.update(&chunk);
                out.write_all(&chunk)?;
            }
            out.flush()?;
            drop(out);

            let actual = crate::hash::digest_to_array(hasher.finalize());
            if actual.as_slice() != expected.as_slice() {
                let _ = std::fs::remove_file(&tmp_dest);
                return Err(format!("SHA256 mismatch for {archive_name}").into());
            }

            std::fs::rename(&tmp_dest, &dest)?;
            Ok(())
        }
    });

    let results: Vec<_> = stream::iter(download_tasks)
        .buffer_unordered(8)
        .collect()
        .await;

    for result in results {
        result?;
    }

    eprintln!(
        "downloaded {} packages in {:.1}s, bundling...",
        packages.len(),
        start.elapsed().as_secs_f64()
    );

    let bundle_start = std::time::Instant::now();
    let out_file = std::fs::File::create(bundle_path)?;
    let zstd_encoder = zstd::Encoder::new(out_file, 1)?;
    let mut tar_builder = tar::Builder::new(zstd_encoder);

    for entry in std::fs::read_dir(&bundle_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && let Some(name) = path.file_name()
        {
            tar_builder.append_path_with_name(&path, name)?;
        }
    }

    let zstd_encoder = tar_builder.into_inner()?;
    zstd_encoder.finish()?;

    let bundle_size = std::fs::metadata(bundle_path)?.len();
    eprintln!(
        "bundle.tar.zst = {:.1} MB ({} packages, bundled in {:.1}s)",
        bundle_size as f64 / 1_048_576.0,
        packages.len(),
        bundle_start.elapsed().as_secs_f64()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;

    use rstest::rstest;
    use tempfile::TempDir;

    const PACKAGE_NAME: &str = "demo-1.0-0.conda";
    const PACKAGE_BYTES: &[u8] = b"locked package contents";

    fn serve_package(status: &'static str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(start.elapsed() < Duration::from_secs(10), "no request");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("package server failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with(&format!("GET /linux-64/{PACKAGE_NAME} ")));
            loop {
                line.clear();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                PACKAGE_BYTES.len()
            )
            .unwrap();
            stream.write_all(PACKAGE_BYTES).unwrap();
        });
        (format!("http://{address}/linux-64/{PACKAGE_NAME}"), server)
    }

    fn package_lock(url: &str, contents: &[u8]) -> LockFile {
        let sha256 = crate::hash::hex(&crate::hash::digest_to_array(sha2::Sha256::digest(
            contents,
        )));
        LockFile::from_str_with_base_directory(
            &format!(
                "version: 6\nenvironments:\n  default:\n    channels: []\n    packages:\n      linux-64:\n        - conda: {url}\npackages:\n  - conda: {url}\n    sha256: {sha256}\n"
            ),
            None,
        )
        .unwrap()
    }

    fn assert_bundle_contents(bundle: &Path) {
        let decoder = zstd::Decoder::new(std::fs::File::open(bundle).unwrap()).unwrap();
        let mut archive = tar::Archive::new(decoder);
        let mut entries = archive.entries().unwrap();
        let mut entry = entries.next().unwrap().unwrap();
        assert_eq!(entry.path().unwrap(), Path::new(PACKAGE_NAME));
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents).unwrap();
        assert_eq!(contents, PACKAGE_BYTES);
        assert!(entries.next().is_none());
    }

    #[rstest]
    fn test_bundle_contains_verified_download_and_removes_stale_contents(
        #[values(false, true)] stale_directory: bool,
    ) {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("bundle");
        if stale_directory {
            std::fs::create_dir(&bundle_dir).unwrap();
            std::fs::write(bundle_dir.join("obsolete-1.0-0.conda"), b"old package").unwrap();
        } else {
            std::fs::write(&bundle_dir, b"stale file").unwrap();
        }
        let bundle = tmp.path().join("bundle.tar.zst");
        let (url, server) = serve_package("200 OK");
        let lock = package_lock(&url, PACKAGE_BYTES);

        let result = gen_bundle_from_lock(
            &lock,
            &tmp.path().join("runtime.lock"),
            Platform::Linux64,
            &bundle,
        );
        server.join().unwrap();

        assert_eq!(result.unwrap(), bundle);
        assert_eq!(std::fs::read_dir(&bundle_dir).unwrap().count(), 1);
        assert_bundle_contents(&bundle);
    }

    #[rstest]
    #[case::http_error("503 Service Unavailable", PACKAGE_BYTES, "HTTP 503")]
    #[case::checksum_mismatch("200 OK", b"different locked contents", "SHA256 mismatch")]
    fn test_failed_download_preserves_completed_bundle(
        #[case] status: &'static str,
        #[case] locked_contents: &[u8],
        #[case] expected: &str,
    ) {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("bundle.tar.zst");
        std::fs::write(&bundle, b"previous completed bundle").unwrap();
        let (url, server) = serve_package(status);
        let lock = package_lock(&url, locked_contents);

        let result = gen_bundle_from_lock(
            &lock,
            &tmp.path().join("runtime.lock"),
            Platform::Linux64,
            &bundle,
        );
        server.join().unwrap();

        let error = result.unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
        assert_eq!(
            std::fs::read(&bundle).unwrap(),
            b"previous completed bundle"
        );
        assert_eq!(
            std::fs::read_dir(tmp.path().join("bundle"))
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_bundle_replaces_a_stale_symlink_without_modifying_its_target() {
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let retained = outside.join("retained");
        std::fs::write(&retained, b"unrelated data").unwrap();
        let bundle_dir = tmp.path().join("bundle");
        std::os::unix::fs::symlink(&outside, &bundle_dir).unwrap();
        let bundle = tmp.path().join("bundle.tar.zst");
        let (url, server) = serve_package("200 OK");
        let lock = package_lock(&url, PACKAGE_BYTES);

        let result = gen_bundle_from_lock(
            &lock,
            &tmp.path().join("runtime.lock"),
            Platform::Linux64,
            &bundle,
        );
        server.join().unwrap();

        result.unwrap();
        assert!(!std::fs::symlink_metadata(&bundle_dir).unwrap().is_symlink());
        assert_eq!(std::fs::read(&retained).unwrap(), b"unrelated data");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
        assert_bundle_contents(&bundle);
    }
}
