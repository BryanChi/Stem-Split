//! The window, in FX Devices' Monolith look: black page, hairlines, square corners,
//! letter-spaced JetBrains Mono caps, one green accent (same palette as the REAPER script).

use crate::dragout;
use crate::engine::{self, Source};
use crate::shared::{CapState, SessionInfo, Shared};
use crate::{app_mode, StemParams, STEM_LABELS};
use nih_plug::prelude::{Param, ParamSetter};
use nih_plug_egui::egui::{
    self, Align2, Color32, FontFamily, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const WIDTH: u32 = 820;
pub const HEIGHT: u32 = 480;
const PAD: f32 = 16.0;

// Monolith palette
const PAGE: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
const BODY: Color32 = Color32::from_rgb(0x0c, 0x0c, 0x0d);
const CHIP: Color32 = Color32::from_rgb(0x11, 0x11, 0x12);
const ACC: Color32 = Color32::from_rgb(0x49, 0xcc, 0x85);
const RED: Color32 = Color32::from_rgb(0xe0, 0x60, 0x4f);
const INK: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
const INK2: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const INK3: Color32 = Color32::from_rgb(0x5f, 0x5f, 0x5f);
const LINE: Color32 = Color32::from_rgba_premultiplied(0x1f, 0x1f, 0x1f, 0x1f);
const LINE2: Color32 = Color32::from_rgba_premultiplied(0x33, 0x33, 0x33, 0x33);
const SOFT: Color32 = Color32::from_rgba_premultiplied(0x0f, 0x0f, 0x0f, 0x0f);
const TICK: Color32 = Color32::from_rgba_premultiplied(0x59, 0x59, 0x59, 0x59);

/// Stem colours, same as the script's STEM_RGB (vocals, drums, bass, guitar, piano, other).
const STEM_COLORS: [Color32; 6] = [
    Color32::from_rgb(220, 70, 90),
    Color32::from_rgb(230, 180, 50),
    Color32::from_rgb(70, 120, 220),
    Color32::from_rgb(230, 120, 50),
    Color32::from_rgb(160, 90, 200),
    Color32::from_rgb(140, 140, 150),
];

fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

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
        |ctx, _| setup_style(ctx),
        move |ctx, setter, state| draw(ctx, setter, state, &params, &shared),
    )
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "jbm".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/JetBrainsMono-Medium.ttf"
        ))),
    );
    fonts.font_data.insert(
        "jbm-bold".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/JetBrainsMono-SemiBold.ttf"
        ))),
    );
    for fam in [FontFamily::Monospace, FontFamily::Proportional] {
        fonts.families.entry(fam).or_default().insert(0, "jbm".into());
    }
    let mut b = vec!["jbm-bold".to_string()];
    b.extend(fonts.families[&FontFamily::Monospace].iter().skip(1).cloned());
    fonts.families.insert(FontFamily::Name("bold".into()), b);
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = PAGE;
    v.window_fill = PAGE;
    v.override_text_color = Some(INK);
    v.selection.bg_fill = alpha(ACC, 0.35);
    ctx.set_visuals(v);
}

// ---------------------------------------------------------------------------------------------
// drawing helpers (ports of the script's caps / button / corner_ticks)

/// Letter-spaced caps; returns the x after the text.
fn caps(p: &Painter, pos: Pos2, text: &str, size: f32, is_bold: bool, sp: f32, color: Color32) -> f32 {
    let font = if is_bold { bold(size) } else { mono(size) };
    let mut x = pos.x;
    let mut buf = [0u8; 4];
    for ch in text.to_uppercase().chars() {
        let g = p.layout_no_wrap(ch.encode_utf8(&mut buf).to_string(), font.clone(), color);
        let w = g.size().x;
        p.galley(Pos2::new(x, pos.y), g, color);
        x += w + sp;
    }
    x - sp
}

fn caps_w(p: &Painter, text: &str, size: f32, is_bold: bool, sp: f32) -> f32 {
    let font = if is_bold { bold(size) } else { mono(size) };
    let n = text.chars().count() as f32;
    // JetBrains Mono is monospaced: every glyph has the same advance
    let adv = p.layout_no_wrap("M".into(), font, INK).size().x;
    (adv * n + sp * (n - 1.0)).max(0.0)
}

