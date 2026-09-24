//! The four guild windows (`FriendsFrame.xml` l.1719-3438 of the reference): the
//! roster pane and its two views, the rank editor, the guild-information notice board, and the
//! member detail card.
//!
//! What these guard that the Rust-side unit tests structurally cannot: the windows are Lua over an
//! engine snapshot, so a getter whose ten returns are in the wrong order, a row template whose
//! FontString is misnamed, a rank comparison written `>=` where the reference wrote `>`, or a
//! button wired to the wrong verb are all invisible to `script::guild`'s own tests and green in the
//! parse sweep. Each test below fails on exactly one of those.
//!
//! **The engine API is STOOD IN FOR here, deliberately.** `benilla-ui/src/script/guild.rs` binds
//! the ~47 real globals over a pushed snapshot; these tests replace all of them with the Lua
//! fixture below *before* the XML loads, so what is under test is the window and nothing else. It
//! also means the window's tests do not need the app's feed seated, and that a change to the
//! engine's plumbing cannot quietly turn one of these green. The fixture's shapes are the ones
//! `script::guild` promises, and two of them are easy to get wrong and worth restating:
//!
//! - **era booleans are `1`/`nil`, never `true`/`false`** — `IsInGuild`, `IsGuildLeader`, every
//!   `Can*`, and `online` out of `GetGuildRosterInfo`;
//! - **two index bases coexist**: the roster's `rankIndex` is **0-based** (0 = guild master), while
//!   the whole `GuildControl*` family is **1-based**, driven straight off dropdown IDs.

use benilla_ui::script::{GuildState, ScriptValue, UiScript, UnitState};

/// The guild engine API, stood in for in Lua (see the module header).
///
/// One mutable table, `BenillaGuildFixture`, is the whole model; every getter reads it and every
/// verb appends to `.calls`, which a test drains with `BenillaGuildCalls()`. Tests mutate the
/// table's FIELDS — never replace the table, since the closures below hold it as an upvalue.
///
/// The three seeded members are chosen to cover the rank comparisons the detail card turns on:
/// the guild master (ourselves), a rank-1 officer, and an offline member sitting in the BOTTOM
/// rank — which is also the one that blocks removing that rank in the editor.
const GUILD_FIXTURE: &str = r#"
BenillaGuildFixture = {
    inGuild = 1,
    isLeader = 1,
    guildName = "Legacy of Steel",
    myRankIndex = 0,
    motd = "Raid Tuesday at eight.",
    infoText = "Be excellent to each other.",
    selection = 0,
    showOffline = 1,
    ranks = { "Guild Master", "Officer", "Veteran", "Member", "Initiate", "Peon" },
    rights = {
        promote = 1, demote = 1, invite = 1, remove = 1, editMOTD = 1,
        editPublicNote = 1, viewOfficerNote = 1, editOfficerNote = 1, editGuildInfo = 1,
    },
    -- The 13-flag rank buffer, per 1-based rank. Only rank 1 is seeded; the others answer
    -- all-nil, which is the shape a never-loaded rank has.
    flags = { [1] = { 1, 1, 1, 1, nil, nil, 1, nil, 1, 1, 1, nil, nil } },
    controlRank = 1,
    calls = {},
    members = {
        { name = "Tigole", rank = "Guild Master", rankIndex = 0, level = 60, class = "Warrior",
          zone = "Ironforge", note = "", officernote = "", online = 1, status = "" },
        { name = "Furor", rank = "Officer", rankIndex = 1, level = 60, class = "Rogue",
          zone = "Orgrimmar", note = "raid lead", officernote = "trusted", online = 1,
          status = "<AFK>" },
        { name = "Kaplan", rank = "Peon", rankIndex = 5, level = 12, class = "Mage",
          zone = "Elwynn Forest", note = "alt", officernote = "", online = nil, status = "",
          lastOnline = { 0, 0, 3, 0 } },
    },
}

local F = BenillaGuildFixture

function BenillaGuildRecord(call)
    table.insert(F.calls, call)
end

-- Drain the recorded verbs as one "|"-joined string, so a test asserts on the whole sequence
-- rather than on "it happened at least once".
function BenillaGuildCalls()
    local out = table.concat(F.calls, "|")
    F.calls = {}
    return out
end

function IsInGuild() return F.inGuild end
function IsGuildLeader() return F.isLeader end

function GetGuildInfo(unit)
    if not F.inGuild then return nil end
    return F.guildName, F.ranks[F.myRankIndex + 1], F.myRankIndex
end

function GetNumGuildMembers() return table.getn(F.members) end

function GetGuildRosterInfo(index)
    local m = F.members[index]
    if not m then return nil end
    return m.name, m.rank, m.rankIndex, m.level, m.class, m.zone, m.note, m.officernote,
        m.online, m.status
end

function GetGuildRosterLastOnline(index)
    local m = F.members[index]
    if not m or not m.lastOnline then return 0, 0, 0, 0 end
    return m.lastOnline[1], m.lastOnline[2], m.lastOnline[3], m.lastOnline[4]
end

function GetGuildRosterMOTD() return F.motd end
function GetGuildRosterSelection() return F.selection end
function SetGuildRosterSelection(index)
    F.selection = index
    BenillaGuildRecord("SetGuildRosterSelection:" .. index)
end
function GetGuildRosterShowOffline() return F.showOffline end
function SetGuildRosterShowOffline(value)
    F.showOffline = value
    BenillaGuildRecord("SetGuildRosterShowOffline:" .. tostring(value))
end
function SortGuildRoster(field) BenillaGuildRecord("SortGuildRoster:" .. field) end
function GuildRoster() BenillaGuildRecord("GuildRoster") end

function GetGuildInfoText() return F.infoText end
function SetGuildInfoText(text)
    F.infoText = text
    BenillaGuildRecord("SetGuildInfoText:" .. text)
end
function GuildSetMOTD(text)
    F.motd = text
    BenillaGuildRecord("GuildSetMOTD:" .. text)
end
function GuildRosterSetPublicNote(index, text)
    BenillaGuildRecord("GuildRosterSetPublicNote:" .. index .. ":" .. text)
end
function GuildRosterSetOfficerNote(index, text)
    BenillaGuildRecord("GuildRosterSetOfficerNote:" .. index .. ":" .. text)
end

function GuildInviteByName(name) BenillaGuildRecord("GuildInviteByName:" .. name) end
function GuildUninviteByName(name) BenillaGuildRecord("GuildUninviteByName:" .. name) end
function GuildPromoteByName(name) BenillaGuildRecord("GuildPromoteByName:" .. name) end
function GuildDemoteByName(name) BenillaGuildRecord("GuildDemoteByName:" .. name) end
function GuildSetLeaderByName(name) BenillaGuildRecord("GuildSetLeaderByName:" .. name) end
function GuildLeave() BenillaGuildRecord("GuildLeave") end
function GuildDisband() BenillaGuildRecord("GuildDisband") end
function AcceptGuild() BenillaGuildRecord("AcceptGuild") end
function DeclineGuild() BenillaGuildRecord("DeclineGuild") end

