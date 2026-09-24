//! The **tuple-driven character preview** builder (decisions 0423 + 0465, widened by 1060) — the
//! entities-side half of every booth that shows a character nobody is standing in. It lives under
//! `attach` so it can reuse the character body pipeline directly
//! (`char_skin::build_char_skin_materials`, the geoset filter, the [`super::super::EntityPart`]
//! source, the item-display models) without widening any of it.
//!
//! Two callers, **one assembly** ([`assemble`]). Each reduces its own look to the same
//! [`PreviewSpec`] — body displayId + the five appearance dials + an enum-shaped
//! `[CharEnumItem; 19]` — and gets back the flat part/rider/effect lists its booth bakes:
//!
//! - **The glue screens** ([`build_glue_preview`]): a **Select** look carries its roster record
//!   verbatim; a **Create** look resolves the (race, class, sex) starting outfit (CharStartOutfit,
//!   decision 0527) into that array — so the create preview is geared exactly like a select body
//!   (the ref's create screen dresses the model, not the underwear-only body decision 0423 assumed).
//! - **The dressing room** ([`build_dressup_preview`], decision 1060): the *live player's* own body
//!   and appearance, wearing their own visible items with the tried-on ones substituted in. That
//!   substitution is the whole feature, and it is why the dressing room cannot mirror a world
//!   entity's spawned children the way the paper doll does (`portrait::sync_body_booth`) — nobody
//!   in the world is wearing the item being previewed.
//!
//! The assembly is the world's own way — displayId → the shared [`Creatures`] display cache →
//! geoset filter → the composited body over a hand-built [`CharLook`] — plus the armor-region
//! composite + real equipment geosets, the cloak, and the attach riders: helm (per-race/sex model,
//! hide-helm honored), shoulders, and the weapons **held in the hands** (the select build
//! `0x472950` → `0x47a0c0`, folded back over 0465's sheathed INTERIM). The portrait
//! module's `sync_glue_booth` / `sync_dressup_booth` then re-light and bake them (they own the
//! *booth*; this owns the *assembly*). A glue body's displayId is added to
//! `update_display_models`'s want-list (in [`super::super`]) so its model builds with no wire
//! entity — the dressing room's is the player's own, already built; item models register through
//! the same [`ItemDisplays`] want the world path uses.

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

/// The enum equipment slots that paint the armor-region composite, in the bodyslot order
/// (shirt · chest · belt · pants · boots · wrist · gloves · tabard → bodyslot 2–9):
/// equipment-slot indices 3, 4, 5, 6, 7, 8, 9, 18.
const ENUM_BODYSLOTS: [usize; 8] = [3, 4, 5, 6, 7, 8, 9, 18];
/// The enum's helm / shoulder / cloak slots.
const ENUM_HELM: usize = 0;
const ENUM_SHOULDER: usize = 2;
const ENUM_CLOAK: usize = 14;
/// Main hand · off hand · ranged, in [`placement`]'s held-triple slot order (0/1/2).
const ENUM_HELD: [usize; 3] = [15, 16, 17];

/// Map an item's `InventoryType` to its equipment-slot index in the enum-shaped `[_; 19]` array
/// (`EQUIPMENT_SLOT_*`), so a CharStartOutfit item (decision 0527) lands where the Select pipeline
/// reads it — the render slots the compositor paints (helm 0 · shoulder 2 · shirt 3 · chest 4 ·
/// waist 5 · legs 6 · feet 7 · wrist 8 · hands 9 · back 14 · tabard 18, `0x478cb0`)
/// plus the held triple (main 15 · off 16 · ranged 17). `None` for a non-worn / non-rendered type
/// (bags, ammo, quiver, relic, and NON_EQUIP consumables — the outfit's food + hearthstone).
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

