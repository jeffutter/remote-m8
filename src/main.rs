mod audio;
mod serial;

use core::panic;
use std::{io::Write, sync::Arc, thread};

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
use clap::Parser;
use flate2::{write::ZlibEncoder, Compression};
use futures::{stream::SplitSink, SinkExt, StreamExt};
use log::info;
use rust_embed::Embed;
use serial::SLIPCodec;
use tokio::sync::{
    broadcast::{Receiver, Sender},
    Mutex,
};
use tokio_util::codec::Framed;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, Parser)]
#[group(required = true, multiple = false)]
struct SerialArgs {
    #[arg(short = 'l')]
    list: bool,

    #[arg(short = 'P')]
    path: Option<String>,
}

#[derive(Debug, Parser)]
struct Args {
    #[command(flatten)]
    serial: SerialArgs,

    #[arg(short = 'p', default_value = "3000")]
    port: usize,
}

struct AppState {
    broadcast_receiver: Receiver<Message>,
    broadcast_sender: Sender<Message>,
    serial_sink: Arc<Mutex<SplitSink<Framed<tokio_serial::SerialStream, SLIPCodec>, WebsocketCmd>>>,
}

#[cfg(debug_assertions)]
#[derive(Embed)]
#[folder = "frontend/"]
struct Asset;

#[cfg(not(debug_assertions))]
#[derive(Embed)]
#[folder = "frontend/deploy/"]
struct Asset;

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            broadcast_sender: self.broadcast_sender.clone(),
            broadcast_receiver: self.broadcast_sender.subscribe(),
            serial_sink: self.serial_sink.clone(),
        }
    }
}

enum WebsocketCmd {
    Connect,
    WsMessage(Vec<u8>),
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    if args.serial.list {
        for p in tokio_serial::available_ports().unwrap() {
            info!("{} - {:?}", p.port_name, p.port_type);
        }
    }

    if let Some(serial_path) = args.serial.path {
        let (broadcast_sender, broadcast_receiver) = tokio::sync::broadcast::channel::<Message>(8);
        let (serial_sink, mut serial_stream) = serial::Serial::new(serial_path).unwrap().stream();
        let mut audio_receiver = audio::run_audio();

        let audio_handler = thread::spawn(move || {});

        let broadcast_sender1 = broadcast_sender.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(data) = serial_stream.next() => {
                        match data {
                            Ok(data) => {
                                let mut prefix_data = vec![b'S'];
                                prefix_data.extend_from_slice(&data);
                                broadcast_sender1.send(Message::Binary(prefix_data)).unwrap();
                            },
                            Err(_) => todo!(),
                        }
                    }
                    Some(data) = audio_receiver.next() => {
                        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                        encoder.write_all(&data).unwrap();
                        let encoded = encoder.finish().unwrap();

                        let mut vec_data: Vec<u8> = vec![b'A'];
                        vec_data.extend_from_slice(&encoded);
                        broadcast_sender1.send(Message::Binary(vec_data)).unwrap();
                    }

                }
            }
        });

        let state = AppState {
            broadcast_receiver,
            broadcast_sender,
            serial_sink: Arc::new(Mutex::new(serial_sink)),
        };

        // build our application with some routes
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

        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", args.port))
            .await
            .unwrap();
        tracing::debug!("listening on {}", listener.local_addr().unwrap());
        axum::serve(listener, app).await.unwrap();

        audio_handler.join().unwrap();
    }
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
