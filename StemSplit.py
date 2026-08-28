#!/usr/bin/env python3
"""REAPER sidecar: split a mix into stems with Demucs-MLX, optionally drums into kit pieces."""

from __future__ import annotations

import argparse
import json
import os
import sys
import traceback
from pathlib import Path


MODEL_ALIASES = {
    "fast": "htdemucs",
    "good": "htdemucs_ft",
    "best": "htdemucs_ft",
    "6stem": "htdemucs_6s",
    "6s": "htdemucs_6s",
    "htdemucs": "htdemucs",
    "htdemucs_ft": "htdemucs_ft",
    "htdemucs_6s": "htdemucs_6s",
}

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


def resolve_model(name: str) -> str:
    key = (name or "fast").strip().lower()
    if key not in MODEL_ALIASES:
        known = ", ".join(sorted(MODEL_ALIASES))
        raise ValueError(f"Unknown model '{name}'. Use one of: {known}")
    return MODEL_ALIASES[key]


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
    from demucs_mlx import save_audio

    path.parent.mkdir(parents=True, exist_ok=True)
    save_audio(wav, str(path), samplerate=samplerate, clip="rescale", bits_per_sample=16)


def ordered_stems(stems: dict, model: str) -> list[tuple[str, object]]:
    order = STEM_ORDER_6 if model == "htdemucs_6s" else STEM_ORDER_4
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


def split_drums(drums_wav, samplerate: int, out_dir: Path, progress_file: str | None) -> dict[str, str]:
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
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    json_path = path.with_suffix(".json")
    json_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")


def run(args: argparse.Namespace) -> dict:
    model = resolve_model(args.model)
    out_dir = Path(args.output_dir).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    write_progress(args.progress_file, 4, f"Loading {model}…")
    from demucs_mlx import Separator

    separator = Separator(model=model, progress=True, seed=0)
    write_progress(args.progress_file, 18, "Loading audio…")
    wav, sr = load_slice(args.input, args.start, args.duration, separator.samplerate)

    write_progress(args.progress_file, 28, "Separating stems on Apple GPU…")
    _origin, stems = separator.separate_tensor(wav)
    sr = int(separator.samplerate)

    write_progress(args.progress_file, 62, "Writing stem files…")
    stem_paths: dict[str, str] = {}
    for name, audio in ordered_stems(stems, model):
        dest = out_dir / f"{name}.wav"
        save_wav(audio, dest, sr)
        stem_paths[name] = str(dest)

    warning = ""
    drum_paths: dict[str, str] = {}
    if args.split_drums:
        drums = stems.get("drums")
        if drums is None:
            warning = "No drums stem to split."
        else:
            try:
                drum_paths = split_drums(drums, sr, out_dir, args.progress_file)
            except Exception as exc:
                warning = f"Drum split failed: {exc}"

    write_progress(args.progress_file, 100, "Done")
    return {
        "ok": True,
        "engine": "demucs-mlx",
        "model": model,
        "samplerate": sr,
        "warning": warning,
        "stems": stem_paths,
        "drum_stems": drum_paths,
        "output_dir": str(out_dir),
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Split a mix into stems for REAPER.")
    p.add_argument("--input", required=True, help="Source audio file")
    p.add_argument("--output-dir", required=True, help="Directory for stem WAVs")
    p.add_argument("--model", default="fast", help="fast, good, 6stem, or a Demucs model name")
    p.add_argument("--split-drums", action="store_true", help="Also split drums into kit pieces")
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
                "engine": "demucs-mlx",
                "model": args.model,
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
