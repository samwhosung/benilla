//! **The aura-state CharProc layer** — what an aura does to the *body itself*: its translucency, its
//! tint and its animation clock, for exactly as long as the aura lives. This is the half of a stage-2
//! "state kit" that
//! isn't an attach-point emitter (those are `creature_anim::spell_visual`'s
//! [`arm_aura_state_fx`](crate::creature_anim::arm_aura_state_fx)): where that
//! watcher hangs *models* on the unit, this one changes how the unit's own model **renders**.
//!
//! B114 ("Stealth shows nothing on the character") was this whole layer missing. Stealth's state
//! kit carries no effect models and no anim at all — its entire visual is one CharProc — so the
//! effects-only watcher resolved the kit, found an empty slot list, and dropped it on the floor.
//!
//! ## The mechanism
//!
//! `UNIT_FIELD_AURA` slot change → the aura watcher `0x604d00` → `0x6123f0` → **`0x5ff350`
//! PlayAuraStateVisual**: the slot's spell → `Spell.dbc` → `SpellVisual` → **field 4 = the state
//! kit** → `PlaySpellVisualKit(kit, stage 2)`, whose tail (`0x60f358: push esi(kit);
//! 0x60f35c: call 0x60d7c0`) runs the **CharProc dispatcher** over the kit's four proc slots
//! (`SpellVisualKit` fields 15–34 — `benilla_formats::CharProc`). Two of the dispatcher's nine cases
//! act on the body:
//!
//! - **proc 14 = translucency** (`0x60d972`): the param becomes a node keyed by spell id
//!   (`node+0x18`, value at `node+0x78`), linked at the HEAD of the unit's list `unit+0xb50`. The
//!   unit's effective alpha is then recomputed by `0x60d180` as **`baseAlpha × the head node's
//!   factor`** — at most one node term, never a product over the chain — `baseAlpha` from the
//!   vtbl+0x6c getter `0x60d2d0` (`CreatureDisplayInfo+0x14 × 1/255`, the SAME slot in the unit
//!   and player vtables: a player's 1.0 is the data talking, not a type fork) — and handed
//!   to **`0x614f80` StartAlphaFade(target, 1000 ms)**, the same ramp block (`+0xec..0x100`) and
//!   the same `clamp01(t)³` ease (`0x614a90`) our appear-fade already rides
//!   ([`benilla_world::model_fade::fade_alpha`]). Per frame `model+0x180 = master × fade`
//!   (`0x614baa → 0x710cb0`), which multiplies into the per-batch alpha (`0x707680`).
//!   The recompute has a second, aura-free caller: the DISPLAYID watcher's refresh (`0x60abe0 →
//!   0x60ad9f`), which is how a display whose row says `CreatureModelAlpha < 255` — Ghost Wolf's
//!   4613 = 102 — renders translucent by itself; [`refresh_base_alpha`] is that leg.
//! - **proc 1 = tint** (`0x60d840`): `round(param)` is a packed `0x00RRGGBB` OR'd with `0xff000000`
//!   into a tint node (list `unit+0xce0`); per frame the head node goes `×1/255` into
//!   `model+0x184/188/18c`.
//!
//! Removal is the mirror image: the aura leaves the slot → `0x5ff290` drops that spell's nodes and
//! recomputes, so the alpha **ramps back** to the new target over the same 1000 ms.
//!
//! The vis-flag is a decoy and must not be used here: `UNIT_FIELD_BYTES_1` byte 3's CREEP (stealth)
//! and GHOST bits drive **no** body render at all — an exhaustive census of every `+0x213` reference
//! found three sites, all nameplate/marker suppression (`0x5ff80d`, `0x607101`, `0x60f62e`). The
//! two co-travel only because the server sets the flag and the aura from the same spell. Keying
//! the body off the flag would be the wrong mechanism that happens to look right on stealth and
//! then fails on every other member of the family.
//!
//! ## The shipped data (read from the real 5875 `SpellVisualKit.dbc` this session —
//! `benilla-extract charprocs` censuses it)
//!
//! 185 of 1772 kits carry a CharProc, and the **state** stage is far and away their home: 103 state
//! kits, ~1200 (spell, proc) pairs. Proc 14 covers **11 kits / 135 spells** — kit 312 `0.3` is the
//! whole sneak family (Stealth 1784-1787, Prowl, Vanish, Shadowmeld, Hide, Sneak, Disguise), kit
//! 3450 `0.5` is Invisibility/Fade, kit 989 `0.5` the ghost aura, 3129 `0.65` Shadowform, 1286 `0.3`
//! Banish, 3989 `1.0` Possess, 5129 `0.0` a quest fade-out. Proc 1 covers 73 state kits (~800
//! spells): Frostbolt's chill blue, Immolate's ember orange, the poison green.
//!
//! ## What is built here, and what is not
//!
//! **Proc 14 ships whole** — nodes, the `baseAlpha × head node` target, the 1000 ms cubic ramp both ways,
//! and the render composition below.
//!
//! **Proc 1 ships too**: its nodes live here ([`AuraNodes::tint`], head-node-wins as
//! the reference's `unit+0xce0` list is) and [`apply_aura_tint`] publishes the head to
//! [`benilla_world::instance_tint`], the per-instance modulate channel keyed on the part's rig slot. Written
//! on change, never eased — the reference's own asymmetry (`unit+0xd04` change-detection straight to
//! `0x710cf0`, with none of the alpha's 1000 ms ramp).
//!
//! 0806 recorded this as blocked on a render-architecture call between per-unit material clones and a
//! tag-indexed palette. That framing was wrong on its own terms: the `MeshTag` rig field already IS a
//! per-instance slot into the shared light buffer, so the tint needed **no** new tag bits (which was
//! the whole stated blocker — the payload has 5 spare bits outdoors and zero indoors) and no material
//! churn. What it did need was reading our own renderer properly.
//!
//! The tint reaches a unit **whole** — the reference's `0x714000` recursion, where an attached model
//! composes onto its parent CM2's computed colours — by two routes, because benilla has two kinds of
//! attached model:
//!
//! - the ordinary one **borrows its wearer's slot**: an item's parts, cards and boneless geosets all
//!   spawn carrying the unit's instance bits, so they read the unit's table entry;
//! - a **rigged** one carries its OWN slot, because the vertex stage indexes the skin palette with
//!   that same field and cannot borrow it (a spell-effect instance; since 0841, the seven shoulder
//!   models that weld geometry to a billboard bone). [`chained_tint`] walks such an instance's
//!   [`benilla_world::model_fade::ParentModel`] link up to the unit — the recursion itself, and the same walk
//!   `ModelAlphas` already does for alpha.
//!
//! **Proc 11 ships too**: the **freeze**. Its param is a playback *rate*
//! written onto the unit's own animation clocks (`0x60db7e` → `SetBoneAnimSpeed 0x712910` on the
//! mount's bone 0, the body's key-bone 4 — the upper-body split — and the body's bone 0), old rates
//! saved in the node and written back when it expires (`0x6203e0`). Rate 0 is Ice Block: the pose
//! holds exactly where it was, *including* a cast one-shot that had just been armed, which is why the
//! caster never gets to raise their hand. [`apply_aura_anim_rate`] is that leg — and it expresses
//! rate 0 as a **pause**, not as a speed, because the reference's rate belongs to a bone while a speed
//! here belongs to a clip that other code copies from ([`AnimRateFreeze`] carries the story).
//!
//! Types 2, 7 and 13 also appear on state kits (Berserk/Bloodlust's 2, the Sap/Feign-Death 7, the
//! "Glowy (Red)" 13) and have **no verified mechanism** — they fall through [`node_for`]'s single `_`
//! arm, which is where each lands as one match arm the day its mechanism is known.

