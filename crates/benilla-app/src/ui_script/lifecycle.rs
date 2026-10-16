//! The UI session's lifecycle, all edges and nothing per-frame: `Startup` installs the glue
//! screens' VM; world entry builds a fresh VM and loads the in-game UI and every addon onto it;
//! leaving the world runs the reference's shutdown tail and installs the glue VM again; `ReloadUI()` is the two edges
//! back to back without leaving the world.

use bevy::prelude::*;

use benilla_assets::{LockRecover, LooseOpen};
use benilla_ui::script::{GlueNotice, UiScript};

use super::manifest::silenced_ui_load;
use super::{load_ingame_ui, CursorPayloadHeld, PlayerUiHover, UiClock, UiKeyboardCapture};
use crate::ui_script::addons;

pub(crate) fn setup_script(world: &mut World) {
    install_glue_side_vm(world, super::glue::GlueScreen::Login.key());
}

/// Install the VM the world holds outside the world: the glue screens', put up on the screen
/// named `screen`. A glue whose select screen did not load (logged as an error by [`new_vm`] or
/// the load) asks for no roster, and nothing else does.
pub(super) fn install_glue_side_vm(world: &mut World, screen: &str) {
    if !install_glue_vm(world, screen) {
        error!("ui_script: the glue VM did not come up with its character select screen");
    }
}

/// Build and install a boot VM, the one a `ReloadUI()` holds between its teardown and its load:
/// the strings the feeds resolve lines from and the emote tokens `/`-commands are built from
/// (`ui_chat::commands`), no frames and no font objects.
fn install_boot_vm(world: &mut World) {
    if let Some(script) = strings_vm(world) {
        world.insert_non_send_resource(script);
    }
}

/// A boot VM: `GlobalStrings.lua` and the emote tokens, for a reader that needs them where the
/// world holds the glue VM. `None` (the world's VM removed) when
/// the VM cannot start.
pub(crate) fn strings_vm(world: &mut World) -> Option<UiScript> {
    let script = new_vm(world, UiScript::new)?;
    load_global_strings(world, &script);
    load_emote_tokens(world, &script);
    Some(script)
}

/// Build the glue VM, load the stock GlueXML onto it, install it and put `screen` up: the glue
/// builder `0x46a7b0` then `SET_GLUE_SCREEN` (`0x402b65` at startup, `0x402090` back from the
/// world). It holds what the glue state holds and nothing of a boot VM's: no `GlobalStrings.lua`,
/// no emote tokens.
fn install_glue_vm(world: &mut World, screen: &str) -> bool {
    let Some(mut script) = new_vm(world, UiScript::new_glue) else {
        return false;
    };
    // The build's size and aspect, and its clear of a pending rebuild (`0x46a7e0`, `0x46a81c`).
    super::glue::glue_built(world);
    seat_raster_seam_for_load(world, &mut script);
    // The CVar table before the load and the first screen, as the in-game build seeds its own:
    // this VM is born before `cvars::sync_cvars`'s per-VM seed, and the character screen's
    // `OnShow` names the realm `GetServerName` reads off `realmName` (`0x46d289`).
    if let Some(cvars) = world.get_resource::<crate::cvars::Cvars>() {
        cvars.seed_vm(&mut script);
    }
    seed_agreements(world, &mut script);
    let start = std::time::Instant::now();
    let failures = super::glue::load_glue(&mut script);
    super::glue::finish_glue_load_log(&mut script);
    start_server_alert(world, &script);
    // What the glue carries across its rebuild, in place before the screen's `OnShow` reads it.
    super::glue_mgr::seat_glue_build(world, &mut script);
    // `SetCurrentScreen`'s buffer is the client's (`[0xb41478]`) and outlives the Lua state: a
    // rebuild signals what it holds (`0x46b9a2`), and the screen's handler writes the same name.
    script.set_glue_screen_name(screen);
    super::glue::signal_glue_screen(&mut script, screen);
    info!(
        "ui_script: stock glue up in {:.0} ms on \"{}\" ({} load failure(s))",
        start.elapsed().as_secs_f32() * 1000.0,
        screen,
        failures.len()
    );
    // Up means the screen that asks for the roster exists: the stock files loaded far enough to
    // define the character select frame and its `OnShow`, not merely that a VM started.
    let charselect = script
        .eval::<bool>(
            "return type(CharacterSelect) == 'table' and type(CharacterSelect_OnShow) == 'function'",
        )
        .unwrap_or(false);
    world.insert_non_send_resource(script);
    charselect
}

/// Start the server alert's fetch, as the glue manager's init does after the GlueXML load and
/// before the first screen (`0x46a541`-`0x46a564`, `0x402b36`): the global `SERVER_ALERT_URL` as
/// the glue files left it, so `GlueLocalization` wins over `GlueStrings`, fetched once per
/// process ([`super::server_alert::begin_once`]).
fn start_server_alert(world: &mut World, script: &UiScript) {
    let url = script.global_text("SERVER_ALERT_URL");
    if let Some(fetch) = super::server_alert::begin_once(&url) {
        if let Some(shared) = world.get_non_send_resource::<super::glue_mgr::SharedGlueMgr>() {
            shared.get_mut().set_alert_fetch(fetch);
        }
    }
}

/// Read the four agreement CVars into the glue VM, which holds their flags, before any screen is
/// set, as the reference does (`0x46a400` at `0x402b36`, then `SET_GLUE_SCREEN` at `0x402b65`):
/// the login screen's `OnShow` asks them at once, and nothing asks again.
fn seed_agreements(world: &mut World, script: &mut UiScript) {
    let Some(cvars) = world.get_resource::<crate::cvars::Cvars>() else {
        return;
    };
    let notices = super::glue_mgr::read_agreements(cvars);
    for notice in GlueNotice::ALL {
        script.set_glue_notice(notice, notices[notice as usize]);
    }
}

/// A fresh VM from `make` on the process clock with the addon asset probes wired, or `None` (the
/// world's VM removed) when the VM cannot start.
fn new_vm<E: std::fmt::Display>(
    world: &mut World,
    make: fn() -> Result<UiScript, E>,
) -> Option<UiScript> {
    // The client globals the VM holds that no rebuild resets: `[0xcf4d24]`, written only by
    // `SetEuropeanNumbers` (`0x771c40`), outlives a `ReloadUI` and a logout.
    let european_numbers = world
        .get_non_send_resource::<UiScript>()
        .is_some_and(UiScript::european_numbers);
    let mut script = match make() {
        Ok(s) => s,
        Err(e) => {
            error!("ui_script: VM init failed: {e}");
            world.remove_non_send_resource::<UiScript>();
            return None;
        }
    };
    script.set_european_numbers(european_numbers);
    // `[0xc0e080]`, which start-up sets once from `Wow.ini`, so every VM answers `GetLocale` alike.
    script.set_locale(super::client_locale().index());
    // `[0x882730]`, `Wow.ini`'s `Region`, read once alike and handed to the name check.
    script.set_client_region(super::client_region());
    seed_vm_clock(world, &mut script);
    install_addon_asset_resolvers(world, &mut script);
    info!("ui_script: new VM, session {}", script.session());
    Some(script)
}

/// Start the new VM's `GetTime()` where the process already is and re-anchor [`UiClock`] to it,
/// both off the `Time<Real>` it anchors on. The reference's `GetTime` is `GetTickCount` × 0.001
/// (`0x515ea0`, via `0x42c010` → `0x42b790`), an OS clock that never restarts; ours lives in the
/// VM, rebuilt at every login and `ReloadUI`, and a clock back at zero puts every converted
/// cooldown start in the past, which stock `CooldownFrame_SetTimer` hides (`Cooldown.lua:3`).
fn seed_vm_clock(world: &mut World, script: &mut UiScript) {
    let (anchor, elapsed) = world
        .get_resource::<Time<bevy::time::Real>>()
        .map_or_else(Default::default, |t| {
            (t.last_update(), t.elapsed_secs_f64())
        });
    script.set_now(elapsed);
    if let Some(mut clock) = world.get_resource_mut::<UiClock>() {
        *clock = UiClock {
            anchor: anchor.unwrap_or_else(std::time::Instant::now),
            ui_now: elapsed,
        };
    }
}

/// Wire the loose files into art and fonts: the decoder's AddOns folder, beside the install's
/// index it already reads, and the VM's texture, size and font probes, so the path forms of
/// `SetTexture` and `SetFont` answer the reference's 1|nil load verdict, and a region with no
/// authored size on an axis takes it from its art as the client's virtual size getters do. Each
/// probe resolves through the store and the open its renderer reads (an interface texture's,
/// [`LooseOpen::Index`], and a font face's, [`LooseOpen::BareNameFirst`]), so the answer is what
/// draws; they test existence, not decode.
fn install_addon_asset_resolvers(world: &mut World, script: &mut UiScript) {
    let Some(mut assets) = world.get_resource_mut::<benilla_assets::WorldAssets>() else {
        return;
    };
    assets.set_loose_addon_root(addons::root());
    let loose = assets.loose().clone();
    let chain = assets.chain.clone();
    let size_chain = chain.clone();
    let size_loose = loose.clone();
    let font_chain = chain.clone();
    let font_loose = loose.clone();
    script.set_texture_probe(Box::new(move |path| {
        benilla_assets::sprite_found(&chain, &loose, path)
    }));
    // Keyed by the path exactly as the region carries it, so a hit allocates nothing (this is
    // asked per zero-size textured region per layout pass); misses are cached too.
    let sizes: std::cell::RefCell<std::collections::HashMap<String, Option<(u32, u32)>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    script.set_texture_size_probe(Box::new(move |path| {
        if let Some(cached) = sizes.borrow().get(path) {
            return *cached;
        }
        let measured = benilla_assets::sprite_dimensions(&size_chain, &size_loose, path);
        sizes.borrow_mut().insert(path.to_string(), measured);
        measured
    }));
    // Existence over the same stores, in the same order, as `ui_text::engine`'s face loader,
    // memoised because a path's answer cannot change and an addon may call `SetFont` every frame.
    let seen: std::cell::RefCell<std::collections::HashMap<String, bool>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    script.set_font_probe(Box::new(move |path| {
        if let Some(&cached) = seen.borrow().get(path) {
            return cached;
        }
        let found = font_loose.find(path, LooseOpen::BareNameFirst).is_some()
            || font_chain.lock_recover().contains(path);
        seen.borrow_mut().insert(path.to_string(), found);
        found
    }));
}

