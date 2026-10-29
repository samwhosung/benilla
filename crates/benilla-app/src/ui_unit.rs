//! The units the engine-free `Unit*` bindings read: each getter asks the call host for the unit
//! its token names as it runs ([`live`]), built into a [`UnitState`] here, and the unit events;
//! the Lua API never reaches into the ECS.

use bevy::prelude::*;

use benilla_formats::{ChrClasses, ChrRaces, Reaction};
use benilla_protocol::messages::ObjectType;
use benilla_ui::script::{
    parse_unit_token, resolve_unit_in, ScriptValue, UiScript, UnitObject, UnitObjectRead,
    UnitState, UnitTokenGlobals, UnitTokenObjects, UnitTokenParse, WornDisplay, WornDisplaySet,
};

use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::target::{Factions, RelUnit, Relations, Selection};
use crate::ui_script::SweepApp;

pub(crate) mod control_gate;
pub(crate) mod field_events;
mod held;
pub(crate) mod live;
#[cfg(test)]
mod relations;
pub(crate) mod tooltip_facts;
pub(crate) mod views;

pub(crate) use held::unheld_state;

/// The unit-feed pass, gated so none of its login one-shots or per-VM memos runs before the in-game
/// interface exists; the demo override orders itself after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UnitFeed;

/// One `UNIT_COMBAT` over a unit, the portrait hit indicator's feed. The emitter `0x494600` fires
/// `(token, action, descriptor, amount, type)` once per token naming the unit, with no
/// self-suppression and no CVar gate; `type` is the school on the melee and spell-damage paths, 0
/// from the miss and heal wrappers, and the 1.12 binary never emits `ENERGIZE`. Each rides the chat
/// drain's queue ([`crate::ui_chat::ChatLog::push_unit_combat`]), so it reaches Lua at its place
/// among the combat-log lines raised beside it. A melee victim's comes from `0x6243e0`
/// ([`melee_unit_combat`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnitCombatFeedback {
    /// The unit, by the guid `0x494600` is handed.
    pub(crate) guid: u64,
    /// `arg2`, the action word (`WOUND`, `MISS`, `DODGE`, `PARRY`, `BLOCK`, `HEAL`, …).
    pub(crate) action: &'static str,
    /// `arg3`, the descriptor: `CRITICAL`/`CRUSHING`/`GLANCING`/`ABSORB`/`BLOCK`/`RESIST`, or `""`.
    pub(crate) flags: &'static str,
    /// `arg4`, the amount; 0 for a word alone.
    pub(crate) amount: u32,
    /// `arg5`, the school (0 physical); stock draws a `type > 0` number spell-yellow.
    pub(crate) school: u32,
}

/// Adds the unit events' lanes and the unit seats no packet makes; the `Unit*` bindings live in
/// `benilla-ui`.
pub(crate) struct UiUnitPlugin;

impl Plugin for UiUnitPlugin {
    fn build(&self, app: &mut App) {
        field_events::add_notice_lane(app);
        views::add_unit_views_lane(app);
        // At the frame's start, so a worldport's leaving edge precedes everything the next frame's
        // packets raise, the entering edge of the new world among them.
        crate::ui_script::add_worldport_sweep(app);
        app.configure_sets(
            Update,
            UnitFeed
                .in_set(crate::ui_script::UiFeed)
                // The whole set: a login one-shot or a per-VM memo spent on the glue VM is lost.
                .run_if(crate::ui_script::ingame_ui_up),
        )
        // A UI-only harness has no sound or net stack, and a missing resource or message is a
        // system-validation panic; declaring twice is idempotent.
        .init_resource::<crate::sound::MessageSounds>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .add_message::<crate::net::WorldportMessage>()
        .init_world_leave_sweep()
        .add_systems(
            Update,
            (warn_sideless_template, feed_far_sight_bit).in_set(UnitFeed),
        )
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
    (race, class): (u8, u8),
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
        .names()
        .filter(|(id, _)| {
            declared.get(id).is_some_and(|spells| {
                spells.iter().any(|&spell| {
                    skill_lines
                        .and_then(|sl| sl.spell_to_line(spell, race, class))
                        .is_some_and(&has_skill_line)
                })
            })
        })
        .map(|(_, name)| name.to_string())
        .collect()
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

/// Seed a new VM with `Languages.dbc`, the static table `SendChatMessage` resolves its language
/// name against (`0x49f8a0`), in the install's locale column, as `GetDefaultLanguage` reads.
pub(crate) fn seed_language_table(world: &World, script: &mut UiScript) {
    let Some(langs) = world.get_resource::<LanguagesRes>() else {
        return;
    };
    script.set_language_table(
        langs
            .0
            .names()
            .map(|(id, name)| (id, name.to_string()))
            .collect(),
    );
}

/// Seed each VM's `Exhaustion.dbc` table; a failed load keeps the shipped-table fallback.
fn load_exhaustion_rows(
    mut hosted_vm: crate::script_calls::HostedVm,
    assets: Option<Res<benilla_assets::WorldAssets>>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    hosted_vm.run(|script| {
        let (Some(script), Some(assets)) = (script, assets) else {
            return;
        };
        if !seeded.claim(script) {
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
            Err(e) => {
                error!("ui_unit: Exhaustion.dbc failed — shipped-table fallback holds: {e:#}")
            }
        }
    })
}

/// A swing's `UNIT_COMBAT` on its victim, as `FloatSwingResult 0x6243e0` raises it (`0x4946d0`, at
/// `0x6244bf` and `0x624518`), queued in the chat drain where its caller stands: for a displayed
/// swing only (`0x625e40`, which returns at `0x624414` past both the number and the event), on a
/// victim held as a unit (each caller's `0x468460`, mask 8; `victim` is its guid and store). The
/// callers are the impact key (`0x6249bb` → `0x624530`), an unheld attacker's swing at receive
/// (`0x625a6d` → `0x624530`), and the flushes of a superseded (`0x625893`) or stopped (`0x624ea7`)
/// record. The swing's `COMBAT_TEXT_UPDATE` rides its combat-log line instead.
pub(crate) fn melee_unit_combat(
    log: &crate::ui_chat::ChatLog,
    s: &crate::creature_anim::SwingMessage,
    victim: Option<(u64, &crate::net::ObjectStore)>,
) {
    if !s.displayed {
        return;
    }
    let Some((guid, _)) = victim.filter(|(_, store)| crate::ui_chat::combat::is_unit(store)) else {
        return;
    };
    crate::combat_log::feedback::swing(log, guid, s.victim_state, s.hit_info, s.damage, s.school);
}

/// `COMBAT_TEXT_UPDATE(messageType, data, extra)` (event `0x21e`, through the five helpers
/// `0x626890`-`0x6269d0`): `data` and `extra` are `arg2` and `arg3`, nil when absent. Each combat-log
/// formatter raises it right after its line, so the line's drain fires it; the usable refresh
/// raises `SPELL_ACTIVE` (`0x4b3226`).
pub(crate) fn fire_combat_text(
    script: &mut UiScript,
    message_type: &str,
    data: Option<String>,
    extra: Option<String>,
) {
    script.fire_event(
        "COMBAT_TEXT_UPDATE",
        combat_text_args(message_type, data, extra),
    );
}

/// [`fire_combat_text`]'s arguments, for a feed that posts the event after its pushes.
pub(crate) fn combat_text_args(
    message_type: &str,
    data: Option<String>,
    extra: Option<String>,
) -> Vec<ScriptValue> {
    let arg = |v: Option<String>| v.map_or(ScriptValue::Nil, ScriptValue::Str);
    vec![
        ScriptValue::Str(message_type.to_string()),
        arg(data),
        arg(extra),
    ]
}

/// Fire one [`UnitCombatFeedback`] as `UNIT_COMBAT` once for each token naming its unit, in the
/// token walk's order (`0x494600` through `0x515c50`): `"player"`, `"pet"`, `"target"`, a group
/// row, `"npc"`, `"mouseover"`, so stock `PetFrame.lua:50-53` hears its pet's hits. The chat drain
/// calls it at the event's place in its queue.
pub(crate) fn fire_unit_combat(script: &mut UiScript, ev: &UnitCombatFeedback) {
    for token in script.unit_tokens_of(ev.guid) {
        script.fire_event(
            "UNIT_COMBAT",
            vec![
                ScriptValue::Str(token),
                ScriptValue::Str(ev.action.to_string()),
                ScriptValue::Str(ev.flags.to_string()),
                ScriptValue::Int(i64::from(ev.amount)),
                ScriptValue::Int(i64::from(ev.school)),
            ],
        );
    }
}

/// The `ChrRaces.dbc` and `ChrClasses.dbc` rows a unit's race and class are read from, as one
/// parameter: the display name at the install's locale column and the token beside it, the pair
/// `UnitRace` (`0x5182e0`) and `UnitClass` (`0x518430`) push. A table that did not load names
/// nothing.
#[derive(Clone, Copy, Default)]
pub(crate) struct UnitNames<'a> {
    pub(crate) races: Option<&'a ChrRaces>,
    pub(crate) classes: Option<&'a ChrClasses>,
    /// Each race's PvP side off the chain.
    pub(crate) teams: Option<&'a crate::chr_classes::RacePvpTeams>,
}

/// Race and class rows for a test: an enUS install's, and a koKR install's for a test that must
/// tell the table's word from English.
#[cfg(test)]
pub(crate) struct TestNames {
    pub(crate) races: ChrRaces,
    pub(crate) classes: ChrClasses,
    pub(crate) teams: crate::chr_classes::RacePvpTeams,
}

#[cfg(test)]
impl TestNames {
    /// The stock enUS names.
    pub(crate) fn english() -> Self {
        Self {
            races: ChrRaces::from_rows([
                (1, "Human", "Human"),
                (2, "Orc", "Orc"),
                (3, "Dwarf", "Dwarf"),
                (4, "Night Elf", "NightElf"),
                (5, "Undead", "Scourge"),
                (6, "Tauren", "Tauren"),
                (7, "Gnome", "Gnome"),
                (8, "Troll", "Troll"),
            ]),
            classes: ChrClasses::from_rows([
                (1, "Warrior", "WARRIOR"),
                (2, "Paladin", "PALADIN"),
                (3, "Hunter", "HUNTER"),
                (4, "Rogue", "ROGUE"),
                (5, "Priest", "PRIEST"),
                (7, "Shaman", "SHAMAN"),
                (8, "Mage", "MAGE"),
                (9, "Warlock", "WARLOCK"),
                (11, "Druid", "DRUID"),
            ]),
            teams: crate::chr_classes::RacePvpTeams::stock(),
        }
    }

    /// The koKR names, tokens unchanged.
    pub(crate) fn korean() -> Self {
        Self {
            races: ChrRaces::from_rows([
                (1, "인간", "Human"),
                (2, "오크", "Orc"),
                (3, "드워프", "Dwarf"),
                (4, "나이트 엘프", "NightElf"),
                (5, "언데드", "Scourge"),
                (6, "타우렌", "Tauren"),
                (7, "노움", "Gnome"),
                (8, "트롤", "Troll"),
            ]),
            classes: ChrClasses::from_rows([
                (1, "전사", "WARRIOR"),
                (2, "성기사", "PALADIN"),
                (3, "사냥꾼", "HUNTER"),
                (4, "도적", "ROGUE"),
                (5, "사제", "PRIEST"),
                (7, "주술사", "SHAMAN"),
                (8, "마법사", "MAGE"),
                (9, "흑마법사", "WARLOCK"),
                (11, "드루이드", "DRUID"),
            ]),
            teams: crate::chr_classes::RacePvpTeams::stock(),
        }
    }

    pub(crate) fn names(&self) -> UnitNames<'_> {
        UnitNames {
            races: Some(&self.races),
            classes: Some(&self.classes),
            teams: Some(&self.teams),
        }
    }

    /// The same rows as the resources the systems read.
    pub(crate) fn insert(&self, app: &mut App) {
        self.insert_world(app.world_mut());
    }

    pub(crate) fn insert_world(&self, world: &mut World) {
        world.insert_resource(crate::chr_classes::ChrRaceTable(self.races.clone()));
        world.insert_resource(crate::chr_classes::ChrClassTable(self.classes.clone()));
        world.insert_resource(self.teams.clone());
    }
}

/// The tables a `World` holds, for the loads that run with exclusive access.
pub(crate) fn world_unit_names(world: &World) -> UnitNames<'_> {
    UnitNames {
        races: world
            .get_resource::<crate::chr_classes::ChrRaceTable>()
            .map(|t| &t.0),
        classes: world
            .get_resource::<crate::chr_classes::ChrClassTable>()
            .map(|t| &t.0),
        teams: world.get_resource::<crate::chr_classes::RacePvpTeams>(),
    }
}

/// The two tables as a system holds them: [`UnitNames`] once borrowed.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct NameTables<'w> {
    races: Option<Res<'w, crate::chr_classes::ChrRaceTable>>,
    classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
    teams: Option<Res<'w, crate::chr_classes::RacePvpTeams>>,
}

impl NameTables<'_> {
    pub(crate) fn get(&self) -> UnitNames<'_> {
        UnitNames {
            races: self.races.as_deref().map(|t| &t.0),
            classes: self.classes.as_deref().map(|t| &t.0),
            teams: self.teams.as_deref(),
        }
    }
}

