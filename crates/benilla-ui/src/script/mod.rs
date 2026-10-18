//! The Lua scripting host: the engine-free VM that runs FrameXML and addons over the frame arena,
//! the anchor resolver and the draw order. It embeds benilla-lua through benilla-mlua's
//! mlua-shaped API: 5.1.5's runtime under the 1.12 grammar, reshaped by [`lua50`] to answer as
//! the 1.12 client's Lua 5.0.
//!
//! A frame's Lua value is a table whose `T[0]` lightuserdata (`0x701bd0`) holds a `u32` id where
//! the reference holds the `CScriptObject*`, with an `__index` metatable (`0x7020b0`). A handler
//! is `pcall`ed with `this`, `event` and `arg1..argN` set as globals and restored after
//! (`0x704d50`); it also gets `(self, event, ...)` as arguments, which 1.12 does not pass.
//!
//! The handle discipline: Rust holds no persistent Lua handle, since each owned `Table` or
//! `Function` is a collector root that keeps everything it reaches alive. Lua-side
//! state lives in the registry under named keys, fetched per call; [`Model`] is plain data in
//! `lua.app_data`, borrowed briefly and released before re-entering Lua.

mod action;
mod action_bar_toggles;
mod addon_message;
mod auction;
mod aura;
mod backdrop;
mod bank;
mod battlefield_positions;
mod battlefield_queue;
mod battlefield_score;
mod bind_confirm;
mod binder;
pub(crate) mod binding_abi;
mod dialog_verbs;
mod tutorial;
mod worldmap_arrow;
// `camera_view`: the five camera views and `FlipCameraYaw`, the reference's `UIUtil\Camera.cpp`.
mod button;
mod calls;
mod camera_view;
mod channel;
mod char_stats;
pub mod chat_bubble;
pub use chat_bubble::{BubbleGeometry, BubbleState};
mod chat_misc;
mod chat_send;
mod chat_types;
mod chat_window;
mod clip;
mod colorselect;
mod console_exec;
mod container;
mod craft;
mod cursor;
mod death;
pub mod diagnostics;
mod editbox;
pub mod instance;
pub(crate) use editbox::{
    adopt_text_region, apply_xml_bytes, editbox_text_region_wrapper, set_selection_color,
};
pub(crate) use messageframe::set_xml_insets as set_message_xml_insets;
pub(crate) use tooltip::post_load as tooltip_post_load;
pub(crate) mod addon;
mod addon_enable;
pub mod addon_gate;
mod char_name;
mod client;
pub use client::FrameTimeRing;
mod clock;
mod cvar_checks;
mod cvars;
mod dressup;
mod duel;
pub(crate) mod event;
mod extract;
mod follow;
pub(crate) mod font;
mod font_block;
mod frame_enum;
pub use char_name::{check_char_name, NameCharsets, NameLists, NoNameLists, CHAR_NAME_SUCCESS};
mod glue;
mod glue_addons;
mod glue_core;
mod glue_create;
mod glue_realm;
mod glue_select;
pub use glue::{glue_events, VmKind};
pub use glue_core::scanf_d;
pub use glue_core::{
    glue_status_key, launch_addon_url, launch_addon_url_allowed, launch_url_allowed, GlueCall,
    GlueCredentials, GlueHost, GlueNotice, GlueRequest, GlueSignal, GlueText, NoticeState,
    GLUE_STATUS_KEYS,
};
pub use glue_create::{
    GlueCreateCatalog, GlueCreateClass, GlueCreateCounts, GlueCreateLook, GlueCreateRace,
    GlueCreateRequest, GlueNameRow,
};
pub use glue_realm::{
    realm_load_classify, realm_load_stats, GlueRealm, GlueRealmCategory, GlueRealmCells,
    GlueRealmType, RealmSortKey,
};
pub use glue_select::{GlueCharacter, GlueScenePane, GlueSelectCells};
mod denied;
mod gm_survey;
mod gm_ticket;
mod gossip;
mod guild;
mod handler_prof;
mod hyperlink;
mod lfg;
mod lottery;
mod screenshot;
mod tabard;
pub use handler_prof::HandlerRow;

mod surface;
pub use surface::widget_method_census;
mod input_verbs;
mod inspect;
mod inventory_verbs;
mod item_stats;
mod item_text;
pub mod keybind;
mod keyboard;
mod layer_batch;
mod lock_check;
mod protected;
pub(crate) mod taint;
pub(crate) mod texel;
pub(crate) mod use_gates;
mod wheel;
pub(crate) use keyboard::enable_for_script;
pub use keyboard::{KeyUp, KeyWalk};
pub use wheel::WheelWalk;
mod layout;
mod layout_cache;
mod loot;
mod loot_roll;
mod lua50;
mod macros;
mod mail;
mod measure;
mod merchant;
mod messageframe;
mod minimap;
mod movieframe;
pub mod nameplate;
pub use nameplate::{PlateBitWrites, PlateGeometry, PlateState, PLATE_BIT_ENEMY, PLATE_BIT_FRIEND};
mod model;
mod modelframe;
mod mouseover;
mod net_stats;
pub(crate) mod object;
pub use object::frame_kind_from_tag;
mod party;
mod pet;
mod petition;
mod pointer;
mod pvp;
mod quest;
mod quest_log;
pub(crate) mod region;
mod region_map;
mod reputation;
mod saved;
mod scrollframe;
mod session;
mod shapeshift;
mod simplehtml;
mod skills;
mod slash;
mod slider;
mod social;
mod sound;
mod spellbook;
mod stable;
mod statusbar;
mod stdlib;
/// An mlua error's bare Lua message, the one value the error handler is called with.
pub use stdlib::lua_message;
mod summon;
mod talent;
mod taxi;
mod tick;
pub use tick::system_chat_args;
mod tooltip;
mod tooltip_item;
mod tooltip_spell;
mod tooltip_unit;
mod ui_errors;
mod ui_scale;
mod update_walk;
pub use tooltip_unit::TooltipTint;
mod trade;
mod tradeskill;
mod trainer;
mod types;
mod unit;
mod video_pairs;
pub(crate) mod visibility;
mod weapon_enchant;
mod who_sort;
mod worldmap;
mod worldstate;
mod worn_display;
mod zone_text;

pub use action::{ActionSlot, ActionState, ActionUse};
pub use addon::{addon_load_context, AddOnInfo, AddonLoad, AddonReply, AddonSecurity};
pub use addon_enable::{AddonStateFiles, EnableHash, EnableStore};
pub use addon_message::{AddonDistribution, AddonSend};
pub use auction::{
    AuctionBid, AuctionCategory, AuctionHighBidder, AuctionItemRow, AuctionListState, AuctionQuery,
    AuctionStartRequest, AuctionState, AuctionSubCategory, BIDDER, LIST, OWNER, SORT_KEYS,
};
pub use aura::{AuraState, TrackingState};
pub use backdrop::{Backdrop, Insets};
pub use bank::BankState;
pub use battlefield_positions::{BattlefieldFlagView, BattlefieldPositionView};
pub use battlefield_queue::{BattlefieldListView, BattlefieldMapInfo, BattlefieldQueueSlot};
pub use battlefield_score::{BattlefieldScoreRow, BattlefieldScores, BattlefieldStatColumn};
pub use bind_confirm::PendingEquipAnswer;
pub use binding_abi::{s_str_to_float, s_str_to_int};
pub use calls::{NearestMode, QueuedCall, ScriptCall};
pub use camera_view::{CameraViewRequest, CAMERA_VIEW_COUNT};
pub use channel::{ChannelCommand, RecruitmentPlan, RecruitmentRun, ZoneChannelRow};
pub use char_stats::{
    weapon_subclass_skill, BankBagSlots, InvSlotView, InventorySlots, UnitCombatStats,
    BANK_BAG_SLOT_COUNT, INVENTORY_SLOT_COUNT, SKILL_DEFENSE, SKILL_UNARMED,
};
pub use chat_misc::EmoteRequest;
pub use chat_send::{ChatGroup, ChatSend, SEND_CHAT_TOKENS};
pub use chat_types::ChatTypeColor;
pub use chat_window::{message_group_index, ChatWindowLook, MESSAGE_GROUPS};
pub use console_exec::{console_word, cvar_name_arg, no_such_cvar, set_args};
pub use container::{
    BagAutoStore, ContainerMove, ContainerSlot, ContainerState, ContainerUse, EnchantView,
    PendingWrap, PetitionSlotView, RandomPropertyView, UiCursorMode, UseArm, UseFork,
};
pub use craft::{CraftReagent, CraftRecipe, CraftState, CraftTooltip};
pub use cursor::equip::{AutoEquip, BIND_WHEN_EQUIPPED};
pub use cursor::money::coin_icon;
pub use cursor::{
    CursorAction, CursorItem, CursorMacro, CursorMerchantItem, CursorMoney, CursorPayload,
    CursorPetAction, CursorSpell, CursorStablePet, EnchantAnswer, EnchantConfirm, HeldCursor,
    ItemPick, WorldPick, EQUIPMENT_BAG,
};
pub use cvar_checks::{
    check_base_mip, check_camera_decay, check_camera_distance_max, check_camera_pitch,
    check_camera_rate, check_camera_smooth_style, check_camera_time, check_camera_view,
    check_camera_yaw, check_farclip, check_frill_density, check_refresh, check_shadow_level,
    check_small_cull, check_tex_lod_bias, check_triple_buffer, CvarCheck, CvarRows, CvarVerdict,
};
pub use cvar_checks::{
    check_dist_cull, check_error_filter, check_footstep_bias, check_horizon_farclip,
    check_lod_dist, check_map_obj_light_lod, check_max_lights, check_particle_density,
    check_shadow_bias, check_target_nearest_distance, check_texture_lod_dist, check_unit_draw_dist,
    check_water_lod,
};
pub use cvars::small_cull_text;
pub use cvars::{
    CvarWrite, MultisampleFormat, ScreenResolution, SeededCvar, VideoCaps, CVAR_FRILL_DENSITY,
    CVAR_GAMMA, CVAR_SMALL_CULL, CVAR_WORLD_DETAIL, IN_WORLD_READ_ONLY_CVARS, LUA_CVAR_LIMIT,
    VIDEO_DEFAULT_CVARS, WORLD_DETAIL_STOPS,
};
pub use death::{DeathAction, DeathUiState};
pub use dressup::DressUpIntent;
pub use duel::DuelRequest;
pub use follow::FollowRequest;
pub use gm_survey::{GmSurveyTables, SurveyAnswerSend};
pub use gm_ticket::{GmTicketIntent, GmTicketWrite};
pub use gossip::{GossipMenu, GossipOptionView, GossipQuestRow};
pub use guild::{
    GuildMemberInfo, GuildRankEdit, GuildRankInfo, GuildRequest, GuildState, LastOnline, UnitGuild,
    MAX_RANKS, MIN_RANKS, RANK_RIGHT_BITS,
};
pub use input_verbs::{BindingInput, FiredInput, HeldInput};
pub use lfg::LfgDungeon;
pub use lottery::UnservedSend;
pub use modelframe::{CharacterPaneFrame, ModelPaneFrame};
pub use petition::{
    validate_guild_name, PetitionRecordView, PetitionRequest, PetitionState, PETITION_TYPE_CHARTER,
    PETITION_TYPE_PETITION,
};
pub use protected::InputEvents;
pub use tabard::{
    emblem_mask_path, TabardHost, TabardIntent, EMBLEM_MASK_TOKEN, TABARD_COUNTS,
    TABARD_CREATION_COST,
};
pub use video_pairs::VIDEO_PAIR_CVARS;
pub use worldmap_arrow::ARROW_MODEL;

