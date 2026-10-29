//! The item layer: item objects in the guid index and the item template cache.
//!
//! An item is `Guid` + [`ObjectStore`] + [`ItemObject`] in the guid index; item fields are private,
//! so only our own arrive. Templates are keyed by entry, asked once, and survive disconnect.

use std::time::{Duration, Instant};

use benilla_assets::LockRecover;
use bevy::prelude::*;

use benilla_protocol::{ItemInfo, ObjectFields};

use bevy::ecs::system::SystemParam;

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, Objects};
use crate::query_cache::{Ask, QueryCache};

/// An item or container entity's kind (`TYPEMASK_ITEM` / `TYPEMASK_CONTAINER`), with no model or
/// pose. Unit readers filter it out (`Without<ItemObject>`): an item block's dwords overlap the
/// unit block's indices.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ItemObject {
    /// `TYPEMASK_CONTAINER`: a bag's slot array lives past the item block.
    pub(crate) container: bool,
}

/// Spawn an item object into the index with `fields` as its store, notifying nothing: the update
/// pass runs its create hook or its watches ([`crate::net::FieldChanged`]).
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
    /// would push a new snapshot every frame while a timer runs.
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

impl HeldTemplate {
    pub(crate) fn of(i: &ItemInfo) -> Self {
        Self {
            display_info_id: i.display_info_id,
            inventory_type: i.inventory_type,
            sheath: i.sheath,
            class: i.class,
            subclass: i.subclass,
            material: i.material,
        }
    }
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

/// The name behind a wrapped item's `ITEM_WRAPPED_BY` line: only an item carrying WRAPPED `0x8`
/// takes the gift arm (`0x52b789`, `0x52e1b9`), which reads `ITEM_FIELD_GIFTCREATOR` (`0x52e32a`)
/// and asks the name cache (`0x52e29d`). A zero guid, an unwrapped item or a name not yet
/// answered gives `None`. Only reads: the lookup is the tooltip build's, which asks it
/// in its call ([`benilla_ui::script::RecordAsk::ItemBuildName`]).
pub(crate) fn gift_creator_name(
    fields: &ObjectFields,
    names: &crate::names::NameCache,
) -> Option<String> {
    if fields.item_flags()? & benilla_protocol::messages::ITEM_DYNFLAG_WRAPPED == 0 {
        return None;
    }
    let giver = fields.item_gift_creator()?;
    names.resolve_player(giver, Ask::Read).map(str::to_string)
}

/// The creator's guid (`ITEM_FIELD_CREATOR`, asked at `0x52e1f4`) and, for a wrapped item only,
/// the giver's (`0x52e28d`), 0 where nothing is asked: the tooltip waits on a nonzero one whose
/// name is not in yet.
pub(crate) fn creator_guids(fields: &ObjectFields) -> (u64, u64) {
    let wrapped = fields
        .item_flags()
        .is_some_and(|f| f & benilla_protocol::messages::ITEM_DYNFLAG_WRAPPED != 0);
    (
        fields.item_creator().unwrap_or(0),
        if wrapped {
            fields.item_gift_creator().unwrap_or(0)
        } else {
            0
        },
    )
}

/// `0x5da2c0`: already bound when `ITEM_FIELD_FLAGS & 1` is set or, for an item whose record
/// flags ([`record_flags`]) lack `0x2000` (`0x5da2e3`, a charter's), a live
/// enchant slot names a binding `SpellItemEnchantment` row (`0x5da300` to `0x5da320`). The enchant
/// cursor's bind question (gate `0x495d60`) and the tooltip's Soulbound line both read it. It
/// reads the raw descriptor, not [`enchant_lines`]: Firestone and Orb of Fire both bind and print
/// no line.
pub(crate) fn already_bound(
    fields: &ObjectFields,
    record_flags: u32,
    cat: Option<&Enchants>,
) -> bool {
    fields.item_flags().is_some_and(|f| f & 0x1 != 0)
        || (record_flags & benilla_protocol::messages::ITEM_FLAG_CHARTER == 0
            && (0..7).any(|slot| live_enchant(fields, slot, cat).is_some_and(|id| binds(id, cat))))
}

/// The item record's PageText as `0x5d9e10` reads it (`[rec+0x19c]`): the cached template's, 0
/// while it is not cached. Only read.
pub(crate) fn record_page_text(items: &Items, entry: u32, guid: u64) -> u32 {
    items
        .template(entry, guid, Ask::Read)
        .map_or(0, |t| t.page_text)
}

/// The item record's flags as `0x5da2a0` reads them: the cached template's, 0 while it is not
/// cached. Only read.
pub(crate) fn record_flags(items: &Items, entry: u32, guid: u64) -> u32 {
    items
        .template(entry, guid, Ask::Read)
        .map_or(0, |t| t.flags)
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
pub(crate) struct RandomProperties {
    pub(crate) catalog: benilla_formats::RandomPropertyCatalog,
    /// `ITEM_SUFFIX_TEMPLATE` as `FrameScript_GetText` reads it off the VM (`0x5d8b7f` →
    /// `0x703bf0`), the empty string for a missing key, copied as each frame's Lua ends
    /// ([`crate::ui_items::feed::copy_suffix_template`]); `None` until a VM has been read.
    pub(crate) suffix_template: Option<String>,
}

impl RandomProperties {
    pub(crate) fn new(catalog: benilla_formats::RandomPropertyCatalog) -> Self {
        RandomProperties {
            catalog,
            suffix_template: None,
        }
    }
}

/// The item's display name, the reference's one formatter `0x5d8b00(entry, randomPropertyId)`:
/// `SStrPrintf(ITEM_SUFFIX_TEMPLATE, name, suffix)` (`0x5d8b78`-`0x5d8b8c`) for an id naming a
/// suffixed row, else the plain name (`0x5d8ba5`). The template is the VM's
/// ([`RandomProperties::suffix_template`]); before a VM has been read, the stock `"%s %s"`. Every
/// surface that shows an item name goes through here.
pub(crate) fn item_display_name(
    base: &str,
    random_property_id: i32,
    props: Option<&RandomProperties>,
) -> String {
    let Some((props, row)) = props.and_then(|p| Some((p, p.catalog.get(random_property_id)?)))
    else {
        return base.to_string();
    };
    let template = props.suffix_template.as_deref().unwrap_or("%s %s");
    benilla_ui::strings::fill(
        template,
        &[
            benilla_ui::strings::Arg::S(base),
            benilla_ui::strings::Arg::S(&row.suffix),
        ],
    )
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
        .catalog
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

/// The two roll catalogs as a system reads them; absent in a world that loaded no DBCs.
#[derive(SystemParam)]
pub(crate) struct Rolls<'w> {
    props: Option<Res<'w, RandomProperties>>,
    enchants: Option<Res<'w, Enchants>>,
}

impl Rolls<'_> {
    /// The catalogs, as [`RollCatalogs`].
    pub(crate) fn get(&self) -> RollCatalogs<'_> {
        RollCatalogs {
            props: self.props.as_deref(),
            enchants: self.enchants.as_deref(),
        }
    }
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
        match self
            .props
            .and_then(|p| p.catalog.get(random_property_id as i32))
        {
            Some(row) => random_property_lines(row, self.enchants),
            None => Vec::new(),
        }
    }
}

