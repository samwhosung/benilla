//! **Re-dressing** a character in place when its worn equipment changes — the gear-change half of
//! [`super`] (decision 0074's teardown, replaced).
//!
//! A gear change used to tear the whole visual down (`despawn_related::<Children>()` + strip the rig,
//! the animation player, the bone-attach table and the held-item bookkeeping) and let
//! [`super::attach_entity_visuals`] rebuild it the next frame. That is not what the reference does,
//! and the difference is visible: it destroys and re-creates *every* attached model too — both
//! weapons, their enchant glows, the helm, the shoulders, every persistent aura visual — so swapping
//! a belt blinked things that had nothing to do with the belt (and, before decision 0833 chained an
//! effect's lifetime to its model, smeared their particle clouds across the ground behind a running
//! character).
//!
//! The reference re-dresses on the SAME `CM2Model`. Its character compositor re-blits the dirty
//! region groups into the component's own 256² target and re-runs the geoset selection `0x477520`,
//! whose only reach into the model is the per-instance visibility array `+0x98` — a range filter
//! writing ordinals (`0x7110d0`), nothing more; the character compositor is the *only* thing in
//! the binary that can hide a submesh. Attachments are installed through a different mechanism
//! entirely (`0x712f70 CM2Model::attachChild`) and the compositor never touches them.
//!
//! So this system is that law, and only that law:
//!
//! 1. re-composite the body atlas + re-resolve the cape texture for the new equipment, and re-point
//!    every standing part's material variants at them ([`super::dress::part_materials`]);
//! 2. re-run the geoset selection — a batch the new gear hides is despawned, one it reveals is
//!    spawned by the same [`super::dress::spawn_part`] the first build used;
//! 3. touch nothing else. The rig, the pose buffer, the bone anchors, the held items and everything
//!    hanging off them are the same objects they were a frame ago.

use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::{NetEntity, ObjectStore};
use crate::portrait::PortraitPart;
use benilla_assets::materials::WowModelMaterial;
use benilla_assets::WorldAssets;
use benilla_world::interior::InteriorLit;
use benilla_world::model_fade::{
    join_unit_appear_fade, FadeMaterials, PendingAppearFade, RenderFade,
};

use super::super::equipment::AppliedEquipment;
use super::super::{
    Characters, Creatures, EntityPart, Equipment, ItemDisplays, SkinComposites, SkinSections,
    VisualAttached,
};
use super::char_skin::{
    build_char_skin_materials, equip_geosets, resolve_char_look, resolve_worn_equip,
    CharSkinMaterials,
};
use super::dress::{part_materials, spawn_group, DressedPart, PartDress};
use super::merge::{self, DressedGroup, MergedFormsCache};

/// The per-part write surface of a re-dress: the displayed material and the three records that
/// decide what a part draws when something *else* owns that channel — the interior classifier's law
/// variants, the fade ramps' twin pair, and the portrait booths' steady mirror. All optional
/// because a [`DressedPart`] is also carried by a billboard batch's **anchor**, which draws nothing
/// (its card is a world root) and holds none of them.
type PartWrites<'a> = (
    Option<Mut<'a, MeshMaterial3d<WowModelMaterial>>>,
    Option<Mut<'a, InteriorLit>>,
    Option<Mut<'a, FadeMaterials>>,
    Option<Mut<'a, PortraitPart>>,
);

