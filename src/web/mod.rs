use std::num::NonZeroU16;

use axum::Router;
use tracing::{error, info};

async fn root_handler() -> &'static str {
    "Hello, World!"
}

async fn get_status_handler(
    axum::extract::Path(status_code): axum::extract::Path<u16>,
) -> axum::response::Response {
    axum::response::Response::builder()
        .status(status_code)
        .body(axum::body::Body::empty())
        .unwrap()
}

pub fn create_router() -> Router {
    Router::new()
        .route("/", axum::routing::get(root_handler))
        .route("/{status_code}", axum::routing::get(get_status_handler))
}

pub async fn setup_server(listen_addr: &str, port: NonZeroU16) -> Result<(), anyhow::Error> {
    let app = create_router();

    let addr = format!("{}:{}", listen_addr, port);
    info!("Starting server on http://{}", addr);
    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    if let Err(err) = axum::serve(listener, app).await {
        error!("Server error: {}", err);
    }
    Ok(())
}
