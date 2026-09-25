//! The unit snapshot and event feed: each frame, ahead of the VM's tick, live ECS state becomes a
//! [`UnitState`] per token for the engine-free `Unit*` bindings, plus the unit events; the Lua API
//! never reaches into the ECS.

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

/// The unit-feed pass, gated so none of its login one-shots or per-VM memos runs before the in-game
/// interface exists; the demo override orders itself after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UnitFeed;

/// One `UNIT_COMBAT` over a unit, the portrait hit indicator's feed. The emitter `0x494600` fires
/// `(token, action, descriptor, amount, type)` once per token naming the unit, with no
/// self-suppression and no CVar gate; `type` is the school on the melee and spell-damage paths, 0
/// from the miss and heal wrappers, and the 1.12 binary never emits `ENERGIZE`. The melee victim
/// event waits for the impact keyframe (`0x6243e0`, reached only from `0x624530`).
#[derive(Message, Clone, Copy)]
pub(crate) struct UnitCombatFeedback {
    pub(crate) unit: Entity,
    /// `arg2`, the action word (`WOUND`, `MISS`, `DODGE`, `PARRY`, `BLOCK`, `HEAL`, …).
    pub(crate) action: &'static str,
    /// `arg3`, the descriptor: `CRITICAL`/`CRUSHING`/`GLANCING`/`ABSORB`/`BLOCK`/`RESIST`, or `""`.
    pub(crate) flags: &'static str,
    /// `arg4`, the amount; 0 for a word alone.
    pub(crate) amount: u32,
    /// `arg5`, the school (0 physical); stock draws a `type > 0` number spell-yellow.
    pub(crate) school: u32,
}

/// One `COMBAT_TEXT_UPDATE` (event `0x21E`, via `0x703f50`), fired at packet parse by every
/// producer, melee included (`0x6255b0` → `0x629d30`); `data`/`extra` are `arg2`/`arg3`. Fired only
/// for the player as recipient; the reference's emit shares the chat combat log's category scope,
/// so it can fire for other participants, by a rule untraced.
#[derive(Message, Clone)]
pub(crate) struct CombatTextEvent {
    pub(crate) message_type: &'static str,
    pub(crate) data: Option<String>,
    pub(crate) extra: Option<String>,
}

/// `PLAYER_LEAVING_WORLD` on a cross-map worldport. The reference fires event `0x111` at one site
/// (`0x490b48`), gated only by the latch `[0xb4b424]`, from the local player's destructor
/// (`0x5dd543`, this system), the shutdown tail `0x490bd0` (ours is `shutdown_ui_state`) and the
/// local player's DESTROY or OUT_OF_RANGE (`0x5e9b5a`); a same-map teleport destroys nothing.
///
/// Deviation: fires with our descriptor present, so `UnitExists("player")` answers true where the
/// reference's (`0x515970`) misses, because matching it breaks `UnitExists` for no stock reader.
fn fire_leaving_world_on_worldport(
    script: Option<NonSendMut<UiScript>>,
    mut armed: ResMut<crate::ui_script::LeavingWorldArmed>,
    mut ports: MessageReader<crate::net::WorldportMessage>,
) {
    // `needs_ack` false is the initial-login map, an arrival; the whole iterator is read so no
    // message carries over.
    let leaving = ports.read().filter(|w| w.needs_ack).count() > 0;
    if !leaving {
        return;
    }
    // The world latch keeps this producer and the shutdown tail from both claiming one departure;
    // spent even with no VM, as the reference clears it at `0x490a8d`, ahead of the fire.
    if !armed.spend() {
        return;
    }
    let Some(mut script) = script else {
        return;
    };
    script.fire_event("PLAYER_LEAVING_WORLD", Vec::new());
}

/// The feed's change-tracking memory: what we last told the VM, plus one server-side log-once.
#[derive(Resource, Default)]
struct UnitFeedState {
    /// What we last told the VM, dying with it, so a `/reload` re-fires everything as a login does.
    vm: crate::ui_script::VmMemo<UnitFeedMemo>,
    /// Whether the sideless-template warning is logged; outside the VM memo, so `/reload` keeps it.
    warned_sideless: bool,
}

/// The per-VM half of [`UnitFeedState`]: the event-trigger diffs.
#[derive(Default)]
struct UnitFeedMemo {
    /// The lazy caches' landing counters: their per-frame `&mut` misses would trip `is_changed`.
    names_generation: gate::Watch,
    guild_generation: gate::Watch,
    /// Whether `PLAYER_ENTERING_WORLD` has fired for this world entry.
    entered_world: bool,
    /// Per token, the last snapshot pushed.
    last: HashMap<String, UnitState>,
    target_guid: Option<u64>,
    last_xp: Option<(u32, u32)>,
    /// `(rest state, pool, PLAYER_FLAGS)`, pushed as one so no binding reads it half-updated.
    last_rest: Option<(u8, u32, u32)>,
    /// Our last level; the first sighting is the login descriptor, not a ding.
    last_level: Option<u32>,
    /// `(count, banked target)`, diffed as a pair because the server writes them as one.
    last_combo: Option<(u8, u64)>,
    /// Our last in-combat flag; first sight fires only when already in combat.
    in_combat: Option<bool>,
    /// Our last PvP-preference bit; the first descriptor is silent, as the reference diffs bits.
    pvp_desired: Option<bool>,
    /// `(HIDE_HELM, HIDE_CLOAK)`, pushed on the edge only: the Options setter flips the VM's
    /// belief ahead of the server, and a per-frame push would snap it back.
    worn_hidden: Option<(bool, bool)>,
    /// `PLAYER_FIELD_BYTES` byte 2 (`GetActionBarToggles`, `0x4e7660`); no field watch, no event.
    action_bar_toggles: Option<u8>,
}

/// Adds the per-frame unit feed; the `Unit*` bindings live in `benilla-ui`.
pub(crate) struct UiUnitPlugin;

impl Plugin for UiUnitPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            Update,
            UnitFeed
                .in_set(crate::ui_script::UiFeed)
                // The whole set: a login one-shot or a per-VM memo spent on the boot VM is lost.
                .run_if(crate::ui_script::ingame_ui_up),
        )
        .init_resource::<UnitFeedState>()
        // A UI-only harness has no sound or net stack, and a missing resource or message is a
        // system-validation panic; declaring twice is idempotent.
        .init_resource::<crate::sound::MessageSounds>()
        .add_message::<UnitCombatFeedback>()
        .add_message::<CombatTextEvent>()
        .add_message::<crate::net::WorldportMessage>()
        .add_message::<crate::net::FieldChanged>()
        .init_resource::<crate::ui_script::LeavingWorldArmed>()
        .add_systems(
            Update,
            (
                // First, so a worldport's leaving edge precedes the entering edge `feed_units`
                // raises for the same port.
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
        // `load_exhaustion_rows` pushes into the VM, so it runs per VM; the rest are one-shots.
        .add_systems(
            Update,
            load_exhaustion_rows.in_set(crate::ui_script::UiFeed),
        )
        .add_systems(PostStartup, (load_default_languages, load_languages));
    }
}

