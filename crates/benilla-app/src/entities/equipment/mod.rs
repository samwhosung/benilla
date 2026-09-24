//! Equipment visuals (decisions 0072/0074): held items (weapons, shields, ranged), worn-armor
//! resolution, and helm/shoulder attach models on units' bodies.
//!
//! Two halves, both per-frame systems chained after [`super::attach_entity_visuals`]:
//!
//! - **Resolution** ([`resolve_equipment`]) — what should each unit hold, and where? A **creature**
//!   carries its item **display ids directly** in `UNIT_VIRTUAL_ITEM_SLOT_DISPLAY` (+ class/invType and
//!   a per-item sheath type in `UNIT_VIRTUAL_ITEM_INFO`) — no lookup. A **player** exposes only item
//!   *entries* (`PLAYER_VISIBLE_ITEM_*`); the display id / inventory type / sheath type come from the
//!   item template, resolved through the ask-once item layer ([`crate::items::Items`], `CMSG_ITEM_QUERY_SINGLE` on
//!   miss — the real client's ItemCache does exactly this; other players' inventory GUIDs are
//!   server-private, so the visible-item entry is the *only* path). Drawn-vs-stowed placement follows
//!   the unit's sheath state (`UNIT_FIELD_BYTES_2` byte 0) + the item's sheath type.
//! - **Attach** ([`attach_held_items`]) — spawn each resolved item's model as a child of the body's
//!   **attach-point joint entity** (via [`BoneAttach`], inserted by the visual attach), so it rides the
//!   hand/hip/back bone through every animation — the modern analogue of the client's attach-transform
//!   install (`0x47a380`). The item model itself is a static mesh: its origin
//!   *is* the grip, aligned by the attach bone's animated frame.
//!
//! Item models cache per **item display id** in [`ItemDisplays`] (a [`DisplayModel`] like creatures/
//! GameObjects use, resolved from `ItemDisplayInfo.dbc` into `Item\ObjectComponents\{Weapon,Shield}\`),
//! with the display's model texture bound to the model's runtime type-2 batches
//! ([`CharSkinSlot::Object`]).
//!
//! Three files, along the same seam the doc above describes: **`resolve`** (what should each unit
//! hold, and where — the descriptor read + the placement law), **`spawn`** (the sub-model children
//! that ride the bones), and this module, which owns what both share — the attach-id table, the
//! display cache, and the components they diff through.

use std::collections::HashMap;

use benilla_formats::ItemDisplayCatalog;
use bevy::prelude::*;

use super::{DisplayModel, ModelHandle};
use benilla_assets::m2_url;

mod resolve;
pub(in crate::entities) use resolve::placement;
pub(crate) use resolve::DressKey;
pub(super) use resolve::{resolve_corpse_equipment, resolve_equipment};
mod spawn;
pub(super) use spawn::attach_held_items;

/// The three held-item descriptor slots (vmangos `WeaponAttackType`): 0 mainhand · 1 offhand · 2 ranged.
const HELD_SLOTS: usize = 3;

/// A player's visible-item equipment slots for the held items (vmangos `EQUIPMENT_SLOT_MAINHAND/
/// OFFHAND/RANGED` = 15/16/17 — the `PLAYER_VISIBLE_ITEM_*` blocks are indexed by equipment slot).
const PLAYER_HELD_SLOTS: [u8; HELD_SLOTS] = [15, 16, 17];

