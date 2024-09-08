use std::collections::VecDeque;

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, SizedSample, Stream,
};
#[cfg(target_os = "macos")]
use cpal::{SampleRate, SupportedStreamConfig};
use futures::channel::mpsc;
use itertools::interleave;
use log::{debug, info};
use rubato::VecResampler;

const SAMPLE_RATE: usize = 48000;
const NUM_CHANNELS: usize = 2;
const OPUS_CHUNK_SIZE: usize = 960;

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

    let resampler = rubato::FftFixedOut::<f32>::new(
        config.sample_rate().0 as usize,
        SAMPLE_RATE,
        OPUS_CHUNK_SIZE,
        2,
        NUM_CHANNELS,
    )
    .unwrap();
    let resampler_output_buffers = resampler.output_buffer_allocate(true);

    std::thread::spawn(move || {
        let stream = match config.sample_format() {
            SampleFormat::I8 => run::<i8>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
            SampleFormat::I16 => run::<i16>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
            SampleFormat::I32 => run::<i32>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
            SampleFormat::I64 => run::<i64>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
            SampleFormat::U8 => run::<u8>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
            SampleFormat::U16 => run::<u16>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
            SampleFormat::U32 => run::<u32>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
            SampleFormat::U64 => run::<u64>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
            SampleFormat::F32 => run::<f32>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
            SampleFormat::F64 => run::<f64>(
                &input_device,
                &config.into(),
                audio_sender,
                (
                    VecDeque::new(),
                    [0.0f32; OPUS_CHUNK_SIZE],
                    [0.0f32; OPUS_CHUNK_SIZE],
                ),
                resampler,
                resampler_output_buffers,
            ),
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

pub fn run<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    audio_sender: mpsc::Sender<Vec<f32>>,
    //TODO: buffer sizes are wrong depending on sample rate
    mut buffer: (VecDeque<T>, [f32; OPUS_CHUNK_SIZE], [f32; OPUS_CHUNK_SIZE]),
    mut resampler: impl VecResampler<f32> + 'static,
    mut resampler_output_buffers: Vec<Vec<f32>>,
) -> Result<Stream, anyhow::Error>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let err_fn = |err| eprintln!("an error occurred on stream: {}", err);

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            write_data(
                data,
                audio_sender.clone(),
                &mut buffer,
                &mut resampler,
                &mut resampler_output_buffers,
            )
        },
        err_fn,
        None,
    )?;

    Ok(stream)
}

fn write_data<T>(
    data: &[T],
    mut audio_sender: mpsc::Sender<Vec<f32>>,
    //TODO: buffer sizes are wrong depending on sample rate
    (sample_buffer, left_buffer, right_buffer): &mut (
        VecDeque<T>,
        [f32; OPUS_CHUNK_SIZE],
        [f32; OPUS_CHUNK_SIZE],
    ),
    resampler: &mut impl VecResampler<f32>,
    resampler_output_buffers: &mut [Vec<f32>],
) where
    T: Sample,
    f32: Sample + FromSample<T>,
{
    // If all 0's return
    let (prefix, aligned, suffix) = unsafe { data.align_to::<u128>() };
    if prefix.iter().all(|&x| f32::from_sample(x) == 0.0)
        && suffix.iter().all(|&x| f32::from_sample(x) == 0.0)
        && aligned.iter().all(|&x| x == 0)
    {
        return;
    }

    // Add to existing samples
    sample_buffer.extend(data);

    // For every appropriate length chunk
    while sample_buffer.len() >= resampler.input_frames_next() * 2 {
        let samples_per_channel = resampler.input_frames_next();
        let all_samples = samples_per_channel * NUM_CHANNELS;
        //TODO: Skip resampling if not necessary

        // Split left/right and convert to f32s
        let mut i = 0;
        for (x, v) in sample_buffer.drain(..all_samples).enumerate() {
            let v = f32::from_sample(v);
            if x % 2 == 0 {
                left_buffer[i] = v;
            } else {
                right_buffer[i] = v;
                i += 1;
            }
        }

        // Resample
        let (_in_len, out_len) = resampler
            .process_into_buffer(
                &[
                    left_buffer[..samples_per_channel].to_vec(),
                    right_buffer[..samples_per_channel].to_vec(),
                ],
                resampler_output_buffers,
                None,
            )
            .unwrap();

        let data = interleave(
            &resampler_output_buffers[0][..out_len],
            &resampler_output_buffers[1][..out_len],
        )
        .cloned()
        .collect::<Vec<f32>>();

        audio_sender.try_send(data).unwrap();
    }
}
