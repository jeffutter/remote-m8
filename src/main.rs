use core::panic;
use std::{borrow::Cow, io::Write, path::PathBuf, thread, time::Duration};

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
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleRate, SupportedStreamConfig,
};
use flate2::{write::GzEncoder, Compression};
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
    broadcast_receiver: Receiver<Message>,
    broadcast_sender: Sender<Message>,
    serial_control_sender: std::sync::mpsc::Sender<WebsocketCmd>,
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            broadcast_sender: self.broadcast_sender.clone(),
            broadcast_receiver: self.broadcast_sender.subscribe(),
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
        let (serial_sender, mut serial_receiver) = tokio::sync::mpsc::channel::<Message>(8);
        let (broadcast_sender, broadcast_receiver) = tokio::sync::broadcast::channel::<Message>(8);
        let (serial_control_sender, serial_control_receiver) =
            std::sync::mpsc::channel::<WebsocketCmd>();

        let (audio_sender, mut audio_receiver) = tokio::sync::mpsc::channel::<Message>(8);

        let audio_handler = thread::spawn(move || {
            let host = cpal::default_host();
            let m8_audio_input_device = host
                .input_devices()
                .into_iter()
                .find_map(|mut d| d.find(|x| x.name().is_ok_and(|name| name == "M8")))
                .expect("Couldn't find M8 Audio Device");
            let config = SupportedStreamConfig::new(
                2,
                SampleRate(44100),
                cpal::SupportedBufferSize::Range { min: 4, max: 4096 },
                cpal::SampleFormat::F32,
            );
            println!("Default input config: {:?}", config);
            let err_fn = move |err| {
                eprintln!("an error occurred on stream: {}", err);
            };
            let stream = m8_audio_input_device
                .build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &_| {
                        // Don't send if all 0's
                        let (prefix, aligned, suffix) = unsafe { data.align_to::<u128>() };
                        if prefix.iter().all(|&x| x == 0.0)
                            && suffix.iter().all(|&x| x == 0.0)
                            && aligned.iter().all(|&x| x == 0)
                        {
                            return;
                        }

                        let u8_data = unsafe {
                            std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4)
                        };
                        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
                        encoder.write_all(u8_data).unwrap();
                        let encoded = encoder.finish().unwrap();

                        let mut vec_data: Vec<u8> = vec![b'A'];
                        vec_data.extend_from_slice(&encoded);

                        audio_sender
                            .blocking_send(Message::Binary(vec_data))
                            .unwrap();
                    },
                    err_fn,
                    None,
                )
                .unwrap();

            stream.play().unwrap();
        });

        let serial_handler = thread::spawn(move || {
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

            let mut buffer = [0; 1024];
            // let mut buffer = [0; 4096];
            let mut work_buffer: Vec<u8> = Vec::new();
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
                        work_buffer.extend(&buffer[..n]);

                        let mut split_at: Option<usize> = None;
                        let mut it = work_buffer.iter().enumerate().peekable();
                        while let Some((idx, e)) = it.next() {
                            match (e, it.peek()) {
                                (0xC0, None) => {
                                    split_at = Some(idx);
                                }
                                (_e, None) => {
                                    break;
                                }
                                (0xC0, _) => {
                                    split_at = Some(idx);
                                    continue;
                                }
                                (0xDB, Some((_, 0xDC))) => {
                                    continue;
                                }
                                (0xDB, Some((_, 0xDD))) => {
                                    continue;
                                }
                                (_, _) => continue,
                            }
                        }

                        match split_at {
                            None => {
                                continue;
                            }
                            Some(idx) => {
                                let tmp_buffer = work_buffer.clone();
                                let (to_send, rest) = tmp_buffer.split_at(idx + 1);
                                work_buffer = rest.to_vec();
                                let mut vec_data: Vec<u8> = vec![b'S'];
                                vec_data.extend_from_slice(to_send);
                                serial_sender
                                    .blocking_send(Message::Binary(vec_data.to_vec()))
                                    .unwrap();
                            }
                        }
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

        let broadcast_sender1 = broadcast_sender.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(msg) = serial_receiver.recv() => {
                        broadcast_sender1.send(msg).unwrap();
                    }
                    Some(msg) = audio_receiver.recv() => {
                        broadcast_sender1.send(msg).unwrap();
                    }

                }
            }
        });

        let state = AppState {
            broadcast_receiver,
            broadcast_sender,
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
                        state.serial_control_sender.send(WebsocketCmd::WsMessage(bytes)).unwrap()
                    }
                    Some(Ok(msg)) => todo!("Unknown WS Message: {:?}", msg)
                }
            }
        }
    }

    println!("Websocket context destroyed");
}
