//! The engine unit tooltip builder (decision 0274 P3, law per 0276): the level-line composition
//! (four `TOOLTIP_UNIT_LEVEL*` keys, the `ELITE`/`BOSS` rank table, "??", `CORPSE`, "Race Class"
//! over `PLAYER` — all seeded here as marked stand-ins, see [`seed_level_strings`]), the flag lines
//! (PvP white / Skinnable red / Civilian green), the world-mouseover drive (default anchor +
//! `UPDATE_MOUSEOVER_UNIT` recolor + the fade arm on loss), and the health-bar watcher.

use super::common::script;
use crate::script::*;

/// A stand-in string table for the level line — the four `TOOLTIP_UNIT_LEVEL*` templates and the
/// three word slots that fill them, **deliberately not the shipped wording**. What these tests
/// establish is *which key* each slot combination reaches and what fills it, never what the
/// sentence says ("assert the identifier, not the sentence"), and here that is not
/// a formality: `TOOLTIP_UNIT_LEVEL_CLASS`'s enUS "Level %s %s" is word-for-word
/// `FRIENDS_LEVEL_TEMPLATE`, `UNIT_TYPE_LEVEL_TEMPLATE` and `CHARACTER_SELECT_INFO`, and the bare
/// template's "Level %s" is also `ITEM_LEVEL`, `LEVEL_GAINED` and `UNIT_LEVEL_TEMPLATE`. An
/// assertion on the English would pass on all seven.
fn seed_level_strings(s: &mut UiScript) {
    s.run(
        r#"
        TOOLTIP_UNIT_LEVEL            = "[LEVEL %s]"
        TOOLTIP_UNIT_LEVEL_CLASS      = "[LEVEL_CLASS %s %s]"
        TOOLTIP_UNIT_LEVEL_TYPE       = "[LEVEL_TYPE %s %s]"
        TOOLTIP_UNIT_LEVEL_CLASS_TYPE = "[LEVEL_CLASS_TYPE %s %s %s]"
        CORPSE = "[CORPSE]"
        PLAYER = "[PLAYER]"
        ELITE  = "[ELITE]"
        BOSS   = "[BOSS]"
    "#,
    )
    .unwrap();
}

fn wolf() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Timber Wolf".into()),
        health: 30,
        max_health: 50,
        level: 10,
        reaction: 2, // hostile
        creature_type_name: Some("Beast".into()),
        rank: 2, // rare-elite prints ELITE (the byte table)
        skinnable: true,
        ..Default::default()
    }
}

