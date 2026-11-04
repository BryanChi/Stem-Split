//! Running the separator. The model (BS-RoFormer SW on MLX) lives in a Python venv, the same one
//! the REAPER script uses; StemSplit.py is built into the plugin and written next to its settings.

use crate::audio_io;
use crate::shared::{CapState, SessionInfo, Shared};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SCRIPT: &str = include_str!("../../StemSplit.py");

/// Python packages the engine needs (same list the REAPER script asks for).
pub const PIP_PACKAGES: &[&str] = &["mlx-audio-separator", "torch>=2.6", "soundfile"];

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Settings + engine folder: ~/Library/Application Support/Stem Split on macOS.
pub fn app_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/Stem Split")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
            .join("Stem Split")
    } else {
        home().join(".local/share/stem-split")
    }
}

/// Where splits are kept (the DAW references these files after a drag, so they must stay put).
pub fn sessions_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Music/Stem Split")
    } else {
        home().join("Documents/Stem Split")
    }
}

fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(target_os = "windows") {
        venv.join("Scripts/python.exe")
    } else {
        venv.join("bin/python")
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Config {
    python: Option<String>,
}

fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

fn read_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn set_python(path: &Path) {
    let _ = std::fs::create_dir_all(app_dir());
    let cfg = Config {
        python: Some(path.to_string_lossy().into_owned()),
    };
    let _ = std::fs::write(config_path(), serde_json::to_string_pretty(&cfg).unwrap_or_default());
}

/// The Python that runs the separator: the configured one, the plugin's own venv, or the
/// .venv-stems the REAPER script set up under REAPER/Scripts.
pub fn find_python() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("STEM_SPLIT_PYTHON").map(PathBuf::from) {
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(p) = read_config().python.map(PathBuf::from) {
        if p.exists() {
            return Some(p);
        }
    }
    let own = venv_python(&app_dir().join(".venv-stems"));
    if own.exists() {
        return Some(own);
    }
    let scripts = if cfg!(target_os = "macos") {
        home().join("Library/Application Support/REAPER/Scripts")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
            .join("REAPER/Scripts")
    } else {
        home().join(".config/REAPER/Scripts")
    };
    for d in ["Bryan's Scripts", "Stem Split", "CoolReaperScripts/Stem Split"] {
        let p = venv_python(&scripts.join(d).join(".venv-stems"));
        if p.exists() {
            return Some(p);
        }
    }
    // any other folder one or two levels down
    let mut dirs = vec![scripts.clone()];
    if let Ok(rd) = std::fs::read_dir(&scripts) {
        dirs.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = venv_python(&e.path().join(".venv-stems"));
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Write the built-in StemSplit.py (only when it changed).
fn script_path() -> Result<PathBuf, String> {
    let dir = app_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join("StemSplit.py");
    if std::fs::read_to_string(&p).ok().as_deref() != Some(SCRIPT) {
        std::fs::write(&p, SCRIPT).map_err(|e| e.to_string())?;
    }
    Ok(p)
}

pub fn new_session_dir(name: &str) -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let clean: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .take(48)
        .collect();
    let base = sessions_dir().join(format!("{} {}", clean.trim(), stamp(secs)));
    let mut p = base.clone();
    let mut k = 2;
    while p.exists() {
        p = PathBuf::from(format!("{} ({k})", base.display()));
        k += 1;
    }
    p
}

/// "2026-11-04 0218" in UTC, without pulling in a date crate.
fn stamp(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}{:02}{:02}", rem / 3600, (rem / 60) % 60, rem % 60)
}

/// Load the stems listed in the session at the host rate, in the background.
pub fn reload_session_if_needed(shared: &Arc<Shared>) {
    let sess = shared.session.read().clone();
    if sess.stems.is_empty() {
        return;
    }
    let rate = shared.host_rate();
    if let Some(cur) = shared.stems().as_ref() {
        if (cur.sample_rate - rate).abs() < 0.5 {
            return;
        }
    }
    if shared.job().running {
        return;
    }
    let shared = shared.clone();
    shared.set_job(|j| {
        *j = crate::shared::JobStatus {
            running: true,
            percent: 100.0,
            message: "Loading stems…".into(),
            error: None,
        }
    });
    std::thread::spawn(move || {
        let res = audio_io::load_stem_set(&sess.stems, rate);
        match res {
            Ok(set) => {
                shared.store_stems(Some(Arc::new(set)));
                shared.set_job(|j| {
                    j.running = false;
                    j.message.clear();
                });
            }
            Err(e) => shared.set_job(|j| {
                j.running = false;
                j.error = Some(format!("Couldn't load the saved stems: {e}"));
            }),
        }
    });
}

/// What to split.
pub enum Source {
    /// An audio file on disk (any format the separator reads: wav, mp3, flac, aiff, m4a…).
    File(PathBuf),
    /// The captured input, which gets written to the session folder first.
    Capture,
}

/// Start separating in the background. Progress lands in `shared.job`.
pub fn start(shared: &Arc<Shared>, source: Source, stem_count: u8) {
    if shared.job().running {
        return;
    }
    shared.cancel.store(false, Ordering::Relaxed);
    shared.set_job(|j| {
        *j = crate::shared::JobStatus {
            running: true,
            percent: 1.0,
            message: "Starting…".into(),
            error: None,
        }
    });
    let shared = shared.clone();
    std::thread::spawn(move || {
        let res = run(&shared, source, stem_count);
        *shared.child.lock() = None;
        match res {
            Ok(()) => shared.set_job(|j| {
                j.running = false;
                j.percent = 100.0;
                j.message.clear();
            }),
            Err(e) => {
                let cancelled = shared.cancel.load(Ordering::Relaxed);
                shared.set_job(|j| {
                    j.running = false;
                    j.error = if cancelled { None } else { Some(e) };
                    j.message.clear();
                })
            }
        }
    });
}

pub fn cancel(shared: &Shared) {
    shared.cancel.store(true, Ordering::Relaxed);
    if let Some(child) = shared.child.lock().as_mut() {
        let _ = child.kill();
    }
}

fn run(shared: &Arc<Shared>, source: Source, stem_count: u8) -> Result<(), String> {
    let python = find_python().ok_or_else(|| {
        "The separation engine isn't installed. Press Install engine (or pick an existing Python \
         with Choose Python)."
            .to_string()
    })?;
    let script = script_path()?;

    // Input file + session folder
    let host_pos_sec;
    let (input, dir, source_name) = match source {
        Source::File(p) => {
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Audio".into());
            let dir = new_session_dir(&name);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            // Stems start where the playhead is when the file is dropped.
            let sr = shared.host_rate().max(1.0) as f64;
            host_pos_sec = shared.host_pos.load(Ordering::Relaxed).max(0) as f64 / sr;
            (p, dir, name)
        }
        Source::Capture => {
            let dir = new_session_dir("Captured");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            shared.set_job(|j| j.message = "Saving captured audio…".into());
            let wav = dir.join("captured.wav");
            let (sr, start) = {
                let cap = shared.capture.lock();
                if cap.len < (cap.sample_rate * 0.5) as usize {
                    drop(cap);
                    shared.cancel_capture();
                    return Err("Captured audio is too short (play at least half a second).".into());
                }
                audio_io::write_wav(
                    &wav,
                    &cap.l[..cap.len],
                    &cap.r[..cap.len],
                    cap.sample_rate.round() as u32,
                    1.0,
                )?;
                (cap.sample_rate, cap.start_pos)
            };
            shared.cancel_capture(); // frees the buffer, back to Idle
            host_pos_sec = start as f64 / sr as f64;
            (wav, dir, "Captured audio".to_string())
        }
    };

    let progress = dir.join("_progress.txt");
    let done = dir.join("_done.txt");
    let manifest = dir.join("manifest.txt");
    let log = dir.join("_log.txt");
    let log_file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&python);
    cmd.arg(&script)
        .arg("--input")
        .arg(&input)
        .arg("--output-dir")
        .arg(&dir)
        .arg("--stems")
        .arg(stem_count.to_string())
        .arg("--merge-empty")
        .arg("--progress-file")
        .arg(&progress)
        .arg("--done-flag")
        .arg(&done)
        .arg("--manifest")
        .arg(&manifest)
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file));
    if let Some(dir) = script.parent() {
        cmd.current_dir(dir);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("Couldn't start {}: {e}", python.display()))?;
    *shared.child.lock() = Some(child);

    let started = Instant::now();
    let status = loop {
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(text) = std::fs::read_to_string(&progress) {
            let mut lines = text.lines();
            let pct: f32 = lines.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0.0);
            let msg = lines.next().unwrap_or("").to_string();
            shared.set_job(|j| {
                j.percent = pct.max(1.0);
                j.message = msg;
            });
        } else if started.elapsed() > Duration::from_secs(2) {
            shared.set_job(|j| j.message = "Starting Python…".into());
        }
        let mut guard = shared.child.lock();
        let Some(child) = guard.as_mut() else {
            return Err("cancelled".into());
        };
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
    };
    if shared.cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("cancelled".into());
    }
    if !status.success() {
        return Err(error_from_log(&log).unwrap_or_else(|| format!("Separator exited with {status}")));
    }

    let (stems, merged) = parse_manifest(&manifest)?;
    shared.set_job(|j| {
        j.percent = 100.0;
        j.message = "Loading stems…".into();
    });
    let set = audio_io::load_stem_set(&stems, shared.host_rate())?;
    {
        let mut s = shared.session.write();
        *s = SessionInfo {
            dir: Some(dir.to_string_lossy().into_owned()),
            source_name,
            stems,
            merged,
            origin_sec: host_pos_sec,
            stem_count,
            bypass: false,
        };
    }
    shared.bypass.store(false, Ordering::Relaxed);
    shared.set_origin_sec(host_pos_sec);
    shared.preview_pos.store(0, Ordering::Relaxed);
    shared.store_stems(Some(Arc::new(set)));
    for f in [&progress, &done] {
        let _ = std::fs::remove_file(f);
    }
    Ok(())
}

