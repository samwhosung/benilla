//! The 1.12.1 world opcode numbers (vmangos `Opcodes_1_12_1.h`). A client packet's header holds
//! the opcode in 4 bytes, but every value fits `u16`; the senders widen it.

pub const SMSG_CHAR_CREATE: u16 = 0x003A;
pub const SMSG_CHAR_ENUM: u16 = 0x003B;
// Character delete: CMSG a full u64 guid, SMSG one result byte (`CHAR_DELETE_SUCCESS` = 0x39).
pub const CMSG_CHAR_DELETE: u16 = 0x0038;
pub const SMSG_CHAR_DELETE: u16 = 0x003C;
/// The server's refusal of a `CMSG_PLAYER_LOGIN`: one result byte.
pub const SMSG_CHARACTER_LOGIN_FAILED: u16 = 0x0041;
pub const SMSG_NAME_QUERY_RESPONSE: u16 = 0x0051; // 81
pub const SMSG_CREATURE_QUERY_RESPONSE: u16 = 0x0061; // 97
/// Answers `CMSG_PET_NAME_QUERY`: `u32 petNumber`, cstring name, `u32 nameTimestamp`. No reply
/// comes unless the guid is a live pet with that number (`PetHandler.cpp:190-192`).
pub const SMSG_PET_NAME_QUERY_RESPONSE: u16 = 0x0053; // 83
/// Answers `CMSG_GAMEOBJECT_QUERY` with the GameObject's template.
pub const SMSG_GAMEOBJECT_QUERY_RESPONSE: u16 = 0x005F; // 95
/// Answers `CMSG_QUEST_QUERY` with the quest's template.
pub const SMSG_QUEST_QUERY_RESPONSE: u16 = 0x005D; // 93

// Page text: the ask-once `PageText.wdb` cache behind readable items and book GameObjects; an id
// names one page, chained by `nextPageId`. Mail letters use `CMSG_ITEM_TEXT_QUERY` instead.
pub const CMSG_PAGE_TEXT_QUERY: u16 = 0x005A; // 90
pub const SMSG_PAGE_TEXT_QUERY_RESPONSE: u16 = 0x005B; // 91

pub const SMSG_NEW_WORLD: u16 = 0x003E;
// The far-teleport preamble: with `SMSG_TRANSFER_PENDING`'s optional transport block, the next
// `SMSG_NEW_WORLD`'s coordinates are transport-local, else world (`Player.cpp:2065-2068`).
pub const SMSG_TRANSFER_PENDING: u16 = 0x003F;
pub const SMSG_TRANSFER_ABORTED: u16 = 0x0040;
pub const SMSG_LOGIN_SETTIMESPEED: u16 = 0x0042;
pub const SMSG_BINDPOINTUPDATE: u16 = 0x0155;
/// The proficiency mask for one item class (vmangos `Skill.h`).
pub const SMSG_SET_PROFICIENCY: u16 = 0x0127;
pub const SMSG_LOGOUT_RESPONSE: u16 = 0x004C;
pub const SMSG_LOGOUT_COMPLETE: u16 = 0x004D;
/// The ack for [`CMSG_LOGOUT_CANCEL`]; empty body.
pub const SMSG_LOGOUT_CANCEL_ACK: u16 = 0x004F;
pub const SMSG_UPDATE_OBJECT: u16 = 0x00A9;
pub const SMSG_DESTROY_OBJECT: u16 = 0x00AA;
// Cinematics: a first login's race intro, or a type-13 GameObject camera. Until the COMPLETE ack,
// vmangos sees from the camera and despawns all around the body (`Player::UpdateCinematic`).
pub const SMSG_TRIGGER_CINEMATIC: u16 = 0x00FA;
/// The next camera of a multi-camera `CinematicSequences` row began; empty body (`0x48efe0`).
pub const CMSG_NEXT_CINEMATIC_CAMERA: u16 = 0x00FB;
pub const CMSG_COMPLETE_CINEMATIC: u16 = 0x00FC;
pub const SMSG_MONSTER_MOVE: u16 = 0x00DD;
/// [`SMSG_MONSTER_MOVE`] for a unit on a transport: the transport's packed guid follows the
/// mover's, and every coordinate is deck-local (`MoveSplineInit.cpp:146-154`).
pub const SMSG_MONSTER_MOVE_TRANSPORT: u16 = 0x02AE; // 686
pub const SMSG_INITIALIZE_FACTIONS: u16 = 0x0122;
/// A faction became visible in the reputation pane: one `u32` list slot. It is the only notice of
/// `FACTION_FLAG_VISIBLE`; no fresh standing comes with it (`ReputationMgr::SendVisible`).
pub const SMSG_SET_FACTION_VISIBLE: u16 = 0x0123;
pub const SMSG_SET_FACTION_STANDING: u16 = 0x0124;
/// Set a faction's at-war flag from the reputation pane; none of the pane's three verbs is acked.
pub const CMSG_SET_FACTION_ATWAR: u16 = 0x0125; // 293
pub const CMSG_SET_FACTION_INACTIVE: u16 = 0x0317; // 791
pub const CMSG_SET_WATCHED_FACTION: u16 = 0x0318; // 792
pub const SMSG_AUTH_CHALLENGE: u16 = 0x01EC;
pub const SMSG_AUTH_RESPONSE: u16 = 0x01EE;
/// The Warden anticheat challenge; vmangos kicks a client that leaves it unanswered for 30 s
/// (`Warden::Update`). Deviation: no Warden ([`crate::WardenRequired`]), so the connect refuses.
pub const SMSG_WARDEN_DATA: u16 = 0x02E6;
/// One record per `## Secure:` addon in `CMSG_AUTH_SESSION`, in order, with no count or names.
pub const SMSG_ADDON_INFO: u16 = 0x02EF;
pub const SMSG_COMPRESSED_UPDATE_OBJECT: u16 = 0x01F6;
/// A zlib envelope of whole movement packets. Routine: vmangos switches a session to it after 300
/// movement packets in ten seconds (`Compression.Movement.Count`).
pub const SMSG_COMPRESSED_MOVES: u16 = 0x02FB;
pub const SMSG_LOGIN_VERIFY_WORLD: u16 = 0x0236;
/// The account's tutorial bits: the whole body (32 bytes from vmangos after login), copied into
/// both of the client's banks. No tutorial fires before it arrives.
pub const SMSG_TUTORIAL_FLAGS: u16 = 0x00FD; // 253
/// `FlagTutorial(n)` and the client's six auto-acknowledge sites: one `u32`, the 0-based id.
pub const CMSG_TUTORIAL_FLAG: u16 = 0x00FE; // 254
/// `ClearTutorials()`: empty body; every bit is set locally and on the server.
pub const CMSG_TUTORIAL_CLEAR: u16 = 0x00FF; // 255
/// `ResetTutorials()`: empty body; every bit is cleared locally and on the server.
pub const CMSG_TUTORIAL_RESET: u16 = 0x0100; // 256
pub const SMSG_PLAY_MUSIC: u16 = 0x0277;
pub const SMSG_PLAY_OBJECT_SOUND: u16 = 0x0278;
pub const SMSG_PLAY_SOUND: u16 = 0x02D2;
/// The zone's weather, pushed by the server.
pub const SMSG_WEATHER: u16 = 0x02F4;
pub const SMSG_EMOTE: u16 = 0x0103;
pub const CMSG_TEXT_EMOTE: u16 = 0x0104;
pub const SMSG_TEXT_EMOTE: u16 = 0x0105;
/// Answers `CMSG_ITEM_QUERY_SINGLE` with the item's template.
pub const SMSG_ITEM_QUERY_SINGLE_RESPONSE: u16 = 0x0058;
pub const CMSG_AUTOEQUIP_ITEM: u16 = 0x010A; // 266
pub const CMSG_AUTOSTORE_BAG_ITEM: u16 = 0x010B; // 267
pub const CMSG_SWAP_ITEM: u16 = 0x010C; // 268
pub const CMSG_SWAP_INV_ITEM: u16 = 0x010D; // 269
pub const CMSG_SPLIT_ITEM: u16 = 0x010E; // 270
pub const CMSG_DESTROYITEM: u16 = 0x0111; // 273, the popup-confirmed world-drop delete
pub const SMSG_INVENTORY_CHANGE_FAILURE: u16 = 0x0112; // 274
/// A bag was auto-equipped: one raw `u64` guid (`ItemHandler.cpp:227`). The reference raises
/// `BAG_OPEN` for it (`0x4f9410`): our own guid is container 0, a cached bag 1..10, else nothing.
pub const SMSG_OPEN_CONTAINER: u16 = 0x0113; // 275

// Trade. CMSG bodies: INITIATE u64 target, ACCEPT u32 (ignored), SET_ITEM 3×u8 (tradeSlot, bag,
// slot), CLEAR_ITEM u8, SET_GOLD u32 copper, the rest empty. SMSG: TRADE_STATUS u32 plus a
// per-status tail; TRADE_STATUS_EXTENDED the 444-byte item/gold snapshot.
pub const CMSG_INITIATE_TRADE: u16 = 0x0116; // 278
pub const CMSG_BEGIN_TRADE: u16 = 0x0117; // 279
pub const CMSG_BUSY_TRADE: u16 = 0x0118; // 280
pub const CMSG_IGNORE_TRADE: u16 = 0x0119; // 281
pub const CMSG_ACCEPT_TRADE: u16 = 0x011A; // 282
pub const CMSG_UNACCEPT_TRADE: u16 = 0x011B; // 283
pub const CMSG_CANCEL_TRADE: u16 = 0x011C; // 284
pub const CMSG_SET_TRADE_ITEM: u16 = 0x011D; // 285
pub const CMSG_CLEAR_TRADE_ITEM: u16 = 0x011E; // 286
pub const CMSG_SET_TRADE_GOLD: u16 = 0x011F; // 287
pub const SMSG_TRADE_STATUS: u16 = 0x0120; // 288
pub const SMSG_TRADE_STATUS_EXTENDED: u16 = 0x0121; // 289
/// Load ammo (`PLAYER_AMMO_ID`); the auto-equip sender `0x5e1480` sends ammo here. Trap: `0x0268`
/// is not decimal 268, which is `CMSG_SWAP_ITEM` (`0x010C`).
pub const CMSG_SET_AMMO: u16 = 0x0268; // 616
/// Set one action-bar button (`HandleSetActionButtonOpcode`, `MiscHandler.cpp:885`).
pub const CMSG_SET_ACTION_BUTTON: u16 = 0x0128; // 296
/// The four extra action bars' visibility: one `u8`, stored as `PLAYER_FIELD_BYTES` byte 2
/// (sent only from `0x4e771d`; `MiscHandler.cpp:923-932`).
pub const CMSG_SET_ACTIONBAR_TOGGLES: u16 = 0x02BF; // 703
pub const SMSG_ACTION_BUTTONS: u16 = 0x0129; // 297
pub const SMSG_INITIAL_SPELLS: u16 = 0x012A; // 298

