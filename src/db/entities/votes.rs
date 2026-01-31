//! DB storage for votes on pets
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "votes")]
/// Votes for pets on given dates
pub struct Model {
    #[sea_orm(primary_key)]
    /// db id
    pub id: i32,
    /// foreign key to pet
    pub pet_id: i32,
    /// date of vote
    pub vote_date: Date,
    /// number of votes on that date
    pub vote_count: i32,
}

/// relations for votes
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::pets::Entity",
        from = "Column::PetId",
        to = "super::pets::Column::Id"
    )]
    /// foreign key relation to pets
    Pets,
}

impl Related<super::pets::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Pets.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
