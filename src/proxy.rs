use crate::connection::ConnectionActor;
use crate::packet::{AnyPacket, FrameworkMessage};
use crate::rate::AtomicRateLimiter;
use crate::state::{AppState, ConnectionAction};
use crate::writer::{TcpWriter, UdpWriter};
use bytes::BytesMut;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::packet::ConnectionId;
const UDP_BUFFER_SIZE: usize = 4096;
const CHANNEL_CAPACITY: usize = 1024;
const PACKET_RATE_LIMIT_WINDOW: Duration = Duration::from_millis(3000);
const PACKET_RATE_LIMIT: u32 = 1000;

pub async fn run(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    let address = format!("0.0.0.0:{}", port);
    let tcp_listener = TcpListener::bind(&address).await?;
    let udp_socket = Arc::new(UdpSocket::bind(&address).await?);

    info!("Proxy Server listening on TCP/UDP {}", port);

    for _ in 0..4 {
        spawn_udp_listener(state.clone(), udp_socket.clone());
    }

    accept_tcp_connection(state, tcp_listener, udp_socket).await
}

fn spawn_udp_listener(state: Arc<AppState>, socket: Arc<UdpSocket>) {
    tokio::spawn(async move {
        let mut buf = BytesMut::with_capacity(UDP_BUFFER_SIZE);

        loop {
            if buf.len() > 0 {
                buf.clear();
            }

            if buf.capacity() < UDP_BUFFER_SIZE {
                buf.reserve(UDP_BUFFER_SIZE);
            }

            match socket.recv_buf_from(&mut buf).await {
                Ok((_len, addr)) => {
                    if let Some((_, limiter)) = state.get_route(&addr) {
                        if !limiter.check() {
                            continue;
                        }
                    }

                    let bytes = buf.split().freeze();
                    let mut cursor = Cursor::new(bytes);

                    match AnyPacket::read(&mut cursor) {
                        Ok(packet) => {
                            match packet {
                                AnyPacket::Framework(FrameworkMessage::RegisterUDP {
                                    connection_id,
                                }) => {
                                    handle_register_udp(&state, connection_id, addr).await;
                                }
                                _ => {
                                    let Some((sender, _)) = state.get_route(&addr) else {
                                        warn!("Unknown UDP sender: {}", addr);
                                        continue;
                                    };

                                    if let Err(e) = sender
                                        .try_send(ConnectionAction::ProcessPacket(packet, false))
                                    {
                                        warn!("Failed to forward UDP packet: {}", e);
                                    }
                                }
                            };
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

async fn handle_register_udp(state: &Arc<AppState>, connection_id: ConnectionId, addr: SocketAddr) {
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
            if let Err(e) = socket.set_nodelay(true) {
                warn!("Failed to set nodelay for connection: {}", e);
            }
            let id = loop {
                let id = ConnectionId(rand::random());
                if !state.has_connection_id(id) {
                    break id;
                }
            };

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
                notified_idle: false,
                room: None,
            };

            if let Err(e) = actor.run(reader).await {
                error!("Connection {} error: {}", id, e);
            }

            state.remove_connection(id, actor.room.as_ref().map(|r| r.room_id().clone()));

            if let Some(addr) = actor.udp_writer.addr {
                state.remove_udp(addr);
                info!("Connection {} closed", id);
            }
        });
    }
}
