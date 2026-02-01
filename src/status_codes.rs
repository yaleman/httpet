//! HTTP status code metadata loaded from a bundled JSON file.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// Metadata for an HTTP status code.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct StatusInfo {
    /// Short summary text from MDN.
    pub summary: String,
    /// MDN reference URL for the status code.
    pub mdn_url: String,
}

/// Errors returned when loading status code metadata.
#[derive(Debug)]
pub enum StatusCodesError {
    /// The JSON payload could not be parsed.
    Parse(serde_json::Error),
    /// A status code key was not a valid u16.
    InvalidCode(String),
    /// The metadata has not been initialized.
    NotInitialized,
}

impl std::fmt::Display for StatusCodesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "Failed to parse status code JSON: {err}"),
            Self::InvalidCode(code) => {
                write!(f, "Invalid status code key in JSON: {code}")
            }
            Self::NotInitialized => write!(f, "Status code metadata has not been initialized"),
        }
    }
}

impl std::error::Error for StatusCodesError {}

static STATUS_CODES: OnceLock<BTreeMap<u16, StatusInfo>> = OnceLock::new();

/// Parse the bundled status code metadata; called during startup.
pub fn init() -> Result<(), StatusCodesError> {
    let raw = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/status_codes.json"));
    let parsed: BTreeMap<String, StatusInfo> =
        serde_json::from_str(raw).map_err(StatusCodesError::Parse)?;

    let mut converted = BTreeMap::new();
    for (code, info) in parsed {
        let code_num: u16 = code.parse().map_err(|_| StatusCodesError::InvalidCode(code))?;
        converted.insert(code_num, info);
    }

    let _ = STATUS_CODES.set(converted);
    Ok(())
}

/// Returns metadata for the given status code.
pub fn status_info(code: u16) -> Option<&'static StatusInfo> {
    STATUS_CODES.get()?.get(&code)
}

/// Returns the full list of status code metadata.
pub fn status_codes() -> Result<&'static BTreeMap<u16, StatusInfo>, StatusCodesError> {
    STATUS_CODES.get().ok_or(StatusCodesError::NotInitialized)
}
