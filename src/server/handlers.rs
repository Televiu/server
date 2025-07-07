use std::{collections::HashMap, sync::Arc};
use tokio::{
    select,
    sync::{RwLock, mpsc},
};

use serde::{Deserialize, Serialize};
use tracing::{Value, debug, error, info, trace, warn};

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

impl ToString for Event {
    fn to_string(&self) -> String {
        return format!(
            "command {:?} with payload of {:?}",
            self.command, self.payload
        );
    }
}

pub async fn player(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<State>>,
) -> impl IntoResponse {
    info!("player route called");

    ws.on_upgrade(move |socket| handle_player(socket, state))
}

async fn handle_player(mut socket: WebSocket, state: Arc<State>) {
    debug!("registering device");

    let device = uuid::Uuid::new_v4().to_string();
    let secret = "".to_string();

    info!(device = device, "device registered");

    let (sx, mut rx) = mpsc::channel(100);

    let mut channels = state.channels.write().await;
    channels.insert(device.clone(), RwLock::new(Channel { sender: Some(sx) }));
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
                                error!(error = e.to_string(), "websocket from player received an error");

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
                                error!(error = e.to_string(), "failed to parse event");
                                continue;
                            }
                        };

                        debug!(event = event.to_string(), "received event on player side");

                        match event.command {
                            Command::Pair => {
                                info!("player paired");

                                if let Err(_) = socket.send(Message::text(msg.clone())).await {
                                    error!("failed to send pair message");

                                    break;
                                };
                            }
                            Command::Play => {
                                info!("palyer played");

                                if let Err(_) = socket.send(Message::text(msg.clone())).await {
                                    error!("failed to send play message");

                                    break;
                                };
                            }
                            Command::Stop => {
                                info!("player stopped");

                                if let Err(_) = socket.send(Message::text(msg.clone())).await {
                                    error!("failed to send stop message");

                                    break;
                                };
                            }
                            Command::Unpair => {
                                info!("player unpaired");

                                if let Err(_) = socket.send(Message::text(msg.clone())).await {
                                    error!("failed to send unpair message");

                                    break;
                                };

                                break;
                            }
                        }

                    },
                    None => {
                        debug!("failed to receive message on websocket player loop");

                        break;
                    },
                };
            }
        };
    }

    trace!("player websocket loop exit");

    match socket.send(Message::Close(None)).await {
        Ok(_) => {
            debug!("websocket close message send from player to client");
        }
        Err(e) => {
            debug!(
                error = e.to_string(),
                "failed to close WebSocket connection from the player to client",
            );
        }
    }

    rx.close();

    trace!("trying to delete the devcie from channels");

    let mut channels = state.channels.write().await;
    channels.remove(&device);

    info!(device = device, "device unregistered");

    info!("webSocket connection closed on player side");
}

pub async fn controller(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<State>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    return ws.on_upgrade(move |socket| handle_controller(socket, state, params));
}

#[derive(Debug, Clone, Copy)]
enum ControllerState {
    Unpaired,
    Paired,
    Played,
    Stopped,
}

impl Default for ControllerState {
    fn default() -> Self {
        ControllerState::Unpaired
    }
}

impl ControllerState {
    fn pair(&mut self) -> bool {
        match *self {
            ControllerState::Unpaired => {
                *self = ControllerState::Paired;
                true
            }
            _ => {
                *self = ControllerState::Unpaired;
                false
            }
        }
    }

    fn play(&mut self) -> bool {
        match *self {
            ControllerState::Paired | ControllerState::Stopped => {
                *self = ControllerState::Played;
                true
            }
            _ => {
                *self = ControllerState::Unpaired;
                false
            }
        }
    }

    fn stop(&mut self) -> bool {
        match *self {
            ControllerState::Played => {
                *self = ControllerState::Stopped;
                true
            }
            _ => {
                *self = ControllerState::Unpaired;
                false
            }
        }
    }

    fn unpair(&mut self) -> bool {
        match *self {
            ControllerState::Paired | ControllerState::Played | ControllerState::Stopped => {
                *self = ControllerState::Unpaired;
                true
            }
            _ => {
                *self = ControllerState::Unpaired;
                false
            }
        }
    }
}

