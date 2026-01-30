use std::num::NonZeroU16;
use std::sync::Arc;

use askama::Template;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header::CONTENT_TYPE};
use axum::response::{Html, IntoResponse};
use chrono::{Duration, Utc};
use sea_orm::sea_query::{Alias, Expr, JoinType, OnConflict, Order, Query};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, QueryFilter, QueryOrder, Set, StatementBuilder,
};
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::db::entities::{pets, votes};

mod admin;
mod middleware;
mod views;

use admin::{admin_handler, create_pet_handler, update_pet_handler};
use middleware::AnimalDomain;
use views::{HomePet, HomeTemplate, TopPet, VotePageTemplate, VoteThanksTemplate};

#[derive(Clone, Debug)]
pub(crate) struct AppState {
    base_domain: String,
    enabled_pets: Arc<RwLock<Vec<String>>>,
    db: DatabaseConnection,
}

impl AppState {
    fn new(base_domain: &str, enabled_pets: Vec<String>, db: DatabaseConnection) -> Self {
        let base_domain = base_domain.trim_end_matches('.').to_ascii_lowercase();

        Self {
            base_domain,
            enabled_pets: Arc::new(RwLock::new(enabled_pets)),
            db,
        }
    }
}

async fn root_handler(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let db = &state.db;
    let enabled = pets::Entity::find()
        .filter(pets::Column::Enabled.eq(true))
        .order_by_asc(pets::Column::Name)
        .all(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let enabled_pets = enabled
        .into_iter()
        .map(|pet| HomePet { name: pet.name })
        .collect::<Vec<_>>();

    let today = Utc::now().date_naive();
    let start_date = today - Duration::days(6);
    let top_query = Query::select()
        .from(pets::Entity)
        .column(pets::Column::Name)
        .expr_as(Expr::col(votes::Column::VoteCount).sum(), Alias::new("total_votes"))
        .join(
            JoinType::InnerJoin,
            votes::Entity,
            Expr::col((pets::Entity, pets::Column::Id))
                .equals((votes::Entity, votes::Column::PetId)),
        )
        .and_where(Expr::col((votes::Entity, votes::Column::VoteDate)).gte(start_date))
        .and_where(Expr::col((votes::Entity, votes::Column::VoteDate)).lte(today))
        .group_by_col((pets::Entity, pets::Column::Id))
        .group_by_col((pets::Entity, pets::Column::Name))
        .order_by(Alias::new("total_votes"), Order::Desc)
        .limit(10)
        .to_owned();

    let stmt = StatementBuilder::build(&top_query, &DatabaseBackend::Sqlite);
    let rows = db
        .query_all(stmt)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut top_pets = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row
            .try_get("", "name")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let votes: i64 = row
            .try_get("", "total_votes")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        top_pets.push(TopPet { name, votes });
    }

    let html = HomeTemplate {
        enabled_pets,
        top_pets,
    }
    .render()
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(html))
}

