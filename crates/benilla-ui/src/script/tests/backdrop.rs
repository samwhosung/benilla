//! Backdrop: Lua verbs + extract emission (the reference's Backdrop object, `0x77e5f0`).

use super::common::script;
use crate::script::*;

// SetBackdrop installs the plate; SetBackdropColor tints the bg only, SetBackdropBorderColor all 8
// border pieces; extract emits bg-then-border at the frame's own slot with those colors.
#[test]
fn backdrop_installs_and_extracts_pieces_with_colors() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Plate")
        f:SetPoint("TOPLEFT", nil, "TOPLEFT", 100, -100)
        f:SetWidth(200); f:SetHeight(100)
        f:SetBackdrop({
            bgFile = "bg", edgeFile = "edge", tile = true,
            tileSize = 16, edgeSize = 16,
            insets = { left = 5, right = 5, top = 5, bottom = 5 },
        })
        f:SetBackdropColor(0.09, 0.09, 0.19)
        f:SetBackdropBorderColor(1, 1, 1)
    "#,
    )
    .unwrap();
    s.resolve();
    let pieces: Vec<(String, [f32; 4])> = s
        .extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Backdrop { path, color, .. } => Some((path, color)),
            _ => None,
        })
        .collect();
    // bg (1) + 8 border pieces.
    assert_eq!(pieces.len(), 9);
    // First is the bg, tinted the tooltip background color — QUANTIZED. The reference's colour
    // field is a packed `0xAARRGGBB` byte quad and the setter converts `×255 + 0.5` through
    // `__ftol` (`SetBackdropColor 0x777d30`), so `0.09` stores as 23 and reads back
    // as `23/255`. This assertion used to hold `0.09` exactly, which was our lossless `[f32; 4]`
    // showing through a store the client cannot make.
    assert_eq!(pieces[0].0, "bg");
    let q = |x: f32| f32::from((x * 255.0 + 0.5) as u8) / 255.0;
    assert_eq!(pieces[0].1, [q(0.09), q(0.09), q(0.19), 1.0]);
    // The remaining 8 are the border, white, from the edge file.
    assert!(pieces[1..]
        .iter()
        .all(|(p, c)| p == "edge" && *c == [1.0, 1.0, 1.0, 1.0]));
}

// `GetBackdrop()` — the four traps of `0x777370`, pinned
// where an addon can observe them. The corpus caller is `BuffCheck2.lua:448`, which reads the table
// back, edits `insets`, and feeds it straight to `SetBackdrop` — so a wrong `tile` type or a missing
// ctor default does not just read wrong, it round-trips wrong.
#[test]
fn get_backdrop_reconstructs_from_the_struct() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "GB")

        -- How many values did that return? 5.0 answers with the implicit vararg table's `n`;
        -- `select` is 5.1's base library and is not a 1.12 global (`lua50::install`).
        local function count(...) return arg.n end

        -- TRAP 2: no backdrop => ZERO values, not nil. The count is the only way to see it.
        assert(count(f:GetBackdrop()) == 0, "unset backdrop must return no values at all")

        -- TRAP 1: the result is rebuilt from the struct, not the caller's table. The alien key is
        -- the proof: SetBackdrop never accepted it, so it cannot come back out.
        local passed = { bgFile = "bg", edgeFile = "edge", tile = true,
                         tileSize = 16, edgeSize = 24,
                         insets = { left = 1, right = 2, top = 3, bottom = 4 },
                         alien = "must not survive" }
        f:SetBackdrop(passed)
        local b = f:GetBackdrop()
        assert(count(f:GetBackdrop()) == 1, "a set backdrop is exactly one value")
        assert(b ~= passed, "must not hand back the caller's own table")
        assert(b.alien == nil, "keys SetBackdrop never read cannot reappear")
        assert(b.insets ~= passed.insets, "the insets subtable is rebuilt too")
        assert(b.bgFile == "bg" and b.edgeFile == "edge", "files round-trip")
        assert(b.tileSize == 16 and b.edgeSize == 24, "sizes round-trip")
        assert(b.insets.left == 1 and b.insets.right == 2
               and b.insets.top == 3 and b.insets.bottom == 4, "insets round-trip")

        -- TRAP 4: `tile` is the NUMBER 1, never the boolean the caller passed.
        assert(type(b.tile) == "number", "tile must be a number, got " .. type(b.tile))
        assert(b.tile == 1, "tile must be 1")

        -- Mutating the caller's table afterwards cannot reach the frame (no reference is kept).
        passed.bgFile = "clobbered"
        assert(f:GetBackdrop().bgFile == "bg", "no live reference to the caller's table")

        -- TRAP 3: a partial SetBackdrop omits NOTHING — a fresh struct is allocated every call, so
        -- the keys left out come back as CTOR DEFAULTS, and none of the old backdrop survives.
        f:SetBackdrop({ edgeFile = "onlyedge" })
        local p = f:GetBackdrop()
        assert(p.bgFile == "", "an omitted bgFile is the empty string, not nil")
        assert(p.edgeSize == 32, "an omitted edgeSize is the ctor's 32, not the previous 24")
        assert(p.tileSize == 0, "an omitted tileSize is 0, not the previous 16")
        assert(p.insets.left == 0 and p.insets.right == 0
               and p.insets.top == 0 and p.insets.bottom == 0, "omitted insets are 0, not 1/2/3/4")
        -- TRAP 4, the other half: tile false pushes nil, so the key is ABSENT — never `false`.
        assert(p.tile == nil, "tile false means the key is absent, not `false`")

        -- The undocumented in-place form: fills and returns YOUR table, recycling `insets`, and
        -- erases a stale `tile` on the way (the nil push is a real `lua_settable`).
        local ins = {}
        local mine = { insets = ins, tile = true, stale = "kept" }
        local got = f:GetBackdrop(mine)
        assert(got == mine, "the in-place form returns the table it was given")
        assert(mine.insets == ins, "an existing insets subtable is reused, not replaced")
        assert(mine.tile == nil, "a stale tile key is erased from a recycled table")
        assert(mine.stale == "kept", "keys it does not write are left alone")
        assert(mine.edgeSize == 32 and ins.bottom == 0, "the recycled table is filled")

        -- SetBackdrop(nil) is indistinguishable from never having set one: back to zero values.
        f:SetBackdrop(nil)
        assert(count(f:GetBackdrop()) == 0, "SetBackdrop(nil) returns to the zero-value shape")
    "#,
    )
    .unwrap();
}

