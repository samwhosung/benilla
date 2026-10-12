use std::rc::Rc;

use super::*;
use crate::script::UiScript;

fn race(id: u8, name: &str, file: &str, faction: &str, tokens: [&str; 3]) -> GlueCreateRace {
    GlueCreateRace {
        id,
        name: name.into(),
        file: file.into(),
        faction: Some((faction.into(), faction.into())),
        tokens: tokens.map(String::from),
    }
}

/// Human and Dwarf for the Alliance, Orc for the Horde. Dwarf female has no model; Orc's tokens
/// tell its three slots apart.
fn catalog() -> GlueCreateCatalog {
    let class = |name: &str, file: &str| GlueCreateClass {
        name: name.into(),
        file: file.into(),
    };
    GlueCreateCatalog {
        races: vec![
            race(
                1,
                "Human",
                "Human",
                "Alliance",
                ["NORMAL", "PIERCINGS", "NORMAL"],
            ),
            race(
                3,
                "Dwarf",
                "Dwarf",
                "Alliance",
                ["NORMAL", "PIERCINGS", "NORMAL"],
            ),
            race(2, "Orc", "Orc", "Horde", ["TUSKS", "PIERCINGS", "HORNS"]),
        ],
        class_lists: [
            (1, vec![1, 2, 4]),
            (3, vec![2, 1, 3]),
            (2, vec![1, 3, 4, 6]),
        ]
        .into_iter()
        .collect(),
        classes: [
            (1, class("Warrior", "WARRIOR")),
            (2, class("Paladin", "PALADIN")),
            (3, class("Hunter", "HUNTER")),
            (4, class("Rogue", "ROGUE")),
        ]
        .into_iter()
        .collect(),
        counts: [
            ((1, 0), counts([10, 5, 12, 10, 9])),
            ((1, 1), counts([10, 15, 19, 10, 7])),
            ((3, 0), {
                // The Dwarf male's last style has four colours.
                let mut c = counts([9, 10, 10, 10, 11]);
                c.hair_colors[9] = 4;
                c
            }),
            ((2, 0), counts([9, 9, 7, 8, 1])),
            ((2, 1), counts([9, 9, 7, 8, 1])),
        ]
        .into_iter()
        .collect(),
        models: [(1, 0), (1, 1), (3, 0), (2, 0), (2, 1)]
            .into_iter()
            .collect(),
        names: vec![
            name_row(1, 0, "Abe"),
            name_row(1, 1, "Ada"),
            name_row(2, 0, "Bob"),
        ],
    }
}

/// Skin, face, hair styles, each style's colours and facial hair.
fn counts([skin, face, styles, colors, facial_hair]: [i32; 5]) -> GlueCreateCounts {
    GlueCreateCounts {
        skin,
        face,
        hair_styles: styles,
        hair_colors: vec![colors; styles as usize],
        facial_hair,
    }
}

fn name_row(race: u32, sex: u32, name: &str) -> GlueNameRow {
    GlueNameRow {
        race,
        sex,
        name: name.into(),
    }
}

/// The word whose draw over `n` is `k`.
fn word(k: u32, n: u32) -> u32 {
    ((u64::from(2 * k + 1) << 31) / u64::from(n)) as u32
}

/// A glue VM with the catalog seated, the frame set, and `FIRED` logging the status dialog.
fn vm() -> UiScript {
    let mut s = UiScript::new_glue().unwrap();
    s.set_glue_create_catalog(Rc::new(catalog()));
    s.run(
        "FIRED = {} local f = CreateFrame('Frame', 'Listener') \
         f:SetScript('OnEvent', function() \
           table.insert(FIRED, event .. ' ' .. tostring(arg1) .. ' ' .. tostring(arg2)) end) \
         f:RegisterEvent('OPEN_STATUS_DIALOG') \
         CreateFrame('ModelFFX', 'Create') SetCharCustomizeFrame('Create')",
    )
    .unwrap();
    s
}

