//! The item layer: item objects in the guid index and the item template cache.
//!
//! An item is `Guid` + [`ObjectStore`] + [`ItemObject`] in the guid index; item fields are private,
//! so only our own arrive. Templates are keyed by entry, asked once, and survive disconnect.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::{ItemInfo, ObjectFields};

use bevy::ecs::system::SystemParam;

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, Objects};
use crate::query_cache::QueryCache;

/// An item or container entity's kind (`TYPEMASK_ITEM` / `TYPEMASK_CONTAINER`), with no model or
/// pose. Unit readers filter it out (`Without<ItemObject>`): an item block's dwords overlap the
/// unit block's indices.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ItemObject {
    /// `TYPEMASK_CONTAINER`: a bag's slot array lives past the item block.
    pub(crate) container: bool,
}

/// Spawn an item object into the index. The seed is never merged, so the create raises no field
/// notify; a re-create of a live guid takes the handler's values path instead.
pub(crate) fn spawn_item(
    commands: &mut Commands,
    index: &mut GuidIndex,
    guid: u64,
    fields: ObjectFields,
    container: bool,
) -> Entity {
    let e = commands
        .spawn((
            Guid(guid),
            ObjectStore(fields),
            ItemObject { container },
            Countdowns::default(),
        ))
        .id();
    index.0.insert(guid, e);
    e
}

/// `ITEM_FIELD_ENCHANTMENT`'s 21 dwords, three per slot; the reference's seven deadline cells at
/// `obj + 0x324 + slot*4` (`0x5d9d00`) end where the next member begins, at `+0x340`.
pub(crate) const ENCHANT_SLOTS: usize = 7;

/// An item's countdowns, the reference's deadline cells on `CGItem_C`: its lifetime at `+0x320`
/// (fed only by `SMSG_ITEM_TIME_UPDATE`) and one temporary-enchant deadline per slot at `+0x324`
/// (fed only by `SMSG_ITEM_ENCHANT_TIME_UPDATE`; the enchantment field's duration is never read).
/// Absolute deadlines, recomputed on every read and never ticked (`0x5d9c60`, `0x5d9d00`).
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Countdowns {
    lifetime: Option<Instant>,
    enchants: [Option<Instant>; ENCHANT_SLOTS],
}

/// The setters' store rule (`0x5d9c00`, `0x5d9cc0`): a signed `<= 0` (`jle`) clears the cell, so a
/// top-bit value is absence, not a 68-year deadline; anything else parks `now + seconds`.
fn deadline(seconds: u32) -> Option<Instant> {
    ((seconds as i32) > 0).then(|| Instant::now() + Duration::from_secs(u64::from(seconds)))
}

/// A cell's time left: `None` when unset, `Some(0)` once elapsed (`0x5d9d00`'s `max(0, …)`).
fn left(cell: Option<Instant>) -> Option<Duration> {
    cell.map(|at| at.saturating_duration_since(Instant::now()))
}

/// The tooltip's read: an elapsed cell is no timer (its `!= 0` gate prints the plain line).
fn remaining_ms(cell: Option<Instant>) -> Option<u64> {
    left(cell)
        .filter(|l| !l.is_zero())
        .map(|l| l.as_millis() as u64)
}

impl Countdowns {
    /// `SMSG_ITEM_ENCHANT_TIME_UPDATE`'s setter `0x5d9cc0`. A slot past the seventh is refused
    /// (`false`), where the reference indexes unchecked and overruns into the next member.
    pub(crate) fn set_enchant(&mut self, slot: u32, seconds: u32) -> bool {
        let Some(cell) = self.enchants.get_mut(slot as usize) else {
            return false;
        };
        *cell = deadline(seconds);
        true
    }

    /// `SMSG_ITEM_TIME_UPDATE`'s setter `0x5d9c00`.
    pub(crate) fn set_lifetime(&mut self, seconds: u32) {
        self.lifetime = deadline(seconds);
    }

    fn enchant(&self, slot: u32) -> Option<Instant> {
        self.enchants.get(slot as usize).copied().flatten()
    }

    /// Milliseconds left on the temporary enchant in `slot`; `None` also once expired.
    pub(crate) fn enchant_remaining_ms(&self, slot: u32) -> Option<u64> {
        remaining_ms(self.enchant(slot))
    }

    /// [`Self::enchant_remaining_ms`] floored to the second, for the snapshot feeds: a per-ms read
    /// would fire `UNIT_INVENTORY_CHANGED` every frame while a timer runs.
    pub(crate) fn enchant_remaining_display_ms(&self, slot: u32) -> Option<u64> {
        self.enchant_remaining_ms(slot).map(|ms| ms - ms % 1000)
    }

    /// The deadline read without the expired-is-absent collapse: `Some(0)` once run out, `None`
    /// only when never set. `GetWeaponEnchantInfo` returns `max(0, deadline - now)` (`0x5d9d00`),
    /// so an elapsed enchant answers 0 and `BuffFrame_Enchant_OnUpdate` draws "0 s".
    pub(crate) fn enchant_deadline_ms(&self, slot: u32) -> Option<u64> {
        left(self.enchant(slot)).map(|l| l.as_millis() as u64)
    }

