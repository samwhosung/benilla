//! The **pet spellbook** — the spellbook window's second tab.
//!
//! Its source has been on the wire and parsed since 0988 and nothing read it:
//! [`benilla_protocol::messages::PetSpells::spells`], the packed-word list `SMSG_PET_SPELLS`
//! carries after the ten bar slots. This module is the join that turns it into a book.
//!
//! **It is not the player book with a flag on it**, and the three differences are the reason this
//! is its own module rather than an arm of [`crate::ui_spellbook`]:
//!
//! - **A different add-gate.** `0x4b2f90` keeps a spell out only on `Attributes & 0x80`
//!   (`DO_NOT_DISPLAY`) — `SpellDisplay::in_pet_book`. The player book's `IS_TRADESKILL` and
//!   `castUI` legs are simply absent.
//! - **No tabs.** `GetNumSpellTabs`/`GetSpellTabInfo` never see this book;
//!   `SpellBook_GetSpellID`'s pet arm is a bare `return id`, so a button's own 1..12 id *is* the
//!   book id.
//! - **Two extra per-slot answers the player book has no analogue for** — the autocast pair, read
//!   off the pet's **raw** words (bits 31/30) rather than the filtered book, and the packed word
//!   itself, which is what `PickupSpell(id, "pet")` puts on the cursor.
//!
//! The **order** is shared, though: `0x4b2fd0(ecx = 0, edx = 1)` sorts the pet array with
//! `0x4b30c0`, the player book's own comparator (name, then the parsed rank number), so
//! [`crate::ui_spellbook::spell_sort_key`] serves both.
//!
//! ## Where each answer comes from
//!
//! | Lua | source |
//! |---|---|
//! | `GetSpellName` / `GetSpellTexture` / `IsSpellPassive` | `Spell.dbc` via [`Spells`] |
//! | `GetSpellCooldown(id, "pet")` | **bank 1** — [`PetBar::cooldowns`], the pet's own store. The reference reaches it with the same `0x6e2ea0(edx = isPet)` `GetPetActionCooldown` uses (`0x4b40dd`), so a spell on the bar and the same spell in the book read one timer, and they do here too. |
//! | `GetSpellAutocast` | the raw word's bits 31/30 |
//! | `IsCurrentCast` | the pet's own auras — `0x4b36f0`'s pet arm scans `[pet.fields + 0xa4]`, which is exactly the predicate the bar's [`crate::ui_pet`] already applies per slot |
//! | `HasPetSpells`'s token | `ChrClasses.dbc` field 4, via [`crate::chr_classes`] |
//!
//! ## The two verbs
//!
//! Both are pet verbs, and neither is the player book's:
//!
//! - **`CastSpell(id, "pet")` → `CMSG_PET_ACTION`** with a **synthesized** type-1 word,
//!   `0x0100_0000 | (spellId & 0xFFFF)` (`0x4b350a`-`0x4b3516`), aimed at the passed target or,
//!   absent one, the current selection (`0x4b34af`). So the book can cast a pet spell that is not
//!   on the bar at all — the word is built for the send, never looked up.
//! - **`ToggleSpellAutocast(id, "pet")` → `CMSG_PET_SPELL_AUTOCAST 0x2F3`**, naming the **spell**.
//!   Its sender `0x4bccb0` does three things before the packet leaves, and all three are
//!   reproduced in [`drain_pet_book`]: it flips bit 30 in the raw word in place, it **mirrors that
//!   bit onto every bar slot carrying the same action** (`0x4bcd20`-`0x4bcd5a`, compared under
//!   `& 0x3FFFFFFF`), and it fires `PET_BAR_UPDATE`. That mirror is why toggling autocast in the
//!   book lights the sparkle on the bar button in the same frame.

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
                // Rides the unit feed beside the player book's, and before the input pass so a
                // tab flipped this frame already reads a populated book.
                feed_pet_book
                    .in_set(UnitFeed)
                    .before(crate::ui_action::CooldownEvents),
                // After the input pass, and — like `ui_pet`'s own drain — writing back into
                // `PetBar`, whose next feed carries the mirrored autocast bit.
                drain_pet_book.after(UiInput),
            ),
        );
    }
}

