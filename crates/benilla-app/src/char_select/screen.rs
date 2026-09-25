//! The select screen's layout, `CharacterSelect.xml` rebuilt in Bevy UI. The glue screen is a
//! 1024×768 virtual screen, so every authored offset and size is scaled by `height / 768`.

use bevy::prelude::*;
use bevy::ui_render::ui_material::MaterialNode;
use bevy::window::PrimaryWindow;

use crate::glue::art::{GlueArt, BACKDROP, DIM, GOLD, NAME_EDGE};
use crate::glue::backdrop::{backdrop_border, tiled_bg_node};
use crate::glue::widgets::{
    abs, glue_button, outlined_text, overlay, GlueBtnKind, GlueText, Hilight, LockHighlight,
};
use crate::glue_strings::GlueStrings;
use crate::portrait::{GluePreview, PortraitImages, PortraitSource, GLUE_SLOT};
use benilla_assets::WorldAssets;

use super::wow_font;

/// One clickable control on the screen.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectAction {
    /// The fullscreen scene pane, dragged to rotate.
    Scene,
    /// A character-list row by 0-based roster index.
    Row(usize),
    EnterWorld,
    /// Back to the login screen.
    Back,
    Delete,
    CreateChar,
    /// Raise the realm list over this screen, keeping the session.
    ChangeRealm,
    /// Open the AddOns list (`CharacterSelectAddonsButton`).
    Addons,
    RotateLeft,
    RotateRight,
}

/// Root of the select screen. `with_art` and the glue scale `s` it was built at let an artless
/// spawn or a resize rebuild it.
#[derive(Component)]
pub(super) struct CharSelectUi {
    pub(super) with_art: bool,
    pub(super) s: f32,
}
/// The selected character's name over the model (`CharSelectCharacterName`).
#[derive(Component)]
pub(super) struct SelectedName;
/// The realm banner (`CharSelectRealmName`).
#[derive(Component)]
pub(super) struct RealmBanner;
/// A row's text line, refreshed from the roster.
#[derive(Component)]
pub(super) enum RowText {
    Name(usize),
    Info(usize),
    Location(usize),
}

const SCREEN_Z: i32 = 1100;
/// `DEFAULT_TOOLTIP_COLOR` (`AccountLogin.lua`): border and background rgb, background at 0.85.
const FRAME_BORDER: Color = Color::srgb(0.8, 0.8, 0.8);
const FRAME_FILL: Color = Color::srgb(0.09, 0.09, 0.09);
const FRAME_FILL_ALPHA: f32 = 0.85;
/// Row geometry (`CharSelectCharacterButtonTemplate`): 256×70 buttons, each TOP 13 above the
/// previous BOTTOM (pitch 57), the hit rect dropping the bottom 15 (55 tall).
pub(super) const MAX_ROWS: usize = 10;
const ROW_W: f32 = 256.0;
const ROW_HIT_H: f32 = 55.0;
const ROW_PITCH: f32 = 57.0;

/// Entry only resets state: the initial state's `OnEnter` fires at startup, before the MPQ chain
/// and booth slots exist, so the tree spawns in [`materialize_screen`].
pub(super) fn enter_select(mut preview: ResMut<GluePreview>) {
    // The model faces the camera on entry: the reference's facing global defaults to zero.
    preview.yaw = 0.0;
}

/// Spawn the screen tree once the art is loaded, respawn it when art lands after an artless spawn
/// or the glue scale changes; with no client data it spawns artless after a second.
pub(super) fn materialize_screen(
    mut commands: Commands,
    existing: Query<(Entity, &CharSelectUi)>,
    assets: Res<AssetServer>,
    portraits: Res<PortraitImages>,
    mut art: ResMut<GlueArt>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut add_mats: ResMut<Assets<crate::glue::add_material::AddUiMaterial>>,
    strings: Option<Res<GlueStrings>>,
    window: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
) {
    if let Some(mut wa) = world_assets {
        art.ensure_loaded(&mut wa, &mut images, &mut add_mats);
    }
    let with_art = art.button_up.is_some();
    let s = crate::glue::screen_scale(window.single().ok());
    match existing.single() {
        Ok((root, ui)) => {
            if (!ui.with_art && with_art) || ui.s != s {
                commands.entity(root).despawn();
                spawn_screen(
                    &mut commands,
                    &assets,
                    &portraits,
                    &art,
                    strings.as_deref(),
                    &window,
                );
            }
        }
        Err(_) => {
            if with_art || time.elapsed_secs() > 1.0 {
                spawn_screen(
                    &mut commands,
                    &assets,
                    &portraits,
                    &art,
                    strings.as_deref(),
                    &window,
                );
            }
        }
    }
}

