//! A selection whose object the object manager does not hold, on the whole client headless: the
//! guid is the selection (`[0xb4e2d8]`), stored by `SetSelection 0x493540` with or without its
//! object (`0x4935bf` → `0x493605`), and every reader takes what 1.12.1 reads for such a guid.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use benilla_protocol::messages::{member_status, GroupMemberEntry, PartyMemberStatsInfo};
use benilla_protocol::SessionEvent;

use super::front_tests::{fix, Fix, ME, OLD};
use super::{by_name, scan, Selection};
use crate::creature_anim::SwingMessage;
use crate::net::{ClientCommand, Guid, GuidIndex, ObjectStore};
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_party::{GroupState, GROUPTYPE_RAID};

/// Our one party member, `party1`, whom the object manager holds only where a test spawns them.
const MEMBER: u64 = 0x51;
const MEMBER_NAME: &str = "Brisca";

/// [`fix`] with [`MEMBER`] in our party and out of range.
fn grouped() -> Fix {
    let mut f = fix(false);
    f.app.world_mut().resource_mut::<GroupState>().apply_list(
        0,
        0,
        vec![GroupMemberEntry {
            name: MEMBER_NAME.into(),
            guid: MEMBER,
            status: member_status::ONLINE,
            flags: 0,
        }],
        ME,
        None,
        Some(ME),
    );
    f
}

/// `lua` with the host lent, as a key's body or a `/script` line runs it.
fn run(f: &mut Fix, lua: &str) {
    let world = f.app.world_mut();
    if world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .is_none()
    {
        world.insert_non_send_resource(benilla_ui::script::UiScript::new().expect("a VM"));
    }
    crate::script_calls::with_hosted_vm(world, |s| s.expect("a VM").run(lua))
        .expect("the script runs");
}

/// The selection's guid and object.
fn selection(f: &Fix) -> (Option<u64>, Option<Entity>) {
    let s = f.app.world().resource::<Selection>();
    (s.guid, s.target)
}

/// The selection sends on the wire since the last look.
fn picks(f: &Fix) -> Vec<u64> {
    f.wire().1
}

/// Select `guid` with no object, as `SetSelection` stores a groupmate out of range.
fn select_unheld(f: &mut Fix, guid: u64) {
    *f.app.world_mut().resource_mut::<Selection>() = Selection {
        guid: Some(guid),
        ..Default::default()
    };
}

/// [`MEMBER`]'s object streaming in: a player, alive, indexed under the guid.
fn stream_in(f: &mut Fix) -> Entity {
    let world = f.app.world_mut();
    let e = world
        .spawn((
            Guid(MEMBER),
            GlobalTransform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(MEMBER, e);
    e
}

/// An object leaving the object manager, as `net::objects` releases it: out of the index, its
/// model left fading on the entity.
fn stream_out(f: &mut Fix, guid: u64) {
    let world = f.app.world_mut();
    if let Some(e) = world.resource_mut::<GuidIndex>().0.remove(&guid) {
        world.entity_mut(e).remove::<Guid>();
    }
}

/// The tracker's frame: the selection's object as the objects that streamed applied.
fn track(f: &mut Fix) {
    f.app
        .world_mut()
        .run_system_once(super::track::follow_selection_object)
        .expect("the tracker runs");
}

/// One packet through the handler table, then the frame's selection systems: the tracker after the
/// net stage, then the drain of the guid-scoped teardowns (`0x493910(guid, 1)`) the packet raised.
fn deliver(f: &mut Fix, ev: SessionEvent) {
    crate::net::handlers::dispatch(f.app.world_mut(), vec![ev]);
    track(f);
    let world = f.app.world_mut();
    world
        .run_system_once(super::click::clear_target_requests)
        .expect("the drain runs");
    // A one-shot reader starts at the oldest message, as the frame's reader does not.
    world
        .resource_mut::<Messages<super::DeselectGuid>>()
        .clear();
}

/// `SMSG_GROUP_LIST` naming `members` (our subgroup is 0), as the handler table delivers it.
fn list(f: &mut Fix, group_type: u8, members: Vec<GroupMemberEntry>, leader: u64) {
    deliver(
        f,
        SessionEvent::GroupList {
            group_type,
            own_flags: 0,
            members,
            leader,
            loot: None,
        },
    );
}

/// [`MEMBER`]'s row in a list, in subgroup `flags`.
fn row(status: u8, flags: u8) -> GroupMemberEntry {
    GroupMemberEntry {
        name: MEMBER_NAME.into(),
        guid: MEMBER,
        status,
        flags,
    }
}

/// [`MEMBER`]'s pet, which the object manager holds nowhere here.
const PET: u64 = 0xF140_0000_0000_0077;

/// [`stream_in`] with the member's `UNIT_FIELD_SUMMON` naming [`PET`].
fn stream_in_with_pet(f: &mut Fix) -> Entity {
    let e = stream_in(f);
    f.app.world_mut().entity_mut(e).insert(ObjectStore(
        benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x19),
            (8, PET as u32),
            (9, (PET >> 32) as u32),
            (22, 100),
            (28, 100),
        ]),
    ));
    e
}