function CanGuildPromote() return F.rights.promote end
function CanGuildDemote() return F.rights.demote end
function CanGuildInvite() return F.rights.invite end
function CanGuildRemove() return F.rights.remove end
function CanEditMOTD() return F.rights.editMOTD end
function CanEditPublicNote() return F.rights.editPublicNote end
function CanViewOfficerNote() return F.rights.viewOfficerNote end
function CanEditOfficerNote() return F.rights.editOfficerNote end
function CanEditGuildInfo() return F.rights.editGuildInfo end

function GuildControlGetNumRanks() return table.getn(F.ranks) end
function GuildControlGetRankName(index) return F.ranks[index] end
function GuildControlSetRank(index)
    F.controlRank = index
    BenillaGuildRecord("GuildControlSetRank:" .. index)
end
function GuildControlGetRankFlags()
    local f = F.flags[F.controlRank or 1] or {}
    return f[1], f[2], f[3], f[4], f[5], f[6], f[7], f[8], f[9], f[10], f[11], f[12], f[13]
end
function GuildControlSetRankFlag(index, on)
    local f = F.flags[F.controlRank or 1]
    if not f then
        f = {}
        F.flags[F.controlRank or 1] = f
    end
    if on then f[index] = 1 else f[index] = nil end
    BenillaGuildRecord("GuildControlSetRankFlag:" .. index .. ":" .. tostring(on))
end
function GuildControlSaveRank(name) BenillaGuildRecord("GuildControlSaveRank:" .. name) end
function GuildControlAddRank(name)
    table.insert(F.ranks, name)
    BenillaGuildRecord("GuildControlAddRank:" .. name)
end
function GuildControlDelRank(name) BenillaGuildRecord("GuildControlDelRank:" .. name) end
"#;

/// The window's own manifest slice, in `benilla.toc` order, with the fixture seated first.
///
/// `UIPanelTemplates.xml` is in the slice where `friends_tests` does not need it: the guild frames
/// inherit the reference's shared `UIPanelButtonTemplate` / `UIPanelCloseButton` /
/// `UIPanelScrollFrameTemplate` rather than a private copy, exactly as the reference's own XML
/// does. `BasicControls.xml` is here for `message()`, which `GuildControlCheckboxUpdate` calls on
/// a missing checkbox.
fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // Before the XML, so `FriendsFrame_OnLoad`'s `GetGuildRosterMOTD()` and
    // `GuildControlPopupFrame_OnLoad`'s `GuildControlGetRankFlags()` read the fixture.
    s.run(GUILD_FIXTURE).unwrap();
    // The roster's self-checks (`UnitName("player") == name`) need a seated player, and the
    // fixture makes us the guild master, so the names must agree.
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Tigole".into()),
            level: 60,
            ..UnitState::default()
        }),
    );
    super::test_ui::load_social_ui(&mut s);
    s
}

/// Open the window on the guild tab.
fn open(s: &UiScript) {
    s.run("ToggleFriendsFrame(3)").unwrap();
}

fn text(s: &UiScript, expr: &str) -> String {
    s.eval::<String>(&format!("return {expr}:GetText() or \"\""))
        .unwrap_or_else(|e| panic!("{expr}:GetText() — {e}"))
}

fn visible(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsVisible()"))
        .unwrap_or_else(|e| panic!("{frame}:IsVisible() — {e}"))
}

fn enabled(s: &UiScript, button: &str) -> bool {
    s.eval::<bool>(&format!("return {button}:IsEnabled() ~= 0"))
        .unwrap_or_else(|e| panic!("{button}:IsEnabled() — {e}"))
}

fn calls(s: &UiScript) -> String {
    s.eval::<String>("return BenillaGuildCalls()").unwrap()
}

/// A region's text colour, rounded to two places. The engine stores colours as `f32`, so 0.82 comes
/// back as 0.8199999928474426 and an exact compare would fail on every one of them.
fn colour(s: &UiScript, region: &str) -> (f64, f64, f64) {
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>(&format!("return {region}:GetTextColor()"))
        .unwrap_or_else(|e| panic!("{region}:GetTextColor() — {e}"));
    let round = |v: f64| (v * 100.0).round() / 100.0;
    (round(r), round(g), round(b))
}

/// The tab is live for a guilded character, the pane paints its four columns from the roster, the
/// title is the GUILD'S name rather than a constant, and showing the pane ASKS for the roster —
/// which the friend list never has to, because that one arrives unasked.
#[test]
fn the_guild_tab_opens_the_roster_and_asks_for_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    assert_eq!(
        s.eval::<i64>("return FriendsFrameTab3.isDisabled or 0")
            .unwrap(),
        0,
        "in a guild, the tab is live"
    );

    let _ = calls(&s);
    open(&s);
    assert!(visible(&s, "FriendsFrame"));
    assert!(visible(&s, "GuildFrame"));
    assert!(
        !visible(&s, "FriendsListFrame"),
        "the friends list is one of the four exclusive sub-frames"
    );
    assert_eq!(
        text(&s, "FriendsFrameTitleText"),
        "Legacy of Steel",
        "the guild tab's title is the guild's own name"
    );
    assert!(
        calls(&s).contains("GuildRoster"),
        "showing the pane requests the roster"
    );

    // The player view is the one that opens (FriendsFrame.playerStatusFrame starts 1).
    assert!(visible(&s, "GuildPlayerStatusFrame"));
    assert!(!visible(&s, "GuildStatusFrame"));

    assert_eq!(text(&s, "GuildFrameButton1Name"), "Tigole");
    assert_eq!(text(&s, "GuildFrameButton1Zone"), "Ironforge");
    assert_eq!(text(&s, "GuildFrameButton1Level"), "60");
    assert_eq!(
        text(&s, "GuildFrameButton1Class"),
        "Warrior",
        "the roster's ten returns in order — class is the fifth, not the fourth"
    );
    assert!(
        visible(&s, "GuildFrameButton3"),
        "three members, three rows"
    );
    assert!(
        !visible(&s, "GuildFrameButton4"),
        "rows past the roster are hidden"
    );

    // "|cffffffff3|r Guild Members" + "(|cffffffff2|r |cff00ff00Online|r)" — the plural form for
    // three, and the online count is a separate string beside it.
    assert_eq!(text(&s, "GuildFrameTotals"), "|cffffffff3|r Guild Members");
    assert_eq!(
        text(&s, "GuildFrameOnlineTotals"),
        "(|cffffffff2|r |cff00ff00Online|r)"
    );
}

/// An OFFLINE member keeps every column — level, class and zone all come down for offline members
/// too, unlike the friends list where an offline friend has no level at all — and only greys.
#[test]
fn an_offline_member_keeps_its_columns_and_only_greys() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    assert_eq!(text(&s, "GuildFrameButton3Name"), "Kaplan");
    assert_eq!(text(&s, "GuildFrameButton3Zone"), "Elwynn Forest");
    assert_eq!(
        text(&s, "GuildFrameButton3Level"),
        "12",
        "an offline member still reports a level"
    );
    assert_eq!(text(&s, "GuildFrameButton3Class"), "Mage");

    assert_eq!(
        colour(&s, "GuildFrameButton3Name"),
        (0.5, 0.5, 0.5),
        "offline rows go flat grey"
    );
    assert_eq!(
        colour(&s, "GuildFrameButton1Name"),
        (1.0, 0.82, 0.0),
        "an online name keeps the gold"
    );
}

