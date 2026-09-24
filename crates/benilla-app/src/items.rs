//! The item layer — decision 0068's T2 (containers) groundwork.
//!
//! The wire splits item knowledge in two:
//!
//! - **Objects** — the item/container *instances* the server streamed at us (`ItemCreate`: our own
//!   inventory at login, loot, trades; they are private, so only ours ever arrive). **An item is
//!   an object** (decision 2334): it is an entity in the one guid index with the same
//!   [`ObjectStore`] every unit has — `Guid` + `ObjectStore` + [`ItemObject`] — created, merged
//!   and destroyed by the object layer's handlers like any other kind, its field edges on the
//!   same watch (`FieldChanged`, kind `Item`/`Container`), gone with the session's sweep. Which
//!   *slot* holds a guid lives one level up, in the player descriptor's `INV_SLOT`/`PACK_SLOT`
//!   arrays and a bag's `CONTAINER_FIELD_SLOT` array; [`crate::net::Objects`] resolves those guids
//!   to the item's fields. Its countdowns (temporary enchants, its own lifetime) are its own
//!   [`Countdowns`] component — the reference's per-object deadline cells (decision 2340).
//!
//! - **Templates** — the static item *definitions* (`SMSG_ITEM_QUERY_SINGLE_RESPONSE`: name,
//!   quality, class, display id), keyed by entry and shared by every copy. The exact twin of
//!   [`crate::names::NameCache`], with the same **ask-once** discipline: [`Items::template`]
//!   returns the answer when known, otherwise sends the query (deduped while in flight) and
//!   reports "not yet". Negative answers are cached — a bad entry never becomes a query loop.
//!   Templates survive disconnect: item definitions are stable across sessions.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::{ItemInfo, ObjectFields};

use bevy::ecs::system::SystemParam;

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, Objects};
use crate::query_cache::QueryCache;

/// **An item or container entity's kind** — the reference's `TYPEMASK_ITEM` / `TYPEMASK_CONTAINER`
/// on the one object index (decision 2334). An item is `Guid` + [`ObjectStore`] + this; it has no
/// `NetEntity` and no `Transform` because it has no model and no pose. Every system that iterates
/// stores as *units* filters this out (`Without<ItemObject>`): an item block's dwords overlap the
/// unit block's indices, so an unfiltered unit read of an item's store answers with item fields.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ItemObject {
    /// `TYPEMASK_CONTAINER` — the create said so; a bag's slot array lives past the item block.
    pub(crate) container: bool,
}

/// Spawn an item object into the index — the item half of the object layer's create, and what a
/// fixture seeds with. The seed is never merged (the create-time notify-suppress, 2297); a
/// re-create of a live guid takes the values path in the handler, not this.
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

/// The item's enchantment slots — `ITEM_FIELD_ENCHANTMENT`'s 21 dwords, three per slot, and the
/// reference's seven enchant deadline cells `[obj + 0x324 + slot*4]` (`0x5d9d00`), which end where
/// the next member begins at `+0x340`.
pub(crate) const ENCHANT_SLOTS: usize = 7;

/// **An item's countdowns** — the reference's per-object deadline cells on `CGItem_C` (decision
/// 2340): its own lifetime at `+0x320` (fed only by `SMSG_ITEM_TIME_UPDATE`, decision 1933) and
/// one temporary-enchant deadline per enchant slot at `+0x324` (fed only by
/// `SMSG_ITEM_ENCHANT_TIME_UPDATE`, decision 0920; the item's `ITEM_FIELD_ENCHANTMENT` duration
/// field is never read for it). Every item object carries one from its spawn, so the cells die
/// with the object as the reference's do, and a write through `Mut` is the landing the inventory
/// feeds' [`ItemChanges`] sees.
///
/// Absolute deadlines, recomputed on read and never ticked — `0x5d9c60` / `0x5d9d00` subtract
/// `now` from the cell on every call.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Countdowns {
    lifetime: Option<Instant>,
    enchants: [Option<Instant>; ENCHANT_SLOTS],
}

/// The two setters' shared store rule (`0x5d9c00`, `0x5d9cc0`): a **signed** `<= 0` clears the
/// cell — `jle` on the wire value, so `0` and anything with the top bit set both store absence
/// rather than a 68-year deadline — and anything else parks `now + seconds`, the wire's seconds
/// times the one `imul 0x3e8`.
fn deadline(seconds: u32) -> Option<Instant> {
    ((seconds as i32) > 0).then(|| Instant::now() + Duration::from_secs(u64::from(seconds)))
}

/// A cell's time left: `None` when unset, `Some(0)` once elapsed (`0x5d9d00`'s `max(0, …)`).
fn left(cell: Option<Instant>) -> Option<Duration> {
    cell.map(|at| at.saturating_duration_since(Instant::now()))
}

/// The tooltip's read: an elapsed cell is no timer at all (the `!= 0` gate prints the plain
/// line).
fn remaining_ms(cell: Option<Instant>) -> Option<u64> {
    left(cell)
        .filter(|l| !l.is_zero())
        .map(|l| l.as_millis() as u64)
}

