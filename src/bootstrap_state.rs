//! Internal ownership state for an in-progress bootstrap.

use std::io::Write;
use std::path::{Path, PathBuf};

use miette::{Context, IntoDiagnostic};

use crate::policy;

const BOOTSTRAP_STATE_SCHEMA_VERSION: u8 = 1;
const BOOTSTRAP_STATE_FILE: &str = ".conda-ship-bootstrap.json";
const BOOTSTRAP_STATE_TEMP_FILE: &str = ".conda-ship-bootstrap.json.tmp";

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BootstrapPhase {
    Installing,
    Ready,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BootstrapState {
    schema_version: u8,
    state: BootstrapPhase,
    display_name: String,
    install_name: String,
    metadata_file: String,
}

impl BootstrapState {
    fn current(state: BootstrapPhase, identity: BootstrapIdentity<'_>) -> Self {
        Self {
            schema_version: BOOTSTRAP_STATE_SCHEMA_VERSION,
            state,
            display_name: identity.display_name.to_string(),
            install_name: identity.install_name.to_string(),
            metadata_file: identity.metadata_file.to_string(),
        }
    }

    pub(crate) fn phase(&self) -> BootstrapPhase {
        self.state
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BootstrapIdentity<'a> {
    pub display_name: &'a str,
    pub install_name: &'a str,
    pub metadata_file: &'a str,
}

fn embedded_identity() -> BootstrapIdentity<'static> {
    BootstrapIdentity {
        display_name: policy::display_name(),
        install_name: policy::install_name(),
        metadata_file: policy::metadata_file(),
    }
}

pub(crate) fn path(prefix: &Path) -> PathBuf {
    prefix.join(BOOTSTRAP_STATE_FILE)
}

fn temporary_path(prefix: &Path) -> PathBuf {
    prefix.join(BOOTSTRAP_STATE_TEMP_FILE)
}

fn remove_regular_file_if_present(path: &Path) -> miette::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(miette::miette!(
                "bootstrap state is not a regular file: {}",
                policy::path_for_display(path)
            ))
        }
        Ok(_) => std::fs::remove_file(path)
            .into_diagnostic()
            .with_context(|| {
                format!(
                    "failed to remove bootstrap state at {}",
                    policy::path_for_display(path)
                )
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).into_diagnostic(),
    }
}

fn read_state_file(path: &Path) -> miette::Result<Option<String>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).into_diagnostic().with_context(|| {
                format!(
                    "failed to inspect bootstrap state at {}",
                    policy::path_for_display(path)
                )
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(miette::miette!(
            "bootstrap state is not a regular file: {}",
            policy::path_for_display(path)
        ));
    }
    std::fs::read_to_string(path)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to read bootstrap state at {}",
                policy::path_for_display(path)
            )
        })
        .map(Some)
}

pub(crate) fn read(prefix: &Path) -> miette::Result<Option<BootstrapState>> {
    let path = path(prefix);
    let (path, data) = if let Some(data) = read_state_file(&path)? {
        (path, data)
    } else {
        let temporary_path = temporary_path(prefix);
        let Some(data) = read_state_file(&temporary_path)? else {
            return Ok(None);
        };
        (temporary_path, data)
    };
    let state = serde_json::from_str(&data)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to parse bootstrap state at {}",
                policy::path_for_display(&path)
            )
        })?;
    Ok(Some(state))
}

pub(crate) fn write_installing(prefix: &Path) -> miette::Result<()> {
    write_installing_for(prefix, embedded_identity())
}

pub(crate) fn write_installing_for(
    prefix: &Path,
    identity: BootstrapIdentity<'_>,
) -> miette::Result<()> {
    std::fs::create_dir_all(prefix)
        .into_diagnostic()
        .with_context(|| format!("failed to create {}", policy::path_for_display(prefix)))?;

    let path = path(prefix);
    let temporary_path = temporary_path(prefix);
    remove_regular_file_if_present(&temporary_path)?;
    let mut temporary = std::fs::File::create(&temporary_path)
        .into_diagnostic()
        .context("failed to create temporary bootstrap state")?;
    serde_json::to_writer_pretty(
        &mut temporary,
        &BootstrapState::current(BootstrapPhase::Installing, identity),
    )
    .into_diagnostic()
    .context("failed to render bootstrap state")?;
    temporary
        .write_all(b"\n")
        .into_diagnostic()
        .context("failed to write bootstrap state")?;
    temporary
        .sync_all()
        .into_diagnostic()
        .context("failed to sync bootstrap state")?;
    drop(temporary);

    remove_regular_file_if_present(&path)?;
    std::fs::rename(&temporary_path, &path)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to persist bootstrap state at {}",
                policy::path_for_display(&path)
            )
        })?;
    Ok(())
}

