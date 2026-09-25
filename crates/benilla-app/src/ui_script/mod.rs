//! The UI-engine bridge: hosts [`benilla_ui::script::UiScript`] and feeds its
//! [`extract`](benilla_ui::script::UiScript::extract) output into the quad pass
//! ([`crate::ui_pass::UiQuads`]) every frame. The script side is WoW UI space (y-up, origin
//! bottom-left, a screen `768/uiScale` units tall); [`seam_scale`] carries quads ×s out and the
//! mouse ÷s in, and extraction flips to y-down window px.

use bevy::prelude::*;

use benilla_ui::script::{ActionSlot, ScriptValue, UiScript, UnitState};

use crate::ui_unit::UnitFeed;
use benilla_world::schedule::WorldStage;

/// The addon folder: discovery, manifests, the enable file and the load walk.
pub(crate) mod addons;
mod content;
pub(crate) mod extract;
mod input;
mod manifest;

/// The stock FrameXML this client runs off the player's own patch chain; its header is the rule.
mod reference_ui;

/// What the host remembers per VM, so a seed or change-memo never outlives the VM it was for.
mod session;

/// The feed gate: a UI feed's input-side early-out, audited by `WOW_FEED_GATE_CHECK=1`.
pub(crate) mod gate;

pub(crate) use session::VmMemo;

// Not test-only: the addon harness loads the whole shipped interface under each addon.
pub(crate) use manifest::load_default_ui;
pub(crate) use manifest::{load_font_registry, load_ingame_ui};

/// Whether the pointer is over any UI (the egui dev overlay or a player-UI frame), combined by
/// [`arbitrate_pointer_over_ui`]; gameplay reads it, so it is not the dev plugin's.
#[derive(Resource, Default)]
pub(crate) struct PointerOverUi(pub(crate) bool);

/// A synthetic pointer owns the mouse this frame (the drag probe's gesture), so
/// [`input::feed_ui_input`] skips its mouse half and never feeds the real cursor.
#[derive(Resource, Default)]
pub(crate) struct SyntheticPointer(pub(crate) bool);

/// A capture never reads the OS pointer: set for a whole `$WOW_CAPTURE`/`$WOW_CAPTURE_UI` run, so
/// its pixels never depend on where the real cursor rests.
#[derive(Resource, Default)]
pub(crate) struct CapturePointerPinned(pub(crate) bool);

/// The egui dev overlay's half of the pointer arbitration, written by `track_pointer_over_ui`;
/// defined here so a build without dev overlays still has the type.
#[derive(Resource, Default)]
pub(crate) struct EguiPointerOver(pub(crate) bool);

/// Whether the dev `I` inspector's world picking is armed; `player::control` and `target::click`
/// read it every frame, so it lives here, and the default (disarmed) is the player's.
#[derive(Resource, Default)]
pub(crate) struct InspectMode {
    pub(crate) enabled: bool,
}

/// One frame's UI-pass phase split in μs, as the `[ui-cost]` line prints it; owned by the
/// producer so it exists whether or not its reader, `hover_log`, is compiled in.
#[derive(Resource, Default, Clone)]
pub(crate) struct UiFrameCost {
    /// FontStrings the layout had the font engine shape this frame, and the first few by name.
    pub(crate) measured: usize,
    pub(crate) measured_texts: Vec<String>,
    pub(crate) tick: u128,
    pub(crate) resolve: u128,
    pub(crate) measure: u128,
    pub(crate) extract: u128,
    pub(crate) convert: u128,
    pub(crate) diff: u128,
    pub(crate) quads: usize,
    pub(crate) solves: u64,
    /// Full layout-graph derivations this frame, each about 30× a solve; expected zero.
    pub(crate) derives: u64,
    pub(crate) skipped: bool,
    /// Entries the splice re-converted this frame; 0 on a settled or full-conversion frame.
    pub(crate) spliced: usize,
    /// Entries the splice dropped (drawn last frame, not now); `[ui-cost] dropped=` prints it.
    pub(crate) dropped: usize,
}

/// Whether [`UiFrameCost`] is wanted this run: `WOW_UI_COST=1` or the hover recorder.
#[derive(Resource, Default)]
pub(crate) struct UiCostWanted(pub(crate) bool);

/// The mouse-enabled player-UI frame under the cursor; world pick and camera look yield to it.
#[derive(Resource, Default)]
pub(crate) struct PlayerUiHover(pub(crate) Option<u32>);

