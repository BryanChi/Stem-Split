//! The plugin window (egui).

use crate::dragout;
use crate::engine::{self, Source};
use crate::shared::{CapState, SessionInfo, Shared};
use crate::{StemParams, STEM_LABELS, STEM_SLOTS};
use nih_plug::prelude::{Param, ParamSetter};
use nih_plug_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const WIDTH: u32 = 840;
pub const HEIGHT: u32 = 470;

const BG: Color32 = Color32::from_rgb(0x15, 0x16, 0x19);
const PANEL: Color32 = Color32::from_rgb(0x1e, 0x20, 0x24);
const PANEL_HI: Color32 = Color32::from_rgb(0x2a, 0x2d, 0x33);
const TEXT: Color32 = Color32::from_rgb(0xe4, 0xe6, 0xea);
const DIM: Color32 = Color32::from_rgb(0x8a, 0x8f, 0x99);
const ACCENT: Color32 = Color32::from_rgb(0x6e, 0xa8, 0xff);
const RED: Color32 = Color32::from_rgb(0xff, 0x5a, 0x5a);
const YELLOW: Color32 = Color32::from_rgb(0xf2, 0xc9, 0x4c);

const STEM_COLORS: [Color32; 6] = [
    Color32::from_rgb(0xff, 0x6b, 0x9a), // vocals
    Color32::from_rgb(0xff, 0xa8, 0x4d), // drums
    Color32::from_rgb(0xa0, 0x84, 0xff), // bass
    Color32::from_rgb(0x5e, 0xd3, 0x9a), // guitar
    Color32::from_rgb(0x4c, 0xc3, 0xff), // piano
    Color32::from_rgb(0xd8, 0xcf, 0x82), // other
];

pub struct UiState {
    python: Option<PathBuf>,
    python_checked: Option<Instant>,
    /// A drag-out is in progress (ignore drops onto our own window until it ends).
    drag_started_at: Option<Instant>,
}

pub fn create(
    params: Arc<StemParams>,
    shared: Arc<Shared>,
) -> Option<Box<dyn nih_plug::prelude::Editor>> {
    let state = UiState {
        python: None,
        python_checked: None,
        drag_started_at: None,
    };
    nih_plug_egui::create_egui_editor(
        params.editor_state.clone(),
        state,
        |ctx, _| {
            let mut v = egui::Visuals::dark();
            v.panel_fill = BG;
            v.window_fill = BG;
            v.override_text_color = Some(TEXT);
            v.widgets.inactive.weak_bg_fill = PANEL_HI;
            v.widgets.inactive.bg_fill = PANEL_HI;
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x35, 0x39, 0x41);
            v.widgets.active.weak_bg_fill = Color32::from_rgb(0x40, 0x45, 0x4f);
            v.selection.bg_fill = ACCENT;
            ctx.set_visuals(v);
            let mut style = (*ctx.style()).clone();
            style.spacing.button_padding = Vec2::new(10.0, 5.0);
            style.spacing.item_spacing = Vec2::new(8.0, 6.0);
            ctx.set_style(style);
        },
        move |ctx, setter, state| {
            draw(ctx, setter, state, &params, &shared);
        },
    )
}

fn fmt_time(sec: f64) -> String {
    let neg = sec < 0.0;
    let s = sec.abs();
    let m = (s / 60.0).floor() as i64;
    let r = s - m as f64 * 60.0;
    format!("{}{}:{:05.2}", if neg { "-" } else { "" }, m, r)
}

