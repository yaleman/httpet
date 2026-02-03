//! Database entities for pets

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(
    Clone, Copy, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize,
)]
#[sea_orm(rs_type = "String", db_type = "Text")]
/// Visibility status for a pet.
pub enum PetStatus {
    /// Newly submitted and hidden from public lists.
    #[sea_orm(string_value = "submitted")]
    Submitted,
    /// Eligible for voting and shown on the home page.
    #[sea_orm(string_value = "voting")]
    Voting,
    /// Enabled for public access.
    #[sea_orm(string_value = "enabled")]
    Enabled,
}

impl PetStatus {
    /// Returns the string representation used in storage and templates.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Voting => "voting",
            Self::Enabled => "enabled",
        }
    }
}

impl std::fmt::Display for PetStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PetStatus {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "submitted" => Ok(Self::Submitted),
            "voting" => Ok(Self::Voting),
            "enabled" => Ok(Self::Enabled),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "pets")]
/// Pets that can be viewed/voted on
pub struct Model {
    #[sea_orm(primary_key)]
    /// db id
    pub id: i32,
    /// pet name, should be normalised before insertion
    pub name: String,
    /// whether the pet is enabled for access - can't vote if enabled
    pub enabled: bool,
    /// status of pet visibility
    pub status: PetStatus,
    /// creation timestamp
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// relations for pets
pub enum Relation {
    #[sea_orm(has_many = "super::votes::Entity")]
    /// can't vote without a pet!
    Votes,
}

impl Related<super::votes::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Votes.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

impl Entity {
    /// List of enabled pets (Models
    pub async fn enabled(db: &DatabaseConnection) -> Result<Vec<Model>, DbErr> {
        Self::find()
            .filter(Column::Status.eq(PetStatus::Enabled))
            .all(db)
            .await
    }

    /// List of enabled pet name
    pub async fn enabled_names(db: &DatabaseConnection) -> Result<Vec<String>, DbErr> {
        Ok(Self::find()
            .filter(Column::Status.eq(PetStatus::Enabled))
            .all(db)
            .await?
            .into_iter()
            .map(|pet| pet.name)
            .collect())
    }

    /// Find a pet by name, helper function
    pub async fn find_by_name<C: ConnectionTrait>(
        db: &C,
        pet_name: &str,
    ) -> Result<Option<Model>, DbErr> {
        Self::find().filter(Column::Name.eq(pet_name)).one(db).await
    }
}
