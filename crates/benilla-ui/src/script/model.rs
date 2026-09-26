use std::collections::{HashMap, HashSet};

use crate::layout::{LayoutInput, LayoutSolver, Rect};
use crate::widget::{FrameHandle, RegionHandle, WidgetArena};

use super::{
    auction, backdrop, bank, bind_confirm, camera_view, char_stats, chat_window, colorselect,
    container, craft, cursor, death, duel, follow, gossip, guild, inspect, item_text, loot,
    loot_roll, macros, mail, merchant, party, petition, pvp, quest, quest_log, reputation, session,
    simplehtml, skills, slider, social, spellbook, stable, taxi, trade, tradeskill, trainer,
    weapon_enchant, ActionSlot, AuraState, FontObject, ItemTemplateView, MusicRequest,
    PlayerReqState, RegionData, ScriptValue, SoundRequest, UnitState,
};

// ── The Rust-side model ──

/// The host's answer to whether a texture path resolves to a file.
pub type TextureProbe = Box<dyn Fn(&str) -> bool>;

/// The host's answer to a texture path's size in texels.
pub type TextureSizeProbe = Box<dyn Fn(&str) -> Option<(u32, u32)>>;

/// The host's answer to whether a font path loads.
pub type FontProbe = Box<dyn Fn(&str) -> bool>;

/// Minted id to handle, indexed directly: ids are dense and every scripted call looks one up.
#[derive(Debug, Clone)]
pub(crate) struct IdMap<T>(Vec<Option<T>>);

impl<T> Default for IdMap<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T: Copy> IdMap<T> {
    pub(crate) fn get(&self, id: &u32) -> Option<&T> {
        self.0.get(*id as usize).and_then(Option::as_ref)
    }
    pub(crate) fn contains_key(&self, id: &u32) -> bool {
        self.get(id).is_some()
    }
    pub(crate) fn insert(&mut self, id: u32, v: T) -> Option<T> {
        let i = id as usize;
        if self.0.len() <= i {
            self.0.resize(i + 1, None);
        }
        self.0[i].replace(v)
    }
    pub(crate) fn remove(&mut self, id: &u32) -> Option<T> {
        self.0.get_mut(*id as usize).and_then(Option::take)
    }
}

/// The Rust-side model behind the Lua VM, in `lua.app_data`. It holds no mlua handles, whose
/// persistent count the vendored `LUAI_MAXCSTACK` caps.
pub(crate) struct Model {
    /// Every discovered addon in load order: the AddOn API's registry, filled at world entry.
    pub(crate) addons: Vec<super::addon::AddOnInfo>,
    /// The Lua index space into `addons`, a title-sorted, hidden-filtered list (`0x51da70`).
    pub(crate) addon_index: Vec<usize>,
    /// Lowercased names `SMSG_ADDON_INFO` marked `status = 2`; `None` until the reply arrives, and
    /// until then `GetNumAddOns()` is 0, as in the reference (`[0xbe1b90]`, reset at `0x51fad1`).
    pub(crate) addon_info_hidden: Option<Vec<String>>,
    /// The AddOns folder, so `LoadAddOn` can read an addon's files from inside a Lua binding.
    pub(crate) addons_root: Option<std::path::PathBuf>,
    /// The host's reader for chain-sourced addon files; without one such an addon is `MISSING`.
    pub(crate) addons_chain_reader: Option<super::addon::SharedChainReader>,
    /// The host's font engine, so a metric answers inside the asking call, not a frame later.
    pub(crate) measurer: Option<Box<dyn super::TextMeasure>>,
    /// Whether a texture path resolves (patch chain or loose addon file), so the path form of
    /// `SetTexture` returns the reference's 1 or nil inline (`0x79bb40`). `None` answers nil.
    pub(crate) texture_probe: Option<TextureProbe>,
    /// A path's texel size, which an axis authored as 0 takes, one texel per unit, as the client's
    /// `GetWidth` (`0x770720`) and `GetHeight` (`0x770790`) do; `None` leaves it as authored.
    pub(crate) texture_size_probe: Option<TextureSizeProbe>,
    /// Whether a font path loads: `SetFont`'s 1 or nil, nil meaning a load failure (`0x79f345`,
    /// `0x79f361`, from `0x5c1ae0` in the `0x44d040` cache). Unlike `texture_probe`, `None` answers
    /// 1 for a non-empty path: without a font backend nothing failed, and addons revert their font
    /// on a nil.
    pub(crate) font_probe: Option<FontProbe>,
    /// Saved-variables folders, one `<Addon>.lua` each: the account's, then this character's.
    pub(crate) addons_saved_account: Option<std::path::PathBuf>,
    pub(crate) addons_saved_character: Option<std::path::PathBuf>,
    /// FrameXML templates, global across files as the client's (`0x6ee500`); held here so the
    /// loader runs from a bare `&Lua`, which lets `LoadAddOn` load inside a binding.
    pub(crate) framexml_templates:
        std::cell::RefCell<std::collections::HashMap<String, crate::framexml::Element>>,
    /// `FrameXML_Debug`'s flag (`0x488440` over `[0xceea30]`, boots 0); the loader's trace lines
    /// print while it is greater than 0 (`0x6ee298`), so it is signed.
    pub(crate) framexml_debug: std::cell::Cell<i32>,
    /// The FrameXML font registry, a namespace apart from templates (a font inherits only a font).
    pub(crate) framexml_fonts:
        std::cell::RefCell<std::collections::HashMap<String, crate::framexml::Element>>,
    pub(crate) arena: WidgetArena,
    /// Per-frame anchors, size and scale; every live frame has one from creation on.
    pub(crate) layout_inputs: HashMap<FrameHandle, LayoutInput>,
    /// Each resolvable frame's rect from the last resolve.
    pub(crate) resolved: HashMap<FrameHandle, Rect>,
    /// The anchor solver, kept across resolves so a steady round allocates nothing; scratch only.
    pub(crate) solver: LayoutSolver,
    /// Tier 2 of the resolve gate, the last converged inputs' fingerprint; `None` forces a resolve.
    pub(crate) layout_fingerprint: Option<super::layout::InputFingerprint>,
    /// Tier 1 of the resolve gate: every layout write bumps it, and a resolve returns at once while
    /// it equals `layout_epoch_resolved`. Never bump it per frame: tier 2 hashes the whole UI.
    pub(crate) layout_epoch: u64,
    /// Layout ids named by writes since the last converged resolve, each write claiming it left the
    /// cached graph's shape alone, so a resolve seeds its dirty closure from them. `None` derives
    /// the graph in full; `WOW_LAYOUT_VERIFY` checks every incremental pass against a full one.
    pub(crate) layout_touched: Option<Vec<u32>>,
    /// Regions a write may have re-keyed for text measure (text, font, outline, wrap, scale) since
    /// the last sweep, which asks only these; `None` walks all. A site that forgets to name its
    /// region is caught by `WOW_LAYOUT_VERIFY`, on for every test here, which panics on the miss.
    pub(crate) measure_dirty: Option<Vec<crate::widget::RegionHandle>>,
    /// Per message frame, `(lines_gen, env hash)` at its last request-free sweep; a match skips it.
    /// A destroyed frame's entry is inert: generational handles never match again.
    pub(crate) msg_swept: std::collections::HashMap<crate::widget::FrameHandle, (u64, u64)>,
    /// Lines the message sweep hashed; a settled second sweep hashes none.
    pub(crate) msg_lines_hashed: u64,
    /// Under `WOW_LAYOUT_VERIFY`, the resolve just taken was incremental and owes a full re-run.
    pub(crate) layout_verify_recheck: bool,
    /// Per-node input hashes and dirty-closure scratch: which nodes a let-through resolve solves.
    pub(crate) layout_scope: super::layout::LayoutScope,
    /// The epoch the last converged resolve closed on; `None` sends the next one past tier 1.
    pub(crate) layout_epoch_resolved: Option<u64>,
    /// Resolves the gate let through, counted at its decision, so verify mode reads the same.
    pub(crate) layout_solves: u64,
    /// Resolves that got past tier 1, including those that then skipped: the whole-roster walk's
    /// cost, which `layout_solves` misses. Verify mode walks anyway, so assert on this only from a
    /// crate that depends on `benilla-ui`; inside this crate, assert on `layout_solves`.
    pub(crate) layout_gate_walks: u64,
    /// Full layout-graph derivations: 0 on a steady UI, so a rise means a write fell back to
    /// `touch_layout`. The `WOW_LAYOUT_VERIFY` re-run's derivation is not counted.
    pub(crate) layout_derives: u64,
    /// The last solve's width, `(frames solved, regions swept)`.
    pub(crate) layout_last_scope: (usize, usize),
    /// Fixpoint rounds run across all solves.
    pub(crate) layout_rounds: u64,
    /// Per frame, rasterized links `(y-up rect, link, full |H…|h markup)` for `OnHyperlinkClick`.
    pub(crate) link_spans: HashMap<FrameHandle, Vec<(Rect, String, String)>>,
    /// Tab was pressed in the chat edit box (`BenillaChatTabPressed`), for the whisper cycle.
    pub(crate) chat_tab: bool,
    /// Region visuals (texture, colour, text) and layout (anchors, size, justify).
    pub(crate) region_data: HashMap<RegionHandle, RegionData>,
    /// Each frame's backdrop plate (`<Backdrop>` or `SetBackdrop`, the client's `frame+0x1ac`).
    pub(crate) backdrops: HashMap<FrameHandle, backdrop::Backdrop>,
    /// `CSimpleHTML`'s state: element fonts `+0x350`, link format `+0x360`, content nodes `+0x340`.
    pub(crate) simple_html: simplehtml::SimpleHtmlStates,
    /// Named `<Font>` objects by lowercased name, as 1.12 matches them with `SStrCmpI` (`0x783870`,
    /// `0x7838c7`); read them only through `font_object`, where the fold lives.
    pub(crate) font_objects_by_lower: HashMap<String, FontObject>,
    /// Anchored regions' owner-relative rects from the last resolve; anchor-less ones are absent.
    pub(crate) region_resolved: HashMap<RegionHandle, Rect>,

    /// The next object id to mint; ids start at 1, since 0 is `SCREEN`.
    pub(crate) next_id: u32,
    pub(crate) id_to_frame: IdMap<FrameHandle>,
    pub(crate) frame_to_id: HashMap<FrameHandle, u32>,
    pub(crate) id_to_region: IdMap<RegionHandle>,
    pub(crate) region_to_id: HashMap<RegionHandle, u32>,
    /// Region name to id, first wins: `SetPoint` anchors to a sibling region by name, after frames.
    pub(crate) region_names: HashMap<String, u32>,

