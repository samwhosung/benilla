//! The character preview behind every booth that shows a character nobody stands in: the glue
//! screens' select and create bodies ([`build_glue_preview`]) and the dressing room
//! ([`build_dressup_preview`]), whose try-ons nobody in the world wears. Each reduces its look to
//! a [`PreviewSpec`], and [`assemble`] builds it the world's way: the display cache, the geoset
//! filter, the composited body, the cloak, and the helm, shoulders and held weapons as riders
//! (the select build `0x472950` holds weapons in the hands, `0x47a0c0`).

use benilla_formats::CharSkinSlot;
use benilla_protocol::{
    CharEnumItem, CHARACTER_FLAG_GHOST, CHARACTER_FLAG_HIDE_CLOAK, CHARACTER_FLAG_HIDE_HELM,
};
use bevy::prelude::*;

use crate::portrait::{
    BoothTwins, DressUpBake, DressUpLook, DressUpPreview, GhostKit, GlueLook, GluePetBake,
    GluePreview, GluePreviewBake, PetLook, PreviewBillboard, PreviewEffects, PreviewPart,
    PreviewRider,
};
use benilla_assets::materials::WowModelMaterial;
use benilla_assets::WorldAssets;

use super::super::equipment::{attach_id, ensure_item_model, placement, ItemModelKind};
use super::super::item_glow::{self, ItemGlows};
use super::super::{
    CharCreate, Characters, Creatures, EntityPart, ItemDisplays, SkinComposites, SkinSections,
};
use super::char_skin::{equip_geosets, BodySkin, CharLook};

/// The equipment slots of shirt, chest, belt, pants, boots, wrist, gloves and tabard, the armor
/// composite's bodyslots 2 to 9.
const ENUM_BODYSLOTS: [usize; 8] = [3, 4, 5, 6, 7, 8, 9, 18];
/// The equipment slots of the helm, shoulders and cloak.
const ENUM_HELM: usize = 0;
const ENUM_SHOULDER: usize = 2;
const ENUM_CLOAK: usize = 14;
/// Main hand, off hand, ranged: [`placement`]'s held slots 0, 1, 2.
const ENUM_HELD: [usize; 3] = [15, 16, 17];

/// An item's `InventoryType` as its `EQUIPMENT_SLOT_*` index, so a CharStartOutfit item lands
/// where a select record carries it (the compositor's render slots, `0x478cb0`); `None` for a
/// type that never renders.
pub(crate) fn equip_slot(inv_type: u8) -> Option<usize> {
    Some(match inv_type {
        1 => 0,             // HEAD
        2 => 1,             // NECK (not rendered; its slot for completeness)
        3 => 2,             // SHOULDERS
        4 => 3,             // BODY (shirt)
        5 | 20 => 4,        // CHEST / ROBE
        6 => 5,             // WAIST
        7 => 6,             // LEGS
        8 => 7,             // FEET
        9 => 8,             // WRISTS
        10 => 9,            // HANDS
        16 => 14,           // BACK (cloak)
        13 | 17 | 21 => 15, // WEAPON / TWOHAND / WEAPONMAINHAND → main hand
        14 | 22 | 23 => 16, // SHIELD / WEAPONOFFHAND / HOLDABLE → off hand
        15 | 25 | 26 => 17, // RANGED / THROWN / RANGEDRIGHT (wand, gun) → ranged
        19 => 18,           // TABARD
        _ => return None, // 0 NON_EQUIP · 11 FINGER · 12 TRINKET · 18 BAG · 24 AMMO · 28 RELIC · …
    })
}

/// One preview's input: the body display, its appearance, and the worn equipment in the
/// `SMSG_CHAR_ENUM` slot shape.
pub(in crate::entities) struct PreviewSpec {
    /// The race and sex body for a glue look, the player's own display for the dressing room.
    pub(in crate::entities) display_id: u32,
    pub(in crate::entities) race: u8,
    pub(in crate::entities) sex: u8,
    pub(in crate::entities) skin: u8,
    pub(in crate::entities) face: u8,
    pub(in crate::entities) hair_style: u8,
    pub(in crate::entities) hair_color: u8,
    pub(in crate::entities) facial_hair: u8,
    /// Worn ItemDisplayInfo ids by `EQUIPMENT_SLOT_*`.
    pub(in crate::entities) equipment: [CharEnumItem; 19],
    pub(in crate::entities) emblem: Option<benilla_formats::GuildEmblem>,
    /// `CHARACTER_FLAG_*` bits; only hide-helm and hide-cloak are read.
    pub(in crate::entities) flags: u32,
    /// `false` for the select mannequin, whose equipment loop skips the ranged slot (`0x472950`,
    /// `0x472bfe`); `true` for the dressing room (`0x504d90`), whose remap (`0x504b7c` to
    /// `0x504b44`) and literal sheath 0 (`0x504adc`) put a tried-on ranged weapon in a hand
    /// whatever its SheatheType, as [`placement`]'s ranged arm does: a bow left, the rest right.
    pub(in crate::entities) ranged_in_hand: bool,
}

/// What [`assemble`] produced: a preview bake's content, in the booth's shape.
pub(in crate::entities) struct Assembled {
    pub(in crate::entities) parts: Vec<PreviewPart>,
    pub(in crate::entities) riders: Vec<PreviewRider>,
    pub(in crate::entities) effects: Vec<PreviewEffects>,
    pub(in crate::entities) billboards: Vec<PreviewBillboard>,
    pub(in crate::entities) grip: [bool; 2],
}