fn errors(f: &Fix) -> Vec<UiError> {
    f.app.world().resource::<UiErrorKeys>().take()
}

/// `TargetUnit` goes through `CGGameUI::Target` (`0x489a40`), which selects a guid the object
/// manager does not hold when `IsRaidMemberOrPet` names it (`0x489a64`): a party member out of
/// range is selected by the guid alone, and the server is told.
#[test]
fn target_unit_selects_a_party_member_out_of_range_by_guid() {
    let mut f = grouped();
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(selection(&f), (Some(MEMBER), None));
    assert_eq!(picks(&f), vec![MEMBER], "CMSG_SET_SELECTION");
    // An empty slot names nobody: a bare `ret`, never a deselect (`0x489a79`).
    run(&mut f, r#"TargetUnit("party2")"#);
    assert_eq!(selection(&f), (Some(MEMBER), None));
}

/// The call's own chunk reads the new target at once: a party member selected out of range reads as
/// its guid does, the party row's (`0x515970`, `0x496400`), not the old target's snapshot.
#[test]
fn the_chunk_that_selects_a_party_member_out_of_range_reads_them() {
    let mut f = grouped();
    let mut script = benilla_ui::script::UiScript::new().expect("a VM");
    script.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: ME,
        target: OLD,
        party: [MEMBER, 0, 0, 0],
        held: [(ME, 0), (OLD, 0)].into(),
        ..Default::default()
    });
    let unit = |guid: u64, name: &str, has_object: bool| benilla_ui::script::UnitState {
        exists: true,
        has_object,
        guid,
        name: Some(name.into()),
        ..Default::default()
    };
    script.set_unit("target", Some(unit(OLD, "Old", true)));
    script.set_unit("party1", Some(unit(MEMBER, MEMBER_NAME, false)));
    f.app.world_mut().insert_non_send_resource(script);
    run(&mut f, r#"TargetUnit("party1") SEEN = UnitName("target")"#);
    let seen = crate::script_calls::with_hosted_vm(f.app.world_mut(), |s| {
        s.expect("a VM").eval::<Option<String>>("return SEEN")
    })
    .expect("the read");
    assert_eq!(seen.as_deref(), Some(MEMBER_NAME));
}

