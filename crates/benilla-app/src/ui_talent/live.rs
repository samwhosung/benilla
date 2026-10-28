//! The talent getters' questions at the call, the call host's half of
//! [`benilla_ui::script::TalentTabs`] and its kin: the class's tabs and talents off the catalog,
//! each rank off the known spells as they stand (`0x4f3200`, `0x60c740`, `0x60c8d0`), and the two point
//! counters off the active player's store as of the drain's place (`+0xe48`, `0x519463`); and the
//! respec question's latch, whose trainer's range `CheckTalentMasterDist` tests at the call
//! (`0x5dfb10`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{CharacterPoints, TalentAt, TalentCount, TalentMasterPending, TalentTabs};

use crate::ui_action::{PlayerActions, Spells};

/// What the talent getters read as they run.
#[derive(SystemParam)]
pub(crate) struct TalentLive<'w, 's> {
    talents: Option<Res<'w, super::Talents>>,
    spells: Option<Res<'w, Spells>>,
    actions: Option<Res<'w, PlayerActions>>,
    /// The ability rows a rank's later ranks are found through (`0x60c8d0`).
    lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    /// The respec question's latch; absent in a world with no trainer flow.
    wipe: Option<Res<'w, crate::ui_talent_wipe::TalentWipeState>>,
    /// The latched trainer's range, which the poll tests on each call.
    reach: crate::ui_dialog_verbs::DialogReach<'w, 's>,
}

/// What a talent answer reads: the catalogs, the known spells, the race and class and the points.
struct Inputs<'a> {
    catalog: &'a benilla_formats::TalentCatalog,
    spells: &'a benilla_formats::SpellCatalog,
    known: super::Known<'a>,
    race: u8,
    class: u8,
    points: (u32, u32),
}

impl TalentLive<'_, '_> {
    /// The active player's store as of here, `None` while none is held.
    fn player(&self) -> Option<&crate::net::ObjectStore> {
        self.objects.player().and_then(|e| self.stores.get(e).ok())
    }

    /// The counters as of here, `(0, 0)` with no player held (`0x5194da`).
    fn points(&self) -> (u32, u32) {
        self.player().map_or((0, 0), |s| {
            (
                s.0.player_talent_points().unwrap_or(0),
                s.0.player_free_professions().unwrap_or(0),
            )
        })
    }

    /// Everything an answer reads, `None` without the catalogs or a player, where the snapshot
    /// answers.
    fn inputs(&self) -> Option<Inputs<'_>> {
        let store = self.player()?;
        Some(Inputs {
            catalog: &self.talents.as_deref()?.catalog,
            spells: &self.spells.as_deref()?.catalog,
            known: super::Known {
                spells: &self.actions.as_deref()?.spells,
                lines: self.lines.as_deref().map(|l| &l.catalog),
            },
            race: store.0.unit_race().unwrap_or(0),
            class: store.0.unit_class().unwrap_or(0),
            points: self.points(),
        })
    }
}

impl Inputs<'_> {
    fn tabs(&self) -> Vec<&benilla_formats::TalentTabInfo> {
        self.catalog.tabs_for_class(self.race, self.class)
    }
}

impl crate::script_calls::live::LiveFamily for TalentLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &TalentTabs| {
            let i = self.inputs()?;
            Some(
                i.tabs()
                    .into_iter()
                    .map(|tab| super::tab_view(tab, super::tab_spent(i.catalog, tab.id, i.known)))
                    .collect(),
            )
        }) || ask.answer(|q: &TalentCount| {
            let i = self.inputs()?;
            Some(
                i.tabs()
                    .get(q.0)
                    .map_or(0, |tab| i.catalog.talents_in_tab(tab.id).len()),
            )
        }) || ask.answer(|q: &TalentAt| {
            let i = self.inputs()?;
            let tabs = i.tabs();
            let Some(&tab) = tabs.get(q.0) else {
                return Some(None);
            };
            let Some(t) = i.catalog.talents_in_tab(tab.id).get(q.1) else {
                return Some(None);
            };
            let spent = super::tab_spent(i.catalog, tab.id, i.known);
            Some(Some(super::talent_view(
                i.catalog, tab, t, spent, i.known, i.spells, i.points,
            )))
        }) || ask.answer(|_: &CharacterPoints| Some(self.points()))
            || ask.answer(|_: &TalentMasterPending| {
                let npc = self.wipe.as_ref().and_then(|w| w.pending());
                Some(npc.is_some_and(|npc| self.reach.polled(npc)))
            })
    }
}
