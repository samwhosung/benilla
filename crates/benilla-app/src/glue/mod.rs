//! What the pre-world glue screens share: the art set, the widget builders, the ADD-mode material,
//! the one dialog and the screen-agnostic interaction systems. Each screen keeps its own layout,
//! actions and selection.

pub(crate) mod add_material;
pub(crate) mod art;
pub(crate) mod backdrop;
pub(crate) mod dialog;
pub(crate) mod widgets;

use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;

use art::{tc_rect, GlueArt, BTN_BG, BTN_HOVER, BUTTON_TC, GOLD};
use widgets::{ArtSwap, GlueBtn, GlueCaption, GlueDisabled, OutlineCopy};

/// The glue widgets' look passes, registered once for every glue screen.
///
/// A screen orders its own refresh `.before(GlueVisuals)` so the flags and captions it writes
/// render the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GlueVisuals;

/// The shared glue infrastructure: the ADD material pipeline, [`GlueArt`], the GlueStrings table
/// and the one [`dialog::GlueDialog`]. Registered before any screen plugin.
pub(crate) struct GluePlugin;

impl Plugin for GluePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy::ui_render::UiMaterialPlugin::<
            add_material::AddUiMaterial,
        >::default())
            .init_resource::<GlueArt>()
            .add_systems(
                Startup,
                crate::glue_strings::load_glue_strings.after(benilla_assets::AssetSet::Open),
            )
            .init_resource::<GlueClicks>()
            .init_resource::<dialog::GlueDialog>()
            .add_message::<dialog::GlueDialogAnswer>()
            .add_systems(PreUpdate, glue_clicks.after(bevy::ui::UiSystems::Focus))
            .add_systems(
                Update,
                (backdrop::fit_backdrop_borders, seat_outline_copies),
            )
            // Fitted before layout, so a screen spawned this frame never lays out against the
            // window first.
            .add_systems(
                PostUpdate,
                fit_glue_canvas.before(bevy::ui::UiSystems::Layout),
            )
            .add_systems(
                Update,
                (art_swaps, glue_button_visuals, glue_hilights, sync_outlines).in_set(GlueVisuals),
            );
    }
}

/// The glue widgets whose click landed this frame, the reference's `OnClick`. Ask this, never
/// `Interaction::Pressed`: pressing is not clicking. Rebuilt every frame in `PreUpdate`.
#[derive(Resource, Default)]
pub(crate) struct GlueClicks(EntityHashSet);

impl GlueClicks {
    pub(crate) fn hit(&self, widget: Entity) -> bool {
        self.0.contains(&widget)
    }
}

/// A glue button fires on the release, only when it lands back on the button that took the press.
///
/// The reference's `CSimpleButton` click mask defaults to `0x100`, `LeftButtonUp` alone
/// (`0x7786d0`), dispatched from the mouse-up handler `0x7792d0` (the mouse-down one, `0x779210`,
/// fires nothing a glue screen registers); the button must be pushed
/// (`[+0x328] == 2`) and the release must hit-test inside the frame (`0x76b020`). In Bevy's
/// [`Interaction`], `Pressed → Hovered` is released inside and `Pressed → None` released outside
/// or hidden mid-press; `pushed` is the reference's state byte.
pub(crate) fn glue_clicks(
    interactions: Query<(Entity, &Interaction)>,
    mut pushed: Local<EntityHashSet>,
    mut clicks: ResMut<GlueClicks>,
) {
    clicks.0.clear();
    pushed.retain(|&e| match interactions.get(e) {
        Ok((_, Interaction::Pressed)) => true, // still held
        Ok((_, Interaction::Hovered)) => {
            clicks.0.insert(e); // released inside: the click
            false
        }
        // Released outside, hidden mid-press, or despawned: the press is dropped.
        _ => false,
    });
    for (e, interaction) in &interactions {
        if *interaction == Interaction::Pressed {
            pushed.insert(e);
        }
    }
}

