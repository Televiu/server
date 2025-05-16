use std::{collections::HashMap, env, io::Error, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{RwLock, mpsc},
};

use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, trace, warn};

use axum::{
    Json, Router,
    extract::{
        Extension, Query,
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    http::{self, HeaderName, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    serve,
};

use tower::ServiceBuilder;
use tower_http::{
    self, compression::CompressionLayer, cors::CorsLayer, propagate_header::PropagateHeaderLayer,
    trace::TraceLayer,
};

pub struct Channel {
    pub sender: Option<mpsc::Sender<Utf8Bytes>>,
    pub receiver: Option<mpsc::Receiver<Utf8Bytes>>,
}

type Device = String;

pub struct State {
    pub channels: RwLock<HashMap<Device, Arc<RwLock<Channel>>>>,
}

impl State {
    fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
        }
    }
}

/// Payload for the register and unregister a new player.
#[derive(Serialize, Deserialize)]
struct PlayerPayload {
    /// Device name.
    device: String,
    /// Device secret.
    secret: String,
}

/// Register a new device with the server.
///
/// This registration is used by the device to aquire the device's name and secret key, beyond the
/// creation of the communication channel between the controller and the player, which is done by
/// the WebSocket connection.
async fn register(Extension(state): Extension<Arc<State>>) -> impl IntoResponse {
    debug!("Registering device");

    let device = uuid::Uuid::new_v4().to_string();
    let secret = "".to_string();

    let (tx, rx) = mpsc::channel(100);

    {
        let mut channels = state.channels.write().await;
        channels.insert(
            device.clone(),
            Arc::new(RwLock::new(Channel {
                sender: Some(tx),
                receiver: Some(rx),
            })),
        );
    }

    info!("Device registered: {}", device);

    (StatusCode::CREATED, Json(PlayerPayload { device, secret }))
}

/// Unregister a device from the server.
///
/// This is used to remove the device from the server and close the communication channel between
/// the controller and the player.
async fn unregister(
    Extension(state): Extension<Arc<State>>,
    Json(payload): Json<PlayerPayload>,
) -> impl IntoResponse {
    // TODO: Remove the channel from the server, cancelling all WebSocket oponed connections.

    let device = payload.device.clone();
    let secret = payload.secret.clone();

    {
        let mut channels = state.channels.write().await;
        channels.remove(&device);
    }

    info!("Device unregistered: {}", device);

    StatusCode::OK
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Command {
    Pair,
    Unpair,
    Play,
    Stop,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Event {
    pub command: Command,
    pub payload: Option<String>,
}

async fn controller(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<State>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    return ws.on_upgrade(move |socket| handle_controller(socket, state, params));
}

async fn handle_controller(
    mut socket: WebSocket,
    state: Arc<State>,
    params: HashMap<String, String>,
) {
    let device = match params.get("device") {
        Some(device) => device.clone(),
        None => {
            error!("No device found in params");

            return;
        }
    };

    let _secret = match params.get("secret") {
        Some(secret) => secret.clone(),
        None => {
            error!("No secret found in params");

            return;
        }
    };

    let channels = state.channels.read().await;
    let device = device;

    let channel = match channels.get(&device) {
        Some(tx) => tx.clone(),
        None => {
            error!("No channel found for device: {}", device);
            return;
        }
    };

    drop(channels);

    let mut lock = channel.write().await;
    let sender = match lock.sender.take() {
        Some(s) => {
            info!("Sender found for device: {}", device);

            s
        }

        None => {
            error!("No sender found for device: {}", device);
            return;
        }
    };
    drop(lock);

    let mut paired = false;
    let mut playing = false;
    let mut closed = false;

    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                println!(
                    "Received message: {:?}",
                    String::from_utf8_lossy(text.as_bytes())
                );

                let event: Event = match serde_json::from_str(&text) {
                    Ok(event) => event,
                    Err(e) => {
                        error!("Failed to parse event: {}", e);
                        continue;
                    }
                };

                debug!("Received event on controller side: {:?}", event);

                match event.command {
                    Command::Pair => {
                        if paired {
                            error!("Device already paired");

                            let close = Utf8Bytes::from(
                                serde_json::to_string(&Event {
                                    command: Command::Unpair,
                                    payload: None,
                                })
                                .unwrap(),
                            );

                            sender.send(close).await.unwrap();

                            break;
                        }

                        info!("Device paired");

                        sender.send(text).await.unwrap();

                        paired = true;
                    }
                    Command::Play => {
                        if playing {
                            error!("Device already playing");

                            let close = Utf8Bytes::from(
                                serde_json::to_string(&Event {
                                    command: Command::Unpair,
                                    payload: None,
                                })
                                .unwrap(),
                            );

                            sender.send(close).await.unwrap();

                            break;
                        }

                        info!("playing file");

                        sender.send(text).await.unwrap();

                        playing = true;
                    }
                    Command::Stop => {
                        if !playing {
                            error!("Device not playing");

                            let close = Utf8Bytes::from(
                                serde_json::to_string(&Event {
                                    command: Command::Unpair,
                                    payload: None,
                                })
                                .unwrap(),
                            );

                            sender.send(close).await.unwrap();

                            break;
                        }

                        info!("stopping file");

                        sender.send(text).await.unwrap();

                        playing = false;
                    }
                    Command::Unpair => {
                        info!("Device unpaired");

                        sender.send(text).await.unwrap();

                        break;
                    }
                }
            }
            Message::Close(_) => {
                info!("WebSocket closed");

                closed = true;

                break;
            }
            _ => {}
        }
    }

    trace!("Closing WebSocket connection on controller side");

    if !closed {
        socket.send(Message::Close(None)).await.unwrap();
    }

    info!("WebSocket connection closed on controller side");
}