/// M2 attachment-point ids (empirically pinned on `HumanMale.m2` — decision 0072): the drawn-hand
/// points, the shield forearm, and the sheathed family the client's `0x47a070` jump table selects
/// from (`K − (mainhand)` over `K ∈ {27, 31, 33}`, const 28 for the shield).
/// `pub(crate)` because the ids are a shared vocabulary, not this module's private numbering:
/// the model panes filter the reference's attach reset and probe hand occupancy by them
/// ([`crate::portrait::attach_reset`], [`crate::portrait::hand_grip`]).
pub(crate) mod attach_id {
    /// Left forearm — a *drawn* shield.
    pub(crate) const SHIELD: u16 = 0;
    /// Right/left shoulder (pivots at ∓0.21 Y, shoulder height) — the pauldron pair.
    pub(crate) const SHOULDER_RIGHT: u16 = 5;
    pub(crate) const SHOULDER_LEFT: u16 = 6;
    /// Head — the helm.
    pub(crate) const HELM: u16 = 11;
    /// Right hand — the drawn mainhand (and a drawn ranged weapon).
    pub(crate) const HAND_RIGHT: u16 = 1;
    /// Left hand — a drawn non-shield offhand.
    pub(crate) const HAND_LEFT: u16 = 2;
    /// Right/left shoulder-blade — the stowed two-hander family (and a stowed ranged weapon).
    pub(crate) const BACK_RIGHT: u16 = 26;
    pub(crate) const BACK_LEFT: u16 = 27;
    /// Centre back — the stowed shield.
    pub(crate) const SHIELD_BACK: u16 = 28;
    /// Lower-back pair — the stowed staff family.
    pub(crate) const BACK_LOWER_MAIN: u16 = 30;
    pub(crate) const BACK_LOWER_OFF: u16 = 31;
    /// Hip pair — the stowed one-hander family (mainhand on the *left* hip, drawn across the body).
    pub(crate) const HIP_MAIN: u16 = 32;
    pub(crate) const HIP_OFF: u16 = 33;
    /// HandArrow (35, bone 126 — flag-0x04 ignore-parent-rotation) — the in-hand nocked arrow's
    /// ONE body-bone attach (`0x712f70(body, 0x23)` from `0x60ba30`/the `$BWP` BowPull handler;
    /// bow/wand only). The old Special2/Special3 (0x18/0x19) reading is REFUTED — those are the
    /// `0x479f40` model-DIRECTORY selectors (`Item\ObjectComponents\Ammo\` vs `…\Weapon\`), never
    /// attach ids.
    pub(crate) const HAND_ARROW: u16 = 0x23;
    /// The quiver-on-back attach (`0x479c50`): the worn ammo container's model parents at M2
    /// attachment id 26 — the same point the stowed two-hander family uses.
    pub(crate) const QUIVER: u16 = 26;
}

/// Item-display rendering: the `ItemDisplayInfo.dbc` catalog (held models + armor region textures,
/// decisions 0072/0074) + a per-display [`DisplayModel`] cache for held items. Optional resource —
/// if the DBC fails to load, units hold nothing and armor stays unpainted.
#[derive(Resource)]
pub(crate) struct ItemDisplays {
    /// `pub(crate)`: the container feed ([`crate::ui_items`]) reads the icon column off the same
    /// parse — one catalog resource serves both the world and the bags.
    pub(crate) catalog: ItemDisplayCatalog,
    /// Keyed by `(display id, model kind)` — a helm display resolves to a different file per
    /// race/sex, a shoulder display to a left/right pair (0074 slice 3c).
    pub(super) models: HashMap<(u32, ItemModelKind), DisplayModel>,
}

#[cfg(test)]
impl ItemDisplays {
    /// The icon-only test seam: a synthetic catalog with an empty model cache — for the UI feeds
    /// that read nothing off this resource but the icon column (the action bar, the bags). They
    /// live outside this module, so `models` is not theirs to build.
    pub(crate) fn icons_for_tests(catalog: ItemDisplayCatalog) -> Self {
        ItemDisplays {
            catalog,
            models: HashMap::new(),
        }
    }
}

/// A player's worn-equipment display ids by **bodyslot − 2** (shirt, chest, belt, pants, boots,
/// wrist, gloves, tabard — the armor-composite slots, decision 0074), `0` = empty. `settled` means
/// every non-empty visible-item entry has resolved through the template cache (hit or recorded
/// miss) — the attach path waits for it so a player composites dressed, not naked-then-flicker.
/// Players only; a character-model NPC's armor ships pre-baked (decision 0060).
#[derive(Component, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) struct Equipment {
    pub(crate) bodyslots: [u32; 8],
    /// The back slot's display id (0 = no cloak) — a geoset + runtime cape texture, no body region.
    pub(crate) cloak: u32,
    /// The head slot's display id (0 = no helm) — an attach sub-model, plus the HelmetGeosetVisData
    /// hide-masks that tuck hair/facial/ears under it (`0x4799a0`).
    pub(crate) helm: u32,
    /// The wearer's guild tabard (decision 1704) — the emblem five, joined off their own PUBLIC
    /// `PLAYER_GUILDID` against the guild identity cache. `None` for a guildless player **and**
    /// for a guild whose `CMSG_GUILD_QUERY` has not answered yet; the tabard shows its own art
    /// until it does.
    ///
    /// It lives here rather than in a component of its own because it is part of *what this body
    /// wears*: the composite key includes it, so the re-dress diff below is the one place a change
    /// of guild, of tabard, and a late-arriving identity all converge — no second latch.
    pub(crate) emblem: Option<benilla_formats::GuildEmblem>,
    /// The tabard designer's preview (decision 1977, `[cc+0xc]` via `0x5e07d0`): set on the
    /// LOCAL player's body while the designer is open, so the body wears the tabard geoset over an
    /// empty slot and the composite paints `emblem` (the design under preview) onto it.
    pub(crate) tabard_preview: bool,
    pub(crate) settled: bool,
}

