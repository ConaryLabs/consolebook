//! Versioned JSON export and import of whole program versions
//! (ADR 0007; PRINCIPLES.md 9; format spec in
//! docs/formats/program-version-export.md).
//!
//! Export is deterministic by construction — fixed member order, arrays in
//! the load order `programs::load_content` guarantees, compact UTF-8, no
//! insignificant whitespace — so identical configuration always exports
//! identical bytes, per the canonical-bytes principles in
//! docs/records-integrity.md. Import validates strictly and creates a new
//! draft; version numbers are assigned by the importing installation,
//! never carried in from the document.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::audit::EventKind;
use crate::capabilities::{self, Capability};
use crate::programs::{self, VersionContent};

/// Envelope discriminator for this document family.
pub const FORMAT: &str = "consolebook-program-version";
/// Current export format version. Bump with any change to the document
/// shape; import accepts exactly the versions it understands.
pub const FORMAT_VERSION: i64 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    format_version: i64,
    content: VersionContent,
}

/// Exports one version (draft or published) as the documented JSON, or
/// `None` when the version does not exist.
pub async fn export_version(pool: &SqlitePool, version_id: i64) -> Result<Option<String>> {
    let Some(content) = programs::load_content(pool, version_id).await? else {
        return Ok(None);
    };
    let envelope = Envelope {
        format: FORMAT.to_owned(),
        format_version: FORMAT_VERSION,
        content,
    };
    Ok(Some(
        serde_json::to_string(&envelope).context("serializing program version export")?,
    ))
}

/// Where an imported version lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTarget {
    /// Create a new program named by the document's `name`.
    NewProgram,
    /// Add the next draft version to an existing program.
    VersionOf(i64),
}

/// Why an import was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum ImportRefusal {
    CapabilityRequired,
    /// Not this document family, or a format version this build does not
    /// understand.
    UnsupportedFormat(String),
    Invalid(Vec<String>),
    ProgramNameTaken,
    NoSuchProgram,
}

/// Imports a program-version document as a new draft. The draft is
/// authored data like any other: it publishes through the normal path.
pub async fn import_version(
    pool: &SqlitePool,
    actor_user_id: i64,
    document: &str,
    target: ImportTarget,
) -> Result<std::result::Result<i64, ImportRefusal>> {
    if !capabilities::user_has(pool, actor_user_id, Capability::ManagePrograms).await? {
        return Ok(Err(ImportRefusal::CapabilityRequired));
    }
    let envelope: Envelope = match serde_json::from_str(document) {
        Ok(envelope) => envelope,
        Err(err) => {
            return Ok(Err(ImportRefusal::UnsupportedFormat(format!(
                "not a program-version export: {err}"
            ))));
        }
    };
    if envelope.format != FORMAT {
        return Ok(Err(ImportRefusal::UnsupportedFormat(format!(
            "unsupported format '{}'",
            envelope.format
        ))));
    }
    if envelope.format_version != FORMAT_VERSION {
        return Ok(Err(ImportRefusal::UnsupportedFormat(format!(
            "unsupported format version {}",
            envelope.format_version
        ))));
    }
    let problems = programs::validate_content(&envelope.content);
    if !problems.is_empty() {
        return Ok(Err(ImportRefusal::Invalid(problems)));
    }

    let mut tx = pool.begin().await.context("starting import")?;
    let program_id = match target {
        ImportTarget::NewProgram => {
            let name = envelope.content.name.trim();
            let taken: Option<i64> =
                sqlx::query_scalar("SELECT id FROM program WHERE name = ?1 COLLATE NOCASE")
                    .bind(name)
                    .fetch_optional(&mut *tx)
                    .await
                    .context("checking program name")?;
            if taken.is_some() {
                return Ok(Err(ImportRefusal::ProgramNameTaken));
            }
            programs::insert_program(&mut tx, name, actor_user_id).await?
        }
        ImportTarget::VersionOf(program_id) => {
            let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM program WHERE id = ?1")
                .bind(program_id)
                .fetch_optional(&mut *tx)
                .await
                .context("checking program")?;
            if exists.is_none() {
                return Ok(Err(ImportRefusal::NoSuchProgram));
            }
            program_id
        }
    };
    let version_id = programs::insert_version(
        &mut tx,
        program_id,
        &envelope.content,
        actor_user_id,
        EventKind::ProgramVersionImported,
    )
    .await?;
    tx.commit().await.context("committing import")?;
    Ok(Ok(version_id))
}