async fn player(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<State>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_player(socket, state, params))
}

async fn handle_player(mut socket: WebSocket, state: Arc<State>, params: HashMap<String, String>) {
    let device = match params.get("device") {
        Some(device) => device.clone(),
        None => {
            error!("No device found in params");

            return;
        }
    };

    let _secret = match params.get("secret") {
        Some(secret) => secret.clone(),
        None => {
            error!("No secret found in params");

            return;
        }
    };

    let channels = state.channels.read().await;
    let device = device;

    let channel = match channels.get(&device) {
        Some(tx) => tx.clone(),
        None => {
            error!("No channel found for device: {}", device);
            return;
        }
    };
    drop(channels);

    let mut lock = channel.write().await;
    let mut receiver = match lock.receiver.take() {
        Some(r) => {
            info!("Receiver found for device: {}", device);

            r
        }

        None => {
            error!("No receiver found for device: {}", device);

            return;
        }
    };
    drop(lock);

    while !receiver.is_closed() {
        match receiver.recv().await {
            Some(msg) => {
                let event: Event = match serde_json::from_str(&msg.to_string()) {
                    Ok(event) => event,
                    Err(e) => {
                        error!("Failed to parse event: {}", e);
                        continue;
                    }
                };

                debug!("Received event on player side: {:?}", event);

                match event.command {
                    Command::Pair => {
                        info!("Device paired on player side");

                        if let Err(_) = socket.send(Message::text(msg.clone())).await {
                            break;
                        };
                    }
                    Command::Play => {
                        info!("playing file on player side");

                        if let Err(_) = socket.send(Message::text(msg.clone())).await {
                            break;
                        };
                    }
                    Command::Stop => {
                        info!("stopping file on player side");

                        if let Err(_) = socket.send(Message::text(msg.clone())).await {
                            break;
                        };
                    }
                    Command::Unpair => {
                        info!("Device unpaired on player side");

                        if let Err(_) = socket.send(Message::text(msg.clone())).await {
                            break;
                        };

                        break;
                    }
                }
            }
            None => {
                debug!("Failed to receive message");

                break;
            }
        }
    }

    trace!("Closing WebSocket connection on player side");

    match socket.send(Message::Close(None)).await {
        Ok(_) => {
            info!("WebSocket connection closed on player side");
        }
        Err(e) => {
            error!("Failed to close WebSocket connection: {}", e);
        }
    }

    info!("WebSocket connection closed on player side");
}

const DEFAULT_SERVER_HOST: &str = "localhost";
const DEFAULT_SERVER_PORT: &str = "9000";
// const DEFAULT_UI_ADDRESS: &str = "localhost:8000";

#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::Builder::from_env("LOG").init();
    info!("televiu server started");

    let server_host = match env::var("TELEVIU_SERVER_HOST") {
        Ok(addr) => addr,
        Err(_) => {
            warn!(
                "TELEVIU_SERVER_HOST not set, using default: {}",
                DEFAULT_SERVER_HOST
            );

            DEFAULT_SERVER_HOST.to_string()
        }
    };

    let server_port = match env::var("TELEVIU_SERVER_PORT") {
        Ok(addr) => addr,
        Err(_) => {
            warn!(
                "TELEVIU_SERVER_PORT not set, using default: {}",
                DEFAULT_SERVER_PORT
            );

            DEFAULT_SERVER_PORT.to_string()
        }
    };

    let server_address = format!("{}:{}", server_host, server_port);

    // let ui_address = match env::var("TELEVIU_UI_ADDRESS") {
    //     Ok(addr) => addr,
    //     Err(_) => {
    //         warn!(
    //             "TELEVIU_UI_ADDRESS not set, using default: {}",
    //             DEFAULT_UI_ADDRESS
    //         );

    //         DEFAULT_UI_ADDRESS.to_string()
    //     }
    // };

    let state = Arc::new(State::new());

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
                .allow_methods([http::Method::GET, http::Method::POST]),
        );

    let router = Router::new()
        .route("/api/register", post(register))
        .route("/api/unregister", post(unregister))
        .route("/ws/controller", get(controller))
        .route("/ws/player", get(player))
        .layer(Extension(state))
        .layer(service);

    let listener = TcpListener::bind(server_address).await?;

    return serve(listener, router).await;
}
