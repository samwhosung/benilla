//! Bodies arriving together, against the real install: each composite runs off the main thread and
//! its body is built only once it can draw dressed, as the reference withholds a unit whose first
//! composite is pending (`0x607da0` → `0x477860`, `0x481749` → `0x710c50`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use benilla_formats::{load_item_display_catalog, open_chain, CharSections, NpcAppearance};
use benilla_protocol::ObjectFields;

use super::super::skin_composite::{land_skin_composites, SkinKey};
use super::super::{Equipment, ItemDisplays};
use super::*;

/// The one character-body display every arrival wears.
const BODY: u32 = 42;

/// Worn display ids by bodyslot − 2: tier-2 sets at their 1.12 displays.
const SETS: [[u32; 8]; 4] = [
    [10840, 33983, 33990, 33986, 33989, 33982, 33984, 0],
    [0, 33650, 31110, 31115, 31111, 31127, 33651, 0],
    [0, 33667, 33665, 33672, 34269, 33666, 33668, 0],
    [0, 34081, 34078, 34084, 34083, 34079, 34082, 0],
];

/// One synthetic batch that can feather, so a built body has a child to show and joins its
/// appear ramp.
fn body_part() -> super::super::EntityPart {
    super::super::EntityPart {
        mesh: Handle::default(),
        geometry: Arc::new(benilla_formats::RenderSubmesh::default()),
        aabb: None,
        skinned_mesh: None,
        welded_billboard: false,
        material: Handle::default(),
        material_interior: None,
        material_interior_bake: None,
        material_interior_bake_blend: None,
        fade_blend: Some(Handle::default()),
        zfill: None,
        blend: benilla_formats::ModelBlend::Opaque,
        additive: false,
        two_sided: false,
        geoset_id: 0,
        char_slot: None,
        billboard: None,
        alpha_anim: None,
        rgb_anim: None,
        rgb_seq: None,
        uv_anim: None,
        uv_seq: None,
        uv_rot_seq: None,
        uv_scale_seq: None,
        ground_quad: None,
    }
}

/// A loaded character-body display; `npc` gives it a creature-display look.
fn body_display(npc: Option<NpcAppearance>) -> DisplayModel {
    DisplayModel {
        handle: ModelHandle::M2(Handle::default()),
        parts: Some(vec![body_part()]),
        is_character_body: true,
        npc_appearance: npc,
        ..super::super::display::empty_shell()
    }
}

/// The attach chain over the real tables and chain: finished composites land, then bodies build.
fn app() -> Option<App> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = open_chain(&data).expect("open chain");
    let tables = CharSections::load(&mut chain).expect("CharSections");
    let items = load_item_display_catalog(&mut chain).expect("ItemDisplayInfo");
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Image>()
        .init_asset::<Mesh>()
        .init_asset::<benilla_assets::materials::WowModelMaterial>()
        .init_resource::<benilla_world::model_render::ModelMaterials>()
        .init_resource::<super::super::corpse::BonesModels>()
        .init_resource::<SkinComposites>()
        .init_resource::<merge::MergedFormsCache>()
        .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
        .init_resource::<benilla_world::doodad_anim::TintAnimMaterials>()
        .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
        .init_resource::<benilla_world::rig_palette::RigPalettes>()
        .init_resource::<benilla_world::collision::ColliderEpoch>()
        .insert_resource(CubeAssets {
            mesh: Handle::default(),
            player_mesh: Handle::default(),
            player_mat: Handle::default(),
            npc_mat: Handle::default(),
        })
        .insert_resource(SkinSections::new(tables, Arc::new(Mutex::new(chain))))
        .insert_resource(ItemDisplays::icons_for_tests(items))
        .add_systems(
            Update,
            (
                super::super::stamp_arrivals,
                land_skin_composites,
                attach_entity_visuals,
            )
                .chain(),
        );
    Some(app)
}

/// A player's look: race, sex, face, hair style and its worn set.
#[derive(Clone, Copy)]
struct Look {
    race: u8,
    sex: u8,
    face: u8,
    hair: u8,
    equip: [u32; 8],
}

impl Look {
    fn key(&self) -> SkinKey {
        SkinKey {
            race: self.race,
            sex: self.sex,
            skin: 0,
            face: self.face,
            facial_hair: 0,
            hair_style: self.hair,
            hair_color: 0,
            equip: self.equip,
            emblem: None,
            tabard_preview: false,
        }
    }
}

/// A player arriving in `look`, its gear settled.
fn arrive(app: &mut App, look: Look) -> Entity {
    let bytes_0 = u32::from(look.race) | u32::from(look.sex) << 16;
    let bytes = u32::from(look.face) << 8 | u32::from(look.hair) << 16;
    app.world_mut()
        .spawn((
            NetEntity {
                kind: EntityKind::Player,
                display_id: Some(BODY),
                scale: 1.0,
            },
            Equipment {
                bodyslots: look.equip,
                settled: true,
                ..Default::default()
            },
            // UNIT_FIELD_BYTES_0 (36): race, gender; PLAYER_BYTES (193): face, hair style.
            ObjectStore(ObjectFields::from_pairs(&[(36, bytes_0), (193, bytes)])),
            Transform::default(),
            Visibility::default(),
        ))
        .id()
}

