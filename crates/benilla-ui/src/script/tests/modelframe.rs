//! The `Model` and `PlayerModel` Lua surface, read back through the API that wrote it, and checked
//! against the reference's method tables in both directions.

use super::common::script;
use crate::script::UiScript;

#[test]
fn the_model_pane_holds_the_scene_it_was_given() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MPane", UIParent)"#)
        .unwrap();

    // A fresh pane: no model, scale 1 (not 0, which would draw nothing), no yaw.
    assert_eq!(
        s.eval::<(Option<String>, f64, f64)>(
            "return MPane:GetModel(), MPane:GetModelScale(), MPane:GetFacing()"
        )
        .unwrap(),
        (None, 1.0, 0.0),
        "a fresh pane has no model and unit scale"
    );

    // The path round-trips verbatim, backslashes and `.mdx` intact (pfUI's autocast shine).
    s.run(r#"MPane:SetModel("Interface\\Buttons\\UI-AutoCastButton.mdx")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return MPane:GetModel()").unwrap(),
        r"Interface\Buttons\UI-AutoCastButton.mdx"
    );

    // `SetFacing` is the `Model` yaw verb (`0x878948[4]`).
    s.run("MPane:SetFacing(-0.25)").unwrap();
    assert_eq!(s.eval::<f64>("return MPane:GetFacing()").unwrap(), -0.25);

    s.run("MPane:SetModelScale(0.4) MPane:SetCamera(2) MPane:SetPosition(0.1, -0.2, 3)")
        .unwrap();
    assert_eq!(
        s.eval::<f64>("return MPane:GetModelScale()").unwrap(),
        0.4_f32 as f64
    );
    let (x, y, z): (f64, f64, f64) = s.eval("return MPane:GetPosition()").unwrap();
    assert_eq!(
        (x as f32, y as f32, z as f32),
        (0.1, -0.2, 3.0),
        "GetPosition returns the three numbers SetPosition took"
    );

    s.run("MPane:ClearModel()").unwrap();
    assert_eq!(
        s.eval::<Option<String>>("return MPane:GetModel()").unwrap(),
        None
    );
}