/// Seat every outline copy one device pixel from its real string, the reference's baked 1 px
/// outline ring; writes only on a change (a spawn, or a display with another scale factor).
pub(crate) fn seat_outline_copies(
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut copies: Query<(&OutlineCopy, &mut Node)>,
) {
    let px = 1.0 / window.single().map(|w| w.scale_factor()).unwrap_or(1.0);
    for (copy, mut node) in &mut copies {
        let (left, top) = (Val::Px(copy.dir.x * px), Val::Px(copy.dir.y * px));
        if node.left != left || node.top != top {
            node.left = left;
            node.top = top;
        }
    }
}

/// The preview drag rate, `CHARACTER_ROTATION_CONSTANT` (CharacterSelect.lua): 0.6 degrees per UI
/// unit, dragging right increasing the facing. Apply it through [`drag_yaw`], never per pixel.
pub(crate) const ROTATION_PER_UI_UNIT: f32 = 0.6 * std::f32::consts::PI / 180.0;

/// The yaw a horizontal cursor move of `delta_px` logical pixels turns the preview by.
///
/// `GetCursorPosition` (`0x46dad0`) answers on the `aspect·768 × 768` virtual canvas, so a drag
/// turns less on a taller window. The divisor is the true `height / 768`, not [`screen_scale`],
/// whose 2.2 cap the cursor's canvas does not have.
pub(crate) fn drag_yaw(delta_px: f32, window: Option<&Window>) -> f32 {
    let ui_per_px = 768.0 / window.map_or(768.0, |w| w.height().max(1.0));
    delta_px * ui_per_px * ROTATION_PER_UI_UNIT
}

/// The rotate buttons' hold rate: the reference's `CHARACTER_FACING_INCREMENT = 2` degrees per
/// `OnUpdate` tick, taken per second at 60 fps, so the turn speed does not follow the frame rate.
pub(crate) const ROTATE_RATE: f32 = 120.0 * std::f32::consts::PI / 180.0;

/// The glue virtual-screen scale: the reference lays glue screens out 768 units tall, scaled by
/// the window height, so an authored coordinate draws at `value · this`. Every spawn site and
/// rescale check reads it here.
///
/// No lower clamp: a floor would push the bottom controls off a short window. Capped at 2.2 on a
/// tall window; whether the reference caps its glue scale is untraced.
pub(crate) fn screen_scale(window: Option<&Window>) -> f32 {
    window.map(|w| (w.height() / 768.0).min(2.2)).unwrap_or(1.0)
}

/// The rect a glue screen's chrome lays out into: the pillarboxed scene's box, not the window, as
/// a reference client of the box's aspect would lay it out.
///
/// The screen's root stays full-window: the bars are the booth camera's clear inside a
/// window-sized target, which the full-bleed scene pane must keep covering.
#[derive(Component)]
pub(crate) struct GlueCanvas;

/// The canvas node: full height, sized by its insets, which [`fit_glue_canvas`] keeps current.
/// Spawn it after the screen's full-bleed scene pane so the chrome draws over the scene.
pub(crate) fn glue_canvas() -> (GlueCanvas, FocusPolicy, Node) {
    (
        GlueCanvas,
        // `Pass` is load-bearing: `ui_focus_system` treats no `FocusPolicy` as `Block`, and the
        // canvas would swallow the scene pane's drag-to-rotate.
        FocusPolicy::Pass,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            top: Val::Px(0.0),
            bottom: Val::Px(0.0),
            ..default()
        },
    )
}

/// Seat every [`GlueCanvas`] on the boxed scene's rect, writing only when the bars change.
///
/// Runs in `PostUpdate` before `UiSystems::Layout`, so a canvas spawned in `Update` is fitted
/// before its first layout.
pub(crate) fn fit_glue_canvas(
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut canvases: Query<&mut Node, With<GlueCanvas>>,
) {
    if canvases.is_empty() {
        return;
    }
    // Off the window, not `CreateScene`: the box is one aspect for every scene, so the canvas
    // never waits on the booth.
    let window = window.single().ok();
    let (left, right) = crate::portrait::glue_canvas_bars(
        window,
        window.and_then(|w| crate::portrait::glue_box_aspect(w.width() / w.height().max(1.0))),
    );
    let (left, right) = (Val::Px(left), Val::Px(right));
    for mut node in &mut canvases {
        if node.left != left || node.right != right {
            node.left = left;
            node.right = right;
        }
    }
}