fn draw(
    ctx: &egui::Context,
    setter: &ParamSetter,
    state: &mut UiState,
    params: &Arc<StemParams>,
    shared: &Arc<Shared>,
) {
    ctx.request_repaint_after(Duration::from_millis(33));
    shared.free_retired();
    let session = shared.session.read().clone();
    let stem_count = if session.stem_count == 6 { 6 } else { 4 };
    engine::poll_capture(shared, stem_count);
    let job = shared.job();

    // Engine (Python) lookup, rechecked every few seconds and after jobs.
    let stale = state
        .python_checked
        .is_none_or(|t| t.elapsed() > Duration::from_secs(4));
    if stale && !job.running {
        state.python = engine::find_python();
        state.python_checked = Some(Instant::now());
    }

    // Drag-out bookkeeping
    if let Some(t) = state.drag_started_at {
        if dragout::drag_ended() || t.elapsed() > Duration::from_secs(60) {
            state.drag_started_at = None;
            shared.dragging_out.store(false, Ordering::Relaxed);
        }
    }

    // Files dropped onto the window
    let dropped: Vec<PathBuf> = ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .filter_map(|f| f.path.clone())
            .collect()
    });
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
    let own_drag = shared.dragging_out.load(Ordering::Relaxed);
    if let Some(p) = dropped.into_iter().next() {
        let own = session
            .dir
            .as_ref()
            .is_some_and(|d| p.starts_with(d));
        if !own_drag && !own && !job.running && shared.cap_state() == CapState::Idle {
            start_file(shared, p, stem_count);
        }
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(BG).inner_margin(14.0))
        .show(ctx, |ui| {
            header(ui, shared, &session, stem_count, &job);
            ui.add_space(6.0);
            let job = shared.job();
            let has_stems = shared.stems().is_some();
            if let Some(err) = &job.error {
                error_bar(ui, shared, err);
            }
            if job.running {
                progress(ui, shared, &job);
            } else if shared.cap_state() != CapState::Idle {
                capture_panel(ui, shared);
            } else if !has_stems {
                drop_zone(ui, shared, state, stem_count, hovering && !own_drag);
            }
            if has_stems && !(job.running && job.message.starts_with("Loading")) {
                stem_rows(ui, setter, state, params, shared, &session);
                ui.add_space(4.0);
                footer(ui, setter, state, params, shared, &session);
            }
        });

    if hovering && !own_drag && shared.stems().is_some() {
        let r = ctx.screen_rect().shrink(6.0);
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
        p.rect_filled(r, 10.0, Color32::from_rgba_unmultiplied(20, 30, 50, 200));
        p.rect_stroke(r, 10.0, Stroke::new(2.0f32, ACCENT), StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, "Drop to split", FontId::proportional(22.0), TEXT);
    }
}

fn start_file(shared: &Arc<Shared>, p: PathBuf, stem_count: u8) {
    shared.store_stems(None);
    shared.preview.store(false, Ordering::Relaxed);
    engine::start(shared, Source::File(p), stem_count);
}

fn header(ui: &mut egui::Ui, shared: &Arc<Shared>, session: &SessionInfo, stem_count: u8, job: &crate::shared::JobStatus) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("STEM SPLIT").font(FontId::monospace(16.0)).strong().color(TEXT));
        let has = shared.stems().is_some();
        if has && !session.source_name.is_empty() {
            ui.label(egui::RichText::new(&session.source_name).color(DIM).font(FontId::monospace(12.0)));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = job.running || shared.cap_state() != CapState::Idle;
            if has && !busy {
                let by = shared.bypass.load(Ordering::Relaxed);
                let b = egui::Button::new(egui::RichText::new("Bypass").color(if by { BG } else { TEXT }))
                    .fill(if by { YELLOW } else { PANEL_HI });
                if ui.add(b).clicked() {
                    shared.bypass.store(!by, Ordering::Relaxed);
                    shared.session.write().bypass = !by;
                }
            }
            if has && !busy && ui.button("New").clicked() {
                shared.store_stems(None);
                shared.preview.store(false, Ordering::Relaxed);
                let keep = shared.session.read().stem_count;
                *shared.session.write() = SessionInfo {
                    stem_count: keep,
                    ..Default::default()
                };
                shared.bypass.store(false, Ordering::Relaxed);
            }
            if !has && !busy {
                for n in [6u8, 4u8] {
                    let on = stem_count == n;
                    let b = egui::Button::new(
                        egui::RichText::new(format!("{n} stems")).color(if on { BG } else { TEXT }),
                    )
                    .fill(if on { ACCENT } else { PANEL_HI });
                    if ui.add(b).clicked() {
                        shared.session.write().stem_count = n;
                    }
                }
            }
        });
    });
}

fn error_bar(ui: &mut egui::Ui, shared: &Arc<Shared>, err: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x3a, 0x1d, 0x20))
        .corner_radius(6.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(err).color(Color32::from_rgb(0xff, 0xb4, 0xb4)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("OK").clicked() {
                        shared.set_job(|j| j.error = None);
                    }
                });
            });
        });
    ui.add_space(6.0);
}

