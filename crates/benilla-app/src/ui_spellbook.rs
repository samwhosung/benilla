//! The spellbook behind the stock `SpellBookFrame.xml`: the book's order from the known spells and
//! the `SpellCatalog` × `SkillLineCatalog` join ([`book_order`]), which the getters read through
//! the call host as it stands ([`live`]), and the signals of the handlers that change it
//! ([`BookSignals`]). A known spell is booked when
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
use benilla_ui::script::{SpellSlotView, SpellTabName, SpellTabView};

use crate::field_watch::{FieldNotice, NoticeLane};
use crate::ui_action::Spells;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

mod live;
pub(crate) use live::BookLive;

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

pub(crate) struct UiSpellbookPlugin;

impl Plugin for UiSpellbookPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_skill_lines.after(AssetSet::Open));
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

/// The signals of the handlers that change the book, at the handler, on the notice lane: the
/// reference's add and removal signal inside the handler that learns or unlearns (`0x4b2b5a`,
/// `0x4b2f3f`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BookSignals<'w, 's> {
    lane: ResMut<'w, NoticeLane>,
    // The learn's line, shown between the form list's edge and the book's signals.
    errors: Res<'w, crate::ui_action::UiErrorKeys>,
    skill_lines: Option<Res<'w, SkillLines>>,
    me: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
}

/// What a learn's spell replaces, for the registrar's form-list leg (`0x4b2849`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Learned {
    /// `SMSG_LEARNED_SPELL`.
    Fresh,
    /// `SMSG_SUPERCEDED_SPELL`'s new rank, over `old`.
    Rank { old: u32 },
}

impl BookSignals<'_, '_> {
    /// The lane the book's signals go on, which the bar's legs of the same packet share
    /// ([`crate::ui_action::SpellSlots`]), so the two keep the handler's order.
    pub(crate) fn lane(&mut self) -> &mut NoticeLane {
        &mut self.lane
    }

    /// `0x4b25b0`'s tail for `spell_id`, now among `actions`' known spells. First its language
    /// leg's `LANGUAGE_LIST_CHANGED` for a spell whose first effect is `SPELL_EFFECT_LANGUAGE`
    /// (`0x4b2641`-`0x4b2659`, live or not), then the form list's leg on the stored list
    /// ([`crate::ui_shapeshift::form_learned`]) and its edge, `UPDATE_SHAPESHIFT_FORMS`
    /// (`0x4b2904`), when it moved. Then the book's: the sort's `SPELLS_CHANGED` (`0x4b2b5a`),
    /// `LEARNED_SPELL_IN_TAB` with its tab's 1-based place in the order it left (`0x4b2b92`), then
    /// The Spellbook's tutorial (`0x4b2ba1`), none of them for a spell the gates keep out of the
    /// book (`0x4b2911`, `0x4b2944`, `0x4b29c4`). The learn's `line` shows between the form edge
    /// and the book's (`0x4b2936`, `0x4b29b7`). With no player held a fresh learn joins the
    /// initial list, which the arrival learns (`0x5e61fb` → `0x5e621e`), and signals nothing; a
    /// rank-up still shows its line.
    pub(crate) fn entered(
        &mut self,
        spell_id: u32,
        how: Learned,
        actions: &mut crate::ui_action::PlayerActions,
        spells: Option<&Spells>,
        line: Option<crate::ui_action::UiError>,
    ) {
        let Ok(store) = self.me.single() else {
            match how {
                Learned::Fresh => actions.initial.push(spell_id),
                // A rank-up reaches the registrar ahead of its player test (`0x5e6363` →
                // `0x4b2f68`, then `0x5e6385`), whose line reads no player (`0x4b29b7`).
                Learned::Rank { .. } => {
                    if let Some(line) = line {
                        self.errors.push(line);
                    }
                }
            }
            return;
        };
        let Some(spells) = spells else {
            return;
        };
        if spells.catalog.get(spell_id).is_some_and(teaches_language) {
            self.lane.write(FieldNotice::Global {
                id: crate::field_watch::id::LANGUAGE_LIST_CHANGED,
            });
        }
        let replaces = match how {
            Learned::Fresh => None,
            Learned::Rank { old } => Some(old),
        };
        if let Some(edits) = actions.form_edits.as_mut() {
            edits.push(crate::ui_shapeshift::FormEdit::Learned { spell_id, replaces });
        }
        if crate::ui_shapeshift::form_learned(
            &mut actions.forms,
            spell_id,
            replaces,
            &spells.catalog,
        ) {
            self.lane.write(FieldNotice::Global {
                id: crate::field_watch::id::UPDATE_SHAPESHIFT_FORMS,
            });
        }
        if let Some(line) = line {
            self.errors.push(line);
        }
        let (race, class) = (
            store.0.unit_race().unwrap_or(0),
            store.0.unit_class().unwrap_or(0),
        );
        let lines = self.skill_lines.as_deref().map(|l| &l.catalog);
        let order = book_order(&actions.spells, &spells.catalog, lines, race, class);
        let Some(tab) = tab_flash_index(spell_id, &order.tab_lines, spells, lines, race, class)
        else {
            return;
        };
        debug!("ui_spellbook: spell {spell_id} landed in tab {tab} — flash");
        self.lane.write_batch([
            FieldNotice::Global {
                id: crate::field_watch::id::SPELLS_CHANGED,
            },
            FieldNotice::LearnedInTab { tab },
            FieldNotice::Tutorial {
                id: crate::tutorial::id::SPELLBOOK,
            },
        ]);
    }