/// One preview's complete input tuple — the body model, its five appearance dials, and the worn
/// equipment in the `SMSG_CHAR_ENUM` slot shape. Both callers reduce their own look to this, so
/// [`assemble`] has exactly one shape to build from and the two previews can never dress by
/// different laws.
pub(in crate::entities) struct PreviewSpec {
    /// The **body** display id, already resolved by the caller: the race/sex body for a glue look
    /// (`CharCreateCatalog::body_display`), the live player's own display for the dressing room.
    /// Its model must be in the display cache (or its want-list) or the assembly waits for it.
    pub(in crate::entities) display_id: u32,
    pub(in crate::entities) race: u8,
    pub(in crate::entities) sex: u8,
    pub(in crate::entities) skin: u8,
    pub(in crate::entities) face: u8,
    pub(in crate::entities) hair_style: u8,
    pub(in crate::entities) hair_color: u8,
    pub(in crate::entities) facial_hair: u8,
    /// Worn **ItemDisplayInfo** ids by equipment slot (`EQUIPMENT_SLOT_*`) — no template hop; the
    /// caller has already resolved entries to displays.
    pub(in crate::entities) equipment: [CharEnumItem; 19],
    /// The wearer's guild tabard (decision 1704), or `None` for no crest.
    pub(in crate::entities) emblem: Option<benilla_formats::GuildEmblem>,
    /// `CHARACTER_FLAG_*` bits; only hide-helm / hide-cloak are read here.
    pub(in crate::entities) flags: u32,
    /// **Whose held law this look follows** (decision 1076) — the one place the two previews
    /// genuinely differ, and a *verified* difference rather than a convenience.
    ///
    /// `false` — the **character-select mannequin** (`0x472950`), whose equipment loop skips
    /// `EQUIPMENT_SLOT_RANGED` outright (`0x472bfe cmp esi,0x11; je`) and forces SheatheType 0.
    ///
    /// `true` — the **dressing room** (`DressUpModel::TryOn 0x504d90`), which does not skip it: its
    /// remap table (`0x504b7c` → `0x504b44`) sends `INVTYPE_RANGED` down the same held arm as a
    /// weapon, and `sheathFlag = 0` is pushed literally at `0x504adc`, so a tried-on ranged weapon
    /// is installed **at a hand and never at a sheath point, whatever its SheatheType** — by the
    /// world's own split (bow → HandLeft, gun/crossbow/wand/thrown → HandRight). Which is exactly
    /// why this rides [`placement`]'s existing, already-verified ranged arm rather than growing a
    /// second copy of that mapping.
    pub(in crate::entities) ranged_in_hand: bool,
}

/// What one [`assemble`] produced — the content half of a preview bake, in the booth's own shape.
pub(in crate::entities) struct Assembled {
    pub(in crate::entities) parts: Vec<PreviewPart>,
    pub(in crate::entities) riders: Vec<PreviewRider>,
    pub(in crate::entities) effects: Vec<PreviewEffects>,
    pub(in crate::entities) billboards: Vec<PreviewBillboard>,
    pub(in crate::entities) grip: [bool; 2],
}

/// The resource borrows [`assemble`] needs — the display caches it reads and registers wants with,
/// the character/skin data the composite is built from, and the asset stores it writes into. A
/// plain borrow struct rather than a `SystemParam`: each driver already holds these as its own
/// `Res`/`ResMut` params and simply lends them for the call.
pub(in crate::entities) struct PreviewCtx<'a, 'w> {
    pub(in crate::entities) creatures: &'a Creatures,
    pub(in crate::entities) characters: Option<&'a Characters>,
    pub(in crate::entities) displays: Option<&'a mut ItemDisplays>,
    /// The item/enchant glow chain (decision 0805): both previews resolve it themselves — the
    /// world's `resolve_equipment` never runs pre-world, and never runs for a look nobody wears.
    pub(in crate::entities) glows: Option<&'a mut ItemGlows>,
    pub(in crate::entities) sections: Option<&'a SkinSections>,
    pub(in crate::entities) world_assets: Option<&'a WorldAssets>,
    pub(in crate::entities) images: &'a mut Assets<Image>,
    pub(in crate::entities) skin_composites: &'a mut SkinComposites,
    pub(in crate::entities) asset_server: &'a AssetServer,
    pub(in crate::entities) mats: &'a mut benilla_world::model_render::M2BatchMaterials<'w>,
}