use benilla_protocol::EntityKind;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_world::interior::InteriorLit;
use benilla_world::model_fade::{fade_alpha, FadeMaterials, PendingAppearFade, RenderFade};

/// The reference's aura-alpha ramp: `StartAlphaFade(target, **1000 ms**)` (`0x614f80` from
/// `0x60d180`'s recompute), eased `clamp01(t)³` by `0x614a90` — [`fade_alpha`] is that curve.
///
/// It is a **code constant, not a kit column**: proc 14's `params[1..]` are `[2.0, 1000.0, 0.0]` on
/// the sneak kit but `[0.0, 0.0, 0.0]` on the ghost's, and the ghost's ramp is byte-cited at the same
/// 1000 ms — so the sneak kit's literal `1000.0` cannot be the source (a data-sourced duration would
/// snap the ghost instantly). Across the whole shipped table proc 14's `params[2]` is only ever
/// `0.0`, `2.0` or `1000.0`, which is no duration scale in any unit. Read from `params[0]` only.
pub(crate) const AURA_ALPHA_FADE_SECS: f32 = 1.0;

/// Settled-ramp epsilon — under half a step of the tag's 6-bit alpha field (1/64), so a ramp that
/// has visually arrived is treated as arrived.
const ALPHA_EPS: f32 = 1.0 / 128.0;

