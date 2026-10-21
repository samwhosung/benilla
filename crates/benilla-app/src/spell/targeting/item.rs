//! The item seam, `0x495d60`, and its two confirm popups. The bag click (`PickupContainerItem
//! 0x4f9b30`) and the paper-doll click (`0x4c7300`) both call it when `IsTargeting` and
//! `TargetingWantsItem 0x6e6330` hold, and pick nothing up; the VM half of that reroute is in
//! `benilla_ui`'s cursor seam, which hands the pick to the call host, so the gate binds, confirms
//! or refuses before the verb returns ([`crate::script_calls`]).

use bevy::prelude::*;

use super::TargetingWants;
use crate::query_cache::Ask;

/// The item guid a confirm popup stands over (`0xb4e3c0`), written by both confirm exits of
/// `0x495d60`. The reference never clears it, since `BindTarget 0x6e5b40` re-tests the word
/// before binding; here every reader goes through [`SpellTargeting::pending_for`], so a stale guid
/// is inert.
#[derive(Resource, Default)]
pub(crate) struct EnchantConfirmItem(Option<u64>);

/// The clicked item as `0x495d60` reads it: template type fields, the rest off the item object.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClickedItem {
    /// Template Class, SubClass and InventoryType (`0x5d9f30`, `0x5d9f90`, `0x5d9ff0`, through the
    /// item cache `0x55ba30`).
    pub(crate) class: u32,
    pub(crate) subclass: u32,
    pub(crate) inventory_type: u32,
    /// `0x5da2c0`: `ITEM_FIELD_FLAGS & 1` (soulbound), or any of the seven enchantment slots names
    /// a row that [`benilla_formats::EnchantCatalog::binds_the_item`].
    pub(crate) already_bound: bool,
    /// `ITEM_FIELD_ENCHANTMENT` PERM (0) and TEMP (1); `None` for empty, a negative id, or an id
    /// with no `SpellItemEnchantment` row.
    pub(crate) existing_enchant: [Option<u32>; 2],
}

/// The enchant the cast would apply, `SpellItemEnchantment[EffectMiscValue]`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NewEnchant {
    pub(crate) id: u32,
    /// `Flags & 1`.
    pub(crate) binds: bool,
}

/// `0x495d60`'s verdict. Every exit but `Bind` returns before `BindTarget`, so the word and the
/// cursor survive both popups; a refusal's report cancels the cursor's spell (`0x6e2218`), which
/// drops it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ItemBind {
    /// `BindTarget 0x6e5b40`: bind the item and send.
    Bind,
    /// `0x6e1a00(spell, 0x0a)`, no packet: the line, `SPELLCAST_FAILED`, and the cursor down.
    Refuse(u8),
    /// Park the guid and fire `BIND_ENCHANT` (event 402, no args); its Yes, `BindEnchant
    /// 0x48d2e0`, re-runs the gate confirmed.
    ConfirmBind,
    /// Park the guid and fire `REPLACE_ENCHANT` (event 403, old name then new); its Yes,
    /// `ReplaceEnchant 0x48d300`, skips the gate and binds.
    ConfirmReplace { existing: u32, new: u32 },
}

