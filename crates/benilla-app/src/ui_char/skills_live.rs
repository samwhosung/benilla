//! The skill list's questions at the call, the call host's half of
//! [`benilla_ui::script::SkillLinesNow`] and [`benilla_ui::script::SkillWords`]: the active
//! player's 128 skill slots off its store as of the drain's place, which the list build walks
//! (`0x4d2cb0`, `0x4d2d26`) and `GetSkillLineInfo` reads a row's words from (`0x5ea3f0`,
//! `0x4d3828`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{SkillLinesNow, SkillSlotWords, SkillWords, SkillsState};

/// What the skill getters read as they run.
#[derive(SystemParam)]
pub(crate) struct SkillLive<'w, 's> {
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
}

impl SkillLive<'_, '_> {
    /// The active player's store as of here, `None` while none is held.
    fn player(&self) -> Option<&crate::net::ObjectStore> {
        self.objects.player().and_then(|e| self.stores.get(e).ok())
    }

    /// The lines the list build keeps, in slot order; `None` without the player or the catalog.
    fn lines(&self) -> Option<SkillsState> {
        let store = self.player()?;
        let catalog = &self.skill_lines.as_deref()?.catalog;
        let (race, class) = (
            store.0.unit_race().unwrap_or(0),
            store.0.unit_class().unwrap_or(0),
        );
        let level = store.0.unit_level().unwrap_or(0);
        let entries = (0..PLAYER_SKILL_SLOTS)
            .filter_map(|i| store.0.player_skill(i))
            .filter_map(|slot| super::skills_row(&slot, catalog, race, class, level))
            .collect();
        Some(SkillsState { entries })
    }

    /// The first slot holding `skill_id` (`0x5ea3f0`), its words; `None` without the player.
    fn words(&self, skill_id: u32) -> Option<Option<SkillSlotWords>> {
        let store = self.player()?;
        Some(
            (0..PLAYER_SKILL_SLOTS)
                .filter_map(|i| store.0.player_skill(i))
                .find(|s| u32::from(s.skill_id) == skill_id)
                .map(|s| SkillSlotWords {
                    value: u32::from(s.value),
                    max: u32::from(s.max),
                    temp_bonus: i32::from(s.temp_bonus),
                    perm_bonus: i32::from(s.perm_bonus),
                    step: u32::from(s.step),
                }),
        )
    }
}

impl crate::script_calls::live::LiveFamily for SkillLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &SkillLinesNow| self.lines()) || ask.answer(|q: &SkillWords| self.words(q.0))
    }
}