    /// Which handlers each frame has; the closures themselves live in Lua's `REG_SCRIPTS` table.
    pub(crate) scripts: HashMap<FrameHandle, HashSet<&'static str>>,
    /// Frames with an `OnUpdate` script, kept by `SetScript`; the tick sorts them by frame id.
    pub(crate) on_update_frames: Vec<FrameHandle>,
    /// Frames with an `OnSizeChanged` script, kept by `SetScript`, for the resolve's size snapshot.
    pub(crate) on_size_changed_frames: Vec<FrameHandle>,
    /// Frames with an `OnUpdateModel` script, fired as a visible model pane paints (`0x76d1a0`).
    pub(crate) on_update_model_frames: Vec<FrameHandle>,
    /// Per model file, a pane clock's sequences and bounds, read by the host off the `MD20`.
    pub(crate) model_facts: HashMap<String, std::sync::Arc<crate::widget::ModelFileFacts>>,
    /// Model files still waiting for their facts, for the host to load; deduplicated.
    pub(crate) model_facts_wanted: Vec<String>,
    /// Edit boxes owed `OnTextChanged`: the reference's dirty bit (`[E+0x31c]` bit 0), set by
    /// `SetText`/`Insert`, cleared only by `0x77d3e0`: one fire per drain, with the final text, as
    /// `MoneyInputFrame_SetCopper` needs. A hidden box's fire waits for a show, key or mouse-down.
    pub(crate) dirty_editboxes: Vec<FrameHandle>,
    /// Event name to its frames in registration order, never a set: `SignalEvent` (`0x703e50`)
    /// dispatches in that order across frames. Re-registering keeps a frame's position.
    pub(crate) event_to_frames: HashMap<String, Vec<FrameHandle>>,
    pub(crate) frame_events: HashMap<FrameHandle, HashSet<String>>,
    /// `RegisterAllEvents` frames in registration order, dispatched after an event's own list and
    /// cleared only by `UnregisterAllEvents`. Deviation: a flag, not the reference's append to
    /// every event's list (`0x7023e0`, which orders by registration time and lets `UnregisterEvent`
    /// drop one event), because events register here by name and no list of every name exists.
    pub(crate) all_event_frames: Vec<FrameHandle>,

    /// The focused edit box (`0xcf4dc8`), taking all keys while visible; `None` admits `autoFocus`.
    pub(crate) focused_editbox: Option<FrameHandle>,

    /// The mouse-enabled frame under the cursor, for the next move's `OnLeave`/`OnEnter`.
    pub(crate) mouseover: Option<FrameHandle>,
    /// A frame under a still cursor hid or showed, so the tick re-runs the hover walk at the saved
    /// position and a newly exposed frame gets `OnEnter` without a mouse move: the reference's
    /// `[root+0x1100]` flag (set at `0x764cbb`/`0x764b8d`, pumped at `0x7657a1` into `0x7660d0`).
    pub(crate) hover_repick: bool,
    /// Per button, the frame its press landed on, for `OnClick`'s same-frame release test.
    pub(crate) mouse_down_on: HashMap<String, FrameHandle>,
    /// The client's one mouse-capture slot (`root+0x80`), read by the mouse-down raise. Set at
    /// `0x7663e6` to the capture, else the hovered frame, so a chorded press keeps the capture;
    /// cleared at `0x7664bb` once no button is down. A title-region press captures nothing.
    pub(crate) mouse_capture: Option<FrameHandle>,
    /// Per frame, when its last single `OnClick` fired: the client's `[CButton+0x334]`, the whole
    /// double-click state. It has no button identity (left then right makes a double), is zeroed
    /// when a double fires, and nothing else clears it, not hide, disable or the cursor leaving.
    pub(crate) last_click: HashMap<FrameHandle, f64>,

    /// Frames the last resolve resized, `(id, width, height)`, owed `OnSizeChanged` outside it.
    pub(crate) pending_size_changed: Vec<(u32, f32, f32)>,

    /// Script errors caught from `pcall`'d handlers, drained by the host each frame.
    pub(crate) errors: Vec<String>,
    /// Errors owed to `geterrorhandler()`, which the reference calls on each caught error; one the
    /// handler itself raises skips this queue, which stops the recursion.
    pub(crate) pending_error_dispatch: Vec<String>,
    /// The session's retained diagnostic log, deduplicated and bounded; `errors` drains each frame.
    pub(crate) diagnostics: super::diagnostics::DiagnosticLog,
    /// Non-fatal warnings for the host, such as a dropped `inherits=` template.
    pub(crate) warnings: Vec<String>,
    /// The screen-root rect (`[bottom, left, top, right]`), the anchor base for top-level frames.
    pub(crate) screen: Rect,

    /// Each unit token's state as pushed this frame, keyed lowercased because 1.12's resolver
    /// (`0x515970`) matches with `SStrCmpI`: read it only through `unit`, where the fold lives.
    pub(crate) units_by_lower: HashMap<String, UnitState>,
    /// Per unit token, auras in display order: the player's by insertion, the rest by aura slot.
    pub(crate) auras: HashMap<String, Vec<AuraState>>,
    /// Spell ids the cancel verbs queued (`CancelPlayerBuff`, `CancelTrackingBuff`, …), one
    /// `CMSG_CANCEL_AURA` each.
    pub(crate) cancel_aura_requests: Vec<u32>,
    /// The player's active tracking aura, behind `GetTrackingTexture`.
    pub(crate) tracking: Option<super::aura::TrackingState>,
    /// `TargetUnit`, `AssistUnit` and `TargetLastEnemy` calls in call order, one queue as the
    /// reference routes all three through one helper; an unresolvable one is a no-op, as there.
    pub(crate) selection_requests: Vec<super::SelectionRequest>,
    /// `TargetNearestFriend` calls, `true` for the reverse cycle.
    pub(crate) target_nearest_friend_requests: Vec<bool>,
    /// `TargetByName(name, exactMatch)` calls, for the app's by-name resolver.
    pub(crate) target_by_name_requests: Vec<(String, bool)>,
    /// `ClearTarget()` fired with a live target, the last step of `ToggleGameMenu`'s ESC chain.
    pub(crate) target_clear: bool,
    /// `DropItemOnUnit` tokens (`0x48d960`), gated by the app; a refusal silently keeps the item.
    pub(crate) drop_item_on_unit: Vec<String>,
    /// `SpellTargetUnit` tokens: a unit frame binds the spell currently waiting on the cursor.
    pub(crate) spell_target_unit: Vec<String>,

    /// Channels the server confirmed, in join order: the numbers `GetChannelName` answers.
    pub(crate) joined_channels: Vec<Option<String>>,
    /// The party or raid roster the app pushes; the default is not in a group.
    pub(crate) party: party::PartyState,
    /// Party and loot-method calls (`AcceptGroup`, `InviteToParty`, `SetLootMethod`, …) queued.
    pub(crate) party_requests: Vec<party::PartyRequest>,
    /// The ready-check deadline and the unanswered flags the timeout tick reads.
    pub(crate) ready_check: party::ReadyCheckState,
    /// Saved raid lockouts from `SMSG_RAID_INSTANCE_INFO`, which outlive any roster change.
    pub(crate) saved_instances: Vec<party::SavedInstanceInfo>,
    /// `SetRaidRosterSelection`'s raid row index, client-side only.
    pub(crate) raid_selection: i64,
    /// The current map's `Map.dbc` `InstanceType`, all `IsInInstance()` reads; `None` for no row.
    pub(crate) instance_type: Option<u32>,
    /// `CanShowResetInstances()`, the reference's four-term predicate (`0x495c90`), app-computed.
    pub(crate) can_reset_instances: bool,
    /// `ResetInstances()` calls queued.
    pub(crate) reset_instance_asks: u32,
    /// Friends, ignores and the last `/who`, display-resolved as the reference resolves them.
    pub(crate) social: social::SocialState,
    /// Social calls (`AddFriend`, `RemoveFriend`, `SendWho`, …) queued.
    pub(crate) social_requests: Vec<social::SocialRequest>,
    /// The client-local LFG slot words (`[0xbc70a0]`) and comment (`[0xbc6e98]`, 0x80 bytes) that
    /// `SetLookingForGroup` writes and `GetLookingForGroup` reads; the reference's slots stay 0.
    pub(crate) lfg_slots: [u32; 3],
    pub(crate) lfg_comment: String,
    /// Guild roster, ranks, MOTD and info, sorted and filtered by the app, which holds the toggles.
    pub(crate) guild: guild::GuildState,
    /// The rank editor's unsaved edits, apart from `guild` so a push cannot discard them.
    pub(crate) guild_control: guild::GuildRankEdit,
    /// Guild calls (`GuildInviteByName`, `GuildSetMOTD`, `GuildControlSaveRank`, …) queued.
    pub(crate) guild_requests: Vec<guild::GuildRequest>,
    /// The charter price and open petition, names resolved from caches as the reference does.
    pub(crate) petition: petition::PetitionState,
    /// Charter calls (`BuyGuildCharter`, `SignPetition`, `TurnInGuildCharter`, …) queued.
    pub(crate) petition_requests: Vec<petition::PetitionRequest>,
    /// Per chat window from `ChatFrame1`, the tint, alpha and font size its tab menu can change.
    pub(crate) chat_window_looks: [chat_window::ChatWindowLook; chat_window::NUM_CHAT_WINDOWS],
    /// 0-based windows whose look Lua changed, the persist cue.
    pub(crate) chat_window_changes: HashSet<usize>,
    /// Chat-type colours (`0xb4e518`): 94 fixed and 10 `CHANNELn`, overlaid by `chat-cache.txt`.
    pub(crate) chat_colors: Vec<super::chat_types::ChatTypeColor>,
    pub(crate) chat_colors_changed: bool,
    /// The languages this character knows, in `Languages.dbc` row order.
    pub(crate) known_languages: Vec<String>,
    pub(crate) zone_channel_catalog: Vec<super::channel::ZoneChannelRow>,
    pub(crate) channel_commands: Vec<super::channel::ChannelCommand>,
    /// The recruitment auto-join latch `[0x843608]`: 0 `STANDARD`, 1 `AUTO` (`0x49ea70` maps any
    /// other word to 1). Boots at 1, as the reference writes `AUTO` for an untouched option.
    pub(crate) guild_recruitment_mode: u8,
    /// Any `SetGuildRecruitmentMode(1)`, even unchanged: the reference runs the cascade `0x49ea90`.
    pub(crate) guild_recruitment_cascade: bool,
    /// Lua changed the recruitment mode: the chat cache's dirty signal.
    pub(crate) guild_recruitment_changed: bool,
    /// `DoEmote` calls queued.
    pub(crate) emote_requests: Vec<super::chat_misc::EmoteRequest>,
    /// `RandomRoll` calls queued.
    pub(crate) roll_requests: Vec<(u32, u32)>,
    /// `UninviteByName` calls queued.
    pub(crate) uninvite_requests: Vec<String>,
    /// `ConsoleExec` lines that were not CVar writes.
    pub(crate) console_lines: Vec<String>,
    /// `LoggingChat` and `LoggingCombat`'s flags, and their change cue.
    pub(crate) logging_chat: bool,
    pub(crate) logging_combat: bool,
    pub(crate) logging_changed: bool,
    /// A user-placed frame moved or resized, or `SetUserPlaced` ran: the layout cache's save cue.
    pub(crate) user_placed_changed: bool,
    /// The default chat language; `None` is no player, so `GetDefaultLanguage()` returns nothing.
    pub(crate) default_language: Option<String>,
    /// `AcceptDuel`, `CancelDuel` and `StartDuel*` calls; Lua sees duels only through events.
    pub(crate) duel_requests: Vec<duel::DuelRequest>,
    /// `FollowUnit` and `FollowByName` calls; Lua sees following only as `AUTOFOLLOW_*` events.
    pub(crate) follow_requests: Vec<follow::FollowRequest>,
    /// `SetView`, `SaveView`, `ResetView`, `NextView`, `PrevView` and `FlipCameraYaw` calls; the
    /// views reach Lua only as CVars, `cameraView` and `camera{Distance,Pitch,Yaw}{,A..D}`.
    pub(crate) camera_view_requests: Vec<camera_view::CameraViewRequest>,
    /// `Logout`, `Quit`, `CancelLogout`, `ForceQuit` calls; Lua sees the countdown only as events.
    pub(crate) session_requests: Vec<session::SessionRequest>,
    /// `TogglePVP` calls, a count because `CMSG_TOGGLE_PVP` has no body.
    pub(crate) pvp_toggles: u32,
    /// The player's private honor fields; before the first push the six self getters read zeros.
    pub(crate) honor: Option<pvp::HonorState>,
    /// The last `MSG_INSPECT_HONOR_STATS` reply; its presence is `HasInspectHonorData`'s answer.
    pub(crate) inspect_honor: Option<pvp::InspectHonorData>,
    /// `RequestInspectHonorData` calls; the pending gate keeps this at 0 or 1.
    pub(crate) inspect_honor_requests: u32,
    /// An inspect-honor query is out (`0xb71fcc`); `RequestInspectHonorData` (`0x4c80a0`) refuses
    /// while set. Cleared by a reply (`0x4c6f4c`) or by the app clearing the slot (`0x4c6f9d`).
    pub(crate) inspect_honor_pending: bool,
    /// `ShowingHelm()`: `PLAYER_FLAGS`' `HIDE_HELM` bit as pushed, set ahead on `ShowHelm` because
    /// the wire verb is a blind toggle. Starts shown, as a fresh character's flags are 0.
    pub(crate) helm_shown: bool,
    /// `ShowingCloak()`, the same for `HIDE_CLOAK`.
    pub(crate) cloak_shown: bool,
    /// Flips from `ShowHelm`/`ShowCloak`, one `CMSG_TOGGLE_HELM`/`CMSG_TOGGLE_CLOAK` each.
    pub(crate) worn_display_toggles: Vec<super::worn_display::WornDisplay>,

