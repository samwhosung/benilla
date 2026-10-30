//! An item's deactivate through the whole client: the out-of-range list's release runs the item's
//! vtable slot 1 (`0x464920` → `[vtable+0x4]` at `0x46496f`, `CGItem_C::Deactivate 0x5d8660`),
//! whose head lets go of what still names the item inside the release's packet: the loot window
//! looting it (`0x5d869b`), the pending record's cast from it (`0x5d86c4`), the cursor holding it
//! (`0x5d86e9`) and the reader on it (`0x5d8713`). A line in the next packet reads what each left.

use benilla_protocol::messages::loot_type;
use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;

use super::lock_live_tests::{client_with, spare, system, Client, SPARE};
use crate::net::ClientCommand;

/// The spy: each signal the legs raise, and at each system line whether the cursor holds an item
/// (`CursorHasItem`).
const SPY: &str = r#"Seen = {}
    local f = CreateFrame("Frame")
    for _, e in ipairs({"CHAT_MSG_SYSTEM", "ITEM_LOCK_CHANGED", "CURSOR_UPDATE", "LOOT_OPENED",
            "LOOT_CLOSED", "ITEM_TEXT_CLOSED", "SPELLCAST_FAILED", "SPELLCAST_STOP"}) do
        f:RegisterEvent(e)
    end
    f:SetScript("OnEvent", function()
        if event == "CHAT_MSG_SYSTEM" then
            table.insert(Seen, arg1 .. ":" .. tostring(CursorHasItem()))
        else
            table.insert(Seen, event)
        end
    end)"#;

/// A use spell, for the pending record's cast.
const SPELL: u32 = 7_001;

/// The release of [`SPARE`], then a line, in one frame.
fn release_then_line(c: &mut Client) {
    c.frame(vec![
        vec![SessionEvent::ObjectsRemoved(vec![SPARE.0])],
        vec![system("after")],
    ]);
}

/// The cursor leg (`0x5d86c9`-`0x5d86e9`): a held item released lets the cursor go as
/// `ClearCursor(1, 1)` does, its unlock first (`0x4951d9`), then `CURSOR_UPDATE`, inside the
/// release's packet; the item is unlocked.
#[test]
fn a_released_item_on_the_cursor_is_let_go_and_unlocked() {
    let mut c = client_with(SPY);
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM")
            .run("PickupContainerItem(0, 1)")
            .expect("the pickup runs")
    });
    {
        let script = c.app.world().non_send_resource::<UiScript>();
        assert_eq!(script.cursor_item().map(|i| i.guid), Some(SPARE.0), "held");
        assert!(script.item_locked(SPARE.0), "the pickup's lock");
    }
    c.seen();
    release_then_line(&mut c);
    assert_eq!(
        c.seen(),
        ["ITEM_LOCK_CHANGED", "CURSOR_UPDATE", "after:nil"]
    );
    let script = c.app.world().non_send_resource::<UiScript>();
    assert!(!script.item_locked(SPARE.0), "unlocked");
}

/// The loot leg (`0x5d867f`-`0x5d869b`): a window looting the released item closes as
/// `0x48f200(1, 0, 0)` does, with its `CMSG_LOOT_RELEASE`, an item source's two unlocks
/// (`0x48f2bd`, `0x4c1d36`) and `LOOT_CLOSED` (`0x4c1dac`), inside the release's packet.
#[test]
fn a_released_item_closes_the_window_looting_it() {
    let mut c = client_with(SPY);
    // `OpenItem` arms the latch with the item it opens (`0x5edcc0`), which admits its response.
    c.app
        .world_mut()
        .resource_mut::<crate::ui_loot::LootLatch>()
        .0 = Some(SPARE.0);
    c.frame(vec![vec![SessionEvent::LootResponse {
        guid: SPARE.0,
        loot_type: loot_type::CORPSE,
        gold: 5,
        items: Vec::new(),
    }]]);
    assert_eq!(
        c.app
            .world()
            .resource::<crate::ui_loot::LootState>()
            .source(),
        Some(SPARE.0),
        "the window loots the item"
    );
    c.seen();
    c.sent.try_iter().for_each(drop);
    release_then_line(&mut c);
    assert_eq!(
        c.seen(),
        [
            "ITEM_LOCK_CHANGED",
            "ITEM_LOCK_CHANGED",
            "LOOT_CLOSED",
            "after:nil"
        ]
    );
    let sent: Vec<ClientCommand> = c.sent.try_iter().collect();
    assert!(
        sent.iter()
            .any(|s| matches!(s, ClientCommand::LootRelease { guid } if *guid == SPARE.0)),
        "the release goes out: {sent:?}"
    );
    assert_eq!(
        c.app
            .world()
            .resource::<crate::ui_loot::LootState>()
            .source(),
        None
    );
}

