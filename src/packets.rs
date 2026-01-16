use crate::models::{ArcCloseReason, CloseReason, MessageType, Stats};
use anyhow::{anyhow, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::io::Cursor;
use tracing::info;

pub const APP_PACKET_ID: i8 = -4;
pub const FRAMEWORK_PACKET_ID: i8 = -2;

#[derive(Debug)]
pub enum AnyPacket {
    Framework(FrameworkMessage),
    App(AppPacket),
    Raw(Bytes),
}

#[derive(Debug)]
pub enum FrameworkMessage {
    Ping { id: i32, is_reply: bool },
    DiscoverHost,
    KeepAlive,
    RegisterUDP { connection_id: i32 },
    RegisterTCP { connection_id: i32 },
}

#[derive(Debug)]
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

#[derive(Debug)]
pub struct ConnectionPacketWrapPacket {
    pub connection_id: i32,
    pub is_tcp: bool,
    pub buffer: Bytes,
}

#[derive(Debug)]
pub struct ConnectionClosedPacket {
    pub connection_id: i32,
    pub reason: ArcCloseReason,
}

#[derive(Debug)]
pub struct ConnectionJoinPacket {
    pub connection_id: i32,
    pub room_id: String,
}

#[derive(Debug)]
pub struct ConnectionIdlingPacket {
    pub connection_id: i32,
}

#[derive(Debug)]
pub struct RoomLinkPacket {
    pub room_id: String,
}

#[derive(Debug)]
pub struct RoomJoinPacket {
    pub room_id: String,
    pub password: String,
}

#[derive(Debug)]
pub struct RoomClosureRequestPacket;

#[derive(Debug)]
pub struct RoomClosedPacket {
    pub reason: CloseReason,
}

#[derive(Debug)]
pub struct RoomCreationRequestPacket {
    pub version: String,
    pub password: String,
    pub data: Stats,
}

#[derive(Debug)]
pub struct MessagePacket {
    pub message: String,
}

#[derive(Debug)]
pub struct PopupPacket {
    pub message: String,
}

#[derive(Debug)]
pub struct Message2Packet {
    pub message: MessageType,
}

#[derive(Debug)]
pub struct StatsPacket {
    pub room_id: String,
    pub data: Stats,
}

impl AnyPacket {
    pub fn read(buf: &mut Cursor<&[u8]>) -> Result<Self> {
        if !buf.has_remaining() {
            return Err(anyhow!("Empty packet"));
        }

        let id = buf.get_i8();

        match id {
            FRAMEWORK_PACKET_ID => Ok(AnyPacket::Framework(FrameworkMessage::read(buf)?)),
            APP_PACKET_ID => Ok(AnyPacket::App(AppPacket::read(buf)?)),
            _ => {
                buf.set_position(buf.position() - 1);
                let remaining = buf.remaining();
                let mut bytes = BytesMut::with_capacity(remaining);
                bytes.put(buf);

                Ok(AnyPacket::Raw(bytes.freeze()))
            }
        }
    }

    pub fn write(&self, out: &mut BytesMut) {
        let mut payload = BytesMut::new();

        match self {
            AnyPacket::Framework(package) => {
                package.write(&mut payload);
            }
            AnyPacket::App(package) => {
                package.write(&mut payload);
            }
            AnyPacket::Raw(bytes) => {
                payload.put_slice(bytes);
            }
        }

        info!("Sending packet: {:?} with len: {}", self, payload.len());

        out.put_u16(payload.len() as u16);
        out.extend_from_slice(&payload);
    }
}

impl FrameworkMessage {
    pub fn read(buf: &mut Cursor<&[u8]>) -> Result<Self> {
        let fid = buf.get_u8();

        match fid {
            0 => Ok(FrameworkMessage::Ping {
                id: buf.get_i32(),
                is_reply: buf.get_u8() == 1,
            }),
            1 => Ok(FrameworkMessage::DiscoverHost),
            2 => Ok(FrameworkMessage::KeepAlive),
            3 => Ok(FrameworkMessage::RegisterUDP {
                connection_id: buf.get_i32(),
            }),
            4 => Ok(FrameworkMessage::RegisterTCP {
                connection_id: buf.get_i32(),
            }),
            _ => Err(anyhow!("Unknown Framework ID: {}", fid)),
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
                buf.put_i32(*connection_id);
            }
            FrameworkMessage::RegisterTCP { connection_id } => {
                buf.put_u8(4);
                buf.put_i32(*connection_id);
            }
        }
    }
}