    /// `PLAYER_FIELD_BYTES` byte 2 (descriptor `+0x102a`), which extra action bars are on. Only
    /// the server moves it, as in the reference: `SetActionBarToggles` leaves it a round trip late.
    pub(crate) action_bar_toggles: Option<u8>,
    /// One `CMSG_SET_ACTIONBAR_TOGGLES` payload per `SetActionBarToggles` call, ungated.
    pub(crate) action_bar_toggle_sends: Vec<u8>,

    /// `PlaySound` and `PlaySoundFile` calls queued.
    pub(crate) sound_queue: Vec<SoundRequest>,

    /// `PlayMusic` and `StopMusic` calls in call order, for the Lua music stream (`[0xb06ccc]`).
    pub(crate) music_queue: Vec<MusicRequest>,

    /// The UI-load sound-suppression depth, the reference's `[0xb05fa0]` (`0x458f50` inc,
    /// `0x458f60` dec), held across the whole UI load `0x48fbf0`, so login and `/reloadui` are
    /// silent. Only name-keyed `PlaySound` (`0x458030`) reads it: the call is dropped before any
    /// lookup yet answers `(willPlay, handle)` as usual. `PlaySound(kitId)` (`0x457fb0`),
    /// `PlaySoundFile` and music play normally.
    pub(crate) sound_suppression: u32,

    /// The CVar table, by lowercased name.
    pub(crate) cvars: HashMap<String, super::cvars::CvarSlot>,
    /// Saved CVar values, set before any registration so a CVar starts at its saved value: the
    /// reference's table outlives `ReloadUI`, while ours is per VM.
    pub(crate) cvars_saved_base: HashMap<String, String>,
    /// `(registered name, new value)` per Lua `SetCVar`, the app's sync and save cue.
    pub(crate) cvar_changes: Vec<(String, String)>,
    /// `(name, default)` per addon `RegisterCVar` that created a slot.
    pub(crate) cvar_registrations: Vec<(String, String)>,
    pub(crate) cvars_warned: HashSet<String>,

    /// The adapter's multisample formats in dropdown order, the reference's list `[0xb4b444]` of
    /// `{colorBits, depthBits, multisample}` (count `[0xb4b440]`, built by `0x48c3e0`).
    pub(crate) multisample_formats: Vec<super::cvars::MultisampleFormat>,

    /// This display's resolutions, ascending, behind `GetScreenResolutions`; empty until pushed.
    pub(crate) screen_resolutions: Vec<super::cvars::ScreenResolution>,
    /// The live size's index in that list, which the host always includes; `None` while the list
    /// is empty. `GetCurrentResolution` answers it 1-based.
    pub(crate) current_resolution: Option<usize>,
    /// What this device and presentation path offer, behind `GetVideoCaps`.
    pub(crate) video_caps: super::cvars::VideoCaps,
    /// `RestartGx()` calls, the video window's apply button.
    pub(crate) restart_gx_asks: u32,

    /// Globals `RegisterForSave` declared, in order: written at logout and re-run at load.
    pub(crate) saved_names: Vec<String>,
    /// Saved-variables files that failed to load; the shutdown write leaves them untouched.
    pub(crate) held_saved_files: Vec<std::path::PathBuf>,

    /// The key-binding table the Key Bindings window edits, with its account and character sets.
    pub(crate) keybinds: super::keybind::KeybindState,

    /// Action slots by Lua action id, 1..120.
    pub(crate) actions: HashMap<u32, ActionSlot>,
    /// Per action, its usable, range, current and cooldown state; an absent one reads cold.
    pub(crate) action_states: HashMap<u32, super::action::StoredActionState>,
    pub(crate) bonus_bar_offset: u8,
    pub(crate) action_uses: Vec<super::action::ActionUse>,
    /// `(action id, packed)` per slot `PickupAction`/`PlaceAction` changed, 0 clearing it: one
    /// `CMSG_SET_ACTION_BUTTON` each, so a drag swap is two sends.
    pub(crate) action_sets: Vec<(u32, u32)>,
    /// GlobalStrings keys of refusals this crate raises, fired by the app as `UI_ERROR_MESSAGE`
    /// (the reference's `DisplayError` `0x496720`), each a literal from `0xb4b498` (stride `0x14`).
    pub(crate) ui_errors: Vec<&'static str>,

    pub(crate) spellbook: spellbook::SpellBookState,
    /// The pet's spellbook, a second array as in the reference (`0xb700f0`, `0xb6f098`).
    pub(crate) pet_book: spellbook::PetBookState,
    /// The player's macros, owned here as 1.12 macros have no server side; the app persists them.
    pub(crate) macros: macros::MacroState,
    /// A script changed the macros: the save and `UPDATE_MACROS` cue.
    pub(crate) macros_dirty: bool,
    /// Bumped by every seed and change, for readers that must not drain `macros_dirty`.
    pub(crate) macros_generation: u64,
    /// The macro icon paths from `SpellIcon.dbc`, behind `GetMacroIconInfo`.
    pub(crate) macro_icons: Vec<String>,
    /// Spell ids `CastSpell` queued.
    pub(crate) spell_casts: Vec<u32>,
    /// `CastSpell(id, "pet")` ids, sent as `CMSG_PET_ACTION` with a type-1 word (`0x4b34ce`).
    pub(crate) pet_spell_casts: Vec<u32>,
    /// `ToggleSpellAutocast` ids for `CMSG_PET_SPELL_AUTOCAST` (`0x2F3`), which names a spell.
    pub(crate) pet_spell_autocasts: Vec<u32>,
    /// An auto-repeat or cast that `SpellStopCasting()` can stop, not a channel (`0x6e6e80`). Its
    /// 1 or nil matters: the ESC chain (`UIParent.lua:1489`) reaches `CloseAllWindows()` on nil.
    pub(crate) casting: bool,
    /// `SpellStopCasting()` fired while casting: the ESC cancel.
    pub(crate) spell_stop: bool,
    /// Spell targeting is on (`SpellIsTargeting`, `0x6e6cd0`); it gates `SpellStopTargeting()`,
    /// whose nil the ESC chain falls through on (`UIParent.lua:1490`).
    pub(crate) spell_targeting: bool,
    /// Tokens for which `SpellCanTargetUnit`'s armed unit word clears fully, used by stock unit
    /// frames before they call `SpellTargetUnit`.
    pub(crate) spell_targetable_units: HashSet<String>,
    /// `SpellStopTargeting()` fired while targeting: the ESC targeting cancel.
    pub(crate) spell_stop_targeting: bool,

    pub(crate) talents: super::talent::TalentUiState,
    /// `LearnTalent(tab, index)` calls queued.
    pub(crate) talent_learns: Vec<(u32, u32)>,
    /// `ConfirmTalentWipe()` calls, one `MSG_TALENT_WIPE_CONFIRM` each; the app holds the trainer.
    pub(crate) talent_wipe_confirms: u32,
    /// `CheckTalentMasterDist()`: the respec question is live and in range; false hides its dialog.
    pub(crate) talent_master_pending: bool,
    // ── The dialog engine's verbs (`0x48dca0` is the first below) ──
    /// `ConfirmPetUnlearn()` calls; the app holds the trainer and the money gate.
    pub(crate) pet_unlearn_confirms: u32,
    /// `CheckPetUntrainerDist()`: the pet trainer's question is pending and in reach.
    pub(crate) pet_untrainer_pending: bool,
    /// `GetInstanceBootTimeRemaining()` in whole seconds, from the `SMSG_RAID_GROUP_ONLY` deadline.
    pub(crate) instance_boot_secs: u32,
    /// A cached area spirit healer (`[0xb4e330/334]`) and seconds to its wave (`[0xb4e338]`).
    pub(crate) area_spirit_healer_cached: bool,
    pub(crate) area_spirit_secs: u32,
    /// `AcceptAreaSpiritHeal()` calls, one `CMSG_AREA_SPIRIT_HEALER_QUEUE` each.
    pub(crate) area_spirit_accepts: u32,
    /// `AcceptBattlefieldPort(index, accept)` calls: the 1-based slot and the normalised answer.
    pub(crate) battlefield_port_requests: Vec<(u8, bool)>,
    /// The battleground scoreboard: pushed rows, faction filter, sort order.
    pub(crate) battlefield_board: super::battlefield_score::ScoreBoard,
    /// `GetBattlefieldInstanceRunTime()`, pushed each frame.
    pub(crate) battlefield_run_time_ms: u32,
    /// `RequestBattlefieldScoreData()` calls.
    pub(crate) battlefield_score_requests: u32,
    /// `LeaveBattlefield()` calls that passed the "ended" gate.
    pub(crate) battlefield_leave_requests: u32,
    /// The battleground instance list and its scalars, as pushed.
    pub(crate) battlefield_list: super::battlefield_queue::BattlefieldListView,
    /// The selected instance id (`[0xb6eba0]`), not an index; written by `SetSelectedBattlefield`.
    pub(crate) battlefield_selected: u32,
    /// The three queue slots, pushed each frame with their clocks reduced to values.
    pub(crate) battlefield_slots: Vec<super::battlefield_queue::BattlefieldQueueSlot>,
    /// `GetBattlefieldInstanceExpiration()`, pushed each frame.
    pub(crate) battlefield_instance_expiration_ms: u32,
    /// `JoinBattlefield` calls, `(instance id, as group)`.
    pub(crate) battlefield_join_requests: Vec<(u32, bool)>,
    /// `ShowBattlefieldList` calls that passed their gates: the queued slot's map id.
    pub(crate) battlefield_list_requests: Vec<u32>,
    /// Teammates' map positions, the flag carrier, the icon scale and position requests.
    pub(crate) battlefield_positions: Vec<super::battlefield_positions::BattlefieldPositionView>,
    pub(crate) battlefield_flag: Option<super::battlefield_positions::BattlefieldFlagView>,
    pub(crate) battlefield_icon_scale: f32,
    pub(crate) battlefield_position_requests: u32,
    /// `CancelMeetingStoneRequest()` calls; the app gates on leadership.
    pub(crate) meeting_stone_cancels: u32,
    /// The meeting stone's queued area (`[0xb72038]`) and status text (`[0xb7203c]`), as pushed.
    pub(crate) meeting_stone_area: u32,
    pub(crate) meeting_stone_status_text: Option<String>,
    /// Acknowledged tutorials, `None` before `SMSG_TUTORIAL_FLAGS`; all `TutorialsEnabled()` reads.
    pub(crate) tutorial_bank: Option<Vec<u8>>,
    /// `FlagTutorial(n)` calls, 0-based and in range.
    pub(crate) tutorial_flag_requests: Vec<u32>,
    pub(crate) tutorial_clears: u32,
    pub(crate) tutorial_resets: u32,

