//! Local JSON HTTP entry point for the Picto Relay gateway.

use std::{collections::BTreeMap, env, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context as _;
use axum::{
    Json, Router,
    extract::{
        Path as AxumPath, State,
        ws::{Message as WebSocketMessage, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, header},
    response::{Html, IntoResponse},
    routing::{delete, get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::{Parser, ValueEnum};
use picto_relay_codec::MacAddress;
use picto_relay_join::{LiveConfig, prepare_live_config};
use picto_relay_server::{
    broker::{BrokerError, BrokerHandle, ClientId},
    protocol::{
        AcceptedResponse, BITMAP_BYTES, CANVAS_HEIGHT, CANVAS_WIDTH, CreateSessionRequest,
        CreateSessionResponse, ErrorResponse, EventsResponse, PROTOCOL_VERSION, RoomsResponse,
        SendMessageRequest, ServerEnvelope, StatusResponse,
    },
};
use serde::Serialize;
use tokio::sync::{Mutex, mpsc};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const MAX_EVENTS_PER_POLL: usize = 32;

#[derive(Clone, Debug)]
struct AppState {
    broker: BrokerHandle,
    backend: &'static str,
    default_room: u8,
    api_sessions: ApiSessions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Backend {
    Memory,
    Ar9271,
}

#[derive(Debug, Parser)]
#[command(
    name = "picto-relay-server",
    about = "Expose PictoChat through a local JSON API"
)]
struct Cli {
    /// Radio backend used by API sessions.
    #[arg(long, value_enum, default_value_t = Backend::Memory)]
    backend: Backend,

    /// Dedicated AR9271 interface used for hardware acknowledgements.
    #[arg(long, default_value = "wlan1")]
    interface: String,

    /// Prepare managed and monitor interfaces before joining.
    #[arg(long)]
    prepare: bool,

    /// 2.4 GHz channel carrying the `PictoChat` room.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=14))]
    channel: u8,

    /// `PictoChat` room: A, B, C, or D.
    #[arg(long, default_value = "A", value_parser = parse_room)]
    room: u8,

    /// Nintendo-OUI MAC address used by the virtual station.
    #[arg(long, default_value = "00:09:bf:52:00:01", value_parser = parse_mac)]
    station: MacAddress,

    /// Seconds to wait for a matching room beacon.
    #[arg(long, default_value_t = 60)]
    scan_timeout: u64,

    /// Milliseconds to wait for each management response.
    #[arg(long, default_value_t = 750)]
    response_timeout_ms: u64,

    /// Authentication and association attempts.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=10))]
    attempts: u8,

    /// Optional radiotap capture path for the physical backend.
    #[arg(long)]
    capture: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
struct ApiSessions {
    events: Arc<Mutex<BTreeMap<ClientId, Option<mpsc::Receiver<ServerEnvelope>>>>>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(ErrorResponse {
                version: PROTOCOL_VERSION,
                code: self.code,
                message: self.message,
            }),
        )
            .into_response()
    }
}

impl From<BrokerError> for ApiError {
    fn from(error: BrokerError) -> Self {
        let status = match error {
            BrokerError::InvalidBitmapSize(_) | BrokerError::InvalidRoom(_) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            BrokerError::Unavailable | BrokerError::Busy => StatusCode::SERVICE_UNAVAILABLE,
        };
        Self::new(status, error_code(error), error.to_string())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let options = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let (broker, backend) = match options.backend {
        Backend::Memory => (BrokerHandle::spawn_memory(), "memory"),
        Backend::Ar9271 => {
            let config = LiveConfig {
                interface: options.interface,
                prepare: options.prepare,
                channel: options.channel,
                room: Some(options.room),
                station: options.station,
                scan_timeout: std::time::Duration::from_secs(options.scan_timeout),
                response_timeout: std::time::Duration::from_millis(options.response_timeout_ms),
                attempts: options.attempts,
                capture: options.capture,
            };
            let config = tokio::task::spawn_blocking(move || prepare_live_config(config))
                .await
                .context("radio preparation task failed")??;
            (BrokerHandle::spawn_live(config), "ar9271")
        }
    };
    let app = app(broker, backend, options.room);
    let address = env::var("PICTO_RELAY_BIND").unwrap_or_else(|_| "0.0.0.0:3001".into());
    let address: SocketAddr = address
        .parse()
        .context("invalid PICTO_RELAY_BIND address")?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind {address}"))?;
    info!(%address, "Picto Relay listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")
}

fn app(broker: BrokerHandle, backend: &'static str, default_room: u8) -> Router {
    let state = AppState {
        broker,
        backend,
        default_room,
        api_sessions: ApiSessions::default(),
    };
    Router::new()
        .route("/", get(index))
        .route("/app.css", get(stylesheet))
        .route("/app.js", get(javascript))
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/rooms", get(api_rooms))
        .route("/api/v1/rooms/ws", get(websocket_rooms))
        .route("/api/v1/sessions", post(create_api_session))
        .route("/api/v1/sessions/{session_id}", delete(delete_api_session))
        .route(
            "/api/v1/sessions/{session_id}/messages",
            post(send_api_message),
        )
        .route("/api/v1/sessions/{session_id}/events", get(poll_api_events))
        .route(
            "/api/v1/sessions/{session_id}/events/ws",
            get(websocket_api_events),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

async fn stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../assets/app.css"),
    )
}

async fn javascript() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../assets/app.js"),
    )
}

async fn api_status(State(state): State<AppState>) -> Json<StatusResponse> {
    Json(StatusResponse {
        version: PROTOCOL_VERSION,
        backend: state.backend,
        bitmap_width: CANVAS_WIDTH,
        bitmap_height: CANVAS_HEIGHT,
        bitmap_bytes: BITMAP_BYTES,
        bitmap_encoding: "base64_row_major_mono_1bpp_msb_first",
    })
}

async fn api_rooms(State(state): State<AppState>) -> Result<Json<RoomsResponse>, ApiError> {
    Ok(Json(RoomsResponse {
        version: PROTOCOL_VERSION,
        rooms: state.broker.rooms().await?,
    }))
}

async fn websocket_rooms(
    State(state): State<AppState>,
    websocket: WebSocketUpgrade,
) -> impl IntoResponse {
    let rooms = state.broker.subscribe_rooms();
    websocket.on_upgrade(move |socket| push_room_updates(socket, rooms))
}

async fn push_room_updates(
    mut socket: WebSocket,
    mut rooms: tokio::sync::watch::Receiver<Vec<picto_relay_server::protocol::RoomSummary>>,
) {
    let initial = RoomsResponse {
        version: PROTOCOL_VERSION,
        rooms: rooms.borrow_and_update().clone(),
    };
    if !send_websocket_json(&mut socket, &initial).await {
        return;
    }
    loop {
        tokio::select! {
            changed = rooms.changed() => {
                if changed.is_err() {
                    break;
                }
                let update = RoomsResponse {
                    version: PROTOCOL_VERSION,
                    rooms: rooms.borrow_and_update().clone(),
                };
                if !send_websocket_json(&mut socket, &update).await {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(WebSocketMessage::Ping(payload))) => {
                        if socket.send(WebSocketMessage::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WebSocketMessage::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

async fn websocket_api_events(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<u32>,
    websocket: WebSocketUpgrade,
) -> Result<impl IntoResponse, ApiError> {
    let session_id = ClientId(session_id);
    let sessions = state.api_sessions.events.lock().await;
    let receiver = sessions
        .get(&session_id)
        .ok_or_else(|| unknown_session(session_id))?;
    if receiver.is_none() {
        return Err(session_stream_attached(session_id));
    }
    drop(sessions);
    Ok(websocket.on_upgrade(move |socket| push_session_events(socket, state, session_id)))
}

async fn push_session_events(mut socket: WebSocket, state: AppState, session_id: ClientId) {
    let receiver = state
        .api_sessions
        .events
        .lock()
        .await
        .get_mut(&session_id)
        .and_then(Option::take);
    let Some(mut receiver) = receiver else {
        return;
    };

    loop {
        tokio::select! {
            event = receiver.recv() => {
                let Some(event) = event else {
                    break;
                };
                if !send_websocket_json(&mut socket, &event).await {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(WebSocketMessage::Ping(payload))) => {
                        if socket.send(WebSocketMessage::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WebSocketMessage::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }

    let mut sessions = state.api_sessions.events.lock().await;
    if let Some(slot @ None) = sessions.get_mut(&session_id) {
        *slot = Some(receiver);
    }
}

async fn send_websocket_json<T: Serialize>(socket: &mut WebSocket, value: &T) -> bool {
    let Ok(json) = serde_json::to_string(value) else {
        warn!("could not serialize WebSocket event");
        return false;
    };
    socket
        .send(WebSocketMessage::Text(json.into()))
        .await
        .is_ok()
}

async fn create_api_session(
    State(state): State<AppState>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), ApiError> {
    let name = normalized_name(&request.name).map_err(|message| {
        ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_name", message)
    })?;
    let room = request.room.unwrap_or(state.default_room);
    let session = state.broker.register(name, room).await?;
    let session_id = session.client_id;
    state
        .api_sessions
        .events
        .lock()
        .await
        .insert(session_id, Some(session.events));
    Ok((
        StatusCode::CREATED,
        Json(CreateSessionResponse {
            version: PROTOCOL_VERSION,
            session_id: session_id.0,
            room,
        }),
    ))
}

async fn delete_api_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<u32>,
) -> Result<StatusCode, ApiError> {
    let session_id = ClientId(session_id);
    let removed = state
        .api_sessions
        .events
        .lock()
        .await
        .remove(&session_id)
        .is_some();
    if !removed {
        return Err(unknown_session(session_id));
    }
    state.broker.unregister(session_id);
    Ok(StatusCode::NO_CONTENT)
}

async fn send_api_message(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<u32>,
    Json(request): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<AcceptedResponse>), ApiError> {
    let session_id = ClientId(session_id);
    if !state
        .api_sessions
        .events
        .lock()
        .await
        .contains_key(&session_id)
    {
        return Err(unknown_session(session_id));
    }
    let bitmap = STANDARD.decode(request.bitmap).map_err(|error| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_bitmap",
            error.to_string(),
        )
    })?;
    state.broker.send_message(session_id, bitmap)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            version: PROTOCOL_VERSION,
            status: "queued",
        }),
    ))
}

async fn poll_api_events(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<u32>,
) -> Result<Json<EventsResponse>, ApiError> {
    let session_id = ClientId(session_id);
    let mut sessions = state.api_sessions.events.lock().await;
    let receiver = sessions
        .get_mut(&session_id)
        .ok_or_else(|| unknown_session(session_id))?;
    let receiver = receiver
        .as_mut()
        .ok_or_else(|| session_stream_attached(session_id))?;
    let mut events = Vec::new();
    while events.len() < MAX_EVENTS_PER_POLL {
        match receiver.try_recv() {
            Ok(event) => events.push(event),
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                return Err(ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "broker_unavailable",
                    "session event stream is unavailable",
                ));
            }
        }
    }
    Ok(Json(EventsResponse {
        version: PROTOCOL_VERSION,
        events,
    }))
}

fn unknown_session(session_id: ClientId) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "unknown_session",
        format!("session {} does not exist", session_id.0),
    )
}

