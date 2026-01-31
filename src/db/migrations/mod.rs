//! Database migrations
use sea_orm_migration::prelude::*;

mod m0001_create_pets_votes;

/// Define the Migrator struct
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m0001_create_pets_votes::Migration)]
    }
}
