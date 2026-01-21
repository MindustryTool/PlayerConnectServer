use crate::constant::{ArcCloseReason, CloseReason, MessageType};
use crate::packet::{
    AnyPacket, AppPacket, ConnectionClosedPacket, ConnectionId, ConnectionPacketWrapPacket,
    FrameworkMessage, Message2Packet, MessagePacket, RoomClosedPacket, RoomLinkPacket,
};
use crate::rate::AtomicRateLimiter;
use crate::state::{AppState, ConnectionAction, RoomInit, RoomUpdate};
use crate::utils::current_time_millis;
use crate::writer::{TcpWriter, UdpWriter};
use anyhow::anyhow;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

const TCP_BUFFER_SIZE: usize = 32768;
const CONNECTION_TIME_OUT_MS: Duration = Duration::from_millis(30000);
const KEEP_ALIVE_INTERVAL_MS: Duration = Duration::from_millis(5000);
const PACKET_LENGTH_LENGTH: usize = 2;
const TICK_INTERVAL_MS: u64 = 1000 / 30;

pub struct ConnectionActor {
    pub id: ConnectionId,
    pub state: Arc<AppState>,
    pub rx: mpsc::Receiver<ConnectionAction>,
    pub tcp_writer: TcpWriter,
    pub udp_writer: UdpWriter,
    pub limiter: Arc<AtomicRateLimiter>,
    pub last_read: Instant,
    pub packet_queue: Vec<Bytes>,
    pub notified_idle: bool,
}

