//! Stem Split as a CLAP / VST3 plugin.
//!
//! Audio gets in two ways: drop (or open) an audio file on the window, or press Capture and play
//! the track, which records the plugin's input while the host transport runs. The separation itself
//! runs the same StemSplit.py as the REAPER script (BS-RoFormer SW on MLX) in a separate process, so
//! the plugin never blocks the host. When the stems are back they play inside the plugin, locked to
//! the host timeline, each with its own volume / mute / solo (all automatable), and every stem can be
//! dragged out of the window into the DAW as a WAV file.

use nih_plug::prelude::*;
use std::sync::atomic::Ordering;
use std::sync::Arc;

mod audio_io;
mod dragout;
mod editor;
mod engine;
mod shared;

use shared::{CapState, SessionInfo, Shared};

/// The six stem slots. 4-stem splits use vocals / drums / bass / other.
pub const STEM_SLOTS: [&str; 6] = ["vocals", "drums", "bass", "guitar", "piano", "other"];
pub const STEM_LABELS: [&str; 6] = ["Vocals", "Drums", "Bass", "Guitar", "Piano", "Other"];

pub fn slot_of(name: &str) -> Option<usize> {
    STEM_SLOTS.iter().position(|s| *s == name)
}

#[derive(Params)]
pub struct StemParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<nih_plug_egui::EguiState>,

    /// Session (where the stems live, where they start on the timeline). Saved with the project so
    /// the stems come back when it is reopened.
    #[persist = "session"]
    pub session: Arc<parking_lot::RwLock<SessionInfo>>,

    #[nested(array, group = "Stem")]
    pub stems: [StemChannel; 6],

    #[id = "out"]
    pub output: FloatParam,
}

#[derive(Params)]
pub struct StemChannel {
    #[id = "gain"]
    pub gain: FloatParam,
    #[id = "mute"]
    pub mute: BoolParam,
    #[id = "solo"]
    pub solo: BoolParam,
}

/// Volume in dB, -60 (= off) to +12, linear across the fader so 0 dB sits at 5/6.
fn gain_param(name: &str) -> FloatParam {
    FloatParam::new(name, 0.0, FloatRange::Linear { min: MIN_DB, max: 12.0 })
        .with_smoother(SmoothingStyle::Linear(25.0))
        .with_step_size(0.1)
        .with_unit(" dB")
        .with_value_to_string(Arc::new(|v| {
            if v <= MIN_DB + 0.05 {
                "-inf".to_string()
            } else {
                format!("{v:.1}")
            }
        }))
        .with_string_to_value(Arc::new(|s| {
            let s = s.trim().trim_end_matches("dB").trim();
            if s.eq_ignore_ascii_case("-inf") {
                Some(MIN_DB)
            } else {
                s.parse().ok()
            }
        }))
}

pub const MIN_DB: f32 = -60.0;

/// dB to linear gain, with the bottom of the fader meaning silence.
#[inline]
pub fn fader_gain(db: f32) -> f32 {
    if db <= MIN_DB + 0.05 {
        0.0
    } else {
        util::db_to_gain_fast(db)
    }
}

impl Default for StemParams {
    fn default() -> Self {
        Self {
            editor_state: nih_plug_egui::EguiState::from_size(editor::WIDTH, editor::HEIGHT),
            session: Arc::new(parking_lot::RwLock::new(SessionInfo::default())),
            stems: std::array::from_fn(|i| StemChannel {
                gain: gain_param(&format!("{} Volume", STEM_LABELS[i])),
                mute: BoolParam::new(format!("{} Mute", STEM_LABELS[i]), false),
                solo: BoolParam::new(format!("{} Solo", STEM_LABELS[i]), false),
            }),
            output: gain_param("Output"),
        }
    }
}

pub struct StemSplit {
    params: Arc<StemParams>,
    shared: Arc<Shared>,
    sample_rate: f32,
    mono_in: bool,
    /// The audio thread's own handle on the current stems (see `Shared::try_stems`).
    stems: Option<Arc<shared::StemSet>>,
    stems_gen: u64,
    /// Per-slot smoothed gain target including mute / solo, so toggles don't click.
    slot_gain: [f32; 6],
}

impl Default for StemSplit {
    fn default() -> Self {
        let params = Arc::new(StemParams::default());
        let shared = Arc::new(Shared::new(params.session.clone()));
        Self {
            params,
            shared,
            sample_rate: 44100.0,
            mono_in: false,
            stems: None,
            stems_gen: u64::MAX,
            slot_gain: [1.0; 6],
        }
    }
}