/// The resources [`assemble`] borrows from its caller's system params.
pub(in crate::entities) struct PreviewCtx<'a, 'w> {
    pub(in crate::entities) creatures: &'a Creatures,
    pub(in crate::entities) characters: Option<&'a Characters>,
    pub(in crate::entities) displays: Option<&'a mut ItemDisplays>,
    /// The glow chain, resolved here: the world's `resolve_equipment` never runs for a look
    /// nobody wears.
    pub(in crate::entities) glows: Option<&'a mut ItemGlows>,
    pub(in crate::entities) sections: Option<&'a SkinSections>,
    pub(in crate::entities) world_assets: Option<&'a WorldAssets>,
    pub(in crate::entities) images: &'a mut Assets<Image>,
    pub(in crate::entities) skin_composites: &'a mut SkinComposites,
    pub(in crate::entities) asset_server: &'a AssetServer,
    pub(in crate::entities) mats: &'a mut benilla_world::model_render::M2BatchMaterials<'w>,
}

/// Assemble the glue-screen preview when its look changes, after `update_display_models`; until
/// the models build it leaves the bake untouched and retries.
pub(in crate::entities) fn build_glue_preview(
    preview: Res<GluePreview>,
    mut bake: ResMut<GluePreviewBake>,
    creatures: Option<Res<Creatures>>,
    characters: Option<Res<Characters>>,
    char_create: Option<Res<CharCreate>>,
    mut displays: Option<ResMut<ItemDisplays>>,
    mut glows: Option<ResMut<ItemGlows>>,
    sections: Option<Res<SkinSections>>,
    world_assets: Option<Res<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut skin_composites: ResMut<SkinComposites>,
    asset_server: Res<AssetServer>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    // The ghost kit (`0x47280f`): its `SpellVisualKit` row, and the world's spell-effect model
    // cache its models load through.
    visuals: Option<Res<crate::creature_anim::SpellVisuals>>,
    mut fx: Option<ResMut<super::super::spell_fx::SpellFx>>,
    mut state: Local<PreviewState>,
) {
    if state.last_look != preview.look {
        state.last_look = preview.look;
        state.built = false;
    }
    if state.built {
        return;
    }

    let Some(look) = preview.look else {
        if bake.look.is_some() {
            *bake = GluePreviewBake {
                revision: bake.revision + 1,
                ..default()
            };
        }
        state.built = true;
        return;
    };

    let (Some(creatures), Some(char_create)) = (creatures.as_deref(), char_create.as_deref())
    else {
        return;
    };
    let (race, sex) = look.body();
    let Some(display_id) = char_create.0.body_display(race, sex) else {
        state.built = true; // a non-playable race: nothing to show, stop retrying
        return;
    };

    let (skin, face, hair_style, hair_color, facial_hair) = match look {
        GlueLook::Create(l) => (l.skin, l.face, l.hair_style, l.hair_color, l.facial_hair),
        GlueLook::Select(l) => (l.skin, l.face, l.hair_style, l.hair_color, l.facial_hair),
    };

    // A select look carries its roster record; a create look wears its (race, class, sex)
    // CharStartOutfit, which has no helm or cloak to hide, so its flags are 0.
    let (equipment, flags): ([CharEnumItem; 19], u32) = match look {
        GlueLook::Create(l) => {
            let mut equipment = [CharEnumItem::default(); 19];
            for item in char_create.0.start_outfit(l.race, l.class, l.sex) {
                if let Some(slot) = equip_slot(item.inv_type) {
                    equipment[slot] = CharEnumItem {
                        display_id: item.display_id,
                        inventory_type: item.inv_type,
                    };
                }
            }
            (equipment, 0)
        }
        GlueLook::Select(l) => (l.equipment, l.flags),
    };

    let spec = PreviewSpec {
        display_id,
        race,
        sex,
        skin,
        face,
        hair_style,
        hair_color,
        facial_hair,
        equipment,
        // No crest: `SMSG_CHAR_ENUM` has no emblem and there is no session to query, so a guild
        // tabard wears its own art, as with the reference's empty guild cache.
        emblem: None,
        flags,
        // The select build skips the ranged slot (`0x472bfe`).
        ranged_in_hand: false,
    };
    let Some(a) = assemble(
        &spec,
        &mut PreviewCtx {
            creatures,
            characters: characters.as_deref(),
            displays: displays.as_deref_mut(),
            glows: glows.as_deref_mut(),
            sections: sections.as_deref(),
            world_assets: world_assets.as_deref(),
            images: &mut images,
            skin_composites: &mut skin_composites,
            asset_server: &asset_server,
            mats: &mut mats,
        },
    ) else {
        return; // a model is still loading: retry next frame
    };

    // The ghost kit, for a select record with the ghost bit (`CHARSELECT+0xfc & 0x2000`).
    let mut a = a;
    let ghost = match look {
        GlueLook::Select(l) if l.flags & CHARACTER_FLAG_GHOST != 0 => {
            let mut pending = false;
            let arm = (|| {
                let body = creatures.models.get(&display_id)?;
                glue_ghost_arm(
                    body,
                    &visuals.as_deref()?.0,
                    fx.as_deref_mut()?,
                    &asset_server,
                    &mut pending,
                )
            })();
            if pending {
                return; // an effect model is still loading: retry
            }
            arm.map(|arm| {
                a.riders.extend(arm.riders);
                a.billboards.extend(arm.billboards);
                a.effects.extend(arm.effects);
                arm.kit
            })
        }
        _ => None,
    };

    debug!(
        "glue preview: race {race} sex {sex} display {display_id} → {} parts, {} riders, \
         {} camera-facing, {} effect model(s) / {} emitter(s) ({})",
        a.parts.len(),
        a.riders.len(),
        a.billboards.len(),
        a.effects.len(),
        a.effects
            .iter()
            .map(|e: &PreviewEffects| e.emitters.len())
            .sum::<usize>(),
        match look {
            GlueLook::Create(_) => "create",
            GlueLook::Select(_) => "select",
        },
    );
    *bake = GluePreviewBake {
        look: Some(look),
        display_id,
        parts: a.parts,
        riders: a.riders,
        effects: a.effects,
        billboards: a.billboards,
        grip: a.grip,
        ghost,
        revision: bake.revision + 1,
    };
    state.built = true;
}

