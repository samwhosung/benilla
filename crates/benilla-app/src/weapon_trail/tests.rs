//! The swing arithmetic, pinned against `0x6c6560`'s own numbers.

use super::*;

/// A shipped proc: kit 324's — `#f82929`, alpha 100, 600 ms.
fn kit_324() -> TrailProc {
    TrailProc {
        packed: 0x64f8_2929,
        duration_ms: 600,
    }
}

fn armed(trail: TrailProc) -> WeaponTrail {
    let mut t = WeaponTrail::new(Vec3::Y, Vec3::ZERO);
    t.arm(trail, 0);
    t
}

/// `0x6c6560`'s arithmetic, worked: with the shipped `CharParamThree = 100` and a 16.7 ms frame,
/// `fadeStep = 5`, so `nSegments = 20` — twenty appended frames ≈ 0.33 s of trail history.
#[test]
fn shipped_frame_gives_five_and_twenty() {
    for dt in [16, 17] {
        assert_eq!(fade_step(dt, 100), 5, "dt {dt} ms at alpha 100");
    }
    assert_eq!(100 / fade_step(16, 100), 20);
}

/// The floor is `cmp al,1` — a **low-byte** test, so a result of exactly 256 takes the floor
/// rather than its own value, and a long hitch does not end the trail.
#[test]
fn fade_step_floor_is_the_low_byte() {
    assert_eq!(fade_step(0, 100), 1, "a zero delta floors");
    assert_eq!(fade_step(1, 100), 1, "trunc(0.33) = 0 floors");
    assert_eq!(
        fade_step(768, 100),
        1,
        "trunc = 256, low byte 0 -> the floor"
    );
    assert_eq!(fade_step(771, 100), 257, "trunc = 257, low byte 1 -> kept");
}

/// The writer masks `& 127` and the reader divides `% 127`. Below the capacity they agree
/// exactly; past it the reader is off by one counter, which swaps the ribbon's two edges.
#[test]
fn ring_read_and_write_diverge_past_127() {
    let mut t = armed(kit_324());
    let s = t.swing.as_mut().expect("armed");
    for i in 0..127u32 {
        s.push(Vec3::splat(i as f32));
    }
    for i in 0..127u32 {
        assert_eq!(s.read(i), Vec3::splat(i as f32), "counter {i} is exact");
    }
    // Counter 127 writes slot 127; counter 128 writes slot 0, and the reader's `128 % 127 = 1`
    // picks up slot 1 — the sample written by counter 129, one AHEAD of the one it wants.
    for i in 127..135u32 {
        s.push(Vec3::splat(i as f32));
    }
    assert_eq!(
        s.read(128),
        Vec3::splat(129.0),
        "the reader lands one counter late — the shipped 5875 mismatch, not a slip"
    );
}

/// The ring never reports more than its capacity, and the floor follows the head.
#[test]
fn push_bounds_the_ring_at_capacity() {
    let mut t = armed(kit_324());
    let s = t.swing.as_mut().expect("armed");
    for i in 0..300u32 {
        s.push(Vec3::splat(i as f32));
        assert!(s.head - s.tail <= RING_SLOTS, "at push {i}");
    }
    assert_eq!(s.head - s.tail, RING_SLOTS);
}

/// Re-arming a live swing resets its ring in place (`0x6c675f`) rather than stacking a second
/// trail — the same creation-time law the effect nodes take (2057).
#[test]
fn rearm_resets_rather_than_stacks() {
    let mut t = armed(kit_324());
    let s = t.swing.as_mut().expect("armed");
    for i in 0..40u32 {
        s.push(Vec3::splat(i as f32));
    }
    assert_eq!(s.head, 40);
    t.arm(kit_324(), 5_000);
    let s = t.swing.as_ref().expect("still armed");
    assert_eq!((s.head, s.tail), (0, 0), "the ring restarts");
    assert_eq!(s.start_ms, 5_000, "and so does the clock");
}