/// `0x495d60`, run at the click, for an `ENCHANT_ITEM` (53) or `ENCHANT_ITEM_TEMPORARY` (54)
/// effect, in this order:
///
/// 1. The equip gate: with a nonzero `EquippedItemSubClassMask` the class must match and the
///    subclass bit be set; with a nonzero `EquippedItemInventoryTypeMask` the InventoryType bit
///    must be set. A miss is [`ItemBind::Refuse`], "Invalid target", no packet.
/// 2. The bind confirm: the new enchant's row binds, the item is not
///    [`ClickedItem::already_bound`], the confirmed flag is clear and the InventoryType is nonzero.
/// 3. The replace confirm: the slot is PERM for 53 and TEMP for 54, and both it and the new
///    enchant name a row. It ignores the confirmed flag, so the bind popup's Yes can raise it next.
///
/// A spell with no enchant effect (Disenchant, the `0x4000` lockbox openers) binds outright.
/// The reference walks all three effect slots; [`benilla_formats::SpellDisplay`] carries only
/// slot 0, and a formats test fails if any item-target row puts its enchant in slot 1 or 2.
pub(crate) fn item_bind_verdict(
    def: &benilla_formats::SpellDisplay,
    item: &ClickedItem,
    new: Option<NewEnchant>,
    confirmed: bool,
) -> ItemBind {
    let perm = def.effects[0] == benilla_formats::SPELL_EFFECT_ENCHANT_ITEM;
    if !perm && def.effects[0] != benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY {
        return ItemBind::Bind;
    }
    if def.equipped_item_subclass_mask != 0 {
        let class_ok = i64::from(item.class) == i64::from(def.equipped_item_class);
        let sub_ok =
            item.subclass < 32 && def.equipped_item_subclass_mask & (1u32 << item.subclass) != 0;
        if !(class_ok && sub_ok) {
            return ItemBind::Refuse(crate::spell::cast_target::ERR_INVALID_TARGET);
        }
    }
    if def.equipped_item_inventory_type_mask != 0
        && !(item.inventory_type < 32
            && def.equipped_item_inventory_type_mask & (1u32 << item.inventory_type) != 0)
    {
        return ItemBind::Refuse(crate::spell::cast_target::ERR_INVALID_TARGET);
    }
    if let Some(new) = new {
        if new.binds && !item.already_bound && !confirmed && item.inventory_type != 0 {
            return ItemBind::ConfirmBind;
        }
        if let Some(existing) = item.existing_enchant[usize::from(!perm)] {
            return ItemBind::ConfirmReplace {
                existing,
                new: new.id,
            };
        }
    }
    ItemBind::Bind
}

/// The confirm popup a pick or a Yes raised (`0x495d60`), signalled as its call returns, under
/// its input events.
pub(crate) enum ConfirmPopup {
    /// `BIND_ENCHANT` (event 402, no args, `0x4960a3`).
    Bind,
    /// `REPLACE_ENCHANT` (event 403, `0x4960e4`): the old enchant's name, then the new one's.
    Replace { existing: String, new: String },
}

impl ConfirmPopup {
    /// Signal the popup's event.
    pub(crate) fn fire(self, script: &mut benilla_ui::script::UiScript) {
        match self {
            ConfirmPopup::Bind => script.fire_event("BIND_ENCHANT", vec![]),
            ConfirmPopup::Replace { existing, new } => script.fire_event(
                "REPLACE_ENCHANT",
                vec![
                    benilla_ui::script::ScriptValue::Str(existing),
                    benilla_ui::script::ScriptValue::Str(new),
                ],
            ),
        }
    }
}

/// A bag or paper-doll click while targeting wants an item, inside its call (`0x4f9c54`,
/// `0x4c76df`): resolve the clicked slot's item and run [`gate_item`] on it, unconfirmed. An empty
/// slot (`0x495d60`'s null-item guard) keeps the mode.
pub(crate) fn pick_item(
    ladder: &mut crate::spell::CastLadder,
    enchants: Option<&crate::items::Enchants>,
    parked: &mut EnchantConfirmItem,
    pick: benilla_ui::script::ItemPick,
) -> Option<ConfirmPopup> {
    let benilla_ui::script::ItemPick {
        bag,
        slot,
        may_cast,
    } = pick;
    let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
    let guid = ladder
        .objects
        .player_fields()
        .and_then(|store| crate::ui_items::slot_guid(store, bag, slot0, &ladder.objects));
    let Some(guid) = guid else {
        debug!("ui_action: item pick on an empty slot (bag {bag} slot {slot}) — mode kept");
        return None;
    };
    gate_item(ladder, enchants, parked, guid, false, may_cast)
}

