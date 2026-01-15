use crate::models::{CloseReason, MessageType, Stats as RoomStats};
use anyhow::{anyhow, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::{io::Cursor, time::Instant};
use tracing::info;

// TODO: Verify this ID.
pub const PACKET_GROUP_ID: i8 = -4;

pub trait Packet: Sized {
    fn id(&self) -> u8;
    fn read(buf: &mut Cursor<&[u8]>) -> Result<Self>;
    fn write(&self, buf: &mut BytesMut);
}

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

// Packet Struct Definitions with IDs
// Assuming IDs are sequential based on structure.md order:
// 0: ConnectionWrapperPacket (Abstract base, maybe not instantiated directly?)
// 1: ConnectionPacketWrapPacket
// 2: ConnectionClosedPacket
// 3: ConnectionJoinPacket
// 4: ConnectionIdlingPacket
// 5: RoomLinkPacket
// 6: RoomJoinPacket
// 7: RoomClosureRequestPacket
// 8: RoomClosedPacket
// 9: RoomCreationRequestPacket
// 10: MessagePacket
// 11: PopupPacket
// 12: Message2Packet
// 13: StatsPacket

#[derive(Debug)]
pub struct ConnectionWrapperPacket {
    pub connection_id: i32,
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
    pub reason: u8, // Using u8 ordinal directly, or could map to CloseReason if applicable?
                    // structure.md says "DcReason" which is an arc.net enum.
                    // Usually: 0=closed, 1=timeout, 2=error.
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
    pub data: RoomStats,
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
    pub data: RoomStats,
}

// Helper to read/write strings
fn read_string(buf: &mut Cursor<&[u8]>) -> Result<String> {
    if buf.remaining() < 2 {
        return Err(anyhow!("Not enough bytes for string length: {}", buf.remaining()));
    }
    
    let len = buf.get_u16() as usize;
    
    if buf.remaining() < len {
        return Err(anyhow!("Not enough bytes for string content, expected {}, got {}", len, buf.remaining()));
    }
    
    let mut bytes = vec![0u8; len];
    
    buf.copy_to_slice(&mut bytes);

    String::from_utf8(bytes).map_err(|e| anyhow!(e))
}

fn write_string(buf: &mut BytesMut, s: &str) {
    let bytes = s.as_bytes();
    buf.put_u16(bytes.len() as u16);
    buf.put_slice(bytes);
}

// Helper to read/write RoomStats (JSON serialization as string?)
// In Java NetworkRelay:
// room.stats = statsPacket.data;
// It seems it's serialized as an object.
// structure.md says "non-primitive (object)".
// In ArcNet, objects are serialized using generic serialization.
// Since we are rewriting in Rust, and we don't have the full Java serialization context,
// we might assume it's JSON string or specific binary format.
// However, looking at models.rs, Stats has strings and arrays.
// Let's assume for now it's serialized as a JSON string for simplicity in Rust implementation,
// OR we implement binary serialization matching the struct fields.
// Given "non-primitive (object)" usually means Kryo/Java serialization in Mindustry context,
// but for PlayerConnect it might be simpler.
// Let's assume binary serialization of fields in order.

fn read_stats(buf: &mut Cursor<&[u8]>) -> Result<RoomStats> {
    // This is a guess. We need to match Java side.
    // Stats { players, mapName, name, gamemode, mods, locale, version, createdAt }

    let json = read_string(buf)?;
    info!("Stats JSON: {}", json);

    // let data = serde_json::from_str(json.as_str());

    Ok(RoomStats{
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

fn write_stats(buf: &mut BytesMut, stats: &RoomStats) {
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

impl AnyPacket {
    pub fn read(mut buf: &mut Cursor<&[u8]>) -> Result<Self> {
        if !buf.has_remaining() {
            return Err(anyhow!("Empty packet"));
        }

        let id = buf.get_i8();

        if id == -2 {
            // Framework Message
            let fid = buf.get_u8();
            match fid {
                0 => Ok(AnyPacket::Framework(FrameworkMessage::Ping {
                    id: buf.get_i32(),
                    is_reply: buf.get_u8() == 1,
                })),
                1 => Ok(AnyPacket::Framework(FrameworkMessage::DiscoverHost)),
                2 => Ok(AnyPacket::Framework(FrameworkMessage::KeepAlive)),
                3 => Ok(AnyPacket::Framework(FrameworkMessage::RegisterUDP {
                    connection_id: buf.get_i32(),
                })),
                4 => Ok(AnyPacket::Framework(FrameworkMessage::RegisterTCP {
                    connection_id: buf.get_i32(),
                })),
                _ => Err(anyhow!("Unknown Framework ID: {}", fid)),
            }
        } else if id == PACKET_GROUP_ID {
            // App Packet
            let pid = buf.get_u8();

            match pid {
                0 => Ok(AnyPacket::App(AppPacket::ConnectionPacketWrap(
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
                ))),
                1 => Ok(AnyPacket::App(AppPacket::ConnectionClosed(
                    ConnectionClosedPacket {
                        connection_id: buf.get_i32(),
                        reason: buf.get_u8(), // DcReason ordinal
                    },
                ))),
                2 => Ok(AnyPacket::App(AppPacket::ConnectionJoin(
                    ConnectionJoinPacket {
                        connection_id: buf.get_i32(),
                        room_id: read_string(&mut buf)?,
                    },
                ))),
                3 => Ok(AnyPacket::App(AppPacket::ConnectionIdling(
                    ConnectionIdlingPacket {
                        connection_id: buf.get_i32(),
                    },
                ))),
                4 => Ok(AnyPacket::App(AppPacket::RoomCreationRequest(
                    RoomCreationRequestPacket {
                        version: read_string(&mut buf)?,
                        password: read_string(&mut buf)?,
                        data: read_stats(&mut buf)?,
                    },
                ))),
                5 => Ok(AnyPacket::App(AppPacket::RoomClosureRequest(
                    RoomClosureRequestPacket,
                ))),
                6 => Ok(AnyPacket::App(AppPacket::RoomClosed(RoomClosedPacket {
                    reason: unsafe { std::mem::transmute(buf.get_u8()) }, // Unsafe or impl TryFrom
                }))),
                7 => Ok(AnyPacket::App(AppPacket::RoomLink(RoomLinkPacket {
                    room_id: read_string(&mut buf)?,
                }))),
                8 => Ok(AnyPacket::App(AppPacket::RoomJoin(RoomJoinPacket {
                    room_id: read_string(&mut buf)?,
                    password: read_string(&mut buf)?,
                }))),
                9 => Ok(AnyPacket::App(AppPacket::Message(MessagePacket {
                    message: read_string(&mut buf)?,
                }))),
                10 => Ok(AnyPacket::App(AppPacket::Message2(Message2Packet {
                    message: unsafe { std::mem::transmute(buf.get_u8()) },
                }))),
                11 => Ok(AnyPacket::App(AppPacket::Popup(PopupPacket {
                    message: read_string(&mut buf)?,
                }))),
                12 => Ok(AnyPacket::App(AppPacket::Stats(StatsPacket {
                    room_id: read_string(&mut buf)?,
                    data: read_stats(&mut buf)?,
                }))),
                _ => Err(anyhow!("Unknown App Packet ID: {}", pid)),
            }
        } else {
            // Raw Packet
            buf.set_position(buf.position() - 1);
            let remaining = buf.remaining();
            let mut bytes = BytesMut::with_capacity(remaining);
            bytes.put(buf);
            Ok(AnyPacket::Raw(bytes.freeze()))
        }
    }

    pub fn write(&self, out: &mut BytesMut) {
        let mut payload = BytesMut::new();

        match self {
            AnyPacket::Framework(msg) => {
                payload.put_i8(-2);

                match msg {
                    FrameworkMessage::Ping { id, is_reply } => {
                        payload.put_u8(0);
                        payload.put_i32(*id);
                        payload.put_u8(if *is_reply { 1 } else { 0 });
                    }
                    FrameworkMessage::DiscoverHost => payload.put_u8(1),
                    FrameworkMessage::KeepAlive => payload.put_u8(2),
                    FrameworkMessage::RegisterUDP { connection_id } => {
                        payload.put_u8(3);
                        payload.put_i32(*connection_id);
                    }
                    FrameworkMessage::RegisterTCP { connection_id } => {
                        payload.put_u8(4);
                        payload.put_i32(*connection_id);
                    }
                }
            }
            AnyPacket::App(pkt) => {
                payload.put_i8(PACKET_GROUP_ID as i8);
                match pkt {
                    AppPacket::ConnectionPacketWrap(p) => {
                        payload.put_u8(0);
                        payload.put_i32(p.connection_id);
                        payload.put_u8(if p.is_tcp { 1 } else { 0 });
                        // Write payloadfer?
                        // payload.put_slice(&p.payloadfer);
                    }
                    AppPacket::ConnectionClosed(p) => {
                        payload.put_u8(1);
                        payload.put_i32(p.connection_id);
                        payload.put_u8(p.reason);
                    }
                    AppPacket::ConnectionJoin(p) => {
                        payload.put_u8(2);
                        payload.put_i32(p.connection_id);
                        write_string(&mut payload, &p.room_id);
                    }
                    AppPacket::ConnectionIdling(p) => {
                        payload.put_u8(3);
                        payload.put_i32(p.connection_id);
                    }
                    AppPacket::RoomCreationRequest(p) => {
                        payload.put_u8(4);
                        write_string(&mut payload, &p.version);
                        write_string(&mut payload, &p.password);
                        write_stats(&mut payload, &p.data);
                    }
                    AppPacket::RoomClosureRequest(_) => {
                        payload.put_u8(5);
                    }
                    AppPacket::RoomClosed(p) => {
                        payload.put_u8(6);
                        payload.put_u8(p.reason as u8);
                    }
                    AppPacket::RoomLink(p) => {
                        payload.put_u8(7);
                        write_string(&mut payload, &p.room_id);
                    }
                    AppPacket::RoomJoin(p) => {
                        payload.put_u8(8);
                        write_string(&mut payload, &p.room_id);
                        write_string(&mut payload, &p.password);
                    }
                    AppPacket::Message(p) => {
                        payload.put_u8(9);
                        write_string(&mut payload, &p.message);
                    }
                    AppPacket::Message2(p) => {
                        payload.put_u8(10);
                        payload.put_u8(p.message as u8);
                    }
                    AppPacket::Popup(p) => {
                        payload.put_u8(11);
                        write_string(&mut payload, &p.message);
                    }
                    AppPacket::Stats(p) => {
                        payload.put_u8(12);
                        write_string(&mut payload, &p.room_id);
                        write_stats(&mut payload, &p.data);
                    }
                }
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