/// Assemble the dressing room's look, whose equipment [`crate::ui_dressup`] composed from the
/// player's worn items and the try-ons; retries while an item model loads.
pub(in crate::entities) fn build_dressup_preview(
    preview: Res<DressUpPreview>,
    mut bake: ResMut<DressUpBake>,
    creatures: Option<Res<Creatures>>,
    characters: Option<Res<Characters>>,
    mut displays: Option<ResMut<ItemDisplays>>,
    mut glows: Option<ResMut<ItemGlows>>,
    sections: Option<Res<SkinSections>>,
    world_assets: Option<Res<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut skin_composites: ResMut<SkinComposites>,
    asset_server: Res<AssetServer>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    mut state: Local<DressUpState>,
) {
    if state.last_look != preview.look {
        state.last_look = preview.look;
        state.built = false;
    }
    if state.built {
        return;
    }

    let Some(look) = preview.look else {
        if bake.look.is_some() {
            *bake = DressUpBake {
                revision: bake.revision + 1,
                ..default()
            };
        }
        state.built = true;
        return;
    };

    let Some(creatures) = creatures.as_deref() else {
        return;
    };
    let spec = PreviewSpec {
        display_id: look.display_id,
        race: look.race,
        sex: look.sex,
        skin: look.skin,
        face: look.face,
        hair_style: look.hair_style,
        hair_color: look.hair_color,
        facial_hair: look.facial_hair,
        equipment: look.equipment,
        emblem: look.emblem,
        // No hide flags: `crate::ui_dressup` already left out a hidden worn helm or cloak, as
        // `SetUnit` (`0x505d70`) clones the live display pointers (`0x476cb0`), and a mask here
        // would hide a tried-on one.
        flags: 0,
        // The ranged slot here only holds a try-on: `SetUnit` clones the live model, which shows
        // a ranged weapon only while ranged-drawn, so `crate::ui_dressup` leaves a worn one out.
        ranged_in_hand: true,
    };
    let Some(a) = assemble(
        &spec,
        &mut PreviewCtx {
            creatures,
            characters: characters.as_deref(),
            displays: displays.as_deref_mut(),
            glows: glows.as_deref_mut(),
            sections: sections.as_deref(),
            world_assets: world_assets.as_deref(),
            images: &mut images,
            skin_composites: &mut skin_composites,
            asset_server: &asset_server,
            mats: &mut mats,
        },
    ) else {
        return; // an item model is still loading: retry next frame
    };

    debug!(
        "dressup preview: race {} sex {} display {} → {} parts, {} riders, {} camera-facing, \
         {} effect model(s)",
        look.race,
        look.sex,
        look.display_id,
        a.parts.len(),
        a.riders.len(),
        a.billboards.len(),
        a.effects.len(),
    );
    *bake = DressUpBake {
        look: Some(look),
        display_id: look.display_id,
        parts: a.parts,
        riders: a.riders,
        effects: a.effects,
        billboards: a.billboards,
        grip: a.grip,
        revision: bake.revision + 1,
    };
    state.built = true;
}

/// The character-select ghost's `SpellVisualKit` row, hard-coded as in the reference: `0x47280f`
/// loads `idmap[989]` behind the bound check at `0x4727f6`. It is `SpellVisual` 886's state kit,
/// spell 8326 Ghost's, which a released ghost in the world rides too (`0x5ff350`).
const GLUE_GHOST_KIT: u32 = 989;

/// The ghost kit's share of a select bake: the body's tint and alpha, and its effect models as
/// booth seats.
struct GhostArm {
    kit: GhostKit,
    riders: Vec<PreviewRider>,
    billboards: Vec<PreviewBillboard>,
    effects: Vec<PreviewEffects>,
}

