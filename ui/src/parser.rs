const RECT_FRAME: u8 = 0xfe;
const TEXT_FRAME: u8 = 0xfd;
const WAVE_FRAME: u8 = 0xfc;
const SYSTEM_FRAME: u8 = 0xff;

const AUDIO_PACKET: u8 = b'A';
const SERIAL_PACKET: u8 = b'S';

const SLIP_FRAME_END: u8 = 0xc0;

pub enum Operation {
    ClearBackground,
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

    pub fn parse(&mut self, msg: &[u8]) -> Vec<Operation> {
        let t = msg[0];
        let rest = msg[1..];

        match msg[0] {
            [SERIAL_PACKET] => {
                let chunks = rest.split(|x| x == SLIP_FRAME_END);
                let mut operations = Vec::new();

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

                    let t = decoded[0];
                    let frame = decoded[1..];

                    match t {
                        [RECT_FRAME] => {
                            let x = frame[0] as f32 + frame[1] as f32 * 256f32;
                            let y = frame[2] as f32 + frame[3] as f32 * 256f32;
                            let mut w = 1.0f32;
                            let mut h = 1.0f32;
                            let mut r = last_r;
                            let mut g = last_g;
                            let mut b = last_b;

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

                            last_r = r;
                            last_g = g;
                            last_b = b;

                            if x == 0.0
                                && y == 0.0
                                && w >= M8_SCREEN_WIDTH as f32
                                && h >= M8_SCREEN_HEIGHT as f32
                            {
                                operations.push(operations::ClearBackground);
                            } else {
                                draw_rectangle(x, y, w, h, Color::from_rgba(r, g, b, 255));
                            }
                        }
                        [TEXT_FRAME] => {
                            let c = frame[0];
                            let x = frame[1] as f32 + frame[2] as f32 * 256f32;
                            let y = frame[3] as f32 + frame[4] as f32 * 256f32;
                            let foreground_r = frame[5];
                            let foreground_g = frame[6];
                            let foreground_b = frame[7];
                            let background_r = frame[8];
                            let background_g = frame[9];
                            let background_b = frame[10];

                            let font = match font_id {
                                0 => &font57,
                                1 => &font89,
                                _ => unimplemented!(),
                            };

                            let c = &[c];
                            let char = std::str::from_utf8(c).unwrap();

                            if (foreground_r, foreground_g, foreground_b)
                                != (background_r, background_g, background_b)
                            {
                                draw_rectangle(
                                    x,
                                    y + 11.0 - 10.0,
                                    8.0,
                                    11.0,
                                    Color::from_rgba(background_r, background_g, background_b, 255),
                                );
                            }

                            let (font_size, font_scale, font_aspect) = camera_font_scale(10.0);
                            draw_text_ex(
                                char,
                                x,
                                y + 11.0, // + 11?
                                TextParams {
                                    font: Some(font),
                                    font_size,
                                    font_scale,
                                    font_scale_aspect: font_aspect,
                                    color: Color::from_rgba(
                                        foreground_r,
                                        foreground_g,
                                        foreground_b,
                                        255,
                                    ),
                                    ..Default::default()
                                },
                            );
                        }
                        [WAVE_FRAME] => {
                            let (color, data) = frame.split_at(3);
                            let r = color[0];
                            let g = color[1];
                            let b = color[2];
                            if data.is_empty() {
                                waveform = None;
                            } else {
                                let mut image = Image::gen_image_color(
                                    M8_SCREEN_WIDTH as u16,
                                    WAVE_HEIGHT as u16,
                                    BLACK,
                                );
                                for (idx, y) in data.iter().enumerate() {
                                    // if y == &255 {
                                    //     continue;
                                    // }
                                    image.set_pixel(
                                        idx as u32,
                                        (*y as u32).min(WAVE_HEIGHT as u32 - 1),
                                        Color::from_rgba(r, g, b, 255),
                                    );
                                }
                                let texture = Texture2D::from_image(&image);
                                texture.set_filter(FilterMode::Linear);
                                waveform = Some(texture);
                            }
                        }
                        [SYSTEM_FRAME] => {
                            font_id = frame[4];
                        }
                        _ => (),
                    }
                }
            }
            [AUDIO_PACKET] => {
                audio_sender.send(rest.to_vec()).unwrap();
            }
            _ => todo!(),
        }
    }
}