fn corner_ticks(p: &Painter, r: Rect, len: f32, c: Color32) {
    let s = Stroke::new(1.0f32, c);
    let (l, t, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
    for (a, bb) in [
        ((l, t), (l + len, t)),
        ((l, t), (l, t + len)),
        ((rr - len, t), (rr, t)),
        ((rr, t), (rr, t + len)),
        ((l, b), (l + len, b)),
        ((l, b - len), (l, b)),
        ((rr - len, b), (rr, b)),
        ((rr, b - len), (rr, b)),
    ] {
        p.line_segment([Pos2::new(a.0, a.1), Pos2::new(bb.0, bb.1)], s);
    }
}

fn dashed_rect(p: &Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.0f32, c);
    let dash = 4.0;
    let gap = 4.0;
    let mut x = r.left();
    while x < r.right() {
        let e = (x + dash).min(r.right());
        p.line_segment([Pos2::new(x, r.top()), Pos2::new(e, r.top())], s);
        p.line_segment([Pos2::new(x, r.bottom()), Pos2::new(e, r.bottom())], s);
        x += dash + gap;
    }
    let mut y = r.top();
    while y < r.bottom() {
        let e = (y + dash).min(r.bottom());
        p.line_segment([Pos2::new(r.left(), y), Pos2::new(r.left(), e)], s);
        p.line_segment([Pos2::new(r.right(), y), Pos2::new(r.right(), e)], s);
        y += dash + gap;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Primary,
    Plain,
    Danger,
}

/// A square Monolith button at `r`; returns the response (check `.clicked()`).
fn button(ui: &mut egui::Ui, r: Rect, id: &str, label: &str, kind: Kind, disabled: bool) -> egui::Response {
    let resp = ui.interact(r, ui.id().with(id), if disabled { Sense::hover() } else { Sense::click_and_drag() });
    let hov = resp.hovered() && !disabled;
    let act = resp.is_pointer_button_down_on() && !disabled;
    let base = match kind {
        Kind::Primary => ACC,
        Kind::Danger => RED,
        Kind::Plain => INK,
    };
    let (fill, edge, clr) = if disabled {
        (Color32::TRANSPARENT, LINE, INK3)
    } else if act {
        (base, base, PAGE)
    } else if kind == Kind::Plain {
        (if hov { SOFT } else { Color32::TRANSPARENT }, if hov { INK2 } else { LINE2 }, if hov { INK } else { INK2 })
    } else {
        (alpha(base, if hov { 0.24 } else { 0.12 }), alpha(base, if hov { 1.0 } else { 0.8 }), INK)
    };
    let p = ui.painter();
    if fill != Color32::TRANSPARENT {
        p.rect_filled(r, 0.0, fill);
    }
    p.rect_stroke(r, 0.0, Stroke::new(1.0f32, edge), StrokeKind::Inside);
    let big = r.height() >= 34.0;
    let (sz, sp) = if big { (11.0, 2.4) } else { (10.0, 1.4) };
    let tw = caps_w(p, label, sz, big, sp);
    caps(
        p,
        Pos2::new((r.center().x - tw / 2.0).round(), (r.center().y - sz * 0.62).round()),
        label,
        sz,
        big,
        sp,
        clr,
    );
    if kind == Kind::Primary && !disabled {
        corner_ticks(p, r.expand(3.0), 5.0, alpha(ACC, if hov { 0.9 } else { 0.45 }));
    }
    resp
}

/// Section label + hairline across the width; returns the y below it.
fn section(p: &Painter, x: f32, y: f32, w: f32, label: &str, right: &str) -> f32 {
    let x2 = caps(p, Pos2::new(x, y), label, 9.0, true, 1.6, INK3);
    let mut rx = x + w;
    if !right.is_empty() {
        let rw = caps_w(p, right, 9.0, false, 1.2);
        caps(p, Pos2::new(rx - rw, y), right, 9.0, false, 1.2, INK2);
        rx -= rw + 10.0;
    }
    if rx > x2 + 10.0 {
        p.line_segment([Pos2::new(x2 + 10.0, y + 6.0), Pos2::new(rx, y + 6.0)], Stroke::new(1.0f32, LINE));
    }
    y + 17.0
}

fn grip(p: &Painter, c: Pos2, color: Color32) {
    for k in 0..3 {
        for j in 0..2 {
            p.rect_filled(
                Rect::from_center_size(Pos2::new(c.x - 2.0 + j as f32 * 4.0, c.y - 4.0 + k as f32 * 4.0), Vec2::splat(1.6)),
                0.0,
                color,
            );
        }
    }
}

fn fmt_time(sec: f64) -> String {
    let neg = sec < 0.0;
    let s = sec.abs();
    let m = (s / 60.0).floor() as i64;
    let r = s - m as f64 * 60.0;
    format!("{}{}:{:05.2}", if neg { "-" } else { "" }, m, r)
}

// ---------------------------------------------------------------------------------------------

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
    let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
    let own_drag = shared.dragging_out.load(Ordering::Relaxed);
    if let Some(p) = dropped.into_iter().next() {
        let own = session.dir.as_ref().is_some_and(|d| p.starts_with(d));
        if !own_drag && !own && !job.running && shared.cap_state() == CapState::Idle {
            start_file(shared, p, stem_count);
        }
    }

    // Space plays / pauses in the app
    let has_stems = shared.stems().is_some();
    if app_mode() && has_stems && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        let on = shared.preview.load(Ordering::Relaxed);
        shared.preview.store(!on, Ordering::Relaxed);
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(PAGE).inner_margin(0.0))
        .show(ctx, |ui| {
            let full = ui.max_rect();
            let area = full.shrink(PAD);
            let mut y = header(ui, area, shared, &session, stem_count, &job);
            let job = shared.job();
            if let Some(err) = &job.error {
                y = error_bar(ui, area, y, shared, err);
            }
            let rest = Rect::from_min_max(Pos2::new(area.left(), y), area.max);
            if job.running {
                progress(ui, rest, shared, &job);
            } else if shared.cap_state() != CapState::Idle {
                capture_panel(ui, rest, shared);
            } else if !has_stems {
                drop_zone(ui, rest, shared, state, stem_count, hovering && !own_drag);
            } else {
                let footer_h = 30.0;
                let rows = Rect::from_min_max(rest.min, Pos2::new(rest.right(), rest.bottom() - footer_h - 12.0));
                stem_rows(ui, rows, setter, state, params, shared, &session);
                let f = Rect::from_min_max(Pos2::new(rest.left(), rest.bottom() - footer_h), rest.max);
                footer(ui, f, setter, state, params, shared, &session);
            }
        });

    if hovering && !own_drag && has_stems {
        let r = ctx.screen_rect().shrink(10.0);
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
        p.rect_filled(r, 0.0, alpha(PAGE, 0.85));
        dashed_rect(&p, r, alpha(ACC, 0.7));
        corner_ticks(&p, r.expand(4.0), 8.0, ACC);
        let tw = caps_w(&p, "Drop to split", 14.0, true, 3.2);
        caps(&p, Pos2::new(r.center().x - tw / 2.0, r.center().y - 9.0), "Drop to split", 14.0, true, 3.2, INK);
    }
}