// Spell-book changes after login: a spell learned, and a rank swapped in place.
pub const SMSG_LEARNED_SPELL: u16 = 0x012B; // 299
pub const SMSG_SUPERCEDED_SPELL: u16 = 0x012C; // 300

/// A spell left the book (`Player::RemoveSpell`): one `u16` id; a talent wipe sends one per rank.
pub const SMSG_REMOVED_SPELL: u16 = 0x0203; // 515
pub const SMSG_CAST_RESULT: u16 = 0x0130; // 304

/// Whether we may drive one unit: packed guid, `u8 allowMove` (`Misc.cpp:677-682`). Each packet
/// is about one unit, not a swap: ending Mind Control sends the caster `(self, 1)` then
/// `(victim, 0)`. The server drops `MSG_MOVE_*` for a new mover until [`CMSG_SET_ACTIVE_MOVER`]
/// answers, and never stops a possessed player: `allowMove = 0` on our guid is ours to enforce.
pub const SMSG_CLIENT_CONTROL_UPDATE: u16 = 0x0159; // 345
/// The whole pet bar: ten slots, react/command state, spell list, cooldowns. The 8-byte
/// guid-only form is the teardown, the only sign the bar is gone (`Player::RemovePetActionBar`).
pub const SMSG_PET_SPELLS: u16 = 0x0179; // 377
/// The state-only refresh: the same four state bytes, with no bar behind them.
pub const SMSG_PET_MODE: u16 = 0x017A; // 378
/// One reason byte for a refused pet order (the red error line).
pub const SMSG_PET_ACTION_FEEDBACK: u16 = 0x02C6; // 710
/// [`SMSG_CAST_RESULT`] for a failed pet cast, with the same `SpellCastResult` codes.
pub const SMSG_PET_CAST_FAILED: u16 = 0x0138; // 312

/// Tame, Call Pet or Revive Pet refused: one `u8` reason. The reference (`0x6e97e0`) shows
/// `ERR_TAME_FAILED` with the reason's `PETTAME_*` string, `PETTAME_UNKNOWNERROR` outside 1..=11.
pub const SMSG_PET_TAME_FAILURE: u16 = 0x0173; // 371
/// A refused pet rename: empty body (`PetHandler.cpp:542-548`), yet the reference shows
/// `ERR_INVALID_PETNAME` for it (`0x5e3e33`).
pub const SMSG_PET_NAME_INVALID: u16 = 0x0178; // 376
/// The pet ran away at zero loyalty (`Pet.cpp:822`): empty body. The reference (`0x4bdc00`) only
/// shows `ERR_PET_BROKEN`; the bar's teardown still comes as an `SMSG_PET_SPELLS`.
pub const SMSG_PET_BROKEN: u16 = 0x02AF; // 687
/// The pet's voice: `u64 petGuid`, `u32` talk selector (`Pet.h:98`). The reference (`0x6040c0`)
/// barks `CreatureSoundData` column 28 (`PetOrder`) for 0, 27 (`PetAttack`) for 1, else nothing,
/// through the unit's one-shot voice slot.
pub const SMSG_PET_ACTION_SOUND: u16 = 0x0324; // 804
/// A dismissed pet's parting sound: `u32 creatureModelDataId`, `f32` x, y, z. The reference
/// (`0x604140`) plays that model's `CreatureSoundData` column 29 there at `z + 1.0`, with no unit.
/// vmangos never sends it.
pub const SMSG_PET_DISMISS_SOUND: u16 = 0x0325; // 805

/// Our own server-driven spline (Charge, taxi) finished: `MovementInfo` at the endpoint, the
/// `splineId`, and a float the server skips (`MoveSplineDone::ReadFromWorldPacket`).
pub const CMSG_MOVE_SPLINE_DONE: u16 = 0x02C9; // 713
/// Our movement clock skipped (a stall, a long frame): the mover and the missing milliseconds.
/// vmangos shifts its clock, and re-creates a just-boarded transport (`MovementHandler.cpp:989`).
pub const CMSG_MOVE_TIME_SKIPPED: u16 = 0x02CE; // 718
/// Another mover's clock skip, relayed: packed guid, `u32` lag (`MovementHandler.cpp:1005-1011`).
/// The reference (`0x603b40`) adds it to that unit's last wire timestamp (`CMovement+0xac`); if
/// dropped, the unit's next packet lands `lag` ms late.
pub const MSG_MOVE_TIME_SKIPPED: u16 = 0x0319; // 793
pub const SMSG_ATTACKSTART: u16 = 0x0143; // 323
pub const SMSG_ATTACKSTOP: u16 = 0x0144; // 324
pub const SMSG_ATTACKERSTATEUPDATE: u16 = 0x014A; // 330

// The server's refusals of a `CMSG_ATTACKSWING`, all with empty bodies (`Combat.cpp`). `0x147`
// NOTSTANDING is absent on purpose: the reference registers no handler for it (`0x6255b0`).
pub const SMSG_ATTACKSWING_NOTINRANGE: u16 = 0x0145; // 325
pub const SMSG_ATTACKSWING_BADFACING: u16 = 0x0146; // 326
pub const SMSG_ATTACKSWING_DEADTARGET: u16 = 0x0148; // 328
pub const SMSG_ATTACKSWING_CANT_ATTACK: u16 = 0x0149; // 329

/// The forced attack cancel: empty body (`Player::SendAttackSwingCancelAttack`). The reference
/// (`0x5e7dd0`) stops our attack and shows nothing.
pub const SMSG_CANCEL_COMBAT: u16 = 0x014E; // 334

/// A Feign Death was resisted: empty body (`Unit.cpp:9465-9471`). The reference (`0x6e9800`) only
/// shows `ERR_FEIGN_DEATH_RESISTED`.
pub const SMSG_FEIGN_DEATH_RESISTED: u16 = 0x02B4; // 692
/// A creature's aggro or alert flare.
pub const SMSG_AI_REACTION: u16 = 0x013C; // 316

pub const SMSG_SPELL_START: u16 = 0x0131; // 305
pub const SMSG_SPELL_GO: u16 = 0x0132; // 306
pub const SMSG_PLAY_SPELL_VISUAL: u16 = 0x01F3; // 499
/// Save the tabard design, both ways; the reply is one of six results.
pub const MSG_SAVE_GUILD_EMBLEM: u16 = 0x01F1; // 497
pub const MSG_TABARDVENDOR_ACTIVATE: u16 = 0x01F2; // 498
pub const SMSG_CANCEL_AUTO_REPEAT: u16 = 0x029C; // 668
pub const SMSG_SPELL_FAILED_OTHER: u16 = 0x02A6; // 678

/// The only source of a chain beam's extra hops: the reference (`0x6e9820`) fills `unit+0xd44`,
/// which the chain `CharProc` consumes once and zeroes.
pub const SMSG_SPELL_UPDATE_CHAIN_TARGETS: u16 = 0x0330; // 816

/// A talent's flat spell modifier; the next opcode is the percent one. Both carry the same 6-byte
/// body, the absolute value of one table cell, never a delta (reference `0x6e9950`).
pub const SMSG_SET_FLAT_SPELL_MODIFIER: u16 = 0x0266; // 614
pub const SMSG_SET_PCT_SPELL_MODIFIER: u16 = 0x0267; // 615

pub const SMSG_SPELL_COOLDOWN: u16 = 0x0134; // 308
pub const SMSG_ITEM_COOLDOWN: u16 = 0x00B0; // 176
pub const SMSG_COOLDOWN_EVENT: u16 = 0x0135; // 309
pub const SMSG_CLEAR_COOLDOWN: u16 = 0x01DE; // 478
pub const SMSG_COOLDOWN_CHEAT: u16 = 0x01E1; // 481

/// A timed item's remaining life in seconds; the display reads this, not `ITEM_FIELD_DURATION`
/// (`Item.cpp:1094`). It shares the reference handler (`0x5e4f69`) with the enchant update below.
pub const SMSG_ITEM_TIME_UPDATE: u16 = 0x01EA; // 490

/// The only source of a temporary enchant's remaining time: the reference tooltip reads a
/// per-slot deadline (`obj + slot*4 + 0x324`) that this handler (`0x5e4f82`) writes.
pub const SMSG_ITEM_ENCHANT_TIME_UPDATE: u16 = 0x01EB; // 491

/// Cast pushback, to the caster: raw `u64` guid, `u32` delay ms (`Spell.cpp:7472`), and the cast
/// bar slides by it. A hit only pushes a cast back, never interrupts it.
pub const SMSG_SPELL_DELAYED: u16 = 0x01E2; // 482

// The channel bar, to the caster only: START is `u32 spellId`, `u32 duration_ms`; UPDATE is
// `u32 remaining_ms`, 0 when the channel is over.
pub const MSG_CHANNEL_START: u16 = 0x0139; // 313
pub const MSG_CHANNEL_UPDATE: u16 = 0x013A; // 314

/// Our own aura's remaining time: `u8 slot`, `u32 remaining_ms`; never for others' or permanent
/// auras (`SpellAuras.cpp:7511-7523`). It arrives before the `UNIT_FIELD_AURA` delta for the slot.
pub const SMSG_UPDATE_AURA_DURATION: u16 = 0x0137; // 311

/// Our own level-up, with no guid and sent only to the leveling player (`Player::GiveLevel`).
pub const SMSG_LEVELUP_INFO: u16 = 0x01D4; // 468
/// A newly explored area: the area id and the XP it granted (0 at max level).
pub const SMSG_EXPLORATION_EXPERIENCE: u16 = 0x01F8; // 504
/// An honor award, the only attributed notice of one; dishonorable kills carry negative honor.
pub const SMSG_PVP_CREDIT: u16 = 0x028C; // 652

pub const SMSG_ENVIRONMENTALDAMAGELOG: u16 = 0x01FC; // 508
pub const SMSG_LOG_XPGAIN: u16 = 0x01D0; // 464
pub const SMSG_SPELLLOGMISS: u16 = 0x024B; // 587
pub const SMSG_PERIODICAURALOG: u16 = 0x024E; // 590
pub const SMSG_SPELLDAMAGESHIELD: u16 = 0x024F; // 591
pub const SMSG_SPELLNONMELEEDAMAGELOG: u16 = 0x0250; // 592