impl ConnectionActor {
    pub async fn run(&mut self, mut reader: tokio::net::tcp::OwnedReadHalf) -> anyhow::Result<()> {
        let register_packet = AnyPacket::Framework(FrameworkMessage::RegisterTCP {
            connection_id: self.id,
        });

        self.write_packet(register_packet).await?;

        self.notify_idle();

        let mut buf = BytesMut::with_capacity(TCP_BUFFER_SIZE);
        let mut tmp_buf = [0u8; TCP_BUFFER_SIZE];
        let mut tick_interval = tokio::time::interval(Duration::from_millis(TICK_INTERVAL_MS));

        loop {
            let mut batch = BytesMut::new();

            tokio::select! {
                action = self.rx.recv() => {
                    if let Some(action) = action {
                        self.handle_action(action, &mut batch).await?;

                        while let Ok(action) = self.rx.try_recv() {
                            self.handle_action(action, &mut batch).await?;
                        }

                        if !batch.is_empty() {
                            self.tcp_writer.write(&batch).await?;
                        }
                    } else {
                        break;
                    }
                }

                read_result = reader.read(&mut tmp_buf) => {
                    match read_result {
                        Ok(0) => break,
                        Ok(n) => {
                            self.last_read = Instant::now();
                            self.notified_idle = false;

                            buf.extend_from_slice(&tmp_buf[..n]);
                            self.process_tcp_buffer(&mut buf).await?;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }

                _ = tick_interval.tick() => {
                    if self.is_idle() && !self.notified_idle {
                        self.notify_idle();
                    } else if self.tcp_writer.last_write.elapsed() > KEEP_ALIVE_INTERVAL_MS {
                         self.write_packet(AnyPacket::Framework(FrameworkMessage::KeepAlive)).await?;
                    }

                    if self.last_read.elapsed() > CONNECTION_TIME_OUT_MS {
                        info!("Connection {} timed out", self.id);
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn process_tcp_buffer(&mut self, buf: &mut BytesMut) -> anyhow::Result<()> {
        loop {
            if buf.len() < PACKET_LENGTH_LENGTH {
                break;
            }

            let len = {
                let mut cur = Cursor::new(&buf[..]);
                cur.get_u16() as usize
            };

            if buf.len() < PACKET_LENGTH_LENGTH + len {
                break;
            }

            buf.advance(PACKET_LENGTH_LENGTH);

            let payload = buf.split_to(len).freeze();
            let mut cursor = Cursor::new(payload);

            match AnyPacket::read(&mut cursor) {
                Ok(packet) => {
                    self.handle_packet(packet, true).await?;
                }
                Err(e) => {
                    error!("Error reading packet: {:?} from connection {}", e, self.id);
                    continue;
                }
            }
        }
        Ok(())
    }

    async fn handle_packet(&mut self, packet: AnyPacket, is_tcp: bool) -> anyhow::Result<()> {
        let is_framework = matches!(packet, AnyPacket::Framework(_));

        if !is_framework {
            let room_id_opt = self.state.rooms.find_connection_room_id(self.id);
            let is_host = if let Some(ref room_id) = room_id_opt {
                if let Some(rooms) = self.state.rooms.read() {
                    rooms
                        .get(room_id)
                        .map(|r| r.host_connection_id == self.id)
                        .unwrap_or(false)
                } else {
                    false
                }
            } else {
                false
            };

            if !is_host && !self.limiter.check() {
                if let Some(ref room_id) = room_id_opt {
                    self.state.rooms.broadcast(
                        room_id,
                        ConnectionAction::SendTCP(AnyPacket::App(AppPacket::Message2(
                            Message2Packet {
                                message: MessageType::PacketSpamming,
                            },
                        ))),
                        None,
                    );
                }

                self.write_packet(AnyPacket::App(AppPacket::ConnectionClosed(
                    ConnectionClosedPacket {
                        connection_id: self.id,
                        reason: ArcCloseReason::Closed,
                    },
                )))
                .await?;

                warn!("Connection {} disconnected for packet spamming.", self.id);
                return Err(anyhow!("Packet Spamming"));
            }
        }

        match packet {
            AnyPacket::Framework(f) => self.handle_framework(f).await?,
            AnyPacket::App(a) => self.handle_app(a).await?,
            AnyPacket::Raw(bytes) => {
                if let Some(room_id) = self.state.rooms.find_connection_room_id(self.id) {
                    let packet = AnyPacket::App(AppPacket::ConnectionPacketWrap(
                        ConnectionPacketWrapPacket {
                            connection_id: self.id,
                            is_tcp,
                            buffer: bytes,
                        },
                    ));

                    self.state
                        .rooms
                        .forward_to_host(&room_id, ConnectionAction::SendTCP(packet));
                } else {
                    if self.packet_queue.len() < 16 {
                        self.packet_queue.push(bytes);
                    } else {
                        warn!(
                            "Connection {} packet queue full, dropping raw packet",
                            self.id
                        );
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_framework(&mut self, packet: FrameworkMessage) -> anyhow::Result<()> {
        match packet {
            FrameworkMessage::Ping { id, is_reply } => {
                if !is_reply {
                    self.write_packet(AnyPacket::Framework(FrameworkMessage::Ping {
                        id,
                        is_reply: true,
                    }))
                    .await?;
                }
            }
            FrameworkMessage::KeepAlive => {}
            FrameworkMessage::RegisterUDP { .. } => {}
            _ => {
                warn!("Unhandled Framework Packet: {:?}", packet);
            }
        }
        Ok(())
    }

    async fn handle_app(&mut self, packet: AppPacket) -> anyhow::Result<()> {
        match packet {
            AppPacket::Stats(p) => {
                if let Some(room_id) = self.state.rooms.find_connection_room_id(self.id) {
                    if let Ok(mut rooms) = self.state.rooms.rooms.write() {
                        if let Some(room) = rooms.get_mut(&room_id) {
                            let sent_at = p.data.created_at;

                            room.stats = p.data;
                            room.updated_at = current_time_millis();
                            room.ping = current_time_millis() - sent_at;

                            if let Err(err) =
                                self.state.rooms.broadcast_sender.send(RoomUpdate::Update {
                                    id: room.id.clone(),
                                    data: room.clone(),
                                })
                            {
                                warn!("Fail to broadcast room update {}", err);
                            }
                        }
                    }
                }
            }
            AppPacket::RoomJoin(p) => {
                if let Some(current_room_id) = self.state.rooms.find_connection_room_id(self.id) {
                    let is_host = if let Some(rooms) = self.state.rooms.read() {
                        rooms
                            .get(&current_room_id)
                            .map(|r| r.host_connection_id == self.id)
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    if is_host {
                        self.write_packet(AnyPacket::App(AppPacket::Message2(Message2Packet {
                            message: MessageType::AlreadyHosting,
                        })))
                        .await?;

                        warn!(
                            "Connection {} tried to join room {} but is already hosting {}",
                            self.id, p.room_id, current_room_id
                        );
                        return Ok(());
                    }

                    info!(
                        "Connection {} left room {} to join {}",
                        self.id, current_room_id, p.room_id
                    );
                    self.state.rooms.leave(self.id);
                }

                let (can_join, wrong_password) = (|| {
                    let rooms = self.state.rooms.read()?;
                    let room = rooms.get(&p.room_id)?;

                    if let Some(ref pass) = room.password {
                        if pass != &p.password {
                            return Some((false, true));
                        }
                    }

                    Some((true, false))
                })()
                .unwrap_or((false, false));

                if wrong_password {
                    warn!(
                        "Connection {} tried to join room {} with wrong password.",
                        self.id, p.room_id
                    );
                    self.write_packet(AnyPacket::App(AppPacket::Message(MessagePacket {
                        message: "Wrong password".to_string(),
                    })))
                    .await?;
                    return Ok(());
                }

                if !can_join {
                    warn!(
                        "Connection {} tried to join a non-existent room {}.",
                        self.id, p.room_id
                    );
                    self.write_packet(AnyPacket::App(AppPacket::ConnectionClosed(
                        ConnectionClosedPacket {
                            connection_id: self.id,
                            reason: ArcCloseReason::Error,
                        },
                    )))
                    .await?;
                    return Ok(());
                }

                if let Some(sender) = self.state.get_sender(self.id) {
                    self.state.rooms.join(self.id, &p.room_id, sender)?;

                    info!("Connection {} joined the room {}.", self.id, p.room_id);

                    for bytes in self.packet_queue.drain(..) {
                        self.state.rooms.forward_to_host(
                            &p.room_id,
                            ConnectionAction::SendTCP(AnyPacket::App(
                                AppPacket::ConnectionPacketWrap(ConnectionPacketWrapPacket {
                                    connection_id: self.id,
                                    is_tcp: false,
                                    buffer: bytes,
                                }),
                            )),
                        );
                    }
                }
            }
            AppPacket::RoomCreationRequest(p) => {
                if let Some(current_room_id) = self.state.rooms.find_connection_room_id(self.id) {
                    self.write_packet(AnyPacket::App(AppPacket::Message2(Message2Packet {
                        message: MessageType::AlreadyHosting,
                    })))
                    .await?;
                    warn!(
                        "Connection {} tried to create a room but is already hosting/in the room {}.",
                        self.id, current_room_id
                    );
                    return Ok(());
                }

                if let Some(sender) = self.state.get_sender(self.id) {
                    let room_id = self.state.rooms.create(RoomInit {
                        connection_id: self.id,
                        password: p.password,
                        stats: p.data,
                        sender,
                    });
                    self.write_packet(AnyPacket::App(AppPacket::RoomLink(RoomLinkPacket {
                        room_id: room_id.clone(),
                    })))
                    .await?;

                    let Some(rooms) = self.state.rooms.read() else {
                        return Err(anyhow!("Can not read rooms"));
                    };

                    let Some(room) = rooms.get(&room_id) else {
                        return Err(anyhow!("Can not find room {}", room_id));
                    };

                    if let Err(err) = self.state.rooms.broadcast_sender.send(RoomUpdate::Update {
                        id: room.id.clone(),
                        data: room.clone(),
                    }) {
                        warn!("Fail to broadcast room update {}", err);
                    }

                    info!("Room {} created by connection {}.", room_id, self.id);
                }
            }
            AppPacket::RoomClosureRequest(_) => {
                if let Some(room_id) = self.state.rooms.find_connection_room_id(self.id) {
                    let is_host = if let Some(rooms) = self.state.rooms.read() {
                        rooms
                            .get(&room_id)
                            .map(|r| r.host_connection_id == self.id)
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    if !is_host {
                        self.write_packet(AnyPacket::App(AppPacket::Message2(Message2Packet {
                            message: MessageType::RoomClosureDenied,
                        })))
                        .await?;
                        warn!(
                            "Connection {} tried to close the room {} but is not the host.",
                            self.id, room_id
                        );
                        return Ok(());
                    }

                    let members = self.state.rooms.get_room_members(&room_id);
                    for (id, sender) in members {
                        if id != self.id {
                            if let Err(e) = sender.try_send(ConnectionAction::SendTCP(
                                AnyPacket::App(AppPacket::RoomClosed(RoomClosedPacket {
                                    reason: CloseReason::Closed,
                                })),
                            )) {
                                warn!("Failed to send room closed packet to {}: {}", id, e);
                            }
                            if let Err(e) = sender.try_send(ConnectionAction::Close) {
                                warn!("Failed to send close action to {}: {}", id, e);
                            }
                        }
                    }

                    self.state.rooms.close(&room_id);
                    info!(
                        "Room {} closed by connection {} (the host).",
                        room_id, self.id
                    );
                }
            }
            AppPacket::ConnectionClosed(p) => {
                if let Some(room_id) = self.state.rooms.find_connection_room_id(self.id) {
                    let is_host = if let Some(rooms) = self.state.rooms.read() {
                        rooms
                            .get(&room_id)
                            .map(|r| r.host_connection_id == self.id)
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    if !is_host {
                        self.write_packet(AnyPacket::App(AppPacket::Message2(Message2Packet {
                            message: MessageType::ConClosureDenied,
                        })))
                        .await?;
                        warn!("Connection {} tried to close the connection {} but is not the host of room {}.", self.id, p.connection_id, room_id);
                        return Ok(());
                    }

                    if let Some(sender) = self.state.get_sender(p.connection_id) {
                        let target_room = self.state.rooms.find_connection_room_id(p.connection_id);
                        if target_room.as_ref() == Some(&room_id) {
                            info!(
                                "Connection {} (room {}) closed the connection {}.",
                                self.id, room_id, p.connection_id
                            );

                            if let Err(e) =
                                sender.try_send(ConnectionAction::SendTCP(AnyPacket::App(
                                    AppPacket::ConnectionClosed(ConnectionClosedPacket {
                                        connection_id: p.connection_id,
                                        reason: p.reason,
                                    }),
                                )))
                            {
                                warn!(
                                    "Failed to send connection closed packet to {}: {}",
                                    p.connection_id, e
                                );
                            }
                            if let Err(e) = sender.try_send(ConnectionAction::Close) {
                                warn!("Failed to send close action to {}: {}", p.connection_id, e);
                            }
                        } else {
                            warn!("Connection {} (room {}) tried to close a connection from another room.", self.id, room_id);
                        }
                    }
                }
            }
            AppPacket::ConnectionPacketWrap(ConnectionPacketWrapPacket {
                connection_id,
                is_tcp,
                buffer,
            }) => {
                if let Some(room_id) = self.state.rooms.find_connection_room_id(self.id) {
                    if let Some(rooms) = self.state.rooms.read() {
                        let is_owner = rooms
                            .get(&room_id)
                            .map(|r| r.host_connection_id == self.id)
                            .unwrap_or(false);

                        if !is_owner {
                            return Err(anyhow!("Not room owner"));
                        }

                        let Some(sender) = self.state.get_sender(connection_id) else {
                            warn!("Connection not found: {}", connection_id);

                            self.state.rooms.forward_to_host(
                                &room_id,
                                ConnectionAction::SendTCP(AnyPacket::App(
                                    AppPacket::ConnectionClosed(ConnectionClosedPacket {
                                        connection_id,
                                        reason: ArcCloseReason::Closed,
                                    }),
                                )),
                            );

                            return Ok(());
                        };

                        let action = if is_tcp {
                            ConnectionAction::SendTCPRaw(buffer)
                        } else {
                            ConnectionAction::SendUDPRaw(buffer)
                        };

                        if let Err(e) = sender.try_send(action) {
                            warn!("Failed to forward packet to {}: {}", connection_id, e);
                        }
                    }
                } else {
                    warn!("No room found for connection {}", self.id);
                }
            }
            _ => {
                warn!("Unhandled App Packet: {:?}", packet);
            }
        }
        Ok(())
    }

    async fn handle_action(
        &mut self,
        action: ConnectionAction,
        batch: &mut BytesMut,
    ) -> anyhow::Result<()> {
        match action {
            ConnectionAction::SendTCP(p) => {
                let bytes = p.to_bytes();
                batch.extend_from_slice(&ConnectionActor::prepend_len(bytes.freeze()));
            }
            ConnectionAction::SendTCPRaw(b) => {
                self.notified_idle = false;
                batch.extend_from_slice(&ConnectionActor::prepend_len(b));
            }
            ConnectionAction::SendUDPRaw(b) => {
                self.notified_idle = false;
                self.udp_writer.send_raw(&b).await?;
            }
            ConnectionAction::Close => {
                return Err(anyhow::anyhow!("Closed"));
            }
            ConnectionAction::RegisterUDP(addr) => {
                self.notified_idle = false;

                if self.udp_writer.addr.is_some() {
                    return Ok(());
                }

                self.udp_writer.set_addr(addr);

                info!("New connection {} from {}", self.id, addr);

                if let Some(sender) = self.state.get_sender(self.id) {
                    self.state.register_udp(addr, sender, self.limiter.clone());
                } else {
                    return Err(anyhow::anyhow!(
                        "No sender found for connection {}",
                        self.id
                    ));
                }

                self.write_packet(AnyPacket::Framework(FrameworkMessage::RegisterUDP {
                    connection_id: self.id,
                }))
                .await?;
            }
            ConnectionAction::ProcessPacket(packet, is_tcp) => {
                self.notified_idle = false;
                self.handle_packet(packet, is_tcp).await?;
            }
        }
        Ok(())
    }

    pub fn prepend_len(payload: Bytes) -> BytesMut {
        let mut out: BytesMut = BytesMut::with_capacity(2 + payload.len());

        out.put_u16(payload.len() as u16);
        out.extend_from_slice(&payload);

        out
    }

    async fn write_packet(&mut self, packet: AnyPacket) -> anyhow::Result<()> {
        self.tcp_writer
            .write(&ConnectionActor::prepend_len(packet.to_bytes().freeze()))
            .await
    }

    fn is_idle(&self) -> bool {
        true
    }

    fn notify_idle(&mut self) {
        if self.state.idle(self.id) {
            self.notified_idle = true;
        }
    }
}
