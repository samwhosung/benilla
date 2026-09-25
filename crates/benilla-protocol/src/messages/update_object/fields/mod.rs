use std::io::{self, Read};

use crate::wire::{capacity_hint, read_u32_le, read_u8, Vector3d};

use super::movement::ObjectType;

// Descriptor field indices for build 5875.
const FIELD_OBJECT_TYPE: u16 = 2;
const FIELD_OBJECT_SCALE_X: u16 = 4;
// The creator of a spell-spawned object (bobber, ritual portal); 0 for a world spawn.
const FIELD_GAMEOBJECT_CREATED_BY: u16 = 6;
const FIELD_GAMEOBJECT_DISPLAYID: u16 = 8;
const FIELD_GAMEOBJECT_FLAGS: u16 = 9;
// The spawn's rotation quaternion (x, y, z, w), four floats.
const FIELD_GAMEOBJECT_ROTATION: u16 = 10;
pub const FIELD_GAMEOBJECT_STATE: u16 = 14;
const FIELD_GAMEOBJECT_POS_X: u16 = 15;
const FIELD_GAMEOBJECT_POS_Y: u16 = 16;
const FIELD_GAMEOBJECT_POS_Z: u16 = 17;
const FIELD_GAMEOBJECT_FACING: u16 = 18;
// The client reads it at GameObject block byte 0x34. vmangos `UpdateFields_1_5_1.h` adds a
// `GAMEOBJECT_TIMESTAMP` that would make it 20, but that is not the 5875 layout.
const FIELD_GAMEOBJECT_DYN_FLAGS: u16 = 19;
// The client reads it at `[go+0x110]+0x38`, between DYN_FLAGS and TYPE_ID.
const FIELD_GAMEOBJECT_FACTION: u16 = 20;
const FIELD_GAMEOBJECT_TYPE_ID: u16 = 21;
// OBJECT_END + 0x10 (vmangos `UpdateFields_1_12_1.h:317`).
const FIELD_GAMEOBJECT_LEVEL: u16 = 22;
/// `CORPSE_FIELD_DYNAMIC_FLAGS`; bit 0 is lootable insignia. On a unit, index 36 is
/// `UNIT_FIELD_BYTES_0`, so a field edge carries its object class.
pub const FIELD_CORPSE_DYNAMIC_FLAGS: u16 = 36;
// UNIT fields. The client reads FLAGS, COMBATREACH, DYNAMIC_FLAGS and NPC_FLAGS at unit block
// bytes 0xa0, 0x1f0, 0x224 and 0x234.
/// The unit's target; the client turns an idle unit to face it, and no packet carries that facing.
const FIELD_UNIT_TARGET: u16 = 16;
/// The unit this one summoned (`UpdateFields_1_12_1.h:42`); on us, the `"pet"` unit. The pet bar
/// reads its guid off `SMSG_PET_SPELLS`, so the two can disagree briefly around a summon.
const FIELD_UNIT_SUMMON: u16 = 8;
/// The charmer, 0 for none. The attack-start check `0x612df0` refuses a swing when it is set and
/// is not us (`ERR_ATTACK_CHARMED`); `0x5ee5a0` and `0x5ff580` prefer it as the owner.
const FIELD_UNIT_CHARMEDBY: u16 = 10;
/// The summoner of a pet, guardian or totem; with CREATEDBY, the "owned by me" test of `0x5efea0`.
const FIELD_UNIT_SUMMONEDBY: u16 = 12;
const FIELD_UNIT_CREATEDBY: u16 = 14;
/// The channel's target, possibly the caster (`UpdateFields_1_12_1.h:48`). Public, so another
/// unit's channel renders from it; `MSG_CHANNEL_START`/`UPDATE` reach only the caster.
const FIELD_UNIT_CHANNEL_OBJECT: u16 = 20;
pub const FIELD_UNIT_HEALTH: u16 = 22;
/// Five power slots: mana, rage, focus, energy, happiness; `MAXPOWER1..5` follow `MAXHEALTH`.
const FIELD_UNIT_POWER1: u16 = 23;
pub const FIELD_UNIT_MAXHEALTH: u16 = 28;
const FIELD_UNIT_MAXPOWER1: u16 = 29;
pub const FIELD_UNIT_LEVEL: u16 = 34;
pub const FIELD_UNIT_FACTIONTEMPLATE: u16 = 35;
const FIELD_UNIT_BYTES_0: u16 = 36;
pub const FIELD_UNIT_BYTES_1: u16 = 138;
pub const FIELD_UNIT_FLAGS: u16 = 46;
// The aura block: four public parallel arrays (`UpdateFields_1_12_1.h:67-70`), which the client
// reads at unit block bytes 0xa4 and 0x164 onward. Duration reaches only the aura's own target,
// over `SMSG_UPDATE_AURA_DURATION`, and no packet carries the caster.
pub const FIELD_UNIT_AURA: u16 = 47;
/// Nibble-packed, 8 slots per `u32` (`SpellAuraHolder::SetAuraFlag`, `SpellAuras.cpp:7456-7462`).
pub const FIELD_UNIT_AURAFLAGS: u16 = 95;
/// Byte-packed, 4 slots per `u32`: the caster's level (`SetAuraLevel`, `SpellAuras.cpp:7484`).
const FIELD_UNIT_AURALEVELS: u16 = 101;
/// Byte-packed, 4 slots per `u32`, holding `stack - 1` (`SpellAuras.cpp:7500-7507`).
pub const FIELD_UNIT_AURAAPPLICATIONS: u16 = 113;
/// Aura-state bits, tested as `1 << (state - 1)` by the usable check (client `[unit+0x110]+0x1dc`).
const FIELD_UNIT_AURASTATE: u16 = 125;
/// Horizontal bounding radius, in yards.
const FIELD_UNIT_BOUNDINGRADIUS: u16 = 129;
const FIELD_UNIT_COMBATREACH: u16 = 130;
const FIELD_UNIT_BASE_MANA: u16 = 162;
const FIELD_UNIT_DISPLAYID: u16 = 131;
/// The unshifted appearance, untouched by forms, morphs and polymorph (`UpdateFields_1_12_1.h:77`).
/// The client sizes the mover collision box from it (`0x60b270`), so a shapeshift keeps the box.
const FIELD_UNIT_NATIVEDISPLAYID: u16 = 132;
/// The ridden mount's `CreatureDisplayInfo` id, 0 when unmounted; this field, not the aura, is the
/// mounted state (`Unit::IsMounted`; client `[unit+0x110]+0x1fc`).
pub const FIELD_UNIT_MOUNTDISPLAYID: u16 = 133;
/// Nonzero on a pet or charm; the rank getter `0x605620` then forces rank 0, so an enslaved mob
/// shows no elite dragon, tooltip rank word or boss skull.
const FIELD_UNIT_PETNUMBER: u16 = 139;
/// Unix time of the pet's last rename (`Pet.cpp:285`). The name itself comes from
/// `CMSG_PET_NAME_QUERY`, cached by pet number, so a change here invalidates that cache.
const FIELD_UNIT_PET_NAME_TIMESTAMP: u16 = 140;
/// With the next field, the `(currXP, nextXP)` pair `GetPetExperience` (`0x4be840`) returns; the
/// client reads both unsigned.
const FIELD_UNIT_PETEXPERIENCE: u16 = 141;
const FIELD_UNIT_PETNEXTLEVELEXP: u16 = 142;
/// Two `u16`s: `GetPetTrainingPoints` (`0x4be790`) returns the high word first, which
/// `PetPaperDollFrame.lua` names `totalPoints`, then the low word as `spent`.
const FIELD_UNIT_TRAINING_POINTS: u16 = 149;
pub const FIELD_UNIT_DYNAMIC_FLAGS: u16 = 143;
/// The spell being channeled, 0 for none (`UpdateFields_1_12_1.h:89`).
pub const FIELD_UNIT_CHANNEL_SPELL: u16 = 144;
/// The summoning spell, 0 for none; the first gate of the client's feed-pet path `0x6ea1e0`.
const FIELD_UNIT_CREATED_BY_SPELL: u16 = 146;
pub const FIELD_UNIT_NPC_FLAGS: u16 = 147;
/// The looping state emote, an `Emotes.dbc` id or 0; on a player, the server's echo of a
/// state-class emote (`ChatHandler.cpp:738`).
const FIELD_UNIT_NPC_EMOTESTATE: u16 = 148;
/// A creature's weapon display ids (main hand, off hand, ranged), with no item behind them
/// (`Creature.cpp:4158-4181`).
const FIELD_UNIT_VIRTUAL_ITEM_SLOT_DISPLAY: u16 = 37;
/// Two dwords per weapon slot: class, subclass, material and inventory type bytes, then the
/// sheath byte (`CreatureDefines.h:621-628`).
const FIELD_UNIT_VIRTUAL_ITEM_INFO: u16 = 40;
/// Byte 0 is the sheath state (`UnitDefines.h:93`); a player's comes only from `CMSG_SETSHEATHED`.
/// Not `FIELD_PLAYER_BYTES_2`.
const FIELD_UNIT_BYTES_2: u16 = 164;
// UNIT combat and stat block: offset from OBJECT_END, then the wire type.
const FIELD_UNIT_BASEATTACKTIME: u16 = 126; // OBJECT_END+0x78; 2 slots [main, offhand], ms, INT
const FIELD_UNIT_RANGEDATTACKTIME: u16 = 128; // +0x7A, INT
const FIELD_UNIT_MINDAMAGE: u16 = 134; // +0x80, FLOAT
const FIELD_UNIT_MAXDAMAGE: u16 = 135; // +0x81, FLOAT
const FIELD_UNIT_MINOFFHANDDAMAGE: u16 = 136; // +0x82, FLOAT
const FIELD_UNIT_MAXOFFHANDDAMAGE: u16 = 137; // +0x83, FLOAT
const FIELD_UNIT_STAT0: u16 = 150; // +0x90 ×5, INT
const FIELD_UNIT_RESISTANCES: u16 = 155; // +0x95 ×7, INT; [0] = armor
const FIELD_UNIT_ATTACK_POWER: u16 = 165; // +0x9F, INT
/// +0xA0, `TWO_SHORT`: signed halves, the low positive, the high already negative or zero
/// (`StatSystem.cpp:335-336`).
const FIELD_UNIT_ATTACK_POWER_MODS: u16 = 166;
const FIELD_UNIT_ATTACK_POWER_MULTIPLIER: u16 = 167; // +0xA1, FLOAT: multiplier − 1.0
const FIELD_UNIT_RANGED_ATTACK_POWER: u16 = 168; // +0xA2, INT
const FIELD_UNIT_RANGED_ATTACK_POWER_MODS: u16 = 169; // +0xA3, TWO_SHORT as above
const FIELD_UNIT_RANGED_ATTACK_POWER_MULTIPLIER: u16 = 170; // +0xA4, FLOAT
const FIELD_UNIT_MINRANGEDDAMAGE: u16 = 171; // +0xA5, FLOAT
const FIELD_UNIT_MAXRANGEDDAMAGE: u16 = 172; // +0xA6, FLOAT

