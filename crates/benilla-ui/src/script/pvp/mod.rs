//! The **PvP + honor** surface — `TogglePVP` (decision 0646) and the vanilla honor system's
//! thirteen bindings (decision 1512): the character window's Honor tab, the inspect window's Honor
//! tab, and the rank every unit frame can read off a *foreign* player.
//!
//! ## The three seams
//!
//! The crate's usual split (decision 0068 §3 — no ECS, no wire, no DBC in here):
//!
//! - **Pushed state.** The local player's honor counters are PRIVATE descriptor fields
//!   (`PLAYER_FIELD_SESSION_KILLS` … `PLAYER_FIELD_BYTES2`), so the app decodes them and pushes a
//!   [`HonorState`] ([`UiScript::set_honor`]); the six self getters read it and nothing else.
//!   The inspect pane's numbers are **not** a descriptor: `MSG_INSPECT_HONOR_STATS` (`0x2D6`) is a
//!   request/reply pair, so they arrive separately as an [`InspectHonorData`]
//!   ([`UiScript::set_inspect_honor`]) — and *whether one is held* is exactly what
//!   `HasInspectHonorData` reports, which is the latch the reference's `OnShow` gates its request
//!   on. That is why the inspect half is an `Option`, and the self half too — but the `Option` is
//!   the **latch only**: the engine's data slots are zero-initialised BSS that every getter reads
//!   ungated, so "nothing held" answers **zeros at full width**, never nils and never a short
//!   return. The one thing an empty slot changes is `HasInspectHonorData`.
//! - **Intents.** `TogglePVP` queues a `CMSG_TOGGLE_PVP` ([`UiScript::take_pvp_toggles`]) and
//!   `RequestInspectHonorData` queues the honor query
//!   ([`UiScript::take_inspect_honor_requests`]) — both counts rather than payloads, because
//!   neither packet carries anything the VM knows: the toggle has an empty body, and the honor
//!   query's body is the *inspected player's* guid, which the app already holds
//!   ([`super::inspect`]'s target). Same reasoning as `NotifyInspect`, one argument smaller.
//!   The honor query carries a second piece of engine state with it — the **`pending` latch**
//!   (`0xb71fcc`), which refuses a second query while one is in flight; see the binding.
//! - **The unit snapshot.** A *foreign* player's rank is knowable at all because `PLAYER_BYTES_3`
//!   is a PUBLIC field — byte 3 of it is the CURRENT rank — so it rides
//!   [`UnitState::pvp_rank`](super::UnitState::pvp_rank) with the other public per-unit facts and
//!   `UnitPVPRank("target")` answers for the inspect pane exactly as the reference's does.
//!
//! ## Two ranks, and neither is the other
//!
//! - **Internal rank**, `0..=19`: the byte on the wire, and the number `GetPVPRankInfo` keys its
//!   title lookup on. `0` is "no rank"; `1..=4` are the dishonorable ranks; `5..=18` are
//!   Scout/Private through High Warlord/Grand Marshal; `19` is the racial-leader "Leader", which
//!   the server sends in `SMSG_PVP_CREDIT` and **`GetPVPRankInfo` refuses** (see below).
//! - **Visual rank**, `-4..=14`: `rank >= 5 ? rank - 4 : rank - 5` (`0x51aa31`) — what
//!   `GetPVPRankInfo` *returns* as its second value, and what FrameXML indexes the badge texture by
//!   (`"…PvPRank"..format("%02d", rankNumber)`, drawn only when it is positive).
//!
//! **The negative half runs the other way from the server's.** vmangos's `HonorMgr.cpp:991`
//! computes `visualRank = rank > 4 ? rank - 4 : rank * -1`, which gives rank 1 → −1 and rank 4 →
//! −4; the *binding* subtracts 5 instead of negating, so it gives rank 1 → **−4** and rank 4 →
//! **−1**. They genuinely disagree, and the client is the authority for what the client's binding
//! returns — [`visual_rank`] implements the binding. Nothing here reads the server's form, and a
//! reader "fixing" this back to `-rank` would be undoing a verified read of [`visual_rank`]'s own
//! bytes.
//!
//! Conflating the two scales puts Sergeant's badge on a Corporal, so [`visual_rank`] is the one
//! place the conversion happens and every caller goes through it. The trap is that both numbers
//! travel: the badge and `GetPVPRankInfo`'s second return are **visual**, while the wire —
//! `PLAYER_BYTES_3` byte 3, `GetInspectHonorData`'s `lifetimeRank`, and `SMSG_PVP_CREDIT`'s
//! `victimRank` (decision 1512 §2's second-pass correction: `HonorMgr::SendPVPCredit` sends
//! `GetRank().rank`) — is **internal** throughout, which is what lets a rank byte index a
//! `PVP_RANK_*` GlobalString with no conversion at all.
//!
//! ## Why the rank titles come out of the VM's own globals
//!
//! A rank's name is the GlobalString `PVP_RANK_<internal>_<team>` — the engine's one format string
//! (`"PVP_RANK_%d_%d"` @ `0x8445b4`), team `0` Horde and `1` Alliance (VERIFIED against the shipped
//! `FactionGroup.dbc` *and* `GlobalStrings.lua`: `PVP_RANK_5_0` is "Scout", `PVP_RANK_5_1` is
//! "Private"). The real `GlobalStrings.lua` is executed off the player's own install at boot, so
//! those keys are already sitting on `_G` when `GetPVPRankInfo` runs, and reading them there is the
//! only correct answer: a table of titles shipped in this repo would be Blizzard text we may not
//! carry (docs/METHOD.md's install-content rule) *and* wrong in every non-enUS locale. There is **no
//! `PVP_RANK_0_*`** — internal rank 0 names nothing, and both reference panes lean on that nil to
//! fall back to the `NONE` GlobalString, so a missing key answers nil here rather than an empty
//! string or a raise.
//!
//! The team digit is a **tri-state**, not a bool: `0x5efe00` answers `0` Horde, `1` Alliance, or
//! **`-1`** for a unit with no side, and −1 is not special-cased anywhere — it is formatted into
//! the key, `PVP_RANK_9_-1` matches no GlobalString, and the miss *is* the "no title" answer.
//!
//! ## `_FEMALE` is real, and the honor pane never sees it
//!
//! The `_FEMALE` twins ship in `GlobalStrings.lua`, and the engine cannot build their keys: the
//! byte sequence `FEMALE` does not occur anywhere in `WoW.exe`. It reaches them by asking
//! **FrameXML** — `0x703bf0(key, ordinal, gender)` takes a Lua path whenever `gender != 0`, calling
//! `GetText(token, gender, ordinal)`, which appends the suffix for gender 3. So the *suffix* is
//! Lua's and the *request* is the engine's, per call site:
//!
//! - **`GetPVPRankInfo` passes gender 0** (`0x51aa0f`) and takes the fast path — so the character
//!   and inspect honor panes render the **male/default title even for a female character**.
//! - **`UnitPVPName`'s title is gendered** by the subject unit (`0x612bf0`), and
//!   **`SMSG_PVP_CREDIT`'s is gendered by the LOCAL PLAYER** even though the *team* digit is the
//!   victim's (`0x625374`) — the app's chat line owns that asymmetry, through
//!   [`UiScript::pvp_rank_title`].
//!
//! So [`rank_title_ungendered`] and [`rank_title_gendered`] are two functions on purpose, and
//! `GetPVPRankInfo` calls the first. **A client that gendered the pane's title would be more
//! "correct" than the reference and would diverge from it**, which is not what we build.
//!
//! ## The counter-intuitive four
//!
//! Everything below is byte law, and every one of them looks like a bug:
//!
//! 1. `GetPVPRankProgress` **multiplies by a slightly-wrong f32 reciprocal** and does not clamp —
//!    [`rank_progress`].
//! 2. The four dishonorable ranks' visual numbers run **backwards** from the server's — see "Two
//!    ranks" above.
//! 3. `GetPVPRankInfo` **rejects rank 19**, whose GlobalString exists and which the credit line and
//!    `UnitPVPName` both render.
//! 4. `GetPVPLifetimeStats` **suppresses a highest-lifetime rank below 5**, reporting `0`.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{number_arg, string_arg};
use super::unit::{check_unit_token, is_civilian_kill};
use super::Model;