fn progress(ui: &mut egui::Ui, shared: &Arc<Shared>, job: &crate::shared::JobStatus) {
    let h = ui.available_height().min(160.0);
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(8.0)
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.set_min_height(h - 32.0);
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                ui.label(egui::RichText::new(if job.message.is_empty() { "Working…" } else { &job.message }).size(15.0));
                ui.add_space(8.0);
                ui.add(
                    egui::ProgressBar::new(job.percent / 100.0)
                        .desired_width(ui.available_width() * 0.8)
                        .fill(ACCENT)
                        .show_percentage(),
                );
                ui.add_space(8.0);
                if !job.message.starts_with("Loading stems") && ui.button("Cancel").clicked() {
                    engine::cancel(shared);
                }
            });
        });
}

fn capture_panel(ui: &mut egui::Ui, shared: &Arc<Shared>) {
    let h = ui.available_height().min(160.0);
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(8.0)
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.set_min_height(h - 32.0);
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(14.0);
                match shared.cap_state() {
                    CapState::Armed => {
                        ui.label(egui::RichText::new("Capture armed: start playback").size(16.0).color(YELLOW));
                        ui.add_space(10.0);
                        if ui.button("Cancel").clicked() {
                            shared.cancel_capture();
                        }
                    }
                    CapState::Recording => {
                        let sr = shared.host_rate().max(1.0) as f64;
                        let len = shared.cap_len.load(Ordering::Relaxed) as f64 / sr;
                        ui.label(
                            egui::RichText::new(format!("Recording  {}", fmt_time(len)))
                                .size(18.0)
                                .color(RED),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            let w = 200.0;
                            ui.add_space((ui.available_width() - w) / 2.0);
                            if ui.button("Stop and split").clicked() {
                                shared.cap_stop.store(true, Ordering::Relaxed);
                                // the audio thread may be idle if the host stopped processing
                                if !shared.host_playing.load(Ordering::Relaxed) {
                                    shared.set_cap_state(CapState::Done);
                                }
                            }
                            if ui.button("Cancel").clicked() {
                                shared.cancel_capture();
                            }
                        });
                    }
                    _ => {
                        ui.label("Saving…");
                    }
                }
            });
        });
}

fn drop_zone(ui: &mut egui::Ui, shared: &Arc<Shared>, state: &mut UiState, stem_count: u8, hovering: bool) {
    let avail = ui.available_size();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail.x, (avail.y - 4.0).max(200.0)), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, if hovering { Color32::from_rgb(0x1f, 0x2a, 0x3d) } else { PANEL });
    let dash = if hovering { ACCENT } else { Color32::from_rgb(0x3a, 0x3e, 0x46) };
    p.rect_stroke(rect.shrink(6.0), 8.0, Stroke::new(1.5f32, dash), StrokeKind::Inside);
    p.text(
        rect.center() - Vec2::new(0.0, 52.0),
        Align2::CENTER_CENTER,
        if hovering { "Drop to split" } else { "Drop audio here" },
        FontId::proportional(22.0),
        TEXT,
    );

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_center_size(rect.center() + Vec2::new(0.0, 14.0), Vec2::new(rect.width() - 40.0, 90.0)))
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    child.horizontal(|ui| {
        let w = 280.0;
        ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
        if ui.add(egui::Button::new("Open file…").min_size(Vec2::new(130.0, 30.0))).clicked() {
            if let Some(p) = pick_file(true) {
                start_file(shared, p, stem_count);
            }
        }
        let cap = egui::Button::new(egui::RichText::new("   Capture").color(RED)).min_size(Vec2::new(130.0, 30.0));
        let cap = ui.add(cap);
        ui.painter().circle_filled(Pos2::new(cap.rect.left() + 38.0, cap.rect.center().y), 4.5, RED);
        if cap.clicked() {
            shared.arm_capture();
        }
    });
    child.add_space(14.0);
    if state.python.is_none() {
        child.label(egui::RichText::new("Separation engine not found").color(YELLOW));
        child.horizontal(|ui| {
            let w = 290.0;
            ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
            if ui.button("Install engine").clicked() {
                engine::install(shared);
                state.python_checked = None;
            }
            if ui.button("Choose Python…").clicked() {
                if let Some(p) = pick_file(false) {
                    engine::set_python(&p);
                    state.python_checked = None;
                }
            }
        });
    }
}

