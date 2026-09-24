//! The Lua scripting host — the engine-free VM that turns the arena/layout/order *model* into a
//! live, addon-facing runtime. No Bevy, no GPU: this module embeds mlua's Lua 5.1
//! (the Classic Era version) and wires it to [`crate::widget`]'s frame arena, [`crate::layout`]'s
//! anchor resolver, and [`crate::order`]'s draw-order traversal, exposing the WoW FrameScript object
//! model and the slice-of stdlib addons expect.
//!
//! ## Ground truth
//!
//! - **The frame object model** (`0x701bd0`/`0x7020b0`): a frame's Lua value
//!   is a table `T` with `T[0] = lightuserdata(handle)`, one *shared* metatable
//!   `__framescript_meta` whose `__index` dispatches by method name; named frames auto-publish to
//!   `_G` non-overwriting. See [`object`].
//! - **The handler-firing context** (`0x704d50`): a handler is fired via `pcall`
//!   with `this`/`event`/`arg1..argN` set as **globals**, saved-then-restored around the call
//!   (nesting-safe). We also pass the *modern* `(self, event, ...)` arguments the Era addons expect
//!   (the transition-era client did both). See [`event`].
//!
//! ## The `LUAI_MAXCSTACK` discipline (probe A)
//!
//! Probe A found the hard constraint that Rust must **never** hold thousands of persistent mlua
//! handles (each owned `Table`/`Function` occupies a slot on mlua's reference thread, capped by the
//! vendored build's `LUAI_MAXCSTACK = 8000`). So this host holds **zero** persistent Lua handles in
//! Rust: every piece of Lua-side state — the two shared metatables, the two method tables, the
//! wrapper cache (`id → wrapper table`), and every frame's script closures (`id → {name → fn}`) —
//! lives in the Lua **registry** under named keys and is fetched transiently per call. The Rust-side
//! [`Model`] is *plain data* (arena, layout inputs, id↔handle maps, region visuals, event
//! registrations, errors) stored in `lua.app_data`, reachable from callbacks via `RefCell`-style
//! dynamic borrows. Every callback takes a *short* borrow and drops it before re-entering Lua.
//!
//! ## Identity: ids, not handle bits
//!
//! `0x701bd0` stores the `CScriptObject*` in the lightuserdata. Benilla's
//! [`crate::widget::FrameHandle`] is an opaque generational handle with private fields (no bit
//! accessor), so we mint a stable `u32` **id** per handle and store *that* in the lightuserdata
//! (`id as *mut c_void`); [`Model`] owns the `id ↔ handle` bijection. The id doubles as the
//! [`crate::layout::Handle`] used by the layout graph (frames only; the screen root is the reserved
//! id [`SCREEN`]). This is faithful to the reference's intent (an opaque identity in a
//! lightuserdata) — only the *encoding* differs.

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
// The five camera views + FlipCameraYaw — the reference's `UIUtil\Camera.cpp` Lua surface.
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

/// The widget-method surface measurement — shared by the `dump_widget_methods` example and the
/// widget-surface gate.
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
/// The 19 Region-map methods, shared by frames and regions the way the reference shares them
/// — its header is the byte-verified chain and the bug that found the split.
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
/// The `/who` list's seven-key sort chain and its comparator — its header is the whole
/// mechanism, including why a repeated header click reverses.
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

// Registry key names — the only place Lua-side roots are kept alive (the MAXCSTACK discipline).
const REG_FRAME_META: &str = "__benilla_frame_meta";
const REG_REGION_META: &str = "__benilla_region_meta";
const REG_FRAME_METHODS: &str = "__benilla_frame_methods";
const REG_REGION_METHODS: &str = "__benilla_region_methods";
/// The full table's registry key, exposed for the exhaustiveness gate in `tests::reference_surface`
/// — the split's correctness is a property of what the VM HOLDS, not of the source that builds it.
#[cfg(test)]
pub(crate) const REG_REGION_METHODS_FOR_TEST: &str = REG_REGION_METHODS;
/// The **title region's** method table + metatable — the 19 Region methods and nothing else.
const REG_TITLE_METHODS: &str = "__benilla_title_methods";
const REG_TITLE_META: &str = "__benilla_title_meta";
/// The two region LEAF tables + metatables — Texture and FontString each answer their own map.
const REG_TEXTURE_METHODS: &str = "__benilla_texture_methods";
const REG_TEXTURE_META: &str = "__benilla_texture_meta";
const REG_FONTSTRING_METHODS: &str = "__benilla_fontstring_methods";
const REG_FONTSTRING_META: &str = "__benilla_fontstring_meta";
/// The stdlib's out-of-the-box error handler, kept by identity so
/// [`UiScript::dispatch_script_errors_to_handler`] can tell "nobody chose a handler" (skip — the
/// default already reports into the host channel) from "FrameXML or an addon installed one"
/// (dispatch — that is what the pair exists for). Stored at [`stdlib::install`] time, the one
/// moment the default is known to be what `geterrorhandler()` answers.
const REG_DEFAULT_ERRORHANDLER: &str = "__benilla_default_errorhandler";

/// The **Region method map** (`0xcf54b4`) — the 19 names every region leaf reaches through its own
/// lookup's fallback, fixed and asserted as a SET by `tests::reference_surface`. Named here because
/// two things need the same list: that test, and the title region's narrower method table (1250
/// §5).
/// Names on **both** region leaves — and each leaf registers its own copy (Texture's own
/// `SetAlpha` `0x79b580`, FontString's `0x79cb70`), so these are NOT on the Region map and must
/// not be hoisted into it.
/// `GetDrawLayer` is the setter's pair on both leaves (Texture `0x79a6c0`, FontString `0x79c660`)
/// and is here now — it was found absent, which was true and was a gap, not a decision; pfUI's
/// `GetNoNameObject` reads it off every child of a frame it reskins.
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

/// **Texture-only.** Note `GetVertexColor` sits here while `SetVertexColor` is shared — an asymmetry
/// no reasonable partition invents, and the bytes call it out. Also note the client's Texture map
/// has `SetGradientAlpha` where FontString has `SetAlphaGradient`: a near-miss pair, and we install
/// only the FontString one.
///
/// **Every name here is the client's own now.** Three of ours were parked in this list pending a
/// per-name check, and all three are gone: `SetRotation` (1.12 registers it only on PlayerModel,
/// `0x84f1fc`/`0x505f00`, and the world-map arrow that justified it is a Model frame now — see
/// `region/paint.rs` at its old site), `SetPortraitToTexture` (1.12 has it as an engine GLOBAL,
/// never a Texture method — `region.rs` registers it there, and a test pins the absence), and
/// `SetSize`, the Era geometry verb that was in neither client map and that nothing outside our
/// own test scaffolding called (decision 2142's census). The 1244/1245 landing partitioned rather
/// than pruned and said the pruning was a separate question per name; this is the answer to three
/// of them.
///
/// Going the other way: `GetBlendMode`, `SetTexCoordModifiesRect` and `GetTexCoordModifiesRect`
/// joined the list, all three of them in the client's Texture map `0x87c128` (`0x79a890` /
/// `0x79c080` / `0x79c120`), and all three of them missing here until then.
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

/// **FontString-only** — the font/text/justify/shadow block plus the string metrics.
///
/// **Every name here is the client's own now.** Three of ours were parked in this list:
/// `GetStringHeight` went first (1251 — byte-verified absent, and ours was a byte-identical
/// duplicate of `GetHeight`), and `SetFormattedText` (not among the client's 32) and
/// `SetSize` (the Era geometry verb the Texture list carried too) went with 2142's census, which
/// found neither in the stock chain nor in either addon corpus. The era spelling of
/// `SetFormattedText` is `SetText(format(fmt, ...))`; of `SetSize`, `SetWidth` + `SetHeight`.
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

