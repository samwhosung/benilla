//! The item uses' engine half (`UseContainerItem`, `UseInventoryItem`, `ConfirmBindOnUse`) and
//! the senders the item calls share ([`send_container_move`], [`send_auto_equip`]), all applied
//! inside their verbs' calls. The verbs lock what they touch in the call, as the reference's do;
//! these only send.

use bevy::prelude::*;

use benilla_protocol::messages::BAG_PLAYER_INVENTORY;
use benilla_ui::script::{ContainerUse, UiScript, UseArm, UseFork, EQUIPMENT_BAG};

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};

use super::{slot_guid, wire_pos};
use crate::query_cache::Ask;

/// The one auto-equip sender (`AutoEquipCursorItem`, `0x5e1480`, past its question): a projectile,
/// as the VM decided by its cached record (`0x5e16ff`, `0x5e1704`), loads by its `ammo` entry
/// with `CMSG_SET_AMMO` and stays in its bag; anything else is `CMSG_AUTOEQUIP_ITEM`. The question
/// and the clear are the VM's too ([`benilla_ui::script::AutoEquip`]).
pub(crate) fn send_auto_equip(commands: &NetCommands, bag_index: u8, slot: u8, ammo: Option<u32>) {
    if let Some(entry) = ammo {
        debug!("ui_items: set ammo entry {entry} (wire {bag_index}/{slot})");
        let _ = commands.0.send(ClientCommand::SetAmmo { entry });
    } else {
        debug!("ui_items: autoequip wire {bag_index}/{slot}");
        let _ = commands
            .0
            .send(ClientCommand::AutoEquipItem { bag_index, slot });
    }
}

/// What a `UseContainerItem`, `UseInventoryItem` or `ConfirmBindOnUse` call reads and writes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScriptItemUse<'w, 's> {
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    merchant: Res<'w, crate::ui_merchant::MerchantOpen>,
    item_text: ResMut<'w, crate::ui_item_text::ItemTextOpen>,
    targeting: crate::spell::cast_target::CastTargeting<'w, 's>,
    // The item use's casts and sends.
    ladder: crate::spell::CastLadder<'w, 's>,
    ui_errors: Res<'w, crate::ui_action::UiErrorKeys>,
    gate: crate::ui_bind_confirm::BindGate<'w>,
    /// What the use's requirement gate judges the item by, seated for it with the host out.
    requirements: super::RequirementsLive<'w, 's>,
}

impl ScriptItemUse<'_, '_> {
    /// `UseInventoryItem(id, onSelf)`, a doll right-click, sent through [`super::send_item_use`],
    /// as the reference's doll click reaches the same `CGItem::Use` a bag click does
    /// (`0x4c7af0`): a worn quest-starter offers its quest. `on_self` binds the player in place of
    /// the selection (`0x4c8e48`). The call refused a locked item. Ids outside 1..=19 are refused.
    pub(crate) fn use_inventory_item(&mut self, script: &mut UiScript, id: u32, on_self: bool) {
        // The use's requirement gate (`0x5d8e0e`) reads the player as the apply finds it.
        self.requirements.seat(script);
        let Self {
            self_q,
            targeting,
            ladder,
            ui_errors,
            gate,
            ..
        } = self;
        if !(1..=19).contains(&id) {
            debug!("ui_items: UseInventoryItem({id}) out of range — ignored");
            return;
        }
        let slot = (id - 1) as u8;
        let (guid, start_quest, spell_index, use_spell, entry, is_charter) = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, EQUIPMENT_BAG, slot, &ladder.objects))
            .and_then(|guid| {
                let entry = ladder.objects.object(guid)?.object_entry()?;
                // The use only reads (`0x5d8d57`).
                let t = ladder.items.template(entry, guid, Ask::Read)?;
                Some((
                    Some(guid),
                    t.start_quest,
                    t.use_spell_index().unwrap_or(0),
                    t.use_spell.map(|u| u.spell_id),
                    Some(entry),
                    t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
                ))
            })
            .unwrap_or((None, 0, 0, None, None, false));
        debug!("ui_items: use equipped item, lua slot {id} (wire 255/{slot})");
        super::send_item_use(
            super::ItemUse {
                guid,
                start_quest,
                bag_index: BAG_PLAYER_INVENTORY,
                slot,
                entry: entry.unwrap_or(0),
                spell_index,
                use_spell,
                on_object: None,
                is_charter,
            },
            &targeting.context().self_bound(on_self),
            ladder,
            script,
            gate,
            false,
            ui_errors,
        );
    }

    /// `UseContainerItem(bag, slot)` outside repair mode: the arm the call took, sent. The call
    /// chose it and took its lock ([`benilla_ui::script::UseArm`]), so this only sends; an item
    /// that opens is handed back for the loot window's `OpenItem` ([`crate::ui_loot::OpenItem`]).
    pub(crate) fn use_container_item(
        &mut self,
        script: &mut UiScript,
        call: ContainerUse,
    ) -> Option<crate::ui_loot::OpenItem> {
        // The use's requirement gate (`0x5d8e0e`) reads the player as the apply finds it.
        self.requirements.seat(script);
        let Self {
            self_q,
            merchant,
            item_text,
            targeting,
            ladder,
            ui_errors,
            gate,
            ..
        } = self;
        let ContainerUse {
            bag,
            slot,
            guid: item_guid,
            arm,
            on_self,
            ..
        } = call;
        let slot0 = u8::try_from(slot.saturating_sub(1)).ok();
        // The slot must still hold the item the call resolved (`0x4fa1a6`), else nothing goes out.
        // The call applies inside its verb, the bank and sale arms ahead of their lock, so no
        // move lands between and there is no lock to let go.
        let still_there = slot0.is_some_and(|s0| {
            self_q
                .iter()
                .next()
                .and_then(|store| slot_guid(&store.0, bag, s0, &ladder.objects))
                == Some(item_guid)
        }) && ladder.objects.object(item_guid).is_some();
        let pos = wire_pos(bag, slot);
        let (Some((bag_index, wire_slot)), true) = (pos, still_there) else {
            debug!("ui_items: UseContainerItem({bag}, {slot}) no longer holds {item_guid:#x}");
            return None;
        };
        let fork = match arm {
            UseArm::Withdraw => {
                debug!("ui_items: withdraw (lua bag {bag} → wire {bag_index}/{wire_slot})");
                let _ = ladder.commands.0.send(ClientCommand::AutoStoreBankItem {
                    bag: bag_index,
                    slot: wire_slot,
                });
                return None;
            }
            UseArm::Deposit => {
                debug!("ui_items: deposit (lua bag {bag} → wire {bag_index}/{wire_slot})");
                let _ = ladder.commands.0.send(ClientCommand::AutoBankItem {
                    bag: bag_index,
                    slot: wire_slot,
                });
                return None;
            }
            // The whole stack sells (`CMSG_SELL_ITEM`, count 0).
            UseArm::Sell => {
                let Some(vendor) = merchant.vendor else {
                    debug!("ui_items: sale of {item_guid:#x} with no vendor open — not sent");
                    return None;
                };
                debug!("ui_items: sell lua bag {bag} slot {slot} (item {item_guid:#x})");
                let _ = ladder.commands.0.send(ClientCommand::SellItem {
                    vendor,
                    item_guid,
                    count: 0,
                });
                return None;
            }
            UseArm::Item(fork) => fork,
        };
        // `None` while the template is in flight, which the call read as a plain use.
        let clicked = ladder.objects.object(item_guid).and_then(|obj| {
            let entry = obj.object_entry()?;
            // The click only reads (`0x4fa3aa`).
            let t = ladder.items.template(entry, item_guid, Ask::Read)?;
            Some(Clicked {
                guid: item_guid,
                entry,
                item_text_id: obj.item_text_id().unwrap_or(0),
                start_quest: t.start_quest,
                spell_index: t.use_spell_index().unwrap_or(0),
                use_spell: t.use_spell.map(|u| u.spell_id),
                page_text: t.page_text,
                is_charter: t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
            })
        });
        match (fork, clicked) {
            // `CMSG_OPEN_ITEM` (emitter `0x5edd60`); vmangos swaps the entry back from
            // `character_gifts`.
            (UseFork::Unwrap, Some(c)) => {
                debug!(
                    "ui_items: unwrap gift {:#x} (lua bag {bag} → wire {bag_index}/{wire_slot})",
                    c.guid
                );
                let _ = ladder.commands.0.send(ClientCommand::OpenItem {
                    bag_index,
                    slot: wire_slot,
                });
            }
            // The call armed the wrap; a left-click on a container slot sends `CMSG_WRAP_ITEM`.
            (UseFork::WrapPaper, _) => {}
            (UseFork::Quest, Some(c)) => {
                debug!(
                    "ui_items: quest-starter {:#x} offers quest {}",
                    c.guid, c.start_quest
                );
                super::send_item_use(
                    super::ItemUse {
                        guid: Some(c.guid),
                        start_quest: c.start_quest,
                        bag_index,
                        slot: wire_slot,
                        entry: c.entry,
                        spell_index: c.spell_index,
                        use_spell: c.use_spell,
                        on_object: None,
                        is_charter: c.is_charter,
                    },
                    &targeting.context().self_bound(on_self),
                    ladder,
                    script,
                    gate,
                    false,
                    ui_errors,
                );
            }
            // The reader (`0x4e32e0`), whose open sends `CMSG_READ_ITEM` for a book not yet read;
            // a re-click closes it.
            (UseFork::Read, Some(c)) => {
                if item_text.read_pages(c.guid) {
                    debug!("ui_items: re-click closes the book {:#x}", c.guid);
                } else {
                    debug!(
                        "ui_items: read book {:#x} (page {}, lua bag {bag} slot {slot})",
                        c.guid, c.page_text
                    );
                }
            }
            // vmangos's `CMSG_READ_ITEM` wants a template `PageText`, so a letter's text comes by
            // item-text query.
            (UseFork::Letter, Some(c)) => {
                if item_text.read_letter(c.guid, c.item_text_id) {
                    debug!("ui_items: re-click closes the letter {:#x}", c.guid);
                } else {
                    debug!(
                        "ui_items: read letter {:#x} (text {}, lua bag {bag} slot {slot})",
                        c.guid, c.item_text_id
                    );
                }
            }
            // `OpenItem 0x5edc80`, whose auto-loot byte, loot target and send are the loot
            // window's. The server answers with a loot window on the item
            // (`SpellHandler.cpp:227`). The send tests neither the lock id nor the unlocked bit,
            // so a locked box sends and the server's `EQUIP_ERR_ITEM_LOCKED` is the "Item is
            // locked" line. No kneel (`0x612710`).
            (UseFork::Open, Some(c)) => {
                debug!(
                    "ui_items: open item {:#x} (lua bag {bag} → wire {bag_index}/{wire_slot})",
                    c.guid
                );
                return Some(crate::ui_loot::OpenItem {
                    guid: c.guid,
                    bag_index,
                    slot: wire_slot,
                });
            }
            // A plain use (food, potions, the hearthstone), or a fork whose template left the
            // cache since the call read it.
            (fork, clicked) => {
                if fork != UseFork::Plain {
                    debug!("ui_items: {fork:?} on {item_guid:#x} with no template — not sent");
                    return None;
                }
                debug!("ui_items: use item (lua bag {bag} → wire {bag_index}/{wire_slot})");
                super::send_item_use(
                    super::ItemUse {
                        guid: Some(item_guid),
                        start_quest: 0,
                        bag_index,
                        slot: wire_slot,
                        entry: clicked.map_or(0, |c| c.entry),
                        spell_index: clicked.map_or(0, |c| c.spell_index),
                        use_spell: clicked.and_then(|c| c.use_spell),
                        on_object: None,
                        is_charter: clicked.is_some_and(|c| c.is_charter),
                    },
                    &targeting.context().self_bound(on_self),
                    ladder,
                    script,
                    gate,
                    false,
                    ui_errors,
                );
            }
        }
        None
    }

    /// `ConfirmBindOnUse()` (`0x48d770`): the pending use re-issued through the cast ladder with
    /// `suppress` set (`0x5d92c1`). The pending state is one cell, taken by the first accept
    /// whatever the protected-action gate said (`0x5d92c6`-`0x5d92cc`), so a doubled accept does
    /// nothing; the cast inside it asks tag 2 (`0x6e4bc1`).
    pub(crate) fn confirm_bind_on_use(&mut self, script: &mut UiScript, may_cast: bool) {
        // The use's requirement gate (`0x5d8e0e`) reads the player as the apply finds it.
        self.requirements.seat(script);
        let Self {
            targeting,
            ladder,
            ui_errors,
            gate,
            ..
        } = self;
        let Some(it) = gate.on_use.0.take() else {
            return;
        };
        if super::item_use_refused(script, it.guid) {
            debug!("ui_items: bind-on-use confirmed on a locked item — refused");
            return;
        }
        debug!(
            "ui_items: bind-on-use confirmed for wire {}/{}",
            it.bag_index, it.slot
        );
        let outer = ladder.permit(may_cast);
        super::send_item_use(
            it,
            &targeting.context(),
            ladder,
            script,
            gate,
            true,
            ui_errors,
        );
        ladder.permit(outer);
    }
}

