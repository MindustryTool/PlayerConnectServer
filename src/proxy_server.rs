use crate::packets::{
    AnyPacket, AppPacket, FrameworkMessage, RoomCreationRequestPacket, RoomLinkPacket,
};
use crate::rate::RateLimiter;
use crate::state::{AppState, ConnectionState, PendingConnectionState, RoomInit};

use anyhow::anyhow;
use bytes::{Buf, BytesMut};
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tracing::{error, info};

static NEXT_CONNECTION_ID: AtomicI32 = AtomicI32::new(1);

const UDP_BUFFER_SIZE: usize = 4096;
const TCP_BUFFER_SIZE: usize = 32768;
const CHANNEL_CAPACITY: usize = 100;
const CONNECTION_TIME_OUT_MS: u64 = 10000;
const KEEP_ALIVE_INTERVAL_MS: u64 = 2000;
const PACKET_LENGTH_LENGTH: usize = 2;
const TICK_INTERVAL_SECS: u64 = 1;

const PACKET_RATE_LIMIT_WINDOW: Duration = Duration::from_millis(3000);
const PACKET_RATE_LIMIT: usize = 300;

pub async fn run(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    let address = format!("0.0.0.0:{}", port);

    let tcp_listener = TcpListener::bind(address.clone()).await?;
    let udp_socket = Arc::new(UdpSocket::bind(address).await?);

    info!("Proxy Server listening on TCP/UDP {}", port);

    spawn_udp_listener(state.clone(), udp_socket);
    accept_tcp_connection(state.clone(), tcp_listener).await
}

fn spawn_udp_listener(state: Arc<AppState>, socket: Arc<UdpSocket>) {
    tokio::spawn(async move {
        let mut buf = [0u8; UDP_BUFFER_SIZE];

        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let data = &buf[..len];
                    if let Err(error) = handle_udp_packet(&state, data, addr).await {
                        error!("UDP Error from {}: {}", addr, error);
                    }
                }
                Err(e) => error!("UDP Receive Error: {}", e),
            }
        }
    });
}

async fn accept_tcp_connection(state: Arc<AppState>, listener: TcpListener) -> anyhow::Result<()> {
    loop {
        let (socket, addr) = listener.accept().await?;

        info!("New connection from {}", addr);

        let state = state.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(state, socket).await {
                error!("Connection error: {}", e);
            }
        });
    }
}

async fn handle_udp_packet(
    state: &Arc<AppState>,
    data: &[u8],
    addr: SocketAddr,
) -> anyhow::Result<()> {
    info!("UDP: Received {} bytes from {}", data.len(), addr);

    let mut payload_cursor = Cursor::new(data);
    let packet = AnyPacket::read(&mut payload_cursor)?;

    match packet {
        AnyPacket::Framework(framework) => match framework {
            FrameworkMessage::RegisterUDP { connection_id } => {
                handle_register_udp(state, connection_id, addr).await;
            }
            FrameworkMessage::KeepAlive => {
                info!("DO SOMETHING WITH THIS");
            }
            _ => panic!("Unhandled UDP packet: {:?}", framework),
        },
        _ => panic!("UDP: Unhandled packet {:?}", packet),
    }
    Ok(())
}

async fn handle_register_udp(state: &Arc<AppState>, connection_id: i32, addr: SocketAddr) {
    if let Some((_, pending_conn)) = state.pending_connections.remove(&connection_id) {
        info!(
            "Registering UDP for connection {} at {}",
            connection_id, addr
        );

        state.connections.insert(
            connection_id,
            ConnectionState {
                id: pending_conn.id,
                tx: pending_conn.tx,
                last_write_time: pending_conn.last_write_time,
                created_at: pending_conn.created_at,
                rate: pending_conn.rate,
                udp_addr: addr,
            },
        );
    } else {
        info!(
            "Received RegisterUDP for unknown connection {}",
            connection_id
        );
        return;
    };

    let packet = AnyPacket::Framework(FrameworkMessage::RegisterUDP { connection_id });

    if let Err(e) = state.send(packet, connection_id).await {
        error!("Failed to send RegisterUDP reply via TCP: {}", e);
    }
}

async fn handle_connection(state: Arc<AppState>, socket: TcpStream) -> anyhow::Result<()> {
    let connection_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);

    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);

    register_pending_connection(&state, connection_id, tx.clone());

    let (reader, writer) = socket.into_split();
    let write_handle = spawn_write_handler(rx, writer);

    send_register_tcp(&state.clone(), connection_id).await?;
    process_tcp_stream(state.clone(), connection_id, reader).await?;

    // Cleanup
    state.disconnect(connection_id).await?;

    write_handle.abort();

    info!("Connection {} cleanup complete", connection_id);
    Ok(())
}

fn register_pending_connection(
    state: &Arc<AppState>,
    connection_id: i32,
    tx: mpsc::Sender<bytes::Bytes>,
) {
    state.pending_connections.insert(
        connection_id,
        PendingConnectionState {
            id: connection_id,
            tx,
            rate: RateLimiter::new(PACKET_RATE_LIMIT, PACKET_RATE_LIMIT_WINDOW),
            last_write_time: Instant::now(),
            created_at: Instant::now(),
        },
    );
    info!("Connection {} registered", connection_id);
}

fn spawn_write_handler(
    mut rx: mpsc::Receiver<bytes::Bytes>,
    mut writer: tokio::net::tcp::OwnedWriteHalf,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            if let Err(e) = writer.write_all(&bytes).await {
                error!("Write error: {}", e);
                break;
            }
        }
    })
}

