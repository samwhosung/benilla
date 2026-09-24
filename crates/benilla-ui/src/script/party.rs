//! The party/raid **Era API surface** (phase 2) — the engine-free seam mirroring
//! [`super::unit`]: the app pushes a roster **snapshot** ([`UiScript::set_party`]) built from its own
//! `GroupState` wire mirror, and the `GetNumPartyMembers`/`GetPartyLeaderIndex`/`GetLootMethod`/…
//! globals here read that plain data. The invite/uninvite/promote/loot-config calls are the outbound
//! half: they queue a [`PartyRequest`] the app drains ([`UiScript::take_party_requests`]) and turns
//! into the matching `CMSG_GROUP_*`/`CMSG_LOOT_METHOD` send — no ECS/net reach from the engine,
//! exactly [`super::unit`]'s split.
//!
//! Per-member game state (health/mana/level/reaction/…) does **not** live here — it rides the
//! existing per-unit snapshots under the `"party1"`..`"party4"` tokens, the same
//! feed `"player"`/`"target"` use ([`super::unit::UnitState`]). This module owns only the
//! roster-level facts a unit snapshot can't carry: how many members, who leads, the loot
//! configuration. `PartyState::default()` is "not in a group" — every getter then answers the
//! solo-player shape a fresh client reports (`GetNumPartyMembers()` `0`, `GetLootMethod()`
//! `("group", nil)`, …).
//!
//! The **raid roster** ([`PartyState::raid`]) is the same shape one level out: the app pushes the
//! whole array — the player included, which is why `UnitInRaid("player")` needs no token special
//! case the way `UnitInParty` does — and `GetNumRaidMembers`/`GetRaidRosterInfo`/`UnitInRaid` read
//! it. It is deliberately ONE list rather than a count beside a list: the reference indexes the
//! array `0xb712a8` bounded by the count `0xb713e0`, and two of the three bindings walk exactly
//! that pair, so a client whose count and array can disagree hands an addon looping
//! `for i = 1, GetNumRaidMembers()` a miss tuple it will then index. The per-member grid/UI is
//! the RaidFrame, which reads exactly this array.
//!
//! The **raid management verbs** are the outbound half again, and they address
//! members three different ways because the reference's own bindings do: by **raid index**
//! (`SetRaidSubgroup`, `SwapRaidSubgroup`, `UninviteFromRaid` — the RaidFrame has the index in
//! hand), by **name** (`PromoteByName`, `PromoteToAssistant`, `DemoteAssistant` — UnitPopup
//! carries a name, never an index), and by **nothing at all** (`ConvertToRaid`, `DoReadyCheck`,
//! `RequestRaidInfo`). Resolving index/name to whatever the wire wants is the app's job at the
//! drain, exactly as `InviteToParty`'s token resolution already is: this side stays plain data.
//!
//! The **saved-instance list** ([`SavedInstanceInfo`]) is a second app-pushed snapshot, separate
//! from [`PartyState`] because it arrives on its own packet (`SMSG_RAID_INSTANCE_INFO`) and
//! outlives every roster change — folding it into the roster push would clear it every time the
//! group moved.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::Model;

/// One roster member's engine-owned facts (decision 0434 §2/§3) — deliberately thin: everything else
/// (health, class, reaction, …) is the unit snapshot under its `"partyN"` token.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartyMemberInfo {
    pub name: String,
    /// The member's GUID — the identity `UnitInParty` matches arbitrary tokens (the target)
    /// against (decision 0434 §5's popup menu pick). `0` = unknown, never matches.
    pub guid: u64,
}

/// One saved raid lockout — `GetSavedInstanceInfo`'s three returns. Pushed whole
/// by the app ([`UiScript::set_saved_instances`]) from `SMSG_RAID_INSTANCE_INFO`, with the map
/// **name** already resolved: the wire carries a `Map.dbc` id and the DBC is the app's to read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedInstanceInfo {
    /// The instance's display name (`Map.dbc`'s own), e.g. `"Molten Core"`.
    pub name: String,
    /// The instance id — what the panel prints in its ID column.
    pub instance: u32,
    /// Seconds remaining until the lockout resets; the panel runs it through `SecondsToTime`.
    pub reset: u32,
}

/// One **raid roster** row — `GetRaidRosterInfo`'s nine returns, as the app resolved them
/// (`0x4bb560`). Field order is the push order of the reference's success tuple.
///
/// The row is the *record*, not the answer: two of the nine are computed at the binding
/// ([`Self::subgroup`]'s 1-based exposure, [`Self::zone`]'s offline substitution), because both
/// adjustments live in the binding's own bytes and doing them at the feed would put a Lua-facing
/// convention in the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RaidMemberInfo {
    /// Return 1 — the member's name, from the guid-keyed name cache (`0x4bb5ee`).
    ///
    /// **Empty means "not cached yet", and that is not a cosmetic difference**: the reference
    /// sends a cache miss to the *same* arm an out-of-range index takes (`0x4bb5f8 je 0x4bb7b9`),
    /// so an occupied slot whose name has not arrived is indistinguishable from no slot at all.
    /// Reproduced — [`raid_roster_info`] answers the miss tuple for an empty name.
    pub name: String,
    /// The member's GUID — what `UnitInRaid` matches an arbitrary token against. `0` = unknown,
    /// never matches (the reference's membership helper `0x4baee0` short-circuits GUID `0:0` to
    /// false at its entry).
    pub guid: u64,
    /// Return 2 — the rank, **exposed exactly as stored** (`0x4bb607 fild [edi+0xc]`, no
    /// adjustment). The numeric convention is not derivable from the binding alone; the corpus
    /// does, consistently: `ChatLog.lua:351-353` prints `@` for `2` and `*`
    /// for `1`, i.e. **0 member · 1 assistant · 2 leader**, which is what the app fills.
    pub rank: u32,
    /// Return 3 — the subgroup **as stored, 0-based**. The binding exposes `subgroup + 1`
    /// (`0x4bb61a inc eax`, a VERIFIED adjustment); keeping the record 0-based means the wire's
    /// own value (`GroupMemberEntry::flags` bits 0-2) lands here unconverted and the +1 has one
    /// home.
    pub subgroup: u32,
    /// Return 4 — the member's level. `0` when neither a streamed object nor a stats packet has
    /// carried one (the reference's own third arm pushes `0` there too).
    pub level: u32,
    /// Return 5 — the **localized** class name ("Warrior"), `None` → `nil`.
    pub class: Option<String>,
    /// Return 6 — the class file/token ("WARRIOR"), the one non-localized string in the tuple.
    pub class_file: Option<String>,
    /// Return 7 — the member's zone name while [`Self::online`]; `None` → `nil`. **When offline
    /// this field is not read at all**: the reference's third arm pushes the localized global
    /// `PLAYER_OFFLINE` in the zone slot rather than `nil`, and the binding does that itself.
    pub zone: Option<String>,
    /// Return 8 — connected. Also the switch for return 7's two arms, because it is in the
    /// reference: return 8 is `1` on exactly the two arms that produced a real zone and `nil` on
    /// the `PLAYER_OFFLINE` one.
    pub online: bool,
    /// Return 9 — **deliberately unlabelled.** The mechanism is confirmed: a streamed member
    /// answers `1` iff `[obj+0x110 +0x40] <= 0` (UNIT_FIELD_HEALTH by the descriptor line this
    /// repo already anchors), and an unstreamed one iff the roster record's `[+0x18]` carries
    /// **both** bits `0x4` and `0x1`. The name itself is a guess, not settled: two independent
    /// reads proposed "isDead" positionally, but it is not adopted here.
    ///
    /// Three things corroborate that guess without settling it, recorded here so whoever settles
    /// it starts ahead: the whole corpus destructures position 9 as `isDead`
    /// (`ChatLog.lua:347`, `CT_RaidAssist/CT_RAOptions.lua:89`, `oRA2/Leader/Ready.lua:241`);
    /// health ≤ 0 is this client's own dead test; and our wire's status byte —
    /// `benilla_protocol::messages::member_status`, from vmangos `Group.cpp:45-63` — spells
    /// `ONLINE = 0x01` and `DEAD = 0x04`, which is *exactly* the bit pair the second arm tests.
    /// The app fills this from that pair, so we reproduce the mechanism whatever it turns out to
    /// be called.
    pub ninth: bool,
}

