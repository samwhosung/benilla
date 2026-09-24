//! **The 1.12 surface is the contract** — enforced here rather than
//! trusted to memory.
//!
//! benilla targets the 1.12.1 API. Anything beyond it is a *listed, justified* exception, and the
//! point of this file is that the list cannot go unknown: every global our VM exposes must either
//! be one 1.12 has, or appear below with a reason. A session that adds an Era-shaped verb has to
//! delete or extend an assertion that says why not — which is exactly what did not happen when
//! 1187 shipped eight Era globals on Era's authority and 1189 had to take them back out.
//!
//! **Why a superset is not free.** Lua branches on presence. An addon writing `if strmatch then`
//! takes a path we cannot honour, and the failure surfaces far from the cause. Extra functions are
//! harmless only if nothing feature-detects, which is not true of this ecosystem.
//!
//! The reference side is `reference/1.12-globals.tsv` — the running 1.12.1 client's own in-world
//! `_G` (see `reference/README.md`). This test reads the `engine` and `lua` rows: the surface a VM
//! is responsible for. It deliberately does **not** assert the converse — that we have everything
//! 1.12 has — because that is a multi-year backlog, not a regression gate;
//! `scripts/api-coverage.sh` is where that number is read.

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

/// Names benilla exposes that 1.12 does not, each with the reason it is allowed to stay.
///
/// **`Benilla*` and `__benilla_*` are covered by prefix**, not enumerated: the first is our host
/// bridge (verbs only our own transcribed FrameXML calls — a paperdoll model's facing, an item's
/// stat block) and the second is the tick's pushed state. Neither is an API-target claim, and
/// neither is reachable by accident from an addon that means to call a WoW function.
fn allowed_beyond_1_12() -> HashSet<&'static str> {
    [
        // ── our Lua runtime is 5.1 where 1.12's is 5.0 ────────────────────────────────────────
        // `_G` is the last of these inherited rather than chosen: 1.12's base library does not
        // export it (an addon reaches the globals with `getfenv(0)`, which is what AceLibrary
        // does), but our own `getglobal`/`setglobal` are written over it. Closing it means
        // rewriting those against the registry first.
        //
        // **This list has been shrinking as the dialect got measured**: `coroutine` left in 1194
        // with the 5.1-only members of `string`/`table`/`math`; `print` and `_VERSION` left in
        // 1197, when the base library's 36-entry table `0x811e28` was found to hold neither.
        // `select` left last, and it is the one that shows what a written reason is worth — it
        // carried two and outlived both. The first ("our transcribed FrameXML uses it in 16 files
        // as the 5.1 spelling of 5.0's implicit `arg` table") was retired by 1751's migration and
        // caught by 2142's census at *zero* sites. The second, written in its place, was that
        // removing it meant giving up the 5.1 varargs it comes with — which 2101 had already given
        // up, three days earlier, in the parser. A reason nobody re-derives is a reason with an
        // expiry date; this list's job is to make the expiry visible, and twice it did not.
        // The list only ever covered globals — the members needed `dump_globals --members` before
        // anyone could see them at all.
        "_G",
        // ── WoW API past 1.12 — 1188 phase 5's list, and the reason that phase exists ─────────
        // **The reason written here was "every one of these is used by our own transcribed
        // FrameXML today", and as of 2026-09-09 that is true of exactly ONE of the sixteen**
        // (`SubmitChatInput`, in the dev-only `ScriptLogFrame.xml`) — a corpus census measured it
        // and this file's own claim was checked against `assets/ui`'s seven remaining files, name
        // by name, before this comment was rewritten. 1751's migration retired the
        // windows that used the rest; the list outlived its justification, which is exactly how
        // 2118 found the modifier keys still answering booleans years after 0068's reason for it
        // had gone.
        //
        // **What they are NOT is load-bearing for the addon corpus, and 2143 corrects 2142 for
        // saying so.** The claim written here was that the vanilla ecosystem assumes several of
        // these exist — `pfUI/modules/loot.lua` calling `wipe` 25 times and defining it nowhere,
        // so ours "makes that module run at all". That is wrong twice over, and the error is
        // instructive: **a "1.12 addon" in that corpus is usually a MULTI-CLIENT addon**. pfUI
        // ships one codebase for three clients and gates per module —
        // `pfUI:RegisterModule("loot", "vanilla:tbc", …)`, matched by
        // `strfind(version, pfUI.expansion)` at `pfUI.lua:250` — so a TBC-era habit sits in code
        // the vanilla client reaches. The `wipe` calls are in `RequestRolls`,
        // `BuildSpecialRecipientsMenu` and `ClearRolls`: the master-looter's roll helper, not the
        // loot window, and that path throws on a real 1.12 client too. And `strsplit` was worse —
        // pfUI has its own `pfUI.api.strsplit` (`api/api.lua:16`) and ShaguTweaks its own
        // `ShaguTweaks.strsplit`; the bare calls are inside the vendored `libs/libpredict.lua`,
        // which has TBC branches of its own.
        //
        // So our superset was HIDING those latent nil-calls, never enabling anything. The one
        // real interaction left is the other direction: `AtlasLoot/Core/AtlasLoot.lua:2119`/`:2148`
        // define GLOBAL `strsplit`/`strtrim` unconditionally, with incompatible pattern-based
        // two-return semantics, and clobber ours for everything loaded after them — so ours is not
        // authoritative at runtime either.
        //
        // Resolving each means replacing it with its 1.12 equivalent (`UnitPower` → `UnitMana`,
        // which 1.12 has and we do not), deleting it, or recording why it stays.
        //
        // Three left with 1751's gossip work: `GetNumGossipQuests`, `GetGossipQuestInfo` and
        // `SelectGossipQuest` were benilla's own single-list shape over the gossip packet's quest
        // rows, and 1.12 has none of them — it publishes `GetGossipAvailableQuests` /
        // `GetGossipActiveQuests` and a select for each, which is what this client answers now.
        // The window that needed them is ours until it migrates; it calls the reference's four.
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
///
/// The failure message is the point: it names what is new and tells you the two ways out, so the
/// next session cannot resolve it by deleting a bare assertion.
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
         1.12 is the target (decision 1188). Either give it its 1.12 spelling, or add it to \
         `allowed_beyond_1_12` in this file WITH the reason it has to stay — an unexplained \
         superset is what 1189 had to roll back.",
        unlisted.len(),
        unlisted.join(" ")
    );
}

