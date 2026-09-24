//! The pin probe: one reported spot replayed offline through the real flood, with the same
//! [`TraceLog`](super::super::probe) recorder as the in-client dump button; and the site
//! regressions pinned at such spots.

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use bevy::math::{Mat4, Vec3};

use super::super::seed::down_ray_claim;
use super::super::EXTERIOR_LIT;
use super::{
    load_subject, Site, BLACKSMITH, DARNASSUS, DEADMINES, EXTERIOR, FARGODEEP, GOLDSHIRE,
    IRONFORGE, SHADOWFANG, UNDERCITY,
};

/// The subjects `WOW_PIN_SITE` names: every [`Site`] the harness carries.
const NAMED_SITES: &[(&str, Site)] = &[
    ("goldshire", GOLDSHIRE),
    ("fargodeep", FARGODEEP),
    ("deadmines", DEADMINES),
    ("blacksmith", BLACKSMITH),
    ("undercity", UNDERCITY),
    ("ironforge", IRONFORGE),
    ("shadowfang", SHADOWFANG),
    ("darnassus", DARNASSUS),
];

/// `WOW_PIN_SITE=<name>` picks a [`NAMED_SITES`] subject (default [`UNDERCITY`]), and
/// `WOW_PIN_WMO`, `_MAP`, `_TILE` and `_UID` each override one field of it.
fn site_from_env() -> Site {
    let base = match std::env::var("WOW_PIN_SITE") {
        Ok(name) => {
            let key = name.trim().to_ascii_lowercase();
            *NAMED_SITES
                .iter()
                .find(|(n, _)| *n == key)
                .map(|(_, s)| s)
                .unwrap_or_else(|| {
                    let known: Vec<&str> = NAMED_SITES.iter().map(|(n, _)| *n).collect();
                    panic!("WOW_PIN_SITE={name:?} is not one of {known:?}")
                })
        }
        Err(_) => UNDERCITY,
    };
    Site {
        wmo: Box::leak(
            std::env::var("WOW_PIN_WMO")
                .unwrap_or_else(|_| base.wmo.to_string())
                .into_boxed_str(),
        ),
        map: Box::leak(
            std::env::var("WOW_PIN_MAP")
                .unwrap_or_else(|_| base.map.to_string())
                .into_boxed_str(),
        ),
        tile: std::env::var("WOW_PIN_TILE").map_or(base.tile, |s| {
            let (a, b) = s.split_once(',').expect("WOW_PIN_TILE wants tx,ty");
            (a.trim().parse().unwrap(), b.trim().parse().unwrap())
        }),
        uid: std::env::var("WOW_PIN_UID").map_or(base.uid, |s| s.trim().parse().unwrap()),
    }
}

