//! **The model-pane family's Lua surface** — `Model` and `PlayerModel`, read back through the API
//! that wrote them.
//!
//! The property under test throughout is that these widgets are *state the app renders*, not state
//! the engine interprets: every setter's value must come back out unchanged, and the places where
//! the widget does have an opinion (one yaw slot written by two verbs on two classes; content being
//! an either/or) must hold.
//!
//! Plus the one structural property, guarded as a whole block:
//! [`the_two_model_tables_are_the_references_own`] asserts our surfaces against the reference's
//! enumerated tables in **both** directions, so neither a missing verb nor an invented one can slip
//! past — and so `Model` can never re-acquire the three that are `PlayerModel`'s.

use super::common::script;
use crate::script::UiScript;

/// The whole scene, set and read back — plus the widget's three actual behaviours: `SetRotation`
/// and `SetFacing` are one slot, `SetModel` and `SetUnit` displace each other, and `ClearModel`
/// empties both.
#[test]
fn the_model_pane_holds_the_scene_it_was_given() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MPane", UIParent)"#)
        .unwrap();

    // A fresh pane: no content, unit SCALE (not zero — a model at scale 0 is invisible, which is
    // why ModelState hand-writes its Default), no yaw, at the origin.
    assert_eq!(
        s.eval::<(Option<String>, f64, f64)>(
            "return MPane:GetModel(), MPane:GetModelScale(), MPane:GetFacing()"
        )
        .unwrap(),
        (None, 1.0, 0.0),
        "a fresh pane has no model and unit scale"
    );

    // The path round-trips verbatim — the client's own path space, backslashes and `.mdx` intact.
    // pfUI's autocast shine is exactly this call.
    s.run(r#"MPane:SetModel("Interface\\Buttons\\UI-AutoCastButton.mdx")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return MPane:GetModel()").unwrap(),
        r"Interface\Buttons\UI-AutoCastButton.mdx"
    );

    // The yaw. `SetFacing` is the `Model` verb for it (`0x878948[4]`); `SetRotation` writes the
    // same field but belongs to `PlayerModel` and is tested there.
    s.run("MPane:SetFacing(-0.25)").unwrap();
    assert_eq!(s.eval::<f64>("return MPane:GetFacing()").unwrap(), -0.25);

    // Scale, camera, position — plain storage, read back through their own getters.
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

    // `SetModel(nil)` is the documented clear and reaches the same place `ClearModel` does — not
    // everything in the corpus calls the dedicated verb.
    s.run("MPane:ClearModel()").unwrap();
    assert_eq!(
        s.eval::<Option<String>>("return MPane:GetModel()").unwrap(),
        None
    );
}

/// **A `<PlayerModel>` is a `<Model>` plus exactly three verbs — and the inheritance runs one way.**
///
/// The client registers four model-pane types and each has its own Lua method table that never
/// repeats its base's; a derived pane reaches its base through the miss leg of `vtable+0x8`
/// (`0x7020b0`). So `PlayerModel`'s three-entry table `0x84f1fc` must sit *over* `Model`'s 23-entry
/// `0x878948`, and nothing may chain the other way: `CSimpleModel`'s lookup `0x76f870` has no leg
/// into `CGCharacterModelBase`'s `0x506260`.
///
/// This is the shape pfUI's unit frames need — `CreateFrame("PlayerModel", ...)` driven by
/// `SetUnit` + `SetCamera`, one verb from each table on the same frame.
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

    // Its own three resolve on the PlayerModel...
    for verb in ["SetUnit", "RefreshUnit", "SetRotation"] {
        assert_eq!(
            s.eval::<String>(&format!("return type(PMPane.{verb})"))
                .unwrap(),
            "function",
            "PlayerModel must answer its own {verb}"
        );
        // ...and on the plain Model they are ABSENT. This is the assertion that would have caught
        // three years of `SetUnit` published on the wrong class.
        assert_eq!(
            s.eval::<String>(&format!("return type(MOnly.{verb})"))
                .unwrap(),
            "nil",
            "a plain Model must NOT answer {verb} — the chain runs derived -> base only"
        );
    }

    // ...and the base's verbs resolve through the chain, unrepeated. pfUI's portrait line.
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

    // `SetRotation` writes THE SAME yaw field `SetFacing` does — `0x505bb0`'s last instruction is
    // `mov [esi+0x39c], eax`, and `+0x39c` is what `0x76dce0` writes. `GetFacing` reads either.
    s.run("PMPane:SetRotation(1.5)").unwrap();
    assert_eq!(s.eval::<f64>("return PMPane:GetFacing()").unwrap(), 1.5);
    s.run("PMPane:SetFacing(-0.25)").unwrap();
    assert_eq!(
        s.eval::<f64>("return PMPane:GetFacing()").unwrap(),
        -0.25,
        "one slot: SetFacing overwrites what SetRotation wrote"
    );

    // RefreshUnit is a live no-op: the pane stores the unit TOKEN and resolves it at render, so
    // there is no cached appearance to invalidate. It must still exist — the reference's own
    // DressUp/PaperDoll frames call it, so an addon hooking them will too.
    assert!(s.run("PMPane:RefreshUnit()").is_ok());

    // ClearModel — a `Model` verb reached through the chain — empties BOTH content slots.
    s.run(r#"PMPane:SetUnit("player") PMPane:ClearModel()"#)
        .unwrap();
    assert_eq!(
        s.eval::<Option<String>>("return PMPane:GetModel()")
            .unwrap(),
        None
    );
}

