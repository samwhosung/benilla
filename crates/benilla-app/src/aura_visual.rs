//! The aura-state CharProc layer: what an aura does to its unit's own body (translucency, tint,
//! animation clock) while it lives; the kit's attach-point models are
//! [`arm_aura_state_fx`](crate::creature_anim::arm_aura_state_fx)'s.
//!
//! In the reference an aura slot change (`0x604d00` → `0x6123f0`) reaches `0x5ff350`, which plays
//! the spell's state kit (`SpellVisual` field 4) at stage 2, and the kit's tail (`0x60f35c`) runs
//! the CharProc dispatcher `0x60d7c0` over its four proc slots (`SpellVisualKit` fields 15-34).
//! Three procs act on the body: 14 installs an alpha node (`0x60d972`), 1 a tint node
//! (`0x60d840`), 11 an animation rate (`0x60db7e`). A node is keyed by spell id and linked at the
//! head of its list, only the head counts, and the aura's removal drops its spell's nodes
//! (`0x5ff290`).
//!
//! `UNIT_FIELD_BYTES_1` byte 3's stealth and ghost bits drive no body render in the reference: its
//! readers (`0x5ff80d`, `0x607101`, `0x60f62e`) only suppress nameplates and markers. The body
//! keys on the aura, never on the flag.
//!
//! Proc types 2, 7 and 13 also appear on state kits; their mechanism is untraced, so [`node_for`]
//! ignores them.

use benilla_protocol::EntityKind;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_world::interior::InteriorLit;
use benilla_world::model_fade::{fade_alpha, FadeMaterials, PendingAppearFade, RenderFade};

/// The aura-alpha ramp, `StartAlphaFade(target, 1000 ms)` (`0x614f80`) eased `clamp01(t)³` by
/// [`fade_alpha`] (`0x614a90`). A code constant, not a kit column: the ghost kit's proc 14 carries
/// no `1000.0` yet ramps over the same second.
pub(crate) const AURA_ALPHA_FADE_SECS: f32 = 1.0;

/// Under half a step of the tag's 6-bit alpha field (1/64): a ramp this close has arrived.
const ALPHA_EPS: f32 = 1.0 / 128.0;

/// One CharProc node a kit installs on a unit, keyed by spell id as the reference keys `node+0x18`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AuraNode {
    /// Proc 14: the alpha factor (`node+0x78`).
    Alpha(f32),
    /// Proc 1: the body tint, unpacked from the param's `0x00RRGGBB`.
    Tint([u8; 3]),
    /// Proc 11: the playback rate of the unit's own clocks, `0.0` for the whole freeze family.
    AnimRate(f32),
}

/// One unit's CharProc node lists (the reference's `unit+0xb50` alpha and `unit+0xce0` tint lists)
/// and its alpha ramp. On the net entity root: the reference keeps one per CGUnit, and every
/// attached model renders off its owner's.
#[derive(Component, Debug, Default)]
pub(crate) struct AuraNodes {
    /// Proc-14 `(spell id, factor)` nodes, newest first, as the reference links at the head.
    alpha: Vec<(u32, f32)>,
    /// Proc-1 `(spell id, rgb)` nodes, newest first.
    tint: Vec<(u32, [u8; 3])>,
    /// Proc-11 `(spell id, rate)` nodes, newest first; the head drives the unit's clocks.
    rate: Vec<(u32, f32)>,
    /// `baseAlpha`, the display row's `CreatureModelAlpha / 255` for every unit, players included
    /// (`0x60d2d0`); kept current by [`refresh_base_alpha`].
    base: f32,
    /// The ramp's live value, which the parts render at.
    current: f32,
    /// The ramp, `from` to `to` over [`AURA_ALPHA_FADE_SECS`] from `started` (elapsed seconds).
    from: f32,
    to: f32,
    started: f32,
    /// Whether [`apply_aura_alpha`] owns this unit's parts: while translucent, plus the one frame
    /// at opaque that writes the settled alpha and hands the material back to its light law.
    authoring: bool,
}

impl AuraNodes {
    fn new(base: f32) -> Self {
        Self {
            base,
            current: base,
            from: base,
            to: base,
            ..Default::default()
        }
    }

    /// `0x60d180`: `baseAlpha` times the head alpha node's factor alone, never a product over the
    /// list; an empty list leaves `baseAlpha` (`0x60d195`).
    fn target(&self) -> f32 {
        let head = self.alpha.first().map_or(1.0, |(_, f)| *f);
        (self.base * head).clamp(0.0, 1.0)
    }

