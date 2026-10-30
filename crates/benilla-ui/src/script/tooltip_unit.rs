//! The engine unit tooltip, in `0x529fe0`'s line order: the name (gold, decorated by `0x609370`
//! as `UnitPVPName`'s is; FrameXML recolours it by reaction), the creature subtitle, the owner
//! line, the level line, the faction name, then `PVP_ENABLED`, `RESURRECTABLE`, the
//! studied-creature block, `UNIT_SKINNABLE`, `PVP_RANK_CIVILIAN` and `PVP_RANK_LEADER`, with
//! health on the `<name>StatusBar` child. The world mouseover drives it through
//! [`super::UiScript::world_tooltip_unit`]; unit-frame hovers call `SetUnit` from Lua.

use mlua::{Lua, Table, Value};

use super::object::frame_handle_of;
use super::tooltip::{append_line, clear, set_owner_core, show_or_hide_empty};
use super::unit::{is_civilian_kill, level_reads_unknown, tooltip_drunk_level, unknownobject};
use super::{KindState, Model, UnitState};
use crate::strings::{fill, Arg};
use crate::widget::{FrameHandle, TooltipAnchor};

/// What only the unit tooltip builder reads of a unit, fed beside its snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitTooltipFacts {
    /// Raw `UNIT_FIELD_HEALTH` (`fields+0x40`), which feign death leaves standing where
    /// [`UnitState::health`] reads 0: the `RESURRECTABLE` gate reads it (`0x52a89f`-`0x52a8a4`).
    pub health: u32,
    /// The owner line `0x52fd30` writes after the subtitle (`0x52a247`), `None` for a unit with no
    /// charmer or creator, or whose owner is not in sight (`0x52fd8b`).
    pub owner: Option<OwnerTitle>,
    /// The member's group record, the party's (`0x4e8820`) else the raid roster's (`0x4bb0f0`),
    /// as `0x496400` finds it by guid; `None` off both. The app fills it on the member's own party
    /// and raid rows, and the builder reads it there by guid for whatever unit it shows
    /// (`0x52a02b`).
    pub party: Option<PartyRecord>,
    /// The studied-creature block's facts ([`super::tooltip_studied`]), filled while the unit's
    /// `UNIT_DYNAMIC_FLAGS` bit 4 is set.
    pub studied: Option<Box<super::StudiedCreature>>,
}

/// What the builder reads of a member's group record, the party's or the raid's, which shares the
/// party layout (the raid entry's record at `+0x10`, `0x4bb127`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartyRecord {
    /// `+0x8` bit 0: clear adds `PLAYER_OFFLINE` and ends the tooltip (`0x52a6e9`).
    pub online: bool,
    /// The name of the area the record's `+0x14` word names (`0x67e460`), the zone line of a
    /// member out of sight; `None` without an `AreaTable.dbc` row.
    pub area: Option<String>,
}

/// The owner line: the owner's name in one of four `UNITNAME_TITLE_*` templates.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerTitle {
    pub kind: OwnerTitleKind,
    /// The owner's `GetUnitName 0x609210` (`0x52fd98`); `None` is its `UNKNOWNOBJECT` miss.
    pub name: Option<String>,
}

/// Which template names the owner: the key table `0x854140`, entries 1 to 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerTitleKind {
    /// `UNITNAME_TITLE_PET`: a beast (creature type 1, `0x52fd6b`) with a charmer, or with a
    /// creator whose spell summons neither a guardian nor an object.
    Pet,
    /// `UNITNAME_TITLE_MINION`: any other type in the same case.
    Minion,
    /// `UNITNAME_TITLE_GUARDIAN`: a creator's spell whose first effect is 42 (`0x52fe24`).
    Guardian,
    /// `UNITNAME_TITLE_CREATION`: one whose first effect summons an object, 50, 74, 87 to 90 or
    /// 104 to 107 (`0x52fe2e`, the byte table `0x52fe44`).
    Creation,
}

impl OwnerTitleKind {
    fn key(self) -> &'static str {
        match self {
            OwnerTitleKind::Pet => "UNITNAME_TITLE_PET",
            OwnerTitleKind::Minion => "UNITNAME_TITLE_MINION",
            OwnerTitleKind::Guardian => "UNITNAME_TITLE_GUARDIAN",
            OwnerTitleKind::Creation => "UNITNAME_TITLE_CREATION",
        }
    }
}

/// The active player as the builder reads it (`[ebp-0x4]`, found at `0x52a281`): none before its
/// object streams, when every line that needs it is left out.
struct Viewer {
    drunk: u8,
    /// `CanAttack 0x606980(player, unit)`, the drunk level's gate (`0x52a3ad`).
    can_attack: bool,
    /// `CanAssist 0x6066f0(player, unit)`, the `RESURRECTABLE` gate (`0x52a8bc`).
    can_assist: bool,
}

