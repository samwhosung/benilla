//! The select screen's **layout** — the reference `CharacterSelect.xml` arrangement rebuilt in
//! Bevy UI, full-bleed and scaled to the window: the glue engine renders a
//! 1024×768 virtual screen scaled to the display, so every authored offset/size below is the
//! ref's number times `height / 768`.
//!
//! Bottom layer: the glue booth's fullscreen render (the `UI_<Race>` scene + the geared selected
//! character), drag-anywhere to rotate. Over it: the WoW logo (TOPLEFT), the selected character's
//! name (`GlueFontNormalHuge` at BOTTOM (0,100)), Enter World (200×60 at BOTTOM (0,30)) with the
//! rotate pair tucked under it, Back (BOTTOMRIGHT (−30,25)) and Delete Character to its left, and
//! the right-column character frame: 260×642 at TOPRIGHT (−5,−15), `Glue-Tooltip` backdrop tinted
//! `DEFAULT_TOOLTIP_COLOR` at 0.85 alpha, holding the realm banner, the Change Realm
//! button (back to [`crate::realm_select`]'s list), ten 256×70 row buttons from TOPLEFT
//! (24,−65) at the authored 57 px pitch (13 px overlap, hit-inset 15), and Create New Character at
//! the frame's BOTTOM (0,15). The delete dialog is [`super::dialog`]'s.

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

/// One clickable control on the screen — a single component so one query dispatches every button.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectAction {
    /// The fullscreen scene pane (drag to rotate — no click action).
    Scene,
    /// A character-list row (0-based roster index; single click selects, double click enters).
    Row(usize),
    EnterWorld,
    /// Back to the login screen (the ref's flow — decision 0539 retired 0465 §6's
    /// exit-the-client collapse).
    Back,
    Delete,
    CreateChar,
    /// Back to the realm list — drops the parked session, keeps the logon.
    ChangeRealm,
    /// Open the AddOns list — the reference's `CharacterSelectAddonsButton`.
    Addons,
    RotateLeft,
    RotateRight,
}

/// Root of the select screen (despawned whole on exit). `with_art` records whether the tree was
/// built with the client art present — an artless early spawn upgrades once the art lands; `s`
/// records the glue scale it was built at, so a window resize (mac fullscreen) rebuilds it too.
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
/// `DEFAULT_TOOLTIP_COLOR` (AccountLogin.lua): border rgb + bg rgb; the frame applies bg at 0.85.
const FRAME_BORDER: Color = Color::srgb(0.8, 0.8, 0.8);
const FRAME_FILL: Color = Color::srgb(0.09, 0.09, 0.09);
const FRAME_FILL_ALPHA: f32 = 0.85;
/// The list's row geometry (`CharSelectCharacterButtonTemplate` + the button anchors): 256×70
/// buttons whose next TOP anchors 13 above the previous BOTTOM — net pitch 57; the hit rect drops
/// the bottom 15 (the visible/clickable row is 55 tall).
pub(super) const MAX_ROWS: usize = 10;
const ROW_W: f32 = 256.0;
const ROW_HIT_H: f32 = 55.0;
const ROW_PITCH: f32 = 57.0;

/// Entry is a cheap state reset only — the tree spawns via [`materialize_screen`], because the
/// INITIAL state's `OnEnter` fires during app startup, before the MPQ chain / booth slots exist
/// (a one-shot spawn here came up artless with no scene pane — the boot-order trap the create
/// screen never sees, entered seconds later).
pub(super) fn enter_select(mut preview: ResMut<GluePreview>) {
    // The model faces the camera on entry (the C-side facing global's zero default; the create
    // screen's −15° is its own reset). The booth feed (`refresh::feed_glue_preview`) establishes
    // the scene + look from the roster each frame.
    preview.yaw = 0.0;
}

