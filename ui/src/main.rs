use std::collections::HashMap;

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, StreamConfig,
};
use itertools::Itertools;
use macroquad::{input::KeyCode, prelude::*};
use parser::{Operation, WaveOperation};
use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapCons, HeapProd, HeapRb,
};
use rubato::Resampler;

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

const OPUS_CHUNK_SIZE: usize = 960;
const NUM_CHANNELS: usize = 2;
const SAMPLE_RATE: usize = 48000;

const WAVE_HEIGHT: usize = 26;

struct AudioResampler {
    resampler: rubato::FftFixedOut<f32>,
    src_rate: usize,
    dest_rate: usize,
    input_buffers: Vec<Vec<f32>>,
    output_buffers: Vec<Vec<f32>>,
    output_buffer: Vec<f32>,
}

impl AudioResampler {
    fn new(src_rate: usize, dest_rate: usize) -> Self {
        // Math here is flipped since we don't actually want OPUS_CHUNK_SIZED INPUT, we want
        // OPUS_CHUNK_SIZED output.
        // let sample_ratio = dest_rate as f32 / src_rate as f32;
        // let in_chunk_size = (OPUS_CHUNK_SIZE as f32 * sample_ratio) as usize;

        // let out_chunk_size = dest_rate / 120; // 2ms
        let out_chunk_size = dest_rate / 10;
        // let out_chunk_size = dest_rate;
        println!("Out Chunk Size: {}", out_chunk_size);
        let resampler =
            rubato::FftFixedOut::<f32>::new(src_rate, dest_rate, out_chunk_size, 2, NUM_CHANNELS)
                .unwrap();
        // let input_buffers = resampler.input_buffer_allocate(true);
        // let output_buffers = resampler.output_buffer_allocate(true);
        let input_buffers = (0..NUM_CHANNELS)
            .map(|_| vec![0f32; dest_rate])
            .collect_vec();
        let output_buffers = (0..NUM_CHANNELS)
            .map(|_| vec![0f32; dest_rate])
            .collect_vec();

        Self {
            resampler,
            src_rate,
            dest_rate,
            input_buffers,
            output_buffers,
            output_buffer: vec![0f32; dest_rate * NUM_CHANNELS],
        }
    }

    fn ratio(&self) -> f32 {
        self.src_rate as f32 / self.dest_rate as f32
    }

    fn input_frames_next(&self) -> usize {
        self.resampler.input_frames_next()
    }

    fn resample(&mut self, samples: &[f32]) -> Box<dyn Iterator<Item = f32> + '_> {
        println!("In Samples: {}", samples.len());
        // self.output_buffer.clear();
        self.output_buffer = vec![0f32; self.dest_rate * NUM_CHANNELS];

        if self.src_rate == self.dest_rate {
            self.output_buffer.extend_from_slice(samples);
            return Box::new(self.output_buffer.drain(..));
        }

        // Split left + right
        // SIMD Optimized deinterleave
        let (lbuffs, rbuffs) = self.input_buffers.split_at_mut(1);
        let lbuff = &mut lbuffs[0][..];
        let rbuff = &mut rbuffs[0][..];

        for ((l, r), src) in lbuff
            .iter_mut()
            .zip(rbuff.iter_mut())
            .zip(samples.chunks_exact(2))
        {
            *l = src[0];
            *r = src[1];
        }

        println!(
            "Input Buffers: {} - Out Buffers: {}, {}",
            samples.len() / 2,
            self.output_buffers[0].len(),
            self.output_buffers[1].len()
        );

        println!(
            "Input Frames Next: {} / {}",
            self.resampler.input_frames_next(),
            self.resampler.input_frames_max()
        );
        println!(
            "Output Frames Next: {} / {}",
            self.resampler.output_frames_next(),
            self.resampler.output_frames_max()
        );

        let (_in_len, out_len_per_channel) = self
            .resampler
            .process_into_buffer(
                &[
                    &self.input_buffers[0][..samples.len() / 2],
                    &self.input_buffers[1][..samples.len() / 2],
                ],
                &mut self.output_buffers,
                None,
            )
            .unwrap();

        //TODO: More than 2 channels?
        //Can probably just hardcode everything to 2
        // SIMD Optimized interleave
        for (dest, (l, r)) in self
            .output_buffer
            .chunks_exact_mut(2)
            .take(out_len_per_channel * NUM_CHANNELS)
            .zip(
                self.output_buffers[0]
                    .iter()
                    .zip(self.output_buffers[1].iter()),
            )
        {
            dest[0] = *l;
            dest[1] = *r;
        }

        // &self.output_buffer[..(out_len_per_channel * NUM_CHANNELS)]
        println!("DRAIN: {}", out_len_per_channel * NUM_CHANNELS);
        Box::new(
            self.output_buffer
                .drain(..(out_len_per_channel * NUM_CHANNELS)),
        )
    }
}

