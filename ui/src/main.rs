use std::collections::HashMap;

use macroquad::{input::KeyCode, prelude::*};

fn window_conf() -> Conf {
    Conf {
        window_title: "Remote M8 UI".to_owned(),
        // window_width: 320,
        // window_height: 240,
        ..Default::default()
    }
}

const M8_SCREEN_WIDTH: usize = 320;
const M8_SCREEN_HEIGHT: usize = 240;
const M8_ASPECT_RATIO: f32 = M8_SCREEN_WIDTH as f32 / M8_SCREEN_HEIGHT as f32;
// const M8_SCREEN_WIDTH: usize = 480;
//

const RECT_FRAME: u8 = 0xfe;
const TEXT_FRAME: u8 = 0xfd;
const WAVE_FRAME: u8 = 0xfc;
const SYSTEM_FRAME: u8 = 0xff;

const AUDIO_PACKET: u8 = b'A';
const SERIAL_PACKET: u8 = b'S';

const SLIP_FRAME_END: u8 = 0xc0;

#[macroquad::main(window_conf)]
async fn main() {
    let font57 = load_ttf_font("./m8stealth57.ttf").await.unwrap();
    let font89 = load_ttf_font("./m8stealth89.ttf").await.unwrap();

    let url = "ws://192.168.10.12:4000/ws".to_string();
    let mut websocket = quad_net::web_socket::WebSocket::connect(url).unwrap();

    let mut last_r = 0;
    let mut last_g = 0;
    let mut last_b = 0;
    let mut font_id = 0;
    let mut keystate = 0;

    let keymap = HashMap::from([
        (KeyCode::Up, 6),
        (KeyCode::Down, 5),
        (KeyCode::Left, 7),
        (KeyCode::Right, 2),
        (KeyCode::LeftShift, 4),
        (KeyCode::Space, 3),
        (KeyCode::Z, 1),
        (KeyCode::X, 0),
    ]);

    let render_target = render_target(M8_SCREEN_WIDTH as u32, M8_SCREEN_HEIGHT as u32);
    render_target.texture.set_filter(FilterMode::Nearest);
    let mut camera = Camera2D::from_display_rect(Rect::new(
        0.0,
        0.0,
        M8_SCREEN_WIDTH as f32,
        M8_SCREEN_HEIGHT as f32,
    ));
    camera.render_target = Some(render_target.clone());

    'runloop: loop {
        if websocket.connected() {
            while let Some(msg) = websocket.try_recv().and_then(|x| {
                if !x.is_empty() {
                    return Some(x);
                }
                None
            }) {
                let (t, rest) = msg.split_at(1);
                match t {
                    [SERIAL_PACKET] => {
                        let chunks = rest.split(|x| x == &0xc0);

                        for chunk in chunks {
                            if chunk.is_empty() {
                                continue;
                            }

                            set_camera(&camera);

                            // This slip library is weird and doesn't
                            // parse anything until after two end
                            // frames
                            let mut tmp = vec![SLIP_FRAME_END, SLIP_FRAME_END];
                            tmp.extend_from_slice(chunk);
                            tmp.push(SLIP_FRAME_END);
                            let decoded = simple_slip::decode(&tmp).unwrap();

                            let (t, frame) = decoded.split_at(1);

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
                                        clear_background(BLACK);
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
                                            y + 11.0 - 9.0,
                                            8.0,
                                            11.0,
                                            Color::from_rgba(
                                                background_r,
                                                background_g,
                                                background_b,
                                                255,
                                            ),
                                        );
                                    }

                                    draw_text_ex(
                                        char,
                                        x,
                                        y + 11.0, // + 11?
                                        TextParams {
                                            font: Some(font),
                                            font_size: 10,
                                            font_scale: 1.0,
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
                                    let height = (24f32 / 320f32) * M8_SCREEN_WIDTH as f32;
                                    draw_rectangle(
                                        0.0,
                                        0.0,
                                        M8_SCREEN_WIDTH as f32,
                                        height,
                                        Color::from_rgba(0, 0, 0, 255),
                                    );

                                    for (idx, y) in data.iter().enumerate() {
                                        // if y == &255 {
                                        //     continue;
                                        // }
                                        draw_rectangle(
                                            idx as f32,
                                            *y.min(&20) as f32,
                                            1.0,
                                            1.0,
                                            Color::from_rgba(r, g, b, 255),
                                        );
                                    }
                                }
                                [SYSTEM_FRAME] => {
                                    font_id = frame[4];
                                }
                                _ => (),
                            }
                        }
                    }
                    [AUDIO_PACKET] => {}
                    _ => todo!(),
                }
            }
        }

        set_default_camera();

        clear_background(BLACK);

        let mut process_key = |key_code: KeyCode, down: bool| {
            if let Some(bit) = keymap.get(&key_code) {
                let new_state = match down {
                    true => keystate | (1 << bit),
                    false => keystate & !(1 << bit),
                };

                if new_state == keystate {
                    return;
                }

                keystate = new_state;

                websocket.send_bytes(&[0x43, keystate]);
            }
        };

        for keycode in get_keys_pressed() {
            if keycode == KeyCode::Q {
                break 'runloop;
            }
            process_key(keycode, true);
        }
        for keycode in get_keys_released() {
            process_key(keycode, false);
        }

        let (width, height) = match (screen_width(), screen_height()) {
            (width, height) if width >= height * M8_ASPECT_RATIO => {
                (height * M8_ASPECT_RATIO, height)
            }
            (width, height) if width <= height * M8_ASPECT_RATIO => {
                (width, width / M8_ASPECT_RATIO)
            }
            (_, _) => unreachable!(),
        };

        draw_texture_ex(
            &render_target.texture,
            (screen_width() - width) / 2.0,
            (screen_height() - height) / 2.0,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(width, height)),
                flip_y: true,
                source: None,
                ..Default::default()
            },
        );

        next_frame().await
    }
}
