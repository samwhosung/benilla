//! **The glue layer's one dialog** — the reference's shared `GlueDialog`.
//!
//! `GlueDialog.xml` is a `GlueParent` child, not a screen: `GlueDialog_Show` texts it, sizes it to
//! its content and `Show()`s it over whichever glue screen happens to be up, and every screen
//! raises the same frame through `OPEN_STATUS_DIALOG`. The realm list is built the same way and
//! for the same reason (2072).
//!
//! Ours lived inside [`crate::login`] until 2084, because the login screen was the only thing that
//! had ever needed one. That stopped being true when a refused *character* login had to be said on
//! the select screen (2071): the choice was a second copy of the widget over there, or this — the
//! move `crate::glue`'s own charter asks for, so "what the screens share" cannot fork.
//!
//! **The split, and where the line is.** This module owns the *widget*: the tree, the text, the
//! spawn/despawn, which key answers which button, the click sound. It does not own what a press
//! *means* — a realmlist Okay that refuses a bad address and stays open, a status Cancel that
//! abandons a login attempt in flight. Those belong to the screen that opened the dialog, so a
//! press is published as [`GlueDialogAnswer`] and answered by [`crate::login::answer_dialog`].
//! The one exception is [`DialogKind::Error`], whose entire behaviour is "say a thing, and its
//! Okay dismisses it" — [`drive_glue_dialog`] ends those itself, which is what lets a screen raise
//! one without also writing its ending.
//!
//! [`drive_glue_dialog`] runs inside **each screen's own chain** rather than being hoisted out
//! with a `before`/`after` pair. That is not a style choice: the shared painters
//! ([`crate::glue::art_swaps`] and friends) sit in those chains, and ordering this both after a
//! screen's input and before those painters from outside makes a painter transitively ordered
//! against its own `SystemTypeSet`, which Bevy rejects at schedule build. It panics at boot, and
//! no unit test can see it (2071).

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

/// `DEFAULT_TOOLTIP_COLOR` (AccountLogin.lua): the edit box's backdrop tint (border rgb, bg rgb) —
/// the login screen's own boxes, so the realmlist editor matches the two behind it.
const BOX_BORDER: Color = Color::srgb(0.8, 0.8, 0.8);
const BOX_FILL: Color = Color::srgb(0.09, 0.09, 0.09);

/// The dialog's two buttons — the ref's `GlueDialogButton1`/`GlueDialogButton2`. Its own
/// component rather than a variant of any screen's action enum: the buttons belong to the widget,
/// and the widget belongs to no screen.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlueDialogAction {
    /// The affirmative one — Cancel on the status dialog, Okay on the others.
    Button1,
    /// The second, on the kinds that declare one (Cancel on the realmlist editor).
    Button2,
}

/// A button was pressed on the dialog, for the screen that opened it to answer. Carries both
/// flags rather than one enum because the realmlist editor's ESCAPE answers *button 2*, and
/// nothing but the kind's own table knows that.
#[derive(Message, Clone, Copy)]
pub(crate) struct GlueDialogAnswer {
    pub(crate) kind: DialogKind,
    pub(crate) button1: bool,
    pub(crate) button2: bool,
}

/// Which dialog is up: the connecting status (Cancel button, text driven by the stages), an
/// error (Okay button), or the realmlist editor (Okay + Cancel over an edit box).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogKind {
    Status,
    Error,
    /// **Queued for a full realm**. The reference has no queue dialog *type*: it
    /// opens the ordinary `CANCEL` status dialog and re-texts it every frame, relabelling the one
    /// button to `CHANGE_REALM`. This is that, as a kind — the relabel is the only thing that
    /// distinguishes it from [`Self::Status`], and a kind is how this screen spells "different
    /// button caption".
    Queued,
    /// The realmlist editor — the reference's `GlueDialog` `hasEditBox` shape,
    /// which is how the shipped dialog asks for a typed value (`GlueDialog.lua`: it shows
    /// `GlueDialogEditBox` and re-heights the box to
    /// `16 + text + 8 + editbox + 8 + button + 16`). The reference never opens this particular
    /// dialog — it has no realmlist UI at all — but the widget is its own.
    Realmlist,
}

impl DialogKind {
    /// Whether this dialog carries the ref's `GlueDialogEditBox` (its `hasEditBox` flag).
    pub(crate) fn has_edit_box(self) -> bool {
        matches!(self, DialogKind::Realmlist)
    }

