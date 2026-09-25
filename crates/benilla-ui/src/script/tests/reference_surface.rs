//! The 1.12 surface as a gate: every global benilla's VM exposes is one the 1.12.1 client has
//! (`reference/1.12-globals.tsv`) or a listed exception, since addons branch on a global's
//! presence; what 1.12 has and benilla lacks is `scripts/api-coverage.sh`'s count.

use std::collections::HashSet;

use super::common::script;

/// `reference/1.12-globals.tsv`, as `name -> origin`.
fn reference() -> Vec<(String, String)> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-globals.tsv"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("reading {path}: {e} — regenerate with scripts/gen-reference-globals.py")
    });
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some((f.next()?.to_string(), f.nth(1)?.to_string()))
        })
        .collect()
}

/// Globals benilla exposes that 1.12 does not. `Benilla*` (the host bridge our own UI files call)
/// and `__benilla_*` (the tick's pushed state) pass by prefix.
fn allowed_beyond_1_12() -> HashSet<&'static str> {
    [
        // ── our Lua runtime is 5.1 where 1.12's is 5.0 ──
        // 1.12's base library does not export `_G` (an addon reaches the globals with
        // `getfenv(0)`); ours does, as our `getglobal`/`setglobal` are written over it.
        "_G",
        // ── WoW API past 1.12 ──
        // Only `SubmitChatInput` is called from our UI files (`ScriptLogFrame.xml`'s slash
        // commands); each of the rest awaits its 1.12 equivalent, removal or a reason.
        "CancelUnitBuff",
        "GetCursorInfo",
        "GetInventoryItemID",
        "GetPlayerFacing",
        "GetTradePartnerName",
        "IsGossipOptionCoded",
        "SubmitChatInput",
        "UnitAura",
        "UnitIsAFK",
        "UnitIsDND",
    ]
    .into_iter()
    .collect()
}

/// Every global benilla's VM exposes is one 1.12 has, or a listed exception.
#[test]
fn our_globals_stay_inside_the_1_12_surface() {
    let reference = reference();
    let known: HashSet<&str> = reference.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        known.len() > 19_000,
        "the reference table looks truncated ({} names) — regenerate it",
        known.len()
    );
    let allowed = allowed_beyond_1_12();

    let ours: Vec<String> = script()
        .eval(
            "local out = {} \
             for k in pairs(_G) do if type(k) == 'string' then table.insert(out, k) end end \
             return out",
        )
        .expect("dump _G");

    let mut unlisted: Vec<&str> = ours
        .iter()
        .map(String::as_str)
        .filter(|n| !known.contains(n))
        .filter(|n| !allowed.contains(n))
        .filter(|n| !n.starts_with("Benilla") && !n.starts_with("__benilla_"))
        .collect();
    unlisted.sort_unstable();

    assert!(
        unlisted.is_empty(),
        "benilla exposes {} global(s) the 1.12.1 client does not, and they are not listed as \
         exceptions:\n    {}\n\n\
         1.12 is the target. Either give it its 1.12 spelling, or add it to \
         `allowed_beyond_1_12` in this file WITH the reason it has to stay — an addon that \
         feature-detects an unexplained superset takes a path we cannot honour.",
        unlisted.len(),
        unlisted.join(" ")
    );
}

/// The exception list is exact: no entry outlives its global, and none excuses a 1.12 name.
#[test]
fn the_exception_list_has_no_dead_entries() {
    let known: HashSet<String> = reference().into_iter().map(|(n, _)| n).collect();
    let ours: HashSet<String> = script()
        .eval::<Vec<String>>(
            "local out = {} \
             for k in pairs(_G) do if type(k) == 'string' then table.insert(out, k) end end \
             return out",
        )
        .expect("dump _G")
        .into_iter()
        .collect();

    let mut dead: Vec<&str> = allowed_beyond_1_12()
        .into_iter()
        .filter(|n| !ours.contains(*n))
        .collect();
    dead.sort_unstable();
    assert!(
        dead.is_empty(),
        "these are excused as beyond-1.12 but benilla no longer exposes them — drop them from \
         `allowed_beyond_1_12`:\n    {}",
        dead.join(" ")
    );

    let mut wrong: Vec<&str> = allowed_beyond_1_12()
        .into_iter()
        .filter(|n| known.contains(*n))
        .collect();
    wrong.sort_unstable();
    assert!(
        wrong.is_empty(),
        "these are excused as beyond-1.12 but the 1.12.1 client DOES have them — they need no \
         excuse:\n    {}",
        wrong.join(" ")
    );
}

/// `Texture:GetTexture()` (`0x79ba70`/`0x79baf0`/`0x835708`): one value, nil when unset, the path
/// without its extension, and the literal `"Solid Texture"` for a colour fill.
#[test]
fn get_texture_returns_the_stripped_path_and_solid_texture_for_a_fill() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        f = CreateFrame("Frame", "GTF")
        tex = f:CreateTexture("GTFTex", "ARTWORK")
        fill = f:CreateTexture("GTFFill", "ARTWORK")
        "#,
    )
    .unwrap();

    assert_eq!(
        s.eval::<Option<String>>("return GTFTex:GetTexture()")
            .unwrap(),
        None
    );
    assert_eq!(
        s.arity("GTFTex:GetTexture()").unwrap(),
        1,
        "exactly one return value"
    );

    s.run(r#"GTFTex:SetTexture("Interface\\Icons\\Spell_Fire_FlameBolt")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GTFTex:GetTexture()").unwrap(),
        "Interface\\Icons\\Spell_Fire_FlameBolt"
    );

    s.run(r#"GTFTex:SetTexture("Interface\\Icons\\Foo.blp")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GTFTex:GetTexture()").unwrap(),
        "Interface\\Icons\\Foo"
    );

    s.run("GTFFill:SetTexture(1, 0, 0, 1)").unwrap();
    assert_eq!(
        s.eval::<String>("return GTFFill:GetTexture()").unwrap(),
        "Solid Texture",
        "a colour-filled region reports a NAME — `if not tex then` must not fire"
    );

    s.run(r#"GTFFill:SetTexture("Interface\\Buttons\\UI-Quickslot2")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GTFFill:GetTexture()").unwrap(),
        "Interface\\Buttons\\UI-Quickslot2"
    );
}

/// A FontString's shadow accessors; `GetShadowColor` returns four values, alpha included
/// (`0x79dd2f`, `mov eax,0x4`).
#[test]
fn region_shadow_accessors_round_trip_four_values() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(r#"f = CreateFrame("Frame", "SHF") fs = f:CreateFontString("SHFText", "ARTWORK")"#)
        .unwrap();

    assert_eq!(
        s.arity("SHFText:GetShadowColor()").unwrap(),
        4,
        "GetShadowColor returns FOUR values — three drops the alpha"
    );
    assert_eq!(s.arity("SHFText:GetShadowOffset()").unwrap(), 2);

    s.run("SHFText:SetShadowColor(0, 0, 0, 0.3) SHFText:SetShadowOffset(1, -1)")
        .unwrap();
    let (r, g, b, a) = s
        .eval::<(f32, f32, f32, f32)>("return SHFText:GetShadowColor()")
        .unwrap();
    assert_eq!((r, g, b, a), (0.0, 0.0, 0.0, 0.3), "the alpha survives");
    assert_eq!(
        s.eval::<(f32, f32)>("return SHFText:GetShadowOffset()")
            .unwrap(),
        (1.0, -1.0)
    );

    s.run("SHFText:SetShadowOffset(2, -2)").unwrap();
    assert_eq!(
        s.eval::<f32>("local _,_,_,a = SHFText:GetShadowColor() return a")
            .unwrap(),
        0.3,
        "setting the offset alone keeps the colour"
    );
}