/// The group census: every group's flags, portal edges and bbox in the model and world frames,
/// aimed by the same `WOW_PIN_*` variables as the pin probe.
#[test]
#[ignore = "instrument: aimed by WOW_PIN_*, run by hand — cargo test -p benilla-world --lib wmo_portal::audit::pin -- --ignored --nocapture"]
fn wmo_group_census() {
    let site = site_from_env();
    let subject = load_subject(site.wmo, Some(&site));
    let placed = subject
        .placed
        .as_ref()
        .expect("the census needs a placement");
    // Each group's liquid, read from its file: the local z range of its liquid vertices.
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let chain = benilla_formats::open_chain(&data).expect("open MPQ chain (set WOW_DATA)");
    let stem = site.wmo.strip_suffix(".wmo").unwrap_or(site.wmo);
    let liquid: Vec<Option<(f32, f32)>> = (0..subject.model.group_nav.len())
        .map(|gi| {
            let bytes = chain.read(&format!("{stem}_{gi:03}.wmo")).ok()?;
            let mesh = benilla_formats::wmo_group_liquid_mesh(&bytes)?;
            let zs = mesh.positions.iter().map(|p| p[2]);
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for z in zs {
                lo = lo.min(z);
                hi = hi.max(z);
            }
            (lo <= hi).then_some((lo, hi))
        })
        .collect();
    println!(
        "== group census: {} ({} groups) ==",
        site.wmo,
        subject.model.group_nav.len()
    );
    // The root's MFOG table, once; each group line below names up to four of its records. The
    // colour decodes as [`super::super::fog`] decodes it (`0xAARRGGBB`).
    for (i, f) in subject.model.fogs.iter().enumerate() {
        println!(
            "mfog f{i}: flags {:#x} pos ({:.1},{:.1},{:.1}) r[{:.2},{:.2}] end {:.1} start×{:.3} = {:.1} rgb({},{},{})",
            f.flags,
            f.pos[0],
            f.pos[1],
            f.pos[2],
            f.radius_inner,
            f.radius_outer,
            f.fog_end,
            f.fog_start_scalar,
            f.fog_end * f.fog_start_scalar,
            (f.color >> 16) & 0xff,
            (f.color >> 8) & 0xff,
            f.color & 0xff,
        );
    }
    // `WOW_PIN_COL=<world-x,world-y>`: every walking-collision face crossing that column, per group,
    // as world z; the down-ray's Leg A candidates for one spot.
    if let Some((cx, cy)) = std::env::var("WOW_PIN_COL").ok().and_then(|v| {
        let c: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (c.len() == 2).then(|| (c[0], c[1]))
    }) {
        let local = bevy_to_wow(
            placed
                .local_from_world
                .transform_point3(wow_to_bevy([cx, cy, 0.0])),
        );
        println!(
            "-- column ({cx:.1},{cy:.1}) = local ({:.2},{:.2}) --",
            local[0], local[1]
        );
        for (gi, tris) in subject.model.group_collision_tris.iter().enumerate() {
            for t in tris {
                if let Some(z) = benilla_formats::triangle_z_at(t, local[0], local[1]) {
                    let w = bevy_to_wow(
                        placed
                            .world_from_local
                            .transform_point3(wow_to_bevy([local[0], local[1], z])),
                    );
                    println!("  under g{gi:03}: local z {z:.2} = world z {:.1}", w[2]);
                }
            }
        }
        // Every face in the file at this column, with its MOPY flags. The reference's walking
        // reject mask `0x84` is DETAIL (`0x04`) plus a transient BSP visited bit (`0x80`), so it
        // skips exactly the faces ours does; a face without `0x04` missing above is a parse
        // divergence, not a filter choice.
        println!("-- raw file faces at the column (all MOPY flags) --");
        for gi in 0..subject.model.group_nav.len() {
            let Ok(gbytes) = chain.read(&format!("{stem}_{gi:03}.wmo")) else {
                continue;
            };
            let Ok(benilla_wmo::ParsedWmo::Group(group)) =
                benilla_wmo::parse_wmo(&mut std::io::Cursor::new(gbytes.as_slice()))
            else {
                continue;
            };
            for (t, mopy) in group.material_info.iter().enumerate() {
                let idx = |k: usize| group.vertex_indices.get(t * 3 + k).copied();
                let (Some(a), Some(b), Some(c)) = (idx(0), idx(1), idx(2)) else {
                    continue;
                };
                let pos = |i: u16| {
                    group
                        .vertex_positions
                        .get(i as usize)
                        .map(|p| [p.x, p.y, p.z])
                };
                let (Some(a), Some(b), Some(c)) = (pos(a), pos(b), pos(c)) else {
                    continue;
                };
                if let Some(z) = benilla_formats::triangle_z_at(&[a, b, c], local[0], local[1]) {
                    println!(
                        "  raw g{gi:03}: local z {z:.2}  mopy {:#04x} material {}",
                        mopy.flags, mopy.material_id
                    );
                }
            }
        }
    }
    // `WOW_PIN_FLOOR=<local-z>`: every walking-collision face whose centroid is within 3 yd of that
    // height, in world coordinates.
    if let Some(floor_z) = std::env::var("WOW_PIN_FLOOR")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        for (gi, tris) in subject.model.group_collision_tris.iter().enumerate() {
            for t in tris {
                let c = [
                    (t[0][0] + t[1][0] + t[2][0]) / 3.0,
                    (t[0][1] + t[1][1] + t[2][1]) / 3.0,
                    (t[0][2] + t[1][2] + t[2][2]) / 3.0,
                ];
                if (c[2] - floor_z).abs() > 3.0 {
                    continue;
                }
                let w = bevy_to_wow(placed.world_from_local.transform_point3(wow_to_bevy(c)));
                println!(
                    "floor g{gi:03} world ({:.1}, {:.1}, {:.1})",
                    w[0], w[1], w[2]
                );
            }
        }
    }
    for (gi, g) in subject.model.group_nav.iter().enumerate() {
        let mut wmin = [f32::INFINITY; 3];
        let mut wmax = [f32::NEG_INFINITY; 3];
        for c in 0..8 {
            let local = [
                if c & 1 == 0 {
                    g.bbox_min[0]
                } else {
                    g.bbox_max[0]
                },
                if c & 2 == 0 {
                    g.bbox_min[1]
                } else {
                    g.bbox_max[1]
                },
                if c & 4 == 0 {
                    g.bbox_min[2]
                } else {
                    g.bbox_max[2]
                },
            ];
            let w = bevy_to_wow(placed.world_from_local.transform_point3(wow_to_bevy(local)));
            for k in 0..3 {
                wmin[k] = wmin[k].min(w[k]);
                wmax[k] = wmax[k].max(w[k]);
            }
        }
        println!(
            "g{gi:03} flags {:#07x}{} edges {:2}  world x[{:.0},{:.0}] y[{:.0},{:.0}] z[{:.0},{:.0}]  local z[{:.1},{:.1}]  walk {:5} camonly {:4}{}",
            g.flags,
            if g.flags & EXTERIOR != 0 { " EXT" } else { "    " },
            g.ref_count,
            wmin[0],
            wmax[0],
            wmin[1],
            wmax[1],
            wmin[2],
            wmax[2],
            g.bbox_min[2],
            g.bbox_max[2],
            subject
                .model
                .group_collision_tris
                .get(gi)
                .map_or(0, Vec::len),
            subject
                .model
                .group_camera_only_tris
                .get(gi)
                .map_or(0, Vec::len),
            liquid[gi]
                .map(|(lo, hi)| format!("  LIQUID local z[{lo:.1},{hi:.1}]"))
                .unwrap_or_default(),
        );
        // The group's MFOG records, and whether it is on the interior fog lane at all
        // (`flags & 0x48 == 0`, the drawer router's own test).
        println!(
            "     fog {:?}{}",
            g.fog_indices,
            if g.flags & 0x48 == 0 {
                "  INTERIOR-LANE"
            } else {
                "  scene-lane (0x48 set)"
            }
        );
        // The group's portal edges in WoW world space, to name the portal at a `.go xyz` spot.
        let start = g.ref_start as usize;
        let end = (start + g.ref_count as usize).min(subject.model.portal_refs.len());
        for r in &subject.model.portal_refs[start..end] {
            let Some(info) = subject.model.portal_infos.get(r.portal as usize) else {
                continue;
            };
            let vs = (info.start_vertex as usize)
                ..(info.start_vertex as usize + info.count as usize)
                    .min(subject.model.portal_vertices.len());
            let verts = &subject.model.portal_vertices[vs];
            if verts.is_empty() {
                continue;
            }
            let mut c = [0.0f32; 3];
            for v in verts {
                for k in 0..3 {
                    c[k] += v[k] / verts.len() as f32;
                }
            }
            let w = bevy_to_wow(placed.world_from_local.transform_point3(wow_to_bevy(c)));
            // The widest chord across the polygon (yd): the doorway's opening.
            let span = verts
                .iter()
                .flat_map(|a| {
                    verts.iter().map(move |b| {
                        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2))
                            .sqrt()
                    })
                })
                .fold(0.0f32, f32::max);
            println!(
                "     p{:<3} ->g{:03}(side {:+}) world ({:.1},{:.1},{:.1}) span {:.1} yd",
                r.portal, r.group, r.side, w[0], w[1], w[2], span,
            );
        }
    }
}

