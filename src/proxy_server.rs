use crate::packets::{
    AnyPacket, AppPacket, FrameworkMessage, RoomCreationRequestPacket, RoomLinkPacket,
};
use crate::state::{AppState, ConnectionState};
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
use uuid::{uuid, Uuid};

static NEXT_CONNECTION_ID: AtomicI32 = AtomicI32::new(1);

const BUFFER_SIZE: usize = 32768;
const CONNECTION_TIME_OUT: u64 = 10000; // In ms
const KEEP_ALIVE_INTERVAL: u64 = 8000; // In
const PACKET_LENGTH_LENGTH: usize = 2;

pub async fn run(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    let address = format!("0.0.0.0:{}", port);

    let listener = TcpListener::bind(address.clone()).await?;
    let udp_socket = Arc::new(UdpSocket::bind(address).await?);

    info!("Proxy Server listening on TCP/UDP {}", port);

    let udp_state = state.clone();
    let udp_socket_clone = udp_socket.clone();

    // UDP Listener Loop
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match udp_socket_clone.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let data = &buf[..len];
                    if let Err(e) = handle_udp_packet(&udp_state, data, addr).await {
                        error!("UDP Error from {}: {}", addr, e);
                    }
                }
                Err(e) => {
                    error!("UDP Receive Error: {}", e);
                }
            }
        }
    });

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
    // Basic packet structure check: Length (2 bytes) + Payload

    info!("UDP: Received {} bytes from {}", data.len(), addr);

    // if data.len() < PACKET_LENGTH_LENGTH {
    //     return Ok(());
    // }

    // let mut cursor = Cursor::new(data);
    // let len = cursor.get_u16() as usize;

    // if data.len() < PACKET_LENGTH_LENGTH + len {
    //     warn!("Invalid packet length {} from {}", len, addr);
    //     return Ok(());
    // }

    // let payload = &data[PACKET_LENGTH_LENGTH..PACKET_LENGTH_LENGTH ];
    let mut payload_cursor = Cursor::new(data);
    let packet = AnyPacket::read(&mut payload_cursor);

    match packet {
        Ok(AnyPacket::Framework(FrameworkMessage::RegisterUDP { connection_id })) => {
            let tx = if let Some(mut conn) = state.connections.get_mut(&connection_id) {
                info!(
                    "Registering UDP for connection {} at {}",
                    connection_id, addr
                );
                conn.udp_addr = Some(addr);

                Some(conn.tx.clone())
            } else {
                info!(
                    "Received RegisterUDP for unknown connection {}",
                    connection_id
                );
                None
            };

            match tx {
                Some(tx) => {
                    // Send RegisterUDP reply via TCP
                    let reply =
                        AnyPacket::Framework(FrameworkMessage::RegisterUDP { connection_id });

                    let mut buf = BytesMut::new();

                    reply.write(&mut buf);

                    if let Err(e) = tx.send(buf.freeze()).await {
                        error!("Failed to send RegisterUDP reply via TCP: {}", e);
                    } else {
                        if let Some(mut conn) = state.connections.get_mut(&connection_id) {
                            conn.last_write_time = Instant::now();
                        }
                    }
                }
                None => {
                    warn!(
                        "Received RegisterUDP for unknown connection {}",
                        connection_id
                    );
                }
            }
        }
        Ok(packet) => {
            // Handle other UDP packets if necessary (e.g. game data)
            // For now, we only care about registration or forwarding
            info!("UDP: Unhandled packet {:?}", packet);
        }
        Err(e) => {
            error!("Error parsing UDP packet from {}: {}", addr, e);
        }
    }
    Ok(())
}

