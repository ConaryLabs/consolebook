//! Reading the ZIP container an export or packet arrives in.
//!
//! The `zip` crate reads entries by name; this module adds what the
//! verifier needs beyond it: one entry's bytes as `Ok(None)` when absent
//! or `Err(detail)` when the container cannot deliver it, and the central
//! directory walked directly (APPNOTE 6.3, ZIP64 fields included) so a
//! name written twice — which the reader collapses — stays visible.

use std::io::{Cursor, Read};

/// An archive read from bytes held in memory.
pub(crate) type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

/// Every entry name in the central directory, duplicates included, in
/// directory order. The `zip` reader indexes entries by name and keeps
/// one per name, so a name written twice — which extraction tools
/// resolve differently — is visible only here. The walk follows
/// APPNOTE 6.3: the end-of-central-directory record (the last record,
/// followed by at most a 65535-byte comment), the ZIP64 locator and
/// record when the classic fields overflow, then the fixed 46-byte
/// central headers with their variable name, extra, and comment parts.
pub(crate) fn central_directory_names(bytes: &[u8]) -> std::result::Result<Vec<String>, String> {
    const EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const ZIP64_LOCATOR: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
    const ZIP64_EOCD: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
    const CENTRAL_HEADER: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    let eocd = (0..=bytes.len().saturating_sub(22))
        .rev()
        .take(usize::from(u16::MAX) + 1)
        .find(|&at| bytes.get(at..at + 4) == Some(&EOCD[..]))
        .ok_or("no end-of-central-directory record")?;
    let mut count = u64::from(le_u16(bytes, eocd + 10)?);
    let mut start = u64::from(le_u32(bytes, eocd + 16)?);
    if count == u64::from(u16::MAX) || start == u64::from(u32::MAX) {
        let locator = eocd
            .checked_sub(20)
            .filter(|&at| bytes.get(at..at + 4) == Some(&ZIP64_LOCATOR[..]))
            .ok_or("ZIP64 fields without a ZIP64 locator")?;
        let zip64 = usize::try_from(le_u64(bytes, locator + 8)?)
            .map_err(|_| "ZIP64 record offset out of range".to_owned())?;
        if bytes.get(zip64..zip64 + 4) != Some(&ZIP64_EOCD[..]) {
            return Err("ZIP64 locator points at no ZIP64 record".to_owned());
        }
        count = le_u64(bytes, zip64 + 32)?;
        start = le_u64(bytes, zip64 + 48)?;
    }
    let mut at = usize::try_from(start).map_err(|_| "central directory offset out of range")?;
    let mut names = Vec::new();
    for _ in 0..count {
        if bytes.get(at..at + 4) != Some(&CENTRAL_HEADER[..]) {
            return Err(format!("no central directory header at offset {at}"));
        }
        let name_len = usize::from(le_u16(bytes, at + 28)?);
        let extra_len = usize::from(le_u16(bytes, at + 30)?);
        let comment_len = usize::from(le_u16(bytes, at + 32)?);
        let name = bytes
            .get(at + 46..at + 46 + name_len)
            .ok_or("truncated central directory header")?;
        names.push(String::from_utf8_lossy(name).into_owned());
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(names)
}

fn le_u16(bytes: &[u8], at: usize) -> std::result::Result<u16, String> {
    bytes
        .get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| format!("truncated record at offset {at}"))
}

fn le_u32(bytes: &[u8], at: usize) -> std::result::Result<u32, String> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("truncated record at offset {at}"))
}

fn le_u64(bytes: &[u8], at: usize) -> std::result::Result<u64, String> {
    bytes
        .get(at..at + 8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
        .ok_or_else(|| format!("truncated record at offset {at}"))
}

/// Reads one entry: `Ok(None)` when absent, `Err(detail)` when the
/// container cannot deliver it (a CRC mismatch included).
pub(crate) fn read_entry(
    archive: &mut Archive<'_>,
    name: &str,
) -> std::result::Result<Option<Vec<u8>>, String> {
    match archive.by_name(name) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|err| err.to_string())?;
            Ok(Some(bytes))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}
