//! The select screen's input — clicks (single selects, double enters — the ref's
//! `CharacterSelectButton_OnClick`/`OnDoubleClick`), the bottom buttons, the keyboard (Enter =
//! enter world, Escape = exit, arrows cycle with wrap), and the model rotation (drag anywhere at
//! the pinned `CHARACTER_ROTATION_CONSTANT` 0.6°/px; hold the rotate pair at ±2°/frame). Sounds
//! are the ref's exact set — a bare selection click is silent.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;

use crate::net::CharPick;
use crate::portrait::GluePreview;
use crate::sound::GlueSound;

use super::dialog::DeleteDialog;
use super::screen::SelectAction;
use super::{class_name, send_pick, ClientState, Roster};

use crate::glue::{drag_yaw, ROTATE_RATE};

/// The double-click window (the ref rides the OS notion; this is the conventional interval).
const DOUBLE_CLICK_SECS: f32 = 0.4;

/// Button presses + the keyboard: selection, enter world, delete (opens the dialog), create,
/// back/escape (return to the login screen — decision 0539, retiring 0465 §6's exit-the-client
/// collapse), arrow-key cycling. Inert while the delete dialog is up (it owns the keyboard and
/// sits over the buttons).
#[allow(clippy::type_complexity)]
pub(super) fn select_input(
    buttons: Query<(Entity, &SelectAction)>,
    clicks: Res<crate::glue::GlueClicks>,
    keys: Res<ButtonInput<KeyCode>>,
    mut roster: ResMut<Roster>,
    mut realms: ResMut<crate::realm_select::Realms>,
    pick: Res<CharPick>,
    mut dialog: ResMut<DeleteDialog>,
    mut panel: ResMut<super::addons::AddonsPanel>,
    choice: Res<crate::net::RealmChoice>,
    mut next: ResMut<NextState<ClientState>>,
    mut sounds: MessageWriter<GlueSound>,
    mut intent: ResMut<crate::login::LoginIntent>,
    glue_dialog: Res<crate::glue::dialog::GlueDialog>,
    time: Res<Time>,
    mut last_click: Local<Option<(usize, f32)>>,
) {
    // A modal owns the input while it is up — the delete confirm, the AddOns list, the realm
    // list (which stands over this screen rather than replacing it), or the shared glue dialog
    // (a refused character login is said in that one, and its Okay must not double as this
    // screen's Enter World / Escape).
    if dialog.open || panel.open || realms.shown || glue_dialog.is_open() {
        return;
    }
    let now = time.elapsed_secs();
    let mut enter_world = false;
    let mut back_to_login = false;
    // Buttons fire on the RELEASE, over the button that took the press — the reference's stock
    // `<Button>` click mask is `LeftButtonUp` alone (1533, `crate::glue::glue_clicks`).
    for (entity, action) in &buttons {
        if !clicks.hit(entity) {
            continue;
        }
        match *action {
            SelectAction::Row(i) if i < roster.chars.len() => {
                // Single click selects (silently, like the ref); a second click on the same row
                // within the window is the double-click → enter world.
                let double =
                    last_click.is_some_and(|(row, at)| row == i && now - at < DOUBLE_CLICK_SECS);
                *last_click = Some((i, now));
                // Selecting is gated on the row actually CHANGING — the ref's own
                // `CharacterSelectButton_OnClick` is that gate and nothing else, so the row you
                // are already on is not re-selected and keeps the facing you dragged into it
                // (`Roster::click_row`).
                roster.click_row(i);
                // `OnDoubleClick` runs the same gated select and then enters the world
                // unconditionally — it does not re-test what was selected before.
                if double {
                    enter_world = true;
                }
            }
            SelectAction::EnterWorld => enter_world = true,
            SelectAction::Delete => {
                // The ref plays the click and opens the typed-confirm dialog for the selection
                // (a no-selection click just plays the sound — the ref's `selectedIndex > 0` gate).
                sounds.write(GlueSound("gsCharacterSelectionDelCharacter"));
                if let Some(c) = roster.selected_char() {
                    dialog.open_for(c.guid, c.name.clone(), c.level, class_name(c.class));
                }
            }
            SelectAction::CreateChar => {
                sounds.write(GlueSound("gsCharacterSelectionCreateNew"));
                next.set(ClientState::CharCreate);
            }
            SelectAction::Addons => {
                // The reference plays `gsCharacterSelectionOpen`-family click here; ours reuses
                // the create-screen open sound rather than inventing a name the client lacks.
                sounds.write(GlueSound("gsCharacterSelectionCreateNew"));
                // The whole roster rides along so the panel's "Configure Addons For:" dropdown
                // can fan out over every character. The realm resolves exactly
                // as `ui_macro::identity`'s does — same fallback, so the enable files the panel
                // writes stay keyed the way the world-entry walk reads them (1191 §7).
                let realm = roster
                    .realm
                    .as_ref()
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| "Realm".into());
                let chars = roster.chars.iter().map(|c| c.name.clone()).collect();
                panel.open_for(realm, chars);
            }
            SelectAction::Back => back_to_login = true,
            // The reference's `CHANGE_REALM`: raise the realm list **over** this screen. Nothing
            // is torn down and nothing is disconnected — the character park serves the list in
            // place and only leaves when a realm is picked, so Cancel puts the player back here
            // with the session they never left. The pending pick still has to go, or the fresh
            // roster on the OTHER realm would be auto-answered with a guid from this one.
            SelectAction::ChangeRealm => {
                sounds.write(GlueSound("gsLoginChangeRealmOK"));
                roster.pending_pick = None;
                crate::realm_select::open_over_char_select(&mut realms, &choice);
            }
            _ => {}
        }
    }

    // Keyboard: Enter = enter world; Escape = back to login; Up/Left + Down/Right cycle with wrap.
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
        enter_world = true;
    }
    if keys.just_pressed(KeyCode::Escape) {
        back_to_login = true;
    }
    if back_to_login {
        // The ref's Back leaves select for the login screen: drop the parked
        // session (the IO thread re-parks pre-logon) and forget both intents — a deliberate Back
        // must not auto-relogin.
        sounds.write(GlueSound("gsCharacterSelectionExit"));
        roster.pending_pick = None;
        intent.clear();
        let _ = pick.0.send(crate::net::CharRequest::Abandon);
        next.set(ClientState::Login);
    }
    let n = roster.chars.len();
    if n > 1 {
        let back = keys.just_pressed(KeyCode::ArrowUp) || keys.just_pressed(KeyCode::ArrowLeft);
        let fwd = keys.just_pressed(KeyCode::ArrowDown) || keys.just_pressed(KeyCode::ArrowRight);
        if back || fwd {
            let cur = roster.selected().unwrap_or(0);
            let sel = if back {
                (cur + n - 1) % n
            } else {
                (cur + 1) % n
            };
            roster.select(Some(sel));
        }
    }

    if enter_world && roster.pending_pick.is_none() {
        let target = roster.selected_char().map(|c| (c.guid, c.name.clone()));
        if let Some((guid, name)) = target {
            info!("char select: entering world as {name}");
            sounds.write(GlueSound("gsCharacterSelectionEnterWorld"));
            send_pick(&mut roster, &pick, guid);
        }
    }
}

/// Rotate the selected character: drag anywhere on the scene pane (the ref's full-frame mouse
/// rotation at the pinned 0.6°/px — dragging right increases the facing), or hold a rotate button
/// (±2°/frame; left decrements).
pub(super) fn rotate_model(
    panes: Query<(&Interaction, &SelectAction)>,
    motion: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut preview: ResMut<GluePreview>,
) {
    let window = window.single().ok();
    for (interaction, action) in &panes {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            SelectAction::Scene if motion.delta.x != 0.0 => {
                preview.yaw += drag_yaw(motion.delta.x, window);
            }
            SelectAction::RotateLeft => preview.yaw -= ROTATE_RATE * time.delta_secs(),
            SelectAction::RotateRight => preview.yaw += ROTATE_RATE * time.delta_secs(),
            _ => {}
        }
    }
}