type Pairs = Vec<(String, String)>;

pub fn parse_manifest(path: &Path) -> Result<(Pairs, Pairs), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("No manifest: {e}"))?;
    let mut stems = Vec::new();
    let mut merged = Vec::new();
    let mut warning = String::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(3, '|').collect();
        match parts.as_slice() {
            ["STEM", name, path] => stems.push((name.to_string(), path.to_string())),
            ["MERGED", name, into] => merged.push((name.to_string(), into.to_string())),
            ["WARNING", w] => warning = w.to_string(),
            _ => {}
        }
    }
    if stems.is_empty() {
        return Err(if warning.is_empty() {
            "The separator returned no stems.".into()
        } else {
            warning
        });
    }
    Ok((stems, merged))
}

/// StemSplit.py prints {"ok": false, "error": "..."} on failure; otherwise use the last line.
fn error_from_log(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    if let Some(i) = text.find("\"error\":") {
        let rest = &text[i + 8..];
        if let Some(v) = serde_json::from_str::<serde_json::Value>(&format!(
            "{{\"error\":{}}}",
            rest.split("\n").next().unwrap_or("").trim_end_matches(',')
        ))
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        {
            return Some(v);
        }
    }
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().chars().take(300).collect())
}

/// Install the engine into the plugin's own venv. Runs in the background; progress in `shared.job`.
pub fn install(shared: &Arc<Shared>) {
    if shared.job().running {
        return;
    }
    shared.set_job(|j| {
        *j = crate::shared::JobStatus {
            running: true,
            percent: 2.0,
            message: "Looking for Python 3.10+…".into(),
            error: None,
        }
    });
    let shared = shared.clone();
    std::thread::spawn(move || {
        let res = install_inner(&shared);
        shared.set_job(|j| {
            j.running = false;
            j.message.clear();
            j.error = res.err();
        });
    });
}