/// `Region:SetParent` (`0x7a1550`, reached by both leaf lookups' fallback: `0x79c650`, `0x79ee50`)
/// moves the region's draw-list membership and leaves its anchors alone; a non-Frame argument
/// raises (`IsA(FrameTag)` at `0x7a16ea`).
#[test]
fn region_set_parent_relinks_the_draw_owner_and_leaves_anchors_alone() {
    /// The resolved rect of the one texture quad whose path contains `needle`, if it draws at all.
    fn tex_rect(s: &crate::script::UiScript, needle: &str) -> Option<crate::layout::Rect> {
        s.extract().iter().find_map(|q| match &q.content {
            crate::script::QuadContent::Texture { path: Some(p), .. } if p.contains(needle) => {
                Some(q.rect.expect("resolved rect"))
            }
            _ => None,
        })
    }

    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        A = CreateFrame("Frame", "SPOwnerA")
        A:SetPoint("BOTTOMLEFT", 0, 0)  A:SetWidth(100); A:SetHeight(50)
        B = CreateFrame("Frame", "SPOwnerB")
        B:SetPoint("BOTTOMLEFT", 300, 0)  B:SetWidth(100); B:SetHeight(50)
        Spark = A:CreateTexture("SPSpark", "ARTWORK")
        Spark:SetTexture("Interface\\SPGlow")
        Spark:SetWidth(24); Spark:SetHeight(24)
        Spark:SetPoint("TOPLEFT", 4, -4)
        "#,
    )
    .unwrap();
    s.resolve();
    // A is [bottom 0, left 0, top 50, right 100]: TOPLEFT +(4,-4), 24x24 ⇒ 4..28 x 22..46.
    let before = tex_rect(&s, "SPGlow").expect("the spark draws under A");
    assert_eq!(before, crate::layout::Rect::new(22.0, 4.0, 46.0, 28.0));

    assert_eq!(
        s.arity("Spark:SetParent(B)").unwrap(),
        0,
        "SetParent returns zero values on every path"
    );
    s.resolve();
    assert_eq!(
        tex_rect(&s, "SPGlow"),
        Some(before),
        "the anchors are untouched — the spark still resolves against A, 300px from B"
    );

    s.run("B:Hide()").unwrap();
    assert_eq!(
        tex_rect(&s, "SPGlow"),
        None,
        "the spark hides with its NEW owner"
    );
    s.run("B:Show() A:Hide()").unwrap();
    s.resolve();
    assert_eq!(
        tex_rect(&s, "SPGlow"),
        Some(before),
        "and not with the frame it is merely anchored to"
    );

    for bad in ["Spark:SetParent(Spark)", "Spark:SetParent(5)"] {
        let e = s
            .run(bad)
            .expect_err("a non-frame parent must raise")
            .to_string();
        assert!(
            e.contains("expected frame"),
            "wanted the reference's 'expected frame' rejection, got: {e}"
        );
    }
    // A missing argument is `TNONE`, which never reaches the nil branch.
    assert!(s.run("Spark:SetParent()").is_err(), "no argument raises");

    // nil detaches the region without destroying it.
    s.run("A:Show() Spark:SetParent(nil)").unwrap();
    s.resolve();
    assert_eq!(
        tex_rect(&s, "SPGlow"),
        None,
        "a detached region draws nothing"
    );
    s.run("Spark:SetParent(A)").unwrap();
    s.resolve();
    assert_eq!(
        tex_rect(&s, "SPGlow"),
        Some(before),
        "and re-parenting restores it"
    );
}

