//! The pet spellbook, the spellbook window's second tab, built from the packed words
//! `SMSG_PET_SPELLS` carries after the ten bar slots
//! ([`benilla_protocol::messages::PetSpells::spells`]).
//!
//! Unlike the player book, its add-gate (`0x4b2f90`) refuses only `Attributes & 0x80`
//! (`DO_NOT_DISPLAY`); it has no tabs, so a button's 1..12 id is the book id; and each slot
//! carries the raw word's autocast bits (31, 30) and the word itself, which `PickupSpell` puts on
//! the cursor. The order is shared: `0x4b2fd0` sorts with `0x4b30c0`, the player book's comparator.
//!
//! A book cooldown is the pet's own store, the one `GetPetActionCooldown` reads (`0x6e2ea0` with
//! `isPet`, `0x4b40dd`), so bar and book share one timer. `CastSpell(id, "pet")` sends
//! `CMSG_PET_ACTION` with a word built for the send, so the book casts spells not on the bar.
//! `ToggleSpellAutocast(id, "pet")` sends `CMSG_PET_SPELL_AUTOCAST` (0x2F3) after `0x4bccb0` flips
//! bit 30 locally, mirrors it onto the bar and fires `PET_BAR_UPDATE`.

use std::time::Instant;

use bevy::prelude::*;

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::PetActionEntry;
use benilla_ui::script::{PetBookState, SpellSlotView, UiScript};

use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::target::Selection;
use crate::ui_action::Spells;
use crate::ui_pet::PetBar;
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;

pub(crate) struct UiPetBookPlugin;

impl Plugin for UiPetBookPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // Before the input pass, so a tab flipped this frame reads a populated book.
                feed_pet_book
                    .in_set(UnitFeed)
                    .before(crate::ui_action::CooldownEvents),
                // After the input pass; it writes back into `PetBar`, whose next feed carries the
                // mirrored autocast bit.
                drain_pet_book.after(UiInput),
            ),
        );
    }
}

/// What the feed last pushed, so `SPELLS_CHANGED` fires only on a real change.
#[derive(Default)]
struct FeedMemory {
    pushed: PetBookState,
}

fn feed_pet_book(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    spells: Option<Res<Spells>>,
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    clock: Res<crate::ui_script::UiClock>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memory = memory.get(&script);
    // Without `Spell.dbc` the book has no slots at all; try again once it lands.
    let Some(spells) = spells.as_deref() else {
        return;
    };
    let now = Instant::now();
    let (anchor, ui_now) = (clock.anchor, clock.ui_now);
    let pet_store = index
        .0
        .get(&bar.spells.pet_guid)
        .and_then(|&e| stores.get(e).ok());

    let mut fresh = PetBookState {
        // `HasPetSpells`'s token is the player's class, not the pet's (`0x4b4463` reads
        // `player.fields + 0x79`), named by `ChrClasses.dbc` field 4.
        token: self_q.single().ok().and_then(|s| {
            let class = u32::from(s.0.unit_class()?);
            classes
                .as_deref()
                .map(|t| t.0.pet_name_token(class).to_string())
        }),
        slots: Vec::new(),
    };
    if bar.has_bar() {
        for &entry in &bar.spells.spells {
            let spell_id = entry.action();
            let Some(d) = spells.catalog.get(spell_id) else {
                // `0x4b2f90`'s first gate: no `Spell.dbc` record, no book slot.
                continue;
            };
            if !d.in_pet_book() {
                continue;
            }
            fresh
                .slots
                .push(slot_view(entry, d, &bar, pet_store, now, anchor, ui_now));
        }
        // `0x4b2fd0`'s re-sort, with the player book's own comparator.
        fresh
            .slots
            .sort_by_key(|s| crate::ui_spellbook::spell_sort_key_of(&s.name, s.rank.as_deref()));
    }

    if fresh != memory.pushed {
        debug!("ui_pet_book: fed {} pet spell(s)", fresh.slots.len());
        let changed = book_changed(&fresh, &memory.pushed);
        script.set_pet_book(fresh.clone());
        memory.pushed = fresh;
        // One `SPELLS_CHANGED` for both books off the shared re-sort (`0x4b2fd0` tail-jumps
        // `SignalEvent(0x104)`). Cooldown and autocast repaint the pet page through
        // `PET_BAR_UPDATE` (`SpellBookFrame.lua:214`, `227-231`), which `ui_pet`'s feed fires.
        if changed {
            script.fire_event("SPELLS_CHANGED", vec![]);
        }
    }
}

