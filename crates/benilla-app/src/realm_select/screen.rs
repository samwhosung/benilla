//! The realm list's layout: `GlueXML/RealmList.xml` in Bevy UI, on the 1024×768 glue scale.
//!
//! A full-screen dim, then the 640×512 `HelpFrame` plate (the AddOns list's) with the title plate,
//! four sort headers, the eight category tabs under the plate, eighteen rows, the selection band,
//! the close X, and Okay and Cancel. Rows and tabs are spawned empty and [`refresh_list`] writes
//! them every frame from [`super::Realms`].
//!
//! The tab strip shows only with two or more categories holding a realm (`RealmList_UpdateTabs`),
//! one tab per category, and the rows are the front tab's.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::glue::art::{tc_rect, GlueArt, COLUMN_TAB_TC, GOLD, REALM_TAB_TC, SORT_ARROW_TC};
use crate::glue::widgets::{
    abs, glue_button, outlined_text, overlay, ArtSwap, GlueBtnKind, GlueDisabled, GlueText, Hilight,
};
use crate::glue_strings::GlueStrings;

use super::category::MAX_TABS;
use super::load;
use super::{Realms, SortKey};

use crate::char_select::wow_font;

/// `frameStrata="DIALOG"` with `toplevel`: above the glue screen, its dialogs and the AddOns
/// tooltip (z 1100, 1200, 1220).
const REALM_Z: i32 = 1250;

/// The panel plate, from `RealmList.xml`.
const BG_W: f32 = 640.0;
const BG_H: f32 = 512.0;
/// `RealmListBackground`'s authored CENTER offset.
const BG_CENTER_OFF_X: f32 = 24.0;

/// `MAX_REALMS_DISPLAYED` (`RealmList.lua:2`).
pub(super) const MAX_ROWS: usize = 18;
/// The row pitch: a 16-tall button plus the 4 px anchor offset; not `REALM_BUTTON_HEIGHT` (16),
/// which is only the scrollbar's step.
const ROW_PITCH: f32 = 20.0;
/// `RealmListRealmButton1` at TOPLEFT (22, −56).
const ROW0_LEFT: f32 = 22.0;
const ROW0_TOP: f32 = 56.0;
const ROW_W: f32 = 512.0;
const ROW_H: f32 = 16.0;
/// `RealmListHighlight`, wider than the row it sits behind.
const HILIGHT_W: f32 = 557.0;

/// The four sort columns: `(key, string key, left, width)`. The lefts chain off
/// `RealmNameSort`'s TOPLEFT (21) through each button's authored width.
const SORT_COLUMNS: [(SortKey, &str, f32, f32); 4] = [
    (SortKey::Name, "REALM_NAME", 21.0, 223.0),
    (SortKey::Type, "REALM_TYPE", 244.0, 80.0),
    (SortKey::Characters, "REALM_CHARACTERS", 324.0, 110.0),
    (SortKey::Load, "REALM_LOAD", 434.0, 144.0),
];
/// `RealmSortButtonTemplate`'s height, and the widths of its two `WhoFrame-ColumnTabs` end caps.
const SORT_H: f32 = 19.0;
const SORT_CAP_L: f32 = 5.0;
const SORT_CAP_R: f32 = 4.0;
/// The sort header's top: its BOTTOMLEFT is anchored at −50 off the plate's TOPLEFT.
const SORT_TOP: f32 = 50.0 - SORT_H;

/// A row's four column boxes, chained off the `RealmListRealmButtonTemplate` anchors:
/// `NormalText` 220 wide at LEFT +5, `PVP` 50 wide at its RIGHT +10, `Players` 32 wide at that
/// RIGHT +51, `Load` 115 wide at that RIGHT +50.
const COL_NAME: (f32, f32) = (5.0, 220.0);
const COL_TYPE: (f32, f32) = (235.0, 50.0);
const COL_PLAYERS: (f32, f32) = (336.0, 32.0);
const COL_LOAD: (f32, f32) = (418.0, 115.0);

