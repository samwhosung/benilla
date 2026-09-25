//! `WOW_SCHED_CENSUS=1`: per schedule in both worlds, every system with its executor flags
//! (non-`Send`, exclusive, has-deferred). Bevy removes a running schedule from `Schedules`
//! (`World::schedule_scope`), so two vantages per world cover each other: `Update`/`PostUpdate`
//! in main, `ExtractSchedule`/`Render` in the render app. Only the `Main`/`RenderStartup` runners
//! stay unseen.

use bevy::prelude::*;
use bevy::render::{ExtractSchedule, Render, RenderApp};

/// The frame to dump at: late enough that every schedule has run once.
const CENSUS_FRAME: u32 = 10;

/// The frame the run exits at; the pipelined render app runs a frame behind [`CENSUS_FRAME`].
const EXIT_FRAME: u32 = 40;

/// The census; the graph is fixed once plugins build, so a login-screen run answers for all states.
pub(crate) struct SchedCensusPlugin;

impl Plugin for SchedCensusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DumpedSchedules>()
            .add_systems(Update, census_vantage("main", "Update"))
            .add_systems(PostUpdate, census_vantage("main", "PostUpdate"))
            .add_systems(Last, census_exit);
        // The render app is still a sub-app here; pipelining detaches it at cleanup.
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<DumpedSchedules>()
                .add_systems(ExtractSchedule, census_vantage("render", "Extract"))
                .add_systems(Render, census_vantage("render", "Render"));
        }
    }
}

/// Schedules this world has printed; the later vantage adds only what the earlier could not see.
#[derive(Resource, Default)]
struct DumpedSchedules(std::collections::HashSet<String>);

/// One vantage: at [`CENSUS_FRAME`], prints every visible schedule this world has not printed.
fn census_vantage(
    world_tag: &'static str,
    vantage: &'static str,
) -> impl FnMut(&mut World, Local<u32>) {
    move |world: &mut World, mut frame: Local<u32>| {
        *frame += 1;
        if *frame != CENSUS_FRAME {
            return;
        }
        world.resource_scope::<DumpedSchedules, _>(|world, mut dumped| {
            world.resource_scope::<Schedules, _>(|_, schedules| {
                let mut labels: Vec<(String, &bevy::ecs::schedule::Schedule)> = schedules
                    .iter()
                    .map(|(label, sched)| (format!("{label:?}"), sched))
                    .filter(|(name, _)| !dumped.0.contains(name))
                    .collect();
                labels.sort_by(|a, b| a.0.cmp(&b.0));
                for (name, sched) in labels {
                    dump_schedule(world_tag, vantage, &name, sched);
                    dumped.0.insert(name);
                }
            });
        });
    }
}

/// Prints one schedule's tally, then one line per system. By [`CENSUS_FRAME`] only a schedule that
/// never ran is uninitialized, and it is flagged.
fn dump_schedule(
    world_tag: &str,
    vantage: &str,
    name: &str,
    sched: &bevy::ecs::schedule::Schedule,
) {
    let Ok(systems) = sched.systems() else {
        println!(
            "SCHED_CENSUS world={world_tag} vantage={vantage} schedule={name} \
             n={} uninitialized=1",
            sched.systems_len()
        );
        return;
    };
    let (mut n, mut nonsend, mut exclusive, mut deferred) = (0u32, 0u32, 0u32, 0u32);
    let mut rows: Vec<String> = Vec::new();
    for (_, sys) in systems {
        n += 1;
        let mut flags = String::new();
        if !sys.is_send() {
            nonsend += 1;
            flags.push('N');
        }
        if sys.is_exclusive() {
            exclusive += 1;
            flags.push('X');
        }
        if sys.has_deferred() {
            deferred += 1;
            flags.push('D');
        }
        rows.push(format!(
            "SCHED_SYS world={world_tag} schedule={name} flags={} name={}",
            if flags.is_empty() { "-".into() } else { flags },
            sys.name()
        ));
    }
    println!(
        "SCHED_CENSUS world={world_tag} vantage={vantage} schedule={name} \
         n={n} nonsend={nonsend} exclusive={exclusive} deferred={deferred}"
    );
    for row in rows {
        println!("{row}");
    }
}

/// Exits once both worlds have printed.
fn census_exit(mut frame: Local<u32>, mut exit: MessageWriter<AppExit>) {
    *frame += 1;
    if *frame == EXIT_FRAME {
        println!("SCHED_CENSUS_DONE");
        exit.write(AppExit::Success);
    }
}