    /// `(button1, button2)` captions — `GlueDialogTypes`' own two fields. A `None` second button
    /// is the ref's centred single-button layout; `Some` is its BOTTOMRIGHT/LEFT pair.
    pub(crate) fn buttons(self, strings: &GlueStrings) -> (&str, Option<&str>) {
        match self {
            DialogKind::Status => (strings.text("CANCEL", "Cancel"), None),
            // The reference's own relabel: leaving a queue is "Change Realm", not "Cancel".
            DialogKind::Queued => (strings.text("CHANGE_REALM", "Change Realm"), None),
            DialogKind::Error => (strings.text("OKAY", "Okay"), None),
            DialogKind::Realmlist => (
                strings.text("OKAY", "Okay"),
                Some(strings.text("CANCEL", "Cancel")),
            ),
        }
    }
}

/// Which of the dialog's two buttons a key press answers, as `(button1, button2)` — the keyboard
/// half of [`drive_glue_dialog`]'s buttons; the mouse half is OR-ed in beside it. Button 1 is the
/// affirmative one on every kind (Cancel on the status dialog, Okay on the others); button 2
/// exists only where the kind declares it. ENTER confirms, ESCAPE dismisses; on a one-button
/// dialog they are the same button.
///
/// **`on_screen` is the whole of the second bug fixed on 2026-08-29.** Pressing ENTER on an empty
/// login form played the sound and showed nothing, while *clicking* Login showed the dialog
/// (director's report). The two systems poll the same [`ButtonInput`]: [`login_input`] opened the
/// error dialog on the ENTER edge, and [`drive_glue_dialog`] — later in the same chained frame, with
/// `keys` untouched — read *the same* `just_pressed(Enter)` as that dialog's own Okay and closed
/// it before it had ever been drawn. Both sounds played, which is exactly what was heard.
///
/// The reference cannot have this bug because it is event-driven: the ENTER that fires
/// `AccountLogin_Login` is *dispatched* to the focused edit box (`OnEnterPressed`), and a dialog
/// that does not exist yet is not in the dispatch. Polling is our seam, so the gate has to be
/// ours too, and it says the same thing the dispatch does — **a dialog answers only keys pressed
/// while it was already on screen**. Not "not this frame": on screen. It is passed the same
/// `fresh` flag that drives the (re)spawn, so the two cannot drift apart.
fn dialog_keys(kind: DialogKind, on_screen: bool, enter: bool, escape: bool) -> (bool, bool) {
    if !on_screen {
        return (false, false);
    }
    let button1 = match kind {
        // The status dialog's one button IS Cancel, so ESCAPE is it. The queue's button is the
        // same act under another name, so it answers to ESCAPE the same way.
        DialogKind::Status | DialogKind::Queued => escape,
        DialogKind::Error => escape || enter,
        DialogKind::Realmlist => enter,
    };
    (button1, kind == DialogKind::Realmlist && escape)
}

/// The glue layer's one dialog (the ref's shared `GlueDialog`): kind + text; the driver spawns/
/// despawns the tree (respawning on a kind change — the button caption differs) and updates the
/// text in place.
#[derive(Resource, Default)]
pub(crate) struct GlueDialog {
    pub(crate) kind: Option<DialogKind>,
    pub(crate) text: String,
    pub(crate) dirty: bool,
    pub(crate) root: Option<Entity>,
    /// The ref's `GlueDialogEditBox` — a real [`EditBoxState`] like the two on the screen behind
    /// it, so the realmlist box gets the same caret, selection, Ctrl+A and clipboard law.
    /// Only meaningful while a [`DialogKind::has_edit_box`] dialog is up; it is
    /// rebuilt from the current value on every open, so a cancelled edit leaves nothing behind.
    pub(crate) edit: EditBoxState,
    /// The queue's sample ring and the realm it is for — live only while [`DialogKind::Queued`]
    /// is up, and rebuilt from empty on every fresh queue so a second login never inherits the
    /// first one's estimate.
    pub(crate) queue: crate::login::queue::QueueEstimate,
    pub(crate) queue_realm: Option<String>,
    /// The kind the spawned tree was built for.
    spawned: Option<DialogKind>,
    /// The glue scale the spawned tree was built at — a resize rebuilds it.
    spawned_s: f32,
}

