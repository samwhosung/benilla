//! World entry and exit through [`super::load_ingame_ui_on_world_entry`] and
//! [`super::end_ui_session`]: each login runs addon file scope in a fresh VM, and each exit root
//! writes what the session owes. Each test writes its probe addon into a hermetic `BENILLA_HOME`.

use bevy::prelude::*;

use crate::char_select::Roster;
use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

/// The corpus idiom: the character's name read once at file scope. `SwitchProbeLoads` counts
/// file-scope runs within one VM, so a fresh VM reads 1 and a load stacked on the same state 2.
const PROBE_LUA: &str = "\
local currentPlayer = UnitName(\"player\")
SwitchProbeFileScope = currentPlayer
SwitchProbeLoads = (SwitchProbeLoads or 0) + 1
SwitchProbeDB = { who = currentPlayer }
";

/// Declares `SwitchProbeDB` saved per character, so the shutdown writes a real file.
const PROBE_TOC: &str = "\
## Interface: 11200
## SavedVariablesPerCharacter: SwitchProbeDB
SwitchProbe.lua
";

/// A roster with a pending pick named `name`, the state [`super::seat_from_roster`] reads.
fn roster_named(name: &str, guid: u64) -> Roster {
    let row = benilla_protocol::Character {
        guid,
        name: name.into(),
        race: 1,  // Human → Alliance
        class: 1, // Warrior
        gender: 0,
        level: 60,
        skin: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
        zone: 0,
        map: 0,
        position: benilla_protocol::wire::Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        flags: 0,
        equipment: [benilla_protocol::CharEnumItem::default(); 19],
        pet_display_id: 0,
        pet_level: 0,
        pet_family: 0,
    };
    Roster::with_pending_pick(vec![row], guid)
}

/// A hermetic state folder holding the probe addon, and the guards that point the client at it;
/// every guard must outlive the world.
fn hermetic_probe(tag: &str) -> (std::path::PathBuf, EnvGuard, EnvGuard) {
    hermetic_addon(tag, "SwitchProbe", PROBE_TOC, PROBE_LUA)
}

