//! **Inward** — the action bar's *identity* feed: what each of the 120 slots shows.
//!
//! [`feed_actions`] resolves each occupied slot's icon (spell: Spell.dbc × SpellIcon.dbc; item: the
//! item template chain, the same ask-once store the bags use) and count (item: the bag walk,
//! `ui_items::count_of`), diffs against what the VM already holds, pushes the changed slots and
//! fires `ACTIONBAR_SLOT_CHANGED` per transition.
//!
//! **What is gated and what is not** is the whole design here. The identity resolve runs on its two
//! inputs having changed — `PlayerActions::dirty` (a real, if occasional, event: login or a local
//! pickup/place) OR a landed item template ([`Items::template_epoch`], decision 0660: an ITEM
//! icon's template is fetched ask-once, so the *first* resolve of a cold entry is the one that
//! ISSUES the query and reads back nothing). Two things drift independently of BOTH and so refresh
//! every frame instead: an ITEM slot's bag **count** (eating a stack down never touches
//! `SMSG_ACTION_BUTTONS`) and a weapon-substituting **icon** ([`super::weapon_icon`], decisions
//! 0230/0231 — a swap changes it without touching the action table). Gating those on the same flag
//! is what leaves a stale Count fontstring or a stale Attack face.
//!
//! The feed also pumps the UIErrorsFrame queues (cast fails, mount refusals, the app's by-key
//! local refusals, the ENGINE's own — `benilla_ui` is engine-free and cannot reach
//! [`UiErrorKeys`], so a refusal raised inside the script crate queues its GlobalStrings key here
//! instead — and [`UiErrorTexts`], the lines that arrive already resolved) into
//! `UI_ERROR_MESSAGE`, or `UI_INFO_MESSAGE` for the yellow arm; and the stance page
//! (`GetBonusBarOffset`) — our descriptor's
//! shapeshift-form byte indexed into `SpellShapeshiftForm.dbc`'s BonusActionBar column
//! (`0x4e4fc0`), firing `UPDATE_BONUS_ACTIONBAR` on change.

use crate::ui_items::{count_of, InventoryScope};
use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use benilla_ui::script::{ActionSlot, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};

use super::errors::{first_missing_totem, first_short_reagent, mount_result_key};
use super::weapon_icon::{auto_attack_icon, substitutes_weapon_icon};
use super::{
    cast_fail, show_messages, ui_error_text, CastErrors, MessageSink, MountErrors, PetTameFailures,
    PlayerActions, Shown, Spells, UiError, UiErrorKeys, UiErrorTexts,
};

/// What an ITEM action shows when its icon cannot be resolved — the reference's own hardcoded
/// literal at `0x847fe4`, returned by the item-icon resolver's failure block `0x5d8927` (exactly
/// two engine sites binary-wide, zero Lua). Reached two ways:
/// the template hasn't answered yet (displayId 0 — the login window decision 0660 closes), or the
/// displayId has no `ItemDisplayInfo` row at all.
pub(super) const MISSING_ITEM_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// The feed's memory of what it last pushed, for per-slot change events.
#[derive(Default)]
pub(super) struct FeedMemory {
    pushed: HashMap<u32, ActionSlot>,
    bonus_offset: u8,
    /// **Has the identity resolve run against the VM this memory is about?**
    ///
    /// Wrapping the memory in `VmMemo` (1290) makes a login's fresh VM reset `pushed` — but the
    /// diff that reads `pushed` lives *inside* the gate below, and every other input to that gate
    /// is host-side and survives the VM. `dirty` is false, and both `template_epoch` and
    /// `macro_generation` can legitimately match the reset memory's zeros. The bar would then be
    /// fed nothing at all, for the whole session.
    ///
    /// So the gate takes the memory's own freshness as an input. It is not a fourth *reason* to
    /// re-resolve; it is the statement that a resolve is only valid for the VM it ran against.
    resolved: bool,
    /// The [`Items::template_epoch`] the last identity resolve ran at — the feed's half of the
    /// landed-template redisplay (decision 0660). An advance re-resolves, exactly like a bar edit.
    template_epoch: u64,
    /// The macro-table generation the last identity resolve ran at (decision 0983) — the THIRD
    /// input, exactly like `template_epoch` above: editing a macro changes its bar icon while
    /// touching neither the action table nor any item template.
    macro_generation: u64,
}