/// The party/raid roster snapshot, pushed whole by the app each frame it changes
/// ([`UiScript::set_party`]) — the `GroupState` merged view's roster-level facts.
/// `PartyState::default()` = not in a group.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartyState {
    /// The other party members, `"party1"`..`"party4"` order (the recipient never appears in its own
    /// list, matching `SMSG_GROUP_LIST`); empty = not in a group. Never more than 4 — in a raid
    /// this is our own subgroup's slice and the whole roster is [`Self::raid`].
    pub members: Vec<PartyMemberInfo>,
    /// The party leader, on the Lua `GetPartyLeaderIndex` scale: `0` the player leads, `1..=4` that
    /// `members` slot (1-based) leads.
    pub leader_index: u32,
    /// **The cached group-leader GUID** — the reference's `[0xbc75f8]:[0xbc75fc]` pair, `0` when
    /// ungrouped. Fed straight from the wire's leader GUID, not derived from
    /// [`Self::leader_index`].
    ///
    /// It exists because `UnitIsPartyLeader`'s second leg is a GUID compare and an index cannot
    /// stand in for it: the leg has to answer for a member whose object the client does **not**
    /// hold — an out-of-range `party3`, any `raidN` — where there is no descriptor to read the
    /// flag from, and it has to answer for the *resolved token's* GUID rather than for a slot.
    ///
    /// **Compared WITHOUT a zero guard**, which is the load-bearing part and reads like a bug:
    /// `IsPartyLeader 0x4e9130` short-circuits on a `0:0` cached leader, and
    /// `UnitIsPartyLeader 0x516210` does not. So an unresolvable-but-non-raising argument (none,
    /// `nil`, `""`, `"target"` with no target) resolves to `0:0`, matches the zeroed leader, and
    /// the reference answers **`1` while solo**. A client that answers `nil` there is not
    /// reproducing 1.12.
    pub leader_guid: u64,
    /// **The active player's own GUID** — the reference's `0x468550` read (`[0xb41414]+0xc0`),
    /// `0` out of world. Fed by the app beside the leader; the pair is the leader gate the
    /// ready-check timeout runs on (`CheckReadyCheckTime`): the reference compares
    /// the two guids and nothing else, so a solo player (leader `0`, own non-zero) never matches.
    pub own_guid: u64,
    /// The **whole raid roster**, `GetRaidRosterInfo`'s 1-based array — empty outside a raid,
    /// and **including the player** (the reference's array does; it is why `UnitInRaid("player")`
    /// answers `1` in a raid and why this list is not `members`'s recipient-excluded shape).
    /// `GetNumRaidMembers()` is its length: one list, no count beside it (module doc).
    pub raid: Vec<RaidMemberInfo>,
    /// `GetLootMethod`'s method string: `"freeforall"` | `"roundrobin"` | `"master"` | `"group"` |
    /// `"needbeforegreed"`. `Default::default()` is `""` — the *native* reports it as `"group"` (the
    /// live shape for a solo/fresh player) when empty; the app is expected to always push a real
    /// method once grouped.
    pub loot_method: String,
    /// The master looter, as a party index (`0` the player, `1..=4` that `members` slot) — the loot
    /// method's second return, `masterlooterPartyID`. `None` = no master looter (any method but
    /// `"master"`, or a master-loot group with none assigned yet).
    pub master_looter: Option<u32>,
    /// `GetLootThreshold`'s quality floor (`2..=4`) below which non-leader loot isn't round-robin/
    /// master gated. `0` (the default) is fine while ungrouped — the getter has nothing to floor.
    pub loot_threshold: u32,
}

/// Outbound party/loot intents queued by the Era API's action calls, drained by the app
/// ([`UiScript::take_party_requests`]) into the matching `CMSG_*` send. Plain data — no mlua/ECS
/// types, [`super::unit::UnitState`]'s `TargetUnit` seam's twin.
#[derive(Clone, Debug, PartialEq)]
pub enum PartyRequest {
    /// `AcceptGroup()` — accept the pending invite.
    Accept,
    /// `DeclineGroup()` — decline the pending invite.
    Decline,
    /// `LeaveParty()` — leave the current group (no confirmation popup).
    Leave,
    /// `InviteByName(name)` — invite by character name.
    InviteName(String),
    /// `InviteToParty(unit)` — invite by unit TOKEN (e.g. `"target"`); the app resolves it to a name.
    InviteUnit(String),
    /// `UninviteFromParty(unit)` — kick a roster member, addressed by unit token (e.g. `"party2"`).
    UninviteUnit(String),
    /// `PromoteToPartyLeader(unit)` — hand leadership to a roster member, by unit token.
    PromoteUnit(String),
    /// `SetLootMethod(method[, masterName])` — the master-looter argument is a character NAME (the
    /// reference's own shape); the app resolves it to a roster member for the send.
    LootMethod {
        method: String,
        master_name: Option<String>,
        /// `SetLootMethod`'s optional THIRD argument. The binding reads it whatever the method is
        /// (`0x4e92a0`, presence-checked via `0x6f34d0`), unlike the master-looter argument, which
        /// it reads only for `"master"`.
        threshold: Option<u32>,
    },
    /// `SetLootThreshold(n)` — the new quality floor.
    LootThreshold(u32),
    /// `SetRaidTarget(unit, index)` — mark (1..=8) or clear (0) the raid-target icon on a
    /// unit, addressed by token; the app resolves the token to a guid for the
    /// `MSG_RAID_TARGET_UPDATE` send (decision 0434 §5's submenu, §6's board law).
    SetRaidTarget { unit: String, index: u8 },
    // ── The raid-management verbs (decision 1549's RaidFrame) ───────────────────────────────
    /// `ConvertToRaid()` — the Raid tab's own button (`CMSG_GROUP_RAID_CONVERT`, leader only).
    ConvertToRaid,
    /// `SetRaidSubgroup(index, group)` — move raid row `index` (1-based) into subgroup `group`
    /// (1-based). The wire (`CMSG_GROUP_CHANGE_SUB_GROUP`) takes a NAME and a 0-based subgroup;
    /// resolving both is the app's, because only it holds the roster the index means.
    SetSubgroup { index: u32, group: u32 },
    /// `SwapRaidSubgroup(index, other)` — trade two raid rows' subgroups
    /// (`CMSG_GROUP_SWAP_SUB_GROUP`, two names on the wire). The drag's "dropped on an occupied
    /// slot" arm; [`Self::SetSubgroup`] is its "dropped on an empty one".
    SwapSubgroup { index: u32, other: u32 },
    /// `PromoteByName(name)` — hand leadership over, addressed by name rather than by token
    /// (`CMSG_GROUP_SET_LEADER`, whose body is a guid — the app resolves it). UnitPopup's
    /// RAID_LEADER row; [`Self::PromoteUnit`] is the same send from a party token.
    PromoteName(String),
    /// `PromoteToAssistant(name)` / `DemoteAssistant(name)` — the raid assistant flag
    /// (`CMSG_GROUP_ASSISTANT_LEADER`: guid + grant byte).
    AssistantLeader { name: String, grant: bool },
    /// `UninviteFromRaid(index)` — kick raid row `index` (1-based). The reference's own row-index
    /// form; `CMSG_GROUP_UNINVITE` takes a name, which the app resolves from the same roster the
    /// index addresses.
    UninviteRaid(u32),
    /// `DoReadyCheck()` — start one (`MSG_RAID_READY_CHECK`, empty body; leader only).
    ReadyCheckStart,
    /// `ConfirmReadyCheck(ready)` — answer one (`MSG_RAID_READY_CHECK`, one byte).
    ReadyCheckAnswer(bool),
    /// `RequestRaidInfo()` — ask for the saved-instance list (`CMSG_REQUEST_RAID_INFO`).
    RequestRaidInfo,
}

impl super::UiScript {
    /// Push the roster snapshot, replacing whatever was there. A bare setter (the `spellbook`/
    /// `action` shape) — firing any `PARTY_*`/roster-changed event is the app's own diff-and-fire
    /// job, never auto-fired here.
    pub fn set_party(&mut self, state: PartyState) {
        self.model_mut().party = state;
        // `SMSG_GROUP_LIST`'s ready-check leg (`0x4ba5f0`, reached only from the `0x7d` handler):
        // the roster scan over the NEW roster — a member who left took their pending flag with
        // them — and when nobody is left pending, a forced close.
        let lua = self.lua();
        let mut model = self.model_mut();
        if model.ready_check.deadline.is_some() {
            let roster: Vec<u64> = model.party.raid.iter().map(|m| m.guid).collect();
            model.ready_check.unanswered.retain(|g| roster.contains(g));
            if model.ready_check.unanswered.is_empty() {
                ready_check_force_close(lua, &mut model);
            }
        }
    }

    /// The `MSG_RAID_READY_CHECK` open form arrived. The handler `0x4ba360` splits
    /// on the leader guid: the **leader** takes the response-collection arm — reads the per-member
    /// records the body carries (none, on vmangos) and force-closes if nobody is left pending;
    /// everyone **else** arms the 30 s deadline (`0x4ba535`) that the leader-gated tick never
    /// acts on for them, and gets the `READY_CHECK` popup — which the app fires on its own
    /// ticket edge, so this method carries only the state half.
    pub fn ready_check_request(&mut self, we_lead: bool) {
        let lua = self.lua();
        let now = clock(lua);
        let mut model = self.model_mut();
        if we_lead {
            if model.ready_check.deadline.is_some() && !ready_check_pending_online(&model) {
                ready_check_force_close(lua, &mut model);
            }
        } else {
            model.ready_check.deadline = Some(now + READY_CHECK_SECONDS);
        }
    }

    /// One member's answer, forwarded to the leader (`{guid, status}`).
    /// The handler's leader arm (`0x4ba360`): the record's guid is matched against the roster
    /// entry in full (`0x4ba3f9`/`0x4ba405`),
    /// and a match stores **the constant 0** into the "has not answered" flag whatever the status
    /// byte says (`0x4ba40e`; `ecx` is zeroed before the loop and never written) — so any answer,
    /// ready or not, clears the member. The status byte gates one thing: a `0` prints
    /// `RAID_MEMBER_NOT_READY` ("%s is not ready") for that member at arrival (`0x4ba414`), the
    /// same raw-`_G` → format → `CHAT_MSG_SYSTEM` path as the summary. Then the close test: once
    /// no member is both pending and ONLINE the check closes at once (`0x4ba4ce` → the worker)
    /// rather than at the deadline — an offline member who never answered does not hold the
    /// check open, and is still listed as AFK when it closes.
    pub fn ready_check_answered(&mut self, guid: u64, ready: bool) {
        let lua = self.lua();
        let mut model = self.model_mut();
        let Some(member) = model.party.raid.iter().find(|m| m.guid == guid) else {
            return; // an unmatched guid writes nothing (`0x4ba491`)
        };
        let name = member.name.clone();
        model.ready_check.unanswered.retain(|g| *g != guid);
        if !ready {
            let template: String = lua
                .globals()
                .get::<String>("RAID_MEMBER_NOT_READY")
                .unwrap_or_default();
            model
                .ready_check
                .lines
                .push(template.replacen("%s", &name, 1));
        }
        if model.ready_check.deadline.is_some() && !ready_check_pending_online(&model) {
            ready_check_force_close(lua, &mut model);
        }
    }

