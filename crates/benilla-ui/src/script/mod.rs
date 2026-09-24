//! The Lua scripting host: the engine-free VM that runs FrameXML and addons over the frame arena,
//! the anchor resolver and the draw order. It embeds mlua's Lua 5.1, reshaped by [`lua50`] to
//! answer as the 1.12 client's Lua 5.0.
//!
//! A frame's Lua value is a table whose `T[0]` lightuserdata (`0x701bd0`) holds a `u32` id where
//! the reference holds the `CScriptObject*`, with an `__index` metatable (`0x7020b0`). A handler
//! is `pcall`ed with `this`, `event` and `arg1..argN` set as globals and restored after
//! (`0x704d50`); it also gets `(self, event, ...)` as arguments, which 1.12 does not pass.
//!
//! The `LUAI_MAXCSTACK` discipline: Rust holds no persistent Lua handle, since each owned `Table`
//! or `Function` takes a slot on mlua's reference thread, capped at the vendored 8000. Lua-side
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
mod binding_abi;
mod dialog_verbs;
mod tutorial;
mod worldmap_arrow;
// `camera_view`: the five camera views and `FlipCameraYaw`, the reference's `UIUtil\Camera.cpp`.
mod button;
mod camera_view;
mod channel;
mod char_stats;
mod chat_misc;
mod chat_send;
mod chat_types;
mod chat_window;
mod clip;
mod colorselect;
mod container;
mod craft;
mod cursor;
mod death;
pub mod diagnostics;
mod editbox;
pub mod instance;
pub(crate) use editbox::{adopt_text_region, editbox_text_region_wrapper};
pub(crate) mod addon;
pub mod addon_gate;
mod client;
mod cvars;
mod dressup;
mod duel;
pub(crate) mod event;
mod extract;
mod follow;
pub(crate) mod font;
mod font_block;
mod gm_ticket;
mod gossip;
mod guild;
mod handler_prof;
mod screenshot;
mod tabard;
pub use handler_prof::HandlerRow;

mod surface;
pub use surface::widget_method_census;
mod inspect;
mod item_stats;
mod item_text;
pub mod keybind;
mod keyboard;
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
pub mod nameplate;
pub use nameplate::{PlateGeometry, PlateState};
mod model;
mod modelframe;
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
mod summon;
mod talent;
mod taxi;
mod tick;
mod tooltip;
mod tooltip_item;
mod tooltip_spell;
mod tooltip_unit;
mod ui_errors;
pub use tooltip_unit::TooltipTint;
mod trade;
mod tradeskill;
mod trainer;
mod types;
mod unit;
mod weapon_enchant;
mod who_sort;
mod worldmap;
mod worldstate;
mod worn_display;

