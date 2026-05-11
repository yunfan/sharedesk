mod config;
mod room;
mod signaling;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::config::Config;
use crate::room::AppState;
use crate::signaling::{
    backend_root, ice_config, landing_page, room_info, room_page, ws_handler, SharedState,
};

#[tokio::main]
async fn main() -> Result<()> {
    let config = Arc::new(Config::from_env()?);

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            config.log.filter.clone(),
        ))
        .init();

    let static_dir = config.web.staticdir.clone();

    let state = SharedState {
        config: config.clone(),
        rooms: AppState::new(),
    };

    let app = Router::new()
        .route("/", get(landing_page))
        .route("/room/:room", get(room_page))
        .route(config.web.backendbase.as_str(), get(backend_root))
        .nest(
            config.web.backendbase.as_str(),
            Router::new()
                .nest(
                    "/api",
                    Router::new()
                        .route("/rooms/:room", get(room_info))
                        .route("/ice", get(ice_config)),
                )
                .route("/ws/:room/:role", get(ws_handler)),
        )
        .nest_service("/assets", ServeDir::new(static_dir))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = TcpListener::bind(config.server.listen).await?;
    info!("listening on http://{}", listener.local_addr()?);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
