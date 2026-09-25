//! **A bar slot never holds a rank the character has outgrown** — the one place
//! that keeps the action bar's spell ids in step with the spell book's ranks.
//!
//! The bar is client-authoritative: the server stores 120 packed words and
//! hands them back at login, and nothing on the server side ever rewrites them when a rank is
//! learned (VERIFIED vmangos — `Player::AddSpell`'s supersede path touches only the spell store;
//! the only `ConvertSpell` that walks `character_action` is the race-change tool). So the stored
//! bar drifts, and it drifts *silently*: a superseded rank is dropped from the book
//! (`Player::SendInitialSpells` skips `!active` spells), so the button comes back pointing at a
//! spell the character no longer knows — dead, with no way to tell from the icon, which is the
//! same art on every rank. That is the reported bug: a level-60 rogue's saved slot 1 still held
//! Sinister Strike **rank 1** (`character_action.action = 1752`) while the book held only rank 8
//! (11294), so the button did nothing.
//!
//! The chain lives in `SkillLineAbility.dbc`'s `forward_spellid` — 406 of the build's 4753 spells
//! carry it, and they are exactly the abilities the server supersedes (every
//! `SMSG_SUPERCEDED_SPELL` vmangos sends is gated on `GetSpellBookSuccessorSpellId`, which reads
//! that column). Warrior/rogue physical abilities and the profession tier openers are chained;
//! caster nukes and heals are not — their lower ranks stay known and castable, which is what
//! makes vanilla down-ranking work. Normalization therefore cannot touch a deliberate down-rank:
//! it only ever moves within a chain, and a chain is precisely a family where only one rank is
//! ever known at a time.
//!
//! This runs on the `dirty` flag — every book arrival, rank-up and local slot write — so it is
//! ONE mechanism covering all of them, not a login special case. Hence
//! [`super::spells::superceded_spell`] no longer re-points buttons itself: the server's
//! `SMSG_SUPERCEDED_SPELL` moves the *book*, and the bar follows from here. It has to be that way
//! round, because the case that bit us got no supersede packet at all (vmangos suppresses them
//! while the character is loading, so ranks gained offline — a `.levelup`, a boost — arrive as a
//! book that silently disagrees with the bar).

use bevy::prelude::*;

use benilla_protocol::messages::ACTION_KIND_SPELL;

use super::PlayerActions;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_spellbook::SkillLines;

/// Re-point every spell button at the highest rank of its ability the book actually holds, and
/// persist each move (a local slot mutation IS a `CMSG_SET_ACTION_BUTTON` — the
/// bar has no other writer, so a fix we keep to ourselves would be re-applied every single login
/// while the server's copy stayed wrong).
///
/// Gated on `dirty` and run before [`super::feed::feed_actions`] consumes it, so the corrected id
/// is what the feed resolves and pushes — the stale rank never reaches a frame.
pub(super) fn normalize_action_ranks(
    mut actions: ResMut<PlayerActions>,
    skill_lines: Option<Res<SkillLines>>,
    commands: Res<NetCommands>,
) {
    let Some(skill_lines) = skill_lines else {
        return;
    };
    if !actions.dirty {
        return;
    }
    // Resolve first (the walk reads `buttons` and `spells` together), then write: an empty book
    // yields no fixes at all, which is what keeps a bar-before-book packet order harmless.
    let fixes: Vec<(u8, u32)> = actions
        .buttons
        .values()
        .filter(|b| b.kind == ACTION_KIND_SPELL)
        .filter_map(|b| {
            let top = skill_lines
                .catalog
                .highest_known_rank(b.action, &actions.spells)?;
            (top != b.action).then_some((b.slot, top))
        })
        .collect();
    for (slot, top) in fixes {
        let Some(button) = actions.buttons.get_mut(&slot) else {
            continue;
        };
        let was = button.action;
        button.action = top;
        let packed = top | (u32::from(button.kind) << 24);
        info!("ui_action: bar slot {slot} rank-normalized {was} -> {top}");
        let _ = commands.0.send(ClientCommand::SetActionButton {
            button: slot,
            packed,
        });
    }
}

#[cfg(test)]
mod tests {
    use benilla_protocol::messages::{ActionButton, ACTION_KIND_ITEM, ACTION_KIND_MACRO};
    use crossbeam_channel::Receiver;

    use super::*;

    /// Sinister Strike's real chain, ranks 1..8 (pinned against the DBC by `benilla_formats`'
    /// own `real_rank_chains_resolve_the_highest_known_rank`).
    const SS: [u32; 8] = [1752, 1757, 1758, 1759, 1760, 8621, 11293, 11294];

    /// These run the walk against the **real** `SkillLineAbility.dbc` — the chain is the thing
    /// under test, and a hand-built fake chain would only test the walk against itself.
    fn client_data() -> Option<std::path::PathBuf> {
        let data = benilla_formats::wow_data_or_skip!(None);
        Some(data)
    }

    fn spell_button(slot: u8, action: u32) -> ActionButton {
        ActionButton {
            slot,
            action,
            kind: ACTION_KIND_SPELL,
        }
    }