/// A confirm popup's Yes on the parked item, inside its call: `BindEnchant 0x48d2e0` re-runs
/// `0x495d60` confirmed, so it may raise the replace popup next; `ReplaceEnchant 0x48d300` binds
/// through `0x6e5b40` directly, with no gate re-run.
pub(crate) fn confirm_enchant(
    ladder: &mut crate::spell::CastLadder,
    enchants: Option<&crate::items::Enchants>,
    parked: &mut EnchantConfirmItem,
    answer: benilla_ui::script::EnchantAnswer,
) -> Option<ConfirmPopup> {
    let item_guid = parked.0?;
    match answer.confirm {
        benilla_ui::script::EnchantConfirm::Bind => {
            gate_item(ladder, enchants, parked, item_guid, true, answer.may_cast)
        }
        benilla_ui::script::EnchantConfirm::Replace => {
            let (spell_id, commit) = ladder.ground.pending_for(TargetingWants::Item)?;
            if answer.may_cast {
                debug!(
                    "ui_action: replace-enchant accepted — cast {spell_id} at item {item_guid:#x}"
                );
                ladder.commit_targeted(
                    spell_id,
                    commit,
                    crate::spell::cast_send::TargetedBind::Item(item_guid),
                );
            }
            None
        }
    }
}

/// `ClickTargetTradeButton(7)`'s clash check `0x496170` (`check`), then the trade slot's bind
/// `0x6e6900`, which `ReplaceTradeEnchant` (`0x48d330`) runs alone. Both return at once while the
/// partner's slot 7 is empty (`[0xb7150c]`, `0x496175`, `0x6e6911`); `partner_enchant` is the
/// permanent enchant its record carries (`0xb715d8`).
///
/// The check reads the in-flight spell (`0x6e3d10`), which must have a `Spell.dbc` row
/// (`0x496185`-`0x4961a4`), and walks its three effects: the first `SPELL_EFFECT_ENCHANT_ITEM`
/// (53, `0x4961c0`) for which both the slot's enchant and the effect's `EffectMiscValue` name a
/// `SpellItemEnchantment` row signals `TRADE_REPLACE_ENCHANT` with the two names and binds
/// nothing (`0x496216`-`0x496230`); with none it tail-jumps to the bind (`0x496211`). The popup's
/// Yes is `ReplaceTradeEnchant`.
pub(crate) fn trade_enchant(
    ladder: &mut crate::spell::CastLadder,
    enchants: Option<&crate::items::Enchants>,
    script: &mut benilla_ui::script::UiScript,
    partner_enchant: Option<u32>,
    check: bool,
) {
    let Some(existing) = partner_enchant else {
        return;
    };
    if check {
        let Some(spell_id) = ladder.core().slot().map(|s| s.spell_id()) else {
            return;
        };
        let Some(def) = ladder
            .spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id))
        else {
            return;
        };
        // A row for an id, as `0x4961cd`-`0x4961fb` index the table: a negative id has none.
        let row = |id: i32| {
            u32::try_from(id)
                .ok()
                .filter(|&id| id != 0 && enchants.is_some_and(|c| c.0.has_row(id)))
        };
        let name = |id: u32| {
            enchants
                .and_then(|c| c.0.name(id))
                .unwrap_or_default()
                .to_string()
        };
        let clash = (0..3).find_map(|i| {
            (def.effects[i] == benilla_formats::SPELL_EFFECT_ENCHANT_ITEM)
                .then(|| row(existing as i32).zip(row(def.effect_misc_value[i])))
                .flatten()
        });
        if let Some((old, new)) = clash {
            debug!("ui_trade: trade slot enchant clash {old} -> {new}");
            script.signal_trade_replace_enchant(name(old), name(new));
            return;
        }
    }
    bind_trade_slot(ladder);
}