    /// Milliseconds left on the item's own lifetime; `None` also once elapsed.
    pub(crate) fn lifetime_remaining_ms(&self) -> Option<u64> {
        remaining_ms(self.lifetime)
    }

    /// [`Self::lifetime_remaining_ms`] floored to the whole second, as the enchant read is.
    pub(crate) fn lifetime_remaining_display_ms(&self) -> Option<u64> {
        self.lifetime_remaining_ms().map(|ms| ms - ms % 1000)
    }

    /// This item's share of [`ItemChanges::countdown_steps`]: `floor(seconds left) + 1` per live
    /// cell, 0 when unset or elapsed, so the final `Some(0)` to `None` collapse is its own step.
    fn steps(&self, now: Instant) -> u64 {
        std::iter::once(self.lifetime)
            .chain(self.enchants)
            .flatten()
            .map(|at| {
                let left = at.saturating_duration_since(now);
                if left.is_zero() {
                    0
                } else {
                    left.as_secs() + 1
                }
            })
            .sum()
    }
}

/// An item entity whose fields or countdown cells were written since the reader last ran.
type ItemMoved = (
    With<ItemObject>,
    Or<(Changed<ObjectStore>, Changed<Countdowns>)>,
);

/// Whether any item object was created, written or destroyed this frame; the inventory feeds' gate.
#[derive(SystemParam)]
pub(crate) struct ItemChanges<'w, 's> {
    changed: Query<'w, 's, (), ItemMoved>,
    removed: RemovedComponents<'w, 's, ItemObject>,
    countdowns: Query<'w, 's, &'static Countdowns>,
}

impl ItemChanges<'_, '_> {
    /// Drains the removal reader, so call it once per run.
    pub(crate) fn moved(&mut self) -> bool {
        let removed = self.removed.read().count() > 0;
        !self.changed.is_empty() || removed
    }

    /// The sum of every item's [`Countdowns`] steps: it moves exactly when a second-floored read
    /// can, and each term only falls between landings, so no change masks another.
    pub(crate) fn countdown_steps(&self) -> u64 {
        let now = Instant::now();
        self.countdowns.iter().map(|c| c.steps(now)).sum()
    }
}

/// The player's inventory as one read: the self descriptor, the object lookup and the item watch.
#[derive(SystemParam)]
pub(crate) struct Inventory<'w, 's> {
    pub(crate) self_store: Query<'w, 's, &'static ObjectStore, With<crate::net::SelfPlayer>>,
    pub(crate) self_changed:
        Query<'w, 's, (), (With<crate::net::SelfPlayer>, Changed<ObjectStore>)>,
    pub(crate) objects: Objects<'w, 's>,
    pub(crate) changes: ItemChanges<'w, 's>,
}

/// The Copy slice of an [`ItemInfo`] that equipment rendering and combat animation read.
#[derive(Clone, Copy)]
pub(crate) struct HeldTemplate {
    pub(crate) display_info_id: u32,
    pub(crate) inventory_type: u32,
    pub(crate) sheath: u32,
    pub(crate) class: u32,
    pub(crate) subclass: u32,
    /// `Material.dbc` id (1 metal, 2 wood, 5 chain, 6 plate, 7 cloth, 8 leather, 0 undefined), the
    /// only input to the draw/stow sound: every `SheatheSoundLookups` row of a material agrees.
    pub(crate) material: u32,
}

/// `SpellItemEnchantment.dbc`, for the weapon glow ([`crate::entities::item_glow`]) and the
/// tooltip enchant line ([`crate::ui_items`]). Optional: absent, neither draws.
#[derive(Resource)]
pub(crate) struct Enchants(pub(crate) benilla_formats::EnchantCatalog);

/// One enchant slot: `(slot index, id, charges, remaining ms)`. The id is signed: the sign picks
/// the line's colour only, and `abs(id)` names the DBC row (`0x52c9f9`).
pub(crate) type EnchantSlot = (u8, i32, u32, Option<u64>);

/// The tooltip lines an item's enchant slots contribute; every tooltip surface feeds through here.
/// The reference's per-slot gate (`0x52c9f9` to `0x52ca23`): `id != 0` and `abs(id)` names a
/// `SpellItemEnchantment` row, else nothing. Our own items carry all seven slots; another
/// player's descriptor carries only the permanent and temporary ones (vmangos).
pub(crate) fn enchant_lines(
    slots: impl IntoIterator<Item = EnchantSlot>,
    enchants: Option<&Enchants>,
) -> Vec<benilla_ui::script::EnchantView> {
    let lines = enchant_lines_quiet(slots, enchants);
    // Logged once per session: a missing enchant looks exactly like an unenchanted item.
    if !lines.is_empty() {
        static FIRST: std::sync::Once = std::sync::Once::new();
        // The whole view: the id, charges and countdown each ride a different wire lane.
        FIRST.call_once(|| info!("item enchant: {lines:?} (the first resolved this session)"));
    }
    lines
}