    /// Run the system once over `buttons` + a book of `spells`, on the real catalog. Returns the
    /// resulting store and the `(button, packed)` pairs that went out on the wire.
    fn normalize(
        data: &std::path::Path,
        buttons: &[ActionButton],
        spells: &[u32],
    ) -> (PlayerActions, Vec<(u8, u32)>) {
        let mut chain = benilla_formats::open_chain(data).expect("open chain");
        let catalog = benilla_formats::load_skill_line_catalog(&mut chain).expect("skill lines");

        let (tx, rx): (_, Receiver<ClientCommand>) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(PlayerActions {
            buttons: buttons.iter().map(|b| (b.slot, *b)).collect(),
            spells: spells.iter().copied().collect(),
            dirty: true,
        })
        .insert_resource(SkillLines { catalog })
        .insert_resource(NetCommands(tx))
        .add_systems(Update, normalize_action_ranks);
        app.update();

        let sent = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::SetActionButton { button, packed } => (button, packed),
                _ => panic!("the rank pass sends nothing but SetActionButton"),
            })
            .collect();
        let store = app.world_mut().remove_resource::<PlayerActions>().unwrap();
        (store, sent)
    }

    /// **The reported bug.** A level-60 rogue's saved slot 1 holds Sinister Strike rank 1 while
    /// the book holds only rank 8: the slot moves to rank 8 AND the move goes out on the wire, so
    /// the server's stored copy stops being wrong.
    #[test]
    fn a_stale_rank_1_slot_moves_to_the_known_rank_and_persists() {
        let Some(data) = client_data() else { return };
        let (store, sent) = normalize(&data, &[spell_button(0, 1752)], &[11294]);
        assert_eq!(store.buttons[&0].action, 11294, "slot 1 holds rank 8");
        assert!(store.dirty, "the feed still re-resolves the slot");
        assert_eq!(sent, vec![(0, 11294)]);
    }

    /// Every rank below the known one lands on the same answer — including from *above* it, the
    /// down-rank direction a forward-only walk can't reach. The already-correct slot doesn't move
    /// and doesn't send.
    #[test]
    fn every_wrong_rank_converges_and_the_right_one_stays_put() {
        let Some(data) = client_data() else { return };
        let buttons: Vec<ActionButton> = SS
            .iter()
            .enumerate()
            .map(|(i, &id)| spell_button(i as u8, id))
            .collect();
        // Rank 4 (1759) known: ranks 1-3 move UP to it, ranks 5-8 move DOWN to it.
        let (store, sent) = normalize(&data, &buttons, &[1759]);
        for slot in 0..8u8 {
            assert_eq!(store.buttons[&slot].action, 1759, "slot {slot}");
        }
        assert_eq!(sent.len(), 7, "one send per moved slot (slot 3 was right)");
        assert!(!sent.iter().any(|(b, _)| *b == 3));
    }

    /// A caster's down-rank is NOT a mistake: Fireball's ranks carry no `forward_spellid`, so a
    /// slot holding rank 1 stays on rank 1 even with rank 2 known and castable.
    #[test]
    fn an_unchained_caster_rank_is_left_alone() {
        let Some(data) = client_data() else { return };
        let (store, sent) = normalize(&data, &[spell_button(0, 133)], &[133, 143]);
        assert_eq!(store.buttons[&0].action, 133);
        assert!(sent.is_empty(), "nothing to persist");
    }

    /// With no rank of the chain known — the book hasn't arrived yet, or the ability was never
    /// learned — the slot is left exactly as it is rather than pointed somewhere arbitrary.
    #[test]
    fn an_unknown_chain_leaves_the_slot_untouched() {
        let Some(data) = client_data() else { return };
        let (store, sent) = normalize(&data, &[spell_button(0, 1752)], &[]);
        assert_eq!(store.buttons[&0].action, 1752);
        assert!(sent.is_empty());
    }

    /// Macro and item slots carry ids from other namespaces — walking them as spell ranks would
    /// be nonsense, so the kind byte gates it.
    #[test]
    fn macro_and_item_slots_are_never_walked() {
        let Some(data) = client_data() else { return };
        let buttons = [
            ActionButton {
                slot: 0,
                action: 1752,
                kind: ACTION_KIND_MACRO,
            },
            ActionButton {
                slot: 1,
                action: 1752,
                kind: ACTION_KIND_ITEM,
            },
        ];
        let (store, sent) = normalize(&data, &buttons, &[11294]);
        assert_eq!(store.buttons[&0].action, 1752);
        assert_eq!(store.buttons[&1].action, 1752);
        assert!(sent.is_empty());
    }

    /// A correct bar is a no-op — nothing moves, nothing is sent. That is what lets this ride the
    /// `dirty` flag (set by every drag-and-drop) without adding traffic.
    #[test]
    fn a_correct_bar_sends_nothing() {
        let Some(data) = client_data() else { return };
        // Rank 8 on the bar, plus the auto-attack (no `SkillLineAbility` row at all).
        let buttons = [
            spell_button(0, 11294),
            spell_button(1, super::super::SPELL_ATTACK),
        ];
        let (_, sent) = normalize(&data, &buttons, &[11294, super::super::SPELL_ATTACK]);
        assert!(sent.is_empty());
    }
}
