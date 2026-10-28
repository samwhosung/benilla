//! The spell and aura views as the call host builds them at the setter's call ([`super::live`]):
//! the pet's view against the pet's reach and auto-attack target, the aura builder's view, and the
//! reagent line's carried test, each asked through a lent host as a setter asks it.

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay, SpellRange, SpellRangeCatalog};
use benilla_protocol::field::{
    FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::live::{LiveAsk, LiveQuery};
use benilla_ui::script::{
    AuraKey, AuraViewQuery, CallHost, CallTail, ScriptCall, SpellTooltipView, SpellViewQuery,
    UiScript,
};
use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use super::live::TooltipLive;
use crate::script_calls::live::LiveFamily;

const ME: u64 = 0x10;
const PET: u64 = 0xF140_0000_0000_0077;
const MOB: u64 = 0xF130_0000_0000_0001;
/// A melee spell on row 2, the one row with flags bit 0.
const STRIKE: u32 = 900_100;
/// A spell needing two of [`POWDER`].
const BRILLIANCE: u32 = 900_101;
const SHADOW_WORD: u32 = 900_102;
const POWDER: u32 = 17020;
/// `UNIT_FIELD_COMBATREACH`.
const REACH: u16 = 130;

fn reach(r: f32) -> (u16, u32) {
    (REACH, r.to_bits())
}

/// Our player, summoning [`PET`], beside [`MOB`]; the catalogs and the caches.
fn world() -> World {
    let mut world = World::new();
    // The template asks go nowhere: a send to a closed channel is dropped.
    let (tx, _) = crossbeam_channel::unbounded();
    world.insert_resource(crate::net::NetCommands(tx));
    world.init_resource::<crate::net::GuidIndex>();
    world.init_resource::<crate::items::Items>();
    world.init_resource::<crate::spell::SpellModifiers>();
    world.init_resource::<crate::creature_anim::UnitEdits>();
    let mut reagents = [(0, 0); 8];
    reagents[0] = (POWDER, 2);
    let mut spells = crate::ui_action::Spells::empty_for_tests();
    spells.catalog = SpellCatalog::from_displays(HashMap::from([
        (
            STRIKE,
            SpellDisplay {
                id: STRIKE,
                name: "Strike".into(),
                range_index: 2,
                ..Default::default()
            },
        ),
        (
            BRILLIANCE,
            SpellDisplay {
                id: BRILLIANCE,
                name: "Brilliance".into(),
                reagents,
                ..Default::default()
            },
        ),
        (
            SHADOW_WORD,
            SpellDisplay {
                id: SHADOW_WORD,
                name: "Shadow Word: Pain".into(),
                aura_description: Some("Shadow damage over time.".into()),
                description: Some("Not the aura's text.".into()),
                ..Default::default()
            },
        ),
    ]));
    spells.ranges = SpellRangeCatalog::from_rows(HashMap::from([(
        2,
        SpellRange {
            min: 0.0,
            max: 5.0,
            flags: 1,
        },
    )]));
    world.insert_resource(spells);
    let unit = |world: &mut World, guid: u64, pairs: &[(u16, u32)]| {
        let e = world
            .spawn((
                crate::net::Guid(guid),
                crate::net::ObjectStore(ObjectFields::from_pairs(pairs)),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(guid, e);
        e
    };
    // `UNIT_FIELD_SUMMON`, 8-9.
    let me = unit(&mut world, ME, &[(8, PET as u32), (9, (PET >> 32) as u32)]);
    world.entity_mut(me).insert(crate::net::SelfPlayer);
    unit(&mut world, PET, &[reach(4.0)]);
    unit(&mut world, MOB, &[reach(2.0)]);
    world.insert_non_send_resource(UiScript::new().expect("a VM"));
    world
}

/// `q`'s answer with a host lent that answers the tooltip family alone, as a setter asks it.
fn ask<Q: LiveQuery>(world: &mut World, q: Q) -> Option<Q::Answer> {
    struct Host<'a, 'w, 's>(&'a mut TooltipLive<'w, 's>);

    impl CallHost for Host<'_, '_, '_> {
        fn apply(&mut self, _: &mut UiScript, _: ScriptCall) -> Option<CallTail> {
            None
        }

        fn read_live(&mut self, ask: &mut LiveAsk<'_>) -> bool {
            self.0.answer(ask)
        }
    }

    let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
    let mut state = SystemState::<TooltipLive>::new(world);
    let lua = script.lua().clone();
    let answer = script.hosted(&mut Host(&mut state.get(world)), |_| {
        benilla_ui::script::live::ask(&lua, &q)
    });
    world.insert_non_send_resource(script);
    answer
}

fn spell(world: &mut World, spell_id: u32, pet: bool) -> SpellTooltipView {
    ask(world, SpellViewQuery { spell_id, pet }).expect("a view")
}

fn text(t: &benilla_ui::script::TooltipText) -> String {
    t.text(benilla_ui::strings::Strings::Host(&|key: &str| match key {
        "SPELL_RANGE" => Some("%s yd range".to_string()),
        "SPELL_REAGENTS" => Some("Reagents: ".to_string()),
        _ => None,
    }))
}

/// The unit selector builds against the pet (`0x6e3130`): the melee cell reads the pet's reach and
/// the pet's auto-attack target (`0x6e3480`, `[caster+0xc48]`), 4.0 + 2.0 + 1.3333334 = 7.333;
/// the player's view, with its default 1.5 and no target, floors at 5.
#[test]
fn the_pet_view_reads_the_pets_reach_and_target() {
    let mut world = world();
    let pet = world.resource::<crate::net::GuidIndex>().0[&PET];
    world.resource::<crate::creature_anim::UnitEdits>().push(
        crate::creature_anim::UnitEdit::StartAttack {
            entity: pet,
            target: MOB,
        },
    );
    let range = |world: &mut World, pet| spell(world, STRIKE, pet).range.as_ref().map(text);
    assert_eq!(range(&mut world, true).as_deref(), Some("7 yd range"));
    assert_eq!(range(&mut world, false).as_deref(), Some("5 yd range"));
    // The pet's target dropped: its own reach counts twice, 4.0 + 4.0 + 1.3333334.
    world
        .resource::<crate::creature_anim::UnitEdits>()
        .push(crate::creature_anim::UnitEdit::EndAttack(pet));
    assert_eq!(range(&mut world, true).as_deref(), Some("9 yd range"));
}

/// The aura builder's view at the call (`0x52f880`): the name and the aura text alone
/// (`0x52f929`); a spell with no record is declined, and the setter reads the store.
#[test]
fn the_aura_view_builds_at_the_call() {
    let mut world = world();
    let key = |spell_id| AuraKey {
        spell_id,
        level: 0,
        stack: 1,
    };
    let v = ask(&mut world, AuraViewQuery(key(SHADOW_WORD))).expect("a view");
    assert_eq!(v.name, "Shadow Word: Pain");
    assert_eq!(text(&v.description), "Shadow damage over time.");
    assert!(ask(&mut world, AuraViewQuery(key(1))).is_none());
}

/// The reagent line is red only when no copy is carried (`0x622270` at mask 0, `0x52f49f`): one of
/// the two the spell needs is carried, its count shown, never compared; a record the cache misses
/// leaves the name out and counts the miss (`0x52f3ff`).
#[test]
fn a_reagent_line_is_red_only_with_none_carried() {
    let mut world = world();
    let v = spell(&mut world, BRILLIANCE, false);
    assert_eq!(v.reagents, None);
    assert_eq!(v.item_misses, [POWDER]);
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(POWDER, Some(crate::items::test_template("Arcane Powder")));
    let line = |world: &mut World| {
        let v = spell(world, BRILLIANCE, false);
        assert!(v.item_misses.is_empty());
        (v.reagents.as_ref().map(text), v.reagents_met)
    };
    let red = Some("Reagents: |cffff2020Arcane Powder (2)|r".to_string());
    assert_eq!(line(&mut world), (red, false), "none carried");
    // One copy in the backpack's first slot (23).
    crate::items::test_spawn_item(
        &mut world,
        0xA1,
        ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, POWDER), (FIELD_ITEM_STACK_COUNT, 1)]),
        false,
    );
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    let slot = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
    world
        .get_mut::<crate::net::ObjectStore>(me)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[(slot, 0xA1), (slot + 1, 0)]));
    let carried = Some("Reagents: Arcane Powder (2)".to_string());
    assert_eq!(line(&mut world), (carried, true), "one of two carried");
}