/// `RealmListTab1`'s BOTTOMLEFT sits at the plate's BOTTOMLEFT (11, −15), so the 32-tall tabs hang
/// below the plate's lower edge.
const TAB_LEFT: f32 = 11.0;
const TAB_H: f32 = 32.0;
const TAB_TOP: f32 = BG_H + 15.0 - TAB_H;
/// Each next tab's LEFT at the one before's RIGHT −15.
const TAB_OVERLAP: f32 = 15.0;
/// `$parentLeft` and `$parentRight`, 20 wide; `GlueTemplates_TabResize(0)` makes the middle the
/// label's own width.
const TAB_END: f32 = 20.0;
/// `$parentLeftDisabled` at TOPLEFT (0, 3): the front tab's art stands 3 higher.
const TAB_FRONT_RISE: f32 = 3.0;
/// `$parentText` at CENTER (0, 2), and the highlight's two anchors at y 2.
const TAB_TEXT_RISE: f32 = 2.0;
/// `$parentHighlightTexture`, anchored 10 in from each side; its height is its art's, 32.
const TAB_HILIGHT_INSET: f32 = 10.0;

/// Root of the realm list; `with_art` and `s` trigger a rebuild when the art lands or the scale
/// changes.
#[derive(Component)]
pub(super) struct RealmListUi {
    with_art: bool,
    s: f32,
}

/// One clickable control on the screen.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum RealmAction {
    /// A realm row: the 0-based screen row, resolved against the scroll offset at click time.
    Row(usize),
    Ok,
    /// The Cancel button and Escape, `RealmList_OnCancel`.
    Cancel,
    /// The close X: `RealmList:Hide()`, silent (`GlueCloseButton`, `GlueTemplates.xml:4`,
    /// declares no sound) where Cancel plays `gsLoginChangeRealmCancel`.
    Close,
    Sort(SortKey),
    /// `RealmListTab<n>`: the category tab with that 1-based ordinal.
    Tab(usize),
}

/// The screen row of a row button and of each of its four texts.
#[derive(Component, Clone, Copy)]
pub(super) struct RowOf(pub(super) usize);

/// Which of a row's four columns a text entity is.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum Column {
    Name,
    Type,
    Players,
    Load,
}
/// The selection band, moved and tinted rather than respawned.
#[derive(Component)]
pub(super) struct RowHighlight;
/// The Okay button, so the refresh can grey it out.
#[derive(Component)]
pub(super) struct OkButton;
/// A category tab's button, label and art pieces, by the tab's 1-based ordinal.
#[derive(Component, Clone, Copy)]
pub(super) struct TabOf(pub(super) usize);
/// One of a tab's six art pieces: the front tab's `ActiveTab` set, or the others' `InActiveTab`.
#[derive(Component, Clone, Copy)]
pub(super) struct TabArt {
    front: bool,
}

/// Spawn, rebuild and despawn the dialog as `shown` says, over whatever glue screen is current.
pub(super) fn drive_screen(
    mut commands: Commands,
    realms: Res<Realms>,
    existing: Query<(Entity, &RealmListUi)>,
    assets: Res<AssetServer>,
    mut art: ResMut<GlueArt>,
    world_assets: Option<ResMut<benilla_assets::WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut add_mats: ResMut<Assets<crate::glue::add_material::AddUiMaterial>>,
    strings: Option<Res<GlueStrings>>,
    window: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
) {
    if !realms.shown {
        for (root, _) in &existing {
            commands.entity(root).despawn();
        }
        return;
    }
    if let Some(mut wa) = world_assets {
        art.ensure_loaded(&mut wa, &mut images, &mut add_mats);
    }
    let with_art = art.help_frame.is_some();
    let s = crate::glue::screen_scale(window.single().ok());
    match existing.single() {
        Ok((root, ui)) => {
            if (!ui.with_art && with_art) || ui.s != s {
                commands.entity(root).despawn();
                spawn_screen(
                    &mut commands,
                    &assets,
                    &art,
                    strings.as_deref(),
                    s,
                    with_art,
                );
            }
        }
        Err(_) => {
            if with_art || time.elapsed_secs() > 1.0 {
                spawn_screen(
                    &mut commands,
                    &assets,
                    &art,
                    strings.as_deref(),
                    s,
                    with_art,
                );
            }
        }
    }
}