/// Whether this run wants the player UI: always outside a capture, and in one only when opted in
/// ([`crate::run_mode::capture_ui_opted_in`]), since capture baselines test the world render.
fn ui_wanted(world: &World) -> bool {
    !world.contains_resource::<crate::run_mode::CaptureMode>()
        || crate::run_mode::capture_ui_opted_in()
}

/// The world-entry UI load still owed: armed at `OnEnter(InWorld)`, run by
/// [`run_pending_entry_load`] once the loading cover is on screen. `OnEnter(InWorld)` runs on the
/// frame that first presents the cover, so a load there would freeze the character screen for its
/// whole duration. The wait is [`crate::loading_screen::EntryCover`]'s, and the loading
/// screen does not clear while this exists, so the reveal never precedes the UI.
#[derive(Resource, Default)]
pub(crate) struct PendingEntryUiLoad;

/// The glue VM, parked between the world-entry edge and the deferred load, out of every feed's
/// reach: each feed takes the VM as an `Option` and returns on `None`.
pub(crate) struct ParkedGlueVm(UiScript);

/// `OnEnter(InWorld)`: arm the deferred entry load and park the glue VM.
pub(crate) fn arm_entry_ui_load(world: &mut World) {
    world.insert_resource(PendingEntryUiLoad);
    if let Some(vm) = world.remove_non_send_resource::<UiScript>() {
        world.insert_non_send_resource(ParkedGlueVm(vm));
    }
}

/// Retire the glue VM and build the one the entry load runs on: the reference re-makes its Lua
/// state inside `UI_Init` (`0x48fe97`, in `0x48fbf0`), so no login shares a session id (and so a
/// [`super::VmMemo`]) with the character screen before it. The glue VM's CVars fold first, since
/// the character screen can write one (the AddOns panel's out-of-date toggle) and this is the last
/// moment its table exists. Everything else is re-seeded onto the new VM or is a
/// [`super::VmMemo`], which is meant to reset here. It starts bare: `FrameXML.toc` runs
/// `GlobalStrings.lua` and `Fonts.xml` as its first rows, and `ChatFrame.lua` the emote tokens.
fn mint_entry_vm(world: &mut World) {
    crate::cvars::fold_dying_vm_cvars(world);
    if let Some(script) = new_vm(world, UiScript::new) {
        world.insert_non_send_resource(script);
    }
}

/// Put the parked glue VM back into the world, if one is parked.
fn unpark_glue_vm(world: &mut World) {
    if let Some(ParkedGlueVm(vm)) = world.remove_non_send_resource::<ParkedGlueVm>() {
        world.insert_non_send_resource(vm);
    }
}

/// Run condition for every in-world feed: the in-game UI is up on the VM in the world, since a
/// feed that fires or drains into a VM with no frames loses what it sends. The reference likewise
/// processes no world state before its UI is up: world entry (`0x420d63`) runs after the same
/// iteration's inbound drain (`0x420d55`, its container `0x420c00` entered once per process),
/// socket threads only enqueue (`0x537b7c`; `SMSG_PONG` at `0x537b56` the one bypass), and the
/// login request waits for the UI (`0x46c272`).
///
/// Two terms, because the latch trails the wire by a frame: `apply_net_updates` drains `Connected`
/// and the login burst behind it in one pass, and the `InWorld` transition that arms the latch and
/// parks the VM runs at the next frame's `StateTransition`; the `InWorld` term covers that frame.
pub(crate) fn ingame_ui_up(
    pending: Option<Res<PendingEntryUiLoad>>,
    state: Option<Res<State<crate::client_state::ClientState>>>,
) -> bool {
    pending.is_none()
        && state.is_some_and(|s| *s.get() == crate::client_state::ClientState::InWorld)
}

/// `PreUpdate`, after [`run_pending_reload`] so a reload never interleaves an armed entry load:
/// run the armed entry load once the cover has presented, at once when there is no cover (a
/// capture booting straight `InWorld`). Leaving the world first drops the latch unrun, and
/// [`end_ui_session`] then skips the shutdown writes, so a UI-less VM never overwrites the saved
/// files.
pub(crate) fn run_pending_entry_load(world: &mut World) {
    if world.get_resource::<PendingEntryUiLoad>().is_none() {
        return;
    }
    let in_world = *world
        .resource::<State<crate::client_state::ClientState>>()
        .get()
        == crate::client_state::ClientState::InWorld;
    if !in_world {
        // Left the world before the load ran: nothing to build a UI for.
        world.remove_resource::<PendingEntryUiLoad>();
        unpark_glue_vm(world);
        return;
    }
    let covering = world
        .get_resource::<crate::loading_screen::LoadingScreen>()
        .is_some_and(|s| s.covering());
    // `presented()` is also true with no cover up: nothing to protect, so load now.
    if !world
        .get_resource::<crate::loading_screen::EntryCover>()
        .is_none_or(|c| c.presented())
    {
        return;
    }
    world.remove_resource::<PendingEntryUiLoad>();
    let start = std::time::Instant::now();
    load_ingame_ui_on_world_entry(world);
    // The world entry's own step after the UI load, which a `ReloadUI()` never takes.
    crate::vplates::clear_at_world_entry(world);
    // Every entry logs the load's cost, whether the cover hid it, and the session every
    // `VmMemo` keys on.
    info!(
        "ui_script: in-game UI up in {:.0} ms (behind the cover: {}, vm session: {})",
        start.elapsed().as_secs_f32() * 1000.0,
        covering,
        world
            .get_non_send_resource::<UiScript>()
            .map_or(0, UiScript::session),
    );
}

/// Seat the VM's screen size and text measurer for the load, under the current window: the
/// load-edge half of [`super::extract::tick_script`]'s per-frame pair. A fresh VM answers 1024×768
/// until then, and a value computed once at load keeps it: stock `WorldMapFrame_OnLoad` sizes the
/// full-screen `BlackoutWorld` from `GetScreenWidth()` and `GetScreenHeight()` and never again
/// (`WorldMapFrame.lua:19-28`), which answer the root while `UIParent` is still at scale 1. With no
/// atlas yet (it bakes on the first `Update`) the per-frame pass seats the measurer.
fn seat_raster_seam_for_load(world: &mut World, script: &mut UiScript) {
    let (w, h) = {
        let mut q = world.query_filtered::<&Window, With<bevy::window::PrimaryWindow>>();
        q.single(world)
            .map_or((0.0, 0.0), |win| (win.width(), win.height()))
    };
    let s = super::seam_scale(h);
    if w > 0.0 && h > 0.0 {
        // The VM lives in a 768-tall root, so logical size over the seam scale is its screen. No
        // `UIParent_ManageFramePositions` here, unlike `tick_script`: nothing is laid out yet.
        script.set_screen_size(w / s, h / s);
    }
    let Some(atlas) = world.get_resource::<crate::ui_text::UiFontAtlas>() else {
        return;
    };
    super::extract::seat_text_measurer(script, atlas, s);
}

/// Which `"player"` a UI load runs its files under, by the edge that runs it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadSeat {
    /// World entry: `UI_Init` runs before `CMSG_PLAYER_LOGIN` goes out (the load at `0x46c236`,
    /// the send in a later step at `0x46c272`), so no player object exists and the roster's
    /// record stands in ([`seat_from_roster`]).
    Roster,
    /// `/reload`: the player object stays in the world through the teardown and the load, so the
    /// live one is seated ([`crate::ui_unit::live_player_seat`]), the roster's if none exists.
    Live,
}

/// Load the in-game UI for this session onto a VM built for it, once per world entry. The
/// reference builds the UI in `UI_Init` (`0x48fbf0`, re-making the Lua state at `0x48fe97`) and
/// destroys it at `0x490bd0`, so every login runs every addon's file scope afresh. Safe on the
/// state edge because the initial transition runs after `PostStartup`: a capture booting straight
/// into `InWorld` still loads after [`benilla_assets::AssetSet::Open`].
pub(crate) fn load_ingame_ui_on_world_entry(world: &mut World) {
    // Before the load, as `0x48f590` runs before `0x48fbf0`: an addon's file scope sets the mask
    // after it, and a `/reload` never takes this step.
    crate::minimap::restore_default_mask(world);
    // `InitializeGame` arms the play-time warning before it jumps to `UI_Init` (`0x48f636`-`0x48f681`).
    crate::ui_billing::arm_at_world_entry(world);
    load_ingame_ui_seated(world, LoadSeat::Roster);
}

