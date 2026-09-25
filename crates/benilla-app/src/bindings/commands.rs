//! The binding command registry: every command benilla implements, in 1.12 `Bindings.xml` order,
//! with the stock default chords (`WTF\DefaultBindings.wtf`) and each command's dispatch class.
//! A command appears only over a real engine action; the rest are in [`ABSENT`], and the two
//! together are exactly the client's 228 live bindings. Labels and headers are the 1.12
//! `BINDING_NAME_*`/`BINDING_HEADER_*` GlobalStrings.
//!
//! Dispatch classes:
//! - [`Kind::Held`]: press latches, base-key release unlatches (the `runOnUp` movement pairs);
//!   engine systems read the latch ([`super::BindingsState`]).
//! - [`Kind::Edge`] / [`Kind::EdgeUpDown`]: runs the reference's binding body as Lua.
//! - [`Kind::Host`]: fires into [`super::BindingsState::fired`] for an engine system.

/// A command's index into [`SPECS`], the engine-side handle (`cmd::JUMP`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Cmd(pub u16);

pub(crate) enum Kind {
    /// Latched while the base key is held; engine systems read the state.
    Held,
    /// Fires this Lua on the matching press.
    Edge(&'static str),
    /// The reference's `runOnUp` button pair: Lua on press, Lua on base-key release.
    EdgeUpDown(&'static str, &'static str),
    /// Fires into [`super::BindingsState`]; an engine system consumes it.
    Host,
}

pub(crate) struct Spec {
    pub name: &'static str,
    /// The category header's global-string key (`BINDING_HEADER_MOVEMENT`).
    pub category: &'static str,
    pub kind: Kind,
    /// The 1.12 default chords; `None` is shipped unbound.
    pub d1: Option<&'static str>,
    pub d2: Option<&'static str>,
}

impl Spec {
    /// The reference's `<Binding runOnUp=…>`; its one reader (`0x4b7bf1`) gates the release half
    /// and nothing else.
    pub(crate) fn run_on_up(&self) -> bool {
        matches!(self.kind, Kind::Held | Kind::EdgeUpDown(..))
    }
}

macro_rules! spec {
    ($name:literal, $cat:ident, $kind:expr, $d1:expr, $d2:expr) => {
        Spec {
            name: $name,
            category: concat!("BINDING_HEADER_", stringify!($cat)),
            kind: $kind,
            d1: $d1,
            d2: $d2,
        }
    };
}

/// Engine-side handles for the host-dispatched commands, as indexes into [`SPECS`].
pub(crate) mod cmd {
    use super::{Cmd, TABLE};

    /// Resolve a command name to its [`Cmd`] at compile time; an unknown name is a build error.
    const fn by_name(name: &str) -> Cmd {
        let mut i = 0;
        while i < TABLE.len() {
            if const_str_eq(TABLE[i].name, name) {
                return Cmd(i as u16);
            }
            i += 1;
        }
        panic!("no such binding command")
    }

    const fn const_str_eq(a: &str, b: &str) -> bool {
        let (a, b) = (a.as_bytes(), b.as_bytes());
        if a.len() != b.len() {
            return false;
        }
        let mut i = 0;
        while i < a.len() {
            if a[i] != b[i] {
                return false;
            }
            i += 1;
        }
        true
    }

