//! Regions in the anchor layout: they resolve through the frames' leaf math, off their own anchors
//! and content-derived span; the owner supplies scale, never a fallback edge.

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

/// A FontString's extent is its measured text floored at one unit (`GetWidth 0x772930`,
/// `GetHeight 0x772a60`), so an unmeasured one is a 1x1 box on its anchor.
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
    // Unmeasured: x runs 5..6 from the pinned left; LEFT pins the y centre (25), so y is ±0.5.
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

/// `ExhaustionLevelFillBar`'s shape, width 0 and one TOPLEFT anchor: its `<Color>` installs an 8x8
/// texture (`0x7700a9` → `0x770360` → `0x44a900`), so `GetWidth 0x770720` answers 8; with no art
/// it answers 0.0, and `assemble 0x767a20` leaves the rect unresolved.
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
    // `CreateTexture 0x773a20` anchors a new texture only on a template hit, so a templateless one
    // has no anchor, no rect and no draw, whatever its size.
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

/// A region anchored by name to a sibling declared after it: `MerchantItemTemplate`'s label plate
/// (`LEFT` to `$parentSlotTexture`'s `RIGHT`, -9, -18), ordered by the resolve alone.
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
    // The row's TOPLEFT (100, 144) plus (-13, 13): the slot spans 87..151 by 93..157.
    let slot = rect("RowSlot");
    assert_eq!(
        (slot.left, slot.top),
        (87.0, 157.0),
        "slot at row TOPLEFT (-13,13)"
    );
    assert_eq!(slot.right, 151.0);
    // The plate's LEFT is the slot's RIGHT (151) - 9 = 142, not the row's right edge (253).
    let plate = rect("RowPlate");
    assert_eq!(plate.left, 142.0, "plate.LEFT = slot.RIGHT − 9");
    assert_eq!(
        plate.right, 270.0,
        "plate spans its 128 width from the slot edge"
    );
    // A LEFT point centres the plate vertically: the slot's centre 125, -18 = 107, ±39.
    assert_eq!((plate.bottom, plate.top), (68.0, 146.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetPortraitTexture` binds a Texture to a unit token, carried on the quad as `portrait_unit`
/// with no path or colour (the app supplies the bake); a later `SetTexture` drops the binding.
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

/// `BenillaSetBoothTexture`, the paper doll's square booth binding: `SetPortraitTexture`'s
/// carriage with `circular` false, as the model pane has no ring to mask for.
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

/// The quest log detail pane's shape: a region anchor chain about 15 deep (title, objectives,
/// description, rewards) with a frame anchored to its tail, which the resolve runs to a fixpoint.
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

/// A child frame created and shown at runtime draws its own regions where it resolves.
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

/// A region draws at its own alpha times its owner frame's, one hop (`SetAlpha 0x76a690` cascades
/// onto child frames, not regions), and `GetAlpha` answers its own: stock `CastingBarFrame.lua:133`
/// ramps `CastingBarFlash` by reading it back.
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

    s.run("tex:SetAlpha(0.5)").unwrap();
    assert_eq!(s.eval::<f32>("return tex:GetAlpha()").unwrap(), 0.5);
    assert_eq!(quad_alpha(&s), 0.5);

    s.run("f:SetAlpha(0.5)").unwrap();
    assert_eq!(
        s.eval::<f32>("return tex:GetAlpha()").unwrap(),
        0.5,
        "the frame's SetAlpha leaves the region's own alpha alone"
    );
    assert_eq!(quad_alpha(&s), 0.25, "0.5 region × 0.5 frame");

    s.run("tex:SetAlpha(1); tex:Hide()").unwrap();
    assert!(
        !s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Texture { path: Some(_), .. })),
        "a hidden region emits no quad"
    );
}

/// A solid colour is a texel (`0x770360`, an 8x8 block at `+0xcc`) and the vertex colour a
/// separate slot (`SetVertexColor 0x77f750`, `+0xb8`); the draw multiplies them, alpha included,
/// as stock `SkillFrame`'s row trough needs (`SkillFrame.xml:35`, `SkillFrame.lua:158`).
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
    // `GetVertexColor` reads the vertex slot, not the product.
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return trough:GetVertexColor()")
            .unwrap(),
        (0.0, 0.0, 0.75, 0.5)
    );

    // Art and a solid colour share `+0xcc`: a path releases the texel and leaves the tint.
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

/// The desaturate flag rides an art quad only: a pathless solid draws its colour as the quad's tint
/// over a white texel, which greying leaves white, so extract drops the flag there.
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

    s.run("art:SetDesaturated(nil)").unwrap();
    assert!(!grey(&s, true), "clearing the flag restores full colour");
}

/// A Texture region's desaturation state, read off the model by name: `IsDesaturated`
/// (`0x79c2c0`) is not built, and a cleared texture emits no quad to read it from.
fn desaturated(s: &UiScript, name: &str) -> bool {
    let lua = s.lua();
    let model = lua.app_data_ref::<crate::script::Model>().expect("model");
    let id = *model.region_names.get(name).expect("region name");
    let h = *model.id_to_region.get(&id).expect("region handle");
    model.region_data.get(&h).is_some_and(|d| d.desaturated)
}

/// The desaturation is the shader slot `+0x128`, which `CSimpleTexture::SetTexture` rewrites with
/// the binding's always-NULL slot 0, except that the same path returns early (`0x770225`).
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

    s.run(r#"icon:SetTexture("Interface\\Icons\\Spell_Nature_Sleep")"#)
        .unwrap();
    assert!(grey(&s), "re-setting the SAME art keeps the grey");

    s.run(r#"icon:SetTexture("Interface\\Icons\\Spell_Fire_Fireball")"#)
        .unwrap();
    assert!(!grey(&s), "a texture CHANGE clears the desaturation");

    // nil takes the same leg (`test esi,esi` falls through to the write).
    s.run("icon:SetDesaturated(1) icon:SetTexture(nil)")
        .unwrap();
    assert!(!grey(&s), "SetTexture(nil) clears it too");

    // The colour form (`0x770360`) does not write the shader slot.
    s.run(r#"icon:SetTexture("Interface\\Icons\\Spell_Nature_Sleep") icon:SetDesaturated(1)"#)
        .unwrap();
    s.run("icon:SetTexture(1, 0, 0)").unwrap();
    assert!(grey(&s), "the colour form does not touch the shader slot");
}

/// `SetDesaturated` reads its flag with `0x6f1c10(L, 2, default=1)`: no argument (`LUA_TNONE`)
/// greys, and a number is truncated to an int, so 0 and 0.5 clear.
#[test]
fn set_desaturated_takes_the_clients_argument_truth_table() {
    let s = script();
    s.run(r#"f = CreateFrame("Frame", "ArgOwner") tex = f:CreateTexture("ArgTex", "ARTWORK")"#)
        .unwrap();
    let grey = |s: &UiScript| desaturated(s, "ArgTex");

    s.run("ArgTex:SetDesaturated()").unwrap();
    assert!(
        grey(&s),
        "a bare SetDesaturated() greys (LUA_TNONE default)"
    );

    s.run("ArgTex:SetDesaturated(nil)").unwrap();
    assert!(!grey(&s), "nil clears");

    s.run("ArgTex:SetDesaturated(1) ArgTex:SetDesaturated(0)")
        .unwrap();
    assert!(!grey(&s), "0 clears — the number arm truncates to int");
    s.run("ArgTex:SetDesaturated(0.5)").unwrap();
    assert!(!grey(&s), "0.5 truncates to 0 and clears");

    s.run("ArgTex:SetDesaturated(true)").unwrap();
    assert!(grey(&s), "true greys");
    s.run("ArgTex:SetDesaturated(false)").unwrap();
    assert!(!grey(&s), "false clears");
}

/// The draw gate is the texture slot, never the colour: `0x7706e0` emits nothing when `+0xcc` is
/// empty, though the tint survives `SetTexture(nil)`.
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

/// `SetTexture`'s path form reads one argument (`0x770200`) and its colour form up to four
/// (`0x770360`), ignoring the rest; a non-numeric channel reads as `lua_tonumber` reads it, 0.
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

    s.run("t:SetTexture(0.25, '0.5', true)")
        .expect("the colour form must tolerate what lua_tonumber tolerates");

    s.run("t:SetTexture(nil) t:SetTexture('') t:SetTexture(1, 0, 0, 1)")
        .expect("clear, blank and the plain colour form are unaffected");
}

/// A gradient fills the texture slot the colour form fills, so a gradient-only region paints;
/// both stops are stored, and the quad paints their midpoint, not the gradient.
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

    // Any orientation token but "VERTICAL" is horizontal.
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

/// Texture's map (`0x87c128`, 22 entries) and FontString's (`0xcf5400`, 32) each answer their own
/// verbs, then fall back to the Region map and stop there.
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

    // Texture-only; `GetVertexColor` is here though `SetVertexColor` is on both leaves.
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
    // `SetRotation` is 1.12's PlayerModel verb (`0x84f1fc`/`0x505f00`), in neither leaf map.
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
    // On both leaves, each registering its own copy, so not on the Region map.
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
    for m in crate::script::REGION_MAP_METHODS {
        assert!(
            has(&s, "Tex", m) && has(&s, "Str", m),
            "{m} is the Region map"
        );
    }
    // 1.12 has no `GetStringHeight` on any table; `GetHeight` reads the same measurement.
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

/// `SetPortraitToTexture` is a 1.12 global, not a Texture method; stock `ContainerFrame.lua:419`
/// and `MailFrame.lua:174` pass it a texture name.
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
    assert_eq!(
        s.eval::<String>("return type(Port.SetPortraitToTexture)")
            .unwrap(),
        "nil",
        "1.12 has no Texture:SetPortraitToTexture — it is a global"
    );
    // An unknown name does not raise here; the reference raises "Couldn't find texture named"
    // (`0x48d7db`).
    s.run(r#"SetPortraitToTexture("NoSuchPortrait", "Interface\\X")"#)
        .unwrap();
    assert!(s.errors().is_empty());
}

/// `Region:GetWidth`/`GetHeight` (`0x7a1e00`/`0x7a2030`) call the virtual size getters the rect
/// resolver uses: a FontString's authored axis wins (`0x772930` skips the measure, `jp 0x77294a`),
/// else its width is the unwrapped extent `GetStringWidth` reads (`0x772890`, `[fs+0xfc]`) and its
/// height the wrapped one (`0x7729b0`, `[fs+0x100]`).
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

/// `0x772930`/`0x772a60` clamp to one unit, so an empty string reads 1. A host measure still in
/// flight, a state the reference does not have, reads 0, so an `if h <= 0` guard keeps waiting.
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

/// A Texture's size getters (`0x770720`/`0x770790`): the authored value unless exactly `0.0`, else
/// the art's texel extent, else `0.0`, with no floor.
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

/// The constructors' string arguments take four shapes, by whether the binding tests its parser's
/// return: a region constructor's `name` and `layer` (`0x6f3510` → `0x6f3690`, untested) and
/// `CreateFrame`'s `name` and `inherits` (`0x6f3690`, unguarded) read a table as absent and
/// stringify a number; a region constructor's `inherits` (a raw `lua_type == LUA_TSTRING`) ignores
/// both; `CreateFrame`'s `kind` (`0x6f3510`, tested) raises on a table and takes a number, which
/// `CreateFrame` coerces in place before its gate (`0x70613f`, `0x6f7cb1`).
#[test]
fn the_constructors_string_arguments_are_four_shapes_not_one() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"host = CreateFrame("Frame", "ArgHost", UIParent)"#)
        .unwrap();

    // ── A table in `inherits` is ignored and the string still built; pfUI passes a button there.
    s.run(r#"fs = ArgHost:CreateFontString(nil, "OVERLAY", ArgHost)"#)
        .unwrap_or_else(|e| panic!("a table in `inherits` must be ignored, not raise: {e}"));
    assert_eq!(
        s.eval::<String>("return type(fs)").unwrap(),
        "table",
        "...and the FontString is really constructed"
    );
    for pos in ["{}, \"OVERLAY\"", "nil, {}", "true, false"] {
        for ctor in ["CreateFontString", "CreateTexture"] {
            s.run(&format!("r = ArgHost:{ctor}({pos})"))
                .unwrap_or_else(|e| panic!("{ctor}({pos}) must not raise: {e}"));
            assert_eq!(s.eval::<String>("return type(r)").unwrap(), "table");
        }
    }

    // ── A number passes `lua_isstring`, so it names the region.
    s.run("named = ArgHost:CreateTexture(4242)").unwrap();
    assert_eq!(
        s.eval::<String>("return named:GetName()").unwrap(),
        "4242",
        "a number in `name` is stringified, not dropped"
    );
    // `inherits` reads the raw tag, so a number there is ignored.
    s.run("ArgHost:CreateTexture(nil, nil, 5)")
        .unwrap_or_else(|e| panic!("a number in a region ctor's `inherits` is ignored: {e}"));

    // ── CreateFrame: `name` coerces the same way.
    s.run("nf = CreateFrame(\"Frame\", 77)").unwrap();
    assert_eq!(
        s.eval::<String>("return nf:GetName()").unwrap(),
        "77",
        "CreateFrame reads `name` through an UNGUARDED lua_tostring"
    );
    assert!(
        s.run(r#"CreateFrame("Frame", nil, nil, "NoSuchTemplateAnywhere")"#)
            .is_err(),
        "an unresolvable template name still raises — the miss branch is luaL_error"
    );

    // ── The layer: case-insensitive, and an unrecognised one leaves the pre-staged ARTWORK.
    s.run(r#"lay = ArgHost:CreateTexture(nil, "not-a-layer")"#)
        .unwrap_or_else(|e| panic!("an unrecognised layer must not raise: {e}"));
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

/// `SetPortraitTexture` lowercases the token, which the app matches exactly: the reference reads
/// unit tokens case-insensitively (`0x515970`, `_strnicmp`), and stock `MerchantFrame.lua:68`
/// passes `"NPC"`.
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

/// `FontString:GetText` (`0x79d690`) pushes nil for an empty string by a first-byte test
/// (`0x79d73b`), though `SetText 0x771d80` keeps an empty buffer; `Button:GetText 0x780e10` does
/// the same (`0x780ec5`), and `EditBox:GetText 0x7985c0` returns `""`.
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

    assert_eq!(text_of(&s, "fresh"), None);
    for write in [r#"fresh:SetText("")"#, r#"fresh:SetText(nil)"#] {
        s.run(write).unwrap();
        assert_eq!(text_of(&s, "fresh"), None, "after `{write}`");
    }

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

    // `Button:SetText(nil)` leaves the label alone.
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

    // Stock `MailFrame.lua:521` compares an EditBox's `GetText() == ""`.
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

/// A path that fails to load leaves the art alone (`0x770288` → `0x77028e`-`0x7702b2` returns 0
/// without touching `+0xcc`); with no probe installed, every path is stored.
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

/// `Texture:GetBlendMode` (`0x79a890`, table `0x87c128`) answers one string, the mode
/// `SetBlendMode` (`0x79a950`) set, `"BLEND"` by default (the ctor's `[+0xd0] = 2`, `0x76fc64`).
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

    for mode in ["DISABLE", "ALPHAKEY", "BLEND", "ADD", "MOD"] {
        s.run(&format!(r#"BTex:SetBlendMode("{mode}")"#)).unwrap();
        assert_eq!(
            s.eval::<String>("return BTex:GetBlendMode()").unwrap(),
            mode,
            "round trip through the setter"
        );
    }
    // Case-insensitive in, canonical out.
    s.run(r#"BTex:SetBlendMode("add")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return BTex:GetBlendMode()").unwrap(),
        "ADD"
    );
    s.run(r#"BTex:SetBlendMode("NOT_A_MODE")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return BTex:GetBlendMode()").unwrap(),
        "ADD",
        "an unknown name changes nothing"
    );

    assert!(s
        .eval::<bool>(
            r#"local fs = BlendOwner:CreateFontString("BStr", "ARTWORK")
               return fs.GetBlendMode == nil and fs.SetBlendMode == nil"#
        )
        .unwrap());
}

/// `Texture:GetTexCoordModifiesRect` (`0x79c120`, table `0x87c128`) answers `1` or nil. The flag is
/// stored, not applied: on the reference it makes `SetTexCoord` re-derive the rect (`0x770462`).
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

    // The assertion that changes when the rect re-derivation is built.
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