/// The pin probe: a report's eye and look point, in WoW world coordinates, run through the real
/// flood offline, printing the down-ray's seed evidence, every portal hop's verdict and the visible
/// set. Its output is derived from the game data: keep it out of the repo.
///
/// ```text
/// WOW_PIN_EYE=1565.2,417.1,-56.2 WOW_PIN_LOOK=1517.5,406.7,-67.1 \
///   cargo test -p benilla-world --lib wmo_pin_probe -- --ignored --nocapture
/// ```
#[test]
#[ignore = "instrument: aimed by WOW_PIN_*, run by hand — cargo test -p benilla-world --lib wmo_portal::audit::pin -- --ignored --nocapture"]
fn wmo_pin_probe() {
    fn xyz(var: &str, default: [f32; 3]) -> [f32; 3] {
        let Ok(s) = std::env::var(var) else {
            return default;
        };
        let v: Vec<f32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        <[f32; 3]>::try_from(&v[..]).unwrap_or_else(|_| panic!("{var} wants x,y,z; got {s:?}"))
    }
    let site = site_from_env();
    let subject = load_subject(site.wmo, Some(&site));
    let placed = subject
        .placed
        .as_ref()
        .expect("the pin probe needs a placement");

    // The report's coordinates are WoW world space; the flood works in the placement's model space.
    let eye_world_wow = xyz("WOW_PIN_EYE", [1565.2, 417.1, -56.2]);
    let look_world_wow = xyz("WOW_PIN_LOOK", [1517.5, 406.7, -67.1]);
    let to_local =
        |wow: [f32; 3]| bevy_to_wow(placed.local_from_world.transform_point3(wow_to_bevy(wow)));
    let eye = to_local(eye_world_wow);
    let look = to_local(look_world_wow);

    // The camera as the per-frame pass builds it, in Bevy world space with the real placement.
    let eye_bevy = placed.world_from_local.transform_point3(wow_to_bevy(eye));
    let look_bevy = placed.world_from_local.transform_point3(wow_to_bevy(look));
    // The app's projection by default (the flood clips only against the side planes, so near and
    // far do not matter); `WOW_PIN_FOVY` and `WOW_PIN_ASPECT` replay a dump header's values.
    let scalar = |var: &str, default: f32| {
        std::env::var(var).map_or(default, |s| {
            s.trim()
                .parse()
                .unwrap_or_else(|_| panic!("{var} wants a number; got {s:?}"))
        })
    };
    let fovy = scalar("WOW_PIN_FOVY", crate::view::CAM_FOVY);
    let aspect = scalar("WOW_PIN_ASPECT", 16.0 / 9.0);
    let clip = Mat4::perspective_rh(fovy, aspect, 0.1, 1000.0)
        * Mat4::look_at_rh(eye_bevy, look_bevy, Vec3::Y);

    let model = &subject.model;
    println!(
        "== pin probe: {} ({} groups, {} portals) ==\n\
         eye  world ({:.2},{:.2},{:.2}) -> local ({:.2},{:.2},{:.2})\n\
         look world ({:.2},{:.2},{:.2}) -> local ({:.2},{:.2},{:.2})",
        site.wmo,
        model.group_nav.len(),
        model.portal_infos.len(),
        eye_world_wow[0],
        eye_world_wow[1],
        eye_world_wow[2],
        eye[0],
        eye[1],
        eye[2],
        look_world_wow[0],
        look_world_wow[1],
        look_world_wow[2],
        look[0],
        look[1],
        look[2],
    );

    let terrain = subject.terrain_z(eye);
    let mut log = crate::wmo_portal::probe::TraceLog::new(model, eye, terrain);
    let pvs = crate::wmo_portal::compute_pvs_traced(
        model,
        eye,
        terrain,
        &clip,
        &placed.world_from_local,
        &mut log,
    );
    // Only the seed evidence and the hop verdicts, not the per-group preamble.
    for line in log.text.lines() {
        if !line.trim_start().starts_with('g') || line.contains("->") {
            println!("{line}");
        }
    }

    let vis: Vec<usize> = pvs
        .visible
        .iter()
        .enumerate()
        .filter(|(_, &v)| v)
        .map(|(i, _)| i)
        .collect();
    println!(
        "visible: {} of {} groups {vis:?}",
        vis.len(),
        pvs.visible.len()
    );
    // The interior-fog gate (`[0xca7f00]`, [`super::super::GroupPvs`]): the groups that wear the
    // building's own MFOG rather than the scene fog.
    let fogged: Vec<usize> = pvs
        .interior_fog
        .iter()
        .enumerate()
        .filter(|(_, &v)| v)
        .map(|(i, _)| i)
        .collect();
    println!("interior-fog lane: {fogged:?}");

    // The portal graph of every group the flood reached: only the edge list tells a flood that ran
    // out of edges from one whose edges were all rejected.
    println!("-- portal graph of the visible set --");
    for &gi in &vis {
        let g = &model.group_nav[gi];
        let start = g.ref_start as usize;
        let end = (start + g.ref_count as usize).min(model.portal_refs.len());
        let edges: Vec<String> = model.portal_refs[start..end]
            .iter()
            .map(|r| format!("p{}->g{}(side {:+})", r.portal, r.group, r.side))
            .collect();
        println!(
            "  g{gi:02} flags {:#07x}{} bbox z[{:.1},{:.1}] refs[{}..+{}] {}",
            g.flags,
            if g.flags & EXTERIOR != 0 { " EXT" } else { "" },
            g.bbox_min[2],
            g.bbox_max[2],
            g.ref_start,
            g.ref_count,
            if edges.is_empty() {
                "NO EDGES — the flood dead-ends here".to_string()
            } else {
                edges.join(" ")
            }
        );
    }

    // The plane and vertex extent of every portal of a group holding the eye or the look point:
    // which side the eye is on needs the plane, not the verdict.
    println!("-- portals of the groups containing eye/look --");
    for (gi, g) in model.group_nav.iter().enumerate() {
        let holds = |p: [f32; 3]| (0..3).all(|k| p[k] >= g.bbox_min[k] && p[k] <= g.bbox_max[k]);
        if !holds(eye) && !holds(look) {
            continue;
        }
        let start = g.ref_start as usize;
        let end = (start + g.ref_count as usize).min(model.portal_refs.len());
        for r in &model.portal_refs[start..end] {
            let Some(info) = model.portal_infos.get(r.portal as usize) else {
                continue;
            };
            let [nx, ny, nz, d] = info.plane;
            let de = nx * eye[0] + ny * eye[1] + nz * eye[2] + d;
            let vs = (info.start_vertex as usize)
                ..(info.start_vertex as usize + info.count as usize)
                    .min(model.portal_vertices.len());
            let mut vmin = [f32::INFINITY; 3];
            let mut vmax = [f32::NEG_INFINITY; 3];
            for v in &model.portal_vertices[vs] {
                for k in 0..3 {
                    vmin[k] = vmin[k].min(v[k]);
                    vmax[k] = vmax[k].max(v[k]);
                }
            }
            println!(
                "  g{gi:02} p{} ->g{}(side {:+}) plane n({:.3},{:.3},{:.3}) d {:.2} | d(eye) {:+.3} | verts x[{:.1},{:.1}] y[{:.1},{:.1}] z[{:.1},{:.1}]",
                r.portal, r.group, r.side, nx, ny, nz, d, de,
                vmin[0], vmax[0], vmin[1], vmax[1], vmin[2], vmax[2],
            );
        }
    }

    // Every group whose MOGI bbox holds the look point, with its verdict.
    println!("-- groups containing the look point --");
    for (gi, g) in model.group_nav.iter().enumerate() {
        let inside = (0..3).all(|k| look[k] >= g.bbox_min[k] && look[k] <= g.bbox_max[k]);
        if inside {
            println!(
                "  g{gi:02} flags {:#07x}{} refs[{}..+{}]  -> {}",
                g.flags,
                if g.flags & EXTERIOR != 0 { " EXT" } else { "" },
                g.ref_start,
                g.ref_count,
                match (pvs.visible[gi], pvs.interior_fog[gi]) {
                    (true, true) => "VISIBLE · interior fog",
                    (true, false) => "VISIBLE · scene fog",
                    (false, _) => "CULLED",
                }
            );
        }
    }
}