    /// `0x4b2c50`'s tail for `spell_id`, which `was_known` says the known spells held, with a
    /// player held. An unlearn (`unlists` true; a rank-up's removal passes 1, `0x4b2db7`) first
    /// takes the spell off the stored form list where it holds it and signals
    /// `UPDATE_SHAPESHIFT_FORMS` ([`crate::ui_shapeshift::form_unlearned`]); then the sort's
    /// `SPELLS_CHANGED` (`0x4b2f3f`) when it was in the book (`0x4b2f2f`).
    pub(crate) fn left(
        &mut self,
        spell_id: u32,
        was_known: bool,
        unlists: bool,
        actions: &mut crate::ui_action::PlayerActions,
        spells: Option<&Spells>,
    ) {
        let d = spells.and_then(|s| s.catalog.get(spell_id));
        if !was_known || self.me.is_empty() {
            return;
        }
        if unlists {
            if let Some(edits) = actions.form_edits.as_mut() {
                edits.push(crate::ui_shapeshift::FormEdit::Unlearned { spell_id });
            }
        }
        if unlists && crate::ui_shapeshift::form_unlearned(&mut actions.forms, spell_id) {
            self.lane.write(FieldNotice::Global {
                id: crate::field_watch::id::UPDATE_SHAPESHIFT_FORMS,
            });
        }
        if d.is_some_and(|d| d.in_spellbook()) {
            self.lane.write(FieldNotice::Global {
                id: crate::field_watch::id::SPELLS_CHANGED,
            });
        }
    }
}

/// Whether a spell's first effect is `SPELL_EFFECT_LANGUAGE`, the registrar's test for its
/// language leg (`0x4b2641`, `[rec+0xf4]`).
pub(crate) fn teaches_language(d: &benilla_formats::SpellDisplay) -> bool {
    d.effects[0] == benilla_formats::SPELL_EFFECT_LANGUAGE
}

/// The 1-based index of `spell_id`'s tab in `tab_lines` (`0x4b2b86 inc edi`), which
/// `SpellBookFrame.lua:81` flashes; `None` for an unbooked spell, the reference's three early
/// returns (`0x4b2911`, `0x4b2944`, `0x4b29c4`).
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

/// The book's order: the tabs in `0x4b3040`'s order with the skill line behind each, and the
/// booked spells by slot in `0x4b30c0`'s.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct BookOrder {
    pub(crate) tabs: Vec<SpellTabView>,
    pub(crate) tab_lines: Vec<u32>,
    pub(crate) slots: Vec<u32>,
}

/// The book `known` makes for `race` and `class`: one tab ordering, which the slots' offsets and
/// a learn's tab index both read.
pub(crate) fn book_order(
    known: &BTreeSet<u32>,
    catalog: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    race: u8,
    class: u8,
) -> BookOrder {
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

    // General first with its fixed icon, named by `GENERAL` when `GetSpellTabInfo` asks, then by
    // `SkillLine.dbc` name; an unresolved line answers no name and no icon (`0x4b3dbd`) and sorts
    // where a line named "General" would.
    let mut lines: Vec<(u32, SpellTabName, Option<String>, Vec<u32>)> = by_line
        .into_iter()
        .map(|(line_id, spell_ids)| {
            let (name, texture) = if line_id == NO_LINE {
                (SpellTabName::General, Some(GENERAL_TAB_ICON.to_string()))
            } else {
                match skill_lines.and_then(|c| c.line(line_id)) {
                    Some(info) => (SpellTabName::Line(info.name.clone()), info.icon.clone()),
                    None => (SpellTabName::Missing, None),
                }
            };
            (line_id, name, texture, spell_ids)
        })
        .collect();
    let sort_name = |n: &SpellTabName| match n {
        SpellTabName::Line(name) => name.clone(),
        SpellTabName::General | SpellTabName::Missing => "General".to_string(),
    };
    lines.sort_by(|a, b| {
        (a.0 != NO_LINE)
            .cmp(&(b.0 != NO_LINE))
            .then(sort_name(&a.1).cmp(&sort_name(&b.1)))
    });

    let mut order = BookOrder::default();
    for (line_id, name, texture, mut spell_ids) in lines {
        order.tab_lines.push(line_id);
        spell_ids.sort_by_cached_key(|&a| spell_sort_key(catalog, a));
        order.tabs.push(SpellTabView {
            name,
            texture,
            offset: order.slots.len() as u32,
            num_spells: spell_ids.len() as u32,
        });
        order.slots.extend(spell_ids);
    }
    order
}