pub(crate) use button::{refresh_label_lua, state_font_lua, LabelFont};
pub(crate) use font_block::{face_loads, own_font_lua, SAME_FONT_HEIGHT};
pub use inspect::{InspectView, UnitReach};
pub use item_stats::{item_usable, ItemSetView, ItemTemplateView, PlayerReqState};
pub use item_text::ItemTextState;
pub use layout_cache::{AnchorLayout, FrameLayout, LayoutPoint};
pub use lock_check::{check_lock, LockCheck, LockOpener, LockOpeners, LockSlotView};
pub use loot::{LootRow, LootState, LOOT_PORTRAIT_UNIT};
pub use loot_roll::{LootRollEntry, LootRollsState};
pub use macros::{MacroBinding, MacroState, MacroView, MAX_MACROS, MAX_MACRO_BODY, MAX_MACRO_NAME};
pub use mail::{
    MailInboxRow, MailInvoice, MailSendRequest, MailState, PackageView, StationeryView,
};
pub use measure::TextMeasure;
pub use merchant::{ItemStatsHead, MerchantItem, MerchantState, RepairCosts};
pub(crate) use minimap::apply_model_attrs as apply_minimap_model_attrs;
pub use minimap::{
    MinimapRimArrow, CORPSE_ARROW as MINIMAP_CORPSE_ARROW, GROUP_ARROW as MINIMAP_GROUP_ARROW,
    GUIDE_ARROW as MINIMAP_GUIDE_ARROW, LANDMARK_ARROW as MINIMAP_LANDMARK_ARROW,
};
pub(crate) use model::Model;
pub use model::{
    FontProbe, MacroIconSource, TextureProbe, TextureSizeFn, TextureSizeProbe, WorldLocFn,
    WorldLocProjector,
};
pub use party::{PartyMemberInfo, PartyRequest, PartyState, RaidMemberInfo, SavedInstanceInfo};
pub use pet::{PetActionView, PetStats};
pub use pvp::{decorated_name, pvp_name_global_keys, HonorState, InspectHonorData, RankTitle};
pub use quest::{
    QuestAction, QuestItemView, QuestPanel, QuestRewardSpell, QuestSelect, QuestState,
};
pub use quest_log::{QuestObjectiveView, QuestRecordView, QuestSlotView, QUEST_LOG_SLOTS};
pub(crate) use region::{
    apply_texture_name, apply_wrap_attrs, implicit_creation_anchor_lua, install_loaded_texture,
    load_blend_mode, load_texture_file, load_texture_name, load_xml_font_file,
    texture_name_verdict, FileLoad,
};
pub use reputation::{FactionEntry, ReputationSend, ReputationState};
pub use session::SessionRequest;
pub use shapeshift::ShapeshiftFormView;
pub(crate) use simplehtml::{
    element_fonts_lua as simplehtml_element_fonts_lua,
    element_of_xml_tag as simplehtml_element_of_xml_tag,
};
pub use skills::{SkillEntry, SkillsState};
pub use social::{FriendInfo, SocialRequest, SocialState, WhoInfo};
pub use sound::{MusicRequest, SoundRequest};
pub use spellbook::{
    resolve_spell_by_name, PetBookState, SpellBookState, SpellSlotView, SpellTabView,
};
pub use stable::{StableIntent, StablePetSlot, StableState, NUM_STABLE_SLOTS};
pub use summon::SummonConfirmUiState;
pub use talent::{TalentPrereqView, TalentTabView, TalentUiState, TalentView};
pub use taxi::{TaxiNodeType, TaxiUiNode, TaxiUiState};
pub use tooltip_spell::SpellTooltipView;
pub use trade::{TradeSideState, TradeSlotItem, TradeState, TRADE_SLOTS};
pub use tradeskill::{TradeSkillDifficulty, TradeSkillReagent, TradeSkillRecipe, TradeSkillState};
pub use trainer::{
    TrainerAbilityReq, TrainerGroup, TrainerService, TrainerServiceCategory, TrainerSkillReq,
    TrainerState, TrainerTooltip, TRAINER_GROUP_KNOWN,
};
pub(crate) use types::{monochrome_flag, FontMask, MeasuredText, RegionData};
pub use types::{
    BlendMode, EditAction, EditBoxAdvanceRequest, EditBoxTextUi, EditOutcome, EditUnit,
    ExtractedQuad, FontObject, FontShadow, Gradient, JustifyH, JustifyV, LineMeasureRequest,
    MeasureRequest, Outline, QuadContent, ScriptValue, TexCoords,
};
pub use unit::by_name::{self, NameCandidate};
pub use unit::{
    grey_band, level_reads_unknown, parse_unit_token, power_token, unit_is_grey, PlayerRecord,
    SelectionRequest, UnitBase, UnitGuids, UnitState, UnitTokenParse,
};
pub use weapon_enchant::WeaponEnchant;
pub use who_sort::{WhoSortChain, WhoSortKey};
pub use worldmap::{
    WorldMapContinentView, WorldMapLandmarkSource, WorldMapLandmarkView, WorldMapLevel,
    WorldMapOverlayView, WorldMapSheetRow, WorldMapState, WorldMapZoneView,
};
pub use worldstate::WorldStateUiView;
pub use worn_display::WornDisplay;
pub use zone_text::ZoneTexts;

use mlua::Lua;

use crate::layout::Rect;
use crate::order::ZTarget;
use crate::widget::{FrameHandle, KindState};

// Registry keys, the only roots of Lua-side state.
const REG_FRAME_META: &str = "__benilla_frame_meta";
const REG_REGION_META: &str = "__benilla_region_meta";
const REG_FRAME_METHODS: &str = "__benilla_frame_methods";
const REG_REGION_METHODS: &str = "__benilla_region_methods";
#[cfg(test)]
pub(crate) const REG_REGION_METHODS_FOR_TEST: &str = REG_REGION_METHODS;
/// The title region's method table and metatable: the 19 Region methods and nothing else.
const REG_TITLE_METHODS: &str = "__benilla_title_methods";
const REG_TITLE_META: &str = "__benilla_title_meta";
/// The region leaf tables and metatables: Texture and FontString each answer their own map.
const REG_TEXTURE_METHODS: &str = "__benilla_texture_methods";
const REG_TEXTURE_META: &str = "__benilla_texture_meta";
const REG_FONTSTRING_METHODS: &str = "__benilla_fontstring_methods";
const REG_FONTSTRING_META: &str = "__benilla_fontstring_meta";
/// The error handler `seterrorhandler` holds, the reference's registry reference `[0x8722cc]`;
/// unset until one is given, as the reference's starts at -1.
const REG_ERRORHANDLER: &str = "__benilla_errorhandler";