/// Who owns this frame's keys (the client's `DAT_00cf4dc8 != 0` gate), written in [`UiInput`] and
/// read after it: a focused EditBox takes every key, a shown keyboard-enabled frame only the key
/// it ate. Neither releases a held key: only the window deactivate (`0x514490`, from `0x493058`)
/// and the world-enter cascade (`0x5144c0`) clear the direction bits, so W keeps running while
/// you type.
#[derive(Resource, Default)]
pub(crate) struct UiKeyboardCapture {
    /// A focused EditBox eats every key (`0x77b35e` returns 1 on every path but alt-arrow).
    pub(crate) typing: bool,
    /// Keys a shown keyboard-enabled frame consumed this frame (the existence gate, `0x76b7d0`),
    /// per key so the map eating `M` suppresses no other binding; raw codes, as read.
    pub(crate) consumed: Vec<bevy::input::keyboard::KeyCode>,
    /// The arrows fall through: the focused box is in alt-arrow mode and ALT is up, so the
    /// reference declines them at `0x77b1c4` and `CGWorldFrame` runs their bindings.
    pub(crate) arrows_fall_through: bool,
}

/// A left press the UI consumed without hitting a frame (a payload dropped on empty world), which
/// world click-pick and orbit-start yield to. Over a world object the reference keeps an item
/// payload and runs SELECT instead.
#[derive(Resource, Default)]
pub(crate) struct PlayerUiClickConsumed(pub(crate) bool);

/// Whether the cursor carries a payload, a `Send` mirror. A click on sky with one held never
/// deselects: the reference gates its nothing-leg `SetSelection(0,0)` on no payload (`0x492d30`).
#[derive(Resource, Default)]
pub(crate) struct CursorPayloadHeld(pub(crate) bool);

/// What [`extract::tick_script`] hands [`extract::paint_script`]; with `live` false (no VM or
/// window) the paint does nothing.
#[derive(Resource, Default)]
pub(crate) struct UiPassState {
    pub(crate) live: bool,
    /// The 768-virtual seam scale this frame was ticked under.
    pub(crate) seam: f32,
    pub(crate) dpi: f32,
    /// The meter's first three phases, so one `[ui-cost]` row still describes one frame.
    pub(crate) us_tick: u128,
    pub(crate) us_resolve: u128,
    pub(crate) us_measure: u128,
    /// The layout counters before the tick, so the row's `solves` and `derives` cover both halves.
    pub(crate) solves_before: u64,
    pub(crate) derives_before: u64,
}

/// UI chrome under the pointer: [`PointerOverUi`] minus the nameplates, which take the mouse but
/// not the wheel. The reference has no such bit: a wheel notch walks the strata and falls through
/// any frame without `OnMouseWheel` to `CGWorldFrame`.
#[derive(Resource, Default)]
pub(crate) struct PointerOverUiPanel(pub(crate) bool);

/// The player-UI feed phase, after [`WorldStage::Net`] and ungated (`UnitFeed` is the gated
/// sub-phase): every push this frame's tick must see, as in the reference's frame, where packet
/// dispatch precedes every `FrameScript_SignalEvent` and both precede `OnUpdate`. Every VM holder
/// in `Update` is in this set or `.after(UiInput)`, as `game_plugins::schedule_tests` checks.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiFeed;

/// The player-UI paint pass, after the camera so world-anchored widgets use this frame's view.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiPaint;

/// The player-UI input pass (tick, hit-test, handlers), before [`WorldStage::Input`] so its
/// [`PlayerUiHover`] reaches `PointerOverUi` before `player::control` reads it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiInput;

/// The frame's atomic (`Instant`, `GetTime`) pair, the one base for mapping a store `Instant` onto
/// `GetTime`: `anchor`'s steps are the VM clock's deltas, so a mapped start is stable frame to
/// frame. Neither leg restarts with the VM: the reference's `GetTime` is `GetTickCount`, and stock
/// `Cooldown.lua:3` gates on `start > 0`, so a reset would hide every running cooldown's sweep.
#[derive(Resource)]
pub(crate) struct UiClock {
    /// `Time<Real>::last_update()` at the tick that produced [`Self::ui_now`].
    pub(crate) anchor: std::time::Instant,
    /// The VM clock after that tick, in seconds since this process started.
    pub(crate) ui_now: f64,
}

/// The pre-boot pair, replaced by [`lifecycle::seed_vm_clock`] at the first VM.
impl Default for UiClock {
    fn default() -> Self {
        Self {
            anchor: std::time::Instant::now(),
            ui_now: 0.0,
        }
    }
}

/// The FrameXML digest this process loads, which the corpus harness stamps on each report.
pub(crate) fn framexml_digest() -> String {
    content::digest()
}

/// Run a Lua chunk and log a failure: the form for any run whose `Result` is not consumed.
pub(crate) fn run_or_warn(script: &benilla_ui::script::UiScript, chunk: &str) {
    if let Err(e) = script.run(chunk) {
        warn!("ui_script: chunk failed: {e}");
    }
}

/// The reference's `uiScale` CVar: the VM's screen is `768/uiScale` units tall, so
/// `uiScale = 768/screenH` is pixel-perfect. Tests pin the `Default` 1.0.
#[derive(Resource)]
pub(crate) struct UiScaleCvar(pub(crate) f32);

