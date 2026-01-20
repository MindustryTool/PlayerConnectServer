use crate::constant::{ArcCloseReason, CloseReason, MessageType};
use crate::models::Stats;
use crate::error::AppError;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::convert::TryFrom;
use std::io::Cursor;

pub const APP_PACKET_ID: i8 = -4;
pub const FRAMEWORK_PACKET_ID: i8 = -2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionId(pub i32);

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoomId(pub String);

impl std::fmt::Display for RoomId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub enum AnyPacket {
    Framework(FrameworkMessage),
    App(AppPacket),
    Raw(BytesMut),
}

#[derive(Debug, Clone, Copy)]
pub enum FrameworkMessage {
    Ping { id: i32, is_reply: bool },
    DiscoverHost,
    KeepAlive,
    RegisterUDP { connection_id: ConnectionId },
    RegisterTCP { connection_id: ConnectionId },
}

#[derive(Debug, Clone)]
pub enum AppPacket {
    ConnectionPacketWrap(ConnectionPacketWrapPacket),
    ConnectionClosed(ConnectionClosedPacket),
    ConnectionJoin(ConnectionJoinPacket),
    ConnectionIdling(ConnectionIdlingPacket),
    RoomLink(RoomLinkPacket),
    RoomJoin(RoomJoinPacket),
    RoomClosureRequest(RoomClosureRequestPacket),
    RoomClosed(RoomClosedPacket),
    RoomCreationRequest(RoomCreationRequestPacket),
    Message(MessagePacket),
    Popup(PopupPacket),
    Message2(Message2Packet),
    Stats(StatsPacket),
}

#[derive(Debug, Clone)]
pub struct ConnectionPacketWrapPacket {
    pub connection_id: ConnectionId,
    pub is_tcp: bool,
    pub buffer: BytesMut,
}

#[derive(Debug, Clone)]
pub struct ConnectionClosedPacket {
    pub connection_id: ConnectionId,
    pub reason: ArcCloseReason,
}

#[derive(Debug, Clone)]
pub struct ConnectionJoinPacket {
    pub connection_id: ConnectionId,
    pub room_id: RoomId,
}

#[derive(Debug, Clone)]
pub struct ConnectionIdlingPacket {
    pub connection_id: ConnectionId,
}

#[derive(Debug, Clone)]
pub struct RoomLinkPacket {
    pub room_id: RoomId,
}