/// Aura slots per unit (vmangos `MAX_AURAS`, `SpellAuraDefines.h:25`).
pub const UNIT_AURA_SLOTS: u8 = 48;
/// Slots below this hold buffs, the rest debuffs (`SpellAuraDefines.h:26`). Passives get no slot,
/// so the client renders the array unfiltered (`SpellAuras.cpp:6715-6736`).
pub const UNIT_AURA_POSITIVE_SLOTS: u8 = 32;

/// `AFLAG_CANCELABLE`: set on a positive aura without `SPELL_ATTR_NO_AURA_CANCEL`, the same test
/// the `CMSG_CANCEL_AURA` handler makes (`SpellAuras.cpp:7467`).
pub const AURA_FLAG_CANCELABLE: u8 = 0x01;
/// `AFLAG_EFF_INDEX_0|1|2`, the client's aura liveness test: a cleared slot can keep a stale spell
/// id, so a live aura is one with any of these bits set.
pub const AURA_FLAG_EFF_INDEX_MASK: u8 = 0x0E;

/// One occupied aura slot, from the four `UNIT_FIELD_AURA*` arrays; duration and caster are not
/// on the descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitAuraSlot {
    /// Also the buff or debuff class, and the key `SMSG_UPDATE_AURA_DURATION` uses.
    pub slot: u8,
    pub spell_id: u32,
    /// The raw `UNIT_FIELD_AURAFLAGS` nibble.
    pub flags: u8,
    /// The caster's level at apply time, the only trace of the caster.
    pub level: u8,
    /// At least 1; the wire byte holds `stack - 1`.
    pub stacks: u8,
}