fn spawn_screen(
    commands: &mut Commands,
    assets: &AssetServer,
    portraits: &PortraitImages,
    art: &GlueArt,
    strings: Option<&GlueStrings>,
    window: &Query<&Window, With<PrimaryWindow>>,
) {
    let font = wow_font(assets);
    let model_image = match portraits.0.get(GLUE_SLOT) {
        Some(PortraitSource::Live(h)) => Some(h.clone()),
        _ => None,
    };
    let s = crate::glue::screen_scale(window.single().ok());
    let px = |v: f32| Val::Px(v * s);
    let empty = GlueStrings::default();
    let strings = strings.unwrap_or(&empty);

    let root = commands
        .spawn((
            CharSelectUi {
                with_art: art.button_up.is_some(),
                s,
            },
            GlobalZIndex(SCREEN_Z),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(BACKDROP),
        ))
        .with_children(|ui| {
            // The fullscreen ModelFFX scene, first so everything draws over it. It covers the
            // window, not the canvas: a pillarbox's bars are the booth camera's own clear.
            let mut pane = ui.spawn((
                SelectAction::Scene,
                Button,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
            if let Some(image) = model_image {
                pane.insert(ImageNode::new(image));
            }
        })
        .id();

    // The chrome hangs off the canvas, the boxed scene's rect, so none of it sits in the bars.
    let mut canvas = commands.spawn((crate::glue::glue_canvas(), ChildOf(root)));
    canvas.with_children(|ui| {
        // `CharacterSelectLogo`, 256×128 at TOPLEFT (3,-7).
        if let Some(logo) = &art.logo {
            ui.spawn((ImageNode::new(logo.clone()), abs(s, 3.0, 7.0, 256.0, 128.0)));
        }

        // `CharSelectCharacterName`, GlueFontNormalHuge gold at BOTTOM (0,100).
        ui.spawn((Node {
            position_type: PositionType::Absolute,
            bottom: px(100.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },))
            .with_children(|c| {
                outlined_text(
                    c,
                    Node::default(),
                    (),
                    SelectedName,
                    GlueText {
                        text: "",
                        size: 22.0, // GlueFontNormalHuge
                        color: GOLD,
                        wrap: false,
                    },
                    &font,
                    s,
                );
            });

        // `CharSelectEnterWorldButton`, GlueButtonTemplate 200×60 at BOTTOM (0,30).
        ui.spawn((Node {
            position_type: PositionType::Absolute,
            bottom: px(30.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },))
            .with_children(|c| {
                glue_button(
                    c,
                    art,
                    &font,
                    SelectAction::EnterWorld,
                    strings.text("ENTER_WORLD", "Enter World"),
                    200.0,
                    60.0,
                    GlueBtnKind::Normal,
                    s,
                );
            });

        rotate_cluster(ui, art, &font, s);

        // `CharacterSelectAddonsButton`, hidden on `GetNumAddOns() == 0` (`UpdateAddonButton`).
        if super::addons::AddonsPanel::any_installed() {
            ui.spawn((Node {
                position_type: PositionType::Absolute,
                left: px(30.0),
                bottom: px(25.0),
                flex_direction: FlexDirection::Row,
                ..default()
            },))
                .with_children(|actions| {
                    glue_button(
                        actions,
                        art,
                        &font,
                        SelectAction::Addons,
                        strings.text("ADDONS", "AddOns"),
                        140.0,
                        35.0,
                        GlueBtnKind::Small,
                        s,
                    );
                });
        }

        // Back (100×35 at BOTTOMRIGHT (-30,25)) and Delete Character (165×35 at its LEFT).
        ui.spawn((Node {
            position_type: PositionType::Absolute,
            right: px(30.0),
            bottom: px(25.0),
            flex_direction: FlexDirection::Row,
            ..default()
        },))
            .with_children(|actions| {
                glue_button(
                    actions,
                    art,
                    &font,
                    SelectAction::Delete,
                    strings.text("DELETE_CHARACTER", "Delete Character"),
                    165.0,
                    35.0,
                    GlueBtnKind::Small,
                    s,
                );
                glue_button(
                    actions,
                    art,
                    &font,
                    SelectAction::Back,
                    strings.text("BACK", "Back"),
                    100.0,
                    35.0,
                    GlueBtnKind::Small,
                    s,
                );
            });

        character_frame(ui, art, &font, s, strings);
    });
}

/// The right-column `CharacterSelectCharacterFrame`, 260×642 at TOPRIGHT (-5,-15).
fn character_frame(
    ui: &mut ChildSpawnerCommands,
    art: &GlueArt,
    font: &Handle<Font>,
    s: f32,
    strings: &GlueStrings,
) {
    let px = |v: f32| Val::Px(v * s);
    ui.spawn((Node {
        position_type: PositionType::Absolute,
        right: px(5.0),
        top: px(15.0),
        width: px(260.0),
        height: px(642.0),
        ..default()
    },))
        .with_children(|frame| {
            // Background inset (10,5,4,9) tiled at 16 under the 16-edge border, both tinted with
            // `DEFAULT_TOOLTIP_COLOR` in the reference's OnLoad.
            if let (Some(bg), Some(border)) = (&art.tooltip_bg, &art.name_border) {
                frame.spawn((
                    tiled_bg_node(
                        bg.clone(),
                        NAME_EDGE,
                        s,
                        FRAME_FILL.with_alpha(FRAME_FILL_ALPHA),
                    ),
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(10.0),
                        right: px(5.0),
                        top: px(4.0),
                        bottom: px(9.0),
                        ..default()
                    },
                ));
                backdrop_border(frame, border, NAME_EDGE, FRAME_BORDER);
            } else {
                frame.spawn((
                    BackgroundColor(FRAME_FILL.with_alpha(FRAME_FILL_ALPHA)),
                    overlay(),
                ));
            }
            // `CharSelectRealmName`, GlueFontDisableLarge at TOP (0,-10).
            outlined_text(
                frame,
                Node {
                    position_type: PositionType::Absolute,
                    top: px(10.0),
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                (),
                RealmBanner,
                GlueText {
                    text: "",
                    size: 18.0, // GlueFontDisableLarge
                    color: DIM,
                    wrap: false,
                },
                font,
                s,
            );
            frame
                .spawn((Node {
                    position_type: PositionType::Absolute,
                    top: px(26.0),
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },))
                .with_children(|c| {
                    glue_button(
                        c,
                        art,
                        font,
                        SelectAction::ChangeRealm,
                        strings.text("CHANGE_REALM", "Change Realm"),
                        135.0,
                        33.0,
                        GlueBtnKind::Small,
                        s,
                    );
                });
            // From TOPLEFT (24,-65); the nodes are the 55-tall hit shape.
            for row in 0..MAX_ROWS {
                row_button(frame, art, font, row, s);
            }
            // Create New Character at the frame's BOTTOM (0,15); the reference's per-free-row
            // anchor is commented out.
            frame
                .spawn((Node {
                    position_type: PositionType::Absolute,
                    bottom: px(15.0),
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },))
                .with_children(|c| {
                    glue_button(
                        c,
                        art,
                        font,
                        SelectAction::CreateChar,
                        strings.text("CREATE_NEW_CHARACTER", "Create New Character"),
                        190.0,
                        45.0,
                        GlueBtnKind::Small,
                        s,
                    );
                });
        });
}

/// One `CharSelectCharacterButtonTemplate` row: Name, Info and Location lines and the ADD-mode
/// `Glue-CharacterSelect-Highlight` card (256×74 at (-20,+8)), lit on hover, locked when selected.
fn row_button(
    frame: &mut ChildSpawnerCommands,
    art: &GlueArt,
    font: &Handle<Font>,
    row: usize,
    s: f32,
) {
    let px = |v: f32| Val::Px(v * s);
    frame
        .spawn((
            SelectAction::Row(row),
            Button,
            LockHighlight::default(),
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                left: px(24.0),
                top: px(65.0 + row as f32 * ROW_PITCH),
                width: px(ROW_W),
                height: px(ROW_HIT_H),
                ..default()
            },
        ))
        .with_children(|b| {
            if let Some(hi) = &art.select_highlight {
                b.spawn((
                    Hilight,
                    Visibility::Hidden,
                    MaterialNode(hi.clone()),
                    abs(s, -20.0, -8.0, 256.0, 74.0),
                ));
            }
            outlined_text(
                b,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: px(5.0),
                    ..default()
                },
                (),
                RowText::Name(row),
                GlueText {
                    text: "",
                    size: 15.0, // GlueFontNormal
                    color: GOLD,
                    wrap: false,
                },
                font,
                s,
            );
            outlined_text(
                b,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: px(24.0),
                    ..default()
                },
                (),
                RowText::Info(row),
                GlueText {
                    text: "",
                    size: 12.0, // GlueFontHighlightSmall
                    color: Color::WHITE,
                    wrap: false,
                },
                font,
                s,
            );
            outlined_text(
                b,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: px(38.0),
                    ..default()
                },
                (),
                RowText::Location(row),
                GlueText {
                    text: "",
                    size: 12.0, // GlueFontDisableSmall
                    color: DIM,
                    wrap: false,
                },
                font,
                s,
            );
        });
}

