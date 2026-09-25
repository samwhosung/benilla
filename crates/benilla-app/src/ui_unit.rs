//! The app-side **unit snapshot + event feed**: the bridge that turns live ECS
//! game state into the plain data the engine-free `Unit*` Lua bindings read, and into the WoW events
//! that drive a frame's `OnEvent`.
//!
//! The architecture is deliberate: the Lua game-state API must **not** reach
//! into the ECS. Instead this runs each frame, *before* the VM's tick/event dispatch
//! ([`crate::ui_script::UiInput`]), and pushes a [`UnitState`] snapshot for each unit token into the
//! VM via [`UiScript::set_unit`]. The `"player"` token reads our own avatar's [`ObjectStore`] (tagged
//! [`SelfPlayer`]); `"target"` reads the [`Selection`]'s entity. Both are found by their ECS entity,
//! not by re-deriving a guid — the ECS already owns the guid↔entity map. `"targettarget"` is the one
//! token here that IS reached by a guid: the target's own `UNIT_FIELD_TARGET`, looked up in the same
//! index.
//!
//! Names come from the [`crate::names::NameCache`] (the 1.12 wire has no descriptor names — the
//! query-cache seam): the feed resolves each token's guid, which asks the server once on a miss and
//! fills in a later frame; the transition fires `UNIT_NAME_UPDATE` so frames repaint.
//!
//! The event surface is fired per field on transitions: `UNIT_HEALTH`/`UNIT_MAXHEALTH`/
//! `UNIT_LEVEL` (arg1 = token), the **1.12** per-resource power pair `UNIT_MANA`/`UNIT_RAGE`/
//! `UNIT_FOCUS`/`UNIT_ENERGY`/`UNIT_HAPPINESS` and their five `UNIT_MAX*` twins (arg1 = token; the
//! resource is in the NAME, not an argument — decision 1819, which retired the Era
//! `UNIT_POWER_UPDATE`/`UNIT_MAXPOWER` pair that no 1.12 frame listens for),
//! `UNIT_DISPLAYPOWER` (power *type* changed), `UNIT_NAME_UPDATE`, plus
//! `PLAYER_ENTERING_WORLD` once and `PLAYER_TARGET_CHANGED` on selection change. A token appearing
//! counts as a transition of every present field (frames also pull on target change, so either path
//! populates).

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::ChrClasses;
use benilla_protocol::messages::ObjectType;
use benilla_ui::script::{power_token, ScriptValue, UiScript, UnitState, WornDisplay};

use crate::names::NameCache;
use crate::net::{
    FieldChanged, FieldEdges, Guid, NetCommands, ObjectStore, Reputations, SelfPlayer,
};
use crate::target::{ring_reaction, Factions, Selection};
use crate::ui_script::{gate, UiInput};

/// The unit-feed pass — the GATED sub-phase of [`crate::ui_script::UiFeed`], which carries the
/// order: after [`benilla_world::schedule::WorldStage::Net`] and before [`UiInput`], so the
/// snapshot + events it produces are in place when the VM ticks and dispatches this frame. That
/// order was found here first (the feeds snapshot state the net apply writes; unordered,
/// `apply_net_updates` could land BETWEEN two feeds, and a synchronous event fired by the later
/// one then re-read the earlier one's pre-mutation push — the spellbook's cooldown pie stayed
/// cold until a manual reopen, reproduced live 2026-07-31), and decision 2304 made it every
/// feed's. What stays this set's own is the gate: every member either fires a login one-shot or
/// latches a per-VM memo, so none may run before the in-game interface exists (1348). A named
/// set so the demo override ([`crate::ui_script`]) can order itself after it. Configured in
/// [`UiUnitPlugin`] — the set's home.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UnitFeed;

/// One combat occurrence over a unit — the `UNIT_COMBAT` event feed (the portrait
/// hit indicator's wire; the shipped `CombatFeedback.lua` is the consumer). The one emitter
/// `0x494600` fires
/// `(token, action, descriptor, amount, type)` once per live token the unit maps to, with **no
/// self-suppression and no cvar gate** — the worldtext Gate A's inverse. `type` (arg5) is the
/// damage school on the melee/spell-damage paths; the miss and heal wrappers hard-code 0. The
/// melee victim event is **deferred to the swing impact keyframe** (it rides inside `0x6243e0`,
/// reached only from the `0x624530` victim dispatcher). ENERGIZE never fires in
/// 5875 (string absent binary-wide). Producers: melee at [`melee_unit_combat`], spells/heals at
/// packet receive (`net/apply/combat_log.rs`). Consumed by [`fire_unit_combat`], which resolves
/// the entity to its live unit tokens.
#[derive(Message, Clone, Copy)]
pub(crate) struct UnitCombatFeedback {
    pub(crate) unit: Entity,
    /// `arg2` — the action: `WOUND`/`MISS`/`DODGE`/`PARRY`/`BLOCK`/`EVADE`/`IMMUNE`/`DEFLECT`/
    /// `RESIST`/`ABSORB`/`REFLECT`/`HEAL`/`ENERGIZE`.
    pub(crate) action: &'static str,
    /// `arg3` — the descriptor: `CRITICAL`/`CRUSHING`/`GLANCING`/`ABSORB`/`BLOCK`/`RESIST`, or `""`.
    pub(crate) flags: &'static str,
    /// `arg4` — the amount (damage/heal/energize; 0 for pure words).
    pub(crate) amount: u32,
    /// `arg5` — the school int (0 = physical; the Lua's `type > 0` draws the number spell-yellow).
    pub(crate) school: u32,
}

/// One center-combat-text message — the `COMBAT_TEXT_UPDATE` event feed (the
/// Blizzard_CombatText transcription is the consumer). Event id 0x21E, fired via the
/// formatted SignalEvent `0x703f50` from the UnitCombatLog_C.cpp emit helpers — every producer
/// fires **at packet parse** (the melee one too: `0x6255b0 → 0x629d30`, one call stack — NOT the
/// impact-keyframe deferral, which belongs to the worldtext/UNIT_COMBAT victim dispatch).
/// `message_type` is the addon's vocabulary (`DAMAGE`/`DAMAGE_CRIT`/`SPELL_DAMAGE`/`HEAL`/…);
/// `data`/`extra` mirror `arg2`/`arg3` (all strings on the real wire — the fmt is `"%s..%d.."`).
/// Producers gate on the SELF recipient — the ref's emit is co-gated with the chat combat-log
/// category scope (participants beyond self CAN fire it there); the exact participant rule is an
/// open residual, and self-only is the display-equivalent conservative cut.
#[derive(Message, Clone)]
pub(crate) struct CombatTextEvent {
    pub(crate) message_type: &'static str,
    pub(crate) data: Option<String>,
    pub(crate) extra: Option<String>,
}

/// The feed's change-tracking memory: what we last told the VM, plus one server-side log-once.
///
/// **`PLAYER_LEAVING_WORLD` on a cross-map worldport** (corrected by 2238).
///
/// The reference fires event `0x111` at `0x490b48`, inside `0x490a80`. A census of both
/// signal helpers puts the id at exactly one site image-wide — 336/336 ids resolved through
/// `0x703e50`, 149/149 through `0x703f50`, and the encoding `b9 11 01 00 00` occurs once in the
/// binary — so that is the whole of the FIRE, and it is what this doc used to conflate with the
/// whole of the event.
///
/// **One fire site, three callers, and this system is one of them** (2238).
/// `0x490a80` is reached from the local player object's own destructor (`0x401bc0` → `0x467700`
/// → `0x467800` → the per-object `[vtbl+0]` → `0x5dd500` → `0x5dd600` → `0x5dd72c` → `0x5dd543`)
/// — **this system's occasion** — and also from `0x490c20` inside the shutdown tail `0x490bd0`,
/// which is where an in-world `/reload`, a logout, a quit and a disconnect reach it, and from
/// `0x5e9b5a`, vtable slot 1, on a DESTROY / OUT_OF_RANGE of the local player object.
/// 2235 read the "three gates" as three gates on the event, which they are not. Two of those
/// gates (`0x5dd71c`/`0x5dd721`/`0x5dd725` and `0x5dd539`/`0x5dd53e`/`0x5dd541`) sit in the
/// destructor chain and gate only the caller below; `0x490a80`'s own only gate is the latch at
/// `[0xb4b424]`.
///
/// **The tail's occasions are already ours**, and were years before this system existed:
/// [`crate::ui_script::shutdown_ui_state`] fires `PLAYER_LEAVING_WORLD` then `PLAYER_LOGOUT` as
/// the head of the same `0x490bd0` tail, from `end_ui_session` (logout, disconnect, and
/// `run_pending_reload`'s `/reload`) and from `shutdown_on_exit` (quit). So the two producers are
/// complements, not duplicates — a fact worth writing down precisely because nothing in either
/// file said so, and a reader of this doc alone would take the tail's fire for a bug.
///
/// **Cross-map only *for this occasion*, and that falls out of the gate rather than being a rule
/// on top of it.** The first gate (`0x5dd728`) admits only the local player's destructor, and a
/// same-map teleport never destroys the object — including the >30 yd variant that forces a
/// blocking terrain reload, which reloads tiles rather than `CGObject`s. So
/// [`crate::net::WorldportMessage`] is the right edge and `needs_ack` is the right discriminator:
/// the one worldport that does NOT need an ack is the initial-login map, where nothing is being
/// left.
///
/// **Nothing shipped listens to it.** Zero of the 232 extracted reference interface files
/// register `PLAYER_LEAVING_WORLD` (controls, same sweep: `PLAYER_ENTERING_WORLD` 22 files,
/// `VARIABLES_LOADED` 7, `PLAYER_LOGIN` 1). This is an addon-facing event, which is exactly why
/// it went missing here for so long — no stock window breaks without it, and only the corpus
/// notices. It is also why this stays a bare fire and promises nothing more: in the reference the
/// handler runs *during* teardown, after the three manager unlinks and ~35 of `0x490a80`'s 37
/// teardown calls, so a real addon's handler already sees a substantially dismantled UI.
///
/// **What "dismantled" costs there is the opposite of what 2235 guessed** (2238). The GUID
/// hash's link is `obj+0x1c` and the destructor splices the object out
/// (`0x467887 call 0x468680`) *before* `call [vtbl+0]`, so on THIS occasion — and only this one —
/// a hash lookup misses. But `UnitName("player")` never asks the hash: `0x517020` short-circuits
/// at `0x51707d` and answers from the cached character record `[0xc27d88]`, which has one writer
/// (`CGlueMgr::EnterWorld`) and no clearer; `UnitRace`, `UnitClass` and `0x517ee0` take the same
/// fast path. The binding that *does* go nil is `UnitExists("player")`, whose `0x515970` resolves
/// through `0x468460` and takes `0x5159c9 je 0x515a39` → `0:0` on the miss. We reproduce none of
/// that ordering and are not trying to: ours fires with the descriptor still present, so every
/// unit binding answers. That is a divergence in our favour, recorded rather than closed —
/// closing it would mean deliberately breaking `UnitExists` to match a teardown artifact no
/// stock file observes.
fn fire_leaving_world_on_worldport(
    script: Option<NonSendMut<UiScript>>,
    mut armed: ResMut<crate::ui_script::LeavingWorldArmed>,
    mut ports: MessageReader<crate::net::WorldportMessage>,
) {
    // `needs_ack` false is the initial-login map (`player::wire_in`'s own split): an entry, not a
    // departure. Read the whole iterator either way so the cursor never carries one over.
    let leaving = ports.read().filter(|w| w.needs_ack).count() > 0;
    if !leaving {
        return;
    }
    // **The world latch, spent here** (2239): this producer and the shutdown tail are the
    // reference's `0x5dd543` and `0x490c20`, two callers of one fire site, and `[0xb4b424]` is
    // what keeps them from both claiming one departure. Spent even if the VM turns out to be
    // absent below — the reference clears it at `0x490a8d`, ahead of the fire and of every
    // teardown call after it, so a departure nobody could be told about is still a departure.
    if !armed.spend() {
        return;
    }
    let Some(mut script) = script else {
        return;
    };
    script.fire_event("PLAYER_LEAVING_WORLD", Vec::new());
}

/// The VM half lives behind a [`crate::ui_script::VmMemo`], **inside the resource** — the same
/// law 1290 wrote for `Local` memos, reached the way a `ResMut` system has to reach it: a memory
/// about what THIS VM was told is unreadable against the next VM, so a `/reload` (1291) — which
/// replaces the VM without despawning the world — re-fires `PLAYER_ENTERING_WORLD` and re-runs
/// every transition diff exactly as a fresh login does. Before this, every one of these fields
/// survived the reload and the new VM never heard the events (the logout path re-armed off the
/// self descriptor despawning, which a reload never does).
#[derive(Resource, Default)]
struct UnitFeedState {
    /// What we last told the VM — dies with the VM it was told to.
    vm: crate::ui_script::VmMemo<UnitFeedMemo>,
    /// Whether we have already warned that our own faction template names no side.
    /// Re-arms when a side resolves again, so a `.gm on` / `.gm off` cycle logs once each way.
    /// **Server memory, not VM memory** — deliberately outside the memo: a `/reload` must not
    /// re-log the GM-mode warning.
    warned_sideless: bool,
}

/// The per-VM half of [`UnitFeedState`] — the event-trigger diffs.
#[derive(Default)]
struct UnitFeedMemo {
    /// The gate's counter memories (1439) — the two lazy caches this feed resolves through
    /// (their per-frame `&mut` misses poison `is_changed`, the counters carry the landings).
    names_generation: gate::Watch,
    guild_generation: gate::Watch,
    /// Whether `PLAYER_ENTERING_WORLD` has been fired (once per world entry, once per VM).
    entered_world: bool,
    /// Per token, the last snapshot we pushed — the per-field event triggers diff against it.
    last: HashMap<String, UnitState>,
    /// The last selection guid, for the `PLAYER_TARGET_CHANGED` trigger.
    target_guid: Option<u64>,
    /// The last `(PLAYER_XP, PLAYER_NEXT_LEVEL_XP)` pair pushed, for the `PLAYER_XP_UPDATE` trigger —
    /// the XP bar's feed is a player-global (like coinage), not a per-unit-token field.
    last_xp: Option<(u32, u32)>,
    /// The last `(restState, restPool, PLAYER_FLAGS)` triple pushed, for the `UPDATE_EXHAUSTION`,
    /// `PLAYER_UPDATE_RESTING` and `PLAYTIME_CHANGED` triggers — player-globals like the XP pair.
    /// Pushed as one snapshot so
    /// `GetRestState`/`GetXPExhaustion`/`IsResting` never read it half-updated.
    ///
    /// The whole flags dword is kept because the two flag events read **different bits of it**
    /// (`0x20` and `0x1000|0x2000`), not because either fires on the word as a whole — 2078
    /// corrected that, and the feed XORs against this to find which arm moved.
    last_rest: Option<(u8, u32, u32)>,
    /// Our avatar's last-seen `UNIT_FIELD_LEVEL`, for the `PLAYER_LEVEL_UP` trigger (decision
    /// 1094). `None` until first seen — the first sighting is the login descriptor, not a ding.
    last_level: Option<u32>,
    /// The last `(count, banked-target guid)` pair pushed, for the `PLAYER_COMBO_POINTS` trigger —
    /// player-globals (`PLAYER_FIELD_BYTES` byte 1 + `PLAYER_FIELD_COMBO_TARGET`), not per-unit-
    /// token fields. Diffed as a pair because the server writes them as one.
    last_combo: Option<(u8, u64)>,
    /// The self unit's last in-combat flag, for the `PLAYER_REGEN_DISABLED`/`ENABLED` triggers
    /// (`None` until first seen — first sight fires only when already IN combat, so logging in
    /// at peace never announces "Leaving Combat").
    in_combat: Option<bool>,
    /// The self unit's last `PLAYER_FLAGS_PVP_DESIRED` bit, for the toggle announcement.
    /// `None` until first seen: the reference reacts to a *changed*-bits mask, so
    /// the descriptor that first carries the flag at login announces nothing.
    pvp_desired: Option<bool>,
    /// The self unit's last `(HIDE_HELM, HIDE_CLOAK)` pair, for the worn-display push (decision
    /// 1472). Pushed on the **edge** and never per frame: the Options row's setter flips the VM's
    /// belief optimistically and the server's answer is a round trip away, so a per-frame push
    /// would snap the value back to the stale descriptor in between.
    worn_hidden: Option<(bool, bool)>,
    /// The self player's last-pushed `PLAYER_FIELD_BYTES` byte 2 — the four extra bars' visibility
    /// (read by `GetActionBarToggles 0x4e7660`). A player-global like the combo pair, pushed on the
    /// edge.
    /// `None` until first seen; there is no event to fire on it, because the real client registers
    /// no field-change callback anywhere near this offset.
    action_bar_toggles: Option<u8>,
}

/// Adds the per-frame unit feed. The `Unit*` bindings themselves live in `benilla-ui`; this only
/// supplies their data (and the events) from ECS state.
pub(crate) struct UiUnitPlugin;

impl Plugin for UiUnitPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            Update,
            UnitFeed
                // A sub-phase of the feed phase, which carries the order (the set's own doc)…
                .in_set(crate::ui_script::UiFeed)
                // …and never before the in-game UI exists (1348). The whole SET, not just
                // `feed_units`: every feed in it either fires a login one-shot or latches a
                // per-VM memo, and both are lost forever against the boot VM. The window and
                // the reference's own ordering: `ui_script::ingame_ui_up`.
                .run_if(crate::ui_script::ingame_ui_up),
        )
        .init_resource::<UnitFeedState>()
        // [`feed_units`] shows catalog messages (the rest-state pair, the PvP toggle) through
        // `ui_action::show_messages`, whose sink is the chat log and the message-sound queue.
        // The queue belongs to the sound stack, which a UI-only harness does not stand up, and a
        // missing `ResMut` is a system-validation panic — so this plugin declares it. `init` is a
        // no-op when the sound plugin has already put it there.
        .init_resource::<crate::sound::MessageSounds>()
        .add_message::<UnitCombatFeedback>()
        .add_message::<CombatTextEvent>()
        // …and the worldport edge [`fire_leaving_world_on_worldport`] reads, for exactly the
        // reason above: the message belongs to `crate::net`, which a UI-only harness does not
        // stand up, and an unregistered `MessageReader` is a system-validation panic rather than
        // an empty read. `add_message` is idempotent, so the net plugin declaring it too costs
        // nothing. (1348's own `the_login_one_shots_wait_for_the_in_game_ui` is the harness that
        // found this — it builds this plugin alone.)
        .add_message::<crate::net::WorldportMessage>()
        .add_message::<crate::net::FieldChanged>()
        // The world latch (2239): this plugin owns one of its two producers, so it declares the
        // resource as well as the message — same reason, and `init_resource` is idempotent
        // against `UiScriptPlugin`'s own.
        .init_resource::<crate::ui_script::LeavingWorldArmed>()
        .add_systems(
            Update,
            (
                // FIRST in the chain, so a worldport's leaving edge precedes the entering edge
                // the same port raises in `feed_units` once the new descriptor lands.
                fire_leaving_world_on_worldport,
                feed_units,
                feed_unit_reach,
                feed_player_control,
                feed_farsight_focus,
                melee_unit_combat,
                fire_unit_combat,
                fire_combat_text,
            )
                .chain()
                .in_set(UnitFeed),
        )
        .add_systems(Update, drain_pvp_toggles.after(UiInput))
        .add_systems(Update, drain_worn_display_toggles.after(UiInput))
        .add_systems(Update, drain_action_bar_toggles.after(UiInput))
        .add_systems(Update, feed_default_language.in_set(UnitFeed))
        .add_systems(Update, feed_known_languages.in_set(UnitFeed))
        // `load_exhaustion_rows` pushes into the VM, so it runs per VM in `Update` (1290), in
        // the feed phase; `load_default_languages` only builds a Bevy resource and stays a
        // one-shot.
        .add_systems(
            Update,
            load_exhaustion_rows.in_set(crate::ui_script::UiFeed),
        )
        .add_systems(PostStartup, (load_default_languages, load_languages));
    }
}

/// The race → default-chat-language join, loaded once ([`benilla_formats::DefaultLanguages`]).
/// Absent when the tables would not load — `GetDefaultLanguage()` then answers the reference's own
/// zero-values shape rather than a made-up word.
#[derive(Resource)]
pub(crate) struct DefaultLanguagesRes(pub(crate) benilla_formats::DefaultLanguages);

/// Load `Languages.dbc` × `ChrRaces.dbc` once at startup ([`load_exhaustion_rows`]'s shape).
/// `Languages.dbc` in row order — the walk behind `GetNumLaguages`/`GetLanguageByIndex`.
#[derive(Resource)]
pub(crate) struct LanguagesRes(pub(crate) benilla_formats::Languages);

fn load_languages(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_languages(&mut chain)
    };
    match loaded {
        Ok(langs) => {
            info!("ui_unit: {} Languages.dbc rows", langs.len());
            commands.insert_resource(LanguagesRes(langs));
        }
        Err(e) => warn!("ui_unit: Languages.dbc unavailable — {e:#}"),
    }
}