impl UnitAuraSlot {
    /// A buff: the slot is in the positive half.
    pub fn is_helpful(&self) -> bool {
        self.slot < UNIT_AURA_POSITIVE_SLOTS
    }
    /// Whether the server will honour a `CMSG_CANCEL_AURA` for this aura.
    pub fn is_cancelable(&self) -> bool {
        self.flags & AURA_FLAG_CANCELABLE != 0
    }
}

// PLAYER fields start at UNIT_END (188); the client decodes appearance bytes at `0x5fb200`.
const FIELD_PLAYER_BYTES: u16 = 193;
const FIELD_PLAYER_BYTES_2: u16 = 194;
// The low u16 is `gender | (drunk & 0xFFFE)`, so byte 1 is the drunk level the client reads at
// `[[unit+0xe68]+0x1d]`; byte 2 is the city-protector race, byte 3 the current honor rank
// (`Player.h:351-356`). Public: the one honor value that streams for every visible player.
const FIELD_PLAYER_BYTES_3: u16 = 195;
// 20 slots of 3 fields (`Player.h:439-444`): the quest id (group-only), then the counters and
// state byte, and the timer (both private).
pub const FIELD_PLAYER_QUEST_LOG_1_1: u16 = 198;

/// The indices a field watch names: the same constants the accessors read, so the two agree.
pub mod field {
    /// The bit a watcher tests on a raw `UNIT_DYNAMIC_FLAGS` edge (feign death).
    pub use super::unit::UNIT_DYNFLAG_DEAD;
    pub use super::{
        FIELD_CORPSE_DYNAMIC_FLAGS, FIELD_GAMEOBJECT_STATE, FIELD_PLAYER_FIELD_COINAGE,
        FIELD_PLAYER_FLAGS, FIELD_PLAYER_INV_SLOT_HEAD, FIELD_PLAYER_QUEST_LOG_1_1,
        FIELD_PLAYER_SKILL_INFO_1_1, FIELD_UNIT_AURA, FIELD_UNIT_AURAAPPLICATIONS,
        FIELD_UNIT_AURAFLAGS, FIELD_UNIT_BYTES_1, FIELD_UNIT_CHANNEL_SPELL,
        FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH,
        FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH, FIELD_UNIT_MOUNTDISPLAYID, FIELD_UNIT_NPC_FLAGS,
    };
}

