//! The engine unit tooltip, in `0x529fe0`'s line order: the name (gold; FrameXML recolours it by
//! reaction), the creature subtitle, the level line, the faction name, then "PvP", "Skinnable",
//! "Civilian" and "Leader", with health on the `<name>StatusBar` child. The world mouseover drives
//! it through [`super::UiScript::world_tooltip_unit`]; unit-frame hovers call `SetUnit` from Lua.

use mlua::{Lua, Table};

use super::object::frame_handle_of;
use super::tooltip::{append_line, clear_content, fire_cleared, show_or_hide_empty, tip_mut};
use super::unit::{is_civilian_kill, level_reads_unknown, unknownobject};
use super::{KindState, Model, UnitState};
use crate::layout::{Anchor, Point};
use crate::strings::{fill, Arg};
use crate::widget::FrameHandle;

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
    } else if u.reaction != 0 && u.reaction <= 4 {
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

/// Render the unit tooltip for `token`'s current snapshot; returns whether the unit existed.
fn render_unit(lua: &Lua, this: &Table, token: &str) -> mlua::Result<bool> {
    let h = frame_handle_of(lua, this)?;
    let (unit, player_level) = {
        let model = lua.app_data_mut::<Model>().expect("model app_data");
        (
            model.unit(token).cloned().filter(|u| u.exists),
            model.player_req.level,
        )
    };
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        clear_content(&mut model, h);
    }
    fire_cleared(lua, h);
    let Some(u) = unit else {
        set_live_token(lua, h, None);
        show_or_hide_empty(lua, h);
        return Ok(false);
    };
    // The name: `CGUnit_C::GetUnitName` `0x609210` (`0x52a187`), whose every miss falls to
    // `UNKNOWNOBJECT`, as `UnitName`'s does. A token with no object never reaches the builder
    // (`0x468460`): no plate, and `SetUnit` answers nil.
    let title = match &u.name {
        Some(n) => n.clone(),
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
    set_live_token(lua, h, Some(token.to_string()));
    update_bar(lua, h, Some(&u));
    show_or_hide_empty(lua, h);
    Ok(true)
}

/// Mark the tooltip as world-hover-owned (the fade-on-loss gate).
fn set_world_owned(lua: &Lua, h: FrameHandle) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if let Some(f) = model.arena.frame_mut(h) {
        if let KindState::Tooltip(t) = &mut f.kind_state {
            t.world_owned = true;
        }
    }
}

/// Remember (or drop) the tooltip's live unit token, the health watcher's key.
fn set_live_token(lua: &Lua, h: FrameHandle, token: Option<String>) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if let Some(f) = model.arena.frame_mut(h) {
        if let KindState::Tooltip(t) = &mut f.kind_state {
            t.unit_token = token;
        }
    }
}

/// Drive the `<name>StatusBar` child from a unit snapshot (`None` hides it); FrameXML's
/// `HealthBar_OnValueChanged` colours the fill.
fn update_bar(lua: &Lua, h: FrameHandle, unit: Option<&UnitState>) {
    let bar = {
        let model = lua.app_data_mut::<Model>().expect("model app_data");
        let name = model.arena.frame(h).and_then(|f| f.name.clone());
        name.and_then(|n| model.arena.lookup(&format!("{n}StatusBar")))
    };
    let Some(bar) = bar else { return };
    let (changed, shown) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let Some(f) = model.arena.frame_mut(bar) else {
            return;
        };
        match (&mut f.kind_state, unit) {
            (KindState::StatusBar(sb), Some(u)) => {
                sb.min = 0.0;
                sb.max = u.max_health.max(1) as f32;
                let v = u.health.min(u.max_health.max(1)) as f32;
                let changed = (sb.value - v).abs() > f32::EPSILON;
                sb.value = v;
                (changed, true)
            }
            _ => (false, false),
        }
    };
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.arena.set_shown(bar, shown);
    }
    if changed {
        let (id, value) = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let v = match model.arena.frame(bar).map(|f| &f.kind_state) {
                Some(KindState::StatusBar(sb)) => sb.value,
                _ => 0.0,
            };
            (model.frame_id(bar), v)
        };
        if let Err(e) = super::event::fire_widget_handler(
            lua,
            id,
            "OnValueChanged",
            vec![mlua::Value::Number(f64::from(value))],
        ) {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .record_script_error(e.to_string());
        }
    }
}