/// The race to default-chat-language join; absent when the tables do not load, and
/// `GetDefaultLanguage()` then answers the reference's zero-value shape.
#[derive(Resource)]
pub(crate) struct DefaultLanguagesRes(pub(crate) benilla_formats::DefaultLanguages);

/// `Languages.dbc` in row order, the walk behind `GetNumLaguages`/`GetLanguageByIndex`.
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

/// The languages this character knows, in `Languages.dbc` order: one a known spell declares
/// (`Effect_1 == 39`, `0x4b25b0`; spell to language, never the reverse) whose skill line
/// (`0x6de040`) is present in `PLAYER_SKILL_INFO`, whatever its value (`0x5ec720`). Two spells on
/// one language both count here; the reference keeps the later learn, invisible on shipped data.
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
    // With no input moved since this VM's push, a rebuild could only reproduce the memo.
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
        // Not fatal: the binding answers the no-table case.
        Err(e) => warn!("ui_unit: default languages unavailable — {e:#}"),
    }
}

/// Seed a new VM's default language from the roster row, before the avatar exists: the load burst
/// reads it, and so does `ChatFrame.lua:1276` at a `PLAYER_ENTERING_WORLD` that [`feed_units`]
/// fires unordered against [`feed_default_language`].
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

/// Push `GetDefaultLanguage()`'s string on a change of race; `None` is the reference's zero value.
/// Locale column 0 (the client's slot is `[0xc0e080]`): only enUS is populated in the 1.12 data.
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

/// Seed each VM's `Exhaustion.dbc` table; a failed load keeps the shipped-table fallback.
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

/// Melee swing to the `UNIT_COMBAT` action (victim-state table `0x83de28`, `0x4946d0`) and
/// descriptor, HitInfo keyed on the amount's sign: above zero CRITICAL `0x80`, GLANCING `0x4000`,
/// CRUSHING `0x8000`; else ABSORB `0x20`, BLOCK `0x800`, RESIST `0x40`.
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
                ("WOUND", "ABSORB")
            } else if hit_info & 0x800 != 0 {
                ("WOUND", "BLOCK") // a full block the parse did not already make state 5
            } else if hit_info & 0x40 != 0 {
                ("WOUND", "RESIST")
            } else {
                ("MISS", "")
            }
        }
    }
}

/// The melee `UNIT_COMBAT` producer, on the swing's impact keyframe, `text_only` flushes included;
/// the center combat text is not here, as the reference fires it at packet parse.
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
            school: 0, // the reference passes sub-damage 0's school (`0x4946fc`), not carried here
        });
    }
}

/// Drain [`CombatTextEvent`] into `COMBAT_TEXT_UPDATE(messageType, data, extra)`.
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

/// Drain [`UnitCombatFeedback`] into `UNIT_COMBAT` for `"player"` and `"target"` only; the
/// reference fires once per token naming the unit, and stock `PetFrame.lua:13` listens for `"pet"`.
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

/// The race id to `UnitRace`'s display name and `raceFile` token (`"Scourge"`, `"NightElf"`); the
/// display name is also what `$R`/`$r` expand to.
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

/// The class id to `UnitClass`'s display name and uppercase `classFileName`.
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

/// A playable race's fixed side, for where no faction template is at hand, as at world entry,
/// where addons concatenate `UnitFactionGroup("player")` at file scope; [`faction_group`] reads
/// the live template.
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

/// A unit's PvP team digit, `0x5efe00`'s `0` Horde, `1` Alliance, `-1` no side, from the race and
/// never the live template: `ChrRaces.dbc` field 2 to `FactionTemplate.dbc` field 3's group mask,
/// `& 4` Horde, else `& 2` Alliance, so a template-35 GM keeps his rank title. Read by
/// `GetPVPRankInfo` (`0x51a9af`, `0x51a9c8`), `UnitPVPName` (`0x5efe60`) and the scoreboard
/// (`0x4aa200`). A frozen copy of the shipped nine rows; race 9 (Goblin) answers Alliance.
pub(crate) fn race_pvp_team(race: u8) -> i8 {
    match race {
        // Group mask 3, Player|Alliance: `& 4` clear, `& 2` set.
        1 | 3 | 4 | 7 | 9 => 1,
        // Group mask 5, Player|Horde: `& 4` set, tested first.
        2 | 5 | 6 | 8 => 0,
        // No `ChrRaces` row: the engine's bounds-failure `-1`, which names no GlobalString.
        _ => -1,
    }
}

/// Resolve a UnitPopup token to the other player's guid it names: `"target"` when it is a player,
/// `"partyN"` through the roster; `"player"` and anything unresolved answer `None`.
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

/// The one unit-token resolver, the reference's `0x515970`: case-insensitive compares, then the
/// object manager; a caller wanting a type tests the resolved unit. [`Selection`] is a parameter
/// because [`crate::target::SelectCommit`] holds it as `ResMut`. `partypetN`, `raidpetN` and `npc`
/// are recognised but unresolved here, a quiet nil.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UnitTokens<'w, 's> {
    /// `Option`, like the two below: a UI-only harness lacks their plugins; a bare `Res` panics.
    index: Option<Res<'w, crate::net::GuidIndex>>,
    pet: Option<Res<'w, crate::ui_pet::PetBar>>,
    hovered: Option<Res<'w, crate::target::Hovered>>,
    group: Res<'w, crate::ui_party::GroupState>,
    pub(crate) stores: Query<'w, 's, &'static ObjectStore>,
    me: Query<'w, 's, (Entity, &'static Guid), With<SelfPlayer>>,
}

impl UnitTokens<'_, '_> {
    fn me(&self) -> Option<(Entity, u64)> {
        self.me.iter().next().map(|(e, g)| (e, g.0))
    }

    /// The guid lookup (`0x468460`), `pub(crate)` for `TargetLastEnemy`, which starts from a guid.
    pub(crate) fn held(&self, guid: u64) -> Option<(Entity, u64)> {
        Some((*self.index.as_ref()?.0.get(&guid)?, guid))
    }

