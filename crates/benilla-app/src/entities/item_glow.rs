//! **Item / enchant glow effects** (decision 0805): the permanent weapon glow a display authors,
//! and the enchant glow a weapon's enchant carries — `Spells\Enchantments\*.mdx` effect models
//! hung on the ITEM's own model, not on the wearer's body.
//!
//! ## The chain, and where each half is verified
//!
//! `ItemDisplayInfo` col 22 → `ItemVisuals` → 5 × `ItemVisualEffects` → a glow `.mdx` per slot
//! ([`benilla_formats::ItemVisualCatalog`], whose module doc owns the table shapes and the
//! reference's per-slot skip rules). Two sources feed the id, and the reference resolves exactly
//! one of them per item (the call sites below were read at the bytes for decision 0805):
//!
//! - **Intrinsic** — the display's own id, passed by the character's held-item attach:
//!   `0x47a200: mov edx,[esi+0x58]` → `0x4798c0(…, itemVisualsId)`, which calls the attach
//!   `0x479700` iff the id is **`> 0`** (`jle` skip). The same primitive attaches the helm (id
//!   11), the shoulders (5/6) and the quiver (26) — and **every one of those call sites pushes a
//!   literal `0`**, so on a character body only the WEAPON/SHIELD hand attach can glow. The
//!   ranged-ammo lane (`0x479f40`) passes `[ebx+0x58]` at its tail, so a thrown/ammo model glows
//!   from its own display too.
//! - **Enchant** — `0x5eed50` per weapon slot → `0x62ec70(item)`: if the base display's
//!   ItemVisuals row **exists**, it returns `-1` and the enchant glow is suppressed (the base
//!   already draws one); otherwise the first of the item's 7 enchant slots whose
//!   `SpellItemEnchantment` row carries a nonzero visual wins. [`effective_visual`] is that fork.
//!
//! ## Where the glow hangs
//!
//! `0x479700` walks the 5 effect columns and calls `0x712f70(glow, itemModel, attachId = the
//! column index)` — so **slot i is M2 attachment id i on the item's own model**, and a row that
//! repeats one effect across all five slots (BlueGlow_Med ×5, the common shape) reads as a glow
//! along the whole blade.
//!
//! **How many of the five actually land is the model's business, and it varies**: of the 160
//! distinct weapon models the 360 glowing displays resolve to, 61 author the full ids 0..4 and 44
//! author only 2/3/4 (Ironfoe's hammer is one — three of its five copies draw). An id the model
//! doesn't author hangs nothing at all, byte-verified: `0x712f70` stores `0x710310`'s `0xffff`
//! miss and every consumer of that field (`0x7140aa`, `0x718668`, `0x719266`) tests
//! `cmp 0xffff; je` and skips the child — there is no fallback to the model origin. Which record
//! an id resolves to is the model's **AttachLookup**, never a scan of its table
//! (`benilla_m2::M2Model::attachment`): ~5% of weapon models author two records under one id.
//!
//! An item model poses at rest (no rig is spawned for it — `super::equipment`), so the attach
//! point is its bind-pose position ([`crate::portrait::attachment_point`]) and the glow instance
//! is a plain child of the item root: it rides the hand/hip/back bone with the weapon, and dies
//! with it on a gear or sheath change. Everything past that — the effect model's own rig,
//! emitters, ribbons, material animation — is the shared effect body
//! ([`super::spell_fx::attach_effect_visuals`]); these models are almost all pure particle
//! emitters (BlueGlow_Med: zero render batches, one Plane/Add emitter on `Flare.blp`).

use std::collections::HashMap;

use benilla_formats::{EnchantCatalog, ItemVisualCatalog, ITEM_VISUAL_SLOTS};
use bevy::prelude::*;

use benilla_assets::m2_url;
use benilla_assets::materials::WowModelMaterial;

use super::equipment::{ItemDisplays, ItemModelKind};
use super::spell_fx::{attach_effect_visuals, EffectHost, FxMaterials, FxTintAnims};
use super::{DisplayModel, ModelHandle};