pub const SMSG_ENCHANTMENTLOG: u16 = 0x01D7; // 471
pub const SMSG_PARTYKILLLOG: u16 = 0x01F5; // 501
pub const SMSG_SPELLLOGEXECUTE: u16 = 0x024C; // 588
pub const SMSG_PROCRESIST: u16 = 0x0260; // 608
pub const SMSG_DISPEL_FAILED: u16 = 0x0262; // 610
pub const SMSG_SPELLORDAMAGE_IMMUNE: u16 = 0x0263; // 611
pub const SMSG_SPELLDISPELLOG: u16 = 0x027B; // 635
pub const SMSG_SPELLINSTAKILLLOG: u16 = 0x032F; // 815

pub const SMSG_SPELLHEALLOG: u16 = 0x0150; // 336
pub const SMSG_SPELLENERGIZELOG: u16 = 0x0151; // 337

pub const CMSG_CHAR_CREATE: u16 = 0x0036;
pub const CMSG_CHAR_ENUM: u16 = 0x0037;
pub const CMSG_PLAYER_LOGIN: u16 = 0x003D;
pub const CMSG_LOGOUT_REQUEST: u16 = 0x004B;
/// The forced logout (`ForceLogout`, `0x48ab50`): empty body, sent in place of
/// `CMSG_LOGOUT_REQUEST` and past the pending-logout latch.
pub const CMSG_PLAYER_LOGOUT: u16 = 0x004A; // 74
/// The CAMP/QUIT dialog's Cancel: empty body, answered by [`SMSG_LOGOUT_CANCEL_ACK`].
pub const CMSG_LOGOUT_CANCEL: u16 = 0x004E;
pub const CMSG_NAME_QUERY: u16 = 0x0050; // 80
pub const CMSG_CREATURE_QUERY: u16 = 0x0060; // 96
/// Ask a pet's name; answered by [`SMSG_PET_NAME_QUERY_RESPONSE`].
pub const CMSG_PET_NAME_QUERY: u16 = 0x0052; // 82

/// A pet bar press: the slot's packed word and a target guid, dispatched on its type byte.
pub const CMSG_PET_ACTION: u16 = 0x0175; // 373
/// A pet bar drag: one or two `(position, packed)` pairs, told apart by body size alone.
pub const CMSG_PET_SET_ACTION: u16 = 0x0174; // 372
/// Set a pet spell's autocast bit to a given value (the right-click).
pub const CMSG_PET_SPELL_AUTOCAST: u16 = 0x02F3; // 755
/// Call the pet off: the Attack button's second press.
pub const CMSG_PET_STOP_ATTACK: u16 = 0x02EA; // 746
/// Cancel one of the pet's auras; `CMSG_CANCEL_AURA` carries only a spell id, no unit.
pub const CMSG_PET_CANCEL_AURA: u16 = 0x026B; // 619

/// The pet menu's Abandon (`PetAbandon`, `0x4be4c0`): deletes a hunter pet, unsummons anything
/// else (`PetHandler.cpp:347-374`). Dismiss sends [`CMSG_PET_ACTION`] word `0x07000003` instead.
pub const CMSG_PET_ABANDON: u16 = 0x0176; // 374
/// Rename the pet (`PetRename`, `0x4be4e0`). One-shot: the server clears `UNIT_FLAG_PET_RENAME`
/// on success (`PetHandler.cpp:302-345`); a refusal is [`SMSG_PET_NAME_INVALID`].
pub const CMSG_PET_RENAME: u16 = 0x0177; // 375

/// The ask-once GameObject template lookup, shaped like `CMSG_CREATURE_QUERY`.
pub const CMSG_GAMEOBJECT_QUERY: u16 = 0x005E; // 94
/// Ask for a quest's template; answered by [`SMSG_QUEST_QUERY_RESPONSE`].
pub const CMSG_QUEST_QUERY: u16 = 0x005C; // 92
pub const CMSG_ITEM_QUERY_SINGLE: u16 = 0x0056; // 86
pub const CMSG_USE_ITEM: u16 = 0x00AB; // 171
/// Open an openable item (a clam, a lockbox, a gift) by bag position; the loot that answers names
/// the item's own guid (`HandleOpenItemOpcode`).
pub const CMSG_OPEN_ITEM: u16 = 0x00AC; // 172
/// Gift-wrap an item: `giftBag`, `giftSlot`, `itemBag`, `itemSlot`, paper first. Sent when the
/// wrap cursor, armed locally by right-clicking the paper (`0x5edea0`), clicks the item.
pub const CMSG_WRAP_ITEM: u16 = 0x01D3; // 467
/// Use a GameObject by full guid; chests loot through this, since `CMSG_LOOT` refuses their guids.
pub const CMSG_GAMEOBJ_USE: u16 = 0x00B1; // 177
/// A GameObject's one-shot custom animation: `u64 guid`, `u32 animId`. The client arms substate
/// `8 + animId` (AnimationData 153..156) and rejects `animId >= 4`; the bobber's bite is 0.
pub const SMSG_GAMEOBJECT_CUSTOM_ANIM: u16 = 0x00B3; // 179
/// The despawn animation: a bare `u64` guid, from `WorldObject` so totems and DynamicObjects too.
/// The client arms substate 12 (AnimationData 157) and the object outlives its
/// `SMSG_DESTROY_OBJECT` for that play.
pub const SMSG_GAMEOBJECT_DESPAWN_ANIM: u16 = 0x0215; // 533
/// We walked into an area trigger: its `AreaTrigger.dbc` id (reference `0x5e2110`).
pub const CMSG_AREATRIGGER: u16 = 0x00B4; // 180
/// An area trigger's refusal (level, ghost, faction), shown like `SMSG_NOTIFICATION` (`0x4945b0`).
pub const SMSG_AREA_TRIGGER_MESSAGE: u16 = 0x02B8; // 696
pub const CMSG_MESSAGECHAT: u16 = 0x0095;
pub const SMSG_MESSAGECHAT: u16 = 0x0096;
// Channels: every CMSG is `cstring channelName` [+ `cstring playerName`]; 1.12 has no channel id.
pub const CMSG_JOIN_CHANNEL: u16 = 0x0097; // 151
pub const CMSG_LEAVE_CHANNEL: u16 = 0x0098; // 152
pub const SMSG_CHANNEL_NOTIFY: u16 = 0x0099; // 153
pub const CMSG_CHANNEL_LIST: u16 = 0x009A; // 154
pub const SMSG_CHANNEL_LIST: u16 = 0x009B; // 155
pub const CMSG_CHANNEL_PASSWORD: u16 = 0x009C; // 156
pub const CMSG_CHANNEL_SET_OWNER: u16 = 0x009D; // 157
pub const CMSG_CHANNEL_OWNER: u16 = 0x009E; // 158
pub const CMSG_CHANNEL_MODERATOR: u16 = 0x009F; // 159
pub const CMSG_CHANNEL_UNMODERATOR: u16 = 0x00A0; // 160
pub const CMSG_CHANNEL_MUTE: u16 = 0x00A1; // 161
pub const CMSG_CHANNEL_UNMUTE: u16 = 0x00A2; // 162
pub const CMSG_CHANNEL_INVITE: u16 = 0x00A3; // 163
pub const CMSG_CHANNEL_KICK: u16 = 0x00A4; // 164
pub const CMSG_CHANNEL_BAN: u16 = 0x00A5; // 165
pub const CMSG_CHANNEL_UNBAN: u16 = 0x00A6; // 166
pub const CMSG_CHANNEL_ANNOUNCEMENTS: u16 = 0x00A7; // 167
pub const CMSG_CHANNEL_MODERATE: u16 = 0x00A8; // 168

/// The ignore notice ("Name is now ignoring you"): a raw `u64` guid (`Misc.cpp:127-130`).
pub const CMSG_CHAT_IGNORED: u16 = 0x0225; // 549
/// A whisper target is not online: one cstring name (`Chat.cpp:26-29`).
pub const SMSG_CHAT_PLAYER_NOT_FOUND: u16 = 0x02A9; // 681
/// A cross-faction whisper was refused; empty body (`Chat.cpp:16-18`).
pub const SMSG_CHAT_WRONG_FACTION: u16 = 0x0219; // 537
/// Nothing hooked (the bobber expired or was clicked early): empty body, `ERR_FISH_NOT_HOOKED`.
pub const SMSG_FISH_NOT_HOOKED: u16 = 0x01C8; // 456
/// The hooked fish got away (the skill roll failed); empty body, `ERR_FISH_ESCAPED`.
pub const SMSG_FISH_ESCAPED: u16 = 0x01C9; // 457
/// A server notice shown in the red UIErrorsFrame: one cstring (`WorldSession.cpp:900-915`).
pub const SMSG_NOTIFICATION: u16 = 0x01CB; // 459

// World broadcasts: to everyone, or to everyone in a zone.
/// An area is under attack: one `u32` `AreaTable.dbc` id. The client (`0x49dcc0`) fills
/// `ZONE_UNDER_ATTACK` and posts it on the joined defense channels, not as a system line.
pub const SMSG_ZONE_UNDER_ATTACK: u16 = 0x0254; // 596
/// A shutdown countdown or operator broadcast: a `u32` `ServerMessages.dbc` id, whose text formats
/// the cstring that follows; shown as `CHAT_MSG_SYSTEM` (`0x49df80`).
pub const SMSG_SERVER_MESSAGE: u16 = 0x0291; // 657
/// A trial account hit its whisper cap: empty body; the client (`0x5e4a09`) shows error `0x1c3`.
pub const SMSG_CHAT_RESTRICTED: u16 = 0x02FD; // 765
/// A defense broadcast (the Eastern Plaguelands towers): `u32 zoneId`, `u32 length`, the text
/// (`Map.cpp:1868-1884`). Posted like [`SMSG_ZONE_UNDER_ATTACK`] (`0x49de30`).
pub const SMSG_DEFENSE_MESSAGE: u16 = 0x033B; // 827