/// [`vm`] reset to a male Human with `dials` (skin, face, hair style, hair colour, facial hair).
fn human(dials: [u32; 5]) -> UiScript {
    let mut s = vm();
    let counts = [10, 5, 12, 10, 9];
    // The draw order: sex, race index, skin, hair style, hair colour, facial hair, face.
    s.script_glue_create_draws(&[
        word(0, 2),
        word(0, 3),
        word(dials[0], counts[0]),
        word(dials[2], counts[2]),
        word(dials[3], counts[3]),
        word(dials[4], counts[4]),
        word(dials[1], counts[1]),
    ]);
    s.run("ResetCharCustomize()").unwrap();
    s
}

fn look(s: &UiScript) -> GlueCreateLook {
    s.glue_create_look().expect("a character stands")
}

fn dials(l: GlueCreateLook) -> [u8; 5] {
    [l.skin, l.face, l.hair_style, l.hair_color, l.facial_hair]
}

fn count(call: &str) -> String {
    format!("local function n(...) return arg.n end return n({call})")
}

fn fired(s: &UiScript) -> Vec<String> {
    s.eval::<Vec<String>>("return FIRED").unwrap()
}

/// `0x4702a0` draws the sex, then the race's index into the list, then `0x4703c0` the five in its
/// order; the class is 1 (`0x4709ed`) and the selected index the character's race's.
#[test]
fn reset_draws_a_sex_a_race_and_five_customizations_in_order() {
    let mut s = vm();
    s.script_glue_create_draws(&[
        word(0, 2),
        word(2, 3),
        word(3, 9),
        word(6, 7),
        word(5, 8),
        word(0, 1),
        word(8, 9),
    ]);
    s.run("ResetCharCustomize()").unwrap();
    assert_eq!(
        look(&s),
        GlueCreateLook {
            race: 2,
            sex: 0,
            class: 1,
            skin: 3,
            face: 8,
            hair_style: 6,
            hair_color: 5,
            facial_hair: 0,
        }
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return GetSelectedRace(), GetSelectedSex()")
            .unwrap(),
        (3.0, 1.0)
    );
    // The draws are `(n · word) >> 32` (`0x455c70`).
    assert_eq!(scale(0xffff_ffff, 9), 8);
    assert_eq!(scale(0x8000_0000, 2), 1);
    assert_eq!(scale(0x7fff_ffff, 2), 0);
}

/// With no frame the reset does nothing (`0x470913`), so nothing stands.
#[test]
fn reset_without_a_frame_does_nothing() {
    let mut s = UiScript::new_glue().unwrap();
    s.set_glue_create_catalog(Rc::new(catalog()));
    s.run("ResetCharCustomize()").unwrap();
    assert_eq!(s.glue_create_look(), None);
    // The frame type is the model's: a plain frame is not taken.
    s.run("CreateFrame('Frame', 'Plain') SetCharCustomizeFrame('Plain') ResetCharCustomize()")
        .unwrap();
    assert_eq!(s.glue_create_look(), None);
    let err = s.run("SetCharCustomizeFrame()").unwrap_err().to_string();
    assert!(
        err.contains("Usage: SetCharCustomizeFrame(\"frameName\")"),
        "{err}"
    );
}

