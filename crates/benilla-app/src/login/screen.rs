//! The login screen's layout: the reference `AccountLogin.xml` arrangement in Bevy UI, scaled by
//! `height / 768` (the glue virtual screen), over the `UI_MainMenu` scene. Each control carries
//! its `AccountLogin.xml` size and anchor at its spawn site.
//!
//! Deviation: the Credits, Cinematics and TOS buttons are absent, because the screen keeps only
//! what logging in needs.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::char_select::wow_font;
use crate::glue::art::{GlueArt, BACKDROP, GOLD};
use crate::glue::widgets::{
    abs, glue_button, glue_edit_box, outlined_text, overlay, paint_glue_field, ArtSwap,
    GlueBtnKind, GlueFieldPart, GlueText, Hilight,
};
use crate::glue_strings::GlueStrings;
use crate::portrait::{PortraitImages, PortraitSource, GLUE_SLOT};
use benilla_assets::WorldAssets;

use super::{ClientState, Field, LoginForm};

const SCREEN_Z: i32 = 1100;
/// `GlueFontDisableSmall`'s color (GlueFonts.xml), the `AccountLoginRealmName` readout's grey.
const DISABLED_GREY: Color = Color::srgb(0.5, 0.5, 0.5);
/// `DEFAULT_TOOLTIP_COLOR` (AccountLogin.lua): the edit boxes' backdrop tint (border rgb, bg rgb).
const BOX_BORDER: Color = Color::srgb(0.8, 0.8, 0.8);
const BOX_FILL: Color = Color::srgb(0.09, 0.09, 0.09);

/// One clickable control on the screen.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginAction {
    FocusAccount,
    FocusPassword,
    Login,
    Quit,
    ToggleSave,
    /// Open the realmlist editor, from the button or the address readout under it.
    Realmlist,
}

/// Root of the login screen; `with_art` and `s` (the glue scale) record what the tree was built
/// with, so late art or a window resize rebuilds it.
#[derive(Component)]
pub(super) struct LoginUi {
    with_art: bool,
    s: f32,
}
/// The account box's row items, painted by [`refresh_boxes`].
#[derive(Component, Clone)]
pub(super) struct AccountText;
/// The password box's row items, displayed as the `*` mask.
#[derive(Component, Clone)]
pub(super) struct PasswordText;
/// The checkbox's checked overlay, shown while the form's save flag is set.
#[derive(Component)]
pub(super) struct CheckMark;
/// The checkbox's hover highlight, driven by [`refresh_checkbox`].
#[derive(Component)]
pub(super) struct CheckHilight;
/// The realmlist readout under the button, in the reference's `AccountLoginRealmName` slot.
#[derive(Component)]
pub(super) struct RealmlistReadout;

