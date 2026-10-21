//! The challenge verbs' own checks, before the app's cast.

use crate::script::{DuelRequest, UiScript, UnitGuids};

const ME: u64 = 0x10;
const MEMBER: u64 = 0x51;

fn vm() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_unit_guids(&UnitGuids {
        player: ME,
        party: [MEMBER, 0, 0, 0],
        ..Default::default()
    });
    s
}

/// `StartDuelUnit` takes any token the resolver names (`0x515970` at `0x4d4caf`), ourselves
/// included, and returns only on a zero guid (`0x4d4cb8`).
#[test]
fn start_duel_unit_queues_the_guid_of_any_token() {
    let mut s = vm();
    for (token, want) in [
        ("party1", vec![DuelRequest::StartByGuid(MEMBER)]),
        ("player", vec![DuelRequest::StartByGuid(ME)]),
        ("target", vec![]),
        ("partypet1", vec![]),
    ] {
        s.run(&format!("StartDuelUnit({token:?})")).unwrap();
        assert_eq!(s.take_duel_requests(), want, "{token}");
    }
}

/// `StartDuelUnit` raises its usage for a non-string (`0x4d4cc5`) and the resolver's line for an
/// unknown token; `StartDuel` returns silently for a non-string (`0x4d4c4f`).
#[test]
fn the_challenge_verbs_check_their_argument() {
    let mut s = vm();
    let err = s.run("StartDuelUnit()").unwrap_err().to_string();
    assert!(err.contains(r#"Usage: StartDuelUnit("unit")"#), "{err}");
    let err = s.run(r#"StartDuelUnit("nobody")"#).unwrap_err().to_string();
    assert!(err.contains("Unknown unit name: nobody"), "{err}");
    s.run("StartDuel()").unwrap();
    s.run("StartDuel({})").unwrap();
    assert!(s.take_duel_requests().is_empty());
}