/// One CharProc node the dispatcher installed on a unit, keyed by the aura's spell id exactly as the
/// reference keys `node+0x18`. Held in [`AuraNodes`]; a `Reap` for that spell id removes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AuraNode {
    /// Proc 14: this aura's alpha factor (`node+0x78`) — the target's node term while it is the head.
    Alpha(f32),
    /// Proc 1: this aura's body tint, unpacked from the param's `0x00RRGGBB`. Modelled, not yet
    /// rendered (module docs).
    Tint([u8; 3]),
    /// Proc 11: this aura's animation playback **rate** for the unit's own model — `0.0` for every
    /// member of the freeze family (Ice Block, Freeze, Petrify, Entangle, Stilled).
    AnimRate(f32),
}

/// The CharProc node lists of one unit — the ECS twin of the reference's per-unit `unit+0xb50`
/// (alpha) and `unit+0xce0` (tint) lists, plus the ramp `0x614f80` drives from their heads.
///
/// Lives on the **net entity root**, like [`benilla_world::entity_shade::GroundShade`]: the reference has one
/// of these per CGUnit and every attached model (held item, helm, shoulder) renders off the owner's,
/// which is also why [`apply_aura_alpha`] walks the root's whole descendant tree rather than its
/// direct children.
#[derive(Component, Debug, Default)]
pub(crate) struct AuraNodes {
    /// `(spell id, factor)` — proc-14 nodes, newest at the front (the reference links a fresh node
    /// at the list head). The HEAD times [`Self::base`] is the target — never a product.
    alpha: Vec<(u32, f32)>,
    /// `(spell id, rgb)` — proc-1 nodes, newest at the front like the alpha list; the head is what
    /// the per-frame apply reads.
    tint: Vec<(u32, [u8; 3])>,
    /// `(spell id, rate)` — proc-11 nodes, head-first like the other two. The head is the rate the
    /// unit's animation clocks run at ([`apply_aura_anim_rate`]); empty = the clocks are the
    /// driver's own again.
    rate: Vec<(u32, f32)>,
    /// `baseAlpha`: the display row's `CreatureModelAlpha / 255` — for EVERY unit, players
    /// included; the getter `0x60d2d0` has no type fork, a normal character's row just says 255.
    /// Owned by [`refresh_base_alpha`], which re-resolves it on every display-id change (a
    /// shapeshift swaps it live) and ramps to the new value.
    base: f32,
    /// Where the ramp is now — what the parts actually render at.
    current: f32,
    /// The ramp: `from → to` over [`AURA_ALPHA_FADE_SECS`] from `started` (`Time::elapsed_secs`).
    from: f32,
    to: f32,
    started: f32,
    /// Whether [`apply_aura_alpha`] still owns this unit's parts. Set while the unit is translucent
    /// and for the one frame the ramp latches back at opaque — that frame performs the **release**
    /// (writes the settled alpha and hands the material back to the part's light law), the same
    /// hand-back protocol the self-avatar feather uses. Without it, a unit whose
    /// stealth drops stays latched on the blend twin at its last low alpha until something else
    /// happens to rewrite the channel.
    authoring: bool,
}

impl AuraNodes {
    /// A fresh node set for a unit whose `baseAlpha` is `base`, ramp settled at `base`.
    fn new(base: f32) -> Self {
        Self {
            base,
            current: base,
            from: base,
            to: base,
            ..Default::default()
        }
    }

    /// The reference's `0x60d180`: `baseAlpha × the HEAD alpha node's factor` — at most ONE node
    /// term, skipped entirely when the list is empty (`0x60d195 je`), never a product over the
    /// chain. Nodes are linked at the head as they install, so the newest one is the term that
    /// counts.
    fn target(&self) -> f32 {
        let head = self.alpha.first().map_or(1.0, |(_, f)| *f);
        (self.base * head).clamp(0.0, 1.0)
    }