/// The languages this character **knows**, folded the reference's way and fed to the VM for
/// `GetNumLaguages`/`GetLanguageByIndex`:
///
/// 1. `0x4b25b0` runs on spell add and stores `[languageId] = spellId` for a spell whose
///    `Effect_1 == 39` — so the table holds only languages *this character's known spells*
///    declare ([`benilla_formats::SpellCatalog::declared_language`], spell → language, never the
///    other way round: five shipped language spells declare Common, and that anomaly is the
///    reference's).
/// 2. `GetNumLaguages` walks `Languages.dbc` rows and keeps those `0x5ec720` answers non-zero
///    for: the language's spell resolves to a `SkillLine` (`0x6de040`, race/class, spell) that is
///    present in the player's `PLAYER_SKILL_INFO` block. Presence, not value: a found line
///    returns 1 whatever its value.
///
/// One knowingly-unreproduced detail: a later learn overwrites an earlier spell on the same
/// language id in the reference's table. The known-spell set here is unordered, so when two
/// known spells declare one language, either's skill line passes — unobservable on shipped data
/// (every language skill a character holds is 300, and the only shared id is Common's).
pub(crate) fn known_languages(
    known: impl IntoIterator<Item = u32>,
    spells: &benilla_formats::SpellCatalog,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    has_skill_line: impl Fn(u32) -> bool,
    languages: &benilla_formats::Languages,
) -> Vec<String> {
    let mut declared: std::collections::HashMap<u32, Vec<u32>> = Default::default();
    for spell in known {
        if let Some(lang) = spells.declared_language(spell) {
            declared.entry(lang).or_default().push(spell);
        }
    }
    languages
        .names(0)
        .filter(|(id, _)| {
            declared.get(id).is_some_and(|spells| {
                spells.iter().any(|&spell| {
                    skill_lines
                        .and_then(|sl| sl.spell_to_line(spell))
                        .is_some_and(&has_skill_line)
                })
            })
        })
        .map(|(_, name)| name.to_string())
        .collect()
}

fn feed_known_languages(
    script: Option<NonSendMut<UiScript>>,
    actions: Option<Res<crate::ui_action::PlayerActions>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    languages: Option<Res<LanguagesRes>>,
    self_q: Query<Ref<ObjectStore>, With<SelfPlayer>>,
    mut pushed: Local<crate::ui_script::VmMemo<Option<Vec<String>>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (Some(actions), Some(spells), Some(languages)) = (actions, spells, languages) else {
        return;
    };
    let pushed = pushed.get(&script);
    let store = self_q.iter().next();
    // A pure function of the spell book, the three catalogs and our descriptor — with all still
    // and a push already made on this VM, the rebuild (a skill-slot scan per language, a
    // `Vec<String>`) can only reproduce the memo.
    let inputs_moved = store.as_ref().is_some_and(|s| s.is_changed())
        || actions.is_changed()
        || spells.is_changed()
        || languages.is_changed()
        || skill_lines.as_ref().is_some_and(|l| l.is_changed());
    if pushed.is_some() && !inputs_moved {
        return;
    }
    let store: Option<&ObjectStore> = store.as_deref();
    let has_skill_line = |line: u32| {
        store.is_some_and(|s| {
            (0..benilla_protocol::messages::PLAYER_SKILL_SLOTS)
                .filter_map(|i| s.0.player_skill(i))
                .any(|slot| u32::from(slot.skill_id) == line)
        })
    };
    let names = known_languages(
        actions.spells.iter().copied(),
        &spells.catalog,
        skill_lines.as_deref().map(|s| &s.catalog),
        has_skill_line,
        &languages.0,
    );
    if pushed.as_ref() != Some(&names) {
        script.set_known_languages(names.clone());
        *pushed = Some(names);
    }
}

fn load_default_languages(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    let Some(assets) = assets else { return };
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_default_languages(&mut chain)
    };
    match loaded {
        Ok(langs) => {
            info!("ui_unit: {} race → default-language rows", langs.len());
            commands.insert_resource(DefaultLanguagesRes(langs));
        }
        // Not fatal: the binding's contract already has an answer for "no table".
        Err(e) => warn!("ui_unit: default languages unavailable — {e:#}"),
    }
}

/// **The player's default language goes into the VM at its birth** (through the
/// seam 2240 established) — because `GetDefaultLanguage()` is read *inside* the load burst, and
/// until now the answer during that burst was `nil` and then a race.
///
/// Two readers, both stock: `ChatEdit_OnLoad` takes it at OnLoad (dead in 1.12, but it is the
/// era's idiom), and `ChatFrame_OnEvent`'s `PLAYER_ENTERING_WORLD` arm stores it as
/// `this.defaultLanguage`, which gates the `[Common]`/`[Orcish]` prefix on every readable chat
/// line for the session. We fire `PLAYER_ENTERING_WORLD` from [`feed_units`] — an `Update` system
/// in the same set as [`feed_default_language`], with no ordering between them — so whether that
/// gate was seeded when the event arrived was Bevy's intra-set order to decide, per run.
///
/// The race comes from the **roster row** rather than the object store, because the avatar does not
/// exist yet at this edge; it is the same value from the same login, and it is the row
/// [`crate::ui_script::seat_from_roster`] builds the player seat from a few lines earlier in the
/// same call. [`feed_default_language`] still runs and still owns the live answer — this only
/// makes sure the burst does not read a nil.
pub(crate) fn seed_default_language(world: &mut World, script: &mut UiScript) {
    let (Some(langs), Some(roster)) = (
        world.get_resource::<DefaultLanguagesRes>(),
        world.get_resource::<crate::char_select::Roster>(),
    ) else {
        return;
    };
    let Some(row) = roster.pending_row() else {
        return;
    };
    script.set_default_language(langs.0.name(u32::from(row.race), 0).map(str::to_string));
}

/// Push `GetDefaultLanguage()`'s one string, on change only.
///
/// The reference resolves it per call from the live player object; we resolve it once per race
/// change, which is the same answer with none of the per-frame churn — a race cannot change
/// without a new world entry. `None` (no player object, or no table) is the reference's zero-value
/// state, and that is what the VM stores.
///
/// **The locale column is 0.** `[0xc0e080]` is the client's locale slot; only enUS is populated in
/// the 5875 data, and every other DBC catalog here reads column 0 for the same reason.
fn feed_default_language(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    langs: Option<Res<DefaultLanguagesRes>>,
    mut pushed: Local<crate::ui_script::VmMemo<Option<Option<String>>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let pushed = pushed.get(&script);
    let name = self_q
        .iter()
        .next()
        .and_then(|store| store.0.unit_race())
        .zip(langs.as_ref())
        .and_then(|(race, langs)| langs.0.name(u32::from(race), 0))
        .map(str::to_string);
    if pushed.as_ref() != Some(&name) {
        script.set_default_language(name.clone());
        *pushed = Some(name);
    }
}

/// Seed the VM's Exhaustion.dbc table once per VM — the rest bindings' data
/// ([`benilla_ui::script::UiScript::set_exhaustion_rows`]; the ui_macro icon-catalog shape).
/// A failed load keeps the model's shipped-table fallback, so the rest surface still behaves
/// like the shipped enUS client rather than going dark.
///
/// Per VM rather than per process (1290) for the reason every seed here is: a login builds a fresh
/// VM, and this one degrades quietly — the fallback table is close enough that nobody would notice
/// it had stopped being seeded.
fn load_exhaustion_rows(
    script: Option<NonSendMut<UiScript>>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    let (Some(mut script), Some(assets)) = (script, assets) else {
        return;
    };
    if !seeded.claim(&script) {
        return;
    }
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_exhaustion(&mut chain)
    };
    match loaded {
        Ok(rows) => {
            info!("ui_unit: {} Exhaustion.dbc rest states", rows.len());
            script.set_exhaustion_rows(
                rows.into_iter()
                    .map(|r| (r.id as u8, r.name, f64::from(r.factor)))
                    .collect(),
            );
        }
        Err(e) => error!("ui_unit: Exhaustion.dbc failed — shipped-table fallback holds: {e:#}"),
    }
}

/// Melee swing → the `UNIT_COMBAT` vocabulary: the action comes from the melee wrapper's
/// per-victim-state table
/// (`0x4946d0` → `actionTable@0x83de28`), the descriptor from HitInfo bits **keyed on the
/// amount's sign** — `amount > 0` picks among CRITICAL `0x80` / GLANCING `0x4000` / CRUSHING
/// `0x8000`, `amount ≤ 0` among ABSORB `0x20` / BLOCK `0x800` / RESIST `0x40`, else `""`.
fn melee_feedback(hit_info: u32, victim_state: u32, damage: u32) -> (&'static str, &'static str) {
    match victim_state {
        2 => ("DODGE", ""),
        3 => ("PARRY", ""),
        5 => ("BLOCK", ""),
        6 => ("EVADE", ""),
        7 => ("IMMUNE", ""),
        8 => ("DEFLECT", ""),
        // 0 UNAFFECTED / 1 NORMAL / 4 INTERRUPT: WOUND, descriptor by the amount-sign key.
        _ => {
            if damage > 0 {
                if hit_info & 0x80 != 0 {
                    ("WOUND", "CRITICAL")
                } else if hit_info & 0x4000 != 0 {
                    ("WOUND", "GLANCING")
                } else if hit_info & 0x8000 != 0 {
                    ("WOUND", "CRUSHING")
                } else {
                    ("WOUND", "")
                }
            } else if hit_info & 0x20 != 0 {
                ("WOUND", "ABSORB") // full absorb
            } else if hit_info & 0x800 != 0 {
                ("WOUND", "BLOCK") // full block, when the bridge didn't rewrite the state
            } else if hit_info & 0x40 != 0 {
                ("WOUND", "RESIST") // full resist
            } else {
                ("MISS", "")
            }
        }
    }
}

/// The melee `UNIT_COMBAT` producer: rides the swing's impact keyframe ([`SwingImpact`]) with the
/// rest of the victim feedback — including `text_only` flushes, which the client fires the text
/// channel for. Every victim qualifies (no Gate A, no source class on the portrait path); token
/// resolution happens in [`fire_unit_combat`], so a swing on an un-tokened bystander simply
/// fires nothing. The center combat text does NOT ride here — the client fires it synchronously
/// at packet parse (the producer lives in `net/apply/combat.rs`).
fn melee_unit_combat(
    mut impacts: MessageReader<crate::creature_anim::SwingImpact>,
    mut out: MessageWriter<UnitCombatFeedback>,
) {
    for crate::creature_anim::SwingImpact { swing: s, .. } in impacts.read() {
        let Some(victim) = s.victim else { continue };
        let (action, flags) = melee_feedback(s.hit_info, s.victim_state, s.damage);
        out.write(UnitCombatFeedback {
            unit: victim,
            action,
            flags,
            amount: s.damage,
            school: 0, // melee is physical (the wire's sub-damage school is not carried — always 0 here)
        });
    }
}

/// Drain [`CombatTextEvent`] into the VM: `COMBAT_TEXT_UPDATE(messageType, data, extra)` — the
/// arg shape the shipped Blizzard_CombatText `OnEvent` reads (`arg1..arg3`).
fn fire_combat_text(
    script: Option<NonSendMut<UiScript>>,
    mut events: MessageReader<CombatTextEvent>,
) {
    let Some(mut script) = script else {
        return;
    };
    for ev in events.read() {
        let arg = |v: &Option<String>| v.clone().map_or(ScriptValue::Nil, ScriptValue::Str);
        script.fire_event(
            "COMBAT_TEXT_UPDATE",
            vec![
                ScriptValue::Str(ev.message_type.to_string()),
                arg(&ev.data),
                arg(&ev.extra),
            ],
        );
    }
}

/// Drain [`UnitCombatFeedback`] into the VM: fire `UNIT_COMBAT` once per live token the entity
/// maps to (`"player"`, `"target"` — the same tokens [`feed_units`] feeds). The frames filter by
/// `arg1` exactly like the real client's; in 1.12 only PlayerFrame/PetFrame register it, so a
/// `"target"` fire is API surface, not pixels.
fn fire_unit_combat(
    script: Option<NonSendMut<UiScript>>,
    mut events: MessageReader<UnitCombatFeedback>,
    self_q: Query<(), With<SelfPlayer>>,
    selection: Res<Selection>,
) {
    let Some(mut script) = script else {
        return;
    };
    for ev in events.read() {
        let mut fire = |token: &str| {
            script.fire_event(
                "UNIT_COMBAT",
                vec![
                    ScriptValue::Str(token.to_string()),
                    ScriptValue::Str(ev.action.to_string()),
                    ScriptValue::Str(ev.flags.to_string()),
                    ScriptValue::Int(i64::from(ev.amount)),
                    ScriptValue::Int(i64::from(ev.school)),
                ],
            );
        };
        if self_q.contains(ev.unit) {
            fire("player");
        }
        if selection.target == Some(ev.unit) {
            fire("target");
        }
    }
}

/// The 1.12 race id → (localized display, `raceFile` token) — `UnitRace`'s two returns. The file
/// token is the client's internal name (undead = `"Scourge"`, the space dropped from `"NightElf"`),
/// the same vocabulary the 2D portrait stand-in files use (`portrait::temporary_portrait`). The
/// display column is also what `$R`/`$r` expand to ([`crate::npc_text`]) — one table, both readers.
pub(crate) fn race_names(race: u8) -> Option<(&'static str, &'static str)> {
    Some(match race {
        1 => ("Human", "Human"),
        2 => ("Orc", "Orc"),
        3 => ("Dwarf", "Dwarf"),
        4 => ("Night Elf", "NightElf"),
        5 => ("Undead", "Scourge"),
        6 => ("Tauren", "Tauren"),
        7 => ("Gnome", "Gnome"),
        8 => ("Troll", "Troll"),
        _ => return None,
    })
}

/// The 1.12 class id → (localized display, `classFileName`) — `UnitClass`'s two returns. The file
/// name is uppercase (the ref's `strupper(classFileName)` tooltip lookups index GlobalStrings keys
/// like `WARRIOR_STRENGTH_TOOLTIP` directly with it).
pub(crate) fn class_names(class: u8) -> Option<(&'static str, &'static str)> {
    Some(match class {
        1 => ("Warrior", "WARRIOR"),
        2 => ("Paladin", "PALADIN"),
        3 => ("Hunter", "HUNTER"),
        4 => ("Rogue", "ROGUE"),
        5 => ("Priest", "PRIEST"),
        7 => ("Shaman", "SHAMAN"),
        8 => ("Mage", "MAGE"),
        9 => ("Warlock", "WARLOCK"),
        11 => ("Druid", "DRUID"),
        _ => return None,
    })
}

/// A playable race id → `UnitFactionGroup`'s token (`"Alliance"`/`"Horde"`), or `None` for a race
/// id that is not one of the eight.
///
/// **A side derived from the RACE, not from the faction template** — deliberately, and only for
/// the one window in which the template does not exist yet. [`faction_group`] is the real answer
/// and reads `UNIT_FIELD_FACTIONTEMPLATE` off the descriptor; during world entry there is no
/// descriptor, and `UnitFactionGroup("player")` answering nil there is not "no faction", it is a
/// state a real player character cannot be in. AceDB-2.0 — embedded across a large slice of the
/// corpus — builds its per-realm key as `realm .. " - " .. faction` at **file scope**, so a nil
/// side is 24 corpus addons stopping on `attempt to concatenate local 'faction'`
/// (`addon_harness::seat_a_session`, decision 1195, which seats exactly this in the survey's VM).
///
/// Every playable race has a fixed side in 1.12, so this is a lookup rather than a guess — and it
/// reads the [`crate::char_create::ALLIANCE`] column the create screen already keeps, so the two
/// cannot drift apart.
pub(crate) fn race_faction_group(race: u8) -> Option<&'static str> {
    if !(1..=8).contains(&race) {
        return None;
    }
    Some(if crate::char_create::ALLIANCE.contains(&race) {
        "Alliance"
    } else {
        "Horde"
    })
}

/// A unit's **PvP team digit** — `0x5efe00`'s tri-state: `0` Horde, `1` Alliance, `-1` no side.
///
/// **This is NOT [`faction_group`], and the difference is the whole of report B378.** The two read
/// different sources and only agree while nothing has moved a unit off its racial faction:
///
/// * `UnitFactionGroup` (`0x516630`) reads the unit's LIVE `UNIT_FIELD_FACTIONTEMPLATE`
///   (`0x5166b8 mov eax,[eax+0x110]` / `0x5166be mov eax,[eax+0x74]`, byte-read here) — so a
///   vmangos GM, forced to template 35, genuinely has no side and the PvP flag icon genuinely
///   hides. That is faithful.
/// * The rank title's team digit (`0x5efe00`) reads the unit's **RACE** and walks
///   `[obj+0x110]+0x78` → `ChrRaces.dbc` field 2 (FactionTemplate id) → `FactionTemplate.dbc`
///   field 3 (factionGroupMask) → `& 4` ⇒ 0, else `& 2` ⇒ 1, else −1 — never the live template.
///   A GM's race does not change, so the reference names his rank exactly as it always did.
///
/// We had the second wired to the first, which is why a Grand Marshal's Honor tab read `NONE` on
/// a GM-flagged account while the 1.12 client on the same server read "Grand Marshal".
/// Every `0x5efe00` caller is race-derived: `GetPVPRankInfo`'s team
/// (`0x51a9af`/`0x51a9c8`), `UnitPVPName`'s rank decoration (`0x5efe60`), the battlefield
/// scoreboard's per-row side (`0x4aa200`, which inlines the same walk off the name-cache record).
///
/// The table is the shipped one, frozen: `ChrRaces.dbc` has **nine** rows in 5875 and race 9
/// (Goblin, unplayable) shares Human's faction template 1, so it answers Alliance — not `None`,
/// which is what a "playable races only" table would say.
/// [`tests::race_pvp_team_matches_the_shipped_tables`] walks the real DBCs and pins every row.
pub(crate) fn race_pvp_team(race: u8) -> i8 {
    match race {
        // factionGroupMask 3 = Player|Alliance → `& 4` clear, `& 2` set.
        1 | 3 | 4 | 7 | 9 => 1,
        // factionGroupMask 5 = Player|Horde → `& 4` set, tested first.
        2 | 5 | 6 | 8 => 0,
        // No `ChrRaces` row (a creature's race byte, or an unstreamed descriptor): the engine's
        // bounds/NULL failure tail, `-1`. It formats into the key and matches no GlobalString.
        _ => -1,
    }
}

/// Resolve a UnitPopup unit token to the **player guid** it names — `"target"` through the
/// selection iff it really is a player (the target frame's PLAYER menu), a `"partyN"` token through
/// the roster (the party frame's PARTY menu). `"player"` (yourself) and any unresolved token answer
/// `None`.
///
/// Shared by every popup verb that acts on another player: trade's `InitiateTrade` (decision 0592
/// P1) and inspect's `NotifyInspect` both need exactly this step, so it lives here
/// rather than once per window.
pub(crate) fn player_token_guid(
    token: &str,
    selection: &Selection,
    group: &crate::ui_party::GroupState,
) -> Option<u64> {
    match token {
        "target" => selection
            .guid
            .filter(|g| benilla_protocol::guid::is_player(*g)),
        "player" => None,
        tok => tok
            .strip_prefix("party")
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| group.party_slots().nth(n))
            .map(|m| m.guid),
    }
}

/// **The one unit-token resolver** — token → the live object it names, or `None`.
///
/// The reference has exactly one of these (`0x515970`, nine `_strnicmp` compares, then the object
/// manager), and every binding that takes a unit reaches its unit through it. We had three: this
/// one (inlined in `TargetUnit`'s drain), [`player_token_guid`]'s players-only popup map, and the
/// per-token legs [`feed_units`] walks. They disagreed, and the disagreement was report **B304** —
/// the range map was built from the players-only one, so a creature `"target"` never entered it and
/// `CheckInteractDistance("target", 4)` answered a constant for every mob at every distance.
///
/// So: one resolver, and a caller that wants a *typemask* applies it to the resolved unit rather
/// than narrowing the token (the same order 1564 §2 settled for the reach map's own entries, and
/// the one `target::by_name`'s follow drain already keeps for the start gate).
///
/// [`Selection`] is a parameter rather than a field because [`crate::target::SelectCommit`] holds
/// it as `ResMut`, and one system cannot take both.
///
/// **Not resolved:** `partypetN`/`raidpetN` (no feed holds those units at all — a token nothing
/// pushes a `UnitState` for would answer a distance for a unit `UnitExists` denies) and `npc`
/// (the interaction target, which we do not keep). Both are *recognised* by the grammar
/// ([`benilla_ui::script`]'s `token_recognised`), so they stay a quiet nil, not a raise.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UnitTokens<'w, 's> {
    /// `Option` for [`feed_units`]' reason: a UI-only harness runs these feeds with no net stack,
    /// and a bare `Res` turns that into a system-validation panic rather than a token that names
    /// nobody. Same for the two below, which belong to the pet and targeting plugins.
    index: Option<Res<'w, crate::net::GuidIndex>>,
    pet: Option<Res<'w, crate::ui_pet::PetBar>>,
    hovered: Option<Res<'w, crate::target::Hovered>>,
    group: Res<'w, crate::ui_party::GroupState>,
    pub(crate) stores: Query<'w, 's, &'static ObjectStore>,
    me: Query<'w, 's, (Entity, &'static Guid), With<SelfPlayer>>,
}

