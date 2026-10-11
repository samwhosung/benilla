//! The backdrop Lua verbs, its nine piece regions and their draw (the reference's Backdrop object,
//! `0x77e5f0`, and its pieces, `0x77e8d0`).

use super::common::script;
use crate::script::*;

// SetBackdropColor tints only the bg, SetBackdropBorderColor the 8 border pieces.
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
            QuadContent::Texture {
                path: Some(path),
                color: Some(color),
                ..
            } => Some((path, color)),
            _ => None,
        })
        .collect();
    // bg (1) + 8 border pieces.
    assert_eq!(pieces.len(), 9);
    // The colour is a packed `0xAARRGGBB` byte quad, stored as `×255 + 0.5` through `__ftol`
    // (`SetBackdropColor 0x777d30`), so 0.09 reads back as 23/255.
    assert_eq!(pieces[0].0, "bg");
    let q = |x: f32| f32::from((x * 255.0 + 0.5) as u8) / 255.0;
    assert_eq!(pieces[0].1, [q(0.09), q(0.09), q(0.19), 1.0]);
    assert!(pieces[1..]
        .iter()
        .all(|(p, c)| p == "edge" && *c == [1.0, 1.0, 1.0, 1.0]));
}

// The four traps of `GetBackdrop()` (`0x777370`), as an addon observes them.
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

// The shape of `BuffCheck2.lua:448`: read the plate back, edit `insets`, set it again.
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
    // bg (1) + 8 border pieces.
    assert_eq!(
        s.extract()
            .iter()
            .filter(|q| matches!(q.content, QuadContent::Texture { .. }))
            .count(),
        9
    );
}

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
        .all(|q| !matches!(q.content, QuadContent::Texture { .. })));
}

/// The colour setters (`0x777d30`, `0x7780d0`) read r/g/b with a bare `lua_tonumber` (nil is 0.0)
/// and gate alpha on `lua_isnumber` (`0x778227`) over a staged 1.0 (`0x778220`).
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

    s.run("f:SetBackdropBorderColor(nil, 1, 1, 1)").unwrap();
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return f:GetBackdropBorderColor()")
            .unwrap(),
        (0.0, 1.0, 1.0, 1.0)
    );

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
    // Stored as a byte: 0.5 reads back as 128/255.
    assert_eq!(a, 128.0 / 255.0);
}

/// The pieces are textures anchored and sized in the frame's own units (`0x77e8d0`), so the layout
/// scales them by its effective scale like any region: a half-scale frame's border is half as
/// thick and its background inset half as deep.
#[test]
fn a_scaled_frames_backdrop_scales_with_it() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Half")
        f:SetScale(0.5)
        f:SetPoint("TOPLEFT", nil, "TOPLEFT", 100, -100)
        f:SetWidth(200); f:SetHeight(100)
        f:SetBackdrop({
            bgFile = "bg", edgeFile = "edge", tile = false, edgeSize = 16,
            insets = { left = 4, right = 4, top = 4, bottom = 4 },
        })
    "#,
    )
    .unwrap();
    s.resolve();
    let rects: Vec<crate::layout::Rect> = s
        .extract()
        .into_iter()
        .filter(|q| matches!(q.content, QuadContent::Texture { .. }))
        .map(|q| q.rect.expect("piece rect"))
        .collect();
    assert_eq!(rects.len(), 9);
    // The frame on screen: left 50, top 550, 100 × 50.
    let bg = rects[0];
    assert_eq!(
        (bg.left, bg.right, bg.bottom, bg.top),
        (52.0, 148.0, 502.0, 548.0),
        "the insets are 4 units of a half-scale frame"
    );
    // Every border piece is 16 units, 8 on screen, thick.
    for r in &rects[1..] {
        assert_eq!(
            r.width().min(r.height()),
            8.0,
            "a border piece {r:?} is edgeSize units of a half-scale frame"
        );
    }
}

// ── The pieces are regions (`0x77e8d0`) ──────────────────────────────────────────────────────

