use std::{
    collections::{HashMap, VecDeque},
    sync::mpsc::Receiver,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, StreamConfig,
};
use itertools::interleave;
use macroquad::{input::KeyCode, prelude::*};
use rubato::VecResampler;

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

const OPUS_CHUNK_SIZE: usize = 960;
const NUM_CHANNELS: usize = 2;
const SAMPLE_RATE: usize = 48000;

struct Resampler<T> {
    resampler: rubato::FftFixedInOut<f32>,
    src_rate: usize,
    dest_rate: usize,
    input_buffers: Vec<Vec<f32>>,
    output_buffers: Vec<Vec<f32>>,
    output_buffer: VecDeque<T>,
}

impl<T> Resampler<T>
where
    T: Sample + FromSample<f32>,
{
    fn new(src_rate: usize, dest_rate: usize) -> Self {
        // Math here is flipped since we don't actually want OPUS_CHUNK_SIZED INPUT, we want
        // OPUS_CHUNK_SIZED output.
        let sample_ratio = dest_rate as f32 / src_rate as f32;
        let in_chunk_size = (OPUS_CHUNK_SIZE as f32 * sample_ratio) as usize;
        let resampler =
            rubato::FftFixedInOut::<f32>::new(src_rate, dest_rate, in_chunk_size, NUM_CHANNELS)
                .unwrap();
        let input_buffers = resampler.input_buffer_allocate(true);
        let output_buffers = resampler.output_buffer_allocate(true);

        Self {
            resampler,
            src_rate,
            dest_rate,
            input_buffers,
            output_buffers,
            output_buffer: VecDeque::new(),
        }
    }

    fn extend(&mut self, samples: &[f32]) {
        if self.src_rate == self.dest_rate {
            self.output_buffer
                .extend(samples.iter().map(|v| T::from_sample(*v)));
            return;
        }

        // Split left + right
        let mut i = 0;
        for (x, v) in samples.iter().enumerate() {
            if x % 2 == 0 {
                self.input_buffers[0][i] = *v;
            } else {
                self.input_buffers[1][i] = *v;
                i += 1;
            }
        }

        let (_in_len, out_len_per_channel) = self
            .resampler
            .process_into_buffer(&self.input_buffers, &mut self.output_buffers, None)
            .unwrap();

        self.output_buffer.extend(
            interleave(
                &self.output_buffers[0][..out_len_per_channel],
                &self.output_buffers[1][..out_len_per_channel],
            )
            .map(|x| T::from_sample(*x)),
        );
    }

    fn drain(&mut self, n: usize) -> impl Iterator<Item = T> + '_ {
        self.output_buffer
            .drain(..(n.min(self.output_buffer.len())))
    }
}

fn write_audio<T: Sample + FromSample<f32>>(
    audio_receiver: &mut Receiver<Vec<u8>>,
    resampler: &mut Resampler<T>,
    decoder: &mut opus::Decoder,
    decode_buffer: &mut [f32; OPUS_CHUNK_SIZE * NUM_CHANNELS],
    data: &mut [T],
) {
    let mut idx = 0;
    while idx < data.len() {
        let resampled = resampler.drain(data.len() - idx);
        for s in resampled {
            data[idx] = s;
            idx += 1;
        }

        if idx < data.len() {
            let packet = audio_receiver.recv().unwrap();
            let n = decoder.decode_float(&packet, decode_buffer, false).unwrap();
            resampler.extend(&decode_buffer[..n * NUM_CHANNELS]);
        }
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let mut decode_buffer = [0f32; OPUS_CHUNK_SIZE * NUM_CHANNELS];
    let (audio_sender, mut audio_receiver) = std::sync::mpsc::channel::<Vec<u8>>();

    let host = cpal::default_host();
    let audio_device = host
        .default_output_device()
        .expect("no output device available");
    let mut supported_configs_range = audio_device
        .supported_output_configs()
        .expect("error while querying configs");
    let supported_config = supported_configs_range
        .next()
        .expect("no supported config?!")
        .with_max_sample_rate();
    println!(
        "Device: {:?}, config: {:?}",
        audio_device.name(),
        supported_config
    );
    let err_fn = |err| eprintln!("an error occurred on the output audio stream: {}", err);
    let sample_format = supported_config.sample_format();
    let config: StreamConfig = supported_config.clone().into();

    let mut font57 = load_ttf_font("./m8stealth57.ttf").await.unwrap();
    font57.set_filter(FilterMode::Linear);
    let mut font89 = load_ttf_font("./m8stealth89.ttf").await.unwrap();
    font89.set_filter(FilterMode::Linear);
    let mut decoder =
        opus::Decoder::new(48000, opus::Channels::Stereo).expect("Couldn't create opus decoder");

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

    let mut last_screen_width = screen_width();
    let mut last_screen_height = screen_height();

    macro_rules! handle_sample {
        ($sample:ty) => {{
            let sample_rate = supported_config.sample_rate().0 as usize;
            let mut resampler = Resampler::<$sample>::new(SAMPLE_RATE, sample_rate);

            let stream = audio_device
                .build_output_stream(
                    &config,
                    move |data: &mut [$sample], _: &cpal::OutputCallbackInfo| {
                        write_audio::<$sample>(
                            &mut audio_receiver,
                            &mut resampler,
                            &mut decoder,
                            &mut decode_buffer,
                            data,
                        );
                    },
                    err_fn,
                    None,
                )
                .unwrap();

            stream.play().unwrap();

            'runloop: loop {
                // println!("FPS: {}", get_fps());
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
                                                draw_rectangle(
                                                    x,
                                                    y,
                                                    w,
                                                    h,
                                                    Color::from_rgba(r, g, b, 255),
                                                );
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
                                                    y + 11.0 - 10.0, // ?
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

                                            let (font_size, font_scale, font_aspect) =
                                                camera_font_scale(10.0);
                                            draw_text_ex(
                                                char,
                                                x,
                                                y + 11.0, // + 11?
                                                TextParams {
                                                    font: Some(font),
                                                    // font_size: 10,
                                                    // font_scale: 1.0,
                                                    // font_scale_aspect: 1.0,
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
                            [AUDIO_PACKET] => {
                                audio_sender.send(rest.to_vec()).unwrap();
                            }
                            _ => todo!(),
                        }
                    }
                }

                if screen_height() != last_screen_height {
                    last_screen_height = screen_height();
                    websocket.send_bytes(&[0x44]);
                    websocket.send_bytes(&[0x45, 0x52]);
                    clear_background(BLACK);
                }
                if screen_width() != last_screen_width {
                    last_screen_width = screen_width();
                    websocket.send_bytes(&[0x44]);
                    websocket.send_bytes(&[0x45, 0x52]);
                    clear_background(BLACK);
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
        }};
    }

    match sample_format {
        SampleFormat::F32 => handle_sample!(f32),
        SampleFormat::I16 => handle_sample!(i16),
        SampleFormat::U16 => handle_sample!(u16),
        sample_format => panic!("Unsupported sample format '{sample_format}'"),
    }
}
