//! Database migrations
use sea_orm_migration::prelude::*;

mod m0001_create_pets_votes;
mod m0002_add_pet_status;

/// Define the Migrator struct
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_create_pets_votes::Migration),
            Box::new(m0002_add_pet_status::Migration),
        ]
    }
}
