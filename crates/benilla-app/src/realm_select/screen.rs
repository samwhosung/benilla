//! The realm list's layout: `GlueXML/RealmList.xml` in Bevy UI, on the 1024×768 glue scale.
//!
//! A full-screen dim, then the 640×512 `HelpFrame` plate (the AddOns list's) with the title plate,
//! four sort headers, eighteen rows, the selection band, the close X, and Okay and Cancel. Rows are
//! spawned empty and [`refresh_rows`] writes them every frame from [`super::Realms`].
//!
//! The category tab strip is not built: `RealmList_UpdateTabs` hides it for a single category, and
//! with several, only the first category's realms show.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::glue::art::{tc_rect, GlueArt, COLUMN_TAB_TC, GOLD, SORT_ARROW_TC};
use crate::glue::widgets::{
    abs, glue_button, outlined_text, overlay, ArtSwap, GlueBtnKind, GlueText, Hilight,
};
use crate::glue_strings::GlueStrings;

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

/// `RealmListUpdate`: write every visible row, each frame, since any realm's population moves
/// every band.
#[allow(clippy::type_complexity)]
pub(super) fn refresh_rows(
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
    mut ok: Query<&mut crate::glue::widgets::GlueDisabled, With<OkButton>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
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
                let (key, c) = load::type_column(realm.realm_type);
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