    /// Point the ramp at the current target, from wherever it is now. A no-op when the target
    /// hasn't moved, so an aura refresh (or a node whose factor equals the live one) doesn't restart
    /// the ease.
    fn retarget(&mut self, now: f32) {
        let target = self.target();
        if (target - self.to).abs() <= f32::EPSILON {
            return;
        }
        self.from = self.current;
        self.to = target;
        self.started = now;
    }

    /// Advance the ramp to `now` and return the live alpha.
    fn tick(&mut self, now: f32) -> f32 {
        let t = (now - self.started) / AURA_ALPHA_FADE_SECS;
        self.current = fade_alpha(self.from, self.to, t);
        self.current
    }

    /// Whether the unit renders translucent right now (the ramp has arrived at, or is heading
    /// anywhere below, opaque).
    fn translucent(&self) -> bool {
        self.current < 1.0 - ALPHA_EPS
    }

    /// The head tint node's RGB — the reference's `unit+0xce0` head, which its per-frame apply reads
    /// (`0x60cbd9`). `None` when the unit carries no tint aura. Published to the render channel by
    /// [`apply_aura_tint`].
    pub(crate) fn head_tint(&self) -> Option<[u8; 3]> {
        self.tint.first().map(|(_, rgb)| *rgb)
    }

    /// The head proc-11 node's rate — what this unit's animation clocks are being held at, or `None`
    /// when no freeze aura is on it. Head-node-wins like the other two lists.
    pub(crate) fn head_anim_rate(&self) -> Option<f32> {
        self.rate.first().map(|(_, r)| *r)
    }

    /// A node list holding one proc-11 rate node — the state a freeze aura leaves on a unit, for
    /// the driver's wound-refusal tenant without running the aura-slot watcher.
    #[cfg(test)]
    pub(crate) fn with_rate_node_for_tests(spell_id: u32, rate: f32) -> Self {
        Self {
            rate: vec![(spell_id, rate)],
            ..Default::default()
        }
    }
}

/// Publish every rig's body tint to the per-instance channel ([`benilla_world::instance_tint`], decision
/// 0812): the head node's RGB packed the reference's way, keyed on the unit's rig slot — the slot
/// each of its skinned parts already carries in its `MeshTag`, and which the fragment stage reads to
/// index the table.
///
/// **On change, not eased** — the reference's own asymmetry between the two node lists: the alpha
/// gets a 1000 ms cubic ramp (`0x614f80`), the tint goes straight in on change-detection
/// (`unit+0xd04` → `0x710cf0`). `InstanceTints::set` is idempotent, so the steady state costs one
/// compare per rig and never touches the upload.
///
/// Iterates every **`RigSkin`**, not every `AuraNodes`, deliberately: reasserting the whole table
/// each frame means no edge can leak a stale colour into a recycled slot — not an `AuraNodes` that
/// went away, not a unit whose last tint aura dropped. (The despawn/rebuild edge is covered
/// structurally too, in `RigSkin`'s free hook.)
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

/// The tint a rig instance renders with: its own head node, else the nearest one **up its
/// [`benilla_world::model_fade::ParentModel`] chain** — the reference's `0x714000` recursion, which composes
/// an attached model's colours onto its parent's computed ones (the same walk [`ModelAlphas`] does
/// for alpha, and the same law `mesh_tag` cites for why an item's parts carry their wearer's slot).
///
/// It exists because a rigged **item** carries its own instance slot rather than its wearer's
/// (the vertex stage indexes the palette with that field, so it cannot be borrowed):
/// without the walk, a dwarf's Stoneform tint would stop at exactly the pauldrons the rig was added
/// for. A despawned link ends the walk.
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

/// `WOW_TINT_PROBE=RRGGBB` — paint **every live rig slot** that colour, auras ignored.
///
/// The one machine check for the GPU half of the channel: the region upload, the fragment stage's
/// slot lookup, the unpack, and the placement of the multiply. No headless test can reach any of
/// that, and the capture harness cannot stage a real aura to do it with — it sees no networked
/// entities at all, so a tinted *unit* is unstageable there. Pointed at a scene with animated
/// (rigged) doodads it turns each of them the probe colour, which the visual harness then diffs.
///
/// It logs the slot count once, so an unchanged capture reads as "nothing in this scene is rigged"
/// rather than as "the channel is broken" — the two are indistinguishable from the image alone.
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