/// One whole swing at 60 Hz, kit 324: how long it lives, how long it gets, and that it dies.
#[test]
fn kit_324_at_sixty_hz() {
    let mut t = armed(kit_324());
    let (mut frame, mut longest, mut appended) = (0u32, 0usize, 0u32);
    let end = loop {
        frame += 1;
        let now = frame * 1000 / 60;
        let blade = Vec3::new(frame as f32 * 0.1, 0.0, 0.0);
        let swing = t.swing.as_mut().expect("live");
        let before = swing.head;
        match swing.step(now, blade, blade + Vec3::Y) {
            Step::Ended => break now,
            Step::Strip(s) => {
                longest = longest.max(s.len());
                appended += (swing.head - before) / 2;
            }
        }
        assert!(frame < 10_000, "the trail must terminate");
    };
    assert_eq!(appended, 35, "appends for the 600 ms duration at 60 Hz");
    // The nominal is 20 pairs (alpha 100 / fadeStep 5), but `fadeStep` is a TRUNCATED product,
    // so as the alpha decays the integer quotient drifts *above* it — alpha 68 at a 16 ms frame
    // truncates to step 3 and asks for 22. The ring's 64 pairs is the real ceiling.
    assert_eq!(
        longest, 22,
        "the retained history peaks just over the nominal 20 pairs"
    );
    // Holding `fadeStep` constant puts the visible life at ≈ duration/2 + (alpha/fadeStep)·dt ≈
    // duration. It is not constant — it is `trunc(dt·alpha/300)`, so it shrinks
    // with the alpha it is decaying, and the truncation shaves it further (alpha 54 asks for 2.88
    // and gets 2). The decay is therefore sub-exponential and a 600 ms kit is on screen for
    // **twice** its duration.
    assert_eq!(
        end, 1_250,
        "a 600 ms kit is visible for 1.25 s, not the constant-step ~0.6 s"
    );
}

/// The alpha ramp's DIRECTION — the one thing a re-implementation is most likely to invert.
/// The oldest retained pair is the opaque end; the pair at the weapon is the transparent one,
/// and once the ring holds its full `2·segments` the ramp reaches exactly 0 there.
#[test]
fn the_far_end_of_the_arc_is_the_opaque_end() {
    let mut t = armed(kit_324());
    let swing = t.swing.as_mut().expect("live");
    let mut strip = Vec::new();
    // Twenty appended frames is where the ring first holds the full `alpha/fadeStep` segments.
    for frame in 1..=20u32 {
        let blade = Vec3::new(frame as f32, 0.0, 0.0);
        match swing.step(frame * 1000 / 60, blade, blade + Vec3::Y) {
            Step::Strip(s) => strip = s,
            Step::Ended => panic!("still within the duration"),
        }
    }
    let alphas: Vec<f32> = strip.iter().map(|&(_, _, a)| a).collect();
    assert!(alphas.len() > 2);
    assert!(
        alphas.windows(2).all(|w| w[0] > w[1]),
        "alpha must fall from the oldest sample to the weapon: {alphas:?}"
    );
    // One `fadeStep` per pair, subtracted before the pair is stored — so the ramp is arithmetic
    // and its last term is exactly zero.
    let drop = alphas[0] - alphas[1];
    assert!(
        alphas.windows(2).all(|w| (w[0] - w[1] - drop).abs() < 1e-6),
        "a constant fadeStep per pair: {alphas:?}"
    );
    assert_eq!(
        alphas[alphas.len() - 1],
        0.0,
        "the end at the blade is transparent: {alphas:?}"
    );
    assert!(
        alphas[0] > 0.3,
        "and the far end is the opaque one: {alphas:?}"
    );
}

