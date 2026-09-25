//! Two one-shot dumps of the live `World`: the bevy_ui node inventory ([`NodeProbePlugin`]) and
//! the archetype census ([`EntityCensusPlugin`]), each printed without the plumbing components.

use bevy::prelude::*;

/// `WOW_NODE_PROBE=<secs>`: once, one line per `ComputedNode` entity with its rect (logical px,
/// y-down), visibility and components: the UI outside the FrameXML quad pass, which
/// `WOW_UI_PROBE` cannot see.
pub(crate) struct NodeProbePlugin;

impl Plugin for NodeProbePlugin {
    fn build(&self, app: &mut App) {
        let at = std::env::var("WOW_NODE_PROBE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10.0);
        app.insert_resource(NodeProbe { at, fired: false })
            .add_systems(Update, fire_node_probe);
    }
}

/// [`NodeProbePlugin`] state.
#[derive(Resource)]
struct NodeProbe {
    at: f32,
    fired: bool,
}

fn fire_node_probe(world: &mut World) {
    {
        let time = world.resource::<Time>().elapsed_secs();
        let probe = world.resource::<NodeProbe>();
        if probe.fired || time < probe.at {
            return;
        }
    }
    world.resource_mut::<NodeProbe>().fired = true;
    let scale = world
        .query::<&bevy::window::Window>()
        .iter(world)
        .next()
        .map_or(1.0, bevy::window::Window::scale_factor);
    // `UiGlobalTransform`: a bevy UI entity carries no `GlobalTransform`.
    let mut q = world.query::<(
        Entity,
        &bevy::ui::ComputedNode,
        &bevy::ui::UiGlobalTransform,
        Option<&InheritedVisibility>,
    )>();
    let rows: Vec<(Entity, Vec2, Vec2, bool)> = q
        .iter(world)
        .map(|(e, node, gt, vis)| (e, node.size(), gt.translation, vis.is_none_or(|v| v.get())))
        .collect();
    // Zero nodes is an anomaly: a bare screen or a query that no longer matches.
    if rows.is_empty() {
        warn!("node probe: NO ui nodes matched — the screen is bare, or this probe has rotted");
    }
    info!("node probe: {} nodes, scale {scale}", rows.len());
    for (e, size, center, vis) in rows {
        let comps: Vec<String> = world.inspect_entity(e).map_or_else(
            |_| Vec::new(),
            |it| {
                it.map(|c| c.name().shortname().to_string())
                    .filter(|n| {
                        // Drop the plumbing components.
                        !matches!(
                            n.as_str(),
                            "Transform"
                                | "GlobalTransform"
                                | "Visibility"
                                | "InheritedVisibility"
                                | "ViewVisibility"
                                | "ChildOf"
                                | "Children"
                        )
                    })
                    .collect()
            },
        );
        // ComputedNode is physical px; translation is the node's center, also physical.
        info!(
            "node probe: [{:.0},{:.0} {:.0}x{:.0}] vis={} {:?}",
            (center.x - size.x * 0.5) / scale,
            (center.y - size.y * 0.5) / scale,
            size.x / scale,
            size.y / scale,
            vis,
            comps
        );
    }
}

/// `WOW_ENTITY_CENSUS=<secs>` (real seconds): once, one line per archetype, largest first, with
/// its entity count and signal components, then a summary.
pub(crate) struct EntityCensusPlugin;

impl Plugin for EntityCensusPlugin {
    fn build(&self, app: &mut App) {
        let at = std::env::var("WOW_ENTITY_CENSUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10.0);
        app.insert_resource(EntityCensus { at, fired: false })
            .add_systems(Update, fire_entity_census);
    }
}

/// [`EntityCensusPlugin`] state.
#[derive(Resource)]
struct EntityCensus {
    at: f32,
    fired: bool,
}

/// Archetype lines the census prints; everything smaller folds into the summary's `other_n`.
const ENTITY_CENSUS_ROWS: usize = 60;

/// Signal components shown per archetype line.
const ENTITY_CENSUS_COMPS: usize = 14;

fn fire_entity_census(world: &mut World) {
    {
        // Real seconds, to compose with `WOW_LIVE_FPS_AT`; virtual time lags by the load stalls.
        let time = world.resource::<Time<bevy::time::Real>>().elapsed_secs();
        let probe = world.resource::<EntityCensus>();
        if probe.fired || time < probe.at {
            return;
        }
    }
    world.resource_mut::<EntityCensus>().fired = true;

    // `RigAnchor`s with no `Children` host nothing: no attachment, emitter, ribbon or card.
    {
        let mut q = world.query_filtered::<Option<&bevy::prelude::Children>, bevy::prelude::With<benilla_world::rig_anim::RigAnchor>>();
        let (mut total, mut childless) = (0u32, 0u32);
        for kids in q.iter(world) {
            total += 1;
            if kids.is_none_or(|k| k.is_empty()) {
                childless += 1;
            }
        }
        eprintln!(
            "ENTITY_CENSUS_ANCHORS total={total} childless={childless} ({:.1}%) hosting={}",
            100.0 * f32::from(u16::try_from(childless).unwrap_or(u16::MAX))
                / f32::from(u16::try_from(total.max(1)).unwrap_or(u16::MAX)),
            total - childless
        );
    }
    let components = world.components();
    let mut rows: Vec<(usize, bool, String)> = world
        .archetypes()
        .iter()
        .filter(|a| !a.is_empty())
        .map(|a| {
            let full: Vec<String> = a
                .components()
                .iter()
                .filter_map(|id| components.get_info(*id))
                .map(|c| c.name().shortname().to_string())
                .collect();
            // `vis=y`: bevy's `check_visibility` sweeps these rows once per active camera.
            let in_vis_population = full.iter().any(|n| n == "ViewVisibility");
            let signal: Vec<String> = full
                .iter()
                .filter(|n| {
                    // Drop the plumbing components.
                    !matches!(
                        n.as_str(),
                        "Transform"
                            | "GlobalTransform"
                            | "Visibility"
                            | "InheritedVisibility"
                            | "ViewVisibility"
                            | "ChildOf"
                            | "Children"
                    )
                })
                .cloned()
                .collect();
            // With one signal component or none, the plumbing tells archetypes apart: print it all.
            let names = if signal.len() <= 1 { full } else { signal };
            let shown = names.len().min(ENTITY_CENSUS_COMPS);
            let more = names.len() - shown;
            let mut comps = names[..shown].join(", ");
            if more > 0 {
                comps.push_str(&format!(" +{more}"));
            }
            (a.len() as usize, in_vis_population, comps)
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let (total_arch, total_n) = (rows.len(), rows.iter().map(|r| r.0).sum::<usize>());
    let vis_n = rows.iter().filter(|r| r.1).map(|r| r.0).sum::<usize>();
    let other_n = rows
        .iter()
        .skip(ENTITY_CENSUS_ROWS)
        .map(|r| r.0)
        .sum::<usize>();
    for (n, vis, comps) in rows.iter().take(ENTITY_CENSUS_ROWS) {
        println!(
            "ENTITY_CENSUS_ARCH n={n} vis={} comps=[{comps}]",
            if *vis { "y" } else { "n" }
        );
    }
    println!(
        "ENTITY_CENSUS total={total_n} vis_n={vis_n} archetypes={total_arch} \
         rows={} other_n={other_n}",
        rows.len().min(ENTITY_CENSUS_ROWS),
    );
}