/// The glow chain's data + the effect-model cache: the joined `ItemVisuals`/`ItemVisualEffects`
/// catalog, plus a path-keyed [`DisplayModel`] per glow model (the [`super::spell_fx::SpellFx`]
/// pattern — entries are created by [`ensure_glow_models`] at equipment-resolve time and built by
/// `super::update_display_models` the same frame). Optional resource: without the DBC nothing
/// glows, exactly as before this lane existed.
///
/// The enchant half of the fork lives in [`crate::items::Enchants`] — the same
/// `SpellItemEnchantment` load the tooltip's enchant line reads (decision 0915). Without it a
/// weapon still shows its INTRINSIC glow; only the enchant leg goes quiet.
#[derive(Resource)]
pub(crate) struct ItemGlows {
    pub(super) visuals: ItemVisualCatalog,
    pub(super) models: HashMap<String, DisplayModel>,
}

impl ItemGlows {
    pub(super) fn new(visuals: ItemVisualCatalog) -> Self {
        ItemGlows {
            visuals,
            models: HashMap::new(),
        }
    }

    /// The glow models an ItemVisuals id resolves to, by attach slot — `None` when the id names
    /// no row (0, the shipped `-1`s, out of range).
    pub(super) fn effects(&self, visual: i32) -> Option<&[Option<String>; ITEM_VISUAL_SLOTS]> {
        self.visuals.effects(visual)
    }
}

/// The ItemVisuals id an item actually glows with — the reference's base-or-enchant fork
/// (`0x62ec70`): the **base** display's id wins whenever it names a real ItemVisuals row (the
/// client returns `-1` from the enchant resolver precisely so the two can't double up); otherwise
/// the **first** enchant slot whose `SpellItemEnchantment` row carries a nonzero visual. `0` when
/// neither does.
/// `enchants` absent (the DBC failed to load) means only the base leg can answer.
pub(in crate::entities) fn effective_visual(
    glows: &ItemGlows,
    enchants: Option<&EnchantCatalog>,
    base: i32,
    item_enchants: impl IntoIterator<Item = u32>,
) -> i32 {
    if glows.visuals.effects(base).is_some() {
        return base;
    }
    let Some(enchants) = enchants else { return 0 };
    for enchant in item_enchants {
        if let Some(visual) = enchants.visual(enchant) {
            return visual;
        }
    }
    0
}

/// Create the [`DisplayModel`] cache entries for a visual's glow models, so
/// `super::update_display_models` builds them the same frame the equipment resolve asked for
/// them (the held-item pattern).
pub(in crate::entities) fn ensure_glow_models(
    glows: &mut ItemGlows,
    visual: i32,
    asset_server: &AssetServer,
) {
    let Some(paths) = glows.visuals.effects(visual) else {
        return;
    };
    // Collected first: `effects` borrows the catalog, and the insert below borrows the cache.
    let wanted: Vec<String> = paths
        .iter()
        .flatten()
        .filter(|p| !glows.models.contains_key(*p))
        .cloned()
        .collect();
    for path in wanted {
        let handle = ModelHandle::M2(asset_server.load(m2_url(&path)));
        glows.models.insert(
            path,
            DisplayModel {
                handle,
                ..super::empty_shell()
            },
        );
    }
}

/// A spawned held-item root that should carry a glow: the item's display-model key (for its
/// attachment table) and the resolved ItemVisuals id. Written by `super::equipment`'s attach;
/// consumed once by [`attach_item_glows`], which then marks it [`ItemGlowAttached`].
#[derive(Component)]
pub(in crate::entities) struct ItemGlow {
    pub(in crate::entities) display: u32,
    pub(in crate::entities) kind: ItemModelKind,
    pub(in crate::entities) visual: i32,
    /// The ITEM's seat on the wearer's body — the attach point's bone and offset
    /// ([`super::BoneAttach`]). Carried purely so this lane can publish the booth mirrors of what it
    /// spawns (decision 0822): a glow's seat is this offset plus the slot's point on the item model,
    /// and by the time the glow models load the attach path that knew the body bone is long gone.
    pub(in crate::entities) bone: u16,
    pub(in crate::entities) offset: bevy::prelude::Vec3,
    /// The ITEM's **M2 attachment id** on the body, for the same reason: a glow is chained under
    /// the item (`0x712f70`), so a widget's attach reset takes the glow exactly when it takes the
    /// item it hangs on ([`crate::portrait::attach_reset`]).
    pub(in crate::entities) attach: u16,
}

/// Set once an item root's glow instances are spawned (or resolved to nothing) — the once-only
/// gate. The instances are children of the root, so they need no lifetime bookkeeping of their
/// own: a gear change or the unit despawning takes the root and them with it. A **sheath swap**
/// keeps them: it moves the root to the new attach point (decision 0826), so a glowing weapon
/// keeps its glow — and its live particles — across the draw.
#[derive(Component)]
pub(crate) struct ItemGlowAttached;

