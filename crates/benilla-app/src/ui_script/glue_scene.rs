//! The stock glue screens' 3-D scenes: the glue VM's shown `ModelFFX` pane drives the glue booth
//! ([`crate::portrait::GluePreview`]), which renders the scene, its light and fog, the selected
//! character on attachment 0 and its pet on attachment 1; the pane then draws the booth's image at
//! its own bucket (the extract's `ModelPane` arm), so every GlueXML region paints over it.
//!
//! The scene is the pane's file: `AccountLogin.xml`'s `UI_MainMenu`, `CharacterSelect`'s OnLoad
//! `UI_Orc` (`CharacterSelect.lua:26`), or the stage `SetCharSelectBackground` gives it. Its fog is
//! the pane's own until `SetCharSelectBackground` swaps the fog fill callback for the select
//! screen's (`0x472110`), which never fogs. The character is the select record the last
//! `SelectCharacter` built and attached (`0x472950`), turned by the facing cell (`[0xb4217c]`,
//! `SetCharacterSelectFacing`).

use bevy::prelude::*;

use benilla_ui::script::{GlueScenePane, UiScript, VmKind};

use super::glue_mgr::GlueMgr;
use crate::portrait::{GlueLook, GluePreview, GlueScene, SceneFog, SelectLook};

/// What the booth stands while the stock glue screens are up: `pane` is the shown `ModelFFX`'s
/// scene, `standing` the character on its attachment 0, `facing` the select facing cell in
/// radians. A file that names none of the shipped scenes stands nothing.
fn stock_preview(
    pane: Option<&GlueScenePane>,
    standing: Option<&benilla_protocol::Character>,
    facing: f32,
) -> (Option<GlueScene>, Option<GlueLook>, f32, Option<SceneFog>) {
    let Some(pane) = pane else {
        return (None, None, 0.0, None);
    };
    let scene = GlueScene::from_model_path(&pane.path);
    if scene.is_none() {
        warn_once!(
            "glue scene: {} is none of the shipped glue scenes; the glue booth stands nothing",
            pane.path
        );
    }
    // The character and the facing are the select frame's scene's alone (`[0xb42174]`).
    let (look, yaw) = if pane.select {
        (
            standing.map(|c| GlueLook::Select(SelectLook::from(c))),
            facing,
        )
    } else {
        (None, 0.0)
    };
    let fog = pane.fog.map(|f| SceneFog {
        rgb: f.rgb(),
        near: f.near,
        far: f.far,
    });
    (scene, look, yaw, fog)
}

/// Feed the glue booth from the glue VM while the stock glue screens are up, and stand it down
/// once they are not (the world entered, the VM gone).
pub(crate) fn feed_stock_glue_preview(
    script: Option<NonSend<UiScript>>,
    mgr: Res<GlueMgr>,
    state: Option<Res<State<crate::char_select::ClientState>>>,
    mut preview: ResMut<GluePreview>,
) {
    let in_world = state.is_some_and(|s| *s.get() == crate::char_select::ClientState::InWorld);
    let script = script.filter(|s| s.vm_kind() == VmKind::Glue && !in_world);
    let (scene, look, yaw, fog) = match script.as_deref() {
        Some(s) => {
            let pane = s.glue_scene();
            let standing = s.glue_standing_guid().and_then(|g| mgr.roster_character(g));
            stock_preview(pane.as_ref(), standing, s.glue_select_cells().facing)
        }
        None => (None, None, 0.0, None),
    };
    if preview.scene != scene {
        preview.scene = scene;
    }
    if preview.look != look {
        preview.look = look;
    }
    if preview.yaw != yaw {
        preview.yaw = yaw;
    }
    if preview.fog != fog {
        preview.fog = fog;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(path: &str, select: bool, fog: Option<benilla_ui::widget::ModelFog>) -> GlueScenePane {
        GlueScenePane {
            path: path.into(),
            fog,
            select,
        }
    }

    /// The login's pane: `UI_MainMenu` under `AccountLogin.xml`'s fog, nobody on it.
    #[test]
    fn the_main_menu_pane_stands_its_scene_under_its_own_fog() {
        let fog = benilla_ui::widget::ModelFog {
            color: 0xff40_0f04,
            near: 0.0,
            far: 1200.0,
        };
        let c = crate::char_select::test_character(7, "Seven");
        let (scene, look, yaw, got) = stock_preview(
            Some(&pane(
                "Interface\\Glues\\Models\\UI_MainMenu\\UI_MainMenu.mdx",
                false,
                Some(fog),
            )),
            Some(&c),
            1.0,
        );
        assert_eq!(scene, Some(GlueScene::MainMenu));
        assert!(
            look.is_none() && yaw == 0.0,
            "only the select frame's scene holds a character"
        );
        let got = got.expect("fogged");
        assert_eq!((got.near, got.far), (0.0, 1200.0));
        assert_eq!(got.rgb, fog.rgb());
    }

    #[test]
    fn no_shown_pane_and_an_unknown_file_stand_nothing() {
        assert_eq!(stock_preview(None, None, 0.0).0, None);
        assert_eq!(
            stock_preview(
                Some(&pane("Interface\\Glues\\Models\\Odd.mdx", true, None)),
                None,
                0.0
            )
            .0,
            None
        );
    }
}
