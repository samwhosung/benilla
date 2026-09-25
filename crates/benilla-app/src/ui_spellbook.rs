//! The spellbook feed: builds the book from the known spells and the `SpellCatalog` ×
//! `SkillLineCatalog` join, behind the stock `SpellBookFrame.xml`. A known spell is booked when
//! [`SpellDisplay::in_spellbook`](benilla_formats::SpellDisplay::in_spellbook) passes (`0x4b25b0`).
//!
//! A spell's tab (`0x4b24f0`) is its skill line, or General (key 0) when the
//! `SkillRaceClassInfo.dbc` lookup (`0x6ddf90`) finds no row for the player's race and class, or
//! one with `SKILL_FLAG_DISPLAY_SORTED` (`0x80`). General comes first, then tabs by
//! `SkillLine.dbc` name (`0x4b3040`, byte order standing in for the enUS collator `0x64a480`);
//! within a tab, name then parsed rank (`0x4b30c0`).
//!
//! Not modelled: which of several admitting `SkillRaceClassInfo` rows `0x6ddf90` picks (the first
//! wins here), and the `SpellLevel` fallback for a digit-less rank (the rank string breaks ties).

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;

use benilla_formats::{SkillLineCatalog, SpellCatalog};
use benilla_ui::script::{ScriptValue, SpellBookState, SpellSlotView, SpellTabView, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::NetCommands;
use crate::spell::{cast_target, CastCommit, CastLadder};
use crate::ui_action::{melee_auto_attack_icon, ranged_weapon_icon, PlayerActions, Spells};
use crate::ui_script::{gate, UiInput};
use crate::ui_unit::UnitFeed;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// `SkillLine.dbc` × `SkillLineAbility.dbc`; absent when the data failed to load.
#[derive(Resource)]
pub(crate) struct SkillLines {
    pub(crate) catalog: SkillLineCatalog,
}

/// The General tab, key 0: pinned first, named from the `GENERAL` GlobalString (`0x846910`) with a
/// fixed icon (`GetSpellTabInfo` `0x4b3ce0`); vmangos's `SKILL_NONE` is 0, so no line collides.
const NO_LINE: u32 = 0;

/// Hardcoded in the reference (`0x8468f0`), extensionless for the loader; key 0 has no DBC row.
const GENERAL_TAB_ICON: &str = "Interface\\Icons\\Ability_Kick";

/// Spells learned mid-session, queued for `LEARNED_SPELL_IN_TAB` (event 510). Filled by the learn
/// and rank-up arms (`crate::spell::net`), never the `SMSG_INITIAL_SPELLS` load, which the
/// reference passes with `0x4b25b0`'s live-mutation flag clear. A queue because the event follows
/// the tab re-sort and `SPELLS_CHANGED` (`0x4b2b5a` before `0x4b2b92`).
#[derive(Resource, Default)]
pub(crate) struct LearnedInTab(pub(crate) Vec<u32>);

pub(crate) struct UiSpellbookPlugin;

impl Plugin for UiSpellbookPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LearnedInTab>()
            .add_systems(Startup, load_skill_lines.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // The feed precedes `CooldownEvents`, whose `SPELL_UPDATE_COOLDOWN` makes the
                    // book re-read its cooldowns; the drain follows the input pass, so a click
                    // casts the same frame.
                    feed_spellbook
                        .in_set(UnitFeed)
                        .before(crate::ui_action::CooldownEvents),
                    drain_spell_casts.after(UiInput),
                ),
            );
    }
}

fn load_skill_lines(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_skill_line_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            info!(
                "ui_spellbook: {} skill line(s) in the tab catalog",
                catalog.len()
            );
            commands.insert_resource(SkillLines { catalog });
        }
        Err(e) => warn!(
            "ui_spellbook: SkillLine.dbc failed to load — every spell falls into one \
             fallback tab: {e:#}"
        ),
    }
}

/// The feed's memory of what it last pushed, for the `SPELLS_CHANGED` diff.
#[derive(Default)]
struct FeedMemory {
    pushed: SpellBookState,
    /// Counter watches for stores whose lazy resolves defeat `is_changed` (template asks, the
    /// cooldown prune), and for the self store's presence: a despawn flips the weapon icons unseen.
    items_templates: gate::Watch,
    cooldown_epoch: gate::Watch,
    self_present: gate::Watch,
}