/// `0x5da2c0`: already bound when `ITEM_FIELD_FLAGS & 1` is set or a live enchant slot names a
/// binding `SpellItemEnchantment` row (`0x5da300` to `0x5da320`). The enchant cursor's bind
/// question (gate `0x495d60`) and the tooltip's Soulbound line both read it. It reads the raw
/// descriptor, not [`enchant_lines`]: Firestone and Orb of Fire both bind and print no line.
pub(crate) fn already_bound(fields: &ObjectFields, cat: Option<&Enchants>) -> bool {
    fields.item_flags().is_some_and(|f| f & 0x1 != 0)
        || (0..7).any(|slot| live_enchant(fields, slot, cat).is_some_and(|id| binds(id, cat)))
}

/// One enchant slot as the bind checks read it (`0x495eec`): the raw id must be positive (`jl` at
/// `0x495ef4`, `0x5da306`) and name a row, where the tooltip line uses `abs(id)`.
pub(crate) fn live_enchant(fields: &ObjectFields, slot: u8, cat: Option<&Enchants>) -> Option<u32> {
    let id = u32::try_from(fields.item_enchant(slot)?).ok()?;
    cat.is_some_and(|c| c.0.has_row(id)).then_some(id)
}

/// `SpellItemEnchantment.Flags & 1`: this enchant soulbinds the item it lands on.
pub(crate) fn binds(id: u32, cat: Option<&Enchants>) -> bool {
    cat.is_some_and(|c| c.0.binds_the_item(id))
}

/// [`enchant_lines`] without the log line, for the load-time resolve of the whole
/// `ItemRandomProperties` table ([`random_property_views`]).
fn enchant_lines_quiet(
    slots: impl IntoIterator<Item = EnchantSlot>,
    enchants: Option<&Enchants>,
) -> Vec<benilla_ui::script::EnchantView> {
    let Some(enchants) = enchants else {
        return Vec::new();
    };
    let lines: Vec<benilla_ui::script::EnchantView> = slots
        .into_iter()
        .filter(|&(_, id, _, _)| id != 0)
        // `SpellItemEnchantment.Flags & 0x2`: no line; both reference printers return before
        // reading the name (`0x6290e4`, `0x62923e`). Twelve rows: the totem weapon imbues,
        // Firestone, Orb of Fire.
        .filter(|&(_, id, _, _)| !enchants.0.tooltip_hides_name(id.unsigned_abs()))
        .filter_map(|(slot, id, charges, remaining_ms)| {
            let name = enchants.0.name(id.unsigned_abs())?.to_string();
            Some(benilla_ui::script::EnchantView {
                slot,
                name,
                negative: id < 0,
                charges,
                remaining_ms,
            })
        })
        .collect();
    lines
}

/// `ItemRandomProperties.dbc`, the random-suffix roll: a name suffix and tooltip enchant slots
/// 2..6 (`0x52b7e0`). Optional: absent, names stay unsuffixed.
#[derive(Resource)]
pub(crate) struct RandomProperties(pub(crate) benilla_formats::RandomPropertyCatalog);

/// The item's display name, the reference's one formatter `0x5d8b00(entry, randomPropertyId)`:
/// `ITEM_SUFFIX_TEMPLATE` (`"%s %s"`) with the roll's suffix, or the plain name when the id names
/// no suffixed row. Every surface that shows an item name goes through here.
pub(crate) fn item_display_name(
    base: &str,
    random_property_id: i32,
    props: Option<&RandomProperties>,
) -> String {
    match props.and_then(|p| p.0.get(random_property_id)) {
        Some(row) => format!("{base} {}", row.suffix),
        None => base.to_string(),
    }
}

/// The tooltip lines of a roll, for a source with no item object: the reference (`0x52b7bf` to
/// `0x52b7fb`) copies the row's five enchant ids into slots 2..6 and prints them as its own.
pub(crate) fn random_property_lines(
    row: &benilla_formats::RandomProperty,
    enchants: Option<&Enchants>,
) -> Vec<benilla_ui::script::EnchantView> {
    let slots = row.enchants.iter().enumerate().map(|(i, &id)| {
        (
            benilla_formats::RANDOM_PROPERTY_FIRST_SLOT + i as u8,
            id as i32,
            0,
            None,
        )
    });
    enchant_lines_quiet(slots, enchants)
}

/// Every `ItemRandomProperties` row as its suffix and named enchant lines, pushed to the engine
/// whole: a chat-link tooltip has no hover re-enter, so a per-id ask would miss the first click.
pub(crate) fn random_property_views(
    props: &RandomProperties,
    enchants: Option<&Enchants>,
) -> std::collections::HashMap<u32, benilla_ui::script::RandomPropertyView> {
    props
        .0
        .iter()
        .map(|(id, row)| {
            (
                id,
                benilla_ui::script::RandomPropertyView {
                    suffix: row.suffix.clone(),
                    enchants: random_property_lines(row, enchants),
                },
            )
        })
        .collect()
}

/// The two catalogs a random-suffix roll resolves through: the roll table and the enchant names.
#[derive(Clone, Copy)]
pub(crate) struct RollCatalogs<'a> {
    pub(crate) props: Option<&'a RandomProperties>,
    pub(crate) enchants: Option<&'a Enchants>,
}