/// Item templates by entry, asked once, and the enchant times queued for items not yet held.
#[derive(Resource, Default)]
pub(crate) struct Items {
    /// A refused entry is the server's unknown one, read as such by
    /// [`Items::template_answered_unknown`] and asked again by the next event's lookup
    /// ([`QueryCache::refuse`]); it is never kept as a record.
    templates: QueryCache<u32, ItemInfo>,
    /// Each entry's last record change: the change count it landed or went at, whether it is held
    /// now, and whether the VM's copy took it at its answer ([`Self::vm_took`]). Read, never
    /// drained, by whoever follows the records ([`Self::changed_since`]): the item-tooltip feed
    /// pushes a landed record unprompted and drops a removed one.
    changes: std::collections::HashMap<u32, (u64, bool, bool)>,
    /// How many record changes [`Self::changes`] has seen.
    change_count: u64,
    /// The reference's `PlayerPendingItemExpiration` list (`0x5ebd40`, at `CGPlayer_C + 0x1cc8`):
    /// `(guid, slot, seconds)` enchant updates for an item not yet held. `0x5ebde0` applies them
    /// when the item is set up (`0x5d8440`), counting seconds from then; they live as long as the
    /// player, here the session. `SMSG_ITEM_TIME_UPDATE` has no such list.
    pending_enchant_times: Vec<(u64, u32, u32)>,
    /// The callbacks the lookups that missed registered, each with the entry it waits on, in the
    /// order they asked: an answer runs its entry's in that order (`0x55ba30` keeps each with no
    /// dedupe for the dedupe flag 0, `dbcache/callbacks#callback-order`). A getter registers one
    /// from a lookup, so the list is locked like the in-flight asks and registering takes `&self`.
    callbacks: std::sync::Mutex<Vec<(u32, RecordCallback)>>,
    /// The equip callbacks a lookup ran on a record the cache held and the VM's copy lacked,
    /// `(entry, callback, found)` in the order they ran
    /// ([`crate::ui_bind_confirm::replay_equip_records`]).
    vm_callbacks: std::sync::Mutex<Vec<(u32, RecordCallback, bool)>>,
    /// The refusals' walks: the one running now, and each of this drain's, whose signals the
    /// drain fires under its marks ([`Self::walk_takes`]).
    walks: std::sync::Mutex<RefusalWalks>,
}