impl Default for UiScaleCvar {
    fn default() -> Self {
        Self(1.0)
    }
}

/// The shipped `uiScale`. Deviation: a flat 0.9, because 1.0 reads oversized. With `useUiScale`
/// off, its default (`0x8430c0`), the reference sets `max(768/H, 0.9)` above 768 px tall and 1.0
/// at or below (`0x492f70`, on a mode set and from `0x4908ad`), so the two agree from ~853 px up.
pub(crate) const DEFAULT_UI_SCALE: f32 = 0.9;

/// `WOW_UI_SCALE=` if set, clamped to the dial's range, else [`DEFAULT_UI_SCALE`].
fn default_ui_scale() -> f32 {
    std::env::var("WOW_UI_SCALE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(0.5, 1.5))
        .unwrap_or(DEFAULT_UI_SCALE)
}

/// The seam scale `s`, window px per UI unit (`windowH/768 × uiScale`), used at every crossing of
/// the VM boundary; identity for a degenerate window (h ≤ 0, before winit).
pub(crate) fn seam_scale(window_h: f32, ui_scale: f32) -> f32 {
    if window_h > 0.0 {
        window_h / 768.0 * ui_scale
    } else {
        1.0
    }
}

/// The Lua UI host (a `NonSend` resource: an mlua VM is `!Send`) and its per-frame passes.
pub(crate) struct UiScriptPlugin;

/// `uiScale`'s change callback: the dial's own `[0.5, 1.5]` clamp.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut scale: ResMut<UiScaleCvar>) {
    if ev.is("uiScale") {
        scale.0 = ev.num().clamp(0.5, 1.5);
    }
}

/// `/console reloadUI`: the same deferred rebuild `ReloadUI()` and `/reload` queue.
fn console_reload_ui(world: &mut World, _args: &str) -> Vec<String> {
    match world.get_non_send_resource_mut::<UiScript>() {
        Some(mut script) => {
            script.queue_session_request(benilla_ui::script::SessionRequest::ReloadUi);
            Vec::new()
        }
        None => vec!["reloadUI: no interface to reload".to_string()],
    }
}

impl Plugin for UiScriptPlugin {
    fn build(&self, app: &mut App) {
        use crate::console::ConsoleCommandApp;
        app.add_observer(on_cvar);
        app.console_command("reloadUI", "Reload the interface.", console_reload_ui);
        // The quit root runs in `Last`: the close button's `AppExit` is written in `PostUpdate`.
        crate::shutdown::on_app_exit(app, shutdown_on_exit.into_configs());
        app.insert_resource(UiScaleCvar(default_ui_scale()))
            .init_resource::<UiFrameCost>()
            .init_resource::<crate::bindings::WheelNotches>()
            .init_resource::<UiCostWanted>()
            .init_resource::<PointerOverUi>()
            .init_resource::<SyntheticPointer>()
            .insert_resource(CapturePointerPinned(
                std::env::var_os("WOW_CAPTURE").is_some()
                    || std::env::var_os("WOW_CAPTURE_UI").is_some(),
            ))
            .init_resource::<EguiPointerOver>()
            .init_resource::<InspectMode>()
            .init_resource::<PlayerUiHover>()
            .init_resource::<UiKeyboardCapture>()
            .init_resource::<PlayerUiClickConsumed>()
            .init_resource::<CursorPayloadHeld>()
            .init_resource::<UiClock>()
            .init_resource::<AddOnIdentity>()
            // After `AssetSet::Open`: the VM's first load is the chain's `GlobalStrings.lua`.
            .add_systems(Startup, setup_script.after(benilla_assets::AssetSet::Open))
            // The in-game UI loads on world entry, as the reference's does, once the loading cover
            // has presented, so its ~0.5 s never stalls the cover's first frame.
            .add_systems(
                OnEnter(crate::char_select::ClientState::InWorld),
                lifecycle::arm_entry_ui_load,
            )
            // The shutdown tail: events, then writes, in the reference's order.
            .add_systems(
                OnExit(crate::char_select::ClientState::InWorld),
                end_ui_session,
            )
            // `init_` here and in `UiUnitPlugin`: either plugin may be built alone in a test.
            .init_resource::<LeavingWorldArmed>()
            .add_systems(Update, lifecycle::arm_leaving_world_on_self_create)
            // A queued `ReloadUI()` runs in `PreUpdate`, a frame after its drain (the reference's
            // deferral, `0x495590`) and before every `Update` seed or feed.
            .init_resource::<ReloadUiPending>()
            // Both are exclusive edges on the VM and must not interleave.
            .add_systems(
                PreUpdate,
                (run_pending_reload, lifecycle::run_pending_entry_load).chain(),
            )
            // The three UI phases in order: feeds after the net drain, the tick, then the paint.
            .configure_sets(
                Update,
                (
                    UiFeed.after(WorldStage::Net),
                    UiInput.before(WorldStage::Input),
                    UiPaint,
                )
                    .chain(),
            )
            // In the feed phase, so an `OnUpdate` reads this frame's `GetFramerate()`.
            .add_systems(Update, feed_framerate.in_set(UiFeed))
            // The input pass hit-tests the rects the tick just resolved, and runs in-world only.
            .init_resource::<UiPassState>()
            .init_resource::<PointerOverUiPanel>()
            .add_systems(
                Update,
                (
                    extract::tick_script,
                    input::feed_ui_input.in_set(crate::char_select::InWorldGated),
                )
                    .chain()
                    .in_set(UiInput)
                    // So a key a focused box consumed never also fires a binding.
                    .before(crate::bindings::BindingSet),
            )
            // After the plate driver seats this frame's nameplates, and before the append lane
            // fills the widget slot this pass parks.
            .add_systems(
                Update,
                extract::paint_script
                    .in_set(UiPaint)
                    .after(crate::vplates::VPlateSet)
                    .before(crate::ui_pass::UiQuadAppend),
            )
            // After the hover is known and before gameplay reads `PointerOverUi`.
            .add_systems(
                Update,
                arbitrate_pointer_over_ui
                    .after(UiInput)
                    .before(WorldStage::Input),
            );

        // A UI capture's synthetic unit snapshots, after the real feed so they win.
        app.add_systems(
            Update,
            demo_unit_feed
                .in_set(UiFeed)
                .after(UnitFeed)
                .run_if(capture_ui_active),
        );
    }
}

