use crate::{M8_SCREEN_HEIGHT, M8_SCREEN_WIDTH, WAVE_HEIGHT};

const RECT_FRAME: u8 = 0xfe;
const TEXT_FRAME: u8 = 0xfd;
const WAVE_FRAME: u8 = 0xfc;
const SYSTEM_FRAME: u8 = 0xff;

const AUDIO_PACKET: u8 = b'A';
const SERIAL_PACKET: u8 = b'S';

const SLIP_FRAME_END: u8 = 0xc0;

pub enum Operation {
    ClearBackground,
    DrawRectangle(f32, f32, f32, f32, u8, u8, u8),
    DrawText(char, Font, f32, f32, u8, u8, u8),
}

pub enum WaveOperation {
    DrawWave(Vec<(u32, u32, u8, u8, u8)>),
    ClearWave,
}

pub enum Font {
    Font57,
    Font89,
}

pub struct Parser {
    last_r: u8,
    last_g: u8,
    last_b: u8,
    font_id: u8,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            last_r: 0,
            last_g: 0,
            last_b: 0,
            font_id: 0,
        }
    }

    pub fn parse<'a>(
        &mut self,
        msg: &'a [u8],
    ) -> (Vec<Operation>, Option<WaveOperation>, Vec<&'a [u8]>) {
        let mut operations = Vec::new();
        let mut audio = Vec::new();
        let mut wave_operation = None;
        let rest = &msg[1..];

        match msg[0] {
            SERIAL_PACKET => {
                let chunks = rest.split(|x| x == &SLIP_FRAME_END);

                for chunk in chunks {
                    if chunk.is_empty() {
                        continue;
                    }

                    // This slip library is weird and doesn't
                    // parse anything until after two end
                    // frames
                    let mut tmp = vec![SLIP_FRAME_END, SLIP_FRAME_END];
                    tmp.extend_from_slice(chunk);
                    tmp.push(SLIP_FRAME_END);
                    let decoded = simple_slip::decode(&tmp).unwrap();

                    let frame = &decoded[1..];

                    match decoded[0] {
                        RECT_FRAME => {
                            let x = frame[0] as f32 + frame[1] as f32 * 256f32;
                            let y = frame[2] as f32 + frame[3] as f32 * 256f32;
                            let mut w = 1.0f32;
                            let mut h = 1.0f32;
                            let mut r = self.last_r;
                            let mut g = self.last_g;
                            let mut b = self.last_b;

                            match frame.len() {
                                11 => {
                                    w = frame[4] as f32 + frame[5] as f32 * 256f32;
                                    h = frame[6] as f32 + frame[7] as f32 * 256f32;
                                    r = frame[8];
                                    g = frame[9];
                                    b = frame[10];
                                }
                                8 => {
                                    w = frame[4] as f32 + frame[5] as f32 * 256f32;
                                    h = frame[6] as f32 + frame[7] as f32 * 256f32;
                                }
                                7 => {
                                    r = frame[4];
                                    g = frame[5];
                                    b = frame[6];
                                }
                                5 => {
                                    w = 1f32;
                                    h = 1f32;
                                }
                                _ => (),
                            }

                            self.last_r = r;
                            self.last_g = g;
                            self.last_b = b;

                            if x == 0.0
                                && y == 0.0
                                && w >= M8_SCREEN_WIDTH as f32
                                && h >= M8_SCREEN_HEIGHT as f32
                            {
                                operations.clear();
                                operations.push(Operation::ClearBackground);
                            } else {
                                operations.push(Operation::DrawRectangle(x, y, w, h, r, g, b));
                            }
                        }
                        TEXT_FRAME => {
                            let c = frame[0];
                            let x = frame[1] as f32 + frame[2] as f32 * 256f32;
                            let y = frame[3] as f32 + frame[4] as f32 * 256f32;
                            let foreground_r = frame[5];
                            let foreground_g = frame[6];
                            let foreground_b = frame[7];
                            let background_r = frame[8];
                            let background_g = frame[9];
                            let background_b = frame[10];

                            if (foreground_r, foreground_g, foreground_b)
                                != (background_r, background_g, background_b)
                            {
                                operations.push(Operation::DrawRectangle(
                                    x,
                                    y + 1f32,
                                    8.0,
                                    11.0,
                                    background_r,
                                    background_g,
                                    background_b,
                                ));
                            }

                            let font = match self.font_id {
                                0 => Font::Font57,
                                1 => Font::Font89,
                                _ => unimplemented!(),
                            };

                            operations.push(Operation::DrawText(
                                c as char,
                                font,
                                x,
                                y + 11.0,
                                foreground_r,
                                foreground_g,
                                foreground_b,
                            ));
                        }
                        WAVE_FRAME => {
                            let (color, data) = frame.split_at(3);
                            let r = color[0];
                            let g = color[1];
                            let b = color[2];
                            if data.is_empty() {
                                wave_operation = Some(WaveOperation::ClearWave)
                                // operations.push(Operation::ClearWave);
                            } else {
                                let points = data
                                    .iter()
                                    .enumerate()
                                    .map(|(idx, y)| {
                                        (
                                            idx as u32,
                                            (*y as u32).min(WAVE_HEIGHT as u32 - 1),
                                            r,
                                            g,
                                            b,
                                        )
                                    })
                                    .collect::<Vec<_>>();

                                // operations.push(Operation::DrawWave(points));
                                wave_operation = Some(WaveOperation::DrawWave(points));
                            }
                        }
                        SYSTEM_FRAME => {
                            self.font_id = frame[4];
                        }
                        _ => (),
                    }
                }
            }
            AUDIO_PACKET => {
                audio.push(rest);
            }
            _ => todo!(),
        }

        (operations, wave_operation, audio)
    }
}
