//! The bag, cursor and paper-doll verbs inside their calls, through the whole client's call host:
//! each packet goes out before the verb returns, as the reference's bindings send in their own
//! bodies, so a chunk's packets leave in call order and its next line reads what the verb did.

use benilla_protocol::field::{FIELD_CONTAINER_SLOT_1, FIELD_OBJECT_ENTRY};
use benilla_protocol::messages::ItemInfo;
use benilla_protocol::ObjectFields;
use benilla_ui::script::{InvSlotView, InventorySlots, UiScript};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::items::{test_spawn_item, test_template, Items};
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::script_calls::with_hosted_vm;

const ME: u64 = 0x07;
const VENDOR: u64 = 0xF130_0000_0000_0D01;
/// `PLAYER_FIELD_INV_SLOT_HEAD`: the player array's slot `i` is the guid pair at `486 + 2i`.
const F_INV_SLOT_HEAD: u16 = 486;
/// `OBJECT_FIELD_TYPE`, and `CONTAINER_FIELD_NUM_SLOTS`.
const F_TYPE: u16 = 2;
const F_NUM_SLOTS: u16 = 48;

/// Linen Cloth, three copies: backpack slots 1, 2 and 3 (a stack of five), wire 23-25.
const LINEN: u32 = 2589;
const A: u64 = 0x4000_0000_0000_0A01;
const B: u64 = 0x4000_0000_0000_0A02;
const C: u64 = 0x4000_0000_0000_0A03;
/// A helm that binds on pickup in no way: backpack slot 4, wire 26.
const HELM: u32 = 1234;
const H: u64 = 0x4000_0000_0000_0A04;
/// Wrapping paper: backpack slot 6, wire 28.
const PAPER: u32 = 5042;
const P: u64 = 0x4000_0000_0000_0A06;
/// Flurry Axe's binding, on a helm: bind on equip, backpack slot 7, wire 29.
const BOE: u32 = 871;
const E: u64 = 0x4000_0000_0000_0A07;
/// A bind-on-use trinket with an on-use spell: backpack slot 8, wire 30.
const BOU: u32 = 9000;
const U: u64 = 0x4000_0000_0000_0A08;
/// The worn helm, doll slot 1 (wire 0).
const W: u64 = 0x4000_0000_0000_0A10;
/// The bag in doll slot 20 (wire 19), holding X in its slot 1.
const BAG_ENTRY: u32 = 4496;
const BAG: u64 = 0x4000_0000_0000_0B01;
const X: u64 = 0x4000_0000_0000_0A20;
/// The vendor's one row.
const ROW_ENTRY: u32 = 159;

/// Commands as their debug text, `ClientCommand` having no `PartialEq`.
fn text(commands: impl IntoIterator<Item = ClientCommand>) -> Vec<String> {
    commands.into_iter().map(|c| format!("{c:?}")).collect()
}

fn roll() -> String {
    format!("{:?}", ClientCommand::RandomRoll { min: 1, max: 100 })
}

/// The whole client, headless: the backpack, a bag in doll slot 20 and a worn helm, in the
/// player's descriptor, and the doll as the VM's snapshot holds it.
struct Bags {
    app: App,
    rx: Receiver<ClientCommand>,
}

