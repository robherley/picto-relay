//! Local JSON HTTP entry point for the Picto Relay gateway.

use std::{collections::BTreeMap, env, net::SocketAddr, sync::Arc};

use anyhow::Context as _;
use axum::{
    Json, Router,
    extract::{
        Path as AxumPath, State,
        ws::{Message as WebSocketMessage, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::{Parser, ValueEnum};
use picto_relay_server::{
    broker::{BrokerError, BrokerHandle, ClientId, DEFAULT_SERIAL_PORT},
    protocol::{
        BITMAP_BYTES, CANVAS_HEIGHT, CANVAS_WIDTH, CreateSessionRequest, CreateSessionResponse,
        DRAWABLE_REGIONS, ErrorResponse, EventsResponse, INK_INDICES, NAME_MAX_UNITS,
        PALETTE_BITMAP_BYTES, PROFILE_COLOR_MAX, PROFILE_MESSAGE_MAX_UNITS, ROOM_NAMES,
        RoomsResponse, SendMessageRequest, SessionEvent, SessionResponse, StatusResponse,
        TEXT_MAX_UNITS,
    },
};
use serde::Serialize;
use tokio::sync::{Mutex, mpsc};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use utoipa::OpenApi;

// --- HTTP server -----------------------------------------------------------------

/// Environment variable that overrides the listening socket address.
const BIND_ADDRESS_ENV: &str = "PICTO_RELAY_BIND";

/// Socket address listened on when `PICTO_RELAY_BIND` is unset: every
/// interface, TCP port 3001.
const DEFAULT_BIND_ADDRESS: &str = "0.0.0.0:3001";

/// Log filter directive used when `RUST_LOG` is unset or invalid.
const DEFAULT_LOG_FILTER: &str = "info";

/// Maximum number of events returned by one polling request.
const MAX_EVENTS_PER_POLL: usize = 32;

// --- Web client ------------------------------------------------------------------

/// The browser client. It is built separately (`just web-build` writes
/// `services/web/dist`) and embedded here at compile time. `allow_missing` lets the
/// server compile without it, in which case it says how to build the client.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../services/web/dist"]
#[allow_missing = true]
struct WebClient;

/// File served for the root path.
const WEB_INDEX: &str = "index.html";

/// Directory whose files the client's bundler names by a hash of their content.
const WEB_HASHED_PREFIX: &str = "assets/";

/// Caching for files named by content hash: they never change.
const CACHE_FOREVER: &str = "public, max-age=31536000, immutable";

/// Caching for everything else: check with the server before reuse.
const CACHE_REVALIDATE: &str = "no-cache";

/// Response body when the server was compiled before the web client was built.
const WEB_CLIENT_MISSING: &str = "The web client has not been built. Run `just web-build` and \
    restart the server. The JSON API under /api/v1 works without it.\n";

// --- Command-line defaults ---------------------------------------------------------

/// Room letter joined when `--room` is omitted.
const DEFAULT_ROOM_NAME: &str = ROOM_NAMES[0];

// --- OpenAPI ---------------------------------------------------------------------

/// Tag for gateway-wide operations.
const TAG_GATEWAY: &str = "gateway";
/// Tag for room listing operations.
const TAG_ROOMS: &str = "rooms";
/// Tag for operations on one session.
const TAG_SESSIONS: &str = "sessions";
/// Path at which the server publishes its own `OpenAPI` document.
const OPENAPI_PATH: &str = "/api/v1/openapi.json";

/// The `OpenAPI` description of the JSON API, generated from the handlers and
/// the types in `protocol.rs`. `docs/openapi.json` is this document, checked in.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Picto Relay",
        description = "Join a Nintendo DS PictoChat room and exchange drawings over HTTP and WebSocket.\n\nA session is one participant in one room. Create it, send drawings or text, and read events by polling or over a WebSocket. Events are `SessionEvent` objects whose `type` field selects the variant."
    ),
    paths(
        api_status,
        api_rooms,
        websocket_rooms,
        create_api_session,
        get_api_session,
        delete_api_session,
        send_api_message,
        poll_api_events,
        websocket_api_events,
    ),
    components(schemas(SessionEvent, RoomsResponse)),
    tags(
        (name = TAG_GATEWAY, description = "What this gateway is and what it accepts"),
        (name = TAG_ROOMS, description = "Who is in rooms A through D"),
        (name = TAG_SESSIONS, description = "Join a room, send messages, receive events"),
    )
)]
struct ApiDoc;

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

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
    Esp32,
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

    /// USB serial device for the ESP32 backend.
    #[arg(long, default_value = DEFAULT_SERIAL_PORT)]
    serial: String,

    /// Record every radio frame the ESP32 hears or sends to this pcap file
    /// (appended to if it exists), for inspection in Wireshark.
    #[arg(long, value_name = "FILE")]
    capture: Option<std::path::PathBuf>,

    /// `PictoChat` room: A, B, C, or D.
    #[arg(long, default_value = DEFAULT_ROOM_NAME, value_parser = parse_room)]
    room: u8,
}

