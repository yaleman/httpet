use httpet::{config::setup_logging, status_codes::fetch_status_page};

#[test]
fn test_fetch_status_page() {
    let _ = setup_logging(true);

    let result = fetch_status_page();
    assert!(
        result.is_ok(),
        "Failed to fetch status page: {:?}",
        result.err()
    );
    let content = result.unwrap();
    assert!(
        content.contains("HTTP response status codes"),
        "Fetched content does not contain expected text"
    );
}