async fn handle_controller(
    mut socket: WebSocket,
    state: Arc<State>,
    params: HashMap<String, String>,
) {
    let device = match params.get("device") {
        Some(device) => device.clone(),
        None => {
            error!("no device found in params");

            return;
        }
    };

    let _secret = match params.get("secret") {
        Some(secret) => secret.clone(),
        None => {
            error!("no secret found in params");

            return;
        }
    };

    let channels = state.channels.read().await;
    let device = device;

    let channel = match channels.get(&device) {
        Some(tx) => tx,
        None => {
            error!("no channel found for device: {}", device);

            return;
        }
    };

    let mut lock = channel.write().await;
    let sender = match lock.sender.take() {
        Some(sender) => {
            info!("sender found for device: {}", device);

            sender
        }

        None => {
            error!("no sender found for device: {}", device);

            return;
        }
    };
    drop(lock);

    let mut controller_state = ControllerState::default();

    while let Some(Ok(msg)) = socket.recv().await {
        if sender.is_closed() {
            debug!("websocket of the screen is closed");

            break;
        }

        match msg {
            Message::Text(text) => {
                println!(
                    "received message: {:?}",
                    String::from_utf8_lossy(text.as_bytes())
                );

                let event: Event = match serde_json::from_str(&text) {
                    Ok(event) => event,
                    Err(e) => {
                        error!("failed to parse event: {}", e);
                        continue;
                    }
                };

                debug!("received event on controller side: {:?}", event);

                match event.command {
                    Command::Pair => {
                        if !controller_state.pair() {
                            error!("controller already paired");

                            let close = Utf8Bytes::from(
                                serde_json::to_string(&Event {
                                    command: Command::Unpair,
                                    payload: None,
                                })
                                .unwrap(),
                            );

                            if let Err(e) = sender.send(close).await {
                                error!("failed to send message from controller to player: {}", e);
                            }

                            break;
                        }

                        info!("controller paired");

                        sender.send(text).await.unwrap();
                    }
                    Command::Play => {
                        if !controller_state.play() {
                            error!("controller already playing");

                            let close = Utf8Bytes::from(
                                serde_json::to_string(&Event {
                                    command: Command::Unpair,
                                    payload: None,
                                })
                                .unwrap(),
                            );

                            if let Err(e) = sender.send(close).await {
                                error!("failed to send message from controller to player: {}", e);
                            }

                            break;
                        }

                        info!("playing file");

                        sender.send(text).await.unwrap();
                    }
                    Command::Stop => {
                        if !controller_state.stop() {
                            error!("controller not playing");

                            let close = Utf8Bytes::from(
                                serde_json::to_string(&Event {
                                    command: Command::Unpair,
                                    payload: None,
                                })
                                .unwrap(),
                            );

                            if let Err(e) = sender.send(close).await {
                                error!("failed to send message from controller to player: {}", e);
                            }

                            break;
                        }

                        info!("stopping file");

                        sender.send(text).await.unwrap();
                    }
                    Command::Unpair => {
                        if !controller_state.unpair() {
                            error!("controller already unpaired");

                            let close = Utf8Bytes::from(
                                serde_json::to_string(&Event {
                                    command: Command::Unpair,
                                    payload: None,
                                })
                                .unwrap(),
                            );

                            if let Err(e) = sender.send(close).await {
                                error!("failed to send message from controller to player: {}", e);
                            }
                        }

                        info!("controller unpaired");

                        sender.send(text).await.unwrap();

                        break;
                    }
                }
            }
            Message::Close(_) => {
                info!("websocket connection received a close message on controller side");

                if !controller_state.unpair() {
                    error!("device already playing");

                    let close = Utf8Bytes::from(
                        serde_json::to_string(&Event {
                            command: Command::Unpair,
                            payload: None,
                        })
                        .unwrap(),
                    );

                    if let Err(e) = sender.send(close).await {
                        error!("failed to send message from controller to player: {}", e);
                    }
                }

                match socket.send(Message::Close(None)).await {
                    Ok(_) => {
                        info!("websocket connection send close message on controller side");
                    }
                    Err(e) => {
                        error!("failed to close websocket connection: {}", e);
                    }
                }

                break;
            }
            _ => {}
        }
    }

    trace!("controller websocket loop exited");

    if let ControllerState::Unpaired = controller_state {
        debug!("controller status is unpaired as expected");
    } else {
        warn!("controller loop existed without being unpaired");
    }

    info!("websocket connection closed on controller side");
}