impl Countdowns {
    /// `SMSG_ITEM_ENCHANT_TIME_UPDATE`'s setter `0x5d9cc0`. The reference indexes the cell array
    /// with the wire's slot unchecked — a slot past the seventh overruns into the next member;
    /// this refuses it instead (`false`), as the spell-modifier tables refuse theirs.
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

    /// Milliseconds left on the temporary enchant in `slot`, or `None` when that slot carries no
    /// timer — including an expired one (`0x5d9d00` returns 0 past the deadline, and the tooltip's
    /// `!= 0` gate then prints the plain name).
    pub(crate) fn enchant_remaining_ms(&self, slot: u32) -> Option<u64> {
        remaining_ms(self.enchant(slot))
    }

    /// [`Self::enchant_remaining_ms`] at **display granularity**: floored to the whole second.
    ///
    /// The snapshot feeds read this one. The tooltip's bucket ladder is ceil at day/hour/min and
    /// truncate at seconds, so every value inside one second renders the same line — but a raw
    /// per-ms read makes the *snapshot* differ every frame, which held the 1439 gates open and
    /// fired `UNIT_INVENTORY_CHANGED` at frame rate for as long as a poison ticked (director
    /// report, 2026-08-19: the char window's cost, and its refusal to settle after closing).
    /// Floored, the snapshot moves once a second — exactly as often as its rendering can.
    /// The live per-ms reader stays for `GetWeaponEnchantInfo` ([`Self::enchant_deadline_ms`]),
    /// whose per-frame push is the reference's own recompute-per-call (`0x5d9d00`).
    pub(crate) fn enchant_remaining_display_ms(&self, slot: u32) -> Option<u64> {
        self.enchant_remaining_ms(slot).map(|ms| ms - ms % 1000)
    }

    /// The same deadline read **without** the tooltip's expired-is-absent collapse: `Some(0)` for a
    /// timer that has run out, `None` only when the slot never had one.
    ///
    /// `GetWeaponEnchantInfo` needs the two apart where the tooltip does not. Its expiration return
    /// is `max(0, deadline − now)` from the client-local deadline array (`0x5d9d00`, subtract on
    /// read), so an enchant whose timer has elapsed answers the NUMBER 0, and
    /// `BuffFrame_Enchant_OnUpdate` then draws "0 s" and pulses the icon. Collapsing that to nil
    /// would silently hide a expiring enchant's last state.
    pub(crate) fn enchant_deadline_ms(&self, slot: u32) -> Option<u64> {
        left(self.enchant(slot)).map(|l| l.as_millis() as u64)
    }

    /// Milliseconds left on the item's own lifetime, or `None` when it carries no timer —
    /// including an elapsed one.
    pub(crate) fn lifetime_remaining_ms(&self) -> Option<u64> {
        remaining_ms(self.lifetime)
    }

    /// [`Self::lifetime_remaining_ms`] at **display granularity** — floored to the whole second,
    /// for the same reason [`Self::enchant_remaining_display_ms`] is.
    pub(crate) fn lifetime_remaining_display_ms(&self) -> Option<u64> {
        self.lifetime_remaining_ms().map(|ms| ms - ms % 1000)
    }

    /// This item's share of [`ItemChanges::countdown_steps`]: each live cell contributes
    /// `floor(seconds left) + 1`, an unset or elapsed one 0 — so the final `Some(0) → None`
    /// collapse (the tooltip reverting to the plain enchant name, the lifetime line vanishing) is
    /// its own step, one frame after the deadline elapses.
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

/// **Did any item object move this frame** — the gate input the inventory feeds watch in place of
/// the item map's old epoch (decision 2334): a create, a values delta, a countdown landing or a
/// destroy on any item entity. `Changed` covers the first three (a spawn is a change, and so is a
/// write to either component), the removal reader the fourth — a bag's slot going empty is the
/// player's own field, but the *item* vanishing is only this.
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

    /// The value a gated feed watches instead of holding its gate open per-frame while a
    /// countdown runs: it moves exactly when some **displayable** countdown can — the two
    /// second-floored reads a bag/equipment snapshot renders. The sum of every item's
    /// [`Countdowns`] steps; between landings (which [`Self::moved`] reports) each term only
    /// falls, so a displayable change can never be masked by another. No live cell — the
    /// overwhelmingly common case — sums to 0.
    pub(crate) fn countdown_steps(&self) -> u64 {
        let now = Instant::now();
        self.countdowns.iter().map(|c| c.steps(now)).sum()
    }
}