/// The little page button between the headers and the list flips to the OTHER view — same 13 rows,
/// same scroll frame, different four columns — and its own label swaps to name the view it would
/// take you back to.
#[test]
fn the_page_button_flips_to_the_guild_status_view() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    assert_eq!(
        text(&s, "GuildFrameGuildListToggleButton"),
        "Show Player Status"
    );

    s.run("GuildFrameGuildListToggleButton:Click()").unwrap();
    assert!(visible(&s, "GuildStatusFrame"));
    assert!(!visible(&s, "GuildPlayerStatusFrame"));
    assert_eq!(
        text(&s, "GuildFrameGuildListToggleButton"),
        "Show Guild Status"
    );

    assert_eq!(text(&s, "GuildFrameGuildStatusButton2Name"), "Furor");
    assert_eq!(text(&s, "GuildFrameGuildStatusButton2Rank"), "Officer");
    assert_eq!(text(&s, "GuildFrameGuildStatusButton2Note"), "raid lead");
    assert_eq!(
        text(&s, "GuildFrameGuildStatusButton2Online"),
        "<AFK>",
        "an online member's STATUS tag replaces the plain Online label"
    );
    assert_eq!(
        text(&s, "GuildFrameGuildStatusButton1Online"),
        "Online",
        "…and an empty status falls back to it"
    );
    assert_eq!(
        text(&s, "GuildFrameGuildStatusButton3Online"),
        "3 days",
        "an offline member reports how long ago, coarsest unit only"
    );

    // …and back.
    s.run("GuildFrameGuildListToggleButton:Click()").unwrap();
    assert!(visible(&s, "GuildPlayerStatusFrame"));
}

/// `GuildFrame_GetLastOnline` takes the COARSEST non-zero unit and never composes two, and all
/// four zero reads "< an hour". Asserted through the function because the four arms are otherwise
/// only reachable by seeding four different members.
#[test]
fn the_last_online_formatter_takes_the_coarsest_unit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    let last = |years, months, days, hours| {
        s.run(&format!(
            "BenillaGuildFixture.members[3].lastOnline = {{ {years}, {months}, {days}, {hours} }}"
        ))
        .unwrap();
        s.eval::<String>("return GuildFrame_GetLastOnline(3)")
            .unwrap()
    };
    assert_eq!(last(0, 0, 0, 0), "< an hour");
    assert_eq!(last(0, 0, 0, 1), "1 hour");
    assert_eq!(last(0, 0, 0, 5), "5 hours");
    assert_eq!(
        last(0, 0, 1, 9),
        "1 day",
        "days outrank the hours beside them"
    );
    assert_eq!(last(0, 2, 4, 9), "2 months");
    assert_eq!(last(1, 2, 4, 9), "1 year");
    assert_eq!(last(3, 0, 0, 0), "3 years");
}

/// A left-click selects the row and opens the detail card; clicking that SAME row again closes it
/// and drops the selection, which is the only way back to "nothing selected".
#[test]
fn a_row_click_toggles_the_detail_card() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    assert!(!visible(&s, "GuildMemberDetailFrame"));

    s.run("GuildFrameButton2:Click()").unwrap();
    assert!(visible(&s, "GuildMemberDetailFrame"));
    assert_eq!(
        s.eval::<i64>("return GetGuildRosterSelection()").unwrap(),
        2
    );
    assert_eq!(text(&s, "GuildMemberDetailName"), "Furor");
    assert_eq!(text(&s, "GuildMemberDetailLevel"), "Level 60 Rogue");
    assert_eq!(text(&s, "GuildMemberDetailZoneText"), "Orgrimmar");
    assert_eq!(text(&s, "GuildMemberDetailRankText"), "Officer");
    assert_eq!(text(&s, "GuildMemberDetailOnlineText"), "Online");
    assert_eq!(text(&s, "PersonalNoteText"), "raid lead");
    assert_eq!(text(&s, "OfficerNoteText"), "trusted");

    s.run("GuildFrameButton2:Click()").unwrap();
    assert!(
        !visible(&s, "GuildMemberDetailFrame"),
        "the same row again closes the card"
    );
    assert_eq!(
        s.eval::<i64>("return GetGuildRosterSelection()").unwrap(),
        0,
        "…and clears the selection"
    );

    // A DIFFERENT row while the card is open re-points it rather than closing it.
    s.run("GuildFrameButton2:Click()").unwrap();
    s.run("GuildFrameButton3:Click()").unwrap();
    assert!(visible(&s, "GuildMemberDetailFrame"));
    assert_eq!(text(&s, "GuildMemberDetailName"), "Kaplan");
    assert_eq!(
        text(&s, "GuildMemberDetailOnlineText"),
        "3 days",
        "an offline member's card shows the last-online line, not Online"
    );
}

/// The card's four buttons are the rank law, and each of the reference's comparisons matters:
/// promote refuses to make a second-in-command your equal, demote refuses on the bottom rank, and
/// with BOTH arrows dead they leave the card rather than sitting greyed.
#[test]
fn the_detail_buttons_follow_the_rank_comparisons() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);

    // Row 2 — the officer directly below us. Nothing to promote him to; demote is fine.
    s.run("GuildFrameButton2:Click()").unwrap();
    assert!(
        !enabled(&s, "GuildFramePromoteButton"),
        "rankIndex 1 is already directly below the master"
    );
    assert!(enabled(&s, "GuildFrameDemoteButton"));
    assert!(
        visible(&s, "GuildFramePromoteButton"),
        "one of the pair live keeps BOTH on screen"
    );
    assert!(enabled(&s, "GuildMemberRemoveButton"));
    assert!(
        enabled(&s, "GuildMemberGroupInviteButton"),
        "an online guildmate can be invited"
    );

    // Row 3 — the bottom rank. Promotable, but there is nothing below to demote him to.
    s.run("GuildFrameButton3:Click()").unwrap();
    assert!(enabled(&s, "GuildFramePromoteButton"));
    assert!(
        !enabled(&s, "GuildFrameDemoteButton"),
        "the bottom rank has nowhere to fall"
    );
    assert!(
        !enabled(&s, "GuildMemberGroupInviteButton"),
        "…and he is offline"
    );

    // Row 1 — ourselves, the guild master. Every rank verb is dead, so the arrows go away.
    s.run("GuildFrameButton1:Click()").unwrap();
    assert!(!enabled(&s, "GuildFramePromoteButton"));
    assert!(!enabled(&s, "GuildFrameDemoteButton"));
    assert!(
        !visible(&s, "GuildFramePromoteButton"),
        "both dead → the arrows leave the card entirely"
    );
    assert!(!visible(&s, "GuildFrameDemoteButton"));
    assert!(
        !enabled(&s, "GuildMemberRemoveButton"),
        "you cannot remove yourself here"
    );
    assert!(
        !enabled(&s, "GuildMemberGroupInviteButton"),
        "nor invite yourself"
    );

    // The verbs address the selected NAME.
    s.run("GuildFrameButton3:Click()").unwrap();
    let _ = calls(&s);
    s.run("GuildFramePromoteButton:Click()").unwrap();
    assert_eq!(calls(&s), "GuildPromoteByName:Kaplan");
    assert!(
        !enabled(&s, "GuildFramePromoteButton"),
        "the arrow disables itself until the roster comes back"
    );
}