/// Resolve [`GLUE_GHOST_KIT`] against the kit catalog and the body's attachments; `pending` is set
/// while an effect model loads. The tint and alpha decode through
/// [`crate::aura_visual::node_for`], the reference's `0x60dbfc` table, as the world aura does, but
/// land instantly (`0x4727f0` writes them through `0x710cb0`/`0x710cf0`), with no 1000 ms
/// `StartAlphaFade`.
fn glue_ghost_arm(
    body: &super::super::DisplayModel,
    visuals: &benilla_formats::SpellVisualCatalog,
    fx: &mut super::super::spell_fx::SpellFx,
    asset_server: &AssetServer,
    pending: &mut bool,
) -> Option<GhostArm> {
    let kit_row = visuals.kit(GLUE_GHOST_KIT)?;
    let (mut tint, mut alpha) = (None, None);
    for node in kit_row
        .char_procs()
        .filter_map(crate::aura_visual::node_for)
    {
        match node {
            crate::aura_visual::AuraNode::Tint(rgb) => tint = Some(rgb),
            crate::aura_visual::AuraNode::Alpha(a) => alpha = Some(a),
            // Row 989 carries only these two; other proc types have no select-screen mechanism.
            _ => {}
        }
    }
    let kit = GhostKit {
        tint: tint?,
        alpha: alpha.unwrap_or(1.0),
    };

    // The kit's effect models at the body's attachment points. The shipped row has only
    // `Spells\Ghost_state.mdx` at Base (`0x13`), emitters only, but the row is data.
    let (mut riders, mut billboards, mut effects) = (Vec::new(), Vec::new(), Vec::new());
    for (tag, effect_id) in kit_row.effects() {
        // The select build hangs models on the character alone (`0x472780` to `0x712f70`), so a
        // world-planted effect has nowhere to go.
        if tag == benilla_formats::WORLD_EFFECT_TAG {
            continue;
        }
        let Some(path) = visuals.effect_path(effect_id) else {
            continue;
        };
        let path = path.to_string();
        // `0x472780` rejects a `-1` attach id (`jl 0x4727dd`); a body without the point hangs
        // nothing.
        let Some(point) = body.attachments.iter().find(|a| a.id == tag) else {
            continue;
        };
        let dm =
            fx.models
                .entry(path.clone())
                .or_insert_with(|| super::super::display::DisplayModel {
                    handle: super::super::display::ModelHandle::M2(
                        asset_server.load(benilla_assets::m2_url(&path)),
                    ),
                    ..super::super::display::empty_shell()
                });
        let Some(parts) = dm.parts.as_deref() else {
            *pending = true; // still loading: the caller retries
            continue;
        };
        for part in parts {
            match &part.billboard {
                Some(info) => billboards.push(PreviewBillboard {
                    mesh: part.mesh.clone(),
                    material: part.material.clone(),
                    bone: point.bone,
                    offset: point.offset + info.pivot,
                    kind: info.kind,
                    twins: BoothTwins {
                        blend: part.fade_blend.clone(),
                        zfill: part.zfill.clone(),
                    },
                }),
                None => riders.push(PreviewRider {
                    mesh: part.mesh.clone(),
                    material: part.material.clone(),
                    bone: point.bone,
                    offset: point.offset,
                    twins: BoothTwins {
                        blend: part.fade_blend.clone(),
                        zfill: part.zfill.clone(),
                    },
                }),
            }
        }
        if !dm.emitters.is_empty() {
            effects.push(PreviewEffects {
                bone: point.bone,
                offset: point.offset,
                emitters: dm.emitters.clone(),
            });
        }
    }
    Some(GhostArm {
        kit,
        riders,
        billboards,
        effects,
    })
}

