use crate::constant::{ArcCloseReason, MessageType};
use crate::packets::{
    AnyPacket, AppPacket, ConnectionClosedPacket, ConnectionPacketWrapPacket, FrameworkMessage,
    Message2Packet, MessagePacket, RoomLinkPacket,
};
use crate::rate::AtomicRateLimiter;
use crate::state::{AppState, ConnectionAction, RoomInit, RoomUpdate};
use crate::utils::current_time_millis;
use anyhow::anyhow;
use bytes::{Buf, BytesMut};
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

static NEXT_CONNECTION_ID: AtomicI32 = AtomicI32::new(1);

const UDP_BUFFER_SIZE: usize = 4096;
const TCP_BUFFER_SIZE: usize = 32768;
const CHANNEL_CAPACITY: usize = 100;
const CONNECTION_TIME_OUT_MS: u64 = 10000;
const KEEP_ALIVE_INTERVAL_MS: u64 = 2000;
const PACKET_LENGTH_LENGTH: usize = 2;
const TICK_INTERVAL_SECS: u64 = 1;

const PACKET_RATE_LIMIT_WINDOW: Duration = Duration::from_millis(3000);
const PACKET_RATE_LIMIT: u32 = 300;

pub async fn run(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    let address = format!("0.0.0.0:{}", port);
    let tcp_listener = TcpListener::bind(&address).await?;
    let udp_socket = Arc::new(UdpSocket::bind(&address).await?);

    info!("Proxy Server listening on TCP/UDP {}", port);

    spawn_udp_listener(state.clone(), udp_socket.clone());
    accept_tcp_connection(state, tcp_listener, udp_socket).await
}

fn spawn_udp_listener(state: Arc<AppState>, socket: Arc<UdpSocket>) {
    tokio::spawn(async move {
        let mut buf = [0u8; UDP_BUFFER_SIZE];

        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let data = &buf[..len];
                    let mut cursor = Cursor::new(data);

                    match AnyPacket::read(&mut cursor) {
                        Ok(packet) => {
                            if let AnyPacket::Framework(FrameworkMessage::RegisterUDP {
                                connection_id,
                            }) = packet
                            {
                                // Handle Register UDP
                                handle_register_udp(&state, connection_id, addr).await;
                            } else {
                                // Normal packet, route it
                                if let Some((sender, limiter)) = state.get_route(&addr) {
                                    if limiter.check() {
                                        if let Err(e) =
                                            sender.try_send(ConnectionAction::SendTCP(packet))
                                        {
                                            info!("Failed to forward UDP packet: {}", e);
                                        }
                                    }
                                } else {
                                    // Unknown UDP sender, ignore
                                }
                            }
                        }
                        Err(e) => {
                            warn!("UDP Parse Error from {}: {}", addr, e);
                        }
                    }
                }
                Err(e) => error!("UDP Receive Error: {}", e),
            }
        }
    });
}

async fn handle_register_udp(state: &Arc<AppState>, connection_id: i32, addr: SocketAddr) {
    if let Some(sender) = state.get_sender(connection_id) {
        if let Err(e) = sender.try_send(ConnectionAction::RegisterUDP(addr)) {
            info!(
                "Failed to register UDP for connection {}: {}",
                connection_id, e
            );
        }
    }
}