    /// The stance bar's forms, in bar order.
    pub(crate) shapeshift_forms: Vec<super::shapeshift::StoredShapeshiftForm>,
    /// Form spell ids `CastShapeshiftForm` queued.
    pub(crate) shapeshift_casts: Vec<u32>,

    /// The pet bar's ten slots and two bar-wide bits, replaced whole by every `SMSG_PET_SPELLS`.
    pub(crate) pet_bar: super::pet::PetBarState,
    /// 1-based slots `CastPetAction` queued.
    pub(crate) pet_actions_pressed: Vec<u32>,
    /// 1-based slot indices `TogglePetAutocast` queued.
    pub(crate) pet_autocast_toggles: Vec<u32>,
    /// `PetStopAttack()` calls.
    pub(crate) pet_stop_attacks: u32,
    /// `PetAttack`, `PetFollow`, `PetWait` and mode orders, as the packed word of their bar slot.
    pub(crate) pet_orders: Vec<u32>,
    /// `HasFullControl`, the reference's `[0xb4b3e4]`: set by `SMSG_CLIENT_CONTROL_UPDATE` for the
    /// player, boots 1, and every cast, item and cursor gate refuses at 0.
    pub(crate) player_control: bool,
    /// One `CMSG_PET_SET_ACTION` per entry, its one or two `(position, packed word)` pairs: the
    /// server tells the forms apart by body size, so a move's two pairs must not be flattened.
    pub(crate) pet_set_actions: Vec<Vec<(u32, u32)>>,
    /// `PetAbandon()` and `PetDismiss()` calls, counted apart though both end at one opcode.
    pub(crate) pet_abandons: u32,
    pub(crate) pet_dismisses: u32,
    /// Names `PetRename` queued, from the `PETRENAMECONFIRM` popup.
    pub(crate) pet_renames: Vec<String>,

    /// Bag contents by API bag id (0 is the backpack), and the queued `UseContainerItem` calls.
    pub(crate) containers: HashMap<i64, container::ContainerState>,
    pub(crate) container_uses: Vec<(i64, u32)>,
    /// Per `(bag, slot)`, `(start, duration, enabled)` in `GetTime` seconds, stamped at push.
    pub(crate) container_cooldowns: HashMap<(i64, u32), (f64, f64, bool)>,
    /// `HasKey()`: a keys-family item in equipment, bags, bank or keyring; gates the keyring UI.
    pub(crate) has_key: bool,
    /// What the cursor carries; nothing moves locally, the server's updates settle the bags.
    pub(crate) cursor: Option<cursor::CursorPayload>,
    /// The action bar's drop grid: shown on the cursor's empty-to-held edge for any payload but a
    /// pet action or a vendor row (mode 5; the grab setter `0x4950f0` shows it for mode 7 only).
    pub(crate) cursor_grid_shown: bool,
    /// The pet bar's grid, lit only by a pet-action pickup (`0x494f28`); never both grids at once.
    pub(crate) pet_grid_shown: bool,
    /// The world pick under the cursor (`[this+0x350]`): a world click drops no payload on an
    /// object, only an item on terrain, and anything over nothing.
    pub(crate) world_pick: cursor::WorldPick,
    /// Held-item bag moves, sent as `CMSG_SWAP_INV_ITEM` or `CMSG_SPLIT_ITEM`.
    pub(crate) container_moves: Vec<container::ContainerMove>,
    /// `(bag, slot)` clicks made in repair mode.
    pub(crate) container_repairs: Vec<(i64, u32)>,
    /// Targeting wants an item (`0x6e6330`): a bag or paper-doll click becomes a pick.
    pub(crate) item_pick_armed: bool,
    /// Picked `(bag, slot)`s; a paper-doll click is `EQUIPMENT_BAG` and its 1-based inventory slot.
    pub(crate) item_picks: Vec<(i64, u32)>,
    /// `BindEnchant()` and `ReplaceEnchant()`, the enchant popups' answers.
    pub(crate) enchant_confirms: Vec<cursor::EnchantConfirm>,
    /// `DeleteCursorItem`'s `(bag, slot, count)`, 0 for the whole stack, as `CMSG_DESTROYITEM`.
    pub(crate) container_destroys: Vec<(i64, u32, u32)>,
    /// The wrapping paper a right-click armed (`0x5edea0`: lock it, cursor mode 2, no packet). Not
    /// a cursor payload: the reference sets only the cursor mode, so `CursorHasItem()` stays nil.
    pub(crate) pending_wrap: Option<container::PendingWrap>,
    /// `(giftBag, giftSlot, itemBag, itemSlot)` per completed wrap, in `CMSG_WRAP_ITEM`'s order.
    pub(crate) container_wraps: Vec<(i64, u32, i64, u32)>,
    /// The cursor mode Lua set (`0xbe2c2c`); `ResetCursor` returns it to the world's mode.
    pub(crate) ui_cursor: Option<container::UiCursorMode>,
    /// A FrameXML cursor call ran. The mode is a write, not a level: `0xbe2c2c` keeps its value
    /// until the world or a hover handler writes it, so a handler-less frame leaves it be.
    pub(crate) ui_cursor_dirty: bool,
    /// `AutoEquipCursorItem`'s `(bag, slot)` sources, sent as `CMSG_AUTOEQUIP_ITEM`.
    pub(crate) container_autoequips: Vec<(i64, u32)>,
    /// `PutItemInBag` and `PutItemInBackpack` stores, sent as `CMSG_AUTOSTORE_BAG_ITEM`, or
    /// `CMSG_SPLIT_ITEM` for a split stack.
    pub(crate) bag_autostores: Vec<container::BagAutoStore>,

    /// `RegisterForDrag` buttons per frame, compared case-insensitively. Nothing destroys a frame,
    /// so this, `scripts` and `frame_events` are never pruned; a destroy path must prune all three.
    pub(crate) drag_registered: HashMap<FrameHandle, HashSet<String>>,
    /// The drag gesture, armed by a press on a drag-registered frame, started past the threshold.
    pub(crate) drag: Option<cursor::DragGesture>,
    /// The one `StartMoving()` in flight, the client's `root+0xcfc` slot, apart from `drag` as in
    /// the reference: a move outlives the button, since mouse-up's auto-stop skips a Lua drag.
    pub(crate) moving: Option<super::object::FrameMove>,
    /// The `StartSizing` in flight, cleared by `StopMovingOrSizing` (`0x776990`).
    pub(crate) sizing: Option<super::object::FrameSizing>,
    /// A Slider thumb drag, left press to release or pointer leave, handled here with no Lua.
    pub(crate) slider_drag: Option<slider::SliderDrag>,
    /// A ColorSelect wheel or value-bar drag, press to release.
    pub(crate) color_drag: Option<colorselect::ColorDrag>,

    /// The open gossip menu, the `SelectGossipOption` calls and whether `CloseGossip` ran.
    pub(crate) gossip: Option<gossip::GossipMenu>,
    pub(crate) gossip_selects: Vec<u32>,
    pub(crate) gossip_close: bool,
    /// `SelectGossipQuest` 1-based rows, each sent as `CMSG_QUESTGIVER_QUERY_QUEST` for its quest.
    pub(crate) gossip_quest_selects: Vec<u32>,

    /// The open vendor's stock, the `BuyMerchantItem` calls and whether `CloseMerchant` ran.
    pub(crate) merchant: Option<merchant::MerchantState>,
    /// `GetRepairAllCost`'s total, which the app pushes every frame a vendor is open.
    pub(crate) repair_all_cost: u32,
    pub(crate) merchant_buys: Vec<(u32, u32)>,
    /// The held `(bag, slot)` when `PickupMerchantItem` sells, sent as `CMSG_SELL_ITEM`.
    pub(crate) merchant_cursor_sells: Vec<(i64, u32)>,
    /// A held vendor row dropped in a bag, `(bag, slot, item entry)`, for `CMSG_BUY_ITEM_IN_SLOT`.
    pub(crate) merchant_slot_buys: Vec<(i64, u32, u32)>,
    pub(crate) merchant_close: bool,
    /// `BuybackItem` 1-based slots, the `RepairAllItems` flag and repair mode (`InRepairMode`).
    pub(crate) merchant_buybacks: Vec<u32>,
    pub(crate) repair_all: bool,
    pub(crate) repair_mode: bool,

    /// The stable window: snapshot, selection, drag and queued calls, reset together on close.
    pub(crate) stable: stable::StableModel,

    /// The open bank's slot-purchase row and calls; its contents are bags -1 and 5..=10.
    pub(crate) bank: Option<bank::BankState>,
    pub(crate) bank_purchase: bool,
    pub(crate) bank_close: bool,

    /// The open trainer, its buys, selection and close. The selection is a spell id, since
    /// filters, collapses and re-lists all move row numbers.
    pub(crate) trainer: Option<trainer::TrainerState>,
    pub(crate) trainer_buys: Vec<u32>,
    pub(crate) trainer_selection: Option<u32>,
    pub(crate) trainer_close: bool,
    /// Available, unavailable and used filters, all on by default; they hide services, not headers.
    pub(crate) trainer_filter: [bool; 3],
    /// Collapsed skill-line ids, kept across a re-push and cleared when the trainer closes.
    pub(crate) trainer_collapsed: HashSet<u32>,

    /// The open taxi map, the `TakeTaxiNode` calls, whether `CloseTaxiMap` ran and whether we ride.
    pub(crate) taxi: Option<taxi::TaxiUiState>,
    pub(crate) taxi_takes: Vec<usize>,
    pub(crate) taxi_close: bool,
    pub(crate) taxi_riding: bool,

