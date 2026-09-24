//! The `Unit*` bindings. The app pushes each token's [`UnitState`] every frame through
//! [`UiScript::set_unit`], and the globals read that plain data, which keeps this crate free of
//! the ECS. Every predicate answers the number `1` or nil, never a Lua boolean.

use mlua::Lua;

use super::Model;

/// A selection ask from Lua, queued in call order for the app to resolve and commit. The reference
/// routes `TargetUnit`, `AssistUnit` and `TargetLastEnemy` through one helper (`0x489a40`: commit,
/// else the group roster, else a silent return), and a macro can observe their order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionRequest {
    /// `TargetUnit(unit)` (`0x4899d0`): the unit the token names.
    Unit(String),
    /// `AssistUnit(unit)` (`0x489b80`): the token's own `UNIT_FIELD_TARGET`; a token with no
    /// target is a silent no-op that sends nothing.
    Assist(String),
    /// `AssistByName(name)` (`0x489c40`), the `/assist <name>` verb: what the named player
    /// targets, resolved by name rather than by token.
    AssistByName(String),
    /// `TargetLastEnemy()`: the last attackable unit committed.
    LastEnemy,
}

/// The reference's local player record (`0xc27d80`): a copy of the chosen `SMSG_CHAR_ENUM` row,
/// written once at the Enter World commit (`0x5abd9e`) before FrameXML loads, and never cleared.
/// `UnitName` (`0x517020`), `UnitRace` (`0x518200`), `UnitClass` (`0x518350`) and `UnitSex`
/// (`0x517e90`) answer `"player"`, matched whole in any case, from it and never from the
/// descriptor, so they answer before the player object exists. `UnitLevel` reads the descriptor:
/// the record's level accessor (`0x5abe00`) has no caller. Race and class are held resolved, as
/// this crate has no DBC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerRecord {
    /// `+0x08`, the name; empty is the unset record, whose `UnitName("player")` is nil
    /// (`0x517095`).
    pub name: String,
    /// `+0x100` as `UnitRace`'s (localized, file token) pair; `None` is race 0, which has no
    /// `ChrRaces` row and answers `nil, nil`.
    pub race: Option<(String, String)>,
    /// `+0x101` as `UnitClass`'s (localized, uppercase token) pair; `None` answers `nil, nil`.
    pub class: Option<(String, String)>,
    /// `+0x102` on `UnitSex`'s scale (2 male, 3 female), not the wire's 0/1. The unset 0 answers
    /// 2, as the reference's all-zero record does: `0x517ef9` indexes `{2,3,1,6}` (`0x808be4`)
    /// with no bounds check.
    pub sex: u8,
}

