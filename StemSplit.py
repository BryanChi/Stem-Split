#!/usr/bin/env python3
"""REAPER sidecar: split a mix into stems with BS-RoFormer SW (MLX), optionally drums into kit pieces."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys
import tempfile
import traceback
from pathlib import Path


ROFORMER_CKPT = "BS-Roformer-SW.ckpt"
ENGINE = "bs-roformer-sw"
SAMPLERATE = 44100

STEM_ORDER_4 = ("vocals", "drums", "bass", "other")
STEM_ORDER_6 = ("vocals", "drums", "bass", "guitar", "piano", "other")
DRUM_ORDER = ("kick", "snare", "toms", "hh", "ride", "crash")


def write_progress(path: str | None, pct: int, message: str) -> None:
    if not path:
        return
    try:
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as fh:
            fh.write(f"{max(0, min(100, int(pct)))}\n{message}\n")
        os.replace(tmp, path)
    except OSError:
        pass


def write_done(path: str | None, code: int) -> None:
    if not path:
        return
    try:
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as fh:
            fh.write(str(int(code)))
        os.replace(tmp, path)
    except OSError:
        pass


def resolve_stems(value) -> int:
    """'4' / '6' (also accepts the old model names: 6stem / htdemucs_6s -> 6, anything else -> 4)."""
    key = str(value or "4").strip().lower()
    return 6 if key in ("6", "6stem", "6s", "roformer_6s", "htdemucs_6s") else 4


def load_slice(path: str, start: float, duration: float, samplerate: int):
    import mlx.core as mx
    import mlx_audio_io as mac

    audio_mx, sr = mac.load(path, sr=samplerate, dtype="float32")
    sr = int(sr)
    if audio_mx.ndim == 1:
        audio_mx = audio_mx[:, None]
    n = int(audio_mx.shape[0])
    start_i = 0
    end_i = n
    if start and start > 0:
        start_i = max(0, min(n, int(round(float(start) * sr))))
    if duration and duration > 0:
        end_i = max(start_i, min(n, start_i + int(round(float(duration) * sr))))
    if start_i != 0 or end_i != n:
        audio_mx = audio_mx[start_i:end_i]
    if int(audio_mx.shape[0]) < 64:
        raise ValueError("Selected region is too short to separate.")
    wav = mx.transpose(audio_mx, (1, 0))
    return wav, sr


def save_wav(wav, path: Path, samplerate: int) -> None:
    import numpy as np
    import soundfile as sf

    # 32-bit float, no per-stem rescale: stems keep their level and sum back to the mix
    audio = np.asarray(wav, dtype=np.float32)
    if audio.ndim == 1:
        audio = audio[None, :]
    path.parent.mkdir(parents=True, exist_ok=True)
    sf.write(str(path), audio.T, int(samplerate), subtype="FLOAT")


def ordered_stems(stems: dict, count: int) -> list[tuple[str, object]]:
    order = STEM_ORDER_6 if count == 6 else STEM_ORDER_4
    out = []
    seen = set()
    for name in order:
        if name in stems:
            out.append((name, stems[name]))
            seen.add(name)
    for name, audio in stems.items():
        if name not in seen:
            out.append((name, audio))
    return out


def looks_like_drum_loop(wav, samplerate: int) -> bool:
    """Cheap percussive-vs-mix check so drum loops can skip mix separation.

    Conservative: a full mix must not be classified as drums-only.
    """
    import numpy as np

    audio = np.asarray(wav)
    if audio.ndim == 2:
        if audio.shape[0] <= 8 and audio.shape[1] > audio.shape[0]:
            audio = np.mean(audio, axis=0)
        else:
            audio = np.mean(audio, axis=-1 if audio.shape[-1] <= 8 else 0)
    audio = np.asarray(audio, dtype=np.float32).reshape(-1)
    n = int(audio.shape[0])
    sr = int(samplerate) or 44100
    if n < int(sr * 0.35):
        return False
    dur = n / float(sr)
    peak = float(np.max(np.abs(audio)) + 1e-9)
    rms = float(np.sqrt(np.mean(audio * audio)) + 1e-9)
    crest = peak / rms
    hop = max(128, int(sr * 0.012))
    win = hop * 2
    window = np.hanning(win).astype(np.float32)
    prev = None
    fluxes = []
    hf_ratios = []
    mid_stable = []
    prev_mid = None
    freqs = np.fft.rfftfreq(win, 1.0 / sr)
    hf_idx = freqs > 4500.0
    mid_idx = (freqs > 180.0) & (freqs < 1800.0)
    for i in range(0, n - win, hop):
        frame = audio[i : i + win] * window
        mag = np.abs(np.fft.rfft(frame))
        total = float(np.sum(mag) + 1e-9)
        hf_ratios.append(float(np.sum(mag[hf_idx])) / total)
        mid = float(np.sum(mag[mid_idx])) / total
        if prev is not None:
            fluxes.append(float(np.mean(np.maximum(mag - prev, 0.0))))
        if prev_mid is not None:
            mid_stable.append(abs(mid - prev_mid))
        prev = mag
        prev_mid = mid
    if not fluxes:
        return False
    flux_norm = float(np.median(fluxes)) / (rms + 1e-9)
    hf_med = float(np.median(hf_ratios)) if hf_ratios else 0.0
    mid_delta = float(np.median(mid_stable)) if mid_stable else 0.0
    if dur > 90.0 and crest < 10.0:
        return False
    if dur > 45.0 and flux_norm < 0.35:
        return False
    drumish = crest >= 7.5 and flux_norm >= 0.18
    drumish = drumish or (crest >= 5.8 and flux_norm >= 0.28 and hf_med >= 0.08)
    if drumish and mid_delta < 0.008 and crest < 9.0 and dur > 20.0:
        return False
    return bool(drumish)


def nearly_empty(stem, ref, samplerate: int) -> bool:
    """True when a stem holds next to nothing: quiet overall (RMS < -38 dBFS) and audible in under
    10% of 0.5 s windows, or 40 dB below `ref` (the mix, or the drums for a kit piece).
    Fit on real splits: bleed-only crash/toms/ride/hh and vocals on an instrumental fall under it;
    the quietest real hats/ride/toms (20%+ active) stay well above it."""
    import numpy as np

    a = np.asarray(stem, dtype=np.float32)
    b = np.asarray(ref, dtype=np.float32)
    if a.ndim == 2:
        a = a.mean(axis=0) if a.shape[0] <= 8 else a.mean(axis=1)
    if b.ndim == 2:
        b = b.mean(axis=0) if b.shape[0] <= 8 else b.mean(axis=1)
    n = min(a.shape[0], b.shape[0])
    hop = max(256, int(samplerate * 0.5))
    frames = n // hop
    if frames < 4:
        return False
    a = a[: frames * hop].reshape(frames, hop)
    b = b[: frames * hop].reshape(frames, hop)
    ea = float(np.mean(a * a))
    eb = float(np.mean(b * b)) + 1e-12
    rms_db = 10.0 * np.log10(ea + 1e-12)
    rel_db = 10.0 * np.log10(ea / eb + 1e-12)
    ra = np.sqrt(np.mean(a * a, axis=1))
    rb = np.sqrt(np.mean(b * b, axis=1))
    # a window counts when the stem is above -45 dBFS and within 30 dB of the reference there
    active = (ra > 10 ** (-45 / 20)) & (ra > rb * 10 ** (-30 / 20))
    frac = float(np.mean(active))
    return rel_db < -40.0 or (rms_db < -38.0 and frac < 0.10)


def split_drums(drums_wav, samplerate: int, out_dir: Path, progress_file: str | None,
                merge_empty: bool = False, merged: list | None = None) -> dict[str, str]:
    import numpy as np
    from mdxnet_infer import MDX23CInference

    write_progress(progress_file, 72, "Loading drum-split model (first run downloads weights)…")
    engine = MDX23CInference.from_pretrained("drumsep-6stem", progress=False)
    audio = np.asarray(drums_wav)
    if audio.ndim == 1:
        audio = np.stack([audio, audio], axis=0)
    # engine.separate wants (samples, 2) or (2, samples)
    write_progress(progress_file, 78, "Splitting drums into kit pieces…")
    kit = engine.separate(audio, sample_rate=int(samplerate), progress=False)
    paths = {}
    for name in DRUM_ORDER:
        if name not in kit:
            continue
        piece = kit[name]
        # (samples, 2) -> (channels, time)
        if getattr(piece, "ndim", 1) == 2 and piece.shape[0] != 2 and piece.shape[1] == 2:
            piece = piece.T
        if merge_empty and nearly_empty(piece, drums_wav, int(samplerate)):
            # nothing worth a track: it is already in the drums stem
            if merged is not None:
                merged.append((name, "drums"))
            continue
        dest = out_dir / f"{name}.wav"
        save_wav(piece, dest, samplerate)
        paths[name] = str(dest)
    return paths


def write_manifest(path: Path, payload: dict) -> None:
    lines = [
        f"OK|{1 if payload.get('ok') else 0}",
        f"ENGINE|{payload.get('engine', '')}",
        f"MODEL|{payload.get('model', '')}",
        f"SAMPLERATE|{payload.get('samplerate', 44100)}",
        f"WARNING|{payload.get('warning', '')}",
    ]
    for name, wav_path in (payload.get("stems") or {}).items():
        lines.append(f"STEM|{name}|{wav_path}")
    for name, wav_path in (payload.get("drum_stems") or {}).items():
        lines.append(f"DRUM|{name}|{wav_path}")
    for name, into in payload.get("merged") or []:
        lines.append(f"MERGED|{name}|{into}")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    json_path = path.with_suffix(".json")
    json_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")


def stem_key(path: str) -> str:
    """'mix_(Vocals)_BS-Roformer-SW.wav' -> 'vocals'."""
    name = Path(path).name
    m = re.search(r"\(([^)]+)\)", name)
    key = (m.group(1) if m else Path(path).stem).strip().lower()
    return {"instrumental": "other", "no vocals": "other"}.get(key, key)


def separate_roformer(wav, sr: int, count: int, out_dir: Path, progress_file: str | None):
    import numpy as np
    import soundfile as sf
    from mlx_audio_separator import Separator

    mix = np.asarray(wav, dtype=np.float32)
    if mix.shape[0] == 1:
        mix = np.concatenate([mix, mix], axis=0)
    # The separator peak-limits its input and each stem to normalization_threshold on its own.
    # Feed it the mix at <= -6 dBFS and undo that gain after, so stems keep their level and sum to the mix.
    peak = float(np.max(np.abs(mix))) if mix.size else 0.0
    gain = min(1.0, 0.5 / peak) if peak > 1e-9 else 1.0

    work = Path(tempfile.mkdtemp(prefix="roformer_", dir=str(out_dir)))
    try:
        src = work / "mix.wav"
        sf.write(str(src), (mix * gain).T, sr, subtype="FLOAT")
        write_progress(progress_file, 12, "Loading BS-RoFormer SW (first run downloads weights)…")
        separator = Separator(output_dir=str(work), output_format="WAV", normalization_threshold=1.0)
        separator.load_model(model_filename=ROFORMER_CKPT)
        write_progress(progress_file, 28, "Separating stems on Apple GPU (BS-RoFormer SW)…")
        outputs = separator.separate(str(src))
        stems = {}
        for out in outputs or []:
            out_path = Path(out)
            if not out_path.is_absolute():
                out_path = work / out_path
            data, _ = sf.read(str(out_path), dtype="float32", always_2d=True)
            stems[stem_key(str(out_path))] = data.T / gain
    finally:
        shutil.rmtree(work, ignore_errors=True)
    if not stems:
        raise RuntimeError("BS-RoFormer SW produced no stems.")

    if count == 4:
        # fold guitar + piano back into "other" for the 4-stem layout
        extra = [stems.pop(k) for k in ("guitar", "piano") if k in stems]
        if extra:
            n = min(a.shape[-1] for a in extra + ([stems["other"]] if "other" in stems else []))
            other = stems["other"][..., :n] if "other" in stems else 0
            for a in extra:
                other = other + a[..., :n]
            stems["other"] = other
    return stems, sr


def fold_skipped(stems: dict, skipped: list[str]) -> dict:
    """Blend the stems the user turned off into "other" (dropped when "other" is off too)."""
    keep = {k: v for k, v in stems.items() if k not in skipped}
    gone = [v for k, v in stems.items() if k in skipped and k != "other"]
    if "other" in keep and gone:
        n = min(a.shape[-1] for a in gone + [keep["other"]])
        other = keep["other"][..., :n]
        for a in gone:
            other = other + a[..., :n]
        keep["other"] = other
    return keep


def run(args: argparse.Namespace) -> dict:
    count = resolve_stems(args.stems or args.model)
    model = f"{ENGINE}-{count}stem"
    out_dir = Path(args.output_dir).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    write_progress(args.progress_file, 6, "Loading audio…")
    wav, sr = load_slice(args.input, args.start, args.duration, SAMPLERATE)
    skip_mix = bool(args.drums_only)
    detect_note = ""
    if args.auto_skip_mix and not skip_mix:
        write_progress(args.progress_file, 10, "Checking if this is already a drum loop…")
        try:
            skip_mix = looks_like_drum_loop(wav, sr)
        except Exception:
            skip_mix = False
        detect_note = "drum loop" if skip_mix else "full mix"

    warning = ""
    merged: list[tuple[str, str]] = []
    stem_paths: dict[str, str] = {}
    drum_paths: dict[str, str] = {}
    engine = ENGINE

    if skip_mix:
        engine = "drums-only"
        write_progress(args.progress_file, 18, "Drum loop — skipping mix split…")
        drums_dest = out_dir / "drums.wav"
        save_wav(wav, drums_dest, sr)
        stem_paths["drums"] = str(drums_dest)
        if args.split_drums:
            try:
                drum_paths = split_drums(wav, sr, out_dir, args.progress_file, args.merge_empty, merged)
            except Exception as exc:
                warning = f"Drum split failed: {exc}"
        if detect_note == "drum loop" and not warning:
            warning = "Skipped mix split (already a drum loop)."
    else:
        stems, sr = separate_roformer(wav, sr, count, out_dir, args.progress_file)
        drums_for_kit = stems.get("drums")
        skipped = [k.strip().lower() for k in (args.skip_stems or "").split(",") if k.strip()]
        if skipped:
            stems = fold_skipped(stems, skipped)
        if args.merge_empty and "other" in stems:
            import numpy as np
            mix_np = np.asarray(wav, dtype=np.float32)
            empty = [k for k, v in stems.items() if k != "other" and nearly_empty(v, mix_np, sr)]
            if empty:
                stems = fold_skipped(stems, empty)
                merged.extend((k, "other") for k in empty)

        write_progress(args.progress_file, 62, "Writing stem files…")
        for name, audio in ordered_stems(stems, count):
            dest = out_dir / f"{name}.wav"
            save_wav(audio, dest, sr)
            stem_paths[name] = str(dest)

        if args.split_drums:
            drums = drums_for_kit
            if drums is None:
                warning = "No drums stem to split."
            else:
                try:
                    drum_paths = split_drums(drums, sr, out_dir, args.progress_file, args.merge_empty, merged)
                except Exception as exc:
                    warning = f"Drum split failed: {exc}"

    write_progress(args.progress_file, 100, "Done")
    return {
        "ok": True,
        "engine": engine,
        "model": model,
        "samplerate": sr,
        "warning": warning,
        "stems": stem_paths,
        "drum_stems": drum_paths,
        "output_dir": str(out_dir),
        "skipped_mix": skip_mix,
        "merged": [list(m) for m in merged],
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Split a mix into stems for REAPER.")
    p.add_argument("--input", required=True, help="Source audio file")
    p.add_argument("--output-dir", required=True, help="Directory for stem WAVs")
    p.add_argument("--stems", default="", help="4 (vocals/drums/bass/other) or 6 (adds guitar + piano)")
    p.add_argument("--model", default="", help="Deprecated: old model name, mapped to --stems")
    p.add_argument("--split-drums", action="store_true", help="Also split drums into kit pieces")
    p.add_argument("--drums-only", action="store_true", help="Skip mix separation; treat the input as drums")
    p.add_argument("--skip-stems", default="", help="Comma list of mix stems to leave out; they are blended into other")
    p.add_argument("--merge-empty", action="store_true", help="Blend nearly empty stems back (mix stems into other; kit pieces stay in drums)")
    p.add_argument("--auto-skip-mix", action="store_true", help="Skip mix separation when the audio looks like a drum loop")
    p.add_argument("--start", type=float, default=0.0, help="Start offset in source seconds")
    p.add_argument("--duration", type=float, default=0.0, help="Duration in source seconds (0 = rest of file)")
    p.add_argument("--progress-file", default="", help="Two-line percent/message file")
    p.add_argument("--done-flag", default="", help="Written with process exit code when finished")
    p.add_argument("--manifest", default="", help="Manifest path (default: output-dir/manifest.txt)")
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    progress_file = args.progress_file or None
    done_flag = args.done_flag or None
    code = 1
    try:
        payload = run(args)
        manifest = Path(args.manifest) if args.manifest else Path(args.output_dir) / "manifest.txt"
        write_manifest(manifest, payload)
        print(json.dumps(payload, indent=2))
        code = 0
    except Exception as exc:
        write_progress(progress_file, 100, f"Error: {exc}")
        err = {
            "ok": False,
            "error": str(exc),
            "traceback": traceback.format_exc(),
        }
        print(json.dumps(err, indent=2), file=sys.stderr)
        try:
            manifest = Path(args.manifest) if args.manifest else Path(args.output_dir) / "manifest.txt"
            write_manifest(manifest, {
                "ok": False,
                "engine": ENGINE,
                "model": ENGINE,
                "samplerate": 44100,
                "warning": str(exc),
                "stems": {},
                "drum_stems": {},
            })
        except Exception:
            pass
        code = 1
    finally:
        write_done(done_flag, code)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