/// The local player's honor snapshot — every number the character window's Honor tab shows, as the
/// app decoded it from the PRIVATE honor descriptor fields (decision 1512 §1). EXACT shape the app
/// feed is written against; do not rename.
///
/// The kill counters are `u16` because their fields are `TWO_SHORT` descriptors read as halves
/// (`PLAYER_FIELD_SESSION_KILLS` low = honorable, high = dishonorable); the honor totals are the
/// whole-dword `*_CONTRIBUTION` fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HonorState {
    /// `PLAYER_FIELD_SESSION_KILLS` low half — honorable kills this session.
    pub session_hk: u16,
    /// `PLAYER_FIELD_SESSION_KILLS` high half — dishonorable kills this session.
    pub session_dk: u16,
    /// `PLAYER_FIELD_YESTERDAY_KILLS` halves.
    pub yesterday_hk: u16,
    pub yesterday_dk: u16,
    /// `PLAYER_FIELD_YESTERDAY_CONTRIBUTION` — yesterday's honor.
    pub yesterday_honor: u32,
    /// `PLAYER_FIELD_THIS_WEEK_KILLS` low half. There is no this-week DK getter: the reference
    /// destructures exactly two values from `GetPVPThisWeekStats`.
    pub this_week_hk: u16,
    /// `PLAYER_FIELD_THIS_WEEK_CONTRIBUTION`.
    pub this_week_honor: u32,
    /// `PLAYER_FIELD_LAST_WEEK_KILLS` halves.
    pub last_week_hk: u16,
    pub last_week_dk: u16,
    /// `PLAYER_FIELD_LAST_WEEK_CONTRIBUTION`.
    pub last_week_honor: u32,
    /// `PLAYER_FIELD_LAST_WEEK_RANK` — last week's **standing** (the ladder position), which is
    /// `GetPVPLastWeekStats`'s fourth return and is not a rank at all.
    pub last_week_standing: u32,
    /// `PLAYER_FIELD_LIFETIME_HONORBALE_KILLS` / `…_DISHONORBALE_KILLS` (the server's spellings).
    pub lifetime_hk: u32,
    pub lifetime_dk: u32,
    /// `PLAYER_FIELD_BYTES` byte 3 — the HIGHEST LIFETIME rank, internal scale. PRIVATE, and
    /// already decoded elsewhere in the app as the honor-rank byte.
    ///
    /// **Carried raw.** `GetPVPLifetimeStats` suppresses a value below 5 to `0` (`0x51a843`), and
    /// that suppression is the *binding's*, not the feed's — push the true byte and let the
    /// binding hide it, because the same field read through any other door is not hidden.
    pub highest_rank: u8,
    /// `PLAYER_BYTES_3` byte 3 — the **current** rank, internal scale, `0` = no rank. The same
    /// PUBLIC byte that reaches every observer as [`UnitState::pvp_rank`](super::UnitState).
    pub rank: u8,
    /// `PLAYER_FIELD_BYTES2` byte 0 — progress through the current rank, `0..=255`. Carried as the
    /// raw byte: the fraction is [`rank_progress`]'s business, not the feed's.
    pub rank_bar: u8,
}

