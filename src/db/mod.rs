pub mod entities;
pub mod migrations;

use sea_orm::{Database, DatabaseConnection, DbErr};

pub async fn connect_db(path: &str) -> Result<DatabaseConnection, DbErr> {
    let url = format!("sqlite://{}?mode=rwc", path);
    Database::connect(url).await
}

#[cfg(test)]
pub async fn connect_test_db() -> Result<DatabaseConnection, DbErr> {
    Database::connect("sqlite::memory:").await
}
