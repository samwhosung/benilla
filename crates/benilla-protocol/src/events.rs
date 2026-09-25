//! Decoded session events; coordinates stay raw WoW, which the app maps to `bevy = (-y, z, -x)`.

use crate::messages::{
    ActionButton, AttackSwingError, AttackerState, AuctionBidderNotification, AuctionCommandTail,
    AuctionListEntry, AuctionOwnerNotification, ChannelNoticeTail, Character, CreateSpline,
    DamageShield, DispelFailed, EnchantmentLog, EnvironmentalDamageLog, ExplorationXp, FriendEntry,
    FriendStatusUpdate, GmTicket, GossipOption, GroupLootInfo, GroupMemberEntry,
    GuildCommandResult, GuildEventNotice, GuildInfo, GuildQueryResponse, GuildRoster,
    InspectHonorStats, ItemInfo, ItemPushResult, JumpInfo, LevelUpInfo, LootAllPassed, LootItem,
    LootRoll, LootRollWon, LootStartRoll, MailListEntry, MirrorTimerStart, MonsterMoveFacing,
    MoverState, ObjectFields, PartyKillLog, PartyMemberStatsInfo, PeriodicAuraLog, PetMode,
    PetSpells, PetitionQueryResponse, PetitionRename, PetitionShowList, PetitionShowSignatures,
    PetitionSignResults, PvpCredit, QuestComplete, QuestConfirmAccept, QuestDetails,
    QuestGiverList, QuestOfferReward, QuestRequestItems, QuestShareMsg, QuestTemplate,
    SpellDamageLog, SpellDispelLog, SpellEnergizeLog, SpellHealLog, SpellInstaKillLog,
    SpellLogExecute, SpellLogMiss, SpellOutcomeLog, StabledPet, TaxiMask, TradeStatus,
    TradeStatusExtended, TrainerSpell, TransportPose, VendorItem, WhoResults, XpGain,
};

/// Coarse entity classification, free of wire types, carried on [`SessionEvent::ObjectCreate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityKind {
    Player,
    Unit,
    GameObject,
    /// `TYPEID_DYNAMICOBJECT` (6): an area effect's anchor, drawn via `DYNAMICOBJECT_SPELLID`.
    DynamicObject,
    /// `TYPEID_CORPSE` (7): a dead player's body or bone pile, drawn as the player from its
    /// `CORPSE_FIELD_BYTES_*` and `CORPSE_FIELD_ITEM`.
    Corpse,
    Other,
}

/// Which character verb a [`SessionEvent::CharActionResult`] answers, and so which code family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharAction {
    Create,
    Delete,
}

/// A unit's six movement speeds from its `LIVING` block: yd/s, except `turn_rate` in rad/s.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MoveSpeeds {
    pub walk: f32,
    pub run: f32,
    pub run_back: f32,
    pub swim: f32,
    pub swim_back: f32,
    pub turn_rate: f32,
}

impl MoveSpeeds {
    fn from_wire(s: [f32; 6]) -> Self {
        Self {
            walk: s[0],
            run: s[1],
            run_back: s[2],
            swim: s[3],
            swim_back: s[4],
            turn_rate: s[5],
        }
    }
}

/// A login attempt's pre-roster stage; the connecting dialog shows its `LOGIN_STATE_*` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginStage {
    /// Dialing the auth server (`LOGIN_STATE_CONNECTING`).
    Connecting,
    /// SRP6 challenge/proof in flight (`LOGIN_STATE_AUTHENTICATING`).
    Authenticating,
    /// Realm picked; the world-socket handshake and char enum (`LOGIN_STATE_HANDSHAKING`).
    Handshaking,
}

/// How a world session ended. On a loss the reference fires `DISCONNECTED_FROM_SERVER` and
/// `GlueParent.lua` returns to the login screen; a logout returns to character select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// A logout the server confirmed (`SMSG_LOGOUT_COMPLETE`); a fresh roster follows.
    LoggedOut,
    /// The stream died: a socket error, an EOF or a post-roster handshake failure. A displacement
    /// kick is this too: vmangos `WorldSession::KickPlayer` only closes the socket, no packet.
    Lost,
}

/// Which server refused the login, and its result byte. The two enums overlap: 0x0C is
/// `AUTH_LOGON_FAILED_SUSPENDED` to realmd but `AUTH_OK` to the world server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginRefusal {
    /// realmd's logon-proof `AuthLogonResult` (`crate::AuthReject`).
    Logon(u8),
    /// The world server's `SMSG_AUTH_RESPONSE` code (`crate::WorldAuthReject`).
    World(u8),
}

impl LoginRefusal {
    /// The raw byte, for logging only: without its server it is ambiguous.
    pub fn byte(self) -> u8 {
        match self {
            LoginRefusal::Logon(b) | LoginRefusal::World(b) => b,
        }
    }
}