impl GlueDialog {
    /// Raise the one-button error dialog over whichever glue screen is up.
    ///
    /// `pub(crate)` for the character-select screen: a refused *character* login
    /// (`SMSG_CHARACTER_LOGIN_FAILED`) is said there, not here, and it is the same dialog — the
    /// reference has exactly one, under `GlueParent`.
    pub(crate) fn open_error(&mut self, text: &str) {
        self.kind = Some(DialogKind::Error);
        self.set_text(text);
    }

    /// Is a dialog up? The glue screens' modal test — while one is, the screen behind it does not
    /// answer clicks or keys (the select screen's own delete confirm and AddOns panel are read the
    /// same way).
    pub(crate) fn is_open(&self) -> bool {
        self.kind.is_some()
    }

    pub(crate) fn open_status(&mut self, text: &str) {
        self.kind = Some(DialogKind::Status);
        self.set_text(text);
    }
    /// Enter the queue: a fresh ring (a second login must not inherit the first one's estimate)
    /// and the realm's name for the `_NAME` text variants.
    pub(crate) fn open_queued(&mut self, realm: Option<String>) {
        self.kind = Some(DialogKind::Queued);
        self.queue = crate::login::queue::QueueEstimate::default();
        self.queue_realm = realm;
        self.set_text("");
    }
    /// Open the realmlist editor over `current`, with the caret at the end of it and the whole
    /// value selected — the reference's `hasEditBox` dialogs open ready to be typed over, and a
    /// player changing servers is replacing the address far more often than editing it.
    pub(crate) fn open_realmlist(&mut self, prompt: &str, current: &str) {
        self.kind = Some(DialogKind::Realmlist);
        self.edit = crate::textinput::field(crate::realmlist::MAX_LETTERS, false);
        self.edit.set_text(current);
        // `HighlightText(0, -1)` — the client's own select-all (`0x77cca0`), which resets the
        // blink on its way so the box opens on a solid caret.
        self.edit.highlight_text(0, -1);
        self.set_text(prompt);
    }
    pub(crate) fn set_text(&mut self, text: &str) {
        if self.text != text {
            self.text = text.to_string();
            self.dirty = true;
        }
    }
    /// Close **and take the tree down in the same frame** — what a button press does. Closing
    /// alone would leave the spawned root standing until [`drive_glue_dialog`] next runs, and on
    /// the edges out of the glue layer that is never.
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

/// The dialog's message text (updated in place on stage changes).
#[derive(Component)]
pub(crate) struct DialogText;
/// The dialog's edit box row items — the ref's `GlueDialogEditBox`, painted from
/// [`GlueDialog::edit`] by [`refresh_dialog_box`].
#[derive(Component, Clone)]
pub(crate) struct DialogEditText;

/// Build the dialog tree (the ref's shared `GlueDialog`): the 512-wide `UI-DialogBox` backdrop
/// centered on the screen, the message (`GlueFontNormalLarge`, wrapping at 440), an optional edit
/// box, and one or two `GlueDialogButtonTemplate` 200×40 buttons — Cancel for the connecting
/// status, Okay for an error, Okay + Cancel over the box for the realmlist editor.
/// Content-sized vertically (the ref's own `GlueDialog_OnShow` resize, by layout).
///
/// The edit box and the second button are both the reference's own (`GlueDialog.lua`'s
/// `hasEditBox` and `button2`); benilla simply had no dialog that used either until 1667. The two
/// authored geometries they bring with them: the button pair is Button1 BOTTOMRIGHT ← the
/// backdrop's BOTTOM at (−6, 16) with Button2 LEFT ← Button1's RIGHT at (13, 0) — a centred pair
/// with a 13 gap — and the box re-heights the backdrop to
/// `16 + text + 8 + editbox + 8 + button + 16`, which the column below is already shaped as.
///
/// **One stated divergence:** the ref's `GlueDialogEditBox` is 130×32, sized for the short values
/// its own dialogs ask for. A realmlist is a hostname, so ours is 300 wide inside the same 512
/// backdrop; the height, insets and border treatment are the login screen's boxes unchanged.
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
                // The message (GlueFontNormalLarge 18 at TOP (0,−16), width 440, wrapping) —
                // **centred**, which `GlueDialogText` gets by omitting `justifyH` (a FontString's
                // default is CENTER; every other wrapped glue string in the shipped XML asks for
                // LEFT explicitly). It read left-aligned until the director's eye caught it.
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
                // The edit box, on the kinds that declare one (`hasEditBox`).
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
                // The buttons (GlueDialogButtonTemplate 200×40) — one centred, or the authored
                // pair with its 13 gap.
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

/// Spawn/despawn the dialog tree with the resource, keep its text current, and read its buttons
/// from the mouse and the keyboard.
///
/// A press plays the click sound, publishes a [`GlueDialogAnswer`] for the screen that opened the
/// dialog, and — for [`DialogKind::Error`] alone — dismisses it here, because that kind's whole
/// behaviour *is* its dismissal. Everything else a press might mean belongs to the opener; see
/// this module's header for where that line falls and why.
///
/// Runs inside each glue screen's own system chain (again: the header says why it cannot be
/// hoisted out with an ordering pair).
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