impl UnitTokens<'_, '_> {
    /// Our own body's entity + guid.
    fn me(&self) -> Option<(Entity, u64)> {
        self.me.iter().next().map(|(e, g)| (e, g.0))
    }

    /// The guid → entity map, when the net stack is present — `0x468460`'s lookup. `pub(crate)`
    /// for the selection drain's `TargetLastEnemy` arm, which starts from a remembered **guid**
    /// rather than a token and must land in the very same index every token arm below does.
    pub(crate) fn held(&self, guid: u64) -> Option<(Entity, u64)> {
        Some((*self.index.as_ref()?.0.get(&guid)?, guid))
    }

    /// Resolve `token` to the live object it names. Case-folded (`_strnicmp`, 1247), and the
    /// prefix order is the reference's: `raid` before `party` for the same reason its own resolver
    /// puts `partypet` before `party` — the first matching compare wins.
    pub(crate) fn resolve(&self, token: &str, selection: &Selection) -> Option<(Entity, u64)> {
        let token = token.to_ascii_lowercase();
        match token.as_str() {
            "player" => self.me(),
            "target" => selection.target.zip(selection.guid),
            // The target's own target, one hop off `UNIT_FIELD_TARGET` — the read `/assist` and
            // the `"targettarget"` snapshot both run.
            "targettarget" => selection
                .target
                .and_then(|e| self.stores.get(e).ok())
                .and_then(|s| s.0.unit_target())
                .filter(|g| *g != 0)
                .and_then(|g| self.held(g)),
            // The hovered unit — the same `Hovered` pick `ui_tooltip` pushes `"mouseover"` from,
            // so `UnitExists("mouseover")` and this can never disagree.
            "mouseover" => {
                let h = self.hovered.as_ref()?;
                h.target.zip(h.guid)
            }
            // Off the bar's cached guid, the word `"pet"`'s snapshot and `UNIT_PET` read.
            "pet" => {
                let guid = self.pet.as_ref()?.spells.pet_guid;
                (guid != 0).then(|| self.held(guid)).flatten()
            }
            t if t.starts_with("raid") => t
                .strip_prefix("raid")
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| (1..=40).contains(n))
                .and_then(|n| {
                    crate::ui_party::raid_row_guid(&self.group, self.me().map(|(_, g)| g), n)
                })
                .and_then(|g| self.held(g)),
            t => t
                .strip_prefix("party")
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| (1..=4).contains(n))
                .and_then(|n| self.group.party_slots().nth(n - 1).map(|m| m.guid))
                .and_then(|g| self.held(g)),
        }
    }
}

/// The tokens [`feed_unit_reach`] offers the map each frame — every token [`UnitTokens`] can
/// resolve, reusing the party feed's own two tables so this set can never name a row that feed
/// does not push a `UnitState` for. `"player"` is in it because the reference answers it too
/// (d² = 0, in range of everything); the popup rows that started this map need only the first few.
fn reach_tokens() -> impl Iterator<Item = &'static str> {
    ["player", "target", "targettarget", "mouseover", "pet"]
        .into_iter()
        .chain(crate::ui_party::PARTY_TOKENS)
        .chain(crate::ui_party::RAID_TOKENS)
}

/// The squared distance between two world positions, in the binary's own accumulation shape: `f32`
/// inputs widened to `f64`, summed `(dz² + dx²) + dy²` (`0x48a26f..0x48a27d`, the kernel
/// `CanInspect 0x48a1b0` and `CheckInteractDistance 0x48ba00` share).
///
/// Our axes are Bevy's rather than the client's WoW triple. d² is invariant under that rotation, so
/// the only conceivable divergence from the binary is a last-ulp one — which can only change the
/// verdict for a unit standing *exactly* on a threshold. The thresholds and comparison operators,
/// which are what actually decide each gate, are transcribed exactly at the two bindings
/// (`benilla_ui::script::inspect`).
fn dist_sq(q: Vec3, p: Vec3) -> f64 {
    let dx = f64::from(q.x) - f64::from(p.x);
    let dy = f64::from(q.y) - f64::from(p.y);
    let dz = f64::from(q.z) - f64::from(p.z);
    (dz * dz + dx * dx) + dy * dy
}

/// `PLAYER_CONTROL_LOST` / `PLAYER_CONTROL_GAINED` — `SMSG_CLIENT_CONTROL_UPDATE` naming the
/// local player reaches `0x4958e0`, which writes the player-control flag and, **on a change**,
/// fires LOST when the byte is zero and GAINED when it is not; the boot init is "in control"
/// (`0x48f626`). The flag is [`crate::player::Player::control_lost`], which `player::wire_in`
/// writes from that packet; this fires the edge, and a fresh VM's memo is the boot value.
fn feed_player_control(
    script: Option<NonSendMut<UiScript>>,
    player: Option<Res<crate::player::Player>>,
    mut lost: Local<crate::ui_script::VmMemo<bool>>,
) {
    let (Some(mut script), Some(player)) = (script, player) else {
        return;
    };
    let lost = lost.get(&script);
    if *lost != player.control_lost {
        *lost = player.control_lost;
        // `HasFullControl`'s flag rides the same edge (1958).
        script.set_player_control(!player.control_lost);
        let event = if player.control_lost {
            "PLAYER_CONTROL_LOST"
        } else {
            "PLAYER_CONTROL_GAINED"
        };
        script.fire_event(event, vec![]);
    }
}

/// `PLAYER_FARSIGHT_FOCUS_CHANGED` — the `PLAYER_FARSIGHT` field-change callback (`0x5de0d0`)
/// fires it on both of its legs, whether or not the new guid resolves to a streamed object.
/// The edge is the FIELD's, so this diffs the descriptor value the camera's `publish_view_subject`
/// reads, never the resolved pose.
fn feed_farsight_focus(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut focus: Local<crate::ui_script::VmMemo<Option<u64>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(store) = self_q.iter().next() else {
        return;
    };
    let anchor = store.0.player_farsight();
    let focus = focus.get(&script);
    if *focus != anchor {
        *focus = anchor;
        script.fire_event("PLAYER_FARSIGHT_FOCUS_CHANGED", vec![]);
    }
}

/// Feed the **unit reach map** — for every token that resolves to a live unit object, its squared
/// distance from us, plus whether that unit passes inspect's own two non-distance refusals.
///
/// **Membership is the object lookup and nothing else** (1564 §2). An absent token is one the
/// object manager holds no unit for, and both bindings answer `nil` there — the reference's own
/// null-object arm. Every typemask therefore rides IN the entry:
///
/// - *attackable* — vmangos's inspect refusal (`IsValidAttackTarget`, `MiscHandler.cpp:945-956`).
///   Keeping the token out of the map instead would make `CheckInteractDistance` — a pure distance
///   test in the binary — gray Follow and Duel on an enemy player the reference leaves live.
/// - *is a player* — vmangos's other one (`sObjectMgr.GetPlayer`). This is the leg that was applied
///   at the token and was report **B304**: a creature `"target"` never entered the map, so the
///   30-yard rung Quiver's Dead Zone band reads answered a constant for every mob — "in range" at
///   the published tag, "out of range" after 1564 flipped the absent default. Both are the same
///   defect: the map was never asked about mobs at all.
///
/// That the *client* checks those two is inferred (the 348-byte `0x48a1b0`'s non-math part is
/// undecoded), but a wrong guess can only cost a request the server would drop.
///
/// Ungated, unlike every other feed here (1439): the numbers change whenever anything moves, so a
/// change gate would fire every frame anyway and cost a comparison for nothing. Nothing keys an
/// event off this map, so a re-push is invisible.
fn feed_unit_reach(
    script: Option<NonSendMut<UiScript>>,
    tokens: UnitTokens,
    selection: Res<Selection>,
    self_q: Query<(&Transform, &ObjectStore), With<SelfPlayer>>,
    transforms: Query<&Transform>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
) {
    let Some(mut script) = script else {
        return;
    };
    let mut reach = HashMap::new();
    if let Some((self_tf, self_store)) = self_q.iter().next() {
        for token in reach_tokens() {
            let Some((entity, guid)) = tokens.resolve(token, &selection) else {
                continue;
            };
            let Ok(tf) = transforms.get(entity) else {
                continue;
            };
            let store = tokens.stores.get(entity).ok();
            let inspectable = benilla_protocol::guid::is_player(guid)
                && !crate::target::can_attack(
                    store,
                    factions.as_deref(),
                    &reputations,
                    Some(self_store),
                );
            reach.insert(
                token.to_string(),
                benilla_ui::script::UnitReach {
                    dist_sq: dist_sq(tf.translation, self_tf.translation),
                    inspectable,
                },
            );
        }
    }
    script.set_unit_reach(reach);
}

/// The object stores [`feed_units`] reads, and the change tracking over them.
///
/// One `SystemParam` rather than three, because Bevy's tuple limit is 16 and this feed had grown
/// to sit exactly on it — the next resource it legitimately needed (`ChrClasses.dbc`, for
/// `UnitHasRelicSlot`) turned the ceiling into a compile error with no hint that a ceiling was
/// what it hit. Grouping the three that always move together buys the room back and says why
/// they belong together, which the flat list did not. Same idiom as
/// [`crate::target::lock::GoLockInputs`].
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UnitStores<'w, 's> {
    /// Every streamed object's descriptor, read by guid → entity.
    all: Query<'w, 's, &'static ObjectStore>,
    /// Whose descriptor moved this frame — the feed's dirty gate. Not an item's: items carry an
    /// `ObjectStore` since 2334, and a durability tick or a stack count naming no unit token
    /// would open the gate for nothing (2343).
    changed: Query<'w, 's, (), (Changed<ObjectStore>, Without<crate::items::ItemObject>)>,
    /// Whose object left the manager — the other half of that gate, and cleared every run. The
    /// [`Guid`], not the store: a torn-down object sheds its guid at the teardown while its
    /// fading model keeps the store another two seconds ([`crate::net::tear_down`]), and a
    /// despawn removes both.
    removed: RemovedComponents<'w, 's, Guid>,
    /// The per-field edges this run — [`fire_transitions`]' three mirror-diff
    /// arms fire off these, per token naming the moved unit.
    edges: MessageReader<'w, 's, FieldChanged>,
}

/// Build a unit snapshot from a streamed object's descriptor (decision 0061's `ObjectFields`) plus
/// its cache-resolved name and its `UnitReaction` value (`1..8`, or `0` for tokens whose reaction we
/// don't resolve — everything but `"target"`; see [`feed_units`]).
///
/// `classes` is `ChrClasses.dbc`, absent when the client data failed to load. Only the relic column
/// is read off it — the class NAMES are this file's own table, because they are localized display
/// strings rather than a DBC column we can key on.
pub(crate) fn snapshot(
    store: &ObjectStore,
    name: Option<String>,
    reaction: u8,
    classes: Option<&ChrClasses>,
) -> UnitState {
    let power_type = store.0.unit_power_type();
    let race = store.0.unit_race().and_then(race_names);
    let class_id = store.0.unit_class();
    let class = class_id.and_then(class_names);
    UnitState {
        exists: true,
        // **This function IS the reference's `0x468460` having succeeded.** It is only ever called
        // with a live `ObjectStore`, so every snapshot it builds is a unit the object manager holds
        // — which is the whole of `UnitIsVisible 0x516030`. Set here rather than at the call
        // sites so a future feed cannot forget it: the roster-only legs build a `UnitState`
        // literally and get `false` from `Default`, which is the correct out-of-range answer.
        has_object: true,
        name,
        // The UI-facing health/power getters, not the raw fields: a unit carrying
        // `UNIT_DYNFLAG_DEAD` — a feigning hunter — answers 0 to `UnitHealth 0x5174d0` and
        // `UnitMana 0x517670`, while the *max* getters stay ungated, so its bars read 0/max (empty)
        // for itself and for everyone watching. The zeroes ride the ordinary per-field diff in
        // [`fire_transitions`], which fires `UNIT_HEALTH` + the power event on the flag's edge —
        // exactly the pair the reference's `UNIT_DYNAMIC_FLAGS` watcher fires there
        // (`0x6004c5`/`0x6004f0`, event `0x10` and `0x11 + powerType`). The power pair also carries
        // the raw→display divide the same two getters do — rage rides the wire ×10
        // and pet happiness ×1000, so these are the numbers the reference shows, not the wire's.
        health: store.0.unit_shown_health().unwrap_or(0),
        max_health: store.0.unit_max_health().unwrap_or(0),
        level: store.0.unit_level().unwrap_or(0),
        power_type,
        power: store.0.unit_shown_power(power_type).unwrap_or(0),
        max_power: store.0.unit_shown_max_power(power_type).unwrap_or(0),
        // `UnitIsDead 0x517ac0` — health ≤ 0 **or** the dead-looking flag, so a feigning unit is
        // dead to the API, to the target frame's DEAD text and to the greyed portrait alike.
        dead: store.0.unit_reads_dead(),
        // The released-ghost predicate: PLAYER_FLAGS bit 0x10 — a ghost's
        // health is 1, so `dead` above is false for it. Zero/absent on creatures.
        ghost: store.0.player_is_ghost(),
        // `UnitIsCharmed 0x516cf0` — `UNIT_FIELD_CHARMEDBY != 0`, the same field `ui_aura`'s
        // charmed-unit buff leg already reads, so the two cannot disagree about who is charmed.
        charmed: store.0.unit_charmed_by().is_some(),
        // The tapped pair — `UNIT_DYNAMIC_FLAGS` bits 0x4/0x8. Set here and only here, so the
        // object-presence conjunct both predicates carry comes for free: a unit with no live
        // descriptor never reaches this function and reads `false` from `Default`.
        tapped: store.0.unit_tapped(),
        tapped_by_player: store.0.unit_tapped_by_player(),
        // `UnitIsPartyLeader`'s descriptor leg. Zero on a creature, which is what the reference's
        // TYPEMASK_PLAYER restriction achieves there: a non-player has no PLAYER block at all, so
        // the field simply reads absent.
        group_leader: store.0.player_is_group_leader(),
        // The raw `PLAYER_FLAGS` dword `PLAYER_FLAGS_CHANGED` fires on — the
        // `flags`/`UNIT_FLAGS` pattern one field over. Absent on a creature, which reads 0 and so
        // can never produce a delta, exactly as the reference's TYPEID_PLAYER-scoped watcher
        // cannot be registered against one.
        player_flags: store.0.player_flags(),
        reaction,
        race: race.map(|(n, _)| n.to_string()),
        race_file: race.map(|(_, f)| f.to_string()),
        class: class.map(|(n, _)| n.to_string()),
        class_file: class.map(|(_, f)| f.to_string()),
        // `UnitHasRelicSlot 0x519e50` — the class byte against `ChrClasses.dbc` field 16, under
        // the reference's own gate and in its order: TYPEMASK_PLAYER first (`0x519e8d`,
        // `[[obj+8]+8] >> 4 & 1` — which is exactly what [`ObjectType::Player`] decodes), the
        // class byte second. The player test is not decoration: a creature carries a class byte
        // too and indexes the same table, so without it a class-2 humanoid NPC answers 1.
        has_relic_slot: matches!(store.0.object_type(), Some(ObjectType::Player))
            && class_id.is_some_and(|c| classes.is_some_and(|t| t.has_relic_slot(u32::from(c)))),
        // The descriptor's gender byte (0 male, 1 female) on the API's `UnitSex` scale (2 male,
        // 3 female; 0 = unknown → the binding's nil).
        sex: match store.0.unit_gender() {
            Some(0) => 2,
            Some(1) => 3,
            _ => 0,
        },
        // The tooltip flag lines (decision 0276's unit law): PvP + Skinnable straight off
        // UNIT_FIELD_FLAGS (vmangos UnitDefines.h: 0x1000 / 0x04000000, VERIFIED).
        pvp: store.0.unit_flags() & 0x1000 != 0,
        skinnable: store.0.unit_flags() & 0x0400_0000 != 0,
        // `UnitPlayerControlled` — the same word, bit 3 (`UNIT_FLAG_PVP_ATTACKABLE 0x8`, which is
        // behaviourally "player-controlled"; `target::relations` already reads it for the duel
        // leg of the selection ring). Wider than `is_player` above: a pet or a charmed creature
        // sets it without being a player.
        player_controlled: store.0.unit_flags() & 0x8 != 0,
        flags: store.0.unit_flags(),
        // The raw `UNIT_DYNAMIC_FLAGS` dword the event of the same name fires on — the `flags`
        // arm above, a hundred descriptor fields over. `tapped`/`tapped_by_player` up the struct
        // are two of its bits; this is the whole word, because the reference's watch is a memcmp
        // over the dword and not a bit test.
        dynamic_flags: store.0.unit_dynamic_flags(),
        owner: store
            .0
            .unit_summoned_by()
            .or_else(|| store.0.unit_charmed_by())
            .or_else(|| store.0.unit_created_by())
            .unwrap_or(0),
        // `UnitAffectingCombat 0x517e10` — the SAME `UNIT_FIELD_FLAGS` word, bit 19
        // (`shr ecx,0x13; test cl,1`). One flag for every token: a whole-image census of
        // that idiom finds the local-player readers reading this identical bit, so there is no
        // player-specific combat latch to model beside it.
        in_combat: store.0.unit_flags() & crate::player::UNIT_FLAG_IN_COMBAT != 0,
        // Free-for-all PvP: `PLAYER_FLAGS` bit 7, the same field the ghost
        // predicate above reads (vmangos `Player.h:322` `PLAYER_FLAGS_FFA_PVP`, cross-read against
        // 0633's byte-level `[+0xe68]+8` bit-7). Zero on creatures, which have no PLAYER_FLAGS —
        // and `UnitIsPVPFreeForAll` is false for them in the reference too.
        is_pvp_ffa: store.0.player_flags() & 0x80 != 0,
        // The unit's CURRENT honor rank: `PLAYER_BYTES_3` byte 3, on the internal
        // 0..=18 scale. It sits in `snapshot` — not in the caller's guid-keyed enrichment like
        // `faction_group` — because it needs nothing but the descriptor, and because it is PUBLIC:
        // the server streams it for every player in view, which is the only reason the inspect
        // pane's `UnitPVPRank("target")` can answer at all. A creature has no PLAYER block and
        // reads 0, the reference's own answer for one.
        pvp_rank: store.0.player_pvp_rank().unwrap_or(0),
        // `0x5efe00`'s team digit — the second `%d` of `PVP_RANK_<rank>_<team>`. It sits here
        // beside the rank byte for the same reason that one does (nothing but the descriptor is
        // needed) and it reads the RACE, not `faction_group`: the engine walks the race through
        // `ChrRaces`/`FactionTemplate` and never looks at the live `UNIT_FIELD_FACTIONTEMPLATE`
        // this unit is carrying, so a GM-flagged player keeps his rank title while losing the PvP
        // icon. See [`race_pvp_team`] and decision 2227 (report B378).
        pvp_team: store.0.unit_race().map_or(-1, race_pvp_team),
        // `PLAYER_BYTES_3` byte 2 — the city-protector title, the same PUBLIC dword as the rank
        // byte above. `UnitPVPName` appends a `PVP_MEDAL<n>` line for a non-zero one; 0 is "no
        // medal", which is every character on this server (vmangos never writes the byte).
        pvp_medal: store.0.player_pvp_medal().unwrap_or(0),
        // is_player + the creature-record extras (subtitle/type/rank/civilian) are the caller's
        // guid-keyed enrichment — [`enrich_unit`].
        ..Default::default()
    }
}

