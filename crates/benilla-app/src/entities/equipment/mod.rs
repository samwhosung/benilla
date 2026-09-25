//! Equipment visuals: `resolve` decides what each unit holds and wears and where, and `spawn`
//! hangs the item models from the body's attachment joints (the reference's attach install,
//! `0x47a380`). A creature carries display ids (`UNIT_VIRTUAL_ITEM_SLOT_DISPLAY`), a player only
//! item entries (`PLAYER_VISIBLE_ITEM_*`), resolved through the templates as the reference's
//! ItemCache does.

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

/// The held-item descriptor slots, in vmangos `WeaponAttackType` order: mainhand, offhand, ranged.
const HELD_SLOTS: usize = 3;

/// The held items' equipment slots, which index the `PLAYER_VISIBLE_ITEM_*` blocks.
const PLAYER_HELD_SLOTS: [u8; HELD_SLOTS] = [15, 16, 17];

/// M2 attachment ids. A stowed item's is `0x47a070`'s: K - 1 for the mainhand and K for the
/// offhand, K = 27 two-hander, 31 staff, 33 one-hander; 28 for a shield.
pub(crate) mod attach_id {
    /// Left forearm: a drawn shield.
    pub(crate) const SHIELD: u16 = 0;
    pub(crate) const SHOULDER_RIGHT: u16 = 5;
    pub(crate) const SHOULDER_LEFT: u16 = 6;
    pub(crate) const HELM: u16 = 11;
    /// The drawn mainhand, and a drawn gun, crossbow, wand or thrown weapon (`0x611e10`).
    pub(crate) const HAND_RIGHT: u16 = 1;
    /// A drawn offhand other than a shield, and a drawn bow.
    pub(crate) const HAND_LEFT: u16 = 2;
    pub(crate) const BACK_RIGHT: u16 = 26;
    pub(crate) const BACK_LEFT: u16 = 27;
    pub(crate) const SHIELD_BACK: u16 = 28;
    pub(crate) const BACK_LOWER_MAIN: u16 = 30;
    pub(crate) const BACK_LOWER_OFF: u16 = 31;
    /// The mainhand's hip is the left one.
    pub(crate) const HIP_MAIN: u16 = 32;
    pub(crate) const HIP_OFF: u16 = 33;
    /// HandArrow, the nocked arrow's one body attach (`0x712f70(body, 0x23)` in `0x60ba30` and the
    /// `$BWP` handler). The 0x18/0x19 that `0x479f40` takes pick a model directory, not an attach.
    pub(crate) const HAND_ARROW: u16 = 0x23;
    /// The worn quiver's attachment (`0x479c50`), the stowed mainhand two-hander's point.
    pub(crate) const QUIVER: u16 = 26;
}

/// The `ItemDisplayInfo.dbc` catalog and the item model cache; absent if the DBC fails to load.
#[derive(Resource)]
pub(crate) struct ItemDisplays {
    pub(crate) catalog: ItemDisplayCatalog,
    /// Keyed by model kind too: a helm display has a file per race and sex, a shoulder a pair.
    pub(super) models: HashMap<(u32, ItemModelKind), DisplayModel>,
}

#[cfg(test)]
impl ItemDisplays {
    pub(crate) fn icons_for_tests(catalog: ItemDisplayCatalog) -> Self {
        ItemDisplays {
            catalog,
            models: HashMap::new(),
        }
    }
}

/// A player's worn display ids, 0 for none (an NPC's armor is baked into its display);
/// `bodyslots` by bodyslot - 2, shirt to tabard. `settled` holds once every worn entry has a
/// template answer, so the first composite is dressed rather than naked for a frame.
#[derive(Component, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) struct Equipment {
    pub(crate) bodyslots: [u32; 8],
    /// A geoset and a runtime cape texture, no body region.
    pub(crate) cloak: u32,
    /// An attach model plus the masks that hide hair, facial hair and ears (`0x4799a0`).
    pub(crate) helm: u32,
    /// The guild emblem off `PLAYER_GUILDID`; `None` for no guild, or until its query answers.
    pub(crate) emblem: Option<benilla_formats::GuildEmblem>,
    /// The tabard designer's preview on our own body (`[cc+0xc]`, `0x5e07d0`).
    pub(crate) tabard_preview: bool,
    pub(crate) settled: bool,
}

/// The [`Equipment`] a player's visual was dressed with; a change re-dresses it in place.
#[derive(Component)]
pub(in crate::entities) struct AppliedEquipment(pub(in crate::entities) Equipment);

/// A visual rebuilt for a mount or display swap, not a spawn: it skips the appear-fade.
#[derive(Component)]
pub(super) struct Reattached;

/// The armor composite's equipment slots and their `Equipment::bodyslots` index.
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