/// Names on both region leaves, each leaf registering its own copy (Texture's `SetAlpha`
/// `0x79b580`, FontString's `0x79cb70`), so they are not on the Region map and must not be hoisted
/// into it. `GetDrawLayer` is on both (Texture `0x79a6c0`, FontString `0x79c660`).
pub(crate) const REGION_LEAF_SHARED: [&str; 9] = [
    "SetDrawLayer",
    "GetDrawLayer",
    "SetVertexColor",
    "SetAlpha",
    "GetAlpha",
    "Show",
    "Hide",
    "IsVisible",
    "IsShown",
];

/// Texture-only, each in the client's Texture map (`0x87c128`): `GetVertexColor` though
/// `SetVertexColor` is shared, and `SetGradientAlpha`, not FontString's `SetAlphaGradient`. 1.12
/// has no Texture `SetSize` or `SetRotation` (PlayerModel's, `0x84f1fc`/`0x505f00`).
pub(crate) const TEXTURE_ONLY_METHODS: [&str; 13] = [
    "SetGradient",
    "SetGradientAlpha",
    "GetTexture",
    "SetTexture",
    "GetTexCoord",
    "SetTexCoord",
    "SetBlendMode",
    "GetBlendMode",
    "SetTexCoordModifiesRect",
    "GetTexCoordModifiesRect",
    "SetDesaturated",
    "IsDesaturated",
    "GetVertexColor",
];

/// FontString-only: the font, text, justify, shadow and spacing block and the string metrics.
/// `GetStringHeight`, `SetFormattedText` and `SetSize` are not 1.12 FontString methods.
pub(crate) const FONTSTRING_ONLY_METHODS: [&str; 23] = [
    "SetFont",
    "GetFont",
    "SetFontObject",
    "GetFontObject",
    "SetTextColor",
    "GetTextColor",
    "SetShadowColor",
    "GetShadowColor",
    "SetShadowOffset",
    "GetShadowOffset",
    "SetSpacing",
    "GetSpacing",
    "SetJustifyH",
    "GetJustifyH",
    "SetJustifyV",
    "GetJustifyV",
    "SetText",
    "GetText",
    "SetTextHeight",
    "GetStringWidth",
    "SetNonSpaceWrap",
    "CanNonSpaceWrap",
    "SetAlphaGradient",
];

/// The Region method map (`0xcf54b4`): the 19 names every region leaf reaches on a miss.
pub(crate) const REGION_MAP_METHODS: [&str; 19] = [
    "GetObjectType",
    "IsObjectType",
    "GetName",
    "GetParent",
    "SetParent",
    "GetCenter",
    "GetLeft",
    "GetRight",
    "GetTop",
    "GetBottom",
    "GetWidth",
    "SetWidth",
    "GetHeight",
    "SetHeight",
    "GetNumPoints",
    "GetPoint",
    "SetPoint",
    "SetAllPoints",
    "ClearAllPoints",
];
const REG_WRAPPERS: &str = "__benilla_wrappers";
const REG_SCRIPTS: &str = "__benilla_scripts";

/// `MINIMAP_PING`'s arguments, `%s%f%f` (`0x4ee48d`): the unit token and the two fractions.
pub(crate) fn ping_args(token: &str, a: f64, b: f64) -> Vec<ScriptValue> {
    vec![
        ScriptValue::Str(token.to_string()),
        ScriptValue::Number(a),
        ScriptValue::Number(b),
    ]
}

/// The layout handle of the screen root (the client's `CSimpleTop`, not the `UIParent` frame),
/// which a top-level `SetPoint` without `relativeTo` anchors to; frame ids, from 1, are the rest.
pub const SCREEN: crate::layout::Handle = 0;

/// The FrameScript handler kinds `SetScript` accepts, each only with the code that fires it
/// ([`crate::script::object::events_regions::set_script`]). The list is flat, where the reference
/// resolves names per widget type (base map `0x76a0d0` plus the type's own; a `<Frame>` has no
/// `OnClick`), so any widget accepts any kind here.
///
/// `OnValueChanged` is the StatusBar's (`+0x32c`) and the Slider's (`+0x330`) own slot; the
/// ScrollFrame's three scroll kinds are `+0x32c`/`+0x334`/`+0x33c` (script-name map `0x786c40`).
/// The EditBox's vtable (`0x81c910`) replaces the key and char slots: an EditBox never fires
/// `OnKeyDown`, and fires `OnChar` only from `Insert`, with the inserted text (`0x77c13c`).
const SCRIPT_KINDS: [&str; 46] = [
    "OnLoad",
    "OnEvent",
    "OnUpdate",
    // The model pane's two: `OnUpdateModel` from its paint in the draw pass
    // (`paint_model_panes`), `OnAnimFinished` from its clock advance in the update walk
    // (`advance_pane`) when the armed sequence completes.
    "OnUpdateModel",
    "OnAnimFinished",
    "OnShow",
    "OnHide",
    "OnClick",
    "OnEnter",
    "OnLeave",
    "OnMouseDown",
    "OnMouseUp",
    "OnMouseWheel",
    "OnValueChanged",
    "OnEnterPressed",
    "OnEscapePressed",
    "OnSpacePressed",
    "OnTabPressed",
    "OnTextChanged",
    "OnTextSet",
    // The caret leg's own (`0x77da80`), fired by the box's flush on dirty bit 2, which a caret
    // move, an edit, a focus change and a re-seat raise: the edge `ScrollingEdit_OnCursorChanged`
    // scrolls a multiline box by.
    "OnCursorChanged",
    "OnEditFocusGained",
    "OnEditFocusLost",
    // The EditBox's input-method slot (script map `0x77a310`, `0x77a40d`, `+0x430`), fired only
    // when an input-method event changes the box's language (`0x77abb0`, stored at `0x77abf2`).
    // benilla has no input method, so it is stored and never runs, as on a western layout.
    "OnInputLanguageChanged",
    "OnHorizontalScroll",
    "OnVerticalScroll",
    "OnScrollRangeChanged",
    "OnDragStart",
    "OnDragStop",
    "OnReceiveDrag",
    // A text frame's hyperlink button hands it these ([`hyperlink`]): the click as
    // `OnHyperlinkClick(link, text, button)`, which `ChatFrameTemplate` passes to `SetItemRef`
    // (`ChatFrame.xml:15`, `ChatFrame.lua:1534`), and the enter and leave with the link alone.
    "OnHyperlinkClick",
    "OnHyperlinkEnter",
    "OnHyperlinkLeave",
    // The ScrollingMessageFrame's own (script map `0x787d90`, `+0x3bc`), fired by a relayout that
    // moved its anchor (`0x788a9a`).
    "OnMessageScrollChanged",
    // The GameTooltip's engine-fired scripts: money render, money clear and world-hover default
    // placement, all three wired by the stock template (`GameTooltipTemplate.xml:617-625`).
    "OnTooltipAddMoney",
    "OnTooltipCleared",
    "OnTooltipSetDefaultAnchor",
    // The ColorSelect's own slot (script map `0x78b4f0`, `+0x338`), fired by its `SetColorRGB`.
    "OnColorSelect",
    // The Button/CheckButton double click (script map `0x778c50`, `+0x4d4`), fired by
    // [`pointer`]'s release edge in place of the second `OnClick` within 300 ms (`0x77937b`).
    "OnDoubleClick",
    // The layout event (base map `0x76a0d0`, `+0x120`), fired by the resolve pass on a size change.
    "OnSizeChanged",
    // The movie frame's three (script map `0x78c2e0`): fired by its player alone, which does not
    // exist here ([`movieframe`]), so they are stored and never run.
    "OnMovieFinished",
    "OnMovieShowSubtitle",
    "OnMovieHideSubtitle",
    // The key channels: `OnChar` and `OnKeyDown` fired by [`keyboard`]'s walks (`0x765df0`,
    // `0x765f10`), `OnKeyUp` by the key-up dispatcher (`0x765fd0`) on the frame that took the
    // key's down. A frame carrying only `OnKeyUp` still consumes every key-down and runs nothing
    // on it, the reference's own asymmetry.
    "OnChar",
    "OnKeyDown",
    "OnKeyUp",
];

// ─────────────────────────────────────────────────────────────────────────────────────────────
// UiScript: the public host
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The Lua scripting host: owns the VM, whose `Model` lives in `lua.app_data`. Its `impl` blocks
/// sit beside their concerns: [`extract`], [`tick`], [`editbox::seam`], [`layout`] and [`pointer`].
pub struct UiScript {
    lua: Lua,
    /// VM instructions counted while a budget is installed ([`UiScript::set_instruction_budget`]).
    instructions: std::sync::Arc<std::sync::atomic::AtomicU64>,
    session: u64,
}

