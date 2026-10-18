//! The script lengths besides the frame size, edges and points: each setter stores `S(v)` and its
//! getter answers `G` of the stored value (benilla-re `systems/ui/methods.md`, "Other methods that
//! convert"). At 4:3, `G(S(100))` is 99.99999850988391 and `G(S(50))` 49.999999254941955.

use super::common::script;
use crate::script::UiScript;

/// `G(S(100))` and friends at a 1024×768 window, from the law's own steps.
const G_100: f64 = 99.99999850988391;
const G_50: f64 = 49.999999254941955;
const G_11: f64 = 11.000000074505804;
const G_12: f64 = 12.00000029802322;
const G_3: f64 = 3.000000074505805;
const G_5: f64 = 4.999999925494195;

fn vm() -> UiScript {
    let mut s = script();
    s.set_window_size(1024.0, 768.0);
    s
}

/// `SetMinResize`/`SetMaxResize` store `S` of both (`0x77610f`, `0x776138`, `0x77638f`,
/// `0x7763b8`) and the getters answer `G` (`0x775fda`, `0x775ffe`, `0x77625a`, `0x77627e`);
/// `SetHitRectInsets` and `GetHitRectInsets` the same on four (`0x775e4d`, `0x775cbc`).
#[test]
fn the_resize_bounds_and_hit_insets_answer_through_the_getter_steps() {
    let s = vm();
    s.run(
        r#"F = CreateFrame("Frame", "Bounds")
           F:SetMinResize(100, 50) F:SetMaxResize(11, 12)
           F:SetHitRectInsets(3, 5, 100, 0)"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return F:GetMinResize()").unwrap(),
        (G_100, G_50)
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return F:GetMaxResize()").unwrap(),
        (G_11, G_12)
    );
    assert_eq!(
        s.eval::<(f64, f64, f64, f64)>("return F:GetHitRectInsets()")
            .unwrap(),
        (G_3, G_5, G_100, 0.0)
    );
}