/// `0x470cf0`: the selected index or one off the list changes nothing; another re-draws the five,
/// takes the class from the list the last build made (`0x470df4`) and lists the new race's.
#[test]
fn set_selected_race_redraws_and_takes_the_old_lists_first_class() {
    let mut s = human([1, 2, 3, 4, 5]);
    assert_eq!(
        s.eval::<Vec<String>>("return {GetClassesForRace()}")
            .unwrap(),
        ["Warrior", "WARRIOR", "Paladin", "PALADIN", "Rogue", "ROGUE"]
    );
    s.run("SetSelectedClass(3)").unwrap();
    assert_eq!(look(&s).class, 4);
    for call in [
        "SetSelectedRace(1)",
        "SetSelectedRace(0)",
        "SetSelectedRace(4)",
        "SetSelectedRace(-1)",
    ] {
        s.run(call).unwrap();
        assert_eq!(dials(look(&s)), [1, 2, 3, 4, 5], "{call}");
        assert_eq!(look(&s).class, 4, "{call}");
    }
    // Dwarf: male kept, five fresh draws.
    s.script_glue_create_draws(&[
        word(8, 9),
        word(9, 10),
        word(0, 10),
        word(10, 11),
        word(7, 10),
    ]);
    s.run("SetSelectedRace(2)").unwrap();
    let l = look(&s);
    assert_eq!((l.race, l.sex), (3, 0));
    assert_eq!(dials(l), [8, 7, 9, 0, 10]);
    assert_eq!(l.class, 1, "the Human list's first");
    assert_eq!(
        s.eval::<Vec<String>>("return {GetClassesForRace()}")
            .unwrap(),
        ["Paladin", "PALADIN", "Warrior", "WARRIOR", "Hunter", "HUNTER"]
    );
    // Orc: the Dwarf list's first is Paladin, so the Orc is a Paladin until a class is picked.
    s.run("SetSelectedRace(3)").unwrap();
    assert_eq!(look(&s).class, 2);
    assert_eq!(s.eval::<f64>("return GetSelectedRace()").unwrap(), 3.0);
    let err = s.run("SetSelectedRace('x')").unwrap_err().to_string();
    assert!(err.contains("Usage: SetSelectedRace(index)"), "{err}");
}

/// `0x470e30`: 3 or more, or the character's own sex, changes nothing; a sex with no model draws
/// and builds nothing; another re-draws the five.
#[test]
fn set_selected_sex_redraws_or_changes_nothing() {
    let mut s = human([1, 2, 3, 4, 5]);
    for call in [
        "SetSelectedSex(1)",
        "SetSelectedSex(4)",
        "SetSelectedSex(0)",
    ] {
        s.run(call).unwrap();
        assert_eq!(look(&s).sex, 0, "{call}");
        assert_eq!(dials(look(&s)), [1, 2, 3, 4, 5], "{call}");
    }
    // Sex 2 is let through (`0x470e3c`, below 3) and has no body.
    s.run("SetSelectedSex(3)").unwrap();
    assert_eq!(look(&s).sex, 0);
    s.script_glue_create_draws(&[
        word(9, 10),
        word(18, 19),
        word(9, 10),
        word(6, 7),
        word(14, 15),
    ]);
    s.run("SetSelectedSex(2)").unwrap();
    let l = look(&s);
    assert_eq!((l.race, l.sex), (1, 1));
    assert_eq!(dials(l), [9, 14, 18, 9, 6]);
    assert_eq!(s.eval::<f64>("return GetSelectedSex()").unwrap(), 2.0);
    let err = s.run("SetSelectedSex()").unwrap_err().to_string();
    assert!(err.contains("Usage: SetSelectedSex(index)"), "{err}");
}

/// A body with no model builds nothing (`0x470b80`): the Dwarf female keeps the male.
#[test]
fn a_race_and_sex_with_no_model_keeps_the_character() {
    let mut s = human([0; 5]);
    s.script_glue_create_draws(&[0; 5]);
    s.run("SetSelectedRace(2)").unwrap();
    let before = look(&s);
    assert_eq!((before.race, before.sex), (3, 0));
    s.run("SetSelectedSex(2)").unwrap();
    assert_eq!(look(&s), before);
}

