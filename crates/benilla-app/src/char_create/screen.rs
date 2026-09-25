//! The create screen's **layout** — the reference `CharacterCreate.xml` arrangement rebuilt in
//! Bevy UI (decision 0423's polish passes), full-bleed and scaled to the window: the glue engine
//! renders a 1024×768 virtual screen scaled to the display, so every authored offset/size below is
//! the ref's number times `height / 768`. The widget shapes it places live in [`super::widgets`],
//! the component vocabulary in [`super::parts`], the art in [`super::art`], and the systems that
//! drive it all in [`super::refresh`].
//!
//! Left: the configuration tower — `UI-CharacterCreate-Background` under three stacked
//! `OuterBorder` pieces, the faction banners behind the 2×4 race grid, the gender pair, the
//! valid-classes-only grid, the five `LabelFrame` dial spinners, Randomize. Right: the three
//! `TextPanel-Border` info panels (faction/race/class), bg-tinted per faction like the ref's
//! `SetBackdropColor`, quoting the GlueStrings paragraphs. Center: the transparent-booth model,
//! full-height, drag- or button-rotatable (the big `UI-RotationRight` pair, bottom-left). Bottom:
//! NAME over the `Glue-Tooltip-Border` edit box; Accept over Back in the corner.

use bevy::prelude::*;
use bevy::ui_render::ui_material::MaterialNode;
use bevy::window::PrimaryWindow;

use crate::char_select::{race_name, wow_font};
use crate::entities::CharCreate;
use crate::glue_strings::GlueStrings;
use crate::portrait::{GlueLook, GluePreview, PortraitImages, PortraitSource, GLUE_SLOT};
use benilla_assets::WorldAssets;

use super::parts::{CharCreateUi, DialRow, DynIcon, DynText, DynTint, StatusLine};
use super::{CreateAction, CreateSelection, ALLIANCE, HORDE, INITIAL_FACING};
use crate::glue::art::{
    tc_rect, GlueArt, ALLIANCE_BORDER, ALLIANCE_FILL, BACKDROP, BTN_BG, DIM, GOLD, INFO_TEXT,
};
use crate::glue::widgets::{
    abs, dial_arrow, glue_button, icon_button, outlined_text, ArtSwap, FallbackFace, GlueBtnKind,
    GlueText, Hilight,
};

const SCREEN_Z: i32 = 1100;

// ── Spawn ────────────────────────────────────────────────────────────────────────────────────────

pub(super) fn enter_create(
    mut commands: Commands,
    assets: Res<AssetServer>,
    catalog: Option<Res<CharCreate>>,
    portraits: Res<PortraitImages>,
    mut sel: ResMut<CreateSelection>,
    mut preview: ResMut<GluePreview>,
    mut art: ResMut<GlueArt>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut add_mats: ResMut<Assets<crate::glue::add_material::AddUiMaterial>>,
    strings: Option<Res<GlueStrings>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    sel.reset(catalog.as_deref());
    preview.scene = Some(crate::portrait::GlueScene::Race(sel.race));
    preview.look = Some(GlueLook::Create(sel.look()));
    preview.yaw = INITIAL_FACING;
    if let Some(mut wa) = world_assets {
        art.ensure_loaded(&mut wa, &mut images, &mut add_mats);
    }
    spawn_screen(
        &mut commands,
        &assets,
        &portraits,
        &art,
        strings.as_deref(),
        &window,
    );
}

