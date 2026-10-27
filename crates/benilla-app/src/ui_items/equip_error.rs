//! The inventory refusals: `SMSG_INVENTORY_CHANGE_FAILURE`'s line and unlocks, fired on a lane of
//! their own at the packet's place ([`EquipErrors`]), and the wire `InventoryResult` to the
//! `GlobalStrings` key of its red error line.
//!
//! Reason 59 (`EQUIP_ERR_NONE`) only clears a pending lock (`ItemHandler.cpp:865`, `:963`), and
//! after a failed item use rides beside the real refusal (`SpellHandler.cpp:130`). The reference
//! maps it like any reason (handler `0x5e3991`, table `0x622794`, registry `0xb4b498`,
//! `DisplayError` `0x496720`) to a key with no 1.12 string, which the sink's empty-string guard
//! (`0x4945b4`) drops. Reason 0, the one coded suppression (`0x5e39a9`), is dropped earlier, by
//! `benilla_protocol::events`.

use std::collections::VecDeque;

use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::items::Items;
use crate::lanes::{Stamp, Until};
use crate::net::{ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;

use super::{BAGS, BAG_SLOT_FIRST, BANK_BAGS, BANK_BAG_SLOT_FIRST};

/// One inventory refusal off the wire, with the fills of the only two reasons that format one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EquipError {
    /// The wire `InventoryResult`.
    pub reason: u8,
    /// Reason 1's `%d`, the packet's `requiredLevel`.
    pub required_level: Option<u32>,
    /// Reason 16's `%s`: the destination bag's `BagFamily` name, read inside the handler as its
    /// `0x5ede00` reads the bag (`0x5e3a1e`), or `None` for the generic line.
    pub bag_family: Option<String>,
    /// The item guids a refused inventory operation unlocks after its line (`0x5e3ac0`,
    /// `0x5e3ad3`); none for a mail refusal.
    pub unlocks: [u64; 2],
}

/// The refusals the handlers queue, each stamped as its handler queued it
/// ([`crate::lanes::stamp`]): the lane that shows each line and runs its unlocks at its packet's
/// place, as the failure's handler does inside its dispatch (`0x496720`, then `0x5e3ac0`,
/// `0x5e3ad3`).
#[derive(Resource, Default)]
pub(crate) struct EquipErrors(VecDeque<(Stamp, EquipError)>);

impl EquipErrors {
    /// Queue `refusal` behind everything stamped before it.
    pub(crate) fn push(&mut self, refusal: EquipError) {
        let stamp = crate::lanes::stamp();
        let at = self.0.partition_point(|(s, _)| *s < stamp);
        self.0.insert(at, (stamp, refusal));
    }

    /// Everything queued, dropped.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    /// The refusals queued, in order.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> Vec<EquipError> {
        self.0.iter().map(|(_, e)| e.clone()).collect()
    }

    fn peek(&self) -> Option<Stamp> {
        self.0.front().map(|(s, _)| *s)
    }
}