/// The active player toward `u`, through the host's relation functions as `UnitCanAttack` and
/// `UnitCanAssist` ask them.
fn viewer(lua: &Lua, model: &Model, u: &UnitState) -> Option<Viewer> {
    let me = model.unit_guids.player;
    if me == 0 {
        return None;
    }
    // The active player's drunkenness off its object as the call runs.
    let drunk = super::unit::view_held(lua, model, "player").map_or(0, |p| {
        let mut drunk = None;
        super::host::read_host(lua, |h| {
            h.read_unit_object(p.guid, &mut |o| drunk = Some(o.drunk()))
        });
        drunk.unwrap_or(p.drunk)
    });
    let ask = |f: &dyn Fn(&dyn super::UnitRelations) -> Option<bool>| {
        u.has_object
            && super::unit::with_relations(lua, model, f)
                .flatten()
                .unwrap_or(false)
    };
    Some(Viewer {
        drunk,
        can_attack: ask(&|r| r.can_attack(me, u.guid)),
        can_assist: ask(&|r| r.can_assist(me, u.guid)),
    })
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
/// `0xc0d420` = `0xff40c040`, the satisfiable-lock green.
const LOCK_OPEN: [f32; 4] = [64.0 / 255.0, 192.0 / 255.0, 64.0 / 255.0, 1.0];
/// `0xffffd200`, the unit name's colour until FrameXML recolours it by reaction.
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];
/// `0xc0d3bc` = `0xff002e5a` (`0x529010`), a game object tooltip's backdrop.
const OBJECT_BACKDROP: crate::color::Rgba8 = crate::color::Rgba8::rgba(0, 0x2e, 0x5a, 0xff);

/// A GameObject tooltip line's colour, as the builder `0x52aa20` picks it; the app picks the tint
/// for each line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipTint {
    /// `0xc0cf60`: a requirement line's normal colour (the key item's "Requires %s").
    White,
    /// `0xc0d3a8` = `0xffff2020`, the unmet "Locked" line. Deviation: the unknown-skill
    /// "Requires %s" shares it, though its reference red is `0xffff0000`, because a 32/255
    /// difference in G and B does not show.
    Red,
    /// The satisfiable "Locked" line: `0xc0d420` = `0xff40c040`, the green rung of the
    /// skill-difficulty ramp `0x529fa0`, not the ordinary green `0xc0d3ac` = `0xff00ff00`.
    LockOpen,
    /// The skill-difficulty ramp `0x529fa0` of a known opener's value against the requirement:
    /// a skill-opened "Locked" line (`0x52ab31`) and `LOCKED_WITH_SPELL_KNOWN` (`0x52abd8`).
    Skill { required: i32, value: i32 },
    /// `0xc0d3e8` = `0xffffd200`, the name line's gold: a type handler's lines, which
    /// `0x530380` adds with that colour on both cells (`0x53038d`, `0x530392`).
    Gold,
}

/// `CreatureType.dbc` id 10, "Not specified".
const CREATURE_TYPE_NOT_SPECIFIED: u32 = 10;

/// The rank word's GlobalString key, from the builder's table `0x854158` (indexed at `0x52a5bd`
/// and `0x52a5d4`): ranks 1 and 2 (elite, rare elite) are `ELITE`, 3 is `BOSS`, and 0 and 4 (rare)
/// are the empty string `0x882748`.
fn rank_key(rank: u32) -> Option<&'static str> {
    match rank {
        1 | 2 => Some("ELITE"),
        3 => Some("BOSS"),
        _ => None,
    }
}