#[derive(Clone, Debug, Default)]
struct ApiSessions {
    events: Arc<Mutex<BTreeMap<ClientId, Option<mpsc::Receiver<SessionEvent>>>>>,
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
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_LOG_FILTER.into()),
        )
        .init();

    let (broker, backend) = match options.backend {
        Backend::Memory => (BrokerHandle::spawn_memory(), "memory"),
        Backend::Esp32 => (
            BrokerHandle::spawn_esp32(options.serial, options.capture),
            "esp32",
        ),
    };
    let app = app(broker, backend, options.room);
    let address = env::var(BIND_ADDRESS_ENV).unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.into());
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
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route(OPENAPI_PATH, get(openapi))
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/rooms", get(api_rooms))
        .route("/api/v1/rooms/ws", get(websocket_rooms))
        .route("/api/v1/sessions", post(create_api_session))
        .route(
            "/api/v1/sessions/{session_id}",
            get(get_api_session).delete(delete_api_session),
        )
        .route(
            "/api/v1/sessions/{session_id}/messages",
            post(send_api_message),
        )
        .route("/api/v1/sessions/{session_id}/events", get(poll_api_events))
        .route(
            "/api/v1/sessions/{session_id}/events/ws",
            get(websocket_api_events),
        )
        .fallback(web_client)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Serves a file of the embedded web client; every path that is not an API
/// route lands here.
async fn web_client(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { WEB_INDEX } else { path };
    match WebClient::get(path) {
        Some(file) => {
            let cache = if path.starts_with(WEB_HASHED_PREFIX) {
                CACHE_FOREVER
            } else {
                CACHE_REVALIDATE
            };
            (
                [
                    (header::CONTENT_TYPE, file.metadata.mimetype().to_owned()),
                    (header::CACHE_CONTROL, cache.to_owned()),
                ],
                file.data,
            )
                .into_response()
        }
        None if path == WEB_INDEX => {
            (StatusCode::SERVICE_UNAVAILABLE, WEB_CLIENT_MISSING).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Describe the gateway
///
/// Reports the active radio backend and the drawing contract: bitmap sizes,
/// text and profile limits, accepted ink indices and the drawable area.
#[utoipa::path(
    get,
    operation_id = "getStatus",
    path = "/api/v1/status",
    tag = TAG_GATEWAY,
    responses((status = 200, description = "Gateway description", body = StatusResponse))
)]
async fn api_status(State(state): State<AppState>) -> Json<StatusResponse> {
    Json(StatusResponse {
        ink_indices: INK_INDICES,
        text_max_units: TEXT_MAX_UNITS,
        profile_message_max_units: PROFILE_MESSAGE_MAX_UNITS,
        profile_color_max: PROFILE_COLOR_MAX,
        drawable_regions: DRAWABLE_REGIONS,
        backend: state.backend,
        bitmap_width: CANVAS_WIDTH,
        bitmap_height: CANVAS_HEIGHT,
        bitmap_bytes: BITMAP_BYTES,
        palette_bitmap_bytes: PALETTE_BITMAP_BYTES,
    })
}

/// List rooms
///
/// Member counts and known participants of rooms A through D. With the ESP32
/// backend and no open session, counts come from periodic channel scans.
#[utoipa::path(
    get,
    operation_id = "listRooms",
    path = "/api/v1/rooms",
    tag = TAG_ROOMS,
    responses(
        (status = 200, description = "Room snapshots in A-through-D order", body = RoomsResponse),
        (status = 503, description = "The broker is unavailable or busy", body = ErrorResponse),
    )
)]
async fn api_rooms(State(state): State<AppState>) -> Result<Json<RoomsResponse>, ApiError> {
    Ok(Json(RoomsResponse {
        rooms: state.broker.rooms().await?,
    }))
}