/// Whether a capture includes the player UI: world baselines stay UI-free unless
/// [`crate::run_mode::capture_ui_opted_in`], which the UI load and window size also read.
fn capture_ui_active(capture: Option<Res<crate::run_mode::CaptureMode>>) -> bool {
    capture.is_some() && crate::run_mode::capture_ui_opted_in()
}

/// `PointerOverUi = egui dev overlay ∨ player-UI hover`; the dev half is optional.
fn arbitrate_pointer_over_ui(
    egui: Option<Res<EguiPointerOver>>,
    hover: Res<PlayerUiHover>,
    plate_hover: Res<crate::vplates::PlateHover>,
    mut over: ResMut<PointerOverUi>,
    mut panel: ResMut<PointerOverUiPanel>,
) {
    // No cinematic term: the hit test finds the full-screen, mouse-enabled `CinematicFrame`.
    over.0 = egui.is_some_and(|e| e.0) || hover.0.is_some();
    // Chrome excludes the plates; the plate hover is last frame's, as the plate driver runs later.
    panel.0 = over.0 && plate_hover.0.is_none();
}

/// The session lifecycle: the VM's birth, identity, death and reload.
mod lifecycle;
pub(crate) use lifecycle::{
    end_ui_session, ingame_ui_up, run_pending_reload, setup_script, AddOnIdentity,
    LeavingWorldArmed, PendingEntryUiLoad, ReloadUiPending,
};
// Test-only: other modules' tests consume these, and a plain re-export would warn unused.
#[cfg(test)]
pub(crate) use lifecycle::load_ingame_ui_on_world_entry;
use lifecycle::shutdown_on_exit;
#[cfg(test)]
pub(crate) use lifecycle::{
    finish_ui_load, is_emote_token_line, seat_from_roster, shutdown_ui_state,
};

/// `GetFramerate()`'s host half: FPS on `Time<Real>`, smoothed by a one-second pole; how the
/// reference averages it is untraced.
fn feed_framerate(
    script: Option<NonSendMut<UiScript>>,
    time: Res<Time<bevy::time::Real>>,
    mut smoothed: Local<f64>,
) {
    let Some(mut script) = script else {
        return;
    };
    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return; // the first frame, or a paused one
    }
    let instant = 1.0 / dt;
    // One-pole IIR with a 1 s time constant, frame-rate independent.
    let alpha = (dt / 1.0).min(1.0);
    *smoothed = if *smoothed <= 0.0 {
        instant
    } else {
        *smoothed + alpha * (instant - *smoothed)
    };
    script.set_framerate(*smoothed);
}

