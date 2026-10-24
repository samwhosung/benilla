//! The transfer screen's route: a transport crossing's loading screen is the world map with the
//! leg's route over it (`0x4072c0`, `loadingscreen/route` in benilla-re). The twelve
//! `Interface\WorldMap\World\World%d` tiles fill the screen in a 4 × 3 grid (`0x4081e0`), the
//! markers sit at the stop left and the stop ahead, and the ribbon of ship quads between them
//! reveals with the load (`0x406fb0`). With a route the plain art, its bar and the tip do not draw
//! (`0x406cd0`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_assets::{LockRecover, LooseOpen, WorldAssets};
use benilla_formats::{
    load_loading_screen_taxi_splines, load_taxi_path_nodes, load_world_map_continent_catalog,
    LoadingScreenTaxiSplines, Route, RouteQuad, TaxiLegTable, WorldMapContinentCatalog,
};
use benilla_world::map_proj::{world_uv, WorldProj};

use super::{LoadingArea, LoadingScreen};

/// `0x82f1f8`, the markers' and ship's sheet.
const DYNAMIC_ELEMENTS: &str = "Interface\\Glues\\LoadingScreens\\DynamicElements.blp";

/// The start-up tables the route reads: the leg table `0x408000` builds from `TaxiPathNode`, the
/// `LoadingScreenTaxiSplines` rows and the `WorldMapContinent` rows `0x4a72b0` projects through.
#[derive(Resource)]
pub(super) struct TransferRoutes {
    legs: TaxiLegTable,
    splines: LoadingScreenTaxiSplines,
    continents: WorldMapContinentCatalog,
}

/// Load the route's tables; without any of them no transfer draws a route.
pub(super) fn setup_transfer_routes(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else {
        return;
    };
    let mut chain = assets.chain.lock_recover();
    let tables = (|| {
        let nodes = load_taxi_path_nodes(&mut chain)?;
        anyhow::Ok(TransferRoutes {
            legs: TaxiLegTable::build(&nodes),
            splines: load_loading_screen_taxi_splines(&mut chain)?,
            continents: load_world_map_continent_catalog(&mut chain)?,
        })
    })();
    match tables {
        Ok(routes) => {
            commands.insert_resource(routes);
        }
        Err(e) => warn!("loading screen: no transfer routes: {e:#}"),
    }
}

impl TransferRoutes {
    /// The route for a transport on taxi path `path`: for `0x4072c0` the leg whose first stop is
    /// on the map left (`0x4073f0` with −1 when none, which fails), for `0x407de0` the leg it is
    /// given; then its vertices.
    fn route(&self, path: u32, leg: RouteLeg) -> Option<Route> {
        let leg = match leg {
            RouteLeg::Leaving(map) => self.legs.leg_from(path, map)?,
            RouteLeg::Index(leg) => leg,
        };
        self.legs.route(path, leg, &self.splines, |map, x, y| {
            // 0x4a72b0: the horizontal result and one minus the vertical, through the map's
            // `WorldMapContinent` row; no row fails.
            let c = self.continents.get(map)?;
            let proj = WorldProj {
                offset_u: c.offset_x,
                offset_v: c.offset_y,
                scale: c.scale,
            };
            let (u, v) = world_uv(proj, x, y);
            Some([u, 1.0 - v])
        })
    }
}

/// Which leg a route shows: the one leaving a map (`0x4072c0`) or one by its index (`0x407de0`).
#[derive(Clone, Copy, Debug)]
pub(super) enum RouteLeg {
    Leaving(u32),
    Index(usize),
}

/// A built route and its textures, `0x8828e0` and the handles `0x4074b0` loads.
pub(crate) struct ActiveRoute {
    route: Route,
    elements: Handle<Image>,
    tiles: Vec<Handle<Image>>,
}

/// What `0x4072c0` reads to build a route: the player's transport and its record.
#[derive(SystemParam)]
pub(super) struct RouteSource<'w, 's> {
    routes: Option<Res<'w, TransferRoutes>>,
    player: Option<Res<'w, crate::player::Player>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    templates: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
}

impl RouteSource<'_, '_> {
    /// `0x4072c0(entry, from_map)` and `0x407de0(entry, leg)`: a route when the player rides a
    /// game object whose entry is `entry` and its taxi path has that leg, and `0x4074b0` then
    /// loads every texture; else `None`, the plain screen.
    pub(super) fn build(
        &self,
        entry: u32,
        leg: RouteLeg,
        assets: Option<&mut WorldAssets>,
        images: &mut Assets<Image>,
    ) -> Option<ActiveRoute> {
        let store = self
            .player
            .as_ref()?
            .ride_entity()
            .and_then(|e| self.stores.get(e).ok())?;
        // 0x407320: the transport's entry field against the message's.
        if store.0.object_entry() != Some(entry) {
            return None;
        }
        // 0x407338: the record's `taxiPathID` column (semantic 0x23).
        let path = self
            .templates
            .as_ref()?
            .of(&store.0)?
            .mo_transport?
            .taxi_path_id;
        let route = self.routes.as_ref()?.route(path, leg)?;
        let assets = assets?;
        let mut load = |path: &str| assets.sprite_texture_opened(path, LooseOpen::Archives, images);
        let elements = load(DYNAMIC_ELEMENTS)?;
        let tiles = (1..=12)
            .map(|i| load(&format!("Interface\\WorldMap\\World\\World{i}.blp")))
            .collect::<Option<Vec<_>>>()?;
        info!("loading screen: route for path {path}, {leg:?}");
        Some(ActiveRoute {
            route,
            elements,
            tiles,
        })
    }
}

