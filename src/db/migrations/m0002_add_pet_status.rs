use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Pets::Table)
                    .add_column(
                        ColumnDef::new(Pets::Status)
                            .string()
                            .not_null()
                            .default("submitted"),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute(Statement::from_string(
                manager.get_database_backend(),
                r#"UPDATE pets
SET status = CASE
    WHEN enabled = 1 THEN 'enabled'
    ELSE 'voting'
END"#
                    .to_string(),
            ))
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Pets::Table)
                    .drop_column(Pets::Status)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Pets {
    Table,
    Status,
}
