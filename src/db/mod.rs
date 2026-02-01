//! Database things
pub mod entities;
pub mod migrations;
use std::sync::Arc;

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};
use tracing::error;

/// Production Database connection
pub async fn connect_db(path: &str, debug: bool) -> Result<Arc<DatabaseConnection>, DbErr> {
    let url = format!("sqlite://{}?mode=rwc", path);
    let mut options = ConnectOptions::new(url);
    options
        .sqlx_logging(debug)
        .acquire_timeout(std::time::Duration::from_secs(1))
        .connect_timeout(std::time::Duration::from_secs(1))
        .connect_lazy(false);
    Ok(Arc::new(
        Database::connect(options.clone())
            .await
            .inspect_err(|_| error!("Failed startup with options: {:?}", options))?,
    ))
}

#[cfg(test)]
/// In-memory test database connection
pub async fn connect_test_db() -> Result<Arc<DatabaseConnection>, DbErr> {
    Ok(Arc::new(Database::connect("sqlite::memory:").await?))
}
