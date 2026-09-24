//! **"The selected one is always Onewarrior no matter what char I log into."** — the director's
//! character-switch report, reproduced at the edge that causes it.
//!
//! ## Why this file exists
//!
//! The symptom arrived as a dropdown bug: Bagnon's character menu never moved its checkmark off
//! whoever the session started as. It is not a dropdown bug and it is not Bagnon's. Every addon on
//! the machine reads the live character **once, at file scope** — `local currentPlayer =
//! UnitName("player")` is the corpus idiom, not an idiosyncrasy — and until this landed the file
//! scope ran exactly once per **process**. Logging out to the character screen and back in kept the
//! same VM, so nothing re-read it.
//!
//! The tests here drive the two real edges — [`super::load_ingame_ui_on_world_entry`] and
//! [`super::end_ui_session`] — over a planted addon that captures the name the same way, and assert
//! the second login sees the second character. Reverting the rebuild makes
//! [`the_second_login_runs_addon_file_scope_under_the_second_character`] report `Onehunter`, which
//! is the director's screenshot in one string.
//!
//! Nothing here needs the client install or the addon corpus: the probe addon is written into a
//! hermetic `BENILLA_HOME` by the test itself.

use bevy::prelude::*;

use crate::char_select::Roster;
use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

/// **Bagnon's own idiom**, reduced to the one line that carries the bug: the live character's name,
/// read once while the file runs, and parked where the test can see it.
///
/// `SwitchProbeLoads` counts file-scope runs *within one VM*, so a rebuild resets it to 1 — that
/// number is what tells a re-entry apart from a second load stacked onto the same state.
const PROBE_LUA: &str = "\
local currentPlayer = UnitName(\"player\")
SwitchProbeFileScope = currentPlayer
SwitchProbeLoads = (SwitchProbeLoads or 0) + 1
SwitchProbeDB = { who = currentPlayer }
";

/// …and it declares that table as a per-character saved variable, so the shutdown writes a real
/// file — which is what [`quitting_from_the_character_screen_does_not_blank_the_session_it_wrote`]
/// watches.
const PROBE_TOC: &str = "\
## Interface: 11200
## SavedVariablesPerCharacter: SwitchProbeDB
SwitchProbe.lua
";

/// A roster with a pick in flight, named — the state a world entry actually runs in
/// ([`super::seat_from_roster`] reads exactly this).
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

/// A hermetic state folder holding one addon — the probe — and the guards that point the whole
/// client at it. Every guard must outlive the world.
fn hermetic_probe(tag: &str) -> (std::path::PathBuf, EnvGuard, EnvGuard) {
    hermetic_addon(tag, "SwitchProbe", PROBE_TOC, PROBE_LUA)
}