async fn get_status_handler(
    domain: AnimalDomain,
    State(state): State<AppState>,
    Path(status_code): Path<u16>,
) -> axum::response::Response {
    let mut builder = axum::response::Response::builder().status(status_code);
    if let Some(animal) = domain.animal.as_deref() {
        let enabled = state
            .enabled_pets
            .read()
            .await
            .contains(&animal.to_string());
        if !enabled {
            return match (VotePageTemplate {
                name: animal.to_string(),
                status_code,
            })
            .render()
            {
                Ok(html) => Html(html).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
        }
        if let Ok(value) = HeaderValue::from_str(animal) {
            builder = builder.header("x-httpet-animal", value);
        }
    } else if let Ok(value) = HeaderValue::from_str(&domain.host) {
        builder = builder.header("x-httpet-host", value);
    }

    builder.body(axum::body::Body::empty()).unwrap()
}

async fn vote_pet_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Html<String>, StatusCode> {
    let db = &state.db;
    let pet = pets::Entity::find()
        .filter(pets::Column::Name.eq(&name))
        .one(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let pet_id = match pet {
        Some(model) => model.id,
        None => {
            let active = pets::ActiveModel {
                name: Set(name.clone()),
                enabled: Set(false),
                ..Default::default()
            };
            active
                .insert(db)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .id
        }
    };

    let today = Utc::now().date_naive();
    let insert = Query::insert()
        .into_table(votes::Entity)
        .columns([
            votes::Column::PetId,
            votes::Column::VoteDate,
            votes::Column::VoteCount,
        ])
        .values_panic([pet_id.into(), today.into(), 1.into()])
        .on_conflict(
            OnConflict::columns([votes::Column::PetId, votes::Column::VoteDate])
                .value(
                    votes::Column::VoteCount,
                    Expr::col(votes::Column::VoteCount).add(1),
                )
                .to_owned(),
        )
        .to_owned();

    let stmt = StatementBuilder::build(&insert, &DatabaseBackend::Sqlite);
    db.execute(stmt)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let html = VoteThanksTemplate { name: name.clone() }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(html))
}

fn create_router() -> Router<AppState> {
    Router::new()
        .route("/", axum::routing::get(root_handler))
        .route("/static/styles.css", axum::routing::get(styles_handler))
        .route("/{status_code}", axum::routing::get(get_status_handler))
        .route("/admin/", axum::routing::get(admin_handler))
        .route("/admin/pets", axum::routing::post(create_pet_handler))
        .route(
            "/admin/pets/{name}",
            axum::routing::post(update_pet_handler),
        )
        .route("/vote/{name}", axum::routing::post(vote_pet_handler))
}

async fn styles_handler() -> impl IntoResponse {
    const STYLES: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/static/styles.css"));
    ([(CONTENT_TYPE, "text/css")], STYLES)
}

pub async fn setup_server(
    listen_addr: &str,
    port: NonZeroU16,
    base_domain: &str,
    enabled_pets: Vec<String>,
    db: DatabaseConnection,
) -> Result<(), anyhow::Error> {
    let app = create_router().with_state(AppState::new(base_domain, enabled_pets, db));

    let addr = format!("{}:{}", listen_addr, port);
    info!("Starting server on http://{}", addr);
    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    if let Err(err) = axum::serve(listener, app).await {
        error!("Server error: {}", err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{header::CONTENT_TYPE, Request};
    use http_body_util::BodyExt;
    use sea_orm_migration::MigratorTrait;
    use tower::ServiceExt;

    async fn setup_state() -> AppState {
        let db = crate::db::connect_test_db()
            .await
            .expect("connect test db");
        crate::db::migrations::Migrator::up(&db, None)
            .await
            .expect("run migrations");
        let enabled = crate::db::entities::pets::enabled(&db)
            .await
            .expect("fetch enabled pets");
        AppState::new("httpet.org", enabled, db)
    }

    async fn read_body(response: axum::response::Response) -> String {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test]
    async fn unenabled_pet_returns_vote_page() {
        let state = setup_state().await;
        let app = create_router().with_state(state);

        let request = Request::builder()
            .method("GET")
            .uri("/500")
            .header("host", "dog.httpet.org")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_body(response).await;
        assert!(body.contains("Vote for dog"));
    }

    #[tokio::test]
    async fn enabled_pet_sets_header_and_status() {
        let state = setup_state().await;
        let db = state.db.clone();
        pets::ActiveModel {
            name: Set("dog".to_string()),
            enabled: Set(true),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert pet");

        let enabled = crate::db::entities::pets::enabled(&db)
            .await
            .expect("fetch enabled");
        let state = AppState::new("httpet.org", enabled, db);
        let app = create_router().with_state(state);

        let request = Request::builder()
            .method("GET")
            .uri("/500")
            .header("host", "dog.httpet.org")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::from_u16(500).unwrap());
        assert_eq!(
            response.headers().get("x-httpet-animal").unwrap(),
            "dog"
        );
        let body = read_body(response).await;
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn root_domain_sets_host_header() {
        let state = setup_state().await;
        let app = create_router().with_state(state);

        let request = Request::builder()
            .method("GET")
            .uri("/404")
            .header("host", "httpet.org")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::from_u16(404).unwrap());
        assert_eq!(
            response.headers().get("x-httpet-host").unwrap(),
            "httpet.org"
        );
    }

    #[tokio::test]
    async fn vote_endpoint_increments_daily_votes() {
        let state = setup_state().await;
        let db = state.db.clone();
        let app = create_router().with_state(state);

        for _ in 0..2 {
            let request = Request::builder()
                .method("POST")
                .uri("/vote/cat")
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        let pet = pets::Entity::find()
            .filter(pets::Column::Name.eq("cat"))
            .one(&db)
            .await
            .expect("fetch pet")
            .expect("pet exists");
        let today = Utc::now().date_naive();
        let vote = votes::Entity::find()
            .filter(votes::Column::PetId.eq(pet.id))
            .filter(votes::Column::VoteDate.eq(today))
            .one(&db)
            .await
            .expect("fetch votes")
            .expect("vote exists");

        assert_eq!(vote.vote_count, 2);
    }

    #[tokio::test]
    async fn admin_page_renders_pet_stats() {
        let state = setup_state().await;
        let db = state.db.clone();
        let app = create_router().with_state(state);

        let pet = pets::ActiveModel {
            name: Set("fox".to_string()),
            enabled: Set(true),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert pet");
        let today = Utc::now().date_naive();
        votes::ActiveModel {
            pet_id: Set(pet.id),
            vote_date: Set(today),
            vote_count: Set(4),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert votes");

        let request = Request::builder()
            .method("GET")
            .uri("/admin/")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = read_body(response).await;
        assert!(body.contains("httpet admin"));
        assert!(body.contains("fox.httpet.org"));
    }

    #[tokio::test]
    async fn homepage_lists_enabled_and_top_votes() {
        let state = setup_state().await;
        let db = state.db.clone();
        let app = create_router().with_state(state);

        let dog = pets::ActiveModel {
            name: Set("dog".to_string()),
            enabled: Set(true),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert dog");
        let cat = pets::ActiveModel {
            name: Set("cat".to_string()),
            enabled: Set(false),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert cat");
        let today = Utc::now().date_naive();
        votes::ActiveModel {
            pet_id: Set(cat.id),
            vote_date: Set(today),
            vote_count: Set(5),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert cat votes");
        votes::ActiveModel {
            pet_id: Set(dog.id),
            vote_date: Set(today),
            vote_count: Set(2),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert dog votes");

        let request = Request::builder()
            .method("GET")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = read_body(response).await;

        assert!(body.contains("Enabled pets"));
        assert!(body.contains("dog.httpet.org"));
        assert!(body.contains("Top votes"));
        assert!(body.contains("cat"));
    }

    #[tokio::test]
    async fn admin_update_toggles_enabled() {
        let state = setup_state().await;
        let db = state.db.clone();
        let enabled_list = state.enabled_pets.clone();
        let app = create_router().with_state(state);

        let request = Request::builder()
            .method("POST")
            .uri("/admin/pets/otter")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("enabled=on"))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let pet = pets::Entity::find()
            .filter(pets::Column::Name.eq("otter"))
            .one(&db)
            .await
            .expect("fetch pet")
            .expect("pet exists");
        assert!(pet.enabled);

        let enabled = enabled_list.read().await;
        assert!(enabled.contains(&"otter".to_string()));
    }

    #[tokio::test]
    async fn admin_create_pet_adds_dog() {
        let state = setup_state().await;
        let db = state.db.clone();
        let app = create_router().with_state(state);

        let request = Request::builder()
            .method("POST")
            .uri("/admin/pets")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("name=dog&enabled=on"))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let pet = pets::Entity::find()
            .filter(pets::Column::Name.eq("dog"))
            .one(&db)
            .await
            .expect("fetch pet")
            .expect("pet exists");
        assert!(pet.enabled);
    }

    #[tokio::test]
    async fn migrations_apply_cleanly() {
        let db = crate::db::connect_test_db()
            .await
            .expect("connect test db");
        crate::db::migrations::Migrator::up(&db, None)
            .await
            .expect("run migrations");

        pets::ActiveModel {
            name: Set("pangolin".to_string()),
            enabled: Set(false),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert pet");
    }
}
