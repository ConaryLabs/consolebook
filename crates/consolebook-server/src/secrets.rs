//! Secret material: opaque tokens, one-time codes, and password hashing.
//!
//! Everything random comes from the operating system's entropy source.
//! Nothing secret is stored raw: passwords become Argon2id PHC strings;
//! session tokens and one-time codes are stored as SHA-256 hex digests and
//! compared by digest.

use anyhow::Result;
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use sha2::{Digest, Sha256};

/// Minimum accepted password length, in bytes of UTF-8.
pub const MIN_PASSWORD_BYTES: usize = 12;
/// Maximum accepted password length, bounding hash cost.
pub const MAX_PASSWORD_BYTES: usize = 512;

/// A freshly generated opaque secret: the raw value leaves the process once
/// (cookie, log line, or command output) and only the digest is persisted.
pub struct OpaqueSecret {
    pub raw: String,
    pub digest_hex: String,
}

impl std::fmt::Debug for OpaqueSecret {
    /// Never prints the raw secret; the digest is what the database stores.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpaqueSecret")
            .field("raw", &"<redacted>")
            .field("digest_hex", &self.digest_hex)
            .finish()
    }
}

fn random_bytes(n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf)
        .map_err(|err| anyhow::anyhow!("reading operating-system entropy: {err}"))?;
    Ok(buf)
}

fn random_hex(bytes: usize) -> Result<String> {
    Ok(hex_encode(&random_bytes(bytes)?))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// SHA-256 of a secret's UTF-8 bytes, hex-encoded, for storage and lookup.
#[must_use]
pub fn digest_hex(secret: &str) -> String {
    hex_encode(&Sha256::digest(secret.as_bytes()))
}

/// 256-bit session token.
pub fn generate_session_token() -> Result<OpaqueSecret> {
    let raw = random_hex(32)?;
    let digest_hex = digest_hex(&raw);
    Ok(OpaqueSecret { raw, digest_hex })
}

/// 128-bit one-time code (setup and password reset). Shorter than a session
/// token because a human relays it, still far beyond guessing range for a
/// code that lives minutes and is single-use.
pub fn generate_one_time_code() -> Result<OpaqueSecret> {
    let raw = random_hex(16)?;
    let digest_hex = digest_hex(&raw);
    Ok(OpaqueSecret { raw, digest_hex })
}

/// Validates the password policy shared by setup, reset, and future
/// password-change paths.
pub fn check_password_policy(
    password: &str,
    username: &str,
) -> std::result::Result<(), &'static str> {
    if password.len() < MIN_PASSWORD_BYTES {
        return Err("password must be at least 12 characters");
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err("password is too long");
    }
    if password.eq_ignore_ascii_case(username) {
        return Err("password must not equal the username");
    }
    Ok(())
}

/// Hashes a password with Argon2id (v19, default parameters: 19 MiB memory,
/// 2 iterations, 1 lane) into a PHC string that records its own parameters.
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::encode_b64(&random_bytes(16)?)
        .map_err(|err| anyhow::anyhow!("encoding salt: {err}"))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|err| anyhow::anyhow!("hashing password: {err}"))?;
    Ok(hash.to_string())
}

/// Verifies a password against a stored PHC string.
#[must_use]
pub fn verify_password(password: &str, phc_string: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc_string) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// A real PHC string for a throwaway password, verified against on login
/// attempts for unknown usernames so response timing does not reveal
/// whether an account exists.
pub fn dummy_password_hash() -> &'static str {
    use std::sync::OnceLock;
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_password("dummy-password-for-timing").expect("hashing constant dummy password")
    })
}
