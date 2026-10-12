//! The stock glue screens' 3-D scenes: the glue VM's shown `ModelFFX` pane drives the glue booth
//! ([`crate::portrait::GluePreview`]), which renders the scene, its light and fog, the selected
//! character on attachment 0 and its pet on attachment 1; the pane then draws the booth's image at
//! its own bucket (the extract's `ModelPane` arm), so every GlueXML region paints over it.
//!
//! The scene is the pane's file: `AccountLogin.xml`'s `UI_MainMenu`, `CharacterSelect`'s OnLoad
//! `UI_Orc` (`CharacterSelect.lua:26`), or the stage `SetCharSelectBackground` or
//! `SetCharCustomizeBackground` gives it. Its fog is the pane's own until `SetCharSelectBackground`
//! swaps the fog fill callback for the select screen's (`0x472110`), which never fogs; the create
//! screen's keeps the fog (`0x4707b0`). On the select scene the character is the record the last
//! `SelectCharacter` built and attached (`0x472950`), turned by the facing cell (`[0xb4217c]`,
//! `SetCharacterSelectFacing`); on the create scene it is the preview character the create verbs
//! last built (`0x470b70`), dressed in its class's outfit (`0x470800`) and turned by the create
//! facing (`[0xb420f4]`, `SetCharacterCreateFacing`).
//!
//! The scene fills the pane at the pane's own aspect, as the reference frames it, and starts over
//! with each `SetModel` instance; the pane's showing resets the glue glow (`0x46fa60`).

use bevy::prelude::*;

use benilla_ui::script::{GlueScenePane, UiScript, VmKind};

use super::glue_mgr::GlueMgr;
use crate::portrait::{CreateLook, GlueLook, GluePreview, GlueScene, SceneFog, SelectLook};

/// The create scene's character and facing, as the create verbs left them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CreateStage {
    look: Option<benilla_ui::script::GlueCreateLook>,
    facing: f32,
}

