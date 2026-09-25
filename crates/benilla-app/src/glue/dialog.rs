//! The glue layer's one dialog, the reference's shared `GlueDialog`: a `GlueParent` child that
//! every glue screen raises over itself.
//!
//! This module owns the widget: tree, text, which key answers which button, the click sound. What
//! a press means belongs to the screen that opened it, so a press is published as
//! [`GlueDialogAnswer`]; only [`DialogKind::Error`] is dismissed here, by its Okay.
//!
//! [`drive_glue_dialog`] runs inside each screen's own chain, not hoisted out with a
//! `before`/`after` pair: ordering it from outside against the shared painters in those chains
//! makes a painter ordered against its own `SystemTypeSet`, which panics at schedule build.
use bevy::prelude::*;

use benilla_ui::widget::EditBoxState;

use crate::glue::art::{GlueArt, GOLD};
use crate::glue::backdrop::{backdrop_border, tiled_bg_node};
use crate::glue::widgets::{
    glue_button, glue_edit_box, outlined_text_centered, overlay, paint_glue_field, GlueBtnKind,
    GlueFieldPart, GlueText,
};
use crate::glue_strings::GlueStrings;
use crate::sound::GlueSound;

use crate::char_select::wow_font;

/// `DEFAULT_TOOLTIP_COLOR` (AccountLogin.lua), the login boxes' border and fill tint.
const BOX_BORDER: Color = Color::srgb(0.8, 0.8, 0.8);
const BOX_FILL: Color = Color::srgb(0.09, 0.09, 0.09);

/// The dialog's two buttons, `GlueDialogButton1` and `GlueDialogButton2`.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlueDialogAction {
    /// The affirmative one: Cancel on the status dialog, Okay on the others.
    Button1,
    Button2,
}

/// A button was pressed on the dialog, for the screen that opened it to answer.
#[derive(Message, Clone, Copy)]
pub(crate) struct GlueDialogAnswer {
    pub(crate) kind: DialogKind,
    pub(crate) button1: bool,
    pub(crate) button2: bool,
}

/// Which dialog is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogKind {
    Status,
    Error,
    /// Queued for a full realm: the reference re-texts the `CANCEL` status dialog every frame
    /// and relabels its button `CHANGE_REALM`.
    Queued,
    /// The realmlist editor, on the reference's `hasEditBox` dialog shape (`GlueDialog.lua`). The
    /// 1.12 client has no realmlist dialog.
    Realmlist,
}

impl DialogKind {
    /// Whether this dialog shows `GlueDialogEditBox` (the `hasEditBox` flag).
    pub(crate) fn has_edit_box(self) -> bool {
        matches!(self, DialogKind::Realmlist)
    }

    /// `GlueDialogTypes`' `(button1, button2)` captions; no second button centres the first.
    pub(crate) fn buttons(self, strings: &GlueStrings) -> (&str, Option<&str>) {
        match self {
            DialogKind::Status => (strings.text("CANCEL", "Cancel"), None),
            DialogKind::Queued => (strings.text("CHANGE_REALM", "Change Realm"), None),
            DialogKind::Error => (strings.text("OKAY", "Okay"), None),
            DialogKind::Realmlist => (
                strings.text("OKAY", "Okay"),
                Some(strings.text("CANCEL", "Cancel")),
            ),
        }
    }
}

/// Which button a key press answers, as `(button1, button2)`: ENTER confirms, ESCAPE dismisses.
///
/// A dialog answers only keys pressed while it was already on screen: the reference dispatches
/// the ENTER that opens a dialog to the focused edit box, but polling would read that same press
/// as the new dialog's Okay. `on_screen` is the `!fresh` that drives the spawn.
fn dialog_keys(kind: DialogKind, on_screen: bool, enter: bool, escape: bool) -> (bool, bool) {
    if !on_screen {
        return (false, false);
    }
    let button1 = match kind {
        // The status and queue dialogs' one button is Cancel, so ESCAPE is it.
        DialogKind::Status | DialogKind::Queued => escape,
        DialogKind::Error => escape || enter,
        DialogKind::Realmlist => enter,
    };
    (button1, kind == DialogKind::Realmlist && escape)
}

