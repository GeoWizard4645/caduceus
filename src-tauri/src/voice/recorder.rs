//! Microphone capture for push-to-talk.
//!
//! Records from the default input device while the hotkey is held, then emits a
//! 16 kHz mono 16-bit WAV — the format every speech recogniser Orbit talks to
//! accepts natively, and small enough to POST to a local Whisper server without
//! thinking about it.
//!
//! `cpal::Stream` is not `Send` on every platform, so the stream is owned by a
//! dedicated thread and controlled through channels.

use std::sync::mpsc;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use thiserror::Error;

/// Sample rate every STT backend Orbit supports is happy with.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Error)]
pub enum RecorderError {
    #[error("no microphone found. Check your input device in System Settings.")]
    NoDevice,
    #[error("could not open the microphone: {0}. On macOS, grant Orbit microphone access in System Settings \u{2192} Privacy & Security.")]
    Device(String),
    #[error("recording failed: {0}")]
    Stream(String),
    #[error("could not encode the recording: {0}")]
    Encode(String),
    #[error("nothing was recorded \u{2014} hold the key a little longer")]
    Empty,
}

pub type RecorderResult<T> = Result<T, RecorderError>;

/// A recording in progress.
pub struct Recording {
    stop_tx: mpsc::Sender<()>,
    result_rx: mpsc::Receiver<RecorderResult<Vec<u8>>>,
}

impl Recording {
    /// Stop recording and return the WAV bytes.
    ///
    /// Blocking, but only for as long as it takes the capture thread to drain —
    /// milliseconds. Call from `spawn_blocking` if that matters.
    pub fn finish(self) -> RecorderResult<Vec<u8>> {
        let _ = self.stop_tx.send(());
        self.result_rx
            .recv()
            .unwrap_or(Err(RecorderError::Stream("capture thread died".into())))
    }
}

/// Begin recording from the default input device.
///
/// `max_secs` is a hard ceiling so a stuck key cannot fill memory.
pub fn start(max_secs: u32) -> RecorderResult<Recording> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (result_tx, result_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel::<RecorderResult<()>>();

    std::thread::Builder::new()
        .name("orbit-mic".into())
        .spawn(move || {
            let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));

            let outcome = (|| -> RecorderResult<(u32, u16)> {
                let host = cpal::default_host();
                let device = host.default_input_device().ok_or(RecorderError::NoDevice)?;
                let supported = device
                    .default_input_config()
                    .map_err(|e| RecorderError::Device(e.to_string()))?;

                // `cpal::SampleRate` is a plain `u32` alias as of 0.18.
                let sample_rate: u32 = supported.sample_rate();
                let channels = supported.channels();
                let config: cpal::StreamConfig = supported.config();

                let sink = samples.clone();
                let max_samples = (sample_rate as usize)
                    .saturating_mul(channels as usize)
                    .saturating_mul(max_secs.max(1) as usize);

                let err_fn = |e| log::error!("microphone stream error: {e}");

                // Every sample format is normalised to f32 in the callback, so
                // the resampling path downstream only handles one type.
                let stream = match supported.sample_format() {
                    cpal::SampleFormat::F32 => device.build_input_stream(
                        config,
                        move |data: &[f32], _: &_| append(&sink, data.iter().copied(), max_samples),
                        err_fn,
                        None,
                    ),
                    cpal::SampleFormat::I16 => device.build_input_stream(
                        config,
                        move |data: &[i16], _: &_| {
                            append(&sink, data.iter().map(|s| *s as f32 / i16::MAX as f32), max_samples)
                        },
                        err_fn,
                        None,
                    ),
                    cpal::SampleFormat::U16 => device.build_input_stream(
                        config,
                        move |data: &[u16], _: &_| {
                            append(
                                &sink,
                                data.iter().map(|s| (*s as f32 - 32768.0) / 32768.0),
                                max_samples,
                            )
                        },
                        err_fn,
                        None,
                    ),
                    other => {
                        return Err(RecorderError::Device(format!(
                            "unsupported sample format {other:?}"
                        )))
                    }
                }
                .map_err(|e| RecorderError::Device(e.to_string()))?;

                stream.play().map_err(|e| RecorderError::Stream(e.to_string()))?;

                // Wait for the stop signal or the safety ceiling, whichever
                // comes first. The stream lives until this scope ends.
                let _ = stop_rx.recv_timeout(std::time::Duration::from_secs(max_secs.max(1) as u64));
                drop(stream);

                Ok((sample_rate, channels))
            })();

            match outcome {
                Err(e) => {
                    // Report the failure through whichever channel is still
                    // being awaited.
                    let msg = e.to_string();
                    let _ = ready_tx.send(Err(e));
                    let _ = result_tx.send(Err(RecorderError::Stream(msg)));
                }
                Ok((sample_rate, channels)) => {
                    let _ = ready_tx.send(Ok(()));
                    let raw = std::mem::take(&mut *samples.lock());
                    let _ = result_tx.send(encode_wav(&raw, sample_rate, channels));
                }
            }
        })
        .map_err(|e| RecorderError::Stream(e.to_string()))?;

    // Surface device-open failures immediately rather than on release of the
    // hotkey, so the UI can say "no microphone" while the user is still holding
    // the key.
    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) | Err(_) => Ok(Recording { stop_tx, result_rx }),
        Ok(Err(e)) => Err(e),
    }
}

