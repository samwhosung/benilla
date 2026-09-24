//! **The model pane's clock, arm, and two handlers** (`0x76d7f0`, `0x7121a0`,
//! `0x76cac0`).
//!
//! A `<Model>` widget owns a private scene whose clock its own `OnUpdate` advances while the
//! frame is visible; `SetSequence`/`SetSequenceTime` arm a sequence with an anchor the sampler
//! re-reads; `OnUpdateModel` fires at the top of every paint and `OnAnimFinished` from the
//! completion of a clamped sequence. Every one of those is Lua-observable — the shipped cooldown
//! sweep is nothing but those handlers driving `SetSequenceTime` — so the law is tested here
//! against the stock `Cooldown.lua`, transcribed verbatim.
//!
//! The engine parses no M2, so a file's sequences and bounds are **facts** the host hands over
//! ([`ModelFileFacts`]); these tests hand over the facts `benilla-extract m2seq` reads off the
//! shipped files.

use super::common::script;
use crate::script::{Model, UiScript};
use crate::widget::{KindState, ModelFileFacts, ModelState, SequenceFacts};

/// A file's facts from `(anim_id, duration_ms, looping)` rows, in file order.
fn facts(rows: &[(u16, u32, bool)]) -> ModelFileFacts {
    ModelFileFacts {
        sequences: rows
            .iter()
            .map(|&(anim_id, duration_ms, looping)| SequenceFacts {
                anim_id,
                duration_ms,
                looping,
            })
            .collect(),
        bbox: ([0.0; 3], [0.0; 3]),
        cameras: 0,
    }
}

/// `UI-Cooldown-Indicator.m2` (`m2seq`): seq 0 = id 0, 1000 ms, clamp; seq 1 = id 1, 1000 ms,
/// clamp.
const COOLDOWN_FILE: &str = r"Interface\Cooldown\UI-Cooldown-Indicator.mdx";
fn cooldown_facts() -> ModelFileFacts {
    facts(&[(0, 1000, false), (1, 1000, false)])
}

/// `MinimapPing.m2` (`m2seq`): seq 0 = id 127, 1333 ms, clamp; seq 1 = id 0, 833 ms, LOOP; seq
/// 2 = id 1, 333 ms, clamp. `SetSequence(0)` is the looping one.
const PING_FILE: &str = r"Interface\MiniMap\Ping\MinimapPing.mdx";
fn ping_facts() -> ModelFileFacts {
    facts(&[(127, 1333, false), (0, 833, true), (1, 333, false)])
}

/// The pane's scene state, read through the arena (1.12 has no getter for any of it).
pub(super) fn pane(s: &UiScript, name: &str) -> ModelState {
    let model = s.lua().app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("pane frame");
    match &model.arena.frame(fh).expect("live frame").kind_state {
        KindState::Model(m) => m.clone(),
        _ => panic!("{name} is not a Model"),
    }
}

/// `(anim_id, cursor_ms)` of the armed sequence under the facts the engine holds.
fn play_head(s: &UiScript, name: &str, path: &str) -> Option<(u16, u32)> {
    let m = pane(s, name);
    let model = s.lua().app_data_ref::<Model>().expect("model");
    let facts = model.model_facts.get(&crate::widget::model_key(path))?;
    m.play_head(facts).map(|p| (p.anim_id, p.cursor_ms))
}

/// The widget's `OnUpdate` (`0x76d7f0`) advances the scene clock by `trunc(elapsed · 1000)` —
/// only while the frame is visible (the UI pump walks visible frames), so a hidden pane's clock
/// stands still and a re-shown pane resumes where it stopped.
#[test]
fn the_clock_runs_only_while_the_pane_is_shown_and_truncates() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(PING_FILE, ping_facts());
    s.run(&format!(
        r#"p = CreateFrame("Model", "Ping", UIParent) p:SetModel("{}") p:SetSequence(0)"#,
        PING_FILE.replace('\\', "\\\\")
    ))
    .unwrap();
    s.tick(0.5);
    assert_eq!(pane(&s, "Ping").clock_ms, 500);
    s.tick(0.0166); // 16.6 ms → 16, not 17
    assert_eq!(pane(&s, "Ping").clock_ms, 516, "trunc, no +0.5 (76d854)");

    s.run("Ping:Hide()").unwrap();
    s.tick(1.0);
    assert_eq!(
        pane(&s, "Ping").clock_ms,
        516,
        "hidden: the clock stands still"
    );
    s.run("Ping:Show()").unwrap();
    s.tick(0.1);
    assert_eq!(
        pane(&s, "Ping").clock_ms,
        616,
        "re-shown: resumes where it stopped"
    );

    // The looping sequence wraps on its 833 ms and never completes.
    assert_eq!(play_head(&s, "Ping", PING_FILE), Some((0, 616)));
    s.tick(0.5); // clock 1116 → 1116 mod 833
    assert_eq!(play_head(&s, "Ping", PING_FILE), Some((0, 283)));
}