/// **The player's inventory, as one read** — what a bag, paper-doll or spellbook feed resolves
/// its slots from (decision 2334): the self descriptor's slot arrays and their change tick, the
/// object lookup those guids resolve through, and the item entities' own change watch.
#[derive(SystemParam)]
pub(crate) struct Inventory<'w, 's> {
    pub(crate) self_store: Query<'w, 's, &'static ObjectStore, With<crate::net::SelfPlayer>>,
    pub(crate) self_changed:
        Query<'w, 's, (), (With<crate::net::SelfPlayer>, Changed<ObjectStore>)>,
    pub(crate) objects: Objects<'w, 's>,
    pub(crate) changes: ItemChanges<'w, 's>,
}

/// The slice of an item template that equipment rendering + combat animation consume (decisions
/// 0072/0073): the ItemDisplayInfo key, the two placement inputs, and the weapon class pair the
/// swing/ready selectors key on. A Copy **view** of the cached [`ItemInfo`] ([`Items::held`]).
#[derive(Clone, Copy)]
pub(crate) struct HeldTemplate {
    pub(crate) display_info_id: u32,
    pub(crate) inventory_type: u32,
    pub(crate) sheath: u32,
    pub(crate) class: u32,
    pub(crate) subclass: u32,
    /// `Material` — the item's `Material.dbc` id (1 metal · 2 wood · 5 chain · 6 plate · 7 cloth ·
    /// 8 leather · 0 undefined). On the wire in `SMSG_ITEM_QUERY_SINGLE_RESPONSE`, and the **only**
    /// input to the draw/stow sound pick: `SheatheSoundLookups` carries one row per weapon subclass
    /// per material, and every row of a material agrees — the subclass is inert (decision 0882).
    pub(crate) material: u32,
}

/// `SpellItemEnchantment.dbc`'s two consumer columns, loaded once and read by both lanes that
/// need them: the **visual** by the weapon-glow chain (decision 0805,
/// [`crate::entities::item_glow`]) and the **name** by the item tooltip's enchant line (decision
/// 0915, [`crate::ui_items`]). It lives here rather than inside either consumer because it is
/// item *data*, and because a second loader over one DBC is how a schema quietly drifts.
///
/// Optional, like every DBC-backed resource: absent, weapons draw unadorned and no tooltip prints
/// an enchant line — each lane's own pre-existing behaviour.
#[derive(Resource)]
pub(crate) struct Enchants(pub(crate) benilla_formats::EnchantCatalog);

/// One enchant SLOT's contribution, as the app resolved it — `(slot index, id, charges,
/// remaining ms)`. The id is **signed**, because its sign is load-bearing downstream: it picks the
/// line's colour and nothing else (`0x52c9f9` — `abs(id)` names the DBC row either way).
pub(crate) type EnchantSlot = (u8, i32, u32, Option<u64>);

/// The tooltip lines an item instance's enchant slots contribute — the one place the app turns
/// enchant *ids* into text (decisions 0915/0920). Every tooltip surface feeds through here, so a
/// bag hover, a paper-doll hover and an inspect hover can never disagree.
///
/// The per-slot gate is the reference's (`0x52c9f9`–`0x52ca23`): `id != 0`, then `abs(id)` must
/// name a real `SpellItemEnchantment` row — **the sign never changes which row**, only the colour
/// the engine paints. An id that names no row contributes nothing rather than a placeholder.
///
/// `slots` is the caller's source, and it differs by surface for a reason the wire fixes: our own
/// items stream as OBJECTS, so all 7 `ITEM_FIELD_ENCHANTMENT` slots (plus charges, plus the
/// `SMSG_ITEM_ENCHANT_TIME_UPDATE` timer) are readable; anybody else's are visible only through
/// the slots their descriptor broadcasts, which in 1.12 vmangos fills for PERM and TEMP only.
pub(crate) fn enchant_lines(
    slots: impl IntoIterator<Item = EnchantSlot>,
    enchants: Option<&Enchants>,
) -> Vec<benilla_ui::script::EnchantView> {
    let lines = enchant_lines_quiet(slots, enchants);
    // One breadcrumb per session the first time any surface resolves an enchant — the
    // machine-readable "this lane is live" signal for a feature whose whole symptom is ABSENCE
    // (`entities::item_glow`'s idiom, and the reason this one exists: the enchant ids reaching the
    // tooltip travel a DIFFERENT wire field from the ones the weapon glow reads, so a silent miss
    // here would look exactly like an unenchanted item).
    if !lines.is_empty() {
        static FIRST: std::sync::Once = std::sync::Once::new();
        // The whole view, not just the name: the countdown and the charges each ride a different
        // wire lane from the id, so "which of the three arrived" is exactly what this must answer.
        FIRST.call_once(|| info!("item enchant: {lines:?} (the first resolved this session)"));
    }
    lines
}