fn assemble(spec: &PreviewSpec, ctx: &mut PreviewCtx<'_, '_>) -> Option<Assembled> {
    let PreviewSpec {
        display_id,
        race,
        sex,
        equipment,
        flags,
        ..
    } = *spec;
    let dm = ctx.creatures.models.get(&display_id)?; // model not built yet: retry next frame
    let parts = dm.parts.as_deref()?; // asset still loading

    let char_look = CharLook {
        race,
        sex,
        skin: spec.skin,
        hair_style: spec.hair_style,
        hair_color: spec.hair_color,
        facial_hair: spec.facial_hair,
        body: BodySkin::Composite { face: spec.face },
    };
    let bodyslots = ENUM_BODYSLOTS.map(|slot| equipment[slot].display_id);
    let cloak = if flags & CHARACTER_FLAG_HIDE_CLOAK != 0 {
        0
    } else {
        equipment[ENUM_CLOAK].display_id
    };
    let helm = if flags & CHARACTER_FLAG_HIDE_HELM != 0 {
        0
    } else {
        equipment[ENUM_HELM].display_id
    };
    let mut held = held_wants(&equipment, helm, race, sex, spec.ranged_in_hand);
    // Grip `[right, left]`: a hand whose attach point holds something closes (`0x5059a0`); a
    // shield sits on the forearm point, so its hand stays open.
    let grip = [
        held.iter().any(|w| w.attach == attach_id::HAND_RIGHT),
        held.iter().any(|w| w.attach == attach_id::HAND_LEFT),
    ];

    // Every rider model must be ready before the bake, or a geared character pops in piecewise.
    if let Some(d) = ctx.displays.as_deref_mut() {
        for w in &held {
            ensure_item_model(d, w.display, w.kind, ctx.asset_server);
        }
        // Glows for held weapons and shields only: the helm and shoulder attaches pass a literal
        // 0 visual (`crate::entities::item_glow`).
        if let Some(g) = ctx.glows.as_deref_mut() {
            for w in held
                .iter_mut()
                .filter(|w| matches!(w.kind, ItemModelKind::Weapon | ItemModelKind::Shield))
            {
                w.visual = d.catalog.get(w.display).map_or(0, |c| c.item_visual);
                item_glow::ensure_glow_models(g, w.visual, ctx.asset_server);
            }
        }
        if !held.iter().all(|w| {
            d.models
                .get(&(w.display, w.kind))
                .and_then(|m| m.parts.as_ref())
                .is_some()
        }) {
            return None; // an item model is still loading: retry next frame
        }
        // The same gate for the glow models.
        if let Some(g) = ctx.glows.as_deref() {
            let pending = held
                .iter()
                .filter_map(|w| g.effects(w.visual))
                .flat_map(|paths| paths.iter().flatten())
                .any(|p| g.models.get(p).is_none_or(|m| m.parts.is_none()));
            if pending {
                return None;
            }
        }
    }

    // The world path's geoset selection, the helm's hide-mask rows included (`0x4799a0`).
    let eg = equip_geosets(ctx.displays.as_deref(), &bodyslots, cloak, helm, false);
    let visible = ctx.characters.map(|c| {
        c.0.visible_geosets(race, sex, char_look.hair_style, char_look.facial_hair, &eg)
    });
    let char_mats = super::char_skin::build_char_skin_materials(
        &char_look,
        bodyslots,
        cloak,
        spec.emblem,
        false,
        ctx.displays.as_deref(),
        ctx.sections,
        ctx.world_assets,
        parts,
        ctx.images,
        &mut ctx.skin_composites.0,
        ctx.asset_server,
        ctx.mats,
    );

    let shows = |p: &EntityPart| {
        visible
            .as_ref()
            .is_none_or(|vis| vis.contains(&p.geoset_id))
    };
    let preview_parts: Vec<PreviewPart> = parts
        .iter()
        .filter(|p| p.billboard.is_none() && shows(p))
        .map(|p| PreviewPart {
            static_mesh: p.mesh.clone(),
            skinned_mesh: p.skinned_mesh.clone(),
            material: steady_material(p, &char_mats).unwrap_or_else(|| p.material.clone()),
            twins: part_twins(p, &char_mats),
            // Not carried yet: a character batch's authored dimming constant previews at 1.0.
            alpha_anim: None,
        })
        .collect();

    // The body's billboard batches (the undead and night-elf eye glow), which the booth faces to
    // its own camera; geoset-gated like the body (the undead glow is geoset 302). Unlit, so they
    // keep their built material; offset zero, since the rigged body's joint bakes the pivot.
    let mut preview_billboards: Vec<PreviewBillboard> = parts
        .iter()
        .filter(|p| shows(p))
        .filter_map(|p| {
            let info = p.billboard.as_ref()?;
            Some(PreviewBillboard {
                mesh: p.mesh.clone(),
                material: p.material.clone(),
                bone: info.bone,
                offset: Vec3::ZERO,
                kind: info.kind,
                twins: BoothTwins {
                    blend: p.fade_blend.clone(),
                    zfill: p.zfill.clone(),
                },
            })
        })
        .collect();

    // Each want seats at the body's attach point: its meshes, billboard batches and emitters, and
    // for a held weapon the same off each glow model its `ItemVisuals` id names, reached as in the
    // world (`0x472c91` to `0x47a0c0` hands `ItemDisplayInfo+0x58` to `0x4798c0`). The enum has
    // no enchant ids (vmangos `Player.cpp:1724`), so no enchant glow shows here.
    let attach_point = |id: u16| dm.attachments.iter().find(|a| a.id == id);
    let mut riders = Vec::new();
    let mut preview_effects = Vec::new();
    if let Some(d) = ctx.displays.as_deref() {
        for w in &held {
            let Some(point) = attach_point(w.attach) else {
                continue; // no such attach point: hold nothing, as in the world
            };
            let Some(item) = d.models.get(&(w.display, w.kind)) else {
                continue; // gated above
            };
            let Some(item_parts) = item.parts.as_deref() else {
                continue;
            };
            for p in item_parts.iter() {
                // A billboard batch rides as a card at the attach point plus its own pivot: an item
                // model gets no rig here to bake it.
                match &p.billboard {
                    Some(info) => preview_billboards.push(PreviewBillboard {
                        mesh: p.mesh.clone(),
                        material: p.material.clone(),
                        bone: point.bone,
                        offset: point.offset + info.pivot,
                        kind: info.kind,
                        twins: BoothTwins {
                            blend: p.fade_blend.clone(),
                            zfill: p.zfill.clone(),
                        },
                    }),
                    None => riders.push(PreviewRider {
                        mesh: p.mesh.clone(),
                        material: p.material.clone(),
                        bone: point.bone,
                        offset: point.offset,
                        twins: BoothTwins {
                            blend: p.fade_blend.clone(),
                            zfill: p.zfill.clone(),
                        },
                    }),
                }
            }
            // The item's own emitters (a torch's flame), spawned off a host at the attach point.
            if !item.emitters.is_empty() {
                preview_effects.push(PreviewEffects {
                    bone: point.bone,
                    offset: point.offset,
                    emitters: item.emitters.clone(),
                });
            }
            let Some(paths) = ctx.glows.as_deref().and_then(|g| g.effects(w.visual)) else {
                continue;
            };
            for (slot, path) in paths
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.as_ref().map(|p| (i, p)))
            {
                // The body's attach point plus the slot's place on the item model; a slot the item
                // does not author hangs nothing.
                let at = crate::portrait::attachment_point(
                    &item.skeleton,
                    &item.attachments,
                    slot as u16,
                );
                let (Some(at), Some(glow)) =
                    (at, ctx.glows.as_deref().and_then(|g| g.models.get(path)))
                else {
                    continue;
                };
                let offset = point.offset + at;
                for p in glow.parts.iter().flatten() {
                    // Split as above: `Sparkle_A.m2` is one additive billboard quad and nothing
                    // else.
                    match &p.billboard {
                        Some(info) => preview_billboards.push(PreviewBillboard {
                            mesh: p.mesh.clone(),
                            material: p.material.clone(),
                            bone: point.bone,
                            offset: offset + info.pivot,
                            kind: info.kind,
                            twins: BoothTwins {
                                blend: p.fade_blend.clone(),
                                zfill: p.zfill.clone(),
                            },
                        }),
                        None => riders.push(PreviewRider {
                            mesh: p.mesh.clone(),
                            material: p.material.clone(),
                            bone: point.bone,
                            offset,
                            twins: BoothTwins {
                                blend: p.fade_blend.clone(),
                                zfill: p.zfill.clone(),
                            },
                        }),
                    }
                }
                if !glow.emitters.is_empty() {
                    preview_effects.push(PreviewEffects {
                        bone: point.bone,
                        offset,
                        emitters: glow.emitters.clone(),
                    });
                }
            }
        }
    }

    Some(Assembled {
        parts: preview_parts,
        riders,
        effects: preview_effects,
        billboards: preview_billboards,
        grip,
    })
}

/// One wanted rider model; `visual` is its `ItemVisuals` glow id, 0 for none, filled in by the
/// caller.
struct HeldWant {
    display: u32,
    kind: ItemModelKind,
    attach: u16,
    visual: i32,
}