/// The officer-note pane is a THREE-state affair, and the third state RESIZES the card: a rank that
/// may not even see officer notes gets 60px less window rather than an empty pane.
#[test]
fn the_officer_note_pane_resizes_the_card() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameButton2:Click()").unwrap();
    assert!(visible(&s, "GuildMemberDetailOfficerNoteLabel"));
    assert_eq!(
        s.eval::<f64>("return GuildMemberDetailFrame:GetHeight()")
            .unwrap(),
        255.0
    );

    // May view but not edit: grey text, mouse-dead pane, and no edit-me placeholder.
    s.run("BenillaGuildFixture.rights.editOfficerNote = nil; GuildStatus_Update()")
        .unwrap();
    assert_eq!(colour(&s, "OfficerNoteText"), (0.65, 0.65, 0.65));

    // May not view at all: the pane goes, and the card shrinks.
    s.run("BenillaGuildFixture.rights.viewOfficerNote = nil; GuildStatus_Update()")
        .unwrap();
    assert!(!visible(&s, "GuildMemberDetailOfficerNoteLabel"));
    assert!(!visible(&s, "GuildMemberOfficerNoteBackground"));
    assert_eq!(
        s.eval::<f64>("return GuildMemberDetailFrame:GetHeight()")
            .unwrap(),
        195.0
    );
}

/// An EMPTY note a rank may edit shows the click-here invitation instead of nothing; the same note
/// with no edit right shows the empty string and a dead pane. Without this the only affordance on
/// an empty note is a blank rectangle nobody would click.
#[test]
fn an_editable_empty_note_invites_the_click() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameButton1:Click()").unwrap(); // Tigole's public note is ""
    assert_eq!(
        text(&s, "PersonalNoteText"),
        "Click here to set a Public Note."
    );
    assert_eq!(colour(&s, "PersonalNoteText"), (1.0, 1.0, 1.0));

    s.run("BenillaGuildFixture.rights.editPublicNote = nil; GuildStatus_Update()")
        .unwrap();
    assert_eq!(
        text(&s, "PersonalNoteText"),
        "",
        "no edit right → the empty note stays empty"
    );
    assert_eq!(colour(&s, "PersonalNoteText"), (0.65, 0.65, 0.65));
}

/// Clicking a note pane opens the WIDE-box dialog, prefilled with the note it edits, and accepting
/// sends it against the SELECTED roster index. This is the first customer of the popup engine's
/// `hasWideEditBox` (UiPanels.xml) and the only test that proves the 420 widen and the box swap.
#[test]
fn the_note_pane_opens_the_wide_dialog_and_sends_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameButton2:Click()").unwrap();
    let _ = calls(&s);

    s.run("GuildMemberNoteBackground:GetScript(\"OnMouseUp\")()")
        .unwrap();
    assert!(visible(&s, "StaticPopup1"));
    assert_eq!(text(&s, "StaticPopup1Text"), "Set Player Note:");
    assert!(
        visible(&s, "StaticPopup1WideEditBox"),
        "the wide box is the one that shows"
    );
    assert!(
        !visible(&s, "StaticPopup1EditBox"),
        "…and the narrow one is hidden, never both"
    );
    assert_eq!(
        s.eval::<f64>("return StaticPopup1:GetWidth()").unwrap(),
        420.0,
        "a guild message dialog is the wide one"
    );
    assert_eq!(
        text(&s, "StaticPopup1WideEditBox"),
        "raid lead",
        "prefilled with the note being edited"
    );

    s.run("StaticPopup1WideEditBox:SetText(\"main tank\")")
        .unwrap();
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(calls(&s), "GuildRosterSetPublicNote:2:main tank");
    assert!(!visible(&s, "StaticPopup1"), "accepting closes it");
}

/// The message of the day paints the CACHED value, is click-to-edit only for a rank that may set
/// it, and the GUILD_MOTD event repaints it without a roster round trip.
#[test]
fn the_motd_is_cached_click_to_edit_and_right_gated() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open(&s);
    assert_eq!(text(&s, "GuildFrameNotesText"), "Raid Tuesday at eight.");
    assert_eq!(
        text(&s, "GuildFrameNotesLabel"),
        "Guild Message Of The Day:"
    );
    assert!(enabled(&s, "GuildMOTDEditButton"));

    s.run("BenillaGuildFixture.rights.editMOTD = nil; GuildStatus_Update()")
        .unwrap();
    assert!(
        !enabled(&s, "GuildMOTDEditButton"),
        "without the right the MOTD is not clickable"
    );
    assert_eq!(colour(&s, "GuildFrameNotesText"), (0.65, 0.65, 0.65));

    s.run("BenillaGuildFixture.rights.editMOTD = 1; GuildStatus_Update()")
        .unwrap();
    let _ = calls(&s);
    s.run("GuildMOTDEditButton:Click()").unwrap();
    assert!(visible(&s, "StaticPopup1WideEditBox"));
    assert_eq!(
        text(&s, "StaticPopup1WideEditBox"),
        "Raid Tuesday at eight.",
        "the dialog opens on the cached MOTD"
    );
    s.run("StaticPopup1WideEditBox:SetText(\"Raid Wednesday.\")")
        .unwrap();
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(calls(&s), "GuildSetMOTD:Raid Wednesday.");

    // The event is what actually repaints the pane — the roster never carries it back.
    s.fire_event(
        "GUILD_MOTD",
        vec![ScriptValue::Str("Raid Thursday.".to_string())],
    );
    assert_eq!(text(&s, "GuildFrameNotesText"), "Raid Thursday.");
}

/// Guild Control is the guild MASTER's alone and Add Member follows the invite right — the two
/// pane-level buttons, both driven by the repaint rather than by a click.
#[test]
fn the_pane_buttons_follow_the_rights() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    assert!(enabled(&s, "GuildFrameControlButton"));
    assert!(enabled(&s, "GuildFrameAddMemberButton"));

    s.run("BenillaGuildFixture.isLeader = nil; BenillaGuildFixture.rights.invite = nil; GuildStatus_Update()")
        .unwrap();
    assert!(
        !enabled(&s, "GuildFrameControlButton"),
        "rank control is the master's alone"
    );
    assert!(!enabled(&s, "GuildFrameAddMemberButton"));

    // Add Member opens the name dialog, and it is the NARROW box (a name, not a sentence).
    s.run("BenillaGuildFixture.rights.invite = 1; GuildStatus_Update()")
        .unwrap();
    let _ = calls(&s);
    s.run("GuildFrameAddMemberButton:Click()").unwrap();
    assert!(visible(&s, "StaticPopup1EditBox"));
    assert!(!visible(&s, "StaticPopup1WideEditBox"));
    assert_eq!(
        s.eval::<f64>("return StaticPopup1:GetWidth()").unwrap(),
        320.0
    );
    s.run("StaticPopup1EditBox:SetText(\"Thrall\")").unwrap();
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(calls(&s), "GuildInviteByName:Thrall");
}

