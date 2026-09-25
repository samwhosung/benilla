//! Equipment resolution: what each unit holds and wears this frame, and where each item hangs.

use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::creature_anim::{
    HandGrip, NockLatch, NockedAmmo, VisualSheath, Wielded, UNIT_FLAG_DISARMED,
};
use crate::items::Items;
use crate::net::{NetCommands, NetEntity, ObjectStore};

use super::super::item_glow::{self, ItemGlows};
use super::super::Creatures;
use super::{
    attach_id, ensure_item_model, DisarmFreeze, Equipment, HeldItems, HeldSlot, ItemDisplays,
    ItemModelKind, ATTACH_SLOTS, COMPOSITE_SLOTS, HELD_SLOTS, NO_GLOW, PLAYER_HELD_SLOTS,
};

/// Where one held slot's item hangs, if it shows: a melee item drawn in sheath state 1, else by
/// its own sheath type (`0x47a070`); a ranged weapon only in state 2, a bow in the left hand and
/// the rest in the right (`0x611e10`), and detached when stowed (`0x7130a0`).
pub(in crate::entities) fn placement(
    slot: usize,
    inv_type: u32,
    item_sheath: u8,
    unit_sheath: u8,
) -> Option<u16> {
    use attach_id::*;
    let shield = inv_type == 14; // INVTYPE_SHIELD
    match slot {
        2 => (unit_sheath == 2).then_some(if inv_type == 15 {
            HAND_LEFT // INVTYPE_RANGED: bows
        } else {
            HAND_RIGHT
        }),
        0 | 1 if unit_sheath == 1 => Some(match (slot, shield) {
            (0, _) => HAND_RIGHT,
            (_, true) => SHIELD,
            (_, false) => HAND_LEFT,
        }),
        0 | 1 => match item_sheath {
            1 => Some(if slot == 0 { BACK_RIGHT } else { BACK_LEFT }),
            2 => Some(if slot == 0 {
                BACK_LOWER_MAIN
            } else {
                BACK_LOWER_OFF
            }),
            3 => Some(if slot == 0 { HIP_MAIN } else { HIP_OFF }),
            4 => Some(SHIELD_BACK),
            _ => None,
        },
        _ => None,
    }
}

/// One held slot's enchant ids folded for [`DressKey`], over the seven `CGItem` slots (`0x62ec70`).
fn enchant_fold(
    s: &benilla_protocol::messages::ObjectFields,
    kind: EntityKind,
    slot: usize,
) -> i32 {
    if kind != EntityKind::Player {
        return 0;
    }
    (0..7u8)
        .filter_map(|j| s.player_visible_item_enchant(PLAYER_HELD_SLOTS[slot], j))
        .fold(0i32, |acc, e| acc.wrapping_mul(31).wrapping_add(e as i32))
}

/// The nocked ammo's attachment (`0x60ba30`): HandArrow, once the `$BWP` latch is set
/// (`[+0xd58] & 0x4000`, [`NockLatch`]). Only a bow gets there: gun and crossbow return early
/// (`gunXbow`), thrown fails the `== 0x18` gate, and a wand's Shoot has no ammo item.
fn ammo_attach(ranged_inv_type: Option<u32>, nock_latched: bool) -> Option<u16> {
    const INVTYPE_RANGED_BOW: u32 = 0x0f;
    (ranged_inv_type == Some(INVTYPE_RANGED_BOW) && nock_latched).then_some(attach_id::HAND_ARROW)
}

/// One item's glow id, requesting the glow's models; 0 when the glow DBCs are absent.
fn resolve_glow(
    glows: Option<&mut ItemGlows>,
    enchant_rows: Option<&benilla_formats::EnchantCatalog>,
    displays: &ItemDisplays,
    display: u32,
    enchants: impl IntoIterator<Item = u32>,
    asset_server: &AssetServer,
) -> i32 {
    let Some(glows) = glows else {
        return NO_GLOW;
    };
    let base = displays.catalog.get(display).map_or(0, |d| d.item_visual);
    let visual = item_glow::effective_visual(glows, enchant_rows, base, enchants);
    item_glow::ensure_glow_models(glows, visual, asset_server);
    visual
}

/// The resolve inputs outside the descriptor and global caches. Skipping a unit on them is sound
/// only while every other input of the rebuild is a store field or a catalog fixed after load.
#[derive(Component, Clone, Copy, PartialEq)]
pub(in crate::entities) struct ResolveKey {
    committed_sheath: u8,
    visual_sheath: Option<[u8; 2]>,
    nocked: Option<u32>,
    latched: bool,
}

/// The dress half of the resolve, one of a model widget's re-take triggers (`0x5dee30`,
/// `crate::portrait::SnapKey`). Its weapons are read above [`placement`], so a sheath that shows
/// or hides one does not move it. Left out: the nocked ammo and quiver, which raise no model event
/// (`0x60ba30` reaches no queue site), and what no sheath moves, read off the mirrored geometry.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DressKey {
    /// The body the widget duplicates.
    pub(crate) display_id: Option<u32>,
    /// Mainhand, offhand, ranged: the display id, model kind and folded enchant ids.
    pub(crate) held: [Option<(u32, ItemModelKind, i32)>; HELD_SLOTS],
    /// Every placed weapon's model has built parts. Deviation: the reference copies a finished
    /// model, ours stream in, so while false the pane keeps re-taking and a late weapon is baked.
    pub(crate) held_ready: bool,
}

