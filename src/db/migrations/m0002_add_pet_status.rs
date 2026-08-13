use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

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

        let update = Query::update()
            .table(Pets::Table)
            .value(
                Pets::Status,
                Expr::case(Expr::col(Pets::Enabled).eq(true), "enabled").finally("voting"),
            )
            .to_owned();
        manager.get_connection().execute(&update).await?;

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
    Enabled,
}
