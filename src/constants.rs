//! Shared constants/setters for things
//!

use std::path::PathBuf;
use std::sync::LazyLock;

/// The default place we put images
pub static IMAGE_DIR: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("./images"));

/// Custom header for the animal used
pub const X_HTTPET_ANIMAL: &str = "x-httpet-animal";

#[cfg(test)]
/// Base domain used in tests
pub const TEST_BASE_DOMAIN: &str = "example.org";

/// Length of CSRF session tokens
pub const CSRF_SESSION_LENGTH: i64 = 300;