/// Hands out [`UiScript::session`] ids: process-global and monotone, so an id is never reused.
static NEXT_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// How often the instruction hook fires: an instruction budget's resolution (low values cost).
pub const INSTRUCTION_HOOK_STEP: u32 = 1_000_000;

/// The chunk name the 1.12 client gives an addon's file, `Interface\AddOns\<Folder>\<File>`,
/// backslashed and with the file, because addons parse it: `FuBarPlugin-2.0` finds its folder with
/// the greedy `\\AddOns\\(.*)\\` over `debugstack`, which needs the file after the folder.
pub fn addon_chunk_name(folder: &str, file: &str) -> String {
    // `@` marks the chunk as a file, so a traceback prints the path plainly.
    format!("@Interface\\AddOns\\{folder}\\{}", file.replace('/', "\\"))
}

/// The state as `0x7039e0` leaves it: benilla-lua's client-shaped opening with the host's base
/// functions, the dialect and the stdlib layer set over it, which replace values and add no key.
fn open_state(vm: VmKind) -> mlua::Result<Lua> {
    // `Lua::new` sets `InitLua`'s 256 KB threshold first (`0x7039f2`), then opens the state.
    let lua = Lua::new();
    let mut model = Model::new();
    model.vm = vm;
    lua.set_app_data(model);
    // Before anything can fire a handler and while no app-data borrow is held, as the
    // profiler's slot requires.
    handler_prof::install(&lua);

    // `0x7039e0`: the base array `0x811e28`, the four libraries and the compat chunk `0x8722e8`.
    stdlib::sandbox(&lua)?;
    // Before the stdlib layer, so its aliases bind the 5.0-shaped functions.
    lua50::install(&lua)?;
    taint::install(&lua)?;
    stdlib::install(&lua)?;

    Ok(lua)
}

/// What both FrameScript states hold: FrameScript's Lua init (`0x7039e0`), the bare state the glue
/// builder starts from (left by startup `0x402aee` on its first build, re-run by its reset
/// `0x703b80` on a later one) and `UI_Init` rebuilds through its own, and the three registrars
/// the glue registrar `0x46abb0` shares with `UI_Init`: `0x51bb20` (the clock, `ConsoleExec` and
/// the file trio), `0x706350` (the frame factory globals and the 17 widget method tables) and
/// `0x4587f0` (sound).
fn install_framescript(vm: VmKind) -> mlua::Result<Lua> {
    let lua = open_state(vm)?;

    // `0x51bb20` (table `0x8503f4`): `GetTime`, `GetGameTime` and `ConsoleExec`. `ReadFile`,
    // `DeleteFile` and `AppendToFile` are not built.
    clock::install(&lua)?;
    console_exec::install(&lua)?;

    // `0x706350`: `CreateFrame`, `CreateFont`, `GetNumFrames`, `EnumerateFrames` (table
    // `0x872e74`), then the widget method tables `0x7a2e70`..`0x78dc60`.
    object::install(&lua)?;
    frame_enum::install(&lua)?;
    // After `object`, whose `publish_global` it reuses, and before any FrameXML loads:
    // `Loader::do_font` publishes into the tables this builds.
    font::install(&lua)?;
    messageframe::install(&lua)?;
    scrollframe::install(&lua)?;
    simplehtml::install(&lua)?;
    slider::install(&lua)?;
    colorselect::install(&lua)?;
    movieframe::install(&lua)?;
    modelframe::install(&lua)?;

    // `0x4587f0` (table `0x835a50`): `PlaySound`, `PlayMusic`, `PlaySoundFile`, `StopMusic`.
    sound::install(&lua)?;
    Ok(lua)
}

/// The in-game surface: everything `UI_Init` registers past the shared registrars (`0x490250` and
/// the calls around it), the in-game widget types' method tables among it.
fn install_in_game(lua: &Lua) -> mlua::Result<()> {
    addon::install(lua)?;
    addon_message::install(lua)?;
    chat_send::install(lua)?;
    chat_types::install(lua)?;
    chat_misc::install(lua)?;
    channel::install(lua)?;
    chat_window::install(lua)?;
    client::install(lua)?;
    screenshot::install(lua)?;
    zone_text::install(lua)?;
    object::install_in_game(lua)?;
    region::install_in_game(lua)?;
    modelframe::install_player_model(lua)?;
    unit::install(lua)?;
    party::install(lua)?;
    social::install(lua)?;
    guild::install(lua)?;
    petition::install(lua)?;
    binder::install(lua)?;
    summon::install(lua)?;
    bind_confirm::install(lua)?;
    instance::install(lua)?;
    gm_ticket::install(lua)?;
    gm_survey::install(lua)?;
    denied::install(lua)?;
    lfg::install(lua)?;
    lottery::install(lua)?;
    duel::install(lua)?;
    follow::install(lua)?;
    camera_view::install(lua)?;
    input_verbs::install(lua)?;
    session::install(lua)?;
    pvp::install(lua)?;
    worn_display::install(lua)?;
    death::install(lua)?;
    aura::install(lua)?;
    cvars::install(lua)?;
    nameplate::install_verbs(lua)?;
    saved::install(lua)?;
    keybind::install(lua)?;
    ui_errors::install(lua)?;
    pointer::install(lua)?;
    action::install(lua)?;
    action_bar_toggles::install(lua)?;
    container::install(lua)?;
    cursor::install(lua)?;
    spellbook::install(lua)?;
    macros::install(lua)?;
    talent::install(lua)?;
    dialog_verbs::install(lua)?;
    battlefield_score::install(lua)?;
    battlefield_queue::install(lua)?;
    battlefield_positions::install(lua)?;
    tutorial::install(lua)?;
    worldmap_arrow::install(lua)?;
    shapeshift::install(lua)?;
    pet::install(lua)?;
    gossip::install(lua)?;
    merchant::install(lua)?;
    bank::install(lua)?;
    stable::install(lua)?;
    item_text::install(lua)?;
    mail::install(lua)?;
    auction::install(lua)?;
    trainer::install(lua)?;
    taxi::install(lua)?;
    trade::install(lua)?;
    inspect::install(lua)?;
    dressup::install(lua)?;
    tabard::install(lua)?;
    tradeskill::install(lua)?;
    craft::install(lua)?;
    reputation::install(lua)?;
    skills::install(lua)?;
    item_stats::install(lua)?;
    char_stats::install(lua)?;
    inventory_verbs::install(lua)?;
    weapon_enchant::install(lua)?;
    loot::install(lua)?;
    loot_roll::install(lua)?;
    quest::install(lua)?;
    quest_log::install(lua)?;
    minimap::install(lua)?;
    tooltip::install(lua)?;
    worldmap::install(lua)?;
    worldstate::install(lua)?;
    net_stats::install(lua)?;
    Ok(())
}

impl UiScript {
    /// The in-game VM: FrameScript's base set and the registrars both states share
    /// ([`install_framescript`]), then the in-game surface `UI_Init` (`0x48fbf0`) registers.
    pub fn new() -> mlua::Result<UiScript> {
        let lua = install_framescript(VmKind::InGame)?;
        install_in_game(&lua)?;
        Ok(Self::host(lua))
    }

    /// The glue screens' VM, as the glue builder (`0x46a7b0`) leaves its state before GlueXML
    /// loads: the base set and the shared registrars, with the glue event table and the glue frame
    /// types ([`glue`]), and none of the in-game surface. Of the glue registrar's own four tables
    /// (`0x46ddf0`, `0x4736c0`, `0x471a20`, `0x46f480`) the screen verbs and the realm list are
    /// built ([`glue::install`]).
    pub fn new_glue() -> mlua::Result<UiScript> {
        let lua = install_framescript(VmKind::Glue)?;
        glue::install(&lua)?;
        Ok(Self::host(lua))
    }