/// The DBC name tables the cast-fail **argument arms** read (`FailArgs`), as one parameter — three
/// `Option<Res<…>>` that are one concept and were pushing this system past Bevy's parameter arity
/// (decision 1948). Each is `Option` for the same reason `FailArgs`' fields are: a client with no
/// game data has none, and the arm then declines and the template strips.
///
/// The fourth table, `SpellShapeshiftForm.dbc` (`0x56`), is not here: it already rides
/// [`Spells`], which this system takes anyway, so [`Self::args`] takes that and joins them.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct FailNameTables<'w> {
    /// `SpellFocusObject.dbc` — the crafting book's catalog, for `0x5e`.
    focus: Option<Res<'w, crate::ui_tradeskill::SpellFocus>>,
    /// `AreaTable.dbc` — the map arc's, for `0x5d`.
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    /// `SpellMechanic.dbc` — for `0x8d`, the one arm whose word is produced locally.
    mechanics: Option<Res<'w, super::SpellMechanics>>,
}

impl FailNameTables<'_> {
    fn args<'a>(&'a self, spells: Option<&'a Spells>) -> cast_fail::FailArgs<'a> {
        cast_fail::FailArgs {
            arg: None,
            focus: self.focus.as_deref().map(|f| &f.catalog),
            areas: self.areas.as_deref().map(|a| &a.0),
            mechanics: self.mechanics.as_deref().map(|m| &m.catalog),
            forms: spells.map(|s| &s.forms),
        }
    }
}

