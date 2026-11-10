//! Hero Defense: vlastni modely ve formatu glTF (GLB) misto M2 z klienta.
//!
//! Server posila jednotky s beznym display id, ktere klient zna (napr. hrad 90000 chodi jako
//! Siege Golem). Pro jednotky vyjmenovane v [`MODELS`] se puvodni model postavi jako obvykle
//! (zustava kvuli klikani a kolizim), jen se schova, a pres nej se vykresli GLB.
//!
//! Modely se ctou ze zdroje `hd://`: slozka z promenne prostredi `HD_ASSETS`, jinak `hd_assets`
//! vedle spustitelneho souboru.

use bevy::asset::io::AssetSourceBuilder;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;

use crate::entities::VisualAttached;
use crate::net::{Guid, NetEntity};

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
}

const MODELS: &[CustomModel] = &[CustomModel {
    entry: 90000, // Hrad
    path: "castle/castle.glb",
    scale: 13.0, // model je ~1.9 jednotky siroky, hrad tak ma kolem 25 yardu
    yaw: 0.0,
}];

/// Jednotka, ktere uz byl vlastni model pridan.
#[derive(Component)]
struct HdModelSwapped;

/// Dite jednotky s vlastnim modelem (to jedine se neschovava).
#[derive(Component)]
struct HdModelRoot;

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
    app.add_systems(Update, (forget_rebuilt, swap_models, hide_original).chain());
}

/// Jednotka, jejiz vizual se prestavuje (zmena display id), dostane vlastni model znovu.
fn forget_rebuilt(mut commands: Commands, mut removed: RemovedComponents<VisualAttached>) {
    for entity in removed.read() {
        if let Ok(mut e) = commands.get_entity(entity) {
            e.remove::<HdModelSwapped>();
        }
    }
}

fn swap_models(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    units: Query<(Entity, &Guid, &NetEntity), (With<VisualAttached>, Without<HdModelSwapped>)>,
) {
    for (entity, guid, net) in &units {
        let Some(entry) = benilla_protocol::guid::entry(guid.0) else {
            continue;
        };
        let Some(model) = MODELS.iter().find(|m| m.entry == entry) else {
            continue;
        };

        // Rodic uz nese scale jednotky ze serveru, model ho vyrovna na pozadovanou velikost.
        let scale = model.scale / net.scale.max(0.01);
        let scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(format!("{HD_SOURCE}://{}", model.path)));
        commands.entity(entity).insert(HdModelSwapped).with_children(|parent| {
            parent.spawn((
                SceneRoot(scene),
                Transform::from_rotation(Quat::from_rotation_y(model.yaw)).with_scale(Vec3::splat(scale)),
                HdModelRoot,
            ));
        });
        info!("hero defense: jednotka {entry} vykreslena modelem {}", model.path);
    }
}

/// Puvodni model schova kazdy snimek: jine systemy (prolinani, LOD) mohou viditelnost vratit.
fn hide_original(
    units: Query<&Children, With<HdModelSwapped>>,
    mut parts: Query<&mut Visibility, Without<HdModelRoot>>,
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