/// Assemble the preview part (+ rider) lists when the glue-screen look changes. Runs in the
/// entity-visuals chain (after `update_display_models` has built the body model from the
/// want-list), so a fresh look's models are ready within a frame or two; until then it leaves the
/// bake untouched and retries. A bare yaw change never reaches here (the booth handles it).
pub(in crate::entities) fn build_glue_preview(
    preview: Res<GluePreview>,
    mut bake: ResMut<GluePreviewBake>,
    creatures: Option<Res<Creatures>>,
    characters: Option<Res<Characters>>,
    char_create: Option<Res<CharCreate>>,
    mut displays: Option<ResMut<ItemDisplays>>,
    // The item/enchant glow chain (decision 0805): the glue screens resolve it themselves — the
    // world's `resolve_equipment` never runs pre-world.
    mut glows: Option<ResMut<ItemGlows>>,
    sections: Option<Res<SkinSections>>,
    world_assets: Option<Res<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut skin_composites: ResMut<SkinComposites>,
    asset_server: Res<AssetServer>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    // The ghost kit (`0x47280f`): the `SpellVisualKit` catalog the row is read from, and the
    // path-keyed spell-effect model cache its attached models load through — the same one the
    // world's kit plays use, so `Spells\Ghost_state.mdx` is loaded once per session whichever
    // screen wants it first.
    visuals: Option<Res<crate::creature_anim::SpellVisuals>>,
    mut fx: Option<ResMut<super::super::spell_fx::SpellFx>>,
    // Change tracking: the look we last emitted a bake for, and whether that bake succeeded (so a
    // look whose models weren't ready yet keeps retrying until they build).
    mut state: Local<PreviewState>,
) {
    // A new look resets the retry latch.
    if state.last_look != preview.look {
        state.last_look = preview.look;
        state.built = false;
    }
    if state.built {
        return;
    }

    // No look → clear the booth once.
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

    // displayId → the shared display cache. The want-list in `update_display_models` has already
    // asked for this display; the assembly waits (retry, `built` stays false) until its model +
    // parts are built.
    let (Some(creatures), Some(char_create)) = (creatures.as_deref(), char_create.as_deref())
    else {
        return;
    };
    let (race, sex) = look.body();
    let Some(display_id) = char_create.0.body_display(race, sex) else {
        state.built = true; // a non-playable race can't resolve — nothing to show, don't spin
        return;
    };

    // The body appearance is identical in shape for both looks (only its source struct differs).
    let (skin, face, hair_style, hair_color, facial_hair) = match look {
        GlueLook::Create(l) => (l.skin, l.face, l.hair_style, l.hair_color, l.facial_hair),
        GlueLook::Select(l) => (l.skin, l.face, l.hair_style, l.hair_color, l.facial_hair),
    };

    // The equipment, unified as an enum-shaped `[CharEnumItem; 19]` + display flags: a Select look
    // carries its roster record verbatim; a Create look resolves the (race, class, sex) starting
    // outfit (CharStartOutfit, decision 0527) into the same slot array — each worn item at the
    // equipment slot its InventoryType maps to. Display ids are ItemDisplayInfo ids in both cases
    // (no template hop). A level-1 outfit carries no helm/cloak and there is no hide flag, so the
    // create preview's flags are 0 — the create body is dressed exactly like a geared select body.
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
        // No crest at character select (decision 1704). `SMSG_CHAR_ENUM` carries a `guildId` but
        // not the five emblem indices, and there is no world session to `CMSG_GUILD_QUERY` with —
        // so a Guild Tabard on the mannequin wears its own `Tabard_A_05Default` art, which is what
        // the reference's own empty guild cache leaves it with too.
        emblem: None,
        flags,
        // The mannequin's own law: the select build skips EQUIPMENT_SLOT_RANGED (0x472bfe).
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
        return; // a model (body, item, or glow) is still loading — retry next frame
    };

    // The ghost kit — a **Select** look only, and only when the roster record carries the bit
    // (`CHARSELECT+0xfc & 0x2000`). A create look never can: the character does not exist yet, and
    // the create screen builds its flags as `0` above. The kit's attached models join the ordinary
    // lists; what the body renders with rides the bake.
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
                return; // an effect model is still loading — retry, don't bake a half ghost
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

