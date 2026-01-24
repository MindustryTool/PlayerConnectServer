use crate::error::AppError;
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::convert::TryFrom;

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
pub enum RoomCloseReason {
    Closed = 0,
    OutdatedVersion = 1,
    ServerClosed = 2,
}

impl TryFrom<u8> for RoomCloseReason {
    type Error = AppError;

    fn try_from(value: u8) -> Result<Self, AppError> {
        match value {
            0 => Ok(RoomCloseReason::Closed),
            1 => Ok(RoomCloseReason::OutdatedVersion),
            2 => Ok(RoomCloseReason::ServerClosed),
            _ => Err(AppError::PacketParsing(format!(
                "Invalid CloseReason: {}",
                value
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(u8)]
pub enum ConnectionCloseReason {
    Closed = 0,
    Timeout = 1,
    Error = 2,
    PacketSpam = 3,
}

impl TryFrom<u8> for ConnectionCloseReason {
    type Error = AppError;

    fn try_from(value: u8) -> Result<Self, AppError> {
        match value {
            0 => Ok(ConnectionCloseReason::Closed),
            1 => Ok(ConnectionCloseReason::Timeout),
            2 => Ok(ConnectionCloseReason::Error),
            3 => Ok(ConnectionCloseReason::PacketSpam),
            _ => Err(AppError::PacketParsing(format!(
                "Invalid ConnectionCloseReason: {}",
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