/// The reserved layout [`crate::layout::Handle`] of the screen root (the client's `CSimpleTop` /
/// `UIParent`), whose rect is the physical screen. Top-level frames whose `SetPoint` omits a
/// `relativeTo` anchor to it. Real frame ids are minted from `1` upward so they never collide.
pub const SCREEN: crate::layout::Handle = 0;

/// The FrameScript handler kinds this host models. The first five are the lifecycle/event set; the
/// six mouse handlers are driven by the hit-testing API in [`pointer`] ([`UiScript::mouse_move`] /
/// [`mouse_button`](UiScript::mouse_button) / [`mouse_wheel`](UiScript::mouse_wheel)) — the app-side
/// event feed (net/window → these calls) is the Bevy side's job. `OnValueChanged`
/// is shared by the StatusBar (`+0x32c`) and the Slider (`+0x330`) — one name,
/// each kind dispatching to its own value-changed slot. The eight
/// `On*Pressed`/text/focus slots are the EditBox's specialized scripts (vtable `0x81c910`): a
/// focused EditBox fires ONLY these, never generic `OnKeyDown`/`OnChar` (its C++ override replaces
/// those slots).
/// `OnHorizontalScroll`/`OnVerticalScroll`/`OnScrollRangeChanged` are the ScrollFrame's own slots
/// (the reference's `[+0x32c]`/`[+0x334]`/`[+0x33c]`, script-name map `0x786c40`).
/// `OnHorizontalScroll` joined the other two with the horizontal offset pair — it is fired by
/// `SetHorizontalScroll`, which is what earns it the row below.
/// `OnDragStart`/`OnDragStop`/`OnReceiveDrag` are the drag trio — driven by
/// `RegisterForDrag` + the same mouse path as the six mouse handlers above, not a separate one.
/// `OnColorSelect` is the ColorSelect's own slot (`+0x338`), fired by its `SetColorRGB`.
///
/// **Every name here is FIRED by something.** A kind that the engine can accept but never raise is
/// strictly worse than the `SetScript: unsupported script` error it replaces — the addon's handler
/// silently never runs and nothing anywhere says so (the bug class decisions 1203/1205/1211 each
/// record). So a script name earns its row here only together with the code that fires it, and the
/// names the reference has that we do NOT fire stay OUT, with the reason recorded at
/// [`crate::script::object::events_regions::set_script`].
///
/// **This list is FLAT; the reference's set is per widget type** (script-name→slot
/// resolvers: base map `0x76a0d0` + the type's own additions — a `<Frame>` has no `OnClick`). That
/// divergence is deliberate and measured, and the 1751 migration has been shrinking the debt it
/// covers. Our own FrameXML used to rely on it in 9 places; seven of them were
/// `PlayerFrame`/`TargetFrame`/`PetFrame`/`PartyMemberFrame1-4` declared as mouse-enabled
/// `<Frame>`s carrying `OnClick` in our transcribed `UnitFrames.xml`, and that file is gone — the
/// reference declares all seven as `<Button>`, which carries `OnClick` by type. What is left of
/// ours is `ChatFrame1`/`ChatFrame2`, plus the corpus's 21 (20 `EditBox` + 1 `<Frame>`
/// `OnEscapePressed`), all of which fire today ([`button::click_button`]'s "plain frames can carry
/// one too" arm). Going per-type is the faithful shape and is what would give us `HasScript` (948
/// corpus call sites across 91 addons); it removes working behaviour from the 23 sites that remain,
/// so it is still a change to make deliberately rather than as a side effect of widening this list
/// — but the FrameXML half of "with FrameXML fixed first" is most of the way there now.
const SCRIPT_KINDS: [&str; 39] = [
    "OnLoad",
    "OnEvent",
    "OnUpdate",
    // The model pane's two: fired by the tick's model pass — `OnUpdateModel` at the top of every
    // paint of a visible pane, `OnAnimFinished` when a clamped sequence completes (
    // `object::events_regions::set_script`'s doc has the sites).
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
    // caret has moved — the edge `ScrollingEdit_OnCursorChanged` + `ScrollingEdit_OnUpdate` scroll
    // a multiline box by. It earns its row here the way this list's rule requires: together with
    // the code that fires it.
    "OnCursorChanged",
    "OnEditFocusGained",
    "OnEditFocusLost",
    "OnHorizontalScroll",
    "OnVerticalScroll",
    "OnScrollRangeChanged",
    "OnDragStart",
    "OnDragStop",
    "OnReceiveDrag",
    // A release over a message-frame hyperlink span (`OnHyperlinkClick(link, text, button)` —
    // the ChatFrameTemplate wires it to SetItemRef; decision 0288 P2).
    "OnHyperlinkClick",
    // The GameTooltip's engine-fired widget scripts (the real template wires all
    // three: money render, money clear, world-hover default placement).
    "OnTooltipAddMoney",
    "OnTooltipCleared",
    "OnTooltipSetDefaultAnchor",
    // The ColorSelect's own slot (`0x78b4f0` script-map, `+0x338`): `OnColorSelect(r, g, b)`
    // — how the colour picker paints its preview swatch, and how TipBuddy's two private
    // `<ColorSelect>` frames learn a colour changed.
    "OnColorSelect",
    // The Button/CheckButton double click (script-map `0x778c50`, `+0x4d4`) —
    // `OnDoubleClick(self, button)`, fired by [`pointer`]'s release-edge detector *instead of* the
    // second `OnClick`, 300 ms (`0x77937b`). **The corpus's
    // single biggest script gap**: 250 call sites across 85 addons, and the *only* thing behind the
    // harness's entire `SetScript: unsupported script` blocker row — 8 addons, 5 dying at load and
    // 3 at session start, every one of them a FuBar plugin or a Titan panel button
    // (`FuBarPlugin-2.0:CreateBasicPluginFrame` wires one, unguarded, on its panel Button).
    "OnDoubleClick",
    // The layout event (base map `0x76a0d0` `+0x120`), fired from the resolve pass by
    // [`crate::layout::size_changed`]'s byte-verified epsilon test — see
    // [`UiScript::resolve_layout`]. `OnSizeChanged(self, width, height)`.
    "OnSizeChanged",
    // The three KEY channels, unblocked by [`keyboard`]'s walk (`0x765f10`). They were the
    // standing exception in
    // [`object::events_regions`]'s note — accepted only once something fired them, which is that
    // module's whole rule. `OnKeyUp` rides in with the other two deliberately: it is *gated* today
    // (a frame carrying only an OnKeyUp consumes every key-down and runs nothing — the reference's
    // own asymmetry) even though this engine's host feeds no key-up to fire it with, and accepting
    // the name is what makes that consumption reachable.
    "OnChar",
    "OnKeyDown",
    "OnKeyUp",
];

// ─────────────────────────────────────────────────────────────────────────────────────────────
// UiScript — the public host
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The Lua scripting host. Owns the mlua VM; the frame arena, layout store, and event registry live
/// inside it (in `lua.app_data`, per the MAXCSTACK discipline) and are driven through this API.
///
/// Construction sandboxes the VM (removes `io`/`os`/`package`/`require`/`dofile`/`loadfile`/`debug`,
/// text-only chunk loading — see [`stdlib`]), installs the WoW stdlib layer (the global aliases, the
/// positional `format`, the `strsplit` family, `getglobal`/`setglobal`, `wipe`, …), and installs the
/// FrameScript object model (`CreateFrame` + the widget/region method surface, the shared metatables).
///
/// The host surface is split, `impl UiScript` blocks beside their concern (the `layout.rs` pattern):
/// the render-list builder in [`extract`], the per-frame runtime loop in [`tick`], the EditBox text
/// seam in [`editbox::seam`], the layout resolve in [`layout`], and the pointer/hit-test in
/// [`pointer`]. What stays here is construction and the small host-facing state pushes/queries.
pub struct UiScript {
    lua: Lua,
    /// VM instructions executed, counted only while a budget is installed
    /// ([`UiScript::set_instruction_budget`]). `Arc` because the hook callback outlives this
    /// borrow; `Relaxed` because nothing orders against it — it is a bound, not a clock.
    instructions: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// **This VM's identity** — see [`UiScript::session`].
    session: u64,
}

