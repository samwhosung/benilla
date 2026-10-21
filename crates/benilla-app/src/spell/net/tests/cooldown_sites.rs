//! The cooldown arms of the spell packets, each pinned to the reference's site: the go's legs and
//! the item cooldown it holds for a record, and the not-ready cast result.

use super::*;
use benilla_formats::SpellDisplay;
use benilla_protocol::messages::{ItemInfo, ItemSpellEntry};
use benilla_protocol::ObjectFields;

/// The spell the go casts.
const SPELL: u32 = 2649;
const ME: u64 = 10;
/// The casting item's guid and entry.
const ITEM: u64 = 0x4000_0000_0000_0050;
const ENTRY: u32 = 6948;
/// `UNIT_FIELD_SUMMONEDBY`.
const SUMMONEDBY: u16 = 12;

/// One go's world: us (guid 10), `display` as [`SPELL`], and whatever the test adds.
struct GoRig {
    app: App,
    display: fn() -> SpellDisplay,
}

impl GoRig {
    fn new(display: fn() -> SpellDisplay) -> Self {
        use crate::combat_text::CombatTextSpawn;
        use crate::go_anim::GoLidOpen;
        use crate::net::{Guid, SelfPlayer};

        let mut spells = Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [(SPELL, display())].into_iter().collect(),
        );
        let mut app = App::new();
        app.add_message::<CastEvent>()
            .add_message::<SpellGoTargets>()
            .add_message::<CombatTextSpawn>()
            .add_message::<GoLidOpen>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<crate::creature_anim::UnitEdits>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::items::Items>()
            .insert_resource(spells);
        let me = app
            .world_mut()
            .spawn((Guid(ME), SelfPlayer, ObjectStore::default()))
            .id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, me);
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
        Self { app, display }
    }

    /// A streamed object of `guid`.
    fn with_object(mut self, guid: u64, store: ObjectStore) -> Self {
        let e = self
            .app
            .world_mut()
            .spawn((crate::net::Guid(guid), store))
            .id();
        self.app
            .world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
        self
    }

    /// The item cache's record of [`ENTRY`] with these spell blocks.
    fn with_record(mut self, blocks: Vec<ItemSpellEntry>) -> Self {
        self.app
            .world_mut()
            .resource_mut::<crate::items::Items>()
            .insert_template(ENTRY, Some(record(blocks)));
        self
    }

    /// `SMSG_SPELL_GO` of [`SPELL`] by `caster`, cast from `item` when given.
    fn go(&mut self, caster: u64, item: Option<u64>) {
        use crate::go_anim::GoLidOpen;
        use bevy::ecs::system::RunSystemOnce;
        let (tx, _rx) = crossbeam_channel::unbounded();
        self.app
            .world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      casting: Query<&Casting>,
                      mut cast_events: MessageWriter<CastEvent>,
                      mut go_targets: MessageWriter<SpellGoTargets>,
                      self_guid: Res<SelfGuid>,
                      stores: Query<&mut ObjectStore>,
                      mut go_lid: MessageWriter<GoLidOpen>,
                      mut cooldowns: ResMut<Cooldowns>,
                      mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                      mut items: ResMut<crate::items::Items>,
                      spells: Res<Spells>,
                      mods: Res<crate::spell::SpellModifiers>,
                      mut sheath: MessageWriter<crate::creature_anim::SheathRequest>,
                      mut stand: MessageWriter<crate::player::StandStateRequest>,
                      edits: Res<crate::creature_anim::UnitEdits>| {
                    let net_commands = crate::net::NetCommands(tx.clone());
                    spell_go(
                        caster,
                        SPELL,
                        0,
                        vec![],
                        vec![],
                        None,
                        None,
                        None,
                        None,
                        item,
                        &mut commands,
                        &index,
                        &casting,
                        &mut cast_events,
                        &mut go_targets,
                        &self_guid,
                        &stores,
                        &mut go_lid,
                        &mut crate::ui_loot::LootLatch::default(),
                        (
                            &mut cooldowns,
                            Some(&*spells),
                            &mut items,
                            &mut pet_bar,
                            &mods,
                        ),
                        (
                            &mut crate::spell::AutoRepeatActive::default(),
                            &mut sheath,
                            &mut stand,
                            &edits,
                            false,
                            crate::creature_anim::StartGate::default(),
                            &mut crate::spell::CastCore {
                                pending: &mut Default::default(),
                                queued_melee: &mut Default::default(),
                                ground: &mut Default::default(),
                                reports: &mut Default::default(),
                                net: &net_commands,
                            },
                        ),
                        1,
                    );
                },
            )
            .unwrap();
        crate::creature_anim::apply_unit_edits(self.app.world_mut());
    }

    fn cooldowns(&self) -> &Cooldowns {
        self.app.world().resource::<Cooldowns>()
    }

    fn display(&self) -> SpellDisplay {
        (self.display)()
    }

    /// The `(spell-keyed, item-keyed)` reads of [`SPELL`]'s duration on our list.
    fn reads(&self) -> (u32, u32) {
        let (d, now) = (self.display(), Instant::now());
        (
            self.cooldowns().info(SPELL, 0, Some(&d), now).duration_ms,
            self.cooldowns()
                .info(SPELL, ENTRY, Some(&d), now)
                .duration_ms,
        )
    }
}

