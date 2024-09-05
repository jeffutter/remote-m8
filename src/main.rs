use core::panic;
use std::{borrow::Cow, io::Write, thread, time::Duration};

use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use clap::Parser;
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, SizedSample, Stream,
};
use flate2::{write::ZlibEncoder, Compression};
use futures::{SinkExt, StreamExt};
use log::{debug, info};
use rust_embed::Embed;
use serialport::{DataBits, FlowControl, Parity};
use tokio::sync::{
    broadcast::{Receiver, Sender},
    mpsc,
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
            serial_control_sender: self.serial_control_sender.clone(),
        }
    }
}

// const BAUD_RATE: u32 = 9600; // 115200
const BAUD_RATE: u32 = 115200;

mod serial_cmd {
    pub const DISCONNECT: u8 = 0x44;
    pub const ENABLE: u8 = 0x45;
    pub const RESET: u8 = 0x52;
}

mod slip {
    pub const END: u8 = 0xc0;
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
            info!("{} - {:?}", p.port_name, p.port_type);
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
            for device in host.input_devices().into_iter() {
                for y in device {
                    info!("Audio Device: {:?}", y.name());
                }
            }

            let input_device = host
                .input_devices()
                .into_iter()
                .find_map(|mut d| {
                    d.find(|x| {
                        x.name().is_ok_and(|name| {
                            #[cfg(target_os = "macos")]
                            return name == "M8";
                            #[cfg(target_os = "linux")]
                            return name == "iec958:CARD=M8,DEV=0";
                        })
                    })
                })
                .expect("Couldn't find M8 Audio Device");

            #[cfg(target_os = "macos")]
            let config = SupportedStreamConfig::new(
                2,
                SampleRate(44100),
                cpal::SupportedBufferSize::Range { min: 4, max: 4096 },
                cpal::SampleFormat::F32,
            );

            #[cfg(target_os = "linux")]
            let config = input_device
                .default_input_config()
                .expect("Could not create default config");

            debug!("Input config: {:?}", config);

            let stream = match config.sample_format() {
                SampleFormat::I8 => run::<i8>(&input_device, &config.into(), audio_sender),
                SampleFormat::I16 => run::<i16>(&input_device, &config.into(), audio_sender),
                SampleFormat::I32 => run::<i32>(&input_device, &config.into(), audio_sender),
                SampleFormat::I64 => run::<i64>(&input_device, &config.into(), audio_sender),
                SampleFormat::U8 => run::<u8>(&input_device, &config.into(), audio_sender),
                SampleFormat::U16 => run::<u16>(&input_device, &config.into(), audio_sender),
                SampleFormat::U32 => run::<u32>(&input_device, &config.into(), audio_sender),
                SampleFormat::U64 => run::<u64>(&input_device, &config.into(), audio_sender),
                SampleFormat::F32 => run::<f32>(&input_device, &config.into(), audio_sender),
                SampleFormat::F64 => run::<f64>(&input_device, &config.into(), audio_sender),
                sample_format => panic!("Unsupported sample format '{sample_format}'"),
            }
            .unwrap();

            info!("Starting Audio Stream");

            stream.play().unwrap();

            thread::park();
            info!("Audio Stream Done");
        });

        let serial_handler = thread::spawn(move || {
            let mut sp = serialport::new(serial_path.clone(), BAUD_RATE)
                .data_bits(DataBits::Eight)
                .parity(Parity::None)
                .stop_bits(serialport::StopBits::One)
                .flow_control(FlowControl::None)
                .timeout(Duration::from_millis(10))
                .open()
                .unwrap();

            sp.write_all(&[serial_cmd::DISCONNECT]).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            sp.write_all(&[serial_cmd::ENABLE, serial_cmd::RESET])
                .unwrap();
            debug!("Serial Init");

            let mut buffer = [0; 1024];
            // let mut buffer = [0; 4096];
            let mut work_buffer: Vec<u8> = Vec::new();
            loop {
                match sp.read(&mut buffer[..]) {
                    Err(e) => match e.kind() {
                        std::io::ErrorKind::TimedOut => (),
                        std::io::ErrorKind::Interrupted => (),
                        _ => panic!("Unknown Serial Error: {}", e),
                    },
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

                        let last_end_idx =
                            work_buffer.iter().enumerate().rev().find_map(|(idx, e)| {
                                if *e == slip::END {
                                    return Some(idx);
                                }
                                None
                            });

                        if let Some(idx) = last_end_idx {
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

                match serial_control_receiver.recv_timeout(Duration::from_millis(10)) {
                    Ok(WebsocketCmd::Connect) => {
                        sp.write_all(&[serial_cmd::ENABLE, serial_cmd::RESET])
                            .unwrap();
                    }
                    Ok(WebsocketCmd::WsMessage(msg)) => match sp.write_all(&msg) {
                        Ok(()) => (),
                        Err(e) => match e.kind() {
                            std::io::ErrorKind::TimedOut => {
                                panic!("Timed out writing message: {:?}", msg)
                            }
                            _ => panic!("Couldn't write message: {:?}. Error: {:?}", msg, e),
                        },
                    },
                    Err(_read_timeout) => {
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
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(msg)) => todo!("Unknown WS Message: {:?}", msg)
                }
            }
        }
    }

    println!("Websocket context destroyed");
}

pub fn run<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    audio_sender: mpsc::Sender<Message>,
) -> Result<Stream, anyhow::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let err_fn = |err| eprintln!("an error occurred on stream: {}", err);

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| write_data(data, audio_sender.clone()),
        err_fn,
        None,
    )?;

    Ok(stream)
}

fn write_data<T>(data: &[T], audio_sender: mpsc::Sender<Message>)
where
    T: Sample,
    f32: Sample + FromSample<T>,
{
    let (prefix, aligned, suffix) = unsafe { data.align_to::<u128>() };
    if prefix.iter().all(|&x| f32::from_sample(x) == 0.0)
        && suffix.iter().all(|&x| f32::from_sample(x) == 0.0)
        && aligned.iter().all(|&x| x == 0)
    {
        return;
    }

    let data = data
        .iter()
        .map(|x| f32::from_sample(*x))
        .collect::<Vec<f32>>();

    let u8_data = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4) };
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(u8_data).unwrap();
    let encoded = encoder.finish().unwrap();

    let mut vec_data: Vec<u8> = vec![b'A'];
    vec_data.extend_from_slice(&encoded);

    audio_sender
        .blocking_send(Message::Binary(vec_data))
        .unwrap();
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
