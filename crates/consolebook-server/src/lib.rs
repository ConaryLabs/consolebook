//! Consolebook server library.
//!
//! The binary in `main.rs` is a thin command-line wrapper; everything it does
//! lives here so integration tests exercise the same code paths operators use.

pub mod audit;
pub mod backup;
pub mod capabilities;
pub mod data_dir;
pub mod doctor;
pub mod http;
pub mod notices;
pub mod program_export;
pub mod programs;
pub mod programs_http;
pub mod restore;
pub mod scheduler;
pub mod secrets;
pub mod serve_lock;
pub mod sessions;
pub mod setup;
pub mod storage;
pub mod users;
pub mod web_assets;

/// Version of the running build, as reported by `/api/health` and `doctor`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
