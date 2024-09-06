use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, SizedSample, Stream,
};
#[cfg(target_os = "macos")]
use cpal::{SampleRate, SupportedStreamConfig};
use futures::channel::mpsc;
use log::{debug, info};

pub fn run_audio() -> impl futures::stream::Stream<Item = Vec<u8>> {
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
            SampleFormat::I8 => run::<i8>(&input_device, &config.into(), audio_sender),
            SampleFormat::I16 => run::<i16>(&input_device, &config.into(), audio_sender),
            SampleFormat::I32 => run::<i32>(&input_device, &config.into(), audio_sender),
            SampleFormat::I64 => run::<i64>(&input_device, &config.into(), audio_sender),
            SampleFormat::U8 => run::<u8>(&input_device, &config.into(), audio_sender),
            SampleFormat::U16 => run::<u16>(&input_device, &config.into(), audio_sender),
            SampleFormat::U32 => run::<u32>(&input_device, &config.into(), audio_sender),
            SampleFormat::U64 => run::<u64>(&input_device, &config.into(), audio_sender),
            SampleFormat::F32 => run::<f32>(&input_device, &config.into(), audio_sender),
            SampleFormat::F64 => run::<f64>(&input_device, &config.into(), audio_sender),
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
    audio_sender: mpsc::Sender<Vec<u8>>,
) -> Result<Stream, anyhow::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let err_fn = |err| eprintln!("an error occurred on stream: {}", err);

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| write_data(data, audio_sender.clone()),
        err_fn,
        None,
    )?;

    Ok(stream)
}

fn write_data<T>(data: &[T], mut audio_sender: mpsc::Sender<Vec<u8>>)
where
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

    // Convert every sample to f32
    let data = data
        .iter()
        .map(|x| f32::from_sample(*x))
        .collect::<Vec<f32>>();

    // Cast all data to &[u8]
    let u8_data = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4) };

    audio_sender.try_send(u8_data.to_vec()).unwrap();
}