/// Fill a snapshot's guid-keyed tooltip fields (decision 0276's unit law): players flag
/// `is_player` (the "Race Class (Player)" level line); creatures pull subtitle/type/rank/
/// civilian/leader from the ask-once template record, and resolve the faction-name line.
/// The type word is `CreatureType.dbc`'s enUS display list (ids 1..9 — a fixed 1.12
/// vocabulary; 10 "Not specified" shows nothing).
pub(crate) fn enrich_unit(
    state: &mut UnitState,
    guid: u64,
    names: &NameCache,
    store: &ObjectStore,
    factions: Option<&Factions>,
    self_store: Option<&ObjectStore>,
) {
    if benilla_protocol::guid::is_player(guid) {
        state.is_player = true;
        // No faction line for players — their PLAYER,* factions carry no reputation slot, so
        // the builder's rep-index gate always drops the line (byte-identical to resolving it).
        return;
    }
    let Some(entry) = benilla_protocol::guid::entry(guid) else {
        return;
    };
    // The ask-once creature record — `CGUnit+0xb30`, filled from `SMSG_CREATURE_QUERY_RESPONSE`.
    // `None` is the round trip before the answer lands, and it is a REAL state the plate is drawn
    // in, not a "nothing is known yet" to render blank: the name reads `UNKNOWNOBJECT` there,
    // and the lines below split on exactly what the record does or does not gate.
    let rec = names.creature_record(entry);
    if let Some(rec) = rec {
        state.subtitle = rec.subname.clone();
        state.creature_type_name = creature_type_word(rec.creature_type).map(str::to_string);
        // The client's one rank getter, both gates (`gated_rank`) — never `rec.rank`
        // directly: an enslaved elite reads rank 0, so it loses its border dragon, its ELITE
        // tooltip word and its world-boss skull together, exactly as in the reference.
        state.rank = crate::names::gated_rank(Some(rec), Some(store));
        state.civilian = rec.civilian;
        state.racial_leader = rec.racial_leader;
    }
    // The faction-name line ("Stormwind", between level and PvP) — the unit builder's tail block,
    // every gate transcribed: the record's HIDE_FACTION_TOOLTIP type flag (0x10), the template →
    // Faction.dbc hop, the reputation-slot gate (rep_index ≥ 0), and the race/class slot walk with
    // its hidden flag (0x4).
    //
    // **Its entry gate is NOT the record.** `0x612610` reads `[unit+0xb30]` and returns 1 when
    // there is none — that is how a PLAYER passes a gate whose only field lives in CreatureInfo,
    // and a creature whose query has not answered takes the identical leg. So the line resolves
    // off the DESCRIPTOR's `UNIT_FIELD_FACTIONTEMPLATE`, which is already streamed, and shows
    // under a pending name exactly as it does under a known one. It was gated on the record here
    // on the premise that "before the query answers we have no name line either" — the premise
    // decision 2002 corrected, and 2040 with it.
    if rec.is_none_or(|r| r.type_flags & crate::names::type_flags::NO_FACTION_TOOLTIP == 0) {
        state.faction_name = (|| {
            let catalog = factions?.catalog();
            let faction_id = catalog.template(store.0.unit_faction_template()?)?.faction;
            let info = catalog.reputation_faction(faction_id)?;
            let self_store = self_store?;
            let race = self_store.0.unit_race().unwrap_or(0);
            let class = self_store.0.unit_class().unwrap_or(0);
            info.tooltip_shows_for(race, class)
                .then(|| catalog.faction_name(faction_id).map(str::to_string))
                .flatten()
        })();
    }
}

/// Drain the `TogglePVP` intents into `CMSG_TOGGLE_PVP` — `/pvp` is the only
/// caller the reference has (`SlashCmdList["PVP"]`), and the only one we have.
///
/// This rides the unit feed's plugin rather than a `ui_pvp` of its own: the family's whole client
/// state is one remembered bit ([`UnitFeedMemo::pvp_desired`]), which lives with the feed's other
/// self-flag edge (`in_combat`) rather than in a plugin of its own.
fn drain_pvp_toggles(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_pvp_toggles() {
        let _ = commands.0.send(crate::net::ClientCommand::TogglePvp);
    }
}

/// Drain the `ShowHelm`/`ShowCloak` flips into `CMSG_TOGGLE_HELM`/`CMSG_TOGGLE_CLOAK` (decision
/// 1472) — the Options window's two equipment-display rows, and the only callers there are.
///
/// The VM has already decided *whether* a flip is needed (the setter compares the asked-for state
/// against the belief it holds and queues nothing when they agree), because only the VM knows what
/// the row just did optimistically. This end is the pure send, the PvP drain's shape exactly.
fn drain_worn_display_toggles(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for which in script.take_worn_display_toggles() {
        let _ = commands.0.send(match which {
            WornDisplay::Helm => crate::net::ClientCommand::ToggleHelm,
            WornDisplay::Cloak => crate::net::ClientCommand::ToggleCloak,
        });
    }
}

/// Drain the `SetActionBarToggles` posts into `CMSG_SET_ACTIONBAR_TOGGLES` (sent at `0x4e771d`)
/// — the Options window's four extra-bar rows, and the only callers there are.
///
/// **Every queued call becomes a packet**, with no did-it-change gate and no coalescing: the real
/// binding has neither (unlike `ShowHelm`/`ShowCloak`, which send only on a difference), so two
/// calls in a frame are two sends. The byte is absolute rather than a flip, so a duplicate is
/// harmless — but dropping one would be an optimisation the reference does not make, and the
/// server is the only store this preference has.
fn drain_action_bar_toggles(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for toggles in script.take_action_bar_toggle_sends() {
        let _ = commands
            .0
            .send(crate::net::ClientCommand::SetActionBarToggles { toggles });
    }
}

/// `PLAYER_FLAGS_PVP_DESIRED` — the PvP *preference* bit (vmangos `PlayerDefines.h`), which is what
/// `CMSG_TOGGLE_PVP` flips. Not to be confused with `UNIT_FIELD_FLAGS`' `PVP` bit `0x1000`, the flag
/// the icon draws: the preference clears instantly, the flag lingers for the server's timer.
const PLAYER_FLAGS_PVP_DESIRED: u32 = 0x200;

/// `PLAYER_FLAGS_RESTING` — inside a rest area now (vmangos `Player.h:320`); the bit
/// `IsResting 0x516ea0` tests (`shr 5; test 1`).
const PLAYER_FLAGS_RESTING: u32 = 0x20;

/// `PLAYER_FLAGS` bit 12 / bit 13 — the two **play-time** regimes an anti-addiction realm puts an
/// account into, read by `PartialPlayTime` (`0x48eb70`) and `NoPlayTime` (`0x48ebe0`). Decision
/// 1746 decoded both, and settled that `0x1000` is PARTIAL_PLAY_TIME on 5875 rather than the
/// pre-1.6.1 `CAN_SELF_RESURRECT` it had been read as. Stock `PlayerFrame_UpdatePlaytime` tests
/// them at LOAD, so their absence is a raise on the first frame, not a cosmetic gap.
const PLAYER_FLAGS_PARTIAL_PLAY_TIME: u32 = 0x1000;
const PLAYER_FLAGS_NO_PLAY_TIME: u32 = 0x2000;

/// The PvP-preference announcement rule: the `(toast, verbose)` **GlobalStrings
/// keys** on a real change of the bit, `None` otherwise.
///
/// `was: None` is first sight and stays silent — the reference's handler is driven by a
/// *changed-bits* mask (`new ^ old`), so the descriptor that first carries the flag at login says
/// nothing.
///
/// **Keys, not sentences**, and the pair rides two different routes because the
/// reference gives them two different natures. `ERR_PVP_TOGGLE_ON`/`_OFF` are message-catalog rows
/// (437/438, `kind 1` — the yellow `UI_INFO_MESSAGE`), so the catalog names their surface;
/// `PVP_TOGGLE_ON_VERBOSE`/`_OFF_VERBOSE` are **not** catalog rows at all, so there is no record to
/// read a kind or a sound off and the chat surface is the handler's own — the `/ginfo` shape
/// (`ui_guild::feed::ginfo_lines`). Both are argument-free, so there is no fill step.
fn pvp_announcement(was: Option<bool>, now: bool) -> Option<(&'static str, &'static str)> {
    if was? == now {
        return None;
    }
    Some(if now {
        ("ERR_PVP_TOGGLE_ON", "PVP_TOGGLE_ON_VERBOSE")
    } else {
        ("ERR_PVP_TOGGLE_OFF", "PVP_TOGGLE_OFF_VERBOSE")
    })
}

/// The rest-state chat line: the rest-state BYTE watcher `0x5de4e0` messages
/// only on a real old≠new transition (the
/// dispatcher's `rep cmpsb` mirror diff at `0x4655bb`), through the hard-coded 3×2 pair table
/// `0x80af50` — state 1 → `ERR_EXHAUSTION_RESTED`, state 2 → `ERR_EXHAUSTION_NORMAL`, state 0 →
/// the table's deliberate no-message sentinel (id 0x1d1), states ≥ 3 gated off before the table
/// (`cmp esi,3; jae`), so the beta tiers never speak even though their strings ship.
///
/// The table holds **message ids**, so the answer here is the row's key and the catalog decides the
/// rest: rows 346/347 are `kind 0` — a plain SYSTEM chat line, never UIErrorsFrame
/// — with no cue and `type_tag 0x44`, so no voice either. Entering rested also arms a one-shot
/// tutorial popup (id 0x19) — the tutorial system isn't built, a named cut.
fn rest_state_message(prev: u8, new: u8) -> Option<&'static str> {
    if prev == new {
        return None;
    }
    match new {
        1 => Some("ERR_EXHAUSTION_RESTED"),
        2 => Some("ERR_EXHAUSTION_NORMAL"),
        _ => None,
    }
}

/// A unit's PvP faction group — `UnitFactionGroup`'s pair, as the icon law reads it (decision
/// 0646 §1/§3): `UNIT_FIELD_FACTIONTEMPLATE` → `FactionTemplate.dbc`'s group mask → the
/// `FactionGroup.dbc` name of its **side** bit.
///
/// The `& 6` is the whole rule and it is load-bearing, not a tidy-up: every playable race's
/// template carries `Player|<side>` (mask 3 Alliance, 5 Horde) and so do the PvP-flagged city
/// guards, while `FactionGroup.dbc`'s Player and Monster rows have EMPTY localized names and no
/// `UI-PVP-Player`/`UI-PVP-Monster` texture ships. Masking to the two side bits before the lookup
/// is therefore the only reading the shipped art admits — a lowest-bit walk would answer "Player"
/// for every player in the game.
pub(crate) fn faction_group(store: &ObjectStore, factions: Option<&Factions>) -> Option<String> {
    let catalog = factions?.catalog();
    let template = catalog.template(store.0.unit_faction_template()?)?;
    // The ENGLISH name (`InternalName`), because `UnitFactionGroup`'s first return is concatenated
    // into a texture path by every stock consumer. The localized twin is below.
    catalog
        .faction_group_internal_name(template.group_mask & 6)
        .map(str::to_string)
}

/// The localized group name for the same unit — `UnitFactionGroup`'s SECOND return, which stock
/// shows as text rather than building a path from.
pub(crate) fn faction_group_localized(
    store: &ObjectStore,
    factions: Option<&Factions>,
) -> Option<String> {
    let catalog = factions?.catalog();
    let template = catalog.template(store.0.unit_faction_template()?)?;
    catalog
        .faction_group_name(template.group_mask & 6)
        .map(str::to_string)
}

/// `CreatureType.dbc` id → the enUS display word (the level line's class slot for creatures).
fn creature_type_word(t: u32) -> Option<&'static str> {
    Some(match t {
        1 => "Beast",
        2 => "Dragonkin",
        3 => "Demon",
        4 => "Elemental",
        5 => "Giant",
        6 => "Undead",
        7 => "Humanoid",
        8 => "Critter",
        9 => "Mechanical",
        // The shipped `CreatureType.dbc` is **1..11 dense**, not 1..9 — this table stopped two
        // rows early. 11 is reachable and the reference's own nameplate filter tests for it
        // (`0x605570 == 0xb`).
        11 => "Totem",
        // 10 is "Not specified" in the DBC. Deliberately still None: this word is the tooltip's
        // level-line class slot and a literal "Not specified" there is noise. The cost is named
        // rather than hidden — `UnitCreatureType` shares this field, so it answers nil for a
        // type-10 unit where the reference answers the DBC word.
        _ => return None,
    })
}

/// Diff a token's fresh snapshot against the last one pushed and fire the per-field Era events.
/// `prev = None` (the token just appeared) treats every present field as a transition — except
/// the three arms that are the reference's per-field watch bridge, which fire off `edges`, the
/// descriptor edges this run: those fire per token naming a unit whose dword
/// moved, and a unit's create moves nothing.
pub(crate) fn fire_transitions(
    script: &mut UiScript,
    token: &str,
    prev: Option<&UnitState>,
    cur: &UnitState,
    edges: &FieldEdges,
) {
    let tok = || ScriptValue::Str(token.to_string());
    let changed = |f: fn(&UnitState) -> u64| prev.is_none_or(|p| f(p) != f(cur));

    if changed(|u| u64::from(u.health)) {
        script.fire_event("UNIT_HEALTH", vec![tok()]);
    }
    if changed(|u| u64::from(u.max_health)) {
        script.fire_event("UNIT_MAXHEALTH", vec![tok()]);
    }
    if changed(|u| u64::from(u.level)) {
        script.fire_event("UNIT_LEVEL", vec![tok()]);
    }
    if changed(|u| u64::from(u.power_type)) {
        script.fire_event("UNIT_DISPLAYPOWER", vec![tok()]);
    }
    // `UNIT_FLAGS` (id 40) — the per-field watch bridge:
    // `0x51bbb0` registers one watch per named unit field, the notifier `0x465570`
    // fires `0x51bd50` → `0x515e50` on any change of the dword's bytes, once per token mapping to
    // the unit, `arg1` the token. The create leg runs no notify pass, so a unit's FIRST snapshot
    // is not a transition here — unlike the fields above, whose first-appearance fire is this
    // feed's own posture (1953, corrected in 1957). Since 2297 that is literally the trigger:
    // the field edge for this unit's dword, which a create never emits, and which a retarget
    // onto a unit with different flags does not emit either (the old `prev.is_some()` snapshot
    // diff fired on that). The stock pet bar filters it for `"pet"`.
    if edges.moved(cur.guid, benilla_protocol::field::FIELD_UNIT_FLAGS) {
        script.fire_event("UNIT_FLAGS", vec![tok()]);
    }
    // `PLAYER_FLAGS_CHANGED` (id 407) — the `UNIT_FLAGS` arm above, one descriptor field over, and
    // until decision 2078 one of the events a migrated stock window registered and nothing here
    // produced. Byte-read out of `WoW.exe` at `0x5eea27`-`0x5eea3d`:
    //
    // ```
    // 5eea27  8b 56 08 8b 02 89 45 f0 8b 4a 04 89 4d f4   ; [ebp-0x10] = this player's own GUID
    // 5eea35  ba 97 01 00 00                              ; edx = 0x197 = 407
    // 5eea3a  8d 4d f0
    // 5eea3d  e8 0e 74 f2 ff                              ; call 0x515e50 — the token fan-out
    // ```
    //
    // Three properties, each load-bearing and each verified rather than assumed:
    //
    // 1. **Unguarded.** No bit test stands between the XOR-diff at `0x5ee9b8` and this fire, so
    //    *any* `PLAYER_FLAGS` bit moving announces itself — not just the ones this struct decodes.
    //    That is why the trigger is the raw dword (`UnitState::player_flags`) and not a bool.
    // 2. **Above the local-GUID gate** at `0x5eea93` (the ghost/resting/PvP/play-time arms below it
    //    are self-only; this one is not) — and the trampoline `0x5e2850` resolves the changed
    //    object by GUID under TYPEMASK_PLAYER, "any player, not the local one". So a *stranger's*
    //    flags fire it, which is the whole reason the stock target frame can listen.
    // 3. **Once per unit token naming that GUID, `arg1` = the token, no `arg2`** — `0x515e50` walks
    //    `0x515c50`'s token array and calls `0x703f50(id, "%s", token)` per entry. That is exactly
    //    this function's own shape, which is why the arm belongs here and not beside the self-only
    //    feeds below.
    //
    // **"Fires for any player" is not "reaches Lua", and the difference is free here.**
    // `0x515e63 test eax,eax / 0x515e6a jle 0x515e8a` skips the fan-out loop entirely on a zero
    // token count, so a remote player who is nobody's target, mouseover, party or raid member
    // announces **nothing** to the VM — the handler still runs, and its helm/cloak arms above the
    // gate still repaint them, but no event is signalled. This feed gets that for nothing by
    // construction: it is only ever called *per token*, so a tokenless player is never reached.
    //
    // Off the field edge for the same reason `UNIT_FLAGS` is: this is a mirror-diff watcher,
    // and a unit's first snapshot is its CREATE, which runs no notify pass (1098 §4).
    //
    // **The sole 1.12 consumer is the target frame's PARTY-LEADER icon, not an AFK/DND badge** —
    // `TargetFrame.lua:88-95` re-runs the `UnitIsPartyLeader("target")` show/hide, and bit `0x1` is
    // that predicate's descriptor leg. Nothing in 1.12 FrameXML draws an AFK or DND badge on a unit
    // frame, and build 5875 has no `UnitIsAFK`/`UnitIsDND` binding at all (a zero-hit whole-image
    // byte census); the `<AFK>`/`<DND>` the era shows are the chat line's `arg6` flag. This comment
    // is here because the gap list said "the AFK/DND badge" for months and sent the first look at
    // the wrong window.
    if edges.moved(cur.guid, benilla_protocol::field::FIELD_PLAYER_FLAGS) {
        script.fire_event("PLAYER_FLAGS_CHANGED", vec![tok()]);
    }
    // `UNIT_DYNAMIC_FLAGS` (id 137) — the third arm of the same bridge, and one benilla had
    // **never fired at all**, which is a different failure from firing it wrong: an addon that
    // registers it hears nothing, forever, and there is no error anywhere to say so.
    //
    // Nothing in 1.12 FrameXML registers it, which is exactly why no gate saw the hole — the
    // producer gate's oracle is what a stock chain file listens for (`reference_ui::
    // every_event_a_chain_file_registers_has_a_producer`), and no stock file listens for this one.
    // The corpus does: `CT_UnitFrames/CT_TargetFrame.xml:200` and `TipBuddy/TipBuddy.lua:17`
    // register it, and both are repaint wires for the tapped/grey-bar state — the same state
    // `UnitIsTapped` publishes and that the `UNIT_FACTION` arm below repaints the stock frames on.
    //
    // Byte-verified, not inherited (see [`UnitState::dynamic_flags`]): the name-table slot for
    // 137 resolves to `"UNIT_DYNAMIC_FLAGS"`, and the watch length is one dword, so the trigger
    // is the RAW word and not the four bits this struct decodes. Off the field edge for the same
    // reason the two arms above are — a unit's first snapshot is its CREATE, and the create
    // block runs no notify pass.
    if edges.moved(cur.guid, benilla_protocol::field::FIELD_UNIT_DYNAMIC_FLAGS) {
        script.fire_event("UNIT_DYNAMIC_FLAGS", vec![tok()]);
    }
    // The POWER pair is named per resource in 1.12, not once with the token as arg2: the reference's
    // `UnitFrameManaBar_Initialize` registers `UNIT_MANA`/`UNIT_RAGE`/`UNIT_FOCUS`/`UNIT_ENERGY`/
    // `UNIT_HAPPINESS` and the five `UNIT_MAX*` twins (`UnitFrame.lua:190-199`), and nothing in
    // 1.12 FrameXML has ever heard of `UNIT_POWER_UPDATE`. `power_token` already yields exactly the
    // suffix, so the name is the token. Health is unaffected — `UNIT_HEALTH`/`UNIT_MAXHEALTH` are
    // spelled the same in both eras — and so is `UNIT_DISPLAYPOWER` above.
    //
    // This was live breakage, not tidiness: while we fired only the Era names, the stock unit frames
    // registered only the 1.12 ones, so a mana/rage/energy bar never moved except when something
    // else happened to call `UnitFrame_Update` (a target change, a pet summon, a frame show).
    // Decision 1819.
    if changed(|u| u64::from(u.power)) {
        script.fire_event(
            &format!("UNIT_{}", power_token(cur.power_type)),
            vec![tok()],
        );
    }
    if changed(|u| u64::from(u.max_power)) {
        script.fire_event(
            &format!("UNIT_MAX{}", power_token(cur.power_type)),
            vec![tok()],
        );
    }
    if prev.is_none_or(|p| p.name != cur.name) {
        script.fire_event("UNIT_NAME_UPDATE", vec![tok()]);
    }
    // UNIT_CLASSIFICATION_CHANGED — the target frame's border-art repaint wire, and
    // the only frame that registers it in the reference. Edge-fired on the gated rank, which IS the
    // classification (`classification_word` is a pure table index), so this fires exactly when the
    // border would change: the creature query landing on a freshly-seen elite, and a mob being
    // enslaved or released. Without it the border would only be right on re-target, because the
    // ref's own `TargetFrame_Update` is the sole other caller of CheckClassification.
    if prev.is_none_or(|p| p.rank != cur.rank) {
        script.fire_event("UNIT_CLASSIFICATION_CHANGED", vec![tok()]);
    }
    // UNIT_FACTION — the PvP-icon repaint wire, fired on the three fields the
    // icon law reads. Exactly the three frames that draw the icon register it in the reference
    // (player, target, party member) and nothing else does. Edge-fired, so the player frame's
    // `igPVPUpdate` sounds once per flag change rather than once per repaint.
    //
    // **And on the TAPPED bit, which is not a faction field and fires this event anyway.** The
    // reference's `UNIT_DYNAMIC_FLAGS` watcher tests bit `0x4` and dispatches event id **29** —
    // `UNIT_FACTION` — at `0x6005a1 test al,4` -> `0x6005b0 mov edx,0x1d` -> `0x515e50`
    // (controlled across all 37 fire sites).
    //
    // This line is the whole reason `UnitIsTapped` is worth publishing. Without it both verbs
    // answer CORRECTLY and no frame ever repaints: pfUI's grey-bar branch runs inside
    // `RefreshUnit`, which is event-driven, so a mob that becomes someone else's tap would stay
    // full-colour until something unrelated happened to refresh it. A passing unit test on the
    // predicates would have said nothing about that — the mechanism existing is not the mechanism
    // applying (`docs/METHOD.md` step 5).
    //
    // Bit `0x8` deliberately has NO arm here: the reference's watcher has none for it either
    // (proven by enumerating all 122 instructions and 12 branches of `0x600440`). The pair is read
    // together, and the `0x4` edge is what moves.
    if prev.is_none_or(|p| {
        (p.pvp, p.is_pvp_ffa, &p.faction_group, p.tapped)
            != (cur.pvp, cur.is_pvp_ffa, &cur.faction_group, cur.tapped)
    }) {
        script.fire_event("UNIT_FACTION", vec![tok()]);
    }
}

