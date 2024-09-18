use std::{
    collections::HashMap,
    thread,
    time::{Duration, Instant},
};

use audio::{write_audio, AudioDecoder, AudioResampler, OPUS_SAMPLE_RATE};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat, StreamConfig,
};
use itertools::Itertools;
use macroquad::{input::KeyCode, prelude::*};
use parser::{Operation, WaveOperation};
use ringbuf::{
    traits::{Producer, Split},
    HeapCons, HeapProd, HeapRb,
};

mod audio;
mod parser;

fn window_conf() -> Conf {
    Conf {
        window_title: "Remote M8 UI".to_owned(),
        // window_width: 320,
        // window_height: 240,
        ..Default::default()
    }
}

pub const M8_SCREEN_WIDTH: usize = 320;
pub const M8_SCREEN_HEIGHT: usize = 240;
pub const M8_ASPECT_RATIO: f32 = M8_SCREEN_WIDTH as f32 / M8_SCREEN_HEIGHT as f32;
// const M8_SCREEN_WIDTH: usize = 480;

const WAVE_HEIGHT: usize = 26;

struct State {
    pub operations: Vec<Operation>,
    pub waveform: Option<WaveOperation>,
    audio_producer: HeapProd<f32>,
    resampler: AudioResampler,
    decoder: AudioDecoder,
}

impl State {
    fn new(src_rate: usize, dest_rate: usize) -> (Self, HeapCons<f32>) {
        let (audio_producer, audio_receiver) = HeapRb::<f32>::new(dest_rate * 4).split();
        (
            Self {
                operations: Vec::new(),
                waveform: None,
                audio_producer,
                resampler: AudioResampler::new(src_rate, dest_rate),
                decoder: AudioDecoder::new(),
            },
            audio_receiver,
        )
    }

    fn queue_operation(&mut self, operaton: Operation) {
        self.operations.push(operaton)
    }

    fn clear_operations(&mut self) {
        self.operations.clear()
    }

    fn set_waveform(&mut self, waveform: WaveOperation) {
        self.waveform = Some(waveform)
    }

    fn enqueue_audio(&mut self, audio: &[u8]) {
        let decoded = self.decoder.decode(audio);
        let resampled = self.resampler.resample(decoded).collect_vec();

        self.audio_producer.push_slice(&resampled);
    }
}

struct InputProcessor {
    keymap: HashMap<KeyCode, usize>,
    key_state: u8,
}

impl InputProcessor {
    fn new() -> Self {
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

        Self {
            keymap,
            key_state: 0,
        }
    }

    fn process_key(
        &mut self,
        keycode: KeyCode,
        down: bool,
        websocket: &mut quad_net::web_socket::WebSocket,
    ) {
        if let Some(bit) = self.keymap.get(&keycode) {
            let new_state = match down {
                true => self.key_state | (1 << bit),
                false => self.key_state & !(1 << bit),
            };

            if new_state == self.key_state {
                return;
            }

            self.key_state = new_state;

            websocket.send_bytes(&[0x43, self.key_state]);
        }
    }
}

