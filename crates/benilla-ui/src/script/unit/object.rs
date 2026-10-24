//! What a unit's own descriptor answers, which the getters read off the object as the call runs.
//! The reference's getters look the object up and read its fields there (`UnitHealth` resolves
//! its token, then reads `[obj+0x110]+0x40`, `0x5174d0`/`0x517553`), so a getter that the lent host
//! can answer reads the object now, not the frame's snapshot ([`super::read_unit`]).

use super::UnitState;
use crate::script::UnitCombatStats;

/// [`UnitObject`] and the two moves between it and a [`UnitState`], from one list of fields.
macro_rules! unit_object {
    ($($field:ident: $ty:ty),* $(,)?) => {
        /// The fields of a [`UnitState`] that its object's descriptor alone answers, with no table,
        /// cache, roster or other unit read: what a getter reads off the held object, as of the
        /// place it runs, whenever a host is lent ([`crate::script::CallHost::unit_object`]).
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct UnitObject {
            $(
                #[doc = concat!("[`UnitState::", stringify!($field), "`].")]
                pub $field: $ty,
            )*
        }

        impl UnitState {
            /// The fields its object answers, as this snapshot holds them.
            pub fn object(&self) -> UnitObject {
                UnitObject { $($field: self.$field.clone(),)* }
            }

            /// Take every field its object answers from `object`.
            pub fn set_object(&mut self, object: UnitObject) {
                $(self.$field = object.$field;)*
            }
        }
    };
}

unit_object! {
    health: u32,
    max_health: u32,
    level: u32,
    power_type: u8,
    power: u32,
    max_power: u32,
    dead: bool,
    ghost: bool,
    charmed: bool,
    tapped: bool,
    tapped_by_player: bool,
    group_leader: bool,
    player_flags: u32,
    flags: u32,
    dynamic_flags: u32,
    owner: u64,
    pvp: bool,
    skinnable: bool,
    player_controlled: bool,
    in_combat: bool,
    pvp_rank: u8,
    pvp_medal: u8,
    quest_log: [[u32; 3]; 20],
    guild_id: u32,
    drunk: u8,
    sex: u8,
    combat: UnitCombatStats,
    summoned_by: u64,
}