/// The level line: level, class and type slots in one of the four `TOOLTIP_UNIT_LEVEL*`
/// GlobalStrings, keyed as the builder keys them (`0x52a622` `_CLASS_TYPE`, `0x52a64d` `_CLASS`,
/// `0x52a682` `_TYPE`, `0x52a6ac` bare), never by their English, which other keys share. `None`
/// when the install has no template for the combination, and then no row shows.
fn level_line(
    lua: &Lua,
    u: &UnitState,
    player_level: u32,
    viewer: Option<&Viewer>,
) -> Option<String> {
    let get = |key: &str| crate::strings::global(lua, key);
    // "??" by the gate `UnitLevel`'s -1 shares (`0x529fe0`), so tooltip and target frame agree;
    // else a drunk player's view of a unit it can attack (`0x52a384`-`0x52a3b4`).
    let level_text = if level_reads_unknown(u, player_level) {
        "??".to_string()
    } else {
        match viewer.filter(|v| v.drunk != 0 && v.can_attack) {
            Some(v) => tooltip_drunk_level(u.level, v.drunk).to_string(),
            None => u.level.to_string(),
        }
    };
    // `CORPSE` when dead, "Race Class" for a player, else the creature type, only when the player's
    // reaction toward the unit is below friendly (`0x52a2d9`-`0x52a2de`, `UnitReaction` 4 or
    // less) and never without an active player (`0x52a2d0`).
    let class_slot = if u.dead {
        get("CORPSE")
    } else if u.is_player {
        match (&u.race, &u.class) {
            (Some(r), Some(c)) => Some(format!("{r} {c}")),
            (Some(r), None) => Some(r.clone()),
            (None, Some(c)) => Some(c.clone()),
            (None, None) => None,
        }
    } else if u.player_reaction != 0
        && u.player_reaction <= 4
        && u.creature_type != CREATURE_TYPE_NOT_SPECIFIED
    {
        // Type 10 is no word here (`0x52a2c1` resolves the type, `0x52a2c6` compares it with 10
        // and skips the row); `UnitCreatureType` answers its word.
        u.creature_type_name.clone()
    } else {
        None
    };
    let type_slot = if u.is_player {
        get("PLAYER")
    } else {
        rank_key(u.rank).and_then(&get)
    };
    let (key, args): (&str, Vec<Arg<'_>>) = match (&class_slot, &type_slot) {
        (Some(c), Some(t)) => (
            "TOOLTIP_UNIT_LEVEL_CLASS_TYPE",
            vec![Arg::S(&level_text), Arg::S(c), Arg::S(t)],
        ),
        (Some(c), None) => (
            "TOOLTIP_UNIT_LEVEL_CLASS",
            vec![Arg::S(&level_text), Arg::S(c)],
        ),
        (None, Some(t)) => (
            "TOOLTIP_UNIT_LEVEL_TYPE",
            vec![Arg::S(&level_text), Arg::S(t)],
        ),
        (None, None) => ("TOOLTIP_UNIT_LEVEL", vec![Arg::S(&level_text)]),
    };
    Some(fill(&get(key)?, &args))
}

/// Whom a unit build describes: the unit a token names at the call (`SetUnit`, the world
/// publisher's `"mouseover"`), or a guid as the refresh holds it (`0x4919f2`).
#[derive(Clone, Copy)]
enum Subject<'a> {
    Token(&'a str),
    Guid(u64),
}

/// Render the unit tooltip for `token`'s current snapshot (`0x529fe0`); returns whether the unit
/// existed. The clear first (`0x52a003`); no unit hides the tooltip through its own hide, shown bit
/// then slot (`0x52a050`/`0x52a058`). A unit is held (`0x52a070`), its bar seeded and shown
/// (`0x52a0e0`–`0x52a146`) before any line, and the lines end in the show (`0x52a711`/`0x52a71b`).
fn render_unit(lua: &Lua, this: &Table, token: &str) -> mlua::Result<bool> {
    render_subject(lua, this, Subject::Token(token))
}

