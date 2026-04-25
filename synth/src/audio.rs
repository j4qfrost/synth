use crate::dsp::hot;
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample, StreamConfig};
use crossbeam_channel::Receiver;
use std::sync::Arc;
use synth_state::{AudioMessage, SharedParams, VoiceManager};

pub struct AudioHandle {
    pub device_name: String,
    pub sample_rate: u32,
    _stream: cpal::Stream,
}

pub fn start_audio(
    rx: Receiver<AudioMessage>,
    params: Arc<SharedParams>,
) -> Result<AudioHandle> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no default output device"))?;
    let device_name = device.name().unwrap_or_else(|_| "<unknown>".into());

    let supported = device.default_output_config()?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.clone().into();

    log::info!(
        "audio device: {} | sample rate: {} Hz | channels: {} | format: {:?}",
        device_name,
        config.sample_rate.0,
        config.channels,
        sample_format
    );

    let sample_rate = config.sample_rate.0;
    let voice_manager = VoiceManager::new(sample_rate as f32, params);

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, voice_manager, rx)?,
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, voice_manager, rx)?,
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, voice_manager, rx)?,
        cpal::SampleFormat::I8 => build::<i8>(&device, &config, voice_manager, rx)?,
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, voice_manager, rx)?,
        cpal::SampleFormat::I64 => build::<i64>(&device, &config, voice_manager, rx)?,
        cpal::SampleFormat::U8 => build::<u8>(&device, &config, voice_manager, rx)?,
        cpal::SampleFormat::U32 => build::<u32>(&device, &config, voice_manager, rx)?,
        cpal::SampleFormat::U64 => build::<u64>(&device, &config, voice_manager, rx)?,
        cpal::SampleFormat::F64 => build::<f64>(&device, &config, voice_manager, rx)?,
        other => return Err(anyhow!("unsupported sample format: {:?}", other)),
    };

    stream.play()?;

    Ok(AudioHandle {
        device_name,
        sample_rate,
        _stream: stream,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut voice_manager: VoiceManager,
    rx: Receiver<AudioMessage>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    // cpal callbacks on macOS are typically 256–4096 frames; pre-size to 8192 so
    // the per-callback slice is just an in-place reborrow.
    let mut mono = vec![0.0f32; 8192];

    let err_fn = |err| log::error!("stream error: {err}");

    let stream = device.build_output_stream(
        config,
        move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = output.len() / channels.max(1);
            if mono.len() < frames {
                mono.resize(frames, 0.0);
            }
            let mono_slice = &mut mono[..frames];

            // One hot call per buffer: drain control messages + render. The
            // hot-lib-reloader proxy takes a brief read-lock; reload (write-lock)
            // happens between buffers in the worst case → at most one buffer of
            // glitch on swap.
            hot::process_buffer(&mut voice_manager, mono_slice, &rx);

            for (frame_idx, frame) in output.chunks_mut(channels).enumerate() {
                let v: T = T::from_sample(mono_slice[frame_idx]);
                for s in frame.iter_mut() {
                    *s = v;
                }
            }
        },
        err_fn,
        None,
    )?;

    Ok(stream)
}