impl Bags {
    fn new() -> Self {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        // `UNIT_FIELD_HEALTH` (22): alive.
        let mut pairs = vec![(22, 100)];
        let mut seat = |slot0: u16, guid: u64| {
            pairs.push((F_INV_SLOT_HEAD + 2 * slot0, guid as u32));
            pairs.push((F_INV_SLOT_HEAD + 2 * slot0 + 1, (guid >> 32) as u32));
        };
        seat(0, W);
        seat(19, BAG);
        for (i, guid) in [A, B, C, H, 0, P, E, U].into_iter().enumerate() {
            if guid != 0 {
                seat(23 + i as u16, guid);
            }
        }
        world.spawn((
            SelfPlayer,
            Guid(ME),
            ObjectStore(ObjectFields::from_pairs(&pairs)),
        ));
        for (guid, entry) in [
            (A, LINEN),
            (B, LINEN),
            (C, LINEN),
            (H, HELM),
            (P, PAPER),
            (E, BOE),
            (U, BOU),
            (W, HELM),
            (X, LINEN),
        ] {
            // `ITEM_FIELD_STACK_COUNT` (14): the linen in slot 3 is a stack of five.
            let count = if guid == C { 5 } else { 1 };
            test_spawn_item(
                world,
                guid,
                ObjectFields::from_pairs(&[
                    (F_TYPE, 0x3),
                    (FIELD_OBJECT_ENTRY, entry),
                    (14, count),
                ]),
                false,
            );
        }
        test_spawn_item(
            world,
            BAG,
            ObjectFields::from_pairs(&[
                (F_TYPE, 0x7),
                (FIELD_OBJECT_ENTRY, BAG_ENTRY),
                (F_NUM_SLOTS, 4),
                (FIELD_CONTAINER_SLOT_1, X as u32),
                (FIELD_CONTAINER_SLOT_1 + 1, (X >> 32) as u32),
            ]),
            true,
        );
        let use_spell = benilla_protocol::messages::ItemUseSpell {
            spell_id: 4321,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        };
        let mut items = world.resource_mut::<Items>();
        items.insert_template(LINEN, Some(test_template("Linen Cloth")));
        items.insert_template(
            HELM,
            Some(ItemInfo {
                inventory_type: 1,
                ..test_template("Helm")
            }),
        );
        items.insert_template(PAPER, Some(test_template("Wrapping Paper")));
        items.insert_template(
            BOE,
            Some(ItemInfo {
                inventory_type: 1,
                bonding: 2,
                ..test_template("Bound Helm")
            }),
        );
        items.insert_template(
            BOU,
            Some(ItemInfo {
                bonding: 3,
                spells: vec![benilla_protocol::messages::ItemSpellEntry {
                    index: 0,
                    spell_id: 4321,
                    trigger: 0,
                    charges: 0,
                    cooldown_ms: -1,
                    category: 0,
                    category_cooldown_ms: -1,
                }],
                use_spell: Some(use_spell),
                ..test_template("Trinket")
            }),
        );
        items.insert_template(BAG_ENTRY, Some(test_template("Linen Bag")));

        let mut script = UiScript::new().expect("a VM");
        let mut doll: InventorySlots = Default::default();
        doll[1] = Some(InvSlotView {
            item_id: HELM,
            guid: W,
            count: 1,
            equip_slots: vec![1],
            icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
            ..Default::default()
        });
        doll[20] = Some(InvSlotView {
            item_id: BAG_ENTRY,
            guid: BAG,
            count: 1,
            equip_slots: vec![20, 21, 22, 23],
            icon: Some("Interface\\Icons\\INV_Misc_Bag_09".into()),
            ..Default::default()
        });
        script.set_inventory_slots(doll);
        // The record feed's copies of the templates above, which the equip questions read.
        for (entry, bonding) in [
            (LINEN, 0),
            (HELM, 0),
            (PAPER, 0),
            (BOE, 2),
            (BOU, 3),
            (BAG_ENTRY, 0),
        ] {
            script.set_item_template(
                entry,
                benilla_ui::script::ItemTemplateView {
                    bonding,
                    ..Default::default()
                },
            );
        }
        world.insert_non_send_resource(script);
        Self { app, rx }
    }

    /// Open the vendor, its one row and its repair service, which the getters read through the
    /// host.
    fn open_vendor(&mut self) {
        let world = self.app.world_mut();
        world
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .open(
                VENDOR,
                vec![benilla_protocol::messages::VendorItem {
                    slot: 1,
                    entry: ROW_ENTRY,
                    display_id: 0,
                    current_count: u32::MAX,
                    price: 0,
                    max_durability: 0,
                    buy_count: 1,
                }],
            );
        let vendor = world
            .spawn((
                Guid(VENDOR),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                    benilla_protocol::field::FIELD_UNIT_NPC_FLAGS,
                    // `UNIT_NPC_FLAG_REPAIR`.
                    0x4000,
                )])),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(VENDOR, vendor);
    }

    /// Run `lua` with the host lent, as every entry that runs Lua lends it.
    fn run(&mut self, lua: &str) {
        with_hosted_vm(self.app.world_mut(), |s| s.expect("a VM").run(lua))
            .unwrap_or_else(|e| panic!("{lua}: {e}"));
    }

    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, lua: &str) -> T {
        with_hosted_vm(self.app.world_mut(), |s| s.expect("a VM").eval::<T>(lua))
            .unwrap_or_else(|e| panic!("{lua}: {e}"))
    }

    fn script(&mut self) -> Mut<'_, UiScript> {
        self.app.world_mut().non_send_resource_mut::<UiScript>()
    }

    /// What went on the wire, the name and item queries aside.
    fn sent(&self) -> Vec<String> {
        text(self.rx.try_iter().filter(|c| {
            !matches!(
                c,
                ClientCommand::ItemQuery { .. } | ClientCommand::NameQuery { .. }
            )
        }))
    }

    /// Run `setup`, then `verb` and a roll in one chunk: what the chunk sent.
    fn verb(&mut self, setup: &str, verb: &str) -> Vec<String> {
        self.run(setup);
        let _ = self.sent();
        self.run(&format!("{verb} RandomRoll(1, 100)"));
        self.sent()
    }
}