/// [`render_unit`] over `subject`.
fn render_subject(lua: &Lua, this: &Table, subject: Subject<'_>) -> mlua::Result<bool> {
    let h = frame_handle_of(lua, this)?;
    // A unit is its object, found by guid (`0x52a021`); guid 0 finds none.
    let (unit, viewer, req, me) = {
        let model = lua.app_data_mut::<Model>().expect("model app_data");
        // The unit's state carries the group record by guid, which the build reads for every unit
        // shown (`0x52a02b`).
        let unit = match subject {
            Subject::Token(token) => crate::script::unit::view(lua, &model, token),
            Subject::Guid(guid) => Some(crate::script::unit::view_of_guid(lua, &model, guid)),
        };
        let unit = unit
            .map(std::borrow::Cow::into_owned)
            .filter(|u| u.exists && u.guid != 0);
        let viewer = unit.as_ref().and_then(|u| viewer(lua, &model, u));
        let req = crate::script::requirements::player_req(lua, &model);
        (unit, viewer, req, model.unit_guids.player)
    };
    let player_level = crate::script::ReqRead::head(&req).level;
    clear(lua, h);
    let Some(u) = unit else {
        super::visibility::set_shown(lua, h, false);
        return Ok(false);
    };
    set_subject(lua, h, Some(u.guid));
    if let Some(bar) = seed_bar(lua, h, &u) {
        super::visibility::set_shown(lua, bar, true);
    }
    // The name: `CGUnit_C::GetUnitName` `0x609210` (`0x52a187`), whose every miss falls to
    // `UNKNOWNOBJECT`, as `UnitName`'s does, then decorated by `0x609370` with the flag a literal
    // `1` (`0x52a19b`), as `UnitPVPName` does, so no CVar gates it: a ranked player's rank and
    // medal line, a civilian kill's prefix. A token with no object never reaches the builder
    // (`0x468460`): no plate, and `SetUnit` answers nil.
    let title = match &u.name {
        Some(name) => super::pvp::pvp_name(lua, &u, name, player_level),
        None => unknownobject(lua)?.to_str()?.to_string(),
    };
    append_line(lua, this, (title, GOLD), None, false)?;
    if let Some(sub) = &u.subtitle {
        append_line(lua, this, (sub.clone(), WHITE), None, false)?;
    }
    // The owner line, white (`0x52a24c`-`0x52a266`): the template filled with the owner's name.
    if let Some(owner) = u.tooltip.owner.as_ref().filter(|_| u.has_object) {
        let name = match &owner.name {
            Some(n) => n.clone(),
            None => unknownobject(lua)?.to_str()?.to_string(),
        };
        if let Some(template) = crate::strings::global(lua, owner.kind.key()) {
            append_line(
                lua,
                this,
                (fill(&template, &[Arg::S(&name)]), WHITE),
                None,
                false,
            )?;
        }
    }
    if let Some(level) = level_line(lua, &u, player_level, viewer.as_ref()) {
        append_line(lua, this, (level, WHITE), None, false)?;
    }
    // A group member whose record is offline, in sight or not: `PLAYER_OFFLINE`, white, and the
    // build ends (`0x52a6e2`-`0x52a72b`).
    if u.tooltip.party.as_ref().is_some_and(|r| !r.online) {
        keyed_line(lua, this, "PLAYER_OFFLINE", WHITE)?;
        show_or_hide_empty(lua, h);
        return Ok(true);
    }
    // The faction name, between the level line and the PvP line (`0x52a7a0`); the app applied its
    // gates.
    if let Some(faction) = &u.faction_name {
        append_line(lua, this, (faction.clone(), WHITE), None, false)?;
    }
    // The status lines read the unit's object (`0x52a840`): a member out of sight has none.
    if u.has_object {
        // `PVP_ENABLED` (`0x52a886`): a player's contested flag, `PLAYER_FLAGS` bit 8
        // (`0x52a856`-`0x52a864`), or the PvP unit flag, bit 12 (`0x52a866`-`0x52a878`).
        let contested = u.is_player && u.player_flags & PLAYER_FLAGS_CONTESTED_PVP != 0;
        if contested || u.flags & UNIT_FLAG_PVP != 0 {
            keyed_line(lua, this, "PVP_ENABLED", WHITE)?;
        }
        // `RESURRECTABLE` (`0x52a8d1`), green: a player at 0 health or less (`0x52a8a4`,
        // `0x52a8ac`) the active player may assist (`0x52a8bc`).
        if (u.tooltip.health as i32) <= 0
            && u.is_player
            && viewer.as_ref().is_some_and(|v| v.can_assist)
        {
            keyed_line(lua, this, "RESURRECTABLE", GREEN)?;
        }
        // The studied-creature block (`0x529a40`), for `UNIT_DYNAMIC_FLAGS` bit 4 (`0x52a8ea`).
        if u.dynamic_flags & super::tooltip_studied::UNIT_DYNFLAG_SPECIALINFO != 0 {
            if let Some(studied) = &u.tooltip.studied {
                super::tooltip_studied::studied_lines(lua, this, u.tooltip.health, studied)?;
            }
        }
        // `UNIT_SKINNABLE` (`0x529ed0`): unit flags bit 26 off a creature (`0x52a914`,
        // `0x52a924`), red but for a player with the skinning spell (`0x529eed`-`0x529f86`).
        if u.skinnable && !u.is_player {
            // The latched spell's points, read only for a skinnable unit (`0x529f06`).
            let color = crate::script::ReqRead::skinning(&req).map_or(RED, |value| {
                super::tooltip_item::names::skill_color(skinning_required(u.level), value)
            });
            keyed_line(lua, this, "UNIT_SKINNABLE", color)?;
        }
    }
    // A party or raid member (`0x4918e0`: `0x4e7f70`, else `0x4baee0`) other than the active
    // player (`0x52a953`) with no object: the name of its record's area, white, and nothing more
    // (`0x52a95e`-`0x52a995`).
    if !u.has_object {
        let area = u
            .tooltip
            .party
            .as_ref()
            .filter(|_| u.guid != me)
            .and_then(|r| r.area.clone());
        if let Some(area) = area {
            append_line(lua, this, (area, WHITE), None, false)?;
        }
    } else {
        // `PVP_RANK_CIVILIAN` (`0x52a9b2`), green: `0x612550`'s civilian kill.
        if is_civilian_kill(&u, player_level) {
            keyed_line(lua, this, "PVP_RANK_CIVILIAN", GREEN)?;
        }
        // `PVP_RANK_LEADER` (`0x52a9e0`): `0x6125c0`, PvP-flagged and a racial leader.
        if u.racial_leader && u.flags & UNIT_FLAG_PVP != 0 {
            keyed_line(lua, this, "PVP_RANK_LEADER", WHITE)?;
        }
    }
    show_or_hide_empty(lua, h);
    Ok(true)
}