/// **`SetModel(nil)` raises; `ClearModel()` clears. They are not the same verb.**
///
/// This binding asserted the opposite until 2026-08-30 — "the documented clear, and how the corpus
/// writes 'no model'" — which was invented, not read. `0x76d950` is shape A (decision 1717's
/// taxonomy): `lua_isstring` gates the argument and `Usage: %s:SetModel("file")` is raised on
/// anything that is not a string or a number. `ClearModel 0x76db20` is a separate table entry.
///
/// The same gate on `ReplaceIconTexture 0x76ed70`, whose *whole* observable behaviour it is: the
/// swap itself lands on a `CM2Model` we do not have, and with no CM2Model the reference drops the
/// call and never replays it.
///
/// A NUMBER is accepted by both — `lua_isstring` takes tags 3|4 and the client renders it to
/// decimal text — which is the half a `Value::String`-only match would get wrong.
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
    // ...and none of those raises disturbed the path that was already set.
    assert_eq!(
        s.eval::<String>("return GateM:GetModel()").unwrap(),
        r"Interface\Buttons\A.mdx",
        "a raised setter leaves the pane alone"
    );
    // A number IS a string to `lua_isstring`, and the client renders it to decimal text.
    s.run("GateM:SetModel(42)").unwrap();
    assert_eq!(s.eval::<String>("return GateM:GetModel()").unwrap(), "42");

    // ClearModel is the clear, takes no argument, and empties the pane.
    s.run("GateM:ClearModel()").unwrap();
    assert_eq!(
        s.eval::<Option<String>>("return GateM:GetModel()").unwrap(),
        None
    );

    // `ReplaceIconTexture` — same gate, and that gate is all of it. A good argument is accepted
    // and changes NOTHING observable: the swap targets the CM2Model's type-14 textures, and a
    // pane with no CM2Model drops the call (the reference's own `[widget+0x318] == 0` leg).
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

    // `AdvanceTime` takes nothing, returns nothing, and does nothing — verified, not stubbed.
    assert_eq!(
        s.eval::<usize>("return table.getn({ GateM:AdvanceTime() })")
            .unwrap(),
        0,
        "AdvanceTime pushes no return value"
    );
}

