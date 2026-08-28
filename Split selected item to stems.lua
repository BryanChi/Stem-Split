-- @description Split selected item to stems (Demucs-MLX)
-- @version 1.1
-- @author BRYAN
-- @about
--   Splits the selected audio item into vocals / drums / bass / other using
--   Demucs-MLX on Apple Silicon. Optional 6-stem (guitar + piano), drum
--   kit split, and tempo map from the drums stem. Requires .venv-stems
--   next to this script.
-- @provides [main] .

local r = reaper
local EXT = "BRYAN_STEM_SPLIT"

local STEM_LABEL = {
  vocals = "Vocals",
  drums = "Drums",
  bass = "Bass",
  other = "Other",
  guitar = "Guitar",
  piano = "Piano",
  kick = "Kick",
  snare = "Snare",
  toms = "Toms",
  hh = "Hats",
  ride = "Ride",
  crash = "Crash",
}

local STEM_RGB = {
  vocals = {220, 70, 90},
  drums = {230, 180, 50},
  bass = {70, 120, 220},
  other = {140, 140, 150},
  guitar = {230, 120, 50},
  piano = {160, 90, 200},
  kick = {200, 60, 60},
  snare = {230, 200, 80},
  toms = {180, 100, 50},
  hh = {200, 220, 90},
  ride = {120, 200, 160},
  crash = {90, 180, 220},
}

local MIX_ORDER = {"vocals", "drums", "bass", "guitar", "piano", "other"}
local DRUM_ORDER = {"kick", "snare", "toms", "hh", "ride", "crash"}

local JOB = nil

local function script_dir()
  local src = debug.getinfo(1, "S").source
  if src:sub(1, 1) == "@" then
    src = src:sub(2)
  end
  src = src:gsub("\\", "/")
  return src:match("^(.*)/") or "."
end

local function join_path(a, b)
  if not a or a == "" then return b end
  if a:sub(-1) == "/" or a:sub(-1) == "\\" then
    return a .. b
  end
  return a .. "/" .. b
end

local function dirname(p)
  if not p or p == "" then return "" end
  return p:match("^(.*)[/\\][^/\\]-$") or ""
end

local function basename_no_ext(p)
  local name = p:match("([^/\\]+)$") or p
  return name:match("^(.*)%.[^%.]+$") or name
end

local function file_exists(path)
  if not path or path == "" then return false end
  if r.file_exists then
    return r.file_exists(path)
  end
  local fh = io.open(path, "r")
  if fh then
    fh:close()
    return true
  end
  return false
end

local function shell_quote(s)
  s = tostring(s or ""):gsub("'", "'\\''")
  return "'" .. s .. "'"
end

local function sanitize_dir_name(s)
  s = tostring(s or "stems")
  s = s:gsub("[<>:\"/\\|?*]", "_")
  s = s:gsub("%s+", " ")
  s = s:gsub("^%s+", ""):gsub("%s+$", "")
  if s == "" then
    s = "stems"
  end
  return s
end

local function ext_get(key, default)
  local v = r.GetExtState(EXT, key)
  if not v or v == "" then
    return default
  end
  return v
end

local function ext_set(key, value)
  r.SetExtState(EXT, key, tostring(value or ""), true)
end

local function yn(s, default_true)
  if not s or s == "" then
    return default_true
  end
  s = s:lower()
  if s == "y" or s == "yes" or s == "1" or s == "true" then
    return true
  end
  if s == "n" or s == "no" or s == "0" or s == "false" then
    return false
  end
  return default_true
end

local function filename_from_source(src)
  local a, b = r.GetMediaSourceFileName(src, "")
  if type(b) == "string" and b ~= "" then return b end
  if type(a) == "string" and a ~= "" then return a end
  a, b = r.GetMediaSourceFileName(src)
  if type(b) == "string" and b ~= "" then return b end
  if type(a) == "string" and a ~= "" then return a end
  return nil
end

