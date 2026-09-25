//! Tests for the aura-state CharProc layer over shipped kit rows (`benilla-extract charprocs`), so
//! a schema or column-order regression shows as a wrong number, not a silent nothing.

use std::collections::HashMap;

use benilla_formats::{
    char_proc_type, CharProc, SpellVisualCatalog, VisualKit, VisualStages, KIT_CHAR_PROCS,
};
use benilla_protocol::messages::ObjectFields;

use super::*;
use crate::creature_anim::SpellVisuals;

// ── The shipped chains ───────────────────────────────────────────────────────────────────────────

/// Stealth rank 1 → `SpellVisual` 184 → state kit 312: proc 7 and proc 14 at 0.3, nothing else.
const STEALTH: u32 = 1784;
const STEALTH_VISUAL: u32 = 184;
const STEALTH_KIT: u32 = 312;
/// The ghost aura → visual 886 → kit 989: proc 1 tint `9222653.0` (`0x8CB9FD`) and proc 14 at 0.5.
const GHOST: u32 = 8326;
const GHOST_VISUAL: u32 = 886;
const GHOST_KIT: u32 = 989;
/// Ice Block → `SpellVisual` 4325 → state kit 3709: proc 1 tint `9074175.0` and proc 11 at 0.0.
const ICE_BLOCK: u32 = 11958;
const ICE_BLOCK_VISUAL: u32 = 4325;
const ICE_BLOCK_KIT: u32 = 3709;

fn proc(ty: i32, param0: f32) -> CharProc {
    CharProc {
        ty,
        params: [param0, 0.0, 0.0, 0.0],
    }
}

fn proc_kit(procs: &[CharProc]) -> VisualKit {
    let mut slots = [None; KIT_CHAR_PROCS];
    for (slot, p) in slots.iter_mut().zip(procs) {
        *slot = Some(*p);
    }
    VisualKit {
        char_proc_slots: slots,
        ..Default::default()
    }
}

// ── The dispatch ─────────────────────────────────────────────────────────────────────────────────

/// [`node_for`] follows the client's proc jump table: 14 alpha, 1 tint, 11 rate, else no node.
#[test]
fn the_dispatch_names_only_the_verified_procs() {
    assert_eq!(
        node_for(proc(char_proc_type::ALPHA, 0.3)),
        Some(AuraNode::Alpha(0.3)),
        "the sneak family's 0.3 rides params[0]"
    );
    assert_eq!(
        node_for(proc(char_proc_type::ALPHA, 0.0)),
        Some(AuraNode::Alpha(0.0)),
        "kit 5129's 0.0 is a real value, not an absent one"
    );
    // 9222653 is 0x8CB9FD, the ghost's blue-white; the client ORs on the opaque byte (`0x60d8cc`).
    assert_eq!(
        node_for(proc(char_proc_type::TINT, 9_222_653.0)),
        Some(AuraNode::Tint([0x8C, 0xB9, 0xFD])),
    );
    // 65280 is 0x00FF00, the poison state kit's green.
    assert_eq!(
        node_for(proc(char_proc_type::TINT, 65_280.0)),
        Some(AuraNode::Tint([0x00, 0xFF, 0x00])),
    );
    // Proc 11's rate passes on unexamined (`0x60db7e` → `SetBoneAnimSpeed` `0x712910`).
    assert_eq!(
        node_for(proc(char_proc_type::ANIM_RATE, 0.0)),
        Some(AuraNode::AnimRate(0.0)),
        "the freeze family's rate rides params[0]"
    );
    assert_eq!(
        node_for(proc(char_proc_type::ANIM_RATE, 8_947_848.0)),
        Some(AuraNode::AnimRate(8_947_848.0)),
    );
    // Types on shipped state kits whose mechanism is untraced get no node, so none arms a kit.
    for ty in [2, 7, 8, 13] {
        assert_eq!(node_for(proc(ty, 1.0)), None, "type {ty} is unmodelled");
    }
}

// ── The target and the ramp ──────────────────────────────────────────────────────────────────────