fn built(app: &App, e: Entity) -> bool {
    app.world().entity(e).contains::<VisualAttached>()
}

fn children(app: &App, e: Entity) -> usize {
    app.world().get::<Children>(e).map_or(0, |c| c.len())
}

/// A crowd arriving in one frame costs that frame no composite: every body waits, drawing
/// nothing, while its look composites off the main thread, and each is built on a frame its
/// finished atlas is already cached. A look already cached builds at once.
#[test]
fn a_crowd_composites_off_the_main_thread_and_each_body_waits_for_its_atlas() {
    let Some(mut app) = app() else { return };
    app.world_mut().insert_resource(Creatures {
        catalog: Default::default(),
        models: HashMap::from([(BODY, body_display(None))]),
    });
    let races = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let looks: Vec<Look> = (0..12)
        .map(|i| Look {
            race: races[i % races.len()],
            sex: (i / races.len()) as u8,
            face: (i % 3) as u8,
            hair: (i % 4) as u8,
            equip: SETS[i % SETS.len()],
        })
        .collect();
    let crowd: Vec<Entity> = looks.iter().map(|&l| arrive(&mut app, l)).collect();

    app.update();
    let composites = app.world().resource::<SkinComposites>();
    assert_eq!(
        composites.done.len(),
        0,
        "the arrival frame composited nothing on the main thread"
    );
    assert_eq!(composites.running(), crowd.len(), "one composite per look");
    for &e in &crowd {
        assert!(!built(&app, e), "a body waits for its composite");
        assert_eq!(children(&app, e), 0, "and draws nothing meanwhile");
    }

    let deadline = Instant::now() + Duration::from_secs(20);
    while crowd.iter().any(|&e| !built(&app, e)) {
        assert!(Instant::now() < deadline, "every composite lands");
        std::thread::sleep(Duration::from_millis(1));
        app.update();
        for (&e, look) in crowd.iter().zip(&looks) {
            if built(&app, e) {
                let atlas = app
                    .world_mut()
                    .resource_mut::<SkinComposites>()
                    .done
                    .fetch(&look.key());
                assert!(
                    matches!(atlas, Some(Some(_))),
                    "a body is built only over its finished atlas"
                );
                assert!(children(&app, e) > 0, "and draws once built");
            }
        }
    }
    assert_eq!(app.world().resource::<SkinComposites>().running(), 0);

    // A look already composited is not waited on.
    let twin = arrive(&mut app, looks[0]);
    app.update();
    assert!(built(&app, twin), "a cached look builds on its first frame");
    assert_eq!(app.world().resource::<SkinComposites>().running(), 0);
}

/// A body that waited on its composite joins the appear ramp its arrival started: the reference's
/// fade is stamped by the create block's appear handler (`0x613af0` → `0x614f80`) and ramps on the
/// wall clock whatever ShouldRender answers (`0x614a90` at `0x60800c`).
#[test]
fn a_body_that_waited_for_its_atlas_fades_from_its_arrival() {
    let Some(mut app) = app() else { return };
    app.world_mut().insert_resource(Creatures {
        catalog: Default::default(),
        models: HashMap::from([(BODY, body_display(None))]),
    });
    let look = Look {
        race: 1,
        sex: 0,
        face: 0,
        hair: 0,
        equip: SETS[0],
    };
    let player = arrive(&mut app, look);
    app.update();
    let arrived = app
        .world()
        .get::<super::super::Arrival>(player)
        .expect("stamped the frame it arrived")
        .0;
    assert!(!built(&app, player), "the body waits for its atlas");

    let deadline = Instant::now() + Duration::from_secs(20);
    while !built(&app, player) {
        assert!(Instant::now() < deadline, "the composite lands");
        std::thread::sleep(Duration::from_millis(1));
        app.update();
    }
    let built_at = app.world().resource::<Time>().elapsed_secs();
    assert!(
        built_at > arrived,
        "the body was built frames after it arrived"
    );
    assert_eq!(
        app.world()
            .get::<benilla_world::model_fade::UnitAppearFade>(player)
            .copied(),
        Some(benilla_world::model_fade::UnitAppearFade::Pending { since: arrived }),
        "its ramp runs from its arrival, not from its build"
    );
}

