//! The feed re-pushing only the views a change reaches, through [`feed_spell_tooltips`] itself.

use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;

use super::spell_feed::feed_spell_tooltips;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};
use bevy::prelude::*;

/// `SpellRange.dbc` rows 2, the melee row, and 4, an authored 0 to 35.
fn melee_and_authored_rows() -> benilla_formats::SpellRangeCatalog {
    let row = |min, max, flags| benilla_formats::SpellRange { min, max, flags };
    benilla_formats::SpellRangeCatalog::from_rows(
        [(2, row(0.0, 5.0, 1)), (4, row(0.0, 35.0, 0))].into(),
    )
}

/// A creature on a live spline at `speed` yd/s over two seconds: a mover at that speed.
fn on_spline(speed: f32) -> crate::net::Spline {
    crate::net::Spline {
        points: vec![[0.0; 3], [speed * 2.0, 0.0, 0.0]],
        start: std::time::Instant::now(),
        duration: std::time::Duration::from_secs(2),
        id: 1,
        grounded: true,
        run_mode: true,
        deck: None,
    }
}

fn walking_and_running_speeds() -> crate::net::UnitSpeeds {
    crate::net::UnitSpeeds(benilla_protocol::MoveSpeeds {
        walk: 2.5,
        run: 7.0,
        run_back: 4.5,
        swim: 4.7,
        swim_back: 2.5,
        turn_rate: 3.1,
    })
}

/// A feed over a bare app: the book holds `make(1)`'s spells, the player is `fields`, and each
/// description names the generation of the catalog it was built from. The feed re-pushes a view
/// only when it must, so which generation a tooltip shows says which views were rebuilt.
struct FeedApp {
    app: App,
    player: Entity,
    ids: Vec<u32>,
    make: Box<dyn Fn(u32) -> Vec<(u32, benilla_formats::SpellDisplay)>>,
}

impl FeedApp {
    fn new(
        make: impl Fn(u32) -> Vec<(u32, benilla_formats::SpellDisplay)> + 'static,
        fields: &[(u16, u32)],
    ) -> Self {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<PlayerActions>()
            .add_systems(Update, feed_spell_tooltips);
        let player = app
            .world_mut()
            .spawn((SelfPlayer, ObjectStore(ObjectFields::from_pairs(fields))))
            .id();
        let ids: Vec<u32> = make(1).iter().map(|&(id, _)| id).collect();
        app.world_mut().resource_mut::<PlayerActions>().spells = ids.iter().copied().collect();
        let mut script = UiScript::new().unwrap();
        script.set_spellbook(benilla_ui::script::SpellBookState {
            tabs: Vec::new(),
            slots: ids
                .iter()
                .map(|&id| benilla_ui::script::SpellSlotView {
                    spell_id: id,
                    name: format!("Spell {id}"),
                    ..Default::default()
                })
                .collect(),
        });
        script
            .run(
                r#"
                local a = CreateFrame("Button", "B"); a:SetPoint("CENTER", 0, 0)
                a:SetWidth(10); a:SetHeight(10)
                CreateFrame("GameTooltip", "TT")
                function TEXT(i)
                    TT:SetOwner(B, "ANCHOR_RIGHT"); TT:SetSpell(i, "spell")
                    local t = {}
                    for k = 1, TT:NumLines() do t[k] = getglobal("TTTextLeft" .. k):GetText() end
                    return table.concat(t, " | ")
                end
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let mut feed = FeedApp {
            app,
            player,
            ids,
            make: Box::new(make),
        };
        feed.catalog(1);
        feed.app.update();
        feed
    }

    /// The catalog as generation `generation`.
    fn catalog(&mut self, generation: u32) {
        let catalog = (self.make)(generation).into_iter().collect();
        self.app.insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(catalog),
            ranges: melee_and_authored_rows(),
            ..Spells::empty_for_tests()
        });
    }

    /// Set the player's fields.
    fn set_player(&mut self, fields: &[(u16, u32)]) {
        self.app
            .world_mut()
            .entity_mut(self.player)
            .insert(ObjectStore(ObjectFields::from_pairs(fields)));
    }

    /// The generation each spell's tooltip shows, in book order.
    fn generations(&mut self) -> Vec<u32> {
        let script = self.app.world().non_send_resource::<UiScript>();
        (1..=self.ids.len())
            .map(|i| {
                let text: String = script.eval(&format!("return TEXT({i})")).unwrap();
                text.split(" | ")
                    .find_map(|line| line.strip_prefix("gen"))
                    .and_then(|g| g.parse().ok())
                    .unwrap_or_else(|| panic!("no generation in {text:?}"))
            })
            .collect()
    }
}

