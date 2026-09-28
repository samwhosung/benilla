//! The host's state lives off `_G`: 1.12.1's global table has no name for the clock, the zone
//! caches, the game clock or the error-handler slot (`reference/1.12-globals.tsv`), and the
//! getters over them read the client's own state, which Lua cannot reach. So a global an addon
//! assigns under one of benilla's old host names is a global no getter reads.

use super::common::script;
use crate::script::{ActionState, AuraState, UiScript};

/// Each old host name, the Lua that writes it, and a probe of every answer it once fed. The probe
/// is read before and after the write; the two must agree, and the name must be absent from `_G`.
const HOST_NAMES: &[(&str, &str, &str)] = &[
    (
        "__benilla_now",
        "__benilla_now = 0",
        "return GetTime() .. ' ' .. GetPlayerBuffTimeLeft(0)",
    ),
    (
        "__benilla_zone_name",
        "__benilla_zone_name = 'x'",
        "return GetZoneText()",
    ),
    (
        "__benilla_real_zone_name",
        "__benilla_real_zone_name = 'x'",
        "return GetRealZoneText()",
    ),
    (
        "__benilla_subzone_name",
        "__benilla_subzone_name = 'x'",
        "return GetSubZoneText()",
    ),
    (
        "__benilla_zone_text",
        "__benilla_zone_text = 'x'",
        "return GetMinimapZoneText()",
    ),
    (
        "__benilla_pvp_type",
        "__benilla_pvp_type = 'hostile'",
        "return tostring((GetZonePVPInfo()))",
    ),
    (
        "__benilla_pvp_faction",
        "__benilla_pvp_faction = 'Horde'",
        "local _, f = GetZonePVPInfo() return tostring(f)",
    ),
    (
        "__benilla_pvp_arena",
        "__benilla_pvp_arena = true",
        "local _, _, a = GetZonePVPInfo() return tostring(a)",
    ),
    (
        "__benilla_game_hour",
        "__benilla_game_hour = 13",
        "local h, m = GetGameTime() return h .. ':' .. m",
    ),
    (
        "__benilla_game_minute",
        "__benilla_game_minute = 59",
        "local h, m = GetGameTime() return h .. ':' .. m",
    ),
    (
        "__benilla_script_error",
        "__benilla_script_error = function() ErrorSinkHit = 1 end",
        "local h = geterrorhandler() if h then pcall(h, 'probe') end \
         return tostring(ErrorSinkHit)",
    ),
];

/// A VM a few seconds into its session with one buff ticking, so the clock's probe has a value
/// worth rewinding.
fn session() -> UiScript {
    let mut s = script();
    s.set_now(500.0);
    s.tick(0.5);
    s.set_player_auras(vec![AuraState {
        spell_id: 1243,
        count: 1,
        duration: 1800.0,
        expiration_time: 1700.0,
        helpful: true,
        cancelable: true,
        ..Default::default()
    }]);
    s
}

