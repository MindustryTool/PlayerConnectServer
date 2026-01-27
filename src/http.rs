use crate::models::{RemoveRemoveEvent, RoomUpdateEvent, RoomView};
use crate::state::{AppState, RoomUpdate};
use axum::{
    extract::State,
    http::header,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Router,
};
use futures::stream::once;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tracing::info;

pub async fn run(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/ping", get(ping))
        .route("/rooms", get(rooms_sse))
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

async fn rooms_sse(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rx = state.room_state.broadcast_sender.subscribe();
    let stream = BroadcastStream::new(rx);

    let initial_rooms: Vec<RoomUpdateEvent> = state.room_state.into_views();

    let init_stream = once(async move {
        Event::default()
            .event("update")
            .json_data(initial_rooms)
            .map_err(axum::BoxError::from)
    });

    let update_stream = stream.filter_map(|msg| match msg {
        Ok(update) => {
            let data = match update {
                RoomUpdate::Update { id, data } => Event::default()
                    .event("update")
                    .json_data(vec![RoomUpdateEvent {
                        room_id: id.0,
                        data: RoomView::from(&data),
                    }])
                    .map_err(axum::BoxError::from),
                RoomUpdate::Remove(id) => Event::default()
                    .event("remove")
                    .json_data(RemoveRemoveEvent { room_id: id.0 })
                    .map_err(axum::BoxError::from),
            };

            Some(data)
        }
        Err(BroadcastStreamRecvError::Lagged(_)) => None,
    });

    let stream = init_stream.chain(update_stream);

    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::CONNECTION, "keep-alive"),
        ],
        Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(3))),
    )
}

async fn room_port(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.config.player_connect_port.to_string()
}