/// From the EXTERIOR_LIT (`0x40`) courtyard g38 (`.go xyz -224.01 2168.63 79.79 33`), the arches
/// open through courtyard g72, also `0x40`, into the true interior g61. The reference draws g61
/// under the scene fog (map 33 at noon: start 83, end 333, none at 70 yd), not the building's MFOG
/// (rgb 40,68,79, start 10.7, end 106.9 yd), because the interior-fog chain breaks at an
/// exterior-lit group. The control stands in g61, which takes its own MFOG.
#[test]
fn shadowfang_courtyard_leaves_the_far_room_on_the_scene_fog() {
    let _data = benilla_formats::wow_data_or_skip!();
    let site = SHADOWFANG;
    let subject = load_subject(site.wmo, Some(&site));
    let placed = subject
        .placed
        .as_ref()
        .expect("the fixture needs a placement");
    let to_local =
        |wow: [f32; 3]| bevy_to_wow(placed.local_from_world.transform_point3(wow_to_bevy(wow)));
    // A third-person camera behind the player at the fountain, aimed at the p50 arches.
    let eye_world = [-227.1_f32, 2157.0, 83.7];
    let look_world = [-212.5_f32, 2232.3, 84.2];
    let eye = to_local(eye_world);
    let eye_bevy = placed.world_from_local.transform_point3(wow_to_bevy(eye));
    let look_bevy = placed
        .world_from_local
        .transform_point3(wow_to_bevy(to_local(look_world)));
    let clip = Mat4::perspective_rh(crate::view::CAM_FOVY, 16.0 / 9.0, 0.1, 1000.0)
        * Mat4::look_at_rh(eye_bevy, look_bevy, Vec3::Y);
    let pvs = crate::wmo_portal::compute_pvs_traced(
        &subject.model,
        eye,
        subject.terrain_z(eye),
        &clip,
        &placed.world_from_local,
        &mut (),
    );

    // The flags the whole verdict turns on, asserted so a data misread can't pass silently.
    assert_eq!(
        subject.model.group_nav[38].flags & 0x48,
        0x40,
        "g38 is the EXTERIOR_LIT courtyard"
    );
    assert_eq!(
        subject.model.group_nav[72].flags & 0x48,
        0x40,
        "g72 is the second EXTERIOR_LIT courtyard"
    );
    assert_eq!(
        subject.model.group_nav[61].flags & 0x48,
        0,
        "g61 is a true interior room"
    );

    assert!(
        pvs.visible[61],
        "the flood reaches the room behind the arches"
    );
    assert!(
        !pvs.interior_fog[61],
        "B335: the room two exterior-lit courtyards away must inherit the SCENE fog, not this \
         building's MFOG teal"
    );
    assert!(
        !pvs.interior_fog[72],
        "the chain is already broken at the second courtyard"
    );
    assert!(
        !pvs.interior_fog[38],
        "the courtyard the camera stands in is exterior-LIT: the exterior drawer pushes no fog, \
         so its own surfaces are on the scene lane too"
    );
    assert!(
        pvs.interior_fog.iter().all(|f| !f),
        "from this courtyard NOTHING wears the building's MFOG — which is what the reference \
         screenshot shows: not one navy pixel in the frame"
    );

    // The control: from inside g61 the room takes the interior lane, so a gate that is always off
    // fails here.
    let inside = to_local([-213.90, 2236.15, 81.5]);
    let inside_bevy = placed
        .world_from_local
        .transform_point3(wow_to_bevy(inside));
    let inside_clip = Mat4::perspective_rh(crate::view::CAM_FOVY, 16.0 / 9.0, 0.1, 1000.0)
        * Mat4::look_at_rh(inside_bevy, eye_bevy, Vec3::Y);
    let inside_pvs = crate::wmo_portal::compute_pvs_traced(
        &subject.model,
        inside,
        subject.terrain_z(inside),
        &inside_clip,
        &placed.world_from_local,
        &mut (),
    );
    assert!(
        inside_pvs.interior_fog[61],
        "control: the room seeds its own flood, so it wears the building's MFOG from inside"
    );
}