/// Resolve every unit's held and worn items, skipping one whose descriptor, [`ResolveKey`] and
/// the global epochs all held still.
#[allow(clippy::type_complexity)]
pub(in crate::entities) fn resolve_equipment(
    mut commands: Commands,
    units: Query<(
        Entity,
        &NetEntity,
        Ref<ObjectStore>,
        Option<&HeldItems>,
        Option<&Wielded>,
        Option<&Equipment>,
        Option<&VisualSheath>,
        Option<&crate::creature_anim::AnimDriver>,
        Option<&NockedAmmo>,
        Has<NockLatch>,
        Option<&ResolveKey>,
        Option<&DressKey>,
        Option<&DisarmFreeze>,
        Has<crate::net::SelfPlayer>,
    )>,
    held: Option<ResMut<ItemDisplays>>,
    templates: Res<Items>,
    net: Res<NetCommands>,
    asset_server: Res<AssetServer>,
    // A character-model NPC's helm, shoulders, race and sex (CreatureDisplayInfoExtra).
    creatures: Option<Res<Creatures>>,
    glows: Option<ResMut<ItemGlows>>,
    enchants: Option<Res<crate::items::Enchants>>,
    // The template epoch and guild identity generation at the last run: the gate's global half.
    mut last_epochs: Local<Option<(u64, u64)>>,
    // The object lookup and the item entities' change watch, one param under Bevy's limit of 16.
    item_objects: (crate::net::Objects, crate::items::ItemChanges),
    // Mutable because a miss sends the `CMSG_GUILD_QUERY` whose answer paints the tabard.
    mut guilds: Option<ResMut<crate::ui_guild::GuildState>>,
    tabard_design: Option<Res<crate::ui_tabard::TabardDesign>>,
) {
    let Some(mut held) = held else {
        return;
    };
    // A guild query answered after a player spawned changes their tabard, not their descriptor.
    let epochs = (
        templates.template_epoch(),
        guilds.as_ref().map_or(0, |g| g.identity_generation()),
    );
    // Drained every run, gate or not: `moved` consumes the removals.
    let (objects, mut item_changes) = item_objects;
    let items_moved = item_changes.moved();
    let caches_moved = last_epochs.replace(epochs) != Some(epochs)
        || items_moved
        || creatures.as_ref().is_some_and(|c| c.is_changed())
        || enchants.as_ref().is_some_and(|e| e.is_changed())
        || tabard_design.as_ref().is_some_and(|d| d.is_changed());
    let mut glows = glows;
    let enchant_rows = enchants.as_deref().map(|e| &e.0);
    for (
        entity,
        net_entity,
        store,
        current,
        current_wielded,
        current_equipment,
        visual_sheath,
        driver,
        nocked,
        nock_latched,
        current_key,
        current_dress,
        freeze,
        is_self,
    ) in &units
    {
        if !matches!(net_entity.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        let s = &store.0;
        // The anim layer's committed sheath state, else the raw byte until the driver runs.
        let committed = driver
            .and_then(|d| d.sheath_state())
            .or_else(|| s.unit_sheath_state())
            .unwrap_or(0);
        let key = ResolveKey {
            committed_sheath: committed,
            visual_sheath: visual_sheath.map(|v| v.0),
            nocked: nocked.map(|n| n.display_id),
            latched: nock_latched,
        };
        if !caches_moved && !store.is_changed() && current_key == Some(&key) {
            continue;
        }
        if current_key != Some(&key) {
            commands.entity(entity).insert(key);
        }
        // `PLAYER_FLAGS`' `HIDE_HELM 0x400` and `HIDE_CLOAK 0x800` are public, so another player's
        // choice shows here; a hidden piece is display id 0, so its model and `0x4799a0` masks go.
        let (hide_helm, hide_cloak) = (s.player_hides_helm(), s.player_hides_cloak());
        if net_entity.kind == EntityKind::Player {
            let mut eq = Equipment {
                settled: true,
                ..default()
            };
            for (slot, idx) in COMPOSITE_SLOTS {
                let Some(entry) = s.player_visible_item_entry(slot).filter(|e| *e != 0) else {
                    continue;
                };
                match templates.held(entry, &net) {
                    Some(t) => eq.bodyslots[idx] = t.display_info_id,
                    None => eq.settled = false, // asked; answer pending
                }
            }
            // The cloak (slot 14) and helm (slot 0). A hidden piece is still looked up and zeroed
            // after, so `settled` keeps its meaning and showing it again needs no round trip.
            if let Some(entry) = s.player_visible_item_entry(14).filter(|e| *e != 0) {
                match templates.held(entry, &net) {
                    Some(t) => eq.cloak = t.display_info_id,
                    None => eq.settled = false,
                }
            }
            if let Some(entry) = s.player_visible_item_entry(0).filter(|e| *e != 0) {
                match templates.held(entry, &net) {
                    Some(t) => eq.helm = t.display_info_id,
                    None => eq.settled = false,
                }
            }
            if hide_cloak {
                eq.cloak = 0;
            }
            if hide_helm {
                eq.helm = 0;
            }
            // The emblem, for every player; the composite paints it only on a flagged tabard.
            eq.emblem = guilds
                .as_deref_mut()
                .and_then(|g| crate::ui_guild::unit_guild_emblem(s, g, &net));
            // The designer's preview, our own body only (`[cc+0xc]`, the `0x47a610` install).
            if is_self {
                if let Some(design) = tabard_design.as_deref().and_then(|d| d.preview()) {
                    eq.emblem = Some(design);
                    eq.tabard_preview = true;
                }
            }
            if current_equipment != Some(&eq) {
                commands.entity(entity).insert(eq);
            }
        }
        // The sheath a slot is placed by: per arm during a draw or stow ceremony (`VisualSheath`),
        // each weapon moving at its own clip's `$SHL`/`$SHR` key rather than at the byte change.
        let sheath_of = |slot: usize, inv_type: u32| {
            visual_sheath.map_or(committed, |v| v.for_slot(slot, inv_type))
        };
        // A player in a non-character display (a form, a morph) resolves `Wielded` but attaches no
        // item models: the reference's attach lives on the character component (`0x47a0c0`).
        let char_component = net_entity.kind != EntityKind::Player
            || net_entity
                .display_id
                .and_then(|d| creatures.as_deref()?.models.get(&d))
                .is_none_or(|dm| dm.is_character_body);
        let mut slots: [Option<HeldSlot>; ATTACH_SLOTS] = [None; ATTACH_SLOTS];
        let mut wielded = Wielded {
            // Read where the reference's `GetWeapon` reads it (`0x5ec2b8`).
            disarmed: s.unit_flags() & UNIT_FLAG_DISARMED != 0,
            ..Wielded::default()
        };
        let mut ranged_inv_type = None;
        let mut worn: [Option<(u32, ItemModelKind, i32)>; HELD_SLOTS] = [None; HELD_SLOTS];
        // Pass 1, what each hand holds (`GetWeapon(slot, 1)`), all before the disarm ladder.
        let mut resolved: [Option<(u32, u32, u8, u8, u8, u8)>; HELD_SLOTS] = [None; HELD_SLOTS];
        for slot in 0..HELD_SLOTS {
            // (display id, inventory type, item sheath type, class, subclass, material) per slot.
            resolved[slot] = match net_entity.kind {
                EntityKind::Unit => {
                    let display = s.unit_virtual_item_display(slot as u8).filter(|d| *d != 0);
                    display.map(|d| {
                        let (class, subclass, material, inv) =
                            s.unit_virtual_item_info(slot as u8).unwrap_or((0, 0, 0, 0));
                        let sheath = s.unit_virtual_item_sheath(slot as u8).unwrap_or(0);
                        (d, inv as u32, sheath, class, subclass, material)
                    })
                }
                EntityKind::Player => s
                    .player_visible_item_entry(PLAYER_HELD_SLOTS[slot])
                    .filter(|e| *e != 0)
                    .and_then(|entry| templates.held(entry, &net))
                    .filter(|t| t.display_info_id != 0)
                    .map(|t| {
                        (
                            t.display_info_id,
                            t.inventory_type,
                            t.sheath as u8,
                            t.class as u8,
                            t.subclass as u8,
                            t.material as u8,
                        )
                    }),
                _ => None,
            };
            let Some((_, inv_type, item_sheath, class, subclass, material)) = resolved[slot] else {
                continue;
            };
            // The item's Material, the draw and stow sound's only key.
            wielded.materials[slot] = material;
            // The class pair the swing and ready anims select by; the mainhand's sheath type
            // picks the draw and stow anim (Sheath 89 at the back, HipSheath 90 at the hip).
            match slot {
                0 => {
                    wielded.main = Some((class, subclass));
                    wielded.main_sheath = item_sheath;
                }
                1 => {
                    wielded.off = Some((class, subclass));
                    wielded.off_sheath = item_sheath;
                }
                2 => {
                    wielded.ranged = Some((class, subclass));
                    wielded.ranged_sheath = item_sheath;
                    wielded.ranged_inv = inv_type;
                    ranged_inv_type = Some(inv_type);
                }
                _ => {}
            }
        }
        // The one hand `UNIT_FLAG_DISARMED` hides, if any.
        let hidden = wielded.disarmed_hand();
        // The rising edge (`0x5ff580` fires on a change); a unit first seen disarmed never had one.
        let disarm_edge = hidden.is_some() && current_wielded.is_some_and(|w| !w.disarmed);
        let mut next_freeze = match hidden {
            // The falling edge clears it: `0x5ff67d` re-attaches by the live sheath state.
            None => None,
            // The edge writes it below, from the placement the weapon had at that moment.
            Some(_) if disarm_edge => None,
            Some(_) => freeze.copied(),
        };
        // Pass 2, the models.
        for slot in 0..HELD_SLOTS {
            let Some((display, inv_type, item_sheath, _, _, _)) = resolved[slot] else {
                continue;
            };
            if !char_component {
                continue;
            }
            let kind = if inv_type == 14 {
                ItemModelKind::Shield
            } else {
                ItemModelKind::Weapon
            };
            // The dress, above the placement gate; its enchants come off the descriptor, since an
            // enchant raises a model event drawn or not and `visual` exists only for a placed slot.
            worn[slot] = Some((display, kind, enchant_fold(s, net_entity.kind, slot)));
            let live = placement(slot, inv_type, item_sheath, sheath_of(slot, inv_type));
            // The hidden hand's weapon stays where the disarm reflex left it (`0x5ff580`).
            let attach = if hidden == Some(slot) {
                if disarm_edge {
                    let kept = live
                        .filter(|a| !matches!(*a, attach_id::HAND_RIGHT | attach_id::HAND_LEFT));
                    next_freeze = Some(DisarmFreeze::new(slot as u8, display, kept));
                    kept
                } else {
                    next_freeze.and_then(|f| f.attach_for(slot as u8, display))
                }
            } else {
                live
            };
            let Some(attach) = attach else {
                continue;
            };
            ensure_item_model(&mut held, display, kind, &asset_server);
            let enchants = (net_entity.kind == EntityKind::Player).then(|| {
                // All seven `CGItem` slots (`0x62ec70`); 1.12 sends two, PERM and TEMP.
                (0..7u8).filter_map(|j| s.player_visible_item_enchant(PLAYER_HELD_SLOTS[slot], j))
            });
            let visual = resolve_glow(
                glows.as_deref_mut(),
                enchant_rows,
                &held,
                display,
                enchants.into_iter().flatten(),
                &asset_server,
            );
            slots[slot] = Some(HeldSlot {
                display,
                kind,
                attach,
                visual,
            });
        }
        // `held_ready` counts only placed slots: a stowed ranged weapon requests no model, and
        // counting it would keep the pane re-taking for ever.
        let dress = DressKey {
            display_id: net_entity.display_id,
            held: worn,
            held_ready: slots[..HELD_SLOTS].iter().flatten().all(|hs| {
                held.models
                    .get(&(hs.display, hs.kind))
                    .and_then(|dm| dm.parts.as_ref())
                    .is_some()
            }),
        };
        if current_dress != Some(&dress) {
            commands.entity(entity).insert(dress);
        }
        // The nocked ammo, per shot from `SMSG_SPELL_START` for any caster (`ammo_attach`).
        if let (true, Some(ammo), Some(attach)) = (
            char_component,
            nocked,
            ammo_attach(ranged_inv_type, nock_latched),
        ) {
            ensure_item_model(
                &mut held,
                ammo.display_id,
                ItemModelKind::Ammo,
                &asset_server,
            );
            // The ammo model's own visual (`0x479f40`, at `0x47a051`); ammo carries no enchants.
            let visual = resolve_glow(
                glows.as_deref_mut(),
                enchant_rows,
                &held,
                ammo.display_id,
                [],
                &asset_server,
            );
            slots[6] = Some(HeldSlot {
                display: ammo.display_id,
                kind: ItemModelKind::Ammo,
                attach,
                visual,
            });
        }
        // The quiver, players only, while the ranged weapon is drawn (`0x611e10`): the first
        // ItemClass 11 bag in the unit's own bag slots, at attachment 26. Those slots are private
        // fields (`UpdateFields_1_12_1.cpp:237`), so only our own body shows one; the reference is
        // inferred to match, as its scan (`0x611f2d`) reads the unit's own inventory.
        // Deviation: it attaches at that clip's `$SHL`, where the reference attaches it at the
        // draw's start, because it keys on the ranged slot's sheath so it arrives with the bow it
        // feeds; the two are a few hundred ms apart in one clip and never seen apart.
        if net_entity.kind == EntityKind::Player
            && char_component
            && sheath_of(2, ranged_inv_type.unwrap_or(0)) == 2
        {
            let mut quiver_display = None;
            for bag in 19u8..23 {
                let entry = s
                    .player_inv_slot(bag)
                    .and_then(|g| objects.object(g))
                    .and_then(|o| o.object_entry());
                let Some(t) = entry.and_then(|e| templates.held(e, &net)) else {
                    continue;
                };
                if t.class == 11 && t.display_info_id != 0 {
                    quiver_display = Some(t.display_info_id);
                    break;
                }
            }
            if let Some(display) = quiver_display {
                ensure_item_model(&mut held, display, ItemModelKind::Quiver, &asset_server);
                slots[7] = Some(HeldSlot {
                    display,
                    kind: ItemModelKind::Quiver,
                    attach: attach_id::QUIVER,
                    visual: NO_GLOW,
                });
            }
        }
        // Helm and shoulders: a player's visible items, or an NPC's CreatureDisplayInfoExtra.
        let head_shoulder: Option<(u32, u32, u8, u8)> = match net_entity.kind {
            EntityKind::Player if char_component => {
                let race = s.unit_race().unwrap_or(1);
                let sex = s.unit_gender().unwrap_or(0).min(1);
                let resolve = |slot: u8| {
                    s.player_visible_item_entry(slot)
                        .filter(|e| *e != 0)
                        .and_then(|entry| templates.held(entry, &net))
                        .map(|t| t.display_info_id)
                        .filter(|d| *d != 0)
                        .unwrap_or(0)
                };
                // Zero when hidden, the id the geoset half got, so the two cannot disagree.
                let helm = if hide_helm { 0 } else { resolve(0) };
                let shoulder = resolve(2);
                Some((helm, shoulder, race, sex))
            }
            EntityKind::Unit => net_entity
                .display_id
                .and_then(|disp| creatures.as_deref()?.models.get(&disp))
                .and_then(|dm| dm.npc_appearance.as_ref())
                .map(|npc| (npc.equipment[0], npc.equipment[1], npc.race, npc.sex.min(1))),
            _ => None,
        };
        if let Some((helm, shoulder, race, sex)) = head_shoulder {
            if helm != 0 {
                let kind = ItemModelKind::Helm { race, sex };
                ensure_item_model(&mut held, helm, kind, &asset_server);
                slots[3] = Some(HeldSlot {
                    display: helm,
                    kind,
                    attach: attach_id::HELM,
                    visual: NO_GLOW,
                });
            }
            if shoulder != 0 {
                for (kind, attach, idx) in [
                    (ItemModelKind::ShoulderLeft, attach_id::SHOULDER_LEFT, 4),
                    (ItemModelKind::ShoulderRight, attach_id::SHOULDER_RIGHT, 5),
                ] {
                    ensure_item_model(&mut held, shoulder, kind, &asset_server);
                    slots[idx] = Some(HeldSlot {
                        display: shoulder,
                        kind,
                        attach,
                        visual: NO_GLOW,
                    });
                }
            }
        }
        let next = HeldItems { slots };
        // A weapon at a hand's attachment closes that hand (`0x60b590`); a forearm shield does not.
        let grip = HandGrip {
            right: next
                .slots
                .iter()
                .flatten()
                .any(|s| s.attach == attach_id::HAND_RIGHT),
            left: next
                .slots
                .iter()
                .flatten()
                .any(|s| s.attach == attach_id::HAND_LEFT),
        };
        if current != Some(&next) {
            commands.entity(entity).insert((next, grip));
        }
        if current_wielded != Some(&wielded) {
            commands.entity(entity).insert(wielded);
        }
        // No freeze is a real state, so the falling edge removes the component.
        match next_freeze {
            Some(f) if freeze != Some(&f) => {
                commands.entity(entity).insert(f);
            }
            None if freeze.is_some() => {
                commands.entity(entity).remove::<DisarmFreeze>();
            }
            _ => {}
        }
    }
}

/// A corpse's dress: the reference loops its 19 `CORPSE_FIELD_ITEM` slots into `0x478cb0`
/// (`0x5d6260`), each `DisplayInfoID | InventoryType << 24` (vmangos `Player.cpp:4822`). Its own
/// `HIDE_HELM 0x08` and `HIDE_CLOAK 0x10` zero head and back (`0x5d6465`, `0x5d6470`); ranged is
/// skipped (`0x5d644e`) and the weapons are looked up as object guids that never resolve
/// (`0x5d649b`, `0x468460`). A bone pile wears nothing (`0x5d6291`) but gets an empty
/// [`Equipment`], which also gates the attach.
#[allow(clippy::type_complexity)]
pub(in crate::entities) fn resolve_corpse_equipment(
    mut commands: Commands,
    corpses: Query<(Entity, &NetEntity, Ref<ObjectStore>, Option<&Equipment>)>,
    held: Option<ResMut<ItemDisplays>>,
    asset_server: Res<AssetServer>,
    net: Res<NetCommands>,
    // Mutable because a miss sends the `CMSG_GUILD_QUERY` whose answer paints the crest.
    mut guilds: Option<ResMut<crate::ui_guild::GuildState>>,
    mut last_guild_epoch: Local<Option<u64>>,
) {
    let Some(mut held) = held else {
        return;
    };
    // A guild answer landing after the corpse streamed in changes its tabard, not its descriptor.
    let epoch = guilds.as_ref().map_or(0, |g| g.identity_generation());
    let guilds_moved = last_guild_epoch.replace(epoch) != Some(epoch);
    for (entity, net_entity, store, current) in &corpses {
        if net_entity.kind != EntityKind::Corpse {
            continue;
        }
        if !(store.is_changed() || current.is_none() || guilds_moved) {
            continue;
        }
        let s = &store.0;
        let bones = s.corpse_is_bones();
        // Every id here is final on arrival, so `settled` always holds.
        let mut eq = Equipment {
            settled: true,
            ..default()
        };
        if !bones {
            for (slot, idx) in COMPOSITE_SLOTS {
                eq.bodyslots[idx] = s.corpse_item(slot).map_or(0, |(display, _)| display);
            }
            if !s.corpse_hides_cloak() {
                eq.cloak = s.corpse_item(14).map_or(0, |(display, _)| display);
            }
            if !s.corpse_hides_helm() {
                eq.helm = s.corpse_item(0).map_or(0, |(display, _)| display);
            }
            // The crest off the corpse's own `CORPSE_FIELD_GUILD` (`0x5d6ec0`, at tabard slot 0x12
            // when its display has flag bit 0), through the living body's guild cache (`0x6d6d20`).
            eq.emblem = guilds
                .as_deref_mut()
                .and_then(|g| crate::ui_guild::corpse_guild_emblem(s, g, &net));
        }
        if current != Some(&eq) {
            commands.entity(entity).insert(eq);
        }
        // Helm and shoulders: ordinary `0x478cb0` slots in the reference, attachments here.
        let mut slots: [Option<HeldSlot>; ATTACH_SLOTS] = [None; ATTACH_SLOTS];
        if let (false, Some(look)) = (bones, s.corpse_look()) {
            let (race, sex) = (look.race, look.sex.min(1));
            if eq.helm != 0 {
                let kind = ItemModelKind::Helm { race, sex };
                ensure_item_model(&mut held, eq.helm, kind, &asset_server);
                slots[3] = Some(HeldSlot {
                    display: eq.helm,
                    kind,
                    attach: attach_id::HELM,
                    visual: NO_GLOW,
                });
            }
            // Shoulders have no hide flag: the reference gates only slots 0 and 0xe.
            if let Some((shoulder, _)) = s.corpse_item(2) {
                for (kind, attach, idx) in [
                    (ItemModelKind::ShoulderLeft, attach_id::SHOULDER_LEFT, 4),
                    (ItemModelKind::ShoulderRight, attach_id::SHOULDER_RIGHT, 5),
                ] {
                    ensure_item_model(&mut held, shoulder, kind, &asset_server);
                    slots[idx] = Some(HeldSlot {
                        display: shoulder,
                        kind,
                        attach,
                        visual: NO_GLOW,
                    });
                }
            }
        }
        commands.entity(entity).insert(HeldItems { slots });
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::super::HELD_SLOTS;
    use super::super::{Equipment, HeldItems, ItemDisplays};
    use super::{ammo_attach, attach_id, placement, resolve_corpse_equipment, resolve_equipment};
    use crate::items::Items;
    use crate::net::{ClientCommand, NetCommands, NetEntity, ObjectStore};
    use benilla_protocol::messages::{ItemInfo, ObjectFields};
    use benilla_protocol::EntityKind;

    #[test]
    fn corpse_dresses_from_its_own_snapshot() {
        use benilla_protocol::messages::ObjectType;

        /// `CORPSE_FIELD_ITEM` is field 13, each slot `DisplayInfoID | InventoryType << 24`.
        fn item(slot: u16, display: u32, inv: u32) -> (u16, u32) {
            (13 + slot, display | (inv << 24))
        }
        // Fields 32, 33 and 35 are `CORPSE_FIELD_BYTES_1` (race and sex in bytes 1 and 2),
        // `CORPSE_FIELD_BYTES_2` and `CORPSE_FIELD_FLAGS`.
        let dressed = |flags: u32| {
            let mut pairs = vec![
                item(0, 900, 1),
                item(2, 901, 3),
                item(4, 902, 5),
                item(14, 903, 16),
                item(15, 904, 13),
                item(17, 905, 15),
                (32, 1 << 8),
                (33, 0),
            ];
            if flags != 0 {
                pairs.push((35, flags));
            }
            ObjectStore(ObjectFields::from_pairs(&pairs).into_created(ObjectType::Corpse))
        };
        let run = |store: ObjectStore| {
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
            let (tx, _rx) = crossbeam_channel::unbounded::<ClientCommand>();
            app.insert_resource(NetCommands(tx));
            app.insert_resource(ItemDisplays::icons_for_tests(
                benilla_formats::ItemDisplayCatalog::from_displays(
                    std::collections::HashMap::new(),
                ),
            ));
            let corpse = app
                .world_mut()
                .spawn((
                    NetEntity {
                        kind: EntityKind::Corpse,
                        display_id: Some(49),
                        scale: 1.0,
                    },
                    store,
                ))
                .id();
            app.add_systems(Update, resolve_corpse_equipment);
            app.update();
            let w = app.world();
            (
                *w.get::<Equipment>(corpse).expect("corpse dressed"),
                w.get::<HeldItems>(corpse).expect("attach slots").clone(),
            )
        };

        let (eq, held) = run(dressed(0));
        // Chest, equipment slot 4, is composite index 1.
        assert_eq!(eq.bodyslots[1], 902, "chest off CORPSE_FIELD_ITEM[4]");
        assert_eq!(eq.helm, 900);
        assert_eq!(eq.cloak, 903);
        assert!(
            eq.settled,
            "a corpse's gear is final on arrival — never pending"
        );
        assert!(held.slots[3].is_some(), "the helm attaches");
        assert!(
            held.slots[4].is_some() && held.slots[5].is_some(),
            "the shoulder pair attaches off equipment slot 2"
        );
        assert!(
            held.slots[..HELD_SLOTS].iter().all(Option::is_none),
            "a corpse wears armour, never weapons"
        );

        // The corpse's own HIDE_HELM 0x08 and HIDE_CLOAK 0x10.
        let (eq, held) = run(dressed(0x08));
        assert_eq!(eq.helm, 0);
        assert_eq!(eq.cloak, 903, "the cloak bit is a different bit");
        assert!(held.slots[3].is_none(), "no helm model either");
        let (eq, _) = run(dressed(0x10));
        assert_eq!(eq.cloak, 0);
        assert_eq!(eq.helm, 900);

        // BONES 0x01: nothing is worn, however full the slots.
        let (eq, held) = run(dressed(0x01));
        assert_eq!(
            eq,
            Equipment {
                settled: true,
                ..default()
            }
        );
        assert!(held.slots.iter().all(Option::is_none));
    }

    /// InventoryType 15 is a bow, 25 thrown, 26 a gun, crossbow or wand.
    #[test]
    fn ranged_slot_hidden_unless_ranged_drawn() {
        for inv_type in [15, 25, 26] {
            for item_sheath in 0..=4u8 {
                assert_eq!(placement(2, inv_type, item_sheath, 0), None);
                assert_eq!(placement(2, inv_type, item_sheath, 1), None);
                let drawn = if inv_type == 15 {
                    attach_id::HAND_LEFT
                } else {
                    attach_id::HAND_RIGHT
                };
                assert_eq!(placement(2, inv_type, item_sheath, 2), Some(drawn));
            }
        }
    }

    #[test]
    fn melee_slots_stow_by_item_sheath_type() {
        assert_eq!(placement(0, 17, 1, 0), Some(attach_id::BACK_RIGHT));
        assert_eq!(placement(0, 21, 3, 0), Some(attach_id::HIP_MAIN));
        assert_eq!(placement(1, 14, 4, 0), Some(attach_id::SHIELD_BACK));
        assert_eq!(placement(0, 21, 3, 1), Some(attach_id::HAND_RIGHT));
    }

    #[test]
    fn ammo_attach_hands_the_volleying_bow_arrow_and_nothing_else() {
        assert_eq!(ammo_attach(Some(0x0f), true), Some(attach_id::HAND_ARROW)); // bow, nocked
        assert_eq!(ammo_attach(Some(0x0f), false), None); // bow before its `$BWP`
        assert_eq!(ammo_attach(Some(0x19), true), None); // thrown: fails the 0x18 gate
        assert_eq!(ammo_attach(Some(0x1a), true), None); // gun, crossbow or wand
        assert_eq!(ammo_attach(None, true), None); // no ranged record
    }

    /// Raw field indices, private to benilla-protocol: `PLAYER_FLAGS` is 190, and the visible-item
    /// blocks start at 258 (`PLAYER_VISIBLE_ITEM_1_CREATOR`), 12 per slot, the entry at +2.
    const PLAYER_FLAGS: u16 = 190;
    const BYTES_0: u16 = 36;

    fn wearing(flags: u32, entries: &[(u8, u32)]) -> ObjectStore {
        let mut pairs = vec![
            (BYTES_0, 1 | 1 << 8), // race 1 (human), class 1, gender 0 (male)
            (PLAYER_FLAGS, flags),
        ];
        for (slot, entry) in entries {
            pairs.push((258 + 2 + 12 * u16::from(*slot), *entry));
        }
        ObjectStore(ObjectFields::from_pairs(&pairs))
    }

    fn worn(display_info_id: u32, inventory_type: u32) -> ItemInfo {
        ItemInfo {
            display_info_id,
            inventory_type,
            ..crate::items::test_template("Worn")
        }
    }

    /// One resolve pass over a player in helm 900, cloak 800 and chest 700: the [`Equipment`] and
    /// whether the helm model attached.
    fn dress(flags: u32) -> (Equipment, bool) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        let mut items = Items::default();
        items.insert_template(100, Some(worn(900, 1))); // INVTYPE_HEAD
        items.insert_template(200, Some(worn(800, 16))); // INVTYPE_CLOAK
        items.insert_template(300, Some(worn(700, 5))); // INVTYPE_CHEST
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        app.insert_resource(items);
        app.init_resource::<crate::net::GuidIndex>();
        app.insert_resource(NetCommands(tx));
        app.insert_resource(ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(std::collections::HashMap::new()),
        ));
        let player = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Player,
                    display_id: Some(49),
                    scale: 1.0,
                },
                wearing(flags, &[(0, 100), (14, 200), (4, 300)]),
            ))
            .id();
        app.add_systems(Update, resolve_equipment);
        app.update();
        drop(rx);
        let w = app.world();
        let eq = *w.get::<Equipment>(player).expect("equipment resolved");
        let helm_attached = w
            .get::<HeldItems>(player)
            .is_some_and(|h| h.slots[3].is_some());
        (eq, helm_attached)
    }

    /// `PLAYER_FLAGS`' `HIDE_HELM 0x400` and `HIDE_CLOAK 0x800` zero the resolved display ids.
    #[test]
    fn the_hide_preferences_undress_the_helm_and_cloak_on_a_world_body() {
        let (shown, helm_attached) = dress(0);
        assert_eq!(shown.helm, 900, "no preference set: the helm is worn");
        assert_eq!(shown.cloak, 800, "…and so is the cloak");
        assert!(
            helm_attached,
            "…and the helm's attach sub-model is asked for"
        );

        let (hidden, helm_attached) = dress(0x400 | 0x800);
        assert_eq!(hidden.helm, 0, "hide-helm zeroes the head slot");
        assert_eq!(hidden.cloak, 0, "hide-cloak zeroes the back slot");
        assert!(
            !helm_attached,
            "the ATTACH half follows the geoset half — the two can never disagree"
        );
        assert_eq!(
            hidden.bodyslots, shown.bodyslots,
            "and nothing else the player is wearing moves"
        );
        assert!(hidden.settled, "the worn set is still fully resolved");
    }

    #[test]
    fn the_two_hide_preferences_do_not_reach_each_other() {
        let (helm_only, helm_attached) = dress(0x400);
        assert_eq!((helm_only.helm, helm_only.cloak), (0, 800));
        assert!(!helm_attached);
        let (cloak_only, helm_attached) = dress(0x800);
        assert_eq!((cloak_only.helm, cloak_only.cloak), (900, 0));
        assert!(helm_attached);
    }

    /// An unmarked store edit must not land, as the gate skips the unit; a marked one must.
    #[test]
    fn an_unchanged_unit_is_not_rebuilt_and_a_real_change_still_lands() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        let mut items = Items::default();
        items.insert_template(100, Some(worn(900, 1))); // INVTYPE_HEAD
        items.insert_template(101, Some(worn(901, 1))); // the swap target
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        app.insert_resource(items);
        app.init_resource::<crate::net::GuidIndex>();
        app.insert_resource(NetCommands(tx));
        app.insert_resource(ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(std::collections::HashMap::new()),
        ));
        let player = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Player,
                    display_id: Some(49),
                    scale: 1.0,
                },
                wearing(0, &[(0, 100)]),
            ))
            .id();
        app.add_systems(Update, resolve_equipment);
        app.update(); // the first run resolves
        app.update(); // steady state: this frame skips
        assert_eq!(app.world().get::<Equipment>(player).unwrap().helm, 900);

        // Helm entry 100 to 101 without marking the store changed.
        app.world_mut()
            .get_mut::<ObjectStore>(player)
            .unwrap()
            .bypass_change_detection()
            .0
            .merge(ObjectFields::from_pairs(&[(258 + 2, 101)]));
        app.update();
        assert_eq!(
            app.world().get::<Equipment>(player).unwrap().helm,
            900,
            "an unmarked store must not be rebuilt — the gate held"
        );

        // Marked changed, as every wire update is.
        app.world_mut()
            .get_mut::<ObjectStore>(player)
            .unwrap()
            .set_changed();
        app.update();
        assert_eq!(
            app.world().get::<Equipment>(player).unwrap().helm,
            901,
            "a marked store rebuilds — the gate opens"
        );
        drop(rx);
    }

    #[test]
    fn a_disarm_takes_a_drawn_weapon_off_the_hand_and_leaves_a_stowed_one() {
        use crate::creature_anim::Wielded;

        /// Raw field indices; byte 0 of `UNIT_FIELD_BYTES_2` is the sheath state.
        const UNIT_FLAGS: u16 = 46;
        const UNIT_BYTES_2: u16 = 164;
        const VISIBLE_ITEM_MAINHAND_ENTRY: u16 = 258 + 2 + 12 * 15;
        /// `UNIT_FLAG_DISARMED`.
        const DISARMED: u32 = 0x0020_0000;

        // A one-handed sword (class 2, subclass 7, InventoryType 21) of sheath type 3: the hip.
        let store = |flags: u32, sheath: u8| {
            ObjectStore(ObjectFields::from_pairs(&[
                (BYTES_0, 1 | 1 << 8), // race 1 (human), class 1, male
                (UNIT_FLAGS, flags),
                (UNIT_BYTES_2, u32::from(sheath)),
                (VISIBLE_ITEM_MAINHAND_ENTRY, 500),
            ]))
        };
        let spawn = |flags: u32, sheath: u8| {
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
            let mut items = Items::default();
            items.insert_template(
                500,
                Some(ItemInfo {
                    class: 2,
                    subclass: 7,
                    display_info_id: 950,
                    inventory_type: 21,
                    sheath: 3,
                    ..crate::items::test_template("Worn Shortsword")
                }),
            );
            let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
            app.insert_resource(items);
            app.init_resource::<crate::net::GuidIndex>();
            app.insert_resource(NetCommands(tx));
            app.insert_resource(ItemDisplays::icons_for_tests(
                benilla_formats::ItemDisplayCatalog::from_displays(
                    std::collections::HashMap::new(),
                ),
            ));
            let player = app
                .world_mut()
                .spawn((
                    NetEntity {
                        kind: EntityKind::Player,
                        display_id: Some(49),
                        scale: 1.0,
                    },
                    store(flags, sheath),
                ))
                .id();
            app.add_systems(Update, resolve_equipment);
            app.update();
            (app, player, rx)
        };
        fn mainhand(app: &App, player: Entity) -> Option<u16> {
            app.world()
                .get::<HeldItems>(player)
                .and_then(|h| h.slots[0].as_ref())
                .map(|slot| slot.attach)
        }

        // (1) Drawn at the edge: detached, and not put back on the hip either.
        let (mut app, player, rx) = spawn(0, 1);
        assert_eq!(
            mainhand(&app, player),
            Some(attach_id::HAND_RIGHT),
            "control: drawn, in the right hand"
        );
        *app.world_mut().get_mut::<ObjectStore>(player).unwrap() = store(DISARMED, 1);
        app.update();
        assert!(
            app.world().get::<Wielded>(player).unwrap().disarmed,
            "the flag reached the hands"
        );
        assert_eq!(
            mainhand(&app, player),
            None,
            "a drawn weapon leaves the hand on the disarm edge"
        );
        // It comes back when the flag clears (`0x5ff67d`, `0x60b770(0)`).
        *app.world_mut().get_mut::<ObjectStore>(player).unwrap() = store(0, 1);
        app.update();
        assert_eq!(
            mainhand(&app, player),
            Some(attach_id::HAND_RIGHT),
            "re-armed: the weapon is put back"
        );
        drop(rx);

        // (2) Stowed at the edge: it stays on the hip, even when the unit then draws.
        let (mut app, player, rx) = spawn(0, 0);
        assert_eq!(mainhand(&app, player), Some(attach_id::HIP_MAIN));
        *app.world_mut().get_mut::<ObjectStore>(player).unwrap() = store(DISARMED, 0);
        app.update();
        assert_eq!(
            mainhand(&app, player),
            Some(attach_id::HIP_MAIN),
            "a stowed weapon is left exactly where it is"
        );
        *app.world_mut().get_mut::<ObjectStore>(player).unwrap() = store(DISARMED, 1);
        app.update();
        assert_eq!(
            mainhand(&app, player),
            Some(attach_id::HIP_MAIN),
            "and stays there — a disarmed hand cannot draw"
        );
        drop(rx);

        // (3) Disarmed when first seen: nothing was ever attached.
        let (app, player, rx) = spawn(DISARMED, 1);
        assert_eq!(
            mainhand(&app, player),
            None,
            "a unit that streams in disarmed shows no mainhand weapon"
        );
        drop(rx);
    }

    #[test]
    fn the_hands_keep_the_worn_reading_through_a_disarm() {
        use crate::creature_anim::Wielded;

        const UNIT_FLAGS: u16 = 46;
        const VISIBLE_ITEM_MAINHAND_ENTRY: u16 = 258 + 2 + 12 * 15;
        const DISARMED: u32 = 0x0020_0000;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        let mut items = Items::default();
        items.insert_template(
            500,
            Some(ItemInfo {
                class: 2,
                subclass: 7,
                display_info_id: 950,
                inventory_type: 21,
                sheath: 3,
                ..crate::items::test_template("Worn Shortsword")
            }),
        );
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        app.insert_resource(items);
        app.init_resource::<crate::net::GuidIndex>();
        app.insert_resource(NetCommands(tx));
        app.insert_resource(ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(std::collections::HashMap::new()),
        ));
        let player = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Player,
                    display_id: Some(49),
                    scale: 1.0,
                },
                ObjectStore(ObjectFields::from_pairs(&[
                    (BYTES_0, 1 | 1 << 8),
                    (UNIT_FLAGS, DISARMED),
                    (VISIBLE_ITEM_MAINHAND_ENTRY, 500),
                ])),
            ))
            .id();
        app.add_systems(Update, resolve_equipment);
        app.update();
        drop(rx);
        let w = *app.world().get::<Wielded>(player).expect("hands resolved");
        assert!(w.disarmed);
        assert_eq!(w.main, Some((2, 7)), "GetWeapon(0, 1) still sees the sword");
        assert_eq!(w.armed_main(), None, "GetWeapon(0, 0) is NULL");
        assert_eq!(
            w.disarmed_hand(),
            Some(0),
            "the main hand is the one hidden"
        );
    }
}