/// The skinning value a creature of `level` requires (`0x529f60`-`0x529f7c`): 1 to level 10, then
/// `(5 · level − 50) · 2` below 20, else `5 · level`.
fn skinning_required(level: u32) -> i32 {
    let level = level as i32;
    if level <= 10 {
        1
    } else if level < 20 {
        (5 * level - 50) * 2
    } else {
        5 * level
    }
}

/// `PLAYER_FLAGS` bit 8, the contested flag (vmangos `PLAYER_FLAGS_CONTESTED_PVP`).
const PLAYER_FLAGS_CONTESTED_PVP: u32 = 0x100;
/// `UNIT_FIELD_FLAGS` bit 12, the PvP flag.
const UNIT_FLAG_PVP: u32 = 0x1000;

/// A line of the install's string under `key` (`0x703bf0`), unwrapped; none when it is absent.
fn keyed_line(lua: &Lua, this: &Table, key: &str, color: [f32; 4]) -> mlua::Result<()> {
    match crate::strings::global(lua, key) {
        Some(text) => append_line(lua, this, (text, color), None, false),
        None => Ok(()),
    }
}

/// Remember (or drop) the guid of the unit the tooltip shows, the health watcher's key.
fn set_subject(lua: &Lua, h: FrameHandle, guid: Option<u64>) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if let Some(f) = model.arena.frame_mut(h) {
        if let KindState::Tooltip(t) = &mut f.kind_state {
            t.unit_guid = guid;
        }
    }
}

/// `f` on tooltip `h`'s state.
fn with_tip(lua: &Lua, h: FrameHandle, f: impl FnOnce(&mut crate::widget::TooltipState)) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if let Some(KindState::Tooltip(t)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        f(t);
    }
}

/// A tooltip with a backdrop takes `SetBackdropColor` of `0xc0d3bc`, the `0xff002e5a` `0x529010`
/// writes (`0x52aa92`-`0x52aaaa`); `GameTooltip_OnHide` restores the default.
fn tint_object_backdrop(lua: &Lua, h: FrameHandle) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    super::backdrop::set_color(&mut model, h, false, OBJECT_BACKDROP);
}

/// A game object line's colour as its tint names it.
fn tint_colour(tint: TooltipTint) -> [f32; 4] {
    match tint {
        TooltipTint::White => WHITE,
        TooltipTint::Red => RED,
        TooltipTint::LockOpen => LOCK_OPEN,
        TooltipTint::Skill { required, value } => {
            super::tooltip_item::names::skill_color(required, value)
        }
        TooltipTint::Gold => GOLD,
    }
}

/// What the game object builder reads of the object a guid names (`0x52aa20`), asked of the lent
/// host as the build runs; the answer is `None` for no game object held under the guid
/// (`0x52aa5e`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectPlateQuery(pub u64);

impl super::live::LiveQuery for ObjectPlateQuery {
    type Answer = Option<ObjectPlate>;
}

/// A game object plate's content: the record's name (`0x5f8100`, empty without a record), then
/// the type handler's lines (`0x52aad4`) and the lock lines (`0x52aae2`-`0x52ace7`), each read
/// through the VM's strings as it is added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectPlate {
    pub name: String,
    pub lines: Vec<(super::TooltipText, TooltipTint)>,
}

/// What the corpse builder reads of the corpse a guid names (`0x52aef0`), asked of the lent host
/// as the build runs: the answer is `None` for no corpse held under the guid (`0x52af1b`), else
/// its owner's name from the name cache, `None` while the lookup waits on its callback
/// (`0x52af77`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorpsePlateQuery(pub u64);

impl super::live::LiveQuery for CorpsePlateQuery {
    type Answer = Option<Option<String>>;
}

/// Drive the `<name>StatusBar` child from a unit snapshot, as the builder (`0x52a0eb` range,
/// `0x52a12e` value) and the health watcher (`0x529560`) do: the raw maximum and health through
/// [`super::statusbar::set_health`], firing each `OnValueChanged` it makes; FrameXML's
/// `HealthBar_OnValueChanged` colours the fill. Neither shows it. Returns the bar.
fn seed_bar(lua: &Lua, h: FrameHandle, u: &UnitState) -> Option<FrameHandle> {
    seed_bar_at(lua, h, u.health, u.max_health)
}

/// [`seed_bar`] off the unit's health and maximum alone.
fn seed_bar_at(lua: &Lua, h: FrameHandle, health: u32, max_health: u32) -> Option<FrameHandle> {
    let (bar, fires) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let bar = super::tooltip::status_bar(&model, h)?;
        if let Some(KindState::Tooltip(t)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            t.unit_health = health;
        }
        let fires = super::statusbar::set_health(&mut model, bar, max_health as f32, health as f32);
        (bar, fires)
    };
    for value in fires.into_iter().flatten() {
        super::statusbar::fire_engine_value_changed(lua, bar, value);
    }
    Some(bar)
}