/// A dodge percentage moving rebuilds the dodge spell's view and no other: the parry and crit
/// spells read other percentages, the plain spell none. A change no view read rebuilds nothing.
#[test]
fn a_dodge_change_rebuilds_only_the_dodge_chance_view() {
    let spell = |effect: u32, passive: bool, generation: u32| benilla_formats::SpellDisplay {
        name: "Spell".into(),
        effects: [effect, 0, 0],
        passive,
        description: Some(format!("gen{generation}")),
        ..Default::default()
    };
    let spells = move |g: u32| {
        vec![
            (1, spell(20, true, g)),  // chance to dodge
            (2, spell(22, true, g)),  // chance to parry
            (3, spell(78, false, g)), // chance to crit
            (4, spell(0, false, g)),  // no chance line
            (5, spell(20, false, g)), // the dodge effect without the passive gate: no line
        ]
    };
    let player = |dodge: f32, health: u32| {
        [
            (22, health),
            (1107, dodge.to_bits()),
            (1108, 6.0f32.to_bits()),
            (1109, 7.0f32.to_bits()),
        ]
    };
    let mut feed = FeedApp::new(spells, &player(5.0, 100));
    assert_eq!(feed.generations(), [1, 1, 1, 1, 1], "every view is pushed");

    feed.catalog(2);
    feed.set_player(&player(9.5, 100));
    feed.app.update();
    assert_eq!(
        feed.generations(),
        [2, 1, 1, 1, 1],
        "only the dodge view read the dodge percentage"
    );

    // Health is nothing a view reads.
    feed.catalog(3);
    feed.set_player(&player(9.5, 60));
    feed.app.update();
    assert_eq!(feed.generations(), [2, 1, 1, 1, 1], "nothing was rebuilt");

    // The other two percentages, one frame each.
    feed.set_player(&[
        (22, 60),
        (1107, 9.5f32.to_bits()),
        (1108, 8.0f32.to_bits()),
        (1109, 7.0f32.to_bits()),
    ]);
    feed.app.update();
    assert_eq!(feed.generations(), [2, 3, 1, 1, 1], "parry");
    feed.catalog(4);
    feed.set_player(&[
        (22, 60),
        (1107, 9.5f32.to_bits()),
        (1108, 8.0f32.to_bits()),
        (1109, 1.0f32.to_bits()),
    ]);
    feed.app.update();
    assert_eq!(feed.generations(), [2, 3, 4, 1, 1], "crit");
}

