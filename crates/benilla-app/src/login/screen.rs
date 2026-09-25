//! The login screen's **layout** — the reference `AccountLogin.xml` arrangement rebuilt in Bevy
//! UI, full-bleed and scaled to the window (`height / 768`, the glue engine's
//! virtual screen).
//!
//! Bottom layer: the glue booth's fullscreen render (the `UI_MainMenu` scene — the burning gate,
//! its authored fog/fires live). Over it: the WoW logo (256×128 at TOPLEFT (3,−7)), the account
//! box (160×37 at BOTTOM (8,345)) and password box (160×37 at BOTTOM (8,270)) with their
//! `GlueFontNormal` labels seated just above each (the ref's BOTTOM→TOP (0,−23) anchor on a
//! 64-tall centered rect ≈ text center 9 px above the box top), Login (`GlueButtonTemplate`
//! 170×45 at TOP (8,−519)), Quit (`GlueButtonSmallTemplate` 150×38 at BOTTOMRIGHT (−5,29)), the
//! Remember Account Name checkbox (20×20 at its resolved absolute (17, top 653) with the 10 px
//! shadowed gold label at LEFT+24), the Blizzard logo (100×100 at BOTTOM (0,8)) under the
//! `BLIZZ_DISCLAIMER` line (BOTTOM (0,10)), and the version block (BOTTOMLEFT (0,10),
//! `VERSION_TEMPLATE` filled with the 5875 build facts). The Credits/Cinematics/TOS side of the
//! reference layout is deliberately absent. The dialog is the ref's shared
//! `GlueDialog` box (512-wide `UI-DialogBox`, text wrapping at 440, one 200×40 button).

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
/// `GlueFontDisableSmall`'s color (GlueFonts.xml) — the grey the reference's own
/// `AccountLoginRealmName` readout draws in.
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
    /// Open the realmlist editor — on the button and on the address readout under
    /// it, so clicking the address you want to change does what it looks like it does.
    Realmlist,
}

/// Root of the login screen (despawned whole on exit); `with_art` mirrors the select screen's
/// artless-early-spawn upgrade latch; `s` is the glue scale the tree was baked at, so a window
/// resize (mac fullscreen) rebuilds it.
#[derive(Component)]
pub(super) struct LoginUi {
    with_art: bool,
    s: f32,
}
/// The account box's row items (segments + carets — [`refresh_boxes`] paints them through
/// [`paint_glue_field`]).
#[derive(Component, Clone)]
pub(super) struct AccountText;
/// The password box's row items (its display is the `*` mask — the box law's own `password` flag).
#[derive(Component, Clone)]
pub(super) struct PasswordText;
/// The checkbox's checked overlay (visibility = the form's save flag).
#[derive(Component)]
pub(super) struct CheckMark;
/// The checkbox's hover highlight (driven by [`refresh_checkbox`] — the checkbox isn't a
/// `GlueBtn`, so the shared button pass doesn't cover it).
#[derive(Component)]
pub(super) struct CheckHilight;
/// The realmlist readout under the button — the ref's own `AccountLoginRealmName` slot, rewritten
/// in place by [`refresh_realmlist`] when the address changes.
#[derive(Component)]
pub(super) struct RealmlistReadout;

