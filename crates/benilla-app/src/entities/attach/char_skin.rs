//! Character appearance → per-look skin materials — the *who wears what* half of [`super`].
//!
//! [`super`] (attach) spawns a streamed entity's visual; this module resolves the character-specific
//! inputs it swaps in: the entity's [`CharLook`] (race/sex/customization — wire fields for a player,
//! CreatureDisplayInfoExtra for a character-model NPC), its worn-equipment display ids
//! ([`WornEquip`]), and the per-appearance material quints over the composited body
//! atlas / hair / cape textures ([`build_char_skin_materials`]).

use benilla_formats::{CharSkinSlot, ModelBlend};
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::{NetEntity, ObjectStore};
use benilla_assets::materials::WowModelMaterial;
use benilla_assets::{repeat_texture_authored, LockRecover, WorldAssets};

use super::super::{DisplayModel, EntityPart, SkinKey, SkinSections};

/// The resolved character appearance for one entity that renders as a character-model body — either a
/// **player** (appearance decoded from the wire) or a **character-model NPC** (from its display's
/// CreatureDisplayInfoExtra). It drives both the geoset selection and the per-appearance
/// skin/hair materials, so the two cases share one code path. `None` for a beast NPC / GameObject / a
/// unit with no appearance data — those render whole, with their built textures, as before.
pub(super) struct CharLook {
    pub(super) race: u8,
    pub(super) sex: u8,
    /// skinColor — keys the composited base skin AND the standalone extra-skin BLP (the tauren fur,
    /// M2 type 8), which even a pre-baked NPC atlas doesn't cover, so it lives here, not in `body`.
    pub(super) skin: u8,
    pub(super) hair_style: u8,
    pub(super) hair_color: u8,
    pub(super) facial_hair: u8,
    /// Where the body-skin atlas comes from.
    pub(super) body: BodySkin,
}

/// The body-skin atlas source for a [`CharLook`].
pub(super) enum BodySkin {
    /// A player (or a character-model NPC row that carries no bake name): composite the atlas live from
    /// CharSections — needs the face (skinColor is on [`CharLook`]) — cached per appearance in
    /// [`SkinComposites`].
    Composite { face: u8 },
    /// A character-model NPC: the pre-baked body atlas the client ships under `Textures\BakedNpcTextures\`
    /// (CreatureDisplayInfoExtra field 18). Loaded directly through the async `mpq://` pipeline — no
    /// compositing (the faithful path: the file ships and the client loads it, it does not re-bake).
    Baked(String),
}

