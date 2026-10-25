//! What a unit's combat stats and the active player's own fields answer, which the stat getters
//! and the player getters read off the object as the call runs. The reference's stat getters
//! resolve their unit (`0x515940`) and read its descriptor there (`UnitStat` `0x518600`, the
//! unit block at `[unit+0x110]`, the player block at `[unit+0xe68]`), and `GetMoney`, `UnitXP`
//! and the rest-state getters read the active player's (`0x518160`, `0x517350`, `0x48d350`), so
//! a getter the lent host can answer reads the field now, not the frame's snapshot
//! ([`read_unit_stats`], [`read_self`]).

use mlua::Lua;

use super::{Model, UnitObjectRead};

/// [`UnitCombatStats`] and [`CombatRead`], and the moves between them, from one list of fields:
/// the per-school and per-stat arrays, which the trait reads one entry at a time, then the rest.
macro_rules! combat_stats {
    (
        arrays { $($(#[$adoc:meta])* $arr:ident: [$aty:ty; $n:literal],)* }
        scalars { $($(#[$doc:meta])* $field:ident: $ty:ty,)* }
    ) => {
        /// One unit's combat stats behind a paper doll, from its descriptor. Arrays are in field
        /// order: stats Str/Agi/Sta/Int/Spi, schools with `[0]` armor. A unit other than the active
        /// player leaves the fields its equipment and skills answer (and the avoidance chances) at
        /// their defaults, and a pet the PLAYER-block fields too.
        #[derive(Clone, Debug, PartialEq)]
        pub struct UnitCombatStats {
            $($(#[$adoc])* pub $arr: [$aty; $n],)*
            $($(#[$doc])* pub $field: $ty,)*
        }

        /// What a unit's combat stats answer, each field read on its own, and an array one entry
        /// at a time: off the held object through the lent host
        /// ([`crate::script::CallHost::read_unit_stats`]), or off a snapshot, which holds them as
        /// its build read them. The app's one derivation of each from a descriptor is its
        /// implementation of this, which its snapshots' builds read too
        /// ([`UnitCombatStats::read`]).
        pub trait CombatRead {
            $(
                #[doc = concat!("Entry `i`, in range, of [`UnitCombatStats::", stringify!($arr), "`].")]
                fn $arr(&self, i: usize) -> $aty;
            )*
            $(
                #[doc = concat!("[`UnitCombatStats::", stringify!($field), "`].")]
                fn $field(&self) -> $ty;
            )*
        }

        impl CombatRead for UnitCombatStats {
            $(
                fn $arr(&self, i: usize) -> $aty {
                    self.$arr[i]
                }
            )*
            $(
                fn $field(&self) -> $ty {
                    self.$field
                }
            )*
        }

        impl UnitCombatStats {
            /// Every field `stats` answers.
            pub fn read(stats: &dyn CombatRead) -> Self {
                UnitCombatStats {
                    $($arr: std::array::from_fn(|i| stats.$arr(i)),)*
                    $($field: stats.$field(),)*
                }
            }
        }
    };
}

combat_stats! {
    arrays {
        /// Effective (post-buff) primary stats (`UNIT_FIELD_STAT0..4`).
        stats: [i32; 5],
        /// Positive stat buff deltas (`PLAYER_FIELD_POSSTAT0..4`, ≥ 0).
        stat_pos: [i32; 5],
        /// Negative stat buff deltas (`PLAYER_FIELD_NEGSTAT0..4`, ≤ 0).
        stat_neg: [i32; 5],
        /// Effective resistances (`UNIT_FIELD_RESISTANCES`, `[0]` = armor).
        resistances: [i32; 7],
        /// Positive resistance buffs (`PLAYER_FIELD_RESISTANCEBUFFMODSPOSITIVE`, ≥ 0).
        resistance_pos: [i32; 7],
        /// Negative resistance buffs (`PLAYER_FIELD_RESISTANCEBUFFMODSNEGATIVE`, ≤ 0).
        resistance_neg: [i32; 7],
    }
    scalars {
    /// Mainhand damage range (`UNIT_FIELD_MINDAMAGE`/`MAXDAMAGE`).
    min_damage: f32,
    max_damage: f32,
    /// Offhand damage range (`UNIT_FIELD_MINOFFHANDDAMAGE`/`MAXOFFHANDDAMAGE`).
    min_offhand_damage: f32,
    max_offhand_damage: f32,
    /// Physical damage-done bonuses (`PLAYER_FIELD_MOD_DAMAGE_DONE_POS[0]` / `_NEG[0]`, neg ≤ 0).
    physical_bonus_pos: i32,
    physical_bonus_neg: i32,
    /// The physical damage multiplier (`PLAYER_FIELD_MOD_DAMAGE_DONE_PCT[0]`), 1.0 until streamed.
    damage_percent: f32,
    /// The same three at the ranged weapon's damage school, which `UnitRangedDamage` reads
    /// (`0x518a77`, `0x518ac0`, `0x518b0e`): the school of the first damage line of the ranged
    /// slot's record with a minimum above 0, else 0 (`0x5189e1`-`0x518a45`).
    ranged_bonus_pos: i32,
    ranged_bonus_neg: i32,
    ranged_percent: f32,
    /// Attack speeds in ms (`UNIT_FIELD_BASEATTACKTIME[0..2]`).
    main_attack_time_ms: u32,
    offhand_attack_time_ms: u32,
    /// Whether an off-hand weapon is equipped, which gates `UnitAttackSpeed`'s second return.
    has_offhand: bool,
    /// Melee AP + its split mods (`UNIT_FIELD_ATTACK_POWER` / `_MODS`, neg ≤ 0).
    attack_power: i32,
    attack_power_pos: i32,
    attack_power_neg: i32,
    /// `UNIT_FIELD_ATTACK_POWER_MULTIPLIER`, the percent bonus minus 1.0.
    attack_power_multiplier: f32,
    /// Ranged AP + its split mods.
    ranged_attack_power: i32,
    ranged_attack_power_pos: i32,
    ranged_attack_power_neg: i32,
    /// `UNIT_FIELD_RANGED_ATTACK_POWER_MULTIPLIER`, the same.
    ranged_attack_power_multiplier: f32,
    /// Ranged attack speed in ms (`UNIT_FIELD_RANGEDATTACKTIME`).
    ranged_attack_time_ms: u32,
    /// Ranged damage range (`UNIT_FIELD_MINRANGEDDAMAGE`/`MAXRANGEDDAMAGE`).
    ranged_min_damage: f32,
    ranged_max_damage: f32,
    /// The main hand's skill line (picked by [`super::weapon_subclass_skill`]) as `(value + perm,
    /// temp)`, the split the reference's skill reader `0x5ea460` makes for all four skill pairs.
    main_weapon_skill: (i32, i32),
    /// The off hand's pair, read the same way; an empty or non-weapon off hand is Unarmed.
    offhand_weapon_skill: (i32, i32),
    /// The ranged weapon's pair, the same split. Player only: `UnitRangedAttack` (`0x518b90`)
    /// has no creature fallback, so a pet answers `(0, 0)` where `UnitDefense` answers level * 5.
    ranged_weapon_skill: (i32, i32),
    /// The [`super::SKILL_DEFENSE`] pair, the same split. The player's only: `UnitDefense` gives a
    /// non-player level * 5 instead (`cgunit_skill`), so the pet feed leaves it unset.
    defense_skill: (i32, i32),
    /// Whether a wand is equipped (`HasWandEquipped`).
    has_wand: bool,
    /// Avoidance chances, already percents on the wire (`PLAYER_DODGE_PERCENTAGE` and siblings).
    dodge_percent: f32,
    parry_percent: f32,
    block_percent: f32,
    }
}

impl Default for UnitCombatStats {
    /// All zeros but the two damage multipliers, 1.0, which stock divides by
    /// (`PaperDollFrame.lua:298`).
    fn default() -> Self {
        UnitCombatStats {
            stats: [0; 5],
            stat_pos: [0; 5],
            stat_neg: [0; 5],
            resistances: [0; 7],
            resistance_pos: [0; 7],
            resistance_neg: [0; 7],
            min_damage: 0.0,
            max_damage: 0.0,
            min_offhand_damage: 0.0,
            max_offhand_damage: 0.0,
            physical_bonus_pos: 0,
            physical_bonus_neg: 0,
            damage_percent: 1.0,
            ranged_bonus_pos: 0,
            ranged_bonus_neg: 0,
            ranged_percent: 1.0,
            main_attack_time_ms: 0,
            offhand_attack_time_ms: 0,
            has_offhand: false,
            attack_power: 0,
            attack_power_pos: 0,
            attack_power_neg: 0,
            attack_power_multiplier: 0.0,
            ranged_attack_power: 0,
            ranged_attack_power_pos: 0,
            ranged_attack_power_neg: 0,
            ranged_attack_power_multiplier: 0.0,
            ranged_attack_time_ms: 0,
            ranged_min_damage: 0.0,
            ranged_max_damage: 0.0,
            main_weapon_skill: (0, 0),
            offhand_weapon_skill: (0, 0),
            ranged_weapon_skill: (0, 0),
            defense_skill: (0, 0),
            has_wand: false,
            dodge_percent: 0.0,
            parry_percent: 0.0,
            block_percent: 0.0,
        }
    }
}

/// What the active player's own fields answer to the getters that read them and no other unit's,
/// each read on its own: off its object through the lent host
/// ([`crate::script::CallHost::read_self`]), or off the VM's copy, which the app's pushes keep
/// ([`Model`]'s implementation). The app's one derivation of each is its implementation of this.
pub trait SelfRead {
    /// `PLAYER_FIELD_COINAGE` (`+0xf70`), `GetMoney`'s (`0x518160`, `0x5181ae`).
    fn money(&self) -> u64;
    /// `PLAYER_XP` (`+0x840`), `UnitXP`'s for the active player (`0x517350`, `0x5173c9`).
    fn xp(&self) -> u32;
    /// `PLAYER_NEXT_LEVEL_XP` (`+0x844`), `UnitXPMax`'s (`0x517410`, `0x517489`).
    fn next_level_xp(&self) -> u32;
    /// `PLAYER_BYTES_2` byte 3, the `Exhaustion.dbc` row `GetRestState` (`0x48d350`) and
    /// `GetXPExhaustion` (`0x48d3f0`) read.
    fn rest_state(&self) -> u8;
    /// `PLAYER_REST_STATE_EXPERIENCE`, raw: the pool `GetXPExhaustion` scales.
    fn rest_pool(&self) -> u32;
    /// `PLAYER_FLAGS` bit `0x20`, `IsResting`'s (`0x516ea0`).
    fn resting(&self) -> bool;
    /// `PLAYER_FLAGS` bit `0x1000`, `PartialPlayTime`'s (`0x48eb70`).
    fn partial_play_time(&self) -> bool;
    /// `PLAYER_FLAGS` bit `0x2000`, `NoPlayTime`'s (`0x48ebe0`).
    fn no_play_time(&self) -> bool;
}

/// No active player held, as the reference's player getters answer one: 0, nil, three nils
/// (`GetMoney` `0x5181db`, `IsResting` `0x516ee8`; rest state 0 is no `Exhaustion.dbc` row).
pub struct NoPlayer;

impl SelfRead for NoPlayer {
    fn money(&self) -> u64 {
        0
    }
    fn xp(&self) -> u32 {
        0
    }
    fn next_level_xp(&self) -> u32 {
        0
    }
    fn rest_state(&self) -> u8 {
        0
    }
    fn rest_pool(&self) -> u32 {
        0
    }
    fn resting(&self) -> bool {
        false
    }
    fn partial_play_time(&self) -> bool {
        false
    }
    fn no_play_time(&self) -> bool {
        false
    }
}

/// The VM's copy, as the pushes left it.
impl SelfRead for Model {
    fn money(&self) -> u64 {
        self.money
    }
    fn xp(&self) -> u32 {
        self.player_xp
    }
    fn next_level_xp(&self) -> u32 {
        self.player_next_level_xp
    }
    fn rest_state(&self) -> u8 {
        self.rest_state
    }
    fn rest_pool(&self) -> u32 {
        self.rest_pool
    }
    fn resting(&self) -> bool {
        self.resting
    }
    fn partial_play_time(&self) -> bool {
        self.partial_play_time
    }
    fn no_play_time(&self) -> bool {
        self.no_play_time
    }
}

/// `f` over the unit object held under `guid` and its combat stats through the lent host, as of
/// the place the getter runs; `None` with no host free to ask (a glue VM, or a call's apply under
/// way) or no object held there, where the getter reads its snapshot.
pub(crate) fn read_unit_stats<T>(
    lua: &Lua,
    guid: u64,
    f: impl FnOnce(&dyn UnitObjectRead, &dyn CombatRead) -> T,
) -> Option<T> {
    if guid == 0 {
        return None;
    }
    let mut f = Some(f);
    let mut out = None;
    super::host::read_host(lua, |h| {
        h.read_unit_stats(guid, &mut |object, stats| {
            out = f.take().map(|f| f(object, stats));
        })
    });
    out
}

/// `f` over the active player's combat stats, `None` with no player held: through the lent host
/// when one is free to ask, else off the VM's copy, the snapshot. The getters with no unit
/// argument read the active player's field block (`GetDodgeChance` `0x516f00`, 0 with none at
/// `0x516f42`).
pub(crate) fn read_own_stats<T>(lua: &Lua, f: impl FnOnce(Option<&dyn CombatRead>) -> T) -> T {
    let mut f = Some(f);
    let mut out = None;
    super::host::read_host(lua, |h| {
        h.read_own_stats(&mut |stats| {
            out = f.take().map(|f| f(stats));
        })
    });
    match out {
        Some(out) => out,
        None => {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            f.take().expect("an unread `f`")(
                model
                    .player_combat_stats
                    .as_ref()
                    .map(|s| s as &dyn CombatRead),
            )
        }
    }
}

/// `f` over the active player's own fields: through the lent host when one is free to ask, which
/// hands [`NoPlayer`] while it holds none, else off the VM's copy.
pub(crate) fn read_self<T>(lua: &Lua, f: impl FnOnce(&dyn SelfRead) -> T) -> T {
    let mut f = Some(f);
    let mut out = None;
    super::host::read_host(lua, |h| {
        h.read_self(&mut |own| {
            out = f.take().map(|f| f(own));
        })
    });
    match out {
        Some(out) => out,
        None => {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            f.take().expect("an unread `f`")(&*model)
        }
    }
}
