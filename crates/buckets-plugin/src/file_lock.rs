//! Explicitly owned host file leases, shared by services and application stores.
use std::fs::{File, TryLockError};

/// Holds an exclusive file lock and releases it before closing the descriptor.
/// Callers open and validate the file; acquisition remains nonblocking.
pub struct ExclusiveFileLock(File);

impl ExclusiveFileLock {
    pub fn try_acquire(file: File) -> Result<Self, TryLockError> {
        file.try_lock()?;
        Ok(Self(file))
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        // Forked children can retain this open-file description until exec.
        // Explicit unlock prevents them from extending a completed lease.
        let _ = self.0.unlock();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs::OpenOptions, path::Path};

    fn open_lock_file(path: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap()
    }

    #[test]
    fn released_lock_does_not_wait_for_an_inherited_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("lease.lock");
        let open = || open_lock_file(&path);
        let writer = ExclusiveFileLock::try_acquire(open()).unwrap();
        // A dup shares the same open-file description, as a forked child does.
        // Keeping it alive makes the failure deterministic without spawning.
        let inherited = writer.0.try_clone().unwrap();
        assert!(matches!(
            ExclusiveFileLock::try_acquire(open()),
            Err(TryLockError::WouldBlock)
        ));
        drop(writer);

        let next = ExclusiveFileLock::try_acquire(open()).expect(
            "a completed lease must release its lock before an inherited descriptor closes",
        );
        drop(inherited);
        assert!(matches!(
            ExclusiveFileLock::try_acquire(open()),
            Err(TryLockError::WouldBlock)
        ));
        drop(next);
        assert!(ExclusiveFileLock::try_acquire(open()).is_ok());
    }

    #[test]
    fn failed_operation_releases_its_lease_while_a_duplicate_survives() {
        fn fail_after_acquiring(path: &Path) -> Result<(), File> {
            let lease = ExclusiveFileLock::try_acquire(open_lock_file(path)).unwrap();
            // Return an error while retaining a descriptor, as a child may do.
            Err(lease.0.try_clone().unwrap())
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("lease.lock");
        let inherited = fail_after_acquiring(&path).unwrap_err();
        let next = ExclusiveFileLock::try_acquire(open_lock_file(&path))
            .expect("an error return must not leave an inherited lease locked");
        drop(inherited);
        assert!(matches!(
            ExclusiveFileLock::try_acquire(open_lock_file(&path)),
            Err(TryLockError::WouldBlock)
        ));
        drop(next);
    }
}