/// `0x60d180`: the target is `baseAlpha` times the head (newest) node's factor alone.
#[test]
fn the_target_is_base_times_the_newest_node() {
    let mut n = AuraNodes::new(1.0);
    assert_eq!(n.target(), 1.0, "no nodes → opaque");

    install(&mut n, STEALTH, &[AuraNode::Alpha(0.3)], 0.0);
    assert!((n.target() - 0.3).abs() < 1e-6);

    install(&mut n, GHOST, &[AuraNode::Alpha(0.5)], 0.0);
    assert!(
        (n.target() - 0.5).abs() < 1e-6,
        "the newest node is the term"
    );

    n.alpha.retain(|(s, _)| *s != GHOST);
    n.retarget(0.0);
    assert!((n.target() - 0.3).abs() < 1e-6);

    // A translucent display's base (128/255) scales the head term, or stands alone with no node.
    let mut half = AuraNodes::new(1.0);
    half.base = 128.0 / 255.0;
    install(&mut half, STEALTH, &[AuraNode::Alpha(0.3)], 0.0);
    assert!((half.target() - (128.0 / 255.0) * 0.3).abs() < 1e-6);
    half.alpha.clear();
    half.retarget(0.0);
    assert!((half.target() - 128.0 / 255.0).abs() < 1e-6);
}

/// A display swap's base change ([`refresh_base_alpha`]) rides the same ramp (`0x614f80`).
#[test]
fn a_base_change_rides_the_same_ramp() {
    let wolf = 102.0 / 255.0;
    let mut n = AuraNodes::new(1.0);
    n.base = wolf;
    n.retarget(0.0);
    assert!((n.tick(0.0) - 1.0).abs() < 1e-6, "starts where it was");
    assert!(
        (n.tick(AURA_ALPHA_FADE_SECS) - wolf).abs() < 1e-6,
        "arrives"
    );
    assert!(n.translucent());

    n.base = 1.0;
    n.retarget(AURA_ALPHA_FADE_SECS);
    assert!((n.from - wolf).abs() < 1e-6);
    assert!((n.tick(2.0 * AURA_ALPHA_FADE_SECS) - 1.0).abs() < 1e-6);
    assert!(!n.translucent(), "settled opaque again");
}

/// Nodes are keyed by spell id (`node+0x18`): a re-applied aura replaces its node.
#[test]
fn re_arming_the_same_spell_replaces_its_node() {
    let mut n = AuraNodes::new(1.0);
    install(&mut n, STEALTH, &[AuraNode::Alpha(0.3)], 0.0);
    install(&mut n, STEALTH, &[AuraNode::Alpha(0.3)], 0.0);
    assert_eq!(n.alpha.len(), 1);
    assert!((n.target() - 0.3).abs() < 1e-6);
}

/// `StartAlphaFade(target, 1000 ms)` eased `clamp01(t)³` both ways, retargeted from the live value.
#[test]
fn the_ramp_eases_over_one_second_both_ways() {
    let mut n = AuraNodes::new(1.0);
    install(&mut n, STEALTH, &[AuraNode::Alpha(0.3)], 0.0);

    assert!((n.tick(0.0) - 1.0).abs() < 1e-6, "t=0 is still opaque");
    // t³ at the halfway mark is 0.125 of the way down.
    let half = n.tick(0.5);
    assert!(
        (half - (1.0 + (0.3 - 1.0) * 0.125)).abs() < 1e-6,
        "cubic ease, not linear (got {half})"
    );
    assert!(
        (n.tick(AURA_ALPHA_FADE_SECS) - 0.3).abs() < 1e-6,
        "arrived at 1000 ms"
    );
    assert!((n.tick(5.0) - 0.3).abs() < 1e-6, "and clamps past the end");
    assert!(n.translucent());

    n.alpha.clear();
    n.retarget(AURA_ALPHA_FADE_SECS);
    assert!((n.from - 0.3).abs() < 1e-6, "ramps from the live value");
    assert!((n.to - 1.0).abs() < 1e-6);
    assert!((n.tick(2.0 * AURA_ALPHA_FADE_SECS) - 1.0).abs() < 1e-6);
    assert!(!n.translucent(), "settled opaque again");

    let mut m = AuraNodes::new(1.0);
    install(&mut m, STEALTH, &[AuraNode::Alpha(0.3)], 0.0);
    let mid = m.tick(0.7);
    m.alpha.clear();
    m.retarget(0.7);
    assert!((m.from - mid).abs() < 1e-6);
}