    fn host(lua: Lua) -> UiScript {
        UiScript {
            lua,
            instructions: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            session: NEXT_SESSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Which FrameScript state this VM is.
    pub fn vm_kind(&self) -> VmKind {
        self.model_ref().vm
    }

    /// The embedded VM, for the app and the loader to add their bindings over this object model.
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Which VM this is: a fresh number per [`UiScript::new`], never reused, so anything the host
    /// seeded into a VM is valid only while this still matches.
    ///
    /// The reference keeps one Lua state (`0xceef74`), replaced only by the reset `0x703b80`: from
    /// `UI_Init` (`0x48fbf0`, at `0x48fe97`) as it loads FrameXML, `ShutdownGame` (`0x491231`) and
    /// the glue builder (`0x46a87b`), never the UI teardown `0x490bd0`. GlueXML and FrameXML never
    /// share a state, so world entry mints its VM at the top of its load.
    pub fn session(&self) -> u64 {
        self.session
    }

    /// An independent copy of this UI, Lua and Rust sides both, as a fresh session: a built state
    /// to start many runs from without building it again. The host's services (the text measurer,
    /// the probes) are shared with the original; an instruction budget is not carried over.
    pub fn fork(&self) -> UiScript {
        let lua = self.lua.fork();
        lua.remove_hook();
        lua.set_app_data(self.model_ref().clone());
        handler_prof::install(&lua);
        UiScript {
            lua,
            instructions: Default::default(),
            session: NEXT_SESSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Bound how long a chunk may run: past `budget` VM instructions the hook aborts the chunk to
    /// the host, past every `pcall` and `xpcall` and running no error handler
    /// (`benilla_lua::Vm::abort`), so a loop that never ends fails its addon and the rest still
    /// runs. The app arms it only for the world-entry load, so a hung load reports instead of
    /// freezing the loading screen, and disarms it ([`Self::clear_instruction_budget`]) before the
    /// session's first frame. Deviation: the client's Lua has no such bound, as nothing sets a
    /// hook (`lua_sethook`, `0x6fba40`, has no caller and no pointer to it), so a load that
    /// never ends hangs it.
    pub fn set_instruction_budget(&self, budget: u64) {
        let used = self.instructions.clone();
        used.store(0, std::sync::atomic::Ordering::Relaxed);
        let _ = self.lua.set_hook(
            mlua::HookTriggers {
                every_nth_instruction: Some(INSTRUCTION_HOOK_STEP),
                ..Default::default()
            },
            move |_, _| {
                let n = used.fetch_add(
                    u64::from(INSTRUCTION_HOOK_STEP),
                    std::sync::atomic::Ordering::Relaxed,
                ) + u64::from(INSTRUCTION_HOOK_STEP);
                if n > budget {
                    return Err(mlua::Error::runtime(format!(
                        "benilla: instruction budget exhausted after {n} VM instructions — \
                         treating this as a non-terminating loop"
                    )));
                }
                Ok(mlua::VmState::Continue)
            },
        );
    }

    /// Remove the instruction budget; the counter keeps its value for [`Self::instructions_used`].
    pub fn clear_instruction_budget(&self) {
        self.lua.remove_hook();
    }

    /// VM instructions since the last [`Self::set_instruction_budget`], to the resolution of
    /// [`INSTRUCTION_HOOK_STEP`]; zero when no budget was ever set.
    pub fn instructions_used(&self) -> u64 {
        self.instructions.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set the screen-root rect from a pixel size (origin `[0,0]`, y-up), compared first so the
    /// app's every-frame call leaves an unchanged layout clean. On a change (`true`) the caller
    /// re-runs `UIParent_ManageFramePositions()`: the window resizes freely, and a seat computed
    /// from `GetScreenHeight()`, like the open-bag stack's column wrap, does not follow anchors.
    pub fn set_screen_size(&mut self, width: f32, height: f32) -> bool {
        self.set_root(Rect::new(0.0, 0.0, height, width), None)
    }

    /// Set the screen root for a window of `width` by `height` pixels: the screen-scale cells from
    /// its aspect `a = RN24(width / height)`, as `0x41ad10` takes it, and the root `768·a` by 768
    /// script units, whose layout extent is exactly the cells' `K1` by `K2` (`0x764284`). A window
    /// with no area reads as 4/3. Returns whether anything moved, as [`Self::set_screen_size`].
    ///
    /// Deviation: the reference sets the cells only when glue or the interface is built, `0x41ad10`
    /// from `0x46a828` (`0x46a7b0`) and `0x48fe2b` (`0x48fbf0`), and a resize reaches them only
    /// through the rebuild it asks for. In glue `0x46abe0` sets the rebuild flag `0xb41d8c` when
    /// the aspect moves by 0.01 or more (`fcomp` at `0x46ac66`); in game `0x492e90` reloads the
    /// interface (`0x491380`) past 0.001 (`0x492f26`, `0x492f31`). The gate and the rebuild both
    /// take the aspect from `gxResolution` (`0x46ab40`, `0x4901e0`; 4/3 while `widescreen` is
    /// off), never the live window. Here every resize sets the cells from the window, because the
    /// UI is kept live across a resize, and `widescreen` off keeps the window's aspect, as a 4:3
    /// root over a wider window would take a seam stretched on one axis.
    pub fn set_window_size(&mut self, width: f32, height: f32) -> bool {
        let a = if width > 0.0 && height > 0.0 {
            width / height
        } else {
            4.0 / 3.0
        };
        self.set_root(Rect::new(0.0, 0.0, 768.0, 768.0 * a), Some(a))
    }

    fn set_root(&mut self, new: Rect, aspect: Option<f32>) -> bool {
        let mut model = self.model_mut();
        if model.screen == new && model.aspect.map(f32::to_bits) == aspect.map(f32::to_bits) {
            return false;
        }
        let before = model.layout_units();
        model.screen = new;
        model.aspect = aspect;
        let units = crate::layout::LayoutUnits::from_aspect(model.screen_aspect());
        model.space = crate::layout::LayoutSpace::new(units, new.width(), new.height());
        if units != before {
            // New cells are a new interface build (`0x41ad10`): every node resolves afresh, and
            // what the build's constructors took from the cells is taken again.
            model.force_every_layout_node();
            let minimaps = model.arena.minimap_kinds().to_vec();
            for h in minimaps {
                minimap::seat_engine_children(&mut model, h);
            }
            worldmap_arrow::rescale(&mut model);
        }
        model.touch_layout();
        drop(model);
        // The implicit rects are in layout units, which follow the aspect.
        self.reapply_implicit_rects();
        true
    }

    /// Push the modifier state behind `IsShiftKeyDown`/`IsControlKeyDown`/`IsAltKeyDown`; the app
    /// calls it before the frame's mouse events, so a click handler reads the state of its click.
    pub fn set_modifiers(&mut self, shift: bool, ctrl: bool, alt: bool) {
        let mut model = self.model_mut();
        model.modifiers = (shift, ctrl, alt);
    }

    /// Push the player's WMO-containment state, the indoor byte `0xceaa60` that picks which zoom
    /// index `GetZoom`/`SetZoom` act on. Call it before the script tick, on each inside/outside
    /// edge and for each new VM.
    pub fn set_minimap_inside(&mut self, inside: bool) {
        self.model_mut().minimap.inside = inside;
    }

    /// The Minimap's per-frame update (`0x4eb040`) on every Minimap's player arrow: once its model
    /// is loaded (`0x7103d0`), `0x4ee100` centres it on its own size, half its width and height
    /// (slots 0x1c and 0x20) in layout units; then `CMinimap::SetPlayerFacing` (`0x4eb8e0`, fed at
    /// `0x4eb0a4`–`0x4eb0b1`) copies the player's facing (radians) raw into the field
    /// `Model:SetFacing` (`0x76dce0`) writes, so the arrow's `GetFacing()` reads it.
    pub fn set_minimap_player_facing(&mut self, facing: f32) {
        let mut model = self.model_mut();
        let unit = model.layout_unit();
        let arrows: Vec<crate::widget::FrameHandle> = model
            .arena
            .minimap_kinds()
            .iter()
            .filter_map(|&h| match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Minimap(m)) => m.player_arrow,
                _ => None,
            })
            .collect();
        for h in arrows {
            let size = model
                .layout_inputs
                .get(&h)
                .map_or((0.0, 0.0), |i| (i.width, i.height));
            let loaded = match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Model(m)) => m
                    .path
                    .as_deref()
                    .is_some_and(|p| model.model_facts.contains_key(&crate::widget::model_key(p))),
                _ => false,
            };
            if let Some(KindState::Model(state)) =
                model.arena.frame_mut(h).map(|f| &mut f.kind_state)
            {
                if loaded {
                    state.position = (size.0 / unit * 0.5, size.1 / unit * 0.5, 0.0);
                }
                state.facing = facing;
            }
        }
    }

    /// The minimap's view radius in yards at the live zoom (`0x6da9b0`).
    pub fn minimap_radius_yd(&self) -> f32 {
        self.model_ref().minimap.radius_yd()
    }

    /// Push the active player's WoW world `(x, y)`, `None` with no player object, before the
    /// script tick: the ping methods read it at the call (`0x4eecdf`, `0x4ef021`).
    pub fn set_minimap_player(&mut self, pos: Option<(f32, f32)>) {
        self.model_mut().minimap.player = pos;
    }

    /// Seat the stored ping a new VM inherits, the reference's statics living across the reload
    /// (`0xbc787c`, `0xbc7880`); no event.
    pub fn set_minimap_ping_point(&mut self, world: (f32, f32)) {
        self.model_mut().minimap.ping = world;
    }

    /// A group member's ping (`0x5e41bd` → `0x4ee330`): stored at once and `MINIMAP_PING` signalled
    /// with the pinger's unit token, all before this returns. Nothing without a player object.
    pub fn apply_minimap_ping(&mut self, world: (f32, f32), token: &str) {
        let fractions = self.model_mut().minimap.apply_ping(world);
        if let Some((a, b)) = fractions {
            self.fire_event("MINIMAP_PING", ping_args(token, a, b));
        }
    }

    /// Drain the world points our own `Minimap:PingLocation` pinged since the last call.
    pub fn take_minimap_pings_sent(&mut self) -> Vec<(f32, f32)> {
        std::mem::take(&mut self.model_mut().minimap_pings_sent)
    }

    /// What this VM's texture setter for `slot` last stored, with the call's stamp; `None` while
    /// this VM has not set it, when the process's texture stands.
    pub fn minimap_art(
        &self,
        slot: crate::widget::MinimapArtSlot,
    ) -> Option<(u64, crate::widget::MinimapArt)> {
        self.model_ref().minimap.art[slot as usize].clone()
    }

    /// Seed the two zoom cells from the persisted CVars before the world entry reseeds them, so a
    /// new VM's file scope reads the live level.
    pub fn set_minimap_zoom(&mut self, zoom: u32, inside_zoom: u32) {
        let mut model = self.model_mut();
        model.minimap.zoom = zoom;
        model.minimap.inside_zoom = inside_zoom;
    }

    /// The minimap initializer every world entry runs (`0x4ed9d0`, from `0x49096f`): its blip
    /// set-up (`0x6d8e10`) copies each zoom CVar's integer into its cell, `minimapZoom` into
    /// `0x86f698` and `minimapInsideZoom` into `0x86f69c` (`0x6d9008`-`0x6d901f`), and signals
    /// `MINIMAP_UPDATE_ZOOM` (`0x6d902a`), on which the stock buttons re-sync. The copy is raw, so a
    /// negative or out-of-range level stands in the cell for `GetZoom` to answer.
    pub(crate) fn minimap_enter_world(&mut self) {
        let outdoor = self.cvar_int("minimapZoom");
        let indoor = self.cvar_int("minimapInsideZoom");
        {
            let mut model = self.model_mut();
            if let Some(v) = outdoor {
                model.minimap.zoom = v as u32;
            }
            if let Some(v) = indoor {
                model.minimap.inside_zoom = v as u32;
            }
        }
        self.fire_event("MINIMAP_UPDATE_ZOOM", Vec::new());
    }

    /// Load and run a text chunk; handler errors during events go to [`UiScript::errors`] instead.
    pub fn run(&self, chunk: &str) -> mlua::Result<()> {
        self.run_chunk(chunk.as_bytes())
    }

    /// [`UiScript::run`] over a file's bytes: the reference hands the file (`0x704bc0`) to
    /// `luaL_loadbuffer` (`0x6f5690`) unconverted, so a cp1252 file's literals keep their bytes.
    /// Its compiler's UTF-8 BOM strip and `#`-line skip apply here ([`crate::source::chunk`]).
    pub fn run_chunk(&self, chunk: &[u8]) -> mlua::Result<()> {
        self.run_chunk_named(chunk, "(chunk)")
    }

    /// Run a chunk under the name the 1.12 client gives it ([`addon_chunk_name`] for addon files).
    pub fn run_chunk_named(&self, chunk: &[u8], name: &str) -> mlua::Result<()> {
        self.lua
            .load(crate::source::chunk(chunk))
            .set_name(name)
            .set_mode(mlua::ChunkMode::Text)
            .exec()
    }

    /// Load and evaluate a Lua chunk, returning its result: for tests and one-shot queries.
    pub fn eval<T: mlua::FromLuaMulti>(&self, chunk: &str) -> mlua::Result<T> {
        self.lua.load(chunk).set_mode(mlua::ChunkMode::Text).eval()
    }

    /// How many values the Lua expression `expr` returns (`"GetItemInfo(1)"`, not a chunk), zero
    /// values and one `nil` told apart; a raise propagates. `select` is not a 1.12 global, so
    /// inside Lua the count is `(function(...) return arg.n end)(expr)`.
    pub fn arity(&self, expr: &str) -> mlua::Result<usize> {
        let values: mlua::Variadic<mlua::Value> = self.eval(&format!("return {expr}"))?;
        Ok(values.len())
    }

    /// The owning frame's name for an [`ExtractedQuad`] target, for tooling that sees only quads.
    pub fn quad_owner_name(&self, target: ZTarget) -> Option<String> {
        let model = self.model_ref();
        let fh = match target {
            ZTarget::Frame(fh) => fh,
            ZTarget::Region(rh) => model.arena.region(rh)?.owner,
        };
        model.arena.frame(fh)?.name.clone()
    }

    /// Append a line from the app's chat feed to the named ScrollingMessageFrame: `r`/`g`/`b` in
    /// `0..1` are byte-quantized round-half-up, and the fade drives the line's alpha.
    pub fn add_chat_message(&mut self, frame: &str, text: &str, r: f32, g: f32, b: f32) -> bool {
        let mut model = self.model_mut();
        let Some(h) = model.arena.lookup(frame) else {
            return false;
        };
        let rows = Self::message_viewport(&model, h);
        match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            Some(crate::widget::KindState::ScrollingMessage(smf)) => {
                smf.add_with_id(text.to_string(), r, g, b, 0, rows);
                drop(model);
                messageframe::fire_scroll_changes(&self.lua, h);
                true
            }
            _ => false,
        }
    }

