use crate::connection::ConnectionActor;
use crate::packet::{AnyPacket, FrameworkMessage};
use crate::rate::AtomicRateLimiter;
use crate::state::{AppState, ConnectionAction};
use crate::writer::{TcpWriter, UdpWriter};
use bytes::Bytes;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::packet::ConnectionId;

static NEXT_CONNECTION_ID: AtomicI32 = AtomicI32::new(1);

const UDP_BUFFER_SIZE: usize = 4096;
const CHANNEL_CAPACITY: usize = 100;
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
                    let mut cursor = Cursor::new(Bytes::copy_from_slice(data));

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
                                
                                if let Some((sender, _)) = state.get_route(&addr) {
                                    if let Err(e) = sender
                                        .try_send(ConnectionAction::ProcessPacket(packet, false))
                                    {
                                        warn!("Failed to forward UDP packet: {}", e);
                                    }
                                } else {
                                    // Unknown UDP sender, ignore
                                    warn!("Unknown UDP sender: {}", addr);
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
            let id = ConnectionId(NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed));

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
