//! The cooldown list as a listener reads it, packet by packet. Each cooldown packet's handler
//! writes the player's list (`0xcecaec`) and signals inside itself: `SMSG_SPELL_COOLDOWN`
//! (`0x6e9460`) adds its records and calls `0x4b31b0` (`0x6e95b0`), `ACTIONBAR_UPDATE_COOLDOWN`
//! then `SPELL_UPDATE_COOLDOWN`; `SMSG_ITEM_COOLDOWN` (`0x6e95d0`) adds its record, then the pair
//! and `BAG_UPDATE_COOLDOWN` (`0x6e9656`, `0x6e965b`). The getters query the list inside the call:
//! `GetActionCooldown` (`0x4e6ed0` → `0x4e6ca0` → `0x6e2ea0`/`0x6e2ed0`), `GetSpellCooldown`
//! (`0x4b40a0` → `0x6e2ea0`), `GetContainerItemCooldown` (`0x4f99b0`) and
//! `GetInventoryItemCooldown` (`0x4c8a60`) through `0x6e2ed0`. So two cooldown packets in one
//! frame signal twice, and each listener reads its own packet's records.

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{ActionButton, ACTION_KIND_ITEM, ACTION_KIND_SPELL};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// Two spells with no cooldown of their own, on bar slots 1 and 2 and in the book.
const FROST_NOVA: u32 = 122;
const BLINK: u32 = 1953;
/// A potion in backpack slot 1, ours, and the on-use spell its record names.
const POTION_GUID: u64 = 0x4000_0000_0000_00F1;
const POTION: u32 = 118;
const POTION_SPELL: u32 = 439;

/// `OBJECT_FIELD_TYPE`, `UNIT_FIELD_FLAGS` and its player-controlled bit.
const OBJECT_TYPE: u16 = 2;
const FLAGS: u16 = 46;
const PLAYER_CONTROLLED: u32 = 0x8;
/// `PLAYER_FIELD_PACK_SLOT_1`.
const PACK_SLOT_1: u16 = 532;

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    _sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// `chunk`'s answer through the lent host when `live`, else off the snapshots alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// What the spy heard since the last call, its events among `events` alone.
    fn seen(&mut self, events: &[&str]) -> Vec<String> {
        let seen: Vec<String> = self.eval(false, "local s = Seen Seen = {} return s");
        seen.into_iter()
            .filter(|l| events.iter().any(|e| l.split(':').next() == Some(e)))
            .collect()
    }
}

/// The two spells' rows and the potion's spell.
fn spells() -> crate::ui_action::Spells {
    spells_with(&[])
}

/// [`spells`] and the rows of `more`.
fn spells_with(more: &[u32]) -> crate::ui_action::Spells {
    let row = |id: u32| {
        (
            id,
            SpellDisplay {
                id,
                name: format!("Spell {id}"),
                icon: Some(format!("Interface\\Icons\\Spell_{id}")),
                ..Default::default()
            },
        )
    };
    crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            [FROST_NOVA, BLINK, POTION_SPELL]
                .iter()
                .chain(more)
                .map(|&id| row(id))
                .collect::<HashMap<_, _>>(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// The client in the world as [`ME`], Frost Nova and Blink in the book and on bar slots 1 and 2,
/// the potion in backpack slot 1 and on bar slot 3; and a spy whose line for each cooldown event
/// carries the durations its listener read: each slot's `GetActionCooldown`, the book's
/// `GetSpellCooldown` for both spells, and the potion's `GetContainerItemCooldown`.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    app.insert_resource(spells());
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"ACTIONBAR_UPDATE_COOLDOWN", "SPELL_UPDATE_COOLDOWN",
                    "BAG_UPDATE_COOLDOWN"}) do
                f:RegisterEvent(e)
            end
            local function d(_, duration) return tostring(duration) end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. d(GetActionCooldown(1)) .. ":"
                    .. d(GetActionCooldown(2)) .. ":" .. d(GetActionCooldown(3)) .. ":"
                    .. d(GetSpellCooldown(1, "spell")) .. ":" .. d(GetSpellCooldown(2, "spell"))
                    .. ":" .. d(GetContainerItemCooldown(0, 1)))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    let world = app.world_mut();
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    world
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .extend([FROST_NOVA, BLINK]);
    let mut potion = crate::items::test_template("Potion");
    potion.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
        spell_id: POTION_SPELL,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    });
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(POTION, Some(potion));
    {
        let mut slots = world.resource_mut::<crate::ui_action::ActionSlots>();
        for (slot, action, kind) in [
            (0, FROST_NOVA, ACTION_KIND_SPELL),
            (1, BLINK, ACTION_KIND_SPELL),
            (2, POTION, ACTION_KIND_ITEM),
        ] {
            slots
                .staged
                .insert(slot, ActionButton { slot, action, kind });
        }
    }
    crate::items::test_spawn_item(
        world,
        POTION_GUID,
        ObjectFields::from_pairs(&[
            (OBJECT_TYPE, 0x3),
            (3, POTION),
            (6, ME as u32),
            (7, (ME >> 32) as u32),
            (14, 2),
        ])
        .into_created(benilla_protocol::messages::ObjectType::Item),
        false,
    );
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (FLAGS, PLAYER_CONTROLLED),
            (PACK_SLOT_1, POTION_GUID as u32),
            (PACK_SLOT_1 + 1, (POTION_GUID >> 32) as u32),
        ],
    )]]);
    c.frame(vec![]);
    c.frame(vec![]);
    c.seen(&[]);
    c
}