/// The layer under [`LoadingArea`] that holds the drawn route, rebuilt when the route changes.
#[derive(Component)]
pub(super) struct RouteLayer;

/// Ribbon quad `k` of the drawn route.
#[derive(Component)]
pub(super) struct RouteDot(usize);

/// `0x4081e0`'s tile `(c, r)`: its unit-box rectangle (left, bottom, right, top, y up) for its pixels
/// of the 1002 × 668 map, and the part of its 256-pixel texture it shows.
fn tile(c: u32, r: u32) -> ([f32; 4], Vec2) {
    let (x0, y0) = (c * 256, r * 256);
    let (x1, y1) = ((x0 + 256).min(1002), (y0 + 256).min(668));
    let rect = [
        x0 as f32 / 1002.0,
        1.0 - y1 as f32 / 668.0,
        x1 as f32 / 1002.0,
        1.0 - y0 as f32 / 668.0,
    ];
    // 0x40831a, 0x408328: the third column shows 0.9140625 of its width, the last row 0.609375.
    let w = if c == 3 { 0.914_062_5 } else { 1.0 };
    let h = if r == 2 { 0.609_375 } else { 1.0 };
    (rect, Vec2::new(w, h))
}

/// A quad of the unit box, `[left, bottom, right, top]`, as a node of the fitted area.
fn quad_node(rect: [f32; 4]) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Percent(rect[0] * 100.0),
        bottom: Val::Percent(rect[1] * 100.0),
        width: Val::Percent((rect[2] - rect[0]) * 100.0),
        height: Val::Percent((rect[3] - rect[1]) * 100.0),
        ..default()
    }
}

/// A route quad's node and the pixel rectangle of `image` its texture coordinates name, `v` down.
fn route_quad(q: &RouteQuad, image: Handle<Image>, size: Vec2) -> (Node, ImageNode) {
    let (lo, hi) = (q[0], q[3]);
    let uv_min = Vec2::new(lo.uv[0].min(hi.uv[0]), lo.uv[1].min(hi.uv[1]));
    let uv_max = Vec2::new(lo.uv[0].max(hi.uv[0]), lo.uv[1].max(hi.uv[1]));
    let node = quad_node([lo.pos[0], lo.pos[1], hi.pos[0], hi.pos[1]]);
    let image = ImageNode {
        image,
        rect: Some(Rect::from_corners(uv_min * size, uv_max * size)),
        image_mode: NodeImageMode::Stretch,
        ..default()
    };
    (node, image)
}