/// Whether the book itself changed; a slot's cooldown, autocast or ring reaches the buttons
/// through `PET_BAR_UPDATE` instead.
fn book_changed(fresh: &PetBookState, old: &PetBookState) -> bool {
    fresh.token != old.token
        || fresh.slots.len() != old.slots.len()
        || fresh.slots.iter().zip(&old.slots).any(|(a, b)| {
            (a.spell_id, &a.name, &a.rank, &a.texture, a.passive)
                != (b.spell_id, &b.name, &b.rank, &b.texture, b.passive)
        })
}

/// One book slot, fully resolved.
fn slot_view(
    entry: PetActionEntry,
    d: &SpellDisplay,
    bar: &PetBar,
    pet_store: Option<&ObjectStore>,
    now: Instant,
    anchor: Instant,
    ui_now: f64,
) -> SpellSlotView {
    let spell_id = entry.action();
    SpellSlotView {
        spell_id,
        name: d.name.clone(),
        rank: d.rank.clone(),
        texture: d.icon.clone(),
        passive: d.passive,
        // `IsCurrentCast`'s pet arm (`0x4b36f0`, and its delegate `0x4b3600`): lit while the
        // spell's aura is on the pet (`[pet.fields + 0xa4]`), the bar's own predicate.
        current: pet_store
            .is_some_and(|s| crate::ui_action::toggle::active_action_toggle(spell_id, d, s)),
        // The pet's own cooldown store, the one `GetPetActionCooldown` reads.
        cooldown: bar
            .cooldowns
            .info(spell_id, 0, Some(d), now)
            .ui_triple(anchor, ui_now),
        // Off the raw word; `0x4bdd65` also requires a `Spell.dbc` record, which every slot has.
        autocast: Some((entry.autocast_allowed(), entry.autocast_on())),
        packed: entry.packed,
    }
}

/// Drains the pet book's two intents.
fn drain_pet_book(
    script: Option<NonSendMut<UiScript>>,
    mut bar: ResMut<PetBar>,
    selection: Res<Selection>,
    commands: Res<NetCommands>,
    spells: Option<Res<Spells>>,
    pet: crate::ui_pet::PetUnit,
) {
    let Some(mut script) = script else {
        return;
    };
    let casts = script.take_pet_spell_casts();
    let autocasts = script.take_pet_spell_autocasts();
    if casts.is_empty() && autocasts.is_empty() {
        return;
    }
    let pet_guid = bar.spells.pet_guid;
    if pet_guid == 0 {
        debug!("ui_pet_book: dropping queued pet book intents — the bar is gone");
        return;
    }
    let target_guid = selection.guid.unwrap_or(0);
    let pet_store = pet.store(pet_guid);

    for spell_id in casts {
        // Cancel first, as on the bar (`0x4b33af`-`0x4b3461`): a spell whose aura is on the pet
        // sends `CMSG_PET_CANCEL_AURA` (0x26B) instead of the order.
        let display = spells.as_ref().and_then(|s| s.catalog.get(spell_id));
        if let (Some(d), Some(store)) = (display, pet_store) {
            if crate::ui_action::toggle::active_action_toggle(spell_id, d, store) {
                debug!("ui_pet_book: cast {spell_id} cancels the pet's own aura");
                let _ = commands
                    .0
                    .send(ClientCommand::PetCancelAura { pet_guid, spell_id });
                continue;
            }
        }
        // The word built for the send (`0x4b350a`-`0x4b3516`): type 1, the spell branch, autocast
        // bits clear; with no target passed it aims at the selection (`0x4b34af`).
        let packed = 0x0100_0000 | (spell_id & 0xFFFF);
        debug!("ui_pet_book: cast {spell_id} (target {target_guid:#x})");
        let _ = commands.0.send(ClientCommand::PetAction {
            pet_guid,
            packed,
            target_guid,
        });
    }

    for spell_id in autocasts {
        let Some(on) = flip_autocast(&mut bar.spells, spell_id) else {
            continue;
        };
        // `0x4bcdc6`'s `SignalEvent(0x161)`: the bar repaints before the packet leaves, and the
        // signal count carries that to the feed.
        bar.bar_signals = bar.bar_signals.wrapping_add(1);
        debug!("ui_pet_book: autocast {spell_id} -> {on}");
        let _ = commands.0.send(ClientCommand::PetSpellAutocast {
            pet_guid,
            spell_id,
            enabled: on,
        });
    }
}

