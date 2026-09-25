//! **The re-shape a `bevy_ui` text root loses when its LAST span is despawned** — an upstream
//! change-detection hole, netted here because falling into it is a hard crash whose panic names
//! none of our code. The way in: enter the world in window mode, then maximize the
//! window on Linux.
//!
//! ## The hole
//!
//! A `bevy_ui` text block is a `Text` root plus its `TextSpan` children. Two pieces of state hang
//! off the root and must agree:
//!
//! - `ComputedTextBlock::buffer` — the shaped cosmic-text buffer, whose every laid-out glyph
//!   carries the **index of the run it came from** in its `metadata`;
//! - `ComputedTextBlock::entities` — the span list that index is an index *into*, rebuilt only by
//!   `TextPipeline::update_buffer`.
//!
//! `bevy_text::detect_text_needs_rerender` is what asks for a re-shape, and on the root it watches
//! `Changed<Text>`, `Changed<TextFont>`, `Changed<TextLayout>`, `Changed<LineHeight>` and
//! `Changed<Children>`. **Despawning the last child does not *change* `Children` — it REMOVES
//! it** (`bevy_ecs`'s relationship `on_replace` hook queues a `remove::<Children>()` once the
//! collection empties), and `Changed<T>` cannot fire for a component the entity no longer has. So
//! a root emptied of spans keeps `needs_rerender == false`, keeps its multi-run buffer, and keeps
//! an `entities` list naming dead entities.
//!
//! Nothing goes wrong until the node is re-laid-out **without** being re-shaped, which is exactly
//! what `widget::text_system` does on any frame where the node's `ComputedNode` changed — a window
//! resize, in other words. `TextPipeline::update_text_layout_info` rebuilds its per-span
//! `glyph_info` by `iter_many`-ing the stored `entities`, and `iter_many` *skips* the dead ones, so
//! the list comes back short while the buffer's glyphs still carry `metadata == 1`. The next line
//! is `self.glyph_info[span_index]` (`bevy_text-0.18.1/src/pipeline.rs:398`) and the client is
//! gone. Nothing about this is platform-specific: Linux is simply where somebody maximized a
//! window after the loading screen had put its tip away.
//!
//! ## The net
//!
//! [`reshape_emptied_text_roots`] restores the invariant the upstream detector cannot see: a
//! `Text` root that just lost its `Children` component is marked changed, one system before the
//! detector runs, so the block is re-shaped in the same frame and can never be re-laid-out stale.
//!
//! It is a **net, not the mechanism** — the site that emptied a node is still expected to leave it
//! coherent (`game_tip::empty_tip` is the one that did not, and it does now). The net exists
//! because the failure mode is a crash rather than a wrong pixel, and because the panic points at
//! third-party code: B383 cost a pastebin core dump and a session to attribute.
//!
//! **Scope: `bevy_ui` text only.** `bevy_sprite`'s `Text2d` has the identical hole and no entry
//! here, because this tree spawns none — and the in-game interface's own `FontString`s do not go
//! through `bevy_ui` at all (they shape through `cosmic-text` under the UI pass). What is
//! left on `bevy_ui::Text` is the glue screens, the debug panel, and the loading screen's tip.

use bevy::prelude::*;
use bevy::text::detect_text_needs_rerender;

pub(crate) struct TextReshapePlugin;

impl Plugin for TextReshapePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            // Ordered against the detector itself rather than against `UiSystems::Content`: the
            // one thing that has to be true is that the mark lands before the detector reads it,
            // and naming the system says so where a set name would only imply it.
            reshape_emptied_text_roots.before(detect_text_needs_rerender::<Text>),
        );
    }
}

/// Mark a `Text` root changed when it loses its `Children` — see the module header.
///
/// `RemovedComponents<Children>` fires for every entity in the world that shed a hierarchy this
/// frame (models, portraits, the streamed scene), so the body is a single archetype lookup that
/// misses for all of them; only a live text root reaches the write.
fn reshape_emptied_text_roots(
    mut emptied: RemovedComponents<Children>,
    mut roots: Query<&mut Text>,
) {
    for entity in emptied.read() {
        if let Ok(mut text) = roots.get_mut(entity) {
            text.set_changed();
        }
    }
}

/// **The two `bevy_ui` calls that bracket a text node, driven by hand** — so a test can put a text
/// block into the state a real frame leaves it in, and then run the frame that used to crash.
///
/// `bevy_ui`'s own `UiPlugin` cannot: it wants a render app, a camera and a window to produce the
/// `ComputedNode` change a resize is. What it does *around* the node is only these two pipeline
/// calls, in this order, and both are `pub` — so the reproduction is exact without any of that.
#[cfg(test)]
pub(crate) mod harness {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::image::TextureAtlasLayout;
    use bevy::prelude::*;
    use bevy::text::{
        detect_text_needs_rerender, ComputedTextBlock, CosmicFontSystem, Font, FontAtlasSet,
        FontHinting, SwashCache, TextBounds, TextLayoutInfo, TextPipeline,
    };
    use bevy::ui::widget::TextUiReader;

