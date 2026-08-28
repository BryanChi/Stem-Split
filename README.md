# Stem Split

REAPER script that splits the selected audio item into vocals / drums / bass / other using Demucs-MLX on Apple Silicon. Optional 6-stem (guitar + piano), drum-kit split, and tempo map from the drums stem.

## Requirements

- [REAPER](https://www.reaper.fm/)
- macOS on Apple Silicon (Demucs-MLX)
- Python 3.13
- A local virtualenv next to these files named `.venv-stems`

## Install

1. Clone this repo into REAPER's Scripts folder, for example:

   `~/Library/Application Support/REAPER/Scripts/Stem Split`

2. Create the Python environment:

   ```bash
   cd ~/Library/Application\ Support/REAPER/Scripts/Stem\ Split
   python3.13 -m venv .venv-stems
   .venv-stems/bin/pip install -r requirements.txt
   ```

3. In REAPER: **Actions → Show action list → Load ReaScript…** and load:

   - `Split selected item to stems.lua`
   - `Tempo map from drum stem.lua` (optional companion)

First run downloads model weights. WAV files work as-is.

## Layout

| Path | Role |
| --- | --- |
| `Split selected item to stems.lua` | Main action: split selected item(s) onto new tracks |
| `StemSplit.py` | Demucs-MLX / drum-split sidecar |
| `Tempo map from drum stem.lua` | Optional: tempo-map the project from the drums stem |

`.venv-stems/` is created locally and is not committed.