    /// The summary lines the timeout worker composed since the last drain — the reference's
    /// `0x49a870(text, 10)` = a `CHAT_MSG_SYSTEM` line the client prints itself; the app pushes
    /// them into the chat log the way every other client-composed system line goes.
    pub fn take_ready_check_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().ready_check.lines)
    }

    /// Drain the party/loot intents queued since the last call.
    pub fn take_party_requests(&mut self) -> Vec<PartyRequest> {
        std::mem::take(&mut self.model_mut().party_requests)
    }

    /// Push the saved raid-lockout list, replacing whatever was there. A bare
    /// setter like [`Self::set_party`] — firing `UPDATE_INSTANCE_INFO` is the app's diff-and-fire
    /// job, never auto-fired here.
    pub fn set_saved_instances(&mut self, saved: Vec<SavedInstanceInfo>) {
        self.model_mut().saved_instances = saved;
    }

    /// Drain the whisper targets `ChatFrame_SendTell` queued since the last call — the app opens
    /// its chat edit box prefilled `/w <name> ` for each (in practice the popup queues one).
    pub fn take_tell_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().tell_requests)
    }
}

/// **The leader predicate — ONE function, because the reference has one.**
///
/// `IsRaidLeader 0x4bb8c0` opens `mov esi,[0xbc75f8]` · `mov edi,[0xbc75fc]` and compares the pair
/// against the local player's own GUID (`0x468550`). `IsPartyLeader 0x4e9130` opens
/// `mov eax,[0xbc75fc]` · `mov esi,[0xbc75f8]` and compares **the same two globals** against the
/// same GUID; its only extra instruction is an `or ecx,eax; je` short-circuit on a `0:0` cached
/// leader, which is behaviourally inert (a zero GUID can never equal a live player's), and both
/// prologues are byte-identical in the globals they load.
///
/// **So `IsRaidLeader()` is TRUE for an ordinary 5-man party leader**, and its body reads no
/// raid-vs-party flag and never touches the roster array/count the other two raid bindings use.
/// Stubbing it `nil` "because there is no raid" would be a divergence — which is exactly why the
/// two registrations below share this function rather than each growing their own rule.
fn leads_the_group(model: &Model) -> bool {
    // Our stand-in for "the cached leader GUID equals mine": the app's leader index is `0` when
    // the player leads. The `members.is_empty()` guard is the reference's `0:0` case — ungrouped,
    // the cached pair is zero and matches nobody, whereas our `leader_index` defaults to `0`.
    !model.party.members.is_empty() && model.party.leader_index == 0
}

/// `GetRaidRosterInfo`'s nine values for a **1-based** index — the whole binding's behaviour,
/// pulled out of the registration so the arity and the miss tuple are testable as one unit.
///
/// **Nine values on every path** (`0x4bb560`, every return site `mov eax,9`): there is no arm that
/// returns fewer and none that returns nothing. Out of range, index ≤ 0, a null slot and a
/// name-cache miss all converge on `0x4bb7b9`, which pushes — in order — `nil`, the number `0`,
/// the number `1`, the number `1`, then five `nil`s. A client that models "no such member" as zero
/// values breaks `local name = GetRaidRosterInfo(i)` differently from the real client, and one
/// that models it as `nil, nil, …` breaks `ChatLog.lua:346`'s `for i = 1, MAX_RAID_MEMBERS` sweep
/// in the subgroup slot.
///
/// Takes the row **by value**, not the model: every arm below re-enters Lua (`create_string`, and
/// the `PLAYER_OFFLINE` global read), and this crate's rule is that a callback drops its `app_data`
/// borrow before it does (the module doc's MAXCSTACK/borrow discipline).
fn raid_roster_info(lua: &Lua, row: Option<RaidMemberInfo>) -> mlua::Result<MultiValue> {
    let Some(m) = row else {
        return Ok(MultiValue::from_vec(vec![
            Value::Nil,
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(1),
            Value::Nil,
            Value::Nil,
            Value::Nil,
            Value::Nil,
            Value::Nil,
        ]));
    };
    let opt_str = |s: &Option<String>| match s {
        Some(s) => lua.create_string(s).map(Value::String),
        None => Ok(Value::Nil),
    };
    // Return 7: a real zone while online, else the **localized global** `PLAYER_OFFLINE` — read
    // out of the VM exactly as `0x703bf0` reads it, so a translated GlobalStrings.lua translates
    // this too. A missing global lands on `0x6f3890`'s NULL guard, which pushes `nil` and still
    // reports one value; `Value::Nil` here is that.
    let zone = if m.online {
        opt_str(&m.zone)?
    } else {
        match lua.globals().get::<Value>("PLAYER_OFFLINE") {
            Ok(v @ Value::String(_)) => v,
            _ => Value::Nil,
        }
    };
    let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };
    Ok(MultiValue::from_vec(vec![
        lua.create_string(&m.name).map(Value::String)?,
        Value::Integer(i64::from(m.rank)),
        // `0x4bb61a inc eax` — stored 0-based, exposed 1-based (VERIFIED adjustment).
        Value::Integer(i64::from(m.subgroup) + 1),
        Value::Integer(i64::from(m.level)),
        opt_str(&m.class)?,
        opt_str(&m.class_file)?,
        zone,
        flag(m.online),
        flag(m.ninth),
    ]))
}

