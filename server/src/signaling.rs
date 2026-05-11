use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::room::{AppState, JoinRequest, KickRequest, PeerRole, ServerEvent, SignalPayload};

#[derive(Clone)]
pub struct SharedState {
    pub config: Arc<Config>,
    pub rooms: AppState,
}

pub async fn landing_page(State(state): State<SharedState>) -> Html<String> {
    let base = state
        .config
        .web
        .publicbase
        .clone()
        .unwrap_or_else(|| "".to_string());
    Html(render_landing_page(&base))
}

pub async fn room_page(Path(room): Path<String>) -> Html<String> {
    Html(render_room_page(&room))
}

pub async fn room_info(
    Path(room): Path<String>,
    State(state): State<SharedState>,
) -> impl IntoResponse {
    match state.rooms.snapshot(&room).await {
        Some(snapshot) => Json(json!({ "ok": true, "room": snapshot })).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "room not found" })),
        )
            .into_response(),
    }
}

pub async fn backend_root() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "backend": true,
        "paths": {
            "api": "/backend/api",
            "ws": "/backend/ws"
        }
    }))
}

pub async fn ice_config(
    Query(query): Query<IceQuery>,
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let room = query.room.unwrap_or_else(|| "public".to_string());
    let role = query.role.unwrap_or_else(|| "viewer".to_string());
    Json(json!({
        "iceServers": state.config.ice_servers(&room, &role)
    }))
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Path((room, role)): Path<(String, String)>,
    Query(query): Query<WsQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Response {
    let role = match parse_role(&role) {
        Ok(role) => role,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };

    if !is_valid_room(&room) {
        return (StatusCode::BAD_REQUEST, "invalid room id").into_response();
    }

    if !origin_allowed(&headers, state.config.server.originany) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }

    if state.rooms.online_count().await >= state.config.server.maxonline {
        return (StatusCode::TOO_MANY_REQUESTS, "room service is full").into_response();
    }

    let join_request = JoinRequest {
        room,
        role,
        peer_id: query.peer_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        client_token: normalized_client_token(query.client_token),
        display_name: query.display_name.unwrap_or_default(),
        password: query.password.filter(|p| !p.is_empty()),
        remote_addr: Some(addr.ip().to_string()),
        tx: mpsc::unbounded_channel().0,
    };

    ws.on_upgrade(move |socket| handle_socket(socket, state, join_request))
}

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    peer_id: Option<String>,
    client_token: Option<String>,
    display_name: Option<String>,
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct IceQuery {
    room: Option<String>,
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientEvent {
    Offer {
        target: String,
        sdp: String,
    },
    Answer {
        target: String,
        sdp: String,
    },
    IceCandidate {
        target: String,
        candidate: serde_json::Value,
    },
    SetDisplayName {
        display_name: String,
    },
    SetJoinPassword {
        password: Option<String>,
    },
    KickPeer {
        target: String,
        ban_client_token: bool,
        ban_networks: bool,
    },
    ReportNetworkFingerprints {
        fingerprints: Vec<String>,
    },
    Ping,
}

async fn handle_socket(mut socket: WebSocket, state: SharedState, mut request: JoinRequest) {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerEvent>();
    request.tx = tx;

    let room = request.room.clone();
    let role = request.role;
    let peer_id = request.peer_id.clone();

    let join = state.rooms.join(request).await;
    let join = match join {
        Ok(join) => join,
        Err(message) => {
            let _ = socket
                .send(Message::Text(
                    serde_json::to_string(&ServerEvent::Error { message })
                        .unwrap()
                        .into(),
                ))
                .await;
            let _ = socket.close().await;
            return;
        }
    };

    info!(room, %peer_id, ?role, "peer connected");

    let welcome = ServerEvent::Welcome {
        peer_id: peer_id.clone(),
        room: room.clone(),
        role,
        host_present: join.host_present,
        viewer_count: join.viewer_count,
        password_protected: join.password_protected,
        self_display_name: join.self_display_name,
        peers: join.peers,
        host_peer_details: join.host_peer_details,
        ice_servers: state.config.ice_servers(&room, role_name(role)),
    };
    if send_event(&mut socket, &welcome).await.is_err() {
        state.rooms.leave(&room, role, &peer_id).await;
        return;
    }

    state.rooms.broadcast_join(&room, &peer_id).await;

    loop {
        tokio::select! {
            outbound = rx.recv() => {
                let Some(event) = outbound else {
                    break;
                };
                if send_event(&mut socket, &event).await.is_err() {
                    break;
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(message)) => {
                        if handle_inbound_message(&state, &room, role, &peer_id, message).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        warn!(room, %peer_id, %error, "websocket receive error");
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    state.rooms.leave(&room, role, &peer_id).await;
    info!(room, %peer_id, ?role, "peer disconnected");
}

async fn handle_inbound_message(
    state: &SharedState,
    room: &str,
    role: PeerRole,
    peer_id: &str,
    message: Message,
) -> Result<(), ()> {
    match message {
        Message::Text(text) => {
            let event: ClientEvent = match serde_json::from_str(&text) {
                Ok(event) => event,
                Err(error) => {
                    warn!(room, %peer_id, %error, "invalid message");
                    return Ok(());
                }
            };

            match event {
                ClientEvent::Offer { target, sdp } => {
                    relay_signal(state, room, peer_id, &target, SignalPayload::Offer { sdp }).await
                }
                ClientEvent::Answer { target, sdp } => {
                    relay_signal(state, room, peer_id, &target, SignalPayload::Answer { sdp }).await
                }
                ClientEvent::IceCandidate { target, candidate } => {
                    relay_signal(
                        state,
                        room,
                        peer_id,
                        &target,
                        SignalPayload::IceCandidate { candidate },
                    )
                    .await
                }
                ClientEvent::SetDisplayName { display_name } => state
                    .rooms
                    .update_display_name(room, peer_id, display_name)
                    .await
                    .map(|_| ())
                    .map_err(|error| warn!(room, %peer_id, %error, "display name update failed")),
                ClientEvent::SetJoinPassword { password } => state
                    .rooms
                    .set_join_password(room, peer_id, password)
                    .await
                    .map(|_| ())
                    .map_err(|error| warn!(room, %peer_id, %error, "password update failed")),
                ClientEvent::KickPeer {
                    target,
                    ban_client_token,
                    ban_networks,
                } => state
                    .rooms
                    .kick_peer(KickRequest {
                        room: room.to_string(),
                        requester_peer_id: peer_id.to_string(),
                        target_peer_id: target,
                        ban_client_token,
                        ban_networks,
                    })
                    .await
                    .map_err(|error| warn!(room, %peer_id, %error, "kick failed")),
                ClientEvent::ReportNetworkFingerprints { fingerprints } => state
                    .rooms
                    .update_network_fingerprints(room, peer_id, fingerprints)
                    .await
                    .map_err(|error| warn!(room, %peer_id, ?role, %error, "network report failed")),
                ClientEvent::Ping => Ok(()),
            }
        }
        Message::Close(_) => Err(()),
        Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => Ok(()),
    }
}

async fn relay_signal(
    state: &SharedState,
    room: &str,
    from: &str,
    target: &str,
    signal: SignalPayload,
) -> Result<(), ()> {
    state
        .rooms
        .relay(
            room,
            target,
            ServerEvent::Signal {
                from: from.to_string(),
                signal,
            },
        )
        .await
        .map_err(|error| warn!(room, %from, %target, %error, "failed to relay signal"))
}

async fn send_event(socket: &mut WebSocket, event: &ServerEvent) -> Result<(), ()> {
    let payload = serde_json::to_string(event).map_err(|_| ())?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|_| ())
}

fn parse_role(input: &str) -> Result<PeerRole, &'static str> {
    match input {
        "host" => Ok(PeerRole::Host),
        "viewer" => Ok(PeerRole::Viewer),
        _ => Err("role must be host or viewer"),
    }
}

fn role_name(role: PeerRole) -> &'static str {
    match role {
        PeerRole::Host => "host",
        PeerRole::Viewer => "viewer",
    }
}

fn is_valid_room(room: &str) -> bool {
    !room.is_empty()
        && room.len() <= 64
        && room
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_'))
}

fn origin_allowed(headers: &HeaderMap, allow_any: bool) -> bool {
    if allow_any {
        return true;
    }

    let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok()) else {
        return true;
    };
    let Some(host) = headers.get(HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };

    let Ok(url) = http::Uri::try_from(origin.replace("http://", "").replace("https://", "")) else {
        return false;
    };

    url.authority()
        .map(|authority| authority.as_str().eq_ignore_ascii_case(host))
        .unwrap_or(false)
}

fn normalized_client_token(input: Option<String>) -> String {
    let value = input.unwrap_or_else(|| Uuid::new_v4().to_string());
    let token: String = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(64)
        .collect();

    if token.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        token
    }
}

