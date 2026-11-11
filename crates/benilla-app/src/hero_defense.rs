//! Hero Defense: vlastni modely ve formatu glTF (GLB) misto M2 z klienta.
//!
//! Server posila jednotky s beznym display id, ktere klient zna (napr. hrad 90000 chodi jako
//! Siege Golem). Pro jednotky vyjmenovane v [`MODELS`] se puvodni model postavi jako obvykle
//! (zustava kvuli klikani a kolizim), jen se schova, a pres nej se vykresli GLB.
//!
//! Modely se ctou ze zdroje `hd://`: slozka z promenne prostredi `HD_ASSETS`, jinak `hd_assets`
//! vedle spustitelneho souboru.
//!
//! Ctyri veci, ktere puvodni M2 dostava od klienta zadarmo a GLB ne:
//! - Svetlo: materialy z glTF se prevedou na material modelu WoW (`WowModelMaterial`), takze je
//!   osvetluje slunce, okolni svetlo, mlha a den/noc stejne jako budovy a objekty mapy.
//! - Orezavani: jednotka se kresli, jen kdyz je v zaberu jeji obrys ([`ModelBound`]), ktery by
//!   jinak byl obrys schovaneho M2.
//! - Stin pod jednotkou: velikost podle vlastniho modelu ([`BlobShadowBox`]), ne podle M2.
//! - Kolize: jednotky ve WoW kolize nemaji, vlastni model dostane pevny valec.
//!
//! Znazorneni aren na zemi je v [`grid`].

use avian3d::prelude::{Collider, RigidBody};
use bevy::asset::io::AssetSourceBuilder;
use bevy::camera::primitives::Aabb;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_formats::{FogPolicy, ModelBlend};
use benilla_world::lighting::SharedLightBuffer;
use benilla_world::model_render::{model_material, MaterialCache, ShadeSel};

use crate::blob_shadow::BlobShadowBox;
use crate::entities::{ModelBound, VisualAttached};
use crate::net::{Guid, NetEntity};

mod grid;

/// Zdroj assetu s modely Hero Defense.
const HD_SOURCE: &str = "hd";

/// Jednotka, ktera se vykresli vlastnim modelem.
struct CustomModel {
    /// `creature_template.entry` na serveru.
    entry: u32,
    /// Cesta k GLB uvnitr zdroje `hd://`.
    path: &'static str,
    /// Vysledna velikost modelu ve svete (nezavisle na scale jednotky ze serveru).
    scale: f32,
    /// Natoceni modelu kolem svisle osy vuci smeru jednotky, v radianech.
    yaw: f32,
    /// Obrys modelu v jeho vlastnich jednotkach (pred `scale`): polovina sirky a vyska.
    /// Pocatek modelu je uprostred zakladny.
    half_width: f32,
    height: f32,
    /// Polomer kolizniho valce v jednotkach modelu; `None` = bez kolize.
    collision_radius: Option<f32>,
}

const MODELS: &[CustomModel] = &[
    CustomModel {
        entry: 90000, // Hrad
        path: "castle/castle.glb",
        scale: 13.0, // model je ~1.9 jednotky siroky, hrad tak ma kolem 25 yardu
        yaw: 0.0,
        half_width: 0.95,
        height: 1.9,
        collision_radius: Some(0.7), // kousek uvnitr obrysu, at se o hroty nezasekava
    },
    // Veze 90010 az 90018 (server hd_defs.h HD_TOWERS). Kazda stoji na jednom poli mrizky
    // 4 x 4 yd: velikost tak, aby byla nejvys 3.8 yd siroka a 7 yd vysoka.
    tower(90010, "towers/human_cannon.glb", 2.0, 0.945, 1.588),
    tower(90011, "towers/human_holy_spire.glb", 3.68, 0.368, 1.902),
    tower(90012, "towers/human_watchtower.glb", 3.68, 0.355, 1.901),
    tower(90013, "towers/nelf_glaive.glb", 2.0, 0.944, 1.895),
    tower(90014, "towers/nelf_moonwell.glb", 3.1, 0.611, 1.905),
    tower(90015, "towers/nelf_sentinel.glb", 3.1, 0.608, 1.903),
    tower(90016, "towers/orc_totem.glb", 2.8, 0.675, 1.903),
    tower(90017, "towers/orc_catapult.glb", 2.0, 0.951, 1.59),
    tower(90018, "towers/orc_watchtower.glb", 3.68, 0.433, 1.901),
];

