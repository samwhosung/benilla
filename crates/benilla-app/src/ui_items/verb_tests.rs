//! The bag, cursor and paper-doll verbs inside their calls, through the whole client's call host:
//! each packet goes out before the verb returns, as the reference's bindings send in their own
//! bodies, so a chunk's packets leave in call order and its next line reads what the verb did.

use benilla_protocol::field::{FIELD_CONTAINER_SLOT_1, FIELD_OBJECT_ENTRY};
use benilla_protocol::messages::ItemInfo;
use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    ContainerSlot, ContainerState, InvSlotView, InventorySlots, MerchantItem, MerchantState,
    UiScript, UseFork,
};
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

fn slot(guid: u64, item_id: u32, count: u32) -> ContainerSlot {
    ContainerSlot {
        guid,
        item_id,
        count,
        texture: Some("Interface\\Icons\\INV_Misc_QuestionMark".into()),
        link: Some(format!("|cffffffff|Hitem:{item_id}:0:0:0|h[Thing]|h|r")),
        // The record cached.
        quality: Some(1),
        ..Default::default()
    }
}

/// The whole client, headless: the backpack, a bag in doll slot 20 and a worn helm, as both the
/// player's descriptor and the VM's snapshot hold them.
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
            test_spawn_item(
                world,
                guid,
                ObjectFields::from_pairs(&[(F_TYPE, 0x3), (FIELD_OBJECT_ENTRY, entry)]),
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
        let helm = |guid: u64, item_id: u32, bonding: u32| ContainerSlot {
            equip_slots: vec![1],
            inventory_type: 1,
            bonding,
            ..slot(guid, item_id, 1)
        };
        let backpack = ContainerState {
            name: None,
            num_slots: 16,
            slots: [
                (1, slot(A, LINEN, 1)),
                (2, slot(B, LINEN, 1)),
                (3, slot(C, LINEN, 5)),
                (4, helm(H, HELM, 0)),
                (6, slot(P, PAPER, 1)),
                (7, helm(E, BOE, 2)),
                (
                    8,
                    ContainerSlot {
                        bonding: 3,
                        use_spell: 4321,
                        use_fork: UseFork::Plain,
                        item_class: Some(0),
                        ..slot(U, BOU, 1)
                    },
                ),
            ]
            .into(),
        };
        script.set_container(0, Some(backpack));
        script.set_container(
            1,
            Some(ContainerState {
                name: Some("Linen Bag".into()),
                num_slots: 4,
                slots: [(1, slot(X, LINEN, 1))].into(),
            }),
        );
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

    /// Open the vendor, its one row and its repair service, in the app and the VM.
    fn open_vendor(&mut self) {
        self.app
            .world_mut()
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .vendor = Some(VENDOR);
        self.script().set_merchant(Some(MerchantState {
            items: vec![MerchantItem {
                item_id: ROW_ENTRY,
                name: Some("Bread".into()),
                texture: Some("Interface\\Icons\\INV_Misc_Food_11".into()),
                quantity: 1,
                num_available: -1,
                ..Default::default()
            }],
            buyback: Vec::new(),
            can_repair: true,
        }));
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