/// Synthetic player and target snapshots for a server-less UI capture, and its first events once.
fn demo_unit_feed(script: Option<NonSendMut<UiScript>>, mut fired: Local<VmMemo<bool>>) {
    /// The synthetic target's guid: a creature high part; only its distinctness matters.
    const DEMO_TARGET_GUID: u64 = 0xF130_0000_0000_0001;

    let Some(mut script) = script else {
        return;
    };
    // Session-keyed: a fresh VM needs the one-shot events and bar seeds again.
    let fired = fired.get(&script);
    // Debug (`WOW_CAPTURE_REHOVER=1`): re-fire the world hover every frame, a flapping raycast.
    if std::env::var("WOW_CAPTURE_REHOVER").as_deref() == Ok("1") {
        script.world_tooltip_unit("mouseover");
    }
    script.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Benilla".into()),
            health: 72,
            max_health: 100,
            level: 12,
            power_type: 0, // mana
            power: 45,
            max_power: 80,
            dead: false,
            reaction: 0, // own avatar: no reaction to itself
            // Race/class so the ui-char capture's level line reads "Level 12 Night Elf Warrior".
            race: Some("Night Elf".into()),
            race_file: Some("NightElf".into()),
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            sex: 2,
            is_player: true,
            player_controlled: true,
            ..Default::default()
        }),
    );
    script.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Young Wolf".into()),
            health: 30,
            max_health: 50,
            level: 3,
            // A powerless beast (maxPower 0): exercises the power-bar-hide path in the capture.
            power_type: 0,
            power: 0,
            max_power: 0,
            dead: false,
            reaction: 4, // neutral: yellow (UnitReactionColor[4])
            // A beast: no race/class tokens (UnitRace/UnitClass report the absent shape).
            race: None,
            race_file: None,
            class: None,
            class_file: None,
            sex: 0,
            // The tooltip's level line reads "Level 3 Beast".
            creature_type_name: Some("Beast".into()),
            // `GetComboPoints` reports only points banked on the current target's guid.
            guid: DEMO_TARGET_GUID,
            ..Default::default()
        }),
    );
    // The combo dots need a rogue and banked points, seeded only for `ui-combopoints`.
    if std::env::var("WOW_CAPTURE").as_deref() == Ok("ui-combopoints") {
        script.set_player_req_state(benilla_ui::script::PlayerReqState {
            level: 12,
            class_id: 4, // rogue
            ..Default::default()
        });
        script.set_combo_points(4, DEMO_TARGET_GUID);
        script.fire_event("PLAYER_COMBO_POINTS", vec![]);
    }
    if !*fired {
        // Battle-stance page slots (actions 73..); slot 80 is a consumable stack of 5, since stock
        // `ActionButton_UpdateCount` shows only a consumable's count (`ActionButton.lua:287`).
        script.set_bonus_bar_offset(1);
        // A macro on button 4; a capture has no character for `ui_macro::load_macros` to read.
        script.set_macros(benilla_ui::script::MacroState {
            account: vec![benilla_ui::script::MacroView {
                name: "spawn".into(),
                texture: Some("Interface\\Icons\\Ability_Racial_Cannibalize".into()),
                body: ".spawn 16032".into(),
                local_only: false,
            }],
            character: Vec::new(),
        });
        for (action, icon, kind, id, count) in [
            (
                73,
                "Interface\\Icons\\Ability_SteelMelee",
                0x00u8,
                100u32,
                0u32,
            ),
            (74, "Interface\\Icons\\Ability_Rogue_Ambush", 0x00, 101, 0),
            (
                75,
                "Interface\\Icons\\Ability_Warrior_BattleShout",
                0x00,
                102,
                0,
            ),
            (
                76,
                "Interface\\Icons\\Ability_Racial_Cannibalize",
                0x40,
                1,
                0,
            ),
            (80, "Interface\\Icons\\INV_Misc_Food_16", 0x80, 117, 5),
            (84, "Interface\\Icons\\Spell_Holy_SealOfMight", 0x00, 103, 0),
            // BottomLeft is actions 61..72, BottomRight 49..60; an empty multibar well hides.
            (
                61,
                "Interface\\Icons\\Spell_Nature_Regenerate",
                0x00,
                104,
                0,
            ),
            (62, "Interface\\Icons\\Spell_Shadow_Curse", 0x00, 105, 0),
            (
                72,
                "Interface\\Icons\\Spell_Frost_FrostBolt02",
                0x00,
                106,
                0,
            ),
            (49, "Interface\\Icons\\Spell_Fire_FlameBolt", 0x00, 107, 0),
            (60, "Interface\\Icons\\Spell_Holy_Heal", 0x00, 108, 0),
        ] {
            script.set_action(
                action,
                Some(ActionSlot {
                    texture: Some(icon.into()),
                    kind,
                    action: id,
                    count,
                    // The seed's only item slot is the food stack, which is consumable.
                    consumable: kind == 0x80,
                }),
            );
            // Server-less a slot has no usable state; a live bar's resting state is usable.
            script.set_action_state(
                action,
                Some(benilla_ui::script::ActionState {
                    usable: true,
                    ..Default::default()
                }),
            );
        }
        // The stance bar: battle active (bonus offset 1), defensive uncastable, berserker cooling.
        script.set_shapeshift_forms(vec![
            benilla_ui::script::ShapeshiftFormView {
                spell_id: 2457,
                texture: Some("Interface\\Icons\\Ability_Warrior_OffensiveStance".into()),
                name: "Battle Stance".into(),
                active: true,
                castable: true,
                cooldown: None,
            },
            benilla_ui::script::ShapeshiftFormView {
                spell_id: 71,
                texture: Some("Interface\\Icons\\Ability_Warrior_DefensiveStance".into()),
                name: "Defensive Stance".into(),
                active: false,
                castable: false,
                cooldown: None,
            },
            benilla_ui::script::ShapeshiftFormView {
                spell_id: 2458,
                texture: Some("Interface\\Icons\\Ability_Racial_Avatar".into()),
                name: "Berserker Stance".into(),
                active: false,
                castable: true,
                cooldown: Some((900, 1500, true)),
            },
        ]);
        script.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
        // 70% XP, set before `PLAYER_ENTERING_WORLD` so the bar's first update reads it.
        script.set_player_xp(4200, 6000);
        script.fire_event("PLAYER_ENTERING_WORLD", vec![]);
        // The bottom multibars ship off and a capture has no toggle byte, so raise them as the
        // Options rows do; `WOW_DEMO_BOTTOM_BARS=0` leaves them down for the stance shelf art,
        // which `ShapeshiftBar_UpdatePosition` hides under the bottom-left bar.
        if std::env::var("WOW_DEMO_BOTTOM_BARS").as_deref() != Ok("0") {
            let _ = script.run(
                "SHOW_MULTI_ACTIONBAR_1 = 1 SHOW_MULTI_ACTIONBAR_2 = 1 \
                 if MultiActionBar_Update then MultiActionBar_Update() end",
            );
        }
        script.fire_event("PLAYER_XP_UPDATE", vec![]);
        for token in ["player", "target"] {
            script.fire_event("UNIT_HEALTH", vec![ScriptValue::Str(token.into())]);
        }
        script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
        *fired = true;
    }
}