/// The glue layer's one dialog, the reference's shared `GlueDialog`.
#[derive(Resource, Default)]
pub(crate) struct GlueDialog {
    pub(crate) kind: Option<DialogKind>,
    pub(crate) text: String,
    pub(crate) dirty: bool,
    pub(crate) root: Option<Entity>,
    /// `GlueDialogEditBox`, rebuilt on every open so a cancelled edit leaves nothing behind.
    pub(crate) edit: EditBoxState,
    /// The queue's sample ring and realm, live while [`DialogKind::Queued`] is up.
    pub(crate) queue: crate::login::queue::QueueEstimate,
    pub(crate) queue_realm: Option<String>,
    spawned: Option<DialogKind>,
    /// The glue scale the spawned tree was built at; a resize rebuilds it.
    spawned_s: f32,
}

impl GlueDialog {
    /// Raise the one-button error dialog over whichever glue screen is up.
    pub(crate) fn open_error(&mut self, text: &str) {
        self.kind = Some(DialogKind::Error);
        self.set_text(text);
    }

    /// The glue screens' modal test: while a dialog is up, the screen behind it takes no input.
    pub(crate) fn is_open(&self) -> bool {
        self.kind.is_some()
    }

    pub(crate) fn open_status(&mut self, text: &str) {
        self.kind = Some(DialogKind::Status);
        self.set_text(text);
    }
    /// Enter the queue with a fresh ring, so a second login never inherits the first's estimate;
    /// `realm` feeds the `_NAME` text variants.
    pub(crate) fn open_queued(&mut self, realm: Option<String>) {
        self.kind = Some(DialogKind::Queued);
        self.queue = crate::login::queue::QueueEstimate::default();
        self.queue_realm = realm;
        self.set_text("");
    }
    /// Open the realmlist editor over `current`, the whole value selected to be typed over.
    pub(crate) fn open_realmlist(&mut self, prompt: &str, current: &str) {
        self.kind = Some(DialogKind::Realmlist);
        self.edit = crate::textinput::field(crate::realmlist::MAX_LETTERS, false);
        self.edit.set_text(current);
        // `HighlightText(0, -1)`, the reference's select-all (`0x77cca0`), also resets the blink
        // so the box opens on a solid caret.
        self.edit.highlight_text(0, -1);
        self.set_text(prompt);
    }
    pub(crate) fn set_text(&mut self, text: &str) {
        if self.text != text {
            self.text = text.to_string();
            self.dirty = true;
        }
    }
    /// Close and despawn the tree this frame: on the edges out of the glue layer,
    /// [`drive_glue_dialog`] never runs again to take it down.
    pub(crate) fn dismiss(&mut self, commands: &mut Commands) {
        self.close();
        if let Some(root) = self.root.take() {
            commands.entity(root).despawn();
        }
    }

    pub(crate) fn close(&mut self) {
        self.kind = None;
        self.text.clear();
        self.dirty = false;
    }
}

/// The dialog's message text, updated in place.
#[derive(Component)]
pub(crate) struct DialogText;
/// The dialog's `GlueDialogEditBox` parts, painted by [`refresh_dialog_box`].
#[derive(Component, Clone)]
pub(crate) struct DialogEditText;