/// Assemble the **dressing room**'s look (decision 1060) — the same law as the glue driver above,
/// over a look whose equipment [`crate::ui_dressup`] composed from the player's own visible items
/// plus the tried-on substitutions. The body display is the player's own, so it is already built
/// (they are standing in the world); an ITEM model still has to load, which is what the retry latch
/// is for.
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
    // A new look resets the retry latch (the glue driver's own shape).
    if state.last_look != preview.look {
        state.last_look = preview.look;
        state.built = false;
    }
    if state.built {
        return;
    }

    // No look → clear the booth once (the window closed, or nothing to show).
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
        // **No hide flags here, because the hiding already happened upstream** (decision 1472).
        // The dressing room DOES honour show-helm/show-cloak — byte-verified: `SetUnit 0x476cb0`
        // clones the live player's per-bodyslot display pointers, so a suppressed piece is not in
        // what is cloned — but a TRY-ON must preview the helm regardless, and only
        // [`crate::ui_dressup`]'s look builder knows which slots are worn and which are tried on.
        // So it applies the preference there, per slot, and hands us an already-correct array;
        // re-applying a flag mask over it here would hide the very item being previewed.
        flags: 0,
        // The widget's law, not the mannequin's (decision 1076). Unconditional, because the ranged
        // slot of a dressing-room look is only ever filled by a TRY-ON: `crate::ui_dressup` leaves
        // a worn ranged weapon out, since the reference's Dress()/SetUnit clones the live world
        // model and a ranged weapon shows there only while ranged-drawn.
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
        return; // an item model is still loading — retry next frame
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

/// The one assembly both previews run — [`PreviewSpec`] in, the booth's flat lists out.
///
/// `None` means **not ready**: the body model, an item model, or a glow model is still loading.
/// A caller must treat that as "retry next frame" and leave its bake untouched, never as an empty
/// look — a geared character popping in piecewise would flicker on every change.
/// The `SpellVisualKit` row the character-select ghost is, **hard-coded exactly as the reference
/// hard-codes it**: `0x47280f` loads `idmap[989]` behind the guard
/// `0x4727f6 cmp ds:0xc0d750,0x3dd`. The id is the constant; every value behind it is DBC data,
/// read below. 989 is `SpellVisual` 886's state kit — the visual of spell 8326 "Ghost", which is
/// also what a released ghost in the WORLD rides (`0x5ff350`), so the two paths share one row by
/// construction.
const GLUE_GHOST_KIT: u32 = 989;

/// What the ghost kit contributes to a select bake: the body's own render properties, plus the
/// kit's attached effect models already reduced to the booth's ordinary seat shapes.
struct GhostArm {
    kit: GhostKit,
    riders: Vec<PreviewRider>,
    billboards: Vec<PreviewBillboard>,
    effects: Vec<PreviewEffects>,
}

