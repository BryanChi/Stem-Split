#!/usr/bin/env python3
"""REAPER sidecar: split a mix into stems with BS-RoFormer SW (MLX) or Demucs-MLX,
optionally drums into kit pieces."""

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


# BS-RoFormer SW: ~3 dB cleaner than htdemucs on every stem, far less metallic.
ROFORMER_4 = "roformer"
ROFORMER_6 = "roformer_6s"
ROFORMER_CKPT = "BS-Roformer-SW.ckpt"

MODEL_ALIASES = {
    "fast": "htdemucs",
    "good": "htdemucs_ft",
    "best": ROFORMER_4,
    "6stem": ROFORMER_6,
    "6s": ROFORMER_6,
    "roformer": ROFORMER_4,
    "roformer_6s": ROFORMER_6,
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


# Extra shifted passes average out Demucs' mask flicker (the "metallic" residue).
DEMUCS_SHIFTS = {"htdemucs": 1, "htdemucs_ft": 2, "htdemucs_6s": 2}


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


def ordered_stems(stems: dict, model: str) -> list[tuple[str, object]]:
    order = STEM_ORDER_6 if model in ("htdemucs_6s", ROFORMER_6) else STEM_ORDER_4
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


def stem_key(path: str) -> str:
    """'mix_(Vocals)_BS-Roformer-SW.wav' -> 'vocals'."""
    name = Path(path).name
    m = re.search(r"\(([^)]+)\)", name)
    key = (m.group(1) if m else Path(path).stem).strip().lower()
    return {"instrumental": "other", "no vocals": "other"}.get(key, key)


def separate_roformer(args: argparse.Namespace, model: str, out_dir: Path):
    import numpy as np
    import soundfile as sf
    from mlx_audio_separator import Separator

    write_progress(args.progress_file, 6, "Loading audio…")
    wav, sr = load_slice(args.input, args.start, args.duration, 44100)
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
        write_progress(args.progress_file, 12, "Loading BS-RoFormer SW (first run downloads weights)…")
        separator = Separator(output_dir=str(work), output_format="WAV", normalization_threshold=1.0)
        separator.load_model(model_filename=ROFORMER_CKPT)
        write_progress(args.progress_file, 28, "Separating stems on Apple GPU (BS-RoFormer SW)…")
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

    if model == ROFORMER_4:
        # fold guitar + piano back into "other" for the 4-stem layout
        extra = [stems.pop(k) for k in ("guitar", "piano") if k in stems]
        if extra:
            n = min(a.shape[-1] for a in extra + ([stems["other"]] if "other" in stems else []))
            other = stems["other"][..., :n] if "other" in stems else 0
            for a in extra:
                other = other + a[..., :n]
            stems["other"] = other
    return stems, sr


def run(args: argparse.Namespace) -> dict:
    model = resolve_model(args.model)
    out_dir = Path(args.output_dir).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    if model in (ROFORMER_4, ROFORMER_6):
        engine = "mlx-audio-separator"
        stems, sr = separate_roformer(args, model, out_dir)
    else:
        engine = "demucs-mlx"
        write_progress(args.progress_file, 4, f"Loading {model}…")
        from demucs_mlx import Separator

        separator = Separator(model=model, shifts=DEMUCS_SHIFTS.get(model, 1), progress=True, seed=0)
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
        "engine": engine,
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
    p.add_argument("--model", default="fast", help="fast, good, best (BS-RoFormer SW), 6stem, or a Demucs model name")
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
