use serde::{Deserialize, Serialize};

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
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
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