/// Resolve [`GLUE_GHOST_KIT`] against the live `SpellVisualKit` catalog and the body's attachment
/// table. `None` means **not ready** — either a DBC is absent (nothing to show, the caller stops
/// retrying) or one of the kit's effect models is still loading (the caller retries), which the
/// `pending` out-flag distinguishes.
///
/// The tint and alpha go through [`crate::aura_visual::node_for`] — the one dispatch point, the ECS
/// twin of the client's `0x60dbfc` jump table — rather than a second decode here: the select screen
/// and the world aura reach the same row, and a proc type newly given a mechanism must light up on
/// both at once. What the select path does NOT share is the world's 1000 ms `StartAlphaFade` ramp:
/// `0x4727f0` writes the instance fields straight through `0x710cb0`/`0x710cf0`, so this is instant.
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
            // Every other proc type the row could carry belongs to a mechanism the select path has
            // no equivalent for (the anim-rate freeze writes bone clocks on a world unit). 989
            // carries only these two; a patched row that carries more is ignored, not guessed at.
            _ => {}
        }
    }
    let kit = GhostKit {
        tint: tint?,
        alpha: alpha.unwrap_or(1.0),
    };

    // The kit's attached effect models, at the body's own attachment points. `Spells\Ghost_state.mdx`
    // (slot tag 0x13, Base) is the only populated slot on the shipped row and is emitter-only — but
    // the row is data, so geometry and camera-facing cards are carried too, split exactly the way the
    // item-glow arm below splits an item's own model.
    let (mut riders, mut billboards, mut effects) = (Vec::new(), Vec::new(), Vec::new());
    for (tag, effect_id) in kit_row.effects() {
        // The world-plant sentinel names no attachment point; the select build hangs models on the
        // character alone (`0x472780` → `0x712f70`), so there is nowhere for it to go.
        if tag == benilla_formats::WORLD_EFFECT_TAG {
            continue;
        }
        let Some(path) = visuals.effect_path(effect_id) else {
            continue;
        };
        let path = path.to_string();
        // `0x472780`'s `test esi,esi; jl 0x4727dd` rejects a `-1` attach id, and a body with no such
        // point hangs nothing — the world path's rule, and the reference's.
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
            *pending = true; // still loading — the caller retries rather than baking half a ghost
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
    let dm = ctx.creatures.models.get(&display_id)?; // model not built yet — retry next frame
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
    // Per-hand grip `[right, left]`: a hand whose attach point holds a weapon curls into `HandsClosed`
    // (the ref's paperdoll rule `0x5059a0`: attach-point occupancy, per hand). Resolved here
    // because the flat rider list drops each held item's attach id. A shield sits on the forearm
    // (Shield attach, id 0), never a hand point, so its hand stays open.
    let grip = [
        held.iter().any(|w| w.attach == attach_id::HAND_RIGHT),
        held.iter().any(|w| w.attach == attach_id::HAND_LEFT),
    ];

    // The rider models: register each want with [`ItemDisplays`] (built by
    // `update_display_models` once the M2 loads) and gate the bake on ALL of them being ready —
    // a geared character popping in piecewise would flicker on every selection change.
    if let Some(d) = ctx.displays.as_deref_mut() {
        for w in &held {
            ensure_item_model(d, w.display, w.kind, ctx.asset_server);
        }
        // The glow ids (decision 0805), and the models they imply: **held weapons/shields only**
        // — the reference passes a display's visual from the hand attach and a literal `0` from
        // the helm/shoulder ones (`crate::entities::item_glow`). The world lane resolves this in
        // `resolve_equipment`, which never runs pre-world.
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
            return None; // an item model is still loading — retry next frame
        }
        // Same gate for the glow models: they are tiny beside the weapon that carries them, and a
        // weapon that pops in ahead of its glow would flicker on every selection change.
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

    // The dressed geoset set: the worn armor's geoset groups + the cloak + the helm's hide-masks
    // (`0x4799a0`), exactly the world path's selection (the shared helper).
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

    // Whether this look shows the part's geoset (billboard or body alike).
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
            // `None` — decision 0807's named gap, deliberately still open: a character batch with
            // an authored dimming constant previews at 1.0. The select body's look is signed off
            // as it stands, so closing it is its own change, not a rider on the pet's.
            alpha_anim: None,
        })
        .collect();

    // The eye-glow (and any character billboard) — the undead/night-elf glowing eyes. The world
    // splits these into camera-facing cards; the booth is a separate camera, so it can't ride that
    // system — carry them through to the booth spawn, which seats each on its billboard bone's joint
    // and re-faces it ([`crate::portrait::booth::face_booth_billboards`]). Geoset-gated like the body
    // parts: the undead glow rides geoset 302 (the glowing "Features" variants), faithful to the DBC.
    // The batch is fullbright (M2 unlit), so it keeps its built material — no char-skin swap. Offset
    // `ZERO`: the body IS rigged here, so its billboard bone's joint already bakes the pivot.
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

    // The riders: each want's item-model parts seated at the body's attach point (bone + offset),
    // the world path's `PortraitRider` shape — plus, for a held weapon, the glows its display's
    // `ItemVisuals` id hangs on the weapon's OWN attachment points (decision 0805). The reference
    // reaches those through the same primitive here as in the world (`0x472c91` → `0x47a0c0`
    // hands `ItemDisplayInfo+0x58` to `0x4798c0`), so a permanently-glowing weapon glows on the
    // select screen too; the enum carries no enchant ids (vmangos `BuildEnumData` sends displayId
    // + inventoryType only), so the fork's enchant half simply has no input here.
    //
    // An item model is not just meshes, and for a while this loop treated it as if it were — it kept
    // the batches with no billboard flag and dropped everything else on the floor, so at character
    // select a worn item's own **effects** (its emitters, its camera-facing batches) simply did not
    // exist. The R14 PVP shoulders' sparkle is the item
    // model's own emitters (decision 0813), and no part of it reached the booth. So each want
    // contributes up to four things at one seat: plain meshes ([`PreviewRider`]), camera-facing
    // batches ([`PreviewBillboard`]), its emitters ([`PreviewEffects`]), and — held weapons only —
    // the same three off each glow model its `ItemVisuals` names.
    let attach_point = |id: u16| dm.attachments.iter().find(|a| a.id == id);
    let mut riders = Vec::new();
    let mut preview_effects = Vec::new();
    if let Some(d) = ctx.displays.as_deref() {
        for w in &held {
            let Some(point) = attach_point(w.attach) else {
                continue; // the body has no such attach point — hold nothing (world-path rule)
            };
            let Some(item) = d.models.get(&(w.display, w.kind)) else {
                continue; // gated above; belt-and-braces
            };
            let Some(item_parts) = item.parts.as_deref() else {
                continue;
            };
            for p in item_parts.iter() {
                // A camera-facing batch of the item — a wand's gem, a glowing rune (270 of the 2681
                // `Item\` models author one). It can't be drawn as a plain rider mesh: the world
                // splits it into a card, so the booth carries it as one too, seated at the attach
                // point **plus its own model-local pivot** (an item model gets no rig here, so
                // nothing else bakes that pivot).
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
            // The item model's OWN particle emitters — the R14 PVP pauldron's `SPARKLE` twinkle
            // (decision 0813), the held torch's flame. 95 `Item\` models hang one on a
            // billboard bone alone; the booth spawns them off a host at the attach point, each
            // billboard-chain emitter through a booth-camera frame.
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
                // The seat: the body's attach point plus where this slot sits on the ITEM's own
                // model. A slot the item model doesn't author hangs nothing — the world lane's
                // rule, and the reference's.
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
                    // Same split as the item's own batches above. This is not a rare arm:
                    // `Spells\Enchantments\Sparkle_A.m2` is *one* additive camera-facing quad and
                    // nothing else — no emitters at all — and `ItemVisuals` 28 (10 item displays)
                    // hangs it on slot 4, so dropping billboard batches here glowed those items
                    // nothing whatsoever at select.
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

/// One wanted rider model: which display + model kind, the body attach point it seats on, and —
/// filled in by the caller once the display catalog is in hand — the `ItemVisuals` id its glow
/// comes from (`0` for everything that never glows: helm, shoulders, and any weapon whose display
/// authors none). Decision 0805.
struct HeldWant {
    display: u32,
    kind: ItemModelKind,
    attach: u16,
    visual: i32,
}

/// The select character's attach-model wants from its enum record: helm + the shoulder pair, and
/// the weapons **held in the hands** — the select build (folding back decision 0465's sheathed
/// INTERIM): the builder forces SheatheType 0 (`0x472c8c`), so the sheath resolver never yields a
/// stow point and the hand code runs — mainhand to HandRight (attach 1), an off-hand weapon to
/// HandLeft (2), a shield to the Shield point (0) — and the RANGED slot is skipped outright
/// (`0x472bfe`). [`placement`]'s melee-drawn arm (`unit_sheath = 1`) is exactly that law,
/// ranged-skip included (its ranged arm only yields while ranged-drawn). A held off-hand frill
/// (INVTYPE_HOLDABLE) rides the same hand law — the reference confirms it at `0x47a0c0` for
/// weapons/shields only, and the world's held placement is the natural reading for the rest.
///
/// **`ranged_in_hand` is the dressing room's departure from that** (decision 1076): the dress-up
/// widget does *not* skip the ranged slot, and installs it at a hand. That is the same mapping
/// [`placement`]'s **ranged-drawn** arm already carries, so the flag simply chooses which sheath
/// state the ranged slot is asked with — the melee pair is always asked drawn.
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
            visual: 0, // a helm never glows: its attach site pushes a literal 0 (0805)
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
                visual: 0, // nor do shoulders — same rule
            });
        }
    }
    for (held_slot, enum_slot) in ENUM_HELD.into_iter().enumerate() {
        let item = equipment[enum_slot];
        if item.display_id == 0 {
            continue;
        }
        // Per-slot, because the two arms want opposite sheath states: the melee pair yields while
        // melee-drawn (1), the ranged slot only while ranged-drawn (2). The mannequin asks every
        // slot drawn-melee, which is its own ranged skip; the dressing room asks the ranged slot
        // drawn-ranged, which is the widget's hand install (decision 1076).
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

/// A batch's translucency twins ([`BoothTwins`]) as the preview carries them — the `AlphaMode::Blend`
/// twin and the depth-prime twin, both already built beside the steady material by
/// `M2BatchMaterials` (an `EntityPart` carries its own pair; a character composite's are the quint's
/// third and sixth members). `char_mats` supplies them for a **character** batch, whose steady
/// material is the composited atlas rather than the part's own built one; every other part answers
/// from itself.
///
/// Only a bake whose instance alpha is below 1 ever draws through them (the ghost select), but they
/// are collected unconditionally: they cost two handle clones on a bake that happens once per
/// selection, and a look that resolves them *conditionally* would have to re-assemble on the ghost
/// edge, which is exactly the sort of second code path the one-alpha law exists to prevent.
fn part_twins(part: &EntityPart, char_mats: &super::char_skin::CharSkinMaterials) -> BoothTwins {
    // The same slot walk [`steady_material`] does, kept beside it so the pair can never disagree
    // about which quint a batch reads.
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
        // The quint's order is the engine's variant set: (steady, interior, fade_blend,
        // interior_bake, interior_bake_blend, zfill) — `char_skin::build_char_skin_materials`.
        Some(q) => BoothTwins {
            blend: Some(q.2.clone()),
            zfill: q.5.clone(),
        },
        // A non-character batch keeps its own built material, so it keeps its own twins.
        None => BoothTwins {
            blend: part.fade_blend.clone(),
            zfill: part.zfill.clone(),
        },
    }
}