/// `0x5da2c0` — **"has this item already been through the bind question?"**: the instance's
/// `ITEM_FIELD_FLAGS & 1` (already soulbound), **or** any of its seven live
/// `ITEM_FIELD_ENCHANTMENT` slots naming a `SpellItemEnchantment` row that binds the item
/// ([`benilla_formats::EnchantCatalog::binds_the_item`], the ref's `5da300`–`5da320` walk).
///
/// One predicate, two consumers — the enchant cursor's bind question
/// ([`crate::ui_action`]'s `ClickedItem::already_bound`, the `0x495d60` gate) and the item
/// tooltip's **Soulbound** override (B310). They must agree: an item the cursor considers
/// already bound is exactly an item whose tooltip says *Soulbound*.
///
/// Read off the RAW descriptor, never off the rendered [`enchant_lines`] list. That list is a
/// *display* view: it drops rows the catalog cannot name, and it drops every
/// `Flags & 0x2` row outright (the line the reference refuses to print — decision 0928). The two
/// flag sets **overlap**, so this is not a hypothetical: **Firestone 1-4 and Orb of Fire carry
/// both bits** — they bind the item AND print no line — so an imbued weapon would read back as
/// "not bound" from the lines while the reference calls it bound.
pub(crate) fn already_bound(fields: &ObjectFields, cat: Option<&Enchants>) -> bool {
    fields.item_flags().is_some_and(|f| f & 0x1 != 0)
        || (0..7).any(|slot| live_enchant(fields, slot, cat).is_some_and(|id| binds(id, cat)))
}

/// One `ITEM_FIELD_ENCHANTMENT` slot as the bind checks read it (`495eec:
/// movl 0x40(%ecx,%eax,4)` with `eax = 3*slot`): the raw id must be **positive** (the ref's `jl`
/// skip at `495ef4`/`5da306`) and must name a real `SpellItemEnchantment` row (its
/// `testl %eax,%eax` after the table load). Anything else is "no enchant here".
///
/// NB this is the *bind-question* reading, not the *line* reading — the line law names its row
/// off `abs(id)` and keeps the sign only for the colour ([`enchant_lines`], `0x52c9f9`).
pub(crate) fn live_enchant(fields: &ObjectFields, slot: u8, cat: Option<&Enchants>) -> Option<u32> {
    let id = u32::try_from(fields.item_enchant(slot)?).ok()?;
    cat.is_some_and(|c| c.0.has_row(id)).then_some(id)
}

/// `SpellItemEnchantment.Flags & 1` — this enchant soulbinds the item it lands on.
pub(crate) fn binds(id: u32, cat: Option<&Enchants>) -> bool {
    cat.is_some_and(|c| c.0.binds_the_item(id))
}

/// [`enchant_lines`] without the breadcrumb — the same gate and the same naming, for the one
/// caller that is not a live item: the startup resolve of the whole `ItemRandomProperties` table
/// ([`random_property_views`]). Routing that through the loud one would fire "the first enchant
/// resolved this session" at load, every session, which is exactly the signal the breadcrumb
/// exists to distinguish from silence.
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
        // `SpellItemEnchantment.Flags & 0x2` — the row prints NO line at all. Both of the
        // reference's enchant-line printers open with it and return before they ever read the
        // name (`6290e4` / `62923e`, each `testb $0x2, 0x5c(...)` → `jne <retl>`). Twelve shipped
        // rows, one family: the totem-granted weapon imbues, Firestone, Orb of Fire — buffs whose
        // source already shows elsewhere on screen, so the weapon does not repeat them. Found and
        // closed while transcribing the *other* bit of that column (decision 0928); 0915 read the
        // name column alone and printed all twelve.
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

/// `ItemRandomProperties.dbc` — the **random-suffix roll**: the "of the Monkey" a drop rolled, and
/// the enchants that roll grants (decision 1547). One table, two consumers, exactly as in the
/// reference: the display NAME ([`item_display_name`], its `0x5d8b00`) and the tooltip's enchant
/// slots 2..6 ([`random_property_lines`], its `0x52b7e0` suffix-row copy).
///
/// Optional like every DBC-backed resource: absent, names stay unsuffixed and a rolled item shows
/// no suffix lines — the behaviour benilla had before this arc.
#[derive(Resource)]
pub(crate) struct RandomProperties(pub(crate) benilla_formats::RandomPropertyCatalog);

/// The item's display NAME — the reference's one name formatter `0x5d8b00(entry, randomPropertyId)`,
/// whose whole law is its two exits: `ITEM_SUFFIX_TEMPLATE` (`"%s %s"`) joined with the roll's
/// suffix, or the plain template name when the id is 0, negative, past the table, or names a row
/// with no suffix string.
///
/// **Every** display of an item's name goes through here, because in the reference every one of
/// them goes through that function: the tooltip plate, the `|Hitem:…|h[Name]|h` link (the link is
/// built FROM this string), the loot row, the chat "You receive loot" line, the auction and mail
/// rows. A surface that composed the name itself would print "Chipped Claw" where the client prints
/// "Chipped Claw of the Bear" — which is exactly the drift decision 0888 recorded and this closes.
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