/// Installing a backdrop builds nine Textures through the texture constructor and links each into
/// the frame's region list (`0x76fc40` → `0x77fd10` → `0x76a750`): the background on BACKGROUND,
/// then LEFT, RIGHT, TOP, BOTTOM, TOPLEFT, TOPRIGHT, BOTTOMLEFT, BOTTOMRIGHT on BORDER, unnamed,
/// shown, parented to the frame.
#[test]
fn a_backdrop_is_nine_texture_regions_in_the_reference_order() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "NinePlate")
        f:SetBackdrop({ bgFile = "Interface\\bg", edgeFile = "Interface\\edge" })
        assert(f:GetNumRegions() == 9, "nine regions, got " .. f:GetNumRegions())
        local r = { f:GetRegions() }
        assert(table.getn(r) == 9, "GetRegions returns the nine")
        for i = 1, 9 do
            local p = r[i]
            assert(p:GetObjectType() == "Texture", "piece " .. i .. " is a Texture")
            assert(p:GetName() == nil, "piece " .. i .. " is unnamed")
            assert(p:GetParent() == f, "piece " .. i .. " belongs to the frame")
            assert(p:IsShown(), "piece " .. i .. " is shown")
        end
        assert(r[1]:GetTexture() == "Interface\\bg", "the first is the background")
        assert(r[1]:GetDrawLayer() == "BACKGROUND", "the background is on BACKGROUND")
        for i = 2, 9 do
            assert(r[i]:GetTexture() == "Interface\\edge", "piece " .. i .. " is an edge")
            assert(r[i]:GetDrawLayer() == "BORDER", "piece " .. i .. " is on BORDER")
        end
    "#,
    )
    .unwrap();
}

/// Each piece is gated on its file (`0x77e91f` for the background, the `+0x3c` mask for the edges).
#[test]
fn a_background_alone_is_one_region_and_a_border_alone_eight() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame")
        f:SetBackdrop({ bgFile = "bg" })
        assert(f:GetNumRegions() == 1, "bg only: " .. f:GetNumRegions())
        f:SetBackdrop({ edgeFile = "edge" })
        assert(f:GetNumRegions() == 8, "border only: " .. f:GetNumRegions())
        f:SetBackdrop({ bgFile = "", edgeFile = "" })
        assert(f:GetNumRegions() == 0, "empty files: " .. f:GetNumRegions())
    "#,
    )
    .unwrap();
}

/// The region list is in link order (`0x76a750` appends), so a texture created after the backdrop
/// is the tenth region; one created before stays first.
#[test]
fn a_region_created_after_the_backdrop_comes_tenth() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame")
        local before = f:CreateTexture(nil, "ARTWORK")
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        local after = f:CreateTexture(nil, "ARTWORK")
        assert(f:GetNumRegions() == 11, "11 regions, got " .. f:GetNumRegions())
        local r = { f:GetRegions() }
        assert(r[1] == before, "the earlier texture stays first")
        assert(r[2]:GetTexture() == "bg", "then the background")
        assert(r[11] == after, "the later texture comes after the nine pieces")
    "#,
    )
    .unwrap();
}

/// `SetBackdrop(nil)` releases all nine (`0x76a5d0` → `0x77f4d0`), and a second `SetBackdrop`
/// releases the old nine before building new ones, which link at the end of the list.
#[test]
fn set_backdrop_nil_leaves_no_pieces_and_a_second_one_relinks_at_the_end() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame")
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        f:SetBackdrop(nil)
        assert(f:GetNumRegions() == 0, "nil leaves none, got " .. f:GetNumRegions())

        f:SetBackdrop({ bgFile = "old", edgeFile = "edge" })
        local mine = f:CreateTexture(nil, "ARTWORK")
        f:SetBackdrop({ bgFile = "new", edgeFile = "edge" })
        assert(f:GetNumRegions() == 10, "nine pieces and one texture, got " .. f:GetNumRegions())
        local r = { f:GetRegions() }
        assert(r[1] == mine, "the texture made between the two is now first")
        assert(r[2]:GetTexture() == "new", "the new background follows it")
    "#,
    )
    .unwrap();
}