/// The rank editor loads its thirteen checkboxes from the rank BUFFER, opens with Accept dead, and
/// arms it on the first edit. The checkbox IDs are the option indices the engine maps to bits, so
/// the click has to carry the index it was declared with.
#[test]
fn the_rank_editor_loads_its_flags_and_arms_accept_on_an_edit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameControlButton:Click()").unwrap();
    assert!(visible(&s, "GuildControlPopupFrame"));
    assert!(
        !visible(&s, "GuildMemberDetailFrame"),
        "the three satellites are mutually exclusive"
    );

    assert_eq!(
        text(&s, "GuildControlPopupFrameCheckbox1Label"),
        "Guildchat Listen"
    );
    assert_eq!(
        text(&s, "GuildControlPopupFrameCheckbox13Label"),
        "Modify Guild Info",
        "all thirteen labels, in the reference's own checkbox order"
    );
    assert_eq!(text(&s, "GuildControlPopupFrameEditBox"), "Guild Master");

    let checked = |n: i32| {
        s.eval::<bool>(&format!(
            "return GuildControlPopupFrameCheckbox{n}:GetChecked() and true or false"
        ))
        .unwrap()
    };
    assert!(checked(1) && checked(4) && checked(7) && checked(11));
    assert!(!checked(5) && !checked(6) && !checked(8) && !checked(13));
    assert!(
        !enabled(&s, "GuildControlPopupAcceptButton"),
        "Accept is the buffer-is-dirty light; it opens dead"
    );

    let _ = calls(&s);
    s.run("GuildControlPopupFrameCheckbox5:Click()").unwrap();
    assert_eq!(
        calls(&s),
        // The Lua hands `this:GetChecked()` straight through, and in 1.12 that is the NUMBER 1
        // (1830) — so 1 is what the reference's own call carries too.
        "GuildControlSetRankFlag:5:1",
        "the checkbox's own ID is what reaches the engine"
    );
    assert!(enabled(&s, "GuildControlPopupAcceptButton"));

    // Accept flushes the buffer under the name in the box, then closes.
    s.run("GuildControlPopupFrameEditBox:SetText(\"Warchief\")")
        .unwrap();
    let _ = calls(&s);
    s.run("GuildControlPopupAcceptButton:Click()").unwrap();
    assert!(
        calls(&s).starts_with("GuildControlSaveRank:Warchief"),
        "one flush, carrying the edited name"
    );
    assert!(!visible(&s, "GuildControlPopupFrame"));
}

/// Picking another rank in the dropdown re-loads the buffer, repaints the boxes from THAT rank's
/// flags, and re-disarms Accept — the edit you were half-way through does not follow you.
#[test]
fn switching_rank_reloads_the_buffer_and_disarms_accept() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameControlButton:Click()").unwrap();
    s.run("GuildControlPopupFrameCheckbox5:Click()").unwrap();
    assert!(enabled(&s, "GuildControlPopupAcceptButton"));

    // Through the real menu, not the handler: the dropdown's rows ARE the ranks, so the row's own
    // ID is the rank the buffer loads, and calling the handler by hand would prove nothing about
    // that wiring.
    let _ = calls(&s);
    s.run("ToggleDropDownMenu(1, nil, GuildControlPopupFrameDropDown)")
        .unwrap();
    assert_eq!(
        text(&s, "DropDownList1Button3"),
        "Veteran",
        "the rows are the ranks, in rank order"
    );
    s.run("DropDownList1Button3:Click()").unwrap();
    assert!(
        calls(&s).contains("GuildControlSetRank:3"),
        "the buffer is re-loaded from rank 3"
    );
    assert_eq!(text(&s, "GuildControlPopupFrameEditBox"), "Veteran");
    assert!(
        !s.eval::<bool>("return GuildControlPopupFrameCheckbox1:GetChecked() and true or false")
            .unwrap(),
        "rank 3 has no flags seeded, so every box clears"
    );
    assert!(
        !enabled(&s, "GuildControlPopupAcceptButton"),
        "switching rank throws the half-made edit away"
    );
}

/// The add/remove rank buttons: ten is the ceiling, and the last rank can only be removed once it
/// is EMPTY — which is why the repaint counts `playersInBotRank` at all.
#[test]
fn the_rank_buttons_follow_the_count_and_the_bottom_rank() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameControlButton:Click()").unwrap();

    s.run("GuildControlPopupFrameAddRankButton_OnUpdate()")
        .unwrap();
    assert!(
        enabled(&s, "GuildControlPopupFrameAddRankButton"),
        "six ranks, room for four more"
    );
    s.run("BenillaGuildFixture.ranks = { \"a\",\"b\",\"c\",\"d\",\"e\",\"f\",\"g\",\"h\",\"i\",\"j\" }")
        .unwrap();
    s.run("GuildControlPopupFrameAddRankButton_OnUpdate()")
        .unwrap();
    assert!(
        !enabled(&s, "GuildControlPopupFrameAddRankButton"),
        "ten is the ceiling"
    );
    s.run("BenillaGuildFixture.ranks = { \"Guild Master\",\"Officer\",\"Veteran\",\"Member\",\"Initiate\",\"Peon\" }")
        .unwrap();

    // Remove only shows on the LAST rank, and only past five ranks.
    s.run("UIDropDownMenu_SetSelectedID(GuildControlPopupFrameDropDown, 1); GuildControlPopupFrameRemoveRankButton_OnUpdate()")
        .unwrap();
    assert!(
        !visible(&s, "GuildControlPopupFrameRemoveRankButton"),
        "you can only ever remove the last rank"
    );
    s.run("UIDropDownMenu_SetSelectedID(GuildControlPopupFrameDropDown, 6); GuildControlPopupFrameRemoveRankButton_OnUpdate()")
        .unwrap();
    assert!(visible(&s, "GuildControlPopupFrameRemoveRankButton"));
    assert!(
        !enabled(&s, "GuildControlPopupFrameRemoveRankButton"),
        "Kaplan still sits in the bottom rank"
    );

    // Move him up and repaint: the counter falls to zero and the button arms.
    s.run("BenillaGuildFixture.members[3].rankIndex = 4; GuildStatus_Update()")
        .unwrap();
    s.run("GuildControlPopupFrameRemoveRankButton_OnUpdate()")
        .unwrap();
    assert!(enabled(&s, "GuildControlPopupFrameRemoveRankButton"));

    let _ = calls(&s);
    s.run("GuildControlPopupFrameRemoveRankButton:Click()")
        .unwrap();
    assert!(
        calls(&s).starts_with("GuildControlDelRank:Peon"),
        "the LAST rank's name is what goes"
    );
}

/// Every column header sorts by its own key, in BOTH views — eight headers over one roster, and a
/// sort is a repaint, never a re-request.
#[test]
fn the_column_headers_sort_by_their_own_keys() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    for (header, key) in [
        ("GuildFrameColumnHeader1", "name"),
        ("GuildFrameColumnHeader2", "zone"),
        ("GuildFrameColumnHeader3", "level"),
        ("GuildFrameColumnHeader4", "class"),
    ] {
        let _ = calls(&s);
        s.run(&format!("{header}:Click()")).unwrap();
        assert_eq!(calls(&s), format!("SortGuildRoster:{key}"));
    }

    s.run("GuildFrameGuildListToggleButton:Click()").unwrap();
    for (header, key) in [
        ("GuildFrameGuildStatusColumnHeader1", "name"),
        ("GuildFrameGuildStatusColumnHeader2", "rank"),
        ("GuildFrameGuildStatusColumnHeader3", "note"),
        ("GuildFrameGuildStatusColumnHeader4", "online"),
    ] {
        let _ = calls(&s);
        s.run(&format!("{header}:Click()")).unwrap();
        assert_eq!(calls(&s), format!("SortGuildRoster:{key}"));
    }
}