pub use action::{ActionSlot, ActionState, ActionUse};
pub use addon::AddOnInfo;
pub use addon_message::{AddonDistribution, AddonSend};
pub use auction::{
    AuctionBid, AuctionCategory, AuctionItemRow, AuctionListState, AuctionQuery,
    AuctionStartRequest, AuctionState, AuctionSubCategory, BIDDER, LIST, OWNER, SORT_KEYS,
};
pub use aura::{AuraState, TrackingState};
pub use backdrop::{inset_atlas_bleed, pieces, Backdrop, BackdropPiece, Insets};
pub use bank::BankState;
pub use battlefield_positions::{BattlefieldFlagView, BattlefieldPositionView};
pub use battlefield_queue::{BattlefieldListView, BattlefieldMapInfo, BattlefieldQueueSlot};
pub use battlefield_score::{BattlefieldScoreRow, BattlefieldScores, BattlefieldStatColumn};
pub use bind_confirm::PendingEquipAnswer;
pub use camera_view::{CameraViewRequest, CAMERA_VIEW_COUNT};
pub use channel::{ChannelCommand, ZoneChannelRow};
pub use char_stats::{
    weapon_subclass_skill, BankBagSlots, InvSlotView, InventorySlots, UnitCombatStats,
    BANK_BAG_SLOT_COUNT, INVENTORY_SLOT_COUNT, SKILL_DEFENSE, SKILL_UNARMED,
};
pub use chat_misc::EmoteRequest;
pub use chat_send::ChatSend;
pub use chat_types::ChatTypeColor;
pub use chat_window::{message_group_index, ChatWindowLook, MESSAGE_GROUPS};
pub use container::{
    BagAutoStore, ContainerMove, ContainerSlot, ContainerState, EnchantView, PendingWrap,
    PetitionSlotView, RandomPropertyView, UiCursorMode,
};
pub use craft::{CraftReagent, CraftRecipe, CraftState, CraftTooltip};
pub use cursor::money::coin_icon;
pub use cursor::{
    CursorAction, CursorItem, CursorMacro, CursorMerchantItem, CursorMoney, CursorPayload,
    CursorPetAction, CursorSpell, CursorStablePet, EnchantConfirm, WorldPick, EQUIPMENT_BAG,
};
pub use cvars::{
    MultisampleFormat, ScreenResolution, SeededCvar, VideoCaps, CVAR_FRILL_DENSITY, CVAR_GAMMA,
    CVAR_NAMEPLATE_ENEMIES, CVAR_NAMEPLATE_FRIENDS, CVAR_WORLD_DETAIL, VIDEO_DEFAULT_CVARS,
    WORLD_DETAIL_STOPS,
};
pub use death::{DeathAction, DeathUiState};
pub use dressup::DressUpIntent;
pub use duel::DuelRequest;
pub use follow::FollowRequest;
pub use gm_ticket::{GmTicketIntent, GmTicketWrite};
pub use gossip::{GossipMenu, GossipOptionView, GossipQuestRow};
pub use guild::{
    GuildMemberInfo, GuildRankEdit, GuildRankInfo, GuildRequest, GuildState, LastOnline, UnitGuild,
    MAX_RANKS, MIN_RANKS, RANK_RIGHT_BITS,
};
pub use modelframe::ModelPaneFrame;
pub use petition::{
    validate_guild_name, PetitionRecordView, PetitionRequest, PetitionState, PETITION_TYPE_CHARTER,
    PETITION_TYPE_PETITION,
};
pub use tabard::{
    emblem_mask_path, TabardHost, TabardIntent, EMBLEM_MASK_TOKEN, TABARD_COUNTS,
    TABARD_CREATION_COST,
};
pub use worldmap_arrow::ARROW_MODEL;

