//! The combat log wire, inbound only: spell damage, periodic ticks, heals, power gains, damage
//! shields, environmental damage, misses, kills, dispels, enchants and spell effect logs.

use std::io::{self, Read};

use crate::wire::{
    capacity_hint, read_f32_le, read_i32_le, read_packed_guid, read_u32_le, read_u64_le, read_u8,
};

/// `SMSG_SPELLNONMELEEDAMAGELOG`, spell damage dealt (`Server/Packets/Spell.cpp:124-140`);
/// `hit_info` bit `0x2` is a crit (`SpellDefines.h:179`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellDamageLog {
    pub target: u64,
    pub attacker: u64,
    pub spell_id: u32,
    pub damage: u32,
    pub school: u8,
    pub absorb: u32,
    pub resist: i32,
    pub periodic: bool,
    pub blocked: u32,
    pub hit_info: u32,
}

pub(super) fn read_spell_damage_log(r: &mut impl Read) -> io::Result<SpellDamageLog> {
    let target = read_packed_guid(r)?;
    let attacker = read_packed_guid(r)?;
    let spell_id = read_u32_le(r)?;
    let damage = read_u32_le(r)?;
    let school = read_u8(r)?;
    let absorb = read_u32_le(r)?;
    let resist = read_i32_le(r)?;
    let periodic = read_u8(r)? != 0;
    let _unused = read_u8(r)?;
    let blocked = read_u32_le(r)?;
    let hit_info = read_u32_le(r)?;
    let _extended_data = read_u8(r)?;
    Ok(SpellDamageLog {
        target,
        attacker,
        spell_id,
        damage,
        school,
        absorb,
        resist,
        periodic,
        blocked,
        hit_info,
    })
}

/// One `SMSG_PERIODICAURALOG` tick, shaped by its aura type (vmangos `SpellAuraDefines.h`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PeriodicTick {
    Damage {
        amount: u32,
        school: u32,
        absorb: u32,
        resist: i32,
    },
    Heal {
        amount: u32,
    },
    Energize {
        power: u32,
        amount: u32,
    },
    ManaLeech {
        power: u32,
        amount: u32,
        multiplier: f32,
    },
}

/// `SMSG_PERIODICAURALOG`, periodic aura ticks (vmangos `Unit.cpp:4395-4443`).
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicAuraLog {
    pub target: u64,
    pub caster: u64,
    pub spell_id: u32,
    pub ticks: Vec<PeriodicTick>,
}

const AURA_PERIODIC_DAMAGE: u32 = 3;
const AURA_PERIODIC_HEAL: u32 = 8;
const AURA_OBS_MOD_HEALTH: u32 = 20;
const AURA_OBS_MOD_MANA: u32 = 21;
const AURA_PERIODIC_ENERGIZE: u32 = 24;
const AURA_PERIODIC_MANA_LEECH: u32 = 64;
const AURA_PERIODIC_DAMAGE_PERCENT: u32 = 89;

/// Read `SMSG_PERIODICAURALOG`; an unknown aura type errors, as its payload width is unknown.
pub(super) fn read_periodic_aura_log(r: &mut impl Read) -> io::Result<PeriodicAuraLog> {
    let target = read_packed_guid(r)?;
    let caster = read_packed_guid(r)?;
    let spell_id = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    // vmangos writes `count = 1` unconditionally (`Objects/Unit.cpp:4410`); 64 is far past it.
    let mut ticks = Vec::with_capacity(capacity_hint(count, 64));
    for _ in 0..count {
        let aura_type = read_u32_le(r)?;
        let tick = match aura_type {
            AURA_PERIODIC_DAMAGE | AURA_PERIODIC_DAMAGE_PERCENT => PeriodicTick::Damage {
                amount: read_u32_le(r)?,
                school: read_u32_le(r)?,
                absorb: read_u32_le(r)?,
                resist: read_i32_le(r)?,
            },
            AURA_PERIODIC_HEAL | AURA_OBS_MOD_HEALTH => PeriodicTick::Heal {
                amount: read_u32_le(r)?,
            },
            AURA_OBS_MOD_MANA | AURA_PERIODIC_ENERGIZE => PeriodicTick::Energize {
                power: read_u32_le(r)?,
                amount: read_u32_le(r)?,
            },
            AURA_PERIODIC_MANA_LEECH => PeriodicTick::ManaLeech {
                power: read_u32_le(r)?,
                amount: read_u32_le(r)?,
                multiplier: read_f32_le(r)?,
            },
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("SMSG_PERIODICAURALOG: unknown aura type {other}"),
                ))
            }
        };
        ticks.push(tick);
    }
    Ok(PeriodicAuraLog {
        target,
        caster,
        spell_id,
        ticks,
    })
}