local function resolve_media_path(src)
  local depth = 0
  while src and depth < 64 do
    local fn = filename_from_source(src)
    if fn and fn ~= "" then
      return fn
    end
    if not r.GetMediaSourceParent then
      break
    end
    src = r.GetMediaSourceParent(src)
    depth = depth + 1
  end
  return nil
end

local function project_dir()
  local _, projfn = r.EnumProjects(-1, "")
  if projfn and projfn ~= "" then
    return dirname(projfn)
  end
  return ""
end

local function absolutize(path)
  if not path or path == "" then return nil end
  if path:sub(1, 1) == "/" then return path end
  if path:match("^%a:[/\\]") then return path end
  local pd = project_dir()
  if pd ~= "" then
    return join_path(pd, path)
  end
  return path
end

local function track_index(tr)
  return math.floor(r.GetMediaTrackInfo_Value(tr, "IP_TRACKNUMBER") + 0.5) - 1
end

local function last_folder_descendant_index(tr)
  local idx = track_index(tr)
  local depth = r.GetMediaTrackInfo_Value(tr, "I_FOLDERDEPTH") or 0
  if depth < 1 then
    return idx
  end
  local n = r.CountTracks(0)
  for i = idx + 1, n - 1 do
    depth = depth + (r.GetMediaTrackInfo_Value(r.GetTrack(0, i), "I_FOLDERDEPTH") or 0)
    if depth <= 0 then
      return i
    end
  end
  return n - 1
end

local function native_color(rgb)
  local rr, gg, bb = rgb[1], rgb[2], rgb[3]
  if r.ColorToNative then
    return r.ColorToNative(rr, gg, bb) | 0x1000000
  end
  return 0x1000000 | (rr + gg * 256 + bb * 65536)
end

local function set_track_color(tr, name)
  local rgb = STEM_RGB[name] or {160, 160, 160}
  r.SetTrackColor(tr, native_color(rgb))
end

local function insert_track_after(after_idx)
  r.InsertTrackAtIndex(after_idx + 1, true)
  return r.GetTrack(0, after_idx + 1), after_idx + 1
end