/// [`load_ingame_ui_on_world_entry`] under the `"player"` its edge has.
fn load_ingame_ui_seated(world: &mut World, seat: LoadSeat) {
    // The parked VM goes back into the world, where the CVar fold reads it; a reload has none
    // parked and uses the live one.
    unpark_glue_vm(world);
    if !ui_wanted(world) {
        return;
    }
    mint_entry_vm(world);
    let Some(mut script) = world.remove_non_send_resource::<UiScript>() else {
        warn!("ui_script: entering the world with no VM — the in-game UI will not load");
        return;
    };
    // The character whose AddOn enable state gates which addons run.
    let identity = world
        .get_resource::<crate::glue_session::roster::Roster>()
        .and_then(crate::ui_macro::identity);
    // The realm's character list is the enable store's node set: the reference keeps one
    // `ADDONSTATELIST` node per character, and an addon this one never set resolves from the rest.
    let roster: Vec<String> = world
        .get_resource::<crate::glue_session::roster::Roster>()
        .map(|r| r.chars.iter().map(|c| c.name.clone()).collect())
        .unwrap_or_default();
    // The addon array before any addon file runs: `SMSG_ADDON_INFO` lands during the handshake, so
    // the reference has it before `UI_Init`'s addon walk and `GetNumAddOns()` at file scope sees
    // it. A server that never answered leaves it empty for the session, as in the reference.
    if let Some(reply) = world
        .get_resource::<crate::net::AddonInfoReply>()
        .and_then(|r| r.0.clone())
    {
        script.note_addon_info_records(super::addons::addon_replies(&reply));
    }
    // The CVar table goes in before the UI loads: stock `UIOptionsFrame.xml`'s camera dropdowns
    // read `cameraSmoothStyle` and `cameraSmoothTrackingStyle` in their `OnLoad`, where a nil is a
    // concat error (`UIOptionsFrame.lua:379-384`, `:501-506`), and this VM was born inside this
    // call, before `cvars::sync_cvars`'s per-VM seed.
    match world.get_resource::<crate::cvars::Cvars>() {
        Some(cvars) => cvars.seed_vm(&mut script),
        None => script.register_cvars(crate::cvars::registered_pairs()),
    }
    // The item locks before any file or event runs: a `/reload`'s survive, as on the reference's
    // item objects.
    crate::ui_items::locks::seed_vm_item_locks(world, &mut script, seat == LoadSeat::Live);
    // The realm before the load, since addons read `GetRealmName()` at file scope; the roster
    // carries the realm-list entry this session connected to.
    let realm = world
        .get_resource::<crate::glue_session::roster::Roster>()
        .and_then(|r| r.realm.as_ref().map(|r| r.name.clone()))
        .unwrap_or_default();
    script.set_realm_name(&realm);
    // `GetBillingTimeRested` reads the connection's word (`0x48ec59`), there for every VM.
    if let Some(billing) = world.get_resource::<crate::net::SessionBilling>() {
        script.set_billing_time_rested(billing.0.time_rested);
    }
    // `CGGameUI::InitializeGame` flags these read-only before `UI_Init` loads anything
    // (`0x48f566`-`0x48f584`), so an in-game `SetCVar` of one raises, logout handlers included;
    // `ShutdownGame` clears them (`0x491240`), and here the flags die with this VM. The host's
    // own realm writes are engine writes, which the flag does not stop.
    for name in benilla_ui::script::IN_WORLD_READ_ONLY_CVARS {
        script.set_cvar_read_only(name, true);
    }
    // The player too, before the addon walk. The record first: the reference's
    // `UnitName`/`UnitRace`/`UnitClass`/`UnitSex` read only a copy of the char-enum row made at the
    // Enter World commit (`0x5abd9e`) and never cleared, and each rebuilt VM is told once. Then
    // the unit: the live player on a `/reload`, else the roster's stand-in until the descriptor
    // streams in.
    if let Some(record) = world
        .get_resource::<crate::glue_session::roster::Roster>()
        .and_then(|roster| record_from_roster(roster, crate::ui_unit::world_unit_names(world)))
    {
        script.set_player_record(record);
    }
    match (seat == LoadSeat::Live)
        .then(|| live_player_seat(world))
        .flatten()
    {
        Some(live) => {
            script.set_unit("player", Some(live.state));
            script.set_player_xp(live.xp.0, live.xp.1);
        }
        None => {
            if let Some(seat) = world
                .get_resource::<crate::glue_session::roster::Roster>()
                .and_then(|roster| {
                    seat_from_roster(roster, crate::ui_unit::world_unit_names(world))
                })
            {
                script.set_unit("player", Some(seat));
            }
        }
    }
    // The addon version gate, read from the persisted base the fold above just made current, so a
    // character-screen "Load out of date AddOns" click counts. A bare test world is check on.
    let version_check = world
        .get_resource::<crate::cvars::Cvars>()
        .is_none_or(crate::cvars::Cvars::addon_version_check);
    // Per-VM `Update` claims the load itself reads, seeded now: the zone-channel catalog (empty,
    // `General` would be filed and joined as a custom channel), the binding table (stock
    // `ActionButton_OnLoad` paints the hotkeys), the default language (`ChatFrame_OnEvent`'s
    // `PLAYER_ENTERING_WORLD` arm stores it, `ChatFrame.lua:1275-1276`) and the language table a
    // file-scope `SendChatMessage` names its tongue from.
    crate::ui_chat::seed_zone_channel_catalog(world, &mut script);
    crate::bindings::seed_bindings_for_vm(world, &mut script);
    // The engine globals a previous VM folded out: the quest log's fold mask and the tradeskill
    // repeat.
    crate::ui_quest_log::seed_quest_folds(world, &mut script);
    crate::ui_tradeskill::seed_repeat(world, &mut script);
    crate::ui_unit::seed_default_language(world, &mut script);
    crate::ui_unit::seed_language_table(world, &mut script);
    // The world map's continent and zone lists: static DBC data in the reference, read at file
    // scope (Astrolabe builds its whole zone table from `GetMapContinents`/`GetMapZones` there).
    crate::ui_world_map::seed_world_map_catalog(world, &mut script);
    // Screen size and measurer before the first `<OnLoad>`: this VM is loaded before `tick_script`
    // seats either, and the load measures text (the stock tab template's `OnLoad` sizes from its
    // text width, `UIPanelTemplates.xml:370-373`), which answers 0 with no measurer.
    seat_raster_seam_for_load(world, &mut script);
    // Read out of the world first: the bracket below borrows `world` for the chat-cache restore.
    let ui_scale = world
        .get_resource::<super::UiScaleCvar>()
        .map_or(1.0, |c| c.0);
    let zoom = {
        let z = world.resource::<crate::minimap::MinimapZoom>();
        (z.outdoor, z.inside)
    };
    // A plate setting `config.toml` still holds, carried into FrameXML's saved variables once.
    // Only by a load with the layer, which registers `FRIENDNAMEPLATES_ON` for save; a stock-UI
    // load leaves it for the next layered one.
    let legacy_plates = if super::manifest::layer_enabled() {
        world
            .get_resource_mut::<crate::cvars::Cvars>()
            .map(|mut cvars| crate::vplates::take_legacy_settings(&mut cvars))
            .unwrap_or_default()
    } else {
        Default::default()
    };
    // No sound during the load edge, as in the reference's `UI_Init` (`0x48fbfa` → `0x49016d`).
    silenced_ui_load(&mut script, |script| {
        let _ = load_ingame_ui(script, identity.as_ref(), &roster, version_check);
        crate::ui_chat::commands::register_dev_commands(script, crate::run_mode::dev_affordances());
        // The new Minimap's zoom indices from the persisted CVars, as the reference's minimap reset
        // copies each CVar into its live index; `Minimap:SetZoom` owns them from here.
        script.set_minimap_zoom(zoom.0, zoom.1);
        // The saved-variables chunk runs after the files set their defaults and before
        // `VARIABLES_LOADED`, the reference's order (`AddOn_Load` `0x51f240`); reversed, the
        // defaults always win. The chat cache restores after `VARIABLES_LOADED` and before any chat
        // is routed: its `UPDATE_CHAT_WINDOWS` registers each chat frame for its `CHAT_MSG_*`
        // events (`ChatFrame.lua:1261-1273`).
        finish_ui_load_with(
            script,
            // Before `VARIABLES_LOADED`, whose `UIParent_OnEvent` arm runs the first
            // `UpdateNameplates()` (`UIParent.lua:231-234`).
            |script| crate::vplates::seat_legacy_settings(script, &legacy_plates),
            |script| {
                crate::ui_chat::restore_chat_looks(world, script);
                // The UI scale lands on `UIParent` once the files, the addons and
                // `VARIABLES_LOADED` are done, after the chat reader (`0x4900d6`) and before the
                // `PLAYER_LOGIN` arm (`0x49011d`), through the display handler (`0x49010e` →
                // `0x492e90`). So every `OnLoad` sees `UIParent` at scale 1 and the root's size.
                let display = {
                    let mut q =
                        world.query_filtered::<&Window, With<bevy::window::PrimaryWindow>>();
                    q.single(world).map_or((0, 0), |w| {
                        (w.width().round() as u32, w.height().round() as u32)
                    })
                };
                let mut none = None;
                let mut cache = world.get_resource_mut::<super::UiAutoScaleCache>();
                let cache = cache.as_deref_mut().map_or(&mut none, |c| &mut c.0);
                script.seat_ui_scale(ui_scale, display, cache);
            },
        );
    });
    // The load's record goes to `Logs\FrameXML.log` (`0x490187`-`0x4901b6`), after any
    // `LoadAddOn` block the load ran; nothing reaches chat.
    script.finish_ui_load_log();
    super::load_log::write(script.take_load_log_writes());
    // `DAMAGE_TEXT_FONT` binds at the end of the load, after every addon's `ADDON_LOADED` (where
    // MikScrollingBattleText and pfUI assign it). The reference reads it eagerly, once per world
    // entry (`0x6c8470`, called at `0x401620` in `0x401570`, after the UI load at `0x401602`),
    // and never again: `[0xce8820]` has one writer, and a `/reloadui` does not re-run it, where
    // this edge re-reads it on every reload.
    world.insert_resource(crate::combat_text::read_damage_text_font(&script));
    // The load is over: disarm its instruction bound, so no handler is killed for being slow.
    script.clear_instruction_budget();
    world.insert_non_send_resource(script);
    world.insert_resource(AddOnIdentity(identity));
    // Arm the world latch, the reference's `0x490168 call 0x4908c0`: a reload creates no entity
    // to re-arm it, so without this a second `/reload` would not fire `PLAYER_LEAVING_WORLD`. Only
    // with our own player present (`0x490166 je`, the GUID still 0 on a fresh login, which arms
    // from the player's create instead), so a load the server then refuses arms nothing.
    let seated = world
        .query_filtered::<(), With<crate::net::SelfPlayer>>()
        .iter(world)
        .next()
        .is_some();
    if let Some(mut armed) = world
        .get_resource_mut::<super::LeavingWorldArmed>()
        .filter(|_| seated)
    {
        armed.arm();
    }
}