/// Vez: obrys a kolize podle modelu (kolize kousek uvnitr obrysu).
const fn tower(entry: u32, path: &'static str, scale: f32, half_width: f32, height: f32) -> CustomModel {
    CustomModel {
        entry,
        path,
        scale,
        yaw: 0.0,
        half_width,
        height,
        collision_radius: Some(half_width * 0.85),
    }
}

/// Jednotka, ktere uz byl vlastni model pridan; nese index do [`MODELS`].
#[derive(Component)]
struct HdModelSwapped(usize);

/// Dite jednotky s vlastnim modelem (to jedine se neschovava).
#[derive(Component)]
struct HdModelRoot;

/// Dite jednotky s kolizi vlastniho modelu (take se neschovava).
#[derive(Component)]
struct HdModelCollider;

/// Zaregistruje zdroj `hd://`. Musi probehnout pred `AssetPlugin` (v `DefaultPlugins`).
pub(crate) fn register_source(app: &mut App) {
    let dir = std::env::var("HD_ASSETS").unwrap_or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join("hd_assets")))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "hd_assets".into())
    });
    info!("hero defense: modely z {dir}");
    app.register_asset_source(HD_SOURCE, AssetSourceBuilder::platform_default(&dir, None));
}

pub(crate) fn plugin(app: &mut App) {
    grid::plugin(app);
    app.add_systems(
        Update,
        (forget_rebuilt, swap_models, hide_original, keep_bound, wow_lit_materials).chain(),
    );
}

/// Jednotka, jejiz vizual se prestavuje (zmena display id), dostane vlastni model znovu.
fn forget_rebuilt(mut commands: Commands, mut removed: RemovedComponents<VisualAttached>) {
    for entity in removed.read() {
        if let Ok(mut e) = commands.get_entity(entity) {
            e.remove::<HdModelSwapped>();
        }
    }
}

/// Pomer mezi jednotkami modelu a mistnimi jednotkami jednotky: rodic uz nese scale ze serveru,
/// model ho vyrovna na pozadovanou velikost.
fn local_scale(model: &CustomModel, net: &NetEntity) -> f32 {
    model.scale / net.scale.max(0.01)
}

fn swap_models(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    units: Query<(Entity, &Guid, &NetEntity), (With<VisualAttached>, Without<HdModelSwapped>)>,
    mut collider_epoch: ResMut<benilla_world::collision::ColliderEpoch>,
) {
    for (entity, guid, net) in &units {
        let Some(entry) = benilla_protocol::guid::entry(guid.0) else {
            continue;
        };
        let Some(index) = MODELS.iter().position(|m| m.entry == entry) else {
            continue;
        };
        let model = &MODELS[index];

        let scale = local_scale(model, net);
        let scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(format!("{HD_SOURCE}://{}", model.path)));
        commands.entity(entity).insert(HdModelSwapped(index)).with_children(|parent| {
            parent.spawn((
                SceneRoot(scene),
                Transform::from_rotation(Quat::from_rotation_y(model.yaw)).with_scale(Vec3::splat(scale)),
                HdModelRoot,
            ));
            if let Some(radius) = model.collision_radius {
                let height = model.height * scale;
                parent.spawn((
                    RigidBody::Static,
                    Collider::cylinder(radius * scale, height),
                    Transform::from_xyz(0.0, height / 2.0, 0.0),
                    HdModelCollider,
                ));
            }
        });
        if model.collision_radius.is_some() {
            // Mimo frontu stavby kolizi, takze sadu kolideru oznaci sam (jako GameObject v attach).
            collider_epoch.bump();
        }
        info!("hero defense: jednotka {entry} vykreslena modelem {}", model.path);
    }
}