/// Hands out [`UiScript::session`] ids. Process-global and monotone, so an id is never reused and
/// two of them can be compared without holding either VM.
static NEXT_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// How often the instruction hook fires — the resolution of an instruction budget.
///
/// mlua's own documentation warns that a low value "can incur a very high overhead", and nothing
/// wants a precise bound: the question a budget answers is "is this chunk ever going to stop?",
/// where being out by a million instructions costs nothing and firing every instruction would cost
/// the whole survey's runtime.
pub const INSTRUCTION_HOOK_STEP: u32 = 1_000_000;

/// The chunk name the CLIENT gives an addon's file: `Interface\AddOns\<Folder>\<File>`.
///
/// Backslashes, and the `Interface\AddOns\` prefix, because addons PARSE this. `FuBarPlugin-2.0`
/// derives each plugin's own folder from a stack trace —
/// `string.find(debugstack(6, 1, 0), "\\AddOns\\(.*)\\")` (`FuBarPlugin-2.0.lua:752`) — and
/// feeds the capture straight into `format("Interface\\AddOns\\%s\\icon", self.folderName)`.
/// With no name set, mlua defaults the chunk to the Rust caller location, the pattern misses,
/// `folderName` is nil, and every FuBar plugin dies formatting it. That was 20 addons.
///
/// The greedy `(.*)` in their pattern is why the FILE has to be in the name too: it captures up to
/// the LAST backslash, so `…\AddOns\FuBar_BagFu\FuBar_BagFu.lua` yields `FuBar_BagFu`. A name
/// stopping at the folder would capture the empty string.
pub fn addon_chunk_name(folder: &str, file: &str) -> String {
    // `@` is Lua's "this chunk is a file" marker; the traceback then prints the path plainly.
    format!("@Interface\\AddOns\\{folder}\\{}", file.replace('/', "\\"))
}

impl UiScript {
    /// Build a fully sandboxed, stdlib- and object-model-equipped host.
    pub fn new() -> mlua::Result<UiScript> {
        let lua = Lua::new();
        lua.set_app_data(Model::new());
        // Before anything can fire a handler, and while nothing holds an app-data borrow — the two
        // conditions the profiler's slot has to be installed under.
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
        // After `object` (it reuses the frame side's `publish_global`), before any FrameXML is
        // loaded — `Loader::do_font` publishes into the tables this builds.
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

    /// The embedded VM — for the Bevy plugin / TOC-XML loader to add the game-state API bindings
    /// ("the Bevy side owns … the API bindings that touch ECS/net") on top of the
    /// object model this crate installs.
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// **Which VM this is** — a fresh number for every [`UiScript::new`], never reused.
    ///
    /// The client destroys its Lua state and builds another several times over a login cycle, so a
    /// host that remembers *what it last pushed into the VM* is remembering something that may no
    /// longer exist. Anything the host seeded — a registry, a catalog, a change-detection memo — is
    /// only valid for the session it was seeded into, and this number is what says so.
    ///
    /// **The addresses, corrected** (2226; this doc carried `0x490bd0` ↔ `0x48fbf0` from 1290 and
    /// that pair is wrong). `ds:0xceef74` is a single global *slot* holding successive instances,
    /// written at exactly two sites image-wide: `0x7039ed` opens, `0x703bab` closes. The reset
    /// choke point `0x703b80` closes-if-present and then **tail-jmps** into the open — which is why
    /// a `call`-only census of it reads "one state per process" and is wrong. Its three callers are
    /// `0x48fe97` (inside `UI_Init 0x48fbf0`, unconditional), `0x491231` (`ShutdownGame`,
    /// unconditional) and `0x46a87b` (the glue builder, gated on its arg). `0x490bd0` destroys the
    /// frame-script *owner object* (`0x490c97`, vtable `0x81c380` slot+4 = `0x764360`) and nils the
    /// 216 bindings; it never touches `ds:0xceef74`.
    ///
    /// So the boundary is `0x48fe97` ↔ `0x491231`/`0x46a87b`: **the rebuild replaces the state, the
    /// teardown does not** — and the rebuild lives *inside* the function that loads FrameXML, which
    /// is why benilla's world entry mints its VM at the top of its own load (2226) rather than
    /// adopting the character screen's. GlueXML and FrameXML never share an instance.
    ///
    /// A host keying its memory on this cannot go stale by omission: a new VM simply does not
    /// match, so the seed happens again. That is the property, and it is why this is a VM-side fact
    /// rather than a host-side counter the host must remember to bump.
    pub fn session(&self) -> u64 {
        self.session
    }

    /// **Bound how long a chunk may run, so an infinite loop reports instead of hanging.**
    ///
    /// A missing capability in this engine has always been a silently WRONG ANSWER — a setter that
    /// ignores you, a getter that says nil (1203/1205/1211/1230). Decision 1247 met the other kind:
    /// `date("*t")` returned a string where Lua returns a table, so `Accountant_WeekStart`'s
    /// `while thisDay ~= weekstart` never terminated and the addon spun the VM forever. That is
    /// invisible to every instrument we own, because an instrument that never returns produces no
    /// roster to diff, no column to compare and no error row to read — the 218-addon survey simply
    /// stopped finishing, and the cause was found by bisecting the corpus BY HAND.
    ///
    /// So a caller that runs untrusted chunks can bound them. Past `budget` VM instructions the
    /// hook raises, and the raise propagates like any other Lua error: the addon reports as failed
    /// with a distinctive message, and everything after it still runs.
    ///
    /// **This is opt-in, and the app arms it only on the world-entry load edge** (
    /// it began harness-only, e463649e). A real session must not kill a player's addon for being
    /// slow, so steady state — every OnUpdate, every event — runs unhooked; but a load walk that
    /// never returns is a client frozen on the loading screen with zero diagnostics (B271's
    /// class), so the entry edge is bounded and [`Self::clear_instruction_budget`] disarms before
    /// the session's first frame.
    ///
    /// The hook fires every [`INSTRUCTION_HOOK_STEP`] instructions rather than every instruction:
    /// mlua's own docs warn that a low value "can incur a very high overhead", and the step is the
    /// resolution of the bound, which nothing here needs to be precise.
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

    /// Remove an installed instruction budget — the load edge's disarm: the bound
    /// covers the world-entry walk, and a session's steady state runs unhooked exactly as before.
    /// The counter keeps its last value, so [`Self::instructions_used`] still answers for the
    /// phase that just ended.
    pub fn clear_instruction_budget(&self) {
        self.lua.remove_hook();
    }

    /// VM instructions executed since the last [`Self::set_instruction_budget`], to the resolution
    /// of [`INSTRUCTION_HOOK_STEP`]. Zero when no budget was ever set — the hook is what counts.
    ///
    /// Reported rather than merely used, because the budget has to be CHOSEN from the corpus and a
    /// number nobody can read is a number nobody can revisit.
    pub fn instructions_used(&self) -> u64 {
        self.instructions.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set the screen-root rect from a pixel size (`[0,0]` origin, y-up). Top-level frames anchor to
    /// it; changing it invalidates the next `resolve`. The app calls this every frame with an
    /// almost-always-identical size — compared before writing so the per-frame idiom doesn't
    /// dirty the layout gate's tier 1 (`Model::touch_layout`).
    ///
    /// **Returns whether the size actually changed.** Anchors follow the new rect on their own, so
    /// most of the UI needs nothing; what does not follow is anything that *computed* a seat from
    /// the old height and stored it — the open-bag stack wraps into a fresh column from
    /// `GetScreenHeight()`, and that answer is stale the moment the window is resized. The 1.12
    /// client never had to care (its resolution changed through a restart, and its own
    /// `DISPLAY_SIZE_CHANGED` listeners are three model panes, none of them the manage pass), but
    /// benilla runs in a freely resizable window, so the caller re-runs
    /// `UIParent_ManageFramePositions()` on a true return.
    pub fn set_screen_size(&mut self, width: f32, height: f32) -> bool {
        let new = Rect::new(0.0, 0.0, height, width);
        let mut model = self.model_mut();
        if model.screen == new {
            return false;
        }
        model.screen = new;
        model.touch_layout();
        drop(model);
        // The implicit rects are measured in layout units, which follow the aspect (2015).
        self.reapply_implicit_rects();
        true
    }

    /// Replace the Era atlas table — pushed once at boot, before the XML loads.
    /// Push the modifier-key state (shift, ctrl, alt) behind `IsShiftKeyDown`/`IsControlKeyDown`/
    /// `IsAltKeyDown`. The app's input pass calls this BEFORE feeding the frame's mouse events, so
    /// a click handler's modifier fork (the reference's shift-split / ctrl-dressup /
    /// shift-pickup) reads the state as of the click.
    pub fn set_modifiers(&mut self, shift: bool, ctrl: bool, alt: bool) {
        let mut model = self.model_mut();
        model.modifiers = (shift, ctrl, alt);
    }

    /// Push the player's WMO-containment state onto every Minimap widget (the client's `0xceaa60`).
    /// It selects which of the two persisted zoom indices `GetZoom`/`SetZoom` act on, so the zoom
    /// buttons drive the indoor level while indoors and the outdoor level while outside. The app
    /// owns the containment test, so it owns this push — call it before the script tick. The
    /// caller pushes on the inside↔outside edge and whenever [`Self::minimap_widgets_created`]
    /// moved (a widget born after the last transition still gets told, without paying the arena
    /// walk every frame).
    pub fn set_minimap_inside(&mut self, inside: bool) {
        self.for_each_minimap(|m| m.inside = inside);
    }

    /// Run `f` over every live Minimap widget's state. Goes through the arena's Minimap registry
    /// rather than a full `iter_frames_mut`, for the reason that registry exists: a corpus UI is
    /// three to four thousand frames and there is normally exactly one Minimap, and one of these
    /// feeds ([`Self::set_minimap_player_facing`]) runs every frame.
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

    /// Push the player's world facing (radians) onto every Minimap's **player-arrow** `Model` —
    /// the client's `CMinimap::SetPlayerFacing 0x4eb8e0`, whose whole 34-byte body is
    /// `[minimap+0x33c] = arg` then, guarded on non-null, `[[minimap+0x338]+0x39c] = arg`: a raw
    /// dword copy into the same field `Model:SetFacing 0x76dce0` writes, with **no negation, offset
    /// or unit change**. Its one caller (`0x4eb0a4`–`0x4eb0b1`) feeds it the player's `GetFacing()`
    /// straight off the FPU stack, so what an addon reads back is the unit's orientation exactly.
    ///
    /// This is what makes `({Minimap:GetChildren()})[9]:GetFacing()` — Questie's `GetPlayerFacing`,
    /// and pfQuest's `compat/client.lua` — return the truth rather than the ctor default `0`, which
    /// is the difference between a waypoint arrow that points at the waypoint and one aimed
    /// permanently north. Called every frame by the app, which owns the player.
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

    /// Publish the live minimap ping's **normalized** offsets from the widget centre (fractions
    /// of the widget side, x right / y up), or `None` when no ping is live —
    /// `Minimap:GetPingPosition()`'s source. The app recomputes this from the ping's WORLD point
    /// every frame a ping is live, before the tick, so a poller sees it track the world as the
    /// player walks.
    ///
    /// One field, not one per widget: there is exactly one ping, and the old
    /// per-widget push walked the whole ~3k-frame arena every frame a ping was live.
    pub fn set_minimap_ping(&mut self, ping: (f32, f32)) {
        self.model_mut().minimap_ping = ping;
    }

    /// Drain a `Minimap:PingLocation(x, y)` call — centre-relative offsets in **UI units**
    /// (x right, y up). The app converts: UI units × the seam scale = window px, ÷ the live
    /// `px_per_yd` = yards, and the click resolves against the geometry drawn in the same frame.
    pub fn take_minimap_ping_request(&mut self) -> Option<(f32, f32)> {
        self.model_mut().minimap_ping_request.take()
    }

    /// The mask art the Minimap widget asks its renderer for — `Minimap:SetMaskTexture`'s value,
    /// or `None` while no widget has set one (the app then keeps
    /// [`crate::widget::MINIMAP_DEFAULT_MASK`]).
    ///
    /// The **first** Minimap in the registry answers, not every one: the mask is a property of the
    /// map the app draws, and the app draws one. (Everything else in `MinimapState` is per widget
    /// because the Lua API reads it back per widget; this has no getter in 1.12 to read back.)
    pub fn minimap_mask_texture(&self) -> Option<String> {
        let model = self.model_ref();
        model.arena.minimap_kinds().iter().find_map(|&h| {
            match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Minimap(m)) => m.mask_texture.clone(),
                _ => None,
            }
        })
    }