/// The clicked item's send data, read as the call applies.
#[derive(Clone, Copy)]
struct Clicked {
    guid: u64,
    entry: u32,
    /// Instance `ITEM_FIELD_ITEM_TEXT_ID`, nonzero on a letter made from mail.
    item_text_id: u32,
    start_quest: u32,
    spell_index: u8,
    /// The template's on-use spell, the key of the cast tail's in-flight guard.
    use_spell: Option<u32>,
    /// The template's `PageText`, nonzero on a book.
    page_text: u32,
    is_charter: bool,
}

/// The arm a right-click on this item takes past the bank and vendor forks, for the feed to push
/// with the slot: `UseContainerItem`'s equip fork (`0x4fa3b9`), an equippable template whose
/// `StartQuest` (`[rec+0x1a8]`) is 0 (`0x4fa3bd`), else the use dispatcher `0x5d8d00`'s arms in
/// its order: a wrapped gift unwraps (`0x5d8d92`), wrapping paper arms (`0x5d8d9d`), a
/// quest-starter offers its quest (`0x5d8dd2`), a `PageText` book reads (`0x5d8e4c`) ahead of the
/// open arm, the inverse of the tooltip, a mail letter reads, and a lootable template opens
/// (`0x5d8f7c`).
pub(crate) fn use_fork(
    t: &benilla_protocol::ItemInfo,
    inst_flags: u32,
    item_text_id: u32,
) -> UseFork {
    if t.inventory_type != 0 && t.start_quest == 0 {
        UseFork::Equip
    } else {
        dispatch_fork(t, inst_flags, item_text_id)
    }
}

/// The world frame's right-press hook's wrap leg (`0x492c20`): a pending gift wrap
/// (`IsWrappingItem 0x5edf10` at `0x492c44`) clears the cursor, `ClearCursor(1, 1)` (`0x495190` at
/// `0x492c54`), which ends the wrap with whatever it held ([`crate::target::right_press_hook`]).
pub(crate) fn clear_wrap_on_right_press(mut hosted_vm: crate::script_calls::HostedVm) {
    hosted_vm.run(|script| {
        if let Some(script) = script.filter(|s| s.gift_wrap_armed().is_some()) {
            script.clear_cursor_payload();
        }
    })
}

/// [`use_fork`] past the equip fork: the dispatcher's own arms, which `UseInventoryItem` reaches
/// directly (`0x4c7af0`).
pub(crate) fn dispatch_fork(
    t: &benilla_protocol::ItemInfo,
    inst_flags: u32,
    item_text_id: u32,
) -> UseFork {
    if t.unwraps_gift(inst_flags) {
        UseFork::Unwrap
    } else if t.begins_gift_wrap(inst_flags) {
        UseFork::WrapPaper
    } else if t.start_quest != 0 {
        UseFork::Quest
    } else if t.page_text != 0 {
        UseFork::Read
    } else if item_text_id != 0 {
        UseFork::Letter
    } else if t.opens_loot() {
        UseFork::Open
    } else {
        UseFork::Plain
    }
}

/// One container move past `SwapItem`'s question, which the verb asked in the VM
/// ([`benilla_ui::script::Swap`]); the cursor verb that filed it locked what it touched. A whole
/// stack is `CMSG_SWAP_INV_ITEM` when both ends are in the player's own array, else
/// `CMSG_SWAP_ITEM` (destination first, `Server/Packets/Item.cpp:30-36`); a split is
/// `CMSG_SPLIT_ITEM`.
pub(crate) fn send_container_move(commands: &NetCommands, mv: benilla_ui::script::ContainerMove) {
    let (Some(src), Some(dst)) = (
        wire_pos(mv.src_bag, mv.src_slot),
        wire_pos(mv.dst_bag, mv.dst_slot),
    ) else {
        debug!("ui_items: container move {mv:?} out of range — ignored");
        return;
    };
    match mv.count {
        None => {
            let (src_wire_bag, src_slot) = src;
            let (dst_wire_bag, dst_slot) = dst;
            if src_wire_bag == BAG_PLAYER_INVENTORY && dst_wire_bag == BAG_PLAYER_INVENTORY {
                debug!(
                    "ui_items: swap backpack lua {}→{} (wire 255 slot {src_slot}↔{dst_slot})",
                    mv.src_slot, mv.dst_slot
                );
                let _ = commands
                    .0
                    .send(ClientCommand::SwapInvItem { src_slot, dst_slot });
            } else {
                debug!(
                    "ui_items: swap whole-space lua {}/{}→{}/{} (wire {src_wire_bag}/{src_slot}→{dst_wire_bag}/{dst_slot})",
                    mv.src_bag, mv.src_slot, mv.dst_bag, mv.dst_slot
                );
                let _ = commands.0.send(ClientCommand::SwapItem {
                    dst_bag: dst_wire_bag,
                    dst_slot,
                    src_bag: src_wire_bag,
                    src_slot,
                });
            }
        }
        Some(n) => {
            let (src_bag, src_slot) = src;
            let (dst_bag, dst_slot) = dst;
            let count = n.min(u32::from(u8::MAX)) as u8;
            debug!(
                "ui_items: split lua {}/{}→{}/{} × {count} (wire {src_bag}/{src_slot}→{dst_bag}/{dst_slot})",
                mv.src_bag, mv.src_slot, mv.dst_bag, mv.dst_slot
            );
            let _ = commands.0.send(ClientCommand::SplitItem {
                src_bag,
                src_slot,
                dst_bag,
                dst_slot,
                count,
            });
        }
    }
}

