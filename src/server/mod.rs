mod handlers;
pub mod state;

use std::{io::Error, sync::Arc};
use tokio::net::TcpListener;

use axum::{
    Router,
    extract::Extension,
    http::{self, HeaderName},
    routing::get,
    serve,
};

use tower::ServiceBuilder;
use tower_http::{
    self, compression::CompressionLayer, cors::CorsLayer, propagate_header::PropagateHeaderLayer,
    trace::TraceLayer,
};

pub async fn router<S: Sync + Send + 'static>(state: S) -> Router {
    let state = Arc::new(state);

    // TODO: Receive `origins` as a router configuration.
    let origins = ["https://televiu.fly.dev".parse().unwrap()];

    let service = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(PropagateHeaderLayer::new(HeaderName::from_static(
            "x-request-id",
        )))
        .layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([http::Method::GET]),
        );

    let router = Router::new()
        .route("/ws/controller", get(handlers::controller))
        .route("/ws/player", get(handlers::player))
        .layer(Extension(state))
        .layer(service);

    return router;
}

pub struct Config {
    pub host: String,
    pub port: String,
}

pub async fn listen(router: Router, config: Config) -> Result<(), Error> {
    let server_address = format!("{}:{}", config.host, config.port);

    // let origins = ["https://televiu.fly.dev".parse().unwrap()];

    let listener = TcpListener::bind(server_address).await?;

    return serve(listener, router).await;
}