/// `MAX_QUEST_LOG_SIZE` (`QuestDef.h:34`).
pub const PLAYER_QUEST_LOG_SLOTS: u8 = 20;

/// One `PLAYER_QUEST_LOG` slot, the durable quest state; `SMSG_QUESTUPDATE_*` are only toasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestLogSlot {
    /// 0 for an empty slot.
    pub quest_id: u32,
    /// Four 6-bit kill, cast and interact counters at bits `6i..6i+6` (`Player.h:1100-1106`); the
    /// client counts item objectives from the bags itself.
    pub counters: [u8; 4],
    /// Byte 3 of the counter field (`Player.h:1107`): a [`quest_slot_state`] bit, 0 in progress.
    pub state: u8,
    /// The absolute end time of a timed quest, else 0.
    pub timer: u32,
}

/// `QUEST_STATE_*` bits of [`QuestLogSlot::state`] (`Player.h:447-451`).
pub mod quest_slot_state {
    pub const COMPLETE: u8 = 0x01;
    pub const FAIL: u8 = 0x02;
}
// Inventory fields. The PLAYER slot arrays are private, and each slot is a 2-field guid.
const FIELD_ITEM_STACK_COUNT: u16 = 14; // OBJECT_END + 0x8
const FIELD_ITEM_ENCHANTMENT: u16 = 22; // OBJECT_END + 0x10; 7 slots × 3 (id, duration, charges)
const FIELD_CONTAINER_NUM_SLOTS: u16 = 48; // ITEM_END = 6 + 0x2A
const FIELD_CONTAINER_SLOT_1: u16 = 50; // ITEM_END + 0x2; 36 slots × 2

// From the CONTAINER block on, the hex comments in vmangos `UpdateFields_1_12_1.h` run 6 low. The
// indices here follow its enum arithmetic, which the server compiles: INV_SLOT_HEAD = 188 + 0x12A.
const FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR: u16 = 258; // UNIT_END + 0x46; 12 fields per slot
pub const FIELD_PLAYER_INV_SLOT_HEAD: u16 = 486; // 23 slots × 2 (equipment 0–18, bags 19–22)
const FIELD_PLAYER_PACK_SLOT_1: u16 = 532; // 16 slots × 2 (the backpack)
const FIELD_PLAYER_BANK_SLOT_1: u16 = 564; // 24 slots × 2
const FIELD_PLAYER_BANK_BAG_SLOT_1: u16 = 612; // 564 + 24×2; 6 bag slots × 2 (item guids)
const FIELD_PLAYER_VENDORBUYBACK_SLOT_1: u16 = 624; // 12 slots × 2 (item guids)
const FIELD_PLAYER_KEYRING_SLOT_1: u16 = 648; // 32 slots × 2 (item guids), wire slots 81–112
const FIELD_PLAYER_FARSIGHT: u16 = 712; // our view's anchor (Mind Vision, Sentry Totem), or 0
const FIELD_PLAYER_BUYBACK_PRICE_1: u16 = 1226; // 12 × u32 copper, indexed slot−69
const FIELD_PLAYER_BUYBACK_TIMESTAMP_1: u16 = 1238; // 12 × u32, the client's sort key only
pub const FIELD_PLAYER_FIELD_COINAGE: u16 = 1176; // copper
const FIELD_PLAYER_XP: u16 = 716;
const FIELD_PLAYER_NEXT_LEVEL_XP: u16 = 717;
// The watched reputation slot, signed: slot 0 is a real faction, so only -1 means none.
const FIELD_PLAYER_WATCHED_FACTION_INDEX: u16 = 1261;
// The rested pool in base kill-XP units: a kill drains it 1:1 while granting +100%
// (`Player::GetXPRestBonus`), so the doubled span on the XP bar is twice this value.
const FIELD_PLAYER_REST_STATE_EXPERIENCE: u16 = 1175;
// EXPLORED_ZONES_1 is the discovery bitset, 2048 bits indexed by `AreaTable.dbc` exploreFlag. Just
// below it sit block, dodge, parry and crit chance (FLOAT, UNIT_END + 0x396..0x399).
const FIELD_PLAYER_BLOCK_PERCENTAGE: u16 = 1106;
const FIELD_PLAYER_DODGE_PERCENTAGE: u16 = 1107;
const FIELD_PLAYER_PARRY_PERCENTAGE: u16 = 1108;
const FIELD_PLAYER_CRIT_PERCENTAGE: u16 = 1109;
const FIELD_PLAYER_EXPLORED_ZONES_1: u16 = 1111;
/// The bitset's slot count (`Size: 64` in the server enum).
pub const PLAYER_EXPLORED_ZONES_SLOTS: u16 = 64;
// PLAYER stat block. POSSTAT, NEGSTAT and the resistance buff mods are floats in vmangos but go out
// as INT (`Object::BuildValuesUpdate` narrows them). Read them signed: the server's cast of a
// negative float wraps on x86 and saturates to 0 on aarch64.
const FIELD_PLAYER_POSSTAT0: u16 = 1177; // UNIT_END+0x3DD ×5, INT
const FIELD_PLAYER_NEGSTAT0: u16 = 1182; // ×5, INT; negative-or-zero where the wire can carry it
const FIELD_PLAYER_RESISTANCEBUFFMODSPOSITIVE: u16 = 1187; // ×7, INT
const FIELD_PLAYER_RESISTANCEBUFFMODSNEGATIVE: u16 = 1194; // ×7, INT; negative-or-zero
const FIELD_PLAYER_MOD_DAMAGE_DONE_POS: u16 = 1201; // ×7 schools ([0] physical), INT
const FIELD_PLAYER_MOD_DAMAGE_DONE_NEG: u16 = 1208; // ×7, INT; negative-or-zero
const FIELD_PLAYER_MOD_DAMAGE_DONE_PCT: u16 = 1215; // ×7, FLOAT (header says INT), default 1.0
                                                    // (`Player.cpp:3336`, `Player.cpp:7274`)
