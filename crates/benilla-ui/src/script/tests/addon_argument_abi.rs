//! The `index or "name"` prologue of `GetAddOnInfo 0x48e390`, `GetAddOnMetadata 0x48e530`,
//! `GetAddOnDependencies 0x48e5e0`, `EnableAddOn 0x48e690`, `DisableAddOn 0x48e760`,
//! `IsAddOnLoadOnDemand 0x48e840`, `IsAddOnLoaded 0x48e8e0` and `LoadAddOn 0x48e980`; both its
//! raises are `luaL_error` (`0x6f4940`), which does not return.

use crate::script::{AddOnInfo, UiScript};

fn seeded() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.register_addons(
        vec![
            AddOnInfo {
                name: "Alpha".into(),
                title: Some("Alpha Title".into()),
                interface: 11200,
                enabled: true,
                ..Default::default()
            },
            AddOnInfo {
                name: "Beta".into(),
                interface: 11200,
                enabled: true,
                ..Default::default()
            },
        ],
        None,
        None,
        None,
    );
    // The index space exists only once the server answers; with no reply every bound is zero.
    s.note_addon_info_reply(&[]);
    s
}

fn raised(s: &UiScript, call: &str) -> String {
    s.eval::<String>(&format!(
        "local ok, e = pcall(function() {call} end) \
         if ok then return \"<no raise>\" end return tostring(e)"
    ))
    .unwrap()
}

/// The client's own literals (`.data` `0x842d68` to `0x842e98`); addons see the spelling.
#[test]
fn a_bad_argument_type_raises_each_verbs_own_usage_string() {
    let s = seeded();
    for (call, usage) in [
        ("GetAddOnInfo({})", "Usage: GetAddOnInfo(index or \"name\")"),
        (
            "GetAddOnMetadata({}, \"Version\")",
            "Usage: GetAddOnMetadata(index or \"name\", \"variable\")",
        ),
        (
            "GetAddOnDependencies({})",
            "Usage: GetAddOnDependencies(index or \"name\")",
        ),
        ("EnableAddOn({})", "Usage: EnableAddOn(index or \"name\")"),
        ("DisableAddOn({})", "Usage: DisableAddOn(index or \"name\")"),
        (
            "IsAddOnLoadOnDemand({})",
            "Usage: IsAddOnLoadOnDemand(index or \"name\")",
        ),
        (
            "IsAddOnLoaded({})",
            "Usage: IsAddOnLoaded(index or \"name\")",
        ),
        ("LoadAddOn({})", "Usage: LoadAddOn(index or \"name\")"),
    ] {
        let got = raised(&s, call);
        assert!(
            got.contains(usage),
            "{call} must raise `{usage}`, got: {got}"
        );
    }
    // `lua_isnumber` reports a slot past `L->top` as not a number, so nil or no argument raises.
    for call in ["LoadAddOn(nil)", "LoadAddOn()", "IsAddOnLoaded(true)"] {
        assert!(
            raised(&s, call).contains("index or \"name\""),
            "{call}: {}",
            raised(&s, call)
        );
    }
    // The second argument's own `lua_isstring` (`0x48e59c`) fails onto the same raise.
    assert!(raised(&s, "GetAddOnMetadata(1)").contains("Usage: GetAddOnMetadata"));
}

/// `"AddOn index must be in the range of 1 to %d"` (`0x837d70`, `%d` from `0x51def0()`). The
/// bound is unsigned (`0x51df00 cmp ecx,[0xbe1b90]; jb`): 0 wraps to `0xFFFFFFFF` and raises.
#[test]
fn an_out_of_range_index_raises_with_the_registrys_own_count() {
    let s = seeded();
    let want = "AddOn index must be in the range of 1 to 2";
    for verb in [
        "GetAddOnInfo",
        "GetAddOnDependencies",
        "EnableAddOn",
        "DisableAddOn",
        "IsAddOnLoadOnDemand",
        "IsAddOnLoaded",
        "LoadAddOn",
    ] {
        for arg in ["0", "3", "-1", "2.9"] {
            // 2.9 truncates to 2 (`_ftol 0x40a2b0`), which is in range.
            let got = raised(&s, &format!("{verb}({arg})"));
            if arg == "2.9" {
                assert!(!got.contains("must be in the range"), "{verb}(2.9): {got}");
            } else {
                assert!(got.contains(want), "{verb}({arg}) must raise: {got}");
            }
        }
    }
    assert!(raised(&s, "GetAddOnMetadata(3, \"Version\")").contains(want));
}

/// `lua_isnumber` (`0x6f34d0`) coerces a numeric string, so the index arm claims it first.
#[test]
fn a_numeric_string_takes_the_index_arm() {
    let s = seeded();
    assert_eq!(
        s.eval::<String>(r#"return (GetAddOnInfo("2"))"#).unwrap(),
        "Beta",
        "\"2\" is the second addon, not an addon named `2`"
    );
    assert!(raised(&s, r#"GetAddOnInfo("9")"#).contains("must be in the range"));
}

/// A name miss echoes the caller's string (`0x48e401`); only an index is existence-checked.
#[test]
fn an_unknown_name_echoes_itself_rather_than_a_placeholder_literal() {
    let s = seeded();
    assert_eq!(
        s.eval::<Vec<String>>(
            r#"local n,t,no,e,l,r,sec = GetAddOnInfo("Nope")
               return { n, tostring(t), tostring(no), tostring(e), tostring(l), r, sec }"#
        )
        .unwrap(),
        vec![
            "Nope".to_string(),
            "nil".into(),
            "nil".into(),
            "nil".into(),
            "nil".into(),
            "MISSING".into(),
            "INSECURE".into(),
        ]
    );
    // Each verb has its own name-miss answer.
    assert_eq!(
        s.eval::<String>(r#"return tostring(IsAddOnLoaded("Nope"))"#)
            .unwrap(),
        "nil"
    );
    assert_eq!(s.arity(r##"GetAddOnDependencies("Nope")"##).unwrap(), 0);
}