pub(crate) fn remove(prefix: &Path) -> miette::Result<()> {
    for path in [path(prefix), temporary_path(prefix)] {
        remove_regular_file_if_present(&path)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn validate_identity(state: &BootstrapState) -> miette::Result<()> {
    validate_identity_for(state, embedded_identity())
}

pub(crate) fn validate_identity_for(
    state: &BootstrapState,
    identity: BootstrapIdentity<'_>,
) -> miette::Result<()> {
    if state.schema_version != BOOTSTRAP_STATE_SCHEMA_VERSION {
        return Err(miette::miette!(
            "unsupported bootstrap state schema version: {}",
            state.schema_version
        ));
    }
    if state.display_name != identity.display_name {
        return Err(miette::miette!(
            "bootstrap state belongs to {}, not {}",
            state.display_name,
            identity.display_name
        ));
    }
    if state.install_name != identity.install_name {
        return Err(miette::miette!(
            "bootstrap state install name is {}, expected {}",
            state.install_name,
            identity.install_name
        ));
    }
    if state.metadata_file != identity.metadata_file {
        return Err(miette::miette!(
            "bootstrap state metadata file is {}, expected {}",
            state.metadata_file,
            identity.metadata_file
        ));
    }
    if state.state != BootstrapPhase::Installing {
        return Err(miette::miette!(
            "bootstrap ownership marker has invalid state: {:?}",
            state.state
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use tempfile::TempDir;

    #[test]
    fn test_bootstrap_installing_state_roundtrip() {
        let tmp = TempDir::new().unwrap();

        write_installing(tmp.path()).unwrap();
        let installing = read(tmp.path()).unwrap().unwrap();
        validate_identity(&installing).unwrap();
        assert_eq!(installing.phase(), BootstrapPhase::Installing);
    }

    #[test]
    fn test_bootstrap_state_rejects_another_runtime() {
        let mut state = BootstrapState::current(BootstrapPhase::Installing, embedded_identity());
        state.install_name = "another-runtime".to_string();

        let error = validate_identity(&state).unwrap_err().to_string();

        assert!(error.contains("another-runtime"));
        assert!(error.contains("expected"));
    }

    #[test]
    fn test_temporary_installing_state_is_recoverable() {
        let tmp = TempDir::new().unwrap();
        let state = BootstrapState::current(BootstrapPhase::Installing, embedded_identity());
        std::fs::write(
            temporary_path(tmp.path()),
            serde_json::to_vec_pretty(&state).unwrap(),
        )
        .unwrap();

        let recovered = read(tmp.path()).unwrap().unwrap();

        validate_identity(&recovered).unwrap();
        assert_eq!(recovered.phase(), BootstrapPhase::Installing);
        assert!(!path(tmp.path()).exists());
    }

    #[test]
    fn test_ready_marker_is_rejected() {
        let state = BootstrapState::current(BootstrapPhase::Ready, embedded_identity());

        let error = validate_identity(&state).unwrap_err().to_string();

        assert!(error.contains("invalid state"));
    }

    #[test]
    fn test_remove_cleans_final_and_temporary_markers() {
        let tmp = TempDir::new().unwrap();
        write_installing(tmp.path()).unwrap();
        std::fs::write(temporary_path(tmp.path()), b"stale").unwrap();

        remove(tmp.path()).unwrap();

        assert!(!path(tmp.path()).exists());
        assert!(!temporary_path(tmp.path()).exists());
    }

    #[rstest]
    fn test_marker_directories_are_preserved(
        #[values(BOOTSTRAP_STATE_FILE, BOOTSTRAP_STATE_TEMP_FILE)] marker_name: &str,
    ) {
        let tmp = TempDir::new().unwrap();
        let marker = tmp.path().join(marker_name);
        std::fs::create_dir(&marker).unwrap();
        let retained = marker.join("retained");
        std::fs::write(&retained, b"unrelated data").unwrap();

        for result in [
            read(tmp.path()).map(|_| ()),
            write_installing(tmp.path()),
            remove(tmp.path()),
        ] {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("not a regular file")
            );
            assert_eq!(std::fs::read(&retained).unwrap(), b"unrelated data");
        }
    }

    #[cfg(unix)]
    #[rstest]
    fn test_marker_symlinks_and_their_targets_are_preserved(
        #[values(BOOTSTRAP_STATE_FILE, BOOTSTRAP_STATE_TEMP_FILE)] marker_name: &str,
    ) {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("prefix");
        std::fs::create_dir(&prefix).unwrap();
        let target = tmp.path().join("external-state");
        std::fs::write(&target, b"unrelated data").unwrap();
        let marker = prefix.join(marker_name);
        std::os::unix::fs::symlink(&target, &marker).unwrap();

        for result in [
            read(&prefix).map(|_| ()),
            write_installing(&prefix),
            remove(&prefix),
        ] {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("not a regular file")
            );
            assert!(std::fs::symlink_metadata(&marker).unwrap().is_symlink());
            assert_eq!(std::fs::read(&target).unwrap(), b"unrelated data");
        }
    }

    #[test]
    fn test_invalid_final_marker_does_not_fall_back_to_temporary_state() {
        let tmp = TempDir::new().unwrap();
        write_installing(tmp.path()).unwrap();
        let valid_state = std::fs::read(path(tmp.path())).unwrap();
        std::fs::write(temporary_path(tmp.path()), &valid_state).unwrap();
        std::fs::write(path(tmp.path()), b"invalid state").unwrap();

        let error = read(tmp.path()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to parse bootstrap state")
        );
        assert_eq!(std::fs::read(path(tmp.path())).unwrap(), b"invalid state");
        assert_eq!(
            std::fs::read(temporary_path(tmp.path())).unwrap(),
            valid_state
        );
    }
}
