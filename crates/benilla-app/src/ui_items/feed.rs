//! The inward half of the container seam (see the parent module doc): the per-frame push from the
//! self player's descriptor into the VM's `ContainerState`/`ContainerSlot` snapshots, plus the
//! shared item-tooltip stat feed the bags/paper-doll/etc. all read from.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::ItemInfo;
use benilla_ui::script::{ContainerSlot, ContainerState, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::pending_item_ops::{LockTransitions, PendingItemOps};
use crate::ui_script::gate;

use super::equip_error::equip_error_key;
use super::{
    find_equip_slot, has_key, keyring_size, slot_guid_count, EquipErrors, BAGS, BAG_SLOT_FIRST,
    BANK_BAGS, BANK_BAG_ID_FIRST, BANK_BAG_SLOT_FIRST, BANK_CONTAINER, BANK_SLOTS,
    KEYRING_CONTAINER, KEYRING_SLOTS, PACK_SLOTS,
};

/// The feed's memory of what it last pushed, for per-bag change events. No cooldown-churn gate:
/// the pushed triple carries the ABSOLUTE start, which is frame-stable for a running cooldown.
#[derive(Default)]
pub(crate) struct FeedMemory {
    pushed: HashMap<i64, ContainerState>,
    /// The last `HasKey()` pushed — kept only so the transition can be logged once instead of
    /// every frame. It is the gate the entire keyring UI hangs off, and "the keyring never
    /// appeared" is otherwise indistinguishable from "the button is mis-anchored".
    had_key: bool,
    /// The gate's counter memories (1439) — the stores whose lazy `&mut` resolves poison
    /// `is_changed` for this feed, watched by their explicit counters instead. The item OBJECTS
    /// left this list with 2334: an instance is an entity, so its edges come off the ordinary
    /// `Changed`/removal watch ([`crate::items::ItemChanges`]).
    items_templates: gate::Watch,
    cooldown_epoch: gate::Watch,
    names_generation: gate::Watch,
    /// `PetitionState::records_epoch` — one step per landed or patched petition record, which is
    /// what a charter slot's tooltip lines are built from. Lazy, so its arrival moves nothing else
    /// here (the same reason `names_generation` exists one line up).
    petition_records: gate::Watch,
    /// `ItemChanges::countdown_steps` — one step per displayable countdown change (the slot
    /// views read second-floored countdowns), including the last elapse's collapsing push; a
    /// landing is the item's own change tick (decision 2340).
    enchant_deadlines: gate::Watch,
    /// The item guid **of the bag itself** in each of the ten bag slots — container ids 1..=10
    /// (the four equipped, then the six bank bags), indexed `id − 1`. Diffed frame to frame for
    /// `BAG_CLOSED`, which is a fact about the BAG, not about its contents, and which the pushed
    /// [`ContainerState`] therefore cannot answer: two different empty 16-slot bags swapped
    /// between two slots produce identical container states and must still close both windows.
    bag_guids: [u64; 10],
    /// The item guid in each of the **24 vault slots** (absolute player slots 39..62), indexed
    /// `slot − 1`. Diffed frame to frame to tell `PLAYERBANKSLOTS_CHANGED`'s two producers apart —
    /// see [`SlotGuids`] and the fire site below. Same reason [`Self::bag_guids`] exists: the
    /// pushed [`ContainerState`] cannot answer a question about item IDENTITY, because two
    /// different instances of one template push the same slot view.
    vault_guids: [u64; BANK_SLOTS as usize],
}

/// The player descriptor's own item-slot guids, as the reference's watchers read them — the ten
/// bag slots and the 24 vault slots, in one value because they are one registration family
/// (`0x5dd8a0` installs `0x5ddcf0` over four bands of `PLAYER_FIELD_*_SLOT_*` with `len = 8`, the
/// guid's width).
///
/// These ride beside the pushed containers rather than inside them because they are read off the
/// player descriptor, not built from the containers — and because they answer what a derived view
/// cannot: *which item*, not *what it looks like*.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct SlotGuids {
    /// Container ids 1..=10 — the four equipped bag slots, then the six bank bag slots — indexed
    /// `id − 1`. `BAG_CLOSED`'s diff (decision 1777).
    pub bags: [u64; 10],
    /// The 24 vault slots, indexed `slot − 1`. `PLAYERBANKSLOTS_CHANGED`'s producer discriminator.
    pub vault: [u64; BANK_SLOTS as usize],
}

/// A spell's tooltip text: the $-substituted description (the real "Use: Restores 392 to 653
/// health over 21 sec" shape). Shared by the item trigger lines and the set-bonus lines.
///
/// **An empty description yields `None`, and the caller drops the whole line** — byte-verified at
/// the item builder `0x52d8a0`: it runs the $-expander into a buffer (`0x52da24` → `0x5075f0`),
/// tests the FIRST BYTE of the result (`0x52da29 mov al,[ebp-0x4f0]` / `0x52da2f test al,al`) and
/// on zero **jumps clean past the entire trigger block** (`0x52da31 je 0x52dd3d`) — past the
/// ONUSE/ONEQUIP/ONPROC prefix select and the `"%s %s"` join alike. No prefix, no line.
///
/// This used to fall back to the spell's bare NAME, which is where "Use: Opening" on a dungeon key
/// came from (director-reported): `Opening` (3365/3366/6247/6477) carries no description in
/// Spell.dbc, so the real client prints nothing and we printed the name. The fallback was an
/// invention, not a transcription — no such branch exists in the builder.
fn spell_desc_text(
    spells: Option<&crate::ui_action::Spells>,
    id: u32,
    home_area: Option<&str>,
    // The `$d`/`$s` tokens resolve `INT_SPELL_DURATION_*` / `INT_SPELL_POINTS_SPREAD_TEMPLATE`
    // out of the VM (decision 2045); `benilla-formats` names the key, this side renders it.
    text: &dyn Fn(&str, &[i64]) -> Option<String>,
) -> Option<String> {
    let sp = spells?;
    let d = sp.catalog.get(id)?;
    match d.description.as_deref().filter(|t| !t.is_empty()) {
        // The $-expander can still yield an empty string from a non-empty template; the builder's
        // test is on the EXPANDED text, so ours is too.
        Some(desc) => {
            let ctx = benilla_formats::TokenContext {
                durations: &sp.durations,
                radii: &sp.radii,
                lookup: &|i| sp.catalog.get(i),
                home_area,
                text,
            };
            Some(benilla_formats::substitute(desc, d, &ctx)).filter(|t| !t.is_empty())
        }
        None => None,
    }
}

/// The tooltip's "N Charge(s)" count — 0 = no line. The real builder's charge gate
/// (byte-VERIFIED, tooltip builder `0x52d8a0`): per spell slot it normalizes a template charge
/// value of 0 to the `-1` sentinel (`0x52da01-0x52da0d`), skips the ITEM_SPELL_CHARGES line
/// entirely when the resolved value is `-1` (`0x52db51-54`), and otherwise prints `abs(value)`
/// (the cdq/xor/sub at `0x52db56-5b`). So a consumable's `-1` — "the item IS the last charge":
/// food, water, potions — shows NO line, while a real charge pool (a wand-style `-5`) shows
/// "5 Charges". We emit one line for the first surviving slot — the builder prints per slot,
/// but no item in the data carries two charge pools (VERIFIED against the live vmangos
/// `item_template`: 111 rows with a pool, none with a second).
fn charges_count(spells: &[benilla_protocol::messages::ItemSpellEntry]) -> i32 {
    spells
        .iter()
        .find(|s| s.spell_id != 0 && s.charges != 0 && s.charges != -1)
        .map(|s| s.charges.abs())
        .unwrap_or(0)
}

/// A reputation rank (0..=7) → its standing label, `FACTION_STANDING_LABEL{rank+1}` off the
/// player's own `GlobalStrings.lua` — the tail of the `ITEM_REQ_REPUTATION` line.
///
/// Keys rather than an eight-string table (decision 2045): the bare tokens, not the `_FEMALE`
/// twins, which is the spelling the reference itself uses wherever it builds a standing label
/// (`FACTION_STANDING_LABEL%d` `0x84b5dc`).
/// `None` for an install that does not carry the row — the line then names no standing.
fn standing_label(rank: u32, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    get(&format!("FACTION_STANDING_LABEL{}", rank.min(7) + 1))
}

/// Build an item template's tooltip view (decision 0274 P1): the full wire fields plus the
/// display strings only the app can resolve — trigger-spell TEXT off the spell catalog (the
/// spell's $-substituted description when it has one, per the verified law's item `$`-expander
/// `0x506f70`; its bare name otherwise), the skill requirement's name off `SkillLine.dbc`, the
/// reputation requirement off `Faction.dbc` names (the red check is the engine's, against the
/// player's rank map).
fn template_view(
    t: &ItemInfo,
    spells: Option<&crate::ui_action::Spells>,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    home_area: Option<&str>,
    factions: Option<&benilla_formats::FactionCatalog>,
    sub_classes: Option<&benilla_formats::ItemSubClassCatalog>,
    classes: Option<&benilla_formats::ItemClassCatalog>,
    icons: Option<&ItemDisplays>,
    // The VM's own `GlobalStrings.lua`, for the reputation-requirement line (decision 2045).
    get: &dyn Fn(&str) -> Option<String>,
    // The same table, for the `$`-engine's own keyed tokens — see [`spell_desc_text`].
    text: &dyn Fn(&str, &[i64]) -> Option<String>,
) -> benilla_ui::script::ItemTemplateView {
    let spell_name = |id: u32| -> Option<String> {
        spells
            .and_then(|s| s.catalog.get(id))
            .map(|sd| sd.name.clone())
    };
    let spell_text = |id: u32| spell_desc_text(spells, id, home_area, text);
    benilla_ui::script::ItemTemplateView {
        name: t.name.clone(),
        quality: t.quality,
        class: t.class,
        subclass: t.subclass,
        inventory_type: t.inventory_type,
        proficiency_alt: sub_classes.and_then(|c| c.proficiency_alt(t.class, t.subclass)),
        hide_subclass: sub_classes.is_some_and(|c| c.hides_name(t.class, t.subclass)),
        // `GetItemInfo`'s type pair. The subclass spelling is `name()` — VerboseName first —
        // because that is the binding's own two-step (`0x48e311`), and it is deliberately NOT the
        // `display_name()` the tooltip's type cell prints: a one-handed sword is "One-Handed
        // Swords" to an addon and "Sword" on the tooltip. Both spellings are in the same row.
        item_type: classes.and_then(|c| c.name(t.class)).map(str::to_string),
        item_sub_type: sub_classes
            .and_then(|c| c.name(t.class, t.subclass))
            .map(str::to_string),
        // …and the tooltip's own spelling of the same row — DisplayName alone.
        sub_class_display: sub_classes
            .and_then(|c| c.display_name(t.class, t.subclass))
            .map(str::to_string),
        flags: t.flags,
        bonding: t.bonding,
        max_count: t.max_count,
        // The stack size — a DIFFERENT wire field from `max_count` (the account-wide unique cap).
        // Linen Cloth is `max_count 0, stackable 20`; `GetItemInfo`'s `itemStackCount` is this one.
        stackable: t.stackable,
        // The same `ItemDisplayInfo.dbc` icon the bag slots resolve, as a ready
        // `Interface\Icons\…` path — `GetItemInfo`'s `itemTexture`.
        icon: icons
            .and_then(|i| i.catalog.get(t.display_info_id))
            .and_then(|d| d.icon.clone()),
        start_quest: t.start_quest,
        container_slots: t.container_slots,
        stats: t.stats.clone(),
        damages: t.damages.iter().map(|d| (d.min, d.max, d.school)).collect(),
        delay_ms: t.delay_ms,
        armor: t.armor,
        block: t.block,
        resistances: t.resistances,
        max_durability: t.max_durability,
        required_level: t.required_level,
        allowable_class: t.allowable_class,
        allowable_race: t.allowable_race,
        required_skill: t.required_skill,
        required_skill_rank: t.required_skill_rank,
        required_skill_name: (t.required_skill != 0)
            .then(|| {
                skill_lines
                    .and_then(|sl| sl.line(t.required_skill))
                    .map(|l| l.name.clone())
            })
            .flatten(),
        required_spell: t.required_spell,
        required_spell_name: (t.required_spell != 0)
            .then(|| spell_name(t.required_spell))
            .flatten(),
        required_honor_rank: t.required_honor_rank,
        required_city_rank: t.required_city_rank,
        // `ITEM_REQ_REPUTATION` ("%s - %s" after its own "Requires ") — `0x854b8c`, the key the
        // reference's own item tooltip builds this line with. Both holes are
        // filled in the template's order; an install missing either key shows no line.
        required_rep_line: (t.required_rep_faction != 0)
            .then(|| {
                let faction = factions.and_then(|c| c.faction_name(t.required_rep_faction))?;
                let standing = standing_label(t.required_rep_rank, get)?;
                Some(benilla_ui::strings::fill(
                    &get("ITEM_REQ_REPUTATION")?,
                    &[
                        benilla_ui::strings::Arg::S(faction),
                        benilla_ui::strings::Arg::S(&standing),
                    ],
                ))
            })
            .flatten(),
        required_rep_faction: t.required_rep_faction,
        required_rep_rank: t.required_rep_rank,
        lock_id: t.lock_id,
        spell_triggers: t
            .spells
            .iter()
            .filter(|s| s.spell_id != 0)
            .filter_map(|s| spell_text(s.spell_id).map(|n| (s.trigger, s.spell_id, n)))
            .collect(),
        charges: charges_count(&t.spells),
        description: t.description.clone(),
        page_text: t.page_text,
        sell_price: t.sell_price,
        item_set: t.item_set,
        random_property: t.random_property,
    }
}