/// `SMSG_SPELL_COOLDOWN` for the player with one spell and its time.
fn spell_cooldown(spell: u32, ms: u32) -> SessionEvent {
    SessionEvent::SpellCooldowns {
        caster: ME,
        cooldowns: vec![(spell, ms)],
    }
}

/// Two `SMSG_SPELL_COOLDOWN`s in one frame: each handler adds its record and signals the pair
/// inside itself (`0x6e9598`, then `0x6e95b0` → `0x4b31b0`), so the pair fires twice, and each
/// listener's queries (`0x6e2ea0`) read the list as its own packet left it: Blink cold to the
/// first, running to the second.
#[test]
fn two_cooldown_packets_signal_twice_each_as_its_packet_left_the_list() {
    let mut c = client();
    c.frame(vec![
        vec![spell_cooldown(FROST_NOVA, 10_000)],
        vec![spell_cooldown(BLINK, 20_000)],
    ]);
    assert_eq!(
        c.seen(&["ACTIONBAR_UPDATE_COOLDOWN", "SPELL_UPDATE_COOLDOWN"]),
        [
            "ACTIONBAR_UPDATE_COOLDOWN:10:0:0:10:0:0",
            "SPELL_UPDATE_COOLDOWN:10:0:0:10:0:0",
            "ACTIONBAR_UPDATE_COOLDOWN:10:20:0:10:20:0",
            "SPELL_UPDATE_COOLDOWN:10:20:0:10:20:0",
        ]
    );
}

/// `SMSG_ITEM_COOLDOWN` then `SMSG_SPELL_COOLDOWN` in one frame: the item's handler adds the
/// potion's 30 s record and signals the pair and the bags (`0x6e9651`, `0x6e9656`, `0x6e965b`),
/// read with Frost Nova still cold; the spell packet's pair after it reads both.
#[test]
fn an_item_cooldowns_listeners_read_its_packets_list() {
    let mut c = client();
    c.frame(vec![
        vec![SessionEvent::ItemCooldown {
            item_guid: POTION_GUID,
            spell_id: POTION_SPELL,
        }],
        vec![spell_cooldown(FROST_NOVA, 10_000)],
    ]);
    assert_eq!(
        c.seen(&[
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "BAG_UPDATE_COOLDOWN"
        ]),
        [
            "ACTIONBAR_UPDATE_COOLDOWN:0:0:30:0:0:30",
            "SPELL_UPDATE_COOLDOWN:0:0:30:0:0:30",
            "BAG_UPDATE_COOLDOWN:0:0:30:0:0:30",
            "ACTIONBAR_UPDATE_COOLDOWN:10:0:30:10:0:30",
            "SPELL_UPDATE_COOLDOWN:10:0:30:10:0:30",
        ]
    );
}