async fn accept_tcp_connection(
    state: Arc<AppState>,
    listener: TcpListener,
    udp_socket: Arc<UdpSocket>,
) -> anyhow::Result<()> {
    loop {
        let (socket, _) = listener.accept().await?;

        let state = state.clone();
        let udp_socket = udp_socket.clone();

        tokio::spawn(async move {
            let _ = socket.set_nodelay(true);
            let id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);

            let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
            let limiter = Arc::new(AtomicRateLimiter::new(
                PACKET_RATE_LIMIT,
                PACKET_RATE_LIMIT_WINDOW,
            ));

            state.register_connection(id, tx, limiter.clone());

            let (reader, writer) = socket.into_split();

            let mut actor = ConnectionActor {
                id,
                state: state.clone(),
                rx,
                tcp_writer: TcpWriter::new(writer),
                udp_writer: UdpWriter::new(udp_socket),
                limiter,
                last_read: Instant::now(),
                packet_queue: Vec::new(),
            };

            if let Err(e) = actor.run(reader).await {
                error!("Connection {} error: {}", id, e);
            }

            state.remove_connection(id);

            if let Some(addr) = actor.udp_writer.addr {
                state.remove_udp(addr);
                info!("Connection {} closed", id);
            }
        });
    }
}

struct ConnectionActor {
    id: i32,
    state: Arc<AppState>,
    rx: mpsc::Receiver<ConnectionAction>,
    tcp_writer: TcpWriter,
    udp_writer: UdpWriter,
    limiter: Arc<AtomicRateLimiter>,
    last_read: Instant,
    packet_queue: Vec<AnyPacket>,
}