fn spawn_screen(
    commands: &mut Commands,
    assets: &AssetServer,
    art: &GlueArt,
    strings: Option<&GlueStrings>,
    s: f32,
    with_art: bool,
) {
    let px = |v: f32| Val::Px(v * s);
    let font = wow_font(assets);
    let text = |key: &'static str| match strings {
        Some(g) => g.text(key, key).to_string(),
        None => key.to_string(),
    };

    commands
        .spawn((
            RealmListUi { with_art, s },
            GlobalZIndex(REALM_Z),
            // `enableMouse="true"` on a full-screen frame: clicks never reach the screen below.
            Button,
            // The reference's full-screen BACKGROUND layer.
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75)),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .with_children(|screen| {
            let mut boxed = screen.spawn(Node {
                width: px(BG_W),
                height: px(BG_H),
                left: px(BG_CENTER_OFF_X),
                ..default()
            });
            boxed.with_children(|b| {
                spawn_plate(b, art, s);
                spawn_header(b, art, &font, &text("SERVER_SELECTION"), s);
                spawn_close(b, art, &font, s);
                spawn_sort_headers(b, art, &font, strings, s);
                spawn_tabs(b, art, &font, s);
                spawn_highlight(b, art, s);
                for row in 0..MAX_ROWS {
                    spawn_row(b, &font, row, s);
                }
                // `GlueDialogButtonTemplate` 125×35: Cancel at BOTTOMRIGHT (−46, +13), Okay off
                // its left edge with an 8 px overlap.
                let cancel_left = BG_W - 46.0 - 125.0;
                let btn_top = BG_H - 13.0 - 35.0;
                for (action, key, left) in [
                    (RealmAction::Ok, "OKAY", cancel_left - 125.0 + 8.0),
                    (RealmAction::Cancel, "CANCEL", cancel_left),
                ] {
                    let mut wrap = b.spawn(abs(s, left, btn_top, 125.0, 35.0));
                    wrap.with_children(|w| {
                        let e = glue_button(
                            w,
                            art,
                            &font,
                            action,
                            &text(key),
                            125.0,
                            35.0,
                            GlueBtnKind::Dialog,
                            s,
                        );
                        if action == RealmAction::Ok {
                            w.commands().entity(e).insert(OkButton);
                        }
                    });
                }
            });
        });
}

/// The six-piece `HelpFrame` plate, two rows of three.
fn spawn_plate(b: &mut ChildSpawnerCommands, art: &GlueArt, s: f32) {
    match &art.help_frame {
        Some(hf) => {
            for (img, l, t, w, h) in [
                (&hf.tl, 0.0, 0.0, 256.0, 256.0),
                (&hf.top, 256.0, 0.0, 256.0, 256.0),
                (&hf.tr, 512.0, 0.0, 128.0, 256.0),
                (&hf.bl, 0.0, 256.0, 256.0, 256.0),
                (&hf.bottom, 256.0, 256.0, 256.0, 256.0),
                (&hf.br, 512.0, 256.0, 128.0, 256.0),
            ] {
                b.spawn((ImageNode::new(img.clone()), abs(s, l, t, w, h)));
            }
        }
        None => {
            b.spawn((
                BackgroundColor(Color::srgba(0.05, 0.05, 0.08, 0.95)),
                overlay(),
            ));
        }
    }
}

/// `UI-DialogBox-Header` (256×64) at TOP (−12, +12), with `SERVER_SELECTION` 14 below its top.
fn spawn_header(
    b: &mut ChildSpawnerCommands,
    art: &GlueArt,
    font: &Handle<Font>,
    title: &str,
    s: f32,
) {
    let left = (BG_W - 256.0) / 2.0 - 12.0;
    if let Some((header, _)) = &art.dialog_header {
        b.spawn((
            ImageNode::new(header.clone()),
            abs(s, left, -12.0, 256.0, 64.0),
        ));
    }
    outlined_text(
        b,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(left * s),
            width: Val::Px(256.0 * s),
            top: Val::Px(2.0 * s),
            justify_content: JustifyContent::Center,
            ..default()
        },
        (),
        (),
        GlueText {
            text: title,
            size: 12.0, // GlueFontNormalSmall
            color: GOLD,
            wrap: false,
        },
        font,
        s,
    );
}

