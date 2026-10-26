//! The pet bar, its pet, its menu, its stat block and its book as a listener reads them, and a
//! press's signal at its place. Each binding reads the cached guid, the slot words, the mode word,
//! the attack latch and the pet the guid resolves inside itself (`0x4bdc50`, `0x4bcf70`,
//! `0x4be0e0`, `0x4be670`, `0x4be500`, `0x4be600`, the book `0x4b4410`); the press dispatcher
//! `0x4bd1d0` asks the gate first (`0x4bd1f2`), then writes the mode word and signals
//! `PET_BAR_UPDATE` (`0x4bc979`) ahead of its send (`0x4bd444`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::{PetActionEntry, PetSpells, PET_ACT_ENABLED};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const PET: u64 = 0xF140_0000_0000_002A;
const CLAW: u32 = 16_827;
const GROWL: u32 = 2_649;
/// `UNIT_FIELD_BYTES_0` (class in byte 1), `UNIT_FIELD_SUMMONEDBY`, `UNIT_FIELD_FLAGS`,
/// `UNIT_FIELD_PETNUMBER` and `UNIT_FIELD_PETEXPERIENCE`, `_PETNEXTLEVELEXP`.
const BYTES_0: u16 = 36;
const SUMMONEDBY: u16 = 12;
const FLAGS: u16 = 46;
const PETNUMBER: u16 = 139;
const PETXP: u16 = 141;
const PETNEXTXP: u16 = 142;
/// `UNIT_FLAG_PET_ABANDON` (`0x4be544`) and stunned, one of the gate's three (`0x4bd075`).
const ABANDON: u32 = 0x20;
const STUNNED: u32 = 0x4_0000;
/// The Stay command and the Attack command, as the bar's words hold them.
const STAY: u32 = 0x0700_0000;
const ATTACK: u32 = 0x0700_0002;

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// `chunk`'s answer through the lent host when `live`, else off the VM's copies alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// Fields written into the pet's store, with no packet and no frame: the world as a packet's
    /// handler leaves it, which the VM's copies have not heard of.
    fn write_pet(&mut self, pairs: &[(u16, u32)]) {
        let world = self.app.world_mut();
        let entity = world.resource::<crate::net::GuidIndex>().0[&PET];
        world
            .get_mut::<crate::net::ObjectStore>(entity)
            .expect("the pet's store")
            .0
            .merge(ObjectFields::from_pairs(pairs));
    }

    /// The bar as a handler or a verb leaves it, with no frame.
    fn bar(&mut self) -> Mut<'_, crate::ui_pet::PetBar> {
        self.app.world_mut().resource_mut::<crate::ui_pet::PetBar>()
    }

    /// The commands sent since the last call, by kind.
    fn wire(&self) -> Vec<&'static str> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::RandomRoll { .. } => Some("RandomRoll"),
                ClientCommand::PetAction { .. } => Some("PetAction"),
                _ => None,
            })
            .collect()
    }
}

fn spell_word(spell_id: u32) -> PetActionEntry {
    PetActionEntry::from(spell_id | (u32::from(PET_ACT_ENABLED) << 24))
}