fn pick_file(audio: bool) -> Option<PathBuf> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let d = rfd::FileDialog::new();
        let d = if audio {
            d.add_filter("Audio", &["wav", "aif", "aiff", "flac", "mp3", "m4a", "ogg", "caf"])
        } else {
            d
        };
        d.pick_file()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // zenity is on most Linux desktops; no dialog crate needed
        let mut c = std::process::Command::new("zenity");
        c.arg("--file-selection");
        if audio {
            c.arg("--file-filter=Audio | *.wav *.aif *.aiff *.flac *.mp3 *.m4a *.ogg");
        }
        let out = c.output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then(|| PathBuf::from(s))
    }
}

/// The file to drag for a slot: the stem itself, or a copy with the current volume baked in.
fn drag_file(params: &StemParams, slot: usize, path: &std::path::Path) -> PathBuf {
    let db = params.stems[slot].gain.value();
    let g = crate::fader_gain(db);
    if db.abs() < 0.05 {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let label = if g <= 0.0 { "muted".to_string() } else { format!("{db:+.1} dB") };
    let out = path.with_file_name(format!("{stem} ({label}).wav"));
    if out.exists() {
        return out;
    }
    match crate::audio_io::read_wav(path) {
        Ok((l, r, sr)) => {
            if crate::audio_io::write_wav(&out, &l, &r, sr, g).is_ok() {
                out
            } else {
                path.to_path_buf()
            }
        }
        Err(_) => path.to_path_buf(),
    }
}

fn begin_drag(state: &mut UiState, shared: &Arc<Shared>, files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    shared.dragging_out.store(true, Ordering::Relaxed);
    state.drag_started_at = Some(Instant::now());
    if !dragout::start_file_drag(&files) {
        shared.dragging_out.store(false, Ordering::Relaxed);
        state.drag_started_at = None;
        dragout::reveal(&files[0]);
    }
}

fn audible(params: &StemParams, slot: usize) -> bool {
    let any_solo = params.stems.iter().any(|s| s.solo.value());
    let ch = &params.stems[slot];
    !(ch.mute.value() || (any_solo && !ch.solo.value()))
}

fn stem_rows(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    state: &mut UiState,
    params: &Arc<StemParams>,
    shared: &Arc<Shared>,
    session: &SessionInfo,
) {
    let guard = shared.stems();
    let Some(set) = guard.as_ref() else { return };
    let slots: Vec<usize> = (0..6).filter(|s| set.slot[*s].is_some()).collect();
    let footer_h = 74.0;
    let avail_h = ui.available_height() - footer_h;
    let row_h = ((avail_h - 6.0 * slots.len() as f32) / slots.len().max(1) as f32).clamp(34.0, 62.0);
    let play = shared.play_pos.load(Ordering::Relaxed);
    let total = set.len.max(1);

    for &slot in &slots {
        let stem = set.slot[slot].as_ref().unwrap();
        let color = STEM_COLORS[slot];
        let on = audible(params, slot);
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
        let p = ui.painter_at(row);
        p.rect_filled(row, 6.0, PANEL);

        // Name chip: drag it out into the DAW
        let chip = Rect::from_min_size(row.min, Vec2::new(118.0, row_h));
        let chip_resp = ui.interact(chip, ui.id().with(("chip", slot)), Sense::click_and_drag());
        let chip_fill = if chip_resp.hovered() || chip_resp.dragged() { PANEL_HI } else { PANEL };
        p.rect_filled(chip, CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, chip_fill);
        p.rect_filled(Rect::from_min_size(chip.min, Vec2::new(4.0, row_h)), CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, color);
        p.text(
            chip.left_center() + Vec2::new(14.0, 0.0),
            Align2::LEFT_CENTER,
            STEM_LABELS[slot],
            FontId::proportional(15.0),
            if on { TEXT } else { DIM },
        );
        grip(&p, Pos2::new(chip.right() - 14.0, chip.center().y), DIM);
        if chip_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if chip_resp.drag_started() {
            let f = drag_file(params, slot, &stem.path);
            begin_drag(state, shared, vec![f]);
        }

        // Controls on the right: M S [fader]
        let fader_w = 150.0;
        let right = row.right() - 8.0;
        let fader = Rect::from_min_max(
            Pos2::new(right - fader_w, row.center().y - 11.0),
            Pos2::new(right, row.center().y + 11.0),
        );
        let s_rect = Rect::from_center_size(Pos2::new(fader.left() - 22.0, row.center().y), Vec2::new(26.0, 22.0));
        let m_rect = Rect::from_center_size(Pos2::new(fader.left() - 54.0, row.center().y), Vec2::new(26.0, 22.0));
        toggle(ui, setter, &params.stems[slot].mute, m_rect, "M", RED, ("m", slot));
        toggle(ui, setter, &params.stems[slot].solo, s_rect, "S", YELLOW, ("s", slot));
        gain_fader(ui, setter, &params.stems[slot].gain, fader, color, ("g", slot));

        // Waveform
        let wave = Rect::from_min_max(
            Pos2::new(chip.right() + 8.0, row.top() + 4.0),
            Pos2::new(m_rect.left() - 10.0, row.bottom() - 4.0),
        );
        draw_wave(ui, &stem.peaks, wave, if on { color } else { color.gamma_multiply(0.3) });
        let wave_resp = ui.interact(wave, ui.id().with(("wave", slot)), Sense::click());
        if wave_resp.clicked() && !shared.host_playing.load(Ordering::Relaxed) {
            if let Some(pos) = wave_resp.interact_pointer_pos() {
                let t = ((pos.x - wave.left()) / wave.width()).clamp(0.0, 1.0);
                shared.preview_pos.store((t * total as f32) as i64, Ordering::Relaxed);
            }
        }
        let head = if play >= 0 {
            Some(play)
        } else if !shared.host_playing.load(Ordering::Relaxed) {
            Some(shared.preview_pos.load(Ordering::Relaxed))
        } else {
            None
        };
        if let Some(h) = head {
            if h >= 0 && (h as usize) <= total {
                let x = wave.left() + wave.width() * (h as f32 / total as f32);
                p.line_segment(
                    [Pos2::new(x, row.top() + 2.0), Pos2::new(x, row.bottom() - 2.0)],
                    Stroke::new(1.0f32, Color32::from_white_alpha(if play >= 0 { 220 } else { 90 })),
                );
            }
        }
        ui.add_space(6.0);
    }
    if !session.merged.is_empty() {
        let names: Vec<String> = session
            .merged
            .iter()
            .map(|(n, into)| format!("{n} into {into}"))
            .collect();
        ui.label(egui::RichText::new(format!("Blended (nearly empty): {}", names.join(", "))).color(DIM).size(11.0));
    }
}

/// Six-dot drag grip centred on `c`.
fn grip(p: &egui::Painter, c: Pos2, color: Color32) {
    for k in 0..3 {
        for j in 0..2 {
            p.circle_filled(
                Pos2::new(c.x - 2.5 + j as f32 * 5.0, c.y - 5.0 + k as f32 * 5.0),
                1.3,
                color,
            );
        }
    }
}

fn draw_wave(ui: &egui::Ui, peaks: &[(f32, f32)], rect: Rect, color: Color32) {
    let p = ui.painter_at(rect);
    let mid = rect.center().y;
    let half = rect.height() * 0.5;
    p.line_segment([Pos2::new(rect.left(), mid), Pos2::new(rect.right(), mid)], Stroke::new(1.0f32, Color32::from_white_alpha(18)));
    if peaks.is_empty() {
        return;
    }
    let cols = rect.width().max(1.0) as usize;
    for c in 0..cols {
        let a = c * peaks.len() / cols;
        let b = ((c + 1) * peaks.len() / cols).max(a + 1).min(peaks.len());
        let mut lo = 0.0f32;
        let mut hi = 0.0f32;
        for &(l, h) in &peaks[a..b] {
            lo = lo.min(l);
            hi = hi.max(h);
        }
        let x = rect.left() + c as f32 + 0.5;
        let y0 = mid - (hi.min(1.0) * half);
        let y1 = mid - (lo.max(-1.0) * half);
        if y1 - y0 >= 0.5 {
            p.line_segment([Pos2::new(x, y0), Pos2::new(x, y1)], Stroke::new(1.0f32, color));
        }
    }
}

fn toggle(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    param: &nih_plug::prelude::BoolParam,
    rect: Rect,
    label: &str,
    on_color: Color32,
    id: impl std::hash::Hash,
) {
    let resp = ui.interact(rect, ui.id().with(id), Sense::click());
    let on = param.value();
    let fill = if on {
        on_color
    } else if resp.hovered() {
        Color32::from_rgb(0x3a, 0x3e, 0x46)
    } else {
        PANEL_HI
    };
    let p = ui.painter();
    p.rect_filled(rect, 4.0, fill);
    p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::monospace(12.0), if on { BG } else { DIM });
    if resp.clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, !on);
        setter.end_set_parameter(param);
    }
}

