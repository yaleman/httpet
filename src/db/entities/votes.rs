//! DB storage for votes on pets
use std::sync::Arc;

use chrono::Utc;
use sea_orm::{ActiveValue::Set, IntoActiveModel, TransactionTrait, entity::prelude::*};

use crate::{error::HttpetError, web::normalize_pet_name_strict};

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

pub(crate) async fn record_vote(
    db: &Arc<DatabaseConnection>,
    name: &str,
) -> Result<(), HttpetError> {
    let name = normalize_pet_name_strict(name)?;
    let db_txn = db.begin().await?;

    let pet = super::pets::Entity::find_by_name(&db_txn, &name).await?;

    let pet_id = match pet {
        Some(model) => model.id,
        None => {
            let active = super::pets::ActiveModel {
                name: Set(name.clone()),
                enabled: Set(false),
                status: Set(super::pets::PetStatus::Submitted),
                ..Default::default()
            };
            active.insert(&db_txn).await?.id
        }
    };

    let today = Utc::now().date_naive();

    match Entity::find()
        .filter(Column::PetId.eq(pet_id).and(Column::VoteDate.eq(today)))
        .one(&db_txn)
        .await?
    {
        Some(model) => {
            let vote_count = model.vote_count + 1;
            let mut am = model.into_active_model();
            am.vote_count = Set(vote_count);
            am.update(&db_txn).await?
        }
        None => {
            let active = ActiveModel {
                pet_id: Set(pet_id),
                vote_date: Set(today),
                vote_count: Set(1),
                ..Default::default()
            };
            active.insert(&db_txn).await?
        }
    };
    db_txn.commit().await?;

    Ok(())
}