/// The colours are each piece's vertex colour (`0x77f750`): `SetBackdropColor` writes the
/// background only (`0x77f410`), `SetBackdropBorderColor` the eight edges only (`0x77f440`).
#[test]
fn the_border_colour_tints_the_eight_edges_and_never_the_background() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame")
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        f:SetBackdropBorderColor(1, 0, 0, 0.6)
        local r = { f:GetRegions() }
        local function is(p, cr, cg, cb, ca, what)
            local pr, pg, pb, pa = p:GetVertexColor()
            assert(math.abs(pr - cr) < 0.01 and math.abs(pg - cg) < 0.01
                   and math.abs(pb - cb) < 0.01 and math.abs(pa - ca) < 0.01,
                   what .. ": " .. pr .. "," .. pg .. "," .. pb .. "," .. pa)
        end
        is(r[1], 1, 1, 1, 1, "the background keeps white")
        for i = 2, 9 do is(r[i], 1, 0, 0, 0.6, "edge " .. i) end
        f:SetBackdropColor(0, 0, 1)
        is(r[1], 0, 0, 1, 1, "the background takes the backdrop colour")
        for i = 2, 9 do is(r[i], 1, 0, 0, 0.6, "edge " .. i .. " after SetBackdropColor") end
    "#,
    )
    .unwrap();
}

/// A piece is an ordinary texture: an addon may hide it or swap its art, and that is what draws.
#[test]
fn an_addon_can_hide_or_retexture_a_piece() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame")
        f:SetPoint("CENTER", 0, 0)
        f:SetWidth(100); f:SetHeight(100)
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        local r = { f:GetRegions() }
        r[1]:SetTexture("mine")
        r[2]:Hide()
    "#,
    )
    .unwrap();
    s.resolve();
    let paths: Vec<String> = s
        .extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(
        paths.len(),
        8,
        "the hidden LEFT edge draws nothing: {paths:?}"
    );
    assert_eq!(paths[0], "mine", "the background draws its new art");
}

/// The edges tile along their run, `dim / edgeSize - 2` (`0x77f0c0`), and a resize re-runs the
/// texcoords (`0x76b580`), which `GetTexCoord` reads back in `SetTexCoord`'s UL, LL, UR, LR order.
#[test]
fn a_resize_re_runs_the_edge_texcoords() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame")
        f:SetPoint("CENTER", 0, 0)
        f:SetWidth(200); f:SetHeight(100)
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge", edgeSize = 16 })
    "#,
    )
    .unwrap();
    s.resolve();
    let left = "local r = { f:GetRegions() } return r[2]:GetTexCoord()";
    let uv = s
        .eval::<(f32, f32, f32, f32, f32, f32, f32, f32)>(left)
        .unwrap();
    // 100 / 16 - 2 = 4.25 periods down the LEFT edge, slice 0.
    assert_eq!(uv, (0.0, 0.0, 0.0, 4.25, 0.125, 0.0, 0.125, 4.25));
    s.run("f:SetHeight(64)").unwrap();
    s.resolve();
    let uv = s
        .eval::<(f32, f32, f32, f32, f32, f32, f32, f32)>(left)
        .unwrap();
    assert_eq!(uv, (0.0, 0.0, 0.0, 2.0, 0.125, 0.0, 0.125, 2.0));
}

/// `<Backdrop>` is one more child of the frame's LoadXML loop (`0x769b1c`), handled in document
/// order with `<Layers>` (`0x769c38`, `0x769cdb`): a backdrop before the layers links its pieces
/// first, one after them links them after.
#[test]
fn xml_backdrop_and_layers_link_in_document_order() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
             <Frame name="BackdropFirst">
               <Backdrop bgFile="bg" edgeFile="edge"/>
               <Layers><Layer level="ARTWORK"><Texture name="$parentArt"/></Layer></Layers>
             </Frame>
             <Frame name="LayersFirst">
               <Layers><Layer level="ARTWORK"><Texture name="$parentArt"/></Layer></Layers>
               <Backdrop bgFile="bg" edgeFile="edge"/>
             </Frame>
           </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    s.run(
        r#"
        local a = { BackdropFirst:GetRegions() }
        assert(table.getn(a) == 10, "ten regions")
        assert(a[1]:GetTexture() == "bg" and a[10] == BackdropFirstArt, "pieces, then the art")
        local b = { LayersFirst:GetRegions() }
        assert(b[1] == LayersFirstArt and b[2]:GetTexture() == "bg", "the art, then the pieces")
    "#,
    )
    .unwrap();
}

