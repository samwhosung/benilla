//! The precip **sim economics** — the drop/patter records, the packet delay-line, the
//! ground oracle cache, and the per-kind frame (`run_kind`). Split from `precip`'s root;
//! byte-cited constants stay in the root's table, ECS wiring stays in the root's systems.

use bevy::platform::collections::HashMap;

use super::*;

/// One falling particle. Positions/velocities are Bevy world space (y up).
pub(super) struct Drop {
    pub(super) pos: Vec3,
    pub(super) vel: Vec3,
    /// Ground height (Bevy y) of the drop's CURRENT grid cell — dies (and may patter) on
    /// reaching it. Re-sampled whenever the drop drifts into a new cell ([`Drop::cell`]): at
    /// grade-1 rain the lateral drift is ~9.5 yd/s over a ~1.2 s fall (~11 yd), so the spawn
    /// column's ground is the WRONG answer by up to a house — a drop seeded outside the inn's
    /// porch drifted through the open front and splashed on the vestibule floor with the
    /// terrain's height (director-caught, 2026-07-13). Whether the reference stores the spawn
    /// answer or re-reads its grid per cell is open; under a roof either reading keeps
    /// splashes out of the room.
    pub(super) land_y: f32,
    /// The grid cell `land_y` was sampled for (see [`HeightCache`]).
    pub(super) cell: (i32, i32),
    /// Seconds since this particle became VISIBLE — the reference's `t − f1`, where `f1` is the
    /// flake's birth stamp on its packet's clock. Snow's fade-in reads it (`snowpoint.bls`, the
    /// point-sprite leg `0x678610`); rain's streaks have no age term and ignore it. A record that
    /// replays late starts at its own lag, exactly as `c_currentTime − f1` would.
    pub(super) age: f32,
}

/// A recorded drop waiting in the packet pipeline: replays at `at` (its packet's `baseTime`
/// plus its record offset), and only once its packet has SEALED.
struct Pending {
    at: f32,
    drop: Drop,
}

/// One drop packet — the reference's `Packet<Drop>`. Records accumulate while OPEN; `baseTime`
/// (`packet+0x36010`) is stamped ONLY at open (`0x67598c`: `baseTime = now + 6144/rate`) and the
/// batch REPLAYS from it — a delay line (visible density = true density delayed by `6144/rate`). On
/// shader hardware a packet draws ONLY from its static buffer, baked once at close (`0x6752b0`,
/// sole caller = the flush `0x675a97`) — so an OPEN packet renders NOTHING: visibility needs sealed
/// AND past its replay instant. Sparse upswing cohorts stamp far-future baseTimes and are never
/// seen at record time (the very first packet stamps ~100 s out and genuinely ghost-replays its
/// faint drizzle later — it passes the 60 s duration guard `[0x80ff6c]`; byte-faithful, kept).
struct Packet {
    /// `pkt+0x3001c` — the camera eye at OPEN, and the only thing [`RETIRE_DIST`] is measured
    /// from. Written once (`0x678598`, from that update's `0xc7cf20` snapshot); never refreshed.
    anchor: Vec3,
    /// `buildTime` — when the packet opened (record offsets are relative to this).
    opened: f32,
    /// `baseTime` — `opened + 6144/rate_at_open`: the instant replay starts.
    visible_at: f32,
    records: Vec<Pending>,
    /// Total records ever pushed (records drain on activation; the CAP tracks this).
    count: u32,
}

/// One ground splash: a camera-facing triangle animated across the 4×4 splash atlas.
pub(super) struct Patter {
    pub(super) pos: Vec3,
    pub(super) age: f32,
    /// Atlas row (0..4 splash variants).
    pub(super) variant: u8,
}

/// The per-kind pool (falling layer + ground layer) — its geometry rides the shared effect
/// stream, so it owns no meshes.
#[derive(Default)]
pub(super) struct Pool {
    /// ACTIVE drops — falling, rendered, landing.
    pub(super) drops: Vec<Drop>,
    /// The OPEN packet (recording; renders nothing).
    open: Option<Packet>,
    /// SEALED packets — closed cohorts replaying (or waiting for) their baseTime windows.
    sealed: Vec<Packet>,
    pub(super) patters: Vec<Patter>,
}

impl Pool {
    /// Records still in the pipeline (open + sealed) — the instrument's "pipe" count.
    pub(super) fn pending_len(&self) -> usize {
        self.open.as_ref().map_or(0, |p| p.records.len())
            + self.sealed.iter().map(|p| p.records.len()).sum::<usize>()
    }

    /// Close-check + (re)open for this frame's recording; returns the packet's remaining
    /// record space and the replay instant a record made NOW carries. Close conditions
    /// (`0x675a6d` full ≥ 6144 / build age ≥ `P/6144` / past `baseTime`): closing SEALS the
    /// packet (bakes its buffer — it may start drawing) and the next record opens a fresh one
    /// stamped at the live rate.
    fn open_for(&mut self, now: f32, rate: f32, close_age: f32, cam: Vec3) -> (usize, f32) {
        if let Some(pk) = &self.open {
            if pk.count >= PACKET_CAP as u32 || now - pk.opened >= close_age || now >= pk.visible_at
            {
                self.seal();
            }
        }
        let pk = self.open.get_or_insert_with(|| Packet {
            anchor: cam,
            opened: now,
            visible_at: now + PACKET_CAP as f32 / rate.max(1.0),
            records: Vec::new(),
            count: 0,
        });
        (
            PACKET_CAP - pk.count as usize,
            pk.visible_at + (now - pk.opened),
        )
    }

