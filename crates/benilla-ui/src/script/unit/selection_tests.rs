//! The selection verbs' own checks.

use crate::script::{SelectionRequest, UiScript};

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
