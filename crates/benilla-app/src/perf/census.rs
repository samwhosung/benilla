//! Env-gated one-shot and per-second counters that check a performance premise before a fix.

use bevy::prelude::*;

/// `WOW_CPU_CENSUS=<at>:<secs>`: the pill's cpu-ms over a window, thread by thread (macOS). Two
/// snapshots of the process clock and every live thread's CPU bracket the window; the residual,
/// process delta minus the live-thread deltas, is the CPU of threads that exited in the window.
/// `pth_user_time`/`pth_system_time` are nanoseconds, not mach ticks.
#[cfg(target_os = "macos")]
pub(super) mod cpu_census {
    use std::collections::HashMap;

    use bevy::prelude::*;
    use bevy::time::Real;

    use crate::perf::clock::process_cpu_secs;

    /// `sys/proc_info.h`; not in the libc crate.
    const PROC_PIDLISTTHREADS: libc::c_int = 6;

    /// One cumulative reading per live thread: `tid → (name, user_ns, system_ns)`; mostly-system
    /// time points at kernel churn (park/wake, semaphores) rather than a system's body.
    fn thread_snapshot() -> HashMap<u64, (String, u64, u64)> {
        let pid = std::process::id() as libc::c_int;
        let mut tids = [0u64; 512];
        // SAFETY: proc_pidinfo writes at most `buffersize` bytes into the buffer and returns
        // how many it wrote; the buffer is a plain u64 array.
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                PROC_PIDLISTTHREADS,
                0,
                tids.as_mut_ptr().cast(),
                std::mem::size_of_val(&tids) as libc::c_int,
            )
        };
        if n <= 0 {
            return HashMap::new();
        }
        let count = n as usize / std::mem::size_of::<u64>();
        let mut out = HashMap::with_capacity(count);
        for &tid in &tids[..count] {
            // SAFETY: zeroed is a valid proc_threadinfo (plain integers + a char array);
            // proc_pidinfo fills it and returns the size written.
            let mut ti: libc::proc_threadinfo = unsafe { std::mem::zeroed() };
            let r = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTHREADINFO,
                    tid,
                    (&raw mut ti).cast(),
                    std::mem::size_of::<libc::proc_threadinfo>() as libc::c_int,
                )
            };
            if r as usize != std::mem::size_of::<libc::proc_threadinfo>() {
                continue; // the thread died mid-sweep: its time lands in the residual
            }
            let name_bytes: Vec<u8> = ti
                .pth_name
                .iter()
                .take_while(|&&c| c != 0)
                .map(|&c| c as u8)
                .collect();
            let name = String::from_utf8_lossy(&name_bytes).into_owned();
            out.insert(tid, (name, ti.pth_user_time, ti.pth_system_time));
        }
        out
    }

    struct Snap {
        t: f32,
        frames: u64,
        process_secs: f64,
        threads: HashMap<u64, (String, u64, u64)>,
    }

    /// Armed from the env at plugin build, on the main thread, which is how `main_tid` is known.
    #[derive(Resource)]
    pub(in crate::perf) struct CpuCensus {
        at: f32,
        window: f32,
        main_tid: u64,
        frames: u64,
        start: Option<Snap>,
        /// Every thread the once-a-second sweeps saw in the window, with its latest reading, so
        /// a thread that dies mid-window can be named in the residual.
        seen: HashMap<u64, (String, u64, u64)>,
        next_sweep: f32,
        done: bool,
    }

    impl CpuCensus {
        /// Parse `WOW_CPU_CENSUS=<at>:<secs>`. Must be called on the main thread.
        pub(in crate::perf) fn from_env() -> Option<Self> {
            let v = std::env::var("WOW_CPU_CENSUS").ok()?;
            let (at, window) = v.split_once(':')?;
            let mut main_tid = 0u64;
            // SAFETY: a null pthread means the calling thread; writes one u64.
            unsafe { libc::pthread_threadid_np(0, &mut main_tid) };
            Some(Self {
                at: at.parse().ok()?,
                window: window.parse().ok()?,
                main_tid,
                frames: 0,
                start: None,
                seen: HashMap::new(),
                next_sweep: 0.0,
                done: false,
            })
        }
    }

    pub(in crate::perf) fn cpu_census(mut census: ResMut<CpuCensus>, time: Res<Time<Real>>) {
        if census.done {
            return;
        }
        census.frames += 1;
        let now = time.elapsed_secs();
        if census.start.is_none() {
            if now >= census.at {
                census.start = Some(Snap {
                    t: now,
                    frames: census.frames,
                    process_secs: process_cpu_secs().unwrap_or(0.0),
                    threads: thread_snapshot(),
                });
            }
            return;
        }
        if now >= census.next_sweep {
            census.next_sweep = now + 1.0;
            for (tid, entry) in thread_snapshot() {
                census.seen.insert(tid, entry);
            }
        }
        let start = census.start.as_ref().unwrap();
        if now - start.t < census.window {
            return;
        }
        let end_process = process_cpu_secs().unwrap_or(0.0);
        let end_threads = thread_snapshot();
        let frames = (census.frames - start.frames).max(1);
        let dt = now - start.t;
        let process_ms = (end_process - start.process_secs) * 1000.0;
        let per_frame = |ms: f64| ms / frames as f64;

        // A thread born inside the window has no start reading; its whole time is its delta.
        let mut rows: Vec<(f64, f64, String)> = Vec::new(); // (user_ms, sys_ms, label)
        let mut live_sum_ms = 0.0f64;
        for (tid, (name, end_u, end_s)) in &end_threads {
            let (start_u, start_s) = start.threads.get(tid).map_or((0, 0), |(_, u, s)| (*u, *s));
            let user_ms = end_u.saturating_sub(start_u) as f64 / 1e6;
            let sys_ms = end_s.saturating_sub(start_s) as f64 / 1e6;
            live_sum_ms += user_ms + sys_ms;
            // Hex tids so a row can be matched against `/usr/bin/sample`'s "Thread 0x…" headers.
            let label = if *tid == census.main_tid {
                format!("main (tid 0x{tid:x})")
            } else if name.is_empty() {
                format!("(unnamed tid 0x{tid:x})")
            } else {
                format!("{name} (tid 0x{tid:x})")
            };
            rows.push((user_ms, sys_ms, label));
        }
        rows.sort_by(|a, b| (b.0 + b.1).total_cmp(&(a.0 + a.1)));
        let residual_ms = process_ms - live_sum_ms;

        eprintln!(
            "[cpu-census] window {dt:.2} s, {frames} frames ({:.1} fps), process {process_ms:.1} ms \
             = {:.4} ms/frame  (the pill's number over this window)",
            frames as f32 / dt,
            per_frame(process_ms),
        );
        eprintln!(
            "[cpu-census] {:>9}  {:>8}  {:>8}  {:>6}  thread",
            "ms/frame", "user", "sys", "share"
        );
        for (u, s, label) in &rows {
            let ms = u + s;
            if per_frame(ms) < 0.0005 {
                continue; // folded into the "under" line below, still in the printed sum
            }
            eprintln!(
                "[cpu-census] {:>9.4}  {:>8.4}  {:>8.4}  {:>5.1}%  {label}",
                per_frame(ms),
                per_frame(*u),
                per_frame(*s),
                ms / process_ms * 100.0
            );
        }
        let tiny: f64 = rows
            .iter()
            .filter(|(u, s, _)| per_frame(u + s) < 0.0005)
            .map(|(u, s, _)| u + s)
            .sum();
        let tiny_n = rows
            .iter()
            .filter(|(u, s, _)| per_frame(u + s) < 0.0005)
            .count();
        if tiny_n > 0 {
            eprintln!(
                "[cpu-census] {:>9.4}  {:>5.1}%  ({tiny_n} threads under 0.5 µs/frame each)",
                per_frame(tiny),
                tiny / process_ms * 100.0
            );
        }
        eprintln!(
            "[cpu-census] {:>9.4}  {:>5.1}%  (residual: threads exited in-window + sweep skew)",
            per_frame(residual_ms),
            residual_ms / process_ms * 100.0
        );
        // Threads the sweeps saw that are gone by the end: their last reading is a lower bound
        // on their share of the residual.
        let mut churned: HashMap<String, (usize, f64)> = HashMap::new();
        for (tid, (name, last_u, last_s)) in &census.seen {
            if end_threads.contains_key(tid) {
                continue;
            }
            let (start_u, start_s) = start.threads.get(tid).map_or((0, 0), |(_, u, s)| (*u, *s));
            let e = churned.entry(name.clone()).or_insert((0, 0.0));
            e.0 += 1;
            e.1 += (last_u.saturating_sub(start_u) + last_s.saturating_sub(start_s)) as f64 / 1e6;
        }
        for (name, (n, ms)) in &churned {
            eprintln!(
                "[cpu-census]   churn: {n} thread(s) '{}' died in-window, ≥{:.4} ms/frame of the residual",
                if name.is_empty() { "(unnamed)" } else { name },
                per_frame(*ms)
            );
        }
        eprintln!(
            "[cpu-census] sum check: Σ live {live_sum_ms:.1} + residual {residual_ms:.1} \
             = {:.1} vs process {process_ms:.1} ms (identity, by construction)",
            live_sum_ms + residual_ms
        );
        census.done = true;
    }
}