pub const FIELD_PLAYER_SKILL_INFO_1_1: u16 = 718; // ×384: 128 skills × 3 dwords

// Unspent talent points and free primary professions, `UnitCharacterPoints("player")`.
const FIELD_PLAYER_CHARACTER_POINTS1: u16 = 1102;
const FIELD_PLAYER_CHARACTER_POINTS2: u16 = 1103;
// Tracking masks the minimap tests: bit `1 << (n - 1)` for creature type or `LockType.dbc` id `n`,
// one bit per active tracking aura.
const FIELD_PLAYER_TRACK_CREATURES: u16 = 1104;
const FIELD_PLAYER_TRACK_RESOURCES: u16 = 1105;
const FIELD_PLAYER_AMMO_ID: u16 = 1223; // UNIT_END+0x40B, INT: the equipped ammo item id
const FIELD_PLAYER_SELF_RES_SPELL: u16 = 1224;
// Bit 0x10 is PLAYER_FLAGS_GHOST (`Player.h:319`), held by the ghost aura 8326 until resurrection.
pub const FIELD_PLAYER_FLAGS: u16 = 190;
// The client's player block base `[player+0xe68]` is field 188: the arbiter at +0x0, PLAYER_FLAGS
// at +0x8 and the duel team at +0x20, the three reads of `UnitReaction`'s duel leg (`0x6061e0`).
const FIELD_PLAYER_DUEL_ARBITER: u16 = 188;
const FIELD_PLAYER_DUEL_TEAM: u16 = 196;
// Public, so `GetGuildInfo(unit)` (`0x4c9330`, reading block +0xc and +0x10) answers for any
// visible player; a guild id of 0 is guildless.
const FIELD_PLAYER_GUILDID: u16 = 191;
const FIELD_PLAYER_GUILDRANK: u16 = 192;
// `GetComboPoints` (`0x51a190`) reads the target at block +0x838 and the count byte at +0x1029.
const FIELD_PLAYER_FIELD_COMBO_TARGET: u16 = 714;
const FIELD_PLAYER_FIELD_BYTES: u16 = 1222; // UNIT_END+0x40A: flags, combo points, action bars,
                                            // highest honor rank; not the appearance PLAYER_BYTES

// The honor block (`UpdateFields_1_12_1.h:288-298`) is private: another player's honor needs
// `MSG_INSPECT_HONOR_STATS`. The four kill counters are TWO_SHORT (honorable, dishonorable), but
// vmangos writes all except SESSION_KILLS as a whole dword. LAST_WEEK_RANK is the weekly standing,
// not a rank. BYTES2 byte 0 is the rank bar (`Player.h:366-372`).
const FIELD_PLAYER_FIELD_SESSION_KILLS: u16 = 1250;
const FIELD_PLAYER_FIELD_YESTERDAY_KILLS: u16 = 1251;
const FIELD_PLAYER_FIELD_LAST_WEEK_KILLS: u16 = 1252;
const FIELD_PLAYER_FIELD_THIS_WEEK_KILLS: u16 = 1253;
const FIELD_PLAYER_FIELD_THIS_WEEK_CONTRIBUTION: u16 = 1254;
const FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS: u16 = 1255;
const FIELD_PLAYER_FIELD_LIFETIME_DISHONORABLE_KILLS: u16 = 1256;
const FIELD_PLAYER_FIELD_YESTERDAY_CONTRIBUTION: u16 = 1257;
const FIELD_PLAYER_FIELD_LAST_WEEK_CONTRIBUTION: u16 = 1258;
const FIELD_PLAYER_FIELD_LAST_WEEK_RANK: u16 = 1259;
const FIELD_PLAYER_FIELD_BYTES2: u16 = 1260;

/// `PLAYER_SKILL_INFO` slots on the wire, 3 fields each; vmangos fills only 127 (`Player.h:69`).
pub const PLAYER_SKILL_SLOTS: u8 = 128;