    /// A node box wide enough that the test strings lay out on one line and narrow enough that the
    /// numbers stay small. Nothing in the mechanism depends on it.
    const BOUNDS: TextBounds = TextBounds::new(400.0, 200.0);

    /// Everything a headless text block needs: `TextPlugin` for the pipeline resources and its
    /// embedded default font (the only face an `AssetServer`-less test can shape with), plus the
    /// two asset stores the glyph atlas fills.
    pub(crate) fn add_text_plugins(app: &mut App) {
        app.add_plugins(bevy::text::TextPlugin);
        app.init_asset::<TextureAtlasLayout>();
        app.init_asset::<Image>();
    }

    /// Point the root and every span at the embedded default font, so [`shape`] can run without an
    /// asset load. Runtime resolves the real face off the patch chain instead.
    ///
    /// **`bypass_change_detection`, and the test suite does not work without it.** This stands in
    /// for the face the runtime would have resolved when the node was spawned, so it must not read
    /// as a `TextFont` change — `detect_text_needs_rerender` watches exactly that, and a harness
    /// that quietly asks for a re-shape every time it shapes cannot observe a missing one. It cost
    /// three of these tests a false green before the flag went on.
    fn use_default_font(app: &mut App, root: Entity) {
        let spans: Vec<Entity> = app
            .world()
            .get::<Children>(root)
            .map(|c| c.to_vec())
            .unwrap_or_default();
        for entity in core::iter::once(root).chain(spans) {
            if let Some(mut font) = app.world_mut().get_mut::<TextFont>(entity) {
                font.bypass_change_detection().font = Handle::default();
            }
        }
    }

    /// Shape the block, the way `widget::measure_text_system` does — the call that (re)fills
    /// `ComputedTextBlock::entities` and clears `needs_rerender`.
    pub(crate) fn shape(app: &mut App, root: Entity) {
        use_default_font(app, root);
        app.world_mut()
            .run_system_once_with(shape_one, root)
            .expect("the shaping system runs");
    }

    fn shape_one(
        In(root): In<Entity>,
        mut pipeline: ResMut<TextPipeline>,
        fonts: Res<Assets<Font>>,
        mut font_system: ResMut<CosmicFontSystem>,
        mut blocks: Query<(&TextLayout, &mut ComputedTextBlock, &FontHinting)>,
        mut reader: TextUiReader,
    ) {
        let (layout, mut computed, hinting) = blocks.get_mut(root).expect("a text root");
        pipeline
            .update_buffer(
                &fonts,
                reader.iter(root),
                layout.linebreak,
                layout.justify,
                BOUNDS,
                1.0,
                &mut computed,
                &mut font_system,
                *hinting,
            )
            .expect("the embedded default font shapes headless");
    }

    /// **The frame a window resize is**, in the order `UiPlugin` runs it: the detector, then the
    /// re-shape it asked for (`measure_text_system`), then the re-layout every node whose
    /// `ComputedNode` moved gets (`text_system`). The last of the three is what panicked.
    ///
    /// Any system the app has scheduled `.before(detect_text_needs_rerender::<Text>)` — the net in
    /// this module, when the plugin is installed — runs first, exactly as it would live.
    pub(crate) fn resize_frame(app: &mut App, root: Entity) {
        app.update();
        if app
            .world()
            .get::<ComputedTextBlock>(root)
            .is_some_and(ComputedTextBlock::needs_rerender)
        {
            shape(app, root);
        }
        app.world_mut()
            .run_system_once_with(relayout_one, root)
            .expect("the relayout system runs");
    }

    fn relayout_one(
        In(root): In<Entity>,
        mut pipeline: ResMut<TextPipeline>,
        mut atlas_set: ResMut<FontAtlasSet>,
        mut atlas_layouts: ResMut<Assets<TextureAtlasLayout>>,
        mut images: ResMut<Assets<Image>>,
        mut font_system: ResMut<CosmicFontSystem>,
        mut swash_cache: ResMut<SwashCache>,
        text_font_query: Query<&TextFont>,
        mut blocks: Query<(&TextLayout, &mut TextLayoutInfo, &mut ComputedTextBlock)>,
    ) {
        let (layout, mut info, mut computed) = blocks.get_mut(root).expect("a text root");
        pipeline
            .update_text_layout_info(
                &mut info,
                text_font_query,
                1.0,
                &mut atlas_set,
                &mut atlas_layouts,
                &mut images,
                &mut computed,
                &mut font_system,
                &mut swash_cache,
                BOUNDS,
                layout.justify,
            )
            .expect("the relayout succeeds");
    }