/// The [`Equipment`] a player's attached visual was **dressed** with (its composite key's equip
/// half). [`super::attach::redress_player_looks`] diffs it against the live resolution and re-dresses
/// the standing visual in place on a change — the reference's own shape, and the reason a weapon
/// glow no longer blinks when a belt is swapped (decision 0835, superseding 0074's teardown).
#[derive(Component)]
pub(in crate::entities) struct AppliedEquipment(pub(in crate::entities) Equipment);

/// Marks a unit whose visual was torn down and is being rebuilt for something **other** than a
/// spawn: the re-attach skips the appear-fade. A mount transition ([`super::mount`]) and a live
/// display swap ([`super::live_display`]) are the two remaining tear-downs; a gear change is not one
/// of them any more (decision 0835).
#[derive(Component)]
pub(super) struct Reattached;

/// Player equipment slots feeding the armor composite → their bodyslot−2 index (decision 0074):
/// shirt(3) chest(4) waist(5) legs(6) feet(7) wrists(8) hands(9) tabard(18). The visible-item block
/// is indexed by equipment slot, so no invType mapping is needed on this path.
const COMPOSITE_SLOTS: [(u8, usize); 8] = [
    (3, 0),
    (4, 1),
    (5, 2),
    (6, 3),
    (7, 4),
    (8, 5),
    (9, 6),
    (18, 7),
];

/// The attach sub-model slots a unit shows this frame — the three held items (mainhand/offhand/
/// ranged) plus the helm, the shoulder pair (0074 slice 3c), and the nocked ammo, each an item
/// display + model variant + the body attachment point it hangs from. Recomputed by
/// [`resolve_equipment`] and diffed by [`attach_held_items`], so equipment/sheath changes
/// re-spawn only on an actual change.
#[derive(Component, Default, Clone, PartialEq, Eq)]
pub(super) struct HeldItems {
    slots: [Option<HeldSlot>; ATTACH_SLOTS],
}

/// Total attach sub-model slots: the 3 held + helm + shoulder L/R + the nocked ammo + the
/// worn quiver (self-only, ranged-drawn — `0x611e10`).
pub(super) const ATTACH_SLOTS: usize = 8;

/// **What the disarm reflex left attached** (decision 1863). The reference's attachment state is
/// built by *events*, not recomputed from the descriptor, and disarm is where that difference
/// becomes visible — so this is the one piece of latched attachment state the equipment layer
/// keeps.
///
/// `UNIT_FIELD_FLAGS`' change reflex `0x5ff580` runs when the DISARM bit **changes**
/// (`0x5ff619 test edi,eax`), for observed remote units as well as our own body, and when the bit
/// goes up it takes the hidden hand's weapon **off the hand** — `0x5ff676 call
/// 0x47a310(model, 0xf, 0, 0)`, an unlink of every child at that attachment plus an `OpenHand` —
/// but only while the sheath state `[unit+0xd40]` is non-zero, i.e. while the weapon is drawn.
/// Then nothing puts it back: every attach site (`0x60b770`, `0x605da0`, `0x60b590`) is dominated
/// by a `GetWeapon(slot, 0)` that now reads NULL, so the weapon can be neither re-attached nor
/// moved until the flag clears.
///
/// The observable shape, and why a plain "hide it while disarmed" is wrong in both directions:
///
/// | when the flag went up | what stays on screen |
/// |---|---|
/// | weapon **drawn** | nothing — it leaves the hand |
/// | weapon **stowed** | it stays on the back/hip, and stays there even if the unit then engages |
/// | we never saw the edge (the unit streamed in already disarmed) | nothing was ever attached |
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) struct DisarmFreeze {
    /// The held slot the ladder hid at the edge — 0 mainhand, 1 offhand.
    slot: u8,
    /// The display id that was in that hand. A *different* item has no edge of its own and so was
    /// never attached (the reference's attach sites all decline while the flag is up).
    display: u32,
    /// Where the reflex left it, or `None` when it was in the hand and got detached.
    attach: Option<u16>,
}

impl DisarmFreeze {
    /// Record what the reflex left, at the edge.
    pub(super) fn new(slot: u8, display: u32, attach: Option<u16>) -> Self {
        DisarmFreeze {
            slot,
            display,
            attach,
        }
    }

