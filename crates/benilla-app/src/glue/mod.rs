//! The shared glue-screen vocabulary — everything the pre-world screens (character
//! select + character create) draw with, factored out of the create screen when select joined it:
//! the client-data art set ([`art::GlueArt`]), the widget builders ([`widgets`]), the ADD-mode UI
//! material ([`add_material`]), and the screen-agnostic interaction systems below. Each screen
//! keeps its own layout, actions, and selection policy; what lives here is exactly what the two
//! share — so the vocabulary can never fork.

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

/// **The glue widgets' look, run once for every glue screen.**
///
/// These four passes are screen-agnostic by construction — they find their work by component, not
/// by state — and every screen registering its own copy was a standing invitation to forget one.
/// The realm list did exactly that: it shipped with `sync_outlines` and without
/// [`glue_button_visuals`]/[`art_swaps`], so no button on it lit on hover, none took its pressed
/// art, and the Okay it carefully marked [`GlueDisabled`] never greyed. Registered here, in
/// [`GluePlugin`], a new screen gets them by existing.
///
/// A screen orders its own refresh **before** this set (`.before(GlueVisuals)`) so the disabled
/// flags and captions it writes are rendered the same frame it writes them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GlueVisuals;

/// The shared glue infrastructure both screens stand on: the ADD-mode UI material pipeline, the
/// [`GlueArt`] resource (loaded on first screen entry), the GlueStrings table, and the one
/// [`dialog::GlueDialog`] every glue screen raises. Registered before either screen plugin
/// (`main.rs`) — which is also why the dialog's resource and message live here rather than in a
/// screen: they outlive any single screen's plugin.
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
            // The chrome's canvas is the boxed scene (2091) — fitted before the layout that reads
            // it, so a screen spawned this frame never lays out against the window first.
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

/// The glue widgets whose click landed **this frame** — the reference's `OnClick`.
///
/// Ask this, never `Interaction::Pressed`, to run a button's action: pressing is not clicking (see
/// [`glue_clicks`]). Rebuilt every frame in `PreUpdate`, so an `Update` reader sees exactly the
/// current frame's clicks and a screen that did not run misses nothing it should have had.
#[derive(Resource, Default)]
pub(crate) struct GlueClicks(EntityHashSet);

impl GlueClicks {
    /// Did this widget's click land this frame?
    pub(crate) fn hit(&self, widget: Entity) -> bool {
        self.0.contains(&widget)
    }
}

/// The glue layer's click law: a button fires on the **release**, and only when the release lands
/// back on the button that took the press.
///
/// In the real 1.12.1 client, `CSimpleButton`'s ctor default click mask `[+0x330] = 0x100`
/// (`0x7786d0`) is **`LeftButtonUp` alone**, so a stock `<Button>` — every glue button is one — is
/// dispatched from the mouse-**up** dispatcher `0x7792d0`; the mouse-**down** dispatcher
/// `0x779210` fires nothing a glue screen registers. Two more predicates ride the release: the
/// button must be in state `[+0x328] == 2`
/// (PUSHED — it took the press), and the release must **hit-test inside the frame** (`0x76b020`).
/// So press-and-slide-off cancels, and press-off-slide-on does nothing — which is what a player
/// expects of every button they have ever used. Our own in-game FrameXML path already implements
/// exactly this (`benilla_ui::script::pointer`'s `same_frame` gate); the glue screens were the half
/// that dispatched on the press edge, and every one of them moved here (1533).
///
/// Bevy's [`Interaction`] carries the same three states under other names, so the up edge is a
/// state transition and needs no cursor maths: `ui_focus_system` sets `Pressed` only on the frame
/// the press lands on a hovered node, holds it through a drag that leaves the node, and on release
/// clears every `Pressed` back to `None` — after which the same run re-raises `Hovered` on whatever
/// still contains the cursor. **`Pressed → Hovered` is therefore exactly "released inside"**, and
/// `Pressed → None` exactly "released outside, or hidden mid-press". The `pushed` set is the ref's
/// state byte.
pub(crate) fn glue_clicks(
    interactions: Query<(Entity, &Interaction)>,
    mut pushed: Local<EntityHashSet>,
    mut clicks: ResMut<GlueClicks>,
) {
    clicks.0.clear();
    pushed.retain(|&e| match interactions.get(e) {
        Ok((_, Interaction::Pressed)) => true, // still held
        Ok((_, Interaction::Hovered)) => {
            clicks.0.insert(e); // released inside — the click
            false
        }
        // Released outside, hidden mid-press, or despawned: the press is simply dropped, as the
        // ref drops one whose release fails the hit test.
        _ => false,
    });
    for (e, interaction) in &interactions {
        if *interaction == Interaction::Pressed {
            pushed.insert(e);
        }
    }
}

