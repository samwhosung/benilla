//! The stock Video Options window (`OptionsFrame.xml`/`.lua`, off the player's chain) standing on
//! its own: open, every box and slider, Okay, Defaults and Cancel, with no Lua error and the
//! values each verb stores.

use benilla_ui::script::{ScreenResolution, UiScript, UnitState, VideoCaps};

/// A loaded default UI with the display facts the window's `OnLoad`s and `_Load` read.
fn video_ui() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_screen_resolutions(
        vec![
            ScreenResolution {
                width: 1280,
                height: 720,
            },
            ScreenResolution {
                width: 1920,
                height: 1080,
            },
        ],
        Some(ScreenResolution {
            width: 1600,
            height: 900,
        }),
    );
    s.set_video_caps(VideoCaps {
        anisotropic: true,
        pixel_shaders: true,
        vertex_shaders: true,
        trilinear: true,
        triple_buffering: false,
        max_anisotropy: 16,
        hardware_cursor: true,
    });
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s
}

fn cvar(s: &UiScript, name: &str) -> Option<String> {
    s.eval::<Option<String>>(&format!("return GetCVar({name:?})"))
        .unwrap()
}

fn slider(s: &UiScript, i: usize) -> f64 {
    s.eval::<f64>(&format!("return OptionsFrameSlider{i}:GetValue()"))
        .unwrap()
}

fn assert_clean(s: &UiScript, step: &str) {
    let errors = s.errors();
    assert!(errors.is_empty(), "{step} raised: {errors:#?}");
    let unknown: Vec<String> = s
        .warnings()
        .into_iter()
        .filter(|w| w.contains("unknown CVar"))
        .collect();
    assert!(
        unknown.is_empty(),
        "{step} touched an unregistered CVar: {unknown:#?}"
    );
}

/// Every box's `(cvar, stored value, checked)`, over the window's own table
/// (`OptionsFrameCheckButtons`, `OptionsFrame.lua:5-22`); the stored value is `"nil"` when absent.
fn boxes(s: &UiScript) -> Vec<(String, String, bool, bool)> {
    s.eval::<Vec<String>>(
        r#"local out = {}
        for _, v in OptionsFrameCheckButtons do
            local b = getglobal("OptionsFrameCheckButton"..v.index)
            local bit = function(x) if x then return "1" else return "0" end end
            table.insert(out, v.cvar.."|"..(GetCVar(v.cvar) or "nil").."|"..bit(b:GetChecked())
                .."|"..bit(b:IsEnabled() == 1))
        end
        return out"#,
    )
    .unwrap()
    .into_iter()
    .map(|row| {
        let f: Vec<&str> = row.split('|').collect();
        (f[0].to_string(), f[1].to_string(), f[2] == "1", f[3] == "1")
    })
    .collect()
}

