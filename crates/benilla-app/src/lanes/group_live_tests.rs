//! What a listener inside a group packet reads of the group, as that packet left it. The group
//! handlers write the party slots and the raid rows themselves (`0x4e82d0`, `0x4e84a0`, the raid
//! writer `0x4ba5f0`) before they signal (`0x5e6c66`), and the relation functions read those tables
//! at the call (`0x6062b0`-`0x60631c` through `0x4e7f70`), so a listener reads its own list's
//! roster.

use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{member_status, GroupMemberEntry};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, created, frame, values};
use crate::target::relations::fixture::{factions, HUMAN, ORC, PC};

const ME: u64 = 0x7;
/// A player of the other faction, held.
const MATE: u64 = 0x22;
/// `UNIT_FIELD_FACTIONTEMPLATE`.
const TEMPLATE: u16 = 35;

/// `SMSG_GROUP_LIST` for a party of us, leading, and [`MATE`].
fn party_list() -> SessionEvent {
    SessionEvent::GroupList {
        group_type: 0,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: "Brisca".into(),
            guid: MATE,
            status: member_status::ONLINE,
            flags: 0,
        }],
        leader: ME,
        loot: None,
    }
}

/// The relation a `PARTY_MEMBERS_CHANGED` listener reads off a list that seats an enemy-faction
/// player is the party's: the active player toward a party member is friendly (`0x6062b0`), which
/// the list's own party slots answer, not the slots of the frame before.
#[test]
fn a_group_list_listener_reads_the_roster_its_list_wrote() {
    let mut app = client();
    app.insert_resource(factions());
    frame(
        &mut app,
        vec![
            values(ME, &[(TEMPLATE, HUMAN), (FIELD_UNIT_FLAGS, PC)]),
            created(
                MATE,
                EntityKind::Player,
                &[
                    (TEMPLATE, ORC),
                    (FIELD_UNIT_FLAGS, PC),
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                ],
            ),
        ],
    );
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("PARTY_MEMBERS_CHANGED")
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(UnitReaction("player", "party1")))
            end)
            "#,
        )
        .unwrap();
    let seen = frame(&mut app, vec![party_list()]);
    assert!(!seen.is_empty(), "the list signals");
    assert!(
        seen.iter().all(|s| s == "PARTY_MEMBERS_CHANGED:5"),
        "friendly to its new party member at every signal: {seen:?}"
    );
}