local function create_stem_tracks(src_track, folder_name, mix_names, drum_names)
  local insert_after = last_folder_descendant_index(src_track)
  local close_depth = r.GetMediaTrackInfo_Value(r.GetTrack(0, insert_after), "I_FOLDERDEPTH") or 0

  local folder, folder_idx = insert_track_after(insert_after)
  r.GetSetMediaTrackInfo_String(folder, "P_NAME", folder_name, true)
  r.SetMediaTrackInfo_Value(folder, "I_FOLDERCOMPACT", 0)

  local children = {}
  local prev_idx = folder_idx
  local function add_child(key, label)
    local tr, idx = insert_track_after(prev_idx)
    r.GetSetMediaTrackInfo_String(tr, "P_NAME", label, true)
    set_track_color(tr, key)
    children[#children + 1] = { key = key, track = tr }
    prev_idx = idx
    return tr
  end

  for _, key in ipairs(mix_names) do
    add_child(key, STEM_LABEL[key] or key)
    if key == "drums" then
      for _, dkey in ipairs(drum_names) do
        add_child(dkey, STEM_LABEL[dkey] or dkey)
      end
    end
  end

  r.SetMediaTrackInfo_Value(folder, "I_FOLDERDEPTH", 1)
  local last = children[#children]
  if last then
    local extra = 0
    if close_depth < 0 then
      extra = close_depth
      r.SetMediaTrackInfo_Value(r.GetTrack(0, insert_after), "I_FOLDERDEPTH", 0)
    end
    r.SetMediaTrackInfo_Value(last.track, "I_FOLDERDEPTH", -1 + extra)
  elseif close_depth < 0 then
    r.SetMediaTrackInfo_Value(folder, "I_FOLDERDEPTH", 1 + close_depth)
    r.SetMediaTrackInfo_Value(r.GetTrack(0, insert_after), "I_FOLDERDEPTH", 0)
  end

  return folder, children
end

local function insert_wav_on_track(track, wav_path, position)
  r.Main_OnCommand(40289, 0) -- unselect items
  r.SetOnlyTrackSelected(track)
  r.SetEditCurPos(position, false, false)
  local inserted = r.InsertMedia(wav_path, 0)
  if not inserted or inserted == 0 then
    return nil
  end
  local item = r.GetSelectedMediaItem(0, 0)
  if item then
    r.SetMediaItemInfo_Value(item, "D_POSITION", position)
  end
  return item
end

local function parse_manifest(path)
  local fh = io.open(path, "r")
  if not fh then
    return nil, "Could not read manifest:\n" .. path
  end
  local info = {
    ok = false,
    engine = "",
    model = "",
    warning = "",
    stems = {},
    drum_stems = {},
  }
  for line in fh:lines() do
    local kind, a, b = line:match("^([^|]+)|([^|]*)|(.*)$")
    if not kind then
      kind, a = line:match("^([^|]+)|(.*)$")
    end
    if kind == "OK" then
      info.ok = a == "1"
    elseif kind == "ENGINE" then
      info.engine = a or ""
    elseif kind == "MODEL" then
      info.model = a or ""
    elseif kind == "WARNING" then
      info.warning = a or ""
    elseif kind == "STEM" and a and b then
      info.stems[a] = b
    elseif kind == "DRUM" and a and b then
      info.drum_stems[a] = b
    end
  end
  fh:close()
  return info
end

local function ordered_present(order, map)
  local out = {}
  for _, key in ipairs(order) do
    if map[key] then
      out[#out + 1] = key
    end
  end
  return out
end

local function read_progress(path)
  local f = io.open(path, "r")
  if not f then
    return 0, ""
  end
  local pct = tonumber((f:read("*l") or "0"):match("%d+")) or 0
  local msg = f:read("*l") or ""
  f:close()
  if pct < 0 then pct = 0 elseif pct > 100 then pct = 100 end
  return pct, msg:gsub("[\r\n]", "")
end

local function read_done(path)
  local f = io.open(path, "r")
  if not f then
    return nil
  end
  local s = f:read("*a") or ""
  f:close()
  return tonumber(s:match("%-?%d+"))
end

local function spawn_detached(cmd)
  local line = "/bin/sh -c " .. shell_quote(cmd .. " >/dev/null 2>&1") .. " &"
  local rc = os.execute(line)
  return rc == true or rc == 0 or rc == nil
end

local function collect_jobs()
  local n = r.CountSelectedMediaItems(0)
  if n < 1 then
    return nil, "Select one or more audio items."
  end
  local jobs = {}
  for i = 0, n - 1 do
    local item = r.GetSelectedMediaItem(0, i)
    local take = item and r.GetActiveTake(item)
    if item and take and not r.TakeIsMIDI(take) then
      local src = r.GetMediaItemTake_Source(take)
      local path = src and resolve_media_path(src)
      path = path and absolutize(path)
      if path and file_exists(path) then
        local _, take_name = r.GetSetMediaItemTakeInfo_String(take, "P_NAME", "", false)
        jobs[#jobs + 1] = {
          item = item,
          take = take,
          track = r.GetMediaItemTrack(item),
          path = path,
          name = (take_name and take_name ~= "" and take_name) or basename_no_ext(path),
          position = r.GetMediaItemInfo_Value(item, "D_POSITION") or 0,
          length = r.GetMediaItemInfo_Value(item, "D_LENGTH") or 0,
          startoffs = r.GetMediaItemTakeInfo_Value(take, "D_STARTOFFS") or 0,
          playrate = r.GetMediaItemTakeInfo_Value(take, "D_PLAYRATE") or 1,
        }
      end
    end
  end
  if #jobs < 1 then
    return nil, "No usable audio items.\n\nItems need a file on disk (render/glue first if this is a recorded take still in RAM, MIDI, or an FX chain you want baked in)."
  end
  return jobs
end

local function ask_options()
  local model = ext_get("MODEL", "fast")
  local drums = ext_get("SPLIT_DRUMS", "n")
  local mute = ext_get("MUTE_ORIGINAL", "y")
  local tempo = ext_get("TEMPO_MAP", "n")
  local ok, csv = r.GetUserInputs(
    "Split to stems",
    4,
    "Model (fast / good / 6stem),Split drums into kit (y/n),Mute original item (y/n),Tempo map from drums (y/n)",
    table.concat({model, drums, mute, tempo}, ",")
  )
  if not ok then
    return nil
  end
  local a, b, c, d = csv:match("^(.-),(.-),(.-),(.*)$")
  if not a then
    a, b, c = csv:match("^(.-),(.-),(.*)$")
    d = tempo
  end
  if not a then
    return nil
  end
  a = a:gsub("^%s+", ""):gsub("%s+$", ""):lower()
  if a ~= "fast" and a ~= "good" and a ~= "6stem" and a ~= "htdemucs" and a ~= "htdemucs_ft" and a ~= "htdemucs_6s" then
    a = "fast"
  end
  ext_set("MODEL", a)
  ext_set("SPLIT_DRUMS", yn(b, false) and "y" or "n")
  ext_set("MUTE_ORIGINAL", yn(c, true) and "y" or "n")
  ext_set("TEMPO_MAP", yn(d, false) and "y" or "n")
  return {
    model = a,
    split_drums = yn(b, false),
    mute_original = yn(c, true),
    tempo_map = yn(d, false),
  }
end

local function job_paths(job)
  local parent = dirname(job.path)
  if parent == "" then
    parent = project_dir()
  end
  if parent == "" then
    parent = os.getenv("HOME") or "/tmp"
  end
  local out_dir = join_path(parent, sanitize_dir_name(job.name) .. " stems")
  return {
    out_dir = out_dir,
    manifest = join_path(out_dir, "manifest.txt"),
    progress = join_path(out_dir, "_progress.txt"),
    done = join_path(out_dir, "_done.txt"),
    log = join_path(out_dir, "_run.log"),
  }
end

local function python_cmd(py, sidecar, job, opts, paths)
  local duration = job.length * (job.playrate ~= 0 and job.playrate or 1)
  local parts = {
    shell_quote(py),
    shell_quote(sidecar),
    "--input", shell_quote(job.path),
    "--output-dir", shell_quote(paths.out_dir),
    "--model", shell_quote(opts.model),
    "--start", string.format("%.6f", job.startoffs or 0),
    "--duration", string.format("%.6f", duration or 0),
    "--progress-file", shell_quote(paths.progress),
    "--done-flag", shell_quote(paths.done),
    "--manifest", shell_quote(paths.manifest),
  }
  if opts.split_drums then
    parts[#parts + 1] = "--split-drums"
  end
  return table.concat(parts, " ") .. " > " .. shell_quote(paths.log) .. " 2>&1"
end

local function import_result(job, opts, info)
  if not job.item or not job.track then
    return false, "Original item is gone. Stems are on disk."
  end
  local mix_names = ordered_present(MIX_ORDER, info.stems)
  local drum_names = ordered_present(DRUM_ORDER, info.drum_stems)
  if #mix_names < 1 then
    return false, "No stem files were written."
  end

  r.Undo_BeginBlock()
  r.PreventUIRefresh(1)
  local folder, children = create_stem_tracks(
    job.track,
    job.name .. " stems",
    mix_names,
    drum_names
  )
  local imported = 0
  local drums_item = nil
  for _, child in ipairs(children) do
    local wav = info.stems[child.key] or info.drum_stems[child.key]
    if wav and file_exists(wav) then
      local item = insert_wav_on_track(child.track, wav, job.position)
      if item then
        imported = imported + 1
        if child.key == "drums" then
          drums_item = item
        end
      end
    end
  end
  if opts.mute_original then
    r.SetMediaItemInfo_Value(job.item, "B_MUTE", 1)
  end
  r.PreventUIRefresh(-1)
  r.UpdateArrange()
  r.Undo_EndBlock(string.format("Split to stems (%d files)", imported), -1)
  return imported > 0, folder and (imported .. " stem(s) imported.") or "Import failed.", drums_item
end

local function load_tempo_map_lib()
  if type(Bryan_ApplyDrumStemTempoMap) == "function" then
    return true
  end
  local path = join_path(script_dir(), "Tempo map from drum stem.lua")
  if not file_exists(path) then
    return false, "Tempo map from drum stem.lua not found next to this script."
  end
  Bryan_DrumStemTempoMap_AsLib = true
  local ok, err = pcall(dofile, path)
  Bryan_DrumStemTempoMap_AsLib = nil
  if not ok then
    return false, tostring(err)
  end
  if type(Bryan_ApplyDrumStemTempoMap) ~= "function" then
    return false, "Tempo map function was not exported."
  end
  return true
end

local function tempo_map_drums_item(item)
  local ok, err = load_tempo_map_lib()
  if not ok then
    return false, err
  end
  return Bryan_ApplyDrumStemTempoMap(item, { quiet = true })
end

local function finish_current()
  local j = JOB
  if not j then
    return
  end
  local job = j.batch[j.cur]
  local paths = job.paths
  local code = read_done(paths.done)
  if code ~= 0 then
    local log = ""
    local lf = io.open(paths.log, "r")
    if lf then
      log = lf:read("*a") or ""
      lf:close()
    end
    if j.gfx_on and gfx and gfx.quit then
      gfx.quit()
    end
    JOB = nil
    r.MB(
      "Stem split failed (exit " .. tostring(code) .. ").\n\n"
        .. (log ~= "" and log:sub(-2500) or ("See log:\n" .. paths.log)),
      "Split to stems",
      0
    )
    return
  end

  local info, err = parse_manifest(paths.manifest)
  if not info or not info.ok then
    if j.gfx_on and gfx and gfx.quit then
      gfx.quit()
    end
    JOB = nil
    r.MB(err or (info and info.warning) or "Stem split produced no output.", "Split to stems", 0)
    return
  end

  local ok, msg, drums_item = import_result(job, j.opts, info)
  if not ok then
    if j.gfx_on and gfx and gfx.quit then
      gfx.quit()
    end
    JOB = nil
    r.MB(msg, "Split to stems", 0)
    return
  end
  if info.warning and info.warning ~= "" then
    j.warnings[#j.warnings + 1] = job.name .. ": " .. info.warning
  end
  if j.opts.tempo_map then
    if drums_item then
      if j.gfx_on and gfx then
        gfx.clear = 0x202020
        gfx.set(230, 230, 230, 255)
        if gfx.setfont then
          gfx.setfont(1, "Arial", 15)
        end
        gfx.x = 16
        gfx.y = 10
        if gfx.drawstr then
          gfx.drawstr("Tempo mapping drums…")
        end
        gfx.update()
      end
      local tok, tmsg = tempo_map_drums_item(drums_item)
      if tok then
        j.warnings[#j.warnings + 1] = job.name .. ": " .. tostring(tmsg)
      else
        j.warnings[#j.warnings + 1] = job.name .. ": tempo map failed (" .. tostring(tmsg or "unknown") .. ")"
      end
    else
      j.warnings[#j.warnings + 1] = job.name .. ": tempo map skipped (no drums stem)"
    end
  end

  if j.cur < #j.batch then
    j.cur = j.cur + 1
    local next_job = j.batch[j.cur]
    pcall(os.remove, next_job.paths.done)
    if not spawn_detached(next_job.cmd) then
      if j.gfx_on and gfx and gfx.quit then
        gfx.quit()
      end
      JOB = nil
      r.MB("Could not start the next stem-split job.", "Split to stems", 0)
      return
    end
    j.t0 = r.time_precise()
    r.defer(poll)
    return
  end

  if j.gfx_on and gfx and gfx.quit then
    gfx.quit()
  end
  JOB = nil
  local extra = ""
  if #j.warnings > 0 then
    extra = "\n\n" .. table.concat(j.warnings, "\n")
  end
  r.MB(
    string.format("Imported stems for %d item(s).%s", #j.batch, extra),
    "Split to stems",
    0
  )
end

function poll()
  local j = JOB
  if not j then
    return
  end
  if r.time_precise() - j.t0 > j.max_wait then
    if j.gfx_on and gfx and gfx.quit then
      gfx.quit()
    end
    JOB = nil
    r.MB("Stem split timed out. The Python process may still be running.", "Split to stems", 0)
    return
  end

  local job = j.batch[j.cur]
  local code = read_done(job.paths.done)
  if code == nil then
    if j.gfx_on and gfx then
      local ch = gfx.getchar and gfx.getchar() or 0
      if ch == -1 then
        j.gfx_on = false
      else
        local pct, msg = read_progress(job.paths.progress)
        gfx.clear = 0x202020
        gfx.set(70, 70, 70, 255)
        gfx.rect(16, 38, 388, 16, 1)
        gfx.set(55, 130, 220, 255)
        gfx.rect(16, 38, math.floor(math.max(0, 388 * pct / 100)), 16, 1)
        gfx.set(230, 230, 230, 255)
        if gfx.setfont then
          gfx.setfont(1, "Arial", 15)
        end
        gfx.x = 16
        gfx.y = 10
        if gfx.drawstr then
          gfx.drawstr(string.format(
            "Stems [%d/%d]  %d%%  %s",
            j.cur, #j.batch, pct, msg
          ))
        end
        gfx.update()
      end
    end
    r.defer(poll)
    return
  end
  finish_current()
end

local function main()
  if JOB then
    r.MB("A stem split is already running.", "Split to stems", 0)
    return
  end
  if r.GetOS():match("Win") then
    r.MB("This script currently uses Demucs-MLX and is Mac / Apple Silicon only.", "Split to stems", 0)
    return
  end

  local dir = script_dir()
  local py = join_path(dir, ".venv-stems/bin/python")
  local sidecar = join_path(dir, "StemSplit.py")
  if not file_exists(py) then
    r.MB(
      "Python environment not found:\n" .. py .. "\n\n"
        .. "Create it with:\n"
        .. "  python3.13 -m venv .venv-stems\n"
        .. "  .venv-stems/bin/pip install 'demucs-mlx[convert]' mdxnet-infer soundfile",
      "Split to stems",
      0
    )
    return
  end
  if not file_exists(sidecar) then
    r.MB("StemSplit.py not found next to this script:\n" .. sidecar, "Split to stems", 0)
    return
  end

  local jobs, err = collect_jobs()
  if not jobs then
    r.MB(err, "Split to stems", 0)
    return
  end

  local opts = ask_options()
  if not opts then
    return
  end

  for _, job in ipairs(jobs) do
    job.paths = job_paths(job)
    pcall(os.remove, job.paths.done)
    pcall(os.remove, job.paths.progress)
    r.RecursiveCreateDirectory(job.paths.out_dir, 0)
    job.cmd = python_cmd(py, sidecar, job, opts, job.paths)
  end

  if not spawn_detached(jobs[1].cmd) then
    r.MB("Could not start StemSplit.py.", "Split to stems", 0)
    return
  end

  local gfx_on = false
  if gfx and gfx.init then
    gfx.init("Split to stems", 420, 70, 0, 200, 200)
    gfx_on = true
  end

  JOB = {
    batch = jobs,
    cur = 1,
    opts = opts,
    t0 = r.time_precise(),
    max_wait = 20 * 60,
    gfx_on = gfx_on,
    warnings = {},
  }
  r.defer(poll)
end

main()
