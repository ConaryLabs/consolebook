//! Consolebook server library.
//!
//! The binary in `main.rs` is a thin command-line wrapper; everything it does
//! lives here so integration tests exercise the same code paths operators use.

pub mod backup;
pub mod data_dir;
pub mod doctor;
pub mod http;
pub mod storage;

/// Version of the running build, as reported by `/api/health` and `doctor`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
