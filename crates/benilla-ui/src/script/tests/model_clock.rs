//! The model pane's clock, sequence arm and handlers (`0x76d7f0`, `0x7121a0`, `0x76cac0`). The
//! engine parses no M2: a file's sequences and bounds are [`ModelFileFacts`] the host hands over.

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

/// `UI-Cooldown-Indicator.m2`'s sequences as `m2seq` reads them.
const COOLDOWN_FILE: &str = r"Interface\Cooldown\UI-Cooldown-Indicator.mdx";
fn cooldown_facts() -> ModelFileFacts {
    facts(&[(0, 1000, false), (1, 1000, false)])
}

/// `MinimapPing.m2`'s sequences as `m2seq` reads them; `SetSequence(0)` is the looping one.
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

/// `OnUpdate` (`0x76d7f0`) adds `trunc(elapsed · 1000)` ms, for visible frames only.
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

    // The looping sequence wraps on its 833 ms.
    assert_eq!(play_head(&s, "Ping", PING_FILE), Some((0, 616)));
    s.tick(0.5); // clock 1116 → 1116 mod 833
    assert_eq!(play_head(&s, "Ping", PING_FILE), Some((0, 283)));
}

/// The loader's completion (`0x70ebd0`) arms Stand (id 0 if the file owns it, else
/// `animations[0]`'s id), at `SetModel` when the facts are known or when they land.
#[test]
fn set_model_seeds_stand_now_or_when_the_facts_land() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
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

    s.set_model_facts(
        r"Interface\Odd.mdx",
        facts(&[(204, 500, true), (166, 500, true)]),
    );
    s.run(r#"d = CreateFrame("Model", "Odd", UIParent) d:SetModel("Interface\\Odd.mdx")"#)
        .unwrap();
    assert_eq!(pane(&s, "Odd").armed.map(|a| a.anim_id), Some(204));
}

/// `0x7121a0` interrupts the track before its bounds check; a queued arm waits for the facts.
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

    // A queued arm the file owns wins over the seed, re-anchored on the landing clock.
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

/// The stock `Cooldown.lua`, which runs on `OnUpdateModel` and `OnAnimFinished` alone.
#[test]
fn the_cooldown_machine_runs_on_the_two_handlers() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(COOLDOWN_FILE, cooldown_facts());
    // `SetTimer` gates on `start > 0`, so the session clock must be past 0.
    s.tick(1.0);
    s.run(&format!(
        r#"
        -- Cooldown.lua, 1.12.1 (the stock file, transcribed for the test).
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

    s.tick(0.5);
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((0, 250)));
    s.tick(1.0);
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((0, 750)));
    // A scrub is an anchor, not a freeze: between paints the clock runs on from it.
    assert_eq!(pane(&s, "CD").armed.map(|a| a.anchor_ms), Some(1500 - 750));

    s.tick(0.6);
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((1, 0)));
    assert!(s.frame_visible("CD"));
    assert_eq!(s.eval::<i64>("return cd.stopping").unwrap(), 1);

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
    s.tick(1.0);
    assert_eq!(s.eval::<i64>("return table.getn(fired)").unwrap(), 1);
    assert_eq!(play_head(&s, "CD", COOLDOWN_FILE), Some((1, 1000)));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnUpdateModel` runs before the completion is read, so re-arming each paint completes nothing.
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
    s.run("M:Show() M:SetScript(\"OnUpdateModel\", function() this:SetSequenceTime(1, 0) end)")
        .unwrap();
    for _ in 0..30 {
        s.tick(0.1);
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `0x719370` enqueues the completion before it tests the loop flag, so a loop's first pass fires
/// `OnAnimFinished`; a pane with no file fails `0x76d24c`'s gate and never paints.
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

/// A pane with no authored size takes its file's bounding box in layout units, `768·√(a²+1)`
/// FrameXML units each (1280 at 4:3), following the screen's aspect.
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

    // At 16:9 a layout unit is 768·√((16/9)²+1) = 1566.4 FrameXML units.
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

/// The type-14 texture override belongs to the model instance and dies with it.
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

/// A model pane's XML `scale=` is the model's scale (`0x76cac0`, `+0x3a0`), never the frame's.
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