/// [`Items::walks`].
#[derive(Default)]
struct RefusalWalks {
    now: Option<RefusalWalk>,
    done: Vec<RefusalWalk>,
}

/// A refused entry's callbacks run with status 0 (`0x55be91`-`0x55bead`), while its node stands
/// with no record. A lookup in a callback links its own at the head (`0x55ba30`), and the walk
/// goes from the tail toward the head, reading each link before the call (`0x55be9c`): it reaches
/// one a callback adds unless the adder was at the head, whose link was read as the end. The
/// node's destroy frees what is left uncalled (`0x55bed2`). Nothing is sent meanwhile: the lookup
/// finds the node.
struct RefusalWalk {
    entry: u32,
    /// The list as it stands, walked and to walk, for the dedupe scan (`0x55bbb3`).
    list: Vec<RecordCallback>,
    /// The next to run.
    next: usize,
    /// Whether the callback running now was the last when it began.
    last: bool,
    /// The marks around the walk, and the start of its last callback: a signal stamped between
    /// `from` and `last_from` was raised by a callback with a next.
    from: crate::lanes::Mark,
    last_from: crate::lanes::Mark,
    to: crate::lanes::Mark,
    /// The callbacks a listener of the walk's signals added, which the walk reaches
    /// ([`Items::take_walk_tail`]), and whether one came from a listener of a callback with a
    /// next, so the last callback's next is no longer the end.
    tail: Vec<RecordCallback>,
    extended: bool,
}

/// A callback an item record's lookup registered, which the record's answer runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordCallback {
    /// An item's create set-up (`0x5d8300` asks with `0x5d8390`, `0x5d8321`-`0x5d833f`).
    SetUp(u64),
    /// An item of ours whose entry changed to one the cache missed (`0x5d9652` asks with
    /// `0x5d96b0`).
    Entry(u64),
    /// A loot row's record, asked by the copier (`0x4c1e77` with `0x4c2ac0`).
    LootRow,
    /// An item push's record (`0x491b05` with `0x491ee0`).
    Push,
    /// A trade getter call (`0x4c017f`, `0x4c0582` with `0x4c0340`), one per call.
    Trade,
    /// An ammo-slot getter call (`0x4c83bc`, `0x4c89cf` with `0x4c84f0`), one per call.
    Ammo,
    /// A swap parked on its record (`0x5e0db1` with `0x5e1020`).
    Swap,
    /// An auto-equip waiting on its record (`0x5e1603` with `0x5e17a0`).
    AutoEquip,
    /// A vendor row's, asked by `GetMerchantItemInfo` (`0x4faf1a` with `0x4fada0`), one per call.
    Merchant,
    /// A buy-back row's, asked by `GetBuybackItemInfo` (`0x4fb40c` with `0x4fb570`), one per call.
    Buyback,
    /// A trainer row's product, asked by `GetTrainerServiceIcon` and `GetTrainerServiceDescription`
    /// (`0x4d907e` with `0x4d9140`, `0x4d9cef` with `0x4d9db0`), one per call.
    Trainer,
    /// A profession getter's, asked by `GetTradeSkillIcon`, `GetTradeSkillItemStats`,
    /// `GetTradeSkillReagentInfo` and `GetTradeSkillTools` (`0x4fdb90`, `0x4fdf77`, `0x4ff6cd`,
    /// `0x4ffac6` with `0x4fdc30`), one per call.
    TradeSkill,
    /// A profession list build's product (`0x4fcd4d` with `0x4fc8e0`), one per row that missed.
    TradeSkillBuild,
    /// A craft getter's, asked by `GetCraftReagentInfo` and `GetCraftSpellFocus` (`0x4f75dd`,
    /// `0x4f79f2` with `0x4f7710`), one per call.
    Craft,
    /// A questgiver row's, asked by `GetQuestItemInfo` (`0x5017de` with `0x5018c0`), one per call.
    Quest,
    /// A turn-in's reward line whose item was not cached (`0x5dc647`, `0x5dc6a1` with
    /// `0x5dc7b0`), one per line.
    QuestReward,
    /// A mail row's, asked by `GetInboxHeaderInfo` and `GetInboxItem` (`0x4aecc7`, `0x4aed63`,
    /// `0x4af66f` with `0x4af0a0`), one per call, or an auction notice's at the list parse
    /// (`0x4acf38` with `0x4ad010`), one per notice.
    Mail,
    /// An auction notice's line whose item was not cached (`0x4ccf6e`, `0x4cd3eb`, `0x4cd4ef` with
    /// `0x4cd190`), one per line.
    AuctionMessage,
    /// The attachment's, asked by `GetSendMailItem` (`0x4ae604` with `0x4ae710`), one per call.
    SendMail,
    /// The sell slot item's, asked by `CalculateAuctionDeposit` and `GetAuctionSellItemInfo`
    /// (`0x4ce25e`, `0x4ce5e3` with `0x4ce2f0`), one per call.
    AuctionSellItem,
    /// A paper's item the world-enter init missed (`0x4acbba` with `0x4acbf0`), one per row.
    Stationery,
    /// A spell go's item cooldown held for its record (`0x6e84c2` with `0x6e8830`), one per go.
    ItemCooldown,
    /// A cooldown query's miss, the action bar's, the bags' or the paper doll's (`0x6e2efb` with
    /// `0x6e2f70`), deduped (`0x6e2ef4`).
    CooldownQuery,
    /// The quest log's leaderboard and reward getters (`0x4e05e0`, `0x4e0bb3`, `0x4e0da3` with
    /// `0x4e0830`), one per entry as the VM keeps its waits.
    QuestLog,
    /// A tooltip build's miss (`0x52e530`, `0x52f820`), one per entry as the VM keeps each
    /// tooltip's wait.
    Tooltip,
    /// A group loot roll's or one of its lines', by the wait's serial: a start's (`0x61b40e` with
    /// `0x61b460`), an announcement's (`0x61c047` with `0x61c4b0`), a result's (`0x61b96a` with
    /// `0x61bd50`) or an all-passed's (`0x61b5c0` with `0x61b7d0`).
    LootRoll(u32),
    /// The action bar's, asked by an item slot's icon, count and spell lookups (`0x4e6b10`,
    /// `0x4e6d69`, `0x4e5b30` with `0x4e5b80`), deduped.
    ActionSlots,
    /// A refused cast's reagent or tool line whose item was not cached (`0x6e1ea4` with
    /// `ShowItemCastFailure 0x6e29b0` and the reason), one per refusal; the spell picks the
    /// line's message as its first pass would.
    CastFailure { spell: u32, reason: u8 },
}

