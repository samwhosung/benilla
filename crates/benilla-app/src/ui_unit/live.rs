//! The unit getters' reads at the call, the call host's half of their questions: the unit a
//! token resolves to ([`benilla_ui::script::UnitStateQuery`]), the units the by-name walk offers
//! ([`benilla_ui::script::HeldCandidatesQuery`]), the six relation verbs' functions over the units
//! as of the call ([`benilla_ui::script::RelationQuery`]), the range verbs' reach
//! ([`benilla_ui::script::UnitReachQuery`]), the item use's control exemption
//! ([`benilla_ui::script::ControlBypassQuery`]) and the language walk
//! ([`benilla_ui::script::KnownLanguagesQuery`], [`benilla_ui::script::DefaultLanguageQuery`])
//! and the zone's faction masks ([`benilla_ui::script::PlayerFactionMasksQuery`]).
//! Each store reads as of the drain's place ([`super::views::ViewStores`]), as the unit getters'
//! own objects do.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    ControlBypassQuery, DefaultLanguageQuery, HeldCandidatesQuery, KnownLanguagesQuery,
    LiveCandidate, LiveCandidates, PlayerFactionMasksQuery, RelationQuery, UnitHeld, UnitHeldQuery,
    UnitReach, UnitReachQuery, UnitRelations, UnitStateAnswer, UnitStateQuery,
};