/// [`hermetic_probe`] for any one-file addon.
fn hermetic_addon(
    tag: &str,
    name: &str,
    toc: &str,
    lua: &str,
) -> (std::path::PathBuf, EnvGuard, EnvGuard) {
    // The pid keeps concurrent test binaries out of each other's tree.
    let tmp =
        std::env::temp_dir().join(format!("benilla-world-entry-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let home = tmp.join("benilla-config");
    let dir = home.join("AddOns").join(name);
    std::fs::create_dir_all(&dir).expect("probe addon dir");
    std::fs::write(dir.join(format!("{name}.toc")), toc).expect("probe toc");
    std::fs::write(dir.join(format!("{name}.lua")), lua).expect("probe lua");
    let capture = EnvGuard::unset("WOW_CAPTURE");
    let benilla_home = EnvGuard::set("BENILLA_HOME", home.to_str().expect("utf-8 temp path"));
    (tmp, capture, benilla_home)
}

/// The world after `Startup` ([`super::setup_script`]): a VM with only the font registry.
fn booted_world() -> World {
    let mut world = World::new();
    world.init_resource::<super::AddOnIdentity>();
    world.init_resource::<crate::minimap::MinimapZoom>();
    world.init_resource::<super::ReloadUiPending>();
    super::setup_script(&mut world);
    world
}

/// Queue and run a `ReloadUI()` as the app does; [`super::run_pending_reload`] checks the client
/// state, so the caller gives it.
fn reload(world: &mut World, state: crate::char_select::ClientState) {
    world.insert_resource(State::new(state));
    world.resource_mut::<super::ReloadUiPending>().0 = true;
    super::run_pending_reload(world);
}

/// One login as the app drives it: the roster carries the pick, then the world-entry edge runs.
fn log_in_as(world: &mut World, name: &str, guid: u64) {
    world.insert_resource(roster_named(name, guid));
    super::load_ingame_ui_on_world_entry(world);
}

/// What the probe addon captured at file scope this session.
fn probe_saw(world: &World) -> Option<String> {
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .and_then(|s| s.eval::<Option<String>>("SwitchProbeFileScope").ok())
        .flatten()
}

/// Is the named frame present in the live VM?
fn frame_exists(world: &World, name: &str) -> bool {
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .and_then(|s| s.eval::<bool>(&format!("return {name} ~= nil")).ok())
        .unwrap_or(false)
}

/// How many times the probe's file scope ran in the live VM.
fn probe_loads(world: &World) -> u32 {
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .and_then(|s| s.eval::<Option<u32>>("SwitchProbeLoads").ok())
        .flatten()
        .unwrap_or(0)
}

/// The second login's addon file scope runs in a fresh VM.
#[test]
fn the_second_login_runs_addon_file_scope_under_the_second_character() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("switch");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Onehunter"),
        "the first login's addon file scope reads the first character"
    );

    super::end_ui_session(&mut world);
    log_in_as(&mut world, "Onewarrior", 2);

    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Onewarrior"),
        "the second login's addon file scope must read the SECOND character — this is the \
         director's \"always Onewarrior\" report, from the other side"
    );
    assert_eq!(
        probe_loads(&world),
        1,
        "the second session is a FRESH VM, not the first one loaded twice — a second load stacked \
         onto the live state would count 2 and would have two of every frame"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Saved variables are written under the identity of the character logged in.
#[test]
fn the_addon_identity_follows_the_character() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("identity");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    let first = world.resource::<super::AddOnIdentity>().0.clone();
    super::end_ui_session(&mut world);
    log_in_as(&mut world, "Onewarrior", 2);
    let second = world.resource::<super::AddOnIdentity>().0.clone();

    assert_ne!(
        first, second,
        "the enable-state / saved-variables identity is re-resolved per login"
    );
    assert_eq!(
        second.as_ref().map(|(_, c)| c.as_str()),
        Some("Onewarrior"),
        "and it names the character actually logged in"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A quit after a logout runs [`super::shutdown_ui_state`] again, against a boot VM, and writes
/// nothing: `ui_saved::save`, `save_enable_state` and `save_addon_variables` each refuse an empty
/// source. The reference guards the case explicitly (`0x401ee0`, its `ds:0x882734` test).
#[test]
fn quitting_from_the_character_screen_does_not_blank_the_session_it_wrote() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("quit");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    super::end_ui_session(&mut world);

    let saved = crate::local_state::addon_saved_character_dir("Realm", "Onehunter")
        .expect("a hermetic home resolves the per-character saved dir")
        .join("SwitchProbe.lua");
    let after_logout = std::fs::read_to_string(&saved).expect("the logout wrote the addon's file");
    assert!(
        after_logout.contains("Onehunter"),
        "…and wrote the character it belonged to: {after_logout}"
    );

    // Quit: `shutdown_on_exit`'s body, against the boot VM the logout left.
    let identity = world.resource::<super::AddOnIdentity>().0.clone();
    let mut script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("a boot VM is live at the character screen");
    // `false`: at the character screen there is no active player.
    super::shutdown_ui_state(&mut script, identity.as_ref(), false);

    assert_eq!(
        std::fs::read_to_string(&saved).ok().as_deref(),
        Some(after_logout.as_str()),
        "the quit pass wrote nothing — the session's file is byte-identical"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The latch that makes a `/logout` fire `PLAYER_LEAVING_WORLD`, armed by the entry load as the
/// reference's world-enter cascade `0x4908c0` arms it, which a fresh login enters from the
/// player's create (`0x5deb60`) and a `/reload` from `0x490168`. A player-object check would read
/// false there: the logout despawns our avatar in the drain before `OnExit(InWorld)` runs.
#[test]
fn the_login_arms_the_world_latch_and_the_logout_spends_it() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("latch");
    let mut world = booted_world();
    // `booted_world` is not the whole `UiScriptPlugin`, so the latch is declared here.
    world.init_resource::<super::LeavingWorldArmed>();

    assert!(
        !world.resource::<super::LeavingWorldArmed>().is_armed(),
        "the character screen is not a world — nothing has armed the latch yet"
    );

    log_in_as(&mut world, "Onehunter", 1);
    assert!(
        world.resource::<super::LeavingWorldArmed>().is_armed(),
        "the entry UI load must arm the latch, or no logout ever fires PLAYER_LEAVING_WORLD"
    );

    super::end_ui_session(&mut world);
    assert!(
        !world.resource::<super::LeavingWorldArmed>().is_armed(),
        "the shutdown tail must SPEND it — an unspent latch lets the following quit fire again"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The reference's one guard in the shutdown tail: `0x490bd0` tests the active player's GUID pair
/// (`0x490bee call 0x468550`, `0x490bf3 or eax,edx`) and with none jumps (`0x490bf5 je 0x490c25`)
/// past only the `PLAYER_LEAVING_WORLD` fire, `0x490c20 call 0x490a80`; in-world roots fire both.
#[test]
fn a_quit_from_the_character_screen_fires_logout_without_leaving_world() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("quitsplit");
    let mut world = booted_world();
    let mut script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("a boot VM is live at the character screen");

    script
        .run(
            r#"
            SeenLeaving = 0 SeenLogout = 0
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_LEAVING_WORLD")
            f:RegisterEvent("PLAYER_LOGOUT")
            f:SetScript("OnEvent", function()
                if event == "PLAYER_LEAVING_WORLD" then
                    SeenLeaving = SeenLeaving + 1
                else
                    SeenLogout = SeenLogout + 1
                end
            end)
            "#,
        )
        .expect("the probe frame registers");

    super::shutdown_ui_state(&mut script, None, false);
    assert_eq!(
        script.eval::<i64>("return SeenLeaving").unwrap(),
        0,
        "the glue-screen quit fired PLAYER_LEAVING_WORLD — the reference's 0x490bf5 skips it"
    );
    assert_eq!(
        script.eval::<i64>("return SeenLogout").unwrap(),
        1,
        "…and it must still fire PLAYER_LOGOUT, which is the whole of the taken side"
    );

    super::shutdown_ui_state(&mut script, None, true);
    assert_eq!(
        script.eval::<i64>("return SeenLeaving").unwrap(),
        1,
        "an in-world root must fire PLAYER_LEAVING_WORLD — the control for the assertion above"
    );
    assert_eq!(
        script.eval::<i64>("return SeenLogout").unwrap(),
        2,
        "…and PLAYER_LOGOUT on every root, guarded or not"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The character screen is native: the old session's frame tree must not survive behind it.
#[test]
fn logging_out_leaves_no_in_game_frames_behind() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("teardown");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    assert!(
        probe_saw(&world).is_some(),
        "the session under test actually loaded"
    );

    super::end_ui_session(&mut world);

    assert_eq!(
        probe_saw(&world),
        None,
        "the session's Lua state is gone at the character screen"
    );
    assert!(
        !frame_exists(&world, "PlayerFrame"),
        "and so is the in-game frame tree — 1051 measured 193 quads' worth of it surviving \
         behind the glue screen's opaque node"
    );
    assert!(
        world
            .get_non_send_resource::<benilla_ui::script::UiScript>()
            .is_some(),
        "a boot VM stays: the character screen's text still bakes off the shared font registry"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ───────────────────────────────── ReloadUI ─────────────────────────────────

/// `ReloadUI()` is the reference's teardown and rebuild pair (`0x495664`/`0x495669`): the logout
/// and login edges, run without leaving the world.
#[test]
fn reload_ui_is_a_fresh_login_in_place() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("reload");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    let first_session = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("in-world VM")
        .session();

    reload(&mut world, crate::char_select::ClientState::InWorld);

    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Onehunter"),
        "the reloaded session ran the addon's file scope again, under the same character"
    );
    assert_eq!(
        probe_loads(&world),
        1,
        "…in a FRESH VM — a reload stacked onto the live state would count 2"
    );
    assert_ne!(
        world
            .get_non_send_resource::<benilla_ui::script::UiScript>()
            .expect("in-world VM")
            .session(),
        first_session,
        "the VM identity changed, so every VmMemo about the old session expires (1290)"
    );
    assert!(
        frame_exists(&world, "PlayerFrame"),
        "and the in-game UI is back up"
    );
    assert!(
        !world.resource::<super::ReloadUiPending>().0,
        "the request was consumed"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// `DisableAddOn` only marks the live registry; the reload's shutdown tail writes `AddOns.txt`
/// (the reference's last write before the state dies) and the rebuild reads it back.
#[test]
fn a_disable_staged_in_the_session_applies_at_the_reload() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("disable");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    assert!(
        probe_saw(&world).is_some(),
        "the probe loaded to begin with"
    );
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("in-world VM")
        .run("DisableAddOn('SwitchProbe')")
        .expect("DisableAddOn");
    assert!(
        probe_saw(&world).is_some(),
        "disabling alone changes nothing in the live session — there is no unload (1197)"
    );

    reload(&mut world, crate::char_select::ClientState::InWorld);

    assert_eq!(
        probe_saw(&world),
        None,
        "after the reload the disabled addon's file scope never ran"
    );
    let enable_file = super::addons::enable_state_path(Some(&("Realm".into(), "Onehunter".into())))
        .expect("hermetic enable path");
    let text = std::fs::read_to_string(&enable_file).expect("the teardown wrote AddOns.txt");
    assert!(
        text.contains("SwitchProbe: disabled"),
        "…because the choice reached disk on the way down: {text}"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The teardown writes saved variables after `PLAYER_LOGOUT`, and the rebuild restores them after
/// file scope, so the saved value wins (the reference's `AddOn_Load` order).
#[test]
fn saved_variables_round_trip_through_a_reload() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("saved");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("in-world VM")
        .run("SwitchProbeDB.mark = 41")
        .expect("mutate the saved table");

    reload(&mut world, crate::char_select::ClientState::InWorld);

    let mark = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("in-world VM")
        .eval::<Option<u32>>("return SwitchProbeDB and SwitchProbeDB.mark")
        .expect("read back")
        .unwrap_or(0);
    assert_eq!(
        mark, 41,
        "the reload wrote the table down and the rebuild restored it OVER the file-scope default"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Dropped, not deferred: the reference's gate (`0x494a50(0xa)`) refuses a reload at the glue.
#[test]
fn reload_outside_the_world_is_dropped() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("glue-reload");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    super::end_ui_session(&mut world);

    reload(&mut world, crate::char_select::ClientState::CharSelect);

    assert_eq!(
        probe_saw(&world),
        None,
        "no addon loaded — the request was dropped, not run against the glue"
    );
    assert!(
        !frame_exists(&world, "PlayerFrame"),
        "and no in-game UI appeared behind the character screen"
    );
    assert!(
        !world.resource::<super::ReloadUiPending>().0,
        "the stale request is consumed, so it cannot fire on the NEXT login's first frame"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A file-scope raise drops only its addon, and the error reaches the stock `ScriptErrors` dialog.
#[test]
fn an_addon_error_while_entering_world_reports_on_screen_and_the_sibling_loads() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("addon-error");
    // A second addon, first alphabetically, that dies at file scope.
    let dir = tmp.join("benilla-config/AddOns/AaBroken");
    std::fs::create_dir_all(&dir).expect("broken addon dir");
    std::fs::write(dir.join("AaBroken.toc"), "## Interface: 11200\nboom.lua\n").expect("toc");
    std::fs::write(dir.join("boom.lua"), "error('file-scope boom')\n").expect("lua");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Onehunter"),
        "the addon AFTER the broken one still loads — a neighbour's error drops only itself"
    );

    // The app's per-frame drain runs the dispatch; the test runs the same call.
    let mut script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("VM");
    script.dispatch_script_errors_to_handler();
    assert!(
        script
            .eval::<bool>("return ScriptErrors:IsVisible()")
            .expect("ScriptErrors exists — BasicControls loaded"),
        "the ScriptErrors dialog is on screen — `seterrorhandler(_ERRORMESSAGE)` is installed \
         and the engine dispatched the caught error to it"
    );
    let shown: String = script
        .eval::<Option<String>>("return ScriptErrors_Message:GetText()")
        .expect("eval")
        .unwrap_or_default();
    assert!(
        shown.contains("file-scope boom"),
        "the dialog names the actual error, got: {shown:?}"
    );
    drop(script);
    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The load's instruction budget fails the looping addon; the test finishing is the claim.
#[test]
fn a_looping_addon_cannot_freeze_world_entry() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("addon-loop");
    let dir = tmp.join("benilla-config/AddOns/AaSpin");
    std::fs::create_dir_all(&dir).expect("spin addon dir");
    std::fs::write(dir.join("AaSpin.toc"), "## Interface: 11200\nspin.lua\n").expect("toc");
    std::fs::write(dir.join("spin.lua"), "while true do end\n").expect("lua");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);

    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Onehunter"),
        "the addon after the spinner still loads — the budget failed one addon, not the entry"
    );
    // The budget raise reaches the handler queue, so the dialog names the loop.
    let mut script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("VM");
    script.dispatch_script_errors_to_handler();
    let shown: String = script
        .eval::<Option<String>>("return ScriptErrors_Message:GetText()")
        .expect("eval")
        .unwrap_or_default();
    assert!(
        shown.contains("instruction budget exhausted"),
        "the dialog names the runaway loop with the budget's distinctive message, got: {shown:?}"
    );

    drop(script);
    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ─────────────────── The deferred entry load ───────────────────

/// With the loading cover up, the armed load waits [`super::lifecycle::run_pending_entry_load`]'s
/// covered frames, so its burst runs behind a presented cover.
#[test]
fn the_entry_load_waits_for_the_cover_to_present() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("defer");
    let mut world = booted_world();
    world.insert_resource(State::new(crate::char_select::ClientState::InWorld));
    world.insert_resource(crate::loading_screen::LoadingScreen::test_covering());
    world.insert_resource(crate::loading_screen::EntryCover::default());
    world.insert_resource(roster_named("Onehunter", 1));
    world.insert_resource(super::PendingEntryUiLoad);

    // Covered frames 1 and 2: the cover has not presented yet.
    for frame in 1..=2 {
        world
            .resource_mut::<crate::loading_screen::EntryCover>()
            .tick(true);
        super::lifecycle::run_pending_entry_load(&mut world);
        assert_eq!(
            probe_saw(&world),
            None,
            "covered frame {frame}: the burst must wait for the cover to reach the glass"
        );
    }
    // Covered frame 3: two cover renders have committed, so the load runs.
    world
        .resource_mut::<crate::loading_screen::EntryCover>()
        .tick(true);
    super::lifecycle::run_pending_entry_load(&mut world);
    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Onehunter"),
        "the third covered frame pays the load, behind a presented cover"
    );
    assert!(
        world.get_resource::<super::PendingEntryUiLoad>().is_none(),
        "the latch is consumed — the loading screen's clear condition reads its absence"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// With no cover (a capture booting straight `InWorld`, or the screen's assets missing) the armed
/// load runs on the first frame, where counting covered frames would never load the UI.
#[test]
fn no_cover_means_the_entry_load_runs_at_once() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("nocover");
    let mut world = booted_world();
    world.insert_resource(State::new(crate::char_select::ClientState::InWorld));
    world.insert_resource(crate::loading_screen::LoadingScreen::default());
    world.insert_resource(roster_named("Onehunter", 1));
    world.insert_resource(super::PendingEntryUiLoad);

    super::lifecycle::run_pending_entry_load(&mut world);
    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Onehunter"),
        "uncovered: the load runs immediately"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// An exit inside the deferral window drops the armed load and writes nothing: a shutdown tail
/// against the boot VM would compose every saved file from nothing.
#[test]
fn leaving_inside_the_deferral_window_drops_the_load_and_writes_nothing() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("dropped");
    let mut world = booted_world();
    world.insert_resource(State::new(crate::char_select::ClientState::InWorld));
    world.insert_resource(crate::loading_screen::LoadingScreen::test_covering());
    world.insert_resource(roster_named("Onehunter", 1));
    world.insert_resource(crate::loading_screen::EntryCover::default());
    world.insert_resource(super::PendingEntryUiLoad);

    world
        .resource_mut::<crate::loading_screen::EntryCover>()
        .tick(true);
    super::lifecycle::run_pending_entry_load(&mut world); // covered frame 1: still pending
    super::end_ui_session(&mut world);

    assert_eq!(probe_saw(&world), None, "no UI ever loaded");
    assert!(
        world.get_resource::<super::PendingEntryUiLoad>().is_none(),
        "the latch died with the session — it must not fire on the glue"
    );
    let flat = crate::local_state::saved_variables_path()
        .expect("hermetic home resolves the flat saved path");
    assert!(
        !flat.exists(),
        "the shutdown tail was skipped — a UI-less VM must not write saved variables"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ───────────── The login one-shots wait for the in-game UI ─────────────

/// The unit feed's login one-shots (`PLAYER_ENTERING_WORLD`, the first `PLAYER_XP_UPDATE` and
/// `UPDATE_EXHAUSTION`) wait for the in-game UI instead of reaching a VM with no frames.
#[test]
fn the_login_one_shots_wait_for_the_in_game_ui() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("oneshot");

    let mut app = App::new();
    app.add_plugins(crate::ui_unit::UiUnitPlugin);
    app.init_resource::<super::AddOnIdentity>();
    app.init_resource::<crate::minimap::MinimapZoom>();
    app.init_resource::<super::ReloadUiPending>();
    app.init_resource::<crate::target::Selection>();
    app.init_resource::<crate::names::NameCache>();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.init_resource::<crate::net::Reputations>();
    app.init_resource::<crate::ui_party::GroupState>();
    app.init_resource::<crate::ui_chat::ChatLog>();
    app.init_resource::<crate::ui_guild::GuildState>();
    app.add_message::<crate::creature_anim::SwingImpact>();
    super::setup_script(app.world_mut());

    // A frame registered as FrameXML registers, in the VM that exists before the entry load.
    app.world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run(
            "EnteringWorldSeen = 0 \
             local f = CreateFrame(\"Frame\") \
             f:RegisterEvent(\"PLAYER_ENTERING_WORLD\") \
             f:SetScript(\"OnEvent\", function() \
                 EnteringWorldSeen = EnteringWorldSeen + 1 end)",
        )
        .expect("probe frame");

    // Our own descriptor has landed, the feed's condition for the one-shots.
    app.world_mut().spawn((
        crate::net::SelfPlayer,
        crate::net::Guid(1),
        crate::net::ObjectStore(
            benilla_protocol::messages::ObjectFields::from_pairs(&[])
                .into_created(benilla_protocol::messages::ObjectType::Player),
        ),
    ));

    let seen = |app: &App| -> i64 {
        app.world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval::<i64>("return EnteringWorldSeen")
            .expect("probe global")
    };

    // The drain's own frame: the wire is in-world, but the state says glue and no load is armed
    // until `OnEnter(InWorld)` next frame.
    app.insert_resource(State::new(crate::char_select::ClientState::CharSelect));
    app.update();
    assert_eq!(
        seen(&app),
        0,
        "the drain's own frame: in-world wire, a boot VM, and no latch yet"
    );

    // The transition ran and the load is still owed: three frames inside the deferral window.
    app.insert_resource(State::new(crate::char_select::ClientState::InWorld));
    app.insert_resource(super::PendingEntryUiLoad);
    for frame in 1..=3 {
        app.update();
        assert_eq!(
            seen(&app),
            0,
            "frame {frame}: the feed must not spend PLAYER_ENTERING_WORLD on a UI-less VM"
        );
    }

    // The entry load has run: the next feed delivers the one-shots.
    app.world_mut()
        .remove_resource::<super::PendingEntryUiLoad>();
    app.update();
    assert_eq!(
        seen(&app),
        1,
        "with the UI up the one-shot fires — once, on the first feed after the load"
    );
    app.update();
    assert_eq!(seen(&app), 1, "and exactly once per world entry");

    drop(app);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// An addon that fails to load without raising (a `.toc` naming a file the package lacks) is kept
/// in the log, readable from Lua by the error window, and announced in chat.
#[test]
fn an_addon_that_fails_to_load_without_raising_is_readable_in_the_error_log() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("addon-missing-file");
    // First alphabetically, so the probe behind it shows this class drops only itself.
    let dir = tmp.join("benilla-config/AddOns/AaMissing");
    std::fs::create_dir_all(&dir).expect("addon dir");
    std::fs::write(
        dir.join("AaMissing.toc"),
        "## Interface: 11200\nBossnames\\BossNames.xml\n",
    )
    .expect("toc");
    // …and no such file is written.
    let mut world = booted_world();
    world.init_resource::<crate::ui_chat::ChatLog>();

    log_in_as(&mut world, "Onehunter", 1);

    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Onehunter"),
        "the addon after the broken one still loads"
    );

    let script = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("VM");

    // 1. Retained, tagged as a load failure.
    let rows = script.diagnostics();
    let row = rows
        .iter()
        .find(|d| d.message.contains("AaMissing"))
        .unwrap_or_else(|| panic!("the missing file is in the log; got {rows:#?}"));
    assert_eq!(
        row.kind,
        benilla_ui::script::diagnostics::DiagnosticKind::Load
    );
    assert!(
        row.message.contains("not found"),
        "the row says what went wrong, verbatim as the terminal line: {:?}",
        row.message
    );

    // 2. Readable from Lua through the window's own reads.
    let count: i64 = script
        .eval("local shown = BenillaGetNumScriptErrors() return shown")
        .expect("BenillaGetNumScriptErrors is installed");
    assert!(count >= 1, "the window's own read sees it");
    let seen: String = script
        .eval(
            "local text = '' \
             for i = 1, BenillaGetNumScriptErrors() do \
                local seq, kind, message = BenillaGetScriptErrorInfo(i) \
                if kind == 'load' then text = message end \
             end \
             return text",
        )
        .expect("BenillaGetScriptErrorInfo is installed");
    assert!(
        seen.contains("AaMissing"),
        "the window walks the log and finds it: {seen:?}"
    );

    assert!(
        script
            .eval::<bool>("return BenillaScriptLogFrame ~= nil")
            .expect("eval"),
        "ScriptLogFrame.xml loaded and built the window"
    );

    // The repaint runs over a real row, so a nil global anywhere on its path raises here.
    script
        .eval::<()>("BenillaScriptLog_Update() return nil")
        .expect("the window repaints over a real log without raising");
    // Some row shows it, not necessarily row 1: the log also holds the warnings a world entry
    // raises before any addon loads.
    let row_labels: Vec<String> = (1..=13)
        .filter_map(|i| {
            script
                .eval::<Option<String>>(&format!("return BenillaScriptLogRow{i}Label:GetText()"))
                .expect("eval")
        })
        .collect();
    assert!(
        row_labels.iter().any(|l| l.contains("AaMissing")),
        "a row shows the failure, trimmed to the row's width: {row_labels:?}"
    );
    let summary: String = script
        .eval::<Option<String>>("return BenillaScriptLogSummary:GetText()")
        .expect("eval")
        .unwrap_or_default();
    assert!(
        summary.contains("problem"),
        "the summary line counted them: {summary:?}"
    );

    // 3. No dialog, since nothing raised: the reference answers an absent or unparseable file
    // with a log line alone.
    assert!(
        !script
            .eval::<bool>("return ScriptErrors:IsVisible()")
            .expect("ScriptErrors exists"),
        "a non-raising load failure must not pop the red dialog — that would put non-errors \
         through `_ERRORMESSAGE` and through every addon handler that replaces it"
    );

    // 4. Deviation: a chat line tells the player to look, where the reference stays silent,
    // because an unannounced log goes unread.
    assert_eq!(
        world.resource::<crate::ui_chat::ChatLog>().pending_len(),
        1,
        "world entry queued the 'N addon load failures — type /errors' line"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// An error raised every frame is one log row with a count, not a flood of rows.
#[test]
fn a_repeating_error_is_one_row_with_a_count_not_a_flood() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("addon-repeat");
    let mut world = booted_world();
    log_in_as(&mut world, "Onehunter", 1);

    let mut script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("VM");
    let before = script.diagnostics().len();
    // The same failure through the engine's catch path: a slash command whose body raises.
    script
        .run("SlashCmdList = SlashCmdList or {} SLASH_B293BOOM1 = '/b293boom' SlashCmdList['B293BOOM'] = function() error('every frame') end")
        .expect("register");
    for _ in 0..500 {
        script.run_slash_command("b293boom", "");
    }

    let rows = script.diagnostics();
    assert_eq!(
        rows.len(),
        before + 1,
        "500 identical raises are ONE new row: {rows:#?}"
    );
    let row = rows.last().expect("a row");
    assert_eq!(row.count, 500, "the count is where the 500 went");
    assert_eq!(
        row.kind,
        benilla_ui::script::diagnostics::DiagnosticKind::Error,
        "code ran and raised — the addon is loaded, unlike a Load row"
    );

    drop(script);
    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ── The layout cache is written by the shutdown tail ───────────────────────────────
// A `/reload` never leaves `InWorld`, so only a saver in the shutdown tail sees every root.

/// A window the player has placed, made as a drag makes one: `SetUserPlaced` refuses a frame that
/// is neither movable nor resizable. Parentless, so it anchors to the screen root (the file's `-`
/// target). Both flags: the cache applies position only to a movable frame, size to a resizable.
fn place_a_window(world: &mut World) {
    world
        .get_non_send_resource_mut::<benilla_ui::script::UiScript>()
        .expect("a VM to place a window in")
        .run(
            "local f = CreateFrame(\"Frame\", \"B353Probe\") \
             f:SetWidth(413) f:SetHeight(147) \
             f:SetPoint(\"BOTTOMLEFT\", 61, 29) \
             f:SetMovable(true) f:SetResizable(true) f:SetUserPlaced(true)",
        )
        .expect("place the probe window");
}

/// The layout cache the shutdown left for this character.
fn layout_cache(character: &str) -> Option<String> {
    let path = crate::local_state::layout_character_path("Realm", character)?;
    std::fs::read_to_string(path).ok()
}

/// What a saved window's row has to say for the player to get it back.
fn assert_probe_row(text: &str) {
    for want in [
        "Frame: B353Probe",
        "W: 413",
        "H: 147",
        "Point: BOTTOMLEFT - BOTTOMLEFT 61 29",
    ] {
        assert!(
            text.contains(want),
            "the saved row is missing `{want}`:\n{text}"
        );
    }
}

#[test]
fn a_placed_window_is_written_at_logout() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("layout-logout");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    place_a_window(&mut world);
    assert!(
        layout_cache("Onehunter").is_none(),
        "nothing is written while the session is running"
    );

    super::end_ui_session(&mut world);
    assert_probe_row(&layout_cache("Onehunter").expect("the logout wrote the layout cache"));

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The reload root never leaves `InWorld`, so an `OnExit(InWorld)` saver would never see it.
#[test]
fn a_placed_window_is_written_at_reload() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("layout-reload");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    place_a_window(&mut world);
    reload(&mut world, crate::char_select::ClientState::InWorld);

    assert_probe_row(&layout_cache("Onehunter").expect(
        "the reload wrote the layout cache — it runs the shutdown tail without a state edge",
    ));

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The cache is per character, written for the one the UI loaded under.
#[test]
fn each_character_gets_its_own_layout_cache() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("layout-two-chars");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    place_a_window(&mut world);
    super::end_ui_session(&mut world);

    log_in_as(&mut world, "Onewarrior", 2);
    super::end_ui_session(&mut world);

    assert_probe_row(&layout_cache("Onehunter").expect("the first character's file"));
    let second = layout_cache("Onewarrior").expect("the second character's file");
    assert!(
        !second.contains("B353Probe"),
        "a character who placed nothing must not inherit another's window:\n{second}"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The probe window as FrameXML authors it, the fresh tree a relog meets: not user-placed, with the
/// authored `movable`/`resizable` pair the cache's apply reads off the live frame.
fn author_a_window(world: &mut World) {
    world
        .get_non_send_resource_mut::<benilla_ui::script::UiScript>()
        .expect("a VM to author a window in")
        .run(
            "local f = CreateFrame(\"Frame\", \"B353Probe\") \
             f:SetWidth(100) f:SetHeight(100) \
             f:SetPoint(\"BOTTOMLEFT\", 0, 0) \
             f:SetMovable(true) f:SetResizable(true)",
        )
        .expect("author the probe window");
}

/// The probe window's live geometry, as the player sees it.
fn window_geometry(world: &World) -> (f32, f32, String, f32, f32) {
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("a VM")
        .eval::<(f32, f32, String, f32, f32)>(
            "local p, _, _, x, y = B353Probe:GetPoint(1) \
             return B353Probe:GetWidth(), B353Probe:GetHeight(), p, x, y",
        )
        .expect("read the probe window back")
}

/// Place a window, `/reload`, meet the authored tree, and the loader seats the saved geometry back.
/// [`crate::ui_layout::load_layout`] runs directly: this harness drives edges, not schedules.
#[test]
fn a_placed_window_comes_back_after_a_reload() {
    use bevy::ecs::system::RunSystemOnce;

    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("layout-roundtrip");
    let mut world = booted_world();
    world.init_resource::<crate::ui_layout::LayoutFile>();

    log_in_as(&mut world, "Onehunter", 1);
    place_a_window(&mut world);
    let placed = window_geometry(&world);

    reload(&mut world, crate::char_select::ClientState::InWorld);
    author_a_window(&mut world);
    assert_ne!(
        window_geometry(&world),
        placed,
        "the rebuilt tree starts on its authored anchors — otherwise this proves nothing"
    );

    world
        .run_system_once(crate::ui_layout::load_layout)
        .expect("the layout loader ran");
    assert_eq!(
        window_geometry(&world),
        placed,
        "the window the player placed is back where they left it"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// With no addons every warning is a gap of ours, so the allowlist names each one, never a count:
/// a new gap fails here, and closing one deletes its line.
#[test]
fn a_clean_world_entry_raises_only_the_warnings_we_have_named() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_tmp, _c, _h) = hermetic_probe("clean-entry-warnings");
    let mut world = booted_world();
    world.init_resource::<crate::ui_chat::ChatLog>();
    log_in_as(&mut world, "Onehunter", 1);
    let script = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("VM");

    // Deviation: the `OnInputLanguageChanged` script slot (`ChatFrame.xml:121`, the IME language
    // indicator) is refused, because benilla has no IME to fire it.
    // Deviation: `gxRefresh`, read by stock `OptionsFrameRefreshDropDown_OnLoad`
    // (`OptionsFrame.lua:300`), is not registered, because no target offers the exclusive mode-set
    // a refresh rate needs; `GetRefreshRates` answers the reference's no-rates sentinel.
    const KNOWN: [&str; 2] = ["OnInputLanguageChanged", "unknown CVar 'gxRefresh'"];

    let unexpected: Vec<String> = script
        .diagnostics()
        .into_iter()
        .filter(|d| d.kind == benilla_ui::script::diagnostics::DiagnosticKind::Warning)
        .map(|d| d.message)
        .filter(|m| !KNOWN.iter().any(|k| m.contains(k)))
        .collect();
    assert!(
        unexpected.is_empty(),
        "a stock world entry warned about something new — fix it or name it here: {unexpected:#?}"
    );
}

// ─────────────────── The predicate every in-world feed runs on ───────────────────

/// The UI is up only in `InWorld` with no [`super::lifecycle::PendingEntryUiLoad`]: the latch is
/// armed at `OnEnter(InWorld)`, a frame after `apply_net_updates` drains `Connected` and the login
/// burst, so for that one frame the wire is in-world while the VM is still the boot one.
#[test]
fn the_ui_is_not_up_in_the_frame_between_the_wire_and_the_state() {
    let mut world = World::new();

    // The drain's frame: no load owed, and the state still says glue until next frame's transition.
    world.insert_resource(State::new(crate::char_select::ClientState::CharSelect));
    assert!(
        !run_ingame_ui_up(&mut world),
        "the wire is in-world a frame before the state is — the boot VM must stay out of reach"
    );

    // The transition ran: `OnEnter` parked the VM and armed the latch.
    world.insert_resource(State::new(crate::char_select::ClientState::InWorld));
    world.insert_resource(super::PendingEntryUiLoad);
    assert!(
        !run_ingame_ui_up(&mut world),
        "the deferral window — 1978's parked VM, and nothing to receive an event"
    );

    // The deferred load ran: the frame tree exists.
    world.remove_resource::<super::PendingEntryUiLoad>();
    assert!(
        run_ingame_ui_up(&mut world),
        "in the world with the in-game UI up — the first frame a feed may push"
    );

    // Leaving drops it again, before `end_ui_session`'s fresh boot VM can be fed anything.
    world.insert_resource(State::new(crate::char_select::ClientState::CharSelect));
    assert!(!run_ingame_ui_up(&mut world), "the world is gone with it");
}

fn run_ingame_ui_up(world: &mut World) -> bool {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(super::lifecycle::ingame_ui_up)
        .expect("the condition runs")
}

/// `GetMapContinents`/`GetMapZones` are static DBC data, and Astrolabe (under Questie and
/// Cartographer) builds its continent and zone table from them at file scope; empty, every icon it
/// places indexes a nil zone. The catalog is planted: this checks the order, not the build.
#[test]
fn an_addon_reads_the_map_catalog_at_file_scope() {
    const MAP_PROBE_TOC: &str = "\
## Interface: 11200
MapProbe.lua
";
    // Astrolabe's two calls, in its idiom: a table constructor around a multi-return.
    const MAP_PROBE_LUA: &str = "\
MapProbeContinents = table.getn({ GetMapContinents() })
MapProbeZones = table.getn({ GetMapZones(1) })
";
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_addon("mapcatalog", "MapProbe", MAP_PROBE_TOC, MAP_PROBE_LUA);
    let mut world = booted_world();
    world.insert_resource(crate::ui_world_map::WorldMapCatalog(vec![
        benilla_ui::script::WorldMapContinentView {
            name: "Kalimdor".into(),
            zones: vec![
                benilla_ui::script::WorldMapZoneView {
                    name: "Durotar".into(),
                    ..Default::default()
                },
                benilla_ui::script::WorldMapZoneView {
                    name: "Mulgore".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    ]));

    log_in_as(&mut world, "Onewarrior", 1);

    let read = |expr: &str| {
        world
            .get_non_send_resource::<benilla_ui::script::UiScript>()
            .and_then(|s| s.eval::<Option<u32>>(expr).ok())
            .flatten()
    };
    assert_eq!(
        read("MapProbeContinents"),
        Some(1),
        "an addon's file scope must see the continent list — it is DBC data the reference has held \
         since load, not a feed that arrives later"
    );
    assert_eq!(
        read("MapProbeZones"),
        Some(2),
        "…and the zone list with it: this is the one Astrolabe builds its whole table from"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Stock `ActionButton_OnLoad` paints its hotkey from `GetBindingKey` at load, and an addon that
/// rebinds a stock command at file scope needs the command to exist.
#[test]
fn an_addon_reads_the_keybinding_table_at_file_scope() {
    const TOC: &str = "\
## Interface: 11200
BindProbe.lua
";
    const LUA: &str = "\
BindProbeKey = GetBindingKey(\"TOGGLEWORLDMAP\")
BindProbeSet = SetBinding(\"J\", \"TOGGLEWORLDMAP\")
";
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_addon("bindprobe", "BindProbe", TOC, LUA);
    // No `BindingFiles` planted: the seed takes it as optional.
    let mut world = booted_world();

    log_in_as(&mut world, "Onewarrior", 1);

    let script = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("VM");
    assert_eq!(
        script
            .eval::<Option<String>>("BindProbeKey")
            .ok()
            .flatten()
            .as_deref(),
        Some("M"),
        "an addon's file scope must read the stock binding — `M` is TOGGLEWORLDMAP's own default"
    );
    assert_eq!(
        script.eval::<Option<u32>>("BindProbeSet").ok().flatten(),
        Some(1),
        "…and a rebind of a stock command must take, not answer the empty table's silent nil"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// With an empty channel catalog, `JoinChannelByName("General")` would join a custom channel of
/// that name on the server. Seeded with no zone, the call matches the built-in row and does
/// nothing, the reference's answer while there is no zone text.
#[test]
fn an_addon_that_joins_general_at_file_scope_puts_nothing_on_the_wire() {
    const TOC: &str = "\
## Interface: 11200
JoinProbe.lua
";
    const LUA: &str = "JoinChannelByName(\"General\")\n";
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_addon("joinprobe", "JoinProbe", TOC, LUA);
    let mut world = booted_world();
    world.insert_resource(crate::ui_chat::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            // Row 1 as `ChatChannels.dbc` has it: auto-joined, its `%s` the zone's name.
            benilla_formats::ChatChannelRow {
                id: 1,
                flags: benilla_formats::chat_channel_flags::INITIAL
                    | benilla_formats::chat_channel_flags::ZONE_DEP,
                pattern: "General - %s".into(),
                shortcut: "General".into(),
            },
        ]),
        ..Default::default()
    });

    log_in_as(&mut world, "Onewarrior", 1);

    let queued = world
        .get_non_send_resource_mut::<benilla_ui::script::UiScript>()
        .expect("VM")
        .take_channel_commands();
    assert!(
        queued.is_empty(),
        "a built-in shortcut with no zone text yet is a no-op, not a custom channel on the \
         server — queued {queued:?}"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The entry load seats the real screen size: a fresh VM's is 1024×768 until `tick_script` runs,
/// and stock `WorldMapFrame_OnLoad` sizes `BlackoutWorld` from it once (`WorldMapFrame.lua:19-28`).
#[test]
fn an_addon_reads_the_real_screen_size_at_file_scope() {
    const TOC: &str = "\
## Interface: 11200
ScreenProbe.lua
";
    const LUA: &str = "\
ScreenProbeWidth = GetScreenWidth()
ScreenProbeHeight = GetScreenHeight()
";
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_addon("screenprobe", "ScreenProbe", TOC, LUA);
    let mut world = booted_world();
    // 2560×1440 at UI scale 1: 1440/768 = 1.875, so the VM's screen is 1365.33 × 768 units. The
    // width is the discriminator: the height is 768 under any window.
    world.spawn((
        Window {
            resolution: bevy::window::WindowResolution::new(2560, 1440),
            ..Default::default()
        },
        bevy::window::PrimaryWindow,
    ));

    log_in_as(&mut world, "Onewarrior", 1);

    let script = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("VM");
    let read = |expr: &str| script.eval::<Option<f32>>(expr).ok().flatten();
    let width = read("ScreenProbeWidth").expect("the probe ran at file scope");
    assert!(
        (width - 2560.0 * 768.0 / 1440.0).abs() < 0.01,
        "an addon's file scope must read the REAL screen width in UI units — got {width}, and \
         1024 is the fresh model's default, i.e. the bug"
    );
    assert_eq!(
        read("ScreenProbeHeight"),
        Some(768.0),
        "…and the height is the 768-tall virtual base (decision 0582)"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// [`super::seat_from_roster`]'s `"player"` push is replaced when the descriptor streams in, but
/// `UnitName("player")` reads only the record seeded beside it, which neither a nameless snapshot
/// nor a despawn clears, as the reference never clears `0xc27d88`.
#[test]
fn the_entry_load_seeds_a_record_the_feed_cannot_take_away() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _capture, _home) = hermetic_probe("nameseed");
    let mut world = booted_world();
    log_in_as(&mut world, "Nelprifour", 0x2A);

    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Nelprifour"),
        "addon file scope reads the live character, as it always has (1230)"
    );

    let mut script = world
        .get_non_send_resource_mut::<benilla_ui::script::UiScript>()
        .expect("a VM");
    // The feed's push when the name cache misses our own guid: a nameless snapshot.
    script.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            has_object: true,
            name: None,
            ..Default::default()
        }),
    );
    assert_eq!(
        script
            .eval::<Option<String>>(r#"return UnitName("player")"#)
            .unwrap()
            .as_deref(),
        Some("Nelprifour"),
        "the record answers, so a nameless snapshot is invisible to the verb"
    );
    assert_eq!(
        script
            .eval::<Option<String>>(r#"local _, t = UnitClass("player"); return t"#)
            .unwrap()
            .as_deref(),
        Some("WARRIOR"),
        "…and the same for the other three fields the reference reads off that record (2263)"
    );

    // A logout despawn removes the token altogether.
    script.set_unit("player", None);
    assert_eq!(
        script
            .eval::<Option<String>>(r#"return UnitName("player")"#)
            .unwrap()
            .as_deref(),
        Some("Nelprifour")
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

// ─────────────────── The production load is silent (the UI-load sound bracket) ───────────────────

/// Every kit name the live VM has queued since the last drain.
fn taken_kit_names(world: &mut World) -> Vec<String> {
    world
        .get_non_send_resource_mut::<benilla_ui::script::UiScript>()
        .expect("in-world VM")
        .take_sounds()
        .into_iter()
        .filter_map(|r| match r {
            benilla_ui::script::SoundRequest::KitName(n) => Some(n),
            _ => None,
        })
        .collect()
}

/// The reference's `0x48fbf0`, which login (`0x48f681`) and `/reloadui` (`0x495669`) both call,
/// wraps the TOC walk, the addons and the saved variables in the counted sound suppression
/// (`0x48fbfa` to `0x49016d`); the login cascade `0x4908c0`, which a fresh login runs later from
/// the player's create (`0x5deb60`), brackets itself (`0x4908d5` to `0x490a56`). Stock
/// `TargetFrame_OnHide` plays at load.
#[test]
fn a_login_and_a_reload_load_without_the_lost_target_sound() {
    let _data = benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("load-silent");
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    assert!(
        frame_exists(&world, "TargetFrame"),
        "the stock target frame loaded"
    );
    let at_login = taken_kit_names(&mut world);
    assert!(
        !at_login
            .iter()
            .any(|n| n == "INTERFACESOUND_LOSTTARGETUNIT"),
        "the login load must be silent — the lost-target click was queued: {at_login:?}"
    );

    reload(&mut world, crate::char_select::ClientState::InWorld);
    let at_reload = taken_kit_names(&mut world);
    assert!(
        !at_reload
            .iter()
            .any(|n| n == "INTERFACESOUND_LOSTTARGETUNIT"),
        "the /reload load must be silent — the lost-target click was queued: {at_reload:?}"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}