    /// Where `display` still hangs in `slot`, or `None` — including when this freeze is about a
    /// different hand or a different item, neither of which the reflex ever attached.
    pub(super) fn attach_for(&self, slot: u8, display: u32) -> Option<u16> {
        (self.slot == slot && self.display == display)
            .then_some(self.attach)
            .flatten()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct HeldSlot {
    display: u32,
    kind: ItemModelKind,
    attach: u16,
    /// The `ItemVisuals.dbc` id this item glows with — its display's intrinsic visual, else its
    /// first enchant's (decision 0805, [`super::item_glow`]). `0` on the overwhelming majority.
    /// Part of the diff key on purpose: applying or losing an enchant is a different MODEL set, so
    /// the item is rebuilt and the glow follows. (A sheath change is not — same item, new attach
    /// point: that one MOVES the spawned root instead, decision 0826.)
    visual: i32,
}

impl HeldSlot {
    /// The same item model, wherever it hangs — the test that separates a **move** (the sheath
    /// swap: one item, a new attach point, so the spawned root is re-parented and everything
    /// riding it comes along — decision 0826) from a **rebuild** (a different display, model
    /// variant, or glow, which is a different model and must be built from scratch).
    fn same_item(&self, other: &Self) -> bool {
        self.display == other.display && self.kind == other.kind && self.visual == other.visual
    }
}

/// Which of an item display's models a slot shows, and where its file lives (decision 0074 slice 3c
/// — all pinned empirically against the real MPQ listing):
/// `Item\ObjectComponents\{Weapon,Shield}\model[0]` for held items; `Shoulder\model[0]`/`model[1]`
/// for the left/right pauldron (each with its own `model_texture` column); `Head\<stem>_<Ra><S>.m2`
/// for helms — per-race/sex files, prefix by race id (Hu Or Dw Ni Sc Ta Gn Tr), M/F by sex.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum ItemModelKind {
    Weapon,
    Shield,
    ShoulderLeft,
    ShoulderRight,
    Helm {
        race: u8,
        sex: u8,
    },
    /// A nocked-ammo model ([`crate::creature_anim::NockedAmmo`]) — the missile module's shape
    /// rule (its module docs): a display with `model[0]` is a thrown weapon in `Weapon\`; one
    /// with only `model[1]` is an arrow/bullet in `Ammo\`.
    Ammo,
    /// The worn ammo container on the back (`0x479c50`): `Quiver\model[0]` + its own
    /// `model_texture[0]` skin, parented at attachment 26 while the ranged weapon is drawn.
    Quiver,
}

/// The per-instance bone-riding surface, inserted at visual attach (decision 0072): the model's
/// attachment points and event markers (id → bone + Bevy-space local offset). **Pure model data**
/// — the anchor *entities* live in `RigPose.anchors` and spawn on first consumer (decision 1355):
/// a rider that needs a frame to parent under resolves `points`/`markers` here, then
/// `RigPose::anchor_for(bone)`; a reader that only needs a position goes straight to
/// `RigPose::posed_point(bone, offset)` and touches no entity at all.
#[derive(Component)]
pub(crate) struct BoneAttach {
    /// Attachment id → `(bone index, bevy-space offset from the bone's bind pivot)`.
    pub(crate) points: HashMap<u16, (u16, Vec3)>,
    /// Animation-event marker 4CC → the same `(bone, offset)` shape — first record per ident
    /// (the client's `0x7130e0` first-match scan). The missile launch points: `$CSL`/`$CSR`/`$CST`
    /// (casting hand left/right/two-hand, `0x60c9b0`'s cascade) and `$BWR` (ranged release).
    pub(crate) markers: HashMap<[u8; 4], (u16, Vec3)>,
}

/// The held-item children currently spawned for a unit: the [`HeldItems`] they were built from (the
/// diff key) + the spawned root entity **per slot** — indexed, not a flat list, because the diff is
/// per slot (decision 0826): a slot that didn't change keeps its root, and a slot that only changed
/// attach point has its root moved. Only a slot whose item really changed is despawned and rebuilt.
#[derive(Component, Default)]
pub(crate) struct HeldAttached {
    applied: HeldItems,
    spawned: [Option<Entity>; ATTACH_SLOTS],
}

impl HeldAttached {
    /// The spawned root per attach slot, read-only — for the **instruments** (`WOW_DRESS_CENSUS`),
    /// which need the *visual* truth and not just the resolution: "is a helm model actually hanging
    /// off this body right now" is a different question from "did we resolve a helm display id",
    /// and a report about a piece of gear showing when it should not (B123) turns on the gap
    /// between them. Nothing in gameplay reads this — the diff owns the array (decision 0026's
    /// dev seam: dev may see anything, nothing may depend on dev).
    pub(crate) fn spawned_slots(&self) -> &[Option<Entity>; ATTACH_SLOTS] {
        &self.spawned
    }