impl Items {
    /// Queue an `SMSG_ITEM_ENCHANT_TIME_UPDATE` for an item not held. The caller has checked the
    /// active player resolves: the reference drops the update when none does.
    pub(crate) fn queue_enchant_time(&mut self, guid: u64, slot: u32, seconds: u32) {
        debug!("enchant timer: item {guid:#x} not held — queued for its arrival");
        self.pending_enchant_times.push((guid, slot, seconds));
    }

    /// A lookup of `entry` that missed registered `callback`.
    pub(crate) fn wait(&self, entry: u32, callback: RecordCallback) {
        if self.walk_takes(entry, callback, false) {
            return;
        }
        self.callbacks.lock_recover().push((entry, callback));
    }

    /// [`Self::wait`] with the dedupe flag set (`0x55bbb3`): no second `callback` on `entry`.
    pub(crate) fn wait_once(&self, entry: u32, callback: RecordCallback) {
        if self.walk_takes(entry, callback, true) {
            return;
        }
        let mut callbacks = self.callbacks.lock_recover();
        if !callbacks.contains(&(entry, callback)) {
            callbacks.push((entry, callback));
        }
    }

    /// The answer for `entry` landed, or refused it: the callbacks waiting on it, in the order
    /// they asked, spent either way.
    pub(crate) fn take_callbacks(&mut self, entry: u32) -> Vec<RecordCallback> {
        let callbacks = self.callbacks.get_mut().unwrap_or_else(|e| e.into_inner());
        let mut taken = Vec::new();
        callbacks.retain(|&(e, cb)| {
            let hit = e == entry;
            if hit {
                taken.push(cb);
            }
            !hit
        });
        taken
    }

    /// A refusal of `entry` begins walking `list` ([`RefusalWalk`]).
    pub(crate) fn begin_refusal_walk(&self, entry: u32, list: Vec<RecordCallback>) {
        let at = crate::lanes::mark();
        self.walks.lock_recover().now = Some(RefusalWalk {
            entry,
            list,
            next: 0,
            last: false,
            from: at,
            last_from: at,
            to: at,
            tail: Vec::new(),
            extended: false,
        });
    }

    /// The walk's next callback, with what its callbacks added so far.
    pub(crate) fn next_walk_step(&self) -> Option<RecordCallback> {
        let mut walks = self.walks.lock_recover();
        let walk = walks.now.as_mut()?;
        let callback = *walk.list.get(walk.next)?;
        walk.next += 1;
        walk.last = walk.next == walk.list.len();
        if walk.last {
            walk.last_from = crate::lanes::mark();
        }
        Some(callback)
    }