/// Spawn the screen tree once its prerequisites exist — and upgrade an artless early spawn the
/// moment the client art lands (despawn + respawn; the tree is cheap and static). With no client
/// data at all the artless tree still spawns after a short grace (the graceful-absence posture).
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
            // Rebuild when the art lands after an early artless spawn, or when a window resize
            // (mac fullscreen, a drag) has changed the glue scale the tree was baked at.
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
            // The 3D scene, full-bleed and first (everything else draws over it) — the ref's
            // screen IS the fullscreen ModelFFX: the selected race's scene with the geared
            // character standing in it. The whole pane drags to rotate (the ref's full-frame mouse
            // rotation); the page tint behind it is the no-art fallback. It keeps the WINDOW while
            // the chrome below does not: a pillarbox's bars are the booth camera's own output
            // clear inside this window-sized target (1619 §3), so the pane that samples it covers
            // the window and brings the bars with it.
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

    // ...and every piece of chrome hangs off the CANVAS — the boxed scene's own rect (decision
    // 2091). Anchored to the window instead, the logo and the whole right-hand character frame
    // (with Delete Character and Back) stood out in the bars at 21:9.
    let mut canvas = commands.spawn((crate::glue::glue_canvas(), ChildOf(root)));
    canvas.with_children(|ui| {
        // The WoW logo (`CharacterSelectLogo`, 256×128 at TOPLEFT (3,−7)).
        if let Some(logo) = &art.logo {
            ui.spawn((ImageNode::new(logo.clone()), abs(s, 3.0, 7.0, 256.0, 128.0)));
        }

        // The selected character's name (`CharSelectCharacterName`, GlueFontNormalHuge 22 gold,
        // BOTTOM (0,100)).
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

        // Enter World (`CharSelectEnterWorldButton`, GlueButtonTemplate 200×60 at BOTTOM (0,30)).
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

        // **AddOns** — the reference's `CharacterSelectAddonsButton`, which 1191
        // §5 recorded as never built because there was no list behind it. There is now. Bottom
        // LEFT, out of the way of the Enter World / Back cluster; the reference sits it in the
        // same lower band. Shown only when something is installed, exactly as the reference's
        // `UpdateAddonButton` hides it on `GetNumAddOns() == 0`.
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

        // Back (100×35 at BOTTOMRIGHT (−30,25)) and Delete Character (165×35 at its LEFT).
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

/// The right-column character frame (`CharacterSelectCharacterFrame`, 260×642 at TOPRIGHT
/// (−5,−15)): the `Glue-Tooltip` backdrop, the realm banner + disabled Change Realm, the ten row
/// buttons, and Create New Character at the bottom.
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
            // The backdrop: bg inset (10,5,4,9) tiled at 16, the 16-edge border over it — both
            // tinted with DEFAULT_TOOLTIP_COLOR (border rgb / bg rgb at 0.85), the ref's OnLoad.
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
            // The realm banner (`CharSelectRealmName`, GlueFontDisableLarge 18 at TOP (0,−10)).
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
            // Change Realm (below the banner) — live. It was drawn permanently disabled for as
            // long as there was no realm list behind it to go back to; there is now, and it
            // costs a world dial rather than a re-login (`crate::realm_select`).
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
            // The ten row buttons (256×70 from TOPLEFT (24,−65), net pitch 57 — the 13 px overlap
            // is the hit-rect's dropped bottom 15; our nodes ARE the hit shape, 55 tall).
            for row in 0..MAX_ROWS {
                row_button(frame, art, font, row, s);
            }
            // Create New Character (width ≈ text+50 ×45 at the frame BOTTOM (0,15) — the ref's
            // shipped client always shows it here; the per-free-row anchor is commented out).
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

/// One character-list row (`CharSelectCharacterButtonTemplate`): Name (GlueFontNormal 15 gold at
/// TOPLEFT (0,−5)), Info (GlueFontHighlightSmall 12 white below), Location (GlueFontDisableSmall
/// 12 gray below), and the ADD-mode `Glue-CharacterSelect-Highlight` card (256×74 at (−20,+8)) —
/// lit on hover and LOCKED on the selected row.
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
            LockHighlight::default(), // `LockHighlight` on the selected row (refresh.rs)
            Visibility::Hidden,       // shown by the refresh while the roster has this row
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

/// The rotate pair (`CharacterSelectRotateLeft/Right`, 50² each): the left button's TOP anchors to
/// Enter World's BOTTOM at (−15,+19) — the pair sits flush with the screen's bottom edge, tucked
/// under the button — the right overlapping −19; `UI-RotationRight-Big` art with the left mirrored,
/// `UI-Common-MouseHilight` ADD on hover.
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

/// **A world loading cover outranks the glue screen.**
///
/// The glue screens sit at [`SCREEN_Z`] = 1100 and the loading screen at 1000, so the glue draws
/// *over* the cover. That is right for the logout direction — the world→glue swap must show the
/// glue — and wrong for the entry one: the cover is raised in `WorldStage::Present` while we are
/// still `CharSelect`, so the raise frame renders the character screen on top of it and the FIRST
/// frame that can show the cover is the **state-flip** frame, which is also the frame the whole
/// world arrives on. Measured this round: click → cover on the glass was ~100 ms, of which the
/// flip frame alone was 60 — the character screen held, frozen, for every millisecond of it. That
/// is the director's *"the char freezes briefly before the loading screen appears"*; 1345 fixed
/// the same shape for the FrameXML burst and left this half standing, because nothing had ever
/// asked which frame the cover first *drew* on.
///
/// Hiding the root moves the cover onto the raise frame's own render, a whole frame earlier and —
/// far more importantly — **off** the flip frame's critical path: the cover is already drawn and
/// presenting while that frame churns. Registered after `WorldStage::Present` so it reads *this*
/// frame's raise; visibility propagates in `PostUpdate` and renders the same frame, exactly like
/// the loading screen's own show/hide. Read from [`LoadingScreen::covering`] and not from
/// [`crate::loading_screen::EntryCover`]: this is the one consumer that must react while still
/// `CharSelect`, which is precisely the frame `EntryCover` cannot count.
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
    // Clear the booth + scene (entering the world or the create screen; the latter re-establishes
    // its own the same frame) and drop any open dialog state.
    preview.look = None;
    preview.scene = None;
    dialog.close();
    // The shared glue dialog is this screen's too while it is up (a refused character login), and
    // it must not follow us into the world or onto the create screen — the login screen's own
    // `exit_login` closes it on the same edge for the same reason. The tree goes with it **here**
    // rather than being left to the driver: the driver runs on the glue screens only, so on the
    // edges out of the glue layer there would be nobody left to despawn the root.
    glue_dialog.dismiss(&mut commands);
}