/// The live player's seat off the world, through the unit feed's own builder; `None` before our
/// own create. A world without the feed's resources (a bare test world) has none to read.
fn live_player_seat(world: &mut World) -> Option<crate::ui_unit::LivePlayerSeat> {
    world
        .run_system_cached(crate::ui_unit::live_player_seat)
        .unwrap_or_else(|e| {
            warn!("ui_script: no live player to seat for the load ({e}); the roster stands in");
            None
        })
}

/// The wire's gender byte (0 male, 1 female) on `UnitSex`'s 2/3 scale, as `ui_unit::snapshot`
/// maps the descriptor's; the record and the seat share it.
fn roster_sex(gender: u8) -> u8 {
    match gender {
        0 => 2,
        1 => 3,
        _ => 0,
    }
}

/// The local player record the UI loads under: our copy of the char-enum row the reference copies
/// at the Enter World commit (layout on [`benilla_ui::script::PlayerRecord`]). Race and class go
/// through the same lookups as [`seat_from_roster`]; the reference resolves them through
/// `ChrRaces`/`ChrClasses` at call time, the same answer. No level: the reference's record has
/// one at `+0x108`, but its accessor (`0x5abe00`) has no caller, so `UnitLevel("player")` reads
/// the descriptor.
pub(crate) fn record_from_roster(
    roster: &crate::glue_session::roster::Roster,
    names: crate::ui_unit::UnitNames<'_>,
) -> Option<benilla_ui::script::PlayerRecord> {
    let row = roster.pending_row()?;
    let race = names.race(row.race);
    let class = names.class(row.class);
    Some(benilla_ui::script::PlayerRecord {
        name: row.name.clone(),
        race: race.map(|(n, f)| (n.to_string(), f.to_string())),
        class: class.map(|(n, f)| (n.to_string(), f.to_string())),
        sex: roster_sex(row.gender),
    })
}

/// The `"player"` snapshot the UI loads under, from the roster row of the pick in flight, until
/// the self descriptor streams in; `None` with no pick (a capture, a scenario, a test world).
/// [`record_from_roster`] answers name, race, class and sex; this carries what else addon file
/// scope reads, the faction group chief among it. Health and power stay zero, the descriptor's
/// to say.
pub(crate) fn seat_from_roster(
    roster: &crate::glue_session::roster::Roster,
    names: crate::ui_unit::UnitNames<'_>,
) -> Option<benilla_ui::script::UnitState> {
    let row = roster.pending_row()?;
    let race = names.race(row.race);
    let class = names.class(row.class);
    let sex = roster_sex(row.gender);
    Some(benilla_ui::script::UnitState {
        // Before the player object exists the reference answers `UnitExists("player")`
        // (`0x515fb0`) nil: the resolver reads the GUID from the object (`0x515994`), and the
        // fallback `0x491900` bails on the zero GUID (`0x4e80aa`) to `0x516001 lua_pushnil`.
        // `UnitLevel` (`0x517fc0`) answers the number 0 (`0x51813e push 0`); the two misses differ.
        exists: false,
        name: Some(row.name.clone()),
        race: race.map(|(n, _)| n.to_string()),
        race_file: race.map(|(_, f)| f.to_string()),
        class: class.map(|(n, _)| n.to_string()),
        class_file: class.map(|(_, f)| f.to_string()),
        sex,
        is_player: true,
        player_controlled: true,
        // AceDB-2.0 concatenates `UnitFactionGroup("player")` at file scope, where nil is an error.
        faction_group: crate::ui_unit::race_faction_group(row.race).map(str::to_string),
        ..Default::default()
    })
}

/// The character the loaded AddOn state belongs to, kept so the shutdown writes go back to its
/// files after the roster's pick is gone.
#[derive(Resource, Default)]
pub(crate) struct AddOnIdentity(pub(crate) Option<(String, String)>);

/// The UI shutdown's `PlaySound` suppression scope (`0x490bd1` → `0x458f50`, closed at
/// `0x490cd9` → `0x458f60`), around the whole of it, on the VM in the world: `true` when entered.
fn silence_ui_shutdown(world: &World) -> bool {
    world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::push_sound_suppression)
        .is_some()
}

/// Leave [`silence_ui_shutdown`]'s scope on the same VM.
fn unsilence_ui_shutdown(world: &World) {
    if let Some(script) = world.get_non_send_resource::<UiScript>() {
        script.pop_sound_suppression();
    }
}

/// Which root runs the UI shutdown `0x490bd0`, which decides the order of its front.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ShutdownRoot {
    /// `/reload` (`0x495664` → `0x490bd0`): the world and its objects stay.
    Reload,
    /// The world's teardown `0x401ee0`, behind a logout, a disconnect, a refused login and an
    /// exit in the world: it destroys the objects first (`0x402011` → `0x467700`), the local
    /// player's destructor sweeping (`0x5dd543`), and nulls the object manager (`0x46771e`)
    /// before it calls the shutdown (`0x402039` → `0x491180` → `0x490bd0`). At the character
    /// screen it returns at once (`0x401ef3`, `[0x882734]` clear), so a quit there runs nothing.
    WorldTeardown,
}

/// The shutdown up to `PLAYER_LOGOUT`, in `root`'s order; `true` when it opened the suppression
/// scope (`0x490bd1`), which the caller closes after the tail (`0x490cd9`).
///
/// - `/reload`: the scope, the head with its sends and events, then past the active-player guard
///   (`0x490bee`-`0x490bf5`) the found arm's move-action cancel (`0x490c1b` → `0x60fb60(0, 1)`)
///   and the sweep (`0x490c20`).
/// - The world's teardown: the sweep from the local player's destructor, then the scope and a
///   silent head: with no object manager `SetSelection(0, 0)` only clears the selection
///   (`0x493549` → `0x467d00`, `0x4939d3`, returning at `0x4939df` before the send and the
///   event), and `CMSG_CANCEL_TRADE` still goes. The guard then finds no player.
///
/// The sweep runs only with our own player present: the guard's `0x468550`, and on the teardown
/// the destructor's own gate (`0x5dd71c`), so a refused login sweeps nothing.
fn run_shutdown_front(world: &mut World, root: ShutdownRoot) -> bool {
    let seated = world
        .query_filtered::<(), With<crate::net::SelfPlayer>>()
        .iter(world)
        .next()
        .is_some();
    match root {
        ShutdownRoot::Reload => {
            let silenced = silence_ui_shutdown(world);
            run_ui_shutdown_head(world, true);
            if seated {
                cancel_move_action(world);
                super::run_leave_world_sweep(world);
            }
            silenced
        }
        ShutdownRoot::WorldTeardown => {
            if seated {
                super::run_leave_world_sweep(world);
            }
            let silenced = silence_ui_shutdown(world);
            run_ui_shutdown_head(world, false);
            silenced
        }
    }
}

/// `0x490bd0`'s head, ahead of its active-player guard: `SetSelection(0, 0)` (`0x490bdf`), then
/// `CMSG_CANCEL_TRADE` (`0x490be4` → `0x5d4310`), a send the sweep's trade leg repeats.
///
/// `loud`, with the object manager up (a `/reload`): the deselect's teardown (`0x493910(1)`)
/// closes a loot window on the target, sends `CMSG_SET_SELECTION` 0 and tells the VM with
/// `PLAYER_TARGET_CHANGED` (`0x493a2a`, `0x493a73`), unless the player is charmed, which makes
/// `SetSelection` a no-op (`0x493589`-`0x49359c`). Not `loud`, after the world's teardown: the
/// selection is only cleared ([`ShutdownRoot::WorldTeardown`]).
fn run_ui_shutdown_head(world: &mut World, loud: bool) {
    if loud {
        let looting = world
            .get_resource::<crate::ui_loot::LootState>()
            .and_then(crate::ui_loot::LootState::source);
        // A harness without the target stack has no selection to drop.
        let deselected = world
            .run_system_cached(crate::target::deselect_on_ui_shutdown)
            .unwrap_or(false);
        // The teardown's own loot close, on a window looting the target (`0x493959`-`0x493974`).
        let loot_closed = looting.is_some()
            && world
                .get_resource::<crate::ui_loot::LootState>()
                .and_then(crate::ui_loot::LootState::source)
                .is_none();
        if loot_closed {
            let _ = world.run_system_cached(crate::ui_loot::tell_loot_closed);
        }
        if deselected {
            if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
                script.set_unit("target", None);
                script.set_unit("targettarget", None);
                script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
            }
        }
    } else {
        let _ = world.run_system_cached(crate::target::clear_selection_on_world_teardown);
    }
    if let Some(net) = world.get_resource::<crate::net::NetCommands>() {
        let _ = net.0.send(crate::net::ClientCommand::CancelTrade);
    }
}