/// `/played`: an empty request; the reply is `u32 total`, `u32 level` seconds (`Misc.cpp:278-282`).
pub const CMSG_PLAYED_TIME: u16 = 0x01CC; // 460
pub const SMSG_PLAYED_TIME: u16 = 0x01CD; // 461
/// Ask the server's clock: an empty request; the reply is one `u32` of unix seconds
/// (`QueryHandler.cpp:418-423`). Timed-quest deadlines are stamps on this clock.
pub const CMSG_QUERY_TIME: u16 = 0x01CE; // 462
pub const SMSG_QUERY_TIME_RESPONSE: u16 = 0x01CF; // 463
/// `/random`, both ways: we send `u32 min`, `u32 max`; the server broadcasts min, max,
/// `u32 roll`, `u64 guid` (`GroupHandler.cpp:394-422`).
pub const MSG_RANDOM_ROLL: u16 = 0x01FB; // 507

/// Ask for a new stand state.
pub const CMSG_STANDSTATECHANGE: u16 = 0x0101; // 257
/// A server-side change to our stand state (eat/drink sit, stand on damage): one `u8`, no guid
/// (`Unit.cpp:9541`). The reference (`0x603e50`) applies it to the local player, ungated.
pub const SMSG_STANDSTATE_UPDATE: u16 = 0x029D; // 669
pub const CMSG_CAST_SPELL: u16 = 0x012E; // 302
/// Cancel our cast: one `u32` spell id. The wand auto-repeat handoff sends it for the cached Shoot
/// before the local cancel (`0x6095b8`).
pub const CMSG_CANCEL_CAST: u16 = 0x012F; // 303
pub const CMSG_CANCEL_AURA: u16 = 0x0136; // 310
/// Cancel our channel: a `u32` spell id the server ignores (`Spell.h:69`) and 1.12 still sends.
pub const CMSG_CANCEL_CHANNELLING: u16 = 0x013B; // 315
/// Inspect a player: a raw 8-byte guid. The server selects it, then answers `SMSG_INSPECT` unless
/// it is beyond 10 yd or attackable (`MiscHandler.cpp:943-960`).
pub const CMSG_INSPECT: u16 = 0x0114; // 276
/// The inspect reply, the echoed `u64` guid; the reference discards it (`0x5e7d70`), so we drop it.
pub const SMSG_INSPECT: u16 = 0x0115; // 277
/// The inspect Honor tab, both ways: we send a raw 8-byte guid, the reply is a 50-byte body.
/// Refused silently like [`CMSG_INSPECT`], but without selecting (`MiscHandler.cpp:962-972`).
pub const MSG_INSPECT_HONOR_STATS: u16 = 0x02D6; // 726
/// Right-click a battlemaster (`0x5e01a0`): a `u64` guid; answered by `SMSG_BATTLEFIELD_LIST`.
pub const CMSG_BATTLEMASTER_HELLO: u16 = 0x02D7; // 727
pub const CMSG_SET_SELECTION: u16 = 0x013D; // 317
pub const CMSG_ATTACKSWING: u16 = 0x0141; // 321
pub const CMSG_ATTACKSTOP: u16 = 0x0142; // 322
/// Set our weapon sheath state.
pub const CMSG_SETSHEATHED: u16 = 0x01E0; // 480
pub const CMSG_AUTH_SESSION: u16 = 0x01ED;
pub const CMSG_SET_ACTIVE_MOVER: u16 = 0x026A;
/// Give up a mover: its full u64 guid, then a `MovementInfo`. The server re-broadcasts a stop for
/// it; unsent, observers keep its last relayed pose (`MovementHandler.cpp:886-965`).
pub const CMSG_MOVE_NOT_ACTIVE_MOVER: u16 = 0x02D1; // 721
/// The far-sight toggle: one `u8`, 1 views through the `PLAYER_FARSIGHT` object, 0 through our
/// body (`MiscHandler.cpp:1138-1155`). Sending 0 moves visibility home while the camera stays on
/// the object, so it must be deliberate; the server expects no reply.
pub const CMSG_FAR_SIGHT: u16 = 0x027A; // 634
/// Stop auto-repeat: empty body, sent from the local cancel (`0x6ea080`), so on every cancel.
pub const CMSG_CANCEL_AUTO_REPEAT_SPELL: u16 = 0x026D; // 621

// Keepalive: the 1.12 client pings every 30 s (`0x537ff0`) with `u32 sequence`, `u32 lastRtt`,
// and the pong echoes the sequence. vmangos kicks after more than 2 pings under 27 s apart.
pub const CMSG_PING: u16 = 0x01DC; // 476
pub const SMSG_PONG: u16 = 0x01DD; // 477

// Speed changes to a unit's controller: `[packed guid][u32 counter][f32 yd/s]`. The client must
// ack with `[full u64 guid][u32 counter][MovementInfo][f32 speed]`, the speed within 0.01 and the
// counter pending; unacked, vmangos force-resolves after 4 s and flags it as cheating.
pub const SMSG_FORCE_RUN_SPEED_CHANGE: u16 = 0x00E2; // 226
pub const CMSG_FORCE_RUN_SPEED_CHANGE_ACK: u16 = 0x00E3; // 227
pub const SMSG_FORCE_RUN_BACK_SPEED_CHANGE: u16 = 0x00E4; // 228
pub const CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK: u16 = 0x00E5; // 229
pub const SMSG_FORCE_SWIM_SPEED_CHANGE: u16 = 0x00E6; // 230
pub const CMSG_FORCE_SWIM_SPEED_CHANGE_ACK: u16 = 0x00E7; // 231
pub const SMSG_FORCE_WALK_SPEED_CHANGE: u16 = 0x02DA; // 730
pub const CMSG_FORCE_WALK_SPEED_CHANGE_ACK: u16 = 0x02DB; // 731
pub const SMSG_FORCE_SWIM_BACK_SPEED_CHANGE: u16 = 0x02DC; // 732
pub const CMSG_FORCE_SWIM_BACK_SPEED_CHANGE_ACK: u16 = 0x02DD; // 733
pub const SMSG_FORCE_TURN_RATE_CHANGE: u16 = 0x02DE; // 734
pub const CMSG_FORCE_TURN_RATE_CHANGE_ACK: u16 = 0x02DF; // 735

// Speed changes on units we do not control, never acked. A creature's, or a player's mid-spline,
// come as `SMSG_SPLINE_SET_*` `[packed guid][f32 speed]`; a freely moving player's come as
// `MSG_MOVE_SET_*` `[packed guid][MovementInfo][f32 speed]`, which also carries a fresh pose.
pub const SMSG_SPLINE_SET_RUN_SPEED: u16 = 0x02FE; // 766
pub const SMSG_SPLINE_SET_RUN_BACK_SPEED: u16 = 0x02FF; // 767
pub const SMSG_SPLINE_SET_SWIM_SPEED: u16 = 0x0300; // 768
pub const SMSG_SPLINE_SET_WALK_SPEED: u16 = 0x0301; // 769
pub const SMSG_SPLINE_SET_SWIM_BACK_SPEED: u16 = 0x0302; // 770
pub const SMSG_SPLINE_SET_TURN_RATE: u16 = 0x0303; // 771

// Mode changes on any unit, for observers: a bare packed guid, no counter, no ack
// (`MovementPacketSender.cpp:399-462`). The reference handles all twelve at `0x603c80`, drops an
// unresolvable guid, and re-picks the unit's animation after each. Trap: SET_RUN_MODE clears
// `MOVEFLAG_WALK_MODE` and SET_WALK_MODE sets it (`0x617e80`).
pub const SMSG_SPLINE_MOVE_UNROOT: u16 = 0x0304; // 772
pub const SMSG_SPLINE_MOVE_FEATHER_FALL: u16 = 0x0305; // 773
pub const SMSG_SPLINE_MOVE_NORMAL_FALL: u16 = 0x0306; // 774
pub const SMSG_SPLINE_MOVE_SET_HOVER: u16 = 0x0307; // 775
pub const SMSG_SPLINE_MOVE_UNSET_HOVER: u16 = 0x0308; // 776
pub const SMSG_SPLINE_MOVE_WATER_WALK: u16 = 0x0309; // 777
pub const SMSG_SPLINE_MOVE_LAND_WALK: u16 = 0x030A; // 778
pub const SMSG_SPLINE_MOVE_START_SWIM: u16 = 0x030B; // 779
pub const SMSG_SPLINE_MOVE_STOP_SWIM: u16 = 0x030C; // 780
pub const SMSG_SPLINE_MOVE_SET_RUN_MODE: u16 = 0x030D; // 781
pub const SMSG_SPLINE_MOVE_SET_WALK_MODE: u16 = 0x030E; // 782
pub const SMSG_SPLINE_MOVE_ROOT: u16 = 0x031A; // 794
pub const MSG_MOVE_SET_RUN_SPEED: u16 = 0x00CD; // 205
pub const MSG_MOVE_SET_RUN_BACK_SPEED: u16 = 0x00CF; // 207
pub const MSG_MOVE_SET_WALK_SPEED: u16 = 0x00D1; // 209
pub const MSG_MOVE_SET_SWIM_SPEED: u16 = 0x00D3; // 211
pub const MSG_MOVE_SET_SWIM_BACK_SPEED: u16 = 0x00D5; // 213
pub const MSG_MOVE_SET_TURN_RATE: u16 = 0x00D8; // 216

// Mount results: one `u32` code (`Player::SendMountResult`). Success (`MOUNTRESULT_OK` = 10,
// `DISMOUNTRESULT_OK` = 3) is silent in the reference; any other code is a red error line.
pub const SMSG_MOUNTRESULT: u16 = 0x016E; // 366
pub const SMSG_DISMOUNTRESULT: u16 = 0x016F; // 367

// The mounted flourish, an empty CMSG; who gets the echo: `ServerPacket::MountSpecialAnim`.
pub const CMSG_MOUNTSPECIAL_ANIM: u16 = 0x0171; // 369
pub const SMSG_MOUNTSPECIAL_ANIM: u16 = 0x0172; // 370