    /// Monotonic count of Minimap widgets ever created in this VM — the O(1) signal that a new
    /// one exists and needs the containment state pushed.
    pub fn minimap_widgets_created(&self) -> u64 {
        self.model_ref().arena.minimap_created()
    }

    /// Seed every Minimap widget's two zoom indices from the persisted levels — the client's
    /// minimap reset path copying each CVar object's parsed int into its live index
    /// (`[0x86f698] ← [[0xb4b410]+0x28]`, `[0x86f69c] ← [[0xb4d90c]+0x28]`). Called **once**, when
    /// the in-game UI materializes and
    /// the widget exists — not per frame: from then on the widget's index is the live truth and
    /// `Minimap:SetZoom` keeps the CVar following it, so a repeated push would fight the +/- buttons.
    /// Both indices clamp into `[0, MINIMAP_ZOOM_LEVELS)` exactly like `set_zoom`, so a hand-edited
    /// `config.toml` cannot seed an out-of-range level.
    pub fn set_minimap_zoom(&mut self, zoom: u8, inside_zoom: u8) {
        let top = crate::widget::MINIMAP_ZOOM_LEVELS - 1;
        let (zoom, inside_zoom) = (zoom.min(top), inside_zoom.min(top));
        self.for_each_minimap(|m| {
            m.zoom = zoom;
            m.inside_zoom = inside_zoom;
        });
    }

    /// Load and run a Lua chunk (text-only; the sandbox rejects bytecode). Errors propagate to the
    /// caller (a *load-time* error is the caller's to see; *handler* errors during events go to
    /// [`UiScript::errors`]).
    pub fn run(&self, chunk: &str) -> mlua::Result<()> {
        self.run_chunk(chunk.as_bytes())
    }

    /// [`UiScript::run`] over a chunk that came off disk, which is **bytes**.
    ///
    /// The reference slurps the file (`0x704bc0`) and hands the buffer to `luaL_loadbuffer`
    /// (`0x6f5690`) with no conversion, and Lua 5.0 strings are byte strings — so a cp1252 locale
    /// file runs there and its literals carry the raw bytes. Reading such a file as `String` is
    /// what made 76 of a real corpus's `.lua` files read as *absent* rather than as text with an
    /// odd glyph. The two front-door transforms the reference's own compiler applies — the UTF-8
    /// BOM strip and the `#`-line skip — are applied here ([`crate::source::chunk`]).
    pub fn run_chunk(&self, chunk: &[u8]) -> mlua::Result<()> {
        self.run_chunk_named(chunk, "(chunk)")
    }

