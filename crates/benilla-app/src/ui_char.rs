//! The app-side **character-window feed** (decision 0208 §3/§4): the bridge that turns our own
//! avatar's descriptor + the item stores into the plain snapshots the paper doll's Lua bindings
//! read, and into the Era events that drive its `OnEvent` repaints — the [`crate::ui_unit`]
//! pattern, for the combat-stats/inventory surface.
//!
//! Three jobs, each frame, before the VM ticks ([`UiInput`]):
//!
//! - **[`UnitCombatStats`]** from the self player's [`ObjectStore`] — the stat/resistance/
//!   damage/attack-power fields (all OWNER_ONLY, verified streamed in 0208) plus the three resolved
//!   skill pairs: the app picks WHICH skill line the equipped main-hand/ranged item uses
//!   (`weapon_subclass_skill` — vmangos `Item.cpp`'s table; no weapon = unarmed 162), reads its
//!   `PLAYER_SKILL_INFO` triplet and the Defense line's ([`SKILL_DEFENSE`]), so the engine-free
//!   bindings just serve the pairs.
//!
//!   The descriptor half is [`unit_combat_stats`], which any unit's store goes through — the pet
//!   paper doll's feed ([`crate::ui_pet_doll`], decision 1057) is the second caller. What
//!   [`combat_stats`] adds on top is exactly the player-only half: the four values derived from the
//!   equipped items, and the skill pairs, neither of which a creature has.
//! - **`InventorySlots`** from the inv-slot guids → [`Items`] objects → templates → the
//!   [`ItemDisplays`] icon — index 0 the ammo slot (`PLAYER_AMMO_ID`, its count summed across the
//!   backpack + the four equipped bags), 1..=19 the equipment slots, 20..=23 the four equipped-bag
//!   ICONS (decision 0216 slice 2's bag bar — the bag item itself, `INV_SLOT` 19..22, not its
//!   contents), all by the client's `GetInventorySlotInfo` ids. A slot whose template answer is
//!   still in flight carries its `item_id` with `icon: None` and fills on a later frame (the
//!   ask-once cache repaints via `UNIT_INVENTORY_CHANGED` when the view changes). Each equipment
//!   slot (1..=23) also carries an `|Hitem:…|h[Name]|h` `link`, its `PendingItemOps`-derived
//!   `locked` (decision 0208 phase 1b — the doll twin of `ui_items::feed_containers`'s bag-slot
//!   `.locked`), and its resolved `equip_slots` (`ui_items::find_equip_slot` over the item
//!   template's `inventoryType` — the cursor arc's "fit rule", decision 0216/0218).
//! - **The paper-doll booth yaw**: the stock pane's rotate buttons call the reference's own
//!   `Model_RotateLeft`/`_RotateRight`, which write `CharacterModelFrame:SetRotation`; the feed
//!   mirrors that pane's facing onto [`PaperDollBooth`] so the booth re-bakes at the new angle
//!   (decision 0208 §5, off the pane itself since 1751).
//!
//! Events, fired on snapshot transitions (grouped by the ref's own registration set,
//! `PaperDollFrame.lua:14-28` — arg1 `"player"`): `UNIT_STATS`, `UNIT_RESISTANCES`, `UNIT_DAMAGE`,
//! `UNIT_ATTACK_SPEED`, `UNIT_ATTACK_POWER`, `UNIT_RANGED_ATTACK_POWER`, `UNIT_RANGEDDAMAGE`,
//! `UNIT_ATTACK` (the weapon-skill pairs) — [`fire_stat_transitions`], shared with the pet feed —
//! and `UNIT_INVENTORY_CHANGED` on any slot-view change. A first snapshot counts as a transition of
//! every group (the `ui_unit` rule).

use bevy::prelude::*;

use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use benilla_ui::script::{
    weapon_subclass_skill, BankBagSlots, InvSlotView, InventorySlots, ScriptValue, SkillEntry,
    SkillsState, UiScript, UnitCombatStats, BANK_BAG_SLOT_COUNT, EQUIPMENT_BAG, SKILL_DEFENSE,
    SKILL_UNARMED,
};
use benilla_ui::strings::Arg;

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::pending_item_ops::PendingItemOps;
use crate::portrait::PaperDollBooth;
use crate::ui_items::{find_equip_slot, item_link};
use crate::ui_script::{gate, UiFeed, UiInput};

/// Equipment slots, 0-based (`EQUIPMENT_SLOT_*`): the inv-slot array index of the main hand,
/// off hand, and ranged slots the weapon-skill / offhand / wand resolutions read.
const SLOT_MAIN_HAND: u8 = 15;
const SLOT_OFF_HAND: u8 = 16;
const SLOT_RANGED: u8 = 17;
/// `ItemPrototype` class 2 = weapon (vmangos `ItemPrototype.h`); the offhand-speed gate.
const ITEM_CLASS_WEAPON: u32 = 2;
/// Weapon subclass 19 = wand (`HasWandEquipped`).
const SUBCLASS_WAND: u32 = 19;

/// A stable comparison key for an f32 snapshot field (the event-transition diffs): quantized to
/// 1/32 so equal wire values always compare equal and real changes always register.
fn fixed(v: f32) -> i64 {
    (v * 32.0).round() as i64
}

/// The feed's change-tracking memory (the [`crate::ui_unit::UnitFeedState`] shape): the last
/// snapshots pushed, so events fire on transitions only.
///
/// All of it is what-we-told-the-VM, so all of it lives behind a [`crate::ui_script::VmMemo`]: a
/// memory about one VM is unreadable against the next (1290), so a `/reload` (1291) — which
/// replaces the VM without despawning the avatar — re-pushes both snapshots and re-fires their
/// events exactly as a fresh login does.
#[derive(Resource, Default)]
pub(crate) struct CharFeedState {
    vm: crate::ui_script::VmMemo<CharFeedMemo>,
}

/// The per-VM half of [`CharFeedState`] — the transition-diff bases.
#[derive(Default)]
struct CharFeedMemo {
    last_stats: Option<UnitCombatStats>,
    last_inv: Option<InventorySlots>,
    last_bank_bags: Option<BankBagSlots>,
    /// The item guid **of the bag itself** in each of the six bank bag slots (absolute player
    /// slots 63..68). `PLAYERBANKSLOTS_CHANGED`'s producer discriminator for this band — the same
    /// question `ui_items::feed`'s `SlotGuids::vault` answers for the vault, and for the same
    /// reason: the pushed view cannot tell two instances of one bag model apart, so a swap of two
    /// identical bags between two slots reads as no change and the reference fires twice
    /// (decision 2140).
    last_bank_bag_guids: [u64; BANK_BAG_SLOT_COUNT],
    /// The gate's counter memories (1439) — the stores whose lazy resolves poison `is_changed`
    /// for this feed (the equipment templates' ask-once, the enchant-name creator lookups). The
    /// item OBJECTS are not among them since 2334: an instance is an entity, so its edges arrive
    /// on the ordinary `Changed`/removal watch ([`crate::items::ItemChanges`]).
    items_templates: gate::Watch,
    names_generation: gate::Watch,
    /// `ItemChanges::countdown_steps` — one step per displayable countdown change (the slot
    /// views read second-floored countdowns), including the last elapse's collapsing push; a
    /// landing is the item's own change tick (decision 2340).
    enchant_deadlines: gate::Watch,
    /// `PendingItemOps::epoch` — one step per change to the in-flight lock set. Watched BESIDE
    /// `!is_empty()` and not instead of it: `is_empty()` holds the gate open *while* an op is in
    /// flight, and this catches the frame it goes away. Without it a clear that moves no object
    /// (`SMSG_INVENTORY_CHANGE_FAILURE`) closes the gate on the same frame it unlocks, and the
    /// last `locked: true` this feed pushed stands (1771).
    pending_epoch: gate::Watch,
}

/// Adds the per-frame character-window feed. The bindings live in `benilla-ui`'s `char_stats`;
/// this supplies their data (and the events) from ECS state.
pub(crate) struct UiCharPlugin;

impl Plugin for UiCharPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharFeedState>().add_systems(
            Update,
            (
                // `.before(feed_containers)`: the container feed's `BAG_UPDATE` handlers read the
                // inventory surface synchronously — an addon mirroring a bag asks the bag ITEM's
                // own row (`GetInventoryItemLink("player", ContainerIDToInventoryID(bag))`,
                // Bagnon_Forever's size row) — so the inventory push must land first. Unordered,
                // the first in-world burst raced it and recorded every equipped bag linkless
                // (`s = "8,0,"` in the director's Bagnon_Forever data). The house "push before
                // fire" rule, across the two feeds.
                feed_char
                    .before(crate::ui_items::feed::feed_containers)
                    .in_set(UiFeed),
                watch_skill_ups.in_set(UiFeed),
                feed_skills.in_set(UiFeed),
                drain_skill_abandons.after(UiInput),
            ),
        );
    }
}

/// The skill block [`watch_skill_ups`] last announced: the self guid it was read off, and that
/// character's `skillId → value` map — the diff base for both the chat lines and the event.
/// `None` = nothing announced yet.
type SkillBlock = Option<(u64, std::collections::HashMap<u16, u16>)>;