/// `0x6e6900`: a standing word that wants an item (`0x4010`, `0x6e6900`) binds the partner's slot
/// 7, item guid `{6, 0}` with mask bit `0x1000` (`0x6e6915`-`0x6e692f`), and sends the cast
/// (`0x6e694f`); the spell the send left in flight is then dropped with no packet and no failure
/// (`CancelCast(0, 0, 0x17)`, `0x6e695a`), since the server holds the cast until the trade is
/// accepted (vmangos `TradeHandler.cpp:347-365`).
fn bind_trade_slot(ladder: &mut crate::spell::CastLadder) {
    let Some((spell_id, commit)) = ladder.ground.pending_for(TargetingWants::Item) else {
        return;
    };
    debug!("ui_trade: cast {spell_id} at the partner's trade slot 7");
    ladder.commit_targeted(
        spell_id,
        commit,
        crate::spell::cast_send::TargetedBind::TradeItem(ENCHANT_TRADE_SLOT),
    );
    ladder.core().cancel(false, false, 0x17);
}

/// The partner's non-traded slot as the trade bind names it, 0-based (`0x6e6925`).
const ENCHANT_TRADE_SLOT: u8 = 6;

/// `0x495d60` on `item_guid`: run [`item_bind_verdict`] and on a pass bind the item for `SendCast
/// 0x6e54f0` (`CMSG_CAST_SPELL` for an enchant, `CMSG_USE_ITEM` for a poison bottle). A confirm
/// parks the guid and keeps the mode; a refusal ends it. The bind is `BindSpellTargetObject
/// 0x6e5b40`, which asks the protected-action gate's tag 2 (`may_cast`) first (`0x6e5b9e`), after
/// every validation above it has run and shown its line or popup.
fn gate_item(
    ladder: &mut crate::spell::CastLadder,
    enchants: Option<&crate::items::Enchants>,
    parked: &mut EnchantConfirmItem,
    item_guid: u64,
    confirmed: bool,
    may_cast: bool,
) -> Option<ConfirmPopup> {
    let Some((spell_id, commit)) = ladder.ground.pending_for(TargetingWants::Item) else {
        return None; // a click raced a cancel
    };
    // An unresolved template binds ungated and the server judges.
    let entry = ladder
        .objects
        .object(item_guid)
        .and_then(|o| o.object_entry());
    let clicked = match entry {
        // Only read (`0x495f76`).
        Some(entry) => ladder
            .items
            .template(entry, item_guid, Ask::Read)
            .map(|t| (t.class, t.subclass, t.inventory_type)),
        None => None,
    };
    let def = ladder
        .spells
        .as_deref()
        .and_then(|s| s.catalog.get(spell_id));
    let (Some(def), Some((class, subclass, inventory_type))) = (def, clicked) else {
        debug!("ui_action: item pick — cast {spell_id} at item {item_guid:#x} (ungated)");
        if may_cast {
            ladder.commit_targeted(
                spell_id,
                commit,
                crate::spell::cast_send::TargetedBind::Item(item_guid),
            );
        }
        return None;
    };
    let fields = ladder.objects.object(item_guid);
    let item = ClickedItem {
        class,
        subclass,
        inventory_type,
        // `0x5da2c0`, the same predicate as the tooltip's Soulbound line.
        already_bound: fields.is_some_and(|f| crate::items::already_bound(f, enchants)),
        existing_enchant: [0u8, 1]
            .map(|slot| fields.and_then(|f| crate::items::live_enchant(f, slot, enchants))),
    };
    // `EffectMiscValue` is signed; the reference rejects a negative one before the row lookup.
    let new = u32::try_from(def.effect_misc_value[0])
        .ok()
        .filter(|&id| enchants.is_some_and(|c| c.0.has_row(id)))
        .map(|id| NewEnchant {
            id,
            binds: crate::items::binds(id, enchants),
        });
    match item_bind_verdict(def, &item, new, confirmed) {
        ItemBind::Refuse(reason) => {
            // The validator reports for the cursor's spell (`0x496068`-`0x496152`, flag 0), whose
            // tail cancels it in flight (`0x6e2218`): the cursor drops.
            debug!("ui_action: cast {spell_id} refused at the item bind ({reason:#x})");
            ladder.report_local(crate::ui_action::CastFail::local(spell_id, reason));
            None
        }
        ItemBind::ConfirmBind => {
            debug!("ui_action: item bind confirm for {spell_id} on item {item_guid:#x}");
            parked.0 = Some(item_guid);
            Some(ConfirmPopup::Bind)
        }
        ItemBind::ConfirmReplace { existing, new } => {
            // Old name first, then new: "Do you want to replace %s with %s?".
            let name = |id: u32| {
                enchants
                    .and_then(|c| c.0.name(id))
                    .unwrap_or_default()
                    .to_string()
            };
            debug!("ui_action: replace-enchant confirm {existing} -> {new} on {item_guid:#x}");
            parked.0 = Some(item_guid);
            Some(ConfirmPopup::Replace {
                existing: name(existing),
                new: name(new),
            })
        }
        ItemBind::Bind if !may_cast => {
            debug!("ui_action: item pick — cast {spell_id} refused by the protected-action gate");
            None
        }
        ItemBind::Bind => {
            debug!("ui_action: item pick — cast {spell_id} at item {item_guid:#x}");
            ladder.commit_targeted(
                spell_id,
                commit,
                crate::spell::cast_send::TargetedBind::Item(item_guid),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{ClientCommand, NetCommands, SelfPlayer};

    const ITEM: u64 = 0x4000_0000_0000_0123;
    const ENTRY: u32 = 777;
    const SPELL: u32 = 5555;

    /// A world with one carried item (`template` gives it a cached template and the spell a
    /// record, so the pick takes the verdict arm; without them it takes the ungated one), a VM
    /// whose bag shows it, and a spell awaiting an item.
    fn item_targeting(template: bool) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        world.spawn((
            SelfPlayer,
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (532, ITEM as u32),
                (533, (ITEM >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            world,
            ITEM,
            benilla_protocol::ObjectFields::from_pairs(&[(3, ENTRY)]),
            false,
        );
        if template {
            world
                .resource_mut::<crate::items::Items>()
                .insert_template(ENTRY, Some(crate::items::test_template("Rune")));
            world.insert_resource(crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(SPELL, benilla_formats::SpellDisplay::default())].into(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            });
        }
        world.resource_mut::<crate::spell::SpellTargeting>().enter(
            SPELL,
            crate::spell::CastCommit::Spell,
            0x0010,
        );
        let mut script = benilla_ui::script::UiScript::new().unwrap();
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            benilla_ui::script::ContainerSlot {
                item_id: ENTRY,
                guid: ITEM,
                count: 1,
                ..Default::default()
            },
        );
        script.set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                name: None,
                num_slots: 16,
                slots,
            }),
        );
        script.set_item_pick_armed(true);
        world.insert_non_send_resource(script);
        (app, rx)
    }

    /// Run `lua` in the VM with the call host lent; the item casts sent, and whether the spell
    /// still awaits its item.
    fn pick(
        app: &mut App,
        rx: &crossbeam_channel::Receiver<ClientCommand>,
        lua: &str,
    ) -> (usize, bool) {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").run(lua))
            .unwrap();
        let sent = rx
            .try_iter()
            .filter(|c| matches!(c, ClientCommand::CastSpellItem { .. }))
            .count();
        let waiting = app
            .world()
            .resource::<crate::spell::SpellTargeting>()
            .pending_for(TargetingWants::Item)
            .is_some();
        (sent, waiting)
    }

    /// The bind that ends `ValidateSpellItemTarget` is `BindSpellTargetObject`, which asks the
    /// gate's tag 2 first (`0x6e5b9e`): a click a macro made outside an input event binds nothing
    /// and leaves the cursor up, on the ungated arm (an unresolved template) and the verdict's.
    #[test]
    fn a_refused_item_pick_binds_nothing_and_keeps_the_cursor() {
        for template in [false, true] {
            let (mut app, rx) = item_targeting(template);
            assert_eq!(
                pick(&mut app, &rx, r#"RunScript("PickupContainerItem(0, 1)")"#),
                (0, true),
                "template {template}: refused"
            );
            assert_eq!(
                pick(&mut app, &rx, "PickupContainerItem(0, 1)"),
                (1, false),
                "template {template}: the client's own click binds"
            );
        }
    }

    /// The pick commits inside the click's call: `PickupContainerItem` (`0x4f9c54`) and
    /// `PickupInventoryItem` (`0x4c76df`) call `ValidateSpellItemTarget 0x495d60`, whose bind
    /// sends the cast before the verb returns, so the script's next line reads targeting over.
    #[test]
    fn an_item_pick_commits_inside_the_call() {
        for template in [false, true] {
            let (mut app, rx) = item_targeting(template);
            app.world_mut()
                .non_send_resource_mut::<benilla_ui::script::UiScript>()
                .set_spell_targeting(true);
            let after = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
                s.expect("a VM")
                    .eval::<Option<i64>>("PickupContainerItem(0, 1) return SpellIsTargeting()")
            })
            .unwrap();
            assert_eq!(after, None, "template {template}: targeting is over");
            let sent = rx
                .try_iter()
                .filter(|c| matches!(c, ClientCommand::CastSpellItem { .. }))
                .count();
            assert_eq!(sent, 1, "template {template}: sent inside the call");
        }
    }

    /// The bind confirm is signalled inside the click (`0x495d60` raises `BIND_ENCHANT` at
    /// `0x4960a3`), and its Yes reruns the gate inside `BindEnchant` (`0x48d2e0`), which binds and
    /// sends before it returns: one chunk hears the popup's event, answers it and reads
    /// targeting over.
    #[test]
    fn the_bind_confirm_and_its_yes_apply_inside_their_calls() {
        const ENCHANT: u32 = 1900;
        let (mut app, rx) = item_targeting(true);
        let world = app.world_mut();
        world.resource_mut::<crate::items::Items>().insert_template(
            ENTRY,
            Some(benilla_protocol::ItemInfo {
                inventory_type: 13,
                ..crate::items::test_template("Runed Blade")
            }),
        );
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(
                    SPELL,
                    benilla_formats::SpellDisplay {
                        effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM, 0, 0],
                        effect_misc_value: [ENCHANT as i32, 0, 0],
                        ..Default::default()
                    },
                )]
                .into(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        // A row whose `Flags & 1` binds the item.
        world.insert_resource(crate::items::Enchants(
            benilla_formats::EnchantCatalog::from_rows(
                Default::default(),
                Default::default(),
                [(ENCHANT, 1)].into(),
            ),
        ));
        let mut script = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
        script.set_spell_targeting(true);
        script
            .run(
                r#"local f = CreateFrame("Frame")
                f:RegisterEvent("BIND_ENCHANT")
                f:SetScript("OnEvent", function() Heard = (Heard or 0) + 1 end)"#,
            )
            .unwrap();
        let (heard, targeting) = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM").eval::<(Option<i64>, Option<i64>)>(
                "PickupContainerItem(0, 1) local heard = Heard BindEnchant() \
                 return heard, SpellIsTargeting()",
            )
        })
        .unwrap();
        assert_eq!(heard, Some(1), "BIND_ENCHANT fired inside the click");
        assert_eq!(targeting, None, "the Yes bound and sent inside its call");
        let sent = rx
            .try_iter()
            .filter(|c| matches!(c, ClientCommand::CastSpellItem { .. }))
            .count();
        assert_eq!(sent, 1);
    }

    /// `ReplaceEnchant` binds the parked item through `BindSpellTargetObject` directly (`0x48d300`
    /// → `0x6e5b40`), behind the same gate.
    #[test]
    fn a_refused_replace_enchant_binds_nothing() {
        let (mut app, rx) = item_targeting(true);
        app.world_mut().resource_mut::<EnchantConfirmItem>().0 = Some(ITEM);
        assert_eq!(
            pick(&mut app, &rx, r#"RunScript("ReplaceEnchant()")"#),
            (0, true)
        );
        assert_eq!(pick(&mut app, &rx, "ReplaceEnchant()"), (1, false));
    }

    #[test]
    fn the_item_bind_gate_mirrors_0x495d60() {
        use benilla_formats::SpellDisplay;
        fn item_target_refusal(
            def: &SpellDisplay,
            class: u32,
            subclass: u32,
            inventory_type: u32,
        ) -> Option<u8> {
            let item = ClickedItem {
                class,
                subclass,
                inventory_type,
                ..Default::default()
            };
            match item_bind_verdict(def, &item, None, false) {
                ItemBind::Refuse(code) => Some(code),
                ItemBind::Bind => None,
                other => panic!("no enchant row resolved — {other:?} is unreachable"),
            }
        }
        // Item classes/subclasses/inventory types, 1.12 values.
        const CLASS_WEAPON: u32 = 2;
        const CLASS_ARMOR: u32 = 4;
        const SUB_DAGGER: u32 = 15;
        const SUB_SHIELD: u32 = 6;
        const INVTYPE_WRIST: u32 = 9;
        const INVTYPE_CHEST: u32 = 5;
        const ENCHANT: u32 = benilla_formats::SPELL_EFFECT_ENCHANT_ITEM;

        // Enchant Bracer - Minor Health (7418): armor, any subclass, WRIST only.
        let bracer = SpellDisplay {
            effects: [ENCHANT, 0, 0],
            equipped_item_class: CLASS_ARMOR as i32,
            equipped_item_subclass_mask: 0x1f,
            equipped_item_inventory_type_mask: 1 << INVTYPE_WRIST,
            ..Default::default()
        };
        assert_eq!(
            item_target_refusal(&bracer, CLASS_ARMOR, 1, INVTYPE_WRIST),
            None,
            "a bracer takes the bracer enchant"
        );
        assert_eq!(
            item_target_refusal(&bracer, CLASS_ARMOR, 1, INVTYPE_CHEST),
            Some(crate::spell::cast_target::ERR_INVALID_TARGET),
            "the inventory-type leg (495e4d) refuses a chestpiece"
        );
        assert_eq!(
            item_target_refusal(&bracer, CLASS_WEAPON, 1, INVTYPE_WRIST),
            Some(crate::spell::cast_target::ERR_INVALID_TARGET),
            "the class leg (495e10) refuses a weapon"
        );

        // Instant Poison (8679): weapon class, a subclass mask, NO inventory-type requirement.
        let poison = SpellDisplay {
            effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, 0, 0],
            equipped_item_class: CLASS_WEAPON as i32,
            equipped_item_subclass_mask: 0x2a5f3,
            equipped_item_inventory_type_mask: 0,
            ..Default::default()
        };
        // Its real mask carries dagger (15), so a rogue's own weapon passes the subclass leg.
        assert_eq!(
            item_target_refusal(&poison, CLASS_WEAPON, SUB_DAGGER, 13),
            None
        );
        assert_eq!(
            item_target_refusal(&poison, CLASS_WEAPON, 1, 13),
            None,
            "and two-handed axe (1) is in it too — poisons are broad, the class leg is the fence"
        );
        assert_eq!(
            item_target_refusal(&poison, CLASS_ARMOR, SUB_SHIELD, 14),
            Some(crate::spell::cast_target::ERR_INVALID_TARGET),
            "a shield is armor — the class leg alone stops it"
        );

        // Disenchant (13262) has no enchant effect: anything binds and the server judges.
        let disenchant = SpellDisplay {
            effects: [99, 0, 0],
            equipped_item_class: -1,
            ..Default::default()
        };
        assert_eq!(
            item_target_refusal(&disenchant, CLASS_ARMOR, 1, INVTYPE_CHEST),
            None
        );
        assert_eq!(
            item_target_refusal(&disenchant, CLASS_WEAPON, SUB_DAGGER, 13),
            None
        );

        // A subclass past the mask's 32 bits is refused, not wrapped.
        assert_eq!(
            item_target_refusal(&poison, CLASS_WEAPON, 40, 13),
            Some(crate::spell::cast_target::ERR_INVALID_TARGET)
        );
    }

    /// The equip gate, then the bind confirm, then the replace confirm; the bind popup's Yes can
    /// raise the replace popup because only the bind confirm reads the confirmed flag.
    #[test]
    fn the_two_confirms_chain_in_the_references_order() {
        use benilla_formats::SpellDisplay;
        const ENCHANT: u32 = benilla_formats::SPELL_EFFECT_ENCHANT_ITEM;
        const TEMP: u32 = benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY;
        const INVTYPE_WEAPON: u32 = 13;

        // A permanent weapon enchant whose row binds the item (`Flags & 1`).
        let perm = SpellDisplay {
            effects: [ENCHANT, 0, 0],
            effect_misc_value: [1900, 0, 0],
            ..Default::default()
        };
        let binder = Some(NewEnchant {
            id: 1900,
            binds: true,
        });
        let plain = Some(NewEnchant {
            id: 1900,
            binds: false,
        });
        let bare = ClickedItem {
            inventory_type: INVTYPE_WEAPON,
            ..Default::default()
        };

        // 1. The bind confirm, and each of its four legs turned off in turn.
        assert_eq!(
            item_bind_verdict(&perm, &bare, binder, false),
            ItemBind::ConfirmBind
        );
        assert_eq!(
            item_bind_verdict(&perm, &bare, plain, false),
            ItemBind::Bind,
            "a row without the flag never asks (495ea3)"
        );
        assert_eq!(
            item_bind_verdict(&perm, &bare, None, false),
            ItemBind::Bind,
            "an EffectMiscValue naming no row never asks (495e9e)"
        );
        assert_eq!(
            item_bind_verdict(
                &perm,
                &ClickedItem {
                    already_bound: true,
                    ..bare
                },
                binder,
                false
            ),
            ItemBind::Bind,
            "0x5da2c0 says it is already bound (495eb3)"
        );
        assert_eq!(
            item_bind_verdict(
                &perm,
                &ClickedItem {
                    inventory_type: 0,
                    ..bare
                },
                binder,
                false
            ),
            ItemBind::Bind,
            "a non-equippable item — a lockbox, a reagent — is never asked (495ec6)"
        );

        // 2. The bind popup's Yes re-enters confirmed and reaches the replace question.
        let enchanted = ClickedItem {
            existing_enchant: [Some(2564), None],
            ..bare
        };
        assert_eq!(
            item_bind_verdict(&perm, &enchanted, binder, false),
            ItemBind::ConfirmBind,
            "the bind question comes first"
        );
        assert_eq!(
            item_bind_verdict(&perm, &enchanted, binder, true),
            ItemBind::ConfirmReplace {
                existing: 2564,
                new: 1900
            },
            "and its Yes lands on the replace question — the reference's two-popup chain"
        );

        // 3. The slot fork, the one place effects 53 and 54 differ: PERM for 53, TEMP for 54.
        let temp_spell = SpellDisplay {
            effects: [TEMP, 0, 0],
            effect_misc_value: [1900, 0, 0],
            ..Default::default()
        };
        assert_eq!(
            item_bind_verdict(&temp_spell, &enchanted, plain, false),
            ItemBind::Bind,
            "a poison over a permanently-enchanted weapon replaces nothing — different slot"
        );
        let poisoned = ClickedItem {
            existing_enchant: [None, Some(2564)],
            ..bare
        };
        assert_eq!(
            item_bind_verdict(&temp_spell, &poisoned, plain, false),
            ItemBind::ConfirmReplace {
                existing: 2564,
                new: 1900
            },
            "but over an already-poisoned one it does"
        );
        assert_eq!(
            item_bind_verdict(&perm, &poisoned, plain, false),
            ItemBind::Bind,
            "and the mirror: a permanent enchant ignores the temp slot"
        );

        // 4. The equip gate runs first: a refusal beats both confirms.
        let bracer_only = SpellDisplay {
            effects: [ENCHANT, 0, 0],
            effect_misc_value: [1900, 0, 0],
            equipped_item_class: 4,
            equipped_item_subclass_mask: 0x1f,
            equipped_item_inventory_type_mask: 1 << 9,
            ..Default::default()
        };
        assert_eq!(
            item_bind_verdict(&bracer_only, &enchanted, binder, false),
            ItemBind::Refuse(crate::spell::cast_target::ERR_INVALID_TARGET)
        );
    }
}
