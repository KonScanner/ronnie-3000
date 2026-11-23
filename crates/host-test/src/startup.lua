-- Runs the commands that ronnie-host-test writes to cmd.txt, and writes one result line for each
-- command to out.txt. REAPER runs this script at start, because its name is __startup.lua.
local dir = os.getenv("RONNIE_HOST_TEST_DIR")
local command_file = dir .. "/cmd.txt"
local output_file = dir .. "/out.txt"
local mark = "ronnie-host-test mark"

-- Writes one result line. A line break in the result becomes a space, so that each command has
-- one line. When the file does not open, the line is lost, but the command loop keeps running.
local function log(text)
  local file = io.open(output_file, "a")
  if file == nil then return end
  file:write((text:gsub("[\r\n]", " ")) .. "\n")
  file:close()
end

local function track(index)
  while reaper.CountTracks(0) <= index do
    reaper.InsertTrackAtIndex(reaper.CountTracks(0), true)
  end
  return reaper.GetTrack(0, index)
end

local commands = {}

function commands.list()
  local found = {}
  local index = 0
  while true do
    local ok, name = reaper.EnumInstalledFX(index)
    if not ok then break end
    if name:find("Ronnie 3000", 1, true) then found[#found + 1] = name end
    index = index + 1
  end
  return table.concat(found, "|")
end

function commands.add(words, line)
  local name = line:match("^add %d+ (.+)$")
  return tostring(reaper.TrackFX_AddByName(track(tonumber(words[2])), name, false, -1))
end

function commands.show(words)
  reaper.TrackFX_Show(track(tonumber(words[2])), tonumber(words[3]), 3)
  return "shown"
end

function commands.hide(words)
  reaper.TrackFX_Show(track(tonumber(words[2])), tonumber(words[3]), 2)
  return "hidden"
end

-- Each parameter as name~stepped~toggle~step, joined by |.
function commands.params(words)
  local t, fx = track(tonumber(words[2])), tonumber(words[3])
  local found = {}
  for index = 0, reaper.TrackFX_GetNumParams(t, fx) - 1 do
    local _, name = reaper.TrackFX_GetParamName(t, fx, index)
    local stepped, step, _, _, toggle = reaper.TrackFX_GetParameterStepSizes(t, fx, index)
    found[#found + 1] = string.format("%s~%s~%s~%s", name, tostring(stepped), tostring(toggle), tostring(step))
  end
  return table.concat(found, "|")
end

function commands.setparam(words)
  local t = track(tonumber(words[2]))
  reaper.TrackFX_SetParamNormalized(t, tonumber(words[3]), tonumber(words[4]), tonumber(words[5]))
  return "set"
end

function commands.getparam(words)
  local t, fx, index = track(tonumber(words[2])), tonumber(words[3]), tonumber(words[4])
  local value = reaper.TrackFX_GetParamNormalized(t, fx, index)
  local _, text = reaper.TrackFX_GetFormattedParamValue(t, fx, index)
  return string.format("%.6f~%s", value, text)
end

function commands.media(words, line)
  local path = line:match("^media %d+ (.+)$")
  local t = track(tonumber(words[2]))
  reaper.SetOnlyTrackSelected(t)
  reaper.SetEditCurPos(0, false, false)
  reaper.InsertMedia(path, 0)
  return "media"
end

-- render DIR NAME. The folder can hold spaces, and the name cannot.
function commands.render(words, line)
  local dir, name = line:match("^render (.+) (%S+)$")
  reaper.GetSetProjectInfo(0, "RENDER_SETTINGS", 0, true)
  reaper.GetSetProjectInfo(0, "RENDER_BOUNDSFLAG", 1, true)
  reaper.GetSetProjectInfo(0, "RENDER_SRATE", 48000, true)
  reaper.GetSetProjectInfo(0, "RENDER_CHANNELS", 2, true)
  reaper.GetSetProjectInfo_String(0, "RENDER_FILE", dir, true)
  reaper.GetSetProjectInfo_String(0, "RENDER_PATTERN", name, true)
  reaper.Main_OnCommand(42230, 0)
  return "rendered"
end

function commands.audio()
  local _, mode = reaper.GetAudioDeviceInfo("MODE")
  local _, rate = reaper.GetAudioDeviceInfo("SRATE")
  local _, size = reaper.GetAudioDeviceInfo("BSIZE")
  return string.format("%s~%s~%s", mode, rate, size)
end

function commands.online()
  reaper.Main_OnCommand(40101, 0)
  return "online"
end

function commands.save(words, line)
  reaper.Main_SaveProjectEx(0, line:match("^save (.+)$"), 0)
  return "saved"
end

function commands.open(words, line)
  reaper.Main_openProject("noprompt:" .. line:match("^open (.+)$"))
  return "opened"
end

-- An undo point with a real change, so that REAPER keeps it.
function commands.mark()
  reaper.Undo_BeginBlock()
  local t = track(0)
  local _, name = reaper.GetSetMediaTrackInfo_String(t, "P_NAME", "", false)
  reaper.GetSetMediaTrackInfo_String(t, "P_NAME", name == "mark" and "mark." or "mark", true)
  reaper.Undo_EndBlock(mark, -1)
  return "marked"
end

-- The undo steps after the mark, newest first, joined by |. This undoes them.
function commands.undos()
  local found = {}
  for _ = 1, 500 do
    local name = reaper.Undo_CanUndo2(0)
    if name == nil or name == mark then break end
    found[#found + 1] = name
    reaper.Undo_DoUndo2(0)
  end
  return #found .. "~" .. table.concat(found, "|")
end

local function run(line)
  local words = {}
  for word in line:gmatch("%S+") do words[#words + 1] = word end
  local command = commands[words[1]]
  if command == nil then return "unknown command" end
  return command(words, line)
end

local function loop()
  local file = io.open(command_file, "r")
  if file then
    local text = file:read("*a")
    file:close()
    os.remove(command_file)
    for line in text:gmatch("[^\n]+") do
      local ok, result = pcall(run, line)
      log((ok and "ok " or "error ") .. tostring(result))
    end
  end
  reaper.defer(loop)
end

log("ok ready")
loop()
