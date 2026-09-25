//! The delete confirm dialog (`CharacterDeleteDialog`) — the ref's typed-confirm:
//! the 512×256 `UI-DialogBox` centered over the screen, `CONFIRM_CHAR_DELETE` naming the selected
//! character, the alert icon, an edit box where typing `DELETE_CONFIRM_STRING` ("DELETE") enables
//! Okay, and Okay/Cancel (`GlueDialogButtonTemplate` 200×40). Esc cancels, Enter confirms when
//! armed; Okay sends `CMSG_CHAR_DELETE` through the parked glue channel and plays
//! `gsTitleOptionOK` (Cancel: `gsTitleOptionExit`). The target is snapshotted at open (guid +
//! title), so a roster refresh mid-dialog can never retarget the delete.
//!
//! The edit box is `CharacterDeleteEditBox` verbatim (CharacterSelect.xml): 130×32, the two
//! `UI-ChatInputBorder-Left/Right` 75×32 pieces overhanging it by 10 each side, typing in
//! `GlueFontHighlight` (FRIZQT 15 white — outlined like every glue font), no TextInsets, and the
//! standard drawn caret bar ([`caret_bar`]) blinking on the ref's 0.5 s clock, solid while typing.

use benilla_ui::widget::EditBoxState;
use bevy::input::keyboard::KeyboardInput;

use crate::textinput::{self, HostClipboard};
use bevy::prelude::*;

use crate::glue::art::{tc_rect, GlueArt, GOLD};
use crate::glue::backdrop::{backdrop_border, tiled_bg_node};
use crate::glue::widgets::{
    caret_bar, glue_button, outlined_text, overlay, paint_glue_field, GlueBtnKind, GlueDisabled,
    GlueFieldPart, GlueText,
};
use crate::glue_strings::GlueStrings;
use crate::net::{CharPick, CharRequest};
use crate::sound::GlueSound;

use super::wow_font;

/// The dialog's state: opened by the Delete button (the target snapshotted), driven by
/// [`drive_delete_dialog`], cleared on leave.
#[derive(Resource, Default)]
pub(super) struct DeleteDialog {
    pub(super) open: bool,
    /// The snapshotted delete target (guid) + its display line pieces (name, level, class name).
    target: Option<(u64, String, u8, &'static str)>,
    /// What's been typed into the confirm box — a real [`EditBoxState`], so it
    /// has the caret, selection, Ctrl+A and clipboard every other field has. The ref's
    /// `letters="32"` cap is the box's own `max_letters`. (`pub(super)` for the shot instrument.)
    pub(super) typed: EditBoxState,
    /// The spawned dialog root, while up.
    root: Option<Entity>,
    /// The glue scale the spawned tree was built at — a resize rebuilds it.
    spawned_s: f32,
}

impl DeleteDialog {
    /// Open for the character (snapshot — the roster may refresh underneath).
    pub(super) fn open_for(&mut self, guid: u64, name: String, level: u8, class: &'static str) {
        self.open = true;
        self.target = Some((guid, name, level, class));
        self.typed.set_text("");
    }

    pub(super) fn close(&mut self) {
        self.open = false;
        self.target = None;
        self.typed.set_text("");
    }

    /// The typed text matches `DELETE_CONFIRM_STRING` (case-insensitive, the ref's `strupper`).
    fn armed(&self, confirm: &str) -> bool {
        self.typed.text.eq_ignore_ascii_case(confirm)
    }
}

/// The dialog's action buttons.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum DialogAction {
    Okay,
    Cancel,
}

/// The typed-text line inside the edit box.
#[derive(Component)]
pub(super) struct TypedText;
/// The edit box's caret bar (the shared [`caret_bar`]; [`drive_delete_dialog`] blinks it — the
/// dialog's one field always holds the focus, like the create screen's name box).
#[derive(Component)]
pub(super) struct DeleteCaret;
/// The dialog root (despawned on close).
#[derive(Component)]
struct DialogUi;

