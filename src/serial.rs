use std::{io::Write, time::Duration};

use anyhow::Result;
use futures::StreamExt;
use log::debug;
use tokio_serial::{DataBits, FlowControl, Parity, SerialPortBuilderExt, SerialStream, StopBits};
use tokio_util::{
    bytes::BytesMut,
    codec::{Decoder, Encoder},
};

use crate::WebsocketCmd;

mod serial_cmd {
    pub const DISCONNECT: u8 = 0x44;
    pub const ENABLE: u8 = 0x45;
    pub const RESET: u8 = 0x52;
}

mod slip {
    pub const END: u8 = 0xc0;
}

pub struct SLIPCodec {}

impl Decoder for SLIPCodec {
    type Item = Vec<u8>;

    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(idx) = src.as_ref().iter().rev().position(|b| *b == slip::END) {
            let packet = src.split_to(src.len() - idx);
            return Ok(Some(packet.to_vec()));
        }

        Ok(None)
    }
}

impl Encoder<WebsocketCmd> for SLIPCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: WebsocketCmd, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            WebsocketCmd::Connect => {
                dst.extend_from_slice(&[serial_cmd::ENABLE, serial_cmd::RESET]);
            }
            WebsocketCmd::WsMessage(data) => {
                dst.extend_from_slice(&data);
            }
        }
        Ok(())
    }
}

pub struct Serial {
    stream: SerialStream,
}

impl Serial {
    pub fn new(path: String) -> Result<Self> {
        let mut serial_port = tokio_serial::new(path, 115200)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .flow_control(FlowControl::None)
            .timeout(Duration::from_millis(10))
            .open_native_async()
            .unwrap();
        serial_port.write_all(&[serial_cmd::DISCONNECT])?;
        std::thread::sleep(Duration::from_millis(50));
        serial_port.write_all(&[serial_cmd::ENABLE, serial_cmd::RESET])?;
        debug!("Serial Init");

        Ok(Self {
            stream: serial_port,
        })
    }

    pub fn stream(
        self,
    ) -> (
        impl futures::sink::Sink<WebsocketCmd, Error = std::io::Error>,
        impl futures::stream::Stream<Item = Result<Vec<u8>, std::io::Error>>,
    ) {
        let serial_codec = SLIPCodec {}.framed(self.stream);
        serial_codec.split()
    }
}