    pub(crate) const MOVE_AND_STEER: Cmd = by_name("MOVEANDSTEER");
    pub(crate) const MOVE_FORWARD: Cmd = by_name("MOVEFORWARD");
    pub(crate) const MOVE_BACKWARD: Cmd = by_name("MOVEBACKWARD");
    pub(crate) const TURN_LEFT: Cmd = by_name("TURNLEFT");
    pub(crate) const TURN_RIGHT: Cmd = by_name("TURNRIGHT");
    pub(crate) const STRAFE_LEFT: Cmd = by_name("STRAFELEFT");
    pub(crate) const STRAFE_RIGHT: Cmd = by_name("STRAFERIGHT");
    pub(crate) const JUMP: Cmd = by_name("JUMP");
    pub(crate) const SIT_OR_STAND: Cmd = by_name("SITORSTAND");
    pub(crate) const TOGGLE_SHEATH: Cmd = by_name("TOGGLESHEATH");
    pub(crate) const TOGGLE_AUTORUN: Cmd = by_name("TOGGLEAUTORUN");
    pub(crate) const TOGGLE_RUN: Cmd = by_name("TOGGLERUN");
    pub(crate) const TARGET_NEAREST_ENEMY: Cmd = by_name("TARGETNEARESTENEMY");
    pub(crate) const TARGET_PREVIOUS_ENEMY: Cmd = by_name("TARGETPREVIOUSENEMY");
    pub(crate) const NAMEPLATES: Cmd = by_name("NAMEPLATES");
    pub(crate) const FRIEND_NAMEPLATES: Cmd = by_name("FRIENDNAMEPLATES");
    pub(crate) const ALL_NAMEPLATES: Cmd = by_name("ALLNAMEPLATES");
    pub(crate) const ATTACK_TARGET: Cmd = by_name("ATTACKTARGET");
    pub(crate) const TOGGLE_UI: Cmd = by_name("TOGGLEUI");
    pub(crate) const CAMERA_ZOOM_IN: Cmd = by_name("CAMERAZOOMIN");
    pub(crate) const CAMERA_ZOOM_OUT: Cmd = by_name("CAMERAZOOMOUT");
}

/// The registry, in 1.12 `Bindings.xml` order. `TABLE` is the `const` view [`cmd::by_name`] scans,
/// since a `static` cannot be read during const evaluation.
pub(crate) static SPECS: &[Spec] = TABLE;

const TABLE: &[Spec] = &[
    // ── Movement (BINDING_HEADER_MOVEMENT) ──────────────────────────────────────────────
    spec!("MOVEANDSTEER", MOVEMENT, Kind::Held, Some("BUTTON3"), None),
    spec!("MOVEFORWARD", MOVEMENT, Kind::Held, Some("W"), Some("UP")),
    spec!(
        "MOVEBACKWARD",
        MOVEMENT,
        Kind::Held,
        Some("S"),
        Some("DOWN")
    ),
    spec!("TURNLEFT", MOVEMENT, Kind::Held, Some("A"), Some("LEFT")),
    spec!("TURNRIGHT", MOVEMENT, Kind::Held, Some("D"), Some("RIGHT")),
    spec!("STRAFELEFT", MOVEMENT, Kind::Held, Some("Q"), None),
    spec!("STRAFERIGHT", MOVEMENT, Kind::Held, Some("E"), None),
    spec!("JUMP", MOVEMENT, Kind::Host, Some("SPACE"), Some("NUMPAD0")),
    spec!("SITORSTAND", MOVEMENT, Kind::Host, Some("X"), None),
    spec!("TOGGLESHEATH", MOVEMENT, Kind::Host, Some("Z"), None),
    spec!(
        "TOGGLEAUTORUN",
        MOVEMENT,
        Kind::Host,
        Some("NUMLOCK"),
        Some("BUTTON4")
    ),
    spec!(
        "TOGGLERUN",
        MOVEMENT,
        Kind::Host,
        Some("NUMPADDIVIDE"),
        None
    ),
    spec!(
        "FOLLOWTARGET",
        MOVEMENT,
        Kind::Edge(r#"FollowUnit("target")"#),
        None,
        None
    ),
    // ── Chat (BINDING_HEADER_CHAT) ──────────────────────────────────────────────────────
    spec!(
        "OPENCHAT",
        CHAT,
        Kind::Edge("ChatFrame_OpenChat(\"\")"),
        Some("ENTER"),
        None
    ),
    spec!(
        "OPENCHATSLASH",
        CHAT,
        Kind::Edge("ChatFrame_OpenChat(\"/\")"),
        Some("/"),
        None
    ),
    spec!(
        "CHATPAGEUP",
        CHAT,
        Kind::Edge("ChatFrame_ChatPageUp()"),
        Some("PAGEUP"),
        None
    ),
    spec!(
        "CHATPAGEDOWN",
        CHAT,
        Kind::Edge("ChatFrame_ChatPageDown()"),
        Some("PAGEDOWN"),
        None
    ),
    spec!(
        "CHATBOTTOM",
        CHAT,
        Kind::Edge("ChatFrame_ScrollToBottom()"),
        Some("SHIFT-PAGEDOWN"),
        None
    ),
    spec!(
        "REPLY",
        CHAT,
        Kind::Edge("ChatFrame_ReplyTell()"),
        Some("R"),
        None
    ),
    // The last person you told, not the last who told you (`ChatFrame.lua:1650`).
    spec!(
        "REPLY2",
        CHAT,
        Kind::Edge("ChatFrame_ReplyTell2()"),
        Some("SHIFT-R"),
        None
    ),
    // The combat-log four (`Bindings.xml:108-119`): ChatFrame2's paging and `ToggleCombatLog`.
    spec!(
        "COMBATLOGPAGEUP",
        CHAT,
        Kind::Edge("ChatFrame2:PageUp()"),
        Some("CTRL-PAGEUP"),
        None
    ),
    spec!(
        "COMBATLOGPAGEDOWN",
        CHAT,
        Kind::Edge("ChatFrame2:PageDown()"),
        Some("CTRL-PAGEDOWN"),
        None
    ),
    spec!(
        "COMBATLOGBOTTOM",
        CHAT,
        Kind::Edge("ChatFrame2:ScrollToBottom()"),
        Some("CTRL-SHIFT-PAGEDOWN"),
        None
    ),
    spec!(
        "TOGGLECOMBATLOG",
        CHAT,
        Kind::Edge("ToggleCombatLog()"),
        Some("SHIFT-C"),
        None
    ),
    // ── Action bar (BINDING_HEADER_ACTIONBAR) ───────────────────────────────────────────
    // The `runOnUp` pair (`Bindings.xml:121`): down shows the pushed visual, up fires.
    spec!(
        "ACTIONBUTTON1",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(1)", "ActionButtonUp(1)"),
        Some("1"),
        None
    ),
    spec!(
        "ACTIONBUTTON2",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(2)", "ActionButtonUp(2)"),
        Some("2"),
        None
    ),
    spec!(
        "ACTIONBUTTON3",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(3)", "ActionButtonUp(3)"),
        Some("3"),
        None
    ),
    spec!(
        "ACTIONBUTTON4",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(4)", "ActionButtonUp(4)"),
        Some("4"),
        None
    ),
    spec!(
        "ACTIONBUTTON5",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(5)", "ActionButtonUp(5)"),
        Some("5"),
        None
    ),
    spec!(
        "ACTIONBUTTON6",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(6)", "ActionButtonUp(6)"),
        Some("6"),
        None
    ),
    spec!(
        "ACTIONBUTTON7",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(7)", "ActionButtonUp(7)"),
        Some("7"),
        None
    ),
    spec!(
        "ACTIONBUTTON8",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(8)", "ActionButtonUp(8)"),
        Some("8"),
        None
    ),
    spec!(
        "ACTIONBUTTON9",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(9)", "ActionButtonUp(9)"),
        Some("9"),
        None
    ),
    spec!(
        "ACTIONBUTTON10",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(10)", "ActionButtonUp(10)"),
        Some("0"),
        None
    ),
    spec!(
        "ACTIONBUTTON11",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(11)", "ActionButtonUp(11)"),
        Some("-"),
        None
    ),
    spec!(
        "ACTIONBUTTON12",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(12)", "ActionButtonUp(12)"),
        Some("="),
        None
    ),
    // ── Self-cast (`Bindings.xml:205-288`) ─────────────────────────────────────────────
    // ACTIONBUTTON's halves with `ActionButtonUp`'s `onSelf` argument set, which stock
    // `ActionButtonUp` forwards to `UseAction`.
    spec!(
        "SELFACTIONBUTTON1",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(1)", "ActionButtonUp(1, 1)"),
        Some("ALT-1"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON2",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(2)", "ActionButtonUp(2, 1)"),
        Some("ALT-2"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON3",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(3)", "ActionButtonUp(3, 1)"),
        Some("ALT-3"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON4",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(4)", "ActionButtonUp(4, 1)"),
        Some("ALT-4"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON5",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(5)", "ActionButtonUp(5, 1)"),
        Some("ALT-5"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON6",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(6)", "ActionButtonUp(6, 1)"),
        Some("ALT-6"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON7",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(7)", "ActionButtonUp(7, 1)"),
        Some("ALT-7"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON8",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(8)", "ActionButtonUp(8, 1)"),
        Some("ALT-8"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON9",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(9)", "ActionButtonUp(9, 1)"),
        Some("ALT-9"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON10",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(10)", "ActionButtonUp(10, 1)"),
        Some("ALT-0"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON11",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(11)", "ActionButtonUp(11, 1)"),
        Some("ALT--"),
        None
    ),
    spec!(
        "SELFACTIONBUTTON12",
        ACTIONBAR,
        Kind::EdgeUpDown("ActionButtonDown(12)", "ActionButtonUp(12, 1)"),
        Some("ALT-="),
        None
    ),
    // The reference body is `ShapeshiftBar_ChangeForm(n)`; this clicks the visible stock
    // button, whose OnClick calls it (`BonusActionBarFrame.xml:37`).
    spec!(
        "SHAPESHIFTBUTTON1",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton1"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F1"),
        None
    ),
    spec!(
        "SHAPESHIFTBUTTON2",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton2"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F2"),
        None
    ),
    spec!(
        "SHAPESHIFTBUTTON3",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton3"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F3"),
        None
    ),
    spec!(
        "SHAPESHIFTBUTTON4",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton4"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F4"),
        None
    ),
    spec!(
        "SHAPESHIFTBUTTON5",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton5"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F5"),
        None
    ),
    spec!(
        "SHAPESHIFTBUTTON6",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton6"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F6"),
        None
    ),
    spec!(
        "SHAPESHIFTBUTTON7",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton7"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F7"),
        None
    ),
    spec!(
        "SHAPESHIFTBUTTON8",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton8"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F8"),
        None
    ),
    spec!(
        "SHAPESHIFTBUTTON9",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton9"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F9"),
        None
    ),
    spec!(
        "SHAPESHIFTBUTTON10",
        ACTIONBAR,
        Kind::Edge(
            r#"local b = getglobal("ShapeshiftButton10"); if b and b:IsVisible() then b:Click() end"#
        ),
        Some("CTRL-F10"),
        None
    ),
    // The pet bar, which 1.12 names BONUSACTIONBUTTON under the ACTIONBAR header
    // (`Bindings.xml:321-390`); its `BonusActionButtonDown/Up` only call
    // `PetActionButtonDown/Up` (`BonusActionBarFrame.lua:106-112`).
    spec!(
        "BONUSACTIONBUTTON1",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(1)", "PetActionButtonUp(1)"),
        Some("CTRL-1"),
        None
    ),
    spec!(
        "BONUSACTIONBUTTON2",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(2)", "PetActionButtonUp(2)"),
        Some("CTRL-2"),
        None
    ),
    spec!(
        "BONUSACTIONBUTTON3",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(3)", "PetActionButtonUp(3)"),
        Some("CTRL-3"),
        None
    ),
    spec!(
        "BONUSACTIONBUTTON4",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(4)", "PetActionButtonUp(4)"),
        Some("CTRL-4"),
        None
    ),
    spec!(
        "BONUSACTIONBUTTON5",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(5)", "PetActionButtonUp(5)"),
        Some("CTRL-5"),
        None
    ),
    spec!(
        "BONUSACTIONBUTTON6",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(6)", "PetActionButtonUp(6)"),
        Some("CTRL-6"),
        None
    ),
    spec!(
        "BONUSACTIONBUTTON7",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(7)", "PetActionButtonUp(7)"),
        Some("CTRL-7"),
        None
    ),
    spec!(
        "BONUSACTIONBUTTON8",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(8)", "PetActionButtonUp(8)"),
        Some("CTRL-8"),
        None
    ),
    spec!(
        "BONUSACTIONBUTTON9",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(9)", "PetActionButtonUp(9)"),
        Some("CTRL-9"),
        None
    ),
    spec!(
        "BONUSACTIONBUTTON10",
        ACTIONBAR,
        Kind::EdgeUpDown("PetActionButtonDown(10)", "PetActionButtonUp(10)"),
        Some("CTRL-0"),
        None
    ),
    // ── Action-bar pages (`Bindings.xml:391-432`) ──────────────────────────────────────
    // Six pages of twelve (slots 1-72). `ChangeActionBarPage` is an engine verb that only fires
    // `ACTIONBAR_PAGE_CHANGED`; the `ActionBar_Page{Up,Down}` wrap is stock ActionButton.lua's.
    spec!(
        "ACTIONPAGE1",
        ACTIONBAR,
        Kind::Edge(
            "if ( CURRENT_ACTIONBAR_PAGE ~= 1 ) then CURRENT_ACTIONBAR_PAGE = 1; \
             ChangeActionBarPage(); end"
        ),
        Some("SHIFT-1"),
        None
    ),
    spec!(
        "ACTIONPAGE2",
        ACTIONBAR,
        Kind::Edge(
            "if ( CURRENT_ACTIONBAR_PAGE ~= 2 ) then CURRENT_ACTIONBAR_PAGE = 2; \
             ChangeActionBarPage(); end"
        ),
        Some("SHIFT-2"),
        None
    ),
    spec!(
        "ACTIONPAGE3",
        ACTIONBAR,
        Kind::Edge(
            "if ( CURRENT_ACTIONBAR_PAGE ~= 3 ) then CURRENT_ACTIONBAR_PAGE = 3; \
             ChangeActionBarPage(); end"
        ),
        Some("SHIFT-3"),
        None
    ),
    spec!(
        "ACTIONPAGE4",
        ACTIONBAR,
        Kind::Edge(
            "if ( CURRENT_ACTIONBAR_PAGE ~= 4 ) then CURRENT_ACTIONBAR_PAGE = 4; \
             ChangeActionBarPage(); end"
        ),
        Some("SHIFT-4"),
        None
    ),
    spec!(
        "ACTIONPAGE5",
        ACTIONBAR,
        Kind::Edge(
            "if ( CURRENT_ACTIONBAR_PAGE ~= 5 ) then CURRENT_ACTIONBAR_PAGE = 5; \
             ChangeActionBarPage(); end"
        ),
        Some("SHIFT-5"),
        None
    ),
    spec!(
        "ACTIONPAGE6",
        ACTIONBAR,
        Kind::Edge(
            "if ( CURRENT_ACTIONBAR_PAGE ~= 6 ) then CURRENT_ACTIONBAR_PAGE = 6; \
             ChangeActionBarPage(); end"
        ),
        Some("SHIFT-6"),
        None
    ),
    spec!(
        "PREVIOUSACTIONPAGE",
        ACTIONBAR,
        Kind::Edge("ActionBar_PageDown()"),
        Some("SHIFT-UP"),
        Some("SHIFT-MOUSEWHEELUP")
    ),
    spec!(
        "NEXTACTIONPAGE",
        ACTIONBAR,
        Kind::Edge("ActionBar_PageUp()"),
        Some("SHIFT-DOWN"),
        Some("SHIFT-MOUSEWHEELDOWN")
    ),
    // Flips the `LOCK_ACTIONBAR` uvar the Options window's Lock ActionBars row writes
    // (`Bindings.xml:433-439`).
    spec!(
        "TOGGLEACTIONBARLOCK",
        ACTIONBAR,
        Kind::Edge(
            r#"if LOCK_ACTIONBAR == "1" then LOCK_ACTIONBAR = "0" else LOCK_ACTIONBAR = "1" end"#
        ),
        None,
        None
    ),
    spec!(
        "TOGGLEAUTOSELFCAST",
        ACTIONBAR,
        Kind::Edge(
            "if ( GetCVar(\"autoSelfCast\") == \"1\" ) then SetCVar(\"autoSelfCast\", \"0\"); \
             else SetCVar(\"autoSelfCast\", \"1\"); end"
        ),
        None,
        None
    ),
    // ── Targeting (BINDING_HEADER_TARGETING) ────────────────────────────────────────────
    spec!(
        "TARGETNEARESTENEMY",
        TARGETING,
        Kind::Host,
        Some("TAB"),
        None
    ),
    spec!(
        "TARGETPREVIOUSENEMY",
        TARGETING,
        Kind::Host,
        Some("SHIFT-TAB"),
        None
    ),
    // The friendly scan; `1` is the reverse flag (`Bindings.xml:458`).
    spec!(
        "TARGETNEARESTFRIEND",
        TARGETING,
        Kind::Edge("TargetNearestFriend()"),
        Some("CTRL-TAB"),
        None
    ),
    spec!(
        "TARGETPREVIOUSFRIEND",
        TARGETING,
        Kind::Edge("TargetNearestFriend(1)"),
        Some("CTRL-SHIFT-TAB"),
        None
    ),
    // An already-targeted unit falls through to its pet (`Bindings.xml:460-494`).
    spec!(
        "TARGETSELF",
        TARGETING,
        Kind::Edge(
            r#"if UnitIsUnit("player", "target") then TargetUnit("pet") else TargetUnit("player") end"#
        ),
        Some("F1"),
        None
    ),
    spec!(
        "TARGETPARTYMEMBER1",
        TARGETING,
        Kind::Edge(
            r#"if UnitIsUnit("party1", "target") then TargetUnit("partypet1") else TargetUnit("party1") end"#
        ),
        Some("F2"),
        None
    ),
    spec!(
        "TARGETPARTYMEMBER2",
        TARGETING,
        Kind::Edge(
            r#"if UnitIsUnit("party2", "target") then TargetUnit("partypet2") else TargetUnit("party2") end"#
        ),
        Some("F3"),
        None
    ),
    spec!(
        "TARGETPARTYMEMBER3",
        TARGETING,
        Kind::Edge(
            r#"if UnitIsUnit("party3", "target") then TargetUnit("partypet3") else TargetUnit("party3") end"#
        ),
        Some("F4"),
        None
    ),
    spec!(
        "TARGETPARTYMEMBER4",
        TARGETING,
        Kind::Edge(
            r#"if UnitIsUnit("party4", "target") then TargetUnit("partypet4") else TargetUnit("party4") end"#
        ),
        Some("F5"),
        None
    ),
    spec!(
        "TARGETPET",
        TARGETING,
        Kind::Edge(r#"TargetUnit("pet")"#),
        Some("SHIFT-F1"),
        None
    ),
    spec!(
        "TARGETPARTYPET1",
        TARGETING,
        Kind::Edge(r#"TargetUnit("partypet1")"#),
        Some("SHIFT-F2"),
        None
    ),
    spec!(
        "TARGETPARTYPET2",
        TARGETING,
        Kind::Edge(r#"TargetUnit("partypet2")"#),
        Some("SHIFT-F3"),
        None
    ),
    spec!(
        "TARGETPARTYPET3",
        TARGETING,
        Kind::Edge(r#"TargetUnit("partypet3")"#),
        Some("SHIFT-F4"),
        None
    ),
    spec!(
        "TARGETPARTYPET4",
        TARGETING,
        Kind::Edge(r#"TargetUnit("partypet4")"#),
        Some("SHIFT-F5"),
        None
    ),
    spec!(
        "TARGETLASTHOSTILE",
        TARGETING,
        Kind::Edge("TargetLastEnemy()"),
        Some("G"),
        None
    ),
    spec!(
        "ASSISTTARGET",
        TARGETING,
        Kind::Edge(r#"AssistUnit("target")"#),
        Some("F"),
        None
    ),
    spec!("NAMEPLATES", TARGETING, Kind::Host, Some("V"), None),
    spec!(
        "FRIENDNAMEPLATES",
        TARGETING,
        Kind::Host,
        Some("SHIFT-V"),
        None
    ),
    spec!("ALLNAMEPLATES", TARGETING, Kind::Host, Some("CTRL-V"), None),
    spec!("ATTACKTARGET", TARGETING, Kind::Host, Some("T"), None),
    spec!(
        "PETATTACK",
        TARGETING,
        Kind::Edge("PetAttack()"),
        Some("SHIFT-T"),
        None
    ),
    // ── Interface panels (BINDING_HEADER_INTERFACE) ─────────────────────────────────────
    spec!(
        "TOGGLECHARACTER0",
        INTERFACE,
        Kind::Edge(r#"ToggleCharacter("PaperDollFrame")"#),
        Some("C"),
        None
    ),
    // The bag family: `B` opens the backpack alone, `SHIFT-B` every bag. Each body is the bare
    // global, as in `Bindings.xml`, never a button's OnClick, which would add its checked state.
    spec!(
        "TOGGLEBACKPACK",
        INTERFACE,
        Kind::Edge("ToggleBackpack()"),
        Some("B"),
        Some("F12")
    ),
    // TOGGLEBAGn is bag 5-n, so F8 opens the slot farthest from the backpack
    // (`Bindings.xml:564-575`).
    spec!(
        "TOGGLEBAG1",
        INTERFACE,
        Kind::Edge("ToggleBag(4)"),
        Some("F8"),
        None
    ),
    spec!(
        "TOGGLEBAG2",
        INTERFACE,
        Kind::Edge("ToggleBag(3)"),
        Some("F9"),
        None
    ),
    spec!(
        "TOGGLEBAG3",
        INTERFACE,
        Kind::Edge("ToggleBag(2)"),
        Some("F10"),
        None
    ),
    spec!(
        "TOGGLEBAG4",
        INTERFACE,
        Kind::Edge("ToggleBag(1)"),
        Some("F11"),
        None
    ),
    spec!(
        "OPENALLBAGS",
        INTERFACE,
        Kind::Edge("OpenAllBags()"),
        Some("SHIFT-B"),
        None
    ),
    spec!(
        "TOGGLEKEYRING",
        INTERFACE,
        Kind::Edge("ToggleKeyRing()"),
        None,
        None
    ),
    spec!(
        "TOGGLESPELLBOOK",
        INTERFACE,
        Kind::Edge("ToggleSpellBook(BOOKTYPE_SPELL)"),
        Some("P"),
        None
    ),
    // The pet book is the same window forked on `bookType`.
    spec!(
        "TOGGLEPETBOOK",
        INTERFACE,
        Kind::Edge("ToggleSpellBook(BOOKTYPE_PET)"),
        Some("SHIFT-I"),
        None
    ),
    spec!(
        "TOGGLETALENTS",
        INTERFACE,
        Kind::Edge("ToggleTalentFrame()"),
        Some("N"),
        None
    ),
    // TOGGLECHARACTERn is the reference's page number, not a tab index: 0 PaperDoll, 1 Skill,
    // 2 Reputation, 3 PetPaperDoll, 4 Honor; the file runs them 4, 3, 2, 1.
    spec!(
        "TOGGLECHARACTER4",
        INTERFACE,
        Kind::Edge(r#"ToggleCharacter("HonorFrame")"#),
        Some("H"),
        None
    ),
    spec!(
        "TOGGLECHARACTER3",
        INTERFACE,
        Kind::Edge(r#"ToggleCharacter("PetPaperDollFrame")"#),
        Some("SHIFT-P"),
        None
    ),
    spec!(
        "TOGGLECHARACTER2",
        INTERFACE,
        Kind::Edge(r#"ToggleCharacter("ReputationFrame")"#),
        Some("U"),
        None
    ),
    spec!(
        "TOGGLECHARACTER1",
        INTERFACE,
        Kind::Edge(r#"ToggleCharacter("SkillFrame")"#),
        Some("K"),
        None
    ),
    spec!(
        "TOGGLEQUESTLOG",
        INTERFACE,
        Kind::Edge("ToggleQuestLog()"),
        Some("L"),
        None
    ),
    spec!(
        "TOGGLEGAMEMENU",
        INTERFACE,
        Kind::Edge("ToggleGameMenu()"),
        Some("ESCAPE"),
        None
    ),
    spec!(
        "TOGGLEMINIMAP",
        INTERFACE,
        Kind::Edge(
            r#"if MinimapCluster:IsVisible() then MinimapCluster:Hide() else MinimapCluster:Show() end"#
        ),
        None,
        None
    ),
    spec!(
        "TOGGLEWORLDMAP",
        INTERFACE,
        Kind::Edge("ToggleWorldMap()"),
        Some("M"),
        None
    ),
    spec!(
        "TOGGLESOCIAL",
        INTERFACE,
        Kind::Edge("ToggleFriendsFrame()"),
        Some("O"),
        None
    ),
    spec!(
        "TOGGLEFRIENDSTAB",
        INTERFACE,
        Kind::Edge("ToggleFriendsFrame(1)"),
        None,
        None
    ),
    spec!(
        "TOGGLEWHOTAB",
        INTERFACE,
        Kind::Edge("ToggleFriendsFrame(2)"),
        None,
        None
    ),
    spec!(
        "TOGGLEGUILDTAB",
        INTERFACE,
        Kind::Edge("ToggleFriendsFrame(3)"),
        None,
        None
    ),
    spec!(
        "TOGGLERAIDTAB",
        INTERFACE,
        Kind::Edge("ToggleFriendsFrame(4)"),
        None,
        None
    ),
    // `ToggleWorldStateScoreFrame` is stock WorldStateFrame.lua's, `ToggleBattlefieldMinimap`
    // stock `UIParent.lua:216` (`Bindings.xml:630-635`).
    spec!(
        "TOGGLEWORLDSTATESCORES",
        INTERFACE,
        Kind::Edge("ToggleWorldStateScoreFrame()"),
        Some("SHIFT-SPACE"),
        None
    ),
    spec!(
        "TOGGLEBATTLEFIELDMINIMAP",
        INTERFACE,
        Kind::Edge("ToggleBattlefieldMinimap()"),
        Some("SHIFT-M"),
        None
    ),
    // ── Miscellaneous (BINDING_HEADER_MISC) ─────────────────────────────────────────────
    spec!(
        "MINIMAPZOOMIN",
        MISC,
        Kind::Edge("Minimap_ZoomInClick()"),
        Some("NUMPADPLUS"),
        None
    ),
    spec!(
        "MINIMAPZOOMOUT",
        MISC,
        Kind::Edge("Minimap_ZoomOutClick()"),
        Some("NUMPADMINUS"),
        None
    ),
    // The CVars stock SoundOptionsFrame.lua moves: `MasterSoundEffects` is the master enable,
    // `MasterVolume` steps by 0.1.
    spec!(
        "TOGGLEMUSIC",
        MISC,
        Kind::Edge(
            r#"if GetCVar("EnableMusic") == "1" then SetCVar("EnableMusic", "0") else SetCVar("EnableMusic", "1") end"#
        ),
        Some("CTRL-M"),
        None
    ),
    spec!(
        "TOGGLESOUND",
        MISC,
        Kind::Edge(
            r#"if GetCVar("MasterSoundEffects") == "1" then SetCVar("MasterSoundEffects", "0") else SetCVar("MasterSoundEffects", "1") end"#
        ),
        Some("CTRL-S"),
        None
    ),
    spec!(
        "MASTERVOLUMEUP",
        MISC,
        Kind::Edge(
            r#"local v = tonumber(GetCVar("MasterVolume")) + 0.1; if v > 1 then v = 1 end; SetCVar("MasterVolume", v)"#
        ),
        Some("CTRL-="),
        None
    ),
    spec!(
        "MASTERVOLUMEDOWN",
        MISC,
        Kind::Edge(
            r#"local v = tonumber(GetCVar("MasterVolume")) - 0.1; if v < 0 then v = 0 end; SetCVar("MasterVolume", v)"#
        ),
        Some("CTRL--"),
        None
    ),
    // ALT-Z is the shipped default; a `bindings-cache.wtf` holding CTRL-Z is a player rebind.
    spec!("TOGGLEUI", MISC, Kind::Host, Some("ALT-Z"), None),
    // Stock WorldFrame.lua's framerate readout.
    spec!(
        "TOGGLEFPS",
        MISC,
        Kind::Edge("ToggleFramerate();"),
        Some("CTRL-R"),
        None
    ),
    // `Edge`: the `<Binding>` has no `runOnUp`, and the dispatcher returns on key-up without it
    // (`0x4b7bea`).
    spec!(
        "SCREENSHOT",
        MISC,
        Kind::Edge("TakeScreenshot();"),
        Some("PRINTSCREEN"),
        None
    ),
    // ── Camera (BINDING_HEADER_CAMERA) ──────────────────────────────────────────────────
    // The five camera views (defaults: the table at `0x84f488`). NEXTVIEW/PREVVIEW stop at the
    // ends (`0x50faa0`/`0x50fac0`); 1.12 has no SAVEVIEW1/RESETVIEW1 row though the engine
    // accepts view 1.
    spec!(
        "NEXTVIEW",
        CAMERA,
        Kind::Edge("NextView()"),
        Some("END"),
        None
    ),
    spec!(
        "PREVVIEW",
        CAMERA,
        Kind::Edge("PrevView()"),
        Some("HOME"),
        None
    ),
    spec!(
        "CAMERAZOOMIN",
        CAMERA,
        Kind::Host,
        Some("MOUSEWHEELUP"),
        None
    ),
    spec!(
        "CAMERAZOOMOUT",
        CAMERA,
        Kind::Host,
        Some("MOUSEWHEELDOWN"),
        None
    ),
    spec!("SETVIEW1", CAMERA, Kind::Edge("SetView(1)"), None, None),
    spec!("SETVIEW2", CAMERA, Kind::Edge("SetView(2)"), None, None),
    spec!("SETVIEW3", CAMERA, Kind::Edge("SetView(3)"), None, None),
    spec!("SETVIEW4", CAMERA, Kind::Edge("SetView(4)"), None, None),
    spec!("SETVIEW5", CAMERA, Kind::Edge("SetView(5)"), None, None),
    spec!("SAVEVIEW2", CAMERA, Kind::Edge("SaveView(2)"), None, None),
    spec!("SAVEVIEW3", CAMERA, Kind::Edge("SaveView(3)"), None, None),
    spec!("SAVEVIEW4", CAMERA, Kind::Edge("SaveView(4)"), None, None),
    spec!("SAVEVIEW5", CAMERA, Kind::Edge("SaveView(5)"), None, None),
    spec!("RESETVIEW2", CAMERA, Kind::Edge("ResetView(2)"), None, None),
    spec!("RESETVIEW3", CAMERA, Kind::Edge("ResetView(3)"), None, None),
    spec!("RESETVIEW4", CAMERA, Kind::Edge("ResetView(4)"), None, None),
    spec!("RESETVIEW5", CAMERA, Kind::Edge("ResetView(5)"), None, None),
    spec!(
        "FLIPCAMERAYAW",
        CAMERA,
        Kind::Edge("FlipCameraYaw(180)"),
        None,
        None
    ),
    // ── MultiActionBar (BINDING_HEADER_MULTIACTIONBAR) ──────────────────────────────────
    // The four multibars' `runOnUp` pairs (`Bindings.xml:799-1134`), stock MultiActionBars.lua's.
    // Deviation: 1.12 files bars 2-4 under the spacer headers `BLANK`, `BLANK2` and `BLANK3`; they
    // sit under MULTIACTIONBAR here because this page's sections are named.
    spec!(
        "MULTIACTIONBAR1BUTTON1",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 1)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 1)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON2",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 2)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 2)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON3",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 3)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 3)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON4",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 4)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 4)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON5",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 5)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 5)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON6",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 6)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 6)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON7",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 7)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 7)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON8",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 8)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 8)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON9",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 9)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 9)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON10",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 10)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 10)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON11",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 11)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 11)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR1BUTTON12",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomLeft", 12)"#,
            r#"MultiActionButtonUp("MultiBarBottomLeft", 12)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON1",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 1)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 1)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON2",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 2)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 2)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON3",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 3)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 3)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON4",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 4)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 4)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON5",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 5)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 5)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON6",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 6)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 6)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON7",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 7)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 7)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON8",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 8)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 8)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON9",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 9)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 9)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON10",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 10)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 10)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON11",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 11)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 11)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR2BUTTON12",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarBottomRight", 12)"#,
            r#"MultiActionButtonUp("MultiBarBottomRight", 12)"#
        ),
        None,
        None
    ),
    // The vertical bars: `MultiBarRight` is bar 3, `MultiBarLeft` bar 4.
    spec!(
        "MULTIACTIONBAR3BUTTON1",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 1)"#,
            r#"MultiActionButtonUp("MultiBarRight", 1)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON2",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 2)"#,
            r#"MultiActionButtonUp("MultiBarRight", 2)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON3",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 3)"#,
            r#"MultiActionButtonUp("MultiBarRight", 3)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON4",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 4)"#,
            r#"MultiActionButtonUp("MultiBarRight", 4)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON5",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 5)"#,
            r#"MultiActionButtonUp("MultiBarRight", 5)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON6",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 6)"#,
            r#"MultiActionButtonUp("MultiBarRight", 6)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON7",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 7)"#,
            r#"MultiActionButtonUp("MultiBarRight", 7)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON8",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 8)"#,
            r#"MultiActionButtonUp("MultiBarRight", 8)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON9",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 9)"#,
            r#"MultiActionButtonUp("MultiBarRight", 9)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON10",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 10)"#,
            r#"MultiActionButtonUp("MultiBarRight", 10)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON11",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 11)"#,
            r#"MultiActionButtonUp("MultiBarRight", 11)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR3BUTTON12",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarRight", 12)"#,
            r#"MultiActionButtonUp("MultiBarRight", 12)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON1",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 1)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 1)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON2",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 2)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 2)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON3",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 3)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 3)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON4",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 4)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 4)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON5",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 5)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 5)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON6",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 6)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 6)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON7",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 7)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 7)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON8",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 8)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 8)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON9",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 9)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 9)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON10",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 10)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 10)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON11",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 11)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 11)"#
        ),
        None,
        None
    ),
    spec!(
        "MULTIACTIONBAR4BUTTON12",
        MULTIACTIONBAR,
        Kind::EdgeUpDown(
            r#"MultiActionButtonDown("MultiBarLeft", 12)"#,
            r#"MultiActionButtonUp("MultiBarLeft", 12)"#
        ),
        None,
        None
    ),
    // ── Raid targeting (BINDING_HEADER_RAID_TARGET) ─────────────────────────────────────
    // `SetRaidTargetIcon` toggles an icon the unit already wears off; 0 clears.
    spec!(
        "RAIDTARGET1",
        RAID_TARGET,
        Kind::Edge(r#"SetRaidTargetIcon("target", 1)"#),
        None,
        None
    ),
    spec!(
        "RAIDTARGET2",
        RAID_TARGET,
        Kind::Edge(r#"SetRaidTargetIcon("target", 2)"#),
        None,
        None
    ),
    spec!(
        "RAIDTARGET3",
        RAID_TARGET,
        Kind::Edge(r#"SetRaidTargetIcon("target", 3)"#),
        None,
        None
    ),
    spec!(
        "RAIDTARGET4",
        RAID_TARGET,
        Kind::Edge(r#"SetRaidTargetIcon("target", 4)"#),
        None,
        None
    ),
    spec!(
        "RAIDTARGET5",
        RAID_TARGET,
        Kind::Edge(r#"SetRaidTargetIcon("target", 5)"#),
        None,
        None
    ),
    spec!(
        "RAIDTARGET6",
        RAID_TARGET,
        Kind::Edge(r#"SetRaidTargetIcon("target", 6)"#),
        None,
        None
    ),
    spec!(
        "RAIDTARGET7",
        RAID_TARGET,
        Kind::Edge(r#"SetRaidTargetIcon("target", 7)"#),
        None,
        None
    ),
    spec!(
        "RAIDTARGET8",
        RAID_TARGET,
        Kind::Edge(r#"SetRaidTargetIcon("target", 8)"#),
        None,
        None
    ),
    spec!(
        "RAIDTARGETNONE",
        RAID_TARGET,
        Kind::Edge(r#"SetRaidTargetIcon("target", 0)"#),
        None,
        None
    ),
];

/// One 1.12 binding command this client does not register, and the mechanism it waits on; a
/// test fails once every global in [`needs`] is defined.
///
/// [`needs`]: Absent::needs
pub(crate) struct Absent {
    /// The 1.12 command name, exactly as `Bindings.xml` spells it.
    pub name: &'static str,
    /// Lua globals the reference's body calls that this client does not define; never empty, and
    /// once all are defined the row belongs in [`SPECS`].
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the falsifier is the gate test's to read, and the gate is the whole point \
                      of the field; an `expect` rather than an `allow` so the attribute goes by \
                      itself the day a runtime reader wants it"
        )
    )]
    pub needs: &'static [&'static str],
    /// What would have to exist, in one line.
    pub why: &'static str,
}

macro_rules! absent {
    ($name:literal, [$($needs:literal),* $(,)?], $why:literal) => {
        Absent {
            name: $name,
            needs: &[$($needs),*],
            why: $why,
        }
    };
}

/// The 1.12 commands benilla does not implement, in `Bindings.xml` order; with `SPECS`, exactly
/// the client's 228 live bindings.
pub(crate) static ABSENT: &[Absent] = &[
    // ── Movement ────────────────────────────────────────────────────────────────────────
    absent!(
        "PITCHUP",
        ["PitchUpStart", "PitchUpStop"],
        "no keyboard pitch: the mover has no pitch axis at all — swim and hover attitude ride the \
         move flags the server echoes, and nothing steers them"
    ),
    absent!(
        "PITCHDOWN",
        ["PitchDownStart", "PitchDownStop"],
        "no keyboard pitch — see PITCHUP"
    ),
    // ── Misc ────────────────────────────────────────────────────────────────────────────
    // The nine `hidden="true" debug="true"` rows: benilla's instruments for these answer to the
    // dev plane's chords, not to a binding.
    absent!(
        "TOGGLESTATS",
        ["ToggleStats"],
        "a dev-plane instrument on a dev chord"
    ),
    absent!(
        "TOGGLETRIS",
        ["ToggleTris"],
        "a dev-plane instrument on a dev chord"
    ),
    absent!(
        "TOGGLEPORTALS",
        ["TogglePortals"],
        "a dev-plane instrument on a dev chord"
    ),
    absent!(
        "TOGGLECOLLISION",
        ["ToggleCollision"],
        "a dev-plane instrument on a dev chord"
    ),
    absent!(
        "TOGGLECOLLISIONDISPLAY",
        ["ToggleCollisionDisplay"],
        "a dev-plane instrument on a dev chord"
    ),
    absent!(
        "TOGGLEPLAYERBOUNDS",
        ["TogglePlayerBounds"],
        "a dev-plane instrument on a dev chord"
    ),
    absent!(
        "TOGGLEPERFORMANCEDISPLAY",
        ["TogglePerformanceDisplay"],
        "a dev-plane instrument on a dev chord"
    ),
    absent!(
        "TOGGLEPERFORMANCEVALUES",
        ["TogglePerformanceValues"],
        "a dev-plane instrument on a dev chord"
    ),
    absent!(
        "RESETPERFORMANCEVALUES",
        ["ResetPerformanceValues"],
        "a dev-plane instrument on a dev chord"
    ),
    // ── The mouse's own three (hidden) ──────────────────────────────────────────────────
    // The reference's mouse-look bindings (BUTTON2, BUTTON1, CTRL-BUTTON1), hidden from the
    // window; benilla's mouse-look is hardwired in `player/camera.rs`.
    absent!(
        "TURNORACTION",
        ["TurnOrActionStart", "TurnOrActionStop"],
        "right-button mouse-look is hardwired, not routed through a binding"
    ),
    absent!(
        "CAMERAORSELECTORMOVE",
        ["CameraOrSelectOrMoveStart", "CameraOrSelectOrMoveStop"],
        "left-button select/move is hardwired, not routed through a binding"
    ),
    absent!(
        "CAMERAORSELECTORMOVESTICKY",
        ["CameraOrSelectOrMoveStart", "CameraOrSelectOrMoveStop"],
        "left-button select/move is hardwired, not routed through a binding"
    ),
    // ── iTunes remote (platform="mac") ──────────────────────────────────────────────────
    // 1.12's only `platform="mac"` rows; they remote the system music player, not the game's.
    absent!(
        "ITUNES_PLAYPAUSE",
        ["MusicPlayer_PlayPause"],
        "platform=\"mac\", and there is no system music player to remote"
    ),
    absent!(
        "ITUNES_NEXTTRACK",
        ["MusicPlayer_NextTrack"],
        "platform=\"mac\", and there is no system music player to remote"
    ),
    absent!(
        "ITUNES_BACKTRACK",
        ["MusicPlayer_BackTrack"],
        "platform=\"mac\", and there is no system music player to remote"
    ),
    absent!(
        "ITUNES_VOLUMEUP",
        ["MusicPlayer_VolumeUp"],
        "platform=\"mac\", and there is no system music player to remote"
    ),
    absent!(
        "ITUNES_VOLUMEDOWN",
        ["MusicPlayer_VolumeDown"],
        "platform=\"mac\", and there is no system music player to remote"
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_chord_parses() {
        for s in SPECS {
            for d in [s.d1, s.d2].into_iter().flatten() {
                assert!(
                    crate::bindings::chord::Chord::parse(d).is_some(),
                    "{}: default '{d}' does not parse",
                    s.name
                );
            }
        }
    }

    /// One key, one command: a duplicate default would leave the loser of the steal pass unbound.
    #[test]
    fn no_two_commands_ship_the_same_default_chord() {
        let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for s in SPECS {
            for d in [s.d1, s.d2].into_iter().flatten() {
                if let Some(prev) = seen.insert(d, s.name) {
                    panic!("default '{d}' is on both {prev} and {}", s.name);
                }
            }
        }
    }

    #[test]
    fn names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for s in SPECS {
            assert!(seen.insert(s.name), "duplicate command {}", s.name);
        }
    }

    /// Every row's `runOnUp` against the install's `Bindings.xml`, and its default chords against
    /// `WTF\DefaultBindings.wtf`.
    #[test]
    fn the_registry_matches_the_installs_own_bindings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open the 1.12 patch chain");

        let xml = String::from_utf8_lossy(
            &chain
                .read_file("Interface\\FrameXML\\Bindings.xml")
                .expect("the install carries Bindings.xml"),
        )
        .into_owned();
        let reference = benilla_ui::bindings_xml::parse(&xml).expect("Blizzard's own file parses");
        let run_on_up: std::collections::HashMap<&str, bool> = reference
            .iter()
            .map(|b| (b.name.as_str(), b.run_on_up))
            .collect();

        // `bind <CHORD> <COMMAND>`; file order is the Key 1, Key 2 order.
        let wtf = String::from_utf8_lossy(
            &chain
                .read_file("WTF\\DefaultBindings.wtf")
                .expect("the install carries DefaultBindings.wtf"),
        )
        .into_owned();
        let mut defaults: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for line in wtf.lines() {
            let mut it = line.split_whitespace();
            if let (Some("bind"), Some(chord), Some(command), None) =
                (it.next(), it.next(), it.next(), it.next())
            {
                defaults.entry(command).or_default().push(chord);
            }
        }

        for s in SPECS {
            let Some(&reference_run_on_up) = run_on_up.get(s.name) else {
                panic!(
                    "{} is not a 1.12 binding at all — the tree is meant to be honest",
                    s.name
                );
            };
            assert_eq!(
                s.run_on_up(),
                reference_run_on_up,
                "{}: runOnUp disagrees with the install's Bindings.xml — that flag decides \
                 whether a press of this command delivers a release half",
                s.name
            );
            let ours: Vec<&str> = [s.d1, s.d2].into_iter().flatten().collect();
            assert_eq!(
                ours,
                defaults.get(s.name).cloned().unwrap_or_default(),
                "{}: default chords disagree with the install's DefaultBindings.wtf",
                s.name
            );
        }
    }

    /// Every one of the client's 228 live bindings is in exactly one of [`SPECS`] and [`ABSENT`],
    /// none of the twelve `hidden="true"` ones is in `SPECS`, and `SPECS` keeps the file's order.
    #[test]
    fn every_1_12_command_is_registered_or_recorded_absent() {
        let Some(reference) = install_bindings() else {
            return;
        };

        let registered: std::collections::HashSet<&str> = SPECS.iter().map(|s| s.name).collect();
        let recorded: std::collections::HashSet<&str> = ABSENT.iter().map(|a| a.name).collect();

        let mut unwritten = Vec::new();
        for b in &reference {
            let name = b.name.as_str();
            match (registered.contains(name), recorded.contains(name)) {
                (true, true) => panic!("{name} is both registered and recorded absent"),
                (false, false) => unwritten.push(name),
                (true, false) => assert!(
                    !b.hidden,
                    "{name} is hidden=\"true\" in 1.12 — the Keybindings page would list a row \
                     the reference never shows"
                ),
                (false, true) => {}
            }
        }
        assert!(
            unwritten.is_empty(),
            "these 1.12 bindings are neither implemented nor recorded absent — add a `spec!` row \
             if the mechanism is here, an `absent!` row with its falsifier if it is not: {unwritten:?}"
        );

        let live: std::collections::HashSet<&str> =
            reference.iter().map(|b| b.name.as_str()).collect();
        for a in ABSENT {
            assert!(
                live.contains(a.name),
                "ABSENT names {}, which is not a live 1.12 binding (the six MOVEVIEW* rows are \
                 commented out in Blizzard's own file and are not commands)",
                a.name
            );
        }

        // The table's order is the Keybindings page's row order.
        let order: Vec<&str> = reference.iter().map(|b| b.name.as_str()).collect();
        let mut at = 0usize;
        for s in SPECS {
            let Some(found) = order[at..].iter().position(|n| *n == s.name) else {
                panic!(
                    "{} is out of 1.12 Bindings.xml order — the page renders SPECS order, so this \
                     is what the player sees",
                    s.name
                );
            };
            at += found + 1;
        }
    }

    /// An [`ABSENT`] row whose `needs` are all defined belongs in [`SPECS`]. "Defined" is read
    /// from source, not from a VM.
    #[test]
    fn the_absent_commands_are_still_absent() {
        let defined = lua_globals_defined();
        for a in ABSENT {
            if a.needs.is_empty() {
                continue;
            }
            let landed: Vec<&str> = a
                .needs
                .iter()
                .copied()
                .filter(|n| defined.contains(*n))
                .collect();
            assert_ne!(
                landed.len(),
                a.needs.len(),
                "{}'s mechanism has landed — {:?} are all defined now, so the row belongs in \
                 SPECS. (Recorded reason: {})",
                a.name,
                a.needs,
                a.why
            );
        }
    }

    /// Unique names, a reason on every row, and no row without `needs`, which could go stale
    /// unnoticed.
    #[test]
    fn the_absent_table_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for a in ABSENT {
            assert!(seen.insert(a.name), "duplicate absent command {}", a.name);
            assert!(!a.why.is_empty(), "{}: no reason recorded", a.name);
        }
        let unfalsifiable: Vec<&str> = ABSENT
            .iter()
            .filter(|a| a.needs.is_empty())
            .map(|a| a.name)
            .collect();
        assert!(
            unfalsifiable.is_empty(),
            "every absent row must carry a falsifier — a row with no `needs` can go stale \
             unnoticed, which is the whole failure this table exists to end: {unfalsifiable:?}"
        );
    }

    /// `lua_globals_defined` sees each store it reads and invents nothing; a blind scanner would
    /// pass every [`ABSENT`] row.
    #[test]
    fn the_global_scanner_sees_both_kinds_of_definition() {
        benilla_formats::wow_data_or_skip!();
        let defined = lua_globals_defined();
        for host in ["TargetUnit", "UseAction", "SetBinding"] {
            assert!(
                defined.contains(host),
                "the Rust scan missed the host registration `{host}`"
            );
        }
        // None of these is in `assets/ui`: `ChangeActionBarPage` is a host registration and the
        // other two come from stock CharacterFrame.lua and UIParent.lua off the chain, so this
        // loop does not isolate the `assets/ui` scan.
        for shipped in ["ToggleCharacter", "ChangeActionBarPage", "ShowUIPanel"] {
            assert!(
                defined.contains(shipped),
                "the scan missed `{shipped}`, a host registration or a stock chain global"
            );
        }
        // Stock ContainerFrame.lua's, sourced off the chain by `benilla.toc`.
        if benilla_formats::wow_data().is_some() {
            for sourced in ["ToggleKeyRing", "ToggleBag", "OpenAllBags"] {
                assert!(
                    defined.contains(sourced),
                    "the chain scan missed `{sourced}` — it is defined in the reference's own \
                     ContainerFrame.lua, which benilla.toc sources off the player's install"
                );
            }
        }
        assert!(
            !defined.contains("BenillaNoSuchGlobalExists"),
            "the scanner claims to define a name nobody wrote"
        );
    }

    /// The install's `Bindings.xml` has the shape `benilla_ui::bindings_xml` documents; the
    /// commented-out `MOVEVIEW*` family must not register.
    #[test]
    fn the_installs_bindings_xml_reads_as_the_parsers_header_says() {
        let Some(binds) = install_bindings() else {
            return;
        };
        assert_eq!(
            binds.len(),
            228,
            "1.12.1 ships 228 live bindings — the other six are inside a comment"
        );
        assert!(binds.iter().all(|b| !b.name.is_empty()));
        assert!(
            !binds.iter().any(|b| b.name.starts_with("MOVEVIEW")),
            "the commented-out family must not register"
        );
        assert_eq!(binds.iter().filter(|b| b.run_on_up).count(), 94);
        assert_eq!(binds.iter().filter(|b| b.header.is_some()).count(), 13);
        assert_eq!(binds.iter().filter(|b| b.hidden).count(), 12);
        assert_eq!(
            binds[0].header.as_deref(),
            Some("MOVEMENT"),
            "the file opens on the MOVEMENT section"
        );
    }

    /// The install's own `Bindings.xml`, parsed.
    fn install_bindings() -> Option<Vec<benilla_ui::bindings_xml::AddonBinding>> {
        let data = benilla_formats::wow_data_or_skip!(None);
        let mut chain = benilla_formats::open_chain(&data).expect("open the 1.12 patch chain");
        let xml = String::from_utf8_lossy(
            &chain
                .read_file("Interface\\FrameXML\\Bindings.xml")
                .expect("the install carries Bindings.xml"),
        )
        .into_owned();
        Some(benilla_ui::bindings_xml::parse(&xml).expect("Blizzard's own file parses"))
    }

    /// Every Lua global this client defines, read from source: host registrations
    /// (`g.set("Name", …)`), function definitions in `assets/ui`'s `<Script>` blocks, and the
    /// stock files `benilla.toc` sources off the chain (none without an install). Deliberately
    /// generous, `local function` included: a false "defined" fails loudly, a false "missing"
    /// would hide a landed mechanism.
    fn lua_globals_defined() -> std::collections::HashSet<String> {
        fn walk(dir: &std::path::Path, ext: &str, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    walk(&path, ext, out);
                } else if path.extension().is_some_and(|e| e == ext) {
                    out.push(path);
                }
            }
        }

        fn push(names: &mut std::collections::HashSet<String>, s: &str) {
            if s.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                names.insert(s.to_string());
            }
        }

        /// The `function Name(` and `Name = function` definitions in one Lua-carrying text.
        fn scan_lua(text: &str, names: &mut std::collections::HashSet<String>) {
            let mut rest = text;
            while let Some(i) = rest.find("function ") {
                rest = &rest[i + "function ".len()..];
                let end = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                if rest[end..].trim_start().starts_with('(') {
                    push(names, &rest[..end]);
                }
            }
            for line in text.lines() {
                let line = line.trim_start();
                if let Some((lhs, rhs)) = line.split_once('=') {
                    let lhs = lhs.trim();
                    if rhs.trim_start().starts_with("function")
                        && lhs.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        push(names, lhs);
                    }
                }
            }
        }

        /// The `.lua` files an XML document pulls in through `<Script file="…"/>`, relative to the
        /// document's directory, as the loader resolves them.
        fn script_files(text: &str) -> Vec<String> {
            let mut out = Vec::new();
            let mut rest = text;
            while let Some(i) = rest.find("file=\"") {
                rest = &rest[i + "file=\"".len()..];
                let Some(end) = rest.find('"') else { break };
                if rest[..end].to_ascii_lowercase().ends_with(".lua") {
                    out.push(rest[..end].to_string());
                }
                rest = &rest[end..];
            }
            out
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the workspace root is two above the crate")
            .to_path_buf();

        let mut names = std::collections::HashSet::new();

        let mut rust = Vec::new();
        walk(&root.join("crates"), "rs", &mut rust);
        for path in rust {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // `.set("Name"`, across any whitespace, as rustfmt breaks long ones onto a new line.
            let mut rest = text.as_str();
            while let Some(i) = rest.find(".set(") {
                rest = &rest[i + ".set(".len()..];
                let head = rest.trim_start();
                if let Some(body) = head.strip_prefix('"') {
                    if let Some(end) = body.find('"') {
                        push(&mut names, &body[..end]);
                    }
                }
            }
        }

        let ui = root.join("crates/benilla-app/assets/ui");
        let mut xml = Vec::new();
        walk(&ui, "xml", &mut xml);
        for path in xml {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            scan_lua(&text, &mut names);
        }

        let Some(data) = benilla_formats::wow_data() else {
            return names;
        };
        let Ok(mut chain) = benilla_formats::open_chain(&data) else {
            return names;
        };
        let manifest =
            std::fs::read_to_string(ui.join("benilla.toc")).expect("the manifest is committed");
        for entry in manifest
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            // An entry with a path separator is a stock file
            // (`ui_script::reference_ui::is_chain_entry`).
            .filter(|l| l.contains('\\') || l.contains('/'))
        {
            let Ok(bytes) = chain.read_file(entry) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            scan_lua(&text, &mut names);
            if !entry.to_ascii_lowercase().ends_with(".xml") {
                continue;
            }
            let dir = entry.rfind('\\').map_or("", |i| &entry[..=i]);
            for script in script_files(&text) {
                if let Ok(b) = chain.read_file(&format!("{dir}{script}")) {
                    scan_lua(&String::from_utf8_lossy(&b), &mut names);
                }
            }
        }
        names
    }
}
