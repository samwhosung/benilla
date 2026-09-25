//! Re-shapes a `bevy_ui` text root whose last span was despawned, which upstream misses.
//!
//! Despawning the last `TextSpan` removes `Children` rather than changing it, so
//! `detect_text_needs_rerender` never fires and the root keeps a multi-run buffer and a span list
//! naming dead entities. The next re-layout without a re-shape, such as a window resize, indexes
//! `glyph_info` past the shortened list and panics (`bevy_text-0.18.1/src/pipeline.rs:398`).
//!
//! This is a net: the site that empties a node should still leave it coherent. It covers
//! `bevy_ui` text only; `Text2d` has the same hole, but this tree spawns none.

use bevy::prelude::*;
use bevy::text::detect_text_needs_rerender;

pub(crate) struct TextReshapePlugin;

impl Plugin for TextReshapePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            // The mark must land before the detector reads it.
            reshape_emptied_text_roots.before(detect_text_needs_rerender::<Text>),
        );
    }
}

/// Marks a `Text` root changed when it loses its `Children`.
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

/// The two `bevy_ui` pipeline calls around a text node, driven by hand, since `UiPlugin` needs a
/// render app, a camera and a window.
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

    /// Wide enough for one line; nothing depends on it.
    const BOUNDS: TextBounds = TextBounds::new(400.0, 200.0);

    /// `TextPlugin` with its embedded default font, and the glyph atlas's asset stores.
    pub(crate) fn add_text_plugins(app: &mut App) {
        app.add_plugins(bevy::text::TextPlugin);
        app.init_asset::<TextureAtlasLayout>();
        app.init_asset::<Image>();
    }

    /// Points the root and spans at the embedded default font. It bypasses change detection: a
    /// `TextFont` change would request the re-shape the tests check is missing.
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

    /// Shapes the block as `widget::measure_text_system` does.
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

    /// A resize frame in `UiPlugin`'s order: the detector, the re-shape it asked for, then the
    /// re-layout (`text_system`).
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

    /// The text stack and the real detector; `net` installs [`super::TextReshapePlugin`].
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

    /// The root's text plus one `TextSpan`, the shape of every coloured string.
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

    /// Asserts the upstream hole on purpose: a bevy that closes it fails here and retires the net.
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

    /// Without the net, `bevy_text` indexes `glyph_info[1]` of a one-entry list.
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

        resize_frame(&mut app, root);
        assert_eq!(
            app.world()
                .get::<ComputedTextBlock>(root)
                .map(|b| b.entities().len()),
            Some(1),
            "the re-shaped block lists the root alone"
        );
    }

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
