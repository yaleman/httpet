use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "votes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub pet_id: i32,
    pub vote_date: Date,
    pub vote_count: i32,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::pets::Entity",
        from = "Column::PetId",
        to = "super::pets::Column::Id"
    )]
    Pets,
}

impl Related<super::pets::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Pets.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