    /// Aim the ramp at the target from where it is now; an unmoved target leaves it alone, so an
    /// aura refresh does not restart the ease.
    fn retarget(&mut self, now: f32) {
        let target = self.target();
        if (target - self.to).abs() <= f32::EPSILON {
            return;
        }
        self.from = self.current;
        self.to = target;
        self.started = now;
    }

    fn tick(&mut self, now: f32) -> f32 {
        let t = (now - self.started) / AURA_ALPHA_FADE_SECS;
        self.current = fade_alpha(self.from, self.to, t);
        self.current
    }

    fn translucent(&self) -> bool {
        self.current < 1.0 - ALPHA_EPS
    }

    /// The head tint node's RGB, which the reference's per-frame apply reads (`0x60cbd9`).
    pub(crate) fn head_tint(&self) -> Option<[u8; 3]> {
        self.tint.first().map(|(_, rgb)| *rgb)
    }

    /// The head proc-11 node's rate, the one the unit's clocks run at.
    pub(crate) fn head_anim_rate(&self) -> Option<f32> {
        self.rate.first().map(|(_, r)| *r)
    }

    /// A node set holding one proc-11 node, as a freeze aura leaves it, without the slot watcher.
    #[cfg(test)]
    pub(crate) fn with_rate_node_for_tests(spell_id: u32, rate: f32) -> Self {
        Self {
            rate: vec![(spell_id, rate)],
            ..Default::default()
        }
    }
}

/// Publish every rig's head tint to [`benilla_world::instance_tint`], keyed on its rig slot. The
/// reference writes the tint as it changes, with none of the alpha's ramp (`unit+0xd04` →
/// `0x710cf0`). Every `RigSkin` is rewritten each frame so a recycled slot never keeps a colour.
pub(crate) fn apply_aura_tint(
    rigs: Query<(Entity, &benilla_world::rig_palette::RigSkin)>,
    chain: Query<(
        Option<&AuraNodes>,
        Option<&benilla_world::model_fade::ParentModel>,
    )>,
    mut tints: ResMut<benilla_world::instance_tint::InstanceTints>,
    mut probe_logged: Local<bool>,
) {
    let probe = tint_probe();
    let mut painted = 0usize;
    for (root, rig) in &rigs {
        let word = probe.or_else(|| chained_tint(&chain, root)).map_or(
            benilla_world::instance_tint::IDENTITY,
            benilla_world::instance_tint::pack,
        );
        tints.set(rig.slot, word);
        painted += 1;
    }
    if probe.is_some() && !*probe_logged {
        *probe_logged = true;
        info!("tint probe: painted {painted} live rig slot(s)");
    }
}

/// The tint a rig renders with: its own head node, else the nearest up its
/// [`benilla_world::model_fade::ParentModel`] chain, as the reference composes an attached model's
/// colours onto its parent's (`0x714000`); only this walk reaches a rigged item's own slot.
fn chained_tint(
    chain: &Query<(
        Option<&AuraNodes>,
        Option<&benilla_world::model_fade::ParentModel>,
    )>,
    instance: Entity,
) -> Option<[u8; 3]> {
    let mut at = instance;
    for _ in 0..benilla_world::model_fade::MAX_MODEL_CHAIN {
        let Ok((nodes, parent)) = chain.get(at) else {
            return None;
        };
        if let Some(rgb) = nodes.and_then(AuraNodes::head_tint) {
            return Some(rgb);
        }
        at = parent?.0;
    }
    None
}

/// `WOW_TINT_PROBE=RRGGBB` paints every live rig slot that colour, auras ignored: the machine check
/// for the GPU half of the tint channel. It logs the slot count once, since an unchanged capture
/// cannot tell "nothing is rigged" from "the channel is broken".
fn tint_probe() -> Option<[u8; 3]> {
    static PROBE: std::sync::OnceLock<Option<[u8; 3]>> = std::sync::OnceLock::new();
    *PROBE.get_or_init(|| {
        let raw = std::env::var("WOW_TINT_PROBE").ok()?;
        let hex = raw.trim().trim_start_matches('#');
        let packed = u32::from_str_radix(hex, 16).ok()?;
        Some([
            (packed >> 16) as u8,
            ((packed >> 8) & 0xff) as u8,
            (packed & 0xff) as u8,
        ])
    })
}

