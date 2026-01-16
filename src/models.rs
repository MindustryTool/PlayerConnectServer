use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

#[derive(Debug, Clone)]
pub struct Room {
    pub id: String,
    pub host_connection_id: i32, // Internal reference
    pub password: Option<String>,
    pub ping: i64,
    pub is_closed: bool,
    pub created_at: u128,
    pub updated_at: u128,
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

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
pub enum CloseReason {
    Closed = 0,
    ObsoleteClient = 1,
    OutdatedVersion = 2,
    ServerClosed = 3,
}

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
pub enum ArcCloseReason {
    Closed = 0,
    Timeout = 1,
    Error = 2,
}

impl ArcCloseReason {
    pub fn from(value: u8) -> Self {
        match value {
            0 => ArcCloseReason::Closed,
            1 => ArcCloseReason::Timeout,
            2 => ArcCloseReason::Error,
            _ => panic!("Invalid ArcCloseReason number"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
pub enum MessageType {
    ServerClosing = 0,
    PacketSpamming = 1,
    AlreadyHosting = 2,
    RoomClosureDenied = 3,
    ConClosureDenied = 4,
}
