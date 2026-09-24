//! `SetPoint` and the anchor resolve: sizes, rects, and the reference's `SetPoint` raises.

use super::common::script;
use crate::layout::Rect;

// FrameXML's anchor-to-the-screen idiom, `SetPoint("P", nil, "P", x, y)`: the explicit `nil`
// still takes its argument slot, so the offsets after it apply.
#[test]
fn setpoint_explicit_nil_relative_to_keeps_offsets() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Nil")
        f:SetPoint("TOPLEFT", nil, "TOPLEFT", 40, -40)
        f:SetWidth(300); f:SetHeight(200)
    "#,
    )
    .unwrap();
    s.resolve();
    let rect = s
        .extract()
        .iter()
        .find_map(|q| match q.target {
            crate::order::ZTarget::Frame(_) => q.rect,
            _ => None,
        })
        .expect("resolved frame rect");
    // screen [0,0,600,800], TOPLEFT+(40,-40): left 40, top 560, size 300×200.
    assert_eq!(rect, Rect::new(360.0, 40.0, 560.0, 340.0));
}

#[test]
fn setpoint_resolve_size_and_rect() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0); // screen rect [bottom 0, left 0, top 600, right 800]
    s.run(
        r#"
        local f = CreateFrame("Frame", "Sized")
        f:SetPoint("TOPLEFT", 10, -5)   -- relativeTo = screen (default), relativePoint = TOPLEFT
        f:SetWidth(200); f:SetHeight(50)
    "#,
    )
    .unwrap();
    s.resolve();

    let (w, h): (f32, f32) = s
        .eval("return Sized:GetWidth(), Sized:GetHeight()")
        .unwrap();
    assert_eq!(w, 200.0);
    assert_eq!(h, 50.0);

    // Hand-computed against the reference rect assembly (`0x767a20`): TOPLEFT anchored to screen
    // [0,0,600,800] at (10,-5), size 200×50 → Rect(bottom 545, left 10, top 595, right 210).
    let quads = s.extract();
    let frame_rect = quads
        .iter()
        .find_map(|q| match q.target {
            crate::order::ZTarget::Frame(_) => q.rect,
            _ => None,
        })
        .expect("resolved frame rect");
    assert_eq!(frame_rect, Rect::new(545.0, 10.0, 595.0, 210.0));
}