/// Spawn/despawn the dialog with [`DeleteDialog::open`], feed its typing, and run its flows:
/// Okay (enabled only while the typed text matches) sends the delete; Cancel/Esc close; Enter
/// confirms when armed. Runs before the list refresh so a successful delete's roster update
/// repaints the same frame it lands.
#[allow(clippy::type_complexity)]
pub(super) fn drive_delete_dialog(
    mut commands: Commands,
    mut dialog: ResMut<DeleteDialog>,
    art: Res<GlueArt>,
    assets: Res<AssetServer>,
    strings: Option<Res<GlueStrings>>,
    pick: Res<CharPick>,
    keys: Res<ButtonInput<KeyCode>>,
    mut keyboard: MessageReader<KeyboardInput>,
    mut sounds: MessageWriter<GlueSound>,
    buttons: Query<(Entity, &DialogAction)>,
    clicks: Res<crate::glue::GlueClicks>,
    mut okay: Query<(&DialogAction, &mut GlueDisabled)>,
    // The five row items of the confirm box (segments + carets), painted by the shared
    // `paint_glue_field`; plus the host pasteboard and the window handle its Wayland backend needs.
    mut parts: Query<(
        &crate::glue::widgets::GlueFieldPart,
        Option<&mut Text>,
        &mut Visibility,
    )>,
    mut clipboard: NonSendMut<HostClipboard>,
    // One query: the window drives the glue scale, and its raw handle carries the `wl_display` the
    // Wayland clipboard backend is built from.
    window: Query<
        (&Window, Option<&bevy::window::RawHandleWrapper>),
        With<bevy::window::PrimaryWindow>,
    >,
    time: Res<Time>,
) {
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);
    let confirm = strings.text("DELETE_CONFIRM_STRING", "DELETE");

    // Closed: make sure nothing is spawned, drain stray keys, done.
    if !dialog.open {
        if let Some(root) = dialog.root.take() {
            commands.entity(root).despawn();
        }
        keyboard.clear();
        return;
    }

    // Spawn on the open edge — and respawn when a window resize has changed the glue scale the
    // tree was baked at (the typed text lives in the resource, so it survives the rebuild).
    let s = crate::glue::screen_scale(window.single().ok().map(|(w, _)| w));
    if dialog.root.is_some() && dialog.spawned_s != s {
        if let Some(root) = dialog.root.take() {
            commands.entity(root).despawn();
        }
    }
    if dialog.root.is_none() {
        dialog.root = Some(spawn_dialog(
            &mut commands,
            &art,
            &assets,
            strings,
            &dialog,
            s,
        ));
        dialog.spawned_s = s;
    }

    // Typing: the shared law — editing, caret, selection and the clipboard trio.
    // ENTER/ESCAPE come back unclaimed and are handled by the button/key block above.
    let mods = textinput::mods_now(&keys);
    let wl = textinput::wayland_display(window.iter().next().and_then(|(_, h)| h));
    for ev in keyboard.read() {
        textinput::feed_key(
            &mut dialog.typed,
            ev,
            mods,
            &mut clipboard,
            wl,
            textinput::CharFilter::Any,
        );
    }
    // The dialog's box is always the focused one while it is up.
    textinput::tick_caret(&mut dialog.typed, true, time.delta_secs());
    paint_glue_field(&dialog.typed, true, parts.iter_mut());

    let armed = dialog.armed(confirm);
    for (action, mut disabled) in &mut okay {
        if *action == DialogAction::Okay && disabled.0 == armed {
            disabled.0 = !armed;
        }
    }

    // The flows.
    let mut do_delete = false;
    let mut do_cancel = false;
    // Both buttons fire on the RELEASE (1533) — a confirm you can slide off and cancel.
    for (entity, action) in &buttons {
        if !clicks.hit(entity) {
            continue;
        }
        match action {
            DialogAction::Okay if armed => do_delete = true,
            DialogAction::Okay => {}
            DialogAction::Cancel => do_cancel = true,
        }
    }
    if (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter)) && armed {
        do_delete = true;
    }
    if keys.just_pressed(KeyCode::Escape) {
        do_cancel = true;
    }
    if do_delete {
        if let Some((guid, name, _, _)) = &dialog.target {
            info!("char select: deleting {name}");
            let _ = pick.0.send(CharRequest::Delete(*guid));
        }
        sounds.write(GlueSound("gsTitleOptionOK"));
        dialog.close();
    } else if do_cancel {
        sounds.write(GlueSound("gsTitleOptionExit"));
        dialog.close();
    }
}