impl Plugin for StemSplit {
    const NAME: &'static str = "Stem Split";
    const VENDOR: &'static str = "Bryan Chi";
    const URL: &'static str = "https://github.com/BryanChi/Stem-Split";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
    ];

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone())
    }

    fn initialize(
        &mut self,
        layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.mono_in = layout.main_input_channels.map(|c| c.get()) == Some(1);
        self.sample_rate = buffer_config.sample_rate;
        self.shared.set_host_rate(buffer_config.sample_rate);
        // Bring back the stems saved with the project (or reload them at a new sample rate).
        engine::reload_session_if_needed(&self.shared);
        // Test hook (debug builds only): split this file on start.
        #[cfg(debug_assertions)]
        if let Some(f) = std::env::var_os("STEM_SPLIT_DEBUG_FILE") {
            if self.shared.stems().is_none() {
                engine::start(&self.shared, engine::Source::File(f.into()), 6);
            }
        }
        true
    }

    fn reset(&mut self) {
        self.slot_gain = std::array::from_fn(|i| self.target_gain(i));
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let shared = &*self.shared;
        let transport = context.transport();
        let playing = transport.playing;
        let host_pos = transport.pos_samples().unwrap_or(0);
        let n = buffer.samples();
        shared.host_playing.store(playing, Ordering::Relaxed);
        shared.host_pos.store(host_pos, Ordering::Relaxed);

        // ---- capture: record the input while the transport plays ----
        match shared.cap_state() {
            CapState::Armed if playing => {
                if let Some(mut cap) = shared.capture.try_lock() {
                    cap.start_pos = host_pos;
                    cap.len = 0;
                    cap.write(buffer);
                    shared.cap_len.store(cap.len, Ordering::Relaxed);
                    shared.set_cap_state(CapState::Recording);
                }
            }
            CapState::Recording => {
                let stop = !playing || shared.cap_stop.swap(false, Ordering::Relaxed);
                if let Some(mut cap) = shared.capture.try_lock() {
                    if !stop {
                        let full = !cap.write(buffer);
                        shared.cap_len.store(cap.len, Ordering::Relaxed);
                        if full {
                            shared.set_cap_state(CapState::Done);
                        }
                    } else {
                        shared.set_cap_state(CapState::Done);
                    }
                } else if stop {
                    shared.set_cap_state(CapState::Done);
                }
            }
            _ => {}
        }

        // ---- stem playback ----
        let gen = shared.stems_gen.load(Ordering::Acquire);
        if gen != self.stems_gen {
            if let Some(s) = shared.try_stems() {
                self.stems = s;
                self.stems_gen = gen;
            }
        }
        let shared = &*self.shared;
        let Some(set) = self.stems.as_deref() else {
            shared.play_pos.store(-1, Ordering::Relaxed);
            self.ensure_stereo(buffer);
            return ProcessStatus::Normal;
        };
        if shared.bypass.load(Ordering::Relaxed) || shared.cap_state() != CapState::Idle {
            shared.play_pos.store(-1, Ordering::Relaxed);
            self.ensure_stereo(buffer);
            return ProcessStatus::Normal;
        }

        // Where in the stems this block starts. Host transport wins; the Preview button plays
        // from the plugin's own position while the host is stopped.
        let start: Option<i64> = if playing {
            shared.preview.store(false, Ordering::Relaxed);
            Some(host_pos - shared.origin_samples.load(Ordering::Relaxed))
        } else if shared.preview.load(Ordering::Relaxed) {
            let p = shared.preview_pos.fetch_add(n as i64, Ordering::Relaxed);
            if p >= set.len as i64 {
                shared.preview.store(false, Ordering::Relaxed);
                shared.preview_pos.store(0, Ordering::Relaxed);
            }
            Some(p)
        } else {
            None
        };
        shared
            .play_pos
            .store(start.unwrap_or(-1), Ordering::Relaxed);

        let targets: [f32; 6] = std::array::from_fn(|i| self.target_gain(i));
        let outs = buffer.as_slice();
        let (left, rest) = outs.split_at_mut(1);
        mix(
            &self.params,
            &mut self.slot_gain,
            targets,
            self.sample_rate,
            set,
            start,
            &mut left[0][..n],
            rest.get_mut(0).map(|r| &mut r[..n]),
        );
        ProcessStatus::Normal
    }
}