// BuffCheck2.lua:448's exact shape: read the plate back, edit `insets`, push it straight back in.
// It is the whole reason this method exists in our client, so it is the test that must not rot.
#[test]
fn get_backdrop_round_trips_through_set_backdrop() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "BC2Plate")
        f:SetPoint("CENTER", 0, 0)
        f:SetWidth(54); f:SetHeight(54)
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge", tile = true,
                        tileSize = 32, edgeSize = 32,
                        insets = { left = 11, right = 12, top = 12, bottom = 11 } })
        local backdrop = f:GetBackdrop()
        backdrop["insets"] = { top = 12, bottom = 11, right = 12, left = 11 }
        backdrop["tile"] = true
        backdrop["tileSize"] = 32
        backdrop["edgeSize"] = 32
        f:SetBackdrop(backdrop)
        local b = f:GetBackdrop()
        assert(b.bgFile == "bg" and b.edgeFile == "edge", "files survive the round trip")
        assert(b.tile == 1 and b.tileSize == 32 and b.edgeSize == 32, "tiling survives")
        assert(b.insets.left == 11 and b.insets.bottom == 11, "insets survive")
    "#,
    )
    .unwrap();
    s.resolve();
    // The plate still draws: bg + 8 border pieces, unchanged by the round trip.
    assert_eq!(
        s.extract()
            .iter()
            .filter(|q| matches!(q.content, QuadContent::Backdrop { .. }))
            .count(),
        9
    );
}

// SetBackdrop(nil) tears the plate down (no pieces after).
#[test]
fn set_backdrop_nil_tears_down() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Plate2")
        f:SetPoint("CENTER", 0, 0)
        f:SetWidth(100); f:SetHeight(100)
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        f:SetBackdrop(nil)
    "#,
    )
    .unwrap();
    s.resolve();
    assert!(s
        .extract()
        .iter()
        .all(|q| !matches!(q.content, QuadContent::Backdrop { .. })));
}

/// **The colour setters' argument gating is asymmetric, and getting it backwards is worse than
/// the bug it replaces** (`0x777d30` / `0x7780d0`).
///
/// r/g/b go through a bare `lua_tonumber` — a `nil` channel is `0.0` and the call COMPLETES.
/// benilla typed them `f32` and raised, which killed ShaguTweaks at `helpers.lua:248`, where
/// `color.r` comes off a table that does not always have one.
///
/// Alpha is different: `lua_isnumber`-gated at `0x778227` with `1.0f` staged at `0x778220`, so a
/// missing **or nil** alpha is OPAQUE. The tempting blanket fix — "treat every nil as 0.0, like
/// its neighbours" — would have turned every one of those borders transparent, which reads as a
/// rendering fault rather than an API bug and is the reason this test names both halves.
#[test]
fn backdrop_colors_coerce_rgb_but_default_alpha_opaque() {
    let s = script();
    s.run(
        r#"
        f = CreateFrame("Frame", "Plate2")
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge", edgeSize = 16 })
    "#,
    )
    .unwrap();

    // A nil channel is 0.0 and the call completes — no raise.
    s.run("f:SetBackdropBorderColor(nil, 1, 1, 1)").unwrap();
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return f:GetBackdropBorderColor()")
            .unwrap(),
        (0.0, 1.0, 1.0, 1.0)
    );

    // A missing alpha is 1.0, and so is an explicit nil one — NOT 0.0.
    s.run("f:SetBackdropColor(0.2, 0.4, 0.6)").unwrap();
    let (_, _, _, a) = s
        .eval::<(f32, f32, f32, f32)>("return f:GetBackdropColor()")
        .unwrap();
    assert_eq!(a, 1.0, "a missing alpha is opaque");
    s.run("f:SetBackdropColor(0.2, 0.4, 0.6, nil)").unwrap();
    let (_, _, _, a) = s
        .eval::<(f32, f32, f32, f32)>("return f:GetBackdropColor()")
        .unwrap();
    assert_eq!(
        a, 1.0,
        "an explicit nil alpha is opaque too — the isnumber gate"
    );

    // Out of range clamps; a table coerces to 0 like any non-number.
    s.run("f:SetBackdropColor(5, -1, {}, 0.5)").unwrap();
    let (r, g, b, a) = s
        .eval::<(f32, f32, f32, f32)>("return f:GetBackdropColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 0.0, 0.0));
    // Quantized `×255 + 0.5` through `__ftol`: 0.5 -> 128/255, not 0.5 exactly. The field is a
    // packed 0xAARRGGBB byte quad and nothing finer survives the store.
    assert_eq!(a, 128.0 / 255.0);
}
