use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Packet parsing error: {0}")]
    PacketParsing(String),

    #[error("Room not found: {0}")]
    RoomNotFound(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Packet spamming detected")]
    PacketSpamming,

    #[error("Lock poison error")]
    LockPoison,
    
    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}
