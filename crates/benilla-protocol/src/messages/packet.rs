//! [`ServerPacket`]: one variant per server opcode benilla decodes, plus its logging name.

use crate::wire::Vector3d;

use super::{
    ActionButton, AttackSwingError, AttackerState, AuctionBidderNotification, AuctionCommandTail,
    AuctionListEntry, AuctionOwnerNotification, CastOutcome, ChannelNotify, Character, ChatMessage,
    CorpseLocation, DamageShield, DispelFailed, EnchantmentLog, EnvironmentalDamageLog,
    ExplorationXp, FriendEntry, FriendStatusUpdate, GameObjectQueryInfo, GmTicket, GossipOption,
    GossipPoi, GroupLootInfo, GroupMemberEntry, GuildCommandResult, GuildEventNotice, GuildInfo,
    GuildQueryResponse, GuildRoster, InitWorldStates, InspectHonorStats, ItemInfo, ItemPushResult,
    JumpInfo, LevelUpInfo, LootAllPassed, LootItem, LootRoll, LootRollWon, LootStartRoll,
    MailListEntry, MirrorTimerStart, MoveMode, Object, PartyKillLog, PartyMemberStatsInfo,
    PeriodicAuraLog, PetMode, PetSpells, PetitionQueryResponse, PetitionRename, PetitionShowList,
    PetitionShowSignatures, PetitionSignResults, PvpCredit, QuestComplete, QuestConfirmAccept,
    QuestDetails, QuestGiverList, QuestOfferReward, QuestOption, QuestPushResult,
    QuestRequestItems, QuestTemplate, ResurrectRequestBody, SpeedKind, SpellChainTargets,
    SpellCooldown, SpellDamageLog, SpellDispelLog, SpellEnergizeLog, SpellGo, SpellHealLog,
    SpellInstaKillLog, SpellLogExecute, SpellLogMiss, SpellOutcomeLog, SpellStart, SplineMode,
    StabledPet, TaxiMask, TradeStatus, TradeStatusExtended, TrainerSpell, TransportPose,
    VendorItem, WhoResults, XpGain,
};

/// The final facing a `SMSG_MONSTER_MOVE` dictates (`moveType`), applied as a hard snap, not a
/// turn (`0x6018f0`); `Spot` and `Target` resolve once, from positions at receipt.
#[derive(Debug, Clone, Copy)]
pub enum MonsterMoveFacing {
    /// `moveType` 0 or a stop: the unit faces its travel direction.
    None,
    /// `moveType` 2: face a world point (raw WoW coords).
    Spot([f32; 3]),
    /// `moveType` 3: face a unit by guid, resolved to its position when applied.
    Target(u64),
    /// `moveType` 4: face a raw orientation (radians, WoW convention).
    Angle(f32),
}

/// One creature template's UI-visible head, from `SMSG_CREATURE_QUERY_RESPONSE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatureQueryInfo {
    pub name: String,
    /// The NPC subtitle ("Stable Master"), the unit tooltip's second line.
    pub subname: String,
    /// `CreatureType.dbc` id: the tooltip level line's type word and the TAB-target filter.
    pub creature_type: u32,
    /// `CreatureFamily.dbc` id; `0`, read as nil, for all but tameable beasts and warlock minions.
    pub pet_family: u32,
    /// 0 normal, 1 elite, 2 rare-elite, 3 world boss, 4 rare (tooltip: "", Elite, Elite, Boss, "").
    pub rank: u32,
    /// Bit `0x10` (HIDE_FACTION_TOOLTIP) drops the tooltip's faction line (`0x612610`).
    pub type_flags: u32,
    /// The tooltip's green CIVILIAN line and the dishonorable-kill mark.
    pub civilian: bool,
    /// The tooltip's white LEADER line (`0x6125c0`).
    pub racial_leader: bool,
    /// `creature_template.display_id[0]` (`QueryHandler.cpp:179`), `0` if none, for drawing an
    /// unseen creature; a spawned unit may use any of its four ids, so its own display id wins.
    pub display_id: u32,
}

