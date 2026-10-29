//! What an item is judged against as a getter runs, the call host's half of
//! [`benilla_ui::script::ReqHeadQuery`] and its kin: the active player's fields and lists as of the
//! drain's place, each read as a getter asks it, as the red lines and the usable gate read them
//! (`0x5eaae0`, `0x5ea930`), the known-spell list (`0x60c740`), and the lock openers the lock walk scans
//! (`0x5da5e0`: the opener list `0xb700b8` and the items `0x622270` finds). With no active player
//! held the head answers none and the lock walk opens everything (`0x5da60b`).

use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    DualWieldQuery, KnowsSpellQuery, LockOpenersQuery, PlayerReqState, ProficiencyQuery,
    RepRankQuery, ReqHead, ReqHeadQuery, ReqSkillQuery, SkinningQuery,
};
use bevy::ecs::change_detection::Tick;

use crate::net::{ObjectStore, Objects, Proficiencies, Reputations};
use crate::target::Factions;
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_spellbook::SkillLines;

/// What the requirement state reads beside the active player's descriptor.
pub(super) struct ReqInputs<'a> {
    pub(super) proficiencies: &'a Proficiencies,
    pub(super) factions: Option<&'a Factions>,
    pub(super) reputations: &'a Reputations,
    pub(super) actions: &'a PlayerActions,
    pub(super) spells: Option<&'a Spells>,
    /// The skinning latch `[0xb700e4]`.
    pub(super) skinning: Option<u32>,
    pub(super) skill_lines: Option<&'a SkillLines>,
}

/// The requirement state of the active player `store`: level, class, race, skills, proficiencies
/// (`SMSG_SET_PROFICIENCY`), reputation ranks (the DBC base plus the `SMSG_INITIALIZE_FACTIONS`
/// standing), honor rank and medals, the dual-wield latch and the skinning value the unit tooltip
/// colours by.
pub(super) fn player_req_state(store: &ObjectStore, inputs: &ReqInputs) -> PlayerReqState {
    let mut skills = HashMap::new();
    for slot in 0..benilla_protocol::messages::PLAYER_SKILL_SLOTS {
        let Some(s) = store.0.player_skill(slot) else {
            continue;
        };
        if s.skill_id != 0 {
            // The requirement checks (`0x5eaae0`, `0x5ea930`) read the value plus the permanent
            // bonus, never the temporary one; the bonus is never negative in live data.
            skills.insert(
                u32::from(s.skill_id),
                u32::from(s.value).saturating_add_signed(i32::from(s.perm_bonus)),
            );
        }
    }
    let race = store.0.unit_race().unwrap_or(0);
    let class = store.0.unit_class().unwrap_or(0);
    let mut rep_ranks = HashMap::new();
    if let Some(cat) = inputs.factions.map(Factions::catalog) {
        for (id, info) in cat.reputation_factions() {
            let standing =
                usize::try_from(info.rep_index).map_or(0, |i| inputs.reputations.standing(i));
            rep_ranks.insert(
                id,
                benilla_formats::reputation_rank(info.base_for(race, class) + standing),
            );
        }
    }
    let can_dual_wield = inputs.actions.knows_dual_wield(inputs.spells);
    // The skinnable line's value (`0x529f06`-`0x529f58`): the latched spell's first `SKINNING`
    // effect, its points at the player's skill in the spell's line, kept only above 0.
    let skinning = inputs
        .skinning
        .and_then(|id| {
            let d = inputs.spells?.catalog.get(id)?;
            let e = d
                .effects
                .iter()
                .position(|&e| e == benilla_formats::SPELL_EFFECT_SKINNING)?;
            let catalog = inputs.skill_lines.map(|s| &s.catalog);
            let skill = crate::spell::spell_skill_value(Some(store), catalog, id);
            Some(d.effect_points_int(e, skill))
        })
        .filter(|&v| v > 0);
    PlayerReqState {
        level: store.0.unit_level().unwrap_or(0),
        class_id: u32::from(class),
        race_id: u32::from(race),
        skills,
        proficiency: inputs.proficiencies.0.clone(),
        rep_ranks,
        can_dual_wield,
        honor_rank: store.0.player_honor_rank().unwrap_or(0),
        pvp_medals: store.0.player_pvp_medals().unwrap_or(0),
        skinning,
    }
}

