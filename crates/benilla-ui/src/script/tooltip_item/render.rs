//! The item tooltip's line law, [`render_view`]: the emission order of the reference's builder
//! `0x52b650`. Every sentence is a key the builder itself names (the string pointers reachable
//! from `[0x52b650, 0x52e600)`), never a match on English: `ITEM_REQ_SKILL` and the three
//! `LOCKED_WITH_*` keys all read "Requires %s" in enUS and differ elsewhere. A key the player's
//! `GlobalStrings.lua` lacks emits no line, as in the reference, whose `FrameScript_GetText`
//! returns the empty string and whose `AddLine` drops an empty row uncounted.

use mlua::{Lua, Table};

use crate::script::tooltip::{append_line, duration_text, plural_template};
use crate::script::{ItemTemplateView, Model};
use crate::strings::{fill, Arg};

use super::names::*;

/// What a source knows beyond the template: an item object's fields, or a block source's name and
/// roll. A template source passes `None`.
#[derive(Default)]
pub(super) struct ItemInstance {
    /// The name with its random suffix joined, which the reference composes in the builder
    /// (`0x52b7bf`, `0x5d8b00` through `ITEM_SUFFIX_TEMPLATE`); `None` takes the template's.
    pub name: Option<String>,
    /// Live `(current, max)`; `None` for an indestructible item or one not yet streamed.
    pub durability: Option<(u32, u32)>,
    /// The resolved `ITEM_FIELD_CREATOR` name (the reference's `0x55f080`); `None` while it is
    /// unresolved, which prints no line either (`0x52e209`).
    pub creator: Option<String>,
    /// `ITEM_FIELD_ITEM_TEXT_ID` ≠ 0, a letter's copy: the creator line reads WRITTEN_BY
    /// (`0x52e223`) and READABLE shows (`0x52e348`).
    pub has_text: bool,
    /// `ITEM_FIELD_FLAGS`: UNLOCKED `0x4` (read at `0x52e30c`), WRAPPED `0x8` (`0x52e31d`).
    pub flags: u32,
    /// Runtime-bound (`0x5da2c0`): `ITEM_FIELD_FLAGS & 1`, or an enchant that binds. App-resolved,
    /// since [`Self::enchants`] drops rows the line law hides.
    pub already_bound: bool,
    /// The petition a charter names, printed between the name and `ITEM_SIGNABLE`.
    pub petition: Option<crate::script::PetitionSlotView>,
    /// The enchant slots, app-resolved, in slot order (`0x52c991`).
    pub enchants: Vec<crate::script::EnchantView>,
    /// The reference's p6 = 0, no caller-supplied instance block (`[this+0x440]`, tested at
    /// `0x52e2e8`; when set, only READABLE is evaluated). It is per call site, not per binding:
    /// `SetBagItem` (`0x534620`) takes p6=1 (`0x534900`) iff the cooldown query `0x6e2ed0` at
    /// `0x53483a` finds one running, else p6=0 (`0x53493e`), so the open line and the cooldown
    /// line never show together there.
    pub openable_source: bool,
    /// A timed item's remaining lifetime in ms, from `SMSG_ITEM_TIME_UPDATE`, not
    /// `ITEM_FIELD_DURATION`: the client displays from the packet (`Objects/Item.cpp:1093`).
    pub duration_ms: Option<u64>,
}

/// The set block's blank gold line, the reference's literal `0x854b2c`: a space and a newline. An
/// empty string would be no row, as `AddLine` (`0x530270`) drops an empty left text uncounted
/// (`0x5302a9`). `" \n"` is one row: the stepper `0x5c7470` consumes the break with the space
/// (`0x5c7659`), as the app's `fontstring_lines` trailing-break rule does.
const SET_SPACER: &str = " \n";

/// The builder's two independent render flags, p4 and p5 of `0x52b650`.
#[derive(Clone, Copy, Default)]
pub(super) struct BuilderFlags {
    /// p5 `[arg+0x18]`: prepend the gray `CURRENTLY_EQUIPPED` line. Only the two compare bindings
    /// set it, and both pass [`name_only`](Self::name_only) zero.
    pub currently_equipped: bool,
    /// p4 `[arg+0x14]`, the compact mode the reference calls `nameOnly` (`0x8552dc`), reachable
    /// only through `SetInventoryItem`'s third argument. The name goes white, the bind/lock region
    /// and the stat body are cut, and all after the cooldown line is skipped; the slot/type cell,
    /// durability, duration, requirements, spell triggers and set block still print.
    pub name_only: bool,
}