impl RollCatalogs<'_> {
    /// No DBC catalogs: the plain name and no suffix lines.
    #[cfg(test)]
    pub(crate) const NONE: RollCatalogs<'static> = RollCatalogs {
        props: None,
        enchants: None,
    };

    /// [`item_display_name`] with this roll's suffix.
    pub(crate) fn name(&self, base: &str, random_property_id: u32) -> String {
        item_display_name(base, random_property_id as i32, self.props)
    }

    /// [`random_property_lines`] for one id; the live surfaces read the engine's pushed table.
    #[cfg(test)]
    pub(crate) fn lines(&self, random_property_id: u32) -> Vec<benilla_ui::script::EnchantView> {
        match self.props.and_then(|p| p.0.get(random_property_id as i32)) {
            Some(row) => random_property_lines(row, self.enchants),
            None => Vec::new(),
        }
    }
}

/// Item templates by entry, asked once, and the enchant times queued for items not yet held.
#[derive(Resource, Default)]
pub(crate) struct Items {
    /// A `None` answer is the server's unknown entry, cached so it is never re-asked.
    templates: QueryCache<u32, ItemInfo>,
    /// Landed since the last [`Self::take_fresh`]; pushed to the UI unprompted.
    fresh: Vec<u32>,
    /// The reference's `PlayerPendingItemExpiration` list (`0x5ebd40`, at `CGPlayer_C + 0x1cc8`):
    /// `(guid, slot, seconds)` enchant updates for an item not yet held. `0x5ebde0` applies them
    /// when the item is set up (`0x5d8440`), counting seconds from then; they live as long as the
    /// player, here the session. `SMSG_ITEM_TIME_UPDATE` has no such list.
    pending_enchant_times: Vec<(u64, u32, u32)>,
}

impl Items {
    /// Queue an `SMSG_ITEM_ENCHANT_TIME_UPDATE` for an item not held. The caller has checked the
    /// active player resolves: the reference drops the update when none does.
    pub(crate) fn queue_enchant_time(&mut self, guid: u64, slot: u32, seconds: u32) {
        debug!("enchant timer: item {guid:#x} not held — queued for its arrival");
        self.pending_enchant_times.push((guid, slot, seconds));
    }

    /// The item `guid` arrived: its queued enchant updates in arrival order, unlinked (`0x5ebde0`).
    pub(crate) fn take_enchant_times(&mut self, guid: u64) -> Vec<(u32, u32)> {
        let mut taken = Vec::new();
        self.pending_enchant_times.retain(|&(g, slot, seconds)| {
            let hit = g == guid;
            if hit {
                taken.push((slot, seconds));
            }
            !hit
        });
        taken
    }

    /// The template for `entry` if known; a miss asks once per entry per connection (`guid` rides
    /// along, `0` for template-only). A cached negative is also `None`, without a re-ask.
    pub(crate) fn template(
        &self,
        entry: u32,
        guid: u64,
        commands: &NetCommands,
    ) -> Option<&ItemInfo> {
        self.templates.get_or_ask(entry, || {
            debug!("items: asking template (entry {entry})");
            let _ = commands.0.send(ClientCommand::ItemQuery { entry, guid });
        })
    }

    /// Whether the server answered `entry` as unknown, not still pending; the cast-fail redisplay
    /// then shows the reference's `"UNKNOWN"` instead of waiting.
    pub(crate) fn template_answered_unknown(&self, entry: u32) -> bool {
        self.templates.answered_unknown(entry)
    }

    /// The [`HeldTemplate`] view of [`Self::template`], asked template-only.
    pub(crate) fn held(&self, entry: u32, commands: &NetCommands) -> Option<HeldTemplate> {
        self.template(entry, 0, commands).map(|i| HeldTemplate {
            display_info_id: i.display_info_id,
            inventory_type: i.inventory_type,
            sheath: i.sheath,
            class: i.class,
            subclass: i.subclass,
            material: i.material,
        })
    }

    /// [`Self::template`] without the ask: the cached record only.
    pub(crate) fn template_cached(&self, entry: u32) -> Option<&ItemInfo> {
        self.templates.get(entry)
    }

    /// Record a template answer (`SMSG_ITEM_QUERY_SINGLE_RESPONSE`); `None` = unknown entry.
    pub(crate) fn insert_template(&mut self, entry: u32, info: Option<ItemInfo>) {
        if info.is_some() {
            self.fresh.push(entry);
        }
        // A negative answer moves the generation too: "still asking" to "answered unknown" is a
        // display transition for the cast-fail redisplay.
        self.templates.insert(entry, info);
    }

    /// Bumped by every landed answer, for consumers besides the drain; the stand-in for the
    /// reference's `DBCACHECALLBACK` redisplay (`0x6e29b0`).
    pub(crate) fn template_epoch(&self) -> u64 {
        self.templates.generation()
    }

    /// Every entry with a cached template.
    pub(crate) fn cached_template_ids(&self) -> Vec<u32> {
        self.templates
            .iter()
            .filter_map(|(&id, t)| t.is_some().then_some(id))
            .collect()
    }

    /// Drain the entries whose template landed since the last drain.
    pub(crate) fn take_fresh(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.fresh)
    }

    /// World enter: release the ask-once latch and keep what the cache learned. An ask is marked
    /// pending before the send, and a send made while no writer is held is dropped silently.
    pub(crate) fn clear_pending(&mut self) {
        self.templates.clear_pending();
    }

    /// Disconnect: drop the in-flight asks and the queued enchant times, keep the templates. The
    /// item objects and their countdowns go with the index's sweep.
    pub(crate) fn clear_session(&mut self) {
        self.pending_enchant_times.clear();
        self.templates.clear_pending();
    }
}