/// Spawn the screen tree once its prerequisites exist (the initial state's `OnEnter` fires before
/// the MPQ chain and booth slots do), and rebuild an artless early spawn when the art lands.
pub(super) fn materialize_screen(
    mut commands: Commands,
    existing: Query<(Entity, &LoginUi)>,
    assets: Res<AssetServer>,
    portraits: Res<PortraitImages>,
    mut art: ResMut<GlueArt>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut add_mats: ResMut<Assets<crate::glue::add_material::AddUiMaterial>>,
    strings: Option<Res<GlueStrings>>,
    form: Res<LoginForm>,
    realmlist: Res<crate::realmlist::Realmlist>,
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
                    &form,
                    &realmlist,
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
                    &form,
                    &realmlist,
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
    form: &LoginForm,
    realmlist: &crate::realmlist::Realmlist,
    window: &Query<&Window, With<PrimaryWindow>>,
) {
    let font = wow_font(assets);
    // The edit boxes type in `GlueEditBoxFont`, ARIALN (GlueFonts.xml).
    let edit_font: Handle<Font> = assets.load("mpq://Fonts/ARIALN.ttf");
    let scene_image = match portraits.0.get(GLUE_SLOT) {
        Some(PortraitSource::Live(h)) => Some(h.clone()),
        _ => None,
    };
    let s = crate::glue::screen_scale(window.single().ok());
    let px = |v: f32| Val::Px(v * s);
    let empty = GlueStrings::default();
    let strings = strings.unwrap_or(&empty);

    let root = commands
        .spawn((
            LoginUi {
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
            // The fullscreen `UI_MainMenu` ModelFFX scene, window-sized: the pillarbox bars are the
            // booth camera's own clear inside the target, so the pane must cover the window.
            if let Some(image) = scene_image {
                ui.spawn((ImageNode::new(image), overlay()));
            }
        })
        .id();

    // The chrome hangs off the canvas, the boxed scene's rect, so it stays out of the bars.
    let mut canvas = commands.spawn((crate::glue::glue_canvas(), ChildOf(root)));
    canvas.with_children(|ui| {
        // The WoW logo (`AccountLoginLogo`, 256×128 at TOPLEFT (3,−7), OVERLAY).
        if let Some(logo) = &art.logo {
            ui.spawn((ImageNode::new(logo.clone()), abs(s, 3.0, 7.0, 256.0, 128.0)));
        }

        // The Blizzard logo (100×100 at BOTTOM (0,8), ARTWORK) under the `BLIZZ_DISCLAIMER`
        // line at BOTTOM (0,10), an authored overlap.
        if let Some(blizz) = &art.blizzard_logo {
            ui.spawn((Node {
                position_type: PositionType::Absolute,
                bottom: px(8.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },))
                .with_children(|c| {
                    c.spawn((
                        ImageNode::new(blizz.clone()),
                        Node {
                            width: px(100.0),
                            height: px(100.0),
                            ..default()
                        },
                    ));
                });
        }
        outlined_text(
            ui,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(10.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            (),
            (),
            GlueText {
                text: strings.text(
                    "BLIZZ_DISCLAIMER",
                    "Copyright 2004-2006  Blizzard Entertainment. All Rights Reserved.",
                ),
                size: 12.0, // GlueFontNormalSmall
                color: GOLD,
                wrap: false,
            },
            &font,
            s,
        );

        // `AccountLoginVersion` (GlueFontNormalSmall at BOTTOMLEFT (0,10)): `VERSION_TEMPLATE`
        // filled with versionType, version, internalVersion, buildType and date.
        let version = {
            let template = strings.text("VERSION_TEMPLATE", "%s %s (%s) (%s)\n%s");
            let build = benilla_protocol::CLIENT_BUILD.to_string();
            let mut out = template.to_string();
            for piece in [
                "Version",
                "1.12.1",
                build.as_str(),
                "Release",
                "Sep 19 2006",
            ] {
                out = out.replacen("%s", piece, 1);
            }
            out
        };
        outlined_text(
            ui,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: px(10.0),
                ..default()
            },
            (),
            (),
            GlueText {
                text: &version,
                size: 12.0,
                color: GOLD,
                wrap: false,
            },
            &font,
            s,
        );

        // The account box (160×37 at BOTTOM (8,345)) and password box (160×37 at BOTTOM
        // (8,270)): a 16·s left margin in a centred row makes the +8 x offset.
        for (bottom, label, action, marker_account) in [
            (
                345.0,
                strings.text("ACCOUNT_NAME", "Account Name"),
                LoginAction::FocusAccount,
                true,
            ),
            (
                270.0,
                strings.text("PASSWORD", "Account Password"),
                LoginAction::FocusPassword,
                false,
            ),
        ] {
            // The label is a 256×64 centred rect anchored BOTTOM to the box's TOP at (0,−23), so
            // it centres on the box at +8, the same left margin as the box row.
            outlined_text(
                ui,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(bottom + 37.0 - 23.0),
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    height: px(64.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    margin: UiRect::left(px(16.0)),
                    ..default()
                },
                (),
                (),
                GlueText {
                    text: label,
                    size: 15.0, // GlueFontNormal
                    color: GOLD,
                    wrap: false,
                },
                &font,
                s,
            );
            ui.spawn((Node {
                position_type: PositionType::Absolute,
                bottom: px(bottom),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },))
                .with_children(|row| {
                    row.spawn(Node {
                        margin: UiRect::left(px(16.0)),
                        ..default()
                    })
                    .with_children(|slot| {
                        if marker_account {
                            glue_edit_box(
                                slot,
                                art,
                                &edit_font,
                                (action, Button),
                                AccountText,
                                (160.0, 37.0),
                                (BOX_BORDER, BOX_FILL),
                                (15.0, 0.0, 0.0, 5.0), // AccountLogin.xml TextInsets
                                s,
                            );
                        } else {
                            glue_edit_box(
                                slot,
                                art,
                                &edit_font,
                                (action, Button),
                                PasswordText,
                                (160.0, 37.0),
                                (BOX_BORDER, BOX_FILL),
                                (15.0, 0.0, 0.0, 5.0), // AccountLogin.xml TextInsets
                                s,
                            );
                        }
                    });
                });
        }

        // Login (`GlueButtonTemplate` 170×45 at TOP (8,−519)).
        ui.spawn((Node {
            position_type: PositionType::Absolute,
            top: px(519.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },))
            .with_children(|row| {
                row.spawn(Node {
                    margin: UiRect::left(px(16.0)),
                    ..default()
                })
                .with_children(|slot| {
                    glue_button(
                        slot,
                        art,
                        &font,
                        LoginAction::Login,
                        strings.text("LOGIN", "Login"),
                        170.0,
                        45.0,
                        GlueBtnKind::Normal,
                        s,
                    );
                });
            });

        // Quit (`GlueButtonSmallTemplate` 150×38 at BOTTOMRIGHT (−5,29)).
        ui.spawn((Node {
            position_type: PositionType::Absolute,
            right: px(5.0),
            bottom: px(29.0),
            ..default()
        },))
            .with_children(|c| {
                glue_button(
                    c,
                    art,
                    &font,
                    LoginAction::Quit,
                    strings.text("QUIT", "Quit"),
                    150.0,
                    38.0,
                    GlueBtnKind::Small,
                    s,
                );
            });

        // The realmlist control, not in the reference, takes the absent TOS button's slot:
        // BOTTOM to `AccountLoginExitButton`'s TOP at (0,80), with the reference's
        // `AccountLoginRealmName` (256 wide, right-justified) at TOPRIGHT to its BOTTOMRIGHT
        // (−8,−10). Resolved: button bottom 67 + 80 = 147, right 5; readout top 631, right 13.
        //
        // The caption is a literal because the reference has no string for it; `CHANGE_REALM`
        // is character select's realm picker, a different act.
        ui.spawn((Node {
            position_type: PositionType::Absolute,
            right: px(5.0),
            bottom: px(147.0),
            ..default()
        },))
            .with_children(|c| {
                let btn = glue_button(
                    c,
                    art,
                    &font,
                    LoginAction::Realmlist,
                    "Realmlist",
                    150.0,
                    38.0,
                    GlueBtnKind::Small,
                    s,
                );
                // Disabled while `$WOW_HOST` owns the session; a click still explains why
                // ([`super::login_input`]).
                if realmlist.pinned_by_env() {
                    c.commands()
                        .entity(btn)
                        .insert(crate::glue::widgets::GlueDisabled(true));
                }
            });
        outlined_text(
            ui,
            Node {
                position_type: PositionType::Absolute,
                right: px(13.0),
                top: px(631.0),
                ..default()
            },
            (LoginAction::Realmlist, Button),
            RealmlistReadout,
            GlueText {
                text: realmlist.address(),
                size: 12.0, // GlueFontDisableSmall
                color: DISABLED_GREY,
                wrap: false,
            },
            &font,
            s,
        );

        // The Remember Account Name checkbox (20×20 at (17, top 653), resolved from its anchor
        // under the absent Community button) and its label at LEFT+24.
        ui.spawn((Node {
            position_type: PositionType::Absolute,
            left: px(17.0),
            top: px(653.0),
            height: px(20.0),
            align_items: AlignItems::Center,
            flex_direction: FlexDirection::Row,
            ..default()
        },))
            .with_children(|row| {
                let mut b = row.spawn((
                    LoginAction::ToggleSave,
                    Button,
                    Node {
                        width: px(20.0),
                        height: px(20.0),
                        ..default()
                    },
                ));
                match &art.checkbox {
                    Some(check) => {
                        b.insert((
                            ImageNode::new(check.up.clone()),
                            ArtSwap {
                                up: check.up.clone(),
                                down: check.down.clone(),
                            },
                        ));
                        b.with_children(|inner| {
                            inner.spawn((
                                CheckMark,
                                if form.save {
                                    Visibility::Inherited
                                } else {
                                    Visibility::Hidden
                                },
                                ImageNode::new(check.checked.clone()),
                                overlay(),
                            ));
                            if let Some(hi) = &check.hi {
                                inner.spawn((
                                    CheckHilight,
                                    Hilight,
                                    Visibility::Hidden,
                                    bevy::ui_render::ui_material::MaterialNode(hi.clone()),
                                    overlay(),
                                ));
                            }
                        });
                    }
                    None => {
                        b.insert(BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.15)));
                        b.with_children(|inner| {
                            inner.spawn((
                                CheckMark,
                                if form.save {
                                    Visibility::Inherited
                                } else {
                                    Visibility::Hidden
                                },
                                Text::new("x"),
                                TextFont {
                                    font: font.clone(),
                                    font_size: 14.0 * s,
                                    ..default()
                                },
                                TextColor(GOLD),
                            ));
                        });
                    }
                }
                row.spawn((
                    Text::new(strings.text("SAVE_ACCOUNT_NAME", "Remember Account Name")),
                    TextFont {
                        font: font.clone(),
                        font_size: 10.0 * s, // the authored FontHeight 10
                        ..default()
                    },
                    TextColor(GOLD),
                    TextShadow {
                        offset: Vec2::new(s, s),
                        color: Color::BLACK,
                    },
                    Node {
                        margin: UiRect::left(px(4.0)), // LEFT+24 from the checkbox's left edge
                        ..default()
                    },
                ));
            });
    });
}

/// Paint both boxes from their [`EditBoxState`]s through the shared [`paint_glue_field`].
#[allow(clippy::type_complexity)]
pub(super) fn refresh_boxes(
    form: Res<LoginForm>,
    mut account: Query<
        (&GlueFieldPart, Option<&mut Text>, &mut Visibility),
        (With<AccountText>, Without<PasswordText>),
    >,
    mut password: Query<(&GlueFieldPart, Option<&mut Text>, &mut Visibility), With<PasswordText>>,
) {
    paint_glue_field(
        &form.account,
        form.focus == Field::Account,
        account.iter_mut(),
    );
    paint_glue_field(
        &form.password,
        form.focus == Field::Password,
        password.iter_mut(),
    );
}

/// The realmlist readout, rewritten in place when the address changes
/// (`AccountLoginRealmName:SetText`); `sync_outlines` carries it to the outline copies. It
/// compares every frame, not on `Res::is_changed`, because a rebuilt tree can postdate the change.
pub(super) fn refresh_realmlist(
    realmlist: Res<crate::realmlist::Realmlist>,
    mut texts: Query<&mut Text, With<RealmlistReadout>>,
) {
    for mut t in &mut texts {
        if t.0 != realmlist.address() {
            t.0 = realmlist.address().to_string();
        }
    }
}

/// The checkbox's checked overlay and hover ring; it is not a `GlueBtn`, so the shared button
/// pass skips it.
#[allow(clippy::type_complexity)]
pub(super) fn refresh_checkbox(
    form: Res<LoginForm>,
    boxes: Query<(&Interaction, &Children), With<ArtSwap>>,
    mut marks: Query<&mut Visibility, (With<CheckMark>, Without<CheckHilight>)>,
    mut hilights: Query<&mut Visibility, With<CheckHilight>>,
) {
    for mut vis in &mut marks {
        let want = if form.save {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
    for (interaction, children) in &boxes {
        for child in children {
            if let Ok(mut vis) = hilights.get_mut(*child) {
                let want = if *interaction != Interaction::None {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
                if *vis != want {
                    *vis = want;
                }
            }
        }
    }
}

/// Leaving the login screen: drop the tree, the scene, and any open dialog.
pub(super) fn exit_login(
    mut commands: Commands,
    roots: Query<Entity, With<LoginUi>>,
    mut preview: ResMut<crate::portrait::GluePreview>,
    mut dialog: ResMut<crate::glue::dialog::GlueDialog>,
) {
    for e in &roots {
        commands.entity(e).despawn();
    }
    // The next screen sets its own scene the same frame.
    preview.scene = None;
    preview.look = None;
    if let Some(root) = dialog.root.take() {
        commands.entity(root).despawn();
    }
    dialog.close();
}

/// `WOW_LOGIN_SHOT_OUT=<path>`: one PNG of the login screen once it has settled.
pub(super) fn debug_login_shot(
    mut commands: Commands,
    state: Res<State<ClientState>>,
    time: Res<Time>,
    mut entered_at: Local<Option<f32>>,
    mut done: Local<bool>,
) {
    if *done || *state.get() != ClientState::Login {
        return;
    }
    let Ok(out) = std::env::var("WOW_LOGIN_SHOT_OUT") else {
        *done = true;
        return;
    };
    let start = *entered_at.get_or_insert(time.elapsed_secs());
    if time.elapsed_secs() - start < 8.0 {
        return;
    }
    use bevy::render::view::screenshot::{save_to_disk, Screenshot};
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(out.clone()));
    info!("login: shot instrument writing {out}");
    *done = true;
}
