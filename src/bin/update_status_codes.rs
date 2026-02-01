//! Regenerate data/status_codes.json from MDN's HTTP status reference page.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use html_escape::decode_html_entities;
use regex::{Regex, RegexBuilder};
use serde_json::json;

const MDN_STATUS_URL: &str = "https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Status";

fn main() -> Result<()> {
    let html = fetch_status_page()?;
    let entries = parse_status_entries(&html)?;
    if entries.is_empty() {
        anyhow::bail!("No status entries found. The MDN page layout may have changed.");
    }

    let output_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("status_codes.json");
    let entry_count = entries.len();
    write_status_codes(&output_path, entries)?;

    println!(
        "Wrote {} ({} entries).",
        output_path.display(),
        entry_count
    );
    Ok(())
}

fn fetch_status_page() -> Result<String> {
    let response = ureq::get(MDN_STATUS_URL)
        .call()
        .context("Failed to fetch MDN status code reference page")?;
    response
        .into_string()
        .context("Failed to read MDN response body")
}

fn parse_status_entries(page_html: &str) -> Result<Vec<(u16, String, String)>> {
    let entry_re = RegexBuilder::new(
        r#"<dt id="[^"]+">\s*<a href="([^"]+)"><code>(\d{3})\s+[^<]+</code></a>.*?</dt>\s*<dd>\s*(.*?)</dd>"#,
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
        let dd = captures
            .get(3)
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
        let code_num: u16 = code
            .parse()
            .with_context(|| format!("Invalid status code {code}"))?;
        let mdn_url = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("https://developer.mozilla.org{href}")
        };

        entries.push((code_num, summary, mdn_url));
    }

    entries.sort_by_key(|(code, _, _)| *code);
    Ok(entries)
}

fn write_status_codes(path: &PathBuf, entries: Vec<(u16, String, String)>) -> Result<()> {
    let mut map = BTreeMap::new();
    for (code, summary, mdn_url) in entries {
        map.insert(code.to_string(), json!({ "summary": summary, "mdn_url": mdn_url }));
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