/// Seat every outline copy at exactly ONE device pixel from its real string (`dir / scale_factor`
/// logical) — the reference's baked 1-px dilation ring, reproduced as string geometry (see
/// [`widgets::OutlineCopy`]). Runs every frame but writes only when the wanted value differs
/// (fresh spawns, or the window moving to a display with another scale factor).
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

/// The character-preview **drag rate**: the ref's `CHARACTER_ROTATION_CONSTANT = 0.6` degrees per
/// **UI unit** (declared once in `CharacterSelect.lua` and read by both screens' `OnUpdate`; the
/// facing setter takes degrees, deg→rad at the C boundary). Dragging right increases the facing.
///
/// It lives here for the reason this module exists: it is one number the two screens share, and it
/// had already forked — the create screen carried a bare `0.01` rad/px, 4.5 % off its neighbour and
/// off its own comment (1533). Apply it through [`drag_yaw`], never to a raw pixel delta.
pub(crate) const ROTATION_PER_UI_UNIT: f32 = 0.6 * std::f32::consts::PI / 180.0;

/// The yaw a horizontal cursor move of `delta_px` **logical** pixels turns the preview by.
///
/// `CHARACTER_ROTATION_CONSTANT` is per UI unit, not per pixel: `GetCursorPosition` (`0x46dad0`)
/// answers on the glue engine's `aspect·768 × 768` virtual canvas, so the same physical drag turns
/// the character *less* on a taller display — 0.6°/px only at a 768-line window, 0.427°/px at 1080p
/// (1533). Feeding it the raw pixel delta ran the drag ~40 % fast at 1080p and made it scale with
/// the window instead of with the canvas.
///
/// The divisor is the true `height / 768`, not [`screen_scale`]: that one is clamped at 2.2 so the
/// *layout* stops growing on a very tall window, and the cursor's canvas has no such clamp.
pub(crate) fn drag_yaw(delta_px: f32, window: Option<&Window>) -> f32 {
    let ui_per_px = 768.0 / window.map_or(768.0, |w| w.height().max(1.0));
    delta_px * ui_per_px * ROTATION_PER_UI_UNIT
}

/// The rotate buttons' **hold rate** — the ref's `CHARACTER_FACING_INCREMENT = 2`, applied per
/// `OnUpdate` tick by both screens' `RotateLeft/Right_OnUpdate`. Ours is per *second* at the ref's
/// 60 fps, so the arrow turns at one speed instead of at the frame rate (`docs/METHOD.md` step 3: the
/// mechanism, not the quirk).
pub(crate) const ROTATE_RATE: f32 = 120.0 * std::f32::consts::PI / 180.0;

/// The glue engine's virtual-screen scale: the ref lays every glue screen out on a 1024×768
/// virtual screen stretched to the window, so each authored coordinate draws at `value · this`.
/// The ONE copy of the formula — every glue spawn site and every rescale check reads it here, and
/// each screen's lifecycle system compares its tree's baked scale against this to know when a
/// window resize (mac fullscreen, a drag) has invalidated the tree.
///
/// **There is no lower clamp**. A floor of 1.0 draws the 768-unit-tall authored layout into a
/// shorter window and the overflow simply falls off the bottom — silently, and always the
/// *bottom-most* controls: on the create screen that is the last customization row and the
/// **RANDOMIZE** button (reproduced at `WOW_WIN=1276x677` — both gone, along with the foot of the
/// right-hand race/class panels). The reference has no such floor: its glue screens are authored in
/// the 768-tall virtual space and scaled by the window height, so a short window makes everything
/// smaller and nothing missing. The upper clamp stays — that is the shipped size on a tall screen,
/// and lifting it would resize the director's UI without being asked.
pub(crate) fn screen_scale(window: Option<&Window>) -> f32 {
    window.map(|w| (w.height() / 768.0).min(2.2)).unwrap_or(1.0)
}

/// **The rect a glue screen's chrome lays out into: the boxed scene, never the window** (decision
/// 2091).
///
/// [`screen_scale`] answers *how big* an authored coordinate draws; this answers *what it is
/// measured from*. The two are not the same question once the scene is pillarboxed (decision
/// 1619): past the aspect its art can fill, the booth camera renders into a centred box with black
/// bars either side, and a chrome node anchored `right: 0` against the *window* lands out in the
/// bar — the logo, the version line, Realmlist/Quit, the character list and Delete/Back all did, at
/// 21:9 (3440×1440). 1619 wrote that residue down and left it; this is the
/// answer. Anchor the chrome to the **box** — the window's height, the box's width — and the glue
/// canvas is exactly what a reference client of the box's own aspect would lay out on, which is
/// what a 1.12 client on a 21:9 monitor looks like: a 16:9 client, centred by the monitor,
/// everything inside it.
///
/// The screen's root stays full-window: the black bars are the booth camera's own *output* clear
/// inside a window-sized render target (1619 §3), so the full-bleed scene pane that samples that
/// target must keep covering the window. Only the chrome moves in.
#[derive(Component)]
pub(crate) struct GlueCanvas;