/// One `PLAYER_SKILL_INFO` slot, three dwords (`Player.cpp:90-99`): `id | step << 16`,
/// `value | max << 16`, `temp_bonus | perm_bonus << 16` with signed bonuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerSkillSlot {
    /// A `SkillLine.dbc` id, 0 for an unused slot.
    pub skill_id: u16,
    /// The tier step, 0 unless a tier-raising effect is live.
    pub step: u16,
    pub value: u16,
    pub max: u16,
    /// From auras, consumables and enchants; a malus is negative.
    pub temp_bonus: i16,
    /// From talents.
    pub perm_bonus: i16,
}

/// A corpse's own appearance, snapshotted at death: the seven `CORPSE_FIELD_BYTES_1`/`_2` bytes
/// the client's dress `0x5d6260` loads, not the owner's live `PLAYER_BYTES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorpseLook {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

/// A sparse update-field set: the wire's block mask and a dense value array. A CREATE carries only
/// nonzero fields (`Object::_SetCreateBits`) into the client's zeroed buffer, so an absent field
/// within the created type's descriptor reads `Some(0)`; in a `Values` delta, or past the
/// descriptor's end, it reads `None`.
#[derive(Debug, Clone, Default)]
pub struct ObjectFields {
    /// The wire's block mask, verbatim (`index = word * 32 + bit`).
    present: Vec<u32>,
    /// Always `present.len() * 32` long; an unset slot holds 0 but must be read through the mask.
    values: Vec<u32>,
    /// The created type's [`descriptor_len`], below which absent reads 0; 0 for a delta or fixture.
    descriptor_end: u16,
}

/// An object's descriptor length in dwords: the `*_END` of its innermost block, so a Player spans
/// OBJECT, UNIT and PLAYER.
fn descriptor_len(object_type: ObjectType) -> u16 {
    match object_type {
        ObjectType::Object => 6,         // OBJECT_END = 0x6
        ObjectType::Item => 48,          // ITEM_END = OBJECT_END + 0x2A
        ObjectType::Container => 122,    // CONTAINER_END = ITEM_END + 0x4A
        ObjectType::Unit => 188,         // UNIT_END = OBJECT_END + 0xB6
        ObjectType::Player => 1282,      // PLAYER_END = UNIT_END + 0x446
        ObjectType::GameObject => 26,    // GAMEOBJECT_END = OBJECT_END + 0x14
        ObjectType::DynamicObject => 16, // DYNAMICOBJECT_END = OBJECT_END + 0xA
        ObjectType::Corpse => 38,        // CORPSE_END = OBJECT_END + 0x20
    }
}

/// Mask words of the widest descriptor, `PLAYER_END`; no 1.12 object needs more.
const MAX_MASK_WORDS: usize = 1282usize.div_ceil(32);

impl ObjectFields {
    pub(super) fn read(r: &mut impl Read) -> io::Result<Self> {
        let amount_of_blocks = read_u8(r)?;
        let mut present = Vec::with_capacity(capacity_hint(amount_of_blocks, MAX_MASK_WORDS));
        for _ in 0..amount_of_blocks {
            present.push(read_u32_le(r)?);
        }
        let mut values = vec![0u32; present.len() * 32];
        for (word, block) in present.iter().enumerate() {
            for bit in 0..32 {
                if block & (1u32 << bit) != 0 {
                    values[word * 32 + bit] = read_u32_le(r)?;
                }
            }
        }
        Ok(Self {
            present,
            values,
            descriptor_end: 0,
        })
    }

    /// Marks a CREATE snapshot of `object_type`, so absent fields in its descriptor read 0.
    pub fn into_created(mut self, object_type: ObjectType) -> Self {
        self.descriptor_end = descriptor_len(object_type);
        self
    }

    /// A fixture from `(index, value)` pairs; [`Self::into_created`] makes it a create.
    pub fn from_pairs(pairs: &[(u16, u32)]) -> Self {
        let mut this = Self::default();
        for &(index, value) in pairs {
            this.insert(index, value);
        }
        this
    }

    /// `CORPSE_FIELD_OWNER` (`UpdateFields_1_12_1.h:339`): the dead player, only on a corpse.
    pub fn corpse_owner(&self) -> Option<u64> {
        self.get_guid(6).filter(|&g| g != 0)
    }

    /// The body's `CreatureDisplayInfo` id, the owner's native display (`Player.cpp:4809`). A bone
    /// pile keeps it, but `0x5d6700` ignores it there and builds the skeleton from race and sex.
    pub fn corpse_display_id(&self) -> Option<u32> {
        self.get_u32(12).filter(|&d| d != 0)
    }

    /// The piece in equipment slot `slot` (0..18) as `(ItemDisplayInfo id, InventoryType)`, packed
    /// `display | type << 24` (`Player.cpp:4822`); unlike `PLAYER_VISIBLE_ITEM`, not an item entry.
    pub fn corpse_item(&self, slot: u8) -> Option<(u32, u8)> {
        let raw = self.get_u32(13 + u16::from(slot))?;
        let display = raw & 0x00ff_ffff;
        (display != 0).then_some((display, (raw >> 24) as u8))
    }

    /// The owner's guild id at death, 0 for none; the client builds the corpse's tabard crest from
    /// it (`0x5d6ec0`).
    pub fn corpse_guild(&self) -> u32 {
        self.get_u32(34).unwrap_or(0)
    }

