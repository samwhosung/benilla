//! Regions join the anchor layout.
//!
//! The smeared-merchant root cause: region `<Size>`/`<Anchors>` used to be dropped — a region either
//! filled its owner or (with a size) drew centered, and its anchors were ignored. Regions now resolve
//! through the same leaf math as frames, off their own anchors and their own span; an edge nothing
//! pins does NOT fall back to the owner's (decision 1664 retired that — the owner supplies scale and
//! nothing else), because the span is content-derived and a pinned edge plus a span is a rect.

use super::common::script;
use crate::layout::Rect;
use crate::script::*;

/// A texture region's resolved rect, found by (a fragment of) its texture path.
fn region_tex_rect(s: &UiScript, needle: &str) -> Rect {
    s.extract()
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } if p.contains(needle) => q.rect,
            _ => None,
        })
        .unwrap_or_else(|| panic!("no texture region rect for {needle}"))
}

/// A fontstring region's resolved rect, found by its exact text.
fn region_text_rect(s: &UiScript, text: &str) -> Rect {
    s.extract()
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } if t == text => q.rect,
            _ => None,
        })
        .unwrap_or_else(|| panic!("no text region rect for {text:?}"))
}

#[test]
fn region_texture_anchored_topleft_resolves_exact_rect() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0)   -- owner [bottom 0, left 0, top 50, right 100]
        f:SetWidth(100); f:SetHeight(50)
        local t = f:CreateTexture("Tex", "ARTWORK")
        t:SetTexture("Interface\\Icon")
        t:SetWidth(24); t:SetHeight(24)
        t:SetPoint("TOPLEFT", 4, -4)
    "#,
    )
    .unwrap();
    s.resolve();
    // TOPLEFT +(4,-4), 24×24 in the owner: left 4, top 46, right 28, bottom 22.
    assert_eq!(
        region_tex_rect(&s, "Interface\\Icon"),
        Rect::new(22.0, 4.0, 46.0, 28.0)
    );
}

/// A FontString's implicit extent is its measured TEXT, **floored at one FrameXML unit**
/// (`CSimpleFontString::GetWidth 0x772930` / `GetHeight 0x772a60`). So a
/// single-anchored FontString whose measure has not landed is a 1×1 box seated on its anchor,
/// not a collapse onto the pinned edge and not the owner's rect: the floor is what makes such a
/// FontString **always resolve**, and it is why the resolver needs no owner-edge fallback at all.
#[test]
fn region_fontstring_span_floors_at_one_unit_until_measured() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0)   -- owner [0, 0, 50, 100]
        f:SetWidth(100); f:SetHeight(50)
        local fs = f:CreateFontString("FS", "ARTWORK")
        fs:SetText("Name")
        fs:SetJustifyH("LEFT")
        fs:SetPoint("LEFT", 5, 0)        -- left edge pinned; no size
    "#,
    )
    .unwrap();
    s.resolve();
    // Pending measure: both spans floor at one unit. x runs right from the pinned left (5→6);
    // a LEFT point pins the y-CENTER only (25), so y is the centre ± half a unit.
    assert_eq!(
        region_text_rect(&s, "Name"),
        Rect::new(24.5, 5.0, 25.5, 6.0)
    );
    // Measured, the rect is the text extent seated on the anchor: 40×12 around y-center 25.
    let answers: Vec<(u32, f32, f32, u64)> = s
        .fontstrings_needing_measure()
        .iter()
        .map(|r| (r.id, 40.0, 12.0, r.key))
        .collect();
    s.set_measured_text_unwrapped(&answers);
    s.resolve();
    assert_eq!(
        region_text_rect(&s, "Name"),
        Rect::new(19.0, 5.0, 31.0, 45.0)
    );
}

/// `ExhaustionLevelFillBar`'s exact shape, both ways: authored width **0**, one TOPLEFT anchor,
/// and a `<Color>`.
///
/// The colour form installs a real 8×8 texture before any resolve (`0x7700a9` → `0x770360` →
/// `0x44a900`), so `CSimpleTexture::GetWidth 0x770720` answers **8** for the authored zero and
/// `combineEdge`'s first leg fires: RIGHT = LEFT + 8, and `assemble 0x767a20` returns 1. Strip the
/// colour and there is no `CGxTex*` at all — the getter answers `0.0`, both `combineEdge` legs
/// fail their `span != 0.0` test, and the rect never resolves. Our owner-edge fallback used to
/// turn that second case into the owner's **full width** (1348's 1024-point white band); it is
/// gone, so the two cases now differ by 8 points and a rect, exactly as they do on the reference.
#[test]
fn a_zero_width_solid_spans_eight_units_and_its_artless_twin_gets_no_rect() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0)   -- owner [0, 0, 50, 100]
        f:SetWidth(100); f:SetHeight(50)
        Fill = f:CreateTexture("Fill", "BORDER")
        Fill:SetTexture(1, 1, 1, 1)      -- the <Color> form: an 8x8 solid
        Fill:SetWidth(0); Fill:SetHeight(13)
        Fill:SetPoint("TOPLEFT", 0, 0)
        Bare = f:CreateTexture("Bare", "BORDER")
        Bare:SetWidth(0); Bare:SetHeight(13)
        Bare:SetPoint("TOPLEFT", 0, 0)   -- same shape, no art at all
    "#,
    )
    .unwrap();
    s.resolve();
    s.run(
        r#"
        assert(Fill:GetLeft() == 0 and Fill:GetRight() == 8,
               "the solid spans its 8 texels, got " .. tostring(Fill:GetRight()))
        assert(Fill:GetTop() == 50 and Fill:GetBottom() == 37, "and its authored 13 of height")
        assert(Bare:GetLeft() == nil, "no art, no span, no rect — not the owner's width")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

#[test]
fn templateless_lua_region_without_anchors_never_draws() {
    // Decision 1310 (`CreateTexture 0x773a20`): the creation-path implicit
    // anchor fires from Lua CreateTexture only on a template-registry hit — a templateless region
    // gets NOTHING, stays rect-less (the resolver has no zero-anchor fallback), and never renders,
    // its explicit size notwithstanding. This replaced the old draws-centered-at-its-size
    // fallback, which was refuted at the bytes (it was B180's squashed stack-split dialog).
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0)   -- owner [0, 0, 50, 100]
        f:SetWidth(100); f:SetHeight(50)
        local t = f:CreateTexture("Tex", "ARTWORK")
        t:SetTexture("Interface\\Ring")
        t:SetWidth(24); t:SetHeight(24)  -- no anchors, and no template ⇒ no rect, no draw
        assert(t:GetLeft() == nil, "a rect-less region reads nil edges")
    "#,
    )
    .unwrap();
    s.resolve();
    let drawn = s.extract().iter().any(|q| {
        matches!(&q.content,
            QuadContent::Texture { path: Some(p), .. } if p.contains("Interface\\Ring"))
            && q.rect.is_some()
    });
    assert!(!drawn, "a templateless anchor-less region must not draw");
}

#[test]
fn region_set_all_points_fills_owner() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0)   -- owner [0, 0, 50, 100]
        f:SetWidth(100); f:SetHeight(50)
        local t = f:CreateTexture("Tex", "ARTWORK")
        t:SetTexture("Interface\\Fill")
        t:SetWidth(24); t:SetHeight(24)  -- size present, but setAllPoints wins
        t:SetAllPoints()
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        region_tex_rect(&s, "Interface\\Fill"),
        Rect::new(0.0, 0.0, 50.0, 100.0)
    );
}