async fn handle_connection(
    state: Arc<AppState>,
    socket: tokio::net::TcpStream,
) -> anyhow::Result<()> {
    let connection_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);

    let (tx, mut rx) = mpsc::channel(100);

    // Register connection
    state.connections.insert(
        connection_id,
        ConnectionState {
            id: connection_id,
            room_id: None,
            is_host: false,
            tx: tx.clone(),
            udp_addr: None,
            last_write_time: Instant::now(),
        },
    );

    info!("Connection {} registered", connection_id);

    // Split socket
    let (mut reader, mut writer) = socket.into_split();

    // Spawn Write Loop
    let write_handle = tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            if let Err(e) = writer.write_all(&bytes).await {
                error!("Write error: {}", e);
                break;
            }
        }
    });

    // Send RegisterTCP to client
    let mut write_buf: BytesMut = BytesMut::new();
    let register_packet = AnyPacket::Framework(FrameworkMessage::RegisterTCP { connection_id });

    register_packet.write(&mut write_buf);

    if let Err(e) = tx.send(write_buf.freeze()).await {
        error!("Failed to send RegisterTCP: {}", e);
        return Ok(());
    }

    // Read Loop
    let mut buf = BytesMut::with_capacity(BUFFER_SIZE);
    let mut tmp_buf = [0u8; BUFFER_SIZE];
    let mut tick_interval = tokio::time::interval(Duration::from_millis(KEEP_ALIVE_INTERVAL));
    let mut last_read_time = Instant::now();

    loop {
        tokio::select! {
            read_result = reader.read(&mut tmp_buf) => {
                match read_result {
                    Ok(n) if n == 0 => {
                        info!("Connection {} closed by peer", connection_id);
                        break;
                    }
                    Ok(n) => {
                        last_read_time = Instant::now();
                        buf.extend_from_slice(&tmp_buf[..n]);

                        loop {
                            info!("TCP: Received {} bytes from connection {}", n, connection_id);

                            if buf.len() < PACKET_LENGTH_LENGTH {
                                break;
                            }

                            // Peek length
                            let len = {
                                let mut cur = Cursor::new(&buf[..]);
                                cur.get_u16() as usize
                            };

                            if buf.len() < PACKET_LENGTH_LENGTH + len {
                                break;
                            }

                            buf.advance(PACKET_LENGTH_LENGTH);

                            // Extract payload
                            let payload = buf.split_to(len);
                            let mut cursor = Cursor::new(&payload[..]);

                            match AnyPacket::read(&mut cursor) {
                                Ok(packet) => {
                                    info!("TCP: Received packet {:?} from connection {}", packet, connection_id);

                                    if let Err(e) = handle_packet(&state, connection_id, packet).await {
                                        error!("Error handling packet for connection {}: {}", connection_id, e);
                                        // Depending on severity, we might want to close connection
                                        // For now, log and continue
                                    }
                                }
                                Err(e) => {
                                    error!("Error parsing packet for connection {} with size {}: {}", connection_id,len, e);
                                    // Packet is consumed and discarded
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Read error for connection {}: {}", connection_id, e);
                        break;
                    }
                }
            }
            _ = tick_interval.tick() => {
                if last_read_time.elapsed() > Duration::from_millis(CONNECTION_TIME_OUT) {
                    info!("Connection {} timed out", connection_id);
                    break;
                }

                let should_send = if let Some(conn) = state.connections.get(&connection_id) {
                     conn.last_write_time.elapsed() > Duration::from_secs(8)
                } else {
                     break;
                };

                if should_send {
                    let mut buf = BytesMut::new();
                    let packet = AnyPacket::Framework(FrameworkMessage::KeepAlive);
                    packet.write(&mut buf);
                    if tx.send(buf.freeze()).await.is_err() {
                        break;
                    }
                    if let Some(mut conn) = state.connections.get_mut(&connection_id) {
                        conn.last_write_time = Instant::now();
                    }
                }
            }
        }
    }

    // Cleanup
    state.connections.remove(&connection_id);
    write_handle.abort();
    info!("Connection {} cleanup complete", connection_id);
    Ok(())
}

async fn handle_packet(
    state: &Arc<AppState>,
    connection_id: i32,
    packet: AnyPacket,
) -> anyhow::Result<()> {
    match packet {
        AnyPacket::Framework(msg) => match msg {
            FrameworkMessage::Ping { id, is_reply } => {
                if !is_reply {
                    let pong = AnyPacket::Framework(FrameworkMessage::Ping { id, is_reply: true });
                    send_packet(state, connection_id, pong).await;
                }
            }
            FrameworkMessage::KeepAlive => {}
            _ => {}
        },
        AnyPacket::App(app_msg) => {
            handle_app_packet(state, connection_id, app_msg).await?;
        }
        AnyPacket::Raw(_) => todo!("Handle raw packet {:?}", packet),
    }
    Ok(())
}

async fn handle_app_packet(
    state: &Arc<AppState>,
    connection_id: i32,
    packet: AppPacket,
) -> anyhow::Result<()> {
    match packet {
        AppPacket::RoomJoin(pkt) => {
            info!("Connection {} joining room {}", connection_id, pkt.room_id);
            if let Some(mut conn) = state.connections.get_mut(&connection_id) {
                conn.room_id = Some(pkt.room_id.clone());
            }
        }
        AppPacket::Message(pkt) => {
            info!("Connection {} sent message: {}", connection_id, pkt.message);
        }
        AppPacket::RoomCreationRequest(RoomCreationRequestPacket {
            version,
            password,
            data,
        }) => {
            send_packet(
                state,
                connection_id,
                AnyPacket::App(AppPacket::RoomLink(RoomLinkPacket {
                    room_id: Uuid::now_v7().to_string(),
                })),
            )
            .await;
        }
        _ => todo!("Handle app packet {:?}", packet),
    }
    Ok(())
}

async fn send_packet(state: &Arc<AppState>, connection_id: i32, packet: AnyPacket) {
    let tx = if let Some(conn) = state.connections.get(&connection_id) {
        Some(conn.tx.clone())
    } else {
        None
    };

    if let Some(tx) = tx {
        let mut buf = BytesMut::new();
        packet.write(&mut buf);
        if let Ok(_) = tx.send(buf.freeze()).await {
            if let Some(mut conn) = state.connections.get_mut(&connection_id) {
                conn.last_write_time = Instant::now();
            }
        }
    } else {
        error!("No connection found for ID {}", connection_id);
    }
}
