//! The app-side **inspect feed** (decision 0631) — the bridge that turns another player's PUBLIC
//! descriptor into the slot views the inspect window's Lua reads.
//!
//! The `CanInspect`/`CheckInteractDistance` distance map used to be fed from here too, back when it
//! held popup tokens only and only for players. It is unit-general now (B304) and lives with the
//! other unit-token feeds — `crate::ui_unit::feed_unit_reach`.
//!
//! This is [`crate::ui_char`]'s pattern turned onto a *foreign* unit, and the difference between
//! the two is the whole reason this module exists rather than a flag on that one:
//!
//! | | the character window | the inspect window |
//! |---|---|---|
//! | source | `PLAYER_FIELD_INV_SLOT_*` guids (PRIVATE) | `PLAYER_VISIBLE_ITEM_*` entries (PUBLIC) |
//! | resolves via | item **objects** → templates | templates only |
//! | so it can show | counts, durability, locks, creator | icon, name, quality |
//!
//! Other players' inventory guids are server-private, so there are no item objects to read — the
//! visible-item entry is the only path (`entities::equipment`'s header records the same finding for
//! rendering their gear). That is exactly why the reference's inspect paper doll shows no stack
//! counts and no durability: the client has none either.
//!
//! Three jobs, each frame, before the VM ticks:
//!
//! - **Drain the intents.** `NotifyInspect(unit)` → resolve the token to a player guid → send
//!   `CMSG_INSPECT` and latch the token as the inspect target. `ClearInspectPlayer()` → drop it.
//!   The window does not wait for `SMSG_INSPECT` (it carries only the echoed guid, and the ref
//!   paints immediately) — the request exists because server-side it also sets our selection.
//! - **Resolve the view.** While a target is latched: re-resolve the **token** each frame (so the
//!   window follows a re-target exactly as the ref's does, `InspectFrame_OnEvent`'s
//!   `PLAYER_TARGET_CHANGED` arm), then read the 19 visible-item entries → the ask-once item
//!   template cache → icon/name/quality/link, and push it on change. A slot whose template answer
//!   is still in flight carries its `item_id` with `icon: None` and fills on a later frame, the
//!   `ui_char` rule.
//! - **Point the booth.** The `"inspect"` body booth ([`InspectBooth`]) gets the resolved entity
//!   and the pane's yaw, so it bakes the inspected player's dressed look (decision 0631 §4).
//!
//! Events, fired on transitions for the **inspected token** (the ref's own registration set,
//! `InspectPaperDollFrame.lua:2-5` + `l.82`): `UNIT_INVENTORY_CHANGED` when any slot view changes,
//! and `UNIT_LEVEL` when the unit's level does. A first resolve counts as a transition (the
//! `ui_unit` rule).

use bevy::prelude::*;

use benilla_ui::script::{InspectView, InvSlotView, InventorySlots, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore};
use crate::portrait::InspectBooth;
use crate::ui_script::UiFeed;

/// Which unit the inspect window is bound to — the ref's `InspectFrame.unit`, mirrored app-side so
/// the feed knows whether to resolve anything. The **token** is the identity (not the guid): the
/// reference re-reads the token on every retarget, so `"target"` follows the selection.
#[derive(Resource, Default)]
pub(crate) struct InspectTarget {
    pub(crate) token: Option<String>,
}

/// The last pushed view + the last level seen, so the feed pushes and fires only on real change
/// (the `ui_char`/`ui_unit` discipline).
///
/// Both are claims about what THIS VM holds, so both sit behind a [`crate::ui_script::VmMemo`]
/// (1290): against a `/reload`'s replacement VM (1291) the memo reads fresh, and the view is
/// re-pushed and its events re-fired for a window the new VM has yet to hear about.
#[derive(Resource, Default)]
struct InspectFeedState {
    vm: crate::ui_script::VmMemo<InspectFeedMemo>,
}