/// A region anchored to a **sibling region by name** resolves against the sibling's rect — the
/// merchant label-plate shape (`plate LEFT → $parentSlot RIGHT −9`), which the old owner-fallback
/// mis-anchored to the row's right edge (the jutting-plates bug the director's A/B caught). The
/// plate is declared BEFORE the slot so only the resolve fixpoint (not declaration order) can
/// order the chain; the slot itself is region-anchored to the row, one deeper than owner-direct.
#[test]
fn region_anchors_to_sibling_region_by_name() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local row = CreateFrame("Button", "Row")
        row:SetPoint("BOTTOMLEFT", 100, 100); row:SetWidth(153); row:SetHeight(44)
        -- Plate first: its target doesn't exist yet at SetPoint time in XML order terms; the
        -- name lookup happens at SetPoint (both exist by then in real loads), the RECT ordering
        -- is the fixpoint's job.
        local plate = row:CreateTexture("RowPlate", "BACKGROUND")
        plate:SetWidth(128); plate:SetHeight(78)
        local slot = row:CreateTexture("RowSlot", "BACKGROUND")
        slot:SetWidth(64); slot:SetHeight(64)
        slot:SetPoint("TOPLEFT", "Row", "TOPLEFT", -13, 13)
        plate:SetPoint("LEFT", "RowSlot", "RIGHT", -9, -18)
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let rect = |name: &str| {
        quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path, .. } if path.is_none())
                    && q.rect.is_some_and(|r| {
                        (r.width() - if name == "RowSlot" { 64.0 } else { 128.0 }).abs() < 0.1
                    })
            })
            .and_then(|q| q.rect)
            .unwrap_or_else(|| panic!("no rect for {name}"))
    };
    // Slot: row TOPLEFT (100..253, 100..144) → slot at (87, 157)-(151, 93)… y-up: top = 144+13 = 157.
    let slot = rect("RowSlot");
    assert_eq!(
        (slot.left, slot.top),
        (87.0, 157.0),
        "slot at row TOPLEFT (-13,13)"
    );
    assert_eq!(slot.right, 151.0);
    // Plate LEFT anchors to the SLOT's RIGHT (151) − 9 = 142 — NOT the row's right edge (253),
    // which is where the old owner-fallback shoved it (253 − 9 + 128 = jutting past everything).
    let plate = rect("RowPlate");
    assert_eq!(plate.left, 142.0, "plate.LEFT = slot.RIGHT − 9");
    assert_eq!(
        plate.right, 270.0,
        "plate spans its 128 width from the slot edge"
    );
    // The LEFT single-point anchor centers the plate vertically on the target point (y-up):
    // slot center-y = (93+157)/2 = 125, −18 → 107; ±39.
    assert_eq!((plate.bottom, plate.top), (68.0, 146.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetPortraitTexture(region, unit)` (the live model-bake portrait) binds a Texture region to a unit
/// token, carried out through [`QuadContent::Texture::portrait_unit`] with no BLP path/color of its
/// own (the app supplies the off-screen bake). A later `SetTexture` makes it an ordinary texture again,
/// dropping the binding.
#[test]
fn set_portrait_texture_binds_unit_token_then_settexture_clears_it() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "PFrame")
        f:SetPoint("TOPLEFT", 0, 0)
        f:SetWidth(100); f:SetHeight(100)
        local p = f:CreateTexture("PFramePortrait", "BACKGROUND")
        p:SetWidth(64); p:SetHeight(64)
        p:SetPoint("TOPLEFT", 0, 0)
        SetPortraitTexture(p, "player")
    "#,
    )
    .unwrap();
    s.resolve();
    let bound = s.extract().into_iter().find_map(|q| match q.content {
        QuadContent::Texture {
            portrait_unit: Some(u),
            path,
            color,
            circular,
            ..
        } => Some((u, path, color, circular)),
        _ => None,
    });
    let (unit, path, color, circular) = bound.expect("a portrait-bound quad is extracted");
    assert_eq!(unit, "player");
    assert_eq!(path, None, "the model bake carries no BLP path");
    assert_eq!(color, None, "the model bake carries no vertex color");
    assert!(circular, "the frame-ring portrait is the round stencil");

    // SetTexture reverts the region to an ordinary texture — the live-unit binding drops.
    s.run(r#" PFramePortrait:SetTexture("Interface\\Icons\\INV_Misc_QuestionMark") "#)
        .unwrap();
    s.resolve();
    assert!(
        !s.extract().iter().any(|q| matches!(
            &q.content,
            QuadContent::Texture {
                portrait_unit: Some(_),
                ..
            }
        )),
        "SetTexture clears the live-unit portrait binding"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `BenillaSetBoothTexture(region, token)` — the paper doll's **square** booth binding (decision
/// 0208 §5): the same `portrait_unit` carriage as `SetPortraitTexture`, but `circular` stays
/// false (the model pane samples the body bake edge to edge, no frame ring to mask for).
#[test]
fn benilla_set_booth_texture_binds_square() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "DollFrame")
        f:SetPoint("TOPLEFT", 0, 0)
        f:SetWidth(200); f:SetHeight(300)
        local m = f:CreateTexture("DollFrameModel", "ARTWORK")
        m:SetAllPoints()
        BenillaSetBoothTexture(m, "paperdoll")
    "#,
    )
    .unwrap();
    s.resolve();
    let bound = s.extract().into_iter().find_map(|q| match q.content {
        QuadContent::Texture {
            portrait_unit: Some(u),
            circular,
            ..
        } => Some((u, circular)),
        _ => None,
    });
    let (token, circular) = bound.expect("a booth-bound quad is extracted");
    assert_eq!(token, "paperdoll");
    assert!(
        !circular,
        "the booth pane draws square — no inscribed-circle mask"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The quest-log detail pane's shape (the 0109 look fix): a LONG region anchor chain — the real
/// client's QuestLogFrame chains ~15 regions deep (title → objectives → obj1..10 → description →
/// rewards) — with a frame (the reward item button) anchored to the chain's tail. The resolver
/// must run to a true fixpoint: the old bounded rounds (2 frame × 3 region passes) left links past
/// ~6 on silent owner-edge fallbacks and dropped the tail-anchored FRAME to the screen origin —
/// decision 0088 §2's "button anchored to a chained region falls to the screen origin" finding,
/// which forced every window into invented fixed offsets instead of ref-verbatim chains.
#[test]
fn long_region_chain_resolves_and_a_frame_binds_to_its_tail() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Book")
        f:SetPoint("TOPLEFT", 0, 0)
        f:SetWidth(384); f:SetHeight(512)
        for i = 1, 12 do
            local t = f:CreateTexture("Line" .. i, "ARTWORK")
            t:SetTexture("Interface\\Line" .. i)
            t:SetWidth(300); t:SetHeight(10)
            if i == 1 then
                t:SetPoint("TOPLEFT", "Book", "TOPLEFT", 5, -5)
            else
                t:SetPoint("TOPLEFT", "Line" .. (i - 1), "BOTTOMLEFT", 0, -2)
            end
        end
        local b = CreateFrame("Button", "TailButton", f)
        b:SetWidth(147); b:SetHeight(41)
        b:SetPoint("TOPLEFT", "Line12", "BOTTOMLEFT", 0, -6)
    "#,
    )
    .unwrap();
    s.resolve();
    // Screen top 600; Line1 top = 600−5 = 595; each link steps 12 (10 height + 2 gap).
    for i in 1..=12u32 {
        let top = 595.0 - 12.0 * (i as f32 - 1.0);
        assert_eq!(
            region_tex_rect(&s, &format!("Interface\\Line{i}")),
            Rect::new(top - 10.0, 5.0, top, 305.0),
            "link {i} must resolve at its true chain position, not an owner-edge fallback"
        );
    }
    // The tail frame: TOPLEFT = Line12's BOTTOMLEFT (5, 453) − 6 → top 447, spans 147×41.
    let button = s
        .extract()
        .iter()
        .find_map(|q| match q.target {
            crate::order::ZTarget::Frame(_) => q.rect.filter(|r| (r.width() - 147.0).abs() < 0.1),
            _ => None,
        })
        .expect("the tail-anchored button resolved a rect (not dropped to origin)");
    assert_eq!(
        button,
        Rect::new(406.0, 5.0, 447.0, 152.0),
        "the frame binds to the CHAIN TAIL's resolved rect"
    );
}

/// Decision 0088 §2 pinned "a child frame shown at runtime does not draw its own `<Layers>`
/// FontStrings" — the engine constraint that forced every window FLAT. Re-tested after the
/// resolver fixpoint: a child frame created and SHOWN at runtime, carrying its own
/// text + texture regions, must extract both quads at the child's resolved position.
#[test]
fn child_frame_layers_regions_render_after_the_fixpoint() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local parent = CreateFrame("Frame", "Win")
        parent:SetPoint("TOPLEFT", 10, -10)
        parent:SetWidth(400); parent:SetHeight(300)
        local child = CreateFrame("Frame", "SubPanel", parent)
        child:SetPoint("TOPLEFT", "Win", "TOPLEFT", 20, -20)
        child:SetWidth(200); child:SetHeight(100)
        local fs = child:CreateFontString("SubText", "ARTWORK")
        fs:SetPoint("TOPLEFT", "SubPanel", "TOPLEFT", 5, -5)
        fs:SetWidth(150); fs:SetHeight(12)
        fs:SetText("hello from the child")
        local tex = child:CreateTexture("SubTex", "BACKGROUND")
        tex:SetTexture("Interface\\SubFill")
        tex:SetAllPoints()
        child:Hide()
        child:Show()
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let text = quads.iter().find(|q| {
        matches!(&q.content, crate::script::QuadContent::Text { text: Some(t), .. } if t == "hello from the child")
    });
    assert!(
        text.is_some_and(|q| q.rect.is_some_and(|r| (r.left - 35.0).abs() < 0.5)),
        "the child frame's own FontString must extract at its resolved spot (got {:?})",
        text.and_then(|q| q.rect)
    );
    let tex = quads.iter().find(|q| {
        matches!(&q.content, crate::script::QuadContent::Texture { path: Some(p), .. } if p == "Interface\\SubFill")
    });
    assert!(
        tex.is_some_and(|q| q.rect.is_some()),
        "the child's texture too"
    );
}

/// `SetAlpha`/`GetAlpha` on a Texture/FontString — the region's *own* alpha, distinct from its
/// owner frame's. A region draws at `ownAlpha × ownerFrame.alpha`: a single hop to the immediate
/// owner (`SetAlpha 0x76a690` overwrite-cascades onto child *frames* and only invalidates child
/// regions). The getter must return the region's value, never the frame's: the ref kit ramps a
/// texture by reading it back (`CastingBarFlash:SetAlpha(GetAlpha() + step)`).
#[test]
fn region_alpha_is_its_own_and_multiplies_the_owner_frames() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "AlphaOwner")
        f:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
        f:SetWidth(10); f:SetHeight(10)
        tex = f:CreateTexture("AlphaTex", "ARTWORK")
        tex:SetTexture("Interface\\Foo")
        assert(tex:GetAlpha() == 1, "an untouched region is opaque")
    "#,
    )
    .unwrap();
    s.resolve();

    let quad_alpha = |s: &UiScript| {
        s.extract()
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(_), .. }))
            .expect("texture quad")
            .alpha
    };
    assert_eq!(quad_alpha(&s), 1.0);

    // The region's own alpha alone.
    s.run("tex:SetAlpha(0.5)").unwrap();
    assert_eq!(s.eval::<f32>("return tex:GetAlpha()").unwrap(), 0.5);
    assert_eq!(quad_alpha(&s), 0.5);

    // …times the owner frame's. The frame's SetAlpha does NOT overwrite the region's own value.
    s.run("f:SetAlpha(0.5)").unwrap();
    assert_eq!(
        s.eval::<f32>("return tex:GetAlpha()").unwrap(),
        0.5,
        "the frame's SetAlpha leaves the region's own alpha alone"
    );
    assert_eq!(quad_alpha(&s), 0.25, "0.5 region × 0.5 frame");

    // A hidden region draws nothing regardless of alpha.
    s.run("tex:SetAlpha(1); tex:Hide()").unwrap();
    assert!(
        !s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Texture { path: Some(_), .. })),
        "a hidden region emits no quad"
    );
}

/// **The texture-colour composition law**: a region's own solid colour is a real *texel*
/// (`SetTexture(r,g,b,a)` → `0x770360` generates an 8×8 block at `+0xcc`), its vertex colour is a
/// *separate* slot (`SetVertexColor 0x77f750`, `+0xb8`), and the draw **multiplies** them per
/// channel, alpha included — it does not replace.
///
/// This is the reference `SkillFrame` row trough, verbatim: declared `<Color 1,1,1,0.2>`, then
/// `SetVertexColor(0, 0, 0.75, 0.5)`'d. It draws at alpha `0.2 × 0.5 = 0.1`. benilla stored ONE
/// colour slot until this test, which made the second call replace the first and drew it at 0.5.
#[test]
fn a_solid_colour_texel_multiplies_with_the_vertex_colour() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0)
        f:SetWidth(100) f:SetHeight(50)
        trough = f:CreateTexture("Trough", "BACKGROUND")
        trough:SetTexture(1, 1, 1, 0.2)
    "#,
    )
    .unwrap();
    s.resolve();

    let solid = |s: &UiScript| {
        s.extract()
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Texture {
                    path: None, color, ..
                } => Some(*color),
                _ => None,
            })
            .expect("solid-colour quad")
    };
    assert_eq!(
        solid(&s),
        Some([1.0, 1.0, 1.0, 0.2]),
        "untinted, the texel draws as declared"
    );

    s.run("trough:SetVertexColor(0, 0, 0.75, 0.5)").unwrap();
    assert_eq!(
        solid(&s),
        Some([0.0, 0.0, 0.75, 0.1]),
        "texel x vertex, alpha included: 0.2 x 0.5 = 0.1, NOT 0.5"
    );
    // The API readback is the vertex slot itself, not the product — the two are distinct storage.
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return trough:GetVertexColor()")
            .unwrap(),
        (0.0, 0.0, 0.75, 0.5)
    );

    // Art and a solid colour share the `+0xcc` slot: setting a path releases the generated texel,
    // and what is left is the tint alone for the renderer to modulate the sample by.
    s.run(r#"trough:SetTexture("Interface\\Bar.blp")"#).unwrap();
    let tinted = s
        .extract()
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains("Bar") => Some(*color),
            _ => None,
        })
        .expect("art quad");
    assert_eq!(
        tinted,
        Some([0.0, 0.0, 0.75, 0.5]),
        "the vertex colour outlives the texel it was multiplying"
    );
}

/// **`SetDesaturated` rides the extract, and only against real ART**.
///
/// The state was stored from the day the verb landed and read by nobody, which is why every
/// greyed-out affordance in the UI was a brightness tint (B162's talent tree). The flag now travels
/// on the quad, so this pins the two ends of that wire — set/clear — plus the one carve-out: a
/// PATHLESS solid has its colour folded into the quad's tint and draws against a 1x1 white texel,
/// so a shader that greys the texel would grey white and change nothing. Carrying the flag there
/// would read as honoured while doing nothing at all, so extract drops it.
#[test]
fn desaturation_rides_the_extract_for_art_and_never_for_a_solid() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "DsOwner")
        f:SetPoint("BOTTOMLEFT", 0, 0)
        f:SetWidth(100) f:SetHeight(50)
        art = f:CreateTexture("DsArt", "ARTWORK")
        art:SetTexture("Interface\\Icons\\Spell_Nature_Sleep")
        art:SetAllPoints()
        solid = f:CreateTexture("DsSolid", "OVERLAY")
        solid:SetTexture(1, 0, 0)
        solid:SetAllPoints()
    "#,
    )
    .unwrap();
    s.resolve();
    let grey = |s: &UiScript, want_path: bool| {
        s.extract()
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Texture {
                    path, desaturated, ..
                } if path.is_some() == want_path => Some(*desaturated),
                _ => None,
            })
            .expect("the quad")
    };
    assert!(!grey(&s, true), "art starts full colour");

    s.run("art:SetDesaturated(1) solid:SetDesaturated(1)")
        .unwrap();
    assert!(grey(&s, true), "the flag reaches the art quad");
    assert!(
        !grey(&s, false),
        "a pathless solid never carries it — greying a white texel is a no-op dressed as a feature"
    );

    // And it clears — the reference's own `SetItemButtonDesaturated(button, nil)` restore.
    s.run("art:SetDesaturated(nil)").unwrap();
    assert!(!grey(&s, true), "clearing the flag restores full colour");
}

/// Read a Texture region's desaturation state straight off the model, by name.
///
/// It has no getter in Lua on purpose — `IsDesaturated` (`0x79c2c0`) exists, but its return shape
/// is unconfirmed, and inventing one to make a test convenient is how an unverified API
/// gets shipped (decision 1327's own residual). The extract quad is the other way to see it, but a
/// cleared texture emits no quad at all, which is exactly the case these tests need to observe.
fn desaturated(s: &UiScript, name: &str) -> bool {
    let lua = s.lua();
    let model = lua.app_data_ref::<crate::script::Model>().expect("model");
    let id = *model.region_names.get(name).expect("region name");
    let h = *model.id_to_region.get(&id).expect("region handle");
    model.region_data.get(&h).is_some_and(|d| d.desaturated)
}

/// **`SetTexture` clears the desaturation — except when the path does not actually change**.
///
/// `+0x128` is a `CGxShader*`, and `CSimpleTexture::SetTexture` writes it from a shader index the
/// Lua binding always passes as slot 0 (permanently NULL). Storing a desaturate boolean *beside*
/// the texture handle — which is what benilla did on 1327 — diverges on the single most common
/// FrameXML shape there is: a repaint that re-sets the icon and expects the grey to follow the art.
/// The same-path early-out (`0x770225`) is what makes the *idempotent* repaint keep its grey, and
/// it is the half a plausible implementation drops.
#[test]
fn set_texture_clears_desaturation_unless_the_path_is_unchanged() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "ClrOwner")
        icon = f:CreateTexture("ClrIcon", "ARTWORK")
        icon:SetTexture("Interface\\Icons\\Spell_Nature_Sleep")
        icon:SetDesaturated(1)
    "#,
    )
    .unwrap();
    let grey = |s: &UiScript| desaturated(s, "ClrIcon");

    // The idempotent repaint: same file, so the client returns before it can clear anything.
    s.run(r#"icon:SetTexture("Interface\\Icons\\Spell_Nature_Sleep")"#)
        .unwrap();
    assert!(grey(&s), "re-setting the SAME art keeps the grey");

    // A different file reaches the write and zeroes the slot.
    s.run(r#"icon:SetTexture("Interface\\Icons\\Spell_Fire_Fireball")"#)
        .unwrap();
    assert!(!grey(&s), "a texture CHANGE clears the desaturation");

    // The clear forms take the same leg (`test esi,esi` falls through to the write).
    s.run("icon:SetDesaturated(1) icon:SetTexture(nil)")
        .unwrap();
    assert!(!grey(&s), "SetTexture(nil) clears it too");

    // The COLOUR form is a different function (`0x770360`) and is not one of the field's writers.
    s.run(r#"icon:SetTexture("Interface\\Icons\\Spell_Nature_Sleep") icon:SetDesaturated(1)"#)
        .unwrap();
    s.run("icon:SetTexture(1, 0, 0)").unwrap();
    assert!(grey(&s), "the colour form does not touch the shader slot");
}

/// **`SetDesaturated`'s argument truth table has two arms that read backwards**
/// (`0x6f1c10`'s jump table).
///
/// `0x6f1c10(L, 2, default=1)` takes its DEFAULT on `LUA_TNONE`, so a bare `SetDesaturated()` greys
/// — the opposite of the `if flag then` an implementation writes without looking. And a number is
/// truncated to an int, so `SetDesaturated(0)` clears where truthiness would have greyed.
#[test]
fn set_desaturated_takes_the_clients_argument_truth_table() {
    let s = script();
    s.run(r#"f = CreateFrame("Frame", "ArgOwner") tex = f:CreateTexture("ArgTex", "ARTWORK")"#)
        .unwrap();
    let grey = |s: &UiScript| desaturated(s, "ArgTex");

    // NO ARGUMENT is ON — the LUA_TNONE default arm, not the nil arm.
    s.run("ArgTex:SetDesaturated()").unwrap();
    assert!(
        grey(&s),
        "a bare SetDesaturated() greys (LUA_TNONE default)"
    );

    s.run("ArgTex:SetDesaturated(nil)").unwrap();
    assert!(!grey(&s), "nil clears");

    // A number truncating to zero clears; a non-zero one greys.
    s.run("ArgTex:SetDesaturated(1) ArgTex:SetDesaturated(0)")
        .unwrap();
    assert!(!grey(&s), "0 clears — the number arm truncates to int");
    s.run("ArgTex:SetDesaturated(0.5)").unwrap();
    assert!(!grey(&s), "0.5 truncates to 0 and clears");

    // Both booleans, and Dewdrop's `true` (the call 98 corpus addons reach).
    s.run("ArgTex:SetDesaturated(true)").unwrap();
    assert!(grey(&s), "true greys");
    s.run("ArgTex:SetDesaturated(false)").unwrap();
    assert!(!grey(&s), "false clears");
}

/// The draw gate is the TEXTURE slot, never the colour: `0x7706e0` tests `+0xcc` and emits NOTHING
/// when it is empty, whatever the vertex colour holds. Since the tint deliberately survives
/// `SetTexture(nil)` ("a tint outlives the art it was tinting"), a cleared region used to leak its
/// tint out of extract as a solid plate — an occupied action button going empty on a character
/// switch drew its surviving 1/1/1 usable-tint as a solid WHITE square (the
/// 2026-07-10 grey wells were the same class).
#[test]
fn a_vertex_colour_without_a_texture_draws_nothing() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0)
        f:SetWidth(100) f:SetHeight(50)
        icon = f:CreateTexture("Icon", "ARTWORK")
        icon:SetTexture("Interface\\Icons\\Spell_Nature_Sleep")
        icon:SetVertexColor(1, 1, 1)
    "#,
    )
    .unwrap();
    s.resolve();
    let drawn = |s: &UiScript| {
        s.extract()
            .iter()
            .filter(|q| {
                matches!(&q.content, QuadContent::Texture { path, color, .. }
                    if path.is_some() || color.is_some())
            })
            .count()
    };
    assert_eq!(drawn(&s), 1, "tinted art draws");

    // The slot empties: the art clears, the tint stays (distinct storage) — and nothing draws.
    s.run("icon:SetTexture(nil)").unwrap();
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return icon:GetVertexColor()")
            .unwrap(),
        (1.0, 1.0, 1.0, 1.0),
        "the tint survives the clear"
    );
    assert_eq!(
        drawn(&s),
        0,
        "no texture at +0xcc -> emit NOTHING, never a solid plate of the surviving tint"
    );
}

/// **`SetTexture` ignores arguments past the path**, because the client's does.
///
/// The path form reads ONE argument (`0x770200`); only the colour form (`0x770360`) reads up to
/// four, and a C function takes what it wants off the Lua stack and ignores the rest. We typed the
/// trailing three as `Option<f32>` and so RAISED on a stray extra — `bad argument #3: error
/// converting Lua boolean to f32` — where the client silently accepts.
///
/// The live case is `_LazyPig/LazyPigMenu.lua:182`:
/// `texture_title:SetTexture("Interface\DialogFrame\UI-DialogBox-Header", true)`. The `true` is
/// meaningless in 1.12; the addon reached us only once the survey began seating the addon registry.
///
/// Asserted as "does not raise", which is the whole claim — there is no `GetTexture` binding to read
/// the path back through, and inventing one to satisfy a test would be the tail wagging the dog.
/// The colour form is exercised alongside so the fix cannot have made it lax: a non-numeric channel
/// must take the same default a missing one does, exactly as `lua_tonumber` of a non-number is 0.
#[test]
fn set_texture_ignores_arguments_past_the_path_like_the_client_does() {
    let s = script();
    s.run(
        r#"
        f = CreateFrame("Frame", "SetTexArgs")
        t = f:CreateTexture(nil, "ARTWORK")
        t:SetTexture("Interface\\DialogFrame\\UI-DialogBox-Header", true)
    "#,
    )
    .expect("a stray extra argument must not raise — the client ignores it");

    // The colour form, with a boolean and a numeric string among the channels.
    s.run("t:SetTexture(0.25, '0.5', true)")
        .expect("the colour form must tolerate what lua_tonumber tolerates");

    // And the ordinary shapes still work.
    s.run("t:SetTexture(nil) t:SetTexture('') t:SetTexture(1, 0, 0, 1)")
        .expect("clear, blank and the plain colour form are unaffected");
}

/// **`SetGradientAlpha` / `SetGradient` exist, store both stops, and paint.**
///
/// These were the single wall in front of the corpus's largest family: `FuBar\FuBar_Panel.lua:144`
/// calls `SetGradientAlpha` while building the bar, so all 20 FuBar plugins died there — right after
/// the chunk-name/`debugstack` fix got them that far.
///
/// Asserted on the region PAINTING (a gradient-only region must emit a quad, because the client
/// generates the gradient into the same texture slot the colour form fills) and on the midpoint
/// being what a one-tint quad shows. The full gradient stays on the region for a renderer that
/// grows a second stop.
#[test]
fn a_gradient_is_stored_whole_and_painted_as_its_midpoint() {
    let mut s = script();
    s.run(
        r#"
        f = CreateFrame("Frame", "GradProbe")
        f:SetWidth(100) f:SetHeight(20)
        f:SetPoint("TOPLEFT", 0, 0)
        t = f:CreateTexture(nil, "ARTWORK")
        t:SetAllPoints(f)
        -- FuBar's own call: white at both stops, ALPHA only, vertical.
        t:SetGradientAlpha("VERTICAL", 1, 1, 1, 0, 1, 1, 1, 0.5)
    "#,
    )
    .expect("SetGradientAlpha must exist and accept the client's argument shape");

    s.resolve();
    let painted = s.extract().iter().any(|q| {
        matches!(&q.content, crate::script::QuadContent::Texture { color: Some(c), .. }
            // midpoint of alpha 0.0 and 0.5
            if (c[3] - 0.25).abs() < 1e-6 && c[0] == 1.0)
    });
    assert!(
        painted,
        "a region carrying only a gradient must paint, at the midpoint of its two stops"
    );

    // The alpha-less twin: both stops opaque, and a non-"VERTICAL" token is horizontal.
    s.run("t:SetGradient('HORIZONTAL', 1, 0, 0, 0, 0, 1)")
        .expect("SetGradient takes six colour arguments and no alpha");
    s.resolve();
    let mid = s.extract().iter().find_map(|q| match &q.content {
        crate::script::QuadContent::Texture { color: Some(c), .. } => Some(*c),
        _ => None,
    });
    assert_eq!(
        mid.map(|c| [c[0], c[1], c[2], c[3]]),
        Some([0.5, 0.0, 0.5, 1.0]),
        "SetGradient's stops are opaque, so the midpoint alpha is 1"
    );
}

/// **The split itself: a Texture answers texture verbs and NOT text ones, and vice versa.**
///
/// Until this landed, one shared table meant a Texture answered `SetText` and a FontString answered
/// `SetTexture` — a superset in both directions (Texture's map `0x87c128` is 22 entries,
/// FontString's `0xcf5400` is 32, both tail-calling the Region map and stopping there).
#[test]
fn the_two_region_leaves_answer_their_own_maps() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        LeafOwner = CreateFrame("Frame", "LeafOwner")
        Tex = LeafOwner:CreateTexture("Tex", "ARTWORK")
        Str = LeafOwner:CreateFontString("Str", "ARTWORK")
        "#,
    )
    .unwrap();
    let has = |s: &crate::script::UiScript, obj: &str, m: &str| {
        s.eval::<String>(&format!("return type({obj}.{m})"))
            .unwrap()
            == "function"
    };

    // Texture-only, and an asymmetry worth noting: `SetVertexColor` is on BOTH leaves while
    // `GetVertexColor` is Texture-only. No reasonable partition invents that.
    for m in [
        "SetTexture",
        "GetTexture",
        "SetTexCoord",
        "SetBlendMode",
        "GetBlendMode",
        "SetTexCoordModifiesRect",
        "GetTexCoordModifiesRect",
        "GetVertexColor",
    ] {
        assert!(has(&s, "Tex", m), "a Texture answers {m}");
        assert!(!has(&s, "Str", m), "a FontString must NOT answer {m}");
    }
    // `SetRotation` is 1.12's PLAYERMODEL verb (`0x84f1fc`/`0x505f00`), not a region one — it is in
    // neither leaf map, and ours used to put it on Texture.
    for leaf in ["Tex", "Str"] {
        assert!(
            !has(&s, leaf, "SetRotation"),
            "SetRotation is not a region method"
        );
    }
    // FontString-only.
    for m in [
        "SetText",
        "GetText",
        "GetStringWidth",
        "SetJustifyH",
        "SetAlphaGradient",
    ] {
        assert!(has(&s, "Str", m), "a FontString answers {m}");
        assert!(!has(&s, "Tex", m), "a Texture must NOT answer {m}");
    }
    // On BOTH — and each leaf registers its own copy in the client, so these are NOT on the Region
    // map and must not be hoisted into it.
    for m in [
        "SetVertexColor",
        "SetAlpha",
        "Show",
        "Hide",
        "IsShown",
        "SetDrawLayer",
    ] {
        assert!(
            has(&s, "Tex", m) && has(&s, "Str", m),
            "{m} is on both leaves"
        );
    }
    // The Region map reaches both, through each leaf's own fallback.
    for m in crate::script::REGION_MAP_METHODS {
        assert!(
            has(&s, "Tex", m) && has(&s, "Str", m),
            "{m} is the Region map"
        );
    }
    // **`GetStringHeight` is GONE and must stay gone.** 1.12 has no such method on any table (0
    // hits in every encoding; the control `GetStringWidth` has 1), Blizzard's own FrameXML calls it
    // 0 times, and ours was a byte-identical duplicate of `GetHeight` — which is the method the
    // reference itself uses for this, falling through to the same cached measurement
    // `GetStringWidth` reads. Keeping the width and dropping the height is not an oversight: the
    // client really is asymmetric here.
    assert!(
        !has(&s, "Str", "GetStringHeight"),
        "1.12 has no GetStringHeight"
    );
    assert!(
        has(&s, "Str", "GetStringWidth"),
        "…but it does have GetStringWidth"
    );
    assert!(
        has(&s, "Str", "GetHeight"),
        "GetHeight is the replacement, via the Region map"
    );

    // The near-miss pair: Texture has SetGradientAlpha, FontString has SetAlphaGradient.
    assert!(has(&s, "Tex", "SetGradientAlpha") && !has(&s, "Str", "SetGradientAlpha"));
    assert!(has(&s, "Str", "SetAlphaGradient") && !has(&s, "Tex", "SetAlphaGradient"));
}

