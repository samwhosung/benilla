//! What the gate `0x60f600` asks of a unit itself and what the bind `0x7cb6d0` reads of it, shared
//! by the frame's gate ([`super::drive_vplates`], from `0x607ef9`) and the host's reads inside a
//! packet ([`super::live::NamePlateLive`], from `0x60706a` and the unit's callbacks).

use benilla_formats::Reaction;
use benilla_ui::script::{unit_is_grey, PlateUnit};

use crate::names::NameCache;
use crate::net::ObjectStore;
use crate::query_cache::Ask;
use crate::target::relations::{RelUnit, Relations};

use super::{con_color, plate_tint, VPlateMode, CREATURE_TYPE_TOTEM};

/// The reads around a unit the gate's own tests and the bind take: the relation functions and
/// the active player as they judge it, the creature types, the name cache, the player's level,
/// the raid board and the script bits.
pub(crate) struct Judge<'a> {
    pub(crate) rel: Relations<'a>,
    pub(crate) me: Option<RelUnit>,
    /// The unit a guid and its descriptor name, with its creature record.
    pub(crate) unit_of: &'a dyn Fn(u64, &ObjectStore) -> RelUnit,
    pub(crate) types: crate::creature_type::CreatureTypeSources<'a>,
    pub(crate) names: Option<&'a NameCache>,
    /// The active player's level, 1 without one.
    pub(crate) my_level: u32,
    /// The active player's drunkenness, `PLAYER_BYTES_3` byte 1 (`[+0xe68]+0x1d`); 0 without one.
    pub(crate) my_drunk: u8,
    pub(crate) group: Option<&'a crate::ui_party::GroupState>,
    pub(crate) mode: VPlateMode,
}

