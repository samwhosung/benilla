//! World entry and exit through [`super::load_ingame_ui_on_world_entry`] and
//! [`super::end_ui_session`]: each login runs addon file scope in a fresh VM, and each exit root
//! writes what the session owes. Each test writes its probe addon into a hermetic `BENILLA_HOME`.

use bevy::prelude::*;

use crate::glue_session::roster::Roster;
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

/// The world after `Startup` ([`super::setup_script`]): the stock glue VM at the login screen.
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
fn reload(world: &mut World, state: crate::client_state::ClientState) {
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
         \"always Onewarrior\" bug, from the other side"
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

/// A quit after a logout runs [`super::shutdown_ui_state`] again, against the glue VM, and writes
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

    // Quit: `shutdown_on_exit`'s body, against the glue VM the logout left.
    let identity = world.resource::<super::AddOnIdentity>().0.clone();
    let mut script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("the glue VM is live at the character screen");
    // At the character screen there is no active player and no world to sweep.
    super::shutdown_ui_state(&mut script, identity.as_ref());

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

    // A load with no player of ours (a login the server then refuses) arms nothing (`0x490166`).
    log_in_as(&mut world, "Onehunter", 1);
    assert!(!world.resource::<super::LeavingWorldArmed>().is_armed());
    super::end_ui_session(&mut world);

    world.spawn(crate::net::SelfPlayer);
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

/// `CGGameUI::InitializeGame` flags `realmList`, `realmName` and `scriptMemory` read-only for the
/// world session (`0x48f566`-`0x48f584`) and `ShutdownGame` clears them (`0x491240`): an
/// in-game `SetCVar` of one raises, through a reload too. The character screen's glue state has no
/// `SetCVar` to try (`reference/glue_surface.tsv`), on either side of the session.
#[test]
fn the_realm_cvars_are_read_only_to_lua_only_in_the_world() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("readonly");
    let mut world = booted_world();
    world.init_resource::<super::LeavingWorldArmed>();
    let set = |world: &World, chunk: &str| {
        let s = world.non_send_resource::<benilla_ui::script::UiScript>();
        // A bare test world has no registry to seed the VM from.
        s.register_cvars(crate::cvars::registered_pairs());
        s.run(chunk).map_err(|e| e.to_string())
    };
    let glue_has_no_set_cvar = |world: &World| {
        let s = world.non_send_resource::<benilla_ui::script::UiScript>();
        s.vm_kind() == benilla_ui::script::VmKind::Glue
            && s.eval::<bool>("return SetCVar == nil").unwrap()
    };

    assert!(glue_has_no_set_cvar(&world), "the character screen's VM");

    log_in_as(&mut world, "Onehunter", 1);
    for name in ["realmList", "realmName"] {
        let err = set(&world, &format!("SetCVar({name:?}, \"x\")")).expect_err(name);
        assert!(err.contains(&format!("\"{name}\" is read-only")), "{err}");
    }
    assert_eq!(set(&world, "SetCVar(\"MusicVolume\", 0.5)"), Ok(()));

    reload(&mut world, crate::client_state::ClientState::InWorld);
    assert!(
        set(&world, "SetCVar(\"realmList\", \"x\")").is_err(),
        "a reload keeps the flag, as the reference's rows keep theirs"
    );

    super::end_ui_session(&mut world);
    assert!(
        glue_has_no_set_cvar(&world),
        "the flags died with the world's VM"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The shutdown tail's one guard: `0x490bd0` tests the active player's GUID pair (`0x490bee call
/// 0x468550`, `0x490bf3 or eax,edx`) and with none jumps (`0x490bf5 je 0x490c25`) past only the
/// sweep, `0x490c20 call 0x490a80`, whose `0x490b4d` fires `PLAYER_LEAVING_WORLD`, while
/// `PLAYER_LOGOUT` fires either way. Ours: a root with no world latch armed.
#[test]
fn a_shutdown_with_no_world_fires_logout_without_leaving_world() {
    use crate::ui_script::SweepApp;
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("quitsplit");
    let mut world = booted_world();
    // A world entry the server then refuses: the in-game VM is built, with no player of ours to
    // arm the latch (`0x490166`).
    log_in_as(&mut world, "Onehunter", 1);
    let script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("the entry's VM");

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
    let mut app = App::new();
    app.init_world_leave_sweep();
    app.insert_non_send_resource(script);
    // A root's order: the sweep, then the tail.
    let root = |app: &mut App| {
        super::run_leave_world_sweep(app.world_mut());
        let mut script = app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>();
        super::shutdown_ui_state(&mut script, None);
    };
    let seen = |app: &App, global: &str| {
        app.world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval::<i64>(&format!("return {global}"))
            .unwrap()
    };

    root(&mut app);
    assert_eq!(
        seen(&app, "SeenLeaving"),
        0,
        "a shutdown with no world fired PLAYER_LEAVING_WORLD — the reference's 0x490bf5 skips it"
    );
    assert_eq!(
        seen(&app, "SeenLogout"),
        1,
        "…and it must still fire PLAYER_LOGOUT, which is the whole of the taken side"
    );

    app.world_mut()
        .resource_mut::<super::LeavingWorldArmed>()
        .arm();
    root(&mut app);
    assert_eq!(
        seen(&app, "SeenLeaving"),
        1,
        "an in-world root must fire PLAYER_LEAVING_WORLD — the control for the assertion above"
    );
    assert_eq!(
        seen(&app, "SeenLogout"),
        2,
        "…and PLAYER_LOGOUT on every root, guarded or not"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The character screen is the glue VM's: the old session's frame tree must not survive behind it.
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
        "and so is the in-game frame tree — a leftover tree once drew 193 quads' worth \
         behind the glue screen's opaque node"
    );
    assert!(
        world
            .get_non_send_resource::<benilla_ui::script::UiScript>()
            .is_some(),
        "the glue VM stands in its place"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ───────────────────────────────── ReloadUI ─────────────────────────────────

/// The sweep's four legs that change a lock or the cursor, in a bare world with the sessions they
/// read, a mailbox, an auctioneer and a vendor open; the reload runs them in the dying VM.
fn sweep_lock_legs(world: &mut World) {
    use crate::ui_script::SweepLeg;
    super::sweep::install_world_leave_sweep(world, |s| {
        s.add_systems((
            crate::ui_loot::close_loot_on_leave_world.in_set(SweepLeg::Loot),
            crate::ui_merchant::close_core.in_set(SweepLeg::Merchant),
            crate::ui_mail::close_mail_on_leave_world.in_set(SweepLeg::Mail),
            crate::ui_auction::close_core.in_set(SweepLeg::Auction),
        ));
    });
    let (tx, _rx) = crossbeam_channel::unbounded();
    world.insert_resource(crate::net::NetCommands(tx));
    world.init_resource::<crate::net::GuidIndex>();
    world.init_resource::<crate::ui_loot::LootLatch>();
    world.init_resource::<Messages<crate::target::DeselectGuid>>();
    world.init_resource::<crate::ui_mail::MailOpen>();
    world.init_resource::<crate::ui_auction::AuctionOpen>();
    world.init_resource::<crate::ui_merchant::MerchantOpen>();
    world.init_resource::<crate::items::Items>();
    world.init_resource::<Messages<crate::sound::LootPickupSound>>();
    // Our own player: the reload's guard (`0x490bee`) passes only with one.
    world.spawn(crate::net::SelfPlayer);
}

/// Arm the world latch as our own create does, so the next reload sweeps.
fn arm_world(world: &mut World) {
    world.resource_mut::<super::LeavingWorldArmed>().arm();
}

/// The item lock is the item object's (`item+0x314`), which a `/reload` keeps, save what the
/// reload's sweep unlocks at its legs: the looted clam (`0x490ab2`), the mail attachment
/// (`0x490b1b`) and the auction sell item (`0x490b25`). A trade offer stays locked, its table
/// zeroed with no unlock (`0x4909b5`), and the cursor, engine state, keeps a held item and an
/// armed wrap with their locks. A new world entry builds its objects afresh and starts with none.
#[test]
fn item_locks_cross_a_reload_as_its_sweep_leaves_them() {
    const CLAM: u64 = 0x4000_0000_0000_0c1a;
    const MAILED: u64 = 0x4000_0000_0000_0a11;
    const AUCTIONED: u64 = 0x4000_0000_0000_0a5c;
    const TRADED: u64 = 0x4000_0000_0000_0a7d;
    const HELD: u64 = 0x4000_0000_0000_0a4e;
    const PAPER: u64 = 0x4000_0000_0000_0a9a;
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("item-locks");
    let mut world = booted_world();
    world.init_resource::<crate::ui_items::locks::HeldItemLocks>();
    sweep_lock_legs(&mut world);
    let locked = |world: &World, guid: u64| {
        world
            .get_non_send_resource::<benilla_ui::script::UiScript>()
            .expect("in-world VM")
            .item_locked(guid)
    };

    log_in_as(&mut world, "Onehunter", 1);
    {
        let mut s = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
        s.lock_item(CLAM); // the open-item send's lock (`0x5edcd9`)
                           // A letter's attachment, an auction's sell item and a trade offer, each holding its
                           // pickup's lock, then an item held on the cursor and a wrap armed.
        let slot = |guid: u64| benilla_ui::script::ContainerSlot {
            item_id: 117,
            count: 1,
            guid,
            ..Default::default()
        };
        s.set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                num_slots: 16,
                slots: std::collections::HashMap::from([
                    (1, slot(MAILED)),
                    (2, slot(AUCTIONED)),
                    (3, slot(TRADED)),
                    (4, slot(HELD)),
                    (5, slot(PAPER)),
                ]),
                ..Default::default()
            }),
        );
        s.run("PickupContainerItem(0, 1) ClickSendMailItemButton()")
            .unwrap();
        s.run("PickupContainerItem(0, 2) ClickAuctionSellItemButton()")
            .unwrap();
        s.set_trade(Some(Default::default()));
        s.run("PickupContainerItem(0, 3) ClickTradeButton(1)")
            .unwrap();
        s.run("PickupContainerItem(0, 4)").unwrap();
    }
    let mut loot = crate::ui_loot::LootState::default();
    loot.open(CLAM, 1, 0, Vec::new());
    world.insert_resource(loot);
    // The clam's own object, `OBJECT_FIELD_TYPE` object and item, which the close looks up
    // (`0x48f29e`) before its unlock.
    crate::items::test_spawn_item(
        &mut world,
        CLAM,
        benilla_protocol::messages::ObjectFields::from_pairs(&[(2, 0x3)]),
        false,
    );
    world.resource_mut::<crate::ui_mail::MailOpen>().mailbox = Some(0xF110_0000_0000_0B0C);
    world
        .resource_mut::<crate::ui_auction::AuctionOpen>()
        .open(0xF130_0000_0000_0A0C, 1);
    arm_world(&mut world);
    reload(&mut world, crate::client_state::ClientState::InWorld);
    assert!(!locked(&world, CLAM), "the sweep's loot close unlocks it");
    assert!(!locked(&world, MAILED), "the sweep's mail reset unlocks it");
    assert!(locked(&world, TRADED), "nothing unlocks a trade offer");
    assert!(
        !locked(&world, AUCTIONED),
        "the sweep's auction close unlocks it (`0x4cd5f0` → `0x4cd640`)"
    );
    {
        let s = world.non_send_resource::<benilla_ui::script::UiScript>();
        assert_eq!(
            s.cursor_item().map(|i| i.guid),
            Some(HELD),
            "no clear on the reload's path touches a held item"
        );
        assert!(s.item_locked(HELD), "and its pickup's lock stays");
    }
    assert!(locked(&world, TRADED));
    // An armed wrap is engine state too (`[0xc4d778]`): it stays armed, its paper locked.
    {
        let mut s = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
        s.run("ClearCursor()").unwrap();
        // The bag as the container feed re-pushes it into the new VM.
        s.set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                num_slots: 16,
                slots: std::collections::HashMap::from([(
                    5,
                    benilla_ui::script::ContainerSlot {
                        item_id: 117,
                        count: 1,
                        guid: PAPER,
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
        );
        s.arm_gift_wrap(0, 5);
    }
    arm_world(&mut world);
    reload(&mut world, crate::client_state::ClientState::InWorld);
    {
        let s = world.non_send_resource::<benilla_ui::script::UiScript>();
        assert_eq!(s.gift_wrap_armed().map(|w| w.guid), Some(PAPER));
        assert!(s.item_locked(PAPER), "the armed paper stays locked");
        assert!(
            !s.item_locked(HELD),
            "the clear before it unlocked the held item"
        );
    }
    // A held vendor row, mode 5, goes with the sweep's merchant close (`0x4fadf0`, `0x4fae4a`).
    {
        let mut s = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
        s.run("ClearCursor()").unwrap();
        s.set_merchant(Some(benilla_ui::script::MerchantState {
            items: vec![benilla_ui::script::MerchantItem {
                name: Some("Refreshing Spring Water".into()),
                price: 25,
                quantity: 5,
                num_available: -1,
                item_id: 159,
                ..Default::default()
            }],
            ..Default::default()
        }));
        s.run("PickupMerchantItem(1)").unwrap();
        assert!(matches!(
            s.cursor_payload(),
            Some(benilla_ui::script::CursorPayload::Merchant(_))
        ));
    }
    world
        .resource_mut::<crate::ui_merchant::MerchantOpen>()
        .open(0xF130_0000_0000_0E1D, Vec::new());
    arm_world(&mut world);
    reload(&mut world, crate::client_state::ClientState::InWorld);
    assert!(world
        .non_send_resource::<benilla_ui::script::UiScript>()
        .cursor_payload()
        .is_none());

    super::end_ui_session(&mut world);
    log_in_as(&mut world, "Onehunter", 1);
    assert!(
        !locked(&world, TRADED),
        "a new world entry inherits no lock"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

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

    reload(&mut world, crate::client_state::ClientState::InWorld);

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
        "the VM identity changed, so every VmMemo about the old session expires"
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

/// The connection's billing group reaches every in-game VM, as `GetBillingTimeRested` reads the
/// session's word (`0x48ec59`), a reload's VM included; the world entry arms the play-time warning
/// (`0x48f630`) and a reload, which jumps to `UI_Init` alone (`0x495669`), does not.
#[test]
fn the_billing_group_reaches_each_vm_and_arms_at_entry_only() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("billing");
    let mut world = booted_world();
    let group = |time_remaining, time_rested| {
        crate::net::SessionBilling(benilla_protocol::messages::Billing {
            time_remaining,
            plan_flags: 0,
            time_rested,
        })
    };
    let rested = |world: &World| {
        world
            .get_non_send_resource::<benilla_ui::script::UiScript>()
            .expect("in-world VM")
            .eval::<f64>("return GetBillingTimeRested()")
            .unwrap()
    };
    let armed = |world: &World| {
        world
            .resource::<crate::ui_billing::PlayTimeWarning>()
            .armed()
    };

    world.insert_resource(group(20, 150));
    log_in_as(&mut world, "Onehunter", 1);
    assert_eq!(rested(&world), 150.0, "the entry's VM reads the session");
    assert!(armed(&world), "20 minutes left arms the warning");

    // A group that would not arm, so only an arm could clear it.
    world.insert_resource(group(0, 151));
    reload(&mut world, crate::client_state::ClientState::InWorld);
    assert_eq!(
        rested(&world),
        151.0,
        "the reload's VM reads the session too"
    );
    assert!(armed(&world), "a reload does not arm again");

    log_in_as(&mut world, "Onehunter", 1);
    assert!(
        !armed(&world),
        "the next entry arms from the group it finds"
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
        "disabling alone changes nothing in the live session — there is no unload"
    );

    reload(&mut world, crate::client_state::ClientState::InWorld);

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

    reload(&mut world, crate::client_state::ClientState::InWorld);

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

/// The quest log's fold mask (`[0xbb748c]`) and selection (`[0xbb7480]`) and the tradeskill
/// repeat (`0xbde05c`, `0xbde060`) are C globals no UI init resets: they fold out of the dying VM
/// and seat in the rebuilt one before its load, so a `/reload` keeps a collapsed header folded,
/// its selected quest selected and a batch armed; the session's end drops them.
#[test]
fn a_reload_keeps_the_quest_folds_and_the_repeat() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("folds");
    let mut world = booted_world();
    world.init_resource::<crate::ui_quest_log::QuestLog>();
    world.init_resource::<crate::ui_tradeskill::TradeSkillRepeatCarry>();
    log_in_as(&mut world, "Onehunter", 1);

    // One quest under its header: two rows open, one folded.
    let rows = || {
        [crate::ui_quest_log::fixture::quest(
            7,
            "Wolves Across the Border",
            10,
        )]
    };
    let visible = |world: &mut World| {
        let mut script = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
        crate::ui_quest_log::fixture::seat(&mut script, &rows());
        script
            .eval::<u32>("return (GetNumQuestLogEntries())")
            .expect("count")
    };
    assert_eq!(visible(&mut world), 2);
    {
        let mut script = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
        // A selection, so the stock `QuestLog_Update` does not re-select the folded header, which
        // would open it (`QuestLogFrame.lua:294`, `:323`).
        script
            .run("SelectQuestLogEntry(2) CollapseQuestHeader(1)")
            .expect("fold");
        script.set_trade_skill(Some(benilla_ui::script::TradeSkillState {
            line: 197,
            line_name: "Tailoring".into(),
            rank: 1,
            max_rank: 75,
            recipes: vec![benilla_ui::script::TradeSkillRecipe {
                group: None,
                spell_id: 2963,
                name: "Bolt of Linen Cloth".into(),
                difficulty: benilla_ui::script::TradeSkillDifficulty::Trivial,
                num_available: 5,
                icon: None,
                min_made: 1,
                max_made: 1,
                cooldown: None,
                product_item: 2996,
                product_inv_type: 0,
                product_item_level: 0,
                reagents: vec![],
                tools: vec![],
            }],
        }));
        script.run("DoTradeSkill(2, 3)").expect("arm the batch");
    }
    assert_eq!(visible(&mut world), 1);

    reload(&mut world, crate::client_state::ClientState::InWorld);
    assert_eq!(visible(&mut world), 1, "the reload unfolded the header");
    assert_eq!(
        world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval::<u32>("return GetQuestLogSelection()")
            .expect("selection"),
        2,
        "the reload dropped the selection, which the stock update would re-pick and unfold"
    );
    assert_eq!(
        world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .trade_skill_repeat(),
        (0, 3),
        "the world-enter init disarms the spell and keeps the count (`0x4fc76b`)"
    );

    world.insert_resource(State::new(crate::client_state::ClientState::CharSelect));
    super::end_ui_session(&mut world);
    log_in_as(&mut world, "Onehunter", 1);
    assert_eq!(visible(&mut world), 2, "a new session starts unfolded");

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

    reload(&mut world, crate::client_state::ClientState::CharSelect);

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
    world.insert_resource(State::new(crate::client_state::ClientState::InWorld));
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
    world.insert_resource(State::new(crate::client_state::ClientState::InWorld));
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
/// against the glue VM would compose every saved file from nothing.
#[test]
fn leaving_inside_the_deferral_window_drops_the_load_and_writes_nothing() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("dropped");
    let mut world = booted_world();
    world.insert_resource(State::new(crate::client_state::ClientState::InWorld));
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

/// An app running the unit feed after `Startup`, with what the feed reads; the receiver is the
/// wire, kept alive beside it.
fn unit_feed_app() -> (App, crossbeam_channel::Receiver<crate::net::ClientCommand>) {
    let mut app = App::new();
    app.add_plugins(crate::ui_unit::UiUnitPlugin);
    app.init_resource::<super::AddOnIdentity>();
    app.init_resource::<crate::minimap::MinimapZoom>();
    app.init_resource::<super::ReloadUiPending>();
    app.init_resource::<crate::target::Selection>();
    app.init_resource::<crate::names::NameCache>();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.init_resource::<crate::net::Reputations>();
    app.init_resource::<crate::ui_party::GroupState>();
    app.init_resource::<crate::ui_chat::ChatLog>();
    app.init_resource::<crate::ui_guild::GuildState>();
    app.add_message::<crate::creature_anim::SwingImpact>();
    app.add_message::<crate::net::WorldEnterCascadeMessage>();
    // The glue VM, which stands until the entry load and must hear none of the in-game events.
    super::setup_script(app.world_mut());
    (app, rx)
}

/// The unit feed's login one-shots (`PLAYER_ENTERING_WORLD`, the first `PLAYER_XP_UPDATE` and
/// `UPDATE_EXHAUSTION`) wait for the in-game UI instead of reaching a VM with no frames.
#[test]
fn the_login_one_shots_wait_for_the_in_game_ui() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("oneshot");
    let (mut app, _rx) = unit_feed_app();

    // A frame registered as FrameXML registers: in the glue VM that stands before the entry load,
    // then in the VM the load builds.
    let probe = |script: &benilla_ui::script::UiScript| {
        script
            .run(
                "EnteringWorldSeen = 0 \
                 local f = CreateFrame(\"Frame\") \
                 f:RegisterEvent(\"PLAYER_ENTERING_WORLD\") \
                 f:SetScript(\"OnEvent\", function() \
                     EnteringWorldSeen = EnteringWorldSeen + 1 end)",
            )
            .expect("probe frame");
    };
    probe(
        app.world()
            .non_send_resource::<benilla_ui::script::UiScript>(),
    );

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
    app.insert_resource(State::new(crate::client_state::ClientState::CharSelect));
    app.update();
    assert_eq!(
        seen(&app),
        0,
        "the drain's own frame: in-world wire, the glue VM, and no latch yet"
    );

    // The transition ran and the load is still owed: three frames inside the deferral window.
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    app.insert_resource(super::PendingEntryUiLoad);
    for frame in 1..=3 {
        app.update();
        assert_eq!(
            seen(&app),
            0,
            "frame {frame}: the feed must not spend PLAYER_ENTERING_WORLD on a UI-less VM"
        );
    }

    // The entry load has run, its VM in place of the glue's: the next feed delivers the one-shots.
    let ingame = benilla_ui::script::UiScript::new().expect("the entry's VM");
    probe(&ingame);
    app.world_mut().insert_non_send_resource(ingame);
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

/// An addon that fails to load without raising (a `.toc` naming a file the package lacks) reaches
/// no Lua error handler, so the `/errors` log (which collects off the handler) does not hold it:
/// the engine's record keeps it, and `Logs\FrameXML.log` names it as the reference's does.
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
        "## Interface: 11200\nBossnames\\BossNames.xml\nDatabase\\hash.lua\n",
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

    // 2. Not in the `/errors` log: it never reached the error handler.
    script
        .eval::<()>("BenillaScriptLog_Update() return nil")
        .expect("the window repaints without raising");
    assert!(
        !script
            .eval::<bool>(
                "for _, row in ipairs(BenillaScriptLog.rows) do \
                    if strfind(row.message, 'AaMissing', 1, 1) then return true end \
                 end \
                 return false"
            )
            .expect("eval"),
        "a failure no handler saw is not in the handler-fed log"
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

    // 4. Nothing in chat, and the reference's lines in `benilla-config/Logs/FrameXML.log`, each
    // stamped `M/D HH:MM:SS.mmm` and ended `\r\n`: the add-on's banner, its toc's, then each
    // miss by its arm (`0x6edaa0`, `0x704bc0`). The clean core and probe add nothing.
    let lines = world.resource::<crate::ui_chat::ChatLog>().pending_lines();
    assert!(
        !lines.iter().any(|l| l.contains("AaMissing")),
        "no chat line names the failure: {lines:?}"
    );
    let log = std::fs::read_to_string(tmp.join("benilla-config/Logs/FrameXML.log"))
        .expect("the load wrote FrameXML.log");
    let texts: Vec<&str> = log
        .split_terminator("\r\n")
        .map(|l| l.split_once("  ").expect("a stamp, then two spaces").1)
        .collect();
    assert_eq!(
        texts,
        [
            "Loading add-on AaMissing",
            "** Loading table of contents Interface\\AddOns\\AaMissing\\AaMissing.toc",
            "Couldn't open Interface\\AddOns\\AaMissing\\Bossnames\\BossNames.xml",
            "Error loading Interface\\AddOns\\AaMissing\\Database\\hash.lua",
        ],
        "{log}"
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
        .run("SlashCmdList = SlashCmdList or {} SLASH_ERRBOOM1 = '/errboom' SlashCmdList['ERRBOOM'] = function() error('every frame') end")
        .expect("register");
    for _ in 0..500 {
        script.run_slash_command("errboom", "");
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
/// is neither movable nor resizable. Both flags: the cache writes `X:`/`Y:` only for a movable
/// frame and `W:`/`H:` only for a resizable one.
fn place_a_window(world: &mut World) {
    world
        .get_non_send_resource_mut::<benilla_ui::script::UiScript>()
        .expect("a VM to place a window in")
        .run(
            "local f = CreateFrame(\"Frame\", \"LayoutProbe\") \
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
    for want in ["Frame: LayoutProbe", "X: 61", "W: 413", "H: 147"] {
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
    reload(&mut world, crate::client_state::ClientState::InWorld);

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
        !second.contains("LayoutProbe"),
        "a character who placed nothing must not inherit another's window:\n{second}"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A file of benilla's earlier anchor form seats the window through the loader, and the next save
/// writes it back in the reference's rect form, so a player's windows survive the change.
#[test]
fn an_anchor_form_file_is_restored_and_rewritten_as_the_rect() {
    use bevy::ecs::system::RunSystemOnce;

    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("layout-anchor-form");
    let mut world = booted_world();
    world.init_resource::<crate::ui_layout::LayoutFile>();

    log_in_as(&mut world, "Onehunter", 1);
    let path = crate::local_state::layout_character_path("Realm", "Onehunter")
        .expect("a layout path in the hermetic state folder");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "Frame: LayoutProbe\nW: 413\nH: 147\nPoint: BOTTOMLEFT - BOTTOMLEFT 61 29\n",
    )
    .unwrap();
    author_a_window(&mut world);
    world
        .run_system_once(crate::ui_layout::load_layout)
        .expect("the layout loader ran");
    let (w, h, left, _) = window_geometry(&world);
    assert_eq!(
        (w, h, left),
        (413.0, 147.0, 61.0),
        "seated from the anchor form"
    );

    super::end_ui_session(&mut world);
    let text = layout_cache("Onehunter").expect("the logout wrote the layout cache");
    assert_probe_row(&text);
    assert!(
        !text.contains("Point:"),
        "rewritten in the rect form:\n{text}"
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
            "local f = CreateFrame(\"Frame\", \"LayoutProbe\") \
             f:SetWidth(100) f:SetHeight(100) \
             f:SetPoint(\"BOTTOMLEFT\", 0, 0) \
             f:SetMovable(true) f:SetResizable(true)",
        )
        .expect("author the probe window");
}

/// The probe window's live geometry, as the player sees it: its size, left and top.
fn window_geometry(world: &World) -> (f32, f32, f32, f32) {
    world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("a VM")
        .eval::<(f32, f32, f32, f32)>(
            "return LayoutProbe:GetWidth(), LayoutProbe:GetHeight(), \
             LayoutProbe:GetLeft(), LayoutProbe:GetTop()",
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

    reload(&mut world, crate::client_state::ClientState::InWorld);
    author_a_window(&mut world);
    assert_ne!(
        window_geometry(&world),
        placed,
        "the rebuilt tree starts on its authored anchors — otherwise this proves nothing"
    );

    world
        .run_system_once(crate::ui_layout::load_layout)
        .expect("the layout loader ran");
    benilla_ui::assert_near!(
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
    const KNOWN: [&str; 1] = ["OnInputLanguageChanged"];

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
/// burst, so for that one frame the wire is in-world while the VM is still the glue one.
#[test]
fn the_ui_is_not_up_in_the_frame_between_the_wire_and_the_state() {
    let mut world = World::new();

    // The drain's frame: no load owed, and the state still says glue until next frame's transition.
    world.insert_resource(State::new(crate::client_state::ClientState::CharSelect));
    assert!(
        !run_ingame_ui_up(&mut world),
        "the wire is in-world a frame before the state is — the glue VM must stay out of reach"
    );

    // The transition ran: `OnEnter` parked the VM and armed the latch.
    world.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    world.insert_resource(super::PendingEntryUiLoad);
    assert!(
        !run_ingame_ui_up(&mut world),
        "the deferral window — a parked VM, and nothing to receive an event"
    );

    // The deferred load ran: the frame tree exists.
    world.remove_resource::<super::PendingEntryUiLoad>();
    assert!(
        run_ingame_ui_up(&mut world),
        "in the world with the in-game UI up — the first frame a feed may push"
    );

    // Leaving drops it again, before `end_ui_session`'s fresh glue VM can be fed anything.
    world.insert_resource(State::new(crate::client_state::ClientState::CharSelect));
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

/// Stock `ActionButton_OnLoad` paints its hotkey from `GetBindingKey` at load, so the key table,
/// the install's defaults, is seeded before any file runs; an addon's rebind takes at file scope.
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
    // The defaults are the install's `WTF\DefaultBindings.wtf`: none without one.
    assert_eq!(
        script
            .eval::<Option<String>>("BindProbeKey")
            .ok()
            .flatten()
            .as_deref(),
        benilla_formats::wow_data().map(|_| "M"),
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
        "…and the height is the 768-tall virtual base"
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
    crate::ui_unit::TestNames::english().insert_world(&mut world);
    log_in_as(&mut world, "Nelprifour", 0x2A);

    assert_eq!(
        probe_saw(&world).as_deref(),
        Some("Nelprifour"),
        "addon file scope reads the live character, as it always has"
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
        "…and the same for the other three fields the reference reads off that record"
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

    reload(&mut world, crate::client_state::ClientState::InWorld);
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

// ───────────── PLAYER_LOGIN fires from the world-enter cascade ─────────────

/// Records what file scope and the two cascade events see of `"player"`, in order.
const LOGIN_PROBE_LUA: &str = "\
LoginProbe = { order = \"\", fileExists = tostring(UnitExists(\"player\")),
    fileLevel = UnitLevel(\"player\") }
local f = CreateFrame(\"Frame\")
f:RegisterEvent(\"PLAYER_LOGIN\")
f:RegisterEvent(\"PLAYER_ENTERING_WORLD\")
f:SetScript(\"OnEvent\", function()
    if event == \"PLAYER_LOGIN\" then
        LoginProbe.order = LoginProbe.order .. \"LOGIN:\" .. tostring(UnitExists(\"player\"))
            .. \":\" .. UnitLevel(\"player\") .. \":\" .. UnitXP(\"player\") .. \" \"
    else
        LoginProbe.order = LoginProbe.order .. \"PEW \"
    end
end)
";

const LOGIN_PROBE_TOC: &str = "## Interface: 11200\nLoginProbe.lua\n";

/// Our own player as its create lands: level 17, 1234 of 5000 XP.
fn spawn_self_player(world: &mut World) -> Entity {
    use benilla_protocol::messages::{ObjectFields, ObjectType};
    // `PLAYER_XP` and `PLAYER_NEXT_LEVEL_XP`, 716 and 717 in the 1.12 descriptor.
    let fields = ObjectFields::from_pairs(&[
        (benilla_protocol::field::FIELD_UNIT_LEVEL, 17),
        (716, 1234),
        (717, 5000),
    ])
    .into_created(ObjectType::Player);
    world
        .spawn((
            crate::net::SelfPlayer,
            crate::net::Guid(1),
            crate::net::ObjectStore(fields),
        ))
        .id()
}

/// A probe global off the live VM, as a string.
fn login_probe(app: &App, field: &str) -> String {
    app.world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .eval::<Option<String>>(&format!("return tostring(LoginProbe.{field})"))
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// `PLAYER_LOGIN` has one fire site, `0x490959` in the world-enter cascade `0x4908c0`, which the
/// UI load only arms (`0x49011d`). A fresh login skips the load's own entry (`0x490166 je`) and
/// runs the cascade from the player's create (`0x5deb60`), so its handlers see the player, and
/// `PLAYER_ENTERING_WORLD` follows at once (`0x49096a`). A worldport loads no UI, so its cascade
/// finds the arm spent and fires 272 alone.
#[test]
fn player_login_fires_from_the_world_entry_with_the_player_seated() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_addon("login", "LoginProbe", LOGIN_PROBE_TOC, LOGIN_PROBE_LUA);
    let (mut app, _rx) = unit_feed_app();
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    app.insert_resource(roster_named("Probelogin", 1));
    super::load_ingame_ui_on_world_entry(app.world_mut());

    assert_eq!(
        login_probe(&app, "fileExists"),
        "nil",
        "a fresh login's file scope runs before the player object exists"
    );
    assert_eq!(
        login_probe(&app, "order"),
        "",
        "the load arms PLAYER_LOGIN; nothing fires it before the player is in the world"
    );

    let me = spawn_self_player(app.world_mut());
    app.update();
    assert_eq!(
        login_probe(&app, "order"),
        "LOGIN:1:17:1234 PEW ",
        "PLAYER_LOGIN sees the player's own unit, then PLAYER_ENTERING_WORLD fires at once"
    );
    app.update();
    assert_eq!(login_probe(&app, "order"), "LOGIN:1:17:1234 PEW ");

    // A cross-map worldport: the player object is destroyed and created again.
    app.world_mut().despawn(me);
    app.update();
    spawn_self_player(app.world_mut());
    app.update();
    assert_eq!(
        login_probe(&app, "order"),
        "LOGIN:1:17:1234 PEW PEW ",
        "a worldport re-fires PLAYER_ENTERING_WORLD and not PLAYER_LOGIN"
    );

    drop(app);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A model pane bound to `"player"` at `PLAYER_ENTERING_WORLD`, as `PaperDollFrame.lua:39` binds
/// `CharacterModelFrame`.
const PANE_PROBE_LUA: &str = "\
local doll = CreateFrame(\"PlayerModel\", \"PaneProbeDoll\", UIParent)
doll:RegisterEvent(\"PLAYER_ENTERING_WORLD\")
doll:SetScript(\"OnEvent\", function() this:SetUnit(\"player\") end)
";

/// The cascade runs from our own create, so `"player"` names us in its handlers whichever feed
/// seats the rest of the unit-token resolver: the stock windows bind their model panes there.
#[test]
fn the_world_enter_cascade_resolves_the_player_for_its_handlers() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_addon(
        "panebind",
        "PaneProbe",
        "## Interface: 11200\nPaneProbe.lua\n",
        PANE_PROBE_LUA,
    );
    let (mut app, _rx) = unit_feed_app();
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    app.insert_resource(roster_named("Probepane", 1));
    super::load_ingame_ui_on_world_entry(app.world_mut());
    spawn_self_player(app.world_mut());
    app.update();

    let s = app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>();
    let doll = s.frame_handle("PaneProbeDoll").expect("the probe loaded");
    assert_eq!(
        s.character_pane(doll).map(|c| c.guid),
        Some(1),
        "SetUnit(\"player\") at the cascade stores our own guid"
    );
    drop(app);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// On a `/reload` the player object stays in the world: `UI_Init` runs its file scope against it
/// and enters the cascade itself (`0x490168`), so `PLAYER_LOGIN` then `PLAYER_ENTERING_WORLD` fire
/// once each on the new VM, both seeing the player.
#[test]
fn a_reload_fires_player_login_once_against_the_live_player() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_addon("relogin", "LoginProbe", LOGIN_PROBE_TOC, LOGIN_PROBE_LUA);
    let (mut app, _rx) = unit_feed_app();
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    app.insert_resource(roster_named("Probelogin", 1));
    super::load_ingame_ui_on_world_entry(app.world_mut());
    spawn_self_player(app.world_mut());
    app.update();
    assert_eq!(login_probe(&app, "order"), "LOGIN:1:17:1234 PEW ");

    reload(app.world_mut(), crate::client_state::ClientState::InWorld);
    assert_eq!(
        (
            login_probe(&app, "fileExists"),
            login_probe(&app, "fileLevel")
        ),
        ("1".to_string(), "17".to_string()),
        "the reloaded file scope runs against the live player"
    );
    app.update();
    app.update();
    assert_eq!(
        login_probe(&app, "order"),
        "LOGIN:1:17:1234 PEW ",
        "the new VM gets PLAYER_LOGIN then PLAYER_ENTERING_WORLD, once each"
    );

    drop(app);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ───────────────────── The minimap mask is the process's, not the VM's ─────────────────────

/// The default mask and an addon's, as [`crate::minimap::seat_mask_for_test`] takes them.
const DEFAULT_MASK: Handle<Image> =
    bevy::asset::uuid_handle!("d0000000-0000-4000-8000-0000000000a1");
const ADDON_MASK: Handle<Image> = bevy::asset::uuid_handle!("d0000000-0000-4000-8000-0000000000a2");

/// The mask is one process-wide texture (`0xbc7968`), which `0x4ed810` loads with the default at
/// `0x48f590`, inside `InitializeGame` (`0x48f480`) at every world entry (`0x401570`); an addon's
/// lasts to the next entry. A relog enters again, and the circle is back whatever the new VM's
/// Minimap says.
#[test]
fn a_relog_restores_the_default_minimap_mask() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("mask-relog");
    let mut world = booted_world();
    crate::minimap::seat_mask_for_test(&mut world, DEFAULT_MASK, DEFAULT_MASK);

    log_in_as(&mut world, "Onehunter", 1);
    // What the mask feed does for an addon's `Minimap:SetMaskTexture`.
    crate::minimap::seat_mask_for_test(&mut world, DEFAULT_MASK, ADDON_MASK);
    super::end_ui_session(&mut world);
    assert_eq!(
        crate::minimap::mask_for_test(&world),
        Some(ADDON_MASK),
        "logging out leaves the texture where it was until the next entry"
    );

    log_in_as(&mut world, "Onehunter", 1);
    assert_eq!(
        crate::minimap::mask_for_test(&world),
        Some(DEFAULT_MASK),
        "the next world entry loads the default over the addon's mask"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// `ReloadUI` runs the shutdown `0x490bd0` and the initializer `0x48fbf0` (`0x495664`,
/// `0x495669`), neither of which reaches `0x4ed810`, so the mask an addon set stays.
#[test]
fn a_reload_keeps_the_minimap_mask_an_addon_set() {
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_probe("mask-reload");
    let mut world = booted_world();
    crate::minimap::seat_mask_for_test(&mut world, DEFAULT_MASK, DEFAULT_MASK);

    log_in_as(&mut world, "Onehunter", 1);
    crate::minimap::seat_mask_for_test(&mut world, DEFAULT_MASK, ADDON_MASK);
    reload(&mut world, crate::client_state::ClientState::InWorld);

    assert_eq!(
        crate::minimap::mask_for_test(&world),
        Some(ADDON_MASK),
        "a /reload does not load the default mask again"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ───────────────────── Addons load against the XML's buff anchors ─────────────────────

/// Records, at file scope and in `ADDON_LOADED`, what `BuffButton8` is anchored to.
const BUFF_PROBE_LUA: &str = "\
local function anchor()
    local _, rel = BuffButton8:GetPoint()
    return rel and rel:GetName() or \"nil\"
end
BuffProbe = { file = anchor() }
local f = CreateFrame(\"Frame\")
f:RegisterEvent(\"ADDON_LOADED\")
f:SetScript(\"OnEvent\", function()
    if arg1 == \"BuffProbe\" then BuffProbe.loaded = anchor() end
end)
";

/// `BuffFrame_OnLoad` does not pitch the buff rows. The stock `UIOptionsFrame.lua` does, in its
/// `VARIABLES_LOADED` arm (l.206), which `CGGameUI::Initialize` fires after every addon has
/// loaded (`0x4900a3`, then `0x4900b2`). So an addon's file scope and its `ADDON_LOADED` see
/// `BuffFrame.xml`'s anchor, `BuffButton8` under `BuffButton0`, and the pass moves it under
/// `TempEnchant1` once the saved variables are in.
#[test]
fn addons_load_against_the_buff_anchors_the_xml_gives() {
    let _data = benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tmp, _c, _h) = hermetic_addon(
        "buff-anchor",
        "BuffProbe",
        "## Interface: 11200\nBuffProbe.lua\n",
        BUFF_PROBE_LUA,
    );
    let mut world = booted_world();

    log_in_as(&mut world, "Onehunter", 1);
    let s = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("in-world VM");
    let probe = |field: &str| {
        s.eval::<String>(&format!("return tostring(BuffProbe.{field})"))
            .unwrap_or_default()
    };
    assert_eq!(
        (probe("file"), probe("loaded")),
        ("BuffButton0".to_string(), "BuffButton0".to_string()),
        "BuffFrame.xml's anchor (BuffFrame.xml:231-239) holds through the addon walk"
    );
    assert_eq!(
        s.eval::<String>("local _, rel = BuffButton8:GetPoint() return rel:GetName()")
            .as_deref()
            .ok(),
        Some("TempEnchant1"),
        "VARIABLES_LOADED then runs the stock pass (BuffFrame.lua:152-160)"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Some 4 MB the hog holds while `MemoryHog` does: 40,000 distinct 70-byte strings.
const HOG_LUA: &str = "\
MemoryHog = {}
for i = 1, 40000 do MemoryHog[i] = string.rep(\"x\", 64) .. i end
";

/// In the world with `MemoryHog` loaded and `scriptMemory` at `limit_kb`, the watch's resources
/// declared as its plugin declares them.
fn hog_world(tag: &str, limit_kb: &str) -> (World, std::path::PathBuf, EnvGuard, EnvGuard) {
    let (tmp, c, h) = hermetic_addon(
        tag,
        "MemoryHog",
        "## Interface: 11200\nMemoryHog.lua\n",
        HOG_LUA,
    );
    let mut world = booted_world();
    world.init_resource::<super::LeavingWorldArmed>();
    world.init_resource::<crate::ui_memory::ScriptMemoryWatch>();
    world.init_resource::<Messages<AppExit>>();
    world.insert_resource(Time::<bevy::time::Real>::default());
    world.spawn(crate::net::SelfPlayer);
    log_in_as(&mut world, "Onehunter", 1);
    let mut s = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
    s.register_cvars(crate::cvars::registered_pairs());
    // The limit as the saved file holds it: in the world the row is read-only (`0x48f566`), so
    // only the mirror sync moves it.
    s.set_cvar_host("scriptMemory", limit_kb);
    assert!(
        s.eval::<bool>("return MemoryHog ~= nil").unwrap(),
        "the hog loaded"
    );
    (world, tmp, c, h)
}

/// One frame of the watch, as `0x495590` runs it.
fn watch_frame(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(crate::ui_memory::watch_script_memory)
        .unwrap();
}

/// The text of the shown `MEMORY_EXHAUSTED` popup, if one is shown.
fn memory_popup(world: &World) -> Option<String> {
    world
        .non_send_resource::<benilla_ui::script::UiScript>()
        .eval::<Option<String>>(
            "local f = StaticPopup_Visible(\"MEMORY_EXHAUSTED\") \
             return f and getglobal(f .. \"Text\"):GetText()",
        )
        .unwrap()
}

/// What the whole state counts, in KB, after a full collection.
fn collected_kb(world: &World) -> usize {
    let s = world.non_send_resource::<benilla_ui::script::UiScript>();
    s.lua().gc_collect().unwrap();
    s.lua().used_memory() >> 10
}

/// `scriptMemory` at 1 MB, which the stock interface and the hog are well over: the first frame
/// shows the stock popup with the limit (`0x49563d`, `MEMORY_EXHAUSTED` of `GlobalStrings.lua`),
/// and the hog let go, the next hides it (`0x495656`).
#[test]
fn a_state_over_script_memory_shows_the_stock_warning_and_recovers() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut world, tmp, _c, _h) = hog_world("memory-over", "1024");

    assert_eq!(
        memory_popup(&world),
        None,
        "nothing before the frame's watch"
    );
    watch_frame(&mut world);
    assert_eq!(
        memory_popup(&world).as_deref(),
        Some(
            "The user interface is using more than 1MB of memory. Please disable some \
             interface add-ons and restart."
        ),
    );

    // Within the limit again: a limit above the stock interface, with the hog freed.
    world
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run("MemoryHog = nil")
        .unwrap();
    let stock_kb = collected_kb(&world);
    world
        .non_send_resource_mut::<benilla_ui::script::UiScript>()
        .set_cvar_host("scriptMemory", &(stock_kb + 1024).to_string());
    watch_frame(&mut world);
    assert_eq!(memory_popup(&world), None, "MEMORY_RECOVERED hides it");
    assert!(world.resource::<Messages<AppExit>>().is_empty());

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A limit the interface and the hog fit within says nothing. The stock interface alone, every
/// FrameXML file loaded and collected, counts some 10 MB here ([`crate::ui_memory`]), under a
/// third of the default's 48 MB.
#[test]
fn a_state_within_script_memory_shows_nothing() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut world, tmp, _c, _h) = hog_world("memory-within", "49152");
    let with_hog = collected_kb(&world);
    watch_frame(&mut world);
    assert_eq!(memory_popup(&world), None);

    world
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run("MemoryHog = nil")
        .unwrap();
    let stock_kb = collected_kb(&world);
    eprintln!("script memory: stock interface {stock_kb} KB, with the hog {with_hog} KB");
    assert!(
        with_hog >= stock_kb + 3 * 1024,
        "the hog holds its megabytes: {with_hog} KB against {stock_kb} KB"
    );
    assert!(
        stock_kb < 49_152 / 3,
        "the stock interface alone counts {stock_kb} KB, a third of the default or more"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}