/// The client-generated skill-up feedback (decision 0437, landed with phase 2; gate corrected by
/// 1309): the server sends NO skill-up message at all — skills mutate only as
/// `PLAYER_SKILL_INFO` descriptor deltas (verified at the vmangos source: no chat send anywhere
/// in `UpdateSkill*`/`SetSkill`) — so the client diff-watches its own skill block exactly as the
/// real one does (the rank-field UpdateField watcher `0x5de180` → message facility `0x496720`,
/// tokens `ERR_SKILL_UP_SI` / `ERR_SKILL_GAINED_S` — correcting 0437's "SKILL_RANK_UP" token
/// guess), prints the GlobalStrings lines on the Skill chat channel, and fires
/// `SKILL_LINES_CHANGED` on ANY block change (the skills pane's repaint event; the real client
/// fires it from a separate skill-manager TU (`0x4d2fec`) on add/remove only — ours riding the
/// same watcher is a benign coarsening, the pane just repaints). **The message gate** (B19/B245,
/// decisions 1309/1314): both lines are skipped when the line's `SkillRaceClassInfo.flags`
/// carries `0x402` (`SkillRaceClass::skill_up_silent`) — the class spec lines, racials,
/// `GENERIC (DND)`, `Dual Wield` and the mount lines — **or when no row admits this race/class
/// at all** (the resolve's own empty branch, `0x5de352`) — which is why a real 1.12 ding
/// announces no skill at all (the level-up movers are all flagged) while weapon/profession
/// skill-ups still print. The
/// login fill (empty → populated) and a character switch (self guid change) re-seed silently —
/// the event fires, the lines don't. Still open: the exact chat channel of the real emitter
/// (`0x496720`'s routing is untraced; Skill is the `ChatTypeInfo` family's own key).
/// One skill-watcher line: resolve the key off the player's own `GlobalStrings.lua`, fill it, and
/// push it as `CHAT_MSG_SKILL`.
///
/// **Deliberately not `ui_action::show_messages`, and the reason is a real gap rather than a
/// preference.** `ERR_SKILL_UP_SI`/`ERR_SKILL_GAINED_S` (rows 54/53) are the two of the three
/// catalog rows whose `chat_type` is **23 = `CHAT_MSG_SKILL`**, not the `10 = CHAT_MSG_SYSTEM`
/// every other `kind 0` row carries — and that field is exactly what the shared sink does not
/// read yet (its own doc says so: "benilla raises none of them yet; when it does, this is the
/// line that has to read the field"). Routing these through it today would silently demote them
/// to SYSTEM and out of the Skill filter. So the key, the fill and the data-suppression rule are
/// the shared ones; only the surface is carried here, until the sink learns `chat_type`.
///
/// An absent or empty key shows nothing — the reference's own null/empty guard.
fn skill_line(script: &UiScript, chat: &mut crate::ui_chat::ChatLog, key: &str, args: &[Arg<'_>]) {
    let Some(template) = script
        .lua()
        .globals()
        .get::<String>(key)
        .ok()
        .filter(|t| !t.is_empty())
    else {
        return;
    };
    chat.push_event(crate::ui_chat::ChatEvent::text_only(
        crate::ui_chat::ChatEventKind::Skill,
        benilla_ui::strings::fill(&template, args),
    ));
}

fn watch_skill_ups(
    script: Option<NonSendMut<UiScript>>,
    self_guid: Res<crate::net::SelfGuid>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    mut chat: ResMut<crate::ui_chat::ChatLog>,
    mut prev: Local<crate::ui_script::VmMemo<SkillBlock>>,
) {
    let (Some(guid), Ok(store)) = (self_guid.0, self_store.single()) else {
        return;
    };
    // The memo is about the VM — it gates `SKILL_LINES_CHANGED`, the pane's only repaint wire — so
    // it re-seeds with the VM (decision 1290). With no VM there is nothing to announce to and
    // nothing worth remembering: the diff would only queue chat lines for a VM that does not exist
    // yet, which is a stale skill-up replayed at the next login. The next VM re-seeds silently.
    let Some(mut script) = script else {
        return;
    };
    let prev = prev.get(&script);
    let mut cur = std::collections::HashMap::new();
    for i in 0..PLAYER_SKILL_SLOTS {
        if let Some(s) = store.0.player_skill(i) {
            if s.skill_id != 0 {
                cur.insert(s.skill_id, s.value);
            }
        }
    }
    match prev.as_ref() {
        // First fill for this character: seed silently (no lines), announce the block once.
        None => {
            if cur.is_empty() {
                return;
            }
            script.fire_event("SKILL_LINES_CHANGED", vec![]);
            *prev = Some((guid, cur));
        }
        Some((prev_guid, _)) if *prev_guid != guid => {
            // A different character logged in — re-seed, never diff across characters.
            script.fire_event("SKILL_LINES_CHANGED", vec![]);
            *prev = Some((guid, cur));
        }
        Some((_, prev_map)) => {
            if *prev_map == cur {
                return;
            }
            // The 0x402 message gate resolves through the player's own race/class row, the same
            // lookup the pane's display law makes ([`skills_row`]).
            let (race, class) = (
                store.0.unit_race().unwrap_or(0),
                store.0.unit_class().unwrap_or(0),
            );
            for (&id, &value) in &cur {
                let line_id = u32::from(id);
                let name = || {
                    skill_lines
                        .as_ref()
                        .and_then(|s| s.catalog.line(line_id))
                        .map(|l| l.name.clone())
                };
                // The fn doc's message gate; `false` with no catalog (nothing to name it by
                // either — the line stays silent, as before the catalog loads).
                let announces = || {
                    skill_lines
                        .as_ref()
                        .is_some_and(|s| s.catalog.announces_skill_ups(line_id, race, class))
                };
                match prev_map.get(&id) {
                    // A rank-up: `ERR_SKILL_UP_SI`, catalog row 54 — **not** `SKILL_RANK_UP`,
                    // which is the same enUS sentence and no catalog row at all (decision 2045).
                    Some(&old) if value > old => {
                        if let Some(name) = name() {
                            // Both verdicts are logged — the retest's instrument: a moved line
                            // either announces or names the gate that held it.
                            if announces() {
                                debug!("chat: skill-up announced ({name} {old}→{value})");
                                skill_line(
                                    &script,
                                    &mut chat,
                                    "ERR_SKILL_UP_SI",
                                    &[Arg::S(&name), Arg::D(i64::from(value))],
                                );
                            } else {
                                debug!("chat: skill-up silenced ({name} {old}→{value}, the 0x402 gate)");
                            }
                        }
                    }
                    // A NEW line: the ERR_SKILL_GAINED_S line (GlobalStrings.lua:1837) — the
                    // fold-back's second token (the watcher `0x5de180` emits both, first-gain vs
                    // rank-up).
                    None => {
                        if let Some(name) = name() {
                            if announces() {
                                debug!("chat: skill-gain announced ({name} at {value})");
                                skill_line(
                                    &script,
                                    &mut chat,
                                    "ERR_SKILL_GAINED_S",
                                    &[Arg::S(&name)],
                                );
                            } else {
                                debug!(
                                    "chat: skill-gain silenced ({name} at {value}, the 0x402 gate)"
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            script.fire_event("SKILL_LINES_CHANGED", vec![]);
            *prev = Some((guid, cur));
        }
    }
}

/// The skills-pane feed (decision 0437 phase 4, corrected by 1091): every `PLAYER_SKILL_INFO` line
/// the real client would LIST, resolved to a flat [`SkillEntry`] — name off `SkillLine.dbc`, group
/// off the line's `categoryId` × `SkillLineCategory.dbc` (name + displayOrder) — pushed via
/// `set_skills` on change; the ENGINE groups/sorts/folds.
///
/// The inclusion predicate is the client's own list build (`0x4d2cb0`), transcribed in its
/// order: a line needs
/// a `SkillLine.dbc` row, an admitting `SkillRaceClassInfo` row and a real `SkillLineCategory` row;
/// `flags & 0x2` drops it outright; and a line held at **rank 0** appears only if its flags admit
/// it at this player's level. Note what is NOT a filter: the `Not Displayed` category (12). We used
/// to hide on it, which happened to hide `GENERIC (DND)` for the right-looking reason — the client
/// drops that line by its `0x2` bit like `Dual Wield` and the racials, and lists a category-12 line
/// without `0x2` under its own header (decision 1091).
fn feed_skills(
    script: Option<NonSendMut<UiScript>>,
    self_store: Query<Ref<ObjectStore>, With<SelfPlayer>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    mut last: Local<crate::ui_script::VmMemo<Option<SkillsState>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let lines_moved = skill_lines.as_ref().is_some_and(|l| l.is_changed());
    let (Ok(store), Some(skill_lines)) = (self_store.single(), skill_lines.as_deref()) else {
        return;
    };
    // The list is a pure function of our descriptor and the catalog: with both still and a
    // push already made on this VM, nothing below can differ from what the memo holds. It used
    // to build ~40 rows with a cloned name each, every frame, to compare and drop them.
    if last.is_some() && !store.is_changed() && !lines_moved {
        return;
    }
    // The display predicate reads the player's own race/class (the SkillRaceClassInfo row-match —
    // the spellbook's own General-collapse inputs, `ui_spellbook::build_book`) and level (the
    // untrained gate).
    let (race, class) = (
        store.0.unit_race().unwrap_or(0),
        store.0.unit_class().unwrap_or(0),
    );
    let level = store.0.unit_level().unwrap_or(0);
    let mut entries = Vec::new();
    for i in 0..PLAYER_SKILL_SLOTS {
        let Some(slot) = store.0.player_skill(i) else {
            continue;
        };
        if let Some(e) = skills_row(&slot, &skill_lines.catalog, race, class, level) {
            entries.push(e);
        }
    }
    let fresh = SkillsState { entries };
    if last.as_ref() == Some(&fresh) {
        return;
    }
    script.set_skills(fresh.clone());
    *last = Some(fresh);
}

/// One `PLAYER_SKILL_INFO` slot as the Skills tab's row — or `None` when the real client's list
/// build would not list it at all ([`feed_skills`]'s doc for the citations; the order of the tests
/// is the client's own, `0x4d2cb0`). Pure, so the whole display predicate is testable against real
/// DBC data and a real character's skill block (`ui_script::skills_frame_tests`).
pub(crate) fn skills_row(
    slot: &benilla_protocol::messages::PlayerSkillSlot,
    catalog: &benilla_formats::SkillLineCatalog,
    race: u8,
    class: u8,
    level: u32,
) -> Option<SkillEntry> {
    if slot.skill_id == 0 {
        return None;
    }
    let id = u32::from(slot.skill_id);
    // No SkillLine row — nothing to name it by, and the client lists nothing.
    let line = catalog.line(id)?;
    // No row admitting this race/class → the client drops the line (its `!srci → continue`).
    let rc = catalog.race_class(id, race, class)?;
    // A category that must really exist (the client's own null-row test) — and note the category
    // ITSELF is never a filter: `Not Displayed` (12) is a header like any other.
    let (category_name, category_order) = catalog.category(line.category_id)?;
    // The hide bit: `Dual Wield`, the racials, the mount lines, `GENERIC (DND)`.
    if rc.hidden() {
        return None;
    }
    // An untrained line shows only where the flags admit it at this level.
    if slot.value == 0 && !rc.displays_untrained(level) {
        return None;
    }
    Some(SkillEntry {
        skill_id: id,
        name: line.name.clone(),
        value: u32::from(slot.value),
        max: u32::from(slot.max),
        temp_bonus: i32::from(slot.temp_bonus),
        perm_bonus: i32::from(slot.perm_bonus),
        min_level: rc.min_level,
        cost_index: rc.cost_index,
        category_id: line.category_id,
        category_name: category_name.to_string(),
        category_order,
        description: line.description.clone(),
        // The client ANDs the DBC's unlearnable bit with a nonzero skill STEP (`0x4d3953`).
        abandonable: slot.step != 0 && rc.unlearnable(),
        mono: rc.mono(),
    })
}

/// The abandon drain (the skills pane's unlearn button → the `UNLEARN_SKILL` popup's accept →
/// the engine's `AbandonSkill` queue): each queued skill line id becomes one
/// `CMSG_UNLEARN_SKILL`. Nothing else happens client-side — the server's `SetSkill(id, 0, 0)`
/// returns as a `PLAYER_SKILL_INFO` delta and [`feed_skills`] re-pushes the shrunken list
/// (vmangos `SkillHandler.cpp`; the gossip/trainer drains' own shape).
fn drain_skill_abandons(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for skill_id in script.take_skill_abandons() {
        debug!("ui_char: abandon skill line {skill_id}");
        let _ = commands.0.send(ClientCommand::UnlearnSkill { skill_id });
    }
}

/// Resolve one equipped slot's item template entry (`None` = empty slot, or the item object /
/// template hasn't streamed yet).
fn slot_entry(store: &ObjectStore, objects: &Objects, slot0: u8) -> Option<u32> {
    let guid = store.0.player_inv_slot(slot0)?;
    objects.object(guid)?.object_entry()
}

/// The equipped item's weapon-skill line id: the vmangos `Item.cpp` subclass table for a class-2
/// item, unarmed (162) for an empty hand or a non-weapon / unmapped subclass.
fn weapon_skill_id(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    slot0: u8,
) -> u32 {
    let Some(entry) = slot_entry(store, objects, slot0) else {
        return SKILL_UNARMED;
    };
    let Some(t) = items.template(entry, 0, commands) else {
        return SKILL_UNARMED; // in flight — refined next frame when the template lands
    };
    if t.class != ITEM_CLASS_WEAPON {
        return SKILL_UNARMED;
    }
    weapon_subclass_skill(t.subclass).unwrap_or(SKILL_UNARMED)
}

/// Read a skill line's `(value + PERM bonus, TEMP bonus)` pair from the `PLAYER_SKILL_INFO`
/// triplets; `(0, 0)` when the line isn't known (a ranged pair with nothing equipped).
///
/// **The split is not `(value, temp + perm)`, which is what this returned until decision 1812.**
/// The reader every skill-shaped binding goes through is `0x5ea460`, and it writes
/// `*out1 = value + permBonus` / `*out2 = tempBonus` — the permanent half folds into the BASE and
/// only the temporary half is the modifier (the field reads are `+0x84C` `movzx` value,
/// `+0x850` `movsx` temp — the block's only sign-extended read — and `+0x852` `movzx` perm).
///
/// The old shape had the right TOTAL and the wrong halves, which is why nothing caught it: the
/// paper doll's Attack row prints `base` and colours `base + mod`, so a 300-value line with a +5
/// talent and a +10 aura read "300" with a green +15 where the reference reads "305" with a green
/// +10.
///
/// **The perm add is skipped when the value is 0** (`0x5ea4b4 jle`), so an unknown line stays a
/// flat 0 rather than reading back a bare talent bonus.
///
/// Signed, because the sum is: `fild dword` on both pushes, and a perm malus deeper than the skill
/// is representable on the wire even if no live server sends one.
fn skill_pair(store: &ObjectStore, skill_id: u32) -> (i32, i32) {
    for i in 0..PLAYER_SKILL_SLOTS {
        if let Some(s) = store.0.player_skill(i) {
            if u32::from(s.skill_id) == skill_id {
                let value = i32::from(s.value);
                let base = if value == 0 {
                    0
                } else {
                    value + i32::from(s.perm_bonus)
                };
                return (base, i32::from(s.temp_bonus));
            }
        }
    }
    (0, 0)
}

/// Build the **descriptor-only** combat-stats snapshot for ANY unit's store (field mappings pinned
/// in decision 0208; the absent shapes are the descriptor's zero defaults, `percent` 1.0).
///
/// Nothing here reads inventory, templates or skill lines — so it is exactly the part a pet can go
/// through ([`crate::ui_pet_doll`], decision 1057). The `player_*` reads are deliberately left in:
/// a creature has no PLAYER block, so they read absent and fall to their defaults, which is the
/// right pet answer (no buff decomposition, `damage_percent` the divide-safe 1.0) rather than a
/// second unit-only copy of the same twenty lines.
///
/// **That premise is load-bearing, and it was false until decision 1081.** A created store used to
/// answer `Some(0)` for *any* absent index, PLAYER block or not, so on a live pet every default
/// here was dead code — `damage_percent` came through as `0` and the ref's `damage / percent` made
/// the pet sheet read `inf - inf` / `nan`. The protocol layer now bounds "absent = 0" to the
/// object's own descriptor, which is what makes this shared core honest for a creature.
/// A swing time in ms, with `0` (and absent) read as "no swing time yet" → the vanilla 2000. Never
/// zero, because it is the reference's `damage / speed` divisor.
fn base_swing(streamed: Option<u32>) -> u32 {
    streamed.filter(|&ms| ms != 0).unwrap_or(2000)
}

pub(crate) fn unit_combat_stats(store: &ObjectStore) -> UnitCombatStats {
    let f = &store.0;

    let mut stats = [0i32; 5];
    let mut stat_pos = [0i32; 5];
    let mut stat_neg = [0i32; 5];
    for i in 0..5u8 {
        stats[usize::from(i)] = f.unit_stat(i).unwrap_or(0) as i32;
        // The four buff-split arrays are INT on the wire (decision 1397) — they used to be read as
        // f32 and rounded, which turned every real value into `0` and is why B165/B251's stats
        // never went green.
        stat_pos[usize::from(i)] = f.player_posstat(i).unwrap_or(0);
        stat_neg[usize::from(i)] = f.player_negstat(i).unwrap_or(0);
    }
    let mut resistances = [0i32; 7];
    let mut resistance_pos = [0i32; 7];
    let mut resistance_neg = [0i32; 7];
    for i in 0..7u8 {
        resistances[usize::from(i)] = f.unit_resistance(i).unwrap_or(0);
        resistance_pos[usize::from(i)] = f.player_resistance_buff_pos(i).unwrap_or(0);
        resistance_neg[usize::from(i)] = f.player_resistance_buff_neg(i).unwrap_or(0);
    }

    let (ap_pos, ap_neg) = f.unit_attack_power_mods();
    let (rap_pos, rap_neg) = f.unit_ranged_attack_power_mods();

    UnitCombatStats {
        stats,
        stat_pos,
        stat_neg,
        resistances,
        resistance_pos,
        resistance_neg,
        min_damage: f.unit_min_damage().unwrap_or(0.0),
        max_damage: f.unit_max_damage().unwrap_or(0.0),
        min_offhand_damage: f.unit_min_offhand_damage().unwrap_or(0.0),
        max_offhand_damage: f.unit_max_offhand_damage().unwrap_or(0.0),
        physical_bonus_pos: f.player_mod_damage_done_pos(0).unwrap_or(0),
        physical_bonus_neg: f.player_mod_damage_done_neg(0).unwrap_or(0),
        damage_percent: f.player_mod_damage_done_pct(0).unwrap_or(1.0),
        // 2000 ms is the vanilla base swing when the field hasn't streamed — keeps the ref's
        // damage/speed division finite on the first frames. **`0` counts as unstreamed**, not as a
        // swing time: on a created store an absent in-block field reads `Some(0)` (the descriptor's
        // zero), so a plain `unwrap_or` would never fire and `damage / speed` would go to infinity
        // (the shape of the bug decision 1081 is about, one field over).
        main_attack_time_ms: base_swing(f.unit_base_attack_time(0)),
        offhand_attack_time_ms: base_swing(f.unit_base_attack_time(1)),
        attack_power: f.unit_attack_power().unwrap_or(0),
        attack_power_pos: i32::from(ap_pos),
        attack_power_neg: i32::from(ap_neg),
        ranged_attack_power: f.unit_ranged_attack_power().unwrap_or(0),
        ranged_attack_power_pos: i32::from(rap_pos),
        ranged_attack_power_neg: i32::from(rap_neg),
        ranged_attack_time_ms: base_swing(f.unit_ranged_attack_time()),
        ranged_min_damage: f.unit_min_ranged_damage().unwrap_or(0.0),
        ranged_max_damage: f.unit_max_ranged_damage().unwrap_or(0.0),
        // The equipment- and skill-derived half is the PLAYER feed's ([`combat_stats`]); a unit
        // with neither keeps these defaults.
        has_offhand: false,
        has_wand: false,
        main_weapon_skill: (0, 0),
        offhand_weapon_skill: (0, 0),
        ranged_weapon_skill: (0, 0),
        defense_skill: (0, 0),
        // Player fields — the player's own snapshot fills them (`combat_stats`); a pet has none.
        dodge_percent: 0.0,
        parry_percent: 0.0,
        block_percent: 0.0,
    }
}

/// The player's snapshot: [`unit_combat_stats`] plus the half only *we* have — the two values that
/// need the equipped items' templates, and the three `PLAYER_SKILL_INFO` pairs.
fn combat_stats(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> UnitCombatStats {
    // The offhand-speed gate is an offhand *weapon* (a shield doesn't swing); the wand check is
    // the ranged item's subclass. Both read the equipped item's template (ask-once in flight →
    // false this frame, refined when it lands).
    let has_offhand = slot_entry(store, objects, SLOT_OFF_HAND)
        .and_then(|e| items.template(e, 0, commands))
        .is_some_and(|t| t.class == ITEM_CLASS_WEAPON);
    let has_wand = slot_entry(store, objects, SLOT_RANGED)
        .and_then(|e| items.template(e, 0, commands))
        .is_some_and(|t| t.class == ITEM_CLASS_WEAPON && t.subclass == SUBCLASS_WAND);

    let main_skill = weapon_skill_id(store, objects, items, commands, SLOT_MAIN_HAND);
    let offhand_skill = weapon_skill_id(store, objects, items, commands, SLOT_OFF_HAND);
    let ranged_skill_id = slot_entry(store, objects, SLOT_RANGED)
        .and_then(|e| items.template(e, 0, commands))
        .filter(|t| t.class == ITEM_CLASS_WEAPON)
        .and_then(|t| weapon_subclass_skill(t.subclass));

    UnitCombatStats {
        has_offhand,
        has_wand,
        main_weapon_skill: skill_pair(store, main_skill),
        // `UnitAttackBothHands`'s SECOND pair (decision 1810) — the binding pushes one per hand,
        // and `weapon_skill_id` already answers Unarmed for an empty or non-weapon off hand, which
        // is what a shield or an empty hand reads as.
        offhand_weapon_skill: skill_pair(store, offhand_skill),
        ranged_weapon_skill: ranged_skill_id.map_or((0, 0), |id| skill_pair(store, id)),
        // `UnitDefense`'s pair. Its repaint wire is `SKILL_LINES_CHANGED` (which the ref's
        // `PaperDollFrame` registers, l.28, and `watch_skill_ups` already fires), NOT a
        // `UNIT_DEFENSE` — the character sheet never registers one.
        defense_skill: skill_pair(store, SKILL_DEFENSE),
        // `GetDodgeChance`/`GetParryChance`/`GetBlockChance` — player fields, so they live on the
        // player's snapshot and not on the shared core a pet also fills. Already a percent on the
        // wire; an unstreamed field is 0, which is the wire's own default.
        dodge_percent: store.0.player_dodge_percentage().unwrap_or(0.0),
        parry_percent: store.0.player_parry_percentage().unwrap_or(0.0),
        block_percent: store.0.player_block_percentage().unwrap_or(0.0),
        ..unit_combat_stats(store)
    }
}

/// One equipped slot's view: entry + count from the item object, icon/name/quality/link/
/// equip_slots from the ask-once template (icon `None` while the answer is in flight — the
/// empty-slot art shows and the landing template flips the view, which fires
/// `UNIT_INVENTORY_CHANGED`), `locked` from the app's `PendingItemOps` (decision 0208 phase 1b —
/// the same feed `ui_items::feed_containers` reads for a bag slot's own `.locked`).
///
/// Takes the item's **guid and its live-API id** rather than a wire slot index, because the two
/// bands that use it resolve their guid out of two different descriptor arrays: the doll's
/// `PLAYER_FIELD_INV_SLOT_*` (live id = wire slot + 1, so equipment 1..=19 and the four
/// equipped-bag icons 20..=23) and the bank bags' own `PLAYER_FIELD_BANK_BAG_SLOT_*` (live ids
/// 64..=69). Everything past the guid — template, enchants, durability, the pending lock — is
/// identical for both, which is the whole reason there is one function here and not two.
fn slot_view(
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    pending: &PendingItemOps,
    names: &crate::names::NameCache,
    // `ItemSubClass.dbc` — the count gate below reads its `DisplayFlags` bit 2. `None` (the DBC
    // failed to load) leaves every bag at 0, which is the plain-bag answer and the safe one.
    sub_classes: Option<&crate::ui_items::ItemSubClasses>,
    // The player's own `ChrClasses.dbc` relic flag — `find_equip_slot`'s second half (1803). It
    // decides INVSLOT 17 both ways, so it reaches every fit-rule read rather than one of them.
    has_relic_slot: bool,
    guid: u64,
    live_id: u32,
) -> Option<InvSlotView> {
    // The item's own countdown cells — the bag feed's twin (decisions 1933, 2340).
    let countdowns = objects.countdowns(guid);
    let enchant_ms: [Option<u64>; crate::items::ENCHANT_SLOTS] =
        std::array::from_fn(|s| countdowns.and_then(|c| c.enchant_remaining_display_ms(s as u32)));
    let duration_ms = countdowns.and_then(|c| c.lifetime_remaining_display_ms());
    let obj = objects.object(guid)?;
    let entry = obj.object_entry()?;
    let count = obj.item_stack_count().unwrap_or(1).max(1);
    // `OBJECT_FIELD_TYPE & TYPEMASK_CONTAINER` — the fork `GetInventoryItemCount` takes first
    // (`0x4c87a6`), read off the object exactly as the reference reads it rather than inferred
    // from the template.
    let is_container = obj.is_container();
    // A worn container's contents, collected while the bag object is borrowed — summed below if
    // its subclass row carries the count gate.
    let content_guids: Vec<u64> = {
        let slots = obj.container_num_slots().unwrap_or(0).min(36) as u8;
        (0..slots).filter_map(|s| obj.container_slot(s)).collect()
    };
    // The live durability pair (see the bag feed's twin) — the equipped tooltip's line.
    let durability = obj
        .item_durability()
        .zip(obj.item_max_durability())
        .filter(|&(_, max)| max > 0);
    // ITEM_FIELD_FLAGS — the broken/alert laws' wrapped (0x08) and force-red (0x10) bits.
    let flags = obj.item_flags().unwrap_or(0);
    // `0x5da2c0` — soulbound, or carrying a binding enchant: the equipped tooltip's Soulbound
    // override (B310 — the doll is exactly where it was reported). Read off the raw descriptor,
    // never off the enchant LINES below.
    let already_bound = crate::items::already_bound(obj, rolls.enchants);
    // ITEM_FIELD_CREATOR → the ask-once name cache — the equipped tooltip's "<Made by %s>"
    // line (see the bag feed's twin in `ui_items::feed::resolve_slot`).
    let creator = obj
        .item_creator()
        .filter(|&g| g != 0)
        .and_then(|g| names.resolve(g, commands).map(str::to_string));
    // The item's own 7 enchant slots — the equipped tooltip's enchant lines (decision 0915). OUR
    // gear streams as item objects, so this is the full array; an INSPECTED player's tooltip sees
    // only the 2 slots their descriptor broadcasts (`ui_inspect`), as the reference does.
    let enchants = crate::items::enchant_lines(
        (0..7).map(|s| {
            (
                s,
                obj.item_enchant(s).unwrap_or(0),
                obj.item_enchant_charges(s),
                enchant_ms[usize::from(s)],
            )
        }),
        rolls.enchants,
    );
    // `ITEM_FIELD_RANDOM_PROPERTIES_ID` — the roll behind the NAME's "of the Bear" (1547); the
    // roll's enchants are already in the slots read just above.
    let roll = obj.item_random_properties_id();
    let t = items.template(entry, guid, commands);
    let (name, quality, display, link, equip_slots, kind, bar_placeable) = match t {
        Some(t) => {
            let name = rolls.name(&t.name, roll);
            (
                Some(name.clone()),
                t.quality as i32,
                t.display_info_id,
                Some(crate::ui_items::item_link_full(
                    entry, 0, roll, 0, &name, t.quality,
                )),
                find_equip_slot(t.inventory_type, has_relic_slot),
                (t.class, t.subclass),
                t.placeable_on_action_bar(),
            )
        }
        None => (None, 0, 0, None, Vec::new(), (0, 0), false),
    };
    // **A container's count is not its stack count** — `GetInventoryItemCount 0x4c8680` forks on
    // the TYPEMASK bit and then on `ItemSubClass.dbc`'s `DisplayFlags & 0x4` (`0x4c881a`): set →
    // `[CGContainer vtbl+0x10]` → `0x622130`, the sum of the contents' stack counts; clear → 0.
    // In the shipped file that bit is on Soul Bag (1/1) and the Quiver class (11/0..3) only, so a
    // quiver shows its arrows on the bag bar and an ordinary bag shows nothing at all.
    //
    // This REPLACES a `class == 11` guess whose own comment admitted it was inferred from what the
    // reference UI looked like, and which was wrong twice over: it missed the soul bag, and it
    // answered a plain bag's stack count (1) where the client answers 0 — the digit the director
    // saw on `BankFrameBag1` after 1751's swap put a real `isBag` button in front of it.
    let counts_contents = sub_classes.is_some_and(|c| c.0.display_flags(kind.0, kind.1) & 0x4 != 0);
    let contents_count = is_container.then(|| {
        if counts_contents {
            content_guids
                .iter()
                .filter_map(|&g| objects.object(g))
                .map(|o| o.item_stack_count().unwrap_or(1).max(1))
                .sum()
        } else {
            0
        }
    });
    let icon = icons
        .and_then(|i| i.catalog.get(display))
        .and_then(|d| d.icon.clone());
    Some(InvSlotView {
        item_id: entry,
        icon,
        count,
        contents_count,
        quality,
        name,
        link,
        durability,
        flags,
        already_bound,
        locked: pending.contains(EQUIPMENT_BAG, live_id),
        equip_slots,
        bar_placeable,
        creator,
        enchants,
        duration_ms,
    })
}

/// Total carried ammo matching `ammo_id`: the backpack's 16 slots + the four equipped bags'
/// contents (`GetInventoryItemCount("player", ammo)` counts what you can actually shoot — bank
/// and buyback never join).
fn ammo_count(store: &ObjectStore, objects: &Objects, ammo_id: u32) -> u32 {
    let mut n = 0;
    let mut add = |guid: Option<u64>| {
        if let Some(o) = guid.and_then(|g| objects.object(g)) {
            if o.object_entry() == Some(ammo_id) {
                n += o.item_stack_count().unwrap_or(1);
            }
        }
    };
    for i in 0..16u8 {
        add(store.0.player_pack_slot(i));
    }
    for b in 19..23u8 {
        if let Some(bag) = store.0.player_inv_slot(b).and_then(|g| objects.object(g)) {
            let slots = bag.container_num_slots().unwrap_or(0).min(36) as u8;
            for s in 0..slots {
                if let Some(o) = bag.container_slot(s).and_then(|g| objects.object(g)) {
                    if o.object_entry() == Some(ammo_id) {
                        n += o.item_stack_count().unwrap_or(1);
                    }
                }
            }
        }
    }
    n
}

/// Build the 24-wide inventory snapshot: `[0]` ammo, `[1..=19]` the equipment slots, `[20..=23]`
/// the four equipped-bag icons (decision 0216 slice 2's bag bar — the bag ITEM occupying `INV_SLOT`
/// 19..22, not its contents) — all the client's 1-based `GetInventorySlotInfo` ids over the
/// 0-based inv-slot array.
fn inventory_slots(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    pending: &PendingItemOps,
    names: &crate::names::NameCache,
    sub_classes: Option<&crate::ui_items::ItemSubClasses>,
    classes: Option<&benilla_formats::ChrClasses>,
) -> InventorySlots {
    // Resolved once from the player's own class byte, right here — the store is already in scope,
    // so nothing new is threaded through the callers for the class half.
    let has_relic_slot = store
        .0
        .unit_class()
        .is_some_and(|c| classes.is_some_and(|t| t.has_relic_slot(u32::from(c))));
    let mut inv: InventorySlots = Default::default();
    let ammo_id = store.0.player_ammo_id().unwrap_or(0);
    if ammo_id != 0 {
        let t = items.template(ammo_id, 0, commands);
        let (name, quality, display, link) = match t {
            Some(t) => (
                Some(t.name.clone()),
                t.quality as i32,
                t.display_info_id,
                Some(item_link(ammo_id, &t.name, t.quality)),
            ),
            None => (None, 0, 0, None),
        };
        let icon = icons
            .and_then(|i| i.catalog.get(display))
            .and_then(|d| d.icon.clone());
        inv[0] = Some(InvSlotView {
            durability: None,
            flags: 0,
            // Ammo is the one doll slot with no item OBJECT behind it (it is built from the
            // template alone), so the runtime-bind predicate has nothing to read.
            already_bound: false,
            item_id: ammo_id,
            icon,
            count: ammo_count(store, objects, ammo_id),
            // Ammo is not a container — slot 0 is `GetInventoryItemCount`'s own `PLAYER_AMMO_ID`
            // leg, which never reaches the TYPEMASK fork.
            contents_count: None,
            quality,
            name,
            link,
            // The ammo slot is a named deferral this slice (decision 0208 phase 1b): no click
            // path wires it, so there's nothing for a pending op to lock and nowhere for it to
            // fit — both stay the inert default.
            locked: false,
            equip_slots: Vec::new(),
            // Ammo is a named deferral too: no drag path reaches the bar from this slot.
            bar_placeable: false,
            creator: None,
            // Ammo carries no enchant slots to read: the wire never streams an ammo instance
            // here, only the template id off the player descriptor — and for the same reason it
            // carries no expiry countdown either (there is no instance guid to key one on).
            enchants: Vec::new(),
            duration_ms: None,
        });
    }
    // Equipment 1..=19, then Bag0Slot..Bag3Slot (ids 20..23, PaperDollItemFrame.dbc-verified) —
    // the bag bar's icon source: the equipped bag ITEM at INV_SLOT 19..22 (BAG_SLOT_FIRST../
    // SLOT_BAG_FIRST in the container feed's own numbering). One band: the doll's live id is its
    // wire slot plus one across the whole range.
    for live in 1..=23u32 {
        let Some(guid) = store.0.player_inv_slot(live as u8 - 1).filter(|g| *g != 0) else {
            continue;
        };
        inv[live as usize] = slot_view(
            objects,
            items,
            icons,
            rolls,
            commands,
            pending,
            names,
            sub_classes,
            has_relic_slot,
            guid,
            live,
        );
    }
    inv
}

/// The six bank-bag slots' views (live ids 64..=69) — the same [`slot_view`] resolution as any
/// doll slot, off the descriptor's own `PLAYER_FIELD_BANK_BAG_SLOT_*` guids.
///
/// These are inventory slots, not bank-*window* state: the reference's bank paints and picks up a
/// bank bag through `GetInventoryItemTexture`/`PickupBagFromSlot` at
/// `BankButtonIDToInvSlotID(i, 1)` (BankFrame.lua:28-35, 194-198), and the guids stream in the
/// player descriptor whether or not a banker is open. So they ride the inventory feed, beside the
/// doll snapshot, rather than the `BankState` the bank window pushes.
fn bank_bag_slots(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    pending: &PendingItemOps,
    names: &crate::names::NameCache,
    sub_classes: Option<&crate::ui_items::ItemSubClasses>,
) -> BankBagSlots {
    let mut bags: BankBagSlots = Default::default();
    for (i, bag) in bags.iter_mut().enumerate() {
        let Some(guid) = store.0.player_bank_bag_slot(i as u8).filter(|g| *g != 0) else {
            continue;
        };
        *bag = slot_view(
            objects,
            items,
            icons,
            rolls,
            commands,
            pending,
            names,
            sub_classes,
            // A bank bag slot holds a BAG, and `find_equip_slot(INVTYPE_BAG)` answers the four
            // bag slots whatever this says — the flag decides INVSLOT 17 alone, which no item in
            // this band can reach. `false` here is provable, not a default.
            false,
            guid,
            BANK_BAG_LIVE_FIRST + i as u32,
        );
    }
    bags
}

/// `BankButtonIDToInvSlotID(1, isBag)` — the live-API id of the FIRST bank-bag slot. The band is
/// six wide (`BANK_BAG_SLOT_COUNT`).
const BANK_BAG_LIVE_FIRST: u32 = 64;

/// Fire the paper doll's per-group repaint events for one unit's snapshot transition, `arg1 =
/// token` — the [`crate::ui_unit::fire_transitions`] shape, for the combat-stats surface.
///
/// The groups ARE the ref's own event registrations (`PaperDollFrame.lua:14-28`, which
/// `PetPaperDollFrame.lua:12-20` repeats verbatim for the pet page), so each event fires exactly
/// when a value some handler of it reads has moved; `prev == None` (a first snapshot) counts as a
/// transition of every group, the `ui_unit` rule.
///
/// Shared by both feeds rather than transcribed twice (decision 1057): the pet page calls the same
/// `PaperDollFrame_Set*` helpers off the same events, so two copies of this grouping could only
/// ever drift into one page repainting on an edge the other misses.
pub(crate) fn fire_stat_transitions(
    script: &mut UiScript,
    token: &str,
    prev: Option<&UnitCombatStats>,
    stats: &UnitCombatStats,
) {
    let tok = || ScriptValue::Str(token.to_string());
    let changed = |sel: fn(&UnitCombatStats) -> Vec<i64>| prev.is_none_or(|p| sel(p) != sel(stats));
    if changed(|s| {
        let mut v: Vec<i64> = s.stats.iter().map(|&x| i64::from(x)).collect();
        v.extend(
            s.stat_pos
                .iter()
                .chain(s.stat_neg.iter())
                .map(|&x| i64::from(x)),
        );
        v
    }) {
        script.fire_event("UNIT_STATS", vec![tok()]);
    }
    if changed(|s| {
        s.resistances
            .iter()
            .chain(s.resistance_pos.iter())
            .chain(s.resistance_neg.iter())
            .map(|&x| i64::from(x))
            .collect()
    }) {
        script.fire_event("UNIT_RESISTANCES", vec![tok()]);
    }
    if changed(|s| {
        vec![
            fixed(s.min_damage),
            fixed(s.max_damage),
            fixed(s.min_offhand_damage),
            fixed(s.max_offhand_damage),
            i64::from(s.physical_bonus_pos),
            i64::from(s.physical_bonus_neg),
            fixed(s.damage_percent),
        ]
    }) {
        script.fire_event("UNIT_DAMAGE", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.main_attack_time_ms),
            i64::from(s.offhand_attack_time_ms),
            i64::from(s.has_offhand),
        ]
    }) {
        script.fire_event("UNIT_ATTACK_SPEED", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.attack_power),
            i64::from(s.attack_power_pos),
            i64::from(s.attack_power_neg),
        ]
    }) {
        script.fire_event("UNIT_ATTACK_POWER", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.ranged_attack_power),
            i64::from(s.ranged_attack_power_pos),
            i64::from(s.ranged_attack_power_neg),
            i64::from(s.has_wand),
        ]
    }) {
        script.fire_event("UNIT_RANGED_ATTACK_POWER", vec![tok()]);
    }
    if changed(|s| {
        vec![
            fixed(s.ranged_min_damage),
            fixed(s.ranged_max_damage),
            i64::from(s.ranged_attack_time_ms),
        ]
    }) {
        script.fire_event("UNIT_RANGEDDAMAGE", vec![tok()]);
    }
    if changed(|s| {
        vec![
            i64::from(s.main_weapon_skill.0),
            i64::from(s.main_weapon_skill.1),
            i64::from(s.ranged_weapon_skill.0),
            i64::from(s.ranged_weapon_skill.1),
        ]
    }) {
        script.fire_event("UNIT_ATTACK", vec![tok()]);
    }
}

pub(crate) fn feed_char(
    // `ChrClasses.dbc` field 16, for the fit rule's relic half — `ui_items::find_equip_slot`
    // (1803). Absent client data reads every class as an ordinary ranged wielder.
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    script: Option<NonSendMut<UiScript>>,
    // The inventory read (2334): the self store, its change tick, the object lookup and the item
    // entities' change watch.
    mut inv: crate::items::Inventory,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    // `SpellItemEnchantment`'s name column — the equipped tooltip's enchant lines (decision 0915).
    enchants: Option<Res<crate::items::Enchants>>,
    // `ItemRandomProperties` — the roll behind an equipped item's "of the Monkey" name (1547).
    props: Option<Res<crate::items::RandomProperties>>,
    commands: Res<NetCommands>,
    mut feed: ResMut<CharFeedState>,
    mut booth: ResMut<PaperDollBooth>,
    pending: Res<PendingItemOps>,
    names: Res<crate::names::NameCache>,
    // `ItemSubClass.dbc` — `GetInventoryItemCount`'s bag gate (`slot_view`'s own note).
    sub_classes: Option<Res<crate::ui_items::ItemSubClasses>>,
) {
    let Some(mut script) = script else {
        return;
    };
    // The pane owns the yaw and the booth mirrors it (0208 §5). Read off the widget's own
    // `SetRotation` state since decision 1751 put the reference's `PaperDollFrame.xml` on the
    // chain: the stock file declares a real `<PlayerModel name="CharacterModelFrame">` and drives
    // it with `Model_OnLoad`/`Model_Rotate*`/`Model_OnUpdate`, where ours used to call a
    // benilla-named `BenillaPaperDollModel_SetFacing` into a scalar on the VM.
    // Write-if-different: an unconditional write would mark the booth changed every frame.
    let yaw = script.model_pane_facing("CharacterModelFrame");
    if booth.yaw != yaw {
        booth.yaw = yaw;
    }

    // Resolved against THIS VM (1290/1291): a `/reload`'s replacement reads a fresh memo, so both
    // snapshots below re-push and their events re-fire for it.
    let (memo, vm_reset) = feed.vm.get_reset(&script);

    let Some(store) = inv.self_store.iter().next() else {
        // The self store's absence is NO DATA, never "the player has nothing equipped". The one
        // window this branch runs with a previously-fed VM is the logout despawn frames —
        // `SMSG_LOGOUT_COMPLETE` despawns the entity a full Update before `OnExit(InWorld)` runs
        // the shutdown, and the `PLAYER_LOGOUT` handlers that follow still read equipment
        // (`GetInventoryItemLink("player", …)` at logout is a stock addon save pattern; the
        // reference keeps it valid through its shutdown). The clear this branch used to push was
        // a pre-1290 relic: one VM then lived across logouts and the next character must not see
        // this one's inventory — now the VM dies with the session and a fresh one starts empty
        // ([`crate::ui_script::end_ui_session`]), so the clear only ever fired into a VM about to
        // run its logout handlers. Same law as the container feed
        // (`ui_items::feed::apply_container_source`).
        return;
    };

    // `GetWeaponEnchantInfo`'s whole data path (the buff bar's TemporaryEnchantFrame row, plus 8
    // corpus addons). Pushed EVERY frame and change-gated by nothing — including the 1439 gate
    // below: the remaining time is a live countdown, so riding a snapshot diff would fire
    // `UNIT_INVENTORY_CHANGED` on every tick. The reference recomputes it per call for exactly
    // the same reason (`0x5d9d00`). Hoisted above the gate (it used to sit last): the push is
    // order-free of the two snapshots, and a handler they fire reads the fresher value this way.
    // Bound before the snapshot build below shadows `inv` with the pushed slot array.
    let objects = &inv.objects;
    script.set_weapon_enchants(
        weapon_enchant(store, objects, EQUIPMENT_SLOT_MAINHAND),
        weapon_enchant(store, objects, EQUIPMENT_SLOT_OFFHAND),
    );

    // The gate (1439), around the two snapshot builds only: the self descriptor (equipment
    // slots, every stat field), the item stores behind the equipped templates and enchant
    // names, the two catalogs, and the pending-op locks (held open while in flight; the
    // resolving frame's own field update moves the object epoch).
    let objects_moved = inv.changes.moved();
    let templates_moved = memo.items_templates.moved(items.template_epoch());
    let names_moved = memo.names_generation.moved(names.generation());
    // The DISPLAY epoch, not a per-frame hold-open: the snapshot reads second-floored countdowns
    // (`enchant_remaining_display_ms`), so it can only differ when this counter steps — once a
    // second per ticking enchant, plus one final step at the elapse. Holding the gate open on
    // `live_enchant_deadlines() > 0` (the first 1439 shape) rebuilt both snapshots and fired
    // `UNIT_INVENTORY_CHANGED` at frame rate for the whole life of a poison.
    let deadlines_moved = memo.enchant_deadlines.moved(inv.changes.countdown_steps());
    let self_changed = !inv.self_changed.is_empty();
    // `is_added` for the icon catalog: the feeds read only its load-once icon column;
    // its model-cache half churns every frame (the containers gate's own note).
    let icons_added = icons.as_ref().is_some_and(|r| r.is_added());
    // Both DBC catalogs load once, at startup — one input covers the pair.
    let enchants_changed = enchants.as_ref().is_some_and(|r| r.is_changed())
        || props.as_ref().is_some_and(|r| r.is_changed());
    let pending_held = !pending.is_empty();
    let pending_moved = memo.pending_epoch.moved(pending.epoch());
    gate::trace(
        "feed_char",
        &[
            ("vm_reset", vm_reset),
            ("objects", objects_moved),
            ("templates", templates_moved),
            ("names", names_moved),
            ("deadlines", deadlines_moved),
            ("self", self_changed),
            ("icons", icons_added),
            ("enchants", enchants_changed),
            ("pending", pending_held),
            ("pending-epoch", pending_moved),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || objects_moved
            || templates_moved
            || names_moved
            || deadlines_moved
            || self_changed
            || icons_added
            || enchants_changed
            || pending_held
            || pending_moved,
    );
    if gate.skip() {
        return;
    }

    let stats = combat_stats(store, objects, &items, &commands);
    if memo.last_stats.as_ref() != Some(&stats) {
        gate.audit("feed_char", "the combat-stats snapshot");
        // PUSH before firing: event dispatch runs the Lua handlers synchronously, so the snapshot
        // must already be in the VM when they repaint (the ui_unit rule — a fire-first ordering
        // paints the OLD values and, being transition-gated, never corrects itself).
        let prev = memo.last_stats.take();
        script.set_player_combat_stats(Some(stats.clone()));
        fire_stat_transitions(&mut script, "player", prev.as_ref(), &stats);
        memo.last_stats = Some(stats);
    }

    let inv = inventory_slots(
        store,
        objects,
        &items,
        icons.as_deref(),
        crate::items::RollCatalogs {
            enchants: enchants.as_deref(),
            props: props.as_deref(),
        },
        &commands,
        &pending,
        &names,
        sub_classes.as_deref(),
        classes.as_deref().map(|t| &t.0),
    );
    let bank_bags = bank_bag_slots(
        store,
        objects,
        &items,
        icons.as_deref(),
        crate::items::RollCatalogs {
            enchants: enchants.as_deref(),
            props: props.as_deref(),
        },
        &commands,
        &pending,
        &names,
        sub_classes.as_deref(),
    );
    // One transition for both bands: they are the same descriptor read at two offsets, and
    // `UNIT_INVENTORY_CHANGED` is the one repaint signal either has. Pushing them separately
    // would fire it twice for a single wire update.
    // `PLAYERBANKSLOTS_CHANGED`'s two producers, planned off the OLD memo before the push
    // replaces it. `false` = P1, the player-descriptor path (`0x5ddd6e`, no arguments — a bag
    // arriving, leaving or exchanged); `true` = P2, the item-object path (`0x4c728d`,
    // `arg1 = "player"` — the same bag, its own fields changed). The full account and why the guid
    // is the only sound discriminator are at `ui_items::feed`'s vault twin; decision 2140.
    let bank_bag_guids: [u64; BANK_BAG_SLOT_COUNT] =
        std::array::from_fn(|i| store.0.player_bank_bag_slot(i as u8).unwrap_or(0));
    let repainted: Vec<bool> = (0..BANK_BAG_SLOT_COUNT)
        .filter_map(|i| {
            if bank_bag_guids[i] != memo.last_bank_bag_guids[i] {
                Some(false)
            } else {
                let was = memo.last_bank_bags.as_ref().and_then(|b| b[i].as_ref());
                (!same_item(was, bank_bags[i].as_ref())).then_some(true)
            }
        })
        .collect();
    memo.last_bank_bag_guids = bank_bag_guids;
    if memo.last_inv.as_ref() != Some(&inv) || memo.last_bank_bags.as_ref() != Some(&bank_bags) {
        gate.audit("feed_char", "the inventory snapshot");
        script.set_inventory_slots(inv.clone());
        script.set_bank_bag_slots(bank_bags.clone());
        script.fire_event(
            "UNIT_INVENTORY_CHANGED",
            vec![ScriptValue::Str("player".to_string())],
        );
        // The six bank BAG buttons' only repaint signal (1771). `BankItemButtonTemplate`'s
        // `<OnUpdate>` is `CursorOnUpdate()` — despite its name, `BankFrameItemButton_OnUpdate`
        // (the icon/count/lock repaint) is reached ONLY from `BankFrameItemButton_OnEvent`, and
        // only for `PLAYERBANKSLOTS_CHANGED` and `BANKFRAME_OPENED`. Without this the buttons
        // showed whatever was true at the last unrelated vault-slot event: a bag dropped in never
        // appeared, one moved between slots stayed drawn in its old one, and a lock left over from
        // the drop stood greyed until the window was reopened.
        //
        // The VAULT band's copy of this event is `ui_items::feed_containers`' — each feed
        // announces the band whose data it owns, and `feed_char` is ordered first, so both fire
        // after their own push.
        //
        // **Which arguments it carries depends on WHICH producer fired** (folded back in 2140).
        // The descriptor watcher `0x5ddd6e` calls `FrameScript_SignalEvent 0x703e50`, an
        // `__fastcall(ecx = id)` with a plain `ret` and no vararg push — zero Lua values; the
        // item-object notifier `0x4c728d` goes through `SignalEvent2` and pushes the literal
        // string `"player"`. benilla fired the argless one for both until 2140. An event is fired
        // per changed slot, as the watcher does — the slot travels in *how many* times it fires,
        // never in an argument (1776).
        memo.last_inv = Some(inv);
        memo.last_bank_bags = Some(bank_bags);
    }
    // Outside the push block, because the identical-bag swap it exists for moves no view at all.
    if !repainted.is_empty() {
        gate.audit("feed_char", "a bank bag slot transition");
    }
    for same_bag in repainted {
        let args = if same_bag {
            vec![ScriptValue::Str("player".to_string())]
        } else {
            Vec::new()
        };
        script.fire_event("PLAYERBANKSLOTS_CHANGED", args);
    }
}

/// Whether two bank-bag views hold the same ITEM — everything but `locked`, which is
/// `ITEM_LOCK_CHANGED`'s business and must not masquerade as a slot change.
fn same_item(a: Option<&InvSlotView>, b: Option<&InvSlotView>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let unlocked = |v: &InvSlotView| InvSlotView {
                locked: false,
                ..v.clone()
            };
            unlocked(a) == unlocked(b)
        }
        _ => false,
    }
}

/// The client's own 0-based `EQUIPMENT_SLOT_*` ids for the two weapon slots — the numbers
/// `GetWeaponEnchantInfo 0x4c9790` hands its container lookup (`push 0xf` at `0x4c97c3`,
/// `push 0x10` at `0x4c988b`). The live-API `GetInventorySlotInfo` ids the *frames* use are one
/// higher (16 `MainHandSlot`, 17 `SecondaryHandSlot`).
const EQUIPMENT_SLOT_MAINHAND: u8 = 15;
const EQUIPMENT_SLOT_OFFHAND: u8 = 16;

/// `ITEM_FIELD_ENCHANTMENT`'s temporary slot (0 = permanent, 1 = temporary, 2..6 random property)
/// — the slot index the reference passes to its remaining-time reader (`push 0x1`, `0x4c9828`).
const TEMP_ENCHANTMENT_SLOT: u8 = 1;

/// One weapon's temporary enchantment for [`benilla_ui::script::WeaponEnchant`], read off the
/// equipped item object.
///
/// **The RAW enchantment triple, not [`InvSlotView::enchants`]**: that view is tooltip-shaped and
/// drops both an id the `SpellItemEnchantment` catalog cannot name and the whole `Flags & 0x2`
/// print-no-line family (decision 0928 — the totem weapon imbues), which is exactly the set the
/// enchant row exists to show. The binary's gate is `[descriptor+0x4c] != 0`, the id and nothing
/// else, and `item_enchant` already answers `None` for a zero id.
fn weapon_enchant(
    store: &ObjectStore,
    objects: &Objects,
    slot0: u8,
) -> Option<benilla_ui::script::WeaponEnchant> {
    let guid = store.0.player_inv_slot(slot0)?;
    // The *deadline* read, not the tooltip's: an elapsed timer must answer 0, not "no timer".
    let remaining_ms = objects
        .countdowns(guid)
        .and_then(|c| c.enchant_deadline_ms(u32::from(TEMP_ENCHANTMENT_SLOT)));
    let obj = objects.object(guid)?;
    obj.item_enchant(TEMP_ENCHANTMENT_SLOT)?;
    Some(benilla_ui::script::WeaponEnchant {
        remaining_ms,
        charges: obj.item_enchant_charges(TEMP_ENCHANTMENT_SLOT),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;

    /// **The skill pair splits `(value + PERM, TEMP)`, not `(value, temp + perm)`** — the reader
    /// every skill-shaped binding goes through (`0x5ea460`).
    ///
    /// It reported the right TOTAL and the wrong halves until decision 1812, which is exactly why
    /// nothing caught it: `UnitAttackBothHands`'s caller prints the base and colours `base + mod`,
    /// so both shapes agree on the number a player adds up and disagree on the two they read. The
    /// fixture below is the one the record quotes — 300 / +5 talent / +10 aura — where the
    /// reference shows **305** with a green **+10** and ours showed 300 with a green +15.
    ///
    /// The zero-value leg is the other half of the law (`0x5ea4b4 jle`): an unknown line must stay
    /// a flat 0 rather than reading back a bare talent bonus.
    #[test]
    fn a_skill_pair_folds_the_permanent_bonus_into_the_base() {
        /// `PLAYER_SKILL_INFO_1_1` — id|step, then value|max, then temp|perm, per slot.
        const F_SKILL: u16 = 718;
        let pair = |lo: u16, hi: u16| u32::from(lo) | (u32::from(hi) << 16);
        // Slot 0: Defense (95) at 300, +10 temporary, +5 permanent.
        // Slot 1: Swords (43) at 0 — known id, no value, a +7 permanent that must NOT surface.
        let store = ObjectStore(ObjectFields::from_pairs(&[
            (F_SKILL, pair(95, 0)),
            (F_SKILL + 1, pair(300, 300)),
            (F_SKILL + 2, pair(10, 5)),
            (F_SKILL + 3, pair(43, 0)),
            (F_SKILL + 4, pair(0, 300)),
            (F_SKILL + 5, pair(0, 7)),
        ]));
        assert_eq!(
            skill_pair(&store, 95),
            (305, 10),
            "the permanent bonus belongs to the base and the temporary one is the modifier"
        );
        assert_eq!(
            skill_pair(&store, 43),
            (0, 0),
            "a line at value 0 skips the perm add — a bare talent bonus is not a skill"
        );
        assert_eq!(
            skill_pair(&store, 162),
            (0, 0),
            "a line the player does not have at all"
        );
        // A malus deeper than the skill is representable, which is why the pair is signed.
        let cursed = ObjectStore(ObjectFields::from_pairs(&[
            (F_SKILL, pair(95, 0)),
            (F_SKILL + 1, pair(5, 300)),
            (F_SKILL + 2, pair(0, u16::MAX - 9)), // perm = -10
        ]));
        assert_eq!(skill_pair(&cursed, 95), (-5, 0));
    }

    /// `PLAYER_FIELD_BANK_BAG_SLOT_1` — bank bag slot 0's guid pair (protocol test `bank.rs`).
    const F_BANK_BAG_1: u16 = 612;
    /// `OBJECT_FIELD_ENTRY` on the item object.
    const F_OBJECT_ENTRY: u16 = 3;
    /// `ITEM_FIELD_STACK_COUNT` — one of the six item fields the reference's own watcher covers.
    const F_ITEM_STACK_COUNT: u16 = 14;
    /// "Traveler's Backpack" — any container entry; the feed only needs it to resolve.
    const BAG_ENTRY: u32 = 4500;
    const BAG: u64 = 0x4000_0000_0000_0abc;

    /// Re-stream one live item object's fields — the wire's values delta, which is what moves
    /// the item entity's own change tick (2334); the create-time spawn is
    /// [`crate::items::test_spawn_item`].
    fn restream(app: &mut App, guid: u64, fields: ObjectFields) {
        let entity = app
            .world()
            .resource::<crate::net::GuidIndex>()
            .0
            .get(&guid)
            .copied()
            .expect("the item was streamed");
        *app.world_mut().get_mut::<ObjectStore>(entity).unwrap() = ObjectStore(fields);
    }

    /// A world with the player carrying `bag` (or nothing) in the FIRST bank bag slot, and a
    /// recorder frame registered for the two events the reference's bank bag buttons repaint on.
    fn world_with_bank_bag(bag: Option<u64>) -> App {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<CharFeedState>()
            .init_resource::<PaperDollBooth>()
            .init_resource::<PendingItemOps>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::names::NameCache>()
            .insert_resource(NetCommands(tx));
        let fields = match bag {
            Some(g) => ObjectFields::from_pairs(&[
                (F_BANK_BAG_1, g as u32),
                (F_BANK_BAG_1 + 1, (g >> 32) as u32),
            ]),
            None => ObjectFields::from_pairs(&[(F_BANK_BAG_1, 0), (F_BANK_BAG_1 + 1, 0)]),
        };
        app.world_mut().spawn((SelfPlayer, ObjectStore(fields)));
        if let Some(g) = bag {
            // The bag is an object in the one index (2334), its template still on `Items`.
            crate::items::test_spawn_item(
                app.world_mut(),
                g,
                ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, BAG_ENTRY)]),
                true,
            );
            app.world_mut().resource_mut::<Items>().insert_template(
                BAG_ENTRY,
                Some(crate::items::test_template("Traveler's Backpack")),
            );
        }
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYERBANKSLOTS_CHANGED")
                f:RegisterEvent("UNIT_INVENTORY_CHANGED")
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, event .. " " .. tostring(arg1))
                end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    fn seen(app: &mut App) -> Vec<String> {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let out = script.eval::<Vec<String>>("return SEEN").unwrap();
        script.run("SEEN = {}").unwrap();
        out
    }

    /// **A bank bag slot's only repaint signal** (1771). `PLAYERBANKSLOTS_CHANGED` is the one
    /// event that reaches `BankFrameItemButton_OnUpdate` for the six bag buttons —
    /// `UNIT_INVENTORY_CHANGED`, which this feed already fired, is not registered by them at all,
    /// and `ITEM_LOCK_CHANGED` repaints the desaturation without ever touching the icon. Without
    /// this fire a bag dropped into the bank never appeared, and one moved between slots stayed
    /// drawn where it had been.
    #[test]
    fn a_bank_bag_slot_filling_fires_playerbankslots_changed() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut().run_system_once(feed_char).unwrap();
        let events = seen(&mut app);
        assert!(
            events.contains(&"PLAYERBANKSLOTS_CHANGED nil".to_string()),
            "the repaint event, and it carries NO argument — `0x703e50` pushes none, and the \
             recorder stringifies the absent arg1 as nil. Got {events:?}"
        );
        assert!(
            events.contains(&"UNIT_INVENTORY_CHANGED player".to_string()),
            "and the doll's, unchanged, got {events:?}"
        );
    }

    /// **The second producer** (decision 2140). The same bag, its own `ITEM_FIELD_STACK_COUNT`
    /// moving, is not the descriptor path — it is `0x4c7180`'s item-object path, which goes
    /// through `SignalEvent2` and pushes the unit token `"player"`. benilla fired the argless
    /// descriptor shape for both.
    #[test]
    fn a_bank_bags_own_field_moving_fires_the_item_object_producer() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut().run_system_once(feed_char).unwrap();
        assert!(seen(&mut app).contains(&"PLAYERBANKSLOTS_CHANGED nil".to_string()));

        // The SAME bag guid, restacked. Only the item object moved — a values update on the
        // live item entity, which is what the wire's own delta is (2334).
        restream(
            &mut app,
            BAG,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, BAG_ENTRY), (F_ITEM_STACK_COUNT, 3)]),
        );
        app.world_mut().run_system_once(feed_char).unwrap();
        let events = seen(&mut app);
        assert!(
            events.contains(&"PLAYERBANKSLOTS_CHANGED player".to_string()),
            "the item path pushes the token, got {events:?}"
        );
    }

    /// Two IDENTICAL bags exchanged between two bank bag slots — the case the pushed view cannot
    /// see, because two instances of one bag model push the same `InvSlotView`. The reference
    /// watches the descriptor GUIDs, so it fires the argless event twice; benilla, diffing the
    /// view, fired nothing at all. Same shape as 1777's `BAG_CLOSED`, one band over.
    #[test]
    fn two_identical_bags_swapped_between_bank_bag_slots_still_announce() {
        let mut app = world_with_bank_bag(Some(BAG));
        // A second, byte-identical bag in bank bag slot 2.
        const BAG2: u64 = 0x4000_0000_0000_0abd;
        // The two bank bag slots' guids, rewritten wholesale — `ObjectFields` has no setter, and
        // a fresh store is what a descriptor update produces anyway.
        let set_slots = |app: &mut App, a: u64, b: u64| {
            let mut q = app
                .world_mut()
                .query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
            let mut store = q.single_mut(app.world_mut()).unwrap();
            *store = ObjectStore(ObjectFields::from_pairs(&[
                (F_BANK_BAG_1, a as u32),
                (F_BANK_BAG_1 + 1, (a >> 32) as u32),
                (F_BANK_BAG_1 + 2, b as u32),
                (F_BANK_BAG_1 + 3, (b >> 32) as u32),
            ]));
        };
        set_slots(&mut app, BAG, BAG2);
        crate::items::test_spawn_item(
            app.world_mut(),
            BAG2,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, BAG_ENTRY)]),
            true,
        );
        app.world_mut().run_system_once(feed_char).unwrap();
        let _ = seen(&mut app);

        // Exchange them. Every pushed view is equal before and after.
        set_slots(&mut app, BAG2, BAG);
        app.world_mut().run_system_once(feed_char).unwrap();
        let events = seen(&mut app);
        assert_eq!(
            events
                .iter()
                .filter(|e| *e == "PLAYERBANKSLOTS_CHANGED nil")
                .count(),
            2,
            "one argless event per slot whose guid moved, got {events:?}"
        );
    }

    /// The other half of the same rule: a LOCK flip is not a slot change. The reference has a
    /// separate event for it (`ITEM_LOCK_CHANGED`, fired by `ui_items::feed_containers` off
    /// `LockTransitions`), and firing the repaint event for it too would announce a change to the
    /// item that did not happen.
    #[test]
    fn a_lock_flip_alone_does_not_fire_the_repaint_event() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut().run_system_once(feed_char).unwrap();
        let _ = seen(&mut app);

        // The same bag, now locked by an outbound move.
        app.world_mut().resource_mut::<PendingItemOps>().add([(
            EQUIPMENT_BAG,
            BANK_BAG_LIVE_FIRST,
            BAG,
            1,
        )]);
        app.world_mut().run_system_once(feed_char).unwrap();
        let events = seen(&mut app);
        assert!(
            events.contains(&"UNIT_INVENTORY_CHANGED player".to_string()),
            "the snapshot did move — `locked` is part of it, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| e.starts_with("PLAYERBANKSLOTS_CHANGED")),
            "…but the ITEM did not change, so the repaint event stays quiet, got {events:?}"
        );
    }

    /// The gate hole behind the stuck grey: the frame an op's lock CLEARS, `!pending.is_empty()`
    /// reads false and every "is anything in flight?" input closes — so a feed that had pushed
    /// `locked: true` must have some other reason to run, or the stale lock stands forever.
    /// [`PendingItemOps::epoch`] is that reason.
    #[test]
    fn the_frame_a_lock_clears_still_repushes_the_slot() {
        let mut app = world_with_bank_bag(Some(BAG));
        app.world_mut().resource_mut::<PendingItemOps>().add([(
            EQUIPMENT_BAG,
            BANK_BAG_LIVE_FIRST,
            BAG,
            1,
        )]);
        app.world_mut().run_system_once(feed_char).unwrap();
        let _ = seen(&mut app);
        assert!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<bool>(r#"return IsInventoryItemLocked(64) and true or false"#)
                .unwrap(),
            "in flight, the slot reads locked"
        );

        // The server answers with a refusal: the lock clears without any object moving, which is
        // exactly the case `!is_empty()` cannot see.
        let cleared = app
            .world_mut()
            .resource_mut::<PendingItemOps>()
            .clear_by_failure(BAG);
        assert_eq!(cleared, vec![(EQUIPMENT_BAG, BANK_BAG_LIVE_FIRST)]);
        app.world_mut().run_system_once(feed_char).unwrap();
        assert!(
            !app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<bool>(r#"return IsInventoryItemLocked(64) and true or false"#)
                .unwrap(),
            "the unlock has to reach the VM in the same frame it happened"
        );
    }
}
