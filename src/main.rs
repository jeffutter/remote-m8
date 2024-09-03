use core::panic;
use std::{borrow::Cow, path::PathBuf, thread, time::Duration};

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
use serialport::{DataBits, FlowControl, Parity};
use tokio::sync::broadcast::{Receiver, Sender};
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

struct AppState {
    serial_broadcast_receiver: Receiver<Message>,
    serial_broadcast_sender: Sender<Message>,
    serial_control_sender: std::sync::mpsc::Sender<WebsocketCmd>,
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            serial_broadcast_sender: self.serial_broadcast_sender.clone(),
            serial_broadcast_receiver: self.serial_broadcast_sender.subscribe(),
            serial_control_sender: self.serial_control_sender.clone(),
        }
    }
}

const BAUD_RATE: u32 = 9600; // 115200

#[repr(u8)]
enum SerialCmd {
    Disconnect = 0x44,
    Enable = 0x45,
    Reset = 0x52,
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
        for p in serialport::available_ports().unwrap() {
            println!("{} - {:?}", p.port_name, p.port_type);
        }
    }

    if let Some(serial_path) = args.serial.path {
        let (serial_sender, mut serial_receiver) = tokio::sync::mpsc::channel::<Message>(1024);
        let (serial_broadcast_sender, serial_broadcast_receiver) =
            tokio::sync::broadcast::channel::<Message>(1024);
        let (serial_control_sender, serial_control_receiver) =
            std::sync::mpsc::channel::<WebsocketCmd>();

        let handler = thread::spawn(move || {
            let mut sp = serialport::new(serial_path.clone(), BAUD_RATE)
                .data_bits(DataBits::Eight)
                .parity(Parity::None)
                .stop_bits(serialport::StopBits::One)
                .flow_control(FlowControl::None)
                .open()
                .unwrap();

            sp.write_all(&[SerialCmd::Disconnect as u8]).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            sp.write_all(&[SerialCmd::Enable as u8, SerialCmd::Reset as u8])
                .unwrap();
            println!("Serial Init");

            // let mut buffer = [0; 1024];
            let mut buffer = [0; 4096];
            loop {
                match sp.read(&mut buffer[..]) {
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::TimedOut {
                            //
                        } else {
                            panic!("Unknown Serial Error: {}", e);
                        }
                    }
                    Ok(n) => {
                        if n == 0 {
                            serial_sender
                                .blocking_send(Message::Close(Some(CloseFrame {
                                    code: axum::extract::ws::close_code::NORMAL,
                                    reason: Cow::from("Goodbye"),
                                })))
                                .unwrap();
                            break;
                        }
                        serial_sender
                            .blocking_send(Message::Binary(buffer[..n].to_vec()))
                            .unwrap();
                    }
                }

                match serial_control_receiver.recv_timeout(Duration::from_millis(10)) {
                    Ok(WebsocketCmd::Connect) => {
                        sp.write_all(&[SerialCmd::Enable as u8, SerialCmd::Reset as u8])
                            .unwrap();
                    }
                    Ok(WebsocketCmd::WsMessage(msg)) => {
                        sp.write_all(&msg).unwrap();
                    }
                    Err(_timeout) => {
                        //
                    }
                }
            }
        });

        let serial_broadcast_sender1 = serial_broadcast_sender.clone();
        tokio::spawn(async move {
            loop {
                if let Some(msg) = serial_receiver.recv().await {
                    serial_broadcast_sender1.send(msg).unwrap();
                }
            }
        });

        let state = AppState {
            serial_broadcast_receiver,
            serial_broadcast_sender,
            serial_control_sender,
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

async fn handle_socket(socket: WebSocket, mut state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    state
        .serial_control_sender
        .send(WebsocketCmd::Connect)
        .unwrap();

    loop {
        tokio::select! {
            msg = state.serial_broadcast_receiver.recv() => {
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
                        state.serial_control_sender.send(WebsocketCmd::WsMessage(bytes)).unwrap()
                    }
                    Some(Ok(msg)) => todo!("Unknown WS Message: {:?}", msg)
                }
            }
        }
    }

    println!("Websocket context destroyed");
}