/// Puvodni model schova kazdy snimek: jine systemy (prolinani, LOD) mohou viditelnost vratit.
fn hide_original(
    units: Query<&Children, With<HdModelSwapped>>,
    mut parts: Query<&mut Visibility, (Without<HdModelRoot>, Without<HdModelCollider>)>,
) {
    for children in &units {
        for child in children.iter() {
            if let Ok(mut visibility) = parts.get_mut(child) {
                if *visibility != Visibility::Hidden {
                    *visibility = Visibility::Hidden;
                }
            }
        }
    }
}

/// Obrys pro orezavani a stin pod jednotkou podle vlastniho modelu, ne podle schovaneho M2.
/// Prepisuje se, kdykoli ho klient vrati na obrys M2 (pri stavbe vizualu).
fn keep_bound(
    mut commands: Commands,
    units: Query<(
        Entity,
        &HdModelSwapped,
        &NetEntity,
        Option<&ModelBound>,
        Option<&BlobShadowBox>,
    )>,
) {
    for (entity, swapped, net, current, shadow) in &units {
        let model = &MODELS[swapped.0];
        let s = local_scale(model, net);
        let (min, max) = (
            Vec3::new(-model.half_width * s, 0.0, -model.half_width * s),
            Vec3::new(model.half_width * s, model.height * s, model.half_width * s),
        );
        let want = Aabb::from_min_max(min, max);
        if current.is_none_or(|b| b.0 != want) {
            commands.entity(entity).insert(ModelBound(want));
        }
        let want_shadow = BlobShadowBox { min, max };
        if shadow != Some(&want_shadow) {
            commands.entity(entity).insert(want_shadow);
        }
    }
}

/// Materialy vlastnich modelu prevede na material modelu WoW (`WowModelMaterial`): slunce,
/// okolni svetlo, mlha a den/noc jako u budov a objektu mapy (stinovani `Matte`). Textura zustava
/// z glTF. Ceka, nez je sdileny buffer svetla k dispozici.
fn wow_lit_materials(
    mut commands: Commands,
    meshes: Query<(Entity, &MeshMaterial3d<StandardMaterial>)>,
    parents: Query<&ChildOf>,
    roots: Query<(), With<HdModelRoot>>,
    standard: Res<Assets<StandardMaterial>>,
    mut wow: ResMut<Assets<WowModelMaterial>>,
    mut images: ResMut<Assets<Image>>,
    light: Option<Res<SharedLightBuffer>>,
    mut cache: Local<MaterialCache>,
) {
    let Some(light) = light else {
        return;
    };
    for (entity, handle) in &meshes {
        if !parents.iter_ancestors(entity).any(|a| roots.contains(a)) {
            continue;
        }
        let texture = standard
            .get(&handle.0)
            .and_then(|m| m.base_color_texture.clone());
        // Shader modelu WoW pocita v gamma prostoru a textury WoW nahrava jako surove bajty
        // (`Rgba8Unorm`). glTF nahrava zakladni barvu jako sRGB, ktere GPU pri cteni prevede do
        // linearniho prostoru, a model by vysel o hodne tmavsi. Stejna data, jen bez prevodu.
        if let Some(image) = texture.as_ref().and_then(|t| images.get_mut(t)) {
            let format = image.texture_descriptor.format;
            if format.is_srgb() {
                image.texture_descriptor.format = format.remove_srgb_suffix();
                image.texture_view_descriptor = None;
            }
        }
        let material = model_material(
            &mut cache,
            &mut wow,
            texture,
            ModelBlend::Opaque,
            false, // two_sided
            false, // is_wmo
            false, // is_interior
            false, // is_emissive
            false, // is_additive
            false, // fade_variant
            false, // no_depth_write
            false, // no_depth_test
            FogPolicy::Scene,
            false, // env_map
            None,  // stage1
            None,  // combine
            ShadeSel::Matte,
            0,    // batch_order
            None, // order
            None, // uv_anim
            None, // rgb_anim
            None, // wmo_class
            None, // sidn
            false, // window
            false, // sky_depth
            &light.0,
            None, // instance
        );
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert(MeshMaterial3d(material));
    }
}