fn feed_units(
    script: Option<NonSendMut<UiScript>>,
    // `ChrClasses.dbc` field 16, the only thing `UnitHasRelicSlot` reads. Absent when the client
    // data failed to load, in which case no class reads as having a relic slot — the reference's
    // own bounds-check-fails leg.
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,

    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    selection: Res<Selection>,
    // The guid -> entity map the `"targettarget"` hop lands in, the same index `/assist` resolves
    // its basis' `UNIT_FIELD_TARGET` through (`target::by_name`). `Option` because it belongs to
    // `NetPlugin` (net.rs's `init_resource`) and this feed does not: a UI-only harness runs this
    // system with no net stack at all, and a bare `Res` turns that into a system-validation panic
    // rather than a token that names nobody. Same shape as `factions` below, same reason.
    index: Option<Res<crate::net::GuidIndex>>,
    mut stores: UnitStores,
    mut feed: ResMut<UnitFeedState>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    group: Res<crate::ui_party::GroupState>,
    // The chat window AND the message-sound queue, because this feed shows catalog messages (the
    // rest-state pair, the PvP toggle) and `show_messages` writes both on every line.
    mut sink: crate::ui_action::MessageSink,
    // The guild-identity cache `GetGuildInfo(unit)` reads — `ResMut` because it is a LAZY cache:
    // a lookup that misses is what sends the `CMSG_GUILD_QUERY`, exactly as a
    // `NameCache::resolve` miss sends the name query above.
    mut guild: ResMut<crate::ui_guild::GuildState>,
    // The NPC the player is interacting with, for the `"npc"` token below. `Option` for the same
    // reason `index` and `factions` are: a UI-only harness runs this feed with no session plugin.
    interact: Option<Res<crate::ui_session::InteractNpc>>,
    // The account's rested billing minutes ride the world-enter message, which is the only moment
    // they ever arrive: the value comes off `SMSG_AUTH_RESPONSE` and the reference
    // parks it in a process-lifetime global. `MessageReader` rather than a resource because it is
    // an EDGE, and it is read here rather than in a system of its own because this feed already
    // holds the script.
    // `Option` for the same reason `index`, `factions` and `interact` above are: the message is
    // registered by `NetPlugin`, and a UI-only harness runs this feed with no net stack at all —
    // a bare reader turns that into a system-validation panic.
    entered_world: Option<MessageReader<crate::net::EnteredWorldMessage>>,
) {
    let Some(mut script) = script else {
        return;
    };
    // Ahead of the gate below, which is about descriptor deltas: this is a login edge and has no
    // snapshot to diff.
    if let Some(entered) =
        entered_world.and_then(|mut r| r.read().last().map(|m| m.billing_time_rested))
    {
        script.set_billing_time_rested(entered);
    }
    let chr = classes.as_deref().map(|t| &t.0);
    // One reborrow so the memo (`feed.vm`) and `feed.warned_sideless` can be borrowed as the
    // disjoint fields they are — through the `ResMut` deref they would alias.
    let feed = &mut *feed;
    let (memo, vm_reset) = feed.vm.get_reset(&script);
    let edges = FieldEdges::collect(&mut stores.edges);

    // The gate (1439): every input the two snapshots and the edge diffs below read — any
    // descriptor change or DESPAWN (a removed store is invisible to `Changed`), the selection,
    // the group/reputation/faction state, and the two lazy caches by their landed counters.
    let names_moved = memo.names_generation.moved(names.generation());
    let guild_moved = memo.guild_generation.moved(guild.identity_generation());
    let selection_changed = selection.is_changed();
    let stores_changed = !stores.changed.is_empty();
    let stores_removed = !stores.removed.is_empty();
    let group_changed = group.is_changed();
    let reps_changed = reputations.is_changed();
    let factions_changed = factions.as_ref().is_some_and(|r| r.is_changed());
    // The interaction NPC is an input of the `"npc"` snapshot below, and it moves on frames
    // nothing else does: a vendor window opening or swapping to a second vendor changes no
    // descriptor, no selection, no group. Without this term such a frame skipped the whole
    // feed and the window's own `MERCHANT_SHOW` handler read the previous NPC.
    let interact_changed = interact.as_ref().is_some_and(|r| r.is_changed());
    gate::trace(
        "feed_units",
        &[
            ("vm_reset", vm_reset),
            ("names", names_moved),
            ("guild", guild_moved),
            ("selection", selection_changed),
            ("stores", stores_changed),
            ("removed", stores_removed),
            ("group", group_changed),
            ("reputations", reps_changed),
            ("factions", factions_changed),
            ("interact", interact_changed),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || names_moved
            || guild_moved
            || selection_changed
            || stores_changed
            || stores_removed
            || group_changed
            || reps_changed
            || factions_changed
            || interact_changed,
    );
    stores.removed.clear();
    if gate.skip() {
        return;
    }

    // "player" = our own avatar's descriptor; "target" = the selected entity's. Absent → None, which
    // set_unit clears (UnitExists false), exactly as the real client reports a missing unit. Names
    // resolve through the cache — a miss queries the server once and lands on a later frame.
    let self_pair = self_q.iter().next();
    let player = self_pair.map(|(store, guid)| {
        let name = names
            .resolve_unit(guid.0, Some(store), &commands)
            .map(str::to_string);
        let mut s = snapshot(store, name, 0, chr);
        s.is_player = true;
        // The stated `is_connected` gap, closed for every token this feed pushes — the field's own
        // doc names the feed as what must set it ("mirroring `exists`"), and a token we push at all
        // is one whose descriptor is streamed. It was reachable, not academic: `GREY_ROW["WHISPER"]`
        // (UnitPopup.xml) greys on `not UnitIsConnected(unit)`, so Whisper sat greyed in the
        // right-click menu of every player you targeted. What stays out of reach is real link-death
        // — the only wire that carries it is the group roster's status byte, which the party feed
        // already reads for its own tokens; a non-group player logging out lingers here as
        // connected until that lands.
        s.is_connected = true;
        // Identity + the raid-target board mark (decision 0434 §5's popup gating; §6's board).
        s.guid = guid.0;
        s.raid_target = group.raid_target_index(guid.0);
        s.faction_group = faction_group(store, factions.as_deref());
        s.faction_group_localized = faction_group_localized(store, factions.as_deref());
        // `GetGuildInfo("player")` — the unit's own PUBLIC descriptor fields (191/192) joined
        // against the app's guild-identity cache, which the miss also asks for.
        // Filled here rather than in `snapshot` for the reason `faction_group` and `can_attack`
        // are: it needs a resource, and `snapshot`'s other six call sites hold none.
        s.guild = crate::ui_guild::unit_guild(&store.0, &mut guild, &commands);
        s
    });
    // The GM-mode confound, made audible. A self player whose faction template
    // names no side is almost always GM mode: vmangos swaps a GM to faction template 35, whose
    // `FactionTemplate.dbc` group mask is 0, and every faction-derived surface then has no side to
    // draw — the PvP flag icon most visibly, which simply hides. That is faithful (the only side
    // art that ships is Alliance/Horde/FFA), and it is indistinguishable on screen from the icon
    // being broken, which has now cost two separate sessions an investigation. So it is a BENCH
    // diagnostic, not UI: nothing appears on screen, exactly as in the reference.
    //
    // The honor arc (1512) was once listed here as a SECOND surface behind this same side, and
    // it is not one: the rank title's team digit is `0x5efe00`, which reads the RACE through
    // `ChrRaces`/`FactionTemplate` and never the live template, so a GM's Honor tab names his
    // rank exactly as it always did (`race_pvp_team`, decision 2227 — the cause of report B378
    // was that we had wired the two together, not the GM mode itself). What this warning still
    // covers is every genuinely template-derived surface: `UnitFactionGroup` and the icons and
    // comparisons built on it.
    if let Some(p) = &player {
        let sideless = p.faction_group.is_none();
        if sideless && !feed.warned_sideless {
            warn!(
                "faction: our own template names no side (usually GM mode — vmangos forces \
                 template 35, group mask 0). Every UnitFactionGroup-derived surface loses its \
                 side while this holds — the PvP flag icon stays hidden however flagged you are. \
                 `.gm off` restores it. (The Honor tab's rank title is NOT one of these: its team \
                 digit comes from your race, not your template.)"
            );
        }
        feed.warned_sideless = sideless;
    }
    // (The VM-half memo was taken at the top — the gate needs its reset flag. `warned_sideless`
    // stays server memory outside it, which the disjoint field borrows above preserve.)
    let target = selection.target.zip(selection.guid).and_then(|(e, guid)| {
        let store = stores.all.get(e).ok()?;
        let name = names
            .resolve_unit(guid, Some(store), &commands)
            .map(str::to_string);
        // The target's reaction toward us, on the `UnitReaction` 1..8 scale — the same decode the
        // selection ring runs (reputation-first, else the faction-template comparator). `ring_reaction`
        // returns the raw 0..7 rank (neutral its no-data fallback), which is `UnitReaction − 1`; +1
        // lands it on the Lua scale the name-plate palette (`UnitReactionColor`) indexes.
        let reaction = ring_reaction(
            factions.as_deref(),
            &reputations,
            Some(store),
            self_pair.map(|(s, _)| s),
        ) + 1;
        let mut s = snapshot(store, name, reaction, chr);
        s.guid = guid;
        s.is_connected = true; // see the player leg
        s.raid_target = group.raid_target_index(guid);
        s.faction_group = faction_group(store, factions.as_deref());
        s.faction_group_localized = faction_group_localized(store, factions.as_deref());
        // The byte-confirmed CanAttack 0x606980 — the same predicate TAB and the
        // combat flash run; `UnitCanAttack("player","target")` gates the target frame's
        // difficulty-colored level (ref TargetFrame_CheckLevel).
        s.can_attack = crate::target::can_attack(
            Some(store),
            factions.as_deref(),
            &reputations,
            self_pair.map(|(s, _)| s),
        );
        // `GetGuildInfo("target")` — see the player leg. PLAYER_GUILDID/RANK are PUBLIC, which is
        // the whole reason the binding is per-unit rather than per-player.
        s.guild = crate::ui_guild::unit_guild(&store.0, &mut guild, &commands);
        enrich_unit(
            &mut s,
            guid,
            &names,
            store,
            factions.as_deref(),
            self_pair.map(|(s, _)| s),
        );
        Some(s)
    });

    // `"targettarget"` — the target's own target, one hop off its `UNIT_FIELD_TARGET`. It is the
    // same read `/assist` runs (`target::by_name`), and the same shape 1.12's own token resolver
    // walks for the `target` SUFFIX: resolve the base unit, then follow the guid its target field
    // carries. Only a STREAMED guid resolves — an unstreamed one leaves `UnitExists("targettarget")`
    // false, exactly as an out-of-range party member is false.
    //
    // No `guild` leg, deliberately: `unit_guild` is the LAZY cache whose miss SENDS a
    // `CMSG_GUILD_QUERY` (1257), and nothing asks a target-of-target for its guild — the party and
    // raid tokens leave it unfilled for the same reason. Everything else here is a pure read of a
    // descriptor we already hold.
    let tot = selection
        .target
        .and_then(|e| stores.all.get(e).ok())
        .and_then(|s| s.0.unit_target())
        .filter(|guid| *guid != 0)
        .and_then(|guid| Some((*index.as_ref()?.0.get(&guid)?, guid)))
        .and_then(|(entity, guid)| {
            let store = stores.all.get(entity).ok()?;
            let name = names
                .resolve_unit(guid, Some(store), &commands)
                .map(str::to_string);
            let reaction = ring_reaction(
                factions.as_deref(),
                &reputations,
                Some(store),
                self_pair.map(|(s, _)| s),
            ) + 1;
            let mut s = snapshot(store, name, reaction, chr);
            s.guid = guid;
            s.is_connected = true; // see the player leg
            s.raid_target = group.raid_target_index(guid);
            s.faction_group = faction_group(store, factions.as_deref());
            s.faction_group_localized = faction_group_localized(store, factions.as_deref());
            s.can_attack = crate::target::can_attack(
                Some(store),
                factions.as_deref(),
                &reputations,
                self_pair.map(|(s, _)| s),
            );
            enrich_unit(
                &mut s,
                guid,
                &names,
                store,
                factions.as_deref(),
                self_pair.map(|(s, _)| s),
            );
            Some(s)
        });

    // `"player"` is pushed only while the descriptor EXISTS: its absence is "no data source",
    // never "the player stopped existing". The two absent windows are pre-arrival at login —
    // where a `None` push would erase the roster seat (`seat_from_roster`) that addon file scopes
    // and the loading-screen UI read — and the logout despawn frames, where `PLAYER_LOGOUT`
    // handlers still key their saved state by `UnitName("player")` (the reference keeps the unit
    // valid through its shutdown; a fresh VM starts with no `"player"` anyway, so nothing needs
    // the clear). `"target"` keeps the unconditional push: a selection's absence IS data — the
    // deselect/despawn transition the real client also reports.
    // Both pushes diff against the SAME memo the event loop below uses (1439): an identical
    // snapshot re-pushed is invisible to the VM, so only a real change pays the clone.
    if let Some(cur) = &player {
        if memo.last.get("player") != Some(cur) {
            gate.audit("feed_units", "the player snapshot");
            script.set_unit("player", player.clone());
        }
    }
    let target_dirty = match (&target, memo.last.get("target")) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if target_dirty {
        gate.audit("feed_units", "the target snapshot");
        script.set_unit("target", target.clone());
    }
    // The same "absence IS data" push as the target's above: the target dropping ITS target is a
    // transition the frame has to hear about, and clearing the token is how it hears it.
    let tot_dirty = match (&tot, memo.last.get("targettarget")) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if tot_dirty {
        gate.audit("feed_units", "the target-of-target snapshot");
        script.set_unit("targettarget", tot.clone());
    }

    // `"npc"` — the unit the player is interacting with, resolved through the SAME interaction guid
    // the real client's token reads (`CGGameUI`'s `[0xb4e2d0]`, what
    // `CGGameUI::SetInteractNPC 0x4930d0` writes; the guild registrar's own opener is one of that
    // function's fourteen callers). `crate::ui_session::InteractNpc` is benilla's model of exactly
    // that cell, so the token and the portrait booth cannot disagree about who "npc" is.
    //
    // **It had no feed at all until decision 1678**, and the gap was invisible because nothing
    // called it: `TaxiFrame.xml` and `TradeFrame.xml` both take the NPC's name from an event
    // argument instead and say in their own comments that they are deviating from the reference's
    // `UnitName("NPC")` to do it. `GuildRegistrarFrame.xml` is the first window here to ship the
    // reference's line unchanged — and it painted a blank name banner, because
    // `GUILD_REGISTRAR_SHOW` is verified to carry no arguments, so there is nothing else for it to
    // read. The probe caught it; a one-frame-lag theory was the first (wrong) explanation.
    //
    // No `guild` leg, for `"targettarget"`'s reason: nothing asks an NPC for its guild, and the
    // lookup is the lazy cache whose miss SENDS a query.
    let npc = interact
        .as_deref()
        .and_then(|i| Some((i.0?, i.1?)))
        .and_then(|(entity, guid)| {
            let store = stores.all.get(entity).ok()?;
            let name = names
                .resolve_unit(guid, Some(store), &commands)
                .map(str::to_string);
            let reaction = ring_reaction(
                factions.as_deref(),
                &reputations,
                Some(store),
                self_pair.map(|(s, _)| s),
            ) + 1;
            let mut s = snapshot(store, name, reaction, chr);
            s.guid = guid;
            s.is_connected = true;
            s.raid_target = group.raid_target_index(guid);
            s.faction_group = faction_group(store, factions.as_deref());
            s.faction_group_localized = faction_group_localized(store, factions.as_deref());
            s.can_attack = crate::target::can_attack(
                Some(store),
                factions.as_deref(),
                &reputations,
                self_pair.map(|(s, _)| s),
            );
            enrich_unit(
                &mut s,
                guid,
                &names,
                store,
                factions.as_deref(),
                self_pair.map(|(s, _)| s),
            );
            Some(s)
        });
    // "Absence IS data" again: closing an NPC window must clear the token, or the next window's
    // first frame paints the last NPC's name. **The memo is written here, not only read**: for
    // its first eight days this diff compared against a row nothing ever inserted, so a `Some`
    // re-pushed every frame and a `None` never cleared — `UnitExists("npc")` stayed true after
    // the window closed, and the stale name was what the next window's first frame painted.
    // No `fire_transitions` leg: the reference's watch bridge fires `UNIT_*`
    // for the frames that draw a unit, and nothing draws `"npc"` as a unit frame.
    let npc_dirty = match (&npc, memo.last.get("npc")) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if npc_dirty {
        gate.audit("feed_units", "the interaction-NPC snapshot");
        script.set_unit("npc", npc.clone());
        match &npc {
            Some(cur) => {
                memo.last.insert("npc".to_string(), cur.clone());
            }
            None => {
                memo.last.remove("npc");
            }
        }
    }

    // The XP bar's feed: push our own avatar's PLAYER_XP / PLAYER_NEXT_LEVEL_XP (both PRIVATE, only
    // ever streamed for self) and fire PLAYER_XP_UPDATE when either changes — the coinage feed's
    // shape. Absent fields read 0 (a fresh descriptor's zero default; the bar shows empty until XP
    // streams in). BEFORE the PLAYER_ENTERING_WORLD fire below, so the first paint reads real
    // values (1087 — the tick's handler divides by UnitXPMax).
    if let Some((store, _)) = self_q.iter().next() {
        let xp = store.0.player_xp().unwrap_or(0);
        let next = store.0.player_next_level_xp().unwrap_or(0);
        if memo.last_xp != Some((xp, next)) {
            gate.audit("feed_units", "the XP pair");
            memo.last_xp = Some((xp, next));
            script.set_player_xp(xp, next);
            script.fire_event("PLAYER_XP_UPDATE", vec![]);
        }
    }

    // The rest feed: the `PLAYER_BYTES_2` rest-state byte, the
    // `PLAYER_REST_STATE_EXPERIENCE` pool and PLAYER_FLAGS, pushed as one snapshot. Runs before
    // the PLAYER_ENTERING_WORLD fire below, like the XP push: in the real client the descriptor
    // always lands before that event, so the first paint reads real state — the model's byte-2
    // default (its doc) is the backstop, this ordering is the guarantee itself.
    //
    // **The self-only arms below are each gated on their OWN bits**, which is `0x5ee990`'s actual
    // shape and not what this comment said until 2078. Everything from `0x5eea93` down — where the
    // handler compares the changed player's GUID against the local one — is self-only, and inside
    // that region each arm carries its own `test`:
    //
    // ```
    // 5eead0  f6 45 fc 20   test byte [ebp-4],0x20   ; the RESTING bit CHANGED?
    // 5eead4  74 26         je   0x5eeafc            ;   no -> skip the whole arm
    // 5eead6..5eeaed              tutorial popup 0x4b5390(0x1d), only if the bit is now SET
    // 5eeaf2  b9 95 01 00 00 / e8 …  mov ecx,0x195 ; call 0x703e50   <- PLAYER_UPDATE_RESTING
    // 5eeafc  ...
    // 5eeaff  f6 c4 02      test ah,0x2              ; bit 0x200, the PvP-desired arm
    // 5eeb65  f6 c4 30      test ah,0x30             ; bits 0x1000|0x2000, the play-time regimes
    // 5eeb6f  e8 …          call 0x703e50 (ecx=0x212) <- PLAYTIME_CHANGED
    // ```
    //
    // `0x5eeaf2` sits INSIDE the `0x20` arm (`0x5eeaf2 < 0x5eeafc`, the `je`'s target), so
    // **PLAYER_UPDATE_RESTING fires only on the resting bit's own edge** — not "on every flags
    // change, not only the resting bit", the reading of `0x5ee990` this feed was built against,
    // which made every helm toggle, every leadership pass and every AFK flip announce a resting
    // change to the UI. The same reading's "argless unconditionally" is true only of the
    // *direction*: the inner `0x5eeae4 je` skips the tutorial popup, never the fire, so both edges
    // of the bit fire it. The bytes above are read straight out of `WoW.exe` (file offset
    // 0x1eead0, PE imagebase 0x400000, .text RVA 0x1000 → raw 0x1000).
    //
    // `UPDATE_EXHAUSTION` is unchanged and was already right: two separate watchers, `0x5de4e0` on
    // the rest-state byte and `0x5de4b0` on the pool field, neither of them this handler.
    if let Some((store, _)) = self_q.iter().next() {
        let rest = (
            store.0.player_rest_state().unwrap_or(0),
            store.0.player_rest_state_experience().unwrap_or(0),
            store.0.player_flags(),
        );
        if memo.last_rest != Some(rest) {
            gate.audit("feed_units", "the rest snapshot");
            let prev = memo.last_rest;
            memo.last_rest = Some(rest);
            script.set_rest_state(rest.0, rest.1, rest.2 & PLAYER_FLAGS_RESTING != 0);
            // The same descriptor word carries the two play-time bits, and they ride the same
            // memo — the PUSH is one snapshot because the wire delivers one dword; only the
            // EVENTS below split by bit.
            script.set_play_time(
                rest.2 & PLAYER_FLAGS_PARTIAL_PLAY_TIME != 0,
                rest.2 & PLAYER_FLAGS_NO_PLAY_TIME != 0,
            );
            if prev.map(|p| (p.0, p.1)) != Some((rest.0, rest.1)) {
                script.fire_event("UPDATE_EXHAUSTION", vec![]);
            }
            // "You feel rested." / "You feel normal.": the BYTE watcher alone
            // messages, and only on a real transition — `prev` None is the login descriptor,
            // which the real client's fresh-CREATE path never runs through the notify pass
            // (byte-verified: login is structurally silent). The pool watcher never messages.
            if let Some(p) = prev {
                let line = rest_state_message(p.0, rest.0)
                    .and_then(|key| crate::ui_action::keyed_line(&script, key));
                crate::ui_action::show_messages(&mut script, &mut sink, "ui_unit", line);
            }
            // The two self-only flag events, each on its own bits' edge (`0x5eead0` and
            // `0x5eeb65` above). `prev` None is the login descriptor and stays silent for both,
            // the same fresh-CREATE reasoning the rest message carries — a mirror-diff watcher
            // has nothing to diff against on a create.
            if let Some(p) = prev {
                let moved = p.2 ^ rest.2;
                if moved & PLAYER_FLAGS_RESTING != 0 {
                    script.fire_event("PLAYER_UPDATE_RESTING", vec![]);
                }
                // PLAYTIME_CHANGED (id 530) — `0x5eeb65 test ah,0x30`, argless, self-only. Stock
                // `PlayerFrame.lua:23` registers it and `PlayerFrame_UpdatePlaytime` reads
                // `PartialPlayTime()`/`NoPlayTime()`, both of which we already feed off this very
                // snapshot; the event was the one missing half.
                if moved & (PLAYER_FLAGS_PARTIAL_PLAY_TIME | PLAYER_FLAGS_NO_PLAY_TIME) != 0 {
                    script.fire_event("PLAYTIME_CHANGED", vec![]);
                }
            }
        }
    }

    // The ding feed: `PLAYER_LEVEL_UP` (arg1 = the new level) when our avatar's
    // `UNIT_FIELD_LEVEL` CHANGES — the event the exhaustion tick (1082) and the max-level rail
    // (1094) register; it was a dead registration until 1094. Any change, not only a rise:
    // vmangos `GiveLevel` runs for demotions too (a GM `.character level` down) and sends
    // `SMSG_LEVELUP_INFO` unconditionally, so the real client hears every change — 1094's
    // rise-only guard left the rail latched shown after a 60→1 demote (the 1106 live repro).
    // Trigger PROVISIONAL (0578's pattern): fired off the descriptor diff, which lands in the
    // same update batch as the ding's XP fields, so consumers read a coherent picture. The
    // real client plausibly fires it from its `SMSG_LEVELUP_INFO` handler instead, with the
    // packet's gain tuple as arg2+.
    //
    // **THE ARGS ARE NOT OPTIONAL, AND THE CLAIM THAT USED TO STAND HERE WAS WRONG.** It said no
    // 1.12 FrameXML consumer reads past arg1, citing `ReputationWatchBar_Update` (arg1) and the
    // tick's handler (none). It missed the main one: `ChatFrame.lua:1283-1320` reads **arg1
    // through arg9** — the level, the health and mana gains, the talent points, and the five stat
    // gains, each printed as its own system line. The reference's own fire site says the same,
    // `%d%d%d%d%d%d%d%d%d` (SignalEvent2). Two consumers were surveyed, the conclusion was drawn
    // from two, and the third is the one that matters.
    //
    // We fire arg1 alone, so the stock ChatFrame's `if ( arg3 > 0 )` would compare nil with a
    // number and raise — this blocks the ChatFrame window. The gains are on a packet we already
    // parse (`SMSG_LEVELUP_INFO`, twelve u32) and already spend: `ui_chat/feed.rs` composes those
    // very lines in Rust because the event could not carry them. Decision 1884 scopes plumbing
    // the tuple here and retiring the Rust duplicate; this trigger is a descriptor diff and the
    // gains arrive on the packet, so it is a join, not a one-liner.
    if let Some((store, _)) = self_q.iter().next() {
        if let Some(level) = store.0.unit_level() {
            let prev = memo.last_level.replace(level);
            if prev.is_some_and(|p| level != p) {
                gate.audit("feed_units", "the level edge");
                // All nine, per the reference's own fire site (`%d%d%d%d%d%d%d%d%d`): level,
                // health gain, mana gain, talent points, then the five stat gains in
                // `SPELL_STAT0..4` order. Absent gains are ZEROS, not a shorter payload — a
                // demotion really did gain nothing, and every consumer guards with `if ( argN > 0 )`
                // so zero reads as "no line" while nil raises.
                let (info, talent_points) = sink.chat.take_level_up_gains(level).unzip();
                let gain = |f: fn(&benilla_protocol::messages::LevelUpInfo) -> u32| {
                    ScriptValue::Int(i64::from(info.as_ref().map_or(0, f)))
                };
                script.fire_event(
                    "PLAYER_LEVEL_UP",
                    vec![
                        ScriptValue::Int(i64::from(level)),
                        gain(|l| l.health),
                        gain(|l| l.powers[0]),
                        ScriptValue::Int(i64::from(talent_points.unwrap_or(0))),
                        gain(|l| l.stats[0]),
                        gain(|l| l.stats[1]),
                        gain(|l| l.stats[2]),
                        gain(|l| l.stats[3]),
                        gain(|l| l.stats[4]),
                    ],
                );
            }
        }
    }

    // The action-bar toggle feed: `PLAYER_FIELD_BYTES` byte 2 — which of the four extra bars the
    // player has switched on. PRIVATE, like the combo byte one address down (`+0x1029` vs
    // `+0x102a`), and pushed on the EDGE.
    //
    // **This push is the ONLY thing that moves the VM's copy**, and that is the mechanism rather
    // than our simplification: no instruction in the real client writes this cell (the one
    // `+0x102a` access image-wide is `GetActionBarToggles`' read at `0x4e768c`), so
    // `SetActionBarToggles` posts the byte and leaves the descriptor alone until the server's
    // UPDATE_OBJECT echoes it. Nothing is notified when it lands either (all 49 field-change
    // registrations at `0x468070` were enumerated; none sits at an offset ≥ `0x1000`), so there is
    // **no event to fire here** — the reference reads the binding exactly once, in `UIParent.lua`'s
    // `PLAYER_ENTERING_WORLD` handler, and keeps `SHOW_MULTI_ACTIONBAR_1..4` as its optimistic copy
    // in between.
    //
    // Which is why this sits ABOVE the fire, on 1087's precedent for the XP/rest pushes: the
    // handler that reads `GetActionBarToggles()` runs synchronously inside `fire_event`, so a push
    // below it would hand the first-paint the previous frame's value — four nils.
    //
    // `unwrap_or(0)` is faithful, not a shrug: with no local player the reference's chain fails
    // soft and the getter returns four `nil`s, which is exactly what a zero byte returns — "not in
    // world" and "byte == 0" share the branch (`0x4e7684`) and are indistinguishable to Lua.
    if let Some((store, _)) = self_q.iter().next() {
        let toggles = store.0.player_action_bar_toggles().unwrap_or(0);
        if memo.action_bar_toggles != Some(toggles) {
            gate.audit("feed_units", "the action-bar toggle byte");
            memo.action_bar_toggles = Some(toggles);
            script.set_action_bar_toggles(toggles);
        }
    }

    // Initial pull: fire PLAYER_ENTERING_WORLD once PER WORLD ENTRY so frames do their first
    // paint on their own — gated on our avatar's descriptor EXISTING. 1087 stated the real
    // client's guarantee (the player object lands before this event) and moved the XP/rest
    // pushes above the fire, but the fire itself still went out on frame 1, seconds before
    // login: every one-shot first-paint read empty state, and only consumers with their own
    // diff events recovered. The 1094 live probe caught the one that couldn't — a level-60
    // login read UnitLevel()=0 at the fire and kept the XP strip. With the gate the guarantee
    // is structural for every consumer.
    //
    // The absent arm is the world-EXIT edge (logout / char switch — the self entity despawns
    // with the streamed world): the real client fires this event on *every* world entry, and
    // since 1290 so do we — the frame tree is torn down and rebuilt across that edge, the way
    // the reference does it. So re-arm the fire and forget the player-global diff
    // memories. Forgetting them makes every next-login first sighting re-seed SILENTLY — the
    // byte-verified fresh-CREATE notify silence (1098 §4), now holding per entry: without it a
    // 60→1 char switch latched the max-level rail shown over a level-1 body (1106's live
    // repro), and a normal→rested char switch would misfire "You feel rested." at login.
    if self_pair.is_some() {
        if !memo.entered_world {
            gate.audit("feed_units", "the PLAYER_ENTERING_WORLD arm");
            script.fire_event("PLAYER_ENTERING_WORLD", vec![]);
            memo.entered_world = true;
        }
    } else if memo.entered_world {
        gate.audit("feed_units", "the world-exit disarm");
        memo.entered_world = false;
        memo.last_xp = None;
        memo.last_rest = None;
        memo.last_level = None;
        memo.last_combo = None;
        memo.in_combat = None;
        memo.pvp_desired = None;
        // The worn-display pair is a player-global like the rest, and forgetting it is what makes
        // the next character's preference reach the VM at all: the push is an EDGE, so a memo
        // carrying the last body's bits would silently skip a new body that happens to disagree
        // with the VM's fresh "both shown" default.
        memo.worn_hidden = None;
        // Same reason as the worn-display pair: the push is an EDGE, so a memo carrying the last
        // body's byte would skip a new character whose own toggles happen to match it — and this
        // one has no optimistic default to fall back on, only four nils.
        memo.action_bar_toggles = None;
    }

    for (token, snap) in [
        ("player", &player),
        ("target", &target),
        ("targettarget", &tot),
    ] {
        match snap {
            Some(cur) => {
                let prev = memo.last.get(token);
                if prev != Some(cur) {
                    gate.audit("feed_units", "a unit-token transition");
                    fire_transitions(&mut script, token, prev, cur, &edges);
                    memo.last.insert(token.to_string(), cur.clone());
                }
            }
            None => {
                // Clearing a token isn't a UNIT_* event; the target frame reacts to
                // PLAYER_TARGET_CHANGED below.
                if memo.last.remove(token).is_some() {
                    gate.audit("feed_units", "a unit-token clear");
                }
            }
        }
    }

    // PLAYER_TARGET_CHANGED (no args, real WoW's shape) when the selection changes.
    if selection.guid != memo.target_guid {
        gate.audit("feed_units", "the PLAYER_TARGET_CHANGED edge");
        memo.target_guid = selection.guid;
        script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    }

    // PLAYER_REGEN_DISABLED/ENABLED: the self in-combat flag transition (`UNIT_FIELD_FLAGS`
    // bit `UNIT_FLAG_IN_COMBAT 0x00080000`, vmangos `UnitDefines.h:564`) — the center combat
    // text's ENTERING/LEAVING_COMBAT feed (the trigger is PROVISIONAL pending
    // the COMBAT_TEXT_UPDATE emission pin).
    if let Some((store, _)) = self_pair {
        let in_combat = store.0.unit_flags() & 0x0008_0000 != 0;
        if memo.in_combat != Some(in_combat) {
            gate.audit("feed_units", "the combat-flag edge");
            let first_sight = memo.in_combat.is_none();
            memo.in_combat = Some(in_combat);
            if !first_sight || in_combat {
                script.fire_event(
                    if in_combat {
                        "PLAYER_REGEN_DISABLED"
                    } else {
                        "PLAYER_REGEN_ENABLED"
                    },
                    vec![],
                );
            }
        }
    }

    // The PvP toggle's own feedback. The reference's local-player PLAYER_FLAGS
    // change handler reacts to the PVP_DESIRED bit — and to nothing else about PvP — with two
    // lines: a yellow UI_INFO_MESSAGE toast, then the verbose sentence as a system chat line. It is
    // keyed on the *preference*, not on the flag the icon draws, which is exactly why it matters:
    // toggling OFF changes no visible flag for five minutes, so without these two lines the key
    // reads as dead. The reference only announces its own player (the whole branch sits behind a
    // guid == localPlayer gate) and only on a change, never on the descriptor that first carries it.
    if let Some((store, _)) = self_pair {
        let desired = store.0.player_flags() & PLAYER_FLAGS_PVP_DESIRED != 0;
        if let Some((toast, verbose)) = pvp_announcement(memo.pvp_desired, desired) {
            gate.audit("feed_units", "the PvP-desired edge");
            // The toast is a catalog row, so its surface is read there; the verbose sentence is
            // not one, so it is emitted `unkeyed` on the handler's own chat surface rather than
            // pushed through `Shown::keyed`, whose unknown-key fallback would turn it RED (2054).
            let lines = [
                crate::ui_action::keyed_line(&script, toast),
                script
                    .lua()
                    .globals()
                    .get::<String>(verbose)
                    .ok()
                    .filter(|t| !t.is_empty())
                    .map(|t| {
                        crate::ui_action::Shown::unkeyed(benilla_ui::messages::MsgKind::Chat, t)
                    }),
            ];
            crate::ui_action::show_messages(
                &mut script,
                &mut sink,
                "ui_unit",
                lines.into_iter().flatten(),
            );
        }
        memo.pvp_desired = Some(desired);
    }

    // The worn-display pair: `PLAYER_FLAGS`' two hide bits, mirrored into the VM
    // so `ShowingHelm()`/`ShowingCloak()` — the Options rows' getters — read the server's truth.
    // On the EDGE, not per frame: the setter flips the VM's belief the instant the box is clicked,
    // and the descriptor only catches up a round trip later. Re-pushing the stale pair in between
    // would un-click the box and make a second click compute the wrong flip.
    if let Some((store, _)) = self_pair {
        let hidden = (store.0.player_hides_helm(), store.0.player_hides_cloak());
        if memo.worn_hidden != Some(hidden) {
            gate.audit("feed_units", "the worn-display pair");
            memo.worn_hidden = Some(hidden);
            script.set_worn_display(!hidden.0, !hidden.1);
        }
    }

    // The combo-point feed: `PLAYER_FIELD_BYTES` byte 1 and the `PLAYER_FIELD_COMBO_TARGET` GUID
    // it is banked against, both PRIVATE, pushed as the pair the server writes.
    //
    // Both halves are PUSHED whenever either moves — `GetComboPoints` reads both, so a stale
    // target would make it lie on the next call. Only the COUNT fires the event: the client's
    // `PLAYER_COMBO_POINTS` (event 202, `0x5ddff0`) is registered at `0x5dd9d9` as a **one-byte
    // field-change watch on `+0x1029`** and nothing else — the combo-target field has no watch of
    // its own. The watch carries no value test, so it fires on the drop back to zero too,
    // which is the edge `ComboFrame` needs to hide itself when the server's 4 s
    // `REACTIVE_OVERPOWER` timer clears the point.
    //
    // The reader's other input — which unit is selected — reaches it through
    // `PLAYER_TARGET_CHANGED` above, the event the reference registers `ComboFrame` for precisely
    // because `GetComboPoints` consults the current target.
    if let Some((store, _)) = self_q.iter().next() {
        let banked = (
            store.0.player_combo_points().unwrap_or(0),
            store.0.player_combo_target(),
        );
        if let Some(fire) = combo_edge(memo.last_combo, banked) {
            gate.audit("feed_units", "the combo-point edge");
            memo.last_combo = Some(banked);
            script.set_combo_points(banked.0, banked.1);
            if fire {
                script.fire_event("PLAYER_COMBO_POINTS", vec![]);
            }
        }
    }
}

/// The combo feed's edge, given the last `(count, banked-target)` pair pushed and the current one:
/// `None` = nothing moved, `Some(fire)` = push the pair, and whether `PLAYER_COMBO_POINTS` fires.
///
/// The two halves diverge because the client's watches do. Event 202 is registered at `0x5dd9d9`
/// as a **one-byte field-change watch on `+0x1029`** — the count — and `PLAYER_FIELD_COMBO_TARGET`
/// carries no watch of its own. So a re-bank onto a different unit at
/// an unchanged count moves the *value* `GetComboPoints` reads without announcing itself, and the
/// UI hears about it through `PLAYER_TARGET_CHANGED` instead. The watch has no value test, so the
/// drop back to zero fires like any other change — the edge that takes the dots down.
fn combo_edge(last: Option<(u8, u64)>, now: (u8, u64)) -> Option<bool> {
    (last != Some(now)).then(|| last.map(|(count, _)| count) != Some(now.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DESCRIPTOR leg of the same answer: `snapshot` takes the team digit off
    /// `UNIT_FIELD_BYTES_0` byte 0 and **never** off `UNIT_FIELD_FACTIONTEMPLATE`.
    ///
    /// The pane-level regression (`ui_script::honor_frame_tests::a_gm_flagged_player_…`) seats
    /// `pvp_team` by hand, so it proves the key is built from the right field and not that the
    /// right field is read. This is that half: a template-35 GM — the exact descriptor vmangos
    /// gives one — still answers his race's side.
    #[test]
    fn the_team_digit_comes_off_the_race_byte_not_the_faction_template() {
        use benilla_protocol::ObjectFields;
        /// `UNIT_FIELD_FACTIONTEMPLATE` / `UNIT_FIELD_BYTES_0`, one dword apart — the reference's
        /// own `[obj+0x110]+0x74` and `+0x78`.
        const FACTIONTEMPLATE: u16 = 35;
        const BYTES_0: u16 = 36;
        /// vmangos's GM template: `FactionTemplate.dbc` group mask 0, friendly to everyone.
        const GM_TEMPLATE: u32 = 35;

        let team = |fields: &[(u16, u32)]| {
            snapshot(
                &ObjectStore(ObjectFields::from_pairs(fields)),
                None,
                0,
                None,
            )
            .pvp_team
        };
        // Byte 0 of BYTES_0 is the race; the class in byte 1 must not disturb it.
        let human_warrior = 1 | (1 << 8);
        let scourge_mage = 5 | (8 << 8);
        assert_eq!(team(&[(BYTES_0, human_warrior)]), 1, "Human → Alliance");
        assert_eq!(team(&[(BYTES_0, scourge_mage)]), 0, "Scourge → Horde");
        // **The report.** The sideless GM template sits right beside the race byte and is not
        // consulted: the answer is the race's, unchanged.
        assert_eq!(
            team(&[(BYTES_0, human_warrior), (FACTIONTEMPLATE, GM_TEMPLATE)]),
            1,
            "a GM keeps his race's side (report B378)"
        );
        assert_eq!(
            team(&[(BYTES_0, scourge_mage), (FACTIONTEMPLATE, GM_TEMPLATE)]),
            0,
            "…on both sides"
        );
        // A unit whose race byte has not streamed is the engine's bounds-failure −1, and a
        // faction template alone cannot stand in for it.
        assert_eq!(team(&[]), -1, "no race byte, no team digit");
        assert_eq!(
            team(&[(FACTIONTEMPLATE, 1)]),
            -1,
            "and a template is not one"
        );
    }

    /// [`race_pvp_team`]'s frozen table against the **shipped tables it is a copy of** — the walk
    /// the engine runs at `0x5efe00`, on the real `ChrRaces.dbc` and `FactionTemplate.dbc`.
    ///
    /// The table is hardcoded because it is nine constant rows of a 2006 file and threading a DBC
    /// resource through every unit snapshot to read them would be pure ceremony. This is what
    /// makes that safe: the file decides, and a row that ever disagrees — or a race the file has
    /// and the table does not (race 9, Goblin, which shares Human's template and is **not** a
    /// `None`) — fails here. Skips without client data.
    #[test]
    fn race_pvp_team_matches_the_shipped_tables() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let want = benilla_formats::load_race_pvp_teams(&mut chain).expect("ChrRaces walk");
        // The misparse guard: 5875 ships nine rows, not eight. An empty or truncated map would
        // otherwise let this test pass by asserting nothing.
        assert_eq!(want.len(), 9, "ChrRaces.dbc row count");
        for (&race, &team) in &want {
            assert_eq!(race_pvp_team(race), team, "race {race}");
        }
        // Both sides are actually represented — a walk that answered one digit for everything
        // would satisfy the loop above and name every rank off one list.
        assert!(want.values().any(|&t| t == 0), "some race is Horde");
        assert!(want.values().any(|&t| t == 1), "some race is Alliance");
        // Off the end of the file is the engine's bounds-failure tail, not a guess.
        for race in [0u8, 10, 255] {
            assert!(!want.contains_key(&race));
            assert_eq!(race_pvp_team(race), -1, "race {race} has no ChrRaces row");
        }
    }

    /// **The tapped bit fires `UNIT_FACTION`, and without this the verbs are decorative.**
    ///
    /// `UnitIsTapped` answering correctly is only half of it. pfUI's grey-bar branch lives inside
    /// `RefreshUnit`, which is event-driven — so a mob that becomes someone else's tap would stay
    /// full-colour until something unrelated happened to repaint the frame. The predicate would be
    /// right and the screen would be wrong, which no test of the predicate can see (`docs/METHOD.md`
    /// step 5: the mechanism existing is not the mechanism applying).
    ///
    /// The reference dispatches event id **29** — `UNIT_FACTION`, not a tapped-specific event —
    /// from its `UNIT_DYNAMIC_FLAGS` watcher's bit-`0x4` arm (`0x6005a1 test al,4` ->
    /// `0x6005b0 mov edx,0x1d` -> `0x515e50`, controlled across all 37 fire sites image-wide).
    /// Reusing the faction event for a non-faction field is not something anyone would invent,
    /// which is exactly why it is pinned.
    ///
    /// Bit `0x8` has **no** arm, in the reference or here — proven there by enumerating all 122
    /// instructions and 12 branches of the watcher. The negative half is asserted too, because
    /// "fire on both, it's cheaper" is the obvious wrong simplification.
    /// `UNIT_FLAGS` rides the raw field: any change of `UNIT_FIELD_FLAGS` fires it with the
    /// token, an unchanged field does not, and a unit's first snapshot is NOT a transition — the
    /// reference's watch bridge has no watch to fire on the create leg (1953, corrected 1957).
    #[test]
    fn a_flags_change_fires_unit_flags_with_the_token() {
        let fired = |prev: Option<UnitState>, cur: UnitState, edges: &FieldEdges| -> Vec<String> {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_FLAGS")
                f:SetScript("OnEvent", function() table.insert(SEEN, event .. ":" .. arg1) end)
            "#,
            )
            .unwrap();
            fire_transitions(&mut s, "pet", prev.as_ref(), &cur, edges);
            s.eval::<Vec<String>>("return SEEN").unwrap()
        };
        const PET: u64 = 0xF140_0000_0000_0001;
        let base = UnitState {
            exists: true,
            has_object: true,
            guid: PET,
            flags: 0x8,
            ..Default::default()
        };
        let moved = FieldEdges::of(&[(PET, benilla_protocol::field::FIELD_UNIT_FLAGS)]);
        assert_eq!(
            fired(
                Some(base.clone()),
                UnitState {
                    flags: 0x8 | 0x0400_0000,
                    ..base.clone()
                },
                &moved,
            ),
            vec!["UNIT_FLAGS:pet".to_string()]
        );
        assert_eq!(
            fired(Some(base.clone()), base.clone(), &FieldEdges::default()),
            Vec::<String>::new()
        );
        // The create block runs no notify pass: no edge, no event — whatever the snapshot says.
        assert_eq!(
            fired(None, base.clone(), &FieldEdges::default()),
            Vec::<String>::new()
        );
        // …and the trigger is the UNIT'S edge, not the token's history: a token acquired on the
        // very frame the field moved hears it (the reference fans out to whoever names the unit
        // at notify time), and another unit's edge is not this one's.
        assert_eq!(
            fired(None, base.clone(), &moved),
            vec!["UNIT_FLAGS:pet".to_string()]
        );
        assert_eq!(
            fired(
                Some(base.clone()),
                base.clone(),
                &FieldEdges::of(&[(PET + 1, benilla_protocol::field::FIELD_UNIT_FLAGS)]),
            ),
            Vec::<String>::new()
        );
    }

    /// The two edges on the player's own state (1953): the control flag's, which fires LOST on
    /// the way down and GAINED on the way up and nothing while it holds (the boot value is "in
    /// control"), and the far-sight field's, which fires on every change including the clear.
    #[test]
    fn the_control_and_far_sight_edges_fire_once_each_way() {
        use bevy::prelude::*;
        const FIELD_PLAYER_FARSIGHT: u16 = 712;
        let mut app = App::new();
        app.init_resource::<crate::player::Player>()
            .add_systems(Update, (feed_player_control, feed_farsight_focus));
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_CONTROL_LOST")
                f:RegisterEvent("PLAYER_CONTROL_GAINED")
                f:RegisterEvent("PLAYER_FARSIGHT_FOCUS_CHANGED")
                f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(0x77),
                ObjectStore(benilla_protocol::ObjectFields::default()),
            ))
            .id();
        let seen = |app: &mut App| -> Vec<String> {
            app.update();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };
        assert_eq!(
            seen(&mut app),
            Vec::<String>::new(),
            "in control, no far sight: quiet"
        );
        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .control_lost = true;
        assert_eq!(seen(&mut app), vec!["PLAYER_CONTROL_LOST".to_string()]);
        assert_eq!(seen(&mut app), Vec::<String>::new(), "held, not repeated");
        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .control_lost = false;
        assert_eq!(seen(&mut app), vec!["PLAYER_CONTROL_GAINED".to_string()]);

        let set_farsight = |app: &mut App, guid: u64| {
            app.world_mut().entity_mut(me).insert(ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&[
                    (FIELD_PLAYER_FARSIGHT, guid as u32),
                    (FIELD_PLAYER_FARSIGHT + 1, (guid >> 32) as u32),
                ]),
            ));
        };
        set_farsight(&mut app, 0xf130_0000_0000_0042);
        assert_eq!(
            seen(&mut app),
            vec!["PLAYER_FARSIGHT_FOCUS_CHANGED".to_string()],
            "set — whether or not the guid resolves"
        );
        assert_eq!(seen(&mut app), Vec::<String>::new());
        set_farsight(&mut app, 0);
        assert_eq!(
            seen(&mut app),
            vec!["PLAYER_FARSIGHT_FOCUS_CHANGED".to_string()],
            "cleared — the other leg"
        );
    }

    #[test]
    fn the_tapped_bit_fires_unit_faction_and_the_by_player_bit_fires_nothing() {
        let fired = |prev: UnitState, cur: UnitState| -> Vec<String> {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_FACTION")
                f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
            "#,
            )
            .unwrap();
            fire_transitions(&mut s, "target", Some(&prev), &cur, &FieldEdges::default());
            s.eval::<Vec<String>>("return SEEN").unwrap()
        };
        let base = UnitState {
            exists: true,
            has_object: true,
            ..Default::default()
        };

        // The 0x4 edge fires it.
        assert_eq!(
            fired(
                base.clone(),
                UnitState {
                    tapped: true,
                    ..base.clone()
                }
            ),
            vec!["UNIT_FACTION".to_string()],
            "a unit becoming tapped must repaint the frames that draw it"
        );
        // ...and back the other way, when the tap is released.
        assert_eq!(
            fired(
                UnitState {
                    tapped: true,
                    ..base.clone()
                },
                base.clone()
            ),
            vec!["UNIT_FACTION".to_string()],
        );
        // The 0x8 edge fires NOTHING — the reference's watcher has no arm for it.
        assert!(
            fired(
                UnitState {
                    tapped: true,
                    ..base.clone()
                },
                UnitState {
                    tapped: true,
                    tapped_by_player: true,
                    ..base.clone()
                }
            )
            .is_empty(),
            "bit 0x8 has no delta arm — firing on it would be an invention"
        );
        // Nothing moved: nothing fires. The control that stops this passing by firing always.
        assert!(fired(base.clone(), base.clone()).is_empty());
    }

    /// **`PLAYER_FLAGS_CHANGED` — the event a migrated stock window listened for and nothing
    /// fired**, pinned on the three properties that make it what it is.
    ///
    /// The gap was invisible from both ends, which is 1819's shape exactly: `TargetFrame.lua`
    /// registered the name and this feed never spoke it, so the target frame's party-leader icon
    /// only ever refreshed on a re-target. The census that caught it
    /// (`reference_ui::every_event_a_chain_file_registers_has_a_producer`) described it as "the
    /// AFK/DND badge"; the handler at `TargetFrame.lua:88-95` is the LEADER icon, and 1.12 has no
    /// AFK/DND badge on any unit frame — so the control below is a bit no `UnitState` field
    /// decodes, proving the trigger is the raw dword and not the two bools next to it.
    #[test]
    fn player_flags_changed_fires_per_token_on_any_bit_and_never_on_first_sight() {
        let fired = |prev: Option<UnitState>, cur: UnitState, edges: &FieldEdges| -> Vec<String> {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_FLAGS_CHANGED")
                f:SetScript("OnEvent", function() table.insert(SEEN, event .. ":" .. arg1) end)
            "#,
            )
            .unwrap();
            fire_transitions(&mut s, "target", prev.as_ref(), &cur, edges);
            s.eval::<Vec<String>>("return SEEN").unwrap()
        };
        const THEM: u64 = 0x2a;
        let base = UnitState {
            exists: true,
            has_object: true,
            guid: THEM,
            ..Default::default()
        };
        let with = |flags: u32| UnitState {
            player_flags: flags,
            group_leader: flags & 0x1 != 0,
            ghost: flags & 0x10 != 0,
            ..base.clone()
        };
        let moved = FieldEdges::of(&[(THEM, benilla_protocol::field::FIELD_PLAYER_FLAGS)]);
        let still = FieldEdges::default();

        // The 1.12 consumer's own bit: `PLAYER_FLAGS_GROUP_LEADER 0x1`, `UnitIsPartyLeader`'s
        // descriptor leg — leadership passing to the player you are targeting.
        assert_eq!(
            fired(Some(with(0)), with(0x1), &moved),
            vec!["PLAYER_FLAGS_CHANGED:target".to_string()],
            "arg1 is the unit token, and there is no arg2 (0x515e50 -> 0x703f50(id, \"%s\", token))"
        );
        // …and back down. The reference tests the XOR-diff, not the new value.
        assert_eq!(
            fired(Some(with(0x1)), with(0), &moved),
            vec!["PLAYER_FLAGS_CHANGED:target".to_string()]
        );

        // **The control that proves the trigger is the RAW DWORD.** `PLAYER_FLAGS_HIDE_HELM 0x400`
        // is a bit no `UnitState` field decodes — the fire at `0x5eea35` carries no bit test, so
        // it announces this exactly as loudly as the leader bit: the edge is the dword's, and the
        // snapshot's decoded bools are never consulted.
        assert_eq!(
            fired(Some(with(0)), with(0x400), &moved),
            vec!["PLAYER_FLAGS_CHANGED:target".to_string()],
            "an undecoded bit still fires it — the handler tests no bit at all"
        );

        // First sight is the CREATE, and a CREATE runs no notify pass (1098 §4) — the same
        // posture `UNIT_FLAGS` holds one field over: no edge, no event.
        assert!(
            fired(None, with(0x1), &still).is_empty(),
            "a unit's first snapshot is its create, not a transition"
        );
        // The control that stops all of the above passing by firing always.
        assert!(fired(Some(with(0x1)), with(0x1), &still).is_empty());
    }

    /// **`UNIT_DYNAMIC_FLAGS` — an event benilla had never fired at all**.
    ///
    /// Same bridge, same shape, the third of the three raw-dword arms: id 137 is the unit-window
    /// index of descriptor field 143, the name-table slot for 137 has exactly one writer in the
    /// image and it points at the string `"UNIT_DYNAMIC_FLAGS"`, and the watch length is 4 — one
    /// dword — so the gate is a `repe cmpsb` over the whole word and **any** bit fires it.
    ///
    /// The control below is bit `0x2` (`UNIT_DYNFLAG_TRACK_UNIT`, Hunter's Mark), which no
    /// `UnitState` field decodes: an arm driven off `tapped`/`tapped_by_player` passes every other
    /// assertion here and fails that one.
    #[test]
    fn unit_dynamic_flags_fires_per_token_on_any_bit_and_never_on_first_sight() {
        let fired = |prev: Option<UnitState>, cur: UnitState, edges: &FieldEdges| -> Vec<String> {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_DYNAMIC_FLAGS")
                f:SetScript("OnEvent", function() table.insert(SEEN, event .. ":" .. arg1) end)
            "#,
            )
            .unwrap();
            fire_transitions(&mut s, "target", prev.as_ref(), &cur, edges);
            s.eval::<Vec<String>>("return SEEN").unwrap()
        };
        const MOB: u64 = 0xF130_0000_0000_0007;
        let moved = FieldEdges::of(&[(MOB, benilla_protocol::field::FIELD_UNIT_DYNAMIC_FLAGS)]);
        let still = FieldEdges::default();
        let with = |dyn_flags: u32| UnitState {
            exists: true,
            has_object: true,
            guid: MOB,
            dynamic_flags: dyn_flags,
            tapped: dyn_flags & 0x4 != 0,
            tapped_by_player: dyn_flags & 0x8 != 0,
            ..Default::default()
        };

        // The bit the corpus registers this event for: `0x4` TAPPED, the grey-bar state
        // (`CT_UnitFrames/CT_TargetFrame.xml:200`, `TipBuddy/TipBuddy.lua:17`).
        assert_eq!(
            fired(Some(with(0)), with(0x4), &moved),
            vec!["UNIT_DYNAMIC_FLAGS:target".to_string()],
            "arg1 is the unit token, and there is no arg2"
        );
        // The control that proves the trigger is the RAW DWORD: a bit this struct never decodes.
        assert_eq!(
            fired(Some(with(0)), with(0x2), &moved),
            vec!["UNIT_DYNAMIC_FLAGS:target".to_string()],
            "an undecoded bit still fires it — the watch is a memcmp over the dword"
        );
        // The create block runs no notify pass (1098 §4), like both arms beside it.
        assert!(fired(None, with(0x4), &still).is_empty());
        // And the control that stops the rest passing by firing always.
        assert!(fired(Some(with(0x4)), with(0x4), &still).is_empty());
    }

    /// **The two SELF-ONLY arms of the same handler, each on its own bits** — the half that was
    /// over-firing, and the half that was missing.
    ///
    /// `0x5ee990`'s local-GUID gate at `0x5eea93` divides it: `PLAYER_FLAGS_CHANGED` above (any
    /// player), and below it three arms that each carry their own `test` against the XOR-diff.
    /// benilla fired `PLAYER_UPDATE_RESTING` on *any* `PLAYER_FLAGS` delta;
    /// `0x5eead0 f6 45 fc 20 / 74 26` says otherwise — the `je`'s target is `0x5eeafc` and the
    /// fire is at `0x5eeaf2`, inside the arm.
    #[test]
    fn the_self_flag_events_each_fire_on_their_own_bits() {
        use bevy::ecs::system::RunSystemOnce;
        const FIELD_PLAYER_FLAGS: u16 = 190;

        let mut app = App::new();
        app.add_message::<FieldChanged>();
        app.init_resource::<Selection>()
            .init_resource::<UnitFeedState>()
            .init_resource::<NameCache>()
            .init_resource::<Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            // The feed's `MessageSink` is chat + sounds since the PvP/rest lines became message
            // KEYS: the row a key names carries the cue, so the sink reads both.
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_UPDATE_RESTING")
                f:RegisterEvent("PLAYTIME_CHANGED")
                f:RegisterEvent("UPDATE_EXHAUSTION")
                f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(0x77),
                ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&[])
                        .into_created(benilla_protocol::messages::ObjectType::Player),
                ),
            ))
            .id();

        // Set PLAYER_FLAGS, run the feed, read back what fired.
        let step = |app: &mut App, flags: u32| -> Vec<String> {
            app.world_mut()
                .entity_mut(me)
                .get_mut::<ObjectStore>()
                .unwrap()
                .0
                .merge(benilla_protocol::ObjectFields::from_pairs(&[(
                    FIELD_PLAYER_FLAGS,
                    flags,
                )]));
            app.world_mut().run_system_once(feed_units).unwrap();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };

        // Login: the first rest snapshot seeds the memo and says nothing about the flags word.
        // (`UPDATE_EXHAUSTION` is a different watcher — `0x5de4e0`/`0x5de4b0`, not this handler —
        // and its first-sight fire is 1087's settled posture, so it is filtered, not asserted on.)
        let flag_events = |v: Vec<String>| -> Vec<String> {
            v.into_iter().filter(|e| e != "UPDATE_EXHAUSTION").collect()
        };
        assert!(
            flag_events(step(&mut app, 0)).is_empty(),
            "the login descriptor is a create: structurally silent (1098 §4)"
        );

        // **The regression.** `PLAYER_FLAGS_HIDE_HELM 0x400` moves and the resting bit does not —
        // before 2078 this announced a resting change to every listener in the UI.
        assert!(
            flag_events(step(&mut app, 0x400)).is_empty(),
            "a non-resting, non-playtime bit fires neither self event"
        );
        // The resting bit's own edge, both ways: `0x5eead0 test byte [ebp-4],0x20`.
        assert_eq!(
            flag_events(step(&mut app, 0x400 | PLAYER_FLAGS_RESTING)),
            vec!["PLAYER_UPDATE_RESTING".to_string()]
        );
        assert_eq!(
            flag_events(step(&mut app, 0x400)),
            vec!["PLAYER_UPDATE_RESTING".to_string()],
            "the fire is on the XOR-diff, so the clear edge fires it too"
        );

        // `PLAYTIME_CHANGED` — `0x5eeb65 test ah,0x30`, the two play-time regimes together.
        assert_eq!(
            flag_events(step(&mut app, 0x400 | PLAYER_FLAGS_PARTIAL_PLAY_TIME)),
            vec!["PLAYTIME_CHANGED".to_string()]
        );
        assert_eq!(
            flag_events(step(
                &mut app,
                0x400 | PLAYER_FLAGS_PARTIAL_PLAY_TIME | PLAYER_FLAGS_NO_PLAY_TIME
            )),
            vec!["PLAYTIME_CHANGED".to_string()],
            "the second regime bit is the same arm, not a second event"
        );

        // Both arms at once, from one dword: the handler runs them in order and neither masks the
        // other.
        assert_eq!(
            flag_events(step(&mut app, 0x400 | PLAYER_FLAGS_RESTING)),
            vec![
                "PLAYER_UPDATE_RESTING".to_string(),
                "PLAYTIME_CHANGED".to_string()
            ]
        );
    }

    /// **Report B304 — Quiver's range indicator read Dead Zone on a far target.**
    ///
    /// Its Dead Zone rung is `IsActionInRange(AutoShot) ~= 1` **and**
    /// `CheckInteractDistance("target", 4)` truthy (`Quiver.bundle.lua:3136-3150`), i.e. "cannot
    /// shoot, but inside 30 yards". The reach map only ever held *players*, so a creature
    /// `"target"` never entered it and the 30-yard rung answered a **constant** for every mob at
    /// every distance: "in range" at the published tag (the old permissive default), and — after
    /// 1564 flipped that default for B316 — "out of range" for a mob standing next to you. Two
    /// different wrong answers from one defect: the map was never asked about creatures at all.
    ///
    /// So this drives the feed with a real creature target at three distances and reads the rung
    /// itself. The player-only leg it used to apply at the token now rides in the entry, which the
    /// `CanInspect` assertions pin: a boar three yards away is in interact range and is still not
    /// inspectable.
    #[test]
    fn a_creature_target_enters_the_reach_map_at_its_real_distance() {
        use benilla_protocol::messages::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;

        /// `UNIT_FIELD_FLAGS` bit 1 (NON_ATTACKABLE) — one of `can_attack`'s five disqualifiers,
        /// so a unit carrying it is `inspectable` as far as the entry is concerned.
        const NON_ATTACKABLE: u32 = 1 << 1;
        const FIELD_UNIT_FLAGS: u16 = 46;
        const ME: u64 = 0x0000_0000_0000_0001;
        const BOAR: u64 = 0xF130_0000_0000_0002;
        const FRIEND: u64 = 0x0000_0000_0000_0003;

        /// Seat one target `yards` away and answer `expr` against it.
        fn ask(guid: u64, flags: u32, yards: f32, expr: &str) -> bool {
            let mut app = App::new();
            app.init_resource::<crate::ui_party::GroupState>()
                .init_resource::<Reputations>();
            app.insert_non_send_resource(UiScript::new().unwrap());

            let me = app
                .world_mut()
                .spawn((
                    SelfPlayer,
                    Guid(ME),
                    ObjectStore(ObjectFields::default()),
                    Transform::from_xyz(0.0, 0.0, 0.0),
                ))
                .id();
            let target = app
                .world_mut()
                .spawn((
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&[(FIELD_UNIT_FLAGS, flags)])),
                    // Along one axis, so d² is exactly yards² and the thresholds are readable.
                    Transform::from_xyz(yards, 0.0, 0.0),
                ))
                .id();
            app.insert_resource(crate::net::GuidIndex(
                [(ME, me), (guid, target)].into_iter().collect(),
            ));
            app.insert_resource(Selection {
                target: Some(target),
                guid: Some(guid),
            });

            app.world_mut().run_system_once(feed_unit_reach).unwrap();
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<bool>(expr)
                .unwrap()
        }

        // The report itself: a mob well past 30 yards is OUT of the 30-yard row, so Quiver's
        // "inside 30 but cannot shoot" conjunction is false and the bar reads Out of Range.
        assert!(
            ask(
                BOAR,
                0,
                40.0,
                r#"return CheckInteractDistance("target", 4) == nil"#
            ),
            "a creature 40 yards away is out of the 30-yard row"
        );
        // …and the same rung still says YES inside the band, which is the half a token that is
        // simply absent can never do — before this, no mob at any distance could light it.
        assert!(
            ask(
                BOAR,
                0,
                15.0,
                r#"return CheckInteractDistance("target", 4) ~= nil"#
            ),
            "a creature 15 yards away is inside the 30-yard row"
        );
        assert!(
            ask(
                BOAR,
                0,
                15.0,
                r#"return CheckInteractDistance("target", 1) == nil"#
            ),
            "…and outside the 10-yard one: the table is indexed, not a constant"
        );

        // The typemask moved into the entry rather than vanishing: a creature in interact range is
        // still not something `CanInspect` says yes to.
        assert!(
            ask(
                BOAR,
                0,
                3.0,
                r#"return CheckInteractDistance("target", 1) ~= nil"#
            ),
            "a creature 3 yards away is in interact range"
        );
        assert!(
            ask(BOAR, 0, 3.0, r#"return CanInspect("target") == nil"#),
            "…and is still not inspectable — the players-only leg rides in the entry"
        );
        // The control: a non-attackable PLAYER at the same spot is inspectable, so the assertion
        // above is about the guid type and not about the map having gone dark.
        assert!(
            ask(
                FRIEND,
                NON_ATTACKABLE,
                3.0,
                r#"return CanInspect("target") ~= nil"#
            ),
            "a non-attackable player 3 yards away is inspectable"
        );
    }

    /// The combo feed pushes on either half moving but speaks only for the count — the client's
    /// watch is on that byte alone.
    #[test]
    fn only_the_count_fires_the_combo_event() {
        const A: u64 = 0xF130_0000_0000_0001;
        const B: u64 = 0xF130_0000_0000_0002;

        assert_eq!(combo_edge(Some((1, A)), (1, A)), None, "nothing moved");
        assert_eq!(
            combo_edge(Some((1, A)), (2, A)),
            Some(true),
            "a builder lands: push and speak"
        );
        assert_eq!(
            combo_edge(Some((5, A)), (0, 0)),
            Some(true),
            "the clear speaks too — the falling edge is what hides the dots"
        );
        assert_eq!(
            combo_edge(Some((1, A)), (1, B)),
            Some(false),
            "re-banked onto another unit at the same count: pushed, but silent like the client"
        );
        assert_eq!(
            combo_edge(None, (0, 0)),
            Some(true),
            "first sight announces once, as the descriptor block's first write does"
        );
    }

    /// **A feigning hunter's frame reads as a corpse's** — the symptom the whole
    /// change exists for: the wire says nothing but `UNIT_DYNFLAG_DEAD`, and the snapshot has to
    /// turn that into empty bars against a real maximum plus `UnitIsDead`. Asserted through
    /// `snapshot` rather than the field getters (which have their own byte-law test) because what
    /// can regress here is the *wiring* — a future edit reaching for `unit_health` again.
    #[test]
    fn a_feigning_unit_snapshots_empty_bars_over_a_real_maximum() {
        use benilla_protocol::messages::ObjectFields;

        /// `UNIT_FIELD_HEALTH` / `MAXHEALTH` / `POWER1` / `MAXPOWER1` / `DYNAMIC_FLAGS`.
        const HEALTH: u16 = 22;
        const POWER1: u16 = 23;
        const MAXHEALTH: u16 = 28;
        const MAXPOWER1: u16 = 29;
        const DYNFLAGS: u16 = 143;

        let vitals = [
            (HEALTH, 1200),
            (MAXHEALTH, 1500),
            (POWER1, 300),
            (MAXPOWER1, 900),
        ];
        let alive = snapshot(
            &ObjectStore(ObjectFields::from_pairs(&vitals)),
            Some("Hunter".into()),
            0,
            None,
        );
        assert_eq!((alive.health, alive.max_health), (1200, 1500));
        assert_eq!((alive.power, alive.max_power), (300, 900));
        assert!(!alive.dead);

        let feigning = snapshot(
            &ObjectStore(ObjectFields::from_pairs(
                &[vitals.as_slice(), &[(DYNFLAGS, 0x20)]].concat(),
            )),
            Some("Hunter".into()),
            0,
            None,
        );
        assert_eq!(
            (feigning.health, feigning.max_health),
            (0, 1500),
            "UnitHealth 0x5174d0 zeroes, UnitHealthMax 0x5175b0 does not — an EMPTY bar, not a gone one"
        );
        assert_eq!(
            (feigning.power, feigning.max_power),
            (0, 900),
            "UnitMana 0x517670 zeroes, UnitManaMax 0x5177e0 does not"
        );
        assert!(feigning.dead, "UnitIsDead 0x517ac0's dynflag leg");
        assert!(
            !feigning.ghost,
            "feign is not a ghost — PLAYER_FLAGS is clear"
        );

        // …and the flag moves the two fields [`fire_transitions`] diffs, so the edge announces
        // itself as `UNIT_HEALTH` + the power event — the very pair the reference's own dynamic-
        // flags watcher fires there (`0x6004c5`, `0x6004f0`). Nothing else about the unit moved,
        // which is why routing the flag through the getters is enough: no extra watcher needed.
        assert_ne!(alive.health, feigning.health);
        assert_ne!(alive.power, feigning.power);
        assert_eq!(
            (alive.max_health, alive.max_power, alive.level),
            (feigning.max_health, feigning.max_power, feigning.level),
        );
    }

    /// The rank getter's two gates (`0x605620`) as they reach a snapshot. The pet
    /// gate is the interesting half: a charmed or enslaved elite reports rank 0, so it loses its
    /// dragon border AND its ELITE tooltip word AND (at rank 3) its world-boss skull together.
    /// Asserted through `enrich_unit` rather than `gated_rank` directly, because the thing that
    /// can regress is the *wiring* — a future edit reaching for `rec.rank` again.
    #[test]
    fn the_rank_gate_zeroes_a_pet_or_charm() {
        use benilla_protocol::messages::ObjectFields;

        const ENTRY: u32 = 12397; // Ol' Sooty, a rank-1 elite
        const GUID: u64 = (0xF130u64 << 48) | ((ENTRY as u64) << 24) | 0x42;
        /// `UNIT_FIELD_PETNUMBER` — absolute descriptor index (`OBJECT_END(6) + 0x85`).
        const PETNUMBER: u16 = 139;

        let mut names = NameCache::default();
        names.insert_creature(
            ENTRY,
            Some(crate::names::CreatureRecord {
                name: "Ol' Sooty".into(),
                subname: None,
                creature_type: 1,
                pet_family: 4, // Bear — a real tameable family, so the record is a plausible one
                rank: 1,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );

        let rank_of = |fields: &[(u16, u32)]| {
            let store = ObjectStore(ObjectFields::from_pairs(fields));
            let mut s = UnitState::default();
            enrich_unit(&mut s, GUID, &names, &store, None, None);
            s.rank
        };

        assert_eq!(rank_of(&[]), 1, "a free elite keeps its template rank");
        assert_eq!(
            rank_of(&[(PETNUMBER, 0)]),
            1,
            "an explicit zero pet number is not a pet"
        );
        assert_eq!(
            rank_of(&[(PETNUMBER, 7)]),
            0,
            "a non-zero pet number forces rank 0 — no dragon on an enslaved elite"
        );

        // The record gate, the getter's other half: no cached template at all → rank 0, and the
        // border stays plain until the creature query answers.
        let store = ObjectStore(ObjectFields::from_pairs(&[]));
        let mut s = UnitState::default();
        enrich_unit(&mut s, GUID ^ (1 << 24), &names, &store, None, None);
        assert_eq!(s.rank, 0, "an un-queried creature has no classification");
    }

    /// **The faction-name line does not wait for the creature query**.
    ///
    /// Its entry gate `0x612610` reads `[unit+0xb30]` and **returns 1 when there is none** — the
    /// leg a PLAYER takes through a gate whose only field lives in CreatureInfo, and the leg a
    /// creature takes for the round trip before `SMSG_CREATURE_QUERY_RESPONSE` lands. Everything
    /// the line itself needs is on the descriptor (`UNIT_FIELD_FACTIONTEMPLATE`) and already
    /// streamed, so it shows under the pending `UNKNOWNOBJECT` title rather than arriving a round
    /// trip after it. It was gated on the record here, on the premise that a pending creature had
    /// no name line either — the premise decision 2002 corrected.
    ///
    /// The record still owns the line's ONE creature-side gate, `HIDE_FACTION_TOOLTIP` (type flag
    /// `0x10`), which is why the answer landing can take the line away again — the reference's
    /// own sequence, not a flicker of ours.
    #[test]
    fn the_faction_line_does_not_wait_for_the_creature_query() {
        use benilla_protocol::messages::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("Faction.dbc");
        let factions = crate::target::Factions::from_catalog(catalog);

        /// `UNIT_FIELD_FACTIONTEMPLATE` / `UNIT_FIELD_BYTES_0` — absolute descriptor indices.
        const FACTIONTEMPLATE: u16 = 35;
        const BYTES_0: u16 = 36;
        /// The local player the slot walk is matched against: race 1 (human), class 1 (warrior),
        /// packed as `UNIT_FIELD_BYTES_0` bytes 0 and 1.
        const HUMAN_WARRIOR: u32 = 1 | (1 << 8);
        /// A creature entry the cache below is deliberately never told about.
        const ENTRY: u32 = 299;
        const GUID: u64 = (0xF130u64 << 48) | ((ENTRY as u64) << 24) | 0x7;

        let me = ObjectStore(ObjectFields::from_pairs(&[(BYTES_0, HUMAN_WARRIOR)]));
        // The first template id whose faction carries a reputation slot this character can see.
        // Derived from the real DBC rather than guessed, so the test names no id it cannot justify.
        let (template_id, expected) = (1u32..3000)
            .find_map(|id| {
                let f = factions.catalog().template(id)?.faction;
                let info = factions.catalog().reputation_faction(f)?;
                info.tooltip_shows_for(1, 1)
                    .then(|| factions.catalog().faction_name(f))
                    .flatten()
                    .map(|n| (id, n.to_string()))
            })
            .expect("some faction template shows a tooltip line to a human warrior");
        let store = ObjectStore(ObjectFields::from_pairs(&[(FACTIONTEMPLATE, template_id)]));

        let line_for = |names: &NameCache| {
            let mut state = UnitState::default();
            enrich_unit(&mut state, GUID, names, &store, Some(&factions), Some(&me));
            state
        };

        // The query is still in flight: no record, so no name, no subtitle and no rank — and the
        // faction line all the same.
        let pending = line_for(&NameCache::default());
        assert_eq!(pending.name, None, "the name is the thing still in flight");
        assert_eq!(pending.subtitle, None);
        assert_eq!(
            pending.faction_name.as_deref(),
            Some(expected.as_str()),
            "the faction line resolves off the descriptor alone"
        );

        let record = |type_flags: u32| crate::names::CreatureRecord {
            name: "Stormwind Guard".into(),
            subname: None,
            creature_type: 7,
            pet_family: 0,
            rank: 0,
            type_flags,
            civilian: false,
            racial_leader: false,
            display_id: 0,
        };
        let mut answered = NameCache::default();
        answered.insert_creature(ENTRY, Some(record(0)));
        assert_eq!(
            line_for(&answered).faction_name.as_deref(),
            Some(expected.as_str()),
            "the answer landing keeps the line it was already showing"
        );

        // The one creature-side gate the record does own.
        let mut hidden = NameCache::default();
        hidden.insert_creature(ENTRY, Some(record(0x10)));
        assert_eq!(
            line_for(&hidden).faction_name,
            None,
            "HIDE_FACTION_TOOLTIP takes the line away once the record says so"
        );
    }

    /// The PvP-preference announcement law, as the reference's changed-bits handler
    /// runs it: silent on first sight, one pair of KEYS per real edge.
    ///
    /// The assertion is on the identifiers, never the sentences — an English
    /// comparison passes exactly where two keys agree in enUS and diverge everywhere else. The
    /// wording lives in the player's own `GlobalStrings.lua` and is checked against it by
    /// [`the_pvp_and_rest_keys_resolve_in_the_real_global_strings`].
    #[test]
    fn pvp_announcement_speaks_only_on_an_edge() {
        assert_eq!(
            pvp_announcement(None, false),
            None,
            "first sight, unflagged"
        );
        assert_eq!(pvp_announcement(None, true), None, "first sight, flagged");
        assert_eq!(pvp_announcement(Some(true), true), None, "no change");
        assert_eq!(pvp_announcement(Some(false), false), None, "no change");

        assert_eq!(
            pvp_announcement(Some(false), true),
            Some(("ERR_PVP_TOGGLE_ON", "PVP_TOGGLE_ON_VERBOSE"))
        );
        assert_eq!(
            pvp_announcement(Some(true), false),
            Some(("ERR_PVP_TOGGLE_OFF", "PVP_TOGGLE_OFF_VERBOSE"))
        );
    }

    /// The four keys, against the player's REAL `GlobalStrings.lua` — the guard a key-based
    /// assertion needs beside it, since a typo'd key degrades a real line to silence rather than
    /// to a wrong sentence. Also pins the one thing the director would actually notice: the OFF
    /// verbose sentence is the one that explains the five-minute wait, which is the whole reason
    /// the toggle doesn't read as dead. Skips without client data.
    #[test]
    fn the_pvp_and_rest_keys_resolve_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).unwrap_or_default();

        for (was, now) in [(false, true), (true, false)] {
            let (toast, verbose) = pvp_announcement(Some(was), now).expect("an edge speaks");
            assert!(!g(toast).is_empty(), "{toast} missing");
            assert!(!g(verbose).is_empty(), "{verbose} missing");
        }
        assert!(
            g("PVP_TOGGLE_OFF_VERBOSE").contains("five minutes"),
            "the OFF sentence is what tells the player the flag lingers"
        );
        for state in [1u8, 2] {
            let key = rest_state_message(0, state).expect("states 1 and 2 speak");
            assert!(!g(key).is_empty(), "{key} missing");
        }
    }

    /// The rest-state chat law (the watcher `0x5de4e0`): a message needs a real byte
    /// TRANSITION, and only states 1/2 speak — state 0 is the pair table's no-message sentinel,
    /// the beta tiers (≥3) are gated off before the table, and a re-send of the same byte is
    /// swallowed by the dispatcher's mirror diff.
    #[test]
    fn rest_state_message_speaks_only_on_a_real_transition() {
        assert_eq!(rest_state_message(2, 1), Some("ERR_EXHAUSTION_RESTED"));
        assert_eq!(rest_state_message(1, 2), Some("ERR_EXHAUSTION_NORMAL"));
        assert_eq!(
            rest_state_message(0, 1),
            Some("ERR_EXHAUSTION_RESTED"),
            "0→1 IS a transition"
        );
        assert_eq!(
            rest_state_message(1, 1),
            None,
            "same byte re-sent — the mirror diff eats it"
        );
        assert_eq!(rest_state_message(2, 2), None);
        assert_eq!(
            rest_state_message(1, 0),
            None,
            "state 0 is the 0x1d1 sentinel: no message"
        );
        assert_eq!(
            rest_state_message(2, 3),
            None,
            "beta tiers are gated off (cmp esi,3; jae)"
        );
        assert_eq!(rest_state_message(1, 5), None);
    }

    /// The `"npc"` token follows the interaction NPC on the frame it moves — including a frame
    /// on which NOTHING else moves — and is cleared when the window closes. The
    /// legs are the vendor-swap probe's first run, in order: a second vendor opened over an
    /// open window kept the first vendor's snapshot (the dirty gate had no interact input), and
    /// a closed window left `UnitExists("npc")` true (the memo row was never written). The
    /// feed runs in a real `Update` schedule rather than `run_system_once`, because a fresh
    /// system instance sees every resource as changed and would hold the gate open by itself.
    #[test]
    fn the_npc_token_follows_the_interaction_npc_and_clears_with_it() {
        use crate::ui_session::InteractNpc;
        use benilla_protocol::messages::ObjectFields;

        const FIELD_UNIT_LEVEL: u16 = 34;
        // Two `HIGHGUID_UNIT` guids (the high word decides the family — `guid::is_player`).
        const BROG: u64 = 0xF130_0000_9700_0001;
        const DOBBINS: u64 = 0xF130_0001_D100_0002;

        let mut app = App::new();
        app.init_resource::<UnitFeedState>()
            .init_resource::<Selection>()
            .init_resource::<NameCache>()
            .init_resource::<Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            // `feed_units` shows catalog messages through `show_messages`, whose sink is the chat
            // log AND the message-sound queue.
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>()
            .init_resource::<InteractNpc>();
        app.add_message::<FieldChanged>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_units);
        // Two vendors, told apart by level alone — no name cache, no descriptor beyond it.
        let mut vendor = |level: u32| {
            app.world_mut()
                .spawn(ObjectStore(ObjectFields::from_pairs(&[(
                    FIELD_UNIT_LEVEL,
                    level,
                )])))
                .id()
        };
        let brog = vendor(7);
        let dobbins = vendor(9);
        let eval = |app: &mut App, expr: &str| -> i64 {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<i64>(expr)
                .unwrap()
        };
        let exists = |app: &mut App| eval(app, r#"return UnitExists("npc") and 1 or 0"#) == 1;
        let level = |app: &mut App| eval(app, r#"return UnitLevel("npc")"#);

        // Nothing open: no token.
        app.update();
        assert!(!exists(&mut app), "no window open, yet UnitExists(\"npc\")");

        // Brog's window opens.
        *app.world_mut().resource_mut::<InteractNpc>() = InteractNpc(Some(brog), Some(BROG));
        app.update();
        assert_eq!(
            level(&mut app),
            7,
            "the token names the vendor whose window opened"
        );

        // Dobbins' window opens OVER it: the interaction NPC is the only thing that moved this
        // frame — no descriptor, no selection, no group — and the token must still follow.
        *app.world_mut().resource_mut::<InteractNpc>() = InteractNpc(Some(dobbins), Some(DOBBINS));
        app.update();
        assert_eq!(
            level(&mut app),
            9,
            "a second vendor over an open window swaps the token"
        );

        // Closed: absence is data.
        *app.world_mut().resource_mut::<InteractNpc>() = InteractNpc::default();
        app.update();
        assert!(
            !exists(&mut app),
            "the window closed, yet UnitExists(\"npc\")"
        );
    }

    /// **`PLAYER_LEAVING_WORLD` fires on a cross-map worldport, and only on one**.
    ///
    /// Measured live before this existed: an addon counting all three world events across a real
    /// mapId 0 → 1 port read `enter=2 leave=0 login=1`. Two of those already matched the
    /// reference — `PLAYER_ENTERING_WORLD` re-fires because the port destroys and re-creates the
    /// descriptor, and `PLAYER_LOGIN` correctly does not, being armed only by a UI load. The
    /// leaving half was simply never wired.
    ///
    /// The `needs_ack` split is the reference's own: `0x111` is fired from the local player
    /// object's destructor, which a same-map teleport never reaches, and the one worldport that
    /// owes no ack is the initial-login map — an arrival, with nothing behind it to leave.
    #[test]
    fn a_cross_map_worldport_fires_leaving_world_and_the_login_map_does_not() {
        let mut app = App::new();
        app.add_message::<crate::net::WorldportMessage>()
            .init_resource::<crate::ui_script::LeavingWorldArmed>()
            .add_systems(Update, fire_leaving_world_on_worldport);
        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.world_mut()
            .non_send_resource::<UiScript>()
            .run(
                "Left = 0 \
                 local f = CreateFrame(\"Frame\") \
                 f:RegisterEvent(\"PLAYER_LEAVING_WORLD\") \
                 f:SetScript(\"OnEvent\", function() Left = Left + 1 end)",
            )
            .expect("probe frame");
        let left = |app: &mut App| -> i64 {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<i64>("return Left")
                .unwrap()
        };
        let port = |needs_ack: bool| crate::net::WorldportMessage {
            map_id: 1,
            position: [0.0; 3],
            orientation: 0.0,
            needs_ack,
            transport_entry: None,
        };

        let arm = |app: &mut App| {
            app.world_mut()
                .resource_mut::<crate::ui_script::LeavingWorldArmed>()
                .arm();
        };

        // A world began (2239's latch — the reference arms it from the local player's create).
        arm(&mut app);
        app.world_mut().write_message(port(false));
        app.update();
        assert_eq!(left(&mut app), 0, "the initial-login map is an arrival");

        app.world_mut().write_message(port(true));
        app.update();
        assert_eq!(left(&mut app), 1, "a cross-map port leaves a world");

        app.update();
        assert_eq!(
            left(&mut app),
            1,
            "once per port, not once per frame after it"
        );

        // **And once per WORLD, which is the latch's own law** (2239): the port above spent it,
        // and nothing here re-armed — no new avatar was created. A second departure off the same
        // world is the window a quit on the loading screen lands in, and the reference fires
        // nothing there.
        app.world_mut().write_message(port(true));
        app.update();
        assert_eq!(
            left(&mut app),
            1,
            "a second departure with the latch spent fired again — [0xb4b424] is per world"
        );

        // Re-armed, as the new world's create does: the next departure is its own.
        arm(&mut app);
        app.world_mut().write_message(port(true));
        app.update();
        assert_eq!(left(&mut app), 2, "the next world's departure fires again");
    }
}
