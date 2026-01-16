use crate::packets::AnyPacket;
use crate::rate::AtomicRateLimiter;
use crate::utils::current_time_millis;
use anyhow::anyhow;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc;
use tracing::{error, info};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ConnectionAction {
    SendTCP(AnyPacket),
    SendUDP(AnyPacket),
    SendTCPRaw(Bytes),
    SendUDPRaw(Bytes),
    Close,
    RegisterUDP(SocketAddr),
}

#[derive(Debug)]
pub struct Room {
    pub id: String,
    pub host_connection_id: i32,
    pub password: Option<String>,
    pub created_at: u128,
    pub updated_at: u128,
    pub members: HashMap<i32, mpsc::Sender<ConnectionAction>>,
    pub stats: Stats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub players: Vec<Player>,
    #[serde(rename = "mapName")]
    pub map_name: String,
    pub name: String,
    pub gamemode: String,
    pub mods: Vec<String>,
    pub locale: String,
    pub version: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub name: String,
    pub locale: String,
}

pub struct Rooms {
    pub rooms: RwLock<HashMap<String, Room>>,
    tx: tokio::sync::broadcast::Sender<RoomUpdate>,
}

pub struct RoomInit {
    pub connection_id: i32,
    pub password: String,
    pub stats: Stats,
    pub sender: mpsc::Sender<ConnectionAction>,
}

impl Rooms {
    pub fn get_sender(
        &self,
        room_id: &str,
        connection_id: i32,
    ) -> Option<mpsc::Sender<ConnectionAction>> {
        let rooms = self.rooms.read().ok()?;

        rooms.get(room_id)?.members.get(&connection_id).cloned()
    }

    pub fn find_connection_room_id(&self, connection_id: i32) -> Option<String> {
        let rooms = self.rooms.read().ok()?;

        rooms
            .iter()
            .find(|(_, room)| room.members.contains_key(&connection_id))
            .map(|(id, _)| id.clone())
    }

    pub fn join(
        &self,
        connection_id: i32,
        room_id: &String,
        sender: mpsc::Sender<ConnectionAction>,
    ) -> anyhow::Result<()> {
        let mut rooms = self.rooms.write().map_err(|_| anyhow!("Lock poison"))?;

        if let Some(room) = rooms.get_mut(room_id) {
            room.members.insert(connection_id, sender);
            Ok(())
        } else {
            Err(anyhow!("Room not found"))
        }
    }

    pub fn leave(&self, connection_id: i32) -> Option<String> {
        let mut rooms = self.rooms.write().ok()?;
        let room_id = rooms
            .iter()
            .find(|(_, room)| room.members.contains_key(&connection_id))
            .map(|(id, _)| id.clone())?;

        if let Some(room) = rooms.get_mut(&room_id) {
            room.members.remove(&connection_id);
        }

        Some(room_id)
    }

    pub fn create(&self, init: RoomInit) -> String {
        let RoomInit {
            password,
            connection_id,
            stats,
            sender,
        } = init;

        let password = if password.is_empty() {
            None
        } else {
            Some(password)
        };

        let room_id = Uuid::now_v7().to_string();
        let mut members = HashMap::new();

        members.insert(connection_id, sender);

        let room = Room {
            id: room_id.clone(),
            host_connection_id: connection_id,
            password,
            stats,
            members,
            created_at: current_time_millis(),
            updated_at: current_time_millis(),
        };

        if let Ok(mut rooms) = self.rooms.write() {
            rooms.insert(room_id.clone(), room);
        }

        room_id
    }

    pub fn close(&self, room_id: &String) {
        let removed = {
            if let Ok(mut rooms) = self.rooms.write() {
                rooms.remove(room_id)
            } else {
                None
            }
        };

        if removed.is_some() {
            let _ = self.tx.send(RoomUpdate::Remove(room_id.clone()));
            info!("Room removed: {}", room_id);
        }
    }

    pub fn subscribe(&self) -> Receiver<RoomUpdate> {
        self.tx.subscribe()
    }

    pub fn broadcast(&self, room_id: &str, action: ConnectionAction, exclude_id: Option<i32>) {
        if let Ok(rooms) = self.rooms.read() {
            if let Some(room) = rooms.get(room_id) {
                for (id, sender) in &room.members {
                    if Some(*id) == exclude_id {
                        continue;
                    }
                    let _ = sender.try_send(action.clone());
                }
            }
        }
    }
}

pub struct AppState {
    pub rooms: Rooms,
    pub connections: RwLock<HashMap<i32, (mpsc::Sender<ConnectionAction>, Arc<AtomicRateLimiter>)>>,
    pub udp_routes:
        RwLock<HashMap<SocketAddr, (mpsc::Sender<ConnectionAction>, Arc<AtomicRateLimiter>)>>,
}

impl AppState {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(100);
        Self {
            rooms: Rooms {
                rooms: RwLock::new(HashMap::new()),
                tx,
            },
            connections: RwLock::new(HashMap::new()),
            udp_routes: RwLock::new(HashMap::new()),
        }
    }

    pub fn register_connection(
        &self,
        id: i32,
        sender: mpsc::Sender<ConnectionAction>,
        limiter: Arc<AtomicRateLimiter>,
    ) {
        match self.connections.write() {
            Ok(mut conns) => {
                conns.insert(id, (sender, limiter));
            }
            Err(err) => {
                error!("{}", err)
            }
        }
    }

    pub fn register_udp(
        &self,
        addr: SocketAddr,
        sender: mpsc::Sender<ConnectionAction>,
        limiter: Arc<AtomicRateLimiter>,
    ) {
        if let Ok(mut routes) = self.udp_routes.write() {
            routes.insert(addr, (sender, limiter));
        }
    }

    pub fn remove_udp(&self, addr: SocketAddr) {
        if let Ok(mut routes) = self.udp_routes.write() {
            routes.remove(&addr);
        }
    }

    pub fn get_sender(&self, id: i32) -> Option<mpsc::Sender<ConnectionAction>> {
        self.connections
            .read()
            .ok()?
            .get(&id)
            .map(|(s, _)| s.clone())
    }

    pub fn get_route(
        &self,
        addr: &SocketAddr,
    ) -> Option<(mpsc::Sender<ConnectionAction>, Arc<AtomicRateLimiter>)> {
        self.udp_routes.read().ok()?.get(addr).cloned()
    }

    pub fn remove_connection(&self, connection_id: i32) {
        if let Ok(mut conns) = self.connections.write() {
            conns.remove(&connection_id);
        }

        // Handle room logic
        let room_id_opt = self.rooms.leave(connection_id);
        if let Some(room_id) = room_id_opt {
            // Check if host
            let should_close = {
                if let Ok(rooms) = self.rooms.rooms.read() {
                    if let Some(room) = rooms.get(&room_id) {
                        room.host_connection_id == connection_id
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            if should_close {
                // Close room and disconnect all members
                let members = {
                    if let Ok(rooms) = self.rooms.rooms.read() {
                        if let Some(room) = rooms.get(&room_id) {
                            room.members.values().cloned().collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    }
                };

                for sender in members {
                    let _ = sender.try_send(ConnectionAction::Close);
                }

                self.rooms.close(&room_id);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum RoomUpdate {
    Update(RoomDisplay),
    Remove(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct RoomDisplay {
    pub id: String,
    pub host_connection_id: i32,
    pub password: bool,
    pub players: usize,
    pub max_players: usize,
    pub stats: Stats,
}
