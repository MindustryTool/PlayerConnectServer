use crate::error::AppError;
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::convert::TryFrom;

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
pub enum CloseReason {
    Closed = 0,
    ObsoleteClient = 1,
    OutdatedVersion = 2,
    ServerClosed = 3,
    PackingSpamming = 4,
}

impl TryFrom<u8> for CloseReason {
    type Error = AppError;

    fn try_from(value: u8) -> Result<Self, AppError> {
        match value {
            0 => Ok(CloseReason::Closed),
            1 => Ok(CloseReason::ObsoleteClient),
            2 => Ok(CloseReason::OutdatedVersion),
            3 => Ok(CloseReason::ServerClosed),
            4 => Ok(CloseReason::PackingSpamming),
            _ => Err(AppError::PacketParsing(format!(
                "Invalid CloseReason: {}",
                value
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
pub enum ArcCloseReason {
    Closed = 0,
    Timeout = 1,
    Error = 2,
}

impl TryFrom<u8> for ArcCloseReason {
    type Error = AppError;

    fn try_from(value: u8) -> Result<Self, AppError> {
        match value {
            0 => Ok(ArcCloseReason::Closed),
            1 => Ok(ArcCloseReason::Timeout),
            2 => Ok(ArcCloseReason::Error),
            _ => Err(AppError::PacketParsing(format!(
                "Invalid ArcCloseReason: {}",
                value
            ))),
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

impl TryFrom<u8> for MessageType {
    type Error = AppError;

    fn try_from(value: u8) -> Result<Self, AppError> {
        match value {
            0 => Ok(MessageType::ServerClosing),
            1 => Ok(MessageType::PacketSpamming),
            2 => Ok(MessageType::AlreadyHosting),
            3 => Ok(MessageType::RoomClosureDenied),
            4 => Ok(MessageType::ConClosureDenied),
            _ => Err(AppError::PacketParsing(format!(
                "Invalid MessageType: {}",
                value
            ))),
        }
    }
}
