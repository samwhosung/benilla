//! Parameter sets whose members may conflict with one another, each member's access gathered once.
//! Bevy's `ParamSet` gathers each member's access twice, the first time on a clone of the access
//! the system has built so far, so a set nested `d` deep gathers its leaves `2^d` times; the call
//! host's appliers are one flat set of these ([`disjoint_set!`]), and a system that holds the host
//! beside state of its own pairs them in a [`Pair`].

use bevy::ecs::change_detection::Tick;
use bevy::ecs::query::FilteredAccessSet;
use bevy::ecs::system::{SystemMeta, SystemParam, SystemParamItem, SystemParamValidationError};
use bevy::ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy::ecs::world::{DeferredWorld, World};

/// Gather the access of `P`, one member of a set, alone, check it against what the system holds
/// outside the set, as `ParamSet` does, and add it to `gathered`. Members may conflict with one
/// another: the set lends one at a time.
pub(crate) fn gather<P: SystemParam>(
    state: &P::State,
    meta: &mut SystemMeta,
    outside: &FilteredAccessSet,
    gathered: &mut FilteredAccessSet,
    world: &mut World,
    member: &str,
) {
    let mut alone = FilteredAccessSet::new();
    P::init_access(state, meta, &mut alone, world);
    assert!(
        outside.is_compatible(&alone),
        "{member} in {} conflicts with a parameter outside its set: {:?}",
        meta.name(),
        outside.get_conflicts(&alone),
    );
    gathered.extend(alone);
}