impl crate::query_cache::AskOnce for Items {
    fn clear_pending(&mut self) {
        Items::clear_pending(self);
    }
}

/// The ask-once test fixture; it holds the live receiver, without which every send fails.
#[cfg(test)]
pub(crate) struct TestDeps {
    pub(crate) items: Items,
    pub(crate) commands: NetCommands,
    /// The object index: seed with [`Self::spawn_item`], read through [`Self::with_objects`].
    pub(crate) world: World,
    rx: crossbeam_channel::Receiver<ClientCommand>,
}

#[cfg(test)]
impl TestDeps {
    pub(crate) fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.init_resource::<GuidIndex>();
        TestDeps {
            items: Items::default(),
            commands: NetCommands(tx),
            world,
            rx,
        }
    }

    /// An item object in the index, as the wire's `ItemCreate` spawns it.
    pub(crate) fn spawn_item(&mut self, guid: u64, fields: ObjectFields) -> Entity {
        test_spawn_item(&mut self.world, guid, fields, false)
    }

    /// Run `f` with the object lookup, the template cache and the command channel borrowed apart.
    pub(crate) fn with_objects<R>(
        &mut self,
        f: impl FnOnce(&Objects, &Items, &NetCommands) -> R,
    ) -> R {
        let (world, items, commands) = (&mut self.world, &self.items, &self.commands);
        let mut state = bevy::ecs::system::SystemState::<Objects>::new(world);
        let objects = state.get(world);
        f(&objects, items, commands)
    }

    /// The entries asked of the server, observable while the icon they feed is still `None`.
    pub(crate) fn queried_entries(&self) -> Vec<u32> {
        self.rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::ItemQuery { entry, .. } => Some(entry),
                _ => None,
            })
            .collect()
    }
}

/// [`spawn_item`] straight into a test world, without the command queue.
#[cfg(test)]
pub(crate) fn test_spawn_item(
    world: &mut World,
    guid: u64,
    fields: ObjectFields,
    container: bool,
) -> Entity {
    let e = world
        .spawn((Guid(guid), ObjectStore(fields), ItemObject { container }))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(guid, e);
    e
}

/// `EQUIPMENT_SLOT_MAINHAND` / `_OFFHAND` (vmangos `EquipmentSlots`).
pub(crate) const EQUIPMENT_SLOT_MAINHAND: u8 = 15;
pub(crate) const EQUIPMENT_SLOT_OFFHAND: u8 = 16;

/// One equipment slot's item class; `None` while empty or its template is in flight.
fn equipped_class(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    slot: u8,
) -> Option<u8> {
    let guid = store.0.player_inv_slot(slot).filter(|&g| g != 0)?;
    let entry = objects.object(guid).and_then(|o| o.object_entry())?;
    Some(items.template(entry, guid, commands)?.class as u8)
}