    fn seal(&mut self) {
        if let Some(pk) = self.open.take() {
            if !pk.records.is_empty() {
                self.sealed.push(pk);
            }
        }
    }

    /// The TYPE-CHANGE cut (driver `0x67be40` cross-fade): emission has already stopped
    /// (`0x67585d`); the OPEN packet is retired unbaked (`0x6756b2–da` — it never drew, so it
    /// is discarded), and every sealed packet whose replay hasn't started is unlinked
    /// (`0x67575a`: discard when `curTick ≤ baseTime`) — the straggler tail dies here. Packets
    /// already replaying finish their windows; active drops fall out normally (≤ ~1.3 s).
    pub(super) fn cut(&mut self, now: f32) {
        self.open = None;
        self.sealed.retain(|p| now > p.visible_at);
    }

    /// The **anchor cull** — retirement condition 3 ([`RETIRE_DIST`]), run at the top of the frame
    /// exactly where the reference's active-list walk (`0x677ff0`) runs it, so the space it frees
    /// is available to this frame's emission.
    ///
    /// Two halves, because benilla splits what the reference keeps in one buffer:
    ///
    /// - **Sealed packets** carry their anchor and are tested exactly as the bytes test them,
    ///   3-D and against the live eye. The **open** packet is deliberately *not* tested: it is not
    ///   in the reference's active list either (it is reached only by the stopping path, `cut`),
    ///   and it falls to this cull one frame after it seals — which is precisely what the
    ///   reference does with it.
    /// - **Active drops** have no packet left to test, so they go by their own distance at a
    ///   threshold slackened past anything the reference would still draw ([`RETIRE_DROP_SLACK`]).
    ///
    /// Retirement is a **discard, not a drain**: nothing here finishes replaying or finishes
    /// falling, and a discarded flake never lands, so it leaves no patter behind.
    pub(super) fn retire_far(&mut self, cam: Vec3, kind: WeatherKind) {
        let anchor_r2 = RETIRE_DIST * RETIRE_DIST;
        self.sealed
            .retain(|pk| pk.anchor.distance_squared(cam) <= anchor_r2);
        let (half_xy, z_off) = spawn_box(kind);
        let corner = 2.0f32.mul_add(half_xy * half_xy, z_off * z_off).sqrt();
        let drop_r = RETIRE_DIST + corner + RETIRE_DROP_SLACK;
        self.drops
            .retain(|d| d.pos.distance_squared(cam) <= drop_r * drop_r);
    }
}

/// Lazy ground-height cache over the reference's weather ground oracle — the fog grid's
/// sampler `0x6b7070` is **WMO/doodad-AWARE**, not terrain-only: after the MCVT terrain sample it
/// probes the chunk's static object refs ±200 yd (`CMapObj::IntersectSegment 0x6a37b0`) and MAXes
/// the hit with terrain (`0x6b7237–4a`). So drops LAND ON ROOFS — splashes on the inn roof, never
/// inside, from any camera. Benilla's equivalent: one downward ray per ~1.04-yd cell from the spawn
/// plane against terrain + walk-WMO + doodads/GameObjects. Cleared when the spawn plane moves a
/// story (the ray start decides which roofs it sees) or when it outgrows its bound.
#[derive(Resource, Default)]
pub(super) struct HeightCache {
    cells: HashMap<(i32, i32), f32>,
    cast_y: f32,
}

/// The reference's cell stride (`0x80ff98` = 1.0416666).
pub(super) const CELL: f32 = 1.041_666_6;
/// A probe that hits nothing reports ground 200 below the query (`0x67c812`: `refZ − 200`).
const MISS_DEPTH: f32 = 200.0;

impl HeightCache {
    pub(super) fn ground_y(
        &mut self,
        x: f32,
        z: f32,
        cast_from_y: f32,
        spatial: &SpatialQuery,
        filter: &SpatialQueryFilter,
    ) -> f32 {
        if (self.cast_y - cast_from_y).abs() > 8.0 || self.cells.len() > 20_000 {
            self.cells.clear();
            self.cast_y = cast_from_y;
        }
        let key = Self::key(x, z);
        if let Some(&y) = self.cells.get(&key) {
            return y;
        }
        let cx = (key.0 as f32 + 0.5) * CELL;
        let cz = (key.1 as f32 + 0.5) * CELL;
        let y = spatial
            .cast_ray(
                Vec3::new(cx, cast_from_y, cz),
                Dir3::NEG_Y,
                MISS_DEPTH + 50.0,
                true,
                filter,
            )
            .map_or(cast_from_y - MISS_DEPTH, |hit| cast_from_y - hit.distance);
        self.cells.insert(key, y);
        y
    }