pub(super) fn feed_actions(
    script: Option<NonSendMut<UiScript>>,
    mut actions: ResMut<PlayerActions>,
    mut cast_errors: ResMut<CastErrors>,
    mut mount_errors: ResMut<MountErrors>,
    mut pet_tame_failures: ResMut<PetTameFailures>,
    // The two client-local refusal queues as one param (the 16-SystemParam ceiling this
    // signature already sits at): the by-KEY route (`DisplayError`) and the already-resolved
    // TEXT route, both drained onto the same red line below.
    ui_errors: (ResMut<UiErrorKeys>, ResMut<UiErrorTexts>),
    spells: Option<Res<Spells>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    // The inventory read (2334): every count, reagent and totem question below walks the bags
    // through it, and the weapon-icon arms resolve the equipped instance here.
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    sub_classes: Option<Res<crate::ui_items::ItemSubClasses>>,
    name_tables: FailNameTables,
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
    // Where a displayed message lands: the chat window (the combat log's own record of a failed
    // cast, 1703, rides the same resource) and the catalog's sound queue (1815).
    mut sink: MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let (mut ui_error_keys, mut ui_error_texts) = ui_errors;
    let memory = memory.get(&script);

    // Rejected casts surface as the client's red error line (UI_ERROR_MESSAGE → the errors
    // frame), resolved through the byte-verified two-layer display ([`cast_fail`]) against the
    // VM's own GlobalStrings — resolve first (immutable script), then fire (mutable).
    // 0x78 TOTEMS / 0x5c REAGENTS are the argument-formatted reasons whose `%s` fill benilla
    // models (decisions 0545 + 0552, the ref's shared fill arm `0x6e1e7f`): "Requires %s" /
    // "Missing reagent: %s" + the FAILING slot's item name — re-derived here exactly as the
    // check derived it (first missing totem / first short reagent against our bags). On an
    // item-cache miss the ref queries and shows nothing that frame, then its DBCACHECALLBACK
    // `0x6e29b0` REDISPLAYS when the answer lands — modeled by keeping the entry queued: the
    // ask-once query is away, and the frame the template answers, the fill succeeds and fires.
    // The DBC-only arms (0x5d REQUIRES_AREA, 0x5e REQUIRES_SPELL_FOCUS) need no round trip and
    // are filled inside [`cast_fail`] itself, off the wire's argument word.
    let self_store = self_q.iter().next();
    let mut await_template: Vec<crate::ui_action::CastFail> = Vec::new();
    // The same failures, worded for the combat log. Collected beside the red line rather than
    // instead of it: `0x6e1a00` calls `0x62c360` AND `DisplayError`, which frame the one string
    // two ways — "Not enough mana." on the screen, "You fail to cast Frostbolt: Not enough mana."
    // in the log.
    let mut fail_lines: Vec<crate::ui_chat::combat::PendingCombat> = Vec::new();
    let fail_args = name_tables.args(spells.as_deref());
    let texts: Vec<cast_fail::CastFailLine> = cast_errors
        .0
        .drain(..)
        .filter_map(|fail| {
            let crate::ui_action::CastFail {
                spell_id,
                reason,
                caster,
                ..
            } = fail;
            let pet = caster == crate::ui_action::Caster::Pet;
            let d = spells.as_ref().and_then(|s| s.catalog.get(spell_id));
            let get = |key: &str| script.lua().globals().get::<String>(key).ok();
            // The displayed line first, the log line off it — the reference's own order, and the
            // reason the two can no longer drift apart (see the combat-log twin below).
            let line = (|| -> Option<cast_fail::CastFailLine> {
                // 0x19/0x1a/0x1b EQUIPPED_ITEM_CLASS* — the other argument-formatted family whose
                // `%s` benilla models (`0x6e1db7`, the arm that resolves an item class/subclass name
                // through `0x6e2380`): "Must have a **Wand** equipped", the SINGULAR DisplayName,
                // where the spell tooltip's own requirement line takes the verbose plural. Purely a
                // DBC read, so no query/redisplay round trip. A multi-bit mask resolves too — through
                // ItemSubClassMask.dbc's group name, else the FIRST matching subclass (the
                // tooltip's twin at `0x52eea7` joins instead).
                if let (0x19..=0x1b, Some(d), Some(subs)) = (reason, d, sub_classes.as_deref()) {
                    if let Some(name) = (d.equipped_item_class >= 0)
                        .then(|| {
                            subs.0.requirement_display_name(
                                d.equipped_item_class as u32,
                                d.equipped_item_subclass_mask,
                            )
                        })
                        .flatten()
                    {
                        let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                        return get(key)
                            .filter(|s| !s.is_empty())
                            .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
                    }
                }
                // `0x31` NEED_EXOTIC_AMMO (`0x6e1e54`) — the same `0x6e2380` helper as the arm
                // above, one class over: `requirement_display_name(6 /* Projectile */, 1 << arg)`
                // reads the singular DisplayName at `row + 0x28 + locale*4`, so "Requires exotic
                // ammo: %s" names the ammo type. The shift is masked to five bits because the
                // reference's is — `0x6e1e5c: shl edx,cl` takes x86's own `cl & 31` — and a
                // subclass naming no row declines to the strip fallback exactly as
                // `0x6e1e6a: je 0x6e21d8` does.
                //
                // **The player's arm only.** The pet handler's index table (`0x6e93d0`) is gated
                // `cmp reason,0x8d; ja default` and carries no `0x31` entry, so a pet's exotic-ammo
                // refusal takes its generic arm `0x6e936a` and shows the bare template — unlike
                // `0x19`–`0x1b` just above, which the pet's own table *does* fill (`0x6e904d`).
                // Same shape as `0x78`/`0x5c` below, and the same trap decision 2033 named.
                //
                // **It declines on a vmangos server, every time**, and that is the server's shape
                // rather than a gap here: `Spell::SendCastResult` fills `failureArg1` for
                // NOT_READY, REQUIRES_SPELL_FOCUS, REQUIRES_AREA and the EQUIPPED_ITEM_CLASS
                // family and for nothing else, so `0x31` arrives with no word and the line reads
                // "Requires exotic ammo:". Modeled anyway because the arm is the mechanism and the
                // word is the server's to supply (decision 2292).
                if let (false, 0x31, Some(arg), Some(subs)) =
                    (pet, reason, fail.arg, sub_classes.as_deref())
                {
                    const ITEM_CLASS_PROJECTILE: u32 = 6;
                    if let Some(name) = subs
                        .0
                        .requirement_display_name(ITEM_CLASS_PROJECTILE, 1u32 << (arg & 31))
                    {
                        let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                        return get(key)
                            .filter(|s| !s.is_empty())
                            .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
                    }
                }
                // `0x78` TOTEMS / `0x5c` REAGENTS — **the player's arms only**. The pet handler's
                // index table (`0x6e93d0`) sends both to its generic arm, so a pet's refusal takes the
                // shared passthrough below and never runs the bag walk or the item query (decision
                // 2033).
                if !pet && (reason == 0x78 || reason == 0x5c) {
                    let d = d?;
                    let failing = if reason == 0x78 {
                        self_store
                            .and_then(|s| first_missing_totem(d, s, &objects))
                            // No store to test against (a race): name the first tool at all.
                            .or_else(|| d.totems.iter().copied().find(|&t| t != 0))
                    } else {
                        self_store
                            .and_then(|s| first_short_reagent(d, s, &objects))
                            .or_else(|| d.reagents.iter().map(|&(id, _)| id).find(|&id| id != 0))
                    }?;
                    let cached = items
                        .template(failing, 0, &commands)
                        .map(|i| i.name.clone());
                    let name = match cached {
                        Some(name) => name,
                        // Answered-unknown → the ref's callback fallback literal (`0x838044`);
                        // still pending → keep the entry queued for the redisplay.
                        None if items.template_answered_unknown(failing) => "UNKNOWN".to_string(),
                        None => {
                            // The cache-miss re-queue — and it comes back MARKED, because the
                            // reference's retry is a different raise (see `CastFail::redisplay`).
                            await_template.push(fail.requeued());
                            return None;
                        }
                    };
                    let key = if reason == 0x78 {
                        "SPELL_FAILED_TOTEMS"
                    } else {
                        "SPELL_FAILED_REAGENTS"
                    };
                    return get(key)
                        .filter(|s| !s.is_empty())
                        .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
                }
                cast_fail::cast_fail_text(
                    caster,
                    reason,
                    d,
                    cast_fail::FailArgs {
                        arg: fail.arg,
                        ..fail_args
                    },
                    &get,
                )
            })();
            // The retest instrument for this whole bug class (decision 1313). A red-line defect is
            // reported as *seen* — B255 arrived as a screenshot of the word "Requires" — and until
            // this line the only way to read what the client resolved was to look at the screen.
            // Logging the reason, its wire argument and the resolved line makes an argument arm
            // that silently declined (a missing word, an unnamed id) legible from a probe run.
            // The line's `Debug` carries BOTH buffers, so a probe also reads the toast string
            // beside the log string — the one reading that makes 2285's class of drift visible
            // without opening the chat window.
            debug!(
                "ui_action: cast fail — {caster:?} spell {spell_id} reason {reason:#04x} \
                 arg {:?} → {:?}",
                fail.arg, line
            );
            // **The combat-log twin — the reference's OTHER buffer, not the displayed text**
            // (decision 2285, correcting 2280). It runs after the resolution because the
            // reference's does: `0x6e1a00` calls `DisplayError 0x496720` at `0x6e21dd` and the
            // log formatter `0x62c360` at `0x6e21fc`. But what it hands the formatter is `edi`
            // (`0x6e21e2`), the **argText** buffer — which is the first-layer
            // `SPELL_FAILED_<name>` string for every reason no argument arm claims, *including*
            // the eighteen the second layer re-words on screen. A cooldown refusal toasts "Spell
            // is not ready yet." and logs "Not yet recovered"; `0x09` toasts "You have no target."
            // and logs "No target". [`CastFailLine::logged`] is that one-byte test.
            //
            // The trap this corrects is that deriving the log's `%s` a *second* time, from the
            // key table, gets six of the seven divergent rows right by accident and the filled
            // arms wrong — which is how a raw "Must be in %s" sat in the log while the red line
            // read "Must be in Cat Form". One resolution, two buffers, is the shape that holds.
            //
            // A refusal that resolved to nothing logs nothing, which is the same control flow:
            // `0x17` DONT_REPORT, the keys 5875 leaves out of `GlobalStrings.lua`, and `0x56`'s
            // no-forms exit all jump past `0x62c360` as well as past `DisplayError`.
            //
            // **A redisplay is not a raise.** The item-cache retry (`0x6e29b0`) calls
            // `DisplayError` directly and never reaches the log formatter, so an entry that had
            // to wait for its item name shows the toast alone — `CastFail::redisplay`.
            //
            // **Only the `…SELF` half is reachable here**: `SMSG_CAST_FAILED` is addressed to the
            // caster alone, so benilla never learns that somebody *else's* cast failed — the
            // `…OTHER` keys exist and stay unproduced, exactly as the reference leaves its own
            // two unreachable `…SELFSTART` keys.
            //
            // **The pet's refusal is not logged.** `0x6e1a00` calls the log formatter `0x62c360`
            // beside its `DisplayError`; `Spell_C::HandlePetCastFailed 0x6e8eb0` calls neither it
            // nor the error sound — its entire call set is the two packet readers, `0x496720` and
            // the string plumbing. Before decision 2033 a pet's refused Growl printed "You fail to
            // cast Growl: ..." in the combat log, attributed to the player.
            if let (Some(shown), Some(display), false, false) =
                (line.as_ref(), d, pet, fail.redisplay)
            {
                // `0x62aff0`: `Attributes` bit 4 marks an ABILITY, which "performs" rather than
                // "casts".
                const ATTR_IS_ABILITY: u32 = 0x10;
                let family = if display.attributes & ATTR_IS_ABILITY != 0 {
                    crate::ui_chat::combat::SPELLFAILPERFORM
                } else {
                    crate::ui_chat::combat::SPELLFAILCAST
                };
                if !display.name.is_empty() {
                    fail_lines.push(crate::ui_chat::combat::PendingCombat {
                        kind: crate::ui_chat::ChatEventKind::SpellFailedLocalPlayer,
                        family,
                        variant: crate::ui_chat::combat::Variant::SelfOther,
                        subject: 0,
                        object: 0,
                        fills: crate::ui_chat::combat::Fills {
                            spell: display.name.clone(),
                            named: shown.logged().to_string(),
                            ..Default::default()
                        },
                        named: crate::ui_chat::combat::Named::Ready,
                        tries: 0,
                    });
                }
            }
            line
        })
        .collect();
    cast_errors.0.extend(await_template);
    for line in fail_lines {
        sink.chat.push_combat(line);
    }
    show_messages(
        &mut script,
        &mut sink,
        "ui_action",
        texts.into_iter().map(|l| Shown::keyed(l.key, l.text)),
    );

    // (Dis)mount refusals ride the same route, keyed straight into GlobalStrings
    // ([`mount_result_key`] — no format arguments in any of these strings).
    let mount_texts: Vec<(&'static str, String)> = mount_errors
        .0
        .drain(..)
        .filter_map(|(mount, code)| {
            let key = mount_result_key(mount, code)?;
            Some((key, script.lua().globals().get::<String>(key).ok()?))
        })
        .collect();
    show_messages(
        &mut script,
        &mut sink,
        "ui_action",
        mount_texts
            .into_iter()
            .map(|(key, text)| Shown::keyed(key, text)),
    );

    // Taming refusals ([`PetTameFailures`]) — the one message whose argText is itself a
    // GlobalStrings lookup. The reference resolves the reason's `PETTAME_*` key first
    // (`0x6e6a20`'s `0x703bf0` call) and passes the resulting STRING as `DisplayError(0xee)`'s
    // argument, so `ERR_TAME_FAILED` ("%s.") renders "Creature is too high level for you to
    // tame." — two lookups, in this order, and neither can be folded into the other.
    let tame_texts: Vec<Shown> = pet_tame_failures
        .0
        .drain(..)
        .filter_map(|reason| {
            let reason_key = benilla_protocol::messages::pet_tame_failure_key(reason);
            let reason_text = script.lua().globals().get::<String>(reason_key).ok()?;
            super::keyed_line_s(&script, "ERR_TAME_FAILED", &[&reason_text])
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", tame_texts);

    // Client-local by-key refusals (the `DisplayError` route — [`UiErrorKeys`]); the key IS the
    // GlobalStrings lookup, no code table between, and the key is also what names the surface:
    // [`UiError::kind`] reads the message record straight out of the catalog instead of the queue
    // carrying a hand-set flag alongside every push (decision 1770).
    let key_lines: Vec<Shown> = ui_error_keys
        .0
        .drain(..)
        .filter_map(|e| {
            ui_error_text(&e, &|key| script.lua().globals().get::<String>(key).ok())
                .map(|t| Shown::keyed(e.key, t))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", key_lines);

    // Already-resolved lines ([`UiErrorTexts`]) — the wire's own text, no key to look up and no
    // record behind it; the queued kind IS the reference's `0x4945b0` flag.
    let resolved: Vec<Shown> = ui_error_texts
        .0
        .drain(..)
        .map(|(text, kind)| Shown::unkeyed(kind, text))
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", resolved);

    // The ENGINE's own by-key refusals ride the very same line. `benilla_ui` is engine-free and
    // cannot reach [`UiErrorKeys`], so a refusal raised inside the script crate (today: dropping a
    // passive spell on the bar, `ERR_PASSIVE_ABILITY`) queues its key and we resolve it here —
    // standing in for the reference's inline `push <errorId>; call CGGameUI::DisplayError`. One
    // frame late by construction (the refusal happens during the input pass this feed precedes),
    // which is invisible on a toast.
    let engine_keys = script.take_ui_errors();
    let engine_lines: Vec<Shown> = engine_keys
        .into_iter()
        .filter_map(|key| {
            let e = UiError::key(key);
            ui_error_text(&e, &|k| script.lua().globals().get::<String>(k).ok())
                .map(|t| Shown::keyed(e.key, t))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", engine_lines);

    let store = self_q.iter().next();

    // Stance page: our own descriptor's form byte, pushed on change (UPDATE_BONUS_ACTIONBAR is
    // the client's event for exactly this transition — the bar re-picks its page on it).
    let form = store.map(|s| s.0.unit_shapeshift_form()).unwrap_or(0);
    let offset = spells
        .as_ref()
        .and_then(|s| s.forms.get(&u32::from(form)))
        .map(|f| f.bonus_bar)
        .unwrap_or(0) as u8;
    if offset != memory.bonus_offset {
        debug!("ui_action: bonus bar offset {} (form {form})", offset);
        memory.bonus_offset = offset;
        script.set_bonus_bar_offset(offset);
        script.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    }

    // The identity resolve has TWO inputs, not one. `dirty` covers the action table; the item
    // TEMPLATE cache is the other, and it fills asynchronously — an ITEM slot's icon needs a
    // template that `Items::template` fetches **ask-once**, so the very first resolve of a cold
    // entry is the call that ISSUES the query and it necessarily reads back `None`. Gating on
    // `dirty` alone left that slot on the fallback question mark until some unrelated bar edit
    // happened to re-dirty it — the login race that put a question mark on every fresh
    // character's food/water button (decision 0660; verified live 2026-07-26: the Tough Jerky
    // ask and the one and only feed landed 0.5 ms apart, in that order, and nothing re-fed).
    // The epoch is the second input, so a landed answer redisplays like the ref's DBCACHECALLBACK.
    let template_epoch = items.template_epoch();
    let macro_generation = script.macros_generation();
    let macros_moved = macro_generation != memory.macro_generation;
    if !memory.resolved || actions.dirty || template_epoch != memory.template_epoch || macros_moved
    {
        actions.dirty = false;
        memory.resolved = true;
        memory.template_epoch = template_epoch;
        memory.macro_generation = macro_generation;
        // Cloned once per re-resolve, never per frame: the gate above is a `u64` compare.
        let macros = script.macros();

        // Resolve every occupied wire slot to its display, diff against what the VM holds, push +
        // fire ACTIONBAR_SLOT_CHANGED (arg1 = the Lua action id) per transition. Item icons/counts
        // resolve via the same ask-once template chain + bag walk the bags use
        // (`ui_items::count_of`) — an in-flight template shows the fallback (no texture), and the
        // epoch gate above re-runs this whole resolve the frame the answer lands.
        let mut fresh: HashMap<u32, ActionSlot> = HashMap::new();
        for (slot, button) in &actions.buttons {
            let (texture, count, consumable) = match button.kind {
                ACTION_KIND_SPELL => {
                    let icon = spells.as_ref().and_then(|sp| {
                        let d = sp.catalog.get(button.action)?;
                        spell_action_icon(
                            button.action,
                            d,
                            sp,
                            store,
                            &objects,
                            &items,
                            icons.as_deref(),
                            &commands,
                        )
                    });
                    (icon, 0, false)
                }
                ACTION_KIND_ITEM => {
                    // The question mark belongs HERE, not in the Lua (decision 0666, correcting
                    // 0660's modeling note): the reference's resolver never returns nil for a
                    // populated ITEM slot — an un-cached template (displayId 0) or a displayId
                    // with no row both fall into `0x5d88b0`'s failure block `0x5d8927`, which
                    // returns the hardcoded `INV_Misc_QuestionMark` at `0x847fe4`. Since ref
                    // FrameXML *hides* the icon on a nil texture, feeding nil here and letting a
                    // Lua `or` paint the fallback would show a BLANK button on faithful
                    // FrameXML — the placeholder is the engine's, at two sites binary-wide.
                    let template = items.template(button.action, 0, &commands).cloned();
                    let texture = template
                        .as_ref()
                        .and_then(|t| icons.as_ref()?.catalog.get(t.display_info_id)?.icon.clone())
                        .unwrap_or_else(|| MISSING_ITEM_ICON.to_string());
                    let count = store
                        .map(|s| count_of(&s.0, &objects, button.action, InventoryScope::CARRIED))
                        .unwrap_or(0);
                    // The Count fontstring's gate — `IsConsumableAction 0x4e5250`: ammo/thrown by
                    // InventoryType, or an ON_USE block with NEGATIVE charges
                    // ([`ItemInfo::is_consumable`], byte-cited there; decision 0926 §3). It comes
                    // from the SAME ask-once template the icon does, so it belongs on the same
                    // push: fed from the per-frame state map instead, it answered the Lua one
                    // frame late for ever and left a fresh character's food with no stack number
                    // (decision 1301 — the count's half of 0660's login race).
                    let consumable = template.as_ref().is_some_and(|t| t.is_consumable());
                    (Some(texture), count, consumable)
                }
                // A MACRO slot serves **the macro's own icon, never its bound spell's** — the one
                // asymmetry in the icon resolver: `0x4e6a50`'s macro arm (`0x4e6bf9`) validates the
                // slot and calls `0x4f0fd0(idx, buf, 0x104)`, the macro record's own icon-path
                // builder, without ever touching `[rec+0x564]`. Its dynamic state DOES go
                // through the bound spell — that split is the whole design (`state`'s macro
                // arm, decision 0983).
                ACTION_KIND_MACRO => (
                    macros
                        .get(button.action as usize)
                        .and_then(|m| m.texture.clone()),
                    0,
                    false,
                ),
                _ => (None, 0, false),
            };
            fresh.insert(
                u32::from(*slot) + 1,
                ActionSlot {
                    texture,
                    kind: button.kind,
                    action: button.action,
                    count,
                    consumable,
                },
            );
        }
        // A MACRO slot's observable is wider than its `ActionSlot`: the name line under the icon
        // reads the macro table through `GetActionText` at repaint (decision 1636), so a rename —
        // which moves the table and nothing in the slot value — must re-fire the slot exactly as
        // a re-icon does, or the bar keeps the old name until an unrelated edit repaints it.
        let changed: Vec<u32> = fresh
            .keys()
            .chain(memory.pushed.keys())
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|a| {
                fresh.get(a) != memory.pushed.get(a)
                    || (macros_moved && fresh.get(a).is_some_and(|s| s.kind == ACTION_KIND_MACRO))
            })
            .collect();
        for &action in &changed {
            script.set_action(action, fresh.get(&action).cloned());
        }
        memory.pushed = fresh;
        debug!(
            "ui_action: fed {} changed slot(s) ({} occupied)",
            changed.len(),
            memory.pushed.len()
        );
        for action in changed {
            script.fire_event(
                "ACTIONBAR_SLOT_CHANGED",
                vec![ScriptValue::Int(i64::from(action))],
            );
        }
    }

    // Two things drift independently of `dirty` (decision 0216 §7's module-doc note) and so must
    // refresh every frame, not just on an action-table edit: an ITEM slot's COUNT (eating down a
    // stack never touches SMSG_ACTION_BUTTONS) and the auto-attack's ICON (it tracks the equipped
    // main-hand weapon, which a weapon swap changes without touching the action table — decision
    // 0230). Gating either on `dirty` leaves it stale until the next unrelated action-bar edit.
    // Bounded to the already-pushed slots (normally a handful) — the same per-frame bag walk /
    // template lookup `count_of` already pays for the quest log's item objectives.
    if let Some(store) = store {
        for (&action, slot) in memory.pushed.iter_mut() {
            let changed = match slot.kind {
                ACTION_KIND_ITEM => {
                    let fresh = count_of(&store.0, &objects, slot.action, InventoryScope::CARRIED);
                    let changed = fresh != slot.count;
                    if changed {
                        slot.count = fresh;
                    }
                    changed
                }
                // Two icon families track live character state, not the action table, so they
                // refresh every frame: a weapon-substituting icon follows the equipped weapon
                // AND the current form (Attack's `0x4e6870` — decisions 0230/0231 + the form
                // face), and a toggle spell with a nonzero ActiveIconID swaps faces with its own
                // aura (`0x4e6a50`'s `0x4e6bbd` predicate — a shift in/out never touches
                // SMSG_ACTION_BUTTONS). A plain spell's icon is stable, so it's skipped.
                ACTION_KIND_SPELL => {
                    let d = spells
                        .as_ref()
                        .and_then(|s| s.catalog.get(slot.action))
                        .filter(|d| substitutes_weapon_icon(d) || d.active_icon_id != 0);
                    match (d, spells.as_ref()) {
                        (Some(d), Some(sp)) => {
                            let fresh = spell_action_icon(
                                slot.action,
                                d,
                                sp,
                                Some(store),
                                &objects,
                                &items,
                                icons.as_deref(),
                                &commands,
                            );
                            let changed = fresh != slot.texture;
                            if changed {
                                debug!(
                                    "ui_action: live icon swap slot {action} ({}) -> {fresh:?}",
                                    slot.action
                                );
                                slot.texture = fresh;
                            }
                            changed
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            if changed {
                script.set_action(action, Some(slot.clone()));
                script.fire_event(
                    "ACTIONBAR_SLOT_CHANGED",
                    vec![ScriptValue::Int(i64::from(action))],
                );
            }
        }
    }
}

/// The whole SPELL-slot icon rule — the reference's `GetActionTexture` resolver `0x4e6a50`, arms
/// in its execution order:
///
/// 1. The **pre-emptive arms** ([`auto_attack_icon`]): Attack serves the current form's face /
///    the main-hand weapon (`0x4e6870`), an auto-repeat shot the ranged weapon (`0x4e6990`) —
///    before the spell's own icon fields are ever read.
/// 2. The **active-toggle swap** (`0x4e6bbd → 0x4e6bc6`): `ActiveIconID` while the button's OWN
///    spell id sits live-and-cancelable in the player's aura slots — the literal `0x4e55f0`
///    predicate `UseAction`'s cancel fork rides ([`super::toggle::active_action_toggle`]; the
///    same function in the binary, by call-target address). Ghost Wolf's swirl while shifted.
/// 3. The spell's own `SpellIconID` face.
///
/// The spellbook's `GetSpellTexture` (`0x4b3f50`) deliberately runs ONLY arms 1 and 3 — it never
/// serves `ActiveIconID` (proof by exhaustion of its return paths). `ui_spellbook` keeps that
/// asymmetry; do not "fix" it to match the bar.
fn spell_action_icon(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    spells: &super::Spells,
    store: Option<&crate::net::ObjectStore>,
    objects: &crate::net::Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Option<String> {
    auto_attack_icon(d, store, &spells.forms, objects, items, icons, commands)
        .or_else(|| {
            store
                .filter(|s| super::toggle::active_action_toggle(spell_id, d, s))
                .and_then(|_| d.active_icon.clone())
        })
        .or_else(|| d.icon.clone())
}
