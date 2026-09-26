//! Re-dressing a character in place when its worn equipment changes. The reference re-dresses the
//! same `CM2Model`: its compositor re-blits the atlas and re-runs the geoset selection
//! (`0x477520`), which writes only the model's visibility array (`+0x98`, through `0x7110d0`), and
//! never touches the attachments (`0x712f70`). So a re-dress re-points the standing parts at the
//! new materials, despawns the batches the gear hides, spawns the ones it reveals, and leaves the
//! rig and everything hanging off it alone.

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

/// What a re-dress writes on a part: its material and its interior, fade and portrait records;
/// all optional, since a billboard anchor carries a [`DressedPart`] and none of them.
type PartWrites<'a> = (
    Option<Mut<'a, MeshMaterial3d<WowModelMaterial>>>,
    Option<Mut<'a, InteriorLit>>,
    Option<Mut<'a, FadeMaterials>>,
    Option<Mut<'a, PortraitPart>>,
);

/// Re-dress every player whose worn equipment changed. A display change is a different model and
/// stays a teardown (`refresh_live_display`).
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
            // A revealed billboard batch's card bone resolves its anchor here.
            Option<&mut benilla_world::rig_anim::RigPose>,
            Option<&benilla_world::interior::BodyBakeCenter>,
            Option<&benilla_world::model_fade::UnitAppearFade>,
        ),
        With<VisualAttached>,
    >,
    // The standing parts, found again by their batch index.
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
    // The skin build chain, nested for Bevy's system-param limit.
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
    // The own-material lane `spawn_part` takes; no character batch in the shipped data uses it.
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
        // Wait until every visible item has resolved, or a half-dressed atlas composites first.
        if net.kind != EntityKind::Player || !live.settled || *live == applied.0 {
            continue;
        }
        // Stamp first: a model-less player has nothing to re-dress and must not retry every frame.
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

        let worn = resolve_worn_equip(net, Some(live), Some(dm));
        let look = resolve_char_look(net, Some(dm), entity, &stores);
        let eg = equip_geosets(
            displays.as_deref(),
            &worn.bodyslots,
            worn.cloak,
            worn.helm,
            worn.tabard_preview,
        );
        let visible = look.as_ref().and_then(|l| {
            let cg = characters.as_deref()?;
            Some(cg.0.visible_geosets(l.race, l.sex, l.hair_style, l.facial_hair, &eg))
        });
        // Cached per (appearance, worn set), so a swap back costs a lookup.
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
        // No look (a druid form on a beast display) means no geoset filter, as at build.
        let shows = |geoset: u16| visible.as_ref().is_none_or(|v| v.contains(&geoset));
        // A standing group survives only if the new grouping keeps its member set; keyed by the
        // first member, the standing entity's `DressedPart::index`.
        let groups = merge::guard_groups(
            merge::group_parts(parts, |p| shows(p.geoset_id)),
            parts,
            &char_mats,
        );
        let group_of: std::collections::HashMap<u32, &merge::BodyGroup> =
            groups.iter().map(|g| (g.first(), g)).collect();

        // Pass 1, the standing parts: despawn if hidden, else re-point.
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
                continue; // a stale index: the teardown handles it
            };
            let standing_members: &[u32] = match group {
                Some(g) => &g.0,
                None => std::slice::from_ref(&dp.index),
            };
            let still_grouped = group_of
                .get(&dp.index)
                .is_some_and(|g| g.members == standing_members);
            if !shows(part.geoset_id) || !still_grouped {
                // A billboard card is a world root, so it is reaped by name.
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
            // A billboard batch is never a character slot, so it has nothing to re-point.
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

        // Pass 2, the revealed batches, which join the unit's appear-fade if one is in flight (the
        // login gear cascade lands mid-ramp). Their card bones resolve anchors first.
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
        // `atlas` must change when the worn set changes a body region.
        info!(
            "redress: {entity} — {hidden} batch(es) hidden, {shown} shown, {repointed} re-pointed, \
             atlas {:?}",
            char_mats.0.as_ref().map(|(single, _)| single.0.id()),
        );
    }
}

/// Re-point one standing part at its unit's new materials. The records always update; the
/// displayed material only when no fade ramp owns it, since `apply_render_fade` re-resolves a
/// ramping part from the records and a steady write would flash it opaque for a frame.
fn repoint_part(
    part: &EntityPart,
    char_mats: &CharSkinMaterials,
    (mat, lit, fade_mats, portrait): PartWrites,
    fading: bool,
) {
    // Gear changes only the character slots' textures.
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

/// The armed idle's authored CAaBox, the picker's volume for a skinned part, read as at build.
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
        // A distinct material per batch, or the fixture would merge into one group (`merge`).
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

    /// A dressed player with a bone anchor and a held item hanging off it.
    struct Standing {
        app: App,
        player: Entity,
        joint: Entity,
        held: Entity,
    }

    /// `geosets` are the model's batches, `showing` the indices already spawned, `characters` the
    /// geoset tables (absent: no filter).
    fn stand(geosets: &[u16], showing: &[usize], characters: Option<CharacterGeosets>) -> Standing {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<WowModelMaterial>()
            .init_resource::<SkinComposites>()
            .init_resource::<MergedFormsCache>()
            // Normally `model_render::plugin`'s.
            .init_resource::<benilla_world::model_render::ModelMaterials>()
            // The own-material lane the system takes, which a re-dress never uses.
            .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
            .init_resource::<benilla_world::doodad_anim::TintAnimMaterials>()
            .init_resource::<benilla_world::mat_anim_table::MatAnimTable>();

        let mut dm = empty_display();
        dm.parts = Some(geosets.iter().map(|g| part(*g)).collect());
        // A look off the display (an NPC row) needs no `ObjectStore`.
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
        // Display 7: gloves whose `geosetGroup[0]` is 1, selecting geoset 402.
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
        // A bone anchor and the item hanging off it.
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

    /// A gear change touches nothing on the unit but its own body batches.
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

    /// The re-dress fires once per change: the composite behind it reads BLPs synchronously.
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

    /// Against the shipped tables: gloves hide the bare-hand 401 batch and show the item's 402, by
    /// despawning and spawning just those two, the reference's two visibility-array flips.
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