    /// Run a chunk under the name the CLIENT would give it — `Interface\AddOns\<Folder>\<File>`
    /// for an addon file (build it with [`addon_chunk_name`]).
    ///
    /// The name is not cosmetic. Without `set_name`, mlua defaults a chunk to the **Rust** caller
    /// location, so every error an addon raised was reported against `crates/benilla-ui/src/...`
    /// — wrong in any error an addon shows a player, and load-bearing for the addons that PARSE a
    /// traceback. `FuBarPlugin-2.0.lua:752` derives a plugin's own folder with
    /// `string.find(debugstack(6, 1, 0), "\\AddOns\\(.*)\\")`, which cannot match a Rust path.
    pub fn run_chunk_named(&self, chunk: &[u8], name: &str) -> mlua::Result<()> {
        self.lua
            .load(crate::source::chunk(chunk))
            .set_name(name)
            .set_mode(mlua::ChunkMode::Text)
            .exec()
    }

    /// Load and evaluate a Lua chunk, returning its result. Primarily for tests / one-shot queries.
    pub fn eval<T: mlua::FromLuaMulti>(&self, chunk: &str) -> mlua::Result<T> {
        self.lua.load(chunk).set_mode(mlua::ChunkMode::Text).eval()
    }

    /// **How many values does `expr` return?** — the return-shape question, asked at the host
    /// boundary instead of inside Lua.
    ///
    /// This is what `select('#', expr)` used to answer in ~170 of our own tests. `select` is not a
    /// 1.12 global and is gone (`lua50::install`), and the replacement is not a workaround: a
    /// binding's arity is a fact about the **binding ABI**, and on this side of it a multiple
    /// return already *is* a `MultiValue` whose length nothing can round off. It keeps the property
    /// the arity gates rest on — **zero values and one `nil` are different answers**, `0` and `1`
    /// (`binding_abi`'s §2), which is exactly what a `Option<T>` return type cannot tell apart.
    ///
    /// `expr` is a Lua **expression**, not a chunk: pass `"GetItemInfo(1)"`, not `"return …"`.
    /// A raise propagates — a call that errors has no arity, and swallowing that into `0` is how a
    /// broken binding scores as a zero-return verb.
    ///
    /// The one place this cannot serve is a probe that has to hold the count *inside* Lua (a
    /// `pcall` loop over generated calls, as `shape_gate` runs). There the 5.0 spelling is 1.12's
    /// own and reads the same: `(function(...) return arg.n end)(expr)`.
    pub fn arity(&self, expr: &str) -> mlua::Result<usize> {
        let values: mlua::Variadic<mlua::Value> = self.eval(&format!("return {expr}"))?;
        Ok(values.len())
    }

    /// The owning frame's name for an [`ExtractedQuad`] target — a debugging affordance for
    /// capture/probe tooling, which sees quads but not widgets ("whose quad is this?").
    pub fn quad_owner_name(&self, target: ZTarget) -> Option<String> {
        let model = self.model_ref();
        let fh = match target {
            ZTarget::Frame(fh) => fh,
            ZTarget::Region(rh) => model.arena.region(rh)?.owner,
        };
        model.arena.frame(fh)?.name.clone()
    }

    /// Append a line to a named [`FrameKind::ScrollingMessageFrame`] (the app's chat feed → the seam,
    /// the analogue of the loot/merchant snapshot pushes). `r`/`g`/`b` are `0..1`; the engine
    /// byte-quantizes them round-half-up and drives the line's alpha via the fade. A missing frame,
    /// or one of another kind, is a no-op (returns `false`) — the app decides whether that's a bug.
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

    /// Open the named EditBox for typing: show it and grab keyboard focus (so [`has_keyboard_focus`]
    /// gates the world's keys). The app's chat-open key (ENTER) drives this; the box's own
    /// `OnEnterPressed`/`OnEscapePressed` handlers hide + `ClearFocus` it on submit/cancel. A missing
    /// frame, or one that isn't an EditBox, is a no-op (`false`).
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

    // The EditBox text-UI seam (advance table, caret/selection geometry, clipboard) lives in
    // `editbox/seam.rs` — an `impl UiScript` block beside its concern, the layout.rs pattern.