/// Stream room updates (WebSocket)
///
/// Upgrades to a WebSocket. The server sends one `RoomsResponse` as a JSON text
/// frame immediately and another whenever a room changes. Client frames are ignored.
#[utoipa::path(
    get,
    operation_id = "streamRooms",
    path = "/api/v1/rooms/ws",
    tag = TAG_ROOMS,
    responses((status = 101, description = "Switching to WebSocket; text frames are `RoomsResponse` JSON"))
)]
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

/// Stream session events (WebSocket)
///
/// Upgrades to a WebSocket. Each event is one `SessionEvent` as a JSON text
/// frame. A session has one event consumer: while a WebSocket is attached,
/// polling returns 409, and a second WebSocket is refused.
#[utoipa::path(
    get,
    operation_id = "streamSessionEvents",
    path = "/api/v1/sessions/{session_id}/events/ws",
    tag = TAG_SESSIONS,
    params(("session_id" = u64, Path, description = "Identifier returned when the session was created")),
    responses(
        (status = 101, description = "Switching to WebSocket; text frames are `SessionEvent` JSON"),
        (status = 404, description = "No such session", body = ErrorResponse),
        (status = 409, description = "Another WebSocket already consumes this session", body = ErrorResponse),
    )
)]
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

/// Join a room
///
/// Creates a session: a virtual `PictoChat` participant with the given name and
/// profile. With the ESP32 backend this joins the DS-hosted room; follow the
/// session's events for `backend_state` progress and `session_ready`.
#[utoipa::path(
    post,
    operation_id = "createSession",
    path = "/api/v1/sessions",
    tag = TAG_SESSIONS,
    request_body = CreateSessionRequest,
    responses(
        (status = 201, description = "Session created", body = CreateSessionResponse),
        (status = 422, description = "Invalid name, room, profile message or profile color", body = ErrorResponse),
        (status = 503, description = "The backend is unavailable or already in use", body = ErrorResponse),
    )
)]
async fn create_api_session(
    State(state): State<AppState>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), ApiError> {
    let name = normalized_name(&request.name).map_err(|message| {
        ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_name", message)
    })?;
    let room = request.room.unwrap_or(state.default_room);
    if request.profile_message.encode_utf16().count() > PROFILE_MESSAGE_MAX_UNITS
        || request.profile_message.contains('\0')
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_profile_message",
            "Profile message must be at most 26 UTF-16 units and contain no NUL characters",
        ));
    }
    if request.profile_color > PROFILE_COLOR_MAX {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_profile_color",
            "Profile color must be between 0 and 15",
        ));
    }
    let session = state
        .broker
        .register_with_profile(name, room, request.profile_message, request.profile_color)
        .await?;
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
            session_id: session_id.0,
            room,
        }),
    ))
}

/// Inspect a session
///
/// Reports the room the session is in and everyone it has seen there. People
/// who have left stay in the list with `active` set to `false`.
#[utoipa::path(
    get,
    operation_id = "getSession",
    path = "/api/v1/sessions/{session_id}",
    tag = TAG_SESSIONS,
    params(("session_id" = u64, Path, description = "Identifier returned when the session was created")),
    responses(
        (status = 200, description = "The session's room and users", body = SessionResponse),
        (status = 404, description = "No such session", body = ErrorResponse),
        (status = 503, description = "The broker is unavailable or busy", body = ErrorResponse),
    )
)]
async fn get_api_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<u32>,
) -> Result<Json<SessionResponse>, ApiError> {
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
    let session = state
        .broker
        .inspect(session_id)
        .await?
        .ok_or_else(|| unknown_session(session_id))?;
    Ok(Json(SessionResponse {
        room: session.room,
        room_letter: ROOM_NAMES[usize::from(session.room)],
        users: session.users,
    }))
}