/// **The Show Offline Members checkbox EXISTS.** The reference declares it `virtual="true"` inside
/// a `<Frames>` block, which reads like "this is a template, do not build it" — and is not: the
/// `virtual` attribute is only consulted by the top-level file loader (`0x6ede10`), while a
/// `<Frames>` child goes straight to the instantiator via `LoadChildFrames 0x76a060`. So the box
/// is a real frame in the reference and must be one here.
///
/// Its click also DROPS THE SELECTION before re-filtering: the roster is about to be re-ordered,
/// so index 7 will not be the member index 7 was.
#[test]
fn the_show_offline_checkbox_is_real_and_drops_the_selection() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    assert!(
        s.eval::<bool>("return GuildFrameLFGButton ~= nil").unwrap(),
        "the reference's `virtual=` on a <Frames> child does not suppress the frame"
    );
    assert_eq!(text(&s, "GuildFrameLFGButtonText"), "Show Offline Members");

    s.run("GuildFrameButton2:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetGuildRosterSelection()").unwrap(),
        2
    );

    let _ = calls(&s);
    s.run("GuildFrameLFGButton:Click()").unwrap();
    let seen = calls(&s);
    assert!(
        seen.starts_with("SetGuildRosterSelection:0"),
        "the selection is dropped FIRST, before the filter changes: {seen}"
    );
    assert!(
        seen.contains("SetGuildRosterShowOffline:"),
        "…and the filter really is pushed: {seen}"
    );
}

/// The guild-information board is read-only without the right: grey text, a dead Accept, and the
/// stored text rather than the click-here invitation.
#[test]
fn the_guild_information_board_is_right_gated() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameGuildInformationButton:Click()").unwrap();
    assert!(visible(&s, "GuildInfoFrame"));
    assert_eq!(text(&s, "GuildInfoEditBox"), "Be excellent to each other.");
    assert!(enabled(&s, "GuildInfoSaveButton"));

    let _ = calls(&s);
    s.run("GuildInfoEditBox:SetText(\"Read the rules.\")")
        .unwrap();
    s.run("GuildInfoSaveButton:Click()").unwrap();
    let seen = calls(&s);
    assert!(
        seen.starts_with("SetGuildInfoText:Read the rules."),
        "{seen}"
    );
    assert!(
        seen.contains("GuildRoster"),
        "saving asks for the roster back: {seen}"
    );
    assert!(!visible(&s, "GuildInfoFrame"), "…and closes");

    // Without the right: grey, dead, and no placeholder even when empty.
    s.run("BenillaGuildFixture.rights.editGuildInfo = nil; BenillaGuildFixture.infoText = \"\"")
        .unwrap();
    s.run("ToggleGuildInfoFrame()").unwrap();
    assert!(visible(&s, "GuildInfoFrame"));
    assert_eq!(text(&s, "GuildInfoEditBox"), "");
    assert!(!enabled(&s, "GuildInfoSaveButton"));
    assert_eq!(colour(&s, "GuildInfoEditBox"), (0.65, 0.65, 0.65));

    // …and with the right, an empty board shows the invitation.
    s.run("ToggleGuildInfoFrame()").unwrap();
    s.run("BenillaGuildFixture.rights.editGuildInfo = 1")
        .unwrap();
    s.run("ToggleGuildInfoFrame()").unwrap();
    assert_eq!(text(&s, "GuildInfoEditBox"), "Click here to set message");
}

/// The three satellite windows are mutually exclusive — each one's opener shuts the other two —
/// and closing the social window takes all three with it, since none of them is its child.
#[test]
fn the_three_satellites_are_exclusive_and_close_with_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameButton2:Click()").unwrap();
    assert!(visible(&s, "GuildMemberDetailFrame"));

    s.run("GuildFrameGuildInformationButton:Click()").unwrap();
    assert!(visible(&s, "GuildInfoFrame"));
    assert!(!visible(&s, "GuildMemberDetailFrame"));

    s.run("GuildFrameControlButton:Click()").unwrap();
    assert!(visible(&s, "GuildControlPopupFrame"));
    assert!(!visible(&s, "GuildInfoFrame"));

    s.run("GuildFrameButton2:Click()").unwrap();
    assert!(visible(&s, "GuildMemberDetailFrame"));
    assert!(!visible(&s, "GuildControlPopupFrame"));

    s.run("HideUIPanel(FriendsFrame)").unwrap();
    assert!(!visible(&s, "GuildMemberDetailFrame"));
    assert!(!visible(&s, "GuildControlPopupFrame"));
    assert!(!visible(&s, "GuildInfoFrame"));
    assert!(!visible(&s, "GuildFrame"));
}

/// `GUILD_ROSTER_UPDATE`'s `arg1` is the STALE flag, and it is the difference between a repaint and
/// a wire round-trip: a column-header sort fires this event without it, and re-requesting there
/// would put a server trip behind every click on a header.
#[test]
fn the_roster_event_only_re_requests_when_told_the_roster_is_stale() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open(&s);

    let _ = calls(&s);
    s.fire_event("GUILD_ROSTER_UPDATE", vec![]);
    assert!(
        !calls(&s).contains("GuildRoster"),
        "a plain repaint must not ask the server again"
    );

    s.fire_event("GUILD_ROSTER_UPDATE", vec![ScriptValue::Int(1)]);
    assert!(
        calls(&s).contains("GuildRoster"),
        "…but a STALE roster is re-requested"
    );

    // With the pane closed the event does nothing at all — the arm's own visibility gate.
    s.run("HideUIPanel(FriendsFrame)").unwrap();
    let _ = calls(&s);
    s.fire_event("GUILD_ROSTER_UPDATE", vec![ScriptValue::Int(1)]);
    assert_eq!(calls(&s), "", "the pane is closed; nothing repaints");
}

/// Leaving the guild while STANDING on the guild tab falls back to tab 1 rather than leaving a
/// pane with no guild behind it on screen — the half of `InGuildCheck` that is easy to omit and
/// impossible to notice until it happens.
#[test]
fn losing_the_guild_falls_back_off_the_guild_tab() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open(&s);
    assert!(visible(&s, "GuildFrame"));

    s.run("BenillaGuildFixture.inGuild = nil").unwrap();
    s.fire_event("PLAYER_GUILD_UPDATE", vec![]);
    assert_eq!(
        s.eval::<i64>("return FriendsFrameTab3.isDisabled or 0")
            .unwrap(),
        1,
        "the tab greys again"
    );
    assert!(!visible(&s, "GuildFrame"), "the pane goes");
    assert!(
        visible(&s, "FriendsListFrame"),
        "and the window falls back to the friends list"
    );

    // The tab also refuses to be selected by number while guildless (the second lock).
    s.run("ToggleFriendsFrame(3)").unwrap();
    assert!(!visible(&s, "GuildFrame"));
    assert!(visible(&s, "FriendsListFrame"));

    // Joining one re-arms it.
    s.run("BenillaGuildFixture.inGuild = 1").unwrap();
    s.fire_event("PLAYER_GUILD_UPDATE", vec![]);
    assert_eq!(
        s.eval::<i64>("return FriendsFrameTab3.isDisabled or 0")
            .unwrap(),
        0
    );
    s.run("FriendsFrameTab3:Click()").unwrap();
    assert!(
        visible(&s, "GuildFrame"),
        "and the tab's own OnClick opens the pane — it had none at all before this arc"
    );
}

