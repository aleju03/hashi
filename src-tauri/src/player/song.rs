use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, OutputCallbackInfo, Sample, SampleFormat, SizedSample, StreamConfig};
use mania_bridge::player::Song;

// further off than this and the sound jumps to where it should be, closer it just carries on
const DRIFT_MS: f64 = 15.0;

#[derive(Default)]
struct Voice {
    song: Option<Arc<Song>>,
    // where in the song (ms) it is at that instant, None for quiet
    at: Option<(Instant, f64)>,
    gain: f32,
    // the next sample to read, in the song's own frames
    pos: Option<f64>,
}

pub struct Player {
    voice: Arc<Mutex<Voice>>,
    broken: Arc<AtomicBool>,
    _stream: cpal::Stream,
}

impl Player {
    pub fn open() -> Result<Player, String> {
        let device = cpal::default_host().default_output_device().ok_or("There's no sound output.")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let voice = Arc::new(Mutex::new(Voice::default()));
        let broken = Arc::new(AtomicBool::new(false));
        let (v, b) = (voice.clone(), broken.clone());
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, &config, v, b),
            SampleFormat::I16 => build::<i16>(&device, &config, v, b),
            SampleFormat::U16 => build::<u16>(&device, &config, v, b),
            SampleFormat::I32 => build::<i32>(&device, &config, v, b),
            SampleFormat::F64 => build::<f64>(&device, &config, v, b),
            other => return Err(format!("The sound output takes {other} samples, which Hashi can't make.")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Player { voice, broken, _stream: stream })
    }

    // the stream died, e.g. its device went away
    pub fn broken(&self) -> bool {
        self.broken.load(Ordering::Relaxed)
    }

    pub fn song(&self) -> Option<Arc<Song>> {
        self.voice.lock().unwrap_or_else(|e| e.into_inner()).song.clone()
    }

    pub fn set_song(&self, song: Option<Arc<Song>>) {
        let mut v = self.voice.lock().unwrap_or_else(|e| e.into_inner());
        v.song = song;
        v.pos = None;
    }

    // at: where the song should be right now in its own ms, None to be quiet
    pub fn follow(&self, at: Option<f64>, volume: f64) {
        let mut v = self.voice.lock().unwrap_or_else(|e| e.into_inner());
        v.at = at.map(|ms| (Instant::now(), ms));
        v.gain = ((volume / 100.0).clamp(0.0, 1.0).powi(2)) as f32;
        if at.is_none() {
            v.pos = None;
        }
    }
}

fn build<T: SizedSample + FromSample<f32>>(device: &cpal::Device, config: &StreamConfig, voice: Arc<Mutex<Voice>>, broken: Arc<AtomicBool>) -> Result<cpal::Stream, String> {
    let channels = config.channels as usize;
    let out_rate = f64::from(config.sample_rate);
    device
        .build_output_stream(
            config.clone(),
            move |data: &mut [T], info: &OutputCallbackInfo| {
                let ts = info.timestamp();
                let late = ts.playback.duration_since(ts.callback);
                fill(&voice, data, channels, out_rate, late);
            },
            move |e| {
                eprintln!("sound: {e}");
                // these leave the stream playing
                if !matches!(e.kind(), ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied) {
                    broken.store(true, Ordering::Relaxed);
                }
            },
            None,
        )
        .map_err(|e| e.to_string())
}

fn fill<T: Sample + FromSample<f32>>(voice: &Mutex<Voice>, data: &mut [T], channels: usize, out_rate: f64, late: Duration) {
    let mut v = voice.lock().unwrap_or_else(|e| e.into_inner());
    let (Some(song), Some((when, ms))) = (v.song.clone(), v.at) else {
        data.fill(T::EQUILIBRIUM);
        return;
    };
    let rate = f64::from(song.rate);
    // what should be heard when this buffer comes out of the speakers
    let heard = Instant::now() + late;
    let since = heard.checked_duration_since(when).map_or_else(|| -(when - heard).as_secs_f64(), |d| d.as_secs_f64());
    let want = (ms / 1000.0 + since) * rate;
    let mut pos = match v.pos {
        Some(p) if (p - want).abs() < DRIFT_MS / 1000.0 * rate => p,
        _ => want,
    };
    let step = rate / out_rate;
    let gain = v.gain;
    let src_ch = song.channels.max(1);
    let frames = song.samples.len() / src_ch;
    let sample = |frame: usize, ch: usize| f32::from(song.samples[frame * src_ch + ch.min(src_ch - 1)]) / 32768.0;
    for out in data.chunks_mut(channels) {
        let i = pos.floor();
        if i >= 0.0 && (i as usize) + 1 < frames {
            let (i, frac) = (i as usize, (pos - i) as f32);
            for (ch, s) in out.iter_mut().enumerate() {
                let x = sample(i, ch) * (1.0 - frac) + sample(i + 1, ch) * frac;
                *s = T::from_sample(x * gain);
            }
        } else {
            out.fill(T::EQUILIBRIUM);
        }
        pos += step;
    }
    v.pos = Some(pos);
}
