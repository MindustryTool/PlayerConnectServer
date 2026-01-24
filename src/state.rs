use crate::config::Config;
use crate::constant::{ConnectionCloseReason, RoomCloseReason};
use crate::error::AppError;
use crate::models::{RoomView, Stats};
use crate::packet::{
    AnyPacket, AppPacket, ConnectionClosedPacket, ConnectionId, ConnectionIdlingPacket,
    ConnectionJoinPacket, RoomClosedPacket, RoomId,
};
use crate::rate::AtomicRateLimiter;
use crate::utils::current_time_millis;
use bytes::Bytes;
use dashmap::DashMap;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ConnectionAction {
    SendTCP(AnyPacket),
    SendTCPRaw(Bytes),
    SendUDPRaw(Bytes),
    Close(ConnectionCloseReason),
    RegisterUDP(SocketAddr),
    ProcessPacket(AnyPacket, bool),
}

#[derive(Debug, Clone)]
pub struct Room {
    pub id: RoomId,
    pub host_connection_id: ConnectionId,
    pub host_sender: mpsc::Sender<ConnectionAction>,
    pub password: Option<String>,
    pub created_at: u128,
    pub updated_at: u128,
    pub members: HashMap<ConnectionId, mpsc::Sender<ConnectionAction>>,
    pub stats: Stats,
    pub ping: u128,
    pub protocol_version: String,
}

impl Room {
    pub fn is_host(&self, connection_id: ConnectionId) -> bool {
        self.host_connection_id == connection_id
    }
}

#[derive(Clone, Debug)]
pub enum RoomUpdate {
    Update { id: RoomId, data: Room },
    Remove(RoomId),
}

pub struct RoomState {
    pub rooms: DashMap<RoomId, Room>,
    pub broadcast_sender: tokio::sync::broadcast::Sender<RoomUpdate>,
    // Keep a receiver to prevent the channel from closing when all clients disconnect
    pub _broadcast_receiver: tokio::sync::broadcast::Receiver<RoomUpdate>,
}

pub struct RoomInit {
    pub connection_id: ConnectionId,
    pub password: String,
    pub stats: Stats,
    pub sender: mpsc::Sender<ConnectionAction>,
    pub protocol_version: String,
}

impl RoomState {
    pub fn get_sender(
        &self,
        room_id: &RoomId,
        connection_id: ConnectionId,
    ) -> Option<mpsc::Sender<ConnectionAction>> {
        self.rooms
            .get(room_id)?
            .members
            .get(&connection_id)
            .cloned()
    }