/// The exception list is exact — no entry outlives the global it excuses.
///
/// Without this, a removed global leaves its excuse behind, and the list slowly stops describing
/// the code. That is how a "listed, justified exception" decays into residue nobody can explain.
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

    // An entry that 1.12 turns out to have is a different bug: the excuse is wrong, not stale.
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

/// **`Texture:GetTexture()` — the three contract details that a plausible implementation gets
/// silently wrong.** Verified at `0x79ba70`/`0x79baf0`/`0x835708`.
///
/// Four corpus addons reach it: `AtlasQuest.lua:228` is `AQATLASMAP = AtlasMap:GetTexture()` and
/// `FuBarPlugin-2.0.lua:343` is `return self.iconFrame:GetTexture()`, each behind two addons.
///
/// The colour case is the one worth a test of its own. `SetTexture(r,g,b)` synthesizes an 8x8 solid
/// and the getter reports the literal name `"Solid Texture"` — so an addon's `if not tex then`
/// guard passes straight through. Returning nil there would read as tidier and would be wrong in
/// exactly the direction callers test for.
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

    // Never set: nil, and exactly one value.
    assert_eq!(
        s.eval::<Option<String>>("return GTFTex:GetTexture()")
            .unwrap(),
        None
    );
    assert_eq!(
        // `.unwrap()`, not the `.unwrap_or(1)` this carried while it was spelled with `select`:
        // that default made a raise indistinguishable from the answer being asserted, which is the
        // one thing an arity check must not do (2171).
        s.arity("GTFTex:GetTexture()").unwrap(),
        1,
        "exactly one return value"
    );

    // A path with no extension comes back unchanged.
    s.run(r#"GTFTex:SetTexture("Interface\\Icons\\Spell_Fire_FlameBolt")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GTFTex:GetTexture()").unwrap(),
        "Interface\\Icons\\Spell_Fire_FlameBolt"
    );

    // ...and one WITH an extension is stripped at the last `.` — the loader appends it, the getter
    // takes it back off.
    s.run(r#"GTFTex:SetTexture("Interface\\Icons\\Foo.blp")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GTFTex:GetTexture()").unwrap(),
        "Interface\\Icons\\Foo"
    );

    // The colour form: the literal name, NOT nil.
    s.run("GTFFill:SetTexture(1, 0, 0, 1)").unwrap();
    assert_eq!(
        s.eval::<String>("return GTFFill:GetTexture()").unwrap(),
        "Solid Texture",
        "a colour-filled region reports a NAME — `if not tex then` must not fire"
    );

    // And a plain SetTexture makes it ordinary again.
    s.run(r#"GTFFill:SetTexture("Interface\\Buttons\\UI-Quickslot2")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GTFFill:GetTexture()").unwrap(),
        "Interface\\Buttons\\UI-Quickslot2"
    );
}

/// **The shadow accessors on a REGION, and `GetShadowColor`'s four values.**
///
/// They existed on the font-object table; a FontString from `CreateFontString` had none, which is
/// exactly where the corpus calls them. `FuBar_NavigatorFu/NavigatorFu.lua:31` is
/// `coordText:SetShadowColor(GameFontNormal:GetShadowColor())` — getter on a font object, setter on
/// a fresh region, in one line — and `KLHThreatMeter/.../KTM_Gui.lua:404` is
/// `fontstring:SetShadowColor(0,0,0,0.3)`.
///
/// **Four values, not three** (`0x79dd2f`, `mov eax,0x4`). Three is
/// the plausible wrong answer and it silently drops the alpha NavigatorFu round-trips: the whole
/// point of its line is that whatever `GameFontNormal` carries arrives intact.
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

    // NavigatorFu's line, in shape: a font object's shadow piped straight into a region's.
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

    // Either half may be set first: setting only the offset must not blank the colour.
    s.run("SHFText:SetShadowOffset(2, -2)").unwrap();
    assert_eq!(
        s.eval::<f32>("local _,_,_,a = SHFText:GetShadowColor() return a")
            .unwrap(),
        0.3,
        "setting the offset alone keeps the colour"
    );
}

/// **`Region:SetParent(frame)` — it exists on a Texture/FontString, and we were the ones missing
/// it.** `SetParent` is in the Region method table (`0x7a1550`) and both leaf lookups fall back to
/// Region's (`0x79c650` / `0x79ee50`), so `FuBar_FuXPFu.lua:210`'s
/// `self.Spark:SetParent(self.XPBar)` — a texture from `XPBar:CreateTexture` — is a working line on
/// the real client.
///
/// The two traps pinned here are the ones a plausible implementation gets wrong in opposite
/// directions. **A non-Frame argument RAISES** (`IsA(FrameTag)` at `0x7a16ea`, message
/// `"…Wrong parent object type, expected frame"`) rather than no-opping; and **anchors are NOT
/// touched** by the re-link, which moves draw-layer/region-list membership only — so a re-parented
/// texture keeps resolving against whatever `SetPoint` named, and FuXPFu's re-point on the next
/// line is the correct pattern rather than a workaround.
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

    // FuXPFu's line, and it returns nothing at all.
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

    // …and the re-link is real, not merely recorded: the spark now draws with B. Hiding B takes it
    // off the screen; hiding A (which it is still anchored to) does not.
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

    // A non-Frame argument raises. A texture is the case the reference names, and it is the one an
    // addon actually hits (a mixed table of frames and textures walked in a loop).
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
    // A MISSING argument is not the nil form: TNONE never reaches the nil branch.
    assert!(s.run("Spark:SetParent()").is_err(), "no argument raises");

    // The nil form detaches without error: orphaned and unrendered, not destroyed — a later
    // re-parent brings the same region back.
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

