use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use rust_embed::Embed;

use crate::{AppState, WebsocketCmd};

#[cfg(debug_assertions)]
#[derive(Embed)]
#[folder = "../frontend/"]
struct Asset;

#[cfg(not(debug_assertions))]
#[derive(Embed)]
#[folder = "../frontend/deploy/"]
struct Asset;

pub async fn run(state: AppState, port: usize) -> Result<()> {
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
        .route("/", get(index_handler))
        .route("/index.html", get(index_handler))
        .route("/*file", get(static_handler))
        .fallback_service(get(not_found));
    // logging so we can see whats going on
    // .layer(
    //     TraceLayer::new_for_http()
    //         .make_span_with(DefaultMakeSpan::default().include_headers(true)),
    // );

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await?;

    Ok(())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, mut state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    state
        .serial_sink
        .lock()
        .await
        .send(WebsocketCmd::Connect)
        .await
        .unwrap();

    loop {
        tokio::select! {
            msg = state.broadcast_receiver.recv() => {
                match msg {
                    Ok(msg) => sender.send(msg).await.unwrap(),
                    Err(_recv_error) => {
                        break;
                    },
                }
            }
            msg = receiver.next() => {
                match msg {
                    None => (),
                    Some(Err(e)) => {
                        eprintln!("Unknown WS Error: {:?}", e);
                        break;
                    },
                    Some(Ok(Message::Binary(bytes))) => {
                        state.serial_sink.lock().await.send(WebsocketCmd::WsMessage(bytes)).await.unwrap();
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(msg)) => todo!("Unknown WS Message: {:?}", msg)
                }
            }
        }
    }

    println!("Websocket context destroyed");
}

// We use static route matchers ("/" and "/index.html") to serve our home
// page.
async fn index_handler() -> impl IntoResponse {
    static_handler("/index.html".parse::<Uri>().unwrap()).await
}

// We use a wildcard matcher ("/*file") to match against everything
// within our defined assets directory. This is the directory on our Asset
// struct below, where folder = "examples/public/".
async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/').to_string();

    StaticFile(path)
}

// Finally, we use a fallback route for anything that didn't match.
async fn not_found() -> Html<&'static str> {
    Html("<h1>404</h1><p>Not Found</p>")
}

pub struct StaticFile<T>(pub T);

impl<T> IntoResponse for StaticFile<T>
where
    T: Into<String>,
{
    fn into_response(self) -> Response {
        let path = self.0.into();

        match Asset::get(path.as_str()) {
            Some(content) => {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
            }
            None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
        }
    }
}