/// **The whole block, both directions** — decision 1718's rule applied to the two model tables.
///
/// The lists below are the reference's OWN enumerations, transcribed entry-for-entry from `Model`'s
/// table `0x878948` and `PlayerModel`'s `0x84f1fc` (each table's count fixed twice: the registering
/// `mov edx, imm32`, and the dword at `base + 8*count` being the start of the string pool). They
/// are **not** the names anyone noticed were missing — that is exactly the mistake 1718 records,
/// and it was re-made inside the test written to enforce it.
///
/// Guarding both directions is the point:
///
/// - **no gap** — every reference name we claim to publish must resolve;
/// - **no superset** — a name the reference does not have on a table must not resolve there
///   (1189: a name we have and the reference lacks routes an addon down a path the real client
///   never takes);
/// - **the `UNBUILT` set must stay absent** — it is empty since 2027, so "unbuilt" cannot
///   quietly become "stubbed" without this test being edited to say so.
#[test]
fn the_two_model_tables_are_the_references_own() {
    /// `Model` — `CSimpleModel`, table `0x878948`, 23 entries, in table order.
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
    /// `PlayerModel` — `CGCharacterModelBase`, table `0x84f1fc`, 3 entries, in table order.
    const PLAYERMODEL_3: [&str; 3] = ["SetUnit", "RefreshUnit", "SetRotation"];
    /// The subset of [`MODEL_23`] this client does not build. Named, not stubbed (1134 §4).
    /// **Empty since decision 2027** — the last five were the fog near/far set, held back only
    /// because `ClearFog`'s effect on the colour/near/far triple was uncarved;
    /// `76f5c5 and [edi+0x3a4],-2` shows bit 0 alone, so the guess is gone and the verbs are
    /// real. Kept as an empty array rather than deleted: the wall below is the thing that
    /// notices when a name arrives or leaves.
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
        // A `Model` verb resolves on BOTH panes — on the PlayerModel through the chain.
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
    // The two names a `strings` scan of the model band would tempt anyone into, which do not exist
    // in 5875 in ANY form (substring scan of the mapped image returns 0, positive control 27 hits
    // for `Creature`). They are later-expansion verbs; publishing one is decision 1189's error.
    for name in ["SetCreature", "SetCustomRace"] {
        for frame in ["GuardM", "GuardPM"] {
            assert!(
                !is_fn(&s, frame, name),
                "{frame}.{name} does not exist in 1.12.1.5875"
            );
        }
    }
}

/// **`SetSequence` and `SetSequenceTime` are one arm** (`0x7121a0`): each interrupts what plays
/// and anchors the new sequence's cursor — at 0, or at the caller's `ms` — on the pane's own
/// clock. The clock law itself (advance, wrap, completion, the two handlers) is
/// `script::tests::model_clock`; this is the binding-level shape, with no file facts known, which
/// is the state the reference's queued replay covers.
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

    // 1.12 has no `GetSequence`, so the arm is read off the model the way `simplehtml`'s tests
    // read their blocks — through the arena, because there is no Lua getter to read it through.
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

    // ...and ClearModel releases the instance, arm included.
    s.run("MSeq:SetSequenceTime(3, 40) MSeq:ClearModel()")
        .unwrap();
    assert_eq!(armed(&s).1, None);
}