/// Spawn each pending item root's glow instances: one effect-model instance per authored slot, at
/// that slot's attachment point on the item's own model.
///
/// All-or-nothing per item — while any of the item's glow models is still loading the whole set
/// waits, so a two-model visual can't spawn half of itself and then re-enter here. A model that
/// never loads simply never glows (the item is still perfectly drawn); the retry is a handful of
/// roots per frame.
pub(super) fn attach_item_glows(
    mut commands: Commands,
    pending: Query<(Entity, &ItemGlow), Without<ItemGlowAttached>>,
    glows: Option<Res<ItemGlows>>,
    items: Option<Res<ItemDisplays>>,
    time: Res<Time>,
    mut wow_materials: ResMut<Assets<WowModelMaterial>>,
    mut tint_reg: ResMut<FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    // One breadcrumb per session the first time a glow actually spawns — the machine-readable
    // "this lane is live" signal for a subsystem whose whole symptom is *absence* (the same
    // idiom `apply_unit_mat_alpha` uses for its cull).
    mut logged: Local<bool>,
) {
    let (Some(glows), Some(items)) = (glows, items) else {
        return;
    };
    let now = time.elapsed_secs();
    for (root, glow) in &pending {
        let Some(paths) = glows.effects(glow.visual) else {
            commands.entity(root).insert(ItemGlowAttached); // nothing to hang
            continue;
        };
        let Some(item) = items.models.get(&(glow.display, glow.kind)) else {
            continue; // the item's own display entry is gone (a cache evict) — retry
        };
        // Ready = every authored slot's model has built its parts. Emitter-only models build an
        // EMPTY part list, which is `Some` — the readiness test is the option, not the length.
        let ready = paths
            .iter()
            .flatten()
            .all(|p| glows.models.get(p).is_some_and(|dm| dm.parts.is_some()));
        if !ready {
            continue;
        }
        let mut spawned = 0usize;
        for (slot, path) in paths
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().map(|p| (i, p)))
        {
            let Some(dm) = glows.models.get(path) else {
                continue;
            };
            // Slot index == M2 attachment id on the ITEM model (module doc). **A model missing
            // that point hangs nothing there**, and that is the reference's behaviour, not a
            // shortcut: `0x712f70` stores the attach-lookup miss as `0xffff`
            // (`0x710310`'s bounds return), and every consumer of that field —
            // `0x7140aa`/`0x718668`/`0x719266` — tests `cmp 0xffff; je` and skips the child
            // outright. It is a common case, not a corner: Ironfoe's hammer authors only ids
            // 2/3/4, so three of its visual's five copies draw and two do not.
            let Some(at) =
                crate::portrait::attachment_point(&item.skeleton, &item.attachments, slot as u16)
            else {
                debug!(
                    "item glow: display {} has no attachment {slot} for {path}",
                    glow.display
                );
                continue;
            };
            let instance = commands
                .spawn((Transform::from_translation(at), Visibility::default()))
                .id();
            commands.entity(root).add_child(instance);
            // The booth mirrors for this instance (decision 0822), at its seat on the BODY: the item's
            // attach point plus this slot's point on the item model. `attach_effect_visuals` below
            // spawns the effect's real geometry and emitters — the meshes as instance children, the
            // emitters as free owner-followed entities — and the shared spell-fx lane is deliberately
            // not the place to stamp portrait markers (a *spell's* fx must never reach a booth), so
            // this lane publishes its own from the same `dm`. Without them a permanently-glowing
            // weapon glowed nothing in the character window: 32 of the 35 shipped glow models are pure
            // emitters, and `Sparkle_A.m2` is a lone camera-facing quad.
            let seat = glow.offset + at;
            for p in dm.parts.iter().flatten() {
                match &p.billboard {
                    Some(info) => {
                        commands.entity(instance).with_child((
                            Transform::default(),
                            Visibility::default(),
                            crate::portrait::PortraitBillboard {
                                mesh: p.mesh.clone(),
                                material: p.material.clone(),
                                bone: glow.bone,
                                seat: crate::portrait::PortraitSeat::Rider(seat + info.pivot),
                                kind: info.kind,
                                attach: Some(glow.attach),
                            },
                        ));
                    }
                    None => {
                        commands.entity(instance).with_child((
                            Transform::default(),
                            Visibility::default(),
                            crate::portrait::PortraitRider {
                                static_mesh: p.mesh.clone(),
                                material: p.material.clone(),
                                bone: glow.bone,
                                offset: seat,
                                attach: Some(glow.attach),
                            },
                        ));
                    }
                }
            }
            if !dm.emitters.is_empty() {
                commands
                    .entity(instance)
                    .insert(crate::portrait::PortraitEffects {
                        bone: glow.bone,
                        offset: seat,
                        attach: Some(glow.attach),
                        emitters: dm.emitters.clone(),
                    });
            }
            attach_effect_visuals(
                &mut commands,
                instance,
                dm,
                now,
                false, // a weapon glow is never ground-anchored
                // An attached model in the client's sense — it rides the ITEM (`0x712f70`), so it
                // is chained to that item root and through it to the wearer: the glow fades in
                // with the body instead of blazing over a character that has not appeared yet,
                // and it dies with the weapon instead of hanging in the air behind it (0833).
                EffectHost { parent: Some(root) },
                // An `ItemVisuals` glow is armed by a different leg than `PlaySpellVisualKit`
                // (0805) and carries no kit stage — the plain single-clip arm, as before.
                None,
                &mut FxMaterials {
                    store: &mut wow_materials,
                    tint: &mut tint_reg,
                    uv: &mut uv_reg,
                    table: &mut anim_table,
                },
                &ibps,
                &mut palettes,
                None, // the glow models author one looping sequence — the default pick
            );
            spawned += 1;
        }
        commands.entity(root).insert(ItemGlowAttached);
        // Counted from what actually spawned, never from what the row *authors* — the two differ
        // whenever the item model lacks a point (above), and a count of authored paths reads as a
        // glow that isn't there.
        let authored = paths.iter().flatten().count();
        if !*logged && spawned > 0 {
            *logged = true;
            info!(
                "item glow: display {} visual {} → {spawned} of {authored} model(s) attached \
                 (the first this session)",
                glow.display, glow.visual,
            );
        }
        debug!(
            "item glow: display {} visual {} → {spawned} of {authored} model(s)",
            glow.display, glow.visual,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(visual_rows: &[u32], enchant_rows: &[(u32, i32)]) -> (ItemGlows, EnchantCatalog) {
        let visuals = visual_rows
            .iter()
            .map(|id| {
                (
                    *id,
                    std::array::from_fn(|i| {
                        (i == 3).then(|| format!("Spells\\Enchantments\\Row{id}.mdx"))
                    }),
                )
            })
            .collect();
        (
            ItemGlows::new(ItemVisualCatalog::from_visuals(visuals)),
            EnchantCatalog::from_rows(
                enchant_rows.iter().copied().collect(),
                HashMap::new(),
                Default::default(),
            ),
        )
    }

    /// The base-or-enchant fork (`0x62ec70`): an intrinsic visual wins and **suppresses** the
    /// enchant's, an item without one takes the first enchant slot that carries a visual, and
    /// enchant slots with no visual are walked past rather than ending the scan.
    #[test]
    fn base_visual_wins_over_enchant_and_suppresses_it() {
        let (glows, ench) = catalog(&[25, 61], &[(1, 61), (7, 25)]);
        let ench = Some(&ench);
        // Base 25 exists → base, even with an enchant that also carries one.
        assert_eq!(effective_visual(&glows, ench, 25, [1]), 25);
        // No base → the enchant's.
        assert_eq!(effective_visual(&glows, ench, 0, [1]), 61);
        // A visual-less enchant in the way: the scan continues to the one that has it.
        assert_eq!(effective_visual(&glows, ench, 0, [999, 7]), 25);
        // Nothing anywhere.
        assert_eq!(effective_visual(&glows, ench, 0, [999]), 0);
        // The shipped `-1` base is not a row: it neither glows nor suppresses the enchant.
        assert_eq!(effective_visual(&glows, ench, -1, [1]), 61);
        assert_eq!(effective_visual(&glows, ench, -1, []), 0);
    }

    /// No enchant catalog (the DBC failed to load): the INTRINSIC leg still answers in full, and
    /// only the enchant leg goes quiet — the split's whole safety claim.
    #[test]
    fn without_the_enchant_catalog_only_the_base_leg_answers() {
        let (glows, _) = catalog(&[25, 61], &[(1, 61)]);
        assert_eq!(effective_visual(&glows, None, 25, [1]), 25);
        assert_eq!(effective_visual(&glows, None, 0, [1]), 0);
    }
}
