use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let update = Query::update()
            .table(Pets::Table)
            .value(
                Pets::Status,
                Expr::case(Expr::col(Pets::Status).eq("enabled"), "e")
                    .case(Expr::col(Pets::Status).eq("voting"), "v")
                    .case(Expr::col(Pets::Status).eq("submitted"), "s")
                    .finally(Expr::col(Pets::Status)),
            )
            .to_owned();
        manager.get_connection().execute(&update).await?;

        if backend != DbBackend::Sqlite {
            manager
                .alter_table(
                    Table::alter()
                        .table(Pets::Table)
                        .modify_column(ColumnDef::new(Pets::Status).default("s"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let update = Query::update()
            .table(Pets::Table)
            .value(
                Pets::Status,
                Expr::case(Expr::col(Pets::Status).eq("e"), "enabled")
                    .case(Expr::col(Pets::Status).eq("v"), "voting")
                    .case(Expr::col(Pets::Status).eq("s"), "submitted")
                    .finally(Expr::col(Pets::Status)),
            )
            .to_owned();
        manager.get_connection().execute(&update).await?;

        if backend != DbBackend::Sqlite {
            manager
                .alter_table(
                    Table::alter()
                        .table(Pets::Table)
                        .modify_column(ColumnDef::new(Pets::Status).default("submitted"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Pets {
    Table,
    Status,
}