#[test]
fn getwidth_falls_back_to_explicit_size_before_resolve() {
    let s = script();
    let w: f32 = s
        .eval(r#"local f = CreateFrame("Frame"); f:SetWidth(123); return f:GetWidth()"#)
        .unwrap();
    assert_eq!(w, 123.0);
}

/// A named `relativeTo` that does not resolve raises and abandons the call, with no fallback
/// (`0x87ccd4`); the region path raises the same, both quoting the receiver's name, `<unnamed>`
/// (`0x84c7f0`) when it has none. An XML anchor's miss warns instead (`Loader::apply_anchor`,
/// `0x767800`).
#[test]
fn setpoint_unresolved_name_raises() {
    let mut s = script();
    s.run(r#"f = CreateFrame("Frame", "Orphan")"#).unwrap();

    let e = s
        .run(r#"Orphan:SetPoint("TOPLEFT", "NoSuchFrame", "TOPLEFT", 0, 0)"#)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("Orphan:SetPoint(): Couldn't find region named 'NoSuchFrame'"),
        "frame path: {e}"
    );

    let e = s
        .run(
            r#"t = Orphan:CreateTexture(nil, "ARTWORK")
               t:SetPoint("TOPRIGHT", "NoSuchRegion")"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("<unnamed>:SetPoint(): Couldn't find region named 'NoSuchRegion'"),
        "region path (anonymous receiver): {e}"
    );

    let n: i64 = s.eval("return Orphan:GetNumPoints()").unwrap();
    assert_eq!(n, 0, "the raise must leave no anchor behind");

    // `SetAllPoints` has its own message, and a number takes the name path there (`lua_isstring`)
    // where `SetPoint` reads it as an offset.
    let e = s
        .run(r#"Orphan:SetAllPoints("NoSuchFrame")"#)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("Orphan:SetAllPoints(): Couldn't find region named 'NoSuchFrame'"),
        "{e}"
    );
    let e = s.run("Orphan:SetAllPoints(42)").unwrap_err().to_string();
    assert!(
        e.contains("Orphan:SetAllPoints(): Couldn't find region named '42'"),
        "a number is a string to `lua_isstring`, so it is looked up as `_G[\"42\"]`: {e}"
    );

    s.run(r#"CreateFrame("Frame", "Target"); Orphan:SetPoint("BOTTOMLEFT", "Target", "TOPLEFT")"#)
        .unwrap();
    assert!(s.take_warnings().is_empty());
}

/// The reference's other `SetPoint` raises: Usage (`0x87cc28`) when `relativeTo` is absent, since
/// `lua_type(L,3)` is `TNONE` at the gate (`0x7a25e6-0x7a2620`); "Unknown region point"
/// (`0x87cd04`, the 9-entry scan `0x6f1840`); "trying to anchor to itself" (`0x87cca8`,
/// `0x87cd54`). The type gates run before the point scan, so an unknown point with no
/// `relativeTo` answers Usage.
#[test]
fn the_other_setpoint_raises() {
    let s = script();
    s.run(r#"f = CreateFrame("Frame", "Ladder")"#).unwrap();

    let e = s
        .run(r#"Ladder:SetPoint("CENTER")"#)
        .unwrap_err()
        .to_string();
    assert!(e.contains("Usage: Ladder:SetPoint(\"point\""), "{e}");

    let e = s
        .run(r#"Ladder:SetPoint("MIDDLE", nil, "MIDDLE", 0, 0)"#)
        .unwrap_err()
        .to_string();
    assert!(e.contains("Ladder:SetPoint(): Unknown region point"), "{e}");

    let e = s
        .run(r#"Ladder:SetPoint("MIDDLE")"#)
        .unwrap_err()
        .to_string();
    assert!(e.contains("Usage:"), "the tag gate runs first: {e}");

    let e = s
        .run(r#"Ladder:SetPoint("CENTER", Ladder, "CENTER", 0, 0)"#)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("Ladder:SetPoint(): trying to anchor to itself"),
        "{e}"
    );
}

/// `lua_type(L,3)` tells `LUA_TNIL` (0, the screen root) from `LUA_TNONE` (-1, Usage), so the
/// raise ladder must see the argument count Lua saw, not a padded or trimmed tuple.
#[test]
fn a_trailing_nil_relative_to_is_present_where_an_absent_one_is_not() {
    let s = script();
    s.run(r#"f = CreateFrame("Frame", "Trail")"#).unwrap();
    s.run(r#"Trail:SetPoint("CENTER", nil)"#).unwrap();
    assert_eq!(s.eval::<i64>("return Trail:GetNumPoints()").unwrap(), 1);
    let e = s
        .run(r#"Trail:SetPoint("CENTER")"#)
        .unwrap_err()
        .to_string();
    assert!(e.contains("Usage:"), "{e}");
}

/// An explicit `nil` `relativeTo` is the screen root, not the parent (`0x7a2710` loads it from
/// `0xcf0bd8`); the one stock site is `UIParent.lua:1549`.
#[test]
fn an_explicit_nil_relative_to_is_the_screen_not_the_parent() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        host = CreateFrame("Frame", "NilHost")
        NilHost:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 200, 100)
        NilHost:SetWidth(100) NilHost:SetHeight(100)
        kid = CreateFrame("Frame", "NilKid", NilHost)
        NilKid:SetWidth(10) NilKid:SetHeight(10)
        NilKid:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    let left: f32 = s.eval("return NilKid:GetLeft()").unwrap();
    assert_eq!(
        left, 0.0,
        "the screen's BOTTOMLEFT, not NilHost's (which is 200)"
    );
}

/// A region resolves from its own anchors and size; its owner supplies only the scale, so a region
/// anchored elsewhere resolves under an unpositioned owner. A sized owner is the control.
#[test]
fn a_region_resolves_even_when_its_owner_frame_has_no_rect() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        host = CreateFrame("Frame", "AnchorHost")
        host:SetWidth(400) host:SetHeight(200)
        host:SetPoint("BOTTOMLEFT", 100, 50)

        -- the shape that was invisible: owner with NO size and NO SetPoint
        bare = CreateFrame("Frame", "BareOwner")
        mark = bare:CreateTexture("BareMark", "ARTWORK")
        mark:SetWidth(20) mark:SetHeight(10)
        mark:SetPoint("BOTTOMLEFT", host, "BOTTOMLEFT", 5, 7)

        -- control: identical region, owner that DOES resolve
        sized = CreateFrame("Frame", "SizedOwner")
        sized:SetWidth(10) sized:SetHeight(10) sized:SetPoint("CENTER", 0, 0)
        ctl = sized:CreateTexture("SizedMark", "ARTWORK")
        ctl:SetWidth(20) ctl:SetHeight(10)
        ctl:SetPoint("BOTTOMLEFT", host, "BOTTOMLEFT", 5, 7)
        "#,
    )
    .unwrap();
    s.resolve();

    assert_eq!(
        s.eval::<Option<f32>>("return BareOwner:GetLeft()").unwrap(),
        None,
        "an unpositioned frame has no rect; the fix must not invent one for it"
    );

    // Host's BOTTOMLEFT (100,50) plus the (5,7) offset.
    assert_eq!(
        s.eval::<Option<f32>>("return BareMark:GetLeft()").unwrap(),
        Some(105.0),
        "a fully-anchored region needs nothing from its owner"
    );
    assert_eq!(
        s.eval::<Option<f32>>("return BareMark:GetBottom()")
            .unwrap(),
        Some(57.0)
    );
    assert_eq!(
        s.eval::<Option<f32>>("return SizedMark:GetLeft()").unwrap(),
        Some(105.0),
        "a sized owner's region must be unaffected by the fix"
    );
}

/// A region anchored to an unpositioned owner has no rect, nor does the chain off it; a zero rect
/// in its place would draw at the screen origin. The shape is `UIDropDownMenuTemplate`'s first two
/// textures under the anchorless `FriendsDropDown` (`FriendsFrame.xml:598`), which the reference
/// does not draw.
#[test]
fn an_unanchored_owners_region_chain_resolves_nowhere() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        -- The dropdown host: a child frame with no anchors and no size, as declared.
        bare = CreateFrame("Frame", "StrayHost")

        -- $parentLeft: anchored to the OWNER (no relativeTo = the owner frame), which has no rect.
        cap = bare:CreateTexture("StrayLeft", "ARTWORK")
        cap:SetWidth(25) cap:SetHeight(64)
        cap:SetPoint("TOPLEFT", 0, 0)

        -- $parentMiddle: the sibling chain that turned a zero rect into 115x64 of visible capsule.
        mid = bare:CreateTexture("StrayMiddle", "ARTWORK")
        mid:SetWidth(115) mid:SetHeight(64)
        mid:SetPoint("LEFT", cap, "RIGHT")
        "#,
    )
    .unwrap();
    s.resolve();

    assert_eq!(
        s.eval::<Option<f32>>("return StrayHost:GetLeft()").unwrap(),
        None,
        "the premise: an unanchored frame has no rect"
    );
    assert_eq!(
        s.eval::<Option<f32>>("return StrayLeft:GetLeft()").unwrap(),
        None,
        "a region with nothing but its unpositioned owner to derive from has no rect either — \
         standing in a zero rect here is what put the capsule at the screen origin"
    );
    assert_eq!(
        s.eval::<Option<f32>>("return StrayMiddle:GetLeft()")
            .unwrap(),
        None,
        "and the chain off it stays unresolved — this is the link that was actually visible"
    );
}

/// The geometry readers settle the layout on demand, so a frame moved inside a call stack reports
/// its new rect in that stack, again after a second move.
#[test]
fn geometry_answers_within_the_call_stack_that_moved_it() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);

    // Dewdrop-2.0's menu `Open` in one eval: no `resolve()` between the writes and the read.
    let (left, bottom): (Option<f32>, Option<f32>) = s
        .eval(
            r#"
            local f = CreateFrame("Frame", "MenuLike", UIParent)
            f:SetWidth(100) f:SetHeight(50)
            f:SetPoint("BOTTOMLEFT", UIParent, "BOTTOMLEFT", 40, 60)
            f:Show()
            return f:GetLeft(), f:GetBottom()
            "#,
        )
        .unwrap();
    assert_eq!(
        (left, bottom),
        (Some(40.0), Some(60.0)),
        "a frame must report its geometry in the stack that anchored it"
    );

    let moved: Option<f32> = s
        .eval(
            r#"
            MenuLike:ClearAllPoints()
            MenuLike:SetPoint("BOTTOMLEFT", UIParent, "BOTTOMLEFT", 200, 60)
            return MenuLike:GetLeft()
            "#,
        )
        .unwrap();
    assert_eq!(moved, Some(200.0), "the settle must re-run after each move");

    assert_eq!(
        s.eval::<(f32, f32)>("return MenuLike:GetWidth(), MenuLike:GetHeight()")
            .unwrap(),
        (100.0, 50.0)
    );
    assert_eq!(
        s.eval::<(Option<f32>, Option<f32>)>("return MenuLike:GetCenter()")
            .unwrap(),
        (Some(250.0), Some(85.0))
    );
}