/// `0x470f50`: an index on the class list takes its id, one off it nothing; no values.
#[test]
fn set_selected_class_takes_the_lists_id() {
    let s = human([0; 5]);
    for (call, want) in [
        ("SetSelectedClass(2)", 2),
        ("SetSelectedClass(0)", 2),
        ("SetSelectedClass(4)", 2),
        ("SetSelectedClass(3)", 4),
        ("SetSelectedClass(1.9)", 1),
    ] {
        assert_eq!(s.eval::<f64>(&count(call)).unwrap(), 0.0, "{call}");
        assert_eq!(look(&s).class, want, "{call}");
    }
    assert_eq!(
        s.eval::<(String, String)>("return GetSelectedClass()")
            .unwrap(),
        ("Warrior".into(), "WARRIOR".into())
    );
    let err = s.run("SetSelectedClass({})").unwrap_err().to_string();
    assert!(err.contains("Usage: SetSelectedClass(index)"), "{err}");
}

/// The Orc list names class 6, which has no `ChrClasses` row: its pair is nil, nil, and it cannot
/// be picked.
#[test]
fn a_listed_class_with_no_row_reads_nil() {
    let mut s = vm();
    s.script_glue_create_draws(&[word(0, 2), word(2, 3)]);
    s.run("ResetCharCustomize()").unwrap();
    assert!(s
        .eval::<bool>(
            "local t = {GetClassesForRace()} \
             return t[5] == 'Rogue' and t[7] == nil and t[8] == nil"
        )
        .unwrap());
    assert_eq!(
        s.eval::<f64>(&count("GetClassesForRace()")).unwrap(),
        8.0,
        "four classes, two values each"
    );
    s.run("SetSelectedClass(4)").unwrap();
    assert_eq!(look(&s).class, 1);
}

/// `0x470f80`: a step wraps, a long one lands on an end, a delta of 0 or an index outside 1..5
/// changes nothing.
#[test]
fn cycle_wraps_a_step_and_lands_a_long_one_on_an_end() {
    let s = human([0, 4, 11, 5, 8]);
    for (call, want) in [
        ("CycleCharCustomization(1, 1)", [1, 4, 11, 5, 8]),
        ("CycleCharCustomization(1, -1)", [0, 4, 11, 5, 8]),
        ("CycleCharCustomization(1, -1)", [9, 4, 11, 5, 8]),
        ("CycleCharCustomization(2, 1)", [9, 0, 11, 5, 8]),
        ("CycleCharCustomization(3, 1)", [9, 0, 0, 5, 8]),
        ("CycleCharCustomization(4, 3)", [9, 0, 0, 8, 8]),
        ("CycleCharCustomization(4, 3)", [9, 0, 0, 0, 8]),
        ("CycleCharCustomization(4, -7)", [9, 0, 0, 9, 8]),
        ("CycleCharCustomization(5, 0)", [9, 0, 0, 9, 8]),
        ("CycleCharCustomization(0, 1)", [9, 0, 0, 9, 8]),
        ("CycleCharCustomization(6, 1)", [9, 0, 0, 9, 8]),
        ("CycleCharCustomization(5, -1)", [9, 0, 0, 9, 7]),
    ] {
        assert_eq!(s.eval::<f64>(&count(call)).unwrap(), 0.0, "{call}");
        assert_eq!(dials(look(&s)), want, "{call}");
    }
    for call in [
        "CycleCharCustomization(1)",
        "CycleCharCustomization('a', 1)",
    ] {
        let err = s.run(call).unwrap_err().to_string();
        assert!(
            err.contains("Usage: CycleCharCustomization(index, delta)"),
            "{err}"
        );
    }
}

/// A hair colour counts its style's colours (`0x47a400`): the draw after the style takes the drawn
/// style's (`0x4703ed`), the cycle and `HasCharCustomization` the current one's.
#[test]
fn a_hair_colour_counts_its_styles_colours() {
    let mut s = human([0; 5]);
    // The Dwarf male's tenth style has four colours, the others ten.
    s.script_glue_create_draws(&[
        word(0, 9),
        word(9, 10),
        word(3, 4),
        word(0, 11),
        word(0, 10),
    ]);
    s.run("SetSelectedRace(2)").unwrap();
    let l = look(&s);
    assert_eq!((l.race, l.hair_style, l.hair_color), (3, 9, 3));
    s.run("CycleCharCustomization(4, 1)").unwrap();
    assert_eq!(look(&s).hair_color, 0, "four colours wrap at 4");
    s.run("CycleCharCustomization(3, -1) CycleCharCustomization(4, -1)")
        .unwrap();
    assert_eq!(look(&s).hair_style, 8);
    assert_eq!(look(&s).hair_color, 9, "the ninth style's ten colours");
}

