//! WAV in / out and resampling for the stems.

use crate::shared::{StemAudio, StemSet};
use rubato::{FftFixedIn, Resampler};
use std::path::{Path, PathBuf};

pub const PEAK_COLUMNS: usize = 1200;

/// Read a WAV as two channels of f32 plus its sample rate.
pub fn read_wav(path: &Path) -> Result<(Vec<f32>, Vec<f32>, u32), String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();
    let chans = spec.channels.max(1) as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    let frames = samples.len() / chans;
    let mut l = Vec::with_capacity(frames);
    let mut r = Vec::with_capacity(frames);
    for f in 0..frames {
        let a = samples[f * chans];
        let b = if chans > 1 { samples[f * chans + 1] } else { a };
        l.push(a);
        r.push(b);
    }
    Ok((l, r, spec.sample_rate))
}

/// Write two channels as 32-bit float WAV.
pub fn write_wav(path: &Path, l: &[f32], r: &[f32], sample_rate: u32, gain: f32) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for (a, b) in l.iter().zip(r) {
        w.write_sample(a * gain).map_err(|e| e.to_string())?;
        w.write_sample(b * gain).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// Resample both channels with rubato's FFT resampler, keeping the length exact and the timing
/// aligned (the resampler's delay is trimmed off).
pub fn resample(l: Vec<f32>, r: Vec<f32>, from: u32, to: u32) -> Result<(Vec<f32>, Vec<f32>), String> {
    if from == to || l.is_empty() {
        return Ok((l, r));
    }
    let chunk = 4096;
    let mut rs = FftFixedIn::<f32>::new(from as usize, to as usize, chunk, 2, 2)
        .map_err(|e| e.to_string())?;
    let want = ((l.len() as f64) * to as f64 / from as f64).round() as usize;
    let delay = rs.output_delay();
    let mut out_l = Vec::with_capacity(want + delay + chunk * 2);
    let mut out_r = Vec::with_capacity(want + delay + chunk * 2);
    let mut pos = 0;
    while pos + chunk <= l.len() {
        let res = rs
            .process(&[&l[pos..pos + chunk], &r[pos..pos + chunk]], None)
            .map_err(|e| e.to_string())?;
        out_l.extend_from_slice(&res[0]);
        out_r.extend_from_slice(&res[1]);
        pos += chunk;
    }
    if pos < l.len() {
        let res = rs
            .process_partial(Some(&[&l[pos..], &r[pos..]]), None)
            .map_err(|e| e.to_string())?;
        out_l.extend_from_slice(&res[0]);
        out_r.extend_from_slice(&res[1]);
    }
    // flush until the tail is out
    let mut guard = 0;
    while out_l.len() < want + delay && guard < 8 {
        let res = rs
            .process_partial::<&[f32]>(None, None)
            .map_err(|e| e.to_string())?;
        out_l.extend_from_slice(&res[0]);
        out_r.extend_from_slice(&res[1]);
        guard += 1;
    }
    let end_l = (delay + want).min(out_l.len());
    let mut nl = out_l[delay.min(end_l)..end_l].to_vec();
    let mut nr = out_r[delay.min(end_l)..end_l].to_vec();
    nl.resize(want, 0.0);
    nr.resize(want, 0.0);
    Ok((nl, nr))
}

pub fn peaks(l: &[f32], r: &[f32], columns: usize) -> Vec<(f32, f32)> {
    if l.is_empty() {
        return Vec::new();
    }
    let per = (l.len() as f64 / columns as f64).max(1.0);
    (0..columns)
        .map(|c| {
            let a = (c as f64 * per) as usize;
            let b = (((c + 1) as f64 * per) as usize).min(l.len());
            let mut lo = 0.0f32;
            let mut hi = 0.0f32;
            for i in a..b.max(a + 1).min(l.len()) {
                let s = 0.5 * (l[i] + r[i]);
                lo = lo.min(s);
                hi = hi.max(s);
            }
            (lo, hi)
        })
        .collect()
}

/// Load stems (name, path) into slots at the host rate. Unknown names are skipped.
pub fn load_stem_set(stems: &[(String, String)], host_rate: f32) -> Result<StemSet, String> {
    let to = host_rate.round() as u32;
    let loaded: Vec<Result<(usize, StemAudio), String>> = std::thread::scope(|s| {
        let handles: Vec<_> = stems
            .iter()
            .filter_map(|(name, path)| crate::slot_of(name).map(|slot| (slot, PathBuf::from(path))))
            .map(|(slot, path)| {
                s.spawn(move || -> Result<(usize, StemAudio), String> {
                    let (l, r, sr) = read_wav(&path)?;
                    let (l, r) = resample(l, r, sr, to)?;
                    let peaks = peaks(&l, &r, PEAK_COLUMNS);
                    Ok((slot, StemAudio { l, r, path, peaks }))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err("stem loader crashed".into())))
            .collect()
    });
    let mut set = StemSet {
        slot: Default::default(),
        len: 0,
        sample_rate: host_rate,
    };
    for item in loaded {
        let (slot, audio) = item?;
        set.len = set.len.max(audio.len());
        set.slot[slot] = Some(audio);
    }
    if set.slot.iter().all(|s| s.is_none()) {
        return Err("No stems found".into());
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_length_and_timing() {
        let n = 44100;
        let l: Vec<f32> = (0..n)
            .map(|i| if i == 22050 { 1.0 } else { 0.0 })
            .collect();
        let (a, _) = resample(l.clone(), l, 44100, 48000).unwrap();
        assert_eq!(a.len(), 48000);
        let peak = a
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.abs().partial_cmp(&y.1.abs()).unwrap())
            .unwrap()
            .0;
        assert!((peak as i64 - 24000).abs() <= 2, "impulse moved to {peak}");
    }

    #[test]
    fn wav_roundtrip() {
        let dir = std::env::temp_dir().join("stem_split_test_wav");
        let p = dir.join("t.wav");
        let l = vec![0.25f32; 100];
        let r = vec![-0.5f32; 100];
        write_wav(&p, &l, &r, 48000, 2.0).unwrap();
        let (a, b, sr) = read_wav(&p).unwrap();
        assert_eq!(sr, 48000);
        assert_eq!(a[10], 0.5);
        assert_eq!(b[10], -1.0);
    }
}
