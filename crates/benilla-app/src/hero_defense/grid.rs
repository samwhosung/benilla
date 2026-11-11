//! Hero Defense: nevyrazne znazorneni aren na zemi kolem hradu.
//!
//! Od hradu ven: perimetr bez aren (finalni cesty), vnitrni hranice aren (kruh), 8 aren
//! rozdelenych na ctverce pro stavbu vezi, vnejsi hranice (kruh). Smer, kterym se hrad diva, je
//! cesta bosse a areny na ni nejsou.
//!
//! Geometrie je stejna jako na serveru (`hd_defs.h`, `HdArena::LocalToWorld`): klient ji
//! spocita z polohy a natoceni hradu. Cary kopiruji teren a pocitaji se jednou, znovu jen kdyz
//! se hrad pohne nebo kdyz pod mrizkou jeste nebyl nacteny teren.

use bevy::gizmos::config::{GizmoConfigGroup, GizmoConfigStore};
use bevy::prelude::*;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_world::world_point::WorldPoint;

use crate::net::Guid;

// ---- rozmery, stejne jako na serveru (src/scripts/custom/hd_defs.h) ----------------------------

const CASTLE_ENTRY: u32 = 90000;
const CELL: f32 = 4.0;
const COLS: i32 = 15;
const ROWS: i32 = 25;
const FINAL_PATH: f32 = 100.0;
const ARENA_COUNT: i32 = 8;
const RING_SLOTS: i32 = ARENA_COUNT + 1;
const BOSS_START: f32 = 240.0;
const BOSS_LANE_HALF: f32 = 10.0;

/// Jak vysoko nad terenem cary lezi, at neblika se zemi.
const LIFT: f32 = 0.08;
/// Jak casto zkusit znovu spocitat cary, kdyz pod nimi jeste nebyl teren (s).
const RETRY_SECS: f32 = 1.0;

const COLOR_BOUNDARY: Color = Color::srgba(1.0, 0.92, 0.75, 0.40);
const COLOR_ARENA: Color = Color::srgba(1.0, 1.0, 1.0, 0.30);
const COLOR_CELL: Color = Color::srgba(1.0, 1.0, 1.0, 0.10);
const COLOR_SPAWN: Color = Color::srgba(1.0, 0.25, 0.2, 0.65);
const COLOR_EXIT: Color = Color::srgba(0.3, 1.0, 0.35, 0.65);
const COLOR_BOSS: Color = Color::srgba(0.95, 0.25, 0.2, 0.35);

/// Vlastni skupina car, at jeji tloustka a hloubka nemeni ostatni cary klienta (tetiva luku...).
#[derive(Default, Reflect, GizmoConfigGroup)]
pub(super) struct HdGridGizmos;

#[derive(Resource, Default)]
struct GridCache {
    /// Poloha (WoW x, y) a natoceni hradu, pro ktere jsou cary spocitane.
    key: Option<(f32, f32, f32)>,
    lines: Vec<(Vec3, Vec3, Color)>,
    /// Vsechny body mely teren; jinak se za chvili pocita znovu.
    complete: bool,
    retry: f32,
}

pub(super) fn plugin(app: &mut App) {
    app.init_gizmo_group::<HdGridGizmos>()
        .init_resource::<GridCache>()
        .add_systems(Startup, configure_gizmos)
        .add_systems(Update, draw_arena_grid);
}

fn configure_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<HdGridGizmos>();
    config.line.width = 1.5;
    // Kousek k divakovi, at cary nezmizi v terenu, ale postavy a hrad je porad zakryji.
    config.depth_bias = -0.0005;
}

/// Bod ve WoW souradnicich (x, y) na terenu, v souradnicich Bevy.
fn ground(world: &WorldPoint, wx: f32, wy: f32) -> Option<Vec3> {
    let flat = wow_to_bevy([wx, wy, 0.0]);
    let h = world.terrain_height_under(flat)?;
    Some(Vec3::new(flat.x, h + LIFT, flat.z))
}

struct Builder<'a, 'w, 's> {
    world: &'a WorldPoint<'w, 's>,
    lines: Vec<(Vec3, Vec3, Color)>,
    complete: bool,
}

impl Builder<'_, '_, '_> {
    /// Lomena cara pres body ve WoW souradnicich; `dash` vynecha kazdy druhy usek.
    fn polyline(&mut self, pts: &[(f32, f32)], color: Color, dash: bool) {
        let mut prev: Option<Vec3> = None;
        for (i, &(x, y)) in pts.iter().enumerate() {
            let p = ground(self.world, x, y);
            if p.is_none() {
                self.complete = false;
            }
            if let (Some(a), Some(b)) = (prev, p) {
                if !dash || i % 2 == 1 {
                    self.lines.push((a, b, color));
                }
            }
            prev = p;
        }
    }
}