/// The regression check on the draw: each piece's quad (rect, texcoords, colour and order) is the
/// one the backdrop geometry (`pieces`, `0x77e8d0` and `0x77f0c0`) gives for the frame, on the
/// stock plates' shapes, at scale 1 and below, after a resize too.
#[test]
fn the_piece_quads_are_the_backdrop_geometry() {
    use crate::layout::Rect;
    // The tooltip, the dialog box (`UI-DialogBox-Border`, asymmetric 11/12 insets), an untiled
    // stretch, and a top inset over the bottom one.
    let shapes = [
        "{ bgFile = 'bg', edgeFile = 'edge', tile = true, tileSize = 16, edgeSize = 16, \
           insets = { left = 5, right = 5, top = 5, bottom = 5 } }",
        "{ bgFile = 'bg', edgeFile = 'edge', tile = true, tileSize = 32, edgeSize = 32, \
           insets = { left = 11, right = 12, top = 12, bottom = 11 } }",
        "{ bgFile = 'bg', edgeFile = 'edge', tile = false, edgeSize = 16, \
           insets = { left = 4, right = 4, top = 4, bottom = 4 } }",
    ];
    for (i, shape) in shapes.iter().enumerate() {
        for scale in [1.0f32, 0.75] {
            let mut s = script();
            s.set_screen_size(1024.0, 768.0);
            s.run(&format!(
                r#"
                f = CreateFrame("Frame")
                f:SetScale({scale})
                f:SetPoint("TOPLEFT", nil, "TOPLEFT", 100, -100)
                f:SetWidth(301); f:SetHeight(157)
                f:SetAlpha(0.5)
                f:SetBackdrop({shape})
                f:SetBackdropColor(0.1, 0.2, 0.3, 0.9)
                f:SetBackdropBorderColor(0.4, 0.5, 0.6)
            "#
            ))
            .unwrap();
            for (w, h) in [(301.0f32, 157.0f32), (90.0, 40.0)] {
                s.run(&format!("f:SetWidth({w}); f:SetHeight({h})"))
                    .unwrap();
                s.resolve();
                // The one frame with a backdrop.
                let (fr, eff) = {
                    let model = s.model_ref();
                    let fh = *model.backdrops.keys().next().unwrap();
                    let fr = model.resolved[&fh];
                    (fr, crate::script::object::eff_scale(&model, fh))
                };
                let quads: Vec<_> = s
                    .extract()
                    .into_iter()
                    .filter_map(|q| match q.content {
                        QuadContent::Texture {
                            path: Some(path),
                            color,
                            tex_coords,
                            ..
                        } => Some((path, q.rect.unwrap(), color, tex_coords, q.alpha)),
                        _ => None,
                    })
                    .collect();
                let spec = {
                    let mut b: Backdrop = s
                        .eval::<mlua::Table>("return f:GetBackdrop()")
                        .map(|t| Backdrop {
                            bg_file: t.get("bgFile").ok(),
                            edge_file: t.get("edgeFile").ok(),
                            tile: t.get::<Option<f32>>("tile").ok().flatten().is_some(),
                            tile_size: t.get("tileSize").unwrap(),
                            edge_size: t.get("edgeSize").unwrap(),
                            insets: {
                                let ins: mlua::Table = t.get("insets").unwrap();
                                Insets {
                                    left: ins.get("left").unwrap(),
                                    right: ins.get("right").unwrap(),
                                    top: ins.get("top").unwrap(),
                                    bottom: ins.get("bottom").unwrap(),
                                }
                            },
                            ..Default::default()
                        })
                        .unwrap();
                    let c = s
                        .eval::<(f32, f32, f32, f32)>("return f:GetBackdropColor()")
                        .unwrap();
                    b.bg_color = [c.0, c.1, c.2, c.3];
                    let c = s
                        .eval::<(f32, f32, f32, f32)>("return f:GetBackdropBorderColor()")
                        .unwrap();
                    b.border_color = [c.0, c.1, c.2, c.3];
                    b
                };
                let want = pieces(fr, &spec, eff);
                let ctx = format!("shape {i} scale {scale} size {w}x{h}");
                assert_eq!(quads.len(), want.len(), "{ctx}");
                for (k, (q, p)) in quads.iter().zip(&want).enumerate() {
                    let (path, rect, color, tc, alpha) = q;
                    let xs = p.corners.map(|c| c[0]);
                    let ys = p.corners.map(|c| c[1]);
                    let bb = Rect::new(
                        ys.iter().copied().fold(f32::MAX, f32::min),
                        xs.iter().copied().fold(f32::MAX, f32::min),
                        ys.iter().copied().fold(f32::MIN, f32::max),
                        xs.iter().copied().fold(f32::MIN, f32::max),
                    );
                    let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
                    // The draw normalizes a rect, so an edge shorter than its two corners, whose
                    // anchors cross, draws as the same box either way.
                    let (lo_y, hi_y) = (rect.bottom.min(rect.top), rect.bottom.max(rect.top));
                    let (lo_x, hi_x) = (rect.left.min(rect.right), rect.left.max(rect.right));
                    assert!(
                        close(lo_x, bb.left)
                            && close(hi_x, bb.right)
                            && close(hi_y, bb.top)
                            && close(lo_y, bb.bottom),
                        "{ctx} piece {k}: rect {rect:?} want {bb:?}"
                    );
                    let want_path = if p.is_bg { "bg" } else { "edge" };
                    assert_eq!(path, want_path, "{ctx} piece {k}");
                    let want_color = if p.is_bg {
                        spec.bg_color
                    } else {
                        spec.border_color
                    };
                    assert_eq!(*color, Some(want_color), "{ctx} piece {k} colour");
                    assert_eq!(*alpha, 0.5, "{ctx} piece {k}: the frame's alpha");
                    // An untiled background keeps the whole texture (no `SetTexCoord`).
                    let uvs = match tc {
                        Some(TexCoords::Corners(c)) => *c,
                        None => [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                        Some(other) => panic!("{ctx} piece {k}: {other:?}"),
                    };
                    for (a, b) in uvs.iter().zip(p.uvs.iter()) {
                        assert!(
                            close(a[0], b[0]) && close(a[1], b[1]),
                            "{ctx} piece {k}: uvs {uvs:?} want {:?}",
                            p.uvs
                        );
                    }
                }
            }
        }
    }
}

/// Each `SetBackdrop` links nine fresh regions, so a frame re-set on every show runs its link
/// positions past the draw key's field; they renumber in link order and the draw keeps it.
#[test]
fn a_backdrop_set_many_times_keeps_its_frames_draw_order() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame")
        f:SetPoint("CENTER", 0, 0)
        f:SetWidth(100); f:SetHeight(100)
        local first = f:CreateTexture(nil, "ARTWORK")
        first:SetAllPoints(f); first:SetTexture("first")
        for i = 1, 600 do f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" }) end
        local last = f:CreateTexture(nil, "ARTWORK")
        last:SetAllPoints(f); last:SetTexture("last")
        local r = { f:GetRegions() }
        assert(table.getn(r) == 11 and r[1] == first and r[11] == last, "link order kept")
    "#,
    )
    .unwrap();
    s.resolve();
    let paths: Vec<String> = s
        .extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p),
            _ => None,
        })
        .collect();
    let mut want = vec!["bg".to_string()];
    want.extend(std::iter::repeat_n("edge".to_string(), 8));
    want.extend(["first".to_string(), "last".to_string()]);
    assert_eq!(paths, want);
}

