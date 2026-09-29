//! The feed's cost per invalidation on the real data, run by hand:
//! `cargo test --release -p benilla-app --lib ui_tooltip::bench -- --ignored --nocapture`.
//! `BENCH_N` sets how many spells the feed has pushed (2000 by default).

use std::time::{Duration, Instant};

use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::feed_spell_tooltips;
use crate::items::Items;
use crate::net::{GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};

/// The mage's family in `Spell.dbc`.
const MAGE: u32 = 3;

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[test]
#[ignore = "a measurement, not a check"]
fn feed_cost_per_invalidation() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = Spells {
        catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
        forms: benilla_formats::load_shapeshift_forms(&mut chain).expect("forms"),
        ranges: benilla_formats::load_spell_ranges(&mut chain).expect("ranges"),
        cast_times: benilla_formats::load_spell_cast_times(&mut chain).expect("cast times"),
        durations: benilla_formats::load_spell_durations(&mut chain).expect("durations"),
        radii: benilla_formats::load_spell_radii(&mut chain).expect("radii"),
    };
    let skill_lines = benilla_formats::load_skill_line_catalog(&mut chain).expect("skill lines");
    let n: usize = std::env::var("BENCH_N")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(2000);

    // What a mage has pushed: its own family's spells, then an even sample of every other kind,
    // as the auras of other units and the trainer's services fill the set.
    let mut described: Vec<u32> = spells
        .catalog
        .iter()
        .filter(|(_, d)| d.description.is_some() && !d.name.is_empty())
        .map(|(id, _)| id)
        .collect();
    described.sort_unstable();
    let (mut ids, others): (Vec<u32>, Vec<u32>) = described.into_iter().partition(|&id| {
        spells
            .catalog
            .get(id)
            .is_some_and(|d| d.spell_family == MAGE)
    });
    let class = ids.len().min(n);
    ids.truncate(class);
    let stride = (others.len() / n.saturating_sub(class).max(1)).max(1);
    ids.extend(others.iter().step_by(stride).take(n - class));
    let mut lines: Vec<u32> = ids
        .iter()
        .filter_map(|&id| skill_lines.spell_to_line(id))
        .collect();
    lines.sort_unstable();
    lines.dedup();

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(spells)
        .insert_resource(NetCommands(tx))
        .insert_resource(crate::ui_spellbook::SkillLines {
            catalog: skill_lines,
        })
        .init_resource::<Items>()
        .init_resource::<GuidIndex>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<PlayerActions>()
        .add_systems(Update, feed_spell_tooltips);
    app.world_mut()
        .resource_mut::<PlayerActions>()
        .spells
        .extend(ids.iter().copied());
    app.world_mut()
        .resource_mut::<crate::spell::SpellModifiers>()
        .set_class_family(MAGE);

    // A level-60 mage: unit fields, the four percentages, the skills of its lines.
    let player_fields = |crit: f32, dodge: f32, reach: f32, form: u32, head: u32, skill: u32| {
        let mut pairs: Vec<(u16, u32)> = vec![
            (22, 3500),
            (28, 4000),
            (34, 60),
            (162, 1373),
            (130, reach.to_bits()),
            (138, form << 16),
            (486, head),
            (1106, 5.0f32.to_bits()),
            (1107, dodge.to_bits()),
            (1108, 4.0f32.to_bits()),
            (1109, crit.to_bits()),
        ];
        for (slot, &line) in lines.iter().take(128).enumerate() {
            let base = 718 + 3 * slot as u16;
            pairs.push((base, line));
            pairs.push((base + 1, if slot == 0 { skill } else { 300 } | 300 << 16));
        }
        ObjectStore(ObjectFields::from_pairs(&pairs))
    };
    let player = app
        .world_mut()
        .spawn((SelfPlayer, player_fields(5.0, 4.0, 1.5, 0, 0, 300)))
        .id();
    // An auto-attack target, whose reach a swap moves.
    const TARGET: u64 = 0xF130_0000_0000_0042;
    let target_fields =
        |reach: f32| ObjectStore(ObjectFields::from_pairs(&[(130, reach.to_bits())]));
    let target = app.world_mut().spawn(target_fields(1.5)).id();
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(TARGET, target);

    let script = UiScript::new().expect("VM");
    crate::ui_script::load_ui_for_test(&script, "Interface\\FrameXML\\GlobalStrings.lua");
    app.insert_non_send_resource(script);

    let timed = |app: &mut App| {
        let start = Instant::now();
        app.update();
        start.elapsed()
    };
    let first = timed(&mut app);
    let steady = median((0..50).map(|_| timed(&mut app)).collect());
    println!(
        "pushed {} spells ({} of them the mage's): first frame {:?}, a quiet frame {:?}",
        ids.len(),
        class,
        first,
        steady
    );

    // Each cause moves, alternating between two values, and the frame after it is timed.
    let run = |name: &str, app: &mut App, step: &mut dyn FnMut(&mut App, bool)| {
        let times: Vec<Duration> = (0..40)
            .map(|i| {
                step(app, i % 2 == 0);
                timed(app)
            })
            .collect();
        let mean = times.iter().sum::<Duration>() / times.len() as u32;
        println!(
            "{name:<28} median {:>9.1?}  mean {:>9.1?}",
            median(times),
            mean
        );
    };
    // Engage the target first: the reach cause is the attack target's.
    let set_player = move |app: &mut App, s: ObjectStore| {
        app.world_mut().entity_mut(player).insert(s);
    };
    run("crit percentage", &mut app, &mut |app, a| {
        let crit = if a { 5.0 } else { 6.5 };
        set_player(app, player_fields(crit, 4.0, 1.5, 0, 0, 300));
    });
    run("dodge percentage", &mut app, &mut |app, a| {
        let dodge = if a { 4.0 } else { 5.5 };
        set_player(app, player_fields(5.0, dodge, 1.5, 0, 0, 300));
    });
    run("caster combat reach", &mut app, &mut |app, a| {
        let reach = if a { 1.5 } else { 2.0 };
        set_player(app, player_fields(5.0, 4.0, reach, 0, 0, 300));
    });
    app.world_mut()
        .entity_mut(player)
        .insert(crate::creature_anim::Engaged(TARGET));
    run("attack target's reach", &mut app, &mut |app, a| {
        let reach = if a { 1.5 } else { 3.0 };
        app.world_mut()
            .entity_mut(target)
            .insert(target_fields(reach));
    });
    run("shapeshift form", &mut app, &mut |app, a| {
        set_player(app, player_fields(5.0, 4.0, 1.5, u32::from(!a), 0, 300));
    });
    run("one worn slot", &mut app, &mut |app, a| {
        set_player(app, player_fields(5.0, 4.0, 1.5, 0, u32::from(!a), 300));
    });
    run("one skill line's value", &mut app, &mut |app, a| {
        let skill = if a { 300 } else { 299 };
        set_player(app, player_fields(5.0, 4.0, 1.5, 0, 0, skill));
    });
    run("one modifier cell (narrow)", &mut app, &mut |app, a| {
        let mut mods = app
            .world_mut()
            .resource_mut::<crate::spell::SpellModifiers>();
        mods.set(
            false,
            5,
            crate::spell::OP_CAST_TIME,
            if a { -10 } else { -20 },
        );
    });
    // A proc that changes the cost of everything the class casts (Clearcasting's shape): one
    // packet per family-flag bit.
    run("all modifier bits (proc)", &mut app, &mut |app, a| {
        let mut mods = app
            .world_mut()
            .resource_mut::<crate::spell::SpellModifiers>();
        for bit in 0..64 {
            mods.set(false, bit, crate::spell::OP_COST, if a { -100 } else { 0 });
        }
    });
}