/// Before the ring fills, the oldest pair carries `alpha − fadeStep` outright — 95/255 at the
/// shipped alpha of 100.
#[test]
fn the_oldest_pair_carries_alpha_minus_one_step() {
    let mut t = armed(kit_324());
    let swing = t.swing.as_mut().expect("live");
    let mut strip = Vec::new();
    for frame in 1..=10u32 {
        match swing.step(frame * 1000 / 60, Vec3::ZERO, Vec3::Y) {
            Step::Strip(s) => strip = s,
            Step::Ended => panic!("still within the duration"),
        }
    }
    assert_eq!(swing.alpha, 100, "still inside the first half");
    assert!((strip[0].2 - 95.0 / 255.0).abs() < 1e-6, "{}", strip[0].2);
}

/// A trail holds full strength for its first half; the decay only starts past `duration/2`.
#[test]
fn alpha_holds_until_the_half_duration_mark() {
    let mut t = armed(kit_324());
    let swing = t.swing.as_mut().expect("live");
    for frame in 1..=22u32 {
        let now = frame * 1000 / 60;
        assert!(matches!(
            swing.step(now, Vec3::ZERO, Vec3::Y),
            Step::Strip(_)
        ));
        if now <= 300 {
            assert_eq!(swing.alpha, 100, "still at full strength at {now} ms");
        }
    }
    assert!(swing.alpha < 100, "and decaying past the mark");
}