fn start_file(shared: &Arc<Shared>, p: PathBuf, stem_count: u8) {
    shared.store_stems(None);
    shared.preview.store(false, Ordering::Relaxed);
    shared.preview_pos.store(0, Ordering::Relaxed);
    engine::start(shared, Source::File(p), stem_count);
}

fn header(
    ui: &mut egui::Ui,
    area: Rect,
    shared: &Arc<Shared>,
    session: &SessionInfo,
    stem_count: u8,
    job: &crate::shared::JobStatus,
) -> f32 {
    let p = ui.painter().clone();
    let (x, y) = (area.left(), area.top());
    let x2 = caps(&p, Pos2::new(x, y + 2.0), "Stem Split", 14.0, true, 3.2, INK);
    p.rect_filled(Rect::from_min_size(Pos2::new(x2 + 8.0, y + 7.0), Vec2::splat(5.0)), 0.0, ACC);
    let has = shared.stems().is_some();
    let sub = if has && !session.source_name.is_empty() { session.source_name.as_str() } else { "BS-RoFormer SW" };
    let sub: String = sub.chars().take(40).collect();
    caps(&p, Pos2::new(x2 + 20.0, y + 6.0), &sub, 9.0, false, 1.3, INK3);

    let busy = job.running || shared.cap_state() != CapState::Idle;
    let mut rx = area.right();
    let bh = 22.0;
    if has && !busy {
        let r = Rect::from_min_max(Pos2::new(rx - 54.0, y), Pos2::new(rx, y + bh));
        if button(ui, r, "new", "New", Kind::Plain, false).clicked() {
            shared.store_stems(None);
            shared.preview.store(false, Ordering::Relaxed);
            let keep = shared.session.read().stem_count;
            *shared.session.write() = SessionInfo { stem_count: keep, ..Default::default() };
            shared.bypass.store(false, Ordering::Relaxed);
        }
        rx -= 62.0;
        if !app_mode() {
            let by = shared.bypass.load(Ordering::Relaxed);
            let r = Rect::from_min_max(Pos2::new(rx - 74.0, y), Pos2::new(rx, y + bh));
            if button(ui, r, "bypass", "Bypass", if by { Kind::Danger } else { Kind::Plain }, false).clicked() {
                shared.bypass.store(!by, Ordering::Relaxed);
                shared.session.write().bypass = !by;
            }
        }
    } else if !busy {
        for n in [6u8, 4u8] {
            let r = Rect::from_min_max(Pos2::new(rx - 78.0, y), Pos2::new(rx, y + bh));
            let on = stem_count == n;
            if button(ui, r, &format!("n{n}"), &format!("{n} stems"), if on { Kind::Primary } else { Kind::Plain }, false)
                .clicked()
            {
                shared.session.write().stem_count = n;
            }
            rx -= 86.0;
        }
    }
    y + 40.0
}

