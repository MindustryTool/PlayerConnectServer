use crate::constant::{ArcCloseReason, CloseReason};
use crate::packets::{AnyPacket, ConnectionClosedPacket, ConnectionJoinPacket};
use crate::rate::AtomicRateLimiter;
use crate::utils::current_time_millis;
use anyhow::anyhow;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock, RwLockReadGuard};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ConnectionAction {
    SendTCP(AnyPacket),
    SendUDP(AnyPacket),
    SendTCPRaw(Bytes),
    SendUDPRaw(Bytes),
    Close,
    RegisterUDP(SocketAddr),
    ProcessPacket(AnyPacket, bool),
}

#[derive(Debug, Clone)]
pub struct Room {
    pub id: String,
    pub host_connection_id: i32,
    pub password: Option<String>,
    pub created_at: u128,
    pub updated_at: u128,
    pub members: HashMap<i32, mpsc::Sender<ConnectionAction>>,
    pub stats: Stats,
    pub ping: u128,
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
    pub created_at: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub name: String,
    pub locale: String,
}

pub struct Rooms {
    pub rooms: RwLock<HashMap<String, Room>>,
    pub tx: tokio::sync::broadcast::Sender<RoomUpdate>,
    // Keep a receiver to prevent the channel from closing when all clients disconnect
    pub _rx: tokio::sync::broadcast::Receiver<RoomUpdate>,
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

    pub fn read(&self) -> Option<RwLockReadGuard<'_, HashMap<String, Room>>> {
        self.rooms.read().ok()
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

            let sender = match room.members.get(&room.host_connection_id) {
                Some(sender) => sender,
                None => {
                    error!(
                        "Host {} not found in room {} while joining",
                        room.host_connection_id, room_id
                    );
                    return Ok(());
                }
            };
            let packet = AnyPacket::App(crate::packets::AppPacket::ConnectionJoin(
                ConnectionJoinPacket {
                    connection_id,
                    room_id: room_id.clone(),
                },
            ));

            if let Err(e) = sender.try_send(ConnectionAction::SendTCP(packet)) {
                info!(
                    "Failed to forward to host {}: {}",
                    room.host_connection_id, e
                );
            }
        } else {
            return Err(anyhow!("Room not found"));
        }

        Ok(())
    }

    pub fn leave(&self, connection_id: i32) -> Option<String> {
        let mut rooms = self.rooms.write().ok()?;

        let room_id = rooms
            .iter()
            .find(|(_, room)| room.members.contains_key(&connection_id))
            .map(|(id, _)| id.clone())?;

        if let Some(room) = rooms.get_mut(&room_id) {
            room.members.remove(&connection_id);

            let sender = match room.members.get(&room.host_connection_id) {
                Some(sender) => sender,
                None => {
                    error!(
                        "Host {} not found in room {} while leaving",
                        room.host_connection_id, room_id
                    );
                    return Some(room_id);
                }
            };

            let packet = AnyPacket::App(crate::packets::AppPacket::ConnectionClosed(
                ConnectionClosedPacket {
                    connection_id,
                    reason: ArcCloseReason::Closed,
                },
            ));

            if let Err(e) = sender.try_send(ConnectionAction::SendTCP(packet)) {
                info!(
                    "Failed to forward to host {}: {}",
                    room.host_connection_id, e
                );
            }
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
            ping: 0,
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
            info!("Room closed {}", room_id);

            if let Err(err) = self.tx.send(RoomUpdate::Remove(room_id.clone())) {
                error!("Failed to send remove room event: {}", err);
            };
        }
    }

    pub fn broadcast(&self, room_id: &str, action: ConnectionAction, exclude_id: Option<i32>) {
        if let Ok(rooms) = self.rooms.read() {
            if let Some(room) = rooms.get(room_id) {
                for (id, sender) in &room.members {
                    if Some(*id) == exclude_id {
                        continue;
                    }
                    if let Err(e) = sender.try_send(action.clone()) {
                        info!("Failed to broadcast to {}: {}", id, e);
                    }
                }
            }
        }
    }

    pub fn forward_to_host(&self, room_id: &str, action: ConnectionAction) {
        let rooms = match self.rooms.read() {
            Ok(rooms) => rooms,
            Err(e) => {
                error!("Failed to acquire rooms read lock: {}", e);
                return;
            }
        };

        let room = match rooms.get(room_id) {
            Some(room) => room,
            None => {
                warn!("Room {} not found for forwarding", room_id);
                return;
            }
        };

        let sender = match room.members.get(&room.host_connection_id) {
            Some(sender) => sender,
            None => {
                error!(
                    "Host {} not found in room {} while forward",
                    room.host_connection_id, room_id
                );
                return;
            }
        };

        if let Err(e) = sender.try_send(action) {
            info!(
                "Failed to forward to host {}: {}",
                room.host_connection_id, e
            );
        }
    }

    pub fn get_room_members(&self, room_id: &str) -> Vec<(i32, mpsc::Sender<ConnectionAction>)> {
        let rooms = match self.rooms.read() {
            Ok(rooms) => rooms,
            Err(e) => {
                error!("Failed to acquire rooms read lock: {}", e);
                return Vec::new();
            }
        };

        if let Some(room) = rooms.get(room_id) {
            room.members.iter().map(|(k, v)| (*k, v.clone())).collect()
        } else {
            warn!("Room {} not found for getting members", room_id);
            Vec::new()
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
        let (tx, rx) = tokio::sync::broadcast::channel(1024);
        Self {
            rooms: Rooms {
                rooms: RwLock::new(HashMap::new()),
                tx,
                _rx: rx,
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
    Update { id: String, data: Room },
    Remove(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct RemoveRemoveEvent {
    #[serde(rename = "roomId")]
    pub room_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoomUpdateEvent {
    #[serde(rename = "roomId")]
    pub room_id: String,
    pub data: RoomView,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoomView {
    pub name: String,
    pub status: String,
    #[serde(rename = "isPrivate")]
    pub is_private: bool,
    #[serde(rename = "isSecured")]
    pub is_secured: bool,
    pub players: Vec<Player>,
    #[serde(rename = "mapName")]
    pub map_name: String,
    pub gamemode: String,
    pub mods: Vec<String>,
    pub locale: String,
    pub version: String,
    #[serde(rename = "createdAt")]
    pub created_at: u128,
    pub ping: u128,
}

impl RoomView {
    pub fn from(room: &Room) -> Self {
        Self {
            name: room.stats.name.clone(),
            status: "UP".to_string(),
            is_private: false,
            is_secured: room.password.is_some(),
            players: room.stats.players.clone(),
            map_name: room.stats.map_name.clone(),
            gamemode: room.stats.gamemode.clone(),
            mods: room.stats.mods.clone(),
            locale: room.stats.locale.clone(),
            version: room.stats.version.clone(),
            created_at: room.stats.created_at,
            ping: room.ping,
        }
    }
}