    /// Resolve `token` case-insensitively (`_strnicmp`); the first matching prefix wins.
    pub(crate) fn resolve(&self, token: &str, selection: &Selection) -> Option<(Entity, u64)> {
        let token = token.to_ascii_lowercase();
        match token.as_str() {
            "player" => self.me(),
            "target" => selection.target.zip(selection.guid),
            // One hop off the target's `UNIT_FIELD_TARGET`, the read `/assist` runs.
            "targettarget" => selection
                .target
                .and_then(|e| self.stores.get(e).ok())
                .and_then(|s| s.0.unit_target())
                .filter(|g| *g != 0)
                .and_then(|g| self.held(g)),
            // The same `Hovered` pick `ui_tooltip` pushes `"mouseover"` from.
            "mouseover" => {
                let h = self.hovered.as_ref()?;
                h.target.zip(h.guid)
            }
            // Off the pet bar's cached guid, as the `"pet"` snapshot reads it.
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

/// Every token [`UnitTokens`] resolves, `"player"` included (the reference answers d² = 0).
fn reach_tokens() -> impl Iterator<Item = &'static str> {
    ["player", "target", "targettarget", "mouseover", "pet"]
        .into_iter()
        .chain(crate::ui_party::PARTY_TOKENS)
        .chain(crate::ui_party::RAID_TOKENS)
}

/// Squared distance as the reference sums it: `f32` widened to `f64`, `(dz² + dx²) + dy²`
/// (`0x48a26f..0x48a27d`, shared by `CanInspect` `0x48a1b0` and `CheckInteractDistance`
/// `0x48ba00`). Bevy's axes change it by at most a last ulp, which matters only on a threshold.
fn dist_sq(q: Vec3, p: Vec3) -> f64 {
    let dx = f64::from(q.x) - f64::from(p.x);
    let dy = f64::from(q.y) - f64::from(p.y);
    let dz = f64::from(q.z) - f64::from(p.z);
    (dz * dz + dx * dx) + dy * dy
}

/// `PLAYER_CONTROL_LOST`/`GAINED` on a change of the flag `SMSG_CLIENT_CONTROL_UPDATE` writes
/// (`0x4958e0`); the boot value, and a fresh VM's memo, is in control (`0x48f626`).
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
        // `HasFullControl`'s flag rides the same edge.
        script.set_player_control(!player.control_lost);
        let event = if player.control_lost {
            "PLAYER_CONTROL_LOST"
        } else {
            "PLAYER_CONTROL_GAINED"
        };
        script.fire_event(event, vec![]);
    }
}

/// `PLAYER_FARSIGHT_FOCUS_CHANGED`: the `PLAYER_FARSIGHT` field callback (`0x5de0d0`) fires it on
/// every change, whether or not the new guid resolves, so this diffs the field, never the pose.
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

/// Feed the unit reach map: per token naming a live unit, its squared distance and whether it
/// passes inspect's two other refusals, a non-player and an attackable one (vmangos
/// `MiscHandler.cpp:945-956`; that the client checks them is inferred, `0x48a1b0` being partly
/// undecoded). Ungated: distances move every frame, and no event keys off the map.
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

/// [`feed_units`]' stores and change tracking, grouped to stay under Bevy's 16-parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UnitStores<'w, 's> {
    all: Query<'w, 's, &'static ObjectStore>,
    /// The dirty gate: whose descriptor moved, items excluded (their writes name no unit token).
    changed: Query<'w, 's, (), (Changed<ObjectStore>, Without<crate::items::ItemObject>)>,
    /// Whose object left the manager, by [`Guid`]: a fading model keeps its store 2 s longer.
    removed: RemovedComponents<'w, 's, Guid>,
    /// This run's per-field edges, which [`fire_transitions`]' watch-bridge arms fire off.
    edges: MessageReader<'w, 's, FieldChanged>,
}

/// Build a unit snapshot from a streamed descriptor, its cached name and its `UnitReaction`
/// (`1..8`, or `0` where none is resolved, as for `"player"`); `classes` feeds the relic column.
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
        // A live descriptor is `0x468460` having succeeded, all of `UnitIsVisible` (`0x516030`).
        has_object: true,
        name,
        // The UI getters: `UNIT_DYNFLAG_DEAD` (feign death) zeroes `UnitHealth` (`0x5174d0`) and
        // `UnitMana` (`0x517670`) but not the maxima, so its edge fires the reference's pair
        // (`0x6004c5`, `0x6004f0`); the power getters divide rage by 10 and happiness by 1000.
        health: store.0.unit_shown_health().unwrap_or(0),
        max_health: store.0.unit_max_health().unwrap_or(0),
        level: store.0.unit_level().unwrap_or(0),
        power_type,
        power: store.0.unit_shown_power(power_type).unwrap_or(0),
        max_power: store.0.unit_shown_max_power(power_type).unwrap_or(0),
        // `UnitIsDead` (`0x517ac0`): health ≤ 0 or the dead-looking flag.
        dead: store.0.unit_reads_dead(),
        // `PLAYER_FLAGS` bit `0x10`; a ghost's health is 1, so `dead` is false for it.
        ghost: store.0.player_is_ghost(),
        // `UnitIsCharmed` (`0x516cf0`): `UNIT_FIELD_CHARMEDBY != 0`.
        charmed: store.0.unit_charmed_by().is_some(),
        // `UNIT_DYNAMIC_FLAGS` bits `0x4`/`0x8`; a unit with no descriptor reads `false`.
        tapped: store.0.unit_tapped(),
        tapped_by_player: store.0.unit_tapped_by_player(),
        // `UnitIsPartyLeader`'s descriptor leg; a creature has no PLAYER block and reads absent.
        group_leader: store.0.player_is_group_leader(),
        // The raw dword `PLAYER_FLAGS_CHANGED` fires on; 0 on a creature, as in the reference.
        player_flags: store.0.player_flags(),
        reaction,
        race: race.map(|(n, _)| n.to_string()),
        race_file: race.map(|(_, f)| f.to_string()),
        class: class.map(|(n, _)| n.to_string()),
        class_file: class.map(|(_, f)| f.to_string()),
        // `UnitHasRelicSlot` (`0x519e50`): TYPEMASK_PLAYER first (`0x519e8d`), then the class
        // byte against `ChrClasses.dbc` field 16; without the player test a class-2 NPC answers 1.
        has_relic_slot: matches!(store.0.object_type(), Some(ObjectType::Player))
            && class_id.is_some_and(|c| classes.is_some_and(|t| t.has_relic_slot(u32::from(c)))),
        // Gender byte 0 male, 1 female, on `UnitSex`'s scale: 2 male, 3 female, 0 unknown (nil).
        sex: match store.0.unit_gender() {
            Some(0) => 2,
            Some(1) => 3,
            _ => 0,
        },
        // `UNIT_FIELD_FLAGS` PvP `0x1000` and Skinnable `0x04000000` (vmangos `UnitDefines.h`).
        pvp: store.0.unit_flags() & 0x1000 != 0,
        skinnable: store.0.unit_flags() & 0x0400_0000 != 0,
        // `UnitPlayerControlled`: bit `0x8`, set by pets and charmed creatures as well as players.
        player_controlled: store.0.unit_flags() & 0x8 != 0,
        flags: store.0.unit_flags(),
        // The whole dword: the reference's watch is a memcmp over it, not a bit test.
        dynamic_flags: store.0.unit_dynamic_flags(),
        owner: store
            .0
            .unit_summoned_by()
            .or_else(|| store.0.unit_charmed_by())
            .or_else(|| store.0.unit_created_by())
            .unwrap_or(0),
        // `UnitAffectingCombat` (`0x517e10`): bit 19, the one combat bit for every token.
        in_combat: store.0.unit_flags() & crate::player::UNIT_FLAG_IN_COMBAT != 0,
        // Free-for-all PvP, `PLAYER_FLAGS` bit 7 (vmangos `Player.h:322`); false on a creature.
        is_pvp_ffa: store.0.player_flags() & 0x80 != 0,
        // The honor rank, `PLAYER_BYTES_3` byte 3 (0..=18), public for every player in view.
        pvp_rank: store.0.player_pvp_rank().unwrap_or(0),
        // The team digit of `PVP_RANK_<rank>_<team>`, from the race ([`race_pvp_team`]).
        pvp_team: store.0.unit_race().map_or(-1, race_pvp_team),
        // `PLAYER_BYTES_3` byte 2, the city-protector title (`PVP_MEDAL<n>`), unset by vmangos.
        pvp_medal: store.0.player_pvp_medal().unwrap_or(0),
        // `is_player` and the creature-record fields come from [`enrich_unit`].
        ..Default::default()
    }
}