/// `GlueCloseButton` at TOPRIGHT (−42, −3).
fn spawn_close(b: &mut ChildSpawnerCommands, art: &GlueArt, font: &Handle<Font>, s: f32) {
    let mut x = b.spawn((
        RealmAction::Close,
        Button,
        abs(s, BG_W - 42.0 - 32.0, 3.0, 32.0, 32.0),
    ));
    match &art.close_btn {
        Some(cb) => {
            x.insert((
                ImageNode::new(cb.up.clone()),
                ArtSwap {
                    up: cb.up.clone(),
                    down: cb.down.clone(),
                },
            ));
            if let Some(hi) = &cb.hi {
                x.with_children(|x| {
                    x.spawn((
                        Hilight,
                        Visibility::Hidden,
                        MaterialNode(hi.clone()),
                        overlay(),
                    ));
                });
            }
        }
        None => {
            x.with_children(|x| {
                outlined_text(
                    x,
                    Node {
                        left: Val::Px(10.0 * s),
                        top: Val::Px(6.0 * s),
                        ..default()
                    },
                    (),
                    (),
                    GlueText {
                        text: "X",
                        size: 14.0,
                        color: GOLD,
                        wrap: false,
                    },
                    font,
                    s,
                );
            });
        }
    }
}

/// The four column headers (`RealmSortButtonTemplate`): a three-slice `WhoFrame-ColumnTabs`
/// plate, the label with `UI-SortArrow`, and the `UI-Character-Tab-Highlight` hover sheen.
fn spawn_sort_headers(
    b: &mut ChildSpawnerCommands,
    art: &GlueArt,
    font: &Handle<Font>,
    strings: Option<&GlueStrings>,
    s: f32,
) {
    for (key, string_key, left, w) in SORT_COLUMNS {
        let label = strings
            .map(|g| g.text(string_key, string_key).to_string())
            .unwrap_or_else(|| string_key.to_string());
        b.spawn((
            RealmAction::Sort(key),
            Button,
            abs(s, left, SORT_TOP, w, SORT_H),
        ))
        .with_children(|h| {
            if let Some((tex, size)) = &art.column_tabs {
                for (tc, l, cw) in [
                    (COLUMN_TAB_TC[0], 0.0, SORT_CAP_L),
                    (COLUMN_TAB_TC[1], SORT_CAP_L, w - SORT_CAP_L - SORT_CAP_R),
                    (COLUMN_TAB_TC[2], w - SORT_CAP_R, SORT_CAP_R),
                ] {
                    h.spawn((
                        ImageNode {
                            image: tex.clone(),
                            rect: Some(tc_rect(*size, tc)),
                            ..default()
                        },
                        abs(s, l, 0.0, cw, SORT_H),
                    ));
                }
            }
            // The sheen, LEFT..RIGHT+4 and 24 tall, centred on the button.
            if let Some(hi) = &art.tab_highlight {
                h.spawn((
                    Hilight,
                    Visibility::Hidden,
                    MaterialNode(hi.clone()),
                    abs(s, 0.0, (SORT_H - 24.0) / 2.0, w + 4.0, 24.0),
                ));
            }
            // `$parentText` at LEFT +8, with `$parentArrow` 3 to the right of its right edge.
            let mut row = h.spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Px(8.0 * s),
                top: Val::Px(0.0),
                height: Val::Px(SORT_H * s),
                align_items: AlignItems::Center,
                column_gap: Val::Px(3.0 * s),
                ..default()
            });
            row.with_children(|r| {
                outlined_text(
                    r,
                    Node::default(),
                    (),
                    (),
                    GlueText {
                        text: &label,
                        size: 12.0, // GlueFontHighlightSmall
                        color: load::HIGHLIGHT,
                        wrap: false,
                    },
                    font,
                    s,
                );
                if let Some((tex, size)) = &art.sort_arrow {
                    r.spawn((
                        ImageNode {
                            image: tex.clone(),
                            rect: Some(tc_rect(*size, SORT_ARROW_TC)),
                            ..default()
                        },
                        Node {
                            width: Val::Px(9.0 * s),
                            height: Val::Px(8.0 * s),
                            top: Val::Px(-2.0 * s),
                            ..default()
                        },
                    ));
                }
            });
        });
    }
}