fn spells() -> crate::ui_action::Spells {
    let spell = |id: u32, name: &str| {
        (
            id,
            SpellDisplay {
                id,
                name: name.into(),
                icon: Some(format!("Interface\\Icons\\Spell_{id}")),
                ..Default::default()
            },
        )
    };
    crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            [spell(CLAW, "Claw"), spell(GROWL, "Growl")]
                .into_iter()
                .collect::<HashMap<_, _>>(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// The client in the world as a hunter, our pet streamed in, summoned by us, abandonable and part
/// way through its level, and its bar: Claw, Stay, Attack, the mode Follow, Claw in its book.
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
    app.insert_non_send_resource(UiScript::new().expect("a VM"));
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client { app, inbound, sent };
    let mut bar = PetSpells {
        pet_guid: PET,
        state: 1 << 8,
        spells: vec![spell_word(CLAW)],
        ..Default::default()
    };
    bar.bar[0] = spell_word(CLAW);
    bar.bar[1] = PetActionEntry::from(STAY);
    bar.bar[2] = PetActionEntry::from(ATTACK);
    c.frame(vec![
        vec![created(ME, EntityKind::Player, &[(BYTES_0, 3 << 8)])],
        vec![created(
            PET,
            EntityKind::Unit,
            &[
                (SUMMONEDBY, ME as u32),
                (FLAGS, ABANDON),
                (PETNUMBER, 7),
                (PETXP, 4200),
                (PETNEXTXP, 8000),
            ],
        )],
    ]);
    // The bar after the world entry's reset, which our create's cascade runs (`0x4bc6f0`).
    c.frame(vec![vec![SessionEvent::PetSpells(Box::new(bar))]]);
    c.frame(vec![]);
    c.wire();
    c
}

/// Each getter follows the world at the call, with no push between: the mode word, the latch,
/// the pet's flags and fields, the expiry and the book's words, as their handlers and verbs leave
/// them; with no host, the VM's copy answers, which nothing pushes.
#[test]
fn the_pet_getters_read_the_world_at_the_call() {
    let mut c = client();
    let bar = "local n, _, _, _, stay = GetPetActionInfo(2) return PetHasActionBar(), \
               GetPetActionsUsable(), (GetPetActionInfo(1)), n, stay, IsPetAttackActive(3)";
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>, String, String, Option<i64>, bool)>(true, bar),
        (
            Some(1),
            Some(1),
            "Claw".into(),
            "PET_ACTION_WAIT".into(),
            None,
            false
        )
    );
    assert_eq!(
        c.eval::<Option<i64>>(false, "return PetHasActionBar()"),
        None,
        "with no host the copy answers"
    );

    // Stay as a press leaves the mode word, the latch up as a possessed unit's Attack raises it,
    // and the pet stunned: the token lights, Attack is active, the bar greys (`0x4bd075`).
    c.bar().spells.state = 0;
    c.app
        .world_mut()
        .resource_mut::<crate::ui_pet::PetAttackLatch>()
        .0 = Some(PET);
    c.write_pet(&[(FLAGS, ABANDON | STUNNED)]);
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>, String, String, Option<i64>, bool)>(true, bar),
        (
            Some(1),
            None,
            "Claw".into(),
            "PET_ACTION_WAIT".into(),
            Some(1),
            true
        )
    );

    // The stat block and the menu off the pet's fields (`0x4be670`, `0x4be840`, `0x4be500`).
    let pet = "local ui, hunter = HasPetUI() local xp, next = GetPetExperience() \
               return ui, hunter, xp, next, PetCanBeAbandoned()";
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>, f64, f64, Option<i64>)>(true, pet),
        (Some(1), Some(1), 4200.0, 8000.0, Some(1))
    );
    c.write_pet(&[(FLAGS, 0), (PETXP, 5000)]);
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>, f64, f64, Option<i64>)>(true, pet),
        (Some(1), Some(1), 5000.0, 8000.0, None)
    );

    // A charm's expiry and a second spell in the book, as the next packet would leave them.
    c.bar().expires = Some(Instant::now() + Duration::from_secs(30));
    c.bar().spells.spells.push(spell_word(GROWL));
    let (left, book) = c.eval::<(f64, i64)>(true, "return GetPetTimeRemaining(), (HasPetSpells())");
    assert!((29_000.0..=30_000.0).contains(&left), "{left} ms to go");
    assert_eq!(book, 2);
    assert_eq!(
        c.eval::<String>(true, "return (GetSpellName(2, BOOKTYPE_PET))"),
        "Growl"
    );

    // The guid gone, as the world entry's reset leaves it: no bar and no pet UI (`0x4be67e`).
    c.bar().spells.pet_guid = 0;
    assert_eq!(
        c.eval::<(Option<i64>, Option<String>, Option<i64>)>(
            true,
            "return PetHasActionBar(), (GetPetActionInfo(1)), (HasPetUI())"
        ),
        (None, None, None)
    );
}

/// `PetWait` through the dispatcher (`0x4be480` → `0x4bd1d0`): with the pet stunned the gate
/// refuses it before the write, the signal and the send (`0x4bd1f2`); usable, the mode word is
/// written and `PET_BAR_UPDATE` fires inside the call (`0x4bc979`), its listener reading Stay lit
/// and sending ahead of the order (`0x4bd444`).
#[test]
fn a_state_press_signals_in_its_call_before_its_send() {
    let mut c = client();
    c.eval::<()>(
        true,
        "HEARD, LIT = 0, nil local f = CreateFrame('Frame') f:RegisterEvent('PET_BAR_UPDATE') \
         f:SetScript('OnEvent', function() HEARD = HEARD + 1 \
         local _, _, _, _, a = GetPetActionInfo(2) LIT = a RandomRoll(1, 100) end)",
    );
    c.write_pet(&[(FLAGS, ABANDON | STUNNED)]);
    c.eval::<()>(true, "PetWait()");
    assert_eq!(
        c.eval::<i64>(true, "return HEARD"),
        0,
        "the gate refused it"
    );
    assert!(c.wire().is_empty());
    assert_eq!(c.bar().spells.state, 1 << 8, "nothing written");

    c.write_pet(&[(FLAGS, ABANDON)]);
    c.eval::<()>(true, "PetWait()");
    assert_eq!(c.eval::<i64>(true, "return HEARD"), 1, "once, in the call");
    assert_eq!(c.eval::<Option<i64>>(true, "return LIT"), Some(1));
    assert_eq!(c.wire(), ["RandomRoll", "PetAction"]);
    assert_eq!(c.bar().spells.state, 0);
}