/// Resolve the character look for a net entity, or `None` if it isn't a character-model body. A player
/// takes race/sex + customization from the wire (its [`ObjectStore`] descriptor fields); a
/// character-model NPC takes them from its display's [`NpcAppearance`](benilla_formats::NpcAppearance)
/// — a baked body when the row names one, else a live composite. A beast NPC / GameObject has no look.
pub(super) fn resolve_char_look(
    net: &NetEntity,
    dm: Option<&DisplayModel>,
    entity: Entity,
    stores: &Query<&ObjectStore>,
) -> Option<CharLook> {
    // The look follows the DISPLAY, not the entity kind: with live display-id
    // swaps a Player-kind entity can wear any display — a druid's bear form is a plain creature
    // model (no look; Monster skins instead), and a GM-morphed player wearing a humanoid NPC
    // display wears ITS CreatureDisplayInfoExtra appearance. The reference's own race/gender
    // getters answer from the display's cached row with the descriptor as fallback (the
    // `0x60c690` getter family) — exactly this order: display appearance first,
    // wire appearance only for a character body that carries none.
    let d = dm?;
    if let Some(npc) = d.npc_appearance.as_ref() {
        // A character-model NPC display (whoever wears it): its CreatureDisplayInfoExtra
        // appearance — a baked body when the row names one, else a live composite.
        return Some(CharLook {
            race: npc.race,
            sex: npc.sex,
            skin: npc.skin,
            hair_style: npc.hair_style,
            hair_color: npc.hair_color,
            facial_hair: npc.facial_hair,
            body: match &npc.bake_name {
                Some(name) => BodySkin::Baked(name.clone()),
                None => BodySkin::Composite { face: npc.face },
            },
        });
    }
    // A **corpse** that is not a bone pile: its look is its own
    // `CORPSE_FIELD_BYTES_1/_2` snapshot, taken at death — not the owner's live `PLAYER_BYTES`,
    // which the corpse cannot see and which may belong to a player who has since logged out. This
    // is the reference's own source: `0x5d6260` reads `[[corpse+0x110]+0x69..+0x6f]` into the
    // corpse's own `CCharacterComponent`. A bone pile builds no component and reads `None` here,
    // which is exactly `0x5d6291`'s early skip.
    if net.kind == EntityKind::Corpse && d.is_character_body {
        let look = super::super::corpse::corpse_char_look(stores.get(entity).ok())?;
        return Some(CharLook {
            race: look.race,
            sex: look.sex,
            skin: look.skin,
            hair_style: look.hair_style,
            hair_color: look.hair_color,
            facial_hair: look.facial_hair,
            body: BodySkin::Composite { face: look.face },
        });
    }
    if net.kind == EntityKind::Player && d.is_character_body {
        // A player wearing a character body. Race/sex come from `UNIT_FIELD_BYTES_0`; the
        // `PLAYER_BYTES` / `PLAYER_BYTES_2` customization is *optional on the wire* — vmangos omits an
        // all-zero field from the create mask, so a fully-default character (like our own "One") sends
        // no `PLAYER_BYTES` at all. Per UpdateFields semantics an absent field means its default (0),
        // not "no data": default each byte to 0 rather than gate the whole look on its presence (an
        // over-strict `?` here left a default-customized avatar unskinned — the pre-0061 component
        // path defaulted these implicitly).
        let s = &stores.get(entity).ok()?.0;
        return Some(CharLook {
            race: s.unit_race()?,
            sex: s.unit_gender()?,
            skin: s.player_skin().unwrap_or(0),
            hair_style: s.player_hair_style().unwrap_or(0),
            hair_color: s.player_hair_color().unwrap_or(0),
            facial_hair: s.player_facial_hair().unwrap_or(0),
            body: BodySkin::Composite {
                face: s.player_face().unwrap_or(0),
            },
        });
    }
    // A beast display (whoever wears it), a GameObject, a model-less display: no look.
    None
}

/// The `mpq://` URL for a pre-baked NPC body atlas (a CreatureDisplayInfoExtra bake name) under
/// `Textures\BakedNpcTextures\`. Lowercased forward-slash, like the other `mpq://` loads.
fn baked_npc_url(bake_name: &str) -> String {
    format!(
        "mpq://textures/bakednpctextures/{}",
        bake_name.replace('\\', "/").to_ascii_lowercase()
    )
}

/// The worn-equipment display ids that drive a character body's geoset selection (the equipment
/// branches in [`benilla_formats::CharacterGeosets::visible_geosets`]) and — for a player — the
/// region-texture composite: the 8 armor bodyslots (shirt..tabard, the [`EquipGeosets`] index order),
/// the cloak, and the helm.
///
/// A **player** takes them from its resolved [`super::Equipment`] (the wire's visible-item entries →
/// item template → display id). A **character-model NPC** takes them from its display's
/// [`NpcAppearance`](benilla_formats::NpcAppearance) equipment columns (CreatureDisplayInfoExtra —
/// decision 0060 named the gap; this is the fill): its bodyslots 2..9 (shirt..tabard) map straight onto
/// the 8 slots, field 0 (head) is the helm, and there is no NPC cloak column (the row stops at bodyslot
/// 9). A beast NPC / GameObject / a unit with no appearance row yields the all-zero naked default.
#[derive(Default)]
pub(super) struct WornEquip {
    /// shirt · chest · belt · pants · boots · wrist · gloves · tabard (bodyslot 2–9).
    pub(super) bodyslots: [u32; 8],
    pub(super) cloak: u32,
    pub(super) helm: u32,
    /// The wearer's guild tabard — a **player's** only, off their resolved
    /// [`super::Equipment`]. A character-model NPC has no guild: CreatureDisplayInfoExtra carries
    /// no guild column, and a display-driven body never joins one, so this stays `None` there and a
    /// tabard in an NPC's bodyslot-9 column keeps its own art.
    pub(super) emblem: Option<benilla_formats::GuildEmblem>,
    pub(super) tabard_preview: bool,
}