impl<'a> UnitNames<'a> {
    /// The race byte's display name and its token (`"Night Elf"`, `"NightElf"`; `$R`/`$r` expand
    /// the name); `None` for a race with no row.
    pub(crate) fn race(&self, race: u8) -> Option<(&'a str, &'a str)> {
        self.races?.names(u32::from(race))
    }

    /// A unit's PvP team digit, `0x5efe00`'s `0` Horde, `1` Alliance, `-1` no side, from the race
    /// and never the live template ([`crate::chr_classes::RacePvpTeams`]), so a template-35 GM
    /// keeps his rank title. Read by `GetPVPRankInfo` (`0x51a9af`, `0x51a9c8`), `UnitPVPName`
    /// (`0x5efe60`) and the scoreboard (`0x4aa200`). With no table no race has a side.
    pub(crate) fn pvp_team(&self, race: u8) -> i8 {
        self.teams.map_or(-1, |t| t.team(race))
    }

    /// The race's side as a `FactionGroup` token, for where no faction template is at hand, as at
    /// world entry, where addons concatenate `UnitFactionGroup("player")` at file scope;
    /// [`faction_group`] reads the live template.
    pub(crate) fn faction_group(&self, race: u8) -> Option<&'static str> {
        self.teams?.faction_group(race)
    }

    /// The class byte's display name and its uppercase token (`"Warrior"`, `"WARRIOR"`); `None`
    /// for a class with no row. A row with a blank column pushes the blank (`0x518451`).
    pub(crate) fn class(&self, class: u8) -> Option<(&'a str, &'a str)> {
        let classes = self.classes?;
        let class = u32::from(class);
        classes.has_row(class).then(|| {
            (
                classes.name(class).unwrap_or_default(),
                classes.file(class).unwrap_or_default(),
            )
        })
    }
}

/// The guid `"pet"` names, off the active player's descriptor `me`: `UNIT_FIELD_CHARM`, else
/// `UNIT_FIELD_SUMMON` (`0x515a5e`-`0x515a6e`), 0 for neither or no player (`0x515a58`). The pet
/// bar's guid is not read: a charm with no bar is the pet, and a bar outliving its unit names none.
pub(crate) fn pet_token_guid(me: Option<&ObjectStore>) -> u64 {
    me.and_then(|s| s.0.unit_pet_guid()).unwrap_or(0)
}

/// The one unit-token resolver, the reference's `0x515970`: case-insensitive compares, then the
/// object manager; a caller wanting a type tests the resolved unit. [`Selection`] is a parameter
/// because [`crate::target::SelectCommit`] holds it as `ResMut`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UnitTokens<'w, 's> {
    /// `Option`, like those below: a UI-only harness lacks their plugins; a bare `Res` panics.
    index: Option<Res<'w, crate::net::GuidIndex>>,
    hovered: Option<Res<'w, crate::target::Hovered>>,
    hovered_go: Option<Res<'w, crate::target::HoveredObject>>,
    interact: Option<Res<'w, crate::ui_session::InteractNpc>>,
    group: Res<'w, crate::ui_party::GroupState>,
    /// The stores as of the drain's place, so a verb a listener calls reads its packet's.
    pub(crate) stores: views::ViewStores<'w, 's>,
    me: Query<'w, 's, (Entity, &'static Guid), With<SelfPlayer>>,
}

impl UnitTokens<'_, '_> {
    /// The guid lookup (`0x468460`), `pub(crate)` for `TargetLastEnemy`, which starts from a guid.
    pub(crate) fn held(&self, guid: u64) -> Option<(Entity, u64)> {
        Some((*self.index.as_ref()?.0.get(&guid)?, guid))
    }

    /// `0x515970` on `token`, through the one derivation the VM's resolver shares
    /// ([`resolve_unit_in`]): the client's globals off the selection, the hover, the interaction
    /// latch and the roster ([`TokenGlobals`]), and the objects as of the drain's place
    /// ([`TokenObjects`]). `None` is nobody.
    pub(crate) fn guid(&self, token: &str, selection: &Selection) -> Option<u64> {
        let UnitTokenParse::Unit { base, hops } = parse_unit_token(token) else {
            return None;
        };
        let globals = TokenGlobals {
            tokens: self,
            selection,
        };
        resolve_unit_in(&globals, &self.objects(), base, hops)
    }

    /// `GetUnitFromToken 0x515940`: the object of the guid `token` names ([`Self::guid`]), held.
    pub(crate) fn resolve(&self, token: &str, selection: &Selection) -> Option<(Entity, u64)> {
        self.held(self.guid(token, selection)?)
    }

    /// What the resolver reads off the objects, as of the drain's place.
    fn objects(&self) -> TokenObjects<'_, '_, '_> {
        TokenObjects {
            index: self.index.as_deref(),
            stores: &self.stores,
            me: self.me.iter().next().map(|(e, g)| (e, g.0)),
        }
    }

    /// The guid in 0-based party slot `row`, the one `partyN` and `partypetN` share.
    fn party_member(&self, row: u32) -> Option<u64> {
        let slot = usize::try_from(row).ok()?;
        self.group.party_slots().nth(slot).map(|m| m.guid)
    }

    /// The guid on 0-based raid row `row`, the one `raidN` and `GetRaidRosterInfo` share.
    fn raid_member(&self, row: u32) -> Option<u64> {
        let index = usize::try_from(row).ok()?.checked_add(1)?;
        crate::ui_party::raid_row_guid(&self.group, index)
    }
}

/// The client's state [`UnitTokens`] resolves through, as the app holds it: the selection, the
/// hover, the interaction latch and the roster.
struct TokenGlobals<'a, 'w, 's> {
    tokens: &'a UnitTokens<'w, 's>,
    selection: &'a Selection,
}

impl UnitTokenGlobals for TokenGlobals<'_, '_, '_> {
    fn target(&self) -> u64 {
        self.selection.guid.unwrap_or(0)
    }
    /// The same pick `ui_tooltip` publishes `"mouseover"` from.
    fn mouseover(&self) -> u64 {
        let t = self.tokens;
        t.hovered
            .as_ref()
            .and_then(|h| h.mouseover(&t.hovered_go.as_deref().copied().unwrap_or_default()))
            .map_or(0, |(_, guid)| guid)
    }
    fn npc(&self) -> u64 {
        self.tokens.interact.as_ref().and_then(|i| i.1).unwrap_or(0)
    }
    fn party(&self, row: u32) -> u64 {
        self.tokens.party_member(row).unwrap_or(0)
    }
    fn raid(&self, row: u32) -> u64 {
        self.tokens.raid_member(row).unwrap_or(0)
    }
    fn party_pet_record(&self, row: u32) -> u64 {
        let group = &self.tokens.group;
        self.tokens
            .party_member(row)
            .and_then(|m| group.party_pet_guid(m, None))
            .unwrap_or(0)
    }
    fn raid_pet_record(&self, row: u32) -> u64 {
        let group = &self.tokens.group;
        self.tokens
            .raid_member(row)
            .and_then(|m| group.raid_pet_guid(m, None))
            .unwrap_or(0)
    }
}

/// What the unit-token resolver reads off the objects, each store as of the drain's place
/// ([`views::ViewStores`]), so a listener resolves as its own packet left them: the one reading of
/// each that the lent host hands the VM's resolver and [`UnitTokens`] reads itself.
pub(crate) struct TokenObjects<'a, 'w, 's> {
    pub(crate) index: Option<&'a crate::net::GuidIndex>,
    pub(crate) stores: &'a views::ViewStores<'w, 's>,
    /// Our player's entity and guid.
    pub(crate) me: Option<(Entity, u64)>,
}

impl TokenObjects<'_, '_, '_> {
    /// The object held under `guid` as of here (`0x468460`), one a later packet destroys
    /// included.
    fn store(&self, guid: u64) -> Option<&ObjectStore> {
        let entity = self.index.and_then(|i| i.0.get(&guid).copied());
        self.stores.at(guid, entity)
    }
}

impl UnitTokenObjects for TokenObjects<'_, '_, '_> {
    fn active_player(&self) -> u64 {
        self.me
            .filter(|&(entity, guid)| self.stores.at(guid, Some(entity)).is_some())
            .map_or(0, |(_, guid)| guid)
    }
    fn pet_of(&self, guid: u64) -> Option<u64> {
        Some(self.store(guid)?.0.unit_pet_guid().unwrap_or(0))
    }
    fn target_of(&self, guid: u64) -> Option<u64> {
        let store = self.store(guid).filter(|s| s.is_unit())?;
        Some(store.0.unit_target().unwrap_or(0))
    }
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

/// Seat the far-sight bit `HasFullControl` refuses on (`0x51a142`). Its event,
/// `PLAYER_FARSIGHT_FOCUS_CHANGED`, is the field watch's (`0x5de0d0`, `crate::field_watch`).
fn feed_far_sight_bit(
    mut hosted_vm: crate::script_calls::HostedVm,
    far_sight: Option<Res<crate::player::FarSight>>,
) {
    hosted_vm.run(|script| {
        if let Some(script) = script {
            script.set_far_sight_on(far_sight.as_deref().is_some_and(|f| f.on()));
        }
    })
}

/// The client tables [`snapshot`] reads beside a descriptor, as one parameter for the feeders that
/// sit at Bevy's 16-parameter limit: `ChrClasses.dbc` for the relic slot, the form table the
/// creature-type resolver's first stage reads and `CreatureType.dbc` for its words. Each is absent
/// without game data.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SnapshotTables<'w> {
    names: NameTables<'w>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    creature_types: Option<Res<'w, crate::target::CreatureTypes>>,
    /// The object manager's index, which the tooltip's owner line finds the owner through.
    index: Option<Res<'w, crate::net::GuidIndex>>,
    /// What the tooltip's studied-creature block reads.
    study: Option<Res<'w, tooltip_facts::StudyTables>>,
    families: Option<Res<'w, crate::ui_pet_stats::PetFamilyTables>>,
}

impl SnapshotTables<'_> {
    /// The race and class tables; without `ChrClasses.dbc` no class has a relic slot, the
    /// reference's bounds leg.
    pub(crate) fn unit_names(&self) -> UnitNames<'_> {
        self.names.get()
    }

    /// `Spell.dbc`, absent without game data.
    pub(crate) fn catalog(&self) -> Option<&benilla_formats::SpellCatalog> {
        self.spells.as_deref().map(|s| &s.catalog)
    }

    /// The studied-creature block's tables.
    pub(crate) fn study(&self) -> tooltip_facts::StudySources<'_> {
        tooltip_facts::StudySources {
            tables: self.study.as_deref(),
            families: self.families.as_deref(),
        }
    }

    /// The entity the object manager holds under `guid`.
    pub(crate) fn entity(&self, guid: u64) -> Option<Entity> {
        self.index.as_deref()?.0.get(&guid).copied()
    }

    /// The creature-type resolver's sources: the name cache, the form table and the type words.
    pub(crate) fn types<'a>(&'a self, names: &'a NameCache) -> CreatureTypeSources<'a> {
        CreatureTypeSources::of_resources(
            names,
            self.spells.as_deref(),
            self.creature_types.as_deref(),
        )
    }
}

/// `UnitReaction(unit, "player")` (`0x5167e0`): the unit's reaction toward the player plus one
/// (`0x51683e`), so `1..=7`, Hated to Revered, and Exalted reads 7 too (`0x606439`). Stock
/// `UnitReactionColor` has seven entries (`TargetFrame.lua:6-14`), one per value. Before the
/// player's object streams it reads neutral.
pub(crate) fn unit_reaction(rel: &Relations, unit: &RelUnit, me: Option<&RelUnit>) -> u8 {
    me.map_or(Reaction::Neutral as u8, |me| rel.reaction(unit, me)) + 1
}

/// The player's reaction toward the unit, `0x6061e0(player, unit)`, on the same scale as
/// [`unit_reaction`]; 0 before the player's object, where the tooltip's creature-type gate shows
/// no type (`0x52a2d0`).
pub(crate) fn player_reaction(rel: &Relations, unit: &RelUnit, me: Option<&RelUnit>) -> u8 {
    me.map_or(0, |me| rel.reaction(me, unit) + 1)
}