/// The tooltip lines a random-property **roll** contributes, for a source that has no item object
/// to read `ITEM_FIELD_ENCHANTMENT` from — a loot slot, a chat link, an auction or mail row.
///
/// This is the reference's mechanism (`0x52b7bf`–`0x52b7fb`), one for one: the tooltip resolves
/// its `+0x424` randomPropertyId against `ItemRandomProperties.dbc` and copies the row's five
/// enchant ids into session slots **2..6**, which the enchant family then prints exactly like an
/// object's own slots (white, since only slots 0/1 ever colour). So the same [`enchant_lines`] gate
/// runs over them — one law for both id sources, which is the point of routing them through it.
///
/// An item OBJECT needs none of this: the server writes the rolled ids into its own enchant slots,
/// and the object path already reads them.
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

/// The whole roll table, resolved for the engine — every `ItemRandomProperties` row as the
/// suffix plus its named enchant lines, ready to push once ([`benilla_ui::script::UiScript::
/// set_random_properties`]).
///
/// Pushed whole rather than asked for per id, because it is a static table the app already holds
/// and its consumers are click-driven: a chat-link tooltip has no hover re-enter loop, so a late
/// answer would leave the first click showing an item with no lines. The reference reads its own
/// loaded DBC store the same way, at draw time, from the id the source supplied.
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

/// The two catalogs a random-suffix **roll** resolves through, as one value: the roll's own table
/// and the enchant names its five ids land on. They are never useful apart — a roll is a name plus
/// a set of enchant ids, and both halves come from a DBC the app owns — and every surface that
/// shows a rolled item needs the pair, so it travels as a pair rather than as two `Option` params
/// threaded through every loot/link/auction/mail feed.
#[derive(Clone, Copy)]
pub(crate) struct RollCatalogs<'a> {
    pub(crate) props: Option<&'a RandomProperties>,
    pub(crate) enchants: Option<&'a Enchants>,
}

impl RollCatalogs<'_> {
    /// No DBC catalogs — what the unit tests run with (no install), and what a shipping session
    /// falls back to when the tables are missing: the plain name, and no suffix lines.
    #[cfg(test)]
    pub(crate) const NONE: RollCatalogs<'static> = RollCatalogs {
        props: None,
        enchants: None,
    };

    /// [`item_display_name`] — the item's name with this roll's suffix joined on.
    pub(crate) fn name(&self, base: &str, random_property_id: u32) -> String {
        item_display_name(base, random_property_id as i32, self.props)
    }

    /// [`random_property_lines`] for one id — the roll's enchant slots 2..6, resolved and named.
    /// The live surfaces do NOT call this (the engine resolves the lines itself, off the pushed
    /// table); it is the app-side check that the id → row → slot mapping is what the law says.
    #[cfg(test)]
    pub(crate) fn lines(&self, random_property_id: u32) -> Vec<benilla_ui::script::EnchantView> {
        match self.props.and_then(|p| p.0.get(random_property_id as i32)) {
            Some(row) => random_property_lines(row, self.enchants),
            None => Vec::new(),
        }
    }
}

/// The item stores: instances by guid, templates by entry (+ the in-flight ask-once set).
/// Filled by the net bridge; read by the container APIs (`GetContainerItemInfo` and kin).
#[derive(Resource, Default)]
pub(crate) struct Items {
    /// The template cache — ask-once through [`QueryCache`] (decision 2288); a `None` answer is
    /// the server's "unknown entry" (the top-bit miss branch), cached so it is never re-asked.
    templates: QueryCache<u32, ItemInfo>,
    /// Entries whose template landed since the last [`Self::take_fresh`] drain — the push half of
    /// the tooltip store (every landed template goes to the UI unprompted, so the first hover of
    /// an item whose name is already on screen never misses).
    fresh: Vec<u32>,
    /// **`PlayerPendingItemExpiration`** — the temporary-enchant updates that named an item not
    /// yet held, kept for the item's arrival (decision 2340). The reference's `0x1EB` arm, on an
    /// item-lookup miss, links a `{item guid, slot, seconds}` record onto the active player's list
    /// (`0x5ebd40`, the list at `CGPlayer_C + 0x1cc8`); `0x5ebde0` walks it when an item of ours is
    /// set up (`0x5d8440`), applies each match through the setter — `seconds` counted from then,
    /// not from the packet — and unlinks it. Records for an item that never arrives live as long
    /// as the player object: here, the session. The item-lifetime arm `0x1EA` has no such list.
    pending_enchant_times: Vec<(u64, u32, u32)>,
}

impl Items {
    /// `SMSG_ITEM_ENCHANT_TIME_UPDATE` named an item we do not hold: keep it for the item's
    /// arrival ([`Self::pending_enchant_times`]'s `0x5ebd40`). The caller has checked the active
    /// player resolves — the reference queues onto a player object, and drops the update when
    /// none resolves.
    pub(crate) fn queue_enchant_time(&mut self, guid: u64, slot: u32, seconds: u32) {
        debug!("enchant timer: item {guid:#x} not held — queued for its arrival");
        self.pending_enchant_times.push((guid, slot, seconds));
    }

    /// The item `guid` arrived: its queued enchant updates, in arrival order, unlinked — what
    /// `0x5ebde0` replays through the setter.
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