impl AppPacket {
    pub fn read(buf: &mut Cursor<&[u8]>) -> Result<Self> {
        let pid = buf.get_u8();

        match pid {
            0 => Ok(AppPacket::ConnectionPacketWrap(
                ConnectionPacketWrapPacket {
                    connection_id: buf.get_i32(),
                    is_tcp: buf.get_u8() == 1,
                    buffer: {
                        let len = buf.get_i32() as usize; // Buffer length?
                                                          // Actually java side: buffer is object.
                                                          // If it's just raw bytes, we need length.
                                                          // Let's assume remaining or prefixed length.
                                                          // Usually `write(buffer, object)` writes generic object.
                                                          // If it's ByteBuffer, it writes raw bytes?
                                                          // NetworkProxy: ((Packets.ConnectionPacketWrapPacket) packet).buffer = (ByteBuffer) ((ByteBuffer) last.get().clear()).put(buffer).flip();
                                                          // It seems it captures the rest of the buffer?
                        let mut b = BytesMut::new();
                        while buf.has_remaining() {
                            b.put_u8(buf.get_u8());
                        }
                        b.freeze()
                    },
                },
            )),
            1 => Ok(AppPacket::ConnectionClosed(ConnectionClosedPacket {
                connection_id: buf.get_i32(),
                reason: ArcCloseReason::from(buf.get_u8()), // DcReason ordinal
            })),
            2 => Ok(AppPacket::ConnectionJoin(ConnectionJoinPacket {
                connection_id: buf.get_i32(),
                room_id: read_string(buf)?,
            })),
            3 => Ok(AppPacket::ConnectionIdling(ConnectionIdlingPacket {
                connection_id: buf.get_i32(),
            })),
            4 => Ok(AppPacket::RoomCreationRequest(RoomCreationRequestPacket {
                version: read_string(buf)?,
                password: read_string(buf)?,
                data: read_stats(buf)?,
            })),
            5 => Ok(AppPacket::RoomClosureRequest(RoomClosureRequestPacket)),
            6 => Ok(AppPacket::RoomClosed(RoomClosedPacket {
                reason: unsafe { std::mem::transmute(buf.get_u8()) }, // Unsafe or impl TryFrom
            })),
            7 => Ok(AppPacket::RoomLink(RoomLinkPacket {
                room_id: read_string(buf)?,
            })),
            8 => Ok(AppPacket::RoomJoin(RoomJoinPacket {
                room_id: read_string(buf)?,
                password: read_string(buf)?,
            })),
            9 => Ok(AppPacket::Message(MessagePacket {
                message: read_string(buf)?,
            })),
            10 => Ok(AppPacket::Message2(Message2Packet {
                message: unsafe { std::mem::transmute(buf.get_u8()) },
            })),
            11 => Ok(AppPacket::Popup(PopupPacket {
                message: read_string(buf)?,
            })),
            12 => Ok(AppPacket::Stats(StatsPacket {
                room_id: read_string(buf)?,
                data: read_stats(buf)?,
            })),
            _ => Err(anyhow!("Unknown App Packet ID: {}", pid)),
        }
    }

    pub fn write(&self, buf: &mut BytesMut) {
        buf.put_i8(APP_PACKET_ID as i8);

        match self {
            AppPacket::ConnectionPacketWrap(p) => {
                buf.put_u8(0);
                buf.put_i32(p.connection_id);
                buf.put_u8(if p.is_tcp { 1 } else { 0 });
                // Write buffer?
                // buf.put_slice(&p.buffer);
            }
            AppPacket::ConnectionClosed(p) => {
                buf.put_u8(1);
                buf.put_i32(p.connection_id);
                buf.put_u8(p.reason as u8);
            }
            AppPacket::ConnectionJoin(p) => {
                buf.put_u8(2);
                buf.put_i32(p.connection_id);
                write_string(buf, &p.room_id);
            }
            AppPacket::ConnectionIdling(p) => {
                buf.put_u8(3);
                buf.put_i32(p.connection_id);
            }
            AppPacket::RoomCreationRequest(p) => {
                buf.put_u8(4);
                write_string(buf, &p.version);
                write_string(buf, &p.password);
                write_stats(buf, &p.data);
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
                write_string(buf, &p.room_id);
            }
            AppPacket::RoomJoin(p) => {
                buf.put_u8(8);
                write_string(buf, &p.room_id);
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
            AppPacket::Stats(p) => {
                buf.put_u8(12);
                write_string(buf, &p.room_id);
                write_stats(buf, &p.data);
            }
        }
    }
}


// Helper to read/write strings
pub fn read_string(buf: &mut Cursor<&[u8]>) -> Result<String> {
    if buf.remaining() < 2 {
        return Err(anyhow!(
            "Not enough bytes for string length: {}",
            buf.remaining()
        ));
    }

    let len = buf.get_u16() as usize;

    if buf.remaining() < len {
        return Err(anyhow!(
            "Not enough bytes for string content, expected {}, got {}",
            len,
            buf.remaining()
        ));
    }

    let mut bytes = vec![0u8; len];

    buf.copy_to_slice(&mut bytes);

    String::from_utf8(bytes).map_err(|e| anyhow!(e))
}

pub fn write_string(buf: &mut BytesMut, s: &str) {
    let bytes = s.as_bytes();
    buf.put_u16(bytes.len() as u16);
    buf.put_slice(bytes);
}

pub fn read_stats(buf: &mut Cursor<&[u8]>) -> Result<Stats> {
    let json = read_string(buf)?;
    info!("Stats JSON: {}", json);

    // let data = serde_json::from_str(json.as_str());

    Ok(Stats {
        players: vec![],
        map_name: String::new(),
        name: String::new(),
        gamemode: String::new(),
        mods: vec![],
        locale: String::new(),
        version: String::new(),
        created_at: 0,
    })
}

pub fn write_stats(buf: &mut BytesMut, stats: &Stats) {
    buf.put_u8(stats.players.len() as u8);
    for p in &stats.players {
        write_string(buf, &p.name);
        write_string(buf, &p.locale);
    }
    write_string(buf, &stats.map_name);
    write_string(buf, &stats.name);
    write_string(buf, &stats.gamemode);
    // write_string(buf, &stats.mods); // TODO
    write_string(buf, &stats.locale);
    write_string(buf, &stats.version);
    buf.put_i64(stats.created_at);
}
