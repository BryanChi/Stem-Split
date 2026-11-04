# Stem Split plugin (CLAP / VST3)

The Stem Split separator as a plugin any DAW can load. Put it on a track, then:

- **Drop an audio file** on the window (or **Open file…**), or press **Capture** and play the
  track: the plugin records its input while the transport runs and splits it when you stop.
- Wait for the split (same engine as the REAPER script: BS-RoFormer SW on Apple-silicon MLX).
- The stems **play back inside the plugin**, locked to the DAW timeline, each with its own
  **volume fader, mute and solo** (all automatable). Captured audio lines up with where it was
  recorded; a dropped file starts at the playhead (**Set to playhead** moves it).
- **Drag a stem's name** (or **Drag all**) out of the window into the DAW to get the WAV files.
  A stem whose fader isn't at 0 dB is dragged with that volume applied.

Stems are kept in `~/Music/Stem Split/<name> <date>/` so dragged files stay valid, and the
plugin reloads them when the project is reopened. **Bypass** passes the track's own audio through.

## Engine

The separation runs `StemSplit.py` (built into the plugin) in a separate Python process, so the
DAW never blocks. The plugin looks for Python in this order:

1. `STEM_SPLIT_PYTHON`, or the Python picked with **Choose Python…**
   (`~/Library/Application Support/Stem Split/config.json`)
2. Its own venv: `~/Library/Application Support/Stem Split/.venv-stems` (**Install engine** makes it)
3. The REAPER script's `.venv-stems` under `REAPER/Scripts` (Bryan's Scripts, Stem Split,
   CoolReaperScripts/Stem Split, or any other folder there)

Separation needs an Apple-silicon Mac (MLX). The plugin itself builds for macOS, Windows and Linux.

## Build

```bash
cd plugin
./build_mac.sh            # universal CLAP + VST3, installed to ~/Library/Audio/Plug-Ins
cargo xtask bundle stem_split_plugin --release   # any OS, current arch -> target/bundled/
```

Tests: `cargo test --lib`. End to end with a stand-in separator:
`STEM_SPLIT_PYTHON=<python> STEM_SPLIT_TEST_INPUT=<audio> cargo test --lib -- --ignored`.
Standalone app for testing without a DAW: `cargo run --features standalone -- --backend dummy`.

## Notes

- Built with [NIH-plug](https://github.com/robbert-vdh/nih-plug) (egui editor). Its VST3 wrapper
  is GPLv3, so the plugin is GPL-3.0-or-later.
- `vendor/egui-baseview` is egui-baseview (MIT) with one change: it accepts files dragged onto the
  window (upstream drops those events).
- Dragging stems out uses a native AppKit drag on macOS. On Windows / Linux the stem is shown in
  the file manager instead, for now.