    /// The template for `entry`, if known. On a miss, asks the server (once per entry per
    /// connection; `guid` rides along when the ask is about a concrete item, `0` for
    /// template-only) and returns `None` — call again after the answer lands. A cached negative
    /// (server doesn't know the entry) is also `None`, without a re-ask.
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

    /// Whether the server has ANSWERED the `entry` query with "unknown item" — the cached
    /// negative, distinct from a still-pending ask (both read `None` from [`Self::template`]).
    /// The cast-fail redisplay queue (decision 0552) keys on it: pending → keep waiting for the
    /// answer (the ref's `DBCACHECALLBACK` redisplay), negative → give up and show the ref's
    /// `"UNKNOWN"` fallback instead of waiting forever.
    pub(crate) fn template_answered_unknown(&self, entry: u32) -> bool {
        self.templates.answered_unknown(entry)
    }

    /// The held/worn display head for `entry` — the [`HeldTemplate`] view of [`Self::template`]
    /// (same ask-once discipline, template-only ask). Equipment rendering + the swing selector
    /// consume this Copy slice instead of borrowing the full info.
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

    /// [`Self::template`]'s **read-only** twin: the record for `entry` only if it is already
    /// cached, and never an ask. For callers holding `&Items` — the cast ladder's equipped-item
    /// rung runs inside a `&Items` borrow, and by the time a button is pressed the greying feed
    /// that shares its search has had the template for many frames.
    pub(crate) fn template_cached(&self, entry: u32) -> Option<&ItemInfo> {
        self.templates.get(entry)
    }

    /// Record a template answer (`SMSG_ITEM_QUERY_SINGLE_RESPONSE`); `None` = unknown entry.
    pub(crate) fn insert_template(&mut self, entry: u32, info: Option<ItemInfo>) {
        if info.is_some() {
            self.fresh.push(entry);
        }
        // A NEGATIVE answer moves the generation too: it flips the entry from "still asking" to
        // "answered unknown", which is a real display transition for anything that waits on the
        // ask (the cast-fail redisplay's `"UNKNOWN"` literal, [`Self::template_answered_unknown`]).
        self.templates.insert(entry, info);
    }

    /// The landed-template broadcast counter — the cache's own generation, bumped by every
    /// landed answer, positive or negative. The **broadcast** twin of [`Self::fresh`]: `fresh`
    /// is a DRAIN (exactly one consumer can take it — the tooltip feed does), so a second
    /// consumer that caches a template-derived view needs its own signal; it keeps the epoch it
    /// last resolved at and re-resolves when it advances — the modern stand-in for the ref's
    /// `DBCACHECALLBACK` redisplay (`0x6e29b0`), which is how the real client repaints a view
    /// drawn while the item cache was still answering (decision 0660).
    pub(crate) fn template_epoch(&self) -> u64 {
        self.templates.generation()
    }

    /// Drain the entries whose template landed since the last drain (see the `fresh` field).
    /// Every entry with a CACHED template — the re-push sweep's domain (a `$z`-style
    /// player-state input changing means every already-pushed view may be stale).
    pub(crate) fn cached_template_ids(&self) -> Vec<u32> {
        self.templates
            .iter()
            .filter_map(|(&id, t)| t.is_some().then_some(id))
            .collect()
    }

    pub(crate) fn take_fresh(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.fresh)
    }

    /// Disconnect: drop the instances (the server re-streams inventory at login) and the in-flight
    /// asks (a query dropped by a dead writer must be re-askable); keep the templates (static).
    /// Release the ask-once latch without dropping what the cache LEARNED — the world-enter
    /// counterpart to [`Self::clear_session`]'s disconnect teardown.
    ///
    /// `template` marks an entry pending *before* the send, and a send made while the io thread
    /// holds no writer evaporates. Nothing told the cache, so the entry stayed pending for the
    /// life of the process and was never re-asked. That is not hypothetical: `feed_mail` carries
    /// no run condition, so on 2026-09-06 it asked all five `Stationery.dbc` templates at the
    /// login screen, every one was dropped "not connected", and the send tab's stationery list
    /// was empty for the whole session — which the stock `SendMailFrame_Reset` turns into every
    /// send silently unsent. The ask site is fixed; this makes the CLASS harmless, because the
    /// cost of a wrong latch (a feature dead all session, in silence) is nothing like the cost of
    /// a redundant re-ask.
    pub(crate) fn clear_pending(&mut self) {
        self.templates.clear_pending();
    }

    /// The objects themselves — and their countdowns — are the index's, swept with every other
    /// entity (decisions 2334, 2340); the pending enchant times die with the player they were
    /// queued on.
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