/// One `MSG_INSPECT_HONOR_STATS` reply (decision 1512 §2), as the app decoded its 50-byte body.
///
/// Fourteen numbers and the guid they were about. **There is no per-period DK here** — the reply
/// carries a session HK/DK pair and then bare HK counts for yesterday/last week/this week (the
/// three `unknownOld` shorts between them are written as zero by the server), which is why
/// `GetInspectHonorData` returns twelve values while the self getters together return more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InspectHonorData {
    /// The player guid the reply was for — the app's key for "is this still the player we are
    /// looking at?". Never returned to Lua; the reference's pane is keyed by its own unit token.
    pub guid: u64,
    pub session_hk: u16,
    pub session_dk: u16,
    pub yesterday_hk: u16,
    pub yesterday_honor: u32,
    pub this_week_hk: u16,
    pub this_week_honor: u32,
    pub last_week_hk: u16,
    pub last_week_honor: u32,
    /// The ladder position, the reply's `lastWeekRank` — `GetInspectHonorData`'s ninth return.
    pub last_week_standing: u32,
    pub lifetime_hk: u32,
    pub lifetime_dk: u32,
    /// The reply's `highestRank` byte — `GetInspectHonorData`'s twelfth return, which the
    /// reference names `lifetimeRank`. Internal scale, like every other rank here.
    ///
    /// **Passed through, where its self-side twin is suppressed below 5.** This pins
    /// `GetPVPLifetimeStats`'s `cmp al,5; jb` at `0x51a843` and records `0x4c9620` as twelve plain
    /// pushes with no such gate, so this one is not filtered here. That is a *recorded absence*
    /// rather than an asserted negative: if a re-read of `0x4c9620` ever turns up the same
    /// compare, this field's getter is the one place it belongs — and nothing else should be
    /// harmonised to match it in the meantime.
    pub highest_rank: u8,
    /// The reply's trailing `rankBar` byte — read only by `GetInspectPVPRankProgress`.
    pub rank_bar: u8,
}

impl super::UiScript {
    /// Drain the PvP-flag toggles queued since the last call — each one is a `CMSG_TOGGLE_PVP`.
    /// A count rather than a payload: the packet is empty, so two toggles in a frame are two
    /// sends, not one collapsed intent.
    pub fn take_pvp_toggles(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().pvp_toggles)
    }

    /// Queue a toggle from the app side — the `/pvp` slash command. In the reference that slash
    /// handler *is* Lua (a one-liner over `TogglePVP`); benilla parses slash lines in Rust, so the
    /// same intent enters the same queue here rather than through the global ([`super::duel`]'s
    /// `queue_duel_request` reasoning, verbatim).
    pub fn queue_pvp_toggle(&mut self) {
        self.model_mut().pvp_toggles += 1;
    }

    /// Push (or clear, with `None`) the local player's honor snapshot — the six self getters'
    /// only source. Pushed on the honor descriptor edge, not per frame; it fires no event of its
    /// own, because the events the reference's pane listens for (`PLAYER_PVP_KILLS_CHANGED`,
    /// `PLAYER_PVP_RANK_CHANGED`) are the app's to fire alongside the push.
    pub fn set_honor(&mut self, honor: Option<HonorState>) {
        self.model_mut().honor = honor;
    }

    /// Push (or clear, with `None`) the last `MSG_INSPECT_HONOR_STATS` reply.
    ///
    /// **The presence of this is `HasInspectHonorData`'s answer**, so clearing it is as
    /// load-bearing as pushing it: the app clears when it drops the inspected player
    /// (`ClearInspectPlayer`), which is what makes the reference's `OnShow` re-request for the
    /// next one instead of painting the previous player's numbers. Pair it with the pane's
    /// `INSPECT_HONOR_UPDATE` event.
    ///
    /// **Both arms clear the in-flight latch**, exactly as the engine's two writers do: a reply
    /// landing zeroes `pending` at `0x4c6f4c`, and re-keying or clearing the slot zeroes it at
    /// `0x4c6f9d`. So an invalidation also says "whatever was in flight is no longer wanted",
    /// which is what lets the *next* `RequestInspectHonorData` through.
    pub fn set_inspect_honor(&mut self, data: Option<InspectHonorData>) {
        let mut model = self.model_mut();
        model.inspect_honor = data;
        model.inspect_honor_pending = false;
    }

    /// Drain the `RequestInspectHonorData` calls queued since the last call — each one is a
    /// `MSG_INSPECT_HONOR_STATS` send carrying the app's current inspect target guid.
    ///
    /// A count, for [`Self::take_pvp_toggles`]'s reason: the binding takes no argument, so there
    /// is no payload to carry, and the request the *app* addresses is its own inspect target. It
    /// drains **0 or 1** and never more, because the binding's `pending` latch (`0x4c80b6`)
    /// refuses a second query until [`Self::set_inspect_honor`] resolves the first — a count kept
    /// rather than a `bool` because that latch is the engine's, not this signature's, and the app
    /// loop over it is identical either way.
    pub fn take_inspect_honor_requests(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().inspect_honor_requests)
    }

    /// A PvP rank's localized title for the app's `SMSG_PVP_CREDIT` chat line — the
    /// `PVP_RANK_<rank>_<team>` GlobalString off the VM's own globals (`team` `0` Horde, `1`
    /// Alliance), **gendered**: the `_FEMALE` twin when `female`, falling back to the base key.
    /// A missing or empty key is `None`, never an empty string.
    ///
    /// **Gendered is right *here* and wrong in the pane**, which is the whole reason this is not
    /// simply `GetPVPRankInfo`'s lookup: the credit formatter resolves through `0x612bf0`
    /// while `GetPVPRankInfo` passes gender 0. It shares [`rank_title_gendered`]
    /// with `UnitPVPName`, the engine's other gendered caller.
    ///
    /// The number `SMSG_PVP_CREDIT` carries is the **INTERNAL** rank — the GlobalString index
    /// itself, which is precisely why it can be passed straight in — and NOT the visual rank the
    /// badge indexes; see the module doc's "Two ranks". Unlike the pane's binding this path applies
    /// **no range check**, so rank 19 legitimately names "Leader" here.
    ///
    /// Two facts the caller owns, neither expressible in this signature: the gender is the
    /// **local player's** while the team is the **victim's**
    /// (`0x625374` hands `0x612bf0` the entry `this`, still the local player), and a victim whose
    /// team resolves to **−1 emits no line at all** (`0x625321 js` bails the whole formatter) —
    /// which is a `u8` here precisely so that decision cannot be smuggled into a key that misses.
    pub fn pvp_rank_title(&self, rank: u8, team: u8, female: bool) -> Option<String> {
        rank_title_gendered(self.lua(), i64::from(rank), i64::from(team), female)
    }
}