#[test]
fn a_retarget_to_the_same_value_does_not_restart_the_ease() {
    let mut n = AuraNodes::new(1.0);
    install(&mut n, STEALTH, &[AuraNode::Alpha(0.3)], 0.0);
    n.tick(AURA_ALPHA_FADE_SECS);
    let started = n.started;
    install(&mut n, STEALTH, &[AuraNode::Alpha(0.3)], 10.0);
    assert_eq!(n.started, started, "same product → the ramp is untouched");
    assert!((n.tick(10.0) - 0.3).abs() < 1e-6, "and stays arrived");
}

/// Proc 1's nodes link at the head of `unit+0xce0`, so the newest aura's tint applies.
#[test]
fn the_tint_head_node_is_the_newest_installed() {
    let mut n = AuraNodes::new(1.0);
    assert_eq!(n.head_tint(), None);
    install(&mut n, GHOST, &[AuraNode::Tint([0x8C, 0xB9, 0xFD])], 0.0);
    install(&mut n, 12881, &[AuraNode::Tint([0x00, 0xFF, 0x00])], 0.0);
    assert_eq!(n.head_tint(), Some([0x00, 0xFF, 0x00]));
    n.tint.retain(|(s, _)| *s != 12881);
    assert_eq!(n.head_tint(), Some([0x8C, 0xB9, 0xFD]));
    assert_eq!(n.target(), 1.0, "a tint node is not an alpha term");
}

// ── The watcher → drain edge ─────────────────────────────────────────────────────────────────────