/// A set of named members that may conflict with one another, lent one at a time through a
/// method of the member's name, each member's access gathered once ([`gather`]). Each member's
/// type is written with `'static` lifetimes; its method lends it for the borrow of the set.
macro_rules! disjoint_set {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident, $state:ident {
            $($(#[$fmeta:meta])* $field:ident: $ty:ty,)*
        }
    ) => {
        $(#[$meta])*
        $vis struct $name<'w, 's> {
            states: &'s mut $state,
            meta: bevy::ecs::system::SystemMeta,
            world: bevy::ecs::world::unsafe_world_cell::UnsafeWorldCell<'w>,
            change_tick: bevy::ecs::change_detection::Tick,
        }

        #[doc(hidden)]
        $vis struct $state {
            $($field: <$ty as bevy::ecs::system::SystemParam>::State,)*
        }

        // SAFETY: the set registers every member's access, each checked against what the system
        // holds outside the set ([`crate::script_calls::disjoint::gather`]), and lends one member
        // at a time, through `&mut self`.
        unsafe impl bevy::ecs::system::SystemParam for $name<'_, '_> {
            type State = $state;
            type Item<'w, 's> = $name<'w, 's>;

            fn init_state(world: &mut bevy::ecs::world::World) -> Self::State {
                $state {
                    $($field: <$ty as bevy::ecs::system::SystemParam>::init_state(world),)*
                }
            }

            fn init_access(
                state: &Self::State,
                meta: &mut bevy::ecs::system::SystemMeta,
                access: &mut bevy::ecs::query::FilteredAccessSet,
                world: &mut bevy::ecs::world::World,
            ) {
                let mut gathered = bevy::ecs::query::FilteredAccessSet::new();
                $(
                    crate::script_calls::disjoint::gather::<$ty>(
                        &state.$field,
                        meta,
                        access,
                        &mut gathered,
                        world,
                        stringify!($field),
                    );
                )*
                access.extend(gathered);
            }

            fn apply(
                state: &mut Self::State,
                meta: &bevy::ecs::system::SystemMeta,
                world: &mut bevy::ecs::world::World,
            ) {
                $(<$ty as bevy::ecs::system::SystemParam>::apply(&mut state.$field, meta, world);)*
            }

            fn queue(
                state: &mut Self::State,
                meta: &bevy::ecs::system::SystemMeta,
                mut world: bevy::ecs::world::DeferredWorld,
            ) {
                $(<$ty as bevy::ecs::system::SystemParam>::queue(
                    &mut state.$field,
                    meta,
                    world.reborrow(),
                );)*
            }

            unsafe fn validate_param(
                state: &mut Self::State,
                meta: &bevy::ecs::system::SystemMeta,
                world: bevy::ecs::world::unsafe_world_cell::UnsafeWorldCell,
            ) -> Result<(), bevy::ecs::system::SystemParamValidationError> {
                // SAFETY: upheld by the caller, for every member's access as for the set's.
                unsafe {
                    $(<$ty as bevy::ecs::system::SystemParam>::validate_param(
                        &mut state.$field,
                        meta,
                        world,
                    )?;)*
                }
                Ok(())
            }

            unsafe fn get_param<'w, 's>(
                state: &'s mut Self::State,
                meta: &bevy::ecs::system::SystemMeta,
                world: bevy::ecs::world::unsafe_world_cell::UnsafeWorldCell<'w>,
                change_tick: bevy::ecs::change_detection::Tick,
            ) -> Self::Item<'w, 's> {
                $name {
                    states: state,
                    meta: meta.clone(),
                    world,
                    change_tick,
                }
            }
        }

        impl $name<'_, '_> {
            $(
                $(#[$fmeta])*
                pub(crate) fn $field(
                    &mut self,
                ) -> bevy::ecs::system::SystemParamItem<'_, '_, $ty> {
                    // SAFETY: the set registered this member's access, and `&mut self` keeps
                    // every other member of the set unlent while this one lives.
                    unsafe {
                        <$ty as bevy::ecs::system::SystemParam>::get_param(
                            &mut self.states.$field,
                            &self.meta,
                            self.world,
                            self.change_tick,
                        )
                    }
                }
            )*
        }
    };
}
pub(crate) use disjoint_set;

/// Two params that may conflict, lent one at a time, as a two-member `ParamSet` with each
/// member's access gathered once ([`gather`]).
pub(crate) struct Pair<'w, 's, A: SystemParam, B: SystemParam> {
    states: &'s mut (A::State, B::State),
    meta: SystemMeta,
    world: UnsafeWorldCell<'w>,
    change_tick: Tick,
}

// SAFETY: as [`disjoint_set!`]'s: both members' access registered and checked against what the
// system holds outside the pair, one member lent at a time.
unsafe impl<A: SystemParam, B: SystemParam> SystemParam for Pair<'_, '_, A, B> {
    type State = (A::State, B::State);
    type Item<'w, 's> = Pair<'w, 's, A, B>;

    fn init_state(world: &mut World) -> Self::State {
        (A::init_state(world), B::init_state(world))
    }

    fn init_access(
        state: &Self::State,
        meta: &mut SystemMeta,
        access: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        let mut gathered = FilteredAccessSet::new();
        gather::<A>(&state.0, meta, access, &mut gathered, world, "p0");
        gather::<B>(&state.1, meta, access, &mut gathered, world, "p1");
        access.extend(gathered);
    }

    fn apply(state: &mut Self::State, meta: &SystemMeta, world: &mut World) {
        A::apply(&mut state.0, meta, world);
        B::apply(&mut state.1, meta, world);
    }

    fn queue(state: &mut Self::State, meta: &SystemMeta, mut world: DeferredWorld) {
        A::queue(&mut state.0, meta, world.reborrow());
        B::queue(&mut state.1, meta, world.reborrow());
    }

    unsafe fn validate_param(
        state: &mut Self::State,
        meta: &SystemMeta,
        world: UnsafeWorldCell,
    ) -> Result<(), SystemParamValidationError> {
        // SAFETY: upheld by the caller, for each member's access as for the pair's.
        unsafe {
            A::validate_param(&mut state.0, meta, world)?;
            B::validate_param(&mut state.1, meta, world)
        }
    }

    unsafe fn get_param<'w, 's>(
        state: &'s mut Self::State,
        meta: &SystemMeta,
        world: UnsafeWorldCell<'w>,
        change_tick: Tick,
    ) -> Self::Item<'w, 's> {
        Pair {
            states: state,
            meta: meta.clone(),
            world,
            change_tick,
        }
    }
}

impl<A: SystemParam, B: SystemParam> Pair<'_, '_, A, B> {
    /// The first member, the second unlent while it lives.
    pub(crate) fn p0(&mut self) -> SystemParamItem<'_, '_, A> {
        // SAFETY: the pair registered `A`'s access, and `&mut self` keeps `B` unlent.
        unsafe { A::get_param(&mut self.states.0, &self.meta, self.world, self.change_tick) }
    }

    /// The second member, the first unlent while it lives.
    pub(crate) fn p1(&mut self) -> SystemParamItem<'_, '_, B> {
        // SAFETY: the pair registered `B`'s access, and `&mut self` keeps `A` unlent.
        unsafe { B::get_param(&mut self.states.1, &self.meta, self.world, self.change_tick) }
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::Pair;

    #[derive(Resource, Default)]
    struct Cell(u32);

    /// The members of a pair may conflict with one another, as the pair lends one at a time.
    #[test]
    fn a_pairs_members_may_conflict_with_each_other() {
        let mut app = App::new();
        app.init_resource::<Cell>().add_systems(
            Update,
            |mut pair: Pair<ResMut<'static, Cell>, Res<'static, Cell>>| {
                pair.p0().0 += 1;
                assert_eq!(pair.p1().0, 1);
            },
        );
        app.update();
    }

    /// A member that conflicts with a parameter outside its set is refused as the system's state
    /// is built, as `ParamSet` refuses it.
    #[test]
    #[should_panic(expected = "conflicts with a parameter outside its set")]
    fn a_member_conflicting_with_a_parameter_outside_the_set_panics() {
        let mut app = App::new();
        app.init_resource::<Cell>().add_systems(
            Update,
            |_outside: ResMut<Cell>, _pair: Pair<Res<'static, Cell>, Res<'static, Cell>>| {},
        );
        app.update();
    }
}