/// The creature law: gold name, subtitle, the CLASS_TYPE level line, red Skinnable.
#[test]
fn creature_line_law() {
    let mut s = script();
    seed_level_strings(&mut s);
    s.set_screen_size(800.0, 600.0);
    let mut u = wolf();
    u.subtitle = Some("Alpha".into());
    s.set_unit("target", Some(u));
    s.set_player_req_state(PlayerReqState {
        level: 12,
        ..Default::default()
    });
    s.run(
        r#"
        local a = CreateFrame("Button", "UF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        assert(tt:SetUnit("target") == 1, "SetUnit returns 1 on a live unit")
        assert(TTTextLeft1:GetText() == "Timber Wolf")
        assert(TTTextLeft2:GetText() == "Alpha")
        assert(TTTextLeft3:GetText() == "[LEVEL_CLASS_TYPE 10 Beast [ELITE]]", "got " .. TTTextLeft3:GetText())
        assert(TTTextLeft4:GetText() == "Skinnable")
        -- A RECOGNISED token naming nothing answers nil...
        assert(tt:SetUnit("party4") == nil, "a recognised but absent unit answers nil")
        -- ...while an UNRECOGNISED one raises, because SetUnit resolves through the client's one
        -- token resolver like every Unit* verb (`0x515970`: a token matching
        -- none of the nine prefixes reaches `luaL_error("Unknown unit name: %s")` and longjmps).
        -- This used to read `SetUnit("nosuch") == nil`, which was the refuted claim.
        assert(pcall(tt.SetUnit, tt, "nosuch") == false, "an unrecognised token raises")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// The snapshot the app pushes for a creature the client has only just seen: the DESCRIPTOR's
/// fields are in (level, reaction, the skinnable flag) and everything the creature record carries
/// — name, subtitle, type word, rank, civilian/leader — is still absent, because they all arrive
/// together in the one `SMSG_CREATURE_QUERY_RESPONSE` that fills `CGUnit+0xb30`.
fn unqueried_wolf() -> UnitState {
    UnitState {
        exists: true,
        has_object: true,
        name: None,
        health: 30,
        max_health: 50,
        level: 10,
        reaction: 2, // hostile
        skinnable: true,
        guid: 0xF130_0000_0000_0001,
        ..Default::default()
    }
}

/// **A name still in flight titles the plate `UNKNOWNOBJECT` — never an empty line** (decision
/// 2040, closing 2002's residue).
///
/// The builder's name read is `CGUnit_C::GetUnitName 0x609210` (`0x52a187`), the same resolver
/// `UnitName 0x517020` delegates to, and every one of its misses — a null `CGUnit+0xb30`
/// (`0x609353 je 0x609324`) among them — ends at the same `FrameScript_GetText("UNKNOWNOBJECT")`
/// tail. So the verb and the plate answer the same string for the same instant, and because the
/// string is read out of the VM's `_G` it translates with `GlobalStrings.lua`.
///
/// The builder has NO counterpart to `UnitName`'s two nils: the `"player"` fast path is the
/// *binding's* (`0x517083`, before any resolve), so a player whose name has not arrived titles
/// `UNKNOWNOBJECT` too; and a token resolving to nothing never reaches a builder at all — the
/// entry gate hands back no object, `SetUnit` answers nil and no plate is drawn.
///
/// Failure looks like the empty first line the residue named: `TextLeft1` reads `""`, the plate
/// measures to a blank row, and the level line reads as the tooltip's title.
#[test]
fn a_pending_name_titles_unknownobject_and_the_answer_replaces_it() {
    let mut s = script();
    seed_level_strings(&mut s);
    s.set_screen_size(800.0, 600.0);
    s.set_unit("target", Some(unqueried_wolf()));
    s.set_player_req_state(PlayerReqState {
        level: 12,
        ..Default::default()
    });
    s.run(
        r#"
        UNKNOWNOBJECT = "Unknown"   -- GlobalStrings.lua:4444, enUS
        local a = CreateFrame("Button", "UF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        assert(tt:SetUnit("target") == 1, "a resolved unit shows, name or no name")
        assert(TTTextLeft1:GetText() == UNKNOWNOBJECT,
               "the title is the GlobalString, got '" .. tostring(TTTextLeft1:GetText()) .. "'")
        -- The record's other lines are absent with it: no subtitle, and the level line takes the
        -- bare TOOLTIP_UNIT_LEVEL template because both the CLASS and TYPE slots are the record's.
        assert(TTTextLeft2:GetText() == "[LEVEL 10]", "got " .. TTTextLeft2:GetText())
        -- The descriptor's own lines are NOT the record's and show anyway.
        assert(TTTextLeft3:GetText() == "Skinnable")
    "#,
    )
    .unwrap();

    // The query answers: the very next render replaces the placeholder with the real name and
    // fills the record's lines. (Live, the hover feed re-drives this without the mouse moving —
    // its rebuild key is the whole line-affecting snapshot, `ui_tooltip::lines_view`.)
    let mut answered = unqueried_wolf();
    answered.name = Some("Timber Wolf".into());
    answered.subtitle = Some("Alpha".into());
    answered.creature_type_name = Some("Beast".into());
    answered.rank = 2;
    s.set_unit("target", Some(answered));
    s.run(
        r#"
        assert(TT:SetUnit("target") == 1)
        assert(TTTextLeft1:GetText() == "Timber Wolf", "the answer replaced UNKNOWNOBJECT")
        assert(TTTextLeft2:GetText() == "Alpha")
        assert(TTTextLeft3:GetText() == "[LEVEL_CLASS_TYPE 10 Beast [ELITE]]", "got " .. TTTextLeft3:GetText())
        assert(TTTextLeft4:GetText() == "Skinnable")
    "#,
    )
    .unwrap();

    // An EMPTY (or absent) global is the same miss as no global — `0x609324`'s own check, which
    // falls to the binary's literal `0x860fa4`.
    s.set_unit("target", Some(unqueried_wolf()));
    s.run(
        r#"
        UNKNOWNOBJECT = ""
        assert(TT:SetUnit("target") == 1)
        assert(TTTextLeft1:GetText() == "Unknown Being", "got " .. TTTextLeft1:GetText())
    "#,
    )
    .unwrap();

    // The `"player"` fast path is the BINDING's, not the builder's: a player whose name-cache row
    // has not answered titles the same placeholder, where `UnitName("player")` would push nil.
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            has_object: true,
            name: None,
            level: 12,
            is_player: true,
            player_controlled: true,
            guid: 0x0000_0000_0000_0007,
            ..Default::default()
        }),
    );
    s.run(
        r#"
        UNKNOWNOBJECT = "Unknown"
        assert(UnitName("player") == nil, "the binding's own fast path still pushes nil (2002)")
        assert(TT:SetUnit("player") == 1)
        assert(TTTextLeft1:GetText() == UNKNOWNOBJECT,
               "the builder has no player fast path, got " .. tostring(TTTextLeft1:GetText()))
    "#,
    )
    .unwrap();

    // The GUID-0 counterpart: a recognised token naming nothing resolves to no object, so no
    // builder runs — nil, and no plate. (There is no third answer; the builder never sees it.)
    s.run(r#"assert(TT:SetUnit("party4") == nil, "an absent unit draws no plate at all")"#)
        .unwrap();
    assert!(s.take_errors().is_empty());
}

/// The faction-name line sits between the level line and "PvP" (the builder-tail block of
/// `0x529fe0` — the director's Marshal McBride reference: Level, Stormwind, PvP); the
/// CIVILIAN line is the dishonorable-kill warning, whole gate (`0x612550`): PvP bit + civilian
/// flag + HOSTILE + GREY/trivial — a friendly (or non-grey) civilian never shows it; LEADER
/// (white) needs only the PvP bit + the flag (`0x6125c0`).
#[test]
fn faction_line_and_civilian_gate() {
    let mut s = script();
    seed_level_strings(&mut s);
    s.set_screen_size(800.0, 600.0);
    s.set_player_req_state(PlayerReqState {
        level: 30,
        ..Default::default()
    });
    // The reference shot's shape: a FRIENDLY civilian guard — faction line, PvP, NO Civilian.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Marshal McBride".into()),
            level: 20,
            reaction: 6, // friendly
            creature_type_name: Some("Humanoid".into()),
            faction_name: Some("Stormwind".into()),
            pvp: true,
            civilian: true,
            ..Default::default()
        }),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "UF9"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetUnit("target")
        assert(TTTextLeft2:GetText() == "[LEVEL 20]", "friendly creature: no type word; got " .. TTTextLeft2:GetText())
        assert(TTTextLeft3:GetText() == "Stormwind", "faction line before PvP; got " .. TTTextLeft3:GetText())
        assert(TTTextLeft4:GetText() == "PvP")
        assert(TTTextLeft5 == nil or TTTextLeft5:GetText() == nil, "friendly civilian shows NO Civilian line")
    "#,
    )
    .unwrap();
    // The warning case: HOSTILE + grey (level 20 vs player 30 → gap 10 > band 8) + PvP-flagged.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Defias Civilian".into()),
            level: 20,
            reaction: 2, // hostile
            creature_type_name: Some("Humanoid".into()),
            pvp: true,
            civilian: true,
            racial_leader: true,
            ..Default::default()
        }),
    );
    s.run(
        r#"
        TT:SetOwner(UF9, "ANCHOR_RIGHT")
        TT:SetUnit("target")
        assert(TTTextLeft2:GetText() == "[LEVEL_CLASS 20 Humanoid]", "got " .. TTTextLeft2:GetText())
        assert(TTTextLeft3:GetText() == "PvP")
        assert(TTTextLeft4:GetText() == "Civilian", "hostile+grey+pvp civilian warns; got " .. TTTextLeft4:GetText())
        assert(TTTextLeft5:GetText() == "Leader")
    "#,
    )
    .unwrap();
    // Same unit but NOT grey (level 25, gap 5 ≤ band 8): the warning drops.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Defias Civilian".into()),
            level: 25,
            reaction: 2,
            creature_type_name: Some("Humanoid".into()),
            pvp: true,
            civilian: true,
            ..Default::default()
        }),
    );
    s.run(
        r#"
        TT:SetOwner(UF9, "ANCHOR_RIGHT")
        TT:SetUnit("target")
        assert(TTTextLeft3:GetText() == "PvP")
        assert(TTTextLeft4 == nil or TTTextLeft4:GetText() == nil, "a non-grey civilian does not warn")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// Players read "Race Class (Player)"; the dead read "Corpse"; a world boss reads "??".
#[test]
fn level_line_variants() {
    let mut s = script();
    seed_level_strings(&mut s);
    s.set_screen_size(800.0, 600.0);
    s.set_player_req_state(PlayerReqState {
        level: 60,
        ..Default::default()
    });
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Bandit".into()),
            level: 32,
            is_player: true,
            race: Some("Human".into()),
            class: Some("Rogue".into()),
            pvp: true,
            ..Default::default()
        }),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "UF2"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetUnit("target")
        assert(TTTextLeft2:GetText() == "[LEVEL_CLASS_TYPE 32 Human Rogue [PLAYER]]", "got " .. TTTextLeft2:GetText())
        assert(TTTextLeft3:GetText() == "PvP")
    "#,
    )
    .unwrap();
    // Dead: the class slot becomes Corpse.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Slain Wolf".into()),
            level: 3,
            dead: true,
            reaction: 2,
            creature_type_name: Some("Beast".into()),
            ..Default::default()
        }),
    );
    s.run(
        r#"
        TT:SetOwner(UF2, "ANCHOR_RIGHT")
        TT:SetUnit("target")
        assert(TTTextLeft2:GetText() == "[LEVEL_CLASS 3 [CORPSE]]", "got " .. TTTextLeft2:GetText())
    "#,
    )
    .unwrap();
    // A world boss (rank 3) reads "??" + (Boss).
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Kazzak".into()),
            level: 63,
            reaction: 2,
            creature_type_name: Some("Demon".into()),
            rank: 3,
            ..Default::default()
        }),
    );
    s.run(
        r#"
        TT:SetOwner(UF2, "ANCHOR_RIGHT")
        TT:SetUnit("target")
        assert(TTTextLeft2:GetText() == "[LEVEL_CLASS_TYPE ?? Demon [BOSS]]", "got " .. TTTextLeft2:GetText())
    "#,
    )
    .unwrap();
    // The "??" gate, byte-pinned: HOSTILE (reaction ≤ 2) + 10 levels up reads "??"…
    let ten_up = |reaction: u8, is_player: bool| UnitState {
        exists: true,
        name: Some("Elder".into()),
        level: 70,
        reaction,
        is_player,
        creature_type_name: (!is_player).then(|| "Beast".into()),
        race: is_player.then(|| "Orc".into()),
        class: is_player.then(|| "Shaman".into()),
        ..Default::default()
    };
    s.set_unit("target", Some(ten_up(2, false)));
    s.run(
        r#"
        TT:SetOwner(UF2, "ANCHOR_RIGHT"); TT:SetUnit("target")
        assert(TTTextLeft2:GetText() == "[LEVEL_CLASS ?? Beast]", "hostile 10-up, got " .. TTTextLeft2:GetText())
    "#,
    )
    .unwrap();
    // …but UNFRIENDLY (reaction 3) does not — the internal bound is hated/hostile only…
    s.set_unit("target", Some(ten_up(3, false)));
    s.run(
        r#"
        TT:SetOwner(UF2, "ANCHOR_RIGHT"); TT:SetUnit("target")
        assert(TTTextLeft2:GetText() == "[LEVEL_CLASS 70 Beast]", "unfriendly 10-up, got " .. TTTextLeft2:GetText())
    "#,
    )
    .unwrap();
    // …and PLAYERS never read "??" regardless of reaction/delta.
    s.set_unit("target", Some(ten_up(2, true)));
    s.run(
        r#"
        TT:SetOwner(UF2, "ANCHOR_RIGHT"); TT:SetUnit("target")
        assert(TTTextLeft2:GetText() == "[LEVEL_CLASS_TYPE 70 Orc Shaman [PLAYER]]", "hostile player, got " .. TTTextLeft2:GetText())
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// The world-mouseover drive: `world_tooltip_unit` fires the default-anchor script, renders,
/// fires `UPDATE_MOUSEOVER_UNIT` (the Lua recolor), and the health bar tracks later pushes;
/// hover loss arms the fade (alpha ramps, then hides).
#[test]
fn world_hover_drive_and_health_watcher() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit("mouseover", Some(wolf()));
    s.run(
        r#"
        anchored, recolored = 0, 0
        -- A real full-screen UIParent, like the shipped UIParent.xml provides: the default-anchor
        -- handler must do the REAL seating (a counter-only stub is exactly the hole that let the
        -- unwired live handler park the plate at screen center).
        UIParent = CreateFrame("Frame", "UIParent"); UIParent:SetAllPoints()
        local tt = CreateFrame("GameTooltip", "GameTooltip")
        tt:Hide()
        -- The XML wiring, test-side: default anchor + the reaction recolor + a status bar child.
        tt:SetScript("OnTooltipSetDefaultAnchor", function()
            anchored = anchored + 1
            GameTooltip:SetOwner(UIParent, "ANCHOR_NONE")
            GameTooltip:SetPoint("BOTTOMRIGHT", "UIParent", "BOTTOMRIGHT", -13, 70)
        end)
        tt:RegisterEvent("UPDATE_MOUSEOVER_UNIT")
        tt:SetScript("OnEvent", function()
            recolored = recolored + 1
            getglobal("GameTooltipTextLeft1"):SetTextColor(1, 0, 0)
        end)
        local bar = CreateFrame("StatusBar", "GameTooltipStatusBar", tt)
        bar:SetPoint("TOPLEFT", tt, "BOTTOMLEFT", 2, -1); bar:SetWidth(100); bar:SetHeight(8)
    "#,
    )
    .unwrap();
    assert!(s.world_tooltip_unit("mouseover"), "the hover shows");
    s.run(
        r#"
        assert(anchored == 1, "default anchor fired")
        assert(recolored == 1, "UPDATE_MOUSEOVER_UNIT fired")
        assert(GameTooltip:IsShown())
        assert(GameTooltipTextLeft1:GetText() == "Timber Wolf")
        local _, mx = GameTooltipStatusBar:GetMinMaxValues()
        assert(mx == 50 and GameTooltipStatusBar:GetValue() == 30, "bar seeded from the snapshot")
    "#,
    )
    .unwrap();
    // The handler's seating resolved: bottom-right of the 800×600 screen, −13 in, 70 up.
    // (Answer the line measures first — the auto-size needs them before the plate has a rect.)
    let answers: Vec<(u32, f32, f32, u64)> = s
        .fontstrings_needing_measure()
        .iter()
        .map(|r| (r.id, 80.0, 10.0, r.key))
        .collect();
    s.set_measured_text_unwrapped(&answers);
    s.resolve();
    s.run(
        r#"assert(GameTooltip:GetRight() == 787 and GameTooltip:GetBottom() == 70,
                  "plate at the default corner, got " .. tostring(GameTooltip:GetRight()) .. "," .. tostring(GameTooltip:GetBottom()))"#,
    )
    .unwrap();
    // A health push for the LIVE token re-drives the bar without a rebuild.
    let mut hurt = wolf();
    hurt.health = 12;
    s.set_unit("mouseover", Some(hurt));
    s.run(
        r#"assert(GameTooltipStatusBar:GetValue() == 12, "the health watcher tracked the push")"#,
    )
    .unwrap();
    // Hover loss: the fade arms; past the ramp the tooltip hides.
    s.world_tooltip_fade();
    s.tick(0.6);
    s.run(r#"assert(not GameTooltip:IsShown(), "faded out after the ramp")"#)
        .unwrap();
    assert!(s.take_errors().is_empty());
}