pub(crate) use button::{set_label_font_justify_h_lua, LabelFont};
pub use inspect::{InspectView, UnitReach};
pub use item_stats::{item_usable, ItemSetView, ItemTemplateView, PlayerReqState};
pub use item_text::ItemTextState;
pub use layout_cache::{FrameLayout, LayoutPoint};
pub use loot::{LootRow, LootState};
pub use loot_roll::{LootRollEntry, LootRollsState};
pub use macros::{MacroState, MacroView, MAX_MACROS, MAX_MACRO_BODY, MAX_MACRO_NAME};
pub use mail::{MailInboxRow, MailInvoice, MailSendRequest, MailState, StationeryView};
pub use measure::TextMeasure;
pub use merchant::{ItemStatsHead, MerchantItem, MerchantState};
pub(crate) use minimap::apply_model_attrs as apply_minimap_model_attrs;
pub(crate) use model::Model;
pub use model::{FontProbe, TextureProbe, TextureSizeProbe};
pub use party::{PartyMemberInfo, PartyRequest, PartyState, RaidMemberInfo, SavedInstanceInfo};
pub use pet::{PetActionView, PetStats};
pub use pvp::{HonorState, InspectHonorData};
pub use quest::{
    QuestAction, QuestItemView, QuestPanel, QuestRewardSpell, QuestSelect, QuestState,
};
pub use quest_log::{QuestLogDetail, QuestLogEntryView, QuestLogObjectiveView, QuestLogState};
pub(crate) use region::{apply_font_parts, implicit_creation_anchor_lua};
pub use reputation::{FactionEntry, ReputationSend, ReputationState};
pub use session::SessionRequest;
pub use shapeshift::ShapeshiftFormView;
pub(crate) use simplehtml::{
    apply_element_font_parts as apply_simplehtml_font_parts,
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
pub use types::{
    BlendMode, EditAction, EditBoxTextUi, EditOutcome, EditUnit, ExtractedQuad, FontObject,
    FontShadow, Gradient, JustifyH, JustifyV, LineMeasureRequest, MeasureRequest, Outline,
    QuadContent, ScriptValue, TexCoords,
};
pub(crate) use types::{FontExplicit, MeasuredText, RegionData};
pub use unit::{
    grey_band, level_reads_unknown, power_token, unit_is_grey, PlayerRecord, SelectionRequest,
    UnitState,
};
pub use weapon_enchant::WeaponEnchant;
pub use who_sort::{WhoSortChain, WhoSortKey};
pub use worldmap::{
    WorldMapContinentView, WorldMapLandmarkView, WorldMapOverlayView, WorldMapState,
    WorldMapZoneView,
};
pub use worldstate::WorldStateUiView;
pub use worn_display::WornDisplay;

use mlua::Lua;

use crate::layout::Rect;
use crate::order::ZTarget;
use crate::widget::{FrameHandle, KindState};

// Registry keys, the only roots of Lua-side state (the `LUAI_MAXCSTACK` discipline).
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
/// The stdlib's default error handler, kept by identity for
/// [`UiScript::dispatch_script_errors_to_handler`] to skip; stored at [`stdlib::install`].
const REG_DEFAULT_ERRORHANDLER: &str = "__benilla_default_errorhandler";

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
pub(crate) const TEXTURE_ONLY_METHODS: [&str; 12] = [
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
    "GetVertexColor",
];

/// FontString-only: the font, text, justify and shadow block and the string metrics.
/// `GetStringHeight`, `SetFormattedText` and `SetSize` are not 1.12 FontString methods.
pub(crate) const FONTSTRING_ONLY_METHODS: [&str; 21] = [
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
const SCRIPT_KINDS: [&str; 39] = [
    "OnLoad",
    "OnEvent",
    "OnUpdate",
    // The model pane's two, fired by the tick's model pass (`tick_model_panes`): `OnUpdateModel`
    // at the top of a visible pane's paint, `OnAnimFinished` when the armed sequence completes.
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
    // The caret flush's own (`0x77da80`), fired by the tick's `drain_cursor_changed` when the
    // caret moved: the edge `ScrollingEdit_OnCursorChanged` scrolls a multiline box by.
    "OnCursorChanged",
    "OnEditFocusGained",
    "OnEditFocusLost",
    "OnHorizontalScroll",
    "OnVerticalScroll",
    "OnScrollRangeChanged",
    "OnDragStart",
    "OnDragStop",
    "OnReceiveDrag",
    // A release over a message-frame hyperlink span, `OnHyperlinkClick(link, text, button)`;
    // `ChatFrameTemplate` passes it to `SetItemRef` (`ChatFrame.xml:15`, `ChatFrame.lua:1534`).
    "OnHyperlinkClick",
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
    // The key channels, fired by [`keyboard`]'s walk (`0x765f10`). `OnKeyUp` never fires, as the
    // host feeds no key-up, but a frame carrying only `OnKeyUp` still consumes every key-down and
    // runs nothing, the reference's own asymmetry.
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

impl UiScript {
    /// Build a fully sandboxed, stdlib- and object-model-equipped host.
    pub fn new() -> mlua::Result<UiScript> {
        let lua = Lua::new();
        lua.set_app_data(Model::new());
        // Before anything can fire a handler and while no app-data borrow is held, as the
        // profiler's slot requires.
        handler_prof::install(&lua);

        addon::install(&lua)?;
        addon_message::install(&lua)?;
        chat_send::install(&lua)?;
        chat_types::install(&lua)?;
        chat_misc::install(&lua)?;
        channel::install(&lua)?;
        chat_window::install(&lua)?;
        client::install(&lua)?;
        screenshot::install(&lua)?;
        stdlib::sandbox(&lua)?;
        // Before the stdlib layer, so its aliases bind the 5.0-shaped functions.
        lua50::install(&lua)?;
        stdlib::install(&lua)?;
        object::install(&lua)?;
        // After `object`, whose `publish_global` it reuses, and before any FrameXML loads:
        // `Loader::do_font` publishes into the tables this builds.
        font::install(&lua)?;
        unit::install(&lua)?;
        party::install(&lua)?;
        social::install(&lua)?;
        guild::install(&lua)?;
        petition::install(&lua)?;
        binder::install(&lua)?;
        summon::install(&lua)?;
        bind_confirm::install(&lua)?;
        instance::install(&lua)?;
        gm_ticket::install(&lua)?;
        duel::install(&lua)?;
        follow::install(&lua)?;
        camera_view::install(&lua)?;
        session::install(&lua)?;
        pvp::install(&lua)?;
        worn_display::install(&lua)?;
        death::install(&lua)?;
        aura::install(&lua)?;
        cvars::install(&lua)?;
        saved::install(&lua)?;
        keybind::install(&lua)?;
        sound::install(&lua)?;
        ui_errors::install(&lua)?;
        pointer::install(&lua)?;
        action::install(&lua)?;
        action_bar_toggles::install(&lua)?;
        container::install(&lua)?;
        cursor::install(&lua)?;
        spellbook::install(&lua)?;
        macros::install(&lua)?;
        talent::install(&lua)?;
        dialog_verbs::install(&lua)?;
        battlefield_score::install(&lua)?;
        battlefield_queue::install(&lua)?;
        battlefield_positions::install(&lua)?;
        tutorial::install(&lua)?;
        worldmap_arrow::install(&lua)?;
        shapeshift::install(&lua)?;
        pet::install(&lua)?;
        gossip::install(&lua)?;
        merchant::install(&lua)?;
        bank::install(&lua)?;
        stable::install(&lua)?;
        item_text::install(&lua)?;
        mail::install(&lua)?;
        auction::install(&lua)?;
        trainer::install(&lua)?;
        taxi::install(&lua)?;
        trade::install(&lua)?;
        inspect::install(&lua)?;
        dressup::install(&lua)?;
        tabard::install(&lua)?;
        tradeskill::install(&lua)?;
        craft::install(&lua)?;
        reputation::install(&lua)?;
        skills::install(&lua)?;
        item_stats::install(&lua)?;
        char_stats::install(&lua)?;
        weapon_enchant::install(&lua)?;
        loot::install(&lua)?;
        loot_roll::install(&lua)?;
        quest::install(&lua)?;
        quest_log::install(&lua)?;
        messageframe::install(&lua)?;
        scrollframe::install(&lua)?;
        simplehtml::install(&lua)?;
        slider::install(&lua)?;
        colorselect::install(&lua)?;
        minimap::install(&lua)?;
        modelframe::install(&lua)?;
        tooltip::install(&lua)?;
        worldmap::install(&lua)?;
        worldstate::install(&lua)?;
        net_stats::install(&lua)?;
        diagnostics::install(&lua)?;

        let s = UiScript {
            lua,
            instructions: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            session: NEXT_SESSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        };
        Ok(s)
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

    /// Bound how long a chunk may run: past `budget` VM instructions the hook raises a Lua error,
    /// so a loop that never ends fails its addon and the rest still runs. The app arms it only for
    /// the world-entry load, so a hung load reports instead of freezing the loading screen, and
    /// disarms it ([`Self::clear_instruction_budget`]) before the session's first frame.
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
        let new = Rect::new(0.0, 0.0, height, width);
        let mut model = self.model_mut();
        if model.screen == new {
            return false;
        }
        model.screen = new;
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

    /// Push the player's WMO-containment state (the client's `0xceaa60`) onto every Minimap: it
    /// picks which persisted zoom index `GetZoom`/`SetZoom` act on. Call it before the script
    /// tick, on each inside/outside edge and whenever [`Self::minimap_widgets_created`] moves.
    pub fn set_minimap_inside(&mut self, inside: bool) {
        self.for_each_minimap(|m| m.inside = inside);
    }

    fn for_each_minimap(&mut self, mut f: impl FnMut(&mut crate::widget::MinimapState)) {
        let mut model = self.model_mut();
        for h in model.arena.minimap_kinds().to_vec() {
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::Minimap(m) = &mut frame.kind_state {
                    f(m);
                }
            }
        }
    }

    /// Push the player's facing (radians) onto every Minimap's player-arrow `Model`, every frame,
    /// raw: `CMinimap::SetPlayerFacing` (`0x4eb8e0`, fed at `0x4eb0a4`–`0x4eb0b1`) copies it into
    /// the field `Model:SetFacing` (`0x76dce0`) writes, so the arrow's `GetFacing()` reads it.
    pub fn set_minimap_player_facing(&mut self, facing: f32) {
        let mut model = self.model_mut();
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
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::Model(state) = &mut frame.kind_state {
                    state.facing = facing;
                }
            }
        }
    }

    /// Publish `Minimap:GetPingPosition()`'s offsets from the widget centre, as fractions of its
    /// side (x right, y up); the app recomputes them from the ping's world point each frame.
    pub fn set_minimap_ping(&mut self, ping: (f32, f32)) {
        self.model_mut().minimap_ping = ping;
    }

    /// Drain a `Minimap:PingLocation(x, y)`: centre-relative offsets in UI units, x right, y up.
    pub fn take_minimap_ping_request(&mut self) -> Option<(f32, f32)> {
        self.model_mut().minimap_ping_request.take()
    }

    /// `Minimap:SetMaskTexture`'s value from the first Minimap (the app draws one map), `None`
    /// until one is set, while the app keeps [`crate::widget::MINIMAP_DEFAULT_MASK`].
    pub fn minimap_mask_texture(&self) -> Option<String> {
        let model = self.model_ref();
        model.arena.minimap_kinds().iter().find_map(|&h| {
            match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Minimap(m)) => m.mask_texture.clone(),
                _ => None,
            }
        })
    }

    /// Minimap widgets ever created in this VM: a move means a new one needs the containment state.
    pub fn minimap_widgets_created(&self) -> u64 {
        self.model_ref().arena.minimap_created()
    }

    /// Seed every Minimap's two zoom indices from the persisted CVars, as the client's reset path
    /// does (`[0x86f698] ← [[0xb4b410]+0x28]`, `[0x86f69c] ← [[0xb4d90c]+0x28]`). Call it once,
    /// when the UI and the widget exist: from then on `Minimap:SetZoom` keeps the CVar following
    /// the widget, and a repeated seed would fight the +/- buttons.
    pub fn set_minimap_zoom(&mut self, zoom: u8, inside_zoom: u8) {
        let top = crate::widget::MINIMAP_ZOOM_LEVELS - 1;
        let (zoom, inside_zoom) = (zoom.min(top), inside_zoom.min(top));
        self.for_each_minimap(|m| {
            m.zoom = zoom;
            m.inside_zoom = inside_zoom;
        });
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
        match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            Some(crate::widget::KindState::ScrollingMessage(smf)) => {
                smf.add(text.to_string(), r, g, b);
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
        model.arena.set_shown(h, true);
        model.focused_editbox = Some(h);
        true
    }

    /// Drain the chat lines `SubmitChatInput` queued since the last call, for the app to parse.
    pub fn take_chat_input(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().chat_input)
    }

    /// Queue a line as if typed into the chat EditBox and submitted, for probes (`WOW_PROBE_CHAT`).
    pub fn push_chat_input(&mut self, line: String) {
        self.model_mut().chat_input.push(line);
    }

    /// Whether Tab was pressed in the chat edit box since the last call: the whisper cycle's cue.
    pub fn take_chat_tab(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().chat_tab)
    }

    /// Replace the hyperlink spans `(frame, y-up rect, link, markup)` the app rasterized this
    /// frame; a release inside one fires `OnHyperlinkClick` on its frame ([`pointer`]).
    pub fn set_link_spans(&mut self, spans: Vec<(FrameHandle, Rect, String, String)>) {
        let mut model = self.model_mut();
        model.link_spans.clear();
        for (fh, rect, link, markup) in spans {
            model
                .link_spans
                .entry(fh)
                .or_default()
                .push((rect, link, markup));
        }
    }

    /// Whether the frame with this global name is shown with every ancestor (`IsVisible()`).
    pub fn frame_visible(&self, name: &str) -> bool {
        self.model_ref()
            .arena
            .iter_frames()
            .any(|(_, f)| f.name.as_deref() == Some(name) && f.effective_visible)
    }

    /// The named frame's effective alpha while it is effectively visible: the app's minimap ping
    /// sprite follows the stock `MiniMapPing` `<Model>` this way.
    pub fn frame_effective_alpha(&self, name: &str) -> Option<f32> {
        self.model_ref()
            .arena
            .iter_frames()
            .find(|(_, f)| f.name.as_deref() == Some(name) && f.effective_visible)
            .map(|(_, f)| f.effective_alpha)
    }

    /// Resolve and cache every frame's rect, which `GetWidth`/`GetHeight`/`extract` read, then fire
    /// `OnSizeChanged` for frames whose size moved ([`event::fire_size_changes`]).
    pub fn resolve(&mut self) {
        {
            let mut model = self.model_mut();
            Self::resolve_layout(&mut model);
        }
        // With a font engine installed ([`Self::set_text_measurer`]), measure what the solve
        // revealed and solve again, so a FontString's box is right in the frame its text was set.
        if self.fill_measures() {
            let mut model = self.model_mut();
            Self::resolve_layout(&mut model);
        }
        event::fire_size_changes(&self.lua);
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
                // Touched per region, so the layout gate need not re-walk the whole roster.
                model.touch_layout_region(rh);
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
        let held: Vec<crate::widget::FrameHandle> = model.mouse_down_on.values().copied().collect();
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
    // Keys arrive by name; the host maps its keycodes. The box routing: a focused box consumes
    // every event; with none focused, the topmost visible `autoFocus` box takes focus and
    // processes that same event; otherwise nothing is consumed.

    /// A typed character (UTF-8) from the host: a focused box inserts it (numeric, cap and
    /// password rules apply) or, for Ctrl+A's control code, selects all. `true` if consumed.
    pub fn char_input(&mut self, text: &str) -> bool {
        // The frame walk first ([`keyboard`]), the focused box a participant at its own strata and
        // level, as in the reference's one dispatcher; an unconsumed event goes to the box routing.
        keyboard::char_input(&self.lua, text) || editbox::char_input(&self.lua, text)
    }

    /// Paste host clipboard text into the focused EditBox as one edit: newlines survive only in a
    /// `multiLine` box, other control characters drop. `true` if a box consumed it.
    pub fn paste(&mut self, text: &str) -> bool {
        editbox::paste(&self.lua, text)
    }

    /// A non-character key by name (`"ENTER"`, `"ESCAPE"`, `"TAB"`); editing keys come through
    /// [`Self::editbox_action`]. A focused box consumes a key even when it does nothing with it.
    pub fn key_input(&mut self, key: &str) -> bool {
        // The same two stages as `char_input`.
        keyboard::key_input(&self.lua, key) || editbox::key_input(&self.lua, key)
    }

    /// An editing key (BACKSPACE, DELETE, the arrows, HOME, END) offered to the keyboard frames
    /// before the focused box gets it as an [`EditAction`]. On `true` the caller dispatches neither
    /// the action nor the key's binding (consumption suppresses it, `0x76b7d0`).
    pub fn frame_key_input(&mut self, key: &str) -> bool {
        keyboard::frame_key_input(&self.lua, key)
    }

    /// One text-editing operation on the focused EditBox, from the host's per-OS keymap.
    pub fn editbox_action(&mut self, action: EditAction) -> bool {
        editbox::action(&self.lua, action)
    }

    /// Whether the focused EditBox is in alt-arrow mode (XML `ignoreArrows`, Lua
    /// `SetAltArrowKeyMode`, `[editbox+0x318] & 0x10`): without ALT the reference declines the four
    /// arrows (`0x77b1c4`), so they reach the world's bindings and turn the player while chat has
    /// focus. The gate is on the key, not the [`EditAction`]: HOME and END also move to an edge.
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
        let model = self.model_ref();
        let templates = model.framexml_templates.borrow();
        templates.contains_key(name) || templates.keys().any(|k| k.eq_ignore_ascii_case(name))
    }

    /// Whether `name` is a registered font object, the other namespace an `inherits=` may name
    /// (`loader::expand_region` forks on it), so a census counts no font as a missing template.
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
        let id = *model.region_names.get(name)?;
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
    /// log, not `errors`, since the caller has already logged it.
    pub fn report_script_error(&self, msg: &str) {
        // A Load row, not an Error one: to the player a file scope that raised and a missing file
        // are the same fact, the addon is not running.
        diagnostics::record_load_failure(&self.lua, msg);
        self.model_mut()
            .pending_error_dispatch
            .push(msg.to_string());
    }

    /// Hand each queued script error to the Lua error handler, as the reference invokes the one
    /// `seterrorhandler`/`geterrorhandler` (`0x702900`/`0x702950`) hold on a caught error;
    /// FrameXML installs `_ERRORMESSAGE`, the ScriptErrors dialog (`BasicControls.xml:16`). The
    /// stdlib default is skipped, as it already reported into [`UiScript::errors`]; a handler that
    /// raises is recorded on the host channel only and stops the batch, so the path cannot recurse.
    pub fn dispatch_script_errors_to_handler(&mut self) {
        let pending = std::mem::take(&mut self.model_mut().pending_error_dispatch);
        if pending.is_empty() {
            return;
        }
        let handler: Option<mlua::Function> = self
            .lua
            .globals()
            .get::<mlua::Function>("geterrorhandler")
            .ok()
            .and_then(|g| g.call::<mlua::Function>(()).ok());
        let Some(handler) = handler else { return };
        if let Ok(default) = self
            .lua
            .named_registry_value::<mlua::Function>(REG_DEFAULT_ERRORHANDLER)
        {
            if handler == default {
                return;
            }
        }
        for msg in pending {
            if let Err(e) = handler.call::<()>(msg) {
                self.model_mut()
                    .errors
                    .push(format!("error handler itself failed: {e}"));
                break;
            }
        }
    }

    /// Register a named [`FontObject`] (a resolved `<Font>`), replacing any of that name, and
    /// publish it as the Lua global `name`, the same pair `Loader::do_font` performs.
    pub fn register_font_object(&self, name: &str, font: FontObject) {
        self.model_mut()
            .font_objects_by_lower
            .insert(name.to_ascii_lowercase(), font);
        // Publishing a fresh table under a string key cannot fail; the record is in place anyway.
        let _ = font::publish(&self.lua, name);
    }

    /// A registered [`FontObject`] by name: its resolved paint.
    pub fn font_object(&self, name: &str) -> Option<FontObject> {
        self.model_ref().font_object(name).cloned()
    }

    /// Every registered [`FontObject`]: the glyph atlas bakes outlined cells for the distinct
    /// `(font, height, outline)` triples here.
    pub fn font_objects(&self) -> Vec<FontObject> {
        self.model_ref()
            .font_objects_by_lower
            .values()
            .cloned()
            .collect()
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
        self.model_mut().record_script_error(e.to_string());
    }
}

#[cfg(test)]
mod tests;