/// Register the party/raid globals reading the roster snapshot store (the same style/place `unit`
/// registers the `Unit*` globals).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumPartyMembers() → the roster's member count (0 = not in a group; never counts the player
    // themself, matching SMSG_GROUP_LIST's recipient-excluded array).
    g.set(
        "GetNumPartyMembers",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.party.members.len() as i64)
        })?,
    )?;

    // GetNumRaidMembers() → the raid roster's length (0 outside a raid). The count the reference
    // reads (`0xb713e0`) bounds the very array `GetRaidRosterInfo` indexes (`0xb712a8`), so it is
    // the same list here, not a number beside one.
    g.set(
        "GetNumRaidMembers",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.party.raid.len() as i64)
        })?,
    )?;

    // GetRaidRosterInfo(index) → name, rank, subgroup, level, class, fileName, zone, online, <9th>
    //
    // 1-BASED, and **exactly nine values on every path** — see [`raid_roster_info`] for the arity
    // law and the fixed miss tuple. The ONLY raise in `0x4bb560` is a non-number argument
    // (`0x4bb582 call 0x6f34d0` → `0x4bb591 call 0x6f4940`, usage string `0x8474a0`); an index of
    // 0, of −1, of 500, or of a live-but-uncached member all return the tuple.
    g.set(
        "GetRaidRosterInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetRaidRosterInfo(index)")?;
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                // `0x4bb5bb dec eax` then `0x4bb5be jae` — an UNSIGNED compare after the
                // decrement, so ONE branch catches index ≤ 0 and index > count together (and the
                // `dec` wraps rather than trapping, which `wrapping_sub` keeps true of `i32::MIN`).
                usize::try_from(index.wrapping_sub(1))
                    .ok()
                    .and_then(|i| model.party.raid.get(i))
                    // A name-cache miss shares that same arm (`0x4bb5f8 je 0x4bb7b9`).
                    .filter(|m| !m.name.is_empty())
                    .cloned()
            };
            raid_roster_info(lua, row)
        })?,
    )?;

    // IsRaidLeader() → 1 / nil, and TRUE FOR A PARTY LEADER — the shared predicate's whole point
    // ([`leads_the_group`]). No arguments, never raises.
    g.set(
        "IsRaidLeader",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if leads_the_group(&model) {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetPartyMember(id) → 1 if id is a live 1-based roster slot, else nil (era 1/nil shape).
    g.set(
        "GetPartyMember",
        lua.create_function(|lua, id: i64| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let n = model.party.members.len() as i64;
            Ok(if id >= 1 && id <= n {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetPartyLeaderIndex() → 0 (the player leads) or 1..4 (that party slot leads).
    g.set(
        "GetPartyLeaderIndex",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.party.leader_index))
        })?,
    )?;

    // IsPartyLeader() → 1 iff we're grouped AND lead it, else nil (a solo player doesn't "lead").
    // Shares [`leads_the_group`] with `IsRaidLeader` because the reference shares the globals.
    g.set(
        "IsPartyLeader",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if leads_the_group(&model) {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetLootMethod() → lootmethod, masterlooterPartyID (the era 2-tuple return; later clients add
    // a raid index third return we don't carry). An unset method (never pushed) reports the
    // fresh-player shape: "group", nil.
    g.set(
        "GetLootMethod",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // **`freeforall`, not `group`.** The three cells this reads sit past `.data`'s raw end
            // in the zero-filled tail, so a client that has never been in a group answers the
            // zeroth method — which is `freeforall`.
            let method = if model.party.loot_method.is_empty() {
                "freeforall"
            } else {
                model.party.loot_method.as_str()
            };
            let master = match model.party.master_looter {
                Some(idx) => Value::Integer(i64::from(idx)),
                None => Value::Nil,
            };
            Ok((Value::String(lua.create_string(method)?), master))
        })?,
    )?;

    // GetLootThreshold() → the quality floor (0 while ungrouped is fine — nothing to floor).
    g.set(
        "GetLootThreshold",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.party.loot_threshold))
        })?,
    )?;

    // The outbound half: each call queues a PartyRequest, the app drains and sends. No-return, era
    // shape (fire-and-forget, like TargetUnit/CastSpell).
    g.set(
        "AcceptGroup",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::Accept);
            Ok(())
        })?,
    )?;
    g.set(
        "DeclineGroup",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::Decline);
            Ok(())
        })?,
    )?;
    g.set(
        "LeaveParty",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::Leave);
            Ok(())
        })?,
    )?;
    g.set(
        "InviteByName",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::InviteName(name));
            Ok(())
        })?,
    )?;
    g.set(
        "InviteToParty",
        lua.create_function(|lua, unit: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::InviteUnit(unit));
            Ok(())
        })?,
    )?;
    g.set(
        "UninviteFromParty",
        lua.create_function(|lua, unit: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::UninviteUnit(unit));
            Ok(())
        })?,
    )?;
    g.set(
        "PromoteToPartyLeader",
        lua.create_function(|lua, unit: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::PromoteUnit(unit));
            Ok(())
        })?,
    )?;
    // SetLootMethod("method" [,master] [,threshold]) — the reference's own usage string
    // (`0x84c42c`). The master-looter argument is read ONLY for "master"; the threshold argument
    // is optional for every method.
    g.set(
        "SetLootMethod",
        lua.create_function(
            |lua, (method, master_name, threshold): (String, Option<String>, Option<u32>)| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.party_requests.push(PartyRequest::LootMethod {
                    method,
                    master_name,
                    threshold,
                });
                Ok(())
            },
        )?,
    )?;
    g.set(
        "SetLootThreshold",
        lua.create_function(|lua, n: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::LootThreshold(n));
            Ok(())
        })?,
    )?;
    // SetRaidTarget(unit, index) — the ENGINE verb. `SetRaidTargetIcon` is NOT one: it is FrameXML,
    // `function SetRaidTargetIcon(unit, index)` at `TargetFrame.lua:486`, a toggle wrapper that
    // calls this with 0 when the unit already wears `index`. We had registered the wrapper's name on
    // this body, which behaved correctly and so never showed — until 1751 put `TargetFrame.lua` on
    // the chain and the wrapper started calling a `SetRaidTarget` that did not exist. Proof both
    // ways: no `function SetRaidTarget(` anywhere in the reference's FrameXML, and both
    // `TargetFrame.lua:488`/`:490` and `Bindings.xml:1160` call it bare.
    g.set(
        "SetRaidTarget",
        lua.create_function(|lua, (unit, index): (String, u8)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .party_requests
                .push(PartyRequest::SetRaidTarget { unit, index });
            Ok(())
        })?,
    )?;

    // IsRaidOfficer() → nil, always: a 1.12 PARTY has no officer rank (the assistant flag is a
    // raid concept). The popup's leader-or-assistant gates read this and fall back to the leader
    // half. The roster now carries the assistant rank ([`RaidMemberInfo::rank`], `1`), so the
    // *data* to answer this exists — what does not is a reading of `IsRaidOfficer`'s own body,
    // and "it is probably rank ≥ 1 for the player's row" is a guess, not a mechanism. Left nil
    // until someone reads the bytes.
    g.set(
        "IsRaidOfficer",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;

    // ── The raid-management verbs ────────────────────────────────────────────
    //
    // All nine share the same shape (`ConvertToRaid 0x4bbc90`, `SetRaidSubgroup 0x4bb990`,
    // `SwapRaidSubgroup 0x4bbb00`, `PromoteToAssistant 0x4bbd20`, `RequestRaidInfo 0x4a1850`,
    // `GetSavedInstanceInfo 0x4a1920`, `UninviteFromRaid 0x48a580`, `SetRaidRosterSelection
    // 0x4bb820`): each marshals and delegates to a C++ method or net-send, with no inline
    // fidelity math. So the *binding* has no law of its own to reproduce: the law is the wire's,
    // which `benilla-protocol`'s `group` family already carries byte-golden, and the marshalling
    // is the queue below. Nothing here is a guess about a body nobody has read.
    //
    // They queue rather than send for the module doc's reason: this crate cannot reach the net.
    g.set(
        "ConvertToRaid",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::ConvertToRaid);
            Ok(())
        })?,
    )?;
    // SetRaidSubgroup(index, group) / SwapRaidSubgroup(index, other) — the drag's two landings.
    // Both take numbers through the shared argument gate, so a non-number raises the reference's
    // usage error rather than silently queueing a zero.
    g.set(
        "SetRaidSubgroup",
        lua.create_function(|lua, (index, group): (Value, Value)| {
            let index = number_arg(lua, index, "Usage: SetRaidSubgroup(index, group)")?;
            let group = number_arg(lua, group, "Usage: SetRaidSubgroup(index, group)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::SetSubgroup {
                index: index.max(0) as u32,
                group: group.max(0) as u32,
            });
            Ok(())
        })?,
    )?;
    g.set(
        "SwapRaidSubgroup",
        lua.create_function(|lua, (index, other): (Value, Value)| {
            let index = number_arg(lua, index, "Usage: SwapRaidSubgroup(index1, index2)")?;
            let other = number_arg(lua, other, "Usage: SwapRaidSubgroup(index1, index2)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::SwapSubgroup {
                index: index.max(0) as u32,
                other: other.max(0) as u32,
            });
            Ok(())
        })?,
    )?;
    g.set(
        "PromoteByName",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::PromoteName(name));
            Ok(())
        })?,
    )?;
    for (binding, grant) in [("PromoteToAssistant", true), ("DemoteAssistant", false)] {
        g.set(
            binding,
            lua.create_function(move |lua, name: String| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .party_requests
                    .push(PartyRequest::AssistantLeader { name, grant });
                Ok(())
            })?,
        )?;
    }
    g.set(
        "UninviteFromRaid",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: UninviteFromRaid(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .party_requests
                .push(PartyRequest::UninviteRaid(index.max(0) as u32));
            Ok(())
        })?,
    )?;
    g.set(
        "DoReadyCheck",
        lua.create_function(|lua, ()| {
            let now = clock(lua);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // The worker `0x4bb1d0`: every roster member except the caller is
            // flagged "has not answered" (`0x4bb24c`), the caller cleared (`0x4bb258`), the 30 s
            // deadline armed (`0x4bb2b6`) — then the packet. A party has no raid roster, so the
            // flags are empty there and the leader's own echo closes the check at once.
            let own = model.party.own_guid;
            model.ready_check.unanswered = model
                .party
                .raid
                .iter()
                .map(|m| m.guid)
                .filter(|g| *g != own)
                .collect();
            model.ready_check.deadline = Some(now + READY_CHECK_SECONDS);
            model.party_requests.push(PartyRequest::ReadyCheckStart);
            Ok(())
        })?,
    )?;
    // `CheckReadyCheckTime` (`0x4bc120` → `0x4bb310`) — the ready-check timeout tick, called from
    // the stock `UIParent.xml`'s `<OnUpdate>` every frame and driving the 30-second expiry alone
    // (nothing in the engine polls it). Four conjuncts, any failure writing nothing: a deadline is
    // armed, it has been reached, and the active player's guid equals the group leader's — a
    // LEADER gate, so it is observably inert for anyone else. On expiry it disarms the deadline
    // (the only clear-to-zero outside module init), lists every roster member still flagged as
    // unanswered, and prints `RAID_MEMBERS_AFK` with the `", "`-joined names — or
    // `READY_CHECK_NO_AFK` — as a `CHAT_MSG_SYSTEM` line. Both keys are read raw off `_G`
    // (`0x704350`: `lua_gettable` on GLOBALSINDEX, empty-string default), never through
    // `GetText`. Sends no packet, raises nothing, returns nothing.
    g.set(
        "CheckReadyCheckTime",
        lua.create_function(|lua, ()| {
            let now = clock(lua);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            ready_check_tick(lua, &mut model, now);
            Ok(())
        })?,
    )?;
    // ConfirmReadyCheck(ready) — the popup's two buttons, and the argument is read for TRUTH, not
    // for a number: the reference's own No button calls it with no argument at all
    // (`ConfirmReadyCheck()`) while Yes passes `1`, so "absent" has to mean "not ready".
    g.set(
        "ConfirmReadyCheck",
        lua.create_function(|lua, ready: Value| {
            let ready = !matches!(ready, Value::Nil | Value::Boolean(false));
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .party_requests
                .push(PartyRequest::ReadyCheckAnswer(ready));
            Ok(())
        })?,
    )?;
    g.set(
        "RequestRaidInfo",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.party_requests.push(PartyRequest::RequestRaidInfo);
            Ok(())
        })?,
    )?;

    // GetNumSavedInstances() / GetSavedInstanceInfo(index) — the Raid Info panel's pair, over the
    // app-pushed list. 1-based like every other indexed getter here, and a miss answers a plain
    // `nil` (the panel's own loop only ever indexes inside the count it just read).
    g.set(
        "GetNumSavedInstances",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.saved_instances.len() as i64)
        })?,
    )?;
    g.set(
        "GetSavedInstanceInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetSavedInstanceInfo(index)")?;
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                usize::try_from(index.wrapping_sub(1))
                    .ok()
                    .and_then(|i| model.saved_instances.get(i))
                    .cloned()
            };
            Ok(match row {
                Some(r) => MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&r.name)?),
                    Value::Integer(i64::from(r.instance)),
                    Value::Integer(i64::from(r.reset)),
                ]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // SetRaidRosterSelection(index) / GetRaidRosterSelection() — a purely CLIENT-SIDE cursor (the
    // reference's `0x4bb820` writes a global; nothing is sent). The RaidFrame sets it when a row
    // is picked up so the menu and the drag agree on who is being acted on.
    g.set(
        "SetRaidRosterSelection",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: SetRaidRosterSelection(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.raid_selection = i64::from(index);
            Ok(())
        })?,
    )?;
    g.set(
        "GetRaidRosterSelection",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.raid_selection)
        })?,
    )?;

    // ChatFrame_SendTell(name) — the popup's WHISPER action. In the ref this is ChatFrame.lua
    // filling the edit box with "/w name "; our chat edit is app-side (ui_chat), so the call
    // queues the name for the app to open the edit box prefilled (UiScript::take_tell_requests
    // drains — the PartyRequest seam's chat sibling).
    g.set(
        "ChatFrame_SendTell",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.tell_requests.push(name);
            Ok(())
        })?,
    )?;

    Ok(())
}