pub(super) fn resolve_worn_equip(
    net: &NetEntity,
    equipment: Option<&super::super::Equipment>,
    dm: Option<&DisplayModel>,
) -> WornEquip {
    match net.kind {
        // A **corpse** rides the player arm: its `Equipment` is resolved from the
        // 19 `CORPSE_FIELD_ITEM` slots, which are already ItemDisplayInfo ids — the same values a
        // player's items resolve to, so the geoset + region-composite law downstream is literally
        // the same law, on a different source.
        EntityKind::Player | EntityKind::Corpse => equipment
            .map(|e| WornEquip {
                bodyslots: e.bodyslots,
                cloak: e.cloak,
                helm: e.helm,
                emblem: e.emblem,
                tabard_preview: e.tabard_preview,
            })
            .unwrap_or_default(),
        // A character-model NPC's worn gear ships in its display's CreatureDisplayInfoExtra columns
        // (bodyslot-indexed) — the same ItemDisplayInfo ids a player's items resolve to. bodyslots
        // 2..9 map directly onto the 8 armor slots; field 0 is the helm; there is no cloak column.
        EntityKind::Unit => dm
            .and_then(|d| d.npc_appearance.as_ref())
            .map(|npc| WornEquip {
                bodyslots: std::array::from_fn(|i| npc.equipment[i + 2]),
                cloak: 0,
                helm: npc.equipment[0],
                emblem: None,
                tabard_preview: false,
            })
            .unwrap_or_default(),
        _ => WornEquip::default(),
    }
}

/// The worn geoset selectors for a set of equipment display ids (decisions 0074/1864's B1–B8
/// branches + the cloak group + the helm's `0x4799a0` hide-mask row pair): each non-zero display
/// resolves its ItemDisplayInfo row's geoset columns, and B3's forearm gate comes off the same
/// composite plan the atlas blits. One helper for the world attach path and the glue-preview
/// builder — the selection law can't fork.
///
/// [`EquipGeosets::tabard_preview`](benilla_formats::EquipGeosets::tabard_preview) (B6) is the
/// tabard designer's flag, up on the local player's body while that window is open (decision
/// 1977) — its only setter in the reference too.
pub(in crate::entities) fn equip_geosets(
    displays: Option<&super::super::ItemDisplays>,
    bodyslots: &[u32; 8],
    cloak: u32,
    helm: u32,
    tabard_preview: bool,
) -> benilla_formats::EquipGeosets {
    let mut eg = benilla_formats::EquipGeosets {
        tabard_preview,
        ..Default::default()
    };
    if let Some(d) = displays {
        let mut worn: [Option<&benilla_formats::ItemDisplay>; 8] = [None; 8];
        for (i, id) in bodyslots.iter().enumerate() {
            if *id != 0 {
                worn[i] = d.catalog.get(*id);
                eg.bodyslots[i] = worn[i].map(|row| row.geoset_groups);
            }
        }
        // B3's gate is the ArmLower tile's own occupancy, not "is a chest equipped" — the same
        // plan the composite blits.
        eg.forearm_dressed = benilla_formats::forearm_dressed(&worn);
        if cloak != 0 {
            eg.cloak = d.catalog.get(cloak).map(|row| row.geoset_groups[0]);
        }
        if helm != 0 {
            // Only a display that names a head MODEL is a worn helm, and only a worn helm tucks
            // hair/facial/ears away (`CreatureDisplayInfoExtra`'s head column points 126
            // character-model NPC displays at model-less jewellery rows that still carry a full
            // hide mask — see [`benilla_formats::ItemDisplay::worn_helm_vis`]).
            eg.helm_vis = d
                .catalog
                .get(helm)
                .and_then(benilla_formats::ItemDisplay::worn_helm_vis);
        }
    }
    eg
}