fn session_stream_attached(session_id: ClientId) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "session_stream_attached",
        format!(
            "session {} already has a WebSocket event consumer",
            session_id.0
        ),
    )
}

fn normalized_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() || name.encode_utf16().count() > 10 {
        return Err("name must contain 1 to 10 UTF-16 code units");
    }
    if name.chars().any(char::is_control) {
        return Err("name must not contain control characters");
    }
    Ok(name.to_owned())
}

fn parse_room(value: &str) -> Result<u8, String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "A" => Ok(0),
        "B" => Ok(1),
        "C" => Ok(2),
        "D" => Ok(3),
        _ => Err("room must be A, B, C, or D".into()),
    }
}

fn parse_mac(value: &str) -> Result<MacAddress, String> {
    let octets = value
        .split(':')
        .map(|octet| u8::from_str_radix(octet, 16).map_err(|_| "invalid MAC address".to_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    let octets = <[u8; 6]>::try_from(octets)
        .map_err(|_| "MAC address must contain six octets".to_owned())?;
    if octets[0] & 1 != 0 {
        return Err("station MAC must be unicast".into());
    }
    Ok(MacAddress(octets))
}

const fn error_code(error: BrokerError) -> &'static str {
    match error {
        BrokerError::Unavailable => "broker_unavailable",
        BrokerError::Busy => "broker_busy",
        BrokerError::InvalidBitmapSize(_) => "invalid_bitmap_size",
        BrokerError::InvalidRoom(_) => "invalid_room",
    }
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        warn!(%error, "failed to install Ctrl-C handler");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode},
        response::Response,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use picto_relay_server::{
        broker::BrokerHandle,
        protocol::{BITMAP_BYTES, PROTOCOL_VERSION},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt as _;

    use super::app;

    fn test_app() -> Router {
        app(BrokerHandle::spawn_memory(), "memory", 0)
    }

    async fn send_json(app: &Router, method: Method, uri: &str, body: Value) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request should build"),
            )
            .await
            .expect("router should answer")
    }

    async fn response_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("response body should be readable");
        serde_json::from_slice(&bytes).expect("response should contain JSON")
    }

    #[tokio::test]
    async fn status_should_describe_the_gateway_bitmap_contract() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/status")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["backend"], "memory");
        assert_eq!(body["bitmap_bytes"], BITMAP_BYTES);
    }

    #[tokio::test]
    async fn frontend_should_be_served_at_the_root() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn frontend_should_use_websockets_without_polling_timers() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/app.js")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");
        let bytes = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("response body should be readable");
        let javascript = str::from_utf8(&bytes).expect("JavaScript should be UTF-8");

        assert!(javascript.contains("new WebSocket"));
        assert!(javascript.contains("participant_joined"));
        assert!(javascript.contains("participant_left"));
        assert!(javascript.contains("room-count"));
        assert!(!javascript.contains("setInterval"));
    }

    #[tokio::test]
    async fn rooms_should_list_a_session_under_its_selected_room() {
        let app = test_app();
        let create = send_json(
            &app,
            Method::POST,
            "/api/v1/sessions",
            json!({"name": "D Client", "room": 3}),
        )
        .await;
        assert_eq!(create.status(), StatusCode::CREATED);

        let rooms = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/rooms")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");
        let body = response_json(rooms).await;

        assert_eq!(body["rooms"][3]["participants"][0]["name"], "D Client");
    }

    #[tokio::test]
    async fn rooms_should_include_a_count_separate_from_the_roster() {
        let app = test_app();
        let create = send_json(
            &app,
            Method::POST,
            "/api/v1/sessions",
            json!({"name": "A Client", "room": 0}),
        )
        .await;
        assert_eq!(create.status(), StatusCode::CREATED);

        let rooms = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/rooms")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");
        let body = response_json(rooms).await;

        assert_eq!(body["rooms"][0]["participant_count"], 1);
    }

    #[tokio::test]
    async fn create_session_should_reject_a_room_outside_a_through_d() {
        let response = send_json(
            &test_app(),
            Method::POST,
            "/api/v1/sessions",
            json!({"name": "Relay", "room": 4}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn json_api_should_create_send_receive_and_delete_a_session() {
        let app = test_app();
        let create = send_json(
            &app,
            Method::POST,
            "/api/v1/sessions",
            json!({"name": "API Relay", "room": 2}),
        )
        .await;
        assert_eq!(create.status(), StatusCode::CREATED);
        let create_body = response_json(create).await;
        let session_id = create_body["session_id"]
            .as_u64()
            .expect("session ID should be numeric");
        assert_eq!(create_body["room"], 2);

        let send = send_json(
            &app,
            Method::POST,
            &format!("/api/v1/sessions/{session_id}/messages"),
            json!({"bitmap": STANDARD.encode(vec![0; BITMAP_BYTES])}),
        )
        .await;
        assert_eq!(send.status(), StatusCode::ACCEPTED);

        let mut event_types = Vec::new();
        for _ in 0..20 {
            let poll = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/sessions/{session_id}/events"))
                        .body(Body::empty())
                        .expect("request should build"),
                )
                .await
                .expect("router should answer");
            assert_eq!(poll.status(), StatusCode::OK);
            let body = response_json(poll).await;
            assert_eq!(body["version"], PROTOCOL_VERSION);
            event_types.extend(
                body["events"]
                    .as_array()
                    .expect("events should be an array")
                    .iter()
                    .filter_map(|event| event["type"].as_str().map(str::to_owned)),
            );
            if event_types.iter().any(|event| event == "message")
                && event_types.iter().any(|event| event == "delivered")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(event_types.iter().any(|event| event == "message"));
        assert!(event_types.iter().any(|event| event == "delivered"));
        assert!(
            event_types
                .iter()
                .any(|event| event == "participant_joined")
        );

        let delete = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/v1/sessions/{session_id}"))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");
        assert_eq!(delete.status(), StatusCode::NO_CONTENT);

        let missing = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/sessions/{session_id}/events"))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