    /// The open tradeskill window's recipes.
    pub(crate) trade_skill: Option<tradeskill::TradeSkillState>,
    /// `DoTradeSkill` calls, `(spell id, count)`.
    pub(crate) trade_skill_dos: Vec<(u32, u32)>,
    /// The 1-based selection, 0 for none, carried across a re-push by the recipe's spell id.
    pub(crate) trade_skill_selection: u32,
    pub(crate) trade_skill_close: bool,
    /// Collapsed recipe groups by `(ItemClass, ItemSubClass)` (`0x55ba30`), kept across a re-push
    /// and a same-profession reopen like the mask `0x84dd68`; a profession switch resets them.
    pub(crate) trade_skill_collapsed: HashSet<(u32, u32)>,
    /// Groups the SubClass filter hides, empty for all, kept by key like the client's header flag
    /// (`header+0xc`, `0x4fca20`, mask `0x84dd60`); a profession switch resets them.
    pub(crate) trade_skill_subclass_hidden: HashSet<(u32, u32)>,
    /// The InvSlot filter mask (`0x84dd64`), a set bit shown and all ones for all slots. Never
    /// pruned on a re-push, as in the client, so a later slot stays hidden; a switch resets it.
    pub(crate) trade_skill_invslot_mask: u32,
    /// The skill line the state was last built for (`0xbde064`): a push for another line resets
    /// the filters, collapses and selection; the same line keeps them, even across a reopen.
    pub(crate) trade_skill_last_line: u32,
    /// The selected recipe's spell id (`0xbde044`), which carries it across reopen and re-push.
    pub(crate) trade_skill_selected_spell: u32,
    /// A filter or subclass collapse changed: `TRADE_SKILL_UPDATE` (`0x13a`) is owed this frame, as
    /// the client fires it inside the call (`0x4fd710`/`0x4fd730`/`0x4fd750`, `0x4fd180`).
    pub(crate) trade_skill_touched: bool,

    /// The open craft window's recipes.
    pub(crate) craft: Option<craft::CraftState>,
    /// `DoCraft` spell ids; no count, as the 1.12 CraftFrame has no Create All.
    pub(crate) craft_dos: Vec<u32>,
    /// The 1-based selection, 0 for none, carried across a re-push by the recipe's spell id.
    pub(crate) craft_selection: u32,
    pub(crate) craft_close: bool,

    /// `EquipPendingItem`/`CancelPendingEquip` answers in order, and `ConfirmBindOnUse()` calls.
    pub(crate) pending_equip_answers: Vec<bind_confirm::PendingEquipAnswer>,
    pub(crate) bind_on_use_confirms: u32,

    /// The open loot, the row clicks and whether `CloseLoot` ran. A click is the reference's take
    /// `0x4c2790` with flag 0, raising `LOOT_BIND` for a bind-on-pickup row (the C `CLootButton`,
    /// here `BenillaTakeLootSlot`); `LootSlot` passes 1 (`0x4c2e70`), taking it after the confirm.
    pub(crate) loot: Option<loot::LootState>,
    pub(crate) loot_picks: Vec<u32>,
    /// `LootSlot(slot)` rows, 1-based, apart from clicks for the reference's pending-slot gate.
    pub(crate) loot_confirms: Vec<u32>,
    pub(crate) loot_close: bool,
    /// `GiveMasterLoot(slot, candidateIndex)`, both 1-based display positions the app translates.
    pub(crate) loot_master_gives: Vec<(u32, u32)>,

    /// The open group-loot rolls, and the `RollOnLoot` `(roll_id, roll_type)` votes.
    pub(crate) loot_rolls: loot_roll::LootRollsState,
    pub(crate) loot_roll_votes: Vec<(u32, u8)>,
    /// Need or Greed on a bind-on-pickup roll: a `CONFIRM_LOOT_ROLL` popup ask, not a vote.
    pub(crate) loot_roll_confirms: Vec<(u32, u8)>,

    /// The item-text reader: the pushed page, then the close and page-turn calls.
    pub(crate) item_text: Option<item_text::ItemTextState>,
    pub(crate) item_text_close: bool,
    pub(crate) item_text_page_turns: Vec<i32>,
    /// The open inbox and mail calls, rows 1-based: `mail_opens` are rows `GetInboxText` read.
    pub(crate) mail: Option<mail::MailState>,
    pub(crate) mail_check_inbox: bool,
    pub(crate) mail_opens: Vec<u32>,
    pub(crate) mail_take_items: Vec<u32>,
    pub(crate) mail_take_money: Vec<u32>,
    pub(crate) mail_deletes: Vec<u32>,
    pub(crate) mail_returns: Vec<u32>,
    pub(crate) mail_take_texts: Vec<u32>,
    pub(crate) mail_close: bool,
    pub(crate) mail_send: Option<(String, String, String)>,
    pub(crate) mail_send_money: u32,
    pub(crate) mail_send_cod: u32,
    /// The Send tab's attached item, resolved to its guid when the send fires.
    pub(crate) mail_send_item: Option<cursor::CursorItem>,
    /// The usable stationery, in the picker's order.
    pub(crate) mail_stationeries: Vec<mail::StationeryView>,
    /// `SelectStationery`'s `Stationery.dbc` id; 0 is none, which silences `SendMail`.
    pub(crate) mail_stationery: u32,
    /// `HasNewMail()` (`0x4afea0`), from `MSG_QUERY_NEXT_MAIL_TIME` and `SMSG_RECEIVED_MAIL`.
    pub(crate) has_new_mail: bool,

    /// The open auction house and the auction calls. `auction_selected` is each list's selected
    /// auction id, 0 for none (`0x4cfda0`/`0x4cfec0`); `auction_can_query` is the 5 s throttle.
    pub(crate) auction: Option<auction::AuctionState>,
    /// The Browse tab's class tree, pushed once per login.
    pub(crate) auction_item_classes: Vec<auction::AuctionCategory>,
    pub(crate) auction_selected: [u32; 3],
    pub(crate) auction_can_query: bool,
    pub(crate) auction_query: Option<auction::AuctionQuery>,
    pub(crate) auction_owner_query: Option<u32>,
    pub(crate) auction_bidder_query: Option<u32>,
    pub(crate) auction_bids: Vec<auction::AuctionBid>,
    pub(crate) auction_cancels: Vec<u32>,
    pub(crate) auction_start: Option<auction::AuctionStartRequest>,
    pub(crate) auction_sorts: Vec<(usize, String)>,
    pub(crate) auction_close: bool,
    /// The sell slot's item, resolved to its guid when `StartAuction` fires.
    pub(crate) auction_sell_item: Option<cursor::CursorItem>,

    /// The open trade and its calls: `InitiateTrade` tokens, the accept, unaccept and cancel flags,
    /// and `SetTradeMoney` copper (`CMSG_SET_TRADE_GOLD`).
    pub(crate) trade: Option<trade::TradeState>,
    pub(crate) trade_initiates: Vec<String>,
    pub(crate) trade_accept: bool,
    pub(crate) trade_unaccept: bool,
    pub(crate) trade_close: bool,
    /// `BeginTrade()` and `CancelTrade()` calls.
    pub(crate) trade_begin: bool,
    pub(crate) trade_cancel: bool,
    pub(crate) trade_set_money: Option<u32>,
    /// Items dropped on our slots, `(1-based trade slot, bag, slot)`, and the slots clicks cleared.
    pub(crate) trade_set_items: Vec<(u32, i64, u32)>,
    pub(crate) trade_clear_items: Vec<u32>,

    /// The open questgiver panel, its greeting-row selects and its button calls.
    pub(crate) quest: Option<quest::QuestState>,
    pub(crate) quest_selects: Vec<quest::QuestSelect>,
    pub(crate) quest_actions: Vec<quest::QuestAction>,

    /// Death countdowns and offers, and the release, reclaim and resurrect calls.
    pub(crate) death: death::DeathUiState,
    pub(crate) death_actions: Vec<death::DeathAction>,

    /// The quest log, selection (1-based), abandon mark (a quest id, `0xbb7484`) and abandons.
    pub(crate) quest_log: quest_log::QuestLogState,
    pub(crate) quest_log_selection: u32,
    pub(crate) quest_log_abandon_mark: u32,
    pub(crate) quest_log_abandons: Vec<u32>,
    /// `QuestLogPushQuest()` quest ids, resolved at the click so a log change cannot retarget one.
    pub(crate) quest_log_pushes: Vec<u32>,
    /// `ConfirmAcceptQuest()` calls, the escort confirm; the app holds the pending quest.
    pub(crate) quest_confirms: u32,
    /// Item id to its tooltip view.
    pub(crate) item_templates: HashMap<u32, ItemTemplateView>,
    /// Item ids asked for and missing, for the app to answer.
    pub(crate) item_stat_asks: HashSet<u32>,
    /// Set id to its tooltip set block (`0x854b1c`), and the misses asked for.
    pub(crate) item_sets: HashMap<u32, super::ItemSetView>,
    pub(crate) item_set_asks: HashSet<u32>,
    /// `ItemRandomProperties` id to its view, pushed whole at startup: a clicked tooltip cannot
    /// repaint on a late answer. The reference reads its loaded store `0xc0dbd4` (`0x52c991`).
    pub(crate) random_properties: HashMap<u32, super::RandomPropertyView>,
    /// The player's level, class, race and skills, for a tooltip's red requirement lines.
    pub(crate) player_req: PlayerReqState,
    /// Spell id to its tooltip view, and the misses asked for.
    pub(crate) spell_tooltips: HashMap<u32, super::SpellTooltipView>,
    pub(crate) spell_tooltip_asks: HashSet<u32>,
    /// `CollapseQuestHeader`/`ExpandQuestHeader` as `(1-based entry, collapse)`, entry 0 for all.
    pub(crate) quest_log_collapses: Vec<(u32, bool)>,
    /// Watched quest ids in watch order, pruned when a quest leaves the log.
    pub(crate) quest_log_watched: Vec<u32>,
    /// The server's clock in Unix seconds, `None` before `SMSG_QUERY_TIME_RESPONSE`. A quest
    /// deadline is a stamp in it, and `GetQuestTimers` subtracts per call, as the reference does.
    pub(crate) server_unix_time: Option<f64>,
    /// `GetMoney()` in copper, from `PLAYER_FIELD_COINAGE`.
    pub(crate) money: u64,

    /// `GetNetStats`'s third return, the average round trip in ms; 0 before any sample.
    pub(crate) net_latency_ms: u32,

    /// `UnitXP("player")` and `UnitXPMax("player")`, from the private `PLAYER_XP` fields.
    pub(crate) player_xp: u32,
    pub(crate) player_next_level_xp: u32,

    /// Raw combo points (`PLAYER_FIELD_BYTES` byte 1) and target, ungated: a warrior banks them too
    /// (Overpower); `GetComboPoints` (`0x51a190`) applies the class and current-target gates.
    pub(crate) combo_points: u8,
    pub(crate) combo_target: u64,

    /// `PLAYER_BYTES_2` byte 3 (1 rested, 2 normal), defaulting to 2 as every 1.12 character has
    /// it: a 0 has no Exhaustion row, and `GetRestState()`'s nil would raise at the unguarded
    /// `>= 3` of `MainMenuBar.lua:25`. `rest_pool` is in base kill-XP units, and `resting` is
    /// `PLAYER_FLAGS_RESTING` (0x20).
    pub(crate) rest_state: u8,
    pub(crate) rest_pool: u32,
    pub(crate) resting: bool,

    /// `PLAYER_FLAGS` bit 12, `PartialPlayTime()` (`0x48eb70`), an anti-addiction regime's flag.
    pub(crate) partial_play_time: bool,

    /// `PLAYER_FLAGS` bit 13, `NoPlayTime()` (`0x48ebe0`), the same regime's harsher half.
    pub(crate) no_play_time: bool,