/// Build the `GlueDialog` tree: a 512-wide `UI-DialogBox` backdrop, the message wrapping at 440,
/// an optional edit box and one or two 200×40 buttons, sized to its content as
/// `GlueDialog_OnShow` does: `16 + text + 8 + editbox + 8 + button + 16`. A button pair is
/// centred with a 13 gap (Button1 BOTTOMRIGHT at the backdrop's BOTTOM (−6, 16)).
///
/// Deviation: the edit box is 300 wide, not `GlueDialogEditBox`'s 130, because a realmlist is a
/// hostname.
pub(crate) fn spawn_dialog(
    commands: &mut Commands,
    art: &GlueArt,
    assets: &AssetServer,
    strings: &GlueStrings,
    kind: DialogKind,
    text: &str,
    s: f32,
) -> Entity {
    let px = |v: f32| Val::Px(v * s);
    let font = wow_font(assets);
    let edit_font: Handle<Font> = assets.load("mpq://Fonts/ARIALN.ttf");
    let (caption, caption2) = kind.buttons(strings);
    commands
        .spawn((
            GlobalZIndex(1200), // over the screen's 1100
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .with_children(|overlay_ui| {
            let mut boxed = overlay_ui.spawn(Node {
                width: px(512.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(0.0), px(16.0)),
                row_gap: px(13.0),
                ..default()
            });
            boxed.with_children(|b| {
                // Backdrop: bg tiled at 32 inside (11,12,12,11), the 32-edge border over it.
                if let (Some(bg), Some(border)) = (&art.dialog_bg, &art.dialog_border) {
                    b.spawn((
                        tiled_bg_node(bg.clone(), 32.0, s, Color::WHITE),
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(11.0),
                            right: px(12.0),
                            top: px(12.0),
                            bottom: px(11.0),
                            ..default()
                        },
                    ));
                    backdrop_border(b, border, 32.0, Color::WHITE);
                } else {
                    b.spawn((
                        BackgroundColor(Color::srgba(0.05, 0.05, 0.08, 0.95)),
                        overlay(),
                    ));
                }
                // GlueFontNormalLarge 18, centred: `GlueDialogText` omits `justifyH`, and a
                // FontString defaults to CENTER.
                outlined_text_centered(
                    b,
                    Node {
                        width: px(440.0),
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    (),
                    DialogText,
                    GlueText {
                        text,
                        size: 18.0,
                        color: GOLD,
                        wrap: true,
                    },
                    &font,
                    s,
                );
                if kind.has_edit_box() {
                    glue_edit_box(
                        b,
                        art,
                        &edit_font,
                        (),
                        DialogEditText,
                        (300.0, 32.0),
                        (BOX_BORDER, BOX_FILL),
                        (15.0, 0.0, 0.0, 5.0), // the login boxes' TextInsets
                        s,
                    );
                }
                // GlueDialogButtonTemplate 200×40: one centred, or the pair with its 13 gap.
                b.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: px(13.0),
                    ..default()
                })
                .with_children(|row| {
                    glue_button(
                        row,
                        art,
                        &font,
                        GlueDialogAction::Button1,
                        caption,
                        200.0,
                        40.0,
                        GlueBtnKind::Dialog,
                        s,
                    );
                    if let Some(caption2) = caption2 {
                        glue_button(
                            row,
                            art,
                            &font,
                            GlueDialogAction::Button2,
                            caption2,
                            200.0,
                            40.0,
                            GlueBtnKind::Dialog,
                            s,
                        );
                    }
                });
            });
        })
        .id()
}