/// Marker: this rig's clocks are being held by a proc-11 aura, so the release knows to let them go.
///
/// It carries **nothing**, and that is the fix for the first shipped version's bug. That one stored
/// the speed of every clip it took over and wrote the values back on release — the reference's own
/// `+0x60`/`+0x64`/`+0x68` save/restore, transliterated one level too low. It cannot work here,
/// because in the reference a rate belongs to a *bone* (one value, saved once, and clips armed later
/// never carry one) while ours would have to belong to a *clip*, and clips get armed under the freeze
/// carrying values copied from other, already-frozen clips. `transplant_up` is exactly that: moving a
/// one-shot to the torso overlay copies the source clip's speed, which under the freeze was `0`, so
/// the overlay was saved at `0`, restored to `0`, and never advanced again — a cast pose welded to the
/// upper body for the rest of the session, surviving every later cast (it never *finished*, so the
/// overlay never released). Pausing owns no value and so cannot poison one.
#[derive(Component, Default, Debug)]
pub(crate) struct AnimRateFreeze;

/// Hold the clocks of every unit whose head proc-11 node says so — its own rig and its mount's
/// (`[unit+0xd8]` and `[unit+0xdc]`, the two models `0x6201d0` writes) — and let them go when the aura
/// does.
///
/// **A rate of 0 is expressed as a PAUSE, not as `set_speed(0.0)`.** They are the same picture on
/// screen (Bevy evaluates a paused animation at its frozen seek, exactly as `0x712910`'s `bias` rebase
/// holds the reference's current frame) but not the same object: a pause is a bit the clock reads,
/// where a speed is a *value* other code copies. `transplant_up` copies a clip's speed when it moves a
/// one-shot to the torso overlay, and that is how the first version welded a cast pose to the upper
/// body permanently — see [`AnimRateFreeze`] and. Nothing here reads or writes a speed, so nothing can
/// inherit the freeze and outlive it.
///
/// **Reasserted every frame, and that is the faithful shape, not a belt-and-braces clamp.** The
/// reference's rate lives on the bone (`+0xb0`, the factor the clock at `0x71458e` multiplies its
/// window by) and **arming an animation does not touch it**: op4 `0x7121a0`'s success leg writes the
/// track index and the cursors and leaves `+0xb0` alone (only its disarm leg and `SetBoneAnimSpeed
/// 0x712910` write it). So "rate 0 until the aura is reaped" is literally the reference's state
/// across every re-arm in between, and re-pausing each frame is how a per-clip flag says the same
/// thing — including for clips armed *under* the freeze, which the reference freezes by construction.
///
/// It runs **after the driver**, so this frame's arms are already in — which is the whole Ice Block
/// observable: the cast one-shot is armed and then frozen at the frame it was on, so the caster never
/// gets to raise their hand.
///
/// **A non-zero rate does nothing** (and only kit 1744's `8947848.0` is one — a packed grey that
/// landed in the rate column, on Stoned / Petrification / Thadius Spawn). Our clock is f32 seconds
/// where the reference's is integer milliseconds with a modulo, so a rate that large is noise there
/// and different noise here; there is no honest transliteration, and inventing one would be worse than
/// saying so. Recorded, not special-cased.
///
/// What it deliberately does **not** touch: attached effect models (the ice block's own
/// `icebarrier_state.mdx` keeps shimmering — the reference writes the unit's two models only) and
/// global sequences (a different clock there, and a different writer here).
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
                    // Resume everything: the set we paused is "whatever was playing", and it grew
                    // every frame the freeze held. Nothing else on a unit rig pauses an animation
                    // (the portrait booth does, on its own rigs), so there is no third party's pause
                    // to trample.
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

/// The freeze's own instrument (`WOW_MOVE_TRACE`, tag `aur`): one line per rig at each edge, naming
/// the clips it took over and the speed each is *left* running at — the difference between "the clocks
/// stopped" and "nothing was playing anyway", which no count of *absent* anim events can tell apart.
///
/// It prints the speeds precisely because the first version's bug was invisible without them: a `@0`
/// on the thaw line is a clip that will never advance again.
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

/// One aura's CharProc edge, written by the aura-slot watcher
/// ([`crate::creature_anim::arm_aura_state_fx`], which owns the slot diff and the state-kit resolve
/// for both halves of a kit) and drained by [`drain_aura_procs`].
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