/// What the booth stands while the stock glue screens are up: `pane` is the shown `ModelFFX`'s
/// scene, `standing` the select character on its attachment 0, `facing` the select facing cell in
/// radians, `create` the create scene's. A file that names none of the shipped scenes stands
/// nothing.
fn stock_preview(
    pane: Option<&GlueScenePane>,
    standing: Option<&benilla_protocol::Character>,
    facing: f32,
    create: CreateStage,
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
    // The character and the facing are the select frame's scene's (`[0xb42174]`) or the
    // customize frame's (`[0xb420f0]`), each its own.
    let (look, yaw) = if pane.select {
        (
            standing.map(|c| GlueLook::Select(SelectLook::from(c))),
            facing,
        )
    } else if pane.create {
        (
            create.look.map(|l| {
                GlueLook::Create(CreateLook {
                    race: l.race,
                    sex: l.sex,
                    class: l.class,
                    skin: l.skin,
                    face: l.face,
                    hair_style: l.hair_style,
                    hair_color: l.hair_color,
                    facial_hair: l.facial_hair,
                })
            }),
            create.facing,
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

/// What the feeder remembers about the glue VM between frames: the pane shown last and the
/// instance it held, numbered into [`GluePreview::stage`].
#[derive(Default)]
pub(crate) struct StockFeed {
    shown: Option<benilla_ui::widget::FrameHandle>,
    instance: Option<(benilla_ui::widget::FrameHandle, u64)>,
}

/// Feed the glue booth from the glue VM while the stock glue screens are up, and stand it down
/// once they are not (the world entered, the VM gone).
pub(crate) fn feed_stock_glue_preview(
    script: Option<NonSend<UiScript>>,
    mgr: Res<GlueMgr>,
    state: Option<Res<State<crate::char_select::ClientState>>>,
    mut preview: ResMut<GluePreview>,
    mut feed: Local<super::VmMemo<StockFeed>>,
) {
    let in_world = state.is_some_and(|s| *s.get() == crate::char_select::ClientState::InWorld);
    let script = script.filter(|s| s.vm_kind() == VmKind::Glue && !in_world);
    let pane = script.as_deref().and_then(|s| s.glue_scene());
    let (scene, look, yaw, fog) = match script.as_deref() {
        Some(s) => {
            let standing = s.glue_standing_guid().and_then(|g| mgr.roster_character(g));
            let create = CreateStage {
                look: s.glue_create_look(),
                facing: s.glue_create_facing(),
            };
            stock_preview(
                pane.as_ref(),
                standing,
                s.glue_select_cells().facing,
                create,
            )
        }
        None => (None, None, 0.0, None),
    };
    // A new VM's panes are new frames: the memo starts over with it.
    let feed = feed.get_for(script.as_deref());
    // A pane that was not the one shown last frame has just shown: its OnShow (`0x46fa60`).
    let shown = pane.as_ref().map(|p| p.handle);
    if shown.is_some() && shown != feed.shown {
        preview.shown = preview.shown.wrapping_add(1);
    }
    feed.shown = shown;
    // A new instance, or another pane's, is another scene.
    let instance = pane.as_ref().map(|p| (p.handle, p.instance));
    if instance.is_some() && instance != feed.instance {
        feed.instance = instance;
        preview.stage = preview.stage.wrapping_add(1);
    }
    let rect = pane.as_ref().and_then(|p| p.rect);
    if preview.pane != rect {
        preview.pane = rect;
    }
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

    /// A shown pane's scene as a glue VM reads it, then given `path`, `select` and `fog`.
    fn pane(path: &str, select: bool, fog: Option<benilla_ui::widget::ModelFog>) -> GlueScenePane {
        let s = UiScript::new_glue().unwrap();
        s.run("CreateFrame('ModelFFX', 'Pane'):SetModel('x.mdx')")
            .unwrap();
        GlueScenePane {
            path: path.into(),
            fog,
            select,
            ..s.glue_scene().unwrap()
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
            CreateStage::default(),
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

    /// The stock login's pane is `GlueParent`'s rect, which fills the screen up to 16:9 and is a
    /// centred 16:9 box past it (`GlueParent.lua:78-84`): the rect the scene is framed in.
    #[test]
    fn the_stock_pane_is_glue_parents_rect_at_every_aspect() {
        let _ = benilla_formats::wow_data_or_skip!();
        for aspect in [4.0 / 3.0, 16.0 / 10.0, 16.0 / 9.0, 21.0 / 9.0] {
            let mut s = UiScript::new_glue().unwrap();
            s.set_screen_size(768.0 * aspect, 768.0);
            let _ = super::super::glue::load_glue(&mut s);
            super::super::glue::set_glue_screen(&mut s, super::super::glue::GlueScreen::Login);
            s.resolve();
            let pane = s.glue_scene().expect("AccountLogin is shown");
            assert!(pane.path.ends_with("UI_MainMenu.mdx"), "{}", pane.path);
            let bar = ((aspect - 16.0 / 9.0) / (2.0 * aspect)).max(0.0);
            let rect = pane.rect.expect("laid out");
            for (got, want) in rect.iter().zip([bar, 1.0 - bar, 0.0, 1.0]) {
                assert!((got - want).abs() < 1e-4, "{aspect}: {rect:?}");
            }
        }
    }

    /// The create pane stands the create verbs' character, dressed as its class, at the create
    /// facing; the select's character and facing are not its own. With none built the stage is
    /// empty.
    #[test]
    fn the_create_pane_stands_the_create_character_at_its_facing() {
        let built = benilla_ui::script::GlueCreateLook {
            race: 2,
            sex: 1,
            class: 3,
            skin: 1,
            face: 2,
            hair_style: 3,
            hair_color: 4,
            facial_hair: 0,
        };
        let mut p = pane("Interface\\Glues\\Models\\UI_Orc\\UI_Orc.mdx", false, None);
        p.create = true;
        let c = crate::char_select::test_character(7, "Seven");
        let (scene, look, yaw, _) = stock_preview(
            Some(&p),
            Some(&c),
            1.0,
            CreateStage {
                look: Some(built),
                facing: -0.25,
            },
        );
        assert_eq!(scene, Some(GlueScene::Orc));
        assert!(
            look == Some(GlueLook::Create(CreateLook {
                race: 2,
                sex: 1,
                class: 3,
                skin: 1,
                face: 2,
                hair_style: 3,
                hair_color: 4,
                facial_hair: 0,
            }))
        );
        assert_eq!(yaw, -0.25);
        let (_, look, _, _) = stock_preview(Some(&p), Some(&c), 1.0, CreateStage::default());
        assert!(look.is_none());
    }

    #[test]
    fn no_shown_pane_and_an_unknown_file_stand_nothing() {
        assert_eq!(
            stock_preview(None, None, 0.0, CreateStage::default()).0,
            None
        );
        assert_eq!(
            stock_preview(
                Some(&pane("Interface\\Glues\\Models\\Odd.mdx", true, None)),
                None,
                0.0,
                CreateStage::default()
            )
            .0,
            None
        );
    }
}