/// `RealmListTab1`..`8` (`RealmListTabButtonTemplate`), spawned hidden in a row that chains each
/// tab 15 into the one before. A tab is its two 20-wide ends round a middle as wide as the label,
/// in both art sets; [`paint_tabs`] labels them and picks the set.
fn spawn_tabs(b: &mut ChildSpawnerCommands, art: &GlueArt, font: &Handle<Font>, s: f32) {
    let px = |v: f32| Val::Px(v * s);
    let mut strip = b.spawn(Node {
        position_type: PositionType::Absolute,
        left: px(TAB_LEFT),
        top: px(TAB_TOP),
        height: px(TAB_H),
        flex_direction: FlexDirection::Row,
        ..default()
    });
    strip.with_children(|strip| {
        for ordinal in 1..=MAX_TABS {
            let overlap = if ordinal == 1 { 0.0 } else { -TAB_OVERLAP };
            let mut tab = strip.spawn((
                RealmAction::Tab(ordinal),
                TabOf(ordinal),
                Button,
                GlueDisabled(false),
                Visibility::Hidden,
                Node {
                    height: px(TAB_H),
                    flex_shrink: 0.0,
                    margin: UiRect::left(px(overlap)),
                    padding: UiRect::horizontal(px(TAB_END)),
                    align_items: AlignItems::Center,
                    ..default()
                },
            ));
            tab.with_children(|t| {
                for (front, sheet) in [(false, &art.tab_inactive), (true, &art.tab_active)] {
                    let Some((tex, size)) = sheet else {
                        continue;
                    };
                    let top = if front { -TAB_FRONT_RISE } else { 0.0 };
                    for (piece, tc) in REALM_TAB_TC.iter().enumerate() {
                        let (left, right, width) = match piece {
                            0 => (px(0.0), Val::Auto, px(TAB_END)),
                            1 => (px(TAB_END), px(TAB_END), Val::Auto),
                            _ => (Val::Auto, px(0.0), px(TAB_END)),
                        };
                        t.spawn((
                            TabOf(ordinal),
                            TabArt { front },
                            Visibility::Hidden,
                            ImageNode {
                                image: tex.clone(),
                                rect: Some(tc_rect(*size, *tc)),
                                ..default()
                            },
                            Node {
                                position_type: PositionType::Absolute,
                                left,
                                right,
                                width,
                                top: px(top),
                                height: px(TAB_H),
                                ..default()
                            },
                        ));
                    }
                }
                if let Some(hi) = &art.tab_highlight {
                    t.spawn((
                        Hilight,
                        Visibility::Hidden,
                        MaterialNode(hi.clone()),
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(TAB_HILIGHT_INSET),
                            right: px(TAB_HILIGHT_INSET),
                            top: px(-TAB_TEXT_RISE),
                            height: px(TAB_H),
                            ..default()
                        },
                    ));
                }
                let label = outlined_text(
                    t,
                    Node {
                        top: px(-TAB_TEXT_RISE),
                        ..default()
                    },
                    (),
                    (),
                    GlueText {
                        text: "",
                        size: 12.0, // GlueFontNormalSmall
                        color: GOLD,
                        wrap: false,
                    },
                    font,
                    s,
                );
                t.commands().entity(label).insert(TabOf(ordinal));
            });
        }
    });
}

/// `RealmListHighlight`: one band, moved to the selected row and vertex-coloured to match it.
fn spawn_highlight(b: &mut ChildSpawnerCommands, art: &GlueArt, s: f32) {
    let mut band = b.spawn((
        RowHighlight,
        Visibility::Hidden,
        abs(s, ROW0_LEFT, ROW0_TOP, HILIGHT_W, ROW_H),
    ));
    match &art.title_highlight {
        Some(tex) => {
            band.insert(ImageNode::new(tex.clone()));
        }
        None => {
            band.insert(BackgroundColor(Color::srgba(1.0, 0.78, 0.0, 0.25)));
        }
    }
}

