//! Web server/views/everything

use std::path::{Path as StdPath, PathBuf};

use crate::cli::CliOptions;
use crate::constants::{IMAGE_DIR, X_HTTPET_ANIMAL};
use crate::db::entities::pets;
use axum::Router;
use rand::prelude::IndexedRandom;
use sea_orm::{DatabaseTransaction, IntoActiveModel, TransactionTrait};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::RwLock;
use tower_http::services::ServeDir;

mod admin;
mod middleware;
mod prelude;
mod views;

use prelude::*;

use admin::{admin_handler, create_pet_handler, update_pet_handler};
use middleware::{AnimalDomain, admin_base_domain_only};
use url::Url;
use views::{VotePageTemplate, VoteThanksTemplate};

#[derive(Clone, Debug)]
pub(crate) struct AppState {
    base_domain: String,
    enabled_pets: Arc<RwLock<Vec<String>>>,
    db: Arc<DatabaseConnection>,
    pub(crate) image_dir: PathBuf,
    listen_port: u16,
    frontend_url: Option<Url>,
}

impl AppState {
    fn new(
        base_domain: &str,
        frontend_url: Option<Url>,
        enabled_pets: Vec<String>,
        db: Arc<DatabaseConnection>,
        image_dir: PathBuf,
        listen_port: u16,
    ) -> Self {
        let base_domain = base_domain
            .trim()
            .trim_end_matches(['.', '/'])
            .to_ascii_lowercase();

        Self {
            base_domain,
            frontend_url,
            enabled_pets: Arc::new(RwLock::new(enabled_pets)),
            db,
            image_dir,
            listen_port,
        }
    }

    pub fn base_url(&self) -> String {
        if let Some(url) = self.frontend_url.as_ref() {
            url.to_string().trim_end_matches('/').to_string()
        } else if self.listen_port == 443 {
            format!("https://{}", self.base_domain)
        } else if self.listen_port == 80 {
            format!("http://{}", self.base_domain)
        } else {
            format!("http://{}:{}", self.base_domain, self.listen_port)
        }
    }
    /// Gets the base URL for a given pet
    pub fn pet_base_url(&self, pet: &str) -> String {
        if let Some(url) = self.frontend_url.as_ref() {
            let mut pet_url = url.clone();
            if let Err(err) = pet_url.set_host(Some(&format!("{}.{}", pet, self.base_domain))) {
                error!(error=?err, pet=%pet, "Failed to set pet host on URL {}", url);
            }
            pet_url.to_string().trim_end_matches('/').to_string()
        } else if self.listen_port == 443 {
            format!("https://{}.{}", pet, self.base_domain)
        } else if self.listen_port == 80 {
            format!("http://{}.{}", pet, self.base_domain)
        } else {
            format!("http://{}.{}:{}", pet, self.base_domain, self.listen_port)
        }
    }

    /// Gets the image path for the given animal and status code
    pub fn image_path(&self, animal: &str, status_code: u16) -> std::path::PathBuf {
        self.image_dir
            .join(animal)
            .join(format!("{}.jpg", status_code))
    }

    pub(crate) async fn create_or_update_pet(
        &self,
        pet_name: &str,
        enabled: bool,
    ) -> Result<(), HttpetError> {
        let db_txn: DatabaseTransaction = self.db.as_ref().begin().await?;
        match pets::Entity::find_by_name(&db_txn, pet_name).await? {
            Some(model) => {
                let mut am = model.into_active_model();
                am.enabled = Set(enabled);
                am.update(&db_txn).await?
            }
            None => {
                pets::ActiveModel {
                    name: Set(pet_name.to_string()),
                    enabled: Set(enabled),
                    ..Default::default()
                }
                .insert(&db_txn)
                .await?
            }
        };

        db_txn.commit().await?;
        let mut enabled = self.enabled_pets.write().await;
        *enabled = pets::Entity::enabled(&self.db)
            .await?
            .into_iter()
            .map(|pet| pet.name)
            .collect();
        Ok(())
    }
}

