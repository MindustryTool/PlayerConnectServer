use crate::config::Config;
use crate::constant::ArcCloseReason;
use crate::error::AppError;
use crate::models::{RoomView, Stats};
use crate::packet::{
    AnyPacket, AppPacket, ConnectionClosedPacket, ConnectionId, ConnectionIdlingPacket,
    ConnectionJoinPacket, RoomId,
};
use crate::rate::AtomicRateLimiter;
use crate::utils::current_time_millis;
use bytes::Bytes;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock, RwLockReadGuard};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ConnectionAction {
    SendTCP(AnyPacket),
    SendTCPRaw(Bytes),
    SendUDPRaw(Bytes),
    Close,
    RegisterUDP(SocketAddr),
    ProcessPacket(AnyPacket, bool),
}

#[derive(Debug, Clone)]
pub struct Room {
    pub id: RoomId,
    pub host_connection_id: ConnectionId,
    pub password: Option<String>,
    pub created_at: u128,
    pub updated_at: u128,
    pub members: HashMap<ConnectionId, mpsc::Sender<ConnectionAction>>,
    pub stats: Stats,
    pub ping: u128,
}

#[derive(Clone, Debug)]
pub enum RoomUpdate {
    Update { id: RoomId, data: Room },
    Remove(RoomId),
}

pub struct Rooms {
    pub rooms: RwLock<HashMap<RoomId, Room>>,
    pub broadcast_sender: tokio::sync::broadcast::Sender<RoomUpdate>,
    // Keep a receiver to prevent the channel from closing when all clients disconnect
    pub _broadcast_receiver: tokio::sync::broadcast::Receiver<RoomUpdate>,
}

pub struct RoomInit {
    pub connection_id: ConnectionId,
    pub password: String,
    pub stats: Stats,
    pub sender: mpsc::Sender<ConnectionAction>,
}

impl Rooms {
    pub fn get_sender(
        &self,
        room_id: &RoomId,
        connection_id: ConnectionId,
    ) -> Option<mpsc::Sender<ConnectionAction>> {
        let rooms = self.rooms.read().ok()?;

        rooms.get(room_id)?.members.get(&connection_id).cloned()
    }

    pub fn find_connection_room_id(&self, connection_id: ConnectionId) -> Option<RoomId> {
        let rooms = self.rooms.read().ok()?;

        rooms
            .iter()
            .find(|(_, room)| room.members.contains_key(&connection_id))
            .map(|(id, _)| id.clone())
    }