/// The bag clicks of a harness's VM ([`UiScript::keep_calls`]), applied as the app's host applies
/// each inside its verb ([`super::calls::apply_kept_calls`]).
#[cfg(test)]
fn run_container_clicks(world: &mut World) {
    crate::ui_loot::seat_loot_steps(world);
    super::calls::apply_kept_calls(world);
}

/// A VM with no engine behind it, which keeps its calls for the harness to apply.
#[cfg(test)]
fn harness_vm() -> UiScript {
    let mut script = UiScript::new().unwrap();
    script.keep_calls();
    script
}

#[cfg(test)]
mod tests {
    use super::super::ITEM_CLASS_PROJECTILE;
    use super::*;
    use crate::items::Items;
    use benilla_protocol::messages::{ItemInfo, ObjectFields, ITEM_FLAG_LOOTABLE};

    /// Small Barnacled Clam.
    const CLAM_ENTRY: u32 = 7973;
    const CLAM: u64 = 0x4000_0000_0000_1939;
    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = 532;
    /// `OBJECT_FIELD_ENTRY` on the item object.
    const F_OBJECT_ENTRY: u16 = 3;

    /// Right-clicks backpack slot 1, holding a lootable template, and runs the click dispatcher.
    fn open_the_clam() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        open_the_clam_with(false, |_| {})
    }

    /// [`open_the_clam`] with Shift held as `shift` says, after `setup`.
    fn open_the_clam_with(
        shift: bool,
        setup: impl FnOnce(&mut App),
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_bank::BankOpen>()
            .init_resource::<crate::ui_item_text::ItemTextOpen>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<crate::ui_loot::LootLatch>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::spell::cast_target::AutoSelfCast>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::player::Player>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_cast::CastReports>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::creature_anim::AttackStopInFlight>()
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<crate::ui_tradeskill::TradeSkillOpens>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));

        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, CLAM as u32),
                (F_PACK_SLOT_1 + 1, (CLAM >> 32) as u32),
            ])),
        ));
        // The item object (`OBJECT_FIELD_TYPE` an item's) and its lootable template, so the open
        // arm claims the click.
        crate::items::test_spawn_item(
            app.world_mut(),
            CLAM,
            ObjectFields::from_pairs(&[(2, 0x3), (F_OBJECT_ENTRY, CLAM_ENTRY)]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                flags: ITEM_FLAG_LOOTABLE,
                ..crate::items::test_template("Small Barnacled Clam")
            }),
        );

        app.insert_non_send_resource(harness_vm());
        feed_clam(&mut app);
        crate::ui_loot::seat_loot_steps(app.world_mut());
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_modifiers(shift, false, false);
        setup(&mut app);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("UseContainerItem(0, 1)")
            .unwrap();
        run_container_clicks(app.world_mut());
        (app, rx)
    }

    /// Push backpack slot 1 as the container feed does: `UseContainerItem` reads the slot's item,
    /// its lock (`0x4fa1c7`, `0x4fa1cd`) and its fork, here from the clam's template as it stands.
    fn feed_clam(app: &mut App) {
        let t = app
            .world()
            .resource::<Items>()
            .template(CLAM_ENTRY, CLAM, Ask::Read)
            .cloned()
            .expect("the clam's template");
        let slot = benilla_ui::script::ContainerSlot {
            item_id: CLAM_ENTRY,
            count: 1,
            guid: CLAM,
            inventory_type: t.inventory_type,
            item_class: Some(t.class),
            quality: Some(t.quality),
            use_fork: use_fork(&t, 0, 0),
            ..Default::default()
        };
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_container(
                0,
                Some(benilla_ui::script::ContainerState {
                    num_slots: 16,
                    slots: std::collections::HashMap::from([(1, slot)]),
                    ..Default::default()
                }),
            );
    }

    /// Open a vendor, or close it with `None`, as the app holds it and as the merchant feed
    /// mirrors it to the VM.
    fn set_vendor(app: &mut App, vendor: Option<u64>) {
        app.world_mut()
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .vendor = vendor;
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_merchant(vendor.map(|_| benilla_ui::script::MerchantState::default()));
    }

    /// Open a banker, as the app holds it and as the bank feed mirrors it to the VM.
    fn open_banker(app: &mut App, banker: u64) {
        app.world_mut()
            .resource_mut::<crate::ui_bank::BankOpen>()
            .open(banker);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_bank(Some(benilla_ui::script::BankState::default()));
    }

    /// vmangos answers with a type-1 `SMSG_LOOT_RESPONSE` on the item's guid, refused against a
    /// cold latch: without the latch the clam greys and no window opens.
    #[test]
    fn the_open_item_send_arms_the_loot_latch_on_the_items_own_guid() {
        let (app, rx) = open_the_clam();
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::OpenItem {
                    bag_index: 255,
                    slot: 23
                })
            ),
            "the open arm ships CMSG_OPEN_ITEM for backpack slot 1"
        );
        assert_eq!(
            app.world().resource::<crate::ui_loot::LootLatch>().0,
            Some(CLAM),
            "…having first latched the item's own guid, or the type-1 answer is refused"
        );
        // The grey lock, the reference's other pre-send write (`0x5edcd9`).
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .item_locked(CLAM),
            "the slot greys at the click"
        );
    }

    /// `OpenItem` stores whether Shift is down through `SetAutoLoot` (`0x5edca7`-`0x5edcb6`) before
    /// it holds the item as the loot target (`0x5edcc0`), locks it (`0x5edcd9`) and sends
    /// `CMSG_OPEN_ITEM`: alone the byte waits for the item's answer; with a window open on another
    /// source the store sweeps it at once (`0x5df470` → `0x5df480`), its rows and its
    /// `LOOT_OPENED` ahead of the lock and the open.
    #[test]
    fn a_shift_open_stores_the_byte_and_sweeps_an_open_window_first() {
        const CORPSE: u64 = 0xF130_0000_0000_00AB;
        const LINEN: u32 = 2589;
        let (app, rx) = open_the_clam_with(true, |_| {});
        assert!(matches!(
            rx.try_iter().collect::<Vec<_>>()[..],
            [ClientCommand::OpenItem { .. }]
        ));
        assert!(app.world().resource::<crate::ui_loot::AutoLootByte>().0);

        let (app, rx) = open_the_clam_with(true, |app| {
            let corpse = app
                .world_mut()
                .spawn(ObjectStore(ObjectFields::from_pairs(&[(2, 0x9)])))
                .id();
            let world = app.world_mut();
            world
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(CORPSE, corpse);
            world
                .resource_mut::<Items>()
                .insert_template(LINEN, Some(crate::items::test_template("Linen Cloth")));
            world.resource_mut::<crate::ui_loot::LootState>().open(
                CORPSE,
                1,
                0,
                vec![benilla_protocol::messages::LootItem {
                    slot: 0,
                    item_id: LINEN,
                    count: 1,
                    display_info_id: 0,
                    random_property_id: 0,
                    slot_type: benilla_protocol::messages::slot_type::ALLOW_LOOT,
                }],
            );
            world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(CORPSE);
            app.world()
                .non_send_resource::<UiScript>()
                .run(
                    "SEEN = {} local f = CreateFrame('Frame') \
                     f:RegisterEvent('LOOT_OPENED') f:RegisterEvent('ITEM_LOCK_CHANGED') \
                     f:SetScript('OnEvent', function() tinsert(SEEN, event) end)",
                )
                .unwrap();
        });
        let seen: String = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return table.concat(SEEN, ',')")
            .unwrap();
        assert_eq!(
            seen, "LOOT_OPENED,ITEM_LOCK_CHANGED",
            "the sweep, then the lock"
        );
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::AutostoreLootItem { slot: 0 },
                    ClientCommand::OpenItem { .. }
                ]
            ),
            "the sweep, then the open: {sent:?}"
        );
    }

    /// A repair-mode right-click on a bag item: `ITEM_REPAIR` as it goes out (`0x4f9ce4`), then
    /// `CMSG_REPAIR_ITEM` with the vendor's and the item's guid, and no use or sale.
    #[test]
    fn a_repair_click_sounds_and_ships_the_items_guid() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .vendor = Some(VENDOR);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.take_sounds();
            let mut bag = benilla_ui::script::ContainerState {
                num_slots: 16,
                ..Default::default()
            };
            bag.slots.insert(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: CLAM_ENTRY,
                    count: 1,
                    ..Default::default()
                },
            );
            script.set_container(0, Some(bag));
            script.set_merchant(Some(benilla_ui::script::MerchantState {
                can_repair: true,
                ..Default::default()
            }));
            script
                .run("ShowRepairCursor() UseContainerItem(0, 1)")
                .unwrap();
        }
        run_container_clicks(app.world_mut());

        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::RepairItem {
                vendor: VENDOR,
                item_guid: CLAM
            })
        ));
        assert!(rx.try_recv().is_err(), "the repair alone: no use or sale");
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert_eq!(
            script.take_sounds(),
            vec![benilla_ui::script::SoundRequest::KitName(
                "ITEM_REPAIR".into()
            )]
        );
    }

    /// A slot whose item object has not streamed ends the call before any arm (`0x4fa1c7`): no
    /// sale goes out and nothing locks, as `LockItem` on an unresolved guid signals nothing.
    #[test]
    fn a_use_on_an_unstreamed_item_does_nothing() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        const GHOST: u64 = 0x4000_0000_0000_0d0d;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, GHOST as u32),
                (F_PACK_SLOT_1 + 1, (GHOST >> 32) as u32),
            ])));
        set_vendor(&mut app, Some(VENDOR));
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.unlock_item(CLAM);
            s.run("UseContainerItem(0, 1)").unwrap();
        }
        run_container_clicks(app.world_mut());
        assert!(rx.try_recv().is_err(), "nothing sold");
        assert!(!app
            .world()
            .non_send_resource::<UiScript>()
            .item_locked(GHOST));
    }

    /// Two uses in one frame: the first's sale locks the item in its call (`0x4fa37f`), so the
    /// second call stops at the lock (`0x4fa1cd`).
    #[test]
    fn two_sales_queued_in_one_frame_send_once() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        set_vendor(&mut app, Some(VENDOR));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("UseContainerItem(0, 1) UseContainerItem(0, 1)")
            .unwrap();
        run_container_clicks(app.world_mut());
        assert_eq!(
            rx.try_iter().count(),
            1,
            "the second use sees the first sale's lock (`0x4fa37f`, `0x4fa1cd`)"
        );
    }

    /// A use whose arm takes no lock leaves the item free for a later call: the reference sends
    /// the use inside `UseContainerItem`, so a pickup after it in the frame picks up and the use
    /// still goes out.
    #[test]
    fn a_use_then_a_pickup_in_one_frame_sends_the_use() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut().resource_mut::<Items>().insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                start_quest: 42,
                ..crate::items::test_template("Questing Clam")
            }),
        );
        feed_clam(&mut app);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.unlock_item(CLAM);
            script
                .run("UseContainerItem(0, 1) PickupContainerItem(0, 1)")
                .unwrap();
        }
        run_container_clicks(app.world_mut());
        assert!(rx.try_recv().is_ok(), "the use went out");
        let script = app.world().non_send_resource::<UiScript>();
        assert!(
            script.cursor_item().is_some() && script.item_locked(CLAM),
            "and the pickup held"
        );
    }

    /// The open arm locks the item inside the call, past its `SetAutoLoot` (`0x5edcd9`), so a
    /// second right-click in the same frame, each applied as its call returns, stops at the lock
    /// (`0x4fa1cd`) and one `CMSG_OPEN_ITEM` goes out.
    #[test]
    fn two_opens_in_one_frame_send_once() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        let click = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run("UseContainerItem(0, 1)")
                .unwrap();
            run_container_clicks(app.world_mut());
        };
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        click(&mut app);
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .item_locked(CLAM),
            "locked in the call, before any send"
        );
        click(&mut app);
        assert_eq!(rx.try_iter().count(), 1);
    }

    /// `UseContainerItem` tests the banker (`[0xbdd038]`, `0x4fa322`) before the vendor
    /// (`[0xbddfa0]`, `0x4fa35b`), so with both open the click deposits.
    #[test]
    fn a_banker_takes_the_click_before_a_vendor() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        set_vendor(&mut app, Some(0xF130_0000_0000_0042));
        open_banker(&mut app, 0xF130_0000_0000_0043);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("UseContainerItem(0, 1)")
            .unwrap();
        run_container_clicks(app.world_mut());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::AutoBankItem { .. })
        ));
        assert!(rx.try_recv().is_err(), "no sale");
    }

    /// An item picked up before its record landed parks `AutoEquipCursorItem` on it
    /// (`0x5e160c`), sending nothing and keeping the item held and locked. The record's callback
    /// (`0x5e17a0`) runs the call again once the bag view has it: a projectile by its class
    /// (`0x5e16ff`), not its inventory type, so `CMSG_SET_AMMO` goes out and the arm's
    /// `ClearCursor(1,1)` lets it go (`0x5e1711`, `0x5e1763`).
    #[test]
    fn a_projectile_picked_up_unresolved_waits_then_loads_by_class_and_unlocks() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        let push = |app: &mut App, cached: bool| {
            let slot = benilla_ui::script::ContainerSlot {
                item_id: CLAM_ENTRY,
                count: 1,
                guid: CLAM,
                item_class: cached.then_some(ITEM_CLASS_PROJECTILE),
                quality: cached.then_some(1),
                ..Default::default()
            };
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_container(
                0,
                Some(benilla_ui::script::ContainerState {
                    num_slots: 16,
                    slots: std::collections::HashMap::from([(1, slot)]),
                    ..Default::default()
                }),
            );
            script.seat_item_records();
        };
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        // The slot as the feed pushes it with the record in flight.
        push(&mut app, false);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();
        app.world_mut().resource_mut::<Items>().insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                class: ITEM_CLASS_PROJECTILE,
                ..crate::items::test_template("Odd Arrow")
            }),
        );
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());
        assert!(rx.try_recv().is_err(), "nothing sent while it waits");
        {
            let script = app.world().non_send_resource::<UiScript>();
            assert_eq!(script.cursor_item().map(|i| i.guid), Some(CLAM), "held");
            assert!(script.item_locked(CLAM), "the pickup's lock");
        }
        // The feed pushes the answer, then the callback runs.
        push(&mut app, true);
        app.world_mut()
            .run_system_once(crate::ui_bind_confirm::replay_equip_records)
            .unwrap();
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::SetAmmo { entry: CLAM_ENTRY })
        ));
        let script = app.world().non_send_resource::<UiScript>();
        assert!(script.cursor_item().is_none());
        assert!(!script.item_locked(CLAM));
    }

    /// A record the cache has not answered is asked with the equip callback (`0x55ba30`): the
    /// auto-equip's `0x5e17a0`, and for a swap `0x5e1020` with the move parked in the table
    /// (`0x5e0dbe`). A refused record frees the parked move unreplayed (`0x5e10cf`), its items
    /// left locked.
    #[test]
    fn an_unanswered_record_is_asked_with_the_equip_callback() {
        use crate::items::RecordCallback;
        use bevy::ecs::system::RunSystemOnce;
        const UNKNOWN: u32 = 99_999;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {}
        let mv = benilla_ui::script::ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: EQUIPMENT_BAG,
            dst_slot: 16,
            count: None,
        };
        app.world_mut()
            .run_system_once(
                move |mut gate: crate::ui_bind_confirm::BindGate,
                      items: Res<Items>,
                      net: Res<NetCommands>| {
                    gate.await_auto_equip(UNKNOWN, &items, &net);
                    gate.park_swap(&mv, UNKNOWN, &items, &net);
                },
            )
            .unwrap();
        assert!(
            rx.try_iter()
                .any(|c| matches!(c, ClientCommand::ItemQuery { entry: UNKNOWN, .. })),
            "the record is asked"
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1,
            "the parked swap holds a record"
        );
        let callbacks = app
            .world_mut()
            .resource_mut::<Items>()
            .take_callbacks(UNKNOWN);
        assert_eq!(callbacks, [RecordCallback::AutoEquip, RecordCallback::Swap]);
        // Refused: the callbacks run with nothing found.
        for cb in callbacks {
            app.world()
                .resource::<Items>()
                .run_with_vm(UNKNOWN, cb, false);
        }
        app.world_mut()
            .run_system_once(crate::ui_bind_confirm::replay_equip_records)
            .unwrap();
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0,
            "the refusal freed the parked move"
        );
        assert!(rx.try_recv().is_err(), "and nothing was sent");
    }

    /// The sale and bank arms lock the clicked item after their send (`0x4fa37f`; `0x4fa312`,
    /// `0x4fa34b`), and a locked item's click stops before any arm (`0x4fa1cd`).
    #[test]
    fn the_sale_and_bank_arms_lock_the_item_and_a_locked_click_stops() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        let click = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run("UseContainerItem(0, 1)")
                .unwrap();
            run_container_clicks(app.world_mut());
        };
        let locked = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .item_locked(CLAM)
        };
        assert!(locked(&app), "the open click's own lock");
        set_vendor(&mut app, Some(VENDOR));
        click(&mut app);
        assert!(rx.try_recv().is_err(), "a locked item sells nothing");

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        click(&mut app);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::SellItem {
                item_guid: CLAM,
                ..
            })
        ));
        assert!(
            locked(&app),
            "the sale locks it until SMSG_SELL_ITEM or the slot moves"
        );

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        set_vendor(&mut app, None);
        open_banker(&mut app, 0xF130_0000_0000_0043);
        click(&mut app);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::AutoBankItem { .. })
        ));
        assert!(locked(&app), "the deposit locks it");
    }

    /// The equip arm locks the item after the equip (`0x4fa418`), but not a projectile, which
    /// loads by entry and stays in its bag (`0x4fa40b`): its arm's `ClearCursor(1,1)` unlocks it
    /// instead, signalling as the lock does (`0x5e1763`).
    #[test]
    fn the_equip_arm_locks_all_but_a_projectile() {
        for (class, inventory_type, want) in [(4u32, 1u32, true), (6, 24, false)] {
            let (mut app, rx) = open_the_clam();
            while rx.try_recv().is_ok() {} // drain the clam's own send
            app.world_mut().resource_mut::<Items>().insert_template(
                CLAM_ENTRY,
                Some(ItemInfo {
                    class,
                    inventory_type,
                    ..crate::items::test_template("Worn Thing")
                }),
            );
            feed_clam(&mut app);
            {
                let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
                // The record feed's copy of the template, which the equip question reads.
                script.seat_item_records();
                script.unlock_item(CLAM);
                script
                    .run(
                        "SIGNALS = 0 local f = CreateFrame('Frame') \
                         f:RegisterEvent('ITEM_LOCK_CHANGED') \
                         f:SetScript('OnEvent', function() SIGNALS = SIGNALS + 1 end)",
                    )
                    .unwrap();
                script.run("UseContainerItem(0, 1)").unwrap();
                // The lock, or a projectile's unlock, signals in the call.
                assert_eq!(
                    script.eval::<i64>("return SIGNALS").unwrap(),
                    1,
                    "class {class}"
                );
            }
            run_container_clicks(app.world_mut());
            assert!(rx.try_recv().is_ok(), "class {class}: the equip went out");
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .item_locked(CLAM),
                want,
                "class {class}"
            );
        }
    }

    /// `CGItem::Use` refuses a locked item first (`0x5d8d32`): a worn item's use from the doll
    /// (`0x4c7af0`) sends nothing while it is locked, which its call reads.
    #[test]
    fn a_locked_worn_item_is_not_used() {
        const HELM: u64 = 0x4000_0000_0000_0777;
        const HELM_ENTRY: u32 = 7777;
        let head = benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (head, HELM as u32),
                (head + 1, (HELM >> 32) as u32),
            ])));
        crate::items::test_spawn_item(
            app.world_mut(),
            HELM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, HELM_ENTRY)]),
            false,
        );
        app.world_mut().resource_mut::<Items>().insert_template(
            HELM_ENTRY,
            Some(ItemInfo {
                start_quest: 42,
                ..crate::items::test_template("Questing Helm")
            }),
        );
        // The doll as the inventory feed pushes it: the call reads the worn item's lock.
        let mut worn: benilla_ui::script::InventorySlots = Default::default();
        worn[1] = Some(benilla_ui::script::InvSlotView {
            item_id: HELM_ENTRY,
            guid: HELM,
            ..Default::default()
        });
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_inventory_slots(worn);
        let use_head = |app: &mut App| {
            use bevy::ecs::system::RunSystemOnce;
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run("UseInventoryItem(1)")
                .unwrap();
            app.world_mut()
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut items: ScriptItemUse| {
                        for call in script.take_script_calls() {
                            if let benilla_ui::script::ScriptCall::UseInventoryItem {
                                slot: id,
                                on_self,
                                ..
                            } = call
                            {
                                items.use_inventory_item(&mut script, id, on_self);
                            }
                        }
                    },
                )
                .unwrap();
        };
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(HELM);
        use_head(&mut app);
        assert!(rx.try_recv().is_err(), "a locked item is not used");
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(HELM);
        use_head(&mut app);
        assert!(rx.try_recv().is_ok(), "unlocked, the use goes out");
    }

    /// A repair-mode click on a worn item (`0x4c7714`): `ITEM_REPAIR` as it goes out (`0x4c7780`),
    /// then `CMSG_REPAIR_ITEM` with the vendor's and the worn item's guid.
    #[test]
    fn a_worn_repair_click_sounds_and_ships_the_items_guid() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        const HELM: u64 = 0x4000_0000_0000_0777;
        let head = benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .vendor = Some(VENDOR);
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (head, HELM as u32),
                (head + 1, (HELM >> 32) as u32),
            ])));
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.take_sounds();
            let mut slots: benilla_ui::script::InventorySlots = Default::default();
            slots[1] = Some(benilla_ui::script::InvSlotView {
                item_id: 7,
                ..Default::default()
            });
            script.set_inventory_slots(slots);
            script.set_merchant(Some(benilla_ui::script::MerchantState {
                can_repair: true,
                ..Default::default()
            }));
            script
                .run("ShowRepairCursor() PickupInventoryItem(1)")
                .unwrap();
        }
        run_container_clicks(app.world_mut());

        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::RepairItem {
                vendor: VENDOR,
                item_guid: HELM
            })
        ));
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert_eq!(
            script.take_sounds(),
            vec![benilla_ui::script::SoundRequest::KitName(
                "ITEM_REPAIR".into()
            )]
        );
    }

    /// What one repair click did: the packets, the VM's sounds, the `UI_ERROR_MESSAGE` lines and
    /// the message rows queued for their sound.
    struct RepairClick {
        sent: Vec<ClientCommand>,
        sounds: Vec<benilla_ui::script::SoundRequest>,
        shown: Vec<String>,
        sounded: Vec<&'static str>,
    }

    /// A repair-mode click on [`crate::ui_merchant::purse_fixture`]'s sword with `money` in the
    /// purse: in backpack slot 1 through `UseContainerItem`, or `worn` in the main hand (slot 15)
    /// through `PickupInventoryItem`.
    fn click_to_repair_the_sword(money: u32, worn: bool) -> RepairClick {
        use crate::ui_merchant::purse_fixture::{player, record_errors, seat, SWORD_ENTRY};
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        seat(app.world_mut());
        let slot_field = if worn {
            FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15
        } else {
            F_PACK_SLOT_1
        };
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert((player(slot_field, money), crate::net::Guid(0x5e1f)));
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.take_sounds();
            record_errors(&script);
            script.set_merchant(Some(benilla_ui::script::MerchantState {
                can_repair: true,
                ..Default::default()
            }));
            let click = if worn {
                let mut slots: benilla_ui::script::InventorySlots = Default::default();
                slots[16] = Some(benilla_ui::script::InvSlotView {
                    item_id: SWORD_ENTRY,
                    ..Default::default()
                });
                script.set_inventory_slots(slots);
                "ShowRepairCursor() PickupInventoryItem(16)"
            } else {
                let mut bag = benilla_ui::script::ContainerState {
                    num_slots: 16,
                    ..Default::default()
                };
                bag.slots.insert(
                    1,
                    benilla_ui::script::ContainerSlot {
                        item_id: SWORD_ENTRY,
                        count: 1,
                        ..Default::default()
                    },
                );
                script.set_container(0, Some(bag));
                "ShowRepairCursor() UseContainerItem(0, 1)"
            };
            script.run(click).unwrap();
        }
        run_container_clicks(app.world_mut());
        let sounded = crate::ui_merchant::purse_fixture::sounded(app.world());
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        RepairClick {
            sent: rx.try_iter().collect(),
            sounds: script.take_sounds(),
            shown: crate::ui_merchant::purse_fixture::shown(&script),
            sounded,
        }
    }

    /// The repair arm's purse test, unsigned `cost > money` on the discounted cost: one copper
    /// short shows `ERR_NOT_ENOUGH_MONEY` with its speech row, plays no `ITEM_REPAIR` and sends
    /// nothing; exactly the cost, under the full price, sounds and sends.
    fn the_purse_test_refuses_one_copper_short_and_pays_the_exact_cost(worn: bool) {
        use crate::ui_merchant::purse_fixture::{sword_prices, NOT_ENOUGH_MONEY, SWORD, VENDOR};
        let (full, discounted) = sword_prices();

        let short = click_to_repair_the_sword(discounted - 1, worn);
        assert!(
            short.sent.is_empty(),
            "short of {discounted}: nothing goes out"
        );
        assert!(short.sounds.is_empty(), "…no ITEM_REPAIR");
        assert_eq!(short.shown, [NOT_ENOUGH_MONEY], "…the red line");
        assert_eq!(
            short.sounded,
            ["ERR_NOT_ENOUGH_MONEY"],
            "…and its row, which speaks line 0x28"
        );

        let exact = click_to_repair_the_sword(discounted, worn);
        assert!(
            matches!(
                exact.sent.as_slice(),
                [ClientCommand::RepairItem {
                    vendor: VENDOR,
                    item_guid: SWORD
                }]
            ),
            "{discounted} in the purse pays the discounted {discounted}, short of the full {full}"
        );
        assert_eq!(
            exact.sounds,
            vec![benilla_ui::script::SoundRequest::KitName(
                "ITEM_REPAIR".into()
            )]
        );
        assert!(exact.shown.is_empty() && exact.sounded.is_empty());
    }

    /// `UseContainerItem`'s repair arm (`0x4fa22e`–`0x4fa236`, error `0x4fa23a`).
    #[test]
    fn a_bag_repair_the_purse_cannot_cover_is_refused_and_one_it_covers_is_sent() {
        the_purse_test_refuses_one_copper_short_and_pays_the_exact_cost(false);
    }

    /// `PickupInventoryItem`'s repair arm (`0x4c775f`–`0x4c7766`, error `0x4c776a`).
    #[test]
    fn a_worn_repair_the_purse_cannot_cover_is_refused_and_one_it_covers_is_sent() {
        the_purse_test_refuses_one_copper_short_and_pays_the_exact_cost(true);
    }

    /// The unwrap arm's other side: wrapping paper arms the wrap cursor and sends nothing.
    #[test]
    fn a_wrapper_right_click_arms_the_cursor_and_ships_no_packet() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
                                       // Slot 1 becomes wrapping paper: WRAPPER on the
                                       // template, no WRAPPED bit on the instance.
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                flags: benilla_protocol::messages::ITEM_FLAG_WRAPPER,
                ..crate::items::test_template("Red Ribboned Wrapping Paper")
            }),
        );
        feed_clam(&mut app);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            // The open click's lock, which a locked item's click would stop at (`0x4fa1cd`).
            script.unlock_item(CLAM);
            script.run("UseContainerItem(0, 1)").unwrap();
        }
        run_container_clicks(app.world_mut());

        assert!(
            rx.try_recv().is_err(),
            "the begin-wrap arm is purely local — nothing goes on the wire until a container \
             click spends it"
        );
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(
            script.gift_wrap_armed(),
            Some(benilla_ui::script::PendingWrap {
                guid: CLAM,
                bag: 0,
                slot: 1
            }),
            "…and the paper is armed"
        );
    }
}