// Movement, both ways: vmangos relays each `HandleMovementOpcodes` opcode to nearby clients as
// `[packed mover guid][MovementInfo]` (`Opcodes.cpp`); the TELEPORT and WORLDPORT acks differ.
pub const MSG_MOVE_START_FORWARD: u16 = 0x00B5; // 181
pub const MSG_MOVE_START_BACKWARD: u16 = 0x00B6; // 182
pub const MSG_MOVE_STOP: u16 = 0x00B7; // 183
pub const MSG_MOVE_START_STRAFE_LEFT: u16 = 0x00B8; // 184
pub const MSG_MOVE_START_STRAFE_RIGHT: u16 = 0x00B9; // 185
pub const MSG_MOVE_STOP_STRAFE: u16 = 0x00BA; // 186
pub const MSG_MOVE_JUMP: u16 = 0x00BB; // 187
pub const MSG_MOVE_START_TURN_LEFT: u16 = 0x00BC; // 188
pub const MSG_MOVE_START_TURN_RIGHT: u16 = 0x00BD; // 189
pub const MSG_MOVE_STOP_TURN: u16 = 0x00BE; // 190
pub const MSG_MOVE_START_PITCH_UP: u16 = 0x00BF; // 191
pub const MSG_MOVE_START_PITCH_DOWN: u16 = 0x00C0; // 192
pub const MSG_MOVE_STOP_PITCH: u16 = 0x00C1; // 193
pub const MSG_MOVE_SET_RUN_MODE: u16 = 0x00C2; // 194
pub const MSG_MOVE_SET_WALK_MODE: u16 = 0x00C3; // 195
pub const MSG_MOVE_TELEPORT_ACK: u16 = 0x00C7; // 199
pub const MSG_MOVE_FALL_LAND: u16 = 0x00C9; // 201
pub const MSG_MOVE_START_SWIM: u16 = 0x00CA; // 202
pub const MSG_MOVE_STOP_SWIM: u16 = 0x00CB; // 203
pub const MSG_MOVE_WORLDPORT_ACK: u16 = 0x00DC; // 220
pub const MSG_MOVE_SET_FACING: u16 = 0x00DA; // 218
pub const MSG_MOVE_SET_PITCH: u16 = 0x00DB; // 219
pub const MSG_MOVE_HEARTBEAT: u16 = 0x00EE; // 238

pub const CMSG_GOSSIP_HELLO: u16 = 0x017B; // 379
pub const CMSG_GOSSIP_SELECT_OPTION: u16 = 0x017C; // 380
pub const SMSG_GOSSIP_MESSAGE: u16 = 0x017D; // 381
pub const SMSG_GOSSIP_COMPLETE: u16 = 0x017E; // 382
pub const CMSG_NPC_TEXT_QUERY: u16 = 0x017F; // 383
pub const SMSG_NPC_TEXT_UPDATE: u16 = 0x0180; // 384

// A guard's map marker, pushed unasked by a gossip option's `action_poi_id` (`GossipDef.cpp:253`).
pub const SMSG_GOSSIP_POI: u16 = 0x0224; // 548

pub const CMSG_QUESTGIVER_STATUS_QUERY: u16 = 0x0182; // 386
pub const SMSG_QUESTGIVER_STATUS: u16 = 0x0183; // 387
pub const CMSG_QUESTGIVER_HELLO: u16 = 0x0184; // 388
pub const SMSG_QUESTGIVER_QUEST_LIST: u16 = 0x0185; // 389
pub const CMSG_QUESTGIVER_QUERY_QUEST: u16 = 0x0186; // 390
pub const SMSG_QUESTGIVER_QUEST_DETAILS: u16 = 0x0188; // 392
pub const CMSG_QUESTGIVER_ACCEPT_QUEST: u16 = 0x0189; // 393
pub const CMSG_QUESTGIVER_COMPLETE_QUEST: u16 = 0x018A; // 394
pub const SMSG_QUESTGIVER_REQUEST_ITEMS: u16 = 0x018B; // 395
pub const CMSG_QUESTGIVER_REQUEST_REWARD: u16 = 0x018C; // 396
pub const SMSG_QUESTGIVER_OFFER_REWARD: u16 = 0x018D; // 397
pub const CMSG_QUESTGIVER_CHOOSE_REWARD: u16 = 0x018E; // 398
pub const SMSG_QUESTGIVER_QUEST_INVALID: u16 = 0x018F; // 399
pub const SMSG_QUESTGIVER_QUEST_COMPLETE: u16 = 0x0191; // 401
pub const SMSG_QUESTGIVER_QUEST_FAILED: u16 = 0x0192; // 402

pub const CMSG_QUESTLOG_SWAP_QUEST: u16 = 0x0193; // 403
pub const CMSG_QUESTLOG_REMOVE_QUEST: u16 = 0x0194; // 404
pub const SMSG_QUESTLOG_FULL: u16 = 0x0195; // 405
pub const SMSG_QUESTUPDATE_FAILED: u16 = 0x0196; // 406
pub const SMSG_QUESTUPDATE_FAILEDTIMER: u16 = 0x0197; // 407
pub const SMSG_QUESTUPDATE_COMPLETE: u16 = 0x0198; // 408
pub const SMSG_QUESTUPDATE_ADD_KILL: u16 = 0x0199; // 409
pub const SMSG_QUESTUPDATE_ADD_ITEM: u16 = 0x019A; // 410

// Quest sharing. `MSG_QUEST_PUSH_RESULT` runs both ways: the receiver sends its verdict, and the
// server relays every verdict, its own and the receiver's, to the sharer.
pub const CMSG_QUEST_CONFIRM_ACCEPT: u16 = 0x019B; // 411
pub const SMSG_QUEST_CONFIRM_ACCEPT: u16 = 0x019C; // 412
pub const CMSG_PUSHQUESTTOPARTY: u16 = 0x019D; // 413
pub const MSG_QUEST_PUSH_RESULT: u16 = 0x0276; // 630

pub const CMSG_LIST_INVENTORY: u16 = 0x019E; // 414
pub const SMSG_LIST_INVENTORY: u16 = 0x019F; // 415
pub const CMSG_SELL_ITEM: u16 = 0x01A0; // 416
pub const SMSG_SELL_ITEM: u16 = 0x01A1; // 417
pub const CMSG_BUY_ITEM: u16 = 0x01A2; // 418
/// Buy into a given bag slot (a vendor row dropped from the cursor); a click sends `CMSG_BUY_ITEM`.
pub const CMSG_BUY_ITEM_IN_SLOT: u16 = 0x01A3; // 419
pub const SMSG_BUY_ITEM: u16 = 0x01A4; // 420
pub const SMSG_BUY_FAILED: u16 = 0x01A5; // 421
/// Buy a sold item back (`BuybackItem`, `0x4fb950`); outside the 0x19E vendor run.
pub const CMSG_BUYBACK_ITEM: u16 = 0x0290; // 656
/// Repair one item by guid, or everything with guid 0, at a repair vendor.
pub const CMSG_REPAIR_ITEM: u16 = 0x02A8; // 680

// Taxi: every guid in these, both ways, is a plain u64, never packed (`ObjectGuid.cpp:174-186`).
pub const SMSG_SHOWTAXINODES: u16 = 0x01A9; // 425
pub const CMSG_TAXINODE_STATUS_QUERY: u16 = 0x01AA; // 426
pub const SMSG_TAXINODE_STATUS: u16 = 0x01AB; // 427
pub const CMSG_TAXIQUERYAVAILABLENODES: u16 = 0x01AC; // 428
pub const CMSG_ACTIVATETAXI: u16 = 0x01AD; // 429
pub const SMSG_ACTIVATETAXIREPLY: u16 = 0x01AE; // 430
pub const SMSG_NEW_TAXI_PATH: u16 = 0x01AF; // 431
/// A multi-hop flight: guid, total cost, node list; a single hop uses `CMSG_ACTIVATETAXI`.
pub const CMSG_ACTIVATETAXIEXPRESS: u16 = 0x0312; // 786

// Trainers, class and profession alike, open from `GOSSIP_OPTION_TRAINER`; there is no open verb.
pub const CMSG_TRAINER_LIST: u16 = 0x01B0; // 432
pub const SMSG_TRAINER_LIST: u16 = 0x01B1; // 433
pub const CMSG_TRAINER_BUY_SPELL: u16 = 0x01B2; // 434
pub const SMSG_TRAINER_BUY_SUCCEEDED: u16 = 0x01B3; // 435
pub const SMSG_TRAINER_BUY_FAILED: u16 = 0x01B4; // 436

// The innkeeper bind: `SMSG_BINDER_CONFIRM` only asks; Accept sends `CMSG_BINDER_ACTIVATE`, then
// spell 3286 binds and sends `SMSG_BINDPOINTUPDATE` and `SMSG_PLAYERBOUND` (`EffectBind`).
pub const CMSG_BINDER_ACTIVATE: u16 = 0x01B5; // 437
pub const SMSG_PLAYERBOUND: u16 = 0x0158; // 344
pub const SMSG_BINDER_CONFIRM: u16 = 0x02EB; // 747

// GM tickets: nothing is pushed at login, so the client asks `CMSG_GMTICKET_GETTICKET`. GM commands
// push GETTICKET and DELETETICKET answers unasked (`TicketCommands.cpp`), and a create or delete
// can go unanswered (`GMTicketHandler.cpp:73-113`), so nothing waits on a reply.
pub const CMSG_GMTICKET_CREATE: u16 = 0x0205; // 517
pub const SMSG_GMTICKET_CREATE: u16 = 0x0206; // 518
pub const CMSG_GMTICKET_UPDATETEXT: u16 = 0x0207; // 519
pub const SMSG_GMTICKET_UPDATETEXT: u16 = 0x0208; // 520
pub const CMSG_GMTICKET_GETTICKET: u16 = 0x0211; // 529
pub const SMSG_GMTICKET_GETTICKET: u16 = 0x0212; // 530
pub const CMSG_GMTICKET_DELETETICKET: u16 = 0x0217; // 535
pub const SMSG_GMTICKET_DELETETICKET: u16 = 0x0218; // 536
pub const CMSG_GMTICKET_SYSTEMSTATUS: u16 = 0x021A; // 538
pub const SMSG_GMTICKET_SYSTEMSTATUS: u16 = 0x021B; // 539

/// A ticket-state push: `u32` 1 updated, 2 closed, 3 survey offered; vmangos never sends it. The
/// reference re-asks for the ticket on 1 (`0x5e7932`). Deviation: 3 opens no survey window,
/// since vmangos can never offer one.
pub const SMSG_GM_TICKET_STATUS_UPDATE: u16 = 0x0328; // 808

// The bank: `CMSG_BANKER_ACTIVATE` opens a pure banker; `SMSG_SHOW_BANK` also arrives unasked
// from the `GOSSIP_OPTION_BANKER` option.
pub const CMSG_BANKER_ACTIVATE: u16 = 0x01B7; // 439
pub const SMSG_SHOW_BANK: u16 = 0x01B8; // 440
pub const CMSG_BUY_BANK_SLOT: u16 = 0x01B9; // 441
pub const SMSG_BUY_BANK_SLOT_RESULT: u16 = 0x01BA; // 442
pub const CMSG_AUTOSTORE_BANK_ITEM: u16 = 0x0282; // 642
pub const CMSG_AUTOBANK_ITEM: u16 = 0x0283; // 643

