use crate::constant::{ArcCloseReason, MessageType};
use crate::packet::{
    AnyPacket, AppPacket, ConnectionClosedPacket, ConnectionId, ConnectionPacketWrapPacket,
    FrameworkMessage, Message2Packet, MessagePacket, PopupPacket, RoomId, RoomLinkPacket,
};
use crate::rate::AtomicRateLimiter;
use crate::state::{AppState, ConnectionAction, RoomInit, RoomUpdate};
use crate::utils::current_time_millis;
use crate::writer::{TcpWriter, UdpWriter};
use anyhow::anyhow;
use bytes::{Buf, Bytes, BytesMut};
use std::io::{Cursor, IoSlice};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

const TCP_BUFFER_SIZE: usize = 32768;
const CONNECTION_TIME_OUT_MS: Duration = Duration::from_millis(30000);
const KEEP_ALIVE_INTERVAL_MS: Duration = Duration::from_millis(3000);
const PACKET_LENGTH_LENGTH: usize = 2;
const TICK_INTERVAL_MS: u64 = 1000 / 15;

pub struct ConnectionRoom {
    room_id: RoomId,
    is_host: bool,
}

impl ConnectionRoom {
    pub fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    pub fn is_host(&self) -> bool {
        self.is_host
    }
}

pub struct ConnectionActor {
    pub id: ConnectionId,
    pub state: Arc<AppState>,
    pub rx: mpsc::Receiver<ConnectionAction>,
    pub tcp_writer: TcpWriter,
    pub udp_writer: UdpWriter,
    pub limiter: Arc<AtomicRateLimiter>,
    pub last_read: Instant,
    pub packet_queue: Vec<Bytes>,
    pub room: Option<ConnectionRoom>,
}