    /// The grid cell owning an XZ position (the reference's 1.0416666-yd stride).
    pub(super) fn key(x: f32, z: f32) -> (i32, i32) {
        ((x / CELL).floor() as i32, (z / CELL).floor() as i32)
    }
}

/// The reference's per-frame record count (`0x6754cc–ef`): `fistp(min(space, quota) − 0.5)`
/// under round-nearest-EVEN — the x87 default. The parity matters: at `space = 1` this is
/// `RNE(0.5) = 0` FOREVER, so a packet that lands on 6143 records can never full-close and
/// seals only at the `P/6144` build age (5.7 s shader rain) — one of the three mechanisms
/// behind the reference's stochastic ~5 s / ~10 s upswing onset.
fn frame_count(space: usize, quota: f32) -> usize {
    ((space as f32).min(quota) - 0.5).round_ties_even().max(0.0) as usize
}

/// One kind's spawn box: `(half_xy, z_off)` — the horizontal half-extent of the scatter plane
/// and the slab's local lift above it (HALF the ctor's vertical extent; see [`RAIN_Z_OFF`]).
pub(super) const fn spawn_box(kind: WeatherKind) -> (f32, f32) {
    match kind {
        WeatherKind::Rain => (RAIN_HALF_XY, RAIN_Z_OFF),
        _ => (SNOW_HALF_XY, SNOW_Z_OFF),
    }
}

/// One particle's **placement law**, as a pure function of its five RNG draws — the reference's
/// composed spawn (`0x677750` snow / `0x674c50` rain):
///
/// ```text
/// pos = R(α, ĥ×ŷ)·(O − T·V)  +  1.75·W  +  C
/// ```
///
/// - `O` — the scatter, uniform over `±half_xy` on a **flat plane through the eye**, its vertical
///   random dead (`·0.0` at `0x6777ce`). This is the plane the particle *arrives* on, not the one
///   it is born on.
/// - `T = z_off/|V.y|`, `O − T·V` — the back-projection up the particle's own velocity, so it
///   starts `z_off` above the plane **in slab-local space** and passes its scatter point at `t = T`.
/// - **`R` — the slab tilt into the direction of travel, and the term benilla was missing.**
///   `α = 65°·sat(|W|/18)` about the horizontal axis ⊥ the heading ([`WeatherWind::slab`]).
///   It rotates the **local offset only**: the velocity is written once at spawn and never
///   re-touched, so drift stays world-fixed (`0x677a41` copies 3 dwords — position, not velocity).
/// - `1.75·W + C` — the wind lead and the live camera eye, added AFTER the rotation, so the
///   volume's origin rides the player while its *shape* leans.
///
/// Why the missing `R` was the whole of B233's "I can outrun the snow": untilted, every particle
/// is born a flat `z_off` up and needs the full `T` to reach eye height — 7.8 s for snow at grade
/// 0.6, during which a running player covers 54 yd against a 45 yd box. Tilted at a 7 yd/s run
/// (α = 25.3°) the slab's leading corner is born ~8 yd up and ~53 yd ahead instead, arriving in
/// ~2 s — so snow keeps meeting the runner head-on. Rain never showed the symptom because its
/// `T` is 1.24 s, but it takes the same rotation.
///
/// Returns `(spawn, velocity)` in Bevy world space.
pub(super) fn spawn_particle(
    kind: WeatherKind,
    w: f32,
    origin: Vec3,
    slab: Quat,
    r: [f32; 5],
) -> (Vec3, Vec3) {
    let (half_xy, z_off) = spawn_box(kind);
    let scatter = Vec3::new(
        (r[0] - 0.5) * 2.0 * half_xy,
        0.0,
        (r[1] - 0.5) * 2.0 * half_xy,
    );
    // The byte kinematics (w = density): the drift heading centres on the WORLD-FIXED −1.57
    // azimuth (0x80ffbc) ± a grade-scaled spread (the FULL width — `(r−0.5)` halves it) — rain
    // stays a coherent sheet (±7.5° at grade 1); calm snow wanders anywhere (2π at grade 0).
    let (vy, drift_mag, spread) = match kind {
        WeatherKind::Rain => (
            -(RAIN_VZ_BASE + RAIN_VZ_W * w + RAIN_VZ_RNG * w * r[2]),
            ((2.0 * r[3] - 1.0) + RAIN_DRIFT_BASE) * w + RAIN_DRIFT_EPS,
            RAIN_SPREAD_W * w + RAIN_SPREAD_BIAS,
        ),
        _ => (
            -(SNOW_VZ_BASE + SNOW_VZ_W * w + w * r[2]),
            ((r[3] - 0.5) + SNOW_DRIFT_OFF) * w + SNOW_DRIFT_EPS,
            std::f32::consts::TAU - SNOW_SPREAD_W * w,
        ),
    };
    let heading = DRIFT_AZ_CENTER + (r[4] - 0.5) * spread;
    // `vy` is strictly negative for both kinds, so `T` is positive and finite.
    let vel = wow_azimuth_to_bevy(heading) * drift_mag + Vec3::Y * vy;
    (origin + slab * (scatter - vel * (z_off / -vy)), vel)
}