/// Render one item into the tooltip in `0x52b650`'s order. Not built: ITEM_COOLDOWN_TIME,
/// ITEM_WRAPPED_BY and the `LOCKED_WITH_*` line under LOCKED.
pub(super) fn render_view(
    lua: &Lua,
    this: &Table,
    v: &ItemTemplateView,
    flags: BuilderFlags,
    // `None`: a template source, with no item object and no instance block.
    inst: Option<&ItemInstance>,
) -> mlua::Result<()> {
    let (req, known_spell, taught_known, set_view, equipped) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let knows = |id: u32| model.spellbook.slots.iter().any(|s| s.spell_id == id);
        let known = v.required_spell != 0 && knows(v.required_spell);
        // A taught spell (trigger 6, learn) the player knows: the always-red ITEM_SPELL_KNOWN.
        let taught = v
            .spell_triggers
            .iter()
            .any(|&(t, id, _)| t == 6 && id != 0 && knows(id));
        // The set view (a miss records the ask) and the equipped ids the set block counts.
        let set_view = (v.item_set != 0)
            .then(|| {
                let view = model.item_sets.get(&v.item_set).cloned();
                if view.is_none() {
                    model.item_set_asks.insert(v.item_set);
                }
                view
            })
            .flatten();
        let equipped: std::collections::HashSet<u32> = model
            .inventory_slots
            .iter()
            .flatten()
            .map(|s| s.item_id)
            .collect();
        (model.player_req.clone(), known, taught, set_view, equipped)
    };
    let add = |l: (String, [f32; 4])| append_line(lua, this, l, None, false);
    let addw = |l: (String, [f32; 4])| append_line(lua, this, l, None, true);
    let add2 =
        |l: (String, [f32; 4]), r: (String, [f32; 4])| append_line(lua, this, l, Some(r), false);
    let get = |key: &str| crate::strings::global(lua, key);
    let keyed = |key: &str, args: &[Arg<'_>], color: [f32; 4], wrap: bool| -> mlua::Result<()> {
        match get(key) {
            Some(t) => append_line(lua, this, (fill(&t, args), color), None, wrap),
            None => Ok(()),
        }
    };

    // The gray header (`[arg+0x18]`, colour `0xc0d3c4`) is all a compare adds: the only two of
    // `0x52b650`'s 31 call sites passing p5 (`0x5362d4`, `0x53603e`) pass p4 = 0, so none is cut.
    if flags.currently_equipped {
        keyed("CURRENTLY_EQUIPPED", &[], GRAY, false)?;
    }
    let name = inst
        .and_then(|i| i.name.clone())
        .unwrap_or_else(|| v.name.clone());
    // `nameOnly` paints the name white (`0x52b8b3`), else the quality arm colours it (`0x52b8ca`);
    // nothing else recolors it, not even for an unusable item.
    let name_color = if flags.name_only {
        WHITE
    } else {
        quality_color(v.quality)
    };
    add((name, name_color))?;
    // The petition lines, below the name: a charter's `GUILD_CHARTER_*` pair or a petition's
    // `PETITION_*`, by the record's charter bit (`GetPetitionInfo`'s first return). A line whose
    // source is unresolved waits for the repaint, as the creator line does.
    if let Some(p) = inst.and_then(|i| i.petition.as_ref()) {
        let (title_key, creator_key) = if p.is_charter {
            ("GUILD_CHARTER_TITLE", "GUILD_CHARTER_CREATOR")
        } else {
            ("PETITION_TITLE", "PETITION_CREATOR")
        };
        if !p.title.is_empty() {
            keyed(title_key, &[Arg::S(&p.title)], WHITE, false)?;
        }
        if let Some(owner) = p.owner.as_deref().filter(|o| !o.is_empty()) {
            keyed(creator_key, &[Arg::S(owner)], WHITE, false)?;
        }
    }
    // ITEM_SIGNABLE: template flag `0x2000`, a petition.
    if v.flags & 0x2000 != 0 {
        keyed("ITEM_SIGNABLE", &[], GREEN, false)?;
    }
    // p4's first cut (`0x52babe`, skipping `[0x52bac9, 0x52bfad)` at `0x52bac3`): the bind/lock
    // region. ITEM_SIGNABLE above survives it.
    if !flags.name_only {
        if v.flags & 0x2 != 0 {
            keyed("ITEM_CONJURED", &[], WHITE, false)?;
        }
        // Bonding (`[record+0x194]`) 1..5 decides whether a bind line prints. A runtime-bound
        // instance (`0x5da2c0`) prints ITEM_SOULBOUND, or ITEM_BIND_QUEST for the quest kinds;
        // otherwise the jump table `0x52e4fc` picks 1 pickup, 2 equip, 3 use, 4 and 5 quest.
        match v.bonding {
            4 | 5 => keyed("ITEM_BIND_QUEST", &[], WHITE, false)?,
            1..=3 if inst.is_some_and(|i| i.already_bound) => {
                keyed("ITEM_SOULBOUND", &[], WHITE, false)?
            }
            1 => keyed("ITEM_BIND_ON_PICKUP", &[], WHITE, false)?,
            2 => keyed("ITEM_BIND_ON_EQUIP", &[], WHITE, false)?,
            3 => keyed("ITEM_BIND_ON_USE", &[], WHITE, false)?,
            _ => {}
        }
        match v.max_count {
            1 => keyed("ITEM_UNIQUE", &[], WHITE, false)?,
            n if n > 1 => keyed("ITEM_UNIQUE_MULTIPLE", &[Arg::D(n.into())], WHITE, false)?,
            _ => {}
        }
        if v.start_quest != 0 {
            keyed("ITEM_STARTS_QUEST", &[], WHITE, false)?;
        }
        // LOCKED, red, until the instance carries UNLOCKED `0x4`.
        if v.lock_id != 0 && inst.is_none_or(|i| i.flags & 0x4 == 0) {
            keyed("LOCKED", &[], RED, false)?;
        }
    }
    // The slot | type line, or for a bag the CONTAINER_SLOTS line in its seat. The type cell is
    // hidden for cloaks (16) and display-hidden subclasses. The cells recolor independently
    // (`0x52c143..0x52c1f9`): a proficiency miss (`0xc4d4a0[class]` bit `1 << subclass`; a class
    // with no mask never reds) reds the type, or the slot when a weapon's alternate subclass is
    // proficient; an off-hand weapon (22) without Dual Wield (`0x5eab70`) reds the slot.
    // The bag gate is `InventoryType == 18` alone (`0x52b754`, read at `0x52bffe`), never the slot
    // count. Quivers and ammo pouches are 18 too, so 27 is dead (`0x809200[27]` maps to no slot).
    if v.inventory_type == 18 {
        // `%s` is the subclass DisplayName the type cell reads ("8 Slot Quiver"); a subclass
        // without one prints no line at all (`0x52c006`, `0x52c018`, `0x52c021`), and
        // displayFlags bit 0 is not read here.
        if let Some(name) = v.sub_class_display.as_deref() {
            keyed(
                "CONTAINER_SLOTS",
                &[Arg::D(v.container_slots.into()), Arg::S(name)],
                WHITE,
                false,
            )?;
        }
    } else {
        // The slot cell: for `ItemClass == 6`, `ItemClass.dbc`'s own name ("Projectile", read
        // from `[0xc0dc24]` at `0x52c0bc`), the column `GetItemInfo`'s itemType reads; else the
        // `0x83ddb0` key, so a bow reads "Ranged | Bow" and a gun, whose key ships no string,
        // "Gun" alone.
        let slot = if v.class == 6 {
            v.item_type.clone()
        } else {
            invtype_key(v.inventory_type).and_then(&get)
        };
        // The type cell: `ItemSubClass.dbc`'s DisplayName, app-resolved (the builder's `0xc0db90`).
        let ty = if v.inventory_type == 16 || v.hide_subclass {
            None
        } else {
            v.sub_class_display.as_deref()
        };
        let mut left_red = false;
        let mut right_red = false;
        if let Some(&mask) = req.proficiency.get(&v.class) {
            if mask & (1 << v.subclass) == 0 {
                let alt_ok =
                    v.class == 2 && v.proficiency_alt.is_some_and(|a| mask & (1 << a) != 0);
                if alt_ok {
                    left_red = true;
                } else {
                    right_red = true;
                }
            }
        }
        if v.inventory_type == 22 && !req.can_dual_wield {
            left_red = true;
        }
        match (slot, ty) {
            (Some(s), Some(t)) => add2(
                (s, req_color(!left_red)),
                (t.to_string(), req_color(!right_red)),
            )?,
            (Some(s), None) => add((s, req_color(!left_red)))?,
            // No slot name: the type stands alone, keeping its own colour.
            (None, Some(t)) => add((t.to_string(), req_color(!right_red)))?,
            _ => {}
        }
    }
    // p4's second cut (`0x52c220`, skipping `[0x52c22b, 0x52cc5b)` at `0x52c225`): damage through
    // the enchant family. The slot/type cell between the two cuts always prints.
    if !flags.name_only {
        // The damage block, `[0x52c22b, 0x52c5a1)`: per slot, a key picked by school (non-zero),
        // ammo (`ItemClass == 6` alone) and, on the no-school non-ammo leaf only, single
        // (`floor(min) == ceil(max)`); every emitted slot after the first takes `PLUS_`.
        let mut first = true;
        let mut dps_acc = 0.0f32;
        for &(min, max, school) in v.damages.iter().take(5) {
            // The biased floor and ceil (`0x808120`), not rounding: 38.7-85.7 reads "38 - 86".
            let (lo, hi) = (floor_min(min), ceil_max(max));
            // A slot is emitted iff either rounded bound is nonzero (`0x52c292`).
            if lo == 0 && hi == 0 {
                continue;
            }
            // The school number picks the arm; a missing `SPELL_SCHOOL%d_CAP` fills its hole empty.
            let school_name = school_key(school).and_then(|k| get(&k)).unwrap_or_default();
            // The rounded bounds' mean, not per second whatever `AMMO_DAMAGE_TEMPLATE` says: an
            // arrow of 1-2 reads "Adds 1.5 damage per second".
            let avg = f64::from((lo + hi) as f32 * 0.5);
            let plus = |k: &str| {
                if first {
                    k.to_string()
                } else {
                    format!("PLUS_{k}")
                }
            };
            let (key, args): (String, Vec<Arg<'_>>) = if school != 0 {
                if v.class == 6 {
                    (
                        plus("AMMO_SCHOOL_DAMAGE_TEMPLATE"),
                        vec![Arg::F(avg), Arg::S(&school_name)],
                    )
                } else {
                    (
                        plus("DAMAGE_TEMPLATE_WITH_SCHOOL"),
                        vec![Arg::D(lo.into()), Arg::D(hi.into()), Arg::S(&school_name)],
                    )
                }
            } else if v.class == 6 {
                (plus("AMMO_DAMAGE_TEMPLATE"), vec![Arg::F(avg)])
            } else if lo == hi {
                (plus("SINGLE_DAMAGE_TEMPLATE"), vec![Arg::D(lo.into())])
            } else {
                (
                    plus("DAMAGE_TEMPLATE"),
                    vec![Arg::D(lo.into()), Arg::D(hi.into())],
                )
            };
            // Speed rides a weapon's first emitted line only (`0x52c494`, `0x52c49c`): the literal
            // `"%s %.2f"` over `Delay × 0.001`, with only the word a key.
            let speed = (first && v.class == 2).then(|| {
                let word = get("SPEED").unwrap_or_default();
                let secs = f64::from(v.delay_ms) * f64::from(0.001_f32);
                format!("{word} {secs:.2}")
            });
            // White in both cells, even for an unusable item (`0x52c4fa..0x52c516`).
            if let Some(t) = get(&key) {
                let line = (fill(&t, &args), WHITE);
                match speed {
                    Some(s) => add2(line, (s, WHITE))?,
                    None => add(line)?,
                }
            }
            // DPS sums the raw floats of the emitted slots, not the printed bounds.
            dps_acc += (max + min) * 0.5;
            first = false;
        }
        // DPS needs an emitted line and `ItemClass == 2`, so ammunition has none.
        if !first && v.class == 2 {
            // The precision is `DPS_TEMPLATE`'s own (`0x854c4c`); the reference has no
            // divide-by-zero guard either.
            let secs = f64::from(v.delay_ms) * f64::from(0.001_f32);
            keyed(
                "DPS_TEMPLATE",
                &[Arg::F(f64::from(dps_acc) / secs)],
                WHITE,
                false,
            )?;
        }
        if v.armor > 0 {
            keyed("ARMOR_TEMPLATE", &[Arg::D(v.armor.into())], WHITE, false)?;
        }
        if v.block > 0 {
            keyed(
                "SHIELD_BLOCK_TEMPLATE",
                &[Arg::D(v.block.into())],
                WHITE,
                false,
            )?;
        }
        // Stats in the display order of `0x808e88` (4,3,7,5,6,1,0, then 8,9,2,10 and zero padding;
        // `0x52c6b0..0x52c801`), not wire order. Deviation: the reference's padding repeats a mana
        // stat once per zero entry; not emulated, since only the internal test ring 6674 has one.
        const STAT_DISPLAY_ORDER: [u32; 7] = [4, 3, 7, 5, 6, 1, 0];
        for &want in &STAT_DISPLAY_ORDER {
            for &(t, val) in &v.stats {
                if t != want || val == 0 {
                    continue;
                }
                // The sign fills the template's `%c` (`"%c%d Agility"`).
                let Some(key) = stat_key(t) else { continue };
                let sign = if val > 0 { "+" } else { "-" };
                keyed(key, &[Arg::S(sign), Arg::D(val.abs().into())], WHITE, false)?;
            }
        }
        // Six equal non-zero resistances print the ALL line; otherwise one line per non-zero
        // school, Holy excluded. Field `i` is school `i + 1`.
        let first_res = v.resistances[0];
        if first_res != 0 && v.resistances.iter().all(|&r| r == first_res) {
            let sign = if first_res > 0 { "+" } else { "-" };
            keyed(
                "ITEM_RESIST_ALL",
                &[Arg::S(sign), Arg::D(first_res.abs().into())],
                WHITE,
                false,
            )?;
        } else {
            // The builder's order, not field order: `0x52c8ad..0x52c93e` walks 1..5 reading school
            // 6 for 1, so Arcane comes first and Holy is displaced rather than tested.
            const RESIST_EMIT_ORDER: [u32; 5] = [6, 2, 3, 4, 5];
            for school in RESIST_EMIT_ORDER {
                let r = v.resistances[school as usize - 1];
                if r == 0 {
                    continue;
                }
                let sign = if r > 0 { "+" } else { "-" };
                let school = school_key(school).and_then(|k| get(&k)).unwrap_or_default();
                keyed(
                    "ITEM_RESIST_SINGLE",
                    &[Arg::S(sign), Arg::D(r.abs().into()), Arg::S(&school)],
                    WHITE,
                    false,
                )?;
            }
        }
        // The enchant family, `[0x52c991, 0x52cc69)`: the slot loop, or with no id source at all
        // the `ITEM_RANDOM_ENCHANT` placeholder (`0x52cc33`). Slots 0 and 1 are green `0xc0d3ac`
        // for a positive id and red `0xc0d398` for a negative one; the suffix slots 2..6 are always
        // white. A signable item (`0x2000`) zeroes every id (`0x52c9e0`) and shows no enchant line.
        let signable = v.flags & 0x2000 != 0;
        let enchant_slots = match signable {
            true => &[][..],
            false => inst.map(|i| i.enchants.as_slice()).unwrap_or_default(),
        };
        // No id source (`0x52c991`): a wrapped gift, or no item object and no instance block
        // (`+0x440 == 0`), here a hover passing no [`ItemInstance`]. The fork tests the block's
        // presence, not its contents (`0x52c9a3`), so a block source never shows the placeholder.
        let no_id_source = inst.is_none_or(|i| i.flags & 0x8 != 0);
        if no_id_source && !signable && v.random_property != 0 {
            keyed("ITEM_RANDOM_ENCHANT", &[], GREEN, false)?;
        }
        for e in enchant_slots {
            let color = match (e.slot < 2, e.negative) {
                (true, false) => GREEN,
                (true, true) => ENCHANT_RED,
                (false, _) => WHITE,
            };
            // A temporary enchant's countdown replaces its name, in the same line and colour
            // (`0x52ca49`); the time is `SMSG_ITEM_ENCHANT_TIME_UPDATE`'s.
            let mut text = match e.remaining_ms {
                // A missing template skips the slot, not falling back to the bare name: the
                // reference formats an empty string, which `AddLine` drops.
                Some(ms) => match enchant_time_left(&e.name, ms, &get) {
                    Some(t) => t,
                    None => continue,
                },
                None => e.name.clone(),
            };
            // The slot's charges: ITEM_SPELL_CHARGES inside the literal `" (%s)"` (`0x854820`,
            // `0x52caa6..0x52cb38`).
            if let Some(charges) = charges_phrase(e.charges, &get) {
                text.push_str(&format!(" ({charges})"));
            }
            add((text, color))?;
        }
    }
    if let Some((cur, max)) = inst.and_then(|i| i.durability).filter(|&(_, max)| max > 0) {
        // Red (`0xc0d390`) only when broken.
        let color = if cur == 0 { RED } else { WHITE };
        keyed(
            "DURABILITY_TEMPLATE",
            &[Arg::D(cur.into()), Arg::D(max.into())],
            color,
            false,
        )?;
    } else if v.max_durability > 0 {
        let full = i64::from(v.max_durability);
        keyed(
            "DURABILITY_TEMPLATE",
            &[Arg::D(full), Arg::D(full)],
            WHITE,
            false,
        )?;
    }
    // ITEM_DURATION (`0x854bb4`, `0x52ce0d`), the item's own expiry: after durability
    // (`0x52cd0e..0x52cd22`), before ITEM_COOLDOWN_TIME (`0x52e140`), through the `0x52fa50`
    // ladder. Its prefix ships no `_P1` twin, so every count reads "days" or "hrs"
    // (`GlobalStrings.lua:2401-2404`). The reference's formatter takes a dword, so a timer past
    // 49.7 days saturates here; no 1.12 item lasts past a fortnight.
    if let Some(ms) = inst.and_then(|i| i.duration_ms) {
        let ms = u32::try_from(ms).unwrap_or(u32::MAX);
        if let Some(line) = duration_text(ms, "ITEM_DURATION", true, &get) {
            add((line, WHITE))?;
        }
    }
    if v.allowable_class > 0
        && (v.allowable_class & full_mask(&CLASS_NAMES)) != full_mask(&CLASS_NAMES)
    {
        let list: Vec<&str> = CLASS_NAMES
            .iter()
            .filter(|&&(id, _)| v.allowable_class & (1 << (id - 1)) != 0)
            .map(|&(_, n)| n)
            .collect();
        if !list.is_empty() {
            let ok = req.class_id > 0 && v.allowable_class & (1 << (req.class_id - 1)) != 0;
            keyed(
                "ITEM_CLASSES_ALLOWED",
                &[Arg::S(&list.join(", "))],
                req_color(ok),
                false,
            )?;
        }
    }
    if v.allowable_race > 0 && (v.allowable_race & full_mask(&RACE_NAMES)) != full_mask(&RACE_NAMES)
    {
        let list: Vec<&str> = RACE_NAMES
            .iter()
            .filter(|&&(id, _)| v.allowable_race & (1 << (id - 1)) != 0)
            .map(|&(_, n)| n)
            .collect();
        if !list.is_empty() {
            let ok = req.race_id > 0 && v.allowable_race & (1 << (req.race_id - 1)) != 0;
            keyed(
                "ITEM_RACES_ALLOWED",
                &[Arg::S(&list.join(", "))],
                req_color(ok),
                false,
            )?;
        }
    }
    // Only a required level above 1 prints (`0x52d2cf`).
    if v.required_level > 1 {
        keyed(
            "ITEM_MIN_LEVEL",
            &[Arg::D(v.required_level.into())],
            req_color(req.level >= v.required_level),
            false,
        )?;
    }
    // ITEM_MIN_SKILL with a rank, ITEM_REQ_SKILL without (`0x52d34e`, `0x52d355`); the required
    // spell takes ITEM_REQ_SKILL too (`0x52d650`).
    if let Some(skill) = &v.required_skill_name {
        let have = req.skills.get(&v.required_skill).copied().unwrap_or(0);
        let color = req_color(have >= v.required_skill_rank.max(1));
        if v.required_skill_rank > 0 {
            let args = [Arg::S(skill), Arg::D(v.required_skill_rank.into())];
            keyed("ITEM_MIN_SKILL", &args, color, false)?;
        } else {
            keyed("ITEM_REQ_SKILL", &[Arg::S(skill)], color, false)?;
        }
    }
    if v.required_spell != 0 {
        if let Some(name) = &v.required_spell_name {
            keyed(
                "ITEM_REQ_SKILL",
                &[Arg::S(name)],
                req_color(known_spell),
                false,
            )?;
        }
    }
    if let Some(rep) = &v.required_rep_line {
        // Red (`0xc0d390`) below the required rank (`[+0x58]`); an unfed faction reads as unmet.
        let rank = req
            .rep_ranks
            .get(&v.required_rep_faction)
            .copied()
            .unwrap_or(0);
        add((
            rep.clone(),
            req_color(u32::from(rank) >= v.required_rep_rank),
        ))?;
    }
    if taught_known {
        keyed("ITEM_SPELL_KNOWN", &[], RED, false)?;
    }
    // Green, wrapped trigger lines: the prefix key and the app-resolved text joined by the
    // literal `"%s %s"` (`0x82ef08`, `0x52da7e`).
    for &(trigger, _, ref text) in &v.spell_triggers {
        let key = match trigger {
            0 | 5 => "ITEM_SPELL_TRIGGER_ONUSE",
            1 => "ITEM_SPELL_TRIGGER_ONEQUIP",
            2 => "ITEM_SPELL_TRIGGER_ONPROC",
            _ => continue,
        };
        let Some(prefix) = get(key) else { continue };
        addw((format!("{prefix} {text}"), GREEN))?;
    }
    if let Some(charges) = charges_phrase(v.charges.max(0) as u32, &get) {
        add((charges, WHITE))?;
    }
    // The set block (`0x52d8a0..0x52e0f5`), above p4's cut at `0x52e14c`: a blank gold line, the
    // gold "name (owned/total)" header, the set's skill line, the members (cream `0xc0d368` when
    // equipped, else gray; one still in flight waits), a second blank, then the bonuses sorted by
    // threshold (`0x52e5c0`), green when the skill is met (`0x5eaae0`) and enough are owned.
    // "Owned" counts equipped members on both builder paths. A met bonus is ITEM_SET_BONUS, the
    // text alone; only an unmet one is ITEM_SET_BONUS_GRAY, with its count (`0x52e056..0x52e0d6`).
    if let Some(set) = &set_view {
        let owned = set
            .members
            .iter()
            .filter(|(id, _)| equipped.contains(id))
            .count();
        addw((SET_SPACER.into(), GOLD))?;
        keyed(
            "ITEM_SET_NAME",
            &[
                Arg::S(&set.name),
                Arg::D(owned as i64),
                Arg::D(set.members.len() as i64),
            ],
            GOLD,
            false,
        )?;
        let skill_met = set.required_skill == 0 || {
            let have = req.skills.get(&set.required_skill).copied().unwrap_or(0);
            have >= set.required_skill_rank
        };
        if let Some(skill) = &set.required_skill_name {
            let color = req_color(skill_met);
            if set.required_skill_rank > 0 {
                let args = [Arg::S(skill), Arg::D(set.required_skill_rank.into())];
                keyed("ITEM_MIN_SKILL", &args, color, false)?;
            } else {
                keyed("ITEM_REQ_SKILL", &[Arg::S(skill)], color, false)?;
            }
        }
        // The two-space indent is the literal `"  %s"` (`0x854b14`), not a key.
        for (id, name) in &set.members {
            let Some(name) = name else { continue };
            let color = if equipped.contains(id) { CREAM } else { GRAY };
            add((format!("  {name}"), color))?;
        }
        addw((SET_SPACER.into(), GOLD))?;
        let mut bonuses: Vec<&(u32, String)> = set.bonuses.iter().collect();
        bonuses.sort_by_key(|&&(threshold, _)| threshold);
        for &(threshold, ref text) in bonuses {
            if skill_met && owned as u32 >= threshold {
                keyed("ITEM_SET_BONUS", &[Arg::S(text)], GREEN, true)?;
            } else {
                let args = [Arg::D(threshold.into()), Arg::S(text)];
                keyed("ITEM_SET_BONUS_GRAY", &args, GRAY, true)?;
            }
        }
    }
    // p4's early return (`0x52e147..0x52e14c`): compact falls through to `0x52e14e`, lays out and
    // returns, so nothing from `0x52e170` on runs: flavor text, creator, open or read line, money.
    if flags.name_only {
        return Ok(());
    }
    if !v.description.is_empty() {
        addw((format!("\"{}\"", v.description), GOLD))?;
    }
    // The reference gates the rest on an item object (`0x52e1c7`, `0x52e2e0`). Here only a template
    // source (`None`) stops, so a link, loot, mail or auction hover, which has an instance but no
    // object, still reaches READABLE.
    let Some(inst) = inst else { return Ok(()) };
    // The creator line (`0x52e1b1..0x52e2db`), white `0xc0cf60`: ITEM_WRITTEN_BY for a letter, else
    // ITEM_CREATED_BY, green by its own `|cff00ff00`. A wrapped instance (`0x8`, gate `0x52b7b0`)
    // takes ITEM_WRAPPED_BY off `ITEM_FIELD_GIFTCREATOR` in the reference; that is not built, so a
    // wrapped instance prints no creator line.
    if inst.flags & 0x8 == 0 {
        if let Some(name) = &inst.creator {
            let key = if inst.has_text {
                "ITEM_WRITTEN_BY"
            } else {
                "ITEM_CREATED_BY"
            };
            keyed(key, &[Arg::S(name)], WHITE, false)?;
        }
    }
    // One green line, OPENABLE winning (`0x52e2f2..0x52e35d`). Openable: a p6=0 source and either
    // the loot flag `0x4` behind its lock gate (a lock needs UNLOCKED) or a wrapped gift (`0x200`
    // and WRAPPED `0x8`). Readable: PageText (`0x5d9e10`) or the letter text. The lock gate is the
    // line's only: the click tests the bare flag (`ItemInfo::opens_loot`), and READABLE wins there.
    let openable = inst.openable_source
        && ((v.flags & 0x4 != 0 && (v.lock_id == 0 || inst.flags & 0x4 != 0))
            || (v.flags & 0x200 != 0 && inst.flags & 0x8 != 0));
    if openable {
        keyed("ITEM_OPENABLE", &[], GREEN, false)?;
    } else if v.page_text != 0 || inst.has_text {
        keyed("ITEM_READABLE", &[], GREEN, false)?;
    }
    Ok(())
}