/// Re-dress every player whose worn equipment changed, in place (module docs). Players only: a
/// creature's look never changes this way, and a character-model NPC wears its display's columns —
/// a *display* change is a different model and stays a teardown
/// ([`super::super::live_display::refresh_live_display`]).
#[allow(clippy::type_complexity)]
pub(in crate::entities) fn redress_player_looks(
    mut commands: Commands,
    mut players: Query<
        (
            Entity,
            &NetEntity,
            &Equipment,
            &mut AppliedEquipment,
            &Children,
            Option<&benilla_world::rig_palette::RigSkin>,
            Option<&super::super::BoneAttach>,
            // The pose buffer: a revealed billboard batch's card bone resolves its anchor on
            // first demand (`RigPose::anchor_for`, decision 1355).
            Option<&mut benilla_world::rig_anim::RigPose>,
            Option<&benilla_world::interior::BodyBakeCenter>,
            Option<&benilla_world::model_fade::UnitAppearFade>,
        ),
        With<VisualAttached>,
    >,
    // The parts already standing under those players — found again by the batch index each carries.
    mut standing: Query<(
        &DressedPart,
        Option<&DressedGroup>,
        Option<&mut MeshMaterial3d<WowModelMaterial>>,
        Option<&mut InteriorLit>,
        Option<&mut FadeMaterials>,
        Option<&mut PortraitPart>,
        Has<RenderFade>,
        Has<PendingAppearFade>,
    )>,
    stores: Query<&ObjectStore>,
    creatures: Option<Res<Creatures>>,
    displays: Option<Res<ItemDisplays>>,
    characters: Option<Res<Characters>>,
    // The character-skin build chain, nested to stay inside Bevy's system-param tuple limit — the
    // same set [`super::attach_entity_visuals`] composites with, because it is the same composite.
    skin_build: (
        Option<Res<SkinSections>>,
        Option<Res<WorldAssets>>,
        ResMut<Assets<Image>>,
        ResMut<SkinComposites>,
        Res<AssetServer>,
        benilla_world::model_render::M2BatchMaterials,
        ResMut<Assets<Mesh>>,
        ResMut<MergedFormsCache>,
    ),
    // The animated-material lane a re-dressed part may need a material of its own on (2295).
    // Plumbing here rather than a live case: a re-dress is a PLAYER's gear change, and no
    // character batch in the corpus carries a per-sequence loop — but the dressing law is one law,
    // and a path that could not express it would be a place for the two to drift.
    mut own_lane: (
        ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
        ResMut<benilla_world::doodad_anim::TintAnimMaterials>,
        ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ),
    time: Res<Time>,
) {
    let (
        sections,
        world_assets,
        mut images,
        mut skin_composites,
        asset_server,
        mut mats,
        mut meshes,
        mut merged,
    ) = skin_build;
    let now = time.elapsed_secs();
    for (entity, net, live, mut applied, children, rig, bones, mut pose, bake_center, unit_fade) in
        &mut players
    {
        // `settled` (decision 0074): every non-empty visible-item entry has resolved through the
        // template cache. Re-dressing on a half-resolved set would composite a half-dressed atlas
        // and then composite again a frame later.
        if net.kind != EntityKind::Player || !live.settled || *live == applied.0 {
            continue;
        }
        // Stamp first, unconditionally: a player whose display resolved to no model at all (the
        // cube fallback) has nothing to re-dress, and must not re-enter this arm every frame.
        applied.0 = *live;
        let Some(dm) = net
            .display_id
            .and_then(|disp| creatures.as_deref()?.models.get(&disp))
        else {
            continue;
        };
        let Some(parts) = dm.parts.as_deref() else {
            continue;
        };

        // The dressing inputs, re-resolved exactly as the first build resolved them.
        let worn = resolve_worn_equip(net, Some(live), Some(dm));
        let look = resolve_char_look(net, Some(dm), entity, &stores);
        let eg = equip_geosets(
            displays.as_deref(),
            &worn.bodyslots,
            worn.cloak,
            worn.helm,
            worn.tabard_preview,
        );
        let visible: Option<Vec<u16>> = look.as_ref().and_then(|l| {
            let cg = characters.as_deref()?;
            Some(cg.0.visible_geosets(l.race, l.sex, l.hair_style, l.facial_hair, &eg))
        });
        // The re-composite. Cached per (appearance, worn set) in `SkinComposites`, so a swap back
        // to a set already worn this session costs a lookup.
        let char_mats: CharSkinMaterials = match look.as_ref() {
            Some(l) => build_char_skin_materials(
                l,
                worn.bodyslots,
                worn.cloak,
                worn.emblem,
                worn.tabard_preview,
                displays.as_deref(),
                sections.as_deref(),
                world_assets.as_deref(),
                parts,
                &mut images,
                &mut skin_composites.0,
                &asset_server,
                &mut mats,
            ),
            None => (None, None, None, (None, None)),
        };
        // No look (a druid form, a GM morph — a Player-kind entity on a beast display) means no
        // geoset filter, exactly as at build: every batch the model authors draws.
        let shows = |geoset: u16| visible.as_ref().is_none_or(|v| v.contains(&geoset));
        // The grouping the new gear asks for (`merge`): a standing group survives exactly when
        // the new grouping contains its member set unchanged — then it re-points in place like
        // any part; otherwise it is torn down and the new group spawns whole. Keyed by first
        // member, which is what a standing entity's `DressedPart::index` carries.
        let groups = merge::guard_groups(
            merge::group_parts(parts, |p| shows(p.geoset_id)),
            parts,
            &char_mats,
        );
        let group_of: std::collections::HashMap<u32, &merge::BodyGroup> =
            groups.iter().map(|g| (g.first(), g)).collect();

        // Pass 1 — the parts already standing: hide-by-despawn, or re-point.
        let mut present = vec![false; parts.len()];
        let (mut hidden, mut repointed) = (0usize, 0usize);
        for child in children.iter() {
            let Ok((dp, group, mat, lit, fade_mats, portrait, ramping, pending)) =
                standing.get_mut(child)
            else {
                continue;
            };
            let dp = *dp;
            let Some(part) = parts.get(dp.index as usize) else {
                continue; // a stale index (the model rebuilt under us) — leave it to the teardown
            };
            let standing_members: &[u32] = match group {
                Some(g) => &g.0,
                None => std::slice::from_ref(&dp.index),
            };
            let still_grouped = group_of
                .get(&dp.index)
                .is_some_and(|g| g.members == standing_members);
            if !shows(part.geoset_id) || !still_grouped {
                // A billboard batch's card is a world ROOT following this anchor's joint, so it does
                // not cascade — reap it by name (decision 0153's lifecycle, [`DressedPart::card`]).
                if let Some(card) = dp.card {
                    if let Ok(mut ec) = commands.get_entity(card) {
                        ec.despawn();
                    }
                }
                commands.entity(child).despawn();
                hidden += 1;
                continue;
            }
            for &m in standing_members {
                present[m as usize] = true;
            }
            // A card's material is the model's own batch material — never a character slot — so a
            // billboard part has nothing to re-point.
            if part.billboard.is_none() {
                repoint_part(
                    part,
                    &char_mats,
                    (mat, lit, fade_mats, portrait),
                    ramping || pending,
                );
                repointed += 1;
            }
        }

        // Pass 2 — the batches the new gear reveals. They join the unit's own appear-fade clock if
        // one is still in flight (the login gear cascade lands mid-ramp), exactly as a
        // late-resolving held item does — never a second ramp of their own, never a pop.
        //
        // A revealed billboard batch's card bone resolves its anchor first (decision 1355), over
        // exactly the parts the spawn loop below will dress.
        let card_anchors: std::collections::HashMap<u16, Entity> = match pose.as_mut() {
            Some(p) => parts
                .iter()
                .enumerate()
                .filter(|&(i, part)| !present[i] && shows(part.geoset_id))
                .filter_map(|(_, part)| part.billboard.as_ref().map(|b| b.bone))
                .filter_map(|bone| p.anchor_for(&mut commands, entity, bone).map(|a| (bone, a)))
                .collect(),
            None => std::collections::HashMap::new(),
        };
        let dress = PartDress {
            unit: entity,
            kind: benilla_world::model_render::ModelKind::Creature,
            char_mats: &char_mats,
            object: &benilla_world::interact::WorldObject {
                kind: benilla_world::model_render::ModelKind::Creature,
                label: super::display_label(&dm.handle),
                id: net.display_id.unwrap_or(0),
                detail: format!("emitters: {}", dm.emitters.len()),
            },
            inst_slot: rig.map_or(0, |r| r.slot),
            rigged: bones.is_some(),
            anchors: card_anchors,
            bake_center: bake_center.map_or(dm.bake_center_local, |c| c.0),
            idle_aabb: idle_aabb(dm),
            now,
            fade: join_unit_appear_fade(unit_fade.copied()),
        };
        let mut shown = 0usize;
        for group in &groups {
            if present[group.first() as usize] {
                continue;
            }
            let forms = merged.forms(parts, group, &mut meshes);
            let part = merge::group_part(parts, group, forms);
            spawn_group(
                &mut commands,
                &part,
                group,
                &dress,
                &mut super::dress::OwnMats {
                    store: mats.materials(),
                    uv: &mut own_lane.0,
                    tint: &mut own_lane.1,
                    table: &mut own_lane.2,
                },
            );
            shown += 1;
        }
        // One line per re-dress — a handful per session, and the only readout of a mechanism whose
        // whole point is that nothing else moves. `atlas` is the composited body material's id: it
        // must CHANGE when the worn set changes a body region, which is what says the re-composite
        // reached the parts rather than merely being built. The counts say which batches the new
        // gear hid and revealed. Nothing about the unit's attachments appears here because this
        // system cannot touch them.
        info!(
            "redress: {entity} — {hidden} batch(es) hidden, {shown} shown, {repointed} re-pointed, \
             atlas {:?}",
            char_mats.0.as_ref().map(|(single, _)| single.0.id()),
        );
    }
}