/// The steady, world-lit material a character part wears — the body atlas (at the part's sidedness),
/// the hair texture, the cape, or the extra-skin fur — mirroring the attach swap ([`super`]) but
/// keeping only the steady variant (the booth wants no fade / interior twin). `None` for a
/// non-character part (it keeps its built material).
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

/// Assemble the select screen's **pet** — the hunter/warlock companion the enum's pet triple names
/// (`SMSG_CHAR_ENUM`'s `petDisplayId`, the reference's secondary model `record+0x114`).
///
/// Its own system beside [`build_glue_preview`], not a limb of it, for the reason
/// [`GluePetBake`] is its own resource: a pet is an ordinary creature display with no compositor,
/// no equipment and no geoset selection between the display cache and the booth, and its model
/// lands on its own schedule. Folding it into the character's assembly would gate the *character*
/// on the pet's asset — a body that waits for a wolf to load is a worse screen than a body that is
/// joined by one a frame later.
///
/// Runs after `update_display_models`, whose want-list asked for this display; retries (leaving
/// `built` false) until its parts build.
pub(in crate::entities) fn build_glue_pet(
    preview: Res<GluePreview>,
    mut bake: ResMut<GluePetBake>,
    creatures: Option<Res<Creatures>>,
    // The `CreatureFamily` size ramp (decision 1538) — the pet's scale is its family's, read at its
    // level. Absent (the DBC failed to load) ⇒ every pet falls back to its display's own scale
    // product, which is the reference's own family-miss fallthrough.
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
    // No pet (every class but a living hunter's or warlock's — the server suppresses the triple
    // for the rest) → clear the bake once and stop.
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
        return; // the model is still loading — retry next frame
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

/// The pet's parts, straight off the shared display cache: every batch of the creature model, split
/// into plain meshes and the camera-facing ones (a voidwalker's eyes are the latter). `None` while
/// the display's model is still building.
///
/// No geoset filter: only the character compositor selects among a model's geosets, and a pet is
/// not a character model — the world's creature path draws every batch of a beast the same way.
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
                // The pet is its own CM2 instance, chained to the SCENE (attachment 1), never to
                // the character (`0x47306e`) — so it composes onto nothing and is never ghosted
                // with its owner. The twins ride anyway, for the same reason the body's do: they
                // are a property of the batch.
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
                    // `ZERO`: the pet is rigged, so its billboard bone's booth joint already bakes
                    // the pivot (the 0130 rig identity) — the same seat the body's eye-glow takes.
                    offset: Vec3::ZERO,
                    kind: info.kind,
                    twins: BoothTwins {
                        blend: p.fade_blend.clone(),
                        zfill: p.zfill.clone(),
                    },
                })
            })
            .collect(),
        // The model's own emitters, carried whole: an imp without its flames is the same kind of
        // wrong as an imp without its wings. `parts` and `emitters` are captured together when the
        // asset lands, so the `parts` gate above already answers for both.
        emitters: model.emitters.clone(),
        // The size: the family's level ramp, and the display's scale product only where the family
        // table can't answer — which is the reference's own order (it computes the product, then
        // overwrites it with the ramp, `0x472e87`). A display neither can size renders at 1.0.
        scale: families
            .and_then(|f| f.families.pet_scale(want.family, want.level))
            .or_else(|| creatures.model_scale(display_id))
            .unwrap_or(1.0),
        revision: 0, // the caller stamps it
    })
}