#[cfg(test)]
mod bind_confirm_tests {
    use super::*;
    use crate::items::Items;
    use benilla_protocol::messages::{ItemInfo, ObjectFields};
    use benilla_ui::script::{ContainerSlot, ContainerState};

    /// Flurry Axe, a real `item_template` row: quality 4, bonding 2 (bind on equip).
    const FLURRY_AXE: u32 = 871;
    const AXE_GUID: u64 = 0x4000_0000_0000_0871;
    /// `ITEM_FIELD_FLAGS` (field 21); bit 0 is soulbound (`0x5da2c0`).
    const F_ITEM_FLAGS: u16 = 21;
    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = 532;
    const F_OBJECT_ENTRY: u16 = 3;

    fn load_ui(s: &UiScript) {
        // Chain files, so through the chain-aware reader.
        for file in [
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\Fonts.xml",
            "Interface\\FrameXML\\BasicControls.xml",
            "Interface\\FrameXML\\LocaleProperties.lua",
            r"Interface\FrameXML\UIParent.xml",
            r"Interface\FrameXML\MoneyFrame.lua",
            r"Interface\FrameXML\MoneyFrame.xml",
            "Interface\\FrameXML\\StaticPopup.xml",
        ] {
            crate::ui_script::load_ui_for_test(s, file);
        }
    }

