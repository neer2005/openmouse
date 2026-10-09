//! HTTP + WebSocket server.
//!
//! Serves the phone web UI (embedded in the binary) and relays input events
//! over a WebSocket using the OpenMouse JSON protocol:
//!
//! ```text
//! {"t":"move","dx":12.5,"dy":-3.0}   relative mouse motion
//! {"t":"click","button":"left"}       button: left|right|middle (+ "count")
//! {"t":"press","button":"left"}       mouse button down (drags)
//! {"t":"release","button":"left"}     mouse button up
//! {"t":"scroll","dx":0,"dy":3}        scroll ticks
//! {"t":"text","text":"hello"}         type literal text
//! {"t":"key","key":"enter"}           special key by name
//! {"t":"media","action":"playpause"}  media key
//! {"t":"hotkey","keys":["ctrl","c"]}  key chord
//! {"t":"power","action":"sleep"}      sleep|shutdown|restart|logoff
//! {"t":"ping"}                        -> {"t":"pong"}
//! ```
//!
//! Every route except `/` and `/qr.png` requires `?token=<secret>`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use tokio::net::TcpListener;

use crate::input;

static INDEX_HTML: &str = include_str!("../static/index.html");
static APP_JS: &str = include_str!("../static/app.js");
static STYLE_CSS: &str = include_str!("../static/style.css");

pub struct AppState {
    pub token: String,
    pub pair_url: String,
    pub qr_png: Vec<u8>,
    pub clients: AtomicUsize,
}

fn authed(params: &HashMap<String, String>, state: &AppState) -> bool {
    params.get("token").map(|t| t == &state.token).unwrap_or(false)
}

fn forbidden() -> impl IntoResponse {
    (StatusCode::FORBIDDEN, "bad token - rescan the QR code")
}

async fn landing(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let html = format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>OpenMouse</title>
<style>body{{font-family:system-ui,sans-serif;max-width:640px;margin:3rem auto;
padding:0 1rem;line-height:1.5}}code{{background:#eee;padding:.2em .4em;
border-radius:4px}}img{{border:1px solid #ddd;border-radius:8px}}</style></head>
<body>
<h1>&#128433; OpenMouse server running</h1>
<p>On your phone (same WiFi), scan this QR code or open:</p>
<p><img src="/qr.png" width="280" height="280" alt="pairing QR"></p>
<p><code>{url}</code></p>
<p>Backend: <b>{backend}</b></p>
</body></html>"#,
        url = state.pair_url,
        backend = input::backend_name(),
    );
    ([(header::CONTENT_TYPE, "text/html")], html)
}

async fn qr_png(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], state.qr_png.clone())
}

async fn remote_ui(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !authed(&params, &state) {
        return forbidden().into_response();
    }
    ([(header::CONTENT_TYPE, "text/html")], INDEX_HTML).into_response()
}

async fn static_file(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    if !authed(&params, &state) {
        return forbidden().into_response();
    }
    match name.as_str() {
        "app.js" => ([(header::CONTENT_TYPE, "text/javascript")], APP_JS).into_response(),
        "style.css" => ([(header::CONTENT_TYPE, "text/css")], STYLE_CSS).into_response(),
        _ => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn status(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !authed(&params, &state) {
        return forbidden().into_response();
    }
    let body = serde_json::json!({
        "backend": input::backend_name(),
        "clients": state.clients.load(Ordering::Relaxed),
    });
    ([(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !authed(&params, &state) {
        return forbidden().into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    state.clients.fetch_add(1, Ordering::Relaxed);
    eprintln!(
        "[openmouse] phone connected ({} total)",
        state.clients.load(Ordering::Relaxed)
    );
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => match dispatch(&text) {
                Dispatch::Pong => {
                    let _ = socket.send(Message::Text(r#"{"t":"pong"}"#.into())).await;
                }
                Dispatch::Handled | Dispatch::Ignored => {}
            },
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    state.clients.fetch_sub(1, Ordering::Relaxed);
    eprintln!(
        "[openmouse] phone disconnected ({} total)",
        state.clients.load(Ordering::Relaxed)
    );
}

enum Dispatch {
    Handled,
    Pong,
    Ignored,
}

/// Handle one protocol message. Unknown types and garbage input are ignored —
/// a bad event must never kill the socket.
fn dispatch(raw: &str) -> Dispatch {
    let v: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Dispatch::Ignored,
    };
    let t = match v.get("t").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return Dispatch::Ignored,
    };
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    match t {
        "ping" => return Dispatch::Pong,
        "move" => input::mouse_move(f("dx"), f("dy")),
        "click" => input::mouse_click(s("button"), f("count") as u32),
        "press" => input::mouse_press(s("button")),
        "release" => input::mouse_release(s("button")),
        "scroll" => input::mouse_scroll(f("dx"), f("dy")),
        "text" => {
            let text = s("text");
            let end = text
                .char_indices()
                .nth(500)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            input::type_text(&text[..end]);
        }
        "key" => input::press_key(s("key")),
        "media" => input::media(s("action")),
        "hotkey" => {
            let keys: Vec<String> = v
                .get("keys")
                .and_then(|k| k.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|x| x.to_string()))
                        .take(4)
                        .collect()
                })
                .unwrap_or_default();
            if !keys.is_empty() {
                input::hotkey(&keys);
            }
        }
        "power" => input::power(s("action")),
        _ => eprintln!("[openmouse] unknown message type: {t:?}"),
    }
    Dispatch::Handled
}

pub async fn run(
    host: &str,
    port: u16,
    state: Arc<AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Router::new()
        .route("/", get(landing))
        .route("/qr.png", get(qr_png))
        .route("/remote", get(remote_ui))
        .route("/static/:name", get(static_file))
        .route("/api/status", get(status))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = TcpListener::bind(addr).await?;
    eprintln!("[openmouse] listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("\n[openmouse] shutting down");
}
