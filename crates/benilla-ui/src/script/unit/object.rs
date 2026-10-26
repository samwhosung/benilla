//! What a unit's own descriptor answers, which the getters read off the object as the call runs.
//! The reference's getters look the object up and read its fields there (`UnitHealth` resolves
//! its token, then reads `[obj+0x110]+0x40`, `0x5174d0`/`0x517553`), so a getter that the lent host
//! can answer reads the field it answers off the object now, not the frame's snapshot
//! ([`super::with_object`]).

use super::UnitState;
use crate::script::UnitCombatStats;

/// [`UnitObjectRead`], [`UnitObject`] and the moves between them and a [`UnitState`], from one
/// list of fields.
macro_rules! unit_object {
    ($($field:ident: $ty:ty),* $(,)?) => {
        /// The fields of a [`UnitState`] that its object's descriptor alone answers, with no table,
        /// cache, roster or other unit read, each read on its own: off the held object through the
        /// lent host ([`crate::script::CallHost::read_unit_object`]), or off a snapshot, which
        /// holds them as its build read them. The app's one derivation of each from a descriptor
        /// is its implementation of this.
        ///
        /// The getters that read one of these off the object as they run: `UnitHealth`,
        /// `UnitHealthMax`, `UnitMana`, `UnitManaMax`, `UnitPowerType`, `UnitLevel`, `UnitIsDead`,
        /// `UnitIsGhost`, `UnitIsDeadOrGhost`, `UnitIsTapped`, `UnitIsTappedByPlayer`,
        /// `UnitIsCharmed`, `UnitIsTrivial`, `UnitIsPlusMob`, `UnitPlayerControlled`,
        /// `UnitAffectingCombat`, `UnitIsPartyLeader`, `UnitIsCivilian`, `UnitPlayerOrPetInParty`,
        /// `UnitPlayerOrPetInRaid` and `UnitSex`; the stat getters read the level and
        /// SUMMONEDBY, and the stats themselves through [`crate::script::CombatRead`]. The rest of
        /// these fields (the quest log, the guild id, the PvP rank and medal, the raw flags
        /// dwords) are read off the snapshot by their getters, until those read the object too.
        pub trait UnitObjectRead {
            $(
                #[doc = concat!("[`UnitState::", stringify!($field), "`].")]
                fn $field(&self) -> $ty;
            )*
        }

        /// Every [`UnitObjectRead`] field at once: what a snapshot's build takes from its object.
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct UnitObject {
            $(
                #[doc = concat!("[`UnitState::", stringify!($field), "`].")]
                pub $field: $ty,
            )*
        }

        impl UnitObject {
            /// Every field `object` answers.
            pub fn read(object: &dyn UnitObjectRead) -> Self {
                UnitObject { $($field: object.$field(),)* }
            }
        }

        // A snapshot hands each field on as it holds it; most are `Copy`, the stats a plain clone.
        #[allow(clippy::clone_on_copy)]
        impl UnitObjectRead for UnitState {
            $(
                fn $field(&self) -> $ty {
                    self.$field.clone()
                }
            )*
        }

        impl UnitState {
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

/// What names a held unit as its getters read it, past its descriptor's own fields: its name off
/// the name caches (`UnitName` looks the object up at `0x51710c` and names a unit through
/// `GetUnitName 0x609210`, `0x517131`) and its race and class bytes through `ChrRaces.dbc` and
/// `ChrClasses.dbc` (`UnitRace 0x518200`, `UnitClass 0x518350`). Off the held object through the
/// lent host ([`crate::script::CallHost::read_unit_identity`]), or off a snapshot, which holds them
/// as its build read them.
pub trait UnitIdentityRead {
    /// [`UnitState::name`].
    fn name(&self) -> Option<String>;
    /// [`UnitState::race`] and [`UnitState::race_file`].
    fn race(&self) -> Option<(String, String)>;
    /// [`UnitState::class`] and [`UnitState::class_file`].
    fn class(&self) -> Option<(String, String)>;
    /// [`UnitState::family`].
    fn family(&self) -> Option<String>;
}

impl UnitIdentityRead for UnitState {
    fn name(&self) -> Option<String> {
        self.name.clone()
    }
    fn race(&self) -> Option<(String, String)> {
        self.race.clone().zip(self.race_file.clone())
    }
    fn class(&self) -> Option<(String, String)> {
        self.class.clone().zip(self.class_file.clone())
    }
    fn family(&self) -> Option<String> {
        self.family.clone()
    }
}