#[cfg(test)]
impl AppState {
    fn write_test_image(&self, pet: &str, status: u16) -> std::path::PathBuf {
        let dir = self.image_dir.join(pet);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir_all(&dir).expect("create image dir");
        let path = dir.join(format!("{status}.jpg"));
        std::fs::write(&path, [0xFF, 0xD8, 0xFF, 0xD9]).expect("write image");
        path
    }
}

/// get the combintion of animal and status code
async fn get_status_handler(
    domain: AnimalDomain,
    State(state): State<AppState>,
    Path(status_code): Path<u16>,
) -> Result<axum::response::Response, HttpetError> {
    if let Some(animal) = domain.animal.as_deref() {
        return pet_status_response(&state, animal, status_code).await;
    }

    // return a random animal image for the root domain
    let enabled = state.enabled_pets.read().await.clone();
    if enabled.is_empty() {
        return Err(HttpetError::NotFound(format!(
            "{}",
            json!({"domain" : domain, "status_code": status_code})
        )));
    }
    let mut candidates = Vec::new();
    for animal in enabled {
        let image_path = state.image_path(&animal, status_code);
        match tokio::fs::metadata(&image_path).await {
            Ok(_) => candidates.push(animal),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                error!(
                    "Failed to read image metadata for {}: {}",
                    image_path.display(),
                    err
                );
                return Err(HttpetError::InternalServerError(err.to_string()));
            }
        }
    }

    let animal = {
        let mut rng = rand::rng();
        match candidates.choose(&mut rng) {
            Some(animal) => animal.to_string(),
            None => {
                return Err(HttpetError::NotFound(format!(
                    "{}",
                    json!({"domain" : domain, "status_code": status_code})
                )));
            }
        }
    };

    pet_status_response(&state, &animal, status_code).await
}

async fn pet_status_response(
    state: &AppState,
    animal: &str,
    status_code: u16,
) -> Result<axum::response::Response, HttpetError> {
    let enabled = state
        .enabled_pets
        .read()
        .await
        .contains(&animal.to_string());
    if !enabled {
        return Err(HttpetError::NeedsVote(state.base_url(), animal.to_string()));
    }
    let image_path = state.image_path(animal, status_code);
    let mut builder = axum::response::Response::builder();
    match tokio::fs::read(&image_path).await {
        Ok(bytes) => {
            if let Ok(value) = HeaderValue::from_str(animal) {
                builder = builder.header(X_HTTPET_ANIMAL, value);
            }
            builder = builder.header(CONTENT_TYPE, "image/jpeg");
            builder
                .body(axum::body::Body::from(bytes))
                .map_err(HttpetError::from)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(HttpetError::NotFound(
            format!("{}", json!({"animal": animal, "status_code": status_code})),
        )),
        Err(err) => {
            error!(
                "Failed to read image file {}: {}",
                image_path.display(),
                err
            );
            Err(HttpetError::InternalServerError(
                "Failed to access image, contact an admin!".to_string(),
            ))
        }
    }
}

async fn vote_pet_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<VoteThanksTemplate, HttpetError> {
    let name = normalize_pet_name(&name);
    record_vote(&state.db, &name).await?;
    Ok(VoteThanksTemplate { name: name.clone() })
}

/// View for voting page
async fn vote_pet_view(
    State(_appstate): State<AppState>,
    Path(name): Path<String>,
) -> VotePageTemplate {
    VotePageTemplate {
        name: normalize_pet_name(&name),
    }
}

#[derive(Deserialize)]
struct VoteForm {
    name: String,
}

async fn vote_form_handler(
    State(state): State<AppState>,
    Form(form): Form<VoteForm>,
) -> Result<VoteThanksTemplate, HttpetError> {
    let name = normalize_pet_name(&form.name);
    if name.is_empty() {
        return Err(HttpetError::BadRequest);
    }
    record_vote(&state.db, &name).await?;
    Ok(VoteThanksTemplate { name })
}

async fn pet_or_status_handler(
    domain: AnimalDomain,
    State(state): State<AppState>,
    Path(segment): Path<String>,
) -> Result<axum::response::Response, HttpetError> {
    if let Ok(status_code) = segment.parse::<u16>() {
        return get_status_handler(domain, State(state), Path(status_code)).await;
    }

    let pet = normalize_pet_name(&segment);
    let link_prefix = format!("/{}", pet);
    views::pet_status_list(state, &pet, &link_prefix).await
}

#[derive(Deserialize)]
struct PetStatusPath {
    pet: String,
    status_code: u16,
}

async fn pet_status_handler(
    State(state): State<AppState>,
    Path(path): Path<PetStatusPath>,
) -> Result<axum::response::Response, HttpetError> {
    let pet = normalize_pet_name(&path.pet);
    pet_status_response(&state, &pet, path.status_code).await
}

fn create_router(state: &AppState) -> Router<AppState> {
    let static_service = ServeDir::new("./static").append_index_html_on_directories(false);
    let admin_routes = Router::new()
        .route("/admin/", axum::routing::get(admin_handler))
        .route("/admin/pets", axum::routing::post(create_pet_handler))
        .route(
            "/admin/pets/{name}",
            axum::routing::post(update_pet_handler),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            admin_base_domain_only,
        ));
    Router::new()
        .merge(admin_routes)
        .route("/", axum::routing::get(views::root_handler))
        .route("/vote", axum::routing::post(vote_form_handler))
        .route(
            "/vote/{name}",
            axum::routing::post(vote_pet_handler).get(vote_pet_view),
        )
        .route(
            "/{pet}/{status_code}",
            axum::routing::get(pet_status_handler),
        )
        .route("/{segment}/", axum::routing::get(pet_or_status_handler))
        .route("/{segment}", axum::routing::get(pet_or_status_handler))
        .nest_service("/static", axum::routing::get_service(static_service))
}