/// Each model-pane type has its own method table and reaches its base's through the miss leg of
/// `vtable+0x8` (`0x7020b0`): `PlayerModel`'s three (`0x84f1fc`) sit over `Model`'s 23
/// (`0x878948`), and `CSimpleModel`'s lookup `0x76f870` has no leg into `0x506260`.
#[test]
fn a_player_model_is_a_model_plus_three_and_the_chain_runs_one_way() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        pm = CreateFrame("PlayerModel", "PMPane", UIParent)
        m  = CreateFrame("Model", "MOnly", UIParent)
    "#,
    )
    .unwrap();

    for verb in ["SetUnit", "RefreshUnit", "SetRotation"] {
        assert_eq!(
            s.eval::<String>(&format!("return type(PMPane.{verb})"))
                .unwrap(),
            "function",
            "PlayerModel must answer its own {verb}"
        );
        assert_eq!(
            s.eval::<String>(&format!("return type(MOnly.{verb})"))
                .unwrap(),
            "nil",
            "a plain Model must NOT answer {verb} — the chain runs derived -> base only"
        );
    }

    // pfUI's portrait line: `SetUnit`, then the base's `SetCamera` through the chain.
    s.run(r#"PMPane:SetUnit("player") PMPane:SetCamera(0)"#)
        .unwrap();
    assert_eq!(
        s.eval::<Option<String>>("return PMPane:GetModel()")
            .unwrap(),
        None,
        "SetUnit displaces the model path — content is an either/or, not layers"
    );
    s.run(r#"PMPane:SetModel("Interface\\Buttons\\Other.mdx")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return PMPane:GetModel()").unwrap(),
        r"Interface\Buttons\Other.mdx",
        "...and back the other way, the direction an addon reskinning a paper doll takes"
    );

    // `SetRotation` (`0x505bb0`) and `SetFacing` (`0x76dce0`) write the same yaw field, `+0x39c`.
    s.run("PMPane:SetRotation(1.5)").unwrap();
    assert_eq!(s.eval::<f64>("return PMPane:GetFacing()").unwrap(), 1.5);
    s.run("PMPane:SetFacing(-0.25)").unwrap();
    assert_eq!(
        s.eval::<f64>("return PMPane:GetFacing()").unwrap(),
        -0.25,
        "one slot: SetFacing overwrites what SetRotation wrote"
    );

    // Stock `PaperDollFrame.xml:221` calls `RefreshUnit`, which rebuilds from the stored GUID.
    assert!(s.run("PMPane:RefreshUnit()").is_ok());

    // `ClearModel`, reached through the chain, empties both content slots.
    s.run(r#"PMPane:SetUnit("player") PMPane:ClearModel()"#)
        .unwrap();
    assert_eq!(
        s.eval::<Option<String>>("return PMPane:GetModel()")
            .unwrap(),
        None
    );
}

/// `SetModel` (`0x76d950`) and `ReplaceIconTexture` (`0x76ed70`) gate on `lua_isstring`, raising
/// their usage string on anything but a string or a number; `ClearModel` (`0x76db20`) is the clear.
#[test]
fn the_string_setters_gate_their_argument_and_a_number_is_a_string() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"g = CreateFrame("Model", "GateM", UIParent)"#)
        .unwrap();

    s.run(r#"GateM:SetModel("Interface\\Buttons\\A.mdx")"#)
        .unwrap();
    for bad in ["nil", "{}", "true", "print", ""] {
        assert!(
            s.run(&format!("GateM:SetModel({bad})")).is_err(),
            "SetModel({bad}) must raise — it is not the clear"
        );
    }
    assert_eq!(
        s.eval::<String>("return GateM:GetModel()").unwrap(),
        r"Interface\Buttons\A.mdx",
        "a raised setter leaves the pane alone"
    );
    // A number passes `lua_isstring` and is stored as its decimal text.
    s.run("GateM:SetModel(42)").unwrap();
    assert_eq!(s.eval::<String>("return GateM:GetModel()").unwrap(), "42");

    s.run("GateM:ClearModel()").unwrap();
    assert_eq!(
        s.eval::<Option<String>>("return GateM:GetModel()").unwrap(),
        None
    );

    // `ReplaceIconTexture` swaps the CM2Model's type-14 textures, and a pane with none
    // (`[widget+0x318] == 0`) drops the call, so only the gate is observable.
    for bad in ["nil", "{}", "true", ""] {
        assert!(
            s.run(&format!("GateM:ReplaceIconTexture({bad})")).is_err(),
            "ReplaceIconTexture({bad}) must raise"
        );
    }
    s.run(r#"GateM:SetModel("Interface\\Buttons\\A.mdx")"#)
        .unwrap();
    s.run(r#"GateM:ReplaceIconTexture("Interface\\Icons\\INV_Misc_QuestionMark")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GateM:GetModel()").unwrap(),
        r"Interface\Buttons\A.mdx",
        "it is a MATERIAL swap, not a content setter — the pane's model is untouched"
    );

    // The reference's `AdvanceTime` takes nothing, returns nothing and does nothing.
    assert_eq!(
        s.eval::<usize>("return table.getn({ GateM:AdvanceTime() })")
            .unwrap(),
        0,
        "AdvanceTime pushes no return value"
    );
}

/// The reference's own method tables, entry for entry (`Model` `0x878948`, `PlayerModel`
/// `0x84f1fc`), checked both ways: every name resolves, and a name a table lacks does not.
#[test]
fn the_two_model_tables_are_the_references_own() {
    /// `Model`, `CSimpleModel`'s table `0x878948`, in table order.
    const MODEL_23: [&str; 23] = [
        "SetModel",
        "GetModel",
        "ClearModel",
        "SetPosition",
        "SetFacing",
        "SetModelScale",
        "SetSequence",
        "SetSequenceTime",
        "SetCamera",
        "SetLight",
        "GetLight",
        "GetPosition",
        "GetFacing",
        "GetModelScale",
        "AdvanceTime",
        "ReplaceIconTexture",
        "SetFogColor",
        "GetFogColor",
        "SetFogNear",
        "GetFogNear",
        "SetFogFar",
        "GetFogFar",
        "ClearFog",
    ];
    /// `PlayerModel`, `CGCharacterModelBase`'s table `0x84f1fc`, in table order.
    const PLAYERMODEL_3: [&str; 3] = ["SetUnit", "RefreshUnit", "SetRotation"];
    /// The names in [`MODEL_23`] this client leaves unbuilt, absent rather than stubbed: none.
    const UNBUILT: [&str; 0] = [];

    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        CreateFrame("Model", "GuardM", UIParent)
        CreateFrame("PlayerModel", "GuardPM", UIParent)
    "#,
    )
    .unwrap();
    let is_fn = |s: &UiScript, frame: &str, name: &str| {
        s.eval::<String>(&format!("return type({frame}.{name})"))
            .unwrap()
            == "function"
    };

    for name in MODEL_23 {
        let want = !UNBUILT.contains(&name);
        // A `Model` verb resolves on both panes, on the PlayerModel through the chain.
        for frame in ["GuardM", "GuardPM"] {
            assert_eq!(
                is_fn(&s, frame, name),
                want,
                "{frame}.{name}: table 0x878948 has it; built = {want}"
            );
        }
    }
    for name in PLAYERMODEL_3 {
        assert!(
            is_fn(&s, "GuardPM", name),
            "GuardPM.{name}: table 0x84f1fc entry, all three are built"
        );
        assert!(
            !is_fn(&s, "GuardM", name),
            "GuardM.{name}: 0x84f1fc is NOT reachable from CSimpleModel's lookup"
        );
    }
    // Later-expansion verbs, absent from the 1.12 client image in any form.
    for name in ["SetCreature", "SetCustomRace"] {
        for frame in ["GuardM", "GuardPM"] {
            assert!(
                !is_fn(&s, frame, name),
                "{frame}.{name} does not exist in 1.12.1.5875"
            );
        }
    }
}

/// `SetSequence` and `SetSequenceTime` share one arm (`0x7121a0`): each interrupts what plays and
/// anchors the new sequence's cursor, at 0 or at the caller's `ms`, on the pane's own clock.
#[test]
fn the_two_sequence_verbs_arm_the_pane_on_its_own_clock() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        m = CreateFrame("Model", "MSeq", UIParent)
        m:SetModel("Interface\\Buttons\\UI-AutoCastButton.mdx")
        m:SetSequence(0)
        m:SetSequenceTime(0, 250)
    "#,
    )
    .unwrap();

    // 1.12 has no `GetSequence`, so the arm is read off the arena.
    let armed = |s: &UiScript| {
        let lua = s.lua();
        let model = lua.app_data_ref::<crate::script::Model>().expect("model");
        let fh = model.arena.lookup("MSeq").expect("MSeq frame");
        match &model.arena.frame(fh).expect("live frame").kind_state {
            crate::widget::KindState::Model(m) => {
                (m.sequence, m.armed.map(|a| (a.anim_id, a.anchor_ms)))
            }
            _ => panic!("MSeq is not a Model"),
        }
    };
    // The scrub anchors the cursor 250 ms in: `anchor = clock − ms` at clock 0.
    assert_eq!(armed(&s), (0, Some((0, -250))));

    s.run("MSeq:SetSequence(3)").unwrap();
    assert_eq!(
        armed(&s),
        (3, Some((3, 0))),
        "a new sequence starts at its own 0 — the old anchor is not carried across"
    );

    // `ClearModel` releases the instance, arm included.
    s.run("MSeq:SetSequenceTime(3, 40) MSeq:ClearModel()")
        .unwrap();
    assert_eq!(armed(&s).1, None);
}