/// A right-click on a roster row opens the shared FRIEND menu with the two GUILD rows live. They
/// are gated on this pane being visible, which is what keeps them off a friends-list or /who row.
#[test]
fn right_clicking_a_roster_row_offers_the_guild_rows() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameButton2:Click(\"RightButton\")").unwrap();
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the menu opens"
    );
    assert_eq!(
        s.eval::<String>("return FriendsDropDown.name").unwrap(),
        "Furor",
        "addressed by name — a roster row has no unit token behind it"
    );

    let row = |value: &str| {
        format!(
            r#"
            for i = 1, UIDROPDOWNMENU_MAXBUTTONS do
                local b = getglobal("DropDownList1Button" .. i)
                if b and b:IsVisible() and b.value == "{value}" then b:Click() return 1 end
            end
            return nil"#
        )
    };
    let _ = calls(&s);
    assert_eq!(
        s.eval::<Option<i64>>(&row("GUILD_PROMOTE")).unwrap(),
        Some(1),
        "the guild master sees Promote on someone else's row"
    );
    assert!(visible(&s, "StaticPopup1"));
    assert_eq!(
        text(&s, "StaticPopup1Text"),
        "Really promote Furor to Guildmaster?"
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(calls(&s), "GuildSetLeaderByName:Furor");

    // GUILD_LEAVE is only ever offered on YOURSELF…
    s.run("GuildFrameButton1:Click(\"RightButton\")").unwrap();
    assert_eq!(
        s.eval::<Option<i64>>(&row("GUILD_PROMOTE")).unwrap(),
        None,
        "…and Promote never is"
    );
    s.run("GuildFrameButton1:Click(\"RightButton\")").unwrap();
    let _ = calls(&s);
    assert_eq!(s.eval::<Option<i64>>(&row("GUILD_LEAVE")).unwrap(), Some(1));
    assert_eq!(
        text(&s, "StaticPopup1Text"),
        "Really leave Legacy of Steel?"
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(calls(&s), "GuildLeave");
}

/// The same two rows are ABSENT from a `/who` row's menu, which opens the identical FRIEND menu.
/// That gate is `GuildFrame:IsVisible()`, and without it a guild verb would sit on every name in
/// the game.
#[test]
fn the_guild_rows_stay_off_a_who_row_menu() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    s.run("ShowWhoPanel()").unwrap();
    // NOT our own name: the WHISPER row hides on yourself, and with INVITE gated the same way the
    // menu would have nothing to show and never open — which would make the assertion below pass
    // for the wrong reason.
    s.run("FriendsFrame_ShowDropdown(\"Thrall\", 1)").unwrap();
    assert!(s.eval::<bool>("return DropDownList1:IsVisible()").unwrap());
    let present = r#"
        for i = 1, UIDROPDOWNMENU_MAXBUTTONS do
            local b = getglobal("DropDownList1Button" .. i)
            if b and b:IsVisible() and (b.value == "GUILD_LEAVE" or b.value == "GUILD_PROMOTE") then
                return 1
            end
        end
        return nil"#;
    assert_eq!(
        s.eval::<Option<i64>>(present).unwrap(),
        None,
        "the guild pane is not up, so neither guild row is"
    );
}

/// A guild invite raises the accept/decline dialog wherever you are — it does not need the social
/// window to have ever been opened, which is why it rides its own hidden driver frame.
#[test]
fn a_guild_invite_raises_its_dialog_without_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    assert!(!visible(&s, "FriendsFrame"), "the window is shut");

    let _ = calls(&s);
    s.fire_event(
        "GUILD_INVITE_REQUEST",
        vec![
            ScriptValue::Str("Furor".to_string()),
            ScriptValue::Str("Legacy of Steel".to_string()),
        ],
    );
    assert!(visible(&s, "StaticPopup1"));
    assert_eq!(
        text(&s, "StaticPopup1Text"),
        "Furor invites you to join Legacy of Steel"
    );
    assert_eq!(text(&s, "StaticPopup1Button1"), "Accept");
    assert_eq!(text(&s, "StaticPopup1Button2"), "Decline");

    s.run("StaticPopup1Button2:Click()").unwrap();
    assert_eq!(
        calls(&s),
        "DeclineGuild",
        "Cancel DECLINES, it does not drop"
    );

    s.fire_event(
        "GUILD_INVITE_REQUEST",
        vec![
            ScriptValue::Str("Furor".to_string()),
            ScriptValue::Str("Legacy of Steel".to_string()),
        ],
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(calls(&s), "AcceptGuild");

    // A withdrawn invite takes its dialog with it.
    s.fire_event(
        "GUILD_INVITE_REQUEST",
        vec![
            ScriptValue::Str("Furor".to_string()),
            ScriptValue::Str("Legacy of Steel".to_string()),
        ],
    );
    assert!(visible(&s, "StaticPopup1"));
    s.fire_event("GUILD_INVITE_CANCEL", vec![]);
    assert!(!visible(&s, "StaticPopup1"));
}

/// Removing a member goes behind a confirm that NAMES them — the registry line carries a
/// placeholder and OnShow splices the real name in, which is the one dialog in this file whose
/// text is rewritten on show.
#[test]
fn removing_a_member_names_them_in_the_confirm() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameButton3:Click()").unwrap();
    let _ = calls(&s);

    s.run("GuildMemberRemoveButton:Click()").unwrap();
    assert_eq!(
        text(&s, "StaticPopup1Text"),
        "Are you sure you want to remove Kaplan from the guild?"
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(calls(&s), "GuildUninviteByName:Kaplan");
    assert!(
        !visible(&s, "GuildMemberDetailFrame"),
        "the card goes with the member"
    );
}

/// **`GuildControlGetRankFlags` really answers THIRTEEN values, and the count survives the
/// 5.0→5.1 vararg swap.** The one place the window and the engine are coupled by *arity* rather
/// than by a name.
///
/// The reference walks the 5.0 vararg table with `for i = 1, arg.n`, and that stock file is the
/// one this window loads, so the checkbox loop's bound *is* whatever the binding pushed. The two
/// agree only while every one of the thirteen returns is actually pushed — and with no rank
/// loaded, ALL THIRTEEN ARE NIL (era booleans are `1`/`nil`).
/// A binding that returned "as many values as are true" would look identical at every other
/// assertion in this file, load without an error, and leave checkboxes stale from the last rank.
/// So this one deliberately does NOT install the fixture: it asks the real `script::guild`.
#[test]
fn the_rank_flags_binding_answers_thirteen_values_even_when_all_are_nil() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    assert_eq!(
        s.arity("GuildControlGetRankFlags()").unwrap(),
        13,
        "an unloaded buffer is all-nil, and every one of the thirteen must still be pushed — \
         `GuildControlCheckboxUpdate` drives checkbox i off argument i and nothing else"
    );
    // …and each of them is nil, not `false`: the era boolean law, which `SetChecked` reads.
    assert_eq!(
        s.eval::<i64>(
            "local function count(...) \
                 local n = 0 \
                 for i = 1, 13 do if arg[i] ~= nil then n = n + 1 end end \
                 return n \
             end \
             return count(GuildControlGetRankFlags())"
        )
        .unwrap(),
        0
    );
}

