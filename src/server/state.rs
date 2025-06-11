use std::collections::HashMap;

use axum::extract::ws::Utf8Bytes;
use tokio::sync::{RwLock, mpsc};

pub struct Channel {
    pub sender: Option<mpsc::Sender<Utf8Bytes>>,
}

type Device = String;

pub struct State {
    pub channels: RwLock<HashMap<Device, RwLock<Channel>>>,
}

impl State {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
        }
    }
}
