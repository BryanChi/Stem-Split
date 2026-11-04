//! State shared between the audio thread, the editor and the background workers.

use nih_plug::prelude::{AtomicF32, Buffer};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

/// Longest capture: 15 minutes. The buffer is zero-allocated, so untouched pages cost nothing.
pub const MAX_CAPTURE_SECONDS: f32 = 15.0 * 60.0;

/// What gets saved with the project.
#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct SessionInfo {
    /// Folder holding the stems (and the captured / source audio).
    pub dir: Option<String>,
    /// Display name of what was split ("Captured audio", or the dropped file's name).
    pub source_name: String,
    /// (stem name, wav path) in display order.
    pub stems: Vec<(String, String)>,
    /// Stems the separator found nearly empty and blended into another one: (name, into).
    #[serde(default)]
    pub merged: Vec<(String, String)>,
    /// Where stem sample 0 sits on the host timeline, in seconds.
    pub origin_sec: f64,
    /// 4 or 6.
    #[serde(default = "default_stem_count")]
    pub stem_count: u8,
    #[serde(default)]
    pub bypass: bool,
}

fn default_stem_count() -> u8 {
    4
}

pub struct StemAudio {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub path: PathBuf,
    /// Min / max per display column, for the waveform.
    pub peaks: Vec<(f32, f32)>,
}

impl StemAudio {
    pub fn len(&self) -> usize {
        self.l.len()
    }
}

/// Decoded stems at the host sample rate, indexed by slot (see `STEM_SLOTS`).
pub struct StemSet {
    pub slot: [Option<StemAudio>; 6],
    pub len: usize,
    pub sample_rate: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CapState {
    Idle = 0,
    /// Waiting for the transport to start.
    Armed = 1,
    Recording = 2,
    /// Finished recording; the editor picks it up and starts separating.
    Done = 3,
}

pub struct Capture {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub len: usize,
    pub start_pos: i64,
    pub sample_rate: f32,
}

impl Capture {
    /// Append the block; false once the buffer is full.
    pub fn write(&mut self, buffer: &Buffer) -> bool {
        let ch = buffer.as_slice_immutable();
        let n = buffer.samples();
        let room = self.l.len().saturating_sub(self.len);
        let take = n.min(room);
        if take > 0 {
            let l = &ch[0][..take];
            let r = if ch.len() > 1 { &ch[1][..take] } else { l };
            self.l[self.len..self.len + take].copy_from_slice(l);
            self.r[self.len..self.len + take].copy_from_slice(r);
            self.len += take;
        }
        take == n
    }
}

#[derive(Clone, Default)]
pub struct JobStatus {
    pub running: bool,
    pub percent: f32,
    pub message: String,
    pub error: Option<String>,
}

pub struct Shared {
    pub session: Arc<RwLock<SessionInfo>>,
    /// The current stems. The audio thread takes its own Arc when `stems_gen` changes (try_lock,
    /// never blocks); replaced sets wait in `retired` so they are never freed on the audio thread.
    stems_slot: Mutex<Option<Arc<StemSet>>>,
    pub stems_gen: AtomicU64,
    retired: Mutex<Vec<Arc<StemSet>>>,
    pub host_rate: AtomicF32,

    pub host_playing: AtomicBool,
    pub host_pos: AtomicI64,
    /// Host sample position of stem sample 0.
    pub origin_samples: AtomicI64,
    /// Stem position the audio thread last played (-1 when silent), for the playhead.
    pub play_pos: AtomicI64,
    pub preview: AtomicBool,
    pub preview_pos: AtomicI64,
    pub bypass: AtomicBool,

    cap_state: AtomicU8,
    pub cap_stop: AtomicBool,
    pub cap_len: AtomicUsize,
    pub capture: Mutex<Capture>,

