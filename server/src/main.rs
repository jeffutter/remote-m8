mod audio;
mod serial;
mod server;

use core::panic;
use std::{pin::Pin, sync::Arc};

use axum::extract::ws::Message;
use clap::Parser;
use futures::StreamExt;
use log::info;
use tokio::sync::{
    broadcast::{Receiver, Sender},
    Mutex,
};
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

type WebsocketCmdStream =
    Pin<Box<dyn futures::sink::Sink<WebsocketCmd, Error = std::io::Error> + Send>>;

struct AppState {
    broadcast_receiver: Receiver<Message>,
    broadcast_sender: Sender<Message>,
    serial_sink: Arc<Mutex<WebsocketCmdStream>>,
}

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
        let (serial_sink, serial_stream) = serial::Serial::new(serial_path).unwrap().stream();
        let audio_receiver = audio::run_audio();

        let mut encoder =
            opus::Encoder::new(48000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut audio_receiver = audio_receiver.map(move |data| {
            let encoded = encoder.encode_vec_float(&data, 960).unwrap();

            let mut vec_data: Vec<u8> = vec![b'A'];
            vec_data.extend_from_slice(&encoded);
            vec_data
        });

        let mut serial_stream = serial_stream.map(|data| {
            data.map(|data| {
                let mut prefix_data = vec![b'S'];
                prefix_data.extend_from_slice(&data);
                prefix_data
            })
        });

        let broadcast_sender1 = broadcast_sender.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(data) = serial_stream.next() => {
                        match data {
                            Ok(data) => {
                                broadcast_sender1.send(Message::Binary(data)).unwrap();
                            },
                            Err(_) => todo!(),
                        }
                    }
                    Some(data) = audio_receiver.next() => {
                        broadcast_sender1.send(Message::Binary(data)).unwrap();
                    }

                }
            }
        });

        let state = AppState {
            broadcast_receiver,
            broadcast_sender,
            serial_sink: Arc::new(Mutex::new(Box::pin(serial_sink))),
        };

        server::run(state, args.port).await.unwrap();
    }
}
