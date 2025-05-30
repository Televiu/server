mod server;

use crate::server::state::State;

use std::{env, io::Error};

use tracing::{debug, info, level_filters::LevelFilter, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_SERVER_HOST: &str = "localhost";
const DEFAULT_SERVER_PORT: &str = "9000";

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_env_var("LOG")
                .with_default_directive(LevelFilter::INFO.into())
                .from_env()
                .unwrap(),
        )
        .json()
        .init();

    info!("televiu server started");

    let host = match env::var("TELEVIU_SERVER_HOST") {
        Ok(addr) => {
            debug!(value = addr, "TELEVIU_SERVER_HOST defined");

            addr
        }
        Err(_) => {
            warn!(
                value = DEFAULT_SERVER_HOST,
                "TELEVIU_SERVER_HOST not set, using default",
            );

            DEFAULT_SERVER_HOST.to_string()
        }
    };

    let port = match env::var("TELEVIU_SERVER_PORT") {
        Ok(addr) => {
            debug!(value = addr, "TELEVIU_SERVER_PORT defined");

            addr
        }
        Err(_) => {
            warn!(
                value = DEFAULT_SERVER_PORT,
                "TELEVIU_SERVER_PORT not set, using default",
            );

            DEFAULT_SERVER_PORT.to_string()
        }
    };

    let state = State::new();

    let router = server::router(state).await;

    return server::listen(router, server::Config { host, port }).await;
}