/// `WOW_MESH_EVENTS=1`: per-second Mesh asset-event counts, with a few Modified ids.
pub(super) fn count_mesh_events(
    mut events: MessageReader<bevy::asset::AssetEvent<Mesh>>,
    time: Res<Time>,
    mut acc: Local<(f32, u32, u32, u32, Vec<String>)>,
) {
    let (last, added, modified, removed, sample) = &mut *acc;
    for e in events.read() {
        match e {
            bevy::asset::AssetEvent::Added { .. } => *added += 1,
            bevy::asset::AssetEvent::Modified { id } => {
                *modified += 1;
                if sample.len() < 4 {
                    sample.push(format!("{id:?}"));
                }
            }
            bevy::asset::AssetEvent::Removed { .. } | bevy::asset::AssetEvent::Unused { .. } => {
                *removed += 1;
            }
            bevy::asset::AssetEvent::LoadedWithDependencies { .. } => {}
        }
    }
    if time.elapsed_secs() - *last >= 1.0 {
        eprintln!(
            "[mesh-events] added={added} modified={modified} removed={removed}/s sample={sample:?}"
        );
        (*added, *modified, *removed) = (0, 0, 0);
        sample.clear();
        *last = time.elapsed_secs();
    }
}

/// `WOW_PART_CHURN=1`: per-second M2-part churn. `rm_frames` counts frames with a
/// `MeshMaterial3d<WowModelMaterial>` removal, the predicate that sends `classify_water_side` to
/// its full walk; `mat_mod` counts the material Modified events that wake `AssetChanged` scans.
pub(super) fn count_part_churn(
    added: Query<(), Added<MeshMaterial3d<benilla_assets::materials::WowModelMaterial>>>,
    mut removed: RemovedComponents<MeshMaterial3d<benilla_assets::materials::WowModelMaterial>>,
    mut mat_events: MessageReader<
        bevy::asset::AssetEvent<benilla_assets::materials::WowModelMaterial>,
    >,
    time: Res<Time>,
    mut acc: Local<(f32, u32, u32, u32, u32)>,
) {
    let (last, add_n, rm_n, rm_frames, mat_mod) = &mut *acc;
    *add_n += added.iter().count() as u32;
    let rm = removed.read().count() as u32;
    *rm_n += rm;
    if rm > 0 {
        *rm_frames += 1;
    }
    *mat_mod += mat_events
        .read()
        .filter(|e| matches!(e, bevy::asset::AssetEvent::Modified { .. }))
        .count() as u32;
    if time.elapsed_secs() - *last >= 1.0 {
        eprintln!(
            "[part-churn] parts +{add_n} -{rm_n} rm_frames={rm_frames}/s mat_modified={mat_mod}/s"
        );
        (*add_n, *rm_n, *rm_frames, *mat_mod) = (0, 0, 0, 0);
        *last = time.elapsed_secs();
    }
}