/// Re-point one standing part at the freshly-built material set its unit now wears.
///
/// The three records are updated whatever the part is doing, because each is read by a *different*
/// owner at a different moment: the interior classifier resolves the law's steady material, the fade
/// ramps resolve their twin every frame from [`FadeMaterials`] + the law, and the portrait booths
/// mirror the steady one whenever a booth rebuilds. The **displayed** material is only written when
/// no ramp owns that channel — a part mid-appear-fade wears its blend twin, and
/// [`benilla_world::model_fade::apply_render_fade`] re-resolves it from the two records above on its next
/// tick, so writing the steady handle here would flash it opaque for a frame.
fn repoint_part(
    part: &EntityPart,
    char_mats: &CharSkinMaterials,
    (mat, lit, fade_mats, portrait): PartWrites,
    fading: bool,
) {
    // A batch with no character texture slot draws its shared model's own built materials, which no
    // amount of gear can change — the body atlas, the hair, the cape and the extra skin are the
    // whole of what a re-dress can touch.
    if part.char_slot.is_none() {
        return;
    }
    let m = part_materials(part, char_mats);
    if let Some(mut fm) = fade_mats {
        fm.cutout = m.steady.clone();
        if let Some(blend) = m.fade_blend {
            fm.blend = blend.clone();
        }
        fm.bake_blend = m.bake_blend.cloned();
    }
    if let Some(mut p) = portrait {
        p.material = m.steady.clone();
    }
    let want = match lit {
        Some(mut lit) => lit.repoint(m.steady, m.bake).clone(),
        None => m.steady.clone(),
    };
    if let Some(mut mat) = mat {
        if !fading && mat.0 != want {
            mat.0 = want;
        }
    }
}

