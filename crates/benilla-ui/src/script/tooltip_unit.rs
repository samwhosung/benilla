//! The engine unit tooltip, in `0x529fe0`'s line order: the name (gold, decorated by `0x609370`
//! as `UnitPVPName`'s is; FrameXML recolours it by reaction), the creature subtitle, the level
//! line, the faction name, then "PvP", "Skinnable", "Civilian" and "Leader", with health on the
//! `<name>StatusBar` child. The world mouseover drives it through
//! [`super::UiScript::world_tooltip_unit`]; unit-frame hovers call `SetUnit` from Lua.

use mlua::{Lua, Table};

use super::object::frame_handle_of;
use super::tooltip::{append_line, clear, set_owner_core, show_or_hide_empty};
use super::unit::{is_civilian_kill, level_reads_unknown, unknownobject};
use super::{KindState, Model, UnitState};
use crate::strings::{fill, Arg};
use crate::widget::{FrameHandle, TooltipAnchor};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
/// `0xc0d420` = `0xff40c040`, the satisfiable-lock green.
const LOCK_OPEN: [f32; 4] = [64.0 / 255.0, 192.0 / 255.0, 64.0 / 255.0, 1.0];
/// `0xffffd200`, the unit name's colour until FrameXML recolours it by reaction.
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];

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
fn level_line(lua: &Lua, u: &UnitState, player_level: u32) -> Option<String> {
    let get = |key: &str| crate::strings::global(lua, key);
    // "??" by the gate `UnitLevel`'s -1 shares (`0x529fe0`), so tooltip and target frame agree.
    let level_text = if level_reads_unknown(u, player_level) {
        "??".to_string()
    } else {
        u.level.to_string()
    };
    // `CORPSE` when dead, "Race Class" for a player, the creature type only if hostile or neutral.
    let class_slot = if u.dead {
        get("CORPSE")
    } else if u.is_player {
        match (&u.race, &u.class) {
            (Some(r), Some(c)) => Some(format!("{r} {c}")),
            (Some(r), None) => Some(r.clone()),
            (None, Some(c)) => Some(c.clone()),
            (None, None) => None,
        }
    } else if u.reaction != 0 && u.reaction <= 4 && u.creature_type != CREATURE_TYPE_NOT_SPECIFIED {
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

/// Render the unit tooltip for `token`'s current snapshot (`0x529fe0`); returns whether the unit
/// existed. The clear first (`0x52a003`); no unit hides the tooltip through its own hide, shown bit
/// then slot (`0x52a050`/`0x52a058`). A unit is held (`0x52a070`), its bar seeded and shown
/// (`0x52a0e0`–`0x52a146`) before any line, and the lines end in the show (`0x52a711`/`0x52a71b`).
fn render_unit(lua: &Lua, this: &Table, token: &str) -> mlua::Result<bool> {
    let h = frame_handle_of(lua, this)?;
    // A unit is its object, found by guid (`0x52a021`); guid 0 finds none.
    let (unit, player_level) = {
        let model = lua.app_data_mut::<Model>().expect("model app_data");
        (
            model
                .unit(token)
                .cloned()
                .filter(|u| u.exists && u.guid != 0),
            model.player_req.level,
        )
    };
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
    if let Some(level) = level_line(lua, &u, player_level) {
        append_line(lua, this, (level, WHITE), None, false)?;
    }
    // The faction name, between the level line and "PvP" (`0x52a7a0`); the app applied its gates.
    if let Some(faction) = &u.faction_name {
        append_line(lua, this, (faction.clone(), WHITE), None, false)?;
    }
    if u.pvp {
        append_line(lua, this, ("PvP".into(), WHITE), None, false)?;
    }
    if u.skinnable {
        append_line(lua, this, ("Skinnable".into(), RED), None, false)?;
    }
    // "Civilian" (`0x612550`): PvP-flagged, a civilian, hostile, and grey to kill (`0x5f0700`).
    if is_civilian_kill(&u, player_level) {
        append_line(lua, this, ("Civilian".into(), GREEN), None, false)?;
    }
    // "Leader" (`0x6125c0`): PvP-flagged and a racial leader, no other gate.
    if u.racial_leader && u.pvp {
        append_line(lua, this, ("Leader".into(), WHITE), None, false)?;
    }
    show_or_hide_empty(lua, h);
    Ok(true)
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

/// Drive the `<name>StatusBar` child from a unit snapshot, as the builder (`0x52a0eb` range,
/// `0x52a12e` value) and the health watcher (`0x529560`) do: the raw maximum and health through
/// [`super::statusbar::set_health`], firing each `OnValueChanged` it makes; FrameXML's
/// `HealthBar_OnValueChanged` colours the fill. Neither shows it. Returns the bar.
fn seed_bar(lua: &Lua, h: FrameHandle, u: &UnitState) -> Option<FrameHandle> {
    let (bar, fires) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let bar = super::tooltip::status_bar(&model, h)?;
        if let Some(KindState::Tooltip(t)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            t.unit_health = u.health;
        }
        let fires =
            super::statusbar::set_health(&mut model, bar, u.max_health as f32, u.health as f32);
        (bar, fires)
    };
    for value in fires.into_iter().flatten() {
        super::statusbar::fire_engine_value_changed(lua, bar, value);
    }
    Some(bar)
}

/// A `set_unit` push of the unit a tooltip shows updates its health bar without rebuilding the
/// lines, as the reference's HEALTH field watcher does: it is keyed on the unit's guid (`0x52a0a8`
/// registers `0x529560` on `+0x368`/`+0x36c`), so a token that stops naming the unit, as
/// `"mouseover"` does when the hover ends, leaves a fading plate's bar standing. It runs only
/// when the health differs from the bytes it last saw (`0x4655bb`), not for a maximum alone.
pub(super) fn on_unit_push(lua: &Lua, token: &str) {
    let (unit, hits): (UnitState, Vec<FrameHandle>) = {
        let model = lua.app_data_mut::<Model>().expect("model app_data");
        let Some(unit) = model.pushed_unit(token).filter(|u| u.exists && u.guid != 0) else {
            return;
        };
        // Registered frames only, as `update_bar` writes layout.
        let hits: Vec<FrameHandle> = model
            .arena
            .tooltip_kinds()
            .iter()
            .copied()
            .filter(|&h| {
                model.frame_to_id.contains_key(&h)
                    && matches!(
                        model.arena.frame(h).map(|f| &f.kind_state),
                        Some(KindState::Tooltip(t))
                            if t.unit_guid == Some(unit.guid) && t.unit_health != unit.health
                    )
            })
            .collect();
        if hits.is_empty() {
            return;
        }
        (unit.clone(), hits)
    };
    for h in hits {
        seed_bar(lua, h, &unit);
    }
}

impl super::UiScript {
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

    /// Show the world-mouseover GameObject tooltip (builder `0x52aa20`): the name in gold, then
    /// the caller's lock lines. The anchor forks on the object's `[vtbl+0x5c]`: true (`0x492a01`)
    /// owns it by `UIParent` in mode 6, `ANCHOR_CURSOR` (`0x52ffe0(UIParent, 6, 0, 0)` at
    /// `0x492a1e`), which the tooltip's own update then keeps on the cursor; false (`0x492a42`)
    /// fires `OnTooltipSetDefaultAnchor` for the default corner, as units (`0x492983`) always do.
    /// `cursor` is that verdict.
    pub fn world_tooltip_gameobject(
        &mut self,
        name: &str,
        lines: &[(String, TooltipTint)],
        cursor: bool,
    ) -> bool {
        let (h, id, root) = {
            let mut model = self.model_mut();
            let Some(h) = model.arena.lookup("GameTooltip") else {
                return false;
            };
            let Some(root) = model.arena.lookup("UIParent") else {
                return false;
            };
            (h, model.frame_id(h), root)
        };
        if cursor {
            set_owner_core(&self.lua, h, Some(root), TooltipAnchor::Cursor, 0.0, 0.0);
        } else if let Err(e) =
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
        let render = || -> mlua::Result<()> {
            clear(&self.lua, h);
            append_line(&self.lua, &wrapper, (name.to_string(), GOLD), None, false)?;
            for (text, tint) in lines {
                let colour = match tint {
                    TooltipTint::White => WHITE,
                    TooltipTint::Red => RED,
                    TooltipTint::LockOpen => LOCK_OPEN,
                };
                append_line(&self.lua, &wrapper, (text.clone(), colour), None, false)?;
            }
            Ok(())
        };
        if let Err(e) = render() {
            self.push_error(e);
            return false;
        }
        show_or_hide_empty(&self.lua, h);
        true
    }

    /// Show the minimap blip tooltip (an `AreaPOI` or quest-dot name), from `Minimap`'s own hover
    /// probe `0x4eb0c0`: owned by `Minimap` itself (`0x4eb19c` keeps `this + 0x24`, pushed at
    /// `0x4eb57d`) in mode 6, `ANCHOR_CURSOR` (`0x4eb587`), so the tooltip's own update keeps it on
    /// the cursor; one gold line, faint gold for a cross-interior dot; then the show
    /// (`0x4eb85f`/`0x4eb869`). The blip is held until the probe finds none
    /// ([`Self::minimap_blip_lost`]) or misses the Minimap.
    pub fn minimap_tooltip(&mut self, text: &str, grey: bool) -> bool {
        let (h, id, root) = {
            let mut model = self.model_mut();
            let Some(h) = model.arena.lookup("GameTooltip") else {
                return false;
            };
            let Some(root) = model.arena.lookup("Minimap") else {
                return false;
            };
            (h, model.frame_id(h), root)
        };
        let wrapper = match super::object::frame_wrapper(&self.lua, id) {
            Ok(w) => w,
            Err(e) => {
                self.push_error(e);
                return false;
            }
        };
        set_owner_core(&self.lua, h, Some(root), TooltipAnchor::Cursor, 0.0, 0.0);
        // A cross-interior entry's `|cffb0b0b0` wrap modulates the gold: faint gold, not grey.
        let dim = 0xb0 as f32 / 255.0;
        let color = if grey {
            [GOLD[0] * dim, GOLD[1] * dim, GOLD[2] * dim, 1.0]
        } else {
            GOLD
        };
        if let Err(e) = append_line(&self.lua, &wrapper, (text.to_string(), color), None, false) {
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
        lua.create_function(|lua, (this, token): (Table, String)| {
            // The token gate every `Unit*` verb uses: an unknown token raises. Not in
            // `render_unit`, which the app calls with canonical tokens.
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