/// **`SetPortraitToTexture` is a GLOBAL in 1.12, not a Texture method.**
///
/// `reference/1.12-globals.tsv` marks it `engine`, and both of the reference's own call sites pass a
/// texture NAME: `ContainerFrame.lua:419` and `MailFrame.lua:174`. The first is the one that binds
/// us — we SOURCE `ContainerFrame.lua` off the patch chain, so the client's own file calls this
/// global inside our VM.
#[test]
fn set_portrait_to_texture_is_a_global_taking_a_name() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        PortHost = CreateFrame("Frame", "PortHost")
        Port = PortHost:CreateTexture("PortHostPortrait", "ARTWORK")
        SetPortraitToTexture("PortHostPortrait", "Interface\\ContainerFrame\\KeyRing-Bag-Icon")
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return Port:GetTexture()").unwrap(),
        "Interface\\ContainerFrame\\KeyRing-Bag-Icon"
    );
    // The Texture METHOD is gone — 1.12's Texture map has no such entry.
    assert_eq!(
        s.eval::<String>("return type(Port.SetPortraitToTexture)")
            .unwrap(),
        "nil",
        "1.12 has no Texture:SetPortraitToTexture — it is a global"
    );
    // An unknown name is not an error: the reference's callers compose names that may not exist
    // yet, and nothing here may raise on the sourced file's behalf.
    s.run(r#"SetPortraitToTexture("NoSuchPortrait", "Interface\\X")"#)
        .unwrap();
    assert!(s.errors().is_empty());
}