/// One unit token's snapshot, pushed by the app each frame and read by the `Unit*` bindings;
/// plain data, with no mlua handles or ECS types.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitState {
    /// `UnitExists`, true for an out-of-range party member through the roster fallback. The
    /// reference (`0x515fb0`) also requires a held object to be selectable before that fallback
    /// (`0x60be60`: `UNIT_FIELD_FLAGS & 0x02000000` clear, or created by us); that is not built.
    pub exists: bool,
    /// `UnitIsVisible` (`0x516030`): the client holds an object for this token, and nothing more;
    /// the range test is the server's, which drops out-of-range objects. Not [`Self::exists`]: an
    /// out-of-range party member exists without an object.
    pub has_object: bool,
    /// `UnitIsTapped` (`0x519c90`): `UNIT_DYNAMIC_FLAGS` (field 143) bit `0x4`, someone holds the
    /// kill credit. Set only from a live descriptor, which supplies the reference's
    /// object-presence check.
    pub tapped: bool,
    /// `UnitIsPartyLeader`'s first leg: `PLAYER_FLAGS & 0x1` on a player's own descriptor, true
    /// for a stranger leading their own party. The reference (`0x516210`) ORs it with a compare
    /// against our group leader's guid.
    pub group_leader: bool,
    /// `UnitIsTappedByPlayer` (`0x519d00`): the same field's bit `0x8`. Tapped without it is
    /// someone else's kill, the grey unit frame.
    pub tapped_by_player: bool,
    /// `UnitName`'s name; `None` until the name query answers, which reads as [`unknownobject`].
    pub name: Option<String>,
    /// Current health (`UnitHealth`).
    pub health: u32,
    /// Maximum health (`UnitHealthMax`).
    pub max_health: u32,
    /// Level (`UnitLevel`).
    pub level: u32,
    /// The active power type (`UnitPowerType`), `UNIT_FIELD_BYTES_0` byte 3: 0 mana, 1 rage,
    /// 2 focus, 3 energy, 4 happiness.
    pub power_type: u8,
    /// Current power of the active type (`UnitMana`).
    pub power: u32,
    /// Maximum power of the active type (`UnitManaMax`).
    pub max_power: u32,
    /// `UnitIsDead`; a released ghost is not dead by it, as its health is 1.
    pub dead: bool,
    /// `UnitIsCharmed` (`0x516cf0`): `UNIT_FIELD_CHARMEDBY` (fields 10-11) is non-zero.
    /// `UNIT_FIELD_CHARM` is never read, so a charmer answers nil, as does a summoned pet.
    pub charmed: bool,
    /// `UnitIsGhost`: `PLAYER_FLAGS` bit `0x10`, so players only.
    pub ghost: bool,
    /// The reaction toward the player on `UnitReaction`'s scale, 1 hated to 8 exalted, 0 unknown
    /// (nil). Fed for the tokens naming another unit; `"player"` and `"pet"` stay 0.
    pub reaction: u8,
    /// `UnitRace`'s localized first return, from `UNIT_FIELD_BYTES_0` byte 0; `None` answers
    /// `nil, nil`.
    pub race: Option<String>,
    /// `UnitRace`'s second return, the file token (`"NightElf"`) texture paths are built from.
    pub race_file: Option<String>,
    /// `UnitClass`'s localized first return.
    pub class: Option<String>,
    /// `UnitClass`'s second return, the uppercase token (`"WARRIOR"`).
    pub class_file: Option<String>,
    /// `UnitHasRelicSlot` (`0x519e50`): INVSLOT 17 is a relic slot, from `ChrClasses.dbc` field
    /// 16. Players only, true for Paladin, Shaman and Druid.
    pub has_relic_slot: bool,
    /// `UnitSex`'s scale, 2 male and 3 female, from `UNIT_FIELD_BYTES_0` byte 2 (0 male, 1
    /// female). The unfilled 0 answers 2, the reference's unresolved answer (`0x517f9f`).
    pub sex: u8,
    /// `UnitIsPlayer`: a player character by guid, whose tooltip level line shows race and class.
    pub is_player: bool,
    /// `UnitPlayerControlled` (`0x516410`): `UNIT_FIELD_FLAGS` bit `0x8`, which a player's pet
    /// and a charmed creature carry as well as a player.
    pub player_controlled: bool,
    /// `UNIT_FIELD_FLAGS`, raw: `UnitIsPlusMob` reads bit `0x40`, and `UNIT_FLAGS` fires when it
    /// moves.
    pub flags: u32,
    /// `PLAYER_FLAGS` (field 190), raw, 0 on a creature. `PLAYER_FLAGS_CHANGED` fires when any
    /// bit moves on any player, remote ones included: the reference's handler (`0x5ee990`) fires
    /// before any bit test or local-player check. Raw, as most of its bits are not decoded here.
    pub player_flags: u32,
    /// `UNIT_DYNAMIC_FLAGS` (field 143), raw. The event of the same name fires when any bit
    /// moves: the reference's field watch (`0x51bbb0`) compares the whole dword, and bits such as
    /// `0x2` (tracked) are not decoded here.
    pub dynamic_flags: u32,
    /// The owner: `UNIT_FIELD_SUMMONEDBY`, else the charmer, else the creator, else 0; the
    /// "or pet" half of `UnitPlayerOrPetInParty` and `UnitPlayerOrPetInRaid`.
    pub owner: u64,
    /// `GetGuildInfo(unit)`, from descriptor fields 191-192 and the guild-name cache; `None`
    /// when guildless or not yet streamed.
    pub guild: Option<super::guild::UnitGuild>,
    /// The creature template's subname ("Stable Master"), the tooltip's second line.
    pub subtitle: Option<String>,
    /// The creature type word ("Beast") from `CreatureType.dbc`, the level line's class slot.
    pub creature_type_name: Option<String>,
    /// Creature rank 0..=4 as the reference's getter (`0x605620`) answers it, so already gated:
    /// 0 without a cached creature template or with a pet number. The tooltip's rank word,
    /// `UnitLevel`'s boss -1 and `UnitClassification` all read it.
    pub rank: u32,
    /// The creature civilian flag, one term of [`is_civilian_kill`].
    pub civilian: bool,
    /// The creature racial-leader flag, the tooltip's LEADER line when PvP-flagged (`0x6125c0`).
    pub racial_leader: bool,
    /// `UNIT_FIELD_FLAGS`' PvP bit: `UnitIsPVP` and the tooltip's PvP line.
    pub pvp: bool,
    /// `UNIT_FIELD_FLAGS`' skinnable bit, the tooltip's red Skinnable line.
    pub skinnable: bool,
    /// The faction name ("Stormwind"), the tooltip line after the level line, from `Faction.dbc`
    /// through the faction template with its hiding gates applied; `None` shows no line.
    pub faction_name: Option<String>,
    /// `UnitIsConnected`; party tokens take the roster status byte's `0x01`. The default `false`
    /// reads disconnected, which greys a stock mana bar (`UnitFrame.lua:214`), so a synthetic
    /// live unit must set it.
    pub is_connected: bool,
    /// `UnitIsAFK`, the roster status byte's `0x40`, fed for party tokens.
    pub is_afk: bool,
    /// `UnitIsDND`, the roster status byte's `0x80`, fed for party tokens.
    pub is_dnd: bool,
    /// `UnitIsPVPFreeForAll`: `PLAYER_FLAGS` bit `0x80`, or the roster status byte's `0x10`;
    /// independent of [`Self::pvp`].
    pub is_pvp_ffa: bool,
    /// `UnitPVPRank`: the current rank on the internal `0..=18` scale (0 none), from the public
    /// `PLAYER_BYTES_3` byte 3. The private lifetime highest rides
    /// [`HonorState`](super::HonorState); [`super::pvp`] converts to the `-4..=14` visual scale.
    pub pvp_rank: u8,
    /// The city-protector title, `PLAYER_BYTES_3` byte 2 (0 none): `UnitPVPName` appends the
    /// `PVP_MEDAL<n>` line for a non-zero one (`0x6093ef`).
    pub pvp_medal: u8,
    /// `UnitFactionGroup`'s first return, `"Alliance"` or `"Horde"`: `FactionGroup.dbc`'s English
    /// `InternalName` for the side bit of the faction template's group mask, which stock builds
    /// texture paths from (`PlayerFrame.lua:68`). `None` for a template with no side.
    pub faction_group: Option<String>,
    /// `UnitFactionGroup`'s second return, `FactionGroup.dbc`'s localized `Name0`: display text.
    pub faction_group_localized: Option<String>,
    /// The team digit of `PVP_RANK_<rank>_<team>` (`0x5efe00`): 0 Horde, 1 Alliance, -1 no side.
    /// It follows the race through `ChrRaces` and `FactionTemplate`, never the live template
    /// `UnitFactionGroup` reads (`0x5166b8`), so a GM on template 35 keeps the rank title. The
    /// default 0 is the reference's answer for a unit it cannot resolve (`0x51a9af`).
    pub pvp_team: i8,
    /// `OBJECT_FIELD_GUID`, which `UnitIsUnit` and `UnitInParty` compare; 0 is unresolved, and
    /// two zeros never match.
    pub guid: u64,
    /// `GetRaidTargetIndex`: 0 none, else the mark 1..=8, from the group's raid-target board.
    pub raid_target: u8,
    /// Whether the player can attack the unit, `UnitCanAttack("player", unit)`, which the binding
    /// (`0x516c50`, delegating to `CanAttack` `0x606980`) answers for both argument orders. Fed for
    /// `target`, `targettarget` and `npc`; other tokens read false.
    pub can_attack: bool,
    /// `UnitIsCorpse` (`0x5161c0`): the token names a `TYPEID_CORPSE` object, which a dead unit
    /// is not. No feed sets it.
    pub corpse_object: bool,
    /// `UnitAffectingCombat` (`0x517e10`): `UNIT_FIELD_FLAGS` bit `0x00080000`, for every token
    /// including `"player"`; the reference has no separate player combat flag.
    pub in_combat: bool,
}