/// The lane and its drop: [`drop_equip_errors`] under the exact complement of the drain's gate, so
/// a refusal is fired or dropped the frame it lands, never to fire into a later interface.
pub(crate) fn add_equip_error_lane(app: &mut App) {
    use crate::lanes::LaneApp;
    app.init_resource::<EquipErrors>()
        .add_lane(
            "equip errors",
            |world| world.get_resource::<EquipErrors>()?.peek(),
            fire_equip_errors,
            None,
        )
        .add_systems(
            Update,
            drop_equip_errors
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_equip_errors(mut errors: ResMut<EquipErrors>) {
    errors.clear();
}

/// What a refusal's handler reads for its line: the player, the bag it names and that bag's
/// family.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RefusalReads<'w, 's> {
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
    // Reason 16's `%s` source; absent, every 16 keeps the generic line.
    bag_families: Option<Res<'w, super::ItemBagFamilies>>,
}

impl RefusalReads<'_, '_> {
    /// Reason 16's `%s`, the bag at player slot `bag_slot` named by its family as the packet
    /// leaves it ([`bag_family_name`]); `None` for any other reason.
    pub(crate) fn bag_family(&self, reason: u8, bag_slot: u8) -> Option<String> {
        (reason == 16)
            .then(|| {
                bag_family_name(
                    self.me.iter().next(),
                    bag_slot,
                    &self.objects,
                    &self.items,
                    self.bag_families.as_deref().map(|c| &c.0),
                )
            })
            .flatten()
    }
}

/// The lane's `fire_until` ([`crate::lanes`]): each refusal `until` admits, its Lua run with the
/// host lent; the lane stops after one whose firing stamped anything, so what it pushed fires next.
fn fire_equip_errors(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut errors: ResMut<EquipErrors>,
    mut sink: crate::ui_action::MessageSink,
) {
    while let Some(stamp) = errors.peek().filter(|&s| until.admits(s)) {
        let Some((_, refusal)) = errors.0.pop_front() else {
            return;
        };
        let ((), moved) = crate::lanes::firing(stamp, || {
            vm.run(|script| {
                if let Some(script) = script {
                    show_refusal(script, &mut sink, refusal);
                }
            })
        });
        if moved {
            return;
        }
    }
}

/// One refusal as the failure's handler shows it: its line through the one sink, which also plays
/// the error speech 18 of these keys carry, then its unlocks (`0x496720`, then `0x5e3ac0`,
/// `0x5e3ad3`). An empty or missing string prints nothing, as the reference's sink guard
/// (`0x4945b4`) drops it; that silences 59.
fn show_refusal(
    script: &mut UiScript,
    sink: &mut crate::ui_action::MessageSink,
    refusal: EquipError,
) {
    // Reason 16 names the bag's family when the bag resolved, as `0x5ede00` does.
    let subclass_fill = refusal.bag_family;
    let key = match subclass_fill {
        Some(_) => "ERR_WRONG_BAG_TYPE_SUBCLASS",
        None => equip_error_key(refusal.reason),
    };
    let text = script
        .lua()
        .globals()
        .get::<String>(key)
        .unwrap_or_default();
    if !text.is_empty() {
        // Reason 1 fills `%d` and 16 `%s`; neither carries the other's fill.
        let text = match refusal.required_level {
            Some(d) => text.replace("%d", &d.to_string()),
            None => text,
        };
        let text = match subclass_fill {
            Some(family) => text.replace("%s", &family),
            None => text,
        };
        crate::ui_action::show_messages(
            script,
            sink,
            "ui_items",
            vec![crate::ui_action::Shown::keyed(key, text)],
        );
    }
    for guid in refusal.unlocks {
        script.unlock_item(guid);
    }
}

/// Reason 16's `%s`, the target bag's `BagFamily` name ("Only Arrows can be placed in that."), or
/// `None` to keep the generic line. `bag_slot` is the bag's player-array slot, and the reference's
/// `0x5ede00` names nothing for 255 (the player's own array) or for a slot at or past
/// `[player+0x1d38]`, 113 for the local player, so equipped bags (19..22) and bank bags (63..68)
/// both resolve. It reads the bag family, not the subclass the errorId's name suggests.
fn bag_family_name(
    player: Option<&ObjectStore>,
    bag_slot: u8,
    objects: &Objects,
    items: &Items,
    families: Option<&benilla_formats::ItemBagFamilyCatalog>,
) -> Option<String> {
    let store = player?;
    let families = families?;
    let guid = match bag_slot {
        s if (BAG_SLOT_FIRST..BAG_SLOT_FIRST + BAGS).contains(&s) => store.0.player_inv_slot(s),
        s if (BANK_BAG_SLOT_FIRST..BANK_BAG_SLOT_FIRST + BANK_BAGS).contains(&s) => {
            store.0.player_bank_bag_slot(s - BANK_BAG_SLOT_FIRST)
        }
        // 255, the player's own array, names no bag.
        _ => None,
    }
    .filter(|&g| g != 0)?;
    let entry = objects.object(guid)?.object_entry().filter(|&e| e != 0)?;
    // Only read (`0x5da08d`).
    let family = items.template(entry, guid, Ask::Read)?.bag_family;
    families.name(family).map(str::to_string)
}

/// `0x622630` on a whole inventory result: past 66 its default, errorId 9 (`0x62278d`); 0 the
/// table's 0x1d1, which the display refuses (`0x49672c`); the rest the table's.
pub(crate) fn inventory_result_key(result: u32) -> Option<&'static str> {
    match result {
        0 => None,
        r if r > 66 => Some("ERR_BAG_FULL"),
        r => Some(equip_error_key(r as u8)),
    }
}