/// Leave the room
///
/// Ends the session and releases the radio.
#[utoipa::path(
    delete,
    operation_id = "deleteSession",
    path = "/api/v1/sessions/{session_id}",
    tag = TAG_SESSIONS,
    params(("session_id" = u64, Path, description = "Identifier returned when the session was created")),
    responses((status = 204, description = "Session ended"), (status = 404, description = "No such session", body = ErrorResponse))
)]
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

/// Send a drawing or text
///
/// Queues one message. Supply exactly one of `text` (drawn by the server in
/// the DS font, optionally with `ink`), `bitmap` (1 bit per pixel) or
/// `palette_bitmap` (4 bits per pixel). Delivery is reported later as a
/// `delivered` or `delivery_timed_out` event.
#[utoipa::path(
    post,
    operation_id = "sendMessage",
    path = "/api/v1/sessions/{session_id}/messages",
    tag = TAG_SESSIONS,
    params(("session_id" = u64, Path, description = "Identifier returned when the session was created")),
    request_body = SendMessageRequest,
    responses(
        (status = 202, description = "Message queued"),
        (status = 404, description = "No such session", body = ErrorResponse),
        (status = 422, description = "Invalid text, ink or bitmap", body = ErrorResponse),
        (status = 503, description = "The backend is unavailable or its queue is full", body = ErrorResponse),
    )
)]
async fn send_api_message(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<u32>,
    Json(request): Json<SendMessageRequest>,
) -> Result<StatusCode, ApiError> {
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
    let bitmap = if let Some(text) = request.text {
        if !request.bitmap.is_empty() || request.palette_bitmap.is_some() {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_message",
                "Supply text, bitmap, or palette_bitmap, not multiple formats",
            ));
        }
        picto_relay_server::text::render(&text, &request.ink.unwrap_or_default()).map_err(
            |message| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_text", message),
        )?
    } else {
        if request.ink.is_some() {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_message",
                "Ink applies only to text; bitmaps already encode their ink",
            ));
        }
        if request.palette_bitmap.is_some() && !request.bitmap.is_empty() {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_bitmap",
                "Supply either bitmap or palette_bitmap, not both",
            ));
        }
        let (encoded, expected) = request
            .palette_bitmap
            .map_or((request.bitmap, BITMAP_BYTES), |data| {
                (data, PALETTE_BITMAP_BYTES)
            });
        let bitmap = STANDARD.decode(encoded).map_err(|error| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_bitmap",
                error.to_string(),
            )
        })?;
        if bitmap.len() != expected {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_bitmap_size",
                format!("Expected {expected} bytes, received {}", bitmap.len()),
            ));
        }
        bitmap
    };
    state.broker.send_message(session_id, bitmap)?;
    Ok(StatusCode::ACCEPTED)
}

/// Poll session events
///
/// Returns and removes the events queued since the previous poll, at most 32
/// per request. Use the WebSocket endpoint instead for push delivery.
#[utoipa::path(
    get,
    operation_id = "pollSessionEvents",
    path = "/api/v1/sessions/{session_id}/events",
    tag = TAG_SESSIONS,
    params(("session_id" = u64, Path, description = "Identifier returned when the session was created")),
    responses(
        (status = 200, description = "Queued events, oldest first", body = EventsResponse),
        (status = 404, description = "No such session", body = ErrorResponse),
        (status = 409, description = "A WebSocket is consuming this session's events", body = ErrorResponse),
        (status = 503, description = "The session's event stream is unavailable", body = ErrorResponse),
    )
)]
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
    Ok(Json(EventsResponse { events }))
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
    if name.is_empty() || name.encode_utf16().count() > NAME_MAX_UNITS {
        return Err("name must contain 1 to 10 UTF-16 code units");
    }
    if name.chars().any(char::is_control) {
        return Err("name must not contain control characters");
    }
    Ok(name.to_owned())
}