/// Mirror every outlined text into its black `OutlineCopy` siblings; refresh systems write only
/// the real text.
#[allow(clippy::type_complexity)]
pub(crate) fn sync_outlines(
    changed: Query<(&Text, &ChildOf), (Changed<Text>, Without<OutlineCopy>)>,
    children: Query<&Children>,
    mut copies: Query<&mut Text, With<OutlineCopy>>,
) {
    for (text, parent) in &changed {
        let Ok(siblings) = children.get(parent.parent()) else {
            continue;
        };
        for &sib in siblings {
            if let Ok(mut copy) = copies.get_mut(sib) {
                if copy.0 != text.0 {
                    copy.0 = text.0.clone();
                }
            }
        }
    }
}

/// Two-state button faces (spinner arrows, rotate buttons): pressed shows the down art.
pub(crate) fn art_swaps(
    mut swaps: Query<(&ArtSwap, &Interaction, &mut ImageNode), Without<GlueBtn>>,
) {
    for (swap, interaction, mut node) in &mut swaps {
        let img = if *interaction == Interaction::Pressed {
            &swap.down
        } else {
            &swap.up
        };
        if node.image != *img {
            node.image = img.clone();
        }
    }
}

/// The glue-panel buttons' look: up, down and disabled art, and the caption whitening on hover
/// (`HighlightFont`). A screen disables with [`GlueDisabled`]; this pass only renders it.
#[allow(clippy::type_complexity)]
pub(crate) fn glue_button_visuals(
    art: Res<GlueArt>,
    mut glue_btns: Query<
        (
            Entity,
            &GlueDisabled,
            &Interaction,
            &mut ImageNode,
            &mut BackgroundColor,
            Has<widgets::FallbackFace>,
            Option<&widgets::BtnTexCoords>,
        ),
        (With<GlueBtn>, Without<ArtSwap>),
    >,
    children: Query<&Children>,
    mut captions: Query<&mut TextColor, With<GlueCaption>>,
) {
    for (btn, disabled, interaction, mut node, mut bg, fallback, tc) in &mut glue_btns {
        let disabled = disabled.0;
        let pressed = !disabled && *interaction == Interaction::Pressed;
        let hovered = !disabled && *interaction != Interaction::None;
        if let Some((up, size)) = &art.button_up {
            let (image, color) = if disabled {
                match &art.button_dis {
                    Some((dis, _)) => (dis.clone(), Color::WHITE),
                    None => (up.clone(), Color::srgb(0.45, 0.45, 0.45)),
                }
            } else if pressed {
                (
                    art.button_down
                        .as_ref()
                        .map(|(d, _)| d.clone())
                        .unwrap_or_else(|| up.clone()),
                    Color::WHITE,
                )
            } else {
                (up.clone(), Color::WHITE)
            };
            node.image = image;
            node.rect = Some(tc_rect(*size, tc.map(|t| t.0).unwrap_or(BUTTON_TC)));
            node.color = color;
        } else if fallback {
            bg.0 = if hovered { BTN_HOVER } else { BTN_BG };
        }
        // Descendants, not children: `outlined_text` nests the `GlueCaption` under a wrapper and
        // a trim node.
        for child in children.iter_descendants(btn) {
            if let Ok(mut caption) = captions.get_mut(child) {
                // The disabled caption grays (`GlueFontDisable`), hover whitens, rest is gold.
                caption.0 = if disabled {
                    Color::srgb(0.5, 0.5, 0.5)
                } else if hovered {
                    Color::WHITE
                } else {
                    GOLD
                };
            }
        }
    }
}

/// What [`glue_hilights`] reads off a button: hover, disabled, and held lit.
type SheenSource = (
    &'static Interaction,
    Option<&'static GlueDisabled>,
    Option<&'static widgets::LockHighlight>,
);

