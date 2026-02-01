use super::prelude::*;
use crate::{
    db::entities::{pets, votes},
    status_codes,
    web::{middleware::AnimalDomain, status_codes_for},
};
use axum::response::Response;

#[derive(Template, WebTemplate)]
#[template(path = "vote_page.html")]
pub(crate) struct VotePageTemplate {
    pub(crate) name: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "vote_thanks.html")]
pub(crate) struct VoteThanksTemplate {
    pub(crate) name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TopPet {
    pub(crate) name: String,
    pub(crate) votes: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct StatusCodeEntry {
    pub(crate) code: u16,
    pub(crate) summary: String,
    pub(crate) mdn_url: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "home.html")]
pub(crate) struct HomeTemplate {
    pub(crate) enabled_pets: Vec<db::entities::pets::Model>,
    pub(crate) top_pets: Vec<TopPet>,
    pub(crate) state: AppState,
}

#[derive(Template, WebTemplate)]
#[template(path = "status_list.html")]
pub(crate) struct StatusListTemplate {
    pub(crate) name: String,
    pub(crate) status_codes: Vec<StatusCodeEntry>,
    pub(crate) base_domain: String,
    state: AppState,
}

/// handles the / GET
pub(crate) async fn root_handler(
    domain: AnimalDomain,
    State(state): State<AppState>,
) -> Result<Response, HttpetError> {
    // if it's a subdomain then handle that.
    if let Some(animal) = domain.animal.as_deref() {
        let enabled = state
            .enabled_pets
            .read()
            .await
            .contains(&animal.to_string());
        if !enabled {
            return Err(HttpetError::NeedsVote(state.base_url(), animal.to_string()));
        }
        let status_codes = status_codes_for(&state.image_dir, animal).await?;
        let status_map = status_codes::status_codes()
            .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
        let mut status_entries = Vec::with_capacity(status_codes.len());
        for code in status_codes {
            let Some(info) = status_map.get(&code) else {
                return Err(HttpetError::InternalServerError(format!(
                    "Missing metadata for status code {code}"
                )));
            };
            status_entries.push(StatusCodeEntry {
                code,
                summary: info.summary.clone(),
                mdn_url: info.mdn_url.clone(),
            });
        }

        return Ok(StatusListTemplate {
            name: animal.to_string(),
            status_codes: status_entries,
            base_domain: state.base_domain.clone(),
            state,
        }
        .into_response());
    }

    let db = &state.db;
    let enabled_pets = pets::Entity::enabled(db.as_ref()).await?;

    let today = Utc::now().date_naive();
    let start_date = today - Duration::days(6);
    let top_query = Query::select()
        .from(pets::Entity)
        .column(pets::Column::Name)
        .expr_as(
            Expr::col(votes::Column::VoteCount).sum(),
            Alias::new("total_votes"),
        )
        .join(
            JoinType::InnerJoin,
            votes::Entity,
            Expr::col((pets::Entity, pets::Column::Id))
                .equals((votes::Entity, votes::Column::PetId)),
        )
        .and_where(Expr::col((pets::Entity, pets::Column::Enabled)).eq(false))
        .and_where(Expr::col((votes::Entity, votes::Column::VoteDate)).gte(start_date))
        .and_where(Expr::col((votes::Entity, votes::Column::VoteDate)).lte(today))
        .group_by_col((pets::Entity, pets::Column::Id))
        .group_by_col((pets::Entity, pets::Column::Name))
        .order_by(Alias::new("total_votes"), Order::Desc)
        .limit(10)
        .to_owned();

    let stmt = StatementBuilder::build(&top_query, &DatabaseBackend::Sqlite);
    let rows = db.query_all(stmt).await?;
    let mut top_pets = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get("", "name")?;
        let votes: i64 = row.try_get("", "total_votes")?;
        top_pets.push(TopPet { name, votes });
    }
    Ok(HomeTemplate {
        enabled_pets,
        top_pets,
        state: state.clone(),
    }
    .into_response())
}