/// Each item verb sends inside its call, so a roll on the chunk's next line follows it on the
/// wire: the bag click's swap (`0x4f9eac`) and split (`0x4f9e43`), the doll click's swap
/// (`0x4c78c8`), `EquipCursorItem`'s (`0x489757`), `AutoEquipCursorItem` (`0x48a063`), the bag
/// worker's store (`0x4c7e49`), `DeleteCursorItem` (`0x489854`) and the wrap (`0x4f9db3`).
#[test]
fn each_item_verb_is_on_the_wire_before_the_next_line() {
    let cmd = |c: ClientCommand| format!("{c:?}");
    let cases = [
        (
            "PickupContainerItem(0, 1)",
            "PickupContainerItem(0, 2)",
            cmd(ClientCommand::SwapInvItem {
                src_slot: 23,
                dst_slot: 24,
            }),
        ),
        (
            "SplitContainerItem(0, 3, 2)",
            "PickupContainerItem(0, 9)",
            cmd(ClientCommand::SplitItem {
                src_bag: 255,
                src_slot: 25,
                dst_bag: 255,
                dst_slot: 31,
                count: 2,
            }),
        ),
        (
            "PickupContainerItem(0, 4)",
            "PickupInventoryItem(1)",
            cmd(ClientCommand::SwapInvItem {
                src_slot: 26,
                dst_slot: 0,
            }),
        ),
        (
            "PickupContainerItem(0, 4)",
            "EquipCursorItem(1)",
            cmd(ClientCommand::SwapInvItem {
                src_slot: 26,
                dst_slot: 0,
            }),
        ),
        (
            "PickupContainerItem(0, 4)",
            "AutoEquipCursorItem()",
            cmd(ClientCommand::AutoEquipItem {
                bag_index: 255,
                slot: 26,
            }),
        ),
        (
            "PickupContainerItem(0, 1)",
            "PutItemInBag(20)",
            cmd(ClientCommand::AutoStoreBagItem {
                src_bag: 255,
                src_slot: 23,
                dst_bag: 19,
            }),
        ),
        (
            "PickupContainerItem(1, 1)",
            "PutItemInBackpack()",
            cmd(ClientCommand::AutoStoreBagItem {
                src_bag: 19,
                src_slot: 0,
                dst_bag: 255,
            }),
        ),
        (
            "PickupContainerItem(0, 1)",
            "DeleteCursorItem()",
            cmd(ClientCommand::DestroyItem {
                bag_index: 255,
                slot: 23,
                count: 0,
            }),
        ),
    ];
    for (setup, verb, sends) in cases {
        let mut h = Bags::new();
        assert_eq!(h.verb(setup, verb), [sends, roll()], "{setup} {verb}");
    }
    let mut h = Bags::new();
    h.script().arm_gift_wrap(0, 6);
    assert_eq!(
        h.verb("", "PickupContainerItem(0, 1)"),
        [
            cmd(ClientCommand::WrapItem {
                gift_bag: 255,
                gift_slot: 28,
                item_bag: 255,
                item_slot: 23,
            }),
            roll()
        ]
    );
}

/// The repair cursor's four clicks send `CMSG_REPAIR_ITEM` inside their calls: the bag click's
/// (`0x4f9c7b`), the bag right-click's (`0x4fa1da`), the doll click's (`0x4c7714`) and the doll
/// right-click's (`0x4c79c4`).
#[test]
fn each_repair_click_is_on_the_wire_before_the_next_line() {
    for (verb, item_guid) in [
        ("PickupContainerItem(0, 1)", A),
        ("UseContainerItem(0, 1)", A),
        ("PickupInventoryItem(1)", W),
        ("UseInventoryItem(1)", W),
    ] {
        let mut h = Bags::new();
        h.open_vendor();
        assert_eq!(
            h.verb("ShowRepairCursor()", verb),
            [
                format!(
                    "{:?}",
                    ClientCommand::RepairItem {
                        vendor: VENDOR,
                        item_guid
                    }
                ),
                roll()
            ],
            "{verb}"
        );
    }
}

