//! The application-owned data directory.
//!
//! One installation is one data directory. The layout is deliberately boring
//! (see `docs/architecture.md`):
//!
//! ```text
//! data/
//! ├── consolebook.db
//! ├── backups/
//! ├── exports/
//! └── instance/
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Resolved locations inside a single installation's data directory.
#[derive(Debug, Clone)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    /// Wraps `root` without touching the filesystem.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Creates the directory layout if it does not exist yet.
    pub fn ensure_layout(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.backups(),
            self.exports(),
            self.instance(),
        ] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating directory {}", dir.display()))?;
        }
        Ok(())
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn database(&self) -> PathBuf {
        self.root.join("consolebook.db")
    }

    #[must_use]
    pub fn backups(&self) -> PathBuf {
        self.root.join("backups")
    }

    #[must_use]
    pub fn exports(&self) -> PathBuf {
        self.root.join("exports")
    }

    #[must_use]
    pub fn instance(&self) -> PathBuf {
        self.root.join("instance")
    }
}