fn error_bar(ui: &mut egui::Ui, area: Rect, y: f32, shared: &Arc<Shared>, err: &str) -> f32 {
    let p = ui.painter().clone();
    let font = mono(11.0);
    let wrap = area.width() - 90.0;
    let g = p.layout(err.to_string(), font, INK, wrap);
    let h = (g.size().y + 16.0).max(34.0);
    let r = Rect::from_min_size(Pos2::new(area.left(), y), Vec2::new(area.width(), h));
    p.rect_filled(r, 0.0, alpha(RED, 0.08));
    p.rect_stroke(r, 0.0, Stroke::new(1.0f32, alpha(RED, 0.6)), StrokeKind::Inside);
    p.rect_filled(Rect::from_min_size(r.min, Vec2::new(3.0, h)), 0.0, RED);
    p.galley(Pos2::new(r.left() + 14.0, r.top() + 8.0), g, INK);
    let b = Rect::from_min_size(Pos2::new(r.right() - 58.0, r.center().y - 11.0), Vec2::new(48.0, 22.0));
    if button(ui, b, "errok", "OK", Kind::Plain, false).clicked() {
        shared.set_job(|j| j.error = None);
    }
    y + h + 12.0
}

fn panel(p: &Painter, r: Rect) {
    p.rect_filled(r, 0.0, BODY);
    p.rect_stroke(r, 0.0, Stroke::new(1.0f32, LINE), StrokeKind::Inside);
}

fn progress(ui: &mut egui::Ui, rest: Rect, shared: &Arc<Shared>, job: &crate::shared::JobStatus) {
    let p = ui.painter().clone();
    let y = section(&p, rest.left(), rest.top(), rest.width(), "Splitting", &format!("{:.0}%", job.percent));
    let r = Rect::from_min_size(Pos2::new(rest.left(), y), Vec2::new(rest.width(), 120.0));
    panel(&p, r);
    let msg = if job.message.is_empty() { "Working…" } else { job.message.as_str() };
    p.text(Pos2::new(r.left() + 18.0, r.top() + 22.0), Align2::LEFT_TOP, msg, mono(12.0), INK);
    let bar = Rect::from_min_size(Pos2::new(r.left() + 18.0, r.top() + 54.0), Vec2::new(r.width() - 36.0, 2.0));
    p.rect_filled(bar, 0.0, LINE);
    let fw = bar.width() * (job.percent / 100.0).clamp(0.0, 1.0);
    p.rect_filled(Rect::from_min_size(bar.min, Vec2::new(fw, 2.0)), 0.0, ACC);
    if !job.message.starts_with("Loading stems") {
        let b = Rect::from_min_size(Pos2::new(r.left() + 18.0, r.top() + 76.0), Vec2::new(90.0, 24.0));
        if button(ui, b, "cancel", "Cancel", Kind::Plain, false).clicked() {
            engine::cancel(shared);
        }
    }
}