/// The `/reload` guard's found arm (`0x490c0d`-`0x490c1b` → `0x60fb60(0, 1)`): a follow or a
/// Click to Move walk in flight is cancelled and the mover stopped, a follow telling the VM
/// `AUTOFOLLOW_END` (`0x60fb84`), so the new VM starts with none.
fn cancel_move_action(world: &mut World) {
    let followed = world
        .run_system_cached(crate::player::cancel_move_action_on_ui_shutdown)
        .unwrap_or(false);
    if followed {
        if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
            script.fire_event("AUTOFOLLOW_END", vec![]);
        }
    }
}

/// The UI shutdown in the reference's order, `0x490bd0`'s tail past the leave-world sweep (which
/// fires `PLAYER_LEAVING_WORLD`, [`super::run_leave_world_sweep`]): `PLAYER_LOGOUT` (271,
/// `0x490c2a`) → `layout-cache.txt` → the flat saved file (`0x490c7e`) → the per-addon files
/// (`0x490c83`) → `AddOns.txt` (`0x490c88`) → destroy the frame-script owner (`0x490c97`) → nil
/// all 216 C bindings out of `_G` (`0x490cba` → `0x490ce0`).
///
/// `PLAYER_LOGOUT` fires before any write, an addon's last chance to change a saved global, and on
/// every root, where the sweep runs only past the active-player guard (`0x490bee call 0x468550`,
/// `0x490bf5 je 0x490c25`) and the world latch. One function on every root because the steps are
/// ordered against each other; the layout save is here because a `/reload` never leaves
/// `InWorld`. No autosave, as the reference has none (the layout cache's crash-only debounce in
/// [`crate::ui_layout`] is ours).
pub(crate) fn shutdown_ui_state(script: &mut UiScript, identity: Option<&(String, String)>) {
    script.fire_event("PLAYER_LOGOUT", vec![]);
    crate::ui_layout::save_now(script, identity);
    crate::ui_saved::save(script);
    addons::save_addon_variables(script, identity);
    addons::save_enable_state(script, identity);
}

/// `OnExit(InWorld)`, a `/logout` or a disconnect (two of the reference's five roots): run the
/// shutdown tail, then end this session's Lua state, so no frame, global or addon upvalue reaches
/// the next login. The reference's state outlives `0x490bd0` and is replaced twice through
/// `0x703b80`, in `ShutdownGame` (at `0x491231`) and then in the glue build (`0x46a7b0`), so the
/// character screen runs on a state of its own. Ours is a fresh glue VM ([`install_glue_side_vm`]).
pub(crate) fn end_ui_session(world: &mut World) {
    close_ui_session(world, ShutdownRoot::WorldTeardown);
    // The session's end drops what the fold just carried, as the disconnect handler, a frame
    // earlier, dropped the rest: only a `/reload` seats it again.
    if let Some(mut log) = world.get_resource_mut::<crate::ui_quest_log::QuestLog>() {
        log.forget_folds();
    }
    if let Some(mut carry) = world.get_resource_mut::<crate::ui_tradeskill::TradeSkillRepeatCarry>()
    {
        carry.0 = 0;
    }
    if let Some(mut petition) = world.get_resource_mut::<crate::ui_petition::PetitionState>() {
        petition.forget_announced();
    }
    // `0x401ee0`'s glue leg: `"charselect"` after a logout, `"login"` after a disconnect
    // (`0x402084`); the state this exit enters says which.
    let screen = match world
        .get_resource::<State<crate::client_state::ClientState>>()
        .map(|s| *s.get())
    {
        Some(crate::client_state::ClientState::CharSelect) => super::glue::GlueScreen::CharSelect,
        _ => super::glue::GlueScreen::Login,
    };
    install_glue_side_vm(world, screen.key());
}

/// [`end_ui_session`] up to the new VM: the shutdown tail, the folds and the host values reset.
fn close_ui_session(world: &mut World, root: ShutdownRoot) {
    // The front runs before the unpark, so a VM still parked (the load never ran) hears none of
    // it; the sweep's latch is spent at the root, so of this edge and the worldport's, the first
    // to a departure owns it.
    let silenced = run_shutdown_front(world, root);
    // A VM still parked goes back into the world first.
    unpark_glue_vm(world);
    // An entry load still armed means no in-game UI was built, and the tail would overwrite the
    // saved files with the glue VM's emptiness.
    let ui_never_loaded = world.remove_resource::<PendingEntryUiLoad>().is_some();
    let identity = world
        .get_resource::<AddOnIdentity>()
        .and_then(|id| id.0.clone());
    if !ui_never_loaded {
        if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
            shutdown_ui_state(&mut script, identity.as_ref());
        }
    }
    if silenced {
        unsilence_ui_shutdown(world);
    }
    // After the shutdown events (a `PLAYER_LOGOUT` handler may `SetCVar`, which in the reference
    // lands in an engine-side store that survives) and before the VM is replaced.
    crate::cvars::fold_dying_vm_cvars(world);
    // The chat cache folds here, not in its own `OnExit(InWorld)` system, which a `/reload` skips.
    crate::ui_chat::settings::fold_dying_vm_chat_cache(world);
    // The item locks too: the reference keeps them on the item objects, which a `/reload` keeps.
    crate::ui_items::locks::fold_dying_vm_item_locks(world);
    // So do the engine globals the VM holds for the session: the quest log's fold mask and the
    // tradeskill repeat, which no UI init resets.
    crate::ui_quest_log::fold_dying_vm_quest_folds(world);
    crate::ui_tradeskill::fold_dying_vm_repeat(world);
    world.insert_resource(AddOnIdentity(None));

    // Host values taken from the dying frame tree go with it (a `VmMemo` resets on its own): the
    // hovered frame, the keyboard capture (`feed_ui_input` stops outside `InWorld`), the cursor
    // payload and the minimap's hole. Not [`UiClock`]: `GetTime` is the reference's OS clock, and
    // [`seed_vm_clock`] re-anchors it for the next VM.
    if let Some(mut hover) = world.get_resource_mut::<PlayerUiHover>() {
        hover.0 = None;
    }
    if let Some(mut keys) = world.get_resource_mut::<UiKeyboardCapture>() {
        keys.typing = false;
        keys.arrows_fall_through = false;
        keys.consumed.clear();
    }
    if let Some(mut held) = world.get_resource_mut::<CursorPayloadHeld>() {
        *held = CursorPayloadHeld::default();
    }
    if let Some(mut minimap) = world.get_resource_mut::<crate::minimap::MinimapWidget>() {
        minimap.0 = None;
    }
}

/// A `ReloadUI()` waiting to run: set when [`crate::ui_logout`] drains
/// [`benilla_ui::script::SessionRequest::ReloadUi`], run by [`run_pending_reload`] in the next
/// frame's `PreUpdate`. A flag, as in the reference (`[0xb4b3f4]`, written only by `0x491380` and
/// read only by the per-frame `0x495590`), so the VM that asked is never mid-call when destroyed.
#[derive(Resource, Default)]
pub(crate) struct ReloadUiPending(pub(crate) bool);

/// Run a pending `ReloadUI()`: the reference's teardown and rebuild (`0x495664 call 0x490bd0`,
/// `0x495669 call 0x48fbf0`), for us [`end_ui_session`] then the load without leaving the world,
/// so a `DisableAddOn` staged in the dying VM reaches `AddOns.txt` before the rebuild reads it.
/// The load runs under the live player, which never left. `UI_Init` then enters the world-enter
/// cascade itself (`0x490168`), firing the `PLAYER_LOGIN` it armed and `PLAYER_ENTERING_WORLD`;
/// ours fire from [`crate::ui_unit`]'s feed on the new VM's first `Update`, once it has re-seated
/// the player. In-world only, as the reference's gate (`0x494a50(0xa)`) is; elsewhere the request
/// is dropped, and the glue's own rebuild is [`super::glue::run_pending_glue_rebuild`]'s.
pub(crate) fn run_pending_reload(world: &mut World) {
    if !std::mem::take(&mut world.resource_mut::<ReloadUiPending>().0) {
        return;
    }
    let in_world = *world
        .resource::<State<crate::client_state::ClientState>>()
        .get()
        == crate::client_state::ClientState::InWorld;
    if !in_world {
        info!("ui_script: ReloadUI outside the world — dropped");
        return;
    }
    info!("ui_script: ReloadUI — ending the UI session and building a new one");
    // A boot VM, never the glue's: the reload builds no glue (`0x495664` then `0x495669`), and the
    // load replaces this VM at once.
    close_ui_session(world, ShutdownRoot::Reload);
    install_boot_vm(world);
    load_ingame_ui_seated(world, LoadSeat::Live);
    // `UI_Init` ends by re-entering the world-enter cascade (`0x490168`), its sends included, but
    // only with the active player set (`0x490166 je`): our own player's entity exists. Not
    // `SetActiveMover`, which only the player's create runs.
    let seated = world
        .query_filtered::<(), With<crate::net::SelfPlayer>>()
        .iter(world)
        .next()
        .is_some();
    if seated {
        world.write_message(crate::net::WorldEnterCascadeMessage);
    }
}