pub(crate) fn normalize_pet_name(name: &str) -> String {
    let trimmed = name.trim().to_ascii_lowercase();
    if trimmed.len() > 1 && trimmed.ends_with('s') && !trimmed.ends_with("ss") {
        trimmed.trim_end_matches('s').to_string()
    } else {
        trimmed
    }
}

async fn status_codes_for(image_dir: &StdPath, animal: &str) -> Result<Vec<u16>, HttpetError> {
    let dir = image_dir.join(animal);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(HttpetError::InternalServerError(err.to_string())),
    };

    let mut codes = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let is_jpg = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("jpg"))
            .unwrap_or(false);
        if !is_jpg {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(code) = stem.parse::<u16>() {
            codes.push(code);
        }
    }

    codes.sort_unstable();
    Ok(codes)
}

/// Start the web server
pub async fn setup_server(
    cli: &CliOptions,
    enabled_pets: Vec<String>,
    db: Arc<DatabaseConnection>,
) -> Result<(), anyhow::Error> {
    let app_state = AppState::new(
        cli.base_domain.as_str(),
        cli.frontend_url.clone(),
        enabled_pets,
        db,
        IMAGE_DIR.clone(),
        cli.port.get(),
    );
    let app = create_router(&app_state).with_state(app_state);

    let addr = format!("{}:{}", cli.listen_address, cli.port.get());
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
    use crate::config::setup_logging;
    use crate::constants::{TEST_BASE_DOMAIN, X_HTTPET_ANIMAL};
    use crate::db::entities::votes;

    use super::*;

    use axum::body::Body;
    use axum::http::{Request, header::CONTENT_TYPE};
    use http_body_util::BodyExt;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use sea_orm_migration::MigratorTrait;
    use tempfile::TempDir;
    use tower::ServiceExt;
    use url::Url;

    struct TestState {
        app_state: AppState,
        _image_dir: TempDir,
    }

    async fn setup_test_state() -> TestState {
        let _ = setup_logging(true);
        crate::status_codes::init().expect("load status code metadata");
        let db = crate::db::connect_test_db().await.expect("connect test db");
        crate::db::migrations::Migrator::up(db.as_ref(), None)
            .await
            .expect("run migrations");
        let enabled = crate::db::entities::pets::Entity::enabled(db.as_ref())
            .await
            .expect("fetch enabled pets")
            .into_iter()
            .map(|pet| pet.name)
            .collect();
        let image_dir = tempfile::tempdir().expect("create temp image dir");
        let app_state = AppState::new(
            TEST_BASE_DOMAIN,
            None,
            enabled,
            db,
            image_dir.path().to_path_buf(),
            0,
        );
        TestState {
            app_state,
            _image_dir: image_dir,
        }
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
    async fn app_state_base_urls_trim_trailing_slashes() {
        let db = crate::db::connect_test_db().await.expect("connect test db");
        let image_dir = tempfile::tempdir().expect("create temp image dir");

        let app_state = AppState::new(
            "example.com/",
            None,
            Vec::new(),
            db.clone(),
            image_dir.path().to_path_buf(),
            3000,
        );
        assert_eq!(app_state.base_url(), "http://example.com:3000");
        assert!(!app_state.base_url().ends_with('/'));
        assert_eq!(app_state.pet_base_url("dog"), "http://dog.example.com:3000");
        assert!(!app_state.pet_base_url("dog").ends_with('/'));

        let frontend_url = Url::parse("https://example.com/front/").expect("parse frontend url");
        let app_state = AppState::new(
            "example.com",
            Some(frontend_url),
            Vec::new(),
            db,
            image_dir.path().to_path_buf(),
            443,
        );
        assert_eq!(app_state.base_url(), "https://example.com/front");
        assert!(!app_state.base_url().ends_with('/'));
        assert_eq!(
            app_state.pet_base_url("dog"),
            "https://dog.example.com/front"
        );
        assert!(!app_state.pet_base_url("dog").ends_with('/'));
    }

    #[tokio::test]
    async fn unenabled_pet_returns_vote_page() {
        let state = setup_test_state().await;
        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("GET")
            .uri("/500")
            .header("host", &format!("dog.{}", TEST_BASE_DOMAIN))
            .body(Body::empty())
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get("location")
            .expect("missing redirect location")
            .to_str()
            .expect("invalid location header");
        assert!(location.contains("/vote/dog"));
    }

    #[tokio::test]
    async fn enabled_pet_sets_header_and_status() {
        let state = setup_test_state().await;
        state
            .app_state
            .create_or_update_pet("dog", true)
            .await
            .expect("create pet");

        let _image_path = state.app_state.write_test_image("dog", 200);

        let app: Router = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("GET")
            .uri("/200")
            .header("host", &format!("dog.{}", TEST_BASE_DOMAIN))
            .body(Body::empty())
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");

        assert_eq!(
            response.status(),
            StatusCode::from_u16(200).expect("invalid status code")
        );
        assert_eq!(
            response
                .headers()
                .get(X_HTTPET_ANIMAL)
                .expect("missing header"),
            "dog"
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .expect("missing header"),
            "image/jpeg"
        );
        let body = read_body(response).await;
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn root_status_returns_enabled_pet_image() {
        let state = setup_test_state().await;
        state
            .app_state
            .create_or_update_pet("capybara", true)
            .await
            .expect("create pet");

        let _image_path = state.app_state.write_test_image("capybara", 418);
        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("GET")
            .uri("/418")
            .header("host", TEST_BASE_DOMAIN)
            .body(Body::empty())
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(X_HTTPET_ANIMAL)
                .expect("missing header"),
            "capybara"
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .expect("missing header"),
            "image/jpeg"
        );
        let body = read_body(response).await;
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn vote_endpoint_increments_daily_votes() {
        let state = setup_test_state().await;
        let db = state.app_state.db.clone();
        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        for _ in 0..2 {
            let request = Request::builder()
                .method("POST")
                .uri("/vote/cat")
                .body(Body::empty())
                .expect("create request");
            let response = app.clone().oneshot(request).await.expect("send request");
            assert_eq!(response.status(), StatusCode::OK);
        }

        let pet = pets::Entity::find_by_name(db.as_ref(), "cat")
            .await
            .expect("fetch pet")
            .expect("pet exists");
        let today = Utc::now().date_naive();
        let vote = votes::Entity::find()
            .filter(votes::Column::PetId.eq(pet.id))
            .filter(votes::Column::VoteDate.eq(today))
            .one(db.as_ref())
            .await
            .expect("fetch votes")
            .expect("vote exists");

        assert_eq!(vote.vote_count, 2);
    }

    #[tokio::test]
    async fn vote_form_adds_pet() {
        let state = setup_test_state().await;
        let db = state.app_state.db.clone();
        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("POST")
            .uri("/vote")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("name=lynx"))
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");
        assert_eq!(response.status(), StatusCode::OK);

        let pet = pets::Entity::find()
            .filter(pets::Column::Name.eq("lynx"))
            .one(db.as_ref())
            .await
            .expect("fetch pet")
            .expect("pet exists");
        let today = Utc::now().date_naive();
        let vote = votes::Entity::find()
            .filter(votes::Column::PetId.eq(pet.id))
            .filter(votes::Column::VoteDate.eq(today))
            .one(db.as_ref())
            .await
            .expect("fetch vote")
            .expect("vote exists");
        assert_eq!(vote.vote_count, 1);
    }

    #[tokio::test]
    async fn admin_page_renders_pet_stats() {
        let state = setup_test_state().await;
        let db = state.app_state.db.clone();
        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let pet = pets::ActiveModel {
            name: Set("fox".to_string()),
            enabled: Set(true),
            ..Default::default()
        }
        .insert(db.as_ref())
        .await
        .expect("insert pet");
        let today = Utc::now().date_naive();
        votes::ActiveModel {
            pet_id: Set(pet.id),
            vote_date: Set(today),
            vote_count: Set(4),
            ..Default::default()
        }
        .insert(db.as_ref())
        .await
        .expect("insert votes");

        let request = Request::builder()
            .method("GET")
            .uri("/admin/")
            .header("host", TEST_BASE_DOMAIN)
            .body(Body::empty())
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");
        let body = read_body(response).await;
        assert!(body.contains("httpet admin"));
        assert!(body.contains(&format!("fox.{}", TEST_BASE_DOMAIN)));
    }

    #[tokio::test]
    async fn admin_redirects_non_base_domain() {
        let state = setup_test_state().await;
        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("GET")
            .uri("/admin/?from=dog")
            .header("host", &format!("dog.{}", TEST_BASE_DOMAIN))
            .body(Body::empty())
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get("location")
            .expect("missing redirect location")
            .to_str()
            .expect("invalid location header");
        assert!(location.contains(TEST_BASE_DOMAIN));
        assert!(location.ends_with("/admin/?from=dog"));
    }

    #[tokio::test]
    async fn homepage_lists_enabled_and_top_votes() {
        let state = setup_test_state().await;
        let db = state.app_state.db.clone();
        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let dog = pets::ActiveModel {
            name: Set("dog".to_string()),
            enabled: Set(true),
            ..Default::default()
        }
        .insert(db.as_ref())
        .await
        .expect("insert dog");
        let cat = pets::ActiveModel {
            name: Set("cat".to_string()),
            enabled: Set(false),
            ..Default::default()
        }
        .insert(db.as_ref())
        .await
        .expect("insert cat");
        let today = Utc::now().date_naive();
        votes::ActiveModel {
            pet_id: Set(cat.id),
            vote_date: Set(today),
            vote_count: Set(5),
            ..Default::default()
        }
        .insert(db.as_ref())
        .await
        .expect("insert cat votes");
        votes::ActiveModel {
            pet_id: Set(dog.id),
            vote_date: Set(today),
            vote_count: Set(2),
            ..Default::default()
        }
        .insert(db.as_ref())
        .await
        .expect("insert dog votes");

        let request = Request::builder()
            .method("GET")
            .uri("/")
            .header("host", TEST_BASE_DOMAIN)
            .body(Body::empty())
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");
        let body = read_body(response).await;

        assert!(body.contains("Available pets"));
        assert!(body.contains(&format!("dog.{}", TEST_BASE_DOMAIN)));
        assert!(body.contains("Top votes"));
        let top_votes_section = body
            .split("Top votes (last 7 days)")
            .nth(1)
            .expect("missing top votes section")
            .split("Vote for a pet")
            .next()
            .expect("missing vote section");
        assert!(top_votes_section.contains("cat"));
        assert!(!top_votes_section.contains("dog"));
    }

    #[tokio::test]
    async fn subdomain_root_lists_status_codes() {
        let state = setup_test_state().await;

        state
            .app_state
            .create_or_update_pet("dog", true)
            .await
            .expect("create pet");
        let _image_path = state.app_state.write_test_image("dog", 404);

        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("GET")
            .uri("/")
            .header("host", &format!("dog.{}", TEST_BASE_DOMAIN))
            .body(Body::empty())
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");
        let body = read_body(response).await;
        assert!(body.contains("Part of the"));
        assert!(body.contains("404"));
        assert!(body.contains("href=\"/404\""));
        assert!(!body.contains("href=\"/dog/404\""));
    }

    #[tokio::test]
    async fn path_root_lists_status_codes() {
        let state = setup_test_state().await;

        state
            .app_state
            .create_or_update_pet("dog", true)
            .await
            .expect("create pet");
        let _image_path = state.app_state.write_test_image("dog", 404);

        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("GET")
            .uri("/dog/")
            .header("host", TEST_BASE_DOMAIN)
            .body(Body::empty())
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");
        let body = read_body(response).await;
        assert!(body.contains("Part of the"));
        assert!(body.contains("MDN"));
        assert!(body.contains("404"));
        assert!(body.contains("href=\"/dog/404\""));
        assert!(!body.contains("href=\"/404\""));
    }

    #[tokio::test]
    async fn path_status_returns_image() {
        let state = setup_test_state().await;
        state
            .app_state
            .create_or_update_pet("dog", true)
            .await
            .expect("create pet");

        let _image_path = state.app_state.write_test_image("dog", 200);

        let app: Router = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("GET")
            .uri("/dog/200")
            .header("host", TEST_BASE_DOMAIN)
            .body(Body::empty())
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");

        assert_eq!(
            response.status(),
            StatusCode::from_u16(200).expect("invalid status code")
        );
        assert_eq!(
            response
                .headers()
                .get(X_HTTPET_ANIMAL)
                .expect("missing header"),
            "dog"
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .expect("missing header"),
            "image/jpeg"
        );
        let body = read_body(response).await;
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn admin_update_toggles_enabled() {
        let state = setup_test_state().await;
        let db = state.app_state.db.clone();
        let enabled_list = state.app_state.enabled_pets.clone();
        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("POST")
            .uri("/admin/pets/otter")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("host", TEST_BASE_DOMAIN)
            .body(Body::from("enabled=on"))
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let pet = pets::Entity::find()
            .filter(pets::Column::Name.eq("otter"))
            .one(db.as_ref())
            .await
            .expect("fetch pet")
            .expect("pet exists");
        assert!(pet.enabled);

        let enabled = enabled_list.read().await;
        assert!(enabled.contains(&"otter".to_string()));
    }

    #[tokio::test]
    async fn admin_create_pet_adds_dog() {
        let state = setup_test_state().await;
        let db = state.app_state.db.clone();
        let app = create_router(&state.app_state).with_state(state.app_state.clone());

        let request = Request::builder()
            .method("POST")
            .uri("/admin/pets")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("host", TEST_BASE_DOMAIN)
            .body(Body::from("name=dog&enabled=on"))
            .expect("create request");
        let response = app.oneshot(request).await.expect("send request");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let pet = pets::Entity::find()
            .filter(pets::Column::Name.eq("dog"))
            .one(db.as_ref())
            .await
            .expect("fetch pet")
            .expect("pet exists");
        assert!(pet.enabled);
    }

    #[tokio::test]
    async fn migrations_apply_cleanly() {
        let db = crate::db::connect_test_db().await.expect("connect test db");
        crate::db::migrations::Migrator::up(db.as_ref(), None)
            .await
            .expect("run migrations");
        assert!(
            pets::Entity::find()
                .all(db.as_ref())
                .await
                .expect("query pets")
                .is_empty()
        );
        pets::ActiveModel {
            name: Set("pangolin".to_string()),
            enabled: Set(false),
            ..Default::default()
        }
        .insert(db.as_ref())
        .await
        .expect("insert pet");
    }
}
