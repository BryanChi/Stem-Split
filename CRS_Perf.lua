-- @noindex
-- CRS_Perf: per-frame timing for a script's defer loop, read by "CRS_Performance Tracker.lua".
-- Shared module: keep this file identical in every script that ships it (like CRS_Feedback.lua).
--
--   local Perf = dofile(dir .. 'CRS_Perf.lua')
--   loop = Perf.Wrap('My Script', loop)   -- before the first r.defer(loop); loop must defer itself by this name
--
-- While no tracker window is open the wrapper costs a counter per frame and one ExtState read
-- every CHECK_EVERY frames. With the tracker open it times each frame (time_precise around the
-- loop body, and the gap between frame starts) and publishes the samples through a
-- non-persistent ExtState a few times per second. gmem is avoided on purpose: gmem_attach is
-- per script, and the scripts attach their own namespaces.

local r = reaper

local SECTION = 'CRS_PerfTracker'
local CHECK_EVERY = 15        -- frames between "is the tracker open?" checks (~0.5 s at 30 fps)
local PUBLISH_EVERY = 0.2     -- seconds between publishes while tracked
local HEARTBEAT_TIMEOUT = 2.0 -- tracker counts as closed when its heartbeat is older than this

local Perf = { VERSION = 1 }
local wrapped = setmetatable({}, { __mode = 'k' })

-- One-time: add the tracker window to the action list when its file is known and not yet added.
local function register_tracker_action()
  if not r.AddRemoveReaScript then return end
  local path = r.GetExtState(SECTION, 'tracker_path')
  if path == '' then
    path = r.GetResourcePath() .. "/Scripts/Bryan's Scripts/Performance Tracker/CRS_Performance Tracker.lua"
  end
  if r.GetExtState(SECTION, 'registered') == path then return end
  local f = io.open(path, 'r')
  if not f then return end
  f:close()
  if r.AddRemoveReaScript(true, 0, path, true) ~= 0 then
    r.SetExtState(SECTION, 'registered', path, true)
  end
end
pcall(register_tracker_action)

local function tracker_open()
  local hb = tonumber(r.GetExtState(SECTION, 'hb'))
  return hb ~= nil and (r.time_precise() - hb) < HEARTBEAT_TIMEOUT
end

local function add_to_list(name)
  local list = r.GetExtState(SECTION, 'list')
  for n in list:gmatch('[^\n]+') do
    if n == name then return end
  end
  r.SetExtState(SECTION, 'list', list == '' and name or (list .. '\n' .. name), false)
end

-- Returns fn wrapped with the timing. name is what the tracker shows; two scripts must not share one.
function Perf.Wrap(name, fn)
  if type(fn) ~= 'function' or wrapped[fn] then return fn end
  name = tostring(name):gsub('[\n|;]', ' ')
  local key = 'd:' .. name
  local time = r.time_precise
  local frame, next_check = 0, 0
  local active, registered_exit = false, false
  local prev_start, last_publish, seq = nil, 0, 0
  local buf, nbuf = {}, 0

  local function publish(now)
    seq = seq + 1
    r.SetExtState(SECTION, key, string.format('%d|%.3f|%d|%s', seq, now,
      math.floor(collectgarbage('count')), table.concat(buf, ';', 1, nbuf)), false)
    nbuf = 0
    last_publish = now
  end

  local function w(...)
    frame = frame + 1
    if frame >= next_check then
      next_check = frame + CHECK_EVERY
      local was = active
      active = tracker_open()
      if active and not was then
        add_to_list(name)
        prev_start, nbuf, last_publish = nil, 0, time()
        if not registered_exit and r.atexit then -- the tracker shows the script as stopped
          registered_exit = true
          r.atexit(function() r.DeleteExtState(SECTION, key, false) end)
        end
      end
    end
    if not active then return fn(...) end

    local t0 = time()
    local gap = prev_start and (t0 - prev_start) or 0
    prev_start = t0
    fn(...)
    local t1 = time()
    nbuf = nbuf + 1
    buf[nbuf] = string.format('%.3f,%.2f', (t1 - t0) * 1000, gap * 1000)
    if t1 - last_publish >= PUBLISH_EVERY then publish(t1) end
  end
  wrapped[w] = true
  return w
end

return Perf
