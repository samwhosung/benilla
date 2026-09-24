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

    // `RefreshUnit` is a no-op, as the pane resolves its unit token at render; stock
    // `PaperDollFrame.xml:221` calls it.
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
        (m.camera_pending, m.camera)
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
