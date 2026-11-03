-- @description Split selected item to stems (BS-RoFormer SW)
-- @version 1.5
-- @author BRYAN
-- @about
--   Splits the selected audio item into vocals / drums / bass / other using
--   BS-RoFormer SW on Apple Silicon (MLX). Optional 6-stem (guitar + piano), drum
--   kit split, and tempo map from the drums stem. Requires .venv-stems
--   next to this script (or in another Scripts folder). Needs ReaImGui.
-- @changelog
--   New window in the Monolith look: stems, input (auto / full mix / drum loop), drum kit
--   split, progress per item, cancel, results with the imported stems. Every stem and kit card
--   toggles: a mix stem turned off is blended into Other, a kit piece turned off gets no track.
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

  local kit_added = false
  for _, key in ipairs(mix_names) do
    add_child(key, STEM_LABEL[key] or key)
    if key == "drums" then
      for _, dkey in ipairs(drum_names) do
        add_child(dkey, STEM_LABEL[dkey] or dkey)
      end
      kit_added = true
    end
  end
  if not kit_added then
    -- the drums stem was turned off (blended into Other): the kit pieces still get their tracks
    for _, dkey in ipairs(drum_names) do
      add_child(dkey, STEM_LABEL[dkey] or dkey)
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
    "--stems", opts.stems,
    "--start", string.format("%.6f", job.startoffs or 0),
    "--duration", string.format("%.6f", duration or 0),
    "--progress-file", shell_quote(paths.progress),
    "--done-flag", shell_quote(paths.done),
    "--manifest", shell_quote(paths.manifest),
  }
  if opts.split_drums then
    parts[#parts + 1] = "--split-drums"
  end
  if opts.skip_stems and opts.skip_stems ~= "" then
    parts[#parts + 1] = "--skip-stems"
    parts[#parts + 1] = shell_quote(opts.skip_stems)
  end
  if opts.input == "drums" then
    parts[#parts + 1] = "--drums-only"
  elseif opts.input == "auto" then
    parts[#parts + 1] = "--auto-skip-mix"
  end
  return table.concat(parts, " ") .. " > " .. shell_quote(paths.log) .. " 2>&1"
end

local function import_result(job, opts, info)
  if not job.item or not job.track then
    return false, "Original item is gone. Stems are on disk."
  end
  local mix_names = ordered_present(MIX_ORDER, info.stems)
  local drum_names = {}
  for _, k in ipairs(ordered_present(DRUM_ORDER, info.drum_stems)) do
    if not (opts.off_kit and opts.off_kit[k]) then
      drum_names[#drum_names + 1] = k
    end
  end
  if #mix_names < 1 and #drum_names < 1 then
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
  local dir = script_dir()
  local path
  for _, name in ipairs({
    "Tempo map from drum stem.lua",
    "CRS_Tempo map from drum stem.lua",
  }) do
    local candidate = join_path(dir, name)
    if file_exists(candidate) then
      path = candidate
      break
    end
  end
  if not path then
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

---------------------------------------------------------------------------------------------
-- The window: ReaImGui in the Monolith look (black, hairlines, square corners, letter-spaced
-- mono caps, one green accent). Setup -> splitting -> results, all in one window.

local TITLE = "Split to stems"

local P = {
  page = 0x000000FF,
  body = 0x0C0C0DFF,
  chip = 0x111112FF,
  acc = 0x49CC85FF,
  red = 0xE0604FFF,
  ink = 0xF2F2F2FF,
  ink2 = 0x9A9A9AFF,
  ink3 = 0x5F5F5FFF,
  line = 0xFFFFFF1F,
  line2 = 0xFFFFFF33,
  soft = 0xFFFFFF0F,
  tick = 0xFFFFFF59,
}

local function alpha(clr, a)
  return (clr & 0xFFFFFF00) | math.floor(math.max(0, math.min(1, a)) * 255 + 0.5)
end

local function rgb_u32(rgb, a)
  return (rgb[1] << 24) | (rgb[2] << 16) | (rgb[3] << 8) | math.floor((a or 1) * 255 + 0.5)
end

local function fmt_time(sec)
  sec = math.max(0, math.floor((sec or 0) + 0.5))
  local h = sec // 3600
  local m = (sec % 3600) // 60
  local s = sec % 60
  if h > 0 then
    return string.format("%d:%02d:%02d", h, m, s)
  end
  return string.format("%d:%02d", m, s)
end

local function read_all(path)
  local f = path and io.open(path, "r")
  if not f then return "" end
  local s = f:read("*a") or ""
  f:close()
  return s
end

local function open_folder(dir)
  if not dir or dir == "" then return end
  if r.CF_ShellExecute then
    r.CF_ShellExecute(dir)
  else
    os.execute("open " .. shell_quote(dir) .. " >/dev/null 2>&1 &")
  end
end

local function spawn_detached(cmd, pidfile)
  -- `exec` so the pid written is Python's own (Cancel kills it); cmd carries its own log redirect
  local inner = cmd
  if pidfile then
    inner = "echo $$ > " .. shell_quote(pidfile) .. "; exec " .. cmd
  end
  local line = "/bin/sh -c " .. shell_quote(inner) .. " </dev/null >/dev/null 2>&1 &"
  local rc = os.execute(line)
  return rc == true or rc == 0 or rc == nil
end

local function kill_job(job)
  local pid = tonumber((read_all(job.paths and job.paths.pid):match("%d+")))
  if pid and pid > 1 then
    os.execute("kill " .. pid .. " >/dev/null 2>&1")
  end
end

---------------------------------------------------------------------------------------------
-- options (ExtState keys are shared with the installer and older versions)

local INPUT_KEYS = { "auto", "mix", "drums" }

local function keyset(csv)
  local t = {}
  for k in tostring(csv or ""):gmatch("[^,%s]+") do t[k] = true end
  return t
end

local function keylist(set, order)
  local out = {}
  for _, k in ipairs(order) do
    if set and set[k] then out[#out + 1] = k end
  end
  return table.concat(out, ",")
end

local function load_opts()
  local input = ext_get("DRUM_LOOP", "auto")
  if input ~= "auto" and input ~= "mix" and input ~= "drums" then
    input = "auto"
  end
  return {
    stems = ext_get("STEMS", "4"):match("6") and "6" or "4",
    split_drums = yn(ext_get("SPLIT_DRUMS", "n"), false),
    mute_original = yn(ext_get("MUTE_ORIGINAL", "y"), true),
    tempo_map = yn(ext_get("TEMPO_MAP", "n"), false),
    input = input,
    off_stems = keyset(ext_get("OFF_STEMS", "")),
    off_kit = keyset(ext_get("OFF_KIT", "")),
  }
end

local function save_opts(o)
  ext_set("STEMS", o.stems)
  ext_set("SPLIT_DRUMS", o.split_drums and "y" or "n")
  ext_set("MUTE_ORIGINAL", o.mute_original and "y" or "n")
  ext_set("TEMPO_MAP", o.tempo_map and "y" or "n")
  ext_set("DRUM_LOOP", o.input)
  ext_set("OFF_STEMS", keylist(o.off_stems, MIX_ORDER))
  ext_set("OFF_KIT", keylist(o.off_kit, DRUM_ORDER))
end

---------------------------------------------------------------------------------------------
-- ReaImGui

if not r.ImGui_GetBuiltinPath then
  r.MB("This script needs ReaImGui.\n\nInstall it with Extensions > ReaPack > Browse packages > ReaImGui, then run the script again.", TITLE, 0)
  return
end
package.path = r.ImGui_GetBuiltinPath() .. "/?.lua;" .. package.path
local im = require("imgui")("0.9.3")

local ctx = im.CreateContext(TITLE)
local F = {}
do
  local rp = r.GetResourcePath()
  local dirs = {
    join_path(script_dir(), "Fonts"),
    rp .. "/FX Devices GITHUB/BryanChi-FX-Devices/BryanChi_FX_Devices/src/Fonts",
    rp .. "/Scripts/FX Devices/BryanChi_FX_Devices/src/Fonts",
    rp .. "/Scripts/BryanChi-FX-Devices/BryanChi_FX_Devices/src/Fonts",
    rp .. "/Scripts/BryanChi_FX_Devices/src/Fonts",
  }
  local function find(file)
    for _, d in ipairs(dirs) do
      local p = join_path(d, file)
      if file_exists(p) then return p end
    end
  end
  local reg = find("JetBrainsMono-Medium.ttf")
  local bold = find("JetBrainsMono-SemiBold.ttf") or reg
  for _, sz in ipairs({ 9, 10, 11, 12 }) do
    F[sz] = im.CreateFont(reg or "monospace", sz)
    im.Attach(ctx, F[sz])
  end
  for _, sz in ipairs({ 10, 11, 14 }) do
    F["b" .. sz] = im.CreateFont(bold or "monospace", sz, (not bold) and im.FontFlags_Bold or nil)
    im.Attach(ctx, F["b" .. sz])
  end
end

local function font(sz, bold)
  return (bold and F["b" .. sz]) or F[sz] or F[11]
end

local AdvCache = {}
local function text_w(fnt, s)
  local k = tostring(fnt) .. s
  local w = AdvCache[k]
  if not w then
    im.PushFont(ctx, fnt)
    w = im.CalcTextSize(ctx, s)
    im.PopFont(ctx)
    AdvCache[k] = w
  end
  return w
end

local function upper(s)
  return (tostring(s):gsub("[a-z]", string.upper))
end
local function chars(s)
  return tostring(s):gmatch("[\0-\x7F\xC2-\xF4][\x80-\xBF]*")
end

--- letter-spaced caps; returns the x after the text
local function caps(dl, x, y, clr, s, sz, bold, sp)
  local fnt = font(sz, bold)
  sp = sp or sz * 0.12
  for ch in chars(upper(s)) do
    im.DrawList_AddTextEx(dl, fnt, sz, x, y, clr, ch)
    x = x + text_w(fnt, ch) + sp
  end
  return x - sp
end

local function caps_w(s, sz, bold, sp)
  local fnt = font(sz, bold)
  sp = sp or sz * 0.12
  local w, n = 0, 0
  for ch in chars(upper(s)) do
    w = w + text_w(fnt, ch)
    n = n + 1
  end
  return w + math.max(0, n - 1) * sp
end

--- plain text (names as typed), clipped to max_w with a trailing '…'
local function txt(dl, x, y, clr, s, sz, max_w)
  local fnt = font(sz)
  s = tostring(s or "")
  if max_w and text_w(fnt, s) > max_w then
    local t = {}
    for ch in chars(s) do t[#t + 1] = ch end
    while #t > 1 and text_w(fnt, table.concat(t) .. "…") > max_w do t[#t] = nil end
    s = table.concat(t) .. "…"
  end
  im.DrawList_AddTextEx(dl, fnt, sz, x, y, clr, s)
end

local function corner_ticks(dl, L, T, R, B, len, clr)
  len, clr = len or 6, clr or P.tick
  im.DrawList_AddLine(dl, L, T, L + len, T, clr, 1) im.DrawList_AddLine(dl, L, T, L, T + len, clr, 1)
  im.DrawList_AddLine(dl, R - len, T, R, T, clr, 1) im.DrawList_AddLine(dl, R, T, R, T + len, clr, 1)
  im.DrawList_AddLine(dl, L, B, L + len, B, clr, 1) im.DrawList_AddLine(dl, L, B - len, L, B, clr, 1)
  im.DrawList_AddLine(dl, R - len, B, R, B, clr, 1) im.DrawList_AddLine(dl, R, B - len, R, B, clr, 1)
end

local function dashed_rect(dl, L, T, R, B, clr)
  local function seg(x1, y1, x2, y2)
    local len = math.max(math.abs(x2 - x1), math.abs(y2 - y1))
    local dx, dy = (x2 - x1) / math.max(len, 1), (y2 - y1) / math.max(len, 1)
    local p = 0
    while p < len do
      local q = math.min(len, p + 4)
      im.DrawList_AddLine(dl, x1 + dx * p, y1 + dy * p, x1 + dx * q, y1 + dy * q, clr, 1)
      p = q + 3
    end
  end
  seg(L, T, R, T) seg(R, T, R, B) seg(R, B, L, B) seg(L, B, L, T)
end

---------------------------------------------------------------------------------------------
-- widgets (each takes the full row width W and advances the cursor)

local W = 432

local function gap(h)
  im.Dummy(ctx, W, h)
end

--- a section label with a hairline to the right edge; right = optional text at the far right
local function section(label, right, right_clr)
  local dl = im.GetWindowDrawList(ctx)
  local x, y = im.GetCursorScreenPos(ctx)
  local x2 = caps(dl, x, y, P.ink3, label, 9, true, 1.6)
  local rx = x + W
  if right and right ~= "" then
    local rw = caps_w(right, 9, false, 1.2)
    caps(dl, rx - rw, y, right_clr or P.ink2, right, 9, false, 1.2)
    rx = rx - rw - 10
  end
  if rx > x2 + 10 then
    im.DrawList_AddLine(dl, x2 + 10, y + 6, rx, y + 6, P.line, 1)
  end
  im.Dummy(ctx, W, 13)
  gap(4)
end

--- a row of equal cells; returns the picked key (or the current one)
local function segmented(id, opts, cur, h, disabled)
  local dl = im.GetWindowDrawList(ctx)
  local x, y = im.GetCursorScreenPos(ctx)
  h = h or 30
  local n = #opts
  local cw = W / n
  local picked = cur
  for i, o in ipairs(opts) do
    local L = math.floor(x + (i - 1) * cw + 0.5)
    local R = math.floor(x + i * cw + 0.5)
    im.SetCursorScreenPos(ctx, L, y)
    if im.InvisibleButton(ctx, id .. i, R - L, h) and not disabled then
      picked = o.key
    end
    local hov = im.IsItemHovered(ctx) and not disabled
    local on = o.key == cur
    if on then
      im.DrawList_AddRectFilled(dl, L, y, R, y + h, alpha(P.acc, disabled and 0.05 or 0.12))
    elseif hov then
      im.DrawList_AddRectFilled(dl, L, y, R, y + h, P.soft)
    end
    local clr = disabled and P.ink3 or (on and P.ink or (hov and P.ink or P.ink2))
    local tw = caps_w(o.label, 10, on, 1.4)
    local ty = y + (o.sub and (h / 2 - 11) or (h / 2 - 6))
    caps(dl, math.floor((L + R - tw) / 2), ty, clr, o.label, 10, on, 1.4)
    if o.sub then
      local sw = caps_w(o.sub, 9, false, 1)
      caps(dl, math.floor((L + R - sw) / 2), ty + 14, (on and not disabled) and P.ink2 or P.ink3, o.sub, 9, false, 1)
    end
    if i > 1 then
      im.DrawList_AddLine(dl, L, y, L, y + h, P.line2, 1)
    end
  end
  im.DrawList_AddRect(dl, x, y, x + W, y + h, P.line2, 0, 0, 1)
  -- the picked cell's edge in the accent, drawn last so it sits on top
  for i, o in ipairs(opts) do
    if o.key == cur then
      local L = math.floor(x + (i - 1) * cw + 0.5)
      local R = math.floor(x + i * cw + 0.5)
      im.DrawList_AddRect(dl, L, y, R + 1, y + h, alpha(P.acc, disabled and 0.3 or 0.85), 0, 0, 1)
    end
  end
  im.SetCursorScreenPos(ctx, x, y)
  im.Dummy(ctx, W, h)
  return picked
end

--- a square check and a caps label; w = its width; returns the new value
local function toggle(id, label, on, w)
  local dl = im.GetWindowDrawList(ctx)
  local x, y = im.GetCursorScreenPos(ctx)
  local h = 22
  local clicked = im.InvisibleButton(ctx, id, w, h)
  local hov = im.IsItemHovered(ctx)
  if hov then
    im.DrawList_AddRectFilled(dl, x, y, x + w, y + h, P.soft)
  end
  local bx, by, bs = x + 6, y + 5, 12
  im.DrawList_AddRect(dl, bx, by, bx + bs, by + bs, on and P.acc or (hov and P.ink2 or P.line2), 0, 0, 1)
  if on then
    im.DrawList_AddRectFilled(dl, bx + 3, by + 3, bx + bs - 3, by + bs - 3, P.acc)
  end
  caps(dl, bx + bs + 9, y + 6, on and P.ink or (hov and P.ink or P.ink2), label, 10, false, 1.3)
  if clicked then
    return not on
  end
  return on
end

--- colour-striped stem names, n per row; lit = set of keys drawn bright (nil = all)
--- id + off (set of keys the user turned off): the lit chips are clickable; returns the clicked key
local function chips(keys, lit, per_row, x0, w0, id, off)
  local dl = im.GetWindowDrawList(ctx)
  local x, y = im.GetCursorScreenPos(ctx)
  x = x0 or x
  local w = w0 or W
  per_row = per_row or #keys
  local cw = (w - (per_row - 1) * 4) / per_row
  local h = 20
  local rows = 0
  local clicked = nil
  for i, key in ipairs(keys) do
    local col = (i - 1) % per_row
    local row = (i - 1) // per_row
    rows = row + 1
    local L = math.floor(x + col * (cw + 4))
    local T = y + row * (h + 4)
    local avail = (lit == nil) or lit[key]
    local is_off = avail and off and off[key]
    local on = avail and not is_off
    local hov = false
    if id and avail then
      im.SetCursorScreenPos(ctx, L, T)
      if im.InvisibleButton(ctx, id .. key, cw, h) then
        clicked = key
      end
      hov = im.IsItemHovered(ctx)
    end
    local rgb = STEM_RGB[key] or { 160, 160, 160 }
    im.DrawList_AddRectFilled(dl, L, T, L + cw, T + h, on and (hov and 0x19191AFF or P.chip) or (hov and 0x111112FF or 0x070707FF))
    im.DrawList_AddRect(dl, L, T, L + cw, T + h, hov and P.line2 or (on and P.line or 0xFFFFFF0C), 0, 0, 1)
    im.DrawList_AddRectFilled(dl, L, T, L + 3, T + h, rgb_u32(rgb, on and 1 or 0.22))
    local x2 = caps(dl, L + 10, T + 5, on and P.ink or (is_off and hov and P.ink2 or P.ink3), STEM_LABEL[key] or key, 9, false, 1.1)
    if is_off then
      im.DrawList_AddLine(dl, L + 9, T + h / 2, x2 + 1, T + h / 2, P.ink3, 1)
    end
  end
  im.SetCursorScreenPos(ctx, x0 or x, y)
  im.Dummy(ctx, w, rows * h + (rows - 1) * 4)
  return clicked
end

--- a square button; kind = "primary" | "plain" | "danger"; returns true on click
local function button(id, label, w, h, kind, disabled)
  local dl = im.GetWindowDrawList(ctx)
  local x, y = im.GetCursorScreenPos(ctx)
  local clicked = im.InvisibleButton(ctx, id, w, h) and not disabled
  local hov = im.IsItemHovered(ctx) and not disabled
  local act = im.IsItemActive(ctx) and not disabled
  local base = kind == "primary" and P.acc or (kind == "danger" and P.red or P.ink)
  local fill, edge, clr
  if disabled then
    fill, edge, clr = 0, P.line, P.ink3
  elseif act then
    fill, edge, clr = base, base, P.page
  elseif kind == "plain" then
    fill, edge, clr = hov and P.soft or 0, hov and P.ink2 or P.line2, hov and P.ink or P.ink2
  else
    fill, edge, clr = alpha(base, hov and 0.24 or 0.12), alpha(base, hov and 1 or 0.8), P.ink
  end
  if fill ~= 0 then
    im.DrawList_AddRectFilled(dl, x, y, x + w, y + h, fill)
  end
  im.DrawList_AddRect(dl, x, y, x + w, y + h, edge, 0, 0, 1)
  local big = h >= 34
  local sz = big and 11 or 10
  local sp = big and 2.4 or 1.4
  local tw = caps_w(label, sz, big, sp)
  caps(dl, math.floor(x + (w - tw) / 2), math.floor(y + (h - sz) / 2) - 1, clr, label, sz, big, sp)
  if kind == "primary" and not disabled then
    corner_ticks(dl, x - 3, y - 3, x + w + 3, y + h + 3, 5, alpha(P.acc, hov and 0.9 or 0.45))
  end
  return clicked
end

--- a thin progress bar with the percent at the right
local function progress_bar(x, y, w, pct, clr)
  local dl = im.GetWindowDrawList(ctx)
  im.DrawList_AddRectFilled(dl, x, y, x + w, y + 2, P.line)
  local fw = math.floor(w * math.max(0, math.min(100, pct)) / 100 + 0.5)
  if fw > 0 then
    im.DrawList_AddRectFilled(dl, x, y, x + fw, y + 2, clr or P.acc)
  end
end

---------------------------------------------------------------------------------------------
-- state

local UI = {
  phase = "setup",     -- setup | running | done
  opts = load_opts(),
  open = true,
  shown = true,        -- false once the window is closed while a split runs in the background
  items = nil,
  items_err = nil,
  items_t = -1,
  env_err = nil,
  py = nil,
  sidecar = nil,
}

-- The venv next to this script, else one in Scripts/Stem Split, Scripts/Bryan's Scripts or any other
-- folder in Scripts (one venv serves every copy of these scripts).
local function find_python(dir)
  local function venv(d) return join_path(d, ".venv-stems/bin/python") end
  if file_exists(venv(dir)) then return venv(dir) end
  local scripts = r.GetResourcePath() .. "/Scripts"
  local dirs = { join_path(scripts, "Stem Split"), join_path(scripts, "Bryan's Scripts") }
  local k = 0
  while true do
    local sub = r.EnumerateSubdirectories(scripts, k)
    if not sub then break end
    dirs[#dirs + 1] = join_path(scripts, sub)
    k = k + 1
  end
  for _, d in ipairs(dirs) do
    if file_exists(venv(d)) then return venv(d) end
  end
  return venv(dir)
end

local function check_env()
  local dir = script_dir()
  UI.py = find_python(dir)
  UI.sidecar = join_path(dir, "StemSplit.py")
  if r.GetOS():match("Win") then
    UI.env_err = "This script uses BS-RoFormer SW on MLX and is Mac / Apple Silicon only."
  elseif not file_exists(UI.py) then
    UI.env_err = "Python environment not found:\n" .. UI.py .. "\n\n"
      .. "Create it with:\n"
      .. "  python3.13 -m venv .venv-stems\n"
      .. "  .venv-stems/bin/pip install mlx-audio-separator 'torch>=2.6' mdxnet-infer soundfile"
  elseif not file_exists(UI.sidecar) then
    UI.env_err = "StemSplit.py not found next to this script:\n" .. UI.sidecar
  else
    UI.env_err = nil
  end
end

local function refresh_items(force)
  local now = r.time_precise()
  if not force and now - UI.items_t < 0.3 then
    return
  end
  UI.items_t = now
  local jobs, err = collect_jobs()
  UI.items = jobs
  UI.items_err = (not jobs) and err or nil
end

local function opts_for_run()
  local o = UI.opts
  return {
    stems = o.stems,
    split_drums = o.split_drums,
    mute_original = o.mute_original,
    tempo_map = o.tempo_map,
    input = o.input,
    skip_stems = (o.input ~= "drums") and keylist(o.off_stems, MIX_ORDER) or "",
    off_kit = o.off_kit,
  }
end

local function start_job(j)
  local job = j.batch[j.cur]
  pcall(os.remove, job.paths.done)
  pcall(os.remove, job.paths.progress)
  pcall(os.remove, job.paths.pid)
  job.status = "running"
  job.pct = 0
  job.msg = "Starting…"
  job.t0 = r.time_precise()
  if not spawn_detached(job.cmd, job.paths.pid) then
    job.status = "failed"
    job.err = "Could not start StemSplit.py."
    return false
  end
  return true
end

local function start_split()
  refresh_items(true)
  if not UI.items or UI.env_err then
    return
  end
  save_opts(UI.opts)
  local opts = opts_for_run()
  local jobs = UI.items
  for _, job in ipairs(jobs) do
    job.paths = job_paths(job)
    job.paths.pid = join_path(job.paths.out_dir, "_pid.txt")
    r.RecursiveCreateDirectory(job.paths.out_dir, 0)
    job.cmd = python_cmd(UI.py, UI.sidecar, job, opts, job.paths)
    job.status = "queued"
    job.pct = 0
    job.msg = ""
    job.notes = {}
  end
  JOB = {
    batch = jobs,
    cur = 1,
    opts = opts,
    t0 = r.time_precise(),
    max_wait = 20 * 60,
  }
  UI.phase = "running"
  start_job(JOB) -- a job that can't start is marked failed; tick() moves on to the next
end

local function finish_run()
  JOB.t1 = r.time_precise()
  UI.phase = "done"
  if not UI.shown then
    -- closed while running: bring the results back
    UI.shown = true
    UI.open = true
  end
end

local function next_job()
  local j = JOB
  while j.cur < #j.batch do
    j.cur = j.cur + 1
    if start_job(j) then
      return
    end
  end
  finish_run()
end

local function fail_job(job, msg)
  job.status = "failed"
  job.err = msg
  local log = read_all(job.paths.log)
  job.log = log ~= "" and log:sub(-2500) or nil
end

--- the Python side finished with `code`: import the stems (runs one frame after "importing")
local function import_job(job, code)
  local j = JOB
  if code ~= 0 then
    fail_job(job, "Stem split failed (exit " .. tostring(code) .. ").")
    return
  end
  local info, err = parse_manifest(job.paths.manifest)
  if not info or not info.ok then
    fail_job(job, err or (info and info.warning ~= "" and info.warning) or "Stem split produced no output.")
    return
  end
  local ok, msg, drums_item = import_result(job, j.opts, info)
  if not ok then
    fail_job(job, msg)
    return
  end
  job.stems = ordered_present(MIX_ORDER, info.stems)
  job.drum_stems = ordered_present(DRUM_ORDER, info.drum_stems)
  job.skipped_mix = info.engine == "drums-only"
  if info.warning and info.warning ~= "" then
    job.notes[#job.notes + 1] = info.warning
  end
  if j.opts.tempo_map then
    if drums_item then
      local tok, tmsg = tempo_map_drums_item(drums_item)
      if tok then
        job.notes[#job.notes + 1] = tostring(tmsg)
      else
        job.notes[#job.notes + 1] = "Tempo map failed (" .. tostring(tmsg or "unknown") .. ")"
      end
    else
      job.notes[#job.notes + 1] = "Tempo map skipped (no drums stem)"
    end
  end
  job.status = "done"
  job.pct = 100
end

local function tick()
  local j = JOB
  if not j or UI.phase ~= "running" then
    return
  end
  local job = j.batch[j.cur]
  if job.status == "importing" then
    import_job(job, job.code)
    job.t1 = r.time_precise()
    next_job()
    return
  end
  if job.status ~= "running" then
    next_job()
    return
  end
  local code = read_done(job.paths.done)
  if code == nil then
    local pct, msg = read_progress(job.paths.progress)
    job.pct = pct
    if msg ~= "" then job.msg = msg end
    if r.time_precise() - job.t0 > j.max_wait then
      kill_job(job)
      fail_job(job, "Timed out after " .. fmt_time(j.max_wait) .. ".")
      job.t1 = r.time_precise()
      next_job()
    end
    return
  end
  job.code = code
  job.status = "importing"
  job.pct = 100
  job.msg = (code == 0) and ((j.opts.tempo_map and "Importing stems, tempo mapping drums…") or "Importing stems…") or "Failed"
end

local function cancel_run()
  local j = JOB
  if not j then return end
  for i = j.cur, #j.batch do
    local job = j.batch[i]
    if job.status == "running" then
      kill_job(job)
    end
    if job.status == "running" or job.status == "queued" or job.status == "importing" then
      job.status = "cancelled"
    end
  end
  finish_run()
end

---------------------------------------------------------------------------------------------
-- drawing

local function header()
  local dl = im.GetWindowDrawList(ctx)
  local x, y = im.GetCursorScreenPos(ctx)
  local x2 = caps(dl, x, y + 1, P.ink, "Stem Split", 14, true, 3.2)
  im.DrawList_AddRectFilled(dl, x2 + 8, y + 6, x2 + 13, y + 11, P.acc)
  caps(dl, x2 + 20, y + 4, P.ink3, "BS-RoFormer SW", 9, false, 1.3)
  -- close
  local cx = x + W - 18
  im.SetCursorScreenPos(ctx, cx, y - 2)
  if im.InvisibleButton(ctx, "##close", 18, 18) then
    UI.open = false
  end
  local hov = im.IsItemHovered(ctx)
  local c = hov and P.ink or P.ink3
  im.DrawList_AddLine(dl, cx + 5, y + 3, cx + 13, y + 11, c, 1.2)
  im.DrawList_AddLine(dl, cx + 13, y + 3, cx + 5, y + 11, c, 1.2)
  im.SetCursorScreenPos(ctx, x, y)
  im.Dummy(ctx, W, 18)
  gap(6)
  local x3, y3 = im.GetCursorScreenPos(ctx)
  im.DrawList_AddLine(dl, x3, y3, x3 + W, y3, P.line2, 1)
  gap(12)
end

local function item_row(job, right, right_clr, stripe)
  local dl = im.GetWindowDrawList(ctx)
  local x, y = im.GetCursorScreenPos(ctx)
  local h = 22
  im.DrawList_AddRectFilled(dl, x, y, x + W, y + h, P.body)
  im.DrawList_AddRectFilled(dl, x, y, x + 2, y + h, stripe or P.ink3)
  local rw = right and caps_w(right, 9, false, 1.1) or 0
  txt(dl, x + 12, y + 5, P.ink, job.name, 11, W - 30 - rw)
  if right then
    caps(dl, x + W - 10 - rw, y + 6, right_clr or P.ink2, right, 9, false, 1.1)
  end
  im.Dummy(ctx, W, h)
end

local function draw_env_error()
  section("Setup")
  im.PushFont(ctx, font(11))
  im.PushStyleColor(ctx, im.Col_Text, P.ink2)
  im.PushTextWrapPos(ctx, im.GetCursorPosX(ctx) + W)
  im.TextWrapped(ctx, UI.env_err)
  im.PopTextWrapPos(ctx)
  im.PopStyleColor(ctx)
  im.PopFont(ctx)
  gap(14)
  if button("##close2", "Close", W, 30, "plain") then
    UI.open = false
  end
end

local function draw_setup()
  refresh_items(false)
  local o = UI.opts
  local items = UI.items

  -- SOURCE
  section("Source", items and (#items .. (#items == 1 and " item" or " items")) or nil)
  if items then
    local shown = math.min(#items, 4)
    for i = 1, shown do
      local job = items[i]
      item_row(job, fmt_time(job.length))
      if i < shown then gap(2) end
    end
    if #items > shown then
      gap(4)
      local dl = im.GetWindowDrawList(ctx)
      local x, y = im.GetCursorScreenPos(ctx)
      caps(dl, x + 12, y, P.ink3, "+" .. (#items - shown) .. " more", 9, false, 1.1)
      im.Dummy(ctx, W, 11)
    end
  else
    local dl = im.GetWindowDrawList(ctx)
    local x, y = im.GetCursorScreenPos(ctx)
    local h = 44
    dashed_rect(dl, x, y, x + W, y + h, P.line2)
    local msg = UI.items_err and UI.items_err:match("^No usable") and "No audio file on disk. Glue the item first."
      or "Select audio items"
    local tw = caps_w(msg, 10, false, 1.4)
    caps(dl, math.floor(x + (W - tw) / 2), y + h / 2 - 6, P.ink2, msg, 10, false, 1.4)
    im.Dummy(ctx, W, h)
  end
  gap(16)

  -- INPUT
  section("Input")
  o.input = segmented("##input", {
    { key = "auto", label = "Auto" },
    { key = "mix", label = "Full mix" },
    { key = "drums", label = "Drum loop" },
  }, o.input, 28)
  gap(16)

  -- STEMS
  local drums_only = o.input == "drums"
  section("Stems")
  o.stems = segmented("##stems", {
    { key = "4", label = "4 stems" },
    { key = "6", label = "6 stems" },
  }, o.stems, 30, drums_only)
  gap(6)
  local lit = {}
  if drums_only then
    lit.drums = true
    o.off_stems.drums = nil
  else
    for _, k in ipairs({ "vocals", "drums", "bass", "other" }) do lit[k] = true end
    if o.stems == "6" then
      lit.guitar = true
      lit.piano = true
    end
  end
  local ck = chips(MIX_ORDER, lit, 6, nil, nil, "##stem_", (not drums_only) and o.off_stems or nil)
  if ck and not drums_only then
    o.off_stems[ck] = (not o.off_stems[ck]) or nil
  end
  gap(16)

  -- DRUMS
  section("Drums")
  o.split_drums = toggle("##kit", "Split drums into kit", o.split_drums, W)
  gap(4)
  local kit_lit = {}
  if o.split_drums then
    for _, k in ipairs(DRUM_ORDER) do kit_lit[k] = true end
  end
  ck = chips(DRUM_ORDER, kit_lit, 6, nil, nil, "##kitp_", o.off_kit)
  if ck then
    o.off_kit[ck] = (not o.off_kit[ck]) or nil
  end
  gap(16)

  -- AFTER
  section("After")
  local x, y = im.GetCursorScreenPos(ctx)
  o.mute_original = toggle("##mute", "Mute original", o.mute_original, W / 2 - 2)
  im.SetCursorScreenPos(ctx, x + W / 2 + 2, y)
  o.tempo_map = toggle("##tempo", "Tempo map from drums", o.tempo_map, W / 2 - 2)
  gap(20)

  -- GO
  local n = items and #items or 0
  local label = n > 1 and ("Split " .. n .. " items") or "Split"
  local go = button("##go", label, W, 38, "primary", n == 0)
  local enter = n > 0 and not im.IsAnyItemActive(ctx)
    and (im.IsKeyPressed(ctx, im.Key_Enter) or im.IsKeyPressed(ctx, im.Key_KeypadEnter))
  if go or enter then
    start_split()
  end
end

local function status_word(job)
  if job.status == "done" then
    return "Done", P.acc
  elseif job.status == "failed" then
    return "Failed", P.red
  elseif job.status == "cancelled" then
    return "Cancelled", P.ink3
  elseif job.status == "running" or job.status == "importing" then
    return job.pct .. "%", P.ink
  end
  return "Queued", P.ink3
end

local function draw_job_detail(job)
  local dl = im.GetWindowDrawList(ctx)
  if job.status == "running" or job.status == "importing" then
    gap(6)
    local x, y = im.GetCursorScreenPos(ctx)
    progress_bar(x, y, W, job.pct)
    im.Dummy(ctx, W, 2)
    gap(6)
    x, y = im.GetCursorScreenPos(ctx)
    txt(dl, x + 12, y, P.ink2, job.msg or "", 10, W - 12)
    im.Dummy(ctx, W, 12)
  elseif job.status == "done" then
    gap(4)
    local keys = {}
    for _, k in ipairs(job.stems or {}) do keys[#keys + 1] = k end
    for _, k in ipairs(job.drum_stems or {}) do
      if not (JOB.opts.off_kit and JOB.opts.off_kit[k]) then keys[#keys + 1] = k end
    end
    if #keys > 0 then
      chips(keys, nil, math.min(6, math.max(4, #keys)))
    end
    for _, note in ipairs(job.notes or {}) do
      gap(4)
      local nx, ny = im.GetCursorScreenPos(ctx)
      txt(dl, nx + 12, ny, P.ink2, note, 10, W - 12)
      im.Dummy(ctx, W, 12)
    end
    gap(4)
    if button("##folder" .. tostring(job.item), "Open folder", 112, 20, "plain") then
      open_folder(job.paths.out_dir)
    end
  elseif job.status == "failed" then
    gap(4)
    local x, y = im.GetCursorScreenPos(ctx)
    txt(dl, x + 12, y, P.red, job.err or "Failed", 10, W - 12)
    im.Dummy(ctx, W, 12)
    if job.log then
      gap(4)
      im.PushFont(ctx, font(9))
      im.PushStyleColor(ctx, im.Col_FrameBg, P.body)
      im.PushStyleColor(ctx, im.Col_Text, P.ink2)
      im.InputTextMultiline(ctx, "##log" .. tostring(job.item), job.log, W, 96, im.InputTextFlags_ReadOnly)
      im.PopStyleColor(ctx, 2)
      im.PopFont(ctx)
    end
    gap(4)
    if button("##logbtn" .. tostring(job.item), "Open folder", 112, 20, "plain") then
      open_folder(job.paths.out_dir)
    end
  end
end

local function draw_run()
  local j = JOB
  local done = UI.phase == "done"
  local n_ok, n_fail = 0, 0
  for _, job in ipairs(j.batch) do
    if job.status == "done" then n_ok = n_ok + 1 end
    if job.status == "failed" then n_fail = n_fail + 1 end
  end
  local total = fmt_time((j.t1 or r.time_precise()) - j.t0)
  local label
  if done then
    label = n_fail > 0 and (n_ok .. " / " .. #j.batch .. " split") or "Done"
  else
    label = "Splitting " .. j.cur .. " / " .. #j.batch
  end
  section(label, total, done and (n_fail > 0 and P.red or P.acc) or P.ink2)

  for i, job in ipairs(j.batch) do
    local word, clr = status_word(job)
    local stripe = (job.status == "done" and P.acc) or (job.status == "failed" and P.red)
      or ((job.status == "running" or job.status == "importing") and P.ink) or P.ink3
    item_row(job, word, clr, stripe)
    draw_job_detail(job)
    if i < #j.batch then gap(8) end
  end
  gap(18)

  if done then
    local bw = (W - 8) / 2
    if button("##again", "Split again", bw, 32, "plain") then
      JOB = nil
      UI.phase = "setup"
      refresh_items(true)
    end
    im.SameLine(ctx, 0, 8)
    if button("##done", "Close", bw, 32, "primary") then
      UI.open = false
    end
  else
    if button("##cancel", "Cancel", W, 32, "danger") then
      cancel_run()
    end
  end
end

local function push_style()
  im.PushStyleVar(ctx, im.StyleVar_WindowRounding, 0)
  im.PushStyleVar(ctx, im.StyleVar_FrameRounding, 0)
  im.PushStyleVar(ctx, im.StyleVar_WindowBorderSize, 1)
  im.PushStyleVar(ctx, im.StyleVar_WindowPadding, 20, 18)
  im.PushStyleVar(ctx, im.StyleVar_ItemSpacing, 0, 0)
  im.PushStyleVar(ctx, im.StyleVar_ScrollbarRounding, 0)
  im.PushStyleColor(ctx, im.Col_WindowBg, P.page)
  im.PushStyleColor(ctx, im.Col_Border, P.line2)
  im.PushStyleColor(ctx, im.Col_Text, P.ink)
  im.PushStyleColor(ctx, im.Col_TextSelectedBg, alpha(P.acc, 0.35))
  im.PushStyleColor(ctx, im.Col_ScrollbarBg, P.page)
  im.PushStyleColor(ctx, im.Col_ScrollbarGrab, P.line2)
  return 6, 6
end

local function loop()
  if UI.phase == "running" then
    tick()
  end

  if UI.shown then
    local nv, nc = push_style()
    local flags = im.WindowFlags_NoTitleBar | im.WindowFlags_AlwaysAutoResize | im.WindowFlags_NoCollapse
      | im.WindowFlags_NoScrollbar | im.WindowFlags_NoDocking
    im.SetNextWindowPos(ctx, 200, 160, im.Cond_FirstUseEver)
    local visible, open = im.Begin(ctx, TITLE, true, flags)
    if visible then
      local dl = im.GetWindowDrawList(ctx)
      local wx, wy = im.GetWindowPos(ctx)
      local ww, wh = im.GetWindowSize(ctx)
      corner_ticks(dl, wx + 7, wy + 7, wx + ww - 7, wy + wh - 7, 7, P.tick)
      header()
      if UI.env_err then
        draw_env_error()
      elseif UI.phase == "setup" then
        draw_setup()
      else
        draw_run()
      end
      if im.IsWindowFocused(ctx, im.FocusedFlags_RootAndChildWindows) and im.IsKeyPressed(ctx, im.Key_Escape)
        and UI.phase ~= "running" then
        UI.open = false
      end
      im.End(ctx)
    end
    im.PopStyleColor(ctx, nc)
    im.PopStyleVar(ctx, nv)
    if open == false then
      UI.open = false
    end
  end

  if not UI.open then
    if UI.phase == "running" then
      -- keep splitting in the background; the window comes back with the results
      UI.shown = false
      UI.open = true
    else
      return
    end
  end
  r.defer(loop)
end

check_env()
refresh_items(true)
-- frame timing for the Performance Tracker window (no cost while it is closed)
pcall(function() loop = dofile(join_path(script_dir(), "CRS_Perf.lua")).Wrap("Stem Split", loop) end)
r.defer(loop)