/// Sum the stems for one block starting at stem position `start` (None = silence).
#[allow(clippy::too_many_arguments)]
fn mix(
    params: &StemParams,
    slot_gain: &mut [f32; 6],
    targets: [f32; 6],
    sample_rate: f32,
    set: &shared::StemSet,
    start: Option<i64>,
    left: &mut [f32],
    mut right: Option<&mut [f32]>,
) {
    // ~10 ms one-pole glide for mute / solo flips; the volume params have their own smoother.
    let glide = 1.0 - (-1.0 / (0.010 * sample_rate)).exp();
    for i in 0..left.len() {
        let mut l = 0.0f32;
        let mut r = 0.0f32;
        let out_gain = fader_gain(params.output.smoothed.next());
        let pos = start.map(|s| s + i as i64);
        for slot in 0..6 {
            let g_param = fader_gain(params.stems[slot].gain.smoothed.next());
            slot_gain[slot] += (targets[slot] - slot_gain[slot]) * glide;
            let (Some(stem), Some(p)) = (set.slot[slot].as_ref(), pos) else {
                continue;
            };
            if p < 0 || p as usize >= stem.len() {
                continue;
            }
            let g = g_param * slot_gain[slot];
            l += stem.l[p as usize] * g;
            r += stem.r[p as usize] * g;
        }
        left[i] = l * out_gain;
        if let Some(right) = right.as_mut() {
            right[i] = r * out_gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn stem(v: &[f32]) -> Option<shared::StemAudio> {
        Some(shared::StemAudio { l: v.to_vec(), r: v.iter().map(|x| -x).collect(), path: PathBuf::new(), peaks: vec![] })
    }

    #[test]
    fn mixes_at_timeline_position_with_gains() {
        let p = StemSplit::default();
        // the host wrapper does this on activation
        p.params.output.smoothed.reset(0.0);
        for c in &p.params.stems {
            c.gain.smoothed.reset(0.0);
        }
        let mut set = shared::StemSet { slot: Default::default(), len: 6, sample_rate: 48000.0 };
        set.slot[0] = stem(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        set.slot[5] = stem(&[10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);
        let mut g = [1.0; 6];
        let mut l = [0.0f32; 4];
        let mut r = [0.0f32; 4];
        // block starts 2 samples before stem sample 0
        mix(&p.params, &mut g, [1.0; 6], 48000.0, &set, Some(-2), &mut l, Some(&mut r));
        assert_eq!(l, [0.0, 0.0, 11.0, 22.0]);
        assert_eq!(r, [0.0, 0.0, -11.0, -22.0]);
        // "other" muted (target 0, already settled) -> vocals only; past the end -> silence
        let mut g = [1.0, 1.0, 1.0, 1.0, 1.0, 0.0];
        mix(&p.params, &mut g, [1.0, 1.0, 1.0, 1.0, 1.0, 0.0], 48000.0, &set, Some(4), &mut l, None);
        assert_eq!(l, [5.0, 6.0, 0.0, 0.0]);
        // no position -> silence
        mix(&p.params, &mut g, [1.0; 6], 48000.0, &set, None, &mut l, None);
        assert_eq!(l, [0.0; 4]);
    }
}

impl StemSplit {
    /// Mono-in / stereo-out layout: copy the left channel to the right so passthrough isn't one-sided.
    fn ensure_stereo(&self, buffer: &mut Buffer) {
        if !self.mono_in {
            return;
        }
        let ch = buffer.as_slice();
        if ch.len() >= 2 {
            let (a, b) = ch.split_at_mut(1);
            b[0].copy_from_slice(a[0]);
        }
    }

    fn target_gain(&self, slot: usize) -> f32 {
        let any_solo = self.params.stems.iter().any(|s| s.solo.value());
        let ch = &self.params.stems[slot];
        if ch.mute.value() || (any_solo && !ch.solo.value()) {
            0.0
        } else {
            1.0
        }
    }
}

impl ClapPlugin for StemSplit {
    const CLAP_ID: &'static str = "com.bryanchi.stem-split";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Separate audio into stems, mix them inside the plugin, drag them into the DAW");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Utility,
        ClapFeature::Analyzer,
    ];
}

impl Vst3Plugin for StemSplit {
    const VST3_CLASS_ID: [u8; 16] = *b"BryanChiStemSplt";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nih_export_clap!(StemSplit);
nih_export_vst3!(StemSplit);