/// An item object of [`ENTRY`] (`OBJECT_FIELD_TYPE` `0x3`, an item).
fn item_object() -> ObjectStore {
    ObjectStore(ObjectFields::from_pairs(&[(2, 0x3), (3, ENTRY)]))
}

/// A record of [`ENTRY`] holding `blocks`.
fn record(blocks: Vec<ItemSpellEntry>) -> ItemInfo {
    ItemInfo {
        spells: blocks,
        ..crate::items::test_template("Trinket")
    }
}

/// A spell block naming `spell_id` with a 7 s cooldown of its own.
fn block(index: u8, spell_id: u32, trigger: u32) -> ItemSpellEntry {
    ItemSpellEntry {
        index,
        spell_id,
        trigger,
        charges: 0,
        cooldown_ms: 7_000,
        category: 0,
        category_cooldown_ms: -1,
    }
}

/// A spell with no cooldown of its own and no GCD.
fn bare() -> SpellDisplay {
    SpellDisplay {
        name: "Bare".into(),
        ..Default::default()
    }
}

/// Growl's shape: a 5 s recovery.
fn growl() -> SpellDisplay {
    SpellDisplay {
        name: "Growl".into(),
        recovery_ms: 5_000,
        ..Default::default()
    }
}

/// The go's spell leg flushes after its insert whether or not a record went in (`0x6e8498`,
/// `0x6e849d`), and so does its pet leg (`0x6e85f7`, `0x6e85fc`, `0x6e8601`).
#[test]
fn every_own_and_pet_go_flushes_whether_or_not_it_inserts() {
    let mut rig = GoRig::new(bare);
    rig.go(ME, None);
    assert_eq!(rig.cooldowns().record_count(), 0);
    assert_eq!(rig.cooldowns().generation, 1, "our own go");

    let pet = ObjectStore(ObjectFields::from_pairs(&[
        (SUMMONEDBY, ME as u32),
        (SUMMONEDBY + 1, 0),
    ]));
    let mut rig = GoRig::new(bare).with_object(20, pet);
    rig.go(20, None);
    let pets = &rig
        .app
        .world()
        .resource::<crate::ui_pet::PetBar>()
        .cooldowns;
    assert_eq!(pets.record_count(), 0);
    assert_eq!(pets.generation, 1, "our pet's go");
}

/// The go's item leg reads the item's entry off its object (`0x6e8425`-`0x6e8437`): none inserts
/// and flushes nothing (`0x6e84a9`).
#[test]
fn an_item_go_from_an_unstreamed_item_inserts_and_flushes_nothing() {
    let mut rig = GoRig::new(growl);
    rig.go(ME, Some(ITEM));
    assert_eq!(rig.reads(), (0, 0));
    assert_eq!(rig.cooldowns().generation, 0);
}

/// With the record cached, the go's spell is searched among all five blocks whatever their trigger
/// (`0x6e84f0`) and a match inserts from that block, keyed by the item (`0x6e8511`-`0x6e8566`); no
/// match inserts nothing and flushes the item leg's pair (`0x6e84fd`-`0x6e8505`).
#[test]
fn an_item_go_inserts_from_the_block_naming_its_spell_or_nothing() {
    let mut rig = GoRig::new(growl)
        .with_object(ITEM, item_object())
        .with_record(vec![block(0, 9999, 0), block(1, SPELL, 2)]);
    rig.go(ME, Some(ITEM));
    assert_eq!(rig.reads(), (0, 7_000), "the item's record, from block 1");

    let mut rig = GoRig::new(growl)
        .with_object(ITEM, item_object())
        .with_record(vec![block(0, 9999, 0)]);
    let bags = rig.cooldowns().bag_flushes;
    rig.go(ME, Some(ITEM));
    assert_eq!(rig.reads(), (0, 0), "no block names the spell");
    assert_eq!(rig.cooldowns().generation, 1);
    assert_eq!(rig.cooldowns().bag_flushes, bags + 1);
}

/// The item leg inserts the block's values, or the spell's columns for a negative one, with no
/// modifier (`0x6e8511`-`0x6e8566`), where the spell leg applies op 11 (`0x6e846f`).
#[test]
fn an_item_go_applies_no_modifier() {
    fn display() -> SpellDisplay {
        SpellDisplay {
            spell_family: 9,
            spell_family_flags: 1,
            ..growl()
        }
    }
    let mut rig = GoRig::new(display)
        .with_object(ITEM, item_object())
        .with_record(vec![ItemSpellEntry {
            cooldown_ms: -1,
            ..block(0, SPELL, 0)
        }]);
    {
        let mut mods = rig
            .app
            .world_mut()
            .resource_mut::<crate::spell::SpellModifiers>();
        mods.set_class_family(9);
        mods.set(true, 0, crate::spell::OP_COOLDOWN, -2000);
    }
    rig.go(ME, Some(ITEM));
    assert_eq!(rig.reads(), (0, 5_000), "the spell's 5 s, unmodified");
    rig.go(ME, None);
    assert_eq!(rig.reads(), (3_000, 5_000), "the spell leg takes op 11");
}