/// The name of a unit whose name is not yet known, as the reference's `UnitName` (`0x517220`) and
/// unit tooltip title (`0x609324`) both answer it: the `UNKNOWNOBJECT` GlobalString (enUS
/// "Unknown"), else the literal "Unknown Being" (`0x860fa4`) when the global is missing or empty.
pub fn unknownobject(lua: &Lua) -> mlua::Result<mlua::String> {
    // One expression: `benilla-app`'s `reference_strings` check matches
    // `globals().get::<String>`, and a line break inside it would hide this lookup.
    let global = lua.globals().get::<String>("UNKNOWNOBJECT").ok();
    lua.create_string(match global.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => "Unknown Being",
    })
}

/// The reference's grey-level band (`0x80ae98`; twins at `0x81dda8` and `0x8076c0`), indexed by
/// player level / 5.
const GREY_BAND: [u32; 20] = [
    4, 4, 5, 5, 6, 6, 7, 7, 8, 9, 10, 11, 12, 12, 12, 12, 12, 12, 12, 12,
];

/// `GetQuestGreenRange`'s answer (`0x4e17d0`): the band for a player level, 12 past the table.
pub fn grey_band(player_level: u32) -> u32 {
    GREY_BAND
        .get((player_level / 5) as usize)
        .copied()
        .unwrap_or(12)
}