/// A cooldown query that missed its record left the callback `0x6e2f70` on the item cache
/// (`0x6e2efb`), and the record's answer runs it inside its handler (`0x55bdb0`): the query again,
/// and with the item's cooldown running the pair and the bags (`0x6e2f89`, `0x6e2f8e`), at the
/// answer's place. So with the answer first and a spell cooldown after it in one frame, the
/// record's flush fires first and reads Frost Nova cold.
#[test]
fn a_cooldown_querys_record_flushes_at_its_answers_place() {
    const HEARTHSTONE: u32 = 6948;
    const HEARTH: u32 = 8690;
    let mut c = client();
    let world = c.app.world_mut();
    {
        let mut slots = world.resource_mut::<crate::ui_action::ActionSlots>();
        slots.buttons.insert(
            3,
            ActionButton {
                slot: 3,
                action: HEARTHSTONE,
                kind: ACTION_KIND_ITEM,
            },
        );
    }
    world
        .resource_mut::<crate::spell::Cooldowns>()
        .apply_wire_item_cooldown(HEARTHSTONE, HEARTH, std::time::Instant::now());
    c.frame(vec![]);
    // The button's cooldown read (`ActionButton_UpdateCooldown`), whose query misses the record.
    c.eval::<()>(true, "GetActionCooldown(4)");
    c.seen(&[]);
    let mut record = crate::items::test_template("Hearthstone");
    record.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
        spell_id: HEARTH,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    });
    c.frame(vec![
        vec![SessionEvent::ItemTemplate {
            entry: HEARTHSTONE,
            info: Some(Box::new(record)),
        }],
        vec![spell_cooldown(FROST_NOVA, 10_000)],
    ]);
    assert_eq!(
        c.seen(&[
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "BAG_UPDATE_COOLDOWN"
        ]),
        [
            "ACTIONBAR_UPDATE_COOLDOWN:0:0:0:0:0:0",
            "SPELL_UPDATE_COOLDOWN:0:0:0:0:0:0",
            "BAG_UPDATE_COOLDOWN:0:0:0:0:0:0",
            "ACTIONBAR_UPDATE_COOLDOWN:10:0:0:10:0:0",
            "SPELL_UPDATE_COOLDOWN:10:0:0:10:0:0",
        ]
    );
}

/// `SMSG_PET_SPELLS` derives the macros that follow the pet's book after its book's sort and
/// ahead of `SetPetGuid` (`0x4bdadb` → `0x4b2fd0`, `0x4bdae0` → `0x4f0ce0`, `0x4bdaf0` →
/// `0x4bc7e0`): a `/cast` of the pet's spell that missed is still unbound to the sort's
/// `SPELLS_CHANGED`, and bound out of the pet's book to `SetPetGuid`'s `PET_BAR_UPDATE`, which
/// `GetActionAutocast` reads through the record's book (`0x4e6fcb`).
#[test]
fn a_pet_spell_list_binds_its_macros_between_its_sort_and_its_pet() {
    use benilla_protocol::messages::{
        PetActionEntry, PetSpells, ACTION_KIND_MACRO, PET_ACT_ENABLED,
    };
    const PET: u64 = 0x2A;
    const SEDUCTION: u32 = 6358;
    let mut c = client();
    let world = c.app.world_mut();
    world.insert_resource(spells_with(&[SEDUCTION]));
    {
        let mut script = world.non_send_resource_mut::<UiScript>();
        script.run(r#"SLASH_CAST1 = "/cast""#).unwrap();
        script.set_macros(benilla_ui::script::MacroState::from_scopes(
            vec![benilla_ui::script::MacroView {
                name: "Sed".into(),
                body: format!("/cast Spell {SEDUCTION}"),
                ..Default::default()
            }],
            Vec::new(),
        ));
        // The load's derivation, with no pet: a miss.
        script.derive_macro_bindings(|_, _| true);
    }
    crate::ui_action::hold_for_test(world, 4, ACTION_KIND_MACRO, 1);
    c.frame(vec![]);
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("SPELLS_CHANGED")
        g:RegisterEvent("PET_BAR_UPDATE")
        g:SetScript("OnEvent", function()
            table.insert(Seen, "PET:" .. event .. ":" .. tostring(GetActionAutocast(4)))
        end)"#,
    );
    c.seen(&[]);
    let entry = PetActionEntry::from(SEDUCTION | (u32::from(PET_ACT_ENABLED) << 24));
    c.frame(vec![vec![SessionEvent::PetSpells(Box::new(PetSpells {
        pet_guid: PET,
        spells: vec![entry],
        ..Default::default()
    }))]]);
    assert_eq!(
        c.seen(&["PET"]),
        ["PET:SPELLS_CHANGED:nil", "PET:PET_BAR_UPDATE:1"]
    );
}