/// One decoded world-stream event; its fieldless twin [`SessionEventKind`] keys the handler table.
#[derive(Debug, Clone, strum::EnumDiscriminants)]
#[strum_discriminants(
    name(SessionEventKind),
    derive(Hash, PartialOrd, Ord, strum::EnumIter, strum::IntoStaticStr)
)]
pub enum SessionEvent {
    /// A login attempt reached `stage`; emitted by the IO thread, never wire-decoded.
    LoginStage { stage: LoginStage },
    /// The account's realms (`CMD_REALM_LIST`); the IO thread blocks for the app's pick. Re-sent on
    /// each refresh (the reference re-requests every 5 s) and on a realm change.
    RealmList { realms: Vec<crate::RealmInfo> },
    /// Queued for a full realm (`AUTH_WAIT_QUEUE`), per queue packet: a wait, not an outcome.
    LoginQueued {
        position: Option<u32>,
        realm: Option<String>,
    },
    /// A login failed before the roster; `refusal` is `None` for a transport failure. A `terminal`
    /// one ([`crate::WardenRequired`]) no retry can fix: show `reason` and stop.
    LoginFailed {
        refusal: Option<LoginRefusal>,
        reason: String,
        terminal: bool,
        /// `Some` only when the socket never opened: which dial failure, and the address.
        dial: Option<crate::DialFailure>,
    },
    /// The roster (`SMSG_CHAR_ENUM`); the IO thread then blocks for the app's pick before
    /// `CMSG_PLAYER_LOGIN`. `realm` is `None` on the raw [`crate::decode`] path, which has no auth.
    CharacterList {
        characters: Vec<Character>,
        realm: Option<crate::RealmInfo>,
    },
    /// `SMSG_CHAR_CREATE`/`_DELETE`'s raw `WorldResult`; on success a fresh roster comes first.
    CharActionResult { action: CharAction, code: u8 },
    /// We are in the world; the IO thread emits this before any object update.
    Connected {
        self_guid: u64,
        name: String,
        /// Rested billing minutes for `GetBillingTimeRested()`, sent once, at auth.
        billing_time_rested: u32,
        /// `SMSG_TUTORIAL_FLAGS` if it came during the handshake; `None` if it comes in-world.
        tutorial_flags: Option<Vec<u8>>,
        /// The addons `SMSG_ADDON_INFO` hid. `None` is no reply at all, where the reference's
        /// `GetNumAddOns()` answers 0; an empty list is a reply that hid nothing.
        addon_info: Option<Vec<String>>,
    },
    /// Our pick was refused (`SMSG_CHARACTER_LOGIN_FAILED`) after [`Self::Connected`] was sent;
    /// `result` is a 1-based index into the reference's six `CHAR_LOGIN_*` strings.
    CharacterLoginFailed { result: u8 },
    /// Logout confirmed (`SMSG_LOGOUT_COMPLETE`); a fresh [`Self::CharacterList`] follows.
    LoggedOut,
    /// `SMSG_LOGOUT_RESPONSE`. A non-zero `reason` refuses: 1 in combat, 2 GM-frozen, 3 falling
    /// (vmangos). Zero starts the 20 s camp timer, or with `instant` (resting, taxi, GM) logs out.
    LogoutResponse { reason: u32, instant: bool },
    /// The server dropped a pending logout at our `CMSG_LOGOUT_CANCEL` (`SMSG_LOGOUT_CANCEL_ACK`).
    LogoutCancelled,
    /// The session ended; `reason` is human-readable.
    Disconnected { reason: String, end: SessionEnd },
    /// A placed object entered view, its spawn identity decoded per object type.
    ObjectCreate {
        guid: u64,
        kind: EntityKind,
        /// The type's own display-id field; `None` when absent or zero.
        display_id: Option<u32>,
        position: [f32; 3],
        orientation: f32,
        /// `OBJECT_FIELD_SCALE_X`, the complete render scale; `1.0` when absent.
        scale: f32,
        /// From the `LIVING` block, so not in `fields`; `None` for a GameObject.
        speeds: Option<MoveSpeeds>,
        /// Flags and swim pitch the unit streams in with, applied by the reference before its model
        /// exists: the flags via the relay's `0x75a07dff` merge, the pitch via `0x7c6420`.
        mover: Option<MoverState>,
        /// `UPDATE_FLAG_TRANSPORT`'s path progress in ms, only on a type-11/15 transport create;
        /// progress is this plus elapsed ms; position is `timetable(progress % period)`.
        transport_progress: Option<u32>,
        /// Its rider pose, local to the transport, when created with `MOVEFLAG_ON_TRANSPORT`.
        transport: Option<TransportPose>,
        /// The path it was already walking (`MOVEFLAG_SPLINE_ENABLED`), with the ms already ridden.
        spline: Option<CreateSpline>,
        /// The full descriptor from the create mask; later [`Self::ObjectValues`] merge into it.
        fields: ObjectFields,
    },
    /// An item or container: a pose-less descriptor (vmangos sends `UPDATEFLAG_ALL` and a
    /// constant); its slot is in the owner's `PLAYER_FIELD_*_SLOT` or `CONTAINER_FIELD_SLOT`.
    ItemCreate {
        guid: u64,
        container: bool,
        /// The full descriptor; `OBJECT_FIELD_ENTRY` is the template for `CMSG_ITEM_QUERY_SINGLE`.
        fields: ObjectFields,
    },
    /// An update-object movement block: a one-off authoritative pose that supersedes any path.
    ObjectMove {
        guid: u64,
        position: [f32; 3],
        orientation: f32,
    },
    /// A relayed player move (`MSG_MOVE_*`, about 2 Hz): `fall_time` is ms airborne, `jump` rides
    /// with `JUMPING`, `transport` is deck-local with `MOVEFLAG_ON_TRANSPORT`.
    UnitMove {
        guid: u64,
        position: [f32; 3],
        orientation: f32,
        flags: u32,
        /// Swim pitch in radians, up positive, `0.0` unless `MOVEFLAG_SWIMMING`; the reference's
        /// swim velocity basis includes it (`0x7c5880`).
        pitch: f32,
        /// vmangos's ms clock stamped at receipt (`MovementInfo::Read`), one clock for all movers;
        /// the reference paces a remote's apply by the deltas between consecutive stamps.
        time: u32,
        /// What the opcode adds to the pose: a heartbeat, a teleport, a root, or nothing.
        verb: crate::messages::RelayVerb,
        fall_time: u32,
        jump: Option<JumpInfo>,
        transport: Option<TransportPose>,
    },
    /// A descriptor `Values` delta, only the changed fields; never empty (the codec drops those).
    ObjectValues { guid: u64, fields: ObjectFields },
    /// Left view range (`OutOfRange`); the reference keeps them in a staging table for re-create.
    ObjectsRemoved(Vec<u64>),
    /// `SMSG_DESTROY_OBJECT`: the reference frees it at once, no fade; a respawn is a fresh create.
    ObjectDestroyed(u64),
    /// `SMSG_TRIGGER_CINEMATIC`. Until `CMSG_COMPLETE_CINEMATIC` vmangos anchors visibility to the
    /// cinematic camera (`Player::UpdateCinematic`), despawning the world around the body.
    CinematicTriggered { cinematic_id: u32 },
    /// `MSG_MOVE_TIME_SKIPPED`: not a pose. The mover's relay stamp advances by `lag_ms`, as the
    /// reference's `[CMovement+0xac] += lag` (`0x603b40` → `0x601560` → `0x61ab90`), or its next
    /// packet is scheduled late.
    MoveTimeSkipped { guid: u64, lag_ms: u32 },
    /// A server path: `path` is the travel-order polyline from `start`, walked at constant speed
    /// over `duration_ms`; a stop, a zero duration or fewer than two points leaves it empty.
    MonsterMove {
        guid: u64,
        /// `SMSG_MONSTER_MOVE_TRANSPORT`: `start`/`path` are deck-local, and the unit now rides it.
        transport: Option<u64>,
        start: [f32; 3],
        /// Echoed in `CMSG_MOVE_SPLINE_DONE` when the spline drives our own player.
        spline_id: u32,
        path: Vec<[f32; 3]>,
        /// The final facing (`moveType` 2/3/4), snapped; how a creature turns without walking.
        facing: MonsterMoveFacing,
        stop: bool,
        duration_ms: u32,
        /// Ground walks take Z from the terrain; flights and `transport` paths keep the wire Z.
        flying: bool,
        /// `SPLINEFLAG_RUNMODE`; its absence forces `MOVEFLAG_WALK_MODE` on the moved unit.
        run_mode: bool,
    },
    /// Same-map teleport (`MSG_MOVE_TELEPORT_ACK`): the app snaps our player and echoes the ack.
    Teleport {
        guid: u64,
        counter: u32,
        position: [f32; 3],
        orientation: f32,
    },
    /// `SMSG_NEW_WORLD` (acked) or the login `SMSG_LOGIN_VERIFY_WORLD`. After a transfer that
    /// named a transport the pose is boat-local (vmangos `SendNewWorld` sends `GetTransportPos()`).
    Worldport {
        map_id: u32,
        position: [f32; 3],
        orientation: f32,
        needs_ack: bool,
    },
    /// `SMSG_TRANSFER_PENDING`: a worldport follows, boat-local when `transport_entry` is set.
    TransferPending {
        map_id: u32,
        transport_entry: Option<u32>,
    },
    /// `SMSG_TRANSFER_ABORTED`: the announced transfer is off (map full, no instance).
    TransferAborted { reason: u8 },
    /// The server in-game clock (`SMSG_LOGIN_SETTIMESPEED`): drives time-of-day lighting.
    TimeSpeed {
        hours: u8,
        minutes: u8,
        /// `year·372 + month·31 + day` of the packed server date; the moon phase's input.
        day_serial: u32,
        timescale: f32,
    },
    /// The server's wall clock in unix seconds (`SMSG_QUERY_TIME_RESPONSE`), the base of absolute
    /// descriptor stamps: a timed quest's deadline is `time(nullptr) + limitTime`.
    ServerUnixTime { unix_time: u32 },
    /// The hearthstone bind (`SMSG_BINDPOINTUPDATE`): the AreaTable id the `$z` token names.
    BindPoint { area: u32 },
    /// `SMSG_GMTICKET_GETTICKET`, `None` for no ticket. The Help window re-polls every 10 minutes
    /// and fires `UPDATE_TICKET` on each answer, changed or not: count these, never diff them.
    GmTicket { ticket: Option<Box<GmTicket>> },
    /// `SMSG_GMTICKET_CREATE`: 2 created, 3 refused, 1 already exists (vmangos never sends 1, and
    /// sometimes sends nothing at all).
    GmTicketCreated { response: u32 },
    /// The answer to editing a ticket (`SMSG_GMTICKET_UPDATETEXT`): 4 = saved, 5 = refused.
    GmTicketUpdated { response: u32 },
    /// `SMSG_GMTICKET_DELETETICKET`: 9 = deleted; also unasked on a GM's `.ticket delete`.
    GmTicketDeleted { response: u32 },
    /// `SMSG_GMTICKETSYSTEMSTATUS`: 1 = the queue takes tickets; drives `UPDATE_GM_STATUS`.
    GmTicketSystemStatus { status: i32 },
    /// `SMSG_GM_TICKET_STATUS_UPDATE`: 1 updated, 2 closed, 3 survey; vmangos never sends it.
    GmTicketStatusUpdate { status: u32 },
    /// `SMSG_BINDER_CONFIRM`, the `CONFIRM_BINDER` dialog; echo `binder` in `CMSG_BINDER_ACTIVATE`.
    BinderConfirm { binder: u64 },
    /// `SMSG_SUMMON_REQUEST`: `zone` is the summoner's AreaTable id, `delay_ms` the auto-decline
    /// hold. Accepting sends `CMSG_SUMMON_RESPONSE`; there is no decline opcode.
    SummonRequest {
        summoner: u64,
        zone: u32,
        delay_ms: u32,
    },
    /// `MSG_TALENT_WIPE_CONFIRM`, `cost` in copper; accepting echoes it, declining sends nothing.
    TalentWipeConfirm { trainer: u64, cost: u32 },
    /// `SMSG_PET_UNLEARN_CONFIRM`: accept with `CMSG_PET_UNLEARN`; declining sends nothing.
    PetUnlearnConfirm { trainer: u64, cost: u32 },
    /// The instance-boot clock (`SMSG_RAID_GROUP_ONLY`): a positive delay arms it
    /// (`INSTANCE_BOOT_START`), zero clears it (`INSTANCE_BOOT_STOP`) and shows `reason` 1 or 2.
    RaidGroupOnly { delay_ms: u32, reason: u32 },
    /// A spirit healer's next resurrection wave (`SMSG_AREA_SPIRIT_HEALER_TIME`): arms
    /// `GetAreaSpiritHealerTime` and fires `AREA_SPIRIT_HEALER_IN_RANGE`.
    AreaSpiritHealerTime { healer: u64, ms: u32 },
    /// One of the client's three battleground queue slots (`SMSG_BATTLEFIELD_STATUS`).
    BattlefieldStatus(crate::messages::BattlefieldStatus),
    /// The battleground scoreboard (`MSG_PVP_LOG_DATA`), rows in wire order, unsorted.
    PvpLogData(crate::messages::PvpLogData),
    /// The battleground instance list (`SMSG_BATTLEFIELD_LIST`); fires `BATTLEFIELDS_SHOW`.
    BattlefieldList(crate::messages::BattlefieldList),
    /// Teammate and flag-carrier positions (`MSG_BATTLEGROUND_PLAYER_POSITIONS`), raw WoW.
    BattlefieldPositions(crate::messages::BattlefieldPositions),
    /// `MSG_TABARDVENDOR_ACTIVATE`: the vendor whose tabard designer opens.
    TabardVendorActivate(u64),
    /// `MSG_SAVE_GUILD_EMBLEM`: the save's result row.
    SaveGuildEmblemResult(u32),
    /// `SMSG_GROUP_JOINED_BATTLEGROUND`: `0xFFFFFFFE` deserters, a map id joined, else the
    /// generic failure; a message line each, no state.
    GroupJoinedBattleground { result: u32 },
    /// `SMSG_BATTLEGROUND_PLAYER_JOINED`/`_LEFT`, printed once the name cache has the guid.
    BattlegroundPlayer { guid: u64, joined: bool },
    /// The meeting-stone queue state (`SMSG 0x295`): the area queued for and a status byte.
    MeetingStoneSetQueue { area: u32, status: u8 },
    /// One of the meeting stone's four chat-line-only replies (`0x297/0x298/0x299/0x2BB`).
    MeetingStoneNotice(crate::messages::MeetingStoneNotice),
    /// The tutorial bank bytes (`SMSG_TUTORIAL_FLAGS`); the client copies both its banks from them.
    TutorialFlags(Vec<u8>),
    /// The bind took (`SMSG_PLAYERBOUND`); `area` matches the [`Self::BindPoint`] sent beside it.
    PlayerBound { binder: u64, area: u32 },
    /// One item class's subclass mask (`SMSG_SET_PROFICIENCY`), the reference's `0xc4d4a0[class]`.
    Proficiency { item_class: u32, subclass_mask: u32 },
    /// `SMSG_INITIALIZE_FACTIONS` at login: `(flags, standing)` per `Faction.dbc`
    /// `reputationIndex`. The standing excludes the DBC race/class base; add it before ranking.
    Reputations { standings: Vec<(u8, i32)> },
    /// `SMSG_SET_FACTION_STANDING`: `(reputationListId, standing)` per slot, base excluded.
    ReputationDelta { standings: Vec<(u32, i32)> },
    /// `SMSG_SET_FACTION_VISIBLE`: sets the slot's visible flag, which decides if it is listed.
    ReputationVisible { list_id: u32 },
    /// `SMSG_NAME_QUERY_RESPONSE`: names are never descriptor fields in 1.12, only query answers.
    /// An unknown guid answers with an empty `name`.
    PlayerName {
        guid: u64,
        name: String,
        race: u32,
        gender: u32,
        class: u32,
    },
    /// `SMSG_PET_NAME_QUERY_RESPONSE`, keyed by the pet number its guid carries in place of an
    /// entry ([`crate::guid::pet_number`]). There is no miss reply: the server stays silent.
    PetName { pet_number: u32, name: String },
    /// `SMSG_CREATURE_QUERY_RESPONSE`, keyed by template `entry`. An unknown entry has a `None`
    /// `name`, and every other field then reads `None`, `0` or `false`.
    CreatureName {
        entry: u32,
        name: Option<String>,
        subname: Option<String>,
        /// `CreatureType.dbc` id; the TAB-target critter/totem filter reads it.
        creature_type: Option<u32>,
        /// `CreatureFamily.dbc` id for `UnitCreatureFamily` and the diet tooltip; `0` is none.
        pet_family: u32,
        /// Elite rank 0..4, the unit tooltip's rank word.
        rank: u32,
        /// Bit `0x10` hides the tooltip's faction-name line.
        type_flags: u32,
        /// `CreatureDisplayInfo.dbc` id, to draw a creature with no world object (a stabled pet).
        display_id: u32,
        /// The tooltip's green CIVILIAN line.
        civilian: bool,
        /// The tooltip's white LEADER line.
        racial_leader: bool,
    },
    /// `SMSG_GAMEOBJECT_QUERY_RESPONSE`, keyed by template `entry`; `data` is the type-specific
    /// raw tail. An unknown entry still fires, every field zero or empty.
    GameObjectInfo {
        entry: u32,
        type_id: u32,
        display_id: u32,
        name: String,
        data: [i32; 24],
    },
    /// `SMSG_GAMEOBJECT_CUSTOM_ANIM`: the reference arms substate `8 + anim_id`, AnimationData
    /// 153..156 (Custom0..3), and rejects `anim_id >= 4`. A fishing bobber's bite sends 0.
    GameObjectCustomAnim { guid: u64, anim_id: u32 },
    /// `SMSG_GAMEOBJECT_DESPAWN_ANIM`: the reference arms substate 12 (AnimationData 157 Despawn)
    /// and keeps the object alive through the same-tick `SMSG_DESTROY_OBJECT` while it plays.
    GameObjectDespawnAnim { guid: u64 },
    /// `SMSG_OPEN_CONTAINER`: the reference fires `BAG_OPEN` with 0 for our own guid (the
    /// backpack), 1..10 for a bag-cache slot, and nothing for a miss.
    OpenContainer { item: u64 },
    /// Our own stand state (`SMSG_STANDSTATE_UPDATE`), applied ungated through the setter a
    /// volunteered change uses (reference: `0x603e50` → `0x6127b0`).
    StandStateUpdate { state: u8 },
    /// Nothing hooked (`SMSG_FISH_NOT_HOOKED`, empty body): the red `ERR_FISH_NOT_HOOKED` line.
    FishNotHooked,
    /// The fish got away on the bobber click (`SMSG_FISH_ESCAPED`, empty body): `ERR_FISH_ESCAPED`.
    FishEscaped,
    /// A server-pushed 2D sound kit (`SMSG_PLAY_SOUND`): BG events, quest/zone scripts.
    PlaySound { sound_id: u32 },
    /// A server-pushed music kit for the music channel (`SMSG_PLAY_MUSIC`).
    PlayMusic { music_id: u32 },
    /// A server-pushed 3D sound kit at object `guid` (`SMSG_PLAY_OBJECT_SOUND`).
    PlayObjectSound { sound_id: u32, guid: u64 },
    /// The zone's weather (`SMSG_WEATHER`); `sound_id` is a SoundEntries loop, 8533..8558, 0 clear.
    Weather {
        weather_type: u32,
        grade: f32,
        sound_id: u32,
        instant: bool,
    },
    /// A chat emote (`SMSG_TEXT_EMOTE`, `EmotesText.dbc`); empty `target_name` means untargeted.
    TextEmote {
        guid: u64,
        text_emote: u32,
        target_name: String,
    },
    /// A unit's anim emote (`SMSG_EMOTE`): an `Emotes.dbc` id, its anim and its `EventSoundID`.
    Emote { guid: u64, emote_id: u32 },
    /// The spell book at login (`SMSG_INITIAL_SPELLS`), ids widened from `u16`, and live cooldowns.
    SpellBook {
        spell_ids: Vec<u32>,
        cooldowns: Vec<crate::messages::SpellCooldown>,
    },
    /// The saved action bar at login (`SMSG_ACTION_BUTTONS`): slots 0..119, 0..11 the main bar.
    ActionButtons { buttons: Vec<ActionButton> },
    /// A spell learned after login (`SMSG_LEARNED_SPELL`), widened from the wire `u16`.
    SpellLearned { spell_id: u32 },
    /// A spell unlearned (`SMSG_REMOVED_SPELL`), widened from `u16`; a respec sends one per rank.
    SpellRemoved { spell_id: u32 },
    /// A rank-up (`SMSG_SUPERCEDED_SPELL`) in book and bar; both ids widened from `u16`.
    SpellSuperceded {
        old_spell_id: u32,
        new_spell_id: u32,
    },
    /// Our cast's verdict (`SMSG_CAST_RESULT`); `arg` fills its `%s`: a `SpellFocusObject.dbc` id
    /// for `REQUIRES_SPELL_FOCUS`, an `AreaTable.dbc` id for `REQUIRES_AREA`.
    CastResult {
        spell_id: u32,
        success: bool,
        reason: Option<u8>,
        arg: Option<u32>,
    },
    /// The whole pet bar (`SMSG_PET_SPELLS`), never a delta; a zero `pet_guid` tears it down.
    PetSpells(Box<PetSpells>),
    /// The pet's react/command state alone (`SMSG_PET_MODE`), with no bar edit.
    PetMode(PetMode),
    /// A refused pet order (`SMSG_PET_ACTION_FEEDBACK`): one reason code for the red error line.
    PetActionFeedback { reason: u8 },
    /// The pet's cast refusal (`SMSG_PET_CAST_FAILED`); it never touches our own cast state.
    PetCastFailed { spell_id: u32, reason: Option<u8> },
    /// A refused tame, Call Pet or Revive Pet (`SMSG_PET_TAME_FAILURE`): the reason's
    /// `PETTAME_*` string fills `ERR_TAME_FAILED`.
    PetTameFailure { reason: u8 },
    /// A refused pet rename (`SMSG_PET_NAME_INVALID`, empty body): `ERR_INVALID_PETNAME`.
    PetNameInvalid,
    /// The pet ran away at zero loyalty (`SMSG_PET_BROKEN`, empty body): `ERR_PET_BROKEN`.
    PetBroken,
    /// The pet's voice (`SMSG_PET_ACTION_SOUND`): a talk selector into its `CreatureSoundData` row.
    PetActionSound { pet_guid: u64, talk: u32 },
    /// A dismissed pet's sound (`SMSG_PET_DISMISS_SOUND`): the `CreatureModelData` column-29 kit.
    PetDismissSound { model_id: u32, position: [f32; 3] },
    /// `SMSG_ITEM_QUERY_SINGLE_RESPONSE`, by entry; `None` is unknown. Boxed, being wide.
    ItemTemplate {
        entry: u32,
        info: Option<Box<ItemInfo>>,
    },
    /// One chat line (`SMSG_MESSAGECHAT`); system lines (type `0x0A`) carry GM command feedback.
    Chat(crate::messages::ChatMessage),
    /// A channel notice (`SMSG_CHANNEL_NOTIFY`); `notice` selects the `tail` payload.
    ChannelNotify {
        notice: u8,
        channel: String,
        tail: ChannelNoticeTail,
    },
    /// A channel roster (`SMSG_CHANNEL_LIST`), member flags per vmangos `Chat/Channel.h:119-130`.
    ChannelList {
        channel: String,
        flags: u8,
        members: Vec<(u64, u8)>,
    },
    /// A whisper target wasn't found online (`SMSG_CHAT_PLAYER_NOT_FOUND`).
    ChatPlayerNotFound { name: String },
    /// A cross-faction whisper was refused (`SMSG_CHAT_WRONG_FACTION`); empty body.
    ChatWrongFaction,
    /// A server notice (`SMSG_NOTIFICATION`) the reference flashes in UIErrorsFrame.
    Notification { text: String },
    /// Why an area trigger refused (`SMSG_AREA_TRIGGER_MESSAGE`), shown as a notification.
    AreaTriggerMessage { text: String },
    /// A shutdown countdown or a broadcast (`SMSG_SERVER_MESSAGE`): a `ServerMessages.dbc` row
    /// and its `%s`, shown as `CHAT_MSG_SYSTEM`.
    ServerMessage { message_type: u32, text: String },
    /// `SMSG_ZONE_UNDER_ATTACK`: an `AreaTable.dbc` id, shown on the joined defense channels.
    ZoneUnderAttack { area_id: u32 },
    /// A defense broadcast (`SMSG_DEFENSE_MESSAGE`), shown where [`Self::ZoneUnderAttack`] is.
    DefenseMessage { zone_id: u32, text: String },
    /// A trial account's whisper cap (`SMSG_CHAT_RESTRICTED`, empty body): `DisplayError(0x1c3)`.
    ChatRestricted,
    /// `/played` (`SMSG_PLAYED_TIME`): total and this-level played time, in seconds.
    PlayedTime { total: u32, level: u32 },
    /// A `/random` broadcast (`MSG_RANDOM_ROLL`).
    RandomRoll {
        min: u32,
        max: u32,
        roll: u32,
        guid: u64,
    },
    /// A refused inventory operation (`SMSG_INVENTORY_CHANGE_FAILURE`), never `EQUIP_ERR_OK`.
    InventoryFailure {
        reason: u8,
        required_level: Option<u32>,
        item_guid: u64,
        /// The destination bag's absolute player slot, reason 16's `%s`; 255 is the player's own.
        bag_slot: u8,
    },
    /// A unit began melee auto-attack (`SMSG_ATTACKSTART`), our own included.
    AttackStart { attacker: u64, victim: u64 },
    /// A unit stopped melee auto-attack (`SMSG_ATTACKSTOP`).
    AttackStop { attacker: u64, victim: u64 },
    /// One melee swing (`SMSG_ATTACKERSTATEUPDATE`): one packet is one swing, no client timer.
    AttackerState(AttackerState),
    /// Our melee swing was refused (`SMSG_ATTACKSWING_*`); sent to the swinger alone.
    AttackSwingError(AttackSwingError),
    /// The server stopped our attack (`SMSG_CANCEL_COMBAT`): the reference stops, no message.
    CancelCombat,
    /// Feign Death resisted (`SMSG_FEIGN_DEATH_RESISTED`): one red line, no state.
    FeignDeathResisted,
    /// `SMSG_AI_REACTION`: 2 hostile (each creature melee start), 0 alert (stealth detection),
    /// anything else a no-op; the reference only plays the vocal.
    AiReaction { unit: u64, reaction: u32 },
    /// A non-triggered cast began (`SMSG_SPELL_START`), instants included with `cast_time_ms` 0.
    SpellStart {
        caster: u64,
        spell_id: u32,
        cast_flags: u16,
        cast_time_ms: u32,
        target: Option<u64>,
        /// The `CAST_FLAG_AMMO` tail: nocked ammo for any caster (`0x60ba30` from `0x6e78b6`).
        ammo_display_id: Option<u32>,
    },
    /// The cast launched (`SMSG_SPELL_GO`); the server times impact off `Spell.dbc` Speed itself.
    SpellGo {
        caster: u64,
        spell_id: u32,
        cast_flags: u16,
        hits: Vec<u64>,
        misses: Vec<(u64, u8)>,
        target: Option<u64>,
        /// The GameObject an open-lock cast targets (`TARGET_FLAG_GAMEOBJECT`), a chest or door.
        go_target: Option<u64>,
        /// The ground point (`TARGET_FLAG_DEST_LOCATION`, raw WoW); the lasting effect is a
        /// DynamicObject.
        dest: Option<[f32; 3]>,
        ammo_display_id: Option<u32>,
        /// The cast item when the first guid is not the caster; the item cooldown keys on it.
        item_caster: Option<u64>,
    },
    /// A channeled beam's hops (`SMSG_SPELL_UPDATE_CHAIN_TARGETS`), consumed once by the next chain
    /// `CharProc`; the reference fills that `unit+0xd44` array from spell-go hits too (`0x6e800d`).
    SpellChainTargets {
        caster: u64,
        spell_id: u32,
        targets: Vec<u64>,
    },
    /// An observed cast was interrupted or cancelled (`SMSG_SPELL_FAILED_OTHER`).
    SpellFailedOther { caster: u64, spell_id: u32 },
    /// Our cast was pushed back by damage (`SMSG_SPELL_DELAYED`); its end moves out by `delay_ms`.
    SpellDelayed { caster: u64, delay_ms: u32 },
    /// Stop our own ranged auto-repeat (`SMSG_CANCEL_AUTO_REPEAT`, self-only).
    CancelAutoRepeat,
    /// `SMSG_SPELL_COOLDOWN` for the player or pet: `(spell_id, cooldown_ms)`, 0 ms meaning the
    /// spell's own `Spell.dbc` recovery; a normal cast's cooldown is client-tracked.
    SpellCooldowns {
        caster: u64,
        cooldowns: Vec<(u32, u32)>,
    },
    /// Put an item instance on the client's fixed 30 s use cooldown (`SMSG_ITEM_COOLDOWN`).
    ItemCooldown { item_guid: u64, spell_id: u32 },
    /// `SMSG_ITEM_TIME_UPDATE`: seconds left on a timed item, 0 when expired. The client shows
    /// this, not `ITEM_FIELD_DURATION` (vmangos `Item::SendTimeUpdate`, `Objects/Item.cpp:1094`).
    ItemTime { item_guid: u64, seconds: u32 },
    /// `SMSG_ITEM_ENCHANT_TIME_UPDATE`: seconds left on a temporary enchant, 0 when expired; the
    /// tooltip countdown's only feed, never the `ITEM_FIELD_ENCHANTMENT` duration.
    ItemEnchantTime {
        item_guid: u64,
        slot: u32,
        seconds: u32,
    },
    /// A talent spell-modifier cell (`SMSG_SET_FLAT_SPELL_MODIFIER`/`_PCT_`), absolute: row
    /// `mask_bit`, a `SpellFamilyFlags` bit, column `op`, neither bounded on the wire. Deviation:
    /// `benilla::spell_mods` refuses an out-of-range pair, because the reference would overrun it.
    SpellModifier {
        flat: bool,
        mask_bit: u8,
        op: u8,
        value: i32,
    },
    /// Start an on-hold (`SPELL_ATTR_COOLDOWN_ON_EVENT`) cooldown now (`SMSG_COOLDOWN_EVENT`).
    CooldownEvent { spell_id: u32, caster: u64 },
    /// Remove one spell's cooldown record (`SMSG_CLEAR_COOLDOWN`).
    ClearCooldown { spell_id: u32, caster: u64 },
    /// Wipe every cooldown for `caster` (`SMSG_COOLDOWN_CHEAT`, the GM reset).
    CooldownCheat { caster: u64 },
    /// Our own channel opened (`MSG_CHANNEL_START`, self-only, no guid on the wire).
    ChannelStart { spell_id: u32, duration_ms: u32 },
    /// A channel's time left (`MSG_CHANNEL_UPDATE`, self-only); 0 is over, natural or interrupted.
    ChannelUpdate { remaining_ms: u32 },
    /// Time left on one of our auras by `UNIT_FIELD_AURA` slot (`SMSG_UPDATE_AURA_DURATION`),
    /// never for a permanent one. It arrives before the delta naming the slot's spell.
    AuraDuration { slot: u8, remaining_ms: u32 },
    /// A spell-visual kit outside the cast sequence (`SMSG_PLAY_SPELL_VISUAL`), such as eat/drink.
    PlaySpellVisual { unit: u64, kit_id: u32 },
    /// Spell damage dealt (`SMSG_SPELLNONMELEEDAMAGELOG`).
    SpellDamageLog(SpellDamageLog),
    /// Periodic aura ticks: DoT, HoT, regen (`SMSG_PERIODICAURALOG`).
    PeriodicAuraLog(PeriodicAuraLog),
    /// A direct heal (`SMSG_SPELLHEALLOG`).
    SpellHealLog(SpellHealLog),
    /// An instant power gain (`SMSG_SPELLENERGIZELOG`).
    SpellEnergizeLog(SpellEnergizeLog),
    /// A damage-shield (Thorns-style) return hit (`SMSG_SPELLDAMAGESHIELD`).
    DamageShield(DamageShield),
    /// Environmental damage (`SMSG_ENVIRONMENTALDAMAGELOG`); kit from `EnvironmentalDamage.dbc`.
    EnvironmentalDamageLog(EnvironmentalDamageLog),
    /// A cast's per-target miss list (`SMSG_SPELLLOGMISS`).
    SpellLogMiss(SpellLogMiss),
    /// The killing blow (`SMSG_PARTYKILLLOG`), the only source of "You have slain %s!".
    PartyKillLog(PartyKillLog),
    /// An instant kill (`SMSG_SPELLINSTAKILLLOG`).
    SpellInstaKillLog(SpellInstaKillLog),
    /// A proc the target resisted (`SMSG_PROCRESIST`).
    ProcResist(SpellOutcomeLog),
    /// A target immune to the spell (`SMSG_SPELLORDAMAGE_IMMUNE`).
    SpellOrDamageImmune(SpellOutcomeLog),
    /// The auras a dispel removed (`SMSG_SPELLDISPELLOG`).
    SpellDispelLog(SpellDispelLog),
    /// The auras a dispel failed to remove (`SMSG_DISPEL_FAILED`).
    DispelFailed(DispelFailed),
    /// An enchant landing on or fading from an item (`SMSG_ENCHANTMENTLOG`).
    EnchantmentLog(EnchantmentLog),
    /// What a cast's effects did (`SMSG_SPELLLOGEXECUTE`).
    SpellLogExecute(SpellLogExecute),
    /// An XP award, kill or non-kill (`SMSG_LOG_XPGAIN`).
    XpGain(XpGain),
    /// A first visit to an area and its XP (`SMSG_EXPLORATION_EXPERIENCE`), the "Discovered" line.
    ExplorationXp(ExplorationXp),
    /// Our own level-up (`SMSG_LEVELUP_INFO`, self-only).
    LevelUp(LevelUpInfo),
    /// A gossip menu (`SMSG_GOSSIP_MESSAGE`): `text_id` is the greeting for
    /// [`crate::messages::npc_text_query`]; `quests` rows are `(quest_id, icon, level, title)`.
    GossipMenu {
        npc: u64,
        text_id: u32,
        options: Vec<GossipOption>,
        quests: Vec<(u32, u32, u32, String)>,
    },
    /// An NPC's `!`/`?` marker (`SMSG_QUESTGIVER_STATUS`), a [`crate::messages::dialog_status`].
    QuestGiverStatus { npc: u64, status: u32 },
    /// The greeting panel: an NPC's offered/active quest rows (`SMSG_QUESTGIVER_QUEST_LIST`).
    QuestGreeting(QuestGiverList),
    /// The accept panel: quest text and rewards on offer (`SMSG_QUESTGIVER_QUEST_DETAILS`).
    QuestDetail(QuestDetails),
    /// The progress panel: what to bring (`SMSG_QUESTGIVER_REQUEST_ITEMS`).
    QuestProgress(QuestRequestItems),
    /// The reward panel: turn-in text and rewards to grant (`SMSG_QUESTGIVER_OFFER_REWARD`).
    QuestOffer(QuestOfferReward),
    /// The turn-in result: XP, money and fixed items granted (`SMSG_QUESTGIVER_QUEST_COMPLETE`).
    QuestComplete(QuestComplete),
    /// The full quest template (`SMSG_QUEST_QUERY_RESPONSE`), asked once and cached by id.
    QuestTemplate(Box<QuestTemplate>),
    /// A kill objective ticked (`SMSG_QUESTUPDATE_ADD_KILL`), the announce line only; a GO
    /// `entry` is `(-id)|0x80000000`, and the durable count is in the `PLAYER_QUEST_LOG` slot.
    QuestObjectiveKill {
        quest_id: u32,
        entry: u32,
        count: u32,
        required: u32,
    },
    /// An item-collection objective toast (`SMSG_QUESTUPDATE_ADD_ITEM`).
    QuestObjectiveItem { item_id: u32, count: u32 },
    /// All objectives done (`SMSG_QUESTUPDATE_COMPLETE`), a line only; the slot state is durable.
    QuestObjectivesComplete { quest_id: u32 },
    /// The quest failed (`SMSG_QUESTUPDATE_FAILED`, or `_FAILEDTIMER` when `timed`).
    QuestFailed { quest_id: u32, timed: bool },
    /// The log refused a new quest: no free slot (`SMSG_QUESTLOG_FULL`).
    QuestLogFull,
    /// A member's verdict on a quest we shared (`MSG_QUEST_PUSH_RESULT`). `member` is that
    /// member, never the sharer ([`crate::messages::QuestPushResult`]); it fills the `%s`.
    QuestPushResult { member: u64, msg: QuestShareMsg },
    /// A member's escort quest offered to us too (`SMSG_QUEST_CONFIRM_ACCEPT`).
    QuestConfirmAccept(QuestConfirmAccept),
    /// The giver won't offer the quest (`SMSG_QUESTGIVER_QUEST_INVALID`): a reason, no quest id
    /// (vmangos `SendCanTakeQuestResponse` on a failed `CanTakeQuest`).
    QuestGiverInvalid { reason: u32 },
    /// An offered quest failed on accept (`SMSG_QUESTGIVER_QUEST_FAILED`); the reference reads it
    /// with a different handler and message table from [`Self::QuestGiverInvalid`].
    QuestGiverFailed { quest_id: u32, reason: u32 },
    /// The gossip window closes (`SMSG_GOSSIP_COMPLETE`); no menu is open server-side.
    GossipComplete,
    /// A guard's directions (`SMSG_GOSSIP_POI`): a map marker sent for an `action_poi_id` option.
    GossipPoi(crate::messages::GossipPoi),
    /// All 8 greeting blocks, undrawn (`SMSG_NPC_TEXT_UPDATE`): the pick depends on the NPC's
    /// gender and a roll as the frame opens; `$N` and its kin are still unsubstituted.
    NpcGreeting {
        text_id: u32,
        blocks: Vec<crate::messages::NpcTextBlock>,
    },
    /// A vendor's stock (`SMSG_LIST_INVENTORY`); `current_count == 0xFFFF_FFFF` is unlimited.
    VendorInventory { vendor: u64, items: Vec<VendorItem> },
    /// Trainer list (`SMSG_TRAINER_LIST`); `trainer_type` 0 class, 1 mount, 2 tradeskill, 3 pet.
    TrainerList {
        trainer: u64,
        trainer_type: u32,
        services: Vec<TrainerSpell>,
        greeting: String,
    },
    /// Forget a player's cached name (`SMSG_INVALIDATE_PLAYER`), the cache's only eviction.
    InvalidatePlayer { guid: u64 },
    /// The stable list (`MSG_LIST_STABLED_PETS`), unasked on the gossip option and on refresh;
    /// slots are rebased, 0 the current pet. Read by slot: a petless hunter has no slot-0 row.
    ListStabledPets {
        npc: u64,
        num_stable_slots: u8,
        pets: Vec<StabledPet>,
    },
    /// Every stable verb's answer (`SMSG_STABLE_RESULT`), a [`crate::messages::stable_result`];
    /// no success carries a list, so repaint with a fresh `MSG_LIST_STABLED_PETS`.
    StableResult { result: u8 },
    /// A purchase took (`SMSG_TRAINER_BUY_SUCCEEDED`); the server never resends the list itself.
    TrainerBuySucceeded { trainer: u64, spell_id: u32 },
    /// A refused purchase (`SMSG_TRAINER_BUY_FAILED`), `error` a [`crate::messages::train_fail`].
    TrainerBuyFailed {
        trainer: u64,
        spell_id: u32,
        error: u32,
    },
    /// A vendor's stock after a purchase (`SMSG_BUY_ITEM`); the item comes by the normal create.
    VendorBuyResult {
        vendor: u64,
        slot: u32,
        new_count: u32,
        purchase_count: u32,
    },
    /// A refused sale (`SMSG_SELL_ITEM`, a [`crate::messages::sell_result`]); a success is silent.
    VendorSellFailed {
        vendor: u64,
        item_guid: u64,
        reason: u8,
    },
    /// A refused purchase (`SMSG_BUY_FAILED`), `reason` a [`crate::messages::buy_result`].
    VendorBuyFailed {
        vendor: u64,
        item_entry: u32,
        reason: u8,
    },
    /// The bank opens (`SMSG_SHOW_BANK`), also unasked via gossip; contents are in the descriptor.
    ShowBank { banker: u64 },
    /// A refused bank-slot purchase (`SMSG_BUY_BANK_SLOT_RESULT`); a success sends nothing, only
    /// the `PLAYER_BYTES_2` bag count and the coinage change.
    BuyBankSlotResult { result: u32 },
    /// A loot window opened (`SMSG_LOOT_RESPONSE`); quest rows follow the normal ones. A row under
    /// a group roll is `ROLL_ONGOING`; under master loot, vmangos marks a member's rows `MASTER`.
    LootResponse {
        guid: u64,
        loot_type: u8,
        gold: u32,
        items: Vec<LootItem>,
    },
    /// A refused loot window (`SMSG_LOOT_RESPONSE`'s error shape), a `loot::loot_error` code.
    LootError { guid: u64, error: u8 },
    /// A loot row was taken, by anyone (`SMSG_LOOT_REMOVED`).
    LootRemoved { slot: u8 },
    /// Our share of the coin (`SMSG_LOOT_MONEY_NOTIFY`), answering `CMSG_LOOT_MONEY`.
    LootMoneyNotify { amount: u32 },
    /// The coin line disappears for every current looter (`SMSG_LOOT_CLEAR_MONEY`).
    LootClearMoney,
    /// The loot window closes (`SMSG_LOOT_RELEASE_RESPONSE`), answering our `CMSG_LOOT_RELEASE`.
    LootReleaseResponse { guid: u64 },
    /// A group roll opened on one drop (`SMSG_LOOT_START_ROLL`); its `rollID` is client-allocated.
    LootStartRoll(LootStartRoll),
    /// A vote or dice (`SMSG_LOOT_ROLL`); `LootRoll::is_dice`/`vote` split the overloaded pair.
    LootRoll(LootRoll),
    /// A group roll resolved (`SMSG_LOOT_ROLL_WON`), closing its frame.
    LootRollWon(LootRollWon),
    /// Everyone passed (`SMSG_LOOT_ALL_PASSED`); the item returns to ordinary looting.
    LootAllPassed(LootAllPassed),
    /// Master-loot candidates (`SMSG_LOOT_MASTER_LIST`), sent by `SendLoot` before its response.
    LootMasterList { candidates: Vec<u64> },
    /// An item landed in our bags (`SMSG_ITEM_PUSH_RESULT`): the "You receive loot" line.
    ItemPushResult(ItemPushResult),
    /// The keepalive echo (`SMSG_PONG`) of a 30 s `CMSG_PING`, matched by `sequence`.
    Pong { sequence: u32 },
    /// Our speed changed (`SMSG_FORCE_*_SPEED_CHANGE`): ack `counter`, the exact `speed` and a live
    /// `MovementInfo`, or vmangos flags anticheat in about 4 s (`CheckPendingMovementChanges`).
    ForceSpeedChange {
        guid: u64,
        kind: crate::messages::SpeedKind,
        counter: u32,
        speed: f32,
    },
    /// Another unit's speed (`SMSG_SPLINE_SET_*_SPEED`/`MSG_MOVE_SET_*_SPEED`), no ack.
    SpeedChanged {
        guid: u64,
        kind: crate::messages::SpeedKind,
        speed: f32,
    },
    /// `SMSG_MOUNTRESULT` (or `_DISMOUNTRESULT` if not `mount`): OK is 10 mounting, 3 dismounting.
    MountResult { mount: bool, code: u32 },
    /// A rider's flourish (`SMSG_MOUNTSPECIAL_ANIM`): MountSpecial (94) on its mount. Whether our
    /// own echo arrives depends on vmangos's broadcaster, so the app plays ours at send instead.
    MountSpecial { guid: u64 },
    /// Control of a unit granted or taken (`SMSG_CLIENT_CONTROL_UPDATE`). Taking it names us with
    /// `allow_move = false`, so "about me" and "may move" are separate questions. A grant needs
    /// `CMSG_SET_ACTIVE_MOVER`: vmangos drops every `MSG_MOVE_*` for an unconfirmed mover.
    ClientControl { mover: u64, allow_move: bool },
    /// A dropped packet: `unparseable` is false when no parse arm exists
    /// (`ServerPacket::Other`), true when its parser errored and the body was skipped.
    PacketDropped { opcode: u16, unparseable: bool },
    /// Where our corpse is (`MSG_CORPSE_QUERY`); `found == false` also comes unasked when it turns
    /// to bones. `display_map`/`position` are entrance-adjusted, `corpse_map` the real map.
    CorpseQuery {
        found: bool,
        display_map: i32,
        position: [f32; 3],
        corpse_map: u32,
    },
    /// Corpse reclaim delay (`SMSG_CORPSE_RECLAIM_DELAY`): 30 s, 60 or 120 s on repeated deaths.
    CorpseReclaimDelay { delay_ms: u32 },
    /// The 10% death durability loss (`SMSG_DURABILITY_DAMAGE_DEATH`, empty body): the red line.
    DurabilityDamageDeath,
    /// A resurrect offer (`SMSG_RESURRECT_REQUEST`), answered by `CMSG_RESURRECT_RESPONSE`.
    /// `name` is empty for a player caster; `sickness`/`has_timer` pick the popup variant.
    ResurrectRequest {
        caster: u64,
        name: String,
        sickness: bool,
        has_timer: bool,
    },
    /// The spirit healer's `CONFIRM_XP_LOSS` popup (`SMSG_SPIRIT_HEALER_CONFIRM`); accepting sends
    /// `CMSG_SPIRIT_HEALER_ACTIVATE` with `npc`.
    SpiritHealerConfirm { npc: u64 },
    /// Our root, water-walk, feather-fall or hover changed. Ack with `counter` and our
    /// `MovementInfo`, or the server never applies it and observers never see it.
    MoveMode {
        guid: u64,
        counter: u32,
        mode: crate::messages::MoveMode,
        apply: bool,
    },
    /// Any unit's movement mode (the twelve `SMSG_SPLINE_MOVE_*`), no ack; on vmangos, a unit the
    /// server drives. `apply` sets the mode's `MOVEMENTFLAGS` bit, and the reference then re-runs
    /// the unit's gait selector (`0x6014ec`).
    SplineMoveMode {
        guid: u64,
        mode: crate::messages::SplineMode,
        apply: bool,
    },
    /// A knockback on our mover (`SMSG_MOVE_KNOCK_BACK`): a launch we fly ourselves, `zspeed`
    /// down-positive. Ack with `counter` and exactly this jump tail, or vmangos logs a cheat and
    /// observers never see it.
    KnockBack {
        guid: u64,
        counter: u32,
        launch: crate::messages::JumpInfo,
    },
    /// Someone invited us to their group (`SMSG_GROUP_INVITE`): the invite popup.
    GroupInvite { inviter: String },
    /// An invite we sent was declined (`SMSG_GROUP_DECLINE`).
    GroupDecline { name: String },
    /// We were removed from our group (`SMSG_GROUP_UNINVITE`, empty body), kicked or left.
    GroupUninvited,
    /// The group's leader changed (`SMSG_GROUP_SET_LEADER`).
    GroupLeaderChanged { name: String },
    /// The group disbanded outright (`SMSG_GROUP_DESTROYED`, empty body).
    GroupDestroyed,
    /// The full roster (`SMSG_GROUP_LIST`), without our own row; `loot` is `None` when alone.
    GroupList {
        group_type: u8,
        own_flags: u8,
        members: Vec<GroupMemberEntry>,
        leader: u64,
        loot: Option<GroupLootInfo>,
    },
    /// A group command's verdict (`SMSG_PARTY_COMMAND_RESULT`): a
    /// [`crate::messages::party_operation`] and a [`crate::messages::party_result`] code.
    PartyCommandResult {
        operation: u32,
        member: String,
        result: u32,
    },
    /// A member's stats (`SMSG_PARTY_MEMBER_STATS`, or the asked-for `_FULL` form when `full`).
    PartyMemberStats {
        guid: u64,
        full: bool,
        info: Box<PartyMemberStatsInfo>,
    },
    /// Someone pinged the minimap (`MSG_MINIMAP_PING`): the group ping marker.
    MinimapPing { guid: u64, x: f32, y: f32 },
    /// One raid-target icon changed (`MSG_RAID_TARGET_UPDATE`, delta shape); `guid == 0` clears it.
    RaidTargetSet { icon: u8, guid: u64 },
    /// The set raid-target icons (`MSG_RAID_TARGET_UPDATE`, full-list shape).
    RaidTargetList { entries: Vec<(u8, u64)> },
    /// The raid leader started a ready check (`MSG_RAID_READY_CHECK`, empty body).
    ReadyCheckRequest,
    /// A member's ready-check answer, to the leader only (`MSG_RAID_READY_CHECK`, with a body).
    ReadyCheckAnswer { guid: u64, ready: u8 },
    /// Our saved raid lockouts (`SMSG_RAID_INSTANCE_INFO`), replacing the list; empty is normal.
    RaidInstanceInfo {
        entries: Vec<crate::messages::RaidInstanceEntry>,
    },
    // ── Instance lockouts ─────────────────────────────────────────────────────────────────
    // Four become client-composed `CHAT_MSG_SYSTEM` lines and two feed `CanShowResetInstances()`;
    // the map name is never on the wire, only a `Map.dbc` id.
    /// A raid lockout's welcome/countdown line (`SMSG_RAID_INSTANCE_MESSAGE`).
    RaidInstanceMessage {
        message: crate::messages::RaidInstanceMessage,
    },
    /// We are now saved to this instance (`SMSG_INSTANCE_SAVE_CREATED`); vmangos always sends
    /// `flag` 0, and 1 is the reference's debug wrapper.
    InstanceSaveCreated { flag: u32 },
    /// An instance reset took (`SMSG_INSTANCE_RESET`); `map` is a `Map.dbc` id.
    InstanceReset { map: u32 },
    /// An instance reset was refused (`SMSG_INSTANCE_RESET_FAILED`).
    InstanceResetFailed {
        failure: crate::messages::InstanceResetFailed,
    },
    /// The `Map.dbc` id of the dungeon we were last inside (`SMSG_UPDATE_LAST_INSTANCE`).
    UpdateLastInstance { map: u32 },
    /// Whether we hold any permanent bind (`SMSG_UPDATE_INSTANCE_OWNERSHIP`).
    UpdateInstanceOwnership { owns: bool },
    /// A duel challenge (`SMSG_DUEL_REQUESTED`), sent to both sides; `arbiter` is the flag object
    /// echoed on accept or cancel, and `challenger` is us when we asked.
    DuelRequested { arbiter: u64, challenger: u64 },
    /// We left the duel-flag bubble (`SMSG_DUEL_OUTOFBOUNDS`): the 10 s forfeit timer runs.
    DuelOutOfBounds,
    /// We are back inside the bubble (`SMSG_DUEL_INBOUNDS`); the forfeit timer is cleared.
    DuelInBounds,
    /// The duel ended (`SMSG_DUEL_COMPLETE`). `started` is false only when it never began.
    DuelComplete { started: bool },
    /// The duel's outcome line (`SMSG_DUEL_WINNER`), to all nearby; `fled` picks the retreat one.
    DuelWinner {
        fled: bool,
        winner: String,
        loser: String,
    },
    /// Start the duel countdown (`SMSG_DUEL_COUNTDOWN`), already in whole seconds.
    DuelCountdown { seconds: u32 },
    /// Another player's honor stats (`MSG_INSPECT_HONOR_STATS`); a refused ask gets no reply.
    InspectHonorStats(InspectHonorStats),
    /// A kill paid out honor (`SMSG_PVP_CREDIT`); a dishonorable kill's `honor` is negative.
    PvpCredit(PvpCredit),
    /// A breath or fatigue bar started or restated (`SMSG_START_MIRROR_TIMER`): with no update
    /// opcode the server resends it on any change, so treat it as idempotent.
    MirrorTimerStart(MirrorTimerStart),
    /// Freeze or unfreeze a mirror timer (`SMSG_PAUSE_MIRROR_TIMER`). vmangos never sends it: the
    /// stock `MirrorTimer.lua` errors on one.
    MirrorTimerPause { kind: u32, paused: bool },
    /// A mirror timer is over (`SMSG_STOP_MIRROR_TIMER`): refilled, or its condition ended.
    MirrorTimerStop { kind: u32 },
    /// The whole friend list (`SMSG_FRIEND_LIST`), replacing what is held; no names on the wire.
    FriendList { friends: Vec<FriendEntry> },
    /// The whole ignore list (`SMSG_IGNORE_LIST`), replacing what is held; guids only.
    IgnoreList { guids: Vec<u64> },
    /// A friend/ignore result (`SMSG_FRIEND_STATUS`): an add/remove ack or a friend's login or
    /// logout broadcast; the [`friend_result`](crate::messages::friend_result) code says which.
    FriendStatus(FriendStatusUpdate),
    /// A `/who` answer (`SMSG_WHO`): at most 49 rows, plus the true match total.
    WhoResults(WhoResults),
    /// A guild's name, ten rank names and tabard (`SMSG_GUILD_QUERY_RESPONSE`), cached by id.
    GuildQueryResponse(GuildQueryResponse),
    /// The whole guild (`SMSG_GUILD_ROSTER`), a complete snapshot that replaces what is held.
    GuildRoster(GuildRoster),
    /// A guild event (`SMSG_GUILD_EVENT`): a [`guild_event`](crate::messages::guild_event) id and
    /// its display-text arguments; a guid rides only on sign-on/off.
    GuildEvent(GuildEventNotice),
    /// A guild verb's verdict (`SMSG_GUILD_COMMAND_RESULT`); its command tag tells result `0x08`'s
    /// two meanings apart ([`guild_command_error`](crate::messages::guild_command_error)).
    GuildCommandResult(GuildCommandResult),
    /// A guild invite (`SMSG_GUILD_INVITE`); `CMSG_GUILD_ACCEPT`/`_DECLINE` do not name it.
    GuildInvite { inviter: String, guild: String },
    /// The player we invited declined (`SMSG_GUILD_DECLINE`), sent to the inviter only.
    GuildDecline { name: String },
    /// The guild's founding date, member and account counts (`SMSG_GUILD_INFO`).
    GuildInfo(GuildInfo),
    /// A registrar's charter list (`SMSG_PETITION_SHOWLIST`), opening its window; one row in 1.12.
    PetitionShowList(PetitionShowList),
    /// A charter's signers (`SMSG_PETITION_SHOW_SIGNATURES`): our own ask or an offer to sign, told
    /// apart only by whether `owner` is us; name and requirement are in the petition query.
    PetitionShowSignatures(PetitionShowSignatures),
    /// A signature's verdict (`SMSG_PETITION_SIGN_RESULTS`); on success signer and owner each get
    /// a copy naming the signer, the same bytes.
    PetitionSignResults(PetitionSignResults),
    /// A petition's guild name and signature requirement (`SMSG_PETITION_QUERY_RESPONSE`).
    PetitionQueryResponse(PetitionQueryResponse),
    /// A turn-in's bare code (`SMSG_TURN_IN_PETITION_RESULTS`); a name collision sends only a
    /// [`Self::GuildCommandResult`].
    TurnInPetitionResults { result: u32 },
    /// Somebody declined our charter (`MSG_PETITION_DECLINE`), sent to the owner only.
    PetitionDeclined { player: u64 },
    /// A charter rename took (`MSG_PETITION_RENAME`); a rejected one is a guild command result.
    PetitionRenamed(PetitionRename),
    /// The taxi map (`SMSG_SHOWTAXINODES`); `nearest_node` is the flight master's own node.
    TaxiNodesShown {
        flightmaster: u64,
        nearest_node: u32,
        known_mask: TaxiMask,
    },
    /// Whether the flight master's nearest node is known (`SMSG_TAXINODE_STATUS`); also sent with
    /// [`Self::NewTaxiPath`] on a first visit (vmangos `SendLearnNewTaxiNode`).
    TaxiNodeStatus { guid: u64, known: bool },
    /// A flight request's verdict (`SMSG_ACTIVATETAXIREPLY`), a [`crate::messages::taxi_reply`]:
    /// 0 is OK; a refusal starts no flight and sends no mount or spline.
    ActivateTaxiReply { code: u32 },
    /// `SMSG_NEW_TAXI_PATH`, empty, sent with [`Self::TaxiNodeStatus`] on a first visit.
    NewTaxiPath,
    /// The inbox (`SMSG_MAIL_LIST_RESULT`), answering `CMSG_GET_MAIL_LIST`.
    MailList { mails: Vec<MailListEntry> },
    /// A mail action's verdict (`SMSG_SEND_MAIL_RESULT`); `equip_error`/`item` are exclusive tails.
    SendMailResult {
        mail_id: u32,
        action: u32,
        error: u32,
        equip_error: Option<u32>,
        item: Option<(u32, u32)>,
    },
    /// One book page (`SMSG_PAGE_TEXT_QUERY_RESPONSE`), for items and `GAMEOBJECT_TYPE_TEXT`;
    /// `next_page_id == 0` ends it, and vmangos pushes the whole chain on the first query.
    PageText {
        page_id: u32,
        text: String,
        next_page_id: u32,
    },
    /// A letter's body (`SMSG_ITEM_TEXT_QUERY_RESPONSE`), for a mail's non-zero `item_text_id`.
    MailItemText { text_id: u32, text: String },
    /// A mail arrived (`SMSG_RECEIVED_MAIL`); `seconds` of delay, always `0.0` from vmangos.
    ReceivedMail { seconds: f32 },
    /// `MSG_QUERY_NEXT_MAIL_TIME`'s reply: `0.0` unread mail, `-86400.0` none; for `HasNewMail()`.
    NextMailTime { seconds: f32 },
    /// `MSG_AUCTION_HELLO`'s reply, which opens the auction window: the auctioneer and its
    /// `AuctionHouse.dbc` row (1..7, the deposit and cut rates).
    AuctionHello { auctioneer: u64, house_id: u32 },
    /// A sell, cancel or bid verdict (`SMSG_AUCTION_COMMAND_RESULT`); `error` selects `tail`,
    /// `auction_id` is 0 on most failures, and several refusals send nothing.
    AuctionCommandResult {
        auction_id: u32,
        action: u32,
        error: u32,
        tail: AuctionCommandTail,
    },
    /// A Browse page (`SMSG_AUCTION_LIST_RESULT`), at most 50 rows; `total_count` is pre-cap.
    AuctionListResult {
        auctions: Vec<AuctionListEntry>,
        total_count: u32,
    },
    /// The Auctions tab page, our own listings (`SMSG_AUCTION_OWNER_LIST_RESULT`).
    AuctionOwnerListResult {
        auctions: Vec<AuctionListEntry>,
        total_count: u32,
    },
    /// The Bids tab page (`SMSG_AUCTION_BIDDER_LIST_RESULT`): refreshed ids first, then live bids,
    /// so a row can appear twice.
    AuctionBidderListResult {
        auctions: Vec<AuctionListEntry>,
        total_count: u32,
    },
    /// We won or were outbid (`SMSG_AUCTION_BIDDER_NOTIFICATION`); `bid_or_zero == 0` means won.
    AuctionBidderNotification(AuctionBidderNotification),
    /// Our auction sold or took a bid (`SMSG_AUCTION_OWNER_NOTIFICATION`); `bidder_guid` 0 is sold.
    AuctionOwnerNotification(AuctionOwnerNotification),
    /// The seller cancelled an auction we bid on (`SMSG_AUCTION_REMOVED_NOTIFICATION`).
    AuctionRemovedNotification {
        auction_id: u32,
        item_entry: u32,
        random_property_id: i32,
    },
    /// One step of the trade state machine (`SMSG_TRADE_STATUS`), refusals included.
    TradeStatus { status: TradeStatus },
    /// One trade side's items and gold (`SMSG_TRADE_STATUS_EXTENDED`), boxed at about 460 bytes.
    TradeStatusExtended { state: Box<TradeStatusExtended> },
    /// World states: `SMSG_INIT_WORLD_STATES` with its `(map, zone)` scope and the wire run
    /// verbatim, terminator included, or `SMSG_UPDATE_WORLD_STATE` as one entry with no scope.
    /// The reference funnels both into one setter too.
    WorldStates {
        scope: Option<(u32, u32)>,
        states: Vec<(u32, u32)>,
    },
}

impl SessionEventKind {
    /// Every kind, in declaration order.
    pub fn all() -> impl Iterator<Item = Self> {
        <Self as strum::IntoEnumIterator>::iter()
    }
}

/// The result of polling the reader for the next packet's events.
pub enum Poll {
    /// A decoded packet; `events` is empty for an opcode parsed but not modelled. `tail` counts
    /// body bytes left unread, which shows a decoder shorter than the server's layout.
    Events {
        opcode: u16,
        events: Vec<SessionEvent>,
        tail: usize,
    },
    /// An unparseable packet, skipped to keep the stream aligned; `reason` has a hex preview.
    Skipped { opcode: u16, reason: String },
}

mod decode;
pub use decode::decode;