/// Marker: a proc-11 aura holds this rig's clocks. It saves no speeds, unlike the reference's
/// per-bone save and restore (`0x6203e0`): here a rate belongs to a clip, and `transplant_up` arms
/// clips under the freeze with speeds copied from frozen ones, so a saved speed restores as 0.
#[derive(Component, Default, Debug)]
pub(crate) struct AnimRateFreeze;

/// Pause the clocks of every unit whose head proc-11 rate is 0, its own rig and its mount's (the
/// two models `0x6201d0` writes), and resume them when the aura goes; effect models and global
/// sequences keep running. The pause is reasserted every frame, since the reference's rate lives
/// on the bone (`+0xb0`, read by the clock at `0x71458e`) and arming leaves it alone
/// (`0x7121a0`), and after the animation driver, so a cast armed this frame freezes too.
///
/// Deviation: a non-zero rate is ignored, because the only one shipped (`8947848.0` on kit 1744, a
/// packed grey in the rate column) is noise on the reference's integer-ms clock and has no faithful
/// form on our f32-second one.
pub(crate) fn apply_aura_anim_rate(
    units: Query<(
        Entity,
        &AuraNodes,
        Option<&crate::entities::mount::MountChild>,
    )>,
    mut rigs: Query<(&mut AnimationPlayer, Has<AnimRateFreeze>)>,
    mut commands: Commands,
) {
    for (entity, nodes, mount) in &units {
        let frozen = nodes.head_anim_rate() == Some(0.0);
        for rig in [Some(entity), mount.map(|m| m.0)].into_iter().flatten() {
            let Ok((mut player, held)) = rigs.get_mut(rig) else {
                continue;
            };
            match (frozen, held) {
                (true, _) => {
                    for (_, anim) in player.playing_animations_mut() {
                        anim.pause();
                    }
                    if !held {
                        trace_clock_edge("freeze", rig, &player);
                        commands.entity(rig).try_insert(AnimRateFreeze);
                    }
                }
                (false, true) => {
                    // Resume every clip: nothing else pauses an animation on a unit rig.
                    for (_, anim) in player.playing_animations_mut() {
                        anim.resume();
                    }
                    trace_clock_edge("thaw", rig, &player);
                    commands.entity(rig).try_remove::<AnimRateFreeze>();
                }
                (false, false) => {}
            }
        }
    }
}

/// Trace (`WOW_MOVE_TRACE`, tag `aur`) one line per rig at each freeze edge, with each clip's
/// speed: a `@0` on a thaw line is a clip that will never advance again.
fn trace_clock_edge(what: &str, rig: Entity, player: &AnimationPlayer) {
    if !benilla_assets::trace::enabled_for("aur") {
        return;
    }
    let clips: Vec<String> = player
        .playing_animations()
        .map(|(n, a)| format!("{}@{}", n.index(), a.speed()))
        .collect();
    benilla_assets::trace::line(
        "aur",
        &format!("{what} rig={rig} clips=[{}]", clips.join(" ")),
    );
}

/// One aura's CharProc edge, from [`crate::creature_anim::arm_aura_state_fx`]'s slot watch.
#[derive(Message, Clone, Debug)]
pub(crate) enum AuraProc {
    /// The aura landed in a slot: install this kit's body procs, keyed by its spell id.
    Begin {
        entity: Entity,
        spell_id: u32,
        /// The kit's proc nodes, in slot order (the client walks all four and dispatches each).
        nodes: Vec<AuraNode>,
    },
    /// The aura left the slots: drop every node this spell id installed (`0x5ff290`).
    Reap { entity: Entity, spell_id: u32 },
}

/// The node one `SpellVisualKit` CharProc installs (the dispatcher's jump table `0x60dbfc`), or
/// `None` for a proc type whose mechanism is untraced.
pub(crate) fn node_for(proc: benilla_formats::CharProc) -> Option<AuraNode> {
    use benilla_formats::char_proc_type as ty;
    match proc.ty {
        ty::ALPHA => Some(AuraNode::Alpha(proc.params[0])),
        // A float holding an integral `0x00RRGGBB`, rounded (`0x60d8cc`): the ghost's 9222653.0
        // is 0x8CB9FD.
        ty::TINT => {
            let packed = proc.params[0].round().clamp(0.0, 0xff_ffff as f32) as u32;
            Some(AuraNode::Tint([
                (packed >> 16) as u8,
                (packed >> 8) as u8,
                packed as u8,
            ]))
        }
        // A playback rate, passed through: `0x60db7e` hands it to `SetBoneAnimSpeed` (`0x712910`)
        // unexamined.
        ty::ANIM_RATE => Some(AuraNode::AnimRate(proc.params[0])),
        _ => None,
    }
}