/// What the feed last pushed, so `SPELLS_CHANGED` fires on a real change rather than every frame
/// (the [`crate::ui_spellbook`] memory pattern).
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
    // Nothing to resolve a name/icon/passive from yet — try again once Spell.dbc lands. Unlike the
    // bar (which can still draw its command tokens), a book with no catalog has no slots at all.
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
        // The token is the PLAYER's class, not the pet's — `0x4b4463` reads `player.fields + 0x79`.
        // A warlock reading "Demon" while a hunter reads "Pet" is the whole of it.
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
        // The reference fires ONE `SPELLS_CHANGED` for both books off the shared re-sort
        // (`0x4b2fd0` tail-jumps `SignalEvent(0x104)`), which is also what re-reads the spell
        // buttons. The cooldown/autocast half rides `PET_BAR_UPDATE` instead — `SpellButton_OnLoad`
        // registers it precisely so the pet page can repaint without a book change
        // (`SpellBookFrame.lua:214`, `l.227-231`) — and `ui_pet`'s own feed already fires that.
        if changed {
            script.fire_event("SPELLS_CHANGED", vec![]);
        }
    }
}

/// Did the book itself move, as opposed to a per-slot cooldown/autocast/ring bit? Only the former
/// is `SPELLS_CHANGED`; the latter reaches the buttons through `PET_BAR_UPDATE`.
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
        // `IsCurrentCast`'s pet arm (`0x4b36f0`, and the delegate `0x4b3600` it shares a shape
        // with): the ring lights while the spell's aura is on the PET. Same predicate the bar's
        // ActiveIcon swap uses, so a lit book slot and a lit bar slot always agree.
        current: pet_store
            .is_some_and(|s| crate::ui_action::toggle::active_action_toggle(spell_id, d, s)),
        // **Bank 1** — the pet's own store, the same one `GetPetActionCooldown` reads.
        cooldown: bar
            .cooldowns
            .info(spell_id, 0, Some(d), now)
            .ui_triple(anchor, ui_now),
        // The autocast pair off the RAW word. `autocast_allowed` is additionally gated on the
        // spell resolving in `Spell.dbc` (`0x4bdd65`'s own second test) — which it has, or we
        // would not be here.
        autocast: Some((entry.autocast_allowed(), entry.autocast_on())),
        packed: entry.packed,
    }
}

/// Drain the pet book's two intents.
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
        // **The press-again-to-cancel arm comes first** (`0x4b33af`-`0x4b3461`), exactly as it does
        // on the bar: a click on a pet spell whose aura is already running on the pet sends
        // `CMSG_PET_CANCEL_AURA` (0x26B) **and returns** — the ordinary order never leaves. Same
        // predicate, same order of tests, one implementation
        // (`crate::ui_action::toggle::active_action_toggle`).
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
        // The synthesized word, `0x4b350a`-`0x4b3516`: `(spellId & 0xFFFF) | 0x01000000`. Type 1
        // is the client's spell branch and the autocast bits are left clear — this word describes
        // the ORDER, not the slot, so nothing is read back out of it.
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
        // `0x4bcdc6`'s `SignalEvent(0x161)` — the bar repainted before the packet left, which is
        // the same "write locally, never wait for a reply" law the rest of this bar runs on
        // (`ui_pet`'s module doc). Bumping the signal count is how that repaint reaches the feed.
        bar.bar_signals = bar.bar_signals.wrapping_add(1);
        debug!("ui_pet_book: autocast {spell_id} -> {on}");
        let _ = commands.0.send(ClientCommand::PetSpellAutocast {
            pet_guid,
            spell_id,
            enabled: on,
        });
    }
}