/// Fill the guid-keyed tooltip fields: `is_player`, or a creature's record fields (the type word
/// from `CreatureType.dbc`'s enUS names) and its faction-name line.
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
        // No faction line for players: their factions have no reputation slot.
        return;
    }
    let Some(entry) = benilla_protocol::guid::entry(guid) else {
        return;
    };
    // The creature record (`CGUnit+0xb30`); `None` until the query answers, a real state the
    // plate draws under the name `UNKNOWNOBJECT`.
    let rec = names.creature_record(entry);
    if let Some(rec) = rec {
        state.subtitle = rec.subname.clone();
        state.creature_type_name = creature_type_word(rec.creature_type).map(str::to_string);
        // The client's one rank getter, never `rec.rank`: an enslaved elite reads rank 0.
        state.rank = crate::names::gated_rank(Some(rec), Some(store));
        state.civilian = rec.civilian;
        state.racial_leader = rec.racial_leader;
    }
    // The faction-name line, every gate of the tooltip builder: the record's `HIDE_FACTION_TOOLTIP`
    // (`0x10`), a reputation slot, and the race/class slot walk with its hidden flag (`0x4`). Its
    // entry gate `0x612610` passes with no record, so the line shows before the query answers.
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

/// Drain `TogglePVP` into `CMSG_TOGGLE_PVP`; `/pvp` is the reference's only caller.
fn drain_pvp_toggles(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_pvp_toggles() {
        let _ = commands.0.send(crate::net::ClientCommand::TogglePvp);
    }
}

/// Drain the `ShowHelm`/`ShowCloak` flips into `CMSG_TOGGLE_HELM`/`CMSG_TOGGLE_CLOAK`; the VM,
/// which alone knows what the Options row did, has already decided each is needed.
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

/// Drain `SetActionBarToggles` into `CMSG_SET_ACTIONBAR_TOGGLES` (sent at `0x4e771d`), one packet
/// per call: the reference binding neither checks for a change nor coalesces.
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

/// The PvP preference bit `CMSG_TOGGLE_PVP` flips (vmangos `Player.h:324`); not the
/// `UNIT_FIELD_FLAGS` PvP bit `0x1000` the icon draws, which lingers for the server's timer.
const PLAYER_FLAGS_PVP_DESIRED: u32 = 0x200;

/// Inside a rest area (vmangos `Player.h:320`), the bit `IsResting` (`0x516ea0`) tests.
const PLAYER_FLAGS_RESTING: u32 = 0x20;

/// `PLAYER_FLAGS` bits 12 and 13, a realm's two play-time limits, read by `PartialPlayTime`
/// (`0x48eb70`) and `NoPlayTime` (`0x48ebe0`); not the pre-1.6.1 `CAN_SELF_RESURRECT`.
const PLAYER_FLAGS_PARTIAL_PLAY_TIME: u32 = 0x1000;
const PLAYER_FLAGS_NO_PLAY_TIME: u32 = 0x2000;

/// The `(toast, verbose)` GlobalStrings keys for a real change of the PvP-preference bit, silent on
/// first sight (the reference diffs bits). The toasts are catalog rows 437/438 (kind 1, yellow
/// `UI_INFO_MESSAGE`); the verbose keys are no rows, so the handler picks their chat surface.
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

/// The rest-state chat key: `0x5de4e0` messages only on a real byte change (the `rep cmpsb` diff at
/// `0x4655bb`), through the pair table `0x80af50`: 1 rested, 2 normal, 0 the no-message sentinel
/// (`0x1d1`), ≥ 3 gated off (`cmp esi,3; jae`). Rows 346/347 are kind 0, a system chat line with
/// no cue or voice (`type_tag 0x44`).
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

/// `UnitFactionGroup`'s (`0x516630`) first return: the faction template's group mask `& 6`, named
/// by `FactionGroup.dbc`. Only the side bits: player and city-guard templates carry
/// `Player|<side>`, and the Player and Monster rows have no name and no icon art.
pub(crate) fn faction_group(store: &ObjectStore, factions: Option<&Factions>) -> Option<String> {
    let catalog = factions?.catalog();
    let template = catalog.template(store.0.unit_faction_template()?)?;
    // The English `InternalName`: every stock consumer concatenates it into a texture path.
    catalog
        .faction_group_internal_name(template.group_mask & 6)
        .map(str::to_string)
}

/// `UnitFactionGroup`'s second return, the localized name stock shows as text.
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

/// `CreatureType.dbc` id to the enUS word, a creature's level-line class slot.
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
        // The shipped table runs 1..11; the nameplate filter tests 11 (`0x605570`).
        11 => "Totem",
        // Deviation: 10, "Not specified", answers None because the word would print in the
        // tooltip's level line; `UnitCreatureType` then answers nil where the reference names it.
        _ => return None,
    })
}

