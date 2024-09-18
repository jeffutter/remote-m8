use cpal::{FromSample, Sample};
use ringbuf::{traits::Consumer, HeapCons};
use rubato::Resampler;

const OPUS_CHUNK_SIZE: usize = 960;
const NUM_CHANNELS: usize = 2;
pub const OPUS_SAMPLE_RATE: usize = 48000;

pub struct AudioDecoder {
    decoder: opus::Decoder,
    decode_buffer: [f32; OPUS_CHUNK_SIZE * NUM_CHANNELS],
}

impl AudioDecoder {
    pub fn new() -> Self {
        Self {
            decode_buffer: [0f32; OPUS_CHUNK_SIZE * NUM_CHANNELS],
            decoder: opus::Decoder::new(OPUS_SAMPLE_RATE as u32, opus::Channels::Stereo)
                .expect("Couldn't create opus decoder"),
        }
    }

    pub fn decode(&mut self, audio: &[u8]) -> &[f32] {
        let n = self
            .decoder
            .decode_float(audio, &mut self.decode_buffer, false)
            .unwrap();
        &self.decode_buffer[..(n * NUM_CHANNELS)]
    }
}

pub struct AudioResampler {
    resampler: rubato::FftFixedInOut<f32>,
    src_rate: usize,
    dest_rate: usize,
    input_buffers: Vec<Vec<f32>>,
    output_buffers: Vec<Vec<f32>>,
    output_buffer: Vec<f32>,
}

impl AudioResampler {
    pub fn new(src_rate: usize, dest_rate: usize) -> Self {
        let resampler =
            rubato::FftFixedInOut::<f32>::new(src_rate, dest_rate, OPUS_CHUNK_SIZE, NUM_CHANNELS)
                .unwrap();
        let input_buffers = resampler.input_buffer_allocate(true);
        let output_buffers = resampler.output_buffer_allocate(true);

        Self {
            resampler,
            src_rate,
            dest_rate,
            input_buffers,
            output_buffers,
            output_buffer: vec![0f32; dest_rate * NUM_CHANNELS],
        }
    }

    pub fn resample(&mut self, samples: &[f32]) -> Box<dyn Iterator<Item = f32> + '_> {
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
            .take(out_len_per_channel)
            .zip(
                self.output_buffers[0]
                    .iter()
                    .zip(self.output_buffers[1].iter()),
            )
        {
            dest[0] = *l;
            dest[1] = *r;
        }

        Box::new(
            self.output_buffer
                .drain(..(out_len_per_channel * NUM_CHANNELS)),
        )
    }
}

pub fn write_audio<T: Sample + FromSample<f32>>(
    audio_receiver: &mut HeapCons<f32>,
    write_buffer: &mut [f32; 2048],
    data: &mut [T],
) {
    let written = audio_receiver.pop_slice(&mut write_buffer[..data.len()]);

    for (src, dest) in write_buffer[..written]
        .iter()
        .map(|x| T::from_sample(*x))
        .zip(data.iter_mut())
    {
        *dest = src;
    }

    if data.len() - written > 0 {
        let last = data[written];
        // println!("Short: {} samples", data.len() - written);
        for dest in data.iter_mut().skip(written) {
            *dest = last;
        }
    }
}
