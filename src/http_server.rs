use crate::state::{AppState, RoomUpdate};
use axum::{
    extract::{Path, State},
    response::{
        sse::{Event, Sse},
        Html, IntoResponse,
    },
    routing::{get, post},
    Router,
};
use futures::stream::Stream;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tracing::info;

pub async fn run(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/api/v1/ping", get(ping))
        .route("/api/v1/rooms", get(rooms_sse))
        .route("/:roomId", get(room_page))
        .route("/:roomId", post(room_port))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    info!("HTTP Server listening on {}", port);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn ping() -> impl IntoResponse {
    "OK"
}

async fn rooms_sse(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, axum::BoxError>>> {
    let rx = state.rooms.subscribe();
    let stream = BroadcastStream::new(rx);

    // Initial state: Send all current rooms
    // We need to construct a stream that starts with current rooms and then follows updates
    // For simplicity here, we just subscribe to updates.
    // Ideally, we should send an initial "snapshot" event or individual add events.

    let stream = stream.map(|msg| match msg {
        Ok(update) => {
            let event = match update {
                RoomUpdate::Update(room) => Event::default().event("update").json_data(room.stats),
                RoomUpdate::Remove(id) => Ok({ Event::default().event("remove").data(id) }),
            };
            event.map_err(|e| axum::BoxError::from(e))
        }
        Err(e) => Err(axum::BoxError::from(e)),
    });

    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(10)))
}

async fn room_page(
    Path(room_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let stats = {
        if let Ok(rooms) = state.rooms.rooms.read() {
            rooms.get(&room_id).map(|r| r.stats.clone())
        } else {
            None
        }
    };

    if let Some(stats) = stats {
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Room: {}</title>
    <meta property="og:title" content="Room: {}" />
    <meta property="og:description" content="Map: {}, Gamemode: {}, Players: {}, Version: {}" />
    <meta property="og:type" content="website" />
    <meta property="og:site_name" content="Mindustry PlayerConnect" />
    
    <!-- Additional OpenGraph Tags for Stats -->
    <meta property="og:map_name" content="{}" />
    <meta property="og:gamemode" content="{}" />
    <meta property="og:player_count" content="{}" />
    <meta property="og:locale" content="{}" />
    <meta property="og:version" content="{}" />
    <meta property="og:created_at" content="{}" />
    
</head>
<body>
    <h1>Room: {}</h1>
    <ul>
        <li><strong>Map:</strong> {}</li>
        <li><strong>Gamemode:</strong> {}</li>
        <li><strong>Players:</strong> {}</li>
        <li><strong>Locale:</strong> {}</li>
        <li><strong>Version:</strong> {}</li>
        <li><strong>Created At:</strong> {}</li>
    </ul>
</body>
</html>"#,
            stats.name,
            stats.name,
            stats.map_name,
            stats.gamemode,
            stats.players.len(),
            stats.version,
            stats.map_name,
            stats.gamemode,
            stats.players.len(),
            stats.locale,
            stats.version,
            stats.created_at,
            stats.name,
            stats.map_name,
            stats.gamemode,
            stats.players.len(),
            stats.locale,
            stats.version,
            stats.created_at
        );
        Html(html)
    } else {
        Html("<h1>Room not found</h1>".to_string())
    }
}

async fn room_port(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    // Return the TCP port (which is in config, maybe we should store it in state or config)
    // For now hardcode or get from config if we passed it.
    // Ideally pass config to state.
    "11010" // Using default, should be dynamic
}