/// `SMSG_SPELLHEALLOG`, a direct heal landing (`Server/Packets/Spell.cpp:105-112`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellHealLog {
    pub target: u64,
    pub healer: u64,
    pub spell_id: u32,
    pub amount: u32,
    pub critical: bool,
}

pub(super) fn read_spell_heal_log(r: &mut impl Read) -> io::Result<SpellHealLog> {
    Ok(SpellHealLog {
        target: read_packed_guid(r)?,
        healer: read_packed_guid(r)?,
        spell_id: read_u32_le(r)?,
        amount: read_u32_le(r)?,
        critical: read_u8(r)? != 0,
    })
}

/// `SMSG_SPELLENERGIZELOG`, an instant power gain (`Server/Packets/Spell.cpp:114-121`); `power` is
/// 0 mana, 1 rage, 2 focus, 3 energy, 4 happiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellEnergizeLog {
    pub target: u64,
    pub caster: u64,
    pub spell_id: u32,
    pub power: u32,
    pub amount: u32,
}

pub(super) fn read_spell_energize_log(r: &mut impl Read) -> io::Result<SpellEnergizeLog> {
    Ok(SpellEnergizeLog {
        target: read_packed_guid(r)?,
        caster: read_packed_guid(r)?,
        spell_id: read_u32_le(r)?,
        power: read_u32_le(r)?,
        amount: read_u32_le(r)?,
    })
}

/// `SMSG_SPELLDAMAGESHIELD`, a Thorns-style return hit (`Server/Packets/Combat.cpp:73-79`):
/// `victim` bears the shield, `attacker` struck it and takes this damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageShield {
    pub victim: u64,
    pub attacker: u64,
    pub damage: u32,
    pub school: u32,
}

pub(super) fn read_damage_shield(r: &mut impl Read) -> io::Result<DamageShield> {
    Ok(DamageShield {
        victim: read_u64_le(r)?,
        attacker: read_u64_le(r)?,
        damage: read_u32_le(r)?,
        school: read_u32_le(r)?,
    })
}

/// `SMSG_ENVIRONMENTALDAMAGELOG`, environmental damage taken (`Server/Packets/Combat.cpp:58-67`).
/// `damage_type` is 0 exhausted, 1 drowning, 2 fall, 3 lava, 4 slime, 5 fire (`Player.h:590`), the
/// row of `EnvironmentalDamage.dbc` whose visual kit plays (fall's is the landing dust puff).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvironmentalDamageLog {
    pub victim: u64,
    pub damage_type: u8,
    pub damage: u32,
    pub absorb: u32,
    pub resist: i32,
}

/// Read `SMSG_ENVIRONMENTALDAMAGELOG`; the victim guid is raw on both ends (client `0x4190b0`).
pub(super) fn read_environmental_damage_log(
    r: &mut impl Read,
) -> io::Result<EnvironmentalDamageLog> {
    Ok(EnvironmentalDamageLog {
        victim: read_u64_le(r)?,
        damage_type: read_u8(r)?,
        damage: read_u32_le(r)?,
        absorb: read_u32_le(r)?,
        resist: read_i32_le(r)?,
    })
}

/// `SMSG_SPELLLOGMISS`, a cast's per-target miss list (`Server/Packets/Spell.cpp:68-86`). Each
/// `u8` is a `SpellMissInfo` (`SpellDefines.h:160-174`): 1 miss, 2 resist, 3 dodge, 4 parry,
/// 5 block, 6 evade, 7 and 8 immune, 9 deflect, 10 absorb, 11 reflect.
#[derive(Debug, Clone, PartialEq)]
pub struct SpellLogMiss {
    pub spell_id: u32,
    pub caster: u64,
    /// The packet's extended byte: set, each entry carries two floats, and the client hands them
    /// to a hook and emits nothing (`0x5e7ec1`-`0x5e7f09`). vmangos never sets it.
    pub extended: bool,
    pub misses: Vec<(u64, u8)>,
}