/// `0x4bccb0`'s local half: flip bit 30 of the spell's raw word, then copy it onto every bar slot
/// whose type and action match under `& 0x3FFFFFFF` (`0x4bcd20`-`0x4bcd5a`), or the bar's sparkle
/// stays stale until the next `SMSG_PET_SPELLS`. `None` where the reference aborts unsent: the
/// word is not autocast-allowed (`0x4bccf5`), as a passive's and a command token's are not.
fn flip_autocast(
    spells: &mut benilla_protocol::messages::PetSpells,
    spell_id: u32,
) -> Option<bool> {
    let word = spells
        .spells
        .iter_mut()
        .find(|w| w.action() == spell_id && w.autocast_allowed())?;
    let on = !word.autocast_on();
    *word = word.with_autocast(on);
    let action = word.packed & 0x3FFF_FFFF;
    for slot in &mut spells.bar {
        if slot.packed & 0x3FFF_FFFF == action {
            *slot = slot.with_autocast(on);
        }
    }
    Some(on)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{
        PetSpells, PET_ACT_DISABLED, PET_ACT_ENABLED, PET_ACT_PASSIVE,
    };

    fn word(kind: u8, spell_id: u32) -> PetActionEntry {
        PetActionEntry::from((u32::from(kind) << 24) | spell_id)
    }

    #[test]
    fn the_pet_books_gate_is_do_not_display_alone() {
        let shown = |attributes: u32, cast_ui: u32| SpellDisplay {
            attributes,
            cast_ui,
            ..Default::default()
        };
        assert!(shown(0, 0).in_pet_book());
        assert!(!shown(0x80, 0).in_pet_book(), "DO_NOT_DISPLAY hides it");
        // The two the player book also refuses.
        assert!(
            shown(0x20, 0).in_pet_book(),
            "IS_TRADESKILL is not a pet-book gate"
        );
        assert!(!shown(0x20, 0).in_spellbook());
        assert!(shown(0, 1).in_pet_book(), "castUI is not a pet-book gate");
        assert!(!shown(0, 1).in_spellbook());
        // A passive shows in both.
        assert!(shown(0x40, 0).in_pet_book() && shown(0x40, 0).in_spellbook());
    }

    #[test]
    fn the_book_toggle_mirrors_onto_the_bar() {
        const CLAW: u32 = 16827;
        let mut spells = PetSpells {
            pet_guid: 0x2A,
            spells: vec![
                word(PET_ACT_DISABLED, CLAW), // autocastable, off
                word(PET_ACT_PASSIVE, 3025),  // a passive: not autocastable
            ],
            ..Default::default()
        };
        // Claw in bar slot 4 and another spell in slot 5: the compare is by action.
        spells.bar[4] = word(PET_ACT_DISABLED, CLAW);
        spells.bar[5] = word(PET_ACT_DISABLED, 2649);

        assert_eq!(flip_autocast(&mut spells, CLAW), Some(true));
        assert!(spells.spells[0].autocast_on(), "the book word flipped");
        assert!(spells.bar[4].autocast_on(), "…and so did the bar slot");
        assert!(
            !spells.bar[5].autocast_on(),
            "a different action is untouched"
        );

        // Flipping back reverses both.
        assert_eq!(flip_autocast(&mut spells, CLAW), Some(false));
        assert!(!spells.spells[0].autocast_on());
        assert!(!spells.bar[4].autocast_on());

        // `0x4bccf5`: a word not autocast-allowed aborts before anything is written.
        assert_eq!(
            flip_autocast(&mut spells, 3025),
            None,
            "a passive cannot be toggled"
        );
        assert_eq!(
            flip_autocast(&mut spells, 99),
            None,
            "nor can a spell the pet lacks"
        );
    }

    /// `ACT_ENABLED` and `ACT_DISABLED` decode to one type, so only bit 30 tells them apart.
    #[test]
    fn the_flip_reads_bit_thirty_not_the_type_byte() {
        const GROWL: u32 = 2649;
        let mut spells = PetSpells {
            spells: vec![word(PET_ACT_ENABLED, GROWL)],
            ..Default::default()
        };
        assert!(
            spells.spells[0].autocast_on(),
            "ACT_ENABLED means bit 30 is set"
        );
        assert_eq!(flip_autocast(&mut spells, GROWL), Some(false));
        assert!(!spells.spells[0].autocast_on());
        assert!(
            spells.spells[0].autocast_allowed(),
            "…and it stays ALLOWED — only bit 30 moves"
        );
    }
}