/// `WOW_MESH_HOLDERS=1`: once a second, the archetype of each entity holding a mesh Modified in
/// that second. Exclusive, since the signature needs the live archetype.
pub(super) fn mesh_holders(
    world: &mut World,
    mut cursor: Local<Option<bevy::ecs::message::MessageCursor<bevy::asset::AssetEvent<Mesh>>>>,
    mut acc: Local<(f32, std::collections::HashSet<bevy::asset::AssetId<Mesh>>)>,
) {
    let messages = world.resource::<bevy::ecs::message::Messages<bevy::asset::AssetEvent<Mesh>>>();
    let cursor = cursor.get_or_insert_with(|| messages.get_cursor());
    let (last, ids) = &mut *acc;
    ids.extend(cursor.read(messages).filter_map(|e| match e {
        bevy::asset::AssetEvent::Modified { id } => Some(*id),
        _ => None,
    }));
    let now = world.resource::<Time>().elapsed_secs();
    if now - *last < 1.0 {
        return;
    }
    *last = now;
    if ids.is_empty() {
        return;
    }
    let short = |full: &str| -> String {
        let base = full.split('<').next().unwrap_or(full);
        let segs: Vec<&str> = base.split("::").collect();
        segs[segs.len().saturating_sub(2)..].join("::")
    };
    let mut found = 0usize;
    let mut holders = world.query::<(Entity, &Mesh3d)>();
    let matches: Vec<Entity> = holders
        .iter(world)
        .filter(|(_, m)| ids.contains(&m.0.id()))
        .map(|(e, _)| e)
        .take(6)
        .collect();
    for e in matches {
        found += 1;
        let sig: Vec<String> = world
            .entity(e)
            .archetype()
            .components()
            .iter()
            .filter_map(|&c| world.components().get_info(c))
            .map(|i| short(&i.name().to_string()))
            .collect();
        eprintln!(
            "[mesh-holders] {e} holds a modified mesh: {}",
            sig.join("+")
        );
    }
    if found == 0 {
        eprintln!(
            "[mesh-holders] {} modified id(s), no Mesh3d holder (2d or unowned)",
            ids.len()
        );
    }
    ids.clear();
}