/// Rebuild the tree when a window resize (mac fullscreen, a drag) has changed the glue scale it
/// was baked at — the create screen has no per-frame materialize (it spawns on entry, after the
/// boot-order traps the other screens dodge), so the rescale watch lives here. Selection and the
/// typed name live in resources and repaint via [`super::refresh`], so the rebuild loses nothing.
pub(super) fn rescale_screen(
    mut commands: Commands,
    existing: Query<(Entity, &CharCreateUi)>,
    assets: Res<AssetServer>,
    portraits: Res<PortraitImages>,
    art: Res<GlueArt>,
    strings: Option<Res<GlueStrings>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let s = crate::glue::screen_scale(window.single().ok());
    for (root, ui) in &existing {
        if ui.s != s {
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
    // The edit box types in `GlueEditBoxFont` — ARIALN, not FRIZQT (GlueFonts.xml).
    let edit_font: Handle<Font> = assets.load("mpq://Fonts/ARIALN.ttf");
    let model_image = match portraits.0.get(GLUE_SLOT) {
        Some(PortraitSource::Live(h)) => Some(h.clone()),
        _ => None,
    };
    // The glue engine scales a 1024×768 virtual screen to the window; scale the authored sizes the
    // same way so the ref proportions hold at any size.
    let s = crate::glue::screen_scale(window.single().ok());
    let px = |v: f32| Val::Px(v * s);
    let empty = GlueStrings::default();
    let strings = strings.unwrap_or(&empty);

    let root = commands
        .spawn((
            CharCreateUi { s },
            DynTint::Backdrop,
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
            // screen IS a fullscreen ModelFFX: the per-race background with the character standing
            // in it (the booth renders both into this window-sized target). The whole pane drags
            // to rotate, the ref's full-frame mouse rotation; the page tint behind it is the
            // no-art fallback. It keeps the WINDOW while the chrome below does not: a pillarbox's
            // bars are the booth camera's own output clear inside this same target (1619 §3).
            let mut pane = ui.spawn((
                CreateAction::Model,
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
    // 2091). The race/class towers are this screen's edge-anchored chrome: against the window they
    // stand over the bars, and 1587's "no void at 21:9" held only because they did (1619 §2).
    let mut canvas = commands.spawn((crate::glue::glue_canvas(), ChildOf(root)));
    canvas.with_children(|ui| {
        left_tower(ui, art, &font, s, strings);

        // The WoW logo (`CharacterCreateWoWLogo`, 256×128 at (3,−7)) — after the tower, like the
        // ref's frame order (child frames draw over the parent's border art).
        if let Some(logo) = &art.logo {
            ui.spawn((ImageNode::new(logo.clone()), abs(s, 3.0, 7.0, 256.0, 128.0)));
        }

        super::panels::right_stack(ui, art, &font, s);
        name_cluster(ui, art, &font, &edit_font, s, strings);
        rotate_cluster(ui, art, &font, s);

        // Accept over Back, bottom-right (`CharCreateOkayButton` 160×35 over `BackButton` 120×30
        // at BOTTOMRIGHT (−50, 20)).
        ui.spawn((Node {
            position_type: PositionType::Absolute,
            right: px(50.0),
            bottom: px(20.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: px(5.0),
            ..default()
        },))
            .with_children(|actions| {
                glue_button(
                    actions,
                    art,
                    &font,
                    CreateAction::Create,
                    strings.text("CHARACTER_CREATE_ACCEPT", "Accept"),
                    160.0,
                    35.0,
                    GlueBtnKind::Normal,
                    s,
                );
                glue_button(
                    actions,
                    art,
                    &font,
                    CreateAction::Back,
                    strings.text("BACK", "Back"),
                    120.0,
                    30.0,
                    GlueBtnKind::Small,
                    s,
                );
            });
    });
}

/// The configuration tower (`CharacterCreateConfigurationFrame`, 206×600 at TOPLEFT (28,−74)):
/// frame art, banners, faction headers, the race/gender/class grids, the dial rows, Randomize —
/// every child at its authored offset.
fn left_tower(
    ui: &mut ChildSpawnerCommands,
    art: &GlueArt,
    font: &Handle<Font>,
    s: f32,
    strings: &GlueStrings,
) {
    let px = |v: f32| Val::Px(v * s);
    ui.spawn((Node {
        position_type: PositionType::Absolute,
        left: px(28.0),
        top: px(74.0),
        width: px(206.0),
        height: px(600.0),
        ..default()
    },))
        .with_children(|tower| {
            // The frame: `UI-CharacterCreate-Background` stretched behind (TOPLEFT of border1 +6 →
            // BOTTOMLEFT of border3 +6), three `OuterBorder` pieces stacked over it (224 wide,
            // centered on the 206 frame → x −9; heights 236/240/210 with the authored texcoords).
            if let Some(bg) = &art.tower_bg {
                tower.spawn((ImageNode::new(bg.clone()), abs(s, -3.0, 0.0, 218.0, 680.0)));
            }
            if let Some((border, size)) = &art.tower_border {
                for (top, height, tc) in [
                    (0.0, 236.0, [0.0, 0.875, 0.0, 0.9375]),
                    (236.0, 240.0, [0.0, 0.875, 0.0, 0.9375]),
                    (476.0, 210.0, [0.0, 0.875, 0.1796875, 1.0]),
                ] {
                    tower.spawn((
                        ImageNode {
                            image: border.clone(),
                            rect: Some(tc_rect(*size, tc)),
                            ..default()
                        },
                        abs(s, -9.0, top, 224.0, height),
                    ));
                }
            }
            // The banners (`CharacterCreateBanners`, 256×259 at TOP (−2,−60)) behind the race grid.
            if let Some(banners) = &art.banners {
                tower.spawn((
                    ImageNode::new(banners.clone()),
                    abs(s, -27.0, 60.0, 256.0, 259.0),
                ));
            }
            // Alliance | Horde over the banner tops (bottom-anchored ±50 of the banner center).
            // The XML's `text="ALLIANCE"` is a localization key — GlueStrings renders it
            // mixed-case ("Alliance"), never the raw key.
            for (key, fallback, center) in
                [("ALLIANCE", "Alliance", 51.0), ("HORDE", "Horde", 151.0)]
            {
                outlined_text(
                    tower,
                    Node {
                        justify_content: JustifyContent::Center,
                        ..abs(s, center - 60.0, 40.0, 120.0, 17.0)
                    },
                    (),
                    (),
                    GlueText {
                        text: strings.text(key, fallback),
                        size: 15.0, // GlueFontNormal
                        color: GOLD,
                        wrap: false,
                    },
                    font,
                    s,
                );
            }

            // The race grid: two columns of 48² check-buttons (col A at (33,68), col B at (127,68),
            // row pitch 48+5).
            for (faction, left) in [(ALLIANCE, 33.0), (HORDE, 127.0)] {
                tower
                    .spawn((Node {
                        position_type: PositionType::Absolute,
                        left: px(left),
                        top: px(68.0),
                        flex_direction: FlexDirection::Column,
                        row_gap: px(5.0),
                        ..default()
                    },))
                    .with_children(|col| {
                        for race in faction {
                            icon_button(
                                col,
                                font,
                                CreateAction::Race(race),
                                Some(DynIcon::Race(race)),
                                None,
                                None::<DynText>,
                                race_name(race),
                                art,
                                s,
                            );
                        }
                    });
            }

            // The gender pair (below race col A: race4's BOTTOMLEFT + (20,−28)).
            tower
                .spawn((Node {
                    position_type: PositionType::Absolute,
                    left: px(53.0),
                    top: px(303.0),
                    flex_direction: FlexDirection::Row,
                    column_gap: px(5.0),
                    ..default()
                },))
                .with_children(|g| {
                    for (sex, key, fallback) in [(0u8, "MALE", "Male"), (1u8, "FEMALE", "Female")] {
                        let icon = art.gender.as_ref().map(|(h, size)| {
                            let half = if sex == 0 {
                                [0.0, 0.5, 0.0, 1.0]
                            } else {
                                [0.5, 1.0, 0.0, 1.0]
                            };
                            (h.clone(), tc_rect(*size, half))
                        });
                        icon_button(
                            g,
                            font,
                            CreateAction::Gender(sex),
                            None::<DynIcon>,
                            icon,
                            None::<DynText>,
                            strings.text(key, fallback),
                            art,
                            s,
                        );
                    }
                });

            // The class grid (3-wide under the banners: cols at x 27/79/131, rows touching) — 8
            // slots refreshed to the selected race's valid classes; unused slots collapse (the
            // ref's enumerate-then-hide compacts the same way).
            tower
                .spawn((Node {
                    position_type: PositionType::Absolute,
                    left: px(27.0),
                    top: px(369.0),
                    width: px(3.0 * 48.0 + 2.0 * 4.0),
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: px(4.0),
                    ..default()
                },))
                .with_children(|grid| {
                    for slot in 0..8u8 {
                        icon_button(
                            grid,
                            font,
                            CreateAction::ClassSlot(slot),
                            Some(DynIcon::ClassSlot(slot)),
                            None,
                            Some(DynText::ClassSlotLabel(slot)),
                            "",
                            art,
                            s,
                        );
                    }
                });

            // The five dial spinners (`CharacterCustomizationFrameTemplate`, 198×32 rows stacked
            // from (4,480) — centered under the class grid).
            tower
                .spawn((Node {
                    position_type: PositionType::Absolute,
                    left: px(4.0),
                    top: px(480.0),
                    width: px(198.0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },))
                .with_children(|dials| {
                    for dial in 0..5u8 {
                        dial_row(dials, art, font, dial, s);
                    }
                });

            // RANDOMIZE (146×30, centered on the row column). The XML anchors it 25 below the
            // dials, but `CharacterCreate_UpdateFacialHairCustomization` re-anchors it on every
            // race set — `SetPoint("TOP", Frame5, "BOTTOM", 0, -5)` — so the shipped client always
            // shows the 5px gap.
            tower
                .spawn((Node {
                    position_type: PositionType::Absolute,
                    left: px(30.0),
                    top: px(645.0),
                    ..default()
                },))
                .with_children(|r| {
                    glue_button(
                        r,
                        art,
                        font,
                        CreateAction::Randomize,
                        strings.text("RANDOMIZE", "Randomize"),
                        146.0,
                        30.0,
                        GlueBtnKind::Small,
                        s,
                    );
                });
        });
}

/// One dial spinner row: the `CharacterCreate-LabelFrame` 3-slice (64-tall art overhanging the
/// 32-tall row), the centered per-race label, and the 32² arrow pair on the right.
fn dial_row(
    dials: &mut ChildSpawnerCommands,
    art: &GlueArt,
    font: &Handle<Font>,
    dial: u8,
    s: f32,
) {
    let px = |v: f32| Val::Px(v * s);
    dials
        .spawn((
            DialRow(dial),
            Node {
                width: px(198.0),
                height: px(32.0),
                ..default()
            },
        ))
        .with_children(|row| {
            // LabelFrame: Left 25 at (−5), Middle stretched, Right 25 ending at x 154 (RIGHT −44,
            // clearing the arrows) — the 128×64 art's 25|78|25 horizontal slices.
            if let Some((frame, size)) = &art.label_frame {
                for (left, width, tc) in [
                    (-5.0, 25.0, [0.0, 0.1953125, 0.0, 1.0]),
                    (20.0, 109.0, [0.1953125, 0.8046875, 0.0, 1.0]),
                    (129.0, 25.0, [0.8046875, 1.0, 0.0, 1.0]),
                ] {
                    row.spawn((
                        ImageNode {
                            image: frame.clone(),
                            rect: Some(tc_rect(*size, tc)),
                            ..default()
                        },
                        abs(s, left, -16.0, width, 64.0),
                    ));
                }
            }
            // The per-race label, centered on the frame middle (`GlueFontHighlightSmall`).
            outlined_text(
                row,
                Node {
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..abs(s, 20.0, 0.0, 109.0, 32.0)
                },
                (),
                DynText::DialLabel(dial),
                GlueText {
                    text: "",
                    size: 12.0,
                    color: INFO_TEXT,
                    wrap: false,
                },
                font,
                s,
            );
            dial_arrow(
                row,
                &art.arrow_left,
                font,
                CreateAction::Dial(dial, -1),
                137.0,
                "<",
                s,
            );
            dial_arrow(
                row,
                &art.arrow_right,
                font,
                CreateAction::Dial(dial, 1),
                166.0,
                ">",
                s,
            );
        });
}

/// NAME over the edit box (`CharacterCreateNameEdit`, 156×40 at BOTTOM (8,50), backdropped in
/// `Glue-Tooltip-Border` — always Alliance-tinted, the ref's `OnLoad`), our status line beneath.
fn name_cluster(
    ui: &mut ChildSpawnerCommands,
    art: &GlueArt,
    font: &Handle<Font>,
    edit_font: &Handle<Font>,
    s: f32,
    strings: &GlueStrings,
) {
    let px = |v: f32| Val::Px(v * s);
    ui.spawn((Node {
        position_type: PositionType::Absolute,
        bottom: px(50.0),
        left: px(8.0),
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: px(2.0),
        ..default()
    },))
        .with_children(|cluster| {
            outlined_text(
                cluster,
                Node::default(),
                (),
                (),
                GlueText {
                    text: strings.text("NAME", "Name"),
                    size: 18.0, // GlueFontNormalLarge
                    color: GOLD,
                    wrap: false,
                },
                font,
                s,
            );
            // The shared glue edit-box chrome — Alliance-tinted, always (the
            // ref's `OnLoad`); the create refresh writes the typed name into the marker.
            crate::glue::widgets::glue_edit_box(
                cluster,
                art,
                edit_font,
                (),
                DynText::Name,
                (156.0, 40.0),
                (ALLIANCE_BORDER, ALLIANCE_FILL),
                (15.0, 0.0, 0.0, 0.0), // CharacterCreate.xml: TextInsets left 15 only
                s,
            );
            // Empty until a create fails — the ref surfaces errors in a dialog; this line is our
            // minimal stand-in, never an idle hint.
            outlined_text(
                cluster,
                Node::default(),
                (),
                StatusLine,
                GlueText {
                    text: "",
                    size: 13.0,
                    color: DIM,
                    wrap: true,
                },
                font,
                s,
            );
        });
}

/// The rotate pair (`CharacterCreateRotateLeft/Right`: 50² at BOTTOMLEFT (237,0), overlapping
/// −19) — `UI-RotationRight-Big` art, the left button mirrored, `UI-Common-MouseHilight` on hover.
fn rotate_cluster(ui: &mut ChildSpawnerCommands, art: &GlueArt, font: &Handle<Font>, s: f32) {
    let px = |v: f32| Val::Px(v * s);
    ui.spawn((Node {
        position_type: PositionType::Absolute,
        left: px(237.0),
        bottom: px(0.0),
        flex_direction: FlexDirection::Row,
        ..default()
    },))
        .with_children(|rot| {
            for (action, flip, overlap) in [
                (CreateAction::RotateLeft, true, 0.0),
                (CreateAction::RotateRight, false, -19.0),
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
                            b.insert(ArtSwap {
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
                        b.insert((FallbackFace, BackgroundColor(BTN_BG)));
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

pub(super) fn exit_create(
    mut commands: Commands,
    roots: Query<Entity, With<CharCreateUi>>,
    mut preview: ResMut<GluePreview>,
) {
    for e in &roots {
        commands.entity(e).despawn();
    }
    // Clear the booth + scene. Back to CharSelect re-establishes both the same frame
    // (its `OnEnter` runs after this `OnExit`).
    preview.look = None;
    preview.scene = None;
}