/// A piece a Button adopts into a slot is still the backdrop's (`+0x00..+0x20`), and its teardown
/// deletes it; the region-removed hook (`0x778cb0`) empties the slot, so the next
/// `SetNormalTexture("path")` builds a fresh texture that draws, not a write into the dead one.
#[test]
fn a_button_slot_holding_a_piece_lets_go_when_the_backdrop_does() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        b = CreateFrame("Button")
        b:SetPoint("CENTER", 0, 0)
        b:SetWidth(100); b:SetHeight(40)
        b:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        local piece = ({ b:GetRegions() })[1]
        b:SetNormalTexture(piece)
        assert(b:GetNormalTexture() == piece, "the slot holds the piece")
        b:SetBackdrop(nil)
        assert(b:GetNormalTexture() == nil, "the slot let go of the freed piece")
        b:SetNormalTexture("fresh")
        assert(b:GetNormalTexture():GetTexture() == "fresh", "a new texture answers")
    "#,
    )
    .unwrap();
    s.resolve();
    let drawn = s
        .extract()
        .into_iter()
        .any(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == "fresh"));
    assert!(drawn, "the fresh normal texture draws");
}

/// The border is on BORDER (`0x77e8d0`), and the draw layer outranks the frame within a level
/// (`0x765920`): a backdrop's edge draws over a later-linked frame's BACKGROUND texture at the
/// same strata and level, where the frame's own slot drew it under.
#[test]
fn a_border_draws_over_another_frames_background_at_its_level() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local a = CreateFrame("Frame")
        a:SetPoint("CENTER", 0, 0)
        a:SetWidth(100); a:SetHeight(100)
        a:SetBackdrop({ edgeFile = "edge" })
        local b = CreateFrame("Frame")
        b:SetAllPoints(a)
        local t = b:CreateTexture(nil, "BACKGROUND")
        t:SetAllPoints(b); t:SetTexture("under")
        assert(a:GetFrameLevel() == b:GetFrameLevel(), "one level")
    "#,
    )
    .unwrap();
    s.resolve();
    let paths: Vec<String> = s
        .extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p),
            _ => None,
        })
        .collect();
    let under = paths.iter().position(|p| p == "under").expect("drawn");
    let first_edge = paths.iter().position(|p| p == "edge").expect("drawn");
    assert!(first_edge > under, "the border draws over it: {paths:?}");
}