impl Judge<'_> {
    /// The unit's reaction toward the active player, by standing, which colours its bar
    /// (`0x7cbaa0`); neutral for a unit whose descriptor has not streamed.
    fn rank(&self, guid: u64, store: Option<&ObjectStore>) -> u8 {
        store
            .map(|s| (self.unit_of)(guid, s))
            .zip(self.me.as_ref())
            .map_or(Reaction::Neutral as u8, |(u, me)| self.rel.reaction(&u, me))
    }

    /// The gate's tests of the unit itself (`0x60f600`), the spatial ones apart: alive
    /// (`0x60f616`), selectable (`0x60f628`), no ghost or creep (`0x60f635`), its side's bit on
    /// (`0x60f6f3`-`0x60f709`), no created, unsummoned unit with flags bit 9 (`0x60f70b`-
    /// `0x60f72d`), no dead look on the hostile side (`0x60f733`) and no totem on the friendly one
    /// (`0x60f750`-`0x60f75a`). The rank its bar colours by when it passes.
    pub(crate) fn admits(
        &self,
        guid: u64,
        is_player: bool,
        store: Option<&ObjectStore>,
    ) -> Option<u8> {
        let mode = self.mode;
        if !mode.enemies && !mode.friends {
            return None;
        }
        // `NOT_SELECTABLE` (flags bit 25) never gets a plate, re-tested every tick (`0x60f622`,
        // from CGUnit's OnUpdate `0x607ed0`). A plate hover publishes the mouseover with no
        // selectability check (`0x7cb869`, unlike the world hover at `0x482982`), so this gate
        // is what keeps such a unit's tooltip away.
        if store.is_some_and(|s| s.0.unit_flags() & (1 << 25) != 0) {
            return None;
        }
        // Health 0 or less (absent reads 0) has no plate. Feign death keeps health, so it passes
        // here.
        if store.is_none_or(|s| s.0.unit_health().unwrap_or(0) == 0) {
            return None;
        }
        // `bytes_1` byte 3 `& 3` (ghost or creep) has no plate.
        if store.is_some_and(|s| s.0.unit_is_ghost_visual()) {
            return None;
        }
        // A CREATEDBY owner, no SUMMONEDBY owner and flags bit 9 (`0x200`): no plate in either
        // category; the bit's 1.12 name is unknown.
        if store.is_some_and(|s| {
            s.0.unit_created_by().is_some_and(|g| g != 0)
                && s.0.unit_summoned_by().is_none_or(|g| g == 0)
                && s.0.unit_flags() & 0x200 != 0
        }) {
            return None;
        }
        // The category is `CanAttack` from the player (a rep faction answers by its at-war bit);
        // the bar colour is the unit's reaction toward us (by standing). A not-at-war neutral NPC
        // is a friendly plate with a yellow bar. A unit whose descriptor has not streamed is
        // attackable by nobody, so friendly unless a player, whom `CanCooperate` cannot pass.
        let pair = store.map(|s| (self.unit_of)(guid, s)).zip(self.me.as_ref());
        let friendly = pair.as_ref().map_or(!is_player, |(u, me)| {
            crate::target::ring::plate_is_friendly(&self.rel, u, me, is_player)
        });
        if friendly && !mode.friends || !friendly && !mode.enemies {
            return None;
        }
        // Only on the hostile side, a dead-looking unit has no plate: a feigning hunter's plate
        // hides from enemies and stays for friends.
        if !friendly && store.is_some_and(|s| s.0.unit_reads_dead()) {
            return None;
        }
        // Only on the friendly side, a totem has no plate (the type `0x605570` resolves); an
        // enemy totem keeps its plate.
        if friendly && store.is_some_and(|s| self.types.of(s) == CREATURE_TYPE_TOTEM) {
            return None;
        }
        Some(self.rank(guid, store))
    }

    /// What the bind reads of the unit (`0x7cb6d0`), its reaction rank `rank` given, or read
    /// anew by a callback ([`Self::unit_now`]).
    pub(crate) fn unit(
        &self,
        guid: u64,
        is_player: bool,
        store: Option<&ObjectStore>,
        rank: u8,
    ) -> PlateUnit {
        let tint = plate_tint(rank, is_player);
        let (health, max_health) = store.map_or((0.0, 0.0), bar_of);
        let my_level = self.my_level;
        // The level the plate shows (`0x7cbb7a` → `0x7cbce0`), which its skull and colour read.
        let level = store.and_then(|s| {
            s.0.unit_level()
                .map(|level| self.displayed_level(guid, s, level))
        });
        let mark = self.group.map_or(0, |g| g.raid_target_index(guid));
        PlateUnit {
            health,
            max_health,
            bar_colour: [tint[0], tint[1], tint[2]],
            // Read, never asked: the frame's gate runs every frame; the unit's create asks.
            name: self
                .names
                .and_then(|n| n.resolve_unit(guid, store, Ask::Read))
                .map(str::to_owned),
            // The skull (`0x7cbb40`): rank 3 through `gated_rank` (so a charmed boss shows its
            // level), or a hostile 10 or more levels up that is not grey (`0x5f0700`, vacuous
            // there but transcribed). A unit with no level yet shows neither.
            level,
            skull: level.is_some_and(|level| {
                crate::names::gated_rank(
                    store
                        .and_then(|s| s.0.object_entry())
                        .and_then(|e| self.names.and_then(|n| n.creature_record(e))),
                    store,
                ) == 3
                    || (rank <= 1 && level >= my_level + 10 && !unit_is_grey(my_level, level))
            }),
            level_colour: {
                let c = level.map_or([1.0, 1.0, 1.0, 1.0], |level| con_color(my_level, level));
                [c[0], c[1], c[2]]
            },
            // 0-based for the atlas: `col = idx & 3`, `row = idx >> 2`.
            raid_icon: (mark >= 1).then(|| mark - 1),
        }
    }

    /// `0x7cbce0(player, unit)`: `level` less `trunc(min(drunk, 100) * 0.05 + 0.5)` (`0x7cbd1b`-
    /// `0x7cbd36`) when the level is above 0 (`0x7cbcf3`, signed), there is an active player
    /// (`0x7cbcf7`), its drunkenness is nonzero (`0x7cbd04`) and it can attack the unit
    /// (`0x7cbd09`); never below 1 (`0x7cbd3d`). Its own copy of the reduction: `UnitLevel`'s
    /// `drunk * 5 * 0.01` (`0x5180c8`) rounds otherwise at 10, 30, 50, 70 and 90.
    fn displayed_level(&self, guid: u64, store: &ObjectStore, level: u32) -> u32 {
        let signed = level as i32;
        if signed <= 0 || self.my_drunk == 0 {
            return level;
        }
        let Some(me) = self.me.as_ref() else {
            return level;
        };
        if !self.rel.can_attack(me, &(self.unit_of)(guid, store)) {
            return level;
        }
        // `fild; fmul [0.05f32]; fadd [0.5f32]; __ftol`: exact in f64 for a byte of at most 100.
        let n = f64::from(self.my_drunk.min(100));
        let cut = (n * f64::from(0.05_f32) + f64::from(0.5_f32)) as i32;
        (signed - cut).max(1) as u32
    }

    /// [`Self::unit`] with its rank read as it stands, for a callback's write.
    pub(crate) fn unit_now(
        &self,
        guid: u64,
        is_player: bool,
        store: Option<&ObjectStore>,
    ) -> PlateUnit {
        self.unit(guid, is_player, store, self.rank(guid, store))
    }
}

/// The bar's value and range off the unit (`0x7cc41b`-`0x7cc442`, `0x7cc593`-`0x7cc5be`): raw
/// health, not a fraction, as `healthbar:GetValue()` is `[bar+0x320]` (`0x78f5d0`) and addons
/// divide it by the maximum. Both fields load signed (`fild` at `0x7cc421`, `0x7cc437`) and go to
/// the bar unclamped, a field not streamed as 0: `0x783270` takes a range of 0 to 0.
pub(crate) fn bar_of(store: &ObjectStore) -> (f32, f32) {
    let signed = |field: Option<u32>| field.unwrap_or(0) as i32 as f32;
    (
        signed(store.0.unit_health()),
        signed(store.0.unit_max_health()),
    )
}
