//! Bounded cross-platform advisory file locking.

use std::fs::File;
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};

use crate::managed_fs::{ManagedPath, ProjectFs};

use super::{StoreError, StoreErrorKind};

/// Timing policy for bounded plan-lock acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LockOptions {
    timeout: Duration,
    retry_interval: Duration,
}

impl LockOptions {
    /// Creates a bounded lock policy.
    ///
    /// # Errors
    ///
    /// Returns an error when either duration is zero.
    pub fn new(timeout: Duration, retry_interval: Duration) -> Result<Self, StoreError> {
        if timeout.is_zero() || retry_interval.is_zero() {
            return Err(StoreError::new(
                StoreErrorKind::InvalidMutation,
                "Lock timeout and retry interval must be positive",
            ));
        }
        Ok(Self {
            timeout,
            retry_interval,
        })
    }

    /// Returns the maximum acquisition duration.
    #[must_use]
    pub const fn timeout(self) -> Duration {
        self.timeout
    }

    /// Returns the delay between acquisition attempts.
    #[must_use]
    pub const fn retry_interval(self) -> Duration {
        self.retry_interval
    }
}

impl Default for LockOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(2),
            retry_interval: Duration::from_millis(10),
        }
    }
}

pub(crate) struct PlanLock {
    file: File,
}

impl PlanLock {
    pub(crate) fn acquire(
        filesystem: &ProjectFs,
        path: &ManagedPath,
        options: LockOptions,
    ) -> Result<Self, StoreError> {
        let display_path = filesystem.display_path(path);
        let file = filesystem
            .open_lock_file(path)
            .map_err(|error| StoreError::new(StoreErrorKind::Io, error.to_string()))?;
        let started_at = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started_at.elapsed() < options.timeout => {
                    let remaining = options.timeout.saturating_sub(started_at.elapsed());
                    thread::sleep(options.retry_interval.min(remaining));
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(StoreError::new(
                        StoreErrorKind::LockTimeout,
                        format!(
                            "Timed out after {} ms acquiring plan lock {}",
                            options.timeout.as_millis(),
                            display_path.display()
                        ),
                    ));
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }
}

impl Drop for PlanLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