/// A refusal reason's key, total like the reference's errorId table (`0x622794`). Positions are
/// vmangos `ItemDefines.h` with every band up to 5875 (stunned 37, dead 38, inventory full 50).
/// Reason 1's string carries a `%d` for the required level.
pub(crate) fn equip_error_key(reason: u8) -> &'static str {
    match reason {
        1 => "ERR_CANT_EQUIP_LEVEL_I",
        2 => "ERR_CANT_EQUIP_SKILL",
        3 => "ERR_WRONG_SLOT",
        // BAG_FULL and its BAG_FULL3/4/6 aliases; 51 is not one.
        4 | 53 | 56 | 62 => "ERR_BAG_FULL",
        5 => "ERR_BAG_IN_BAG",
        6 => "ERR_TRADE_EQUIPPED_BAG",
        7 => "ERR_AMMO_ONLY",
        8 => "ERR_PROFICIENCY_NEEDED",
        9 | 12 | 18 => "ERR_NO_SLOT_AVAILABLE",
        10 | 11 => "ERR_CANT_EQUIP_EVER",
        13 => "ERR_2HANDED_EQUIPPED",
        14 => "ERR_2HSKILLNOTFOUND",
        15 | 16 => "ERR_WRONG_BAG_TYPE",
        17 => "ERR_ITEM_MAX_COUNT",
        19 | 55 => "ERR_CANT_STACK",
        20 => "ERR_NOT_EQUIPPABLE",
        21 => "ERR_CANT_SWAP",
        22 => "ERR_SLOT_EMPTY",
        23 | 54 => "ERR_ITEM_NOT_FOUND",
        24 => "ERR_DROP_BOUND_ITEM",
        25 => "ERR_OUT_OF_RANGE",
        26 => "ERR_TOO_FEW_TO_SPLIT",
        27 => "ERR_SPLIT_FAILED",
        28 => "ERR_SPELL_FAILED_REAGENTS_GENERIC",
        29 => "ERR_NOT_ENOUGH_MONEY",
        30 => "ERR_NOT_A_BAG",
        31 => "ERR_DESTROY_NONEMPTY_BAG",
        32 => "ERR_NOT_OWNER",
        33 => "ERR_ONLY_ONE_QUIVER",
        34 => "ERR_NO_BANK_SLOT",
        35 => "ERR_NO_BANK_HERE",
        36 => "ERR_ITEM_LOCKED",
        37 => "ERR_GENERIC_STUNNED",
        38 => "ERR_PLAYER_DEAD",
        39 => "ERR_CLIENT_LOCKED_OUT",
        40 => "ERR_INTERNAL_BAG_ERROR",
        // ERR_ONLY_ONE_BOLT's 1.12 string also says "quiver".
        41 => "ERR_ONLY_ONE_BOLT",
        42 => "ERR_ONLY_ONE_AMMO",
        43 => "ERR_CANT_WRAP_STACKABLE",
        44 => "ERR_CANT_WRAP_EQUIPPED",
        45 => "ERR_CANT_WRAP_WRAPPED",
        46 => "ERR_CANT_WRAP_BOUND",
        47 => "ERR_CANT_WRAP_UNIQUE",
        48 => "ERR_CANT_WRAP_BAGS",
        49 => "ERR_LOOT_GONE",
        50 => "ERR_INV_FULL",
        // EQUIP_ERR_BANK_FULL: the reference sets errorId 1 (`0x622661`), `ERR_BANK_FULL` (init
        // `0x484cda`, key `0x842180`), though vmangos tags it `ERR_BAG_FULL`.
        51 => "ERR_BANK_FULL",
        52 | 57 => "ERR_VENDOR_SOLD_OUT",
        58 => "ERR_OBJECT_IS_BUSY",
        // EQUIP_ERR_NONE, the lock-clear sentinel: a real key with no 1.12 string.
        59 => "ERR_CANT_BE_DISENCHANTED",
        60 => "ERR_NOT_IN_COMBAT",
        61 => "ERR_NOT_WHILE_DISARMED",
        63 => "ERR_CANT_EQUIP_RANK",
        64 => "ERR_CANT_EQUIP_REPUTATION",
        65 => "ERR_TOO_MANY_SPECIAL_BAGS",
        66 => "ERR_LOOT_CANT_LOOT_THAT_NOW",
        // The reference's default past 66 (`0x62278d`): errorId 9, `ERR_BAG_FULL`.
        _ => "ERR_BAG_FULL",
    }
}