/// The reference's grey level test (`0x5f0700`): the unit is more than the band below the player,
/// strictly, so a gap equal to the band is still green. The reference also requires the unit not
/// be player-controlled; every caller here passes creatures.
pub fn unit_is_grey(player_level: u32, unit_level: u32) -> bool {
    player_level > unit_level && player_level - unit_level > grey_band(player_level)
}

/// The civilian gate (`0x612550`), a dishonorable kill: PvP-flagged, a civilian creature, hostile
/// (`UnitReaction` <= 2) and grey. The tooltip's CIVILIAN line and `UnitPVPName`'s civilian prefix
/// (`0x609370`) both read it. The terms are the tooltip line's; `0x612550`'s body is not traced.
pub fn is_civilian_kill(u: &UnitState, player_level: u32) -> bool {
    u.civilian && u.pvp && u.reaction != 0 && u.reaction <= 2 && unit_is_grey(player_level, u.level)
}

/// The tooltip's "??" level gate (`0x529fe0`): never a player; level 0, world-boss rank 3, or a
/// hostile (reaction <= 2) at least 10 levels above the player. `UnitLevel` (`0x517fc0`) answers
/// the boss and hostile legs with -1, but returns a level of 0 as is.
pub fn level_reads_unknown(u: &UnitState, player_level: u32) -> bool {
    let much_higher_hostile =
        u.reaction != 0 && u.reaction <= 2 && u.level >= player_level.saturating_add(10);
    !u.is_player && (u.level == 0 || u.rank == 3 || much_higher_hostile)
}

/// `UnitClassification` (`0x516d90`): the word table at `0x850424` indexed by the gated
/// [`UnitState::rank`]; an unresolved token reads index 0, "normal" (`0x516dc4`). "rareelite" is
/// a real answer, though stock draws it with the elite art (`TargetFrame.lua:209`).
pub fn classification_word(rank: u32) -> &'static str {
    match rank {
        1 => "elite",
        2 => "rareelite",
        3 => "worldboss",
        4 => "rare",
        _ => "normal",
    }
}

/// The resource name for a power type, the suffix of 1.12's per-resource events (`UNIT_RAGE`,
/// `UNIT_MAXRAGE`); an unknown index reads as mana, the descriptor default.
pub fn power_token(ty: u8) -> &'static str {
    match ty {
        1 => "RAGE",
        2 => "FOCUS",
        3 => "ENERGY",
        4 => "HAPPINESS",
        _ => "MANA",
    }
}

impl super::UiScript {
    /// Push, or clear with `None`, a token's snapshot; the app's feed calls it each frame before
    /// event dispatch, so a frame's `OnEvent` sees current values.
    pub fn set_unit(&mut self, token: &str, state: Option<UnitState>) {
        {
            let mut model = self.model_mut();
            match state {
                // Keys fold to lowercase on the way in too, so a `"Target"` push cannot shadow
                // `"target"`.
                Some(s) => {
                    // A `"player"` push updates each record field it carries and never blanks
                    // one, so a nameless snapshot cannot reach the four record verbs.
                    if token.eq_ignore_ascii_case("player") {
                        let rec = &mut model.player_record;
                        if let Some(name) = s.name.as_deref().filter(|n| !n.is_empty()) {
                            if rec.name != name {
                                rec.name = name.to_string();
                            }
                        }
                        if let Some(race) = s.race.clone().zip(s.race_file.clone()) {
                            rec.race = Some(race);
                        }
                        if let Some(class) = s.class.clone().zip(s.class_file.clone()) {
                            rec.class = Some(class);
                        }
                        if s.sex != 0 {
                            rec.sex = s.sex;
                        }
                    }
                    model.units_by_lower.insert(token.to_ascii_lowercase(), s);
                }
                None => {
                    model.units_by_lower.remove(&token.to_ascii_lowercase());
                }
            }
        }
        // A push for a tooltip's live unit token re-drives its health bar, without a line rebuild.
        super::tooltip_unit::on_unit_push(&self.lua, token);
    }