/// Build a unit snapshot from a streamed descriptor, its guid, its cached name and its
/// `UnitReaction` (`1..=7`, or `0` where none is resolved, as for `"player"`); `tables` feeds the
/// race and class names and the relic column, and `types` the creature type, which every unit carries, players included.
/// The fields the descriptor alone answers are [`unit_object`]'s, which the getters also read off
/// the object as they run.
pub(crate) fn snapshot(
    store: &ObjectStore,
    guid: u64,
    name: Option<String>,
    reaction: u8,
    tables: UnitNames<'_>,
    types: CreatureTypeSources<'_>,
) -> UnitState {
    let race = unit_race_names(store, &tables);
    let class_id = store.0.unit_class();
    let class = unit_class_names(store, &tables);
    let classes = tables.classes;
    let creature_type = types.of(store);
    let mut s = UnitState {
        exists: true,
        // A live descriptor is `0x468460` having succeeded, all of `UnitIsVisible` (`0x516030`).
        has_object: true,
        // `UnitIsConnected` (`0x517d50`) answers 1 for any unit the object manager holds
        // (`0x517daf`); only an unheld member reads the roster record's online bit.
        is_connected: true,
        // What the token resolver `0x515970` yields, and `UnitIsUnit` (`0x516070`) compares.
        guid,
        name,
        reaction,
        // The one resolver `0x605570` behind `UnitCreatureType` (`0x51a2bc`) and the tooltip's
        // type slot (`0x52a2e5`): the form's type, else the template's, else the race's.
        creature_type_name: types.word(creature_type).map(str::to_string),
        creature_type,
        race: race.map(|(n, _)| n.to_string()),
        race_file: race.map(|(_, f)| f.to_string()),
        class: class.map(|(n, _)| n.to_string()),
        class_file: class.map(|(_, f)| f.to_string()),
        // `UnitHasRelicSlot` (`0x519e50`): TYPEMASK_PLAYER first (`0x519e8d`), then the class
        // byte against `ChrClasses.dbc` field 16; without the player test a class-2 NPC answers 1.
        has_relic_slot: matches!(store.0.object_type(), Some(ObjectType::Player))
            && class_id.is_some_and(|c| classes.is_some_and(|t| t.has_relic_slot(u32::from(c)))),
        // `GetDamageBonusStat` (`0x48b520`): the class byte against `ChrClasses.dbc` field 2,
        // read for the active player only (TYPEMASK_PLAYER, `0x48b538`).
        damage_bonus_stat: matches!(store.0.object_type(), Some(ObjectType::Player))
            .then(|| class_id.and_then(|c| classes.and_then(|t| t.damage_bonus_stat(u32::from(c)))))
            .flatten(),
        // `UnitIsPVPFreeForAll` as the unit alone answers it: a player that answers to nobody is
        // its own controller ([`pvp_free_for_all`]); a held unit's view asks its controller.
        is_pvp_ffa: matches!(store.0.object_type(), Some(ObjectType::Player))
            && store.0.unit_charmed_by().is_none()
            && store.0.unit_created_by().is_none()
            && store.0.unit_flags() & 0x8 != 0
            && store.0.player_flags() & PLAYER_FLAGS_FFA_PVP != 0,
        // The team digit of `PVP_RANK_<rank>_<team>`, from the race ([`UnitNames::pvp_team`]).
        pvp_team: store.0.unit_race().map_or(-1, |r| tables.pvp_team(r)),
        tooltip: benilla_ui::script::UnitTooltipFacts {
            // The builder's raw read (`0x52a89f`), not `UnitHealth`'s feign-death zero. The facts
            // that read beyond the descriptor are [`tooltip_facts`]'.
            health: store.0.unit_health().unwrap_or(0),
            ..Default::default()
        },
        // `is_player` and the creature-record fields come from [`enrich_unit`].
        ..Default::default()
    };
    s.set_object(unit_object(store));
    s
}

/// A unit's race as `UnitRace` names it (`0x518200`): its `UNIT_FIELD_BYTES_0` race byte through
/// `ChrRaces.dbc`, the localised name and the file token; the one reading a snapshot's build and
/// the live read ([`LiveIdentity`]) share.
pub(crate) fn unit_race_names<'a>(
    store: &ObjectStore,
    tables: &UnitNames<'a>,
) -> Option<(&'a str, &'a str)> {
    store.0.unit_race().and_then(|r| tables.race(r))
}

/// A unit's class as `UnitClass` names it (`0x518350`): its class byte through `ChrClasses.dbc`,
/// read as [`unit_race_names`] reads the race.
pub(crate) fn unit_class_names<'a>(
    store: &ObjectStore,
    tables: &UnitNames<'a>,
) -> Option<(&'a str, &'a str)> {
    store.0.unit_class().and_then(|c| tables.class(c))
}

/// What names a held unit as the getters read it now ([`benilla_ui::script::UnitIdentityRead`]):
/// its name off the name caches as `GetUnitName` (`0x609210`) reads them, asking a miss once while
/// it is pending, its race and class bytes through the tables, and its creature family off its
/// cached record.
pub(crate) struct LiveIdentity<'a> {
    pub(crate) guid: u64,
    pub(crate) store: &'a ObjectStore,
    pub(crate) names: Option<&'a NameCache>,
    pub(crate) commands: Option<&'a NetCommands>,
    pub(crate) tables: UnitNames<'a>,
    pub(crate) families: Option<&'a crate::ui_pet_stats::PetFamilyTables>,
}

impl benilla_ui::script::UnitIdentityRead for LiveIdentity<'_> {
    fn name(&self) -> Option<String> {
        let ask = match self.commands {
            Some(commands) => Ask::New(commands),
            None => Ask::Read,
        };
        self.names?
            .resolve_unit(self.guid, Some(self.store), ask)
            .map(str::to_string)
    }
    fn race(&self) -> Option<(String, String)> {
        unit_race_names(self.store, &self.tables).map(|(n, f)| (n.to_string(), f.to_string()))
    }
    fn class(&self) -> Option<(String, String)> {
        unit_class_names(self.store, &self.tables).map(|(n, f)| (n.to_string(), f.to_string()))
    }
    /// `UnitCreatureFamily` (`0x51a310`): the word of the record's family id, read only.
    fn family(&self) -> Option<String> {
        crate::ui_pet_stats::family_for(Some(self.store), self.names?, self.families).0
    }
}

/// What a unit's descriptor alone answers ([`UnitObject`]), read off `store` with no table or
/// cache: the snapshot's fields of it.
pub(crate) fn unit_object(store: &ObjectStore) -> UnitObject {
    UnitObject::read(&StoreObject(store))
}

/// A held unit's descriptor as the getters read it ([`UnitObjectRead`]): each field derived as it
/// is asked for, the one derivation a snapshot's build takes too ([`unit_object`]).
pub(crate) struct StoreObject<'a>(pub(crate) &'a ObjectStore);

impl UnitObjectRead for StoreObject<'_> {
    // The UI getters: `UNIT_DYNFLAG_DEAD` (feign death) zeroes `UnitHealth` (`0x5174d0`) and
    // `UnitMana` (`0x517670`) but not the maxima, so its edge fires the reference's pair
    // (`0x6004c5`, `0x6004f0`); the power getters divide rage by 10 and happiness by 1000.
    fn health(&self) -> u32 {
        self.0 .0.unit_shown_health().unwrap_or(0)
    }
    fn max_health(&self) -> u32 {
        self.0 .0.unit_max_health().unwrap_or(0)
    }
    fn level(&self) -> u32 {
        self.0 .0.unit_level().unwrap_or(0)
    }
    fn power_type(&self) -> u8 {
        self.0 .0.unit_power_type()
    }
    fn power(&self) -> u32 {
        let f = &self.0 .0;
        f.unit_shown_power(f.unit_power_type()).unwrap_or(0)
    }
    fn max_power(&self) -> u32 {
        let f = &self.0 .0;
        f.unit_shown_max_power(f.unit_power_type()).unwrap_or(0)
    }
    /// `UnitIsDead` (`0x517ac0`): health ≤ 0 or the dead-looking flag.
    fn dead(&self) -> bool {
        self.0 .0.unit_reads_dead()
    }
    /// `PLAYER_FLAGS` bit `0x10`; a ghost's health is 1, so `dead` is false for it.
    fn ghost(&self) -> bool {
        self.0 .0.player_is_ghost()
    }
    /// `UnitIsCharmed` (`0x516cf0`): `UNIT_FIELD_CHARMEDBY != 0`.
    fn charmed(&self) -> bool {
        self.0 .0.unit_charmed_by().is_some()
    }
    /// `UNIT_DYNAMIC_FLAGS` bits `0x4`/`0x8`; a unit with no descriptor reads `false`.
    fn tapped(&self) -> bool {
        self.0 .0.unit_tapped()
    }
    fn tapped_by_player(&self) -> bool {
        self.0 .0.unit_tapped_by_player()
    }
    /// `UnitIsPartyLeader`'s descriptor leg; a creature has no PLAYER block and reads absent.
    fn group_leader(&self) -> bool {
        self.0 .0.player_is_group_leader()
    }
    /// The raw dword `PLAYER_FLAGS_CHANGED` fires on; 0 on a creature, as in the reference.
    fn player_flags(&self) -> u32 {
        self.0 .0.player_flags()
    }
    fn flags(&self) -> u32 {
        self.0 .0.unit_flags()
    }
    /// The whole dword: the reference's watch is a memcmp over it, not a bit test.
    fn dynamic_flags(&self) -> u32 {
        self.0 .0.unit_dynamic_flags()
    }
    fn owner(&self) -> u64 {
        let f = &self.0 .0;
        f.unit_summoned_by()
            .or_else(|| f.unit_charmed_by())
            .or_else(|| f.unit_created_by())
            .unwrap_or(0)
    }
    /// `UNIT_FIELD_FLAGS` PvP `0x1000` (vmangos `UnitDefines.h`).
    fn pvp(&self) -> bool {
        self.0 .0.unit_flags() & 0x1000 != 0
    }
    /// `UNIT_FIELD_FLAGS` Skinnable `0x04000000` (vmangos `UnitDefines.h`).
    fn skinnable(&self) -> bool {
        self.0 .0.unit_flags() & 0x0400_0000 != 0
    }
    /// `UnitPlayerControlled`: bit `0x8`, set by pets and charmed creatures as well as players.
    fn player_controlled(&self) -> bool {
        self.0 .0.unit_flags() & 0x8 != 0
    }
    /// `UnitAffectingCombat` (`0x517e10`): bit 19, the one combat bit for every token.
    fn in_combat(&self) -> bool {
        self.0 .0.unit_flags() & crate::player::UNIT_FLAG_IN_COMBAT != 0
    }
    /// The honor rank, `PLAYER_BYTES_3` byte 3 (0..=18), public for every player in view.
    fn pvp_rank(&self) -> u8 {
        self.0 .0.player_pvp_rank().unwrap_or(0)
    }
    /// `PLAYER_BYTES_3` byte 2, the city-protector title (`PVP_MEDAL<n>`), unset by vmangos.
    fn pvp_medal(&self) -> u8 {
        self.0 .0.player_pvp_medal().unwrap_or(0)
    }
    /// The quest-log window, off a player alone: `IsUnitOnQuest` looks the guid up with
    /// TYPEMASK_PLAYER (`0x4dfe97`) before it reads `[obj+0xe68]`.
    fn quest_log(&self) -> [[u32; 3]; 20] {
        if matches!(self.0 .0.object_type(), Some(ObjectType::Player)) {
            self.0 .0.player_quest_log_window()
        } else {
            Default::default()
        }
    }
    /// `PLAYER_GUILDID`, the cache key of `GetGuildInfo` (`0x4c93a9`).
    fn guild_id(&self) -> u32 {
        self.0 .0.player_guild_id()
    }
    /// `PLAYER_BYTES_3` byte 1; a creature has no PLAYER block and reads 0.
    fn drunk(&self) -> u8 {
        self.0 .0.player_drunk_byte().unwrap_or(0)
    }
    /// Gender byte 0 male, 1 female, on `UnitSex`'s scale: 2 male, 3 female, 0 unknown (nil).
    fn sex(&self) -> u8 {
        match self.0 .0.unit_gender() {
            Some(0) => 2,
            Some(1) => 3,
            _ => 0,
        }
    }
    /// What the stat getters read of any unit but the active player (`0x515940`, then the
    /// object's fields).
    fn combat(&self) -> benilla_ui::script::UnitCombatStats {
        crate::ui_char::unit_combat_stats(self.0)
    }
    /// The SUMMONEDBY the stat getters' summon gate compares (`0x519272`).
    fn summoned_by(&self) -> u64 {
        self.0 .0.unit_summoned_by().unwrap_or(0)
    }
}

/// The active player's own fields as the getters read them ([`benilla_ui::script::SelfRead`]),
/// the derivation the reload's seat takes too ([`xp_pair`]).
impl benilla_ui::script::SelfRead for StoreObject<'_> {
    fn money(&self) -> u64 {
        self.0 .0.player_money().map_or(0, u64::from)
    }
    fn xp(&self) -> u32 {
        self.0 .0.player_xp().unwrap_or(0)
    }
    fn next_level_xp(&self) -> u32 {
        self.0 .0.player_next_level_xp().unwrap_or(0)
    }
    fn rest_state(&self) -> u8 {
        self.0 .0.player_rest_state().unwrap_or(0)
    }
    fn rest_pool(&self) -> u32 {
        self.0 .0.player_rest_state_experience().unwrap_or(0)
    }
    fn resting(&self) -> bool {
        self.0 .0.player_flags() & PLAYER_FLAGS_RESTING != 0
    }
    fn partial_play_time(&self) -> bool {
        self.0 .0.player_flags() & PLAYER_FLAGS_PARTIAL_PLAY_TIME != 0
    }
    fn no_play_time(&self) -> bool {
        self.0 .0.player_flags() & PLAYER_FLAGS_NO_PLAY_TIME != 0
    }
    fn shapeshift_form(&self) -> u8 {
        self.0 .0.unit_shapeshift_form()
    }
    fn shows_helm(&self) -> bool {
        !self.0 .0.player_hides_helm()
    }
    fn shows_cloak(&self) -> bool {
        !self.0 .0.player_hides_cloak()
    }
    fn combo_points(&self) -> u8 {
        self.0 .0.player_combo_points().unwrap_or(0)
    }
    fn combo_target(&self) -> u64 {
        self.0 .0.player_combo_target()
    }
    fn action_bar_toggles(&self) -> u8 {
        self.0 .0.player_action_bar_toggles().unwrap_or(0)
    }
    fn on_taxi(&self) -> bool {
        self.0 .0.unit_flags() & crate::player::UNIT_FLAG_TAXI_FLIGHT != 0
    }
}