fn render_landing_page(base: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Room Share</title>
    <link rel="stylesheet" href="/assets/style.css">
  </head>
  <body class="landing">
    <main class="shell">
      <section class="hero-card">
        <p class="eyebrow">Anonymous WebRTC desktop share</p>
        <h1>Share a screen with a room link.</h1>
        <p class="lede">Rust server, P2P WebRTC delivery, password-controlled rooms, host moderation, and lightweight static client deployment.</p>
        <form class="room-form">
          <label class="field">
            <span>Room ID</span>
            <input id="room-input" autocomplete="off" spellcheck="false" placeholder="design-review-7" maxlength="64">
          </label>
          <div class="actions">
            <button type="button" id="host-btn">Open as host</button>
            <button type="button" id="viewer-btn" class="secondary">Open as viewer</button>
          </div>
        </form>
        <div class="meta">
          <span>Base URL: {base}</span>
          <span>Server config comes from `ROOM_*` env vars.</span>
        </div>
      </section>
    </main>
    <script>
      const input = document.getElementById('room-input');
      const slug = () => Math.random().toString(36).slice(2, 10);
      if (!input.value) input.value = slug();
      document.getElementById('host-btn').onclick = () => {{
        location.href = `/room/${{encodeURIComponent(input.value || slug())}}?role=host`;
      }};
      document.getElementById('viewer-btn').onclick = () => {{
        location.href = `/room/${{encodeURIComponent(input.value || slug())}}?role=viewer`;
      }};
    </script>
  </body>
</html>"#
    )
}