/// The grid's record is `race·2 + sex` (`0x47a3e0`): a sex of 2 draws over the next race's male
/// counts, five words for a Human over the Orc male's, then builds nothing.
#[test]
fn sex_two_draws_over_the_next_races_male_counts() {
    let mut s = human([1, 1, 1, 1, 1]);
    s.script_glue_create_draws(&[0, 0, 0, 0, 0]);
    s.script_glue_create_draws(&[
        word(9, 10),
        word(11, 12),
        word(0, 10),
        word(3, 9),
        word(4, 5),
    ]);
    s.run("SetSelectedSex(3) RandomizeCharCustomization()")
        .unwrap();
    let l = look(&s);
    assert_eq!((l.race, l.sex), (1, 0));
    assert_eq!(
        dials(l),
        [9, 4, 11, 0, 3],
        "the sex-2 draws took the first five words"
    );
}

/// `0x471080`: five fresh draws in `0x4703c0`'s order, the race, sex and class kept.
#[test]
fn randomize_redraws_the_five_in_place() {
    let mut s = human([1, 1, 1, 1, 1]);
    s.run("SetSelectedClass(2)").unwrap();
    s.script_glue_create_draws(&[
        word(9, 10),
        word(11, 12),
        word(0, 10),
        word(3, 9),
        word(4, 5),
    ]);
    assert_eq!(
        s.eval::<f64>(&count("RandomizeCharCustomization()"))
            .unwrap(),
        0.0
    );
    let l = look(&s);
    assert_eq!((l.race, l.sex, l.class), (1, 0, 2));
    assert_eq!(dials(l), [9, 4, 11, 0, 3]);
}

/// `0x471820`: 1 for more than one choice, nil for one, none or an index outside 1..5.
#[test]
fn has_char_customization_answers_one_or_nil() {
    let mut s = vm();
    s.script_glue_create_draws(&[word(0, 2), word(2, 3)]);
    s.run("ResetCharCustomize()").unwrap();
    for (call, want) in [
        ("HasCharCustomization(1)", Some(1.0)),
        ("HasCharCustomization(4)", Some(1.0)),
        ("HasCharCustomization(5)", None),
        ("HasCharCustomization(0)", None),
        ("HasCharCustomization(6)", None),
    ] {
        assert_eq!(
            s.eval::<Option<f64>>(&format!("return {call}")).unwrap(),
            want,
            "{call}"
        );
        assert_eq!(s.eval::<f64>(&count(call)).unwrap(), 1.0, "{call}");
    }
    let err = s.run("HasCharCustomization()").unwrap_err().to_string();
    assert!(err.contains("Usage: HasCharCustomization(index)"), "{err}");
}