/// Build the dialog tree (`CharacterDeleteDialog`'s shipped layout): the 512×256 DialogBox
/// backdrop centered, the two-tone `CONFIRM_CHAR_DELETE` line (gold lead, white identity — the
/// ref's inline `|cffffffff…|r` span, split across two stacked lines), the instructions, the
/// alert icon, the ChatInputBorder edit box, Okay + Cancel. The vertical chain is the ref's
/// anchor chain resolved: Text1 TOP (0,−16) → instructions 20 below → box 5 below (top 102 —
/// which also centers it on the LEFT (12,+10) alert icon, the authored cross-check).
fn spawn_dialog(
    commands: &mut Commands,
    art: &GlueArt,
    assets: &AssetServer,
    strings: &GlueStrings,
    dialog: &DeleteDialog,
    s: f32,
) -> Entity {
    let px = |v: f32| Val::Px(v * s);
    let font = wow_font(assets);
    let (name, level, class) = dialog
        .target
        .as_ref()
        .map(|(_, n, l, c)| (n.clone(), *l, *c))
        .unwrap_or_default();
    // `CONFIRM_CHAR_DELETE` = "Do you want to delete\n|cffffffff%s   Level %d   %s|r?" — the
    // colour-span split: everything before the |c is the gold lead; the span (+ the trailing "?")
    // is the white identity line.
    let raw = strings.text(
        "CONFIRM_CHAR_DELETE",
        "Do you want to delete\n|cffffffff%s   Level %d   %s|r?",
    );
    let filled = raw
        .replacen("%s", &name, 1)
        .replacen("%d", &level.to_string(), 1)
        .replacen("%s", class, 1);
    let (lead, identity) = match filled.split_once("|cffffffff") {
        Some((a, b)) => (a.trim_end_matches('\n').to_string(), b.replace("|r", "")),
        None => (filled.clone(), String::new()),
    };

    commands
        .spawn((
            DialogUi,
            GlobalZIndex(1200), // over the select screen's 1100
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
                height: px(256.0),
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
                // The alert icon (64² at LEFT (12,10) — center 118 from the top at height 256).
                if let Some(alert) = &art.dialog_alert {
                    b.spawn((
                        ImageNode::new(alert.clone()),
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(12.0),
                            top: px(86.0),
                            width: px(64.0),
                            height: px(64.0),
                            ..default()
                        },
                    ));
                }
                // The two-tone question (GlueFontNormalLarge 18): gold lead, white identity.
                for (top, text, color) in [
                    (16.0, lead.as_str(), GOLD),
                    (40.0, identity.as_str(), Color::WHITE),
                ] {
                    outlined_text(
                        b,
                        Node {
                            position_type: PositionType::Absolute,
                            top: px(top),
                            width: Val::Percent(100.0),
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                        (),
                        (),
                        GlueText {
                            text,
                            size: 18.0,
                            color,
                            wrap: false,
                        },
                        &font,
                        s,
                    );
                }
                // The instructions (`CONFIRM_CHAR_DELETE_INSTRUCTIONS`, small).
                outlined_text(
                    b,
                    Node {
                        position_type: PositionType::Absolute,
                        top: px(83.0),
                        width: Val::Percent(100.0),
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    (),
                    (),
                    GlueText {
                        text: strings.text(
                            "CONFIRM_CHAR_DELETE_INSTRUCTIONS",
                            "Type \"DELETE\" into the field to confirm.",
                        ),
                        size: 12.0,
                        color: GOLD,
                        wrap: false,
                    },
                    &font,
                    s,
                );
                // The edit box (`CharacterDeleteEditBox`, 130×32 centered): the two
                // `UI-ChatInputBorder-Left/Right` 75×32 pieces overhanging by 10 each side
                // (their authored sub-rects), the typed line in `GlueFontHighlight` (FRIZQT 15
                // white, outlined) at the box's left edge (no TextInsets), the caret bar after it.
                b.spawn((Node {
                    position_type: PositionType::Absolute,
                    top: px(102.0),
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },))
                    .with_children(|row| {
                        let framed =
                            art.chat_input_left.is_some() && art.chat_input_right.is_some();
                        let mut boxed = row.spawn(Node {
                            width: px(130.0),
                            height: px(32.0),
                            ..default()
                        });
                        if !framed {
                            boxed.insert(BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)));
                        }
                        boxed.with_children(|b| {
                            if let (Some((left, lsize)), Some((right, rsize))) =
                                (&art.chat_input_left, &art.chat_input_right)
                            {
                                b.spawn((
                                    ImageNode {
                                        image: left.clone(),
                                        rect: Some(tc_rect(*lsize, [0.0, 0.292_968_75, 0.0, 1.0])),
                                        ..default()
                                    },
                                    Node {
                                        position_type: PositionType::Absolute,
                                        left: px(-10.0),
                                        top: px(0.0),
                                        width: px(75.0),
                                        height: px(32.0),
                                        ..default()
                                    },
                                ));
                                b.spawn((
                                    ImageNode {
                                        image: right.clone(),
                                        rect: Some(tc_rect(*rsize, [0.707_031_25, 1.0, 0.0, 1.0])),
                                        ..default()
                                    },
                                    Node {
                                        position_type: PositionType::Absolute,
                                        right: px(-10.0),
                                        top: px(0.0),
                                        width: px(75.0),
                                        height: px(32.0),
                                        ..default()
                                    },
                                ));
                            }
                            b.spawn((Node {
                                position_type: PositionType::Absolute,
                                left: px(0.0),
                                right: px(0.0),
                                top: px(0.0),
                                bottom: px(0.0),
                                align_items: AlignItems::Center,
                                ..default()
                            },))
                                .with_children(|f| {
                                    // The same five-item row every other field uses
                                    // (`GlueFieldPart`): segments either side of the selection,
                                    // with a caret slot at each selection edge. Each segment is
                                    // its own `outlined_text`, and the outline copies follow
                                    // automatically (`glue::sync_outline_text`).
                                    let segment = |f: &mut ChildSpawnerCommands, part| {
                                        outlined_text(
                                            f,
                                            Node::default(),
                                            (),
                                            (TypedText, part),
                                            GlueText {
                                                text: "",
                                                size: 15.0, // GlueFontHighlight
                                                color: Color::WHITE,
                                                wrap: false,
                                            },
                                            &font,
                                            s,
                                        );
                                    };
                                    segment(f, GlueFieldPart::Before);
                                    caret_bar(
                                        f,
                                        (DeleteCaret, GlueFieldPart::CaretAtStart),
                                        15.0,
                                        s,
                                    );
                                    segment(f, GlueFieldPart::Selected);
                                    caret_bar(f, (DeleteCaret, GlueFieldPart::CaretAtEnd), 15.0, s);
                                    segment(f, GlueFieldPart::After);
                                });
                        });
                    });
                // Okay (right edge at center −6) + Cancel (left edge at center +7), 200×40,
                // bottom 16 — the GlueDialogButtonTemplate pair.
                b.spawn((Node {
                    position_type: PositionType::Absolute,
                    bottom: px(16.0),
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    column_gap: px(13.0),
                    flex_direction: FlexDirection::Row,
                    ..default()
                },))
                    .with_children(|row| {
                        glue_button(
                            row,
                            art,
                            &font,
                            DialogAction::Okay,
                            strings.text("OKAY", "Okay"),
                            200.0,
                            40.0,
                            GlueBtnKind::Dialog,
                            s,
                        );
                        glue_button(
                            row,
                            art,
                            &font,
                            DialogAction::Cancel,
                            strings.text("CANCEL", "Cancel"),
                            200.0,
                            40.0,
                            GlueBtnKind::Dialog,
                            s,
                        );
                    });
            });
        })
        .id()
}