/// The getters answer through the lent host for every slot, the book and the bags, cold and
/// running.
#[test]
fn every_cooldown_getter_answers_live_cold_and_running() {
    let mut c = client();
    let chunk = r#"
        local out = {}
        local function note(what, a, b, e)
            table.insert(out, what .. " " .. tostring(a) .. "," .. tostring(b) .. ","
                .. tostring(e))
        end
        for s = 1, 4 do note("action " .. s, GetActionCooldown(s)) end
        for s = 1, 2 do note("spell " .. s, GetSpellCooldown(s, "spell")) end
        note("bag 0 1", GetContainerItemCooldown(0, 1))
        note("bag 0 2", GetContainerItemCooldown(0, 2))
        return out"#;
    let agree = |c: &mut Client| -> Vec<String> { c.eval(true, chunk) };
    let cold = agree(&mut c);
    assert!(cold.iter().any(|l| l == "action 1 0,0,1"), "{cold:#?}");
    assert!(cold.iter().any(|l| l == "action 4 0,0,0"), "{cold:#?}");
    c.frame(vec![
        vec![spell_cooldown(FROST_NOVA, 10_000)],
        vec![SessionEvent::ItemCooldown {
            item_guid: POTION_GUID,
            spell_id: POTION_SPELL,
        }],
    ]);
    let running = agree(&mut c);
    for prefix in ["action 1 ", "spell 1 ", "action 3 ", "bag 0 1 "] {
        let line = running
            .iter()
            .find(|l| l.starts_with(prefix))
            .expect("noted");
        assert!(!line.ends_with(" 0,0,1"), "{line} runs");
    }
}

/// A record a packet added after the frame's pushes reads through the host at once; the
/// snapshot still holds the push.
#[test]
fn the_cooldown_getters_read_the_list_as_they_run() {
    let mut c = client();
    let spells = spells();
    let blink = spells.catalog.get(BLINK).expect("a row");
    c.app
        .world_mut()
        .resource_mut::<crate::spell::Cooldowns>()
        .apply_spell_cooldowns(
            [(BLINK, 15_000, Some(blink))],
            &crate::spell::SpellModifiers::default(),
            crate::spell::CategoryPad::NoPlayer,
            std::time::Instant::now(),
        );
    let chunk = "local _, a = GetActionCooldown(2) local _, b = GetSpellCooldown(2, 'spell') \
                 return a, b";
    assert_eq!(c.eval::<(f64, f64)>(true, chunk), (15.0, 15.0));
    assert_eq!(c.eval::<(f64, f64)>(false, chunk), (0.0, 0.0));
}

/// `GetInventoryItemCooldown` (`0x4c8a60`) reads the list inside the call: the worn slot found as
/// of there (`0x4c8b10`), its record queried through `0x6e2ed0` (`0x4c8b86`). The potion's
/// `ITEM_COOLDOWN` first, then the worn trinket's, in one frame: the first's `BAG_UPDATE_COOLDOWN`
/// reads the trinket cold, the second's reads it running, and live and off the snapshot agree once
/// the frame's feeds ran.
#[test]
fn a_worn_items_cooldown_reads_live_inside_its_packet() {
    const TRINKET_GUID: u64 = 0x4000_0000_0000_00F2;
    const TRINKET: u32 = 119;
    /// `PLAYER_FIELD_INV_SLOT_HEAD` + 2 × 12: the first trinket slot, 13 to Lua.
    const INV_TRINKET_1: u16 = 486 + 24;
    let mut c = client();
    let world = c.app.world_mut();
    let mut trinket = crate::items::test_template("Trinket");
    trinket.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
        spell_id: BLINK,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    });
    // Block 0 names the spell, without which the getter runs no query (`0x4c8b47`).
    trinket.spells = vec![benilla_protocol::messages::ItemSpellEntry {
        index: 0,
        spell_id: BLINK,
        trigger: 0,
        charges: 0,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    }];
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(TRINKET, Some(trinket));
    crate::items::test_spawn_item(
        world,
        TRINKET_GUID,
        ObjectFields::from_pairs(&[
            (OBJECT_TYPE, 0x3),
            (3, TRINKET),
            (6, ME as u32),
            (7, (ME >> 32) as u32),
            (14, 1),
        ])
        .into_created(benilla_protocol::messages::ObjectType::Item),
        false,
    );
    c.frame(vec![vec![values(
        ME,
        &[
            (INV_TRINKET_1, TRINKET_GUID as u32),
            (INV_TRINKET_1 + 1, (TRINKET_GUID >> 32) as u32),
        ],
    )]]);
    c.frame(vec![]);
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("BAG_UPDATE_COOLDOWN")
        g:SetScript("OnEvent", function()
            local _, d = GetInventoryItemCooldown("player", 13)
            table.insert(Seen, "WORN:" .. tostring(d))
        end)"#,
    );
    let chunk = "local _, d = GetInventoryItemCooldown('player', 13) return d";
    assert_eq!(c.eval::<f64>(true, chunk), 0.0, "cold before");
    c.frame(vec![
        vec![SessionEvent::ItemCooldown {
            item_guid: POTION_GUID,
            spell_id: POTION_SPELL,
        }],
        vec![SessionEvent::ItemCooldown {
            item_guid: TRINKET_GUID,
            spell_id: BLINK,
        }],
    ]);
    assert_eq!(c.seen(&["WORN"]), ["WORN:0", "WORN:30"]);
    assert_eq!(c.eval::<f64>(true, chunk), 30.0);
}

