use std::collections::VecDeque;

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, SizedSample, Stream,
};
#[cfg(target_os = "macos")]
use cpal::{SampleRate, SupportedStreamConfig};
use futures::channel::mpsc;
use itertools::{interleave, Itertools};
use log::{debug, info};
use rubato::VecResampler as _;

const SAMPLE_RATE: usize = 48000;
const NUM_CHANNELS: usize = 2;
const OPUS_CHUNK_SIZE: usize = 960;

struct Resampler<T> {
    resampler: rubato::FftFixedOut<f32>,
    src_rate: usize,
    dest_rate: usize,
    output_buffers: Vec<Vec<f32>>,
    //TODO: buffer sizes are wrong depending on sample rate
    left_buffer: [f32; 1024],
    right_buffer: [f32; 1024],
    input_buffer: VecDeque<T>,
    src_samples_per_chunk: usize,
    src_samples_per_channel: usize,
}

impl<T> Resampler<T>
where
    T: Sample,
    f32: Sample + FromSample<T>,
{
    fn new(src_rate: usize, dest_rate: usize) -> Self {
        let resampler =
            rubato::FftFixedOut::<f32>::new(src_rate, dest_rate, OPUS_CHUNK_SIZE, 2, NUM_CHANNELS)
                .unwrap();
        let output_buffers = resampler.output_buffer_allocate(true);
        let src_samples_per_channel =
            ((OPUS_CHUNK_SIZE as f32 / dest_rate as f32) * src_rate as f32) as usize;
        let src_samples_per_chunk = src_samples_per_channel * NUM_CHANNELS;

        Self {
            resampler,
            src_rate,
            dest_rate,
            output_buffers,
            left_buffer: [0.0; 1024],
            right_buffer: [0.0; 1024],
            input_buffer: VecDeque::new(),
            src_samples_per_chunk,
            src_samples_per_channel,
        }
    }

    fn resample(&mut self) -> impl Iterator<Item = Vec<f32>> + '_ {
        std::iter::from_fn(|| {
            if self.input_buffer.len() >= self.src_samples_per_chunk {
                let chunk = self
                    .input_buffer
                    .drain(..self.src_samples_per_chunk)
                    .map(|v| f32::from_sample(v));

                // Skip resampling, just copy the bytes
                if self.src_rate == self.dest_rate {
                    return Some(chunk.collect_vec());
                }

                // Split left + right
                let mut i = 0;
                for (x, v) in chunk.enumerate() {
                    if x % 2 == 0 {
                        self.left_buffer[i] = v;
                    } else {
                        self.right_buffer[i] = v;
                        i += 1;
                    }
                }

                let (_in_len, out_len) = self
                    .resampler
                    .process_into_buffer(
                        &[
                            self.left_buffer[..self.src_samples_per_channel].to_vec(),
                            self.right_buffer[..self.src_samples_per_channel].to_vec(),
                        ],
                        &mut self.output_buffers,
                        None,
                    )
                    .unwrap();

                let data = interleave(
                    &self.output_buffers[0][..out_len],
                    &self.output_buffers[1][..out_len],
                )
                .cloned()
                .collect::<Vec<f32>>();

                return Some(data);
            }
            None
        })
    }

    fn extend(&mut self, samples: &[T]) {
        // If all 0's return
        let (prefix, aligned, suffix) = unsafe { samples.align_to::<u128>() };
        if prefix.iter().all(|&x| f32::from_sample(x) == 0.0)
            && suffix.iter().all(|&x| f32::from_sample(x) == 0.0)
            && aligned.iter().all(|&x| x == 0)
        {
            return;
        }

        self.input_buffer.extend(samples)
    }
}

pub fn run_audio() -> impl futures::stream::Stream<Item = Vec<f32>> {
    let (audio_sender, audio_receiver) = mpsc::channel(8);

    let host = cpal::default_host();

    for device in host.input_devices().into_iter() {
        for y in device {
            info!("Audio Device: {:?}", y.name());
        }
    }

    let input_device = host
        .input_devices()
        .into_iter()
        .find_map(|mut d| {
            d.find(|x| {
                x.name().is_ok_and(|name| {
                    #[cfg(target_os = "macos")]
                    return name == "M8";
                    #[cfg(target_os = "linux")]
                    return name == "iec958:CARD=M8,DEV=0";
                })
            })
        })
        .expect("Couldn't find M8 Audio Device");

    #[cfg(target_os = "macos")]
    let config = SupportedStreamConfig::new(
        2,
        SampleRate(44100),
        cpal::SupportedBufferSize::Range { min: 4, max: 4096 },
        cpal::SampleFormat::F32,
    );

    #[cfg(target_os = "linux")]
    let config = input_device
        .default_input_config()
        .expect("Could not create default config");

    debug!("Input config: {:?}", config);

    std::thread::spawn(move || {
        let stream = match config.sample_format() {
            SampleFormat::I8 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<i8>(&input_device, &config.into(), audio_sender, resampler)
            }
            SampleFormat::I16 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<i16>(&input_device, &config.into(), audio_sender, resampler)
            }
            SampleFormat::I32 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<i32>(&input_device, &config.into(), audio_sender, resampler)
            }
            SampleFormat::I64 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<i64>(&input_device, &config.into(), audio_sender, resampler)
            }
            SampleFormat::U8 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<u8>(&input_device, &config.into(), audio_sender, resampler)
            }
            SampleFormat::U16 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<u16>(&input_device, &config.into(), audio_sender, resampler)
            }
            SampleFormat::U32 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<u32>(&input_device, &config.into(), audio_sender, resampler)
            }
            SampleFormat::U64 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<u64>(&input_device, &config.into(), audio_sender, resampler)
            }
            SampleFormat::F32 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<f32>(&input_device, &config.into(), audio_sender, resampler)
            }
            SampleFormat::F64 => {
                let resampler = Resampler::new(config.sample_rate().0 as usize, SAMPLE_RATE);
                run::<f64>(&input_device, &config.into(), audio_sender, resampler)
            }
            sample_format => panic!("Unsupported sample format '{sample_format}'"),
        }
        .unwrap();

        info!("Starting Audio Stream");
        stream.play().unwrap();
        std::thread::park();
        info!("Audio Stream Done");
    });

    audio_receiver
}

fn run<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut audio_sender: mpsc::Sender<Vec<f32>>,
    mut resampler: Resampler<T>,
) -> Result<Stream, anyhow::Error>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            resampler.extend(data);
            for data in resampler.resample() {
                audio_sender.try_send(data).unwrap();
            }
        },
        |err| eprintln!("an error occurred on stream: {}", err),
        None,
    )?)
}