/// Horizontal volume fader: drag to change, double-click for 0 dB, value in dB on top.
fn gain_fader(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    param: &nih_plug::prelude::FloatParam,
    rect: Rect,
    color: Color32,
    id: impl std::hash::Hash,
) {
    let resp = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    if resp.drag_started() {
        setter.begin_set_parameter(param);
    }
    if resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let fine = ui.input(|i| i.modifiers.shift);
            let norm = if fine {
                (param.unmodulated_normalized_value() + resp.drag_delta().x / rect.width() * 0.15).clamp(0.0, 1.0)
            } else {
                ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0)
            };
            setter.set_parameter_normalized(param, norm);
        }
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(param);
    }
    if resp.double_clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, param.default_plain_value());
        setter.end_set_parameter(param);
    }
    let norm = param.unmodulated_normalized_value();
    let p = ui.painter();
    p.rect_filled(rect, 4.0, Color32::from_rgb(0x12, 0x13, 0x15));
    let fill = Rect::from_min_max(rect.min, Pos2::new(rect.left() + rect.width() * norm, rect.bottom()));
    p.rect_filled(fill, 4.0, color.gamma_multiply(if resp.hovered() || resp.dragged() { 0.75 } else { 0.55 }));
    // 0 dB tick
    let zero = param.preview_normalized(param.default_plain_value());
    let zx = rect.left() + rect.width() * zero;
    p.line_segment([Pos2::new(zx, rect.top()), Pos2::new(zx, rect.bottom())], Stroke::new(1.0f32, Color32::from_white_alpha(60)));
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        param.normalized_value_to_string(norm, true),
        FontId::monospace(11.5),
        TEXT,
    );
}