/// [`build_glue_pet`]'s per-run memory — the pet display it last emitted a bake for, and whether
/// that bake succeeded (a display whose model wasn't ready keeps retrying).
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

/// [`build_dressup_preview`]'s per-run memory — the same latch over the dressing room's look.
#[derive(Default)]
pub(in crate::entities) struct DressUpState {
    last_look: Option<DressUpLook>,
    built: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GOLDEN, against the **real shipped 5875 `SpellVisualKit.dbc`** — the one row the reference
    /// hard-codes for the character-select ghost, decoded through the same dispatch point the world
    /// aura uses.
    ///
    /// Every number here is read at the record itself (row 989's `+0x3c` reads
    /// `01 00 00 00  0e 00 00 00  ff ff ff ff  ff ff ff ff  fd b9 0c 4b  00 00 00 3f`), and
    /// `0x4B0CB53D → (140,181,61)` is a misreading of those bytes. That is exactly the failure
    /// this test exists to catch: a tint transcribed one nibble wrong is still a plausible pale
    /// blue, and nothing but the shipped row can tell them apart.
    ///
    /// It also pins the *shape* the select path depends on — one populated effect slot, at
    /// attachment `0x13` (Base) — because "nine ghost sub-models" was an earlier reading (benilla
    /// decision 0469 §6 carried it), and `0x472780`'s `test esi,esi; jl` rejects the eight `-1`s.
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