#[macroquad::main(window_conf)]
async fn main() {
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
    let dest_sample_rate = supported_config.sample_rate().0 as usize;
    let config: StreamConfig = supported_config.clone().into();

    let mut font57 = load_ttf_font("./m8stealth57.ttf").await.unwrap();
    font57.set_filter(FilterMode::Nearest);
    let mut font89 = load_ttf_font("./m8stealth89.ttf").await.unwrap();
    font89.set_filter(FilterMode::Nearest);

    let url = "ws://192.168.10.12:4000/ws".to_string();
    let mut websocket = quad_net::web_socket::WebSocket::connect(url).unwrap();

    let mut input_processor = InputProcessor::new();

    let render_target = render_target(M8_SCREEN_WIDTH as u32, M8_SCREEN_HEIGHT as u32);
    render_target.texture.set_filter(FilterMode::Nearest);
    let mut camera = Camera2D::from_display_rect(Rect::new(
        0.0,
        0.0,
        M8_SCREEN_WIDTH as f32,
        M8_SCREEN_HEIGHT as f32,
    ));
    camera.render_target = Some(render_target.clone());
    let waveform_texture = Texture2D::from_image(&Image::gen_image_color(
        M8_SCREEN_WIDTH as u16,
        WAVE_HEIGHT as u16,
        BLACK,
    ));
    waveform_texture.set_filter(FilterMode::Nearest);

    let mut last_screen_width = screen_width();
    let mut last_screen_height = screen_height();
    let mut parser = parser::Parser::new();

    let (mut state, mut audio_receiver) = State::new(OPUS_SAMPLE_RATE, dest_sample_rate);
    let mut write_buffer = [0f32; 2048];

    macro_rules! handle_sample {
        ($sample:ty) => {
            audio_device
                .build_output_stream(
                    &config,
                    move |data: &mut [$sample], _: &cpal::OutputCallbackInfo| {
                        write_audio::<$sample>(&mut audio_receiver, &mut write_buffer, data);
                    },
                    err_fn,
                    None,
                )
                .unwrap()
        };
    }

    let stream = match sample_format {
        SampleFormat::I8 => handle_sample!(i8),
        SampleFormat::I16 => handle_sample!(i16),
        SampleFormat::I32 => handle_sample!(i32),
        SampleFormat::I64 => handle_sample!(i64),
        SampleFormat::U8 => handle_sample!(u8),
        SampleFormat::U16 => handle_sample!(u16),
        SampleFormat::U32 => handle_sample!(u32),
        SampleFormat::U64 => handle_sample!(u64),
        SampleFormat::F32 => handle_sample!(f32),
        SampleFormat::F64 => handle_sample!(f64),
        sample_format => panic!("Unsupported sample format '{sample_format}'"),
    };
    stream.play().unwrap();

    let mut last_update = Instant::now();
    'runloop: loop {
        // println!("FPS: {}", get_fps());

        'stall: loop {
            // Websocket Data
            if websocket.connected() {
                while let Some(msg) = websocket.try_recv() {
                    parser.parse(&msg, &mut state);
                }
            }

            // Input
            for keycode in get_keys_pressed() {
                if keycode == KeyCode::Q {
                    break 'runloop;
                }
                // if keycode == KeyCode::P {
                //     // Pause to debug a frame
                //     thread::sleep(Duration::from_secs(10));
                // }
                input_processor.process_key(keycode, true, &mut websocket);
            }
            for keycode in get_keys_released() {
                input_processor.process_key(keycode, false, &mut websocket);
            }

            let now = Instant::now();
            if now - last_update >= Duration::from_millis(30) {
                last_update = now;
                break 'stall;
            }
            // next_frame().await;
            thread::sleep(Duration::from_millis(1));
        }

        // If screen size changed, tell m8 to redraw and clear the background
        if screen_height() != last_screen_height || screen_width() != last_screen_width {
            last_screen_height = screen_height();
            last_screen_width = screen_width();
            websocket.send_bytes(&[0x44]);
            websocket.send_bytes(&[0x45, 0x52]);
            clear_background(BLACK);
        }

        // Draw Texture
        set_camera(&camera);

        for operation in state.operations.drain(..) {
            match operation {
                parser::Operation::ClearBackground => clear_background(BLACK),
                parser::Operation::DrawRectangle(x, y, w, h, r, g, b) => {
                    draw_rectangle(x, y, w, h, Color::from_rgba(r, g, b, 255));
                }
                parser::Operation::DrawText(c, font, x, y, r, g, b) => {
                    let font = match font {
                        parser::Font::Font57 => &font57,
                        parser::Font::Font89 => &font89,
                    };
                    draw_text_ex(
                        &c.to_string(),
                        x,
                        y,
                        TextParams {
                            font: Some(font),
                            font_size: 16,
                            font_scale: 0.5,
                            font_scale_aspect: 1.0,
                            color: Color::from_rgba(r, g, b, 255),
                            ..Default::default()
                        },
                    );
                }
            }
        }

        match &state.waveform {
            None => (),
            Some(parser::WaveOperation::ClearWave) => {
                waveform_texture.update(&Image::gen_image_color(
                    M8_SCREEN_WIDTH as u16,
                    WAVE_HEIGHT as u16,
                    BLACK,
                ));
            }
            Some(parser::WaveOperation::DrawWave(points)) => {
                let mut image =
                    Image::gen_image_color(M8_SCREEN_WIDTH as u16, WAVE_HEIGHT as u16, BLACK);
                for (x, y, r, g, b) in points {
                    image.set_pixel(*x, *y, Color::from_rgba(*r, *g, *b, 255));
                }
                waveform_texture.update(&image);
            }
        }

        draw_texture_ex(
            &waveform_texture,
            0.0,
            0.0,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(M8_SCREEN_WIDTH as f32, WAVE_HEIGHT as f32)),
                flip_y: true,
                source: None,
                ..Default::default()
            },
        );

        // Draw Main
        set_default_camera();

        clear_background(BLACK);

        let (viewport_width, viewport_height) = match (screen_width(), screen_height()) {
            (width, height) if width >= height * M8_ASPECT_RATIO => {
                let width = (height * M8_ASPECT_RATIO).floor();
                (width, height)
            }
            (width, height) if width <= height * M8_ASPECT_RATIO => {
                let height = (width / M8_ASPECT_RATIO).floor();
                (width, height)
            }
            (_, _) => unreachable!(),
        };

        let screen_left = ((screen_width() - viewport_width) / 2.0).floor();
        let screen_top = ((screen_height() - viewport_height) / 2.0).floor();

        draw_texture_ex(
            &render_target.texture,
            screen_left,
            screen_top,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(viewport_width, viewport_height)),
                flip_y: true,
                source: None,
                ..Default::default()
            },
        );

        draw_text(
            &format!(
                "{} FPS - {}x{} ({}x{}) ",
                get_fps(),
                viewport_width,
                viewport_height,
                screen_width(),
                screen_height()
            ),
            10.0,
            screen_height() - 20.0,
            20.0,
            WHITE,
        );

        next_frame().await
    }
}