/// The item SET-block feed: answer the engine's ask-once set ids from ItemSet.dbc, joining
/// member NAMES from the template cache (each miss fires its own `CMSG_ITEM_QUERY` through
/// [`Items::template`], like the real client querying set members) and bonus TEXT from the
/// spell catalog's `$`-engine. A set with members still in flight stays pending and re-pushes
/// as answers land; it leaves the pending map once every member resolved. (`TokenContext.home_
/// area` is `None` here: set bonuses are equip auras — no `$z`; the passthrough would surface
/// one if the data ever carried it.)
pub(super) fn feed_item_sets(
    script: Option<NonSendMut<UiScript>>,
    sets: Option<Res<super::ItemSets>>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    spells: Option<Res<crate::ui_action::Spells>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    mut pending: Local<
        crate::ui_script::VmMemo<std::collections::HashMap<u32, benilla_ui::script::ItemSetView>>,
    >,
) {
    let Some(mut script) = script else {
        return;
    };
    let pending = pending.get(&script);
    for id in script.take_item_set_asks() {
        pending.entry(id).or_default();
    }
    if pending.is_empty() {
        return;
    }
    let Some(sets) = sets.as_deref() else {
        return; // catalog absent (no client data): the asks stay parked, the block stays off
    };
    let spell_res = spells.as_deref();
    let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
    let mut done: Vec<u32> = Vec::new();
    let mut push: Vec<(u32, benilla_ui::script::ItemSetView)> = Vec::new();
    // Scoped, so the push below can take the VM mutably: the build reads the string table, the
    // push writes the store, and the two cannot hold it at once.
    let token_text = crate::ui_script::token_text(&script);
    for (&set_id, last) in pending.iter_mut() {
        let Some(row) = sets.0.set(set_id) else {
            done.push(set_id); // no such row — drop the ask for good
            continue;
        };
        let members: Vec<(u32, Option<String>)> = row
            .items
            .iter()
            .map(|&id| (id, items.template(id, 0, &commands).map(|t| t.name.clone())))
            .collect();
        let view = benilla_ui::script::ItemSetView {
            name: row.name.clone(),
            bonuses: row
                .bonuses
                .iter()
                .filter_map(|&(n, spell)| {
                    spell_desc_text(spell_res, spell, None, &token_text).map(|desc| (n, desc))
                })
                .collect(),
            required_skill: row.required_skill,
            required_skill_rank: row.required_skill_rank,
            required_skill_name: (row.required_skill != 0)
                .then(|| {
                    skill_catalog
                        .and_then(|sl| sl.line(row.required_skill))
                        .map(|l| l.name.clone())
                })
                .flatten(),
            members,
        };
        if view.members.iter().all(|(_, n)| n.is_some()) {
            done.push(set_id);
        }
        if *last != view {
            *last = view.clone();
            push.push((set_id, view));
        }
    }
    drop(token_text);
    for (id, view) in push {
        script.set_item_set(id, view);
    }
    for id in done {
        pending.remove(&id);
    }
}

/// The shared item-tooltip feed, both halves. **Push**: every template that lands in the app
/// cache goes to the UI store unprompted (`Items::take_fresh`) — so by the time an item's name is
/// on screen its tooltip stats are already there, and the first hover never misses (the real
/// client reads one item cache synchronously; a store the UI only fills on a read miss re-created
/// the "hover twice" flake this replaces). **Ask**: a renderer read of an id the app never
/// resolved still records a miss, which triggers the `CMSG_ITEM_QUERY` here and lands via the
/// same push when the answer arrives.
/// Push the **whole** random-suffix table into the engine, once per VM (decision 1547).
///
/// Not an ask-once feed like the templates beside it: the roll table is a static DBC the app holds
/// from load, and its consumers are click-driven — a chat-link tooltip has no hover re-enter loop
/// to repaint on a late answer. One push per VM (a `/reload` mints a new one), and it waits for
/// both catalogs, so a session that starts before the DBC load simply pushes on the next frame.
pub(super) fn feed_random_properties(
    script: Option<NonSendMut<UiScript>>,
    props: Option<Res<crate::items::RandomProperties>>,
    enchants: Option<Res<crate::items::Enchants>>,
    mut pushed: Local<crate::ui_script::VmMemo<bool>>,
) {
    let Some(mut script) = script else {
        return;
    };
    if *pushed.get(&script) {
        return;
    }
    // The enchant catalog is what turns the roll's five ids into lines; without it every row would
    // push empty and the push would never be retried. Both, or neither.
    let (Some(props), Some(enchants)) = (props, enchants) else {
        return;
    };
    let rows = crate::items::random_property_views(&props, Some(&enchants));
    let with_lines = rows.values().filter(|v| !v.enchants.is_empty()).count();
    info!(
        "random-property table: {} rows pushed, {with_lines} with enchant lines",
        rows.len()
    );
    script.set_random_properties(rows);
    *pushed.get(&script) = true;
}

pub(super) fn feed_item_stats(
    script: Option<NonSendMut<UiScript>>,
    mut items: ResMut<Items>,
    commands: Res<NetCommands>,
    spells: Option<Res<crate::ui_action::Spells>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    // The $z token's inputs: the bind-point area id (SMSG_BINDPOINTUPDATE) named through the
    // AreaTable catalog the quest-log/zone-text already carry.
    home_bind: Option<Res<crate::net::HomeBind>>,
    area_names: Option<Res<crate::ui_quest_log::QuestHeaderNamesRes>>,
    factions: Option<Res<crate::target::Factions>>,
    sub_classes: Option<Res<super::ItemSubClasses>>,
    // `GetItemInfo`'s `itemType` and `itemTexture`: the class-name table, and the same
    // ItemDisplayInfo icons the bag slots already resolve through.
    classes: Option<Res<super::ItemClasses>>,
    icons: Option<Res<ItemDisplays>>,
    mut pending: Local<crate::ui_script::VmMemo<std::collections::HashSet<u32>>>,
    mut last_home: Local<crate::ui_script::VmMemo<Option<String>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let pending = pending.get(&script);
    let last_home = last_home.get(&script);
    // **`GetBindLocation()`'s push, and it happens BEFORE the early return below.**
    //
    // The name resolution lives here because this system already owns it for the hearthstone's
    // `$z` token, and a second AreaTable lookup elsewhere is the two-parallel-paths drift that has
    // cost this codebase real bugs — the binding and the token must never disagree about where the
    // player is bound. But the item feed idles whenever nothing is pending, and the bind point
    // arrives long after world entry (`SMSG_BINDPOINTUPDATE` at login and on every re-bind), so the
    // push cannot sit behind that gate.
    let home_area: Option<&str> = home_bind
        .as_deref()
        .and_then(|b| b.0)
        .and_then(|id| area_names.as_deref()?.0.resolve(id as i32));
    // …pushed on CHANGE, per VM: the memo resets with the VM, so a rebuilt one is fed again
    // the frame it appears. It used to push every frame — with a fresh String each time —
    // because it cannot sit behind the pending gate below; the memo compare is the gate it can
    // sit behind, and the re-substitute rides the same edge.
    if last_home.as_deref() != home_area {
        script.set_bind_location(home_area.unwrap_or_default());
        *last_home = home_area.map(str::to_string);
        // A bind-point change re-substitutes every held view: templates pushed before the
        // login's SMSG_BINDPOINTUPDATE landed carry a raw $z otherwise (the hearthstone's login
        // race).
        pending.extend(items.cached_template_ids());
    }

    pending.extend(items.take_fresh());
    pending.extend(script.take_item_stat_asks());
    if pending.is_empty() {
        return;
    }
    let spell_res = spells.as_deref();
    let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
    let ready: Vec<u32> = pending
        .iter()
        .copied()
        .filter(|&id| items.template(id, 0, &commands).is_some())
        .collect();
    // Built first, pushed after: the reference-string lookup borrows the VM and `set_item_template`
    // needs it mutably, so the resolve and the push cannot interleave.
    let views: Vec<(u32, benilla_ui::script::ItemTemplateView)> = {
        let get = |key: &str| {
            script
                .lua()
                .globals()
                .get::<String>(key)
                .ok()
                .filter(|t| !t.is_empty())
        };
        ready
            .into_iter()
            .filter_map(|id| {
                pending.remove(&id);
                let t = items.template(id, 0, &commands)?.clone();
                Some((
                    id,
                    template_view(
                        &t,
                        spell_res,
                        skill_catalog,
                        home_area,
                        factions.as_deref().map(|f| f.catalog()),
                        sub_classes.as_deref().map(|s| &s.0),
                        classes.as_deref().map(|c| &c.0),
                        icons.as_deref(),
                        &get,
                        &crate::ui_script::token_text(&script),
                    ),
                ))
            })
            .collect()
    };
    for (id, view) in views {
        script.set_item_template(id, view);
    }
}

