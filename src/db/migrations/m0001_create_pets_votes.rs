use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Pets::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Pets::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Pets::Name).string().not_null().unique_key())
                    .col(
                        ColumnDef::new(Pets::Enabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(Pets::CreatedAt)
                            .date_time()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Votes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Votes::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Votes::PetId).integer().not_null())
                    .col(ColumnDef::new(Votes::VoteDate).date().not_null())
                    .col(
                        ColumnDef::new(Votes::VoteCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Votes::CreatedAt)
                            .date_time()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_votes_pet")
                            .from(Votes::Table, Votes::PetId)
                            .to(Pets::Table, Pets::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .index(
                        Index::create()
                            .name("idx_votes_pet_date")
                            .table(Votes::Table)
                            .col(Votes::PetId)
                            .col(Votes::VoteDate)
                            .unique(),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Votes::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Pets::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Pets {
    Table,
    Id,
    Name,
    Enabled,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Votes {
    Table,
    Id,
    PetId,
    VoteDate,
    VoteCount,
    CreatedAt,
}
