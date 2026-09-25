//! The guild windows (`FriendsFrame.xml:1719-3438`): the roster pane and its two views, the rank
//! editor, the guild-information board and the member detail card. A Lua fixture stands in for
//! the `script::guild` globals, seated before the XML loads, so only the windows are under test.
//! Its shapes are the bindings': 1.12 booleans are `1`/`nil`, the roster's `rankIndex` is 0-based
//! (0 = guild master) and the `GuildControl*` family is 1-based, off dropdown IDs.

use benilla_ui::script::{GuildState, ScriptValue, UiScript, UnitState};

/// One mutable table, `BenillaGuildFixture`, is the whole model: getters read it and verbs append
/// to `.calls`, drained by `BenillaGuildCalls()`. Tests mutate its fields and never replace it,
/// since the functions hold it as an upvalue. The members: the guild master (us), a rank-1
/// officer, and an offline member in the bottom rank.
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

fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // Before the XML: `FriendsFrame_OnLoad` reads `GetGuildRosterMOTD()` and
    // `GuildControlPopupFrame_OnLoad` reads `GuildControlGetRankFlags()`.
    s.run(GUILD_FIXTURE).unwrap();
    // The player is the fixture's guild master; the roster checks `UnitName("player") == name`.
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

/// A region's text colour to two places: colours are `f32`, so 0.82 reads back 0.8199999928474426.
fn colour(s: &UiScript, region: &str) -> (f64, f64, f64) {
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>(&format!("return {region}:GetTextColor()"))
        .unwrap_or_else(|e| panic!("{region}:GetTextColor() — {e}"));
    let round = |v: f64| (v * 100.0).round() / 100.0;
    (round(r), round(g), round(b))
}

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

    // The player view opens first: `FriendsFrame_OnLoad` sets `playerStatusFrame = 1`.
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

    assert_eq!(text(&s, "GuildFrameTotals"), "|cffffffff3|r Guild Members");
    assert_eq!(
        text(&s, "GuildFrameOnlineTotals"),
        "(|cffffffff2|r |cff00ff00Online|r)"
    );
}

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

    s.run("GuildFrameGuildListToggleButton:Click()").unwrap();
    assert!(visible(&s, "GuildPlayerStatusFrame"));
}

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

/// The rank comparisons of `FriendsFrame.lua:393-415`; with both arrows dead, they leave the card.
#[test]
fn the_detail_buttons_follow_the_rank_comparisons() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);

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

    s.run("GuildFrameButton3:Click()").unwrap();
    let _ = calls(&s);
    s.run("GuildFramePromoteButton:Click()").unwrap();
    assert_eq!(calls(&s), "GuildPromoteByName:Kaplan");
    assert!(
        !enabled(&s, "GuildFramePromoteButton"),
        "the arrow disables itself until the roster comes back"
    );
}

/// Without `CanViewOfficerNote` the pane goes and the card shrinks from 255 to 195
/// (`GUILD_DETAIL_OFFICER_HEIGHT`, `GUILD_DETAIL_NORM_HEIGHT`; `FriendsFrame.lua:370-390`).
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

    // Can view but not edit: grey text, a mouse-dead pane, no placeholder.
    s.run("BenillaGuildFixture.rights.editOfficerNote = nil; GuildStatus_Update()")
        .unwrap();
    assert_eq!(colour(&s, "OfficerNoteText"), (0.65, 0.65, 0.65));

    // Cannot view: the pane goes, and the card shrinks.
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

/// The guild message dialogs are 420 wide with the wide edit box (`StaticPopup.lua:1581-1587`).
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

    // The pane paints `CURRENT_GUILD_MOTD`, which only `GUILD_MOTD` updates after load.
    s.fire_event(
        "GUILD_MOTD",
        vec![ScriptValue::Str("Raid Thursday.".to_string())],
    );
    assert_eq!(text(&s, "GuildFrameNotesText"), "Raid Thursday.");
}

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

    // Add Member opens the narrow box: a name, not a sentence.
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

/// A checkbox's ID is the option index the engine maps to a bit.
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
        // `this:GetChecked()` passes straight through, and in 1.12 it is the number 1.
        "GuildControlSetRankFlag:5:1",
        "the checkbox's own ID is what reaches the engine"
    );
    assert!(enabled(&s, "GuildControlPopupAcceptButton"));

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