/// A temporary enchant's name with its countdown (`0x52fa50`): `ITEM_ENCHANT_TIME_LEFT_<unit>`, its
/// `_P1` twin picked by [`plural_template`]. Two holes, the name then the count, so not
/// `duration_text`; `None` when the install lacks the family, and the caller draws no line.
fn enchant_time_left(name: &str, ms: u64, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let (suffix, n) = time_bucket(ms);
    let template = plural_template(&format!("ITEM_ENCHANT_TIME_LEFT_{suffix}"), n, get)?;
    Some(fill(&template, &[Arg::S(name), Arg::D(i64::from(n))]))
}

/// `0x52fa50`'s unit and count: the largest unit that fits, the count ceiled for days, hours and
/// minutes (all seven callers pass `roundUp` 1) and truncated for seconds.
fn time_bucket(ms: u64) -> (&'static str, u32) {
    const SEC: u64 = 1_000;
    const MIN: u64 = 60 * SEC;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    let (suffix, n) = match ms {
        _ if ms >= DAY => ("DAYS", ms.div_ceil(DAY)),
        _ if ms >= HOUR => ("HOURS", ms.div_ceil(HOUR)),
        _ if ms >= MIN => ("MIN", ms.div_ceil(MIN)),
        _ => ("SEC", ms / SEC),
    };
    // The count is a dword in the reference; a timer long enough to overflow one does not exist.
    (suffix, u32::try_from(n).unwrap_or(u32::MAX))
}