/// One kind's frame: record (RNE-counted, packet-stamped), activate sealed cohorts,
/// integrate, land into the ground layer.
pub(super) fn run_kind(
    pool: &mut Pool,
    kind: WeatherKind,
    weather: &WeatherState,
    wind: &WeatherWind,
    heights: &mut HeightCache,
    spatial: &SpatialQuery,
    filter: &SpatialQueryFilter,
    rng: &mut u32,
    now: f32,
    dt: f32,
    cam_pos: Vec3,
) {
    let density = weather.density_for(kind);
    let p = match kind {
        WeatherKind::Rain => RAIN_P,
        _ => SNOW_P,
    };
    let z_off = spawn_box(kind).1;
    // Every ground probe this frame casts from ONE plane. It has to be a constant: the slab tilt
    // spreads the spawn heights across `z_off·(cos α ∓ (half_xy/z_off)·sin α)` — ~8..46 yd at a
    // 7 yd/s run — and [`HeightCache`] throws its whole grid away when the cast height moves 8 yd,
    // so probing from each particle's own height would nuke the cache every few particles.
    let cast_plane = cam_pos.y + z_off;

    // ===== retire (the active-list walk, before anything is emitted) =====
    pool.retire_far(cam_pos, kind);

    // ===== record (the emit gate + the RNE frame count) =====
    // The per-frame emit gate (`0x6754a0`, threshold `[0x8015b8]` = 1.0): a frame records only
    // when `min(dt, 1/60)·rate > 1.0` — below ~60 drops/s NOTHING spawns (no trickle of ones).
    //
    // `REF_FPS_GAIN` denominates the rate in the reference install's frame cap (1165) — the one
    // deliberate departure from the byte law on this path, and the reason benilla no longer reads
    // twice as dense as the A/B it is judged against.
    let rate = weather.density_gain() * p * density * REF_FPS_GAIN;
    let quota = rate * dt.min(DT_CAP);
    if quota > 1.0 {
        let close_age = p / PACKET_CAP as f32;
        let (space, replay_at) = pool.open_for(now, rate, close_age, cam_pos);
        let n = frame_count(space, quota)
            .min(pool_bound(kind).saturating_sub(pool.drops.len() + pool.pending_len()));
        // The emission volume's origin: the live camera eye carried forward by the wind lead
        // (`+1.75·W + C`, added AFTER the tilt — the volume rides the player, its shape leans).
        let anchor = cam_pos + wind.vel * WIND_LEAD;
        let origin = Vec3::new(anchor.x, cam_pos.y, anchor.z);
        let w = density;
        for _ in 0..n {
            let r = [
                rand01(rng),
                rand01(rng),
                rand01(rng),
                rand01(rng),
                rand01(rng),
            ];
            let (spawn, vel) = spawn_particle(kind, w, origin, wind.slab, r);
            let land_y = heights.ground_y(spawn.x, spawn.z, cast_plane, spatial, filter);
            // Ground above the whole spawn column (a mountainside or roof over the box top) —
            // the oracle reports ground ≥ spawn z and the drop dies at birth (`0x675051`).
            if land_y >= spawn.y {
                continue;
            }
            let drop = Drop {
                pos: spawn,
                vel,
                land_y,
                cell: HeightCache::key(spawn.x, spawn.z),
                age: 0.0,
            };
            if let Some(pk) = &mut pool.open {
                pk.count += 1;
                pk.records.push(Pending {
                    at: replay_at,
                    drop,
                });
            }
        }
    }

    // ===== activate: sealed cohorts replay their records from baseTime onward =====
    // A record whose replay instant passed while its packet was still baking appears mid-
    // window: fast-forward its fall (drift included — re-sample the ground at the cell it
    // drifted to) and skip it if it already landed (no splash — an unseen landing never drew).
    let drops = &mut pool.drops;
    for pk in &mut pool.sealed {
        let mut i = 0;
        while i < pk.records.len() {
            if pk.records[i].at <= now {
                let rec = pk.records.swap_remove(i);
                let lag = now - rec.at;
                let mut drop = rec.drop;
                drop.pos += drop.vel * lag;
                drop.age = lag;
                let cell = HeightCache::key(drop.pos.x, drop.pos.z);
                if cell != drop.cell {
                    drop.cell = cell;
                    drop.land_y =
                        heights.ground_y(drop.pos.x, drop.pos.z, cast_plane, spatial, filter);
                }
                if drop.pos.y > drop.land_y {
                    drops.push(drop);
                }
            } else {
                i += 1;
            }
        }
    }
    pool.sealed.retain(|pk| !pk.records.is_empty());

    // Ground layer. Rain: a patter is created **1:1 with each landing drop** (emit `0x6754a0`).
    // The `|v|² ≤ 2` gate (0x6755c2, manager+0x5c) tests the RIDDEN-TRANSPORT velocity
    // (object 0x903) — ≈0 whenever the player isn't on a moving transport — NOT the wind:
    // running never kills splashes. Benilla has no ridden transports yet, so the gate always
    // passes; when transports land, test the transport's planar speed² ≤ 2.0 here. Snow: every
    // landing flake settles and fades over the `+0.25 s` window. Patter pool cap = 0x1800
    // (byte-cited; at full shader rain the landing rate saturates it).
    let ground_life = match kind {
        WeatherKind::Rain => PATTER_LIFE,
        _ => SNOW_SETTLE_LIFE,
    };
    let ground_gate = true;

    // ===== integrate + land =====
    // The landing test runs against the ground of the drop's CURRENT cell (re-sampled on cell
    // change — see [`Drop::land_y`]): a drop drifting under cover "lands" the moment its cell's
    // ground (the roof top, the grid is a max-from-above) rises above it, and its patter sits ON
    // that ground — the drip line splashes on the porch roof, never the room floor.
    let mut landed: Vec<Patter> = Vec::new();
    {
        pool.drops.retain_mut(|d| {
            d.pos += d.vel * dt;
            d.age += dt;
            let cell = HeightCache::key(d.pos.x, d.pos.z);
            if cell != d.cell {
                d.cell = cell;
                d.land_y = heights.ground_y(d.pos.x, d.pos.z, cast_plane, spatial, filter);
            }
            if d.pos.y > d.land_y {
                return true;
            }
            if ground_gate {
                landed.push(Patter {
                    pos: Vec3::new(d.pos.x, d.land_y + 0.02, d.pos.z),
                    age: 0.0,
                    variant: 0, // assigned below (needs the rng)
                });
            }
            false
        });
    }
    for l in &mut landed {
        l.variant = (rand01(rng) * 4.0) as u8 & 3;
    }
    landed.truncate(GROUND_CAP.saturating_sub(pool.patters.len()));
    pool.patters.append(&mut landed);
    pool.patters.retain_mut(|l| {
        l.age += dt;
        l.age < ground_life
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A converged [`WeatherWind`] for a player running Bevy +X at `speed` — driven through the
    /// real 149 ms window rather than hand-built, so these tests also pin the wiring.
    fn wind_at(speed: f32) -> WeatherWind {
        let mut wind = WeatherWind::default();
        let dt = 1.0 / 60.0;
        for i in 0..30 {
            wind.update(Vec3::X * (speed * dt * i as f32), Vec3::X, speed, dt);
        }
        wind
    }

    /// The wire grade the director ran, through the published knee (`0x67bcc8`,
    /// `max(0, (grade−0.25)·4/3)`) — the density the spawn kernel actually sees.
    fn grade(wire: f32) -> f32 {
        ((wire - 0.25) * (4.0 / 3.0)).max(0.0)
    }

    /// The slab **leans into the direction of travel** (`α = 65°·sat(|W|/18)`, `0x677965`–
    /// `0x677a55`): at a 7 yd/s run the leading corner is born ~8 yd up and ~53 yd ahead instead
    /// of a flat 30 up / 45 ahead, and the trailing corner rides up to ~46. Reproduces the
    /// placement law's own worked numbers.
    #[test]
    fn the_spawn_slab_leans_into_the_run() {
        let wind = wind_at(7.0);
        let (half_xy, z_off) = spawn_box(WeatherKind::Snow);
        let lead = wind.slab * Vec3::new(half_xy, z_off, 0.0);
        let trail = wind.slab * Vec3::new(-half_xy, z_off, 0.0);
        assert!(
            (lead.x - 53.5).abs() < 0.5 && (lead.y - 7.9).abs() < 0.5,
            "leading corner {lead:?} — the worked value is (53.5, 7.9)"
        );
        assert!(
            (trail.x + 27.9).abs() < 0.5 && (trail.y - 46.3).abs() < 0.5,
            "trailing corner {trail:?} — the worked value is (−27.9, 46.3)"
        );
        // Standing still there is no heading to lean into, and the slab is flat.
        assert_eq!(wind_at(0.0).slab, Quat::IDENTITY);
    }

    /// The two wind rotations are **separate ramps off the same axis**, not one constant reused:
    /// at 18 yd/s the slab has saturated at 65° while the streak apex is only at 27°. Collapsing
    /// them would silently halve the slab's lean at every running speed.
    #[test]
    fn the_slab_and_streak_tilts_are_distinct_ramps() {
        let wind = wind_at(18.0);
        let deg = |q: Quat| q.angle_between(Quat::IDENTITY).to_degrees();
        assert!(
            (deg(wind.slab) - 65.0).abs() < 0.5,
            "slab {}",
            deg(wind.slab)
        );
        assert!(
            (deg(wind.tilt) - 27.0).abs() < 0.5,
            "streak {}",
            deg(wind.tilt)
        );
    }

    /// **B233's pin — a running player must keep meeting snow head-on.**
    ///
    /// For each particle, ask where it crosses EYE HEIGHT and where the camera is at that
    /// instant. Untilted, a snowflake at the director's grade needs its whole ~7.8 s fall to get
    /// there, and a 7 yd/s runner covers 54 yd in that time against a 45 yd box — so nearly every
    /// arrival lands *behind* them. That is exactly what the live probe measured: the forward eye
    /// band collapsed 25× and hit literally zero, and the field's centroid sat 36 yd back.
    ///
    /// The slab tilt is the term that fixes it, so this test asserts the *contrast*: the fix has
    /// to be worth several times the broken behaviour, not merely nonzero.
    #[test]
    fn a_running_player_still_meets_snow_head_on() {
        let ahead_share = |speed: f32, slab: Quat| {
            let m = grade(0.6);
            let origin = Vec3::X * (speed * WIND_LEAD);
            let mut rng = 0x9E37_79B9_u32;
            let (mut ahead, mut total) = (0u32, 0u32);
            for _ in 0..40_000 {
                let r = [
                    rand01(&mut rng),
                    rand01(&mut rng),
                    rand01(&mut rng),
                    rand01(&mut rng),
                    rand01(&mut rng),
                ];
                let (spawn, vel) = spawn_particle(WeatherKind::Snow, m, origin, slab, r);
                // Born at or below the eye: it never crosses the plane, so it is not an arrival.
                if spawn.y <= 0.0 {
                    continue;
                }
                // The camera runs +X from the origin; eye height is y = 0 and the ground is flat.
                let tau = spawn.y / -vel.y;
                total += 1;
                if (spawn.x + vel.x * tau) - speed * tau > 0.0 {
                    ahead += 1;
                }
            }
            f64::from(ahead) / f64::from(total)
        };

        let standing = ahead_share(0.0, Quat::IDENTITY);
        assert!(
            (standing - 0.5).abs() < 0.05,
            "a standing player's arrivals should split ~50/50, got {standing:.3}"
        );

        let running = wind_at(7.0);
        let tilted = ahead_share(7.0, running.slab);
        let flat = ahead_share(7.0, Quat::IDENTITY);
        eprintln!(
            "snow arrivals ahead of the player: standing {standing:.3}, \
             running tilted {tilted:.3}, running flat (the B233 shape) {flat:.3}"
        );
        assert!(
            flat < 0.10,
            "the untilted slab is supposed to reproduce B233 (nearly nothing arrives ahead of a \
             runner); got {flat:.3} — if this rose, the symptom's cause moved"
        );
        assert!(
            tilted > 0.25,
            "with the slab tilt a runner should still meet a quarter or more of the arrivals \
             head-on (the worked value is ~0.34); got {tilted:.3}"
        );
        assert!(
            tilted > flat * 3.0,
            "the tilt must dominate, not nudge: tilted {tilted:.3} vs flat {flat:.3}"
        );
    }

    /// Rain takes the same rotation and must not change character: its fall is 1.24 s, so it
    /// never had B233's problem, and the tilt shifts its arrival band forward without emptying
    /// either side. (The worked arrival band: `[−61,+69]` → `[−45,+86]`.)
    #[test]
    fn the_tilt_leaves_rain_balanced() {
        let m = grade(0.6);
        let speed = 7.0;
        let slab = wind_at(speed).slab;
        let origin = Vec3::X * (speed * WIND_LEAD);
        let mut rng = 0x1234_5678_u32;
        let (mut ahead, mut total) = (0u32, 0u32);
        for _ in 0..40_000 {
            let r = [
                rand01(&mut rng),
                rand01(&mut rng),
                rand01(&mut rng),
                rand01(&mut rng),
                rand01(&mut rng),
            ];
            let (spawn, vel) = spawn_particle(WeatherKind::Rain, m, origin, slab, r);
            if spawn.y <= 0.0 {
                continue;
            }
            let tau = spawn.y / -vel.y;
            total += 1;
            if (spawn.x + vel.x * tau) - speed * tau > 0.0 {
                ahead += 1;
            }
        }
        let share = f64::from(ahead) / f64::from(total);
        assert!(
            (0.35..0.75).contains(&share),
            "rain should stay balanced fore/aft under the tilt, got {share:.3}"
        );
    }

    /// The whole of decision 1165, as arithmetic: benilla at 60 fps must put the same field on
    /// screen as the **reference install** does at its own `SET maxfps "30"`, because that 30 fps
    /// client is what the director's A/B is judged against. Byte-faithfully we ran at exactly
    /// twice it — same `P`, same `K`, same knee — purely because the reference's per-frame budget
    /// discards its remainder and so thins out below 60 fps.
    #[test]
    fn the_frame_cap_gain_matches_the_reference_installs_throughput() {
        // Nominal snow at wire grade 1.0, weatherDensity 3 (K = 1) — the trace's 14000/s.
        let raw = SNOW_P * grade(1.0);
        // The reference's own law: a per-frame budget of `min(dt, 1/60)·rate` whose sub-unit
        // remainder is discarded, so throughput falls off below 60 fps.
        let per_second = |rate: f32, fps: f32| {
            frame_count(PACKET_CAP, rate * (1.0 / fps).min(DT_CAP)) as f32 * fps
        };

        // What the director is actually looking at: the reference at its `maxfps "30"`.
        let reference = per_second(raw, REF_MAXFPS);
        assert!(
            (reference / (raw * 0.5) - 1.0).abs() < 0.01,
            "a 30-capped reference emits half its nominal rate, got {reference} of {raw}"
        );
        // benilla at 60 fps with the gain lands on that same field…
        let ours = per_second(raw * REF_FPS_GAIN, 60.0);
        assert!(
            (ours / reference - 1.0).abs() < 0.01,
            "ours {ours} must match the reference's {reference}"
        );
        // …where byte-faithfully it ran at exactly twice it. This factor, not any constant, was
        // the whole of the director's "way too dense" on B233.
        assert!((per_second(raw, 60.0) / reference - 2.0).abs() < 0.01);
        // Above 60 fps the reference's own `min(dt, 1/60)` already flattens throughput, so the
        // gain does not compound with a fast machine.
        assert!((per_second(raw * REF_FPS_GAIN, 144.0) / ours - 1.0).abs() < 0.02);
    }

    /// The RNE frame count (`0x6754cc–ef`, x87 round-nearest-even): normal quotas round to
    /// the nearest count, and `space = 1` yields `RNE(0.5) = 0` — the parity stick that keeps
    /// a 6143-record packet from ever full-closing.
    #[test]
    fn frame_count_rounds_nearest_even() {
        assert_eq!(frame_count(6144, 583.3), 583); // RNE(582.8)
        assert_eq!(frame_count(300, 583.3), 300); // space-clamped: RNE(299.5) → 300 (even)
        assert_eq!(frame_count(1, 583.3), 0); // the parity stick: RNE(0.5) → 0
        assert_eq!(frame_count(2, 583.3), 2); // RNE(1.5) → 2 (even)
        assert_eq!(frame_count(6144, 1.2), 1); // RNE(0.7) → 1
    }

    /// The packet clock: baseTime is stamped at OPEN alone (`0x67598c`); a packet re-opens
    /// (re-stamping at the live rate) once past its own baseTime or the `P/6144` build age.
    /// Records replay ONLY after their packet seals (shader legs draw from the close-baked
    /// buffer, `0x6752b0` ← `0x675a97` sole caller).
    #[test]
    fn packet_seals_before_replaying() {
        let mut pool = Pool::default();
        let close_age = RAIN_P / PACKET_CAP as f32; // 35000/6144 ≈ 5.7 s
        let (_, at0) = pool.open_for(0.0, RAIN_P, close_age, Vec3::ZERO);
        assert!((at0 - PACKET_CAP as f32 / RAIN_P).abs() < 1e-3);
        // 0.1 s later, same packet — the stamp is unchanged; the record offset advances.
        let (_, at1) = pool.open_for(0.1, RAIN_P * 2.0, close_age, Vec3::ZERO);
        assert!((at1 - (at0 + 0.1)).abs() < 1e-3);
        // Past the baseTime the packet re-opens at the live rate.
        let (_, at2) = pool.open_for(1.0, RAIN_P * 2.0, close_age, Vec3::ZERO);
        assert!((at2 - (1.0 + PACKET_CAP as f32 / (RAIN_P * 2.0))).abs() < 1e-3);
    }

    /// The type-change cut: the open packet is discarded unbaked, sealed-but-not-yet-
    /// replaying packets are unlinked (`0x67575a`), replaying ones keep their records.
    #[test]
    fn cut_discards_the_unreplayed_pipeline() {
        let d = || Drop {
            pos: Vec3::ZERO,
            vel: Vec3::NEG_Y,
            land_y: -100.0,
            cell: (0, 0),
            age: 0.0,
        };
        let mut pool = Pool {
            open: Some(Packet {
                anchor: Vec3::ZERO,
                opened: 9.0,
                visible_at: 12.0,
                records: vec![Pending {
                    at: 12.0,
                    drop: d(),
                }],
                count: 1,
            }),
            ..Default::default()
        };
        pool.sealed.push(Packet {
            anchor: Vec3::ZERO,
            opened: 0.0,
            visible_at: 1.0, // replaying since t=1
            records: vec![Pending { at: 4.0, drop: d() }],
            count: 1,
        });
        pool.sealed.push(Packet {
            anchor: Vec3::ZERO,
            opened: 8.0,
            visible_at: 60.0, // never started replaying — the straggler
            records: vec![Pending {
                at: 60.5,
                drop: d(),
            }],
            count: 1,
        });
        pool.cut(10.0);
        assert!(pool.open.is_none());
        assert_eq!(pool.sealed.len(), 1, "only the replaying packet survives");
        assert!((pool.sealed[0].visible_at - 1.0).abs() < 1e-6);
    }

    /// The upswing onset under the shader-leg model: on a full upswing the earliest possible
    /// replay is ~4.9 s (packet-1 is always the ~100 s forecast orphan) and the deterministic
    /// fixed-dt path lands in the reference's observed 4.5–15 s onset band — never the
    /// pre-pipeline 2.5 s.
    #[test]
    fn upswing_first_visible_rain_is_gated() {
        let mut pool = Pool::default();
        let close_age = RAIN_P / PACKET_CAP as f32;
        let dt = 1.0 / 60.0;
        let mut first_visible = f32::MAX;
        let mut t = 0.0f32;
        while t < 30.0 {
            let a = (t / 10.0).min(1.0);
            let intensity = ((a - 0.25) * (4.0 / 3.0)).max(0.0);
            let rate = RAIN_P * intensity; // K = 1 (weatherDensity 3)
            let quota = rate * dt;
            if quota > 1.0 {
                let (space, replay_at) = pool.open_for(t, rate, close_age, Vec3::ZERO);
                let n = frame_count(space, quota);
                if let Some(pk) = &mut pool.open {
                    pk.count += n as u32;
                    for _ in 0..n {
                        pk.records.push(Pending {
                            at: replay_at,
                            drop: Drop {
                                pos: Vec3::ZERO,
                                vel: Vec3::NEG_Y,
                                land_y: -100.0,
                                cell: (0, 0),
                                age: 0.0,
                            },
                        });
                    }
                }
            }
            // Replay eligibility: sealed AND past the record instant.
            for pk in &pool.sealed {
                for r in &pk.records {
                    if r.at <= t {
                        first_visible = first_visible.min(r.at.max(pk.visible_at));
                    }
                }
            }
            t += dt;
        }
        assert!(
            (4.5..15.0).contains(&first_visible),
            "first visible rain at {first_visible:.2} s — the reference band is ~5/8.5–14 s"
        );
    }

    /// Retirement condition 3 (`0x6780d3`–`0x678110` against `[0x810008]` = 200.0): a `.go`
    /// teleport **discards** the field and everything queued behind it, instead of leaving it to
    /// fall out at the old position over the ~7.5 s a flake takes to reach the ground. A 150 yd
    /// walk — inside every distance the rule protects — touches nothing.
    #[test]
    fn a_teleport_retires_the_field_and_its_pipeline() {
        let field = || {
            let d = |x: f32| Drop {
                pos: Vec3::new(x, 0.0, 0.0),
                vel: Vec3::NEG_Y,
                land_y: -100.0,
                cell: (0, 0),
                age: 0.0,
            };
            Pool {
                drops: vec![d(0.0), d(40.0)],
                sealed: vec![Packet {
                    anchor: Vec3::ZERO,
                    opened: 0.0,
                    visible_at: 1.0,
                    records: vec![Pending {
                        at: 4.0,
                        drop: d(0.0),
                    }],
                    count: 1,
                }],
                ..Default::default()
            }
        };
        let mut walked = field();
        walked.retire_far(Vec3::new(150.0, 0.0, 0.0), WeatherKind::Snow);
        assert_eq!(walked.drops.len(), 2, "150 yd is inside the guard");
        assert_eq!(walked.sealed.len(), 1);

        let mut jumped = field();
        jumped.retire_far(Vec3::new(500.0, 0.0, 0.0), WeatherKind::Snow);
        assert!(
            jumped.drops.is_empty(),
            "the stranded field is discarded, not drained"
        );
        assert!(
            jumped.sealed.is_empty(),
            "and so is everything queued behind it"
        );
    }

    /// The active-drop half of the cull stands in for a packet test benilla no longer has the
    /// identity to run, so it must be **strictly weaker** than the rule it replaces: keeping a
    /// flake the reference has already retired costs a frame of latency, retiring one the
    /// reference is still drawing is a visible hole. And the anchor rule itself must stay what
    /// the bytes make it — a discontinuity guard that motion can never trip.
    #[test]
    fn the_drop_cull_never_beats_the_reference_to_a_flake() {
        /// Ground speed at +100% (the epic mount), yd/s — the fastest the eye moves continuously.
        const EPIC_MOUNT: f32 = 14.0;
        for kind in [WeatherKind::Rain, WeatherKind::Snow] {
            let (half_xy, z_off) = spawn_box(kind);
            let corner = 2.0f32.mul_add(half_xy * half_xy, z_off * z_off).sqrt();
            // The widest a flake the reference is STILL DRAWING can sit from the eye: its
            // packet's anchor is inside 200, the flake is inside the box's corner of the emission
            // origin, and that origin leads the eye by `1.75·W`.
            let widest_drawn = RETIRE_DIST + corner + WIND_LEAD * EPIC_MOUNT;
            let ours = RETIRE_DIST + corner + RETIRE_DROP_SLACK;
            assert!(
                ours >= widest_drawn,
                "{kind:?}: culling at {ours:.1} yd would beat the reference's {widest_drawn:.1} yd"
            );
        }
        // A snow packet lives at most `close_age + fall_time`; even at epic-mount speed that
        // carries the eye barely half of `RETIRE_DIST`, so nothing but a teleport ever trips it.
        let packet_life = SNOW_P / PACKET_CAP as f32 + SNOW_Z_OFF / (SNOW_VZ_BASE + SNOW_VZ_W);
        assert!(
            packet_life * EPIC_MOUNT < RETIRE_DIST,
            "a packet outlives {:.0} yd of running — the anchor cull is no longer teleport-only",
            packet_life * EPIC_MOUNT
        );
    }
}