        // Exactly ONE populated slot, and it is the Base attach point — not nine.
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

    /// **The bug the imp filed** (decision 1539): a pet is a *creature* display, and creature models
    /// routinely author their own particle emitters — the Imp's three flame jets, the Voidwalker's
    /// four smoke plumes. `assemble_pet` read `parts` and nothing else, so they never reached the
    /// booth and the select screen stood a grey imp beside the warlock.
    ///
    /// Asserted as "everything the display cache holds arrives", not "three emitters arrive": the
    /// failure mode was a *dropped field*, and a count pinned to one model would pass just as
    /// silently the next time one is added. The billboard-chain emitter is in the fixture because
    /// it is the branch that survives a booth's `without_camera_billboards` only through the frame
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
        // Built, and it drew nothing of its own — the emitters are the whole model here. An empty
        // `parts` list is still `Some`, which is what "the asset landed" means to the gate.
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

        // A display still loading (`parts: None`) yields nothing at all rather than a pet with
        // emitters and no body: the retry latch is what stands it up a frame later.
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

    /// The InventoryType → equipment-slot map lands each worn kind where the Select pipeline reads
    /// it, and drops the non-rendered kinds. Pinned against the real Human Warrior recruit set
    /// (byte-verified in decision 0527): shirt→body(3), pants→legs(6), boots→feet(7), the mainhand
    /// sword→15, the shield→offhand(16); the outfit's food/hearthstone (inv 0) drop out.
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
                                                        // Non-worn / non-rendered kinds drop out entirely.
        for inv in [0u8, 11, 12, 18, 24, 27, 28] {
            assert_eq!(equip_slot(inv), None, "inv {inv} should not map to a slot");
        }
        // Every mapped slot is a valid index into the 19-slot enum array.
        for inv in 0u8..=30 {
            if let Some(slot) = equip_slot(inv) {
                assert!(slot < 19, "inv {inv} → slot {slot} out of range");
            }
        }
    }

    /// **The two held laws, side by side** (decision 1076). The character-select mannequin skips
    /// `EQUIPMENT_SLOT_RANGED` outright (`0x472bfe`); the dressing-room widget installs it at a
    /// hand, by the world's own split — bow (INVTYPE_RANGED 15) to HandLeft, gun/crossbow/wand
    /// (26) and thrown (25) to HandRight.
    ///
    /// Asserted per inventory type rather than once, because a one-sided implementation — placing
    /// every ranged type at the same hand — is the plausible wrong answer and it still shows *a*
    /// bow, which is all the report asked for.
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

    /// …and the flag touches ONLY the ranged slot: the melee pair is asked drawn either way, so a
    /// dressing-room look still hands a sword to HandRight and a shield to the Shield point.
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
