//! A plate's alpha under a selection whose object the object manager does not hold.

use super::{plate_alpha, DIM_ALPHA};
use crate::target::Selection;

const MOB: u64 = 0x31;
const MEMBER: u64 = 0x51;

/// The alpha compares each plate's guid with the selected guid (`0x7cb9e7`-`0x7cb9f3`), whatever
/// holds its object, so a party member selected out of range dims every plate in view.
#[test]
fn a_selection_with_no_object_dims_every_plate() {
    let unheld = Selection {
        guid: Some(MEMBER),
        ..Default::default()
    };
    assert_eq!(plate_alpha(&unheld, MOB), DIM_ALPHA);
    assert_eq!(plate_alpha(&unheld, MEMBER), 255, "its own plate");
    assert_eq!(
        plate_alpha(&Selection::default(), MOB),
        255,
        "nothing selected"
    );
}