/// A preview's helm, shoulder pair and held weapons. The select build forces SheatheType 0
/// (`0x472c8c`), so weapons go to the hands (main hand right, off hand left, a shield to its
/// point) and the ranged slot is skipped (`0x472bfe`): [`placement`]'s melee-drawn arm. A held
/// off-hand frill takes the same hand law; `0x47a0c0` is traced for weapons and shields only.
/// `ranged_in_hand` asks the ranged slot ranged-drawn instead, the dressing room's hand install.
fn held_wants(
    equipment: &[CharEnumItem; 19],
    helm: u32,
    race: u8,
    sex: u8,
    ranged_in_hand: bool,
) -> Vec<HeldWant> {
    let mut wants = Vec::new();
    if helm != 0 {
        wants.push(HeldWant {
            display: helm,
            kind: ItemModelKind::Helm { race, sex },
            attach: attach_id::HELM,
            visual: 0, // a helm never glows: its attach site passes a literal 0
        });
    }
    let shoulder = equipment[ENUM_SHOULDER].display_id;
    if shoulder != 0 {
        for (kind, attach) in [
            (ItemModelKind::ShoulderLeft, attach_id::SHOULDER_LEFT),
            (ItemModelKind::ShoulderRight, attach_id::SHOULDER_RIGHT),
        ] {
            wants.push(HeldWant {
                display: shoulder,
                kind,
                attach,
                visual: 0, // nor do shoulders
            });
        }
    }
    for (held_slot, enum_slot) in ENUM_HELD.into_iter().enumerate() {
        let item = equipment[enum_slot];
        if item.display_id == 0 {
            continue;
        }
        // The melee pair yields melee-drawn (1), the ranged slot only ranged-drawn (2).
        let unit_sheath = if held_slot == 2 && ranged_in_hand {
            2
        } else {
            1
        };
        let Some(attach) = placement(held_slot, item.inventory_type as u32, 0, unit_sheath) else {
            continue;
        };
        let kind = if item.inventory_type == 14 {
            ItemModelKind::Shield
        } else {
            ItemModelKind::Weapon
        };
        wants.push(HeldWant {
            display: item.display_id,
            kind,
            attach,
            visual: 0, // filled by the caller, which holds the display catalog
        });
    }
    wants
}

/// A batch's blend and depth-prime twins ([`BoothTwins`]): a character batch's from its slot's
/// set, any other's from itself. Only a bake below alpha 1 (the ghost) draws them, but they are
/// always collected so the ghost edge needs no re-assembly.
fn part_twins(part: &EntityPart, char_mats: &super::char_skin::CharSkinMaterials) -> BoothTwins {
    // [`steady_material`]'s slot walk; the two must agree.
    let quint = (|| match part.char_slot {
        Some(CharSkinSlot::Body) => {
            let (single, two) = char_mats.0.as_ref()?;
            Some(if part.two_sided { two } else { single })
        }
        Some(CharSkinSlot::Hair) => char_mats.1.as_ref(),
        Some(CharSkinSlot::Object) => char_mats.2.as_ref(),
        Some(CharSkinSlot::SkinExtra) => {
            let (single, two) = &char_mats.3;
            (if part.two_sided { two } else { single }).as_ref()
        }
        None => None,
    })();
    match quint {
        // `MatQuint` order: the fade blend third, the depth-prime twin sixth.
        Some(q) => BoothTwins {
            blend: Some(q.2.clone()),
            zfill: q.5.clone(),
        },
        None => BoothTwins {
            blend: part.fade_blend.clone(),
            zfill: part.zfill.clone(),
        },
    }
}

/// A character part's steady material, the attach swap without its fade and interior variants.
fn steady_material(
    part: &EntityPart,
    char_mats: &super::char_skin::CharSkinMaterials,
) -> Option<Handle<WowModelMaterial>> {
    let quint = match part.char_slot {
        Some(CharSkinSlot::Body) => {
            let (single, two) = char_mats.0.as_ref()?;
            if part.two_sided {
                two
            } else {
                single
            }
        }
        Some(CharSkinSlot::Hair) => char_mats.1.as_ref()?,
        Some(CharSkinSlot::Object) => char_mats.2.as_ref()?,
        Some(CharSkinSlot::SkinExtra) => {
            let (single, two) = &char_mats.3;
            (if part.two_sided { two } else { single }).as_ref()?
        }
        None => return None,
    };
    Some(quint.0.clone())
}

/// Assemble the select screen's pet, the enum's `petDisplayId` (the reference's secondary model,
/// `record+0x114`); its own system, so the character never waits on the pet's model.
pub(in crate::entities) fn build_glue_pet(
    preview: Res<GluePreview>,
    mut bake: ResMut<GluePetBake>,
    creatures: Option<Res<Creatures>>,
    // The `CreatureFamily` size ramp; without it a pet takes its display's scale, the reference's
    // family-miss fallthrough.
    families: Option<Res<crate::ui_pet_stats::PetFamilyTables>>,
    mut state: Local<PetState>,
) {
    let want = preview.look.and_then(|l| l.pet());
    if state.last != want {
        state.last = want;
        state.built = false;
    }
    if state.built {
        return;
    }
    // The server sends a pet only for a living hunter or warlock (vmangos `Player.cpp:1693`).
    let Some(want) = want else {
        if bake.display_id != 0 {
            *bake = GluePetBake {
                revision: bake.revision + 1,
                ..default()
            };
        }
        state.built = true;
        return;
    };
    let Some(creatures) = creatures.as_deref() else {
        return;
    };
    let Some(pet) = assemble_pet(creatures, families.as_deref(), want) else {
        return; // the model is still loading: retry next frame
    };
    debug!(
        "glue pet: display {} (family {} level {}) → {} parts, {} camera-facing, scale {:.3}",
        want.display_id,
        want.family,
        want.level,
        pet.parts.len(),
        pet.billboards.len(),
        pet.scale,
    );
    *bake = GluePetBake {
        revision: bake.revision + 1,
        ..pet
    };
    state.built = true;
}