/// From inside g61, looking back across the courtyards, the flood reaches true-interior rooms that
/// are off the interior-fog chain. A unit in one classifies indoors by its own down-ray, so the
/// `[P+0x98]` conjunct is what puts it on the scene fog: g13's centre is 91 yd from the eye, where
/// the building's MFOG is ~84% saturated and the scene fog ~3%. The conjunct's own test is
/// `crate::interior`'s `a_settled_anchor_follows_its_rooms_fog_gate_without_moving`.
#[test]
fn shadowfang_sees_true_interiors_that_are_off_the_fog_chain() {
    let _data = benilla_formats::wow_data_or_skip!();
    let site = SHADOWFANG;
    let subject = load_subject(site.wmo, Some(&site));
    let placed = subject
        .placed
        .as_ref()
        .expect("the fixture needs a placement");
    let to_local =
        |wow: [f32; 3]| bevy_to_wow(placed.local_from_world.transform_point3(wow_to_bevy(wow)));
    let inside = to_local([-213.90, 2236.15, 81.5]);
    let look = to_local([-227.1, 2157.0, 83.7]);
    let inside_bevy = placed
        .world_from_local
        .transform_point3(wow_to_bevy(inside));
    let look_bevy = placed.world_from_local.transform_point3(wow_to_bevy(look));
    let clip = Mat4::perspective_rh(crate::view::CAM_FOVY, 16.0 / 9.0, 0.1, 1000.0)
        * Mat4::look_at_rh(inside_bevy, look_bevy, Vec3::Y);
    let pvs = crate::wmo_portal::compute_pvs_traced(
        &subject.model,
        inside,
        subject.terrain_z(inside),
        &clip,
        &placed.world_from_local,
        &mut (),
    );

    assert!(
        pvs.interior_fog[61],
        "the camera's own room is on the chain — otherwise nothing here is being tested"
    );
    let off_chain: Vec<usize> = (0..subject.model.group_nav.len())
        .filter(|&g| {
            pvs.visible[g] && !pvs.interior_fog[g] && subject.model.group_nav[g].flags & 0x48 == 0
        })
        .collect();
    // A unit standing in one of those rooms attaches to it, by the light classifier's own ray and
    // `0x48` mask (`indoor_verdict_at`): g13 claims g13, off the chain, and the control g61 claims
    // g61, on it.
    let attach_room = |gi: usize| {
        let g = &subject.model.group_nav[gi];
        let centre = [
            (g.bbox_min[0] + g.bbox_max[0]) * 0.5,
            (g.bbox_min[1] + g.bbox_max[1]) * 0.5,
            (g.bbox_min[2] + g.bbox_max[2]) * 0.5,
        ];
        down_ray_claim(
            &subject.model.group_collision_tris,
            &subject.model.group_collision_bounds,
            &subject.model.group_collision_grids,
            &subject.model.group_nav,
            centre,
            None,
            EXTERIOR | EXTERIOR_LIT,
        )
        .map(|c| (c.group, c.outdoor))
    };
    assert_eq!(
        attach_room(13),
        Some((13, false)),
        "a body standing in g13 attaches to g13 and classifies INDOORS — the light law says fog it"
    );
    assert!(
        !pvs.interior_fog[13],
        "…and its room is off the chain, so the fog it takes is the SCENE's (1792 §5's fix)"
    );
    assert_eq!(
        attach_room(61),
        Some((61, false)),
        "the control: a body in the camera's own room attaches to it"
    );

    assert_eq!(
        off_chain,
        vec![1, 2, 13, 68],
        "four true-interior rooms are in frame across the courtyards and none is on the chain: \
         this is the population a unit's fog gate has to answer for. It was three until 1853 — \
         g68 sits behind an exterior group no portal joins to the seed's half of the graph, so it \
         arrives only once Pass 2 WALKS ON from the window that admits that group instead of \
         merely marking it. The gate's verdict on all four is identical; what changed is how many \
         rooms the cull lets the director see at once"
    );
}

