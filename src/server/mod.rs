mod handlers;
pub mod state;

use std::{io::Error, sync::Arc};
use tokio::net::TcpListener;

use axum::{
    Router,
    extract::Extension,
    http::{self, HeaderName, HeaderValue},
    routing::get,
    serve,
};

use tower::ServiceBuilder;
use tower_http::{
    self, compression::CompressionLayer, cors::CorsLayer, limit::RequestBodyLimitLayer,
    propagate_header::PropagateHeaderLayer, trace::TraceLayer,
};

const REQUEST_BODY_LIMIT: usize = 16;

pub async fn router<S: Sync + Send + 'static>(state: S) -> Router {
    let state = Arc::new(state);

    let service = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT))
        .layer(PropagateHeaderLayer::new(HeaderName::from_static(
            "x-request-id",
        )))
        .layer(
            // TODO: Receive `origins` as a router configuration.
            CorsLayer::new()
                .allow_origin([HeaderValue::from_str("https://televiu.fly.dev").unwrap()])
                .allow_methods([http::Method::GET]),
        );

    // TODO: Remove magic routes.
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
    let addr = format!("{}:{}", config.host, config.port);

    let listener = TcpListener::bind(addr).await?;

    return serve(listener, router).await;
}