/// A modifier packet rebuilds the views of the spells whose family bit it names, for the class the
/// tables gate on, and no other; a new class family rebuilds them all.
#[test]
fn a_modifier_change_rebuilds_only_the_matching_familys_views() {
    let spell = |family: u32, bits: &[u32], generation: u32| benilla_formats::SpellDisplay {
        name: "Spell".into(),
        spell_family: family,
        spell_family_flags: bits.iter().fold(0u64, |mask, b| mask | 1 << b),
        description: Some(format!("gen{generation}")),
        ..Default::default()
    };
    let spells = move |g: u32| {
        vec![
            (10, spell(3, &[5], g)),    // a mage spell on bit 5
            (11, spell(3, &[6], g)),    // a mage spell on bit 6
            (12, spell(8, &[5], g)),    // a rogue spell on the same bit
            (13, spell(0, &[5], g)),    // no family: never modified
            (14, spell(3, &[5, 6], g)), // a mage spell on both
        ]
    };
    let mut feed = FeedApp::new(spells, &[(22, 100)]);
    let mods = |feed: &mut FeedApp, f: &dyn Fn(&mut crate::spell::SpellModifiers)| {
        f(&mut feed
            .app
            .world_mut()
            .resource_mut::<crate::spell::SpellModifiers>());
        feed.app.update();
    };
    // The class family arrives after the first push.
    feed.catalog(2);
    mods(&mut feed, &|m| m.set_class_family(3));
    assert_eq!(
        feed.generations(),
        [2, 2, 2, 2, 2],
        "a new class family rebuilds every view"
    );

    feed.catalog(3);
    mods(&mut feed, &|m| m.set(true, 5, crate::spell::OP_COST, -10));
    assert_eq!(
        feed.generations(),
        [3, 2, 2, 2, 3],
        "the mage spells with bit 5"
    );

    // The other table, another op, the other bit.
    feed.catalog(4);
    mods(&mut feed, &|m| {
        m.set(false, 6, crate::spell::OP_CAST_TIME, 20)
    });
    assert_eq!(feed.generations(), [3, 4, 2, 2, 4], "bit 6");

    // A bit no spell carries, and a packet that restates a cell, rebuild nothing.
    feed.catalog(5);
    mods(&mut feed, &|m| m.set(true, 40, crate::spell::OP_COST, 5));
    mods(&mut feed, &|m| m.set(true, 5, crate::spell::OP_COST, -10));
    assert_eq!(
        feed.generations(),
        [3, 4, 2, 2, 4],
        "the mage spells with bit 40"
    );
}

/// The melee range cell reads the two units' motion (`0x6e3648`-`0x6e36a2`): the caster and its
/// auto-attack target both running at run speed, or ceasing to, rebuilds the views on the melee
/// row and no other, and a change that leaves the bonus where it was rebuilds nothing.
#[test]
fn a_units_run_rebuilds_only_the_melee_range_views() {
    use crate::creature_anim::{move_flags, Engaged};
    use crate::net::Embodied;

    const MOB: u64 = 0xF130_0000_0000_0009;
    let spell = |range_index: u32, generation: u32| benilla_formats::SpellDisplay {
        name: "Spell".into(),
        range_index,
        description: Some(format!("gen{generation}")),
        ..Default::default()
    };
    let spells = move |g: u32| {
        vec![
            (30, spell(2, g)), // the melee row
            (31, spell(4, g)), // an authored row, which reads no unit
            (32, spell(0, g)), // no row at all
        ]
    };
    let mut feed = FeedApp::new(spells, &[(22, 100)]);
    let speeds = walking_and_running_speeds();
    let mob = feed
        .app
        .world_mut()
        .spawn((speeds, ObjectStore(ObjectFields::from_pairs(&[(22, 100)]))))
        .id();
    feed.app
        .world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(MOB, mob);
    let player = feed.player;
    feed.app
        .world_mut()
        .entity_mut(player)
        .insert((Embodied, speeds, Engaged(MOB)));
    let flags = |feed: &mut FeedApp, flags: u32| {
        feed.app
            .insert_resource(crate::player::Player::with_move_flags(flags));
        feed.app.update();
    };
    flags(&mut feed, 0);
    assert_eq!(feed.generations(), [1, 1, 1]);

    // The bonus needs both units running: the caster alone changes nothing a view shows.
    feed.catalog(2);
    flags(&mut feed, move_flags::FORWARD);
    assert_eq!(feed.generations(), [1, 1, 1], "the caster starts to run");

    feed.catalog(3);
    feed.app.world_mut().entity_mut(mob).insert(on_spline(8.0));
    feed.app.update();
    assert_eq!(
        feed.generations(),
        [3, 1, 1],
        "its target starts to run: the moving bonus"
    );

    // Still running, by another flag: each unit's answer is where it was.
    feed.catalog(4);
    flags(&mut feed, move_flags::FORWARD | move_flags::STRAFE_LEFT);
    assert_eq!(feed.generations(), [3, 1, 1], "nothing was rebuilt");

    // A walk is no run.
    feed.catalog(5);
    flags(&mut feed, move_flags::FORWARD | move_flags::WALK_MODE);
    assert_eq!(feed.generations(), [5, 1, 1], "the caster walks");
}