/// The race verbs: the list's name and file pairs; the character's race's name, file and faction;
/// the selected list race's tokens, facial hair by the character's sex.
#[test]
fn the_race_verbs_read_the_list_and_the_character() {
    let mut s = vm();
    assert_eq!(
        s.eval::<Vec<String>>("return {GetAvailableRaces()}")
            .unwrap(),
        ["Human", "Human", "Dwarf", "Dwarf", "Orc", "Orc"]
    );
    // Before a build: the null character's cells read 0, a race with no row.
    assert_eq!(
        s.eval::<(Option<String>, Option<String>)>("return GetNameForRace()")
            .unwrap(),
        (None, None)
    );
    assert_eq!(s.eval::<f64>(&count("GetNameForRace()")).unwrap(), 2.0);
    assert_eq!(s.eval::<f64>(&count("GetFactionForRace()")).unwrap(), 2.0);
    assert_eq!(
        s.eval::<(f64, f64)>("return GetSelectedRace(), GetSelectedSex()")
            .unwrap(),
        (1.0, 1.0)
    );
    s.script_glue_create_draws(&[word(1, 2), word(2, 3)]);
    s.run("ResetCharCustomize()").unwrap();
    assert_eq!(
        s.eval::<(String, String, String, String)>(
            "local a, b = GetNameForRace() local c, d = GetFactionForRace() return a, b, c, d"
        )
        .unwrap(),
        ("Orc".into(), "Orc".into(), "Horde".into(), "Horde".into())
    );
    assert_eq!(
        s.eval::<(String, String)>("return GetHairCustomization(), GetFacialHairCustomization()")
            .unwrap(),
        ("HORNS".into(), "PIERCINGS".into())
    );
}

/// The facing: degrees in, radians in the cell, degrees out; no gate, wrap or clamp; the setter
/// answers its last argument (`0x47196e`).
#[test]
fn the_facing_is_degrees_both_ways_with_no_gate() {
    let s = UiScript::new_glue().unwrap();
    s.run("SetCharacterCreateFacing(-15)").unwrap();
    assert!((s.glue_create_facing() + 15f32.to_radians()).abs() < 1e-6);
    let got = s.eval::<f64>("return GetCharacterCreateFacing()").unwrap();
    assert!((got + 15.0).abs() < 1e-4, "{got}");
    assert_eq!(
        s.eval::<(f64, String, f64)>(
            "local n = function(...) return arg.n end \
             return n(SetCharacterCreateFacing(5)), SetCharacterCreateFacing('45'), \
             SetCharacterCreateFacing(1, 7)"
        )
        .unwrap(),
        (1.0, "45".to_string(), 7.0)
    );
    s.run("SetCharacterCreateFacing(-725.5)").unwrap();
    let got = s.eval::<f64>("return GetCharacterCreateFacing()").unwrap();
    assert!((got + 725.5).abs() < 1e-3, "{got}");
    let err = s
        .run("SetCharacterCreateFacing('x')")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Usage: SetCharacterCreateFacing(degrees)"),
        "{err}"
    );
}

/// The facing survives a build (`0x470c33` re-reads it), the reset included.
#[test]
fn a_build_keeps_the_facing() {
    let mut s = vm();
    s.run("SetCharacterCreateFacing(30)").unwrap();
    s.script_glue_create_draws(&[0, 0, 0, 0, 0, 0, 0]);
    s.run("ResetCharCustomize() SetSelectedRace(2) SetSelectedSex(2)")
        .unwrap();
    let got = s.eval::<f64>("return GetCharacterCreateFacing()").unwrap();
    assert!((got - 30.0).abs() < 1e-4, "{got}");
}

/// The background is the frame's own `SetModel` (`0x4707c8`): nothing without the frame or a
/// name, and the scene keeps the fog `SetModel` installs. The scene the frame shows is the
/// create one.
#[test]
fn the_background_is_the_frames_set_model_and_keeps_its_fog() {
    let s = UiScript::new_glue().unwrap();
    s.run("CreateFrame('ModelFFX', 'Create') SetCharCustomizeBackground('a.mdx')")
        .unwrap();
    assert_eq!(s.glue_scene(), None, "no frame, no file");
    s.run(
        "SetCharCustomizeFrame('Create') SetCharCustomizeBackground('') \
         Create:SetFogColor(0.5, 0.5, 0.5) Create:SetFogFar(270)",
    )
    .unwrap();
    assert_eq!(s.glue_scene(), None, "an empty name");
    s.run("SetCharCustomizeBackground('UI_Orc.mdx')").unwrap();
    let pane = s.glue_scene().expect("the create scene");
    assert_eq!(pane.path, "UI_Orc.mdx");
    assert!(pane.create && !pane.select);
    assert_eq!(pane.fog.map(|f| f.far), Some(270.0));
    let err = s
        .run("SetCharCustomizeBackground(nil)")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Usage: SetCharCustomizeBackground(\"filename\")"),
        "{err}"
    );
}