/// No old host name is a global, and assigning one changes no answer it once fed.
#[test]
fn a_host_name_is_absent_from_g_and_writing_it_changes_nothing() {
    let mut failures = Vec::new();
    for &(name, write, probe) in HOST_NAMES {
        let s = session();
        if !s
            .eval::<bool>(&format!("return rawget(getfenv(0), '{name}') == nil"))
            .unwrap()
        {
            failures.push(format!("{name} is a global"));
        }
        let before = s.eval::<String>(probe).unwrap();
        s.run(write).unwrap();
        let after = s.eval::<String>(probe).unwrap();
        if before != after {
            failures.push(format!(
                "`{write}` moved `{probe}` from {before:?} to {after:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The clock an addon cannot rewind: after `__benilla_now = 0`, the next tick advances `GetTime`
/// from where it was, and every countdown on it (a buff's time left, an action's cooldown) reads
/// on as before.
#[test]
fn rewinding_the_old_clock_name_leaves_the_session_clock_running() {
    let mut s = session();
    s.set_action_state(
        1,
        Some(ActionState {
            cooldown: Some((490_000, 30_000, true)),
            ..Default::default()
        }),
    );
    let read = |s: &UiScript| {
        s.eval::<(f64, f64, f64)>(
            "local start, duration = GetActionCooldown(1) \
             return GetTime(), GetPlayerBuffTimeLeft(0), start + duration",
        )
        .unwrap()
    };
    let (t0, left0, cd0) = read(&s);
    assert_eq!((t0, left0, cd0), (500.5, 1199.5, 520.0));
    s.run("__benilla_now = 0").unwrap();
    s.tick(0.25);
    let (t1, left1, cd1) = read(&s);
    assert_eq!(t1, 500.75, "the tick advances from the session clock");
    assert_eq!(left1, 1199.25);
    assert_eq!(cd1, 520.0, "the running cooldown still reads as running");
    assert_eq!(s.now(), 500.75, "and the host reads the same clock");
}

/// The getters answer what the host pushed, in the reference's shapes: the four zone texts one
/// string each, never nil (`0x48a0a0`..`0x48a100`); `GetZonePVPInfo` three values, isArena 1 or
/// nil (`0x48d540`); `GetGameTime` two numbers (`0x515ee0`); `GetTime` one (`0x515ea0`).
#[test]
fn the_getters_answer_the_pushed_state_in_their_1_12_shapes() {
    let mut s = script();
    for call in [
        "GetZoneText()",
        "GetRealZoneText()",
        "GetSubZoneText()",
        "GetMinimapZoneText()",
        "GetTime()",
    ] {
        assert_eq!(s.arity(call).unwrap(), 1, "{call}");
    }
    assert_eq!(s.arity("GetZonePVPInfo()").unwrap(), 3);
    assert_eq!(s.arity("GetGameTime()").unwrap(), 2);
    assert_eq!(
        s.eval::<String>(
            "local t, f, a = GetZonePVPInfo() \
             return GetZoneText() .. '|' .. GetSubZoneText() .. '|' .. GetMinimapZoneText() .. '|' \
             .. tostring(t) .. '|' .. tostring(f) .. '|' .. tostring(a)"
        )
        .unwrap(),
        "|||nil|nil|nil",
        "before the first push: empty strings, never nil, and no PvP info"
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return GetGameTime()").unwrap(),
        (0.0, 0.0)
    );

    s.set_zone_texts(crate::script::ZoneTexts {
        zone: "Stormwind City".into(),
        real_zone: "Elwynn Forest".into(),
        subzone: "".into(),
        minimap: "Stormwind City".into(),
        pvp_type: Some("friendly".into()),
        pvp_faction: Some("Alliance".into()),
        is_arena: true,
    });
    s.set_game_time(21, 7);
    assert_eq!(
        s.eval::<(String, String, String, String)>(
            "return GetZoneText(), GetRealZoneText(), GetSubZoneText(), GetMinimapZoneText()"
        )
        .unwrap(),
        (
            "Stormwind City".into(),
            "Elwynn Forest".into(),
            String::new(),
            "Stormwind City".into()
        )
    );
    assert_eq!(
        s.eval::<(String, String, f64)>("return GetZonePVPInfo()")
            .unwrap(),
        ("friendly".into(), "Alliance".into(), 1.0)
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return GetGameTime()").unwrap(),
        (21.0, 7.0)
    );
}

/// The reference's handler slot starts empty (`[0x8722cc]` = -1 at `0x7039e0`), so
/// `geterrorhandler()` (`0x702950`) answers one nil until FrameXML sets `_ERRORMESSAGE`.
/// `seterrorhandler` (`0x702900`) takes a function alone, raising `Usage: seterrorhandler(errfunc)`
/// (`0x872a30`) on anything else and keeping the slot, and returns nothing.
#[test]
fn the_error_handler_slot_starts_empty_and_takes_only_a_function() {
    let s = script();
    assert_eq!(s.arity("geterrorhandler()").unwrap(), 1);
    assert!(s.eval::<bool>("return geterrorhandler() == nil").unwrap());

    s.run("Handler = function() end").unwrap();
    assert_eq!(s.arity("seterrorhandler(Handler)").unwrap(), 0);
    assert!(s
        .eval::<bool>("return geterrorhandler() == Handler")
        .unwrap());
    for bad in ["nil", "", "'Handler'", "{}"] {
        let e = s
            .run(&format!("seterrorhandler({bad})"))
            .expect_err(bad)
            .to_string();
        assert!(e.contains("Usage: seterrorhandler(errfunc)"), "{bad}: {e}");
        assert!(
            s.eval::<bool>("return geterrorhandler() == Handler")
                .unwrap(),
            "seterrorhandler({bad}) keeps the slot"
        );
    }
}

/// The engine calls the handler in its slot on a caught error (`0x703b40`), not whatever the
/// `geterrorhandler` global answers, so an addon replacing that global redirects nothing.
#[test]
fn the_engine_reads_the_handler_slot_not_the_geterrorhandler_global() {
    let mut s = script();
    s.run(
        "Heard, Decoy = {}, {} \
         seterrorhandler(function(m) table.insert(Heard, m) end) \
         geterrorhandler = function() return function(m) table.insert(Decoy, m) end end \
         local f = CreateFrame('Frame') \
         f:RegisterEvent('ERROR_PROBE') \
         f:SetScript('OnEvent', function() error('slot probe') end)",
    )
    .unwrap();
    s.fire_event("ERROR_PROBE", vec![]);
    s.dispatch_script_errors_to_handler();
    assert_eq!(
        s.eval::<(i64, i64)>("return table.getn(Heard), table.getn(Decoy)")
            .unwrap(),
        (1, 0)
    );
}