/// A `set_unit` push for a tooltip's live token updates its health bar without rebuilding the
/// lines, as the reference's HEALTH field watcher does.
pub(super) fn on_unit_push(lua: &Lua, token: &str) {
    let hits: Vec<FrameHandle> = {
        let model = lua.app_data_mut::<Model>().expect("model app_data");
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
                        Some(KindState::Tooltip(t)) if t.unit_token.as_deref() == Some(token)
                    )
            })
            .collect()
    };
    for h in hits {
        let unit = {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            model.unit(token).cloned().filter(|u| u.exists)
        };
        update_bar(lua, h, unit.as_ref());
    }
}

impl super::UiScript {
    /// Show the world-mouseover unit tooltip: fire `OnTooltipSetDefaultAnchor`, render `token`,
    /// fire `UPDATE_MOUSEOVER_UNIT`. Call on a hover-target change; a re-show mid-fade returns to
    /// full alpha. `false` when there is no tooltip frame or no such unit.
    pub fn world_tooltip_unit(&mut self, token: &str) -> bool {
        let h = {
            let mut model = self.model_mut();
            let Some(h) = model.arena.lookup("GameTooltip") else {
                return false;
            };
            let id = model.frame_id(h);
            (h, id)
        };
        let (h, id) = h;
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
            set_world_owned(&self.lua, h);
            let mut model = self.model_mut();
            if let Some(f) = model.arena.frame_mut(h) {
                if let KindState::Tooltip(t) = &mut f.kind_state {
                    if t.fade_start.take().is_some() {
                        model.arena.set_alpha(h, 1.0);
                    }
                }
            }
            drop(model);
            self.fire_event("UPDATE_MOUSEOVER_UNIT", vec![]);
        }
        shown
    }

    /// Show the world-mouseover GameObject tooltip (builder `0x52aa20`): the name in gold, then
    /// the caller's lock lines. The anchor forks on the object's `[vtbl+0x5c]`: true (`0x492a01`)
    /// seats it at the cursor, anchor state 6 (`0x52ffe0(owner, 6, 0, 0)`); false (`0x492a42`)
    /// keeps the default corner, as units (`0x492983`) always do. `cursor` is that verdict, and
    /// [`Self::world_tooltip_move`] follows a cursor-seated plate. Hover loss arms the unit flow's
    /// fade, where the reference's loss path (`0x530ae0`) hides an anchor-state-6 plate at once.
    pub fn world_tooltip_gameobject(
        &mut self,
        name: &str,
        lines: &[(String, TooltipTint)],
        cursor: Option<(f32, f32)>,
    ) -> bool {
        let (h, id, root, root_id) = {
            let mut model = self.model_mut();
            let Some(h) = model.arena.lookup("GameTooltip") else {
                return false;
            };
            let Some(root) = model.arena.lookup("UIParent") else {
                return false;
            };
            let (id, root_id) = (model.frame_id(h), model.frame_id(root));
            (h, id, root, root_id)
        };
        match cursor {
            // The cursor arm: centred above the pointer; compare-then-touch, so a still pointer
            // never re-layouts.
            Some((ui_x, ui_y)) => {
                let mut model = self.model_mut();
                // The owner: this arm is the SetOwner core (`0x52ffe0`), which stores it at
                // `+0x314` (`0x53000c`); the other arms get theirs from FrameXML's
                // `OnTooltipSetDefaultAnchor`. Without one, any Lua `GameTooltip:Show()` hides the
                // plate: `0x530a80` shows only when `+0x314` and `+0x31c` are both set, else it
                // takes `0x530a60`.
                if let Ok(t) = tip_mut(&mut model, h) {
                    t.owner = Some(root);
                }
                let input = model.layout_inputs.entry(h).or_default();
                let new = Anchor::new(Point::Bottom, root_id, Point::BottomLeft, ui_x, ui_y);
                let same = input.anchors.len() == 1
                    && super::object::anchor_bits_eq(&input.anchors[0], &new);
                if !same {
                    input.anchors = vec![new];
                    model.touch_layout();
                }
            }
            // The corner arm: the default-anchor handler the unit flow fires too.
            None => {
                if let Err(e) = super::event::fire_widget_handler(
                    &self.lua,
                    id,
                    "OnTooltipSetDefaultAnchor",
                    vec![],
                ) {
                    self.push_error(e);
                }
            }
        }
        let wrapper = match super::object::frame_wrapper(&self.lua, id) {
            Ok(w) => w,
            Err(e) => {
                self.push_error(e);
                return false;
            }
        };
        let render = || -> mlua::Result<()> {
            {
                let mut model = self.lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(&self.lua, h);
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
        set_world_owned(&self.lua, h);
        {
            let mut model = self.model_mut();
            if let Some(f) = model.arena.frame_mut(h) {
                if let KindState::Tooltip(t) = &mut f.kind_state {
                    if t.fade_start.take().is_some() {
                        model.arena.set_alpha(h, 1.0);
                    }
                }
            }
        }
        show_or_hide_empty(&self.lua, h);
        true
    }

    /// Show the minimap blip tooltip (an `AreaPOI` or quest-dot name): one gold line, faint gold
    /// for a cross-interior dot, its bottom at the given UI point above the cursor.
    /// [`Self::world_tooltip_move`] follows the pointer, and hover loss arms
    /// [`Self::world_tooltip_fade`].
    pub fn minimap_tooltip(&mut self, text: &str, ui_x: f32, ui_y: f32, grey: bool) -> bool {
        let (h, id, root_id) = {
            let mut model = self.model_mut();
            let Some(h) = model.arena.lookup("GameTooltip") else {
                return false;
            };
            let Some(root) = model.arena.lookup("UIParent") else {
                return false;
            };
            let (id, root_id) = (model.frame_id(h), model.frame_id(root));
            (h, id, root_id)
        };
        let wrapper = match super::object::frame_wrapper(&self.lua, id) {
            Ok(w) => w,
            Err(e) => {
                self.push_error(e);
                return false;
            }
        };
        {
            let mut model = self.model_mut();
            clear_content(&mut model, h);
            let input = model.layout_inputs.entry(h).or_default();
            // Anchor only: the frame's own clamp flag (`Frame::clamped_to_screen`, G flags bit 4)
            // slides a plate near the window edge back on screen.
            let new = Anchor::new(Point::Bottom, root_id, Point::BottomLeft, ui_x, ui_y);
            // Compare-then-touch, so a still cursor does not dirty the layout.
            let same =
                input.anchors.len() == 1 && super::object::anchor_bits_eq(&input.anchors[0], &new);
            if !same {
                input.anchors = vec![new];
                model.touch_layout();
            }
        }
        fire_cleared(&self.lua, h);
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
        set_world_owned(&self.lua, h);
        {
            let mut model = self.model_mut();
            if let Some(f) = model.arena.frame_mut(h) {
                if let KindState::Tooltip(t) = &mut f.kind_state {
                    if t.fade_start.take().is_some() {
                        model.arena.set_alpha(h, 1.0);
                    }
                }
            }
        }
        show_or_hide_empty(&self.lua, h);
        true
    }

    /// Re-seat the shown world-owned tooltip at a new pointer point, anchor only; else a no-op.
    pub fn world_tooltip_move(&mut self, ui_x: f32, ui_y: f32) {
        let mut model = self.model_mut();
        let Some(h) = model.arena.lookup("GameTooltip") else {
            return;
        };
        let Some(root) = model.arena.lookup("UIParent") else {
            return;
        };
        let owned_shown = model
            .arena
            .frame(h)
            .map(|f| matches!(&f.kind_state, KindState::Tooltip(t) if t.world_owned) && f.shown)
            .unwrap_or(false);
        if !owned_shown {
            return;
        }
        let root_id = model.frame_id(root);
        let input = model.layout_inputs.entry(h).or_default();
        let new = Anchor::new(Point::Bottom, root_id, Point::BottomLeft, ui_x, ui_y);
        // Compare-then-touch: this runs on every pointer event.
        let same =
            input.anchors.len() == 1 && super::object::anchor_bits_eq(&input.anchors[0], &new);
        if !same {
            input.anchors = vec![new];
            model.touch_layout();
        }
    }

    /// Whether `GameTooltip` is still the shown plate a world hover put up, with no Lua
    /// `SetOwner`, content `Set*` or `Hide` since.
    pub fn world_tooltip_up(&mut self) -> bool {
        let model = self.model_mut();
        let Some(h) = model.arena.lookup("GameTooltip") else {
            return false;
        };
        model
            .arena
            .frame(h)
            .map(|f| matches!(&f.kind_state, KindState::Tooltip(t) if t.world_owned) && f.shown)
            .unwrap_or(false)
    }

    /// On hover loss, arm the world tooltip's timed fade (the reference's unit plate fades rather
    /// than hiding); a no-op unless a world-owned plate is shown.
    pub fn world_tooltip_fade(&mut self) {
        let now = self.now();
        let mut model = self.model_mut();
        let Some(h) = model.arena.lookup("GameTooltip") else {
            return;
        };
        if let Some(f) = model.arena.frame_mut(h) {
            if let KindState::Tooltip(t) = &mut f.kind_state {
                if t.world_owned && f.shown && t.fade_start.is_none() {
                    t.fade_start = Some(now);
                }
            }
        }
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