fn capture_panel(ui: &mut egui::Ui, rest: Rect, shared: &Arc<Shared>) {
    let p = ui.painter().clone();
    let st = shared.cap_state();
    let right = match st {
        CapState::Recording => {
            let sr = shared.host_rate().max(1.0) as f64;
            fmt_time(shared.cap_len.load(Ordering::Relaxed) as f64 / sr)
        }
        _ => String::new(),
    };
    let y = section(&p, rest.left(), rest.top(), rest.width(), "Capture", &right);
    let r = Rect::from_min_size(Pos2::new(rest.left(), y), Vec2::new(rest.width(), 120.0));
    panel(&p, r);
    let (label, clr) = match st {
        CapState::Armed => ("Armed: start playback", INK2),
        CapState::Recording => ("Recording", RED),
        _ => ("Saving", INK2),
    };
    if st == CapState::Recording {
        let blink = (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0;
        p.rect_filled(Rect::from_min_size(Pos2::new(r.left() + 18.0, r.top() + 25.0), Vec2::splat(7.0)), 0.0, alpha(RED, if blink { 1.0 } else { 0.35 }));
    }
    caps(&p, Pos2::new(r.left() + 34.0, r.top() + 22.0), label, 11.0, true, 2.4, clr);
    let b1 = Rect::from_min_size(Pos2::new(r.left() + 18.0, r.top() + 70.0), Vec2::new(150.0, 28.0));
    let b2 = Rect::from_min_size(Pos2::new(r.left() + 178.0, r.top() + 70.0), Vec2::new(90.0, 28.0));
    match st {
        CapState::Recording => {
            if button(ui, b1, "stopsplit", "Stop + split", Kind::Primary, false).clicked() {
                shared.cap_stop.store(true, Ordering::Relaxed);
                if !shared.host_playing.load(Ordering::Relaxed) {
                    shared.set_cap_state(CapState::Done);
                }
            }
            if button(ui, b2, "capcancel", "Cancel", Kind::Plain, false).clicked() {
                shared.cancel_capture();
            }
        }
        CapState::Armed => {
            if button(ui, b1, "capcancel", "Cancel", Kind::Plain, false).clicked() {
                shared.cancel_capture();
            }
        }
        _ => {}
    }
}

fn drop_zone(ui: &mut egui::Ui, rest: Rect, shared: &Arc<Shared>, state: &mut UiState, stem_count: u8, hovering: bool) {
    let p = ui.painter().clone();
    let y = section(&p, rest.left(), rest.top(), rest.width(), "Input", if app_mode() { "file" } else { "file or capture" });
    let r = Rect::from_min_max(Pos2::new(rest.left(), y), rest.max);
    p.rect_filled(r, 0.0, if hovering { alpha(ACC, 0.06) } else { BODY });
    let inner = r.shrink(10.0);
    dashed_rect(&p, inner, if hovering { alpha(ACC, 0.8) } else { LINE2 });
    corner_ticks(&p, r, 8.0, if hovering { ACC } else { TICK });

    let c = r.center();
    let title = if hovering { "Drop to split" } else { "Drop audio" };
    let tw = caps_w(&p, title, 14.0, true, 3.2);
    caps(&p, Pos2::new(c.x - tw / 2.0, c.y - 62.0), title, 14.0, true, 3.2, if hovering { ACC } else { INK });
    let sub = "wav  aiff  flac  mp3  m4a";
    let sw = caps_w(&p, sub, 9.0, false, 1.3);
    caps(&p, Pos2::new(c.x - sw / 2.0, c.y - 36.0), sub, 9.0, false, 1.3, INK3);

    let bw = 150.0;
    let bh = 36.0;
    let two = !app_mode();
    let total = if two { bw * 2.0 + 12.0 } else { bw };
    let b1 = Rect::from_min_size(Pos2::new(c.x - total / 2.0, c.y - 6.0), Vec2::new(bw, bh));
    if button(ui, b1, "open", "Open file", Kind::Primary, false).clicked() {
        if let Some(f) = pick_file(true) {
            start_file(shared, f, stem_count);
        }
    }
    if two {
        let b2 = Rect::from_min_size(Pos2::new(b1.right() + 12.0, b1.top()), Vec2::new(bw, bh));
        if button(ui, b2, "capture", "Capture", Kind::Danger, false).clicked() {
            shared.arm_capture();
        }
    }

    if state.python.is_none() {
        let ty = c.y + 58.0;
        let t = "Separation engine not found";
        let w = caps_w(&p, t, 9.0, true, 1.6);
        caps(&p, Pos2::new(c.x - w / 2.0, ty), t, 9.0, true, 1.6, RED);
        let i1 = Rect::from_min_size(Pos2::new(c.x - 156.0, ty + 20.0), Vec2::new(150.0, 24.0));
        let i2 = Rect::from_min_size(Pos2::new(c.x + 6.0, ty + 20.0), Vec2::new(150.0, 24.0));
        if button(ui, i1, "install", "Install engine", Kind::Plain, false).clicked() {
            engine::install(shared);
            state.python_checked = None;
        }
        if button(ui, i2, "choosepy", "Choose Python", Kind::Plain, false).clicked() {
            if let Some(f) = pick_file(false) {
                engine::set_python(&f);
                state.python_checked = None;
            }
        }
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
    area: Rect,
    setter: &ParamSetter,
    state: &mut UiState,
    params: &Arc<StemParams>,
    shared: &Arc<Shared>,
    session: &SessionInfo,
) {
    let Some(set) = shared.stems() else { return };
    let p = ui.painter().clone();
    let slots: Vec<usize> = (0..6).filter(|s| set.slot[*s].is_some()).collect();
    let sr = set.sample_rate.max(1.0) as f64;
    let total = set.len.max(1);
    let merged = if session.merged.is_empty() {
        String::new()
    } else {
        format!("blended: {}", session.merged.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(" "))
    };
    let y0 = section(&p, area.left(), area.top(), area.width(), &format!("Stems  {}", slots.len()), &merged);
    let gap = 4.0;
    let avail = area.bottom() - y0;
    let row_h = ((avail - gap * (slots.len() as f32 - 1.0)) / slots.len().max(1) as f32).clamp(30.0, 58.0);

    let play = shared.play_pos.load(Ordering::Relaxed);
    let host_playing = shared.host_playing.load(Ordering::Relaxed);
    let head = if play >= 0 {
        Some(play)
    } else if !host_playing {
        Some(shared.preview_pos.load(Ordering::Relaxed))
    } else {
        None
    };

    for (k, &slot) in slots.iter().enumerate() {
        let stem = set.slot[slot].as_ref().unwrap();
        let color = STEM_COLORS[slot];
        let on = audible(params, slot);
        let row = Rect::from_min_size(Pos2::new(area.left(), y0 + k as f32 * (row_h + gap)), Vec2::new(area.width(), row_h));

        // Name chip: drag it into the DAW / Finder
        let chip = Rect::from_min_size(row.min, Vec2::new(130.0, row_h));
        let chip_resp = ui.interact(chip, ui.id().with(("chip", slot)), Sense::click_and_drag());
        let hov = chip_resp.hovered() || chip_resp.dragged();
        p.rect_filled(row, 0.0, BODY);
        p.rect_filled(chip, 0.0, if hov { Color32::from_rgb(0x19, 0x19, 0x1a) } else { CHIP });
        p.rect_stroke(row, 0.0, Stroke::new(1.0f32, if hov { LINE2 } else { LINE }), StrokeKind::Inside);
        p.line_segment([Pos2::new(chip.right(), row.top()), Pos2::new(chip.right(), row.bottom())], Stroke::new(1.0f32, LINE));
        p.rect_filled(Rect::from_min_size(row.min, Vec2::new(3.0, row_h)), 0.0, alpha(color, if on { 1.0 } else { 0.22 }));
        caps(&p, Pos2::new(chip.left() + 14.0, chip.center().y - 6.5), STEM_LABELS[slot], 10.0, true, 1.6, if on { INK } else { INK3 });
        grip(&p, Pos2::new(chip.right() - 14.0, chip.center().y), if hov { INK2 } else { INK3 });
        if hov {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if chip_resp.drag_started() {
            let f = drag_file(params, slot, &stem.path);
            begin_drag(state, shared, vec![f]);
        }

        // M  S  fader
        let fader_w = 160.0;
        let right = row.right() - 10.0;
        let fader = Rect::from_min_max(Pos2::new(right - fader_w, row.center().y - 10.0), Pos2::new(right, row.center().y + 10.0));
        let s_rect = Rect::from_center_size(Pos2::new(fader.left() - 20.0, row.center().y), Vec2::new(22.0, 20.0));
        let m_rect = Rect::from_center_size(Pos2::new(fader.left() - 46.0, row.center().y), Vec2::new(22.0, 20.0));
        toggle(ui, setter, &params.stems[slot].mute, m_rect, "M", RED, ("m", slot));
        toggle(ui, setter, &params.stems[slot].solo, s_rect, "S", ACC, ("s", slot));
        fader_widget(ui, setter, &params.stems[slot].gain, fader, color, ("g", slot));

        // Waveform; click to move the play position
        let wave = Rect::from_min_max(Pos2::new(chip.right() + 10.0, row.top() + 5.0), Pos2::new(m_rect.left() - 14.0, row.bottom() - 5.0));
        draw_wave(&p, &stem.peaks, wave, alpha(color, if on { 0.85 } else { 0.22 }));
        let wave_resp = ui.interact(wave, ui.id().with(("wave", slot)), Sense::click_and_drag());
        if (wave_resp.clicked() || wave_resp.dragged()) && !host_playing {
            if let Some(pos) = wave_resp.interact_pointer_pos() {
                let t = ((pos.x - wave.left()) / wave.width()).clamp(0.0, 1.0);
                shared.preview_pos.store((t * total as f32) as i64, Ordering::Relaxed);
            }
        }
        if let Some(h) = head {
            if h >= 0 && (h as usize) <= total {
                let x = wave.left() + wave.width() * (h as f32 / total as f32);
                p.line_segment(
                    [Pos2::new(x, row.top() + 1.0), Pos2::new(x, row.bottom() - 1.0)],
                    Stroke::new(1.0f32, if play >= 0 { INK } else { INK3 }),
                );
            }
        }
    }
    let _ = sr;
}

fn draw_wave(p: &Painter, peaks: &[(f32, f32)], rect: Rect, color: Color32) {
    let mid = rect.center().y;
    let half = rect.height() * 0.5;
    p.line_segment([Pos2::new(rect.left(), mid), Pos2::new(rect.right(), mid)], Stroke::new(1.0f32, LINE));
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
        let y0 = mid - hi.min(1.0) * half;
        let y1 = mid - lo.max(-1.0) * half;
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
    let p = ui.painter();
    if on {
        p.rect_filled(rect, 0.0, on_color);
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, SOFT);
    }
    p.rect_stroke(rect, 0.0, Stroke::new(1.0f32, if on { on_color } else if resp.hovered() { INK2 } else { LINE2 }), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, label, bold(10.0), if on { PAGE } else { INK2 });
    if resp.clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, !on);
        setter.end_set_parameter(param);
    }
}