/// Forget a cached player name: `u64 guid`, the cache's only eviction for a player (it has no
/// TTL; `0x556ff0`). vmangos never sends it.
pub const SMSG_INVALIDATE_PLAYER: u16 = 0x031C; // 796

// The pet stable. `MSG_LIST_STABLED_PETS` runs both ways: the server sends the list to open the
// window, the client sends one guid to refresh. Mutations get only one `SMSG_STABLE_RESULT` byte.
pub const MSG_LIST_STABLED_PETS: u16 = 0x026F; // 623
pub const CMSG_STABLE_PET: u16 = 0x0270; // 624
pub const CMSG_UNSTABLE_PET: u16 = 0x0271; // 625
pub const CMSG_BUY_STABLE_SLOT: u16 = 0x0272; // 626
pub const SMSG_STABLE_RESULT: u16 = 0x0273; // 627
pub const CMSG_STABLE_SWAP_PET: u16 = 0x0275; // 629

/// Spend a talent point; the rank's `SMSG_LEARNED_SPELL` and `PLAYER_CHARACTER_POINTS1` answer it.
pub const CMSG_LEARN_TALENT: u16 = 0x0251; // 593

/// The respec, both ways: the trainer's gossip line (`Player.cpp:12406`) makes the server ask with
/// the trainer's guid and the cost; the `CONFIRM_TALENT_WIPE` Accept sends the latched guid back.
pub const MSG_TALENT_WIPE_CONFIRM: u16 = 0x02AA; // 682
/// The instance boot timer: `u32 delayMs`, `u32 reason`; boot START for a positive delay, STOP for
/// zero, and with reason 1 or 2 a zero delay also shows `ERR_RAID_GROUP_ONLY` or `_FULL`.
pub const SMSG_RAID_GROUP_ONLY: u16 = 0x0286; // 646
/// A battleground queue slot: `u32 slot`, `u32 mapId`, `u8 bracket`, `u32`, `u32 status`, then
/// one `u32` for status 2, two for 3; fires `UPDATE_BATTLEFIELD_STATUS`.
pub const SMSG_BATTLEFIELD_STATUS: u16 = 0x02D4; // 724
/// `AcceptBattlefieldPort(index, accept)`: `u32 mapId` (the slot's Map.dbc row), `u8 accept`.
pub const CMSG_BATTLEFIELD_PORT: u16 = 0x02D5; // 725
/// The battleground scoreboard, both ways: an empty request (throttled to 5000 ms); the reply is
/// `u8 ended`, `u8 winner` if ended, `u32 count`, then per row `u64 guid`, `u32` rank,
/// killingBlows, honorableKills, deaths, honorGained, statCount, and statCount `u32` (the client
/// keeps eight). Fires `UPDATE_BATTLEFIELD_SCORE` once every row's name resolves.
pub const MSG_PVP_LOG_DATA: u16 = 0x02E0; // 736
/// `LeaveBattlefield()`: `u32 mapId`, sent only once the scoreboard's ended byte has arrived.
pub const CMSG_LEAVE_BATTLEFIELD: u16 = 0x02E1; // 737
/// Battleground teammate positions, both ways: an empty request (5000 ms throttle, active slot
/// only); the reply is `u32 count`, count × `(u64 guid, f32 x, f32 y)`, `u8 hasCarrier`, and if
/// set one more such triple for the flag carrier. No event fires; the UI polls.
pub const MSG_BATTLEGROUND_PLAYER_POSITIONS: u16 = 0x02E9; // 745
/// `ShowBattlefieldList(index)`: a queued slot's `u32 mapId`, reopening the list without an NPC.
pub const CMSG_BATTLEFIELD_LIST: u16 = 0x023C; // 572
/// The battleground instance list: `u64 battlemaster` (0 away from an NPC), `u32 mapId`,
/// `u8 bracket`, `u32 count`, count × `u32 instanceId`. Fires `BATTLEFIELDS_SHOW`; the
/// battlemaster guid decides which join opcode `JoinBattlefield` sends.
pub const SMSG_BATTLEFIELD_LIST: u16 = 0x023D; // 573
/// `JoinBattlefield`, no battlemaster: `u32 mapId`, `u32 instanceId` (0 = any), `u8 asGroup`.
pub const CMSG_BATTLEFIELD_JOIN: u16 = 0x023E; // 574
/// `JoinBattlefield` at a battlemaster: `u64 guid`, then the body of [`CMSG_BATTLEFIELD_JOIN`].
pub const CMSG_BATTLEMASTER_JOIN: u16 = 0x02EE; // 750
/// The world-entry status request: empty; answered by one `SMSG_BATTLEFIELD_STATUS` per live slot.
pub const CMSG_BATTLEFIELD_STATUS: u16 = 0x02D3; // 723
/// A group join's verdict: `u32`, `0xFFFFFFFE` for deserters (message 439), a Map.dbc id joined
/// (440), anything else failure (441).
pub const SMSG_GROUP_JOINED_BATTLEGROUND: u16 = 0x02E8; // 744
/// A player joined the battleground: `u64 guid`, printed as message 444 once the name resolves.
pub const SMSG_BATTLEGROUND_PLAYER_JOINED: u16 = 0x02EC; // 748
/// A player left the battleground: `u64 guid`, message 445.
pub const SMSG_BATTLEGROUND_PLAYER_LEFT: u16 = 0x02ED; // 749
/// Sent on adopting a new area spirit healer: `u64 guid`; [`SMSG_AREA_SPIRIT_HEALER_TIME`] answers.
pub const CMSG_AREA_SPIRIT_HEALER_QUERY: u16 = 0x02E2; // 738
/// `AcceptAreaSpiritHeal()`: the cached area healer's `u64 guid`, never an argument.
pub const CMSG_AREA_SPIRIT_HEALER_QUEUE: u16 = 0x02E3; // 739
/// The healer's next resurrection: `u64 guid`, `u32 ms`. For the cached healer and a positive
/// time it arms the deadline and fires `AREA_SPIRIT_HEALER_IN_RANGE`.
pub const SMSG_AREA_SPIRIT_HEALER_TIME: u16 = 0x02E4; // 740
/// Right-click a meeting stone (GameObject type 23): `u64` guid, sent by `0x4c9ff0`. A stone never
/// sends [`CMSG_GAMEOBJ_USE`], which vmangos ignores for type 23 (`GameObject.cpp:1836`).
pub const CMSG_MEETINGSTONE_JOIN: u16 = 0x0292; // 658
/// `CancelMeetingStoneRequest()`: empty, party leader only; the server's `0x295` reply clears the
/// state. The name is the emulators'; the number is the client's.
pub const CMSG_MEETINGSTONE_LEAVE: u16 = 0x0293; // 659
/// The meeting-stone queue state: `u32 areaId`, `u8 status`; always stored, a message per
/// status, then `MEETINGSTONE_CHANGED`.
pub const SMSG_MEETINGSTONE_SETQUEUE: u16 = 0x0295; // 661
/// The world-entry meeting-stone status query: empty, once per session; answered by `0x295`.
/// vmangos names it `CMSG_MEETINGSTONE_INFO`.
pub const CMSG_MEETINGSTONE_STATUS_QUERY: u16 = 0x0296; // 662
/// Display only: empty; `ERR_MEETING_STONE_SUCCESS`.
pub const SMSG_MEETINGSTONE_SUCCESS: u16 = 0x0297; // 663
/// Display only: empty; `ERR_MEETING_STONE_IN_PROGRESS`.
pub const SMSG_MEETINGSTONE_IN_PROGRESS: u16 = 0x0298; // 664
/// Display only: `u64 guid`; `ERR_MEETING_STONE_MEMBER_ADDED_S` once the name resolves.
pub const SMSG_MEETINGSTONE_MEMBER_ADDED: u16 = 0x0299; // 665
/// Display only: `u8 code`, 1 must be leader, 2 group full, 3 no raid group, else nothing.
pub const SMSG_MEETINGSTONE_JOIN_FAILED: u16 = 0x02BB; // 699
/// `ConfirmPetUnlearn()`: the latched `u64 trainerGuid`, never an argument.
pub const CMSG_PET_UNLEARN: u16 = 0x02F0; // 752
/// The pet trainer asks: `u64 trainerGuid`, `u32 costCopper`, latched for
/// `CONFIRM_PET_UNLEARN`; a zero guid shows `ERR_TALENT_WIPE_ERROR` instead.
pub const SMSG_PET_UNLEARN_CONFIRM: u16 = 0x02F1; // 753

/// A summon offer (warlock, meeting stone, GM). The client can only accept, with
/// `CMSG_SUMMON_RESPONSE` (`0x48b770`); a decline is silence until the two-minute timeout.
pub const SMSG_SUMMON_REQUEST: u16 = 0x02AB; // 683
pub const CMSG_SUMMON_RESPONSE: u16 = 0x02AC; // 684

/// Abandon a skill line; no ack, the removal arrives as a `PLAYER_SKILL_INFO` update.
pub const CMSG_UNLEARN_SKILL: u16 = 0x0202; // 514

/// Toggle our PvP flag: we send the empty body, a toggle; one byte would set a state instead
/// (`TogglePvP`). No ack; the flag arrives in `UNIT_FIELD_FLAGS`, and turning it off waits out
/// the server's 300 s timer.
pub const CMSG_TOGGLE_PVP: u16 = 0x0253; // 595

/// Toggle `PLAYER_FLAGS_HIDE_HELM`: an empty body and a pure toggle, so send only when the held
/// flag differs (`CharacterHandler.cpp:753-761`). No ack; the public flag hides it for everyone.
pub const CMSG_TOGGLE_HELM: u16 = 0x02B9; // 697
/// [`CMSG_TOGGLE_HELM`] for `PLAYER_FLAGS_HIDE_CLOAK`.
pub const CMSG_TOGGLE_CLOAK: u16 = 0x02BA; // 698

pub const CMSG_AUTOSTORE_LOOT_ITEM: u16 = 0x0108; // 264
pub const CMSG_LOOT: u16 = 0x015D; // 349
pub const CMSG_LOOT_MONEY: u16 = 0x015E; // 350
pub const CMSG_LOOT_RELEASE: u16 = 0x015F; // 351
pub const SMSG_LOOT_RESPONSE: u16 = 0x0160; // 352
pub const SMSG_LOOT_RELEASE_RESPONSE: u16 = 0x0161; // 353
pub const SMSG_LOOT_REMOVED: u16 = 0x0162; // 354
pub const SMSG_LOOT_MONEY_NOTIFY: u16 = 0x0163; // 355
pub const SMSG_LOOT_CLEAR_MONEY: u16 = 0x0165; // 357
pub const SMSG_ITEM_PUSH_RESULT: u16 = 0x0166; // 358