fn feed_spellbook(
    script: Option<NonSendMut<UiScript>>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    skill_lines: Option<Res<SkillLines>>,
    mut inv: crate::items::Inventory,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    cooldowns: Res<crate::spell::Cooldowns>,
    clock: Res<crate::ui_script::UiClock>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
    mut learned: ResMut<LearnedInTab>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (memory, vm_reset) = memory.get_reset(&script);
    // The book depends on these inputs alone. `UiClock` is not one: `ui_triple` is frame-stable
    // per arm, and expiry moves the store's `feed_epoch`.
    let objects_moved = inv.changes.moved();
    let templates_moved = memory.items_templates.moved(items.template_epoch());
    let cooldowns_moved = memory.cooldown_epoch.moved(cooldowns.feed_epoch());
    let presence_moved = memory
        .self_present
        .moved(u64::from(!inv.self_store.is_empty()));
    // The frame a timer crosses zero, before the prune moves the epoch.
    let sweep = cooldowns.sweep_pending(clock.anchor);
    let self_changed = !inv.self_changed.is_empty();
    let actions_changed = actions.is_changed();
    let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
    let lines_changed = skill_lines.as_ref().is_some_and(|r| r.is_changed());
    // `is_added`: only the load-once icon column is read; the model cache churns every frame.
    let icons_added = icons.as_ref().is_some_and(|r| r.is_added());
    gate::trace(
        "feed_spellbook",
        &[
            ("vm_reset", vm_reset),
            ("objects", objects_moved),
            ("templates", templates_moved),
            ("cooldowns", cooldowns_moved),
            ("sweep", sweep),
            ("presence", presence_moved),
            ("self", self_changed),
            ("actions", actions_changed),
            ("spells", spells_changed),
            ("skill_lines", lines_changed),
            ("icons", icons_added),
            // A queued learn never waits behind a skipped frame.
            ("learned", !learned.0.is_empty()),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || objects_moved
            || templates_moved
            || cooldowns_moved
            || sweep
            || presence_moved
            || self_changed
            || actions_changed
            || spells_changed
            || lines_changed
            || icons_added
            || !learned.0.is_empty(),
    );
    if gate.skip() {
        return;
    }
    // Drained before a later return can strand it; an unresolved id fires nothing, as in 1.12.
    let learned = std::mem::take(&mut learned.0);
    let Some(spells) = spells.as_deref() else {
        return;
    };
    // Race and class drive the General collapse; 0/0 before the descriptor skips it.
    let store = inv.self_store.single().ok();
    let (race, class) = store
        .map(|s| (s.0.unit_race().unwrap_or(0), s.0.unit_class().unwrap_or(0)))
        .unwrap_or((0, 0));
    // The melee auto-attack shows the main-hand weapon, or Spell-Reset unarmed, not its `Temp`.
    let attack_icon = store.map(|s| {
        melee_auto_attack_icon(
            s,
            &spells.forms,
            &inv.objects,
            &items,
            icons.as_deref(),
            &commands,
        )
    });
    // Auto Shot and wand Shoot show the ranged weapon; without one, their own icon.
    let ranged_icon = store
        .and_then(|s| ranged_weapon_icon(s, &inv.objects, &items, icons.as_deref(), &commands));
    let (mut fresh, tab_lines) = build_book(
        &actions.spells,
        &spells.catalog,
        skill_lines.as_deref().map(|s| &s.catalog),
        race,
        class,
        attack_icon,
        ranged_icon,
    );
    // The book's `IsCurrentCast` is `0x4b3600`, not the action bar's `0x4e53a0`, with two arms: a
    // form spell is current while the form byte is its form, and the open trade-skill window, not
    // built. It has no casting-now arm, so a book slot never lights during a cast.
    let form_byte = store.map(|s| s.0.unit_shapeshift_form()).unwrap_or(0);
    if form_byte != 0 {
        for slot in &mut fresh.slots {
            slot.current = spells
                .catalog
                .get(slot.spell_id)
                .is_some_and(|d| d.shapeshift_form == Some(u32::from(form_byte)));
        }
    }
    // Each slot's cooldown (`GetCooldownInfo` `0x6e13e0`: id, category and GCD alike) on the
    // `GetTime` clock, frame-stable per arm, so a running cooldown never churns the diff.
    let (anchor, ui_now) = (clock.anchor, clock.ui_now);
    for slot in &mut fresh.slots {
        slot.cooldown = cooldowns
            .info(slot.spell_id, 0, spells.catalog.get(slot.spell_id), anchor)
            .ui_triple(anchor, ui_now);
    }
    if fresh != memory.pushed {
        gate.audit("feed_spellbook", "the spellbook snapshot");
        // A checked-ring move fires `CURRENT_SPELL_CAST_CHANGED`, a book change `SPELLS_CHANGED`.
        let ring_moved = fresh.slots.len() != memory.pushed.slots.len()
            || fresh
                .slots
                .iter()
                .zip(&memory.pushed.slots)
                .any(|(a, b)| a.current != b.current);
        let book_changed = fresh.tabs != memory.pushed.tabs
            || fresh.slots.len() != memory.pushed.slots.len()
            || fresh.slots.iter().zip(&memory.pushed.slots).any(|(a, b)| {
                (a.spell_id, &a.name, &a.rank, &a.texture, a.passive)
                    != (b.spell_id, &b.name, &b.rank, &b.texture, b.passive)
            });
        debug!(
            "ui_spellbook: fed {} tab(s), {} spell(s) (book {book_changed}, ring {ring_moved})",
            fresh.tabs.len(),
            fresh.slots.len()
        );
        script.set_spellbook(fresh.clone());
        memory.pushed = fresh;
        if book_changed {
            script.fire_event("SPELLS_CHANGED", vec![]);
        }
        if ring_moved {
            script.fire_event("CURRENT_SPELL_CAST_CHANGED", vec![]);
        }
    }
    // The tab flash after `SPELLS_CHANGED` (`0x4b2b5a` before `0x4b2b92`), outside the diff: the
    // reference fires on the packet, so a re-learn that changes nothing still flashes.
    fire_tab_flashes(
        &mut script,
        &learned,
        &tab_lines,
        spells,
        skill_lines.as_deref().map(|s| &s.catalog),
        race,
        class,
    );
}

/// Fire `LEARNED_SPELL_IN_TAB` with the 1-based index of each learned spell's tab in the strip
/// `build_book` just published (`0x4b2b86 inc edi`), which `SpellBookFrame.lua:81` flashes. The
/// reference's three early returns (`0x4b2911`, `0x4b2944`, `0x4b29c4`) are `in_spellbook`.
fn fire_tab_flashes(
    script: &mut UiScript,
    learned: &[u32],
    tab_lines: &[u32],
    spells: &Spells,
    skill_lines: Option<&SkillLineCatalog>,
    race: u8,
    class: u8,
) {
    for &spell_id in learned {
        let Some(index) = tab_flash_index(spell_id, tab_lines, spells, skill_lines, race, class)
        else {
            continue;
        };
        debug!("ui_spellbook: spell {spell_id} landed in tab {index} — flash");
        script.fire_event("LEARNED_SPELL_IN_TAB", vec![ScriptValue::Int(index)]);
    }
}

/// The 1-based index of `spell_id`'s tab in `tab_lines`; `None` for an unbooked spell.
fn tab_flash_index(
    spell_id: u32,
    tab_lines: &[u32],
    spells: &Spells,
    skill_lines: Option<&SkillLineCatalog>,
    race: u8,
    class: u8,
) -> Option<i64> {
    if !spells
        .catalog
        .get(spell_id)
        .is_some_and(|d| d.in_spellbook())
    {
        return None;
    }
    let tab = skill_lines.map_or(NO_LINE, |c| c.spell_tab(spell_id, race, class));
    let index = tab_lines.iter().position(|&l| l == tab)?;
    Some(index as i64 + 1)
}

/// Build the book, and the skill-line id behind each published tab in order, for
/// [`LearnedInTab`]'s index: one tab ordering, never recomputed.
fn build_book(
    known: &BTreeSet<u32>,
    catalog: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    race: u8,
    class: u8,
    attack_icon: Option<String>,
    ranged_icon: Option<String>,
) -> (SpellBookState, Vec<u32>) {
    let mut by_line: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &spell_id in known {
        // The add-gate (module doc); a spell missing from `Spell.dbc` is dropped too.
        if !catalog.get(spell_id).is_some_and(|d| d.in_spellbook()) {
            continue;
        }
        // The tab after the General collapse; with no skill-line catalog, General.
        let tab = skill_lines
            .map(|c| c.spell_tab(spell_id, race, class))
            .unwrap_or(NO_LINE);
        by_line.entry(tab).or_default().push(spell_id);
    }

    // General first with its fixed name and icon, then by `SkillLine.dbc` name; an unresolved
    // line reads "General" with no icon.
    let mut lines: Vec<(u32, String, Option<String>, Vec<u32>)> = by_line
        .into_iter()
        .map(|(line_id, spell_ids)| {
            let (name, texture) = if line_id == NO_LINE {
                ("General".to_string(), Some(GENERAL_TAB_ICON.to_string()))
            } else {
                match skill_lines.and_then(|c| c.line(line_id)) {
                    Some(info) => (info.name.clone(), info.icon.clone()),
                    None => ("General".to_string(), None),
                }
            };
            (line_id, name, texture, spell_ids)
        })
        .collect();
    lines.sort_by(|a, b| (a.0 != NO_LINE).cmp(&(b.0 != NO_LINE)).then(a.1.cmp(&b.1)));

    let mut tabs = Vec::with_capacity(lines.len());
    let mut tab_lines = Vec::with_capacity(lines.len());
    let mut slots = Vec::new();
    for (line_id, name, texture, mut spell_ids) in lines {
        tab_lines.push(line_id);
        spell_ids.sort_by_key(|&a| spell_sort_key(catalog, a));
        let offset = slots.len() as u32;
        let num_spells = spell_ids.len() as u32;
        for spell_id in spell_ids {
            let d = catalog.get(spell_id);
            // Keyed on the effect type, as the action bar's substitution is.
            let texture = if d.is_some_and(|d| d.is_melee_auto_attack()) {
                attack_icon
                    .clone()
                    .or_else(|| d.and_then(|d| d.icon.clone()))
            } else if d.is_some_and(|d| d.ranged_icon_substitution()) {
                // Without a ranged weapon, or with a thrown one, the spell's own (`0x4e6990`).
                ranged_icon
                    .clone()
                    .or_else(|| d.and_then(|d| d.icon.clone()))
            } else {
                d.and_then(|d| d.icon.clone())
            };
            slots.push(SpellSlotView {
                spell_id,
                name: d.map(|d| d.name.clone()).unwrap_or_default(),
                rank: d.and_then(|d| d.rank.clone()),
                texture,
                passive: d.is_some_and(|d| d.passive),
                // Stamped by the feed after the build, from live state.
                current: false,
                cooldown: None,
                // `GetSpellAutocast` answers (nil, nil) for the player book, and a known spell has
                // no packed word.
                autocast: None,
                packed: 0,
            });
        }
        tabs.push(SpellTabView {
            name,
            texture,
            offset,
            num_spells,
        });
    }
    (SpellBookState { tabs, slots }, tab_lines)
}

/// `0x4b30c0`'s tail: name, then the parsed rank number, then the rank string. The rank is the
/// first digit run of `NameSubtext` (`0x4b3c30`), so any localized prefix parses.
fn spell_sort_key(catalog: &SpellCatalog, spell_id: u32) -> (String, u32, String) {
    let d = catalog.get(spell_id);
    spell_sort_key_of(
        d.map(|d| d.name.as_str()).unwrap_or_default(),
        d.and_then(|d| d.rank.as_deref()),
    )
}

/// [`spell_sort_key`] over a resolved `(name, rank)`, shared with the pet book: `0x4b2fd0` sorts
/// both books with `0x4b30c0`.
pub(crate) fn spell_sort_key_of(name: &str, rank: Option<&str>) -> (String, u32, String) {
    let rank_str = rank.unwrap_or_default().to_string();
    (name.to_string(), leading_number(&rank_str), rank_str)
}

/// The first ASCII digit run as a number, else 0: the reference's rank parse (`0x4b3c30`).
fn leading_number(s: &str) -> u32 {
    s.chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .fold(None::<u32>, |acc, c| {
            Some(acc.unwrap_or(0) * 10 + c.to_digit(10).unwrap())
        })
        .unwrap_or(0)
}

/// Drain `take_spell_casts` through the cast tail the action bar uses.
fn drain_spell_casts(
    script: Option<NonSendMut<UiScript>>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
) {
    let Some(mut script) = script else {
        return;
    };
    for spell_id in script.take_spell_casts() {
        // `CastSpell`'s two cancel forks (`0x4b3300`), in order: the active-action toggle
        // (`0x4b36f0`), then the form match (`0x4b348b`), which `UseAction` lacks, with its silent
        // no-op (`0x4b35cf`).
        if let (Some(sp), Some(store)) =
            (ladder.spells.as_ref(), targeting.self_store.iter().next())
        {
            if let Some(d) = sp.catalog.get(spell_id) {
                if crate::ui_action::toggle::active_action_toggle(spell_id, d, store) {
                    debug!("ui_spellbook: cast {spell_id} re-pressed — aura cancels");
                    let _ = ladder
                        .commands
                        .0
                        .send(crate::net::ClientCommand::CancelAura { spell_id });
                    continue;
                }
                let form = store.0.unit_shapeshift_form();
                let row = sp.forms.get(&u32::from(form));
                match crate::ui_action::toggle::form_recast_disposition(d, form, row) {
                    Some(true) => {
                        debug!("ui_spellbook: cast {spell_id} — the active form cancels");
                        let _ = ladder
                            .commands
                            .0
                            .send(crate::net::ClientCommand::CancelAura { spell_id });
                        continue;
                    }
                    Some(false) => {
                        debug!("ui_spellbook: cast {spell_id} — non-cancelable form, silent no-op");
                        continue;
                    }
                    None => {}
                }
            }
        }
        debug!(
            "ui_spellbook: cast {spell_id} (target {:?})",
            targeting.selection.guid
        );
        ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::SpellDisplay;
    use std::collections::HashMap;

    #[test]
    fn leading_number_scans_the_first_digit_run() {
        assert_eq!(leading_number("Rank 1"), 1);
        assert_eq!(leading_number("Rank 10"), 10);
        assert_eq!(leading_number("Rank 2"), 2);
        assert_eq!(leading_number(""), 0);
        assert_eq!(leading_number("Racial"), 0); // digit-less: the reference uses SpellLevel
        assert_eq!(leading_number("Apprentice"), 0);
    }

    /// A minimal `SpellDisplay`: `attributes` drives the gate, `rank` the sort.
    fn spell(name: &str, rank: Option<&str>, attributes: u32) -> SpellDisplay {
        SpellDisplay {
            name: name.into(),
            rank: rank.map(Into::into),
            attributes,
            passive: attributes & 0x40 != 0,
            ..Default::default()
        }
    }

    /// With no skill-line catalog every booked spell is in General, index 1 (`0x4b2b86 inc edi`).
    #[test]
    fn the_flash_index_is_one_based_into_the_published_tab_strip() {
        let mut map = HashMap::new();
        map.insert(133, spell("Fireball", Some("Rank 2"), 0x10000));
        map.insert(668, spell("Language: Common", None, 0x80)); // DO_NOT_DISPLAY
        map.insert(818, spell("Cooking", None, 0x20)); // IS_TRADESKILL
        let mut castbar = spell("Some Castbar Spell", None, 0);
        castbar.cast_ui = 2;
        map.insert(1234, castbar);
        let catalog = SpellCatalog::from_displays(map);
        let known: BTreeSet<u32> = [133, 668, 818, 1234].into_iter().collect();
        let (book, tab_lines) = build_book(&known, &catalog, None, 1, 1, None, None);

        assert_eq!(
            tab_lines.len(),
            book.tabs.len(),
            "one line id per published tab"
        );
        assert_eq!(tab_lines, vec![NO_LINE], "General is the only tab here");

        let spells = Spells {
            catalog,
            ..Spells::empty_for_tests()
        };
        assert_eq!(
            tab_flash_index(133, &tab_lines, &spells, None, 1, 1),
            Some(1),
            "a booked spell flashes its tab, 1-based"
        );

        // The reference's three returns before `0x4b2b92`, the book add-gate.
        for (id, why) in [
            (668u32, "DO_NOT_DISPLAY returns at 0x4b2911"),
            (818, "IS_TRADESKILL returns at 0x4b2944"),
            (1234, "castUI > 0 returns at 0x4b29c4"),
        ] {
            assert_eq!(
                tab_flash_index(id, &tab_lines, &spells, None, 1, 1),
                None,
                "{why}"
            );
        }

        assert_eq!(
            tab_flash_index(99999, &tab_lines, &spells, None, 1, 1),
            None,
            "an id with no Spell.dbc record flashes nothing"
        );
    }

    /// Shown spells sort name then rank, and General takes the fixed name and icon.
    #[test]
    fn build_book_gates_hidden_spells_and_pins_general() {
        let mut map = HashMap::new();
        map.insert(133, spell("Fireball", Some("Rank 2"), 0x10000));
        map.insert(145, spell("Fireball", Some("Rank 1"), 0x10000)); // out-of-order rank
        map.insert(2136, spell("Fire Blast", Some("Rank 1"), 0x0));
        map.insert(668, spell("Language: Common", None, 0x80)); // DO_NOT_DISPLAY
        map.insert(818, spell("Cooking", None, 0x20)); // IS_TRADESKILL
        let catalog = SpellCatalog::from_displays(map);
        let known: BTreeSet<u32> = [133, 145, 2136, 668, 818].into_iter().collect();

        // No skill-line catalog: every shown spell lands in General.
        let book = build_book(&known, &catalog, None, 1, 1, None, None).0;

        assert_eq!(book.tabs.len(), 1);
        assert_eq!(book.tabs[0].name, "General");
        assert_eq!(book.tabs[0].texture.as_deref(), Some(GENERAL_TAB_ICON));
        assert_eq!(book.tabs[0].num_spells, 3);
        let order: Vec<(&str, Option<&str>)> = book
            .slots
            .iter()
            .map(|s| (s.name.as_str(), s.rank.as_deref()))
            .collect();
        assert_eq!(
            order,
            vec![
                ("Fire Blast", Some("Rank 1")),
                ("Fireball", Some("Rank 1")),
                ("Fireball", Some("Rank 2")),
            ]
        );
    }

    /// Keyed on `Effect[0] == SPELL_EFFECT_ATTACK`, not the id; `None` keeps the spell's icon.
    #[test]
    fn build_book_attack_shows_the_resolved_icon_by_effect() {
        const ATTACK: u32 = 6603;
        let mut attack = spell("Attack", None, 0x10);
        attack.icon = Some("Interface\\Icons\\Temp".into()); // the real DBC placeholder
        attack.effects[0] = 78; // SPELL_EFFECT_ATTACK
        let catalog = SpellCatalog::from_displays(HashMap::from([(ATTACK, attack)]));
        let known: BTreeSet<u32> = [ATTACK].into_iter().collect();

        // The feed's resolved icon, the weapon or Spell-Reset, wins over Temp.
        for resolved in [
            "Interface\\Icons\\INV_Sword_04",
            "Interface\\Buttons\\Spell-Reset",
        ] {
            let book = build_book(&known, &catalog, None, 1, 1, Some(resolved.into()), None).0;
            assert_eq!(book.slots.len(), 1);
            assert_eq!(book.slots[0].spell_id, ATTACK);
            assert_eq!(book.slots[0].texture.as_deref(), Some(resolved));
        }

        // No character: the spell's own icon.
        let bare = build_book(&known, &catalog, None, 1, 1, None, None).0;
        assert_eq!(
            bare.slots[0].texture.as_deref(),
            Some("Interface\\Icons\\Temp")
        );
    }

    /// Both bits (`Attributes & 0x2`, `AttributesEx2 & 0x20`) substitute; Throw's `0x2` alone
    /// does not, and without a weapon the spell keeps its own icon, never Spell-Reset (`0x4e6990`).
    #[test]
    fn build_book_ranged_shots_borrow_the_ranged_weapon_icon() {
        const AUTO_SHOT: u32 = 75;
        const THROW: u32 = 2764;
        let mut auto_shot = spell("Auto Shot", None, 0x2);
        auto_shot.icon = Some("Interface\\Icons\\Ability_AutoShot".into());
        auto_shot.attributes_ex2 = 0x20;
        let mut throw = spell("Throw", None, 0x2);
        throw.icon = Some("Interface\\Icons\\Ability_Throw".into());
        let catalog =
            SpellCatalog::from_displays(HashMap::from([(AUTO_SHOT, auto_shot), (THROW, throw)]));
        let known: BTreeSet<u32> = [AUTO_SHOT, THROW].into_iter().collect();

        let bow = "Interface\\Icons\\INV_Weapon_Bow_02";
        let book = build_book(&known, &catalog, None, 1, 1, None, Some(bow.into())).0;
        let icon_of = |b: &SpellBookState, id: u32| {
            b.slots
                .iter()
                .find(|s| s.spell_id == id)
                .and_then(|s| s.texture.clone())
        };
        assert_eq!(icon_of(&book, AUTO_SHOT).as_deref(), Some(bow));
        assert_eq!(
            icon_of(&book, THROW).as_deref(),
            Some("Interface\\Icons\\Ability_Throw"),
            "ranged-slot alone (no auto-repeat bit) keeps the spell icon"
        );

        // No ranged weapon: Auto Shot keeps its own icon.
        let bare = build_book(&known, &catalog, None, 1, 1, None, None).0;
        assert_eq!(
            icon_of(&bare, AUTO_SHOT).as_deref(),
            Some("Interface\\Icons\\Ability_AutoShot")
        );
    }
}