/// `AppExit`, the quit root, read as a message because a quit from in-world never leaves
/// `InWorld`: the world's teardown ([`ShutdownRoot::WorldTeardown`]). A quit on a worldport's
/// loading screen finds the latch spent, so the sweep does not run; a quit from the character
/// screen runs nothing at all (`0x401ef3`).
pub(crate) fn shutdown_on_exit(
    world: &mut World,
    exits: &mut bevy::ecs::system::SystemState<MessageReader<AppExit>>,
) {
    if exits.get_mut(world).read().next().is_none() {
        return;
    }
    let in_world = world
        .get_resource::<State<crate::client_state::ClientState>>()
        .is_some_and(|s| *s.get() == crate::client_state::ClientState::InWorld);
    // Same guard as [`end_ui_session`]: a quit inside the entry-load window has no UI to save.
    if !in_world || world.contains_resource::<PendingEntryUiLoad>() {
        return;
    }
    let silenced = run_shutdown_front(world, ShutdownRoot::WorldTeardown);
    let identity = world
        .get_resource::<AddOnIdentity>()
        .and_then(|id| id.0.clone());
    if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
        shutdown_ui_state(&mut script, identity.as_ref());
    }
    if silenced {
        unsilence_ui_shutdown(world);
    }
}

/// The load's ordered tail without the host seams, for tests; production runs
/// [`finish_ui_load_with`]. The reference's `UI_Init` (`0x48fbf0`) loads every non-LoadOnDemand
/// addon, each firing `ADDON_LOADED` at `0x51f5ad` (`0x4900a3` → `0x51f600`), reads the flat
/// saved file and fires `VARIABLES_LOADED` (`0x4900b2` → `0x4913b0`), and arms `PLAYER_LOGIN`
/// (`[0xb4e260]`, set at `0x49011d`). The world-enter cascade `0x4908c0` fires `PLAYER_LOGIN` if
/// armed (read at `0x49094b`, fired at `0x490959`, disarmed at `0x49095e`), then
/// `PLAYER_ENTERING_WORLD` (`0x49096a`): [`UiScript::fire_world_enter`].
///
/// On a `/reload`, `UI_Init` enters the cascade itself (`0x490168`). On a fresh login that call is
/// skipped (`0x490166 je`, the active-player GUID still 0) and the cascade runs from the local
/// player's own create (`0x5deb60 call 0x4908c0`). Either way the player object exists when it
/// runs, so this only arms, and [`crate::ui_unit`]'s feed fires both once it has seated the
/// player.
#[cfg(test)]
pub(crate) fn finish_ui_load(script: &mut UiScript) {
    finish_ui_load_with(script, |_| {}, |_| {});
}

/// [`finish_ui_load`] with its two host seams. `host_settings` runs between the saved-variables
/// chunk and `VARIABLES_LOADED`, carrying over a setting `config.toml` held before it moved into
/// the saved variables. `between` runs after `VARIABLES_LOADED` and before the `PLAYER_LOGIN` arm
/// (so before the event too): the chat-cache restore's `UPDATE_CHAT_WINDOWS` +
/// `UPDATE_CHAT_COLOR` burst, where the reference registers its reader (`0x4900d6` → `0x498a20`),
/// then the UI scale (`0x49010e`). On a fresh login the reference's cache (`0x5afe50`) defers that
/// reader until the server's account data reconciles (`0x5afa42`/`0x5afd01`), after `UI_Init` has
/// returned. Either way a `VARIABLES_LOADED` handler sees the boot chat colours, not the file's.
pub(crate) fn finish_ui_load_with(
    script: &mut UiScript,
    host_settings: impl FnOnce(&mut UiScript),
    between: impl FnOnce(&mut UiScript),
) {
    // Still the load edge, so still bounded: re-armed so the saved variables and every
    // `VARIABLES_LOADED` handler get the full allowance; the entry edge disarms it after. The
    // world-enter events fire later, unbounded, as every event the feeds fire is.
    script.set_instruction_budget(addons::LOAD_INSTRUCTION_BUDGET);
    crate::ui_saved::load_saved_variables(script, host_settings);
    between(script);
    script.arm_player_login();
}

/// Run the patch chain's `Interface\FrameXML\GlobalStrings.lua`, the first file in the reference's
/// `FrameXML.toc` and the source of every localized string global. Failures are logged loudly:
/// without it every red error line is silently empty.
fn load_global_strings(world: &mut World, script: &UiScript) {
    let Some(assets) = world.get_resource::<benilla_assets::WorldAssets>() else {
        warn!("ui_script: no patch chain — GlobalStrings absent, error lines will be empty");
        return;
    };
    let bytes = {
        let mut chain = assets.chain.lock_recover();
        chain.read_file("Interface\\FrameXML\\GlobalStrings.lua")
    };
    let src = match bytes {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => {
            error!("ui_script: GlobalStrings.lua read failed — error lines will be empty: {e:#}");
            return;
        }
    };
    if let Err(e) = script.run(&src) {
        error!("ui_script: GlobalStrings.lua failed to run: {e}");
        return;
    }
    // Sentinel: the cast-fail drain's own lookup, checked for presence only (any locale).
    let sentinel: Option<String> = script.lua().globals().get("SPELL_FAILED_NO_AMMO").ok();
    match sentinel {
        Some(s) if !s.is_empty() => info!("ui_script: GlobalStrings loaded"),
        other => {
            error!("ui_script: GlobalStrings sentinel missing ({other:?}) — error lines broken")
        }
    }
}

/// Run the emote token table from the patch chain's `ChatFrame.lua` (`EMOTE87_TOKEN = "SIT"`),
/// which maps each `GlobalStrings.lua` alias (`EMOTE87_CMD1 = "/sit"`) to its `EmotesText` name.
/// Only whole lines matching [`is_emote_token_line`] run, none of the file's code.
fn load_emote_tokens(world: &mut World, script: &UiScript) {
    let Some(assets) = world.get_resource::<benilla_assets::WorldAssets>() else {
        return; // already warned by `load_global_strings`
    };
    let bytes = {
        let mut chain = assets.chain.lock_recover();
        chain.read_file("Interface\\FrameXML\\ChatFrame.lua")
    };
    let src = match bytes {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => {
            error!("ui_script: ChatFrame.lua read failed — emote commands will be dead: {e:#}");
            return;
        }
    };
    let table: Vec<&str> = src
        .lines()
        .map(str::trim)
        .filter(|l| is_emote_token_line(l))
        .collect();
    let count = table.len();
    if let Err(e) = script.run(&table.join("\n")) {
        error!("ui_script: emote token table failed to run: {e}");
        return;
    }
    // Sentinel: `EMOTE87` is `/sit`.
    let sentinel: Option<String> = script.lua().globals().get("EMOTE87_TOKEN").ok();
    match sentinel.as_deref() {
        Some("SIT") => info!("ui_script: {count} emote tokens loaded"),
        other => error!(
            "ui_script: emote token sentinel is {other:?}, not \"SIT\" ({count} lines) — \
             emote slash commands are broken"
        ),
    }
}