/// The pet's list read inside the call, through the book and the bar: `GetSpellCooldown(id,
/// "pet")` queries list 1 (`0x4b40dd` → `0x4b410f` → `0x6e2ea0`), and `GetActionCooldown` on a
/// slot whose macro is bound to the pet's book likewise (`0x4e6cf7` → `0x4e6d0b`). Two pet
/// `SMSG_SPELL_COOLDOWN`s in one frame: the first packet's `SPELL_UPDATE_COOLDOWN` reads Bite cold
/// through both, the second's reads it running.
#[test]
fn the_pet_book_and_a_pet_bound_slot_read_the_pets_list_inside_their_packet() {
    use benilla_protocol::messages::{PetActionEntry, ACTION_KIND_MACRO, PET_ACT_ENABLED};
    const PET: u64 = 0xF130_0000_0000_0042;
    const GROWL: u32 = 2649;
    const BITE: u32 = 17253;
    let mut c = client();
    let world = c.app.world_mut();
    world.insert_resource(spells_with(&[GROWL, BITE]));
    let entry = |id: u32| PetActionEntry::from(id | (u32::from(PET_ACT_ENABLED) << 24));
    {
        let mut bar = world.resource_mut::<crate::ui_pet::PetBar>();
        bar.spells.pet_guid = PET;
        bar.spells.bar[0] = entry(GROWL);
        bar.spells.bar[1] = entry(BITE);
        bar.spells.spells = vec![entry(GROWL), entry(BITE)];
    }
    let pet = world
        .spawn((
            crate::net::Guid(PET),
            Transform::default(),
            crate::net::ObjectStore(ObjectFields::from_pairs(&[
                (OBJECT_TYPE, 0x9),
                (22, 100),
                (12, ME as u32),
                (13, (ME >> 32) as u32),
            ])),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(PET, pet);
    crate::ui_action::hold_for_test(world, 4, ACTION_KIND_MACRO, 1);
    c.frame(vec![]);
    c.frame(vec![]);
    crate::ui_macro::bind_for_test(
        &mut c.app.world_mut().non_send_resource_mut::<UiScript>(),
        &[(
            1,
            crate::ui_macro::Bound {
                spell: crate::ui_macro::BoundSpell::Spell(BITE),
                pet_book: true,
            },
        )],
    );
    c.frame(vec![]);
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("SPELL_UPDATE_COOLDOWN")
        g:SetScript("OnEvent", function()
            local _, a = GetSpellCooldown(1, "pet")
            local _, b = GetSpellCooldown(2, "pet")
            local _, s = GetActionCooldown(4)
            table.insert(Seen, "PET:" .. tostring(a) .. ":" .. tostring(b) .. ":" .. tostring(s))
        end)"#,
    );
    c.seen(&[]);
    let pet_cooldown = |spell: u32, ms: u32| SessionEvent::SpellCooldowns {
        caster: PET,
        cooldowns: vec![(spell, ms)],
    };
    c.frame(vec![
        vec![pet_cooldown(GROWL, 3_000)],
        vec![pet_cooldown(BITE, 4_000)],
    ]);
    // The book sorts Bite ahead of Growl.
    assert_eq!(c.seen(&["PET"]), ["PET:0:3:0", "PET:4:3:4"]);
}

/// A cross-map `SMSG_NEW_WORLD` in a frame, as the cross-map one carries `needs_ack`.
fn port() -> SessionEvent {
    SessionEvent::Worldport {
        map_id: 1,
        position: [0.0; 3],
        orientation: 0.0,
        needs_ack: true,
    }
}

/// Frost Nova's and Blink's action slots and the potion's bag slot, as `GetActionCooldown` and
/// `GetContainerItemCooldown` read the lists through the lent host: the durations in seconds.
fn running(c: &mut Client) -> (f64, f64, f64) {
    c.eval(
        true,
        "local _, a = GetActionCooldown(1) local _, b = GetActionCooldown(2) \
         local _, p = GetContainerItemCooldown(0, 1) return a, b, p",
    )
}

/// The records both lists hold: the player's, then the pet's.
fn held(c: &Client) -> (usize, usize) {
    let world = c.app.world();
    (
        world.resource::<crate::spell::Cooldowns>().record_count(),
        world
            .resource::<crate::ui_pet::PetBar>()
            .cooldowns
            .record_count(),
    )
}

/// Frost Nova on 10 s, the potion on the client's 30 s, and Blink on the pet's list.
fn running_lists() -> Client {
    let mut c = client();
    c.frame(vec![
        vec![spell_cooldown(FROST_NOVA, 10_000)],
        vec![SessionEvent::ItemCooldown {
            item_guid: POTION_GUID,
            spell_id: POTION_SPELL,
        }],
    ]);
    let spells = spells();
    c.app
        .world_mut()
        .resource_mut::<crate::ui_pet::PetBar>()
        .cooldowns
        .apply_spell_cooldowns(
            [(BLINK, 15_000, spells.catalog.get(BLINK))],
            &crate::spell::SpellModifiers::default(),
            crate::spell::CategoryPad::Skip,
            std::time::Instant::now(),
        );
    // The pet record's flush fires with the frame's lanes.
    c.frame(vec![]);
    c.seen(&[]);
    assert_eq!(running(&mut c), (10.0, 0.0, 30.0));
    assert_eq!(
        held(&c),
        (2, 1),
        "Frost Nova's and the potion's, and Blink's"
    );
    c
}

/// The world load's body calls `ClearItemAndSpellCooldowns` (`0x401c17` → `0x6e9ad0`), in the tick
/// the timer runs: both history lists emptied and the item cooldowns freed, with no flush, so no
/// cooldown event fires. The server's `SMSG_INITIAL_SPELLS` after it then seeds only what it lists.
#[test]
fn a_worldport_empties_the_cooldowns_and_the_resend_seeds_only_what_it_lists() {
    let mut c = running_lists();
    c.frame(vec![vec![port()]]);
    assert_eq!(held(&c), (2, 1), "the port's own tick clears nothing");
    c.frame(vec![]);
    // The potion goes with the objects, so its getter reads cold whatever its record does: the
    // lists are read by their records.
    assert_eq!(
        held(&c),
        (0, 0),
        "the timer's tick empties both lists, the item cooldown with them"
    );
    assert_eq!(running(&mut c), (0.0, 0.0, 0.0));
    assert_eq!(
        c.seen(&[
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "BAG_UPDATE_COOLDOWN"
        ]),
        Vec::<String>::new(),
        "the clear signals nothing"
    );
    c.frame(vec![vec![SessionEvent::SpellBook {
        spell_ids: vec![FROST_NOVA, BLINK],
        cooldowns: vec![benilla_protocol::messages::SpellCooldown {
            spell_id: BLINK as u16,
            item_id: 0,
            category: 0,
            spell_cd_ms: 20_000,
            category_cd_ms: 0,
        }],
    }]]);
    let (nova, blink, _) = running(&mut c);
    assert_eq!(
        (nova, blink, held(&c).0),
        (0.0, 20.0, 1),
        "Blink alone, as the resend lists it"
    );
}

/// A teleport inside the map (`MSG_MOVE_TELEPORT`) runs no world load: both lists stand.
#[test]
fn a_same_map_teleport_leaves_the_cooldowns() {
    let mut c = running_lists();
    let before = held(&c);
    c.frame(vec![vec![SessionEvent::Teleport {
        guid: ME,
        counter: 1,
        position: [1.0, 2.0, 3.0],
        orientation: 0.0,
    }]]);
    c.frame(vec![]);
    assert_eq!(running(&mut c), (10.0, 0.0, 30.0));
    assert_eq!(held(&c), before);
}