impl ConnectionActor {
    async fn run(&mut self, mut reader: tokio::net::tcp::OwnedReadHalf) -> anyhow::Result<()> {
        let register_packet = AnyPacket::Framework(FrameworkMessage::RegisterTCP {
            connection_id: self.id,
        });

        self.write_packet(register_packet).await?;

        let mut buf = BytesMut::with_capacity(TCP_BUFFER_SIZE);
        let mut tmp_buf = [0u8; TCP_BUFFER_SIZE];
        let mut tick_interval = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));

        loop {
            let mut batch = BytesMut::new();

            tokio::select! {
                // TCP Read
                read_result = reader.read(&mut tmp_buf) => {
                    match read_result {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            self.last_read = Instant::now();

                            buf.extend_from_slice(&tmp_buf[..n]);
                            self.process_tcp_buffer(&mut buf).await?;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }

                // Channel Read
                action = self.rx.recv() => {
                    if let Some(action) = action {
                        self.handle_action(action, &mut batch).await?;

                        while let Ok(action) = self.rx.try_recv() {
                            self.handle_action(action, &mut batch).await?;
                        }
                        // Flush batch
                        if !batch.is_empty() {
                            self.tcp_writer.write(&batch).await?;
                        }
                    } else {
                        // Channel closed
                        break;
                    }
                }

                // Tick
                _ = tick_interval.tick() => {
                    if self.last_read.elapsed() > Duration::from_millis(CONNECTION_TIME_OUT_MS) {
                        info!("Connection {} timed out", self.id);
                        break;
                    }

                    if self.tcp_writer.last_write.elapsed() > Duration::from_millis(KEEP_ALIVE_INTERVAL_MS) {
                         self.write_packet(AnyPacket::Framework(FrameworkMessage::KeepAlive)).await?;
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
            let payload = buf.split_to(len);
            let mut cursor = Cursor::new(&payload[..]);

            let packet = AnyPacket::read(&mut cursor)?;
            // Handle packet
            self.handle_packet(packet).await?;
        }
        Ok(())
    }

    async fn handle_packet(&mut self, packet: AnyPacket) -> anyhow::Result<()> {
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
                    self.state.rooms.broadcast(
                        &room_id,
                        ConnectionAction::SendTCPRaw(bytes),
                        Some(self.id),
                    );
                } else {
                    if self.packet_queue.len() < 16 {
                        self.packet_queue.push(AnyPacket::Raw(bytes));
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
            FrameworkMessage::KeepAlive => {
                // Handled by activity update
            }
            FrameworkMessage::RegisterUDP { .. } => {
                // Should not happen via TCP?
                // But if it does, ignore?
            }
            _ => {}
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

                            if let Err(err) = self.state.rooms.tx.send(RoomUpdate::Update {
                                id: room.id.clone(),
                                data: room.clone(),
                            }) {
                                info!("Fail to broadcast room update {}", err);
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
                    info!(
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
                    info!(
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

                    for pkt in self.packet_queue.drain(..) {
                        self.state.rooms.broadcast(
                            &p.room_id,
                            match pkt {
                                AnyPacket::Raw(b) => ConnectionAction::SendTCPRaw(b),
                                _ => ConnectionAction::SendTCP(pkt),
                            },
                            Some(self.id),
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
                                info!(
                                    "Failed to send connection closed packet to {}: {}",
                                    p.connection_id, e
                                );
                            }
                            if let Err(e) = sender.try_send(ConnectionAction::Close) {
                                info!("Failed to send close action to {}: {}", p.connection_id, e);
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

                        let action = if is_tcp {
                            ConnectionAction::SendTCPRaw(buffer)
                        } else {
                            ConnectionAction::SendUDPRaw(buffer)
                        };

                        let Some(sender) = self.state.get_sender(connection_id) else {
                            return Err(anyhow!("Connection not found: {}", connection_id));
                        };

                        if let Err(e) = sender.try_send(action) {
                            warn!("Failed to forward packet to {}: {}", connection_id, e);
                        }
                    }
                } else {
                    info!("No room found for connection {}", self.id);
                }
            }
            _ => {
                info!("Unhandled {:?}", packet);
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
                batch.extend_from_slice(&bytes);
            }
            ConnectionAction::SendUDP(p) => {
                self.udp_writer.send(p).await?;
            }
            ConnectionAction::SendTCPRaw(b) => {
                batch.extend_from_slice(&b);
            }
            ConnectionAction::SendUDPRaw(b) => {
                self.udp_writer.send_raw(&b).await?;
            }
            ConnectionAction::Close => {
                // Return error to break loop
                return Err(anyhow::anyhow!("Closed"));
            }
            ConnectionAction::RegisterUDP(addr) => {
                if self.udp_writer.addr.is_some() {
                    return Ok(());
                }

                self.udp_writer.set_addr(addr);

                info!("New connection {} from {}", self.id, addr);

                // Register in state
                if let Some(sender) = self.state.get_sender(self.id) {
                    self.state.register_udp(addr, sender, self.limiter.clone());
                } else {
                    return Err(anyhow::anyhow!(
                        "No sender found for connection {}",
                        self.id
                    ));
                }
                // Send reply
                self.write_packet(AnyPacket::Framework(FrameworkMessage::RegisterUDP {
                    connection_id: self.id,
                }))
                .await?;
            }
        }
        Ok(())
    }

    async fn write_packet(&mut self, packet: AnyPacket) -> anyhow::Result<()> {
        self.tcp_writer.write_packet(packet).await
    }
}

struct TcpWriter {
    writer: tokio::net::tcp::OwnedWriteHalf,
    last_write: Instant,
}

impl TcpWriter {
    fn new(writer: tokio::net::tcp::OwnedWriteHalf) -> Self {
        Self {
            writer,
            last_write: Instant::now(),
        }
    }

    async fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(data).await?;
        self.last_write = Instant::now();
        Ok(())
    }

    async fn write_packet(&mut self, packet: AnyPacket) -> anyhow::Result<()> {
        let bytes = packet.to_bytes();
        self.write(&bytes).await
    }
}

struct UdpWriter {
    socket: Arc<UdpSocket>,
    addr: Option<SocketAddr>,
}

impl UdpWriter {
    fn new(socket: Arc<UdpSocket>) -> Self {
        Self { socket, addr: None }
    }

    fn set_addr(&mut self, addr: SocketAddr) {
        self.addr = Some(addr);
    }

    async fn send(&self, packet: AnyPacket) -> anyhow::Result<()> {
        return self.send_raw(&packet.to_bytes()).await;
    }

    async fn send_raw(&self, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(addr) = self.addr {
            self.socket.send_to(bytes, addr).await?;

            return Ok(());
        }

        return Err(anyhow!("UPD not registered"));
    }
}