fn build(world: &WorldPoint, cx: f32, cy: f32, o: f32) -> (Vec<(Vec3, Vec3, Color)>, bool) {
    let mut b = Builder { world, lines: Vec::new(), complete: true };
    let step = std::f32::consts::TAU / RING_SLOTS as f32;

    // Bod v mistnich souradnicich smeru `dir`: `forward` od hradu, `left` doleva.
    let local = |dir: f32, forward: f32, left: f32| {
        (
            cx + forward * dir.cos() - left * dir.sin(),
            cy + forward * dir.sin() + left * dir.cos(),
        )
    };
    // Usek rozdeleny po `seg` yardech, at kopiruje teren.
    let segment = |dir: f32, f0: f32, l0: f32, f1: f32, l1: f32, seg: f32| {
        let n = (((f1 - f0).hypot(l1 - l0)) / seg).ceil().max(1.0) as i32;
        (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                local(dir, f0 + (f1 - f0) * t, l0 + (l1 - l0) * t)
            })
            .collect::<Vec<_>>()
    };

    // Vnitrni a vnejsi hranice aren.
    let depth = ROWS as f32 * CELL;
    let half = COLS as f32 * CELL / 2.0;
    let outer = (FINAL_PATH + depth).hypot(half);
    for r in [FINAL_PATH, outer] {
        let n = (std::f32::consts::TAU * r / 3.0).ceil() as i32;
        let pts: Vec<_> = (0..=n)
            .map(|i| local(o + std::f32::consts::TAU * i as f32 / n as f32, r, 0.0))
            .collect();
        b.polyline(&pts, COLOR_BOUNDARY, false);
    }

    // Areny: obrys, mrizka ctvercu, spawn a vychod.
    for k in 1..=ARENA_COUNT {
        let dir = o + k as f32 * step;
        for c in 0..=COLS {
            let l = -half + c as f32 * CELL;
            let color = if c == 0 || c == COLS { COLOR_ARENA } else { COLOR_CELL };
            b.polyline(&segment(dir, FINAL_PATH, l, FINAL_PATH + depth, l, CELL), color, false);
        }
        for r in 0..=ROWS {
            let f = FINAL_PATH + r as f32 * CELL;
            let color = if r == 0 || r == ROWS { COLOR_ARENA } else { COLOR_CELL };
            b.polyline(&segment(dir, f, -half, f, half, CELL), color, false);
        }
        // Spawn: prostredni pole posledni rady; vychod: prostredni pole prvni rady (HdGrid).
        let mid = -half + (COLS / 2) as f32 * CELL;
        for (row, color) in [(ROWS - 1, COLOR_SPAWN), (0, COLOR_EXIT)] {
            let f = FINAL_PATH + row as f32 * CELL;
            let (i0, i1) = (0.4, CELL - 0.4);
            let square = [
                local(dir, f + i0, mid + i0),
                local(dir, f + i0, mid + i1),
                local(dir, f + i1, mid + i1),
                local(dir, f + i1, mid + i0),
                local(dir, f + i0, mid + i0),
            ];
            b.polyline(&square, color, false);
        }
    }

    // Cesta bosse: prerusovane okraje od hradu ke startu na zavodni draze.
    for l in [-BOSS_LANE_HALF, BOSS_LANE_HALF] {
        b.polyline(&segment(o, 15.0, l, BOSS_START, l, 3.0), COLOR_BOSS, true);
    }

    (b.lines, b.complete)
}

fn draw_arena_grid(
    castles: Query<(&Guid, &GlobalTransform)>,
    world: WorldPoint,
    mut cache: ResMut<GridCache>,
    mut gizmos: Gizmos<HdGridGizmos>,
    time: Res<Time>,
) {
    let castle = castles
        .iter()
        .find(|(guid, _)| benilla_protocol::guid::entry(guid.0) == Some(CASTLE_ENTRY));
    let Some((_, gt)) = castle else {
        return;
    };

    // Natoceni jednotky je `from_rotation_y(o)` a dopredu je -Z (`net::motion`).
    let wow = bevy_to_wow(gt.translation());
    let fwd = gt.rotation() * Vec3::NEG_Z;
    let o = (-fwd.x).atan2(-fwd.z);
    let key = (
        (wow[0] * 10.0).round() / 10.0,
        (wow[1] * 10.0).round() / 10.0,
        (o * 1000.0).round() / 1000.0,
    );

    cache.retry -= time.delta_secs();
    let moved = cache.key != Some(key);
    if moved || (!cache.complete && cache.retry <= 0.0) {
        let (lines, complete) = build(&world, wow[0], wow[1], o);
        if moved || lines.len() >= cache.lines.len() {
            cache.lines = lines;
        }
        cache.complete = complete;
        cache.key = Some(key);
        cache.retry = RETRY_SECS;
    }

    for &(a, b, color) in &cache.lines {
        gizmos.line(a, b, color);
    }
}