/// A record not yet cached holds the cooldown under the item (`0x6e8660`) and ends the handler
/// (`0x6e84dd`); the record's arrival inserts it as the item leg does, with the go's time
/// (`0x6e8830`, `0x6e89a3`), and flushes the pair and the bags (`0x6e89a8`, `0x6e89ad`). A refusal
/// frees the node with nothing inserted (`0x6e88d8`).
#[test]
fn an_item_go_without_its_record_holds_the_cooldown_until_the_record_lands() {
    let mut rig = GoRig::new(growl).with_object(ITEM, item_object());
    rig.go(ME, Some(ITEM));
    assert_eq!(rig.reads(), (0, 0), "nothing at the go");
    let held_at = rig.cooldowns().generation;

    let landed = record(vec![block(2, SPELL, 0)]);
    let spells = Spells::empty_for_tests();
    let display = rig.display();
    let bags = rig.cooldowns().bag_flushes;
    let mut cds = rig.app.world_mut().resource_mut::<Cooldowns>();
    cds.item_record_landed(ENTRY, Some(&landed), |id| {
        (id == SPELL)
            .then_some(&display)
            .or_else(|| spells.catalog.get(id))
    });
    assert_eq!(cds.generation, held_at + 1);
    assert_eq!(cds.bag_flushes, bags + 1);
    assert_eq!(rig.reads(), (0, 7_000), "the item's record");

    // Once: the node is freed.
    let mut cds = rig.app.world_mut().resource_mut::<Cooldowns>();
    cds.item_record_landed(ENTRY, Some(&landed), |_| Some(&display));
    assert_eq!(rig.cooldowns().record_count(), 1);

    let mut rig = GoRig::new(growl).with_object(ITEM, item_object());
    rig.go(ME, Some(ITEM));
    let before = rig.cooldowns().generation;
    let mut cds = rig.app.world_mut().resource_mut::<Cooldowns>();
    cds.item_record_landed(ENTRY, None, |_| Some(&display));
    assert_eq!(cds.generation, before, "a refusal flushes nothing");
    cds.item_record_landed(ENTRY, Some(&landed), |_| Some(&display));
    assert_eq!(rig.reads(), (0, 0), "and leaves no node behind");
}

impl CastSeq {
    /// `SMSG_CAST_RESULT` with status 2, `reason` and the first dword `arg` (`None`: absent, -1).
    fn failed_with(&mut self, spell_id: u32, reason: u8, arg: Option<u32>) {
        self.app
            .world_mut()
            .run_system_once(
                move |edits: Res<crate::creature_anim::UnitEdits>,
                      self_guid: Res<SelfGuid>,
                      index: Res<GuidIndex>,
                      casting: Query<&Casting>,
                      mut cast_events: MessageWriter<CastEvent>,
                      mut core: crate::spell::CastSlots,
                      mut cooldowns: ResMut<Cooldowns>,
                      mut auto_repeat: ResMut<AutoRepeatActive>,
                      net: Res<crate::net::NetCommands>,
                      spells: Option<Res<Spells>>| {
                    cast_result(
                        spell_id,
                        false,
                        Some(reason),
                        arg,
                        &edits,
                        &self_guid,
                        &index,
                        &casting,
                        &mut cast_events,
                        &mut core.core(&net),
                        &mut cooldowns,
                        &mut auto_repeat,
                        spells.as_deref(),
                        None,
                        &net,
                        1,
                    );
                },
            )
            .unwrap();
        crate::creature_anim::apply_unit_edits(self.app.world_mut());
    }
}

/// `ReportCastFailure 0x6e1a00` for NOT_READY whose first dword is 0 (`0x6e1af1`): a held record of
/// the spell (`0x6e1690`, `0x6e1b02`) goes with all the spell's records (`0x6e3050(spell, 0, 0, 1)`,
/// `0x6e1b13`). An absent dword reads -1 (`0x6e736e`) and keeps it, as any nonzero one does.
#[test]
fn a_not_ready_result_with_argument_0_drops_the_held_record() {
    const SHADOWFORM: u32 = 15473;
    let form = || SpellDisplay {
        start_recovery_category: 133,
        start_recovery_ms: 1_500,
        attributes: 0x0200_0000,
        ..Default::default()
    };
    for (arg, held) in [(Some(0), false), (Some(7), true), (None, true)] {
        let t0 = Instant::now();
        let mut s = CastSeq::new();
        s.app.insert_resource(catalog_with(SHADOWFORM, form()));
        s.app.world_mut().resource_mut::<Cooldowns>().start_gcd(
            SHADOWFORM,
            &form(),
            t0,
            &crate::spell::SpellModifiers::default(),
        );
        s.commit(SHADOWFORM, None);
        s.failed_with(SHADOWFORM, 0x3c, arg);
        assert_eq!(
            s.app
                .world()
                .resource::<Cooldowns>()
                .has_on_hold_record(SHADOWFORM, 0, 0),
            held,
            "argument {arg:?}"
        );
    }
}