/// The per-VM half of [`InspectFeedState`] — the change bases.
#[derive(Default)]
struct InspectFeedMemo {
    last: Option<InspectView>,
    last_level: Option<u32>,
}

pub(crate) struct InspectUiPlugin;

impl Plugin for InspectUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InspectTarget>()
            .init_resource::<InspectFeedState>()
            .add_systems(Update, feed_inspect.in_set(UiFeed));
    }
}

/// One inspected equipment slot, resolved from the target's PUBLIC visible-item entry. `slot0` is
/// the 0-based `EQUIPMENT_SLOT_*` index; the live-API id the Lua sees is `slot0 + 1`.
///
/// Everything an item *object* would supply is absent by construction (see the module doc), so the
/// count is the ref's own always-1 and durability/flags/locks/creator/equip-fit stay at their inert
/// defaults — an inspected item is not draggable, lockable, or repairable.
///
/// The **enchant lines are the one exception** (decision 0915): a unit's descriptor broadcasts the
/// enchants of what it wears — `PLAYER_VISIBLE_ITEM_<slot>_0 + 1 + j` — so an inspected weapon's
/// enchant is readable without any object. 1.12 fills exactly two of those seven slots (PERM,
/// TEMP: vmangos `SetVisibleItemSlot`'s `MAX_INSPECTED_ENCHANTMENT_SLOT`), and that is the same
/// data the reference's own inspect tooltip has.
fn inspect_slot_view(
    store: &ObjectStore,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    slot0: u8,
) -> Option<InvSlotView> {
    let entry = store.0.player_visible_item_entry(slot0)?;
    // The broadcast roll (`PLAYER_VISIBLE_ITEM_<slot>_PROPERTIES`'s low half — the same `movzx
    // WORD` the reference's inspect leg feeds its `+0x424` with): the NAME's suffix, and only
    // that. Its *lines* would come from the visible enchant slots 2..6, which 1.12 servers leave
    // empty — so an inspected roll names itself and shows no stat lines, on both clients (1547).
    let roll = store.0.player_visible_item_properties(slot0);
    // Template-only ask (`guid` 0 — `Items::template`'s own documented shape): there is no item
    // object to name, and asking by entry is precisely what the real client's ItemCache does.
    let (name, quality, display) = match items.template(entry, 0, commands) {
        Some(t) => (
            Some(rolls.name(&t.name, roll)),
            t.quality,
            t.display_info_id,
        ),
        None => (None, 0, 0),
    };
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link_full(entry, 0, roll, 0, n, quality));
    Some(InvSlotView {
        item_id: entry,
        icon: icons
            .and_then(|i| i.catalog.get(display))
            .and_then(|d| d.icon.clone()),
        count: 1,
        quality: quality as i32,
        name,
        link,
        // `already_bound` stays `false` here, deliberately: `0x5da2c0` reads `ITEM_FIELD_FLAGS`
        // off an item OBJECT, and an inspected player's gear arrives as descriptor fields on
        // THEM — there is no item object and no flags word to read. So an inspect tooltip prints
        // the template's own bind line (Binds when equipped), which is what the reference prints
        // for the same reason. The enchant half below cannot rescue it: those slots name rows,
        // not the instance's bound state.
        already_bound: false,
        // All 7 slots, exactly as the reference's own inspect leg copies and renders them
        // (`0x533354`) — a 1.12 server happens to fill only PERM and TEMP. No item object here, so
        // no charges and no `SMSG_ITEM_ENCHANT_TIME_UPDATE` countdown: the reference's inspect
        // tooltip has neither either.
        enchants: crate::items::enchant_lines(
            (0..7).map(|j| {
                let id = store.0.player_visible_item_enchant(slot0, j).unwrap_or(0);
                (j, id as i32, 0, None)
            }),
            rolls.enchants,
        ),
        ..Default::default()
    })
}