/// The equipment slot this character's disarm hides ([`crate::creature_anim::disarmed_hand`] over
/// the raw inventory), for the action bar's equipped-item requirement (`0x5f0c50`) and the
/// item-use refusal (`CGItem::Use` rung 15). `None` while the flag is down or neither hand holds
/// a weapon.
pub(crate) fn disarmed_equipment_slot(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> Option<u8> {
    if store.0.unit_flags() & crate::creature_anim::UNIT_FLAG_DISARMED == 0 {
        return None;
    }
    let main = equipped_class(store, objects, items, commands, EQUIPMENT_SLOT_MAINHAND);
    let off = equipped_class(store, objects, items, commands, EQUIPMENT_SLOT_OFFHAND);
    crate::creature_anim::disarmed_hand(main, off).map(|hand| EQUIPMENT_SLOT_MAINHAND + hand as u8)
}

/// [`disarmed_equipment_slot`] without the ask.
pub(crate) fn disarmed_equipment_slot_cached(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
) -> Option<u8> {
    if store.0.unit_flags() & crate::creature_anim::UNIT_FLAG_DISARMED == 0 {
        return None;
    }
    let class_of = |slot: u8| -> Option<u8> {
        let guid = store.0.player_inv_slot(slot).filter(|&g| g != 0)?;
        let entry = objects.object(guid).and_then(|o| o.object_entry())?;
        Some(items.template_cached(entry)?.class as u8)
    };
    crate::creature_anim::disarmed_hand(
        class_of(EQUIPMENT_SLOT_MAINHAND),
        class_of(EQUIPMENT_SLOT_OFFHAND),
    )
    .map(|hand| EQUIPMENT_SLOT_MAINHAND + hand as u8)
}

/// A minimal valid item template named `name`; the sentinels that matter are `allowable_*` = -1
/// and `stackable` = 1.
#[cfg(test)]
pub(crate) fn test_template(name: &str) -> ItemInfo {
    ItemInfo {
        class: 0,
        subclass: 0,
        name: name.into(),
        display_info_id: 1,
        quality: 1,
        flags: 0,
        buy_price: 0,
        sell_price: 0,
        inventory_type: 0,
        allowable_class: -1,
        allowable_race: -1,
        item_level: 0,
        required_level: 0,
        required_skill: 0,
        required_skill_rank: 0,
        required_spell: 0,
        required_honor_rank: 0,
        required_city_rank: 0,
        required_rep_faction: 0,
        required_rep_rank: 0,
        max_count: 0,
        stackable: 1,
        container_slots: 0,
        stats: Vec::new(),
        damages: Vec::new(),
        dmg_min: 0.0,
        dmg_max: 0.0,
        dmg_type: 0,
        armor: 0,
        resistances: [0; 6],
        delay_ms: 0,
        ammo_type: 0,
        ranged_mod_range: 0.0,
        spells: Vec::new(),
        spell_charges_0: 0,
        use_spell: None,
        bonding: 0,
        description: String::new(),
        page_text: 0,
        language_id: 0,
        page_material: 0,
        start_quest: 0,
        lock_id: 0,
        material: 0,
        sheath: 0,
        random_property: 0,
        block: 0,
        item_set: 0,
        max_durability: 0,
        area: 0,
        map: 0,
        bag_family: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_template as info;
    use benilla_protocol::messages::{
        ITEM_DYNFLAG_UNLOCKED, ITEM_DYNFLAG_WRAPPED, ITEM_FLAG_LOOTABLE, ITEM_FLAG_WRAPPER,
    };
    use crossbeam_channel::TryRecvError;
    use std::collections::HashMap;

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    #[test]
    fn template_miss_queries_once_then_serves_the_answer() {
        let (cmds, rx) = commands();
        let mut items = Items::default();

        assert!(items.template(117, 0x42, &cmds).is_none());
        // Second copy of the same item: no second query.
        assert!(items.template(117, 0x43, &cmds).is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::ItemQuery {
                entry: 117,
                guid: 0x42
            })
        ));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        items.insert_template(117, Some(info("Tough Jerky")));
        assert_eq!(
            items.template(117, 0x43, &cmds).map(|i| i.name.as_str()),
            Some("Tough Jerky")
        );
    }

    #[test]
    fn the_template_epoch_counts_landings_and_asks_count_nothing() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        let t0 = items.template_epoch();

        assert!(items.template(117, 0x42, &cmds).is_none());
        assert_eq!(
            items.template_epoch(),
            t0,
            "an ask-once miss is not a change"
        );
        items.insert_template(117, Some(info("Tough Jerky")));
        assert_ne!(items.template_epoch(), t0, "a landing is");
    }

    /// The step count is bounded, not exact: the test cannot pin the sub-second phase.
    #[test]
    fn countdown_steps_move_by_displayable_seconds() {
        let now = Instant::now();
        let mut c = Countdowns::default();
        assert_eq!(c.steps(now), 0, "no deadlines, no steps");

        assert!(c.set_enchant(1, 90));
        let steps = c.steps(Instant::now());
        assert!(
            (90..=91).contains(&steps),
            "a 90 s deadline contributes floor(secs)+1, got {steps}"
        );
        let shown = c.enchant_remaining_display_ms(1).unwrap();
        assert_eq!(shown % 1000, 0, "the display read is second-floored");
        assert!(shown <= 90_000);

        c.set_lifetime(1800);
        let shown = c.lifetime_remaining_display_ms().expect("parked");
        assert_eq!(shown % 1000, 0);
        assert!(shown <= 1_800_000);
        let steps = c.steps(Instant::now());
        assert!(
            (1890..=1892).contains(&steps),
            "both cells step, got {steps}"
        );

        assert!(c.set_enchant(1, 0));
        c.set_lifetime(0);
        assert_eq!(c.steps(Instant::now()), 0, "cleared cells, terms gone");
        assert_eq!(c.enchant_remaining_display_ms(1), None);
        assert_eq!(c.lifetime_remaining_ms(), None);
    }

    #[test]
    fn the_setters_clear_on_signed_non_positive_and_refuse_a_slot_past_the_array() {
        let mut c = Countdowns::default();
        c.set_lifetime(600);
        assert!(c.lifetime_remaining_ms().is_some());
        c.set_lifetime(0x8000_0000);
        assert_eq!(
            c.lifetime_remaining_ms(),
            None,
            "a negative-as-signed duration clears, it does not park a far future"
        );

        assert!(c.set_enchant(1, 600));
        assert!(c.enchant_remaining_ms(1).is_some());
        assert!(c.set_enchant(1, 0xffff_fff0));
        assert_eq!(
            c.enchant_remaining_ms(1),
            None,
            "the enchant setter's test is the same signed `jle`"
        );

        assert!(c.set_enchant(6, 30), "the seventh slot is the last cell");
        let before = c.clone();
        assert!(!c.set_enchant(7, 30), "the eighth is past the array");
        assert!(!c.set_enchant(u32::MAX, 30));
        assert_eq!(c, before, "a refused slot writes nothing");

        assert_eq!(c.enchant_deadline_ms(0), None, "never set");
        assert!(c.enchant_deadline_ms(6).is_some_and(|ms| ms <= 30_000));
    }

    #[test]
    fn landed_templates_drain_as_fresh_once() {
        let mut items = Items::default();
        items.insert_template(117, Some(info("Tough Jerky")));
        items.insert_template(9999, None);
        assert_eq!(items.take_fresh(), vec![117]);
        assert!(items.take_fresh().is_empty(), "a drain empties the queue");
    }

    /// The tooltip's open line waits for an unlocked instance; the click is the bare template bit,
    /// so a locked junkbox still sends and the server refuses.
    #[test]
    fn open_line_carries_the_lock_gate_the_open_send_does_not() {
        let plain = |flags: u32, lock_id: u32| {
            let mut t = info("Small Barnacled Clam");
            t.flags = flags;
            t.lock_id = lock_id;
            t
        };

        // The clam: lootable, no lock; line and send agree.
        assert!(plain(ITEM_FLAG_LOOTABLE, 0).shows_open_line(0));
        assert!(plain(ITEM_FLAG_LOOTABLE, 0).opens_loot());
        // An ordinary item does neither, however its instance is flagged.
        assert!(!plain(0, 0).shows_open_line(ITEM_DYNFLAG_UNLOCKED | ITEM_DYNFLAG_WRAPPED));
        assert!(!plain(0, 0).opens_loot());
        // A junkbox, lootable but locked: no line until unlocked, but the click goes out.
        assert!(!plain(ITEM_FLAG_LOOTABLE, 7).shows_open_line(0));
        assert!(plain(ITEM_FLAG_LOOTABLE, 7).shows_open_line(ITEM_DYNFLAG_UNLOCKED));
        assert!(
            plain(ITEM_FLAG_LOOTABLE, 7).opens_loot(),
            "the send ignores LockID entirely — the server owns the refusal"
        );
        // Gift wrap unwraps only while the instance is still wrapped, its own dispatcher arm.
        assert!(!plain(ITEM_FLAG_WRAPPER, 0).unwraps_gift(0));
        assert!(plain(ITEM_FLAG_WRAPPER, 0).unwraps_gift(ITEM_DYNFLAG_WRAPPED));
        assert!(
            !plain(ITEM_FLAG_WRAPPER, 0).opens_loot(),
            "WRAPPER is not LOOTABLE"
        );
        assert!(plain(ITEM_FLAG_WRAPPER, 0).shows_open_line(ITEM_DYNFLAG_WRAPPED));
        // A wrapped gift of a locked box: the gift arm ignores the lock gate on both sides.
        let gift_box = plain(ITEM_FLAG_WRAPPER | ITEM_FLAG_LOOTABLE, 7);
        assert!(gift_box.unwraps_gift(ITEM_DYNFLAG_WRAPPED));
        assert!(gift_box.shows_open_line(ITEM_DYNFLAG_WRAPPED));
    }

    #[test]
    fn negative_template_answer_is_cached() {
        let (cmds, rx) = commands();
        let mut items = Items::default();

        assert!(items.template(9999, 0, &cmds).is_none());
        let _ = rx.try_recv();
        items.insert_template(9999, None); // server: unknown entry
        assert!(items.template(9999, 0, &cmds).is_none());
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "no re-ask"
        );
    }

    #[test]
    fn an_item_is_an_object_in_the_index_and_its_changes_are_watched() {
        use bevy::ecs::system::RunSystemOnce;
        const GUID: u64 = 0x4000_0000_0000_0042;
        let mut world = World::new();
        world.init_resource::<GuidIndex>();
        world
            .run_system_once(|mut commands: Commands, mut index: ResMut<GuidIndex>| {
                spawn_item(
                    &mut commands,
                    &mut index,
                    GUID,
                    ObjectFields::from_pairs(&[(3, 117), (14, 5)]),
                    false,
                );
            })
            .unwrap();
        // A registered system keeps its change ticks between runs (a one-shot would read every
        // store as new every time).
        let read = world.register_system(
            |objects: Objects, mut changes: ItemChanges| -> (Option<u32>, Option<u32>, bool) {
                let o = objects.object(GUID);
                (
                    o.and_then(|o| o.object_entry()),
                    o.and_then(|o| o.item_stack_count()),
                    changes.moved(),
                )
            },
        );
        assert_eq!(
            world.run_system(read).unwrap(),
            (Some(117), Some(5), true),
            "the spawn is a change"
        );
        assert_eq!(
            world.run_system(read).unwrap(),
            (Some(117), Some(5), false),
            "nothing moved since"
        );
        // A values delta lands in the store and moves the watch once.
        let e = world.resource::<GuidIndex>().0[&GUID];
        world
            .get_mut::<ObjectStore>(e)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[(14, 4)]));
        assert_eq!(world.run_system(read).unwrap(), (Some(117), Some(4), true));
        assert!(!world.run_system(read).unwrap().2);
        // A countdown landing is the item's own change, and its cell joins the step sum.
        world
            .get_mut::<Countdowns>(e)
            .expect("every item carries its cells")
            .set_enchant(1, 90);
        assert!(
            world.run_system(read).unwrap().2,
            "a landing moves the watch"
        );
        let steps = world
            .run_system_once(|c: ItemChanges| c.countdown_steps())
            .unwrap();
        assert!((90..=91).contains(&steps), "got {steps}");
        // The destroy: the index drops the guid, the entity goes, the watch moves once more.
        world.resource_mut::<GuidIndex>().0.remove(&GUID);
        world.despawn(e);
        assert_eq!(world.run_system(read).unwrap(), (None, None, true));
        assert!(!world.run_system(read).unwrap().2);
    }

    #[test]
    fn a_queued_enchant_time_waits_for_its_item_and_the_session_keeps_templates() {
        let mut items = Items::default();
        items.queue_enchant_time(0x42, 1, 30);
        items.queue_enchant_time(0x43, 1, 60);
        items.queue_enchant_time(0x42, 1, 45);
        assert_eq!(
            items.take_enchant_times(0x42),
            vec![(1, 30), (1, 45)],
            "arrival order — the later record is applied last and wins"
        );
        assert!(
            items.take_enchant_times(0x42).is_empty(),
            "unlinked once replayed"
        );

        // A disconnect drops unarrived records and in-flight asks, and keeps the templates.
        let (cmds, rx) = commands();
        items.insert_template(117, Some(info("Tough Jerky")));
        assert!(items.template(118, 0, &cmds).is_none()); // leaves 118 in flight
        items.clear_session();
        assert!(items.take_enchant_times(0x43).is_empty());
        assert!(items.template(117, 0, &cmds).is_some(), "templates survive");
        // 118's ask was dropped with the writer, so it re-asks now.
        let _ = rx.try_recv();
        assert!(items.template(118, 0, &cmds).is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::ItemQuery { entry: 118, .. })
        ));
    }

    /// `abs(id)` names the row and the sign only travels (`0x52c9f9`).
    #[test]
    fn enchant_lines_join_named_ids_in_slot_order() {
        let cat = Enchants(benilla_formats::EnchantCatalog::from_rows(
            Default::default(),
            [
                (2564, "Agility +15".to_string()),
                (1900, "Crusader".to_string()),
            ]
            .into_iter()
            .collect(),
            Default::default(),
        ));
        let named = |v: Vec<benilla_ui::script::EnchantView>| -> Vec<String> {
            v.into_iter().map(|e| e.name).collect()
        };
        assert_eq!(
            named(enchant_lines(
                [(0, 2564, 0, None), (1, 0, 0, None), (4, 1900, 0, None)],
                Some(&cat)
            )),
            vec!["Agility +15".to_string(), "Crusader".to_string()]
        );
        // A negative id resolves off `abs(id)` and carries its sign onward.
        let neg = enchant_lines([(0, -2564, 0, None)], Some(&cat));
        assert_eq!(neg.len(), 1);
        assert_eq!(neg[0].name, "Agility +15");
        assert!(neg[0].negative, "the sign travels to the colour rule");
        // Charges and a countdown ride the slot through untouched (the engine formats them).
        let temp = enchant_lines([(1, 1900, 5, Some(90_000))], Some(&cat));
        assert_eq!((temp[0].charges, temp[0].remaining_ms), (5, Some(90_000)));
        // An id the table does not name contributes nothing.
        assert!(enchant_lines([(0, 999_999, 0, None)], Some(&cat)).is_empty());
        // No DBC, no lines.
        assert!(enchant_lines([(0, 2564, 0, None)], None).is_empty());
    }

    /// `SpellItemEnchantment` field 23 for three synthetic rows: one that binds, one that binds
    /// AND hides its tooltip line (the Firestone shape), one that does neither.
    fn catalog() -> Enchants {
        Enchants(benilla_formats::EnchantCatalog::from_rows(
            HashMap::new(),
            HashMap::new(),
            HashMap::from([(11, 0x1), (12, 0x1 | 0x2), (13, 0x0)]),
        ))
    }

    /// One item object with the given `ITEM_FIELD_FLAGS` and enchant slot 0.
    fn item(flags: u32, slot0: u32) -> ObjectFields {
        ObjectFields::from_pairs(&[(21, flags), (22, slot0)])
    }

    /// `0x5da2c0`: the Firestone shape binds and prints no line, so a predicate read off
    /// [`enchant_lines`] would call it unbound.
    #[test]
    fn the_bind_predicate_reads_the_descriptor_not_the_rendered_lines() {
        let cat = catalog();
        let cat = Some(&cat);
        assert!(
            already_bound(&item(0x1, 0), cat),
            "ITEM_FIELD_FLAGS & 1 — already soulbound"
        );
        assert!(
            !already_bound(&item(0x0, 0), cat),
            "no flag, no enchant — the plain BoE"
        );
        assert!(
            !already_bound(&item(0x8, 0), cat),
            "a WRAPPED gift is not a bound item — the bit is 0x1, not any bit"
        );
        assert!(
            already_bound(&item(0x0, 11), cat),
            "a live enchant slot naming a binding row"
        );
        assert!(
            !already_bound(&item(0x0, 13), cat),
            "a non-binding enchant leaves the item unbound"
        );
        assert!(
            !already_bound(&item(0x0, 99), cat),
            "an id that names no row binds nothing (the ref's `testl` after the table load)"
        );
        assert!(
            !already_bound(&item(0x0, 11), None),
            "no catalog loaded — the enchant half cannot answer, and does not guess"
        );
        // The case that pins the design: bound, and no line.
        assert!(
            already_bound(&item(0x0, 12), cat),
            "the Firestone shape — binds AND hides its line"
        );
        assert!(
            enchant_lines_quiet([(0u8, 12i32, 0u32, None)], cat).is_empty(),
            "…and the rendered lines are empty for it, which is why they are not the source"
        );
    }
}
