//! Regenerate data/status_codes.json from MDN's HTTP status reference page.

use anyhow::Result;
use httpet::status_codes::{fetch_status_page, parse_status_entries, write_status_codes};
use std::path::PathBuf;

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

    println!("Wrote {} ({} entries).", output_path.display(), entry_count);
    Ok(())
}