/// Volume fader: drag to set (Shift = fine), double-click for 0 dB.
fn fader_widget(
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
    let hov = resp.hovered() || resp.dragged();
    let norm = param.unmodulated_normalized_value();
    let p = ui.painter();
    p.rect_filled(rect, 0.0, PAGE);
    let fx = rect.left() + rect.width() * norm;
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(fx, rect.bottom())), 0.0, alpha(color, if hov { 0.42 } else { 0.30 }));
    p.line_segment([Pos2::new(fx, rect.top()), Pos2::new(fx, rect.bottom())], Stroke::new(1.0f32, color));
    let zx = rect.left() + rect.width() * param.preview_normalized(param.default_plain_value());
    p.line_segment([Pos2::new(zx, rect.top()), Pos2::new(zx, rect.top() + 4.0)], Stroke::new(1.0f32, TICK));
    p.line_segment([Pos2::new(zx, rect.bottom() - 4.0), Pos2::new(zx, rect.bottom())], Stroke::new(1.0f32, TICK));
    p.rect_stroke(rect, 0.0, Stroke::new(1.0f32, if hov { LINE2 } else { LINE }), StrokeKind::Inside);
    let txt = format!("{} dB", param.normalized_value_to_string(norm, false));
    p.text(Pos2::new(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, txt, mono(10.0), if hov { INK } else { INK2 });
}

