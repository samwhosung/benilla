//! The pet getters' questions at the call, the call host's half of
//! [`benilla_ui::script::PetBarFlags`] and its kin: the bar as `SMSG_PET_SPELLS`, `SMSG_PET_MODE`
//! and the presses leave it ([`PetBar`], [`PetAttackLatch`]), and the unit its guid names and the
//! active player, each store as of the drain's place, as the reference reads the cached guid, the
//! slot words and the pet's object inside each binding (`0x4bdc50`, `0x4bcf70`, `0x4be670`,
//! `0x4be500`, the book `0xb6f098`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    PetActionView, PetBarFlags, PetBarFlagsView, PetBookNow, PetBookState, PetMenuNow, PetSlot,
    PetSlots, PetStats, PetStatsNow,
};

use crate::net::ObjectStore;
use crate::target::UNIT_FLAG_POSSESSED;
use crate::ui_action::Spells;

use super::{PetAttackLatch, PetBar};

/// What the pet getters read as they run.
#[derive(SystemParam)]
pub(crate) struct PetLive<'w, 's> {
    bar: Option<Res<'w, PetBar>>,
    latch: Option<Res<'w, PetAttackLatch>>,
    me: Option<Res<'w, crate::net::SelfGuid>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    spells: Option<Res<'w, Spells>>,
    names: Option<Res<'w, crate::names::NameCache>>,
    stat_tables: Option<Res<'w, crate::ui_pet_stats::PetStatTables>>,
    family_tables: Option<Res<'w, crate::ui_pet_stats::PetFamilyTables>>,
    classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
}

impl PetLive<'_, '_> {
    /// The unit `guid` names, as of here; `None` for none held, a zero guid's included.
    fn unit(&self, guid: u64) -> Option<&ObjectStore> {
        (guid != 0)
            .then(|| self.stores.at(guid, self.objects.entity(guid)))
            .flatten()
    }

    /// The active player's store as of here, the object the client's player guid resolves to
    /// (`0x468550`, `0x4bcf89`).
    fn player(&self) -> Option<&ObjectStore> {
        self.unit(self.me.as_deref().and_then(|g| g.0)?)
    }

    /// The bar's guid-level answers: the guid (`0x4bdc20`), the gate `0x4bcf70`, the pickup's
    /// gate (`0x4be1f7`, `0x4be20a`) and the time to the expiry `0xb714a8` off the clock now
    /// (`0x4be614`).
    fn flags(&self, bar: &PetBar) -> PetBarFlagsView {
        let pet = self.unit(bar.spells.pet_guid);
        let me = self.me.as_deref().and_then(|g| g.0);
        let now = std::time::Instant::now();
        #[allow(clippy::cast_possible_truncation)] // a tick difference
        let remaining_ms = bar.expires.map(|t| {
            let ms = |d: std::time::Duration| (d.as_secs_f64() * 1000.0).round() as i64;
            match t.checked_duration_since(now) {
                Some(ahead) => ms(ahead),
                None => -ms(now.duration_since(t)),
            }
        });
        PetBarFlagsView {
            has_bar: bar.has_bar(),
            actions_usable: super::usable::actions_usable(bar, me, self.player(), pet),
            pickup_allowed: pet.is_some_and(|s| s.0.unit_flags() & UNIT_FLAG_POSSESSED == 0),
            remaining_ms,
        }
    }

    /// Slot `index`, 0-based, as `0x4bdc50` paints it: `None` with no guid or past the ten
    /// (`0x4bdc9d`-`0x4bdcbf`). A spell's cooldown is the list's, which `GetPetActionCooldown`
    /// asks itself.
    fn slot(&self, bar: &PetBar, index: usize) -> Option<PetActionView> {
        let entry = *bar.spells.bar.get(index).filter(|_| bar.has_bar())?;
        let pet = self.unit(bar.spells.pet_guid);
        let attacking = self.latch.as_deref().is_some_and(PetAttackLatch::up);
        let display = entry
            .is_spell()
            .then(|| self.spells.as_deref()?.catalog.get(entry.action()))
            .flatten();
        let active = super::active_aura_press(entry, pet, display).is_some();
        Some(super::bar::slot_view(
            entry,
            &bar.spells,
            display,
            None,
            attacking,
            active,
        ))
    }

    /// The ten slots, none with no guid.
    fn slots(&self, bar: &PetBar) -> Vec<PetActionView> {
        (0..bar.spells.bar.len())
            .map_while(|i| self.slot(bar, i))
            .collect()
    }

    /// `HasPetUI`'s first return and the stat block off the pet the guid resolves (`0x4be670`).
    fn stats(&self, bar: &PetBar) -> (bool, PetStats) {
        let pet = self.unit(bar.spells.pet_guid);
        let family = match self.names.as_deref() {
            Some(names) => {
                crate::ui_pet_stats::family_for(pet, names, self.family_tables.as_deref())
            }
            None => (None, None, Vec::new()),
        };
        crate::ui_pet_stats::stats_for(pet, self.player(), self.stat_tables.as_deref(), family)
    }

    /// The menu's predicates: the pet summoned by us alone, no charmed-by leg (`0x4be500`).
    fn menu(&self, bar: &PetBar) -> (bool, bool) {
        let me = self.me.as_deref().and_then(|g| g.0);
        let flags = self
            .unit(bar.spells.pet_guid)
            .filter(|s| s.0.unit_summoned_by() == me)
            .map_or(0, |s| s.0.unit_flags());
        super::menu::menu_predicates(flags)
    }

    /// The pet's book off the bar's raw words, `None` without the catalog.
    fn book(&self, bar: &PetBar) -> Option<PetBookState> {
        let spells = self.spells.as_deref()?;
        // `HasPetSpells`'s token is the player's class (`0x4b4463`), `ChrClasses.dbc` field 4.
        let token = self.player().and_then(|s| {
            let class = u32::from(s.0.unit_class()?);
            self.classes
                .as_deref()
                .map(|t| t.0.pet_name_token(class).to_string())
        });
        let pet = self.unit(bar.spells.pet_guid);
        Some(crate::ui_pet_book::pet_book(
            bar,
            &spells.catalog,
            token,
            pet,
        ))
    }
}

impl crate::script_calls::live::LiveFamily for PetLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        let Some(bar) = self.bar.as_deref() else {
            return false;
        };
        ask.answer(|_: &PetBarFlags| Some(self.flags(bar)))
            || ask.answer(|q: &PetSlot| Some(self.slot(bar, q.0)))
            || ask.answer(|_: &PetSlots| Some(self.slots(bar)))
            || ask.answer(|_: &PetStatsNow| Some(self.stats(bar)))
            || ask.answer(|_: &PetMenuNow| Some(self.menu(bar)))
            || ask.answer(|_: &PetBookNow| self.book(bar))
    }
}