// Need/Greed/Pass rolls: group or need-before-greed loot on a drop at or above the threshold.
pub const SMSG_LOOT_ALL_PASSED: u16 = 0x029E; // 670
pub const SMSG_LOOT_ROLL_WON: u16 = 0x029F; // 671
pub const CMSG_LOOT_ROLL: u16 = 0x02A0; // 672
pub const SMSG_LOOT_START_ROLL: u16 = 0x02A1; // 673
pub const SMSG_LOOT_ROLL: u16 = 0x02A2; // 674

// Master loot: no roll; the master looter gets the eligible members when the window opens.
pub const CMSG_LOOT_MASTER_GIVE: u16 = 0x02A3; // 675
pub const SMSG_LOOT_MASTER_LIST: u16 = 0x02A4; // 676

// Death: `CMSG_REPOP_REQUEST` and our `MSG_CORPSE_QUERY` request have empty bodies.
pub const CMSG_REPOP_REQUEST: u16 = 0x015A; // 346
pub const SMSG_RESURRECT_REQUEST: u16 = 0x015B; // 347
pub const CMSG_RESURRECT_RESPONSE: u16 = 0x015C; // 348
pub const CMSG_RECLAIM_CORPSE: u16 = 0x01D2; // 466
pub const MSG_CORPSE_QUERY: u16 = 0x0216; // 534
pub const CMSG_SPIRIT_HEALER_ACTIVATE: u16 = 0x021C; // 540
pub const SMSG_SPIRIT_HEALER_CONFIRM: u16 = 0x0222; // 546
pub const SMSG_CORPSE_RECLAIM_DELAY: u16 = 0x0269; // 617
/// `UseSoulstone()`: empty body; the server casts our `PLAYER_SELF_RES_SPELL` and zeroes it
/// (`SpellHandler.cpp:461`). No answer packet, only descriptor deltas.
pub const CMSG_SELF_RES: u16 = 0x02B3; // 691
/// A non-PvP death's 10% durability loss: empty body, a red error line (`Unit.cpp:1170-1182`).
pub const SMSG_DURABILITY_DAMAGE_DEATH: u16 = 0x02BD; // 701

// Mover modes, acked (`IsFlagAckOpcode`): the SMSG is a packed guid and `u32 counter`; the ack
// echoes the full u64 guid, the counter and a `MovementInfo`, plus a `u32 apply` for all but root
// (`Movement.cpp:38-59`). An unacked change never reaches observers.
pub const SMSG_MOVE_WATER_WALK: u16 = 0x00DE; // 222
pub const SMSG_MOVE_LAND_WALK: u16 = 0x00DF; // 223
pub const CMSG_MOVE_WATER_WALK_ACK: u16 = 0x02D0; // 720
pub const SMSG_FORCE_MOVE_ROOT: u16 = 0x00E8; // 232
pub const CMSG_FORCE_MOVE_ROOT_ACK: u16 = 0x00E9; // 233
pub const SMSG_FORCE_MOVE_UNROOT: u16 = 0x00EA; // 234
pub const CMSG_FORCE_MOVE_UNROOT_ACK: u16 = 0x00EB; // 235
pub const SMSG_MOVE_FEATHER_FALL: u16 = 0x00F2; // 242
pub const SMSG_MOVE_NORMAL_FALL: u16 = 0x00F3; // 243
pub const SMSG_MOVE_SET_HOVER: u16 = 0x00F4; // 244
pub const SMSG_MOVE_UNSET_HOVER: u16 = 0x00F5; // 245
pub const CMSG_MOVE_HOVER_ACK: u16 = 0x00F6; // 246
pub const CMSG_MOVE_FEATHER_FALL_ACK: u16 = 0x02CF; // 719

// The observers' leg of those modes and of teleports (`MovementPacketSender.h:30-60`): the relay
// shape `[packed guid][MovementInfo]`, handled like the relay opcodes (`0x603bb0`). Only root has
// two opcodes; apply is in the flags word. `MSG_MOVE_TELEPORT` has no counter and comes twice per
// near teleport, both with the destination; a creature blink arrives the same way.
pub const MSG_MOVE_TELEPORT: u16 = 0x00C5; // 197
pub const MSG_MOVE_ROOT: u16 = 0x00EC; // 236
pub const MSG_MOVE_UNROOT: u16 = 0x00ED; // 237
pub const MSG_MOVE_HOVER: u16 = 0x00F7; // 247
pub const MSG_MOVE_FEATHER_FALL: u16 = 0x02B0; // 688
pub const MSG_MOVE_WATER_WALK: u16 = 0x02B1; // 689

// Knockback. `SMSG_MOVE_KNOCK_BACK` to the mover: packed guid, `u32 counter`, `f32` vcos, vsin,
// speedXY, speedZ, with speedZ down-positive like the jump tail. The mandatory ack is full u64
// guid, counter, `MovementInfo`, whose jump tail must echo the four floats within 0.01 with
// `MOVEFLAG_JUMPING` set; a knockback is never re-sent (`Unit.cpp:6912`). Observers get
// `MSG_MOVE_KNOCK_BACK`: the relay shape plus the four floats, re-launched by `0x6026f0`.
pub const SMSG_MOVE_KNOCK_BACK: u16 = 0x00EF; // 239
pub const CMSG_MOVE_KNOCK_BACK_ACK: u16 = 0x00F0; // 240
pub const MSG_MOVE_KNOCK_BACK: u16 = 0x00F1; // 241

pub const CMSG_WHO: u16 = 0x0062; // 98
pub const SMSG_WHO: u16 = 0x0063; // 99
pub const CMSG_FRIEND_LIST: u16 = 0x0066; // 102
pub const SMSG_FRIEND_LIST: u16 = 0x0067; // 103
pub const SMSG_FRIEND_STATUS: u16 = 0x0068; // 104
pub const CMSG_ADD_FRIEND: u16 = 0x0069; // 105
/// Our LFG slots and comment, sent only from `0x4e8948`; nothing answers it. The name is the
/// emulators'.
pub const CMSG_SET_LOOKING_FOR_GROUP: u16 = 0x0200; // 512
pub const CMSG_DEL_FRIEND: u16 = 0x006A; // 106
pub const SMSG_IGNORE_LIST: u16 = 0x006B; // 107
pub const CMSG_ADD_IGNORE: u16 = 0x006C; // 108
pub const CMSG_DEL_IGNORE: u16 = 0x006D; // 109

pub const CMSG_GUILD_QUERY: u16 = 0x0054; // 84
pub const SMSG_GUILD_QUERY_RESPONSE: u16 = 0x0055; // 85
/// Never handled (`STATUS_NEVER`, `Opcodes.cpp:210`): 1.12 guilds are founded by petition.
pub const CMSG_GUILD_CREATE: u16 = 0x0081; // 129
pub const CMSG_GUILD_INVITE: u16 = 0x0082; // 130
pub const SMSG_GUILD_INVITE: u16 = 0x0083; // 131
pub const CMSG_GUILD_ACCEPT: u16 = 0x0084; // 132
pub const CMSG_GUILD_DECLINE: u16 = 0x0085; // 133
pub const SMSG_GUILD_DECLINE: u16 = 0x0086; // 134
pub const CMSG_GUILD_INFO: u16 = 0x0087; // 135
pub const SMSG_GUILD_INFO: u16 = 0x0088; // 136
pub const CMSG_GUILD_ROSTER: u16 = 0x0089; // 137
/// The roster, cut by the sender at `0x8000 - 4` bytes; one field is conditional (see
/// `super::guild::read_guild_roster`).
pub const SMSG_GUILD_ROSTER: u16 = 0x008A; // 138
pub const CMSG_GUILD_PROMOTE: u16 = 0x008B; // 139
pub const CMSG_GUILD_DEMOTE: u16 = 0x008C; // 140
pub const CMSG_GUILD_LEAVE: u16 = 0x008D; // 141
pub const CMSG_GUILD_REMOVE: u16 = 0x008E; // 142
pub const CMSG_GUILD_DISBAND: u16 = 0x008F; // 143
pub const CMSG_GUILD_LEADER: u16 = 0x0090; // 144
pub const CMSG_GUILD_MOTD: u16 = 0x0091; // 145
pub const SMSG_GUILD_EVENT: u16 = 0x0092; // 146
pub const SMSG_GUILD_COMMAND_RESULT: u16 = 0x0093; // 147
pub const CMSG_GUILD_RANK: u16 = 0x0231; // 561
pub const CMSG_GUILD_ADD_RANK: u16 = 0x0232; // 562
pub const CMSG_GUILD_DEL_RANK: u16 = 0x0233; // 563
pub const CMSG_GUILD_SET_PUBLIC_NOTE: u16 = 0x0234; // 564
pub const CMSG_GUILD_SET_OFFICER_NOTE: u16 = 0x0235; // 565
pub const CMSG_GUILD_INFO_TEXT: u16 = 0x02FC; // 764

pub const CMSG_PETITION_SHOWLIST: u16 = 0x01BB; // 443
pub const SMSG_PETITION_SHOWLIST: u16 = 0x01BC; // 444
pub const CMSG_PETITION_BUY: u16 = 0x01BD; // 445
pub const CMSG_PETITION_SHOW_SIGNATURES: u16 = 0x01BE; // 446
/// Answers our `CMSG_PETITION_SHOW_SIGNATURES` and another player's `CMSG_OFFER_PETITION` alike;
/// only `owner` tells them apart.
pub const SMSG_PETITION_SHOW_SIGNATURES: u16 = 0x01BF; // 447
pub const CMSG_PETITION_SIGN: u16 = 0x01C0; // 448
pub const SMSG_PETITION_SIGN_RESULTS: u16 = 0x01C1; // 449
/// Both ways, different bodies: we send the charter item's guid, the owner gets the decliner's.
pub const MSG_PETITION_DECLINE: u16 = 0x01C2; // 450
pub const CMSG_OFFER_PETITION: u16 = 0x01C3; // 451
pub const CMSG_TURN_IN_PETITION: u16 = 0x01C4; // 452
pub const SMSG_TURN_IN_PETITION_RESULTS: u16 = 0x01C5; // 453
pub const CMSG_PETITION_QUERY: u16 = 0x01C6; // 454
pub const SMSG_PETITION_QUERY_RESPONSE: u16 = 0x01C7; // 455
/// Both ways, one body: `u64 item` and the new name; echoed back only on success.
pub const MSG_PETITION_RENAME: u16 = 0x02C1; // 705

