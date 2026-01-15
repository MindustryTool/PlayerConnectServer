use anyhow::Result;
use dotenvy::dotenv;
use std::env;

pub struct Config {
    pub player_connect_port: u16,
    pub player_connect_http_port: u16,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenv().ok();

        let player_connect_port = env::var("PLAYER_CONNECT_PORT")
            .unwrap_or_else(|_| "11010".to_string())
            .parse()?;

        let player_connect_http_port = env::var("PLAYER_CONNECT_HTTP_PORT")
            .unwrap_or_else(|_| "11011".to_string())
            .parse()?;

        Ok(Config {
            player_connect_port,
            player_connect_http_port,
        })
    }
}