/// The tooltips showing `guid`, whose health is now `health` of `max_health`: each whose bar last
/// saw another health is seeded again (`0x529560`, its compare at `0x4655bb`).
fn on_unit_health(lua: &Lua, guid: u64, health: u32, max_health: u32) {
    let hits: Vec<FrameHandle> = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        // Registered frames only, as `update_bar` writes layout.
        model
            .arena
            .tooltip_kinds()
            .iter()
            .copied()
            .filter(|&h| {
                model.frame_to_id.contains_key(&h)
                    && matches!(
                        model.arena.frame(h).map(|f| &f.kind_state),
                        Some(KindState::Tooltip(t))
                            if t.unit_guid == Some(guid) && t.unit_health != health
                    )
            })
            .collect()
    };
    for h in hits {
        seed_bar_at(lua, h, health, max_health);
    }
}

impl super::UiScript {
    /// The unit tooltip's own watcher on the health of the unit it shows, which the render
    /// registers on the object (`0x52a0a8`, `0x529560` on `+0x368`/`+0x36c`): the object `guid`'s
    /// health moved to `health` of `max_health`, inside the notify pass of the packet that wrote
    /// it. Each tooltip showing the unit whose bar last saw another health is seeded again.
    pub fn tooltip_unit_health(&mut self, guid: u64, health: u32, max_health: u32) {
        if guid != 0 {
            on_unit_health(&self.lua, guid, health, max_health);
        }
    }

    /// `RefreshTooltipForUnit` (`0x4919d0`): while `GameTooltip` (`[0xb4b3c4]`) still shows
    /// `guid`, its remembered unit (`+0x368`/`+0x36c`, `0x4919dc`-`0x4919ec`), which every clear
    /// zeroes, the unit builder runs again over that guid (`0x4919f2` → `0x529fe0`): the clear,
    /// the lines read as the call runs, the show. No anchor script and no event: those are the
    /// publisher's alone (`0x49299a`, `0x4929b3`). Whether it rebuilt.
    pub fn tooltip_unit_refresh(&mut self, guid: u64) -> bool {
        let id = {
            let mut model = self.model_mut();
            let Some(h) = model.arena.lookup("GameTooltip") else {
                return false;
            };
            let shows = matches!(
                model.arena.frame(h).map(|f| &f.kind_state),
                Some(KindState::Tooltip(t)) if guid != 0 && t.unit_guid == Some(guid)
            );
            if !shows {
                return false;
            }
            model.frame_id(h)
        };
        let rebuilt = super::object::frame_wrapper(&self.lua, id)
            .and_then(|wrapper| render_subject(&self.lua, &wrapper, Subject::Guid(guid)));
        match rebuilt {
            Ok(shown) => shown,
            Err(e) => {
                self.push_error(e);
                false
            }
        }
    }

    /// Show the world-mouseover unit tooltip: fire `OnTooltipSetDefaultAnchor`, render `token`,
    /// fire `UPDATE_MOUSEOVER_UNIT`. Call on a hover-target change; the render's show returns a
    /// fading plate to full alpha. `false` when there is no tooltip frame or no such unit.
    pub fn world_tooltip_unit(&mut self, token: &str) -> bool {
        let id = {
            let mut model = self.model_mut();
            let Some(h) = model.arena.lookup("GameTooltip") else {
                return false;
            };
            model.frame_id(h)
        };
        if let Err(e) =
            super::event::fire_widget_handler(&self.lua, id, "OnTooltipSetDefaultAnchor", vec![])
        {
            self.push_error(e);
        }
        let wrapper = match super::object::frame_wrapper(&self.lua, id) {
            Ok(w) => w,
            Err(e) => {
                self.push_error(e);
                return false;
            }
        };
        let shown = match render_unit(&self.lua, &wrapper, token) {
            Ok(s) => s,
            Err(e) => {
                self.push_error(e);
                false
            }
        };
        if shown {
            self.fire_event("UPDATE_MOUSEOVER_UNIT", vec![]);
        }
        shown
    }

    /// Show the world-mouseover GameObject tooltip from given content: the backdrop tinted, the
    /// name in gold, then `lines`, seated by `cursor` as [`Self::world_tooltip_object`] seats it.
    pub fn world_tooltip_gameobject(
        &mut self,
        name: &str,
        lines: &[(String, TooltipTint)],
        cursor: bool,
    ) -> bool {
        let Some((h, id)) = self.seat_world_plate(cursor) else {
            return false;
        };
        self.build_plate(h, id, |lua, this, h| {
            tint_object_backdrop(lua, h);
            append_line(lua, this, (name.to_string(), GOLD), None, false)?;
            for (text, tint) in lines {
                append_line(lua, this, (text.clone(), tint_colour(*tint)), None, false)?;
            }
            Ok(Some(true))
        })
    }