    /// The spawned roots as they would be after an attach pass — the one field a consumer test
    /// needs, without standing up the resolver, the display cache and the attach chain to get it.
    #[cfg(test)]
    pub(crate) fn with_spawned(spawned: [Option<Entity>; ATTACH_SLOTS]) -> Self {
        Self {
            applied: HeldItems::default(),
            spawned,
        }
    }
}

/// The attach-slot names, in [`HeldAttached::spawned_slots`] order — the instruments' labels for
/// the eight slots [`resolve_equipment`](super::equipment::resolve) fills.
pub(crate) const ATTACH_SLOT_NAMES: [&str; ATTACH_SLOTS] = [
    "main", "off", "ranged", "helm", "shL", "shR", "ammo", "quiver",
];

/// The glow id of a slot the reference never glows: **helm (attach 11), shoulders (5/6) and the
/// quiver (26) push a literal `0`** into the item-attach primitive `0x4798c0` — byte-read at
/// `0x479aa2`/`0x479e5f`/`0x479cf2`/`0x479db4` (decision 0805). Only the weapon/shield hand attach
/// (`0x47a200`) and the ranged/ammo builder pass a display's real visual, so a glowing *shoulder*
/// display — three robes and a chest carry one — is inert in 1.12, and inert here.
const NO_GLOW: i32 = 0;

/// Ensure `display` has a [`DisplayModel`] entry: resolve its ItemDisplayInfo row to the
/// `Item\ObjectComponents\{Weapon,Shield}\` model + its runtime object texture (the display's model
/// texture — an independently-named BLP in the same folder, never derived from the model name).
pub(in crate::entities) fn ensure_item_model(
    held: &mut ItemDisplays,
    display: u32,
    kind: ItemModelKind,
    asset_server: &AssetServer,
) {
    if held.models.contains_key(&(display, kind)) {
        return;
    }
    // Per-kind: the ObjectComponents directory, which model/texture column, and the helm's
    // per-race/sex filename (`<stem>_<Ra><S>.m2` — prefix by race id, empirically pinned, 0074).
    // Ammo follows the missile module's shape rule: a `model[0]` row is a thrown weapon
    // (`Weapon\`), a `model[1]`-only row an arrow/bullet (`Ammo\`).
    let (dir_name, col) = match kind {
        ItemModelKind::Weapon => ("Weapon", 0),
        ItemModelKind::Shield => ("Shield", 0),
        ItemModelKind::ShoulderLeft => ("Shoulder", 0),
        ItemModelKind::ShoulderRight => ("Shoulder", 1),
        ItemModelKind::Helm { .. } => ("Head", 0),
        ItemModelKind::Ammo => {
            if held
                .catalog
                .get(display)
                .is_some_and(|d| d.model[0].is_some())
            {
                ("Weapon", 0)
            } else {
                ("Ammo", 1)
            }
        }
        ItemModelKind::Quiver => ("Quiver", 0),
    };
    let dir = format!("Item\\ObjectComponents\\{dir_name}");
    let dm = match held.catalog.get(display) {
        Some(d) if d.model[col].is_some() => {
            let mut model = d.model[col].clone().unwrap();
            if let ItemModelKind::Helm { race, sex } = kind {
                // Race id 1–8 → Hu Or Dw Ni Sc Ta Gn Tr; M/F by sex. All 16 variants ship for
                // every helm stem (verified against the full MPQ listing).
                const RACE_PREFIX: [&str; 8] = ["Hu", "Or", "Dw", "Ni", "Sc", "Ta", "Gn", "Tr"];
                let prefix = RACE_PREFIX[(race.clamp(1, 8) - 1) as usize];
                let letter = if sex == 1 { 'F' } else { 'M' };
                let stem = model.strip_suffix(".m2").unwrap_or(&model).to_string();
                model = format!("{stem}_{prefix}{letter}.m2");
            }
            let disp_id = display;
            debug!(
                "item model display {disp_id} → {dir}\\{model} (tex {:?})",
                d.model_texture[col]
            );
            DisplayModel {
                handle: ModelHandle::M2(asset_server.load(m2_url(&format!("{dir}\\{model}")))),
                // The runtime object skin (bound to the model's type-2 batches) — its own basename in
                // the same folder, never derived from the model name (decision 0072's naming trap).
                object_texture: d.model_texture[col].clone(),
                dir,
                ..super::empty_shell()
            }
        }
        _ => super::empty_display(),
    };
    held.models.insert((display, kind), dm);
}