/// `SetModel` runs the loader's completion (`0x70ebd0`) — arm **Stand** (id 0 if the file owns
/// it, else `animations[0]`'s id), variation 0 — synchronously when the facts are known, and when
/// they land otherwise; until then the pane is a waiter and the host is asked for the file.
#[test]
fn set_model_seeds_stand_now_or_when_the_facts_land() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    // Facts first: the arm is immediate.
    s.set_model_facts(COOLDOWN_FILE, cooldown_facts());
    s.run(&format!(
        r#"a = CreateFrame("Model", "Known", UIParent) a:SetModel("{}")"#,
        COOLDOWN_FILE.replace('\\', "\\\\")
    ))
    .unwrap();
    let m = pane(&s, "Known");
    assert!(!m.pending_seed);
    assert_eq!(
        m.armed.map(|a| a.anim_id),
        Some(0),
        "Stand, armed at the call"
    );
    assert_eq!(play_head(&s, "Known", COOLDOWN_FILE), Some((0, 0)));

    // Facts unknown: a waiter, and the file goes on the host's list once.
    s.run(&format!(
        r#"b = CreateFrame("Model", "Waiting", UIParent) b:SetModel("{0}")
           c = CreateFrame("Model", "Waiting2", UIParent) c:SetModel("{0}")"#,
        PING_FILE.replace('\\', "\\\\")
    ))
    .unwrap();
    let m = pane(&s, "Waiting");
    assert!(m.pending_seed && m.armed.is_none());
    assert_eq!(
        s.model_facts_wanted(),
        vec!["interface/minimap/ping/minimapping".to_string()],
        "one request per file, case-folded, no extension"
    );
    assert!(s.model_facts_wanted().is_empty(), "drained");

    // The facts land: every waiter holding the file arms its Stand at its own clock.
    s.tick(0.25);
    s.set_model_facts(PING_FILE, ping_facts());
    for name in ["Waiting", "Waiting2"] {
        let m = pane(&s, name);
        assert!(!m.pending_seed);
        assert_eq!(
            m.armed.map(|a| (a.anim_id, a.anchor_ms)),
            Some((0, 250)),
            "{name}: Stand (the file owns id 0), anchored at the clock the file landed on"
        );
    }

    // A file that does not own id 0 seeds `animations[0]`'s own id.
    s.set_model_facts(
        r"Interface\Odd.mdx",
        facts(&[(204, 500, true), (166, 500, true)]),
    );
    s.run(r#"d = CreateFrame("Model", "Odd", UIParent) d:SetModel("Interface\\Odd.mdx")"#)
        .unwrap();
    assert_eq!(pane(&s, "Odd").armed.map(|a| a.anim_id), Some(204));
}