/// The ask-once trio every UI resolver needs to be tested: the template cache, the command
/// channel, and — crucially — the channel's live **receiver**, without which every `ItemQuery`
/// send fails and the "asked the server" half of the law goes unobservable. `items` starts empty,
/// so a fresh `TestDeps` reads as "template in flight"; seed one with [`Items::insert_template`]
/// to test the landed arm.
///
/// Shared because the three icon laws (`ui_trainer::service_icon`, `ui_tradeskill::recipe_icon`,
/// `ui_craft::craft_icon`) all terminate in this same cache — the fixture is the one thing they
/// legitimately have in common, unlike the laws themselves.
#[cfg(test)]
pub(crate) struct TestDeps {
    pub(crate) items: Items,
    pub(crate) commands: NetCommands,
    /// The object index the item objects live in (decision 2334) — seed it with
    /// [`Self::spawn_item`], read it through [`Self::with_objects`].
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

    /// An item object in the index — what the wire's `ItemCreate` spawns.
    pub(crate) fn spawn_item(&mut self, guid: u64, fields: ObjectFields) -> Entity {
        test_spawn_item(&mut self.world, guid, fields, false)
    }

    /// Run `f` with the lookup the helpers under test take, beside the template cache and the
    /// command channel — the three borrows split so a test can hold all of them at once.
    pub(crate) fn with_objects<R>(
        &mut self,
        f: impl FnOnce(&Objects, &Items, &NetCommands) -> R,
    ) -> R {
        let (world, items, commands) = (&mut self.world, &self.items, &self.commands);
        let mut state = bevy::ecs::system::SystemState::<Objects>::new(world);
        let objects = state.get(world);
        f(&objects, items, commands)
    }

    /// The entries the resolver asked the server for — an ask-once gate firing is observable here
    /// even when the icon it feeds is still `None`.
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

/// Spawn an item object straight into a test world's index — [`spawn_item`] without the command
/// queue, for a test that holds the `World`.
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

/// Equipment slots 15/16 — `EQUIPMENT_SLOT_MAINHAND` / `_OFFHAND` (vmangos `EquipmentSlots`).
pub(crate) const EQUIPMENT_SLOT_MAINHAND: u8 = 15;
pub(crate) const EQUIPMENT_SLOT_OFFHAND: u8 = 16;

/// One equipment slot's item class, through the guid → entry → template walk. `None` for an empty
/// slot or a template still in flight.
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

/// **Which equipment slot this character's disarm hides** — the ladder
/// ([`crate::creature_anim::disarmed_hand`]) asked of the raw inventory rather than of the
/// resolved hands, which is the form the two *item*-side consumers need: the action bar's
/// equipped-item requirement (`0x5f0c50`, which strips the bit out of its slot mask) and the
/// item-use refusal (`CGItem::Use`'s rung 15, which compares it against the clicked item's own
/// worn position). `None` while the flag is down or neither hand holds a weapon.
///
/// It lives here, next to the cache it reads, so the ladder is stated once and asked twice rather
/// than re-derived per consumer — the failure decision 0664 names, and the reason the quest fork
/// was once missing from all three of its call sites.
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

/// [`disarmed_equipment_slot`]'s read-only twin — same ladder, no ask (decision 1925).
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

/// A minimal, VALID item template named `name` — the shared test seam for every module that
/// needs a landed template (the sentinels that matter are `allowable_*` = −1 and `stackable` = 1).
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

    /// The gated feeds' template-side counter (1439): a template landing moves it, and a lazy
    /// `template()` miss — the per-frame read that poisons `is_changed` — moves nothing. The
    /// instance side is the item entities' own change ticks since 2334
    /// ([`an_item_is_an_object_in_the_index_and_its_changes_are_watched`]).
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

    /// The display step/quantize pair on one item's cells (decision 2340): a parked deadline
    /// contributes `floor(secs)+1` (bounded here, not exact — the test can't pin the sub-second
    /// phase), the display read is floored to the whole second, and clearing the cell takes its
    /// term away (the `Some(0) → None` collapse is the term's own last step).
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

        // The lifetime cell joins the same step sum.
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

    /// The two setters' store rules, each the reference's (`0x5d9c00` / `0x5d9cc0`):
    ///
    /// - **A non-POSITIVE value clears, in both cells.** The test is SIGNED (`jle`), so `0` and
    ///   a wire value with the top bit set both store absence instead of a 68-year deadline.
    ///   Before decision 2340 only the lifetime path knew this; the enchant path parked the far
    ///   future.
    /// - **A slot past the seventh is refused**, where the reference would index past the cell
    ///   array into the next member.
    /// - The deadline read answers `Some(_)` for a live cell and `None` for a slot that never had
    ///   one, where the tooltip read collapses both an unset and an elapsed cell to `None`.
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

    /// The push half of the tooltip store: a landed template is marked fresh exactly once (the
    /// drain empties), and a cached negative never is (there's nothing to push).
    #[test]
    fn landed_templates_drain_as_fresh_once() {
        let mut items = Items::default();
        items.insert_template(117, Some(info("Tough Jerky")));
        items.insert_template(9999, None);
        assert_eq!(items.take_fresh(), vec![117]);
        assert!(items.take_fresh().is_empty(), "a drain empties the queue");
    }