/// `0x4bccb0`'s local half, in its own order: find the spell's **raw** word, flip bit 30 in place,
/// and mirror the new bit onto every **bar** slot carrying the same action. Returns the new state,
/// or `None` when the reference would have aborted before sending anything.
///
/// The mirror is the load-bearing half and the easy one to miss (`0x4bcd20`-`0x4bcd5a`): the loop
/// walks all ten bar words, compares `(bar[i] ^ word) & 0x3FFFFFFF` — type and action, autocast
/// bits excluded — and copies bit 30 across on a match. Without it, flipping autocast in the book
/// leaves the bar button's sparkle stale until the next `SMSG_PET_SPELLS`.
///
/// The gate is `0x4bccf5`: **the word must be autocast-ALLOWED** (bit 31). A passive's is not, and
/// neither is a command token's — so neither can be toggled from anywhere.
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

    /// **The pet book's add-gate is not the player book's.** `0x4b2f90` tests exactly one bit —
    /// `Attributes & 0x80` (DO_NOT_DISPLAY) — where the player book also refuses `IS_TRADESKILL`
    /// (`0x20`) and `castUI != 0`. A spell with either of those two SHOWS in a pet book.
    #[test]
    fn the_pet_books_gate_is_do_not_display_alone() {
        let shown = |attributes: u32, cast_ui: u32| SpellDisplay {
            attributes,
            cast_ui,
            ..Default::default()
        };
        assert!(shown(0, 0).in_pet_book());
        assert!(!shown(0x80, 0).in_pet_book(), "DO_NOT_DISPLAY hides it");
        // The two the PLAYER book additionally refuses — kept here, and this is the whole point.
        assert!(
            shown(0x20, 0).in_pet_book(),
            "IS_TRADESKILL is not a pet-book gate"
        );
        assert!(!shown(0x20, 0).in_spellbook());
        assert!(shown(0, 1).in_pet_book(), "castUI is not a pet-book gate");
        assert!(!shown(0, 1).in_spellbook());
        // A passive is shown in both — being passive was never a book gate.
        assert!(shown(0x40, 0).in_pet_book() && shown(0x40, 0).in_spellbook());
    }

    /// The autocast toggle's local half (`0x4bccb0`): the flip lands on the raw **book** word AND
    /// on every **bar** slot carrying the same action, compared under `& 0x3FFFFFFF`. Missing the
    /// mirror leaves the bar button's sparkle stale until the next `SMSG_PET_SPELLS`.
    #[test]
    fn the_book_toggle_mirrors_onto_the_bar() {
        const CLAW: u32 = 16827;
        let mut spells = PetSpells {
            pet_guid: 0x2A,
            spells: vec![
                word(PET_ACT_DISABLED, CLAW), // autocastable, currently OFF
                word(PET_ACT_PASSIVE, 3025),  // a passive: not autocastable
            ],
            ..Default::default()
        };
        // Claw sits in bar slot 4; slot 5 holds a DIFFERENT spell with the same low bits pattern
        // is impossible, so use a plain second spell to prove the compare is by action.
        spells.bar[4] = word(PET_ACT_DISABLED, CLAW);
        spells.bar[5] = word(PET_ACT_DISABLED, 2649);

        assert_eq!(flip_autocast(&mut spells, CLAW), Some(true));
        assert!(spells.spells[0].autocast_on(), "the book word flipped");
        assert!(spells.bar[4].autocast_on(), "…and so did the bar slot");
        assert!(
            !spells.bar[5].autocast_on(),
            "a different action is untouched"
        );

        // Flipping back does the same in reverse.
        assert_eq!(flip_autocast(&mut spells, CLAW), Some(false));
        assert!(!spells.spells[0].autocast_on());
        assert!(!spells.bar[4].autocast_on());

        // `0x4bccf5`: a word that is not autocast-ALLOWED aborts before anything is written.
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

    /// A word already ON flips OFF and an `ACT_ENABLED` word starts ON — the two server states our
    /// decode collapses onto one type (`0988`), which is exactly why the flip reads bit 30 and not
    /// the type byte.
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
