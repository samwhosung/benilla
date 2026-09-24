//! **The `index or "name"` prologue every in-game addon verb opens with, and its two raises**
//! (2139's "left open", closed at the bytes).
//!
//! Eight bindings — `GetAddOnInfo 0x48e390`, `GetAddOnMetadata 0x48e530`,
//! `GetAddOnDependencies 0x48e5e0`, `EnableAddOn 0x48e690`, `DisableAddOn 0x48e760`,
//! `IsAddOnLoadOnDemand 0x48e840`, `IsAddOnLoaded 0x48e8e0`, `LoadAddOn 0x48e980` — share one
//! eleven-instruction prologue, and `luaL_error 0x6f4940` does not return from either of its
//! failure arms. We answered a placeholder, a `nil` or a silent no-op for all of them.

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
    // **The index space exists only once the server has answered**. These tests
    // are about the prologue's bounds and raises, and a bound of zero would make every one of
    // them pass vacuously — so the fixture seats the reply an in-world VM has always had, hiding
    // nothing.
    s.note_addon_info_reply(&[]);
    s
}

/// The message of a raise, or `"<no raise>"` if the call completed.
fn raised(s: &UiScript, call: &str) -> String {
    s.eval::<String>(&format!(
        "local ok, e = pcall(function() {call} end) \
         if ok then return \"<no raise>\" end return tostring(e)"
    ))
    .unwrap()
}

/// **Every verb raises its own `Usage:` literal on a non-number, non-string argument.** Read out
/// of `.data` (`0x842d68` … `0x842e98`) rather than reconstructed: the message reaches an addon's
/// error handler, so the spelling is the contract.
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
    // `nil` and a missing argument fail the same two tests a table does — `lua_isnumber` reports
    // NULL past `L->top` as "not a number" — so they take the same raise.
    for call in ["LoadAddOn(nil)", "LoadAddOn()", "IsAddOnLoaded(true)"] {
        assert!(
            raised(&s, call).contains("index or \"name\""),
            "{call}: {}",
            raised(&s, call)
        );
    }
    // The second argument has its own `lua_isstring` (`0x48e59c`) onto the SAME raise.
    assert!(raised(&s, "GetAddOnMetadata(1)").contains("Usage: GetAddOnMetadata"));
}

/// **An out-of-range numeric index raises, on every verb, with the count in the message** —
/// `"AddOn index must be in the range of 1 to %d"` (`0x837d70`), `%d` from `0x51def0()`.
///
/// The bound is **unsigned** (`0x51df00 cmp ecx,[0xbe1b90]; jb`), so `f(0)` decrements to
/// `0xFFFFFFFF` and raises by the same route as `f(count+1)`. Only three of the eight raised at
/// all before this, and those three said something the image does not.
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
            // 2.9 truncates toward zero (`_ftol 0x40a2b0` chops) to 2, which is IN range — the
            // one of the four that must NOT raise.
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

/// **A numeric STRING is an index, not a name** — `lua_isnumber 0x6f34d0` coerces one, so the
/// number arm claims it before `lua_isstring` is ever reached.
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

/// **`GetAddOnInfo`'s string miss echoes the caller's own name back** (`0x48e401`, non-NULL by
/// `lua_isstring`), then five metadata misses, `"MISSING"` and `"INSECURE"`. We answered the
/// literal `"NoSuchAddon"` — an arbitrary choice, not a constant in the image.
///
/// The name form is never existence-checked by the prologue; only the numeric one is.
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
    // The other name-miss answers are each verb's own, and they do not agree with GetAddOnInfo's.
    assert_eq!(
        s.eval::<String>(r#"return tostring(IsAddOnLoaded("Nope"))"#)
            .unwrap(),
        "nil"
    );
    assert_eq!(s.arity(r##"GetAddOnDependencies("Nope")"##).unwrap(), 0);
}
