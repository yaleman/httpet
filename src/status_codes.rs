//! HTTP status code metadata loaded from a bundled JSON file.

use anyhow::Context;
use axum::http::header::USER_AGENT;
use html_escape::decode_html_entities;
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::LazyLock;

use crate::error::HttpetError;

/// MDN reference URL for HTTP status codes.
pub const MDN_STATUS_URL: &str =
    "https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Status";

/// Metadata for an HTTP status code.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct StatusInfo {
    /// Status name from MDN.
    pub name: String,
    /// Short summary text from MDN.
    pub summary: String,
    /// MDN reference URL for the status code.
    pub mdn_url: String,
}

type StatusCodes = BTreeMap<u16, StatusInfo>;

/// Global status code metadata loaded at startup.
pub static STATUS_CODES: LazyLock<StatusCodes> = LazyLock::new(|| match init() {
    Ok(status_codes) => status_codes,
    Err(err) => {
        tracing::error!(error=?err, "Failed to init status codes");
        std::process::abort();
    }
});

/// Parse the bundled status code metadata; called during startup.
pub fn init() -> Result<StatusCodes, HttpetError> {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/data/status_codes.json"
    ));
    let res: StatusCodes = serde_json::from_str(raw)?;
    Ok(res)
}

/// Fetches the MDN status code reference page.
pub fn fetch_status_page() -> anyhow::Result<String> {
    let mut response = ureq::get(MDN_STATUS_URL)
        .header(
            axum::http::HeaderName::from_str(USER_AGENT.as_ref())?,
            &format!("Httpet.org {}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .context("Failed to fetch MDN status code reference page")?;
    let body = response.body_mut();
    body.read_to_string()
        .context("Failed to read MDN response body")
}

/// parses the status entries from the MDN status code reference page
pub fn parse_status_entries(page_html: &str) -> anyhow::Result<Vec<(u16, String, String, String)>> {
    let entry_re = RegexBuilder::new(
        r#"<dt id="[^"]+">\s*<a href="([^"]+)"><code>(\d{3})\s+([^<]+)</code></a>.*?</dt>\s*<dd>\s*(.*?)</dd>"#,
    )
    .dot_matches_new_line(true)
    .build()
    .context("Failed to compile entry regex")?;
    let paragraph_re = RegexBuilder::new(r#"<p>(.*?)</p>"#)
        .dot_matches_new_line(true)
        .build()
        .context("Failed to compile paragraph regex")?;
    let tag_re = Regex::new(r#"<[^>]+>"#).context("Failed to compile tag regex")?;

    let mut entries = Vec::new();
    for captures in entry_re.captures_iter(page_html) {
        let href = captures
            .get(1)
            .context("Missing MDN href capture")?
            .as_str();
        let code = captures
            .get(2)
            .context("Missing status code capture")?
            .as_str();
        let name = captures
            .get(3)
            .context("Missing status name capture")?
            .as_str();
        let dd = captures
            .get(4)
            .context("Missing description capture")?
            .as_str();

        let paragraph = match paragraph_re.captures(dd) {
            Some(captures) => captures
                .get(1)
                .context("Missing paragraph capture")?
                .as_str(),
            None => continue,
        };

        let stripped = tag_re.replace_all(paragraph, "");
        let summary = decode_html_entities(stripped.trim()).to_string();
        let name = decode_html_entities(name).to_string();
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let code_num: u16 = code
            .parse()
            .with_context(|| format!("Invalid status code {code}"))?;
        let mdn_url = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("https://developer.mozilla.org{href}")
        };

        entries.push((code_num, name, summary, mdn_url));
    }

    entries.sort_by_key(|(code, _, _, _)| *code);
    Ok(entries)
}

/// writes out the file
pub fn write_status_codes(
    path: &PathBuf,
    entries: Vec<(u16, String, String, String)>,
) -> anyhow::Result<()> {
    let mut map = BTreeMap::new();
    for (code, name, summary, mdn_url) in entries {
        map.insert(
            code.to_string(),
            json!({ "name": name, "summary": summary, "mdn_url": mdn_url }),
        );
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let output = serde_json::to_string_pretty(&map).context("Failed to serialize JSON")?;
    fs::write(path, format!("{output}\n"))
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}
