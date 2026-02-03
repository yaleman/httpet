use super::csrf;
use super::prelude::*;
use crate::{
    db::entities::{pets, votes},
    status_codes,
    web::{middleware::AnimalDomain, status_codes_for},
};
use axum::response::Response;
use rand::prelude::IndexedRandom;
use serde_json::json;
use tokio::fs;

#[derive(Template, WebTemplate)]
#[template(path = "vote_page.html")]
pub(crate) struct VotePageTemplate {
    pub(crate) name: String,
    pub(crate) csrf_token: String,
    pub(crate) frontend_url: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "vote_thanks.html")]
pub(crate) struct VoteThanksTemplate {
    pub(crate) name: String,
    pub(crate) frontend_url: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TopPet {
    pub(crate) name: String,
    pub(crate) votes: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct StatusCodeEntry {
    pub(crate) code: u16,
    pub(crate) name: String,
    pub(crate) summary: String,
    pub(crate) mdn_url: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "home.html")]
pub(crate) struct HomeTemplate {
    pub(crate) enabled_pets: Vec<db::entities::pets::Model>,
    pub(crate) top_pets: Vec<TopPet>,
    pub(crate) state: AppState,
    pub(crate) csrf_token: String,
    pub(crate) frontend_url: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "not_found.html")]
pub(crate) struct NotFoundTemplate {
    pub(crate) has_image: bool,
    pub(crate) image_url: String,
    pub(crate) frontend_url: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "about.html")]
pub(crate) struct AboutTemplate {
    pub(crate) frontend_url: String,
    pub(crate) pet_example_url: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "status_list.html")]
pub(crate) struct StatusListTemplate {
    pub(crate) name: String,
    pub(crate) status_codes: Vec<StatusCodeEntry>,
    pub(crate) base_domain: String,
    pub(crate) info_link_prefix: String,
    pub(crate) frontend_url: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "status_info.html")]
pub(crate) struct StatusInfoTemplate {
    pub(crate) pet_name: String,
    pub(crate) status_code: u16,
    pub(crate) status_name: String,
    pub(crate) status_summary: String,
    pub(crate) mdn_url: String,
    pub(crate) image_url: String,
    pub(crate) frontend_url: String,
}

#[derive(Deserialize)]
pub(crate) struct InfoPath {
    pub(crate) pet: String,
    pub(crate) status_code: u16,
}

pub(crate) async fn pet_status_list(state: AppState, pet: &str) -> Result<Response, HttpetError> {
    let enabled = state.enabled_pets.read().await.contains(&pet.to_string());
    if !enabled {
        return Err(HttpetError::NeedsVote(state.base_url(), pet.to_string()));
    }

    let status_codes = status_codes_for(&state.image_dir, pet).await?;
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
            name: info.name.clone(),
            summary: info.summary.clone(),
            mdn_url: info.mdn_url.clone(),
        });
    }

    Ok(StatusListTemplate {
        name: pet.to_string(),
        status_codes: status_entries,
        base_domain: state.base_domain.clone(),
        info_link_prefix: format!("/info/{}", pet),
        frontend_url: frontend_url_for_state(&state),
    }
    .into_response())
}

pub(crate) async fn status_info_view(
    State(state): State<AppState>,
    Path(path): Path<InfoPath>,
) -> Result<Response, HttpetError> {
    let pet = normalize_pet_name_strict(&path.pet)?;
    if !(100..=599).contains(&path.status_code) {
        return Err(HttpetError::BadRequest);
    }

    let enabled = state.enabled_pets.read().await.contains(&pet);
    if !enabled {
        return Err(HttpetError::NeedsVote(state.base_url(), pet));
    }

    let image_path = state.image_path(&pet, path.status_code);
    match tokio::fs::metadata(&image_path).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(HttpetError::NotFound(format!(
                "{}",
                json!({"animal": pet, "status_code": path.status_code})
            )));
        }
        Err(err) => {
            return Err(HttpetError::InternalServerError(err.to_string()));
        }
    }

    let status_info = status_codes::status_info(path.status_code).ok_or_else(|| {
        HttpetError::NotFound(format!("{}", json!({"status_code": path.status_code})))
    })?;

    Ok(StatusInfoTemplate {
        pet_name: pet.clone(),
        status_code: path.status_code,
        status_name: status_info.name.clone(),
        status_summary: status_info.summary.clone(),
        mdn_url: status_info.mdn_url.clone(),
        image_url: format!("/{}/{}", pet, path.status_code),
        frontend_url: frontend_url_for_state(&state),
    }
    .into_response())
}

pub(crate) async fn not_found_response(state: &AppState) -> Response {
    let image_url = random_404_image_url(state).await;
    let has_image = image_url.is_some();
    let mut response = NotFoundTemplate {
        has_image,
        image_url: image_url.unwrap_or_default(),
        frontend_url: frontend_url_for_state(state),
    }
    .into_response();
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

pub(crate) async fn about_view(State(state): State<AppState>) -> Result<Response, HttpetError> {
    Ok(AboutTemplate {
        frontend_url: frontend_url_for_state(&state),
        pet_example_url: state.pet_base_url("dog"),
    }
    .into_response())
}

/// handles the / GET
pub(crate) async fn root_handler(
    domain: AnimalDomain,
    State(state): State<AppState>,
    session: Session,
) -> Result<Response, HttpetError> {
    // if it's a subdomain then handle that.
    if let Some(animal) = domain.animal.as_deref() {
        let animal = normalize_pet_name_strict(animal)?;
        return pet_status_list(state, &animal).await;
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
    let csrf_token = csrf::csrf_token(&session).await?;
    Ok(HomeTemplate {
        enabled_pets,
        top_pets,
        state: state.clone(),
        csrf_token,
        frontend_url: frontend_url_for_state(&state),
    }
    .into_response())
}

pub(crate) fn frontend_url_for_state(state: &AppState) -> String {
    if let Some(url) = state.frontend_url.as_ref() {
        url.to_string().trim_end_matches('/').to_string()
    } else if state.listen_port == 443 {
        format!("https://{}", state.base_domain)
    } else if state.listen_port == 80 {
        format!("http://{}", state.base_domain)
    } else {
        format!("http://{}:{}", state.base_domain, state.listen_port)
    }
}

async fn random_404_image_url(state: &AppState) -> Option<String> {
    let mut entries = fs::read_dir(&state.image_dir).await.ok()?;
    let mut candidates = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let file_type = match entry.file_type().await {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if normalize_pet_name(&dir_name) != dir_name {
            continue;
        }
        if normalize_pet_name_strict(&dir_name).is_err() {
            continue;
        }
        let image_path = entry.path().join("404.jpg");
        if fs::metadata(&image_path).await.is_ok() {
            candidates.push(dir_name);
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let mut rng = rand::rng();
    let pet = candidates.choose(&mut rng)?;
    Some(format!("/{pet}/404"))
}
