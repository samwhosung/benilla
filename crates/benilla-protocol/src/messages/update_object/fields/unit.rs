//! Named accessors over [`ObjectFields`] for the object scale and the unit block.

use super::*;

/// `UNIT_DYNAMIC_FLAGS` bit 5 (`SharedDefines.h:1158`): reads as a corpse wherever health is
/// shown; set for feign death and appear-dead spawns, with health left intact.
pub const UNIT_DYNFLAG_DEAD: u32 = 0x20;

/// `UNIT_STAND_STATE_DEAD` (`UnitDefines.h:109`), the third leg of `0x605f90`; vmangos never
/// writes it. Both `[vtbl+0xa4]` overrides (`0x60be50`, `0x5ed570`) read `UNIT_FIELD_BYTES_1`
/// byte 0.
const STAND_STATE_DEAD: u8 = 7;

/// The reference's raw-to-shown power divisor (`0x6e7130`, table `0x86f978`): 10 for rage, 1000
/// for pet happiness, 1 for any other type, `POWER_HEALTH` (-2) included. Every power figure a
/// player reads divides by it: `UnitMana`, `UnitManaMax`, party power, the energize and periodic
/// logs, the drain formatter and the spell cost; happiness thresholds stay on the raw scale.
pub fn power_display_scale(ty: u32) -> u32 {
    match ty {
        1 => 10,   // rage
        4 => 1000, // happiness
        _ => 1,
    }
}

/// The field [`ObjectFields::unit_owner`] falls back to when `CHARMEDBY` is clear, the one thing
/// the client's two owner readers do differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerFallback {
    /// `UNIT_FIELD_CREATEDBY`, as the own-pet resolver `0x5ee5a0` reads.
    CreatedBy,
    /// `UNIT_FIELD_SUMMONEDBY`, as the `PET_ATTACK_*` callback `0x5ff580` reads.
    SummonedBy,
}