/// The armed idle's authored CAaBox — the mouseover picker's volume for a skinned part (decision
/// 0637). Same read as the first build's; see the note there for why the bind box is not a fair
/// stand-in.
fn idle_aabb(dm: &super::super::DisplayModel) -> Option<bevy::camera::primitives::Aabb> {
    let anims = dm.animations.as_ref()?;
    let clip = anims.first_seq.and_then(|i| anims.clips.get(i))?;
    (clip.bounds_max.cmpgt(clip.bounds_min).all())
        .then(|| bevy::camera::primitives::Aabb::from_min_max(clip.bounds_min, clip.bounds_max))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use benilla_formats::{CharacterGeosets, ItemDisplay, ItemDisplayCatalog, NpcAppearance};

    use super::super::super::display::{empty_display, EntityPart};
    use super::super::super::{BoneAttach, Creatures};
    use super::*;

    /// One synthetic body batch at `geoset`.
    fn part(geoset: u16) -> EntityPart {
        // A DISTINCT material per stub batch: every real batch of a model has its own built
        // material unless it shares a texture, and a fixture whose batches all bound the one
        // default handle would merge into a single group (`merge`) and stop being the
        // per-batch hide/show/re-point walk these tests are about.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        EntityPart {
            mesh: Handle::default(),
            geometry: std::sync::Arc::new(benilla_formats::RenderSubmesh::default()),
            aabb: None,
            skinned_mesh: None,
            welded_billboard: false,
            material: Handle::from(bevy::asset::uuid::Uuid::from_u128(
                0xd0d0_0000 + u128::from(n),
            )),
            material_interior: None,
            material_interior_bake: None,
            material_interior_bake_blend: None,
            fade_blend: None,
            zfill: None,
            blend: benilla_formats::ModelBlend::Opaque,
            additive: false,
            two_sided: false,
            geoset_id: geoset,
            char_slot: None,
            billboard: None,
            alpha_anim: None,
            rgb_anim: None,
            rgb_seq: None,
            uv_anim: None,
            uv_seq: None,
            uv_rot_seq: None,
            uv_scale_seq: None,
            ground_quad: None,
        }
    }

    /// A player standing fully dressed: the display model behind it, the resource set the re-dress
    /// reads, and — the point of the whole change — a bone anchor with a held item hanging off it,
    /// exactly where a weapon and its enchant glow live.
    struct Standing {
        app: App,
        player: Entity,
        joint: Entity,
        held: Entity,
    }

    /// Build one. `geosets` are the model's batches in order; `showing` the batch indices already
    /// spawned as children (what the first build selected); `characters` the geoset tables (absent ⇒
    /// no filter at all, the druid-form / beast-display arm).
    fn stand(geosets: &[u16], showing: &[usize], characters: Option<CharacterGeosets>) -> Standing {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<WowModelMaterial>()
            .init_resource::<SkinComposites>()
            .init_resource::<MergedFormsCache>()
            // The engine's material cache — normally `model_render::plugin`'s, which this bare
            // harness does not install.
            .init_resource::<benilla_world::model_render::ModelMaterials>()
            // The animated-material lane the dressing path now takes (2295) — never exercised by
            // a re-dress, which is a player's gear change, but the system asks for it.
            .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
            .init_resource::<benilla_world::doodad_anim::TintAnimMaterials>()
            .init_resource::<benilla_world::mat_anim_table::MatAnimTable>();

        let mut dm = empty_display();
        dm.parts = Some(geosets.iter().map(|g| part(*g)).collect());
        // The look comes off the DISPLAY here (a character-model NPC row), which is the same
        // `CharLook` the wire path builds — and needs no `ObjectStore` to stand one up.
        dm.npc_appearance = Some(NpcAppearance {
            race: 1,
            sex: 0,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
            equipment: [0; 10],
            bake_name: None,
        });
        app.insert_resource(Creatures {
            catalog: Default::default(),
            models: HashMap::from([(42u32, dm)]),
        });
        // Display 7 = a pair of gloves whose `geosetGroup[0]` is 1 (→ geoset 402, the glove group's
        // own range disabled).
        app.insert_resource(ItemDisplays::icons_for_tests(
            ItemDisplayCatalog::from_displays(HashMap::from([(
                7u32,
                ItemDisplay {
                    geoset_groups: [1, 0, 0],
                    ..Default::default()
                },
            )])),
        ));
        if let Some(cg) = characters {
            app.insert_resource(Characters(cg));
        }

        let player = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Player,
                    display_id: Some(42),
                    scale: 1.0,
                },
                Equipment {
                    settled: true,
                    ..Default::default()
                },
                AppliedEquipment(Equipment {
                    settled: true,
                    ..Default::default()
                }),
                VisualAttached,
                Transform::default(),
                Visibility::default(),
            ))
            .id();
        // The rig's bone anchor and the item hanging off it — the weapon root a gear change used to
        // take with it.
        let joint = app
            .world_mut()
            .spawn((Transform::default(), Visibility::default(), ChildOf(player)))
            .id();
        let held = app
            .world_mut()
            .spawn((Transform::default(), Visibility::default(), ChildOf(joint)))
            .id();
        app.world_mut().entity_mut(player).insert(BoneAttach {
            points: HashMap::new(),
            markers: HashMap::new(),
        });
        for &i in showing {
            app.world_mut().spawn((
                Transform::default(),
                Visibility::default(),
                ChildOf(player),
                MeshMaterial3d(Handle::<WowModelMaterial>::default()),
                DressedPart {
                    index: i as u32,
                    card: None,
                },
            ));
        }
        app.add_systems(Update, redress_player_looks);
        app.update();
        Standing {
            app,
            player,
            joint,
            held,
        }
    }

    impl Standing {
        /// Swap the player's gear and run one re-dress pass.
        fn wear(&mut self, gear: Equipment) {
            *self
                .app
                .world_mut()
                .get_mut::<Equipment>(self.player)
                .unwrap() = gear;
            self.app.update();
        }

        /// The batch indices currently standing under the player.
        fn showing(&mut self) -> Vec<u32> {
            let mut out: Vec<u32> = self
                .app
                .world_mut()
                .query::<&DressedPart>()
                .iter(self.app.world())
                .map(|d| d.index)
                .collect();
            out.sort_unstable();
            out
        }
    }

    /// **The director's report.** A gear change used to `despawn_related::<Children>()` the whole
    /// visual, taking both weapons, their enchant glows, the helm, the shoulders and every aura
    /// visual with it — everything hanging off a bone was destroyed and re-created because a belt
    /// changed. Nothing on the unit but its own body batches may be touched.
    #[test]
    fn a_gear_change_leaves_the_rig_and_every_attachment_standing() {
        let mut s = stand(&[0, 401], &[0, 1], None);
        s.wear(Equipment {
            cloak: 9,
            settled: true,
            ..Default::default()
        });
        let w = s.app.world();
        assert!(w.get_entity(s.joint).is_ok(), "the bone anchor survives");
        assert!(w.get_entity(s.held).is_ok(), "the held item survives");
        assert!(
            w.get::<BoneAttach>(s.player).is_some(),
            "the attach table survives",
        );
        assert_eq!(
            s.showing(),
            vec![0, 1],
            "the body batches are the same ones"
        );
    }

    /// …and the diff is stamped, so the re-dress fires **once** per change rather than every frame
    /// (the composite behind it is a synchronous BLP read on a miss).
    #[test]
    fn a_gear_change_restamps_what_the_visual_is_dressed_with() {
        let mut s = stand(&[0], &[0], None);
        let gear = Equipment {
            cloak: 9,
            settled: true,
            ..Default::default()
        };
        s.wear(gear);
        assert_eq!(
            s.app.world().get::<AppliedEquipment>(s.player).unwrap().0,
            gear,
        );
    }

    /// The **geoset half**, against the shipped customization tables: putting gloves on replaces the
    /// glove group — the naked 401 batch stops drawing and the item's 402 batch starts — and that
    /// now happens by despawning/spawning exactly those two batches instead of rebuilding the unit.
    /// (The reference flips two entries of the model's own visibility array; the visible result is
    /// the same set, which is what this pins.)
    #[test]
    fn worn_gloves_replace_the_glove_geoset_in_place() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cg = CharacterGeosets::load(&mut chain).expect("customization tables");

        let mut s = stand(&[0, 401, 402], &[0, 1], Some(cg));
        assert_eq!(s.showing(), vec![0, 1], "bare-handed to start");
        let mut gear = Equipment {
            settled: true,
            ..Default::default()
        };
        gear.bodyslots[6] = 7; // the gloves slot
        s.wear(gear);
        assert_eq!(
            s.showing(),
            vec![0, 2],
            "the glove batch replaced the bare-hand one",
        );
        assert!(
            s.app.world().get_entity(s.held).is_ok(),
            "and the held item never moved",
        );
    }
}