/// The red-line law's player state (decision 0274 P1): level + class/race ids (the allowable-mask
/// bits) + the full skill-rank map, read off the self player's descriptor, plus the equip
/// proficiencies (`SMSG_SET_PROFICIENCY`) and the faction → reputation-rank map (DBC base for our
/// race/class + the `SMSG_INITIALIZE_FACTIONS` standing, ranked) — pushed on change.
pub(super) fn feed_player_req(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    changed_self: Query<(), (With<SelfPlayer>, Changed<ObjectStore>)>,
    proficiencies: Res<crate::net::Proficiencies>,
    reputations: Res<crate::net::Reputations>,
    factions: Option<Res<crate::target::Factions>>,
    actions: Res<crate::ui_action::PlayerActions>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut last: Local<crate::ui_script::VmMemo<Option<benilla_ui::script::PlayerReqState>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (last, vm_reset) = last.get_reset(&script);
    // The gate (1439): the state below is a pure function of these six — the self descriptor
    // (skills/level/race/class/honor; its absence returns pushless), the proficiency and
    // reputation mirrors, the faction catalog, and the known-spell set with its catalog.
    let self_changed = !changed_self.is_empty();
    let prof_changed = proficiencies.is_changed();
    let reps_changed = reputations.is_changed();
    let factions_changed = factions.as_ref().is_some_and(|r| r.is_changed());
    let actions_changed = actions.is_changed();
    let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
    gate::trace(
        "feed_player_req",
        &[
            ("vm_reset", vm_reset),
            ("self", self_changed),
            ("proficiencies", prof_changed),
            ("reputations", reps_changed),
            ("factions", factions_changed),
            ("actions", actions_changed),
            ("spells", spells_changed),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || self_changed
            || prof_changed
            || reps_changed
            || factions_changed
            || actions_changed
            || spells_changed,
    );
    if gate.skip() {
        return;
    }
    let Some(store) = self_q.iter().next() else {
        return;
    };
    let mut skills = std::collections::HashMap::new();
    for slot in 0..benilla_protocol::messages::PLAYER_SKILL_SLOTS {
        let Some(s) = store.0.player_skill(slot) else {
            continue;
        };
        if s.skill_id != 0 {
            // The gate laws (`0x5eaae0`, `0x5ea930`) read value + PERM bonus (the talent half;
            // the temp half never counts toward requirements). The client zero-extends the bonus
            // word before a signed compare — a negative perm bonus can't happen in live data, so
            // the sane saturating add mirrors every reachable case.
            skills.insert(
                u32::from(s.skill_id),
                u32::from(s.value).saturating_add_signed(i32::from(s.perm_bonus)),
            );
        }
    }
    let race = store.0.unit_race().unwrap_or(0);
    let class = store.0.unit_class().unwrap_or(0);
    let mut rep_ranks = std::collections::HashMap::new();
    if let Some(cat) = factions.as_deref().map(|f| f.catalog()) {
        for (id, info) in cat.reputation_factions() {
            let standing = usize::try_from(info.rep_index)
                .ok()
                .and_then(|i| reputations.0.get(i))
                .map(|&(_, s)| s)
                .unwrap_or(0);
            rep_ranks.insert(
                id,
                benilla_formats::reputation_rank(info.base_for(race, class) + standing),
            );
        }
    }
    // The client's `0xc4d770`: set when a spell whose Effect[0] is 40 (SPELL_EFFECT_DUAL_WIELD)
    // is learned, cleared on its unlearn — mirrored as a spellbook scan.
    let can_dual_wield = spells.as_deref().is_some_and(|s| {
        actions
            .spells
            .iter()
            .any(|&id| s.catalog.get(id).is_some_and(|sd| sd.effects[0] == 40))
    });
    let state = benilla_ui::script::PlayerReqState {
        level: store.0.unit_level().unwrap_or(0),
        class_id: u32::from(class),
        race_id: u32::from(race),
        skills,
        proficiency: proficiencies.0.clone(),
        rep_ranks,
        can_dual_wield,
        honor_rank: store.0.player_honor_rank().unwrap_or(0),
    };
    if last.as_ref() != Some(&state) {
        gate.audit("feed_player_req", "the player-requirement state");
        *last = Some(state.clone());
        script.set_player_req_state(state);
    }
}

/// One bag slot from its item guid: the instance (store) + template (cache, ask-once) + icon
/// (DBC) + the use-spell's running cooldown (`GetContainerItemCooldown`'s data — the same store
/// read the action feed's ITEM arm does). `None` = the slot is empty (guid 0/unsent) — an
/// *unresolved* occupied slot is `Some` with empty fields instead, so the bag shows the item
/// exists before its query answers.
fn resolve_slot(
    guid: u64,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    cooldowns: &crate::spell::Cooldowns,
    spells: Option<&benilla_formats::SpellCatalog>,
    names: &crate::names::NameCache,
    // The petition record cache, for a charter slot's tooltip lines — a LAZY fill, so it is taken
    // mutably for the same reason `names` is: the read is what issues the query.
    petitions: &mut crate::ui_petition::PetitionState,
    now: std::time::Instant,
    ui_now: f64,
    // The player's own `ChrClasses.dbc` relic flag — `find_equip_slot`'s second half. It decides
    // ONE slot (INVSLOT 17) and it decides it both ways, so it has to reach every fit-rule read
    // rather than being applied at one of them (1803).
    has_relic_slot: bool,
) -> Option<ContainerSlot> {
    if guid == 0 {
        return None;
    }
    // The item's own countdown cells (decision 2340): one `Option<ms>` per temporary-enchant
    // slot, and its lifetime — `SMSG_ITEM_TIME_UPDATE`'s only surface (decision 1933) — both
    // second-floored for the snapshot.
    let countdowns = objects.countdowns(guid);
    let enchant_ms: [Option<u64>; crate::items::ENCHANT_SLOTS] =
        std::array::from_fn(|s| countdowns.and_then(|c| c.enchant_remaining_display_ms(s as u32)));
    let duration_ms = countdowns.and_then(|c| c.lifetime_remaining_display_ms());
    let (entry, count, durability, readable, creator, flags, already_bound, roll, enchant_lines) =
        match objects.object(guid) {
            Some(fields) => (
                fields.object_entry().unwrap_or(0),
                fields.item_stack_count().unwrap_or(1),
                // The live instance pair — the wire updates ITEM_FIELD_DURABILITY on damage/repair
                // (death 10%, spirit healer 25%); max 0 = indestructible ⇒ no line.
                fields
                    .item_durability()
                    .zip(fields.item_max_durability())
                    .filter(|&(_, max)| max > 0),
                // The instance carries letter text (a mail permanent copy) — the same gate the
                // right-click reader fork uses (`ui_items::drain`); the hover magnifier keys off it.
                // Template-`PageText` books join when the books read path lands (decision 0572).
                fields.item_text_id().is_some_and(|id| id != 0),
                // `ITEM_FIELD_CREATOR` → the ask-once name cache: the tooltip's "Written by %s" /
                // "<Made by %s>" line. `None` while the query is in flight — the changed re-push
                // repaints the hover when the answer lands (the ref's cache-callback shape).
                fields
                    .item_creator()
                    .filter(|&g| g != 0)
                    .and_then(|g| names.resolve(g, commands).map(str::to_string)),
                // `ITEM_FIELD_FLAGS` — the tooltip's UNLOCKED (0x4) / WRAPPED (0x8) sub-gates.
                fields.item_flags().unwrap_or(0),
                // `0x5da2c0` — soulbound, or carrying a binding enchant: the Soulbound
                // override (B310). Read off the raw descriptor, not off the enchant LINES below.
                crate::items::already_bound(fields, rolls.enchants),
                // `ITEM_FIELD_RANDOM_PROPERTIES_ID` — the roll behind the NAME's "of the Bear"
                // (decision 1547). Only the name: the roll's own enchants are already in the
                // instance's slots 2..6 below, written there by the server.
                fields.item_random_properties_id(),
                // The instance's own 7 enchant slots — the tooltip's enchant lines (0915/0920). An
                // item we hold streams as an OBJECT, so all seven are here, with their charges and
                // their `SMSG_ITEM_ENCHANT_TIME_UPDATE` countdowns; the wire's 2-slot broadcast is
                // what everyone ELSE's items are limited to.
                crate::items::enchant_lines(
                    (0..7).map(|s| {
                        (
                            s,
                            fields.item_enchant(s).unwrap_or(0),
                            fields.item_enchant_charges(s),
                            enchant_ms[usize::from(s)],
                        )
                    }),
                    rolls.enchants,
                ),
            ),
            // The player descriptor references a guid whose create hasn't landed (yet) —
            // occupied, unresolved.
            None => return Some(ContainerSlot::default()),
        };
    if entry == 0 {
        return Some(ContainerSlot::default());
    }
    let template: Option<ItemInfo> = items.template(entry, guid, commands).cloned();
    let Some(t) = template else {
        // Asked (or a cached negative); show the slot occupied while the answer is in flight.
        return Some(ContainerSlot {
            durability: None,
            item_id: entry,
            count,
            readable,
            creator,
            flags,
            already_bound,
            enchants: enchant_lines,
            duration_ms,
            ..Default::default()
        });
    };
    Some(ContainerSlot {
        // **Line 3 of the tooltip law** — a charter's guild name and master. Gated on the template's
        // signable flag, then keyed by the petition id the server stuffs into the item's
        // `ITEM_FIELD_ENCHANTMENT` slot 0 (`PetitionsHandler.cpp:126`) — which is the same field the
        // client reads it from (`0x5ef337`), and the same one the tooltip's ENCHANT lines are
        // forced to skip for exactly this reason (`0x52c9e0 test ah,0x20`). So slot 0 on a charter
        // is a petition id by contract on both ends, and this is its one consumer here.
        //
        // The lookup is lazy: hovering an unopened charter is what sends the query, so the first
        // hover shows the name and the green line alone. The creator line right below has the same
        // rule.
        petition: (t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0)
            .then(|| {
                let id = objects
                    .object(guid)
                    .and_then(|f| f.item_enchant(0))
                    .unwrap_or(0);
                petitions.tooltip_view(id as u32, guid, names, commands)
            })
            .flatten(),
        texture: icons
            .and_then(|i| i.catalog.get(t.display_info_id))
            .and_then(|d| d.icon.clone()),
        count,
        durability,
        quality: Some(t.quality),
        item_id: entry,
        // The link carries the roll — both in its `randomPropertyId` field and in the bracketed
        // NAME, which the reference builds out of the suffix-joining formatter `0x5d8b00`. The
        // slot's tooltip plate reads its name back off this string.
        link: Some(crate::ui_items::item_link_full(
            entry,
            0,
            roll,
            0,
            &rolls.name(&t.name, roll),
            t.quality,
        )),
        locked: false,
        readable,
        creator,
        flags,
        already_bound,
        enchants: enchant_lines,
        duration_ms,
        equip_slots: find_equip_slot(t.inventory_type, has_relic_slot),
        bar_placeable: t.placeable_on_action_bar(),
        cooldown: t.use_spell.and_then(|u| {
            let sd = spells.and_then(|s| s.get(u.spell_id));
            cooldowns
                .info(u.spell_id, entry, sd, now)
                .ui_triple(now, ui_now)
        }),
    })
}

/// Reason 16's `%s`: the destination bag's **`BagFamily` name** — "Arrows", "Soul Shards",
/// "Herbs" — for *"Only %s can be placed in that."* `None` = no bag to name, so the caller keeps
/// the generic `ERR_WRONG_BAG_TYPE` line.
///
/// `bag_slot` is the wire's ABSOLUTE player slot (see
/// `benilla_protocol::messages::items::read_inventory_change_failure`), and the reference's helper
/// `0x5ede00` bails on exactly two shapes we mirror: `slot == 0xFF` (`INVENTORY_SLOT_BAG_0`, the
/// player's own array — a backpack/equipment refusal names no container), and a slot past the
/// player's slot array. Everything else it indexes straight into that array and resolves as an
/// item.
///
/// Despite the errorId's `_SUBCLASS` name this reads the bag's `BagFamily`, **not** its
/// ItemSubClass — which is what makes a quiver say "Only Arrows can be placed in that." rather
/// than naming the quiver's own type (`benilla_formats::itembagfamily`, and the DBC read there).
///
/// The bank-bag leg (63..=68) is ours by symmetry rather than byte-pinned: the reference bounds
/// this on `[player+0x1d38]`, whose value is unresolved, so whether a bank bag reaches
/// the substitution or falls to the generic line is unverified. Both outcomes are ordinary
/// sentences; resolving it is the strictly more useful one, and it is flagged here rather than
/// silently assumed.
fn bag_family_name(
    player: Option<&ObjectStore>,
    bag_slot: u8,
    objects: &Objects,
    items: &Items,
    families: Option<&benilla_formats::ItemBagFamilyCatalog>,
    commands: &NetCommands,
) -> Option<String> {
    let store = player?;
    let families = families?;
    let guid = match bag_slot {
        // The equipped bag slots.
        s if (BAG_SLOT_FIRST..BAG_SLOT_FIRST + BAGS).contains(&s) => store.0.player_inv_slot(s),
        // The purchasable bank bag slots.
        s if (BANK_BAG_SLOT_FIRST..BANK_BAG_SLOT_FIRST + BANK_BAGS).contains(&s) => {
            store.0.player_bank_bag_slot(s - BANK_BAG_SLOT_FIRST)
        }
        // 255 = the player's own array, and anything past the bag slots: no container to name.
        _ => None,
    }
    .filter(|&g| g != 0)?;
    let entry = objects.object(guid)?.object_entry().filter(|&e| e != 0)?;
    let family = items.template(entry, guid, commands)?.bag_family;
    families.name(family).map(str::to_string)
}

/// The pending-lock **resolving clear** (decision 0216 §4 / 0218 §3 "the field-update watcher"):
/// release any outstanding op whose slots have moved on, and queue what unlocked for
/// [`feed_containers`] to announce with `ITEM_LOCK_CHANGED`.
///
/// **Its own system, ordered ahead of BOTH feeds** (1771). It used to live inside
/// `feed_containers`, which is ordered *after* `feed_char` — so `feed_char`, which owns the
/// doll's and the bank bags' `locked`, read a lock set that the same frame was about to clear,
/// pushed `locked: true`, and then went quiet: its gate closed the next frame (nothing in flight,
/// no object moved) and the stale lock stood until something else forced a repush. On the bank's
/// six bag buttons — whose only repaint signals are `PLAYERBANKSLOTS_CHANGED` and
/// `BANKFRAME_OPENED` — that showed as a bag greyed out until the window was reopened.
///
/// Resolve first, then push, then fire: the house rule, now actually enforceable across the two
/// feeds because neither of them owns the clear any more.
pub(crate) fn resolve_item_locks(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    mut pending: ResMut<PendingItemOps>,
    mut transitions: ResMut<LockTransitions>,
) {
    if pending.is_empty() {
        return;
    }
    let player = self_q.iter().next();
    if player.is_none() {
        return;
    }
    transitions
        .0
        .extend(pending.resolve(|bag, slot1| slot_guid_count(player, bag, slot1, &objects)));
}

#[allow(clippy::type_complexity)] // the param list IS the input set
pub(crate) fn feed_containers(
    script: Option<NonSendMut<UiScript>>,
    // `ChrClasses.dbc` field 16, for the fit rule's relic half — see `find_equip_slot` (1803).
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    // The two item-DBC catalogs, as one param (the 16-SystemParam ceiling): `SpellItemEnchantment`'s
    // name column — the tooltip's enchant lines (decision 0915) — and `ItemRandomProperties`, the
    // roll behind a slot's "of the Monkey" name (decision 1547).
    catalogs: (
        Option<Res<crate::items::Enchants>>,
        Option<Res<crate::items::RandomProperties>>,
    ),
    // The inventory read (2334): the self store the slot arrays come off and its change tick —
    // the gate's cheapest input (1439) — the object lookup the guids resolve through, and the
    // item entities' own change watch.
    mut inv: crate::items::Inventory,
    commands: Res<NetCommands>,
    cooldowns: Res<crate::spell::Cooldowns>,
    spells: Option<Res<crate::ui_action::Spells>>,
    // The refusals and where they land, as one param (the 16-SystemParam ceiling this signature
    // already sits at): the queue, and `show_messages`' chat + sound sinks (1815).
    equip: (ResMut<EquipErrors>, crate::ui_action::MessageSink),
    // Reason 16's `%s` source (decision 0916); absent = every 16 keeps the generic line.
    bag_families: Option<Res<crate::ui_items::ItemBagFamilies>>,
    pending: Res<PendingItemOps>,
    // The two handler-fed queues this feed fires for (the 16-SystemParam ceiling): the lock
    // transitions (`ITEM_LOCK_CHANGED`) and the server-opened containers (`BAG_OPEN`, 2339).
    queues: (ResMut<LockTransitions>, ResMut<super::net::BagOpens>),
    names: Res<crate::names::NameCache>,
    // Paired into one param (the 16-SystemParam ceiling this signature already sits at): the UI
    // clock, and the petition record cache a charter slot's tooltip lines read — `ResMut` because
    // that cache is LAZY and the hover is what issues its query.
    clock_and_petitions: (
        Res<crate::ui_script::UiClock>,
        ResMut<crate::ui_petition::PetitionState>,
    ),
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (clock, mut petitions) = clock_and_petitions;
    let (mut equip_errors, mut sink) = equip;
    let (mut lock_cleared, mut opens) = queues;
    let (memory, vm_reset) = memory.get_reset(&script);
    // The gate (1439): the snapshot below is a function of the self descriptor's slot arrays,
    // the item stores (both epochs — `is_changed` on `Items`/`NameCache`/`Cooldowns` is
    // poisoned by per-frame lazy resolves, so the counters carry the truth), the four
    // catalogs, and the drained side-channels (each held open while non-empty; their
    // *arrival* frames are covered because a drain writer marks them non-empty). `UiClock`
    // is deliberately NOT an input: the pushed cooldown triple carries the ABSOLUTE start
    // (frame-stable, the memory struct's own doc), and natural expiry moves the store's
    // `feed_epoch` through the prune.
    let objects_moved = inv.changes.moved();
    let templates_moved = memory.items_templates.moved(items.template_epoch());
    let cooldowns_moved = memory.cooldown_epoch.moved(cooldowns.feed_epoch());
    let names_moved = memory.names_generation.moved(names.generation());
    let petitions_moved = memory.petition_records.moved(petitions.records_epoch());
    // The DISPLAY epoch (see `feed_char`'s twin comment): the snapshot's countdowns are
    // second-floored, so one watch step per displayable change replaces the per-frame hold-open
    // that rebuilt every bag snapshot for the whole life of a ticking enchant.
    let deadlines_moved = memory
        .enchant_deadlines
        .moved(inv.changes.countdown_steps());
    // Bound as `let`s (not a bare OR-chain) so the gate trace below can name each input.
    let sweep = cooldowns.sweep_pending(clock.anchor);
    let self_changed = !inv.self_changed.is_empty();
    // `is_added`, NOT `is_changed`: the feeds read only the load-once icon CATALOG off this
    // resource (its struct doc says so verbatim); the world's held-model cache half is
    // get-or-insert every frame, so `is_changed` reads true forever — 1439's gate-trace found
    // the containers gate held open by exactly this.
    let icons_changed = icons.as_ref().is_some_and(|r| r.is_added());
    // Both DBC catalogs load once, at startup — one input covers the pair.
    let enchants_changed = catalogs.0.as_ref().is_some_and(|r| r.is_changed())
        || catalogs.1.as_ref().is_some_and(|r| r.is_changed());
    let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
    let families_changed = bag_families.as_ref().is_some_and(|r| r.is_changed());
    let errors_held = !equip_errors.0.is_empty();
    let locks_held = !lock_cleared.0.is_empty() || !pending.is_empty() || !opens.0.is_empty();
    gate::trace(
        "feed_containers",
        &[
            ("vm_reset", vm_reset),
            ("objects", objects_moved),
            ("templates", templates_moved),
            ("cooldowns", cooldowns_moved),
            ("sweep", sweep),
            ("names", names_moved),
            ("petition-records", petitions_moved),
            ("deadlines", deadlines_moved),
            ("self", self_changed),
            ("icons", icons_changed),
            ("enchants", enchants_changed),
            ("spells", spells_changed),
            ("families", families_changed),
            ("errors", errors_held),
            ("locks", locks_held),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || objects_moved
            || templates_moved
            || cooldowns_moved
            // The frame a timer crosses zero, BEFORE the per-frame prune has moved the epoch
            // (`sweep_pending`'s own doc) — the slot triple flips to None right then.
            || sweep
            || names_moved
            || petitions_moved
            || deadlines_moved
            || self_changed
            || icons_changed
            || enchants_changed
            || spells_changed
            || families_changed
            || errors_held
            || locks_held,
    );
    if gate.skip() {
        return;
    }
    // `WOW_FEED_COST=1` — the counter that PRICED gating this feed (the 2026-08-15 ledger's #8,
    // built when the snapshot+diff below still ran every frame; the gate above is 1439's
    // answer). Kept as the gate's wiring instrument: it accumulates only when the body runs, so
    // a parked gated run prints (almost) nothing where the ungated binary printed once a second.
    let cost_t0 = std::env::var_os("WOW_FEED_COST")
        .is_some()
        .then(std::time::Instant::now);
    // The frame's atomic clock pair (`crate::ui_script::UiClock`): the slot resolves read the
    // cooldown store and convert triples through ONE coherent instant, so a running cooldown's
    // pushed start is frame-stable (the resource's own doc).
    let (now, ui_now) = (clock.anchor, clock.ui_now);
    let spell_catalog = spells.as_deref().map(|s| &s.catalog);
    let rolls = crate::items::RollCatalogs {
        enchants: catalogs.0.as_deref(),
        props: catalogs.1.as_deref(),
    };
    // Inventory refusals surface as the client's red error line (the cast path's exact shape):
    // the wire code keys into the VM's own GlobalStrings ([`equip_error_key`], total), reason 1
    // filling its `%d` with the packet's required level.
    //
    // **An unresolvable or empty key prints NOTHING** — this is the reference's own behaviour,
    // not a fallback: `CGGameUI::DisplayError` is called unconditionally and the sink's
    // `cmp byte [ecx],0` guard (`0x4945b4`) drops an empty string before it renders or sounds.
    // It is what silences reason 59, the lock-clear sentinel that rides alongside a real refusal
    // and used to print a second, hex-debug line over it (B198) — and it is the same law
    // `ui_action::cast_fail` already runs. No hex debug line on the player's screen: a code we
    // failed to map can't reach here (the table is total), and a key we typo'd is caught by
    // `equip_error`'s resolution test against the real `GlobalStrings.lua`, not at runtime.
    let player = inv.self_store.iter().next();
    let mut refusals = Vec::new();
    for e in equip_errors.0.drain(..) {
        // Reason 16's substitution — the ONE reason whose text is chosen by the app rather than
        // by the table, because the choice needs the named bag (decision 0916). The reference's
        // helper `0x5ede00(player, bagSlot)` resolves the bag and calls
        // `DisplayError(ERR_WRONG_BAG_TYPE_SUBCLASS, familyName)` *itself*, returning 1 so the
        // caller skips the generic line; a bag that doesn't resolve leaves the generic
        // `ERR_WRONG_BAG_TYPE` standing. Same fork, same fallback.
        let subclass_fill = (e.reason == 16)
            .then(|| {
                bag_family_name(
                    player,
                    e.bag_slot,
                    &inv.objects,
                    &items,
                    bag_families.as_deref().map(|c| &c.0),
                    &commands,
                )
            })
            .flatten();
        let key = match subclass_fill {
            Some(_) => "ERR_WRONG_BAG_TYPE_SUBCLASS",
            None => equip_error_key(e.reason),
        };
        let text = script
            .lua()
            .globals()
            .get::<String>(key)
            .unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        // The two argument-taking reasons, each filling its own specifier. Neither code ever
        // carries the other's fill, so the order is bookkeeping, not precedence.
        let text = match e.required_level {
            Some(d) => text.replace("%d", &d.to_string()),
            None => text,
        };
        let text = match subclass_fill {
            Some(family) => text.replace("%s", &family),
            None => text,
        };
        refusals.push(crate::ui_action::Shown::keyed(key, text));
    }
    // Through the one sink, so the row that named the text also names the surface and the SOUND —
    // 18 of these keys carry an error-speech line (`ERR_INV_FULL`, `ERR_BAG_FULL`,
    // `ERR_NOT_ENOUGH_MONEY`, …), which is the largest single source of it in the client
    // (decision 1815). Every key this table can emit is `kind = 2`, so nothing moved off the red
    // line when the surface stopped being hardcoded here.
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_items", refusals);
    // Both lock clears of the frame — the resolving field-update watch ([`resolve_item_locks`],
    // which runs ahead of EVERY feed) and the failure-driven clears
    // `net/apply/loot.rs::inventory_failure` queued (it has no `UiScript` to fire through). Both
    // fire `ITEM_LOCK_CHANGED` here, the bag windows' own repaint trigger (0218: the popup's
    // No/ESC clear paths the bag never clicked through, so only the event reaches it). Fired
    // AFTER the slot loop below pushes the corrected `.locked` state, so the repaint they trigger
    // sees the unlocked slot, not stale data — and after `feed_char`'s own push, for the same
    // reason on the doll and bank-bag bands (1771).
    let transitioned: Vec<(i64, u32)> = std::mem::take(&mut lock_cleared.0);
    // The containers the server opened this frame (`SMSG_OPEN_CONTAINER`, decision 2339) — the
    // packet handler queued them; they fire as `BAG_OPEN(id)` after the push below, like the locks.
    let opened: Vec<i64> = std::mem::take(&mut opens.0);

    let mut fresh: HashMap<i64, ContainerState> = HashMap::new();
    // The player's own relic flag, resolved once. `find_equip_slot` reads it for INVSLOT 17 and
    // reads it BOTH ways, so an absent table (no client data) answers false — every class then
    // reads as an ordinary ranged wielder, which is the safe degradation.
    let has_relic_slot = player.is_some_and(|p| {
        p.0.unit_class().is_some_and(|c| {
            classes
                .as_deref()
                .is_some_and(|t| t.0.has_relic_slot(u32::from(c)))
        })
    });
    if let Some(store) = player {
        // Bag 0: the backpack — its slots live directly in the player descriptor.
        let mut slots = HashMap::new();
        for i in 0..PACK_SLOTS {
            let guid = store.0.player_pack_slot(i).unwrap_or(0);
            if let Some(mut slot) = resolve_slot(
                guid,
                &inv.objects,
                &items,
                icons.as_deref(),
                rolls,
                &commands,
                &cooldowns,
                spell_catalog,
                &names,
                &mut petitions,
                now,
                ui_now,
                has_relic_slot,
            ) {
                slot.locked = pending.contains(0, u32::from(i) + 1);
                slots.insert(u32::from(i) + 1, slot);
            }
        }
        fresh.insert(
            0,
            ContainerState {
                name: Some("Backpack".into()),
                num_slots: u32::from(PACK_SLOTS),
                slots,
            },
        );

        // Bags 1..4: each INV bag slot holds a container object with its own slot array.
        for bag in 1..=BAGS {
            let bag_guid = store
                .0
                .player_inv_slot(BAG_SLOT_FIRST + bag - 1)
                .unwrap_or(0);
            if bag_guid == 0 {
                continue; // no bag equipped → absent → GetContainerNumSlots = 0
            }
            let (entry, num_slots, slot_guids) = match inv.objects.object(bag_guid) {
                Some(f) => {
                    let n = f.container_num_slots().unwrap_or(0);
                    let guids: Vec<u64> = (0..n.min(36) as u8)
                        .map(|j| f.container_slot(j).unwrap_or(0))
                        .collect();
                    (f.object_entry().unwrap_or(0), n, guids)
                }
                None => (0, 0, Vec::new()),
            };
            let name = (entry != 0)
                .then(|| {
                    items
                        .template(entry, bag_guid, &commands)
                        .map(|t| t.name.clone())
                })
                .flatten();
            let mut slots = HashMap::new();
            for (j, &guid) in slot_guids.iter().enumerate() {
                if let Some(mut slot) = resolve_slot(
                    guid,
                    &inv.objects,
                    &items,
                    icons.as_deref(),
                    rolls,
                    &commands,
                    &cooldowns,
                    spell_catalog,
                    &names,
                    &mut petitions,
                    now,
                    ui_now,
                    has_relic_slot,
                ) {
                    slot.locked = pending.contains(i64::from(bag), j as u32 + 1);
                    slots.insert(j as u32 + 1, slot);
                }
            }
            fresh.insert(
                i64::from(bag),
                ContainerState {
                    name,
                    num_slots,
                    slots,
                },
            );
        }

        // The bank (decision 0604): container −1 = the 24 generic vault slots straight off the
        // player descriptor, containers 5..=10 = the bank bags — each a container object exactly
        // like an equipped bag. Fed unconditionally like the backpack: the descriptor streams at
        // login, the window (BANKFRAME_OPENED) is a UI concern, not a data one.
        let mut slots = HashMap::new();
        for i in 0..BANK_SLOTS {
            let guid = store.0.player_bank_slot(i).unwrap_or(0);
            if let Some(mut slot) = resolve_slot(
                guid,
                &inv.objects,
                &items,
                icons.as_deref(),
                rolls,
                &commands,
                &cooldowns,
                spell_catalog,
                &names,
                &mut petitions,
                now,
                ui_now,
                has_relic_slot,
            ) {
                slot.locked = pending.contains(BANK_CONTAINER, u32::from(i) + 1);
                slots.insert(u32::from(i) + 1, slot);
            }
        }
        fresh.insert(
            BANK_CONTAINER,
            ContainerState {
                name: Some("Bank".into()),
                num_slots: u32::from(BANK_SLOTS),
                slots,
            },
        );
        for bank_bag in 0..BANK_BAGS {
            let bag_id = BANK_BAG_ID_FIRST + i64::from(bank_bag);
            let bag_guid = store.0.player_bank_bag_slot(bank_bag).unwrap_or(0);
            if bag_guid == 0 {
                continue; // no bag in the slot → absent → GetContainerNumSlots = 0
            }
            let (entry, num_slots, slot_guids) = match inv.objects.object(bag_guid) {
                Some(f) => {
                    let n = f.container_num_slots().unwrap_or(0);
                    let guids: Vec<u64> = (0..n.min(36) as u8)
                        .map(|j| f.container_slot(j).unwrap_or(0))
                        .collect();
                    (f.object_entry().unwrap_or(0), n, guids)
                }
                None => (0, 0, Vec::new()),
            };
            let name = (entry != 0)
                .then(|| {
                    items
                        .template(entry, bag_guid, &commands)
                        .map(|t| t.name.clone())
                })
                .flatten();
            let mut slots = HashMap::new();
            for (j, &guid) in slot_guids.iter().enumerate() {
                if let Some(mut slot) = resolve_slot(
                    guid,
                    &inv.objects,
                    &items,
                    icons.as_deref(),
                    rolls,
                    &commands,
                    &cooldowns,
                    spell_catalog,
                    &names,
                    &mut petitions,
                    now,
                    ui_now,
                    has_relic_slot,
                ) {
                    slot.locked = pending.contains(bag_id, j as u32 + 1);
                    slots.insert(j as u32 + 1, slot);
                }
            }
            fresh.insert(
                bag_id,
                ContainerState {
                    name,
                    num_slots,
                    slots,
                },
            );
        }

        // The keyring (decision 0765): container −2, the player array's slots 81.., no container
        // object of its own — structurally the bank's twin. Its capacity is NOT a wire field: both
        // the reference (`GetKeyRingSize`) and the server (`GetMaxKeyringSize`) derive it from the
        // player's level with the same ladder, so [`keyring_size`] computes it here and Lua's
        // `GetKeyRingSize()` reads it back off this snapshot. Slots past that count are never fed
        // (they can't hold anything — the server refuses to store there), so the window's own
        // physIndex-past-size branch hides exactly the right buttons with no keyring-specific code.
        let size = keyring_size(store.0.unit_level().unwrap_or(1));
        let mut slots = HashMap::new();
        for i in 0..size.min(u32::from(KEYRING_SLOTS)) as u8 {
            let guid = store.0.player_keyring_slot(i).unwrap_or(0);
            if let Some(mut slot) = resolve_slot(
                guid,
                &inv.objects,
                &items,
                icons.as_deref(),
                rolls,
                &commands,
                &cooldowns,
                spell_catalog,
                &names,
                &mut petitions,
                now,
                ui_now,
                has_relic_slot,
            ) {
                slot.locked = pending.contains(KEYRING_CONTAINER, u32::from(i) + 1);
                slots.insert(u32::from(i) + 1, slot);
            }
        }
        fresh.insert(
            KEYRING_CONTAINER,
            ContainerState {
                name: Some("Keyring".into()),
                num_slots: size,
                slots,
            },
        );

        // `HasKey()` — the gate that decides whether the keyring exists in the UI at all. Pushed
        // beside the containers because it is the same knowledge (item templates) read over the
        // same slot arrays, and it must be fresh on exactly the frames a BAG_UPDATE fires.
        let key = has_key(&store.0, &inv.objects, &items, &commands);
        if key != memory.had_key {
            gate.audit("feed_containers", "the HasKey() flip");
            debug!(
                "ui_items: HasKey() -> {key} (keyring {})",
                if key { "shown" } else { "hidden" }
            );
            memory.had_key = key;
        }
        script.set_has_key(key);
    }

    if let Some(t0) = cost_t0 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SUM_NS: AtomicU64 = AtomicU64::new(0);
        static N: AtomicU64 = AtomicU64::new(0);
        let sum = SUM_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let n = N.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(60) {
            let slots: usize = fresh.values().map(|c| c.slots.len()).sum();
            eprintln!(
                "[feed-cost] ms={:.3} slots={slots} containers={}",
                sum as f64 / n as f64 / 1e6,
                fresh.len(),
            );
        }
    }
    if apply_container_source(
        &mut script,
        memory,
        player.is_some().then_some(fresh),
        SlotGuids {
            bags: std::array::from_fn(|i| player.map_or(0, |s| bag_slot_guid(s, i as i64 + 1))),
            vault: std::array::from_fn(|i| player.map_or(0, |s| vault_slot_guid(s, i as u8))),
        },
        transitioned,
        opened,
    ) {
        gate.audit(
            "feed_containers",
            "a bag diff, a lock event or a server-opened bag",
        );
    }
}

/// The feed's outward half: diff `source` against what was last pushed, push each changed bag into
/// the VM and fire the reference's events — `BAG_UPDATE(bagID)` (`PLAYERBANKSLOTS_CHANGED` for
/// the vault), one `BAG_UPDATE_DELAYED` per batch, then the lock transitions.
///
/// **`source: None` means the self player STORE is absent this frame — "no data source", never
/// "the player has no items."** The two absent windows are pre-arrival at login (the fresh VM's
/// containers are already empty; nothing to say) and the logout despawn frames:
/// `SMSG_LOGOUT_COMPLETE` despawns the self entity (`net/apply/session.rs::logged_out`) at least
/// one full Update before the `OnExit(InWorld)` shutdown, so this runs against a still-live VM.
/// Diffing the absence as an all-empty snapshot fired a full `BAG_UPDATE` burst whose every bag
/// read `GetContainerNumSlots() == 0` — and an addon that mirrors bags into its saved variables
/// deletes a bag's record on size 0 (Bagnon_Forever's `SaveBagData`), so the burst erased the
/// whole record moments before [`crate::ui_script::shutdown_ui_state`] wrote the file. That was
/// the director's offline-bags report: every recently-logged-out character money-only, the view
/// stale. The reference never delivers such a burst — its UI shutdown (`0x490bd0`) runs with the
/// inventory intact — so here the VM simply keeps its last-pushed state and the shutdown writes
/// the bags the player actually had. Lock-clear events still flush either way: they are
/// packet-driven and must not sit around to fire into a later VM.
pub(crate) fn apply_container_source(
    script: &mut UiScript,
    memory: &mut FeedMemory,
    source: Option<HashMap<i64, ContainerState>>,
    // The player's item-slot guids ([`bag_slot_guid`], [`vault_slot_guid`]) — `BAG_CLOSED`'s diff
    // and `PLAYERBANKSLOTS_CHANGED`'s producer discriminator. Rides beside the source rather than
    // inside it because it is read off the player descriptor, not built from the containers, and
    // the absent-source path must leave the memory alone rather than diff against zeros (that path
    // is the logout despawn window — see this function's own note).
    guids: SlotGuids,
    transitioned: Vec<(i64, u32)>,
    opened: Vec<i64>,
) -> bool {
    // Diff whole bags; push + fire BAG_UPDATE per transition, one BAG_UPDATE_DELAYED per batch. A
    // pending-lock transition always flips a slot's `.locked` (part of `ContainerSlot`'s equality),
    // so it always shows up here too — but `transitioned`'s ITEM_LOCK_CHANGED fires unconditionally
    // below rather than leaning on that invariant.
    let mut pushed = false;
    if let Some(fresh) = source {
        pushed |= diff_and_push(script, memory, fresh, guids);
    }
    // The lock-transition event (decision 0218: the bag windows' own repaint trigger) — after the
    // container push above, so a listener's repaint reads the corrected `.locked` state.
    pushed |= !transitioned.is_empty();
    // **`ITEM_LOCK_CHANGED` carries NO arguments** (decision 2140, found by the argument gate).
    // All five fire sites in the image — `0x495415`, `0x495455`, `0x49557d` (the local lock/unlock
    // paths), `0x5d859a` and `0x5d94b9` (the item field watchers) — are
    // `FrameScript_SignalEvent 0x703e50`, an `__fastcall(ecx = id)` with a plain `ret` and no vararg
    // mechanism at all, so the event cannot carry one. Every stock consumer repaints from `this`
    // (`ContainerFrame.lua:39`, `PaperDollFrame.lua:601`, `BankFrame.lua:209`). The `(bag, slot)`
    // pair benilla pushed is the LATER clients' shape, invented here.
    for _ in transitioned {
        script.fire_event("ITEM_LOCK_CHANGED", Vec::new());
    }
    // `SMSG_OPEN_CONTAINER` → `BAG_OPEN(containerId)` (decision 2339). The reference's arm
    // `0x5e3b35` fires it from the packet handler; ours reaches the VM here — after this frame's
    // `BAG_UPDATE`s, so the frame the stock `ContainerFrame_OnEvent` shows paints the bag as it now
    // is. The id is the reference's own: 0 backpack, 1..4 equipped, 5..10 bank.
    pushed |= !opened.is_empty();
    for bag in opened {
        script.fire_event("BAG_OPEN", vec![ScriptValue::Int(bag)]);
    }
    // Whether anything went into the VM — the caller's gate audit reads it (1439).
    pushed
}

/// The guid of the **bag itself** in container `id` (1..=10) — the four equipped bag slots, then
/// the six bank bag slots. `0` = that slot is empty. Anything else (the backpack 0, the vault −1,
/// the keyring −2) has no bag slot behind it and answers 0.
///
/// This is what `BAG_CLOSED`/`BAG_UPDATE`'s watcher reads: `0x4f8cc0`'s four install loops cover
/// the descriptor bytes `0x540`–`0x55f` (equipped bags 19–22) and `0x6a0`–`0x6cf` (bank bags
/// 63–68) with callback `0x4f8ec0`, and the backpack and keyring loops with `0x4f8db0`, which
/// never reaches the event at all. So exactly these ten slots close a window.
/// The guid of the item in vault slot `i` (0-based, 24 of them — absolute player slots 39..62).
/// `0` = empty. The band `0x5dd8a0`'s L2 loop registers `0x5ddcf0` over (`edi=0x5e0`..`0x698`).
fn vault_slot_guid(store: &ObjectStore, i: u8) -> u64 {
    store.0.player_bank_slot(i).unwrap_or(0)
}

fn bag_slot_guid(store: &ObjectStore, id: i64) -> u64 {
    match id {
        1..=4 => store.0.player_inv_slot(BAG_SLOT_FIRST + (id as u8 - 1)),
        5..=10 => store.0.player_bank_bag_slot(id as u8 - 5),
        _ => None,
    }
    .unwrap_or(0)
}

/// The present-source half of [`apply_container_source`]: push every changed bag, announce each.
fn diff_and_push(
    script: &mut UiScript,
    memory: &mut FeedMemory,
    fresh: HashMap<i64, ContainerState>,
    guids: SlotGuids,
) -> bool {
    // **`BAG_CLOSED` — the only thing in the image that hides an open bag window** (one fire site,
    // `0x4f92b5`, and `ContainerFrame_OnEvent`'s `this:Hide()` is its one consumer;
    // `CloseBag`/`CloseAllBags` are not even strings in `WoW.exe`). Its condition is **not** "the
    // bag went away" — it is `new != old && old != 0` (`0x4f923d je` unchanged, `0x4f9247 je`
    // old-was-empty), so a bag SWAPPED for another fires `BAG_CLOSED(n)` and then `BAG_UPDATE(n)`.
    //
    // Diffed on the BAG's own guid rather than on the pushed `ContainerState` below, because it is
    // a fact about the bag and not about its contents: two identical empty bags exchanged between
    // two slots leave both container states equal and must still close both windows.
    let closed: Vec<i64> = (1..=10)
        .filter(|&id| {
            let (was, now) = (
                memory.bag_guids[id as usize - 1],
                guids.bags[id as usize - 1],
            );
            was != 0 && now != was
        })
        .collect();
    // The other half of the same branch: a bag REMOVED fires `BAG_CLOSED` and no `BAG_UPDATE`
    // (`0x4f92cc`). Every other transition still announces.
    let emptied: Vec<i64> = closed
        .iter()
        .copied()
        .filter(|&id| guids.bags[id as usize - 1] == 0)
        .collect();
    memory.bag_guids = guids.bags;
    for id in &closed {
        script.fire_event("BAG_CLOSED", vec![ScriptValue::Int(*id)]);
    }
    // **`PLAYERBANKSLOTS_CHANGED` has TWO producers in the reference, and they carry different
    // arguments** (decision 2140). benilla had one fire for both, so half of them were argless
    // where the reference pushes a string:
    //
    // * **P1, the player-descriptor path** — `0x5ddcf0`'s watcher sees the slot's own GUID field
    //   change and fires `0x5ddd6e`, `FrameScript_SignalEvent 0x703e50`, `__fastcall(ecx = id)`
    //   with a plain `ret`: **zero Lua values**. This is an item arriving, leaving, or being
    //   exchanged for a different one.
    // * **P2, the item-object path** — the changed ITEM's own watched fields move (stack count,
    //   spell charges, enchantment, flags, durability, entry) or its `CGItem` enters the world;
    //   `0x4c7180` resolves it to a flat slot and fires `0x4c728d`,
    //   `SignalEvent2(347, "%s", "player")`: **`arg1` is the unit token `"player"`**. This is the
    //   same item, changed.
    //
    // So the discriminator is the slot's item GUID, not its pushed view — and that is not a
    // refinement, it is the only thing that answers: two different instances of one template push
    // an identical `ContainerSlot`, so a view diff reads a swap of two identical stacks as *no
    // change at all* and fires nothing where the reference fires P1 twice. Exactly 1777's reason
    // for diffing `BAG_CLOSED` on the bag's own guid, one band over.
    //
    // Planned here, before `memory` is rewritten, and fired below after the push — a listener must
    // repaint off the corrected state. It is deliberately OUTSIDE the `changed` gate for the same
    // identical-swap reason: that transition moves no container state.
    let vault: Vec<bool> = {
        let empty = HashMap::new();
        let now = fresh.get(&BANK_CONTAINER).map_or(&empty, |c| &c.slots);
        let was = memory
            .pushed
            .get(&BANK_CONTAINER)
            .map_or(&empty, |c| &c.slots);
        (1..=u32::from(BANK_SLOTS))
            .filter_map(|slot| {
                let i = slot as usize - 1;
                if guids.vault[i] != memory.vault_guids[i] {
                    Some(false)
                } else {
                    (now.get(&slot) != was.get(&slot)).then_some(true)
                }
            })
            .collect()
    };
    memory.vault_guids = guids.vault;
    let changed: Vec<i64> = fresh
        .keys()
        .chain(memory.pushed.keys())
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .filter(|b| fresh.get(b) != memory.pushed.get(b))
        .collect();
    if !changed.is_empty() {
        // **The spent-ammo signal** (decision 1509). The reference registers a field mirror on
        // `ITEM_FIELD_STACK_COUNT` for TYPEID ITEM (`ClntObjMgrSetTypeMirrorHandler 0x468070` at
        // `0x5d9360`, handler `0x5d9400`); for any item the active player owns, a stack-count
        // write fires **ITEM_LOCK_CHANGED first**, ahead of the `BAG_UPDATE` the same handler
        // emits further down. Nothing else in the image reaches that event from a server update —
        // its other four fire sites are all local lock/unlock — so it is the ONLY per-shot signal
        // an addon can detect a spent arrow with, and every auto-shot timer is built on it.
        //
        // Computed before `memory.pushed` is replaced at the bottom of this block.
        let restacked = {
            let empty = HashMap::new();
            let mut v: Vec<(i64, u32)> = Vec::new();
            for &bag in &changed {
                let now = fresh.get(&bag).map_or(&empty, |c| &c.slots);
                let was = memory.pushed.get(&bag).map_or(&empty, |c| &c.slots);
                for (&slot, n) in now {
                    // The SAME item, restacked. A slot whose entry changed is a create or a swap,
                    // not a field write on one item, and it does not take this path.
                    if was.get(&slot).is_some_and(|w| {
                        w.item_id != 0 && w.item_id == n.item_id && w.count != n.count
                    }) {
                        v.push((bag, slot));
                    }
                }
            }
            // Map order is not an order, and an event stream has to be reproducible.
            v.sort_unstable();
            v
        };
        for &bag in &changed {
            script.set_container(bag, fresh.get(&bag).cloned());
        }
        // Ahead of BAG_UPDATE below, which is the reference handler's own order. One event per
        // restacked slot and no arguments (see the note above) — `restacked` keeps the `(bag,
        // slot)` pairs rather than a count because they are what identifies the slot for anyone
        // reading this at a breakpoint; the event itself has never carried them.
        for _ in &restacked {
            script.fire_event("ITEM_LOCK_CHANGED", Vec::new());
        }
        // Name the bags, not just the count: "3 changed" can't tell you WHICH container moved, and
        // the negative ids (−1 bank, −2 keyring) are exactly the ones you go looking for.
        debug!(
            "ui_items: fed {} changed bag(s) — {}",
            changed.len(),
            changed
                .iter()
                .map(|b| format!("{b}:{}", fresh.get(b).map_or(0, |c| c.slots.len())))
                .collect::<Vec<_>>()
                .join(" ")
        );
        for &bag in &changed {
            // The vault fires the reference's own event, once per changed slot and **with no
            // arguments** (`0x5ddd6e` calls `FrameScript_SignalEvent 0x703e50`, an
            // `__fastcall(ecx = id)` with a plain `ret` and no vararg push). It is a broadcast:
            // every bank button repaints from its own `GetInventorySlot()`, which is why no slot id
            // is needed and why the bank BAG band's copy of this event — fired by
            // `ui_char::feed_char`, the feed that owns that band — is indistinguishable from this
            // one.
            //
            // Everything else — backpack, equipped bags, AND bank bags (ordinary container frames
            // in the reference) — fires BAG_UPDATE(bagID). The vault is the one band that fires
            // NEITHER `BAG_UPDATE` nor `BAG_CLOSED`: `0x4f8cc0` installs its container listener
            // over the six bank-bag fields (`0x6a0`–`0x6c8`) and over nothing in the 24 vault
            // fields (`0x5e0`–`0x698`).
            if bag != BANK_CONTAINER && !emptied.contains(&bag) {
                script.fire_event("BAG_UPDATE", vec![ScriptValue::Int(bag)]);
            }
        }
        memory.pushed = fresh;
    }
    // The vault band's announce, planned above. One event per changed slot, as the watcher does —
    // the slot travels in *how many times it fires*, never in an argument (1776).
    for &same_item in &vault {
        let args = if same_item {
            vec![ScriptValue::Str("player".into())]
        } else {
            Vec::new()
        };
        script.fire_event("PLAYERBANKSLOTS_CHANGED", args);
    }
    // A `BAG_CLOSED` or a vault event with nothing else to say still went into the VM — two
    // identical bags (or two identical items) swapped between two slots change no container state
    // at all.
    !changed.is_empty() || !closed.is_empty() || !vault.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{
        apply_container_source, charges_count, ContainerSlot, FeedMemory, SlotGuids,
        BANK_CONTAINER, BANK_SLOTS,
    };

    /// No bag in any of the ten bag slots, nothing in the vault — every test below drives the
    /// backpack (container 0), which has no bag slot behind it and can never raise `BAG_CLOSED`.
    const NO_BAGS: SlotGuids = SlotGuids {
        bags: [0; 10],
        vault: [0; BANK_SLOTS as usize],
    };

    /// **`PLAYERBANKSLOTS_CHANGED` has two producers and they carry different arguments**
    /// (decision 2140).
    ///
    /// benilla fired the argless one for every transition, which is right for half of them and
    /// wrong for the other half — and the half it got wrong is the one a view diff cannot even
    /// see. The three arms below are the three the reference distinguishes:
    ///
    ///  · a slot's item GUID changes (arrive / leave / exchange) → `0x5ddd6e`, **no arguments**;
    ///  · the same item's own fields change (a restack) → `0x4c728d`, **`arg1 = "player"`**;
    ///  · two IDENTICAL items exchanged between two slots → the pushed views are equal, so the
    ///    container diff sees nothing at all, and the reference fires the argless one **twice**.
    /// `SMSG_OPEN_CONTAINER` → `BAG_OPEN(containerId)` (decision 2339): the ids the handler
    /// resolved reach the VM in order, after the frame's container push, and the stock
    /// `ContainerFrame_OnEvent` sees the id as `arg1` — the reference's own event shape.
    #[test]
    fn a_server_opened_bag_fires_bag_open_with_its_container_id() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "SEEN = {} \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('BAG_OPEN') \
             f:RegisterEvent('BAG_UPDATE') \
             f:SetScript('OnEvent', function() \
                 table.insert(SEEN, event .. ' ' .. tostring(arg1)) end)",
        )
        .unwrap();
        let mut memory = FeedMemory::default();
        // A bag equipped into the second bag slot, and nothing else changed this frame.
        assert!(apply_container_source(
            &mut s,
            &mut memory,
            None,
            NO_BAGS,
            Vec::new(),
            vec![2]
        ));
        let seen = s.eval::<Vec<String>>("return SEEN").unwrap();
        assert_eq!(seen, vec!["BAG_OPEN 2"]);
        // Nothing queued: nothing fired, and the push reports nothing went in.
        s.run("SEEN = {}").unwrap();
        assert!(!apply_container_source(
            &mut s,
            &mut memory,
            None,
            NO_BAGS,
            Vec::new(),
            Vec::new()
        ));
        assert!(s.eval::<Vec<String>>("return SEEN").unwrap().is_empty());
    }

    #[test]
    fn playerbankslots_changed_names_its_producer_by_the_slot_guid() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "SEEN = {} \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('PLAYERBANKSLOTS_CHANGED') \
             f:SetScript('OnEvent', function() \
                 table.insert(SEEN, event .. ' ' .. tostring(arg1)) end)",
        )
        .unwrap();
        let seen = |s: &mut UiScript| {
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };
        // The vault, with `slot` holding `count` of item 1234.
        let vault = |slot: u32, count: u32| {
            HashMap::from([(
                BANK_CONTAINER,
                ContainerState {
                    name: None,
                    num_slots: u32::from(BANK_SLOTS),
                    slots: HashMap::from([(
                        slot,
                        ContainerSlot {
                            item_id: 1234,
                            count,
                            ..Default::default()
                        },
                    )]),
                },
            )])
        };
        let mut guids = NO_BAGS;
        let mut memory = FeedMemory::default();

        // An item ARRIVES in vault slot 1 — the descriptor path, no arguments.
        guids.vault[0] = 0xF00D;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(vault(1, 5)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["PLAYERBANKSLOTS_CHANGED nil"]);

        // The SAME item restacks — its own `ITEM_FIELD_STACK_COUNT` moved, which is the item
        // watcher's path, and the reference pushes the unit token there.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(vault(1, 9)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["PLAYERBANKSLOTS_CHANGED player"]);

        // Nothing moved at all.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(vault(1, 9)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert!(seen(&mut s).is_empty(), "an unchanged vault says nothing");

        // Two IDENTICAL stacks exchanged between slots 1 and 2. Both slot views are equal before
        // and after, so `fresh == memory.pushed` and the container diff is empty — the guids are
        // the only witness, and the reference fires twice.
        let two = || {
            HashMap::from([(
                BANK_CONTAINER,
                ContainerState {
                    name: None,
                    num_slots: u32::from(BANK_SLOTS),
                    slots: HashMap::from([
                        (
                            1,
                            ContainerSlot {
                                item_id: 1234,
                                count: 9,
                                ..Default::default()
                            },
                        ),
                        (
                            2,
                            ContainerSlot {
                                item_id: 1234,
                                count: 9,
                                ..Default::default()
                            },
                        ),
                    ]),
                },
            )])
        };
        guids.vault[1] = 0xBEEF;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(two()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["PLAYERBANKSLOTS_CHANGED nil"]);
        guids.vault.swap(0, 1);
        let pushed = apply_container_source(
            &mut s,
            &mut memory,
            Some(two()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            seen(&mut s),
            vec![
                "PLAYERBANKSLOTS_CHANGED nil".to_string(),
                "PLAYERBANKSLOTS_CHANGED nil".to_string()
            ],
            "the swap the pushed containers cannot see"
        );
        assert!(pushed, "and the feed reports that it spoke to the VM");
    }

    /// **`BAG_CLOSED`, the only thing that hides an open bag window** — and the three ways to get
    /// its condition wrong (`0x4f92b5`).
    ///
    /// benilla never fired this event at all, so a `ContainerFrame` outlived the bag it belonged
    /// to: `ContainerFrame_OnEvent`'s `this:Hide()` arm is its one consumer in all of FrameXML,
    /// and `CloseBag`/`CloseAllBags` are not even strings in `WoW.exe` — there is no other
    /// mechanism, for either band.
    ///
    /// The three traps, each asserted below:
    ///  · the condition is `new != old && old != 0`, NOT "the bag went away" — a SWAP fires it;
    ///  · an unequip fires `BAG_CLOSED` and **no** `BAG_UPDATE`, while a swap fires both;
    ///  · the diff is on the BAG's own guid, not on the pushed container — two identical empty
    ///    bags exchanged between two slots leave every container state equal, and both windows
    ///    must still close.
    #[test]
    fn bag_closed_fires_on_the_bag_guid_moving_and_only_off_a_non_empty_slot() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "SEEN = {} \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('BAG_CLOSED') \
             f:RegisterEvent('BAG_UPDATE') \
             f:SetScript('OnEvent', function() \
                 table.insert(SEEN, event .. ' ' .. tostring(arg1)) end)",
        )
        .unwrap();
        let seen = |s: &mut UiScript| {
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };
        let pouch = |n: u32| {
            HashMap::from([(
                1,
                ContainerState {
                    name: Some("Small Brown Pouch".into()),
                    num_slots: n,
                    slots: HashMap::new(),
                },
            )])
        };
        let mut guids = NO_BAGS;
        let mut memory = FeedMemory::default();

        // Equip a bag into the first equipped bag slot: BAG_UPDATE, and no BAG_CLOSED — the slot
        // was empty, which is the `0x4f9247 je` arm.
        guids.bags[0] = 0xAAAA;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(pouch(6)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["BAG_UPDATE 1"]);

        // Its contents change: the bag guid stands, so neither the close nor its suppression
        // applies and the ordinary announce runs.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(pouch(6)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert!(
            seen(&mut s).is_empty(),
            "an unchanged container says nothing"
        );

        // SWAP for a different bag: closed FIRST, then updated. This is the arm a "the bag went
        // away" reading gets wrong — the slot is still occupied.
        guids.bags[0] = 0xBBBB;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(pouch(10)),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["BAG_CLOSED 1", "BAG_UPDATE 1"]);

        // UNEQUIP: closed, and **no** BAG_UPDATE.
        guids.bags[0] = 0;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(HashMap::new()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["BAG_CLOSED 1"]);

        // Two IDENTICAL empty bags exchanged between bank bag slots 5 and 6. Every container
        // state is equal, so the container diff sees nothing at all — and both windows must still
        // close. This is the case that makes the guid the right thing to watch.
        let mut memory = FeedMemory::default();
        let two = || {
            HashMap::from([
                (
                    5,
                    ContainerState {
                        name: Some("Linen Bag".into()),
                        num_slots: 6,
                        slots: HashMap::new(),
                    },
                ),
                (
                    6,
                    ContainerState {
                        name: Some("Linen Bag".into()),
                        num_slots: 6,
                        slots: HashMap::new(),
                    },
                ),
            ])
        };
        let mut guids = NO_BAGS;
        guids.bags[4] = 0x1111;
        guids.bags[5] = 0x2222;
        apply_container_source(
            &mut s,
            &mut memory,
            Some(two()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        let _ = seen(&mut s);
        guids.bags.swap(4, 5);
        let pushed = apply_container_source(
            &mut s,
            &mut memory,
            Some(two()),
            guids,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(seen(&mut s), vec!["BAG_CLOSED 5", "BAG_CLOSED 6"]);
        assert!(
            pushed,
            "a BAG_CLOSED with no container change still went into the VM — the gate audit reads \
             this return"
        );
    }
    use benilla_protocol::messages::ItemSpellEntry;
    use benilla_ui::script::{ContainerState, UiScript};
    use std::collections::HashMap;

    /// The logout-wipe law (the director's stale-offline-bags report): an **absent** self player
    /// store is "no data source", never "the player has no items". `SMSG_LOGOUT_COMPLETE` despawns
    /// the self entity a full Update before `OnExit(InWorld)` shuts the VM down, and the feed used
    /// to diff that absence as an all-empty snapshot — a `BAG_UPDATE` burst whose every bag read
    /// `GetContainerNumSlots() == 0`, which made Bagnon_Forever erase its records right before the
    /// saved-variables write. A *present* source that lost a bag is a real transition and must
    /// still announce — the law gates on the source's existence, not on its emptiness.
    #[test]
    fn an_absent_self_player_is_no_source_never_an_empty_bag_burst() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "BAG_EVENTS = 0 \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('BAG_UPDATE') \
             f:SetScript('OnEvent', function() BAG_EVENTS = BAG_EVENTS + 1 end)",
        )
        .unwrap();
        let bag0 = ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: HashMap::new(),
        };
        let mut memory = FeedMemory::default();

        // The in-session push: source present, bag new → pushed + announced.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(HashMap::from([(0, bag0)])),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 16);
        assert_eq!(s.eval::<i64>("return BAG_EVENTS").unwrap(), 1);

        // The logout despawn frame: no store. The VM keeps its last-pushed bags and no event
        // fires — an addon reading its bags out of the PLAYER_LOGOUT edge sees them intact.
        apply_container_source(&mut s, &mut memory, None, NO_BAGS, Vec::new(), Vec::new());
        assert_eq!(
            s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(),
            16,
            "an absent source must not empty the VM's containers"
        );
        assert_eq!(
            s.eval::<i64>("return BAG_EVENTS").unwrap(),
            1,
            "an absent source must not fire a BAG_UPDATE burst"
        );

        // A PRESENT source without the bag is a genuine transition: push + announce.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(HashMap::new()),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return BAG_EVENTS").unwrap(), 2);
    }

    /// **The spent-ammo signal** (decision 1509, B267's second half). A stack ticking down must
    /// fire `ITEM_LOCK_CHANGED`, **before** the `BAG_UPDATE` for its bag — the reference's
    /// `ITEM_FIELD_STACK_COUNT` mirror handler's own order. The event carries no arguments
    /// (2140): `0x5d94b9` is a `SignalEvent` site, which has no vararg mechanism at all.
    ///
    /// This is not cosmetic ordering. Quiver's auto-shot timer has no other way to learn a shot
    /// fired: it starts the reload drain from this event, and without it the bar fills once and
    /// sits at 100% forever (the director's report). Every vanilla shot timer works this way.
    ///
    /// The negative half matters as much: a slot whose ENTRY changed is a swap or a create, not a
    /// field write on one item, and must NOT fire it — over-firing would make the addon count
    /// shots that never happened.
    #[test]
    fn a_stack_ticking_down_fires_item_lock_changed_before_bag_update() {
        use benilla_ui::script::ContainerSlot;

        let mut s = UiScript::new().unwrap();
        s.run(
            "ORDER = {} \
             local f = CreateFrame('Frame') \
             f:RegisterEvent('ITEM_LOCK_CHANGED') \
             f:RegisterEvent('BAG_UPDATE') \
             f:SetScript('OnEvent', function() \
                 table.insert(ORDER, event .. ':' .. tostring(arg1) .. ',' .. tostring(arg2)) \
             end)",
        )
        .unwrap();

        let arrows = |count: u32| ContainerSlot {
            item_id: 2512, // Rough Arrow
            count,
            ..Default::default()
        };
        let bag = |s: ContainerSlot| {
            HashMap::from([(
                0,
                ContainerState {
                    name: Some("Backpack".into()),
                    num_slots: 16,
                    slots: HashMap::from([(1, s)]),
                },
            )])
        };
        let mut memory = FeedMemory::default();

        // The quiver arrives full — a create, not a restack. No lock event.
        apply_container_source(
            &mut s,
            &mut memory,
            Some(bag(arrows(200))),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            s.eval::<String>("return table.concat(ORDER, ' ')").unwrap(),
            "BAG_UPDATE:0,nil",
            "a slot appearing is a create, not a stack-count field write"
        );

        // An arrow leaves the quiver.
        s.run("ORDER = {}").unwrap();
        apply_container_source(
            &mut s,
            &mut memory,
            Some(bag(arrows(199))),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            s.eval::<String>("return table.concat(ORDER, ' ')").unwrap(),
            "ITEM_LOCK_CHANGED:nil,nil BAG_UPDATE:0,nil",
            "the shot signal fires — with NO arguments, as all five of the reference's fire sites \
             do (decision 2140) — and precedes BAG_UPDATE. The slot it names travels in the fact \
             that it fired at all, which is enough: every consumer repaints from its own `this`, \
             and Quiver's shot timer only needs to know that A shot happened"
        );

        // A DIFFERENT item in the same slot: a swap. The count differs too, and it must still
        // not fire — otherwise an addon counts a shot every time you rearrange your bags.
        s.run("ORDER = {}").unwrap();
        let mut other = arrows(20);
        other.item_id = 3033; // Razor Arrow
        apply_container_source(
            &mut s,
            &mut memory,
            Some(bag(other)),
            NO_BAGS,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            s.eval::<String>("return table.concat(ORDER, ' ')").unwrap(),
            "BAG_UPDATE:0,nil",
            "a changed entry is a swap, not a stack-count write"
        );
    }

    fn slot(spell_id: u32, charges: i32) -> ItemSpellEntry {
        ItemSpellEntry {
            index: 0,
            spell_id,
            trigger: 0,
            charges,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        }
    }

    /// The real builder's charge gate (`0x52da01`/`0x52db51`): the `-1` consume-on-use sentinel
    /// prints NO line — the fix for food/water/potions (Tough Hunk of Bread's wire `-1` was
    /// rendering "1 Charge") — while a real pool prints its absolute value.
    #[test]
    fn charge_gate_matches_the_real_builder() {
        // Food/water/potions: spellcharges -1 (VERIFIED live vmangos item_template — bread 4540,
        // spring water 159, conjured water 5350 all carry `433/430, -1`). No line.
        assert_eq!(charges_count(&[slot(433, -1)]), 0, "food's -1 = no line");
        // A real charge pool, negative = item destroyed when depleted (Flame Deflector 4376:
        // `4057, -5`) — prints the absolute count.
        assert_eq!(charges_count(&[slot(4057, -5)]), 5, "wand-style pool");
        // A positive pool would print as-is (the abs is a no-op).
        assert_eq!(charges_count(&[slot(4057, 3)]), 3);
        // Chargeless slots and empty slots: no line.
        assert_eq!(charges_count(&[slot(433, 0)]), 0, "template 0 = sentinel");
        assert_eq!(charges_count(&[slot(0, -5)]), 0, "no spell = no slot");
        assert_eq!(charges_count(&[]), 0);
        // The election walks slots: a leading sentinel slot doesn't mask a later pool.
        assert_eq!(charges_count(&[slot(433, -1), slot(4057, -10)]), 10);
    }

    /// The trigger line's **empty-description law**, byte-verified at the item builder
    /// (`0x52da29`-`0x52da31`: test the expanded text's first byte, jump past the whole block on
    /// zero) — checked against the REAL 5875 Spell.dbc, because the whole point is what the actual
    /// data holds.
    ///
    /// The director's report: a dungeon key's tooltip read "Use: Opening". `Opening` is a real
    /// spell with a real name and NO description, and the old code fell back to the name.
    #[test]
    fn an_undescribed_spell_prints_no_trigger_line_on_real_data() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let spells = crate::ui_action::Spells {
            catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
            forms: benilla_formats::load_shapeshift_forms(&mut chain)
                .expect("SpellShapeshiftForm.dbc"),
            ranges: benilla_formats::load_spell_ranges(&mut chain).expect("SpellRange.dbc"),
            cast_times: benilla_formats::load_spell_cast_times(&mut chain)
                .expect("SpellCastTimes.dbc"),
            durations: benilla_formats::load_spell_durations(&mut chain)
                .expect("SpellDuration.dbc"),
            radii: benilla_formats::load_spell_radii(&mut chain).expect("SpellRadius.dbc"),
        };

        // No string table: what this test is about is whether a line is built at all, and
        // Fireball's description reaches no keyed token — a `$s1` spread renders from the spell's
        // own columns. A fixture that pretended to hold GlobalStrings would be claiming coverage
        // this assertion does not have.
        let no_strings = |_: &str, _: &[i64]| None;

        // Every "Opening"/"Closing" the lock chain can reach — the key spells (3365/3366/6247/6477
        // are all literally named "Opening") carry no description, so NO Use: line may be built.
        for id in [3365u32, 3366, 6246, 6247, 6477, 21651] {
            let d = spells.catalog.get(id).expect("a real Spell.dbc row");
            assert!(
                !d.name.is_empty(),
                "spell {id} has a name — which is exactly what must NOT leak into the tooltip"
            );
            assert_eq!(
                super::spell_desc_text(Some(&spells), id, None, &no_strings),
                None,
                "spell {id} ({:?}) has no description, so the reference prints no trigger line",
                d.name
            );
        }

        // The control: a described spell still produces its line, so this is a law about EMPTY
        // descriptions and not a blanket mute. Fireball (133) is the tooltip suite's own anchor.
        let fireball = super::spell_desc_text(Some(&spells), 133, None, &no_strings)
            .expect("a described spell still yields its line");
        assert!(
            fireball.contains("damage"),
            "expected the substituted Fireball description, got {fireball:?}"
        );
    }
}
