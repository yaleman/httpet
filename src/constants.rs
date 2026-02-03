//! Shared constants/setters for things
//!

use std::path::PathBuf;
use std::sync::LazyLock;

/// The default place we put images
pub static IMAGE_DIR: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("./images"));

/// Custom header for the animal used
pub const X_HTTPET_ANIMAL: &str = "x-httpet-animal";

/// Max age (in seconds) for image cache entries.
pub const IMAGE_CACHE_MAX_AGE_SECONDS: u64 = 60 * 60;

/// Shared cache max age (in seconds) for image cache entries.
pub const IMAGE_CACHE_S_MAXAGE_SECONDS: u64 = 60 * 60 * 24;

/// Stale-while-revalidate window (in seconds) for image cache entries.
pub const IMAGE_CACHE_STALE_WHILE_REVALIDATE_SECONDS: u64 = 60 * 60 * 24;

/// Cache-Control value for image responses.
pub static IMAGE_CACHE_CONTROL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "public, max-age={}, s-maxage={}, stale-while-revalidate={}",
        IMAGE_CACHE_MAX_AGE_SECONDS,
        IMAGE_CACHE_S_MAXAGE_SECONDS,
        IMAGE_CACHE_STALE_WHILE_REVALIDATE_SECONDS
    )
});

#[cfg(test)]
/// Base domain used in tests
pub const TEST_BASE_DOMAIN: &str = "example.org";

/// Length of CSRF session tokens
pub const CSRF_SESSION_LENGTH: i64 = 300;
