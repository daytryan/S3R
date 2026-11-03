-- Skate 1 online: the natives the disc's challenge Lua calls (skate1_online.rs).
-- Queries answer from S, which the binding keeps current (players, local slot, clock, tick, the
-- skaters' trick state); every other call is recorded in S.calls for the binding to apply.
S = {
  n = 1, me = 0, names = {}, gone = {}, time = 0, tick = 0, seed = 1, per_gate = 0, race = false, kind = 0,
  -- The match token's draw (Session::new: token % players), the random first turn every console agrees on (RULES).
  first = 0,
  -- A race's route (skate1_online.rs): per way point (0-based) {cum, mid}, the lap's and the race's length.
  route = {}, lap = 0, total = 0,
  timers = {}, next_id = 1, hud_timer = nil, calls = {},
  -- Per player (0-based id): what the observer last saw. air, trick (doing one), types[t] (doing type t),
  -- grab (the last air had a grab), switch (the last air started switch), regular (stance), finished (race).
  skaters = {},
  -- S.K.A.T.E.: the trick the leader set / the follower matched (the binding compares the observed tricks).
  pattern = {}, matched = {},
}

local function arg(v)
  local t = type(v)
  if t == "table" then return "<table>" end
  if t == "function" or t == "userdata" or t == "thread" then return "<" .. t .. ">" end
  return v
end
local function record(name, ...)
  local c = {name, n = select("#", ...)}
  for i = 1, c.n do c[i + 1] = arg((select(i, ...))) end
  S.calls[#S.calls + 1] = c
end
function __drain() local c = S.calls S.calls = {} return c end

-- The disc's enums: HUD's come from the HUD movies' own tables (__enums), Trick.eTrickType from
-- the trick data; any other e* table answers "Table.eEnum.Key", which the scripts only compare.
local function enum(prefix)
  return setmetatable({}, {__index = function(e, k) local v = prefix .. "." .. k rawset(e, k, v) return v end})
end
for _, T in ipairs({"AI", "Challenge", "ChallengeAsset", "ChallengeBoundary", "ChallengeManager", "ChallengeMusic",
    "ChallengeReplay", "ChallengeSFX", "ChallengeSpeech", "ChallengeTimer", "Contest", "Debug", "Entity", "HUD",
    "NIS", "Online", "Pause", "Random", "Skate", "Time", "Transition", "Trick", "Trigger"}) do
  _G[T] = setmetatable({}, {__index = function(t, k)
    local v
    if type(k) == "string" and k:match("^e%u") then
      v = enum(T .. "." .. k)
    else
      local name = T .. "." .. tostring(k)
      v = function(...) record(name, ...) end
    end
    rawset(t, k, v)
    return v
  end})
end
function __enums(tables)
  for T, enums in pairs(tables) do
    for k, values in pairs(enums) do rawset(_G[T], k, values) end
  end
end

local function skater(id) return S.skaters[id] or {} end
local function quiet(T, k, f) rawset(_G[T], k, f) end
-- Recorded and answered.
local function answer(T, k, f)
  local name = T .. "." .. k
  rawset(_G[T], k, function(...) record(name, ...) return f(...) end)
end

quiet("Online", "IsOnline", function() return true end)
quiet("Online", "GetLocalPlayerID", function() return S.me end)
quiet("Online", "IsPlayerDisconnected", function(i) return S.gone[i] == true end)
quiet("Online", "GetNumOnlineSkaters", function()
  local n = 0
  for i = 0, S.n - 1 do if not S.gone[i] then n = n + 1 end end
  return n
end)
quiet("ChallengeManager", "GetSkaterCount", function() return S.n end)
quiet("ChallengeManager", "GetSkaterName", function(i) return S.names[i] or "" end)
-- The challenge manager's per-player records (0x160 bytes each from ChallengeManager+0x1daf0): the score
-- (SetPlayerScore 0x823db860, +0x54); finished / DNF / DQF (0x827bcff0 / 0x827bcf30 / 0x827bcf90: +0x5e / +0x5c /
-- +0x5d), each also storing the race clock then (+0x14); the penalty (NotifyNumMissedWayPoint 0x823dd1a8: missed x
-- PenaltyPerMissedGate, +0x20); laps (NotifyLapFinished, +0x38) and the next way point (NotifyPlayerNextWayPoint,
-- +0x4c); left (+0x5b, 0x823f286c: a player leaving while the challenge runs). NotifyPlayerHitWayPoint (0x827bcd60)
-- reads the player and gate only: the script's time argument isn't stored. The race clock runs from ChallengeStart
-- (0x823dc468 zeroes it) and stops at ChallengeStop (0x823dc538). Every player's script reports every player, so each
-- game's records are whole.
S.rec, S.rank, S.running, S.start, S.stop = {}, {}, false, 0, nil
local function rec(i)
  local r = S.rec[i]
  if r == nil then r = {score = 0, at = 0, penalty = 0, laps = 0, next = 0, est = 0, split = 0, dist = 0} S.rec[i] = r end
  return r
end
-- The place order (+0x9ac), slot order until the first sort (SetupChallengeStatus 0x823daba8).
local function ranks()
  if #S.rank ~= S.n then
    S.rank = {}
    for i = 0, S.n - 1 do S.rank[i + 1] = i end
  end
  return S.rank
end
-- Not retail (user 10-12): an event relayed from another player's game is clocked at that game's race clock (S.heard,
-- set by the binding while it delivers the event), so every game times a finish as the finisher's own did. Retail
-- clocked it on arrival: a finish closer than the wire could place differently per game (online audit 10-12).
local function clock() return S.heard or (S.stop or S.time) - S.start end
-- The race clock this game stamps on its own relayed events (nil while no race runs).
function __race_clock() if S.running and S.race then return S.time - S.start end end
local function out(r) return r.dnf or r.dqf or r.left end
local function ended(i, flag) local r = rec(i) r[flag] = true r.at = clock() end
-- a before b. Races (0x823da688): in before out; two in, the lower time (a finished one's clock + penalty, a racing
-- one's live clock + its estimate); two out, the higher clock + penalty. The rest (0x823da600): one who left after, then
-- the higher score (b having left puts a first even when a left too).
local function before(a, b)
  local x, y = rec(a), rec(b)
  if not S.race then
    if y.left then return true end
    if x.left then return false end
    return x.score > y.score
  end
  if out(x) then return out(y) and x.at + x.penalty > y.at + y.penalty end
  if out(y) then return true end
  local function t(r) if r.finished then return r.at + r.penalty end return clock() + r.est end
  return t(x) < t(y)
end
-- 0x823e5d48 / 0x823e5df0: an insertion sort of the current order, so equals keep their places.
local function sort()
  local o = ranks()
  for k = 2, #o do
    local v, j = o[k], k - 1
    while j >= 1 and before(v, o[j]) do o[j + 1] = o[j] j = j - 1 end
    o[j + 1] = v
  end
end
answer("ChallengeManager", "SetPlayerScore", function(i, v) rec(i).score = v or 0 end)
answer("ChallengeManager", "NotifyNumMissedWayPoint", function(i, m) rec(i).penalty = S.per_gate * (m or 0) end)
answer("ChallengeManager", "NotifyLapFinished", function(i, n) rec(i).laps = n or 0 end)
answer("ChallengeManager", "NotifyPlayerNextWayPoint", function(i, w) rec(i).next = w or 0 end)
answer("ChallengeManager", "NotifyPlayerFinished", function(i) ended(i, "finished") end)
answer("ChallengeManager", "NotifyPlayerDNF", function(i) ended(i, "dnf") end)
answer("ChallengeManager", "NotifyPlayerDQF", function(i) ended(i, "dqf") end)
answer("ChallengeManager", "ChallengeStart", function() S.running, S.start, S.stop = true, S.time, nil sort() end)
-- Online, everyone still racing is out (0x823ddc58: DNF, no clock stored).
answer("ChallengeManager", "ChallengeStop", function()
  S.running, S.stop = false, S.time
  for i = 0, S.n - 1 do
    local r = rec(i)
    if not (r.finished or out(r)) then r.dnf = true end
  end
end)
-- 0x823dd6e8: online the order is sorted as it stands; Own The Spot (type 9) first puts anyone without a point out.
answer("ChallengeManager", "FinalizeChallengeResult", function()
  if S.kind == 9 then for i = 0, S.n - 1 do if rec(i).score <= 0 then rec(i).dnf = true end end end
  sort()
end)
-- 0x827bd320 -> 0x823dd258: every human still in is finished, DNF, DQF or left.
quiet("ChallengeManager", "GetAllHumanFinished", function()
  for i = 0, S.n - 1 do
    local r = rec(i)
    if not S.gone[i] and not (r.finished or out(r)) then return false end
  end
  return true
end)
-- GetPlayerWon (0x827bd2c8): the first place.
quiet("ChallengeManager", "GetPlayerWon", function() return ranks()[1] end)
-- A player left (0x823f286c marks the record only while the challenge runs).
function __left(i) if S.running then rec(i).left = true end end
-- The manager's frame (0x823dbe80) while the challenge runs: the rest only sort (by score); a race, once its clock is
-- past 1 ms, works out each racer's estimate (metres to go: the route left after its next way point plus the way to
-- that gate's middle, at 9 m/s, plus its penalty), sorts, then each one's gap to the leader. `where[i]` = {x, y, z}
-- of the players the binding sees; S.route[k] = {cum, {x, y, z} middle} per way point (0-based k as the scripts
-- number them), S.total / S.lap the route's length (SetupChallengeStatus 0x823daba8).
function __frame(where)
  if not S.running then return end
  if not S.race then sort() return end
  if clock() <= 0.001 then return end
  for i = 0, S.n - 1 do
    local r = rec(i)
    if not (r.finished or out(r)) then
      local gate = S.route[r.next]
      local p = where[i]
      if gate and gate.mid and p then
        local dx, dy, dz = p[1] - gate.mid[1], p[2] - gate.mid[2], p[3] - gate.mid[3]
        r.dist = math.sqrt(dx * dx + dy * dy + dz * dz)
      end
      r.est = (S.total - (r.laps * S.lap + (gate and gate.cum or 0)) + r.dist) / 9 + r.penalty
    end
  end
  sort()
  local lead = rec(ranks()[1])
  local t0 = lead.finished and lead.at + lead.penalty or lead.est + clock()
  for i = 0, S.n - 1 do
    local r = rec(i)
    if r.finished then r.split = r.at + r.penalty - t0
    elseif not out(r) then r.split = r.est + clock() - t0
    else r.split = 0 end
  end
end
-- The binding's view: the order (0-based slots, first place first), and per player (1-based lists) finished (and
-- not out), out (DNF, DQF or left), the clock at the end plus the penalty, and the split.
function __standings()
  local finished, outs, time, split = {}, {}, {}, {}
  for i = 0, S.n - 1 do
    local r = rec(i)
    finished[i + 1], outs[i + 1], time[i + 1], split[i + 1] = r.finished == true and not out(r), out(r) == true, r.at + r.penalty, r.split
  end
  return ranks(), finished, outs, time, split
end
quiet("ChallengeAsset", "IsChallengeAssetLoadComplete", function() return true end)
quiet("Challenge", "CheckIfCheatEnabled", function() return 0 end)
quiet("Entity", "DoesSkaterExist", function() return true end)
quiet("Time", "GetVisualGameTick", function() return S.tick end)
quiet("ChallengeReplay", "GetLastFrameNumberWritten", function() return S.tick end)
quiet("ChallengeReplay", "MinusFrames", function(a, b) return a - b end)
quiet("Contest", "CheckIfWinning", function() return false end)
answer("AI", "LoadPaths", function() return 0 end)
quiet("AI", "GetCharacterAnnouncerWinningIndex", function() return 0 end)
quiet("AI", "GetCharacterAnnouncerWinningPlayerContext", function() return 0 end)
quiet("AI", "GetCharacterName", function() return "" end)
quiet("AI", "GetCharacterModel", function() return "" end)
quiet("AI", "GetIfSkaterIsRegularStance", function(i) return skater(i).regular ~= false end)

local handles = 0
local function handle() handles = handles + 1 return handles end
quiet("Debug", "DebugTextPermanent", function() return handle() end)
quiet("Debug", "DebugTextTimed", function() return handle() end)
answer("ChallengeBoundary", "CreateVTBoundary", function() return handle() end)
answer("ChallengeBoundary", "CreateWPBoundary", function() return handle() end)

-- Timers: ids from 1 (the scripts park 0 in ids they compare), seconds on S.time.
local function timer(t)
  local id = S.next_id
  S.next_id = id + 1
  S.timers[id] = t
  return id
end
answer("ChallengeTimer", "AddCountdownTimer", function(s) return timer({at = S.time + (s or 0)}) end)
answer("ChallengeTimer", "AddHUDCountdownTimer", function(s)
  local id = timer({at = S.time + (s or 0), hud = true, length = s or 0})
  S.hud_timer = id
  return id
end)
answer("ChallengeTimer", "AddCountupTimer", function() return timer({from = S.time}) end)
local function remove(id) if id ~= nil then S.timers[id] = nil end if S.hud_timer == id then S.hud_timer = nil end end
answer("ChallengeTimer", "RemoveCountdownTimer", remove)
answer("ChallengeTimer", "RemoveHUDCountdownTimer", remove)
answer("ChallengeTimer", "RemoveCountupTimer", remove)
answer("ChallengeTimer", "ResetCountdownTimer", function(id, s)
  local t = S.timers[id]
  if t then t.at = S.time + (s or 0) t.length = s or t.length end
end)
quiet("ChallengeTimer", "GetCountdownTime", function(id)
  local t = S.timers[id]
  return t and t.at and math.max(0, t.at - S.time) or 0
end)
quiet("ChallengeTimer", "GetCountupTime", function(id)
  local t = S.timers[id]
  return t and t.from and S.time - t.from or 0
end)
-- A countdown that runs out now (Session::settled): it fires on the next step.
function __run_out(id)
  local t = S.timers[id]
  if t and t.at then t.at = math.min(t.at, S.time) end
end
-- The earliest countdown that has run out, removed (nil when none): one at a time, so a handler that removes
-- another expired timer stops it firing.
function __next_expired()
  local best
  for id, t in pairs(S.timers) do
    if t.at and t.at <= S.time and (best == nil or t.at < S.timers[best].at or (t.at == S.timers[best].at and id < best)) then
      best = id
    end
  end
  if best then S.timers[best] = nil end
  return best
end
-- The HUD timer's seconds left (nil: none).
function __hud_time()
  local t = S.hud_timer and S.timers[S.hud_timer]
  return t and math.max(0, t.at - S.time) or nil
end

-- Random: xorshift on the match seed; GetRandomNumber(n) is 1..n (it indexes the scripts' 1-based lists).
local function rand()
  local x = S.seed
  x = x ~ (x << 13) x = x ~ (x >> 7) x = x ~ (x << 17)
  S.seed = x
  return x
end
local function range(a, b) if b < a then return a end return a + (rand() >> 1) % (b - a + 1) end
answer("Random", "GetRandomNumber", function(n) return range(1, math.max(1, math.tointeger(n) or 1)) end)
answer("Random", "GetRandomNumberRange", function(a, b) return range(math.tointeger(a) or 0, math.tointeger(b) or 0) end)

-- Trick state of any player, as the observer last reported it.
quiet("Trick", "IsSkaterInAir", function(i) return skater(i).air == true end)
quiet("Trick", "IsDoingTrick", function(i) return skater(i).trick == true end)
-- types is keyed by the type's number as text (the binding's JSON).
quiet("Trick", "IsDoingTrickType", function(i, t) return (skater(i).types or {})[tostring(math.tointeger(t) or t)] == true end)
quiet("Trick", "GetCompletedAirGrabOccured", function(i) return skater(i).grab == true end)
quiet("Trick", "DidCompletedAirStartSwitched", function(i) return skater(i).switch == true end)
quiet("Trick", "GetCompletedAir180Count", function(i) return skater(i).spins or 0 end)
quiet("Trick", "IsInAirWithValidLaunchTrick", function(i) return skater(i).launch == true end)
quiet("Trick", "MatchedPattern", function() return false end)

-- S.K.A.T.E.
-- eSkateChallengeType as default.xex registers it (0x827be5e0); every online row is Trick.
rawset(Skate, "eSkateChallengeType", {Trick = 0, TrickSequence = 1, TrickType = 2, TrickTypeSequence = 3})
-- A scorable id is the air's first trick (aircompleted's FirstScorableId: the trick definition's index, the same on
-- every player's game); its type is the one the player who did it reported (S.skaters[id].first_type).
quiet("Trick", "GetTrickIndexFromScorableID", function(id) return id end)
quiet("Trick", "GetTrickTypeFromScorableID", function(id)
  for _, s in pairs(S.skaters) do if s.first == id then return s.first_type or 0 end end
  return 0
end)
-- The tricks the leaders set (SetPatternLastTrick, default.xex 0x827ab400 -> 0x82343270): the last three, each the
-- trick and its 180s. One among them with the same 180s is "already been done" (false); else it is the newest (true).
S.set, S.newest = {}, 0
answer("Trick", "SetPatternLastTrick", function(trick, player)
  local spins = skater(player).spins or 0
  for i = 1, 3 do
    local p = S.set[i]
    if p and p.trick == trick and p.spins == spins then S.hit = i return false end
  end
  S.newest = S.newest % 3 + 1
  S.set[S.newest] = {trick = trick, spins = spins}
  return true
end)
-- The HUD's "already been done" list (SKATEMission.GetNumABDs / GetABDTrick / GetABDIndex): the set tricks oldest
-- first as {trick, spins}, and the 0-based place of the one the leader last repeated (-1: none).
function __abds()
  local out, hit = {}, -1
  for k = 1, 3 do
    local i = (S.newest + k - 1) % 3 + 1
    if S.set[i] then
      if S.hit == i then hit = #out end
      out[#out + 1] = {S.set[i].trick, S.set[i].spins}
    end
  end
  return out, hit
end
-- Skate.MatchedPattern (0x827be900): the follower's air matched the set trick (the C++ trick tracker's flags; here
-- its first trick) with the newest set trick's 180s. No stance (audit 10-28 U-13): it never reads the switch byte
-- SetPatternLastTrick keeps per set trick. The follower copies in its own stance (Skate 3's rule, user 10-28).
answer("Skate", "MatchedPattern", function(player)
  local p, s = S.set[S.newest], skater(player)
  return p ~= nil and s.first == p.trick and (s.spins or 0) == p.spins
end)

-- Skate 3's rules over the disc's scripts (user 10-28: a Skate 1 mode keeps its screens and its identity; the details of
-- a turn follow Skate 3's version of the mode, fix-brief-3.md). RULES[kind]["state/event"] = function(orig, self,
-- message, vars) takes the place of that step's handler (__rule, from Session::new); kind is the row's type, S.kind
-- (4 S.K.A.T.E., 8 Best Trick, 9 Own The Spot, 15 Jam). "S3 n" = the decompiled Skate 3 script's line n
-- (s3online-out skate.lua / spot_battle.lua, s3contest-out contest_shared.lua).
local RULES = {[4] = {}, [8] = {}, [9] = {}, [15] = {}}
function __rule(state, event, f)
  local rule, orig = (RULES[S.kind] or {})[state .. "/" .. event], _G[f]
  if rule and orig then _G[f] = function(s, m, v) return rule(orig, s, m, v) end end
end

-- S.K.A.T.E.
local SK = RULES[4]
local function leader(vars) return vars.LeaderID end
local function follower(vars) return vars.FollowerOrder[vars.FollowerIndex] end

-- Skate 3's rule (user 10-28): the first leader is drawn at random (S3 137), the same on every console: the match token's
-- draw (S.first), as skate3_skate_game's; the followers after it in slot order, wrapping (S3 round_setup 299-307). The
-- list is every slot, a leaver's too (review 10-28 I2): the disc counts only the players still here (SKATE 34, 54), so
-- consoles that heard a leave at different times listed different players and leaders. The disc's own disconnect
-- handling then skips a leaver (round_over, process_attempt; a leader who left gets 1 s, `held`).
SK["setup_skate/stateenter"] = function(o, self, m, vars)
  o(self, m, vars)
  local n = S.n
  vars.NumSkaters, vars.SkatersList, vars.SkatersRankingList, vars.FollowerOrder = n, {}, {}, {}
  for i = 1, n do
    vars.SkatersList[i] = {id = i - 1, status = 0, time = 0, name = tostring(ChallengeManager.GetSkaterName(i - 1)), rank = 0}
    vars.SkatersRankingList[i] = {id = i - 1, rank = 0}
  end
  vars.LeaderID = S.first % n
  for k = 1, n - 1 do vars.FollowerOrder[k] = (vars.LeaderID + k) % n end
  Skate.UpdateCurrentPlayerIndex(vars.LeaderID)
end

-- Review 10-28 C1: a follower leaving in follower_wait (its 2.5 s before the copy) sets TimerID = -1 (SKATE 1151-1155),
-- so its timerexpired never matches and every console waited there until the 30-minute cap. A timer at once instead:
-- the flow goes on to wait_to_enter_spot2, which gives the leaver 1 s (`held`) and moves on.
SK["follower_wait/playerdisconnected"] = function(o, self, m, vars)
  o(self, m, vars)
  if vars.TimerID == -1 then vars.TimerID = ChallengeTimer.AddCountdownTimer(0.01) end
end

-- Skate 3's rule (user 10-28): once every follower has copied, the lead passes to the next player, no letter (S3 634 /
-- 929 AllFollowersHaveCopied, process_round 1162-1164, leader_get_letter 1440-1445). The disc's round_over keeps its
-- leader (978-1028); here the leader goes to the end of the order and the first follower leads, as the disc's
-- leader_failure rotates them. A leader who left is the disc's own round_over case.
SK["leader_success/stateenter"] = function(o, self, m, vars) vars.AllFollowersHaveCopied = true return o(self, m, vars) end
SK["follower_failure/stateenter"] = function(o, self, m, vars) vars.AllFollowersHaveCopied = false return o(self, m, vars) end
SK["round_over/stateenter"] = function(o, self, m, vars)
  if vars.AllFollowersHaveCopied == true and table.getn(vars.FollowerOrder) > 0 and not Online.IsPlayerDisconnected(vars.LeaderID) then
    vars.FollowerOrder[table.getn(vars.FollowerOrder) + 1] = vars.LeaderID
    vars.LeaderID = table.remove(vars.FollowerOrder, 1)
  end
  vars.AllFollowersHaveCopied = false
  return o(self, m, vars)
end

-- Skate 3's rule (user 10-28): stepping off the board once in the spot fails the attempt (S3 619-627: the leader's trick
-- is invalid; 1149-1156: the follower's not copied). The disc has no skateroffboard handler: Session::new names these for
-- leader_attempt / follower_attempt, the states the disc enters on a spot volume (S3's OkToScore). They fail it as the
-- disc's own judged fails do: its message, then WaitTime (SKATE 386-391, 818-823).
local function off_board(who, sub)
  return function(self, message, vars)
    if message.PlayerID == who(vars) and vars.LockoutUser ~= true then
      vars.LockoutUser, vars.AttemptSuccessful = true, false
      HUD.ChangeHUDState(HUD.eSk8HUDState.MissionSkateChallenge, HUD.eSk8HUDSkateState[sub])
      vars.TimerID = ChallengeTimer.AddCountdownTimer(vars.WaitTime)
    end
  end
end
__offboard_leader = off_board(leader, "MessageTrickNotSet")
__offboard_follower = off_board(follower, "MessageTrickNotCopied")

-- Skate 3's rule (user 10-28): the attempt's clock starts as S3's: a 5 s pre-attempt (wait_for_leader 412,
-- wait_for_follower 874), cut short when the attempter leaves the start volume or the session (418-437, 882), then the
-- row's limit, 1 s for one who left (443-447). The limit stays the row's UserAttemptTimeOut. Skate 1's rows have no start
-- volume: reaching a spot volume cuts it short (and then counts). The disc's own start stays (the leader's 1 s settle,
-- NewWaitTimer, then the unpin and the HUD's set-the-trick; the follower's turn message): only its clock waits, the
-- attempt locked meanwhile (the disc's handlers check LockoutUser).
local PRE_ATTEMPT = 5
local function in_spot(self, volume)
  for i = 1, self.SpotVolumes.Count do if self.SpotVolumes[i] == volume then return true end end
  return false
end
local function hold_clock(vars)
  ChallengeTimer.RemoveHUDCountdownTimer(vars.AttemptTimerID)
  vars.AttemptTimerID, vars.LockoutUser = nil, true
  vars.PreAttemptTimerID = ChallengeTimer.AddCountdownTimer(PRE_ATTEMPT)
end
local function start_clock(self, vars, who)
  if vars.PreAttemptTimerID == nil then return end
  ChallengeTimer.RemoveCountdownTimer(vars.PreAttemptTimerID)
  vars.PreAttemptTimerID, vars.LockoutUser = nil, false
  vars.AttemptTimerID = ChallengeTimer.AddHUDCountdownTimer(Online.IsPlayerDisconnected(who) and 1 or self.UserAttemptTimeOut)
end
local function held(self, vars, who)
  hold_clock(vars)
  if Online.IsPlayerDisconnected(who) then start_clock(self, vars, who) end
end
SK["wait_to_enter_spot_volume/timerexpired"] = function(o, self, m, vars)
  if m.TimerID == vars.PreAttemptTimerID then return start_clock(self, vars, vars.LeaderID) end
  local settled = m.TimerID == vars.NewWaitTimer
  o(self, m, vars)
  if settled then held(self, vars, vars.LeaderID) end
end
SK["wait_to_enter_spot2/stateenter"] = function(o, self, m, vars)
  o(self, m, vars)
  held(self, vars, follower(vars))
end
SK["wait_to_enter_spot2/timerexpired"] = function(o, self, m, vars)
  if m.TimerID == vars.PreAttemptTimerID then return start_clock(self, vars, follower(vars)) end
  return o(self, m, vars)
end
for state, who in pairs({wait_to_enter_spot_volume = leader, wait_to_enter_spot2 = follower}) do
  SK[state .. "/triggerentercollision"] = function(o, self, m, vars)
    if m.PlayerID == who(vars) and in_spot(self, m.VolumeID) then start_clock(self, vars, who(vars)) end
    return o(self, m, vars)
  end
  SK[state .. "/playerdisconnected"] = function(o, self, m, vars)
    if m.PlayerIndex == who(vars) then start_clock(self, vars, who(vars)) end
    return o(self, m, vars)
  end
end

-- Skate 3's rule (user 10-28): a bail fails the attempt at once and the turn moves on 2 s later (S3 451-475 / 986-1010:
-- the wipeout's ChangeState, then leader_failure's 2 s, 709). The disc locks it and shows Bailed at once but waits 3.5 s
-- (SKATE 247-258, 291-300, 682-694, 744-756): that wait is 2 s.
local BAIL_HOLD = 2
local function bailed(o, self, m, vars)
  local locked = vars.LockoutUser == true
  o(self, m, vars)
  if not locked and vars.LockoutUser == true then ChallengeTimer.ResetCountdownTimer(vars.TimerID, BAIL_HOLD) end
end
for _, state in ipairs({"wait_to_enter_spot_volume", "leader_attempt", "wait_to_enter_spot2", "follower_attempt"}) do
  SK[state .. "/wipeout"] = bailed
end

-- Own The Spot (Skate 1's Spot Battle). The turn itself (TimeLimit of lines, the best one through every spot scores, a
-- bail ends it 2 s on) stays the disc's.
local OT = RULES[9]
-- Skate 3's rule (user 10-28): a random first player, every round starting again from that player, slot order after
-- (S3 spot_battle.lua 143-144 RandomStartIndex, 245-266), the same on every console (S.first, as skate3_uniform's). The
-- disc goes from slot 0 every round (ownspot 73-125, 351-382); its getNextSkaterIdx / isAnySkaterInRound take the skater
-- after the index they're given, so that index is set to the one before S3's next.
local function first_up(vars) return S.first % vars.NumOfSkaters + 1 end
-- The skater still here after slot `idx` (1-based; 0: none yet) in this round's order, nil once the round is done.
local function next_up(vars, idx)
  local n, r = vars.NumOfSkaters, first_up(vars)
  for k = idx == 0 and 0 or (idx - r) % n + 1, n - 1 do
    local s = (r - 1 + k) % n + 1
    if not Online.IsPlayerDisconnected(s - 1) then return s end
  end
end
OT["challenge_setup/stateenter"] = function(o, self, m, vars)
  local s = vars.S3NextUp or next_up(vars, 0)
  vars.S3NextUp = nil
  if s then vars.ActiveSkaterIdx = s - 1 end
  return o(self, m, vars)
end
-- The disc starts the challenge manager's records on slot 0's first turn (ownspot 142-144); with another first, on its.
OT["challenge_setup/timerexpired"] = function(o, self, m, vars)
  if m.TimerID == vars.CDTimerID and vars.CurrentRound == 1 and not vars.S3Started then
    vars.S3Started = true
    if vars.ActiveSkaterIdx ~= 1 then ChallengeManager.ChallengeStart() end
  end
  return o(self, m, vars)
end
-- Skate 3's rule (user 10-28): at the buzzer the turn runs 2 s more, so a line landing (or a bail) in them still counts,
-- but not on the match's last run (S3 spot_battle.lua 902-911 EndRunTimerID / LastRun; skate3_uniform END_RUN). The
-- disc's turn is the same kind of window; the 2 s take TurnTimerID's place, so its own end comes after them. The user
-- kept the 10 s turn: only a line already under way at the buzzer counts in them (S3's turn is that one line), and a
-- player who has left gets none (review 10-28 M1 / M2).
local END_RUN = 2
OT["entered_trick_boundary/stateenter"] = function(o, self, m, vars) vars.S3Grace = false return o(self, m, vars) end
OT["entered_trick_boundary/sequencecomplete"] = function(o, self, m, vars)
  if vars.S3Grace and (m.StartSimGameTick or 0) > vars.S3GraceTick then return end
  return o(self, m, vars)
end
OT["entered_trick_boundary/timerexpired"] = function(o, self, m, vars)
  local id, s = m.TimerID, next_up(vars, vars.ActiveSkaterIdx)
  local here = not Online.IsPlayerDisconnected(vars.ActiveSkaterIdx - 1)
  if id == vars.TurnTimerID and not vars.S3Grace and here and (s ~= nil or vars.CurrentRound < self.NumRounds) then
    vars.S3Grace, vars.S3GraceTick, vars.TurnTimerID = true, Time.GetVisualGameTick(), ChallengeTimer.AddCountdownTimer(END_RUN)
    return
  end
  if id == vars.TurnTimerID or id == vars.OOBTimer or id == vars.DCToolTimer or id == vars.WipeToolTimer then
    vars.S3NextUp, vars.ActiveSkaterIdx = s, s and s - 1 or vars.NumOfSkaters
  end
  return o(self, m, vars)
end

-- Best Trick / Jam
-- Skate 3's rule (user 10-28): every competitor still here earns its place's points, a 0 round score too (S3
-- contest_shared 1561-1573). The disc gives a 0 none (BT 689-693, `0 < score`): for its pass a still-here 0 counts as 0.5
-- (under every real score, level with the other 0s), the half taken off its running total first so the standings it
-- reports stay whole; both are put back after.
RULES[8]["results/stateenter"] = function(o, self, m, vars)
  if vars.AccumulatedScores == nil then
    vars.AccumulatedScores = {}
    for i = 1, vars.NumOfSkaters do vars.AccumulatedScores[i] = 0 end
  end
  local zero = {}
  for i = 1, vars.NumOfSkaters do
    if vars.RoundScore[i] == 0 and not Online.IsPlayerDisconnected(i - 1) then
      zero[#zero + 1] = i
      vars.RoundScore[i], vars.AccumulatedScores[i] = 0.5, vars.AccumulatedScores[i] - 0.5
    end
  end
  o(self, m, vars)
  for _, i in ipairs(zero) do
    vars.RoundScore[i], vars.AccumulatedScores[i] = 0, math.floor(vars.AccumulatedScores[i] + 0.5)
  end
end
-- Skate 3's rule (user 10-28): a sequence done after a marker return scores nothing (S3 contest_shared 1091, `not
-- message.UsedSessionMarker`; the probe flags the sequence after a return).
local function unmarked(o, self, m, vars) if m.UsedSessionMarker == true then return end return o(self, m, vars) end
RULES[8]["event_round/sequencecomplete"] = unmarked
RULES[15]["event_round/sequencecomplete"] = unmarked
-- Skate 3's rule (user 10-28): each round's start sets the session marker (S3 contest_shared 880), Jam's as Best Trick's
-- event_round does (BT 507).
RULES[15]["event_round/stateenter"] = function(o, self, m, vars)
  if vars.RoundTimerSet == false then Challenge.SetNewSessionMarker(Online.GetLocalPlayerID()) end
  return o(self, m, vars)
end