fn parse_room(value: &str) -> Result<u8, String> {
    let letter = value.trim().to_ascii_uppercase();
    ROOM_NAMES
        .iter()
        .position(|name| *name == letter)
        .and_then(|room| u8::try_from(room).ok())
        .ok_or_else(|| "room must be A, B, C, or D".into())
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
        http::{Method, Request, StatusCode, header},
        response::Response,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use picto_relay_server::{
        broker::BrokerHandle,
        protocol::{BITMAP_BYTES, PALETTE_BITMAP_BYTES},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt as _;

    use super::{ApiDoc, WEB_INDEX, WebClient, app};
    use utoipa::OpenApi as _;

    /// `docs/openapi.json` is the published schema clients generate from. It must
    /// be exactly what the server serves; regenerate it with `just openapi`.
    #[test]
    fn checked_in_openapi_document_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/openapi.json");
        let expected = ApiDoc::openapi().to_pretty_json().unwrap() + "\n";
        if std::env::var_os("PICTO_WRITE_FIXTURES").is_some() {
            std::fs::write(&path, &expected).unwrap();
        }
        assert!(
            std::fs::read_to_string(&path).unwrap() == expected,
            "docs/openapi.json is out of date; run `just openapi`"
        );
    }

    #[test]
    fn openapi_document_covers_every_api_route() {
        let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let paths = document["paths"].as_object().unwrap();
        for path in [
            "/api/v1/status",
            "/api/v1/rooms",
            "/api/v1/rooms/ws",
            "/api/v1/sessions",
            "/api/v1/sessions/{session_id}",
            "/api/v1/sessions/{session_id}/messages",
            "/api/v1/sessions/{session_id}/events",
            "/api/v1/sessions/{session_id}/events/ws",
        ] {
            assert!(paths.contains_key(path), "{path} is not documented");
        }
        assert_eq!(paths.len(), 8);
        let session = paths["/api/v1/sessions/{session_id}"].as_object().unwrap();
        assert!(session.contains_key("get") && session.contains_key("delete"));
        // Every event variant is distinguishable by its `type` tag.
        let events = &document["components"]["schemas"]["SessionEvent"]["oneOf"];
        assert_eq!(events.as_array().unwrap().len(), 10);
        // The API is versioned by its path alone.
        assert!(!document.to_string().contains("\"version\":{"));
    }

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
    async fn server_publishes_its_openapi_document() {
        let response = send_json(&test_app(), Method::GET, super::OPENAPI_PATH, json!({})).await;
        assert_eq!(response.status(), StatusCode::OK);
        let served = response_json(response).await;
        assert_eq!(served, serde_json::to_value(ApiDoc::openapi()).unwrap());
        assert_eq!(
            served["paths"]["/api/v1/sessions"]["post"]["operationId"],
            "createSession"
        );
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
    async fn root_should_serve_the_web_client_or_say_how_to_build_it() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");

        if WebClient::get(WEB_INDEX).is_some() {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
        } else {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    #[tokio::test]
    async fn unknown_paths_should_not_be_found() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/assets/missing.js")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
            assert!(body.get("version").is_none());
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
    #[tokio::test]
    async fn palette_api_preserves_indices() {
        let app = test_app();
        send_json(
            &app,
            Method::POST,
            "/api/v1/sessions",
            json!({"name":"Color", "room":0}),
        )
        .await;
        let mut pixels = vec![0; PALETTE_BITMAP_BYTES];
        pixels[0] = 0xf3;
        let encoded = STANDARD.encode(pixels);
        for invalid in [
            json!({"palette_bitmap":"AA=="}),
            json!({"palette_bitmap":encoded,"bitmap":"AA=="}),
        ] {
            let response =
                send_json(&app, Method::POST, "/api/v1/sessions/1/messages", invalid).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
        let response = send_json(
            &app,
            Method::POST,
            "/api/v1/sessions/1/messages",
            json!({"palette_bitmap":encoded}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        for _ in 0..20 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/sessions/1/events")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = response_json(response).await;
            if let Some(event) = body["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["type"] == "message")
            {
                assert_eq!(event["palette_bitmap"], encoded);
                assert!(event.get("bitmap").is_none());
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("color message was not received");
    }
    #[tokio::test]
    async fn session_can_be_inspected_and_remembers_who_left() {
        let app = test_app();
        for name in ["Ann", "Ben"] {
            let created = send_json(
                &app,
                Method::POST,
                "/api/v1/sessions",
                json!({"name": name, "room": 2}),
            )
            .await;
            assert_eq!(created.status(), StatusCode::CREATED);
        }
        let inspect = |uri: &'static str| {
            let app = app.clone();
            async move {
                app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                    .await
                    .unwrap()
            }
        };

        let body = response_json(inspect("/api/v1/sessions/1").await).await;
        assert_eq!(body["room"], 2);
        assert_eq!(body["room_letter"], "C");
        assert_eq!(
            body["users"],
            json!([
                {"client_id": 1, "name": "Ann", "active": true},
                {"client_id": 2, "name": "Ben", "active": true},
            ])
        );

        let left = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/sessions/2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(left.status(), StatusCode::NO_CONTENT);
        // The broker hears of the departure a moment after the request returns.
        let mut users = Value::Null;
        for _ in 0..50 {
            users = response_json(inspect("/api/v1/sessions/1").await).await["users"].take();
            if users[1]["active"] == false {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(
            users,
            json!([
                {"client_id": 1, "name": "Ann", "active": true},
                {"client_id": 2, "name": "Ben", "active": false},
            ])
        );

        assert_eq!(
            inspect("/api/v1/sessions/2").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn profile_message_rejects_overflow_and_embedded_nul() {
        let app = test_app();
        for message in ["a".repeat(27), "a\0b".into(), "😀".repeat(14)] {
            let response = send_json(
                &app,
                Method::POST,
                "/api/v1/sessions",
                json!({"name":"Relay", "profile_message":message}),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
        let response = send_json(
            &app,
            Method::POST,
            "/api/v1/sessions",
            json!({"name":"Relay", "profile_message":"abcdefghijklmnopqrstuvwxyz"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
    }
    #[tokio::test]
    async fn profile_color_validates_ds_palette_range() {
        let app = test_app();
        let invalid = send_json(
            &app,
            Method::POST,
            "/api/v1/sessions",
            json!({"name":"Relay", "profile_color":16}),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let valid = send_json(
            &app,
            Method::POST,
            "/api/v1/sessions",
            json!({"name":"Relay", "profile_color":15}),
        )
        .await;
        assert_eq!(valid.status(), StatusCode::CREATED);
    }
    #[tokio::test]
    async fn rest_text_sends_colored_bitmap_and_rejects_ambiguous_payloads() {
        let app = test_app();
        send_json(
            &app,
            Method::POST,
            "/api/v1/sessions",
            json!({"name":"API","profile_message":"Hi","profile_color":2}),
        )
        .await;
        for body in [
            json!({"text":"Hi","bitmap":"AA=="}),
            json!({"text":"Hi","ink":0}),
            json!({"ink":3,"bitmap":"AA=="}),
        ] {
            assert_eq!(
                send_json(&app, Method::POST, "/api/v1/sessions/1/messages", body)
                    .await
                    .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        assert_eq!(
            send_json(
                &app,
                Method::POST,
                "/api/v1/sessions/1/messages",
                json!({"text":"Hello world","ink":5})
            )
            .await
            .status(),
            StatusCode::ACCEPTED
        );
        for _ in 0..20 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/sessions/1/events")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = response_json(response).await;
            if let Some(event) = body["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["type"] == "message")
            {
                let bytes = STANDARD
                    .decode(event["palette_bitmap"].as_str().unwrap())
                    .unwrap();
                assert!(bytes.iter().any(|b| *b != 0));
                assert!(
                    bytes
                        .iter()
                        .all(|b| [0, 5].contains(&(b & 15)) && [0, 5].contains(&(b >> 4)))
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("missing rendered text event");
    }
}