/// `SetLight` writes the pane's `CGLight` by the reference's argument walk (`0x76e1e0`), folding
/// intensities into colours and normalising the direction; `GetLight` answers 7, 10 or 13 values.
#[test]
fn the_light_tuple_is_opaque_and_survives_the_round_trip() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MLight", UIParent)"#)
        .unwrap();

    // The constructor's light is off and white; both colour blocks are non-zero, so arity 13.
    let fresh: Vec<f64> = s.eval("return { MLight:GetLight() }").unwrap();
    assert_eq!(
        fresh,
        vec![0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        "the <Model> ctor: disabled, omni, at the origin, white ambient and diffuse"
    );

    // The commented-out call at `CharacterCreate.lua:61`.
    s.run("MLight:SetLight(1, 0, 0, -0.707, -0.707, 0.7, 1, 1, 1, 0.8, 1, 1, 0.8)")
        .unwrap();
    let l: Vec<f64> = s.eval("return { MLight:GetLight() }").unwrap();
    assert_eq!(l.len(), 13);
    assert_eq!((l[0], l[1]), (1.0, 0.0), "enabled, directional");
    // The direction is normalised on write (`0x71b6a0`).
    let dir = [l[2], l[3], l[4]];
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    assert!((len - 1.0).abs() < 1e-5, "normalised on write: {dir:?}");
    // The intensity is folded into the colour, so ambient `1,1,1` at 0.7 reads 0.7 at intensity 1.
    assert_eq!(l[5], 1.0);
    for c in &l[6..9] {
        assert!((c - 0.7).abs() < 1e-5, "ambient folded: {c}");
    }
    assert_eq!(l[9], 1.0);
    for (c, want) in l[10..13].iter().zip([0.8, 0.8, 0.64]) {
        assert!((c - want).abs() < 1e-5, "diffuse folded: {c} vs {want}");
    }

    // `SetLight(0, …)` returns before the copy: a no-op, not a way to switch a light off.
    s.run("MLight:SetLight(0, 1, 5, 5, 5, 1, 1, 1, 1, 1, 1, 1, 1)")
        .unwrap();
    let after: Vec<f64> = s.eval("return { MLight:GetLight() }").unwrap();
    assert_eq!(after, l, "SetLight(0, …) is a no-op, not a disable");

    // A zero ambient intensity skips its colour triple without advancing the cursor, so the next
    // argument (stack index 8) is the diffuse intensity: 0.5, against a white colour.
    s.run("MLight:SetLight(1, 1, 0, 0, 0, 0, 0.5)").unwrap();
    let t2: Vec<f64> = s.eval("return { MLight:GetLight() }").unwrap();
    assert_eq!(t2.len(), 10, "a zero ambient block collapses the arity");
    assert_eq!(t2[5], 0.0, "ambient (0,0,0) reports as a lone 0");
    assert_eq!(t2[6], 1.0);
    for c in &t2[7..10] {
        assert!((c - 0.5).abs() < 1e-5, "diffuse read at index 8: {c}");
    }

    // A non-number where the binding requires one raises rather than coercing.
    assert!(s.run(r#"MLight:SetLight(1, 0, 0, 0, 0)"#).is_err());
    assert!(s.run(r#"MLight:SetLight("x")"#).is_err());

    // The fog colour is one packed `0xAARRGGBB` dword, `0xffffffff` from the constructor, so a
    // fresh pane reads four values, all 1; there is no unset state.
    assert_eq!(
        s.eval::<usize>("return table.getn({ MLight:GetFogColor() })")
            .unwrap(),
        4
    );
    assert_eq!(
        s.eval::<(f64, f64, f64, f64)>("return MLight:GetFogColor()")
            .unwrap(),
        (1.0, 1.0, 1.0, 1.0),
        "never set is white and opaque, not four zeros"
    );

    // With three arguments alpha defaults to 1.0; the packed store keeps 8 bits a channel.
    s.run("MLight:SetFogColor(0.1, 0.2, 0.3)").unwrap();
    let (r, g, b, a): (f64, f64, f64, f64) = s.eval("return MLight:GetFogColor()").unwrap();
    assert_eq!(a, 1.0, "the omitted alpha defaults to 1.0");
    for (got, want) in [(r, 0.1), (g, 0.2), (b, 0.3)] {
        assert!(
            (got - want).abs() <= 1.0 / 255.0,
            "within one 8-bit step of {want}, got {got}"
        );
    }

    // Alpha is the fourth argument, clamped like the rest.
    s.run("MLight:SetFogColor(1, 1, 1, 0)").unwrap();
    assert_eq!(
        s.eval::<f64>("local _, _, _, a = MLight:GetFogColor() return a")
            .unwrap(),
        0.0
    );
}

/// `SetCamera(n)` picks by raw index, checked against the file's camera count once the file is
/// known, and the pane waits to draw until then (`0x76cec0`, `0x76ce80`, `0x76ce00`, `0x76d5f0`).
/// `SetCamera` resolves the camera on the file held then (`0x76ceef` → `0x713590`) and the pane
/// holds it: `SetModel` leaves `+0x31c` (`0x76cd30`) and the new file's ready hook installs
/// nothing while none is pending (`0x76ce44`-`0x76ce47`), so the pane keeps aiming through the
/// old file's camera until the next `SetCamera`.
#[test]
fn set_model_without_set_camera_keeps_the_old_files_camera() {
    use super::model_clock::pane;
    use crate::widget::{InstalledCamera, ModelFileFacts};

    let facts = |cameras: u32| ModelFileFacts {
        sequences: Vec::new(),
        bbox: ([0.0; 3], [0.0; 3]),
        cameras,
    };
    let camera = |s: &UiScript| pane(s, "MSwap").camera;
    let on = |file: &str, index| {
        Some(InstalledCamera {
            file: file.to_string(),
            index,
        })
    };
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts("a.mdx", facts(2));
    s.set_model_facts("b.mdx", facts(3));
    s.run(r#"m = CreateFrame("Model", "MSwap", UIParent) m:SetModel("a.mdx") m:SetCamera(1)"#)
        .unwrap();
    assert_eq!(camera(&s), on("a.mdx", 1));
    s.run(r#"MSwap:SetModel("b.mdx")"#).unwrap();
    assert_eq!(
        camera(&s),
        on("a.mdx", 1),
        "b's ready hook installs nothing"
    );
    s.run("MSwap:SetCamera(2)").unwrap();
    assert_eq!(camera(&s), on("b.mdx", 2));
}

/// A camera draws in perspective; none, or an index past the count, draws orthographic.
#[test]
fn set_camera_is_a_raw_index_bounds_checked_against_the_file() {
    use super::model_clock::pane;
    use crate::widget::{ModelFileFacts, SequenceFacts};

    let facts = |cameras: u32| ModelFileFacts {
        sequences: vec![SequenceFacts {
            anim_id: 0,
            duration_ms: 1000,
            looping: true,
        }],
        bbox: ([0.0; 3], [0.0; 3]),
        cameras,
    };
    let state = |s: &UiScript, name: &str| {
        let m = pane(s, name);
        (m.camera_pending, m.camera.as_ref().map(|c| c.index))
    };
    const FILE: &str = r"Creature\Wolf\Wolf.mdx";

    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MCam", UIParent) m:SetModel("Creature\\Wolf\\Wolf.mdx")"#)
        .unwrap();
    // The constructor's request for camera 0 (`0x76c910` writes `+0x320 = 0`) waits for the file.
    assert_eq!(state(&s, "MCam"), (Some(0), None));
    assert!(s.visible_model_panes().is_empty(), "the draw gate holds");

    // The file lands with two cameras, so the pending index 0 installs: perspective.
    s.set_model_facts(FILE, facts(2));
    assert_eq!(state(&s, "MCam"), (None, Some(0)));
    assert_eq!(s.visible_model_panes().len(), 1, "the gate is settled");

    // With the file known, `SetCamera` resolves at once.
    s.run("MCam:SetCamera(1)").unwrap();
    assert_eq!(state(&s, "MCam"), (None, Some(1)));

    // An index past the count installs no camera, not the last one, and the pane draws on,
    // orthographic.
    s.run("MCam:SetCamera(7)").unwrap();
    assert_eq!(state(&s, "MCam"), (None, None));
    assert_eq!(s.visible_model_panes().len(), 1);
    // A negative index lands in the same place (the reference compares unsigned).
    s.run("MCam:SetCamera(1) MCam:SetCamera(-1)").unwrap();
    assert_eq!(state(&s, "MCam"), (None, None));

    // A file with no cameras, like every shipped UI M2, always draws orthographic.
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MFlat", UIParent) m:SetModel("Interface\\Cooldown\\UI-Cooldown-Indicator.mdx")"#)
        .unwrap();
    s.set_model_facts(r"Interface\Cooldown\UI-Cooldown-Indicator.mdx", facts(0));
    assert_eq!(state(&s, "MFlat"), (None, None));

    // `SetCamera` before the file is known defers, and the index is checked when the file lands.
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MLate", UIParent) m:SetCamera(1) m:SetModel("Creature\\Wolf\\Wolf.mdx")"#)
        .unwrap();
    assert_eq!(state(&s, "MLate"), (Some(1), None));
    assert!(s.visible_model_panes().is_empty());
    s.set_model_facts(FILE, facts(2));
    assert_eq!(state(&s, "MLate"), (None, Some(1)));
}

/// `SetFogColor` arms the fog, `ClearFog` clears bit 0 alone (`0x76f5c5`), and the Lua setters
/// store near and far raw (`0x76ee60`/`0x76f540`).
#[test]
fn the_fog_block_arms_on_colour_and_clears_only_its_bit() {
    use super::model_clock::pane;

    let fog = |s: &UiScript| {
        let m = pane(s, "MFog");
        (m.fog, m.fog_near, m.fog_far)
    };

    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MFog", UIParent)"#)
        .unwrap();
    // The constructor's fog: off, near 0, far 1 (`0x76c950`).
    assert_eq!(fog(&s), (false, 0.0, 1.0));
    assert_eq!(
        s.eval::<(f64, f64)>("return MFog:GetFogNear(), MFog:GetFogFar()")
            .unwrap(),
        (0.0, 1.0)
    );

    // `GlueParent.lua:213-215` with the Tauren row (`:18`); only `SetFogColor` arms the fog.
    s.run("MFog:SetFogNear(0) MFog:SetFogFar(153)").unwrap();
    assert_eq!(fog(&s), (false, 0.0, 153.0), "near/far do not arm the fog");
    s.run("MFog:SetFogColor(1.0, 0.61, 0.42)").unwrap();
    assert!(fog(&s).0, "the colour arms it");

    // `ClearFog` keeps the colour, near and far, so a later `SetFogColor` re-arms the same ramp.
    s.run("MFog:ClearFog()").unwrap();
    assert_eq!(fog(&s), (false, 0.0, 153.0));
    let (r, g, b, _): (f64, f64, f64, f64) = s.eval("return MFog:GetFogColor()").unwrap();
    assert!(
        (r - 1.0).abs() < 0.01 && (g - 0.61).abs() < 0.01 && (b - 0.42).abs() < 0.01,
        "the colour survives a clear: {r}, {g}, {b}"
    );

    // The Lua setters store raw, with no clamp or ordering check; only the XML path clamps at 0.
    s.run("MFog:SetFogNear(-40) MFog:SetFogFar(-1)").unwrap();
    assert_eq!(fog(&s), (false, -40.0, -1.0));
}

/// The light `GetLight` reads back, as `f32`s (the getter widens the stored floats, `0x76e8ca`).
fn light_of(s: &UiScript, pane: &str) -> Vec<f32> {
    let v: mlua::MultiValue = s.eval(&format!("return {pane}:GetLight()")).unwrap();
    v.into_iter()
        .map(|x| match x {
            mlua::Value::Number(n) => n as f32,
            mlua::Value::Integer(i) => i as f32,
            other => panic!("GetLight returned {other:?}"),
        })
        .collect()
}

/// `0x505680` makes the three character kinds' light directional, ambient 0.7, diffuse
/// (0.8, 0.8, 0.64), direction (0, 1, 0) (the second `0x71b6a0` write) and enabled; a plain
/// `Model` keeps its ctor's disabled white point light.
#[test]
fn a_fresh_character_pane_answers_its_constructors_light() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        CreateFrame("PlayerModel", "LitPM", UIParent)
        CreateFrame("DressUpModel", "LitDM", UIParent)
        CreateFrame("TabardModel", "LitTM", UIParent)
        CreateFrame("Model", "DarkM", UIParent)
    "#,
    )
    .unwrap();
    let character = [
        1.0, 0.0, 0.0, 1.0, 0.0, // enabled, directional, (0, 1, 0)
        1.0, 0.7, 0.7, 0.7, // ambient
        1.0, 0.8, 0.8, 0.64, // diffuse
    ];
    for pane in ["LitPM", "LitDM", "LitTM"] {
        let got = light_of(&s, pane);
        assert_eq!(got, character, "{pane}");
        // Bit for bit the ctor's immediates (`0x3f333333`, `0x3f4ccccd`, `0x3f23d70a`).
        assert_eq!(got[6].to_bits(), 0x3f33_3333);
        assert_eq!(got[10].to_bits(), 0x3f4c_cccd);
        assert_eq!(got[12].to_bits(), 0x3f23_d70a);
    }
    assert_eq!(
        light_of(&s, "DarkM"),
        [0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        "a plain Model's light is off"
    );
}

/// The resolver's inputs: the player, a target and the target's target.
fn guids(target: u64, next: u64) -> crate::script::UnitGuids {
    crate::script::UnitGuids {
        player: PLAYER,
        target,
        held: [(PLAYER, 0), (target, next)].into_iter().collect(),
        ..Default::default()
    }
}

const PLAYER: u64 = 0x0000_0000_0000_0001;
const ORC: u64 = 0xF130_0000_0000_0002;
const WOLF: u64 = 0xF130_0000_0000_0003;

fn char_state(s: &UiScript, name: &str) -> crate::widget::CharacterPane {
    s.character_pane(s.frame_handle(name).unwrap()).unwrap()
}

/// `SetUnit` resolves its token once and stores the GUID (`0x505b50`): the pane keeps showing
/// that unit when the token moves, `RefreshUnit` rebuilds the GUID it stored (`0x505ee3`), and
/// only a new `SetUnit` re-reads the token.
#[test]
fn set_unit_stores_the_guid_and_refresh_unit_rebuilds_it() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit_guids(&guids(ORC, WOLF));
    s.run(
        r#"
        a = CreateFrame("PlayerModel", nil, UIParent)
        a:SetWidth(90) a:SetHeight(90) a:SetPoint("CENTER", 0, 0)
        a:SetUnit("TaRgEt")
        b = CreateFrame("PlayerModel", "TotPane", UIParent)
        b:SetUnit("targettarget")
    "#,
    )
    .unwrap();
    let panes = s.character_panes();
    assert_eq!(panes.len(), 2, "every visible character pane, named or not");
    assert_eq!(panes[0].name, None, "the anonymous pane is listed");
    assert_eq!((panes[0].guid, panes[1].guid), (ORC, WOLF));
    assert_eq!(panes[0].builds, 1, "a visible pane asks for its copy");

    // The target moves; the pane does not follow it.
    s.set_unit_guids(&guids(WOLF, 0));
    assert_eq!(s.character_panes()[0].guid, ORC);
    s.run("a:RefreshUnit()").unwrap();
    let refreshed = &s.character_panes()[0];
    assert_eq!(
        refreshed.guid, ORC,
        "RefreshUnit reads +0x3e0, not the token"
    );
    assert_eq!(refreshed.builds, 2, "and asks for a fresh copy of it");
    s.run("a:SetUnit('target')").unwrap();
    assert_eq!(s.character_panes()[0].guid, WOLF);

    // A token nobody holds stores 0 and asks for nothing: no unit, no build (`0x505b8d`).
    s.run("a:SetUnit('mouseover')").unwrap();
    let nobody = &s.character_panes()[0];
    assert_eq!((nobody.guid, nobody.builds), (0, 3));

    // The token is checked like the reference's: a string (`0x84f284`), and a name it knows.
    let err = s.run("a:SetUnit(nil)").unwrap_err().to_string();
    assert!(err.contains(r#"Usage: SetUnit("unit")"#), "{err}");
    let err = s.run("a:SetUnit('nosuchunit')").unwrap_err().to_string();
    assert!(err.contains("Unknown unit name: nosuchunit"), "{err}");
}

/// The reference builds the copy inside `SetUnit` (`0x505b99`), so a `SetModel` or `ClearModel`
/// after it wins: the request it overtook is answered, and a late copy for it is refused.
#[test]
fn a_file_after_set_unit_overtakes_the_copy_in_flight() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit_guids(&guids(ORC, WOLF));
    s.run(r#"a = CreateFrame("PlayerModel", "Doll", UIParent) a:SetUnit("target")"#)
        .unwrap();
    let h = s.frame_handle("Doll").unwrap();
    let pane = |s: &UiScript| s.character_pane(h).unwrap();
    assert!(pane(&s).pending(), "the copy is asked for");
    s.run(r#"a:SetModel("Creature\Wolf\Wolf.mdx")"#).unwrap();
    assert!(!pane(&s).pending(), "the file overtook it");
    let build = pane(&s).builds;
    s.character_copy_built(h, build);
    assert_eq!(
        pane(&s).copy,
        None,
        "a late copy does not displace the file"
    );
    assert!(s.character_panes()[0].file, "the file stays");

    // A later `SetUnit` asks again, and its copy replaces the file.
    s.run("a:SetUnit('target')").unwrap();
    let build = pane(&s).builds;
    s.character_copy_built(h, build);
    assert!(pane(&s).copy.is_some_and(|c| c.build == build));
    assert!(!s.character_panes()[0].file, "the copy took +0x318");
    // One answer per request.
    let arms = pane(&s).arms;
    s.character_copy_built(h, build);
    assert_eq!(pane(&s).arms, arms, "a second answer arms nothing");

    s.run("a:ClearModel()").unwrap();
    assert_eq!(pane(&s).copy, None);
    assert!(!pane(&s).pending());
}

/// The armed id and whether a file sits at `+0x318`, read off the arena.
fn file_arm(s: &UiScript, name: &str) -> (Option<i32>, bool) {
    let lua = s.lua();
    let model = lua.app_data_ref::<crate::script::Model>().expect("model");
    let fh = model.arena.lookup(name).expect("frame");
    match &model.arena.frame(fh).expect("live frame").kind_state {
        crate::widget::KindState::Model(m) => (m.armed.map(|a| a.anim_id), m.path.is_some()),
        _ => panic!("{name} is not a Model"),
    }
}

/// `0x505bb0` and `0x505c50` act on whatever `+0x318` holds, a file `SetModel` put there
/// included, and the hide hook releases it (`0x505ce7`): a re-show builds from the subject
/// alone, so a pane with none shows nothing.
#[test]
fn a_character_pane_turns_and_releases_a_file_as_it_does_a_copy() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        p = CreateFrame("PlayerModel", "FilePane", UIParent)
        p:SetWidth(90) p:SetHeight(90) p:SetPoint("CENTER", 0, 0)
        p:SetModel("Creature\\Wolf\\Wolf.mdx")
        p:SetSequence(0)
        p:SetRotation(0.5)
    "#,
    )
    .unwrap();
    assert_eq!(
        file_arm(&s, "FilePane"),
        (Some(12), true),
        "the file shuffles"
    );
    s.tick(0.05);
    s.tick(0.2);
    assert_eq!(
        file_arm(&s, "FilePane"),
        (Some(0), true),
        "the paint returns the file to Stand"
    );

    s.run("p:Hide() p:Show()").unwrap();
    assert_eq!(
        file_arm(&s, "FilePane"),
        (None, false),
        "the hide released it"
    );
    assert_eq!(
        char_state(&s, "FilePane").builds,
        0,
        "no subject, nothing asked"
    );
}

/// An arm in the same chunk as the build lands on the copy on its way (the reference builds
/// inside `SetUnit` and the show hook, `0x505b99`, `0x505d3c`), after its Stand; and a request the
/// host found no unit for is spent, so nothing builds until the next one (`0x505b8d`).
#[test]
fn an_arm_waits_for_the_copy_on_its_way_and_a_missed_unit_is_spent() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit_guids(&guids(ORC, 0));
    s.run(r#"p = CreateFrame("PlayerModel", "Queue", UIParent) p:SetUnit("player") p:SetSequence(60)"#)
        .unwrap();
    let h = s.frame_handle("Queue").unwrap();
    assert!(char_state(&s, "Queue").copy.is_none());
    s.character_copy_built(h, 1);
    let arm = char_state(&s, "Queue").copy.unwrap().arm;
    assert_eq!((arm.anim_id, arm.blend, arm.serial), (60, false, 2));

    s.run("p:Hide() p:Show() p:SetSequence(61)").unwrap();
    let c = char_state(&s, "Queue");
    assert!(c.pending());
    s.character_copy_missed(h, c.builds);
    let c = char_state(&s, "Queue");
    assert!(!c.pending(), "spent");
    assert!(c.queued.is_none() && c.copy.is_none());
    s.character_copy_built(h, c.builds);
    assert!(
        char_state(&s, "Queue").copy.is_none(),
        "a late copy for a spent request is refused"
    );
}

/// A `SetCamera` or an arm held for build 1 is not build 2's: the second request's copy arrives
/// with the pane's own camera (`0x505b30` → `0x50589c`) and Stand (`0x505a2f`). Build 1 answered
/// before build 2 is asked still takes both.
#[test]
fn a_held_camera_and_arm_die_with_the_request_they_wait_for() {
    use crate::widget::{CopyCamera, ANIM_STAND};
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit_guids(&guids(ORC, 0));
    s.run(r#"p = CreateFrame("PlayerModel", "Held", UIParent) p:SetUnit("target") p:SetCamera(0) p:SetSequence(60) p:RefreshUnit()"#)
        .unwrap();
    let h = s.frame_handle("Held").unwrap();
    assert_eq!(char_state(&s, "Held").builds, 2);
    assert_eq!(row_camera(&s, "Held"), CopyCamera::Pane);
    s.character_copy_built(h, 1);
    assert!(char_state(&s, "Held").copy.is_none(), "build 1 is stale");
    s.character_copy_built(h, 2);
    let c = char_state(&s, "Held");
    assert_eq!(c.camera, CopyCamera::Pane);
    assert_eq!(c.copy.unwrap().arm.anim_id, ANIM_STAND);

    // Answered before the next request, a build takes what it waited for.
    s.run("p:SetUnit('target') p:SetCamera(0) p:SetSequence(61)")
        .unwrap();
    let b = char_state(&s, "Held").builds;
    s.character_copy_built(h, b);
    let c = char_state(&s, "Held");
    assert_eq!(c.camera, CopyCamera::Raw(0));
    assert_eq!(c.copy.unwrap().arm.anim_id, 61);
    s.run("p:RefreshUnit()").unwrap();
    s.character_copy_built(h, b + 1);
    let c = char_state(&s, "Held");
    assert_eq!(c.camera, CopyCamera::Pane);
    assert_eq!(c.copy.unwrap().arm.anim_id, ANIM_STAND);
}

/// Hiding drops the copy and keeps the subject (`0x505ce0`); showing asks for a fresh copy
/// (`0x505d00`); a build answered for a replaced request is dropped.
#[test]
fn hide_drops_the_copy_and_show_asks_for_a_new_one() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit_guids(&guids(ORC, 0));
    s.run(
        r#"
        p = CreateFrame("PlayerModel", "Doll", UIParent)
        p:SetWidth(90) p:SetHeight(90) p:SetPoint("CENTER", 0, 0)
        p:SetUnit("player")
    "#,
    )
    .unwrap();
    let h = s.frame_handle("Doll").unwrap();
    s.character_copy_built(h, 1);
    let copy = char_state(&s, "Doll").copy.expect("built");
    assert_eq!(
        (copy.build, copy.arm.anim_id),
        (1, 0),
        "built with Stand armed"
    );

    s.run("p:Hide()").unwrap();
    let hidden = char_state(&s, "Doll");
    assert!(hidden.copy.is_none(), "the hide hook drops the copy");
    assert_eq!(hidden.guid, PLAYER, "and keeps the GUID");
    assert!(
        s.character_panes().is_empty(),
        "a hidden pane is no paint list row"
    );
    s.character_copy_built(h, 1);
    assert!(
        char_state(&s, "Doll").copy.is_none(),
        "a hidden pane takes no copy"
    );

    s.run("p:Show()").unwrap();
    assert_eq!(char_state(&s, "Doll").builds, 2, "the show hook asks again");
    s.character_copy_built(h, 1);
    assert!(
        char_state(&s, "Doll").copy.is_none(),
        "a copy for the replaced request is stale"
    );
    s.character_copy_built(h, 2);
    assert_eq!(char_state(&s, "Doll").copy.map(|c| c.build), Some(2));

    // A pane shown with no subject asks for nothing.
    s.run(
        r#"
        e = CreateFrame("PlayerModel", "Empty", UIParent) e:Hide() e:Show()
    "#,
    )
    .unwrap();
    assert_eq!(char_state(&s, "Empty").builds, 0);
}

/// `SetRotation` arms the shuffle toward the angle unless it already plays (`0x505bb0`), latches
/// and holds 100 ms; the paint returns to Stand once no rotation came since the last paint and the
/// deadline passed (`0x505c50`); a `SetSequence` arm lasts one paint; a whole-body completion arms
/// Stand (`0x5062a0`) and no `OnAnimFinished` runs.
#[test]
fn the_turn_latch_arms_shuffles_and_the_paint_returns_to_stand() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit_guids(&guids(ORC, 0));
    s.run(
        r#"
        p = CreateFrame("PlayerModel", "Turner", UIParent)
        p:SetWidth(90) p:SetHeight(90) p:SetPoint("CENTER", 0, 0)
        p:SetUnit("player")
        FINISHED = 0
        p:SetScript("OnAnimFinished", function() FINISHED = FINISHED + 1 end)
    "#,
    )
    .unwrap();
    let h = s.frame_handle("Turner").unwrap();
    let arm = |s: &UiScript| char_state(s, "Turner").copy.unwrap().arm;

    // The copy on its way takes the turn: the reference builds inside `SetUnit` (`0x505b99`), so
    // the shuffle lands on the new copy after its Stand.
    s.run("p:SetRotation(0.5)").unwrap();
    assert!(char_state(&s, "Turner").turn_latch);
    s.tick(0.0);
    s.character_copy_built(h, 1);
    let right = arm(&s);
    assert_eq!(
        right.anim_id, 12,
        "facing grew: ShuffleRight, held for the copy"
    );
    assert!(
        right.blend && right.serial == 2,
        "after the copy's Stand (serial 1)"
    );

    // Held the same way, armed once.
    s.run("p:SetRotation(0.6)").unwrap();
    assert_eq!(arm(&s), right);
    s.run("p:SetRotation(0.7)").unwrap();
    assert_eq!(arm(&s), right, "a held direction arms once");
    // Shrinking: ShuffleLeft at once.
    s.run("p:SetRotation(0.65)").unwrap();
    assert_eq!(arm(&s).anim_id, 11);

    // The paint inside the hold keeps it; one past the deadline with no rotation returns to Stand.
    s.tick(0.05);
    assert_eq!(arm(&s).anim_id, 11, "the latch held it");
    s.tick(0.03);
    assert_eq!(arm(&s).anim_id, 11, "before the deadline");
    s.tick(0.1);
    let back = arm(&s);
    assert_eq!(back.anim_id, 0, "past the deadline, Stand");
    assert!(back.blend);
    assert_eq!(char_state(&s, "Turner").turn_deadline_ms, 0);

    // `SetSequence` arms the copy with no blend, and the next paint replaces it.
    s.run("p:SetSequence(60)").unwrap();
    let emote = arm(&s);
    assert_eq!((emote.anim_id, emote.blend), (60, false));
    s.tick(0.01);
    assert_eq!(arm(&s).anim_id, 0, "Stand is the resting state");

    // A completion of the live arm re-arms Stand; one for an arm already replaced does nothing.
    let live = arm(&s);
    s.character_arm_finished(h, 1, live.serial - 1);
    assert_eq!(arm(&s), live, "a cut arm never completes");
    s.character_arm_finished(h, 1, live.serial);
    let again = arm(&s);
    assert_eq!(again.anim_id, 0);
    assert!(
        again.serial > live.serial,
        "a fresh Stand arm, rolled again"
    );
    s.tick(0.5);
    assert_eq!(
        s.eval::<u32>("return FINISHED").unwrap(),
        0,
        "the copy's callback replaces OnAnimFinished"
    );
}

/// `OnUpdateModel` runs only while a model sits at `+0x318` (`0x76d24c`): a stored GUID with no
/// copy built is no model.
#[test]
fn on_update_model_waits_for_the_copy() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit_guids(&guids(ORC, 0));
    s.run(
        r#"
        p = CreateFrame("PlayerModel", "Painted", UIParent)
        p:SetWidth(90) p:SetHeight(90) p:SetPoint("CENTER", 0, 0)
        p:SetUnit("target")
        PAINTS = 0
        p:SetScript("OnUpdateModel", function() PAINTS = PAINTS + 1 end)
    "#,
    )
    .unwrap();
    s.tick(0.02);
    assert_eq!(
        s.eval::<u32>("return PAINTS").unwrap(),
        0,
        "no copy, no paint"
    );
    let h = s.frame_handle("Painted").unwrap();
    s.character_copy_built(h, 1);
    s.tick(0.02);
    assert_eq!(s.eval::<u32>("return PAINTS").unwrap(), 1);
    s.run("p:Hide() p:Show()").unwrap();
    s.tick(0.02);
    assert_eq!(
        s.eval::<u32>("return PAINTS").unwrap(),
        1,
        "the shown pane waits for its new copy"
    );
}

/// The camera a character pane's feed row carries, the one its copy draws through.
fn row_camera(s: &UiScript, name: &str) -> crate::widget::CopyCamera {
    let h = s.frame_handle(name).unwrap();
    s.character_panes()
        .into_iter()
        .find(|p| p.handle == h)
        .expect("a visible pane is a row")
        .camera
}

/// `PlayerModel` adds no `SetCamera` (`0x84f1fc`), so `Model`'s (`0x76e0e0` → `0x76cec0`)
/// installs raw camera `n` on the resident copy; the copy `SetUnit` builds is resident inside the
/// call (`0x505b99`, ready at once through `0x710560`), so `SetCamera(0)` after it takes. Every
/// build's ready hook installs the pane's own camera again (`0x505b30` → `0x50589c`).
#[test]
fn set_camera_installs_a_raw_camera_on_the_copy_until_the_next_build() {
    use crate::widget::CopyCamera;
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit_guids(&guids(ORC, 0));
    s.run(r#"p = CreateFrame("PlayerModel", "Portrait3D", UIParent) p:SetUnit("target") p:SetCamera(0)"#)
        .unwrap();
    let h = s.frame_handle("Portrait3D").unwrap();
    assert_eq!(
        row_camera(&s, "Portrait3D"),
        CopyCamera::Raw(0),
        "the copy on its way draws through the camera named after its build"
    );
    s.character_copy_built(h, 1);
    assert_eq!(char_state(&s, "Portrait3D").camera, CopyCamera::Raw(0));
    assert_eq!(row_camera(&s, "Portrait3D"), CopyCamera::Raw(0));

    // `RefreshUnit` builds again, and the ready hook installs camera 1 over it.
    s.run("p:RefreshUnit()").unwrap();
    assert_eq!(row_camera(&s, "Portrait3D"), CopyCamera::Pane);
    s.character_copy_built(h, 2);
    assert_eq!(char_state(&s, "Portrait3D").camera, CopyCamera::Pane);

    // On the copy that stands, at once; past the table, and below it, no camera.
    s.run("p:SetCamera(99)").unwrap();
    assert_eq!(row_camera(&s, "Portrait3D"), CopyCamera::Raw(99));
    s.run("p:SetCamera(-1)").unwrap();
    assert_eq!(
        row_camera(&s, "Portrait3D"),
        CopyCamera::Raw(u32::MAX),
        "the bound is an unsigned compare (`0x76ceeb`)"
    );

    // A hidden pane holds no copy: the index waits for the show's build, which overwrites it.
    s.run("p:Hide() p:SetCamera(0) p:Show()").unwrap();
    assert_eq!(row_camera(&s, "Portrait3D"), CopyCamera::Pane);
    s.character_copy_built(h, 3);
    assert_eq!(row_camera(&s, "Portrait3D"), CopyCamera::Pane);

    // A unit the client does not hold builds nothing, so the camera lands on the copy that stays.
    s.set_unit_guids(&guids(WOLF, 0));
    s.run("p:SetUnit('target') p:SetCamera(0)").unwrap();
    let builds = char_state(&s, "Portrait3D").builds;
    s.character_copy_missed(h, builds);
    assert_eq!(char_state(&s, "Portrait3D").camera, CopyCamera::Raw(0));
}