/// `SetLight` writes the widget's embedded `CGLight` by the reference's own argument walk
/// (`0x76e1e0`), and `GetLight` reads it back at arity **7 | 10 | 13**.
///
/// This test is the law, not a round trip: the tuple used to be stored verbatim because nobody had
/// read the binding at the bytes, and "what went in comes out" is the one thing the reference does
/// NOT do — the intensities are folded into the colours, the direction is normalised, and both
/// traps change what a call means.
#[test]
fn the_light_tuple_is_opaque_and_survives_the_round_trip() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MLight", UIParent)"#)
        .unwrap();

    // A fresh pane: the ctor's white, switched off. Both colour blocks are `> 0`, so both come
    // back as `1.0, r, g, b` — arity 13 before anything has ever been set.
    let fresh: Vec<f64> = s.eval("return { MLight:GetLight() }").unwrap();
    assert_eq!(
        fresh,
        vec![0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        "the <Model> ctor: disabled, omni, at the origin, white ambient and diffuse"
    );

    // The commented-out GlueXML line, which is the corpus's only example of the shape.
    s.run("MLight:SetLight(1, 0, 0, -0.707, -0.707, 0.7, 1, 1, 1, 0.8, 1, 1, 0.8)")
        .unwrap();
    let l: Vec<f64> = s.eval("return { MLight:GetLight() }").unwrap();
    assert_eq!(l.len(), 13);
    assert_eq!((l[0], l[1]), (1.0, 0.0), "enabled, directional");
    // The direction is NORMALISED on write (`0x71b6a0`), so `(0, -0.707, -0.707)` comes back as
    // a unit vector — the one place a naive round trip would disagree.
    let dir = [l[2], l[3], l[4]];
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    assert!((len - 1.0).abs() < 1e-5, "normalised on write: {dir:?}");
    // The intensity is FOLDED into the colour: ambient `1,1,1` at `0.7` is `(0.7, 0.7, 0.7)`,
    // and the reported intensity is a constant 1.0.
    assert_eq!(l[5], 1.0);
    for c in &l[6..9] {
        assert!((c - 0.7).abs() < 1e-5, "ambient folded: {c}");
    }
    assert_eq!(l[9], 1.0);
    for (c, want) in l[10..13].iter().zip([0.8, 0.8, 0.64]) {
        assert!((c - want).abs() < 1e-5, "diffuse folded: {c} vs {want}");
    }

    // TRAP 1: `SetLight(0, …)` returns before the local is copied, so it changes NOTHING — it is
    // not a way to switch a light off, and a `<PlayerModel>`'s built-in one cannot be killed.
    s.run("MLight:SetLight(0, 1, 5, 5, 5, 1, 1, 1, 1, 1, 1, 1, 1)")
        .unwrap();
    let after: Vec<f64> = s.eval("return { MLight:GetLight() }").unwrap();
    assert_eq!(after, l, "SetLight(0, …) is a no-op, not a disable");

    // TRAP 2: an ambient intensity of 0 makes the walk skip its colour triple WITHOUT advancing
    // the cursor, so the second intensity is read at index 8 — the seven-argument form. Here
    // `1, 1, 1` are the ambient rgb the walk never reaches, and `0.5` is index 8 = the diffuse
    // intensity against a white colour.
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

    // **Fog is NOT the same shape, and this used to assert the opposite.** It read "unset fog
    // returns NOTHING rather than three zeros — a pane with no fog and a pane fogged to black are
    // different states", which is a good argument about a model the client does not have: there is
    // no unset state. The fog colour is one packed `0xAARRGGBB` dword whose ctor writes
    // `0xffffffff`, so a fresh pane reads **four** values, `1, 1, 1, 1`.
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

    // Three arguments set alpha to **1.0**, not 0 — the fifth is guarded with that default where
    // r/g/b are read unconditionally. The round trip is LOSSY by 8 bits a channel, because the
    // store is that packed dword: 0.1 does not survive, 0.2 does.
    s.run("MLight:SetFogColor(0.1, 0.2, 0.3)").unwrap();
    let (r, g, b, a): (f64, f64, f64, f64) = s.eval("return MLight:GetFogColor()").unwrap();
    assert_eq!(a, 1.0, "the omitted alpha defaults to 1.0");
    for (got, want) in [(r, 0.1), (g, 0.2), (b, 0.3)] {
        assert!(
            (got - want).abs() <= 1.0 / 255.0,
            "within one 8-bit step of {want}, got {got}"
        );
    }

    // …and the alpha really is the fifth argument, on the same clamp as the rest.
    s.run("MLight:SetFogColor(1, 1, 1, 0)").unwrap();
    assert_eq!(
        s.eval::<f64>("local _, _, _, a = MLight:GetFogColor() return a")
            .unwrap(),
        0.0
    );
}