use crate::names::NameCache;
use crate::net::{Guid, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::target::{Candidacy, NameSearch, RelUnit, RelationWorld, Relations, UnitLookup};

/// What a unit's state reads beside its descriptor and the relation inputs: the tables, the name
/// and creature caches, the roster's raid marks and the guild cache; each absent in a harness
/// without its plugin, where the getters read their snapshots.
#[derive(SystemParam)]
pub(crate) struct StateSources<'w, 's> {
    tables: super::SnapshotTables<'w>,
    names: Option<Res<'w, NameCache>>,
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    guild: Option<Res<'w, crate::ui_guild::GuildState>>,
    /// Every object by its guid, the units the by-name walk offers among them.
    objects: Query<'w, 's, (Entity, &'static Guid)>,
}

/// What the unit getters read as they run beside the objects themselves.
#[derive(SystemParam)]
pub(crate) struct UnitFeedLive<'w, 's> {
    /// The relation inputs: the faction tables, the reputations, the roster, the creature records.
    relations: RelationWorld<'w, 's>,
    stores: super::views::ViewStores<'w, 's>,
    index: Option<Res<'w, crate::net::GuidIndex>>,
    me: Query<'w, 's, Entity, With<SelfPlayer>>,
    /// The scene's positions as of here, each composed through its parents on demand, so a unit
    /// created or moved this frame, or riding a transport, reads where it stands.
    transforms: bevy::transform::helper::TransformHelper<'w, 's>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    actions: Option<Res<'w, crate::ui_action::PlayerActions>>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    languages: Option<Res<'w, super::LanguagesRes>>,
    default_languages: Option<Res<'w, super::DefaultLanguagesRes>>,
    state: StateSources<'w, 's>,
}

impl UnitFeedLive<'_, '_> {
    /// The active player's entity and store as of here.
    fn player(&self) -> Option<(Entity, &ObjectStore)> {
        let entity = self.me.single().ok()?;
        Some((entity, self.stores.get(entity).ok()?))
    }

    /// The active player as the relation functions judge it, once its guid and object are known.
    fn me(&self) -> Option<RelUnit> {
        let guid = self.relations.me_guid()?;
        Some(self.relations.unit_of(guid, self.player()?.1))
    }

    /// The unit `guid` names as of here (`0x468460`, typemask 8), its entity with it.
    fn held(&self, guid: u64) -> Option<(Option<Entity>, &ObjectStore)> {
        let entity = self.index.as_deref().and_then(|i| i.0.get(&guid).copied());
        let store = self.stores.at(guid, entity).filter(|s| s.is_unit())?;
        Some((entity, store))
    }

    /// The relation functions over the units as of here.
    fn rel(&self) -> Relations<'_> {
        Relations {
            factions: self.relations.factions(),
            reputations: self.relations.reputations(),
            me: self.relations.me_guid(),
            roster: self.relations.roster(),
            units: self,
        }
    }

    /// Both units, `None` when either is not held.
    fn pair(&self, a: u64, b: u64) -> Option<(RelUnit, RelUnit)> {
        Some((self.unit(a)?, self.unit(b)?))
    }

    /// `guid`'s reach from the active player: its squared distance between the two positions
    /// (`0x48a26f`), and `0x4944a0`'s player and cooperation legs.
    fn reach(&self, guid: u64) -> Option<UnitReach> {
        let (entity, store) = self.held(guid)?;
        let at = self.position(entity?)?;
        let me_at = self.position(self.player()?.0)?;
        let unit = self.relations.unit_of(guid, store);
        Some(super::held::reach_between(
            &self.rel(),
            Some(&unit),
            self.me().as_ref(),
            at,
            me_at,
        ))
    }

    /// `entity`'s position in the world as of here.
    fn position(&self, entity: Entity) -> Option<Vec3> {
        let global = self.transforms.compute_global_transform(entity).ok()?;
        Some(global.translation())
    }

    /// Whether `spell` passes the lost-control gate for the active player as of here
    /// ([`super::control_gate::exempts_lost_control`]); a spell with no record does not
    /// (`0x5d8ec7`), nor does any with no player held.
    fn exempts(&self, spell: u32) -> bool {
        let (Some(spells), Some((_, store))) = (self.spells.as_deref(), self.player()) else {
            return false;
        };
        spells.catalog.get(spell).is_some_and(|cast| {
            super::control_gate::exempts_lost_control(&spells.catalog, cast, &store.0)
        })
    }

    /// The languages the active player understands as of here ([`super::known_languages`]),
    /// `Some(None)` with no player held (`0x49fb5e`), `None` without the tables.
    fn known_languages(&self) -> Option<Option<Vec<String>>> {
        let Some((_, store)) = self.player() else {
            return Some(None);
        };
        let (actions, spells, languages) = (
            self.actions.as_deref()?,
            self.spells.as_deref()?,
            self.languages.as_deref()?,
        );
        let has_skill_line = |line: u32| {
            (0..benilla_protocol::messages::PLAYER_SKILL_SLOTS)
                .filter_map(|i| store.0.player_skill(i))
                .any(|slot| u32::from(slot.skill_id) == line)
        };
        Some(Some(super::known_languages(
            actions.spells.iter().copied(),
            &spells.catalog,
            self.skill_lines.as_deref().map(|s| &s.catalog),
            store.race_class(),
            has_skill_line,
            &languages.0,
        )))
    }

    /// The active player's faction template's `(friend, enemy)` group masks as of here
    /// (`0x48d5d6`-`0x48d613`): `None` with no player held (`0x48d5bc`), no template sent or no
    /// template row (`0x48d5e1`-`0x48d5fe`).
    fn faction_masks(&self) -> Option<(u32, u32)> {
        let (_, store) = self.player()?;
        let template = self
            .relations
            .factions()?
            .catalog()
            .template(store.0.unit_faction_template()?)?;
        Some((template.friend_group_mask, template.enemy_group_mask))
    }

    /// The active player's race's base language's name as of here (`0x5ec890`, `0x49fcd0`),
    /// `None` while no player is held, where the VM's seat, none, answers.
    fn default_language(&self) -> Option<Option<String>> {
        let (_, store) = self.player()?;
        Some(
            store
                .0
                .unit_race()
                .zip(self.default_languages.as_deref())
                .and_then(|(race, langs)| langs.0.name(u32::from(race)))
                .map(str::to_string),
        )
    }
}

impl UnitFeedLive<'_, '_> {
    /// The unit `q` names as of here, built as its snapshot is: the active player as `"player"`
    /// reads it, any other held unit as a token naming it does, a guid held by nothing as what
    /// the client knows of it; `None` for a guid held as no unit, or without the sources.
    fn unit_state(&self, q: &UnitStateQuery) -> Option<UnitStateAnswer> {
        let src = &self.state;
        let (names, group, guild) = (
            src.names.as_deref()?,
            src.group.as_deref()?,
            src.guild.as_deref()?,
        );
        let entity = self
            .index
            .as_deref()
            .and_then(|i| i.0.get(&q.guid).copied());
        // The object lookup each getter makes after the resolver (`0x468460`).
        let Some(store) = self.stores.at(q.guid, entity) else {
            let tables = src.tables.unit_names();
            return Some(UnitStateAnswer::Unheld(super::unheld_state(
                names, group, q.guid, tables,
            )));
        };
        // A game object or an item exists for nobody (`0x515fee`) and is named apart.
        if !store.is_unit() {
            return None;
        }
        let rel = self.rel();
        let me = self.me();
        if q.player {
            let mut s = super::player_snapshot(
                store,
                q.guid,
                &src.tables,
                names,
                Ask::Read,
                group,
                rel.factions,
                crate::ui_guild::cached_unit_guild(&store.0, guild),
            );
            s.is_pvp_ffa = me
                .as_ref()
                .is_some_and(|me| super::pvp_free_for_all(&rel, me));
            return Some(UnitStateAnswer::Held(s));
        }
        let object = |guid: u64| {
            let entity = self.index.as_deref().and_then(|i| i.0.get(&guid).copied());
            self.stores.at(guid, entity)
        };
        let held = super::held::HeldUnits {
            names,
            group,
            self_store: self.player().map(|(_, s)| s),
            unit_names: src.tables.unit_names(),
            types: src.tables.types(names),
            rel,
            me,
            tooltip: super::tooltip_facts::TooltipSources {
                names,
                catalog: src.tables.catalog(),
                object: &object,
                study: src.tables.study(),
            },
            guild,
        };
        // The name as the cache holds it: `UnitName` asks a miss inside its own call.
        let name = names
            .resolve_unit(q.guid, Some(store), Ask::Read)
            .map(str::to_string);
        Some(UnitStateAnswer::Held(held.state_named(store, q.guid, name)))
    }

    /// Whether `q` names a unit held as of here, with the legs a reader takes beside the object:
    /// built from the descriptor and the relation inputs alone, nothing copied; `None` for a guid
    /// held as no unit.
    fn unit_held(&self, q: &UnitStateQuery) -> Option<UnitHeld> {
        let entity = self
            .index
            .as_deref()
            .and_then(|i| i.0.get(&q.guid).copied());
        let Some(store) = self.stores.at(q.guid, entity) else {
            return Some(UnitHeld::Unheld);
        };
        if !store.is_unit() {
            return None;
        }
        // `"player"` carries no reaction toward itself; any other token the unit's toward us
        // (`0x5167e0`), plus one.
        let reaction = if q.player {
            0
        } else {
            let unit = self.relations.unit_of(q.guid, store);
            super::unit_reaction(&self.rel(), &unit, self.me().as_ref())
        };
        Some(UnitHeld::Held {
            is_player: q.player || benilla_protocol::guid::is_player(q.guid),
            reaction,
        })
    }

    /// The units `0x493aa0`'s object pass offers as of here (typemask 8, a cached name, no query
    /// sent), each measured from the active player at the call; none without an active player
    /// (`0x493ae0`), `None` without the name cache.
    fn candidates(&self) -> Option<LiveCandidates> {
        let names = self.state.names.as_deref()?;
        let Some((me_entity, _)) = self.player() else {
            return Some(LiveCandidates::default());
        };
        let origin = self.position(me_entity);
        let (rel, me) = (self.rel(), self.me());
        let held = origin
            .into_iter()
            .flat_map(|origin| {
                self.state.objects.iter().filter_map(move |(entity, guid)| {
                    let guid = guid.0;
                    let Candidacy::Named(name) = NameSearch::AnyUnit.candidacy(names, guid) else {
                        return None;
                    };
                    let at = self.position(entity)?;
                    // Held as a unit as of here (typemask 8): one a later packet creates is not.
                    let store = self.stores.at(guid, Some(entity)).filter(|s| s.is_unit())?;
                    let unit = self.relations.unit_of(guid, store);
                    Some(LiveCandidate {
                        guid,
                        name: name.to_string(),
                        dist2: at.distance_squared(origin),
                        is_player: benilla_protocol::guid::is_player(guid),
                        assistable_alive: crate::target::assistable_alive(
                            &rel,
                            me.as_ref(),
                            Some((&unit, store)),
                        ),
                    })
                })
            })
            .collect();
        Some(LiveCandidates {
            active_player: true,
            held,
        })
    }
}

impl UnitLookup for UnitFeedLive<'_, '_> {
    fn unit(&self, guid: u64) -> Option<RelUnit> {
        if self.relations.me_guid() == Some(guid) {
            return self.me();
        }
        let (_, store) = self.held(guid)?;
        Some(self.relations.unit_of(guid, store))
    }
}

impl UnitRelations for UnitFeedLive<'_, '_> {
    fn holds(&self, guid: u64) -> bool {
        self.unit(guid).is_some()
    }

    fn reaction(&self, a: u64, b: u64) -> Option<u8> {
        let (a, b) = self.pair(a, b)?;
        Some(self.rel().reaction(&a, &b))
    }

    fn can_attack(&self, a: u64, b: u64) -> Option<bool> {
        let (a, b) = self.pair(a, b)?;
        Some(self.rel().can_attack(&a, &b))
    }

    fn can_assist(&self, a: u64, b: u64) -> Option<bool> {
        let (a, b) = self.pair(a, b)?;
        Some(self.rel().can_assist(&a, &b))
    }

    fn can_cooperate(&self, a: u64, b: u64) -> Option<bool> {
        let (a, b) = self.pair(a, b)?;
        Some(self.rel().can_cooperate(&a, &b))
    }

    fn is_pvp(&self, guid: u64) -> Option<bool> {
        Some(self.rel().is_pvp(&self.unit(guid)?))
    }
}

impl crate::script_calls::live::LiveFamily for UnitFeedLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &UnitHeldQuery| self.unit_held(&q.0))
            || ask.answer(|q: &UnitStateQuery| self.unit_state(q))
            || ask.answer(|_: &HeldCandidatesQuery| self.candidates())
            || ask.answer(|q: &RelationQuery| Some(q.answer(&*self)))
            || ask.answer(|q: &UnitReachQuery| Some(self.reach(q.0)))
            || ask.answer(|q: &ControlBypassQuery| Some(self.exempts(q.0)))
            || ask.answer(|_: &KnownLanguagesQuery| self.known_languages())
            || ask.answer(|_: &DefaultLanguageQuery| self.default_language())
            || ask.answer(|_: &PlayerFactionMasksQuery| Some(self.faction_masks()))
    }
}