/// `Button:SetFont` (`0x780880`) returns nothing, discarding the shared impl's `1`/nil
/// (`xor eax,eax`), and sets the button's normal font without touching the label pointer
/// (`+0x338`), so a bare Button makes no label; `GetFont` returns three values off that font.
#[test]
fn button_set_font_returns_nothing_and_is_a_no_op_without_a_label() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        Bare = CreateFrame("Button", "SFBare")
        Labelled = CreateFrame("Button", "SFLabelled")
        Labelled:SetText("Show Keybinds")
        "#,
    )
    .unwrap();

    assert_eq!(
        s.arity(r#"SFBare:SetFont("Fonts\\FRIZQT__.TTF", 8)"#)
            .unwrap(),
        0,
        "SetFont returns ZERO values — the delegate's 1/nil is discarded by a Button"
    );
    assert!(
        s.eval::<bool>("return SFBare:GetFontString() == nil")
            .unwrap(),
        "no label existed and none was lazily created"
    );

    assert_eq!(
        s.arity("SFBare:GetFont()").unwrap(),
        3,
        "GetFont returns THREE values"
    );
    assert_eq!(
        s.eval::<(String, f32, String)>("return SFBare:GetFont()")
            .unwrap(),
        ("Fonts\\FRIZQT__.TTF".to_string(), 8.0, String::new())
    );
    // Nothing set: still three values, the path and height nil.
    assert_eq!(s.arity("SFLabelled:GetFont()").unwrap(), 3);
    assert!(s
        .eval::<bool>("local f = SFLabelled:GetFont() return f == nil")
        .unwrap());

    s.run(r#"SFBare:SetFont("Fonts\\SKURRI.TTF", 12, "THICKOUTLINE")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("local _, _, flags = SFBare:GetFont() return flags")
            .unwrap(),
        "THICKOUTLINE"
    );
    // Both arguments are required (`lua_isstring` + `lua_isnumber`, else the usage error).
    assert!(s.run(r#"SFBare:SetFont("Fonts\\SKURRI.TTF")"#).is_err());

    // Unset locally, GetFont reads through the normal state's font object.
    s.run(
        r#"
        CreateFont("SFInherited")
        SFInherited:SetFont("Fonts\\MORPHEUS.TTF", 14)
        SFLabelled:SetTextFontObject(SFInherited)
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(String, f32)>("local p, h = SFLabelled:GetFont() return p, h")
            .unwrap(),
        ("Fonts\\MORPHEUS.TTF".to_string(), 14.0)
    );

    // A local SetFont outranks that object on the label's paint too.
    s.run(r#"SFLabelled:SetFont("Fonts\\SKURRI.TTF", 9)"#)
        .unwrap();
    let (font, height) = s
        .extract()
        .iter()
        .find_map(|q| match &q.content {
            crate::script::QuadContent::Text {
                text: Some(t),
                font,
                font_height,
                ..
            } if t == "Show Keybinds" => Some((font.clone(), *font_height)),
            _ => None,
        })
        .expect("the labelled button draws its text");
    assert_eq!(
        (font.as_deref(), height),
        (Some("Fonts\\SKURRI.TTF"), Some(9.0)),
        "the button's own font repaints the label over the inherited font object"
    );
}

/// FontString's `SetNonSpaceWrap`/`CanNonSpaceWrap` (`0x79e9f0`/`0x79ead0`): on by default, the
/// getter answers `1` or nil, and a bare `SetNonSpaceWrap()` turns it on.
#[test]
fn non_space_wrap_defaults_on_and_answers_one_or_nil() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(r#"f = CreateFrame("Frame", "NSF") fs = f:CreateFontString("NSFText", "ARTWORK")"#)
        .unwrap();

    assert_eq!(
        s.eval::<Option<i64>>("return NSFText:CanNonSpaceWrap()")
            .unwrap(),
        Some(1),
        "default on, answered as 1"
    );
    assert!(
        !s.eval::<bool>("return NSFText:CanNonSpaceWrap() == true")
            .unwrap(),
        "it must NOT be a boolean — an addon comparing against 1 would break"
    );

    s.run("NSFText:SetNonSpaceWrap(false)").unwrap();
    assert_eq!(
        s.eval::<Option<i64>>("return NSFText:CanNonSpaceWrap()")
            .unwrap(),
        None,
        "off answers nil, not 0 and not false"
    );

    s.run("NSFText:SetNonSpaceWrap()").unwrap();
    assert_eq!(
        s.eval::<Option<i64>>("return NSFText:CanNonSpaceWrap()")
            .unwrap(),
        Some(1),
        "a bare SetNonSpaceWrap() turns it ON"
    );
}

/// EditBox's registrar table (`.data 0x87bb68`, 48 entries, `mov edx,0x30` at `0x799ab5`) opens
/// with the font block each text-bearing type re-declares over the shared impl `0x79f210`: its
/// fourteen font verbs answer, its `SetSpacing`/`GetSpacing` (#10, #11) are not installed as
/// nothing models line spacing, and other tables' names do not leak onto it.
#[test]
fn editbox_carries_the_font_block_its_registrar_table_declares() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(r#"EB = CreateFrame("EditBox", "EBFont")"#).unwrap();

    for name in [
        "SetFontObject",
        "GetFontObject",
        "SetFont",
        "GetFont",
        "SetTextColor",
        "GetTextColor",
        "SetShadowColor",
        "GetShadowColor",
        "SetShadowOffset",
        "GetShadowOffset",
        "SetJustifyH",
        "GetJustifyH",
        "SetJustifyV",
        "GetJustifyV",
    ] {
        assert_eq!(
            s.eval::<String>(&format!("return type(EBFont.{name})"))
                .unwrap(),
            "function",
            "EditBox table entry '{name}' is missing"
        );
    }

    for name in [
        // On EditBox's table, not installed: nothing models line spacing.
        "SetSpacing",
        "GetSpacing",
        // Not on EditBox's table.
        "CopyFontObject",
        "SetNonSpaceWrap",
        "CanNonSpaceWrap",
        "SetTextHeight",
        "GetStringWidth",
    ] {
        assert!(
            s.eval::<bool>(&format!("return EBFont.{name} == nil"))
                .unwrap(),
            "'{name}' must NOT answer on an EditBox"
        );
    }
}

/// Each EditBox font binding returns what the shared implementation returns (no `mov eax,N` or
/// `xor eax,eax` after the call), so unlike `Button:SetFont` (`0x780880`), `EditBox:SetFont`
/// (`0x797210` → `0x79f210`) answers the number `1` or nil (`0x79f345`/`0x79f361`), and
/// `GetShadowColor` returns four values (`0x79f9b3 mov eax,0x4`).
#[test]
fn editbox_font_block_return_shapes_are_the_shared_implementations() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        EB = CreateFrame("EditBox", "EBShape")
        CreateFont("EBProbeFont")
        EBProbeFont:SetFont("Fonts\\FRIZQT__.TTF", 14)
        "#,
    )
    .unwrap();

    for call in [
        "SetFontObject(EBProbeFont)",
        "SetTextColor(1, 0, 0)",
        "SetShadowColor(0, 0, 0, 0.3)",
        "SetShadowOffset(1, -1)",
        r#"SetJustifyH("CENTER")"#,
        r#"SetJustifyV("TOP")"#,
    ] {
        assert_eq!(
            s.arity(&format!("EBShape:{call}")).unwrap(),
            0,
            "EditBox:{call} must return ZERO values"
        );
    }

    for (call, want) in [
        ("GetFontObject()", 1),
        ("GetFont()", 3),
        ("GetTextColor()", 4),
        ("GetShadowColor()", 4),
        ("GetShadowOffset()", 2),
        ("GetJustifyH()", 1),
        ("GetJustifyV()", 1),
    ] {
        assert_eq!(
            s.arity(&format!("EBShape:{call}")).unwrap(),
            want,
            "EditBox:{call} must return {want} value(s)"
        );
    }

    assert_eq!(
        s.arity(r#"EBShape:SetFont("Fonts\\SKURRI.TTF", 12)"#)
            .unwrap(),
        1,
        "EditBox:SetFont returns one value, unlike Button:SetFont"
    );
    assert_eq!(
        s.eval::<String>(r#"return type(EBShape:SetFont("Fonts\\SKURRI.TTF", 12))"#)
            .unwrap(),
        "number",
        "the number 1, never the boolean true"
    );
    assert!(s
        .eval::<bool>(r#"return EBShape:SetFont("Fonts\\SKURRI.TTF", 12) == 1"#)
        .unwrap());
    // An unloadable (empty) path answers nil, not an error.
    assert!(s
        .eval::<bool>(r#"return EBShape:SetFont("", 12) == nil"#)
        .unwrap());
    // Both arguments are gated (`lua_isstring` + `lua_isnumber`, else the usage error).
    assert!(s.run(r#"EBShape:SetFont("Fonts\\SKURRI.TTF")"#).is_err());

    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return EBShape:GetShadowColor()")
            .unwrap(),
        (0.0, 0.0, 0.0, 0.3)
    );
    assert_eq!(
        s.eval::<(f32, f32)>("return EBShape:GetShadowOffset()")
            .unwrap(),
        (1.0, -1.0)
    );
    // GetFontObject answers the object, not its name.
    assert_eq!(
        s.eval::<String>("return type(EBShape:GetFontObject())")
            .unwrap(),
        "table"
    );
}

/// `Dewdrop-2.0.lua:1673-1675`'s shape: an anonymous EditBox under a frame, then
/// `SetFontObject(ChatFontNormal)`.
#[test]
fn the_dewdrop_editbox_font_shape_resolves_and_paints() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        CreateFont("ChatFontNormal")
        ChatFontNormal:SetFont("Fonts\\ARIALN.TTF", 11)
        ChatFontNormal:SetTextColor(0.9, 0.9, 0.9)

        editBoxFrame = CreateFrame("Frame", "DewdropEBFrame")
        local editBox = CreateFrame("EditBox", nil, editBoxFrame)
        editBoxFrame.editBox = editBox
        editBox:SetFontObject(ChatFontNormal)
        editBox:SetWidth(160)
        editBox:SetHeight(13)
        "#,
    )
    .unwrap();

    assert!(s
        .eval::<bool>("return DewdropEBFrame.editBox:GetFontObject() == ChatFontNormal")
        .unwrap());
    // The object's paint reaches the box's own FontString (`[this+0x324]`, what the shim passes).
    assert_eq!(
        s.eval::<(String, f32)>("local p, h = DewdropEBFrame.editBox:GetFont() return p, h")
            .unwrap(),
        ("Fonts\\ARIALN.TTF".to_string(), 11.0)
    );
    let (r, g, b, _) = s
        .eval::<(f32, f32, f32, f32)>("return DewdropEBFrame.editBox:GetTextColor()")
        .unwrap();
    assert_eq!((r, g, b), (0.9, 0.9, 0.9));

    // `AceGUIWidget-Slider.lua:204-210` ends the same shape with this.
    s.run(r#"DewdropEBFrame.editBox:SetJustifyH("CENTER")"#)
        .unwrap();
}

/// The justify enum (`.rdata 0x811ad0`) keeps both axes in one dword, bits 0-2 horizontal and 3-5
/// vertical: an unknown token raises (`0x87c77c`), and a token from the other axis masks to
/// nothing, so `GetJustifyH()` answers `"UNKNOWN"` (`0x6f1a00`, `.data 0x838044`).
#[test]
fn editbox_justify_masks_to_its_axis_and_answers_unknown() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(r#"EB = CreateFrame("EditBox", "EBJustify")"#)
        .unwrap();

    // A fresh EditBox is LEFT: its ctor turns the font default `0x212` (CENTER | MIDDLE | 0x200)
    // into `0x211` (`0x779bcd`, stored at `0x779be4`).
    assert_eq!(
        s.eval::<String>("return EBJustify:GetJustifyH()").unwrap(),
        "LEFT"
    );
    assert_eq!(
        s.eval::<String>("return EBJustify:GetJustifyV()").unwrap(),
        "MIDDLE"
    );

    s.run(r#"EBJustify:SetJustifyH("LEFT") EBJustify:SetJustifyV("BOTTOM")"#)
        .unwrap();
    assert_eq!(
        s.eval::<(String, String)>("return EBJustify:GetJustifyH(), EBJustify:GetJustifyV()")
            .unwrap(),
        ("LEFT".to_string(), "BOTTOM".to_string())
    );

    s.run(r#"EBJustify:SetJustifyH("TOP")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return EBJustify:GetJustifyH()").unwrap(),
        "UNKNOWN",
        "a vertical token on SetJustifyH clears the axis rather than centering it"
    );
    assert_eq!(
        s.eval::<String>("return EBJustify:GetJustifyV()").unwrap(),
        "BOTTOM",
        "and it leaves the vertical axis untouched"
    );

    assert!(s.run(r#"EBJustify:SetJustifyH("SIDEWAYS")"#).is_err());
}

/// `EditBox:SetJustifyV` reaches the getter (`0x79fd73` reads the font instance) but never the
/// pixels: `SetMultiLine 0x77a4a0`, which the ctor always calls (`0x779c2f`), clears the vertical
/// inherit bits (`CSimpleFontString+0x124`, over the drawn justify `+0x120`) and writes TOP or
/// MIDDLE itself; nothing sets those bits back, so `0x77086e` masks the setter's value out.
#[test]
fn editbox_justify_v_echoes_but_multiline_alone_decides_the_pixels() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        EB = CreateFrame("EditBox", "EBJV")
        EBJV:SetPoint("CENTER", 0, 0) EBJV:SetWidth(120) EBJV:SetHeight(40)
        EBJV:SetText("typed")
        "#,
    )
    .unwrap();

    /// The vertical justification the box's text actually draws with.
    fn drawn_v(s: &crate::script::UiScript) -> crate::script::JustifyV {
        s.extract()
            .iter()
            .find_map(|q| match &q.content {
                crate::script::QuadContent::Text {
                    text: Some(t),
                    justify_v,
                    ..
                } if t == "typed" => Some(*justify_v),
                _ => None,
            })
            .expect("the box draws its text")
    }

    // A single-line box renders MIDDLE, the `0x77a599` leg.
    assert_eq!(drawn_v(&s), crate::script::JustifyV::Middle);

    s.run(r#"EBJV:SetJustifyV("TOP")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return EBJV:GetJustifyV()").unwrap(),
        "TOP",
        "the getter reads the font instance and echoes faithfully"
    );
    assert_eq!(
        drawn_v(&s),
        crate::script::JustifyV::Middle,
        "SetJustifyV is masked out of the rendered justify — multiLine alone decides it"
    );

    // Multi-line moves it, the `0x77a509` leg.
    s.run("EBJV:SetMultiLine(true)").unwrap();
    assert_eq!(drawn_v(&s), crate::script::JustifyV::Top);
    assert_eq!(
        s.eval::<String>("return EBJV:GetJustifyV()").unwrap(),
        "TOP"
    );
}

/// `Texture:SetDesaturated` (`0x79c1e0`) answers `shaderSupported`, `1` or nil; benilla's renderer
/// greys, so it answers `1`, and stock `ItemButtonTemplate.lua:69` keeps the caller's tint rather
/// than its 0.5 fallback.
#[test]
fn set_desaturated_reports_shader_support_and_does_not_raise() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(r#"f = CreateFrame("Frame", "DsF") tex = f:CreateTexture("DsTex", "ARTWORK")"#)
        .unwrap();

    s.run("DsTex:SetDesaturated(true)").unwrap();

    assert!(
        s.eval::<bool>("return DsTex:SetDesaturated(true) == 1")
            .unwrap(),
        "shaderSupported must be 1 (1|nil C shape), not true"
    );
    assert_eq!(
        s.arity("DsTex:SetDesaturated(true)").unwrap(),
        1,
        "one return value"
    );

    // `SetItemButtonDesaturated`'s branch, transcribed: a truthy answer keeps the caller's tint.
    s.run(
        r#"
        local shaderSupported = DsTex:SetDesaturated(true)
        local r, g, b = 0.65, 0.65, 0.65
        if not shaderSupported then r, g, b = 0.5, 0.5, 0.5 end
        DsTex:SetVertexColor(r, g, b)
        "#,
    )
    .unwrap();
    let (r, g, b) = s
        .eval::<(f32, f32, f32)>("return DsTex:GetVertexColor()")
        .unwrap();
    assert!(
        (r - 0.65).abs() < 1e-6 && (g - 0.65).abs() < 1e-6 && (b - 0.65).abs() < 1e-6,
        "the shader arm keeps the caller's tint, got {r},{g},{b}"
    );

    s.run("DsTex:SetDesaturated(nil) DsTex:SetDesaturated(false)")
        .unwrap();
}

/// `UnitCreatureType` (`0x51a280`, stages 2 and 3 of three built): a creature's cached record,
/// else (`0x605570`) the race's `ChrRaces.dbc` column 9, which is 7, "Humanoid", for every race.
#[test]
fn unit_creature_type_answers_the_record_then_falls_back_to_humanoid() {
    let mut s = script();
    s.set_unit(
        "target",
        Some(crate::script::UnitState {
            exists: true,
            creature_type_name: Some("Beast".into()),
            ..Default::default()
        }),
    );
    s.set_unit(
        "player",
        Some(crate::script::UnitState {
            exists: true,
            is_player: true,
            ..Default::default()
        }),
    );

    assert_eq!(
        s.eval::<String>(r#"return UnitCreatureType("target")"#)
            .unwrap(),
        "Beast"
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitCreatureType("player")"#)
            .unwrap(),
        "Humanoid"
    );
    // An unresolved token is nil; a missing argument fails `lua_isstring` and raises.
    assert!(s
        .eval::<bool>(r#"return UnitCreatureType("party4") == nil"#)
        .unwrap());
    let err = s
        .run("UnitCreatureType()")
        .expect_err("a missing arg must raise");
    assert!(
        format!("{err}").contains("Usage: UnitCreatureType"),
        "got {err}"
    );
}

/// `GetInventorySlotInfo` (`0x4c81b0`) matches the whole name case-insensitively (`0x4c8215`,
/// `_strnicmp`) and returns three values, the third the number 1 for `RangedSlot` alone; a miss
/// raises the reference's message, with no `Usage:` and no name.
#[test]
fn get_inventory_slot_info_folds_case_and_flags_only_the_ranged_slot() {
    let s = script();

    for name in ["AmmoSlot", "ammoSlot", "AMMOSLOT"] {
        assert_eq!(
            s.eval::<i64>(&format!("return GetInventorySlotInfo('{name}')"))
                .unwrap(),
            0,
            "{name} must fold to AmmoSlot"
        );
    }
    assert_eq!(
        s.eval::<i64>("return GetInventorySlotInfo('MAINHANDSLOT')")
            .unwrap(),
        16
    );

    assert_eq!(s.arity("GetInventorySlotInfo('HeadSlot')").unwrap(), 3);
    let (id, art) = s
        .eval::<(i64, String)>("return GetInventorySlotInfo('HeadSlot')")
        .unwrap();
    assert_eq!(id, 1);
    // The DBC string verbatim, lowercase directory and `.blp`: the binding pushes `[esi+4]` as is.
    assert_eq!(art, "interface\\paperdoll\\UI-PaperDoll-Slot-Head.blp");

    assert!(s
        .eval::<bool>("local _,_,r = GetInventorySlotInfo('RangedSlot') return r == 1")
        .unwrap());
    assert!(s
        .eval::<bool>("local _,_,r = GetInventorySlotInfo('HeadSlot') return r == nil")
        .unwrap());

    // `Bag1`..`Bag12` are SlotNumbers 64..75 (64..69 the bank bags) and share one string offset
    // with `Bag0Slot`..`Bag3Slot`, so all sixteen answer the same art.
    assert_eq!(
        s.eval::<i64>("return GetInventorySlotInfo('Bag1')")
            .unwrap(),
        64
    );
    assert_eq!(
        s.eval::<i64>("return GetInventorySlotInfo('bag12')")
            .unwrap(),
        75,
        "the new rows fold case like every other"
    );
    assert_eq!(
        s.eval::<String>("local _,a = GetInventorySlotInfo('Bag6') return a")
            .unwrap(),
        s.eval::<String>("local _,a = GetInventorySlotInfo('Bag0Slot') return a")
            .unwrap(),
        "all sixteen bag rows share one string-block offset"
    );
    // `Bag1` (64) and `Bag1Slot` (21) are different rows.
    assert_eq!(
        s.eval::<i64>("return GetInventorySlotInfo('Bag1Slot')")
            .unwrap(),
        21
    );

    let err = s
        .run("GetInventorySlotInfo('NoSuchSlot')")
        .expect_err("an unknown slot name must raise");
    let err = format!("{err}");
    assert!(
        err.contains("Invalid inventory slot in GetInventorySlotInfo"),
        "got {err}"
    );
    assert!(
        !err.contains("NoSuchSlot"),
        "the reference does not interpolate the offending name: {err}"
    );
}

/// Every Region-map name (`0xcf54b4`, 19 entries) is callable on a Texture and a FontString, whose
/// lookups fall back to it (FontString's `0x79ee20` chains its own `0xcf5400`); a Font object's
/// lookup (`0x7a1100`) has no fallback.
#[test]
fn every_region_map_method_is_callable_on_a_texture_and_a_fontstring() {
    /// All 19, `GetObjectType`/`IsObjectType` (`0x7a11d0`/`0x7a1290`) among them.
    const REGION_MAP: [&str; 19] = crate::script::REGION_MAP_METHODS;
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        RMOwner = CreateFrame("Frame", "RMOwner")
        RMOwner:SetPoint("BOTTOMLEFT", 0, 0)  RMOwner:SetWidth(100); RMOwner:SetHeight(50)
        RMTex = RMOwner:CreateTexture("RMTex", "ARTWORK")
        RMStr = RMOwner:CreateFontString("RMStr", "ARTWORK")
        "#,
    )
    .unwrap();

    let mut absent: Vec<String> = Vec::new();
    for kind in ["RMTex", "RMStr"] {
        for name in REGION_MAP {
            if !s
                .eval::<bool>(&format!("return type({kind}.{name}) == 'function'"))
                .unwrap_or(false)
            {
                absent.push(format!("{kind}:{name}"));
            }
        }
    }
    assert!(
        absent.is_empty(),
        "the Region map 0xcf54b4 is missing from our regions: {absent:?}"
    );
}

/// The Region-map readers where a region differs from a frame: `GetParent` is the owner frame
/// (`TheoryCraftUI.lua:720` calls it on a FontString), and `GetPoint` answers a sibling region.
#[test]
fn the_region_map_readers_answer_the_way_the_edges_do() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        RRFrame = CreateFrame("Frame", "RRFrame")
        RRFrame:SetPoint("BOTTOMLEFT", 100, 200)  RRFrame:SetWidth(200); RRFrame:SetHeight(100)
        RRPlate = RRFrame:CreateTexture("RRPlate", "ARTWORK")
        RRPlate:SetPoint("TOPLEFT", 10, -10)  RRPlate:SetWidth(50); RRPlate:SetHeight(20)
        -- the sibling-region anchor the real XML uses everywhere
        RRLabel = RRFrame:CreateFontString("RRLabel", "OVERLAY")
        RRLabel:SetPoint("LEFT", RRPlate, "RIGHT", 4, 0)
        "#,
    )
    .unwrap();
    s.resolve();

    assert!(
        s.eval::<bool>("return RRPlate:GetParent() == RRFrame")
            .unwrap(),
        "a region's parent is the frame that created it"
    );
    assert!(
        s.eval::<bool>("return RRLabel:GetParent():GetName() == 'RRFrame'")
            .unwrap(),
        "…and it is a real frame handle, not a bare id"
    );

    // GetCenter agrees with the edge readers by construction, so none of them is scaled alone.
    let (cx, cy): (f64, f64) = s.eval("return RRPlate:GetCenter()").unwrap();
    let (l, r, t, b): (f64, f64, f64, f64) = s
        .eval("return RRPlate:GetLeft(), RRPlate:GetRight(), RRPlate:GetTop(), RRPlate:GetBottom()")
        .unwrap();
    assert_eq!((cx, cy), ((l + r) * 0.5, (t + b) * 0.5));

    assert_eq!(s.eval::<i64>("return RRPlate:GetNumPoints()").unwrap(), 1);
    assert_eq!(
        s.eval::<i64>("return RRLabel:GetNumPoints()").unwrap(),
        1,
        "the sibling-anchored label carries its one point"
    );

    // Frames and regions share an id space, so `relativeTo` must come back as the region handle,
    // not a frame wrapper onto its id.
    let (p, rp, x, y): (String, String, f64, f64) = s
        .eval("local p, _, rp, x, y = RRLabel:GetPoint(1) return p, rp, x, y")
        .unwrap();
    assert_eq!((p.as_str(), rp.as_str(), x, y), ("LEFT", "RIGHT", 4.0, 0.0));
    assert!(
        s.eval::<bool>("local _, rel = RRLabel:GetPoint(1) return rel == RRPlate")
            .unwrap(),
        "relativeTo is the sibling REGION handle itself"
    );
    assert_eq!(
        s.arity("RRPlate:GetPoint(7)").unwrap(),
        5,
        "an out-of-range index still answers five values, all nil"
    );
    assert!(
        s.eval::<bool>("return RRPlate:GetPoint(7) == nil").unwrap(),
        "…and the first of them is nil"
    );
}

/// A region's `GetObjectType`/`IsObjectType` (`0x7a11d0`/`0x7a1290`).
#[test]
fn the_type_identity_verbs_answer_what_the_binary_answers() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        TIOwner = CreateFrame("Frame", "TIOwner")
        TITex = TIOwner:CreateTexture("TITex", "ARTWORK")
        TIStr = TIOwner:CreateFontString("TIStr", "ARTWORK")
        TIAnon = TIOwner:CreateTexture(nil, "ARTWORK")
        "#,
    )
    .unwrap();

    // One value each (`lua_pushstring`); extra arguments are ignored.
    assert_eq!(
        s.eval::<String>("return TITex:GetObjectType()").unwrap(),
        "Texture"
    );
    assert_eq!(
        s.eval::<String>("return TIStr:GetObjectType()").unwrap(),
        "FontString"
    );
    assert_eq!(
        s.arity("TITex:GetObjectType('ignored')").unwrap(),
        1,
        "one value, and a stray argument is ignored rather than an arity error"
    );

    // The chain is two deep: the leaf, then Region.
    for (obj, leaf) in [("TITex", "Texture"), ("TIStr", "FontString")] {
        assert_eq!(
            s.eval::<i64>(&format!("return {obj}:IsObjectType('{leaf}')"))
                .unwrap(),
            1,
            "{obj} is its own leaf type"
        );
        assert_eq!(
            s.eval::<i64>(&format!("return {obj}:IsObjectType('Region')"))
                .unwrap(),
            1,
            "{obj} is a Region"
        );
        // 1.12.1 has no LayoutFrame, ScriptObject or Object type at all.
        for absent in ["LayoutFrame", "ScriptObject", "Object", "Frame", "Font"] {
            assert!(
                s.eval::<Option<i64>>(&format!("return {obj}:IsObjectType('{absent}')"))
                    .unwrap()
                    .is_none(),
                "{obj}:IsObjectType('{absent}') must be nil — 1.12 has no such type in the chain"
            );
        }
    }
    assert!(s
        .eval::<Option<i64>>("return TITex:IsObjectType('FontString')")
        .unwrap()
        .is_none());

    // Case-insensitive and whole-string (`SStrCmpI`).
    for spelling in ["texture", "TEXTURE", "TeXtUrE", "region", "REGION"] {
        assert_eq!(
            s.eval::<i64>(&format!("return TITex:IsObjectType('{spelling}')"))
                .unwrap(),
            1,
            "'{spelling}' must match — the compare folds case"
        );
    }
    for partial in ["Tex", "TextureX", "Regio", ""] {
        assert!(
            s.eval::<Option<i64>>(&format!("return TITex:IsObjectType('{partial}')"))
                .unwrap()
                .is_none(),
            "'{partial}' must NOT match — whole-string, not a prefix"
        );
    }

    assert_eq!(
        s.eval::<String>("return type(TITex:IsObjectType('Texture'))")
            .unwrap(),
        "number",
        "the reference pushes tag 3 (number); tag 1 (boolean) is never written"
    );
    for arg in ["'Texture'", "'nope'"] {
        assert_eq!(
            s.arity(&format!("TITex:IsObjectType({arg})")).unwrap(),
            1,
            "exactly one value on both the hit and the miss path"
        );
    }

    // A number passes `lua_isstring` and never matches a type name.
    assert!(
        s.eval::<Option<i64>>("return TITex:IsObjectType(5)")
            .unwrap()
            .is_none(),
        "a number argument is accepted and quietly answers nil, NOT a raise"
    );

    for bad in ["", "nil", "true", "{}", "print"] {
        let err = s
            .run(&format!("TITex:IsObjectType({bad})"))
            .expect_err(&format!("IsObjectType({bad}) must raise"))
            .to_string();
        assert!(
            err.contains(r#"Usage: TITex:IsObjectType("TYPE")"#),
            "the raise carries the reference's Usage text and the region's name: {err}"
        );
    }
    let anon = s
        .run("TIAnon:IsObjectType(nil)")
        .expect_err("anonymous region raises too")
        .to_string();
    assert!(
        anon.contains(r#"Usage: <unnamed>:IsObjectType("TYPE")"#),
        "an anonymous region reports <unnamed>, as GetName()'s absence does: {anon}"
    );
}

/// Frames also carry 1.12's frame-side pair, `GetFrameType 0x773640`/`IsFrameType 0x773700`
/// (`CSimpleFrameScript.cpp`), beside `CScriptRegion`'s `GetObjectType`/`IsObjectType`.
#[test]
fn the_frame_side_type_identity_verbs_are_1_12s_own_names() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        FTFrame = CreateFrame("Frame", "FTFrame")
        FTButton = CreateFrame("Button", "FTButton")
        FTModel = CreateFrame("Model", nil, FTFrame)
        FTTex = FTFrame:CreateTexture("FTTex", "ARTWORK")
        "#,
    )
    .unwrap();

    // Both read the same per-class type-name slot (`[edx+0x1c]`).
    for (obj, leaf) in [
        ("FTFrame", "Frame"),
        ("FTButton", "Button"),
        ("FTModel", "Model"),
    ] {
        assert_eq!(
            s.eval::<String>(&format!("return {obj}:GetFrameType()"))
                .unwrap(),
            leaf
        );
        assert_eq!(
            s.eval::<String>(&format!("return {obj}:GetObjectType()"))
                .unwrap(),
            leaf,
            "the two names must never disagree — they read one slot"
        );
    }

    // How Cartographer's `LookNFeel.lua:368` finds the world map's player arrow.
    assert!(
        s.eval::<bool>(r#"return FTModel:GetFrameType() == "Model" and not FTModel:GetName()"#)
            .unwrap(),
        "an anonymous Model must answer its type AND a nil name — LookNFeel.lua:368"
    );

    assert_eq!(
        s.eval::<i64>("return FTButton:IsFrameType('Button')")
            .unwrap(),
        1
    );
    assert_eq!(
        s.eval::<i64>("return FTButton:IsFrameType('frame')")
            .unwrap(),
        1
    );
    assert_eq!(
        s.eval::<i64>("return FTButton:IsFrameType('REGION')")
            .unwrap(),
        1
    );
    for absent in ["LayoutFrame", "ScriptObject", "Object", "Slider", "Fram"] {
        assert!(
            s.eval::<Option<i64>>(&format!("return FTButton:IsFrameType('{absent}')"))
                .unwrap()
                .is_none(),
            "{absent} is not in a Button's chain"
        );
    }

    // A region has no frame-side pair: the two registrars are distinct.
    assert!(s.eval::<bool>("return FTTex.GetFrameType == nil").unwrap());
    assert!(s.eval::<bool>("return FTTex.IsFrameType == nil").unwrap());
}

/// A frame's type chain follows the reference's 23-class roster, a hardcoded list per class rather
/// than a runtime parent walk.
#[test]
fn a_frames_type_chain_matches_the_roster() {
    let s = crate::script::UiScript::new().unwrap();
    // (CreateFrame kind, GetObjectType string, full chain)
    let cases: &[(&str, &str, &[&str])] = &[
        ("Frame", "Frame", &["Frame", "Region"]),
        ("Button", "Button", &["Button", "Frame", "Region"]),
        (
            "CheckButton",
            "CheckButton",
            &["CheckButton", "Button", "Frame", "Region"],
        ),
        ("EditBox", "EditBox", &["EditBox", "Frame", "Region"]),
        ("StatusBar", "StatusBar", &["StatusBar", "Frame", "Region"]),
        ("Slider", "Slider", &["Slider", "Frame", "Region"]),
        (
            "ScrollFrame",
            "ScrollFrame",
            &["ScrollFrame", "Frame", "Region"],
        ),
        (
            "MessageFrame",
            "MessageFrame",
            &["MessageFrame", "Frame", "Region"],
        ),
        // Not via MessageFrame, despite the name (roster `0x787940`).
        (
            "ScrollingMessageFrame",
            "ScrollingMessageFrame",
            &["ScrollingMessageFrame", "Frame", "Region"],
        ),
        // Capital HTML, unlike the `SimpleHtml` variant; addons compare the string with `==`.
        (
            "SimpleHTML",
            "SimpleHTML",
            &["SimpleHTML", "Frame", "Region"],
        ),
        (
            "ColorSelect",
            "ColorSelect",
            &["ColorSelect", "Frame", "Region"],
        ),
        (
            "GameTooltip",
            "GameTooltip",
            &["GameTooltip", "Frame", "Region"],
        ),
        ("Minimap", "Minimap", &["Minimap", "Frame", "Region"]),
        // `PlayerModel` derives from `Model` (`0x505830`/`0x5057c0`), so a `SetUnit` portrait pane
        // has `SetCamera` too; `DressUpModel`, like `TabardModel`, derives from `PlayerModel`: the
        // roster's maximum depth, 5.
        (
            "PlayerModel",
            "PlayerModel",
            &["PlayerModel", "Model", "Frame", "Region"],
        ),
        (
            "DressUpModel",
            "DressUpModel",
            &["DressUpModel", "PlayerModel", "Model", "Frame", "Region"],
        ),
    ];
    for (kind, leaf, chain) in cases {
        s.run(&format!(r#"TC = CreateFrame("{kind}")"#))
            .unwrap_or_else(|e| panic!("CreateFrame(\"{kind}\"): {e}"));
        assert_eq!(
            &s.eval::<String>("return TC:GetObjectType()").unwrap(),
            leaf,
            "{kind}:GetObjectType()"
        );
        for want in *chain {
            assert_eq!(
                s.eval::<i64>(&format!("return TC:IsObjectType('{want}')"))
                    .unwrap(),
                1,
                "{kind} must be a {want}"
            );
        }
        for (other, other_leaf, _) in cases {
            if chain.contains(other_leaf) {
                continue;
            }
            assert!(
                s.eval::<Option<i64>>(&format!("return TC:IsObjectType('{other_leaf}')"))
                    .unwrap()
                    .is_none(),
                "{kind}'s chain must NOT contain {other_leaf} (via {other})"
            );
        }
    }

    s.run(r#"TCNamed = CreateFrame("Button", "TCNamed")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return type(TCNamed:IsObjectType('button'))")
            .unwrap(),
        "number",
        "case-folded hit is the number 1"
    );
    let err = s
        .run("TCNamed:IsObjectType({})")
        .expect_err("a table argument raises")
        .to_string();
    assert!(
        err.contains(r#"Usage: TCNamed:IsObjectType("TYPE")"#),
        "the frame's Usage text names the frame: {err}"
    );
}

/// Every name on the full region table the VM holds is in a leaf list: `script::region` copies
/// names out of it into the Texture and FontString leaves, and one in neither is unreachable.
#[test]
fn every_installed_region_method_lands_in_a_leaf() {
    use crate::script::{
        FONTSTRING_ONLY_METHODS, REGION_LEAF_SHARED, REGION_MAP_METHODS, TEXTURE_ONLY_METHODS,
    };
    let s = crate::script::UiScript::new().unwrap();
    let full: mlua::Table = s
        .lua()
        .named_registry_value(crate::script::REG_REGION_METHODS_FOR_TEST)
        .expect("the full region method table");

    let mut known: std::collections::HashSet<&str> = std::collections::HashSet::new();
    known.extend(REGION_MAP_METHODS);
    known.extend(REGION_LEAF_SHARED);
    known.extend(TEXTURE_ONLY_METHODS);
    known.extend(FONTSTRING_ONLY_METHODS);

    let mut orphans: Vec<String> = Vec::new();
    for pair in full.pairs::<String, mlua::Value>() {
        let (name, _) = pair.expect("region method entry");
        if !known.contains(name.as_str()) {
            orphans.push(name);
        }
    }
    orphans.sort();
    assert!(
        orphans.is_empty(),
        "installed on the region table but in NO leaf list, so no region can call them: {orphans:?}"
    );
}

/// The Environment Detail pair, `SetWorldDetail 0x488dd0`/`GetWorldDetail 0x488d70`.
#[test]
fn set_world_detail_writes_the_stop_table_and_validates_like_the_reference() {
    let s = script();
    s.register_cvars([
        (crate::script::CVAR_WORLD_DETAIL, "1"),
        (crate::script::CVAR_FRILL_DENSITY, "32"),
    ]);
    let frill = |s: &crate::script::UiScript| s.cvar(crate::script::CVAR_FRILL_DENSITY);
    let stop = |s: &crate::script::UiScript| s.cvar(crate::script::CVAR_WORLD_DETAIL);

    // The preset table at `0x804518`, verbatim: {16, 32, 48}.
    for (n, want) in [(0, "16"), (1, "32"), (2, "48")] {
        s.run(&format!("SetWorldDetail({n})")).unwrap();
        assert_eq!(
            frill(&s).as_deref(),
            Some(want),
            "stop {n} writes frillDensity"
        );
        assert_eq!(
            s.eval::<i64>("return GetWorldDetail()").unwrap(),
            n,
            "and the getter reads that stop back in the SAME tick — the host has not drained yet"
        );
    }
    assert_eq!(
        crate::script::WORLD_DETAIL_STOPS,
        [16, 32, 48],
        "the table is the reference's, not a transcription that can drift"
    );

    // Truncation is toward zero (`0x40a2b0` sets RC = chop), so -0.5 is stop 0 and accepted.
    s.run("SetWorldDetail(2.9)").unwrap();
    assert_eq!(frill(&s).as_deref(), Some("48"));
    s.run("SetWorldDetail(-0.5)").unwrap();
    assert_eq!(
        frill(&s).as_deref(),
        Some("16"),
        "-0.5 truncates to 0, which is in range: only <= -1 raises"
    );

    // `lua_isnumber` accepts a numeric string.
    s.run(r#"SetWorldDetail("2")"#).unwrap();
    assert_eq!(frill(&s).as_deref(), Some("48"));

    for bad in [
        "SetWorldDetail(3)",
        "SetWorldDetail(-1)",
        "SetWorldDetail(99)",
    ] {
        let e = s.run(bad).unwrap_err().to_string();
        assert!(
            e.contains("value must be in the range 0, 2"),
            "{bad} must raise 0x8423b8 verbatim, got: {e}"
        );
    }
    for bad in [
        "SetWorldDetail()",
        "SetWorldDetail(nil)",
        r#"SetWorldDetail("x")"#,
        "SetWorldDetail({})",
    ] {
        let e = s.run(bad).unwrap_err().to_string();
        assert!(
            e.contains("Usage: SetWorldDetail(value)"),
            "{bad} must raise 0x8423e8 verbatim, got: {e}"
        );
    }

    s.run("SetWorldDetail(1)").unwrap();
    assert_eq!(
        s.arity("SetWorldDetail(1)").unwrap(),
        0,
        "every `ret` in 0x488dd0 leaves eax = 0"
    );
    assert_eq!(s.arity("GetWorldDetail()").unwrap(), 1);
    assert_eq!(
        s.eval::<String>("return type(GetWorldDetail())").unwrap(),
        "number"
    );
    // The getter ignores an argument; pfUI's hook passes one.
    assert_eq!(s.eval::<i64>("return GetWorldDetail(7)").unwrap(), 1);
    assert_eq!(stop(&s).as_deref(), Some("1"));
}

/// pfUI's `hdgraphic` module (`modules/hdgraphic.lua:4-39`), which hooks both verbs to drive
/// `frillDensity` past the top stop, run against the bindings.
#[test]
fn the_pfui_hdgraphic_extended_arm_runs() {
    let s = script();
    s.register_cvars([
        (crate::script::CVAR_WORLD_DETAIL, "1"),
        (crate::script::CVAR_FRILL_DENSITY, "32"),
    ]);
    s.run(
        r#"
        local HookSetWorldDetail = SetWorldDetail
        function SetWorldDetail(arg)
          HookSetWorldDetail((arg > 2 and 2 or arg))
          if arg > 2 then ConsoleExec("frillDensity " .. (arg+1)*16)
          else ConsoleExec("frillDensity 24") end
        end
        local HookGetWorldDetail = GetWorldDetail
        function GetWorldDetail(arg)
          local frill = tonumber(GetCVar("frillDensity"))
          return frill > 48 and frill/16-1 or HookGetWorldDetail()
        end
    "#,
    )
    .unwrap();

    assert_eq!(
        s.eval::<i64>("SetWorldDetail(9); return GetWorldDetail()")
            .unwrap(),
        9,
        "the extended stop round-trips through frillDensity, which is the module's whole point"
    );
    assert_eq!(
        s.cvar(crate::script::CVAR_FRILL_DENSITY).as_deref(),
        Some("160"),
        "(9+1)*16 — inside the reference's own [1, 256], which is why its range is wider than its slider"
    );
    // The low arm falls through to the stock getter.
    assert_eq!(
        s.eval::<i64>("SetWorldDetail(2); return GetWorldDetail()")
            .unwrap(),
        2
    );
}

/// `ShowNameplates 0x489450`, `HideNameplates 0x489460`, `ShowFriendNameplates 0x489470` and
/// `HideFriendNameplates 0x489480` read no argument and return nothing: four 10-byte bodies over
/// two setters, differing only in an `or`/`and` mask.
#[test]
fn the_nameplate_verbs_ignore_their_arguments_and_return_nothing() {
    let s = script();
    s.register_cvars([
        (crate::script::CVAR_NAMEPLATE_ENEMIES, "1"),
        (crate::script::CVAR_NAMEPLATE_FRIENDS, "0"),
    ]);

    let get = |s: &crate::script::UiScript, n: &str| s.cvar(n);
    assert_eq!(
        get(&s, crate::script::CVAR_NAMEPLATE_ENEMIES).as_deref(),
        Some("1")
    );

    s.run("HideNameplates()").unwrap();
    assert_eq!(
        get(&s, crate::script::CVAR_NAMEPLATE_ENEMIES).as_deref(),
        Some("0")
    );
    assert_eq!(
        get(&s, crate::script::CVAR_NAMEPLATE_FRIENDS).as_deref(),
        Some("0"),
        "the friendly bit is a separate setter — hiding enemies must not touch it"
    );
    s.run("ShowFriendNameplates()").unwrap();
    assert_eq!(
        get(&s, crate::script::CVAR_NAMEPLATE_FRIENDS).as_deref(),
        Some("1")
    );
    assert_eq!(
        get(&s, crate::script::CVAR_NAMEPLATE_ENEMIES).as_deref(),
        Some("0"),
        "...and back the other way"
    );

    s.run("ShowNameplates(false)").unwrap();
    assert_eq!(
        get(&s, crate::script::CVAR_NAMEPLATE_ENEMIES).as_deref(),
        Some("1"),
        "the verb IS the value — a falsy argument does not invert it"
    );
    for call in ["HideNameplates(1, 2, 3)", "ShowFriendNameplates({})"] {
        s.run(call)
            .unwrap_or_else(|e| panic!("{call} must not raise: {e}"));
    }

    assert_eq!(
        s.arity("ShowNameplates()").unwrap(),
        0,
        "zero values — observably different from nil for a caller that counts"
    );
}