/// **Each `Can*` predicate reads the bit its checkbox owns, end to end.**
///
/// `RANK_RIGHT_BITS`' own unit test asserts the table as *data*. That is necessary and not
/// sufficient: a wrong table is only a bug because the predicates read it, and a mutation that
/// renumbered Promote/Demote/Invite/Remove into the naive contiguous layout left every window test
/// in this file green — they drive the fixture's stand-ins, not the engine's predicates. So this
/// one pushes a real snapshot with exactly ONE right set and asks the real globals.
///
/// The four it checks are precisely the four the naive `1 << (i - 1)` layout would swap, which is
/// what makes it a falsifier rather than a restatement.
#[test]
fn each_permission_predicate_reads_the_bit_its_checkbox_owns() {
    let _data = benilla_formats::wow_data_or_skip!();
    // Index 7, "Invite Member" — bit 0x10. A shift-based table would put 0x10 at index 5,
    // "Promote", so this single word separates the two layouts in both directions.
    let mut s = UiScript::new().unwrap();
    s.set_guild(GuildState {
        in_guild: true,
        rights: 0x0000_0010,
        ..Default::default()
    });
    assert_eq!(
        s.eval::<i64>("return CanGuildInvite() and 1 or 0").unwrap(),
        1,
        "0x10 is Invite Member (checkbox 7)"
    );
    for global in ["CanGuildPromote", "CanGuildDemote", "CanGuildRemove"] {
        assert_eq!(
            s.eval::<i64>(&format!("return {global}() and 1 or 0"))
                .unwrap(),
            0,
            "{global} must not read 0x10 — that is Invite's bit, and confusing the two is exactly \
             what the non-monotonic table exists to prevent"
        );
    }

    // Index 5, "Promote" — bit 0x80, which the naive layout would call Remove Member.
    s.set_guild(GuildState {
        in_guild: true,
        rights: 0x0000_0080,
        ..Default::default()
    });
    assert_eq!(
        s.eval::<i64>("return CanGuildPromote() and 1 or 0")
            .unwrap(),
        1,
        "0x80 is Promote (checkbox 5), NOT Remove Member"
    );
    assert_eq!(
        s.eval::<i64>("return CanGuildRemove() and 1 or 0").unwrap(),
        0
    );

    // And a guildless player holds no right at all, whatever the stale word says.
    s.set_guild(GuildState {
        in_guild: false,
        rights: u32::MAX,
        ..Default::default()
    });
    for global in ["CanGuildInvite", "CanGuildPromote", "CanEditMOTD"] {
        assert_eq!(
            s.eval::<i64>(&format!("return {global}() and 1 or 0"))
                .unwrap(),
            0,
            "{global} is false for a guildless player regardless of the rights word"
        );
    }
}

/// **`GetGuildRosterInfo` really answers TEN values, and the tenth is `status`.** The second
/// place the window and the engine are coupled by arity, and the one that nearly shipped wrong.
///
/// Six of the reference's seven call sites destructure only nine (`FriendsFrame.lua:344`, `:447`,
/// `:479`, `:709`, `StaticPopup.lua:1008`, `:1045`). Only the player-status view takes the tenth
/// (`:541`) and branches on `status == ""` to choose between the "Online" label and the
/// `<AFK>`/`<DND>` tag (`:548-551`). So a nine-value binding leaves exactly one column blank while
/// every other assertion in this file — and every other window — still passes.
///
/// Like the rank-flags falsifier above, this deliberately does NOT install the fixture: the
/// fixture returns ten by construction, so asking it would prove nothing. It asks the real
/// `script::guild`, with an empty roster, where every return is nil and only the *count* is
/// evidence.
#[test]
fn the_roster_binding_answers_ten_values_and_the_tenth_is_status() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    assert_eq!(
        s.arity("GetGuildRosterInfo(1)").unwrap(),
        10,
        "an out-of-range index still pushes all ten — the reference calls this with \
         GetGuildRosterSelection(), which is 0 whenever nothing is selected, on every \
         GuildStatus_Update pass before it ever checks `> 0`"
    );
    assert_eq!(
        s.arity("GetGuildRosterInfo(0)").unwrap(),
        10,
        "…including index 0, the nothing-selected case the reference passes unguarded"
    );
}

/// The whole slice runs clean: no Lua error reaches the session's error sink through any of the
/// paths above. A handler that raises mid-way still leaves the frames it already touched looking
/// right, so an assertion on the look is not enough on its own.
#[test]
fn driving_the_guild_windows_raises_no_script_errors() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open(&s);
    s.run("GuildFrameButton1:Click()").unwrap();
    s.run("GuildFrameGuildListToggleButton:Click()").unwrap();
    s.run("GuildFrameGuildStatusButton2:Click()").unwrap();
    s.run("GuildFrameGuildListToggleButton:Click()").unwrap();
    s.run("GuildFrameControlButton:Click()").unwrap();
    s.run("GuildControlPopupFrameCheckbox2:Click()").unwrap();
    s.run("GuildControlPopupFrameCancelButton:Click()").unwrap();
    s.run("GuildFrameGuildInformationButton:Click()").unwrap();
    s.run("GuildInfoCancelButton:Click()").unwrap();
    s.fire_event("GUILD_ROSTER_UPDATE", vec![ScriptValue::Int(1)]);
    s.fire_event("GUILD_MOTD", vec![ScriptValue::Str("hi".to_string())]);
    s.run("HideUIPanel(FriendsFrame)").unwrap();
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Every list in this window scrolls with the wheel — friends, ignore, who AND guild.
///
/// The guild pane shipped without its `OnMouseWheel` while the comment three lines above its own
/// scroll frame said the list was "driven by `updateFunc` + the pane's OnMouseWheel", and while
/// its three siblings each carried one. A gap that the surrounding prose asserts is closed is
/// invisible to reading, so this asserts the WIRING rather than the comment.
#[test]
fn every_list_in_the_window_takes_the_mouse_wheel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    // The reference's wheel lives on each list's FauxScrollFrameTemplate scroll frame, not on
    // the pane (1959).
    for frame in [
        "FriendsFrameFriendsScrollFrame",
        "FriendsFrameIgnoreScrollFrame",
        "WhoListScrollFrame",
        "GuildListScrollFrame",
    ] {
        assert!(
            s.eval::<bool>(&format!(
                "return {frame}:GetScript(\"OnMouseWheel\") ~= nil"
            ))
            .unwrap(),
            "{frame} has no OnMouseWheel — its list cannot be scrolled with the wheel"
        );
    }
}