/// benilla's own palette heal rebuilds a body that was drawn: its composite is forced, so even a
/// look the cache has dropped rebuilds in the same frame and never drops out.
#[test]
fn a_healed_rig_rebuilds_its_body_in_the_same_frame() {
    use bevy::ecs::system::RunSystemOnce;
    let Some(mut app) = app() else { return };
    app.world_mut().insert_resource(Creatures {
        catalog: Default::default(),
        models: HashMap::from([(BODY, body_display(None))]),
    });
    let look = Look {
        race: 2,
        sex: 1,
        face: 1,
        hair: 2,
        equip: SETS[1],
    };
    let player = arrive(&mut app, look);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !built(&app, player) {
        assert!(Instant::now() < deadline, "the composite lands");
        std::thread::sleep(Duration::from_millis(1));
        app.update();
    }

    // The scope sweep dropped the look, then the rig starved and heals.
    app.world_mut()
        .resource_mut::<SkinComposites>()
        .done
        .clear();
    app.world_mut()
        .entity_mut(player)
        .insert((crate::net::Guid(7), benilla_world::rig_palette::RigStarved));
    app.world_mut()
        .run_system_once(super::super::live_display::heal_rig_starved)
        .expect("the heal runs");
    assert!(!built(&app, player), "the heal tore the visual down");
    app.update();
    assert!(built(&app, player), "and it rebuilt in the same frame");
    assert!(children(&app, player) > 0, "drawing");
    assert_eq!(app.world().resource::<SkinComposites>().running(), 0);
}

/// A creature display with a baked atlas loads it as is and is never waited on (`0x477866`); one
/// without composites like a player and waits.
#[test]
fn a_baked_npc_skin_never_waits_on_the_compositor() {
    let Some(mut app) = app() else { return };
    let npc = |bake_name: Option<&str>| NpcAppearance {
        race: 1,
        sex: 0,
        skin: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
        equipment: [0; 10],
        bake_name: bake_name.map(str::to_owned),
    };
    app.world_mut().insert_resource(Creatures {
        catalog: Default::default(),
        models: HashMap::from([
            (7, body_display(Some(npc(Some("HumanMaleGuard.blp"))))),
            (8, body_display(Some(npc(None)))),
        ]),
    });
    let mut unit = |display: u32| {
        app.world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: Some(display),
                    scale: 1.0,
                },
                Transform::default(),
                Visibility::default(),
            ))
            .id()
    };
    let (baked, live) = (unit(7), unit(8));
    app.update();
    assert!(built(&app, baked), "a baked skin builds on its first frame");
    assert!(
        !built(&app, live),
        "a composited NPC skin waits for its atlas"
    );
    assert_eq!(app.world().resource::<SkinComposites>().running(), 1);
}

/// What a crowd of 40 distinct looks costs the main thread: its arrival frame's requests, each
/// frame's landing, against compositing the same crowd on this thread.
#[test]
#[ignore = "instrument: run by hand — cargo test -p benilla-app --lib arrival_tests::crowd_main_thread_cost -- --ignored --nocapture"]
fn crowd_main_thread_cost() {
    use super::super::skin_composite::SkinComposites;
    let data = benilla_formats::wow_data_or_skip!();
    bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    let mut chain = open_chain(&data).expect("open chain");
    let tables = CharSections::load(&mut chain).expect("CharSections");
    let items = load_item_display_catalog(&mut chain).expect("ItemDisplayInfo");
    let sections = SkinSections::new(tables, Arc::new(Mutex::new(chain)));
    let races = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let keys: Vec<SkinKey> = (0..40)
        .map(|i| {
            Look {
                race: races[i % races.len()],
                sex: (i / races.len() % 2) as u8,
                face: (i % 3) as u8,
                hair: (i % 4) as u8,
                equip: SETS[i % SETS.len()],
            }
            .key()
        })
        .collect();
    let plan = |key: &SkinKey| {
        let worn: [Option<&benilla_formats::ItemDisplay>; 8] = std::array::from_fn(|i| {
            (key.equip[i] != 0)
                .then(|| items.get(key.equip[i]))
                .flatten()
        });
        sections.tables.composite_plan(
            key.race,
            key.sex,
            key.skin,
            key.face,
            key.facial_hair,
            key.hair_style,
            key.hair_color,
            worn,
            key.emblem,
            key.tabard_preview,
        )
    };
    let ms = |d: Duration| d.as_secs_f64() * 1e3;

    let mut images = Assets::<Image>::default();
    let mut lane = SkinComposites::default();
    let t = Instant::now();
    for key in &keys {
        lane.request(*key, &sections, || plan(key));
    }
    let arrival = t.elapsed();
    let (mut landing, mut worst, mut frames) = (Duration::ZERO, Duration::ZERO, 0);
    let wall = Instant::now();
    while lane.running() > 0 {
        std::thread::sleep(Duration::from_millis(1));
        let t = Instant::now();
        if lane.land(&mut images) > 0 {
            frames += 1;
        }
        let d = t.elapsed();
        landing += d;
        worst = worst.max(d);
    }
    println!(
        "LANE arrival frame: {:.3} ms for {} looks; landing {:.3} ms over {frames} landing frames \
         (worst {:.3} ms); all landed {:.1} ms after arrival",
        ms(arrival),
        keys.len(),
        ms(landing),
        ms(worst),
        ms(wall.elapsed())
    );

    let mut sync = SkinComposites::default();
    let t = Instant::now();
    for key in &keys {
        sync.force(*key, &sections, || plan(key), &mut images);
    }
    println!(
        "LANE the same crowd composited on this thread: {:.1} ms",
        ms(t.elapsed())
    );
}