/// **`Region:GetWidth`/`GetHeight` are the VIRTUAL getters** — the same content-derived law the
/// rect resolver calls, because the Lua bindings dispatch through the same geometry-vtable slots
/// (`GetWidth 0x7a1e00` ends `ff 52 1c`, `GetHeight 0x7a2030` ends `ff 52 20`).
///
/// Both halves of the FontString row, each of which we had backwards:
///
/// * the **authored** value wins per axis — `0x772930`'s `jp 0x77294a` skips the measure entirely
///   when the authored width is not `0.0`, so `<Size x="300" y="0"/>` reports 300 from the author
///   and the height from the text (`CharacterNameText` is exactly that, in the reference's own
///   file), where we used to report the 37-point measure;
/// * on an axis authored `0` the **width** is the NATURAL, unwrapped extent — the very cell
///   `GetStringWidth` returns (`0x772890`'s `[fs+0xfc]`) — while the **height** is the *wrapped*
///   one (`0x7729b0`'s `[fs+0x100]`). We used to report the laid-out width for both.
#[test]
fn the_size_getters_take_the_author_first_then_the_natural_width_and_wrapped_height() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0); f:SetWidth(100); f:SetHeight(50)
        Sized = f:CreateFontString("Sized", "ARTWORK")
        Sized:SetPoint("TOPLEFT", 0, 0); Sized:SetWidth(300); Sized:SetText("Name")
        Auto = f:CreateFontString("Auto", "ARTWORK")
        Auto:SetPoint("TOPLEFT", 0, 0); Auto:SetText("Name")
    "#,
    )
    .unwrap();
    s.resolve();
    // A wrapped measure: 40 wide as laid out, 24 tall over two lines, 90 unwrapped.
    let answers: Vec<(u32, f32, f32, f32, u64)> = s
        .fontstrings_needing_measure()
        .iter()
        .map(|r| (r.id, 40.0, 24.0, 90.0, r.key))
        .collect();
    s.set_measured_text(&answers);
    s.run(
        r#"
        assert(Sized:GetWidth() == 300, "the AUTHORED width wins, got " .. tostring(Sized:GetWidth()))
        assert(Sized:GetHeight() == 24, "and the un-authored height is the measure, got " .. tostring(Sized:GetHeight()))
        assert(Auto:GetWidth() == 90, "no author ⇒ the NATURAL width, got " .. tostring(Auto:GetWidth()))
        assert(Auto:GetWidth() == Auto:GetStringWidth(), "which is GetStringWidth's own cell")
        assert(Auto:GetHeight() == 24, "and the WRAPPED height, got " .. tostring(Auto:GetHeight()))
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// The floor, and the state the reference does not have. `0x772930`/`0x772a60` end in a one-unit
/// clamp, so a genuinely empty string reads back **1**, never `0.0`. But a measure that has not
/// LANDED is not an extent at all — it is the host round-trip a VM with **no measurer installed**
/// still takes, which the reference has no equivalent of. A caller that guards
/// `if h <= 0 then return end` reads that zero as "not yet"; flooring it would tell them to stop
/// waiting. So: floor a known extent, not the absence of one.
#[test]
fn an_empty_string_reads_back_one_unit_and_a_pending_measure_reads_back_zero() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0); f:SetWidth(100); f:SetHeight(50)
        Pending = f:CreateFontString("Pending", "ARTWORK")
        Pending:SetPoint("TOPLEFT", 0, 0); Pending:SetText("Name")
        Empty = f:CreateFontString("Empty", "ARTWORK")
        Empty:SetPoint("TOPLEFT", 0, 0); Empty:SetText("")
        assert(Pending:GetHeight() == 0, "a pending measure is not a size, got " .. tostring(Pending:GetHeight()))
        assert(Empty:GetHeight() == 1, "an EMPTY string is one unit, got " .. tostring(Empty:GetHeight()))
        assert(Empty:GetWidth() == 1, "both axes")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// A TEXTURE's getters are the same virtual law with the other override behind them