    /// Race, gender and skin in bytes 1..3; byte 0 is unused (`Corpse.cpp:228`).
    fn corpse_bytes_1(&self) -> Option<u32> {
        self.get_u32(32)
    }
    /// Face, hair style, hair colour and facial hair (`Corpse.cpp:229`).
    fn corpse_bytes_2(&self) -> Option<u32> {
        self.get_u32(33)
    }
    /// The corpse's appearance, `None` in a delta that lacks either BYTES word.
    pub fn corpse_look(&self) -> Option<CorpseLook> {
        let (b1, b2) = (self.corpse_bytes_1()?, self.corpse_bytes_2()?);
        Some(CorpseLook {
            race: (b1 >> 8) as u8,
            sex: (b1 >> 16) as u8,
            skin: (b1 >> 24) as u8,
            face: b2 as u8,
            hair_style: (b2 >> 8) as u8,
            hair_color: (b2 >> 16) as u8,
            facial_hair: (b2 >> 24) as u8,
        })
    }

    /// `CORPSE_FIELD_FLAGS`, 0 when absent (client `[[corpse+0x110]+0x74]`).
    pub fn corpse_flags(&self) -> u32 {
        self.get_u32(35).unwrap_or(0)
    }
    /// [`Self::corpse_flags`] keeping absence: untouched flags in a delta are `None`, not all
    /// clear. A reader acting on a change must use this one.
    pub fn corpse_flags_present(&self) -> Option<u32> {
        self.get_u32(35)
    }
    /// `CORPSE_FLAG_BONES`: a bone pile, which `0x5d6260` tests first; it wears nothing and takes
    /// its model from race and sex.
    pub fn corpse_is_bones(&self) -> bool {
        self.corpse_flags() & 0x01 != 0
    }
    /// `CORPSE_FLAG_HIDE_HELM`, the owner's setting at death; the client skips slot 0 (`0x5d6465`).
    pub fn corpse_hides_helm(&self) -> bool {
        self.corpse_flags() & 0x08 != 0
    }
    /// `CORPSE_FLAG_HIDE_CLOAK`: the same for slot 14 (`0x5d6470`).
    pub fn corpse_hides_cloak(&self) -> bool {
        self.corpse_flags() & 0x10 != 0
    }

    /// `CORPSE_DYNFLAG_LOOTABLE` (`Map.cpp:3655`): the bone pile has insignia; `0x5d6e20` gates the
    /// loot highlight and the `CMSG_LOOT` click on it.
    pub fn corpse_lootable(&self) -> bool {
        self.get_u32(FIELD_CORPSE_DYNAMIC_FLAGS).unwrap_or(0) & 0x01 != 0
    }

    /// `CORPSE_FLAG_LOOTABLE`: the battleground insignia is takeable, by spell 22027 "Remove
    /// Insignia". The cursor `0x482740` and right-click `0x5d6bf0` test it; the corpse's PvP flag
    /// is bit 2 (`UnitIsPVP`, `0x516460`).
    pub fn corpse_pvp_insignia(&self) -> bool {
        self.corpse_flags() & 0x20 != 0
    }

    /// The ground caster (`UpdateFields_1_12_1.h:325`), at the same index as a corpse's owner.
    pub fn dynamicobject_caster(&self) -> Option<u64> {
        self.get_guid(6).filter(|&g| g != 0)
    }
    /// The type byte; vmangos sends 1 (area spell) for every persistent-area cast.
    pub fn dynamicobject_bytes(&self) -> Option<u32> {
        self.get_u32(8)
    }
    /// The anchoring spell, the root of the ground-targeted visual chain.
    pub fn dynamicobject_spell_id(&self) -> Option<u32> {
        self.get_u32(9).filter(|&s| s != 0)
    }
    /// The area's radius in yards, which the server resolves from `SpellRadius.dbc`.
    pub fn dynamicobject_radius(&self) -> Option<f32> {
        self.get_f32(10)
    }
    /// The anchored point and facing in raw WoW coordinates, equal to the create's position.
    pub fn dynamicobject_position(&self) -> Option<([f32; 3], f32)> {
        Some((
            [self.get_f32(11)?, self.get_f32(12)?, self.get_f32(13)?],
            self.get_f32(14).unwrap_or(0.0),
        ))
    }

    /// Whether the mask carries the field, ignoring the created store's absent-is-zero.
    fn contains(&self, index: u16) -> bool {
        self.present
            .get(usize::from(index / 32))
            .is_some_and(|w| w & (1u32 << (index % 32)) != 0)
    }

    fn get_raw(&self, index: u16) -> Option<u32> {
        self.contains(index)
            .then(|| self.values[usize::from(index)])
    }

    fn insert(&mut self, index: u16, value: u32) {
        // A `u16` index bounds the store at 64 Ki values, whatever the wire says.
        let word = usize::from(index / 32);
        if word >= self.present.len() {
            self.present.resize(word + 1, 0);
            self.values.resize(self.present.len() * 32, 0);
        }
        self.present[word] |= 1u32 << (index % 32);
        self.values[usize::from(index)] = value;
    }