/// `TargetByName` runs the resolver's party pass (`0x493b22`), which offers a member the object
/// manager does not hold, and hands the pick to `SetSelection` as it is (`0x489daa`).
#[test]
fn target_by_name_selects_a_party_member_out_of_range() {
    let mut f = grouped();
    run(&mut f, r#"TargetByName("Bris")"#);
    assert_eq!(selection(&f), (Some(MEMBER), None));
    assert_eq!(picks(&f), vec![MEMBER]);
    assert!(errors(&f).is_empty(), "no miss line");
}

/// The assist tail selects the basis unit's target through `0x489a40` (`0x489bcb`), so a basis
/// targeting a party member out of range selects that member by guid.
#[test]
fn assist_selects_the_bases_target_out_of_range_by_guid() {
    let mut f = grouped();
    let old = f.app.world().resource::<GuidIndex>().0[&OLD];
    f.make_unit_with(old, &[(16, MEMBER as u32), (17, 0)]);
    run(&mut f, r#"AssistUnit("target")"#);
    assert_eq!(selection(&f), (Some(MEMBER), None));
    assert_eq!(picks(&f), vec![MEMBER]);
}

/// `AssistUnit` shows `ERR_GENERIC_NO_TARGET` when its token names no unit the object manager
/// holds (`0x489c0e`); `AssistByName` shows `ERR_UNIT_NOT_FOUND` for a name whose pick is no held
/// unit (`0x489d17`), a party member out of range included (`0x489c9b`), and
/// `ERR_GENERIC_NO_TARGET` for an empty name with nothing selected (`0x489d2b`). A call's line is
/// signalled inside it; the stock `/assist`, which no call carries, queues the same lines.
#[test]
fn an_assist_that_finds_no_unit_says_so() {
    let mut f = grouped();
    *f.app.world_mut().resource_mut::<Selection>() = Selection::default();
    run(
        &mut f,
        r#"ERR_GENERIC_NO_TARGET = "You have no target."
        ERR_UNIT_NOT_FOUND = "Unknown unit."
        SHOWN = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("UI_ERROR_MESSAGE")
        f:SetScript("OnEvent", function() table.insert(SHOWN, arg1) end)"#,
    );
    let shown = |f: &mut Fix, lua: &str| {
        run(f, &format!("SHOWN = {{}} {lua}"));
        crate::script_calls::with_hosted_vm(f.app.world_mut(), |s| {
            s.expect("a VM")
                .eval::<Vec<String>>("return SHOWN")
                .expect("the lines")
        })
    };
    let none = || vec!["You have no target.".to_string()];
    let unknown = || vec!["Unknown unit.".to_string()];

    assert_eq!(
        shown(&mut f, r#"AssistUnit("target")"#),
        none(),
        "nothing selected"
    );
    assert_eq!(
        shown(&mut f, r#"AssistByName("Nobody")"#),
        unknown(),
        "a name that matches nobody"
    );
    assert_eq!(
        shown(&mut f, r#"AssistByName("Brisca")"#),
        unknown(),
        "a groupmate out of range is no unit"
    );
    assert_eq!(
        shown(&mut f, r#"AssistByName("")"#),
        none(),
        "an empty name assists the selection"
    );

    let assist = |f: &mut Fix, name: Option<&str>| {
        let world = f.app.world_mut();
        world
            .resource_mut::<Messages<by_name::AssistRequest>>()
            .write(by_name::AssistRequest {
                name: name.map(str::to_string),
            });
        world
            .run_system_once(by_name::assist_requests)
            .expect("the /assist reader runs");
        // A one-shot reader starts at the oldest message, as the frame's reader does not.
        world
            .resource_mut::<Messages<by_name::AssistRequest>>()
            .clear();
    };
    assist(&mut f, None);
    assert_eq!(
        errors(&f),
        vec![UiError::key("ERR_GENERIC_NO_TARGET")],
        "a bare /assist"
    );
    assist(&mut f, Some("Nobody"));
    assert_eq!(
        errors(&f),
        vec![UiError::key("ERR_UNIT_NOT_FOUND")],
        "/assist Nobody"
    );
    assert_eq!(selection(&f), (None, None), "no miss moves the selection");
}

/// `ClearTarget` deselects through `0x493540(0, 0)` (`0x48a00e`), whose teardown tests the guid
/// (`0x493937`), so a selection with no object ends and the server is told.
#[test]
fn clear_target_ends_a_selection_with_no_object() {
    let mut f = grouped();
    select_unheld(&mut f, MEMBER);
    run(&mut f, "ClearTarget()");
    assert_eq!(selection(&f), (None, None));
    assert_eq!(picks(&f), vec![0]);
}

/// The struck player's acquire of its attacker tests the selected guid (`0x6259e6`), so a party
/// member selected out of range keeps the selection.
#[test]
fn an_attacker_never_replaces_a_selection_with_no_object() {
    let mut f = grouped();
    select_unheld(&mut f, MEMBER);
    let (me, mob) = (f.app.world().resource::<GuidIndex>().0[&ME], f.mob);
    f.app
        .world_mut()
        .resource_mut::<Messages<SwingMessage>>()
        .write(SwingMessage {
            attacker: mob,
            victim: Some(me),
            hit_info: 0,
            victim_state: 1,
            damage: 1,
            displayed: true,
            seq: 0,
        });
    f.app
        .world_mut()
        .run_system_once(scan::auto_acquire)
        .expect("the acquire runs");
    assert_eq!(selection(&f), (Some(MEMBER), None));
    assert_eq!(picks(&f), Vec::<u64>::new());
}

/// A unit's deactivation runs the teardown on a matching selection (`0x5fbbe5`) unless
/// `IsRaidMemberOrPet` names it (`0x5fbbcd`): a party member streaming out stays selected by its
/// guid, with nothing sent, and a unit off the roster ends the selection with the send.
#[test]
fn a_party_member_streaming_out_stays_selected_by_guid() {
    let mut f = grouped();
    let member = stream_in(&mut f);
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(selection(&f), (Some(MEMBER), Some(member)));
    assert_eq!(picks(&f), vec![MEMBER]);

    stream_out(&mut f, MEMBER);
    track(&mut f);
    assert_eq!(selection(&f), (Some(MEMBER), None), "the guid stays");
    assert_eq!(f.app.world().resource::<Selection>().lit, None);
    assert_eq!(picks(&f), Vec::<u64>::new(), "no teardown send");

    // Control: a unit no roster names ends at its deactivation.
    let mut f = grouped();
    stream_out(&mut f, OLD);
    track(&mut f);
    assert_eq!(selection(&f), (None, None));
    assert_eq!(picks(&f), vec![0]);
}

/// The object a selected guid names again when it streams back in is the selection's object, which
/// the target circle finds by comparing guids each frame (`0x614a96`); `SetSelection` lit only the
/// object of its own select (`0x493701`), so the new one is not lit.
#[test]
fn a_party_member_streaming_back_in_is_the_selections_object() {
    let mut f = grouped();
    select_unheld(&mut f, MEMBER);
    let member = stream_in(&mut f);
    track(&mut f);
    let s = *f.app.world().resource::<Selection>();
    assert_eq!(
        (s.guid, s.target, s.lit),
        (Some(MEMBER), Some(member), None)
    );
    assert_eq!(picks(&f), Vec::<u64>::new());
}

/// A list that no longer seats a party member tears down that member's guid when the object manager
/// does not hold them (`0x5e6d5a` → `0x5e6df7`), with its send.
#[test]
fn a_party_member_leaving_the_group_ends_a_selection_with_no_object() {
    let mut f = grouped();
    select_unheld(&mut f, MEMBER);
    list(&mut f, 0, vec![row(member_status::ONLINE, 0)], ME);
    assert_eq!(selection(&f), (Some(MEMBER), None), "still a member");
    list(&mut f, 0, Vec::new(), 0);
    assert_eq!(selection(&f), (None, None));
    assert_eq!(picks(&f), vec![0]);
}

/// The list re-seats each member of our subgroup through `0x4e82d0`, whose argument is the row's
/// online bit (`0x5e6c40`): a member it lists offline has the selection of their guid torn down
/// (`0x4e8448` → `0x4e845a`), held or not, on every list that names them so; vmangos sends one when
/// a member logs out (`WorldSession.cpp:814` → `Group.cpp:1448`). A member listed online keeps it.
#[test]
fn a_groupmate_listed_offline_ends_the_selection() {
    let mut f = grouped();
    let member = stream_in(&mut f);
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(selection(&f), (Some(MEMBER), Some(member)));
    assert_eq!(picks(&f), vec![MEMBER]);
    list(&mut f, 0, vec![row(member_status::ONLINE, 0)], ME);
    assert_eq!(selection(&f), (Some(MEMBER), Some(member)), "listed online");
    list(&mut f, 0, vec![row(member_status::OFFLINE, 0)], ME);
    assert_eq!(selection(&f), (None, None), "listed offline");
    assert_eq!(picks(&f), vec![0]);

    // By the guid alone, out of range: the next list naming them offline tears it down again.
    stream_out(&mut f, MEMBER);
    select_unheld(&mut f, MEMBER);
    list(&mut f, 0, vec![row(member_status::OFFLINE, 0)], ME);
    assert_eq!(selection(&f), (None, None));
    assert_eq!(picks(&f), vec![0]);
}

/// A departed party member's teardown is their guid alone when the object manager does not hold
/// them (`0x5e6df7`), and their charm, else summon, when it holds them and not that pet
/// (`0x5e6da1` → `0x5e6dc4`). So the pet of a member who leaves out of range stays selected.
#[test]
fn a_departed_members_pet_stays_selected() {
    let mut f = grouped();
    let mut record = PartyMemberStatsInfo::placeholder(true);
    record.pet_guid = Some(PET);
    f.app
        .world_mut()
        .resource_mut::<GroupState>()
        .stats
        .insert(MEMBER, record);
    run(&mut f, r#"TargetUnit("partypet1")"#);
    assert_eq!(selection(&f), (Some(PET), None));
    assert_eq!(picks(&f), vec![PET]);
    list(&mut f, 0, Vec::new(), 0);
    assert_eq!(selection(&f), (Some(PET), None), "the member's guid only");
    assert_eq!(picks(&f), Vec::<u64>::new());

    // A member the object manager holds names a pet it does not: that pet is torn down.
    let mut f = grouped();
    stream_in_with_pet(&mut f);
    run(&mut f, r#"TargetUnit("partypet1")"#);
    assert_eq!(selection(&f), (Some(PET), None));
    let _ = picks(&f);
    list(&mut f, 0, Vec::new(), 0);
    assert_eq!(selection(&f), (None, None));
    assert_eq!(picks(&f), vec![0]);
}

/// A raid rebuild tears down each row it drops by the same rule (`0x4ba9f0`-`0x4baae4`): a member
/// the object manager does not hold by guid (`0x4baad9`).
#[test]
fn a_raid_member_leaving_out_of_range_ends_their_selection() {
    let mut f = grouped();
    // A raid of us and the member, who sits in another subgroup: a row, not a party slot.
    list(
        &mut f,
        GROUPTYPE_RAID,
        vec![row(member_status::ONLINE, 1)],
        ME,
    );
    select_unheld(&mut f, MEMBER);
    list(
        &mut f,
        GROUPTYPE_RAID,
        vec![row(member_status::ONLINE, 1)],
        ME,
    );
    assert_eq!(selection(&f), (Some(MEMBER), None), "still a row");
    let _ = picks(&f);
    const OTHER: u64 = 0x52;
    let other = GroupMemberEntry {
        name: "Other".into(),
        guid: OTHER,
        status: member_status::ONLINE,
        flags: 1,
    };
    list(&mut f, GROUPTYPE_RAID, vec![other], ME);
    assert_eq!(selection(&f), (None, None));
    assert_eq!(picks(&f), vec![0]);
}

/// The kick, the disband and the leave acknowledgement empty the party slots through
/// `0x4e84a0(1)` (`0x5e6855`, `0x5e6898`, `0x5e68fb`), which tears down each member the object
/// manager does not hold (`0x4e856f` → `0x4e8587`).
#[test]
fn an_ending_ends_the_selection_of_a_member_out_of_range() {
    use benilla_protocol::messages::{party_operation, party_result};
    let endings = [
        SessionEvent::GroupUninvited,
        SessionEvent::GroupDestroyed,
        SessionEvent::PartyCommandResult {
            operation: party_operation::LEAVE,
            member: String::new(),
            result: party_result::OK,
        },
    ];
    for ending in endings {
        let mut f = grouped();
        select_unheld(&mut f, MEMBER);
        let what = format!("{ending:?}");
        deliver(&mut f, ending);
        assert_eq!(selection(&f), (None, None), "{what}");
        assert_eq!(picks(&f), vec![0], "{what}");
    }
}

/// `SetSelection` plays `igCharacterSelect` when the guid it stores names no object (`0x4936f7 je
/// 0x493843`, `0x843328` through `0x458030`); one that finds its object lights it (`0x493701`) and
/// plays nothing, and the setter's dedup returns before either (`0x493617`).
#[test]
fn selecting_a_guid_with_no_object_plays_the_select_sound() {
    let cues = |f: &Fix| {
        f.app
            .world()
            .resource::<crate::sound::MessageSounds>()
            .queued_cues()
    };
    let mut f = grouped();
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(cues(&f), vec!["igCharacterSelect"]);
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(cues(&f), vec!["igCharacterSelect"], "the dedup");

    let mut f = grouped();
    stream_in(&mut f);
    run(&mut f, r#"TargetUnit("party1")"#);
    assert_eq!(selection(&f).0, Some(MEMBER));
    assert!(cues(&f).is_empty(), "a held member");
}

/// `TargetLastEnemy`'s memory is written inside `SetSelection` (`0x49377d`), past the gates on the
/// object it selected: a unit that streams in under the selected guid is the selection's object but
/// was never selected, so it is not remembered. The object a select lit is.
#[test]
fn a_unit_streaming_in_under_the_selection_is_not_the_last_enemy() {
    const FOE: u64 = 0xF130_0000_0000_0042;
    let mut f = fix(false);
    select_unheld(&mut f, FOE);
    let world = f.app.world_mut();
    let foe = world
        .spawn((
            Guid(FOE),
            GlobalTransform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(FOE, foe);
    track(&mut f);
    assert_eq!(selection(&f), (Some(FOE), Some(foe)));
    let remember = |f: &mut Fix| {
        f.app
            .world_mut()
            .run_system_once(scan::remember_last_enemy)
            .expect("the stamp runs");
        f.app.world().resource::<scan::LastEnemy>().0
    };
    assert_eq!(remember(&mut f), None);

    *f.app.world_mut().resource_mut::<Selection>() = Selection {
        target: Some(foe),
        guid: Some(FOE),
        lit: Some(foe),
        ..Default::default()
    };
    assert_eq!(remember(&mut f), Some(FOE), "the object a select lit");
}

/// `ArmCast` looks the selected guid up (`0x468460` at `0x6e53bc`) and `BindTarget` binds no null
/// object (`0x6e5b5e`): a unit spell with a party member selected out of range binds nothing and
/// waits at the cursor, sending no cast.
#[test]
fn a_cast_never_binds_a_selection_with_no_object() {
    const BLESSING: u32 = 19740;
    let mut f = grouped();
    select_unheld(&mut f, MEMBER);
    let mut spells = crate::ui_action::Spells::empty_for_tests();
    spells.catalog = benilla_formats::SpellCatalog::from_displays(
        [(
            BLESSING,
            benilla_formats::SpellDisplay {
                targets: 0x2,
                range_index: 5,
                ..Default::default()
            },
        )]
        .into(),
    );
    spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(
        [(
            5,
            benilla_formats::SpellRange {
                min: 0.0,
                max: 30.0,
                flags: 0,
            },
        )]
        .into(),
    );
    f.app.world_mut().insert_resource(spells);
    f.app
        .world_mut()
        .run_system_once(
            |targeting: crate::spell::cast_target::CastTargeting,
             mut ladder: crate::spell::CastLadder| {
                ladder.send(
                    BLESSING,
                    &targeting.context(),
                    crate::spell::CastCommit::Spell,
                );
            },
        )
        .expect("the ladder runs");
    let sent: Vec<_> = f.rx.try_iter().collect();
    assert!(
        !sent.iter().any(
            |c| matches!(c, ClientCommand::CastSpell { spell_id, .. } if *spell_id == BLESSING)
        ),
        "no cast: {sent:?}"
    );
    assert!(f.cursor_is_up(), "the word waits at the cursor");
}

/// With `autoSelfCast` off, `ArmCast` sends at once a spell whose maximum range is above 100
/// (`0x6e5447`, `0x806b10`) when its candidate is a guid the object manager does not hold
/// (`0x6e5425`): the guid is parked in the pending block (`0xceac60`), whose mask holds no unit
/// bit, so the packet names no target (`0x6e5454`-`0x6e5467`). Any standing word reaches that leg:
/// Mind Vision's second rank (50000 yards, a unit word) and Eagle Eye's location word go out so;
/// the first rank, at 100 yards, waits at the cursor.
#[test]
fn a_far_spell_at_a_selection_with_no_object_sends_with_no_target() {
    const MIND_VISION_1: u32 = 2096;
    const MIND_VISION_2: u32 = 10909;
    const EAGLE_EYE: u32 = 6197;
    let display = |targets: u32, implicit: u32, range_index: u32| benilla_formats::SpellDisplay {
        targets,
        implicit_target_a1: implicit,
        range_index,
        ..Default::default()
    };
    let rows = [(6, 100.0), (13, 50_000.0)].map(|(id, max)| {
        (
            id,
            benilla_formats::SpellRange {
                min: 0.0,
                max,
                flags: 0,
            },
        )
    });
    let cast = |spell: u32| {
        let mut f = grouped();
        select_unheld(&mut f, MEMBER);
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (MIND_VISION_1, display(0, 25, 6)),
                (MIND_VISION_2, display(0, 25, 13)),
                (EAGLE_EYE, display(0x40, 0, 13)),
            ]
            .into(),
        );
        spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(rows.into());
        f.app.world_mut().insert_resource(spells);
        f.app
            .world_mut()
            .run_system_once(
                move |targeting: crate::spell::cast_target::CastTargeting,
                      mut ladder: crate::spell::CastLadder| {
                    ladder.send(spell, &targeting.context(), crate::spell::CastCommit::Spell);
                },
            )
            .expect("the ladder runs");
        let sent: Vec<_> = f
            .rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, target } if spell_id == spell => Some(target),
                _ => None,
            })
            .collect();
        (sent, f.cursor_is_up())
    };
    assert_eq!(cast(MIND_VISION_2), (vec![None], false), "Mind Vision 2");
    assert_eq!(cast(EAGLE_EYE), (vec![None], false), "Eagle Eye");
    assert_eq!(cast(MIND_VISION_1), (vec![], true), "Mind Vision 1");
}
