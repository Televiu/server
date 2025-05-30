use std::{collections::HashMap, sync::Arc};
use tokio::{
    select,
    sync::{RwLock, mpsc},
};

use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, trace};

use axum::{
    extract::{
        Extension, Query,
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};

use crate::server::state::{Channel, State};

/// Payload for the register and unregister a new player.
#[derive(Serialize, Deserialize)]
struct Registration {
    /// Device name.
    device: String,
    /// Device secret.
    secret: String,
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

pub async fn player(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<State>>,
) -> impl IntoResponse {
    info!("player route called");

    ws.on_upgrade(move |socket| handle_player(socket, state))
}

async fn handle_player(mut socket: WebSocket, state: Arc<State>) {
    debug!("Registering device");

    let device = uuid::Uuid::new_v4().to_string();
    let secret = "".to_string();

    info!("Device {} registered", device);

    let (sx, mut rx) = mpsc::channel(100);

    let mut channels = state.channels.write().await;
    channels.insert(
        device.clone(),
        Arc::new(RwLock::new(Channel { sender: Some(sx) })),
    );
    drop(channels);

    let registration = Registration {
        device: device.clone(),
        secret,
    };
    let msg = serde_json::to_string(&registration).unwrap();

    if let Err(_) = socket.send(Message::text(msg.clone())).await {
        error!("failed to send the registration message on websocket connection");

        return;
    };

    loop {
        select! {
            val = socket.recv() => {
                match val {
                    Some(result) => {
                        debug!("websocket from player received a message");

                        match result {
                            Err(e) => {
                                error!("websocket from player received an error: {}", e);

                                break;
                            },
                            _ => {}
                        }
                    },
                    None => {
                        debug!("websocket from player didn't received a message");

                        break;
                    },
                };
            }
            val = rx.recv() => {
                match val {
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
                                    error!("failed to send message");

                                    break;
                                };
                            }
                            Command::Play => {
                                info!("playing file on player side");

                                if let Err(_) = socket.send(Message::text(msg.clone())).await {
                                    error!("failed to send message");

                                    break;
                                };
                            }
                            Command::Stop => {
                                info!("stopping file on player side");

                                if let Err(_) = socket.send(Message::text(msg.clone())).await {
                                    error!("failed to send message");

                                    break;
                                };
                            }
                            Command::Unpair => {
                                info!("Device unpaired on player side");

                                if let Err(_) = socket.send(Message::text(msg.clone())).await {
                                    error!("failed to send message");

                                    break;
                                };

                                break;
                            }
                        }

                    },
                    None => {
                        debug!("Failed to receive message");

                        break;
                    },
                };
            }
        };
    }

    trace!("Closing WebSocket connection on player side");

    match socket.send(Message::Close(None)).await {
        Ok(_) => {
            info!("Websocket connection send close message on player side");
        }
        Err(e) => {
            debug!(
                "Failed to close WebSocket connection from the player: {}",
                e
            );
        }
    }

    rx.close();

    trace!("trying to delete the devcie from channels");

    let mut channels = state.channels.write().await;
    channels.remove(&device);

    info!("Device {} unregistered", device);

    info!("WebSocket connection closed on player side");
}

pub async fn controller(
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
        if sender.is_closed() {
            debug!("websocket of the screen is closed");

            break;
        }

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
                info!("Websocket connection received a close message on controller side");

                closed = true;

                break;
            }
            _ => {}
        }
    }

    trace!("Closing Websocket connection on controller side");

    if !closed {
        match socket.send(Message::Close(None)).await {
            Ok(_) => {
                info!("websocket connection send close message on controller side");
            }
            Err(e) => {
                error!("failed to close websocket connection: {}", e);
            }
        }
    }

    info!("websocket connection closed on controller side");
}