/// `ITEM_SPELL_CHARGES` or its `_P1` twin (`0x84e3b4`), pushed at `0x52cae8` for the enchant
/// suffix and `0x52db61` for the charges line; zero charges have no phrase.
fn charges_phrase(n: u32, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if n == 0 {
        return None;
    }
    let template = plural_template("ITEM_SPELL_CHARGES", n, get)?;
    Some(fill(&template, &[Arg::D(i64::from(n))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in string table, not the shipped wording: the tests check which key is reached and
    /// what fills it.
    fn table(key: &str) -> Option<String> {
        Some(
            match key {
                "ITEM_ENCHANT_TIME_LEFT_DAYS" => "<%s :: %d D>",
                "ITEM_ENCHANT_TIME_LEFT_DAYS_P1" => "<%s :: %d DD>",
                "ITEM_ENCHANT_TIME_LEFT_HOURS" => "<%s :: %d H>",
                "ITEM_ENCHANT_TIME_LEFT_HOURS_P1" => "<%s :: %d HH>",
                "ITEM_ENCHANT_TIME_LEFT_MIN" => "<%s :: %d M>",
                "ITEM_ENCHANT_TIME_LEFT_SEC" => "<%s :: %d S>",
                "ITEM_SPELL_CHARGES" => "<%d chg>",
                "ITEM_SPELL_CHARGES_P1" => "<%d chgs>",
                _ => return None,
            }
            .to_string(),
        )
    }

    /// The `0x52fa50` ladder: minutes and seconds have no `_P1` twin, so each reaches one key.
    #[test]
    fn enchant_countdown_buckets_and_rounding() {
        let t = |ms| enchant_time_left("Rockbiter", ms, &table);
        // Seconds truncate: 1900 ms is 1, not 2.
        assert_eq!(t(1_900).as_deref(), Some("<Rockbiter :: 1 S>"));
        assert_eq!(t(59_999).as_deref(), Some("<Rockbiter :: 59 S>"));
        // Minutes ceil: one second past 4 minutes already reads 5.
        assert_eq!(t(60_000).as_deref(), Some("<Rockbiter :: 1 M>"));
        assert_eq!(t(241_000).as_deref(), Some("<Rockbiter :: 5 M>"));
        // Hours ceil, and only the non-singular count takes the `_P1` key.
        assert_eq!(t(3_600_000).as_deref(), Some("<Rockbiter :: 1 H>"));
        assert_eq!(t(3_600_001).as_deref(), Some("<Rockbiter :: 2 HH>"));
        // Days ceil.
        assert_eq!(t(86_400_000).as_deref(), Some("<Rockbiter :: 1 D>"));
        assert_eq!(t(86_400_001).as_deref(), Some("<Rockbiter :: 2 DD>"));
        // The app drops an expired timer, but zero must not panic; it takes the plural arm,
        // `GetPluralIndex`'s rule.
        assert_eq!(t(0).as_deref(), Some("<Rockbiter :: 0 S>"));
    }

    #[test]
    fn a_missing_countdown_family_draws_nothing() {
        assert_eq!(enchant_time_left("Rockbiter", 60_000, &|_| None), None);
        assert_eq!(charges_phrase(3, &|_| None), None);
    }

    #[test]
    fn charges_phrase_picks_the_plural() {
        assert_eq!(charges_phrase(1, &table).as_deref(), Some("<1 chg>"));
        assert_eq!(charges_phrase(5, &table).as_deref(), Some("<5 chgs>"));
        assert_eq!(charges_phrase(0, &table), None);
    }
}
