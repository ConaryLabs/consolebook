//! The canonical byte serializer (ADR 0011; `docs/records-integrity.md`).
//!
//! A finalized record's content is one JSON document under RFC 8785
//! (JSON Canonicalization Scheme) semantics, restricted to the closed
//! subset the format permits: UTF-8, ASCII object-member names in
//! sorted order, strings as authored, booleans, `null`, and integers
//! with magnitude below 2^53. Floats and larger magnitudes are refused
//! as defects, never rounded. Hashes are computed over exactly these
//! bytes; golden vectors in `tests/canonical_format.rs` pin every rule.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The canonicalization identifier stamped into every envelope.
pub const CANONICALIZATION: &str = "jcs-v1";

/// The record schema this build writes.
pub const RECORD_SCHEMA: i64 = 1;

/// Chain-hash domain separator (`docs/records-integrity.md`).
const CHAIN_DOMAIN: &[u8] = b"consolebook-version-v1";

/// The largest integer magnitude the format permits: 2^53 - 1 keeps
/// number serialization identical across JSON implementations.
const MAX_MAGNITUDE: i64 = (1 << 53) - 1;

/// Serializes a validated document to its canonical bytes.
///
/// `serde_json`'s default map orders members by key bytes, which
/// equals RFC 8785 member ordering for the ASCII-only names the
/// validation enforces, and its compact writer emits JCS string
/// escaping (the two-character escapes, lowercase `\u00xx` for other
/// control characters, everything else unescaped UTF-8) and plain
/// integer formatting. The golden vectors are the authority; this
/// implementation must keep matching them.
pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>> {
    validate(value, "$")?;
    serde_json::to_vec(value).context("serializing canonical document")
}

fn validate(value: &Value, path: &str) -> Result<()> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
        Value::Number(number) => {
            let Some(integer) = number.as_i64() else {
                bail!("the canonical subset takes integers only, at {path}");
            };
            if !(-MAX_MAGNITUDE..=MAX_MAGNITUDE).contains(&integer) {
                bail!("integer magnitude exceeds 2^53 - 1 at {path}");
            }
            Ok(())
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate(item, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        Value::Object(members) => {
            for (name, member) in members {
                if !name.is_ascii() {
                    bail!("non-ASCII member name {name:?} at {path}");
                }
                validate(member, &format!("{path}.{name}"))?;
            }
            Ok(())
        }
    }
}

/// The SHA-256 content hash of canonical bytes, lowercase hex.
#[must_use]
pub fn content_hash_hex(canonical: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical);
    to_hex(&hasher.finalize())
}

/// The domain-separated integrity-chain hash, lowercase hex:
/// `SHA-256(domain || 0x00 || predecessor || bytes)` with the
/// predecessor's raw 32-byte content hash, or 32 zero bytes for a
/// first version (ADR 0011 fixes the missing-predecessor treatment).
pub fn chain_hash_hex(predecessor_content_hash: Option<&str>, canonical: &[u8]) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(CHAIN_DOMAIN);
    hasher.update([0u8]);
    match predecessor_content_hash {
        Some(hex) => hasher.update(from_hex32(hex)?),
        None => hasher.update([0u8; 32]),
    }
    hasher.update(canonical);
    Ok(to_hex(&hasher.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn from_hex32(hex: &str) -> Result<[u8; 32]> {
    if hex.len() != 64 {
        bail!("a content hash is 64 hex characters, got {}", hex.len());
    }
    let mut bytes = [0u8; 32];
    for (index, chunk) in hex.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let pair = std::str::from_utf8(chunk).context("hash hex encoding")?;
        bytes[index] = u8::from_str_radix(pair, 16).context("hash hex encoding")?;
    }
    Ok(bytes)
}
