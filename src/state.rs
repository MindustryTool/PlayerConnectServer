use crate::models::Room;
use bytes::Bytes;
use dashmap::DashMap;
use std::net::SocketAddr;
use std::time::Instant;
use tokio::sync::mpsc;

pub struct AppState {
    pub rooms: DashMap<String, Room>,
    pub connections: DashMap<i32, ConnectionState>,
    // Maps connection ID to its queue of packets waiting for room join
    pub packet_queue: DashMap<i32, Vec<Bytes>>,
    // Channel to broadcast room updates to SSE (sender)
    pub room_updates_tx: tokio::sync::broadcast::Sender<RoomUpdate>,
}

#[derive(Clone, Debug)]
pub enum RoomUpdate {
    Update(Room),
    Remove(String),
}

pub struct ConnectionState {
    pub id: i32,
    pub room_id: Option<String>,
    pub is_host: bool,
    pub tx: mpsc::Sender<Bytes>, // To send data back to the TCP connection
    pub udp_addr: Option<SocketAddr>,
    pub last_write_time: Instant,
}

impl AppState {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(100);
        Self {
            rooms: DashMap::new(),
            connections: DashMap::new(),
            packet_queue: DashMap::new(),
            room_updates_tx: tx,
        }
    }
}