/// The canvas node itself — full height, inset to the pillarbox's bars, sized by its own insets so
/// it recentres itself. Spawn one under a screen's root and hang the chrome off it; the insets are
/// kept current by [`fit_glue_canvas`] (which also fills them in on the spawn frame, before the
/// first layout runs).
///
/// Spawn it as a sibling **after** the screen's full-bleed scene pane, so the chrome keeps drawing
/// over the scene exactly as it did when it hung off the root.
pub(crate) fn glue_canvas() -> (GlueCanvas, FocusPolicy, Node) {
    (
        GlueCanvas,
        // **`Pass`, and it is load-bearing.** `ui_focus_system` treats a hovered node with no
        // `FocusPolicy` as `Block` (`focus_policy.unwrap_or(&FocusPolicy::Block)`) and stops the
        // walk there — so a canvas spanning the whole box would swallow every hover and press
        // before they reached the full-bleed scene pane beneath it, and the select and create
        // screens' drag-to-rotate would simply stop working. The canvas is a coordinate frame, not
        // a surface: it passes everything its own chrome does not take.
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

/// Seat every [`GlueCanvas`] on the boxed scene's rect, every frame the bars change.
///
/// In `PostUpdate` **before** `UiSystems::Layout`, which is what keeps a screen spawned this frame
/// from laying out once against the window and snapping in the next: the tree's commands have been
/// applied by then, so a canvas born in `Update` is fitted before it is ever laid out. Writing
/// only on change keeps it off Bevy's `Changed<Node>` path on the ~every frame nothing moves.
pub(crate) fn fit_glue_canvas(
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut canvases: Query<&mut Node, With<GlueCanvas>>,
) {
    if canvases.is_empty() {
        return;
    }
    // Off the **window**, not off `CreateScene`: the frame is one aspect for every
    // scene now, so the canvas has no reason to wait on the booth — which is what used to move it
    // when a stage swap cleared the box for a frame, and what made a screen spawned before its
    // scene lay out against the window once and snap in.
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

/// Mirror every outlined text's content into its black copies ([`widgets::outlined_text`]) — the
/// refresh systems write only the real text entity; the copies are its `OutlineCopy` siblings.
/// Registered by each glue screen under its own state (the trees only exist there).
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

/// Two-state button faces (spinner arrows, rotate buttons): pressed swaps to the down art.
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

/// The glue-panel buttons' look: up/down/disabled art states + the caption whitening on hover (the
/// ref's `HighlightFont`). Disabling is the screen's call — it toggles [`GlueDisabled`]; this pass
/// only renders it (the ref's `Enable()`/`Disable()` split the same way).
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
        // DESCENDANTS, not children. The caption is a **grandchild's** child: `outlined_text`
        // wraps every glue string in a layout wrapper + the −1px trim node before the real string,
        // so a direct-children scan finds the wrapper and never the `GlueCaption` — which is why a
        // disabled button's caption never grayed (1533). The walk is over a ~11-entity subtree;
        // depth is the primitive's business, not this pass's.
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

/// What [`glue_hilights`] needs off a button to decide whether its sheen is lit: is the cursor on
/// it, is it disabled, and is the screen holding it.
type SheenSource = (
    &'static Interaction,
    Option<&'static GlueDisabled>,
    Option<&'static widgets::LockHighlight>,
);

/// **Every button's highlight sheen, in one pass.** A [`widgets::Hilight`] lights while its owning
/// [`Button`] is hovered — or while the screen holds it with [`widgets::LockHighlight`], the
/// reference's own verb for a selected row.
///
/// There were **four** of these before, one per screen, and the realm list made five by having
/// none: no button on it lit at all. Worse, the four had drifted — two folded selection into the
/// same expression as hover, one folded in a scroll-position disable, and the one that lived
/// inside [`glue_button_visuals`] was scoped `With<GlueBtn>, Without<ArtSwap>`, so the
/// `GlueCloseButton` X on every panel in the client had a sheen that could never light. Splitting
/// "is it lit?" (here) from "is it chosen?" (the screen's, as a `LockHighlight` flag) is what
/// makes one owner possible.
///
/// **Walks UP, from each sheen to its nearest button** — not down from each button, which is the
/// obvious shape and the wrong one: buttons nest. A modal dialog's own root is a `Button` (that is
/// how it eats the clicks that miss its controls), so a descendant walk from there would light
/// every sheen in the dialog at once. The nearest enclosing button is the one whose sheen it is,
/// and there is exactly one of those.
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

    /// Drive one widget's [`Interaction`] through a press/release and read back whether the click
    /// landed — the whole of the law in [`glue_clicks`], exercised the way `ui_focus_system` writes
    /// it (see that function's doc for the three transitions).
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

    /// **A press is not a click.** The reference dispatches a stock `<Button>` from the mouse-UP
    /// handler alone (ctor click mask `0x100` = `LeftButtonUp`), so the action must not fire while
    /// the button is held — the whole glue layer used to fire here (1533).
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

    /// The release **over the button that took the press** is the click, and it fires exactly once.
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

    /// Press, slide off, release: **no click**. The ref hit-tests the release inside the frame
    /// (`0x76b020`) before firing, which is the escape hatch every player expects of a button they
    /// pressed by mistake. Bevy leaves the node `Pressed` through the slide and drops it to `None`
    /// on a release elsewhere, so the law reads it off the transition.
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

    /// A press that begins somewhere else and releases here is not this button's click.
    #[test]
    fn a_release_without_a_press_is_not_a_click() {
        let (mut app, widget) = click_app();
        assert!(
            !click_run(&mut app, widget, Interaction::Hovered),
            "no press ever landed"
        );
    }

    /// The caption whitens on hover and grays when disabled — the ref's `<HighlightFont>` /
    /// `<DisabledFont>` on `GlueButtonTemplate`.
    ///
    /// The regression this pins is a *depth* bug, so the test builds the real depth:
    /// [`super::widgets::outlined_text`] wraps every glue string in a layout wrapper and a −1px
    /// trim node before the string itself, which put `GlueCaption` three levels under the button
    /// while `glue_button_visuals` scanned the button's direct children. The sheen (a direct child)
    /// lit on hover and the caption never moved off gold (1533).
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
                // wrapper → trim → the real string, exactly as `outlined_text` builds it.
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

    /// **The chrome canvas passes what it does not take**. `ui_focus_system`
    /// reads a hovered node with no `FocusPolicy` as `Block` and stops the walk there, so a canvas
    /// spanning the whole boxed scene would eat every hover and press before the full-bleed scene
    /// pane under it saw one — and drag-to-rotate on the select and create screens would go dead
    /// with nothing on screen to show for it. Pinned here because the failure is invisible to
    /// every other check we run.
    #[test]
    fn the_chrome_canvas_never_swallows_the_scenes_drags() {
        let (_, policy, node) = glue_canvas();
        assert_eq!(policy, FocusPolicy::Pass);
        // …and it is sized by its own four insets, so [`fit_glue_canvas`] can move it by writing
        // two of them (a width would have to be recomputed, and could disagree with the camera).
        assert_eq!(node.position_type, PositionType::Absolute);
        assert_eq!((node.width, node.height), (Val::Auto, Val::Auto));
        for inset in [node.left, node.right, node.top, node.bottom] {
            assert_eq!(inset, Val::Px(0.0));
        }
    }

    /// The authored layout must FIT the window at every height — the B120 regression.
    /// [`screen_scale`] is the only thing standing between a 768-unit-tall tree and a shorter
    /// window, so a floor there silently amputates the bottom of every glue screen (the create
    /// screen's RANDOMIZE button is authored at y 719..749 of 768).
    #[test]
    fn the_authored_layout_fits_any_window_height() {
        /// Bottom edge of the create screen's lowest control (the RANDOMIZE button) in authored
        /// units: the configuration tower's TOPLEFT y 74 + its 645 in-tower offset + 30 tall.
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

    /// Spawn `button > wrapper > sheen`, so the sheen is a **grand**child — the shape
    /// `outlined_text` and every art overlay actually produce, and the one a direct-children scan
    /// would miss.
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

    /// Hover lights it, leaving darkens it — and it reaches a sheen nested below a wrapper.
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

    /// **`LockHighlight` holds it lit with the cursor elsewhere** — the reference's own verb, and
    /// what lets a selected row stay marked without the screen touching `Visibility`.
    #[test]
    fn a_locked_sheen_stays_lit_unhovered() {
        let (mut app, _, sheen) = sheen_app(false, true);
        app.update();
        assert!(sheen_lit(&app, sheen));
    }

    /// A disabled button lights for neither reason. The reference's `Disable()` takes the
    /// highlight with it, and an offline realm row (or a scroll arrow at the end of its travel)
    /// must not glow under the cursor.
    #[test]
    fn a_disabled_button_never_lights() {
        let (mut app, button, sheen) = sheen_app(true, true);
        app.update();
        assert!(!sheen_lit(&app, sheen), "locked but disabled: dark");
        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Hovered;
        app.update();
        assert!(!sheen_lit(&app, sheen), "and hovering does not revive it");
    }

    /// **The walk stops at the nearest button.** A modal dialog's root is itself a `Button` (that
    /// is how it eats the clicks that miss its controls), so hovering the dim must not light every
    /// sheen inside it — which is exactly what a descendant walk from each button would do.
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
