//! The select screen's input: row clicks (single selects, double enters, as the reference's
//! `CharacterSelectButton_OnClick`/`OnDoubleClick`), the bottom buttons, the keyboard, and model
//! rotation (drag at `CHARACTER_ROTATION_CONSTANT` 0.6°/px, or hold a rotate button at ±2°/frame).
//! A bare selection click is silent, as in the reference.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;

use crate::net::CharPick;
use crate::portrait::GluePreview;
use crate::sound::GlueSound;

use super::dialog::DeleteDialog;
use super::screen::SelectAction;
use super::{class_name, send_pick, ClientState, Roster};

use crate::glue::{drag_yaw, ROTATE_RATE};

/// The double-click window: the conventional interval, where the reference takes the OS's.
const DOUBLE_CLICK_SECS: f32 = 0.4;

/// Button presses and the keyboard: Enter enters the world, Escape and Back return to the login
/// screen, the arrows cycle the selection with wrap.
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
    // A modal owns the input while it is up: the delete confirm, the AddOns list, the realm list
    // over this screen, or the glue dialog (whose Okay must not double as Enter World or Escape).
    if dialog.open || panel.open || realms.shown || glue_dialog.is_open() {
        return;
    }
    let now = time.elapsed_secs();
    let mut enter_world = false;
    let mut back_to_login = false;
    // Buttons fire on release over the pressed button: the stock `<Button>` click mask is
    // `LeftButtonUp` alone (`crate::glue::glue_clicks`).
    for (entity, action) in &buttons {
        if !clicks.hit(entity) {
            continue;
        }
        match *action {
            SelectAction::Row(i) if i < roster.chars.len() => {
                let double =
                    last_click.is_some_and(|(row, at)| row == i && now - at < DOUBLE_CLICK_SECS);
                *last_click = Some((i, now));
                // `CharacterSelectButton_OnClick` selects only when the row changes, so the
                // current row keeps the facing dragged into it.
                roster.click_row(i);
                // `OnDoubleClick` runs the same select, then enters the world unconditionally.
                if double {
                    enter_world = true;
                }
            }
            SelectAction::EnterWorld => enter_world = true,
            SelectAction::Delete => {
                // With no selection the click only plays the sound (`selectedIndex > 0` gate).
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
                // The stock `OnClick` plays no sound (`CharacterSelect.xml:231-233`); this plays
                // the create-screen one.
                sounds.write(GlueSound("gsCharacterSelectionCreateNew"));
                // The whole roster feeds the "Configure Addons For:" dropdown. The realm must
                // resolve as `ui_macro::identity` does, or the enable files are keyed apart from
                // what world entry reads.
                let realm = roster
                    .realm
                    .as_ref()
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| "Realm".into());
                let chars = roster.chars.iter().map(|c| c.name.clone()).collect();
                panel.open_for(realm, chars);
            }
            SelectAction::Back => back_to_login = true,
            // `CHANGE_REALM`: the realm list rises over this screen with the session kept, so
            // Cancel returns here. The pending pick is cleared, or the other realm's roster would
            // be answered with a guid from this one.
            SelectAction::ChangeRealm => {
                sounds.write(GlueSound("gsLoginChangeRealmOK"));
                roster.pending_pick = None;
                crate::realm_select::open_over_char_select(&mut realms, &choice);
            }
            _ => {}
        }
    }

    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
        enter_world = true;
    }
    if keys.just_pressed(KeyCode::Escape) {
        back_to_login = true;
    }
    if back_to_login {
        // Back drops the parked session and both intents, so it does not auto-relogin.
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

/// Rotate the selected character: drag on the scene (0.6°/px, right increases the facing) or
/// hold a rotate button (±2°/frame, left decrements).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{RealmChoice, RealmRequest};
    use crate::realm_select::{list_keys, take_input, Realms};

    /// Escape on the realm list raised by Change Realm closes the list and nothing else: the
    /// press is the list's (`Realms::owns_input`), so this screen does not also go back to
    /// login, in either order the two run.
    #[test]
    fn escape_on_the_realm_list_does_not_also_leave_for_login() {
        for list_first in [true, false] {
            let (pick_tx, pick_rx) = crossbeam_channel::unbounded();
            let (realm_tx, realm_rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
                .insert_state(ClientState::CharSelect)
                .init_resource::<crate::glue::GlueClicks>()
                .init_resource::<ButtonInput<KeyCode>>()
                .init_resource::<Roster>()
                .init_resource::<Realms>()
                .init_resource::<DeleteDialog>()
                .init_resource::<super::super::addons::AddonsPanel>()
                .init_resource::<crate::login::LoginIntent>()
                .init_resource::<crate::glue::dialog::GlueDialog>()
                .insert_resource(CharPick(pick_tx))
                .insert_resource(RealmChoice(realm_tx))
                .add_message::<GlueSound>()
                .add_message::<bevy::input::mouse::MouseWheel>()
                .add_systems(PreUpdate, take_input);
            if list_first {
                app.add_systems(Update, (list_keys, select_input).chain());
            } else {
                app.add_systems(Update, (select_input, list_keys).chain());
            }
            app.world_mut()
                .resource_scope(|world, mut realms: Mut<Realms>| {
                    crate::realm_select::open_over_char_select(
                        &mut realms,
                        world.resource::<RealmChoice>(),
                    );
                });
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::Escape);
            app.update();
            app.world_mut().resource_mut::<ButtonInput<KeyCode>>().clear();
            app.update(); // `StateTransition` applies a pending state at the next frame

            let order = if list_first { "list first" } else { "screen first" };
            assert!(
                realm_rx
                    .try_iter()
                    .any(|r| matches!(r, RealmRequest::Abandon)),
                "{order}: the list's Cancel ran"
            );
            assert_eq!(
                *app.world().resource::<State<ClientState>>().get(),
                ClientState::CharSelect,
                "{order}: the press was the list's, so the screen under it stays"
            );
            assert!(
                pick_rx.try_recv().is_err(),
                "{order}: and the session is not dropped"
            );
        }
    }
}
