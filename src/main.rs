use core::panic;
use std::{borrow::Cow, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use serialport::{DataBits, FlowControl, Parity, SerialPort};
use tokio::{sync::Mutex, time::timeout};
use tower_http::{
    services::ServeDir,
    trace::{DefaultMakeSpan, TraceLayer},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, Parser)]
#[group(required = true, multiple = false)]
struct Serial {
    #[arg(short = 'l')]
    list: bool,

    #[arg(short = 'P')]
    path: Option<String>,
}

#[derive(Debug, Parser)]
struct Args {
    #[command(flatten)]
    serial: Serial,

    #[arg(short = 'p', default_value = "3000")]
    port: usize,
}

#[derive(Clone)]
struct AppState {
    serial: Arc<Mutex<Box<dyn SerialPort>>>,
    serial_path: String,
}

const BAUD_RATE: u32 = 9600; // 115200

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
        for p in serialport::available_ports().unwrap() {
            println!("{} - {:?}", p.port_name, p.port_type);
        }
    }

    if let Some(serial_path) = args.serial.path {
        let sp = serialport::new(serial_path.clone(), BAUD_RATE)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(FlowControl::None)
            .open()
            .unwrap();

        let state = AppState {
            serial: Arc::new(Mutex::new(sp)),
            serial_path,
        };

        // let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend/deploy");
        let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend");

        // build our application with some routes
        let app = Router::new()
            .fallback_service(ServeDir::new(assets_dir).append_index_html_on_directories(true))
            .route("/ws", get(ws_handler))
            .with_state(state)
            // logging so we can see whats going on
            .layer(
                TraceLayer::new_for_http()
                    .make_span_with(DefaultMakeSpan::default().include_headers(true)),
            );

        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", args.port))
            .await
            .unwrap();
        tracing::debug!("listening on {}", listener.local_addr().unwrap());
        axum::serve(listener, app).await.unwrap();
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut serial = state.serial.lock_owned().await;
    serial.write_all(&[0x44]).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    serial.write_all(&[0x45, 0x52]).unwrap();
    println!("Serial Init");

    // let mut buffer = [0; 1024];
    let mut buffer = [0; 4096];

    let (mut sender, mut receiver) = socket.split();

    loop {
        match serial.read(&mut buffer[..]) {
            Err(e) => {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    //
                } else {
                    panic!("Unknown Serial Error: {}", e);
                }
            }
            Ok(n) => {
                if n == 0 {
                    sender
                        .send(Message::Close(Some(CloseFrame {
                            code: axum::extract::ws::close_code::NORMAL,
                            reason: Cow::from("Goodbye"),
                        })))
                        .await
                        .unwrap();
                    break;
                }
                sender
                    .send(Message::Binary(buffer[..n].to_vec()))
                    .await
                    .unwrap();
            }
        }

        match timeout(Duration::from_millis(10), receiver.next()).await {
            Ok(None) => {
                //
            }
            Ok(Some(Err(e))) => {
                //
            }
            Ok(Some(Ok(msg))) => match msg {
                Message::Text(_) => todo!(),
                Message::Binary(bytes) => serial.write_all(&bytes).unwrap(),
                Message::Ping(_) => todo!(),
                Message::Pong(_) => todo!(),
                Message::Close(_) => todo!(),
            },
            Err(_) => {
                //Timeout
            }
        }
    }

    println!("Websocket context destroyed");
}