    /// The walk ended and the node went (`0x55bed2`). Whether a signal a callback with a next
    /// raised may still add to it, through a listener the drain runs later: then the walk goes on
    /// behind its signals ([`Self::take_walk_tail`]).
    pub(crate) fn end_refusal_walk(&self) -> bool {
        let mut walks = self.walks.lock_recover();
        let Some(mut walk) = walks.now.take() else {
            return false;
        };
        walk.to = crate::lanes::mark();
        let goes_on = walk.last_from != walk.from;
        walks.done.push(walk);
        goes_on
    }

    /// What the listeners of the walk of `entry` that began at `from` added, spent, to walk on with
    /// after its signals: the reference's listeners run inside the callbacks.
    pub(crate) fn take_walk_tail(
        &self,
        entry: u32,
        from: crate::lanes::Mark,
    ) -> Vec<RecordCallback> {
        let mut walks = self.walks.lock_recover();
        walks
            .done
            .iter_mut()
            .find(|w| w.entry == entry && w.from == from)
            .map(|w| std::mem::take(&mut w.tail))
            .unwrap_or_default()
    }

    /// The mark the walk running now began at.
    pub(crate) fn walk_began(&self) -> Option<crate::lanes::Mark> {
        self.walks.lock_recover().now.as_ref().map(|w| w.from)
    }

    /// The drain fired what the walks raised. Locked, so the drain's end leaves the records'
    /// change detection alone.
    pub(crate) fn end_refusal_walks(&self) {
        self.walks.lock_recover().done.clear();
    }

    /// Whether a lookup of `entry` now runs inside its refusal's walk: in a callback, or in a
    /// listener of a signal one raised. Its lookup finds the dying node, so it sends nothing
    /// (`0x556aa0`).
    fn in_refusal_walk(&self, entry: u32) -> bool {
        let walks = self.walks.lock_recover();
        walks.now.as_ref().is_some_and(|w| w.entry == entry)
            || walks
                .done
                .iter()
                .any(|w| w.entry == entry && crate::lanes::firing_between(w.from, w.to))
    }

    /// A lookup inside a refusal's walk of `entry` registering `callback` (`once`: with the dedupe
    /// flag): linked for the walk to reach, or dropped, as [`RefusalWalk`] says. Whether the walk
    /// took it, so it is not kept as a wait.
    fn walk_takes(&self, entry: u32, callback: RecordCallback, once: bool) -> bool {
        let mut walks = self.walks.lock_recover();
        let RefusalWalks { now, done } = &mut *walks;
        if let Some(walk) = now.as_mut().filter(|w| w.entry == entry) {
            if !walk.last && !(once && walk.list.contains(&callback)) {
                walk.list.push(callback);
            }
            return true;
        }
        let Some(walk) = done
            .iter_mut()
            .rev()
            .find(|w| w.entry == entry && crate::lanes::firing_between(w.from, w.to))
        else {
            return false;
        };
        let with_next = crate::lanes::firing_between(walk.from, walk.last_from);
        let reached = with_next || walk.extended;
        let seen = once && (walk.list.contains(&callback) || walk.tail.contains(&callback));
        if reached && !seen {
            walk.tail.push(callback);
            walk.extended |= with_next;
        }
        true
    }

    /// Drop every waiting callback of `kind`, as a cancel by callback drops its registrations
    /// (`0x55c0c0`).
    pub(crate) fn cancel_waits(&self, kind: RecordCallback) {
        self.callbacks.lock_recover().retain(|&(_, cb)| cb != kind);
    }

    /// Run `callback` for the record of `entry`, answered (`found`) or refused, with the VM, as
    /// the frame's drain comes.
    pub(crate) fn run_with_vm(&self, entry: u32, callback: RecordCallback, found: bool) {
        self.vm_callbacks
            .lock_recover()
            .push((entry, callback, found));
    }

    /// The callbacks [`Self::run_with_vm`] queued, in order, spent.
    pub(crate) fn take_vm_callbacks(&self) -> Vec<(u32, RecordCallback, bool)> {
        std::mem::take(&mut *self.vm_callbacks.lock_recover())
    }

    /// An entry change asked for the record of `entry` with the callback `0x5d96b0`.
    pub(crate) fn wait_for_entry(&mut self, guid: u64, entry: u32) {
        self.wait(entry, RecordCallback::Entry(guid));
    }

    /// A trade getter missed the cache on `entry` and asked with the callback `0x4c0340`.
    pub(crate) fn wait_for_trade_record(&self, entry: u32) {
        self.wait(entry, RecordCallback::Trade);
    }

    /// An ammo-slot getter missed the cache on `entry` and asked with the callback `0x4c84f0`.
    pub(crate) fn wait_for_ammo_record(&self, entry: u32) {
        self.wait(entry, RecordCallback::Ammo);
    }