pub(super) fn read_spell_log_miss(r: &mut impl Read) -> io::Result<SpellLogMiss> {
    let spell_id = read_u32_le(r)?;
    let caster = read_u64_le(r)?;
    let extended = read_u8(r)? != 0;
    let count = read_u32_le(r)?;
    // vmangos sends one entry at a time (`SpellCaster.cpp:663-673`), so 64 is an arbitrary cap.
    let mut misses = Vec::with_capacity(capacity_hint(count, 64));
    for _ in 0..count {
        let target = read_u64_le(r)?;
        let miss_info = read_u8(r)?;
        if extended {
            let _arg1 = read_f32_le(r)?;
            let _arg2 = read_f32_le(r)?;
        }
        misses.push((target, miss_info));
    }
    Ok(SpellLogMiss {
        spell_id,
        caster,
        extended,
        misses,
    })
}

/// `SMSG_PARTYKILLLOG`, the killing blow, sent to the killer's party (`Combat.cpp:52-56`): the
/// only source of "You have slain %s!"; "%s dies." comes from the client's own death handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartyKillLog {
    pub killer: u64,
    pub victim: u64,
}

pub(super) fn read_party_kill_log(r: &mut impl Read) -> io::Result<PartyKillLog> {
    Ok(PartyKillLog {
        killer: read_u64_le(r)?,
        victim: read_u64_le(r)?,
    })
}

/// `SMSG_SPELLINSTAKILLLOG`, an instant kill (vmangos `Spells/SpellEffects.cpp:274-279`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellInstaKillLog {
    pub victim: u64,
    pub spell_id: u32,
}

pub(super) fn read_spell_insta_kill_log(r: &mut impl Read) -> io::Result<SpellInstaKillLog> {
    Ok(SpellInstaKillLog {
        victim: read_u64_le(r)?,
        spell_id: read_u32_le(r)?,
    })
}

/// `SMSG_PROCRESIST` or `SMSG_SPELLORDAMAGE_IMMUNE`, which share one body
/// (`Server/Packets/Spell.cpp:88-102`). The 1.12 client reads `log_format` as the "is periodic"
/// flag that lets `CombatLogPeriodicSpells` gate the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellOutcomeLog {
    pub caster: u64,
    pub target: u64,
    pub spell_id: u32,
    pub log_format: u8,
}

pub(super) fn read_spell_outcome_log(r: &mut impl Read) -> io::Result<SpellOutcomeLog> {
    Ok(SpellOutcomeLog {
        caster: read_u64_le(r)?,
        target: read_u64_le(r)?,
        spell_id: read_u32_le(r)?,
        log_format: read_u8(r)?,
    })
}

/// `SMSG_SPELLDISPELLOG`, the auras a dispel removed (vmangos `SpellEffects.cpp:2524-2539`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellDispelLog {
    /// The unit the auras were on.
    pub victim: u64,
    /// The dispeller; the `AURADISPEL*` line never names them.
    pub caster: u64,
    pub spell_ids: Vec<u32>,
}

pub(super) fn read_spell_dispel_log(r: &mut impl Read) -> io::Result<SpellDispelLog> {
    let victim = read_packed_guid(r)?;
    let caster = read_packed_guid(r)?;
    let count = read_u32_le(r)?;
    let mut spell_ids = Vec::with_capacity(capacity_hint(count, 64));
    for _ in 0..count {
        spell_ids.push(read_u32_le(r)?);
    }
    Ok(SpellDispelLog {
        victim,
        caster,
        spell_ids,
    })
}

/// `SMSG_DISPEL_FAILED`, the auras a dispel failed to remove (`SpellEffects.cpp:2549-2555`). No
/// count: the spell ids run to the end of the body, as the 1.12 client reads them (`0x628c20`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispelFailed {
    pub caster: u64,
    pub victim: u64,
    pub spell_ids: Vec<u32>,
}