impl ConnectionActor {
    pub async fn run(&mut self, mut reader: tokio::net::tcp::OwnedReadHalf) -> anyhow::Result<()> {
        let register_packet = AnyPacket::Framework(FrameworkMessage::RegisterTCP {
            connection_id: self.id,
        });

        self.write_packet(register_packet).await?;

        self.notify_idle();

        let mut buf = BytesMut::with_capacity(TCP_BUFFER_SIZE);
        let mut tick_interval = tokio::time::interval(Duration::from_millis(TICK_INTERVAL_MS));

        loop {
            let mut headers: Vec<[u8; 2]> = Vec::with_capacity(64);
            let mut payloads: Vec<Bytes> = Vec::with_capacity(64);

            tokio::select! {
                read_result = reader.read_buf(&mut buf) => {
                    match read_result {
                        Ok(0) => return Err(anyhow::anyhow!("Connection closed by peer")),
                        Ok(_n) => {
                            self.last_read = Instant::now();

                            self.process_tcp_buffer(&mut buf).await?;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }

                action = self.rx.recv() => {
                    if let Some(action) = action {
                        self.handle_action(action, &mut headers, &mut payloads).await?;

                        while let Ok(action) = self.rx.try_recv() {
                            self.handle_action(action, &mut headers, &mut payloads).await?;
                        }

                        if !payloads.is_empty() {
                            let mut slices = Vec::with_capacity(headers.len() * 2);
                            for (header, payload) in headers.iter().zip(payloads.iter()) {
                                slices.push(IoSlice::new(header));
                                slices.push(IoSlice::new(payload));
                            }
                            self.tcp_writer.write_vectored(&slices).await?;
                        }

                        if self.is_idle() && !self.tcp_writer.notified_idle {
                            self.notify_idle();
                        }

                    } else {
                        return Err(anyhow::anyhow!("Connection closed by peer, no action"));
                    }
                }

                _ = tick_interval.tick() => {
                    if self.tcp_writer.last_write.elapsed() > KEEP_ALIVE_INTERVAL_MS {
                         self.write_packet(AnyPacket::Framework(FrameworkMessage::KeepAlive)).await?;
                    }

                    if self.last_read.elapsed() > CONNECTION_TIME_OUT_MS {
                        info!("Connection {} timed out", self.id);
                        return Err(anyhow::anyhow!("Connection timed out"));
                    }
                }
            }
        }
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

            if !self.limiter.check() {
                warn!("Connection {} rate limit exceeded", self.id);
                return Err(anyhow::anyhow!("Rate limit exceeded"));
            }

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

                    let packet = AnyPacket::App(AppPacket::Popup(PopupPacket {
                        message: "Your version of MindustryTool is no longer supported. Please update to the latest version.".to_string(),
                    }));

                    self.write_packet(packet).await?;
                    continue;
                }
            }
        }
        Ok(())
    }

    async fn handle_packet(&mut self, packet: AnyPacket, is_tcp: bool) -> anyhow::Result<()> {
        let is_framework = matches!(packet, AnyPacket::Framework(_));

        if !is_framework {
            let is_host = self.room.as_ref().map(|r| r.is_host).unwrap_or(false);

            if !is_host && !self.limiter.check() {
                if let Some(ref room) = self.room {
                    self.state.room_state.broadcast(
                        &room.room_id,
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
                if let Some(ref room) = self.room {
                    let packet = AnyPacket::App(AppPacket::ConnectionPacketWrap(
                        ConnectionPacketWrapPacket {
                            connection_id: self.id,
                            is_tcp,
                            buffer: bytes,
                        },
                    ));

                    self.state
                        .room_state
                        .forward_to_host(&room.room_id, ConnectionAction::SendTCP(packet));
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
            FrameworkMessage::RegisterUDP { .. } => panic!("This should be handled previous"),
            _ => {
                warn!("Unhandled Framework Packet: {:?}", packet);
            }
        }
        Ok(())
    }

    async fn handle_app(&mut self, packet: AppPacket) -> anyhow::Result<()> {
        match packet {
            AppPacket::Stats(p) => {
                if let Some(ref room) = self.room {
                    if let Some(mut r) = self.state.room_state.rooms.get_mut(&room.room_id) {
                        if !room.is_host {
                            warn!(
                                "Connection {} tried to update stats but is not host",
                                self.id
                            );
                            return Ok(());
                        }

                        let sent_at = p.data.created_at;

                        r.stats = p.data;
                        r.updated_at = current_time_millis();
                        r.ping = current_time_millis() - sent_at;

                        if let Err(err) =
                            self.state
                                .room_state
                                .broadcast_sender
                                .send(RoomUpdate::Update {
                                    id: r.id.clone(),
                                    data: r.clone(),
                                })
                        {
                            warn!("Fail to broadcast room update {}", err);
                        }
                    }
                }
            }
            AppPacket::RoomJoin(p) => {
                if let Some((current_room_id, is_host)) =
                    self.room.as_ref().map(|r| (r.room_id.clone(), r.is_host))
                {
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
                    self.state.room_state.leave(self.id, &current_room_id);
                    self.room = None;
                }

                let (can_join, wrong_password) = (|| {
                    let room = self.state.room_state.rooms.get(&p.room_id)?;

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
                    self.write_packet(AnyPacket::App(AppPacket::Popup(PopupPacket {
                        message: "Room not found".to_string(),
                    })))
                    .await?;

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
                    self.state.room_state.join(self.id, &p.room_id, sender)?;
                    self.room = Some(ConnectionRoom {
                        room_id: p.room_id.clone(),
                        is_host: false,
                    });

                    info!("Connection {} joined the room {}.", self.id, p.room_id);

                    for bytes in self.packet_queue.drain(..) {
                        self.state.room_state.forward_to_host(
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
                if let Some(room_id) = self.room.as_ref().map(|r| r.room_id.clone()) {
                    self.write_packet(AnyPacket::App(AppPacket::Message2(Message2Packet {
                        message: MessageType::AlreadyHosting,
                    })))
                    .await?;
                    warn!(
                        "Connection {} tried to create a room but is already hosting/in the room {}.",
                        self.id, room_id
                    );
                    return Ok(());
                }

                if let Some(sender) = self.state.get_sender(self.id) {
                    let room_id = self.state.room_state.create(RoomInit {
                        connection_id: self.id,
                        password: p.password,
                        stats: p.data,
                        protocol_version: p.version,
                        sender,
                    });
                    self.room = Some(ConnectionRoom {
                        room_id: room_id.clone(),
                        is_host: true,
                    });

                    self.write_packet(AnyPacket::App(AppPacket::RoomLink(RoomLinkPacket {
                        room_id: room_id.clone(),
                    })))
                    .await?;

                    let Some(room) = self.state.room_state.rooms.get(&room_id) else {
                        return Err(anyhow!("Can not find room {}", room_id));
                    };

                    if let Err(err) =
                        self.state
                            .room_state
                            .broadcast_sender
                            .send(RoomUpdate::Update {
                                id: room.id.clone(),
                                data: room.clone(),
                            })
                    {
                        warn!("Fail to broadcast room update {}", err);
                    }

                    info!("Room {} created by connection {}.", room_id, self.id);
                }
            }
            AppPacket::RoomClosureRequest(_) => {
                if let Some((room_id, is_host)) =
                    self.room.as_ref().map(|r| (r.room_id.clone(), r.is_host))
                {
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

                    self.state.room_state.close(&room_id);
                    self.room = None;

                    info!(
                        "Room {} closed by connection {} (the host).",
                        room_id, self.id
                    );
                }
            }
            AppPacket::ConnectionClosed(p) => {
                if let Some((room_id, is_host)) =
                    self.room.as_ref().map(|r| (r.room_id.clone(), r.is_host))
                {
                    if !is_host {
                        self.write_packet(AnyPacket::App(AppPacket::Message2(Message2Packet {
                            message: MessageType::ConClosureDenied,
                        })))
                        .await?;
                        warn!("Connection {} tried to close the connection {} but is not the host of room {}.", self.id, p.connection_id, room_id);
                        return Ok(());
                    }

                    if let Some(sender) = self.state.get_sender(p.connection_id) {
                        let is_in_room = self
                            .state
                            .room_state
                            .rooms
                            .get(&room_id)
                            .map(|r| r.members.contains_key(&p.connection_id))
                            .unwrap_or(false);

                        if is_in_room {
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
                            warn!(
                                "Connection {} (room {}) tried to close a connection from another room.",
                                self.id, room_id
                            );
                        }
                    }
                }
            }
            AppPacket::ConnectionPacketWrap(ConnectionPacketWrapPacket {
                connection_id,
                is_tcp,
                buffer,
            }) => {
                if let Some((room_id, is_host)) =
                    self.room.as_ref().map(|r| (r.room_id.clone(), r.is_host))
                {
                    if !is_host {
                        return Err(anyhow!("Not room owner"));
                    }

                    let Some(sender) = self.state.get_sender(connection_id) else {
                        warn!("Connection not found: {}", connection_id);

                        self.state.room_state.forward_to_host(
                            &room_id,
                            ConnectionAction::SendTCP(AnyPacket::App(AppPacket::ConnectionClosed(
                                ConnectionClosedPacket {
                                    connection_id,
                                    reason: ArcCloseReason::Closed,
                                },
                            ))),
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
        headers: &mut Vec<[u8; 2]>,
        payloads: &mut Vec<Bytes>,
    ) -> anyhow::Result<()> {
        match action {
            ConnectionAction::SendTCP(p) => {
                let bytes = p.to_bytes().freeze();
                let len = bytes.len() as u16;
                headers.push(len.to_be_bytes());
                payloads.push(bytes);
            }
            ConnectionAction::SendTCPRaw(b) => {
                let len = b.len() as u16;
                headers.push(len.to_be_bytes());
                payloads.push(b);
            }
            ConnectionAction::SendUDPRaw(b) => {
                self.udp_writer.send_raw(&b).await?;
            }
            ConnectionAction::Close => {
                return Err(anyhow::anyhow!("Closed"));
            }
            ConnectionAction::RegisterUDP(addr) => {
                if let Some(addr) = self.udp_writer.addr {
                    self.state.remove_udp(addr);
                }

                if let Some(sender) = self.state.get_sender(self.id) {
                    self.udp_writer.set_addr(addr);
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

                info!("New connection {} from {}", self.id, addr);
            }
            ConnectionAction::ProcessPacket(packet, is_tcp) => {
                self.handle_packet(packet, is_tcp).await?;
            }
        }
        Ok(())
    }

    async fn write_packet(&mut self, packet: AnyPacket) -> anyhow::Result<()> {
        let bytes = packet.to_bytes().freeze();
        let len = (bytes.len() as u16).to_be_bytes();
        let slices = [IoSlice::new(&len), IoSlice::new(&bytes)];
        self.tcp_writer.write_vectored(&slices).await
    }

    fn is_idle(&self) -> bool {
        true
    }

    fn notify_idle(&mut self) {
        let Some(room) = &self.room else {
            return;
        };

        if self.state.idle(self.id, &room.room_id()) {
            self.tcp_writer.notified_idle = true;
        }
    }
}