    /// The trade getter calls waiting on `entry`, spent.
    #[cfg(test)]
    pub(crate) fn take_trade_waiters(&mut self, entry: u32) -> usize {
        self.take_waiters(entry, RecordCallback::Trade)
    }

    /// The ammo-slot getter calls waiting on `entry`, spent.
    #[cfg(test)]
    pub(crate) fn take_ammo_waiters(&mut self, entry: u32) -> usize {
        self.take_waiters(entry, RecordCallback::Ammo)
    }

    /// The answer for `entry` landed: how many callbacks of `kind` waited on it, which it spends.
    #[cfg(test)]
    pub(crate) fn take_waiters(&mut self, entry: u32, kind: RecordCallback) -> usize {
        let callbacks = self.callbacks.get_mut().unwrap_or_else(|e| e.into_inner());
        let before = callbacks.len();
        callbacks.retain(|&(e, cb)| !(e == entry && cb == kind));
        before - callbacks.len()
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

    /// The template for `entry` if known, read as `ask` says; an ask goes once while in flight
    /// (`guid` rides along, `0` for template-only). A refused entry is also `None`.
    pub(crate) fn template(&self, entry: u32, guid: u64, ask: Ask<'_>) -> Option<&ItemInfo> {
        if !matches!(ask, Ask::Read) && self.in_refusal_walk(entry) {
            return None;
        }
        self.templates.read(entry, ask, |commands| {
            Self::send_template(entry, guid, commands)
        })
    }

    fn send_template(entry: u32, guid: u64, commands: &NetCommands) {
        debug!("items: asking template (entry {entry})");
        let _ = commands.0.send(ClientCommand::ItemQuery { entry, guid });
    }

    /// Whether the cache holds a record, a pending ask or a refusal for `entry`
    /// ([`QueryCache::knows`]).
    pub(crate) fn template_known(&self, entry: u32) -> bool {
        self.templates.knows(entry)
    }

    /// Whether the cache answered `entry`, with a record or a refusal: the point at which a lookup's
    /// callback has run, status 1 or 0.
    pub(crate) fn template_answered(&self, entry: u32) -> bool {
        self.templates.answered(entry)
    }

    /// Whether the server refused `entry` and nothing has asked since, not still pending: the
    /// status-0 answer its waiters were told.
    pub(crate) fn template_answered_unknown(&self, entry: u32) -> bool {
        self.templates.answered_unknown(entry)
    }

    /// The [`HeldTemplate`] view of [`Self::template`], asked template-only.
    pub(crate) fn held(&self, entry: u32, ask: Ask<'_>) -> Option<HeldTemplate> {
        self.template(entry, 0, ask).map(HeldTemplate::of)
    }

    /// Record a template answer (`SMSG_ITEM_QUERY_SINGLE_RESPONSE`); `None` = a refusal.
    pub(crate) fn insert_template(&mut self, entry: u32, info: Option<ItemInfo>) {
        match info {
            Some(info) => {
                self.note_change(entry, true);
                self.templates.insert(entry, info);
            }
            // A refusal moves the generation too: "still asking" to "unknown" is a display
            // transition.
            None => {
                if self.templates.refuse(entry) == crate::query_cache::Refused::Record {
                    self.note_change(entry, false);
                }
            }
        }
    }

    /// `entry`'s record landed (`held`) or went.
    fn note_change(&mut self, entry: u32, held: bool) {
        self.change_count += 1;
        self.changes.insert(entry, (self.change_count, held, false));
    }

    /// The VM's copy took `entry`'s last change at its answer, so the feed need not push it again.
    pub(crate) fn vm_took(&mut self, entry: u32) {
        if let Some(change) = self.changes.get_mut(&entry) {
            change.2 = true;
        }
    }

    /// How many record changes have been made: a reader's cursor for [`Self::changed_since`].
    pub(crate) fn change_count(&self) -> u64 {
        self.change_count
    }

    /// Each entry whose record landed or went after change `count` and that the VM's copy did not
    /// take at its answer, with whether it is held now.
    pub(crate) fn changed_since(&self, count: u64) -> impl Iterator<Item = (u32, bool)> + '_ {
        self.changes
            .iter()
            .filter(move |(_, &(at, _, took))| at > count && !took)
            .map(|(&entry, &(_, held, _))| (entry, held))
    }

    /// The template store, the one [`crate::wdb`] saves.
    pub(crate) fn template_store(&self) -> &QueryCache<u32, ItemInfo> {
        &self.templates
    }

    pub(crate) fn template_store_mut(&mut self) -> &mut QueryCache<u32, ItemInfo> {
        &mut self.templates
    }