/// Spawn the screen tree once its prerequisites exist (the select screen's boot-order pattern:
/// the INITIAL state's `OnEnter` fires before the MPQ chain / booth slots do) — and upgrade an
/// artless early spawn the moment the client art lands.
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
    // The edit boxes type in `GlueEditBoxFont` — ARIALN, not FRIZQT (GlueFonts.xml).
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
            // The 3D scene, full-bleed and first — the ref's screen IS the fullscreen ModelFFX
            // (`UI_MainMenu`); the page tint behind it is the no-art fallback. It stays on the
            // WINDOW while the chrome below does not: the pillarbox's black bars are the booth
            // camera's own output clear *inside* this window-sized target (1619 §3), so the pane
            // that samples the target covers the window and brings the bars with it.
            if let Some(image) = scene_image {
                ui.spawn((ImageNode::new(image), overlay()));
            }
        })
        .id();

    // ...and every piece of chrome hangs off the CANVAS — the boxed scene's own rect (decision
    // 2091). Anchored to the window instead, the logo, the version line and Realmlist/Quit stood
    // out in the bars at 21:9.
    let mut canvas = commands.spawn((crate::glue::glue_canvas(), ChildOf(root)));
    canvas.with_children(|ui| {
        // The WoW logo (`AccountLoginLogo`, 256×128 at TOPLEFT (3,−7), OVERLAY).
        if let Some(logo) = &art.logo {
            ui.spawn((ImageNode::new(logo.clone()), abs(s, 3.0, 7.0, 256.0, 128.0)));
        }

        // The Blizzard logo (100×100 at BOTTOM (0,8), ARTWORK) with the `BLIZZ_DISCLAIMER`
        // copyright line at BOTTOM (0,10) drawn over its lower band (the authored overlap — the
        // wordmark pixels sit above it).
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

        // The version block (`AccountLoginVersion`, GlueFontNormalSmall at BOTTOMLEFT (0,10),
        // justifyH LEFT): `VERSION_TEMPLATE` = "%s %s (%s) (%s)\n%s" filled with our wire
        // identity's frozen facts — versionType, version, internalVersion, buildType, date.
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

        // The two edit boxes + their labels. Each box is BOTTOM-anchored with the ref's +8 x
        // offset (a 16·s left margin inside a centered row shifts the center by 8·s); the label's
        // resolved seat is text-center ≈ 9 px above the box top (see the module doc).
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
            // The ref hangs the label's FontString off the EDIT BOX (`BOTTOM` ← the box's `TOP`,
            // offset (0,−23), a 256×64 rect with the text centred in it) — so it is centred on the
            // box, which sits at the screen centre +8. Centring a full-width node with a left
            // margin instead put it at +16: eight units right of the box it labels.
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

        // **The realmlist control** — benilla's, in the reference's coordinates.
        //
        // The bottom-right column of `AccountLogin.xml` is a stack anchored off the Quit button:
        // TOS sits BOTTOM ← `AccountLoginExitButton`'s TOP at (0, 80), with Credits and Cinematics
        // above it. Decision 0539 cut all three, so the slot is empty — and the reference hangs
        // `AccountLoginRealmName` off exactly that button (a 256-wide right-justified
        // `GlueFontDisableSmall` at TOPRIGHT ← its BOTTOMRIGHT, (−8, −10)), filled in
        // `AccountLogin_OnShow` from `GetServerName()`. So the authored layout already reserves a
        // button here with a server readout beneath it; we put ours in that slot rather than
        // inventing a spot.
        //
        // Resolved: Quit is BOTTOMRIGHT (−5, 29) at 150×38, so its top edge is bottom 67 and the
        // button lands at bottom 67 + 80 = 147, sharing Quit's right offset of 5. The readout's
        // top is 10 below the button's bottom (bottom 137 → top 768 − 137 = 631) and its right
        // edge is 8 further in (5 + 8 = 13).
        //
        // The caption is a bare literal, deliberately: the reference has no string for this,
        // because it has no such control. `CHANGE_REALM` was the near miss and is the wrong
        // word — it is the *character select* screen's button for picking another realm out of a
        // list already fetched, which is a different act from repointing the client at a different
        // logon server. "Realmlist" is what the file, the CVar and every private server's setup
        // page call it, in every locale.
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
                // `$WOW_HOST` owns the session, so the button reads disabled — the ref's own
                // `Enable()`/`Disable()` split, rendered by `glue_button_visuals`. Clicking it
                // still explains itself ([`super::login_input`]) rather than doing nothing.
                if realmlist.pinned_by_env() {
                    c.commands()
                        .entity(btn)
                        .insert(crate::glue::widgets::GlueDisabled(true));
                }
            });
        // `AccountLoginRealmName`'s seat, right-justified by being anchored on its right edge.
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
                size: 12.0, // GlueFontDisableSmall = GlueFontNormalSmall's 12, in grey
                color: DISABLED_GREY,
                wrap: false,
            },
            &font,
            s,
        );

        // The Remember Account Name checkbox (20×20 at the resolved absolute (17, top 653) —
        // the ref anchors it under the Community button we cut; the spot is the same) + its
        // 10 px shadowed gold label at LEFT+24.
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

/// Paint both boxes from their [`EditBoxState`]s — segments, selection highlight, and the caret at
/// the cursor — through the shared [`paint_glue_field`].
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

/// The realmlist readout, rewritten in place when the address changes (the ref's own
/// `AccountLoginRealmName:SetText`). `sync_outlines` carries the write into the eight outline
/// copies, so the whole nine-string stack follows.
///
/// An unconditional compare rather than a `Res::is_changed` gate: it is one string comparison
/// against one entity, and the gate would silently depend on the screen tree never being spawned
/// **after** the last change to the resource — which is exactly what a rebuild on a window resize
/// does. Cheap beats subtly order-dependent.
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

/// The checkbox's visuals: the checked overlay tracks the form's save flag; the ADD hover ring
/// tracks the button's interaction (the checkbox isn't a `GlueBtn`, so the shared pass skips it).
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
    // The next screen re-establishes its own scene the same frame (select's per-frame feed).
    preview.scene = None;
    preview.look = None;
    if let Some(root) = dialog.root.take() {
        commands.entity(root).despawn();
    }
    dialog.close();
}

/// The login-screen shot instrument (`WOW_LOGIN_SHOT_OUT=<path>`): once the
/// screen has been up a few seconds (art + scene settled), write one PNG via Bevy's framebuffer
/// readback. Inert without the env.
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