/// `EquipPendingItem` re-issues the parked swap inside its call (`0x4898f0` → `0x5e1be0`), and
/// `CancelPendingEquip` releases both ends' locks inside its own (`0x489960`, `0x5e1ca5`), so the
/// next line reads the item unlocked.
#[test]
fn the_bind_answers_act_inside_their_calls() {
    let mut h = Bags::new();
    h.run("PickupContainerItem(0, 7) PickupInventoryItem(1)");
    assert_eq!(h.sent(), Vec::<String>::new(), "the place is parked");
    assert_eq!(
        h.verb("", "EquipPendingItem(0)"),
        [
            format!(
                "{:?}",
                ClientCommand::SwapInvItem {
                    src_slot: 29,
                    dst_slot: 0
                }
            ),
            roll()
        ]
    );

    let mut h = Bags::new();
    h.run("PickupContainerItem(0, 7) PickupInventoryItem(1)");
    let locked: Option<i64> = h.eval(
        "CancelPendingEquip(0) local _, _, locked = GetContainerItemInfo(0, 7) return locked",
    );
    assert_eq!(
        locked, None,
        "the cancel unlocked the item before the next line"
    );
    assert_eq!(h.sent(), Vec::<String>::new(), "a cancel sends nothing");
}

/// An accepted pending swap sends `CMSG_SWAP_INV_ITEM` (`0x5e0fdb`) and only then clears the
/// cursor (`0x5e0fe7`), so the clear's handlers send after the packet. The send is handed over in
/// place, so the census counts no send made while a handler waits.
#[test]
fn an_accepted_swap_sends_before_its_clears_signals() {
    let mut h = Bags::new();
    h.run("PickupContainerItem(0, 7) PickupInventoryItem(1)");
    h.run(
        r#"local f = CreateFrame("Frame") f:RegisterAllEvents()
        f:SetScript("OnEvent", function()
            if not ROLLED then ROLLED = 1 RandomRoll(1, 100) end
        end)"#,
    );
    let _ = h.sent();
    benilla_ui::script::census::take();
    h.run("EquipPendingItem(0)");
    assert_eq!(
        h.sent(),
        [
            format!(
                "{:?}",
                ClientCommand::SwapInvItem {
                    src_slot: 29,
                    dst_slot: 0
                }
            ),
            roll()
        ],
        "the packet, then the clear's handler"
    );
    crate::script_calls::census::shows_no_send();
}

/// An auto-equip's bind answer re-issues the pick-up and equip from the slot as it stands
/// (`0x5e1c23` into `0x5e1840`), which it reads with the host applying the answer: the
/// bind-on-equip helm still in backpack slot 7 goes out from wire slot 29.
#[test]
fn an_auto_equips_accept_reads_the_slot_as_it_stands() {
    let mut h = Bags::new();
    h.run("UseContainerItem(0, 7)");
    assert_eq!(h.sent(), Vec::<String>::new(), "the equip is parked");
    assert_eq!(
        h.verb("", "EquipPendingItem(0)"),
        [
            format!(
                "{:?}",
                ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 29
                }
            ),
            roll()
        ]
    );
}

/// What `verb` sent with a listener armed that rolls 1-100 on `CURSOR_UPDATE` and 2-100 on
/// `ITEM_LOCK_CHANGED`, after `setup`: each command by its kind, a listener's roll by its event.
fn heard_around(h: &mut Bags, setup: &str, verb: &str) -> Vec<&'static str> {
    h.run(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("CURSOR_UPDATE") f:RegisterEvent("ITEM_LOCK_CHANGED")
        f:SetScript("OnEvent", function()
            if ARMED then RandomRoll(event == "CURSOR_UPDATE" and 1 or 2, 100) end
        end)"#,
    );
    h.run(setup);
    let _ = h.sent();
    h.run(&format!("ARMED = true {verb} ARMED = nil"));
    h.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::RandomRoll { min: 1, .. } => Some("cursor"),
            ClientCommand::RandomRoll { .. } => Some("lock"),
            ClientCommand::AutoEquipItem { .. } => Some("equip"),
            ClientCommand::SwapInvItem { .. } => Some("swap"),
            ClientCommand::DestroyItem { .. } => Some("destroy"),
            ClientCommand::SplitItem { .. } => Some("split"),
            ClientCommand::AutoStoreBagItem { .. } => Some("store"),
            ClientCommand::SetAmmo { .. } => Some("ammo"),
            ClientCommand::BuyItemInSlot { .. } => Some("buy"),
            ClientCommand::WrapItem { .. } => Some("wrap"),
            ClientCommand::AutoBankItem { .. } => Some("deposit"),
            ClientCommand::AutoStoreBankItem { .. } => Some("withdraw"),
            ClientCommand::SellItem { .. } => Some("sell"),
            ClientCommand::ItemQuery { .. } | ClientCommand::NameQuery { .. } => None,
            _ => Some("other"),
        })
        .collect()
}