    /// Drops every template, which the interface's copy drops too: another realm's file replaces
    /// them.
    pub(crate) fn forget_templates(&mut self) {
        let held: Vec<u32> = self.templates.iter().map(|(&entry, _)| entry).collect();
        for entry in held {
            self.note_change(entry, false);
        }
        self.templates.clear();
    }

    /// Bumped by every landed answer, for consumers besides the drain.
    pub(crate) fn template_epoch(&self) -> u64 {
        self.templates.generation()
    }

    /// Every entry with a cached template.
    pub(crate) fn cached_template_ids(&self) -> Vec<u32> {
        self.templates.iter().map(|(&id, _)| id).collect()
    }

    /// Disconnect: drop the callbacks waiting on asks or on the VM, and the queued enchant times,
    /// keep the templates; [`crate::wdb`] clears their in-flight asks. The item objects and their
    /// countdowns go with the index's sweep.
    pub(crate) fn clear_session(&mut self) {
        self.pending_enchant_times.clear();
        self.callbacks
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.vm_callbacks
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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

/// Any streamed object, straight into a test world: its guid indexed and its descriptor stored.
#[cfg(test)]
pub(crate) fn test_spawn_object(world: &mut World, guid: u64, fields: ObjectFields) -> Entity {
    let e = world.spawn((Guid(guid), ObjectStore(fields))).id();
    world.resource_mut::<GuidIndex>().0.insert(guid, e);
    e
}

/// `EQUIPMENT_SLOT_MAINHAND` / `_OFFHAND` / `_RANGED` (vmangos `EquipmentSlots`).
pub(crate) const EQUIPMENT_SLOT_MAINHAND: u8 = 15;
pub(crate) const EQUIPMENT_SLOT_OFFHAND: u8 = 16;
pub(crate) const EQUIPMENT_SLOT_RANGED: u8 = 17;

/// Whether the disarm flag is up (`UNIT_FIELD_FLAGS & UNIT_FLAG_DISARMED`), which hides one hand
/// from the equipped-item search.
pub(crate) fn is_disarmed(store: &ObjectStore) -> bool {
    store.0.unit_flags() & crate::creature_anim::UNIT_FLAG_DISARMED != 0
}

/// The equipment slot this character's disarm hides ([`crate::creature_anim::disarmed_hand`] over
/// the raw inventory), for the action bar's equipped-item requirement (`0x5f0c50`) and the
/// item-use refusal (`CGItem::Use` rung 15). `None` while the flag is down or neither hand holds
/// a weapon. The templates are only read, as the reference's item getters read them (`0x5d9f6d`).
pub(crate) fn disarmed_equipment_slot(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
) -> Option<u8> {
    if !is_disarmed(store) {
        return None;
    }
    let class_of = |slot: u8| -> Option<u8> {
        let guid = store.0.player_inv_slot(slot).filter(|&g| g != 0)?;
        let entry = objects.object(guid).and_then(|o| o.object_entry())?;
        Some(items.template(entry, 0, Ask::Read)?.class as u8)
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

        assert!(items.template(117, 0x42, Ask::New(&cmds)).is_none());
        // Second copy of the same item: no second query.
        assert!(items.template(117, 0x43, Ask::New(&cmds)).is_none());
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
            items
                .template(117, 0x43, Ask::New(&cmds))
                .map(|i| i.name.as_str()),
            Some("Tough Jerky")
        );
    }

    #[test]
    fn the_template_epoch_counts_landings_and_asks_count_nothing() {
        let (cmds, _rx) = commands();
        let mut items = Items::default();
        let t0 = items.template_epoch();

        assert!(items.template(117, 0x42, Ask::New(&cmds)).is_none());
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

    /// A reader follows the records by its cursor, never draining them: a landed record shows
    /// once past its cursor, a refusal of an unheld entry not at all, and a refusal of a held one
    /// as gone.
    #[test]
    fn a_record_change_shows_past_a_readers_cursor() {
        let mut items = Items::default();
        items.insert_template(117, Some(info("Tough Jerky")));
        items.insert_template(9999, None);
        let since = |items: &Items, at| {
            let mut v: Vec<(u32, bool)> = items.changed_since(at).collect();
            v.sort_unstable();
            v
        };
        assert_eq!(since(&items, 0), [(117, true)]);
        let cursor = items.change_count();
        assert!(since(&items, cursor).is_empty(), "nothing past the cursor");
        items.insert_template(117, None);
        assert_eq!(since(&items, cursor), [(117, false)]);
        assert_eq!(
            since(&items, 0),
            [(117, false)],
            "a second reader reads it too"
        );
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

    /// `0x55bdf7`: an item key with its top bit set is a refusal. The waiters read unknown
    /// (`template_answered_unknown`), nothing is kept, the reads between events never ask it
    /// however often they run, and the next event's lookup does.
    #[test]
    fn a_refused_template_reads_unknown_and_only_the_next_event_asks_again() {
        let (cmds, rx) = commands();
        let mut items = Items::default();

        assert!(items.template(9999, 0, Ask::New(&cmds)).is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::ItemQuery { entry: 9999, .. })
        ));
        assert!(!items.template_answered_unknown(9999), "still pending");
        let before = items.template_epoch();
        items.insert_template(9999, None); // server: unknown entry
        assert_ne!(
            items.template_epoch(),
            before,
            "the redisplay counter moves"
        );
        assert!(items.template_answered_unknown(9999));
        assert!(
            items.template(9999, 0, Ask::Read).is_none(),
            "and nothing is kept"
        );
        assert!(items.cached_template_ids().is_empty());

        for _ in 0..600 {
            assert!(items.template(9999, 0, Ask::New(&cmds)).is_none());
        }
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "a read between events does not send"
        );

        assert!(items.template(9999, 0, Ask::Event(&cmds)).is_none());
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::ItemQuery { entry: 9999, .. })
            ),
            "the event's lookup does"
        );
        assert!(!items.template_answered_unknown(9999), "pending again");
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
        assert!(items.template(118, 0, Ask::New(&cmds)).is_none()); // leaves 118 in flight
        items.clear_session();
        items.template_store_mut().clear_pending(); // the saved stores' clear, in `wdb`
        assert!(items.take_enchant_times(0x43).is_empty());
        assert!(
            items.template(117, 0, Ask::New(&cmds)).is_some(),
            "templates survive"
        );
        // 118's ask was dropped with the writer, so it re-asks now.
        let _ = rx.try_recv();
        assert!(items.template(118, 0, Ask::New(&cmds)).is_none());
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

    /// The gift arm (`0x52e1b9`) names the gift creator of a wrapped item and no other: an
    /// unwrapped item, a zero guid and a name not yet answered add no name.
    #[test]
    fn only_a_wrapped_item_names_its_gift_creator() {
        let mut names = crate::names::NameCache::default();
        let giver =
            |flags: u32, guid: u32| ObjectFields::from_pairs(&[(21, flags), (12, guid), (13, 0)]);
        names.insert_player(5, "Santa".to_string(), None);
        assert_eq!(gift_creator_name(&giver(0, 5), &names), None, "unwrapped");
        assert_eq!(gift_creator_name(&giver(0x8, 0), &names), None, "no giver");
        assert_eq!(
            gift_creator_name(&giver(0x8, 6), &names),
            None,
            "not answered"
        );
        assert_eq!(
            gift_creator_name(&giver(0x8, 5), &names).as_deref(),
            Some("Santa")
        );
    }

    /// `0x5da2c0`: the Firestone shape binds and prints no line, so a predicate read off
    /// [`enchant_lines`] would call it unbound.
    #[test]
    fn the_bind_predicate_reads_the_descriptor_not_the_rendered_lines() {
        let cat = catalog();
        let cat = Some(&cat);
        assert!(
            already_bound(&item(0x1, 0), 0, cat),
            "ITEM_FIELD_FLAGS & 1 — already soulbound"
        );
        assert!(
            !already_bound(&item(0x0, 0), 0, cat),
            "no flag, no enchant — the plain BoE"
        );
        assert!(
            !already_bound(&item(0x8, 0), 0, cat),
            "a WRAPPED gift is not a bound item — the bit is 0x1, not any bit"
        );
        assert!(
            already_bound(&item(0x0, 11), 0, cat),
            "a live enchant slot naming a binding row"
        );
        assert!(
            !already_bound(&item(0x0, 13), 0, cat),
            "a non-binding enchant leaves the item unbound"
        );
        assert!(
            !already_bound(&item(0x0, 99), 0, cat),
            "an id that names no row binds nothing (the reference's `testl` after the table load)"
        );
        assert!(
            !already_bound(&item(0x0, 11), 0, None),
            "no catalog loaded — the enchant half cannot answer, and does not guess"
        );
        let charter = benilla_protocol::messages::ITEM_FLAG_CHARTER;
        assert!(
            !already_bound(&item(0x0, 11), charter, cat),
            "a record flagged 0x2000 skips the enchant leg (0x5da2e3)"
        );
        assert!(
            already_bound(&item(0x1, 11), charter, cat),
            "but not the instance's own bound bit"
        );
        // The case that pins the design: bound, and no line.
        assert!(
            already_bound(&item(0x0, 12), 0, cat),
            "the Firestone shape — binds AND hides its line"
        );
        assert!(
            enchant_lines_quiet([(0u8, 12i32, 0u32, None)], cat).is_empty(),
            "…and the rendered lines are empty for it, which is why they are not the source"
        );
    }
}