fn render_room_page(room: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Room {room}</title>
    <link rel="stylesheet" href="/assets/style.css">
  </head>
  <body data-room="{room}">
    <main class="room-shell">
      <section class="room-header">
        <div>
          <p class="eyebrow">Room</p>
          <h1>{room}</h1>
        </div>
        <div class="header-actions">
          <button id="copy-link" class="secondary">Copy viewer link</button>
          <a href="/" class="secondary link-button">New room</a>
        </div>
      </section>

      <section class="control-card">
        <div class="field">
          <span>Display name</span>
          <input id="display-name" autocomplete="off" maxlength="48" placeholder="guest">
        </div>
        <div class="field">
          <span>Join password</span>
          <input id="join-password" type="password" autocomplete="off" maxlength="64" placeholder="optional for host, required for viewer">
        </div>
        <div class="field">
          <span>Backend URL</span>
          <input id="backend-url" autocomplete="off" maxlength="256" placeholder="https://share.example.com/backend">
        </div>
        <div class="role-grid">
          <label><input type="radio" name="role" value="host"> Host</label>
          <label><input type="radio" name="role" value="viewer"> Viewer</label>
        </div>
        <div class="actions">
          <button id="connect-btn">Connect</button>
          <button id="share-btn" class="secondary" disabled>Start screen capture</button>
          <button id="set-password-btn" class="secondary" disabled>Update join password</button>
        </div>
        <p id="status" class="status">Idle</p>
      </section>

      <section class="layout">
        <div class="video-card">
          <video id="remote-video" autoplay playsinline controls></video>
          <video id="local-preview" autoplay playsinline muted></video>
        </div>
        <aside class="side-card">
          <h2>Endpoint model</h2>
          <ul class="notes">
            <li>Frontend page paths stay on the public site.</li>
            <li>Backend uses one distinct base path for API and WebSocket calls.</li>
            <li>Default backend URL can be set by `VITE_BACKEND_URL`.</li>
          </ul>
          <h2>Participants</h2>
          <div id="participants" class="participants"></div>
          <h2>Room notes</h2>
          <ul class="notes">
            <li>Single host per room.</li>
            <li>Password changes only affect future joins.</li>
            <li>Host can kick and ban by token or network fingerprint.</li>
          </ul>
          <pre id="log" class="log"></pre>
        </aside>
      </section>
    </main>
    <script src="/assets/config.js"></script>
    <script type="module" src="/assets/app.js"></script>
  </body>
</html>"#
    )
}