    pub fn join(
        &self,
        connection_id: ConnectionId,
        room_id: &RoomId,
        sender: mpsc::Sender<ConnectionAction>,
    ) -> Result<(), AppError> {
        if let Some(mut room) = self.rooms.get_mut(room_id) {
            room.members.insert(connection_id, sender);

            let packet = AnyPacket::App(AppPacket::ConnectionJoin(ConnectionJoinPacket {
                connection_id,
                room_id: room_id.clone(),
            }));

            if let Err(e) = room.host_sender.try_send(ConnectionAction::SendTCP(packet)) {
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

    pub fn leave(&self, connection_id: ConnectionId, room_id: &RoomId) {
        if let Some(mut room) = self.rooms.get_mut(room_id) {
            room.members.remove(&connection_id);

            let packet = AnyPacket::App(AppPacket::ConnectionClosed(ConnectionClosedPacket {
                connection_id,
                reason: ConnectionCloseReason::Closed,
            }));

            if let Err(e) = room.host_sender.try_send(ConnectionAction::SendTCP(packet)) {
                info!(
                    "Failed to forward to host {}: {}",
                    room.host_connection_id, e
                );
            }
        }
    }

    pub fn create(&self, init: RoomInit) -> RoomId {
        let RoomInit {
            password,
            connection_id,
            stats,
            sender,
            protocol_version,
        } = init;

        let password = if password.is_empty() {
            None
        } else {
            Some(password)
        };

        let room_id = RoomId(Uuid::now_v7().to_string());
        let members = HashMap::new();

        let room = Room {
            id: room_id.clone(),
            host_connection_id: connection_id,
            host_sender: sender,
            password,
            stats,
            members,
            created_at: current_time_millis(),
            updated_at: current_time_millis(),
            protocol_version,
            ping: 0,
        };

        self.rooms.insert(room_id.clone(), room);

        room_id
    }

    pub fn close(&self, room_id: &RoomId) {
        let removed = self.rooms.remove(room_id);

        if let Some((_, room)) = removed {
            info!("Room closed {}", room_id);

            for (id, sender) in room.members {
                if let Err(e) = sender.try_send(ConnectionAction::SendTCP(AnyPacket::App(
                    AppPacket::RoomClosed(RoomClosedPacket {
                        reason: RoomCloseReason::Closed,
                    }),
                ))) {
                    warn!("Failed to send room closed packet to {}: {}", id, e);
                }

                if let Err(e) =
                    sender.try_send(ConnectionAction::Close(ConnectionCloseReason::Closed))
                {
                    warn!("Failed to send close action to {}: {}", id, e);
                }
            }

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
        if let Some(room) = self.rooms.get(room_id) {
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

    pub fn forward_to_host(&self, room_id: &RoomId, action: ConnectionAction) {
        let room = match self.rooms.get(room_id) {
            Some(room) => room,
            None => {
                warn!("Room {} not found for forwarding", room_id);
                return;
            }
        };

        if let Err(e) = room.host_sender.try_send(action) {
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
        if let Some(room) = self.rooms.get(room_id) {
            room.members.iter().map(|(k, v)| (*k, v.clone())).collect()
        } else {
            warn!("Room {} not found for getting members", room_id);
            Vec::new()
        }
    }

    pub fn idle(&self, connection_id: ConnectionId, room_id: &RoomId) -> bool {
        if let Some(room) = self.rooms.get(room_id) {
            if room.is_host(connection_id) {
                return true;
            }

            let packet = AnyPacket::App(AppPacket::ConnectionIdling(ConnectionIdlingPacket {
                connection_id,
            }));

            match room.host_sender.try_send(ConnectionAction::SendTCP(packet)) {
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
        true
    }

    pub fn is_in_room(&self, connection_id: &ConnectionId, room_id: &RoomId) -> bool {
        self.rooms
            .get(&room_id)
            .map(|r| r.members.contains_key(&connection_id))
            .unwrap_or(false)
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
            protocol_version: room.protocol_version.clone(),
        }
    }
}

pub struct AppState {
    pub config: Config,
    pub room_state: RoomState,
    pub connections:
        DashMap<ConnectionId, (mpsc::Sender<ConnectionAction>, Arc<AtomicRateLimiter>)>,
    pub udp_routes: DashMap<SocketAddr, (mpsc::Sender<ConnectionAction>, Arc<AtomicRateLimiter>)>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let (tx, rx) = tokio::sync::broadcast::channel(1024);
        Self {
            config,
            room_state: RoomState {
                rooms: DashMap::new(),
                broadcast_sender: tx,
                _broadcast_receiver: rx,
            },
            connections: DashMap::new(),
            udp_routes: DashMap::new(),
        }
    }

    pub fn register_connection(
        &self,
        id: ConnectionId,
        sender: mpsc::Sender<ConnectionAction>,
        limiter: Arc<AtomicRateLimiter>,
    ) {
        self.connections.insert(id, (sender, limiter));
    }

    pub fn has_connection_id(&self, id: ConnectionId) -> bool {
        self.connections.contains_key(&id)
    }

    pub fn register_udp(
        &self,
        addr: SocketAddr,
        sender: mpsc::Sender<ConnectionAction>,
        limiter: Arc<AtomicRateLimiter>,
    ) {
        self.udp_routes.insert(addr, (sender, limiter));
    }

    pub fn remove_udp(&self, addr: SocketAddr) {
        self.udp_routes.remove(&addr);
    }

    pub fn get_sender(&self, id: ConnectionId) -> Option<mpsc::Sender<ConnectionAction>> {
        self.connections.get(&id).map(|val| val.0.clone())
    }

    pub fn get_route(
        &self,
        addr: &SocketAddr,
    ) -> Option<(mpsc::Sender<ConnectionAction>, Arc<AtomicRateLimiter>)> {
        self.udp_routes.get(addr).map(|val| val.clone())
    }

    pub fn idle(&self, connection_id: ConnectionId, room_id: &RoomId) -> bool {
        self.room_state.idle(connection_id, room_id)
    }

    pub fn remove_connection(&self, connection_id: ConnectionId, room_id: Option<RoomId>) {
        self.connections.remove(&connection_id);

        // Handle room logic
        if let Some(room_id) = room_id {
            self.room_state.leave(connection_id, &room_id);

            // Check if host
            let should_close = {
                if let Some(room) = self.room_state.rooms.get(&room_id) {
                    room.is_host(connection_id)
                } else {
                    false
                }
            };

            if should_close {
                self.room_state.close(&room_id);
            }
        }
    }
}
