//! Keeps each staged spell word on the highest rank of its chain the book holds, and sends each
//! move. A chain is `SkillLineAbility.dbc`'s `forward_spellid`, the column the server's supersede
//! reads; caster ranks are unchained, so a deliberate down-rank is never moved. Deviation: the
//! book's and the bar's login packets run this over every staged spell word, where the reference re-points slots
//! only at `SMSG_SUPERCEDED_SPELL`, old rank to new (`0x4e5e60`, [`super::spell_slots`]), because
//! vmangos never rewrites stored slots on a rank-up (`Player::AddSpell`), drops superseded ranks
//! from the book (`Player.cpp:3463`) and sends no supersede while loading (`Player.cpp:3704`), so
//! a stored slot can name a spell the character no longer knows.

use std::collections::BTreeSet;

use benilla_formats::SkillLineCatalog;
use benilla_protocol::messages::ACTION_KIND_SPELL;
use bevy::prelude::*;

use super::ActionSlots;
use crate::net::{ClientCommand, NetCommands};

/// Re-points each staged spell word at the highest rank of its chain in `known` and sends the
/// move, so the server's stored copy is fixed too; our player's arrival, behind the login's
/// packets, loads the moved words, which the book knows, into the bar and the VM's copy
/// (`super::recount::arrival_slot_pass`). An empty book yields no fixes, so a bar arriving first
/// is harmless.
pub(crate) fn normalize_ranks(
    slots: &mut ActionSlots,
    known: &BTreeSet<u32>,
    skill_lines: &SkillLineCatalog,
    commands: &NetCommands,
) {
    // Resolve, then write.
    let fixes: Vec<(u8, u32)> = slots
        .staged
        .values()
        .filter(|b| b.kind == ACTION_KIND_SPELL)
        .filter_map(|b| {
            let top = skill_lines.highest_known_rank(b.action, known)?;
            (top != b.action).then_some((b.slot, top))
        })
        .collect();
    for (slot, top) in fixes {
        let Some(button) = slots.staged.get_mut(&slot) else {
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

    /// Sinister Strike's chain, ranks 1 to 8.
    const SS: [u32; 8] = [1752, 1757, 1758, 1759, 1760, 8621, 11293, 11294];

    /// These run against the real `SkillLineAbility.dbc`: the chain is what is under test.
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

    /// Runs the pass once; returns the store and the `(button, packed)` pairs sent.
    fn normalize(
        data: &std::path::Path,
        buttons: &[ActionButton],
        spells: &[u32],
    ) -> (ActionSlots, Vec<(u8, u32)>) {
        let mut chain = benilla_formats::open_chain(data).expect("open chain");
        let catalog = benilla_formats::load_skill_line_catalog(&mut chain).expect("skill lines");

        let (tx, rx): (_, Receiver<ClientCommand>) = crossbeam_channel::unbounded();
        let mut store = ActionSlots {
            staged: buttons.iter().map(|b| (b.slot, *b)).collect(),
            ..Default::default()
        };
        let known = spells.iter().copied().collect();
        normalize_ranks(&mut store, &known, &catalog, &NetCommands(tx));
        let sent = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::SetActionButton { button, packed } => (button, packed),
                _ => panic!("the rank pass sends nothing but SetActionButton"),
            })
            .collect();
        (store, sent)
    }

    #[test]
    fn a_stale_rank_1_slot_moves_to_the_known_rank_and_persists() {
        let Some(data) = client_data() else { return };
        let (store, sent) = normalize(&data, &[spell_button(0, 1752)], &[11294]);
        assert_eq!(store.staged[&0].action, 11294, "slot 1 holds rank 8");
        assert_eq!(sent, vec![(0, 11294)]);
    }

    #[test]
    fn every_wrong_rank_converges_and_the_right_one_stays_put() {
        let Some(data) = client_data() else { return };
        let buttons: Vec<ActionButton> = SS
            .iter()
            .enumerate()
            .map(|(i, &id)| spell_button(i as u8, id))
            .collect();
        // Rank 4 (spell 1759) known: ranks 1-3 move up to it, ranks 5-8 down.
        let (store, sent) = normalize(&data, &buttons, &[1759]);
        for slot in 0..8u8 {
            assert_eq!(store.staged[&slot].action, 1759, "slot {slot}");
        }
        assert_eq!(sent.len(), 7, "one send per moved slot (slot 3 was right)");
        assert!(!sent.iter().any(|(b, _)| *b == 3));
    }

    /// Fireball's ranks 1 and 2 (133, 143) carry no `forward_spellid`.
    #[test]
    fn an_unchained_caster_rank_is_left_alone() {
        let Some(data) = client_data() else { return };
        let (store, sent) = normalize(&data, &[spell_button(0, 133)], &[133, 143]);
        assert_eq!(store.staged[&0].action, 133);
        assert!(sent.is_empty(), "nothing to persist");
    }

    #[test]
    fn an_unknown_chain_leaves_the_slot_untouched() {
        let Some(data) = client_data() else { return };
        let (store, sent) = normalize(&data, &[spell_button(0, 1752)], &[]);
        assert_eq!(store.staged[&0].action, 1752);
        assert!(sent.is_empty());
    }

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
        assert_eq!(store.staged[&0].action, 1752);
        assert_eq!(store.staged[&1].action, 1752);
        assert!(sent.is_empty());
    }

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