fn base_python() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    for v in ["3.13", "3.12", "3.11", "3.10"] {
        for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/Library/Frameworks/Python.framework/Versions"] {
            if dir.ends_with("Versions") {
                cands.push(PathBuf::from(format!("{dir}/{v}/bin/python{v}")));
            } else {
                cands.push(PathBuf::from(format!("{dir}/python{v}")));
            }
        }
    }
    cands.push(PathBuf::from("/opt/homebrew/bin/python3"));
    cands.push(PathBuf::from("/usr/local/bin/python3"));
    for c in cands {
        if c.exists() && python_ok(&c) {
            return Some(c);
        }
    }
    for name in ["python3", "python"] {
        let p = PathBuf::from(name);
        if python_ok(&p) {
            return Some(p);
        }
    }
    None
}

fn python_ok(p: &Path) -> bool {
    Command::new(p)
        .args(["-c", "import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn install_inner(shared: &Shared) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("The separation engine (MLX) runs on Apple-silicon Macs only for now.".into());
    }
    let base = base_python().ok_or(
        "No Python 3.10+ found. Install one (brew install python@3.13 or python.org), then press Install engine again.",
    )?;
    let venv = app_dir().join(".venv-stems");
    std::fs::create_dir_all(app_dir()).map_err(|e| e.to_string())?;
    shared.set_job(|j| {
        j.percent = 8.0;
        j.message = "Creating the Python environment…".into();
    });
    let st = Command::new(&base)
        .args(["-m", "venv"])
        .arg(&venv)
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("python -m venv failed".into());
    }
    let py = venv_python(&venv);
    shared.set_job(|j| {
        j.percent = 15.0;
        j.message = "Downloading the separator (MLX, torch; a few minutes)…".into();
    });
    let log = app_dir().join("install_log.txt");
    let out = Command::new(&py)
        .args(["-m", "pip", "install", "--upgrade", "pip"])
        .output()
        .map_err(|e| e.to_string())?;
    let _ = std::fs::write(&log, &out.stderr);
    let out = Command::new(&py)
        .args(["-m", "pip", "install"])
        .args(PIP_PACKAGES)
        .output()
        .map_err(|e| e.to_string())?;
    let _ = std::fs::write(&log, [&out.stdout[..], &out.stderr[..]].concat());
    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr)
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .to_string();
        return Err(format!("pip install failed: {tail}"));
    }
    set_python(&py);
    Ok(())
}