/// `PLAYER_FLAGS` bit 7, free-for-all PvP (vmangos `Player.h:322`).
const PLAYER_FLAGS_FFA_PVP: u32 = 0x80;

/// `UnitIsPVPFreeForAll` (`0x516540`) on a held unit: `UNIT_FIELD_FLAGS` 0x8 (`0x5165ad`-
/// `0x5165bf`), then the player controlling it (`0x606170` at `0x5165c3`) and that player's
/// `PLAYER_FLAGS` 0x80 (`0x5165cc`-`0x5165d5`), so a pet or a charmed unit answers with its
/// owner's or charmer's flag.
pub(crate) fn pvp_free_for_all(rel: &Relations, unit: &RelUnit) -> bool {
    unit.player_controlled()
        && rel
            .controller(unit)
            .is_some_and(|c| c.player_flags & PLAYER_FLAGS_FFA_PVP != 0)
}

/// Fill the guid-keyed tooltip fields: `is_player`, or a creature's record fields and its
/// faction-name line.
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
    // The record is keyed by the unit's own `OBJECT_FIELD_ENTRY` (`[[unit+8]+0xc]`, `0x604629`);
    // an entry of 0 is no key: the cache refuses it (`0x556aad`-`0x556ab6`), so `0x604646` skips
    // the store.
    let entry = store.0.object_entry().filter(|&e| e != 0);
    // The creature record (`CGUnit+0xb30`); `None` until the query answers, a real state the
    // plate draws under the name `UNKNOWNOBJECT`.
    let rec = entry.and_then(|e| names.creature_record(e));
    if let Some(rec) = rec {
        state.subtitle = rec.subname.clone();
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

/// `TogglePVP()` inside its call ([`benilla_ui::script::PvpCall`]); `/pvp` is the reference's only
/// caller.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PvpSteps<'w> {
    net: Res<'w, NetCommands>,
}

impl PvpSteps<'_> {
    /// `0x48d700`: the empty `CMSG_TOGGLE_PVP` (`0x48d736`).
    pub(crate) fn toggle(&self) {
        let _ = self.net.0.send(crate::net::ClientCommand::TogglePvp);
    }
}

/// `ShowHelm` and `ShowCloak`, inside their calls
/// ([`benilla_ui::script::ScriptCall::WornDisplay`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct WornDisplaySteps<'w, 's> {
    net: Res<'w, NetCommands>,
    me: Query<
        'w,
        's,
        (&'static ObjectStore, &'static mut crate::entities::WornPins),
        With<SelfPlayer>,
    >,
    items: Res<'w, crate::items::Items>,
}

impl WornDisplaySteps<'_, '_> {
    /// The show or hide method on the active player, nothing with none (`0x48de43`): the model's
    /// piece re-dressed, then the toggle the live bit owes.
    pub(crate) fn set(&mut self, set: WornDisplaySet) {
        let Ok((store, mut pins)) = self.me.single_mut() else {
            return;
        };
        let s = &store.0;
        let hidden = match set.piece {
            WornDisplay::Helm => s.player_hides_helm(),
            WornDisplay::Cloak => s.player_hides_cloak(),
        };
        // The piece on the model, on the native display alone (`0x5ee690`, `0x5ee75e`); a show only
        // for a worn item whose record gives a display (`0x5ee66e`, `0x5ee67b`).
        let display = s
            .player_visible_item_entry(crate::entities::WornPins::equipment_slot(set.piece))
            .and_then(|entry| self.items.template(entry, 0, crate::query_cache::Ask::Read))
            .map_or(0, |t| t.display_info_id);
        let native =
            s.unit_displayid().is_some() && s.unit_displayid() == s.unit_native_displayid();
        if native && (display != 0 || !set.show) {
            // A hide while the record is still asked for is re-dressed when it lands (`0x5ed880`).
            let waiting = s
                .player_visible_item_entry(crate::entities::WornPins::equipment_slot(set.piece))
                .is_some_and(|entry| !self.items.template_known(entry));
            pins.pin(set.piece, set.show, waiting);
        }
        // The bodiless toggle only while the bit says the opposite (`0x5ee6c4`, `0x5ee783`).
        if set.show == hidden {
            let _ = self.net.0.send(match set.piece {
                WornDisplay::Helm => crate::net::ClientCommand::ToggleHelm,
                WornDisplay::Cloak => crate::net::ClientCommand::ToggleCloak,
            });
        }
    }
}

/// Inside a rest area (vmangos `Player.h:320`), the bit `IsResting` (`0x516ea0`) tests.
const PLAYER_FLAGS_RESTING: u32 = 0x20;

/// `PLAYER_FLAGS` bits 12 and 13, a realm's two play-time limits, read by `PartialPlayTime`
/// (`0x48eb70`) and `NoPlayTime` (`0x48ebe0`); not the pre-1.6.1 `CAN_SELF_RESURRECT`.
const PLAYER_FLAGS_PARTIAL_PLAY_TIME: u32 = 0x1000;
const PLAYER_FLAGS_NO_PLAY_TIME: u32 = 0x2000;

/// The `(toast, verbose)` GlobalStrings keys for the PvP-preference bit's new state
/// ([`crate::field_watch::FieldNotice::PvpPreference`]). The toasts are catalog rows 437/438
/// (`0x1b5`/`0x1b6`, kind 1, yellow `UI_INFO_MESSAGE`); the verbose keys are no rows, so the handler
/// picks their chat surface.
fn pvp_announcement(on: bool) -> (&'static str, &'static str) {
    if on {
        ("ERR_PVP_TOGGLE_ON", "PVP_TOGGLE_ON_VERBOSE")
    } else {
        ("ERR_PVP_TOGGLE_OFF", "PVP_TOGGLE_OFF_VERBOSE")
    }
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

/// `"player"`'s snapshot off our own descriptor: no reaction (the unit is ours), the raid icon,
/// both faction-group spellings and `guild`. One builder for the seat a `/reload`'s load runs under ([`live_player_seat`]) and for the getters' live read
/// ([`live::UnitFeedLive`]); its name is looked up as `ask` says.
#[allow(clippy::too_many_arguments)] // the snapshot's inputs, as each caller holds them
pub(crate) fn player_snapshot(
    store: &ObjectStore,
    guid: u64,
    tables: &SnapshotTables,
    names: &NameCache,
    ask: Ask,
    group: &crate::ui_party::GroupState,
    factions: Option<&Factions>,
    guild: Option<benilla_ui::script::UnitGuild>,
) -> UnitState {
    let name = names
        .resolve_unit(guid, Some(store), ask)
        .map(str::to_string);
    let mut s = snapshot(
        store,
        guid,
        name,
        0,
        tables.unit_names(),
        tables.types(names),
    );
    s.is_player = true;
    s.raid_target = group.raid_target_index(guid);
    s.faction_group = faction_group(store, factions);
    s.faction_group_localized = faction_group_localized(store, factions);
    s.guild = guild;
    s
}

/// `"player"`, the `arg1` of the player-field watcher `0x5de4b0` (`"%s"` at `0x82e280`, `"player"`
/// at `0x847894`).
pub(crate) fn player_token() -> ScriptValue {
    ScriptValue::Str("player".to_string())
}

/// `UnitXP("player")` and `UnitXPMax("player")` off our own descriptor, zero while unsent.
fn xp_pair(store: &ObjectStore) -> (u32, u32) {
    use benilla_ui::script::SelfRead;
    let own = StoreObject(store);
    (own.xp(), own.next_level_xp())
}

/// The live player a UI load seats before any file runs, the VM's copies its getters read with no
/// host lent: its snapshot and XP pair.
pub(crate) struct LivePlayerSeat {
    pub(crate) state: UnitState,
    pub(crate) xp: (u32, u32),
}

/// [`LivePlayerSeat`] for an exclusive edge, `None` before our own create. A `/reload` loads with
/// the player object still in the world, so `UI_Init`'s file scope, `ADDON_LOADED` and
/// `VARIABLES_LOADED` read it there as `PLAYER_LOGIN` does (`0x490168`).
pub(crate) fn live_player_seat(
    tables: SnapshotTables,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    group: Res<crate::ui_party::GroupState>,
    factions: Option<Res<Factions>>,
    guild: Res<crate::ui_guild::GuildState>,
    world: crate::target::RelationWorld,
) -> Option<LivePlayerSeat> {
    let (store, guid) = self_q.iter().next()?;
    // The unit's create looked its name up with a callback (`0x5dec12`); a miss is asked here.
    let mut state = player_snapshot(
        store,
        guid.0,
        &tables,
        &names,
        Ask::New(&commands),
        &group,
        factions.as_deref(),
        // `GetGuildInfo`: the public guild fields (191/192) joined against the lazy guild cache.
        crate::ui_guild::unit_guild(&store.0, &guild, &commands),
    );
    if let Some(me) = world.me() {
        state.is_pvp_ffa = pvp_free_for_all(&world.get(), &me);
    }
    Some(LivePlayerSeat {
        state,
        xp: xp_pair(store),
    })
}

/// Log once while our template names no side, almost always GM mode (vmangos forces template 35,
/// group mask 0): every `UnitFactionGroup` surface loses its side, as in the reference.
fn warn_sideless_template(
    me: Query<&ObjectStore, With<SelfPlayer>>,
    factions: Option<Res<Factions>>,
    mut warned: Local<bool>,
) {
    let Some(store) = me.iter().next() else {
        return;
    };
    let sideless = faction_group(store, factions.as_deref()).is_none();
    if sideless && !*warned {
        warn!(
            "faction: our own template names no side (usually GM mode — vmangos forces template \
             35, group mask 0). Every UnitFactionGroup-derived surface loses its side while this \
             holds — the PvP flag icon stays hidden however flagged you are. `.gm off` restores \
             it. (The Honor tab's rank title is NOT one of these: its team digit comes from your \
             race, not your template.)"
        );
    }
    *warned = sideless;
}

/// The VM's copies of the four tokens' units, `"player"`, `"target"`, `"targettarget"` and
/// `"npc"`, as the stores stand, which the getters read with no host free to ask and no frame
/// feed pushes in game: for a test that drives the stock unit frames off engine state with no
/// host lent.
#[cfg(test)]
#[allow(clippy::too_many_arguments)] // the snapshots' inputs, as the live reads take them
pub(crate) fn seat_tokens(
    mut script: NonSendMut<UiScript>,
    tables: SnapshotTables,
    selection: Res<Selection>,
    world: crate::target::RelationWorld,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    index: Option<Res<crate::net::GuidIndex>>,
    all: Query<&ObjectStore>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    group: Res<crate::ui_party::GroupState>,
    guild: Res<crate::ui_guild::GuildState>,
    interact: Option<Res<crate::ui_session::InteractNpc>>,
) {
    let held = |guid: u64, entity: Option<Entity>| Some((all.get(entity?).ok()?, guid));
    let object = |guid: u64| {
        let entity = index.as_ref().and_then(|i| i.0.get(&guid).copied());
        held(guid, entity).map(|(s, _)| s)
    };
    let stores = views::TokenStores {
        me: self_q.iter().next().map(|(s, g)| (s, g.0)),
        target: selection
            .target
            .zip(selection.guid)
            .and_then(|(e, g)| held(g, Some(e))),
        npc: interact
            .as_deref()
            .and_then(|i| Some((i.0?, i.1?)))
            .and_then(|(e, g)| held(g, Some(e))),
        by_guid: &object,
    };
    let sources = views::TokenSources {
        tables: &tables,
        names: &names,
        commands: &commands,
        group: &group,
        guild: &guild,
        world: &world,
    };
    let [player, target, tot, npc] = views::token_views(&sources, &stores);
    if player.is_some() {
        script.set_unit("player", player);
    }
    script.set_unit("target", target);
    script.set_unit("targettarget", tot);
    script.set_unit("npc", npc);
}

/// [`seat_tokens`] once, for a test that reads the four tokens with no host lent.
#[cfg(test)]
pub(crate) fn seat_tokens_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(seat_tokens)
        .expect("the token seats run as a one-shot system");
}

