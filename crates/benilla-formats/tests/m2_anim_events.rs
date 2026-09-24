//! M2 events: the table at MD20 `0x114`, 44 bytes a record, its keys timed on the global sequence
//! timeline and rebased into each sequence; tags are stored forward, `$xxx`.

use benilla_formats::{open_chain, parse_m2_animations};

#[test]
fn creature_anim_events_parse_within_sequences() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");

    for model in [
        "Creature\\DireWolf\\DireWolf.m2",
        "Creature\\Kobold\\Kobold.m2",
        "Creature\\Murloc\\Murloc.m2",
    ] {
        let bytes = chain.read(model).expect("model bytes");
        let anims = parse_m2_animations(&bytes);
        assert!(!anims.is_empty(), "{model}: sequences parse");

        let mut total_events = 0usize;
        let mut footstep_tags = 0usize;
        for a in &anims {
            for e in &a.events {
                total_events += 1;
                assert!(
                    e.time >= 0.0 && e.time <= a.duration + 1e-3,
                    "{model}: event {:?} at {}s escapes its {}s sequence (anim {})",
                    std::str::from_utf8(&e.ident),
                    e.time,
                    a.duration,
                    a.anim_id
                );
                assert_eq!(e.ident[0], b'$', "{model}: forward-stored $ tag");
                if matches!(&e.ident[..3], b"$FL" | b"$FR" | b"$RL" | b"$RR") {
                    footstep_tags += 1;
                }
            }
        }
        assert!(total_events > 0, "{model}: has event keys");
        assert!(
            footstep_tags > 0,
            "{model}: walking creature has footstep tags"
        );
    }
}

/// A fired key carries its own record's bone and position, not the tag's first match: the event
/// kernel `0x719370` snapshots `placementMatrix · (boneMatrix[event.bone] · event.position)` into
/// the callback record. Every player model authors six `$CSD` records, one per emote clip.
#[test]
fn a_fired_key_carries_its_own_records_bone_and_point() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");
    let model = "Character\\Human\\Male\\HumanMale.m2";
    let bytes = chain.read(model).expect("model bytes");

    let markers = benilla_formats::parse_m2_event_markers(&bytes).expect("event markers");
    let csd_records: Vec<_> = markers.iter().filter(|m| &m.ident == b"$CSD").collect();
    assert_eq!(
        csd_records.len(),
        6,
        "{model}: the six emote-voice records are what makes this test worth having"
    );
    let distinct: std::collections::BTreeSet<_> =
        csd_records.iter().map(|m| (m.bone, m.data_key())).collect();
    assert_eq!(
        distinct.len(),
        6,
        "{model}: the six records are distinct — a first-match lookup cannot stand in for them"
    );

    let anims = parse_m2_animations(&bytes);
    let mut fired: std::collections::BTreeSet<(u16, [u32; 3])> = Default::default();
    for a in &anims {
        for e in a.events.iter().filter(|e| &e.ident == b"$CSD") {
            let key = (e.bone, bits(e.position));
            assert!(
                distinct.contains(&key),
                "{model}: fired $CSD at bone {} is not one of the authored records",
                e.bone
            );
            fired.insert(key);
        }
    }
    assert!(
        fired.len() > 1,
        "{model}: only {} distinct $CSD point(s) ever fire — the record identity is being lost \
         somewhere between the table and the key",
        fired.len()
    );
}

/// Exact bit patterns, so a float compare never decides record identity.
fn bits(p: [f32; 3]) -> [u32; 3] {
    [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]
}

trait MarkerKey {
    fn data_key(&self) -> [u32; 3];
}
impl MarkerKey for benilla_formats::EventMarker {
    fn data_key(&self) -> [u32; 3] {
        bits(self.position)
    }
}