/// A test font engine: every character `.0` wide, 12 tall per line, greedily wrapped.
#[cfg(test)]
pub(crate) struct FixedWidthFont(pub(crate) f32);

#[cfg(test)]
impl benilla_ui::script::TextMeasure for FixedWidthFont {
    fn measure(&mut self, req: &benilla_ui::script::MeasureRequest) -> (f32, f32, f32) {
        let natural = req.text.chars().count() as f32 * self.0;
        match req.wrap_width {
            Some(w) if w > 0.0 && natural > w => (w, 12.0 * (natural / w).ceil(), natural),
            _ => (natural, 12.0, natural),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_ui;

/// The chat loader's two login events (`0x498a60`): `UPDATE_CHAT_WINDOWS` once, then
/// `UPDATE_CHAT_COLOR` per registry entry.
#[cfg(test)]
pub(crate) fn fire_chat_login(s: &mut benilla_ui::script::UiScript) {
    // `FCF_OnUpdate` reads `UIOptionsFrame:IsShown()` every frame; stand in a closed one.
    s.run("if not UIOptionsFrame then UIOptionsFrame = CreateFrame('Frame') UIOptionsFrame:Hide() end")
        .expect("the options stand-in");
    s.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    let renorm = |b: u8| f64::from(b as f32 * (1.0f32 / 255.0f32));
    for entry in s.chat_colors() {
        s.fire_event(
            "UPDATE_CHAT_COLOR",
            vec![
                benilla_ui::script::ScriptValue::Str(entry.name),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[0])),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[1])),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[2])),
            ],
        );
    }
}

/// The `benilla_formats::TokenContext::text` seam: a `GlobalStrings` key, its `%d` holes filled.
pub(crate) fn token_text(
    script: &benilla_ui::script::UiScript,
) -> impl Fn(&str, &[i64]) -> Option<String> + '_ {
    |key: &str, args: &[i64]| {
        let template = benilla_ui::strings::global(script.lua(), key)?;
        let args: Vec<_> = args
            .iter()
            .map(|n| benilla_ui::strings::Arg::D(*n))
            .collect();
        Some(benilla_ui::strings::fill(&template, &args))
    }
}

/// [`test_ui::load_ui`] for a test module outside `ui_script`, such as `ui_action::feed_tests`.
#[cfg(test)]
pub(crate) fn load_ui_for_test(script: &benilla_ui::script::UiScript, entry: &str) -> usize {
    test_ui::load_ui(script, entry)
}

#[cfg(test)]
mod cinematic_tests;

#[cfg(test)]
mod cast_tests;

#[cfg(test)]
mod mirror_timer_tests;

#[cfg(test)]
mod combat_text_tests;

#[cfg(test)]
mod combo_frame_tests;

#[cfg(test)]
mod unit_frame_tests;

#[cfg(test)]
mod unit_popup_tests;