    /// The publisher's game object arm (`0x4929f6`) over `guid`: the anchor forks on the object's
    /// `[vtbl+0x5c]`, true (`0x492a01`) owning it by `UIParent` in mode 6, `ANCHOR_CURSOR`
    /// (`0x52ffe0(UIParent, 6, 0, 0)` at `0x492a1e`), which the tooltip's own update then keeps on
    /// the cursor, false (`0x492a42`) firing `OnTooltipSetDefaultAnchor` for the default corner,
    /// as units (`0x492983`) always do; `cursor` is that verdict. Then the builder `0x52aa20`
    /// ([`Self::build_object`]).
    pub fn world_tooltip_object(&mut self, guid: u64, cursor: bool) -> bool {
        let Some((h, id)) = self.seat_world_plate(cursor) else {
            return false;
        };
        self.build_object(h, id, guid)
    }

    /// `CGTooltip::OnLockKeyItem` (`0x52ad10`) on a found key record: while `GameTooltip` keeps a
    /// game object (`+0x370`, `0x52ad34`-`0x52ad44`), the builder again over it (`0x52ad52`), with
    /// no anchor. Whether it rebuilt.
    pub fn tooltip_object_refresh(&mut self) -> bool {
        match self.game_tooltip(|t| t.object_guid) {
            Some((h, id, Some(guid))) => self.build_object(h, id, guid),
            _ => false,
        }
    }

    /// The publisher's corpse arm (`0x492a7d`) over `guid`: `OnTooltipSetDefaultAnchor`
    /// (`0x492a94`), then the builder `0x52aef0` ([`Self::build_corpse`]).
    pub fn world_tooltip_corpse(&mut self, guid: u64) -> bool {
        let Some((h, id)) = self.seat_world_plate(false) else {
            return false;
        };
        self.build_corpse(h, id, guid)
    }

    /// `CGTooltip::OnCorpseOwnerName` (`0x52afe0`) on a found name: while `GameTooltip` keeps a
    /// corpse (`+0x388`, `0x52b004`-`0x52b014`), the builder again over it (`0x52b022`), with no
    /// anchor. Whether it rebuilt.
    pub fn tooltip_corpse_refresh(&mut self) -> bool {
        match self.game_tooltip(|t| t.corpse_guid) {
            Some((h, id, Some(guid))) => self.build_corpse(h, id, guid),
            _ => false,
        }
    }

    /// `CGTooltip::BuildGameObject` (`0x52aa20`) over `guid`, as the host reads it at the call
    /// ([`ObjectPlateQuery`]): the clear (`0x52aa40`); no game object hides the tooltip
    /// (`0x52aa6c`, `0x52aa76`); else the guid kept (`0x52aa8c`), the backdrop tinted, the record's
    /// name in gold (`0x52aac0`), the handler's and the lock lines, and the show (`0x52acfa`).
    fn build_object(&mut self, h: FrameHandle, id: u32, guid: u64) -> bool {
        self.build_plate(h, id, |lua, this, h| {
            let plate = super::live::ask(lua, &ObjectPlateQuery(guid)).flatten();
            let Some(plate) = plate else {
                return Ok(Some(false));
            };
            with_tip(lua, h, |t| t.object_guid = Some(guid));
            tint_object_backdrop(lua, h);
            append_line(lua, this, (plate.name, GOLD), None, false)?;
            for (text, tint) in &plate.lines {
                if let Some(line) = text.line(lua) {
                    append_line(lua, this, (line, tint_colour(*tint)), None, false)?;
                }
            }
            Ok(Some(true))
        })
    }

    /// `CGTooltip::BuildCorpse` (`0x52aef0`) over `guid`, as the host reads it at the call
    /// ([`CorpsePlateQuery`]): the clear (`0x52aefd`); no corpse hides the tooltip (`0x52af2e`);
    /// else the guid kept (`0x52af3e`) and, with the owner's name, `CORPSE_TOOLTIP` in white
    /// (`0x52afb5`) and the show (`0x52afc8`). A name the cache lacks leaves it there, unshown
    /// (`0x52af7e`), for the name's callback.
    fn build_corpse(&mut self, h: FrameHandle, id: u32, guid: u64) -> bool {
        self.build_plate(h, id, |lua, this, h| {
            let owner = super::live::ask(lua, &CorpsePlateQuery(guid)).flatten();
            let Some(owner) = owner else {
                return Ok(Some(false));
            };
            with_tip(lua, h, |t| t.corpse_guid = Some(guid));
            let Some(name) = owner else {
                return Ok(None);
            };
            if let Some(template) = crate::strings::global(lua, "CORPSE_TOOLTIP") {
                let line = fill(&template, &[Arg::S(&name)]);
                append_line(lua, this, (line, WHITE), None, false)?;
            }
            Ok(Some(true))
        })
    }