/// The dual-wield flag a learn sets (`0x5e9fc1`), read off the book the last time it or the
/// catalog changed.
#[derive(Default)]
pub(crate) struct LatchMemo {
    key: Option<(Tick, Tick)>,
    dual_wield: bool,
}

/// What the requirement getters read as they run, the active player's store and its items as of
/// the drain's place; each store absent in a harness without it, where its questions are
/// declined.
#[derive(SystemParam)]
pub(crate) struct RequirementsLive<'w, 's> {
    objects: Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    proficiencies: Option<Res<'w, Proficiencies>>,
    factions: Option<Res<'w, Factions>>,
    reputations: Option<Res<'w, Reputations>>,
    actions: Option<Res<'w, PlayerActions>>,
    spells: Option<Res<'w, Spells>>,
    skill_lines: Option<Res<'w, SkillLines>>,
    /// `[0xb700e4]`, the skinning spell the registrar latched (`0x4b262c`).
    learned: Option<Res<'w, crate::ui_action::LearnedAbilities>>,
    latches: Local<'s, LatchMemo>,
}

impl RequirementsLive<'_, '_> {
    /// The active player's store as of here, `None` while none is held.
    fn player(&self) -> Option<&ObjectStore> {
        self.objects.player().and_then(|e| self.stores.get(e).ok())
    }

    /// The two learn latches: the skinning spell as the registrar holds it, and the dual-wield
    /// flag, read again only when the book or the catalog moved.
    fn latches(&mut self) -> Option<(Option<u32>, bool)> {
        let (actions, spells) = (self.actions.as_ref()?, self.spells.as_ref()?);
        let key = (actions.last_changed(), spells.last_changed());
        if self.latches.key != Some(key) {
            *self.latches = LatchMemo {
                key: Some(key),
                dual_wield: actions.knows_dual_wield(Some(spells)),
            };
        }
        let skinning = self.learned.as_ref().and_then(|l| l.skinning);
        Some((skinning, self.latches.dual_wield))
    }

    /// The whole requirement state as of here, which the app seats in the VM for its own reads
    /// with the host out ([`Self::seat`]).
    fn req(&mut self) -> Option<PlayerReqState> {
        let skinning = self.latches().and_then(|(s, _)| s);
        let store = self.player()?;
        let inputs = ReqInputs {
            proficiencies: self.proficiencies.as_deref()?,
            factions: self.factions.as_deref(),
            reputations: self.reputations.as_deref()?,
            actions: self.actions.as_deref()?,
            spells: self.spells.as_deref(),
            skinning,
            skill_lines: self.skill_lines.as_deref(),
        };
        Some(player_req_state(store, &inputs))
    }

    /// Seat the requirement state, the known spells and our taxi flag in `script`, where the
    /// app's own reads inside a call's apply, with no host to ask, find them: the item use's
    /// requirement gate (`0x5d8e0e`), the bar's equip question (`0x5e1650`) and the cursor
    /// setter's control gate, which reads the flag off our object (`0x494b97`-`0x494ba6`).
    pub(crate) fn seat(&mut self, script: &mut benilla_ui::script::UiScript) {
        script.set_on_taxi(
            self.player()
                .is_some_and(|s| s.0.unit_flags() & crate::player::UNIT_FLAG_TAXI_FLIGHT != 0),
        );
        script.set_player_req_state(self.req().unwrap_or_default());
        if let Some(actions) = self.actions.as_deref() {
            script.set_known_spells(actions.spells.iter().copied().collect());
        }
    }

    fn head(&self) -> Option<ReqHead> {
        let s = &self.player()?.0;
        Some(ReqHead {
            level: s.unit_level().unwrap_or(0),
            class_id: u32::from(s.unit_class().unwrap_or(0)),
            race_id: u32::from(s.unit_race().unwrap_or(0)),
            honor_rank: s.player_honor_rank().unwrap_or(0),
            pvp_medals: s.player_pvp_medals().unwrap_or(0),
        })
    }

