//! The selection verbs' own checks, and `"target"` for a selection whose object the object
//! manager does not hold.

use crate::script::{SelectionRequest, UiScript, UnitGuids, UnitState};

const ME: u64 = 0x10;
/// party1, out of range.
const FAR: u64 = 0x22;

/// A target with no object, a party member out of range, reads as its guid does: the getters
/// resolve `"target"` to the guid (`0x515970`) and, with no object (`0x468460`), read the party
/// row's record (`0x496400`) and the roster's `UnitExists` (`0x491900`), so the target frame
/// shows the member.
#[test]
fn a_target_with_no_object_reads_its_party_row() {
    let mut s = UiScript::new().unwrap();
    s.set_unit_guids(&UnitGuids {
        player: ME,
        target: FAR,
        party: [FAR, 0, 0, 0],
        ..Default::default()
    });
    s.set_unit(
        "party1",
        Some(UnitState {
            exists: true,
            guid: FAR,
            name: Some("Brisca".into()),
            health: 300,
            max_health: 1000,
            level: 41,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<Option<String>>(r#"return UnitName("target")"#)
            .unwrap()
            .as_deref(),
        Some("Brisca")
    );
    assert!(s
        .eval::<bool>(r#"return UnitExists("target") == 1"#)
        .unwrap());
    assert_eq!(
        s.eval::<i64>(r#"return UnitHealth("target")"#).unwrap(),
        300
    );
    assert!(s
        .eval::<bool>(r#"return UnitIsUnit("target", "party1") == 1"#)
        .unwrap());
}

/// `AssistUnit` raises its usage for a non-string (`0x489c22`) and the resolver's line for an
/// unknown token (`0x515940` → `0x515970`), and queues nothing; `AssistByName` hands the host any
/// string, the empty one included, whose assist is the selection's (`0x489c89`).
#[test]
fn the_assist_verbs_check_their_argument() {
    let mut s = UiScript::new().unwrap();
    s.keep_calls();
    let err = s.run("AssistUnit()").unwrap_err().to_string();
    assert!(err.contains(r#"Usage: AssistUnit("unit")"#), "{err}");
    let err = s.run(r#"AssistUnit("nobody")"#).unwrap_err().to_string();
    assert!(err.contains("Unknown unit name: nobody"), "{err}");
    let err = s.run("AssistByName()").unwrap_err().to_string();
    assert!(err.contains(r#"Usage: AssistByName("name")"#), "{err}");
    s.run(r#"AssistByName("")"#).unwrap();
    assert_eq!(
        s.take_selection_requests(),
        vec![SelectionRequest::AssistByName(String::new())]
    );
}