    pub fn read(&self) -> Option<RwLockReadGuard<'_, HashMap<RoomId, Room>>> {
        self.rooms.read().ok()
    }

    pub fn join(
        &self,
        connection_id: ConnectionId,
        room_id: &RoomId,
        sender: mpsc::Sender<ConnectionAction>,
    ) -> Result<(), AppError> {
        let mut rooms = self.rooms.write().map_err(|_| AppError::LockPoison)?;

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
            let packet = AnyPacket::App(AppPacket::ConnectionJoin(ConnectionJoinPacket {
                connection_id,
                room_id: room_id.clone(),
            }));

            if let Err(e) = sender.try_send(ConnectionAction::SendTCP(packet)) {
                info!(
                    "Failed to forward to host {}: {}",
                    room.host_connection_id, e
                );
            }
        } else {
            return Err(AppError::RoomNotFound(room_id.to_string()));
        }

        Ok(())
    }

    pub fn leave(&self, connection_id: ConnectionId) -> Option<RoomId> {
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

            let packet = AnyPacket::App(AppPacket::ConnectionClosed(ConnectionClosedPacket {
                connection_id,
                reason: ArcCloseReason::Closed,
            }));

            if let Err(e) = sender.try_send(ConnectionAction::SendTCP(packet)) {
                info!(
                    "Failed to forward to host {}: {}",
                    room.host_connection_id, e
                );
            }
        }

        Some(room_id)
    }

    pub fn create(&self, init: RoomInit) -> RoomId {
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

        let room_id = RoomId(Uuid::now_v7().to_string());
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

    pub fn close(&self, room_id: &RoomId) {
        let removed = {
            if let Ok(mut rooms) = self.rooms.write() {
                rooms.remove(room_id)
            } else {
                None
            }
        };

        if removed.is_some() {
            info!("Room closed {}", room_id);

            if let Err(err) = self
                .broadcast_sender
                .send(RoomUpdate::Remove(room_id.clone()))
            {
                error!("Failed to send remove room event: {}", err);
            };
        }
    }

    pub fn broadcast(
        &self,
        room_id: &RoomId,
        action: ConnectionAction,
        exclude_id: Option<ConnectionId>,
    ) {
        if let Ok(rooms) = self.rooms.read() {
            if let Some(room) = rooms.get(room_id) {
                for (id, sender) in &room.members {
                    if Some(*id) == exclude_id {
                        continue;
                    }
                    if let Err(e) = sender.try_send(action.clone()) {
                        warn!("Failed to broadcast to {}: {}", id, e);
                    }
                }
            }
        }
    }

    pub fn forward_to_host(&self, room_id: &RoomId, action: ConnectionAction) {
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
            warn!(
                "Failed to forward to host {}: {}",
                room.host_connection_id, e
            );
        }
    }

    pub fn get_room_members(
        &self,
        room_id: &RoomId,
    ) -> Vec<(ConnectionId, mpsc::Sender<ConnectionAction>)> {
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

    pub fn idle(&self, connection_id: ConnectionId) -> bool {
        let rooms = match self.rooms.read() {
            Ok(rooms) => rooms,
            Err(err) => {
                error!("Failed to acquire rooms read lock: {}", err);
                return true;
            }
        };

        if let Some(room) = rooms
            .iter()
            .find(|(_, room)| room.members.contains_key(&connection_id))
            .map(|(_, room)| room)
        {
            if room.host_connection_id == connection_id {
                return true;
            }

            if let Some(sender) = room.members.get(&room.host_connection_id) {
                let packet = AnyPacket::App(AppPacket::ConnectionIdling(ConnectionIdlingPacket {
                    connection_id,
                }));

                match sender.try_send(ConnectionAction::SendTCP(packet)) {
                    Ok(_) => return true,
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        warn!("Host channel full, retrying idle packet later");
                        return false;
                    }
                    Err(e) => {
                        warn!(
                            "Failed to forward idle packet to host {}: {}",
                            room.host_connection_id, e
                        );
                        return true;
                    }
                }
            }
        }
        true
    }
}

impl From<&Room> for RoomView {
    fn from(room: &Room) -> Self {
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

pub struct AppState {
    pub config: Config,
    pub rooms: Rooms,
    pub connections:
        RwLock<HashMap<ConnectionId, (mpsc::Sender<ConnectionAction>, Arc<AtomicRateLimiter>)>>,
    pub udp_routes:
        RwLock<HashMap<SocketAddr, (mpsc::Sender<ConnectionAction>, Arc<AtomicRateLimiter>)>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let (tx, rx) = tokio::sync::broadcast::channel(1024);
        Self {
            config,
            rooms: Rooms {
                rooms: RwLock::new(HashMap::new()),
                broadcast_sender: tx,
                _broadcast_receiver: rx,
            },
            connections: RwLock::new(HashMap::new()),
            udp_routes: RwLock::new(HashMap::new()),
        }
    }

    pub fn register_connection(
        &self,
        id: ConnectionId,
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

    pub fn has_connection_id(&self, id: ConnectionId) -> bool {
        self.connections
            .read()
            .ok()
            .map(|conns| conns.contains_key(&id))
            .unwrap_or(false)
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

    pub fn get_sender(&self, id: ConnectionId) -> Option<mpsc::Sender<ConnectionAction>> {
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

    pub fn idle(&self, connection_id: ConnectionId) -> bool {
        self.rooms.idle(connection_id)
    }

    pub fn remove_connection(&self, connection_id: ConnectionId) {
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
                    if let Err(e) = sender.try_send(ConnectionAction::Close) {
                        warn!("Failed to send close action to member: {}", e);
                    }
                }

                self.rooms.close(&room_id);
            }
        }
    }
}