/// (`0x770720`/`0x770790`): the authored value when it is not exactly `0.0`, else the art's own
/// texel extent, else `0.0` — and **no floor**, which only the FontString override carries. This
/// is the getter half of 1662; before it, Lua saw `0` for a size the screen was already drawing.
#[test]
fn a_textures_getters_report_its_texel_span_on_an_unsized_axis() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_texture_size_probe(Box::new(|p| (p == "Interface\\Crest").then_some((128, 96))));
    s.run(
        r#"
        local f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", 0, 0); f:SetWidth(100); f:SetHeight(50)
        Art = f:CreateTexture("Art", "ARTWORK")
        Art:SetTexture("Interface\\Crest"); Art:SetPoint("TOPLEFT", 0, 0)
        Half = f:CreateTexture("Half", "ARTWORK")
        Half:SetTexture("Interface\\Crest"); Half:SetPoint("TOPLEFT", 0, 0); Half:SetHeight(13)
        Bare = f:CreateTexture("Bare", "ARTWORK")
        Bare:SetPoint("TOPLEFT", 0, 0)
        assert(Art:GetWidth() == 128 and Art:GetHeight() == 96, "the art's own texels")
        assert(Half:GetWidth() == 128 and Half:GetHeight() == 13, "per AXIS: authored 13 wins, width still derived")
        assert(Bare:GetWidth() == 0, "no art, no span — and no floor on a texture")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// **The three constructors' string arguments are FOUR different shapes, not one rule.**
///
/// The discriminator is **not** the argument's type — it is whether the binding *tests* its
/// parser's return:
///
/// | position | fetch | a table there | a number there |
/// |---|---|---|---|
/// | `CreateFontString`/`CreateTexture` `name`, `layer` | `0x6f3510` → `0x6f3690`, untested | absent | **accepted, stringified** |
/// | `CreateFontString`/`CreateTexture` `inherits` | raw `lua_type == LUA_TSTRING`, no coercion | ignored | **ignored** |
/// | `CreateFrame` `name`, `inherits` | `0x6f3690` **unguarded** | absent | **accepted, stringified** |
/// | `CreateFrame` `kind` | `0x6f3510`, tested | **raises** | accepted |
///
/// The last two rows are the same value taking opposite paths from argument order alone:
/// `CreateFrame` coerces at `0x70613f` *before* its `cmp 4`, and `0x6f7cb1` retags the number's
/// stack slot **in place**, so a number arrives at the gate already a string.
#[test]
fn the_constructors_string_arguments_are_four_shapes_not_one() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"host = CreateFrame("Frame", "ArgHost", UIParent)"#)
        .unwrap();

    // ── pfUI's line, verbatim. `f.buffs[i]:CreateFontString(nil, "OVERLAY", f.buffs[i])` passes
    //    the BUTTON as `inheritsFrom`. The real client constructs it, ignores the argument, and
    //    logs nothing — this raised `bad argument #4` until 2026-08-30 and stopped pfUI, and with
    //    it pfQuest, pfQuest-turtle and ShaguDPS.
    s.run(r#"fs = ArgHost:CreateFontString(nil, "OVERLAY", ArgHost)"#)
        .unwrap_or_else(|e| panic!("a table in `inherits` must be ignored, not raise: {e}"));
    assert_eq!(
        s.eval::<String>("return type(fs)").unwrap(),
        "table",
        "...and the FontString is really constructed"
    );
    // Every other non-string in every region-constructor position: also silent, also constructed.
    for pos in ["{}, \"OVERLAY\"", "nil, {}", "true, false"] {
        for ctor in ["CreateFontString", "CreateTexture"] {
            s.run(&format!("r = ArgHost:{ctor}({pos})"))
                .unwrap_or_else(|e| panic!("{ctor}({pos}) must not raise: {e}"));
            assert_eq!(s.eval::<String>("return type(r)").unwrap(), "table");
        }
    }

    // ── A NUMBER is a string to `lua_isstring` — so it NAMES the region.
    s.run("named = ArgHost:CreateTexture(4242)").unwrap();
    assert_eq!(
        s.eval::<String>("return named:GetName()").unwrap(),
        "4242",
        "a number in `name` is stringified, not dropped"
    );
    // ...but NOT in `inherits`, whose gate reads the raw tag and never coerces. A number there is
    // ignored, where the same number in CreateFrame's fourth argument would raise.
    s.run("ArgHost:CreateTexture(nil, nil, 5)")
        .unwrap_or_else(|e| panic!("a number in a region ctor's `inherits` is ignored: {e}"));

    // ── CreateFrame: `name` coerces the same way.
    s.run("nf = CreateFrame(\"Frame\", 77)").unwrap();
    assert_eq!(
        s.eval::<String>("return nf:GetName()").unwrap(),
        "77",
        "CreateFrame reads `name` through an UNGUARDED lua_tostring"
    );
    // ...and an unresolvable `inherits` still raises, which is the pre-existing contract and the
    // control for the changes above: loosening the OTHER positions must not have loosened this.
    assert!(
        s.run(r#"CreateFrame("Frame", nil, nil, "NoSuchTemplateAnywhere")"#)
            .is_err(),
        "an unresolvable template name still raises — the miss branch is luaL_error"
    );

    // ── The layer argument is PERMISSIVE, and an unrecognised one is ARTWORK, not an error.
    //    `0x6f18b0` returns 0 with its out-param unwritten and neither constructor reads the
    //    result, so the pre-staged ARTWORK (2) stands. Matching is case-insensitive.
    s.run(r#"lay = ArgHost:CreateTexture(nil, "not-a-layer")"#)
        .unwrap_or_else(|e| panic!("an unrecognised layer must not raise: {e}"));
    // Read off the arena — 1.12 publishes no `GetDrawLayer`, so there is no Lua getter to ask,
    // the same reason the sequence scrub is read this way in `modelframe`'s tests.
    let layer_of = |s: &UiScript, name: &str| {
        let lua = s.lua();
        let model = lua.app_data_ref::<crate::script::Model>().expect("model");
        let id = *model.region_names.get(name).expect("published region");
        let rh = *model.id_to_region.get(&id).expect("live region");
        model.arena.region(rh).expect("region").draw_layer
    };
    s.run(r#"ArgHost:CreateTexture("LayLower", "overlay")"#)
        .unwrap();
    s.run(r#"ArgHost:CreateTexture("LayUpper", "OVERLAY")"#)
        .unwrap();
    s.run(r#"ArgHost:CreateTexture("LayJunk", "not-a-layer")"#)
        .unwrap();
    assert_eq!(
        layer_of(&s, "LayLower"),
        layer_of(&s, "LayUpper"),
        "layer matching is case-insensitive (SStrCmpI 0x64a4c0)"
    );
    assert_eq!(
        layer_of(&s, "LayJunk"),
        crate::order::DrawLayer::Artwork,
        "an unrecognised layer leaves the PRE-STAGED default standing — ARTWORK (2), not an error \
         and not BACKGROUND: `0x6f18b0` returns 0 with its out-param unwritten and neither \
         constructor reads the result"
    );
}

/// The token is CANONICAL on the way out — `"NPC"`, which the stock merchant, guild registrar
/// and trade windows all write (`MerchantFrame.lua:68`, `GuildRegistrarFrame.lua:4`,
/// `TradeFrame.lua:41`), binds as `"npc"`. The reference resolves every unit token
/// case-insensitively (`0x515970`'s ten compares are all `_strnicmp`), and the app samples the
/// booth by this exact string — a raw `"NPC"` matched no slot and left the ring empty on all
/// three windows.
#[test]
fn set_portrait_texture_folds_the_token_to_lowercase() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "NFrame")
        f:SetPoint("TOPLEFT", 0, 0)
        f:SetWidth(100); f:SetHeight(100)
        local p = f:CreateTexture("NFramePortrait", "BACKGROUND")
        p:SetWidth(64); p:SetHeight(64)
        p:SetPoint("TOPLEFT", 0, 0)
        SetPortraitTexture(p, "NPC")
    "#,
    )
    .unwrap();
    s.resolve();
    let bound = s.extract().into_iter().find_map(|q| match q.content {
        QuadContent::Texture {
            portrait_unit: Some(u),
            ..
        } => Some(u),
        _ => None,
    });
    assert_eq!(bound.as_deref(), Some("npc"));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **`FontString:GetText` substitutes nil for an EMPTY string, and that substitution is the
/// getter's own** — `0x79d690` reads the cell, and eleven bytes before the pushstring it does a
/// FIRST-BYTE test and zeroes the pointer on `'\0'`:
///
/// ```text
/// 79d735 test eax,eax / je 0x79d740     ; NULL      -> substitute
/// 79d73b cmp byte ptr [eax],0x0         ; the first-byte test
/// 79d73e jne 0x79d742                   ; non-empty -> keep
/// 79d740 xor eax,eax                    ; EMPTY     -> NULL
/// 79d746 call 0x6f3890                  ; NULL -> pushnil
/// ```
///
/// So it cannot return `""`, whatever the cell holds — and the cell really does hold non-NULL
/// empty strings, because `SetText 0x771d80` never writes NULL to `+0xf0` on any leg: NULL and
/// `""` share one leg that truncates the buffer in place. Per binding, not per family:
/// `Button:GetText 0x780e10` carries the same substitution (`0x780ec5`), `EditBox:GetText
/// 0x7985c0` carries none (decision 2110).
///
/// The director's shape (Cartographer 2.02, the world map's hover label): the stock
/// `WorldMapFrameAreaDescription` is blanked with `SetText("")` by `WorldMapPOI_OnEnter`/`_OnLeave`
/// on **every** POI hover, and Cartographer's ZoneInfo reads
/// `if WorldMapFrameAreaDescription:GetText() then` as "this POI has a status line, so put the
/// zone's level range there instead of on the label". Answering `""` made that true forever after
/// the first POI touch — the name lost its faction colour, the range moved to the description's own
/// line beneath it, and nothing ever cleared it again.
#[test]
fn an_empty_fontstring_reads_back_nil_and_an_edit_box_does_not() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "TextCell")
        fresh = f:CreateFontString(nil, "OVERLAY")
        held  = f:CreateFontString(nil, "OVERLAY")
    "#,
    )
    .unwrap();
    let text_of = |s: &UiScript, which: &str| {
        s.eval::<Option<String>>(&format!("return {which}:GetText()"))
            .unwrap()
    };

    // Never written, and every shape of an empty write, all nil.
    assert_eq!(text_of(&s, "fresh"), None);
    for write in [r#"fresh:SetText("")"#, r#"fresh:SetText(nil)"#] {
        s.run(write).unwrap();
        assert_eq!(text_of(&s, "fresh"), None, "after `{write}`");
    }

    // A string that HAS held text still reads nil once blanked — the substitution is the getter's,
    // so it does not matter that the cell keeps a (truncated) buffer.
    s.run(r#"held:SetText("In Conflict")"#).unwrap();
    assert_eq!(text_of(&s, "held").as_deref(), Some("In Conflict"));
    s.run(r#"held:SetText("")"#).unwrap();
    assert_eq!(
        text_of(&s, "held"),
        None,
        "a blanked FontString reads back nil, not an empty string"
    );
    s.run(r#"held:SetText("back"); held:SetText(nil)"#).unwrap();
    assert_eq!(text_of(&s, "held"), None);

    // Button:GetText carries the SAME substitution; Button:SetText(nil) is its own no-op guard.
    s.run(
        r#"
        local b = CreateFrame("Button", "TextCellButton")
        b:SetText("Accept")
        b:SetText(nil)
    "#,
    )
    .unwrap();
    assert_eq!(
        text_of(&s, "TextCellButton").as_deref(),
        Some("Accept"),
        "a nil never reaches the button's label (`0x778dcc`)"
    );
    s.run(r#"TextCellButton:SetText("")"#).unwrap();
    assert_eq!(text_of(&s, "TextCellButton"), None, "an empty label is nil");

    // And the EditBox is the counter-example that keeps this from being hoisted: stock
    // `MailFrame.lua` compares `GetText() == ""` and calls `strlen(GetText())` on one.
    s.run(
        r#"
        local e = CreateFrame("EditBox", "TextCellEdit")
        e:SetText("")
    "#,
    )
    .unwrap();
    assert_eq!(
        text_of(&s, "TextCellEdit").as_deref(),
        Some(""),
        "EditBox:GetText 0x7985c0 reads [edit+0x32c] straight through — no substitution"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **A `SetTexture` the host cannot resolve leaves the region's art alone** — it does not overwrite
/// it with the path that failed.
///
/// The reference's load-failure arm is explicit about it: `0x770288 cmp [ebp-4],2; jl` →
/// `0x77028e`–`0x7702b2` releases the handle it just built and returns 0 **without ever touching
/// `+0xcc`**. Ours stored the path first and used the
/// probe's verdict only for the return value, so a mistyped or not-yet-shipped path erased the art
/// it failed to replace — `GetTexture()` echoed the miss, and the extract dropped the quad, so the
/// region went blank with nothing said anywhere.
///
/// The control that must not change is the second half: a resolvable path still replaces, and a VM
/// with **no probe at all** still stores, because it has no backend to ask.
#[test]
fn an_unresolvable_set_texture_keeps_the_art_the_region_had() {
    let mut s = script();
    s.set_texture_probe(Box::new(|path: &str| !path.contains("Nope")));
    s.run(
        r#"
        f = CreateFrame("Frame", "ProbeHost")
        t = f:CreateTexture("ProbeTex")
        t:SetTexture("Interface\\Real")
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return t:GetTexture()").unwrap(),
        "Interface\\Real"
    );

    // The miss: nil back, and the art it could not replace is still there.
    assert!(
        s.eval::<bool>(r#"return t:SetTexture("Interface\\Nope") == nil"#)
            .unwrap(),
        "an unresolvable path must answer nil"
    );
    assert_eq!(
        s.eval::<String>("return t:GetTexture()").unwrap(),
        "Interface\\Real",
        "the failed load overwrote the art the region was holding"
    );

    // The control: a hit still replaces, and an explicit clear still clears.
    assert!(s
        .eval::<bool>(r#"return t:SetTexture("Interface\\Other") == 1"#)
        .unwrap());
    assert_eq!(
        s.eval::<String>("return t:GetTexture()").unwrap(),
        "Interface\\Other"
    );
    s.run("t:SetTexture(nil)").unwrap();
    assert!(s.eval::<bool>("return t:GetTexture() == nil").unwrap());
}

/// `Texture:GetBlendMode()` — ONE string, the reference's own spelling, round-tripping whatever
/// `SetBlendMode` was given (`0x79a890`/`0x79a950`, table `0x87c128`, argc 1, arity 1, kinds
/// `(string?)`).
///
/// The untouched default is asserted alongside the round trip because it is the load-bearing half:
/// `ShaguTweaks/mods/dark-ui-elements.lua:169` guards a recolour with
/// `region.GetBlendMode and region:GetBlendMode() == "ADD"`, so the answer an ordinary texture gives
/// decides whether the reference's additive art is left alone. `"BLEND"` is the CSimpleTexture
/// ctor's `[+0xd0] = 2` (`0x76fc64`).
#[test]
fn get_blend_mode_answers_one_string_and_defaults_to_the_ctors_blend() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        BlendOwner = CreateFrame("Frame", "BlendOwner")
        BTex = BlendOwner:CreateTexture("BTex", "ARTWORK")
        "#,
    )
    .unwrap();

    // Arity 1 and kind string, measured the way the shape gate measures it.
    assert_eq!(s.arity("BTex:GetBlendMode()").unwrap(), 1, "arity 1");
    assert_eq!(
        s.eval::<String>("return type(BTex:GetBlendMode())")
            .unwrap(),
        "string",
        "kind string"
    );
    assert_eq!(
        s.eval::<String>("return BTex:GetBlendMode()").unwrap(),
        "BLEND",
        "an untouched texture is the ctor's mode 2"
    );

    // Every one of the enum's five round-trips, including the three the renderer flattens.
    for mode in ["DISABLE", "ALPHAKEY", "BLEND", "ADD", "MOD"] {
        s.run(&format!(r#"BTex:SetBlendMode("{mode}")"#)).unwrap();
        assert_eq!(
            s.eval::<String>("return BTex:GetBlendMode()").unwrap(),
            mode,
            "round trip through the setter"
        );
    }
    // Case-insensitive in, canonical out — the setter coerces, the getter answers the table's name.
    s.run(r#"BTex:SetBlendMode("add")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return BTex:GetBlendMode()").unwrap(),
        "ADD"
    );
    // A name outside the enum leaves the mode alone rather than inventing one.
    s.run(r#"BTex:SetBlendMode("NOT_A_MODE")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return BTex:GetBlendMode()").unwrap(),
        "ADD",
        "an unknown name changes nothing"
    );

    // A FontString is not a Texture: neither half of the pair is on its map.
    assert!(s
        .eval::<bool>(
            r#"local fs = BlendOwner:CreateFontString("BStr", "ARTWORK")
               return fs.GetBlendMode == nil and fs.SetBlendMode == nil"#
        )
        .unwrap());
}

/// `Texture:GetTexCoordModifiesRect()` — the 1.12 predicate return, `1`/`nil` and never a Lua
/// boolean (`0x79c120`, table `0x87c128`, argc 1, arity 1, kinds `(nil) | (number)`).
/// `pfUI/modules/thirdparty-tbc.lua:319` calls it bare on a Texture.
///
/// The flag's effect on the region's rect is deliberately NOT wired (see
/// `RegionData::tex_coord_modifies_rect`), so this asserts the state and the shape — and asserts
/// the un-wiring too, by checking that setting the flag moves no rect: a future change that wires
/// the geometry will fail here and have to say so.
#[test]
fn tex_coord_modifies_rect_is_one_slash_nil_and_moves_no_rect_yet() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        TCMOwner = CreateFrame("Frame", "TCMOwner")
        TCMOwner:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        TCMOwner:SetWidth(200); TCMOwner:SetHeight(200)
        TCMTex = TCMOwner:CreateTexture("TCMTex", "ARTWORK")
        TCMTex:SetTexture("Interface\\TcmArt")
        TCMTex:SetPoint("BOTTOMLEFT", TCMOwner, "BOTTOMLEFT", 0, 0)
        TCMTex:SetWidth(64); TCMTex:SetHeight(32)
        "#,
    )
    .unwrap();
    s.resolve();
    let before = region_tex_rect(&s, "TcmArt");

    assert_eq!(
        s.arity("TCMTex:GetTexCoordModifiesRect()").unwrap(),
        1,
        "arity 1"
    );
    assert!(
        s.eval::<bool>("return TCMTex:GetTexCoordModifiesRect() == nil")
            .unwrap(),
        "unset is nil, not false"
    );

    s.run("TCMTex:SetTexCoordModifiesRect(1)").unwrap();
    assert!(
        s.eval::<bool>("return TCMTex:GetTexCoordModifiesRect() == 1")
            .unwrap(),
        "set is the NUMBER 1, not true"
    );
    assert_eq!(
        s.eval::<String>("return type(TCMTex:GetTexCoordModifiesRect())")
            .unwrap(),
        "number",
        "kind number, never boolean"
    );

    // The flag is state only: a SetTexCoord under it still leaves the rect where the anchors and
    // the size put it. When the geometry leg is built, this assertion is the one that must change.
    s.run("TCMTex:SetTexCoord(0, 0.25, 0, 0.5)").unwrap();
    s.resolve();
    assert_eq!(
        region_tex_rect(&s, "TcmArt"),
        before,
        "the flag does not (yet) let SetTexCoord re-derive the region's rect"
    );

    s.run("TCMTex:SetTexCoordModifiesRect(nil)").unwrap();
    assert!(
        s.eval::<bool>("return TCMTex:GetTexCoordModifiesRect() == nil")
            .unwrap(),
        "cleared back to nil"
    );
}