/// Keep the dialog tree in step with [`GlueDialog`] and read its buttons from mouse and keys.
pub(crate) fn drive_glue_dialog(
    mut commands: Commands,
    mut dialog: ResMut<GlueDialog>,
    art: Res<crate::glue::art::GlueArt>,
    assets: Res<AssetServer>,
    strings: Option<Res<GlueStrings>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Query<(Entity, &GlueDialogAction)>,
    clicks: Res<crate::glue::GlueClicks>,
    mut texts: Query<&mut Text, With<DialogText>>,
    mut sounds: MessageWriter<GlueSound>,
    mut answers: MessageWriter<GlueDialogAnswer>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);

    let Some(kind) = dialog.kind else {
        if let Some(root) = dialog.root.take() {
            commands.entity(root).despawn();
        }
        dialog.spawned = None;
        return;
    };

    // Respawn on open, a kind change or a resize; a text change updates in place. `fresh` (the
    // dialog appearing, not a resize) is also `dialog_keys`' `!on_screen`.
    let s = crate::glue::screen_scale(window.single().ok());
    let fresh = dialog.root.is_none() || dialog.spawned != Some(kind);
    if fresh || dialog.spawned_s != s {
        if let Some(root) = dialog.root.take() {
            commands.entity(root).despawn();
        }
        dialog.root = Some(spawn_dialog(
            &mut commands,
            &art,
            &assets,
            strings,
            kind,
            &dialog.text,
            s,
        ));
        dialog.edit.reset_blink();
        dialog.spawned = Some(kind);
        dialog.spawned_s = s;
        dialog.dirty = false;
    } else if dialog.dirty {
        for mut t in &mut texts {
            if t.0 != dialog.text {
                t.0 = dialog.text.clone();
            }
        }
        dialog.dirty = false;
    }

    // A click needs no `fresh` guard: its button did not exist on the frame the tree spawned.
    let hit = |want: GlueDialogAction| buttons.iter().any(|(e, a)| *a == want && clicks.hit(e));
    let (key1, key2) = dialog_keys(
        kind,
        !fresh,
        keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter),
        keys.just_pressed(KeyCode::Escape),
    );
    let button1 = hit(GlueDialogAction::Button1) || key1;
    let button2 = hit(GlueDialogAction::Button2) || key2;

    // `GlueDialog_OnClick` plays `gsTitleOptionOK` for either button on every dialog type, on
    // the press whatever its outcome.
    if button1 || button2 {
        sounds.write(GlueSound("gsTitleOptionOK"));
        answers.write(GlueDialogAnswer {
            kind,
            button1,
            button2,
        });
        // An error's Okay only dismisses it, so any glue screen can raise one and answer nothing.
        if kind == DialogKind::Error {
            dialog.dismiss(&mut commands);
        }
    }
}

/// Paint the dialog's edit box from [`GlueDialog::edit`] through [`paint_glue_field`].
///
/// Runs after [`drive_glue_dialog`], which spawns the box; before it, an opening dialog would show
/// one frame of empty box.
#[allow(clippy::type_complexity)]
pub(crate) fn refresh_dialog_box(
    dialog: Res<GlueDialog>,
    mut boxes: Query<(&GlueFieldPart, Option<&mut Text>, &mut Visibility), With<DialogEditText>>,
) {
    if dialog.kind.is_some_and(DialogKind::has_edit_box) {
        paint_glue_field(&dialog.edit, true, boxes.iter_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_realmlist_dialog_has_a_box_and_two_buttons() {
        let strings = GlueStrings::default();
        assert!(!DialogKind::Status.has_edit_box());
        assert!(!DialogKind::Error.has_edit_box());
        assert!(DialogKind::Realmlist.has_edit_box());
        assert_eq!(DialogKind::Status.buttons(&strings), ("Cancel", None));
        assert_eq!(DialogKind::Error.buttons(&strings), ("Okay", None));
        assert_eq!(
            DialogKind::Realmlist.buttons(&strings),
            ("Okay", Some("Cancel")),
        );
    }

    #[test]
    fn a_dialog_does_not_answer_the_key_that_opened_it() {
        // The frame it appears on: the key that opened it is not its answer.
        assert_eq!(
            dialog_keys(DialogKind::Error, false, true, false),
            (false, false),
        );
        assert_eq!(
            dialog_keys(DialogKind::Error, false, false, true),
            (false, false),
        );
        // Once it is up, the same press is.
        assert_eq!(
            dialog_keys(DialogKind::Error, true, true, false),
            (true, false),
        );
    }

    #[test]
    fn each_dialog_kind_maps_its_own_keys() {
        for (kind, enter, escape, want) in [
            (DialogKind::Error, true, false, (true, false)),
            (DialogKind::Error, false, true, (true, false)),
            (DialogKind::Status, true, false, (false, false)),
            (DialogKind::Status, false, true, (true, false)),
            (DialogKind::Queued, false, true, (true, false)),
            (DialogKind::Queued, true, false, (false, false)),
            (DialogKind::Realmlist, true, false, (true, false)),
            (DialogKind::Realmlist, false, true, (false, true)),
        ] {
            assert_eq!(
                dialog_keys(kind, true, enter, escape),
                want,
                "{kind:?} enter={enter} escape={escape}",
            );
        }
    }
}