/// Apply this frame's [`AuraProc`] edges to the units' node lists.
pub(crate) fn drain_aura_procs(
    mut edges: MessageReader<AuraProc>,
    time: Res<Time>,
    mut commands: Commands,
    mut nodes: Query<&mut AuraNodes>,
    units: Query<&crate::net::NetEntity>,
    creatures: Option<Res<crate::entities::Creatures>>,
) {
    let now = time.elapsed_secs();
    let mut fresh: bevy::ecs::entity::EntityHashMap<AuraNodes> = Default::default();
    for edge in edges.read() {
        match edge {
            AuraProc::Begin {
                entity,
                spell_id,
                nodes: procs,
            } => {
                if procs.is_empty() {
                    continue;
                }
                if let Ok(mut n) = nodes.get_mut(*entity) {
                    install(&mut n, *spell_id, procs, now);
                    trace_edge("arm", *entity, *spell_id, &n);
                } else {
                    // Staged, not inserted: `Commands` apply at the end of the system, so a second
                    // Begin this frame would build a fresh set and overwrite the first.
                    let n = fresh.entry(*entity).or_insert_with(|| {
                        AuraNodes::new(base_alpha(*entity, &units, creatures.as_deref()))
                    });
                    install(n, *spell_id, procs, now);
                    trace_edge("arm", *entity, *spell_id, n);
                }
            }
            AuraProc::Reap { entity, spell_id } => {
                if let Ok(mut n) = nodes.get_mut(*entity) {
                    reap(&mut n, *spell_id, now);
                    trace_edge("reap", *entity, *spell_id, &n);
                } else if let Some(n) = fresh.get_mut(entity) {
                    // Armed and reaped in one frame: the staged set is the live one.
                    reap(n, *spell_id, now);
                    trace_edge("reap", *entity, *spell_id, n);
                }
            }
        }
    }
    for (entity, n) in fresh {
        commands.entity(entity).try_insert(n);
    }
}

/// Drop every node one spell installed (`0x5ff290`) and re-aim the ramp.
fn reap(n: &mut AuraNodes, spell_id: u32, now: f32) {
    n.alpha.retain(|(s, _)| *s != spell_id);
    n.tint.retain(|(s, _)| *s != spell_id);
    n.rate.retain(|(s, _)| *s != spell_id);
    n.retarget(now);
}

/// Install one spell's nodes at the head of each list, replacing any it had: nodes are keyed by
/// spell id, so a second caster's copy of an aura installs one set.
fn install(n: &mut AuraNodes, spell_id: u32, procs: &[AuraNode], now: f32) {
    n.alpha.retain(|(s, _)| *s != spell_id);
    n.tint.retain(|(s, _)| *s != spell_id);
    n.rate.retain(|(s, _)| *s != spell_id);
    for node in procs {
        match *node {
            AuraNode::Alpha(f) => n.alpha.insert(0, (spell_id, f)),
            AuraNode::Tint(rgb) => n.tint.insert(0, (spell_id, rgb)),
            AuraNode::AnimRate(r) => n.rate.insert(0, (spell_id, r)),
        }
    }
    n.retarget(now);
}

/// Trace (`WOW_MOVE_TRACE=<path>`, tag `aur`) each node edge: the spell, its nodes, the target.
fn trace_edge(what: &str, entity: Entity, spell_id: u32, n: &AuraNodes) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    let alphas: Vec<f32> = n.alpha.iter().map(|(_, f)| *f).collect();
    let tint = n.head_tint().map_or_else(String::new, |[r, g, b]| {
        format!(" tint=#{r:02x}{g:02x}{b:02x}")
    });
    let rate = n
        .head_anim_rate()
        .map_or_else(String::new, |r| format!(" animrate={r}"));
    benilla_assets::trace::line(
        "aur",
        &format!(
            "{what} e={entity} spell={spell_id} alpha={alphas:?}{tint}{rate}              base {:.2} cur {:.2} target {:.2}",
            n.base,
            n.current,
            n.target()
        ),
    );
}

