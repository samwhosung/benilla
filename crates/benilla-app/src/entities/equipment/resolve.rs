//! Equipment **resolution** (split out of `super`'s one file): what each unit
//! should be holding this frame, and where — the descriptor read (a creature's virtual items, a
//! player's visible-item entries through the ask-once item layer), the drawn-vs-stowed placement
//! law, the item/enchant glow id, and the player teardown a gear change forces.
//! The output is [`HeldItems`] + [`Equipment`] + [`Wielded`], which `super::spawn` turns into
//! children.

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

/// The drawn/stowed attachment point for one held slot, or `None` when the item shows nothing (empty
/// slot, sheath-type-less item while stowed, or an unresolved template).
///
/// Drawn: the unit's sheath state (`UNIT_FIELD_BYTES_2` byte 0: 0 stowed · 1 melee · 2 ranged) draws
/// the matching slots into the hands (shield → forearm). Stowed: the **item's** sheath type picks the
/// body point — 1 two-hander → back · 2 staff → lower back · 3 one-hander → hip · 4 shield → centre
/// back (mainhand takes the `K−1` side of each pair — `0x47a070`'s `dl != 0` is the mainhand
/// bodyslot `0xf`). A sheathed ranged weapon renders **nothing** — the client
/// detaches it rather than re-pointing it to a body bone (`0x7130a0`: a pure unlink/release,
/// uniform across bow/gun/crossbow/thrown/wand). Drawn ranged splits by inventory type: a **bow**
/// rides the left hand, gun/crossbow/wand/thrown the right (`0x611e10`'s invType test).
pub(in crate::entities) fn placement(
    slot: usize,
    inv_type: u32,
    item_sheath: u8,
    unit_sheath: u8,
) -> Option<u16> {
    use attach_id::*;
    let shield = inv_type == 14; // INVTYPE_SHIELD
    match slot {
        // Ranged: in hand while ranged-drawn (bow left, everything else right), invisible otherwise.
        2 => (unit_sheath == 2).then_some(if inv_type == 15 {
            HAND_LEFT // INVTYPE_RANGED — bows
        } else {
            HAND_RIGHT
        }),
        // Melee/shield slots: drawn in melee sheath state, else stowed by the item's sheath type.
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

/// One held slot's enchant ids, folded to a single change-detectable number for [`DressKey`].
///
/// The **scan width is the glow resolver's** (`0x62ec70`'s seven `CGItem` enchant slots), so the
/// fold moves exactly when the thing it stands in for — the item's glow — could. Creatures carry
/// none: a virtual item has no enchant fields, like the synthetic item the reference's
/// `GetVirtualItem` hands its own resolver.
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

/// The nocked ammo's attach point (`0x60ba30`): the ONE body-bone attach in the whole mechanism is
/// HandArrow (35), fired for a **bow** once its BowPull event latches `[+0xd58]&0x4000` —
/// `nock_latched` is [`NockLatch`], driven by the real `$BWP`/`$BWR` listener
/// (`drive_nock_latch`). Everything else shows NO nocked
/// model: gun/crossbow hit the client's `gunXbow` early return, thrown resolves the `0x19`
/// *directory* (its own weapon-model copy) but fails the `==0x18` attach gate, and a wand's
/// Shoot has no ammo item.
fn ammo_attach(ranged_inv_type: Option<u32>, nock_latched: bool) -> Option<u16> {
    const INVTYPE_RANGED_BOW: u32 = 0x0f;
    (ranged_inv_type == Some(INVTYPE_RANGED_BOW) && nock_latched).then_some(attach_id::HAND_ARROW)
}

/// The glow id for one held item, and the model requests it implies: the base-or-enchant fork
/// ([`item_glow::effective_visual`]) plus the cache entries for whatever it resolves to. `0` when
/// the glow chain's DBCs are absent — the item simply draws unadorned, as before.
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

/// The resolve inputs that live OUTSIDE the unit's own descriptor and the global caches — the
/// settled sheath byte, the ceremony's per-arm visual state, and the nock lane. Compared each
/// frame by [`resolve_equipment`]'s skip gate (1490): a unit whose descriptor tick, key, and
/// cache epochs all held still resolves to the same loadout **by construction** — every other
/// read in the rebuild is either a field of the store or a static-after-load catalog
/// (`ItemDisplays::catalog`, the enchant rows; the `models` map is write-only from here) — so
/// the rebuild (the template probes, the 7-slot enchant scans, the quiver bag walk, the
/// ensure/glow calls) is skipped whole. The output diff below stays as the last fence: this
/// gate may only ever skip work, never change what lands.
#[derive(Component, Clone, Copy, PartialEq)]
pub(in crate::entities) struct ResolveKey {
    committed_sheath: u8,
    visual_sheath: Option<[u8; 2]>,
    nocked: Option<u32>,
    latched: bool,
}

/// The **dress** half of the same resolve — [`ResolveKey`]'s complement, and the reason it is a
/// separate component: a model widget's duplicate is re-taken on a *dress* change and not on a
/// *placement* one (`0x5dee30`; `crate::portrait::SnapKey`).
///
/// Everything here is read **before** [`placement`], which is the whole point. The three weapon
/// slots are the only ones whose very *presence* a sheath state can decide — a stowed ranged
/// weapon renders nothing at all, and so does a sheath-type-less melee weapon — so a key built
/// from `HeldItems` would move every time the player drew a bow. This one does not.
///
/// Deliberately absent: the **nocked ammo** and the **quiver**, both sheath-derived, and neither a
/// model-event producer in the reference (`0x60ba30` reaches no queue site at all). Helm,
/// shoulders and the body composite are absent for the opposite reason — they cannot move with a
/// sheath, so the pane keys on their mirrored geometry directly.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DressKey {
    /// The body the widget duplicates.
    pub(crate) display_id: Option<u32>,
    /// Mainhand · offhand · ranged: `(ItemDisplayInfo id, model kind, ItemVisuals id)`.
    pub(crate) held: [Option<(u32, ItemModelKind, i32)>; HELD_SLOTS],
    /// Every weapon identity above has a model with **built parts**. Ours, not the reference's:
    /// it duplicates a model the world had already finished assembling, where our item models
    /// stream in. While false the pane keeps re-taking, so a weapon that lands a few frames after
    /// the window opened is not frozen out of the bake.
    pub(crate) held_ready: bool,
}

/// Resolve every unit's held items from its descriptor. Creatures read display/invType/sheath straight
/// from the virtual-item fields; players go visible-item entry → [`crate::items::Items`] (ask-once query on
/// a miss). Ensures each needed display id has a [`DisplayModel`] entry in [`ItemDisplays`] (built
/// by [`super::update_display_models`] once the asset loads) and writes [`HeldItems`] on change.
///
/// **Skip-gated per unit** ([`ResolveKey`]): the full rebuild runs only when the unit's own
/// descriptor changed, its key changed, or a global input moved — the [`Items`] epochs (an
/// object ingest covers the quiver bag walk, a landed template answers every pending ask) and
/// the two client-data load edges. The idle crowd costs one tick check and one small compare.
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
    // The creature display cache — a character-model NPC's helm/shoulder ids + race/sex live on its
    // display's `NpcAppearance` (CreatureDisplayInfoExtra), read here to resolve its attach models.
    creatures: Option<Res<Creatures>>,
    // The item/enchant glow chain: resolved here beside the item itself, so a glow
    // model is requested the same frame its weapon is and an enchant change rides the item diff.
    // The enchant column rides its own resource — shared with the tooltip lane.
    glows: Option<ResMut<ItemGlows>>,
    enchants: Option<Res<crate::items::Enchants>>,
    // The [`Items`] template epoch as of the last run — the skip gate's global half. Deliberately
    // the explicit counter and never the resource's change tick: `templates` is `ResMut` in this
    // very system (ask-once misses write it), so a tick gate would read its own writes and never
    // close. The item *objects* are entities since 2334, watched through `item_changes`.
    mut last_epochs: Local<Option<(u64, u64)>>,
    // The object lookup the equipped guids resolve through and the item entities' change watch,
    // as one param (the 16-SystemParam ceiling).
    item_objects: (crate::net::Objects, crate::items::ItemChanges),
    // The guild identity cache — `ResMut` because it is LAZY: the miss below is
    // what sends the `CMSG_GUILD_QUERY` whose answer paints the tabard. `Option` for the same
    // reason `creatures` is: a harness without the UI plugins still resolves equipment.
    mut guilds: Option<ResMut<crate::ui_guild::GuildState>>,
    // The tabard designer's five under preview — a change re-dresses our body.
    tabard_design: Option<Res<crate::ui_tabard::TabardDesign>>,
) {
    let Some(mut held) = held else {
        return;
    };
    // The guild cache's landed counter joins the item epochs for the same reason those are here:
    // a `CMSG_GUILD_QUERY` answered three frames after a player spawned changes what that player's
    // tabard paints, and nothing about their descriptor or the item cache moves to say so.
    let epochs = (
        templates.template_epoch(),
        guilds.as_ref().map_or(0, |g| g.identity_generation()),
    );
    // The watch is drained whether or not the gate is read (`moved` reads the removals).
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
        // The settled sheath state: the anim layer's **client-side committed state** (the
        // setter/reconcile cache, decision 0080 — the descriptor byte plus the policy's forces);
        // else, before the driver first runs, the raw descriptor byte.
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
        // The two **equipment-display preferences**: `PLAYER_FLAGS`'
        // `HIDE_HELM 0x400` / `HIDE_CLOAK 0x800`, read off THIS unit's own descriptor. The field is
        // public, so this is per rendered body and not a local setting — a remote player who hides
        // their helm hides it on our screen, exactly as ours hides on theirs. A creature has no
        // `PLAYER_FLAGS` and reads `false` for both (its head/shoulder columns come from
        // CreatureDisplayInfoExtra, which carries no such preference).
        //
        // The suppression is a **display id of zero**, applied below to every place the piece is
        // resolved — which is the same thing "no helm equipped" means everywhere else in this
        // module, so the whole downstream chain follows for free: the helm's attach sub-model is not
        // requested, its `0x4799a0` HelmetGeosetVisData hide-masks are not applied (hair, facial
        // hair and ears come back), the cloak's geoset group is not selected and its cape texture
        // is not resolved. It is also the shape the glue lane has honoured since 0465
        // (`attach::preview` zeroes the same two slots off the char-enum record's
        // `CHARACTER_FLAG_HIDE_*`, which vmangos round-trips into these very bits at login) — the
        // world was the half that never consumed it.
        let (hide_helm, hide_cloak) = (s.player_hides_helm(), s.player_hides_cloak());
        // Worn armor (players): resolve the composite slots' entries → display ids.
        // `settled` only once every non-empty entry has an answer, so the first attach composites the
        // dressed atlas directly (the template cache makes later logins instant).
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
            // The cloak (equipment slot 14): geoset + cape texture, resolved the same way.
            // The hide preference zeroes the RESOLVED id rather than skipping the lookup, so
            // `settled` keeps meaning "every worn entry has an answer" — switching the preference
            // back on then re-dresses from a warm template cache instead of stalling a frame on a
            // round trip the player would see as a flicker.
            if let Some(entry) = s.player_visible_item_entry(14).filter(|e| *e != 0) {
                match templates.held(entry, &net) {
                    Some(t) => eq.cloak = t.display_info_id,
                    None => eq.settled = false,
                }
            }
            // The helm (equipment slot 0): attach model + the `0x4799a0` hide-masks (its geoset
            // effect — a hair/facial/ears change — rides the Equipment diff, so donning one
            // re-attaches).
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
            // The guild tabard. Resolved for every player, tabard worn or not —
            // the composite's own gate is the tabard DISPLAY's flag, and asking here keeps the
            // query on the same lazy-cache idiom as every other read of that cache. A miss answers
            // `None` for this frame and re-runs when the response bumps the counter above.
            eq.emblem = guilds
                .as_deref_mut()
                .and_then(|g| crate::ui_guild::unit_guild_emblem(s, g, &net));
            // The tabard designer's preview: while it is open on OUR body the
            // five under design replace the guild's emblem and the tabard geoset is forced on
            // over the empty slot — the reference's `[cc+0xc]` flag and its `0x47a610` install,
            // both on the local player's character component and nobody else's.
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
        // …and the *visual* sheath governing a given slot's placement (`committed` was read
        // into the key above), which during a draw/stow
        // ceremony is **per arm** ([`VisualSheath`]): each hand's weapon moves at its own clip's
        // authored $SHL/$SHR moment, not at the byte change. A melee → ranged toggle therefore
        // has the sword already on the back while the bow is still on its way to the other hand —
        // the ceremony's two movements (`creature_anim::sheath`).
        let sheath_of = |slot: usize, inv_type: u32| {
            visual_sheath.map_or(committed, |v| v.for_slot(slot, inv_type))
        };
        // A player wearing a NON-character display (druid form, GM morph)
        // attaches no equipment sub-models at all: the reference's held/helm/shoulder attach
        // lives on the CCharacterComponent (`0x47a0c0`), which only
        // a character body builds — a bear-form druid shows no weapon by construction, not by a
        // hide flag. [`Wielded`] (the anim-class pair) still resolves below: what's IN the hand
        // is independent of whether its model is displayed. The creature virtual-item path (a
        // naga's trident) is a different, unit-level mechanism and rides the Unit arms untouched.
        // An unresolved display cache entry (the one-frame window after a live swap) reads as a
        // character body — harmless: this diff re-runs every frame, and an attach needs the
        // rebuilt body's `BoneAttach` first anyway.
        let char_component = net_entity.kind != EntityKind::Player
            || net_entity
                .display_id
                .and_then(|d| creatures.as_deref()?.models.get(&d))
                .is_none_or(|dm| dm.is_character_body);
        let mut slots: [Option<HeldSlot>; ATTACH_SLOTS] = [None; ATTACH_SLOTS];
        let mut wielded = Wielded {
            // The disarm bit, read off this unit's own descriptor exactly where the reference
            // reads it — inside `GetWeapon` (`[[unit+0x110]+0xa0] & 0x200000`, `0x5ec2b8`). It
            // decides the COMBAT reading of the hands ([`Wielded::armed_main`]) and, through
            // [`DisarmFreeze`] below, what the hidden hand still shows.
            disarmed: s.unit_flags() & UNIT_FLAG_DISARMED != 0,
            ..Wielded::default()
        };
        let mut ranged_inv_type = None;
        // The DRESS, gathered as we go: what the unit *wears* in the three weapon slots, recorded
        // before `placement` decides whether any of it is currently rendered ([`DressKey`]).
        let mut worn: [Option<(u32, ItemModelKind, i32)>; HELD_SLOTS] = [None; HELD_SLOTS];
        // **Pass 1 — what each hand HOLDS** (`GetWeapon(slot, 1)`). All three slots first, because
        // the disarm ladder is a question about BOTH hands at once (`Wielded::disarmed_hand`) and
        // cannot be asked while they are still being filled in.
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
            // The item's Material — the draw/stow sound's only key. Both wire
            // sources carry it; neither is a guess.
            wielded.materials[slot] = material;
            // The wielded weapon-class pair (decision 0073's swing/ready selectors) — what's *in*
            // the hand, independent of whether its model is displayed (a sheath-less item still
            // swings with its own class). The mainhand's sheath type picks the draw/stow one-shot
            // (Sheath 89 back / HipSheath 90 hip).
            match slot {
                0 => {
                    wielded.main = Some((class, subclass));
                    wielded.main_sheath = item_sheath;
                }
                1 => {
                    wielded.off = Some((class, subclass));
                    wielded.off_sheath = item_sheath;
                }
                // The ranged slot: the local auto-repeat idle's Load/Hold selector reads it
                // (`select::ranged_load_anim`, 0099 phase 5); the InventoryType picks the
                // nocked ammo's attach point below.
                2 => {
                    wielded.ranged = Some((class, subclass));
                    wielded.ranged_sheath = item_sheath;
                    wielded.ranged_inv = inv_type;
                    ranged_inv_type = Some(inv_type);
                }
                _ => {}
            }
        }
        // **The ladder** — which single hand `UNIT_FLAG_DISARMED` hides, now that
        // both are known. `None` while the flag is down, or when neither hand holds a weapon.
        let hidden = wielded.disarmed_hand();
        // The rising edge, as SEEN HERE: the reference's reflex `0x5ff580` fires on the bit
        // *changing*, so a unit that streamed in already disarmed never had its weapon attached at
        // all, and one whose edge we watched keeps whatever the reflex left (see [`DisarmFreeze`]).
        let disarm_edge = hidden.is_some() && current_wielded.is_some_and(|w| !w.disarmed);
        let mut next_freeze = match hidden {
            // Flag down, or neither hand holds a weapon: no freeze at all, and the falling edge
            // therefore clears it — `0x5ff67d`'s re-attach, which puts the weapon back wherever
            // the live sheath state says it belongs.
            None => None,
            // The edge itself writes it below, from the placement the weapon had at that moment.
            Some(_) if disarm_edge => None,
            // Steady state: keep what the reflex left.
            Some(_) => freeze.copied(),
        };
        // **Pass 2 — the models.**
        for slot in 0..HELD_SLOTS {
            let Some((display, inv_type, item_sheath, _, _, _)) = resolved[slot] else {
                continue;
            };
            if !char_component {
                continue; // wielded resolved; the model never attaches on a non-character body
            }
            let kind = if inv_type == 14 {
                ItemModelKind::Shield
            } else {
                ItemModelKind::Weapon
            };
            // The dress, recorded **above** the placement gate — the one line that makes a
            // widget's snapshot blind to the sheath. Third term: the slot's enchant ids, folded.
            // Read straight off the descriptor rather than taken from the resolved `visual` below,
            // because that one is only computed for a slot that is actually placed — and an
            // enchant change is a model-event producer whether or not the weapon is drawn.
            worn[slot] = Some((display, kind, enchant_fold(s, net_entity.kind, slot)));
            let live = placement(slot, inv_type, item_sheath, sheath_of(slot, inv_type));
            // The disarm reflex (`0x5ff580`). The hidden hand's weapon is NOT
            // placed from the live sheath state — while the flag is up nothing may attach it or
            // move it, so it is wherever the reflex left it at the edge: gone if it was in the
            // hand (`0x5ff676`'s detach), still at its body point if it was stowed, and gone if
            // we never saw the edge because nothing ever attached it.
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
            // The glow: the display's intrinsic visual, else this weapon slot's
            // first enchant with one. The enchant half is **players only** — the enchant ids ride
            // `PLAYER_VISIBLE_ITEM`, and a creature's virtual item carries none, exactly like the
            // synthetic item the reference's `GetVirtualItem` hands its resolver.
            let enchants = (net_entity.kind == EntityKind::Player).then(|| {
                // All 7 CGItem enchant slots, the reference's scan width (`0x62ec70`); 1.12
                // broadcasts the first two (PERM, TEMP).
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
        // Publish the dress the moment the weapons are resolved. `held_ready` asks only about
        // slots that are actually PLACED — a stowed ranged weapon has no model requested at all,
        // so counting it would leave the key permanently unready and the pane permanently live,
        // which is the very thing this component exists to stop.
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
        // The nocked ammo (`0x60ba30`): the ONE body-bone attach in the whole mechanism is
        // HandArrow (35), **bow-only** — gun/crossbow (`gunXbow` early return) and thrown
        // (directory selector `0x19`, never an attach id) show NO nocked model. The [`NockedAmmo`]
        // display is written per shot from `SMSG_SPELL_START`, any caster; the attach follows the
        // client's `$BWP`/`$BWR` keyframes through [`NockLatch`] (`drive_nock_latch`, decision
        // 0408 — the arrow appears at the pull and leaves with the release).
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
            // The ammo model's own intrinsic visual — the reference attaches it at the tail of
            // the ranged/ammo builder `0x479f40` (`47a051: mov edx,[ebx+0x58]`), the same way the
            // hand attach does. No enchant lane here: this is a model, not an equipped item.
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
        // The quiver on the back: while the RANGED weapon is drawn — the same `0x611e10`
        // ranged-draw transition, cleared on every other ranged state — the client scans the
        // player's OWN inventory for an ItemClass-11 container (Quiver/Ammo Pouch) and parents its
        // display model at attachment 26 (no transform override; no cloak conflict). Self-only by
        // construction, exactly like the client: bag slots are never replicated in 1.12, so a
        // remote player's scan finds nothing (a two-client capture would be the clean
        // confirmation).
        // Timed off the RANGED slot's own visual state, so the quiver arrives with the bow it
        // feeds. (Named deviation: the ref attaches it at the *start* of the ranged draw — the
        // `0x611e10(1)` call inside each drawer — where ours waits for that clip's $SHL. Same
        // clip, a few hundred ms apart, and the two are never seen separately.)
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
        // Helm + shoulders (0074 slice 3c / the npc-armor arc): attach sub-models like the held items —
        // the helm's file is per-race/sex, the shoulders a left/right model pair off one display row.
        // A **player** sources them from its visible-item entries (wire → item template → display id)
        // with race/sex off its descriptor; a **character-model NPC** sources them from its display's
        // CreatureDisplayInfoExtra head/shoulder columns with race/sex from the same row — those are
        // direct ItemDisplayInfo display ids, no template round-trip. A beast NPC (no appearance row)
        // resolves nothing here, exactly as before.
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
                // The helm's attach sub-model. Zero when hidden — the same id the geoset half
                // above was given, so the two halves of one preference can never disagree.
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
        // Per-hand grip: a weapon in a hand's attach point curls that hand's fingers (`0x60b590`)
        // — mainhand → right (id 1), non-shield offhand → left (id 2); a forearm shield (id 0) or
        // an empty hand stays open. Drives [`HandGrip`]'s finger overlay.
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
        // The freeze rides the same diff as the hands. Absent is a real state — "nothing is
        // attached to that hand" — so the falling edge REMOVES it rather than storing an empty.
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

/// **The corpse's dress** — [`resolve_equipment`]'s sibling for a `TYPEID_CORPSE`
/// body, kept apart from it rather than threaded through it because almost nothing it does applies.
///
/// A corpse's gear is a *snapshot*, and it is already resolved: the 19 `CORPSE_FIELD_ITEM` slots
/// carry `DisplayInfoID | (InventoryType << 24)` (vmangos `Player.cpp:4821`), so there is no item
/// entry, no template round trip, and therefore no `settled` handshake to wait on — the answer is
/// complete the moment the descriptor lands. There is likewise no sheath state, no draw/stow
/// ceremony, no nock lane, no quiver, no enchant scan and no glow: the reference's corpse dress
/// (`0x5d6260`) is one flat loop over the 19 slots into `0x478cb0`, and nothing else.
///
/// **Three slots never dress**, and the two shapes are different:
/// - slot 0 (head) when this corpse's own `CORPSE_FLAG_HIDE_HELM 0x08` is set, and slot 14 (back)
///   when `HIDE_CLOAK 0x10` is (`0x5d6465`/`0x5d6470` — its own bits on its own field, snapshotted
///   from `PLAYER_FLAGS` at death). Suppression is a display id of **zero**, the same shape the
///   player lane uses, so the whole downstream chain — the helm's attach model, its
///   `0x4799a0` hide-masks, the cloak's geoset and cape texture — follows for free.
/// - slots 15/16/17 (mainhand, offhand, ranged) — **always**. Ranged is skipped outright
///   (`0x5d644e`); the two weapon slots take a branch that looks up the packed item word as an
///   *object guid* (`0x5d649b` → `0x468460`, typemask 2) and can therefore never resolve. A corpse
///   wears armour, not weapons. See this module's sibling `entities::corpse` for why we reproduce
///   the outcome and not the dead lookup.
///
/// A **bone pile** wears nothing at all (`0x5d6291`'s early skip — it builds no character component
/// in the first place). It still gets an empty [`Equipment`], because that component is also the
/// attach gate: a corpse whose descriptor has not landed must wait a frame rather than composite a
/// naked body it would never re-dress (the corpse lane has no re-dress — its gear cannot change).
#[allow(clippy::type_complexity)] // one query's tuple + its change filter
pub(in crate::entities) fn resolve_corpse_equipment(
    mut commands: Commands,
    corpses: Query<(Entity, &NetEntity, Ref<ObjectStore>, Option<&Equipment>)>,
    held: Option<ResMut<ItemDisplays>>,
    asset_server: Res<AssetServer>,
    net: Res<NetCommands>,
    // The guild identity cache, `ResMut` for the same reason the player lane's is: the miss is
    // what SENDS the `CMSG_GUILD_QUERY` whose answer paints the crest.
    mut guilds: Option<ResMut<crate::ui_guild::GuildState>>,
    mut last_guild_epoch: Local<Option<u64>>,
) {
    let Some(mut held) = held else {
        return;
    };
    // The change gate moved off the query filter and in here to make room for the guild counter
    // (the player lane's own shape): an answer landing three frames after the corpse streamed in
    // changes what its tabard paints, and nothing about the corpse's descriptor moves to say so.
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
        // The armour composite + the two preference-gated slots. `settled` is unconditionally
        // true: every id here is final on arrival.
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
            // The guild tabard crest, from the corpse's OWN `CORPSE_FIELD_GUILD` snapshot — the
            // reference's `0x5d6ec0`, reached from the dress loop at the tabard slot (`ebx == 0x12`
            // with that display's `ItemDisplayInfo` flag bit 0) and resolved through the same
            // name cache a living body's is (`0x6d6d20`). A
            // bone pile builds no character component, so it never reaches this leg — which is
            // exactly where this sits.
            eq.emblem = guilds
                .as_deref_mut()
                .and_then(|g| crate::ui_guild::corpse_guild_emblem(s, g, &net));
        }
        if current != Some(&eq) {
            commands.entity(entity).insert(eq);
        }
        // The two attach sub-models a corpse can wear: the helm (equipment slot 0, per-race/sex
        // file) and the shoulder pair (slot 2, one display row → a left/right model pair). Both are
        // ordinary `0x478cb0` slots in the reference's loop; they are attachments on our side
        // because that is how a character body carries them.
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
            // Shoulders carry no hide preference — the reference gates only slots 0 and 0xe.
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

    /// A corpse's dress is the descriptor, packed: armour off the
    /// `CORPSE_FIELD_ITEM` slots as ItemDisplayInfo ids with no template round trip, the head and
    /// back slots suppressed by the corpse's OWN `CORPSE_FLAG_HIDE_HELM`/`HIDE_CLOAK` bits, and
    /// **never** a weapon or a ranged slot.
    #[test]
    fn corpse_dresses_from_its_own_snapshot() {
        use benilla_protocol::messages::ObjectType;

        /// `CORPSE_FIELD_ITEM + slot` = field 13 + slot, packing
        /// `DisplayInfoID | (InventoryType << 24)`.
        fn item(slot: u16, display: u32, inv: u32) -> (u16, u32) {
            (13 + slot, display | (inv << 24))
        }
        // Head 900, shoulders 901, chest 902, back 903, mainhand 904, ranged 905 — and race 1 /
        // sex 0 in CORPSE_FIELD_BYTES_1 bytes 1/2.
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
            // The guild-crest leg's channel (the emblem resolve is lazy and would send a
            // `CMSG_GUILD_QUERY` on a miss); no `GuildState` here, so the crest reads `None` —
            // which is the guildless case and exactly what this test's corpse is.
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
        // Chest is composite index 1 (the COMPOSITE_SLOTS table's equipment slot 4).
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
        // The three slots the reference never dresses: mainhand, offhand, ranged.
        assert!(
            held.slots[..HELD_SLOTS].iter().all(Option::is_none),
            "a corpse wears armour, never weapons"
        );

        // HIDE_HELM 0x08 / HIDE_CLOAK 0x10 — this corpse's own bits, suppressing by a ZERO id so
        // the geoset masks and the attach model fall away together.
        let (eq, held) = run(dressed(0x08));
        assert_eq!(eq.helm, 0);
        assert_eq!(eq.cloak, 903, "the cloak bit is a different bit");
        assert!(held.slots[3].is_none(), "no helm model either");
        let (eq, _) = run(dressed(0x10));
        assert_eq!(eq.cloak, 0);
        assert_eq!(eq.helm, 900);

        // BONES 0x01 — no character component at all, so nothing is worn however full the slots.
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

    /// The ranged slot's whole placement law: in hand only while ranged-drawn (state 2) — bow
    /// (INVTYPE_RANGED 15) to the left hand, gun/crossbow/wand/thrown (RANGEDRIGHT 26 / THROWN 25)
    /// to the right — and invisible in every other sheath state, regardless of the item's own
    /// sheath type.
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

    /// Melee slots keep the sheath-type stow table (unchanged by the ranged rule).
    #[test]
    fn melee_slots_stow_by_item_sheath_type() {
        assert_eq!(placement(0, 17, 1, 0), Some(attach_id::BACK_RIGHT));
        assert_eq!(placement(0, 21, 3, 0), Some(attach_id::HIP_MAIN));
        assert_eq!(placement(1, 14, 4, 0), Some(attach_id::SHIELD_BACK));
        assert_eq!(placement(0, 21, 3, 1), Some(attach_id::HAND_RIGHT));
    }

    /// The nocked-ammo attach law (`0x60ba30`): HandArrow (35) is the ONE attach,
    /// bow-only, gated on the `$BWP` nock latch.
    /// Gun/crossbow/thrown never attach a nocked model.
    #[test]
    fn ammo_attach_hands_the_volleying_bow_arrow_and_nothing_else() {
        assert_eq!(ammo_attach(Some(0x0f), true), Some(attach_id::HAND_ARROW)); // bow, volleying
        assert_eq!(ammo_attach(Some(0x0f), false), None); // bow, idle — pre-BowPull, no attach
        assert_eq!(ammo_attach(Some(0x19), true), None); // thrown — directory 0x19, never an id
        assert_eq!(ammo_attach(Some(0x1a), true), None); // gun/xbow/wand — the gunXbow early return
        assert_eq!(ammo_attach(None, true), None); // no ranged record
    }

    /// `PLAYER_FLAGS` (field 190) and the visible-item block (`PLAYER_VISIBLE_ITEM_1_CREATOR` 258,
    /// +12 per slot, the entry at +2) by their raw wire indices — the constants are crate-private
    /// to benilla-protocol, so every descriptor fixture in this crate spells them out.
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

    /// Run one `resolve_equipment` pass over a player wearing helm 900 / cloak 800 / chest 700,
    /// with `flags` on their descriptor, and return what the body was dressed with: the
    /// [`Equipment`] triple and whether the helm's ATTACH sub-model slot was filled.
    fn dress(flags: u32) -> (Equipment, bool) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        let mut items = Items::default();
        items.insert_template(100, Some(worn(900, 1))); // INVTYPE_HEAD
        items.insert_template(200, Some(worn(800, 16))); // INVTYPE_CLOAK
        items.insert_template(300, Some(worn(700, 5))); // INVTYPE_CHEST
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        app.insert_resource(items);
        // The one object index `Objects` reads (2334); empty, like the map these tests never seeded.
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

    /// **B123**: the two equipment-display preferences are consumed on the WORLD
    /// body, not only on the character-select one. `PLAYER_FLAGS_HIDE_HELM 0x400` /
    /// `HIDE_CLOAK 0x800` zero the resolved display id, which is what makes every downstream
    /// consumer follow — no cape geoset, no cape texture, no helm attach model, and no `0x4799a0`
    /// hide-mask stripping the hair.
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

    /// The two bits are independent: hiding one leaves the other worn.
    #[test]
    fn the_two_hide_preferences_do_not_reach_each_other() {
        let (helm_only, helm_attached) = dress(0x400);
        assert_eq!((helm_only.helm, helm_only.cloak), (0, 800));
        assert!(!helm_attached);
        let (cloak_only, helm_attached) = dress(0x800);
        assert_eq!((cloak_only.helm, cloak_only.cloak), (900, 0));
        assert!(helm_attached);
    }

    /// **The skip gate** (1490): a unit whose descriptor tick, [`super::ResolveKey`] and cache
    /// epochs held still is not rebuilt at all. Proven the sharp way: a descriptor edit smuggled
    /// past change detection — a thing the wire can never do — must NOT land, because the only
    /// system that could land it skipped; the same edit under a normal (marked) touch lands on
    /// the next pass. The control half is what guards against the gate ever wrongly holding.
    #[test]
    fn an_unchanged_unit_is_not_rebuilt_and_a_real_change_still_lands() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        let mut items = Items::default();
        items.insert_template(100, Some(worn(900, 1))); // INVTYPE_HEAD
        items.insert_template(101, Some(worn(901, 1))); // the swap target
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        app.insert_resource(items);
        // The one object index `Objects` reads (2334); empty, like the map these tests never seeded.
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
        app.update(); // resolve (caches_moved: first run)
        app.update(); // steady state — this frame already skips
        assert_eq!(app.world().get::<Equipment>(player).unwrap().helm, 900);

        // The smuggled edit: helm entry 100 → 101 with the change tick left untouched.
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

        // The wire's shape: the same store, now MARKED changed — the swap lands.
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

    /// **What a disarm does to the weapon MODEL**. The reference's attachment
    /// state is built by events, and `UNIT_FIELD_FLAGS`' change reflex `0x5ff580` is one of them:
    /// when the DISARM bit goes up it calls `0x47a310(model, 0xf, 0, 0)` — an unlink of the
    /// main-hand attachment — **but only while the weapon is drawn**, and afterwards every attach
    /// site declines because its `GetWeapon(slot, 0)` reads NULL. So the weapon does NOT stay in
    /// the fist (as `0x60b590` read alone would suggest), and it does NOT vanish off the back
    /// either. Three cases, one body each.
    #[test]
    fn a_disarm_takes_a_drawn_weapon_off_the_hand_and_leaves_a_stowed_one() {
        use crate::creature_anim::Wielded;

        /// `UNIT_FIELD_FLAGS`, `UNIT_FIELD_BYTES_2` (byte 0 = the sheath state) and the
        /// mainhand's public entry — raw wire indices, spelled out like every other descriptor
        /// fixture in this file.
        const UNIT_FLAGS: u16 = 46;
        const UNIT_BYTES_2: u16 = 164;
        const VISIBLE_ITEM_MAINHAND_ENTRY: u16 = 258 + 2 + 12 * 15;
        /// `UNIT_FLAG_DISARMED`.
        const DISARMED: u32 = 0x0020_0000;

        // A one-handed sword worn on the hip: class 2 subclass 7, INVTYPE_WEAPONMAINHAND, sheath
        // type 3 — so the stowed placement has a real body point to land on and the drawn one is
        // the right hand.
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
            // The one object index `Objects` reads (2334); empty, like the map these tests never seeded.
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
        /// Where the mainhand's model hangs this frame, or `None` for "not on screen at all".
        fn mainhand(app: &App, player: Entity) -> Option<u16> {
            app.world()
                .get::<HeldItems>(player)
                .and_then(|h| h.slots[0].as_ref())
                .map(|slot| slot.attach)
        }

        // (1) DRAWN when the flag goes up — the reflex detaches it. It is not in the hand, and it
        //     does not fall back to the hip either: nothing re-attaches while the flag is up.
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
        // …and comes back when the flag clears (`0x5ff67d` → `0x60b770(0)`).
        *app.world_mut().get_mut::<ObjectStore>(player).unwrap() = store(0, 1);
        app.update();
        assert_eq!(
            mainhand(&app, player),
            Some(attach_id::HAND_RIGHT),
            "re-armed: the weapon is put back"
        );
        drop(rx);

        // (2) STOWED when the flag goes up — the reflex's detach is gated on the sheath state
        //     being non-zero, so nothing happens and the sword stays on the hip. It stays there
        //     even once the unit draws: while disarmed the transition moves nothing.
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

        // (3) Already disarmed when we first see the unit — the edge happened elsewhere, so
        //     nothing was ever attached to that hand and the attach sites all decline.
        let (app, player, rx) = spawn(DISARMED, 1);
        assert_eq!(
            mainhand(&app, player),
            None,
            "a unit that streams in disarmed shows no mainhand weapon"
        );
        drop(rx);
    }

    /// The COMBAT reading is separate from the display one, and only the display one is frozen:
    /// the hands still report what is worn (`GetWeapon(slot, 1)` — what the paperdoll shows),
    /// while `armed_main` reports what the unit can fight with.
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
        // The one object index `Objects` reads (2334); empty, like the map these tests never seeded.
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
