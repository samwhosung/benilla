//! The `SetCVar` (`0x488c10`) and `RegisterCVar` (`0x488b00`) bindings: how each reads its
//! arguments, the read-only flag, and when `CVAR_UPDATE` fires.

use crate::script::UiScript;

fn script_with(rows: &[(&str, &str)]) -> UiScript {
    let s = UiScript::new().unwrap();
    s.register_cvars(rows.iter().copied());
    s
}

fn get(s: &UiScript, name: &str) -> String {
    s.eval::<String>(&format!("return GetCVar({name:?})"))
        .unwrap()
}

fn raised(s: &UiScript, chunk: &str) -> String {
    s.run(chunk)
        .expect_err(&format!("{chunk} must raise"))
        .to_string()
}

/// `lua_tostring` on argument 2 returns NULL for anything but a string or a number, and the
/// binding stores `"0"` for it (`0x488c8d`-`0x488c98`): an unticked box's `GetChecked()` is nil.
#[test]
fn set_cvar_stores_zero_for_a_value_with_no_string_form() {
    let mut s = script_with(&[("EnableMusic", "1")]);
    for value in ["nil", "true", "false", "{}", "print"] {
        s.set_cvar_host("EnableMusic", "1");
        s.run(&format!("SetCVar(\"EnableMusic\", {value})"))
            .unwrap_or_else(|e| panic!("SetCVar with {value} raised: {e}"));
        assert_eq!(get(&s, "EnableMusic"), "0", "SetCVar with {value}");
        assert_eq!(
            s.take_cvar_changes(),
            vec![("EnableMusic".to_string(), "0".to_string())],
            "SetCVar with {value} is a real write"
        );
    }
    s.set_cvar_host("EnableMusic", "1");
    s.run("SetCVar(\"EnableMusic\")").unwrap();
    assert_eq!(get(&s, "EnableMusic"), "0", "an absent value");
}

/// A number goes through `luaV_tostring` (`0x6f7c80`), MSVC's `%.14g`; a string is kept as is.
#[test]
fn set_cvar_stores_a_number_as_lua_text() {
    let s = script_with(&[("MusicVolume", "1.0")]);
    for (lua, stored) in [
        // The stock Sound slider's single-precision 0.4, widened.
        ("0.4000000059604645", "0.40000000596046"),
        ("0.7", "0.7"),
        ("3", "3"),
        ("-2.5", "-2.5"),
        ("1e20", "1e+020"),
        ("0.00001", "1e-005"),
        ("\"0.4000000059604645\"", "0.4000000059604645"),
    ] {
        s.run(&format!("SetCVar(\"MusicVolume\", {lua})")).unwrap();
        assert_eq!(get(&s, "MusicVolume"), stored, "SetCVar with {lua}");
    }
}

/// `lua_isstring(1)` fails (`0x488c1a`) and the binding raises the reference's own usage text
/// (`0x84235c`); a number passes and names the CVar by its text.
#[test]
fn set_cvar_raises_the_reference_usage_without_a_name() {
    let s = script_with(&[("5", "a")]);
    for chunk in [
        "SetCVar()",
        "SetCVar(nil, 1)",
        "SetCVar({}, 1)",
        "SetCVar(true, 1)",
    ] {
        assert!(
            raised(&s, chunk).contains("Usage: SetCVar(\"cvar\", value [, \"scriptCvar\")"),
            "{chunk}"
        );
    }
    s.run("SetCVar(5, \"b\")").unwrap();
    assert_eq!(get(&s, "5"), "b");
}

/// Flag bit2 (`0x488c67`-`0x488c78`): the in-game session flags three rows (`0x48f566`), and
/// only the Lua binding tests it; before the flag the same write lands.
#[test]
fn set_cvar_refuses_a_read_only_row_only_once_flagged() {
    let mut s = script_with(&[
        ("realmList", "us.logon.worldofwarcraft.com"),
        ("realmName", ""),
        ("scriptMemory", "0"),
        ("MusicVolume", "1.0"),
    ]);
    s.run(
        "SEEN = 0 \
         f = CreateFrame(\"Frame\") \
         f:RegisterEvent(\"CVAR_UPDATE\") \
         f:SetScript(\"OnEvent\", function() SEEN = SEEN + 1 end)",
    )
    .unwrap();
    for name in crate::script::IN_WORLD_READ_ONLY_CVARS {
        s.run(&format!("SetCVar({name:?}, \"before\")")).unwrap();
        assert_eq!(
            get(&s, name),
            "before",
            "{name} is writable before the flag"
        );
    }
    s.take_cvar_changes();

    for name in crate::script::IN_WORLD_READ_ONLY_CVARS {
        s.set_cvar_read_only(name, true);
    }
    for name in crate::script::IN_WORLD_READ_ONLY_CVARS {
        let upper = name.to_ascii_uppercase();
        assert_eq!(
            raised(&s, &format!("SetCVar({upper:?}, \"after\", \"TOKEN\")"))
                .lines()
                .next(),
            Some(format!("runtime error: \"{upper}\" is read-only").as_str()),
            "the name as passed, any case"
        );
        assert_eq!(get(&s, name), "before", "{name} kept its value");
    }
    assert!(s.take_cvar_changes().is_empty(), "nothing was written");
    assert_eq!(
        s.eval::<f64>("return SEEN").unwrap(),
        0.0,
        "a refused write fires nothing"
    );
    s.run("SetCVar(\"MusicVolume\", 0.5)").unwrap();
    assert_eq!(get(&s, "MusicVolume"), "0.5", "other rows stay writable");

    // The host's and the console's writes go through `CVar::Set` (`0x63df50`), which never
    // tests the flag.
    s.set_cvar_engine("realmList", "logon.example.org");
    assert_eq!(get(&s, "realmList"), "logon.example.org");
    s.run("ConsoleExec(\"realmName Elsewhere\")").unwrap();
    assert_eq!(get(&s, "realmName"), "Elsewhere");

    // `ShutdownGame`'s clear (`0x491252`).
    s.set_cvar_read_only("realmList", false);
    s.run("SetCVar(\"realmList\", \"again\")").unwrap();
    assert_eq!(get(&s, "realmList"), "again");
}

