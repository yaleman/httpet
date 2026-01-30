use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "pets")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    pub enabled: bool,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::votes::Entity")]
    Votes,
}

impl Related<super::votes::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Votes.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

pub async fn enabled(db: &DatabaseConnection) -> Result<Vec<String>, DbErr> {
    Ok(Entity::find()
        .filter(Column::Enabled.eq(true))
        .all(db)
        .await?
        .into_iter()
        .map(|pet| pet.name)
        .collect())
}