/// `SetSequence(id)` with an id the file does not own **stops what was playing and arms
/// nothing** — `0x7121a0`'s interrupt runs before its bounds check. A queued arm on a
/// file still loading is kept until the facts say otherwise.
#[test]
fn an_unowned_id_stops_the_track_and_arms_nothing() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(PING_FILE, ping_facts());
    s.run(&format!(
        r#"p = CreateFrame("Model", "P", UIParent) p:SetModel("{}") p:SetSequence(0)"#,
        PING_FILE.replace('\\', "\\\\")
    ))
    .unwrap();
    assert!(pane(&s, "P").armed.is_some());
    s.run("P:SetSequence(42)").unwrap();
    assert_eq!(pane(&s, "P").sequence, 42, "the raw id is still recorded");
    assert!(pane(&s, "P").armed.is_none(), "…but nothing plays");

    // Queued on an unloaded file, then refused when the facts land.
    s.run(r#"q = CreateFrame("Model", "Q", UIParent) q:SetModel("Interface\\Late.mdx") q:SetSequence(7)"#)
        .unwrap();
    assert_eq!(
        pane(&s, "Q").armed.map(|a| a.anim_id),
        Some(7),
        "kept for the replay"
    );
    s.set_model_facts(r"Interface\Late.mdx", facts(&[(0, 100, true)]));
    assert!(
        pane(&s, "Q").armed.is_none(),
        "the seed armed Stand, then the queued arm replayed and named an id the file does not \
         own — nothing plays"
    );

    // The other order of the same replay: a queued arm the file DOES own wins over the seed,
    // at its original offset, re-anchored on the clock the file landed on.
    s.run(r#"r = CreateFrame("Model", "R", UIParent) r:SetModel("Interface\\Later.mdx") r:SetSequenceTime(5, 40)"#)
        .unwrap();
    s.tick(0.3);
    s.set_model_facts(
        r"Interface\Later.mdx",
        facts(&[(0, 100, true), (5, 900, false)]),
    );
    let a = pane(&s, "R").armed.expect("the queued arm replays");
    assert_eq!((a.anim_id, a.armed_at_ms, a.anchor_ms), (5, 300, 300 - 40));
}

/// The shipped cooldown, on the two handlers and nothing else — `Cooldown.lua` verbatim:
/// `SetTimer` arms sequence 0 and shows; every paint `OnUpdateModel` scrubs
/// `SetSequenceTime(0, elapsed/duration · 1000)` until done, then flips to sequence 1 at 0; the
/// flash's completion (`OnAnimFinished`, 1000 ms later) hides the frame.
#[test]
fn the_cooldown_machine_runs_on_the_two_handlers() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(COOLDOWN_FILE, cooldown_facts());
    // `SetTimer`'s gate is `start > 0`: a session clock still at 0 refuses the timer, as the
    // reference's would — so the session is a second old before the button is pressed.
    s.tick(1.0);
    s.run(&format!(
        r#"
        -- Cooldown.lua, 1.12.1 (the stock file, transcribed for the test — decision 1602).
        function CooldownFrame_SetTimer(this, start, duration, enable)
            if ( start > 0 and duration > 0 and enable > 0) then
                this.start = start;
                this.duration = duration;
                this.stopping = 0;
                this:SetSequence(0);
                this:Show();
            else
                this:Hide();
            end
        end
        function CooldownFrame_OnUpdateModel()
            if ( this.stopping == 0 ) then
                local finished = (GetTime() - this.start) / this.duration;
                if ( finished < 1.0 ) then
                    local time = finished * 1000;
                    this:SetSequenceTime(0, time);
                    return;
                end
                this.stopping = 1;
                this:SetSequence(1);
                this:SetSequenceTime(1, 0);
            else
                this:AdvanceTime();
            end
        end
        function CooldownFrame_OnAnimFinished()
            if ( this.stopping == 1 ) then
                this:Hide();
            end
        end
        cd = CreateFrame("Model", "CD", UIParent)
        cd:SetModel("{}")
        cd:SetScript("OnUpdateModel", CooldownFrame_OnUpdateModel)
        cd:SetScript("OnAnimFinished", CooldownFrame_OnAnimFinished)
        cd:Hide()
        fired = {{}}
        cd:SetScript("OnAnimFinished", function() table.insert(fired, "finished") CooldownFrame_OnAnimFinished() end)
        CooldownFrame_SetTimer(cd, GetTime(), 2, 1)
    "#,
        COOLDOWN_FILE.replace('\\', "\\\\")
    ))
    .unwrap();
    assert!(s.frame_visible("CD"));
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((0, 0)));

    // The sweep: each paint scrubs sequence 0 to the elapsed fraction.
    s.tick(0.5);
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((0, 250)));
    s.tick(1.0);
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((0, 750)));
    // A scrub is an ANCHOR, not a freeze: between two paints the clock runs on from it. The
    // cooldown never sees that (it re-scrubs every paint), the ping relies on it.
    assert_eq!(pane(&s, "CD").armed.map(|a| a.anchor_ms), Some(1500 - 750));

    // Done: the flip to the flash, sequence 1 at 0.
    s.tick(0.6);
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((1, 0)));
    assert!(s.frame_visible("CD"));
    assert_eq!(s.eval::<i64>("return cd.stopping").unwrap(), 1);

    // The flash runs its 1000 ms; the completion fires once, and the handler hides the frame.
    s.tick(0.5);
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((1, 500)));
    assert!(s.eval::<bool>("return table.getn(fired) == 0").unwrap());
    s.tick(0.6);
    assert_eq!(
        s.eval::<i64>("return table.getn(fired)").unwrap(),
        1,
        "OnAnimFinished on the clamped sequence's natural completion"
    );
    assert!(
        !s.frame_visible("CD"),
        "…which the stock handler turns into Hide()"
    );
    // Hidden: no paint, no second completion, the cursor holds at the end.
    s.tick(1.0);
    assert_eq!(s.eval::<i64>("return table.getn(fired)").unwrap(), 1);
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((1, 1000)));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnUpdateModel` runs with `this` set, once per paint of a **visible** pane, before the
/// completion is read — so a handler that re-arms in the same paint completes nothing.
#[test]
fn on_update_model_fires_per_visible_paint_with_this() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(COOLDOWN_FILE, cooldown_facts());
    s.run(&format!(
        r#"
        seen = {{}}
        m = CreateFrame("Model", "M", UIParent)
        m:SetModel("{}")
        m:SetScript("OnUpdateModel", function() table.insert(seen, this:GetName()) end)
        m:SetScript("OnAnimFinished", function() error("must not fire") end)
        m:SetSequence(1)
    "#,
        COOLDOWN_FILE.replace('\\', "\\\\")
    ))
    .unwrap();
    s.tick(0.1);
    s.tick(0.1);
    assert_eq!(
        s.eval::<i64>("return table.getn(seen)").unwrap(),
        2,
        "one per tick"
    );
    assert_eq!(s.eval::<String>("return seen[1]").unwrap(), "M");
    s.run("M:Hide()").unwrap();
    s.tick(0.1);
    assert_eq!(
        s.eval::<i64>("return table.getn(seen)").unwrap(),
        2,
        "no paint while hidden"
    );
    // Re-arming every paint keeps the clamped sequence from ever completing.
    s.run("M:Show() M:SetScript(\"OnUpdateModel\", function() this:SetSequenceTime(1, 0) end)")
        .unwrap();
    for _ in 0..30 {
        s.tick(0.1);
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A LOOPING sequence fires `OnAnimFinished` once too — at the end of its first pass — and keeps
/// looping (`0x719370` enqueues the completion before it tests the loop flag). And a pane with no
/// file fires no `OnUpdateModel` at all (`76d24c`'s gate), however visible.
#[test]
fn a_loop_completes_once_and_a_fileless_pane_paints_nothing() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(PING_FILE, ping_facts());
    s.run(
        r#"
        done = 0 painted = 0
        p = CreateFrame("Model", "Loop", UIParent)
        p:SetScript("OnAnimFinished", function() done = done + 1 end)
        p:SetScript("OnUpdateModel", function() painted = painted + 1 end)
    "#,
    )
    .unwrap();
    s.tick(0.1);
    assert_eq!(
        s.eval::<i64>("return painted").unwrap(),
        0,
        "no file: no paint"
    );
    s.run(&format!(
        r#"p:SetModel("{}") p:SetSequence(0)"#,
        PING_FILE.replace('\\', "\\\\")
    ))
    .unwrap();
    s.tick(0.5);
    assert_eq!(
        s.eval::<i64>("return painted").unwrap(),
        1,
        "a file: one paint per tick"
    );
    assert_eq!(s.eval::<i64>("return done").unwrap(), 0);
    s.tick(0.4); // 900 ms ≥ the 833 ms loop
    assert_eq!(
        s.eval::<i64>("return done").unwrap(),
        1,
        "the first pass completes"
    );
    assert_eq!(
        play_head(&s, "Loop", PING_FILE),
        Some((0, 67)),
        "…and the loop goes on"
    );
    s.tick(1.0);
    assert_eq!(s.eval::<i64>("return done").unwrap(), 1, "once per arm");
    s.run("p:SetSequence(0)").unwrap();
    s.tick(0.9);
    assert_eq!(
        s.eval::<i64>("return done").unwrap(),
        2,
        "a re-arm completes again"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **The implicit rect**: a model pane that authored no size takes its file's
/// bounding-box extent in LAYOUT units — `768·√(a²+1)` FrameXML units per unit, `1280` at 4:3
/// — the moment the facts are known; it follows the screen's aspect; an authored size wins.
#[test]
fn a_size_less_pane_takes_its_files_rect_in_layout_units() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    // The map arrow's own box (`MinimapArrow.m2`, `0x76d080`/`0x76d0d0`): 0.0262 × 0.0263 units.
    let arrow = |s: &mut UiScript| {
        s.set_model_facts(
            r"Interface\Minimap\MinimapArrow.mdx",
            ModelFileFacts {
                sequences: vec![SequenceFacts {
                    anim_id: 0,
                    duration_ms: 3333,
                    looping: true,
                }],
                bbox: ([-0.0127, -0.0118, 0.0], [0.0135, 0.0145, 0.0]),
                cameras: 0,
            },
        );
    };
    s.run(
        r#"
        a = CreateFrame("Model", "Sized", UIParent)
        a:SetPoint("CENTER", 0, 0)
        b = CreateFrame("Model", "Authored", UIParent)
        b:SetPoint("CENTER", 0, 0) b:SetWidth(50) b:SetHeight(20)
        a:SetModel("Interface\\Minimap\\MinimapArrow.mdx")
        b:SetModel("Interface\\Minimap\\MinimapArrow.mdx")
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return Sized:GetWidth()").unwrap(),
        0.0,
        "no facts yet: no rect"
    );
    arrow(&mut s);
    s.resolve();
    let (w, h): (f32, f32) = s
        .eval("return Sized:GetWidth(), Sized:GetHeight()")
        .unwrap();
    assert!(
        (w - 0.0262 * 1280.0).abs() < 0.05 && (h - 0.0263 * 1280.0).abs() < 0.05,
        "{w}×{h}"
    );
    let (bw, bh): (f32, f32) = s
        .eval("return Authored:GetWidth(), Authored:GetHeight()")
        .unwrap();
    assert_eq!((bw, bh), (50.0, 20.0), "an authored size is untouched");
    assert!(pane(&s, "Sized").implicit_size && !pane(&s, "Authored").implicit_size);

    // 16:9 — a layout unit is 768·√((16/9)²+1) = 1566.4 FrameXML units.
    s.set_screen_size(1600.0, 900.0);
    s.resolve();
    let w: f32 = s.eval("return Sized:GetWidth()").unwrap();
    assert!((w - 0.0262 * 1566.4).abs() < 0.1, "{w}");

    // Authoring a size later ends the implicit rect for good.
    s.run("Sized:SetWidth(10)").unwrap();
    assert!(!pane(&s, "Sized").implicit_size);
    s.set_screen_size(1024.0, 768.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sized:GetWidth()").unwrap(), 10.0);
}

/// `ReplaceIconTexture` is the type-14 texture override on the INSTANCE: stored over a file
/// (queued and replayed while it streams), dropped with no file, released by `SetModel` and
/// `ClearModel`.
#[test]
fn replace_icon_texture_lives_and_dies_with_the_instance() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        card = CreateFrame("Model", "Card", UIParent)
        card:ReplaceIconTexture("Interface\\Icons\\INV_Misc_Bag_08")
    "#,
    )
    .unwrap();
    assert_eq!(
        pane(&s, "Card").icon,
        None,
        "no instance: dropped, never replayed"
    );
    s.run(
        r#"
        card:SetModel("Interface\\ItemAnimations\\ForcedBackpackItem.mdx")
        card:ReplaceIconTexture("Interface\\Icons\\INV_Misc_Bag_08")
    "#,
    )
    .unwrap();
    assert_eq!(
        pane(&s, "Card").icon.as_deref(),
        Some(r"Interface\Icons\INV_Misc_Bag_08")
    );
    s.run(r#"card:SetModel("Interface\\ItemAnimations\\ForcedBackpackItem.mdx")"#)
        .unwrap();
    assert_eq!(
        pane(&s, "Card").icon,
        None,
        "a fresh instance has no override"
    );
    s.run(r#"card:ReplaceIconTexture("x") card:ClearModel()"#)
        .unwrap();
    assert_eq!(pane(&s, "Card").icon, None);
    assert!(
        s.run("card:ReplaceIconTexture(nil)").is_err(),
        "the shape-A usage raise stays"
    );
}

/// The XML `scale=` on a model pane is the MODEL's scale (`0x76cac0` → `+0x3a0`), never the
/// frame's — the cooldown template's 0.75 must not shrink the widget's rect.
#[test]
fn the_model_scale_attribute_is_the_models_own() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    // The pet bar's shine (`PetActionBarFrame.xml:25`), and a `scale="0"` beside it.
    let doc = crate::framexml::parse(
        r#"<Ui>
            <Frame name="Host"><Size><AbsDimension x="30" y="30"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <Frames>
                    <Model name="ShinePane" file="Interface\Buttons\UI-AutoCastButton.mdx" scale="1.2" hidden="true" setAllPoints="true"/>
                    <Model name="ZeroPane" file="Interface\Buttons\UI-AutoCastButton.mdx" scale="0" hidden="true" setAllPoints="true"/>
                </Frames>
            </Frame>
        </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert_eq!(
        s.eval::<(f64, f64)>("return ShinePane:GetModelScale(), ShinePane:GetScale()")
            .unwrap(),
        (1.2_f32 as f64, 1.0),
        "the attribute lands on SetModelScale; the frame scale is untouched"
    );
    assert_eq!(
        s.eval::<f64>("return ZeroPane:GetModelScale()").unwrap(),
        1.0,
        "≤ 0 is the reference's raise, not a clamp — the default stands"
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("Invalid model scale")),
        "{:?}",
        report.warnings
    );
}