/// A reset whose build fails leaves the old character detached (`0x470922`); a background
/// attaches it to the new scene (`0x4707ed`).
#[test]
fn a_failed_reset_detaches_until_a_background_attaches() {
    let mut s = human([0; 5]);
    let before = look(&s);
    let mut c = catalog();
    c.models.clear();
    s.set_glue_create_catalog(Rc::new(c));
    s.run("ResetCharCustomize()").unwrap();
    assert_eq!(s.glue_create_look(), None);
    assert_eq!(s.eval::<f64>("return GetSelectedRace()").unwrap(), 1.0);
    s.run("SetCharCustomizeBackground('UI_Human.mdx')").unwrap();
    assert_eq!(s.glue_create_look(), Some(before));
}

/// `UpdateCustomizationBackground` is `0x470cf0` on the selected index, which its own gate turns
/// away; `UpdateCustomizationScene` is the scene drive's. Neither draws nor changes anything.
#[test]
fn the_two_update_verbs_change_nothing() {
    let s = human([1, 2, 3, 4, 5]);
    let before = look(&s);
    assert_eq!(
        s.eval::<(f64, f64)>(
            "local n = function(...) return arg.n end \
             return n(UpdateCustomizationBackground()), n(UpdateCustomizationScene())"
        )
        .unwrap(),
        (0.0, 0.0)
    );
    assert_eq!(look(&s), before);
}

/// `0x475480`: the run from the first row of the race and sex goes on while the race holds, so a
/// male learns his race's female names too; a pair with no row keeps the table.
#[test]
fn the_name_generator_learns_a_run_by_race() {
    let rows = vec![
        name_row(1, 0, "Ab"),
        name_row(1, 1, "Cd"),
        name_row(2, 0, "Ef"),
    ];
    let keys = |g: &NameGen| -> Vec<String> {
        g.table
            .iter()
            .map(|(k, n)| format!("{}{n}", String::from_utf8_lossy(k)))
            .collect()
    };
    let mut g = NameGen::default();
    g.set(&rows, 1, 0);
    assert_eq!(keys(&g), ["_ab1", "_cd1", "ab1", "cd1"]);
    g.set(&rows, 1, 1);
    assert_eq!(keys(&g), ["_cd1", "cd1"]);
    g.set(&rows, 1, 2);
    assert_eq!(keys(&g), ["_cd1", "cd1"], "no row, the table stands");
    // Lowered, and a repeated window counted.
    g.set(&[name_row(5, 0, "AnAnan")], 5, 0);
    assert_eq!(keys(&g), ["_an1", "an1", "ana2", "nan2"]);
}

/// `0x475130`: an opening key is drawn among the `_` keys by count, then each next byte among
/// the keys opening with the last two; a two-byte key ends the name; the `_` is dropped and the
/// first byte raised.
#[test]
fn the_generator_walks_the_trigrams_by_count() {
    let mut g = NameGen::default();
    g.set(&[name_row(1, 0, "Abab"), name_row(1, 0, "Bab")], 1, 0);
    let keys: Vec<(String, u32)> = g
        .table
        .iter()
        .map(|(k, n)| (String::from_utf8_lossy(k).into_owned(), *n))
        .collect();
    assert_eq!(
        keys,
        [
            ("_ab".to_string(), 1),
            ("_ba".to_string(), 1),
            ("ab".to_string(), 2),
            ("aba".to_string(), 1),
            ("bab".to_string(), 2),
        ]
    );
    // "_ab", then of "ab"(2) and "aba"(1) the second: "aba" appends "a"; then of "ba": "bab"
    // appends "b"; then "ab" ends it.
    let mut words = Words::default();
    words
        .scripted
        .extend([word(0, 2), word(2, 3), 0, word(0, 3)]);
    assert_eq!(g.generate(&mut words, NAME_BUFFER), b"Abab");
    // The buffer's cap ends the name at 12 bytes past the `_`.
    let mut words = Words::default();
    words.scripted.extend([word(0, 2)]);
    words.scripted.extend(std::iter::repeat_n(word(2, 3), 20));
    let long = g.generate(&mut words, NAME_BUFFER);
    assert_eq!(long.len(), NAME_BUFFER - 2);
    // With no table the name is empty.
    assert!(NameGen::default()
        .generate(&mut Words::default(), NAME_BUFFER)
        .is_empty());
}