/// The rotate pair (`CharacterSelectRotateLeft/Right`, 50² each): the left's TOP anchors to Enter
/// World's BOTTOM at (-15,+19), the right overlaps it by 19; the left mirrors the right's art.
fn rotate_cluster(ui: &mut ChildSpawnerCommands, art: &GlueArt, font: &Handle<Font>, s: f32) {
    let px = |v: f32| Val::Px(v * s);
    ui.spawn((Node {
        position_type: PositionType::Absolute,
        left: Val::Percent(50.0),
        bottom: px(-1.0),
        margin: UiRect::left(px(-40.0)),
        flex_direction: FlexDirection::Row,
        ..default()
    },))
        .with_children(|rot| {
            for (action, flip, overlap) in [
                (SelectAction::RotateLeft, true, 0.0),
                (SelectAction::RotateRight, false, -19.0),
            ] {
                let mut b = rot.spawn((
                    action,
                    Button,
                    Node {
                        width: px(50.0),
                        height: px(50.0),
                        margin: UiRect::left(px(overlap)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                ));
                match (&art.rotate_up, &art.rotate_down) {
                    (Some(up), down) => {
                        b.insert(ImageNode {
                            image: up.clone(),
                            flip_x: flip,
                            ..default()
                        });
                        if let Some(down) = down {
                            b.insert(crate::glue::widgets::ArtSwap {
                                up: up.clone(),
                                down: down.clone(),
                            });
                        }
                        if let Some(hi) = &art.mouse_hilight {
                            b.with_children(|inner| {
                                inner.spawn((
                                    Hilight,
                                    Visibility::Hidden,
                                    MaterialNode(hi.clone()),
                                    abs(s, 10.0, 10.0, 30.0, 30.0),
                                ));
                            });
                        }
                    }
                    (None, _) => {
                        b.insert((
                            crate::glue::widgets::FallbackFace,
                            BackgroundColor(crate::glue::art::BTN_BG),
                        ));
                        b.with_children(|inner| {
                            inner.spawn((
                                Text::new(if flip { "<" } else { ">" }),
                                TextFont {
                                    font: font.clone(),
                                    font_size: 16.0 * s,
                                    ..default()
                                },
                                TextColor(GOLD),
                            ));
                        });
                    }
                }
            }
        });
}

/// Hide the screen under a world loading cover. The glue at z 1100 draws over the loading screen
/// at z 1000, and on world entry the cover rises while still `CharSelect`; hiding the root shows it
/// on that frame, not the state-flip frame the world loads on. Runs after `WorldStage::Present` and
/// reads `LoadingScreen::covering`, since `EntryCover` does not count this frame.
pub(super) fn hide_under_world_cover(
    loading: Res<crate::loading_screen::LoadingScreen>,
    mut roots: Query<&mut Visibility, With<CharSelectUi>>,
) {
    let want = if loading.covering() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut vis in &mut roots {
        if *vis != want {
            *vis = want;
        }
    }
}

pub(super) fn exit_select(
    mut commands: Commands,
    roots: Query<Entity, With<CharSelectUi>>,
    mut preview: ResMut<GluePreview>,
    mut dialog: ResMut<super::dialog::DeleteDialog>,
    mut glue_dialog: ResMut<crate::glue::dialog::GlueDialog>,
) {
    for e in &roots {
        commands.entity(e).despawn();
    }
    preview.look = None;
    preview.scene = None;
    dialog.close();
    // The glue dialog must not follow into the world or the create screen. Its tree is despawned
    // here, since its driver runs on the glue screens only.
    glue_dialog.dismiss(&mut commands);
}