fn feed_inspect(
    script: Option<NonSendMut<UiScript>>,
    mut target: ResMut<InspectTarget>,
    mut feed: ResMut<InspectFeedState>,
    mut booth: ResMut<InspectBooth>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    // `SpellItemEnchantment`'s name column — the inspected item's enchant line (decision 0915) —
    // and `ItemRandomProperties`, the roll behind its "of the Monkey" name (1547).
    catalogs: (
        Option<Res<crate::items::Enchants>>,
        Option<Res<crate::items::RandomProperties>>,
    ),
    commands: Res<NetCommands>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    selection: Res<crate::target::Selection>,
    group: Res<crate::ui_party::GroupState>,
) {
    let Some(mut script) = script else {
        return;
    };
    // Resolved against THIS VM (1290/1291): a `/reload` keeps the latched token, and the fresh
    // memo re-pushes the view into the VM that replaced the one which last saw it.
    let memo = feed.vm.get(&script);

    // NotifyInspect(unit) → CMSG_INSPECT + latch the token. Resolving here (not in Lua) is what
    // lets the window bind to a token while the wire speaks guids.
    for token in script.take_inspect_notifies() {
        match crate::ui_unit::player_token_guid(&token, &selection, &group) {
            Some(guid) => {
                debug!("inspect: {token:?} -> {guid:#x}; sending CMSG_INSPECT");
                let _ = commands.0.send(ClientCommand::Inspect { target: guid });
                target.token = Some(token);
            }
            None => {
                debug!("inspect: {token:?} did not resolve to a player guid — nothing sent");
            }
        }
    }
    // ClearInspectPlayer() — the ref's InspectFrame_OnHide. Drop everything so the resolve below
    // stops and the booth empties.
    if script.take_inspect_clear() {
        target.token = None;
    }

    // Re-resolve the LATCHED TOKEN every frame: `"target"` now means whoever is selected now, which
    // is how the reference's retarget arm works (module doc).
    let resolved = target.token.as_ref().and_then(|token| {
        let guid = crate::ui_unit::player_token_guid(token, &selection, &group)?;
        let entity = *index.0.get(&guid)?;
        Some((token.clone(), guid, entity))
    });

    let Some((token, guid, entity)) = resolved else {
        // Nothing inspected (or the target isn't streamed): clear the view and empty the booth.
        if memo.last.is_some() {
            script.set_inspect(None);
            memo.last = None;
            memo.last_level = None;
        }
        booth.unit = None;
        return;
    };

    booth.unit = Some(entity);
    // The stock `InspectPaperDollFrame.lua` turns the doll by writing the PANE
    // (`InspectModelFrame:SetRotation`), the same way the character sheet, the pet doll and the
    // stable booth already read theirs (decision 1832).
    booth.yaw = script.model_pane_facing("InspectModelFrame");

    let Ok(store) = stores.get(entity) else {
        return;
    };
    let mut slots: InventorySlots = Default::default();
    for slot in 1..=19u8 {
        slots[usize::from(slot)] = inspect_slot_view(
            store,
            &items,
            icons.as_deref(),
            crate::items::RollCatalogs {
                enchants: catalogs.0.as_deref(),
                props: catalogs.1.as_deref(),
            },
            &commands,
            slot - 1,
        );
    }
    let view = InspectView {
        unit: token.clone(),
        guid,
        slots,
    };
    if memo.last.as_ref() != Some(&view) {
        script.set_inspect(Some(view.clone()));
        memo.last = Some(view);
        // The ref's InspectPaperDollItemSlotButton_OnEvent listens for exactly this, filtered on
        // arg1 == the inspected unit.
        script.fire_event(
            "UNIT_INVENTORY_CHANGED",
            vec![ScriptValue::Str(token.clone())],
        );
    }
    let level = store.0.unit_level();
    if memo.last_level != level {
        memo.last_level = level;
        script.fire_event("UNIT_LEVEL", vec![ScriptValue::Str(token)]);
    }
}
