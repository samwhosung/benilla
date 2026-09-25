//! Named accessors over [`ObjectFields`] for the item, container, player and GameObject blocks.

use super::*;

impl ObjectFields {
    /// `OBJECT_FIELD_ENTRY` (3): the template entry. The only route to a pet's template: its guid
    /// holds a pet number instead, but vmangos writes the entry here (`Creature.cpp:376`).
    pub fn object_entry(&self) -> Option<u32> {
        self.get_u32(3)
    }
    /// `ITEM_FIELD_STACK_COUNT`: an item object's stack size.
    pub fn item_stack_count(&self) -> Option<u32> {
        self.get_u32(FIELD_ITEM_STACK_COUNT)
    }
    /// `ITEM_FIELD_SPELL_CHARGES + i` (field `16 + i`, the client's `[item+0x114]+0x28`):
    /// charges left on spell `i`, signed; negative is consumed when empty, 0 is spent.
    pub fn item_spell_charges(&self, i: u8) -> Option<i32> {
        (i < 5)
            .then(|| self.get_u32(FIELD_ITEM_STACK_COUNT + 2 + u16::from(i)))?
            .map(|v| v as i32)
    }
    /// `ITEM_FIELD_FLAGS` (field 21): `0x08` wrapped, never alerts; `0x10` forces red status 4.
    pub fn item_flags(&self) -> Option<u32> {
        self.get_u32(21)
    }
    /// `ITEM_FIELD_CREATOR` (field 10): crafter, or letter sender (`HandleMailCreateTextItem`).
    pub fn item_creator(&self) -> Option<u64> {
        self.get_guid(10).filter(|&g| g != 0)
    }
    /// `ITEM_FIELD_ITEM_TEXT_ID` (field 45, owner only): a mailed letter copy's text; nonzero
    /// makes a right-click read it (`CMSG_ITEM_TEXT_QUERY`) instead of using the item.
    pub fn item_text_id(&self) -> Option<u32> {
        self.get_u32(45)
    }
    /// `ITEM_FIELD_ENCHANTMENT + 3*slot` (field `22 + 3*slot`): the enchant id in one of 7 slots
    /// (0 permanent, 1 temporary, 2-6 random properties), each an id, duration and charges
    /// (`Item.h:117-119`). Signed: the tooltip looks up `abs(id)` and paints a negative one red.
    pub fn item_enchant(&self, slot: u8) -> Option<i32> {
        (slot < 7)
            .then(|| self.get_u32(FIELD_ITEM_ENCHANTMENT + 3 * u16::from(slot)))?
            .map(|id| id as i32)
            .filter(|&id| id != 0)
    }
    /// `ITEM_FIELD_ENCHANTMENT + 3*slot + 2`: charges left, shown as `" (N Charges)"` if nonzero.
    /// The tooltip skips the duration; a countdown comes from `SMSG_ITEM_ENCHANT_TIME_UPDATE`.
    pub fn item_enchant_charges(&self, slot: u8) -> u32 {
        (slot < 7)
            .then(|| self.get_u32(FIELD_ITEM_ENCHANTMENT + 3 * u16::from(slot) + 2))
            .flatten()
            .unwrap_or(0)
    }
    /// `ITEM_FIELD_RANDOM_PROPERTIES_ID` (field 44): the `ItemRandomProperties.dbc` suffix, 0 if
    /// none; only the name uses it (`0x5d8b00`), the roll's enchants fill enchant slots 2-6.
    pub fn item_random_properties_id(&self) -> u32 {
        self.get_u32(44).unwrap_or(0)
    }
    /// `ITEM_FIELD_DURABILITY` (field 46): current durability.
    pub fn item_durability(&self) -> Option<u32> {
        self.get_u32(46)
    }
    /// `ITEM_FIELD_MAXDURABILITY`: the durability ceiling, 0 for an item without durability.
    pub fn item_max_durability(&self) -> Option<u32> {
        self.get_u32(47)
    }
    /// `CONTAINER_FIELD_NUM_SLOTS`: a bag object's capacity.
    pub fn container_num_slots(&self) -> Option<u32> {
        self.get_u32(FIELD_CONTAINER_NUM_SLOTS)
    }
    /// `CONTAINER_FIELD_SLOT_1 + 2i`: the item guid in bag slot `i`, `Some(0)` when empty.
    pub fn container_slot(&self, i: u8) -> Option<u64> {
        (i < 36).then(|| self.get_guid(FIELD_CONTAINER_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_VISIBLE_ITEM_<slot>_0`: the public entry worn in equipment slot `i`, which other
    /// clients render from; each slot is 12 dwords, the entry after a 2-dword creator.
    pub fn player_visible_item_entry(&self, i: u8) -> Option<u32> {
        (i < 19)
            .then(|| {
                self.get_u32(FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2 + 12 * u16::from(i))
                    .filter(|&e| e != 0)
            })
            .flatten()
    }
    /// `PLAYER_VISIBLE_ITEM_<slot>_0 + 1 + j`: broadcast enchant `j` of the item in slot `i`;
    /// vmangos fills only 0 and 1 (`MAX_INSPECTED_ENCHANTMENT_SLOT`). The reference's enchant
    /// visuals for every unit, itself included, come from these (`item+0xc`).
    pub fn player_visible_item_enchant(&self, i: u8, j: u8) -> Option<u32> {
        (i < 19 && j < 7)
            .then(|| {
                self.get_u32(
                    FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 3 + 12 * u16::from(i) + u16::from(j),
                )
                .filter(|&e| e != 0)
            })
            .flatten()
    }
    /// `PLAYER_VISIBLE_ITEM_<slot>_PROPERTIES`: slot `i`'s broadcast suffix roll, the low half
    /// (`0x53339f`, `movzx WORD`) of the dword after the 7 enchants. Only the name uses it.
    pub fn player_visible_item_properties(&self, i: u8) -> u32 {
        (i < 19)
            .then(|| self.get_u32(FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 10 + 12 * u16::from(i)))
            .flatten()
            .unwrap_or(0)
            & 0xffff
    }
    /// `PLAYER_FIELD_INV_SLOT_HEAD + 2i`: our equipment (0-18) and equipped-bag (19-22) guids.
    pub fn player_inv_slot(&self, i: u8) -> Option<u64> {
        (i < 23).then(|| self.get_guid(FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_PACK_SLOT_1 + 2i`: our backpack's 16 item guids.
    pub fn player_pack_slot(&self, i: u8) -> Option<u64> {
        (i < 16).then(|| self.get_guid(FIELD_PLAYER_PACK_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_BANK_SLOT_1 + 2i`: our bank's 24 item guids.
    pub fn player_bank_slot(&self, i: u8) -> Option<u64> {
        (i < 24).then(|| self.get_guid(FIELD_PLAYER_BANK_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_BANK_BAG_SLOT_1 + 2i`: bank bag `i`'s guid; its contents stream on the bag
    /// item's own `CONTAINER_FIELD_SLOT_*`, addressed as bag 63-68.
    pub fn player_bank_bag_slot(&self, i: u8) -> Option<u64> {
        (i < 6).then(|| self.get_guid(FIELD_PLAYER_BANK_BAG_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_KEYRING_SLOT_1 + 2i`: 32 guids the client walks as slots 81-112, but only
    /// 16 are addressable (vmangos `KEYRING_SLOT_END 97`) and level unlocks 4, 8, 12 or 16.
    pub fn player_keyring_slot(&self, i: u8) -> Option<u64> {
        (i < 32).then(|| self.get_guid(FIELD_PLAYER_KEYRING_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_VENDORBUYBACK_SLOT_1 + 2i`: buyback slot `i`, inventory slot `69 + i`.
    pub fn player_buyback_slot(&self, i: u8) -> Option<u64> {
        (i < 12).then(|| self.get_guid(FIELD_PLAYER_VENDORBUYBACK_SLOT_1 + 2 * u16::from(i)))?
    }
    /// `PLAYER_FIELD_BUYBACK_PRICE_1 + i`: what buying slot `i` back costs, in copper.
    pub fn player_buyback_price(&self, i: u8) -> Option<u32> {
        (i < 12).then(|| self.get_u32(FIELD_PLAYER_BUYBACK_PRICE_1 + u16::from(i)))?
    }
    /// `PLAYER_FIELD_BUYBACK_TIMESTAMP_1 + i`: expiry, used only to sort oldest first (`0x4fafd0`).
    pub fn player_buyback_timestamp(&self, i: u8) -> Option<u32> {
        (i < 12).then(|| self.get_u32(FIELD_PLAYER_BUYBACK_TIMESTAMP_1 + u16::from(i)))?
    }
    /// `PLAYER_FIELD_COINAGE`: our money, in copper.
    pub fn player_money(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_COINAGE)
    }
    /// `PLAYER_FARSIGHT` (private): the view's anchor during Mind Vision and the like, or 0.
    pub fn player_farsight(&self) -> Option<u64> {
        self.get_guid(FIELD_PLAYER_FARSIGHT).filter(|&g| g != 0)
    }
    /// `PLAYER_XP` (private): experience within the current level, `UnitXP("player")`.
    pub fn player_xp(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_XP)
    }
    /// `PLAYER_NEXT_LEVEL_XP` (private): experience to the next level, `UnitXPMax("player")`.
    pub fn player_next_level_xp(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_NEXT_LEVEL_XP)
    }
    /// `PLAYER_FIELD_WATCHED_FACTION_INDEX` (private): the reputation slot on the main menu bar,
    /// signed: none is `-1`, since slot 0 is the Bloodsail Buccaneers.
    pub fn player_watched_faction(&self) -> Option<i32> {
        self.get_u32(FIELD_PLAYER_WATCHED_FACTION_INDEX)
            .map(|v| v as i32)
    }
    /// `PLAYER_CHARACTER_POINTS1` (private): unspent talent points, first of `UnitCharacterPoints`.
    pub fn player_talent_points(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_CHARACTER_POINTS1)
    }
    /// `PLAYER_CHARACTER_POINTS2` (private): free professions, second of `UnitCharacterPoints`.
    pub fn player_free_professions(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_CHARACTER_POINTS2)
    }
    /// `PLAYER_TRACK_CREATURES` (private): bit `1 << (creatureType - 1)` per creature tracking
    /// aura, the mask the minimap's red dots test.
    pub fn player_track_creatures(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_TRACK_CREATURES).unwrap_or(0)
    }
    /// `PLAYER_TRACK_RESOURCES` (private): bit `1 << (lockType - 1)` per resource tracking aura
    /// (`LockType.dbc`), the mask the minimap's gold dots test.
    pub fn player_track_resources(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_TRACK_RESOURCES).unwrap_or(0)
    }
    /// `PLAYER_FIELD_BYTES` byte 0 bit `0x2` (private): set by aura type 151, track stealthed;
    /// every creep-flagged unit then gets a minimap dot (`0x5ed210`, the `+0x1028` read).
    pub fn player_track_stealthed(&self) -> bool {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES).unwrap_or(0) & 0x2 != 0
    }
    /// `PLAYER_QUEST_LOG_<slot+1>_*`: one quest-log slot, `quest_id == 0` when the server cleared
    /// it; the count-state and timer are written with the id (`Player.h:1094-1099`).
    pub fn player_quest_log(&self, slot: u8) -> Option<QuestLogSlot> {
        if slot >= PLAYER_QUEST_LOG_SLOTS {
            return None;
        }
        let base = FIELD_PLAYER_QUEST_LOG_1_1 + 3 * u16::from(slot);
        let quest_id = self.get_u32(base)?;
        let count_state = self.get_u32(base + 1).unwrap_or(0);
        let timer = self.get_u32(base + 2).unwrap_or(0);
        let mut counters = [0u8; 4];
        for (i, c) in counters.iter_mut().enumerate() {
            *c = ((count_state >> (6 * i)) & 0x3F) as u8;
        }
        Some(QuestLogSlot {
            quest_id,
            counters,
            state: (count_state >> 24) as u8,
            timer,
        })
    }
    /// `PLAYER_EXPLORED_ZONES_1 + i`: bitset dword `i`; bit `n` is `AreaTable` explore flag `n`.
    pub fn player_explored_zone_slot(&self, i: u16) -> u32 {
        if i >= PLAYER_EXPLORED_ZONES_SLOTS {
            return 0;
        }
        self.get_u32(FIELD_PLAYER_EXPLORED_ZONES_1 + i).unwrap_or(0)
    }
    /// `PLAYER_BLOCK_PERCENTAGE`: block chance, already a percent (2.62 means 2.62%).
    pub fn player_block_percentage(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_BLOCK_PERCENTAGE)
    }
    /// `PLAYER_DODGE_PERCENTAGE`: dodge chance, already a percent.
    pub fn player_dodge_percentage(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_DODGE_PERCENTAGE)
    }
    /// `PLAYER_PARRY_PERCENTAGE`: parry chance, already a percent.
    pub fn player_parry_percentage(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_PARRY_PERCENTAGE)
    }
    /// `PLAYER_CRIT_PERCENTAGE`: melee crit chance, already a percent; ranged is the next field.
    pub fn player_crit_percentage(&self) -> Option<f32> {
        self.get_f32(FIELD_PLAYER_CRIT_PERCENTAGE)
    }
    /// `PLAYER_FIELD_POSSTAT0 + i`: stat `i`'s positive buff total from gear, enchants and
    /// auras; an int on the wire, narrowed from the server's float (`BuildValuesUpdate`).
    pub fn player_posstat(&self, i: u8) -> Option<i32> {
        (i < 5).then(|| self.get_i32(FIELD_PLAYER_POSSTAT0 + u16::from(i)))?
    }
    /// `PLAYER_FIELD_NEGSTAT0 + i`: stat `i`'s negative buff total, an int at or below 0. An x86
    /// server sends it in two's complement, an arm64 one saturates it to 0.
    pub fn player_negstat(&self, i: u8) -> Option<i32> {
        (i < 5).then(|| self.get_i32(FIELD_PLAYER_NEGSTAT0 + u16::from(i)))?
    }
    /// `PLAYER_FIELD_RESISTANCEBUFFMODSPOSITIVE + school`: the positive resistance buff, an int.
    pub fn player_resistance_buff_pos(&self, school: u8) -> Option<i32> {
        (school < 7)
            .then(|| self.get_i32(FIELD_PLAYER_RESISTANCEBUFFMODSPOSITIVE + u16::from(school)))?
    }
    /// `PLAYER_FIELD_RESISTANCEBUFFMODSNEGATIVE + school`: the negative resistance buff, at most 0.
    pub fn player_resistance_buff_neg(&self, school: u8) -> Option<i32> {
        (school < 7)
            .then(|| self.get_i32(FIELD_PLAYER_RESISTANCEBUFFMODSNEGATIVE + u16::from(school)))?
    }
    /// `PLAYER_FIELD_MOD_DAMAGE_DONE_POS + school`: the positive damage bonus, school 0 physical.
    pub fn player_mod_damage_done_pos(&self, school: u8) -> Option<i32> {
        (school < 7).then(|| self.get_i32(FIELD_PLAYER_MOD_DAMAGE_DONE_POS + u16::from(school)))?
    }
    /// `PLAYER_FIELD_MOD_DAMAGE_DONE_NEG + school`: the negative damage bonus, at most 0.
    pub fn player_mod_damage_done_neg(&self, school: u8) -> Option<i32> {
        (school < 7).then(|| self.get_i32(FIELD_PLAYER_MOD_DAMAGE_DONE_NEG + u16::from(school)))?
    }
    /// `PLAYER_FIELD_MOD_DAMAGE_DONE_PCT + school`: the damage multiplier, a true float that the
    /// server starts at 1.0 (`Player.cpp:3336`); a caller reads `None` as 1.0.
    pub fn player_mod_damage_done_pct(&self, school: u8) -> Option<f32> {
        (school < 7).then(|| self.get_f32(FIELD_PLAYER_MOD_DAMAGE_DONE_PCT + u16::from(school)))?
    }
    /// `PLAYER_SKILL_INFO_<slot+1>_*`: one skill slot; vmangos writes the id and value together
    /// (`SetSkill`), so an absent value or bonus reads 0.
    pub fn player_skill(&self, slot: u8) -> Option<PlayerSkillSlot> {
        if slot >= PLAYER_SKILL_SLOTS {
            return None;
        }
        let base = FIELD_PLAYER_SKILL_INFO_1_1 + 3 * u16::from(slot);
        let (skill_id, step) = self.get_u16_pair(base)?;
        let (value, max) = self.get_u16_pair(base + 1).unwrap_or((0, 0));
        let (temp_bonus, perm_bonus) = self.get_u16_pair(base + 2).unwrap_or((0, 0));
        Some(PlayerSkillSlot {
            skill_id,
            step,
            value,
            max,
            temp_bonus: temp_bonus as i16,
            perm_bonus: perm_bonus as i16,
        })
    }
    /// `PLAYER_AMMO_ID`: the item id of the selected arrows or bullets, 0 for none.
    pub fn player_ammo_id(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_AMMO_ID)
    }
    /// `PLAYER_SELF_RES_SPELL`: the spell `CMSG_SELF_RES` casts (Soulstone 3026, 20758-20761,
    /// Reincarnation 21169, Twisting Nether 23700) and the whole condition for offering it; set
    /// one flush before health 0 at death (`Unit.cpp:1136-1143`), zeroed by any other resurrection.
    pub fn player_self_res_spell(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_SELF_RES_SPELL)
            .filter(|&s| s != 0)
    }
    /// `PLAYER_BYTES`: skinColor, faceType, hairStyle and hairColor, bytes 0 to 3.
    pub fn player_bytes(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_BYTES)
    }
    /// `PLAYER_BYTES_2`: facialHair, unknown, bank bag slots and rest state, bytes 0 to 3.
    pub fn player_bytes_2(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_BYTES_2)
    }
    /// `PLAYER_BYTES_2` byte 3: rest state, 1 rested, 2 normal (`Player.h:673`), set with
    /// hysteresis on the pool (`SetRestBonus`). Absent reads 0, which `GetRestState()` answers
    /// with nils; do not default it to 2, which would hide a read before the create landed.
    pub fn player_rest_state(&self) -> Option<u8> {
        self.player_bytes_2().map(|b| (b >> 24) as u8)
    }
    /// `PLAYER_REST_STATE_EXPERIENCE` (field 1175, private): the rested pool in base kill XP,
    /// drained 1:1 while kills pay double (`GetXPRestBonus`), so the bar's rested span is twice it.
    pub fn player_rest_state_experience(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_REST_STATE_EXPERIENCE)
    }
    /// `PLAYER_BYTES_3` byte 1: drunkenness, the high byte of `drunk & 0xFFFE` in the low half
    /// (`SetDrunkValue`); the reference clamps it to 100 and scales by 0.01 (`0x5e2a90`).
    pub fn player_drunk_byte(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_BYTES_3).map(|v| (v >> 8) as u8)
    }
    /// `PLAYER_FLAGS` (field 190): the player state flags.
    pub fn player_flags(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_FLAGS).unwrap_or(0)
    }
    /// `PLAYER_FLAGS` bit `0x1`: leads their group, for a stranger's party too; the first leg of
    /// `UnitIsPartyLeader` (`0x51624a`) and the bit's only test.
    pub fn player_is_group_leader(&self) -> bool {
        self.player_flags() & 0x1 != 0
    }
    /// `PLAYER_FLAGS_GHOST` (`0x10`), from aura 8326; a ghost has health 1, so not `unit_is_dead`.
    pub fn player_is_ghost(&self) -> bool {
        self.player_flags() & 0x10 != 0
    }
    /// Dead or a ghost, the reference's `0x605f30`: health 0, or for a player the ghost flag. Not
    /// feign death, which is `unit_reads_dead`; a creature has no `PLAYER_FLAGS`, so reads clear.
    pub fn is_dead_or_ghost(&self) -> bool {
        self.unit_is_dead() || self.player_is_ghost()
    }
    /// `PLAYER_FLAGS_HIDE_HELM` (`0x400`, `Player.h:325`), set by `CMSG_TOGGLE_HELM`; public.
    pub fn player_hides_helm(&self) -> bool {
        self.player_flags() & 0x400 != 0
    }
    /// `PLAYER_FLAGS_HIDE_CLOAK` (`0x800`), public like the helm bit.
    pub fn player_hides_cloak(&self) -> bool {
        self.player_flags() & 0x800 != 0
    }
    /// `PLAYER_FLAGS_RESTING` (`0x20`, `Player.h:320`): in an inn or city; `IsResting()` reads it.
    pub fn player_is_resting(&self) -> bool {
        self.player_flags() & 0x20 != 0
    }
    /// `PLAYER_DUEL_ARBITER` (field 188, guid, public): the duel flag GameObject, 0 for none;
    /// set on both duellists at the challenge (`Spell::EffectDuel`).
    pub fn player_duel_arbiter(&self) -> u64 {
        self.get_guid(FIELD_PLAYER_DUEL_ARBITER).unwrap_or(0)
    }
    /// `PLAYER_DUEL_TEAM` (field 196): 0 until the countdown ends, then 1 challenger, 2
    /// challenged (`Player::UpdateDuelFlag`); a duel is live only when both sides are nonzero.
    pub fn player_duel_team(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_DUEL_TEAM).unwrap_or(0)
    }
    /// `PLAYER_GUILDID` (field 191, public): the guild, 0 for none; names come from
    /// `SMSG_GUILD_QUERY_RESPONSE`, and `GetGuildInfo` (`0x4c9330`) answers nil on 0 or a miss.
    pub fn player_guild_id(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_GUILDID).unwrap_or(0)
    }
    /// `PLAYER_GUILDRANK` (field 192): the guild rank, 0 the guild master; the reference indexes
    /// its rank names with it unbounded (`0x4c93e4`), so bound it to the ten ranks.
    pub fn player_guild_rank(&self) -> u32 {
        self.get_u32(FIELD_PLAYER_GUILDRANK).unwrap_or(0)
    }
    /// `PLAYER_FIELD_BYTES` byte 0 bit `0x08`, `PLAYER_FIELD_BYTE_RELEASE_TIMER`: set at death
    /// outside instances, where the server releases the spirit after 6 minutes; the client arms
    /// its own `now + 360000` ms, and with the bit clear `GetReleaseTimeRemaining()` is -1.
    pub fn player_release_timer_running(&self) -> bool {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES).unwrap_or(0) & 0x08 != 0
    }
    /// `PLAYER_FIELD_BYTES` byte 1 (private): combo points, 0-5, read at `[player+0xe68]+0x1029`.
    /// vmangos also sets one when a warrior's target dodges (the 4 s Overpower window): the usable
    /// check reads it for any class, `GetComboPoints` (`0x51a190`) only for rogues and druids.
    pub fn player_combo_points(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES)
            .map(|b| ((b >> 8) & 0xff) as u8)
    }
    /// `PLAYER_FIELD_COMBO_TARGET` (field 714, guid, private): the unit the combo points sit on;
    /// `GetComboPoints` shows 0 unless it is the current target (`0xb4e2d8`).
    pub fn player_combo_target(&self) -> u64 {
        self.get_guid(FIELD_PLAYER_FIELD_COMBO_TARGET).unwrap_or(0)
    }
    /// `PLAYER_FIELD_BYTES` byte 2 (private, field 1222): the four extra action bars' toggles,
    /// bits `0x01..0x08` (`Player.h:360-363`), read by `GetActionBarToggles` (`0x4e7660`) at
    /// `[[player+0xe68]+0x102a]`. The client never writes it and no event fires when it moves.
    /// vmangos's `// 0x4C0` beside the field is stale; `UNIT_END + 0x40A` is right.
    pub fn player_action_bar_toggles(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES)
            .map(|b| ((b >> 16) & 0xff) as u8)
    }
    /// `PLAYER_FIELD_BYTES` byte 3 (private): the highest honor rank held, 0 never ranked, which
    /// `RequiredHonorRank` checks (`0x5ea930`); only ever raised (`HonorMgr.cpp:894-895`), so it
    /// outlives a demotion, unlike [`Self::player_pvp_rank`].
    pub fn player_honor_rank(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES)
            .map(|b| ((b >> 24) & 0xff) as u8)
    }
    /// `PLAYER_BYTES_3` byte 3 (public, `UpdateFields_1_12_1.h:125`): the current internal rank
    /// (`HonorMgr.cpp:900`), 0 unranked, 1-4 negative, 5 up positive, indexing `PVP_RANK_*`;
    /// the drawn rank is `rank > 4 ? rank - 4 : -rank` (`HonorMgr.cpp:991`).
    pub fn player_pvp_rank(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_BYTES_3)
            .map(|b| ((b >> 24) & 0xff) as u8)
    }
    /// `PLAYER_BYTES_3` byte 2 (public): the city-protector title, a race id (`Player.h:354`);
    /// `UnitPVPName` (`0x609370`) appends `PVP_MEDAL<n>` when nonzero. vmangos never writes it.
    pub fn player_pvp_medal(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_BYTES_3)
            .map(|b| ((b >> 16) & 0xff) as u8)
    }
    /// `PLAYER_FIELD_SESSION_KILLS` (private): today's honorable and dishonorable kills, both
    /// halves written (`HonorMgr.cpp:913-914`); "session" means today, `GetPVPSessionStats()`.
    pub fn player_session_kills(&self) -> Option<(u16, u16)> {
        self.get_u16_pair(FIELD_PLAYER_FIELD_SESSION_KILLS)
    }
    /// `PLAYER_FIELD_YESTERDAY_KILLS` (private): yesterday's kills, two shorts; vmangos writes a
    /// whole dword (`HonorMgr.cpp:917`), so the dishonorable half is 0, not a decode bug.
    pub fn player_yesterday_kills(&self) -> Option<(u16, u16)> {
        self.get_u16_pair(FIELD_PLAYER_FIELD_YESTERDAY_KILLS)
    }
    /// `PLAYER_FIELD_LAST_WEEK_KILLS` (private): last week's kills; the dishonorable half is 0
    /// from vmangos (`HonorMgr.cpp:929` writes a whole dword).
    pub fn player_last_week_kills(&self) -> Option<(u16, u16)> {
        self.get_u16_pair(FIELD_PLAYER_FIELD_LAST_WEEK_KILLS)
    }
    /// `PLAYER_FIELD_THIS_WEEK_KILLS` (private): this week's kills; the dishonorable half is 0
    /// from vmangos (`HonorMgr.cpp:924` writes a whole dword).
    pub fn player_this_week_kills(&self) -> Option<(u16, u16)> {
        self.get_u16_pair(FIELD_PLAYER_FIELD_THIS_WEEK_KILLS)
    }
    /// `PLAYER_FIELD_THIS_WEEK_CONTRIBUTION` (private): honor earned this week, floored at 0
    /// (`HonorMgr.cpp:925`).
    pub fn player_this_week_contribution(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_THIS_WEEK_CONTRIBUTION)
    }
    /// `PLAYER_FIELD_YESTERDAY_CONTRIBUTION` (private): yesterday's honor (`HonorMgr.cpp:918`).
    pub fn player_yesterday_contribution(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_YESTERDAY_CONTRIBUTION)
    }
    /// `PLAYER_FIELD_LAST_WEEK_CONTRIBUTION` (private): last week's honor, what the weekly rank
    /// calculation used (`HonorMgr.cpp:930`).
    pub fn player_last_week_contribution(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_LAST_WEEK_CONTRIBUTION)
    }
    /// `PLAYER_FIELD_LAST_WEEK_RANK` (private): last week's standing on the honor ladder, not an
    /// honor rank; 0 is unranked (`HonorMgr.cpp:931`).
    pub fn player_last_week_rank(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_LAST_WEEK_RANK)
    }
    /// `PLAYER_FIELD_LIFETIME_HONORBALE_KILLS` (private, vmangos's spelling): lifetime honorable
    /// kills (`HonorMgr.cpp:934`).
    pub fn player_lifetime_honorable_kills(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS)
    }
    /// `PLAYER_FIELD_LIFETIME_DISHONORBALE_KILLS` (private, vmangos's spelling): lifetime
    /// dishonorable kills (`HonorMgr.cpp:935`).
    pub fn player_lifetime_dishonorable_kills(&self) -> Option<u32> {
        self.get_u32(FIELD_PLAYER_FIELD_LIFETIME_DISHONORABLE_KILLS)
    }
    /// `PLAYER_FIELD_BYTES2` byte 0 (private): progress through the current rank, 0-255
    /// (`HonorMgr.cpp:905-909`); a negative rank's `-255` scale wraps in the `uint8` cast.
    pub fn player_honor_rank_bar(&self) -> Option<u8> {
        self.get_u32(FIELD_PLAYER_FIELD_BYTES2)
            .map(|b| (b & 0xff) as u8)
    }
    /// `UNIT_FIELD_BYTES_0` byte 0: race.
    pub fn unit_race(&self) -> Option<u8> {
        self.unit_bytes_0().map(|b| (b & 0xff) as u8)
    }
    /// `UNIT_FIELD_BYTES_0` byte 1: class.
    pub fn unit_class(&self) -> Option<u8> {
        self.unit_bytes_0().map(|b| ((b >> 8) & 0xff) as u8)
    }
    /// `UNIT_FIELD_BYTES_0` byte 2: gender.
    pub fn unit_gender(&self) -> Option<u8> {
        self.unit_bytes_0().map(|b| ((b >> 16) & 0xff) as u8)
    }
    /// `PLAYER_BYTES` byte 0: skinColor.
    pub fn player_skin(&self) -> Option<u8> {
        self.player_bytes().map(|b| (b & 0xff) as u8)
    }
    /// `PLAYER_BYTES` byte 1: faceType.
    pub fn player_face(&self) -> Option<u8> {
        self.player_bytes().map(|b| ((b >> 8) & 0xff) as u8)
    }
    /// `PLAYER_BYTES` byte 2: hairStyle.
    pub fn player_hair_style(&self) -> Option<u8> {
        self.player_bytes().map(|b| ((b >> 16) & 0xff) as u8)
    }
    /// `PLAYER_BYTES` byte 3: hairColor.
    pub fn player_hair_color(&self) -> Option<u8> {
        self.player_bytes().map(|b| ((b >> 24) & 0xff) as u8)
    }
    /// `PLAYER_BYTES_2` byte 0: facialHair.
    pub fn player_facial_hair(&self) -> Option<u8> {
        self.player_bytes_2().map(|b| (b & 0xff) as u8)
    }
    /// `PLAYER_BYTES_2` byte 2 (`Player.h:347`): bank bag slots bought, of 6; this delta is the
    /// only reply to a successful `CMSG_BUY_BANK_SLOT` (`PLAYERBANKBAGSLOTS_CHANGED`).
    pub fn player_bank_bag_slots_purchased(&self) -> Option<u8> {
        self.player_bytes_2().map(|b| ((b >> 16) & 0xff) as u8)
    }
    /// `OBJECT_FIELD_CREATED_BY` (fields 6-7): who summoned a spell-spawned object; the faction
    /// resolver `0x5f7fd0` prefers its reaction, but a bobber's use gate is our channel object.
    pub fn gameobject_created_by(&self) -> Option<u64> {
        self.get_guid(FIELD_GAMEOBJECT_CREATED_BY)
            .filter(|&g| g != 0)
    }
    /// `GAMEOBJECT_DISPLAYID`.
    pub fn gameobject_displayid(&self) -> Option<i32> {
        self.get_i32(FIELD_GAMEOBJECT_DISPLAYID)
    }
    /// `GAMEOBJECT_STATE` (`OBJECT_END + 0x8`): 0 active (open), 1 ready (closed), 2 active-alt.
    pub fn gameobject_state(&self) -> Option<u32> {
        self.get_u32(FIELD_GAMEOBJECT_STATE)
    }
    /// `GAMEOBJECT_ROTATION`: the spawn quaternion `(x, y, z, w)`, unsent components 0; the
    /// reference places a GameObject by it, never by `GAMEOBJECT_FACING`.
    pub fn gameobject_rotation(&self) -> Option<[f32; 4]> {
        let any_sent = (0..4u16).any(|i| self.contains(FIELD_GAMEOBJECT_ROTATION + i));
        any_sent.then(|| {
            [
                self.get_f32(FIELD_GAMEOBJECT_ROTATION).unwrap_or(0.0),
                self.get_f32(FIELD_GAMEOBJECT_ROTATION + 1).unwrap_or(0.0),
                self.get_f32(FIELD_GAMEOBJECT_ROTATION + 2).unwrap_or(0.0),
                self.get_f32(FIELD_GAMEOBJECT_ROTATION + 3).unwrap_or(0.0),
            ]
        })
    }
    /// `GAMEOBJECT_POS_*` and `GAMEOBJECT_FACING`; a create reads `Some` even with no position
    /// sent, as for a transport, so check [`Self::gameobject_pos_sent`].
    pub fn gameobject_position(&self) -> Option<(Vector3d, f32)> {
        Some((
            Vector3d {
                x: self.get_f32(FIELD_GAMEOBJECT_POS_X)?,
                y: self.get_f32(FIELD_GAMEOBJECT_POS_Y)?,
                z: self.get_f32(FIELD_GAMEOBJECT_POS_Z)?,
            },
            self.get_f32(FIELD_GAMEOBJECT_FACING).unwrap_or(0.0),
        ))
    }
    /// Whether any `GAMEOBJECT_POS_*` was really sent; vmangos sends none for a transport.
    pub fn gameobject_pos_sent(&self) -> bool {
        self.contains(FIELD_GAMEOBJECT_POS_X)
            || self.contains(FIELD_GAMEOBJECT_POS_Y)
            || self.contains(FIELD_GAMEOBJECT_POS_Z)
    }
    /// `GAMEOBJECT_TYPE_ID`: absent is 0, `GAMEOBJECT_TYPE_DOOR`: a create omits zero fields.
    pub fn gameobject_type_id(&self) -> i32 {
        self.get_i32(FIELD_GAMEOBJECT_TYPE_ID).unwrap_or(0)
    }
    /// `GAMEOBJECT_FLAGS`: `0x1` in use, `0x4` needs the quest activate bit, `0x10` no interact.
    pub fn gameobject_flags(&self) -> u32 {
        self.get_u32(FIELD_GAMEOBJECT_FLAGS).unwrap_or(0)
    }
    /// `GAMEOBJECT_FACTION`: the `FactionTemplate.dbc` id, 0 neutral; hostile to the player
    /// means not highlightable (`[go+0x110]+0x38`): no cursor, tooltip or hover.
    pub fn gameobject_faction(&self) -> u32 {
        self.get_u32(FIELD_GAMEOBJECT_FACTION).unwrap_or(0)
    }
    /// `GAMEOBJECT_DYN_FLAGS` (per player): `0x1` `GO_DYNFLAG_LO_ACTIVATE`, the sparkle, set by
    /// `GameObject::ActivateToQuest` and read only under flag `0x4`.
    pub fn gameobject_dynamic_flags(&self) -> u32 {
        self.get_u32(FIELD_GAMEOBJECT_DYN_FLAGS).unwrap_or(0)
    }
    /// `GAMEOBJECT_LEVEL` (field 22): a lock with `Skill[0]` 0 asks for `level * 5` (`0x5f3490`).
    pub fn gameobject_level(&self) -> u32 {
        self.get_u32(FIELD_GAMEOBJECT_LEVEL).unwrap_or(0)
    }

    /// `OBJECT_FIELD_TYPE & TYPEMASK_CONTAINER`: a bag, whose mask carries the item bit too; the
    /// reference's test at `0x4c879d` (`GetInventoryItemCount`) and `0x4c7dbb` (`PutItemInBag`).
    pub fn is_container(&self) -> bool {
        self.get_u32(FIELD_OBJECT_TYPE).unwrap_or(0) & 0x4 != 0
    }

    /// The object's class from `OBJECT_FIELD_TYPE`, for updates with no `TypeId`; the player bit
    /// `0x10` is the reference's own is-player test (`CanAttack`, `CanInteract`).
    pub fn object_type(&self) -> Option<ObjectType> {
        self.get_u32(FIELD_OBJECT_TYPE).map(|bits| {
            if bits & 0x10 != 0 {
                ObjectType::Player
            } else if bits & 0x08 != 0 {
                ObjectType::Unit
            } else if bits & 0x20 != 0 {
                ObjectType::GameObject
            } else {
                ObjectType::Object
            }
        })
    }
}
