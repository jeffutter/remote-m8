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

const OPUS_CHUNK_SIZE: usize = 960;
const NUM_CHANNELS: usize = 2;
const SAMPLE_RATE: usize = 48000;

const WAVE_HEIGHT: usize = 26;

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
    render_target.texture.set_filter(FilterMode::Linear);
    let mut camera = Camera2D::from_display_rect(Rect::new(
        0.0,
        0.0,
        M8_SCREEN_WIDTH as f32,
        M8_SCREEN_HEIGHT as f32,
    ));
    camera.render_target = Some(render_target.clone());
    let mut waveform: Option<Texture2D> = None;

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
                    while let Some(msg) = websocket.try_recv() {
                        for operation in parser::parse(msg) {
                            match operation {}
                        }
                    }
                }

                if screen_height() != last_screen_height || screen_width() != last_screen_width {
                    last_screen_height = screen_height();
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

                if let Some(waveform) = &waveform {
                    let x = (screen_width() - width) / 2.0;
                    let y = (screen_height() - height) / 2.0;
                    let height = (width / 320.0) * WAVE_HEIGHT as f32;

                    draw_texture_ex(
                        waveform,
                        x,
                        y,
                        WHITE,
                        DrawTextureParams {
                            dest_size: Some(vec2(width, height)),
                            flip_y: true,
                            source: None,
                            ..Default::default()
                        },
                    );
                }

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