/// Turn one `SpellVisualKit` CharProc into the node it installs, or `None` for a proc type we have
/// no verified mechanism for (module docs). **The single dispatch point** — the ECS twin of the
/// client's jump table `0x60dbfc`, and the one place a newly-REd proc type gets an arm.
pub(crate) fn node_for(proc: benilla_formats::CharProc) -> Option<AuraNode> {
    use benilla_formats::char_proc_type as ty;
    match proc.ty {
        ty::ALPHA => Some(AuraNode::Alpha(proc.params[0])),
        // `round(param)` → `0x00RRGGBB` (`0x60d8cc` then ORs the opaque byte on). The param is a
        // float column holding an integral colour: the ghost's 9222653.0 is 0x8CB9FD, pale
        // blue-white; the poison kit's 65280.0 is 0x00FF00.
        ty::TINT => {
            let packed = proc.params[0].round().clamp(0.0, 0xff_ffff as f32) as u32;
            Some(AuraNode::Tint([
                (packed >> 16) as u8,
                (packed >> 8) as u8,
                packed as u8,
            ]))
        }
        // `params[0]` is a playback rate, straight through — `0x60db7e` hands it to
        // `SetBoneAnimSpeed` unexamined, so the one outlier (kit 1744's `8947848.0`, a packed grey
        // that landed in the rate column) is passed through rather than special-cased.
        ty::ANIM_RATE => Some(AuraNode::AnimRate(proc.params[0])),
        _ => None,
    }
}

