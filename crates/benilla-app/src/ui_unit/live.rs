//! The unit getters' reads at the call that no unit snapshot carries, the call host's half of
//! their questions: the six relation verbs' functions over the units as of the call
//! ([`benilla_ui::script::RelationQuery`]), the range verbs' reach
//! ([`benilla_ui::script::UnitReachQuery`]), the item use's control exemption
//! ([`benilla_ui::script::ControlBypassQuery`]) and the language walk
//! ([`benilla_ui::script::KnownLanguagesQuery`], [`benilla_ui::script::DefaultLanguageQuery`]).
//! Each store reads as of the drain's place ([`super::views::ViewStores`]), as the unit getters'
//! own objects do.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    ControlBypassQuery, DefaultLanguageQuery, KnownLanguagesQuery, RelationQuery, UnitReach,
    UnitReachQuery, UnitRelations,
};

use crate::net::{ObjectStore, SelfPlayer};
use crate::target::{RelUnit, RelationWorld, Relations, UnitLookup};

/// What the unit getters read as they run beside the objects themselves.
#[derive(SystemParam)]
pub(crate) struct UnitFeedLive<'w, 's> {
    /// The relation inputs: the faction tables, the reputations, the roster, the creature records.
    relations: RelationWorld<'w, 's>,
    stores: super::views::ViewStores<'w, 's>,
    index: Option<Res<'w, crate::net::GuidIndex>>,
    me: Query<'w, 's, Entity, With<SelfPlayer>>,
    /// The scene's positions as the last frame's propagation left them, which no system of this
    /// frame's `Update` writes: the by-name walk's reading too ([`crate::target::PlayerLookup`]).
    transforms: Query<'w, 's, &'static GlobalTransform>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    actions: Option<Res<'w, crate::ui_action::PlayerActions>>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    languages: Option<Res<'w, super::LanguagesRes>>,
    default_languages: Option<Res<'w, super::DefaultLanguagesRes>>,
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
        let at = self.transforms.get(entity?).ok()?.translation();
        let me_at = self.transforms.get(self.player()?.0).ok()?.translation();
        let unit = self.relations.unit_of(guid, store);
        Some(super::held::reach_between(
            &self.rel(),
            Some(&unit),
            self.me().as_ref(),
            at,
            me_at,
        ))
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

    /// The languages the active player understands as of here, `None` without the tables
    /// ([`super::known_languages`]).
    fn known_languages(&self) -> Option<Vec<String>> {
        let (actions, spells, languages) = (
            self.actions.as_deref()?,
            self.spells.as_deref()?,
            self.languages.as_deref()?,
        );
        let store = self.player().map(|(_, s)| s);
        let has_skill_line = |line: u32| {
            store.is_some_and(|s| {
                (0..benilla_protocol::messages::PLAYER_SKILL_SLOTS)
                    .filter_map(|i| s.0.player_skill(i))
                    .any(|slot| u32::from(slot.skill_id) == line)
            })
        };
        Some(super::known_languages(
            actions.spells.iter().copied(),
            &spells.catalog,
            self.skill_lines.as_deref().map(|s| &s.catalog),
            store.map_or((0, 0), |s| s.race_class()),
            has_skill_line,
            &languages.0,
        ))
    }

    /// The active player's race's base language's name as of here (`0x5ec890`, `0x49fcd0`),
    /// `None` while no player is held, where the load's seat answers.
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
        ask.answer(|q: &RelationQuery| Some(q.answer(&*self)))
            || ask.answer(|q: &UnitReachQuery| Some(self.reach(q.0)))
            || ask.answer(|q: &ControlBypassQuery| Some(self.exempts(q.0)))
            || ask.answer(|_: &KnownLanguagesQuery| self.known_languages())
            || ask.answer(|_: &DefaultLanguageQuery| self.default_language())
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
    use benilla_ui::script::{CallHost, CallTail, ScriptCall, UiScript};
    use bevy::ecs::system::SystemState;

    struct Host<'a, 'w, 's> {
        live: &'a mut UnitFeedLive<'w, 's>,
        calls: Vec<ScriptCall>,
    }

    impl CallHost for Host<'_, '_, '_> {
        fn apply(&mut self, _: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            self.calls.push(call);
            None
        }

        fn read_live(&mut self, ask: &mut LiveAsk<'_>) -> bool {
            crate::script_calls::live::LiveFamily::answer(self.live, ask)
        }
    }

    let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
    let mut state = SystemState::<UnitFeedLive>::new(world);
    let mut host = Host {
        live: &mut state.get(world),
        calls: Vec::new(),
    };
    let answer = script.hosted(&mut host, |s| s.eval::<T>(chunk).expect("the chunk runs"));
    let calls = host.calls;
    world.insert_non_send_resource(script);
    (answer, calls)
}