    /// `GetBillingTimeRested()` (`0x48ec50`) in minutes from `SMSG_AUTH_RESPONSE`, unconverted.
    pub(crate) billing_time_rested: u32,
    /// `InCinematic()`. While set, `StaticPopup_Show` refuses any dialog without
    /// `interruptCinematic` (`StaticPopup.lua:1454`).
    pub(crate) in_cinematic: bool,
    /// `Exhaustion.dbc` by rest byte, `(name, factor)`: `GetRestState` indexes it and row 1 scales
    /// `GetXPExhaustion` (`0x48d3f0`). enUS rows seed it until the app loads the install's own.
    pub(crate) exhaustion: HashMap<u8, (String, f64)>,

    /// The paper doll's combat stats, `None` until the first push.
    pub(crate) player_combat_stats: Option<char_stats::UnitCombatStats>,
    /// The pet's combat stats, which the reference's pet sheet reads as `UnitStat("pet", …)`.
    pub(crate) pet_combat_stats: Option<char_stats::UnitCombatStats>,
    pub(crate) inventory_slots: char_stats::InventorySlots,
    /// The six bank-bag slots, ids 64..=69, fed from the player descriptor's guids.
    pub(crate) bank_bag_slots: char_stats::BankBagSlots,
    /// `GetInventoryAlertStatus` in `0x806eb8` order; every push fires `UPDATE_INVENTORY_ALERTS`.
    pub(crate) inventory_alerts: [u8; 12],
    /// `UseInventoryItem` slot ids, sent as `CMSG_USE_ITEM` on the equipped item.
    pub(crate) inventory_uses: Vec<u32>,
    /// Equipped slot ids clicked while the merchant repair cursor is armed.
    pub(crate) inventory_repairs: Vec<u32>,
    /// Main- and off-hand temporary enchants in `GetWeaponEnchantInfo`'s order, pushed each frame.
    pub(crate) weapon_enchants: [Option<weapon_enchant::WeaponEnchant>; 2],

    /// The inspected unit's gear, from its public `PLAYER_VISIBLE_ITEM_*` fields.
    pub(crate) inspect: Option<inspect::InspectView>,
    /// `NotifyInspect` unit tokens, sent as `CMSG_INSPECT`.
    pub(crate) inspect_notifies: Vec<String>,
    /// `ClearInspectPlayer` ran; the app drops its inspect target.
    pub(crate) inspect_clear: bool,
    /// `DressUpModel` `SetUnit`, `Dress`, `Undress` and `TryOn` calls, applied in call order.
    pub(crate) dressup_intents: Vec<super::dressup::DressUpIntent>,
    /// Tabard designs per `TabardModel`, the app's preview five, the host facts and queued calls.
    pub(crate) tabard_designs: HashMap<crate::widget::FrameHandle, [i32; 5]>,
    pub(crate) tabard_preview: Option<[i32; 5]>,
    pub(crate) tabard_host: super::tabard::TabardHost,
    pub(crate) tabard_intents: Vec<super::tabard::TabardIntent>,
    /// The `WorldFrame` type's one-shot record: true once the first is made.
    pub(crate) world_frame_made: bool,
    /// Per lowercased live-unit token, reach for `CanInspect`/`CheckInteractDistance`; absent: nil.
    pub(crate) unit_reach: HashMap<String, super::UnitReach>,

    /// The skills pane's snapshot and the display tree built from it.
    pub(crate) skills: skills::SkillsState,
    pub(crate) skills_groups: Vec<skills::SkillGroup>,
    /// Collapsed skill categories by id, kept across a re-push.
    pub(crate) skills_collapsed: HashSet<u32>,
    /// The selection as a skill id, not a row.
    pub(crate) skills_selected: Option<u32>,
    /// `AbandonSkill` ids for `CMSG_UNLEARN_SKILL`; nothing changes until the server's update.
    pub(crate) skill_abandons: Vec<u32>,

    /// The reputation pane's snapshot and the display tree built from it.
    pub(crate) reputation: reputation::ReputationState,
    pub(crate) reputation_groups: Vec<reputation::FactionGroup>,
    /// Folded headers by key (a `Faction.dbc` id, 0 "Other", -1 "Inactive"), reset on every push
    /// to all open but Inactive, as the client's rebuild does.
    pub(crate) reputation_collapsed: HashSet<i64>,
    /// The selection as a reputation-list slot, not a row.
    pub(crate) reputation_selected: Option<u32>,
    /// Reputation calls queued, applied locally first because none of the three sends is acked.
    pub(crate) reputation_sends: Vec<reputation::ReputationSend>,

    /// Lines the chat box submitted (`SubmitChatInput`), for the app's slash-command parser.
    pub(crate) chat_input: Vec<String>,

    /// The world map's pushed catalog and feed, and its selection.
    pub(crate) worldmap: super::worldmap::WorldMapState,
    /// Nameplate `Button`s under `WorldFrame`, never shrunk, visible to `GetChildren()`.
    pub(crate) nameplates: super::nameplate::NamePlates,
    /// The world-state readout's rows, gated and resolved by the app.
    pub(crate) worldstate: super::worldstate::WorldStateUiState,
    /// Events bindings raise. Deviation: fired at the next tick, not synchronously as in the
    /// reference, because a binding runs inside Lua and cannot re-enter handler dispatch.
    pub(crate) pending_events: Vec<(String, Vec<ScriptValue>)>,
    /// The last cursor position in UI space (logical px, y-up), behind `GetCursorPosition()`.
    pub(crate) cursor_pos: (f32, f32),

    /// A `Minimap:PingLocation(x, y)`, drained the same frame: centre-relative offsets in UI units,
    /// `GetCursorPosition`'s space, not the window pixels of the app's minimap geometry.
    pub(crate) minimap_ping_request: Option<(f32, f32)>,
    /// The ping's offsets as fractions of the minimap's side, updated by the app every frame.
    /// `GetPingPosition()` always answers both, from statics never cleared (`0x4eefd0`).
    pub(crate) minimap_ping: (f32, f32),
    /// `SendChatMessage` lines, apart from chat-box input, whose drain runs the slash grammar.
    pub(crate) chat_sends: Vec<super::chat_send::ChatSend>,
    /// `SendAddonMessage` broadcasts: `LANG_ADDON`, four distributions, `prefix` TAB `message`.
    pub(crate) addon_sends: Vec<super::addon_message::AddonSend>,
    /// `RequestTimePlayed()` calls, one empty `CMSG_PLAYED_TIME` each.
    pub(crate) played_time_asks: u32,
    /// `Screenshot()` calls, one capture each.
    pub(crate) screenshot_asks: u32,
    /// `GetRealmName()`: `""` until pushed, never nil, since addons index tables with it at load.
    pub(crate) realm_name: String,
    /// The local player record (`0xc27d80`), seeded at world entry before addons load, then kept by
    /// `"player"` pushes that carry it: corrected, never blanked. A per-login rebuild is unseen, as
    /// its verbs exist only in game (`0x850438`), not at the glue screen (`0x46abb0`).
    pub(crate) player_record: super::PlayerRecord,
    /// `GetBindLocation()`, the bind point's area name: `""` before it lands, never nil.
    pub(crate) bind_location: String,
    /// `GMTicketCategory.dbc` `(id, name)` rows in file order; the ids are wire values.
    pub(crate) gm_ticket_categories: Vec<(u32, String)>,
    /// GM ticket calls in call order, so a delete then a get reach the wire in that order.
    pub(crate) gm_ticket_intents: Vec<super::gm_ticket::GmTicketIntent>,
    /// `Stuck()` calls, each a cast of spell 7355, the Help window's Auto-Unstuck.
    pub(crate) stuck_casts: u32,
    /// `ConfirmBinder()` calls, one `CMSG_BINDER_ACTIVATE` each; the app holds the innkeeper.
    pub(crate) binder_confirms: u32,
    /// `CheckBinderDist()`: the innkeeper's question is live and in range; false hides its dialog.
    pub(crate) binder_pending: bool,
    /// The summon getters' answers, defaulting to the reference's `""`, `""`, 0 with none pending.
    pub(crate) summon_confirm: super::summon::SummonConfirmUiState,
    /// `ConfirmSummon()` calls, one `CMSG_SUMMON_RESPONSE` each; the app holds the summoner.
    pub(crate) summon_confirms: u32,
    /// `GetFramerate()`, pushed each tick.
    pub(crate) framerate: f64,
    /// `(shift, ctrl, alt)` for `Is*KeyDown`, pushed before the frame's mouse events.
    pub(crate) modifiers: (bool, bool, bool),
}

impl Model {
    /// The selected quest-log row's detail, resolved per call as the reference's bindings do.
    pub(crate) fn selected_quest_detail(&self) -> Option<&quest_log::QuestLogDetail> {
        let index = usize::try_from(self.quest_log_selection.checked_sub(1)?).ok()?;
        self.quest_log.entries.get(index)?.detail.as_ref()
    }
}

impl Model {
    /// A named font object, matched case-insensitively (ASCII) as the client's `SStrCmpI` does.
    pub(crate) fn font_object(&self, name: &str) -> Option<&FontObject> {
        if name.bytes().any(|b| b.is_ascii_uppercase()) {
            self.font_objects_by_lower.get(&name.to_ascii_lowercase())
        } else {
            self.font_objects_by_lower.get(name)
        }
    }

    /// Records a caught script error for the host, the retained log and the Lua error handler; an
    /// error raised by the handler itself goes to `errors` directly, which stops the recursion.
    pub(crate) fn record_script_error(&mut self, msg: String) {
        self.diagnostics
            .record(super::diagnostics::DiagnosticKind::Error, &msg);
        self.pending_error_dispatch.push(msg.clone());
        self.errors.push(msg);
    }

    /// Records a non-fatal warning for the host's terminal and the retained diagnostic log.
    pub(crate) fn record_warning(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.diagnostics
            .record(super::diagnostics::DiagnosticKind::Warning, &msg);
        self.warnings.push(msg);
    }

    /// A warning for the host's terminal only, for a message already retained under another kind.
    pub(crate) fn warn_host_only(&mut self, msg: String) {
        self.warnings.push(msg);
    }

    /// A unit token's state, folded as 1.12's resolver `0x515970` folds (`_strnicmp`: `A`..`Z`
    /// only, never a byte of 0x80 or more), so `to_ascii_lowercase`, never `to_lowercase`.
    pub(crate) fn unit(&self, token: &str) -> Option<&UnitState> {
        if token.bytes().any(|b| b.is_ascii_uppercase()) {
            self.units_by_lower.get(&token.to_ascii_lowercase())
        } else {
            self.units_by_lower.get(token)
        }
    }