/// `baseAlpha` (`0x60d2d0`): the current display row's `CreatureModelAlpha / 255`, players
/// included (Ghost Wolf's display 4613 reads 102); no row reads 1.0 (`0x60d2e4`).
fn base_alpha(
    entity: Entity,
    units: &Query<&crate::net::NetEntity>,
    creatures: Option<&crate::entities::Creatures>,
) -> f32 {
    let Ok(net) = units.get(entity) else {
        return 1.0;
    };
    display_base_alpha(net, creatures)
}

/// The display-row half of [`base_alpha`], for callers already holding the `NetEntity`.
fn display_base_alpha(
    net: &crate::net::NetEntity,
    creatures: Option<&crate::entities::Creatures>,
) -> f32 {
    if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
        return 1.0;
    }
    match (creatures, net.display_id) {
        (Some(c), Some(display)) => c.display_base_alpha(display).unwrap_or(1.0),
        _ => 1.0,
    }
}

/// Re-resolve a unit's base alpha when its display changes and ramp to it over the same 1000 ms,
/// as the reference's DISPLAYID watcher does (`0x604990` → `0x60abe0`, the row cached at
/// `0x60afb0`, → `0x60ad9f` → `0x60d180`): a display below 255 alpha is translucent with no aura.
/// `Changed<NetEntity>` stands for its record-change gate (`0x60ae10`); the base compare ignores a
/// scale-only change.
pub(crate) fn refresh_base_alpha(
    time: Res<Time>,
    creatures: Option<Res<crate::entities::Creatures>>,
    mut commands: Commands,
    mut units: Query<
        (Entity, &crate::net::NetEntity, Option<&mut AuraNodes>),
        Changed<crate::net::NetEntity>,
    >,
) {
    let now = time.elapsed_secs();
    for (entity, net, nodes) in &mut units {
        let base = display_base_alpha(net, creatures.as_deref());
        match nodes {
            Some(mut n) => {
                if (n.base - base).abs() > f32::EPSILON {
                    debug!(
                        "aura_visual: e={entity} display {:?} base alpha {:.2} -> {base:.2} (1 s ramp)",
                        net.display_id, n.base
                    );
                    n.base = base;
                    n.retarget(now);
                }
            }
            // A translucent display gets a node set, created at opaque so it ramps down.
            None if base < 1.0 => {
                debug!(
                    "aura_visual: e={entity} display {:?} authors base alpha {base:.2} (1 s ramp)",
                    net.display_id
                );
                let mut n = AuraNodes::new(1.0);
                n.base = base;
                n.retarget(now);
                commands.entity(entity).try_insert(n);
            }
            None => {}
        }
    }
}

/// The fadeable parts [`apply_aura_alpha`] writes, with every other factor of their alpha.
type AuraParts<'w, 's> = Query<
    'w,
    's,
    (
        &'static FadeMaterials,
        &'static mut MeshTag,
        &'static mut MeshMaterial3d<WowModelMaterial>,
        Option<&'static benilla_world::doodad_anim::MatAnim>,
        Option<&'static InteriorLit>,
        Option<&'static RenderFade>,
        Has<PendingAppearFade>,
        Has<benilla_world::model_render::FarSideOfWater>,
    ),
    // Disjoint from the card query, as both take `&mut MeshTag`; no card carries `FadeMaterials`.
    Without<benilla_world::billboard::BillboardCard>,
>;

/// Drive every unit's aura-alpha ramp and write its parts' render alpha and material;
/// [`AuraNodes::authoring`] latches the release.
///
/// Each part gets the reference's product (`0x614baa` → `0x710cb0`, `0x707680`): the ramped aura
/// alpha × the batch's animated colour alpha × its appear/despawn ramp, recomputed from state so a
/// rerun cannot compound. A cutout or opaque batch ignores instance alpha, so a translucent unit's
/// parts move to their blend material, keyed on the instance alpha alone.
pub(crate) fn apply_aura_alpha(
    time: Res<Time>,
    mut commands: Commands,
    mut roots: Query<(
        Entity,
        &mut AuraNodes,
        Option<&benilla_world::model_fade::ModelFade>,
    )>,
    children_of: Query<&Children>,
    mut parts: AuraParts,
    far_twins: Res<benilla_world::model_render::FarSideTwins>,
    mut cards: Query<(
        &benilla_world::billboard::BillboardCard,
        &mut MeshTag,
        Option<&benilla_world::doodad_anim::MatAnim>,
    )>,
    mut reauthor: ResMut<benilla_world::interior::InteriorReauthor>,
) {
    let now = time.elapsed_secs();
    for (root, mut n, declared) in &mut roots {
        let alpha = n.tick(now);
        // Declared to the engine's composed model alpha (`model_fade::ModelFade`) before the
        // settled-opaque early-out, so the ramp's arrival at 1.0 is declared too.
        if declared.is_none_or(|d| d.0 != alpha) {
            commands
                .entity(root)
                .insert(benilla_world::model_fade::ModelFade(alpha));
        }
        let translucent = n.translucent();
        if !translucent && !n.authoring {
            continue; // settled opaque: not ours, nothing to release
        }
        // The walked set tells the card pass which anchors are this unit's.
        let mut walked = bevy::ecs::entity::EntityHashSet::default();
        author_descendants(
            root,
            alpha,
            now,
            &children_of,
            &mut parts,
            &far_twins,
            &mut reauthor,
            &mut walked,
        );
        author_cards(alpha, &walked, &mut cards);
        // A frame written at opaque was the release; the latch drops after it.
        n.authoring = translucent;
    }
}