/// Called by the editor every frame: a finished capture starts the split.
pub fn poll_capture(shared: &Arc<Shared>, stem_count: u8) {
    if shared.cap_state() == CapState::Done && !shared.job().running {
        start(shared, Source::Capture, stem_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_is_a_date() {
        assert_eq!(stamp(0), "1970-01-01 000000");
        assert_eq!(stamp(1_793_758_713), "2026-11-04 021833");
    }

    #[test]
    fn manifest() {
        let p = std::env::temp_dir().join("stem_split_manifest_test.txt");
        std::fs::write(
            &p,
            "OK|1\nENGINE|x\nWARNING|\nSTEM|vocals|/a/vocals.wav\nSTEM|other|/a/o|ther.wav\nMERGED|piano|other\n",
        )
        .unwrap();
        let (s, m) = parse_manifest(&p).unwrap();
        assert_eq!(s[1], ("other".into(), "/a/o|ther.wav".into()));
        assert_eq!(m[0], ("piano".into(), "other".into()));
    }
}

/// End-to-end split with a stand-in separator. Run with
/// STEM_SPLIT_PYTHON=<fake python> STEM_SPLIT_TEST_INPUT=<audio> cargo test -- --ignored
#[cfg(test)]
mod e2e {
    use super::*;

    #[test]
    #[ignore]
    fn split_file_end_to_end() {
        let input = PathBuf::from(std::env::var("STEM_SPLIT_TEST_INPUT").expect("STEM_SPLIT_TEST_INPUT"));
        let shared = Arc::new(Shared::new(Arc::new(parking_lot::RwLock::new(SessionInfo::default()))));
        shared.set_host_rate(44100.0);
        shared.host_pos.store(44100 * 3, Ordering::Relaxed); // playhead at 0:03
        start(&shared, Source::File(input), 6);
        let t = Instant::now();
        while shared.job().running && t.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let job = shared.job();
        assert!(job.error.is_none(), "{:?}", job.error);
        let sess = shared.session.read().clone();
        assert_eq!(sess.stem_count, 6);
        assert!((sess.origin_sec - 3.0).abs() < 1e-9);
        assert_eq!(shared.origin_samples.load(Ordering::Relaxed), 44100 * 3);
        assert!(sess.merged.iter().any(|(n, _)| n == "piano"));
        let set = shared.stems().expect("stems loaded");
        assert_eq!(set.sample_rate, 44100.0);
        // 6 s at 48 kHz in, resampled to the 44.1 kHz host
        assert!((set.len as i64 - 6 * 44100).abs() <= 1, "len {}", set.len);
        assert!(set.slot[crate::slot_of("piano").unwrap()].is_none());
        // stems sum back to (roughly) the mix
        let energy: f32 = set.slot.iter().flatten().map(|s| s.l.iter().map(|x| x * x).sum::<f32>()).sum();
        assert!(energy > 1.0);
        eprintln!("session: {sess:?}");
    }
}