/// `AutoEquipCursorItem` sends (`0x5e1757`) before its clear (`0x5e1763`), and `SwapItem`
/// (`0x5e0fdb`) before its own (`0x5e0fe7`), so the clear's listeners send after the equip:
/// `AutoEquipCursorItem()`, `EquipCursorItem` on a slot the item fits (`0x489757`, then `LockItem`
/// on the occupant, `0x489761`) and on one it does not (`0x4897a0`), the bag click's equip
/// (`0x4fa3ff`, after its own clear `0x4fa198`, then `LockItem` `0x4fa418`), the bind-confirm
/// accept (`0x5e1c23`, whose `SetCursorItem` drops the held item first, `0x494bb6`), and the bag
/// click's whole-stack swap (`LockItem` `0x4f9e85`, `SwapItem` `0x4f9eac`, its own clear
/// `0x4f9eb8`).
#[test]
fn each_cursor_equip_sends_ahead_of_its_clears_listeners() {
    let cases: [(&str, &str, &[&str]); 6] = [
        (
            "PickupContainerItem(0, 4)",
            "AutoEquipCursorItem()",
            &["equip", "cursor"],
        ),
        (
            "PickupContainerItem(0, 4)",
            "EquipCursorItem(1)",
            &["swap", "cursor", "lock"],
        ),
        (
            "PickupContainerItem(0, 1)",
            "EquipCursorItem(1)",
            &["equip", "cursor"],
        ),
        (
            "",
            "UseContainerItem(0, 4)",
            &["cursor", "cursor", "cursor", "equip", "cursor", "lock"],
        ),
        (
            "UseContainerItem(0, 7)",
            "EquipPendingItem(0)",
            &["lock", "cursor", "cursor", "equip", "cursor"],
        ),
        (
            "PickupContainerItem(0, 1)",
            "PickupContainerItem(0, 2)",
            &["lock", "swap", "cursor", "cursor"],
        ),
    ];
    for (setup, verb, wire) in cases {
        let mut h = Bags::new();
        assert_eq!(heard_around(&mut h, setup, verb), wire, "{setup} {verb}");
    }
}

/// Arrows in backpack slot 5 (wire 27), the projectile class the ammo slot loads by.
fn with_arrows(h: &mut Bags) {
    const ARROWS: u32 = 2512;
    const R: u64 = 0x4000_0000_0000_0A05;
    let world = h.app.world_mut();
    let me = world
        .query_filtered::<Entity, With<SelfPlayer>>()
        .single(world)
        .expect("us");
    world
        .get_mut::<ObjectStore>(me)
        .expect("our store")
        .0
        .merge(ObjectFields::from_pairs(&[
            (F_INV_SLOT_HEAD + 2 * 27, R as u32),
            (F_INV_SLOT_HEAD + 2 * 27 + 1, (R >> 32) as u32),
        ]));
    test_spawn_item(
        world,
        R,
        ObjectFields::from_pairs(&[(F_TYPE, 0x3), (FIELD_OBJECT_ENTRY, ARROWS), (14, 200)]),
        false,
    );
    world.resource_mut::<Items>().insert_template(
        ARROWS,
        Some(ItemInfo {
            class: crate::ui_items::ITEM_CLASS_PROJECTILE,
            ..test_template("Rough Arrow")
        }),
    );
}

