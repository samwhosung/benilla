//! The dialogs' getters' reads at the call, the call host's half of
//! [`benilla_ui::script::BinderPendingQuery`] and its kin: the service latches (`0xc4d7c0`,
//! `0xc4d7b0`) with their NPC's range tested at the call (`0x5dff50`, `0x5dfd30`), the boot and spirit-healer deadlines less now (`0x496380`, `[0xb4e338]`), the
//! meeting stone's two globals (`0xb72038`, `0xb7203c`), the summon latch read off the name cache,
//! `AreaTable.dbc` and the clock (`0x48b6a0`, `0x48b720`, `0x4963e0`), and the instance terms off
//! `Map.dbc` and the map the world stands on (`0x48a750`, `0x495c90`), each as the handlers left
//! it.

use std::time::Instant;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    AreaSpiritHealerSecsQuery, BinderPendingQuery, CanResetInstancesQuery, InstanceBootSecsQuery,
    InstanceTypeQuery, MeetingStoneQuery, PetUntrainerPendingQuery, SummonConfirmQuery,
};

use super::{AreaSpiritHealer, InstanceBoot, MeetingStone, PetUnlearnState};
use crate::area::AreaTableRes;

/// What the dialogs' getters read as they run; each absent in a harness without it, where the
/// getters read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct DialogLive<'w, 's> {
    binder: Option<Res<'w, crate::ui_binder::BinderState>>,
    pet: Option<Res<'w, PetUnlearnState>>,
    /// The latched NPC's range, which the polls test on each call.
    reach: super::DialogReach<'w, 's>,
    boot: Option<Res<'w, InstanceBoot>>,
    spirit: Option<Res<'w, AreaSpiritHealer>>,
    stone: Option<Res<'w, MeetingStone>>,
    areas: Option<Res<'w, AreaTableRes>>,
    summon: Option<Res<'w, crate::ui_summon::SummonState>>,
    names: Option<Res<'w, crate::names::NameCache>>,
    /// The real clock the summon deadline and the reset window run on.
    real: Option<Res<'w, Time<Real>>>,
    instance: Option<Res<'w, crate::ui_instance::InstanceState>>,
    maps: Option<Res<'w, benilla_assets::MapCatalogRes>>,
    map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
}

impl crate::script_calls::live::LiveFamily for DialogLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        let areas = self.areas.as_deref();
        let reach = &self.reach;
        ask.answer(|_: &BinderPendingQuery| {
            Some(
                self.binder
                    .as_ref()?
                    .pending()
                    .is_some_and(|npc| reach.polled(npc)),
            )
        }) || ask.answer(|_: &PetUntrainerPendingQuery| {
            Some(
                self.pet
                    .as_ref()?
                    .pending()
                    .is_some_and(|npc| reach.polled(npc)),
            )
        }) || ask.answer(|_: &InstanceBootSecsQuery| Some(self.boot.as_ref()?.secs(Instant::now())))
            || ask.answer(|_: &AreaSpiritHealerSecsQuery| {
                Some(self.spirit.as_ref()?.secs(Instant::now()))
            })
            || ask.answer(|_: &MeetingStoneQuery| Some(self.stone.as_ref()?.status(areas)))
            || ask.answer(|_: &SummonConfirmQuery| {
                let now = self.real.as_ref()?.elapsed_secs_f64();
                Some(
                    self.summon
                        .as_ref()?
                        .confirm(self.names.as_deref()?, areas, now),
                )
            })
            || ask.answer(|_: &InstanceTypeQuery| {
                let maps = self.maps.as_deref()?;
                Some(self.map.as_ref().and_then(|m| maps.0.instance_type(m.0)))
            })
            || ask.answer(|_: &CanResetInstancesQuery| {
                let (instance, maps) = (self.instance.as_ref()?, self.maps.as_deref());
                let now = self.real.as_ref()?.elapsed().as_secs();
                let party_dungeon = |m: u32| maps.is_some_and(|c| c.0.is_party_dungeon(m));
                Some(instance.can_reset(&party_dungeon, now))
            })
    }
}