/// `0x77f0c0` re-applies both colours after the texcoords (`0x77f3e9`, `0x77f3f4`), so a resize
/// repaints a piece an addon tinted itself.
#[test]
fn a_resize_repaints_a_piece_over_its_own_vertex_colour() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame")
        f:SetPoint("CENTER", 0, 0)
        f:SetWidth(100); f:SetHeight(100)
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        f:SetBackdropBorderColor(1, 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    let left = "local r = { f:GetRegions() } return r[2]:GetVertexColor()";
    s.run("local r = { f:GetRegions() } r[2]:SetVertexColor(0, 1, 0)")
        .unwrap();
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>(left).unwrap(),
        (0.0, 1.0, 0.0, 1.0)
    );
    s.run("f:SetWidth(120)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>(left).unwrap(),
        (1.0, 0.0, 0.0, 1.0),
        "the resize put the border colour back"
    );
}

/// The background's BOTTOMRIGHT anchor carries the top inset as its y (`0x77e9ae` loads `+0x50`),
/// which `GetPoint` shows though the drawn rect never does.
#[test]
fn the_bg_bottomright_anchor_answers_the_top_inset() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame")
        f:SetBackdrop({ bgFile = "bg", insets = { left = 3, right = 4, top = 7, bottom = 9 } })
        local bg = f:GetRegions()
        local found
        for i = 1, bg:GetNumPoints() do
            local point, rel, relPoint, x, y = bg:GetPoint(i)
            if point == "BOTTOMRIGHT" then
                assert(rel == f and relPoint == "BOTTOMRIGHT", "on the frame's corner")
                assert(x == -4 and y == 7, "x " .. x .. ", y " .. y .. ": the top inset")
                found = true
            elseif point == "BOTTOMLEFT" then
                assert(x == 3 and y == 9, "the bottom-left takes the bottom inset")
            end
        end
        assert(found, "a BOTTOMRIGHT anchor")
    "#,
    )
    .unwrap();
}

/// A teardown kills the pieces' layout nodes; whatever anchored to one re-resolves against the
/// missing rect in the dirty-scope solve, exactly as a from-scratch solve of the same model does.
#[test]
fn a_teardown_re_resolves_what_anchored_to_a_piece() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame")
        f:SetPoint("CENTER", 0, 0)
        f:SetWidth(100); f:SetHeight(100)
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        local g = CreateFrame("Frame")
        dep = g:CreateTexture(nil, "ARTWORK")
        dep:SetTexture("dep")
        dep:SetWidth(10); dep:SetHeight(10)
        dep:SetPoint("TOPLEFT", ({ f:GetRegions() })[1], "BOTTOMLEFT", 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    let drawn = |s: &UiScript| {
        s.extract().into_iter().any(|q| {
            q.rect.is_some()
                && matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == "dep")
        })
    };
    assert!(drawn(&s), "anchored to the live background");
    s.run("f:SetBackdrop(nil)").unwrap();
    s.resolve();
    let incremental = {
        let m = s.model_ref();
        (m.resolved.clone(), m.region_resolved.clone())
    };
    {
        let mut m = s.model_mut();
        m.layout_scope.invalidate();
        m.touch_layout();
    }
    s.resolve();
    let full = {
        let m = s.model_ref();
        (m.resolved.clone(), m.region_resolved.clone())
    };
    assert!(
        incremental == full,
        "the dirty-scope solve matches a full one"
    );
    assert!(!drawn(&s), "its anchor target is gone");
}