    /// The first slot holding `skill_id`, its value plus its permanent bonus (`0x5eaae0` reads
    /// no temporary one).
    fn skill(&self, skill_id: u32) -> Option<Option<u32>> {
        let store = self.player()?;
        Some(
            (0..benilla_protocol::messages::PLAYER_SKILL_SLOTS)
                .filter_map(|i| store.0.player_skill(i))
                .find(|s| s.skill_id != 0 && u32::from(s.skill_id) == skill_id)
                .map(|s| u32::from(s.value).saturating_add_signed(i32::from(s.perm_bonus))),
        )
    }

    /// The rank with `faction`: its base for our race and class plus the standing in its slot.
    fn rep_rank(&self, faction: u32) -> Option<u8> {
        let store = self.player()?;
        let reputations = self.reputations.as_deref()?;
        let Some(info) = self
            .factions
            .as_deref()
            .and_then(|f| f.catalog().reputation_faction(faction))
        else {
            return Some(0);
        };
        let (race, class) = (
            store.0.unit_race().unwrap_or(0),
            store.0.unit_class().unwrap_or(0),
        );
        let standing = usize::try_from(info.rep_index).map_or(0, |i| reputations.standing(i));
        Some(benilla_formats::reputation_rank(
            info.base_for(race, class) + standing,
        ))
    }

    /// The latched spell's `SKINNING` points at our skill in its line, kept above 0.
    fn skinning(&mut self) -> Option<Option<i32>> {
        let latched = self.latches()?.0;
        let store = self.player()?;
        let catalog = self.skill_lines.as_deref().map(|s| &s.catalog);
        Some(
            latched
                .and_then(|id| {
                    let d = self.spells.as_deref()?.catalog.get(id)?;
                    let e = d
                        .effects
                        .iter()
                        .position(|&e| e == benilla_formats::SPELL_EFFECT_SKINNING)?;
                    let skill = crate::spell::spell_skill_value(Some(store), catalog, id);
                    Some(d.effect_points_int(e, skill))
                })
                .filter(|&v| v > 0),
        )
    }

    /// The opener list's spells with their points at the player's skill now, and the entries of
    /// what the player carries as of here (`0x622270`, mode 0), as the walk reads them; `None`
    /// with no active player, where the walk opens everything (`0x5da60b`).
    fn openers(&self) -> Option<Option<benilla_ui::script::LockOpeners>> {
        let actions = self.actions.as_deref()?;
        let Some(store) = self.player() else {
            return Some(None);
        };
        let skill_catalog = self.skill_lines.as_deref().map(|s| &s.catalog);
        let held = super::stack_counts(&store.0, &self.stores.item_objects(&self.objects))
            .into_keys()
            .collect();
        Some(Some(super::lock_openers::lock_openers(
            &actions.openers,
            self.spells.as_deref().map(|s| &s.catalog),
            &|id| crate::spell::spell_skill_value(Some(store), skill_catalog, id),
            held,
        )))
    }
}

impl crate::script_calls::live::LiveFamily for RequirementsLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        // A harness without the player's lists answers none of them.
        if self.actions.is_none() || self.proficiencies.is_none() {
            return false;
        }
        ask.answer(|_: &ReqHeadQuery| Some(self.head()))
            || ask.answer(|q: &ReqSkillQuery| self.skill(q.0))
            || ask.answer(|q: &ProficiencyQuery| {
                self.player()?;
                Some(self.proficiencies.as_deref()?.0.get(&q.0).copied())
            })
            || ask.answer(|q: &RepRankQuery| self.rep_rank(q.0))
            || ask.answer(|_: &DualWieldQuery| {
                self.player()?;
                Some(self.latches()?.1)
            })
            || ask.answer(|_: &SkinningQuery| self.skinning())
            || ask
                .answer(|q: &KnowsSpellQuery| Some(self.actions.as_deref()?.spells.contains(&q.0)))
            || ask.answer(|_: &LockOpenersQuery| self.openers())
    }
}
