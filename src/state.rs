use crate::models::{ArcCloseReason, Room, Stats};
use crate::packets::{AnyPacket, AppPacket, ConnectionClosedPacket};
use crate::rate::RateLimiter;
use crate::utils::current_time_millis;
use anyhow::anyhow;
use bytes::Bytes;
use dashmap::DashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::{self};
use tracing::info;
use uuid::Uuid;

pub struct Rooms {
    rooms: DashMap<String, Room>,
    connection_room: DashMap<i32, String>,
    // Channel to broadcast room updates to SSE (sender)
    tx: tokio::sync::broadcast::Sender<RoomUpdate>,
}
pub struct RoomInit {
    pub connection_id: i32,
    pub password: String,
    pub stats: Stats,
}

impl Rooms {
    pub fn get(
        &self,
        room_id: &String,
    ) -> std::option::Option<dashmap::mapref::one::Ref<'_, std::string::String, Room>> {
        return self.rooms.get(room_id);
    }

    pub fn find_connection_room(
        &self,
        connection_id: &i32,
    ) -> std::option::Option<dashmap::mapref::one::Ref<'_, std::string::String, Room>> {
        self.connection_room
            .get(connection_id)
            .and_then(|room_id| self.rooms.get(&room_id.clone()))
    }

    pub fn join(&self, connection_id: i32, room_id: &String) {
        self.connection_room.insert(connection_id, room_id.clone());
    }

    pub fn create(&self, init: RoomInit) -> String {
        let RoomInit {
            password,
            connection_id,
            stats,
        } = init;

        let password = if password.len() == 0 {
            None
        } else {
            Some(password)
        };

        let room_id = Uuid::now_v7().to_string();
        let room = Room {
            id: room_id.clone(),
            host_connection_id: connection_id,
            password,
            ping: 0,
            is_closed: false,
            stats,
            created_at: current_time_millis(),
            updated_at: current_time_millis(),
        };

        self.rooms.insert(room_id.clone(), room);

        return room_id;
    }

    pub fn close(&self, room_id: &String) {
        if let Some((_, _)) = self.rooms.remove(room_id) {
            let _ = self
                .tx
                .send(crate::state::RoomUpdate::Remove(room_id.clone()));

            info!("Room removed: {}", room_id);
        } else {
            info!("Room not exists: {}", room_id);
        }
    }

    pub async fn disconnect(
        &self,
        state: &AppState,
        connection_id: i32,
        reason: ArcCloseReason,
    ) -> anyhow::Result<()> {
        if let Some((_, room_id)) = self.connection_room.remove(&connection_id) {
            let is_host = self
                .rooms
                .get(&room_id)
                .map(|r| r.host_connection_id == connection_id)
                .unwrap_or(false);

            if is_host {
                self.close(&room_id);
            } else {
                let packet = AnyPacket::App(AppPacket::ConnectionClosed(ConnectionClosedPacket {
                    connection_id,
                    reason,
                }));

                state.send(packet, connection_id).await?;
            }
        }

        Ok(())
    }

    pub fn subscribe(&self) -> Receiver<RoomUpdate> {
        return self.tx.subscribe();
    }
}

pub struct AppState {
    pub rooms: Rooms,
    pub pending_connections: DashMap<i32, PendingConnectionState>,
    pub connections: DashMap<i32, ConnectionState>,
    // Maps connection ID to its queue of packets waiting for room join
    pub packet_queue: DashMap<i32, Vec<Bytes>>,
}

impl AppState {
    pub async fn disconnect(&self, connection_id: i32) -> anyhow::Result<()> {
        self.pending_connections.remove(&connection_id);
        self.connections.remove(&connection_id);
        self.packet_queue.remove(&connection_id);
        self.rooms
            .disconnect(&self, connection_id, ArcCloseReason::Closed)
            .await?;

        info!("Disconnected connection {}", connection_id);

        Ok(())
    }

    pub async fn send(&self, packet: AnyPacket, connection_id: i32) -> anyhow::Result<()> {
        let bytes = packet.bytes();
        let length = bytes.len();

        // info!("Sending packet: {:?} with len: {}", packet, length);

        if let Some(mut conn) = self.connections.get_mut(&connection_id) {
            conn.tx.send(bytes).await?;
            conn.last_write_time = Instant::now();
        } else if let Some(mut conn) = self.pending_connections.get_mut(&connection_id) {
            conn.tx.send(bytes).await?;
            conn.last_write_time = Instant::now();
        } else {
            return Err(anyhow!("No connection for id: {}", connection_id));
        }

        info!("Sent packet: {:?} with len: {}", packet, length);

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum RoomUpdate {
    Update(Room),
    Remove(String),
}

pub struct PendingConnectionState {
    pub id: i32,
    pub tx: mpsc::Sender<Bytes>, // To send data back to the TCP connection
    pub last_write_time: Instant,
    pub rate: RateLimiter,
    pub created_at: Instant,
}

pub struct ConnectionState {
    pub id: i32,
    pub tx: mpsc::Sender<Bytes>, // To send data back to the TCP connection
    pub udp_addr: SocketAddr,
    pub rate: RateLimiter,
    pub last_write_time: Instant,
    pub created_at: Instant,
}

impl AppState {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(100);
        Self {
            connections: DashMap::new(),
            pending_connections: DashMap::new(),
            packet_queue: DashMap::new(),
            rooms: Rooms {
                rooms: DashMap::new(),
                connection_room: DashMap::new(),
                tx: tx,
            },
        }
    }
}
