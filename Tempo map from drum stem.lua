-- @description Tempo map from drum stem
-- @version 1.1
-- @author BRYAN
-- @about
--   Detects hits on the selected drum-stem audio item, places a stretch
--   marker on every hit, then tempo-maps the project to that audio.
--   Steady grooves (tempo within 2 BPM) get a single tempo marker;
--   larger rubato or tempo changes get a full map. No Python required.
-- @provides [main] .

local r = reaper

local SR = 12000
local HOP_SEC = 0.008
local WIN_SEC = 0.016
local MIN_HIT_GAP = 0.028
local MERGE_S = 0.030
local BPM_MIN = 40.0
local BPM_MAX = 240.0
local STABLE_MAD_RATIO = 0.018
local TEMPO_CHANGE_MIN_BPM = 2.0
local SECTION_WIN_SEC = 8.0

local function log(msg)
  r.ShowConsoleMsg(tostring(msg) .. "\n")
end

local function clamp(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

local function copy_sorted(values)
  local out = {}
  for i = 1, #values do
    out[i] = values[i]
  end
  table.sort(out)
  return out
end

local function median(values)
  if #values == 0 then
    return 0.0
  end
  local ordered = copy_sorted(values)
  local mid = math.floor(#ordered / 2) + 1
  if #ordered % 2 == 1 then
    return ordered[mid]
  end
  return 0.5 * (ordered[mid - 1] + ordered[mid])
end

local function percentile(values, p)
  if #values == 0 then
    return 0.0
  end
  local ordered = copy_sorted(values)
  if p <= 0 then
    return ordered[1]
  end
  if p >= 100 then
    return ordered[#ordered]
  end
  local idx = (p / 100.0) * (#ordered - 1) + 1
  local lo = math.floor(idx)
  local hi = math.ceil(idx)
  if lo == hi then
    return ordered[lo]
  end
  local t = idx - lo
  return ordered[lo] * (1.0 - t) + ordered[hi] * t
end

local function read_accessor_chunk(acc, start_t, n)
  local buf = r.new_array(n)
  local ok = r.GetAudioAccessorSamples(acc, SR, 1, start_t, n, buf)
  if ok ~= 1 and r.AudioAccessorGetSamples then
    ok = r.AudioAccessorGetSamples(acc, SR, 1, start_t, n, buf)
  end
  if ok ~= 1 and ok ~= true then
    return nil
  end
  return buf.table()
end

local function scan_envelopes(take, item_len)
  local acc = r.CreateTakeAudioAccessor(take)
  if not acc then
    return nil, "Could not read audio from the selected take."
  end

  local hop = math.max(16, math.floor(SR * HOP_SEC + 0.5))
  local win = math.max(hop * 2, math.floor(SR * WIN_SEC + 0.5))
  local hop_s = hop / SR
  local lp_a = 1.0 - math.exp(-2.0 * math.pi * 160.0 / SR)

  local q_x, q_d, q_l = {}, {}, {}
  local qn, qhead = 0, 1
  local sum_x, sum_d, sum_l = 0.0, 0.0, 0.0
  local prev, lp, sample_i = 0.0, 0.0, 0
  local full_env, hf_env, lf_env = {}, {}, {}

  local function push(x)
    local d = x - prev
    prev = x
    lp = lp + lp_a * (x - lp)
    local x2, d2, l2 = x * x, d * d, lp * lp
    if qn < win then
      qn = qn + 1
      q_x[qn], q_d[qn], q_l[qn] = x2, d2, l2
      sum_x, sum_d, sum_l = sum_x + x2, sum_d + d2, sum_l + l2
    else
      sum_x = sum_x - q_x[qhead] + x2
      sum_d = sum_d - q_d[qhead] + d2
      sum_l = sum_l - q_l[qhead] + l2
      q_x[qhead], q_d[qhead], q_l[qhead] = x2, d2, l2
      qhead = (qhead % win) + 1
    end
    sample_i = sample_i + 1
    if qn == win and ((sample_i - win) % hop) == 0 then
      local inv = 1.0 / win
      full_env[#full_env + 1] = math.sqrt(math.max(0.0, sum_x) * inv)
      hf_env[#hf_env + 1] = math.sqrt(math.max(0.0, sum_d) * inv)
      lf_env[#lf_env + 1] = math.sqrt(math.max(0.0, sum_l) * inv)
    end
  end

  for _ = 1, win do
    push(0.0)
  end

  local t = 0.0
  local chunk = SR
  while t < item_len - 1e-6 do
    local remain = item_len - t
    local n = math.min(chunk, math.max(1, math.floor(remain * SR + 0.5)))
    local tab = read_accessor_chunk(acc, t, n)
    if tab then
      for i = 1, n do
        push(tab[i] or 0.0)
      end
    end
    t = t + n / SR
    if n <= 0 then
      break
    end
  end

  r.DestroyAudioAccessor(acc)
  -- Silence pad shifts every frame by win samples; subtract that below.
  return {
    full = full_env,
    hf = hf_env,
    lf = lf_env,
    hop_s = hop_s,
    pad_s = win / SR,
  }
end

local function pos_diff(env)
  local out = {}
  local prev = env[1] or 0.0
  for i = 1, #env do
    local d = env[i] - prev
    out[i] = d > 0.0 and d or 0.0
    prev = env[i]
  end
  return out
end

local function normalize(env)
  local p95 = percentile(env, 95.0)
  if p95 <= 1e-12 then
    return env
  end
  local inv = 1.0 / p95
  local out = {}
  for i = 1, #env do
    out[i] = env[i] * inv
  end
  return out
end

local function parabolic_time(nov, idx, hop_s)
  if idx <= 1 or idx >= #nov then
    return (idx - 1) * hop_s
  end
  local a, b, c = nov[idx - 1], nov[idx], nov[idx + 1]
  local denom = a - 2.0 * b + c
  if math.abs(denom) <= 1e-12 then
    return (idx - 1) * hop_s
  end
  local shift = clamp(0.5 * (a - c) / denom, -0.5, 0.5)
  return (idx - 1 + shift) * hop_s
end

local function merge_hits(onsets)
  if #onsets == 0 then
    return onsets
  end
  table.sort(onsets, function(a, b) return a.time < b.time end)
  local groups = {{onsets[1]}}
  for i = 2, #onsets do
    local cur = groups[#groups]
    if onsets[i].time - cur[1].time <= MERGE_S then
      cur[#cur + 1] = onsets[i]
    else
      groups[#groups + 1] = {onsets[i]}
    end
  end
  local merged = {}
  for g = 1, #groups do
    local group = groups[g]
    local best = group[1]
    local tmin = group[1].time
    local weight = 0.0
    local kickish, snareish = false, false
    for i = 1, #group do
      local o = group[i]
      weight = weight + o.weight
      if o.time < tmin then tmin = o.time end
      if o.weight > best.weight then best = o end
      if o.kickish then kickish = true end
      if o.snareish then snareish = true end
    end
    merged[#merged + 1] = {
      time = tmin,
      weight = weight,
      kind = kickish and "kick" or (snareish and "snare" or best.kind),
      kickish = kickish,
      snareish = snareish,
    }
  end
  return merged
end

local function pick_onsets(full, hf, lf, hop_s, pad_s)
  local n = math.min(#full, #hf, #lf)
  if n < 8 then
    return {}
  end
  local flux = normalize(pos_diff(full))
  local hf_flux = normalize(pos_diff(hf))
  local lf_flux = normalize(pos_diff(lf))
  local novelty = {}
  for i = 1, n do
    novelty[i] = 1.15 * flux[i] + 0.95 * hf_flux[i] + 1.25 * lf_flux[i]
  end

  local look = math.max(2, math.floor(0.018 / hop_s + 0.5))
  local min_dist = math.max(2, math.floor(MIN_HIT_GAP / hop_s + 0.5))
  local local_win = math.max(8, math.floor(1.2 / hop_s + 0.5))
  local peak = 0.0
  for i = 1, n do
    if novelty[i] > peak then peak = novelty[i] end
  end
  local floor = math.max(peak * 0.045, percentile(novelty, 55.0) * 1.1)

  local picked = {}
  local last = -min_dist
  for i = 2, n - 1 do
    local value = novelty[i]
    if value >= floor and value >= novelty[i - 1] and value > novelty[i + 1] then
      local look_l = math.min(look, i - 1)
      local look_r = math.min(look, n - i)
      local neigh_max = value
      for k = i - look_l, i + look_r do
        if novelty[k] > neigh_max then neigh_max = novelty[k] end
      end
      if value >= neigh_max then
        local lo = math.max(1, i - local_win)
        local hi = math.min(n, i + local_win)
        local local_vals = {}
        for k = lo, hi do
          local_vals[#local_vals + 1] = novelty[k]
        end
        local local_bar = percentile(local_vals, 70.0)
        if value >= math.max(floor, local_bar * 1.25) then
          if i - last < min_dist then
            if #picked > 0 and value > picked[#picked].raw then
              picked[#picked] = {idx = i, raw = value, lf = lf_flux[i], hf = hf_flux[i]}
            end
          else
            picked[#picked + 1] = {idx = i, raw = value, lf = lf_flux[i], hf = hf_flux[i]}
            last = i
          end
        end
      end
    end
  end

  local onsets = {}
  for i = 1, #picked do
    local p = picked[i]
    local time = parabolic_time(novelty, p.idx, hop_s) - pad_s
    if time >= -0.01 then
      if time < 0 then time = 0 end
      local kind, kickish, snareish = "hit", false, false
      local weight = 0.45 + clamp(p.raw, 0.0, 4.0)
      if p.lf > 0.32 and p.lf >= p.hf * 1.35 then
        kind, kickish, weight = "kick", true, weight + 1.35
      elseif p.hf > 0.28 and p.hf >= p.lf * 1.45 then
        kind, weight = "hat", weight + 0.15
      elseif p.lf > 0.2 and p.hf > 0.22 then
        kind, snareish, weight = "snare", true, weight + 0.55
      end
      onsets[#onsets + 1] = {
        time = time,
        weight = weight,
        kind = kind,
        kickish = kickish,
        snareish = snareish,
      }
    end
  end
  return merge_hits(onsets)
end

local function musical_prior(bpm)
  if bpm < BPM_MIN or bpm > BPM_MAX then
    return 0.0
  end
  local z = (bpm - 112.0) / 48.0
  return math.exp(-0.5 * z * z) + 0.18
end

local function prefer_drum_tempo(period, bpm)
  bpm = clamp(bpm or 120.0, BPM_MIN, BPM_MAX)
  period = period or (60.0 / bpm)
  if bpm < 78.0 and bpm * 2.0 <= BPM_MAX then
    return period * 0.5, bpm * 2.0
  end
  if bpm > 168.0 and bpm * 0.5 >= BPM_MIN then
    return period * 2.0, bpm * 0.5
  end
  return period, bpm
end

local function estimate_period(onsets)
  if #onsets < 2 then
    return 0.5, 120.0
  end
  local hop = 0.01
  local t0 = onsets[1].time
  local t1 = onsets[#onsets].time
  local n = math.max(4, math.floor((t1 - t0) / hop) + 4)
  local env = {}
  for i = 1, n do
    env[i] = 0.0
  end
  for i = 1, #onsets do
    local o = onsets[i]
    local idx = math.floor((o.time - t0) / hop + 0.5) + 1
    if idx >= 1 and idx <= n then
      env[idx] = env[idx] + o.weight
      if idx > 1 then env[idx - 1] = env[idx - 1] + o.weight * 0.45 end
      if idx < n then env[idx + 1] = env[idx + 1] + o.weight * 0.45 end
    end
  end

  local min_lag = math.max(1, math.floor((60.0 / BPM_MAX) / hop + 0.5))
  local max_lag = math.min(n - 2, math.floor((60.0 / BPM_MIN) / hop + 0.5))
  if max_lag <= min_lag then
    return 0.5, 120.0
  end

  local best_lag, best_score, best_bpm = min_lag, -1.0, 120.0
  local by_lag = {}
  for lag = min_lag, max_lag do
    local tot, norm = 0.0, 0.0
    for i = 1, n - lag do
      tot = tot + env[i] * env[i + lag]
      norm = norm + env[i] * env[i]
    end
    if norm > 1e-9 then
      local bpm = 60.0 / (lag * hop)
      local score = (tot / norm) * musical_prior(bpm)
      by_lag[lag] = {score, bpm}
      if score > best_score then
        best_score, best_lag, best_bpm = score, lag, bpm
      end
    end
  end

  local double_lag = best_lag * 2
  local d = by_lag[double_lag]
  if d then
    if best_bpm > 160.0 and d[1] > best_score * 0.55 then
      best_lag, best_score, best_bpm = double_lag, d[1], d[2]
    elseif d[2] >= 70.0 and d[2] <= 160.0 and d[1] > best_score * 0.85 then
      best_lag, best_score, best_bpm = double_lag, d[1], d[2]
    end
  end
  if best_lag % 2 == 0 then
    local h = by_lag[math.floor(best_lag / 2)]
    if h and best_bpm < 70.0 and h[1] > best_score * 0.7 then
      best_lag, best_bpm = math.floor(best_lag / 2), h[2]
    end
  end

  local period = best_lag * hop
  return prefer_drum_tempo(period, 60.0 / period)
end

local function nearest_onset(onsets, t, radius)
  local best, best_d = nil, radius
  for i = 1, #onsets do
    local d = math.abs(onsets[i].time - t)
    if d <= best_d then
      best_d = d
      best = onsets[i]
    elseif onsets[i].time - t > radius then
      break
    end
  end
  return best
end

local function track_beats(onsets, period)
  if #onsets == 0 or period <= 1e-4 then
    return {}
  end
  local t1 = onsets[#onsets].time
  local starts = {onsets[1].time}
  for i = 1, math.min(8, #onsets) do
    if onsets[i].kickish then
      starts[#starts + 1] = onsets[i].time
      break
    end
  end

  local function run_from(start)
    local beats = {start}
    local t = start + period
    while t <= t1 + period * 0.12 do
      local hit = nearest_onset(onsets, t, period * 0.12)
      local snapped = hit and hit.time or t
      if snapped <= beats[#beats] + period * 0.4 then
        snapped = t
      end
      beats[#beats + 1] = snapped
      t = snapped + period
      if #beats > 8000 then
        break
      end
    end
    return beats
  end

  local best_beats, best_score = nil, -1.0
  for s = 1, #starts do
    local beats = run_from(starts[s])
    local score = 0.0
    for i = 1, #beats do
      local hit = nearest_onset(onsets, beats[i], period * 0.12)
      if hit then
        score = score + hit.weight + (hit.kickish and 1.2 or 0.0)
      end
    end
    score = score / math.max(1, #beats)
    if score > best_score then
      best_score = score
      best_beats = beats
    end
  end
  return best_beats or {onsets[1].time}
end

local function interval_bpms(beats)
  local pairs = {}
  for i = 1, #beats - 1 do
    local dt = beats[i + 1] - beats[i]
    if dt > 1e-4 then
      pairs[#pairs + 1] = {time = beats[i], bpm = 60.0 / dt}
    end
  end
  return pairs
end

local function repair_octave(pairs)
  if #pairs < 3 then
    return pairs
  end
  local bpms = {}
  for i = 1, #pairs do
    bpms[i] = pairs[i].bpm
  end
  local med = median(bpms)
  local out = {}
  for i = 1, #pairs do
    local bpm = pairs[i].bpm
    if bpm < med * 0.62 then
      bpm = bpm * 2.0
    elseif bpm > med * 1.55 then
      bpm = bpm * 0.5
    end
    out[i] = {time = pairs[i].time, bpm = clamp(bpm, BPM_MIN, BPM_MAX)}
  end
  return out
end

local function median_filter(pairs, width)
  width = width or 3
  if #pairs < width then
    return pairs
  end
  local half = math.floor(width / 2)
  local bpms = {}
  for i = 1, #pairs do
    bpms[i] = pairs[i].bpm
  end
  local out = {}
  for i = 1, #pairs do
    local lo = math.max(1, i - half)
    local hi = math.min(#bpms, i + half)
    local slice = {}
    for k = lo, hi do
      slice[#slice + 1] = bpms[k]
    end
    out[i] = {time = pairs[i].time, bpm = median(slice)}
  end
  return out
end

local function tempo_is_stable(beats)
  local pairs = repair_octave(interval_bpms(beats))
  if #pairs < 4 then
    local bpms = {}
    for i = 1, #pairs do
      bpms[i] = pairs[i].bpm
    end
    return true, median(bpms) > 0 and median(bpms) or 120.0
  end
  local bpms = {}
  for i = 1, #pairs do
    bpms[i] = pairs[i].bpm
  end
  local med = median(bpms)
  if med <= 1e-6 then
    return true, 120.0
  end
  local abs_dev = {}
  local max_dev = 0.0
  for i = 1, #bpms do
    local d = math.abs(bpms[i] - med)
    abs_dev[i] = d
    if d > max_dev then max_dev = d end
  end
  local mad_ratio = median(abs_dev) / med

  local section_jump = false
  if beats[#beats] - beats[1] >= SECTION_WIN_SEC * 2.2 then
    local t = beats[1]
    local window_bpms = {}
    while t < beats[#beats] do
      local local_bpms = {}
      for i = 1, #pairs do
        if pairs[i].time >= t and pairs[i].time < t + SECTION_WIN_SEC then
          local_bpms[#local_bpms + 1] = pairs[i].bpm
        end
      end
      if #local_bpms >= 4 then
        window_bpms[#window_bpms + 1] = median(local_bpms)
      end
      t = t + SECTION_WIN_SEC * 0.5
    end
    if #window_bpms >= 2 then
      for i = 2, #window_bpms do
        if math.abs(window_bpms[i] - window_bpms[1]) > TEMPO_CHANGE_MIN_BPM
          or math.abs(window_bpms[i] - med) > TEMPO_CHANGE_MIN_BPM then
          section_jump = true
          break
        end
      end
    end
  end

  local stable = (not section_jump)
    and mad_ratio <= STABLE_MAD_RATIO
    and max_dev <= TEMPO_CHANGE_MIN_BPM
  return stable, med
end

local function build_markers(beats, stable, median_bpm)
  local bpm0 = clamp(median_bpm or 120.0, BPM_MIN, BPM_MAX)
  if #beats == 0 then
    return {{time = 0.0, bpm = bpm0, num = 4, den = 4}}
  end
  if stable or #beats < 4 then
    return {{time = beats[1], bpm = bpm0, num = 4, den = 4}}
  end
  local pairs = median_filter(repair_octave(interval_bpms(beats)), 3)
  if #pairs == 0 then
    return {{time = beats[1], bpm = bpm0, num = 4, den = 4}}
  end
  local markers = {}
  local last_bpm = nil
  for i = 1, #pairs do
    local bpm = clamp(pairs[i].bpm, BPM_MIN, BPM_MAX)
    if not last_bpm or math.abs(bpm - last_bpm) > TEMPO_CHANGE_MIN_BPM then
      markers[#markers + 1] = {time = pairs[i].time, bpm = bpm, num = 4, den = 4}
      last_bpm = bpm
    end
  end
  if #markers == 0 then
    markers[1] = {time = beats[1], bpm = bpm0, num = 4, den = 4}
  end
  return markers
end

local function stretch_markers_actually_stretch(take)
  local count = r.GetTakeNumStretchMarkers(take) or 0
  if count <= 0 then
    return false, 0
  end
  local prev_pos, prev_src = nil, nil
  for idx = 0, count - 1 do
    local ok, pos, srcpos = r.GetTakeStretchMarker(take, idx)
    if ok then
      if prev_pos and prev_src and math.abs(pos - prev_pos) > 1e-9 then
        if math.abs(((srcpos - prev_src) / (pos - prev_pos)) - 1.0) > 0.001 then
          return true, count
        end
      end
      prev_pos, prev_src = pos, srcpos
    end
  end
  return false, count
end

local function clear_unity_stretch_markers(take)
  local stretching, count = stretch_markers_actually_stretch(take)
  if stretching then
    return false, count
  end
  if count > 0 then
    r.DeleteTakeStretchMarkers(take, 0, count)
  end
  return true, count
end

local function find_marker_near_time(target_time, tolerance)
  tolerance = tolerance or 0.01
  local best_idx, best_diff = -1, nil
  for i = 0, r.CountTempoTimeSigMarkers(0) - 1 do
    local ok, time = r.GetTempoTimeSigMarker(0, i)
    if ok then
      local diff = math.abs(time - target_time)
      if not best_diff or diff < best_diff then
        best_diff, best_idx = diff, i
      end
    end
  end
  if best_idx >= 0 and best_diff and best_diff <= tolerance then
    return best_idx
  end
  return -1
end

local function add_tempo_marker(timepos, bpm, num, den)
  local ok = r.AddTempoTimeSigMarker(0, timepos, bpm, num, den, false)
  if not ok then
    return false
  end
  local idx = find_marker_near_time(timepos, 0.05)
  if idx >= 0 and r.SetTempoTimeSigMarker then
    r.SetTempoTimeSigMarker(0, idx, timepos, -1, -1, bpm, num, den, false)
  end
  return true
end

local function clear_tempo_markers_in_range(start_time, end_time)
  if end_time < start_time then
    start_time, end_time = end_time, start_time
  end
  local removed = 0
  for i = r.CountTempoTimeSigMarkers(0) - 1, 0, -1 do
    local ok, time = r.GetTempoTimeSigMarker(0, i)
    if ok and time >= start_time and time <= end_time then
      r.DeleteTempoTimeSigMarker(0, i)
      removed = removed + 1
    end
  end
  return removed
end

local function selected_audio_item()
  local count = r.CountSelectedMediaItems(0)
  if count == 0 then
    return nil, "Select one audio item (the drum stem)."
  end
  if count > 1 then
    return nil, "Select only one audio item."
  end
  local item = r.GetSelectedMediaItem(0, 0)
  local take = item and r.GetActiveTake(item)
  if not take then
    return nil, "Selected item has no active take."
  end
  if r.TakeIsMIDI(take) then
    return nil, "Selected item is MIDI. Select the drum-stem audio item."
  end
  return item, take
end

-- opts.quiet: no message boxes (caller handles errors).
-- Returns ok, summary_or_error.
local function apply_to_item(item, opts)
  opts = opts or {}
  local take = item and r.GetActiveTake(item)
  if not take then
    return false, "Drums item has no active take."
  end
  if r.TakeIsMIDI(take) then
    return false, "Drums item is MIDI."
  end

  local item_pos = r.GetMediaItemInfo_Value(item, "D_POSITION") or 0.0
  local item_len = r.GetMediaItemInfo_Value(item, "D_LENGTH") or 0.0
  if item_len <= 0.05 then
    return false, "The drums item is too short to tempo-map."
  end

  log(string.format("Scanning %.2fs of audio…", item_len))
  local envs, err = scan_envelopes(take, item_len)
  if not envs then
    return false, err or "Could not analyze the take."
  end

  local onsets = pick_onsets(envs.full, envs.hf, envs.lf, envs.hop_s, envs.pad_s)
  if #onsets < 2 then
    return false, "No drum hits detected on the drums item."
  end

  local period, pulse_bpm = estimate_period(onsets)
  local beats = track_beats(onsets, period)
  local stable, median_bpm = tempo_is_stable(beats)
  if not median_bpm or median_bpm <= 1e-6 then
    median_bpm = pulse_bpm
  end
  local markers = build_markers(beats, stable, median_bpm)
  local mode = stable and "single" or "full"

  log(string.format("Hits: %d", #onsets))
  log(string.format("Beats: %d", #beats))
  log(string.format("Mode: %s", mode))
  log(string.format("Median tempo: %.3f BPM", median_bpm))
  log(string.format("Tempo markers: %d", #markers))
  for i = 1, #markers do
    if i <= 12 or i == #markers then
      log(string.format("  %d: time=%.4f  bpm=%.3f", i, markers[i].time, markers[i].bpm))
    elseif i == 13 then
      log("  ...")
    end
  end

  r.Undo_BeginBlock()
  r.PreventUIRefresh(1)

  r.SetMediaItemInfo_Value(item, "C_BEATATTACHMODE", 0)

  local cleared, existing = clear_unity_stretch_markers(take)
  if cleared and existing > 0 then
    log(string.format("Cleared %d existing stretch marker(s).", existing))
  elseif not cleared then
    log(string.format(
      "Left %d existing stretch marker(s) alone because they already time-stretch the take.",
      existing or 0
    ))
  end

  local added_hits = 0
  for i = 1, #onsets do
    local take_pos = onsets[i].time
    if take_pos >= -0.001 and take_pos <= item_len + 0.02 then
      if take_pos < 0 then take_pos = 0 end
      local idx = r.SetTakeStretchMarker(take, -1, take_pos)
      if idx and idx >= 0 then
        added_hits = added_hits + 1
      end
    end
  end
  log(string.format("Added %d stretch marker(s).", added_hits))

  local clear_start = math.max(0, item_pos - 0.05)
  local clear_end = item_pos + item_len + 0.05
  if #markers > 0 then
    clear_start = math.max(0, math.min(clear_start, item_pos + markers[1].time - 0.05))
    clear_end = math.max(clear_end, item_pos + markers[#markers].time + 0.05)
  end
  local removed = clear_tempo_markers_in_range(clear_start, clear_end)
  log(string.format("Cleared %d existing tempo marker(s) in %.3f–%.3f.", removed, clear_start, clear_end))

  local added = 0
  for i = 1, #markers do
    local m = markers[i]
    if add_tempo_marker(item_pos + m.time, m.bpm, m.num, m.den) then
      added = added + 1
    else
      log(string.format("Failed to add tempo marker at %.4f", item_pos + m.time))
    end
  end

  r.UpdateItemInProject(item)
  r.PreventUIRefresh(-1)
  r.UpdateTimeline()
  if r.UpdateArrange then
    r.UpdateArrange()
  end

  local summary
  if mode == "single" then
    summary = string.format(
      "Drum stem tempo map: 1 tempo at %.2f BPM, %d stretch markers",
      median_bpm, added_hits
    )
  else
    summary = string.format(
      "Drum stem tempo map: %d tempo markers, %d stretch markers",
      added, added_hits
    )
  end

  if added > 0 then
    r.Undo_EndBlock(summary, 0)
    log(summary)
    return true, summary
  end
  r.Undo_EndBlock("Tempo map from drum stem (no tempo markers added)", -1)
  return false, "Hits were found, but no tempo markers could be added."
end

local function main()
  r.ClearConsole()
  log("Tempo map from drum stem")
  log("========================")

  local item, take_or_err = selected_audio_item()
  if not item then
    r.ShowMessageBox(take_or_err, "Tempo map from drum stem", 0)
    return
  end

  local ok, err = apply_to_item(item, { quiet = false })
  if not ok then
    r.ShowMessageBox(err or "Tempo map failed.", "Tempo map from drum stem", 0)
  end
end

Bryan_ApplyDrumStemTempoMap = apply_to_item

if not Bryan_DrumStemTempoMap_AsLib then
  main()
end