    pub(crate) fn new() -> Model {
        Model {
            addons: Vec::new(),
            addon_index: Vec::new(),
            addon_info_hidden: None,
            addons_root: None,
            addons_chain_reader: None,
            measurer: None,
            texture_probe: None,
            texture_size_probe: None,
            font_probe: None,
            addons_saved_account: None,
            addons_saved_character: None,
            framexml_templates: Default::default(),
            framexml_debug: Default::default(),
            framexml_fonts: Default::default(),
            arena: WidgetArena::new(),
            layout_inputs: HashMap::new(),
            solver: LayoutSolver::new(),
            layout_fingerprint: None,
            layout_epoch: 0,
            layout_touched: None,
            measure_dirty: None,
            msg_swept: std::collections::HashMap::new(),
            msg_lines_hashed: 0,
            layout_verify_recheck: false,
            layout_derives: 0,
            layout_scope: super::layout::LayoutScope::default(),
            layout_last_scope: (0, 0),
            layout_epoch_resolved: None,
            layout_solves: 0,
            layout_gate_walks: 0,
            layout_rounds: 0,
            resolved: HashMap::new(),
            link_spans: HashMap::new(),
            chat_tab: false,
            region_data: HashMap::new(),
            backdrops: HashMap::new(),
            simple_html: simplehtml::SimpleHtmlStates::new(),
            font_objects_by_lower: HashMap::new(),
            region_resolved: HashMap::new(),
            next_id: 1,
            id_to_frame: IdMap::default(),
            frame_to_id: HashMap::new(),
            id_to_region: IdMap::default(),
            region_to_id: HashMap::new(),
            region_names: HashMap::new(),
            scripts: HashMap::new(),
            on_update_frames: Vec::new(),
            on_size_changed_frames: Vec::new(),
            on_update_model_frames: Vec::new(),
            model_facts: HashMap::new(),
            model_facts_wanted: Vec::new(),
            dirty_editboxes: Vec::new(),
            event_to_frames: HashMap::new(),
            frame_events: HashMap::new(),
            all_event_frames: Vec::new(),
            focused_editbox: None,
            mouseover: None,
            hover_repick: false,
            mouse_down_on: HashMap::new(),
            mouse_capture: None,
            last_click: HashMap::new(),
            pending_size_changed: Vec::new(),
            errors: Vec::new(),
            pending_error_dispatch: Vec::new(),
            diagnostics: Default::default(),
            warnings: Vec::new(),
            // 1024x768 until the host calls `set_screen_size`; y-up `[bottom, left, top, right]`.
            screen: Rect::new(0.0, 0.0, 768.0, 1024.0),
            units_by_lower: HashMap::new(),
            auras: HashMap::new(),
            cancel_aura_requests: Vec::new(),
            tracking: None,
            selection_requests: Vec::new(),
            target_nearest_friend_requests: Vec::new(),
            target_by_name_requests: Vec::new(),
            drop_item_on_unit: Vec::new(),
            spell_target_unit: Vec::new(),
            target_clear: false,
            joined_channels: Vec::new(),
            party: party::PartyState::default(),
            party_requests: Vec::new(),
            ready_check: party::ReadyCheckState::default(),
            saved_instances: Vec::new(),
            raid_selection: 0,
            instance_type: None,
            can_reset_instances: false,
            reset_instance_asks: 0,
            social: social::SocialState::default(),
            social_requests: Vec::new(),
            lfg_slots: [0; 3],
            lfg_comment: String::new(),
            guild: guild::GuildState::default(),
            guild_control: guild::GuildRankEdit::default(),
            guild_requests: Vec::new(),
            petition: petition::PetitionState::default(),
            petition_requests: Vec::new(),
            // Per window, as the stock dock differs: 1 and 2 are the dock, the rest undocked.
            chat_window_looks: std::array::from_fn(chat_window::ChatWindowLook::stock),
            chat_window_changes: HashSet::new(),
            chat_colors: super::chat_types::seed(),
            chat_colors_changed: false,
            known_languages: Vec::new(),
            zone_channel_catalog: Vec::new(),
            channel_commands: Vec::new(),
            guild_recruitment_mode: 1,
            guild_recruitment_cascade: false,
            guild_recruitment_changed: false,
            emote_requests: Vec::new(),
            roll_requests: Vec::new(),
            uninvite_requests: Vec::new(),
            console_lines: Vec::new(),
            logging_chat: false,
            logging_combat: false,
            logging_changed: false,
            user_placed_changed: false,
            default_language: None,
            duel_requests: Vec::new(),
            follow_requests: Vec::new(),
            camera_view_requests: Vec::new(),
            session_requests: Vec::new(),
            pvp_toggles: 0,
            honor: None,
            inspect_honor: None,
            inspect_honor_requests: 0,
            inspect_honor_pending: false,
            helm_shown: true,
            cloak_shown: true,
            worn_display_toggles: Vec::new(),
            action_bar_toggles: None,
            action_bar_toggle_sends: Vec::new(),
            sound_queue: Vec::new(),
            music_queue: Vec::new(),
            sound_suppression: 0,
            cvars: HashMap::new(),
            cvars_saved_base: HashMap::new(),
            cvar_changes: Vec::new(),
            cvar_registrations: Vec::new(),
            cvars_warned: HashSet::new(),
            multisample_formats: Vec::new(),
            screen_resolutions: Vec::new(),
            current_resolution: None,
            video_caps: super::cvars::VideoCaps::default(),
            restart_gx_asks: 0,
            saved_names: Vec::new(),
            held_saved_files: Vec::new(),
            keybinds: super::keybind::KeybindState::default(),
            actions: HashMap::new(),
            action_states: HashMap::new(),
            bonus_bar_offset: 0,
            action_uses: Vec::new(),
            action_sets: Vec::new(),
            ui_errors: Vec::new(),
            spellbook: spellbook::SpellBookState::default(),
            pet_book: spellbook::PetBookState::default(),
            macros: macros::MacroState::default(),
            macros_dirty: false,
            macros_generation: 0,
            macro_icons: Vec::new(),
            spell_casts: Vec::new(),
            pet_spell_casts: Vec::new(),
            pet_spell_autocasts: Vec::new(),
            casting: false,
            spell_stop: false,
            spell_targeting: false,
            spell_targetable_units: HashSet::new(),
            spell_stop_targeting: false,
            talents: super::talent::TalentUiState::default(),
            talent_learns: Vec::new(),
            talent_wipe_confirms: 0,
            talent_master_pending: false,
            pet_unlearn_confirms: 0,
            pet_untrainer_pending: false,
            instance_boot_secs: 0,
            area_spirit_healer_cached: false,
            area_spirit_secs: 0,
            area_spirit_accepts: 0,
            battlefield_port_requests: Vec::new(),
            battlefield_board: Default::default(),
            battlefield_run_time_ms: 0,
            battlefield_score_requests: 0,
            battlefield_leave_requests: 0,
            battlefield_list: Default::default(),
            battlefield_selected: 0,
            battlefield_slots: Vec::new(),
            battlefield_instance_expiration_ms: 0,
            battlefield_join_requests: Vec::new(),
            battlefield_list_requests: Vec::new(),
            battlefield_positions: Vec::new(),
            battlefield_flag: None,
            battlefield_icon_scale: 1.0,
            battlefield_position_requests: 0,
            meeting_stone_cancels: 0,
            meeting_stone_area: 0,
            meeting_stone_status_text: None,
            tutorial_bank: None,
            tutorial_flag_requests: Vec::new(),
            tutorial_clears: 0,
            tutorial_resets: 0,
            shapeshift_forms: Vec::new(),
            shapeshift_casts: Vec::new(),
            pet_bar: super::pet::PetBarState::default(),
            pet_actions_pressed: Vec::new(),
            pet_autocast_toggles: Vec::new(),
            pet_stop_attacks: 0,
            pet_orders: Vec::new(),
            player_control: true,
            pet_set_actions: Vec::new(),
            pet_abandons: 0,
            pet_dismisses: 0,
            pet_renames: Vec::new(),
            containers: HashMap::new(),
            container_uses: Vec::new(),
            container_cooldowns: HashMap::new(),
            has_key: false,
            cursor: None,
            cursor_grid_shown: false,
            pet_grid_shown: false,
            world_pick: cursor::WorldPick::default(),
            container_moves: Vec::new(),
            container_repairs: Vec::new(),
            item_pick_armed: false,
            item_picks: Vec::new(),
            enchant_confirms: Vec::new(),
            container_destroys: Vec::new(),
            pending_wrap: None,
            container_wraps: Vec::new(),
            ui_cursor: None,
            ui_cursor_dirty: false,
            container_autoequips: Vec::new(),
            bag_autostores: Vec::new(),
            drag_registered: HashMap::new(),
            drag: None,
            moving: None,
            sizing: None,
            slider_drag: None,
            color_drag: None,
            gossip: None,
            gossip_selects: Vec::new(),
            gossip_close: false,
            gossip_quest_selects: Vec::new(),
            merchant: None,
            repair_all_cost: 0,
            merchant_buys: Vec::new(),
            merchant_cursor_sells: Vec::new(),
            merchant_slot_buys: Vec::new(),
            merchant_close: false,
            merchant_buybacks: Vec::new(),
            repair_all: false,
            repair_mode: false,
            stable: Default::default(),
            bank: None,
            bank_purchase: false,
            bank_close: false,
            trainer: None,
            trainer_buys: Vec::new(),
            trainer_selection: None,
            trainer_close: false,
            trainer_filter: [true; 3],
            trainer_collapsed: HashSet::new(),
            taxi: None,
            taxi_takes: Vec::new(),
            taxi_close: false,
            taxi_riding: false,
            trade_skill: None,
            trade_skill_dos: Vec::new(),
            trade_skill_selection: 0,
            trade_skill_close: false,
            trade_skill_collapsed: HashSet::new(),
            trade_skill_subclass_hidden: HashSet::new(),
            trade_skill_invslot_mask: u32::MAX,
            trade_skill_last_line: 0,
            trade_skill_selected_spell: 0,
            trade_skill_touched: false,
            craft: None,
            craft_dos: Vec::new(),
            craft_selection: 0,
            craft_close: false,
            loot: None,
            pending_equip_answers: Vec::new(),
            bind_on_use_confirms: 0,
            loot_picks: Vec::new(),
            loot_confirms: Vec::new(),
            loot_close: false,
            loot_master_gives: Vec::new(),
            loot_rolls: loot_roll::LootRollsState::default(),
            loot_roll_votes: Vec::new(),
            loot_roll_confirms: Vec::new(),
            item_text: None,
            item_text_close: false,
            item_text_page_turns: Vec::new(),
            mail: None,
            mail_check_inbox: false,
            mail_opens: Vec::new(),
            mail_take_items: Vec::new(),
            mail_take_money: Vec::new(),
            mail_deletes: Vec::new(),
            mail_returns: Vec::new(),
            mail_take_texts: Vec::new(),
            mail_close: false,
            mail_send: None,
            mail_send_money: 0,
            mail_send_cod: 0,
            mail_send_item: None,
            mail_stationeries: Vec::new(),
            mail_stationery: 0,
            has_new_mail: false,
            auction: None,
            auction_item_classes: Vec::new(),
            auction_selected: [0; 3],
            auction_can_query: true,
            auction_query: None,
            auction_owner_query: None,
            auction_bidder_query: None,
            auction_bids: Vec::new(),
            auction_cancels: Vec::new(),
            auction_start: None,
            auction_sorts: Vec::new(),
            auction_close: false,
            auction_sell_item: None,
            trade: None,
            trade_initiates: Vec::new(),
            trade_accept: false,
            trade_unaccept: false,
            trade_close: false,
            trade_begin: false,
            trade_cancel: false,
            trade_set_money: None,
            trade_set_items: Vec::new(),
            trade_clear_items: Vec::new(),
            quest: None,
            quest_selects: Vec::new(),
            quest_actions: Vec::new(),
            death: death::DeathUiState::default(),
            death_actions: Vec::new(),
            quest_log: quest_log::QuestLogState::default(),
            quest_log_selection: 0,
            quest_log_abandon_mark: 0,
            quest_log_abandons: Vec::new(),
            quest_log_pushes: Vec::new(),
            quest_confirms: 0,
            item_templates: HashMap::new(),
            item_sets: HashMap::new(),
            item_set_asks: HashSet::new(),
            random_properties: HashMap::new(),
            item_stat_asks: HashSet::new(),
            player_req: PlayerReqState::default(),
            spell_tooltips: HashMap::new(),
            spell_tooltip_asks: HashSet::new(),
            quest_log_collapses: Vec::new(),
            quest_log_watched: Vec::new(),
            server_unix_time: None,
            worldmap: super::worldmap::WorldMapState::default(),
            nameplates: super::nameplate::NamePlates::default(),
            worldstate: super::worldstate::WorldStateUiState::default(),
            pending_events: Vec::new(),
            cursor_pos: (0.0, 0.0),
            minimap_ping_request: None,
            minimap_ping: (0.0, 0.0),
            chat_sends: Vec::new(),
            addon_sends: Vec::new(),
            played_time_asks: 0,
            screenshot_asks: 0,
            realm_name: String::new(),
            player_record: super::PlayerRecord::default(),
            bind_location: String::new(),
            gm_ticket_categories: Vec::new(),
            gm_ticket_intents: Vec::new(),
            stuck_casts: 0,
            binder_confirms: 0,
            binder_pending: false,
            summon_confirm: super::summon::SummonConfirmUiState::default(),
            summon_confirms: 0,
            framerate: 0.0,
            modifiers: (false, false, false),
            money: 0,
            net_latency_ms: 0,
            player_xp: 0,
            player_next_level_xp: 0,
            combo_points: 0,
            combo_target: 0,
            rest_state: 2,
            rest_pool: 0,
            resting: false,
            partial_play_time: false,
            no_play_time: false,
            billing_time_rested: 0,
            in_cinematic: false,
            exhaustion: [
                (1, ("Rested".to_string(), 2.0)),
                (2, ("Normal".to_string(), 1.0)),
                (3, ("XXXTired".to_string(), 1.0)),
                (4, ("XXXTired".to_string(), 0.5)),
                (5, ("XXXExhausted".to_string(), 0.25)),
            ]
            .into_iter()
            .collect(),
            player_combat_stats: None,
            pet_combat_stats: None,
            inventory_slots: Default::default(),
            bank_bag_slots: Default::default(),
            inventory_alerts: [0; 12],
            inventory_uses: Vec::new(),
            inventory_repairs: Vec::new(),
            weapon_enchants: [None; 2],
            inspect: None,
            inspect_notifies: Vec::new(),
            inspect_clear: false,
            dressup_intents: Vec::new(),
            tabard_designs: HashMap::new(),
            tabard_preview: None,
            tabard_host: Default::default(),
            tabard_intents: Vec::new(),
            world_frame_made: false,
            unit_reach: HashMap::new(),
            chat_input: Vec::new(),
            skills: skills::SkillsState::default(),
            skills_groups: Vec::new(),
            skills_collapsed: HashSet::new(),
            skills_selected: None,
            skill_abandons: Vec::new(),
            reputation: reputation::ReputationState::default(),
            reputation_groups: Vec::new(),
            reputation_collapsed: HashSet::new(),
            reputation_selected: None,
            reputation_sends: Vec::new(),
        }
    }