pub const CMSG_GROUP_INVITE: u16 = 0x006E; // 110
pub const SMSG_GROUP_INVITE: u16 = 0x006F; // 111
pub const CMSG_GROUP_ACCEPT: u16 = 0x0072; // 114
pub const CMSG_GROUP_DECLINE: u16 = 0x0073; // 115
pub const SMSG_GROUP_DECLINE: u16 = 0x0074; // 116
pub const CMSG_GROUP_UNINVITE: u16 = 0x0075; // 117
pub const CMSG_GROUP_UNINVITE_GUID: u16 = 0x0076; // 118
pub const SMSG_GROUP_UNINVITE: u16 = 0x0077; // 119
pub const CMSG_GROUP_SET_LEADER: u16 = 0x0078; // 120
pub const SMSG_GROUP_SET_LEADER: u16 = 0x0079; // 121
pub const CMSG_LOOT_METHOD: u16 = 0x007A; // 122
pub const CMSG_GROUP_DISBAND: u16 = 0x007B; // 123
pub const SMSG_GROUP_DESTROYED: u16 = 0x007C; // 124
pub const SMSG_GROUP_LIST: u16 = 0x007D; // 125
pub const SMSG_PARTY_MEMBER_STATS: u16 = 0x007E; // 126
pub const SMSG_PARTY_COMMAND_RESULT: u16 = 0x007F; // 127
/// Both ways: our ping has no guid; the server adds ours and relays it (`GroupHandler.cpp:382`).
pub const MSG_MINIMAP_PING: u16 = 0x01D5; // 469
pub const CMSG_GROUP_CHANGE_SUB_GROUP: u16 = 0x027E; // 638
pub const CMSG_REQUEST_PARTY_MEMBER_STATS: u16 = 0x027F; // 639
pub const CMSG_GROUP_SWAP_SUB_GROUP: u16 = 0x0280; // 640
pub const CMSG_GROUP_RAID_CONVERT: u16 = 0x028E; // 654
pub const CMSG_GROUP_ASSISTANT_LEADER: u16 = 0x028F; // 655
pub const SMSG_PARTY_MEMBER_STATS_FULL: u16 = 0x02F2; // 754
/// Both ways; the server's side is mode-prefixed (`Group.cpp:77-82` read, `:132-147` write).
pub const MSG_RAID_TARGET_UPDATE: u16 = 0x0321; // 801
/// Both ways: an empty body starts a check, a non-empty one answers (`Group.cpp:84-96`).
pub const MSG_RAID_READY_CHECK: u16 = 0x0322; // 802
/// Ask for our raid lockouts: empty body, sent by `RequestRaidInfo()` on every RaidFrame show.
pub const CMSG_REQUEST_RAID_INFO: u16 = 0x02CD; // 717
/// Our permanent binds: `u32 count`, count × `{u32 mapId, u32 secondsUntilReset, u32 instanceId}`.
pub const SMSG_RAID_INSTANCE_INFO: u16 = 0x02CC; // 716

// Duels (reference handlers registered at `0x4d4710`). A challenge is a `CMSG_CAST_SPELL` of the
// `SPELL_EFFECT_DUEL` (83) spell; there is no opcode to start one.
pub const SMSG_DUEL_REQUESTED: u16 = 0x0167; // 359
pub const SMSG_DUEL_OUTOFBOUNDS: u16 = 0x0168; // 360
pub const SMSG_DUEL_INBOUNDS: u16 = 0x0169; // 361
pub const SMSG_DUEL_COMPLETE: u16 = 0x016A; // 362
pub const SMSG_DUEL_WINNER: u16 = 0x016B; // 363
pub const CMSG_DUEL_ACCEPTED: u16 = 0x016C; // 364
pub const CMSG_DUEL_CANCELLED: u16 = 0x016D; // 365
/// The duel countdown in milliseconds; the client divides by 1000 (`0x4d4aef`). Registered with
/// the 0x167 block (`0x4d474a`) despite its number.
pub const SMSG_DUEL_COUNTDOWN: u16 = 0x02B7; // 695

// Mirror timers (breath, fatigue, feign death): server countdowns; any change re-sends a START.
pub const SMSG_START_MIRROR_TIMER: u16 = 0x01D9; // 473
/// Never sent by vmangos, which re-sends a START instead: the stock `MirrorTimer.lua` errors on
/// a real pause (`Player::SendMirrorTimers`).
pub const SMSG_PAUSE_MIRROR_TIMER: u16 = 0x01DA; // 474
pub const SMSG_STOP_MIRROR_TIMER: u16 = 0x01DB; // 475

// Mail opens client-side; every CMSG leads with the mailbox guid, checked at 5 yd (`CheckMailBox`).
pub const CMSG_SEND_MAIL: u16 = 0x0238; // 568
pub const SMSG_SEND_MAIL_RESULT: u16 = 0x0239; // 569
pub const CMSG_GET_MAIL_LIST: u16 = 0x023A; // 570
pub const SMSG_MAIL_LIST_RESULT: u16 = 0x023B; // 571
/// A letter's text, fetched ask-once by `itemTextId`; the mail list never carries it.
pub const CMSG_ITEM_TEXT_QUERY: u16 = 0x0243; // 579
pub const SMSG_ITEM_TEXT_QUERY_RESPONSE: u16 = 0x0244; // 580
pub const CMSG_MAIL_TAKE_MONEY: u16 = 0x0245; // 581
pub const CMSG_MAIL_TAKE_ITEM: u16 = 0x0246; // 582
/// No response; the client sets the letter's read bit itself.
pub const CMSG_MAIL_MARK_AS_READ: u16 = 0x0247; // 583
pub const CMSG_MAIL_RETURN_TO_SENDER: u16 = 0x0248; // 584
pub const CMSG_MAIL_DELETE: u16 = 0x0249; // 585
pub const CMSG_MAIL_CREATE_TEXT_ITEM: u16 = 0x024A; // 586
/// Both ways: an empty request; the reply is one `f32`, `0.0` for unread mail, `-86400.0` none.
pub const MSG_QUERY_NEXT_MAIL_TIME: u16 = 0x0284; // 644
/// A mail arrived (text-only at once, else when its delivery timer ends): one `u32`, always 0.
pub const SMSG_RECEIVED_MAIL: u16 = 0x0285; // 645

// The auction house: every CMSG leads with the auctioneer guid, and the server checks 5 yd on
// each, so the hello is no session token.
/// Both ways: we send the auctioneer guid; the reply echoes it with `u32 houseId`
/// (`AuctionHouse.dbc` 1..7) and opens the window.
pub const MSG_AUCTION_HELLO: u16 = 0x0255; // 597
pub const CMSG_AUCTION_SELL_ITEM: u16 = 0x0256; // 598
pub const CMSG_AUCTION_REMOVE_ITEM: u16 = 0x0257; // 599
/// The Browse search: ten fields and no sort bytes; the server filters, the client sorts.
pub const CMSG_AUCTION_LIST_ITEMS: u16 = 0x0258; // 600
pub const CMSG_AUCTION_LIST_OWNER_ITEMS: u16 = 0x0259; // 601
/// Bid or buy out: a buyout is `price >= buyout && buyout != 0`, never flagged.
pub const CMSG_AUCTION_PLACE_BID: u16 = 0x025A; // 602
/// The sell, cancel or bid verdict, with a tail keyed on its error; some refusals send nothing.
pub const SMSG_AUCTION_COMMAND_RESULT: u16 = 0x025B; // 603
/// Shared with the owner and bidder list results: `u32 count`, 64-byte records, then
/// `u32 totalCount`, the matches before the 50-row page cap.
pub const SMSG_AUCTION_LIST_RESULT: u16 = 0x025C; // 604
pub const SMSG_AUCTION_OWNER_LIST_RESULT: u16 = 0x025D; // 605
/// To the bidder, won or outbid: `bidOrZero == 0` means won, not "no bid".
pub const SMSG_AUCTION_BIDDER_NOTIFICATION: u16 = 0x025E; // 606
/// To the seller: a different field order from the bidder notification, and no `houseId`.
pub const SMSG_AUCTION_OWNER_NOTIFICATION: u16 = 0x025F; // 607
pub const CMSG_AUCTION_LIST_BIDDER_ITEMS: u16 = 0x0264; // 612
pub const SMSG_AUCTION_BIDDER_LIST_RESULT: u16 = 0x0265; // 613
/// Pushed to a bidder whose auction the seller cancelled.
pub const SMSG_AUCTION_REMOVED_NOTIFICATION: u16 = 0x028D; // 653

// The world-state table; these two are its only writers (reference `0x48f690`). NPC text's
// `$<n>w`/`$<n>e` tokens read it.
pub const SMSG_INIT_WORLD_STATES: u16 = 0x02C2; // 706
pub const SMSG_UPDATE_WORLD_STATE: u16 = 0x02C3; // 707

// ── Instance and raid lockouts ──

/// "You are now saved to this instance": a `u32` flag, 0 from vmangos (`0x4e7e60`; 1 adds a debug
/// prefix). Deviation: 2 or more prints nothing; the reference prints an uninitialized buffer.
pub const SMSG_INSTANCE_SAVE_CREATED: u16 = 0x02CB; // 715
/// A raid-lockout warning: `u32 type`, `u32 mapId`, `u32 secondsUntilReset` (`0x49e1c0`).
pub const SMSG_RAID_INSTANCE_MESSAGE: u16 = 0x02FA; // 762
/// "Reset all instances": empty body (`ResetInstances`, `0x48a6b0`).
pub const CMSG_RESET_INSTANCES: u16 = 0x031D; // 797
/// "%s has been reset.": `u32 mapId`; `0x49e470` clears the last-instance latch before reading it.
pub const SMSG_INSTANCE_RESET: u16 = 0x031E; // 798
/// The reset refusal: `u32 reason`, `u32 mapId` (`0x49e540`).
pub const SMSG_INSTANCE_RESET_FAILED: u16 = 0x031F; // 799
/// The dungeon we were last in: `u32 mapId` (`0x49e670`), half of what
/// `CanShowResetInstances()` reads; it shows no line.
pub const SMSG_UPDATE_LAST_INSTANCE: u16 = 0x0320; // 800
/// Whether we hold any permanent bind: `u32` flag (`0x49e6c0`), half of `CanShowResetInstances()`.
pub const SMSG_UPDATE_INSTANCE_OWNERSHIP: u16 = 0x032B; // 811