/// A decoded server packet; an unmodelled opcode is [`Self::Other`].
pub enum ServerPacket {
    AuthChallenge {
        server_seed: u32,
    },
    AuthResponse {
        result: u8,
        /// Our login-queue place; only for [`super::AUTH_WAIT_QUEUE`] with a long enough body.
        queue_position: Option<u32>,
        /// Rested billing minutes (`PlayerFrame.lua:246` divides by 60), returned unconverted by
        /// `GetBillingTimeRested()` (`0x48ec50`); `None` on a body too short for the group.
        billing_time_rested: Option<u32>,
    },
    CharEnum {
        characters: Vec<Character>,
    },
    CharCreate {
        result: u8,
    },
    CharDelete {
        result: u8,
    },
    /// `SMSG_CHARACTER_LOGIN_FAILED`: a 1-based reason index, not a `ResponseCodes` value, that
    /// the client maps (`0x5aae08`) 1 to "world server is down" and past 6 to "login failed".
    CharacterLoginFailed {
        result: u8,
    },
    UpdateObject {
        objects: Vec<Object>,
    },
    /// `SMSG_COMPRESSED_MOVES`: a `u32` size, then deflated records `[u8 size][u16 opcode][body]`,
    /// `size` counting the opcode (`MovementData::AddPacket`). vmangos switches to it past 300
    /// movement packets in ten seconds (`SendMovementPacket`), so it is the normal carrier.
    CompressedMoves {
        packets: Vec<ServerPacket>,
    },
    /// `SMSG_DESTROY_OBJECT`: a plain `u64` guid that ceased to exist server-side. The client frees
    /// it outright (`0x4674a0`), unlike an `OutOfRange` block, whose objects it keeps staged.
    DestroyObject {
        guid: u64,
    },
    /// `SMSG_TRIGGER_CINEMATIC`: a `CinematicSequences.dbc` id, sent at first login and by type-13
    /// camera objects. vmangos anchors visibility to the camera until `CMSG_COMPLETE_CINEMATIC`.
    TriggerCinematic {
        cinematic_id: u32,
    },
    /// `MSG_MOVE_TIME_SKIPPED`: an observed mover skipped `lag_ms`; its wire clock moves with it.
    MoveTimeSkipped {
        guid: u64,
        lag_ms: u32,
    },
    MonsterMove {
        guid: u64,
        /// `SMSG_MONSTER_MOVE_TRANSPORT` only: the transport whose frame `start` and `path` are in.
        transport: Option<u64>,
        start: Vector3d,
        /// Echoed in `CMSG_MOVE_SPLINE_DONE` when the spline moves our own player.
        spline_id: u32,
        /// The whole path, `[start, waypoints…, endpoint]`, walked at constant arc-length speed;
        /// empty for a stop. Ground waypoints are `¼`-yd endpoint offsets, flying ones absolute.
        path: Vec<Vector3d>,
        /// The dictated final facing (`moveType` 2/3/4), applied as a snap.
        facing: MonsterMoveFacing,
        stop: bool,
        duration_ms: u32,
        /// `SPLINE_FLAG_FLYING` (`Mask_CatmullRom`): keep the spline's Z, else clamp to terrain.
        flying: bool,
        /// `SPLINEFLAG_RUNMODE`; a spline without it forces `MOVEFLAG_WALK_MODE` on for the unit.
        run_mode: bool,
    },
    /// A relayed player `MSG_MOVE_*`: authoritative pose and live `moveFlags`; `transport` is set
    /// while `MOVEFLAG_ON_TRANSPORT`, a pose local to the transport.
    PlayerMove {
        guid: u64,
        opcode: u16,
        flags: u32,
        position: Vector3d,
        orientation: f32,
        /// Swim pitch (radians, +up) while `MOVEFLAG_SWIMMING`, else `0.0`: observers integrate
        /// the vertical with it, as the client's swim velocity basis does (`0x7c5880`).
        pitch: f32,
        /// vmangos's own ms clock at receipt (`MovementInfo::stime`), shared by all relayed moves.
        time: u32,
        fall_time: u32,
        jump: Option<JumpInfo>,
        transport: Option<TransportPose>,
    },
    Teleport {
        guid: u64,
        counter: u32,
        position: Vector3d,
        orientation: f32,
    },
    NewWorld {
        map: u32,
        position: Vector3d,
        orientation: f32,
    },
    /// `SMSG_TRANSFER_PENDING`: the far-teleport's map, plus `(transportEntry, oldMapId)` when
    /// riding a transport, which makes the following [`Self::NewWorld`] coordinates boat-local.
    TransferPending {
        map: u32,
        transport: Option<(u32, u32)>,
    },
    /// `SMSG_TRANSFER_ABORTED`: the announced transfer will not happen (map full, no instance).
    TransferAborted {
        reason: u8,
    },
    LoginVerifyWorld {
        map: u32,
        position: Vector3d,
        orientation: f32,
    },
    TimeSpeed {
        hours: u8,
        minutes: u8,
        /// Day count from the packed date, `year·372 + month·31 + day`; drives the moon phase.
        day_serial: u32,
        timescale: f32,
    },
    /// `SMSG_QUERY_TIME_RESPONSE`: wall-clock unix seconds, the base of absolute descriptor stamps.
    QueryTimeResponse {
        unix_time: u32,
    },
    /// `SMSG_BINDPOINTUPDATE` (login, re-bind): the hearth point and the AreaTable id `$z` names.
    BindPoint {
        position: Vector3d,
        map: u32,
        area: u32,
    },
    /// `SMSG_GMTICKET_GETTICKET`: our open ticket, `None` for "no ticket". Also pushed unasked
    /// when a GM views, escalates or completes it (`GMTicketMgr.cpp:153-159`).
    GmTicketAnswer {
        ticket: Option<Box<GmTicket>>,
    },
    /// `SMSG_GMTICKET_CREATE`: 2 created, 3 refused, 1 already have one; vmangos never sends 1
    /// and answers some refusals with silence.
    GmTicketCreated {
        response: u32,
    },
    /// `SMSG_GMTICKET_UPDATETEXT`: the answer to an edit, 4 saved, 5 refused.
    GmTicketUpdated {
        response: u32,
    },
    /// `SMSG_GMTICKET_DELETETICKET`: 9 = deleted. Also sent unasked when a GM runs
    /// `.ticket delete` (`TicketCommands.cpp:100-103`).
    GmTicketDeleted {
        response: u32,
    },
    /// `SMSG_GMTICKETSYSTEMSTATUS`: 1 = tickets accepted, 0 = not; vmangos sends it only on ask.
    GmTicketSystemStatus {
        status: i32,
    },
    /// `SMSG_GM_TICKET_STATUS_UPDATE`: 1 updated, 2 closed, 3 survey; vmangos never sends it.
    GmTicketStatusUpdate {
        status: u32,
    },
    /// `SMSG_BINDER_CONFIRM`: an innkeeper's question; `CMSG_BINDER_ACTIVATE` with the guid binds.
    BinderConfirm {
        binder: u64,
    },
    /// `MSG_TALENT_WIPE_CONFIRM` inbound: a trainer offers a talent wipe for `cost`; nothing is
    /// unlearned until the opcode goes back with `trainer`. `trainer == 0` means no talents.
    TalentWipeConfirm {
        trainer: u64,
        cost: u32,
    },
    /// `SMSG_PET_UNLEARN_CONFIRM`: a pet trainer asks; `CMSG_PET_UNLEARN` with the guid confirms.
    PetUnlearnConfirm {
        trainer: u64,
        cost: u32,
    },
    /// `SMSG_RAID_GROUP_ONLY`: a delay arms the instance boot; zero clears it and shows `reason`.
    RaidGroupOnly {
        delay_ms: u32,
        reason: u32,
    },
    /// `SMSG_AREA_SPIRIT_HEALER_TIME`: a battleground spirit healer's next wave.
    AreaSpiritHealerTime {
        healer: u64,
        ms: u32,
    },
    /// `SMSG_BATTLEFIELD_STATUS`: one of the three queue slots.
    BattlefieldStatus(crate::messages::BattlefieldStatus),
    /// `MSG_PVP_LOG_DATA` inbound: the battleground scoreboard.
    PvpLogData(crate::messages::PvpLogData),
    /// `SMSG_BATTLEFIELD_LIST`: the battleground instance list.
    BattlefieldList(crate::messages::BattlefieldList),
    /// `MSG_BATTLEGROUND_PLAYER_POSITIONS` inbound: the teammates and the flag carrier.
    BattlefieldPositions(crate::messages::BattlefieldPositions),
    /// `MSG_TABARDVENDOR_ACTIVATE` inbound: the vendor guid that opens the tabard designer.
    TabardVendorActivate(u64),
    /// `MSG_SAVE_GUILD_EMBLEM` inbound: the save's result row.
    SaveGuildEmblemResult(u32),
    /// `SMSG_GROUP_JOINED_BATTLEGROUND`: a group join's verdict.
    GroupJoinedBattleground {
        result: u32,
    },
    /// `SMSG_BATTLEGROUND_PLAYER_JOINED` / `_LEFT`: one guid each.
    BattlegroundPlayer {
        guid: u64,
        joined: bool,
    },
    /// `SMSG_MEETINGSTONE_SETQUEUE` (`0x295`): the meeting-stone queue state.
    MeetingStoneSetQueue {
        area: u32,
        status: u8,
    },
    /// `0x297`/`0x298`/`0x299`/`0x2BB`: the meeting stone's display-only replies.
    MeetingStoneNotice(crate::messages::MeetingStoneNotice),
    /// `SMSG_TUTORIAL_FLAGS`: the account's tutorial bank.
    TutorialFlags(crate::messages::TutorialFlags),
    /// `SMSG_SUMMON_REQUEST`: someone asks to pull us to them; nothing moves until
    /// `CMSG_SUMMON_RESPONSE`, and with no decline opcode the offer just expires.
    SummonRequest {
        summoner: u64,
        zone: u32,
        delay_ms: u32,
    },
    /// `SMSG_PLAYERBOUND`: the bind took (the "now your home" line), beside [`Self::BindPoint`].
    PlayerBound {
        binder: u64,
        area: u32,
    },
    /// `SMSG_SET_PROFICIENCY`: equippable subclasses of item class 2 or 4 (`0xc4d4a0[class]`).
    SetProficiency {
        item_class: u8,
        subclass_mask: u32,
    },
    /// `SMSG_INITIALIZE_FACTIONS` (login): `(flags, standing)` for all 64 slots by `Faction.dbc`
    /// `reputationIndex`. Standing excludes the race/class `BaseRepValue`, which the client adds.
    InitializeFactions {
        standings: Vec<(u8, i32)>,
    },
    /// `SMSG_SET_FACTION_STANDING`: `(reputationListId, standing)` per changed slot, base excluded.
    SetFactionStanding {
        standings: Vec<(u32, i32)>,
    },
    /// `SMSG_SET_FACTION_VISIBLE`: sets `FACTION_FLAG_VISIBLE` on one slot; it carries no standing.
    SetFactionVisible {
        list_id: u32,
    },
    /// `SMSG_NAME_QUERY_RESPONSE`: guid, name, realm (empty on one realm), race/gender/class as
    /// `u32`s (`NameQueryResponse::AppendBodyTo`); an unknown guid gets an empty name.
    NameQueryResponse {
        guid: u64,
        name: String,
        race: u32,
        gender: u32,
        class: u32,
    },
    /// `SMSG_CREATURE_QUERY_RESPONSE`: a template's UI head; `unk` and the pet spell-list id are
    /// read and dropped. A miss is a lone `entry | 0x8000_0000` (`HandleCreatureQueryOpcode`).
    CreatureQueryResponse {
        entry: u32,
        info: Option<CreatureQueryInfo>,
    },
    /// `SMSG_PET_NAME_QUERY_RESPONSE`: keyed by pet number, as the client's pet-name cache is; the
    /// `nameTimestamp` tail, which only ages that on-disk cache, is read and dropped.
    PetNameQueryResponse {
        pet_number: u32,
        name: String,
    },
    /// `SMSG_GAMEOBJECT_QUERY_RESPONSE`: a template's type, display, name and raw type-specific
    /// `data[24]`; a miss is a lone `entry | 0x8000_0000`.
    GameObjectQueryResponse {
        entry: u32,
        info: Option<GameObjectQueryInfo>,
    },
    /// `SMSG_PAGE_TEXT_QUERY_RESPONSE`: one book page; `next_page_id == 0` is the last, and
    /// vmangos answers one query with every page of the chain.
    PageTextQueryResponse {
        page_id: u32,
        text: String,
        next_page_id: u32,
    },
    /// `SMSG_GAMEOBJECT_CUSTOM_ANIM`: `u64 guid, u32 animId`; the client arms substate
    /// `8 + animId` (AnimationData 153..156, `animId >= 4` rejected). The bobber's bite is 0.
    GameObjectCustomAnim {
        guid: u64,
        anim_id: u32,
    },
    /// `SMSG_OPEN_CONTAINER`: a bag's raw `u64` guid on equip; the reference only fires `BAG_OPEN`.
    OpenContainer {
        item: u64,
    },
    /// `SMSG_INSPECT`: the target's raw `u64` guid, ignored, as the reference ignores it.
    Inspect {
        guid: u64,
    },
    /// `SMSG_STANDSTATE_UPDATE`: the server set our stand state (`u8`), applied ungated.
    StandStateUpdate {
        state: u8,
    },
    /// `SMSG_GAMEOBJECT_DESPAWN_ANIM`: a bare `u64` guid. The client arms substate 12 (anim 157)
    /// and pins the object, so the same tick's `SMSG_DESTROY_OBJECT` waits for the play to end.
    GameObjectDespawnAnim {
        guid: u64,
    },
    /// `SMSG_FISH_NOT_HOOKED`: the channel ended unhooked (empty body; `ERR_FISH_NOT_HOOKED`).
    FishNotHooked,
    /// `SMSG_FISH_ESCAPED`: the skill roll failed on the click (empty body; `ERR_FISH_ESCAPED`).
    FishEscaped,
    /// `SMSG_PLAY_SOUND`: one `u32` SoundEntries id, played 2D (`Map::PlayDirectSoundToMap`).
    PlaySound {
        sound_id: u32,
    },
    /// `SMSG_PLAY_MUSIC`: one `u32` SoundEntries id for the music channel.
    PlayMusic {
        music_id: u32,
    },
    /// `SMSG_PLAY_OBJECT_SOUND`: `u32 soundId, u64 sourceGuid`, played 3D at the source object.
    PlayObjectSound {
        sound_id: u32,
        guid: u64,
    },
    /// `SMSG_WEATHER`: `u32 type, f32 grade, u32 soundId, u8 instant`; sounds 8533..8558, 0 clear.
    Weather {
        weather_type: u32,
        grade: f32,
        sound_id: u32,
        instant: bool,
    },
    /// `SMSG_TEXT_EMOTE`: `u64 guid, u32 textEmote, u32 emoteNum, u32 namelen, char name[namelen]`
    /// (`EmoteChatBuilder`; a lone NUL for no target). The raw `target_name` picks the sentence
    /// form, as in the reference; `emoteNum` is read and ignored, as the reference ignores it.
    TextEmote {
        guid: u64,
        text_emote: u32,
        target_name: String,
    },
    /// `SMSG_EMOTE`: `u32 emoteId (Emotes.dbc), u64 guid`, an animation emote.
    Emote {
        guid: u64,
        emote_id: u32,
    },
    /// `SMSG_ITEM_QUERY_SINGLE_RESPONSE`: the full item template, boxed for size; `None` is a miss.
    ItemQueryResponse {
        entry: u32,
        info: Option<Box<ItemInfo>>,
    },
    /// `SMSG_MESSAGECHAT`: one chat line; GM dot-commands answer as system lines (type `0x0A`).
    MessageChat(ChatMessage),
    /// `SMSG_CHANNEL_NOTIFY`: a channel join, leave, error or moderation notice.
    ChannelNotify(ChannelNotify),
    /// `SMSG_CHANNEL_LIST`: a channel's roster, `(guid, memberFlags)` per member.
    ChannelList {
        channel: String,
        flags: u8,
        members: Vec<(u64, u8)>,
    },
    /// `SMSG_CHAT_PLAYER_NOT_FOUND`: the whisper target is not online
    /// (`Server/Packets/Chat.cpp:26-29`).
    ChatPlayerNotFound {
        name: String,
    },
    /// `SMSG_CHAT_WRONG_FACTION`: a cross-faction whisper was refused; empty body.
    ChatWrongFaction,
    /// `SMSG_NOTIFICATION`: one cstring server notice (`WorldSession.cpp:900-915`).
    Notification {
        text: String,
    },
    /// `SMSG_AREA_TRIGGER_MESSAGE`: why a trigger refused us, as `u32 length` then a cstring
    /// (`WorldSession.cpp:883-898`).
    AreaTriggerMessage {
        text: String,
    },
    /// `SMSG_SERVER_MESSAGE`: a `ServerMessages.dbc` row (shutdown, broadcast) and its `%s`.
    ServerMessage {
        message_type: u32,
        text: String,
    },
    /// `SMSG_ZONE_UNDER_ATTACK`: one `AreaTable.dbc` id under attack by enemy players.
    ZoneUnderAttack {
        area_id: u32,
    },
    /// `SMSG_DEFENSE_MESSAGE`: a world-defense broadcast (EPL towers), the zone and composed text.
    DefenseMessage {
        zone_id: u32,
        text: String,
    },
    /// `SMSG_CHAT_RESTRICTED`: a trial account hit its whisper cap; empty body.
    ChatRestricted,
    /// `SMSG_PLAYED_TIME` (`/played`): total and this-level played time, in seconds.
    PlayedTime {
        total: u32,
        level: u32,
    },
    /// `MSG_RANDOM_ROLL`: the server's `/random` broadcast.
    RandomRoll {
        min: u32,
        max: u32,
        roll: u32,
        guid: u64,
    },
    /// `SMSG_INVENTORY_CHANGE_FAILURE`: an `InventoryResult`; `required_level` rides only reason 1.
    InventoryChangeFailure {
        reason: u8,
        required_level: Option<u32>,
        item_guid: u64,
        /// The destination bag's absolute player slot (255 = the player's own array).
        bag_slot: u8,
    },
    /// `SMSG_INITIAL_SPELLS`: the spell book and active cooldowns, once at login.
    InitialSpells {
        spell_ids: Vec<u16>,
        cooldowns: Vec<SpellCooldown>,
    },
    /// `SMSG_ACTION_BUTTONS` (login): the occupied slots of the 120-slot action bar array.
    ActionButtons {
        buttons: Vec<ActionButton>,
    },
    /// `SMSG_LEARNED_SPELL`: a spell joined the book after login.
    LearnedSpell {
        spell_id: u16,
    },
    /// `SMSG_REMOVED_SPELL`: one spell left the book; a talent wipe sends one per rank per talent.
    RemovedSpell {
        spell_id: u16,
    },
    /// `SMSG_SUPERCEDED_SPELL`: a rank-up replaced its predecessor in the book and action bar.
    SupercededSpell {
        old_spell_id: u16,
        new_spell_id: u16,
    },
    /// `SMSG_CAST_RESULT`: the server's verdict on our `CMSG_CAST_SPELL`.
    CastResult {
        spell_id: u32,
        outcome: CastOutcome,
    },
    /// `SMSG_PET_SPELLS`: the pet bar's whole state; a zero `pet_guid` tears it down.
    PetSpells(PetSpells),
    /// `SMSG_PET_MODE`: the pet's react and command state alone.
    PetMode(PetMode),
    /// `SMSG_PET_ACTION_FEEDBACK`: one reason byte for a refused pet order.
    PetActionFeedback {
        reason: u8,
    },
    /// `SMSG_PET_CAST_FAILED`: the pet's cast refusal, in `SMSG_CAST_RESULT`'s vocabulary.
    PetCastFailed {
        spell_id: u32,
        outcome: CastOutcome,
    },
    /// `SMSG_PET_TAME_FAILURE`: one `PetTameFailureReason` byte.
    PetTameFailure {
        reason: u8,
    },
    /// `SMSG_PET_NAME_INVALID`: the rename was refused; empty body.
    PetNameInvalid,
    /// `SMSG_PET_BROKEN`: the pet's loyalty hit zero and it ran away; empty body.
    PetBroken,
    /// `SMSG_PET_ACTION_SOUND`: the pet's voice; `talk` is [`super::pet::PET_TALK_ORDER`] or
    /// [`super::pet::PET_TALK_ATTACK`].
    PetActionSound {
        pet_guid: u64,
        talk: u32,
    },
    /// `SMSG_PET_DISMISS_SOUND`: a `CreatureModelData` id whose column-29 kit plays at `position`.
    PetDismissSound {
        model_id: u32,
        position: Vector3d,
    },
    /// `SMSG_ATTACKSTART`: a unit began melee auto-attack, our own echo included.
    AttackStart {
        attacker: u64,
        victim: u64,
    },
    /// `SMSG_ATTACKSTOP`: a unit stopped melee auto-attack.
    AttackStop {
        attacker: u64,
        victim: u64,
    },
    /// `SMSG_ATTACKERSTATEUPDATE`: one completed melee swing, the attacker's swing-animation cue.
    AttackerState(AttackerState),
    /// The server refused our `CMSG_ATTACKSWING` (`SMSG_ATTACKSWING_*` `0x145`, `0x146`, `0x148`,
    /// `0x149`, all empty), collapsed to the three arms the reference wires.
    AttackSwingError(AttackSwingError),
    /// `SMSG_CANCEL_COMBAT` (`0x14e`, empty): stop our attack, no message (reference `0x5e7dd0`).
    CancelCombat,
    /// `SMSG_FEIGN_DEATH_RESISTED` (`0x2b4`, empty): a bare `DisplayError(421)` (`0x6e9800`).
    FeignDeathResisted,
    /// `SMSG_AI_REACTION`: a creature's aggro flare (2 HOSTILE) or stealth alert (0 ALERT).
    AiReaction {
        unit: u64,
        reaction: u32,
    },
    /// `SMSG_SPELL_START`: a non-triggered cast began, instants included.
    SpellStart(SpellStart),
    /// `SMSG_SPELL_GO`: the cast launched, with hit and miss lists and a ranged spell's ammo;
    /// missile travel is not on the wire, the server times impact off `Spell.dbc` Speed.
    SpellGo(SpellGo),
    SpellChainTargets(SpellChainTargets),
    /// `SMSG_SPELL_FAILED_OTHER`: an observed cast was interrupted; ours is [`Self::CastResult`].
    SpellFailedOther {
        caster: u64,
        spell_id: u32,
    },
    /// `SMSG_SPELL_DELAYED`: pushback; a hit extended our cast by `delay_ms` (`Spell::Delayed`).
    SpellDelayed {
        caster: u64,
        delay_ms: u32,
    },
    /// `SMSG_CANCEL_AUTO_REPEAT` (self-only, empty): stop our ranged auto-repeat; vmangos sends it
    /// on every autorepeat interrupt, target death included (`Player::SendAutoRepeatCancel`).
    CancelAutoRepeat,
    /// `SMSG_SPELL_COOLDOWN`: server-pushed cooldowns for the player or pet, by caster guid.
    SpellCooldownList {
        caster: u64,
        /// `(spell_id, cooldown_ms)`; `0` ms means the spell's own DBC recovery and category times.
        cooldowns: Vec<(u32, u32)>,
    },
    /// `SMSG_ITEM_COOLDOWN`: put an item instance on the client's fixed 30 s on-use cooldown.
    ItemCooldown {
        item_guid: u64,
        spell_id: u32,
    },
    /// `SMSG_ITEM_TIME_UPDATE`: a duration-limited item instance's time left, in seconds.
    ItemTime {
        item_guid: u64,
        seconds: u32,
    },
    /// `SMSG_ITEM_ENCHANT_TIME_UPDATE`: a temporary enchant's time left, the tooltip's only source.
    ItemEnchantTime {
        item_guid: u64,
        slot: u32,
        seconds: u32,
    },
    /// `SMSG_SET_FLAT_SPELL_MODIFIER` / `SMSG_SET_PCT_SPELL_MODIFIER`: one absolute cell of a
    /// talent modifier table; one handler reads both (`0x6e9950`) and the opcode picks the table.
    SpellModifier {
        /// `true`: `0x266`, the flat table (summed, added). `false`: `0x267`, the percent table
        /// (summed, plus 100, clamped at 0, then a raw multiplier).
        flat: bool,
        /// The row: a `SpellFamilyFlags` bit index, 0..=63, unbounded on the wire. Deviation: the
        /// consumer refuses 64 and up, because there the reference's store overruns its table.
        mask_bit: u8,
        /// The column: a SpellModOp, 0..=28, also unbounded on the wire.
        op: u8,
        /// The cell's new value, signed and absolute, never a delta.
        value: i32,
    },
    /// `SMSG_COOLDOWN_EVENT`: start a parked `SPELL_ATTR_COOLDOWN_ON_EVENT` cooldown now.
    CooldownEvent {
        spell_id: u32,
        caster: u64,
    },
    /// `SMSG_CLEAR_COOLDOWN`: remove one spell's cooldown record outright.
    ClearCooldown {
        spell_id: u32,
        caster: u64,
    },
    /// `SMSG_COOLDOWN_CHEAT`: wipe every cooldown for the named unit (the GM reset).
    CooldownCheat {
        caster: u64,
    },
    /// `MSG_CHANNEL_START`: our own channel opened (self-only, no guid on the wire).
    ChannelStart {
        spell_id: u32,
        duration_ms: u32,
    },
    /// `MSG_CHANNEL_UPDATE` (self-only): our channel's time left; `0` ends it, end or interrupt.
    ChannelUpdate {
        remaining_ms: u32,
    },
    /// `SMSG_UPDATE_AURA_DURATION` (self-only, never for a permanent aura): one aura slot's time
    /// left. It arrives before the delta naming the slot's spell, so buffer it by slot.
    UpdateAuraDuration {
        slot: u8,
        remaining_ms: u32,
    },
    /// `SMSG_PLAY_SPELL_VISUAL`: a spell-visual kit on a unit outside a cast, at the fixed stage 0.
    PlaySpellVisual {
        unit: u64,
        kit_id: u32,
    },
    /// `SMSG_SPELLNONMELEEDAMAGELOG`: non-melee (spell) damage dealt.
    SpellDamageLog(SpellDamageLog),
    /// `SMSG_PERIODICAURALOG`: periodic (DoT, HoT, regen) aura ticks.
    PeriodicAuraLog(PeriodicAuraLog),
    /// `SMSG_SPELLHEALLOG`: a direct heal landing.
    SpellHealLog(SpellHealLog),
    /// `SMSG_SPELLENERGIZELOG`: an instant power gain.
    SpellEnergizeLog(SpellEnergizeLog),
    /// `SMSG_SPELLDAMAGESHIELD`: a damage-shield (Thorns-style) return hit.
    DamageShield(DamageShield),
    /// `SMSG_ENVIRONMENTALDAMAGELOG`: environmental damage taken (falling, drowning, …).
    EnvironmentalDamageLog(EnvironmentalDamageLog),
    /// `SMSG_SPELLLOGMISS`: a cast's per-target miss list.
    SpellLogMiss(SpellLogMiss),
    /// `SMSG_PARTYKILLLOG`: the killing blow.
    PartyKillLog(PartyKillLog),
    /// `SMSG_SPELLINSTAKILLLOG`: an instant kill.
    SpellInstaKillLog(SpellInstaKillLog),
    /// `SMSG_PROCRESIST`: a proc the target resisted.
    ProcResist(SpellOutcomeLog),
    /// `SMSG_SPELLORDAMAGE_IMMUNE`: a target immune to the spell.
    SpellOrDamageImmune(SpellOutcomeLog),
    /// `SMSG_SPELLDISPELLOG`: the auras a dispel removed.
    SpellDispelLog(SpellDispelLog),
    /// `SMSG_DISPEL_FAILED`: the auras a dispel failed to remove.
    DispelFailed(DispelFailed),
    /// `SMSG_ENCHANTMENTLOG`: an enchant landing on or fading from an item.
    EnchantmentLog(EnchantmentLog),
    /// `SMSG_SPELLLOGEXECUTE`: what a cast's effects did: created items, interrupts, drains, ….
    SpellLogExecute(SpellLogExecute),
    /// `SMSG_LOG_XPGAIN`: an XP award, kill or non-kill.
    XpGain(XpGain),
    /// `SMSG_EXPLORATION_EXPERIENCE`: a first visit to an area, its id and XP award.
    ExplorationXp(ExplorationXp),
    /// `SMSG_LEVELUP_INFO`: our own level-up, self-addressed only.
    LevelUp(LevelUpInfo),
    /// `SMSG_QUESTGIVER_STATUS`: one NPC's `!`/`?` marker, a [`super::quest::dialog_status`] value.
    QuestGiverStatus {
        npc: u64,
        status: u32,
    },
    /// `SMSG_QUESTGIVER_QUEST_LIST`: the greeting panel, an NPC's offered and active quests.
    QuestGiverQuestList(QuestGiverList),
    /// `SMSG_QUESTGIVER_QUEST_DETAILS`: the accept panel, full quest text and rewards.
    QuestGiverDetails(QuestDetails),
    /// `SMSG_QUESTGIVER_REQUEST_ITEMS`: the progress panel, required items and completability.
    QuestGiverRequestItems(QuestRequestItems),
    /// `SMSG_QUESTGIVER_OFFER_REWARD`: the reward panel, turn-in text and rewards.
    QuestGiverOfferReward(QuestOfferReward),
    /// `SMSG_QUESTGIVER_QUEST_COMPLETE`: the turn-in result, XP, money and fixed items.
    QuestGiverComplete(QuestComplete),
    /// `SMSG_QUESTGIVER_QUEST_INVALID` (`Quest.cpp:126`): accept or query refused; `msg` says why.
    QuestGiverInvalid {
        msg: u32,
    },
    /// `SMSG_QUESTGIVER_QUEST_FAILED` (`Quest.cpp:110`): `reason` is a `QuestFailedReason` code.
    QuestGiverFailed {
        quest_id: u32,
        reason: u32,
    },
    /// `SMSG_QUEST_QUERY_RESPONSE`: the full quest template (400+ bytes, hence boxed).
    QuestQueryResponse(Box<QuestTemplate>),
    /// `SMSG_QUESTLOG_FULL`: no free log slot for a new quest; empty body (`Quest.cpp:87`).
    QuestLogFull,
    /// `MSG_QUEST_PUSH_RESULT`: a member's verdict on a quest we shared, sent to the sharer; per
    /// member the server sends `SHARING_QUEST` first, then the real outcome.
    QuestPushResult(QuestPushResult),
    /// `SMSG_QUEST_CONFIRM_ACCEPT`: join a member's `QUEST_FLAGS_PARTY_ACCEPT` (escort) quest?
    QuestConfirmAccept(QuestConfirmAccept),
    /// `SMSG_QUESTUPDATE_COMPLETE`: all objectives done; the slot becomes `QUEST_STATE_COMPLETE`.
    QuestUpdateComplete {
        quest_id: u32,
    },
    /// `SMSG_QUESTUPDATE_FAILED`: the quest failed outright (`Quest.cpp:116`).
    QuestUpdateFailed {
        quest_id: u32,
    },
    /// `SMSG_QUESTUPDATE_FAILEDTIMER`: a timed quest's clock ran out (`Quest.cpp:121`).
    QuestUpdateFailedTimer {
        quest_id: u32,
    },
    /// `SMSG_QUESTUPDATE_ADD_KILL`: a kill or use toast, `entry` encoded as
    /// [`super::quest::QuestObjective::creature_or_go`]; the durable count is `PLAYER_QUEST_LOG`.
    QuestUpdateAddKill {
        quest_id: u32,
        entry: u32,
        count: u32,
        required: u32,
        guid: u64,
    },
    /// `SMSG_QUESTUPDATE_ADD_ITEM`: an item-collection toast (`Quest.cpp:138`).
    QuestUpdateAddItem {
        item_id: u32,
        count: u32,
    },
    /// `SMSG_GOSSIP_MESSAGE`: an NPC's gossip menu and quest rows (`GossipDef.cpp:180-225`, no
    /// box-money field in 1.12); `text_id` is for a follow-up `CMSG_NPC_TEXT_QUERY`.
    GossipMessage {
        npc: u64,
        text_id: u32,
        options: Vec<GossipOption>,
        quests: Vec<QuestOption>,
    },
    /// `SMSG_GOSSIP_COMPLETE`: the gossip window closes; empty body (`Npc.cpp:90`).
    GossipComplete,
    /// `SMSG_GOSSIP_POI`: a guard's directions marker, unrequested (`GossipDef.cpp:253`).
    GossipPoi(GossipPoi),
    /// `SMSG_NPC_TEXT_UPDATE`: always 8 weighted text blocks (`GossipDef.cpp:298-369`), kept
    /// undrawn; [`super::gossip::select_greeting`] picks one by gender and a roll at frame open.
    NpcText {
        text_id: u32,
        blocks: Vec<super::gossip::NpcTextBlock>,
    },
    /// `SMSG_LIST_INVENTORY`: a vendor's stock (`ItemHandler.cpp:741-810`); an empty stock sends
    /// `count = 0` and one error byte, always 0, which is dropped.
    VendorList {
        vendor: u64,
        items: Vec<VendorItem>,
    },
    /// `SMSG_BUY_ITEM`: the vendor's stock after a purchase (`Server/Packets/Item.cpp:190-196`).
    BuyItem {
        vendor: u64,
        slot: u32,
        new_count: u32,
        purchase_count: u32,
    },
    /// `SMSG_SELL_ITEM`: errors only (`Player.cpp:11723`), a [`super::vendor::sell_result`] code;
    /// a successful sell sends nothing and shows only in the descriptors.
    SellItemResult {
        vendor: u64,
        item_guid: u64,
        reason: u8,
    },
    /// `SMSG_BUY_FAILED`: a refused purchase, `reason` a [`super::vendor::buy_result`] code.
    BuyFailed {
        vendor: u64,
        item_entry: u32,
        reason: u8,
    },
    /// `SMSG_SHOW_BANK`: open the bank window, answering `CMSG_BANKER_ACTIVATE` or unprompted
    /// from the banker gossip option (`Player.cpp:12426`); the vault streams in the descriptor.
    ShowBank {
        banker: u64,
    },
    /// `SMSG_BUY_BANK_SLOT_RESULT`: a refused slot purchase, a [`super::bank::bank_slot_result`];
    /// success sends nothing, only `PLAYER_BYTES_2`'s bank-bag count advances.
    BuyBankSlotResult {
        result: u32,
    },
    /// `SMSG_TRAINER_LIST`: services, greeting; `trainer_type` 0 class, 1 mount, 2 trade, 3 pet.
    TrainerList {
        trainer: u64,
        trainer_type: u32,
        services: Vec<TrainerSpell>,
        title: String,
    },
    /// `SMSG_TRAINER_BUY_SUCCEEDED`: confirmation and sound only; the spell arrives as
    /// `SMSG_LEARNED_SPELL`, and repainting the list takes a fresh `CMSG_TRAINER_LIST`.
    TrainerBuySucceeded {
        trainer: u64,
        spell_id: u32,
    },
    /// `SMSG_TRAINER_BUY_FAILED`: `error` is a [`super::trainer::train_fail`] code.
    TrainerBuyFailed {
        trainer: u64,
        spell_id: u32,
        error: u32,
    },
    /// `MSG_LIST_STABLED_PETS`: opens the stable unprompted and is its only refresh when we send
    /// it. `num_stable_slots` is slots bought (0..=2); `pets` must be read by
    /// [`StabledPet::slot`] (`0` = current), never by position, as the current pet may be absent.
    ListStabledPets {
        npc: u64,
        num_stable_slots: u8,
        pets: Vec<StabledPet>,
    },
    /// `SMSG_INVALIDATE_PLAYER`: drop this guid from the player-name cache, which ages nothing out.
    InvalidatePlayer {
        guid: u64,
    },
    /// `SMSG_STABLE_RESULT`: the whole answer to a stable ask, a [`super::stable::stable_result`]
    /// code; success carries no list, so a repaint takes a fresh `MSG_LIST_STABLED_PETS`.
    StableResult {
        result: u8,
    },
    /// `SMSG_LOOT_RESPONSE`, normal shape: a loot window opened. `loot_type` is a
    /// [`super::loot::loot_type`] code; `items` includes quest-item rows.
    LootResponse {
        guid: u64,
        loot_type: u8,
        gold: u32,
        items: Vec<LootItem>,
    },
    /// `SMSG_LOOT_RESPONSE`, error shape: `error` is a [`super::loot::loot_error`] code.
    LootError {
        guid: u64,
        error: u8,
    },
    /// `SMSG_LOOT_RELEASE_RESPONSE`: the loot window closes; vmangos always sends `result` 1.
    LootReleaseResponse {
        guid: u64,
        result: u8,
    },
    /// `SMSG_LOOT_REMOVED`: one loot-window row was taken, by anyone.
    LootRemoved {
        slot: u8,
    },
    /// `SMSG_LOOT_MONEY_NOTIFY`: our share of the loot's coin pile, answering `CMSG_LOOT_MONEY`.
    LootMoneyNotify {
        amount: u32,
    },
    /// `SMSG_LOOT_CLEAR_MONEY`: the coin line disappears for every current looter; empty body.
    LootClearMoney,
    /// `SMSG_LOOT_START_ROLL`: a group roll opened on one drop, a `GroupLootFrame`.
    LootStartRoll(LootStartRoll),
    /// `SMSG_LOOT_ROLL`: one roller's vote or dice result.
    LootRoll(LootRoll),
    /// `SMSG_LOOT_ROLL_WON`: a group roll resolved.
    LootRollWon(LootRollWon),
    /// `SMSG_LOOT_ALL_PASSED`: everyone passed; the item goes back to ordinary corpse looting.
    LootAllPassed(LootAllPassed),
    /// `SMSG_LOOT_MASTER_LIST`: master-loot candidates; precedes its `SMSG_LOOT_RESPONSE`.
    LootMasterList {
        candidates: Vec<u64>,
    },
    /// `SMSG_ITEM_PUSH_RESULT`: an item landed in our bags, the "You receive loot" line.
    ItemPushResult(ItemPushResult),
    /// `MSG_CORPSE_QUERY` answer: where our corpse is; pushed as not-found when it turns to bones.
    CorpseQuery(CorpseLocation),
    /// `SMSG_CORPSE_RECLAIM_DELAY`: ms until reclaim, sent at release and at a login while dead.
    CorpseReclaimDelay {
        delay_ms: u32,
    },
    /// `SMSG_DURABILITY_DAMAGE_DEATH` (empty): the 10% death durability loss, a red error line.
    DurabilityDamageDeath,
    /// `SMSG_RESURRECT_REQUEST`: a resurrection offer.
    ResurrectRequest(ResurrectRequestBody),
    /// `SMSG_SPIRIT_HEALER_CONFIRM`: the healer asks; `CMSG_SPIRIT_HEALER_ACTIVATE` targets `npc`.
    SpiritHealerConfirm {
        npc: u64,
    },
    /// Root, water-walk, feather-fall or hover granted (`apply`) or revoked on our mover. Unless
    /// acked with `counter` ([`MoveMode::ack_opcode`]), the server never applies it.
    MoveMode {
        guid: u64,
        counter: u32,
        mode: MoveMode,
        apply: bool,
    },
    /// The twelve `SMSG_SPLINE_MOVE_*`: any unit's mode change, a bare packed guid with no ack.
    /// `apply` is the direction of [`SplineMode::flag`]'s bit, run/walk inversion already folded.
    SplineMoveMode {
        guid: u64,
        mode: SplineMode,
        apply: bool,
    },
    /// `SMSG_MOVE_KNOCK_BACK`: a launch for our mover to fly, on the wire as `vcos, vsin, speedXY,
    /// speedZ`, not [`JumpInfo`]'s `zspeed`-first order; `zspeed` is down-positive. Owes
    /// `CMSG_MOVE_KNOCK_BACK_ACK` with `counter` and a jump tail of exactly this quad.
    KnockBack {
        guid: u64,
        counter: u32,
        launch: JumpInfo,
    },
    /// `SMSG_LOGOUT_COMPLETE`: the world session is over; we are back at character select.
    LogoutComplete,
    /// `SMSG_LOGOUT_RESPONSE`: `{u32 reason, u8 instant}` (reference `0x5b4630`). A non-zero
    /// `reason` refuses (1 combat, 2 frozen, 3 falling); `instant` (resting, taxi, GM) means
    /// `LogoutComplete` follows at once, else a 20 s server timer runs, the CAMP countdown.
    LogoutResponse {
        reason: u32,
        instant: bool,
    },
    /// `SMSG_LOGOUT_CANCEL_ACK` (empty): the logout was cancelled; it takes the countdown down.
    LogoutCancelAck,
    /// `SMSG_PONG`: our `CMSG_PING`'s sequence echoed back, timed for the round trip.
    Pong {
        sequence: u32,
    },
    /// `SMSG_FORCE_*_SPEED_CHANGE`: one of our mover's six speeds, in yd/s (rad/s for turning);
    /// owes the matching `_ACK` with `counter` and the exact `speed`.
    ForceSpeedChange {
        guid: u64,
        kind: SpeedKind,
        counter: u32,
        speed: f32,
    },
    /// `SMSG_GROUP_INVITE`: someone invited us to their group.
    GroupInvite {
        inviter: String,
    },
    /// `SMSG_GROUP_DECLINE`: an invite we sent was declined.
    GroupDecline {
        name: String,
    },
    /// `SMSG_GROUP_UNINVITE`: we were removed from our group (kicked or left); empty body.
    GroupUninvited,
    /// `SMSG_GROUP_SET_LEADER`: the group's leader changed.
    GroupLeaderChanged {
        name: String,
    },
    /// `SMSG_GROUP_DESTROYED`: the group disbanded outright; empty body.
    GroupDestroyed,
    /// `SMSG_GROUP_LIST`: the full roster on every change, without our own row (see `own_flags`);
    /// `loot` is absent when there are no other members.
    GroupList {
        group_type: u8,
        own_flags: u8,
        members: Vec<GroupMemberEntry>,
        leader: u64,
        loot: Option<GroupLootInfo>,
    },
    /// `SMSG_PARTY_COMMAND_RESULT`: `operation` a [`super::group::party_operation`], `result` a
    /// [`super::group::party_result`].
    PartyCommandResult {
        operation: u32,
        member: String,
        result: u32,
    },
    /// `SMSG_PARTY_MEMBER_STATS` (delta) or `_FULL` (answering our request, or an offline miss).
    PartyMemberStats {
        guid: u64,
        full: bool,
        info: Box<PartyMemberStatsInfo>,
    },
    /// `MSG_MINIMAP_PING`: someone pinged the minimap for the group.
    MinimapPing {
        guid: u64,
        x: f32,
        y: f32,
    },
    /// `MSG_RAID_TARGET_UPDATE`, delta shape: one raid-target icon changed; `guid == 0` clears it.
    RaidTargetSet {
        icon: u8,
        guid: u64,
    },
    /// `MSG_RAID_TARGET_UPDATE`, full shape: every icon currently set.
    RaidTargetList {
        entries: Vec<(u8, u64)>,
    },
    /// `MSG_RAID_READY_CHECK`, empty body: the raid leader started a ready check.
    ReadyCheckRequest,
    /// `MSG_RAID_READY_CHECK`, non-empty body: one member's answer, forwarded to the leader only.
    ReadyCheckAnswer {
        guid: u64,
        ready: u8,
    },
    /// `SMSG_RAID_INSTANCE_INFO`: our saved raid lockouts, answering `CMSG_REQUEST_RAID_INFO`.
    RaidInstanceInfo {
        entries: Vec<super::group::RaidInstanceEntry>,
    },
    /// `SMSG_RAID_INSTANCE_MESSAGE`: a raid lockout's welcome or countdown line.
    RaidInstanceMessage {
        message: super::instance::RaidInstanceMessage,
    },
    /// `SMSG_INSTANCE_SAVE_CREATED`: "now saved"; vmangos sends 0 (the client's `1` line differs).
    InstanceSaveCreated {
        flag: u32,
    },
    /// `SMSG_INSTANCE_RESET`: the reset took; the payload is the `Map.dbc` id that was reset.
    InstanceReset {
        map: u32,
    },
    /// `SMSG_INSTANCE_RESET_FAILED`: the reset was refused.
    InstanceResetFailed {
        failure: super::instance::InstanceResetFailed,
    },
    /// `SMSG_UPDATE_LAST_INSTANCE`: the `Map.dbc` id of the dungeon we were last in.
    UpdateLastInstance {
        map: u32,
    },
    /// `SMSG_UPDATE_INSTANCE_OWNERSHIP`: non-zero while we hold at least one permanent bind.
    UpdateInstanceOwnership {
        owns: u32,
    },
    /// `SMSG_DUEL_REQUESTED`: sent to both sides; we issued it when `challenger` is our guid.
    DuelRequested {
        arbiter: u64,
        challenger: u64,
    },
    /// `SMSG_DUEL_OUTOFBOUNDS`: we left the 75 yd bubble around the duel flag; 10 s to return.
    DuelOutOfBounds,
    /// `SMSG_DUEL_INBOUNDS`: we came back inside (70 yd, the hysteresis edge).
    DuelInBounds,
    /// `SMSG_DUEL_COMPLETE`: `started` false means declined or cancelled ("Duel cancelled.").
    DuelComplete {
        started: bool,
    },
    /// `SMSG_DUEL_WINNER`: the outcome line, broadcast to everyone nearby.
    DuelWinner {
        fled: bool,
        winner: String,
        loser: String,
    },
    /// `SMSG_DUEL_COUNTDOWN`: the "Duel starting: N" tick, converted from the wire's milliseconds.
    DuelCountdown {
        seconds: u32,
    },
    /// The `MSG_INSPECT_HONOR_STATS` reply, only ever to our ask; a refused ask gets silence.
    InspectHonorStats(InspectHonorStats),
    /// `SMSG_PVP_CREDIT`: an attributed honor payout, negative for a dishonorable kill.
    PvpCredit(PvpCredit),
    /// `SMSG_START_MIRROR_TIMER`: start or restate a breath or fatigue bar; resent on each change.
    MirrorTimerStart(MirrorTimerStart),
    /// `SMSG_PAUSE_MIRROR_TIMER`: freeze or unfreeze a running timer; vmangos never sends it.
    MirrorTimerPause {
        kind: u32,
        paused: bool,
    },
    /// `SMSG_STOP_MIRROR_TIMER`: that timer is over; hide its bar.
    MirrorTimerStop {
        kind: u32,
    },
    /// `SMSG_FRIEND_LIST`: the whole friend list, pushed at login and on `CMSG_FRIEND_LIST`.
    FriendList {
        friends: Vec<FriendEntry>,
    },
    /// `SMSG_IGNORE_LIST`: the whole ignore list, guids only.
    IgnoreList {
        guids: Vec<u64>,
    },
    /// `SMSG_FRIEND_STATUS`: an add or remove ack, or a friend's login or logout.
    FriendStatus(FriendStatusUpdate),
    /// `SMSG_WHO`: the `/who` answer, up to 49 rows plus the true match total.
    WhoResults(WhoResults),
    /// `SMSG_GUILD_QUERY_RESPONSE`: a guild's name, ten rank names and tabard, cached by guild id.
    GuildQueryResponse(GuildQueryResponse),
    /// `SMSG_GUILD_ROSTER`: a complete snapshot, resent on any change: MOTD, info, ranks, members.
    GuildRoster(GuildRoster),
    /// `SMSG_GUILD_EVENT`: an event id and its formatted string arguments.
    GuildEvent(GuildEventNotice),
    /// `SMSG_GUILD_COMMAND_RESULT`: read the code with its command tag; `0x08` means two things.
    GuildCommandResult(GuildCommandResult),
    /// `SMSG_GUILD_INVITE`: an invite into a guild. `CMSG_GUILD_ACCEPT` and `CMSG_GUILD_DECLINE`
    /// echo nothing back, so the pending invite is client state.
    GuildInvite {
        inviter: String,
        guild: String,
    },
    /// `SMSG_GUILD_DECLINE`: the player we invited turned it down; sent only to the inviter.
    GuildDecline {
        name: String,
    },
    /// `SMSG_GUILD_INFO`: the founded, members and accounts summary, apart from the roster.
    GuildInfo(GuildInfo),
    /// `SMSG_PETITION_SHOWLIST`: a registrar's charter list, always one row; opens the window.
    PetitionShowList(PetitionShowList),
    /// `SMSG_PETITION_SHOW_SIGNATURES`: a charter's signers, answering our ask or someone's offer
    /// to us; only `owner` tells them apart.
    PetitionShowSignatures(PetitionShowSignatures),
    /// `SMSG_PETITION_SIGN_RESULTS`: one signature's verdict; on success both sides get it.
    PetitionSignResults(PetitionSignResults),
    /// `SMSG_PETITION_QUERY_RESPONSE`: the proposed guild name and signature requirement.
    PetitionQueryResponse(PetitionQueryResponse),
    /// `SMSG_TURN_IN_PETITION_RESULTS`: a bare result code; a name collision gets no packet.
    TurnInPetitionResults {
        result: u32,
    },
    /// `MSG_PETITION_DECLINE` inbound: who declined our charter, sent only to its owner.
    PetitionDeclined {
        player: u64,
    },
    /// `MSG_PETITION_RENAME` inbound: the echo of a rename that took, success only.
    PetitionRenamed(PetitionRename),
    /// `SMSG_SPLINE_SET_*_SPEED`: a unit we don't control, `[packed guid][f32 speed]`, no ack.
    SplineSpeedChange {
        guid: u64,
        kind: SpeedKind,
        speed: f32,
    },
    /// `MSG_MOVE_SET_*_SPEED`: `[packed guid][MovementInfo][f32 speed]`, a speed and a fresh pose.
    MoveSetSpeed {
        guid: u64,
        kind: SpeedKind,
        flags: u32,
        position: Vector3d,
        orientation: f32,
        /// Swim pitch (radians, +up), `0.0` unless swimming.
        pitch: f32,
        /// The `MovementInfo` time word, stamped by vmangos's clock at receipt.
        time: u32,
        fall_time: u32,
        jump: Option<JumpInfo>,
        /// The rider's pose on its transport while `MOVEFLAG_ON_TRANSPORT` is set.
        transport: Option<TransportPose>,
        speed: f32,
    },
    /// `SMSG_MOUNTRESULT` / `SMSG_DISMOUNTRESULT` (by `mount`): a raw `UnitMountResult` (OK = 10)
    /// or `UnitDismountResult` (OK = 3).
    MountResult {
        mount: bool,
        code: u32,
    },
    /// `SMSG_MOUNTSPECIAL_ANIM`: one raw `u64` guid, sent with `self` false
    /// (`MovementHandler.cpp:972`), so the sender's own echo depends on vmangos's broadcaster: off
    /// forces `self` true (`Object.cpp:2273-2274`), on keeps it false (`Object.cpp:2276-2280`).
    MountSpecialAnim {
        guid: u64,
    },
    /// `SMSG_CLIENT_CONTROL_UPDATE`: packed guid, `u8` allowMove (`Misc.cpp:677-682`); `mover`
    /// is our own guid when the server takes control away.
    ClientControlUpdate {
        mover: u64,
        allow_move: bool,
    },
    /// `SMSG_SHOWTAXINODES` (`TaxiHandler.cpp:82-96`): the taxi map; vmangos always writes
    /// `window` 1, and `nearest_node` is the flight master's own node.
    ShowTaxiNodes {
        window: u32,
        flightmaster: u64,
        nearest_node: u32,
        known: TaxiMask,
    },
    /// `SMSG_TAXINODE_STATUS`: answers the status query, and rides a first-visit learn
    /// (`TaxiHandler.cpp:117-138`). `guid` is plain; `known` means its node is in our mask.
    TaxiNodeStatus {
        guid: u64,
        known: bool,
    },
    /// `SMSG_ACTIVATETAXIREPLY`: a [`super::taxi_reply`]; `0` is OK, anything else refuses.
    ActivateTaxiReply {
        code: u32,
    },
    /// `SMSG_NEW_TAXI_PATH` (empty): rides a first-visit learn beside [`Self::TaxiNodeStatus`].
    NewTaxiPath,
    /// `SMSG_MAIL_LIST_RESULT`: the inbox, answering `CMSG_GET_MAIL_LIST`.
    MailList {
        mails: Vec<MailListEntry>,
    },
    /// `SMSG_SEND_MAIL_RESULT`: a [`super::mail::mail_action`] and [`super::mail::mail_error`];
    /// `equip_error` and `item` are mutually exclusive tails.
    SendMailResult {
        mail_id: u32,
        action: u32,
        error: u32,
        equip_error: Option<u32>,
        item: Option<(u32, u32)>,
    },
    /// `SMSG_ITEM_TEXT_QUERY_RESPONSE`: a letter's body, fetched once for a nonzero `item_text_id`.
    ItemTextQueryResponse {
        text_id: u32,
        text: String,
    },
    /// `SMSG_RECEIVED_MAIL`: a mail arrived; `seconds` until it waits, always `0.0` from vmangos,
    /// though the client runs it through its countdown.
    ReceivedMail {
        seconds: f32,
    },
    /// `MSG_QUERY_NEXT_MAIL_TIME` reply: `0.0` means unread mail waits, `-86400.0` none.
    NextMailTime {
        seconds: f32,
    },
    /// `MSG_AUCTION_HELLO` reply: the auctioneer and its `AuctionHouse.dbc` row (1..7, the deposit
    /// and cut rates); this reply, not our send, opens the window.
    AuctionHello {
        auctioneer: u64,
        house_id: u32,
    },
    /// `SMSG_AUCTION_COMMAND_RESULT`: `action` an [`super::auction::auction_action`], `error` an
    /// [`super::auction::auction_error`] that selects `tail`; `auction_id` is 0 on most failures.
    AuctionCommandResult {
        auction_id: u32,
        action: u32,
        error: u32,
        tail: AuctionCommandTail,
    },
    /// `SMSG_AUCTION_LIST_RESULT`: a Browse page; `total_count` is the match count before the
    /// 50-row cap and rides at the end of the body.
    AuctionListResult {
        auctions: Vec<AuctionListEntry>,
        total_count: u32,
    },
    /// `SMSG_AUCTION_OWNER_LIST_RESULT`: the Auctions tab (our listings), shaped as a Browse page.
    AuctionOwnerListResult {
        auctions: Vec<AuctionListEntry>,
        total_count: u32,
    },
    /// `SMSG_AUCTION_BIDDER_LIST_RESULT`: the Bid tab; the server lists the refreshed ids, then
    /// our live bids, so a row can appear twice.
    AuctionBidderListResult {
        auctions: Vec<AuctionListEntry>,
        total_count: u32,
    },
    /// `SMSG_AUCTION_BIDDER_NOTIFICATION`: we won (`bid_or_zero == 0`) or were outbid.
    AuctionBidderNotification(AuctionBidderNotification),
    /// `SMSG_AUCTION_OWNER_NOTIFICATION`: our auction sold (all-zero bidder guid) or took a bid.
    AuctionOwnerNotification(AuctionOwnerNotification),
    /// `SMSG_AUCTION_REMOVED_NOTIFICATION`: an auction we had bid on was cancelled by its seller.
    AuctionRemovedNotification {
        auction_id: u32,
        item_entry: u32,
        random_property_id: i32,
    },
    /// `SMSG_TRADE_STATUS`: one step of the trade state machine, tails included.
    TradeStatus {
        status: TradeStatus,
    },
    /// `SMSG_TRADE_STATUS_EXTENDED`: one side's items and gold, pushed whenever that side changes.
    TradeStatusExtended {
        state: Box<TradeStatusExtended>,
    },
    /// `SMSG_INIT_WORLD_STATES`: a zone's whole world-state table, at login and each zone change.
    InitWorldStates(InitWorldStates),
    /// `SMSG_UPDATE_WORLD_STATE`: one `(id, value)` write into that table.
    UpdateWorldState {
        id: u32,
        value: u32,
    },
    /// `SMSG_ADDON_INFO` (`0x2ef`): one record per addon we sent in `CMSG_AUTH_SESSION`, in that
    /// order with no count or names, so record i is `STOCK_SECURE_ADDONS[i]`. Status 2 drops the
    /// addon from the client's Lua index, which hides Blizzard's addons from the AddOns list.
    AddonInfo {
        /// One status per record, in arrival order.
        statuses: Vec<u8>,
    },
    Other {
        opcode: u16,
    },
}