/// Whether a line is one of `ChatFrame.lua`'s `EMOTE<digits>_TOKEN = "<NAME>";` assignments, with
/// `NAME` in `[A-Z0-9_]`; that whole-line shape makes running it the same as reading data.
pub(crate) fn is_emote_token_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("EMOTE") else {
        return false;
    };
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let Some(rest) = rest[digits..].strip_prefix("_TOKEN = \"") else {
        return false;
    };
    let Some(name) = rest.strip_suffix("\";") else {
        return false;
    };
    digits > 0
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The roster's stand-in `"player"` and the record `UnitRace`/`UnitClass` read carry the
    /// table's names at the install's locale, and the tokens beside them.
    #[test]
    fn the_roster_seat_and_record_name_race_and_class_from_the_tables() {
        let mut c = crate::glue_session::roster::test_character(7, "Harness");
        c.race = 4; // Night Elf
        c.class = 5; // Priest
        let roster = crate::glue_session::roster::Roster::with_pending_pick(vec![c], 7);
        let korean = crate::ui_unit::TestNames::korean();
        let seat = seat_from_roster(&roster, korean.names()).expect("a pick seats");
        assert_eq!(seat.race.as_deref(), Some("나이트 엘프"));
        assert_eq!(seat.race_file.as_deref(), Some("NightElf"));
        assert_eq!(seat.class.as_deref(), Some("사제"));
        assert_eq!(seat.class_file.as_deref(), Some("PRIEST"));
        let record = record_from_roster(&roster, korean.names()).expect("a pick records");
        assert_eq!(
            record.race,
            Some(("나이트 엘프".to_string(), "NightElf".to_string()))
        );
        assert_eq!(
            record.class,
            Some(("사제".to_string(), "PRIEST".to_string()))
        );
    }

    /// Every VM, glue and in-game, answers `GetLocale` with the install's locale (`[0xc0e080]`).
    #[test]
    fn every_new_vm_is_seated_with_the_installs_locale() {
        let mut world = World::new();
        let want = super::super::client_locale();
        let in_game = new_vm(&mut world, UiScript::new).expect("a VM");
        let glue = new_vm(&mut world, UiScript::new_glue).expect("a glue VM");
        for script in [in_game, glue] {
            assert_eq!(script.locale(), want.index());
            let answer = script.eval::<String>("return GetLocale()").unwrap();
            assert_eq!(answer, want.name());
            assert_eq!(script.client_region(), super::super::client_region());
        }
    }

    /// `[0xcf4d24]` has one writer, `SetEuropeanNumbers` (`0x771c40`), and nothing resets it, so
    /// the VM that replaces a dying one (a `ReloadUI`, a logout, a login) opens with its value.
    #[test]
    fn the_european_numbers_switch_outlives_the_vm_that_set_it() {
        let mut world = World::new();
        let first = UiScript::new().unwrap();
        assert!(!first.european_numbers(), "off at boot");
        world.insert_non_send_resource(first);
        let text = |world: &World| {
            let script = world.non_send_resource::<UiScript>();
            script
                .run(
                    "F = CreateFrame('Frame'):CreateFontString() \
                     F:SetFont('Fonts\\\\FRIZQT__.TTF', 12) F:SetText('1.5')",
                )
                .unwrap();
            script.eval::<String>("return F:GetText()").unwrap()
        };
        world
            .non_send_resource::<UiScript>()
            .run("SetEuropeanNumbers(1)")
            .unwrap();
        for _ in 0..2 {
            let next = new_vm(&mut world, UiScript::new).expect("a VM");
            world.insert_non_send_resource(next);
            assert_eq!(text(&world), "1,5");
        }
        world
            .non_send_resource::<UiScript>()
            .run("SetEuropeanNumbers(false)")
            .unwrap();
        let next = new_vm(&mut world, UiScript::new).expect("a VM");
        world.insert_non_send_resource(next);
        assert_eq!(text(&world), "1.5");
    }

    #[test]
    fn the_entry_edge_parks_the_glue_vm_and_the_session_end_returns_it() {
        let mut world = World::new();
        let boot = UiScript::new().unwrap();
        let session = boot.session();
        world.insert_non_send_resource(boot);
        arm_entry_ui_load(&mut world);
        assert!(
            world.get_non_send_resource::<UiScript>().is_none(),
            "no VM in the deferral window"
        );
        assert!(world.get_resource::<PendingEntryUiLoad>().is_some());
        assert!(world.get_non_send_resource::<ParkedGlueVm>().is_some());
        // Left the world before the load ran: the VM comes back, untouched.
        unpark_glue_vm(&mut world);
        let vm = world
            .get_non_send_resource::<UiScript>()
            .expect("the parked VM is back");
        assert_eq!(vm.session(), session, "the same VM, no session moved");
        assert!(world.get_non_send_resource::<ParkedGlueVm>().is_none());
    }

    /// The plate bits clear at the world entry (`0x401639`, in the entry setup `0x401570`) and
    /// not at a `ReloadUI()`, which never runs that setup. A capture world, so no UI is built.
    #[test]
    fn the_world_entry_clears_the_plate_bits_and_a_reload_does_not() {
        use crate::vplates::VPlateMode;
        let on = VPlateMode {
            enemies: true,
            friends: true,
        };
        let world_with = |on: VPlateMode| {
            let mut world = World::new();
            world.insert_resource(State::new(crate::client_state::ClientState::InWorld));
            world.insert_resource(crate::run_mode::CaptureMode);
            world.insert_resource(on);
            world
        };

        let mut world = world_with(on);
        world.insert_resource(PendingEntryUiLoad);
        run_pending_entry_load(&mut world);
        assert_eq!(*world.resource::<VPlateMode>(), VPlateMode::default());

        let mut world = world_with(on);
        world.insert_resource(ReloadUiPending(true));
        run_pending_reload(&mut world);
        assert!(!world.resource::<ReloadUiPending>().0, "the reload ran");
        assert_eq!(*world.resource::<VPlateMode>(), on, "a reload keeps them");
    }

    /// A `ReloadUI()` runs the leave-world sweep (`0x490c20`, past the armed latch) and then
    /// re-enters the world-enter cascade (`0x490168`), whose readers re-send the time, mail,
    /// battlefield and meeting-stone queries, only with the active player set (`0x490166 je`).
    #[test]
    fn a_reload_sweeps_and_reenters_the_world_enter_cascade_once_the_player_exists() {
        use crate::net::WorldEnterCascadeMessage;
        let reload = |seated: bool| {
            let mut world = World::new();
            world.insert_resource(State::new(crate::client_state::ClientState::InWorld));
            world.insert_resource(crate::run_mode::CaptureMode);
            world.init_resource::<Messages<WorldEnterCascadeMessage>>();
            world.init_resource::<super::super::SweptWorlds>();
            world.init_resource::<super::super::LeavingWorldArmed>();
            world
                .resource_mut::<super::super::LeavingWorldArmed>()
                .arm();
            if seated {
                world.spawn(crate::net::SelfPlayer);
            }
            world.insert_resource(ReloadUiPending(true));
            run_pending_reload(&mut world);
            assert!(!world.resource::<ReloadUiPending>().0, "the reload ran");
            let sweeps = world.resource::<super::super::SweptWorlds>().0;
            let cascades = world
                .resource_mut::<Messages<WorldEnterCascadeMessage>>()
                .drain()
                .count();
            (sweeps, cascades)
        };
        assert_eq!(reload(true), (1, 1), "in the world");
        assert_eq!(
            reload(false).1,
            0,
            "no cascade before our own player's create"
        );
    }

    /// The agreements are read from their CVars before the first screen is set (`0x46a400` at
    /// `0x402b36`, `SET_GLUE_SCREEN` at `0x402b65`): a returning player's login shows the boxes,
    /// not the EULA, and a first launch still shows the EULA.
    #[test]
    fn the_login_screen_comes_up_on_the_accepted_agreements() {
        use crate::client_state::ClientState;
        let _ = benilla_formats::wow_data_or_skip!();
        let screens = |cvars: crate::cvars::Cvars| {
            let mut world = World::new();
            world.insert_resource(State::new(ClientState::Login));
            world.insert_resource(cvars);
            world.init_non_send_resource::<super::super::glue_mgr::SharedGlueMgr>();
            setup_script(&mut world);
            world
                .non_send_resource::<UiScript>()
                .eval::<(bool, bool, bool)>(
                    "return TOSFrame:IsShown() == 1, AccountLoginUI:IsShown() == 1, \
                     AccountLoginAccountEdit:IsVisible() == 1",
                )
                .unwrap()
        };
        let mut accepted = crate::cvars::Cvars::with_value("readEULA", "1");
        accepted.set("readTOS", "1");
        assert_eq!(
            screens(accepted),
            (false, true, true),
            "accepted agreements go straight to the login boxes"
        );
        assert_eq!(
            screens(crate::cvars::Cvars::default()),
            (true, false, false),
            "a first launch is asked for the EULA"
        );
    }

    /// The world holds the glue VM outside the world, at startup on the login screen
    /// (`0x402b65`) and back from the world on the screen the exit enters (`0x402090`); a
    /// `ReloadUI()` never builds the glue.
    #[test]
    fn the_glue_stands_outside_the_world() {
        use crate::client_state::ClientState;
        use benilla_ui::script::VmKind;
        let shown = |world: &World, frame: &str| {
            benilla_formats::wow_data().is_none()
                || world
                    .non_send_resource::<UiScript>()
                    .eval::<bool>(&format!("return {frame}:IsShown() == 1"))
                    .unwrap()
        };
        let mut world = World::new();
        world.insert_resource(State::new(ClientState::Login));
        setup_script(&mut world);
        assert_eq!(
            world.non_send_resource::<UiScript>().vm_kind(),
            VmKind::Glue
        );
        assert!(shown(&world, "AccountLogin"), "the boot screen is login");

        world.insert_resource(State::new(ClientState::CharSelect));
        world.init_resource::<PendingEntryUiLoad>();
        end_ui_session(&mut world);
        assert_eq!(
            world.non_send_resource::<UiScript>().vm_kind(),
            VmKind::Glue
        );
        assert!(
            shown(&world, "CharacterSelect"),
            "a logout puts the character screen up"
        );

        world.insert_resource(State::new(ClientState::InWorld));
        world.insert_resource(crate::run_mode::CaptureMode);
        world.insert_resource(ReloadUiPending(true));
        run_pending_reload(&mut world);
        assert_eq!(
            world.non_send_resource::<UiScript>().vm_kind(),
            VmKind::InGame,
            "the reload's VM is a boot VM, never the glue's"
        );
    }

    /// The reference re-makes its Lua state inside `UI_Init` (`0x48fe97`).
    #[test]
    fn the_world_entry_builds_its_own_vm_rather_than_adopting_the_glue_phases() {
        let mut world = World::new();
        let glue = UiScript::new().unwrap();
        let glue_session = glue.session();
        world.insert_non_send_resource(glue);

        mint_entry_vm(&mut world);

        let entry = world
            .get_non_send_resource::<UiScript>()
            .expect("the entry load has a VM to run on");
        assert_ne!(
            entry.session(),
            glue_session,
            "the entry VM is a new session — every VmMemo resets here, so a one-shot spent \
             against the frameless glue VM is spent again against the one that has an interface"
        );
    }

    /// A one-shot fired at the frameless glue VM reaches nobody (the reference's `0x703f50` drops
    /// an event with no listener), so the entry VM must be told again.
    #[test]
    fn a_one_shot_spent_against_the_character_screens_vm_is_spent_again_after_the_entry() {
        let mut world = World::new();
        world.insert_non_send_resource(UiScript::new().expect("a glue VM"));
        let mut told: super::super::VmMemo<bool> = Default::default();

        assert!(
            told.claim(world.non_send_resource::<UiScript>()),
            "the window: a feed fires its login one-shot at the frameless character-screen VM"
        );
        assert!(
            !told.claim(world.non_send_resource::<UiScript>()),
            "and the memo has it as told — which is correct for THAT VM"
        );

        mint_entry_vm(&mut world);

        assert!(
            told.claim(world.non_send_resource::<UiScript>()),
            "the VM the in-game UI loads onto has never been told, so the feed tells it again — \
             no run condition, no ordering edge, and nothing the feed had to know about"
        );
    }

    /// The reference's `GetTime` is `GetTickCount` × 0.001 (`0x515ea0`) and never restarts; stock
    /// `CooldownFrame_SetTimer` hides a cooldown whose start is not above 0.
    #[test]
    fn a_rebuilt_vm_inherits_the_running_gettime_clock() {
        use std::time::{Duration, Instant};

        let mut world = World::new();
        world.init_resource::<UiClock>();
        // The process has been up 90 s, on `Time<Real>` and on the dying VM alike.
        let startup = Instant::now() - Duration::from_secs(90);
        let mut time = Time::<bevy::time::Real>::new(startup);
        // `Time<Real>` measures `elapsed` from its first update, not from `startup`.
        time.update_with_instant(startup);
        time.update_with_instant(startup + Duration::from_secs(90));
        world.insert_resource(time);
        let mut dying = UiScript::new().unwrap();
        dying.set_now(90.0);
        world.insert_non_send_resource(dying);
        // No in-game UI was loaded, so the shutdown tail and its file writes are skipped.
        world.init_resource::<PendingEntryUiLoad>();

        end_ui_session(&mut world);

        let vm = world
            .get_non_send_resource::<UiScript>()
            .expect("the session end installs a fresh glue VM");
        assert_eq!(
            vm.now(),
            90.0,
            "the new VM's GetTime clock restarted at {} instead of the process's 90 s",
            vm.now()
        );
        // The conversion pair moved too: a 10-minute cooldown armed 30 s before the rebuild keeps
        // its real start.
        let clock = world.resource::<UiClock>();
        let armed = crate::spell::cooldowns::CooldownInfo {
            start: startup + Duration::from_secs(60),
            remaining_ms: 570_000,
            duration_ms: 600_000,
            enabled: true,
        };
        let triple = armed
            .ui_triple(clock.anchor, clock.ui_now)
            .expect("a running cooldown pushes a triple");
        assert_eq!(
            triple,
            (60_000, 600_000, true),
            "a cooldown armed before the rebuild derived start {} ms; stock Cooldown.lua hides \
             anything but `start > 0`",
            triple.0
        );
    }

    const CORPSE: u64 = 0xF130_0000_0000_0042;
    const LEADER: u64 = 0x0000_0000_0000_0009;
    const FRONT_EVENTS: [&str; 4] = [
        "LOOT_CLOSED",
        "PLAYER_TARGET_CHANGED",
        "AUTOFOLLOW_END",
        "PLAYER_LEAVING_WORLD",
    ];

    /// A world in play for the shutdown's front: the corpse targeted and looted, a follow on,
    /// the latch armed, our own player present when `seated` (charmed when `charmed`), and the
    /// sweep with only its own event.
    fn front_world(
        seated: bool,
        charmed: bool,
    ) -> (
        World,
        crossbeam_channel::Receiver<crate::net::ClientCommand>,
    ) {
        use benilla_protocol::messages::ObjectFields;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(crate::net::NetCommands(tx));
        world.init_resource::<crate::target::Selection>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<crate::ui_loot::LootLatch>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::player::Approach>();
        world.init_resource::<crate::player::FollowState>();
        world.resource_mut::<crate::player::FollowState>().guid = Some(LEADER);
        super::super::sweep::install_world_leave_sweep(&mut world, |_| {});
        world
            .resource_mut::<super::super::LeavingWorldArmed>()
            .arm();
        let mut loot = crate::ui_loot::LootState::default();
        loot.open(CORPSE, 1, 0, Vec::new());
        world.insert_resource(loot);
        let body = world.spawn_empty().id();
        {
            let mut selection = world.resource_mut::<crate::target::Selection>();
            selection.target = Some(body);
            selection.guid = Some(CORPSE);
        }
        if seated {
            // `UNIT_FIELD_CHARMEDBY` (10, 11).
            let fields = if charmed {
                ObjectFields::from_pairs(&[(10, 0x77), (11, 0)])
            } else {
                ObjectFields::from_pairs(&[])
            };
            world.spawn((crate::net::SelfPlayer, crate::net::ObjectStore(fields)));
        }
        world.insert_non_send_resource(crate::ui_script::sweep_test::spied_vm(&FRONT_EVENTS));
        (world, rx)
    }

    fn front_seen(world: &World) -> Vec<String> {
        crate::ui_script::sweep_test::seen(world.non_send_resource::<UiScript>())
    }

    /// `/reload` (`0x495664` → `0x490bd0`): the head with the object manager up drops the target
    /// loudly, its teardown closing the loot on it (`0x493959`-`0x493974`) and sending
    /// `CMSG_SET_SELECTION` 0 and `PLAYER_TARGET_CHANGED` (`0x493a2a`, `0x493a73`), then
    /// `CMSG_CANCEL_TRADE` (`0x490be4`); past the guard the found arm ends the follow
    /// (`0x60fb84`), then the sweep (`0x490c20`).
    #[test]
    fn a_reload_drops_the_target_loudly_ends_the_follow_then_sweeps() {
        use crate::net::ClientCommand;
        let (mut world, rx) = front_world(true, false);
        assert!(run_shutdown_front(&mut world, ShutdownRoot::Reload));
        assert_eq!(
            front_seen(&world),
            [
                "LOOT_CLOSED",
                "PLAYER_TARGET_CHANGED",
                "AUTOFOLLOW_END",
                "PLAYER_LEAVING_WORLD"
            ]
        );
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(
                sent.as_slice(),
                [
                    ClientCommand::LootRelease { guid: CORPSE },
                    ClientCommand::SetSelection { guid: 0 },
                    ClientCommand::CancelTrade,
                ]
            ),
            "{sent:?}"
        );
        assert_eq!(world.resource::<crate::target::Selection>().guid, None);
        assert_eq!(world.resource::<crate::player::FollowState>().guid, None);
    }

    /// A charmed player's `SetSelection` is a no-op (`0x493589`-`0x49359c`): a `/reload` keeps
    /// the target and sends no `CMSG_SET_SELECTION`, and the trade cancel still goes.
    #[test]
    fn a_charmed_reload_keeps_the_target() {
        use crate::net::ClientCommand;
        let (mut world, rx) = front_world(true, true);
        run_shutdown_front(&mut world, ShutdownRoot::Reload);
        assert!(!front_seen(&world).contains(&"PLAYER_TARGET_CHANGED".to_string()));
        assert!(rx
            .try_iter()
            .all(|c| !matches!(c, ClientCommand::SetSelection { .. })));
        assert_eq!(
            world.resource::<crate::target::Selection>().guid,
            Some(CORPSE)
        );
    }

    /// A logout, a disconnect or an exit in the world (`0x401ee0`): the destructor's sweep first
    /// (`0x402011` → `0x5dd543`), then the head with no object manager, which clears the
    /// selection silently (`0x4939df`) and still cancels the trade; no follow cancel.
    #[test]
    fn a_world_teardown_sweeps_first_then_clears_the_target_silently() {
        use crate::net::ClientCommand;
        let (mut world, rx) = front_world(true, false);
        world
            .resource_mut::<crate::ui_loot::LootState>()
            .clear_session();
        assert!(run_shutdown_front(&mut world, ShutdownRoot::WorldTeardown));
        assert_eq!(front_seen(&world), ["PLAYER_LEAVING_WORLD"]);
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(sent.as_slice(), [ClientCommand::CancelTrade]),
            "{sent:?}"
        );
        let selection = world.resource::<crate::target::Selection>();
        assert_eq!((selection.guid, selection.last), (None, Some(CORPSE)));
    }

    /// A refused login has no player of ours to destroy, so no sweep; the head still cancels.
    #[test]
    fn a_refused_login_sweeps_nothing_and_cancels_the_trade() {
        use crate::net::ClientCommand;
        let (mut world, rx) = front_world(false, false);
        run_shutdown_front(&mut world, ShutdownRoot::WorldTeardown);
        assert!(front_seen(&world).is_empty());
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(sent.as_slice(), [ClientCommand::CancelTrade]),
            "{sent:?}"
        );
    }

    /// A quit from the character screen runs nothing (`0x401ef3`): no head, no cancel, no
    /// `PLAYER_LOGOUT`.
    #[test]
    fn a_quit_at_the_character_screen_runs_nothing() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut world, rx) = front_world(false, false);
        world.insert_resource(State::new(crate::client_state::ClientState::CharSelect));
        world.init_resource::<AddOnIdentity>();
        world.init_resource::<Messages<AppExit>>();
        world
            .non_send_resource::<UiScript>()
            .run(
                "SEEN_LOGOUT = 0 local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_LOGOUT') \
                 f:SetScript('OnEvent', function() SEEN_LOGOUT = SEEN_LOGOUT + 1 end)",
            )
            .unwrap();
        world.write_message(AppExit::Success);
        world.run_system_once(shutdown_on_exit).unwrap();
        assert_eq!(rx.try_iter().count(), 0);
        assert_eq!(
            world
                .non_send_resource::<UiScript>()
                .eval::<i64>("return SEEN_LOGOUT")
                .unwrap(),
            0
        );
    }
}