/// Open, walk every box and slider, Okay; reopen and Defaults; reopen, move the gamma and Cancel.
/// Each step runs the stock Lua to its end: an aborted `OptionsFrame_Load` leaves
/// `OptionsFrame.gamma` nil for `OnHide`'s `SetGamma` (`OptionsFrame.lua:260`).
#[test]
fn the_stock_video_window_opens_saves_restores_defaults_and_cancels_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = video_ui();

    // ── Open: `OptionsFrame_Load` reads every slider through its verb or its CVar. ──
    s.run("ShowUIPanel(OptionsFrame)").unwrap();
    assert_clean(&s, "opening the window");
    assert!(s.eval::<bool>("return OptionsFrame:IsShown()").unwrap());
    // Environment Detail: `GetWorldDetail`, stop 1 at the registered `SmallCull` 0.04.
    assert_eq!(slider(&s, 3), 1.0);
    // Terrain Mip: `GetTerrainMip` is `1 - shadowLevel` (`0x488fbd`), `shadowLevel` "1".
    assert_eq!(slider(&s, 4), 0.0);
    // Texture Detail: `GetBaseMip` is `1 - baseMip` (`0x48928d`), `baseMip` "0".
    assert_eq!(slider(&s, 5), 1.0);
    // Spell Detail: the CVar path, `spellEffectLevel` "2".
    assert_eq!(slider(&s, 8), 2.0);
    // `OptionsFrame_Load:153-154` recorded both for `OnHide`.
    assert_eq!(s.eval::<f64>("return OptionsFrame.gamma").unwrap(), 0.0);
    assert_eq!(
        s.eval::<Option<String>>("return OptionsFrame.desktopGamma")
            .unwrap()
            .as_deref(),
        Some("0"),
        "`DesktopGamma` registers \"0\" (`0x402d4d`)"
    );

    // Every box shows its CVar's stored value: all eighteen are registered, and an enabled box is
    // ticked exactly when its value is "1" (`OptionsFrame_Load`'s `EnableCheckBox(button, 1,
    // GetCVar(...))`).
    let at_open = boxes(&s);
    assert_eq!(at_open.len(), 18);
    for (cvar, value, checked, enabled) in &at_open {
        assert!(value == "0" || value == "1", "{cvar} stores {value:?}");
        if *enabled {
            assert_eq!(*checked, value == "1", "{cvar}'s box shows its value");
        }
    }

    // ── Every box, then every slider to a position off its default. ──
    for i in 1..=18 {
        s.run(&format!("OptionsFrameCheckButton{i}:Click()"))
            .unwrap();
    }
    for (i, v) in [
        (1, "1.0"),
        (2, "777"),
        (3, "2"),
        (4, "1"),
        (5, "0"),
        (6, "0.5"),
        (7, "5"),
        (8, "0"),
        (9, "0"),
    ] {
        s.run(&format!("OptionsFrameSlider{i}:SetValue({v})"))
            .unwrap();
    }
    assert_clean(&s, "walking the boxes and sliders");

    // ── Okay: `OptionsFrame_Save`, then `OnHide`'s `OptionsFrame_Cancel`. ──
    s.run("OptionsFrameOkay:Click()").unwrap();
    assert_clean(&s, "Okay");
    assert!(!s.eval::<bool>("return OptionsFrame:IsShown()").unwrap());
    // `SetWorldDetail(2)`: `frillDensity` 48 and `smallCull` "%f" of 0.01 (`0x488e1e`, `0x488e56`).
    assert_eq!(cvar(&s, "frillDensity").as_deref(), Some("48"));
    assert_eq!(cvar(&s, "SmallCull").as_deref(), Some("0.010000"));
    assert_eq!(s.eval::<f64>("return GetWorldDetail()").unwrap(), 2.0);
    // `SetTerrainMip(1)` writes `1 - 1` as "%d" (`0x48901d`-`0x489025`).
    assert_eq!(cvar(&s, "shadowLevel").as_deref(), Some("0"));
    // `SetBaseMip(0)` writes `trunc(1 - 0)` as "%d" (`0x4892f5`-`0x489301`).
    assert_eq!(cvar(&s, "baseMip").as_deref(), Some("1"));
    assert_eq!(cvar(&s, "spellEffectLevel").as_deref(), Some("0"));
    assert_eq!(cvar(&s, "farclip").as_deref(), Some("777"));
    assert_eq!(cvar(&s, "gamma").as_deref(), Some("0.500000"));
    // The Use Desktop Gamma box was ticked, `_Save` wrote "1" and the recorded value follows it.
    assert_eq!(cvar(&s, "desktopGamma").as_deref(), Some("1"));
    // `_Save` stored every box as it was left, and each moved box kept its click.
    let saved = boxes(&s);
    for (cvar, value, checked, _) in &saved {
        assert_eq!(
            value,
            if *checked { "1" } else { "0" },
            "{cvar} kept its box"
        );
    }
    let moved: Vec<&str> = at_open
        .iter()
        .zip(&saved)
        .filter(|(a, b)| a.1 != b.1)
        .map(|(a, _)| a.0.as_str())
        .collect();
    for cvar in [
        "ffxGlow",
        "specular",
        "movieSubtitle",
        "lod",
        "gxCursor",
        "useWeatherShaders",
    ] {
        assert!(
            moved.contains(&cvar),
            "{cvar} was clicked and saved: {moved:?}"
        );
    }
    // Enable All Shaders is copied into `ffx` (`OptionsFrame.lua:202-204`).
    assert_eq!(cvar(&s, "ffx"), cvar(&s, "pixelShaders"));

    // ── Defaults: `OptionsFrame_SetDefaults` reads `GetCVarDefault` for `smallCull`,
    //    `shadowLevel` and `baseMip` (`:431-443`) and ends in `RestoreVideoDefaults`. ──
    s.run("ShowUIPanel(OptionsFrame)").unwrap();
    assert_clean(&s, "reopening for Defaults");
    s.run("OptionsFrameDefaults:Click()").unwrap();
    assert_clean(&s, "Defaults");
    assert!(!s.eval::<bool>("return OptionsFrame:IsShown()").unwrap());
    // `RestoreVideoDefaults` restores the graphics CVars benilla registers among `0x639a60`'s.
    for (name, default) in [
        ("farclip", "350"),
        ("SmallCull", "0.04"),
        ("shadowLevel", "1"),
        ("baseMip", "0"),
        ("doodadAnim", "1"),
    ] {
        assert_eq!(cvar(&s, name).as_deref(), Some(default), "{name} restored");
    }

    // ── Cancel: `OnHide` puts back the gamma and desktop gamma recorded at open. ──
    s.run("ShowUIPanel(OptionsFrame)").unwrap();
    let gamma_at_open = cvar(&s, "gamma");
    let desktop_at_open = cvar(&s, "desktopGamma");
    s.run("OptionsFrameCheckButton1:SetChecked(nil) OptionsFrameSlider6:SetValue(-0.5) SetGamma(-0.5)")
        .unwrap();
    s.run("OptionsFrameCancel:Click()").unwrap();
    assert_clean(&s, "Cancel");
    assert!(!s.eval::<bool>("return OptionsFrame:IsShown()").unwrap());
    assert_eq!(
        cvar(&s, "gamma"),
        gamma_at_open,
        "Cancel restores the gamma"
    );
    assert_eq!(
        cvar(&s, "desktopGamma"),
        desktop_at_open,
        "…and the desktop gamma (`OptionsFrame.lua:261`)"
    );
}