fn write_audio<T: Sample + FromSample<f32>>(
    audio_receiver: &mut HeapCons<f32>,
    resampler: &mut AudioResampler,
    resampled_buffer: &mut HeapRb<f32>,
    data: &mut [T],
) {
    if (audio_receiver.occupied_len() + resampled_buffer.occupied_len()) < data.len() {
        // TODO: Probably the end of the audio? Maybe should partial resample?
        println!(
            "Not enough Audio in buffers: {} + {} < {}",
            audio_receiver.occupied_len(),
            resampled_buffer.occupied_len(),
            data.len()
        );
        return;
    }
    println!("Processing for {} samples", data.len());

    if audio_receiver.occupied_len() >= resampler.input_frames_next() * NUM_CHANNELS {
        println!("resampling");
        let resampled = resampler.resample(&audio_receiver.pop_iter().collect_vec());
        resampled_buffer.push_slice(&resampled.collect_vec());
    }

    for (src, dest) in resampled_buffer.pop_iter().zip(data.iter_mut()) {
        *dest = T::from_sample(src);
    }
}

struct State {
    pub operations: Vec<Operation>,
    pub waveform: Option<WaveOperation>,
    audio_producer: HeapProd<f32>,
    decoder: opus::Decoder,
    decode_buffer: [f32; OPUS_CHUNK_SIZE * NUM_CHANNELS],
}

impl State {
    fn new() -> (Self, HeapCons<f32>) {
        let (audio_producer, audio_receiver) = HeapRb::new(SAMPLE_RATE).split();
        (
            Self {
                operations: Vec::new(),
                waveform: None,
                audio_producer,
                decode_buffer: [0f32; OPUS_CHUNK_SIZE * NUM_CHANNELS],
                decoder: opus::Decoder::new(SAMPLE_RATE as u32, opus::Channels::Stereo)
                    .expect("Couldn't create opus decoder"),
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
        let n = self
            .decoder
            .decode_float(audio, &mut self.decode_buffer, false)
            .unwrap();
        self.audio_producer.push_slice(&self.decode_buffer[..n]);
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
    let sample_rate = supported_config.sample_rate().0 as usize;
    let config: StreamConfig = supported_config.clone().into();

    let mut font57 = load_ttf_font("./m8stealth57.ttf").await.unwrap();
    font57.set_filter(FilterMode::Linear);
    let mut font89 = load_ttf_font("./m8stealth89.ttf").await.unwrap();
    font89.set_filter(FilterMode::Linear);

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
    let waveform_texture = Texture2D::from_image(&Image::gen_image_color(
        M8_SCREEN_WIDTH as u16,
        WAVE_HEIGHT as u16,
        BLACK,
    ));
    waveform_texture.set_filter(FilterMode::Linear);

    let mut last_screen_width = screen_width();
    let mut last_screen_height = screen_height();
    let mut parser = parser::Parser::new();

    let mut resampler = AudioResampler::new(SAMPLE_RATE, sample_rate);

    let (mut state, mut audio_receiver) = State::new();

    let mut resampled_buffer =
        HeapRb::new((SAMPLE_RATE as f32 * resampler.ratio()).ceil() as usize);

    macro_rules! handle_sample {
        ($sample:ty) => {
            audio_device
                .build_output_stream(
                    &config,
                    move |data: &mut [$sample], _: &cpal::OutputCallbackInfo| {
                        write_audio::<$sample>(
                            &mut audio_receiver,
                            &mut resampler,
                            &mut resampled_buffer,
                            data,
                        );
                    },
                    err_fn,
                    None,
                )
                .unwrap()
        };
    }

    let stream = match sample_format {
        SampleFormat::F32 => handle_sample!(f32),
        SampleFormat::I16 => handle_sample!(i16),
        SampleFormat::U16 => handle_sample!(u16),
        sample_format => panic!("Unsupported sample format '{sample_format}'"),
    };
    stream.play().unwrap();

    // let mut last_update = Instant::now();
    'runloop: loop {
        // println!("FPS: {}", get_fps());

        if websocket.connected() {
            while let Some(msg) = websocket.try_recv() {
                parser.parse(&msg, &mut state);
            }
        }

        // if (Instant::now() - last_update) <= Duration::from_millis(5) {
        //     println!(
        //         "Skipping Redraw. To Render: {} operations",
        //         state.operations.len()
        //     );
        //     next_frame().await;
        //     continue;
        // }
        // last_update = Instant::now();
        // println!("To Render: {} operations", state.operations.len());

        set_camera(&camera);
        let (font_size, font_scale, font_aspect) = camera_font_scale(10.0);

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
                            font_size,
                            font_scale,
                            font_scale_aspect: font_aspect,
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

        let x = (screen_width() - width) / 2.0;
        let y = (screen_height() - height) / 2.0;
        let height = (width / 320.0) * WAVE_HEIGHT as f32;

        draw_texture_ex(
            &waveform_texture,
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

        next_frame().await
    }
}