/// A banker open, as the app holds it and the bank feed mirrors it to the VM, with linen in the
/// bank's slot 1 (the player's slot 39).
fn with_bank(h: &mut Bags) {
    const L: u64 = 0x4000_0000_0000_0A30;
    let world = h.app.world_mut();
    world
        .resource_mut::<crate::ui_bank::BankOpen>()
        .open(0xF130_0000_0000_0043);
    let me = world
        .query_filtered::<Entity, With<SelfPlayer>>()
        .single(world)
        .expect("us");
    world
        .get_mut::<ObjectStore>(me)
        .expect("our store")
        .0
        .merge(ObjectFields::from_pairs(&[
            (F_INV_SLOT_HEAD + 2 * 39, L as u32),
            (F_INV_SLOT_HEAD + 2 * 39 + 1, (L >> 32) as u32),
        ]));
    test_spawn_item(
        world,
        L,
        ObjectFields::from_pairs(&[(F_TYPE, 0x3), (FIELD_OBJECT_ENTRY, LINEN), (14, 1)]),
        false,
    );
    world
        .non_send_resource_mut::<UiScript>()
        .set_bank(Some(benilla_ui::script::BankState::default()));
}

/// Past `UseContainerItem`'s own clear (`0x4fa198`), its withdraw, deposit and sale send first
/// (`0x4fa305` into `0x5e18f0`,
/// `0x4fa33e` into `0x5e19e0`, `0x4fa378` into `0x5e1d50`) and lock after (`LockItem` at
/// `0x4fa312`, `0x4fa34b`, `0x4fa37f`); `PickupMerchantItem`'s sale sends (`0x4fb7a7`) ahead of
/// its `ClearCursor(0,1)` (`0x4fb7b3`).
#[test]
fn each_bank_and_vendor_send_goes_ahead_of_its_signals() {
    type Prep = fn(&mut Bags);
    let cases: [(Prep, &str, &str, &[&str]); 4] = [
        (
            with_bank,
            "",
            "UseContainerItem(-1, 1)",
            &["cursor", "withdraw", "lock"],
        ),
        (
            with_bank,
            "",
            "UseContainerItem(0, 1)",
            &["cursor", "deposit", "lock"],
        ),
        (
            Bags::open_vendor,
            "",
            "UseContainerItem(0, 1)",
            &["cursor", "sell", "lock"],
        ),
        (
            Bags::open_vendor,
            "PickupContainerItem(0, 1)",
            "PickupMerchantItem(0)",
            &["sell", "cursor"],
        ),
    ];
    for (prep, setup, verb, wire) in cases {
        let mut h = Bags::new();
        prep(&mut h);
        assert_eq!(heard_around(&mut h, setup, verb), wire, "{setup} {verb}");
    }
}

/// The other cursor verbs send in their bodies too, ahead of the clear or unlock that follows:
/// `DeleteCursorItem` (`0x4898a3`, then `ClearCursor(1,1)` `0x4898af` and `LockItem` `0x4898bc`),
/// the bag click's split (`0x4f9e43` into `0x5e1210`, then `0x4f9e4f`), the bag worker's store
/// (`0x4c7e49` into `0x5e12e0`, then `0x4c7e55`), the ammo slot's load (`0x4c7440`, then
/// `ClearCursor(1,1)` at `0x4c794d`), a vendor row's drop (`0x4f9f3c` into `0x5e1f30`, then
/// `0x4f9f48`) and the gift wrap (`0x5ee0a5`, then the paper's unlock, `0x5ee0aa` into
/// `0x5eded0`).
#[test]
fn each_cursor_send_goes_ahead_of_its_clears_listeners() {
    let none = |_: &mut Bags| {};
    // Armed with the host lent, so the paper resolves and its lock is real.
    let wrap = |h: &mut Bags| {
        with_hosted_vm(h.app.world_mut(), |s| s.expect("a VM").arm_gift_wrap(0, 6));
    };
    type Prep = fn(&mut Bags);
    let cases: [(Prep, &str, &str, &[&str]); 6] = [
        (
            none,
            "PickupContainerItem(0, 1)",
            "DeleteCursorItem()",
            &["destroy", "lock", "cursor", "lock"],
        ),
        (
            none,
            "SplitContainerItem(0, 3, 2)",
            "PickupContainerItem(0, 9)",
            &["split", "cursor"],
        ),
        (
            none,
            "PickupContainerItem(0, 1)",
            "PutItemInBag(20)",
            &["store", "cursor"],
        ),
        (
            with_arrows,
            "PickupContainerItem(0, 5)",
            "PickupInventoryItem(0)",
            &["ammo", "lock", "cursor"],
        ),
        (
            Bags::open_vendor,
            "PickupMerchantItem(1)",
            "PickupContainerItem(0, 5)",
            &["buy", "cursor"],
        ),
        (wrap, "", "PickupContainerItem(0, 1)", &["wrap", "lock"]),
    ];
    for (prep, setup, verb, wire) in cases {
        let mut h = Bags::new();
        prep(&mut h);
        assert_eq!(heard_around(&mut h, setup, verb), wire, "{setup} {verb}");
    }
}

