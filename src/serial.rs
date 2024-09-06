use std::{
    borrow::Cow,
    thread::{self, JoinHandle},
    time::Duration,
};

use axum::extract::ws::{CloseFrame, Message};
use log::debug;
use serialport::{DataBits, FlowControl, Parity};

use crate::WebsocketCmd;

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

pub(crate) fn spawn(
    serial_path: String,
) -> (
    JoinHandle<()>,
    tokio::sync::mpsc::Receiver<Message>,
    std::sync::mpsc::Sender<WebsocketCmd>,
) {
    let (serial_sender, serial_receiver) = tokio::sync::mpsc::channel::<Message>(8);
    let (serial_control_sender, serial_control_receiver) =
        std::sync::mpsc::channel::<WebsocketCmd>();

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

                    let last_end_idx = work_buffer.iter().enumerate().rev().find_map(|(idx, e)| {
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

    (serial_handler, serial_receiver, serial_control_sender)
}