    /// Show the named EditBox and give it keyboard focus, for the app's chat-open key (ENTER).
    pub fn focus_editbox(&mut self, name: &str) -> bool {
        let mut model = self.model_mut();
        let Some(h) = model.arena.lookup(name) else {
            return false;
        };
        if !matches!(
            model.arena.frame(h).map(|f| &f.kind_state),
            Some(crate::widget::KindState::EditBox(_))
        ) {
            return false;
        }
        model.set_shown_silent(h, true);
        model.focused_editbox = Some(h);
        true
    }

    /// Drain the chat lines queued since the last call (a probe's, a host `SlashCmdList` row's), for
    /// the app to parse.
    pub fn take_chat_input(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().chat_input)
    }

    /// Queue a line as if typed into the chat EditBox and submitted, for probes (`WOW_PROBE_CHAT`).
    pub fn push_chat_input(&mut self, line: String) {
        self.model_mut().chat_input.push(line);
    }

    /// Replace the hyperlink spans `(frame, y-up rect, link, markup)` the app rasterized this
    /// frame: each is a hyperlink button over its span, a child of its frame ([`hyperlink`]).
    pub fn set_link_spans(&mut self, spans: Vec<(FrameHandle, Rect, String, String)>) {
        hyperlink::set_spans(&self.lua, spans);
    }

    /// Whether the frame with this global name is shown with every ancestor (`IsVisible()`).
    pub fn frame_visible(&self, name: &str) -> bool {
        self.model_ref()
            .arena
            .iter_frames()
            .any(|(_, f)| f.name.as_deref() == Some(name) && f.effective_visible)
    }

    /// The named frame's alpha byte as a fraction (`+0xc8`, which the walk `0x76a690` hands down
    /// as the effective alpha) while it is effectively visible: the app's minimap ping sprite
    /// follows the stock `MiniMapPing` `<Model>` this way.
    pub fn frame_alpha(&self, name: &str) -> Option<f32> {
        self.model_ref()
            .arena
            .iter_frames()
            .find(|(_, f)| f.name.as_deref() == Some(name) && f.effective_visible)
            .map(|(_, f)| crate::color::alpha_f32(f.alpha))
    }

    /// The layout drain (`0x768ed0`), which [`Self::tick`] runs before and after its walk: resolve
    /// and cache every frame's rect, which `GetWidth`/`GetHeight`/`extract` read, then fire
    /// `OnSizeChanged` for frames whose size moved ([`event::fire_size_changes`]) and re-seat the
    /// text of every resized EditBox.
    pub fn resolve(&mut self) {
        self.resolve_and_measure();
        event::fire_size_changes(&self.lua);
        // The EditBox's own `OnSizeChanged` (`0x77a8d0`) runs after the script fire and re-seats a
        // resized box's text, which the reference's drain resolves in the same pass.
        let reseated = editbox::reseat_resized(&mut self.model_mut());
        if reseated {
            self.resolve_and_measure();
        }
    }

    /// Resolve, and with a font engine installed ([`Self::set_text_measurer`]) measure what the
    /// solve revealed and solve again, so a FontString's box is right in the frame its text was set.
    fn resolve_and_measure(&mut self) {
        {
            let mut model = self.model_mut();
            Self::resolve_layout(&mut model);
        }
        if self.fill_measures() {
            let mut model = self.model_mut();
            Self::resolve_layout(&mut model);
        }
    }

    /// Store host measurements `(id, w, h, natural_w, key)` for [`MeasureRequest`]s; the next
    /// [`UiScript::resolve`] sizes the FontStrings by `w`/`h`, the text as laid out (wrapped in a
    /// declared width), while `natural_w` is its unwrapped width, which `GetStringWidth` reports.
    pub fn set_measured_text(&mut self, measures: &[(u32, f32, f32, f32, u64)]) {
        let mut model = self.model_mut();
        for &(id, w, h, natural_w, key) in measures {
            let Some(&rh) = model.id_to_region.get(&id) else {
                continue;
            };
            let mut moved = false;
            if let Some(d) = model.region_data.get_mut(&rh) {
                let new = MeasuredText {
                    w,
                    h,
                    natural_w,
                    key,
                };
                // The key always lands, or the region re-requests forever; the epoch moves only
                // with the laid-out extent.
                moved = MeasuredText::layout_moved(d.measured, new);
                d.measured = Some(new);
            }
            if moved {
                // Touched per region, so the layout gate need not re-walk the whole roster; stored
                // whole, as the reference's `SetText` clears the resolved bit (`0x771ea4`).
                model.touch_layout_region(rh);
                model.queue_force(id);
            }
        }
    }

    /// [`Self::set_measured_text`] for unwrapped text, whose two widths are one; for tests.
    pub fn set_measured_text_unwrapped(&mut self, measures: &[(u32, f32, f32, u64)]) {
        let widened: Vec<_> = measures
            .iter()
            .map(|&(id, w, h, key)| (id, w, h, w, key))
            .collect();
        self.set_measured_text(&widened);
    }

    /// Force the next [`UiScript::resolve`] past both change gates to rebuild the whole layout
    /// graph: the scoped resolve's falsifier, as a full rebuild must reach the same rects.
    pub fn force_full_layout_resolve(&mut self) {
        let mut model = self.model_mut();
        model.layout_scope.invalidate();
        model.layout_fingerprint = None;
        model.layout_epoch_resolved = None;
        model.touch_layout();
    }

    /// Drop every cached host text metric when the host's raster environment changes (resize,
    /// fullscreen, uiScale): glyph advances snap to whole pixels at the drawn size, so a stale
    /// measure can fail the ellipsis fit and truncate text that fits.
    pub fn invalidate_text_measures(&mut self) {
        let mut model = self.model_mut();
        model.tooltip_measures.clear();
        for d in model.region_data.values_mut() {
            d.measured = None;
        }
        // A bumped key never matches the recomputed content hash, so the next sweep re-requests.
        for (_, frame) in model.arena.iter_frames_mut() {
            match &mut frame.kind_state {
                KindState::ScrollingMessage(smf) => {
                    for line in &mut smf.lines {
                        line.rows_key = line.rows_key.wrapping_add(1);
                    }
                }
                KindState::EditBox(eb) => {
                    eb.advances_key = eb.advances_key.wrapping_add(1);
                    eb.line_height = None;
                    // A multi-line box's height is its text's measure: owed again on the new
                    // raster, which moves no rect here to re-seat it.
                    eb.relayout_owed = true;
                }
                _ => {}
            }
        }
        // Measured extents are auto-size inputs, the layout gate's read set.
        model.touch_layout();
        // Every stored measure was just wiped; only the whole-roster sweep can re-request them.
        model.touch_measure_all();
    }

    // ── Input: the pointer leaving the window ────────────────────────────────────────────────────

    /// The OS pointer left the window: the hovered frame loses the hover, with its leave notify
    /// ([`mouseover::leave`]) as a walk that found nothing would run it. It is no cursor event, so
    /// it feeds no position: no probe runs and a move or size in flight holds its last sample
    /// (`[root+0xd08]`/`[root+0xd0c]`), as the reference's pump only sees real cursor events
    /// (`0x7655b0`). The saved cursor position stays too.
    pub fn pointer_left_hover(&mut self) {
        let leave = {
            let mut model = self.model_mut();
            // A frame destroyed since it took the hover gets no leave.
            let old = model
                .mouseover
                .take()
                .filter(|&h| model.arena.frame(h).is_some());
            old.map(|h| {
                let script = model
                    .frame_to_id
                    .get(&h)
                    .copied()
                    .filter(|_| button::hover_notify_runs(&model, Some(h)));
                (h, script)
            })
        };
        if let Some((h, script)) = leave {
            mouseover::leave(&self.lua, h, script);
        }
    }

    /// The OS pointer left the window, so no release will be fed: clears [`Model::drag`], which
    /// would start a drag on re-entry, and [`Model::mouse_down_on`], which would click the frame
    /// it left. The app calls it where it fires the synthetic `OnLeave`.
    pub fn pointer_left_window(&mut self) {
        // A started drag ends with `OnDragStop`, as the reference's release always ends it; dropped
        // silently, its `StopMovingOrSizing` never runs and the frame stays glued to the cursor.
        let abandoned = {
            let mut model = self.model_mut();
            cursor::abandon_drag(&mut model)
        };
        if let Some(source) = abandoned {
            self.fire_drag_stop(source);
        }
        let mut model = self.model_mut();
        let held: Vec<crate::widget::FrameHandle> =
            model.mouse_down_on.values().copied().flatten().collect();
        model.mouse_down_on.clear();
        // Every held button gets the release edge never fed (`0x7793de`), else it stays pushed.
        for h in held {
            button::edge(&mut model, h, crate::widget::ButtonState::on_mouse_up);
        }
        // The capture slot (`root+0x80`) too: a stale one aims the next press's raise at it.
        model.mouse_capture = None;
        // [`Model::last_click`] stays: `[CButton+0x334]` has no writer on leave, hide or disable,
        // so a double click survives the cursor leaving and returning within 300 ms.
        // A slider thumb drag is abandoned too: its release is never fed either.
        model.slider_drag = None;
    }

    // ── Keyboard entry ───────────────────────────────────────────────────────────────────────────
    //
    // Keys arrive by name; the host maps its keycodes. Every EditBox is a frame in the key and
    // char walks ([`keyboard`]): the focused box consumes every event at its own strata and level;
    // with none focused, the first visible `autoFocus` box in walk order takes focus and
    // processes that same event; otherwise nothing is consumed.

    /// A typed character (UTF-8) from the host: a focused box inserts it (numeric, cap and
    /// password rules apply) or, for Ctrl+A's control code, selects all. `true` if consumed.
    pub fn char_input(&mut self, text: &str) -> bool {
        // The one dispatcher (`0x765df0`): the boxes are frames in the walk, at their own strata
        // and level.
        keyboard::char_input(&self.lua, text)
    }

    /// Paste host clipboard text into the focused EditBox, typed a character at a time as the
    /// reference's paste is (`0x77e250`), with a typed character's escapes and events. `true` if
    /// a box consumed it.
    pub fn paste(&mut self, text: &str) -> bool {
        editbox::paste(&self.lua, text)
    }

    /// A key pressed, by name (`"ENTER"`, `"A"`, `"SHIFT"`, `"UNKNOWN"`): the key-down walk
    /// (`0x765f10`), which an EditBox joins at its own place. `Box` means the walk reached a box
    /// that took the key, and the host then runs the key's editing action on it
    /// ([`Self::editbox_action`], [`Self::paste`]); `Frame`, a frame that took it, and the host
    /// runs neither the action nor the key's binding (`0x76b7d0`).
    pub fn key_down(&mut self, key: &str) -> KeyWalk {
        keyboard::key_down(&self.lua, key)
    }

    /// A key held down and repeating, by name: the auto-repeat dispatcher (`0x766070`) hands it to
    /// the frame that took the press, a stub on a plain frame and the key-down again on a box.
    pub fn key_repeat(&mut self, key: &str) -> KeyWalk {
        keyboard::key_repeat(&self.lua, key)
    }

    /// A key released, by the name its press went by: the key-up dispatcher (`0x765fd0`) runs
    /// `OnKeyUp` on the frame that consumed that key's last down.
    pub fn key_up(&mut self, key: &str) -> KeyUp {
        keyboard::key_up(&self.lua, key)
    }

    /// A binding took this key's press: the world frame is its consumer (`0x483b70`).
    pub fn note_world_key(&mut self, key: &str) {
        keyboard::note_world_key(&self.lua, key);
    }

    /// Whether ALT is held, which an alt-arrow EditBox reads for the arrows (`0x77b1b3`).
    pub fn set_alt_down(&mut self, alt: bool) {
        self.model_mut().alt_down = alt;
    }

    /// Whether the key-down walk takes `key`, a frame or a box.
    pub fn key_input(&mut self, key: &str) -> bool {
        self.key_down(key) != KeyWalk::Declined
    }

    /// Whether a frame, not a box, takes `key`.
    pub fn frame_key_input(&mut self, key: &str) -> bool {
        self.key_down(key) == KeyWalk::Frame
    }

    /// One text-editing operation on the focused EditBox, from the host's per-OS keymap.
    pub fn editbox_action(&mut self, action: EditAction) -> bool {
        editbox::action(&self.lua, action)
    }

    /// Whether the focused EditBox is in alt-arrow mode (XML `ignoreArrows`, Lua
    /// `SetAltArrowKeyMode`, `[editbox+0x318] & 0x10`): without ALT the reference declines the four
    /// arrows (`0x77b1c4`), so they reach the world's bindings and turn the player while chat has
    /// focus. The gate is on the key, not the [`EditAction`]: HOME and END also move the caret.
    pub fn editbox_alt_arrow_mode(&self) -> bool {
        let model = self.model_ref();
        model.focused_editbox.is_some_and(|h| {
            model.arena.frame(h).is_some_and(|f| {
                f.effective_visible
                    && matches!(&f.kind_state,
                        crate::widget::KindState::EditBox(eb) if eb.alt_arrow_key_mode)
            })
        })
    }

    /// Whether a visible EditBox holds keyboard focus, which gates the world's keys as the client's
    /// `0xcf4dc8 != 0` test does.
    pub fn has_keyboard_focus(&self) -> bool {
        let model = self.model_ref();
        model
            .focused_editbox
            .is_some_and(|h| model.arena.frame(h).is_some_and(|f| f.effective_visible))
    }

    /// The focused EditBox's name, unfiltered by visibility, unlike [`Self::has_keyboard_focus`].
    /// Host-side only: 1.12's EditBox table has `SetFocus`/`ClearFocus` and no getter.
    pub fn focused_editbox_name(&self) -> Option<String> {
        let model = self.model_ref();
        let h = model.focused_editbox?;
        model.arena.frame(h)?.name.clone()
    }

    /// Resolves the layout change gate let through; a per-frame delta of 0 means a quiet frame.
    pub fn layout_solves(&self) -> u64 {
        self.model_ref().layout_solves
    }

    /// Resolves past tier 1, which pay the whole-roster preamble ([`Model::layout_gate_walks`]):
    /// the gate's true cost, at least [`Self::layout_solves`], since a walk may find nothing moved.
    pub fn layout_gate_walks(&self) -> u64 {
        self.model_ref().layout_gate_walks
    }

    /// Whole-graph layout derivations, a resolve's one expensive step; flat while a UI animates.
    pub fn layout_derivations(&self) -> u64 {
        self.model_ref().layout_derives
    }

    /// Fixpoint rounds across every solve; over [`Self::layout_solves`], the per-pass depth.
    pub fn layout_rounds(&self) -> u64 {
        self.model_ref().layout_rounds
    }

    /// The last solve's scope, `(frames solved, regions swept)`: a change to ten FontStrings must
    /// read a handful here however large the UI grows.
    pub fn layout_last_scope(&self) -> (usize, usize) {
        self.model_ref().layout_last_scope
    }

    /// Whether `name` is a registered FrameXML template, case-folded as the loader resolves it, for
    /// the corpus harness: an addon naming a missing template gets a bare frame and no load error.
    pub fn has_framexml_template(&self, name: &str) -> bool {
        self.model_ref().framexml_templates.borrow().contains(name)
    }

    /// Whether `name` is a registered font object, the other namespace an `inherits=` may name
    /// (a FontString's `inherits=` links it, `0x770f69`), so a census counts no font as a missing
    /// template.
    pub fn has_font_object(&self, name: &str) -> bool {
        self.model_ref().font_object(name).is_some()
    }

    /// The widget kind of a published name, in `CreateFrame`'s spelling (`SimpleHTML`): host-side,
    /// for the corpus harness to attribute a call site to a class unseen by any addon.
    pub fn widget_kind(&self, name: &str) -> Option<&'static str> {
        let model = self.model_ref();
        if let Some(h) = model.arena.lookup(name) {
            return model.arena.frame(h).map(|f| match f.kind {
                crate::widget::FrameKind::Frame => "Frame",
                // A `Frame` to Lua: the registered name never becomes a class.
                crate::widget::FrameKind::WorldFrame => "Frame",
                crate::widget::FrameKind::Button => "Button",
                crate::widget::FrameKind::CheckButton => "CheckButton",
                // `GetObjectType` (`0x495b60`) says "LootButton" (`[0x847ce0]` → `0x843414`).
                crate::widget::FrameKind::LootButton => "LootButton",
                crate::widget::FrameKind::EditBox => "EditBox",
                crate::widget::FrameKind::StatusBar => "StatusBar",
                crate::widget::FrameKind::Slider => "Slider",
                crate::widget::FrameKind::ScrollFrame => "ScrollFrame",
                crate::widget::FrameKind::Model => "Model",
                crate::widget::FrameKind::PlayerModel => "PlayerModel",
                crate::widget::FrameKind::DressUpModel => "DressUpModel",
                crate::widget::FrameKind::TabardModel => "TabardModel",
                crate::widget::FrameKind::MessageFrame => "MessageFrame",
                crate::widget::FrameKind::ScrollingMessageFrame => "ScrollingMessageFrame",
                crate::widget::FrameKind::ColorSelect => "ColorSelect",
                crate::widget::FrameKind::SimpleHtml => "SimpleHTML",
                crate::widget::FrameKind::MovieFrame => "MovieFrame",
                crate::widget::FrameKind::GameTooltip => "GameTooltip",
                crate::widget::FrameKind::Minimap => "Minimap",
            });
        }
        // Region leaves publish into their own name table, not the arena's.
        let id = *model.region_by_name.get(name)?;
        let h = *model.id_to_region.get(&id)?;
        model.arena.region(h).map(|r| match r.kind {
            crate::widget::RegionKind::Texture => "Texture",
            crate::widget::RegionKind::FontString => "FontString",
            // A title region is a plain Region (`0x76c440`); `CreateTitleRegion` takes no name.
            crate::widget::RegionKind::Title => "Region",
        })
    }

    /// A snapshot of the script errors collected so far (from `pcall`'d handlers).
    pub fn errors(&self) -> Vec<String> {
        self.model_ref().errors.clone()
    }

    /// Drain the collected script errors.
    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().errors)
    }

    /// A snapshot of non-fatal host warnings (e.g. ignored `CreateFrame` templates).
    pub fn warnings(&self) -> Vec<String> {
        self.model_ref().warnings.clone()
    }

    /// Drain the non-fatal host warnings, for the host to log.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().warnings)
    }

    /// Report a script error the host caught outside the VM's dispatch (an addon file that failed
    /// to compile or raised at file scope): it joins the handler-dispatch queue and the retained
    /// log, not `errors`, since the caller has already logged it. `log` is the decorated text the
    /// log keeps; `handler` is the one value the error handler is called with, the bare Lua
    /// message as `lua_pcall` leaves it (`0x703b40`).
    pub fn report_script_error(&self, log: &str, handler: &str) {
        // A Load row, not an Error one: to the player a file scope that raised and a missing file
        // are the same fact, the addon is not running.
        diagnostics::record_load_failure(&self.lua, log);
        self.model_mut()
            .pending_error_dispatch
            .push(model::HandlerArg::Text(handler.to_string()));
    }

    /// Hand each queued script error to the Lua error handler, as the reference's catch closure
    /// (`0x703b40`) calls the one `seterrorhandler` (`0x702900`) holds, read from its slot and not
    /// through the `geterrorhandler` global; FrameXML installs `_ERRORMESSAGE`, the ScriptErrors
    /// dialog (`BasicControls.xml:16`). With none installed the error stays on the host channel,
    /// [`UiScript::errors`], alone; a handler that raises is recorded there only and stops the
    /// batch, so the path cannot recurse.
    pub fn dispatch_script_errors_to_handler(&mut self) {
        let pending = std::mem::take(&mut self.model_mut().pending_error_dispatch);
        if pending.is_empty() {
            return;
        }
        let Some(handler) = stdlib::error_handler(&self.lua) else {
            return;
        };
        for arg in pending {
            if let Err(e) = arg.value(&self.lua).and_then(|v| handler.call::<()>(v)) {
                self.model_mut()
                    .errors
                    .push(format!("error handler itself failed: {e}"));
                break;
            }
        }
    }

    /// Register a named [`FontObject`] (a resolved `<Font>`), its block replaced by `font` and the
    /// change pushed to what follows it, and publish it as the Lua global `name`.
    pub fn register_font_object(&self, name: &str, font: FontObject) {
        let id = {
            let mut model = self.model_mut();
            let id = model.fonts.named(name);
            font::replace_block(&mut model, id, font);
            id
        };
        // Publishing a fresh table under a string key cannot fail; the record is in place anyway.
        let _ = font::publish(&self.lua, id, name);
    }

    /// A registered [`FontObject`] by name: its resolved paint.
    pub fn font_object(&self, name: &str) -> Option<FontObject> {
        self.model_ref().font_object(name).cloned()
    }

    // ── internals ────────────────────────────────────────────────────────────────────────────

    fn model_ref(&self) -> mlua::AppDataRef<'_, Model> {
        self.lua
            .app_data_ref::<Model>()
            .expect("model app_data set")
    }

    fn model_mut(&self) -> mlua::AppDataRefMut<'_, Model> {
        self.lua
            .app_data_mut::<Model>()
            .expect("model app_data set")
    }

    fn push_error(&self, e: mlua::Error) {
        self.model_mut().record_lua_error(&e);
    }
}

#[cfg(test)]
mod tests;