/// Whirlwind's 10 000 ms spin is the one shipped kit that outruns the ring, so it is the one the
/// reader/writer modulus mismatch is visible on.
#[test]
fn only_whirlwind_outruns_the_ring() {
    let whirlwind = TrailProc {
        packed: 0x64f7_1717,
        duration_ms: 10_000,
    };
    for (trail, outruns) in [(kit_324(), false), (whirlwind, true)] {
        let mut t = armed(trail);
        let mut frame = 0u32;
        let head = loop {
            frame += 1;
            let swing = t.swing.as_mut().expect("live");
            if matches!(
                swing.step(frame * 1000 / 60, Vec3::ZERO, Vec3::Y),
                Step::Ended
            ) {
                break swing.head;
            }
            assert!(frame < 10_000);
        };
        assert_eq!(
            head > READ_MODULUS,
            outruns,
            "{} ms appended {head} samples",
            trail.duration_ms
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The latch: armed by the kit, consumed by the unit's next animation, one slot per unit.

use crate::creature_anim::AnimDriver;
use crate::entities::{HeldAttached, ATTACH_SLOT_NAMES};

/// A unit holding a trail-capable weapon in each hand, plus the app that fires them.
fn latch_app() -> (App, Entity, [Entity; 2]) {
    let mut app = App::new();
    app.add_message::<TrailArm>()
        .init_resource::<TrailLatch>()
        .init_resource::<Time>()
        .add_systems(Update, fire_weapon_trails);
    let hands = [
        app.world_mut()
            .spawn(WeaponTrail::new(Vec3::Y, Vec3::ZERO))
            .id(),
        app.world_mut()
            .spawn(WeaponTrail::new(Vec3::Y, Vec3::ZERO))
            .id(),
    ];
    let mut spawned = [None; ATTACH_SLOT_NAMES.len()];
    spawned[0] = Some(hands[0]);
    spawned[1] = Some(hands[1]);
    let unit = app
        .world_mut()
        .spawn((AnimDriver::default(), HeldAttached::with_spawned(spawned)))
        .id();
    (app, unit, hands)
}

fn trailing(app: &App, hand: Entity) -> bool {
    app.world()
        .get::<WeaponTrail>(hand)
        .expect("the hand exists")
        .swing
        .is_some()
}

fn play_anim(app: &mut App, unit: Entity) {
    app.world_mut()
        .get_mut::<AnimDriver>(unit)
        .expect("the unit exists")
        .set_started_anim(true);
}

/// The proc ARMS; it does not draw. Nothing happens until the unit plays an animation — which is
/// what makes Charge (kit 44, anim id −1) start its trail on the charge itself.
#[test]
fn an_arm_alone_fires_nothing() {
    let (mut app, unit, hands) = latch_app();
    app.world_mut().write_message(TrailArm {
        entity: unit,
        trail: kit_324(),
    });
    app.update();
    assert!(
        hands.iter().all(|&h| !trailing(&app, h)),
        "no animation yet"
    );
    play_anim(&mut app, unit);
    app.update();
    assert!(
        hands.iter().all(|&h| trailing(&app, h)),
        "the next animation fires it — on BOTH hands (0x60e550's two slots)"
    );
}

/// `0x5fe4b1 mov [ebx+0xd20],edi` — the duration is cleared the moment it is read, so one arm is
/// one trail. A second animation must not restart it.
#[test]
fn the_latch_is_consumed_exactly_once() {
    let (mut app, unit, hands) = latch_app();
    app.world_mut().write_message(TrailArm {
        entity: unit,
        trail: kit_324(),
    });
    play_anim(&mut app, unit);
    app.update();
    assert!(trailing(&app, hands[0]));
    // Clear the swing and play again: with the latch consumed, nothing re-arms.
    app.world_mut()
        .get_mut::<WeaponTrail>(hands[0])
        .expect("the hand exists")
        .swing = None;
    play_anim(&mut app, unit);
    app.update();
    assert!(!trailing(&app, hands[0]), "the arm was one-shot");
}

/// One pair of fields on the unit means one pending arm: a second proc before the first fires
/// OVERWRITES it rather than queueing behind it.
#[test]
fn a_second_arm_replaces_the_pending_one() {
    let (mut app, unit, hands) = latch_app();
    let whirlwind = TrailProc {
        packed: 0x64f7_1717,
        duration_ms: 10_000,
    };
    for trail in [kit_324(), whirlwind] {
        app.world_mut().write_message(TrailArm {
            entity: unit,
            trail,
        });
    }
    play_anim(&mut app, unit);
    app.update();
    let hand = app
        .world()
        .get::<WeaponTrail>(hands[0])
        .expect("the hand exists");
    let swing = hand.swing.as_ref().expect("armed");
    assert_eq!(swing.duration_ms, 10_000, "the LAST arm wins");
}

/// A hand holding nothing — or a weapon whose model authors neither marker — has no object to
/// arm, and the other hand is unaffected (`0x60e550`'s `test ecx,ecx ; je` per slot).
#[test]
fn an_empty_hand_is_skipped() {
    let (mut app, unit, hands) = latch_app();
    app.world_mut().entity_mut(hands[1]).remove::<WeaponTrail>();
    app.world_mut().write_message(TrailArm {
        entity: unit,
        trail: kit_324(),
    });
    play_anim(&mut app, unit);
    app.update();
    assert!(trailing(&app, hands[0]), "the armed hand still fires");
}

/// The pending arm dies with the unit — the reference drops it in the CGUnit teardown
/// (`0x5fbcdc`), eight instructions after destroying both trail objects.
#[test]
fn the_latch_is_swept_when_the_unit_goes() {
    let (mut app, unit, _) = latch_app();
    app.world_mut().write_message(TrailArm {
        entity: unit,
        trail: kit_324(),
    });
    app.update();
    assert_eq!(app.world().resource::<TrailLatch>().0.len(), 1);
    app.world_mut().entity_mut(unit).despawn();
    app.update();
    assert!(
        app.world().resource::<TrailLatch>().0.is_empty(),
        "the arm goes with the unit"
    );
}

// ---------------------------------------------------------------------------------------------
// The draw: an armed trail must actually reach the shared effect stream.

use benilla_world::particles::buffer::{begin_effect_frame, EffectQuads};

/// End to end through the render half: an armed swing on a moving blade commits vertices, they
/// carry the kit's colour and the ramp, and the draw stops when the trail does.
#[test]
fn an_armed_trail_commits_a_strip_to_the_effect_stream() {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<Assets<Image>>()
        .init_resource::<EffectQuads>()
        .add_systems(Startup, init_trail_white)
        .add_systems(Update, (begin_effect_frame, draw_weapon_trails).chain());
    let cam = app.world_mut().spawn(WorldCamera).id();
    let _ = cam;
    let wearer = app.world_mut().spawn_empty().id();
    let mut trail = WeaponTrail::new(Vec3::Y, Vec3::ZERO);
    trail.arm(kit_324(), 0);
    let hand = app
        .world_mut()
        .spawn((
            trail,
            Transform::default(),
            GlobalTransform::default(),
            InheritedVisibility::VISIBLE,
            benilla_world::model_fade::ParentModel(wearer),
        ))
        .id();
    // Frame 1 seeds one pair; a strip needs two, so the first commit is empty by construction.
    app.update();
    // Sweep the blade so the samples are distinct, and let the ring fill.
    for f in 1..=8u32 {
        app.world_mut()
            .get_mut::<Transform>(hand)
            .expect("the hand exists")
            .translation = Vec3::new(f as f32 * 0.2, 0.0, 0.0);
        app.world_mut()
            .get_mut::<GlobalTransform>(hand)
            .expect("the hand exists")
            .clone_from(&GlobalTransform::from_translation(Vec3::new(
                f as f32 * 0.2,
                0.0,
                0.0,
            )));
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
    }
    let quads = app.world().resource::<EffectQuads>();
    assert_eq!(quads.draws.len(), 1, "one draw for the one live trail");
    let draw = &quads.draws[0];
    let verts = &quads.verts[draw.range.start as usize..draw.range.end as usize];
    assert!(verts.len() >= 8, "at least two quads: {}", verts.len());
    assert_eq!(verts.len() % 4, 0, "whole quads");
    // Kit 324's #f82929, and an alpha ramp that actually varies across the strip.
    for v in verts {
        assert!((v.color[0] - 248.0 / 255.0).abs() < 1e-6, "{:?}", v.color);
        assert!((v.color[1] - 41.0 / 255.0).abs() < 1e-6, "{:?}", v.color);
    }
    // Eight appended frames is short of the 20 the ring will hold, so the ramp has not yet
    // reached 0 at the blade — but it must already fall, by one `fadeStep` per quad, from the
    // opaque far end (alpha − fadeStep = 95/255) toward it.
    let alphas: Vec<f32> = verts.iter().map(|v| v.color[3]).collect();
    let (lo, hi) = (
        alphas.iter().cloned().fold(f32::MAX, f32::min),
        alphas.iter().cloned().fold(f32::MIN, f32::max),
    );
    assert!((hi - 95.0 / 255.0).abs() < 1e-6, "the opaque end: {hi}");
    assert!(hi - lo > 0.1, "a real ramp, not one flat alpha: {alphas:?}");
    // Each quad is `[b0, b1, t1, t0]` — the two ends of one segment, so its corners pair up.
    for q in verts.as_chunks::<4>().0 {
        assert_eq!(
            q[0].color[3], q[3].color[3],
            "the older pair shares an alpha"
        );
        assert_eq!(q[1].color[3], q[2].color[3], "and so does the newer");
        assert!(q[0].color[3] > q[1].color[3], "falling toward the weapon");
    }
    // The blade swept along +X, so the strip must span more than a single blade's worth of it.
    let xs: Vec<f32> = verts.iter().map(|v| v.pos[0]).collect();
    let span =
        xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min);
    assert!(span > 0.5, "the ribbon follows the blade's arc: {span}");
}

/// A hidden prop draws no trail — our own vis chain, the same gate [`crate::bowstring`] takes.
#[test]
fn a_hidden_weapon_draws_nothing() {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<Assets<Image>>()
        .init_resource::<EffectQuads>()
        .add_systems(Startup, init_trail_white)
        .add_systems(Update, (begin_effect_frame, draw_weapon_trails).chain());
    app.world_mut().spawn(WorldCamera);
    let wearer = app.world_mut().spawn_empty().id();
    let mut trail = WeaponTrail::new(Vec3::Y, Vec3::ZERO);
    trail.arm(kit_324(), 0);
    app.world_mut().spawn((
        trail,
        Transform::default(),
        GlobalTransform::default(),
        InheritedVisibility::HIDDEN,
        benilla_world::model_fade::ParentModel(wearer),
    ));
    for _ in 0..6 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
    }
    assert!(app.world().resource::<EffectQuads>().draws.is_empty());
}

/// **The trail takes its WEARER's light, not the scene's** (decision 2086).
///
/// The draw runs inside the weapon model's own per-frame callback, during the wearer's model draw,
/// so the enabled lights it inherits are the ones the M2 collector committed for that unit —
/// four-way byte-derived through `0x70d982 → 0x70ca50 → 0x70baf0` plus the held-weapon `[+0x3b8]`
/// alias. Outdoors that is the day/night ambient, which is the fallback. Indoors it is the light
/// node's own committed word, and taking the sky's instead tinted every indoor trail cool.
#[test]
fn an_indoor_wearers_committed_ambient_wins_over_the_scenes() {
    fn strip_rgb(app: &App) -> [f32; 3] {
        let quads = app.world().resource::<EffectQuads>();
        let draw = quads.draws.first().expect("one draw");
        let v = quads.verts[draw.range.start as usize];
        [v.color[0], v.color[1], v.color[2]]
    }
    // A Goldshire-inn character's committed ambient word against a night sky's.
    const ROOM: [f32; 3] = [0.298, 0.216, 0.141];
    const SKY: [f32; 3] = [0.224, 0.259, 0.290];

    let mut lit = [[0.0; 3]; 2];
    for (i, indoors) in [false, true].into_iter().enumerate() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Assets<Image>>()
            .init_resource::<EffectQuads>()
            .insert_resource({
                let mut l = benilla_world::lighting::WowLighting::default();
                l.ambient = SKY;
                l
            })
            .add_systems(Startup, init_trail_white)
            .add_systems(Update, (begin_effect_frame, draw_weapon_trails).chain());
        app.world_mut().spawn(WorldCamera);
        let mut wearer = app.world_mut().spawn_empty();
        if indoors {
            wearer.insert(benilla_world::interior::NodeAmbient(ROOM));
        }
        let wearer = wearer.id();
        let mut trail = WeaponTrail::new(Vec3::Y, Vec3::ZERO);
        trail.arm(kit_324(), 0);
        let hand = app
            .world_mut()
            .spawn((
                trail,
                Transform::default(),
                GlobalTransform::default(),
                InheritedVisibility::VISIBLE,
                benilla_world::model_fade::ParentModel(wearer),
            ))
            .id();
        for f in 1..=4u32 {
            app.world_mut()
                .get_mut::<GlobalTransform>(hand)
                .expect("the hand exists")
                .clone_from(&GlobalTransform::from_translation(Vec3::X * f as f32));
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(16));
            app.update();
        }
        lit[i] = strip_rgb(&app);
    }
    // Kit 324's #f82929 through each ambient. The two are close in magnitude and opposite in hue,
    // so a channel RATIO is the assertion that would actually catch the wrong one being used.
    let [out_r, out_g, _] = lit[0];
    let [in_r, in_g, _] = lit[1];
    assert!(
        (out_r / (248.0 / 255.0) - SKY[0]).abs() < 1e-5,
        "outdoors takes the scene ambient: {lit:?}"
    );
    assert!(
        (in_r / (248.0 / 255.0) - ROOM[0]).abs() < 1e-5,
        "indoors takes the wearer's committed word: {lit:?}"
    );
    assert!(
        in_r > out_r && in_g < out_g,
        "the room is warmer AND redder than the sky — the hue flip is the visible half: {lit:?}"
    );
}