/// The pet's parts off the display cache, meshes and billboards apart. No geoset filter: only the
/// character compositor hides geosets.
fn assemble_pet(
    creatures: &Creatures,
    families: Option<&crate::ui_pet_stats::PetFamilyTables>,
    want: PetLook,
) -> Option<GluePetBake> {
    let display_id = want.display_id;
    let model = creatures.models.get(&display_id)?;
    let parts = model.parts.as_deref()?;
    let (billboard_parts, mesh_parts): (Vec<&EntityPart>, Vec<&EntityPart>) =
        parts.iter().partition(|p| p.billboard.is_some());
    Some(GluePetBake {
        display_id,
        parts: mesh_parts
            .iter()
            .map(|p| PreviewPart {
                static_mesh: p.mesh.clone(),
                skinned_mesh: p.skinned_mesh.clone(),
                material: p.material.clone(),
                alpha_anim: p.alpha_anim.clone(),
                // The pet hangs on the scene, never the character (`0x47306e`), so it is never
                // ghosted with its owner; the twins are the batch's all the same.
                twins: BoothTwins {
                    blend: p.fade_blend.clone(),
                    zfill: p.zfill.clone(),
                },
            })
            .collect(),
        billboards: billboard_parts
            .iter()
            .filter_map(|p| {
                let info = p.billboard.as_ref()?;
                Some(PreviewBillboard {
                    mesh: p.mesh.clone(),
                    material: p.material.clone(),
                    bone: info.bone,
                    // The rigged pet's booth joint already bakes the pivot.
                    offset: Vec3::ZERO,
                    kind: info.kind,
                    twins: BoothTwins {
                        blend: p.fade_blend.clone(),
                        zfill: p.zfill.clone(),
                    },
                })
            })
            .collect(),
        // `parts` and `emitters` land together, so the `parts` gate covers both.
        emitters: model.emitters.clone(),
        // The family's level ramp overwrites the display's scale product (`0x472e87`).
        scale: families
            .and_then(|f| f.families.pet_scale(want.family, want.level))
            .or_else(|| creatures.model_scale(display_id))
            .unwrap_or(1.0),
        revision: 0, // the caller stamps it
    })
}

/// [`build_glue_pet`]'s per-run memory.
#[derive(Default)]
pub(in crate::entities) struct PetState {
    last: Option<PetLook>,
    built: bool,
}

/// [`build_glue_preview`]'s per-run memory.
#[derive(Default)]
pub(in crate::entities) struct PreviewState {
    last_look: Option<GlueLook>,
    built: bool,
}

/// [`build_dressup_preview`]'s per-run memory.
#[derive(Default)]
pub(in crate::entities) struct DressUpState {
    last_look: Option<DressUpLook>,
    built: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the shipped `SpellVisualKit.dbc`: row 989's `+0x3c` reads
    /// `01 00 00 00  0e 00 00 00  ff ff ff ff  ff ff ff ff  fd b9 0c 4b  00 00 00 3f`, and it fills
    /// one effect slot, at Base (`0x13`); `0x472780` rejects the other eight `-1`s.
    #[test]
    fn the_shipped_ghost_kit_decodes_to_the_verified_row() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open the install's MPQ chain");
        let catalog = benilla_formats::load_spell_visual_catalog(&mut chain)
            .expect("SpellVisual/SpellVisualKit");
        let kit = catalog
            .kit(GLUE_GHOST_KIT)
            .expect("the reference hard-codes kit 989; the shipped table has it");

        let nodes: Vec<_> = kit
            .char_procs()
            .filter_map(crate::aura_visual::node_for)
            .collect();
        assert!(
            nodes.contains(&crate::aura_visual::AuraNode::Tint([140, 185, 253])),
            "kit 989's CharProc 1 is 9222653.0 → 0x8CB9FD → RGB (140,185,253); got {nodes:?}"
        );
        assert!(
            nodes
                .iter()
                .any(|n| matches!(n, crate::aura_visual::AuraNode::Alpha(a) if *a == 0.5)),
            "kit 989's CharProc 14 is 0.5; got {nodes:?}"
        );

