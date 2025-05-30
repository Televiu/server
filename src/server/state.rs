use std::{collections::HashMap, sync::Arc};

use axum::extract::ws::Utf8Bytes;
use tokio::sync::{RwLock, mpsc};

pub struct Channel {
    pub sender: Option<mpsc::Sender<Utf8Bytes>>,
}

type Device = String;

pub struct State {
    pub channels: RwLock<HashMap<Device, Arc<RwLock<Channel>>>>,
}

impl State {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
        }
    }
}