/// Pass 2 walks on from a window rather than only marking the group it admits: the reference ends
/// its per-group predicate in the portal recursion (`0x6b3d39 call 0x6b41c0`), Pass 1's own entry,
/// not in Pass 3's callback draw (`0x6b4160`). Measured as the gain, the groups the Pass 2 walk
/// entered that no window admitted and Pass 1 never reached, from each Darnassus interior with a
/// doorway; the city's portal-linked outdoor areas give the walk somewhere to go.
#[test]
fn darnassus_pass_two_walks_on_from_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    use std::collections::HashSet;

    /// The flood's `entered` steps split by pass, and the groups a window admitted directly.
    #[derive(Default)]
    struct WalkOn {
        in_pass2: bool,
        seen_pass1: HashSet<usize>,
        roots: HashSet<usize>,
        reached: HashSet<usize>,
    }
    impl super::super::FloodTrace for WalkOn {
        fn seed(&mut self, seeds: super::super::DownRaySeeds) {
            self.seen_pass1.extend(seeds.in_group);
            self.seen_pass1.extend(seeds.across);
        }
        fn entered(
            &mut self,
            _from: usize,
            _portal: u16,
            to: usize,
            _rect: super::super::Rect,
            _on_plane: bool,
        ) {
            if self.in_pass2 {
                self.reached.insert(to);
            } else {
                self.seen_pass1.insert(to);
            }
        }
        fn pass2(&mut self, group: usize, _flags: u32, _window: usize, admitted: bool) {
            self.in_pass2 = true;
            if admitted {
                self.roots.insert(group);
            }
        }
    }

    let site = DARNASSUS;
    let subject = load_subject(site.wmo, Some(&site));
    let placed = subject.placed.as_ref().expect("Darnassus has a placement");
    let nav = &subject.model.group_nav;

    // Every true interior with a doorway is a pose: stand on its floor and look out through its
    // first portal.
    let mut poses = 0usize;
    let mut total_gain = 0usize;
    let mut best: Option<(usize, usize, usize)> = None; // (gain, group, exterior drawn)
    for (gi, g) in nav.iter().enumerate() {
        if g.flags & (EXTERIOR | EXTERIOR_LIT) != 0 || g.ref_count == 0 {
            continue;
        }
        let eye = [
            (g.bbox_min[0] + g.bbox_max[0]) * 0.5,
            (g.bbox_min[1] + g.bbox_max[1]) * 0.5,
            g.bbox_min[2] + super::EYE_HEIGHT,
        ];
        // Only a pose whose seed names the intended room counts: a bbox centre can sit in a wall,
        // over a stairwell or above a mezzanine floor.
        if subject.seeds(eye).in_group != Some(gi) {
            continue;
        }
        let r = &subject.model.portal_refs[g.ref_start as usize];
        let Some(verts) = super::super::portal_poly(
            &subject.model.portal_vertices,
            &subject.model.portal_infos[r.portal as usize],
        ) else {
            continue;
        };
        let n = verts.len() as f32;
        let look = [
            verts.iter().map(|v| v[0]).sum::<f32>() / n,
            verts.iter().map(|v| v[1]).sum::<f32>() / n,
            verts.iter().map(|v| v[2]).sum::<f32>() / n,
        ];
        if look == eye {
            continue;
        }
        let eye_bevy = placed.world_from_local.transform_point3(wow_to_bevy(eye));
        let look_bevy = placed.world_from_local.transform_point3(wow_to_bevy(look));
        let clip = Mat4::perspective_rh(crate::view::CAM_FOVY, 16.0 / 9.0, 0.1, 1000.0)
            * Mat4::look_at_rh(eye_bevy, look_bevy, Vec3::Y);
        let mut tap = WalkOn::default();
        let pvs = crate::wmo_portal::compute_pvs_traced(
            &subject.model,
            eye,
            subject.terrain_z(eye),
            &clip,
            &placed.world_from_local,
            &mut tap,
        );
        let gain: HashSet<usize> = tap
            .reached
            .difference(&tap.roots)
            .copied()
            .filter(|g| !tap.seen_pass1.contains(g))
            .collect();
        let ext_drawn = nav
            .iter()
            .zip(&pvs.visible)
            .filter(|(n, v)| **v && n.flags & EXTERIOR != 0)
            .count();
        println!(
            "g{gi:03} windows={} walk steps={} exterior drawn={ext_drawn}/50 walk-on gain={} {:?}",
            pvs.windows.len(),
            pvs.iters,
            gain.len(),
            {
                let mut v: Vec<usize> = gain.iter().copied().collect();
                v.sort_unstable();
                v
            }
        );
        poses += 1;
        total_gain += gain.len();
        if best.is_none_or(|(b, _, _)| gain.len() > b) {
            best = Some((gain.len(), gi, ext_drawn));
        }
    }
    println!("poses={poses} total walk-on gain={total_gain} best={best:?}");
    assert!(
        poses >= 20,
        "the sweep needs the real city, got {poses} poses"
    );
    assert!(
        total_gain > 0,
        "Pass 2 must WALK ON from a window-admitted exterior group (0x6b3d39 -> 0x6b41c0); a \
         marking pass scores exactly zero here"
    );
}