/// `chunk`'s answer with a host lent that answers this family's questions alone, off `world` as
/// it stands: the world's VM taken out for the call and put back.
#[cfg(test)]
pub(crate) fn eval_lent<T: benilla_ui::mlua::FromLuaMulti>(world: &mut World, chunk: &str) -> T {
    eval_lent_calls(world, chunk).0
}

/// [`eval_lent`], with the calls the chunk handed the host, which applies none.
#[cfg(test)]
pub(crate) fn eval_lent_calls<T: benilla_ui::mlua::FromLuaMulti>(
    world: &mut World,
    chunk: &str,
) -> (T, Vec<benilla_ui::script::ScriptCall>) {
    lend(world, chunk, false)
}

/// [`eval_lent_calls`] with the unit objects and the selection's and the interaction's globals
/// read through the host too, as the resolver reads them in game.
#[cfg(test)]
pub(crate) fn eval_lent_objects<T: benilla_ui::mlua::FromLuaMulti>(
    world: &mut World,
    chunk: &str,
) -> (T, Vec<benilla_ui::script::ScriptCall>) {
    lend(world, chunk, true)
}

#[cfg(test)]
fn lend<T: benilla_ui::mlua::FromLuaMulti>(
    world: &mut World,
    chunk: &str,
    objects: bool,
) -> (T, Vec<benilla_ui::script::ScriptCall>) {
    use crate::script_calls::live::LiveFamily;
    use benilla_ui::script::{CallHost, CallTail, ScriptCall, UiScript};
    use bevy::ecs::system::SystemState;

    type Lent<'w, 's> = (
        UnitFeedLive<'w, 's>,
        crate::script_calls::live::UnitLive<'w, 's>,
        crate::target::SelectionLive<'w, 's>,
    );

    struct Host<'a, 'w, 's> {
        live: &'a mut Lent<'w, 's>,
        objects: bool,
        calls: Vec<ScriptCall>,
    }

    impl CallHost for Host<'_, '_, '_> {
        fn apply(&mut self, _: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.calls.push(call);
            None
        }

        fn read_unit_object(
            &mut self,
            guid: u64,
            read: &mut dyn FnMut(&dyn benilla_ui::script::UnitObjectRead),
        ) -> bool {
            self.objects && self.live.1.read(guid, read)
        }

        fn read_unit_identity(
            &mut self,
            guid: u64,
            read: &mut dyn FnMut(&dyn benilla_ui::script::UnitIdentityRead),
        ) -> bool {
            self.objects && self.live.1.read_identity(guid, read)
        }

        fn read_unit_tokens(
            &mut self,
            read: &mut dyn FnMut(&dyn benilla_ui::script::UnitTokenObjects),
        ) -> bool {
            if self.objects {
                self.live.1.read_tokens(read);
            }
            self.objects
        }

        fn read_live(&mut self, ask: &mut LiveAsk<'_>) -> bool {
            self.live.0.answer(ask) || (self.objects && self.live.2.answer(ask))
        }
    }

    let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
    let mut state = SystemState::<Lent>::new(world);
    let mut host = Host {
        live: &mut state.get(world),
        objects,
        calls: Vec::new(),
    };
    let answer = script.hosted(&mut host, |s| s.eval::<T>(chunk).expect("the chunk runs"));
    let calls = host.calls;
    world.insert_non_send_resource(script);
    (answer, calls)
}