/// A re-seed from the host replaces every row, and the flag outlives it, as the reference's
/// rows keep their flags through every write.
#[test]
fn the_read_only_flag_survives_a_host_reseed() {
    let mut s = script_with(&[("realmName", "")]);
    s.set_cvar_read_only("realmName", true);
    s.seed_cvars([crate::script::SeededCvar {
        name: "realmName".into(),
        value: "Here".into(),
        default: String::new(),
        latched: false,
    }]);
    assert!(raised(&s, "SetCVar(\"realmName\", \"x\")").contains("is read-only"));
}

/// `CVAR_UPDATE` fires whenever argument 3 passes `lua_isstring`, changed or not, with the token
/// as arg1 and the value text as arg2 (`0x488cad`-`0x488cd5`), inside the call: `SignalEvent2`
/// (`0x703f50`) walks its listeners in place.
#[test]
fn set_cvar_fires_cvar_update_inside_the_call_for_every_token() {
    let s = script_with(&[("MusicVolume", "1.0")]);
    s.run(
        "SEEN = {} \
         f = CreateFrame(\"Frame\") \
         f:RegisterEvent(\"CVAR_UPDATE\") \
         f:SetScript(\"OnEvent\", function() table.insert(SEEN, arg1 .. \"=\" .. arg2) end)",
    )
    .unwrap();
    let seen = |s: &UiScript| -> Vec<String> { s.eval("return SEEN").unwrap() };

    // No token, or one with no string form: nothing fires.
    for chunk in [
        "SetCVar(\"MusicVolume\", \"0.5\")",
        "SetCVar(\"MusicVolume\", \"0.5\", nil)",
        "SetCVar(\"MusicVolume\", \"0.5\", true)",
        "SetCVar(\"MusicVolume\", \"0.5\", {})",
    ] {
        s.run(chunk).unwrap();
        assert!(seen(&s).is_empty(), "{chunk} fires nothing");
    }

    // Seen by the next statement of the same chunk, with no tick between.
    assert_eq!(
        s.eval::<f64>("SetCVar(\"MusicVolume\", \"0.6\", \"MUSIC_VOLUME\") return getn(SEEN)")
            .unwrap(),
        1.0,
        "the handler ran before SetCVar returned"
    );
    // The same value again still fires.
    s.run("SetCVar(\"MusicVolume\", \"0.6\", \"MUSIC_VOLUME\")")
        .unwrap();
    // A number token is its text; a value with no string form is "0".
    s.run("SetCVar(\"MusicVolume\", nil, 7)").unwrap();
    s.run("SetCVar(\"MusicVolume\", 0.25, \"VOL\")").unwrap();
    assert_eq!(
        seen(&s),
        vec!["MUSIC_VOLUME=0.6", "MUSIC_VOLUME=0.6", "7=0", "VOL=0.25"]
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `RegisterCVar` takes argument 2 as the default only when `lua_isstring` passes (`0x488b56`),
/// else `"0"` (`0x488b6f`); a number default is `%.14g` text. Without a name it raises its usage
/// text (`0x8422e8`).
#[test]
fn register_cvar_defaults_to_zero_and_reads_numbers_as_lua_text() {
    let mut s = UiScript::new().unwrap();
    for (name, arg, stored) in [
        ("Bare", "", "0"),
        ("Nil", ", nil", "0"),
        ("True", ", true", "0"),
        ("Table", ", {}", "0"),
        ("Float", ", 0.4000000059604645", "0.40000000596046"),
        ("Int", ", 3", "3"),
        ("Text", ", \"on\"", "on"),
    ] {
        s.run(&format!("RegisterCVar({name:?}{arg})")).unwrap();
        assert_eq!(get(&s, name), stored, "RegisterCVar({name:?}{arg})");
        assert_eq!(
            s.eval::<String>(&format!("return GetCVarDefault({name:?})"))
                .unwrap(),
            stored,
            "the default too"
        );
    }
    assert!(s
        .take_cvar_registrations()
        .contains(&("Bare".to_string(), "0".to_string())));
    assert!(
        s.eval::<bool>("return tonumber(GetCVar(\"Bare\")) == 0")
            .unwrap(),
        "an addon's tonumber of a bare registration is 0, not nil"
    );
    for chunk in [
        "RegisterCVar()",
        "RegisterCVar(nil, \"1\")",
        "RegisterCVar({})",
    ] {
        assert!(
            raised(&s, chunk).contains("Usage: RegisterCVar(\"cvar\" [, default])"),
            "{chunk}"
        );
    }
}