fn footer(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    state: &mut UiState,
    params: &Arc<StemParams>,
    shared: &Arc<Shared>,
    session: &SessionInfo,
) {
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Drag all (audible) stems at once
                let all = ui.add(
                    egui::Button::new("      Drag all")
                        .sense(Sense::click_and_drag())
                        .min_size(Vec2::new(0.0, 26.0)),
                );
                grip(ui.painter(), Pos2::new(all.rect.left() + 13.0, all.rect.center().y), DIM);
                if all.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                if all.drag_started() {
                    let guard = shared.stems();
                    if let Some(set) = guard.as_ref() {
                        let files: Vec<PathBuf> = (0..6)
                            .filter(|s| audible(params, *s))
                            .filter_map(|s| set.slot[s].as_ref().map(|a| drag_file(params, s, &a.path)))
                            .collect();
                        begin_drag(state, shared, files);
                    }
                }
                if ui.button(if cfg!(target_os = "macos") { "Show in Finder" } else { "Show files" }).clicked() {
                    if let Some(set) = shared.stems().as_ref() {
                        if let Some(a) = set.slot.iter().flatten().next() {
                            dragout::reveal(&a.path);
                        }
                    }
                }
                ui.separator();

                let host_playing = shared.host_playing.load(Ordering::Relaxed);
                let previewing = shared.preview.load(Ordering::Relaxed);
                let label = if previewing { "■ Stop" } else { "▶ Preview" };
                if ui.add_enabled(!host_playing, egui::Button::new(label)).clicked() {
                    shared.preview.store(!previewing, Ordering::Relaxed);
                }
                ui.separator();
                ui.label(egui::RichText::new("Starts at").color(DIM));
                ui.label(egui::RichText::new(fmt_time(session.origin_sec)).font(FontId::monospace(12.5)));
                if ui.button("Set to playhead").clicked() {
                    let sr = shared.host_rate().max(1.0) as f64;
                    let pos = shared.host_pos.load(Ordering::Relaxed).max(0) as f64 / sr;
                    shared.set_origin_sec(pos);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(120.0, 22.0), Sense::hover());
                    gain_fader(ui, setter, &params.output, r, ACCENT, "out");
                    ui.label(egui::RichText::new("Out").color(DIM));
                });
            });
        });
    let _ = STEM_SLOTS;
}