/// The reader leg (`0x5d86ee`-`0x5d8713`): a reader open on the released item closes as
/// `0x4e32e0` of a zero guid does: its latch release re-enters the core, which signals first
/// (`0x493310`), then the core's own `ITEM_TEXT_CLOSED`, inside the release's packet.
#[test]
fn a_released_item_closes_the_reader_on_it() {
    let mut c = client_with(SPY);
    let world = c.app.world_mut();
    world
        .resource_mut::<crate::ui_item_text::ItemTextOpen>()
        .open_letter(SPARE.0, 1);
    // The open's tail registers the reader on the latch (`0x4e349a`).
    crate::ui_session::register(world, SPARE.0);
    c.seen();
    release_then_line(&mut c);
    assert_eq!(
        c.seen(),
        ["ITEM_TEXT_CLOSED", "ITEM_TEXT_CLOSED", "after:nil"]
    );
    let world = c.app.world();
    assert_eq!(
        world
            .resource::<crate::ui_item_text::ItemTextOpen>()
            .object(),
        None
    );
    assert_eq!(
        world.resource::<crate::ui_session::InteractLatch>().guid(),
        None
    );
}

/// The cast leg (`0x5d86a0`-`0x5d86c4`): a cast whose pending record's source is the released item
/// is cancelled as `CancelCast(1, 0, 0x1c)` does: the slot pops and `SPELLCAST_FAILED` fires for
/// our record, with no `CMSG_CANCEL_CAST`, inside the release's packet.
#[test]
fn a_released_item_cancels_the_cast_from_it() {
    let mut c = client_with(SPY);
    {
        let mut pending = c
            .app
            .world_mut()
            .resource_mut::<crate::spell::PendingCast>();
        pending.push_cast(
            SPELL,
            Some(benilla_protocol::messages::ITEM_FLAG_PLAYERCAST),
            Some(SPARE.0),
        );
        pending.fill_record();
        pending.fill_record_spell(SPELL);
        pending.fill_record_item(Some(SPARE.0));
    }
    release_then_line(&mut c);
    assert_eq!(c.seen(), ["SPELLCAST_FAILED", "after:nil"]);
    assert_eq!(
        c.app
            .world()
            .resource::<crate::spell::PendingCast>()
            .current(),
        None,
        "the slot popped"
    );
    let sent: Vec<ClientCommand> = c.sent.try_iter().collect();
    assert!(
        !sent
            .iter()
            .any(|s| matches!(s, ClientCommand::CancelCast { .. })),
        "nothing sent: {sent:?}"
    );
}

/// A disenchant window with a row left on the released item (`0x48f245`-`0x48f264`): the close
/// sweeps it inside the deactivate, the item still live, so the sweep's end finds contents
/// (`0x4c2228` → `0x4c2a8c`) and signals `LOOT_OPENED` (`0x4c2240`), the window kept, with no
/// release and no `LOOT_CLOSED`.
#[test]
fn a_released_disenchanted_item_sweeps_its_window_and_keeps_it() {
    const ESSENCE: u32 = 10_938;
    let mut c = client_with(SPY);
    c.app
        .world_mut()
        .resource_mut::<crate::ui_loot::LootLatch>()
        .0 = Some(SPARE.0);
    c.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(ESSENCE, Some(crate::items::test_template("Essence")));
    c.frame(vec![vec![SessionEvent::LootResponse {
        guid: SPARE.0,
        loot_type: loot_type::DISENCHANTING,
        gold: 0,
        items: vec![benilla_protocol::messages::LootItem {
            slot: 0,
            item_id: ESSENCE,
            count: 1,
            display_info_id: 0,
            random_property_id: 0,
            slot_type: benilla_protocol::messages::slot_type::ALLOW_LOOT,
        }],
    }]]);
    c.seen();
    c.sent.try_iter().for_each(drop);
    release_then_line(&mut c);
    assert_eq!(c.seen(), ["LOOT_OPENED", "after:nil"]);
    let sent: Vec<ClientCommand> = c.sent.try_iter().collect();
    assert!(
        matches!(sent[..], [ClientCommand::AutostoreLootItem { slot: 0 }]),
        "the row's take alone: {sent:?}"
    );
    assert_eq!(
        c.app
            .world()
            .resource::<crate::ui_loot::LootState>()
            .source(),
        Some(SPARE.0),
        "the window stays"
    );
}