/// Apply this frame's [`AuraProc`] edges to the units' node lists, then re-aim every live ramp.
///
/// `base_alpha` here only SEEDS a node set created by an aura edge — its steady-state owner is
/// [`refresh_base_alpha`], which re-resolves it on every display change. The getter is the same
/// for players and creatures (the display row's `CreatureModelAlpha`, read by `0x60d2d0`).
/// A unit whose display is unknown falls back to `1.0` — opaque, i.e. the aura's factor alone,
/// which is the safe direction (the alternative would hide a unit whose row simply failed to
/// load).
pub(crate) fn drain_aura_procs(
    mut edges: MessageReader<AuraProc>,
    time: Res<Time>,
    mut commands: Commands,
    mut nodes: Query<&mut AuraNodes>,
    units: Query<&crate::net::NetEntity>,
    creatures: Option<Res<crate::entities::Creatures>>,
) {
    let now = time.elapsed_secs();
    // Node sets for units that don't carry the component yet — see the `Begin` arm.
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
                    // First edge for this unit: stage the node set locally rather than inserting
                    // straight away. `Commands` are deferred to the end of the system, so a unit
                    // that streams in already carrying two aura procs (or gets two in one packet
                    // burst) emits two Begins this frame and the second would `get_mut` the
                    // still-absent component, build a *fresh* set, and insert over the first —
                    // silently keeping only the last aura's factor. Staging makes the outcome of a
                    // same-frame burst come out right, which is the common case for a unit observed
                    // mid-buff, not a corner.
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
                    // Armed and reaped inside one frame (a flickering re-apply) — the staged set is
                    // the live one, so the removal has to land there too.
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

/// Drop every node one spell installed (`0x5ff290`) and re-aim the ramp at the new target
/// (the next head node down, or the bare base).
fn reap(n: &mut AuraNodes, spell_id: u32, now: f32) {
    n.alpha.retain(|(s, _)| *s != spell_id);
    n.tint.retain(|(s, _)| *s != spell_id);
    n.rate.retain(|(s, _)| *s != spell_id);
    n.retarget(now);
}

/// Install one spell's nodes, replacing any it already had (an aura re-applied by a second caster
/// holds a second slot but installs one node set — the reference's own per-spell-id keying).
/// New nodes link **at the head** (the newest node is the one the recompute `0x60d180` reads),
/// which is why both lists insert at the front here and both readers take `.first()`.
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

/// The layer's instrument (`WOW_MOVE_TRACE=<path>`, tag `aur`) — one line per node edge, carrying
/// the spell, the nodes it installed, and the target the ramp is now aiming at.
///
/// This is how "stealth shows nothing" is *reproduced and then closed* without eyeballing a capture
/// (`docs/METHOD.md` §5/§6): with the aura layer dead the trace is silent on `.cast 1784`; with it live
/// the same press prints `aur arm spell=1784 alpha=[0.3] -> base 1.00 target 0.30`, and the drop
/// prints the ramp back to 1.00. It rides the shared move-trace sink so an aura edge interleaves on
/// one timeline with the movement/anim lines — a "why did it not fade when I moved" question needs
/// exactly that.
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

/// `baseAlpha` for a unit — the getter `0x60d2d0`: its CURRENT display row's
/// `CreatureModelAlpha / 255`, for **every** unit kind. There is no player override — the same
/// vtable slot in both classes, byte-identical; an unshifted character reads 1.0 only because its
/// row (49, 50, …) says 255, and a shaman wearing Ghost Wolf's display 4613 reads 102/255 = 0.4
/// through exactly this path. No row → 1.0 (the `0x60d2e4` NULL fallback).
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

/// Re-resolve every unit's base alpha when its display changes — the reference's DISPLAYID
/// watcher path (`0x604990 → 0x60abe0 → 0x60afb0` caches the row →
/// `0x60ad9f → 0x60d180 → StartAlphaFade(target, 1000 ms)`), self-gated on a real record change
/// (`0x60ae10`). This is what makes a unit with `CreatureModelAlpha < 255` translucent with **no
/// aura in play** — the shifted shaman's ghost look — and what ramps it back to opaque when the
/// display swaps home. The 1000 ms cubic fade is part of the mechanism: a display swap always
/// eases to the new alpha, never snaps.
///
/// `Changed<NetEntity>` is our `0x60ae10`: [`crate::entities`]' live-display refresh writes
/// `net.display_id` only on an actual swap (and insertion covers the create path). The base
/// compare keeps a scale-only mutation from restarting anything.
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
            // A unit that never carried a node set needs one exactly when its display is
            // authored translucent — created settled at opaque so the retarget rides the same
            // 1000 ms ramp down a fresh aura node would.
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

/// The fadeable-part view [`apply_aura_alpha`] authors through: the part's material pair and tag,
/// plus every other factor of its alpha (its animated colour alpha, its live appear/despawn ramp)
/// and the light law that decides which material variant it should be on. [`FadeMaterials`] is the
/// "this is a fadeable mesh" marker every writer of this channel keys on.
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
    // Disjointness for the card pass (both want `&mut MeshTag`); a card never carries
    // `FadeMaterials`, so this filter excludes nothing that would otherwise match — the same
    // trick `apply_self_model_fade` uses for the same pair of queries.
    Without<benilla_world::billboard::BillboardCard>,
>;

/// Drive every unit's aura-alpha ramp and author its parts' render alpha + material.
///
/// Ordered **after** the three steady-state authors of a unit part's alpha field (the interior
/// classifier, the visibility authority, `entities::apply_unit_mat_alpha`) and **before** the
/// self-avatar zoom feather, so it overrides them for exactly the units that carry a live aura
/// alpha, and the self feather stays the last word on your own body (it folds this factor in
/// itself — [`crate::player::apply_self_model_fade`]). This is the same override-then-release
/// protocol the feather uses; [`AuraNodes::authoring`] is its latch.
///
/// The product it writes is the reference's own: this unit's `ramped aura alpha × the part's
/// animated colour alpha × its live appear/despawn ramp`. Reading the fade back out of
/// [`RenderFade`] rather than the tag keeps every factor a pure function of state, so the write is
/// idempotent (running it twice can't compound) exactly as `apply_unit_mat_alpha`'s is.
///
/// **The material swap is load-bearing, not cosmetic.** A cutout/opaque batch ignores instance
/// alpha, so writing `0.3` into the tag without moving the part onto its **blend twin** renders no
/// translucency at all — the same "blend pass while `0 < fade < 1`" the reference runs for its
/// doodad fade ([`benilla_world::model_fade`]). It goes through [`FadeMaterials::material_for`], the one
/// place the blend-vs-law axes compose, so this writer and the classifier can never disagree.
/// Note the swap keys on the *instance* alpha only: a batch whose own animated colour alpha dips is
/// a per-batch quantity the reference combines separately, and forcing those onto a blend pass would
/// change every existing unit's look.
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
        // Declare it to the engine, which owns the composed model alpha and its chain walk
        // (`model_fade::ModelFade` — 1164's inversion). This is the whole of the aura's claim on
        // the channel: a number and a reason, never a write.
        //
        // Written **before** the settled-opaque early-out below, and unconditionally on change:
        // a declaration the engine keeps reading has to be retired by its declarer, and the ramp
        // arriving at 1.0 is exactly when that matters. (`AuraNodes` itself is never removed — it
        // parks at 1.0 — so there is no removal hook to hang this on.)
        if declared.is_none_or(|d| d.0 != alpha) {
            commands
                .entity(root)
                .insert(benilla_world::model_fade::ModelFade(alpha));
        }
        let translucent = n.translucent();
        if !translucent && !n.authoring {
            continue; // settled opaque: not ours, nothing to release
        }
        // The walked set doubles as "which anchors belong to this unit" for the card pass below —
        // built only for a unit we're authoring, so a quiet world never pays for it.
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
        // The release frame is the one where we authored the settled value and handed the material
        // back; after it this unit costs a ramp tick and the branch above.
        n.authoring = translucent;
    }
}