/// One per-appearance material set for a character runtime texture slot: (steady, interior-matte,
/// appear-fade-blend, interior-bake, interior-bake-blend, depth-prime twin). `model_material` /
/// `zfill_material` dedup by texture+blend, so all players of one look share them.
pub(super) type MatQuint = (
    Handle<WowModelMaterial>,
    Handle<WowModelMaterial>,
    Handle<WowModelMaterial>,
    Handle<WowModelMaterial>,
    Handle<WowModelMaterial>,
    Option<Handle<WowModelMaterial>>,
);

/// The full character material set [`build_char_skin_materials`] returns: `(body, hair, object,
/// skin_extra)`, the body + skin-extra each a (single-sided, two-sided) pair. Each per-slot
/// [`MatQuint`] is `None` for an absent row (a bald style, a non-fur race) or missing tables. Named
/// so the create-preview builder (`super::create_preview`) can select the steady variant per part.
pub(super) type CharSkinMaterials = (
    Option<(MatQuint, MatQuint)>,
    Option<MatQuint>,
    Option<MatQuint>,
    (Option<MatQuint>, Option<MatQuint>),
);

/// The `WOW_PROBE_SHARED_SKIN` pricing lever (see its use in [`build_char_skin_materials`]).
fn shared_skin_probe() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_PROBE_SHARED_SKIN").is_some())
}