        let effects: Vec<_> = kit.effects().collect();
        assert_eq!(effects.len(), 1, "kit 989 fills one slot, got {effects:?}");
        let (tag, effect_id) = effects[0];
        assert_eq!(tag, 0x13, "the Base attachment (KIT_SLOT_TAGS[2])");
        let path = catalog
            .effect_path(effect_id)
            .expect("SpellVisualEffectName row for the ghost state model")
            .to_ascii_lowercase();
        assert!(
            path.replace('\\', "/").ends_with("spells/ghost_state.mdx"),
            "the ghost state model, got {path:?}"
        );
    }

    /// Every emitter the display holds reaches the bake (an imp's flames). The billboard-chain one
    /// is in the fixture because a booth keeps it only through the frame
    /// [`crate::portrait::spawn_booth_own_emitters`] spawns for it.
    #[test]
    fn a_pet_carries_every_emitter_its_display_authored() {
        let emitter = |bone: u16, billboard: Option<u16>| benilla_assets::ModelEmitter {
            def: benilla_formats::ParticleEmitterDef {
                bone,
                ..benilla_world::testing::plain_particle_def()
            },
            texture: None,
            bone_pivot: [0.0; 3],
            billboard: billboard.map(|b| benilla_assets::EmitterBillboard {
                kind: benilla_formats::BillboardKind::Spherical,
                pivot: [0.0, 0.0, 1.0],
                bone: b,
            }),
            recursion: None,
            geometry: None,
            owner_reach: 0.0,
            water_bound: (Vec3::ZERO, 0.0),
            idle_seq: usize::from(bone),
        };
        let mut dm = crate::entities::display::empty_shell();
        // An empty `parts` is still `Some`: the asset landed.
        dm.parts = Some(Vec::new());
        dm.emitters = vec![emitter(17, None), emitter(45, None), emitter(51, Some(42))];
        let mut creatures = Creatures {
            catalog: Default::default(),
            models: std::collections::HashMap::new(),
        };
        creatures.models.insert(4449, dm);

        let bake = assemble_pet(
            &creatures,
            None,
            PetLook {
                display_id: 4449,
                level: 60,
                family: 0,
            },
        )
        .expect("a display whose parts have landed assembles");
        assert_eq!(
            bake.emitters.len(),
            3,
            "every emitter the display holds must reach the bake, not a subset"
        );
        assert_eq!(
            bake.emitters.iter().map(|e| e.def.bone).collect::<Vec<_>>(),
            [17, 45, 51],
            "and in file order, on their own bones"
        );
        assert_eq!(
            bake.emitters[2].billboard.map(|b| b.bone),
            Some(42),
            "the billboard host bone rides along — a booth needs it to seat the frame"
        );

        // A display still loading yields no pet, not emitters without a body.
        let mut loading = crate::entities::display::empty_shell();
        loading.emitters = vec![emitter(17, None)];
        creatures.models.insert(4450, loading);
        assert!(
            assemble_pet(
                &creatures,
                None,
                PetLook {
                    display_id: 4450,
                    level: 60,
                    family: 0
                }
            )
            .is_none(),
            "emitters alone are not a pet"
        );
    }

    /// Pinned against the Human Warrior starting outfit: shirt, pants, boots, a main-hand sword and
    /// a shield; its food and hearthstone (inv 0) drop out.
    #[test]
    fn equip_slot_maps_the_recruit_set_and_drops_non_worn() {
        assert_eq!(equip_slot(4), Some(ENUM_BODYSLOTS[0])); // shirt (BODY) → slot 3
        assert_eq!(equip_slot(7), Some(ENUM_BODYSLOTS[3])); // pants (LEGS) → slot 6
        assert_eq!(equip_slot(8), Some(ENUM_BODYSLOTS[4])); // boots (FEET) → slot 7
        assert_eq!(equip_slot(21), Some(ENUM_HELD[0])); // WEAPONMAINHAND → main hand (15)
        assert_eq!(equip_slot(14), Some(ENUM_HELD[1])); // SHIELD → off hand (16)
        assert_eq!(equip_slot(20), Some(ENUM_BODYSLOTS[1])); // ROBE shares the chest slot (4)
        assert_eq!(equip_slot(1), Some(ENUM_HELM)); // HEAD → helm slot
        assert_eq!(equip_slot(3), Some(ENUM_SHOULDER)); // SHOULDERS → shoulder slot
        assert_eq!(equip_slot(16), Some(ENUM_CLOAK)); // BACK → cloak slot
        assert_eq!(equip_slot(19), Some(ENUM_BODYSLOTS[7])); // TABARD → slot 18
        assert_eq!(equip_slot(15), Some(ENUM_HELD[2])); // RANGED → ranged slot (17)
        for inv in [0u8, 11, 12, 18, 24, 27, 28] {
            assert_eq!(equip_slot(inv), None, "inv {inv} should not map to a slot");
        }
        for inv in 0u8..=30 {
            if let Some(slot) = equip_slot(inv) {
                assert!(slot < 19, "inv {inv} → slot {slot} out of range");
            }
        }
    }

    /// The mannequin skips the ranged slot (`0x472bfe`); the dressing room hands a bow (15) to the
    /// left and a gun, crossbow or wand (26) or a thrown weapon (25) to the right.
    #[test]
    fn the_dressing_room_hands_a_ranged_weapon_where_the_mannequin_drops_it() {
        let ranged = |inv: u8| {
            let mut e = [CharEnumItem::default(); 19];
            e[ENUM_HELD[2]] = CharEnumItem {
                display_id: 8500,
                inventory_type: inv,
            };
            e
        };
        for (inv, hand, what) in [
            (15u8, attach_id::HAND_LEFT, "bow"),
            (26, attach_id::HAND_RIGHT, "gun/crossbow/wand"),
            (25, attach_id::HAND_RIGHT, "thrown"),
        ] {
            let e = ranged(inv);
            assert!(
                held_wants(&e, 0, 1, 0, false).is_empty(),
                "the select mannequin skips the ranged slot ({what})"
            );
            let wants = held_wants(&e, 0, 1, 0, true);
            assert_eq!(wants.len(), 1, "the dressing room holds the {what}");
            assert_eq!(wants[0].attach, hand, "{what} rides the right hand point");
            assert_eq!(wants[0].display, 8500);
        }
    }

    /// The flag touches only the ranged slot: a sword still goes to HandRight, a shield to its
    /// point.
    #[test]
    fn the_ranged_flag_leaves_the_melee_pair_alone() {
        let mut e = [CharEnumItem::default(); 19];
        e[ENUM_HELD[0]] = CharEnumItem {
            display_id: 5500,
            inventory_type: 21, // WEAPONMAINHAND
        };
        e[ENUM_HELD[1]] = CharEnumItem {
            display_id: 5600,
            inventory_type: 14, // SHIELD
        };
        for ranged_in_hand in [false, true] {
            let wants = held_wants(&e, 0, 1, 0, ranged_in_hand);
            let at = |display: u32| {
                wants
                    .iter()
                    .find(|w| w.display == display)
                    .map(|w| w.attach)
            };
            assert_eq!(at(5500), Some(attach_id::HAND_RIGHT));
            assert_eq!(at(5600), Some(attach_id::SHIELD));
        }
    }
}