#[test]
fn switching_rank_reloads_the_buffer_and_disarms_accept() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    s.run("GuildFrameControlButton:Click()").unwrap();
    s.run("GuildControlPopupFrameCheckbox5:Click()").unwrap();
    assert!(enabled(&s, "GuildControlPopupAcceptButton"));

    // Through the real menu: a row's ID is the rank the buffer loads (`FriendsFrame.lua:864-871`).
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

/// Ten ranks is the ceiling (`FriendsFrame.lua:887`), and the last rank goes only once
/// `playersInBotRank` is zero (`:907-918`).
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

    // Remove shows only on the last rank, and only past five ranks.
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

/// The Show Offline checkbox is declared `virtual="true"` inside a `<Frames>` block
/// (`FriendsFrame.xml:1816`) and is still a real frame: only the top-level loader reads `virtual`
/// (`0x6ede10`), while `LoadChildFrames` (`0x76a060`) instantiates every child. Its click drops the
/// selection before re-filtering (`FriendsFrame.xml:1852`).
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

    s.run("ToggleGuildInfoFrame()").unwrap();
    s.run("BenillaGuildFixture.rights.editGuildInfo = 1")
        .unwrap();
    s.run("ToggleGuildInfoFrame()").unwrap();
    assert_eq!(text(&s, "GuildInfoEditBox"), "Click here to set message");
}

/// `FriendsFrame_OnHide` closes all three satellites (`FriendsFrame.lua:133-135`).
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

/// `GUILD_ROSTER_UPDATE` re-requests the roster only when `arg1` marks it stale
/// (`FriendsFrame.lua:646-653`); a header sort fires the event without it.
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

    // With the pane closed the event does nothing: the arm's visibility gate.
    s.run("HideUIPanel(FriendsFrame)").unwrap();
    let _ = calls(&s);
    s.fire_event("GUILD_ROSTER_UPDATE", vec![ScriptValue::Int(1)]);
    assert_eq!(calls(&s), "", "the pane is closed; nothing repaints");
}

/// `InGuildCheck` moves a guildless window off the guild tab (`FriendsFrame.lua:920-931`).
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

    // `ToggleFriendsFrame(3)` returns early while guildless (`FriendsFrame.lua:750`).
    s.run("ToggleFriendsFrame(3)").unwrap();
    assert!(!visible(&s, "GuildFrame"));
    assert!(visible(&s, "FriendsListFrame"));

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

/// The FRIEND menu's two guild rows are gated on `GuildFrame:IsVisible()`
/// (`UnitPopup.lua:323-330`), which keeps them off a friends-list or `/who` row.
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

    // GUILD_LEAVE is offered only on yourself.
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

#[test]
fn the_guild_rows_stay_off_a_who_row_menu() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    s.run("ShowWhoPanel()").unwrap();
    // Not our own name: on yourself the menu has no row to show and never opens, so the check
    // below would pass vacuously.
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

/// `UIParent_OnEvent` raises the invite dialog (`UIParent.lua:296-303`), social window shut or not.
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

/// The confirm names the member: the registry text carries a placeholder that OnShow replaces
/// (`StaticPopup.lua:903`, `:911`).
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

/// `GuildControlCheckboxUpdate` loops `for i=1, arg.n` (`FriendsFrame.lua:876`), so the binding
/// must push all thirteen returns even when every one is nil. Asks the real `script::guild`, not
/// the fixture.
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
    // Each is nil, not `false`: a 1.12 boolean, which `SetChecked` reads.
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

/// Asks the real globals over a snapshot with one right set. Checkboxes 5-8 own bits 0x80, 0x100,
/// 0x10 and 0x20, out of order, so a `1 << (i - 1)` table reads the wrong bit for each.
#[test]
fn each_permission_predicate_reads_the_bit_its_checkbox_owns() {
    let _data = benilla_formats::wow_data_or_skip!();
    // Checkbox 7, Invite Member, is bit 0x10, which a shift-based table gives checkbox 5, Promote.
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

    // Checkbox 5, Promote, is bit 0x80, which a shift-based table gives checkbox 8, Remove Member.
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

/// Only the guild-status view takes the tenth return, `status`, showing it in place of "Online"
/// (`FriendsFrame.lua:541`, `:548-551`); the other call sites take nine. Asks the real
/// `script::guild` with an empty roster, where only the count is evidence.
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

#[test]
fn every_list_in_the_window_takes_the_mouse_wheel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    open(&s);
    // The wheel handler is each list's scroll frame's, from `UIPanelScrollFrameTemplate`
    // (`UIPanelTemplates.xml:208`), not the pane's.
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