/// Every button's highlight sheen: a [`widgets::Hilight`] lights while its nearest enclosing
/// [`Button`] is hovered or held by [`widgets::LockHighlight`], and never while disabled.
///
/// It walks up from each sheen, not down from each button: a modal dialog's root is itself a
/// `Button`, so a walk down would light every sheen in the dialog.
pub(crate) fn glue_hilights(
    mut hilights: Query<(Entity, &mut Visibility), With<widgets::Hilight>>,
    parents: Query<&ChildOf>,
    buttons: Query<SheenSource, With<Button>>,
) {
    for (sheen, mut vis) in &mut hilights {
        let lit = parents
            .iter_ancestors(sheen)
            .find_map(|up| buttons.get(up).ok())
            .is_some_and(|(interaction, disabled, locked)| {
                let live = !disabled.is_some_and(|d| d.0);
                live && (*interaction != Interaction::None || locked.is_some_and(|l| l.0))
            });
        let want = if lit {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::widgets::{
        FallbackFace, GlueBtn, GlueCaption, GlueDisabled, Hilight, LockHighlight,
    };
    use super::{
        glue_button_visuals, glue_canvas, glue_clicks, glue_hilights, screen_scale, FocusPolicy,
        GlueArt, GlueClicks, GOLD,
    };
    use bevy::prelude::*;
    use bevy::window::WindowResolution;

    /// Set one widget's [`Interaction`] as `ui_focus_system` would and read whether a click landed.
    fn click_run(app: &mut App, widget: Entity, interaction: Interaction) -> bool {
        *app.world_mut().get_mut::<Interaction>(widget).unwrap() = interaction;
        app.update();
        app.world().resource::<GlueClicks>().hit(widget)
    }

    fn click_app() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<GlueClicks>()
            .add_systems(Update, glue_clicks);
        let widget = app.world_mut().spawn(Interaction::None).id();
        (app, widget)
    }

    #[test]
    fn a_held_button_has_not_clicked_yet() {
        let (mut app, widget) = click_app();
        assert!(
            !click_run(&mut app, widget, Interaction::Hovered),
            "hover alone is not a click"
        );
        assert!(
            !click_run(&mut app, widget, Interaction::Pressed),
            "the press is not the click"
        );
        assert!(
            !click_run(&mut app, widget, Interaction::Pressed),
            "and holding it does not become one"
        );
    }

    #[test]
    fn the_release_over_the_button_is_the_click() {
        let (mut app, widget) = click_app();
        click_run(&mut app, widget, Interaction::Pressed);
        assert!(
            click_run(&mut app, widget, Interaction::Hovered),
            "released inside → the click"
        );
        assert!(
            !click_run(&mut app, widget, Interaction::Hovered),
            "and it does not repeat while the cursor rests there"
        );
    }

    #[test]
    fn sliding_off_before_the_release_cancels() {
        let (mut app, widget) = click_app();
        click_run(&mut app, widget, Interaction::Pressed);
        click_run(&mut app, widget, Interaction::Pressed); // dragged off, still held
        assert!(
            !click_run(&mut app, widget, Interaction::None),
            "released outside → nothing"
        );
        assert!(
            !click_run(&mut app, widget, Interaction::Hovered),
            "and coming back afterwards is not a click either"
        );
    }

    #[test]
    fn a_release_without_a_press_is_not_a_click() {
        let (mut app, widget) = click_app();
        assert!(
            !click_run(&mut app, widget, Interaction::Hovered),
            "no press ever landed"
        );
    }

    #[test]
    fn the_caption_whitens_on_hover_however_deep_the_outline_nests_it() {
        let mut app = App::new();
        app.init_resource::<GlueArt>()
            .add_systems(Update, glue_button_visuals);
        let mut caption = Entity::PLACEHOLDER;
        let button = app
            .world_mut()
            .spawn((
                GlueBtn,
                GlueDisabled(false),
                FallbackFace,
                Interaction::None,
                ImageNode::default(),
                BackgroundColor::DEFAULT,
            ))
            .with_children(|btn| {
                // wrapper → trim → the real string, as `outlined_text` builds it.
                btn.spawn(Node::default()).with_children(|wrapper| {
                    wrapper.spawn(Node::default()).with_children(|trim| {
                        caption = trim.spawn((GlueCaption, TextColor(GOLD))).id();
                    });
                });
            })
            .id();

        let colour = |app: &App| app.world().get::<TextColor>(caption).unwrap().0;
        app.update();
        assert_eq!(colour(&app), GOLD, "at rest the caption is gold");

        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Hovered;
        app.update();
        assert_eq!(
            colour(&app),
            Color::WHITE,
            "hover whitens it (the ref's HighlightFont)"
        );

        app.world_mut().get_mut::<GlueDisabled>(button).unwrap().0 = true;
        app.update();
        assert_ne!(
            colour(&app),
            Color::WHITE,
            "a disabled button does not highlight"
        );
        assert_ne!(colour(&app), GOLD, "it grays (the ref's GlueFontDisable)");
    }

    #[test]
    fn the_chrome_canvas_never_swallows_the_scenes_drags() {
        let (_, policy, node) = glue_canvas();
        assert_eq!(policy, FocusPolicy::Pass);
        // Sized by its four insets, so `fit_glue_canvas` moves it by writing two of them.
        assert_eq!(node.position_type, PositionType::Absolute);
        assert_eq!((node.width, node.height), (Val::Auto, Val::Auto));
        for inset in [node.left, node.right, node.top, node.bottom] {
            assert_eq!(inset, Val::Px(0.0));
        }
    }

    #[test]
    fn the_authored_layout_fits_any_window_height() {
        /// The RANDOMIZE button's bottom edge: the tower's TOPLEFT y 74, its 645 offset, 30 tall.
        const LOWEST_CONTROL: f32 = 74.0 + 645.0 + 30.0;
        for h in [480u32, 600, 677, 720, 768, 900, 1286, 2160] {
            let window = Window {
                resolution: WindowResolution::new(1600, h),
                ..default()
            };
            let s = screen_scale(Some(&window));
            assert!(
                LOWEST_CONTROL * s <= window.height(),
                "at a {h}px window (scale {s}) the lowest glue control lands at {} — off the bottom",
                LOWEST_CONTROL * s
            );
        }
    }

    // ── The sheen ────────────────────────────────────────────────────────────────────────────

    /// Spawn `button > wrapper > sheen`, the grandchild shape the overlays make.
    fn sheen_app(disabled: bool, locked: bool) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_systems(Update, glue_hilights);
        let sheen = app.world_mut().spawn((Hilight, Visibility::Hidden)).id();
        let wrapper = app.world_mut().spawn(Node::default()).add_child(sheen).id();
        let button = app
            .world_mut()
            .spawn((
                Button,
                Interaction::None,
                GlueDisabled(disabled),
                LockHighlight(locked),
            ))
            .add_child(wrapper)
            .id();
        (app, button, sheen)
    }

    fn sheen_lit(app: &App, sheen: Entity) -> bool {
        *app.world().get::<Visibility>(sheen).unwrap() == Visibility::Inherited
    }

    #[test]
    fn a_sheen_follows_its_nearest_button() {
        let (mut app, button, sheen) = sheen_app(false, false);
        app.update();
        assert!(!sheen_lit(&app, sheen), "at rest, dark");

        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Hovered;
        app.update();
        assert!(sheen_lit(&app, sheen), "hovered, lit");

        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::None;
        app.update();
        assert!(
            !sheen_lit(&app, sheen),
            "and dark again when the cursor leaves"
        );
    }

    #[test]
    fn a_locked_sheen_stays_lit_unhovered() {
        let (mut app, _, sheen) = sheen_app(false, true);
        app.update();
        assert!(sheen_lit(&app, sheen));
    }

    /// The reference's `Disable()` takes the highlight with it.
    #[test]
    fn a_disabled_button_never_lights() {
        let (mut app, button, sheen) = sheen_app(true, true);
        app.update();
        assert!(!sheen_lit(&app, sheen), "locked but disabled: dark");
        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Hovered;
        app.update();
        assert!(!sheen_lit(&app, sheen), "and hovering does not revive it");
    }

    #[test]
    fn an_outer_modal_button_does_not_light_the_sheens_inside_it() {
        let (mut app, button, sheen) = sheen_app(false, false);
        let root = app
            .world_mut()
            .spawn((Button, Interaction::Hovered))
            .add_child(button)
            .id();
        app.update();
        assert!(
            !sheen_lit(&app, sheen),
            "the dim is hovered, the inner button is not"
        );
        let _ = root;
    }
}