/// Internal rank → **visual** rank, `0x51aa31`'s two-legged subtraction:
///
/// ```text
/// 51aa31  cmp edi,0x5
/// 51aa36  jge 0x51aa55        ; SIGNED
/// 51aa38  edi += -5           ; ranks 1..4  -> -4 .. -1
/// 51aa55  edi += -4           ; ranks 5..18 ->  1 .. 14
/// ```
///
/// Defined on `1..=18` only, because that is the whole domain that reaches it: `GetPVPRankInfo`
/// range-gates first and answers `0` for everything outside, so there is no "rank 0 maps to 0" case
/// to write here — the badge stays off an unranked player through the gate, not through this.
///
/// The negative leg **subtracts 5**; it does not negate. See the module doc — the server's
/// `visualRank` negates and the two therefore disagree for exactly ranks 1..4.
fn visual_rank(rank: i64) -> i64 {
    if rank >= 5 {
        rank - 4
    } else {
        rank - 5
    }
}

/// The f32 constant at `0x8026c8` — the four bytes `81 80 80 3b` — that `GetPVPRankProgress` and
/// `GetInspectPVPRankProgress` multiply the rank-bar byte by. It is the f32 *nearest* `1/255`
/// (`0.003921568859368563`, against `1/255`'s `0.00392156862745098`) and it is **not** `1/255`.
const RANK_BAR_SCALE: f64 = f32::from_bits(0x3B80_8081) as f64;

/// The rank progress bar's byte → the fraction the reference feeds straight to a `StatusBar` whose
/// `minValue`/`maxValue` are `0`/`1`.
///
/// **A multiply by [`RANK_BAR_SCALE`], not a division by 255, and no clamp** — both halves are
/// verified at the bytes, correcting the natural reading, and both are deliberate:
///
/// - `0x51aace` is `fild DWORD` then `fmul DWORD PTR ds:0x8026c8`. Under the client's PC_53 x87
///   invariant `(double)b * (double)K` and `(double)b / 255.0` differ for **255 of the 256** byte
///   values (all but 0), so `bar as f64 / 255.0` is bit-wrong on every non-zero input. A byte of
///   255 comes out as `1.0000000091389835`, *above* 1.0, and a byte of 51 is not exactly 0.2.
/// - There is no `fcom`, no min/max and no compare anywhere in either function. The value is
///   `byte * K` and nothing else. Clamping would be tidier and would be a divergence — and it
///   would hide precisely the >1.0 overshoot that proves the multiply.
///
/// A full bar therefore hands `StatusBar:SetValue` a number a hair over its `maxValue`. That is
/// what the real client does; how our `StatusBar` treats an out-of-range value is that widget's
/// business, not this binding's.
fn rank_progress(bar: u8) -> f64 {
    f64::from(bar) * RANK_BAR_SCALE
}

/// The local player's honor snapshot, or `None` before the first push.
fn honor(lua: &Lua) -> Option<HonorState> {
    lua.app_data_ref::<Model>().expect("model app_data").honor
}

/// The last inspect-honor reply, or `None` when none is held.
fn inspect_honor(lua: &Lua) -> Option<InspectHonorData> {
    lua.app_data_ref::<Model>()
        .expect("model app_data")
        .inspect_honor
}