/// The item models a unit shows this frame, in [`ATTACH_SLOT_NAMES`] order.
#[derive(Component, Default, Clone, PartialEq, Eq)]
pub(super) struct HeldItems {
    slots: [Option<HeldSlot>; ATTACH_SLOTS],
}

pub(super) const ATTACH_SLOTS: usize = 8;

/// What the disarm reflex left attached: the reference builds attachments from events, and this is
/// the one such state kept across frames. `0x5ff580` runs on any unit whose disarm bit changes
/// (`0x5ff619`); as it rises, a drawn weapon leaves the hidden hand (`0x5ff676`,
/// `0x47a310(model, 0xf, 0, 0)`) and a stowed one stays, and no attach site (`0x60b770`,
/// `0x605da0`, `0x60b590`) moves it while `GetWeapon(slot, 0)` reads NULL.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) struct DisarmFreeze {
    /// The hand hidden at the edge: 0 mainhand, 1 offhand.
    slot: u8,
    /// The item in that hand; a different one was never attached while the flag is up.
    display: u32,
    /// Where the reflex left it; `None` when it was detached from the hand.
    attach: Option<u16>,
}

impl DisarmFreeze {
    pub(super) fn new(slot: u8, display: u32, attach: Option<u16>) -> Self {
        DisarmFreeze {
            slot,
            display,
            attach,
        }
    }

    /// Where `display` still hangs in `slot`; `None` for any other hand or item.
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
    /// The `ItemVisuals.dbc` glow, the display's own else its first enchant's; usually 0.
    visual: i32,
}

impl HeldSlot {
    /// The same model wherever it hangs: a change of attach point alone is a move, not a rebuild.
    fn same_item(&self, other: &Self) -> bool {
        self.display == other.display && self.kind == other.kind && self.visual == other.visual
    }
}

/// Which of an item display's models a slot shows; `ensure_item_model` maps each to its file.
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
    /// A nocked-ammo model: a display with `model[0]` is a thrown weapon under `Weapon\`, one with
    /// only `model[1]` an arrow or bullet under `Ammo\`.
    Ammo,
    /// The worn ammo container (`0x479c50`), on the back while the ranged weapon is drawn.
    Quiver,
}

/// A body model's attachment points and event markers, each a bone and a Bevy-space offset from
/// its bind pivot; the joint entities are `RigPose::anchor_for`'s.
#[derive(Component)]
pub(crate) struct BoneAttach {
    pub(crate) points: HashMap<u16, (u16, Vec3)>,
    /// First record per FourCC (`0x7130e0`): the missile launch points `$CSL`/`$CSR`/`$CST`
    /// (`0x60c9b0`) and `$BWR`.
    pub(crate) markers: HashMap<[u8; 4], (u16, Vec3)>,
}

/// The item models spawned for a unit: the [`HeldItems`] they were built from and each slot's root.
#[derive(Component, Default)]
pub(crate) struct HeldAttached {
    applied: HeldItems,
    spawned: [Option<Entity>; ATTACH_SLOTS],
}

impl HeldAttached {
    /// The spawned root per attach slot: the models actually on the body, not the resolution.
    pub(crate) fn spawned_slots(&self) -> &[Option<Entity>; ATTACH_SLOTS] {
        &self.spawned
    }

    #[cfg(test)]
    pub(crate) fn with_spawned(spawned: [Option<Entity>; ATTACH_SLOTS]) -> Self {
        Self {
            applied: HeldItems::default(),
            spawned,
        }
    }
}

/// The attach slots' names, in [`HeldAttached::spawned_slots`] order.
pub(crate) const ATTACH_SLOT_NAMES: [&str; ATTACH_SLOTS] = [
    "main", "off", "ranged", "helm", "shL", "shR", "ammo", "quiver",
];

/// The glow of a slot the reference never lights: the helm, shoulder and quiver builders pass a
/// literal 0 to `0x4798c0` (`0x479aa2`, `0x479e5f`, `0x479cf2`, `0x479db4`); only the hand attach
/// (`0x47a200`) and the ranged and ammo builder pass the display's visual.
const NO_GLOW: i32 = 0;

/// Ensure `display` has a [`DisplayModel`] for `kind`; its texture is the BLP the display row
/// names, never derived from the model name.
pub(in crate::entities) fn ensure_item_model(
    held: &mut ItemDisplays,
    display: u32,
    kind: ItemModelKind,
    asset_server: &AssetServer,
) {
    if held.models.contains_key(&(display, kind)) {
        return;
    }
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
                // Race 1-8 is Hu Or Dw Ni Sc Ta Gn Tr, sex M or F.
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
                // The runtime object skin, bound to the model's type-2 batches.
                object_texture: d.model_texture[col].clone(),
                dir,
                ..super::empty_shell()
            }
        }
        _ => super::empty_display(),
    };
    held.models.insert((display, kind), dm);
}