#[cfg(test)]
mod chat_resize_tests;

#[cfg(test)]
mod dropdown_tests;

#[cfg(test)]
mod action_bar_tests;

#[cfg(test)]
mod exp_bar_tests;

#[cfg(test)]
mod multibar_stance_tests;

#[cfg(test)]
mod pet_bar_tests;

#[cfg(test)]
mod pet_frame_tests;

#[cfg(test)]
mod pet_stable_tests;

#[cfg(test)]
mod tot_frame_tests;

#[cfg(test)]
mod micro_menu_tests;

#[cfg(test)]
mod perf_bar_tests;

#[cfg(test)]
mod panel_tests;

#[cfg(test)]
mod fade_tests;

#[cfg(test)]
mod merchant_tests;

#[cfg(test)]
mod money_frame_tests;

#[cfg(test)]
mod faux_scroll_tests;

/// The stock UIPanelTemplates and OptionsFrameTemplates kit, driven the way an addon drives it.
#[cfg(test)]
mod panel_template_tests;

/// The return-shape gate: `reference/1.12-shapes.tsv` against what this client answers.
#[cfg(test)]
mod shape_gate;

/// The event argument-shape gate: every fire site against `reference/1.12-events.tsv`.
#[cfg(test)]
mod event_shape_gate;

/// The verb-fired event gate, against `reference/1.12-verb-events.tsv`.
#[cfg(test)]
mod verb_event_gate;

/// The stock BasicControls.xml, which benilla never calls, entered from Lua as an addon does.
#[cfg(test)]
mod basic_controls_tests;

#[cfg(test)]
mod color_picker_tests;

/// `UIParent.xml`'s loose addon-facing helpers, such as `MouseIsOver`.
#[cfg(test)]
mod uiparent_tests;

#[cfg(test)]
mod trainer_tests;

#[cfg(test)]
mod bank_tests;

#[cfg(test)]
mod taxi_tests;

#[cfg(test)]
mod loot_tests;

#[cfg(test)]
mod group_loot_tests;

#[cfg(test)]
mod chat_tests;

/// The chat bubble's `UIMenu` kit driven as a menu: the rows' label and shortcut anchoring.
#[cfg(test)]
mod ui_menu_tests;

/// The chat tab's options menu end to end, over the whole dropdown and colour-picker stack.
#[cfg(test)]
mod chat_options_tests;

#[cfg(test)]
mod bag_tests;
#[cfg(test)]
mod resolve_bench;

#[cfg(test)]
mod tooltip_anchor_tests;

#[cfg(test)]
mod tooltip_compare_tests;

/// `GameTooltipTemplate` as an addon sees it, through `inherits=` and `CreateFrame`.
#[cfg(test)]
mod tooltip_template_tests;

#[cfg(test)]
mod escape_tests;

#[cfg(test)]
mod game_menu_tests;

#[cfg(test)]
mod macro_tests;

// `pub(crate)` for `harness`, `on_page` and `label`, which the bindings dispatch tests use.
#[cfg(test)]
pub(crate) mod keybindings_tests;
#[cfg(test)]
mod options_tests;

#[cfg(test)]
mod delete_item_tests;
#[cfg(test)]
mod instance_tests;

#[cfg(test)]
mod static_popup_tests;

#[cfg(test)]
mod binder_tests;

#[cfg(test)]
mod summon_tests;

#[cfg(test)]
mod talent_wipe_tests;

#[cfg(test)]
mod death_tests;

#[cfg(test)]
mod duel_tests;

#[cfg(test)]
mod enchant_confirm_tests;

/// The `toplevel`, mouse and `id` flags of the whole loaded tree against the reference.
#[cfg(test)]
mod frame_flag_gate;

#[cfg(test)]
mod friends_tests;

/// The four guild windows, over a Lua stand-in for the guild engine API.
#[cfg(test)]
mod guild_tests;

/// The GM help window and its ticket toast, over a pushed `GMTicketCategory.dbc` catalog.
#[cfg(test)]
mod help_frame_tests;

/// The guild-charter registrar and petition sheet, over a Lua stand-in for the charter API.
#[cfg(test)]
mod petition_tests;

/// The social window's fourth tab: the raid pane and its grid, over a pushed raid roster.
#[cfg(test)]
mod raid_tests;

#[cfg(test)]
mod quest_share_tests;

#[cfg(test)]
mod quest_tests;

#[cfg(test)]
mod quest_timer_tests;

#[cfg(test)]
mod battlefield_tests;

/// The battle map: the stock `Blizzard_BattlefieldMinimap` addon, demand-loaded as SHIFT-M does.
#[cfg(test)]
mod battlefield_minimap_tests;

#[cfg(test)]
mod tutorial_tests;