/// A GlobalString off the VM's own globals, `None` when it is absent **or empty** — the
/// no-install / bare-VM path. (`GetPVPRankInfo` tests both explicitly: `0x51aa1c` NULL,
/// `0x51aa20` empty first byte.)
fn global_string(lua: &Lua, key: &str) -> Option<String> {
    lua.globals()
        .get::<Option<String>>(key)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// A unit's PvP **team digit** — `0x5efe00`'s tri-state: `0` Horde, `1` Alliance, `-1` for a unit
/// whose race resolves to no side.
///
/// The engine walks race → `ChrRaces` → `FactionTemplate` and tests `[rec+0xc]`'s factionGroupMask
/// `& 4` → 0, `& 2` → 1, else −1. This crate holds no DBC (decision 0068 §3), so the walk is the
/// app's and the answer arrives pre-resolved as [`UnitState::pvp_team`](super::UnitState).
///
/// **It is not [`UnitState::faction_group`](super::UnitState), which this used to read.** That
/// field is `UnitFactionGroup`'s — the unit's LIVE `UNIT_FIELD_FACTIONTEMPLATE` — and the two
/// part company the moment anything moves a unit off its racial faction. A vmangos GM is forced
/// to template 35 (group mask 0), so every rank title in the Honor tab read `NONE` for a Grand
/// Marshal while the 1.12 client on the same server read "Grand Marshal" off his unchanged race:
/// report B378, decision 2227.
///
/// −1 needs no special case: it formats into the key, and `PVP_RANK_9_-1` matches no GlobalString.
fn team_of(u: &super::UnitState) -> i64 {
    i64::from(u.pvp_team)
}

/// `GetPVPRankInfo`'s **second argument** → the team digit, the engine's three-way dispatch
/// (`0x51a98c` / `0x51a9af` / `0x51a9c8`, converging on `0x51a9ee mov ebx,eax`):
///
/// | argument | team |
/// |---|---|
/// | a **number** | the number itself, truncated toward zero — no unit is resolved at all |
/// | a **string** | that unit token's [`team_of`], or **0** if the token names nothing or names a non-player |
/// | absent / anything else | the local player's [`team_of`], or **0** if there is no player |
///
/// The 0 on the two "not found" edges is the team register's initial value, not a decision — so a
/// rank asked for on a client with no player snapshot is named off the HORDE list. That is the
/// engine's behaviour and it is deliberately reproduced; a unit that *is* found but has no side
/// answers −1 instead, and −1 misses the key ([`team_of`]).
///
/// An unrecognised token raises `Unknown unit name`, as `0x515970` does for every unit binding.
fn team_arg(lua: &Lua, v: Value) -> mlua::Result<i64> {
    // `lua_isnumber` before `lua_isstring`, which is what puts a numeric string on the number arm.
    if let Some(n) = lua.coerce_number(v.clone())? {
        return Ok(i64::from(n as i64 as i32));
    }
    let Some(token) = lua.coerce_string(v)? else {
        // Absent, boolean, table — the local-player preamble.
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        return Ok(model.unit("player").map_or(0, team_of));
    };
    let token = Some(token.to_str()?.to_owned());
    check_unit_token(&token)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    Ok(token
        .as_ref()
        .and_then(|t| model.unit(t))
        // `0x51a9af`'s `shr edx,4; test dl,1` player gate. Applied here, unlike in `UnitPVPRank`
        // (see there), because on this arm nothing else can tell a creature from a player: a
        // creature carries a race byte and a faction template like anything else, and the two
        // misses are different numbers anyway (this gate's 0 against `0x5efe00`'s −1) — it is
        // this gate the engine reaches first. The cost is the mirror-image miss — an un-enriched player snapshot reads as a non-player — which
        // lands on a different title rather than on none, and this arm is addon surface: neither
        // pane passes a second argument at all.
        .filter(|u| u.is_player)
        .map_or(0, team_of))
}

/// A rank's localized title, read out of the VM's own globals (see the module doc):
/// `PVP_RANK_<internal>_<team>`, **ungendered** — `0x703bf0(key, -1, 0)`, the fast path that never
/// enters Lua.
///
/// This is `GetPVPRankInfo`'s lookup (`0x51aa0f push 0`), and the reason the honor panes show a
/// male/default title to a female character. Do not "fix" it; see the module doc's `_FEMALE`
/// section.
fn rank_title_ungendered(lua: &Lua, rank: i64, team: i64) -> Option<String> {
    global_string(lua, &format!("PVP_RANK_{rank}_{team}"))
}

/// The same title, **gendered** by the subject — `0x612bf0`: it hands the gender selector (2 male /
/// 3 female) to `0x703bf0`, which routes into FrameXML's `GetText` and gets `_FEMALE` appended for
/// 3, then **retries ungendered** when that misses (`0x612c2d`).
///
/// Two callers, both verified to be gendered: `UnitPVPName`'s decoration (by the *subject unit*)
/// and [`UiScript::pvp_rank_title`], the app's `SMSG_PVP_CREDIT` line (by the *local player*, even
/// though the team digit is the victim's — `0x625374`).
fn rank_title_gendered(lua: &Lua, rank: i64, team: i64, female: bool) -> Option<String> {
    female
        .then(|| global_string(lua, &format!("PVP_RANK_{rank}_{team}_FEMALE")))
        .flatten()
        .or_else(|| rank_title_ungendered(lua, rank, team))
}

/// Fill a **two-`%s` C format string** — the shape `snprintf(buf, size, GlobalString(...), a, b)`
/// needs, and no more than that shape: the engine pushes exactly two varargs (`0x6093f2 add
/// esp,0x14`), so any further specifier in a locale's template would read uninitialised stack
/// there and is simply dropped here.
///
/// It substitutes rather than hardcoding `"{a} {b}"` because the template is *install data*: enUS
/// ships `UNIT_PVP_NAME = "%s %s"`, and a locale that punctuates or spaces it differently must come
/// out differently. `%%` is a literal `%`; anything else after a `%` is passed through untouched.
fn format_two_strings(fmt: &str, a: &str, b: &str) -> String {
    let mut out = String::with_capacity(fmt.len() + a.len() + b.len());
    let mut args = [a, b].into_iter();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push_str(args.next().unwrap_or("")),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// `0x609370` — the name builder behind `UnitPVPName`, called with its flag set. `name` is the
/// unit's plain `UnitName`; see the binding for the three legs and the two divergences.
fn pvp_name(lua: &Lua, u: &super::UnitState, name: &str, player_level: u32) -> String {
    // Leg A: a player with a rank. The title is gendered by THIS unit and range-unchecked.
    if u.is_player && u.pvp_rank != 0 {
        let title = rank_title_gendered(lua, i64::from(u.pvp_rank), team_of(u), u.sex == 3);
        if let (Some(fmt), Some(title)) = (global_string(lua, "UNIT_PVP_NAME"), title) {
            let mut out = format_two_strings(&fmt, &title, name);
            // Leg A′: the city-protector medal, on its own line, ungendered (`0x60941d push 0`).
            if u.pvp_medal != 0 {
                if let Some(medal) = global_string(lua, &format!("PVP_MEDAL{}", u.pvp_medal)) {
                    out.push('\n');
                    out.push_str(&medal);
                }
            }
            return out;
        }
        return name.to_owned();
    }
    // Leg B: the dishonorable-kill warning, on the same predicate the tooltip's CIVILIAN line uses.
    if is_civilian_kill(u, player_level) {
        if let Some(civilian) = global_string(lua, "PVP_RANK_CIVILIAN") {
            return format!("{civilian} {name}");
        }
    }
    // Leg C.
    name.to_owned()
}

/// Register the PvP + honor globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // TogglePVP() — /pvp and the unit popup's PvP row. Takes no argument in 1.12: the *state*
    // form of the opcode (a one-byte body) has no binding, so there is nothing to pass. The
    // reference registers it at `0x48d700` and calls it from exactly one place in the whole
    // shipped 1.12 UI: `SlashCmdList["PVP"]` (ChatFrame.lua); benilla's popup row is a deliberate
    // second caller — decision 0646 §3.
    g.set(
        "TogglePVP",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pvp_toggles += 1;
            Ok(())
        })?,
    )?;

    // ── The self getters ─────────────────────────────────────────────────────────────────────
    // Every one of them answers at full arity on every path, zeros included: the reference
    // destructures the return straight into `format()` arguments, so a short return would leave a
    // pane painting nils where the real client paints `0`. Before the first push (`None`) they
    // therefore read as a zeroed snapshot — which is also exactly what a fresh character has.

    // GetPVPSessionStats() → hk, dk.
    g.set(
        "GetPVPSessionStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            Ok((i64::from(h.session_hk), i64::from(h.session_dk)))
        })?,
    )?;

    // GetPVPYesterdayStats() → hk, dk, contribution (THREE — the weekly pair below is different).
    g.set(
        "GetPVPYesterdayStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            Ok((
                i64::from(h.yesterday_hk),
                i64::from(h.yesterday_dk),
                i64::from(h.yesterday_honor),
            ))
        })?,
    )?;

    // GetPVPThisWeekStats() → hk, contribution. **Two, not three**: the reference destructures
    // `local hk, contribution = GetPVPThisWeekStats()` and shows no this-week DK column at all.
    g.set(
        "GetPVPThisWeekStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            Ok((i64::from(h.this_week_hk), i64::from(h.this_week_honor)))
        })?,
    )?;

    // GetPVPLastWeekStats() → hk, dk, contribution, standing. **Four**: the fourth is
    // `PLAYER_FIELD_LAST_WEEK_RANK`, which is the ladder STANDING, not a rank — the pane prints it
    // through `format(PVP_RANK_LAST_WEEK, standing)` and never looks a title up for it.
    g.set(
        "GetPVPLastWeekStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            Ok((
                i64::from(h.last_week_hk),
                i64::from(h.last_week_dk),
                i64::from(h.last_week_honor),
                i64::from(h.last_week_standing),
            ))
        })?,
    )?;

    // GetPVPLifetimeStats() → hk, dk, highestRank. The third is the HIGHEST LIFETIME rank
    // (`PLAYER_FIELD_BYTES` byte 3), internal scale — not the current one `UnitPVPRank` answers.
    //
    // **A highest rank below 5 is reported as 0, not as itself** (`0x51a843 cmp al,5; jb`,
    // UNSIGNED, falling into the same `0.0` push the absent-player tail uses). Ranks 1..4 are the
    // dishonorable titles, and this is why the reference pane shows NONE for a character whose
    // best was one of them: the suppression is here, in the binding, not in the pane's Lua.
    // Suppressed to 0 — not to nil — so the pane's `GetPVPRankInfo(highestRank)` still gets a
    // number.
    g.set(
        "GetPVPLifetimeStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            let highest = if h.highest_rank >= 5 {
                h.highest_rank
            } else {
                0
            };
            Ok((
                i64::from(h.lifetime_hk),
                i64::from(h.lifetime_dk),
                i64::from(highest),
            ))
        })?,
    )?;

    // GetPVPRankProgress() → the bar fraction. A multiply, and unclamped — see `rank_progress`,
    // which is the whole law of this binding. Absent player → `0.0`, still one value.
    g.set(
        "GetPVPRankProgress",
        lua.create_function(|lua, ()| Ok(rank_progress(honor(lua).unwrap_or_default().rank_bar)))?,
    )?;

    // GetPVPRankInfo(rank [, unit]) → rankName, rankNumber — the title off the VM's globals
    // (module doc) and the VISUAL rank. **Always two values**: every failure edge answers
    // `nil, 0`, and that nil is what both reference panes branch on to substitute the `NONE`
    // GlobalString. The first argument is the INTERNAL rank, which is what `UnitPVPRank` and the
    // `*LifetimeStats`/`InspectHonorData` rank returns all carry.
    //
    // Three things the name gives no hint of, all `0x51a930`:
    //
    //  * **The accepted range is [1, 18], SIGNED** (`0x51a9f0 cmp edi,1; jl` / `0x51a9f5 cmp
    //    edi,0x12; jg`). Rank 0 fails it, and so does **rank 19** — the racial-leader "Leader",
    //    whose `PVP_RANK_19_*` GlobalStrings exist and which the server really does send in
    //    `SMSG_PVP_CREDIT`. The pane simply cannot name it. That rejection is why the engine's own
    //    badge table has fifteen entries while FrameXML only ever indexes fourteen: rank 19 →
    //    `PvPRank15` is reachable from the world-text kill toast (`0x6c7f10`, badge index
    //    `rank − 5`) and from nowhere on this path.
    //  * **The second argument exists**, and it is a three-way dispatch, not a unit token: a
    //    NUMBER is the team digit itself (no unit resolution at all), a STRING is a unit token, and
    //    absent-or-anything-else means the local player. Lua's own `isnumber` accepts a numeric
    //    string, so `"1"` takes the number arm and `"player"` the token arm — `coerce_number`
    //    first is that same order.
    //  * **A team the engine cannot resolve is 0, not a failure.** An unresolvable token, a
    //    non-player unit, or no local player leaves the team register at its initial 0 — the HORDE
    //    list — while a unit that resolves but has no *side* gets −1 and misses the key. The two
    //    are different answers and this reproduces both.
    g.set(
        "GetPVPRankInfo",
        lua.create_function(|lua, (rank, unit): (Value, Value)| {
            let rank = i64::from(number_arg(
                lua,
                rank,
                "Usage: GetPVPRankInfo(rank [, unit])",
            )?);
            let team = team_arg(lua, unit)?;
            // The range gate first: outside [1, 18] no key is even built.
            let name = if (1..=18).contains(&rank) {
                rank_title_ungendered(lua, rank, team)
            } else {
                None
            };
            let Some(name) = name else {
                return Ok(MultiValue::from_vec(vec![Value::Nil, Value::Integer(0)]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                Value::Integer(visual_rank(rank)),
            ]))
        })?,
    )?;

    // UnitPVPRank(unit) → the unit's CURRENT rank, internal scale, 0 for no rank.
    //
    // It answers for a FOREIGN unit on purpose: `0x51a8a0` resolves the named token and reads
    // **that object's own** `[+0xe68]+0x1f` — no cache, no local-player special case — and the byte
    // lives in `PLAYER_BYTES_3`, a PUBLIC descriptor field the server streams to every observer.
    // That is why the reference's inspect pane can call `UnitPVPRank("target")` at all, and why a
    // foreign rank never comes from the inspect reply.
    //
    // A creature, a non-player, and an unresolvable token all read **0 — a number, never nil**
    // (`0x51a916`), so a genuine rank-0 player and an unknown token are indistinguishable to
    // script. A **non-string argument raises** (`0x51a8ac`), which is why this takes its token
    // through `string_arg` rather than an `Option`: `UnitPVPRank()` abandons the caller's
    // statement, it does not answer 0.
    //
    // **The engine's player gate (`0x51a8e1 shr edx,4; test dl,1`) is deliberately NOT re-applied
    // here**, and the reason is a real one rather than laziness: the engine gates on the object's
    // TYPE MASK, and our nearest equivalent — `UnitState::is_player` — is not that. It is filled by
    // the app's *guid-keyed enrichment* step, one hop after the descriptor snapshot the rank byte
    // itself rides on, so a snapshot pushed without enrichment would read as a non-player and
    // silently zero a real player's rank in a pane that repaints every event. The creature case
    // needs no gate anyway: a creature has no PLAYER descriptor block, so the byte the app decodes
    // is already 0 — the reference's own answer for one, by its own path.
    g.set(
        "UnitPVPRank",
        lua.create_function(|lua, token: Value| {
            let token = Some(string_arg(lua, token, "Usage: UnitPVPRank(\"unit\")")?);
            check_unit_token(&token)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(token
                .as_ref()
                .and_then(|t| model.unit(t))
                .map_or(0i64, |u| i64::from(u.pvp_rank)))
        })?,
    )?;

    // UnitPVPName(unit) → the unit's name, rank-title-decorated — `0x5172b0` into the builder
    // `0x609370`, whose three legs are:
    //
    //   A  a PLAYER with a non-zero rank byte → snprintf(GlobalString("UNIT_PVP_NAME"),
    //      rankName, plainName). The `add esp,0x14` at `0x6093f2` proves exactly two varargs, and
    //      the shipped template is "%s %s" — RANK FIRST. The title is `0x5efe60` → `0x612bf0`, so
    //      it is GENDERED by this unit (unlike the pane's, module doc) and has NO range check, so
    //      a rank-19 racial leader really does render as "Leader Whoever".
    //   A′ …and if PLAYER_BYTES_3 byte 2 (the city-protector title) is non-zero, a second line:
    //      "\n" + GlobalString("PVP_MEDAL<n>"), ungendered.
    //   B  otherwise, a unit that passes the CIVILIAN predicate `0x612550` →
    //      GlobalString("PVP_RANK_CIVILIAN") + " " + plainName. Reached by creatures: leg A takes
    //      every ranked player first, and our snapshot only ever flags creatures civilian.
    //   C  otherwise the plain name.
    //
    // Two edges answer **nil**: no snapshot for the token, and a snapshot whose name has not
    // resolved. The engine distinguishes a third — a token resolving to GUID 0 returns one value
    // having pushed nothing (`0x517343`), i.e. non-nil and unspecified. That is an INFERRED
    // reading of a `luaD_poscall` detail, not a fact about this function, and reproducing an
    // unspecified value is not something a client can honestly do: we answer nil there too.
    //
    // The no-install divergence, stated rather than hidden: with `UNIT_PVP_NAME` or the rank title
    // missing from `_G`, the engine would snprintf through an empty format and hand back an empty
    // string. We answer the plain name instead — the decoration is install data, and without the
    // data there is no decoration.
    g.set(
        "UnitPVPName",
        lua.create_function(|lua, token: Value| {
            let token = Some(string_arg(lua, token, "Usage: UnitPVPName(\"unit\")")?);
            check_unit_token(&token)?;
            let (unit, player_level) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                (
                    token.as_ref().and_then(|t| model.unit(t)).cloned(),
                    model.player_req.level,
                )
            };
            let Some(u) = unit else {
                return Ok(Value::Nil);
            };
            let Some(name) = u.name.clone() else {
                return Ok(Value::Nil);
            };
            let decorated = pvp_name(lua, &u, &name, player_level);
            Ok(Value::String(lua.create_string(&decorated)?))
        })?,
    )?;

    // ── The inspect half ─────────────────────────────────────────────────────────────────────

    // RequestInspectHonorData() — the reference's `InspectHonorFrame_OnShow` calls it iff
    // `HasInspectHonorData()` is false. Queues the intent; the app pairs it with the guid it is
    // already inspecting and sends `MSG_INSPECT_HONOR_STATS` (see `NotifyInspect`, which is the
    // same shape with a token to carry). Zero Lua return values on every path (`0x4c9610`).
    //
    // **`0x4c80a0` has three silent gates**, and they are the binding's, not the pane's: bail if
    // data is already held (`0x4c80a6`), bail if a query is already in flight (`0x4c80b6`), bail
    // if the target guid is zero (`0x4c80c2`). Two of the three are ours to keep:
    //
    //  * `hasData` — modelled here even though the reference's own `OnShow` tests it first,
    //    because it is the engine that refuses; a pane (or addon) that asks anyway must get the
    //    engine's silence, not a second query.
    //  * `pending` — modelled here, and this is the one that earns its keep: it is what stops a
    //    pane shown/hidden/shown before the reply lands sending duplicates.
    //  * the guid — **not** modelled here and it cannot be: this crate never sees a guid
    //    (decision 0068 §3). The app drops a queued request with no inspect target, which is the
    //    same outcome one seam further out. The cost is that our `pending` latches on *queue*
    //    where the engine's latches on *send*, so a request the app drops leaves the latch set
    //    until the next `set_inspect_honor` — which the app's own `ClearInspectPlayer` path
    //    calls, exactly as `0x4c6f70` clears the engine's.
    g.set(
        "RequestInspectHonorData",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.inspect_honor.is_some() || model.inspect_honor_pending {
                return Ok(());
            }
            model.inspect_honor_requests += 1;
            model.inspect_honor_pending = true;
            Ok(())
        })?,
    )?;

    // HasInspectHonorData() → 1/nil. The latch the request above is gated on: true exactly while a
    // reply is held, so dropping the inspected player (which clears the push) makes the next
    // `OnShow` re-request instead of repainting the last player's numbers.
    g.set(
        "HasInspectHonorData",
        lua.create_function(|lua, ()| {
            Ok(if inspect_honor(lua).is_some() {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetInspectHonorData() → the reference's twelve, in its own order:
    //   sessionHK, sessionDK, yesterdayHK, yesterdayHonor, thisweekHK, thisweekHonor,
    //   lastweekHK, lastweekHonor, lastweekStanding, lifetimeHK, lifetimeDK, lifetimeRank
    // Note there is no per-period DK past the session pair — the reply carries yesterday/last
    // week/this week DK halves (vmangos's `unknownOld1/2/3`) and the client surfaces none of them.
    //
    // **Twelve on every path, zeros included.** `0x4c9620` is UNGATED — it never consults the
    // has-data flag; it reads sixteen zero-initialised BSS globals, so a client with no reply held
    // answers twelve zeros, not nils and not a short return. (The engine's slots are also never
    // *cleared*: re-keying the cache zeroes the two flags and leaves the data, so the real client
    // can answer a previous target's numbers here while `HasInspectHonorData` says no. We cannot —
    // our data and our latch are one `Option` — and that is the one place this pair diverges: it
    // needs the app to own the flag separately, which is a seam change, not a fix here.)
    g.set(
        "GetInspectHonorData",
        lua.create_function(|lua, ()| {
            let d = inspect_honor(lua).unwrap_or_default();
            let out = vec![
                Value::Integer(i64::from(d.session_hk)),
                Value::Integer(i64::from(d.session_dk)),
                Value::Integer(i64::from(d.yesterday_hk)),
                Value::Integer(i64::from(d.yesterday_honor)),
                Value::Integer(i64::from(d.this_week_hk)),
                Value::Integer(i64::from(d.this_week_honor)),
                Value::Integer(i64::from(d.last_week_hk)),
                Value::Integer(i64::from(d.last_week_honor)),
                Value::Integer(i64::from(d.last_week_standing)),
                Value::Integer(i64::from(d.lifetime_hk)),
                Value::Integer(i64::from(d.lifetime_dk)),
                Value::Integer(i64::from(d.highest_rank)),
            ];
            debug_assert_eq!(out.len(), 12, "the tuple is twelve wide on every path");
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetInspectPVPRankProgress() → the same bar fraction as GetPVPRankProgress, off the reply's
    // trailing `rankBar` byte. `0x51ab00` is the same three-instruction kernel over the cached
    // byte — the `fild`/`fmul ds:0x8026c8`/`fstp` at `0x51ab04` is byte-identical to `0x51aace` —
    // with no object lookup, no gate and no clamp, so it too reads 0.0 from an empty slot rather
    // than nil ([`rank_progress`]).
    g.set(
        "GetInspectPVPRankProgress",
        lua.create_function(|lua, ()| {
            Ok(rank_progress(
                inspect_honor(lua).unwrap_or_default().rank_bar,
            ))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests;
