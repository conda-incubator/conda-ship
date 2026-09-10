//! Cross-process serialization for automatic runtime bootstrap.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs4::FileExt;
use miette::{Context, IntoDiagnostic};

use crate::policy;

pub(crate) struct BootstrapLock {
    _file: File,
}

impl BootstrapLock {
    pub(crate) fn acquire(prefix: &Path) -> miette::Result<Self> {
        let path = path(prefix)?;
        let parent = path
            .parent()
            .ok_or_else(|| miette::miette!("bootstrap lock has no parent directory"))?;
        std::fs::create_dir_all(parent)
            .into_diagnostic()
            .with_context(|| {
                format!(
                    "failed to create bootstrap lock directory at {}",
                    policy::path_for_display(parent)
                )
            })?;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .into_diagnostic()
            .with_context(|| {
                format!(
                    "failed to open bootstrap lock at {}",
                    policy::path_for_display(&path)
                )
            })?;
        FileExt::lock(&file).into_diagnostic().with_context(|| {
            format!(
                "failed to acquire bootstrap lock at {}",
                policy::path_for_display(&path)
            )
        })?;
        Ok(Self { _file: file })
    }
}

pub(crate) fn path(prefix: &Path) -> miette::Result<PathBuf> {
    let parent = prefix.parent().ok_or_else(|| {
        miette::miette!(
            "install path has no parent directory: {}",
            policy::path_for_display(prefix)
        )
    })?;
    let name = prefix.file_name().ok_or_else(|| {
        miette::miette!(
            "install path has no final component: {}",
            policy::path_for_display(prefix)
        )
    })?;
    Ok(parent.join(format!(".{}.conda-ship.lock", name.to_string_lossy())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_lock_is_adjacent_to_prefix() {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("runtime");

        let lock = path(&prefix).unwrap();

        assert_eq!(lock.parent(), prefix.parent());
        assert!(!lock.starts_with(&prefix));
    }

    #[test]
    fn test_lock_excludes_another_handle_until_released() {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("nested").join("runtime");
        let lock = BootstrapLock::acquire(&prefix).unwrap();
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path(&prefix).unwrap())
            .unwrap();

        assert!(matches!(
            FileExt::try_lock(&contender),
            Err(fs4::TryLockError::WouldBlock)
        ));
        assert!(!prefix.exists());

        drop(lock);

        FileExt::try_lock(&contender).unwrap();
        FileExt::unlock(&contender).unwrap();
        BootstrapLock::acquire(&prefix).unwrap();
    }

    #[test]
    fn test_lock_does_not_truncate_an_existing_file() {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("runtime");
        let lock_path = path(&prefix).unwrap();
        std::fs::write(&lock_path, b"existing lock data").unwrap();

        let lock = BootstrapLock::acquire(&prefix).unwrap();
        drop(lock);

        assert_eq!(std::fs::read(lock_path).unwrap(), b"existing lock data");
    }

    #[test]
    fn test_lock_reports_parent_file_without_replacing_it() {
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path().join("occupied");
        std::fs::write(&parent, b"keep me").unwrap();

        let error = BootstrapLock::acquire(&parent.join("runtime"))
            .err()
            .expect("a file cannot contain the lock directory");

        assert!(
            error
                .to_string()
                .contains("failed to create bootstrap lock directory")
        );
        assert_eq!(std::fs::read(parent).unwrap(), b"keep me");
    }

    #[test]
    fn test_lock_reports_directory_at_lock_path_without_removing_it() {
        let tmp = TempDir::new().unwrap();
        let prefix = tmp.path().join("runtime");
        let lock_path = path(&prefix).unwrap();
        std::fs::create_dir(&lock_path).unwrap();
        std::fs::write(lock_path.join("keep"), b"keep me").unwrap();

        let error = BootstrapLock::acquire(&prefix)
            .err()
            .expect("a directory cannot be opened as a lock file");

        assert!(error.to_string().contains("failed to open bootstrap lock"));
        assert_eq!(std::fs::read(lock_path.join("keep")).unwrap(), b"keep me");
    }
}
