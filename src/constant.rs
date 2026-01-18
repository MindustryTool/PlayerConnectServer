use serde_repr::{Deserialize_repr, Serialize_repr};

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
pub enum CloseReason {
    Closed = 0,
    ObsoleteClient = 1,
    OutdatedVersion = 2,
    ServerClosed = 3,
    PackingSpamming = 4,
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