/// One realm row: a 512×16 button carrying four text columns.
fn spawn_row(b: &mut ChildSpawnerCommands, font: &Handle<Font>, row: usize, s: f32) {
    let top = ROW0_TOP + row as f32 * ROW_PITCH;
    b.spawn((
        RealmAction::Row(row),
        RowOf(row),
        Button,
        Visibility::Hidden,
        abs(s, ROW0_LEFT, top, ROW_W, ROW_H),
    ))
    .with_children(|r| {
        // The name is `GlueFontNormal` (15); the three computed columns are the Small fonts (12).
        for ((col_left, col_w), size, column) in [
            (COL_NAME, 15.0, Column::Name),
            (COL_TYPE, 12.0, Column::Type),
            (COL_PLAYERS, 12.0, Column::Players),
            (COL_LOAD, 12.0, Column::Load),
        ] {
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(col_left * s),
                width: Val::Px(col_w * s),
                top: Val::Px(0.0),
                ..default()
            };
            let e = outlined_text(
                r,
                node,
                (),
                (),
                GlueText {
                    text: "",
                    size,
                    color: GOLD,
                    wrap: false,
                },
                font,
                s,
            );
            r.commands().entity(e).insert((RowOf(row), column));
        }
    });
}

/// The tab strip's buttons, art pieces and labels, kept apart from the rows' by their markers.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)]
pub(super) struct TabNodes<'w, 's> {
    tabs: Query<
        'w,
        's,
        (
            &'static TabOf,
            &'static Interaction,
            &'static mut Visibility,
            &'static mut GlueDisabled,
        ),
        (
            With<RealmAction>,
            Without<RowOf>,
            Without<OkButton>,
            Without<TabArt>,
        ),
    >,
    arts: Query<
        'w,
        's,
        (&'static TabOf, &'static TabArt, &'static mut Visibility),
        (Without<RealmAction>, Without<RowHighlight>),
    >,
    labels:
        Query<'w, 's, (&'static TabOf, &'static mut Text, &'static mut TextColor), Without<RowOf>>,
}

/// `RealmListUpdate`: the tab strip (`RealmList_UpdateTabs`), then every visible row, each frame,
/// since any realm's population moves every band.
#[allow(clippy::type_complexity)]
pub(super) fn refresh_list(
    realms: Res<Realms>,
    strings: Option<Res<GlueStrings>>,
    mut rows: Query<
        (&RowOf, &Interaction, &mut Visibility),
        (With<RealmAction>, Without<RowHighlight>),
    >,
    mut cols: Query<(&RowOf, &Column, &mut Text, &mut TextColor)>,
    mut band: Query<
        (&mut Node, &mut Visibility, Option<&mut ImageNode>),
        (With<RowHighlight>, Without<RealmAction>),
    >,
    mut ok: Query<&mut GlueDisabled, With<OkButton>>,
    mut tab_nodes: TabNodes,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    paint_tabs(&realms, &mut tab_nodes);
    let s = crate::glue::screen_scale(window.single().ok());
    let visible = realms.rows();
    let (mean, stddev) = realms.stats();
    let selected = realms.selected().map(|r| r.name.clone());
    let text = |key: &'static str| match strings.as_deref() {
        Some(g) => g.text(key, key).to_string(),
        None => key.to_string(),
    };
    let at = |row: usize| visible.get(realms.offset + row).copied();

    // A row's only hover state is the template's `HighlightFont`; it has no HighlightTexture.
    let mut hovered = None;
    for (RowOf(row), interaction, mut vis) in &mut rows {
        *vis = match at(*row) {
            Some(_) => Visibility::Inherited,
            None => Visibility::Hidden,
        };
        if *interaction != Interaction::None {
            hovered = Some(*row);
        }
    }

    let mut band_row = None;
    for (RowOf(row), column, mut t, mut color) in &mut cols {
        let Some(realm) = at(*row).map(|i| &realms.realms[i]) else {
            continue;
        };
        let down = super::is_down(realm);
        let invalid = super::is_invalid(realm);
        // `LockHighlight()` on the chosen row, `Disable()` on an offline one.
        let is_selected = selected.as_deref() == Some(realm.name.as_str()) && !down;
        let lit = !down && (is_selected || hovered == Some(*row));
        let (new, c) = match column {
            Column::Name => {
                if is_selected {
                    band_row = Some((*row, load::highlight_color(invalid, realm.characters)));
                }
                let (normal, highlight) = load::name_colors(down, invalid, realm.characters);
                (realm.name.clone(), if lit { highlight } else { normal })
            }
            // `RealmListUpdate` turns the selected row's type and load `HIGHLIGHT_FONT_COLOR`.
            Column::Type => {
                let (key, c) = load::type_column(realms.pvp_rp(realm.realm_type));
                (text(key), if is_selected { load::HIGHLIGHT } else { c })
            }
            Column::Players => (load::players_text(realm.characters), load::HIGHLIGHT),
            Column::Load => {
                let level = load::realm_load_classify(realm.flags, realm.population, mean, stddev);
                let (key, c) = load::load_column(down, level);
                (text(key), if is_selected { load::HIGHLIGHT } else { c })
            }
        };
        if t.0 != new {
            t.0 = new;
        }
        if color.0 != c {
            color.0 = c;
        }
    }

    if let Ok((mut node, mut vis, tint)) = band.single_mut() {
        match band_row {
            Some((row, color)) => {
                node.top = Val::Px((ROW0_TOP + row as f32 * ROW_PITCH) * s);
                *vis = Visibility::Inherited;
                if let Some(mut image) = tint {
                    if image.color != color {
                        image.color = color;
                    }
                }
            }
            None => *vis = Visibility::Hidden,
        }
    }
    if let Ok(mut disabled) = ok.single_mut() {
        let now = !realms.can_enter();
        if disabled.0 != now {
            disabled.0 = now;
        }
    }
}

/// `RealmList_UpdateTabs` and `GlueTemplates_UpdateTabs`: one category hides the strip; two to
/// eight show that many tabs, each labelled with its category's name. The front tab is
/// `Disable()`d on the `ActiveTab` art in its `DisabledFont` (`GlueFontHighlightSmall`, white);
/// the others stand on `InActiveTab` in gold, white with the highlight under the cursor.
fn paint_tabs(realms: &Realms, nodes: &mut TabNodes) {
    let TabNodes { tabs, arts, labels } = nodes;
    let names = realms.tabs();
    let shown = if names.len() >= 2 { names.len() } else { 0 };
    let front = realms.category();
    let mut hovered = None;
    for (TabOf(ordinal), interaction, mut vis, mut disabled) in tabs.iter_mut() {
        let want = if *ordinal <= shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        let is_front = *ordinal == front;
        if disabled.0 != is_front {
            disabled.0 = is_front;
        }
        if !is_front && *interaction != Interaction::None {
            hovered = Some(*ordinal);
        }
    }
    for (TabOf(ordinal), art, mut vis) in arts.iter_mut() {
        let want = if art.front == (*ordinal == front) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
    for (TabOf(ordinal), mut text, mut color) in labels.iter_mut() {
        let Some(name) = names.get(*ordinal - 1) else {
            continue;
        };
        if text.0 != *name {
            text.0 = name.to_string();
        }
        let c = if *ordinal == front || hovered == Some(*ordinal) {
            load::HIGHLIGHT
        } else {
            GOLD
        };
        if color.0 != c {
            color.0 = c;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glue::{glue_clicks, GlueClicks};
    use benilla_protocol::RealmInfo;

    fn realm(name: &str, category: u8) -> RealmInfo {
        RealmInfo {
            name: name.into(),
            address: "127.0.0.1:8085".into(),
            population: 1.0,
            characters: 0,
            realm_type: 0,
            flags: 0,
            category,
            id: 0,
        }
    }

    /// The tab strip as [`spawn_tabs`] builds it, both art sets stood in, under the US client's
    /// categories, with the click and refresh passes the plugin runs.
    fn tab_app(list: &[(&str, u8)]) -> App {
        let mut app = App::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.init_resource::<Time>()
            .init_resource::<GlueClicks>()
            .insert_resource(crate::net::RealmChoice(tx))
            .add_message::<crate::sound::GlueSound>()
            .insert_resource(Realms {
                realms: list.iter().map(|&(n, c)| realm(n, c)).collect(),
                categories: Some(super::super::category::tests::us()),
                shown: true,
                ..Realms::default()
            })
            .add_systems(
                Update,
                (glue_clicks, super::super::input::clicks, refresh_list).chain(),
            );
        let mut art = GlueArt::default();
        art.tab_active = Some((Handle::default(), Vec2::new(128.0, 32.0)));
        art.tab_inactive = Some((Handle::default(), Vec2::new(128.0, 32.0)));
        app.world_mut()
            .commands()
            .spawn(Node::default())
            .with_children(|b| spawn_tabs(b, &art, &Handle::default(), 1.0));
        app.world_mut().flush();
        app.update();
        app
    }

    /// Each shown tab's `(ordinal, label, front)`, in ordinal order.
    fn shown_tabs(app: &mut App) -> Vec<(usize, String, bool)> {
        let mut labels = app.world_mut().query::<(&TabOf, &Text)>();
        let labels: Vec<(usize, String)> = labels
            .iter(app.world())
            .map(|(t, text)| (t.0, text.0.clone()))
            .collect();
        let mut tabs = app
            .world_mut()
            .query::<(&TabOf, &Visibility, &GlueDisabled, &RealmAction)>();
        let mut out: Vec<(usize, String, bool)> = tabs
            .iter(app.world())
            .filter(|(_, vis, _, _)| **vis != Visibility::Hidden)
            .map(|(t, _, disabled, _)| {
                let label = labels.iter().find(|(o, _)| *o == t.0).unwrap().1.clone();
                (t.0, label, disabled.0)
            })
            .collect();
        out.sort();
        out
    }

    /// The front tab stands on the `ActiveTab` art alone, every other on `InActiveTab`.
    fn art_matches_front(app: &mut App, front: usize) -> bool {
        let mut arts = app.world_mut().query::<(&TabOf, &TabArt, &Visibility)>();
        arts.iter(app.world())
            .all(|(t, art, vis)| (*vis != Visibility::Hidden) == (art.front == (t.0 == front)))
    }

    fn click(app: &mut App, ordinal: usize) {
        let mut tabs = app.world_mut().query::<(Entity, &TabOf, &RealmAction)>();
        let tab = tabs
            .iter(app.world())
            .find(|(_, t, _)| t.0 == ordinal)
            .unwrap()
            .0;
        for interaction in [Interaction::Pressed, Interaction::Hovered] {
            *app.world_mut().get_mut::<Interaction>(tab).unwrap() = interaction;
            app.update();
        }
    }

    fn rows(app: &App) -> Vec<String> {
        let realms = app.world().resource::<Realms>();
        realms
            .rows()
            .iter()
            .map(|&i| realms.realms[i].name.clone())
            .collect()
    }

    /// One category holding a realm, a byte-0 realm folded in: `RealmList_UpdateTabs` hides all.
    #[test]
    fn one_category_hides_the_whole_strip() {
        let mut app = tab_app(&[("Alpha", 1), ("Dev", 0)]);
        assert!(shown_tabs(&mut app).is_empty());
        assert_eq!(rows(&app), ["Alpha", "Dev"]);
    }

    /// Two categories: two tabs named from `Cfg_Categories.dbc`, the first in front.
    #[test]
    fn two_categories_show_two_named_tabs_the_first_in_front() {
        let mut app = tab_app(&[("Alpha", 1), ("Down Under", 5)]);
        assert_eq!(
            shown_tabs(&mut app),
            [
                (1, "United States".to_string(), true),
                (2, "Oceanic".to_string(), false)
            ]
        );
        assert!(art_matches_front(&mut app, 1));
        assert_eq!(rows(&app), ["Alpha"]);
    }

    /// A click on the second tab fronts it and lists its realms; the front tab takes no click.
    #[test]
    fn a_tab_click_fronts_the_tab_and_relists() {
        let mut app = tab_app(&[("Alpha", 1), ("Down Under", 5)]);
        click(&mut app, 2);
        assert_eq!(rows(&app), ["Down Under"]);
        assert_eq!(
            shown_tabs(&mut app),
            [
                (1, "United States".to_string(), false),
                (2, "Oceanic".to_string(), true)
            ]
        );
        assert!(art_matches_front(&mut app, 2));

        click(&mut app, 1);
        assert_eq!(rows(&app), ["Alpha"]);
    }
}