#[cfg(test)]
mod tests {
    use super::equip_error_key;

    /// Stunned and dead are 37 and 38 at 5875; 39 and 40 are "right now" and the bag error.
    #[test]
    fn equip_error_table_matches_the_5875_enum() {
        assert_eq!(equip_error_key(50), "ERR_INV_FULL");
        assert_eq!(equip_error_key(37), "ERR_GENERIC_STUNNED");
        assert_eq!(equip_error_key(38), "ERR_PLAYER_DEAD");
        assert_eq!(equip_error_key(39), "ERR_CLIENT_LOCKED_OUT");
        assert_eq!(equip_error_key(40), "ERR_INTERNAL_BAG_ERROR");
        assert_eq!(equip_error_key(1), "ERR_CANT_EQUIP_LEVEL_I");
    }

    /// The reference's table maps 51 to `ERR_BANK_FULL`, against vmangos's `ItemDefines.h` tag.
    #[test]
    fn a_full_bank_says_bank_not_bag() {
        assert_eq!(equip_error_key(51), "ERR_BANK_FULL");
        for alias in [4u8, 53, 56, 62] {
            assert_eq!(equip_error_key(alias), "ERR_BAG_FULL", "reason {alias}");
        }
    }

    /// A mount used in cat form answers `SPELL_FAILED_NO_ITEMS_WHILE_SHAPESHIFTED` and reason 59
    /// (`SpellHandler.cpp:130`); 59 must print nothing, or the refusal shows twice.
    #[test]
    fn the_lock_clear_sentinel_maps_to_the_stringless_key() {
        assert_eq!(equip_error_key(59), "ERR_CANT_BE_DISENCHANTED");
    }

    /// The drain swaps 16's line for the bag-family one when the bag resolves.
    #[test]
    fn the_wrong_bag_reasons_share_the_generic_key() {
        assert_eq!(equip_error_key(15), "ERR_WRONG_BAG_TYPE");
        assert_eq!(equip_error_key(16), "ERR_WRONG_BAG_TYPE");
    }

    #[test]
    fn codes_past_the_enum_clamp_to_the_bag_full_default() {
        assert_eq!(equip_error_key(67), "ERR_BAG_FULL");
        assert_eq!(equip_error_key(0xFF), "ERR_BAG_FULL");
    }

    /// Every key resolves in the shipped `GlobalStrings.lua` except reason 59's, which must not: a
    /// typo'd key would swallow a refusal in silence. Skips without client data.
    #[test]
    fn every_equip_error_key_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        // Reason 0 never reaches this table (gated at `events::decode`).
        for reason in 1..=66u8 {
            let key = equip_error_key(reason);
            let text = g(key).unwrap_or_default();
            if reason == 59 {
                // Empty, which the drain's `is_empty()` skip drops as `0x4945b4`'s guard does.
                assert!(
                    text.is_empty(),
                    "reason 59's {key} resolved to {text:?} — the duplicate line is back"
                );
                continue;
            }
            assert!(!text.is_empty(), "{key} (reason {reason}) missing");
        }
        assert_eq!(g(equip_error_key(50)).unwrap(), "Inventory is full.");
        assert_eq!(
            g(equip_error_key(1)).unwrap().replace("%d", "30"),
            "You must reach level 30 to use that item."
        );
        assert_eq!(g(equip_error_key(51)).unwrap(), "Your bank is full");
        assert_eq!(g(equip_error_key(67)).unwrap(), "That bag is full.");
        // Reason 16's generic line, and the bag-family one the drain substitutes.
        assert_eq!(
            g(equip_error_key(16)).unwrap(),
            "That item doesn't go in that container."
        );
        assert_eq!(
            g("ERR_WRONG_BAG_TYPE_SUBCLASS")
                .unwrap()
                .replace("%s", "Arrows"),
            "Only Arrows can be placed in that."
        );
    }
}