/// The minimap BLIP tooltip (`minimap_tooltip`): refuses without a Minimap widget; with one, one
/// white line seated against the widget (the INTERIM ANCHOR_LEFT law), world-owned so the shared
/// fade arm hides it on hover loss.
#[test]
fn minimap_blip_tooltip_shows_and_fades() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"CreateFrame("GameTooltip", "GameTooltip")"#)
        .unwrap();
    assert!(
        !s.minimap_tooltip("Stormwind", 700.0, 500.0, false),
        "no UIParent yet: the show refuses"
    );
    s.run(
        r#"
        local up = CreateFrame("Frame", "UIParent")
        up:SetPoint("BOTTOMLEFT", 0, 0); up:SetWidth(800); up:SetHeight(600)
    "#,
    )
    .unwrap();
    assert!(s.minimap_tooltip("Stormwind", 795.0, 500.0, false));
    s.run(
        r#"
        assert(GameTooltipTextLeft1:GetText() == "Stormwind")
        assert(GameTooltip:IsShown(), "the blip tooltip shows")
    "#,
    )
    .unwrap();
    // Seated 5 px from the 800-wide screen's right edge: the clamp (the client's G bit4)
    // slides the plate back inside instead of letting it clip (director-caught).
    let answers: Vec<(u32, f32, f32, u64)> = s
        .fontstrings_needing_measure()
        .iter()
        .map(|r| (r.id, 80.0, 10.0, r.key))
        .collect();
    s.set_measured_text_unwrapped(&answers);
    s.resolve();
    s.run(
        r#"assert(GameTooltip:GetRight() <= 800, "clamped inside the screen, got " .. tostring(GameTooltip:GetRight()))"#,
    )
    .unwrap();
    // Follow: a move re-seats without rebuilding the line.
    s.world_tooltip_move(400.0, 300.0);
    s.run(r#"assert(GameTooltipTextLeft1:GetText() == "Stormwind", "content survives a move")"#)
        .unwrap();
    // Hover loss rides the shared world fade: armed, then hidden past the ramp.
    s.world_tooltip_fade();
    s.tick(0.6);
    s.run(r#"assert(not GameTooltip:IsShown(), "faded out after the ramp")"#)
        .unwrap();
    assert!(s.take_errors().is_empty());
}