/// The ready-check deadline, `0x7530` ms after the arm (`0x4ba535`, `0x4bb2b6`).
const READY_CHECK_SECONDS: f64 = 30.0;

/// The client's ready-check state (`RaidInfo.cpp`): the armed deadline
/// (`[0xb713f4]`, `None` = disarmed) and the roster members whose "has not answered yet" flag
/// (`[entry+0x158]`) is set, plus the summary lines the timeout worker composed and the app has
/// not yet pushed into chat. Outside [`PartyState`] because a roster push replaces that whole
/// snapshot and this survives one — the reference keeps the two in different tables too.
#[derive(Debug, Default)]
pub(crate) struct ReadyCheckState {
    pub(crate) deadline: Option<f64>,
    pub(crate) unanswered: Vec<u64>,
    pub(crate) lines: Vec<String>,
}

/// `GetTime()`'s clock — the session seconds `UiScript::tick` advances.
fn clock(lua: &Lua) -> f64 {
    lua.globals().get("__benilla_now").unwrap_or(0.0)
}

/// The timeout worker `0x4bb310` — see `CheckReadyCheckTime`'s note for the contract.
fn ready_check_tick(lua: &Lua, model: &mut Model, now: f64) {
    let Some(deadline) = model.ready_check.deadline else {
        return;
    };
    if now < deadline {
        return;
    }
    if model.party.own_guid != model.party.leader_guid {
        return;
    }
    model.ready_check.deadline = None;
    // The roster walk: an unresolved name is silently skipped (`0x55f080` with its query arm
    // off), so a flagged member the roster cannot name contributes nothing to the list.
    let names: Vec<&str> = model
        .ready_check
        .unanswered
        .iter()
        .filter_map(|g| model.party.raid.iter().find(|m| m.guid == *g))
        .map(|m| m.name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    let raw = |key: &str| lua.globals().get::<String>(key).unwrap_or_default();
    let text = if names.is_empty() {
        raw("READY_CHECK_NO_AFK")
    } else {
        raw("RAID_MEMBERS_AFK").replacen("%s", &names.join(", "), 1)
    };
    model.ready_check.lines.push(text);
}

/// The `0x322` handler's close predicate (`0x4ba498`–`0x4ba4cc`): a member still
/// holds the check open only while they are BOTH flagged unanswered AND online (`[entry+0x18]`
/// bit 0, the `SMSG_GROUP_LIST` online byte). An empty roster closes at once (`0x4ba3ea`).
fn ready_check_pending_online(model: &Model) -> bool {
    model
        .ready_check
        .unanswered
        .iter()
        .any(|g| model.party.raid.iter().any(|m| m.guid == *g && m.online))
}

/// The two C-side force-close sites (`0x4ba4d4`, `0x4bacb4`): the deadline is set to `now − 1`
/// and the worker runs at once. The arm test is theirs too — both force only an armed check.
fn ready_check_force_close(lua: &Lua, model: &mut Model) {
    if model.ready_check.deadline.is_none() {
        return;
    }
    let now = clock(lua);
    model.ready_check.deadline = Some(now - 1.0);
    ready_check_tick(lua, model, now);
}

#[cfg(test)]
mod tests {
    use crate::script::{PartyMemberInfo, PartyRequest, PartyState, UiScript};

    fn two_member_party() -> PartyState {
        PartyState {
            members: vec![
                PartyMemberInfo {
                    name: "Alice".into(),
                    guid: 0xA11CE,
                },
                PartyMemberInfo {
                    name: "Bob".into(),
                    guid: 0xB0B,
                },
            ],
            leader_index: 1, // Alice (party1) leads
            leader_guid: 0xA11CE,
            own_guid: 0x5E1F,
            raid: Vec::new(),
            loot_method: "group".into(),
            master_looter: None,
            loot_threshold: 2,
        }
    }

    #[test]
    fn read_natives_report_the_pushed_roster() {
        let mut s = UiScript::new().unwrap();
        s.set_party(two_member_party());

        assert_eq!(s.eval::<i64>("return GetNumPartyMembers()").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return GetNumRaidMembers()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetPartyMember(1)").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetPartyMember(2)").unwrap(), 1);
        assert!(s.eval::<bool>("return GetPartyMember(3) == nil").unwrap());
        assert!(s.eval::<bool>("return GetPartyMember(0) == nil").unwrap());
        assert_eq!(s.eval::<i64>("return GetPartyLeaderIndex()").unwrap(), 1);
        // Alice (party1) leads, not us.
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());
        let (method, master) = s
            .eval::<(String, Option<i64>)>("return GetLootMethod()")
            .unwrap();
        assert_eq!(method, "group");
        assert_eq!(master, None);
        assert_eq!(s.eval::<i64>("return GetLootThreshold()").unwrap(), 2);
    }

    #[test]
    fn is_party_leader_reports_when_the_player_leads() {
        let mut s = UiScript::new().unwrap();
        let mut party = two_member_party();
        party.leader_index = 0; // the player leads
        s.set_party(party);
        assert_eq!(s.eval::<i64>("return IsPartyLeader()").unwrap(), 1);
    }

    #[test]
    fn get_loot_method_reports_the_assigned_master() {
        let mut s = UiScript::new().unwrap();
        let mut party = two_member_party();
        party.loot_method = "master".into();
        party.master_looter = Some(2); // Bob (party2)
        s.set_party(party);
        let (method, master) = s.eval::<(String, i64)>("return GetLootMethod()").unwrap();
        assert_eq!(method, "master");
        assert_eq!(master, 2);
    }

    #[test]
    fn empty_state_reports_the_solo_player_shape() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumPartyMembers()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetPartyMember(1) == nil").unwrap());
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());
        let (method, master) = s
            .eval::<(String, Option<i64>)>("return GetLootMethod()")
            .unwrap();
        // **`freeforall` is the UNGROUPED answer**, not `group`: the cells the reference reads
        // sit in `.data`'s zero-filled tail, so a client that has never been in a group reports
        // the zeroth method. The `group` above is a real party's, and stays.
        assert_eq!(method, "freeforall");
        assert_eq!(master, None);
    }

    // ── The raid trio (`UnitInRaid`, `GetRaidRosterInfo`, `IsRaidLeader`) ────────────────────
    //
    // One test per return-shape trap, in `item_stats`'s `get_item_info_tests` style: the ARITY
    // assertion is half of each, because a signature regression is the failure every individual
    // read still survives.

    fn raider(name: &str, guid: u64) -> crate::script::RaidMemberInfo {
        crate::script::RaidMemberInfo {
            name: name.into(),
            guid,
            rank: 0,
            subgroup: 0,
            level: 60,
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            zone: Some("Molten Core".into()),
            online: true,
            ninth: false,
        }
    }

    fn ten_player_raid() -> PartyState {
        let mut raid: Vec<crate::script::RaidMemberInfo> = (0u64..10)
            .map(|i| raider(&format!("Raider{i}"), 0x100 + i))
            .collect();
        raid[0].rank = 2; // the leader
        raid[0].name = "Me".into();
        raid[3].subgroup = 1; // stored 0-based — the binding exposes 2
        PartyState {
            raid,
            ..Default::default()
        }
    }

    /// **Nine values on every path**, including the paths that have no member to describe. Every
    /// return site in `0x4bb560` is `mov eax,9`; the out-of-range arm `0x4bb7b9` pushes nine
    /// values of its own rather than returning nothing.
    #[test]
    fn get_raid_roster_info_returns_nine_values_on_every_path() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());

        for index in ["1", "10", "0", "-1", "41", "9999"] {
            assert_eq!(
                s.arity(&format!("GetRaidRosterInfo({index})")).unwrap(),
                9,
                "index {index} must still be a nine-value answer"
            );
        }
        // And with no raid at all — the shape does not depend on being in one.
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetRaidRosterInfo(1)").unwrap(), 9);
    }

    /// The miss tuple is `(nil, 0, 1, 1, nil, nil, nil, nil, nil)` — **not** nine nils, and not
    /// nothing. `0x4bb7b9` pushes nil, then the number 0, then 1, then 1, then five nils.
    #[test]
    fn get_raid_roster_info_misses_with_the_fixed_tuple() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        let miss = "local a,b,c,d,e,f,g,h,i = GetRaidRosterInfo(99) \
                    return a == nil, b, c, d, e == nil, f == nil, g == nil, h == nil, i == nil";
        let (a, b, c, d, e, f, g, h, i): (bool, i64, i64, i64, bool, bool, bool, bool, bool) =
            s.eval(miss).unwrap();
        assert!(a, "name is nil");
        assert_eq!(
            (b, c, d),
            (0, 1, 1),
            "rank 0, subgroup 1, level 1 — numbers"
        );
        assert!(e && f && g && h && i, "the last five are nil");

        // An in-range member whose NAME has not been cached takes the same arm — the reference's
        // `0x4bb5f8 je 0x4bb7b9`, which is why a client must not model this as "returns nothing".
        let mut party = ten_player_raid();
        party.raid[1].name = String::new();
        s.set_party(party);
        let (name_nil, rank): (bool, i64) = s
            .eval("local a,b = GetRaidRosterInfo(2) return a == nil, b")
            .unwrap();
        assert!(name_nil && rank == 0, "an uncached name is a full miss");
    }

    /// The success tuple, destructured positionally exactly as `ChatLog.lua:347` and
    /// `oRA2/Leader/Ready.lua:241` destructure it.
    #[test]
    fn get_raid_roster_info_reports_the_pushed_row() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        let (name, rank, subgroup, level, class, file, zone, online, ninth): (
            String,
            i64,
            i64,
            i64,
            String,
            String,
            String,
            i64,
            Option<i64>,
        ) = s
            .eval("local a,b,c,d,e,f,g,h,i = GetRaidRosterInfo(1) return a,b,c,d,e,f,g,h,i")
            .unwrap();
        assert_eq!(name, "Me");
        assert_eq!(rank, 2, "as stored, no adjustment");
        assert_eq!(subgroup, 1, "stored 0 → exposed 1 (`0x4bb61a inc eax`)");
        assert_eq!(level, 60);
        assert_eq!((class.as_str(), file.as_str()), ("Warrior", "WARRIOR"));
        assert_eq!(zone, "Molten Core");
        assert_eq!(online, 1);
        assert_eq!(ninth, None, "1/nil, never true/false");
        // Subgroup is the one adjusted field: member 4 is stored in subgroup 1 and reads 2.
        assert_eq!(
            s.eval::<i64>("local _,_,g = GetRaidRosterInfo(4) return g")
                .unwrap(),
            2
        );
    }

    /// An OFFLINE member's zone slot carries the localized `PLAYER_OFFLINE` global, never nil —
    /// and return 8 goes nil on that same arm (`0x4bb705`).
    #[test]
    fn get_raid_roster_info_puts_player_offline_in_the_zone_slot() {
        let mut s = UiScript::new().unwrap();
        let mut party = ten_player_raid();
        party.raid[1].online = false;
        party.raid[1].zone = Some("Molten Core".into()); // ignored on the offline arm
        s.set_party(party);
        // GlobalStrings.lua is the app's to load; the binding reads whatever the VM has.
        s.run(r#"PLAYER_OFFLINE = "Offline""#).unwrap();
        let (zone, online): (String, Option<i64>) = s
            .eval("local _,_,_,_,_,_,g,h = GetRaidRosterInfo(2) return g, h")
            .unwrap();
        assert_eq!(zone, "Offline");
        assert_eq!(online, None);
    }

    /// A non-number argument is the ONLY raise in the whole function (`0x4bb591`), and it raises
    /// rather than returning nil or nothing — `0x6f4940` never returns.
    #[test]
    fn get_raid_roster_info_raises_only_on_a_non_number() {
        let s = UiScript::new().unwrap();
        let err = s.arity("GetRaidRosterInfo({})").unwrap_err();
        assert!(
            format!("{err}").contains("Usage: GetRaidRosterInfo(index)"),
            "got {err}"
        );
        // A numeric string coerces (Lua 5.1's `lua_isnumber`) and does NOT raise.
        assert_eq!(s.arity(r#"GetRaidRosterInfo("3")"#).unwrap(), 9);
    }

    /// `UnitInRaid` answers the **constant 1**, not a roster index — the value is the hard-coded
    /// double at `0x51637e`, and the helper underneath never exposes a counter.
    #[test]
    fn unit_in_raid_answers_one_not_an_index() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        s.set_unit("player", Some(unit(true, 0x100)));
        s.set_unit("target", Some(unit(true, 0x105))); // roster row 6
        s.set_unit("mouseover", Some(unit(true, 0xDEAD)));

        assert_eq!(s.eval::<i64>(r#"return UnitInRaid("player")"#).unwrap(), 1);
        assert_eq!(
            s.eval::<i64>(r#"return UnitInRaid("target")"#).unwrap(),
            1,
            "row 6 still answers 1 — this is not an index"
        );
        assert!(s
            .eval::<bool>(r#"return UnitInRaid("mouseover") == nil"#)
            .unwrap());
        // **The input partition, and it is NOT "never raises".** A quiet nil covers a missing,
        // wrong-typed, or a recognised-but-unmatched token; a token matching none of the
        // resolver's prefixes raises instead.
        //
        // Quiet nil: `0x6f3690` returns NULL for a missing or uncoercible argument, `0x515970` maps
        // NULL/empty to GUID `0:0`, and `0x4baee0` short-circuits `0:0` to false at entry.
        for call in ["UnitInRaid()", "UnitInRaid(nil)", r#"UnitInRaid("party3")"#] {
            assert!(
                s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
                "{call} must answer nil"
            );
        }
        // Raise: a token matching none of the nine prefixes reaches
        // `luaL_error("Unknown unit name: %s")` and longjmps — and a NUMBER is coerced to a string
        // first, so it raises too. `UnitInRaid("bogus")` never returns on the real client.
        for call in ["UnitInRaid(7)", r#"UnitInRaid("nosuchtoken")"#] {
            assert!(
                s.run(call).is_err(),
                "{call} must raise — the token resolves to no prefix at all"
            );
        }
    }

    /// **`IsRaidLeader()` is true for an ordinary 5-man party leader.** Its body reads the same
    /// two leader-GUID globals `IsPartyLeader` does and carries no raid-vs-party flag at all
    /// (`0x4bb8c0` vs `0x4e9130`). Answering nil
    /// "because there is no raid" would be the divergence.
    #[test]
    fn is_raid_leader_is_true_for_a_party_leader() {
        let mut s = UiScript::new().unwrap();
        let mut party = two_member_party();
        party.leader_index = 0; // we lead — a PARTY, with an empty raid roster
        s.set_party(party);
        assert!(
            s.eval::<i64>("return GetNumRaidMembers()").unwrap() == 0,
            "no raid, and that is exactly the point"
        );
        assert_eq!(s.eval::<i64>("return IsRaidLeader()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return IsPartyLeader()").unwrap(), 1);

        // Somebody else leads: both go nil, together.
        s.set_party(two_member_party()); // leader_index 1 = Alice
        assert!(s.eval::<bool>("return IsRaidLeader() == nil").unwrap());
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());

        // Ungrouped: nil (the reference's cached leader pair is `0:0` and matches nobody).
        let s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return IsRaidLeader() == nil").unwrap());
    }

    /// One list, not a count beside one: `GetNumRaidMembers` and `GetRaidRosterInfo` cannot
    /// disagree about how many members there are.
    #[test]
    fn get_num_raid_members_bounds_the_roster_it_indexes() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        assert_eq!(s.eval::<i64>("return GetNumRaidMembers()").unwrap(), 10);
        assert!(s
            .eval::<bool>("return GetRaidRosterInfo(GetNumRaidMembers()) ~= nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetRaidRosterInfo(GetNumRaidMembers() + 1) == nil")
            .unwrap());
    }

    #[test]
    fn intent_natives_queue_the_exact_request_sequence() {
        let mut s = UiScript::new().unwrap();
        // Nothing queued until a call lands.
        assert!(s.take_party_requests().is_empty());

        s.run("AcceptGroup()").unwrap();
        s.run("DeclineGroup()").unwrap();
        s.run("LeaveParty()").unwrap();
        s.run(r#"InviteByName("Bob")"#).unwrap();
        s.run(r#"InviteToParty("target")"#).unwrap();
        s.run(r#"UninviteFromParty("party2")"#).unwrap();
        s.run(r#"PromoteToPartyLeader("party2")"#).unwrap();
        s.run(r#"SetLootMethod("master", "Bob")"#).unwrap();
        s.run(r#"SetLootThreshold(3)"#).unwrap();

        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::Accept,
                PartyRequest::Decline,
                PartyRequest::Leave,
                PartyRequest::InviteName("Bob".into()),
                PartyRequest::InviteUnit("target".into()),
                PartyRequest::UninviteUnit("party2".into()),
                PartyRequest::PromoteUnit("party2".into()),
                PartyRequest::LootMethod {
                    method: "master".into(),
                    master_name: Some("Bob".into()),
                    threshold: None,
                },
                PartyRequest::LootThreshold(3),
            ]
        );
        // The drain is a take — a second read is empty.
        assert!(s.take_party_requests().is_empty());
    }

    #[test]
    fn set_loot_method_without_a_master_name_queues_none() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SetLootMethod("freeforall")"#).unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![PartyRequest::LootMethod {
                method: "freeforall".into(),
                master_name: None,
                threshold: None,
            }]
        );

        // The optional THIRD argument, which the real binding reads for every method — not only
        // for "master", the way the master-looter argument is read.
        s.run(r#"SetLootMethod("group", nil, 4)"#).unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![PartyRequest::LootMethod {
                method: "group".into(),
                master_name: None,
                threshold: Some(4),
            }]
        );
    }

    #[test]
    fn set_raid_target_icon_queues_the_token_and_index() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SetRaidTarget("target", 8)"#).unwrap();
        s.run(r#"SetRaidTarget("party2", 0)"#).unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::SetRaidTarget {
                    unit: "target".into(),
                    index: 8,
                },
                PartyRequest::SetRaidTarget {
                    unit: "party2".into(),
                    index: 0,
                },
            ]
        );
    }

    /// `IsRaidOfficer()` still answers nil, and the raid arc landing did NOT
    /// change that — it is not waiting on a feature, it is waiting on a reading of `0x4bb910`'s
    /// own body. The binding's doc carries the refusal; this pins the behaviour so a later
    /// "surely it is just rank >= 1" edit has to argue with a test.
    ///
    /// What it costs today, stated where someone will find it: `RaidFrameAddMemberButton` is
    /// enabled for the raid LEADER only, where the reference also enables it for an assistant.
    #[test]
    fn is_raid_officer_is_still_nil_and_that_is_a_missing_carve_not_a_missing_feature() {
        let s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return IsRaidOfficer() == nil").unwrap());
    }

    #[test]
    fn chat_frame_send_tell_queues_the_name() {
        let mut s = UiScript::new().unwrap();
        assert!(s.take_tell_requests().is_empty());
        s.run(r#"ChatFrame_SendTell("Alice")"#).unwrap();
        assert_eq!(s.take_tell_requests(), vec!["Alice".to_string()]);
        assert!(s.take_tell_requests().is_empty());
    }

    // ── The identity predicates (the popup's menu pick + gating) ─────────────

    fn unit(exists: bool, guid: u64) -> crate::script::UnitState {
        crate::script::UnitState {
            exists,
            guid,
            ..Default::default()
        }
    }

    #[test]
    fn unit_is_unit_compares_guids_and_tokens() {
        let mut s = UiScript::new().unwrap();
        s.set_unit("player", Some(unit(true, 0x10)));
        s.set_unit("target", Some(unit(true, 0x10)));
        s.set_unit("party1", Some(unit(true, 0x20)));
        // Same guid across tokens; same token trivially; different guids nil.
        assert_eq!(
            s.eval::<i64>(r#"return UnitIsUnit("target", "player")"#)
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>(r#"return UnitIsUnit("player", "player")"#)
                .unwrap(),
            1
        );
        assert!(s
            .eval::<bool>(r#"return UnitIsUnit("party1", "player") == nil"#)
            .unwrap());
        // Zero guids never match across tokens (unknown identity is not identity).
        s.set_unit("target", Some(unit(true, 0)));
        s.set_unit("mouseover", Some(unit(true, 0)));
        assert!(s
            .eval::<bool>(r#"return UnitIsUnit("target", "mouseover") == nil"#)
            .unwrap());
        // A missing token is nil.
        assert!(s
            .eval::<bool>(r#"return UnitIsUnit("pet", "player") == nil"#)
            .unwrap());
    }

    /// `UnitPlayerOrPetInParty`/`InRaid` (1958): a member's own guid, or a unit whose OWNER is a
    /// member — the party's for the one, the raid roster's for the other; a stranger's pet is
    /// nobody's; ungrouped is nil.
    #[test]
    fn unit_player_or_pet_in_party_reads_the_owner() {
        let mut s = UiScript::new().unwrap();
        s.set_party(two_member_party());
        s.set_unit("player", Some(unit(true, 0x10)));
        let mut alices_pet = unit(true, 0xC0FFEE);
        alices_pet.owner = 0xA11CE;
        s.set_unit("target", Some(alices_pet));
        assert_eq!(
            s.eval::<i64>(r#"return UnitPlayerOrPetInParty("target")"#)
                .unwrap(),
            1,
            "Alice's pet is in the party by its owner"
        );
        assert!(
            s.eval::<bool>(r#"return UnitPlayerOrPetInRaid("target") == nil"#)
                .unwrap(),
            "no raid: the raid twin says nil"
        );
        let mut strangers_pet = unit(true, 0xC0FFEE);
        strangers_pet.owner = 0xDEAD;
        s.set_unit("target", Some(strangers_pet));
        assert!(s
            .eval::<bool>(r#"return UnitPlayerOrPetInParty("target") == nil"#)
            .unwrap());
        // A member's own guid still answers, as UnitInParty does.
        s.set_unit("target", Some(unit(true, 0xB0B)));
        assert_eq!(
            s.eval::<i64>(r#"return UnitPlayerOrPetInParty("target")"#)
                .unwrap(),
            1
        );
        // The raid twin over the raid roster.
        s.set_party(ten_player_raid());
        let mut raiders_pet = unit(true, 0xC0FFEE);
        raiders_pet.owner = ten_player_raid().raid[0].guid;
        s.set_unit("target", Some(raiders_pet));
        assert_eq!(
            s.eval::<i64>(r#"return UnitPlayerOrPetInRaid("target")"#)
                .unwrap(),
            1
        );
        s.set_party(crate::script::PartyState::default());
        assert!(s
            .eval::<bool>(r#"return UnitPlayerOrPetInParty("target") == nil"#)
            .unwrap());
    }

    #[test]
    fn unit_in_party_matches_roster_guids() {
        let mut s = UiScript::new().unwrap();
        s.set_party(two_member_party());
        s.set_unit("player", Some(unit(true, 0x10)));
        s.set_unit("party1", Some(unit(true, 0xA11CE)));
        // The target IS Alice (guid match through an arbitrary token).
        s.set_unit("target", Some(unit(true, 0xA11CE)));
        assert_eq!(s.eval::<i64>(r#"return UnitInParty("target")"#).unwrap(), 1);
        assert_eq!(s.eval::<i64>(r#"return UnitInParty("party1")"#).unwrap(), 1);
        // A stranger's guid is nil.
        s.set_unit("target", Some(unit(true, 0xDEAD)));
        assert!(s
            .eval::<bool>(r#"return UnitInParty("target") == nil"#)
            .unwrap());
        // Ungrouped: everything is nil, the player included.
        s.set_party(crate::script::PartyState::default());
        assert!(s
            .eval::<bool>(r#"return UnitInParty("player") == nil"#)
            .unwrap());
    }

    #[test]
    fn unit_can_cooperate_needs_a_friendly_player() {
        let mut s = UiScript::new().unwrap();
        let mut friendly = unit(true, 0x30);
        friendly.is_player = true;
        friendly.reaction = 5;
        s.set_unit("target", Some(friendly.clone()));
        assert_eq!(
            s.eval::<i64>(r#"return UnitCanCooperate("player", "target")"#)
                .unwrap(),
            1
        );
        // A hostile player, and a friendly NPC, both fail the gate.
        let mut hostile = friendly.clone();
        hostile.reaction = 2;
        s.set_unit("target", Some(hostile));
        assert!(s
            .eval::<bool>(r#"return UnitCanCooperate("player", "target") == nil"#)
            .unwrap());
        let mut npc = friendly;
        npc.is_player = false;
        s.set_unit("target", Some(npc));
        assert!(s
            .eval::<bool>(r#"return UnitCanCooperate("player", "target") == nil"#)
            .unwrap());
    }

    /// Every raid-management verb queues the request it names, with its arguments in the order
    /// the reference's binding takes them — the seam the whole pane acts through.
    #[test]
    fn the_raid_verbs_queue_what_they_name() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            ConvertToRaid()
            SetRaidSubgroup(7, 3)
            SwapRaidSubgroup(7, 12)
            PromoteByName("Alice")
            PromoteToAssistant("Bob")
            DemoteAssistant("Bob")
            UninviteFromRaid(9)
            DoReadyCheck()
            RequestRaidInfo()
            "#,
        )
        .unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::ConvertToRaid,
                PartyRequest::SetSubgroup { index: 7, group: 3 },
                PartyRequest::SwapSubgroup {
                    index: 7,
                    other: 12
                },
                PartyRequest::PromoteName("Alice".into()),
                PartyRequest::AssistantLeader {
                    name: "Bob".into(),
                    grant: true
                },
                PartyRequest::AssistantLeader {
                    name: "Bob".into(),
                    grant: false
                },
                PartyRequest::UninviteRaid(9),
                PartyRequest::ReadyCheckStart,
                PartyRequest::RequestRaidInfo,
            ]
        );
        assert!(s.take_party_requests().is_empty(), "the drain empties");
    }

    /// `ConfirmReadyCheck` reads its argument for TRUTH, not for a number — the reference's No
    /// button calls it with **no argument at all** while Yes passes `1`, so "absent" has to be the
    /// not-ready answer or every declined ready check reads as accepted.
    #[test]
    fn confirm_ready_check_treats_an_absent_argument_as_not_ready() {
        let mut s = UiScript::new().unwrap();
        s.run("ConfirmReadyCheck(1) ConfirmReadyCheck() ConfirmReadyCheck(false) ConfirmReadyCheck(0)")
            .unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::ReadyCheckAnswer(true),
                PartyRequest::ReadyCheckAnswer(false),
                PartyRequest::ReadyCheckAnswer(false),
                // `0` is TRUTHY in Lua, and the binding is a truth test — so this is ready.
                PartyRequest::ReadyCheckAnswer(true),
            ]
        );
    }

    /// The saved-lockout pair: 1-based, three returns, and a miss is a plain nothing (the panel's
    /// own loop only ever indexes inside the count it just read).
    #[test]
    fn saved_instance_info_reads_the_pushed_list() {
        use crate::script::SavedInstanceInfo;
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSavedInstances()").unwrap(), 0);
        s.set_saved_instances(vec![
            SavedInstanceInfo {
                name: "Molten Core".into(),
                instance: 1234,
                reset: 86_400,
            },
            SavedInstanceInfo {
                name: "Onyxia's Lair".into(),
                instance: 77,
                reset: 3_600,
            },
        ]);
        assert_eq!(s.eval::<i64>("return GetNumSavedInstances()").unwrap(), 2);
        let (name, id, reset) = s
            .eval::<(String, i64, i64)>("return GetSavedInstanceInfo(1)")
            .unwrap();
        assert_eq!((name.as_str(), id, reset), ("Molten Core", 1234, 86_400));
        assert_eq!(
            s.eval::<String>("return GetSavedInstanceInfo(2)").unwrap(),
            "Onyxia's Lair"
        );
        for miss in ["0", "3", "-1"] {
            assert!(
                s.eval::<bool>(&format!("return GetSavedInstanceInfo({miss}) == nil"))
                    .unwrap(),
                "index {miss} is a miss"
            );
        }
        // A non-number still raises, like every other indexed getter here.
        assert!(s
            .eval::<i64>(r#"return GetSavedInstanceInfo("x")"#)
            .is_err());
    }

    /// `SetRaidRosterSelection` is purely client-side: it moves a cursor and sends nothing.
    #[test]
    fn the_raid_roster_selection_is_a_local_cursor() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetRaidRosterSelection()").unwrap(), 0);
        s.run("SetRaidRosterSelection(11)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetRaidRosterSelection()").unwrap(),
            11
        );
        assert!(
            s.take_party_requests().is_empty(),
            "nothing about a selection goes on the wire"
        );
    }

    #[test]
    fn get_raid_target_index_reads_the_fed_mark() {
        let mut s = UiScript::new().unwrap();
        let mut marked = unit(true, 0x40);
        marked.raid_target = 8;
        s.set_unit("target", Some(marked));
        assert_eq!(
            s.eval::<i64>(r#"return GetRaidTargetIndex("target")"#)
                .unwrap(),
            8
        );
        s.set_unit("target", Some(unit(true, 0x40)));
        assert!(s
            .eval::<bool>(r#"return GetRaidTargetIndex("target") == nil"#)
            .unwrap());
    }

    // ── The ready-check timeout ─────────────────────────────────────────────

    /// A raid we lead: us, Alice and Bob on the roster; the two strings the summary reads raw.
    fn raid_we_lead(s: &UiScript) -> PartyState {
        s.run(
            r#"RAID_MEMBERS_AFK = "The following players are AFK: %s"
               READY_CHECK_NO_AFK = "No players are AFK""#,
        )
        .unwrap();
        let row = |name: &str, guid: u64| crate::script::RaidMemberInfo {
            name: name.into(),
            guid,
            online: true,
            ..Default::default()
        };
        PartyState {
            members: vec![
                PartyMemberInfo {
                    name: "Alice".into(),
                    guid: 0xA11CE,
                },
                PartyMemberInfo {
                    name: "Bob".into(),
                    guid: 0xB0B,
                },
            ],
            leader_index: 0,
            leader_guid: 0x5E1F,
            own_guid: 0x5E1F,
            raid: vec![
                row("Probefour", 0x5E1F),
                row("Alice", 0xA11CE),
                row("Bob", 0xB0B),
            ],
            loot_method: "group".into(),
            master_looter: None,
            loot_threshold: 2,
        }
    }

    /// `DoReadyCheck` flags every roster member but the caller and arms 30 s; the tick is a no-op
    /// before the deadline, prints the `", "`-joined unanswered names on it, and — the only
    /// clear-to-zero — disarms, so the next tick prints nothing.
    #[test]
    fn the_leaders_check_times_out_into_the_afk_list_and_disarms() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.run("__benilla_now = 100 DoReadyCheck()").unwrap();
        assert_eq!(s.take_party_requests(), vec![PartyRequest::ReadyCheckStart]);

        s.run("__benilla_now = 129.9 CheckReadyCheckTime()")
            .unwrap();
        assert!(
            s.take_ready_check_lines().is_empty(),
            "not before the deadline"
        );
        s.run("__benilla_now = 130 CheckReadyCheckTime()").unwrap();
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["The following players are AFK: Alice, Bob".to_string()]
        );
        s.run("__benilla_now = 200 CheckReadyCheckTime()").unwrap();
        assert!(
            s.take_ready_check_lines().is_empty(),
            "disarmed by the summary"
        );
    }

    /// An answer clears its member's flag; the answer that leaves nobody pending (and online)
    /// closes the check at once (`0x4ba4d9`), well before the deadline. (A "not ready" answer
    /// does the same and prints its line first — the test below.)
    #[test]
    fn an_answer_clears_its_member_and_the_last_one_closes_the_check_at_once() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.run("__benilla_now = 100 DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        assert!(
            s.take_ready_check_lines().is_empty(),
            "Bob is still pending"
        );
        s.ready_check_answered(0xB0B, true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()],
            "nobody left pending ⇒ the summary now, not at 130"
        );
        s.run("__benilla_now = 130 CheckReadyCheckTime()").unwrap();
        assert!(s.take_ready_check_lines().is_empty());
    }

    /// A "not ready" answer prints `RAID_MEMBER_NOT_READY` for that member the moment it arrives —
    /// and clears their flag all the same, because the leader arm stores the constant 0 whatever
    /// the byte says (`0x4ba40e`). A guid the roster does
    /// not hold writes nothing.
    #[test]
    fn a_not_ready_answer_prints_its_line_and_still_clears_the_member() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.run(r#"RAID_MEMBER_NOT_READY = "%s is not ready""#)
            .unwrap();
        s.set_party(party);
        s.run("__benilla_now = 100 DoReadyCheck()").unwrap();
        s.ready_check_answered(0xDEAD, false);
        assert!(
            s.take_ready_check_lines().is_empty(),
            "an unmatched guid writes nothing"
        );
        s.ready_check_answered(0xA11CE, false);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["Alice is not ready".to_string()]
        );
        s.run("__benilla_now = 130 CheckReadyCheckTime()").unwrap();
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["The following players are AFK: Bob".to_string()],
            "Alice answered — not ready is not AFK"
        );
    }

    /// An OFFLINE member who never answers does not hold the check open (`0x4ba4a7 test byte
    /// [eax+0x18],dl`), but the summary that closes it still lists them: the worker walks the
    /// flags with no online test (`0x4bb3a8`).
    #[test]
    fn an_offline_member_never_blocks_the_close_but_is_still_listed() {
        let mut s = UiScript::new().unwrap();
        let mut party = raid_we_lead(&s);
        party
            .raid
            .iter_mut()
            .for_each(|m| m.online = m.guid != 0xB0B); // Bob is offline
        s.set_party(party);
        s.run("__benilla_now = 100 DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["The following players are AFK: Bob".to_string()],
            "Alice's answer was the last ONLINE one, so the check closed and listed offline Bob"
        );
    }

    /// The tick is leader-gated: a member's client arms the same 30 s on the open form and the
    /// four conjuncts fail on the guid compare, every frame, forever — the deadline stays armed
    /// (nothing on the member's side ever clears it) and nothing is ever printed.
    #[test]
    fn the_tick_is_inert_for_a_non_leader() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"READY_CHECK_NO_AFK = "No players are AFK""#)
            .unwrap();
        s.set_party(two_member_party()); // Alice leads; we are 0x5E1F
        s.run("__benilla_now = 100").unwrap();
        s.ready_check_request(false);
        s.run("__benilla_now = 130 CheckReadyCheckTime()").unwrap();
        assert!(s.take_ready_check_lines().is_empty());
        assert_eq!(s.model_mut().ready_check.deadline, Some(130.0));
    }

    /// `SMSG_GROUP_LIST`'s leg: a member who left took their pending flag with them, and a roster
    /// scan that finds nobody pending force-closes the check.
    #[test]
    fn a_roster_change_with_nobody_left_pending_closes_the_check() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party.clone());
        s.run("__benilla_now = 100 DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        // Bob leaves: the roster without him.
        let mut without_bob = party;
        without_bob.raid.retain(|m| m.guid != 0xB0B);
        without_bob.members.retain(|m| m.guid != 0xB0B);
        s.set_party(without_bob);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()]
        );
    }

    /// A plain party has no raid roster, so `DoReadyCheck` flags nobody — and the leader's own
    /// echo of the open form, taking the response-collection arm, closes it on the spot.
    #[test]
    fn the_leaders_own_echo_closes_an_empty_check_at_once() {
        let mut s = UiScript::new().unwrap();
        let mut party = raid_we_lead(&s);
        party.raid.clear();
        s.set_party(party);
        s.run("__benilla_now = 100 DoReadyCheck()").unwrap();
        s.ready_check_request(true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()]
        );
    }

    /// The two keys are read raw off `_G` with an empty-string default — a missing GlobalStrings
    /// prints an empty line and never raises.
    #[test]
    fn a_missing_summary_string_prints_an_empty_line() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.run("RAID_MEMBERS_AFK = nil __benilla_now = 100 DoReadyCheck()")
            .unwrap();
        s.run("__benilla_now = 130 CheckReadyCheckTime()").unwrap();
        assert_eq!(s.take_ready_check_lines(), vec![String::new()]);
    }
}
