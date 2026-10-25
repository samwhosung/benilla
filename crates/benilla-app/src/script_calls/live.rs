//! What the getters read as they run, through the call host: every family of live reads, in one
//! set ([`LiveReads`]). The reference's getters read the live object and caches at the call
//! (`UnitHealth`, `0x5174d0`), so a listener reads the state its own packet left.
//!
//! A family that answers a question of its own type ([`benilla_ui::script::live::LiveQuery`])
//! implements [`LiveFamily`] in its own file and adds one line to [`LiveReads`]: its member.
//! The host's trait and its appliers stay as they are. The units, the items, the stats and the
//! auras also hand borrowed views through methods of the host's trait of their own.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;

/// A family of the getters' live reads: what it answers of their questions.
pub(crate) trait LiveFamily {
    /// Answer `ask` if it is one of this family's questions ([`LiveAsk::answer`]); whether it was.
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        let _ = ask;
        false
    }
}

/// A family absent in a harness without its inputs answers nothing, so its getters read their
/// snapshots.
impl<T: LiveFamily> LiveFamily for Option<T> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        self.as_mut().is_some_and(|family| family.answer(ask))
    }
}

/// [`super::disjoint::disjoint_set!`] of live families, with the walk that hands a question to
/// each member in turn until one takes it.
macro_rules! live_reads {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident, $state:ident {
            $($(#[doc = $doc:literal])* $(#[cfg($cfg:meta)])? $field:ident: $ty:ty,)*
        }
    ) => {
        super::disjoint::disjoint_set! {
            $(#[$meta])*
            $vis struct $name, $state {
                $($(#[doc = $doc])* $(#[cfg($cfg)])? $field: $ty,)*
            }
        }

        impl $name<'_, '_> {
            /// Hand `ask` to each family until one takes it; whether one did.
            pub(crate) fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
                $(
                    $(#[cfg($cfg)])?
                    if LiveFamily::answer(&mut self.$field(), ask) {
                        return true;
                    }
                )*
                false
            }
        }
    };
}

live_reads! {
    /// Every family of the getters' live reads, each lent alone.
    pub(crate) struct LiveReads, LiveReadsState {
        /// The unit objects, the items, the stats and the auras; the auras' absent in a harness
        /// without the relations' inputs.
        units: UnitLive<'static, 'static>,
        items: crate::ui_items::live::ItemLive<'static, 'static>,
        stats: StatsLive<'static, 'static>,
        auras: Option<crate::ui_aura::AuraLive<'static, 'static>>,
        /// The player's cooldown lists.
        cooldowns: crate::spell::CooldownLive<'static, 'static>,
        /// The guild roster, its ranks and the guild record.
        guild: crate::ui_guild::GuildLive<'static, 'static>,
        #[cfg(test)]
        probe: tests::ProbeLive<'static>,
    }
}

impl LiveFamily for UnitLive<'_, '_> {}
impl LiveFamily for crate::ui_items::live::ItemLive<'_, '_> {}
impl LiveFamily for crate::ui_aura::AuraLive<'_, '_> {}

impl LiveFamily for StatsLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &benilla_ui::script::WearsItem| Some(self.wears(q.0)))
    }
}

/// The unit objects the `Unit*` getters read off as they run, as the reference's read the object
/// they resolve (`UnitHealth`, `0x5174d0`): by guid through the object manager (`0x468460`), each
/// store as of the drain's place, so a listener reads its own packet's.
#[derive(SystemParam)]
pub(crate) struct UnitLive<'w, 's> {
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    me: Query<'w, 's, (Entity, &'static crate::net::Guid), With<crate::net::SelfPlayer>>,
    /// The name caches and the race and class tables a held unit is named through.
    names: Option<Res<'w, crate::names::NameCache>>,
    commands: Option<Res<'w, crate::net::NetCommands>>,
    tables: crate::ui_unit::NameTables<'w>,
}

impl UnitLive<'_, '_> {
    /// Hand `read` what names the object held under `guid`, as of here; whether one was held as
    /// a unit (`0x51710c`, then `0x609210` for a unit).
    pub(crate) fn read_identity(
        &self,
        guid: u64,
        read: &mut dyn FnMut(&dyn benilla_ui::script::UnitIdentityRead),
    ) -> bool {
        let entity = self.index.as_deref().and_then(|i| i.0.get(&guid).copied());
        let Some(store) = self.stores.at(guid, entity).filter(|s| s.is_unit()) else {
            return false;
        };
        read(&crate::ui_unit::LiveIdentity {
            guid,
            store,
            names: self.names.as_deref(),
            commands: self.commands.as_deref(),
            tables: self.tables.get(),
        });
        true
    }

    /// Hand `read` what the unit-token resolver reads off the objects, as of here (`0x515970`).
    pub(crate) fn read_tokens(
        &self,
        read: &mut dyn FnMut(&dyn benilla_ui::script::UnitTokenObjects),
    ) {
        read(&crate::ui_unit::TokenObjects {
            index: self.index.as_deref(),
            stores: &self.stores,
            me: self.me.iter().next().map(|(e, g)| (e, g.0)),
        });
    }

    /// Hand `read` the object held under `guid`, as of here; whether one was held.
    pub(crate) fn read(
        &self,
        guid: u64,
        read: &mut dyn FnMut(&dyn benilla_ui::script::UnitObjectRead),
    ) -> bool {
        // As of here, one a later packet destroys included.
        let entity = self.index.as_deref().and_then(|i| i.0.get(&guid).copied());
        let Some(store) = self.stores.at(guid, entity) else {
            return false;
        };
        read(&crate::ui_unit::StoreObject(store));
        true
    }
}

/// The unit objects the stat getters and the active player's getters read off as they run, as
/// the reference's read the object they resolve (`UnitStat`, `0x518600`) and the active player's
/// fields (`GetMoney`, `0x518160`): each store as of the drain's place, so a listener reads its
/// own packet's, and the active player's worn weapons' records, read without asking.
#[derive(SystemParam)]
pub(crate) struct StatsLive<'w, 's> {
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    items: Res<'w, crate::items::Items>,
}

impl StatsLive<'_, '_> {
    /// Hand `read` the object held under `guid` and its stats, as of here, the active player's
    /// with its weapons; whether one was held.
    pub(crate) fn read(
        &self,
        guid: u64,
        read: &mut dyn FnMut(
            &dyn benilla_ui::script::UnitObjectRead,
            &dyn benilla_ui::script::CombatRead,
        ),
    ) -> bool {
        let Some(entity) = self.objects.entity(guid) else {
            return false;
        };
        let Ok(store) = self.stores.get(entity) else {
            return false;
        };
        let entry = |guid| self.item_entry(guid);
        let stats = if self.objects.player() == Some(entity) {
            crate::ui_char::StatsObject::player(
                store,
                crate::ui_char::Worn::new(store, &entry, &self.items),
            )
        } else {
            crate::ui_char::StatsObject::unit(store)
        };
        read(&crate::ui_unit::StoreObject(store), &stats);
        true
    }

    /// An item's entry as of here, a worn weapon a later packet destroys included.
    fn item_entry(&self, guid: u64) -> Option<u32> {
        self.stores
            .by_guid(guid, self.objects.store(guid))?
            .0
            .object_entry()
    }

    /// The active player's store as of here, `None` while none is held.
    fn player(&self) -> Option<&crate::net::ObjectStore> {
        self.objects.player().and_then(|e| self.stores.get(e).ok())
    }

    /// Hand `read` the active player's stats, as of here, `None` while none is held.
    pub(crate) fn read_own(
        &self,
        read: &mut dyn FnMut(Option<&dyn benilla_ui::script::CombatRead>),
    ) {
        let Some(store) = self.player() else {
            return read(None);
        };
        let entry = |guid| self.item_entry(guid);
        read(Some(&crate::ui_char::StatsObject::player(
            store,
            crate::ui_char::Worn::new(store, &entry, &self.items),
        )));
    }

    /// Hand `read` the active player's own fields as of here, [`benilla_ui::script::NoPlayer`]'s
    /// while none is held.
    pub(crate) fn read_self(&self, read: &mut dyn FnMut(&dyn benilla_ui::script::SelfRead)) {
        match self.player() {
            Some(store) => read(&crate::ui_unit::StoreObject(store)),
            None => read(&benilla_ui::script::NoPlayer),
        }
    }

    /// Whether the active player's worn slots, as of here, hold a whole item of `entry`, each item
    /// found as of here; `false` while none is held.
    fn wears(&self, entry: u32) -> bool {
        let items = self.stores.item_objects(&self.objects);
        self.player()
            .is_some_and(|store| crate::ui_action::wears_entry(store, &items, entry))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use bevy::ecs::system::SystemState;
    use bevy::prelude::*;

    use benilla_ui::script::live::{LiveAsk, LiveQuery};

    use super::LiveFamily;
    use crate::net::SelfGuid;

    /// The probe's one question: our guid, state a test writes and no getter reads.
    struct ProbeGuid;

    impl LiveQuery for ProbeGuid {
        type Answer = u64;
    }

    /// The probe family: [`SelfGuid`] as of each ask, declined while none is known.
    #[derive(bevy::ecs::system::SystemParam)]
    pub(crate) struct ProbeLive<'w> {
        guid: Option<Res<'w, SelfGuid>>,
    }

    impl LiveFamily for ProbeLive<'_> {
        fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
            ask.answer(|_: &ProbeGuid| self.guid.as_ref().and_then(|g| g.0))
        }
    }

    /// A getter reads, through the seam, the state written ahead of it in the same turn: each
    /// write between two chunks shows in the next, with no push between, and with no host lent
    /// the getter has nothing to read.
    #[test]
    fn a_getter_reads_through_the_seam_what_its_turn_wrote() {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let world = app.world_mut();
        crate::script_calls::insert_test_vm(world);
        let lua = crate::script_calls::with_hosted_vm(world, |s| s.expect("a VM").lua().clone());
        let get = lua
            .create_function(|lua, ()| Ok(benilla_ui::script::live::ask(lua, &ProbeGuid)))
            .unwrap();
        lua.globals().set("ProbeGet", get).unwrap();
        let mut state = SystemState::<crate::script_calls::VmWith<ResMut<SelfGuid>>>::new(world);
        let mut vm = state.get_mut(world);
        let read = |vm: &mut crate::script_calls::VmWith<ResMut<SelfGuid>>| {
            vm.run(|s| {
                s.expect("a VM")
                    .eval::<Option<u64>>("return ProbeGet()")
                    .unwrap()
            })
        };
        vm.own().0 = Some(7);
        assert_eq!(read(&mut vm), Some(7));
        vm.own().0 = Some(9);
        assert_eq!(
            read(&mut vm),
            Some(9),
            "the second write, with no push between"
        );
        state.apply(world);
        let unlent: Option<u64> = crate::script_calls::eval_in_vm(world, "return ProbeGet()");
        assert_eq!(unlent, None, "no host lent, no live read");
    }
}