    /// Drain the chat lines submitted through the input EditBox since the last call (the
    /// `SubmitChatInput` Lua binding queued them). The app parses each into an outbound chat command.
    pub fn take_chat_input(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().chat_input)
    }

    /// Queue a line as if it had been typed into the chat EditBox and submitted — the headless
    /// twin of `SubmitChatInput`, for scripted probes (`WOW_PROBE_CHAT`). Going through this seam
    /// rather than straight to the wire is what lets a probe drive **client-side** slash commands
    /// (`/duel`, `/reaction`); a plain line or a `.gm`-style server command still reaches the wire
    /// exactly as before, because that is what the chat drain does with anything it doesn't own.
    pub fn push_chat_input(&mut self, line: String) {
        self.model_mut().chat_input.push(line);
    }

    /// Whether Tab was pressed in the chat edit box since the last call (`BenillaChatTabPressed`)
    /// — the whisper-target cycle's trigger (decision 0288 P5).
    pub fn take_chat_tab(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().chat_tab)
    }

    /// Replace the frame-keyed hyperlink spans (`(frame, rect, link, markup)`, rects in the
    /// engine's y-up screen space) — the app feeds these each frame after rasterizing message
    /// lines (it alone knows where the glyphs actually landed). A release inside a span fires
    /// `OnHyperlinkClick(link, markup, button)` on the owning frame ([`pointer`]).
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

    /// Invoke a compiled handler `func` under the `0x704d50` frame-globals convention
    /// (`this`/`self` = `wrapper`), the same set/restore path registry handlers use. For the
    /// [`crate::loader`], which holds the `OnLoad` `Function` directly (to fire it bottom-up)
    /// rather than through the registry: this keeps the convention in one home instead of
    /// duplicating it. Errors are returned so the caller routes them (the loader records them in
    /// its own report).
    /// Whether the frame with this global name is effectively visible — shown, with every ancestor
    /// shown (`IsVisible()`'s answer, read host-side). `false` for a name no live frame carries.
    /// The read side of a window for a host that keeps state per window: the dressing-room feed
    /// empties its booth when `DressUpFrame` hides, and the stock file has no hook of ours in its
    /// OnHide to say so (1969). A linear scan, like [`Self::model_pane`], for the same reason.
    pub fn frame_visible(&self, name: &str) -> bool {
        self.model_ref()
            .arena
            .iter_frames()
            .any(|(_, f)| f.name.as_deref() == Some(name) && f.effective_visible)
    }

    /// The effective alpha of the frame with this global name while it is effectively visible,
    /// `None` when it is hidden or no live frame carries the name. The read side of a frame whose
    /// pixels a host draws for the engine: the stock `MiniMapPing` is a `<Model>` this engine
    /// renders nothing for, so the app's ping sprite follows the frame's own show/hide and alpha
    /// — the lifetime the reference's `Minimap.lua` owns — instead of keeping a clock of its own
    /// (1974). A linear scan, like [`Self::frame_visible`].
    pub fn frame_effective_alpha(&self, name: &str) -> Option<f32> {
        self.model_ref()
            .arena
            .iter_frames()
            .find(|(_, f)| f.name.as_deref() == Some(name) && f.effective_visible)
            .map(|(_, f)| f.effective_alpha)
    }

    /// Resolve every frame's rect: sync each frame's effective scale from the arena into its layout
    /// input, run the [`crate::layout`] graph (screen root as the external base), and cache the
    /// resolved rects. `GetWidth`/`GetHeight`/`extract` read this cache.
    /// Frames whose size moved fire `OnSizeChanged` here, once the borrow is released
    /// ([`event::fire_size_changes`]) — this is the drain the resolver's queue exists for, and the
    /// one every per-frame host tick runs.
    pub fn resolve(&mut self) {
        {
            let mut model = self.model_mut();
            Self::resolve_layout(&mut model);
        }
        // With a font engine installed ([`Self::set_text_measurer`]) the VM closes the measure
        // round-trip itself, right here — solve, measure what the solve revealed, solve again.
        // That is the same two-solve shape the host's own drive loop already runs, moved inside so
        // a FontString's box is right in the frame its text was set rather than the frame after.
        // Without an engine this is a no-op and the host's batch pass stays the only answer.
        if self.fill_measures() {
            let mut model = self.model_mut();
            Self::resolve_layout(&mut model);
        }
        event::fire_size_changes(&self.lua);
    }

    /// Store host measurements for [`MeasureRequest`]s (`(id, w, h, natural_w, key)` — id/key
    /// verbatim from the request). The next [`UiScript::resolve`] uses `w`/`h` as the FontStrings'
    /// implicit size.
    ///
    /// `w`/`h` are the text **as laid out** (wrapped inside a declared width); `natural_w` is what
    /// it would take **unwrapped**, and is what `GetStringWidth` reports — see [`MeasuredText`] for
    /// why the two must not be conflated. For a region with no declared width they are the same
    /// number, which is why [`Self::set_measured_text_unwrapped`] exists for tests.
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
                // The KEY always lands (or the region re-requests its own measure forever); the
                // EPOCH moves only if the laid-out extent did — see `MeasuredText::layout_moved`.
                moved = MeasuredText::layout_moved(d.measured, new);
                d.measured = Some(new);
            }
            if moved {
                // Measured extents are the auto-size axes' inputs — the layout gate's read set.
                // Touched PER REGION rather than once for the batch: the batch
                // touch could only say "some extent moved", which is exactly the whole-roster
                // question the ledger exists to stop asking.
                model.touch_layout_region(rh);
            }
        }
    }

    /// [`Self::set_measured_text`] for text with **no wrap constraint**, where the laid-out and
    /// natural widths are by definition the same number. Test-facing: a hand-fed measure for an
    /// unconstrained string should not have to say `w` twice, and a test that DOES care about the
    /// distinction (a declared-width region) should be forced to spell it out.
    pub fn set_measured_text_unwrapped(&mut self, measures: &[(u32, f32, f32, u64)]) {
        let widened: Vec<_> = measures
            .iter()
            .map(|&(id, w, h, key)| (id, w, h, w, key))
            .collect();
        self.set_measured_text(&widened);
    }

    /// Drop every cached host text metric — FontString measures, message-line row counts, editbox
    /// advance tables. The host calls this when its **raster environment** changes (a window
    /// resize / fullscreen toggle / uiScale move — anything that shifts the px-per-unit seam its
    /// answers were taken under): glyph advances step to whole pixels at the drawn raster size,
    /// so a metric measured under one environment does not rescale to another — the font-size
    /// snap alone moves a string's unit width by several percent, which is exactly enough for a
    /// boot-size measure to fail the post-resize ellipsis fit test and truncate text that fits
    /// (the director's fullscreen "Contr..." rows). The environment is the host's to watch — the
    /// engine deliberately never sees the seam scale — so staleness is the host's to declare; the
    /// round-trips re-answer on the frames that follow (the same one-frame convergence every
    /// measure already has).
    /// Force the next [`UiScript::resolve`] to rebuild the **whole** layout graph rather than the
    /// dirty closure a scoped resolve would — and to run at all, rather than stop
    /// at either change gate.
    ///
    /// This is the scoped resolve's own falsifier, and it exists so the claim the scope rests on —
    /// *a node whose own inputs and whose dependencies did not move recomputes to the rect it
    /// already holds* — is something a test can **disprove** rather than something the engine
    /// argues. Drive a change, resolve, read the rects; call this, resolve again, read them again;
    /// they must be identical. `WOW_LAYOUT_VERIFY` makes the same comparison automatically on every
    /// SETTLED frame; this is for the frames that never settle, which is exactly the hover sweep
    /// the scope was built for.
    pub fn force_full_layout_resolve(&mut self) {
        let mut model = self.model_mut();
        model.layout_scope.invalidate();
        model.layout_fingerprint = None;
        model.layout_epoch_resolved = None;
        model.touch_layout();
    }

    pub fn invalidate_text_measures(&mut self) {
        let mut model = self.model_mut();
        for d in model.region_data.values_mut() {
            d.measured = None;
        }
        // The frame-side caches key on content hashes; a bumped stored key can never equal the
        // recomputed one, so the next needing-measure sweep re-requests without this method
        // having to know what the keys hash.
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
        // Measured extents are auto-size inputs — the layout gate's read set, same as
        // [`Self::set_measured_text`]'s.
        model.touch_layout();
        // Every stored measure was just wiped; only the whole-roster sweep can re-request them.
        model.touch_measure_all();
    }

    // ── Input: pointer-leaves-window cleanup (the hit-test/mouse dispatch that
    // used to sit here now lives in [`pointer`]) ─────────────────────────────────────────────────

    /// The OS pointer left the window (decision 0216 §3's drag-gesture leak): clears
    /// [`Model::drag`] AND [`Model::mouse_down_on`] — the two press-tracked states nothing else
    /// clears when the pointer leaves the window mid-press, since the matching release is never
    /// fed. Left uncleared, a stale armed [`Model::drag`] would fire a spurious `OnDragStart` the
    /// instant the pointer re-enters and crosses the threshold against a press point that no
    /// longer means anything; a stale [`Model::mouse_down_on`] entry would fire a same-frame
    /// `OnClick` if the pointer re-enters and releases over the very frame it left from. The app
    /// calls this from the same branch that fires the synthetic `OnLeave` (`ui_script/input.rs`).
    pub fn pointer_left_window(&mut self) {
        // **A started drag ENDS here, it is not merely forgotten.** The reference cannot reach
        // this state at all — the OS holds the pointer for the whole of a button-held drag, so its
        // release always arrives and `OnDragStop` always runs. Ours has no such capture: walk the
        // cursor off the window edge mid-drag and no release is ever fed. Dropping the gesture
        // silently (what this did) leaves the addon's `OnDragStop → StopMovingOrSizing` unrun, so
        // the engine's single `Model::moving` slot stays taken and `object::movable::advance_move`
        // glues that frame to the cursor for the rest of the session — where it also swallows
        // every press aimed at whatever is underneath. That is B310: one raid row dragged off the
        // window edge, and no row could be dragged again. Firing the stop is the faithful end,
        // because it is the same end the reference's own release gives the handler.
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
        // Every button that capture was holding down goes back to NORMAL — the release edge the
        // OS never fed us (`0x7793de`), without which a button walked off the window edge
        // mid-press keeps its pushed art for the rest of the session.
        for h in held {
            button::edge(&mut model, h, crate::widget::ButtonState::on_mouse_up);
        }
        // …and its one-slot twin `root+0x80`, which the mouse-down raise reads: a capture left
        // behind would aim the next press's raise at whatever the pointer was last holding.
        model.mouse_capture = None;
        // [`Model::last_click`] is deliberately NOT cleared here. It looks like it belongs in this
        // list, and the binary says otherwise: `[CButton+0x334]` has exactly three writers
        // image-wide (the ctor, the fired-double zero, the fired-single stamp) and none of them is
        // a hide, a disable, or a mouse-leave — so a half-finished double click really does survive
        // the cursor leaving the window and coming back inside the 300 ms.
        // A thumb drag in progress when the pointer leaves is abandoned too —
        // the release that would end it is never fed, same leak as the drag gesture above.
        model.slider_drag = None;
    }

    // ── Keyboard entry (the EditBox focus + key/char routing law below) ──────────────────────────
    //
    // benilla speaks *key names*, not scancodes — the host maps its window keycodes to these. The
    // routing is the client's exactly: if a box is focused it processes and CONSUMES every event; if
    // none is focused, the topmost effectively-visible `autoFocus` box self-acquires focus and
    // processes this same event; otherwise nothing is consumed. Today the ways a box gets focus here
    // are this self-acquire path, a click, and Lua `SetFocus` — the reference has a fourth, an
    // `autoFocus` box focusing itself on SHOW, which we do not implement yet (the flag's own doc
    // carries the correction and what it waits on).

    /// A typed character (may be multi-byte UTF-8) arriving from the host. Routes per that law and,
    /// on a focused box, inserts it (numeric/cap/password rules apply) or — for the Ctrl+A control
    /// code — selects all. Returns `true` if consumed (a focused box consumes every char).
    pub fn char_input(&mut self, text: &str) -> bool {
        // The frame walk first ([`keyboard`]): the focused box is a PARTICIPANT in it, at its own
        // strata/level, so this is not "frames before boxes" — it is the reference's one dispatcher
        // in the reference's order. An event no frame consumed still falls through to the box
        // routing, which owns focus acquisition (`autoFocus` self-acquire and kin).
        keyboard::char_input(&self.lua, text) || editbox::char_input(&self.lua, text)
    }

    /// Paste text from the host OS clipboard into the focused EditBox. The engine-free runtime can't
    /// reach the clipboard itself, so the app reads it (Cmd+V on macOS / Ctrl+V elsewhere) and hands
    /// the string here; newlines survive only in a `multiLine` box, other control chars are dropped,
    /// and the remainder inserts as one edit. Returns `true` if a box consumed it.
    pub fn paste(&mut self, text: &str) -> bool {
        editbox::paste(&self.lua, text)
    }

    /// A non-character key press arriving from the host, by name — the three *box-event* keys
    /// (`"ENTER"`, `"ESCAPE"`, `"TAB"`) fire their FrameXML scripts; editing keys arrive as
    /// semantic [`EditAction`]s via [`Self::editbox_action`] instead (the host's per-OS keymap
    /// owns which chord means what). Routes per that law; a focused box consumes the key even when
    /// it does nothing with it. Returns `true` if consumed.
    pub fn key_input(&mut self, key: &str) -> bool {
        // Same two-stage shape as `char_input` — see its note.
        keyboard::key_input(&self.lua, key) || editbox::key_input(&self.lua, key)
    }

    /// A key the host delivers to a focused EditBox as an [`EditAction`] chord rather than by name
    /// (BACKSPACE, DELETE, the arrows, HOME, END), offered to the **keyboard frames** first.
    ///
    /// Returns `true` if a frame consumed it, in which case the caller must NOT also dispatch the
    /// chord — and the key's binding must not fire either (consumption suppresses it, `0x76b7d0`).
    /// A `false` means either nothing wanted it or the focused box owns it; the caller proceeds
    /// exactly as it did before this entry point existed. See [`keyboard::frame_key_input`] for
    /// why declining at the box is the faithful answer rather than skipping it.
    pub fn frame_key_input(&mut self, key: &str) -> bool {
        keyboard::frame_key_input(&self.lua, key)
    }

    /// One semantic text-editing operation on the focused EditBox — the output of the host's
    /// per-OS keymap. Same routing/consumption law as [`Self::key_input`].
    /// Returns `true` if consumed.
    pub fn editbox_action(&mut self, action: EditAction) -> bool {
        editbox::action(&self.lua, action)
    }

    /// Is the focused EditBox in **alt-arrow mode** — the flag the XML spells `ignoreArrows` and
    /// the Lua surface spells `SetAltArrowKeyMode` (`[editbox+0x318] & 0x10`)?
    ///
    /// The host asks this to decide whether an arrow key reaches the box at all. With the flag set
    /// and ALT not held, the reference's key handler returns 0 at `0x77b1c4` for the four arrow
    /// codes — **not consumed** — and the strata walk carries down to `CGWorldFrame`, which runs
    /// the key's binding. That is what lets you turn while the chat box has focus, and it is why
    /// the gate is on the key rather than on the [`EditAction`]: `Move { unit: Edge }` reaches the
    /// box from HOME/END as well as from an arrow, and only the arrow is gated.
    ///
    /// `false` when nothing is focused, which is the same answer as an unflagged box: neither
    /// declines the key.
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

    /// Whether an EditBox currently holds keyboard focus (and is effectively visible) — the app gates
    /// world/player key input on this, matching the client's `DAT_00cf4dc8 != 0` test.
    pub fn has_keyboard_focus(&self) -> bool {
        let model = self.model_ref();
        model
            .focused_editbox
            .is_some_and(|h| model.arena.frame(h).is_some_and(|f| f.effective_visible))
    }

    /// **Which** EditBox holds the focus, by name — [`Self::has_keyboard_focus`] answers
    /// *whether*. Unfiltered by visibility, because the focus cell itself is: a box that hides
    /// while focused keeps the cell until something clears it, and the tests that watch the
    /// hand-off between two boxes are watching exactly that cell.
    ///
    /// This is the host-side read of `Model::focused_editbox`. There is no Lua verb for it and
    /// there must not be: 1.12's EditBox table has `SetFocus`/`ClearFocus` and no getter, which
    /// is why `pfQuest/browser.lua:760` ships its own focus flag rather than asking (decision
    /// 2142).
    pub fn focused_editbox_name(&self) -> Option<String> {
        let model = self.model_ref();
        let h = model.focused_editbox?;
        model.arena.frame(h)?.name.clone()
    }

    /// How many resolves the layout change gate has let through (`Model::layout_solves`) — the
    /// gate's effectiveness, readable by tests and the app's cost meters (a per-frame delta of 0
    /// means the fingerprint judged the frame quiet).
    pub fn layout_solves(&self) -> u64 {
        self.model_ref().layout_solves
    }

    /// How many resolves got past **tier 1** and paid the whole-roster preamble
    /// ([`Model::layout_gate_walks`]) — the gate's true cost counter, ≥
    /// [`Self::layout_solves`] because a walk that concludes "nothing moved" pays the same
    /// preamble and never reaches the solve counter.
    pub fn layout_gate_walks(&self) -> u64 {
        self.model_ref().layout_gate_walks
    }

    /// How many times a resolve DERIVED the layout graph from scratch ([`Model::layout_derives`])
    /// — the whole-roster walk, and since decision 1388 the only expensive thing a resolve can do.
    /// A UI that merely animates should hold this flat.
    pub fn layout_derivations(&self) -> u64 {
        self.model_ref().layout_derives
    }

    /// Total fixpoint ROUNDS across every solve ([`Model::layout_rounds`]) — a solve costs
    /// rounds × the whole graph, so the ratio against [`Self::layout_solves`] is the per-pass
    /// depth.
    pub fn layout_rounds(&self) -> u64 {
        self.model_ref().layout_rounds
    }

    /// The last solve's SCOPE — `(frames solved, regions swept)`, decision 1350's meter.
    ///
    /// The third axis of a solve's cost, and the one that used to be "all of it": solves says how
    /// OFTEN, rounds says how DEEP, this says how WIDE. A change that touches ten FontStrings must
    /// read a handful here however large the UI grows; a scope that tracks the graph is the
    /// regression, and it is asserted as a COUNT because milliseconds have twice failed to catch
    /// this class.
    pub fn layout_last_scope(&self) -> (usize, usize) {
        self.model_ref().layout_last_scope
    }

    /// Is `name` a registered FrameXML template — one `CreateFrame`'s fourth argument or an
    /// `inherits=` can resolve?
    ///
    /// A pure query on the VM's live registry, for the corpus harness: an addon naming a template
    /// we have not transcribed gets a bare frame and **no load error**, so nothing else can see it.
    /// **Folded**, because the resolution it reports on is folded. An exact `contains_key` made this
    /// census disagree with the loader the moment `inherits=` became case-insensitive: it went on
    /// listing `UIDropdownMenuTemplate`, `CT_RaCheckButtonTemplate` and `MSBTColorSwatchTemplate` as
    /// missing while the loader was resolving all three. An instrument that reports a gap the code
    /// does not have is worse than no instrument — it is a build queue pointing at finished work
    /// (1242/1246, and 1251 §3's rule that a source-derived answer be checked against the runtime
    /// artefact).
    pub fn has_framexml_template(&self, name: &str) -> bool {
        let model = self.model_ref();
        let templates = model.framexml_templates.borrow();
        templates.contains_key(name) || templates.keys().any(|k| k.eq_ignore_ascii_case(name))
    }

    /// Is `name` a registered FONT object — the *other* thing an `inherits=` may legally name?
    ///
    /// [`Self::has_framexml_template`]'s twin, and only useful beside it: `inherits=` is one
    /// attribute over two namespaces (`<FontString inherits="GameFontNormal">` names a font,
    /// `<Button inherits="UIPanelButtonTemplate">` names a template — `loader::expand_region` is
    /// where that fork lives). A census that asks only the template registry reports every font in
    /// the corpus as a missing template.
    pub fn has_font_object(&self, name: &str) -> bool {
        self.model_ref().font_object(name).is_some()
    }

    /// The **widget kind of a published name** — `"MessageFrame"`, `"Button"`, `"Texture"`, … — or
    /// `None` if nothing by that name is a live widget.
    ///
    /// [`Self::has_framexml_template`]'s sibling, and the same kind of thing: a pure Rust-side query
    /// on the live model, for the corpus harness. The harness needs to know what `UIErrorsFrame` in
    /// `UIErrorsFrame:AddMessage(…)` actually *is* in our object graph, because a widget-method
    /// census that asks "does ANY kind answer this name" cannot see a verb wired to one class and
    /// forgotten on its sibling. Attributing the call site to a kind is what makes
    /// that question askable, and this is the only honest answer to "what kind is that global": the
    /// arena's own record, not a name list.
    ///
    /// **Deliberately NOT a Lua binding.** The reference publishes this as `GetObjectType`, which we
    /// do not implement outside font objects; adding it here would put a new verb in front of every
    /// addon and move the very numbers the census is measured by. An instrument must be able to
    /// claim it perturbed nothing, so it stays on the Rust side where no addon can observe it.
    ///
    /// The spellings are `CreateFrame`'s own (`SimpleHTML`, not `SimpleHtml`), so a caller can
    /// compare a name here against a kind it passed to `CreateFrame`.
    pub fn widget_kind(&self, name: &str) -> Option<&'static str> {
        let model = self.model_ref();
        if let Some(h) = model.arena.lookup(name) {
            return model.arena.frame(h).map(|f| match f.kind {
                crate::widget::FrameKind::Frame => "Frame",
                // A `Frame` to Lua (1984): the registered name never becomes a class identity.
                crate::widget::FrameKind::WorldFrame => "Frame",
                crate::widget::FrameKind::Button => "Button",
                crate::widget::FrameKind::CheckButton => "CheckButton",
                // `GetObjectType 0x495b60` is two instructions and returns `"LootButton"`
                // (`[0x847ce0]` → `0x843414`). It does NOT answer "Button".
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
        // The region leaves publish into their own name table (`region_names`), not the arena's —
        // and they matter here: `GameTooltipTextLeft1:GetText()` is a FontString call, and the
        // corpus scrapes those constantly.
        let id = *model.region_names.get(name)?;
        let h = *model.id_to_region.get(&id)?;
        model.arena.region(h).map(|r| match r.kind {
            crate::widget::RegionKind::Texture => "Texture",
            crate::widget::RegionKind::FontString => "FontString",
            // A title region is a plain Region and says so (`0x76c440`) — and it is unreachable by
            // name anyway: `CreateTitleRegion` takes no name argument at all.
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

    /// Drain the accumulated non-fatal host warnings (an ignored `CreateFrame` template, a layout
    /// fixpoint that hit its round cap) — the host logs them; un-drained they pile up unseen.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().warnings)
    }

    /// Report a script error the host caught **outside** the VM's own dispatch — an addon file
    /// that failed to compile or raised at file scope during the load walk. It joins the
    /// handler-dispatch queue and the retained log, but **not** `errors`: the caller has already
    /// logged it (the load walk's per-file `error!` + `failures` contract), so putting it there too
    /// would double-log at the app's per-frame drain.
    ///
    /// Its silent sibling is [`Self::report_load_failure`] — same retention, no
    /// dispatch, for the load failures that never raise at all.
    pub fn report_script_error(&self, msg: &str) {
        // Retained as a **Load** row, not an Error one: every caller of this is
        // the load walk, and from the player's side "the addon's file scope raised" and "the
        // addon's file was missing" are the same fact — the addon is not running. The retention
        // half is `diagnostics::record_load_failure`, shared with the demand-load path so the
        // rule has one implementation (2107); the dispatch is what makes this the loud sibling.
        diagnostics::record_load_failure(&self.lua, msg);
        self.model_mut()
            .pending_error_dispatch
            .push(msg.to_string());
    }

    /// Hand every queued script error to the Lua-side error handler — the reference's own shape:
    /// `seterrorhandler`/`geterrorhandler` are engine globals (`0x702900`/`0x702950`,
    /// the captured `_G`), the engine invokes the registered handler on a caught script error
    /// (that is the pair's contract — a handler nothing invokes would be two dead globals), and
    /// FrameXML answers with `_ERRORMESSAGE` → the red ScriptErrors dialog.
    ///
    /// Called at a safe seam (the app's per-frame drain), never from inside the failed call.
    /// Three guards keep it bounded and honest:
    /// - **The stdlib default handler is skipped by identity.** It reports into
    ///   [`UiScript::errors`] — where every queued message already is — so dispatching it would
    ///   only duplicate. The queue still drains, so a handler installed later starts clean.
    /// - **A handler that raises is recorded on the host channel only** and never re-queued:
    ///   the error path cannot recurse by construction.
    /// - **One failure stops the batch** — a broken handler fails the same way for every message,
    ///   and one line names it.
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

    /// Register a named virtual [`FontObject`] (a resolved `<Font>`), overwriting any prior one of
    /// the same name, **and publish it as the Lua global `name`** — the same pair
    /// `Loader::do_font` performs, so a font registered from Rust is addressable from Lua exactly
    /// like one declared in XML (`fs:SetFontObject(Name)`, `Name:GetFont()`). One registration act,
    /// one outcome: the two paths cannot drift.
    pub fn register_font_object(&self, name: &str, font: FontObject) {
        self.model_mut()
            .font_objects_by_lower
            .insert(name.to_ascii_lowercase(), font);
        // Publishing cannot fail for a fresh table + a string key; a registry hiccup is not worth
        // an unwrap in a host-facing setter, and the record is already in place either way.
        let _ = font::publish(&self.lua, name);
    }

    /// Look up a registered [`FontObject`] by name (the resolved paint), if any. Used by tests and by
    /// the `SetFontObject` binding.
    pub fn font_object(&self, name: &str) -> Option<FontObject> {
        self.model_ref().font_object(name).cloned()
    }

    /// Every registered [`FontObject`] (the resolved paints of all loaded `<Font>` nodes) — the
    /// host's bake census: the glyph atlas reads the distinct `(font, height, outline)` triples
    /// off this to know which outlined cell variants the shipped UI can actually request
    /// (the outlined-glyph bake, the fade-composite fold-back record).
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
