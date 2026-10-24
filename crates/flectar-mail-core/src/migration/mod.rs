//! Moving accounts from other mail applications into Flectar.
//!
//! Import runs in three stages with separate responsibilities:
//!
//! 1. A source adapter ([`k9`], [`thunderbird`]) reads a foreign format into
//!    the source-neutral [`model`]. Adapters have no database or credential
//!    access and keep every source value, including ones Flectar cannot use.
//! 2. The [`plan`]ner decides, per account, whether and how it maps onto a
//!    Flectar account and records each difference as a [`plan::Finding`].
//!    The host shows this plan for review before anything is written.
//! 3. [`crate::Core::import_accounts`] persists the selected drafts after
//!    validating them again.
//!
//! Imports never carry passwords or tokens: every imported account waits for
//! the user to sign in, and existing Flectar accounts are never modified.
//! Flectar's own account backups are a restore operation, not a migration;
//! [`detect`] recognizes them so a host can offer one entry point for both.

mod apply;
pub mod k9;
pub mod model;
pub mod plan;
mod prefs;
pub mod thunderbird;

pub use apply::{AccountImportReport, FailedAccount, ImportedAccount, MAX_IMPORTED_ACCOUNTS};
pub use model::*;
pub use plan::{
    AccountDraft, AccountPlan, AliasDraft, Finding, FindingCode, PlanStatus, Severity, SignIn,
    SignatureDraft, plan,
};

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Largest settings file read into memory. Thunderbird profiles with many
/// add-ons can have multi-megabyte `prefs.js` files; account data is small.
pub const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

/// The format of a selected file, decided from its content rather than its
/// name: mobile pickers often rename files or drop extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedFormat {
    K9Settings,
    ThunderbirdPrefs,
    /// A ZIP archive, such as Thunderbird's profile export.
    Zip,
    /// A Flectar Mail account backup, which hosts restore instead.
    FlectarBackup,
}

/// What a selected file or folder contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Accounts(ImportInventory),
    /// The bytes of a Flectar Mail account backup, for the restore path.
    FlectarBackup(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("the file could not be read: {0}")]
    Unreadable(String),
    #[error("the file is larger than the {} MB limit", MAX_SOURCE_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("the file is not a supported settings export")]
    UnknownFormat,
    #[error("the {format} settings are damaged: {reason}")]
    Malformed {
        format: &'static str,
        reason: String,
    },
    #[error("settings format version {0} is not supported")]
    UnsupportedVersion(String),
    #[error("the settings contain no mail accounts")]
    NoAccounts,
    /// A Thunderbird data folder with several profiles and no default.
    #[error("the folder contains several Thunderbird profiles")]
    MultipleProfiles,
    /// A chosen folder that is neither a profile nor Thunderbird's data
    /// folder.
    #[error("the folder is not a Thunderbird profile")]
    NotAProfile,
    /// A Thunderbird data folder whose profile is on a network location,
    /// which is only opened when the user chooses it directly.
    #[error("the folder's Thunderbird profile is on a network location")]
    NetworkProfile,
}

pub fn detect(bytes: &[u8]) -> Option<DetectedFormat> {
    if bytes.starts_with(b"PK\x03\x04") {
        return Some(DetectedFormat::Zip);
    }
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(64 * 1024)]);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    if text.starts_with('{') {
        let backup = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
        return (backup.get("format").and_then(serde_json::Value::as_str)
            == Some("flectar-mail-account-backup"))
        .then_some(DetectedFormat::FlectarBackup);
    }
    if text.starts_with('<') {
        return text
            .contains("<k9settings")
            .then_some(DetectedFormat::K9Settings);
    }
    text.contains("user_pref(")
        .then_some(DetectedFormat::ThunderbirdPrefs)
}

/// Reads a K-9 export, a Thunderbird `prefs.js`, profile folder or profile
/// export, or a Flectar backup. Only settings files are read into memory, so
/// a profile export may be far larger than [`MAX_SOURCE_BYTES`].
pub fn open(path: &Path) -> Result<Source, SourceError> {
    if path.is_dir() {
        return accounts(thunderbird::read_profile_dir(path)?);
    }
    let mut file =
        std::fs::File::open(path).map_err(|error| SourceError::Unreadable(error.to_string()))?;
    let mut magic = [0; 4];
    let magic_len = read_prefix(&mut file, &mut magic)?;
    if detect(&magic[..magic_len]) == Some(DetectedFormat::Zip) {
        drop(file);
        return accounts(thunderbird::read_export(path)?);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| SourceError::Unreadable(error.to_string()))?;
    let bytes = read_bounded(file)?;
    match detect(&bytes) {
        Some(DetectedFormat::FlectarBackup) => Ok(Source::FlectarBackup(bytes)),
        Some(DetectedFormat::K9Settings) => accounts(k9::parse(&bytes)?),
        Some(DetectedFormat::ThunderbirdPrefs) => accounts(thunderbird::parse_prefs(&bytes)?),
        Some(DetectedFormat::Zip) | None => Err(SourceError::UnknownFormat),
    }
}

fn accounts(inventory: ImportInventory) -> Result<Source, SourceError> {
    if inventory.accounts.is_empty() {
        Err(SourceError::NoAccounts)
    } else {
        Ok(Source::Accounts(inventory))
    }
}

fn read_prefix(file: &mut std::fs::File, buffer: &mut [u8]) -> Result<usize, SourceError> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(SourceError::Unreadable(error.to_string())),
        }
    }
    Ok(filled)
}

fn read_bounded(reader: impl Read) -> Result<Vec<u8>, SourceError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| SourceError::Unreadable(error.to_string()))?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(SourceError::TooLarge);
    }
    Ok(bytes)
}