    fn get_guid(&self, index: u16) -> Option<u64> {
        // Raw on purpose: a guid is present iff its low half is, even on a created store.
        let lo = self.get_raw(index)?;
        let hi = self.get_u32(index + 1).unwrap_or(0);
        Some(u64::from(lo) | (u64::from(hi) << 32))
    }

    fn get_u32(&self, index: u16) -> Option<u32> {
        self.get_raw(index)
            .or((index < self.descriptor_end).then_some(0))
    }
    fn get_f32(&self, index: u16) -> Option<f32> {
        self.get_u32(index).map(f32::from_bits)
    }
    fn get_i32(&self, index: u16) -> Option<i32> {
        self.get_u32(index).map(|v| v as i32)
    }
    /// A `TWO_SHORT` field as its (low, high) halves (vmangos `shared/Common.h:119-121`).
    fn get_u16_pair(&self, index: u16) -> Option<(u16, u16)> {
        self.get_u32(index).map(|v| (v as u16, (v >> 16) as u16))
    }

    /// One slot's `UNIT_FIELD_AURAFLAGS` nibble; an absent word reads 0, as in the client.
    fn get_aura_nibble(&self, slot: u8) -> u8 {
        let word = self
            .get_u32(FIELD_UNIT_AURAFLAGS + u16::from(slot >> 3))
            .unwrap_or(0);
        ((word >> ((slot & 7) * 4)) & 0x0F) as u8
    }

    /// One slot's byte of `UNIT_FIELD_AURALEVELS` or `UNIT_FIELD_AURAAPPLICATIONS`; an absent word
    /// reads 0.
    fn get_aura_byte(&self, base: u16, slot: u8) -> u8 {
        let word = self.get_u32(base + u16::from(slot >> 2)).unwrap_or(0);
        (word >> ((slot & 3) * 8)) as u8
    }

    /// Folds in an update: a `Values` delta overlays, since a field going to 0 is sent as 0; a
    /// re-CREATE replaces, as overlaying it would keep fields that have since dropped to 0.
    pub fn merge(&mut self, delta: ObjectFields) {
        self.merge_diff(delta, |_, _, _| {});
    }

    /// [`Self::merge`], calling `changed(index, old, new)` once per changed dword in ascending
    /// order, as the client's `CMirrorHandler` pass does (`0x465330`): a resend of the same value
    /// reports nothing, and an absent old value reads 0. A first create is seeded, never merged.
    pub fn merge_diff(&mut self, delta: ObjectFields, mut changed: impl FnMut(u16, u32, u32)) {
        if delta.descriptor_end != 0 {
            let words = self.present.len().max(delta.present.len());
            for word in 0..words {
                let mask = self.present.get(word).copied().unwrap_or(0)
                    | delta.present.get(word).copied().unwrap_or(0);
                if mask == 0 {
                    continue;
                }
                for bit in 0..32u16 {
                    if mask & (1u32 << bit) == 0 {
                        continue;
                    }
                    let index = word as u16 * 32 + bit;
                    let (old, new) = (
                        self.get_raw(index).unwrap_or(0),
                        delta.get_raw(index).unwrap_or(0),
                    );
                    if old != new {
                        changed(index, old, new);
                    }
                }
            }
            *self = delta;
        } else {
            for (index, value) in delta.raw_fields() {
                let old = self.get_raw(index).unwrap_or(0);
                if old != value {
                    changed(index, old, value);
                }
                self.insert(index, value);
            }
        }
    }

    /// The type this was created as, from its descriptor length (the eight lengths are distinct).
    /// Unlike [`Self::object_type`] it tells a corpse from an item, as index 36 needs.
    pub fn created_as(&self) -> Option<ObjectType> {
        const ALL: [ObjectType; 8] = [
            ObjectType::Object,
            ObjectType::Item,
            ObjectType::Container,
            ObjectType::Unit,
            ObjectType::Player,
            ObjectType::GameObject,
            ObjectType::DynamicObject,
            ObjectType::Corpse,
        ];
        (self.descriptor_end != 0)
            .then(|| {
                ALL.into_iter()
                    .find(|&t| descriptor_len(t) == self.descriptor_end)
            })
            .flatten()
    }

    /// No fields carried; the codec skips an empty `Values` delta.
    pub fn is_empty(&self) -> bool {
        self.present.iter().all(|&w| w == 0)
    }
    /// Every carried `(index, value)` pair in ascending order, for debugging and probes.
    pub fn raw_fields(&self) -> impl Iterator<Item = (u16, u32)> + '_ {
        self.present.iter().enumerate().flat_map(move |(word, &w)| {
            (0..32u16)
                .filter(move |bit| w & (1u32 << bit) != 0)
                .map(move |bit| {
                    let index = word as u16 * 32 + bit;
                    (index, self.values[usize::from(index)])
                })
        })
    }
}

mod player;
mod unit;

pub use unit::{power_display_scale, OwnerFallback};

#[cfg(test)]
mod tests;
