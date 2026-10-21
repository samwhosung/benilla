//! `"target"`'s portrait under a selection whose object the object manager does not hold.

use bevy::prelude::*;

use super::{player_temporary_portrait, target_subject, Subject};
use crate::names::NameCache;
use crate::target::Selection;
use crate::ui_party::GroupState;

const MEMBER: u64 = 0x51;
const CREATURE: u64 = 0xF130_0000_0000_0031;
/// A player whose name record the cache does not hold.
const STRANGER: u64 = 0x52;

/// A selection with no object, a party member out of range, draws as any unit the object manager
/// does not hold (`0x519fe6`): the bake cached under its guid, else the stand-in of the race and
/// sex its name record holds (`0x51a08e` → `0x525ba0`); a player with no name record draws nothing
/// (`0x51a095`), nor a guid that is no player and has no pet record (`0x51a0ce`). A held selection
/// bakes its object.
#[test]
fn a_selection_with_no_object_draws_its_stand_in() {
    let roster = GroupState::default();
    let mut names = NameCache::default();
    // Night elf, warrior, female.
    names.insert_player(MEMBER, "Brisca".into(), Some((4, 1, 1)));
    let unheld = |guid: u64| Selection {
        guid: Some(guid),
        ..Default::default()
    };
    assert_eq!(
        target_subject(&unheld(MEMBER), &roster, &names),
        Subject {
            entity: None,
            occupant: Some(MEMBER),
            unseen: Some(player_temporary_portrait(Some(4), Some(1))),
        }
    );
    assert_eq!(
        target_subject(&unheld(STRANGER), &roster, &names),
        Subject {
            entity: None,
            occupant: Some(STRANGER),
            unseen: None,
        }
    );
    assert_eq!(
        target_subject(&unheld(CREATURE), &roster, &names),
        Subject {
            entity: None,
            occupant: Some(CREATURE),
            unseen: None,
        }
    );
    let object = World::new().spawn_empty().id();
    let held = Selection {
        target: Some(object),
        guid: Some(MEMBER),
        ..Default::default()
    };
    assert_eq!(
        target_subject(&held, &roster, &names),
        Subject {
            entity: Some(object),
            ..Default::default()
        }
    );
}