/// `GetRandomName` answers the first name that passes the check and runs 4 to 10 bytes, one
/// value; with nothing learned it gives up with nil.
#[test]
fn get_random_name_answers_a_passing_name() {
    let mut s = human([0; 5]);
    // Human male learned "Abe" and "Ada"; any name they make is three bytes, too short.
    assert_eq!(
        s.eval::<Option<String>>("return GetRandomName()").unwrap(),
        None
    );
    let mut c = catalog();
    c.names = vec![name_row(1, 0, "Abcde"), name_row(1, 0, "Fghij")];
    s.set_glue_create_catalog(Rc::new(c));
    s.script_glue_create_draws(&[0; 7]);
    s.run("ResetCharCustomize()").unwrap();
    let name = s.eval::<String>("return GetRandomName()").unwrap();
    assert!(name == "Abcde" || name == "Fghij", "{name}");
    assert_eq!(s.eval::<f64>(&count("GetRandomName()")).unwrap(), 1.0);
}

/// `0x4711d0`: a failing name opens `OKAY` with the check's text and sends nothing; a missing one
/// is `CHAR_NAME_NO_NAME` with no check; a passing one sends the character's bytes. No values,
/// no usage error.
#[test]
fn create_character_checks_the_name_then_asks_the_host() {
    let mut s = human([1, 2, 3, 4, 5]);
    s.run("SetSelectedClass(3) CHAR_NAME_TOO_SHORT = 'short' CHAR_NAME_NO_NAME = 'none'")
        .unwrap();
    s.take_glue_calls();
    for (call, text) in [
        ("CreateCharacter('A')", "short"),
        ("CreateCharacter()", "none"),
        ("CreateCharacter({})", "none"),
        ("CreateCharacter('')", "none"),
        ("CreateCharacter('\\0Abc')", "none"),
    ] {
        s.run("FIRED = {}").unwrap();
        assert_eq!(s.eval::<f64>(&count(call)).unwrap(), 0.0, "{call}");
        assert_eq!(
            fired(&s),
            [format!("OPEN_STATUS_DIALOG OKAY {text}")],
            "{call}"
        );
    }
    // A number is a name too, and no letter.
    s.run("FIRED = {} CreateCharacter(12)").unwrap();
    assert_eq!(fired(&s), ["OPEN_STATUS_DIALOG OKAY "]);
    assert!(s.take_glue_calls().is_empty());

    s.run("FIRED = {} CreateCharacter('Zoë\\0Bob')").unwrap();
    assert!(fired(&s).is_empty());
    let queued = s.take_glue_calls();
    assert!(
        matches!(
            queued.as_slice(),
            [GlueCall::CreateCharacter(GlueCreateRequest {
                name,
                race: 1,
                class: 4,
                sex: 0,
                skin: 1,
                face: 2,
                hair_style: 3,
                hair_color: 4,
                facial_hair: 5,
            })] if name == "Zoë"
        ),
        "{queued:?}"
    );
}

/// With no character built nothing is sent; the check still runs.
#[test]
fn create_character_with_no_character_sends_nothing() {
    let mut s = vm();
    s.run("CreateCharacter('Newbie')").unwrap();
    assert!(s.take_glue_calls().is_empty());
    assert!(fired(&s).is_empty());
}