/// Diff a token's snapshot against the last pushed and fire the per-field `UNIT_*` events. The
/// watch-bridge arms fire off `edges`, as the reference's create runs no notify pass; the rest fire
/// on a token's first snapshot (`prev = None`) too, where the reference's behaviour is untraced.
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
    // `UNIT_FLAGS` (id 40), the per-field watch bridge: `0x51bbb0` registers one watch per named
    // unit field, and on any change of the dword the notifier `0x465570` fires `0x51bd50` →
    // `0x515e50`, once per token naming the unit, `arg1` the token. The stock pet bar reads it.
    if edges.moved(cur.guid, benilla_protocol::field::FIELD_UNIT_FLAGS) {
        script.fire_event("UNIT_FLAGS", vec![tok()]);
    }
    // `PLAYER_FLAGS_CHANGED` (id 407, `0x5eea35` into `0x515e50`): no bit test after the XOR diff
    // (`0x5ee9b8`) and above the local-GUID gate (`0x5eea93`), so any bit of any player fires it,
    // once per token naming it (none, nothing: `0x515e63`). The sole 1.12 consumer is the target
    // frame's leader icon (`TargetFrame.lua:88-95`); 1.12 has no AFK/DND unit-frame badge.
    if edges.moved(cur.guid, benilla_protocol::field::FIELD_PLAYER_FLAGS) {
        script.fire_event("PLAYER_FLAGS_CHANGED", vec![tok()]);
    }
    // `UNIT_DYNAMIC_FLAGS` (id 137), the bridge's third arm: the watch is one dword, so any bit
    // fires it. No stock file registers it; addons do, to repaint the tapped state.
    if edges.moved(cur.guid, benilla_protocol::field::FIELD_UNIT_DYNAMIC_FLAGS) {
        script.fire_event("UNIT_DYNAMIC_FLAGS", vec![tok()]);
    }
    // 1.12 names the power events per resource (`UNIT_MANA`, `UNIT_MAXRAGE`, …;
    // `UnitFrame.lua:190-199`), and `power_token` yields the suffix.
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
    // `UNIT_CLASSIFICATION_CHANGED`, the target frame's border repaint, on a change of the gated
    // rank, which is the classification: the creature query landing, a mob enslaved or released.
    if prev.is_none_or(|p| p.rank != cur.rank) {
        script.fire_event("UNIT_CLASSIFICATION_CHANGED", vec![tok()]);
    }
    // `UNIT_FACTION`, the PvP-icon repaint, on the fields the icon reads and on the tapped bit: the
    // reference's dynamic-flags watcher (`0x600440`) fires event 29 on bit `0x4` (`0x6005a1` →
    // `0x6005b0`) and has no arm for `0x8`.
    if prev.is_none_or(|p| {
        (p.pvp, p.is_pvp_ffa, &p.faction_group, p.tapped)
            != (cur.pvp, cur.is_pvp_ffa, &cur.faction_group, cur.tapped)
    }) {
        script.fire_event("UNIT_FACTION", vec![tok()]);
    }
}