/// An item action's press equips the item it finds in the bags (`0x4e602a` → `0x5e1840`), read
/// with the host applying the press: with no helm worn, the helm in backpack slot 4 goes out from
/// wire slot 26.
#[test]
fn an_item_actions_equip_reads_the_bags_as_it_runs() {
    let mut h = Bags::new();
    let world = h.app.world_mut();
    let me = world
        .query_filtered::<Entity, With<SelfPlayer>>()
        .single(world)
        .expect("us");
    world
        .get_mut::<ObjectStore>(me)
        .expect("our store")
        .0
        .merge(ObjectFields::from_pairs(&[
            (F_INV_SLOT_HEAD, 0),
            (F_INV_SLOT_HEAD + 1, 0),
        ]));
    h.app
        .world_mut()
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            0,
            benilla_protocol::messages::ActionButton {
                slot: 0,
                action: HELM,
                kind: benilla_protocol::messages::ACTION_KIND_ITEM,
            },
        );
    assert_eq!(
        h.verb("", "UseAction(1)"),
        [
            format!(
                "{:?}",
                ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 26
                }
            ),
            roll()
        ]
    );
}

/// The bind question fires inside the swap's call (`0x5e0ee4`). The doll click's `SwapItem`
/// leaves past its clear (`0x5e0eec` → `0x5e0f63`), so the event's handler reads the helm still
/// held and no `CURSOR_UPDATE` follows; `PickupContainerItem`'s own `ClearCursor(0,1)` follows its
/// swap (`0x4f9eb8`), so a worn helm dropped on the bind-on-equip helm hears `CURSOR_UPDATE` after
/// the question, which the stock dialog takes as its cancel (`UIParent.lua:357-361`).
#[test]
fn the_bind_question_fires_inside_the_swap_ahead_of_the_bag_clicks_clear() {
    let listen = r#"SEEN = {} local f = CreateFrame("Frame")
        f:RegisterEvent("EQUIP_BIND_CONFIRM") f:RegisterEvent("CURSOR_UPDATE")
        f:SetScript("OnEvent", function()
            table.insert(SEEN, event .. (CursorHasItem() and 1 or 0)) end)"#;
    let mut h = Bags::new();
    h.run(listen);
    let seen: String = h.eval(
        "PickupContainerItem(0, 7) SEEN = {} PickupInventoryItem(1) return table.concat(SEEN, ' ')",
    );
    assert_eq!(seen, "EQUIP_BIND_CONFIRM1", "the doll click");
    assert_eq!(h.sent(), Vec::<String>::new(), "nothing sent");

    let mut h = Bags::new();
    h.run(listen);
    let seen: String = h.eval(
        "PickupInventoryItem(1) SEEN = {} PickupContainerItem(0, 7) return table.concat(SEEN, ' ')",
    );
    assert_eq!(seen, "EQUIP_BIND_CONFIRM1 CURSOR_UPDATE0", "the bag click");
    assert_eq!(h.sent(), Vec::<String>::new(), "nothing sent");
}