/// **`Button:SetFont(file, height [, flags])` — it exists, it returns NOTHING, and it never
/// touches the label.** `0x780880`.
/// `_LazyPig/LazyPigMenu.lua:214` calls it straight on a `CreateFrame("Button", …)` and is blocked
/// on it today.
///
/// Three traps, each pinned here because each is a plausible implementation's silent divergence.
/// **Zero return values**: the shared impl pushes `1`/nil (a real font-load probe on a Font object)
/// and Button *discards* it (`xor eax,eax`), so answering `true` would hand an addon a probe the
/// client does not have. **`GetFont` returns three values**, off the normal embedded font — which
/// resolves through what that state inherits when nothing set it locally. And **a bare Button with
/// no `<ButtonText>` is a silent no-op**: `SetFont`/`GetFont` never dereference the FontString
/// pointer `+0x338`, so there is no error and — the observable half — **no lazy label creation**,
/// which `GetFontString()` would report.
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

    // _LazyPig's line, on a Button that has no label at all.
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
    // Still three when nothing has ever set a font — path and height are nil, not absent.
    assert_eq!(s.arity("SFLabelled:GetFont()").unwrap(), 3);
    assert!(s
        .eval::<bool>("local f = SFLabelled:GetFont() return f == nil")
        .unwrap());

    // The flags argument is normalized to its OUTLINETYPE token, like every other GetFont.
    s.run(r#"SFBare:SetFont("Fonts\\SKURRI.TTF", 12, "THICKOUTLINE")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("local _, _, flags = SFBare:GetFont() return flags")
            .unwrap(),
        "THICKOUTLINE"
    );
    // Both arguments are required (`lua_isstring` + `lua_isnumber`, else the usage error).
    assert!(s.run(r#"SFBare:SetFont("Fonts\\SKURRI.TTF")"#).is_err());

    // Unset locally, GetFont reads through what the NORMAL state inherits — how a
    // GameMenuButtonTemplate button answers its template's face before anything calls SetFont.
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

    // …and a local SetFont outranks that inherited object on the label's PAINT, not only in the
    // getter — the whole point of the call `_LazyPig` makes.
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

/// **`SetNonSpaceWrap` / `CanNonSpaceWrap`** — FontString only (`0x79e9f0`/`0x79ead0`).
/// `oRA2/Leader/Item.lua:561` is `f.textname:SetNonSpaceWrap(false)`, reached
/// by two addons.
///
/// Two contract details, both easy to get wrong and both pinned here: the getter is
/// **`CanNonSpaceWrap`**, not `GetNonSpaceWrap`, and it answers **`1` or nil** rather than a
/// boolean — 1.12 predates that convention and an addon may compare against 1. And a **no-argument
/// call ENABLES** it; the default is on, so it is not a query.
#[test]
fn non_space_wrap_defaults_on_and_answers_one_or_nil() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(r#"f = CreateFrame("Frame", "NSF") fs = f:CreateFontString("NSFText", "ARTWORK")"#)
        .unwrap();

    // Default is ON, and the answer is the NUMBER 1 — not `true`.
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

    // oRA2's line.
    s.run("NSFText:SetNonSpaceWrap(false)").unwrap();
    assert_eq!(
        s.eval::<Option<i64>>("return NSFText:CanNonSpaceWrap()")
            .unwrap(),
        None,
        "off answers nil, not 0 and not false"
    );

    // A no-argument call ENABLES rather than querying.
    s.run("NSFText:SetNonSpaceWrap()").unwrap();
    assert_eq!(
        s.eval::<Option<i64>>("return NSFText:CanNonSpaceWrap()")
            .unwrap(),
        Some(1),
        "a bare SetNonSpaceWrap() turns it ON"
    );
}

/// **The EditBox font block — the first sixteen entries of its own registrar table, and the largest
/// single gap the per-kind widget-method census found** (whose ranking opens
/// `63  EditBox:SetFontObject   (on Texture, FontString)`).
///
/// `EditBox`'s table is `.data 0x87bb68`, **48 entries** — the count read from the `mov edx,0x30`
/// at the registering site `0x799ab5`, never from a run-length scan. There is no `FontInstance`
/// class in the 1.12 Lua chain (the shared C++ impl `0x79f210`): each of the six text-bearing types
/// re-declares the block in its own flat table, so membership is a per-table fact and **a name we
/// add that the table does not carry is exactly as wrong as one we miss**.
///
/// The two halves pinned here are therefore both directions. Present: the fourteen we wire.
/// Absent: `SetSpacing`/`GetSpacing`, which the table *does* carry (#10–#11) and we deliberately
/// withhold because nothing here models line spacing — a stored-but-undrawn setter is the silent
/// divergence of 1203/1205/1211, while a nil-value call names itself, and corpus demand is zero.
/// Absent too: the names that belong to *other* tables and must not leak onto an EditBox through
/// ours — `CopyFontObject` (Font object only), `SetNonSpaceWrap`/`CanNonSpaceWrap` and
/// `SetTextHeight`/`GetStringWidth` (FontString only).
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
        // On EditBox's real table, withheld on purpose (nothing models spacing).
        "SetSpacing",
        "GetSpacing",
        // Not on EditBox's table at all — adding one is the same class of error as missing one.
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

/// **Every EditBox font binding is a type-guard shim that returns whatever the shared implementation
/// returned**, so the arities below are the shared ones verbatim — verified at the bytes: each
/// binding ends `call <shared>` then `pop edi; pop esi; pop ebx; ret`, with **no `mov eax,N` and no
/// `xor eax,eax`** after the call.
///
/// The trap is `SetFont`, and it points the opposite way to the one already pinned for Button.
/// `Button:SetFont` (`0x780880`) ends `xor eax,eax` and returns **nothing** —
/// [`button_set_font_returns_nothing_and_is_a_no_op_without_a_label`]. `EditBox:SetFont`
/// (`0x797210` → `0x79f210`) does not discard, so the shared `lua_pushnumber(1.0)` (`0x79f345`) /
/// `lua_pushnil` (`0x79f361`) passes straight through: **one value, the number `1`, not `true` and
/// not zero values**. A probe written `if not eb:SetFont(f, s) then` needs the nil half; a probe
/// written `== 1` needs the number half.
///
/// `GetShadowColor` returns **four** values (`0x79f9b3 mov eax,0x4`); three is the plausible wrong
/// answer and silently drops the alpha.
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

    // The mutators push nothing at all.
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

    // The getters' exact widths.
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

    // SetFont: ONE value, and it is the number 1 — not `true`, and not nothing (the Button shape).
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
    // An unloadable (empty) path is the FALSEY answer, not an error — `!OmniCC`'s probe idiom.
    assert!(s
        .eval::<bool>(r#"return EBShape:SetFont("", 12) == nil"#)
        .unwrap());
    // Both arguments are gated (`lua_isstring` + `lua_isnumber`, else the usage error).
    assert!(s.run(r#"EBShape:SetFont("Fonts\\SKURRI.TTF")"#).is_err());

    // The shadow round-trips all four channels — three would drop the alpha.
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
    // GetFontObject answers the OBJECT, never its name — the corpus indexes the result immediately.
    assert_eq!(
        s.eval::<String>("return type(EBShape:GetFontObject())")
            .unwrap(),
        "table"
    );
}

/// **The real `Dewdrop-2.0` shape, which is the whole reason this landed.**
///
/// `Dewdrop-2.0.lua:1673-1675` is verbatim:
/// ```lua
/// local editBox = CreateFrame("EditBox", nil, editBoxFrame)
/// editBoxFrame.editBox = editBox
/// editBox:SetFontObject(ChatFontNormal)
/// ```
/// — an **anonymous** EditBox parented to a frame, then `SetFontObject` on it two lines later. That
/// file is vendored into **63 addon folders** of the 218-addon corpus (65 copies: `FuBar` plus ~50
/// FuBar plugins, `BigWigs`, `AtlasLoot`, `oRA2`, `SnaFu`, …), so the census's `63` is **one library
/// replicated**, not 63 independent addons.
///
/// `AceGUIWidget-Slider.lua:204-210` is the same shape with `SetJustifyH("CENTER")` on the end.
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

    // The link is live and readable back as the object.
    assert!(s
        .eval::<bool>("return DewdropEBFrame.editBox:GetFontObject() == ChatFontNormal")
        .unwrap());
    // …and the object's paint really reached the box's implicit FontString (the `[this+0x324]` the
    // reference's shim hands the shared implementation).
    assert_eq!(
        s.eval::<(String, f32)>("local p, h = DewdropEBFrame.editBox:GetFont() return p, h")
            .unwrap(),
        ("Fonts\\ARIALN.TTF".to_string(), 11.0)
    );
    let (r, g, b, _) = s
        .eval::<(f32, f32, f32, f32)>("return DewdropEBFrame.editBox:GetTextColor()")
        .unwrap();
    assert_eq!((r, g, b), (0.9, 0.9, 0.9));

    // The AceGUI slider's tail, on the same box.
    s.run(r#"DewdropEBFrame.editBox:SetJustifyH("CENTER")"#)
        .unwrap();
}

/// **`SetJustifyH`/`SetJustifyV` mask to their own axis, and a token from the other axis silently
/// clears rather than erroring.** The enum is `.rdata 0x811ad0`, `{bits, token}`: `LEFT` 0x01,
/// `CENTER` 0x02, `RIGHT` 0x04, `TOP` 0x08, `MIDDLE` 0x10, `BOTTOM` 0x20, stored in one dword with
/// bits 0–2 horizontal and 3–5 vertical.
///
/// Two traps, both of which our older FontString/Font copies of these verbs get wrong by coercing
/// anything unrecognised to CENTER/MIDDLE:
///  · **an unknown token RAISES** `Usage: %s:SetJustifyH("justify")` (`0x87c77c`);
///  · **a valid token from the wrong axis parses and then masks to nothing** — `SetJustifyH("TOP")`
///    yields 0x08 and `0x08 & 0x07 == 0`, so justifyH is CLEARED and `GetJustifyH()` answers the
///    literal `"UNKNOWN"` (`0x6f1a00`, `.data 0x838044`). No error is raised either way.
#[test]
fn editbox_justify_masks_to_its_axis_and_answers_unknown() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(r#"EB = CreateFrame("EditBox", "EBJustify")"#)
        .unwrap();

    // **LEFT, not the generic font default CENTER.** The `CSimpleFont` ctor really does default to
    // `0x212` (CENTER | MIDDLE | 0x200), but the EditBox ctor overrides the horizontal axis right
    // after linking its font instance (`0x779bcd … and eax,~6; or eax,1;` stored at `0x779be4`), so
    // a fresh box starts at `0x211`. Taking the generic default is the plausible wrong answer.
    assert_eq!(
        s.eval::<String>("return EBJustify:GetJustifyH()").unwrap(),
        "LEFT"
    );
    assert_eq!(
        s.eval::<String>("return EBJustify:GetJustifyV()").unwrap(),
        "MIDDLE"
    );

    // Each axis is replaced independently — setting V leaves H alone.
    s.run(r#"EBJustify:SetJustifyH("LEFT") EBJustify:SetJustifyV("BOTTOM")"#)
        .unwrap();
    assert_eq!(
        s.eval::<(String, String)>("return EBJustify:GetJustifyH(), EBJustify:GetJustifyV()")
            .unwrap(),
        ("LEFT".to_string(), "BOTTOM".to_string())
    );

    // The cross-axis trap: "TOP" parses, masks to 0, and CLEARS justifyH.
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

    // An unrecognised token is the usage error, never a silent fallback to CENTER.
    assert!(s.run(r#"EBJustify:SetJustifyH("SIDEWAYS")"#).is_err());
}

/// **`EditBox:SetJustifyV` echoes through its getter and never reaches the pixels — on the real
/// client too, permanently and by construction.** `CSimpleFontString+0x124` is a per-bit *inherit*
/// mask over the rendered justify `+0x120`, and `SetMultiLine 0x77a4a0` clears the whole vertical
/// group `0x38` from it on **both** legs while writing the V bits locally — multi-line → TOP,
/// single-line → MIDDLE. The
/// EditBox ctor calls `SetMultiLine` unconditionally at birth (`0x779c2f`, with the ctor's zero
/// register), and a census of all 256 `+0x124` operands image-wide found every `CSimpleFontString`
/// writer to be an AND: **nothing ever ORs an inherit bit back in.** So the value written by
/// `SetJustifyV` is masked out at `0x77086e`, while `GetJustifyV` reads the font *instance*
/// (`0x79fd73`) and answers it faithfully.
///
/// This pins the shape rather than the accident: a future change that "fixes" the getter by wiring
/// V justify through to the region would match neither the client nor our own draw law.
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

    // SetJustifyV("TOP") is echoed by the getter…
    s.run(r#"EBJV:SetJustifyV("TOP")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return EBJV:GetJustifyV()").unwrap(),
        "TOP",
        "the getter reads the font instance and echoes faithfully"
    );
    // …and does NOT move the pixels. This is the disagreement, and it is faithful.
    assert_eq!(
        drawn_v(&s),
        crate::script::JustifyV::Middle,
        "SetJustifyV is masked out of the rendered justify — multiLine alone decides it"
    );

    // multiLine is what actually moves it, on the `0x77a509` leg.
    s.run("EBJV:SetMultiLine(true)").unwrap();
    assert_eq!(drawn_v(&s), crate::script::JustifyV::Top);
    // …and the getter still echoes whatever was last set, unaffected.
    assert_eq!(
        s.eval::<String>("return EBJV:GetJustifyV()").unwrap(),
        "TOP"
    );
}

/// **`Texture:SetDesaturated(flag)` answers `shaderSupported`, and since 1327 ours says yes.**
///
/// `0x79c1e0`. The reference's own `ItemButtonTemplate.lua:69` is
/// `local shaderSupported = icon:SetDesaturated(desaturated)`, and lines 70-78 fall back to a 0.5
/// grey vertex tint when that answer is falsy — 1.12 shipped on cards without the shader, so the
/// verb reporting "no" is a real machine's answer, not a stub.
///
/// benilla answered nil for exactly as long as nothing greyed. Decision 1327 gave the renderer the
/// luminance fold, so the honest answer flipped: a caller that asks for grey now gets grey, and
/// keeps its own tint instead of having it overwritten by the 0.5 fallback. The test pins the
/// return SHAPE, because the return is what callers branch on — `1`, not `true`, because the C
/// answer is `1|nil` and every reference consumer writes `not shaderSupported`.
///
/// 98 of the 109 draw-then-raise addons in the corpus die on this one verb, via
/// `FuBar_Panel.lua:43` → Dewdrop `AddLine` → `button.arrow:SetDesaturated(true)`, unguarded.
#[test]
fn set_desaturated_reports_shader_support_and_does_not_raise() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(r#"f = CreateFrame("Frame", "DsF") tex = f:CreateTexture("DsTex", "ARTWORK")"#)
        .unwrap();

    // Dewdrop's exact call — the one 98 addons reach. It must not raise.
    s.run("DsTex:SetDesaturated(true)").unwrap();

    // ...and it answers truthy, so the reference's shader arm is taken.
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

    // The reference's own consumer, transcribed: a truthy answer keeps the CALLER's tint, and the
    // 0.5 fallback never fires. This is the half B162 turned on — the talent tree asks for
    // `(1, 0.65, 0.65, 0.65)` and must get 0.65, greyscale, not a flat 0.5 colour multiply.
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

    // Both spellings of "off" are off — nil and false.
    s.run("DsTex:SetDesaturated(nil) DsTex:SetDesaturated(false)")
        .unwrap();
}

/// `UnitCreatureType(unit)` — `0x51a280`'s three-stage resolver, of which we model stages 2 and 3.
///
/// The load-bearing assertion is the **player** one. `0x605570` falls through the (never populated)
/// creature record to a `ChrRaces.dbc` col-9 lookup that is **7 for all nine shipped races**, and
/// `CreatureType[7]` is `"Humanoid"` — so a player answers a word, not nil. Reading only the
/// creature record, which is what our data alone suggested, would have been wrong for every
/// `UnitCreatureType("player")` call in the corpus.
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

    // Stage 2: a creature's cached record wins.
    assert_eq!(
        s.eval::<String>(r#"return UnitCreatureType("target")"#)
            .unwrap(),
        "Beast"
    );
    // Stage 3: a player has no record and falls through to race -> Humanoid.
    assert_eq!(
        s.eval::<String>(r#"return UnitCreatureType("player")"#)
            .unwrap(),
        "Humanoid"
    );
    // An unresolved TOKEN is nil — not a raise.
    assert!(s
        .eval::<bool>(r#"return UnitCreatureType("party4") == nil"#)
        .unwrap());
    // A non-string ARGUMENT is a raise, which is a different failure from the nil above:
    // `lua_isstring` gates it and `luaL_error` longjmps, so a missing arg abandons the statement.
    let err = s
        .run("UnitCreatureType()")
        .expect_err("a missing arg must raise");
    assert!(
        format!("{err}").contains("Usage: UnitCreatureType"),
        "got {err}"
    );
}

/// `GetInventorySlotInfo(slotName)` — `0x4c81b0`, three returns and a **case-insensitive**,
/// full-string name match.
///
/// The case-insensitivity is the whole point: two 1.12-era corpus addons died at session start on
/// case variants of real names — `FuBar_AmmoFu` passes `"ammoSlot"`, `FuBar_PoisonFu`
/// `"MAINHANDSLOT"` — and both worked on the real client, because `0x4c8215` reaches the CRT
/// `_strnicmp`, which folds both operands.
///
/// The other two assertions are things a plausible implementation gets wrong and nothing notices:
/// the third return is the **number 1**, only for `RangedSlot`, never a boolean; and a miss
/// **raises** with the reference's own message, which carries no `Usage:` prefix and does not
/// interpolate the offending name.
#[test]
fn get_inventory_slot_info_folds_case_and_flags_only_the_ranged_slot() {
    let s = script();

    // The exact spelling, and the two case variants the corpus actually ships.
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

    // Three values, and the second is the empty-slot background art the paper-doll buttons use.
    assert_eq!(s.arity("GetInventorySlotInfo('HeadSlot')").unwrap(), 3);
    let (id, art) = s
        .eval::<(i64, String)>("return GetInventorySlotInfo('HeadSlot')")
        .unwrap();
    assert_eq!(id, 1);
    // The DBC string verbatim: LOWERCASE directory and the `.blp` extension. The binding pushes
    // `[esi+4]` with no normalisation, so a caller that keys a table by this sees these bytes.
    assert_eq!(art, "interface\\paperdoll\\UI-PaperDoll-Slot-Head.blp");

    // checkRelic: the NUMBER 1 for the ranged slot alone, nil everywhere else — not `false`, which
    // is falsey like nil but the wrong type for a caller that compares it against 1.
    assert!(s
        .eval::<bool>("local _,_,r = GetInventorySlotInfo('RangedSlot') return r == 1")
        .unwrap());
    assert!(s
        .eval::<bool>("local _,_,r = GetInventorySlotInfo('HeadSlot') return r == nil")
        .unwrap());

    // The twelve rows this table was short of — `Bag1`..`Bag12` at SlotNumbers 64..75, of which
    // 64..69 is the bank-bag band. They share ONE string offset with Bag0Slot..Bag3Slot, so all
    // sixteen bag rows answer the same art.
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
    // ...and `Bag1` (64) is NOT `Bag1Slot` (21): different names, different ids, both real rows.
    assert_eq!(
        s.eval::<i64>("return GetInventorySlotInfo('Bag1Slot')")
            .unwrap(),
        21
    );

    // A miss raises — there is no nil path — with the reference's own string.
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

/// **The whole Region method map, on both leaves that chain to it.**
///
/// `SetParent` above landed as one name because one addon line named it. That is how this table has
/// always grown, and it is why `GetParent` — its own getter — was still absent while `SetParent`
/// worked. The reference exposes it as a SET, not as names: FontString's lookup `0x79ee20` chains
/// its own map `0xcf5400` to the Region map `0xcf54b4`, whose 19 entries are
///
/// ```text
/// GetObjectType IsObjectType GetName GetParent SetParent GetCenter GetLeft GetRight GetTop
/// GetBottom GetWidth SetWidth GetHeight SetHeight GetNumPoints GetPoint SetPoint SetAllPoints
/// ClearAllPoints
/// ```
///
/// (the reference's Font lookup `0x7a1100` has no fallback — a `<Font>` object does NOT chain and
/// so has none of these. Texture reaches the identical map through its own leaf lookup, which is
/// why both are asserted here.)
///
/// So this asserts membership rather than behaviour: each name is *present and callable* on a
/// Texture and on a FontString. Behaviour belongs in the focused tests around it — what is pinned
/// here is that the set cannot quietly lose a member again, which is the failure `GetParent` was.
#[test]
fn every_region_map_method_is_callable_on_a_texture_and_a_fontstring() {
    /// All 19. `GetObjectType`/`IsObjectType` were held out of this list when 1244 landed the other
    /// four — dispatched rather than guessed — and joined it once `0x7a11d0`/`0x7a1290` were
    /// confirmed on the Region map. The list is the whole map again.
    // The one list, shared with the title region's narrower table (`script::REGION_MAP_METHODS`)
    // so the two can never disagree about what "the Region map" is.
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

/// **The four Region-map readers, on the cases a frame-shaped copy gets wrong.**
///
/// `TheoryCraftUI.lua:720` is the line that found them: `buttontext:GetParent():GetID()`, where
/// `buttontext` is a FontString — a working line on the real client, and `attempt to call method
/// 'GetParent' (a nil value)` here, every session.
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

    // GetParent is the OWNER frame — the identity TheoryCraft then calls :GetID() on.
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

    // GetCenter agrees with the edge readers BY CONSTRUCTION — the invariant that forbids scaling
    // one and not the others.
    let (cx, cy): (f64, f64) = s.eval("return RRPlate:GetCenter()").unwrap();
    let (l, r, t, b): (f64, f64, f64, f64) = s
        .eval("return RRPlate:GetLeft(), RRPlate:GetRight(), RRPlate:GetTop(), RRPlate:GetBottom()")
        .unwrap();
    assert_eq!((cx, cy), ((l + r) * 0.5, (t + b) * 0.5));

    // GetNumPoints counts what SetPoint wrote, and ClearAllPoints takes it back to 0.
    assert_eq!(s.eval::<i64>("return RRPlate:GetNumPoints()").unwrap(), 1);
    assert_eq!(
        s.eval::<i64>("return RRLabel:GetNumPoints()").unwrap(),
        1,
        "the sibling-anchored label carries its one point"
    );

    // GetPoint's relativeTo must come back as the SIBLING REGION, not a frame wrapper onto the same
    // id — the one place regions genuinely differ from frames, since both share an id space.
    let (p, rp, x, y): (String, String, f64, f64) = s
        .eval("local p, _, rp, x, y = RRLabel:GetPoint(1) return p, rp, x, y")
        .unwrap();
    assert_eq!((p.as_str(), rp.as_str(), x, y), ("LEFT", "RIGHT", 4.0, 0.0));
    assert!(
        s.eval::<bool>("local _, rel = RRLabel:GetPoint(1) return rel == RRPlate")
            .unwrap(),
        "relativeTo is the sibling REGION handle itself"
    );
    // Out of range is five nils, like the frame twin.
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

/// **`GetObjectType`/`IsObjectType` — every detail confirmed at the bytes, asserted.**
///
/// 1244 shipped four Region-map members and left these two out rather than guess them, because each
/// of the four traps below is a coin-flip a reimplementation loses (1203/1205/1211 are three
/// records of losing it). This test turns that reading into a gate, so the next person to touch
/// these has to disagree with the binary rather than with me.
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

    // The leaf names, one value each (`lua_pushstring`, no arity check on extra args).
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

    // The chain is TWO deep and stops there.
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
        // **The invented root.** 1.12.1 has no LayoutFrame/ScriptObject/Object type at all — those
        // strings live only in __FILE__ paths and allocator tags. Knowing later clients is exactly
        // what would put them here.
        for absent in ["LayoutFrame", "ScriptObject", "Object", "Frame", "Font"] {
            assert!(
                s.eval::<Option<i64>>(&format!("return {obj}:IsObjectType('{absent}')"))
                    .unwrap()
                    .is_none(),
                "{obj}:IsObjectType('{absent}') must be nil — 1.12 has no such type in the chain"
            );
        }
    }
    // …and the two leaves are not each other.
    assert!(s
        .eval::<Option<i64>>("return TITex:IsObjectType('FontString')")
        .unwrap()
        .is_none());

    // Case-INSENSITIVE and WHOLE-string (SStrCmpI folds both operands; the compare stops at the
    // first NUL, so no prefix or substring match either).
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

    // A hit is the NUMBER 1, never a boolean; a miss is nil. Both paths push exactly one value.
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

    // A NUMBER is accepted (lua_isstring takes tags 4 and 3) and stringified in place — it simply
    // never matches, because no type name is numeric.
    assert!(
        s.eval::<Option<i64>>("return TITex:IsObjectType(5)")
            .unwrap()
            .is_none(),
        "a number argument is accepted and quietly answers nil, NOT a raise"
    );

    // Everything else RAISES with the reference's own Usage text, naming the region — or
    // `<unnamed>` for one declared anonymously.
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

/// **`GetFrameType`/`IsFrameType` — 1.12's own names for the pair above, on the FRAME side**.
///
/// The client registers type identity twice: `CScriptRegion` publishes
/// `GetObjectType`/`IsObjectType` (the test above), and `CSimpleFrameScript.cpp` publishes
/// `GetFrameType 0x773640` / `IsFrameType 0x773700`. Later clients kept the first pair and dropped
/// the second; we had shipped only the first, which is the inverse of 1189's usual failure — a
/// real 1.12 name we were *missing*, not an invented one we offered.
///
/// It cost the world map. Cartographer 2.02's `LookNFeel.lua:368` finds the map's player arrow
/// with `v:GetFrameType() == "Model" and not v:GetName()`; with the verb nil that raised inside
/// `OnEnable`, AceAddon swallowed it, and every map open then put
/// `LookNFeel.lua:737: attempt to index field 'playerModel'` on screen.
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

    // `GetFrameType` reads the same per-class type-name slot `GetObjectType` does (`[edx+0x1c]`),
    // so the two agree by construction — one value, extra arguments ignored.
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

    // The exact line Cartographer walks the world map with.
    assert!(
        s.eval::<bool>(r#"return FTModel:GetFrameType() == "Model" and not FTModel:GetName()"#)
            .unwrap(),
        "an anonymous Model must answer its type AND a nil name — LookNFeel.lua:368"
    );

    // `IsFrameType` walks the same chain: the leaf, every base, the number 1 on a hit and nil on
    // a miss, case-insensitive and whole-string.
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

    // A REGION does not get the frame-side pair — the two registrars are distinct, and offering
    // both names on everything would be the superset 1189 forbids.
    assert!(s.eval::<bool>("return FTTex.GetFrameType == nil").unwrap());
    assert!(s.eval::<bool>("return FTTex.IsFrameType == nil").unwrap());
}

/// **The frame side of the type identity, and the three chains a guess gets wrong.**
///
/// `_Nameplates.lua` is why this half exists: it asks `Region:GetObjectType()` (the region twin)
/// AND `Nameplate:GetObjectType() ~= "Button"` / `Frame:GetObjectType() == "StatusBar"` in the same
/// file. The chains come from the reference's own 23-class roster, which is a hardcoded
/// straight-line list per class in the binary rather than a runtime parent walk.
#[test]
fn a_frames_type_chain_matches_the_roster() {
    let s = crate::script::UiScript::new().unwrap();
    // (CreateFrame kind, GetObjectType string, full chain)
    let cases: &[(&str, &str, &[&str])] = &[
        ("Frame", "Frame", &["Frame", "Region"]),
        ("Button", "Button", &["Button", "Frame", "Region"]),
        // Depth 4 — the longest chain we can build.
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
        // **NOT via MessageFrame**, despite the name (roster `0x787940`).
        (
            "ScrollingMessageFrame",
            "ScrollingMessageFrame",
            &["ScrollingMessageFrame", "Frame", "Region"],
        ),
        // **Capital HTML** — the enum variant is `SimpleHtml`, and `GetObjectType` is compared with
        // `==` by addons, so a variant-derived string would be silently wrong here.
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
        // `PlayerModel` derives from `Model`, which is why `SetUnit` on a portrait pane finds
        // `SetCamera` too (`0x505830`/`0x5057c0`) — and `DressUpModel`
        // derives from it in turn: the roster's maximum depth, 5 (1969; `TabardModel` is the
        // other depth-5 chain — built by 1977).
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
        // Every OTHER leaf in the roster must be absent from this chain — the assertion that
        // catches a chain built by "sounds related".
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

    // The frame verbs share the region twin's contract: number 1 / nil, one value, and a raise
    // naming the frame.
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

/// **Every region method belongs to a leaf — no name may be installed and invisible to both.**
///
/// The split (`script::region`) copies names out of the full table into a Texture leaf and a
/// FontString leaf. A name in neither list is still installed, still costs a closure, and is
/// reachable from NOTHING — the silent half of a wrong partition.
///
/// This is not hypothetical: the partition was first built from a grep, and that grep missed
/// `SetGradient`/`SetGradientAlpha` because they are installed from a LOOP rather than a literal
/// `m.set("…")`. One test caught one of them; this gate catches the whole class, and it reads the
/// table the VM actually holds rather than the source that builds it.
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

/// **The Environment Detail pair — `SetWorldDetail 0x488dd0` / `GetWorldDetail 0x488d70`.**
///
/// Every assertion below is a byte read out of `WoW.exe`, not a shape chosen here; decision 2163
/// carries the carve and `script::cvars::install_world_detail_verbs` the reasoning.
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

    // **Truncation is toward zero** (`0x40a2b0` forces RC = chop), so a fractional stop floors
    // toward 0 from both sides — and -0.5 is therefore stop 0, ACCEPTED.
    s.run("SetWorldDetail(2.9)").unwrap();
    assert_eq!(frill(&s).as_deref(), Some("48"));
    s.run("SetWorldDetail(-0.5)").unwrap();
    assert_eq!(
        frill(&s).as_deref(),
        Some("16"),
        "-0.5 truncates to 0, which is in range: only <= -1 raises"
    );

    // **`lua_isnumber` accepts a numeric string.**
    s.run(r#"SetWorldDetail("2")"#).unwrap();
    assert_eq!(frill(&s).as_deref(), Some("48"));

    // Both raise paths, with the reference's own strings.
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

    // Zero return values from the setter; exactly one number from the getter.
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
    // The getter reads no argument — pfUI declares its replacement `function(arg)` and the stock
    // one it chains to is called with one.
    assert_eq!(s.eval::<i64>("return GetWorldDetail(7)").unwrap(), 1);
    assert_eq!(stop(&s).as_deref(), Some("1"));
}

/// **pfUI's `hdgraphic` module, run against the real bindings.**
///
/// The module hooks both verbs and drives `frillDensity` past the panel's top stop; this is that
/// arm executed verbatim (`modules/hdgraphic.lua` l.4-39), because "the verbs exist" and "the
/// module works" are different claims and only the second one is the point.
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

    // The hook's own precondition: `local Hook... = SetWorldDetail` captured a FUNCTION. Before
    // 2163 both were nil and this line took the module down with it.
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
    // ...and the low arm falls through to the stock getter, which is where a nil hook used to bite.
    assert_eq!(
        s.eval::<i64>("SetWorldDetail(2); return GetWorldDetail()")
            .unwrap(),
        2
    );
}

/// **The four nameplate verbs take no argument and return nothing.**
///
/// `ShowNameplates 0x489450` / `HideNameplates 0x489460` / `ShowFriendNameplates 0x489470` /
/// `HideFriendNameplates 0x489480` — four 10-byte bodies over two setters, differing only in an
/// `or`/`and` mask.
///
/// Both halves of the shape are asserted because both are easy to invent differently:
///
/// - **No argument is read.** `ShowNameplates(false)` still shows — there is no `lua_gettop` and
///   no `lua_toboolean` in any of the four. That is *why* there are four verbs instead of two
///   taking a flag, and a binding typed `fn(bool)` would reject a call the reference accepts.
/// - **Zero return values, not nil.** The return-list count is 0, which is observably different
///   from one nil for a caller that counts.
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

    // Each verb writes its own bit and leaves the other alone.
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

    // **An argument is accepted and IGNORED.** `ShowNameplates(false)` shows, which is the whole
    // reason the reference ships four verbs rather than two taking a flag.
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

    // ZERO return values, not one nil.
    assert_eq!(
        s.arity("ShowNameplates()").unwrap(),
        0,
        "zero values — observably different from nil for a caller that counts"
    );
}