/// The pet's melee views read the pet's and its own auto-attack target's motion: a pet chasing a
/// running mob reads the moving bonus in its range cell, and stops when either halts.
#[test]
fn a_pet_view_reads_the_pets_and_its_targets_run() {
    use crate::creature_anim::Engaged;
    use benilla_ui::script::{PetBookState, SpellSlotView};

    const ME: u64 = 0x10;
    const IMP: u64 = 0xF140_0000_0000_0077;
    const MOB: u64 = 0xF130_0000_0000_0009;
    let claw = benilla_formats::SpellDisplay {
        id: 1082,
        name: "Claw".into(),
        range_index: 2,
        ..Default::default()
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(Spells {
        catalog: benilla_formats::SpellCatalog::from_displays([(1082, claw)].into()),
        ranges: melee_and_authored_rows(),
        ..Spells::empty_for_tests()
    })
    .insert_resource(NetCommands(tx))
    .init_resource::<Items>()
    .init_resource::<crate::net::GuidIndex>()
    .init_resource::<crate::spell::SpellModifiers>()
    .add_systems(Update, feed_spell_tooltips);
    let speeds = walking_and_running_speeds();
    let store = |pairs: &[(u16, u32)]| ObjectStore(ObjectFields::from_pairs(pairs));
    let me = app
        .world_mut()
        .spawn((
            SelfPlayer,
            store(&[(8, IMP as u32), (9, (IMP >> 32) as u32), (34, 60)]),
        ))
        .id();
    let imp = app
        .world_mut()
        .spawn((speeds, Engaged(MOB), store(&[(34, 14)])))
        .id();
    let mob = app.world_mut().spawn((speeds, store(&[(22, 100)]))).id();
    let mut index = app.world_mut().resource_mut::<crate::net::GuidIndex>();
    index.0.insert(ME, me);
    index.0.insert(IMP, imp);
    index.0.insert(MOB, mob);

    let mut script = UiScript::new().unwrap();
    script.set_pet_book(PetBookState {
        token: Some("CAT".into()),
        slots: vec![SpellSlotView {
            spell_id: 1082,
            name: "Claw".into(),
            ..Default::default()
        }],
    });
    script
        .run(
            r#"
            SPELL_RANGE = "%s yd range"
            local a = CreateFrame("Button", "B"); a:SetPoint("CENTER", 0, 0)
            a:SetWidth(10); a:SetHeight(10)
            CreateFrame("GameTooltip", "TT")
            function RANGE()
                TT:SetOwner(B, "ANCHOR_RIGHT"); TT:SetSpell(1, "pet")
                for k = 1, TT:NumLines() do
                    for _, side in ipairs({"TTTextLeft", "TTTextRight"}) do
                        local t = getglobal(side .. k):GetText()
                        if t and string.find(t, "yd range") then return t end
                    end
                end
            end
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    let range = |app: &mut App| {
        app.update();
        let script = app.world().non_send_resource::<UiScript>();
        script.eval::<Option<String>>("return RANGE()").unwrap()
    };
    let _ = range(&mut app);
    assert_eq!(range(&mut app).as_deref(), Some("5 yd range"), "both stand");
    app.world_mut().entity_mut(imp).insert(on_spline(8.0));
    assert_eq!(
        range(&mut app).as_deref(),
        Some("5 yd range"),
        "the pet runs, its target stands"
    );
    app.world_mut().entity_mut(mob).insert(on_spline(8.0));
    assert_eq!(
        range(&mut app).as_deref(),
        Some("8 yd range"),
        "both run: 5.0 + 2.6667"
    );
    app.world_mut()
        .entity_mut(mob)
        .remove::<crate::net::Spline>();
    assert_eq!(
        range(&mut app).as_deref(),
        Some("5 yd range"),
        "the target halts"
    );
}