/// `PickupContainerItem` asks inside `SwapItem`, then clears with its own `ClearCursor(0,1)`
/// past the call (`0x4f9eb8`). Through the stock FrameXML the clear's `CURSOR_UPDATE` hides
/// `EQUIP_BIND` (`UIParent.lua:357-361`), whose `OnHide` calls `CancelPendingEquip` into
/// `0x5e1be0`: the record is freed, both ends unlock (`0x5e1ca5`, `0x5e1d04`), nothing is sent and
/// the cursor ends empty.
#[test]
fn the_bag_clicks_clear_cancels_its_bind_question_and_unlocks_both_ends() {
    benilla_formats::wow_data_or_skip!();
    let mut h = Bags::new();
    h.script().set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\UIParent.xml",
        "Interface\\FrameXML\\MoneyFrame.lua",
        "Interface\\FrameXML\\MoneyFrame.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
    ] {
        crate::ui_script::load_ui_for_test(&h.script(), file);
    }
    let shown = "return StaticPopup_FindVisible(\"EQUIP_BIND\") ~= nil";
    h.run(
        r#"ASKED = 0 local f = CreateFrame("Frame") f:RegisterEvent("EQUIP_BIND_CONFIRM")
        f:SetScript("OnEvent", function() ASKED = ASKED + 1 end)"#,
    );
    h.run("PickupInventoryItem(1)");
    assert!(h.script().item_locked(W), "the pickup's lock");
    let _ = h.sent();
    h.run("PickupContainerItem(0, 7)");
    assert_eq!(h.eval::<i64>("return ASKED"), 1, "asked inside the swap");
    assert!(!h.eval::<bool>(shown), "the clear's CURSOR_UPDATE hid it");
    assert_eq!(h.sent(), Vec::<String>::new(), "nothing sent");
    {
        let script = h.script();
        assert!(script.cursor_item().is_none(), "the binding's clear");
        assert!(!script.item_locked(W), "the worn helm unlocked");
        assert!(!script.item_locked(E), "the bag's helm unlocked");
    }
    assert_eq!(
        h.app
            .world()
            .resource::<crate::ui_bind_confirm::PendingEquips>()
            .live(),
        0,
        "the record freed"
    );
}

/// `CGItem::Use` signals `USE_BIND_CONFIRM` in place, inside the use (`0x5d9217`-`0x5d9222`,
/// `SignalEvent(0x122)`), and returns: the chunk's next line has heard it.
#[test]
fn the_bind_on_use_question_fires_inside_the_use() {
    let mut h = Bags::new();
    h.run(
        r#"HEARD = 0 local f = CreateFrame("Frame") f:RegisterEvent("USE_BIND_CONFIRM")
        f:SetScript("OnEvent", function() HEARD = HEARD + 1 end)"#,
    );
    let heard: i64 = h.eval("UseContainerItem(0, 8) return HEARD");
    assert_eq!(heard, 1);
    assert_eq!(h.sent(), Vec::<String>::new(), "the use is parked");
}

/// `ConfirmBindOnUse` re-issues the parked use inside its call (`0x48d770` → `0x5d92c1`).
#[test]
fn the_bind_on_use_accept_uses_the_item_inside_its_call() {
    let mut h = Bags::new();
    h.run("UseContainerItem(0, 8)");
    assert_eq!(h.sent(), Vec::<String>::new(), "the use is parked");
    let sent = h.verb("", "ConfirmBindOnUse()");
    assert_eq!(sent.last(), Some(&roll()), "{sent:?}");
    assert!(
        sent[..sent.len() - 1]
            .iter()
            .any(|c| c.starts_with("UseItem {")),
        "{sent:?}"
    );
}

/// A held vendor row dropped on a bag slot buys into that bag (`0x4c7c00` → `0x4c7e66`):
/// `CMSG_BUY_ITEM_IN_SLOT` with the bag's guid, the player's for the backpack (`0x4c7c7d`), slot
/// `0xFF` and count 1 (`0x4c7e9c`-`0x4c7eaa`), then `ClearCursor(1,1)` and nil. An empty bag slot
/// goes to the doll click first (`0x4c7ca7`), which buys into the slot itself (`0x4c78fd`).
#[test]
fn a_vendor_row_dropped_on_a_bag_buys_into_it() {
    let buy = |bag_guid: u64, bag_slot: u8| {
        format!(
            "{:?}",
            ClientCommand::BuyItemInSlot {
                vendor: VENDOR,
                entry: ROW_ENTRY,
                bag_guid,
                bag_slot,
                count: 1,
            }
        )
    };
    for (drop, bag_guid, bag_slot) in [
        ("PutItemInBag(20)", BAG, 0xFF),
        ("PutItemInBackpack()", ME, 0xFF),
        ("PutItemInBag(21)", ME, 20),
    ] {
        let mut h = Bags::new();
        h.open_vendor();
        h.run("PickupMerchantItem(1)");
        let answer: Option<i64> = h.eval(&format!("return {drop}"));
        assert_eq!(answer, None, "{drop}: the buy answers nil");
        assert_eq!(h.sent(), [buy(bag_guid, bag_slot)], "{drop}");
        assert!(
            h.script().cursor_payload().is_none(),
            "{drop}: the row is spent"
        );
    }
}