fn append(sink: &Arc<Mutex<Vec<f32>>>, iter: impl Iterator<Item = f32>, max_samples: usize) {
    let mut buf = sink.lock();
    if buf.len() >= max_samples {
        return;
    }
    for s in iter {
        if buf.len() >= max_samples {
            break;
        }
        buf.push(s);
    }
}

/// Downmix to mono, resample to 16 kHz, and write a WAV.
fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> RecorderResult<Vec<u8>> {
    if samples.is_empty() {
        return Err(RecorderError::Empty);
    }

    let mono = downmix(samples, channels);
    let resampled = resample(&mono, sample_rate, TARGET_SAMPLE_RATE);

    // Roughly 100ms of audio; below that there is nothing to transcribe.
    if resampled.len() < (TARGET_SAMPLE_RATE / 10) as usize {
        return Err(RecorderError::Empty);
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)
            .map_err(|e| RecorderError::Encode(e.to_string()))?;
        for s in resampled {
            let clamped = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer
                .write_sample(clamped)
                .map_err(|e| RecorderError::Encode(e.to_string()))?;
        }
        writer.finalize().map_err(|e| RecorderError::Encode(e.to_string()))?;
    }
    Ok(cursor.into_inner())
}

fn downmix(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let n = channels as usize;
    samples
        .chunks(n)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Linear-interpolation resampling.
///
/// Speech at 16 kHz through a naive resampler is indistinguishable to a
/// recogniser from one that does proper band-limited interpolation; pulling in
/// a full DSP crate for this would be dead weight.
fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 / ratio;
        let idx = src.floor() as usize;
        let frac = (src - idx as f64) as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[1.0, 2.0, 3.0], 1), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn resampling_scales_the_sample_count() {
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 / 100.0).sin()).collect();
        let out = resample(&input, 48_000, 16_000);
        assert!((out.len() as i64 - 16_000).abs() <= 1, "got {}", out.len());
    }

    #[test]
    fn resampling_is_a_no_op_at_the_same_rate() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample(&input, 16_000, 16_000), input);
    }

    #[test]
    fn resampling_preserves_the_signal_shape() {
        // A ramp stays a ramp: endpoints are preserved and it is monotonic.
        let input: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let out = resample(&input, 32_000, 16_000);
        assert!(out[0] < 0.01);
        assert!(out[out.len() - 1] > 0.98);
        assert!(out.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn encoding_produces_a_readable_wav() {
        let samples: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 0.01).sin() * 0.5)
            .collect();
        let wav = encode_wav(&samples, 48_000, 1).unwrap();

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");

        let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).unwrap();
        assert_eq!(reader.spec().sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().bits_per_sample, 16);
    }

    #[test]
    fn stereo_input_becomes_mono_output() {
        let stereo: Vec<f32> = (0..32_000).map(|i| if i % 2 == 0 { 0.5 } else { -0.5 }).collect();
        let wav = encode_wav(&stereo, 16_000, 2).unwrap();
        let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).unwrap();
        assert_eq!(reader.spec().channels, 1);
    }

    #[test]
    fn too_short_recordings_are_rejected_with_a_useful_error() {
        assert!(matches!(encode_wav(&[], 48_000, 1), Err(RecorderError::Empty)));
        assert!(matches!(
            encode_wav(&[0.0; 100], 48_000, 1),
            Err(RecorderError::Empty)
        ));
    }

    #[test]
    fn loud_samples_are_clamped_not_wrapped() {
        let samples = vec![9.0f32; 16_000];
        let wav = encode_wav(&samples, 16_000, 1).unwrap();
        let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).unwrap();
        let peak = reader
            .into_samples::<i16>()
            .filter_map(Result::ok)
            .map(i16::abs)
            .max()
            .unwrap();
        assert_eq!(peak, i16::MAX);
    }
}