fn feed_units(
    script: Option<NonSendMut<UiScript>>,
    // Absent when the data failed to load: no class has a relic slot, the reference's bounds leg.
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,

    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    selection: Res<Selection>,
    // `Option`, as `factions`, `interact` and `entered_world`: a UI-only harness lacks their
    // plugins, and a bare `Res` is a system-validation panic.
    index: Option<Res<crate::net::GuidIndex>>,
    mut stores: UnitStores,
    mut feed: ResMut<UnitFeedState>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    group: Res<crate::ui_party::GroupState>,
    // Chat and the message-sound queue, for the rest and PvP lines.
    mut sink: crate::ui_action::MessageSink,
    // `ResMut` because the guild-identity cache is lazy: a miss sends `CMSG_GUILD_QUERY`.
    mut guild: ResMut<crate::ui_guild::GuildState>,
    interact: Option<Res<crate::ui_session::InteractNpc>>,
    // Rested billing minutes, sent only in `SMSG_AUTH_RESPONSE`; the reference keeps a global.
    entered_world: Option<MessageReader<crate::net::EnteredWorldMessage>>,
) {
    let Some(mut script) = script else {
        return;
    };
    // Ahead of the gate: a login edge, with no snapshot to diff.
    if let Some(entered) =
        entered_world.and_then(|mut r| r.read().last().map(|m| m.billing_time_rested))
    {
        script.set_billing_time_rested(entered);
    }
    let chr = classes.as_deref().map(|t| &t.0);
    // One reborrow, so the memo and `warned_sideless` borrow disjointly rather than alias.
    let feed = &mut *feed;
    let (memo, vm_reset) = feed.vm.get_reset(&script);
    let edges = FieldEdges::collect(&mut stores.edges);

    // The gate: every input read below, despawns included (invisible to `Changed`).
    let names_moved = memo.names_generation.moved(names.generation());
    let guild_moved = memo.guild_generation.moved(guild.identity_generation());
    let selection_changed = selection.is_changed();
    let stores_changed = !stores.changed.is_empty();
    let stores_removed = !stores.removed.is_empty();
    let group_changed = group.is_changed();
    let reps_changed = reputations.is_changed();
    let factions_changed = factions.as_ref().is_some_and(|r| r.is_changed());
    // The interaction NPC moves when nothing else does, and `MERCHANT_SHOW` reads `"npc"`.
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

    // A missing unit is `None`, which `set_unit` clears; a name miss lands on a later frame.
    let self_pair = self_q.iter().next();
    let player = self_pair.map(|(store, guid)| {
        let name = names
            .resolve_unit(guid.0, Some(store), &commands)
            .map(str::to_string);
        let mut s = snapshot(store, name, 0, chr);
        s.is_player = true;
        // Every token pushed here is streamed, so connected; real link-death rides only the group
        // roster's status byte, which the party feed reads for its own tokens.
        s.is_connected = true;
        s.guid = guid.0;
        s.raid_target = group.raid_target_index(guid.0);
        s.faction_group = faction_group(store, factions.as_deref());
        s.faction_group_localized = faction_group_localized(store, factions.as_deref());
        // `GetGuildInfo`: the public guild fields (191/192) joined against the lazy guild cache.
        s.guild = crate::ui_guild::unit_guild(&store.0, &mut guild, &commands);
        s
    });
    // Log once when our template names no side, almost always GM mode (vmangos forces template
    // 35, group mask 0): every `UnitFactionGroup` surface loses its side, as in the reference.
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
    let target = selection.target.zip(selection.guid).and_then(|(e, guid)| {
        let store = stores.all.get(e).ok()?;
        let name = names
            .resolve_unit(guid, Some(store), &commands)
            .map(str::to_string);
        // `UnitReaction` (1..8) is the selection ring's 0..7 rank plus one.
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
        // `CanAttack` (`0x606980`); `UnitCanAttack` gates the target frame's level colour.
        s.can_attack = crate::target::can_attack(
            Some(store),
            factions.as_deref(),
            &reputations,
            self_pair.map(|(s, _)| s),
        );
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

    // `"targettarget"`: one hop off the target's `UNIT_FIELD_TARGET`, streamed guids only. No guild
    // leg: nothing asks it for one, and a lazy-cache miss sends a query.
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

    // `"player"` is pushed only while its descriptor exists: the roster seat stands in before
    // arrival, and `PLAYER_LOGOUT` handlers still read `UnitName("player")`, as the reference's do.
    // `"target"` pushes its absence too; both diff against the event loop's memo.
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
    // Absence is data here too: the target dropping its target clears the token.
    let tot_dirty = match (&tot, memo.last.get("targettarget")) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if tot_dirty {
        gate.audit("feed_units", "the target-of-target snapshot");
        script.set_unit("targettarget", tot.clone());
    }

    // `"npc"`: the interaction NPC, the reference's `[0xb4e2d0]` that `CGGameUI::SetInteractNPC`
    // (`0x4930d0`) writes, which `InteractNpc` models. No guild leg, as for `"targettarget"`.
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
    // Closing the window must clear the token, so the memo is written here, not only read. No
    // `fire_transitions`: nothing draws `"npc"` as a unit frame.
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

    // The XP pair and `PLAYER_XP_UPDATE`, ahead of the `PLAYER_ENTERING_WORLD` fire so its
    // handlers read real values. The event fires on first sight too, at login and after a
    // `/reload`, where the reference's field watchers stay silent.
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

    // The rest snapshot (rest-state byte, pool, `PLAYER_FLAGS`), ahead of the entering-world fire
    // as the reference's descriptor is. Below `0x5ee990`'s local-GUID gate (`0x5eea93`) each arm
    // tests its own bits: `PLAYER_UPDATE_RESTING` on either edge of `0x20` (`0x5eead0`, fire
    // `0x5eeaf2`), `PLAYTIME_CHANGED` on `0x3000` (`0x5eeb65`, fire `0x5eeb6f`).
    // `UPDATE_EXHAUSTION` is two other watchers, `0x5de4e0` on the byte and `0x5de4b0` on the pool;
    // here it also fires on first sight, where they stay silent on a create and on `/reload`.
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
            // The play-time bits share the dword and the memo; only the events split by bit.
            script.set_play_time(
                rest.2 & PLAYER_FLAGS_PARTIAL_PLAY_TIME != 0,
                rest.2 & PLAYER_FLAGS_NO_PLAY_TIME != 0,
            );
            if prev.map(|p| (p.0, p.1)) != Some((rest.0, rest.1)) {
                script.fire_event("UPDATE_EXHAUSTION", vec![]);
            }
            // Only the byte watcher messages, and only on a real change: `prev` None is the login
            // descriptor, a create the reference never runs through the notify pass.
            if let Some(p) = prev {
                let line = rest_state_message(p.0, rest.0)
                    .and_then(|key| crate::ui_action::keyed_line(&script, key));
                crate::ui_action::show_messages(&mut script, &mut sink, "ui_unit", line);
            }
            // The self-only flag events on their own bits' edges; the login create is silent.
            if let Some(p) = prev {
                let moved = p.2 ^ rest.2;
                if moved & PLAYER_FLAGS_RESTING != 0 {
                    script.fire_event("PLAYER_UPDATE_RESTING", vec![]);
                }
                // `PLAYTIME_CHANGED` (id 530), argless; stock `PlayerFrame.lua:23` registers it.
                if moved & (PLAYER_FLAGS_PARTIAL_PLAY_TIME | PLAYER_FLAGS_NO_PLAY_TIME) != 0 {
                    script.fire_event("PLAYTIME_CHANGED", vec![]);
                }
            }
        }
    }

    // `PLAYER_LEVEL_UP` on any level change off the descriptor, demotions included (vmangos
    // `GiveLevel` sends `SMSG_LEVELUP_INFO` for those too); the reference's trigger is untraced.
    if let Some((store, _)) = self_q.iter().next() {
        if let Some(level) = store.0.unit_level() {
            let prev = memo.last_level.replace(level);
            if prev.is_some_and(|p| level != p) {
                gate.audit("feed_units", "the level edge");
                // All nine, as the reference formats them (`%d` nine times) and
                // `ChatFrame.lua:1283-1320` reads them; missing gains are zeros, since stock's
                // `if ( argN > 0 )` raises on nil.
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

    // `PLAYER_FIELD_BYTES` byte 2, the four extra bars, on the edge: the reference never writes the
    // cell (its one access is the read at `0x4e768c`) or watches it (`0x468070`), and stock reads
    // it once, at `PLAYER_ENTERING_WORLD` (`UIParent.lua:364`), so this goes ahead of that fire. No
    // player and a zero byte share the four-nil branch (`0x4e7684`).
    if let Some((store, _)) = self_q.iter().next() {
        let toggles = store.0.player_action_bar_toggles().unwrap_or(0);
        if memo.action_bar_toggles != Some(toggles) {
            gate.audit("feed_units", "the action-bar toggle byte");
            memo.action_bar_toggles = Some(toggles);
            script.set_action_bar_toggles(toggles);
        }
    }

    // `PLAYER_ENTERING_WORLD` once per world entry, after our descriptor lands, as the reference's.
    // At world exit, re-arm it and forget the player-global memos so the next character seeds them.
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
        // Edge-only pushes: a stale memo would skip a new character whose values match it.
        memo.worn_hidden = None;
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
                // A clear fires nothing; the target frame hears `PLAYER_TARGET_CHANGED` below.
                if memo.last.remove(token).is_some() {
                    gate.audit("feed_units", "a unit-token clear");
                }
            }
        }
    }

    // `PLAYER_TARGET_CHANGED`, argless, when the selection changes. A `/reload`'s fresh memo fires
    // it for a kept selection, where the reference's shutdown clears the target (`0x490bdf`).
    if selection.guid != memo.target_guid {
        gate.audit("feed_units", "the PLAYER_TARGET_CHANGED edge");
        memo.target_guid = selection.guid;
        script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    }

    // `PLAYER_REGEN_DISABLED`/`ENABLED` on our in-combat flag's edge (`UNIT_FLAG_IN_COMBAT`
    // `0x00080000`, vmangos `UnitDefines.h:564`); the reference's own trigger is untraced.
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

    // The reference's local `PLAYER_FLAGS` handler answers a preference change (`0x5eeaff`), not
    // the icon's flag, with a yellow toast and a system chat line.
    if let Some((store, _)) = self_pair {
        let desired = store.0.player_flags() & PLAYER_FLAGS_PVP_DESIRED != 0;
        if let Some((toast, verbose)) = pvp_announcement(memo.pvp_desired, desired) {
            gate.audit("feed_units", "the PvP-desired edge");
            // The verbose sentence is no catalog row, so it goes `unkeyed` to chat:
            // `Shown::keyed`'s unknown-key fallback would show it red.
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

    // The hide bits for `ShowingHelm()`/`ShowingCloak()`, on the edge only: the setter flips the
    // VM's belief on the click, and a stale push before the server answers would undo it.
    if let Some((store, _)) = self_pair {
        let hidden = (store.0.player_hides_helm(), store.0.player_hides_cloak());
        if memo.worn_hidden != Some(hidden) {
            gate.audit("feed_units", "the worn-display pair");
            memo.worn_hidden = Some(hidden);
            script.set_worn_display(!hidden.0, !hidden.1);
        }
    }

    // The combo count and its banked target, pushed as a pair since `GetComboPoints` reads both;
    // only the count fires `PLAYER_COMBO_POINTS` (`0x5ddff0`).
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

/// The combo feed's edge: `None` when nothing moved, else whether `PLAYER_COMBO_POINTS` fires.
/// Event 202 is a one-byte watch on the count (`+0x1029`, registered at `0x5dd9d9`) with no value
/// test, so the drop to zero fires; the target has no watch, so a same-count re-bank is silent.
fn combo_edge(last: Option<(u8, u64)>, now: (u8, u64)) -> Option<bool> {
    (last != Some(now)).then(|| last.map(|(count, _)| count) != Some(now.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The team digit comes off the race byte, whatever template sits beside it (a GM's 35).
    #[test]
    fn the_team_digit_comes_off_the_race_byte_not_the_faction_template() {
        use benilla_protocol::ObjectFields;
        /// `UNIT_FIELD_FACTIONTEMPLATE` and `UNIT_FIELD_BYTES_0` (`[obj+0x110]+0x74`, `+0x78`).
        const FACTIONTEMPLATE: u16 = 35;
        const BYTES_0: u16 = 36;
        /// vmangos's GM template: `FactionTemplate.dbc` group mask 0.
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
        assert_eq!(
            team(&[(BYTES_0, human_warrior), (FACTIONTEMPLATE, GM_TEMPLATE)]),
            1,
            "a GM keeps his race's side"
        );
        assert_eq!(
            team(&[(BYTES_0, scourge_mage), (FACTIONTEMPLATE, GM_TEMPLATE)]),
            0,
            "…on both sides"
        );
        // No race byte is the engine's bounds-failure −1, and a template cannot stand in for it.
        assert_eq!(team(&[]), -1, "no race byte, no team digit");
        assert_eq!(
            team(&[(FACTIONTEMPLATE, 1)]),
            -1,
            "and a template is not one"
        );
    }

    /// [`race_pvp_team`] against the shipped DBCs, walked as `0x5efe00` walks them.
    #[test]
    fn race_pvp_team_matches_the_shipped_tables() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let want = benilla_formats::load_race_pvp_teams(&mut chain).expect("ChrRaces walk");
        // 1.12 ships nine rows; a truncated map would pass by asserting nothing.
        assert_eq!(want.len(), 9, "ChrRaces.dbc row count");
        for (&race, &team) in &want {
            assert_eq!(race_pvp_team(race), team, "race {race}");
        }
        assert!(want.values().any(|&t| t == 0), "some race is Horde");
        assert!(want.values().any(|&t| t == 1), "some race is Alliance");
        for race in [0u8, 10, 255] {
            assert!(!want.contains_key(&race));
            assert_eq!(race_pvp_team(race), -1, "race {race} has no ChrRaces row");
        }
    }

    /// A unit's first snapshot is no transition: the reference's create runs no notify pass.
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
        assert_eq!(
            fired(None, base.clone(), &FieldEdges::default()),
            Vec::<String>::new()
        );
        // The unit's edge, not the token's history: a token acquired as the field moved hears it
        // (the reference fans out at notify time), and another unit's edge does not.
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

    /// LOST going down, GAINED going up (boot is in control); far sight on every change.
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

        // The `0x4` edge fires `UNIT_FACTION`, the reference's event 29 (`0x6005a1`).
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
        // The `0x8` edge fires nothing: the reference's watcher has no arm for it.
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
        // The control against firing always.
        assert!(fired(base.clone(), base.clone()).is_empty());
    }

    /// The control is a bit no `UnitState` field decodes, so the trigger is the raw dword.
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

        // `PLAYER_FLAGS_GROUP_LEADER`, the bit the 1.12 target frame reads.
        assert_eq!(
            fired(Some(with(0)), with(0x1), &moved),
            vec!["PLAYER_FLAGS_CHANGED:target".to_string()],
            "arg1 is the unit token, and there is no arg2 (0x515e50 -> 0x703f50(id, \"%s\", token))"
        );
        // And back down: the reference tests the XOR diff, not the new value.
        assert_eq!(
            fired(Some(with(0x1)), with(0), &moved),
            vec!["PLAYER_FLAGS_CHANGED:target".to_string()]
        );

        // `PLAYER_FLAGS_HIDE_HELM`: no `UnitState` field decodes it, and `0x5eea35` tests no bit.
        assert_eq!(
            fired(Some(with(0)), with(0x400), &moved),
            vec!["PLAYER_FLAGS_CHANGED:target".to_string()],
            "an undecoded bit still fires it — the handler tests no bit at all"
        );

        assert!(
            fired(None, with(0x1), &still).is_empty(),
            "a unit's first snapshot is its create, not a transition"
        );
        // The control against firing always.
        assert!(fired(Some(with(0x1)), with(0x1), &still).is_empty());
    }

    /// Event 137 watches descriptor field 143 over one dword (`repe cmpsb`), so any bit fires it;
    /// the control is `0x2` (`UNIT_DYNFLAG_TRACK_UNIT`), which no `UnitState` field decodes.
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

        // `0x4` TAPPED, the grey-bar state addons register this event for.
        assert_eq!(
            fired(Some(with(0)), with(0x4), &moved),
            vec!["UNIT_DYNAMIC_FLAGS:target".to_string()],
            "arg1 is the unit token, and there is no arg2"
        );
        assert_eq!(
            fired(Some(with(0)), with(0x2), &moved),
            vec!["UNIT_DYNAMIC_FLAGS:target".to_string()],
            "an undecoded bit still fires it — the watch is a memcmp over the dword"
        );
        // The create runs no notify pass.
        assert!(fired(None, with(0x4), &still).is_empty());
        // The control against firing always.
        assert!(fired(Some(with(0x4)), with(0x4), &still).is_empty());
    }

    /// `PLAYER_UPDATE_RESTING` fires at `0x5eeaf2`, inside the `0x20` arm whose `je` (`0x5eead4`)
    /// skips to `0x5eeafc`.
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
            // The feed's `MessageSink` reads chat and sounds: a message key's row carries its cue.
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

        // `UPDATE_EXHAUSTION` is other watchers (`0x5de4e0`/`0x5de4b0`) and fires on first sight
        // here, so it is filtered out.
        let flag_events = |v: Vec<String>| -> Vec<String> {
            v.into_iter().filter(|e| e != "UPDATE_EXHAUSTION").collect()
        };
        assert!(
            flag_events(step(&mut app, 0)).is_empty(),
            "the login descriptor is a create: structurally silent"
        );

        // `PLAYER_FLAGS_HIDE_HELM` `0x400` moves and the resting bit does not.
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

        // `PLAYTIME_CHANGED`: `0x5eeb65 test ah,0x30`, the two play-time regimes together.
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

        // Both arms from one dword, in the handler's order.
        assert_eq!(
            flag_events(step(&mut app, 0x400 | PLAYER_FLAGS_RESTING)),
            vec![
                "PLAYER_UPDATE_RESTING".to_string(),
                "PLAYTIME_CHANGED".to_string()
            ]
        );
    }

    /// The player-only refusal rides in the entry, so a boar in interact range is not inspectable.
    #[test]
    fn a_creature_target_enters_the_reach_map_at_its_real_distance() {
        use benilla_protocol::messages::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;

        /// `UNIT_FIELD_FLAGS` bit 1 (NON_ATTACKABLE), which `can_attack` refuses.
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
                    // Along one axis, so d² is exactly yards².
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

        assert!(
            ask(
                BOAR,
                0,
                40.0,
                r#"return CheckInteractDistance("target", 4) == nil"#
            ),
            "a creature 40 yards away is out of the 30-yard row"
        );
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
        // The control: a non-attackable player there is inspectable, so the refusal is the type.
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

    /// Either half moving pushes; only the count, the one byte the client watches, fires.
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

    /// `UNIT_DYNFLAG_DEAD` alone makes empty bars over real maxima, through `snapshot`'s wiring.
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

        // The flag moves only health and power, the pair the reference's watcher fires
        // (`0x6004c5`, `0x6004f0`).
        assert_ne!(alive.health, feigning.health);
        assert_ne!(alive.power, feigning.power);
        assert_eq!(
            (alive.max_health, alive.max_power, alive.level),
            (feigning.max_health, feigning.max_power, feigning.level),
        );
    }

    /// The rank getter's two gates (`0x605620`), through `enrich_unit`'s wiring.
    #[test]
    fn the_rank_gate_zeroes_a_pet_or_charm() {
        use benilla_protocol::messages::ObjectFields;

        const ENTRY: u32 = 12397; // Ol' Sooty, a rank-1 elite
        const GUID: u64 = (0xF130u64 << 48) | ((ENTRY as u64) << 24) | 0x42;
        /// `UNIT_FIELD_PETNUMBER`, absolute descriptor index (`OBJECT_END(6) + 0x85`).
        const PETNUMBER: u16 = 139;

        let mut names = NameCache::default();
        names.insert_creature(
            ENTRY,
            Some(crate::names::CreatureRecord {
                name: "Ol' Sooty".into(),
                subname: None,
                creature_type: 1,
                pet_family: 4, // Bear, a real tameable family
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

        // The record gate: rank 0 until the creature query answers.
        let store = ObjectStore(ObjectFields::from_pairs(&[]));
        let mut s = UnitState::default();
        enrich_unit(&mut s, GUID ^ (1 << 24), &names, &store, None, None);
        assert_eq!(s.rank, 0, "an un-queried creature has no classification");
    }

    /// The entry gate `0x612610` passes with no record; the record's `HIDE_FACTION_TOOLTIP`
    /// (`0x10`) can then take the line away, as in the reference.
    #[test]
    fn the_faction_line_does_not_wait_for_the_creature_query() {
        use benilla_protocol::messages::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("Faction.dbc");
        let factions = crate::target::Factions::from_catalog(catalog);

        /// `UNIT_FIELD_FACTIONTEMPLATE` and `UNIT_FIELD_BYTES_0`, absolute descriptor indices.
        const FACTIONTEMPLATE: u16 = 35;
        const BYTES_0: u16 = 36;
        /// Race 1 and class 1 in `UNIT_FIELD_BYTES_0` bytes 0 and 1, for the slot walk.
        const HUMAN_WARRIOR: u32 = 1 | (1 << 8);
        /// A creature entry the cache is never told about.
        const ENTRY: u32 = 299;
        const GUID: u64 = (0xF130u64 << 48) | ((ENTRY as u64) << 24) | 0x7;

        let me = ObjectStore(ObjectFields::from_pairs(&[(BYTES_0, HUMAN_WARRIOR)]));
        // The first template whose faction shows this character a line, from the real DBC.
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

        // The query in flight: no record, and the faction line all the same.
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

    /// Asserted on keys: English text cannot tell apart two keys that agree in enUS.
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

    /// A mistyped key silences a line; the OFF verbose sentence explains the five-minute wait.
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

    /// Only states 1 and 2 speak, and only on a real byte change (`0x5de4e0`).
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

    /// A real `Update` schedule: a fresh `run_system_once` sees every resource as changed and
    /// would hold the gate open.
    #[test]
    fn the_npc_token_follows_the_interaction_npc_and_clears_with_it() {
        use crate::ui_session::InteractNpc;
        use benilla_protocol::messages::ObjectFields;

        const FIELD_UNIT_LEVEL: u16 = 34;
        // Two `HIGHGUID_UNIT` guids: the high word decides the family.
        const BROG: u64 = 0xF130_0000_9700_0001;
        const DOBBINS: u64 = 0xF130_0001_D100_0002;

        let mut app = App::new();
        app.init_resource::<UnitFeedState>()
            .init_resource::<Selection>()
            .init_resource::<NameCache>()
            .init_resource::<Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            // `show_messages`' sink is the chat log and the message-sound queue.
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>()
            .init_resource::<InteractNpc>();
        app.add_message::<FieldChanged>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_units);
        // Two vendors, told apart by level alone.
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

        app.update();
        assert!(!exists(&mut app), "no window open, yet UnitExists(\"npc\")");

        *app.world_mut().resource_mut::<InteractNpc>() = InteractNpc(Some(brog), Some(BROG));
        app.update();
        assert_eq!(
            level(&mut app),
            7,
            "the token names the vendor whose window opened"
        );

        // Dobbins' window opens over it: the interaction NPC is the only thing that moved.
        *app.world_mut().resource_mut::<InteractNpc>() = InteractNpc(Some(dobbins), Some(DOBBINS));
        app.update();
        assert_eq!(
            level(&mut app),
            9,
            "a second vendor over an open window swaps the token"
        );

        *app.world_mut().resource_mut::<InteractNpc>() = InteractNpc::default();
        app.update();
        assert!(
            !exists(&mut app),
            "the window closed, yet UnitExists(\"npc\")"
        );
    }

    /// Event `0x111` fires from the local player's destructor, which a same-map teleport never
    /// reaches, and the initial-login map needs no ack.
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

        // A world began: the reference arms the latch from the local player's create.
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

        // Once per world: the port above spent the latch and no new avatar re-armed it, so a
        // second departure (a quit on the loading screen) fires nothing, as in the reference.
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