pub(super) fn read_dispel_failed(r: &mut impl Read) -> io::Result<DispelFailed> {
    let caster = read_u64_le(r)?;
    let victim = read_u64_le(r)?;
    let mut spell_ids = Vec::new();
    loop {
        let mut b = [0u8; 4];
        match r.read_exact(&mut b) {
            Ok(()) => spell_ids.push(u32::from_le_bytes(b)),
            // The end of the body is the list's only terminator.
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
    }
    Ok(DispelFailed {
        caster,
        victim,
        spell_ids,
    })
}

/// `SMSG_ENCHANTMENTLOG`, an enchant landing on or fading from an item
/// (`Server/Packets/Item.cpp:235-242`); a zero `caster` means it faded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantmentLog {
    pub caster: u64,
    pub owner: u64,
    pub item_entry: u32,
    /// A `SpellItemEnchantment.dbc` row, though vmangos names it `spellId`: every caller passes
    /// the item's enchantment id (`Objects/Item.cpp:1040`, `:1043`, `:1079`).
    pub enchant_id: u32,
    /// False on the owner's copy, true on everyone else's; the 1.12 client picks the
    /// enchant-added message by it.
    pub show_affiliation: bool,
}

pub(super) fn read_enchantment_log(r: &mut impl Read) -> io::Result<EnchantmentLog> {
    Ok(EnchantmentLog {
        caster: read_u64_le(r)?,
        owner: read_u64_le(r)?,
        item_entry: read_u32_le(r)?,
        enchant_id: read_u32_le(r)?,
        show_affiliation: read_u8(r)? != 0,
    })
}

/// One `SMSG_SPELLLOGEXECUTE` row, shaped by its effect id as vmangos writes it
/// (`Spells/Spell.cpp:4694-4771`) and the 1.12 client reads it (`0x5e8074`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExecuteLog {
    /// Effect 8 `POWER_DRAIN`: amount, then power (the 1.12 client, `0x5e807b` and `0x62793c`).
    PowerDrain {
        target: u64,
        amount: u32,
        power: u32,
        multiplier: f32,
    },
    /// Effects 10 `HEAL` / 62 `HEAL_MAX_HEALTH`.
    Heal {
        target: u64,
        amount: u32,
        critical: bool,
    },
    /// Effect 30 `ENERGIZE`.
    Energize {
        target: u64,
        amount: u32,
        power: u32,
    },
    /// Effect 19 `ADD_EXTRA_ATTACKS`: "You gain %d extra attacks through %s."
    ExtraAttacks { target: u64, count: u32 },
    /// Effect 24 `CREATE_ITEM`, the tradeskill line: no target guid, only the item entry.
    CreateItem { item_entry: u32 },
    /// Effect 68 `INTERRUPT_CAST`: the interrupted target and spell.
    InterruptCast { target: u64, spell_id: u32 },
    /// Effect 101 `FEED_PET`: an item entry alone.
    FeedPet { item_entry: u32 },
    /// Effect 111 `DURABILITY_DAMAGE`, both fields signed; `-1`, `-1` is the all-items form
    /// (`SPELLDURABILITYDAMAGEALL*`). The 1.12 client reads vmangos's `unk` as the item slot.
    DurabilityDamage {
        target: u64,
        item_entry: i32,
        slot: i32,
    },
    /// Every other effect vmangos logs: a bare target guid.
    Target { target: u64 },
}

/// `SMSG_SPELLLOGEXECUTE`, what a cast's effects did (`Spells/Spell.cpp:4662-4778`): one group of
/// rows per logging effect, keyed by the effect id that picks the combat-log line.
#[derive(Debug, Clone, PartialEq)]
pub struct SpellLogExecute {
    pub caster: u64,
    pub spell_id: u32,
    /// `(effect id, the rows that effect logged)`, in wire order.
    pub effects: Vec<(u32, Vec<ExecuteLog>)>,
}

// vmangos `SpellEffects.h` effect ids, named for the arms that read a payload wider than a guid.
const EFFECT_POWER_DRAIN: u32 = 8;
const EFFECT_HEAL: u32 = 10;
const EFFECT_ADD_EXTRA_ATTACKS: u32 = 19;
const EFFECT_CREATE_ITEM: u32 = 24;
const EFFECT_ENERGIZE: u32 = 30;
const EFFECT_HEAL_MAX_HEALTH: u32 = 62;
const EFFECT_INTERRUPT_CAST: u32 = 68;
const EFFECT_FEED_PET: u32 = 101;
const EFFECT_DURABILITY_DAMAGE: u32 = 111;