/// Build a character body's per-appearance materials — the **body** atlas (as a (single-sided,
/// two-sided) pair — a body batch keeps its own M2 0x04, e.g. the robe skirt) and the **hair**-mesh
/// texture (a single CharSections BLP) — each as a (steady, interior-matte, fade,
/// interior-bake) quad. Works for a
/// player (atlas composited live from CharSections + overlays) *and* a
/// character-model NPC (atlas = the shipped pre-baked BLP), selected by [`CharLook::body`]. Returns
/// `(body, hair, object, skin_extra)`; each is `None` for an absent row (e.g. a bald style has no hair
/// texture, only tauren author an extra skin) or when the tables / world chain / lighting aren't
/// available (those parts then keep their built, untextured material). A composited atlas is uploaded once per look ([`super::super::SkinComposites`] cache); a baked or hair
/// BLP loads through the async `mpq://` pipeline (which dedups by path). `parts` supplies the hair
/// batches' blend (hair is alpha-cut, so it can't be forced opaque like the body).
#[allow(clippy::type_complexity)]
pub(super) fn build_char_skin_materials(
    look: &CharLook,
    // The worn armor display ids (bodyslot 2–9) + the cloak's, and the ItemDisplayInfo catalog to
    // resolve their region textures. Consumed **only** on the live-composite path
    // (a player, or the rare bake-less NPC row); a baked NPC atlas already owns the skin, so its
    // equip ids don't paint here — they drive the geosets only. `[0; 8]`/`0`/`None` = the naked body.
    equip: [u32; 8],
    cloak: u32,
    // The wearer's guild tabard, painted over the torso layers of a tabard whose
    // display asks for it. `None` = no guild, or its identity has not arrived; both leave the
    // tabard garment showing its own art.
    emblem: Option<benilla_formats::GuildEmblem>,
    tabard_preview: bool,
    displays: Option<&super::super::ItemDisplays>,
    sections: Option<&SkinSections>,
    world_assets: Option<&WorldAssets>,
    parts: &[EntityPart],
    images: &mut Assets<Image>,
    skin_cache: &mut benilla_assets::SpatialCache<SkinKey, Handle<Image>>,
    asset_server: &AssetServer,
    mats: &mut benilla_world::model_render::M2BatchMaterials,
) -> CharSkinMaterials {
    let (Some(sections), true) = (sections, mats.ready()) else {
        return (None, None, None, (None, None));
    };
    // The quint IS the engine's variant set, in the order the character swap reads it: steady,
    // interior-matte, appear-fade-blend, interior-bake, interior-bake-blend, depth-prime twin.
    let quint = |v: benilla_world::model_render::BatchVariants| {
        (
            v.steady,
            v.interior,
            v.fade_blend,
            v.interior_bake,
            v.interior_bake_blend,
            v.zfill,
        )
    };

    // Body-skin atlas. A character-model NPC loads its shipped, pre-baked atlas directly; a player (or an
    // NPC row without a bake name) composites it live from CharSections + overlays, uploaded + cached
    // once per appearance ([`SkinComposites`]). The composite reads its half-dozen BLPs synchronously off
    // the shared chain (like the WDL/clutter main-thread loads) — fine behind the cache (once per look).
    let body_tex: Option<Handle<Image>> = match &look.body {
        BodySkin::Baked(name) => Some(asset_server.load::<Image>(baked_npc_url(name))),
        BodySkin::Composite { face } => world_assets.and_then(|world| {
            let mut key = SkinKey {
                race: look.race,
                sex: look.sex,
                skin: look.skin,
                face: *face,
                facial_hair: look.facial_hair,
                hair_style: look.hair_style,
                hair_color: look.hair_color,
                equip,
                emblem,
                tabard_preview,
            };
            // `WOW_PROBE_SHARED_SKIN=1` — a PRICING lever, never a look: every body composites
            // the same key, so every body part of one mesh shares one material and bevy's
            // batcher can instance them. What it measures is the ceiling of the shared-body-
            // material lane (draw count → render-thread CPU) before that lane is built.
            if shared_skin_probe() {
                key = SkinKey {
                    race: 1,
                    sex: 0,
                    skin: 0,
                    face: 0,
                    facial_hair: 0,
                    hair_style: 0,
                    hair_color: 0,
                    equip: [0; 8],
                    emblem: None,
                    tabard_preview: false,
                };
            }
            match skin_cache.fetch(&key) {
                Some(handle) => Some(handle),
                None => {
                    // The worn ItemDisplayInfo rows whose region textures dress the atlas;
                    // an unknown/zero display id contributes nothing.
                    let catalog = displays.map(|d| &d.catalog);
                    let mut worn: [Option<&benilla_formats::ItemDisplay>; 8] = [None; 8];
                    if let Some(catalog) = catalog {
                        for (i, id) in equip.iter().enumerate() {
                            if *id != 0 {
                                worn[i] = catalog.get(*id);
                            }
                        }
                    }
                    let chain = &mut world.chain.lock_recover();
                    let composed = sections
                        .0
                        .composite_body(
                            chain,
                            key.race,
                            key.sex,
                            key.skin,
                            key.face,
                            key.facial_hair,
                            key.hair_style,
                            key.hair_color,
                            worn,
                            key.emblem,
                            key.tabard_preview,
                        )
                        .ok()??;
                    // Through the upload gate like every other texture: a composite is
                    // layered texel-by-texel on the CPU so it is already RGBA8, and
                    // `for_upload` is a no-op on it — but going through it is what makes
                    // the format and the bytes provably agree.
                    let handle = images.add(repeat_texture_authored(
                        benilla_assets::for_upload(composed),
                        (true, true),
                    ));
                    skin_cache.insert(key, handle.clone());
                    Some(handle)
                }
            }
        }),
    };
    // Body atlas: a (single-sided, two-sided) quint pair, chosen per body batch at the swap. The
    // naked body is a closed single-sided mesh (the approved decision-0044 look), but body-slot
    // batches carry their own M2 0x04: every race's robe skirt (geoset 1302) — and the undead
    // ragged-trouser batch — is authored two-sided, and flattening it to the closed-body default
    // culled the robe's inner faces (see-through from below). `model_material` dedups by key, so
    // the second quint is a handful of cache entries per look, shared like the first.
    let body = body_tex.map(|tex| {
        (
            quint(
                mats.char_variants(tex.clone(), ModelBlend::Opaque, false)
                    .expect("light buffer checked at entry"),
            ),
            quint(
                mats.char_variants(tex, ModelBlend::Opaque, true)
                    .expect("light buffer checked at entry"),
            ),
        )
    });

    // Hair-mesh texture — a single CharSections BLP loaded through the async pipeline (dedups by path).
    // Alpha-cut, so it takes the hair batches' own blend. `None` for a model with no hair part. Same
    // for a player + an NPC: the baked/composited body atlas covers the head *skin*, but the 3D hair
    // geometry is a separate geoset with its own texture, keyed here — and that same type-6 unit also
    // dresses the *facial* hair on the races whose beards are geometry, so this resolves through
    // `hair_mesh_texture`'s bald fallback rather than the raw row (a bald orc/gnome still has a beard
    // to texture; taking the blank row left it flat white).
    let hair = sections
        .0
        .hair_mesh_texture(look.race, look.sex, look.hair_style, look.hair_color)
        .and_then(|path| {
            // Hair cards are alpha-cut + two-sided (M2 `0x04`); carry both onto the swapped material.
            let hair_part = parts
                .iter()
                .find(|p| p.char_slot == Some(CharSkinSlot::Hair))?;
            let tex = asset_server.load::<Image>(format!(
                "mpq://{}",
                path.replace('\\', "/").to_ascii_lowercase()
            ));
            mats.char_variants(tex, hair_part.blend, hair_part.two_sided)
                .map(&quint)
        });

    // Cape texture (decision 0074's empirical pin): the worn cloak's ItemDisplayInfo
    // `model_texture[0]` is the Cape BLP basename under `Item\ObjectComponents\Cape\`, bound to the
    // body's runtime type-2 batches ([`CharSkinSlot::Object`]) — the cloak-geoset skin. `None`
    // (cloak-less, unknown display, no Object part) keeps those parts' built material — moot when
    // the cloak geoset branch has them hidden anyway.
    let object = (cloak != 0)
        .then_some(())
        .and_then(|()| displays?.catalog.get(cloak)?.model_texture[0].as_deref())
        .and_then(|tex_name| {
            let part = parts
                .iter()
                .find(|p| p.char_slot == Some(CharSkinSlot::Object))?;
            let tex = asset_server.load::<Image>(format!(
                "mpq://item/objectcomponents/cape/{}.blp",
                tex_name.to_ascii_lowercase()
            ));
            mats.char_variants(tex, part.blend, part.two_sided)
                .map(&quint)
        });

    // Extra-skin texture (the tauren fur, M2 type 8) — CharSections `sectionType 0` `TextureName[1]`
    // keyed by skinColor, loaded plain through the async pipeline (the client's extra loader is a bare
    // TextureCreate — no compositing; a pre-baked NPC atlas doesn't cover it either, so this applies to
    // players and NPCs alike). The batches come in exactly two authored flavors — the opaque
    // single-sided fur core and the alpha-cut two-sided fringe cards — so build one quint per flavor
    // from a representative part (a flavor no part carries is never selected). Empty column (every
    // non-fur race) ⇒ `(None, None)`; those models carry no type-8 batch anyway.
    let skin_extra = sections
        .0
        .skin_extra_texture(look.race, look.sex, look.skin)
        .map_or((None, None), |path| {
            let tex = asset_server.load::<Image>(format!(
                "mpq://{}",
                path.replace('\\', "/").to_ascii_lowercase()
            ));
            let mut quint_for = |two_sided: bool| {
                let part = parts.iter().find(|p| {
                    p.char_slot == Some(CharSkinSlot::SkinExtra) && p.two_sided == two_sided
                })?;
                mats.char_variants(tex.clone(), part.blend, part.two_sided)
                    .map(&quint)
            };
            (quint_for(false), quint_for(true))
        });

    (body, hair, object, skin_extra)
}
