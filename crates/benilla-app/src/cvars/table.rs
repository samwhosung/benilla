//! The registry's table: every host-backed CVar, its default, and that default's standing against
//! the reference's registered one ([`Reference`]). The runtime that holds the live values is the
//! parent module.

/// One host-backed CVar: its name, benilla's default, and where that default stands against the
/// reference's ([`Reference`]).
pub(crate) struct Registered {
    /// The registered name, in the reference's own spelling.
    pub(crate) name: &'static str,
    /// What a fresh `benilla-config` runs at, and what `GetCVar` answers until the player moves it.
    pub(crate) default: &'static str,
    /// Where `default` stands against the reference's. Read only by the tests: it records the
    /// reference, never a value this client acts on.
    #[allow(dead_code)]
    pub(crate) reference: Reference,
    /// Registered with flag bit1 (`rec+0x1c & 0x2`, `flags` 2 or 3 at the register site): a write
    /// is staged in [`super::Row::pending`] until [`super::Cvars::commit_latched`].
    pub(crate) latched: bool,
}

impl Registered {
    /// Mark the row latched.
    pub(crate) const fn latched(self) -> Self {
        Self {
            latched: true,
            ..self
        }
    }
}

/// benilla's default against the reference's: the string the reference's `CVar::Register`
/// (`0x63db90`) passes for the name, or, for a setting 1.12 keeps in FrameXML, the value
/// `UIOptionsFrame.lua` boots it at. Not a `Config.wtf` line (`SaveConfig 0x63d980` writes only
/// values off their default), and not always what a fresh install runs at: `hwDetect` rewrites
/// sixteen video CVars from `VideoHardware.dbc` before the first frame ([`Reference::Overridden`]).
/// Each row's register site or FrameXML line is cited above it.
#[allow(dead_code)] // read only by the tests
pub(crate) enum Reference {
    /// The reference registers this default and benilla ships it; the test compares the two.
    Same(&'static str),
    /// The reference registers `registered`, but its own boot code overwrites it before the first
    /// frame; `default` is where that lands, and `why` is the override.
    Overridden {
        registered: &'static str,
        why: &'static str,
    },
    /// The reference ships `value` and benilla ships another; `why` is the reason.
    Deviates {
        value: &'static str,
        why: &'static str,
    },
    /// No reference setting to match: benilla's own knob, or a later-era name for something 1.12
    /// never made settable. `why` says which, and what the reference does instead.
    Ours(&'static str),
}

/// A row whose default is the reference's own registered string.
const fn same(name: &'static str, default: &'static str) -> Registered {
    Registered {
        name,
        default,
        reference: Reference::Same(default),
        latched: false,
    }
}

/// A row whose default follows the reference's boot-time override of its registered string.
const fn overridden(
    name: &'static str,
    default: &'static str,
    registered: &'static str,
    why: &'static str,
) -> Registered {
    Registered {
        name,
        default,
        reference: Reference::Overridden { registered, why },
        latched: false,
    }
}

/// A row that ships something other than the reference's `value`, for `why`.
const fn deviates(
    name: &'static str,
    default: &'static str,
    value: &'static str,
    why: &'static str,
) -> Registered {
    Registered {
        name,
        default,
        reference: Reference::Deviates { value, why },
        latched: false,
    }
}

/// A row the reference has no counterpart for.
const fn ours(name: &'static str, default: &'static str, why: &'static str) -> Registered {
    Registered {
        name,
        default,
        reference: Reference::Ours(why),
        latched: false,
    }
}

/// The table as the script VM's registrar wants it: `(name, default)` pairs in table order.
pub(crate) fn registered_pairs() -> impl Iterator<Item = (&'static str, &'static str)> {
    REGISTERED.iter().map(|r| (r.name, r.default))
}

/// The host-backed CVars, one row per knob that has a reader: a host knob, or a Lua consumer
/// such as the stock Video Options window's verbs.
///
/// Not registered for want of a reader, though pfUI's `hdgraphic` writes them: `lodDist`
/// (`0x688524`, "100.0", read at `0x6afb1d` for the doodad LOD swap), `footstepBias` (`0x6888b4`,
/// "0.125", read at `0x68fcb6`), `mapObjLightLOD` (`0x6886ec`, "0") and `SkyCloudLOD` (`0x6d1d33`,
/// "0"). `DistCull` (`0x688570`) has no reader in the reference either. `maxLOD` is no 1.12 CVar.
pub(crate) const REGISTERED: &[Registered] = &[
    // `realmName` (`0x83f2d0`): registered `""` (`0x882748`), help "Last realm connected to"
    // (`0x85d684`); the client builds its SavedVariables path from it (`0x5ab7d0`). Written from
    // the session's realm when addons load. `Ace/AceState.lua:27` trims it at
    // PLAYER_ENTERING_WORLD, so a nil breaks every Ace addon.
    same("realmName", ""),
    // The logon server address (register site `0x5ab6a6`), a string row judged by
    // `realmlist::on_cvar`.
    deviates(
        crate::realmlist::CVAR_REALMLIST,
        crate::realmlist::DEFAULT_REALMLIST,
        "us.logon.worldofwarcraft.com:3724",
        "that host has not resolved since 2019, so shipping it makes every first launch a \
         DNS failure; benilla dials the machine it is running on",
    ),
    // `autoClearAFK` (`0x5e24d4`, "1" `0x82e748`, handle `[0xc4d68c]` set at `0x5e24ef`, read at
    // `0x5eb84b`) gates five implicit AFK clears: any chat send but type `0x14`, Jump,
    // forward/back, strafe and turn (`0x513d36`/`0x514e23`/`0x514f0b`/`0x514fca`). Off, the clear
    // does nothing at all: no echo, no mirror write, no packet.
    same("autoClearAFK", "1"),
    same("MasterVolume", "1"),
    same("SoundVolume", "1"),
    same("MusicVolume", "0.4"),
    same("AmbienceVolume", "0.6"),
    // Registered "1" at `0x45737a`/`0x45739b`/`0x460a9d`. `MasterSoundEffects` is the Enable All
    // Sound box (`SoundOptionsFrame.lua:6`), which pauses the whole sound engine, not an SFX
    // toggle.
    same("MasterSoundEffects", "1"),
    same("EnableMusic", "1"),
    same("EnableAmbience", "1"),
    // The race/sex refusal voice lines (`0x457877`), `SoundOptionsFrame.lua:3`; the master enable
    // greys it.
    same("EnableErrorSpeech", "1"),
    // Not a 1.12 CVar or checkbox; the later-era name. The reference mutes on losing focus, music
    // included (`WM_ACTIVATE` to `0x7a4860`'s `FSOUND_SetMute(-3, active ? 0 : 1)`), so "0" is its
    // behaviour. The knob is `SoundConfig::background_sound`.
    same("Sound_EnableSoundWhenGameIsInBG", "0"),
    // Registered "1" (`0x4573be`); `SoundConfig::reverb` carries the evidence for shipping "0".
    deviates(
        "SoundReverb",
        "0",
        "1",
        "the reference's reverb is EAX-over-hardware, and that hardware has not existed since \
         Vista, so \"1\" would ship audio the real client has never actually produced on any \
         machine a player runs today",
    ),
    // FMOD 3's mix-ahead buffer in ms (`0x4571ca`, flags 2: read once at sound init); here it sizes
    // the render thread's ring ahead of the IO callback (`sound::output`). `0x457520` registers
    // "50" or "100" by a host probe (`0x835e10`/`0x835e0c`), which one on a current machine
    // untraced; ours is the larger, since the depth has to hide a whole stalled IO cycle.
    same("SoundBufferSize", "100").latched(),
    // benilla's own, not a 1.12 CVar. Deviation: the reference clips at full scale and has no
    // headroom mechanism (its SFX-bus duck `0x457960` is a sidechain armed only by server-pushed
    // voice lines); benilla limits because every SFX is mastered to full scale and overlapping
    // kits clip.
    ours(
        "SoundOutputLimiter",
        "1",
        "benilla's own — the reference sums at full scale and clips; every WoW SFX is mastered \
         to full scale, so overlapping kits need a limiter to keep from distorting",
    ),
    overridden(
        "uiScale",
        "0.9",
        "1.0",
        "a fresh reference client never consults this CVar: `useUiScale` registers \"0\" \
         (`0x48fce4`), and the OFF leg `0x492f70` computes clamp(768/height, 0.9, 1.0) instead — \
         0.9 at 854 px tall and up, which is every window we ship against. It is 1.0 at 768 and \
         below, where our flat 0.9 does diverge; `ui_script::DEFAULT_UI_SCALE` carries that. \
         See `useUiScale` below, whose row this one used to say did not exist",
    ),
    // `useUiScale` (`0x8430c0`), the switch the `uiScale` override gates on:
    // `ContainerFrame.lua:483` and `UIDropDownMenu.lua:525` branch on it and `OptionsFrame.lua:13`
    // gives it a checkbox.
    same("useUiScale", "0"),
    same("farclip", "350"),
    // `nearclip` (`0x68867a`: name `0x84ffb0`, default `0x84fb48` "0.1", flags 1, callback
    // `0x688d90`, record `[0xc7f348]`). The camera re-reads the record every frame: `0x511bc0`
    // (sole caller `0x483094`) stamps `[cam+0x38]` from it through the handle `[0xbe1078]` that
    // `0x50b728` caches, overwriting the ctor's 1/9 (`0x3de38e39`);
    // `benilla_world::view::stamp_near_clip` is that.
    // The callback's derived global `[0xc7b480]` has no reader. pfUI's `hdgraphic` writes
    // 0.06..0.30, inside `[0.01, 0.33]`.
    same("nearclip", "0.1"),
    // `deselectOnClick` and `mouseInvertPitch` are 1.12's own (`UIOptionsFrame.lua:8,4`);
    // `autoLootDefault` is the later-era name, 1.12 having only the shift gesture.
    same("deselectOnClick", "1"),
    // `BlockTrades` (`0x842fbc`), `UIOptionsFrame.lua:11`: the refusal leg `0x4bf7bc` fires only
    // when it is set. The knob is [`crate::ui_trade::BlockTrades`].
    same("BlockTrades", "0"),
    // `autoSelfCast` (register site `0x6e731d`, record `[0xceac34]`, read at `0x6e53d7`; `0x870dc0`
    // is its name string): a friendly cast that binds nothing falls back to the caster.
    // `TOGGLEAUTOSELFCAST` toggles it.
    same("autoSelfCast", "0"),
    // The five saved camera views and the live index, at the reference's names and default strings;
    // owned by [`crate::player::camera_view`]. Registered so a `SaveView` persists.
    same(
        crate::player::camera_view::CVAR_ACTIVE_VIEW,
        crate::player::camera_view::ACTIVE_VIEW_DEFAULT,
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[0][0],
        crate::player::camera_view::VIEW_DEFAULTS[0][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[0][1],
        crate::player::camera_view::VIEW_DEFAULTS[0][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[0][2],
        crate::player::camera_view::VIEW_DEFAULTS[0][2],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[1][0],
        crate::player::camera_view::VIEW_DEFAULTS[1][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[1][1],
        crate::player::camera_view::VIEW_DEFAULTS[1][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[1][2],
        crate::player::camera_view::VIEW_DEFAULTS[1][2],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[2][0],
        crate::player::camera_view::VIEW_DEFAULTS[2][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[2][1],
        crate::player::camera_view::VIEW_DEFAULTS[2][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[2][2],
        crate::player::camera_view::VIEW_DEFAULTS[2][2],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[3][0],
        crate::player::camera_view::VIEW_DEFAULTS[3][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[3][1],
        crate::player::camera_view::VIEW_DEFAULTS[3][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[3][2],
        crate::player::camera_view::VIEW_DEFAULTS[3][2],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[4][0],
        crate::player::camera_view::VIEW_DEFAULTS[4][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[4][1],
        crate::player::camera_view::VIEW_DEFAULTS[4][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[4][2],
        crate::player::camera_view::VIEW_DEFAULTS[4][2],
    ),
    same("mouseInvertPitch", "0"),
    ours(
        "autoLootDefault",
        "0",
        "1.12 has no auto-loot CVar at all — vanilla offers only the shift gesture, so OFF \
         IS the reference's own behaviour; the spelling is era's",
    ),
    // The overhead-name gates, registered at `0x6c7470` into mask `0xce8720`: `UnitNamePlayer`
    // (`0x86c694`) "1" (`0x82e748`), `UnitNameNPC` (`0x86c6a4`) and `UnitNameOwn` (`0x86c6b0`) "0"
    // (`0x82e570`).
    same("UnitNamePlayer", "1"),
    same("UnitNameNPC", "0"),
    same("UnitNameOwn", "0"),
    // `UnitNamePlayerGuild` (`0x86c680`, "1", mask bit `0x10`) is not a show gate: `ShouldShowName`
    // (`0x6070a0`) reads bits `0x1/0x2/0x4`, and this gates the `"\n<%s>"` guild line at
    // `0x609085`. `UnitNamePlayerPVPTitle` (bit `0x20`, "1") has no row: nothing here draws the
    // rank prefix.
    same("UnitNamePlayerGuild", "1"),
    // `WorldDetail` is no 1.12 CVar but the `GetWorldDetail`/`SetWorldDetail` verb name
    // (`OptionsFrame.lua:27`); stops 0/1/2 are `frillDensity` 16/32/48. `SetWorldDetail 0x488dd0`
    // also writes `SmallCull` {0.07, 0.04, 0.01}, and `GetWorldDetail` reads only `SmallCull`,
    // whose registered 0.04 is stop 1: the reference's slider boots at Medium.
    same("WorldDetail", "1"),
    // `SmallCull` (`0x68854a`: name `0x8696f8`, default `0x869718` "0.04", flags 1, callback
    // `0x688b10`, record `[0xc7f330]`). The callback refuses outside [0.001, 2.0] and stores into
    // `[0x868620]`, which nothing reads: no mechanism here either. Its readers are Lua ones:
    // `SetWorldDetail` writes it per stop, and `OptionsFrame_SetDefaults` reads its default
    // (`OptionsFrame.lua:431`). `hwDetect` sets 0.04 on the fallback row a modern GPU reaches.
    same(benilla_ui::script::CVAR_SMALL_CULL, "0.04"),
    // `frillDensity` (`0x68862e`: name `0x8423d8`, default `0x864644` "16", flags 1, callback
    // `0x688de0`, record `[0xc7f2f4]`): detail-doodad cells visited per chunk, clamped to [1, 256]
    // and handed through `0x6725a0` to `[0xc7b494]`, the bound of the scatter loop at
    // `0x6bfcfb`/`0x6bff1c`. One knob with `WorldDetail`, each keeping its own clamp. `hwDetect`
    // (`0x639a60`) sets it from `VideoHardware.dbc` field `+0x18`, 24 on videoID 170. pfUI's
    // `hdgraphic` reads `GetCVar("frillDensity") > 48` and writes up to 256.
    deviates(
        "frillDensity",
        "32",
        "16",
        "the reference's registered 16 is stop 0 and its post-`hwDetect` 24 is on no \
         stop at all, so every stop diverges; Medium (32) is the nearest one no sparser than a \
         fresh install, and erring sparse is the worse failure for ground cover",
    ),
    // ── Combat log display ranges, in yards ──────────────────────────────────────────────────────
    //
    // Registered by `0x626d00` from the `{name, default}` pairs at `0x8629e0`, read as the record's
    // float (`+0x24`). Classes 0 and 1, you and your pet, have no CVar there, only the `100000.0`
    // sentinel.
    same("CombatLogRangeParty", "50"),
    same("CombatLogRangePartyPet", "50"),
    same("CombatLogRangeFriendlyPlayers", "50"),
    same("CombatLogRangeFriendlyPlayersPets", "50"),
    same("CombatLogRangeHostilePlayers", "50"),
    same("CombatLogRangeHostilePlayersPets", "50"),
    same("CombatLogRangeCreature", "30"),
    // Outside that table (`0x626d5f`, "60" at `0x862e14`): `0x62c160` reads it first and falls back
    // to the per-class range only when the lookup fails.
    same(crate::ui_chat::combat::DEATH_LOG_RANGE_CVAR, "60"),
    // ── Floating combat text ─────────────────────────────────────────────────────────────────────
    //
    // `CombatDamage` (`0x6032df`, record `[0xc4d944]`) is the master: its two readers, the word
    // emitter `0x607140` and the number emitter `0x6128b0`, both return at "0", so nothing floats.
    // The `Pet*` rows gate only the owned-by-you branch; the stock Pet Melee Damage box writes both
    // (`UIOptionsFrame.lua:335-336`).
    same("CombatDamage", "1"),
    same("PetMeleeDamage", "1"),
    same("PetSpellDamage", "1"),
    // `CombatLogPeriodicSpells` (`0x6033b3`): the handle is discarded and every use looks it up by
    // name; read as the record's int.
    same(crate::ui_chat::combat::LOG_PERIODIC_CVAR, "1"),
    // ── Sound-panel check buttons ────────────────────────────────────────────────────────────────
    //
    // Category-7 registrations that keep no handle: the reference looks each up by name at use.
    //
    // `SoundListenerAtCharacter` (`0x457890`): the listener at the character, else at the camera
    // (`update_audio_listener`).
    same("SoundListenerAtCharacter", "1"),
    // `EmoteSounds` (`0x4573b9`): the received text-emote voice kit, and only that.
    same("EmoteSounds", "1"),
    // `SoundZoneMusicNoDelay` (`0x4578b3`): `next_track_time`'s immediate path.
    same("SoundZoneMusicNoDelay", "0"),
    // `assistAttack` (`0x48fc50`, record `[0xb4d8f8]`): `/assist` also starts the swing. The "3"
    // beside it is the next registration's, `minimapZoom`: stock `/assist` selects without
    // swinging.
    same("assistAttack", "0"),
    // ── Mouse-look speed, per axis ───────────────────────────────────────────────────────────────
    //
    // `cameraYawMoveSpeed` is the MOUSE_LOOK_SPEED slider (`UIOptionsFrame.lua:89`); a nil there
    // raises in `slider:SetValue` (`0x790980`) and stops `UIOptionsFrame_Load`. The stock Save
    // writes `cameraPitchMoveSpeed` as half of it (`:355-356`). The reference integrates
    // OS-accelerated pixels where we take raw device deltas, so the unit factor lives in
    // `camera::LOOK_YAW_PER_SPEED` and these defaults stay the reference's. The validator
    // `0x50c000` → `0x50b330` rejects values outside [0.1, 360] rather than clamping, and
    // `player::camera::on_cvar` does the same.
    same("cameraYawMoveSpeed", "180"),
    same("cameraPitchMoveSpeed", "90"),
    // MOUSE_SENSITIVITY (`UIOptionsFrame.lua:87`): FrameXML's spelling of the binary's `mouseSpeed`
    // (`0x402c7b`); lookups are case-insensitive (`CVar::Lookup 0x63de30`), here as there. The
    // reference's default is `SPI_GETMOUSESPEED × 0.1`, "1.0" on stock Windows, and its record
    // `[0x882704]` has no reader: the slider sets the OS pointer speed (`0x402ec0`, [0.1, 2.0]).
    // The default matches; Deviation: here the dial multiplies the camera's own rate, because
    // benilla does not change the OS pointer speed, a system-wide setting.
    same("mousespeed", "1"),
    // The zoom-out limit is `cameraDistanceMax × cameraDistanceMaxFactor`, held to [0, 50]
    // (`0x5112d6`). `cameraDistanceMax` (`0x50beb2`, default `0x84fbd0`) has no panel row; its
    // validator `0x50b310` refuses a value outside [0, 50], and `player::camera::on_cvar` does the
    // same. The factor is MAX_FOLLOW_DIST (`UIOptionsFrame.lua:90`), registered "1.0" (`0x82e92c`)
    // with no validator: the slider's 1 to 2 is the slider's, not the CVar's.
    same("cameraDistanceMax", "15.0"),
    same("cameraDistanceMaxFactor", "1"),
    // `cameraSmoothStyle` (`0x50ba92`, default `[0x84f4f4]` "1"), the auto-return behind the
    // character. The engine's enum is 0 Never, 1 Smart, 2 Always, as the stock dropdown writes it
    // (`UIOptionsFrame.lua:525,536,547`); 3, the Never entry's position, is the validator's upper
    // bound (`0x50b330(v, 0, 3)`). See `FollowStyle`.
    same("cameraSmoothStyle", "1"),
    // Read instead of `cameraSmoothStyle` while the state mask holds Track or Fear; no panel row.
    same("cameraSmoothTrackingStyle", "1"),
    // AUTO_FOLLOW_SPEED (`UIOptionsFrame.lua:88`), deg/s, registered "180.0" (`[0xbe1070]`): it
    // sets the transition's duration (`|dyaw| / rate * factor`), an average rate, not a slew. The
    // knob clamps it to `FOLLOW_SPEED_RANGE`. The stock Save also writes `cameraPitchSmoothSpeed`
    // at a quarter (`:353`), unregistered here: `FollowRig` has one rate.
    same("cameraYawSmoothSpeed", "180"),
    // `cameraPivot` `[0xbe10a4]` "1" (`0x50bda3`), smart pivot: gate `0x510690`, routing
    // `0x50fee0`, release `0x5107f0`; ours is `player::camera_dynamics::SmartPivot`.
    same("cameraPivot", "1"),
    // Read by the routing (`0x50fff5`/`0x510004`) in radians of camera rotation, so they carry to
    // benilla's raw-device units unchanged.
    same("cameraPivotDXMax", "0.05"),
    same("cameraPivotDYMin", "0"),
    // The pitch bias's ease-back once the pivot lets go, deg/s (`[0xbe0fc8]`; `0x512a50` divides
    // |Δ| by rate · π/180 for the duration); no panel row.
    same("cameraTargetSmoothSpeed", "90"),
    // `cameraWaterCollision` `[0xbe1088]` "1" (`0x50bd63`, `0x82e748`): one register, two consumers
    // that must ship together. `0x50e5ec` builds it; its `0xf0000` nibble joins the trace mask of
    // all three `0x50e570` queries (reaching `0x69cc13`), and `0x50e629` tests it to lift the sweep
    // origin to `surface + 2/9`. Ours: `benilla_world::collision::camera_filter` and
    // `player::camera_water`.
    same("cameraWaterCollision", "1"),
    // `cameraTerrainTilt` `[0xbe0fd4]` "0" (`0x50bcfd`), Follow Terrain: probe and staircase
    // `0x50d900`, arm `0x50dbc0`; ours is `player::camera_dynamics::TerrainTilt`.
    same("cameraTerrainTilt", "0"),
    // Rate in deg/s (`[0xbe0fc0]`), duration bounds in seconds (`[0xbe1050]`/`[0xbe1054]`). The 3 s
    // floor always binds (20° at 7.5°/s is 2.67 s), so the camera leans rather than tracks.
    same("cameraGroundSmoothSpeed", "7.5"),
    same("cameraTerrainTiltTimeMin", "3"),
    same("cameraTerrainTiltTimeMax", "10"),
    // `cameraBobbing` `[0xbe10c0]` "0" (`0x50b76d`), head bob: kernel `0x511920`, gate `0x5105e0`;
    // ours is `player::camera_dynamics::HeadBob`.
    same("cameraBobbing", "0"),
    // Amplitudes in the CVar's units, scaled by 1/36 (`[0x7ff9d0]`) to yards.
    // `cameraBobbingSmoothSpeed` is the decay rate, read only in the disarm `0x51113a`, which
    // divides the largest component by it for the ramp's duration (~0.069 s at these defaults).
    same("cameraBobbingLRAmplitude", "2"),
    same("cameraBobbingUDAmplitude", "2"),
    same("cameraBobbingFrequency", "0.8"),
    same("cameraBobbingSmoothSpeed", "0.8"),
    // `statusBarText` (`0x48fc34`, record `[0xb4d904]`, no engine reader), read by
    // `TextStatusBar.lua:47,97`: "0" shows the numbers on hover only.
    same("statusBarText", "0"),
    // Enhanced Tooltips (`UIOptionsFrame.lua:15`), registered "1" at `0x48fddd` (`0x82e748`); read
    // only by the stock interface.
    same("UberTooltips", "1"),
    // Registered at `0x603280`: `ChatBubbles` "1", `ChatBubblesParty` "0".
    same("ChatBubbles", "1"),
    same("ChatBubblesParty", "0"),
    // Registered "1" (`0x82e748`), category 4. `profanityFilter` (`0x402e68`, name `0x82e7f4`,
    // callback `0x403570`) masks `ChatProfanity.dbc` spans inside the shared masker `0x4a1a66`,
    // covering all thirteen call sites; `spamFilter` (`0x402e8e`, name `0x82e7d4`, callback
    // `0x4035b0`) silently drops a line matching `SpamMessages.dbc`. The knob is
    // [`crate::text_filter::TextFilterSwitches`].
    same("profanityFilter", "1"),
    same("spamFilter", "1"),
    // Registered by `CGlueMgr::EnterWorld` (`0x46b633` `gameTip` "0", `0x46b658` `showGameTips`
    // "1"), category 5. `gameTip` is the cursor and holds the next tip, not the one on screen;
    // `crate::game_tip` advances it.
    same("gameTip", "0"),
    same("showGameTips", "1"),
    // `showLootSpam` (`0x48fd1c`, name `0x8430a0`, "1" `0x82e748`, record `0xb4e2bc`): off, group
    // loot-roll lines are hidden and only the winner shows. Its three readers are the roll-line
    // composers; the knob is [`crate::ui_loot::LootConfig::show_loot_spam`].
    same("showLootSpam", "1"),
    // `guildMemberNotify` (`0x5e24c7`, "0" `0x82e570`, record `0xc4d3c4`): guildmate log on/off
    // lines, read only in `SMSG_GUILD_EVENT`'s handler. The knob is
    // [`crate::ui_guild::GuildMemberNotify`].
    same("guildMemberNotify", "0"),
    // Both registered "3" (`0x48fc6c`, `0x48fc88`); the minimap's +/- buttons write them through
    // `Minimap:SetZoom`. The knob is [`crate::minimap::MinimapZoom`].
    same("minimapZoom", "3"),
    same("minimapInsideZoom", "3"),
    // Load out of date AddOns, inverted (`0x402c3b`): "1" enforces the check. Read by the load walk
    // ([`Cvars::addon_version_check`]) and live by the VM's gate.
    same("checkAddonVersion", "1"),
    // `gxApi` (`0x63a833`: name `0x842a64`, default `0x864f7c` "direct3d", flags 3, callback
    // `0x63b030`, record `[0xc4ea94]`): the reference builds D3D9 unless it reads "OpenGl"
    // (`0x63a3c4`, `0x842a5c`). Here it reports the wgpu backend (`wgpu::Backend::to_str`), pushed
    // from `RenderAdapterInfo` and owned by the session, so it is never persisted. pfUI's
    // `panel.lua:185` concatenates it.
    deviates(
        "gxApi",
        "",
        "direct3d",
        "descriptive, not a selector — benilla renders through wgpu, which has no D3D9 \
         backend and no chooser; the value is the live adapter's own and is never persisted",
    )
    .latched(),
    // `gxVSync` (`0x63a859`, "1", flags 3), `OptionsFrame.lua:9`. The knob is
    // [`crate::video::VideoConfig::vsync`], which the window's present mode follows at the
    // `RestartGx` commit. `$WOW_NOVSYNC=1` overrides it for the session.
    same("gxVSync", "1").latched(),
    // `gxWindow` (`0x63a889`): "0" on enUS; zhCN registers "1", as it does `gxMaximize`
    // (`0x63a8e0`), and koKR `AutoInteract` (`0x603390`). The knob is
    // [`crate::video::VideoConfig::display`]. Deviation: "0" raises a borderless fullscreen window
    // instead of mode-setting the display, because Wayland and macOS offer no mode-set and X11's
    // leaves the desktop changed after a crash ([`crate::video`]).
    same("gxWindow", "0").latched(),
    // `gxResolution`, a string row parsed by `video::on_cvar`. Here it is only the windowed size:
    // fullscreen is the monitor's own and no mode list is offered.
    deviates(
        "gxResolution",
        "1600x900",
        "640x480",
        "narrowed to the WINDOWED size only — fullscreen is the monitor's own and we expose \
         no mode list, and 640x480 is not a window anyone would ship a client at",
    )
    .latched(),
    // benilla's own: a body pane's doll renders at half the frame rate while the pane is open; the
    // reference draws it in the main pass. The knob is [`crate::portrait::PaneRate`].
    ours(
        "boothHalfRate",
        "1",
        "benilla's own — the reference draws its doll inside the main pass and has no \
         second view to rate-limit",
    ),
    // `gxMultisample` (`0x63a950`, flags 3). The reference formats its default from field 21 of the
    // `VideoHardware.dbc` row `DetectHardware` (`0x641260`) matches, which is 1, no multisampling,
    // on every fallback row a modern GPU reaches. The knob is [`benilla_world::view::MsaaSetting`],
    // read once at the camera's spawn; `$WOW_MSAA` overrides it for the session.
    same("gxMultisample", "1").latched(),
    // `GetCurrentMultisampleFormat 0x48c580` looks up all three of the Video dropdown's values by
    // name, so these must exist. They describe the swapchain's own pair and steer nothing;
    // `SetMultisampleFormat` writes them as `0x48c640` does.
    deviates(
        "gxColorBits",
        "32",
        "16",
        "these describe, they do not steer — the pair is our swapchain's own, and every \
         format `MsaaFormats` publishes carries it",
    )
    .latched(),
    deviates(
        "gxDepthBits",
        "32",
        "16",
        "as `gxColorBits` — the depth half of the same descriptive pair",
    )
    .latched(),
    // `trilinear` and `anisotropic`, over `benilla_assets::TexFilterSetting` (`tex_filter.rs`
    // carries the derivation). A change applies at the next launch, since a sampler is baked into
    // each texture at load; the reference's UI says "enabled upon restart".
    // `$WOW_TRILINEAR`/`$WOW_ANISO` override for the session.
    //
    // `trilinear` registers "0", but `hwDetect` runs `DetectHardware 0x641260` and sets it from the
    // matched `VideoHardware.dbc` row before the first frame; the fallback rows an unlisted GPU
    // reaches are 168/169/170, and 169 and 170 give 1. The reference install's `gx.log` resolves to
    // videoID 170.
    overridden(
        "trilinear",
        "1",
        "0",
        "`hwDetect` sets it from `VideoHardware.dbc` field 9 before the first frame, and \
         that field is 1 on both fallback rows an unlisted modern GPU can reach — measured on the \
         reference's own `Logs/gx.log` (`videoID: 170`)",
    ),
    // Not one of `hwDetect`'s sixteen (`[0x639a60, 0x639b80)` never reads `0xc7f2e4`), so the
    // registered "1", off, stands.
    same("anisotropic", "1"),
    // The four video CVars the stock window's verbs read and write (`benilla_ui` `video_pairs`);
    // benilla has no mechanism behind any of them. `shadowLevel` (`0x6885bc`, "1", flags 1,
    // callback `0x688c10`, record `[0xc7f350]`) is behind `Get/SetTerrainMip` as `1 − level`: the
    // callback refuses above 1 and prints "Shadow mip level changed upon restart.", and the
    // terrain's shadow-map mip (`0x66f877`) reads it at load. `hwDetect` leaves it at 1.
    same("shadowLevel", "1"),
    // `doodadAnim` (`0x6884d8`, "1", flags 1, callback `0x688a10`, record `[0xc7f354]`), behind
    // `Get/SetDoodadAnim`: the callback toggles bit `0x8000` of `[0xc7b2a4]` and calls `0x6953d0`,
    // a bare `ret`, so no animation reads it. `hwDetect` sets 0 or 1 by CPU tier.
    same("doodadAnim", "1"),
    // `texLodBias` (`0x6885e2`, "0.0" at `0x84fad4`, flags 1, callback `0x688c90`, record
    // `[0xc7f2f8]`), behind `Get/SetTexLodBias`: the callback refuses outside [-1.0, 1.0] and its
    // sink `0x672640` is `ret 4`.
    same("texLodBias", "0.0"),
    // `baseMip` (`0x6887aa`, "0", flags 1, callback `0x689090`, record `[0xc7f2f0]`), behind
    // `Get/SetBaseMip` as `1 − level`: the callback refuses outside [0, 1] ("BaseMip must be 0 or
    // 1") and sets the device's first uploaded mip level (`0x589bf0`, read at `0x59f6ce`). benilla
    // uploads every level; the write is kept and has no effect. `hwDetect` leaves it at 0.
    same("baseMip", "0"),
    // `spellEffectLevel` (`0x688900`: name `0x869324`, default `0x843068` "2", flags 1, callback
    // `0x689510`, record `[0xc7f2a4]`), the Spell Detail slider (`OptionsFrame.lua:32`). The
    // callback's two effects here: the particle emission scalar ([`crate::video::on_cvar`]) and
    // the dynamic-object shard rate, read at each emitter's spawn (`crate::entities`). Its third,
    // skipping one M2-scene ground decal below level 2 (`0x672b3a`), has no counterpart: benilla
    // does not draw that projection.
    same("spellEffectLevel", "2"),
    // Weather Intensity (`OptionsFrame.lua:33`), registered "2" (`0x67b806`, flags 0, callback
    // `0x67b870`, name `0x8685ac`). The reader is `benilla_world::weather::WeatherState`, which
    // scales the precipitation spawn rate by `0x67b870`'s table {0.1, 0.33, 0.66, 1.0}; rendering
    // only.
    deviates(
        "weatherDensity",
        "3",
        "2",
        "every precipitation rate in `benilla-world`'s own precipitation module was \
         derived and graded against the reference install's own apitrace captures, and that \
         install runs \
         `SET weatherDensity \"3\"` (K = 1.0) — so 3 is the value a benilla-vs-reference \
         side-by-side is correct at, and the registered 2 would thin every rate to 0.66 against \
         the only client we compare with. The slider is how a player takes it back down",
    ),
    // `gamma` (`0x402d70`: name `0x82e924` "Gamma", default `0x82e92c` "1.0", flags 0, callback
    // `0x4034d0`). The reference uploads `pow(i/255, gamma)` (`0x591680`) through
    // `SetDeviceGammaRamp`, except when windowed (`byte[dev+0x20b]`). Deviation: benilla, which
    // has no exclusive mode, applies the same curve in the composite pass
    // ([`crate::ui_gamma::DisplayGamma`]), because the skipped upload would make a slider that
    // moves no pixel. Spelled "1.000000" because `SetGamma 0x4891f0` formats with `"%f"`, and
    // Restore Defaults must compare equal to the default; [`sync_cvars`] seeds it the same way.
    same("gamma", "1.000000"),
    // `DesktopGamma` (`0x402d4d`: name `0x82e930`, default "0" `0x82e570`, flags 0, callback
    // `0x403500`, record `[0x8826d4]`), the stock window's Use Desktop Gamma box, which it also
    // sets whenever Windowed Mode is ticked (`OptionsFrame.lua:399-408`). On 1 the callback puts
    // the desktop's own ramp back, and `gamma`'s callback (`0x4034d0`) uploads only while this
    // reads 0. No effect here: [`crate::ui_gamma::DisplayGamma`] follows `gamma` alone, because
    // the stock window ticks this box whenever Windowed Mode is (`:399-408`), so honouring it would
    // drop the gamma of every windowed player who opens that window.
    same("DesktopGamma", "0"),
    // ── The stock Video Options window's other boxes (`OptionsFrame.lua:5-22`) ───────────────────
    //
    // `ffx` (`0x6ccfe3`: name `0x86f2ac`, record `[0xce8a04]`), `ffxGlow` (`0x6cc17d`, name
    // `0x86f200`) and `ffxDeath` (`0x6cc6d1`, name `0x86f238`): each "1", flags 1, with the echo stub
    // `0x6cde30` as callback, and each record's integer read every frame. The FFX begin/end pair
    // (`0x6cd890`/`0x6cda70`) runs the active pass only while `ffx` (`0x6cd8a6`), `pixelShaders`
    // (`0x6cd8b6`) and that pass's own enable hold: `CFFXGlow` (`0x6cc5a0`) reads `ffxGlow` at
    // `0x6cc5a8`, `CFFXDeath` (`0x6cded0`) reads `ffxDeath` at `0x6cdf10`. The stock Okay writes
    // `ffx` as Enable All Shaders (`OptionsFrame.lua:202-204`). The knob is
    // [`benilla_world::ffx_glow::FfxSwitches`].
    same("ffx", "1"),
    same("ffxGlow", "1"),
    same("ffxDeath", "1"),
    // `pixelShaders` (`0x688712`: name `0x869560`, "0", flags 1, callback `0x688f40`, record
    // `[0xc7f360]`) and `specular` (`0x6886a0`: name `0x8695d8`, "0", flags 1, callback `0x688e20`,
    // record `[0xc7f2cc]`), Enable All Shaders and Terrain Highlights. Both callbacks act only on a
    // device whose caps field `[0x58a230()+0x98]` is above -1, setting bits 28 and 27 of the render
    // flags `[0xc7b2a4]`, which gate the shader negotiation `0x6941f0` for terrain, water and
    // models; `pixelShaders` also gates the FFX pass (`0x6cd8b6`). No effect here: benilla always
    // draws the shader leg (`liquid.wgsl` records it), and the stock Okay carries this box into
    // `ffx`, which is wired.
    overridden(
        "pixelShaders",
        "1",
        "0",
        "`hwDetect` (`0x639a60`) sets it from `VideoHardware.dbc` field 11 before the first frame, \
         and that field is non-zero on fallback row 170, the one a modern GPU reaches \
         (`Logs/gx.log`: `videoID: 170`)",
    ),
    overridden(
        "specular",
        "1",
        "0",
        "`hwDetect` sets it from the same field 11 as `pixelShaders`, so row 170 gives 1",
    ),
    // `M2UseShaders` (`0x4028e9`, name `0x82e67c`, "1") and `M2UsePixelShaders` (`0x40290c`, name
    // `0x82e644`, "0"), flags 0, no callback: their registering function reads six of its eight
    // `M2*` records at `0x40297f`-`0x4029dd` into the M2 renderer's feature word `[0xceefb0]`
    // (`0x706dd0`), once at startup, hence the stock tooltip's logout requirement. No effect here:
    // benilla's M2 draw has one skinning and lighting path, chosen by no switch.
    same("M2UseShaders", "1"),
    same("M2UsePixelShaders", "0"),
    // `lod` (`0x68848c`: name `0x8697d0`, "1", flags 1, callback `0x688920`, record `[0xc7f314]`),
    // World LOD: bit 2 of `[0xc7b2a4]`, which gates the doodad LOD distance test at `0x6afb11`. No
    // effect here: benilla has no doodad LOD swap, as `lodDist` above has no row.
    same("lod", "1"),
    // `movieSubtitle` (`0x402c18`: name `0x82e9d8`, "0", flags 0, record `[0x882640]`), Cinematic
    // Subtitles: the pre-rendered movies' subtitles, read by the glue's `GetMovieSubtitles`
    // (`0x46dbc0`). No effect here: benilla plays no pre-rendered movie.
    same("movieSubtitle", "0"),
    // `useWeatherShaders` (`0x67b81d`: name `0x868598`, "1", flags 0, no callback, record
    // `[0xc7b588]`), Weather Shaders: read in each precipitation effect's constructor (`0x67480f`,
    // `0x6775a7`, `0x679274`), which clears the effect's shader flag at 0. No effect here: rain's
    // two legs are one draw with other scalars, but snow's fixed-function leg (`0x678960`) is a
    // CPU-built triangle per flake where the shader leg (`0x678610`) is a point sprite, a second
    // draw path `benilla_world::weather` does not have, so the shader leg always runs.
    same("useWeatherShaders", "1"),
    // The latched `gx*` rows the stock window's boxes and refresh dropdown write (flags 3, committed
    // by `RestartGx`), each callback storing one field of the device-format record `0xc4eb98`.
    //
    // `gxMaximize` (`0x63a8e0`: name `0x864edc`, "0", "1" on zhCN like `gxWindow`, callback
    // `0x63b1f0` → `+0x09` `[0xc4eba1]`): the window rebuild `0x58cf10` makes a windowed, maximized
    // window a borderless popup (style `0x90000000`) at the screen's full size (`GetSystemMetrics`
    // 0 and 1). The knob is [`crate::video::VideoConfig::maximize`].
    same("gxMaximize", "0").latched(),
    // `gxTripleBuffer` (`0x63a80d`: name `0x864f88`, "0", callback `0x63afe0` → `+0x18`), whose
    // callback refuses any value but 0 or 1 ("TripleBuffer must be 0 or 1", `0x8650b8`), a range
    // this registry has no way to refuse with. No effect here: wgpu's present modes offer no
    // triple-buffer choice.
    same("gxTripleBuffer", "0").latched(),
    // `gxCursor` (`0x63a906`: name `0x864eb8`, "1", callback `0x63b220` → `+0x05` `[0xc4eb9d]`),
    // Hardware Cursor. No effect here: benilla always draws the OS cursor (`crate::cursor`) and has
    // no software cursor to fall back to.
    same("gxCursor", "1").latched(),
    // `gxFixLag` (`0x63a9c1`: name `0x864e40`, callback `0x63b310` → `+0x06` `[0xc4eb9e]`), Fix
    // Input Lag: its default is `"%d"` of `VideoHardware.dbc` field 20 (`[[0xc4e6e4]+0x50]`) of the
    // row `DetectHardware` matched, 1 on all three fallback rows (168, 169, 170) a modern GPU can
    // reach. No effect here: benilla has no such switch in its frame pacing.
    same("gxFixLag", "1").latched(),
    // `gxRefresh` (`0x63a7e7`: name `0x864fa8`, "75", callback `0x63aed0` → `+0x28`), the refresh
    // dropdown (`OptionsFrame.lua:242-245`, `:300`); the callback accepts only the eleven rates at
    // `0x8645e0` (60 to 200). No effect here: fullscreen is borderless at the monitor's own rate,
    // and `GetRefreshRates` answers the reference's no-rates sentinel, which disables the dropdown.
    same("gxRefresh", "75").latched(),
    // benilla's own: the world renders at `window × this` while the UI stays native. The knob is
    // [`crate::world_backdrop::RenderScale`], clamped to `RENDER_SCALE_RANGE`; at "1" nothing is
    // resampled. `$WOW_RENDER_SCALE` overrides it for the session.
    ours(
        "renderScale",
        "1",
        "benilla's own — the reference has no off-screen buffer to hang a resolution dial \
         on; its nearest equivalent, `gxResolution`, drops the interface with the world",
    ),
    // benilla's own: `/console fpsJournal 1` appends a per-second row of position, frame cost and
    // per-pass GPU time to `benilla-config/Diagnostics/fps-journal.csv`. The knob is
    // [`crate::perf::FpsJournalSetting`].
    ours(
        "fpsJournal",
        "0",
        "benilla's own — 1.12 has no player-side perf log; its nearest thing is the \
         Ctrl+R framerate label, a number with no file behind it",
    ),
    // `lastCharacterIndex` (`0x402d93`, "0" `0x82e570`, category 4, handle `[0x882674]`), help
    // "Last character selected": a 0-based row (the selection cell `[0x83856c]` under `"%d"`), so
    // "0" is the first character. It mirrors [`crate::char_select::Roster::pending_index`].
    same(crate::char_select::CVAR_LAST_CHARACTER, "0"),
];
