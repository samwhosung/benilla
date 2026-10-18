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
//! with each `SetModel` instance; the pane's showing resets the glue glow (`0x46fa60`). A hidden
//! pane keeps its scene with its clock stopped (`0x76d7f0`), so a screen with none (the credits)
//! leaves the last one standing hidden, and its return resumes it. A pane hidden behind another's
//! keeps its scene too (the login's `UI_MainMenu` has no `SetModel` but its `file=`): each (pane,
//! instance) keeps one stage number while its pane holds it, and the booth parks it meanwhile.

use bevy::prelude::*;

use benilla_ui::script::{GlueScenePane, UiScript, VmKind};

use super::glue_mgr::SharedGlueMgr;
use crate::portrait::{
    CreateLook, GlueLook, GluePreview, GlueSceneFile, SceneFog, SceneRoot, SelectLook,
};

/// The create scene's character and facing, as the create verbs left them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CreateStage {
    look: Option<benilla_ui::script::GlueCreateLook>,
    facing: f32,
}

/// What the booth stands while the stock glue screens are up: `pane` is the shown `ModelFFX`'s
/// scene, `standing` the select character on its attachment 0, `facing` the select facing cell in
/// radians, `create` the create scene's. The scene is the pane's file, whatever it names: slot 37
/// (`0x76cce0`) loads any non-empty name, and an empty one leaves the pane no model.
fn stock_preview(
    pane: Option<&GlueScenePane>,
    standing: Option<&benilla_protocol::Character>,
    facing: f32,
    create: CreateStage,
) -> (
    Option<GlueSceneFile>,
    Option<GlueLook>,
    f32,
    Option<SceneFog>,
) {
    let Some(pane) = pane.filter(|p| !p.path.is_empty()) else {
        return (None, None, 0.0, None);
    };
    let scene = Some(GlueSceneFile::new(&pane.path));
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

/// What the feeder remembers about the glue VM between frames: the pane shown last, the pane and
/// instance the scene stands, and the number each (pane, instance) took in [`GluePreview::stage`].
#[derive(Default)]
pub(crate) struct StockFeed {
    shown: Option<benilla_ui::widget::FrameHandle>,
    instance: Option<(benilla_ui::widget::FrameHandle, u64)>,
    /// Every scene a pane still holds, by its (pane, instance), with its stage number.
    stages: Vec<((benilla_ui::widget::FrameHandle, u64), u64)>,
}

/// Feed the glue booth from the glue VM while the stock glue screens are up, and stand it down
/// once they are not (the world entered, the VM gone).
pub(crate) fn feed_stock_glue_preview(
    script: Option<NonSend<UiScript>>,
    mgr: NonSend<SharedGlueMgr>,
    state: Option<Res<State<crate::client_state::ClientState>>>,
    mut preview: ResMut<GluePreview>,
    mut feed: Local<super::VmMemo<StockFeed>>,
    // The highest stage number given out, kept across VMs: a new VM's first scene must not take a
    // number a scene parked under the old one still holds.
    mut top: Local<u64>,
) {
    let in_world = state.is_some_and(|s| *s.get() == crate::client_state::ClientState::InWorld);
    let script = script.filter(|s| s.vm_kind() == VmKind::Glue && !in_world);
    // A new VM's panes are new frames: the memo starts over with it.
    let feed = feed.get_for(script.as_deref());
    let visible = script.as_deref().and_then(|s| s.glue_scene());
    // With none shown, the pane last stood keeps its scene, hidden: a `ModelFFX` holds its model
    // and scene while hidden, and only its clock stops (`0x76d7f0`, called by `0x765650`).
    let hidden = match (&visible, script.as_deref(), feed.instance) {
        (None, Some(s), Some((h, _))) => s.glue_pane(h),
        _ => None,
    };
    let is_hidden = hidden.is_some();
    let shown = visible.as_ref().map(|p| p.handle);
    let pane = visible.or(hidden);
    let mgr = mgr.get();
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
    // A pane that was not the one shown last frame has just shown: its OnShow (`0x46fa60`).
    if shown.is_some() && shown != feed.shown {
        preview.shown = preview.shown.wrapping_add(1);
    }
    feed.shown = shown;
    // A new instance, or another pane's, is another scene; a hidden pane's own is the same one.
    // Each (pane, instance) keeps one number, so a pane shown again on the instance it held finds
    // its scene as it left it: a hidden `ModelFFX` keeps model, scene and stopped clock.
    let instance = pane.as_ref().map(|p| (p.handle, p.instance));
    if let Some(key) = instance.filter(|_| instance != feed.instance) {
        feed.instance = instance;
        let id = match feed.stages.iter().find(|(k, _)| *k == key) {
            Some(&(_, id)) => id,
            None => {
                *top = top.max(preview.stage).wrapping_add(1);
                feed.stages.push((key, *top));
                *top
            }
        };
        preview.stage = id;
    }
    // A pane that took a new model let its old instance go (`0x76cce0`), and its scene with it.
    match script.as_deref() {
        Some(s) => feed
            .stages
            .retain(|&((h, inst), _)| s.glue_pane(h).is_some_and(|p| p.instance == inst)),
        None => feed.stages.clear(),
    }
    let live: Vec<u64> = feed.stages.iter().map(|&(_, id)| id).collect();
    if preview.live != live {
        preview.live = live;
    }
    if preview.hidden != is_hidden {
        preview.hidden = is_hidden;
    }
    let rect = pane.as_ref().and_then(|p| p.rect);
    if preview.pane != rect {
        preview.pane = rect;
    }
    let root = pane.as_ref().map_or(SceneRoot::default(), |p| SceneRoot {
        model_scale: p.model_scale,
        layout_scale: p.layout_scale,
        position: Vec3::from_array(p.position),
        facing: p.facing,
    });
    if preview.root != root {
        preview.root = root;
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
        let c = crate::glue_session::roster::test_character(7, "Seven");
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
        assert_eq!(scene, Some(file("UI_MainMenu")));
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
        let c = crate::glue_session::roster::test_character(7, "Seven");
        let (scene, look, yaw, _) = stock_preview(
            Some(&p),
            Some(&c),
            1.0,
            CreateStage {
                look: Some(built),
                facing: -0.25,
            },
        );
        assert_eq!(scene, Some(file("UI_Orc")));
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

    /// The stock folder's scene file `UI_<name>`.
    fn file(name: &str) -> GlueSceneFile {
        GlueSceneFile::new(&format!("Interface\\Glues\\Models\\{name}\\{name}.mdx"))
    }

    /// The same file spelled with `/`, another key of the cache (`0x706b35`-`0x706b8a`).
    fn slashed(name: &str) -> GlueSceneFile {
        GlueSceneFile::new(&format!("Interface/Glues/Models/{name}/{name}.mdx"))
    }

    /// No shown pane, or one whose `SetModel` name was empty, stands nothing; any other file
    /// stands as itself (`0x76cce0`), a scene 1.12.1 never shipped included.
    #[test]
    fn a_pane_stands_whatever_file_it_holds() {
        let none = CreateStage::default();
        assert_eq!(stock_preview(None, None, 0.0, none).0, None);
        assert_eq!(
            stock_preview(Some(&pane("", true, None)), None, 0.0, none).0,
            None
        );
        let gnome = "Interface\\Glues\\Models\\UI_Gnome\\UI_Gnome.mdx";
        let (scene, ..) = stock_preview(Some(&pane(gnome, true, None)), None, 0.0, none);
        assert_eq!(scene.as_ref().map(GlueSceneFile::path), Some(gnome));
        let custom = "Interface\\Glues\\Models\\Custom\\UI_Orc.mdx";
        let (scene, ..) = stock_preview(Some(&pane(custom, true, None)), None, 0.0, none);
        assert_eq!(scene, Some(GlueSceneFile::new(custom)));
        assert_ne!(
            scene,
            Some(file("UI_Orc")),
            "its own file, not the stock UI_Orc"
        );
    }

    /// Through the verbs: `SetCharSelectBackground` stands its file unfogged whatever the script
    /// sets after (`0x472150` replaces the fog callback), and `SetCharCustomizeBackground` keeps
    /// the pane's fog (`0x4707b0`), none after `ClearFog`.
    #[test]
    fn the_background_verbs_stand_any_file_under_their_screens_fog() {
        let s = UiScript::new_glue().unwrap();
        s.run(
            "CreateFrame('ModelFFX', 'Select') SetCharSelectModelFrame('Select') \
             SetCharSelectBackground('Interface\\\\Glues\\\\Models\\\\UI_Goblin\\\\UI_Goblin.mdx') \
             Select:SetFogColor(1, 0, 0) Select:SetFogFar(100)",
        )
        .unwrap();
        let none = CreateStage::default();
        let (scene, _, _, fog) = stock_preview(s.glue_scene().as_ref(), None, 0.0, none);
        assert_eq!(scene, Some(file("UI_Goblin")));
        assert_eq!(fog, None, "the select scene never fogs");

        s.run(
            "Select:Hide() CreateFrame('ModelFFX', 'Create') SetCharCustomizeFrame('Create') \
             SetCharCustomizeBackground('Custom\\\\Stage.mdx') \
             Create:SetFogColor(1, 0, 0) Create:SetFogFar(100)",
        )
        .unwrap();
        let (scene, _, _, fog) = stock_preview(s.glue_scene().as_ref(), None, 0.0, none);
        assert_eq!(scene, Some(GlueSceneFile::new("Custom\\Stage.mdx")));
        assert_eq!(
            fog.map(|f| f.far),
            Some(100.0),
            "the create scene keeps its fog"
        );
        s.run("Create:ClearFog()").unwrap();
        let (.., fog) = stock_preview(s.glue_scene().as_ref(), None, 0.0, none);
        assert_eq!(fog, None);
    }

    /// `SetGlueScreen("credits")` hides `AccountLogin` and shows no model (`GlueParent.lua:34-45`):
    /// the login pane keeps its model and scene while hidden, so the booth keeps standing it,
    /// hidden; the return shows the same instance, which fires its OnShow (`0x46fa60`) again.
    #[test]
    fn a_hidden_pane_keeps_its_scene_and_its_return_is_the_same_scene() {
        let mut app = App::new();
        let s = UiScript::new_glue().unwrap();
        s.run(
            "CreateFrame('ModelFFX', 'Login')\
             :SetModel('Interface/Glues/Models/UI_MainMenu/UI_MainMenu.mdx')",
        )
        .unwrap();
        app.insert_non_send_resource(s)
            .insert_non_send_resource(SharedGlueMgr::default())
            .init_resource::<GluePreview>()
            .add_systems(Update, feed_stock_glue_preview);
        let run = |app: &mut App, chunk: &str| {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.run(chunk).unwrap();
            s.resolve();
            app.update();
            let p = app.world().resource::<GluePreview>();
            (p.scene.clone(), p.hidden, p.stage, p.shown)
        };
        let (scene, hidden, stage, shown) = run(&mut app, "");
        assert_eq!((scene, hidden), (Some(slashed("UI_MainMenu")), false));

        assert_eq!(
            run(&mut app, "Login:Hide()"),
            (Some(slashed("UI_MainMenu")), true, stage, shown),
            "hidden, the scene stays up: no teardown, no new instance"
        );
        assert_eq!(
            run(&mut app, "Login:Show()"),
            (Some(slashed("UI_MainMenu")), false, stage, shown + 1),
            "shown again, the same instance resumes, and its OnShow pins the glow"
        );

        // A new `SetModel` instance while hidden is a new scene when it shows.
        run(
            &mut app,
            "Login:Hide() Login:SetModel('Interface/Glues/Models/UI_Orc/UI_Orc.mdx')",
        );
        let (scene, hidden, again, _) = run(&mut app, "Login:Show()");
        assert_eq!((scene, hidden), (Some(slashed("UI_Orc")), false));
        assert_ne!(again, stage);
    }

    /// Each (pane, instance) keeps its stage number while its pane holds it, so a pane shown again
    /// on the instance it held brings that number back; a new `SetModel` there lets it go.
    #[test]
    fn a_pane_shown_again_on_its_instance_brings_its_stage_back() {
        let mut app = App::new();
        let s = UiScript::new_glue().unwrap();
        s.run(
            "CreateFrame('ModelFFX', 'Login')\
             :SetModel('Interface/Glues/Models/UI_MainMenu/UI_MainMenu.mdx') \
             CreateFrame('ModelFFX', 'Select')\
             :SetModel('Interface/Glues/Models/UI_Orc/UI_Orc.mdx') Select:Hide()",
        )
        .unwrap();
        app.insert_non_send_resource(s)
            .insert_non_send_resource(SharedGlueMgr::default())
            .init_resource::<GluePreview>()
            .add_systems(Update, feed_stock_glue_preview);
        let run = |app: &mut App, chunk: &str| {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.run(chunk).unwrap();
            s.resolve();
            app.update();
            let p = app.world().resource::<GluePreview>();
            (p.scene.clone(), p.stage, p.live.clone())
        };
        let (scene, login, live) = run(&mut app, "");
        assert_eq!((scene, live), (Some(slashed("UI_MainMenu")), vec![login]));

        let (scene, select, live) = run(&mut app, "Login:Hide() Select:Show()");
        assert_eq!(scene, Some(slashed("UI_Orc")));
        assert_ne!(select, login);
        assert_eq!(
            live,
            vec![login, select],
            "the hidden login pane still holds its scene"
        );

        let (scene, back, live) = run(&mut app, "Select:Hide() Login:Show()");
        assert_eq!(
            (scene, back),
            (Some(slashed("UI_MainMenu")), login),
            "the same instance, the same stage"
        );
        assert_eq!(live, vec![login, select]);

        let (_, _, live) = run(
            &mut app,
            "Select:SetModel('Interface/Glues/Models/UI_Human/UI_Human.mdx')",
        );
        assert_eq!(live, vec![login], "the select pane let its old scene go");
    }

    /// A new VM's panes are new frames, but their scenes' numbers must not repeat one the old VM
    /// gave out: a scene parked under it would stand in for the new VM's first.
    #[test]
    fn a_new_vm_never_reuses_a_stage_number() {
        let mut app = App::new();
        let vm = || {
            let s = UiScript::new_glue().unwrap();
            s.run(
                "CreateFrame('ModelFFX', 'Login')\
                 :SetModel('Interface/Glues/Models/UI_MainMenu/UI_MainMenu.mdx') \
                 CreateFrame('ModelFFX', 'Select')\
                 :SetModel('Interface/Glues/Models/UI_Orc/UI_Orc.mdx') Select:Hide()",
            )
            .unwrap();
            s
        };
        app.insert_non_send_resource(vm())
            .insert_non_send_resource(SharedGlueMgr::default())
            .init_resource::<GluePreview>()
            .add_systems(Update, feed_stock_glue_preview);
        let run = |app: &mut App, chunk: &str| {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.run(chunk).unwrap();
            s.resolve();
            app.update();
            app.world().resource::<GluePreview>().stage
        };
        let login = run(&mut app, "");
        let select = run(&mut app, "Login:Hide() Select:Show()");
        assert_eq!(run(&mut app, "Select:Hide() Login:Show()"), login);

        // A new VM: its login scene takes a number past every one given out.
        app.insert_non_send_resource(vm());
        let fresh = run(&mut app, "");
        assert!(fresh > select.max(login), "{fresh} after {login}, {select}");
    }
}