    pub job: Mutex<JobStatus>,
    pub child: Mutex<Option<std::process::Child>>,
    pub cancel: AtomicBool,
    /// Set while a stem is being dragged out, so dropping it back on our own window is ignored.
    pub dragging_out: AtomicBool,
}

impl Shared {
    pub fn new(session: Arc<RwLock<SessionInfo>>) -> Self {
        Self {
            session,
            stems_slot: Mutex::new(None),
            stems_gen: AtomicU64::new(0),
            retired: Mutex::new(Vec::new()),
            host_rate: AtomicF32::new(44100.0),
            host_playing: AtomicBool::new(false),
            host_pos: AtomicI64::new(0),
            origin_samples: AtomicI64::new(0),
            play_pos: AtomicI64::new(-1),
            preview: AtomicBool::new(false),
            preview_pos: AtomicI64::new(0),
            bypass: AtomicBool::new(false),
            cap_state: AtomicU8::new(CapState::Idle as u8),
            cap_stop: AtomicBool::new(false),
            cap_len: AtomicUsize::new(0),
            capture: Mutex::new(Capture {
                l: Vec::new(),
                r: Vec::new(),
                len: 0,
                start_pos: 0,
                sample_rate: 44100.0,
            }),
            job: Mutex::new(JobStatus::default()),
            child: Mutex::new(None),
            cancel: AtomicBool::new(false),
            dragging_out: AtomicBool::new(false),
        }
    }

    /// The current stems (not for the audio thread: see `try_stems`).
    pub fn stems(&self) -> Option<Arc<StemSet>> {
        self.stems_slot.lock().clone()
    }

    /// Audio thread: the current stems if the lock is free right now.
    pub fn try_stems(&self) -> Option<Option<Arc<StemSet>>> {
        self.stems_slot.try_lock().map(|g| g.clone())
    }

    pub fn store_stems(&self, set: Option<Arc<StemSet>>) {
        let old = std::mem::replace(&mut *self.stems_slot.lock(), set);
        if let Some(old) = old {
            self.retired.lock().push(old);
        }
        self.stems_gen.fetch_add(1, Ordering::AcqRel);
        self.free_retired();
    }

    /// Drop replaced stem sets the audio thread has let go of (call from a non-audio thread).
    pub fn free_retired(&self) {
        self.retired.lock().retain(|a| Arc::strong_count(a) > 1);
    }

    pub fn cap_state(&self) -> CapState {
        match self.cap_state.load(Ordering::Acquire) {
            1 => CapState::Armed,
            2 => CapState::Recording,
            3 => CapState::Done,
            _ => CapState::Idle,
        }
    }

    pub fn set_cap_state(&self, s: CapState) {
        self.cap_state.store(s as u8, Ordering::Release);
    }

    pub fn host_rate(&self) -> f32 {
        self.host_rate.load(Ordering::Relaxed)
    }

    pub fn set_host_rate(&self, sr: f32) {
        self.host_rate.store(sr, Ordering::Relaxed);
        let origin = self.session.read().origin_sec;
        self.origin_samples
            .store((origin * sr as f64).round() as i64, Ordering::Relaxed);
        self.bypass
            .store(self.session.read().bypass, Ordering::Relaxed);
    }

    pub fn set_origin_sec(&self, sec: f64) {
        self.session.write().origin_sec = sec;
        let sr = self.host_rate() as f64;
        self.origin_samples
            .store((sec * sr).round() as i64, Ordering::Relaxed);
    }

    /// Arm a new capture (allocates on the calling thread, never on the audio thread).
    pub fn arm_capture(&self) {
        let sr = self.host_rate();
        let frames = (MAX_CAPTURE_SECONDS * sr) as usize;
        {
            let mut cap = self.capture.lock();
            if cap.l.len() != frames {
                cap.l = vec![0.0; frames];
                cap.r = vec![0.0; frames];
            }
            cap.len = 0;
            cap.start_pos = 0;
            cap.sample_rate = sr;
        }
        self.cap_len.store(0, Ordering::Relaxed);
        self.cap_stop.store(false, Ordering::Relaxed);
        self.set_cap_state(CapState::Armed);
    }

    pub fn cancel_capture(&self) {
        self.set_cap_state(CapState::Idle);
        let mut cap = self.capture.lock();
        cap.l = Vec::new();
        cap.r = Vec::new();
        cap.len = 0;
    }

    pub fn job(&self) -> JobStatus {
        self.job.lock().clone()
    }

    pub fn set_job(&self, f: impl FnOnce(&mut JobStatus)) {
        f(&mut self.job.lock());
    }
}