    /// The right-click-to-open predicates — and the fact that the **line and the click are not the
    /// same test** (decision 0896). The tooltip's `shows_open_line` carries the lock sub-gate (a
    /// `LockID` template earns the line only once the INSTANCE says UNLOCKED); the click's
    /// `opens_loot` is the BARE template bit, so a still-locked junkbox sends anyway and the
    /// server supplies the refusal. Getting that backwards eats the click in silence.
    #[test]
    fn open_line_carries_the_lock_gate_the_open_send_does_not() {
        let plain = |flags: u32, lock_id: u32| {
            let mut t = info("Small Barnacled Clam");
            t.flags = flags;
            t.lock_id = lock_id;
            t
        };

        // The clam: LOOTABLE, no lock — line and send agree, instance flags irrelevant.
        assert!(plain(ITEM_FLAG_LOOTABLE, 0).shows_open_line(0));
        assert!(plain(ITEM_FLAG_LOOTABLE, 0).opens_loot());
        // An ordinary item does neither, however its instance is flagged.
        assert!(!plain(0, 0).shows_open_line(ITEM_DYNFLAG_UNLOCKED | ITEM_DYNFLAG_WRAPPED));
        assert!(!plain(0, 0).opens_loot());
        // A junkbox: LOOTABLE but locked. **The two predicates part company here** — no line
        // until the instance says UNLOCKED, but the click goes out either way.
        assert!(!plain(ITEM_FLAG_LOOTABLE, 7).shows_open_line(0));
        assert!(plain(ITEM_FLAG_LOOTABLE, 7).shows_open_line(ITEM_DYNFLAG_UNLOCKED));
        assert!(
            plain(ITEM_FLAG_LOOTABLE, 7).opens_loot(),
            "the send ignores LockID entirely — the server owns the refusal"
        );
        // Gift wrap: the WRAPPER template unwraps only while the instance is still WRAPPED, and
        // that arm is its own dispatcher position, not a nested case of the loot arm.
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

    /// **An item is an object** (decision 2334): spawned into the one index with its store,
    /// resolved through [`Objects`] like a unit, and its create, its delta, a countdown landing
    /// and its despawn each move [`ItemChanges`] exactly once — the gate the inventory feeds watch.
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
        // A countdown landing is the item's own change too (decision 2340), and its cell joins
        // the step sum the feeds watch between landings.
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

    /// **`PlayerPendingItemExpiration`** (decision 2340): an enchant time for an item not yet held
    /// waits for it, is handed over in arrival order once, and a disconnect drops what never
    /// arrived — while the templates the session learned survive it.
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

        // A disconnect drops the records for items that never arrived and the in-flight asks,
        // and keeps the templates.
        let (cmds, rx) = commands();
        items.insert_template(117, Some(info("Tough Jerky")));
        assert!(items.template(118, 0, &cmds).is_none()); // leaves 118 in flight
        items.clear_session();
        assert!(items.take_enchant_times(0x43).is_empty());
        assert!(items.template(117, 0, &cmds).is_some(), "templates survive");
        // 118's ask was dropped with the writer — it must re-ask now.
        let _ = rx.try_recv();
        assert!(items.template(118, 0, &cmds).is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::ItemQuery { entry: 118, .. })
        ));
    }

    /// The id → row join (decisions 0915/0920): slot order is preserved, a `0` slot and an id with
    /// no `SpellItemEnchantment` name are both silently absent (never a placeholder line), and with
    /// no catalog at all nothing renders. Plus the reference's sign rule — **`abs(id)` names the
    /// row, the sign only travels** (`0x52c9f9`), which is why a negative id resolves at all.
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
        // A NEGATIVE id resolves off `abs(id)` — the same row — and only carries its sign onward.
        let neg = enchant_lines([(0, -2564, 0, None)], Some(&cat));
        assert_eq!(neg.len(), 1);
        assert_eq!(neg[0].name, "Agility +15");
        assert!(neg[0].negative, "the sign travels to the colour rule");
        // Charges and a countdown ride the slot through untouched (the engine formats them).
        let temp = enchant_lines([(1, 1900, 5, Some(90_000))], Some(&cat));
        assert_eq!((temp[0].charges, temp[0].remaining_ms), (5, Some(90_000)));
        // An id the table doesn't name contributes nothing at all.
        assert!(enchant_lines([(0, 999_999, 0, None)], Some(&cat)).is_empty());
        // No DBC → no lines, the pre-0915 tooltip.
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

    /// **`0x5da2c0` — the bind question's predicate**, and the reason it reads the raw descriptor.
    ///
    /// Two halves, `||`: the instance's soulbound bit, or any live enchant slot naming a row with
    /// `Flags & 1`. The last case is the one that decides the shape: the Firestone family carries
    /// BOTH the binding bit and the tooltip-suppression bit, so the same item is bound *and*
    /// prints no enchant line — a predicate read off [`enchant_lines`] would call it unbound.
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
        // The one that pins the design: bound, and invisible to the line law.
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