fn footer(
    ui: &mut egui::Ui,
    f: Rect,
    setter: &ParamSetter,
    state: &mut UiState,
    params: &Arc<StemParams>,
    shared: &Arc<Shared>,
    session: &SessionInfo,
) {
    let p = ui.painter().clone();
    p.line_segment([Pos2::new(f.left(), f.top() - 6.0), Pos2::new(f.right(), f.top() - 6.0)], Stroke::new(1.0f32, LINE));
    let h = 26.0;
    let y = f.top() + 2.0;
    let mut x = f.left();
    let mut place = |w: f32| {
        let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
        x += w + 8.0;
        r
    };

    // Drag all (audible) stems at once
    let r = place(110.0);
    let all = button(ui, r, "dragall", "    Drag all", Kind::Primary, false);
    grip(&p, Pos2::new(r.left() + 16.0, r.center().y), INK);
    if all.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if all.drag_started() {
        if let Some(set) = shared.stems() {
            let files: Vec<PathBuf> = (0..6)
                .filter(|s| audible(params, *s))
                .filter_map(|s| set.slot[s].as_ref().map(|a| drag_file(params, s, &a.path)))
                .collect();
            begin_drag(state, shared, files);
        }
    }
    let r = place(if cfg!(target_os = "macos") { 132.0 } else { 104.0 });
    if button(ui, r, "reveal", if cfg!(target_os = "macos") { "Show in Finder" } else { "Show files" }, Kind::Plain, false).clicked() {
        if let Some(set) = shared.stems() {
            if let Some(a) = set.slot.iter().flatten().next() {
                dragout::reveal(&a.path);
            }
        }
    }

    let host_playing = shared.host_playing.load(Ordering::Relaxed);
    let previewing = shared.preview.load(Ordering::Relaxed);
    let r = place(84.0);
    let label = if previewing { "Pause" } else if app_mode() { "Play" } else { "Preview" };
    if button(ui, r, "play", label, if previewing { Kind::Primary } else { Kind::Plain }, host_playing).clicked() {
        shared.preview.store(!previewing, Ordering::Relaxed);
    }

    if app_mode() {
        if let Some(set) = shared.stems() {
            let sr = set.sample_rate.max(1.0) as f64;
            let pos = shared.preview_pos.load(Ordering::Relaxed).max(0) as f64 / sr;
            let t = format!("{} / {}", fmt_time(pos), fmt_time(set.len as f64 / sr));
            p.text(Pos2::new(x + 4.0, y + h / 2.0), Align2::LEFT_CENTER, t, mono(11.0), INK2);
        }
    } else {
        let x0 = caps(&p, Pos2::new(x + 4.0, y + 8.0), "Starts", 9.0, true, 1.6, INK3);
        p.text(Pos2::new(x0 + 8.0, y + h / 2.0), Align2::LEFT_CENTER, fmt_time(session.origin_sec), mono(11.0), INK);
        x = x0 + 72.0;
        let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(110.0, h));
        if button(ui, r, "toplay", "To playhead", Kind::Plain, false).clicked() {
            let sr = shared.host_rate().max(1.0) as f64;
            let pos = shared.host_pos.load(Ordering::Relaxed).max(0) as f64 / sr;
            shared.set_origin_sec(pos);
        }
    }

    let out = Rect::from_min_max(Pos2::new(f.right() - 140.0, y + 3.0), Pos2::new(f.right(), y + h - 3.0));
    let lx = out.left() - caps_w(&p, "Out", 9.0, true, 1.6) - 10.0;
    caps(&p, Pos2::new(lx, y + 8.0), "Out", 9.0, true, 1.6, INK3);
    fader_widget(ui, setter, &params.output, out, ACC, "out");
}