/// The relation functions' seat, the four tokens' copies, then the field notices' events, for a
/// test whose reads run with no host lent.
#[cfg(test)]
pub(crate) fn add_token_seats(app: &mut App) {
    app.init_resource::<crate::field_watch::NoticeLane>()
        .add_systems(
            Update,
            (
                relations::seat,
                seat_tokens,
                field_events::fire_field_notices,
            )
                .chain(),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Reputations;
    use std::collections::HashMap;

    /// `0x6243e0` raises `UNIT_COMBAT` for a displayed swing on a victim held as a unit, and
    /// returns past it for a swing `0x625e40` hides (`0x624414`).
    #[test]
    fn a_swing_flashes_only_a_unit_and_only_when_displayed() {
        let log = crate::ui_chat::ChatLog::default();
        let swing = |displayed| crate::creature_anim::SwingMessage {
            attacker: Entity::PLACEHOLDER,
            victim: None,
            hit_info: 0x80,
            victim_state: 1,
            damage: 40,
            school: 0,
            displayed,
            seq: 0,
        };
        let store = |kind| {
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, kind)]))
        };
        let (player, object) = (store(0x19), store(0x21));
        melee_unit_combat(&log, &swing(false), Some((1, &player)));
        melee_unit_combat(&log, &swing(true), Some((1, &object)));
        melee_unit_combat(&log, &swing(true), None);
        assert!(log.pending_unit_combat().is_empty());
        melee_unit_combat(&log, &swing(true), Some((1, &player)));
        assert_eq!(
            log.pending_unit_combat(),
            [UnitCombatFeedback {
                guid: 1,
                action: "WOUND",
                flags: "CRITICAL",
                amount: 40,
                school: 0,
            }]
        );
    }

    /// `"mouseover"` resolves through the pick the tooltip publishes: the hovered unit, until a
    /// nearer GameObject wins it and the resolver rejects the GameObject's guid (`0x515bd9 je`).
    #[test]
    fn the_mouseover_token_resolves_nobody_behind_a_nearer_gameobject() {
        use crate::target::{Hovered, HoveredObject};
        use bevy::ecs::system::RunSystemOnce;
        const WOLF: u64 = 0xF130_0000_4500_0001;

        let mut app = App::new();
        app.init_resource::<crate::ui_party::GroupState>()
            .init_resource::<HoveredObject>();
        // A unit's object (`OBJECT_FIELD_TYPE` unit), which the resolver's lookup tests
        // (`0x515bd2`, typemask 8).
        let wolf = app
            .world_mut()
            .spawn(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x9),
            ])))
            .id();
        app.insert_resource(crate::net::GuidIndex([(WOLF, wolf)].into_iter().collect()));
        app.insert_resource(Hovered {
            target: Some(wolf),
            guid: Some(WOLF),
            distance: 10.0,
            ..Default::default()
        });
        let resolve = |app: &mut App| {
            app.world_mut()
                .run_system_once(|tokens: UnitTokens| {
                    tokens.resolve("mouseover", &Selection::default())
                })
                .unwrap()
        };
        assert_eq!(resolve(&mut app), Some((wolf, WOLF)), "the hovered unit");
        let chest = app.world_mut().spawn_empty().id();
        app.insert_resource(HoveredObject {
            target: Some(chest),
            guid: Some(0xF110_0000_0000_0004),
            distance: 5.0,
        });
        assert_eq!(resolve(&mut app), None, "a nearer GameObject names nobody");
    }

    /// `partypetN` and `raidpetN` through the one resolver (`0x515970`): the pet the member's
    /// descriptor names while it is held, else the roster record's, and a pet the object manager
    /// does not hold names no unit.
    mod group_pet_tokens {
        use super::*;
        use crate::net::GuidIndex;
        use crate::ui_party::{GroupState, GROUPTYPE_RAID};
        use benilla_protocol::messages::{member_status, GroupMemberEntry, PartyMemberStatsInfo};
        use benilla_protocol::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 0x10;
        const MY_PET: u64 = 0xF140_0000_0000_0010;
        /// `UNIT_FIELD_CHARM` and `UNIT_FIELD_SUMMON`, each a two-field guid.
        const CHARM: u16 = 6;
        const SUMMON: u16 = 8;

        /// The `i`-th party member (1-based), `0x1000 + i`, and its pet.
        fn member(i: u64) -> u64 {
            0x1000 + i
        }
        fn pet(i: u64) -> u64 {
            0xF140_0000_0000_0000 + i
        }

        fn guid_field(field: u16, guid: u64) -> [(u16, u32); 2] {
            [(field, guid as u32), (field + 1, (guid >> 32) as u32)]
        }

        fn store(pairs: &[(u16, u32)]) -> ObjectStore {
            ObjectStore(ObjectFields::from_pairs(pairs))
        }

        /// Hold `guid` in the object manager with these fields.
        fn stream(app: &mut App, guid: u64, pairs: &[(u16, u32)]) -> Entity {
            let e = app.world_mut().spawn((Guid(guid), store(pairs))).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            e
        }

        fn entry(guid: u64, flags: u8) -> GroupMemberEntry {
            GroupMemberEntry {
                name: format!("M{guid:x}"),
                guid,
                status: member_status::ONLINE,
                flags,
            }
        }

        /// Us, held, and an empty group.
        fn app() -> (App, Entity) {
            let mut app = App::new();
            app.init_resource::<GuidIndex>()
                .init_resource::<GroupState>();
            let me = app
                .world_mut()
                .spawn((SelfPlayer, Guid(ME), store(&[])))
                .id();
            app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, me);
            (app, me)
        }

        /// A party of `n` with each member's record naming its pet as `online`; `pets` holds the
        /// pets, `members` the members, a member's descriptor naming its pet by `SUMMON`.
        fn party(n: u64, members: bool, pets: bool) -> App {
            let (mut app, _) = app();
            let list = (1..=n).map(|i| entry(member(i), 0)).collect();
            app.world_mut()
                .resource_mut::<GroupState>()
                .apply_list(0, 0, list, ME, None, Some(ME));
            for i in 1..=n {
                let record = PartyMemberStatsInfo {
                    status: Some(member_status::ONLINE),
                    pet_guid: Some(pet(i)),
                    ..Default::default()
                };
                app.world_mut()
                    .resource_mut::<GroupState>()
                    .apply_stats(member(i), true, record);
                if pets {
                    stream(&mut app, pet(i), &[]);
                }
                if members {
                    stream(&mut app, member(i), &guid_field(SUMMON, pet(i)));
                }
            }
            app
        }

        fn resolve(app: &mut App, token: &str) -> Option<(Entity, u64)> {
            let token = token.to_string();
            app.world_mut()
                .run_system_once(move |tokens: UnitTokens| {
                    tokens.resolve(&token, &Selection::default())
                })
                .unwrap()
        }

        fn guid_of(app: &mut App, token: &str) -> Option<u64> {
            resolve(app, token).map(|(_, guid)| guid)
        }

        /// `partypetN` is the Nth party slot's pet, and `partyN` is still the slot's member: the
        /// pet arms sit ahead of `party`, whose prefix they share.
        #[test]
        fn partypet_names_the_pet_of_the_slot_and_party_the_member() {
            let mut app = party(4, true, true);
            for i in 1..=4 {
                assert_eq!(guid_of(&mut app, &format!("partypet{i}")), Some(pet(i)));
                assert_eq!(guid_of(&mut app, &format!("party{i}")), Some(member(i)));
            }
            let entity = resolve(&mut app, "partypet2").unwrap().0;
            assert_eq!(
                app.world().get::<Guid>(entity).map(|g| g.0),
                Some(pet(2)),
                "the entity is the pet's"
            );
        }

        /// The compares fold ASCII case (`_strnicmp`, `0x5159b4`).
        #[test]
        fn the_pet_tokens_fold_case() {
            let mut app = party(1, true, true);
            for token in ["PartyPet1", "PARTYPET1", "partyPET1"] {
                assert_eq!(guid_of(&mut app, token), Some(pet(1)), "{token}");
            }
        }

        /// The number is 1-based and read as digits: `0`, a missing number, a letter, a sign and
        /// a number past the four slots each name no slot. The reference's `partypetN` has no bound
        /// and reads past its table; here it is nobody, as `partyN`.
        #[test]
        fn a_partypet_number_outside_one_to_four_names_nobody() {
            let mut app = party(4, true, true);
            for token in [
                "partypet0",
                "partypet5",
                "partypet",
                "partypetX",
                "partypet+1",
                "partypet1x",
                "partypet99999999999999999999",
            ] {
                assert_eq!(resolve(&mut app, token), None, "{token}");
            }
        }

        /// A held member names `CHARM` before `SUMMON` (`0x4e8204`), and a held member with
        /// neither is petless whatever its record says (`0x4e8218`).
        #[test]
        fn a_held_member_names_its_charm_else_summon_and_never_its_record() {
            let mut app = party(1, false, true);
            let charmed = stream(&mut app, 0xF130_0000_0000_0042, &[]);
            let held = stream(&mut app, member(1), &guid_field(SUMMON, pet(1)));
            assert_eq!(guid_of(&mut app, "partypet1"), Some(pet(1)), "SUMMON");

            let both: Vec<_> = guid_field(CHARM, 0xF130_0000_0000_0042)
                .into_iter()
                .chain(guid_field(SUMMON, pet(1)))
                .collect();
            app.world_mut().entity_mut(held).insert(store(&both));
            assert_eq!(
                resolve(&mut app, "partypet1"),
                Some((charmed, 0xF130_0000_0000_0042)),
                "CHARM first"
            );

            app.world_mut().entity_mut(held).insert(store(&[]));
            assert_eq!(
                resolve(&mut app, "partypet1"),
                None,
                "the record names a held pet, but the member's own descriptor is asked first"
            );
        }

        /// A member the object manager does not hold answers from its record's pet guid, which
        /// `partypetN` reads only while the record reads online (`0x4e8227`).
        #[test]
        fn an_unheld_member_names_its_records_pet_while_online() {
            let mut app = party(1, false, false);
            assert_eq!(resolve(&mut app, "partypet1"), None, "the pet is not held");

            let held = stream(&mut app, pet(1), &[]);
            assert_eq!(resolve(&mut app, "partypet1"), Some((held, pet(1))));

            let offline = PartyMemberStatsInfo {
                status: Some(member_status::OFFLINE),
                ..Default::default()
            };
            app.world_mut()
                .resource_mut::<GroupState>()
                .apply_stats(member(1), false, offline);
            assert_eq!(resolve(&mut app, "partypet1"), None, "an offline record");
        }

        /// A raid of five: the wire's members, the last in another subgroup, then us (row 5).
        fn raid() -> App {
            let (mut app, _) = app();
            let list = vec![
                entry(member(1), 0),
                entry(member(2), 1),
                entry(member(3), 1),
                entry(member(4), 2),
            ];
            app.world_mut().resource_mut::<GroupState>().apply_list(
                GROUPTYPE_RAID,
                0,
                list,
                ME,
                None,
                Some(ME),
            );
            app
        }

        /// `raidpetN` is the pet of `GetRaidRosterInfo` row N, the row `raidN` names.
        #[test]
        fn raidpet_names_the_pet_of_the_row_raid_names() {
            let mut app = raid();
            // Row 2 is `member(2)`, held with a pet; row 5 is us.
            let held_pet = stream(&mut app, pet(2), &[]);
            stream(&mut app, member(2), &guid_field(SUMMON, pet(2)));
            let mine = stream(&mut app, MY_PET, &[]);
            let me = app.world().resource::<GuidIndex>().0[&ME];
            app.world_mut()
                .entity_mut(me)
                .insert(store(&guid_field(SUMMON, MY_PET)));

            assert_eq!(resolve(&mut app, "raidpet2"), Some((held_pet, pet(2))));
            assert_eq!(
                resolve(&mut app, "raidpet5"),
                Some((mine, MY_PET)),
                "our own row"
            );
            assert_eq!(
                guid_of(&mut app, "raid2"),
                Some(member(2)),
                "raidN is unchanged"
            );
            assert_eq!(guid_of(&mut app, "raid5"), Some(ME));
            assert_eq!(resolve(&mut app, "raidpet1"), None, "row 1 has no pet");
            for token in ["raidpet0", "raidpet6", "raidpet", "raidpetX", "raidpet+3"] {
                assert_eq!(resolve(&mut app, token), None, "{token}");
            }
            assert_eq!(guid_of(&mut app, "RaidPet2"), Some(pet(2)), "case folds");
        }

        /// `raidpetN`'s record leg has no online test (`0x4919ae`), where `partypetN`'s has one.
        #[test]
        fn a_raid_pet_reads_the_record_of_an_offline_member_a_party_pet_does_not() {
            let mut app = raid();
            let record = PartyMemberStatsInfo {
                status: Some(member_status::OFFLINE),
                pet_guid: Some(pet(1)),
                ..Default::default()
            };
            app.world_mut()
                .resource_mut::<GroupState>()
                .apply_stats(member(1), true, record);
            let held = stream(&mut app, pet(1), &[]);
            // Row 1 and party slot 1 are the same member.
            assert_eq!(resolve(&mut app, "raidpet1"), Some((held, pet(1))));
            assert_eq!(resolve(&mut app, "partypet1"), None);
        }

        /// A party pet in view has a distance, so the range verbs answer for it: the VM resolves
        /// the token to its guid, and the host measures the unit at the call.
        #[test]
        fn the_range_verbs_measure_a_party_pet() {
            let mut app = party(1, true, false);
            let me = app.world().resource::<GuidIndex>().0[&ME];
            app.world_mut()
                .entity_mut(me)
                .insert(Transform::from_xyz(0.0, 0.0, 0.0));
            // A unit (`OBJECT_FIELD_TYPE` 0x9), as the range verbs' lookup asks (typemask 8).
            let held = stream(&mut app, pet(1), &[(2, 0x9)]);
            // Along one axis, so d² is exactly yards².
            app.world_mut()
                .entity_mut(held)
                .insert(Transform::from_xyz(15.0, 0.0, 0.0));
            app.init_resource::<Reputations>();
            app.insert_resource(Selection::default());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .set_unit_guids(&benilla_ui::script::UnitGuids {
                    player: ME,
                    party: [member(1), 0, 0, 0],
                    party_pets: [pet(1), 0, 0, 0],
                    held: HashMap::from([(ME, 0), (pet(1), 0)]),
                    ..Default::default()
                });
            let answer =
                |app: &mut App, expr: &str| -> bool { live::eval_lent(app.world_mut(), expr) };
            assert!(answer(
                &mut app,
                r#"return CheckInteractDistance("partypet1", 4) ~= nil"#
            ));
            assert!(
                answer(
                    &mut app,
                    r#"return CheckInteractDistance("partypet1", 1) == nil"#
                ),
                "15 yards is outside the 10-yard row"
            );
            assert!(
                answer(
                    &mut app,
                    r#"return CheckInteractDistance("partypet2", 4) == nil"#
                ),
                "no second member"
            );
        }
    }

    /// The `target` chain through the one resolver (`0x5159d3`-`0x515a2c`): a base, then a hop off
    /// each held unit's `UNIT_FIELD_TARGET`, so `party1target`, `pettarget` and `raid3target` name
    /// the unit at the end of the chain and any break in it names nobody.
    mod target_chain_tokens {
        use super::*;
        use crate::net::GuidIndex;
        use crate::target::{Hovered, HoveredObject};
        use crate::ui_party::{GroupState, GROUPTYPE_RAID};
        use benilla_protocol::messages::{member_status, GroupMemberEntry};
        use benilla_protocol::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 0x10;
        /// party1 and raid1, party2 and raid2, party3 and raid3; we are raid4.
        const A: u64 = 0x1001;
        const B: u64 = 0x1002;
        const C: u64 = 0x1003;
        const MOB: u64 = 0xF130_0000_0000_0001;
        /// A unit with no target, the mouseover.
        const LONELY: u64 = 0xF130_0000_0000_0002;
        const MY_PET: u64 = 0xF140_0000_0000_0010;
        const A_PET: u64 = 0xF140_0000_0000_1001;
        /// A game object that still carries a value where a unit keeps its target.
        const CHEST: u64 = 0xF110_0000_0000_0003;
        /// Named by `C`'s target and held by no one.
        const GONE: u64 = 0xF130_0000_0000_0099;

        /// `OBJECT_FIELD_TYPE`, and the masks the object manager's typemask test reads.
        const OBJECT_TYPE: u16 = 2;
        const PLAYER: u32 = 0x19;
        const UNIT: u32 = 0x09;
        const GAME_OBJECT: u32 = 0x21;
        /// `UNIT_FIELD_SUMMON` and `UNIT_FIELD_TARGET`, each a two-field guid.
        const SUMMON: u16 = 8;
        const TARGET: u16 = 16;

        fn fields(kind: u32, summon: u64, target: u64) -> ObjectStore {
            let mut pairs = vec![(OBJECT_TYPE, kind)];
            for (field, guid) in [(SUMMON, summon), (TARGET, target)] {
                pairs.push((field, guid as u32));
                pairs.push((field + 1, (guid >> 32) as u32));
            }
            ObjectStore(ObjectFields::from_pairs(&pairs))
        }

        fn entry(guid: u64) -> GroupMemberEntry {
            GroupMemberEntry {
                name: format!("M{guid:x}"),
                guid,
                status: member_status::ONLINE,
                flags: 0,
            }
        }

        /// The world's targets: we target the mob and so does `A`, the mob targets `B`, `B` targets
        /// `A`, our pet targets the mob and `A`'s targets `B`, `C` targets a unit nobody holds, and
        /// `LONELY` targets nobody. We select the mob and hover `LONELY`. The group is a raid of
        /// us, `A`, `B` and `C`, all in our subgroup, so the three are also party slots 1 to 3.
        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<GuidIndex>()
                .init_resource::<GroupState>()
                .init_resource::<HoveredObject>();
            let spawn = |app: &mut App, guid: u64, store: ObjectStore| {
                let e = app.world_mut().spawn((Guid(guid), store)).id();
                app.world_mut()
                    .resource_mut::<GuidIndex>()
                    .0
                    .insert(guid, e);
                e
            };
            let me = spawn(&mut app, ME, fields(PLAYER, MY_PET, MOB));
            app.world_mut().entity_mut(me).insert(SelfPlayer);
            spawn(&mut app, A, fields(PLAYER, A_PET, MOB));
            spawn(&mut app, B, fields(PLAYER, 0, A));
            spawn(&mut app, C, fields(PLAYER, 0, GONE));
            let mob = spawn(&mut app, MOB, fields(UNIT, 0, B));
            let lonely = spawn(&mut app, LONELY, fields(UNIT, 0, 0));
            spawn(&mut app, MY_PET, fields(UNIT, 0, MOB));
            spawn(&mut app, A_PET, fields(UNIT, 0, B));
            app.world_mut().resource_mut::<GroupState>().apply_list(
                GROUPTYPE_RAID,
                0,
                vec![entry(A), entry(B), entry(C)],
                ME,
                None,
                Some(ME),
            );
            app.insert_resource(Selection {
                target: Some(mob),
                guid: Some(MOB),
                ..Default::default()
            });
            app.insert_resource(Hovered {
                target: Some(lonely),
                guid: Some(LONELY),
                distance: 5.0,
                ..Default::default()
            });
            app
        }

        fn resolve(app: &mut App, token: &str) -> Option<(Entity, u64)> {
            let token = token.to_string();
            app.world_mut()
                .run_system_once(move |tokens: UnitTokens, selection: Res<Selection>| {
                    tokens.resolve(&token, &selection)
                })
                .unwrap()
        }

        fn guid_of(app: &mut App, token: &str) -> Option<u64> {
            resolve(app, token).map(|(_, guid)| guid)
        }

        /// Every base takes hops, and each hop reads the current unit's own target, so the answer
        /// is the unit the chain ends on, its entity the one the object manager holds.
        #[test]
        fn a_target_suffix_names_the_unit_at_the_end_of_the_chain() {
            let mut app = app();
            for (token, want) in [
                ("party1target", MOB),
                ("party2target", A),
                ("party1targettarget", B),
                ("party1targettargettarget", A),
                ("playertarget", MOB),
                ("playertargettarget", B),
                ("partypet1target", B),
                ("pettarget", MOB),
                ("pettargettarget", B),
                ("target", MOB),
                ("targettarget", B),
                ("targettargettarget", A),
                ("raid4target", MOB),
                ("raid1target", MOB),
                ("raid2target", A),
                ("raid1targettarget", B),
                ("raidpet4target", MOB),
                ("raidpet1target", B),
                ("mouseover", LONELY),
            ] {
                assert_eq!(guid_of(&mut app, token), Some(want), "{token}");
            }
            let (entity, guid) = resolve(&mut app, "party2targettarget").unwrap();
            assert_eq!(guid, MOB);
            assert_eq!(
                app.world().get::<Guid>(entity).map(|g| g.0),
                Some(MOB),
                "the entity is the mob's"
            );
        }

        /// A unit with no target ends the chain (`0x515a2e`), as does a target the object manager
        /// does not hold (`0x515a16`), a hop off anything but a unit (`0x515a0f`, typemask 8) and
        /// a base that names no one.
        #[test]
        fn a_chain_that_breaks_names_nobody() {
            let mut app = app();
            for token in [
                // The mouseover targets nobody, and so nothing follows it.
                "mouseovertarget",
                "mouseovertargettarget",
                // `C` targets a guid nobody holds.
                "party3target",
                "raid3target",
                // Nor does a further hop go past the break.
                "party3targettarget",
                // A base no one holds: party4 is nobody, raid5 is past the roster, a missing or
                // zero number names no row, and `B` has no pet.
                "party4target",
                "raid5target",
                "raid0target",
                "partytarget",
                "partypet2target",
                "raidpet2target",
            ] {
                assert_eq!(resolve(&mut app, token), None, "{token}");
            }

            // A hop off a game object is nobody, whatever its descriptor holds where a unit
            // keeps its target.
            let chest = app
                .world_mut()
                .spawn((Guid(CHEST), fields(GAME_OBJECT, 0, A)))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(CHEST, chest);
            app.insert_resource(Selection {
                target: Some(chest),
                guid: Some(CHEST),
                ..Default::default()
            });
            assert_eq!(guid_of(&mut app, "target"), Some(CHEST), "the base is held");
            assert_eq!(resolve(&mut app, "targettarget"), None);
        }

        /// Text after the base, or after a hop, that is not `target` names nobody, and `npc` is
        /// an exact compare with no chain.
        #[test]
        fn text_that_is_not_a_target_hop_names_nobody() {
            let mut app = app();
            for token in [
                "party1foo",
                "party1targetfoo",
                "party1targettarge",
                "party1 target",
                "playerfoo",
                "playertargets",
                "pettarge",
                "targetfoo",
                "mouseoverx",
                "raid3targettargetfoo",
                "npctarget",
                "bogus",
                "",
            ] {
                assert_eq!(resolve(&mut app, token), None, "{token}");
            }
            // The control: the same tokens without the tail resolve.
            assert_eq!(guid_of(&mut app, "party1"), Some(A));
            assert_eq!(guid_of(&mut app, "player"), Some(ME));
        }

        /// The compares fold ASCII case, the hops' included (`_strnicmp`, `0x5159f1`), and the
        /// chain has no depth limit: three hops round `A`, the mob and `B` come back to `A`.
        #[test]
        fn a_chain_folds_case_and_has_no_depth_limit() {
            let mut app = app();
            for token in [
                "PARTY1TARGET",
                "Party1Target",
                "party1TARGET",
                "PartY1tArGeT",
            ] {
                assert_eq!(guid_of(&mut app, token), Some(MOB), "{token}");
            }
            assert_eq!(guid_of(&mut app, "PlayerTarget"), Some(MOB));
            assert_eq!(guid_of(&mut app, "TargetTargetTarget"), Some(A));
            assert_eq!(
                guid_of(&mut app, "RaidPet4Target"),
                Some(MOB),
                "our own row's pet"
            );
            for hops in [3, 30, 3000] {
                let token = format!("party1{}", "target".repeat(hops));
                assert_eq!(guid_of(&mut app, &token), Some(A), "{hops} hops round A");
            }
            let token = format!("party1{}", "target".repeat(3001));
            assert_eq!(guid_of(&mut app, &token), Some(MOB));
        }

        /// `"pet"` is our `UNIT_FIELD_CHARM`, else our `UNIT_FIELD_SUMMON` (`0x515a5e`-`0x515a6e`),
        /// read off our descriptor at the call: the pet bar's guid names nothing, a charm takes
        /// the summon's place, and with both zero the token is nobody (`0x5159d3`).
        #[test]
        fn the_pet_base_is_our_charm_else_our_summon() {
            const CHARM: u16 = 6;
            let mut app = app();
            let me = app.world().resource::<GuidIndex>().0[&ME];
            let mut bar = crate::ui_pet::PetBar::default();
            bar.spells.pet_guid = A_PET;
            app.insert_resource(bar);
            assert_eq!(
                guid_of(&mut app, "pet"),
                Some(MY_PET),
                "the summon, not the bar"
            );
            assert_eq!(guid_of(&mut app, "pettarget"), Some(MOB));

            let mut charmed = fields(PLAYER, MY_PET, MOB);
            charmed.0.merge(ObjectFields::from_pairs(&[
                (CHARM, LONELY as u32),
                (CHARM + 1, (LONELY >> 32) as u32),
            ]));
            app.world_mut().entity_mut(me).insert(charmed);
            assert_eq!(guid_of(&mut app, "pet"), Some(LONELY), "the charm first");

            app.world_mut()
                .entity_mut(me)
                .insert(fields(PLAYER, 0, MOB));
            assert_eq!(guid_of(&mut app, "pet"), None, "neither names nobody");
        }

        /// A number wraps in 32 bits as the reference's inline parse does (`0x515af4`): 2^32 + 1
        /// is row 1, and digits stop at the first letter.
        #[test]
        fn a_row_number_wraps_as_the_reference_parse_does() {
            let mut app = app();
            assert_eq!(guid_of(&mut app, "party4294967297target"), Some(MOB));
            assert_eq!(guid_of(&mut app, "party01target"), Some(MOB));
            assert_eq!(guid_of(&mut app, "party1x"), None);
        }
    }

    /// The range verbs end to end, with the resolver feed that pushes its inputs and the
    /// host that measures the unit each names at the call: `CheckInteractDistance` and `CanInspect`
    /// answer for a `target` chain (`0x48ba00` and `0x48a1b0` resolve their token through
    /// `0x515970`), on the unit the chain names and not on the base.
    #[test]
    fn the_range_verbs_answer_for_a_target_chain() {
        use benilla_protocol::messages::{member_status, GroupMemberEntry, ObjectFields};

        const ME: u64 = 0x10;
        const A: u64 = 0x1001;
        const B: u64 = 0x1002;
        const MOB: u64 = 0xF130_0000_0000_0001;
        const A_PET: u64 = 0xF140_0000_0000_1001;
        /// `OBJECT_FIELD_TYPE`, `UNIT_FIELD_SUMMON`, `UNIT_FIELD_TARGET` and
        /// `UNIT_FIELD_FACTIONTEMPLATE`.
        const OBJECT_TYPE: u16 = 2;
        const SUMMON: u16 = 8;
        const TARGET: u16 = 16;
        const TEMPLATE: u16 = 35;
        use crate::target::relations::fixture::{factions, HUMAN};

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(crate::client_state::ClientState::InWorld)
            .init_resource::<Selection>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .insert_resource(factions())
            .insert_resource(NetCommands(tx))
            .add_plugins(crate::ui_aura::UiAuraPlugin);
        app.insert_non_send_resource(UiScript::new().unwrap());

        // We at the origin, party1 (`A`) 5 yards off with a pet 15 yards off, the mob 20 yards
        // off, and `B` 3 yards off. We and `A` target the mob, the mob targets `B`, the pet
        // targets `B`, and `B` targets nobody. All are Human, so the players cooperate with us.
        let mut spawn = |guid: u64, kind: u32, summon: u64, target: u64, yards: f32| {
            let mut pairs = vec![(OBJECT_TYPE, kind), (TEMPLATE, HUMAN)];
            for (field, guid) in [(SUMMON, summon), (TARGET, target)] {
                pairs.push((field, guid as u32));
                pairs.push((field + 1, (guid >> 32) as u32));
            }
            let e = app
                .world_mut()
                .spawn((
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&pairs)),
                    // Along one axis, so d² is exactly yards².
                    Transform::from_xyz(yards, 0.0, 0.0),
                ))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(guid, e);
            e
        };
        let me = spawn(ME, 0x19, 0, MOB, 0.0);
        spawn(A, 0x19, A_PET, MOB, 5.0);
        spawn(A_PET, 0x09, 0, B, 15.0);
        let mob = spawn(MOB, 0x09, 0, B, 20.0);
        spawn(B, 0x19, 0, 0, 3.0);
        app.world_mut().entity_mut(me).insert(SelfPlayer);
        app.world_mut()
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                0,
                0,
                vec![GroupMemberEntry {
                    name: "Brisca".into(),
                    guid: A,
                    status: member_status::ONLINE,
                    flags: 0,
                }],
                ME,
                None,
                Some(ME),
            );
        app.insert_resource(Selection {
            target: Some(mob),
            guid: Some(MOB),
            ..Default::default()
        });
        app.update();

        let answers = |app: &mut App, expr: &str| -> bool {
            live::eval_lent(app.world_mut(), &format!("return {expr} ~= nil"))
        };
        // The base and each hop, at their own distances: the 10-yard row is `1`, the 30-yard `4`.
        for (token, kind, want) in [
            ("player", 1, true),
            ("target", 1, false),
            ("target", 4, true),
            ("playertarget", 1, false),
            ("playertarget", 4, true),
            ("party1target", 1, false),
            ("party1target", 4, true),
            ("PARTY1TARGET", 4, true),
            ("party1targettarget", 1, true),
            ("targettarget", 1, true),
            ("partypet1", 1, false),
            ("partypet1", 4, true),
            ("partypet1target", 1, true),
            // `B` targets nobody, so the chain ends there.
            ("party1targettargettarget", 4, false),
            ("party2target", 4, false),
            ("party1foo", 4, false),
        ] {
            assert_eq!(
                answers(
                    &mut app,
                    &format!(r#"CheckInteractDistance("{token}", {kind})"#)
                ),
                want,
                "CheckInteractDistance({token}, {kind})"
            );
        }
        // A token the resolver does not know raises (`0x515c14`): `npc` is one exact compare with
        // no chain, so it knows no `npctarget`; a bad suffix after a known prefix is GUID 0.
        let call = r#"CheckInteractDistance("npctarget", 4)"#;
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .run(call)
                .is_err(),
            "{call}"
        );
        // `CanInspect`: a player within 10 yards, so `B` and not the mob 20 yards off.
        for (token, want) in [
            ("party1targettarget", true),
            ("targettarget", true),
            ("party1", true),
            ("party1target", false),
            ("target", false),
            ("party1targettargettarget", false),
        ] {
            assert_eq!(
                answers(&mut app, &format!(r#"CanInspect("{token}")"#)),
                want,
                "CanInspect({token})"
            );
        }
    }

    /// `UnitRace` and `UnitClass` push the table's name at the install's locale column beside the
    /// token (`0x518301`/`0x51830c`, `0x518451`/`0x51845c`): a koKR table gives Korean names and
    /// the same tokens, a unit whose race or class has no row gets neither, and no table names
    /// nothing.
    #[test]
    fn the_snapshot_names_race_and_class_from_the_tables() {
        const BYTES_0: u16 = 36;
        let snap = |bytes: u32, names: UnitNames<'_>| {
            snapshot(
                &ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                    BYTES_0, bytes,
                )])),
                0,
                None,
                0,
                names,
                Default::default(),
            )
        };
        // Night Elf (4) Priest (5).
        let bytes = 4 | (5 << 8);
        let korean = TestNames::korean();
        let s = snap(bytes, korean.names());
        assert_eq!(s.race.as_deref(), Some("나이트 엘프"));
        assert_eq!(s.race_file.as_deref(), Some("NightElf"));
        assert_eq!(s.class.as_deref(), Some("사제"));
        assert_eq!(s.class_file.as_deref(), Some("PRIEST"));
        let english = TestNames::english();
        let s = snap(bytes, english.names());
        assert_eq!(
            (s.race.as_deref(), s.class.as_deref()),
            (Some("Night Elf"), Some("Priest"))
        );
        // Class 6 has no row in 1.12; race 0 none either.
        let s = snap(4 | (6 << 8), korean.names());
        assert_eq!((s.class, s.class_file), (None, None));
        let s = snap(5 << 8, korean.names());
        assert_eq!((s.race, s.race_file), (None, None));
        // No table loaded: no names, as the reference with no row.
        let s = snap(bytes, UnitNames::default());
        assert_eq!((s.race, s.class), (None, None));
    }

    /// The team digit comes off the race byte, whatever template sits beside it (a GM's 35).
    #[test]
    fn the_team_digit_comes_off_the_race_byte_not_the_faction_template() {
        use benilla_protocol::ObjectFields;
        /// `UNIT_FIELD_FACTIONTEMPLATE` and `UNIT_FIELD_BYTES_0` (`[obj+0x110]+0x74`, `+0x78`).
        const FACTIONTEMPLATE: u16 = 35;
        const BYTES_0: u16 = 36;
        /// vmangos's GM template: `FactionTemplate.dbc` group mask 0.
        const GM_TEMPLATE: u32 = 35;

        let teams = crate::chr_classes::RacePvpTeams::stock();
        let team = |fields: &[(u16, u32)]| {
            snapshot(
                &ObjectStore(ObjectFields::from_pairs(fields)),
                0,
                None,
                0,
                UnitNames {
                    teams: Some(&teams),
                    ..Default::default()
                },
                Default::default(),
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

    /// A new VM is seeded with the shipped `Languages.dbc`, so each race's own tongue resolves by
    /// name to the id vmangos keys `KnowsLanguage` on (`SharedDefines.h:253-269`).
    #[test]
    fn a_new_vm_resolves_every_racial_language_by_name() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let mut world = World::new();
        world.insert_resource(LanguagesRes(
            benilla_formats::load_languages(&mut chain).expect("Languages.dbc"),
        ));
        let mut script = UiScript::new().expect("VM");
        script.keep_calls();
        // `SendChatMessage` needs a local player (`0x49f223`).
        script.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                ..Default::default()
            }),
        );
        seed_language_table(&world, &mut script);
        let want = [
            ("Orcish", 1),
            ("Darnassian", 2),
            ("Taurahe", 3),
            ("Dwarvish", 6),
            ("Common", 7),
            ("Gnomish", 13),
            ("Troll", 14),
            ("Gutterspeak", 33),
        ];
        for (name, _) in want {
            script
                .run(&format!(r#"SendChatMessage("hi", "SAY", "{name}")"#))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let got: Vec<Option<u32>> = script
            .take_chat_sends()
            .iter()
            .map(|c| c.language)
            .collect();
        let ids: Vec<Option<u32>> = want.iter().map(|&(_, id)| Some(id)).collect();
        assert_eq!(got, ids);
    }

    /// The race load's walk of the shipped DBCs, as `0x5efe00` walks them: the stock chain's nine
    /// rows, each with its side.
    #[test]
    fn the_race_sides_load_off_the_shipped_tables() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let teams = benilla_formats::load_race_pvp_teams(&mut chain).expect("ChrRaces walk");
        assert_eq!(teams, crate::chr_classes::RacePvpTeams::stock().0);
    }

    /// `CanInspect` answers by `0x4944a0` (`0x48a2a1`), whose player and cooperation legs the host
    /// judges at the call: a boar in interact range is not inspectable, nor an unflagged Orc, while a Human
    /// duel or free-for-all opponent is, `CanCooperate 0x606ba0` reading the faction group alone.
    /// You are, and a charmed Human is not (`0x606bab`). `CheckInteractDistance` is distance alone.
    #[test]
    fn a_creature_target_reads_its_real_distance() {
        use crate::target::relations::fixture::{
            creature, duelling, factions, flagged, player, store_of, FFA, HUMAN, ORC,
        };
        use crate::target::RelUnit;

        const ME: u64 = 0x0000_0000_0000_0001;
        const BOAR: u64 = 0xF130_0000_0000_0002;
        const OTHER: u64 = 0x0000_0000_0000_0003;
        const ARBITER: u64 = 0xF110_0000_0000_0004;

        /// Seat `me` and one target `yards` away and answer `expr` against them.
        fn ask(me: RelUnit, target: RelUnit, yards: f32, expr: &str) -> bool {
            let mut app = App::new();
            app.init_resource::<crate::ui_party::GroupState>()
                .init_resource::<Reputations>()
                .insert_resource(factions());
            app.insert_non_send_resource(UiScript::new().unwrap());

            let me_entity = app
                .world_mut()
                .spawn((
                    SelfPlayer,
                    Guid(me.guid),
                    store_of(&me),
                    Transform::from_xyz(0.0, 0.0, 0.0),
                ))
                .id();
            let target_entity = app
                .world_mut()
                .spawn((
                    Guid(target.guid),
                    store_of(&target),
                    // Along one axis, so d² is exactly yards².
                    Transform::from_xyz(yards, 0.0, 0.0),
                ))
                .id();
            app.insert_resource(crate::net::GuidIndex(
                [(me.guid, me_entity), (target.guid, target_entity)]
                    .into_iter()
                    .collect(),
            ));
            app.insert_resource(Selection {
                target: Some(target_entity),
                guid: Some(target.guid),
                ..Default::default()
            });
            // The resolver's inputs its feed would push: us and the target, both held.
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .set_unit_guids(&benilla_ui::script::UnitGuids {
                    player: me.guid,
                    target: target.guid,
                    held: HashMap::from([(me.guid, 0), (target.guid, 0)]),
                    ..Default::default()
                });

            live::eval_lent(app.world_mut(), expr)
        }

        let me = player(ME, HUMAN);
        let boar = creature(BOAR, ORC);
        assert!(
            ask(
                me,
                boar,
                40.0,
                r#"return CheckInteractDistance("target", 4) == nil"#
            ),
            "a creature 40 yards away is out of the 30-yard row"
        );
        assert!(
            ask(
                me,
                boar,
                15.0,
                r#"return CheckInteractDistance("target", 4) ~= nil"#
            ),
            "a creature 15 yards away is inside the 30-yard row"
        );
        assert!(
            ask(
                me,
                boar,
                15.0,
                r#"return CheckInteractDistance("target", 1) == nil"#
            ),
            "…and outside the 10-yard one: the table is indexed, not a constant"
        );
        assert!(
            ask(
                me,
                boar,
                3.0,
                r#"return CheckInteractDistance("target", 1) ~= nil"#
            ),
            "a creature 3 yards away is in interact range"
        );
        assert!(
            ask(me, boar, 3.0, r#"return CanInspect("target") == nil"#),
            "…and is still not inspectable: the player leg rides in the entry"
        );

        let inspectable = |me: RelUnit, them: RelUnit, yards: f32| {
            ask(me, them, yards, r#"return CanInspect("target") ~= nil"#)
        };
        let human = player(OTHER, HUMAN);
        assert!(inspectable(me, human, 3.0), "a Human 3 yards away");
        assert!(!inspectable(me, human, 15.0), "…not 15 yards away");
        assert!(
            !inspectable(me, player(OTHER, ORC), 3.0),
            "an unflagged Orc: not attackable, and not one to cooperate with"
        );
        assert!(
            inspectable(duelling(me, ARBITER, 1), duelling(human, ARBITER, 2), 3.0),
            "a Human duel opponent: attackable, and still of our faction group"
        );
        assert!(
            inspectable(flagged(me, FFA), flagged(human, FFA), 3.0),
            "a Human free-for-all opponent"
        );
        let charmed = RelUnit {
            charmed_by: BOAR,
            ..human
        };
        assert!(!inspectable(me, charmed, 3.0), "a charmed Human");
        assert!(
            ask(me, boar, 3.0, r#"return CanInspect("player") ~= nil"#),
            "yourself, ahead of the cooperation leg (`0x4944dd`)"
        );
    }

    /// The two reaction fields run opposite ways: `UnitReaction`'s is the unit's toward the player,
    /// the tooltip's creature-type gate the player's toward the unit (`0x52a2d9`). The Cenarion
    /// Circle, template 996, not at war, reads neutral toward the player and friendly from him, so
    /// its tooltip shows no creature type. No active player reads 0, which shows none.
    #[test]
    fn the_player_reaction_is_the_players_toward_the_unit() {
        use crate::target::relations::fixture::{creature, human_warrior, Scene};

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = crate::target::Factions::from_catalog(
            benilla_formats::load_faction_catalog(&mut chain).expect("faction tables"),
        );
        let me = human_warrior(0x10);
        let mut scene = Scene::new(me, Some(factions));
        let druid = scene.hold(creature(0xF130_0000_0000_0011, 996));
        let rel = scene.rel();
        assert_eq!(
            unit_reaction(&rel, &druid, Some(&me)),
            4,
            "neutral toward us"
        );
        assert_eq!(
            player_reaction(&rel, &druid, Some(&me)),
            5,
            "friendly from us"
        );
        assert_eq!(player_reaction(&rel, &druid, None), 0);
    }

    /// A race's PvP side is the chain's (`0x5efe00`: `ChrRaces` field 2 to the template's group
    /// mask), so a chain that moves Goblin (9) to the Horde and adds a race 10 on the Alliance
    /// side answers both, in the snapshot's team digit and the race's faction group, and a race
    /// with no row has no side.
    #[test]
    fn a_races_side_is_the_chains() {
        use benilla_protocol::messages::ObjectFields;
        /// `UNIT_FIELD_BYTES_0`, whose byte 0 is the race.
        const BYTES_0: u16 = 36;
        let teams = crate::chr_classes::RacePvpTeams(HashMap::from([(1, 1), (9, 0), (10, 1)]));
        let names = UnitNames {
            teams: Some(&teams),
            ..Default::default()
        };
        let team = |race: u32| {
            snapshot(
                &ObjectStore(ObjectFields::from_pairs(&[(BYTES_0, race)])),
                0,
                None,
                0,
                names,
                Default::default(),
            )
            .pvp_team
        };
        assert_eq!(team(9), 0, "the chain's Goblin is Horde");
        assert_eq!(team(10), 1, "a race the stock chain lacks");
        assert_eq!(team(2), -1, "a race this chain has no row for");
        assert_eq!(names.faction_group(9), Some("Horde"));
        assert_eq!(names.faction_group(10), Some("Alliance"));
        assert_eq!(names.faction_group(2), None);
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
            0,
            Some("Hunter".into()),
            0,
            Default::default(),
            Default::default(),
        );
        assert_eq!((alive.health, alive.max_health), (1200, 1500));
        assert_eq!((alive.power, alive.max_power), (300, 900));
        assert!(!alive.dead);

        let feigning = snapshot(
            &ObjectStore(ObjectFields::from_pairs(
                &[vitals.as_slice(), &[(DYNFLAGS, 0x20)]].concat(),
            )),
            0,
            Some("Hunter".into()),
            0,
            Default::default(),
            Default::default(),
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

    /// The quest-log window rides a player's snapshot whole, and a non-player's is all zero, as
    /// `IsUnitOnQuest`'s lookup is TYPEMASK_PLAYER (`0x4dfe97`).
    #[test]
    fn a_players_snapshot_carries_its_quest_log_window_and_a_creatures_none() {
        use benilla_protocol::messages::ObjectFields;

        const OBJECT_TYPE: u16 = 2;
        const QUEST_LOG_1_1: u16 = 198;
        let log = [
            (QUEST_LOG_1_1, 783),
            (QUEST_LOG_1_1 + 1, 0x0100_0003),
            (QUEST_LOG_1_1 + 57, 7),
        ];
        let snap = |type_bits: u32| {
            snapshot(
                &ObjectStore(ObjectFields::from_pairs(
                    &[log.as_slice(), &[(OBJECT_TYPE, type_bits)]].concat(),
                )),
                0,
                None,
                0,
                Default::default(),
                Default::default(),
            )
        };
        let player = snap(0x19);
        assert_eq!(player.quest_log[0], [783, 0x0100_0003, 0]);
        assert_eq!(player.quest_log[19], [7, 0, 0]);
        assert_eq!(player.quest_log[1], [0, 0, 0]);
        assert_eq!(
            snap(0x09).quest_log,
            [[0; 3]; 20],
            "a creature has no PLAYER block"
        );
    }

    /// The rank getter's two gates (`0x605620`), through `enrich_unit`'s wiring.
    #[test]
    fn the_rank_gate_zeroes_a_pet_or_charm() {
        use benilla_protocol::messages::ObjectFields;

        const ENTRY: u32 = 12397; // Ol' Sooty, a rank-1 elite
                                  // The guid's entry bits name another creature: the record is keyed by the descriptor's
                                  // `OBJECT_FIELD_ENTRY` (field 3), `[[unit+8]+0xc]` at `0x604629`.
        const GUID: u64 = (0xF130u64 << 48) | (299u64 << 24) | 0x42;
        const OBJECT_ENTRY: u16 = 3;
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
                pet_spell_list: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );

        let rank_of = |fields: &[(u16, u32)]| {
            let mut pairs = vec![(OBJECT_ENTRY, ENTRY)];
            pairs.extend_from_slice(fields);
            let store = ObjectStore(ObjectFields::from_pairs(&pairs));
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
        let store = ObjectStore(ObjectFields::from_pairs(&[(OBJECT_ENTRY, ENTRY + 1)]));
        let mut s = UnitState::default();
        enrich_unit(&mut s, GUID, &names, &store, None, None);
        assert_eq!(s.rank, 0, "an un-queried creature has no classification");
        let store = ObjectStore(ObjectFields::from_pairs(&[]));
        let mut s = UnitState::default();
        enrich_unit(&mut s, GUID, &names, &store, None, None);
        assert_eq!(s.rank, 0, "a descriptor with no entry yet has no record");
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
        let store = ObjectStore(ObjectFields::from_pairs(&[
            (3, ENTRY), // `OBJECT_FIELD_ENTRY`
            (FACTIONTEMPLATE, template_id),
        ]));

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
            pet_spell_list: 0,
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
    fn pvp_announcement_names_the_new_state() {
        assert_eq!(
            pvp_announcement(true),
            ("ERR_PVP_TOGGLE_ON", "PVP_TOGGLE_ON_VERBOSE")
        );
        assert_eq!(
            pvp_announcement(false),
            ("ERR_PVP_TOGGLE_OFF", "PVP_TOGGLE_OFF_VERBOSE")
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

        for on in [true, false] {
            let (toast, verbose) = pvp_announcement(on);
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

    /// `UnitCreatureType` through the real feed (`0x51a280`, resolver `0x605570`): a shapeshifted
    /// player answers the form's type before the race's, an unshifted one the race's, a form of
    /// type -1 falls through to the race, and a creature keeps its template's unless a form
    /// outranks it.
    #[test]
    fn unit_creature_type_names_the_form_before_the_race_and_a_creature_its_template() {
        use crate::names::CreatureRecord;
        use benilla_formats::ShapeshiftForm;
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::ObjectFields;

        /// `UNIT_FIELD_BYTES_0` and `UNIT_FIELD_BYTES_1`, absolute descriptor indices, and
        /// `OBJECT_FIELD_ENTRY`.
        const BYTES_0: u16 = 36;
        const BYTES_1: u16 = 138;
        const OBJECT_FIELD_ENTRY: u16 = 3;
        const NIGHT_ELF: u32 = 4;
        const CAT_FORM: u32 = 1;
        /// A row whose type is -1 in the shipped table.
        const NO_TYPE_FORM: u32 = 2;
        /// `HIGHGUID_UNIT` guids of entries 69 (a wolf, a Beast) and 70 (a druid, a Humanoid).
        const WOLF: u64 = 0xF130_0000_4500_0001;
        const DRUID: u64 = 0xF130_0000_4600_0001;

        let record = |name: &str, creature_type| CreatureRecord {
            name: name.into(),
            subname: None,
            creature_type,
            pet_family: 0,
            rank: 0,
            type_flags: 0,
            pet_spell_list: 0,
            civilian: false,
            racial_leader: false,
            display_id: 0,
        };
        let mut app = App::new();
        app.init_resource::<Selection>()
            .init_resource::<Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        let mut names = NameCache::default();
        names.insert_creature(69, Some(record("Wolf", 1)));
        names.insert_creature(70, Some(record("Druid", 7)));
        app.insert_resource(names);
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        let form = |creature_type| ShapeshiftForm {
            creature_type,
            ..Default::default()
        };
        spells.forms.insert(CAT_FORM, form(1));
        spells.forms.insert(NO_TYPE_FORM, form(-1));
        app.insert_resource(spells);
        // The words are the table's own, not English: a koKR install's column.
        app.insert_resource(crate::target::CreatureTypes::korean_for_tests());
        app.add_systems(Update, seat_tokens);

        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(0x77),
                ObjectStore(
                    ObjectFields::from_pairs(&[(BYTES_0, NIGHT_ELF)])
                        .into_created(ObjectType::Player),
                ),
            ))
            .id();
        let mut creature = |entry: u32| {
            app.world_mut()
                .spawn(ObjectStore(ObjectFields::from_pairs(&[(
                    OBJECT_FIELD_ENTRY,
                    entry,
                )])))
                .id()
        };
        let wolf = creature(69);
        let druid = creature(70);
        let creature_type = |app: &mut App, token: &str| -> Option<String> {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<Option<String>>(&format!(r#"return UnitCreatureType("{token}")"#))
                .unwrap()
        };
        let shift = |app: &mut App, unit: Entity, form: u32| {
            app.world_mut()
                .entity_mut(unit)
                .get_mut::<ObjectStore>()
                .unwrap()
                .0
                .merge(ObjectFields::from_pairs(&[(BYTES_1, form << 16)]));
            app.update();
        };
        let select = |app: &mut App, unit: Entity, guid: u64| {
            let mut selection = app.world_mut().resource_mut::<Selection>();
            selection.target = Some(unit);
            selection.guid = Some(guid);
            app.update();
        };

        app.update();
        assert_eq!(
            creature_type(&mut app, "player").as_deref(),
            Some("인간형"),
            "no form: the race's type"
        );
        shift(&mut app, me, CAT_FORM);
        assert_eq!(
            creature_type(&mut app, "player").as_deref(),
            Some("야수"),
            "Cat Form: the form's type, before the race's"
        );
        shift(&mut app, me, NO_TYPE_FORM);
        assert_eq!(
            creature_type(&mut app, "player").as_deref(),
            Some("인간형"),
            "a form of type -1 falls through to the race"
        );
        shift(&mut app, me, 0);
        assert_eq!(
            creature_type(&mut app, "player").as_deref(),
            Some("인간형"),
            "out of form again"
        );

        select(&mut app, wolf, WOLF);
        assert_eq!(
            creature_type(&mut app, "target").as_deref(),
            Some("야수"),
            "a creature's template type"
        );
        select(&mut app, druid, DRUID);
        assert_eq!(
            creature_type(&mut app, "target").as_deref(),
            Some("인간형"),
            "a Humanoid template"
        );
        shift(&mut app, druid, CAT_FORM);
        assert_eq!(
            creature_type(&mut app, "target").as_deref(),
            Some("야수"),
            "a form outranks the template, on a creature as on a player"
        );
    }

    /// Event `0x111` fires from the local player's destructor, which a same-map teleport never
    /// reaches, and the initial-login map needs no ack.
    #[test]
    fn a_cross_map_worldport_fires_leaving_world_and_the_login_map_does_not() {
        let mut app = App::new();
        app.init_world_leave_sweep();
        crate::ui_script::add_worldport_sweep(&mut app);
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
        // The initial-login map posts no load; a cross-map port posts one, which the objects'
        // leg (absent here) takes after the frame.
        let port = |app: &mut App, needs_ack: bool| {
            if needs_ack {
                app.world_mut()
                    .resource_mut::<crate::net::PendingWorldports>()
                    .post_for_test();
            }
            app.update();
            app.world_mut()
                .resource_mut::<crate::net::PendingWorldports>()
                .take_for_test();
        };

        let arm = |app: &mut App| {
            app.world_mut()
                .resource_mut::<crate::ui_script::LeavingWorldArmed>()
                .arm();
        };

        // A world began: the reference arms the latch from the local player's create.
        arm(&mut app);
        port(&mut app, false);
        assert_eq!(left(&mut app), 0, "the initial-login map is an arrival");

        port(&mut app, true);
        assert_eq!(left(&mut app), 1, "a cross-map port leaves a world");

        app.update();
        assert_eq!(
            left(&mut app),
            1,
            "once per port, not once per frame after it"
        );

        // Once per world: the port above spent the latch and no new avatar re-armed it, so a
        // second departure (a quit on the loading screen) fires nothing, as in the reference.
        port(&mut app, true);
        assert_eq!(
            left(&mut app),
            1,
            "a second departure with the latch spent fired again — [0xb4b424] is per world"
        );

        // Re-armed, as the new world's create does: the next departure is its own.
        arm(&mut app);
        port(&mut app, true);
        assert_eq!(left(&mut app), 2, "the next world's departure fires again");
    }
}