/// Read `SMSG_SPELLLOGEXECUTE`; an unknown effect errors, as its row width is not on the wire.
pub(super) fn read_spell_log_execute(r: &mut impl Read) -> io::Result<SpellLogExecute> {
    let caster = read_packed_guid(r)?;
    let spell_id = read_u32_le(r)?;
    let group_count = read_u32_le(r)?;
    let mut effects = Vec::with_capacity(capacity_hint(group_count, 8));
    for _ in 0..group_count {
        let effect = read_u32_le(r)?;
        let rows = read_u32_le(r)?;
        let mut out = Vec::with_capacity(capacity_hint(rows, 64));
        for _ in 0..rows {
            out.push(match effect {
                EFFECT_POWER_DRAIN => ExecuteLog::PowerDrain {
                    target: read_u64_le(r)?,
                    amount: read_u32_le(r)?,
                    power: read_u32_le(r)?,
                    multiplier: read_f32_le(r)?,
                },
                EFFECT_HEAL | EFFECT_HEAL_MAX_HEALTH => ExecuteLog::Heal {
                    target: read_u64_le(r)?,
                    amount: read_u32_le(r)?,
                    critical: read_u8(r)? != 0,
                },
                EFFECT_ENERGIZE => ExecuteLog::Energize {
                    target: read_u64_le(r)?,
                    amount: read_u32_le(r)?,
                    power: read_u32_le(r)?,
                },
                EFFECT_ADD_EXTRA_ATTACKS => ExecuteLog::ExtraAttacks {
                    target: read_u64_le(r)?,
                    count: read_u32_le(r)?,
                },
                EFFECT_CREATE_ITEM => ExecuteLog::CreateItem {
                    item_entry: read_u32_le(r)?,
                },
                EFFECT_INTERRUPT_CAST => ExecuteLog::InterruptCast {
                    target: read_u64_le(r)?,
                    spell_id: read_u32_le(r)?,
                },
                EFFECT_FEED_PET => ExecuteLog::FeedPet {
                    item_entry: read_u32_le(r)?,
                },
                EFFECT_DURABILITY_DAMAGE => ExecuteLog::DurabilityDamage {
                    target: read_u64_le(r)?,
                    item_entry: read_i32_le(r)?,
                    slot: read_i32_le(r)?,
                },
                _ if rows_are_guid_only(effect) => ExecuteLog::Target {
                    target: read_u64_le(r)?,
                },
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("SMSG_SPELLLOGEXECUTE: unknown effect {other}"),
                    ))
                }
            });
        }
        effects.push((effect, out));
    }
    Ok(SpellLogExecute {
        caster,
        spell_id,
        effects,
    })
}

/// The effects vmangos logs as a bare target guid (`Spells/Spell.cpp:4735-4770`).
fn rows_are_guid_only(effect: u32) -> bool {
    matches!(
        effect,
        1     // INSTAKILL
            | 18  // RESURRECT
            | 28  // SUMMON
            | 33  // OPEN_LOCK
            | 38  // DISPEL
            | 41  // SUMMON_WILD
            | 42  // SUMMON_GUARDIAN
            | 50  // TRANS_DOOR
            | 56  // SUMMON_PET
            | 59  // OPEN_LOCK_ITEM
            | 63  // THREAT
            | 69  // DISTRACT
            | 73  // SUMMON_POSSESSED
            | 74  // SUMMON_TOTEM
            | 76  // SUMMON_OBJECT_WILD
            | 79  // SANCTUARY
            | 87..=90 // SUMMON_TOTEM_SLOT1..4
            | 91  // THREAT_ALL
            | 97  // SUMMON_CRITTER
            | 102 // DISMISS_PET
            | 104..=107 // SUMMON_OBJECT_SLOT1..4
            | 108 // DISPEL_MECHANIC
            | 112 // SUMMON_DEMON
            | 113 // RESURRECT_NEW
            | 114 // ATTACK_ME
            | 116 // SKIN_PLAYER_CORPSE
            | 125 // MODIFY_THREAT_PERCENT
            | 126 // SPELL_EFFECT_126
    )
}