impl ServerPacket {
    /// A short name for logs and tallies; the opcode in hex for an unmodelled packet.
    pub fn name(&self) -> String {
        match self {
            ServerPacket::AuthChallenge { .. } => "SMSG_AUTH_CHALLENGE".into(),
            ServerPacket::AuthResponse { .. } => "SMSG_AUTH_RESPONSE".into(),
            ServerPacket::CharEnum { .. } => "SMSG_CHAR_ENUM".into(),
            ServerPacket::CharCreate { .. } => "SMSG_CHAR_CREATE".into(),
            ServerPacket::CharDelete { .. } => "SMSG_CHAR_DELETE".into(),
            ServerPacket::CharacterLoginFailed { .. } => "SMSG_CHARACTER_LOGIN_FAILED".into(),
            ServerPacket::UpdateObject { .. } => "SMSG_UPDATE_OBJECT".into(),
            ServerPacket::CompressedMoves { .. } => "SMSG_COMPRESSED_MOVES".into(),
            ServerPacket::DestroyObject { .. } => "SMSG_DESTROY_OBJECT".into(),
            ServerPacket::TriggerCinematic { .. } => "SMSG_TRIGGER_CINEMATIC".into(),
            ServerPacket::MoveTimeSkipped { .. } => "MSG_MOVE_TIME_SKIPPED".into(),
            ServerPacket::MonsterMove { transport, .. } => {
                if transport.is_some() {
                    "SMSG_MONSTER_MOVE_TRANSPORT".into()
                } else {
                    "SMSG_MONSTER_MOVE".into()
                }
            }
            ServerPacket::PlayerMove { opcode, .. } => format!("MSG_MOVE relay ({opcode:#06x})"),
            ServerPacket::Teleport { .. } => "MSG_MOVE_TELEPORT_ACK".into(),
            ServerPacket::NewWorld { .. } => "SMSG_NEW_WORLD".into(),
            ServerPacket::TransferPending { .. } => "SMSG_TRANSFER_PENDING".into(),
            ServerPacket::TransferAborted { .. } => "SMSG_TRANSFER_ABORTED".into(),
            ServerPacket::LoginVerifyWorld { .. } => "SMSG_LOGIN_VERIFY_WORLD".into(),
            ServerPacket::TimeSpeed { .. } => "SMSG_LOGIN_SETTIMESPEED".into(),
            ServerPacket::QueryTimeResponse { .. } => "SMSG_QUERY_TIME_RESPONSE".into(),
            ServerPacket::BindPoint { .. } => "SMSG_BINDPOINTUPDATE".into(),
            ServerPacket::GmTicketAnswer { .. } => "SMSG_GMTICKET_GETTICKET".into(),
            ServerPacket::GmTicketCreated { .. } => "SMSG_GMTICKET_CREATE".into(),
            ServerPacket::GmTicketUpdated { .. } => "SMSG_GMTICKET_UPDATETEXT".into(),
            ServerPacket::GmTicketDeleted { .. } => "SMSG_GMTICKET_DELETETICKET".into(),
            ServerPacket::GmTicketSystemStatus { .. } => "SMSG_GMTICKETSYSTEMSTATUS".into(),
            ServerPacket::GmTicketStatusUpdate { .. } => "SMSG_GM_TICKET_STATUS_UPDATE".into(),
            ServerPacket::BinderConfirm { .. } => "SMSG_BINDER_CONFIRM".into(),
            ServerPacket::PlayerBound { .. } => "SMSG_PLAYERBOUND".into(),
            ServerPacket::SummonRequest { .. } => "SMSG_SUMMON_REQUEST".into(),
            ServerPacket::TalentWipeConfirm { .. } => "MSG_TALENT_WIPE_CONFIRM".into(),
            ServerPacket::PetUnlearnConfirm { .. } => "SMSG_PET_UNLEARN_CONFIRM".into(),
            ServerPacket::RaidGroupOnly { .. } => "SMSG_RAID_GROUP_ONLY".into(),
            ServerPacket::AreaSpiritHealerTime { .. } => "SMSG_AREA_SPIRIT_HEALER_TIME".into(),
            ServerPacket::BattlefieldStatus(_) => "SMSG_BATTLEFIELD_STATUS".into(),
            ServerPacket::PvpLogData(_) => "MSG_PVP_LOG_DATA".into(),
            ServerPacket::BattlefieldList(_) => "SMSG_BATTLEFIELD_LIST".into(),
            ServerPacket::BattlefieldPositions(_) => "MSG_BATTLEGROUND_PLAYER_POSITIONS".into(),
            ServerPacket::TabardVendorActivate(_) => "MSG_TABARDVENDOR_ACTIVATE".into(),
            ServerPacket::SaveGuildEmblemResult(_) => "MSG_SAVE_GUILD_EMBLEM".into(),
            ServerPacket::GroupJoinedBattleground { .. } => "SMSG_GROUP_JOINED_BATTLEGROUND".into(),
            ServerPacket::BattlegroundPlayer { joined: true, .. } => {
                "SMSG_BATTLEGROUND_PLAYER_JOINED".into()
            }
            ServerPacket::BattlegroundPlayer { joined: false, .. } => {
                "SMSG_BATTLEGROUND_PLAYER_LEFT".into()
            }
            ServerPacket::MeetingStoneSetQueue { .. } => "SMSG_MEETINGSTONE_SETQUEUE".into(),
            ServerPacket::MeetingStoneNotice(crate::messages::MeetingStoneNotice::Success) => {
                "SMSG_MEETINGSTONE_SUCCESS".into()
            }
            ServerPacket::MeetingStoneNotice(crate::messages::MeetingStoneNotice::InProgress) => {
                "SMSG_MEETINGSTONE_IN_PROGRESS".into()
            }
            ServerPacket::MeetingStoneNotice(
                crate::messages::MeetingStoneNotice::MemberAdded { .. },
            ) => "SMSG_MEETINGSTONE_MEMBER_ADDED".into(),
            ServerPacket::MeetingStoneNotice(crate::messages::MeetingStoneNotice::JoinFailed {
                ..
            }) => "SMSG_MEETINGSTONE_JOIN_FAILED".into(),
            ServerPacket::TutorialFlags(_) => "SMSG_TUTORIAL_FLAGS".into(),
            ServerPacket::SetProficiency { .. } => "SMSG_SET_PROFICIENCY".into(),
            ServerPacket::InitializeFactions { .. } => "SMSG_INITIALIZE_FACTIONS".into(),
            ServerPacket::SetFactionStanding { .. } => "SMSG_SET_FACTION_STANDING".into(),
            ServerPacket::SetFactionVisible { .. } => "SMSG_SET_FACTION_VISIBLE".into(),
            ServerPacket::NameQueryResponse { .. } => "SMSG_NAME_QUERY_RESPONSE".into(),
            ServerPacket::CreatureQueryResponse { .. } => "SMSG_CREATURE_QUERY_RESPONSE".into(),
            ServerPacket::PetNameQueryResponse { .. } => "SMSG_PET_NAME_QUERY_RESPONSE".into(),
            ServerPacket::GameObjectQueryResponse { .. } => "SMSG_GAMEOBJECT_QUERY_RESPONSE".into(),
            ServerPacket::PageTextQueryResponse { .. } => "SMSG_PAGE_TEXT_QUERY_RESPONSE".into(),
            ServerPacket::GameObjectCustomAnim { .. } => "SMSG_GAMEOBJECT_CUSTOM_ANIM".into(),
            ServerPacket::GameObjectDespawnAnim { .. } => "SMSG_GAMEOBJECT_DESPAWN_ANIM".into(),
            ServerPacket::OpenContainer { .. } => "SMSG_OPEN_CONTAINER".into(),
            ServerPacket::Inspect { .. } => "SMSG_INSPECT".into(),
            ServerPacket::StandStateUpdate { .. } => "SMSG_STANDSTATE_UPDATE".into(),
            ServerPacket::FishNotHooked => "SMSG_FISH_NOT_HOOKED".into(),
            ServerPacket::FishEscaped => "SMSG_FISH_ESCAPED".into(),
            ServerPacket::PlaySound { .. } => "SMSG_PLAY_SOUND".into(),
            ServerPacket::PlayMusic { .. } => "SMSG_PLAY_MUSIC".into(),
            ServerPacket::PlayObjectSound { .. } => "SMSG_PLAY_OBJECT_SOUND".into(),
            ServerPacket::Weather { .. } => "SMSG_WEATHER".into(),
            ServerPacket::TextEmote { .. } => "SMSG_TEXT_EMOTE".into(),
            ServerPacket::Emote { .. } => "SMSG_EMOTE".into(),
            ServerPacket::ItemQueryResponse { .. } => "SMSG_ITEM_QUERY_SINGLE_RESPONSE".into(),
            ServerPacket::InventoryChangeFailure { .. } => "SMSG_INVENTORY_CHANGE_FAILURE".into(),
            ServerPacket::MessageChat(_) => "SMSG_MESSAGECHAT".into(),
            ServerPacket::ChannelNotify(_) => "SMSG_CHANNEL_NOTIFY".into(),
            ServerPacket::ChannelList { .. } => "SMSG_CHANNEL_LIST".into(),
            ServerPacket::ChatPlayerNotFound { .. } => "SMSG_CHAT_PLAYER_NOT_FOUND".into(),
            ServerPacket::ChatWrongFaction => "SMSG_CHAT_WRONG_FACTION".into(),
            ServerPacket::Notification { .. } => "SMSG_NOTIFICATION".into(),
            ServerPacket::AreaTriggerMessage { .. } => "SMSG_AREA_TRIGGER_MESSAGE".into(),
            ServerPacket::ServerMessage { .. } => "SMSG_SERVER_MESSAGE".into(),
            ServerPacket::ZoneUnderAttack { .. } => "SMSG_ZONE_UNDER_ATTACK".into(),
            ServerPacket::DefenseMessage { .. } => "SMSG_DEFENSE_MESSAGE".into(),
            ServerPacket::ChatRestricted => "SMSG_CHAT_RESTRICTED".into(),
            ServerPacket::PlayedTime { .. } => "SMSG_PLAYED_TIME".into(),
            ServerPacket::RandomRoll { .. } => "MSG_RANDOM_ROLL".into(),
            ServerPacket::InitialSpells { .. } => "SMSG_INITIAL_SPELLS".into(),
            ServerPacket::ActionButtons { .. } => "SMSG_ACTION_BUTTONS".into(),
            ServerPacket::LearnedSpell { .. } => "SMSG_LEARNED_SPELL".into(),
            ServerPacket::RemovedSpell { .. } => "SMSG_REMOVED_SPELL".into(),
            ServerPacket::SupercededSpell { .. } => "SMSG_SUPERCEDED_SPELL".into(),
            ServerPacket::CastResult { .. } => "SMSG_CAST_RESULT".into(),
            ServerPacket::PetSpells(_) => "SMSG_PET_SPELLS".into(),
            ServerPacket::PetMode(_) => "SMSG_PET_MODE".into(),
            ServerPacket::PetActionFeedback { .. } => "SMSG_PET_ACTION_FEEDBACK".into(),
            ServerPacket::PetTameFailure { .. } => "SMSG_PET_TAME_FAILURE".into(),
            ServerPacket::PetNameInvalid => "SMSG_PET_NAME_INVALID".into(),
            ServerPacket::PetBroken => "SMSG_PET_BROKEN".into(),
            ServerPacket::PetActionSound { .. } => "SMSG_PET_ACTION_SOUND".into(),
            ServerPacket::PetDismissSound { .. } => "SMSG_PET_DISMISS_SOUND".into(),
            ServerPacket::PetCastFailed { .. } => "SMSG_PET_CAST_FAILED".into(),
            ServerPacket::AttackStart { .. } => "SMSG_ATTACKSTART".into(),
            ServerPacket::AttackStop { .. } => "SMSG_ATTACKSTOP".into(),
            ServerPacket::AttackerState(_) => "SMSG_ATTACKERSTATEUPDATE".into(),
            // DEADTARGET and CANT_ATTACK share one arm, as in the client; the name says which arm.
            ServerPacket::AttackSwingError(e) => match e {
                AttackSwingError::NotInRange => "SMSG_ATTACKSWING_NOTINRANGE".into(),
                AttackSwingError::BadFacing => "SMSG_ATTACKSWING_BADFACING".into(),
                AttackSwingError::DeadOrUnattackable => {
                    "SMSG_ATTACKSWING_DEADTARGET/CANT_ATTACK".into()
                }
            },
            ServerPacket::CancelCombat => "SMSG_CANCEL_COMBAT".into(),
            ServerPacket::FeignDeathResisted => "SMSG_FEIGN_DEATH_RESISTED".into(),
            ServerPacket::AiReaction { .. } => "SMSG_AI_REACTION".into(),
            ServerPacket::SpellStart(_) => "SMSG_SPELL_START".into(),
            ServerPacket::SpellGo(_) => "SMSG_SPELL_GO".into(),
            ServerPacket::SpellChainTargets(_) => "SMSG_SPELL_UPDATE_CHAIN_TARGETS".into(),
            ServerPacket::SpellFailedOther { .. } => "SMSG_SPELL_FAILED_OTHER".into(),
            ServerPacket::SpellDelayed { .. } => "SMSG_SPELL_DELAYED".into(),
            ServerPacket::CancelAutoRepeat => "SMSG_CANCEL_AUTO_REPEAT".into(),
            ServerPacket::SpellCooldownList { .. } => "SMSG_SPELL_COOLDOWN".into(),
            ServerPacket::ItemCooldown { .. } => "SMSG_ITEM_COOLDOWN".into(),
            ServerPacket::ItemTime { .. } => "SMSG_ITEM_TIME_UPDATE".into(),
            ServerPacket::ItemEnchantTime { .. } => "SMSG_ITEM_ENCHANT_TIME_UPDATE".into(),
            ServerPacket::SpellModifier { flat, .. } => if *flat {
                "SMSG_SET_FLAT_SPELL_MODIFIER"
            } else {
                "SMSG_SET_PCT_SPELL_MODIFIER"
            }
            .into(),
            ServerPacket::CooldownEvent { .. } => "SMSG_COOLDOWN_EVENT".into(),
            ServerPacket::ClearCooldown { .. } => "SMSG_CLEAR_COOLDOWN".into(),
            ServerPacket::CooldownCheat { .. } => "SMSG_COOLDOWN_CHEAT".into(),
            ServerPacket::ChannelStart { .. } => "MSG_CHANNEL_START".into(),
            ServerPacket::ChannelUpdate { .. } => "MSG_CHANNEL_UPDATE".into(),
            ServerPacket::UpdateAuraDuration { .. } => "SMSG_UPDATE_AURA_DURATION".into(),
            ServerPacket::PlaySpellVisual { .. } => "SMSG_PLAY_SPELL_VISUAL".into(),
            ServerPacket::SpellDamageLog(_) => "SMSG_SPELLNONMELEEDAMAGELOG".into(),
            ServerPacket::PeriodicAuraLog(_) => "SMSG_PERIODICAURALOG".into(),
            ServerPacket::SpellHealLog(_) => "SMSG_SPELLHEALLOG".into(),
            ServerPacket::SpellEnergizeLog(_) => "SMSG_SPELLENERGIZELOG".into(),
            ServerPacket::DamageShield(_) => "SMSG_SPELLDAMAGESHIELD".into(),
            ServerPacket::EnvironmentalDamageLog(_) => "SMSG_ENVIRONMENTALDAMAGELOG".into(),
            ServerPacket::SpellLogMiss(_) => "SMSG_SPELLLOGMISS".into(),
            ServerPacket::PartyKillLog(_) => "SMSG_PARTYKILLLOG".into(),
            ServerPacket::SpellInstaKillLog(_) => "SMSG_SPELLINSTAKILLLOG".into(),
            ServerPacket::ProcResist(_) => "SMSG_PROCRESIST".into(),
            ServerPacket::SpellOrDamageImmune(_) => "SMSG_SPELLORDAMAGE_IMMUNE".into(),
            ServerPacket::SpellDispelLog(_) => "SMSG_SPELLDISPELLOG".into(),
            ServerPacket::DispelFailed(_) => "SMSG_DISPEL_FAILED".into(),
            ServerPacket::EnchantmentLog(_) => "SMSG_ENCHANTMENTLOG".into(),
            ServerPacket::SpellLogExecute(_) => "SMSG_SPELLLOGEXECUTE".into(),
            ServerPacket::XpGain(_) => "SMSG_LOG_XPGAIN".into(),
            ServerPacket::ExplorationXp(_) => "SMSG_EXPLORATION_EXPERIENCE".into(),
            ServerPacket::LevelUp(_) => "SMSG_LEVELUP_INFO".into(),
            ServerPacket::QuestGiverStatus { .. } => "SMSG_QUESTGIVER_STATUS".into(),
            ServerPacket::QuestGiverQuestList(_) => "SMSG_QUESTGIVER_QUEST_LIST".into(),
            ServerPacket::QuestGiverDetails(_) => "SMSG_QUESTGIVER_QUEST_DETAILS".into(),
            ServerPacket::QuestGiverRequestItems(_) => "SMSG_QUESTGIVER_REQUEST_ITEMS".into(),
            ServerPacket::QuestGiverOfferReward(_) => "SMSG_QUESTGIVER_OFFER_REWARD".into(),
            ServerPacket::QuestGiverComplete(_) => "SMSG_QUESTGIVER_QUEST_COMPLETE".into(),
            ServerPacket::QuestGiverInvalid { .. } => "SMSG_QUESTGIVER_QUEST_INVALID".into(),
            ServerPacket::QuestGiverFailed { .. } => "SMSG_QUESTGIVER_QUEST_FAILED".into(),
            ServerPacket::QuestQueryResponse(_) => "SMSG_QUEST_QUERY_RESPONSE".into(),
            ServerPacket::QuestLogFull => "SMSG_QUESTLOG_FULL".into(),
            ServerPacket::QuestPushResult(_) => "MSG_QUEST_PUSH_RESULT".into(),
            ServerPacket::QuestConfirmAccept(_) => "SMSG_QUEST_CONFIRM_ACCEPT".into(),
            ServerPacket::QuestUpdateComplete { .. } => "SMSG_QUESTUPDATE_COMPLETE".into(),
            ServerPacket::QuestUpdateFailed { .. } => "SMSG_QUESTUPDATE_FAILED".into(),
            ServerPacket::QuestUpdateFailedTimer { .. } => "SMSG_QUESTUPDATE_FAILEDTIMER".into(),
            ServerPacket::QuestUpdateAddKill { .. } => "SMSG_QUESTUPDATE_ADD_KILL".into(),
            ServerPacket::QuestUpdateAddItem { .. } => "SMSG_QUESTUPDATE_ADD_ITEM".into(),
            ServerPacket::GossipMessage { .. } => "SMSG_GOSSIP_MESSAGE".into(),
            ServerPacket::GossipComplete => "SMSG_GOSSIP_COMPLETE".into(),
            ServerPacket::GossipPoi(..) => "SMSG_GOSSIP_POI".into(),
            ServerPacket::NpcText { .. } => "SMSG_NPC_TEXT_UPDATE".into(),
            ServerPacket::VendorList { .. } => "SMSG_LIST_INVENTORY".into(),
            ServerPacket::BuyItem { .. } => "SMSG_BUY_ITEM".into(),
            ServerPacket::SellItemResult { .. } => "SMSG_SELL_ITEM".into(),
            ServerPacket::BuyFailed { .. } => "SMSG_BUY_FAILED".into(),
            ServerPacket::ShowBank { .. } => "SMSG_SHOW_BANK".into(),
            ServerPacket::BuyBankSlotResult { .. } => "SMSG_BUY_BANK_SLOT_RESULT".into(),
            ServerPacket::TrainerList { .. } => "SMSG_TRAINER_LIST".into(),
            ServerPacket::TrainerBuySucceeded { .. } => "SMSG_TRAINER_BUY_SUCCEEDED".into(),
            ServerPacket::TrainerBuyFailed { .. } => "SMSG_TRAINER_BUY_FAILED".into(),
            ServerPacket::ListStabledPets { .. } => "MSG_LIST_STABLED_PETS".into(),
            ServerPacket::StableResult { .. } => "SMSG_STABLE_RESULT".into(),
            ServerPacket::InvalidatePlayer { .. } => "SMSG_INVALIDATE_PLAYER".into(),
            ServerPacket::LootResponse { .. } => "SMSG_LOOT_RESPONSE".into(),
            ServerPacket::LootError { .. } => "SMSG_LOOT_RESPONSE (error)".into(),
            ServerPacket::LootReleaseResponse { .. } => "SMSG_LOOT_RELEASE_RESPONSE".into(),
            ServerPacket::LootRemoved { .. } => "SMSG_LOOT_REMOVED".into(),
            ServerPacket::LootMoneyNotify { .. } => "SMSG_LOOT_MONEY_NOTIFY".into(),
            ServerPacket::LootClearMoney => "SMSG_LOOT_CLEAR_MONEY".into(),
            ServerPacket::LootStartRoll(_) => "SMSG_LOOT_START_ROLL".into(),
            ServerPacket::LootRoll(_) => "SMSG_LOOT_ROLL".into(),
            ServerPacket::LootRollWon(_) => "SMSG_LOOT_ROLL_WON".into(),
            ServerPacket::LootAllPassed(_) => "SMSG_LOOT_ALL_PASSED".into(),
            ServerPacket::LootMasterList { .. } => "SMSG_LOOT_MASTER_LIST".into(),
            ServerPacket::ItemPushResult(_) => "SMSG_ITEM_PUSH_RESULT".into(),
            ServerPacket::CorpseQuery(_) => "MSG_CORPSE_QUERY".into(),
            ServerPacket::CorpseReclaimDelay { .. } => "SMSG_CORPSE_RECLAIM_DELAY".into(),
            ServerPacket::DurabilityDamageDeath => "SMSG_DURABILITY_DAMAGE_DEATH".into(),
            ServerPacket::ResurrectRequest(_) => "SMSG_RESURRECT_REQUEST".into(),
            ServerPacket::SpiritHealerConfirm { .. } => "SMSG_SPIRIT_HEALER_CONFIRM".into(),
            ServerPacket::MoveMode { mode, apply, .. } => match (mode, apply) {
                (MoveMode::Root, true) => "SMSG_FORCE_MOVE_ROOT".into(),
                (MoveMode::Root, false) => "SMSG_FORCE_MOVE_UNROOT".into(),
                (MoveMode::WaterWalk, true) => "SMSG_MOVE_WATER_WALK".into(),
                (MoveMode::WaterWalk, false) => "SMSG_MOVE_LAND_WALK".into(),
                (MoveMode::FeatherFall, true) => "SMSG_MOVE_FEATHER_FALL".into(),
                (MoveMode::FeatherFall, false) => "SMSG_MOVE_NORMAL_FALL".into(),
                (MoveMode::Hover, true) => "SMSG_MOVE_SET_HOVER".into(),
                (MoveMode::Hover, false) => "SMSG_MOVE_UNSET_HOVER".into(),
            },
            ServerPacket::SplineMoveMode { mode, apply, .. } => match (mode, apply) {
                (SplineMode::Root, true) => "SMSG_SPLINE_MOVE_ROOT".into(),
                (SplineMode::Root, false) => "SMSG_SPLINE_MOVE_UNROOT".into(),
                (SplineMode::WaterWalk, true) => "SMSG_SPLINE_MOVE_WATER_WALK".into(),
                (SplineMode::WaterWalk, false) => "SMSG_SPLINE_MOVE_LAND_WALK".into(),
                (SplineMode::FeatherFall, true) => "SMSG_SPLINE_MOVE_FEATHER_FALL".into(),
                (SplineMode::FeatherFall, false) => "SMSG_SPLINE_MOVE_NORMAL_FALL".into(),
                (SplineMode::Hover, true) => "SMSG_SPLINE_MOVE_SET_HOVER".into(),
                (SplineMode::Hover, false) => "SMSG_SPLINE_MOVE_UNSET_HOVER".into(),
                (SplineMode::Swimming, true) => "SMSG_SPLINE_MOVE_START_SWIM".into(),
                (SplineMode::Swimming, false) => "SMSG_SPLINE_MOVE_STOP_SWIM".into(),
                // Inverted on purpose: see `SplineMode::WalkMode`.
                (SplineMode::WalkMode, true) => "SMSG_SPLINE_MOVE_SET_WALK_MODE".into(),
                (SplineMode::WalkMode, false) => "SMSG_SPLINE_MOVE_SET_RUN_MODE".into(),
            },
            ServerPacket::KnockBack { .. } => "SMSG_MOVE_KNOCK_BACK".into(),
            ServerPacket::LogoutComplete => "SMSG_LOGOUT_COMPLETE".into(),
            ServerPacket::LogoutResponse { .. } => "SMSG_LOGOUT_RESPONSE".into(),
            ServerPacket::LogoutCancelAck => "SMSG_LOGOUT_CANCEL_ACK".into(),
            ServerPacket::Pong { .. } => "SMSG_PONG".into(),
            ServerPacket::ForceSpeedChange { kind, .. } => {
                format!("SMSG_FORCE_{kind:?}_SPEED_CHANGE")
            }
            ServerPacket::GroupInvite { .. } => "SMSG_GROUP_INVITE".into(),
            ServerPacket::GroupDecline { .. } => "SMSG_GROUP_DECLINE".into(),
            ServerPacket::GroupUninvited => "SMSG_GROUP_UNINVITE".into(),
            ServerPacket::GroupLeaderChanged { .. } => "SMSG_GROUP_SET_LEADER".into(),
            ServerPacket::GroupDestroyed => "SMSG_GROUP_DESTROYED".into(),
            ServerPacket::GroupList { .. } => "SMSG_GROUP_LIST".into(),
            ServerPacket::PartyCommandResult { .. } => "SMSG_PARTY_COMMAND_RESULT".into(),
            ServerPacket::PartyMemberStats { full: true, .. } => {
                "SMSG_PARTY_MEMBER_STATS_FULL".into()
            }
            ServerPacket::PartyMemberStats { full: false, .. } => "SMSG_PARTY_MEMBER_STATS".into(),
            ServerPacket::MinimapPing { .. } => "MSG_MINIMAP_PING".into(),
            ServerPacket::RaidTargetSet { .. } | ServerPacket::RaidTargetList { .. } => {
                "MSG_RAID_TARGET_UPDATE".into()
            }
            ServerPacket::ReadyCheckRequest | ServerPacket::ReadyCheckAnswer { .. } => {
                "MSG_RAID_READY_CHECK".into()
            }
            ServerPacket::RaidInstanceInfo { .. } => "SMSG_RAID_INSTANCE_INFO".into(),
            ServerPacket::DuelRequested { .. } => "SMSG_DUEL_REQUESTED".into(),
            ServerPacket::RaidInstanceMessage { .. } => "SMSG_RAID_INSTANCE_MESSAGE".into(),
            ServerPacket::InstanceSaveCreated { .. } => "SMSG_INSTANCE_SAVE_CREATED".into(),
            ServerPacket::InstanceReset { .. } => "SMSG_INSTANCE_RESET".into(),
            ServerPacket::InstanceResetFailed { .. } => "SMSG_INSTANCE_RESET_FAILED".into(),
            ServerPacket::UpdateLastInstance { .. } => "SMSG_UPDATE_LAST_INSTANCE".into(),
            ServerPacket::UpdateInstanceOwnership { .. } => "SMSG_UPDATE_INSTANCE_OWNERSHIP".into(),
            ServerPacket::DuelOutOfBounds => "SMSG_DUEL_OUTOFBOUNDS".into(),
            ServerPacket::DuelInBounds => "SMSG_DUEL_INBOUNDS".into(),
            ServerPacket::DuelComplete { .. } => "SMSG_DUEL_COMPLETE".into(),
            ServerPacket::DuelWinner { .. } => "SMSG_DUEL_WINNER".into(),
            ServerPacket::DuelCountdown { .. } => "SMSG_DUEL_COUNTDOWN".into(),
            ServerPacket::InspectHonorStats(..) => "MSG_INSPECT_HONOR_STATS".into(),
            ServerPacket::PvpCredit(..) => "SMSG_PVP_CREDIT".into(),
            ServerPacket::MirrorTimerStart(..) => "SMSG_START_MIRROR_TIMER".into(),
            ServerPacket::MirrorTimerPause { .. } => "SMSG_PAUSE_MIRROR_TIMER".into(),
            ServerPacket::MirrorTimerStop { .. } => "SMSG_STOP_MIRROR_TIMER".into(),
            ServerPacket::FriendList { .. } => "SMSG_FRIEND_LIST".into(),
            ServerPacket::IgnoreList { .. } => "SMSG_IGNORE_LIST".into(),
            ServerPacket::FriendStatus(..) => "SMSG_FRIEND_STATUS".into(),
            ServerPacket::WhoResults(..) => "SMSG_WHO".into(),
            ServerPacket::GuildQueryResponse(..) => "SMSG_GUILD_QUERY_RESPONSE".into(),
            ServerPacket::GuildRoster(..) => "SMSG_GUILD_ROSTER".into(),
            ServerPacket::GuildEvent(..) => "SMSG_GUILD_EVENT".into(),
            ServerPacket::GuildCommandResult(..) => "SMSG_GUILD_COMMAND_RESULT".into(),
            ServerPacket::GuildInvite { .. } => "SMSG_GUILD_INVITE".into(),
            ServerPacket::GuildDecline { .. } => "SMSG_GUILD_DECLINE".into(),
            ServerPacket::GuildInfo(..) => "SMSG_GUILD_INFO".into(),
            ServerPacket::PetitionShowList(..) => "SMSG_PETITION_SHOWLIST".into(),
            ServerPacket::PetitionShowSignatures(..) => "SMSG_PETITION_SHOW_SIGNATURES".into(),
            ServerPacket::PetitionSignResults(..) => "SMSG_PETITION_SIGN_RESULTS".into(),
            ServerPacket::PetitionQueryResponse(..) => "SMSG_PETITION_QUERY_RESPONSE".into(),
            ServerPacket::TurnInPetitionResults { .. } => "SMSG_TURN_IN_PETITION_RESULTS".into(),
            ServerPacket::PetitionDeclined { .. } => "MSG_PETITION_DECLINE".into(),
            ServerPacket::PetitionRenamed(..) => "MSG_PETITION_RENAME".into(),
            ServerPacket::SplineSpeedChange { kind, .. } => {
                format!("SMSG_SPLINE_SET_{kind:?}_SPEED")
            }
            ServerPacket::MoveSetSpeed { kind, .. } => format!("MSG_MOVE_SET_{kind:?}_SPEED"),
            ServerPacket::MountResult { mount: true, .. } => "SMSG_MOUNTRESULT".into(),
            ServerPacket::MountResult { mount: false, .. } => "SMSG_DISMOUNTRESULT".into(),
            ServerPacket::MountSpecialAnim { .. } => "SMSG_MOUNTSPECIAL_ANIM".into(),
            ServerPacket::ClientControlUpdate { .. } => "SMSG_CLIENT_CONTROL_UPDATE".into(),
            ServerPacket::ShowTaxiNodes { .. } => "SMSG_SHOWTAXINODES".into(),
            ServerPacket::TaxiNodeStatus { .. } => "SMSG_TAXINODE_STATUS".into(),
            ServerPacket::ActivateTaxiReply { .. } => "SMSG_ACTIVATETAXIREPLY".into(),
            ServerPacket::NewTaxiPath => "SMSG_NEW_TAXI_PATH".into(),
            ServerPacket::MailList { .. } => "SMSG_MAIL_LIST_RESULT".into(),
            ServerPacket::SendMailResult { .. } => "SMSG_SEND_MAIL_RESULT".into(),
            ServerPacket::ItemTextQueryResponse { .. } => "SMSG_ITEM_TEXT_QUERY_RESPONSE".into(),
            ServerPacket::ReceivedMail { .. } => "SMSG_RECEIVED_MAIL".into(),
            ServerPacket::NextMailTime { .. } => "MSG_QUERY_NEXT_MAIL_TIME".into(),
            ServerPacket::AuctionHello { .. } => "MSG_AUCTION_HELLO".into(),
            ServerPacket::AuctionCommandResult { .. } => "SMSG_AUCTION_COMMAND_RESULT".into(),
            ServerPacket::AuctionListResult { .. } => "SMSG_AUCTION_LIST_RESULT".into(),
            ServerPacket::AuctionOwnerListResult { .. } => "SMSG_AUCTION_OWNER_LIST_RESULT".into(),
            ServerPacket::AuctionBidderListResult { .. } => {
                "SMSG_AUCTION_BIDDER_LIST_RESULT".into()
            }
            ServerPacket::AuctionBidderNotification(_) => "SMSG_AUCTION_BIDDER_NOTIFICATION".into(),
            ServerPacket::AuctionOwnerNotification(_) => "SMSG_AUCTION_OWNER_NOTIFICATION".into(),
            ServerPacket::AuctionRemovedNotification { .. } => {
                "SMSG_AUCTION_REMOVED_NOTIFICATION".into()
            }
            ServerPacket::TradeStatus { .. } => "SMSG_TRADE_STATUS".into(),
            ServerPacket::TradeStatusExtended { .. } => "SMSG_TRADE_STATUS_EXTENDED".into(),
            ServerPacket::InitWorldStates(_) => "SMSG_INIT_WORLD_STATES".into(),
            ServerPacket::UpdateWorldState { .. } => "SMSG_UPDATE_WORLD_STATE".into(),
            ServerPacket::AddonInfo { .. } => "SMSG_ADDON_INFO".into(),
            ServerPacket::Other { opcode } => format!("opcode {opcode:#06x}"),
        }
    }
}