    // (Re)spawn on the open edge, a kind change (the button caption differs), or a window resize
    // (the tree bakes the glue scale); a text-only change updates the message line in place.
    //
    // `fresh` is *this dialog appearing*, as opposed to a resize rebuilding a dialog already up —
    // and it is the same flag [`dialog_keys`] takes as `!on_screen`, so the frame the dialog is
    // drawn on and the frame it starts answering keys on are one decision, not two.
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

    // The buttons, from the mouse or the keys. A click needs no `fresh` guard of its own: the
    // button it would hit did not exist to be clicked on the frame the tree spawned.
    let hit = |want: GlueDialogAction| buttons.iter().any(|(e, a)| *a == want && clicks.hit(e));
    let (key1, key2) = dialog_keys(
        kind,
        !fresh,
        keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter),
        keys.just_pressed(KeyCode::Escape),
    );
    let button1 = hit(GlueDialogAction::Button1) || key1;
    let button2 = hit(GlueDialogAction::Button2) || key2;

    // **Every glue dialog button clicks** — `GlueDialog_OnClick` ends with
    // `PlaySound("gsTitleOptionOK")` for button 1 and button 2 alike, whatever the dialog's type.
    // Ours played nothing at all. Emitted here, once, before the kinds diverge, so no arm can
    // forget it (and so the realmlist editor's Okay-that-refuses still clicks: the reference plays
    // the sound on the press, not on the outcome).
    if button1 || button2 {
        sounds.write(GlueSound("gsTitleOptionOK"));
        // **What a press MEANS belongs to whoever opened the dialog**, not to the widget. A
        // realmlist Okay may refuse the typed value and stay open; a status Cancel has a login
        // attempt to abandon. Both are the login screen's business, and neither is reachable from
        // any other screen — so the press is published and [`crate::login::answer_dialog`]
        // answers it.
        answers.write(GlueDialogAnswer {
            kind,
            button1,
            button2,
        });
        // The one kind with no owner: an `Error` states a thing and its Okay dismisses it, which
        // is the whole of its behaviour. Ending it here is what lets ANY glue screen raise one
        // without also having to write its ending — the character-select screen's refused login
        // (2071) raises an error and answers nothing.
        if kind == DialogKind::Error {
            dialog.dismiss(&mut commands);
        }
    }
}

/// Paint the dialog's edit box from [`GlueDialog::edit`], through the same
/// [`paint_glue_field`] the two screen boxes use — so the realmlist box gets the
/// identical caret, selection highlight and scrolling.
///
/// Runs **after** [`drive_glue_dialog`], which is what spawns the box: on the frame a dialog
/// opens the entities do not exist until that system has run, and painting before it would show
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

    /// The dialog kinds' own shape: only the realmlist editor carries the ref's `hasEditBox`, and
    /// only it declares a second button. The caret clock and the keyboard routing both branch on
    /// the first of those, and `spawn_dialog` lays out from the second.
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

    /// **A dialog never answers the key press that opened it.** ENTER on an empty login form
    /// opened the error dialog in `login_input` and then, later in the *same* frame with the same
    /// `ButtonInput`, the driver read that identical `just_pressed(Enter)` as the dialog's own
    /// Okay — so the popup appeared and vanished inside one frame and only the two sounds were
    /// heard (director's report, 2026-08-29). The gate is being on screen, not a frame count.
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

    /// Which key answers which button, per kind: ENTER and ESCAPE are the same (single) button on
    /// an error dialog; ESCAPE alone works the Cancel-shaped ones; the two-button editor splits
    /// them, ENTER to Okay and ESCAPE to Cancel.
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