impl ObjectFields {
    /// `OBJECT_FIELD_SCALE_X`: the per-object size multiplier.
    pub fn object_scale_x(&self) -> Option<f32> {
        self.get_f32(FIELD_OBJECT_SCALE_X)
    }
    /// `UNIT_FIELD_DISPLAYID` (units + players).
    pub fn unit_displayid(&self) -> Option<i32> {
        self.get_i32(FIELD_UNIT_DISPLAYID)
    }
    /// `UNIT_FIELD_NATIVEDISPLAYID`: the appearance no shapeshift touches; the collision prism
    /// comes from it, so a druid in bear form keeps the druid's water lines.
    pub fn unit_native_displayid(&self) -> Option<i32> {
        self.get_i32(FIELD_UNIT_NATIVEDISPLAYID)
    }
    /// `UNIT_FIELD_MOUNTDISPLAYID`: the mount's `CreatureDisplayInfo` id, the only mounted signal.
    pub fn unit_mount_display_id(&self) -> u32 {
        self.get_u32(FIELD_UNIT_MOUNTDISPLAYID).unwrap_or(0)
    }
    /// `UNIT_FIELD_TARGET`: the guid this unit is attacking or interacting with.
    pub fn unit_target(&self) -> Option<u64> {
        self.get_guid(FIELD_UNIT_TARGET).filter(|&g| g != 0)
    }
    /// `UNIT_FIELD_CHANNEL_OBJECT`: what a channel is aimed at, possibly the caster itself.
    pub fn unit_channel_object(&self) -> Option<u64> {
        self.get_guid(FIELD_UNIT_CHANNEL_OBJECT).filter(|&g| g != 0)
    }
    /// `UNIT_CHANNEL_SPELL`: the spell being channelled, 0 for none.
    pub fn unit_channel_spell(&self) -> u32 {
        self.get_u32(FIELD_UNIT_CHANNEL_SPELL).unwrap_or(0)
    }
    /// `UNIT_FIELD_SUMMON`: the unit this one summoned; on our own descriptor, the `"pet"` unit.
    pub fn unit_summon(&self) -> Option<u64> {
        self.get_guid(FIELD_UNIT_SUMMON).filter(|&g| g != 0)
    }
    /// `UNIT_FIELD_SUMMONEDBY`: the summoner of a pet, guardian or totem.
    pub fn unit_summoned_by(&self) -> Option<u64> {
        self.get_guid(FIELD_UNIT_SUMMONEDBY).filter(|&g| g != 0)
    }
    /// `UNIT_FIELD_CHARMEDBY`: whoever is charming this unit.
    pub fn unit_charmed_by(&self) -> Option<u64> {
        self.get_guid(FIELD_UNIT_CHARMEDBY).filter(|&g| g != 0)
    }
    /// The owner as the client's owner readers compute it: `CHARMEDBY` if set, else the fallback,
    /// which differs by caller (`0x5ee62f` reads `CREATEDBY`, `0x5ff769` `SUMMONEDBY`).
    pub fn unit_owner(&self, fallback: OwnerFallback) -> Option<u64> {
        self.unit_charmed_by().or_else(|| match fallback {
            OwnerFallback::CreatedBy => self.unit_created_by(),
            OwnerFallback::SummonedBy => self.unit_summoned_by(),
        })
    }
    /// `UNIT_FIELD_CREATEDBY`: the creator of a totem or created object.
    pub fn unit_created_by(&self) -> Option<u64> {
        self.get_guid(FIELD_UNIT_CREATEDBY).filter(|&g| g != 0)
    }
    /// `UNIT_CREATED_BY_SPELL`: the summoning spell; `0x6ea1e0` requires it to feed a pet.
    pub fn unit_created_by_spell(&self) -> Option<u32> {
        self.get_u32(FIELD_UNIT_CREATED_BY_SPELL)
            .filter(|&s| s != 0)
    }
    /// `UNIT_FIELD_PETNUMBER` is nonzero: a pet or charm, which the rank getter `0x605620`
    /// reports as rank 0. vmangos writes 0 for guardians and totems (`CharmInfo::SetPetNumber`
    /// without the stat window), so they keep their template's rank.
    pub fn unit_is_pet_or_charm(&self) -> bool {
        self.unit_pet_number() != 0
    }
    /// `UNIT_FIELD_PETNUMBER`: `GetUnitName` (`0x609210`) keys the pet-name cache on it when
    /// nonzero; a `HIGHGUID_PET` guid's number is not a substitute, no name query answers it.
    pub fn unit_pet_number(&self) -> u32 {
        self.get_u32(FIELD_UNIT_PETNUMBER).unwrap_or(0)
    }
    /// `UNIT_FIELD_PET_NAME_TIMESTAMP`: changes when the pet is renamed, staling the name cache.
    pub fn unit_pet_name_timestamp(&self) -> Option<u32> {
        self.get_u32(FIELD_UNIT_PET_NAME_TIMESTAMP)
    }
    /// `UNIT_FIELD_HEALTH`: current hit points, 0 when dead.
    pub fn unit_health(&self) -> Option<u32> {
        self.get_u32(FIELD_UNIT_HEALTH)
    }
    /// `UNIT_FIELD_MAXHEALTH`: full hit points.
    pub fn unit_max_health(&self) -> Option<u32> {
        self.get_u32(FIELD_UNIT_MAXHEALTH)
    }
    /// Really dead: health 0 with a known maximum, since a dead unit's create sends no health.
    /// For how the unit looks, which a feigning hunter shares, use [`Self::unit_reads_dead`].
    pub fn unit_is_dead(&self) -> bool {
        self.unit_max_health().unwrap_or(0) > 0 && self.unit_health().unwrap_or(0) == 0
    }
    /// Reads as dead, the reference's `0x605f90`: health 0, `UNIT_DYNFLAG_DEAD` (feign death,
    /// `0x605f9d`) or stand state 7 (`0x605faa`). Feign death sets only the flag
    /// (`Unit.cpp:9491`) and every viewer, the feigner included, receives it.
    pub fn unit_reads_dead(&self) -> bool {
        self.unit_is_dead()
            || self.unit_dynamic_flags() & UNIT_DYNFLAG_DEAD != 0
            || self.unit_stand_state() == STAND_STATE_DEAD
    }
    /// Health as `UnitHealth` (`0x5174d0`) reads it: 0 under `UNIT_DYNFLAG_DEAD`, else raw.
    /// `UnitHealthMax` (`0x5175b0`) has no such gate, so a feigning unit's bar shows empty.
    pub fn unit_shown_health(&self) -> Option<u32> {
        if self.unit_dynamic_flags() & UNIT_DYNFLAG_DEAD != 0 {
            return Some(0);
        }
        self.unit_health()
    }
    /// Power as `UnitMana` (`0x517670`) reads it: 0 under `UNIT_DYNFLAG_DEAD`, else divided by
    /// [`power_display_scale`]. Its `POWER_HEALTH` leg (`0x5176f7`) compares a zero-extended
    /// byte with -2, so it never matches and is left out.
    pub fn unit_shown_power(&self, ty: u8) -> Option<u32> {
        if self.unit_dynamic_flags() & UNIT_DYNFLAG_DEAD != 0 {
            return (ty < 5).then_some(0);
        }
        Some(self.unit_power(ty)? / power_display_scale(u32::from(ty)))
    }
    /// Maximum power as `UnitManaMax` (`0x5177e0`) reads it: divided, with no dead gate.
    pub fn unit_shown_max_power(&self, ty: u8) -> Option<u32> {
        Some(self.unit_max_power(ty)? / power_display_scale(u32::from(ty)))
    }
    /// `UNIT_FIELD_LEVEL`: the unit's level.
    pub fn unit_level(&self) -> Option<u32> {
        self.get_u32(FIELD_UNIT_LEVEL)
    }
    /// One aura slot, live only when its `AURAFLAGS` nibble has an effect bit (`& 0x0E`) as the
    /// client tests it, since a cleared slot can keep a stale spell id.
    pub fn unit_aura(&self, slot: u8) -> Option<UnitAuraSlot> {
        if slot >= UNIT_AURA_SLOTS {
            return None;
        }
        let flags = self.get_aura_nibble(slot);
        if flags & AURA_FLAG_EFF_INDEX_MASK == 0 {
            return None;
        }
        let spell_id = self
            .get_u32(FIELD_UNIT_AURA + u16::from(slot))
            .filter(|&id| id != 0)?;
        Some(UnitAuraSlot {
            slot,
            spell_id,
            flags,
            level: self.get_aura_byte(FIELD_UNIT_AURALEVELS, slot),
            // The wire byte is `stack - 1` and saturates at 254, so an occupied slot is always ≥ 1.
            stacks: self
                .get_aura_byte(FIELD_UNIT_AURAAPPLICATIONS, slot)
                .saturating_add(1),
        })
    }
    /// Live auras by ascending slot, buffs 0-31 then debuffs 32-47: the order `UnitBuff` and
    /// `UnitDebuff` read another unit in (`0x519500`, `0x5198f0`). The local player's buff bar
    /// keeps insertion order in a packed cache (`0xbc6040`) instead, which this cannot recover.
    pub fn unit_auras(&self) -> impl Iterator<Item = UnitAuraSlot> + '_ {
        (0..UNIT_AURA_SLOTS).filter_map(|slot| self.unit_aura(slot))
    }
    /// Every `UNIT_FIELD_AURA` slot's raw spell id, zeros and stale ids included, as the cast
    /// validator's crowd-control scan (`0x6e9ca0`) reads them, without `AURAFLAGS`.
    pub fn unit_aura_ids(&self) -> impl Iterator<Item = u32> + '_ {
        (0..UNIT_AURA_SLOTS)
            .map(|slot| self.get_u32(FIELD_UNIT_AURA + u16::from(slot)).unwrap_or(0))
    }

    /// `UNIT_FIELD_BYTES_0` byte 3: power type, 0 mana, 1 rage, 2 focus, 3 energy, 4 happiness.
    pub fn unit_power_type(&self) -> u8 {
        (self.unit_bytes_0().unwrap_or(0) >> 24) as u8
    }
    /// `UNIT_FIELD_BYTES_1` byte 0: stand state, 0 stand, 1 sit, 2 chair, 3 sleep, 4-6 low,
    /// medium and high chair, 8 kneel; for a player, the echo of `CMSG_STANDSTATECHANGE`.
    pub fn unit_stand_state(&self) -> u8 {
        (self.get_u32(FIELD_UNIT_BYTES_1).unwrap_or(0) & 0xff) as u8
    }
    /// `UNIT_FIELD_BYTES_1` byte 1: a hunter pet's loyalty level, 1-8, which `GetPetLoyalty`
    /// (`0x4be700`) indexes `PetLoyalty.dbc` with; 0 is none and answers nil.
    pub fn unit_loyalty_level(&self) -> u8 {
        ((self.get_u32(FIELD_UNIT_BYTES_1).unwrap_or(0) >> 8) & 0xff) as u8
    }
    /// `UNIT_FIELD_PETEXPERIENCE` and `UNIT_FIELD_PETNEXTLEVELEXP`: `GetPetExperience`'s
    /// `(currXP, nextXP)`; absent reads 0, the binding's own failure value, never nil.
    pub fn unit_pet_experience(&self) -> (u32, u32) {
        (
            self.get_u32(FIELD_UNIT_PETEXPERIENCE).unwrap_or(0),
            self.get_u32(FIELD_UNIT_PETNEXTLEVELEXP).unwrap_or(0),
        )
    }
    /// `UNIT_TRAINING_POINTS`: `(total, spent)`, total in the high half as the client splits it.
    pub fn unit_training_points(&self) -> (u16, u16) {
        let packed = self.get_u32(FIELD_UNIT_TRAINING_POINTS).unwrap_or(0);
        ((packed >> 16) as u16, (packed & 0xffff) as u16)
    }
    /// `UNIT_FIELD_BYTES_1` byte 3 bit `0x2`, `UNIT_VIS_FLAGS_CREEP` (`SpellAuras.cpp:3610`):
    /// stealthed; read by the usable check (`[+0x110]+0x213 & 2`) and the tracker (`0x5ed210`).
    pub fn unit_is_stealthed(&self) -> bool {
        (self.get_u32(FIELD_UNIT_BYTES_1).unwrap_or(0) >> 24) & 0x2 != 0
    }
    /// `UNIT_FIELD_BYTES_1` byte 3 bit `0x1`, `UNIT_VIS_FLAGS_GHOST`, set by the ghost aura: it
    /// only hides names and plates (`byte3 & 0x03`, `0x607101`, `0x60f62e`); the ghostly body is
    /// the aura's own visual kit (spell 8326, kit 989).
    pub fn unit_is_ghost_visual(&self) -> bool {
        (self.get_u32(FIELD_UNIT_BYTES_1).unwrap_or(0) >> 24) & 0x1 != 0
    }

    /// `UNIT_FIELD_BYTES_1` byte 3 bit `0x4` (vmangos `UNIT_VIS_FLAGS_UNTRACKABLE`): no minimap
    /// dot at all, quest or tracking (`byte [eax+0x213] & 4`, the byte's only such test).
    pub fn unit_is_untrackable(&self) -> bool {
        (self.get_u32(FIELD_UNIT_BYTES_1).unwrap_or(0) >> 24) & 0x4 != 0
    }

    /// `UNIT_FIELD_AURASTATE`: the aura-state bits (1 defense, 2 health under 20%, ...), which
    /// the usable check tests as `1 << (state - 1)`.
    pub fn unit_aura_state(&self) -> u32 {
        self.get_u32(FIELD_UNIT_AURASTATE).unwrap_or(0)
    }

    /// `UNIT_FIELD_BYTES_1` byte 2: the shapeshift form, 0 none, 17-19 the warrior stances, the
    /// druid forms low; the creature-type resolver `0x605570` checks it first.
    pub fn unit_shapeshift_form(&self) -> u8 {
        (self.get_u32(FIELD_UNIT_BYTES_1).unwrap_or(0) >> 16) as u8
    }
    /// `UNIT_FIELD_POWER1..5`: current power of type `ty`, on the wire's raw scale.
    pub fn unit_power(&self, ty: u8) -> Option<u32> {
        (ty < 5).then(|| self.get_u32(FIELD_UNIT_POWER1 + u16::from(ty)))?
    }
    /// `UNIT_FIELD_MAXPOWER1..5`: maximum power of type `ty`, raw.
    pub fn unit_max_power(&self, ty: u8) -> Option<u32> {
        (ty < 5).then(|| self.get_u32(FIELD_UNIT_MAXPOWER1 + u16::from(ty)))?
    }
    /// `UNIT_FIELD_BASE_MANA` (field 162, owner only): what `ManaCostPercentage` costs scale from.
    pub fn unit_base_mana(&self) -> Option<u32> {
        self.get_u32(FIELD_UNIT_BASE_MANA)
    }
    /// `UNIT_FIELD_FACTIONTEMPLATE` (public, `+0x74`): the `FactionTemplate.dbc` row reactions use.
    pub fn unit_faction_template(&self) -> Option<u32> {
        self.get_u32(FIELD_UNIT_FACTIONTEMPLATE)
    }
    /// `UNIT_FIELD_FLAGS` (`+0xa0`): unit state flags; bit `0x0400_0000` is skinnable.
    pub fn unit_flags(&self) -> u32 {
        self.get_u32(FIELD_UNIT_FLAGS).unwrap_or(0)
    }
    /// `UNIT_FIELD_COMBATREACH` (`+0x1f0`): melee reach, summed as `rA + rB + 1.333`, floor 5.0,
    /// by the attack range gate (`0x6e3480`); 1.5, the vanilla default, before it streams.
    pub fn unit_combat_reach(&self) -> f32 {
        self.get_f32(FIELD_UNIT_COMBATREACH).unwrap_or(1.5)
    }
    /// `UNIT_FIELD_BOUNDINGRADIUS`: horizontal radius in yards; the water-foam depth gate takes
    /// `max(2 * radius, 1.0)` (`0x5fa760`), so absent reads 0.
    pub fn unit_bounding_radius(&self) -> f32 {
        self.get_f32(FIELD_UNIT_BOUNDINGRADIUS).unwrap_or(0.0)
    }
    /// `UNIT_DYNAMIC_FLAGS` (`+0x224`), per viewer: `0x1` lootable, `0x2` tracked, `0x20` dead.
    pub fn unit_dynamic_flags(&self) -> u32 {
        self.get_u32(FIELD_UNIT_DYNAMIC_FLAGS).unwrap_or(0)
    }
    /// `UNIT_DYNAMIC_FLAGS` bit `0x20` alone: `SetSelection`'s last-enemy stamp gates on
    /// `HEALTH > 0 || dynflag 0x20` (`0x49372f`), not on [`Self::unit_reads_dead`].
    pub fn unit_dynflag_dead(&self) -> bool {
        self.unit_dynamic_flags() & UNIT_DYNFLAG_DEAD != 0
    }
    /// `UNIT_DYNAMIC_FLAGS` bit `0x4`: tapped, the only test in `UnitIsTapped` (`0x519c90`).
    pub fn unit_tapped(&self) -> bool {
        self.unit_dynamic_flags() & 0x4 != 0
    }
    /// `UNIT_DYNAMIC_FLAGS` bit `0x8`: tapped by me (`UnitIsTappedByPlayer`, `0x519d00`), which
    /// the server sets per viewer; tapped but not by me is the grey unit frame.
    pub fn unit_tapped_by_player(&self) -> bool {
        self.unit_dynamic_flags() & 0x8 != 0
    }
    /// `UNIT_DYNAMIC_FLAGS` bit `0x1`: lootable by me, stripped per viewer by the server; gates
    /// the loot cursor, right-click looting and the corpse sparkle.
    pub fn unit_lootable(&self) -> bool {
        self.unit_dynamic_flags() & 0x1 != 0
    }
    /// `UNIT_NPC_FLAGS` (`+0x234`, `UnitDefines.h`): gossip `0x1`, questgiver `0x2`, vendor
    /// `0x4`, flight master `0x8`, trainer `0x10`, ..., repair `0x4000`; cursor: first bit wins.
    pub fn unit_npc_flags(&self) -> u32 {
        self.get_u32(FIELD_UNIT_NPC_FLAGS).unwrap_or(0)
    }
    /// `UNIT_NPC_EMOTESTATE`: the looping `Emotes.dbc` state, which the server keeps while moving.
    pub fn unit_emote_state(&self) -> u32 {
        self.get_u32(FIELD_UNIT_NPC_EMOTESTATE).unwrap_or(0)
    }
    /// `UNIT_FIELD_BYTES_0`: race, class, gender and power type, bytes 0 to 3.
    pub fn unit_bytes_0(&self) -> Option<u32> {
        self.get_u32(FIELD_UNIT_BYTES_0)
    }
    /// `UNIT_VIRTUAL_ITEM_SLOT_DISPLAY + slot`: a creature's weapon `DisplayInfoID` for slot 0
    /// main hand, 1 off hand or 2 ranged, with no item behind it.
    pub fn unit_virtual_item_display(&self, slot: u8) -> Option<u32> {
        (slot < 3).then(|| self.get_u32(FIELD_UNIT_VIRTUAL_ITEM_SLOT_DISPLAY + u16::from(slot)))?
    }
    /// `UNIT_VIRTUAL_ITEM_INFO + 2*slot`: bytes `(class, subclass, material, inventory_type)`.
    pub fn unit_virtual_item_info(&self, slot: u8) -> Option<(u8, u8, u8, u8)> {
        let v =
            (slot < 3).then(|| self.get_u32(FIELD_UNIT_VIRTUAL_ITEM_INFO + 2 * u16::from(slot)))?;
        v.map(|v| {
            (
                (v & 0xff) as u8,
                ((v >> 8) & 0xff) as u8,
                ((v >> 16) & 0xff) as u8,
                ((v >> 24) & 0xff) as u8,
            )
        })
    }
    /// `UNIT_VIRTUAL_ITEM_INFO + 2*slot + 1` byte 0: the virtual item's sheath type.
    pub fn unit_virtual_item_sheath(&self, slot: u8) -> Option<u8> {
        let v = (slot < 3)
            .then(|| self.get_u32(FIELD_UNIT_VIRTUAL_ITEM_INFO + 2 * u16::from(slot) + 1))?;
        v.map(|v| (v & 0xff) as u8)
    }
    /// `UNIT_FIELD_BYTES_2` byte 0: sheath state, 0 stowed, 1 melee drawn, 2 ranged drawn; for a
    /// player, whatever they last sent in `CMSG_SETSHEATHED`.
    pub fn unit_sheath_state(&self) -> Option<u8> {
        self.get_u32(FIELD_UNIT_BYTES_2).map(|v| (v & 0xff) as u8)
    }
    /// `UNIT_FIELD_STAT0 + i`: primary stat `i` after buffs, 0 Str, 1 Agi, 2 Sta, 3 Int, 4 Spi.
    pub fn unit_stat(&self, i: u8) -> Option<u32> {
        (i < 5).then(|| self.get_u32(FIELD_UNIT_STAT0 + u16::from(i)))?
    }
    /// `UNIT_FIELD_RESISTANCES + i`: resistance `i`, signed, 0 armor, 1-6 the magic schools.
    pub fn unit_resistance(&self, i: u8) -> Option<i32> {
        (i < 7).then(|| self.get_i32(FIELD_UNIT_RESISTANCES + u16::from(i)))?
    }
    /// `UNIT_FIELD_ATTACK_POWER`: melee attack power before the split pos/neg mods.
    pub fn unit_attack_power(&self) -> Option<i32> {
        self.get_i32(FIELD_UNIT_ATTACK_POWER)
    }
    /// `UNIT_FIELD_ATTACK_POWER_MODS`: flat `(positive, negative)` mods, the low and high i16.
    pub fn unit_attack_power_mods(&self) -> (i16, i16) {
        let (lo, hi) = self
            .get_u16_pair(FIELD_UNIT_ATTACK_POWER_MODS)
            .unwrap_or((0, 0));
        (lo as i16, hi as i16)
    }
    /// `UNIT_FIELD_ATTACK_POWER_MULTIPLIER`: the percent bonus minus 1.0, so 0.0 is no bonus.
    pub fn unit_attack_power_multiplier(&self) -> Option<f32> {
        self.get_f32(FIELD_UNIT_ATTACK_POWER_MULTIPLIER)
    }
    /// `UNIT_FIELD_RANGED_ATTACK_POWER`: ranged attack power before the split pos/neg mods.
    pub fn unit_ranged_attack_power(&self) -> Option<i32> {
        self.get_i32(FIELD_UNIT_RANGED_ATTACK_POWER)
    }
    /// `UNIT_FIELD_RANGED_ATTACK_POWER_MODS`: the ranged `(positive, negative)` flat mods.
    pub fn unit_ranged_attack_power_mods(&self) -> (i16, i16) {
        let (lo, hi) = self
            .get_u16_pair(FIELD_UNIT_RANGED_ATTACK_POWER_MODS)
            .unwrap_or((0, 0));
        (lo as i16, hi as i16)
    }
    /// `UNIT_FIELD_RANGED_ATTACK_POWER_MULTIPLIER`: the ranged percent bonus minus 1.0.
    pub fn unit_ranged_attack_power_multiplier(&self) -> Option<f32> {
        self.get_f32(FIELD_UNIT_RANGED_ATTACK_POWER_MULTIPLIER)
    }
    /// `UNIT_FIELD_MINDAMAGE`: the mainhand weapon's minimum damage.
    pub fn unit_min_damage(&self) -> Option<f32> {
        self.get_f32(FIELD_UNIT_MINDAMAGE)
    }
    /// `UNIT_FIELD_MAXDAMAGE`: the mainhand weapon's maximum damage.
    pub fn unit_max_damage(&self) -> Option<f32> {
        self.get_f32(FIELD_UNIT_MAXDAMAGE)
    }
    /// `UNIT_FIELD_MINOFFHANDDAMAGE`: the offhand minimum damage, sent even with no offhand worn.
    pub fn unit_min_offhand_damage(&self) -> Option<f32> {
        self.get_f32(FIELD_UNIT_MINOFFHANDDAMAGE)
    }
    /// `UNIT_FIELD_MAXOFFHANDDAMAGE`: the offhand weapon's maximum damage.
    pub fn unit_max_offhand_damage(&self) -> Option<f32> {
        self.get_f32(FIELD_UNIT_MAXOFFHANDDAMAGE)
    }
    /// `UNIT_FIELD_MINRANGEDDAMAGE`: the ranged weapon's minimum damage.
    pub fn unit_min_ranged_damage(&self) -> Option<f32> {
        self.get_f32(FIELD_UNIT_MINRANGEDDAMAGE)
    }
    /// `UNIT_FIELD_MAXRANGEDDAMAGE`: the ranged weapon's maximum damage.
    pub fn unit_max_ranged_damage(&self) -> Option<f32> {
        self.get_f32(FIELD_UNIT_MAXRANGEDDAMAGE)
    }
    /// `UNIT_FIELD_BASEATTACKTIME + i`: attack speed in ms, 0 mainhand, 1 offhand.
    pub fn unit_base_attack_time(&self, i: u8) -> Option<u32> {
        (i < 2).then(|| self.get_u32(FIELD_UNIT_BASEATTACKTIME + u16::from(i)))?
    }
    /// `UNIT_FIELD_RANGEDATTACKTIME`: ranged weapon attack speed in ms.
    pub fn unit_ranged_attack_time(&self) -> Option<u32> {
        self.get_u32(FIELD_UNIT_RANGEDATTACKTIME)
    }
}