/// Fold the unit's aura alpha into its **billboard cards** — the batches that can't be tree children.
///
/// An M2's billboard batches are world ROOT entities that merely *follow* an anchor inside the model
/// (their mesh is centred on the bone pivot and their transform belongs to the
/// billboard system), so a descendant walk cannot see them. Skipping them is a burned trap, not a
/// hypothetical: the night-elf eye glow — two additive `…EYEGLOW.BLP` quads at head height — went on
/// burning in mid-air after the body it belongs to had faded out (ledger B71), and a stealthed night
/// elf with full-brightness eyes is the same bug wearing this system's clothes. Cards are picked up
/// by the same rule the self-avatar feather uses: test the card's follow-anchor against the walked
/// set. One multiply covers both halves — the card dims with the body, and for an ADD blend the
/// shader's `out_rgb *= faded_alpha` takes a low alpha toward gone.
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
        // Compose from the animation's own factor rather than from the tag we'd read back, so the
        // card's per-sequence alpha animation stays alive under the aura — and so the release frame's
        // `alpha = 1` write lands exactly on the value the card would have had anyway.
        let authored = anim.map_or(1.0, |a| a.current);
        let bits = benilla_world::mesh_tag::with_alpha(tag.0, authored * alpha);
        if tag.0 != bits {
            tag.0 = bits;
        }
    }
}

/// Depth-first author for [`apply_aura_alpha`]: apply to `entity` if it is a fadeable part, then
/// recurse regardless — a joint or an attach-model root carries no [`FadeMaterials`] itself but the
/// held weapon / helm / shoulder meshes hang beneath it, and the reference's alpha is a per-CGUnit
/// property that its attached models render off (the same reason
/// [`benilla_world::model_fade::apply_despawn_fade`] and the self feather both walk descendants, not
/// children).
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
        // A part still WAITING on its appear-fade is deliberately invisible until the ramp arms (its
        // tag alpha is seeded near-zero at spawn), so authoring it would flash a streaming-in unit's
        // geometry at the aura alpha for the pending window. Every other author of this channel
        // filters the same way. Skip the write, still recurse — a joint's children can be past it.
        if !pending {
            // Every live factor on this part, recomputed from state (never read back from the tag),
            // so the write is idempotent: the unit's ramped aura alpha × the batch's animated colour
            // alpha × the part's own live appear/despawn ramp.
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
            // The INSTANCE alpha decides the pass; a settled-opaque release hands the law's steady
            // material back and asks the classifier to re-author, so nothing is left on a stale
            // one. The water-plane axis composes here like everywhere else (`far_resolved`).
            let want = benilla_world::model_render::far_resolved(
                fm.material_for(lit, alpha < 1.0),
                far_side,
                far_twins,
            )
            .clone();
            if mat.0 != want {
                mat.0 = want;
                if alpha >= 1.0 && lit.is_some() {
                    // 0734's queue: let the classifier re-assert this part's full payload (probe
                    // slot / fog bit) over whatever this episode wrote.
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

/// This unit's live aura alpha, for the writers that compose it into their own product rather than
/// deferring to [`apply_aura_alpha`] (the self-avatar feather, which runs after it and wins on the
/// self body). `1.0` when the unit carries no aura nodes.
pub(crate) fn root_alpha(nodes: Option<&AuraNodes>) -> f32 {
    nodes.map_or(1.0, |n| n.current)
}

#[cfg(test)]
mod tests;