fn app_with_chains() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<crate::creature_anim::SpellKitFx>();
    app.add_message::<crate::net::FieldChanged>();
    app.add_message::<crate::creature_anim::SpellKitSound>();
    app.add_message::<AuraProc>();
    app.add_message::<crate::creature_anim::BaseAnimRecompute>();
    app.init_resource::<benilla_world::model_render::FarSideTwins>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                STEALTH_VISUAL,
                VisualStages {
                    state: STEALTH_KIT,
                    ..Default::default()
                },
            ),
            (
                GHOST_VISUAL,
                VisualStages {
                    state: GHOST_KIT,
                    ..Default::default()
                },
            ),
            (
                ICE_BLOCK_VISUAL,
                VisualStages {
                    state: ICE_BLOCK_KIT,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::from([
            // Kit 312's shipped order: the unmodelled proc 7 must not shadow proc 14 after it.
            (
                STEALTH_KIT,
                proc_kit(&[proc(7, 1.0), proc(char_proc_type::ALPHA, 0.3)]),
            ),
            (
                GHOST_KIT,
                proc_kit(&[
                    proc(char_proc_type::TINT, 9_222_653.0),
                    proc(char_proc_type::ALPHA, 0.5),
                ]),
            ),
            (
                ICE_BLOCK_KIT,
                proc_kit(&[
                    proc(char_proc_type::TINT, 9_074_175.0),
                    proc(char_proc_type::ANIM_RATE, 0.0),
                ]),
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(HashMap::from([
            (
                STEALTH,
                benilla_formats::SpellDisplay {
                    visual: STEALTH_VISUAL,
                    ..Default::default()
                },
            ),
            (
                GHOST,
                benilla_formats::SpellDisplay {
                    visual: GHOST_VISUAL,
                    ..Default::default()
                },
            ),
            (
                ICE_BLOCK,
                benilla_formats::SpellDisplay {
                    visual: ICE_BLOCK_VISUAL,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(
        Update,
        (
            crate::creature_anim::arm_aura_state_fx_for_test,
            drain_aura_procs,
        )
            .chain(),
    );
    app
}

fn app_with_tint_channel() -> App {
    let mut app = app_with_chains();
    app.init_resource::<benilla_world::instance_tint::InstanceTints>();
    app.init_resource::<benilla_world::rig_palette::RigPalettes>();
    app.add_systems(Update, apply_aura_tint.after(drain_aura_procs));
    app
}

/// Stealth's proc-only state kit (no effect models) still installs its proc-14 node.
#[test]
fn a_proc_only_state_kit_arms_stealth_translucency() {
    let mut app = app_with_chains();

    // The aura lands in slot 0 (UNIT_FIELD_AURA[0], field 47) with an occupied AURAFLAGS nibble.
    let stealthed = ObjectFields::from_pairs(&[(47, STEALTH), (95, 0x0E)]);
    let unit = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            stealthed.into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();

    let n = app
        .world()
        .entity(unit)
        .get::<AuraNodes>()
        .expect("the sneak kit armed a node set");
    assert_eq!(n.alpha.as_slice(), [(STEALTH, 0.3)]);
    assert!((n.target() - 0.3).abs() < 1e-6);
    assert!(n.head_tint().is_none(), "kit 312 carries no tint");

    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        ObjectFields::from_pairs(&[(95, 0)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.update();
    let n = app.world().entity(unit).get::<AuraNodes>().unwrap();
    assert!(n.alpha.is_empty(), "reaped on the aura-remove edge");
    assert!((n.target() - 1.0).abs() < 1e-6);
}

/// One slot edge installs both of the ghost kit's nodes: the dispatcher walks all four proc slots.
#[test]
fn a_kit_with_both_procs_installs_both_nodes() {
    let mut app = app_with_chains();
    let unit = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            ObjectFields::from_pairs(&[(47, GHOST), (95, 0x0E)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();

    let n = app.world().entity(unit).get::<AuraNodes>().unwrap();
    assert_eq!(n.alpha.as_slice(), [(GHOST, 0.5)]);
    assert_eq!(n.head_tint(), Some([0x8C, 0xB9, 0xFD]));
}

/// The newest node holds the head; reaping the older aura leaves it.
#[test]
fn two_auras_stack_and_unstack_through_the_slots() {
    let mut app = app_with_chains();
    let both = ObjectFields::from_pairs(&[(47, STEALTH), (48, GHOST), (95, 0xEE)]);
    let unit = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            both.into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();
    let n = app.world().entity(unit).get::<AuraNodes>().unwrap();
    assert!(
        (n.target() - 0.5).abs() < 1e-6,
        "the ghost node installed last → it is the head term"
    );

    // Stealth alone drops (slot 0 cleared, slot 1 still occupied).
    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        ObjectFields::from_pairs(&[(48, GHOST), (95, 0xE0)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.update();
    let n = app.world().entity(unit).get::<AuraNodes>().unwrap();
    assert_eq!(n.alpha.as_slice(), [(GHOST, 0.5)]);
    assert!((n.target() - 0.5).abs() < 1e-6);
}

#[test]
fn an_unmodelled_proc_only_kit_arms_nothing() {
    const SAP: u32 = 6770;
    const SAP_VISUAL: u32 = 700;
    const SAP_KIT: u32 = 691;

    let mut app = app_with_chains();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            SAP_VISUAL,
            VisualStages {
                state: SAP_KIT,
                ..Default::default()
            },
        )]),
        // Kit 691's shipped content: proc 7 at 2.0 and nothing else.
        HashMap::from([(SAP_KIT, proc_kit(&[proc(7, 2.0)]))]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(HashMap::from([(
            SAP,
            benilla_formats::SpellDisplay {
                visual: SAP_VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });

    let unit = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            ObjectFields::from_pairs(&[(47, SAP), (95, 0x0E)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();
    assert!(
        app.world().entity(unit).get::<AuraNodes>().is_none(),
        "an unmodelled proc invents no body change"
    );
}

// ── The render authoring ─────────────────────────────────────────────────────────────────────────

/// The author writes the alpha down the whole tree (the reference's alpha is per CGUnit) and moves
/// each part to its blend twin, as a cutout batch ignores alpha; at opaque it releases.
#[test]
fn the_author_owns_the_tree_then_releases_at_opaque() {
    use benilla_world::model_fade::FadeMaterials;
    use bevy::mesh::MeshTag;

    let cutout: Handle<benilla_assets::materials::WowModelMaterial> =
        bevy::asset::uuid_handle!("c0000000-0000-4000-8000-00000000a114");
    let blend: Handle<benilla_assets::materials::WowModelMaterial> =
        bevy::asset::uuid_handle!("b1000000-0000-4000-8000-00000000a114");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.init_resource::<benilla_world::interior::InteriorReauthor>();
    app.init_resource::<benilla_world::model_render::FarSideTwins>();
    app.add_systems(Update, apply_aura_alpha);

    let fm = FadeMaterials {
        cutout: cutout.clone(),
        blend: blend.clone(),
        bake_blend: None,
        zfill: None,
    };
    // root → body part, and root → joint → held weapon (the depth the walk must reach).
    let mut nodes = AuraNodes::new(1.0);
    install(&mut nodes, STEALTH, &[AuraNode::Alpha(0.3)], 0.0);
    let root = app.world_mut().spawn(nodes).id();
    let body = app
        .world_mut()
        .spawn((
            fm.clone(),
            MeshTag(0),
            MeshMaterial3d(cutout.clone()),
            ChildOf(root),
        ))
        .id();
    let joint = app.world_mut().spawn(ChildOf(root)).id();
    let weapon = app
        .world_mut()
        .spawn((
            fm.clone(),
            MeshTag(0),
            MeshMaterial3d(cutout.clone()),
            ChildOf(joint),
        ))
        .id();

    // `MinimalPlugins`' clock barely advances per update, so the ramp is ended on its own clock.
    app.update();
    {
        let mut root_mut = app.world_mut().entity_mut(root);
        let mut n = root_mut.get_mut::<AuraNodes>().unwrap();
        n.started = -10.0; // the ramp finished 10 s ago
    }
    app.update();

    for part in [body, weapon] {
        let tag = app.world().entity(part).get::<MeshTag>().unwrap().0;
        assert!(
            (benilla_world::mesh_tag::alpha_of(tag) - 0.3).abs() < 1.0 / 63.0,
            "part {part} carries the aura alpha (tag {tag:#x})"
        );
        assert_eq!(
            app.world()
                .entity(part)
                .get::<MeshMaterial3d<benilla_assets::materials::WowModelMaterial>>()
                .unwrap()
                .0,
            blend,
            "part {part} draws on the blend twin while translucent"
        );
    }

    // The aura drops and the ramp completes: the release frame restores opacity and material.
    {
        let mut root_mut = app.world_mut().entity_mut(root);
        let mut n = root_mut.get_mut::<AuraNodes>().unwrap();
        n.alpha.clear();
        n.retarget(0.0);
        n.started = -10.0;
    }
    app.update();
    for part in [body, weapon] {
        let tag = app.world().entity(part).get::<MeshTag>().unwrap().0;
        assert!((benilla_world::mesh_tag::alpha_of(tag) - 1.0).abs() < 1.0 / 63.0);
        assert_eq!(
            app.world()
                .entity(part)
                .get::<MeshMaterial3d<benilla_assets::materials::WowModelMaterial>>()
                .unwrap()
                .0,
            cutout,
            "released back to the part's steady law"
        );
    }
    assert!(
        !app.world()
            .entity(root)
            .get::<AuraNodes>()
            .unwrap()
            .authoring,
        "and stops claiming the channel"
    );
}

// ── The tint's render channel ─────────────────────────────────────────────────────

/// The ghost's tint lands at the unit's rig slot packed as `param | 0xff000000`, and clears with
/// the aura.
#[test]
fn the_ghost_tint_reaches_the_instance_table_and_clears_with_the_aura() {
    let mut app = app_with_tint_channel();
    let skin = benilla_world::rig_palette::RigSkin::allocate_bones(
        app.world_mut()
            .resource_mut::<benilla_world::rig_palette::RigPalettes>()
            .as_mut(),
        8,
        Handle::default(),
    )
    .expect("a fresh palette has room");
    let slot = skin.slot;
    assert!(slot >= 1, "slot 0 is the world's no-rig sentinel");
    let unit = app
        .world_mut()
        .spawn((
            crate::net::ObjectStore(
                ObjectFields::from_pairs(&[(47, GHOST), (95, 0x0E)])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
            ),
            skin,
        ))
        .id();

    app.update();
    let tints = app
        .world()
        .resource::<benilla_world::instance_tint::InstanceTints>();
    assert_eq!(
        tints.get(slot),
        0xff8c_b9fd,
        "the ghost's pale blue-white, alpha byte and all"
    );

    // Dropped with no ease (`0x5ff320`); identity is the shader's no-op word, not a colour.
    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        ObjectFields::from_pairs(&[(95, 0x00)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.update();
    let tints = app
        .world()
        .resource::<benilla_world::instance_tint::InstanceTints>();
    assert_eq!(tints.get(slot), benilla_world::instance_tint::IDENTITY);
}

/// A rigged attached model has its own instance slot, which the vertex stage indexes the palette
/// by, so its tint comes up the `ParentModel` chain, as the reference composes it (`0x714000`).
#[test]
fn a_rigged_attachment_inherits_its_wearers_tint_through_the_model_chain() {
    let mut app = app_with_tint_channel();
    let (body, shoulder) = {
        let mut palettes = app
            .world_mut()
            .resource_mut::<benilla_world::rig_palette::RigPalettes>();
        let mut alloc = |bones| {
            benilla_world::rig_palette::RigSkin::allocate_bones(
                &mut palettes,
                bones,
                Handle::default(),
            )
            .expect("a fresh palette has room")
        };
        (alloc(8), alloc(3))
    };
    let (body_slot, shoulder_slot) = (body.slot, shoulder.slot);
    assert_ne!(body_slot, shoulder_slot, "two rigs, two slots");
    let unit = app
        .world_mut()
        .spawn((
            crate::net::ObjectStore(
                ObjectFields::from_pairs(&[(47, GHOST), (95, 0x0E)])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
            ),
            body,
        ))
        .id();
    // The item root: its own rig, no aura nodes of its own, chained to the unit wearing it.
    app.world_mut()
        .spawn((shoulder, benilla_world::model_fade::ParentModel(unit)));

    app.update();
    let tints = app
        .world()
        .resource::<benilla_world::instance_tint::InstanceTints>();
    assert_eq!(tints.get(body_slot), 0xff8c_b9fd, "the body is ghost-blue");
    assert_eq!(
        tints.get(shoulder_slot),
        0xff8c_b9fd,
        "and so is the pauldron riding it"
    );

    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        ObjectFields::from_pairs(&[(95, 0x00)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.update();
    let tints = app
        .world()
        .resource::<benilla_world::instance_tint::InstanceTints>();
    assert_eq!(tints.get(body_slot), benilla_world::instance_tint::IDENTITY);
    assert_eq!(
        tints.get(shoulder_slot),
        benilla_world::instance_tint::IDENTITY
    );
}

#[test]
fn an_unchained_rig_inherits_nothing() {
    let mut app = app_with_tint_channel();
    let (body, loose) = {
        let mut palettes = app
            .world_mut()
            .resource_mut::<benilla_world::rig_palette::RigPalettes>();
        let mut alloc = |bones| {
            benilla_world::rig_palette::RigSkin::allocate_bones(
                &mut palettes,
                bones,
                Handle::default(),
            )
            .expect("a fresh palette has room")
        };
        (alloc(8), alloc(3))
    };
    let loose_slot = loose.slot;
    app.world_mut().spawn((
        crate::net::ObjectStore(
            ObjectFields::from_pairs(&[(47, GHOST), (95, 0x0E)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        ),
        body,
    ));
    app.world_mut().spawn(loose);
    app.update();
    assert_eq!(
        app.world()
            .resource::<benilla_world::instance_tint::InstanceTints>()
            .get(loose_slot),
        benilla_world::instance_tint::IDENTITY,
        "a tinted unit standing next to it changes nothing"
    );
}

/// `RigSkin`'s free hook clears the tint on a despawn or a component swap, before any reuse.
#[test]
fn the_rig_free_hook_clears_a_dead_units_tint() {
    let mut app = app_with_tint_channel();
    let skin = benilla_world::rig_palette::RigSkin::allocate_bones(
        app.world_mut()
            .resource_mut::<benilla_world::rig_palette::RigPalettes>()
            .as_mut(),
        8,
        Handle::default(),
    )
    .expect("a fresh palette has room");
    let slot = skin.slot;
    let unit = app
        .world_mut()
        .spawn((
            crate::net::ObjectStore(
                ObjectFields::from_pairs(&[(47, GHOST), (95, 0x0E)])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
            ),
            skin,
        ))
        .id();
    app.update();
    assert_eq!(
        app.world()
            .resource::<benilla_world::instance_tint::InstanceTints>()
            .get(slot),
        0xff8c_b9fd,
    );

    // Despawn with no aura edge: the unit streamed out mid-buff.
    app.world_mut().entity_mut(unit).despawn();
    assert_eq!(
        app.world()
            .resource::<benilla_world::instance_tint::InstanceTints>()
            .get(slot),
        benilla_world::instance_tint::IDENTITY,
        "the slot is clean before anyone can reallocate it",
    );
}

// ── The freeze (proc 11) ─────────────────────────────────────────────────────────────────────────

fn app_with_freeze() -> App {
    let mut app = app_with_chains();
    app.add_systems(Update, apply_aura_anim_rate.after(drain_aura_procs));
    app
}

fn rig_playing(speeds: &[(u32, f32)]) -> AnimationPlayer {
    let mut player = AnimationPlayer::default();
    for (node, speed) in speeds {
        player
            .play(AnimationNodeIndex::new(*node as usize))
            .set_speed(*speed);
    }
    player
}

/// `(paused, speed)` of one armed clip: the freeze holds the clock and leaves the speed.
fn clip(app: &App, rig: Entity, node: u32) -> (bool, f32) {
    let a = app
        .world()
        .entity(rig)
        .get::<AnimationPlayer>()
        .expect("the rig kept its player")
        .animation(AnimationNodeIndex::new(node as usize))
        .expect("the clip is still armed");
    (a.is_paused(), a.speed())
}

/// Ice Block's proc 11 at rate 0 holds the gait and the cast one-shot; the aura's end frees them.
#[test]
fn ice_block_holds_the_clocks_and_the_drop_lets_them_go() {
    let mut app = app_with_freeze();
    let unit = app
        .world_mut()
        .spawn((
            crate::net::ObjectStore(
                ObjectFields::from_pairs(&[(47, ICE_BLOCK), (95, 0x0E)])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
            ),
            rig_playing(&[(4, 1.35), (54, 1.0)]),
        ))
        .id();
    app.update();

    let n = app.world().entity(unit).get::<AuraNodes>().unwrap();
    assert_eq!(n.head_anim_rate(), Some(0.0), "kit 3709's proc 11 is 0");
    // 9074175 is 0x8A75FF, the same kit's ice-blue tint.
    assert_eq!(n.head_tint(), Some([0x8A, 0x75, 0xFF]), "kit 3709's proc 1");
    assert!(clip(&app, unit, 4).0, "the gait stops mid-stride");
    assert!(clip(&app, unit, 54).0, "so does the cast one-shot");
    // The speeds are untouched, so `transplant_up`, copying a clip's rate, never inherits a freeze.
    assert_eq!(clip(&app, unit, 4).1, 1.35);
    assert_eq!(clip(&app, unit, 54).1, 1.0);

    app.update(); // idempotent

    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        ObjectFields::from_pairs(&[(95, 0)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.update();
    assert!(app
        .world()
        .entity(unit)
        .get::<AuraNodes>()
        .unwrap()
        .rate
        .is_empty());
    assert!(!clip(&app, unit, 4).0, "running again");
    assert!(!clip(&app, unit, 54).0);
    assert_eq!(clip(&app, unit, 4).1, 1.35, "at its own rate");
    assert!(
        app.world().entity(unit).get::<AnimRateFreeze>().is_none(),
        "the release drops the marker with it"
    );
}

/// A one-shot moved to the torso overlay under the freeze copies its source's speed, which the
/// pause leaves real, so the thaw runs it out; a copied 0 would never finish or free the overlay.
#[test]
fn a_clip_armed_under_the_freeze_comes_back_at_its_own_speed() {
    let mut app = app_with_freeze();
    let unit = app
        .world_mut()
        .spawn((
            crate::net::ObjectStore(
                ObjectFields::from_pairs(&[(47, ICE_BLOCK), (95, 0x0E)])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
            ),
            rig_playing(&[(4, 1.35), (54, 1.0)]),
        ))
        .id();
    app.update();

    // `transplant_up`'s own shape: read the frozen source clip, arm the torso twin from it.
    let speed = clip(&app, unit, 54).1;
    assert_eq!(speed, 1.0, "the source's speed is not the freeze");
    let seek = app
        .world()
        .entity(unit)
        .get::<AnimationPlayer>()
        .unwrap()
        .animation(AnimationNodeIndex::new(54))
        .unwrap()
        .seek_time();
    {
        let mut ent = app.world_mut().entity_mut(unit);
        let mut player = ent.get_mut::<AnimationPlayer>().unwrap();
        let upper = player.play(AnimationNodeIndex::new(156));
        upper.seek_to(seek);
        upper.set_speed(speed);
    }
    app.update();
    assert!(clip(&app, unit, 156).0, "held on arrival");

    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        ObjectFields::from_pairs(&[(95, 0)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.update();
    assert!(!clip(&app, unit, 156).0);
    assert_eq!(
        clip(&app, unit, 156).1,
        1.0,
        "the overlay runs out and releases instead of welding a cast pose on"
    );
}

/// `0x6201d0` writes the mount `[unit+0xdc]` before the body `[unit+0xd8]`: the mount freezes too.
#[test]
fn the_freeze_reaches_the_mount_body() {
    let mut app = app_with_freeze();
    let mount = app
        .world_mut()
        .spawn(rig_playing(&[(5, 2.0), (91, 1.0)]))
        .id();
    let unit = app
        .world_mut()
        .spawn((
            crate::net::ObjectStore(
                ObjectFields::from_pairs(&[(47, ICE_BLOCK), (95, 0x0E)])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
            ),
            rig_playing(&[(91, 1.0), (54, 1.0)]),
            crate::entities::mount::MountChild(mount),
        ))
        .id();
    app.update();

    assert!(clip(&app, unit, 91).0, "the rider");
    assert!(clip(&app, mount, 5).0, "and the mount under them");

    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        ObjectFields::from_pairs(&[(95, 0)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.update();
    assert!(!clip(&app, mount, 5).0);
    assert_eq!(clip(&app, mount, 5).1, 2.0);
}

/// The freeze is proc 11's alone, never any aura's and never the stun flag's (`0x40000`).
#[test]
fn an_aura_without_the_proc_never_touches_a_clock() {
    let mut app = app_with_freeze();
    let unit = app
        .world_mut()
        .spawn((
            crate::net::ObjectStore(
                ObjectFields::from_pairs(&[(47, STEALTH), (95, 0x0E)])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
            ),
            rig_playing(&[(4, 1.35), (54, 1.0)]),
        ))
        .id();
    app.update();
    assert!(!clip(&app, unit, 4).0);
    assert!(app.world().entity(unit).get::<AnimRateFreeze>().is_none());
}

/// The one shipped non-zero rate, `8947848.0`, installs a node and holds no clock
/// ([`apply_aura_anim_rate`]).
#[test]
fn a_non_zero_rate_installs_a_node_and_holds_nothing() {
    let mut n = AuraNodes::new(1.0);
    install(&mut n, 17624, &[AuraNode::AnimRate(8_947_848.0)], 0.0);
    assert_eq!(n.head_anim_rate(), Some(8_947_848.0));

    let mut app = app_with_freeze();
    let unit = app.world_mut().spawn(rig_playing(&[(4, 1.35)])).id();
    app.world_mut().entity_mut(unit).insert(n);
    app.update();
    assert!(!clip(&app, unit, 4).0);
    assert_eq!(clip(&app, unit, 4).1, 1.35);
}
