//! The single-server lock.
//!
//! `serve` holds an exclusive OS file lock on `instance/serve.lock` for its
//! whole life. That gives two invariants cheaply: two servers cannot share
//! one data directory, and `restore` can refuse to replace a database that
//! a running server holds — a lock check, not a guess.

use std::fs::File;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::data_dir::DataDir;

/// An exclusive hold on the installation. Released on drop (process exit
/// included).
pub struct ServeLock {
    _file: File,
    path: PathBuf,
}

impl ServeLock {
    /// Acquires the lock, failing immediately if another process holds it.
    pub fn acquire(data_dir: &DataDir) -> Result<Self> {
        let path = data_dir.instance().join("serve.lock");
        let file = File::create(&path)
            .with_context(|| format!("creating lock file {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(std::fs::TryLockError::WouldBlock) => bail!(
                "another process holds {}; is the server running?",
                path.display()
            ),
            Err(std::fs::TryLockError::Error(err)) => {
                Err(err).with_context(|| format!("locking {}", path.display()))
            }
        }
    }

    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}