    /// `GameTooltip`, its id and what `f` reads of its state.
    fn game_tooltip<T>(
        &mut self,
        f: impl FnOnce(&crate::widget::TooltipState) -> T,
    ) -> Option<(FrameHandle, u32, T)> {
        let mut model = self.model_mut();
        let h = model.arena.lookup("GameTooltip")?;
        let read = match model.arena.frame(h).map(|f| &f.kind_state) {
            Some(KindState::Tooltip(t)) => f(t),
            _ => return None,
        };
        Some((h, model.frame_id(h), read))
    }

    /// Seat `GameTooltip` as a world plate, the cursor or the default-anchor script; `None` without
    /// `GameTooltip` or `UIParent`.
    fn seat_world_plate(&mut self, cursor: bool) -> Option<(FrameHandle, u32)> {
        let (h, id, root) = {
            let mut model = self.model_mut();
            let h = model.arena.lookup("GameTooltip")?;
            let root = model.arena.lookup("UIParent")?;
            (h, model.frame_id(h), model.frame_id(root))
        };
        if cursor {
            set_owner_core(&self.lua, h, Some(root), TooltipAnchor::Cursor, 0.0, 0.0);
        } else if let Err(e) =
            super::event::fire_widget_handler(&self.lua, id, "OnTooltipSetDefaultAnchor", vec![])
        {
            self.push_error(e);
        }
        Some((h, id))
    }

    /// A world builder's run on the tooltip `h`: the clear (`0x530050`), then `fill`, whose
    /// `Some(true)` ends in the show of what it holds, `Some(false)` in the hide of a builder that
    /// found no object (slot `0x84`), and `None` in neither. Whether it shows.
    fn build_plate(
        &mut self,
        h: FrameHandle,
        id: u32,
        fill: impl FnOnce(&Lua, &Table, FrameHandle) -> mlua::Result<Option<bool>>,
    ) -> bool {
        let wrapper = match super::object::frame_wrapper(&self.lua, id) {
            Ok(w) => w,
            Err(e) => {
                self.push_error(e);
                return false;
            }
        };
        clear(&self.lua, h);
        match fill(&self.lua, &wrapper, h) {
            Ok(Some(true)) => {
                show_or_hide_empty(&self.lua, h);
                true
            }
            Ok(Some(false)) => {
                super::visibility::set_shown(&self.lua, h, false);
                false
            }
            Ok(None) => false,
            Err(e) => {
                self.push_error(e);
                false
            }
        }
    }
    /// Show the minimap blip tooltip, from `Minimap`'s own hover probe `0x4eb0c0`: owned by
    /// `Minimap` itself (`0x4eb19c` keeps `this + 0x24`, pushed at `0x4eb57d`) in mode 6,
    /// `ANCHOR_CURSOR` (`0x4eb587`), so the tooltip's own update keeps it on the cursor; `text`, the
    /// names of everything under the cursor joined by `\n`, as one gold line (`0x530380`); then the
    /// show (`0x4eb85f`/`0x4eb869`). The text is held until the probe finds nothing
    /// ([`Self::minimap_blip_lost`]) or misses the Minimap.
    pub fn minimap_tooltip(&mut self, text: &str) -> bool {
        let (h, id, root) = {
            let mut model = self.model_mut();
            let Some(h) = model.arena.lookup("GameTooltip") else {
                return false;
            };
            let Some(root) = model.arena.lookup("Minimap") else {
                return false;
            };
            (h, model.frame_id(h), model.frame_id(root))
        };
        let wrapper = match super::object::frame_wrapper(&self.lua, id) {
            Ok(w) => w,
            Err(e) => {
                self.push_error(e);
                return false;
            }
        };
        set_owner_core(&self.lua, h, Some(root), TooltipAnchor::Cursor, 0.0, 0.0);
        if let Err(e) = append_line(&self.lua, &wrapper, (text.to_string(), GOLD), None, false) {
            self.push_error(e);
            return false;
        }
        self.model_mut().minimap_blip = Some(text.to_string());
        show_or_hide_empty(&self.lua, h);
        true
    }
}

/// Register the unit content channel into the GameTooltip kind method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:SetUnit(token) → 1 or nil, which `UnitFrame_OnEnter` reads to arm its re-poll.
    m.set(
        "SetUnit",
        lua.create_function(|lua, (this, token): (Table, Value)| {
            frame_handle_of(lua, &this)?;
            // `is-number-or-string` else `Usage: SetUnit("unit")` (`0x534a42`, `0x84f284`).
            let token = super::binding_abi::string_arg(lua, token, r#"Usage: SetUnit("unit")"#)?;
            // The token gate every `Unit*` verb uses (`0x515970`): an unknown token raises. Not
            // in `render_unit`, which the app calls with canonical tokens.
            crate::script::unit::check_unit_token(&Some(token.clone()))?;
            let ok = render_unit(lua, &this, &token)?;
            Ok(if ok {
                mlua::Value::Integer(1)
            } else {
                mlua::Value::Nil
            })
        })?,
    )?;
    Ok(())
}