#[cfg(test)]
mod durability_tests;
// `pub(crate)` for `harness`, `push` and `row`, which `perf::hud`'s test uses.
#[cfg(test)]
pub(crate) mod world_state_tests;

#[cfg(test)]
mod screenshot_tests;

#[cfg(test)]
mod questlog_tests;

#[cfg(test)]
mod character_tests;

#[cfg(test)]
mod skills_frame_tests;

#[cfg(test)]
mod reputation_frame_tests;

#[cfg(test)]
mod honor_frame_tests;

#[cfg(test)]
mod pet_paperdoll_tests;

#[cfg(test)]
mod inspect_tests;

#[cfg(test)]
mod dressup_tests;

#[cfg(test)]
mod minimap_tests;

#[cfg(test)]
mod spellbook_tests;

#[cfg(test)]
mod buff_tests;

#[cfg(test)]
mod target_aura_tests;

#[cfg(test)]
mod zone_text_tests;

#[cfg(test)]
mod errors_tests;

#[cfg(test)]
mod shipped_xml_tests;

#[cfg(test)]
mod bottom_hud_tests;

#[cfg(test)]
mod bagnon_render_tests;

/// A hunter's Quiver addon publishes its global functions, end to end.
#[cfg(test)]
mod quiver_tests;

/// Teardown at the character screen and a genuine second load at the next login.
#[cfg(test)]
mod world_entry_tests;

/// The guild tabard designer: the stock `TabardFrame.xml` off the chain.
#[cfg(test)]
mod tabard_tests;
/// The stock world map: its POI pool, unit blips, player arrow and full-screen quads.
#[cfg(test)]
mod world_map_tests;

#[cfg(test)]
mod seam_scale_tests {
    use super::seam_scale;

    #[test]
    fn seam_scale_is_the_768_base_times_the_dial() {
        // Identity at the design height, proportional elsewhere.
        assert_eq!(seam_scale(768.0, 1.0), 1.0);
        assert_eq!(seam_scale(1536.0, 1.0), 2.0);
        // The dial multiplies it.
        assert_eq!(seam_scale(768.0, 0.9), 0.9);
        // The reference's pixel-perfect setting: uiScale = 768/screenH → 1 px per UI unit.
        assert!((seam_scale(1080.0, 768.0 / 1080.0) - 1.0).abs() < 1e-6);
        // A degenerate (pre-winit) window is identity, never a division blow-up.
        assert_eq!(seam_scale(0.0, 0.9), 1.0);
    }
}

#[cfg(test)]
mod pointer_arbiter_tests {
    use super::*;

    /// A world to run the arbiter in: its inputs and its two outputs, nothing else.
    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<PlayerUiHover>()
            .init_resource::<crate::vplates::PlateHover>()
            .init_resource::<PointerOverUi>()
            .init_resource::<PointerOverUiPanel>()
            .add_systems(Update, arbitrate_pointer_over_ui);
        app
    }

    #[test]
    fn a_hovered_plate_is_ui_but_not_chrome() {
        let mut app = app();
        let plate = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<PlayerUiHover>().0 = Some(7);
        app.world_mut()
            .resource_mut::<crate::vplates::PlateHover>()
            .0 = Some(plate);
        app.update();
        assert!(
            app.world().resource::<PointerOverUi>().0,
            "the plate takes the pointer"
        );
        assert!(
            !app.world().resource::<PointerOverUiPanel>().0,
            "…and the WHEEL looks straight through it"
        );
    }

    /// The camera reads the raw flag (`0x7662c0` gives a mouse-down to exactly one frame, the
    /// plate); the wheel reads chrome, walking past a frame that only takes the mouse.
    #[test]
    fn the_camera_yields_to_a_plate_and_the_wheel_does_not() {
        let mut app = app();
        let plate = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<PlayerUiHover>().0 = Some(7);
        app.world_mut()
            .resource_mut::<crate::vplates::PlateHover>()
            .0 = Some(plate);
        app.update();
        // `latch_world_mouse` reads this one: the press is not the world's, so no look starts.
        assert!(
            app.world().resource::<PointerOverUi>().0,
            "the camera must yield the press to the plate"
        );
        // …and `bindings`' wheel branch reads this one.
        assert!(
            !app.world().resource::<PointerOverUiPanel>().0,
            "the wheel must still reach the world over a plate"
        );
    }

    #[test]
    fn a_hovered_panel_is_both() {
        let mut app = app();
        app.world_mut().resource_mut::<PlayerUiHover>().0 = Some(7);
        app.update();
        assert!(app.world().resource::<PointerOverUi>().0);
        assert!(app.world().resource::<PointerOverUiPanel>().0);
    }

    #[test]
    fn no_hover_is_neither() {
        let mut app = app();
        app.update();
        assert!(!app.world().resource::<PointerOverUi>().0);
        assert!(!app.world().resource::<PointerOverUiPanel>().0);
    }
}