    /// An app with the text stack and the real detector scheduled where `UiPlugin` puts it.
    /// `net` installs [`super::TextReshapePlugin`] — the two tests below run the same world with
    /// and without it.
    pub(crate) fn text_app(net: bool) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        add_text_plugins(&mut app);
        app.add_systems(PostUpdate, detect_text_needs_rerender::<Text>);
        if net {
            app.add_plugins(super::TextReshapePlugin);
        }
        app
    }
}

#[cfg(test)]
mod tests {
    use super::harness::{resize_frame, shape, text_app};
    use bevy::prelude::*;
    use bevy::text::ComputedTextBlock;

    /// A two-run block: the root's own text plus one `TextSpan` child, which is the shape every
    /// coloured string in this tree has (`|cffffd100Tip:|r …`, the glue's markup runs).
    fn two_run_root(app: &mut App) -> Entity {
        let root = app
            .world_mut()
            .spawn((
                Text::new("Tip:"),
                TextLayout::default(),
                TextFont::default(),
                TextColor::WHITE,
            ))
            .with_children(|c| {
                c.spawn((
                    TextSpan::new(" talk to the innkeeper."),
                    TextFont::default(),
                ));
            })
            .id();
        app.update();
        shape(app, root);
        assert_eq!(
            app.world()
                .get::<ComputedTextBlock>(root)
                .map(|b| b.entities().len()),
            Some(2),
            "the shaped block lists both runs"
        );
        root
    }

    /// **The hole itself, named at the level the crash needs.** Despawning the last span leaves
    /// `needs_rerender` false — `Children` was removed, not changed — so `bevy_ui` will re-lay-out
    /// a buffer whose glyphs point at a run that no longer exists. This asserts the *broken*
    /// upstream behaviour on purpose: it is the premise the net and every emptying site rest on,
    /// and a bevy release that closes the hole should fail here and retire both.
    #[test]
    fn upstream_misses_the_last_spans_despawn() {
        let mut app = text_app(false);
        let root = two_run_root(&mut app);

        app.world_mut()
            .entity_mut(root)
            .despawn_related::<Children>();
        app.update();

        assert!(
            app.world().get::<Children>(root).is_none(),
            "the last child's despawn REMOVES `Children`, which is the whole mechanism"
        );
        assert!(
            !app.world()
                .get::<ComputedTextBlock>(root)
                .expect("the block")
                .needs_rerender(),
            "upstream cannot see the removal — if this ever fails, bevy fixed it"
        );
    }

    /// **The crash itself.** The same sequence with the net taken out:
    /// the resize frame re-lays-out the stale buffer and `bevy_text` indexes `glyph_info[1]` of a
    /// one-entry list. Pinned as a `should_panic` because it is the whole justification for the
    /// net — when a bevy release closes the hole this fails, and the net (and this test, and the
    /// premise test above) go with it.
    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn without_the_net_the_resize_panics_inside_bevy_text() {
        let mut app = text_app(false);
        let root = two_run_root(&mut app);

        app.world_mut()
            .entity_mut(root)
            .despawn_related::<Children>();
        app.update();
        resize_frame(&mut app, root);
    }

    /// **B383, end to end.** Same world, plus the net: the emptied root is marked for re-shape, so
    /// the resize frame re-shapes before it re-lays-out and the pipeline never indexes a dead run.
    /// Without [`super::TextReshapePlugin`] this same sequence panics inside `bevy_text` with
    /// `index out of bounds` at `pipeline.rs:398` — the client's own crash.
    #[test]
    fn the_net_reshapes_an_emptied_root_before_the_next_resize() {
        let mut app = text_app(true);
        let root = two_run_root(&mut app);

        app.world_mut()
            .entity_mut(root)
            .despawn_related::<Children>();
        app.update();

        assert!(
            app.world()
                .get::<ComputedTextBlock>(root)
                .expect("the block")
                .needs_rerender(),
            "the net marks the emptied root for a re-shape"
        );

        // The frame the report is about. It must not panic, and it must leave the block honest.
        resize_frame(&mut app, root);
        assert_eq!(
            app.world()
                .get::<ComputedTextBlock>(root)
                .map(|b| b.entities().len()),
            Some(1),
            "the re-shaped block lists the root alone"
        );
    }

    /// The control: a root that keeps a span through a resize is untouched by any of this — the
    /// net must not be doing the re-shaping in the ordinary case.
    #[test]
    fn a_root_that_kept_its_span_is_not_reshaped() {
        let mut app = text_app(true);
        let root = two_run_root(&mut app);

        app.update();
        assert!(
            !app.world()
                .get::<ComputedTextBlock>(root)
                .expect("the block")
                .needs_rerender(),
            "nothing changed, so nothing is asked to re-shape"
        );
        resize_frame(&mut app, root);
        assert_eq!(
            app.world()
                .get::<ComputedTextBlock>(root)
                .map(|b| b.entities().len()),
            Some(2),
            "both runs are still there"
        );
    }
}