/// A destroy for a guid neither live nor released releases nothing (`0x464931`, `0x46493a`): a
/// reader on it stays open.
#[test]
fn a_destroy_of_an_unknown_guid_runs_no_deactivate() {
    const GHOST: u64 = (benilla_protocol::guid::HIGH_ITEM as u64) << 48 | 0x99;
    let mut c = client_with(SPY);
    c.app
        .world_mut()
        .resource_mut::<crate::ui_item_text::ItemTextOpen>()
        .open_letter(GHOST, 1);
    c.frame(vec![
        vec![SessionEvent::ObjectDestroyed(GHOST)],
        vec![system("after")],
    ]);
    assert_eq!(c.seen(), ["after:nil"]);
    assert_eq!(
        c.app
            .world()
            .resource::<crate::ui_item_text::ItemTextOpen>()
            .object(),
        Some(GHOST)
    );
}

/// The record set-up's spy: each `BAG_UPDATE` with its bag (the set-up's `0x5d85e8` for our
/// item in the backpack), and each system line.
const SET_UP_SPY: &str = r#"Seen = {}
    local f = CreateFrame("Frame")
    f:RegisterEvent("BAG_UPDATE")
    f:RegisterEvent("CHAT_MSG_SYSTEM")
    f:SetScript("OnEvent", function()
        if event == "BAG_UPDATE" then
            table.insert(Seen, "BAG_UPDATE " .. arg1)
        else
            table.insert(Seen, arg1)
        end
    end)"#;

/// [`SPARE`]'s record, answered.
fn spare_record() -> SessionEvent {
    SessionEvent::ItemTemplate {
        entry: SPARE.1,
        info: Some(Box::new(crate::items::test_template("Spare"))),
    }
}

/// The record answered after a destroy and a new object under the guid, both before it: the
/// destroy's deactivate dropped the first object's set-up callback (`0x55c0c0` at `0x5d8733`), so
/// the answer sets up the new object once.
#[test]
fn a_destroy_before_the_record_leaves_the_next_object_one_set_up() {
    let mut c = client_with(SET_UP_SPY);
    c.frame(vec![
        vec![SessionEvent::ObjectDestroyed(SPARE.0)],
        vec![spare()],
    ]);
    c.seen();
    c.frame(vec![vec![spare_record()], vec![system("after")]]);
    assert_eq!(c.seen(), ["BAG_UPDATE 0", "after"]);
}

/// The record answered after a release and the item's bring-back: the release dropped its
/// set-up callback, and the bring-back's slot 3 asked again (`0x5d8430` → `0x5d8300`), so the
/// answer sets it up once.
#[test]
fn a_release_before_the_record_and_a_bring_back_set_up_once() {
    let mut c = client_with(SET_UP_SPY);
    c.frame(vec![
        vec![SessionEvent::ObjectsRemoved(vec![SPARE.0])],
        vec![spare()],
    ]);
    c.seen();
    c.frame(vec![vec![spare_record()], vec![system("after")]]);
    assert_eq!(c.seen(), ["BAG_UPDATE 0", "after"]);
}

/// A released item whose record landed while it was away sets up again as it comes back: its
/// slot 3 finds the record cached (`0x5d8430` → `0x5d8300` → `0x5d8440`), and the set-up's
/// `BAG_UPDATE` fires inside the bring-back's packet.
#[test]
fn a_released_item_brought_back_sets_up_off_its_cached_record() {
    let mut c = client_with(SET_UP_SPY);
    c.frame(vec![vec![SessionEvent::ObjectsRemoved(vec![SPARE.0])]]);
    c.frame(vec![vec![spare_record()]]);
    c.seen();
    c.frame(vec![vec![spare()], vec![system("after")]]);
    assert_eq!(c.seen(), ["BAG_UPDATE 0", "after"]);
}
