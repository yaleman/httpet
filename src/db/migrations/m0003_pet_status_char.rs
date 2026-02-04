use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                r#"UPDATE pets
SET status = CASE status
    WHEN 'enabled' THEN 'e'
    WHEN 'voting' THEN 'v'
    WHEN 'submitted' THEN 's'
    ELSE status
END"#
                    .to_string(),
            ))
            .await?;

        if backend != DbBackend::Sqlite {
            manager
                .get_connection()
                .execute(Statement::from_string(
                    backend,
                    "ALTER TABLE pets ALTER COLUMN status SET DEFAULT 's'".to_string(),
                ))
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                r#"UPDATE pets
SET status = CASE status
    WHEN 'e' THEN 'enabled'
    WHEN 'v' THEN 'voting'
    WHEN 's' THEN 'submitted'
    ELSE status
END"#
                    .to_string(),
            ))
            .await?;

        if backend != DbBackend::Sqlite {
            manager
                .get_connection()
                .execute(Statement::from_string(
                    backend,
                    "ALTER TABLE pets ALTER COLUMN status SET DEFAULT 'submitted'".to_string(),
                ))
                .await?;
        }

        Ok(())
    }
}