/// One booked spell's slot off its record: its name, rank and passivity, and its icon, which
/// the melee auto-attack takes from `attack_icon` and a ranged shot from `ranged_icon` where
/// they give one (`0x4b3f8a`, `0x4b3f99`), else the spell's own (`0x4b3fae`). The checked ring,
/// the bar's face and the cooldown are the caller's.
fn slot_view(
    spell_id: u32,
    catalog: &SpellCatalog,
    attack_icon: impl FnOnce() -> Option<String>,
    ranged_icon: impl FnOnce() -> Option<String>,
) -> SpellSlotView {
    let d = catalog.get(spell_id);
    // Keyed on the effect type, as the action bar's substitution is.
    let texture = if d.is_some_and(|d| d.is_melee_auto_attack()) {
        attack_icon().or_else(|| d.and_then(|d| d.icon.clone()))
    } else if d.is_some_and(|d| d.ranged_icon_substitution()) {
        // Without a ranged weapon, or with a thrown one, the spell's own (`0x4e6990`).
        ranged_icon().or_else(|| d.and_then(|d| d.icon.clone()))
    } else {
        d.and_then(|d| d.icon.clone())
    };
    SpellSlotView {
        spell_id,
        name: d.map(|d| d.name.clone()).unwrap_or_default(),
        rank: d.and_then(|d| d.rank.clone()),
        texture,
        passive: d.is_some_and(|d| d.passive),
        current: false,
        cooldown: None,
        // `GetSpellAutocast` answers (nil, nil) for the player book, and a known spell has no
        // packed word.
        autocast: None,
        packed: 0,
        bar_texture: None,
    }
}

/// The book with fixed weapon icons, for the tests of the order and the icons.
#[cfg(test)]
fn build_book(
    known: &BTreeSet<u32>,
    catalog: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    race: u8,
    class: u8,
    attack_icon: Option<String>,
    ranged_icon: Option<String>,
) -> (benilla_ui::script::SpellBookState, Vec<u32>) {
    let order = book_order(known, catalog, skill_lines, race, class);
    let slots = order
        .slots
        .iter()
        .map(|&id| slot_view(id, catalog, || attack_icon.clone(), || ranged_icon.clone()))
        .collect();
    (
        benilla_ui::script::SpellBookState {
            tabs: order.tabs,
            slots,
        },
        order.tab_lines,
    )
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

/// `CastSpell(id, "spell", onSelf)` and `CastSpellByName(name, onSelf)` (`0x4b3300`'s player leg),
/// through the cast tail the action bar uses, at the selection as the calls before it left it, or
/// with `onSelf` at the caster.
pub(crate) fn cast_spell(cast: &mut crate::spell::ScriptCast, spell_id: u32, on_self: bool) {
    let crate::spell::ScriptCast { targeting, ladder } = cast;
    // `CastSpell`'s three forks ahead of the cast (`0x4b3300`), in order, all ahead of the gate:
    // the active-action toggle (`0x4b36f0`), the stop of the targeting cursor this spell armed
    // (`0x4b3474`-`0x4b347d`), then the form match (`0x4b348b`), which `UseAction` lacks, with its
    // silent no-op (`0x4b35cf`).
    let catalog_row = ladder
        .spells
        .as_ref()
        .and_then(|sp| Some((sp, targeting.self_store.iter().next()?)))
        .and_then(|(sp, store)| Some((sp, store, sp.catalog.get(spell_id)?)));
    if let Some((_, store, d)) = catalog_row {
        if crate::ui_action::toggle::active_action_toggle(spell_id, d, store) {
            debug!("ui_spellbook: cast {spell_id} re-pressed — aura cancels");
            let _ = ladder
                .commands
                .0
                .send(crate::net::ClientCommand::CancelAura { spell_id });
            return;
        }
    }
    // `GetTargetingSpellId 0x6e48e0` is this spell: the press toggles the cursor off and casts
    // nothing. Any other spell's cursor is aborted by the cast itself (`TryCast`, `0x6e4d62`).
    if ladder.ground.spell() == Some(spell_id) {
        debug!("ui_spellbook: cast {spell_id} re-pressed — targeting toggles off");
        ladder.stop_targeting();
        return;
    }
    if let Some((sp, store, d)) = catalog_row {
        let form = store.0.unit_shapeshift_form();
        let row = sp.forms.get(&u32::from(form));
        match crate::ui_action::toggle::form_recast_disposition(d, form, row) {
            Some(true) => {
                debug!("ui_spellbook: cast {spell_id} — the active form cancels");
                let _ = ladder
                    .commands
                    .0
                    .send(crate::net::ClientCommand::CancelAura { spell_id });
                return;
            }
            Some(false) => {
                debug!("ui_spellbook: cast {spell_id} — non-cancelable form, silent no-op");
                return;
            }
            None => {}
        }
    }
    debug!(
        "ui_spellbook: cast {spell_id} (target {:?}{})",
        targeting.selection.guid,
        if on_self { ", on self" } else { "" }
    );
    ladder.send_spell(spell_id, &targeting.context().self_bound(on_self), on_self);
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::SpellDisplay;
    use benilla_ui::script::SpellBookState;
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
        assert_eq!(book.tabs[0].name, SpellTabName::General);
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