/// `SetCamera(n)` selects by **raw table index**, is bounds-checked against the file's camera
/// count, defers until the file's facts land, and gates the paint until the question is settled —
/// the four halves of `0x76cec0`/`0x76ce80`/`0x76ce00`/`76d5f0`.
///
/// The whole point of the state is which render leg a pane takes: an installed camera is the
/// perspective leg, the NULL camera is the orthographic one, and an index past the count is a
/// NULL camera rather than an error or a clamp to the last record.
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
    // The ctor's standing request for camera 0 (`0x76c910` writes `+0x320 = 0`), still pending
    // because the file's facts have not arrived — so the pane is NOT on the paint list.
    assert_eq!(state(&s, "MCam"), (Some(0), None));
    assert!(s.visible_model_panes().is_empty(), "the draw gate holds");

    // The facts land: the model-ready hook applies the pending index. Wolf has two cameras, so
    // index 0 installs and the pane takes the perspective leg.
    s.set_model_facts(FILE, facts(2));
    assert_eq!(state(&s, "MCam"), (None, Some(0)));
    assert_eq!(s.visible_model_panes().len(), 1, "the gate is settled");

    // With the facts in hand `SetCamera` resolves immediately — raw index 1 is a real record.
    s.run("MCam:SetCamera(1)").unwrap();
    assert_eq!(state(&s, "MCam"), (None, Some(1)));

    // An index past the count installs the NULL camera: the ORTHOGRAPHIC leg, and the pane keeps
    // drawing. Not a clamp to the last record, not a raise.
    s.run("MCam:SetCamera(7)").unwrap();
    assert_eq!(state(&s, "MCam"), (None, None));
    assert_eq!(s.visible_model_panes().len(), 1);
    // A negative index lands in the same place (the reference compares unsigned).
    s.run("MCam:SetCamera(1) MCam:SetCamera(-1)").unwrap();
    assert_eq!(state(&s, "MCam"), (None, None));

    // A file with no camera table at all: every index is out of range, so a plain <Model> on it
    // is always the orthographic leg — which is every shipped in-game UI M2.
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MFlat", UIParent) m:SetModel("Interface\\Cooldown\\UI-Cooldown-Indicator.mdx")"#)
        .unwrap();
    s.set_model_facts(r"Interface\Cooldown\UI-Cooldown-Indicator.mdx", facts(0));
    assert_eq!(state(&s, "MFlat"), (None, None));

    // `SetCamera` before the file is known DEFERS, and the deferred index is what the facts
    // apply — bounds-checked then, not at the call.
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"m = CreateFrame("Model", "MLate", UIParent) m:SetCamera(1) m:SetModel("Creature\\Wolf\\Wolf.mdx")"#)
        .unwrap();
    assert_eq!(state(&s, "MLate"), (Some(1), None));
    assert!(s.visible_model_panes().is_empty());
    s.set_model_facts(FILE, facts(2));
    assert_eq!(state(&s, "MLate"), (None, Some(1)));
}

/// The fog block: `SetFogColor` **arms** it, `ClearFog` disarms **bit 0 alone**, and near/far are
/// stored raw by the Lua setters (`0x76ee60`/`0x76f540`). The five verbs held back
/// as uncarved since 1134 §4 are real now.
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
    // The ctor: fog OFF, near 0.0, far **1.0** (`76c950`) — not far 0, which would be a
    // degenerate ramp the batch would refuse.
    assert_eq!(fog(&s), (false, 0.0, 1.0));
    assert_eq!(
        s.eval::<(f64, f64)>("return MFog:GetFogNear(), MFog:GetFogFar()")
            .unwrap(),
        (0.0, 1.0)
    );

    // The glue's own call shape. `SetFogColor` is the arming verb; near/far are not.
    s.run("MFog:SetFogNear(0) MFog:SetFogFar(153)").unwrap();
    assert_eq!(fog(&s), (false, 0.0, 153.0), "near/far do not arm the fog");
    s.run("MFog:SetFogColor(1.0, 0.61, 0.42)").unwrap();
    assert!(fog(&s).0, "the colour arms it");

    // `ClearFog` touches bit 0 and nothing else: the colour, the near and the far all survive, so
    // a later SetFogColor re-arms the same ramp.
    s.run("MFog:ClearFog()").unwrap();
    assert_eq!(fog(&s), (false, 0.0, 153.0));
    let (r, g, b, _): (f64, f64, f64, f64) = s.eval("return MFog:GetFogColor()").unwrap();
    assert!(
        (r - 1.0).abs() < 0.01 && (g - 0.61).abs() < 0.01 && (b - 0.42).abs() < 0.01,
        "the colour survives a clear: {r}, {g}, {b}"
    );

    // The Lua setters store RAW — no clamp, no ordering check. Only the XML attribute path
    // clamps at `>= 0`.
    s.run("MFog:SetFogNear(-40) MFog:SetFogFar(-1)").unwrap();
    assert_eq!(fog(&s), (false, -40.0, -1.0));
}