/// Draw the route `0x406f20` and `0x406fb0` draw: the tiles, then the ribbon's revealed quads,
/// then the two markers over them. The layer is respawned when the screen takes a new route and
/// hidden while there is none.
#[allow(clippy::type_complexity)]
pub(super) fn draw_route(
    mut commands: Commands,
    screen: Res<LoadingScreen>,
    images: Res<Assets<Image>>,
    area: Query<Entity, With<LoadingArea>>,
    layer: Query<(Entity, &Visibility), With<RouteLayer>>,
    mut dots: Query<(&RouteDot, &mut Visibility), Without<RouteLayer>>,
    mut built: Local<u32>,
) {
    let route = screen
        .route
        .as_ref()
        .filter(|_| screen.active && !screen.blackout);
    if *built != screen.route_built {
        *built = screen.route_built;
        for (e, _) in &layer {
            commands.entity(e).despawn();
        }
        let (Some(active), Ok(area)) = (screen.route.as_ref(), area.single()) else {
            return;
        };
        let size = |h: &Handle<Image>| images.get(h).map_or(Vec2::ONE, |i| i.size().as_vec2());
        let elements = size(&active.elements);
        commands.entity(area).with_children(|area| {
            area.spawn((
                RouteLayer,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                if route.is_some() {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
            ))
            .with_children(|layer| {
                for (i, image) in active.tiles.iter().enumerate() {
                    let (rect, part) = tile(i as u32 % 4, i as u32 / 4);
                    layer.spawn((
                        quad_node(rect),
                        ImageNode {
                            image: image.clone(),
                            rect: Some(Rect::from_corners(Vec2::ZERO, part * size(image))),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                    ));
                }
                let revealed = active.route.revealed(screen.displayed);
                for (k, q) in active.route.ribbon.iter().enumerate() {
                    let (node, image) = route_quad(q, active.elements.clone(), elements);
                    let vis = if k < revealed {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    };
                    layer.spawn((RouteDot(k), node, image, vis));
                }
                for q in &active.route.markers {
                    layer.spawn(route_quad(q, active.elements.clone(), elements));
                }
            });
        });
        return;
    }
    let want = if route.is_some() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for (e, vis) in &layer {
        if *vis != want {
            commands.entity(e).insert(want);
        }
    }
    let Some(active) = route else {
        return;
    };
    let revealed = active.route.revealed(screen.displayed);
    for (dot, mut vis) in &mut dots {
        let want = if dot.0 < revealed {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        vis.set_if_neq(want);
    }
}

#[cfg(test)]
impl ActiveRoute {
    /// A route with no texture, for the screen's tests.
    pub(crate) fn test(route: Route) -> Self {
        Self {
            route,
            elements: Handle::default(),
            tiles: vec![Handle::default(); 12],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centre(q: &RouteQuad) -> [f32; 2] {
        [
            (q[0].pos[0] + q[3].pos[0]) * 0.5,
            (q[0].pos[1] + q[3].pos[1]) * 0.5,
        ]
    }

    /// The Booty Bay to Ratchet ship leaving Kalimdor: Ratchet's marker west of and above Booty
    /// Bay's on the world map, y up, the ribbon between them, every quad on the screen.
    #[test]
    fn leaving_ratchet_draws_ratchet_to_booty_bay() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let nodes = load_taxi_path_nodes(&mut chain).expect("TaxiPathNode");
        let routes = TransferRoutes {
            legs: TaxiLegTable::build(&nodes),
            splines: load_loading_screen_taxi_splines(&mut chain).expect("splines"),
            continents: load_world_map_continent_catalog(&mut chain).expect("continents"),
        };
        let route = routes
            .route(241, RouteLeg::Leaving(1))
            .expect("path 241 has a route leaving Kalimdor");
        let (ratchet, booty_bay) = (centre(&route.markers[0]), centre(&route.markers[1]));
        eprintln!(
            "ratchet {ratchet:?} booty bay {booty_bay:?} {} quads",
            route.ribbon.len()
        );
        assert!(ratchet[0] < booty_bay[0] && ratchet[1] > booty_bay[1]);
        let ends = (route.ribbon.first().unwrap(), route.ribbon.last().unwrap());
        for (got, want) in [(centre(ends.0), ratchet), (centre(ends.1), booty_bay)] {
            assert!((got[0] - want[0]).abs() < 1e-5 && (got[1] - want[1]).abs() < 1e-5);
        }
        for v in route.ribbon.iter().chain(&route.markers).flatten() {
            assert!((0.0..=1.0).contains(&v.pos[0]) && (0.0..=1.0).contains(&v.pos[1]));
        }
        assert!(
            routes.route(241, RouteLeg::Leaving(0)).is_some(),
            "and leaving Azeroth"
        );
        assert!(
            routes.route(241, RouteLeg::Leaving(30)).is_none(),
            "a map it never leaves has none"
        );
        // `0x407de0` names the leg itself: leg 0 is the one leaving Kalimdor.
        let by_index = routes.route(241, RouteLeg::Index(0)).expect("leg 0");
        assert_eq!(by_index.markers, route.markers);
        // Path 301 changes legs on Eastern Kingdoms, the one change that reaches `0x407de0`.
        assert_eq!(
            [0, 1, 2].map(|l| routes.route(301, RouteLeg::Index(l)).is_some()),
            [true, true, false]
        );
    }

    /// `0x406f20`: the twelve tiles, the ribbon's quads revealed as the load goes, the two
    /// markers; nothing once the screen has gone.
    #[test]
    fn the_route_layer_reveals_the_ribbon_with_the_load() {
        let quad = [benilla_formats::RouteVertex {
            pos: [0.5; 2],
            uv: [0.0; 2],
        }; 4];
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<LoadingScreen>()
            .add_systems(Update, draw_route);
        app.world_mut().spawn((LoadingArea, Node::default()));
        {
            let mut screen = app.world_mut().resource_mut::<LoadingScreen>();
            screen.active = true;
            screen.set_route(Some(ActiveRoute::test(Route {
                markers: [quad; 2],
                ribbon: vec![quad; 5],
            })));
        }
        app.update();
        let shown = |app: &mut App| {
            let mut q = app.world_mut().query::<(&RouteDot, &Visibility)>();
            q.iter(app.world())
                .filter(|(_, v)| **v != Visibility::Hidden)
                .count()
        };
        let images = app
            .world_mut()
            .query_filtered::<(), With<ImageNode>>()
            .iter(app.world())
            .count();
        assert_eq!(images, 12 + 5 + 2, "the tiles, the ribbon and the markers");
        assert_eq!(shown(&mut app), 1, "the first quad shows at once");
        app.world_mut().resource_mut::<LoadingScreen>().displayed = 0.5;
        app.update();
        assert_eq!(shown(&mut app), 3);
        app.world_mut().resource_mut::<LoadingScreen>().dismiss();
        app.update();
        let layers = app
            .world_mut()
            .query_filtered::<(), With<RouteLayer>>()
            .iter(app.world())
            .count();
        assert_eq!(layers, 0, "the dismiss takes the route away");
    }
}