    /// Mint (or fetch) the stable id of a frame handle.
    pub(crate) fn frame_id(&mut self, h: FrameHandle) -> u32 {
        if let Some(&id) = self.frame_to_id.get(&h) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.frame_to_id.insert(h, id);
        self.id_to_frame.insert(id, h);
        // Every creation path mints here, so a new frame always opens the layout gate.
        self.touch_layout();
        id
    }

    /// Mint (or fetch) the stable id of a region handle.
    pub(crate) fn region_id(&mut self, h: RegionHandle) -> u32 {
        if let Some(&id) = self.region_to_id.get(&h) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.region_to_id.insert(h, id);
        self.id_to_region.insert(id, h);
        // A new region can join the resolve's external set, so it opens the gate too.
        self.touch_layout();
        id
    }

    /// A layout write that names no node. The resolve's tooltip pre-pass must not call this, or
    /// the gate never closes; a missed call is caught by `WOW_LAYOUT_VERIFY`.
    pub(crate) fn touch_layout(&mut self) {
        // Naming nothing, the next resolve derives the graph in full: slow, never wrong.
        self.give_up_naming();
        self.bump_layout_epoch();
    }

    /// A write that moved one region's offsets, size or measured extent, not its targets or
    /// liveness; a region missing from the cached graph falls back to `touch_layout`.
    pub(crate) fn touch_layout_region(&mut self, rh: RegionHandle) {
        match self.region_to_id.get(&rh) {
            Some(&id) => self.touch_layout_node(id),
            None => self.touch_layout(),
        }
    }

    /// The same for a frame: an anchor offset or size, nothing that moves an edge or the roster.
    pub(crate) fn touch_layout_frame(&mut self, h: FrameHandle) {
        match self.frame_to_id.get(&h) {
            Some(&id) => self.touch_layout_node(id),
            None => self.touch_layout(),
        }
    }

    /// A `SetParent` under `root`: only the subtree's scale moves, as anchor targets are ids fixed
    /// at `SetPoint`. Addon pools reparent hundreds of frames a redraw, so this stays precise.
    pub(crate) fn touch_layout_reparent(&mut self, root: FrameHandle) {
        let mut subtree = vec![root];
        let mut at = 0;
        while at < subtree.len() {
            let h = subtree[at];
            at += 1;
            if let Some(f) = self.arena.frame(h) {
                subtree.extend(f.children.iter().copied());
            }
        }
        // Write the new scale here: only a full derivation syncs it, and a named node skips that.
        let Model {
            arena,
            layout_inputs,
            ..
        } = self;
        for &h in &subtree {
            if let Some(f) = arena.frame(h) {
                layout_inputs.entry(h).or_default().scale = f.effective_scale;
            }
        }
        for h in subtree {
            self.touch_layout_frame(h);
        }
    }

    /// A write changed frame `h`'s anchor targets; `old` and `new` are its full target lists, so
    /// the cached edges are re-pointed. Falls back to `touch_layout` when the scope refuses.
    pub(crate) fn touch_layout_retarget_frame(&mut self, h: FrameHandle, old: &[u32], new: &[u32]) {
        match self.frame_to_id.get(&h).copied() {
            // Already conservative: the next resolve rebuilds every edge, so patching is wasted.
            Some(id)
                if self.layout_touched.is_some() && self.layout_scope.retarget(id, old, new) =>
            {
                self.touch_layout_node(id);
            }
            _ => self.touch_layout(),
        }
    }

    /// `touch_layout_retarget_frame` for a region.
    pub(crate) fn touch_layout_retarget_region(
        &mut self,
        rh: RegionHandle,
        old: &[u32],
        new: &[u32],
    ) {
        match self.region_to_id.get(&rh).copied() {
            Some(id)
                if self.layout_touched.is_some() && self.layout_scope.retarget(id, old, new) =>
            {
                self.touch_layout_node(id);
            }
            _ => self.touch_layout(),
        }
    }

    /// Names `rh` on the measure ledger for a write that may move its measure key.
    pub(crate) fn touch_measure(&mut self, rh: RegionHandle) {
        if let Some(list) = &mut self.measure_dirty {
            list.push(rh);
        }
    }

    /// A measure-key write that names no region, such as a font object: the next sweep walks all.
    pub(crate) fn touch_measure_all(&mut self) {
        self.measure_dirty = None;
    }

    /// Names a frame without bumping the epoch, for the resolve's own tooltip pre-pass: a bump
    /// inside a resolve would hold the gate open for good, yet an unnamed write could go stale.
    pub(crate) fn note_layout_frame_write(&mut self, h: FrameHandle) {
        if self.layout_touched.is_some() {
            match self.frame_to_id.get(&h) {
                Some(&id) if self.layout_scope.has_node(id) => {
                    self.layout_touched
                        .as_mut()
                        .expect("checked above")
                        .push(id);
                }
                _ => self.give_up_naming(),
            }
        }
    }

    /// `note_layout_frame_write` for a region.
    pub(crate) fn note_layout_region_write(&mut self, rh: RegionHandle) {
        if self.layout_touched.is_some() {
            match self.region_to_id.get(&rh) {
                Some(&id) if self.layout_scope.has_node(id) => {
                    self.layout_touched
                        .as_mut()
                        .expect("checked above")
                        .push(id);
                }
                _ => self.give_up_naming(),
            }
        }
    }

    /// Names `id` as what this write moved, if the cached graph has it and naming is still on.
    fn touch_layout_node(&mut self, id: u32) {
        let in_graph = self.layout_scope.has_node(id);
        match &mut self.layout_touched {
            // Already conservative: a precise touch cannot un-say an imprecise one.
            None => {}
            Some(_) if !in_graph => self.give_up_naming(),
            Some(list) => list.push(id),
        }
        self.bump_layout_epoch();
    }

    /// Every write that gives up naming comes through here, so the next resolve derives in full.
    /// `WOW_LAYOUT_DERIVE_TRACE=<secs>:<n>` backtraces the first `n` of these after `secs`.
    fn give_up_naming(&mut self) {
        if self.layout_touched.is_some() && derive_trace_armed() {
            eprintln!(
                "[layout-derive] the ledger gave up naming, at:\n{}",
                std::backtrace::Backtrace::force_capture()
            );
        }
        self.layout_touched = None;
    }

    /// Bumps tier 1's counter, reopening the gate; `layout_touched` decides how much it derives.
    fn bump_layout_epoch(&mut self) {
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
        // `WOW_LAYOUT_TOUCH_TRACE=<secs>:<n>`: backtraces the first `n` touches after `secs`.
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::OnceLock;
        static SPEC: OnceLock<Option<(std::time::Instant, f64, AtomicU32)>> = OnceLock::new();
        let spec = SPEC.get_or_init(|| {
            let v = std::env::var("WOW_LAYOUT_TOUCH_TRACE").ok()?;
            let (secs, n) = v.split_once(':')?;
            Some((
                std::time::Instant::now(),
                secs.trim().parse().ok()?,
                AtomicU32::new(n.trim().parse().ok()?),
            ))
        });
        if let Some((t0, delay, left)) = spec {
            if t0.elapsed().as_secs_f64() >= *delay
                && left
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1))
                    .is_ok()
            {
                eprintln!(
                    "[layout-touch] epoch={} at:\n{}",
                    self.layout_epoch,
                    std::backtrace::Backtrace::force_capture()
                );
            }
        }
    }
}

/// `WOW_LAYOUT_DERIVE_TRACE=<secs>:<n>`: armed after `secs`, past the UI load, then `n` prints.
fn derive_trace_armed() -> bool {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::OnceLock;
    static SPEC: OnceLock<Option<(std::time::Instant, f64, AtomicU32)>> = OnceLock::new();
    let Some((t0, delay, left)) = SPEC.get_or_init(|| {
        let v = std::env::var("WOW_LAYOUT_DERIVE_TRACE").ok()?;
        let (secs, n) = v.split_once(':')?;
        Some((
            std::time::Instant::now(),
            secs.trim().parse().ok()?,
            AtomicU32::new(n.trim().parse().ok()?),
        ))
    }) else {
        return false;
    };
    t0.elapsed().as_secs_f64() >= *delay
        && left
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1))
            .is_ok()
}