#[derive(Debug, Clone)]
pub struct RoomJoinPacket {
    pub room_id: RoomId,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct RoomClosureRequestPacket;

#[derive(Debug, Clone)]
pub struct RoomClosedPacket {
    pub reason: CloseReason,
}

#[derive(Debug, Clone)]
pub struct RoomCreationRequestPacket {
    pub version: String,
    pub password: String,
    pub data: Stats,
}

#[derive(Debug, Clone)]
pub struct MessagePacket {
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct PopupPacket {
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Message2Packet {
    pub message: MessageType,
}

#[derive(Debug, Clone)]
pub struct StatsPacket {
    pub data: Stats,
}

impl AnyPacket {
    pub fn read(buf: &mut Cursor<Bytes>) -> Result<Self, AppError> {
        if !buf.has_remaining() {
            return Err(AppError::PacketParsing("Empty packet".to_string()));
        }

        let id = buf.get_i8();

        match id {
            FRAMEWORK_PACKET_ID => Ok(AnyPacket::Framework(FrameworkMessage::read(buf)?)),
            APP_PACKET_ID => Ok(AnyPacket::App(AppPacket::read(buf)?)),
            _ => {
                buf.set_position(buf.position() - 1);
                let remaining = buf.remaining();
                let start = buf.position() as usize;
                let end = start + remaining;

                let bytes = buf.get_ref().slice(start..end);
                buf.advance(remaining);

                Ok(AnyPacket::Raw(BytesMut::from(bytes)))
            }
        }
    }

    pub fn to_bytes(&self) -> BytesMut {
        let mut payload = BytesMut::new();

        match self {
            AnyPacket::Framework(package) => {
                package.write(&mut payload);
            }
            AnyPacket::App(package) => {
                package.write(&mut payload);
            }
            AnyPacket::Raw(raw) => {
                payload.extend_from_slice(raw);
            }
        }
        payload
    }
}

impl FrameworkMessage {
    pub fn read(buf: &mut Cursor<Bytes>) -> Result<Self, AppError> {
        let fid = buf.get_u8();

        match fid {
            0 => Ok(FrameworkMessage::Ping {
                id: buf.get_i32(),
                is_reply: buf.get_u8() != 0,
            }),
            1 => Ok(FrameworkMessage::DiscoverHost),
            2 => Ok(FrameworkMessage::KeepAlive),
            3 => Ok(FrameworkMessage::RegisterUDP {
                connection_id: ConnectionId(buf.get_i32()),
            }),
            4 => Ok(FrameworkMessage::RegisterTCP {
                connection_id: ConnectionId(buf.get_i32()),
            }),
            _ => Err(AppError::PacketParsing(format!(
                "Unknown Framework ID: {}",
                fid
            ))),
        }
    }

    pub fn write(&self, buf: &mut BytesMut) {
        buf.put_i8(FRAMEWORK_PACKET_ID);

        match self {
            FrameworkMessage::Ping { id, is_reply } => {
                buf.put_u8(0);
                buf.put_i32(*id);
                buf.put_u8(if *is_reply { 1 } else { 0 });
            }
            FrameworkMessage::DiscoverHost => buf.put_u8(1),
            FrameworkMessage::KeepAlive => buf.put_u8(2),
            FrameworkMessage::RegisterUDP { connection_id } => {
                buf.put_u8(3);
                buf.put_i32(connection_id.0);
            }
            FrameworkMessage::RegisterTCP { connection_id } => {
                buf.put_u8(4);
                buf.put_i32(connection_id.0);
            }
        }
    }
}

impl AppPacket {
    pub fn read(buf: &mut Cursor<Bytes>) -> Result<Self, AppError> {
        let pid = buf.get_u8();

        match pid {
            0 => {
                let connection_id = ConnectionId(buf.get_i32());
                let is_tcp = buf.get_u8() != 0;

                let start = buf.position() as usize;
                let end = start + buf.remaining();
                let buffer = buf.get_ref().slice(start..end);

                buf.set_position(end as u64);

                Ok(AppPacket::ConnectionPacketWrap(
                    ConnectionPacketWrapPacket {
                        connection_id,
                        is_tcp,
                        buffer: BytesMut::from(buffer),
                    },
                ))
            }
            1 => Ok(AppPacket::ConnectionClosed(ConnectionClosedPacket {
                connection_id: ConnectionId(buf.get_i32()),
                reason: ArcCloseReason::try_from(buf.get_u8())?,
            })),
            2 => Ok(AppPacket::ConnectionJoin(ConnectionJoinPacket {
                connection_id: ConnectionId(buf.get_i32()),
                room_id: RoomId(read_string(buf)?),
            })),
            3 => Ok(AppPacket::ConnectionIdling(ConnectionIdlingPacket {
                connection_id: ConnectionId(buf.get_i32()),
            })),
            4 => Ok(AppPacket::RoomCreationRequest(RoomCreationRequestPacket {
                version: read_string(buf)?,
                password: read_string(buf)?,
                data: read_stats(buf)?,
            })),
            5 => Ok(AppPacket::RoomClosureRequest(RoomClosureRequestPacket)),
            6 => Ok(AppPacket::RoomClosed(RoomClosedPacket {
                reason: CloseReason::try_from(buf.get_u8())?,
            })),
            7 => Ok(AppPacket::RoomLink(RoomLinkPacket {
                room_id: RoomId(read_string(buf)?),
            })),
            8 => Ok(AppPacket::RoomJoin(RoomJoinPacket {
                room_id: RoomId(read_string(buf)?),
                password: read_string(buf)?,
            })),
            9 => Ok(AppPacket::Message(MessagePacket {
                message: read_string(buf)?,
            })),
            10 => Ok(AppPacket::Message2(Message2Packet {
                message: MessageType::try_from(buf.get_u8())?,
            })),
            11 => Ok(AppPacket::Popup(PopupPacket {
                message: read_string(buf)?,
            })),
            12 => Ok(AppPacket::Stats(StatsPacket {
                data: read_stats(buf)?,
            })),
            _ => Err(AppError::PacketParsing(format!(
                "Unknown App Packet ID: {}",
                pid
            ))),
        }
    }

    pub fn write(&self, buf: &mut BytesMut) {
        buf.put_i8(APP_PACKET_ID as i8);

        match self {
            AppPacket::ConnectionPacketWrap(p) => {
                buf.put_u8(0);
                buf.put_i32(p.connection_id.0);
                buf.put_u8(if p.is_tcp { 1 } else { 0 });
                buf.extend_from_slice(&p.buffer);
            }
            AppPacket::ConnectionClosed(p) => {
                buf.put_u8(1);
                buf.put_i32(p.connection_id.0);
                buf.put_u8(p.reason as u8);
            }
            AppPacket::ConnectionJoin(p) => {
                buf.put_u8(2);
                buf.put_i32(p.connection_id.0);
                write_string(buf, &p.room_id.0);
            }
            AppPacket::ConnectionIdling(p) => {
                buf.put_u8(3);
                buf.put_i32(p.connection_id.0);
            }
            AppPacket::RoomCreationRequest(_) => {
                panic!("Client only")
            }
            AppPacket::RoomClosureRequest(_) => {
                buf.put_u8(5);
            }
            AppPacket::RoomClosed(p) => {
                buf.put_u8(6);
                buf.put_u8(p.reason as u8);
            }
            AppPacket::RoomLink(p) => {
                buf.put_u8(7);
                write_string(buf, &p.room_id.0);
            }
            AppPacket::RoomJoin(p) => {
                buf.put_u8(8);
                write_string(buf, &p.room_id.0);
                write_string(buf, &p.password);
            }
            AppPacket::Message(p) => {
                buf.put_u8(9);
                write_string(buf, &p.message);
            }
            AppPacket::Message2(p) => {
                buf.put_u8(10);
                buf.put_u8(p.message as u8);
            }
            AppPacket::Popup(p) => {
                buf.put_u8(11);
                write_string(buf, &p.message);
            }
            AppPacket::Stats(_) => {
                panic!("Client only")
            }
        }
    }
}

pub fn read_string(buf: &mut Cursor<Bytes>) -> Result<String, AppError> {
    if buf.remaining() < 2 {
        return Err(AppError::PacketParsing(format!(
            "Not enough bytes for string length: {}",
            buf.remaining()
        )));
    }

    let len = buf.get_u16() as usize;

    if buf.remaining() < len {
        return Err(AppError::PacketParsing(format!(
            "Not enough bytes for string content, expected {}, got {}",
            len,
            buf.remaining()
        )));
    }

    let mut bytes = vec![0u8; len];

    buf.copy_to_slice(&mut bytes);

    String::from_utf8(bytes).map_err(|e| AppError::PacketParsing(e.to_string()))
}

pub fn write_string(buf: &mut BytesMut, s: &str) {
    let bytes = s.as_bytes();
    buf.put_u16(bytes.len() as u16);
    buf.put_slice(bytes);
}

pub fn read_stats(buf: &mut Cursor<Bytes>) -> Result<Stats, AppError> {
    let json = read_string(buf)?;

    match serde_json::from_str::<Stats>(&json) {
        Ok(data) => Ok(data),
        Err(e) => {
            // Log? No, we return error.
            Err(AppError::PacketParsing(format!(
                "Failed to parse stats: {}. JSON: {}",
                e, json
            )))
        }
    }
}