    /// Push the player's copper (`PLAYER_FIELD_COINAGE`), read by `GetMoney`.
    pub fn set_money(&mut self, copper: u64) {
        self.model_mut().money = copper;
    }

    /// Push `PLAYER_XP` and `PLAYER_NEXT_LEVEL_XP`, private fields that `UnitXP` and `UnitXPMax`
    /// answer for `"player"` alone.
    pub fn set_player_xp(&mut self, xp: u32, next_level_xp: u32) {
        let mut model = self.model_mut();
        model.player_xp = xp;
        model.player_next_level_xp = next_level_xp;
    }

    /// Push the rest state (`PLAYER_BYTES_2` byte 3), the rested pool
    /// (`PLAYER_REST_STATE_EXPERIENCE`, raw) and `PLAYER_FLAGS_RESTING` together, so
    /// `GetRestState`, `GetXPExhaustion` and `IsResting` never read a half update.
    pub fn set_rest_state(&mut self, state: u8, pool: u32, resting: bool) {
        let mut model = self.model_mut();
        model.rest_state = state;
        model.rest_pool = pool;
        model.resting = resting;
    }

    /// Push `PLAYER_FLAGS` bits 12 (`PartialPlayTime`) and 13 (`NoPlayTime`) together, as
    /// `PlayerFrame.lua:244` tests them in one if/elseif.
    pub fn set_play_time(&mut self, partial: bool, none: bool) {
        let mut model = self.model_mut();
        model.partial_play_time = partial;
        model.no_play_time = none;
    }

    /// Push `GetBillingTimeRested()`'s minutes, from the `SMSG_AUTH_RESPONSE` that admitted the
    /// session; nothing else on the wire writes it.
    pub fn set_billing_time_rested(&mut self, minutes: u32) {
        self.model_mut().billing_time_rested = minutes;
    }

    /// Enter the UI-load sound-suppression scope (`0x458f50`): a depth every name-keyed
    /// `PlaySound` reads, so scopes nest and only the outermost exit re-enables sound.
    pub fn push_sound_suppression(&self) {
        let mut model = self.model_mut();
        model.sound_suppression = model.sound_suppression.saturating_add(1);
    }

    /// Leave it (`0x458f60`), saturating so an unbalanced pop cannot wrap to a muted UI.
    pub fn pop_sound_suppression(&self) {
        let mut model = self.model_mut();
        model.sound_suppression = model.sound_suppression.saturating_sub(1);
    }

    /// Push whether a cinematic is playing, `InCinematic()`'s answer.
    pub fn set_in_cinematic(&mut self, playing: bool) {
        self.model_mut().in_cinematic = playing;
    }

    /// Push `Exhaustion.dbc`'s (rest state, localized name, factor) rows, read as `0x48d350` and
    /// `0x48d3f0` read them; an empty push keeps the fallback table the model seeds.
    pub fn set_exhaustion_rows(&mut self, rows: Vec<(u8, String, f64)>) {
        if rows.is_empty() {
            return;
        }
        let mut model = self.model_mut();
        model.exhaustion = rows.into_iter().map(|(id, n, f)| (id, (n, f))).collect();
    }

    /// Push the combo points (`PLAYER_FIELD_BYTES` byte 1) and their target
    /// (`PLAYER_FIELD_COMBO_TARGET`) together, as `Player::SetComboPoints` writes them. Raw:
    /// `GetComboPoints`' class and target gates are the binding's.
    pub fn set_combo_points(&mut self, points: u8, target: u64) {
        let mut model = self.model_mut();
        model.combo_points = points;
        model.combo_target = target;
    }

    /// Drain the queued [`SelectionRequest`]s, in call order.
    pub fn take_selection_requests(&mut self) -> Vec<SelectionRequest> {
        std::mem::take(&mut self.model_mut().selection_requests)
    }