/// When the one-shot archetype census fires, in seconds of `Time`; `f32::MAX` once spent.
#[derive(Resource)]
pub(super) struct ArchCensusAt(pub(super) f32);

/// `WOW_ARCH_CENSUS`: the entity total, then the 60 largest archetypes' entity counts and
/// component sets.
pub(super) fn arch_census(world: &mut World) {
    let due = world.resource::<ArchCensusAt>().0;
    if world.resource::<bevy::time::Time>().elapsed_secs() < due {
        return;
    }
    world.resource_mut::<ArchCensusAt>().0 = f32::MAX;
    let short = |full: &str| -> String {
        let base = full.split('<').next().unwrap_or(full);
        let segs: Vec<&str> = base.split("::").collect();
        segs[segs.len().saturating_sub(2)..].join("::")
    };
    let mut rows: Vec<(u32, String)> = world
        .archetypes()
        .iter()
        .filter(|a| !a.is_empty())
        .map(|a| {
            let mut names: Vec<String> = a
                .components()
                .iter()
                .filter_map(|&c| world.components().get_info(c))
                .map(|i| short(&i.name().to_string()))
                .collect();
            names.sort();
            (a.len(), names.join("+"))
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let total: u32 = rows.iter().map(|r| r.0).sum();
    eprintln!(
        "[census] {} entities across {} archetypes",
        total,
        rows.len()
    );
    for (n, sig) in rows.iter().take(60) {
        eprintln!("[census] {n:>7}  {sig}");
    }
}

/// `WOW_CAM_CHANGED=1`: per-second count of frames whose world-camera `Transform` /
/// `GlobalTransform` registered as changed.
pub(super) fn count_camera_changes(
    t_changed: Query<(), (With<benilla_world::view::WorldCamera>, Changed<Transform>)>,
    g_changed: Query<
        (),
        (
            With<benilla_world::view::WorldCamera>,
            Changed<GlobalTransform>,
        ),
    >,
    time: Res<Time>,
    mut acc: Local<(f32, u32, u32, u32)>,
) {
    let (last, frames, t_n, g_n) = &mut *acc;
    *frames += 1;
    *t_n += u32::from(!t_changed.is_empty());
    *g_n += u32::from(!g_changed.is_empty());
    if time.elapsed_secs() - *last >= 1.0 {
        eprintln!("[cam-changed] frames={frames} transform={t_n} global={g_n}/s");
        (*frames, *t_n, *g_n) = (0, 0, 0);
        *last = time.elapsed_secs();
    }
}

/// One live static row's clonable component set.
type BloatSource<'w, 's> = Query<
    'w,
    's,
    (
        &'static Mesh3d,
        &'static MeshMaterial3d<benilla_assets::materials::WowModelMaterial>,
        &'static benilla_world::model_render::ModelPart,
        &'static bevy::mesh::MeshTag,
        &'static bevy::camera::primitives::Aabb,
    ),
    Without<benilla_world::rig_palette::RigPart>,
>;

/// `WOW_ROW_BLOAT=<n>`: once a static model row exists, spawn `n` inert clones of it parked
/// 10,000 yd underground, so the walks that scale with total rows pay for them and the
/// per-visible work never sees them; a leg A/B reads d(cpu_ms)/d(rows).
pub(super) fn row_bloat(mut commands: Commands, mut done: Local<bool>, source: BloatSource) {
    if *done {
        return;
    }
    let Some((mesh, mat, part, tag, aabb)) = source.iter().next() else {
        return; // no static row streamed yet
    };
    let n: usize = std::env::var("WOW_ROW_BLOAT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for _ in 0..n {
        commands.spawn((
            Mesh3d(mesh.0.clone()),
            MeshMaterial3d(mat.0.clone()),
            Transform::from_xyz(0.0, -10_000.0, 0.0),
            *part,
            tag.clone(),
            *aabb,
            bevy::camera::visibility::NoAutoAabb,
        ));
    }
    eprintln!(
        "[row-bloat] spawned {n} inert static rows (cloned a live world row, parked at y=-10000)"
    );
    *done = true;
}

/// `WOW_MESH_TOUCH=<secs>`: from `secs` on, mark one scratch [`Mesh`] modified every frame and
/// nothing else, pricing one arming of [`bevy::asset::AssetChanged`] as a within-run delta. Any
/// `Assets<Mesh>` modification, even a 2D UI mesh, arms a walk of every `Mesh3d` row per
/// registered material type.
pub(super) fn mesh_touch(
    mut meshes: ResMut<Assets<Mesh>>,
    time: Res<bevy::time::Time<bevy::time::Real>>,
    at: Res<MeshTouchAt>,
    mut scratch: Local<Option<Handle<Mesh>>>,
) {
    if time.elapsed_secs() < at.0 {
        return;
    }
    let handle = scratch.get_or_insert_with(|| {
        meshes.add(Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        ))
    });
    // `get_mut` writes `AssetEvent::Modified`, bumping the tick every `AssetChanged` filter reads.
    let _ = meshes.get_mut(&*handle);
}

/// When [`mesh_touch`] starts touching (seconds of `Time<Real>`).
#[derive(Resource)]
pub(super) struct MeshTouchAt(pub f32);

/// `WOW_RES_CENSUS=<at>:<secs>`: on how many of the window's frames each resource read as
/// changed, noisiest first. Bevy marks a resource changed on every `&mut` borrow, not every
/// write, so one per-frame `ResMut<T>` disarms every gate on `T.is_changed()`. `NonSend`
/// resources are not walked: the world has no iterator over them.
pub(super) mod res_census {
    use std::collections::HashMap;

    use bevy::ecs::change_detection::Tick;
    use bevy::ecs::component::ComponentId;
    use bevy::prelude::*;
    use bevy::time::Real;

    #[derive(Resource)]
    pub(in crate::perf) struct ResCensus {
        at: f32,
        window: f32,
        /// The tick this census last counted at, so a touch counts once per frame.
        last_run: Option<Tick>,
        frames: u32,
        counts: HashMap<ComponentId, u32>,
        done: bool,
    }

    impl ResCensus {
        /// Parse `WOW_RES_CENSUS=<at>:<secs>`.
        pub(in crate::perf) fn from_env() -> Option<Self> {
            let v = std::env::var("WOW_RES_CENSUS").ok()?;
            let (at, window) = v.split_once(':')?;
            Some(Self {
                at: at.parse().ok()?,
                window: window.parse().ok()?,
                last_run: None,
                frames: 0,
                counts: HashMap::new(),
                done: false,
            })
        }
    }

    pub(in crate::perf) fn res_census(world: &mut World) {
        let now = world.resource::<Time<Real>>().elapsed_secs();
        let this_run = world.read_change_tick();
        // `resource_scope` lifts the census out of the world for the walk, so it never counts
        // its own borrow.
        world.resource_scope(|world, mut census: Mut<ResCensus>| {
            if census.done || now < census.at {
                return;
            }
            let Some(last_run) = census.last_run.replace(this_run) else {
                return; // the window's first frame: arm the tick, count from the next
            };
            if now >= census.at + census.window {
                census.done = true;
                report(world, &census);
                return;
            }
            census.frames += 1;
            let changed: Vec<ComponentId> = world
                .iter_resources()
                .map(|(info, _)| info.id())
                .filter(|&id| {
                    world
                        .get_resource_change_ticks_by_id(id)
                        .is_some_and(|t| t.is_changed(last_run, this_run))
                })
                .collect();
            for id in changed {
                *census.counts.entry(id).or_insert(0) += 1;
            }
        });
    }

    fn report(world: &World, census: &ResCensus) {
        let total = world.iter_resources().count();
        let mut rows: Vec<(u32, String)> = census
            .counts
            .iter()
            .map(|(&id, &n)| {
                let name = world
                    .components()
                    .get_info(id)
                    .map(|i| i.name().shortname().to_string())
                    .unwrap_or_else(|| format!("{id:?}"));
                (n, name)
            })
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let frames = census.frames.max(1);
        let hot = |n: u32| n * 10 >= frames * 9;
        // Bevy's own asset stores read hot every frame by design, so they are one count, not
        // rows.
        let bevy_store =
            |name: &str| name.starts_with("Assets<") || name.starts_with("AssetChanges<");
        let every_frame = rows.iter().filter(|(n, _)| hot(*n)).count();
        let bevy_hot = rows
            .iter()
            .filter(|(n, name)| hot(*n) && bevy_store(name))
            .count();
        let mut out = format!(
            "RES_CENSUS frames={frames} resources={total} touched={} at_90pct_or_more={every_frame} (of which bevy asset stores: {bevy_hot})\n",
            rows.len()
        );
        // Every row at half the window or more, unabridged.
        for (n, name) in rows
            .iter()
            .filter(|(n, name)| *n * 2 >= frames && !bevy_store(name))
        {
            out.push_str(&format!("  {n:>5}/{frames}  {name}\n"));
        }
        eprint!("{out}");
    }
}
