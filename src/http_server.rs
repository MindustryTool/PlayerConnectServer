use crate::state::{AppState, RemoveRemoveEvent, RoomUpdate, RoomUpdateEvent};
use axum::{
    extract::{Path, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse,
    },
    routing::{get, post},
    Router,
};
use futures::stream::{once, Stream};
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
    let rx = state.tx.subscribe();
    let stream = BroadcastStream::new(rx);

    let initial_rooms: Vec<RoomUpdateEvent> = {
        let rooms = state.rooms.read();

        if let Some(rooms) = rooms {
            rooms
                .iter()
                .map(|(key, room)| RoomUpdateEvent {
                    room_id: key.clone(),
                    data: room.stats.clone(),
                })
                .collect()
        } else {
            vec![]
        }
    };

    let init_stream = once(async move {
        Event::default()
            .event("update")
            .json_data(initial_rooms)
            .map_err(axum::BoxError::from)
    });

    let update_stream = stream.map(|msg| match msg {
        Ok(update) => {
            let event = match update {
                RoomUpdate::Update { id, data } => {
                    Event::default().event("update").json_data(RoomUpdateEvent {
                        room_id: id.clone(),
                        data: data.clone(),
                    })
                }
                RoomUpdate::Remove(id) => Event::default()
                    .event("remove")
                    .json_data(RemoveRemoveEvent { room_id: id }),
            };
            event.map_err(|e| axum::BoxError::from(e))
        }
        Err(e) => Err(axum::BoxError::from(e)),
    });

    let stream = init_stream.chain(update_stream);

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
}

async fn room_page(
    Path(room_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let stats = {
        if let Some(rooms) = state.rooms.read() {
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