/// [`hermetic_probe`] for any one-file addon: the folder, its `.toc` and its Lua, and the guards.
/// The whole point of every test here is what an addon sees **while its file scope runs**, and
/// that differs per question — so the probe's body is a parameter.
fn hermetic_addon(
    tag: &str,
    name: &str,
    toc: &str,
    lua: &str,
) -> (std::path::PathBuf, EnvGuard, EnvGuard) {
    // The pid keeps two concurrent `benilla_app` test binaries out of each other's tree, the same
    // reason `addons::tests::hermetic_root` carries one.
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

/// The world a session boots into: `Startup` has run ([`super::setup_script`]), so there is a VM
/// carrying the font registry and nothing else.
fn booted_world() -> World {
    let mut world = World::new();
    world.init_resource::<super::AddOnIdentity>();
    world.init_resource::<crate::minimap::MinimapZoom>();
    world.init_resource::<super::ReloadUiPending>();
    super::setup_script(&mut world);
    world
}

/// Queue and run a `ReloadUI()` the way the app does: the pending flag, then
/// [`super::run_pending_reload`] — which checks the client state itself, so the test states it.
fn reload(world: &mut World, state: crate::char_select::ClientState) {
    world.insert_resource(State::new(state));
    world.resource_mut::<super::ReloadUiPending>().0 = true;
    super::run_pending_reload(world);
}

/// One login, driven exactly as the app drives it: the roster carries the pick, then the world-entry
/// edge runs.
fn log_in_as(world: &mut World, name: &str, guid: u64) {
    world.insert_resource(roster_named(name, guid));
    super::load_ingame_ui_on_world_entry(world);
}

/// What the probe addon captured at file scope this session — `None` if it never ran.
fn probe_saw(world: &World) -> Option<String> {
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .and_then(|s| s.eval::<Option<String>>("SwitchProbeFileScope").ok())
        .flatten()
}

/// Is a named frame our own FrameXML creates present in the live VM?
fn frame_exists(world: &World, name: &str) -> bool {
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .and_then(|s| s.eval::<bool>(&format!("return {name} ~= nil")).ok())
        .unwrap_or(false)
}

/// How many times the probe's file scope ran **in the VM that is live now**.
fn probe_loads(world: &World) -> u32 {
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .and_then(|s| s.eval::<Option<u32>>("SwitchProbeLoads").ok())
        .flatten()
        .unwrap_or(0)
}

/// **The director's report.** Log in as one character, log out to the character screen, log in as
/// another: the second character's addons must see the second character.
///
/// Pre-fix this asserts `Onehunter` on the second login — the whole bug, in one string.
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

/// The identity the shutdown writes under follows the character, so a logout does not file the
/// second character's saved variables under the first one's name.
///
/// This is the data-corruption half of the same bug: with the load latched, `AddOnIdentity` was
/// only ever written on the first entry, so every later session's `SavedVariables` went into the
/// first character's folder.
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

/// **Quitting from the character screen must not blank what the session already wrote.**
///
/// The client's shutdown runs from five roots, and two of them can fire in sequence: a `/logout`
/// ends the session, and then the player quits from the character screen — where `AppExit` runs
/// [`super::shutdown_ui_state`] again, now against a boot VM with no addon in it. Writing the saved
/// variables *from* that VM would compose every file from nothing.
///
/// It does not, and this pins why: the three write paths each refuse an empty source
/// (`ui_saved::save` on `names.is_empty()`, `save_enable_state` on `states.is_empty()` — its own
/// comment already called an empty write a wipe — and `save_addon_variables` because a boot VM
/// declares no variable sets to iterate). The reference reaches the same place with an explicit
/// guard (`0x401ee0`'s `ds:0x882734` test: "logout then quit writes once, not twice"); ours falls
/// out of the writers having nothing to say, which is only a *safe* answer for as long as those
/// guards hold. Hence a test rather than a comment.
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

    // Now quit — `shutdown_on_exit`'s body, against the boot VM the logout left behind.
    let identity = world.resource::<super::AddOnIdentity>().0.clone();
    let mut script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("a boot VM is live at the character screen");
    // `false`: this models `shutdown_on_exit` at the CHARACTER SCREEN, and that is the one root
    // the reference reaches with no active player — see the test below.
    super::shutdown_ui_state(&mut script, identity.as_ref(), false);

    assert_eq!(
        std::fs::read_to_string(&saved).ok().as_deref(),
        Some(after_logout.as_str()),
        "the quit pass wrote nothing — the session's file is byte-identical"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// **The login arms the world latch, and the logout spends it** (decision 2239) — the chain that
/// makes `PLAYER_LEAVING_WORLD` fire on the commonest root of all.
///
/// This is the regression the latch design could most easily have caused, so it is pinned rather
/// than argued. The obvious guard for the tail — "is there a player object?" — reads FALSE on a
/// `/logout` by the time the tail runs: `net::session::logged_out` despawns our avatar in
/// the same drain that writes `LoggedOutMessage`, `back_on_logout` sets `NextState` off that same
/// message, and `OnExit(InWorld)` does not run until the next frame's `StateTransition`. A
/// predicate would have silenced the event on every logout to fix a quit on a loading screen.
///
/// The latch does not care: it was armed by the entry load (the reference's `0x490168 call
/// 0x4908c0`) and nothing between there and here spends it.
#[test]
fn the_login_arms_the_world_latch_and_the_logout_spends_it() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("latch");
    let mut world = booted_world();
    // `booted_world` seats the VM and the state this file needs, not the whole `UiScriptPlugin`,
    // so the latch is declared here the way the plugin declares it.
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

/// **A quit from the character screen fires `PLAYER_LOGOUT` and NOT `PLAYER_LEAVING_WORLD`** —
/// the reference's one guard inside the shutdown tail (decision 2238).
///
/// `0x490bd0` tests the object manager's active-player GUID pair (`0x490bee call 0x468550` /
/// `0x490bf3 or eax,edx` / `0x490bf5 je 0x490c25`) and the taken side skips exactly one
/// instruction — `0x490c20 call 0x490a80`, the `PLAYER_LEAVING_WORLD` fire — landing on the
/// `PLAYER_LOGOUT` block. Every in-world root takes the other side and fires both.
///
/// Ours fired both from every root, which was invisible: the only root that reaches the tail
/// without a player is this one, and its VM has no frames and no addon files, so nothing was
/// listening. The test exists because that is an argument about *today's* boot VM, not about the
/// split — and the day a glue addon loads, an unguarded fire would be a bug with no trail back.
///
/// Both directions in one test on purpose: asserting only the absence would pass against a
/// `shutdown_ui_state` that had stopped firing the event at all.
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

/// Between a logout and the next login there is **no in-game UI at all** — the character screen is
/// native, and the previous session's frame tree must not survive behind it.
///
/// 1051 measured what that costs when it does: probed under login-screen conditions the in-game
/// tree emits 193 quads, invisible only because the glue screen's opaque node covers them.
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

// ───────────────────────────────── ReloadUI (decision 1291) ─────────────────────────────────

/// **`ReloadUI()` is a real login run in place** — the reference's teardown/rebuild pair
/// (`0x495664`/`0x495669`), which for us is the same two edge functions the logout/login cycle
/// runs. A fresh VM, a fresh file scope, the same character, and the UI back up — without leaving
/// the world.
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

/// **A toggle staged through the API takes effect at the reload** — the whole point of the verb.
/// `DisableAddOn` only marks the live registry; the reload's shutdown tail writes `AddOns.txt`
/// (the reference's own last write before the state dies), and the rebuild reads it back — so the
/// addon is genuinely not loaded, not hidden.
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

/// **Saved variables survive the reload** — written by the teardown (after `PLAYER_LOGOUT`),
/// restored by the rebuild after file scope, so the saved value wins over the file-scope default
/// (the byte-verified `AddOn_Load` order, 1128).
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

/// A `ReloadUI()` that fires outside the world is dropped, not deferred: at the glue there is no
/// in-game UI to rebuild and no identity to load addons under (the reference's own gate,
/// `0x494a50(0xa)`, refuses there too).
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

/// **B271, the error half.** An addon that raises at file scope while entering world must not
/// take the client with it: the walk reports it, the sibling addon still loads, and the player
/// sees the reference's red ScriptErrors dialog (decision 1305) — the report was debugged
/// entirely off terminal WARN lines because the client showed nothing.
#[test]
fn an_addon_error_while_entering_world_reports_on_screen_and_the_sibling_loads() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("addon-error");
    // A second addon, alphabetically FIRST, that dies at file scope — so the probe behind it
    // proves a broken neighbour drops only itself.
    let dir = tmp.join("benilla-config/AddOns/AaBroken");
    std::fs::create_dir_all(&dir).expect("broken addon dir");
    std::fs::write(dir.join("AaBroken.toc"), "## Interface: 11200\nboom.lua\n").expect("toc");
    std::fs::write(dir.join("boom.lua"), "error('B271: file-scope boom')\n").expect("lua");
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
        shown.contains("B271: file-scope boom"),
        "the dialog names the actual error, got: {shown:?}"
    );
    drop(script);
    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// **B271, the freeze half.** An addon that never returns cannot freeze world entry: the load
/// bound (decision 1306) fails it with the distinctive budget message, the sibling addon still
/// loads, and this test FINISHING is the claim — before 1306 it would hang here forever.
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
    // The budget raise travelled the load walk's failure arm into the handler queue (1305), so
    // the player-facing dialog is where it lands — the frozen loading screen becomes a dialog
    // that NAMES the loop.
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

// ─────────────────── The deferred entry load (0962's frame accounting) ───────────────────

/// **The director's "frozen char for 1 sec" report, from the other side.** With the cover up,
/// the armed entry load waits [`super::lifecycle::run_pending_entry_load`]'s covered-frame
/// count — the frames whose renders put the cover on the glass — and only then pays the burst.
/// Before the deferral the load ran inside `OnEnter(InWorld)`, which is exactly the frame whose
/// render would first present the cover, so the ~0.5 s of FrameXML + addons + `PLAYER_LOGIN`
/// held the previous present: the frozen character screen.
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

    // Covered frames 1 and 2: the cover has not provably presented yet — no load.
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
    // Covered frame 3: two cover renders have committed — the burst is hidden. Load.
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

/// No cover — a capture booting straight `InWorld`, or the screen's assets missing — means no
/// glass to protect: the armed load runs on the first frame. Without this arm a coverless run
/// would count covered frames that never come and the UI would never load.
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

/// An exit inside the deferral window (an instant disconnect at entry) drops the armed load and
/// **writes nothing**: the session never built a UI, so the shutdown tail running against the
/// boot VM would compose every saved file from emptiness — the wipe
/// [`quitting_from_the_character_screen_does_not_blank_the_session_it_wrote`] guards at the
/// other edge.
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
    super::lifecycle::run_pending_entry_load(&mut world); // covered frame 1 — still pending
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

// ───────────── The login one-shots wait for the in-game UI (decision 1348) ─────────────

/// **The director's white XP bar, from the side that causes it.**
///
/// The world-entry UI load is deferred a few covered frames (0962/1051), and the unit feed is not:
/// it fires `PLAYER_ENTERING_WORLD`, the first `PLAYER_XP_UPDATE` and the first `UPDATE_EXHAUSTION`
/// the moment our own descriptor lands. When that lands *inside* the deferral window the events go
/// to a VM with no frames, and because every one of them is latched by a [`super::VmMemo`] keyed on
/// the VM's session — which the entry load does not change — they never fire again. The frames
/// built moments later do their first paint with no first paint, which is why
/// `ExhaustionTick_Update` had never run and `ExhaustionLevelFillBar` was still wearing its
/// authored opaque white across the whole XP strip, with the tick parked at the strip's centre.
///
/// It is a RACE against the wire, so it took some logins and not others.
///
/// The probe is a plain frame in the boot VM registering the event the way FrameXML does — if the
/// feed runs at all in the window, it sees it.
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

    // The probe frame — FrameXML's own shape, in the VM that exists before the entry load.
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

    // Our own descriptor has landed — the condition the feed fires the one-shots on.
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

    // **The frame the latch cannot see** (B376): the descriptor above came off the same drain as
    // `Connected`, so the wire is in-world while the state still says glue and no load is armed
    // yet — `OnEnter(InWorld)` runs next frame. A feed gated only on the latch runs here.
    app.insert_resource(State::new(crate::char_select::ClientState::CharSelect));
    app.update();
    assert_eq!(
        seen(&app),
        0,
        "the drain's own frame: in-world wire, a boot VM, and no latch yet"
    );

    // …the transition ran, and the in-game UI is still owed. Three frames inside the deferral
    // window.
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

    // The entry load has run (its own tests cover the timing) — the latch is gone, and the very
    // next feed delivers the full set to the frames that now exist.
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

/// **B293's headline, at the edge that produces it** (decision 1495). An addon that fails to load
/// *without raising* — the commonest shape by far, a `.toc` naming a file the package does not
/// ship — used to `warn!` to the terminal and vanish. Nothing raised, so 1305's dialog could not
/// fire; the walk's failure list was dropped on the floor at `load_ingame_ui_on_world_entry`; and
/// the per-frame drain kept no history. From the player's chair the addon simply was not there and
/// the client said nothing.
///
/// Three claims, and the third is the one that makes the first two reachable: the failure is
/// **retained**, it is **readable from Lua** (so the window is a view of it, not a second copy),
/// and the player is **told to look**.
#[test]
fn an_addon_that_fails_to_load_without_raising_is_readable_in_the_error_log() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("addon-missing-file");
    // Alphabetically first, so the probe behind it also proves this class drops only itself.
    let dir = tmp.join("benilla-config/AddOns/AaMissing");
    std::fs::create_dir_all(&dir).expect("addon dir");
    std::fs::write(
        dir.join("AaMissing.toc"),
        "## Interface: 11200\nBossnames\\BossNames.xml\n",
    )
    .expect("toc");
    // …and no such file is written. This is the director's own AtlasLoot copy, reduced.
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

    // 1 · retained, and tagged as a LOAD failure — the kind that means "this addon is not running".
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

    // 2 · readable from Lua by the same three reads the window uses.
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

    // …and the window itself materialized, so `/errors` has something to toggle.
    assert!(
        script
            .eval::<bool>("return BenillaScriptLogFrame ~= nil")
            .expect("eval"),
        "ScriptLogFrame.xml loaded and built the window"
    );

    // **The repaint actually runs, over a log that has a row in it.** Asserting the globals exist
    // would prove nothing: this drives the real path — `FauxScrollFrame_Update`, the row rebind,
    // `strfind`/`strsub`/`strlen`/`format`, `SetTextColor`, the highlight seat and the detail
    // pane — and a `nil` global anywhere in it raises here instead of on the player's first
    // `/errors` (which is exactly the shape of failure this whole record exists to stop shipping).
    script
        .eval::<()>("BenillaScriptLog_Update() return nil")
        .expect("the window repaints over a real log without raising");
    // **A ROW shows it — not necessarily row 1.** The log is ordered by first occurrence and it
    // carries warnings now (2135), which a world entry produces before any addon is reached, so
    // asserting an index would be asserting how many warnings the stock UI happens to raise.
    // What this pins is the row TEXT, which is the window's own trim path over a real message.
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

    // 3 · **this class deliberately does NOT seize the screen.** Nothing raised; the reference's
    // answer to an unparseable/absent document is a log line and silence, and 1495 keeps that.
    // What it changes is that the silence is no longer total — hence the chat notice below.
    assert!(
        !script
            .eval::<bool>("return ScriptErrors:IsVisible()")
            .expect("ScriptErrors exists"),
        "a non-raising load failure must not pop the red dialog — that would put non-errors \
         through `_ERRORMESSAGE` and through every addon handler that replaces it"
    );

    // 4 · the player is told to look. Without this the log is a room nobody knows about, and
    // silence — not the missing list — is the actual defect B293 reports.
    assert_eq!(
        world.resource::<crate::ui_chat::ChatLog>().pending_len(),
        1,
        "world entry queued the 'N addon load failures — type /errors' line"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// **The burst 1305 measured, as the log sees it.** An `OnUpdate` that raises every frame produced
/// 470–1113 collected errors in 1305's own runs, of which `_ERRORMESSAGE` shows the **first** and
/// the per-frame drain keeps none. Deduplication is what makes a log survive that: one row with a
/// count, not 1,113 rows and not a truncated window of the last few.
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
    // The same failure, over and over, through the engine's own catch path — a slash command whose
    // body raises is the cheapest real one to drive from a test.
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

// ── B353 · the layout cache is a resident of the shutdown tail ───────────────────────────────
//
// The symptom: an unlocked chat window, resized or dragged, comes back at its original size after
// a `/reload` or a relog, and `benilla-config/layout/` is never created. The engine seam and the
// file round trip were already proven by
// [`crate::ui_script::chat_resize_tests::the_geometry_round_trips_through_the_save_file`]; what
// was wrong is which edge writes. [`crate::ui_layout`] hung its saver off `OnExit(InWorld)`, and a
// `/reload` never leaves `InWorld` — [`super::run_pending_reload`] calls the shutdown and the
// rebuild back to back — so on the root a player uses most nothing was written at all. These two
// drive the roots themselves, which is the only place that distinction is visible.

/// A window the player has placed, made the way a drag makes one: movable first, then the
/// userPlaced bit (`SetUserPlaced` refuses a frame that is neither movable nor resizable).
/// Parentless, so its anchor is the screen root — the file's `-` target — which is what lets this
/// need no FrameXML and no install.
/// **Both flags, deliberately**: the layout cache's apply is gated per arm — position behind
/// `movable`, size behind `resizable` (decision 2193) — so a probe standing in for a window the
/// player both moved and resized has to carry both, or half its geometry is correctly left behind.
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

/// The layout cache the shutdown left behind for this character, if any.
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

/// **Logging out writes the window's geometry** — the tail's step three, on the root that leaves
/// the world.
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

/// **`/reload` writes it too — B353 itself.** The reload root never leaves `InWorld`, so it is
/// exactly the root an `OnExit(InWorld)` saver cannot see: pre-fix this finds no file at all, and
/// the player's unlocked chat window comes back on its authored anchors.
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

/// The file is **per character**, and the tail writes back to the one the UI loaded under: a
/// second character's logout must not answer with the first one's windows.
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

/// A window as FrameXML would author it — the shape the restore has to overwrite. Same name as
/// [`place_a_window`]'s, different geometry, and **not** user-placed: this is the fresh tree a
/// relog meets.
///
/// It carries the same `movable`/`resizable` pair, because those are the window's **authored**
/// state — XML attributes on a real resizable window, rebuilt with it — and the layout cache's
/// apply reads them off the live frame to decide which arm runs (decision 2193).
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

/// **The whole loop, on the root that reported it**, in one test: place a window,
/// `/reload`, meet a fresh tree that has it on its authored anchors, and let the loader seat the
/// saved geometry back over the top.
///
/// [`crate::ui_layout::load_layout`] is run directly because this harness drives the world's edges
/// rather than its schedules; the authored window stands in for the FrameXML the real reload
/// rebuilds.
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

/// **A clean world entry raises the KNOWN warnings and no others** — the tripwire that keeps
/// decision 2135's channel worth having.
///
/// The stock FrameXML is loaded here with no addons at all, so every row is a gap of *ours*.
/// Before 2135 these reached a terminal and nothing else, and four of them had been sitting in
/// every session for as long as the interface has been stock: two `OnCursorChanged` refusals (the
/// mail body and the GM ticket box, which therefore did not scroll as you typed — 2141), one
/// `OnInputLanguageChanged`, and an unregistered `useUiScale` that `ContainerFrame.lua` and
/// `UIDropDownMenu.lua` both read.
///
/// The allowlist is deliberately a **list of names, not a count**: a new silent gap in a stock
/// file reddens this instead of scrolling past, and closing one means deleting a line here.
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

    // **`OnInputLanguageChanged` stays out permanently**, and this is where that is said out loud.
    // It is a real 1.12 slot (`FloatingChatFrame.xml` wires it to the IME language indicator) with
    // zero corpus call sites, and benilla has no IME — so nothing here could ever fire it, and
    // `SCRIPT_KINDS`' rule is that a name we cannot fire stays out. The refusal is the honest
    // answer; the row is the price of saying it out loud.
    //
    // **`gxRefresh` stays out permanently too** (decision 2177). The stock VIDEO options window
    // reads it in `OptionsFrameRefreshDropDown_OnLoad` — one of the two `<OnLoad>` paths that run
    // on the spot when that file loads — and benilla does not register it, because a refresh rate
    // is only selectable through an exclusive mode-set and this client ships none on any target
    // (`crate::video`'s module doc walks each). `GetRefreshRates` therefore returns the
    // reference's own "no rates available" sentinel, the dropdown greys itself, and nothing ever
    // reads the variable. Registering it would be a key with no reader — 1134 §4's silent
    // pretence — so the warn-once is the honest answer and this row is the price of saying so.
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

// ─────────────────── The predicate every in-world feed runs on (B376) ───────────────────

/// **`not(ingame_ui_pending)` was only half the gate**, and the missing half is a whole frame
/// wide.
///
/// [`super::lifecycle::PendingEntryUiLoad`] is armed at `OnEnter(InWorld)`, and that edge trails
/// the wire by one frame: `apply_net_updates` drains `Connected` and the login burst behind it in
/// a single `try_iter`, `enter_on_connected` sets `NextState` out of that same drain, and the
/// transition — with it the park (1978) and this latch — does not run until the next frame's
/// `StateTransition`. So there is exactly one frame holding in-world wire state and a live *boot*
/// VM, and a feed gated only on the latch runs straight through it: B376's `GUILD_MOTD` fired
/// into a VM with no `ChatFrame1`, spent its `VmMemo` edge, and the login line never printed.
///
/// The `InWorld` term is what closes it — the state flips on the same edge that arms the latch,
/// so the pair is shut at both ends.
#[test]
fn the_ui_is_not_up_in_the_frame_between_the_wire_and_the_state() {
    let mut world = World::new();

    // **THE FRAME.** A live boot VM, no load owed — and the state still says glue, because
    // `Connected` has only just been drained (so the guild/unit burst is already in ECS state)
    // and the transition it queued runs next frame. The latch alone reads this as "the UI is up".
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

/// **The map catalog is in the VM before the first addon file runs** (decision 2240) — Questie's
/// `Astrolabe.lua:62: attempt to index local 'zoneData' (a nil value)`, from the side that causes
/// it.
///
/// `GetMapContinents`/`GetMapZones` are static DBC data, and the corpus reads them at **file
/// scope**: Astrolabe — the positioning library under Questie and Cartographer — builds its whole
/// continent → zone table inside `AceLibrary:Register`'s synchronous `activate`, and every icon it
/// ever places indexes that table. Pushed from an `Update` system, the catalog landed ~210 ms after
/// this edge returned (measured live, both logins of a round trip: `conts=0 zones(1)=0`), so the
/// table was built from two empty lists and every placement afterwards indexed a nil zone.
///
/// The catalog is planted rather than built: the question here is the ORDER, and the build itself
/// stands against the real chain in
/// [`super::world_map_tests::the_real_feralas_catalog_names_dire_maul_under_the_cursor`].
#[test]
fn an_addon_reads_the_map_catalog_at_file_scope() {
    const MAP_PROBE_TOC: &str = "\
## Interface: 11200
MapProbe.lua
";
    // Astrolabe's own two calls, in its own idiom — a list constructor around a multi-return.
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

/// **The keybinding table is in the VM before the first addon file runs** (decision 2241).
///
/// Stock `ActionButton_OnLoad` paints its hotkey corner from `GetBindingText(GetBindingKey(action))`
/// at OnLoad, and an addon that rebinds a stock command needs that command to exist. Seeded from an
/// `Update` system, the table held only the addons' own `Bindings.xml` rows for the whole load edge:
/// `GetBindingKey("TOGGLEWORLDMAP")` answered nothing and `SetBinding` on a stock command was a
/// silent nil.
///
/// No fixture: the registry is a compile-time table, which is exactly why its absence during the
/// burst was a timing bug and nothing else.
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
    // No `BindingFiles` planted on purpose: the seed takes it optionally, which is what keeps
    // every other harness in this file working.
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

/// **The zone-channel catalog is in the VM before the first addon file runs** (decision 2241) —
/// and this one puts a packet on the wire when it is not.
///
/// An empty catalog is not "no zone yet" to `JoinChannelByName`: it is *"no such built-in
/// channel"*, so `JoinChannelByName("General")` takes the custom-channel leg and queues a real
/// `CMSG_JOIN_CHANNEL("General")` — a custom channel of that name on the server, and the chat
/// cache damage `ui_chat::channels` documents. Seeded zone-less, the same call matches the row,
/// finds `resolved: None`, and does nothing — the reference's own answer while there is no zone
/// text.
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
            // Row 1 as the shipped table has it: auto-joined, and its `%s` is the zone's name —
            // which is what makes it unanswerable before a zone and answerable after.
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

/// **The screen size is in the VM before the first `<OnLoad>` runs** (decision 2242) — the
/// director's "the map is missing the bg again", from the side that causes it.
///
/// A fresh `Model` starts at 1024×768 and only `tick_script` (an `Update` system) corrects it, so
/// since 2226 gave the entry its own VM, every OnLoad and every addon file scope read that default.
/// Anchored frames survive it — the first frame's resize re-solves them — but a number a file
/// *computes once* does not, and the stock `WorldMapFrame_OnLoad` computes exactly one: the size of
/// `BlackoutWorld`, the quad that hides the world behind the map. At 1024×768 units on a wider
/// window it stops short of the edges and the world shows through.
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
    // 2560×1440 with no `UiScaleCvar` planted (so the dial is 1): the seam scale is 1440/768 =
    // 1.875, which puts the VM's screen at 1365.33 × 768 units. The WIDTH is the discriminator —
    // 768 is what the height reads under any window, and 1024 is what the width read before this.
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

/// **The entry load seeds the player record, not just the snapshot** (decisions 2261/2263).
///
/// [`super::seat_from_roster`]'s `"player"` push is the descriptor's stand-in and is *replaced*
/// the moment the real one streams in — which is how decision 2260's nameless push reached
/// `UnitName("player")`. The buffer is seeded beside it, from the same roster row, and the verb
/// reads only that; so the token can be replaced by a nameless snapshot or removed outright and
/// the name still answers, exactly as the reference's never-cleared `0xc27d88` does.
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
    // The feed's 2260 push: the descriptor landed, the name cache missed for our own guid.
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

    // …and so does the logout despawn, which removes the token altogether.
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

/// **A login and a `/reload` load without a sound** — `0x48fbf0` brackets itself in the counted
/// suppression scope (`0x48fbfa` → `0x49016d`) across the TOC walk, the addons, the saved
/// variables and the login cascade, and both of its callers (login `0x48f681`, `/reloadui`
/// `0x495669`) go through it.
///
/// Stock `TargetFrame_OnLoad` → `TargetFrame_Update` → `Hide()` → `TargetFrame_OnHide` really does
/// call `PlaySound("INTERFACESOUND_LOSTTARGETUNIT")` at load; the engine drops it. Before the
/// production edge carried the bracket only the tests' whole-manifest load did, and every
/// `/reload` played the lost-target click.
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