    fn bag_with_the_axe(quality: u32) -> ContainerState {
        bag_with_the_axe_of(2, quality, false)
    }

    /// The backpack as the feed pushes it with the axe's record cached: its bonding and bound bit
    /// are what the VM's `SwapItem` question reads.
    fn bag_with_the_axe_of(bonding: u32, quality: u32, already_bound: bool) -> ContainerState {
        let mut bag = bag_with_the_axe_raw(quality);
        let axe = bag.slots.get_mut(&1).expect("the axe");
        axe.bonding = bonding;
        axe.already_bound = already_bound;
        bag
    }

    fn bag_with_the_axe_raw(quality: u32) -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                uncached_link: None,
                use_fork: Default::default(),
                item_class: None,
                duration_ms: None,
                field_duration: 0,
                petition: None,
                already_bound: false,
                occupied_seats: None,
                bonding: 2,
                use_spell: 0,
                bar_placeable: true,
                consumable: false,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Axe_01".into()),
                count: 1,
                quality: Some(quality),
                inventory_type: 0,
                item_id: FLURRY_AXE,
                link: Some("|cffa335ee|Hitem:871:0:0:0|h[Flurry Axe]|h|r".into()),
                guid: AXE_GUID,
                equip_slots: vec![16], // MainHandSlot
                cooldown: None,
                readable: false,
                creator: None,
                gift_creator: None,
                creator_guid: 0,
                gift_creator_guid: 0,
                flags: 0,
                enchants: Vec::new(),
                spell_0: false,
                spell_charges: [0; 5],
            },
        );
        ContainerState {
            name: None,
            num_slots: 16,
            slots,
        }
    }

    /// Places the axe from backpack slot 1 on the empty main hand and applies the move's call.
    fn place_the_axe_on_the_doll() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        place_the_axe_with(2, 4, false)
    }

    fn place_the_axe_with_bonding(
        bonding: u32,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        place_the_axe_with(bonding, 4, false)
    }

    /// The place, over everything the gate reads; `quality` must not matter.
    fn place_the_axe_with(
        bonding: u32,
        quality: u32,
        already_bound: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        place_the_axe_after(bonding, quality, already_bound, |_| {})
    }

    /// [`place_the_axe_with`] after `setup` on the VM.
    fn place_the_axe_after(
        bonding: u32,
        quality: u32,
        already_bound: bool,
        setup: impl FnOnce(&mut UiScript),
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));

        app.world_mut().spawn((
            SelfPlayer,
            // `UNIT_FIELD_HEALTH` (22): alive.
            ObjectStore(ObjectFields::from_pairs(&[
                (22, 100),
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[
                (F_OBJECT_ENTRY, FLURRY_AXE),
                (F_ITEM_FLAGS, u32::from(already_bound)),
            ]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality,
                bonding,
                ..crate::items::test_template("Flurry Axe")
            }),
        );

        let mut script = harness_vm();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(
            0,
            Some(bag_with_the_axe_of(bonding, quality, already_bound)),
        );
        let doll: benilla_ui::script::InventorySlots = Default::default();
        script.set_inventory_slots(doll);
        setup(&mut script);
        script.seat_item_records();
        // Pick up and drop on the main hand: the swap's call, kept for the host.
        script.run("PickupContainerItem(0, 1)").unwrap();
        script.run("PickupInventoryItem(16)").unwrap();
        app.insert_non_send_resource(script);
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());
        (app, rx)
    }

    fn shown(app: &mut App, which: &str) -> bool {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<bool>(&format!(
                "return StaticPopup_FindVisible(\"{which}\") ~= nil"
            ))
            .unwrap()
    }

    /// The question is signalled inside the swap's call (`0x5e0ee4`), and the doll click's
    /// `SwapItem` leaves by its exit past the clear (`0x5e0eec` → `0x5e0f63`): the axe stays held
    /// and locked under the dialog, no `CURSOR_UPDATE` hides it, nothing is sent. A later cursor
    /// change retires it (`UIParent.lua:357-361`), and its `OnHide` frees the record.
    #[test]
    fn placing_a_boe_on_the_doll_asks_in_the_call_with_the_axe_held() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        assert!(
            rx.try_iter().next().is_none(),
            "the deferred place sends nothing"
        );
        assert!(shown(&mut app, "EQUIP_BIND"), "up inside the call");
        {
            let script = app.world_mut().non_send_resource_mut::<UiScript>();
            assert_eq!(script.cursor_item().map(|i| i.guid), Some(AXE_GUID));
            assert!(script.item_locked(AXE_GUID), "held and locked");
            assert!(script.eval::<bool>("return CursorHasItem() == 1").unwrap());
            assert_eq!(
                script
                    .eval::<i64>("return StaticPopup_FindVisible(\"EQUIP_BIND\").data")
                    .unwrap(),
                0,
                "carrying the pending-array index, which is 0 for the first record"
            );
        }

        // A quiet frame changes nothing.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "EQUIP_BIND"), "and stays up");

        // A later cursor change retires it, and its OnHide cancels the record.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .fire_event("CURSOR_UPDATE", vec![]);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "a later cursor change still retires the stale question"
        );
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0,
            "and the record it was holding is freed, not leaked"
        );
        assert!(
            rx.try_iter().next().is_none(),
            "a retired question never sends"
        );
    }

    /// A place whose item's record the bag view does not hold yet parks on it (`0x5e0dbe`): no
    /// question, nothing sent, the axe held, a record in the table. The callback (`0x5e1020`)
    /// replays the swap once the view has the record, which asks now (`0x5e0ee4`), and frees the
    /// parked record (`0x5e10cf`).
    #[test]
    fn a_place_on_an_uncached_record_waits_then_asks_on_its_arrival() {
        benilla_formats::wow_data_or_skip!();
        use bevy::ecs::system::RunSystemOnce;
        let pending = |s: &mut UiScript| {
            let mut bag = bag_with_the_axe(4);
            bag.slots.get_mut(&1).expect("the axe").quality = None;
            s.set_container(0, Some(bag));
        };
        let (mut app, rx) = place_the_axe_after(2, 4, false, pending);
        assert!(!shown(&mut app, "EQUIP_BIND"), "no question while it waits");
        assert!(rx.try_iter().next().is_none(), "nothing sent");
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1,
            "the parked move holds a record"
        );
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .cursor_item()
                .map(|i| i.guid),
            Some(AXE_GUID)
        );
        // The bag view takes the record, then the callback replays the swap.
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_container(0, Some(bag_with_the_axe(4)));
            script.seat_item_records();
        }
        app.world_mut()
            .run_system_once(crate::ui_bind_confirm::replay_equip_records)
            .unwrap();
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());
        assert!(shown(&mut app, "EQUIP_BIND"), "the replay asks");
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1,
            "its question's record alone: the parked one is freed"
        );
    }

    /// The accept re-issues `SwapItem` confirmed (`0x5e1c46`; 1.12 has no confirm opcode): it
    /// sends without asking again, and its clear (`0x5e0fe7`) takes the axe off the cursor.
    #[test]
    fn accepting_re_issues_the_place_and_does_not_ask_again() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        assert!(shown(&mut app, "EQUIP_BIND"));

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"EQUIP_BIND\"), 1)")
            .unwrap();
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());

        // Backpack slot 1 is 255/23 and the main hand 255/15, both in the player's own array.
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::SwapInvItem {
                    src_slot: 23,
                    dst_slot: 15
                }]
            ),
            "the accept sends the original swap, once: {sent:?}"
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0,
            "and frees its record — the OnHide's cancel finds nothing left to cancel"
        );
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "and no second question: the re-issue was confirmed"
        );
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .cursor_item()
                .is_none(),
            "the re-issue's send clears the cursor"
        );
    }

    /// The stock popup cancels twice, from `OnCancel` and `OnHide`; the second finds the record
    /// already free. The cancel unlocks what the place locked (`0x5e1ca5`, `0x5e1d04`): the axe,
    /// and the empty main hand's nothing.
    #[test]
    fn cancelling_sends_nothing_unlocks_and_the_doubled_cancel_is_harmless() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .item_locked(AXE_GUID),
            "the pickup's lock holds under the question"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"EQUIP_BIND\"), 2)")
            .unwrap();
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            let answers = s.take_pending_equip_answers();
            assert_eq!(answers.len(), 2, "OnCancel and OnHide both fire");
            // Answered again as calls, as the popup made them.
            for a in answers {
                s.run(&format!("CancelPendingEquip({})", a.index)).unwrap();
            }
        }
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());
        assert!(rx.try_iter().next().is_none(), "a cancel never sends");
        assert!(!app
            .world()
            .non_send_resource::<UiScript>()
            .item_locked(AXE_GUID));
    }

    /// The equip predicate is `bonding == 2` alone, with no quality leg (`0x5e0e54`).
    #[test]
    fn only_bind_on_equip_defers_the_place() {
        benilla_formats::wow_data_or_skip!();
        for (bonding, why) in [
            (0u32, "no bind"),
            (1, "bind on PICKUP is the loot arm's value, not this one"),
            (3, "bind on USE is the use arm's"),
            (4, "quest item"),
        ] {
            let (mut app, rx) = place_the_axe_with_bonding(bonding);
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .tick(0.01);
            assert!(
                !shown(&mut app, "EQUIP_BIND"),
                "bonding {bonding} must not ask ({why})"
            );
            assert!(
                rx.try_iter().next().is_some(),
                "bonding {bonding} places straight through ({why})"
            );
        }
    }

    #[test]
    fn a_white_bind_on_equip_item_still_asks() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_with(2, 1, false);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "EQUIP_BIND"),
            "quality 1 is irrelevant to the equip arm"
        );
        assert!(rx.try_iter().next().is_none());
    }

    /// `0x5da2c0`, the predicate the tooltip's Soulbound override also uses.
    #[test]
    fn an_already_bound_item_places_without_asking() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_with(2, 4, true);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "there is nothing left to bind"
        );
        assert!(rx.try_iter().next().is_some(), "and it places");
    }

    /// The auto-equip arm (`0x5e1480`, event 289): the same predicate and re-issue, its own dialog.
    #[test]
    fn auto_equipping_a_boe_asks_and_the_accept_re_issues() {
        benilla_formats::wow_data_or_skip!();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        app.world_mut().spawn((
            SelfPlayer,
            // `UNIT_FIELD_HEALTH` (22): alive.
            ObjectStore(ObjectFields::from_pairs(&[
                (22, 100),
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality: 4,
                bonding: 2,
                ..crate::items::test_template("Flurry Axe")
            }),
        );
        let mut script = harness_vm();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(4)));
        script.seat_item_records();
        script.run("PickupContainerItem(0, 1)").unwrap();
        script.run("AutoEquipCursorItem()").unwrap();
        app.insert_non_send_resource(script);
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());

        assert!(
            rx.try_iter().next().is_none(),
            "the deferred equip sends nothing"
        );
        {
            let script = app.world().non_send_resource::<UiScript>();
            assert_eq!(
                script.cursor_item().map(|i| i.guid),
                Some(AXE_GUID),
                "the axe stays on the cursor under the question (`0x5e16ce`)"
            );
            assert!(script.item_locked(AXE_GUID), "and locked");
        }
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "AUTOEQUIP_BIND"),
            "the auto-equip arm raises its OWN dialog, not the placement one"
        );
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "and the two are distinct entries"
        );

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"AUTOEQUIP_BIND\"), 1)")
            .unwrap();
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 23
                }]
            ),
            "the accept re-issues the auto-equip on the item's wire position: {sent:?}"
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0
        );
        let script = app.world().non_send_resource::<UiScript>();
        assert!(
            script.cursor_item().is_none(),
            "the accept's pick-up and send leave the cursor empty"
        );
        assert_eq!(
            app.world_mut()
                .resource_mut::<Messages<crate::sound::AutoEquipSound>>()
                .drain()
                .count(),
            1,
            "and the re-issue's pickup and put-down play"
        );
    }

    /// The cancel only unlocks (`0x5e1c50`): the axe stays on the cursor, free, nothing is sent.
    #[test]
    fn cancelling_a_boe_auto_equip_leaves_the_item_held_and_unlocked() {
        benilla_formats::wow_data_or_skip!();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        app.world_mut().spawn((
            SelfPlayer,
            // `UNIT_FIELD_HEALTH` (22): alive.
            ObjectStore(ObjectFields::from_pairs(&[
                (22, 100),
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
            false,
        );
        app.world_mut().resource_mut::<Items>().insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality: 4,
                bonding: 2,
                ..crate::items::test_template("Flurry Axe")
            }),
        );
        let mut script = harness_vm();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(4)));
        script.seat_item_records();
        script.run("PickupContainerItem(0, 1)").unwrap();
        script.run("AutoEquipCursorItem()").unwrap();
        app.insert_non_send_resource(script);
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"AUTOEQUIP_BIND\"), 2)")
            .unwrap();
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            for a in s.take_pending_equip_answers() {
                s.run(&format!("CancelPendingEquip({})", a.index)).unwrap();
            }
        }
        crate::ui_items::calls::apply_kept_item_calls(app.world_mut());

        assert!(rx.try_iter().next().is_none(), "a cancel sends nothing");
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(
            script.cursor_item().map(|i| i.guid),
            Some(AXE_GUID),
            "the axe is still on the cursor"
        );
        assert!(!script.item_locked(AXE_GUID), "and free");
    }

    /// The `0x5ea930` conjunct, driven by its level leg.
    #[test]
    fn an_unusable_item_is_never_asked_about() {
        benilla_formats::wow_data_or_skip!();
        // A real player level (the fixture's 0 declines to judge) and a template it refuses.
        let (mut app, rx) = place_the_axe_after(2, 4, false, |s| {
            s.set_player_req_state(benilla_ui::script::PlayerReqState {
                level: 10,
                class_id: 1,
                race_id: 1,
                ..Default::default()
            });
            s.set_item_template(
                FLURRY_AXE,
                benilla_ui::script::ItemTemplateView {
                    bonding: 2,
                    required_level: 60,
                    allowable_class: -1,
                    allowable_race: -1,
                    ..Default::default()
                },
            );
        });
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "a level-60 axe on a level-10 player asks nothing"
        );
        assert!(
            rx.try_iter().next().is_some(),
            "it places, and the server refuses it — which is where that refusal belongs"
        );
    }

    /// An app with what the bag's use arm reads, the cast ladder's included.
    fn use_arm_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<crate::spell::ScriptCastPermit>()
            .add_message::<crate::sound::AutoEquipSound>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_bank::BankOpen>()
            .init_resource::<crate::ui_item_text::ItemTextOpen>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<crate::ui_loot::LootLatch>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::spell::cast_target::AutoSelfCast>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::player::Player>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_cast::CastReports>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::creature_anim::AttackStopInFlight>()
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<crate::ui_tradeskill::TradeSkillOpens>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        (app, rx)
    }

    /// `UseContainerItem(bag, slot, onSelf)` (`0x4fa17d`): a set flag makes the player's guid the
    /// use's target (`0x4fa186`, `0x468550`) in place of the selection `[0xb4e2d8]`.
    #[test]
    fn use_container_item_on_self_targets_the_player() {
        const ME: u64 = 0x0000_0000_0000_0007;
        const FRIEND: u64 = 0x0000_0000_0000_0009;
        let target = |chunk: &str| {
            let (mut app, rx) = use_arm_app();
            app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
            app.world_mut()
                .resource_mut::<crate::target::Selection>()
                .guid = Some(FRIEND);
            app.world_mut().spawn((
                SelfPlayer,
                // `UNIT_FIELD_HEALTH` (22): alive.
                ObjectStore(ObjectFields::from_pairs(&[
                    (22, 100),
                    (F_PACK_SLOT_1, AXE_GUID as u32),
                    (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
                ])),
            ));
            crate::items::test_spawn_item(
                app.world_mut(),
                AXE_GUID,
                ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
                false,
            );
            let mut template = crate::items::test_template("A Bandage");
            template.inventory_type = 0;
            template.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
                spell_id: 4321,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            });
            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(FLURRY_AXE, Some(template));
            let mut script = harness_vm();
            script.set_container(0, Some(bag_with_the_axe(1)));
            script.run(chunk).unwrap();
            app.insert_non_send_resource(script);
            run_container_clicks(app.world_mut());
            rx.try_iter()
                .find_map(|c| match c {
                    ClientCommand::UseItem { target, .. } => Some(target),
                    _ => None,
                })
                .expect(chunk)
        };
        use benilla_protocol::messages::UseItemTarget;
        assert_eq!(
            target("UseContainerItem(0, 1)"),
            UseItemTarget::Unit(FRIEND)
        );
        assert_eq!(
            target("UseContainerItem(0, 1, nil)"),
            UseItemTarget::Unit(FRIEND)
        );
        assert_eq!(target("UseContainerItem(0, 1, 1)"), UseItemTarget::Unit(ME));
    }

    /// Right-clicks a bind-on-use item in a bag (the use arm, `0x5d8d00`, event 290);
    /// `no_use_spell` drops its on-use spell, since four of `0x5d91d3`'s five predecessors are
    /// failed spell lookups and the reference still asks.
    fn right_click_a_bind_on_use_item(
        no_use_spell: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut app, rx) = use_arm_app();
        app.world_mut().spawn((
            SelfPlayer,
            // `UNIT_FIELD_HEALTH` (22): alive.
            ObjectStore(ObjectFields::from_pairs(&[
                (22, 100),
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        let mut template = crate::items::test_template("A Bind-On-Use Thing");
        template.bonding = 3;
        template.quality = 3;
        template.inventory_type = 0; // not equippable, so the equip fork above cannot claim it
        if !no_use_spell {
            template.spells = vec![benilla_protocol::messages::ItemSpellEntry {
                index: 0,
                spell_id: 4321,
                trigger: 0, // ON_USE
                charges: 0,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            }];
            template.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
                spell_id: 4321,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            });
        }
        items.insert_template(FLURRY_AXE, Some(template));

        let mut script = harness_vm();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(3)));
        script.run("UseContainerItem(0, 1)").unwrap();
        app.insert_non_send_resource(script);
        run_container_clicks(app.world_mut());
        (app, rx)
    }

    #[test]
    fn right_clicking_a_bind_on_use_item_asks_before_using_it() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(false);
        assert!(
            rx.try_iter().next().is_none(),
            "the deferred use sends nothing"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "USE_BIND"), "USE_BIND is raised");
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<String>(
                    "return getglobal(StaticPopup_FindVisible(\"USE_BIND\"):GetName() .. \"Text\"):GetText()"
                )
                .unwrap(),
            "Using this item will bind it to you.",
            "the real GlobalStrings USE_NO_DROP"
        );

        // Accept → ConfirmBindOnUse() → the use is re-issued with suppress set.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"USE_BIND\"), 1)")
            .unwrap();
        crate::ui_items::calls::apply_kept_calls(app.world_mut());
        assert!(
            rx.try_iter().count() > 0,
            "the accept re-issues the use rather than sending a confirm packet"
        );
    }

    /// The question the call raised is filed whatever the slot holds by the time it applies:
    /// the reference asked inside the call (`0x5e16c6`), the item is on the cursor, and the answer
    /// re-judges the slot. A miss here used to unlock the item and leave it held with no record.
    #[test]
    fn an_asked_equip_is_filed_though_the_slot_moved_on() {
        benilla_formats::wow_data_or_skip!();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_bank::BankOpen>()
            .init_resource::<crate::ui_item_text::ItemTextOpen>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<crate::ui_loot::LootLatch>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::spell::cast_target::AutoSelfCast>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::player::Player>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_cast::CastReports>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::creature_anim::AttackStopInFlight>()
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<crate::ui_tradeskill::TradeSkillOpens>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                ObjectStore(ObjectFields::from_pairs(&[
                    (F_PACK_SLOT_1, AXE_GUID as u32),
                    (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
                ])),
            ))
            .id();
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
            false,
        );
        app.world_mut().resource_mut::<Items>().insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality: 4,
                bonding: 2,
                inventory_type: 13,
                ..crate::items::test_template("Flurry Axe")
            }),
        );
        let mut script = harness_vm();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        let mut bag = bag_with_the_axe(4);
        bag.slots.get_mut(&1).unwrap().use_fork = UseFork::Equip;
        bag.slots.get_mut(&1).unwrap().item_class = Some(2);
        script.set_container(0, Some(bag));
        script.seat_item_records();
        script.run("UseContainerItem(0, 1)").unwrap();
        app.insert_non_send_resource(script);
        // A move applied before the question's call: the slot is empty now.
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::default()));
        run_container_clicks(app.world_mut());

        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1,
            "the question is filed"
        );
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(script.cursor_item().map(|i| i.guid), Some(AXE_GUID));
        assert!(script.item_locked(AXE_GUID), "still locked, still held");
    }

    /// `ConfirmBindOnUse` spends its pending use whatever the protected-action gate says
    /// (`0x5d92c6`-`0x5d92cc` after `CGItem::Use`), but the cast inside asks tag 2: a macro's
    /// accept outside an input event sends nothing, and the pending use is gone for the next.
    #[test]
    fn a_refused_bind_on_use_confirm_spends_the_pending_use_and_sends_nothing() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(false);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "USE_BIND"));
        let _ = rx.try_iter().count();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(r#"RunScript("ConfirmBindOnUse()")"#)
            .unwrap();
        crate::ui_items::calls::apply_kept_calls(app.world_mut());
        let sent: Vec<String> = rx.try_iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(sent, Vec::<String>::new(), "the cast is refused");
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("ConfirmBindOnUse()")
            .unwrap();
        crate::ui_items::calls::apply_kept_calls(app.world_mut());
        assert_eq!(rx.try_iter().count(), 0, "the pending use was spent");
    }

    /// The confirm's re-issue goes through `CGItem::Use` (`0x5d92c1`), which refuses an item
    /// locked meanwhile (`0x5d8d32`).
    #[test]
    fn a_bind_on_use_confirm_skips_an_item_locked_meanwhile() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(false);
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.tick(0.01);
            s.lock_item(AXE_GUID);
            s.run("StaticPopup_OnClick(StaticPopup_FindVisible(\"USE_BIND\"), 1)")
                .unwrap();
        }
        crate::ui_items::calls::apply_kept_calls(app.world_mut());
        assert_eq!(rx.try_iter().count(), 0, "a locked item is not used");
    }

    #[test]
    fn a_bind_on_use_item_with_no_on_use_spell_still_asks() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(true);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "USE_BIND"),
            "the reference's bind arm sits BELOW the on-use lookup's failure exits, not above them"
        );
        assert!(rx.try_iter().next().is_none());
    }
}