    /// Drain the `TargetNearestFriend([reverse])` calls, `true` for reverse. It names no unit: the
    /// reference runs the TAB cycler (`0x493f60`, mode 2) straight into `SetSelection`.
    pub fn take_target_nearest_friend_requests(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.model_mut().target_nearest_friend_requests)
    }

    /// Drain the `TargetByName(name, exactMatch)` calls for the app's by-name resolver.
    pub fn take_target_by_name_requests(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.model_mut().target_by_name_requests)
    }

    /// Drain `ClearTarget()`: true if it fired with a live target; the app deselects (guid 0).
    pub fn take_target_clear(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().target_clear)
    }

    /// Drain `DropItemOnUnit`'s tokens; on the pet the app casts the learned Feed Pet spell at
    /// the held item (`0x48d960`).
    pub fn take_drop_item_on_unit(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().drop_item_on_unit)
    }
}

/// The non-`"player"` token of a two-unit call such as `UnitIsEnemy(a, b)`, which the snapshot
/// holds the relationship on; the test folds case, as the resolver does.
fn pick_unit_token(a: &Option<String>, b: &Option<String>) -> Option<String> {
    match (a, b) {
        (Some(x), _) if !x.eq_ignore_ascii_case("player") => Some(x.clone()),
        (_, Some(y)) => Some(y.clone()),
        (x, _) => x.clone(),
    }
}

/// The token prefixes the resolver (`0x515970`) tests, in its order (`partypet` before `party`).
/// Each is a prefix test, so `"playerfoo"` is recognised; `npc`, its one full-string compare, is
/// tested apart.
const UNIT_TOKEN_PREFIXES: [&str; 8] = [
    "player",
    "pet",
    "target",
    "partypet",
    "party",
    "raidpet",
    "raid",
    "mouseover",
];

/// Whether the resolver recognises the token, not whether it names a unit: `"party5"` solo is
/// recognised and answers nil, and only a token none of its nine compares match raises.
pub(crate) fn token_recognised(token: &str) -> bool {
    // `npc` full-string, the rest prefixes, all folded as `_strnicmp` folds: ASCII only.
    token.eq_ignore_ascii_case("npc")
        || UNIT_TOKEN_PREFIXES
            .iter()
            // Bytes, not a `str` slice, which panics mid-character on a multibyte token.
            .any(|p| {
                token.len() >= p.len()
                    && token.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
            })
}

/// The resolver's token check: an unrecognised token raises `Unknown unit name`, as `0x515970`
/// ends in `luaL_error`; `""`, an absent token and a recognised one naming nothing pass. Whether
/// nil arrives is per binding: of the 83 in the table at `0x850438`, 53 raise `Usage:` first and
/// 13 unit-token bindings take nil with no gate.
pub(crate) fn check_unit_token(token: &Option<String>) -> mlua::Result<()> {
    match token {
        Some(t) if !t.is_empty() && !token_recognised(t) => {
            Err(mlua::Error::runtime(format!("Unknown unit name: {t}")))
        }
        _ => Ok(()),
    }
}

/// Map a token's snapshot through `f` under a short model borrow, or answer `default` for an
/// absent unit, after the token check.
fn with_unit<T>(
    lua: &Lua,
    token: &Option<String>,
    default: T,
    f: impl FnOnce(&UnitState) -> T,
) -> mlua::Result<T> {
    check_unit_token(token)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    Ok(match token.as_ref().and_then(|t| model.unit(t)) {
        Some(u) => f(u),
        None => default,
    })
}

/// A unit predicate's body: the number 1 or nil through [`super::binding_abi::flag`], never a Lua
/// boolean, as the reference pushes only `lua_pushnumber` (`0x6f3810`) or `lua_pushnil`
/// (`0x6f37f0`). `UnitReaction`, `UnitLevel` and `UnitPowerType` are getters with their own
/// shapes and do not route here.
fn unit_predicate(
    lua: &Lua,
    token: &Option<String>,
    f: impl FnOnce(&UnitState) -> bool,
) -> mlua::Result<mlua::Value> {
    Ok(super::binding_abi::flag(with_unit(lua, token, false, f)?))
}

/// The `Unit*` and `GetQuestGreenRange` registrations.
mod bindings;
#[cfg(test)]
mod tests;

pub(super) use bindings::install;