async fn send_register_tcp(state: &Arc<AppState>, connection_id: i32) -> anyhow::Result<()> {
    let packet = AnyPacket::Framework(FrameworkMessage::RegisterTCP { connection_id });

    state
        .send(packet, connection_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send RegisterTCP: {}", e))
}

async fn process_tcp_stream(
    state: Arc<AppState>,
    connection_id: i32,
    mut reader: tokio::net::tcp::OwnedReadHalf,
) -> anyhow::Result<()> {
    let mut buf = BytesMut::with_capacity(TCP_BUFFER_SIZE);
    let mut tmp_buf = [0u8; TCP_BUFFER_SIZE];
    let mut tick_interval = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));
    let mut last_read_time = Instant::now();

    loop {
        tokio::select! {
            read_result = reader.read(&mut tmp_buf) => {
                match read_result {
                    Ok(0) => {
                        info!("Connection {} closed by peer", connection_id);
                        break;
                    }
                    Ok(n) => {
                        last_read_time = Instant::now();
                        buf.extend_from_slice(&tmp_buf[..n]);

                        process_buffer(&state, connection_id, &mut buf).await?;
                    }
                    Err(e) => {
                        error!("Read error for connection {}: {}", connection_id, e);
                        break;
                    }
                }
            }
            _ = tick_interval.tick() => {
                if !handle_tick(&state, connection_id, last_read_time).await {
                    break;
                }
            }
        }
    }
    Ok(())
}

async fn process_buffer(
    state: &Arc<AppState>,
    connection_id: i32,
    buf: &mut BytesMut,
) -> anyhow::Result<()> {
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

        match AnyPacket::read(&mut cursor) {
            Err(e) => {
                error!(
                    "Error parsing packet for connection {} with size {}: {}",
                    connection_id, len, e
                );
            }
            Ok(packet) => {
                info!(
                    "TCP: Received packet {:?} from connection {}",
                    packet, connection_id
                );

                if let Err(e) = handle_packet(state, connection_id, packet).await {
                    error!(
                        "Error handling packet for connection {}: {}",
                        connection_id, e
                    );
                }
            }
        }
    }

    Ok(())
}

async fn handle_tick(state: &Arc<AppState>, connection_id: i32, last_read_time: Instant) -> bool {
    if last_read_time.elapsed() > Duration::from_millis(CONNECTION_TIME_OUT_MS) {
        info!("Connection {} timed out", connection_id);
        return false;
    }

    let should_send = if let Some(conn) = state.connections.get(&connection_id) {
        conn.last_write_time.elapsed() > Duration::from_millis(KEEP_ALIVE_INTERVAL_MS)
    } else if let Some(conn) = state.pending_connections.get(&connection_id) {
        conn.last_write_time.elapsed() > Duration::from_millis(KEEP_ALIVE_INTERVAL_MS)
    } else {
        info!("Unknown connection id: {}", connection_id);
        return false;
    };

    if should_send {
        let packet = AnyPacket::Framework(FrameworkMessage::KeepAlive);

        if let Err(err) = state.send(packet, connection_id).await {
            error!("Fail to send keep alive: {}", err)
        };
    }

    true
}

async fn handle_packet(
    state: &Arc<AppState>,
    connection_id: i32,
    packet: AnyPacket,
) -> anyhow::Result<()> {
    match packet {
        AnyPacket::Framework(packet) => {
            handle_framework(state, connection_id, packet).await?;
        }
        AnyPacket::App(packet) => {
            handle_app_packet(state, connection_id, packet).await?;
        }
        AnyPacket::Raw(_) => todo!("Handle raw packet {:?}", packet),
    }

    Ok(())
}

async fn handle_framework(
    state: &Arc<AppState>,
    connection_id: i32,
    packet: FrameworkMessage,
) -> anyhow::Result<()> {
    match packet {
        FrameworkMessage::Ping { id, is_reply } => {
            if !is_reply {
                let packet = AnyPacket::Framework(FrameworkMessage::Ping { id, is_reply: true });

                state.send(packet, connection_id).await?;
            }
        }
        FrameworkMessage::KeepAlive => {
            if let Some(mut conn) = state.connections.get_mut(&connection_id) {
                conn.last_write_time = Instant::now();
            }
        }
        _ => panic!("Unhandled packet: {:?}", packet),
    }

    Ok(())
}

async fn handle_app_packet(
    state: &Arc<AppState>,
    connection_id: i32,
    packet: AppPacket,
) -> anyhow::Result<()> {
    let room = state.rooms.find_connection_room(&connection_id);

    let is_rate_limited = {
        let mut connection = state
            .connections
            .get_mut(&connection_id)
            .expect("Connection must exist");

        room.map(|r| r.host_connection_id == connection_id)
            .unwrap_or(true)
            && !connection.rate.allow()
    };

    if is_rate_limited {
        info!("Connection rate limited: {}", connection_id);
        state.disconnect(connection_id).await?;
        return Err(anyhow!("Rate limited"));
    }

    match packet {
        AppPacket::RoomJoin(package) => {
            // Add pending request
            state.rooms.join(connection_id, &package.room_id);
        }
        AppPacket::RoomCreationRequest(RoomCreationRequestPacket {
            version: _,
            password,
            data: stats,
        }) => {
            let room_id = state.rooms.create(RoomInit {
                connection_id,
                password,
                stats,
            });

            let packet = AnyPacket::App(AppPacket::RoomLink(RoomLinkPacket {
                room_id: room_id.clone(),
            }));

            state.send(packet, connection_id).await?;
        }
        _ => info!("Handle app packet {:?}", packet),
    }

    Ok(())
}