/// Fold the unit's aura alpha into its billboard cards, world-root entities that follow an anchor
/// in the model and so escape the descendant walk (a stealthed night elf's eye glow would stay
/// lit). A card is the unit's when its follow anchor is in `walked`.
fn author_cards(
    alpha: f32,
    walked: &bevy::ecs::entity::EntityHashSet,
    cards: &mut Query<(
        &benilla_world::billboard::BillboardCard,
        &mut MeshTag,
        Option<&benilla_world::doodad_anim::MatAnim>,
    )>,
) {
    for (card, mut tag, anim) in cards.iter_mut() {
        if !card.follows().is_some_and(|a| walked.contains(&a)) {
            continue;
        }
        // From the animation's own factor, not the tag, so the card's alpha animation survives and
        // the release lands on the card's own value.
        let authored = anim.map_or(1.0, |a| a.current);
        let bits = benilla_world::mesh_tag::with_alpha(tag.0, authored * alpha);
        if tag.0 != bits {
            tag.0 = bits;
        }
    }
}

/// Write `entity` if it is a fadeable part, then recurse regardless: held items, helms and
/// shoulders hang under joints that carry no [`FadeMaterials`].
fn author_descendants(
    entity: Entity,
    alpha: f32,
    now: f32,
    children_of: &Query<&Children>,
    parts: &mut AuraParts,
    far_twins: &benilla_world::model_render::FarSideTwins,
    reauthor: &mut benilla_world::interior::InteriorReauthor,
    walked: &mut bevy::ecs::entity::EntityHashSet,
) {
    walked.insert(entity);
    if let Ok((fm, mut tag, mut mat, anim, lit, fade, pending, far_side)) = parts.get_mut(entity) {
        // A part awaiting its appear-fade stays invisible until the ramp arms; writing it would
        // flash it at the aura alpha.
        if !pending {
            // Recomputed from state, never read back from the tag, so the write is idempotent.
            let ramp = fade.map_or(1.0, |f| {
                let t = if f.duration > 0.0 {
                    (now - f.started) / f.duration
                } else {
                    1.0
                };
                fade_alpha(f.from, f.to, t)
            });
            let combined = alpha * anim.map_or(1.0, |a| a.current) * ramp;
            let bits = benilla_world::mesh_tag::with_alpha(tag.0, combined);
            if tag.0 != bits {
                tag.0 = bits;
            }
            // The instance alpha alone picks the pass; the far-side-of-water axis composes on top.
            let want = benilla_world::model_render::far_resolved(
                fm.material_for(lit, alpha < 1.0),
                far_side,
                far_twins,
            )
            .clone();
            if mat.0 != want {
                mat.0 = want;
                if alpha >= 1.0 && lit.is_some() {
                    // The released part's probe slot and fog bit are the interior classifier's.
                    reauthor.0.push(entity);
                }
            }
        }
    }
    if let Ok(kids) = children_of.get(entity) {
        for kid in kids {
            author_descendants(
                *kid,
                alpha,
                now,
                children_of,
                parts,
                far_twins,
                reauthor,
                walked,
            );
        }
    }
}

/// A unit's live aura alpha, for the self-avatar feather, which composes it into its own product.
pub(crate) fn root_alpha(nodes: Option<&AuraNodes>) -> f32 {
    nodes.map_or(1.0, |n| n.current)
}

#[cfg(test)]
mod tests;
