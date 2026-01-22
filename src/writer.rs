use anyhow::anyhow;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::net::UdpSocket;

pub struct TcpWriter {
    writer: tokio::net::tcp::OwnedWriteHalf,
    pub last_write: Instant,
}

impl TcpWriter {
    pub fn new(writer: tokio::net::tcp::OwnedWriteHalf) -> Self {
        Self {
            writer,
            last_write: Instant::now(),
        }
    }

    pub async fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(data).await?;
        self.last_write = Instant::now();
        Ok(())
    }

    pub async fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> anyhow::Result<()> {
        self.writer.write_vectored(bufs).await?;
        self.last_write = Instant::now();
        Ok(())
    }
}

pub struct UdpWriter {
    socket: Arc<UdpSocket>,
    pub addr: Option<SocketAddr>,
}

impl UdpWriter {
    pub fn new(socket: Arc<UdpSocket>) -> Self {
        Self { socket, addr: None }
    }

    pub fn set_addr(&mut self, addr: SocketAddr) {
        self.addr = Some(addr);
    }

    pub async fn send_raw(&self, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(addr) = self.addr {
            self.socket.send_to(bytes, addr).await?;

            return Ok(());
        }

        return Err(anyhow!("UPD not registered"));
    }
}
