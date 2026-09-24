//! The command registry (decision 0997) — every binding command benilla actually implements, in
//! 1.12 `Bindings.xml` order, with the 1.12 default chords (byte-real: the client's own
//! `bindings-cache.wtf`, account ONE) and each command's dispatch class.
//!
//! **Honest tree**: a command appears here only over a real engine action — the same law as the
//! options rows (0954). The 1.12 commands with no benilla mechanism yet are absent, not stubbed;
//! the page shows only what's here, and only non-empty categories (era law). Labels/headers are
//! the 1.12 GlobalStrings (`BINDING_NAME_*`/`BINDING_HEADER_*`), defined in the window's XML.
//!
//! **…and the absence is written down** ([`ABSENT`], decision 1745). The honest tree's one hole
//! was that nothing noticed when a mechanism ARRIVED: 0997 promised "each returns the day its
//! mechanism lands, one registry row", and then the keyring (0765), the pet book (1050), the
//! reputation and honor pages, the six action-bar pages and the two vertical multibars (1500)
//! all shipped their mechanism with no row — some for five hundred commits, none of them a
//! mistake anyone could see. `SPECS` ∪ `ABSENT` is now exactly the client's 228 live bindings,
//! and each absent row names the Lua globals whose arrival falsifies it.
//!
//! Three dispatch classes:
//! - [`Kind::Held`] — press latches, base-key release unlatches (the reference's `runOnUp`
//!   movement pairs); engine systems read the latch ([`super::BindingsState`]).
//! - [`Kind::Edge`] / [`Kind::EdgeUpDown`] — fires Lua in the VM (the reference's binding body,
//!   quoted 1:1 where our FrameXML port has the same functions).
//! - [`Kind::Host`] — fires into [`super::BindingsState::fired`]; an engine system consumes it
//!   (chat open, TAB targeting, nameplates, autorun, …).
//!
//! Recorded default divergences (0997): `TOGGLEUI` ships `ALT-Z` (0870: the cache's `CTRL-Z` is
//! a player rebind all three accounts inherited, not the shipped default — the one row that does
//! not trust the cache). That is now the ONLY one — the bag row's divergence (`OPENALLBAGS` wearing
//! both `B` and `SHIFT-B`, because benilla had a single all-bags knob) is gone as of 1494: the
//! reference's split ships whole, keys and bodies alike.

/// A command's index into [`SPECS`] — the engine-side handle (`cmd::JUMP`).
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
    /// The category header's global-string key (`BINDING_HEADER_MOVEMENT`) — the window's
    /// sidebar groups by it, era-style.
    pub category: &'static str,
    pub kind: Kind,
    /// The 1.12 default chords (canonical strings; `None` = shipped unbound).
    pub d1: Option<&'static str>,
    pub d2: Option<&'static str>,
}

impl Spec {
    /// Press+release semantics — the reference's `<Binding runOnUp=…>`, whose ONE reader is
    /// `RunCommand 0x4b7bf1`: it gates the release half and nothing else. (It used to gate a
    /// mousewheel refusal at `SetBinding` time here; that refusal is not in the client — 1295.)
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

/// Engine-side handles for the host-dispatched commands (indexes into [`SPECS`], asserted by
/// test). Lua-bodied commands need no handle — nothing in Rust names them.
pub(crate) mod cmd {
    use super::{Cmd, TABLE};

    /// Resolve a command NAME to its [`Cmd`] handle at compile time — a linear scan of [`TABLE`]
    /// in a `const fn`, so a name that is not in the registry is a BUILD error, not a runtime
    /// surprise.
    ///
    /// These handles used to be hand-written row numbers, and every row that landed ahead of one
    /// silently re-pointed it at its neighbour: 1057's `TOGGLECHARACTER3` moved three of them
    /// 86/87/88 → 87/88/89, 1136's `TOGGLEACTIONBARLOCK` moved the same three again, and 1494's
    /// bag family would have moved them a third time (`TOGGLE_UI` landing on `MINIMAPZOOMOUT`). A
    /// guard test named the drift but the numbers still had to be re-typed by hand each time; the
    /// name is the thing we actually mean, so the name is what the table is asked for now.
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

/// The registry, 1.12 `Bindings.xml` order. Sub-tables (action buttons, shapeshift, raid
/// targets) are written out because each row carries its own Lua body string.
///
/// `TABLE` is the `const` view [`cmd::by_name`] scans at compile time (a `static` cannot be read
/// during const evaluation); `SPECS` is the one everything else uses.
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
    // The other reply: the last person YOU told, not the last who told you
    // (`ChatEdit_GetLastToldTarget`, ChatFrame.lua l.1650). The memory was already being kept by
    // the send path and read by nothing — 1745.
    spec!(
        "REPLY2",
        CHAT,
        Kind::Edge("ChatFrame_ReplyTell2()"),
        Some("SHIFT-R"),
        None
    ),
    // The combat-log four (Bindings.xml l.108-119): ChatFrame2's own paging and the reference's
    // `ToggleCombatLog`, both FloatingChatFrame.lua's since the chat window became the
    // reference's (1948). Chords from `bindings-cache.wtf`, account ONE.
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
    // The ref's runOnUp pair (Bindings.xml:121: DOWN shows the pushed visual, UP fires) —
    // exactly what the old hardcoded number-row table sent.
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
    // The stance/shapeshift row (ref ShapeshiftBar_ChangeForm(n)) — ours clicks the bar's own
    // buttons, which carry the full form-switch law (stock BonusActionBarFrame.xml, 1938).
    // ── The self-cast dozen (1.12 `Bindings.xml`:257-293) ───────────────────────────────
    // The same two halves as ACTIONBUTTON, with `ActionButtonUp`'s second argument set: the
    // reference's own `onSelf`, which stock `ActionButtonUp` forwards to `UseAction`'s third
    // and the host used to drop (1745). `ALT-1`…`ALT-=` are byte-real from DefaultBindings.wtf,
    // and they sit one modifier off the plain bar exactly as the reference lays them out.
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
    // The PET bar's row — 1.12 calls it BONUSACTIONBUTTON and files it under this same ACTIONBAR
    // header (Bindings.xml:321-390 carries no `header` attribute, so it inherits l.121's), which
    // is why "Secondary Action Button 1-10" reads under Action Bar in the reference's own window.
    // The ref's body is BonusActionButtonDown/Up(id), and those two are one-liners onto
    // PetActionButtonDown/Up (BonusActionBarFrame.lua:106-112) — the PET bar's buttons, not the
    // bonus bar's. benilla has no bonus bar at all, so the pet bar is the whole lane, exactly as
    // the reference wires it. Defaults are byte-real and unanimous: CTRL-1..CTRL-0 in all three
    // of the install's `bindings-cache.wtf` files (unlike TOGGLEUI's rebind, 0870).
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
    // ── The action-bar PAGES (1.12 `Bindings.xml`:395-431) ──────────────────────────────
    // The bar is six pages of twelve (action slots 1..72) and it has been since 1500 shipped the
    // multibars; `ChangeActionBarPage` is the engine's (it fires ACTIONBAR_PAGE_CHANGED and nothing
    // else — 1938) and the `ActionBar_Page{Up,Down}` wrap is stock ActionButton.lua's. `SHIFT-1..6`
    // and the SHIFT-arrow / SHIFT-wheel steps are
    // byte-real from `WTF\\DefaultBindings.wtf`.
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
    // The action-bar lock (decision 1136), the ref's own binding body verbatim (Bindings.xml:433-
    // 439) — it flips the `LOCK_ACTIONBAR` uvar OptionsFrame.xml declares (1938), the same global the
    // Options window's Action Bars row writes. It sits here because the reference files it under
    // this header (l.433 carries no `header=`, so it inherits l.121's ACTIONBAR), and it ships
    // **unbound**: no `TOGGLEACTIONBARLOCK` line in any of the install's three
    // `bindings-cache.wtf` files, which is also what the option's own tooltip implies ("can be
    // bound to a function key in the keybindings interface").
    spec!(
        "TOGGLEACTIONBARLOCK",
        ACTIONBAR,
        Kind::Edge(
            r#"if LOCK_ACTIONBAR == "1" then LOCK_ACTIONBAR = "0" else LOCK_ACTIONBAR = "1" end"#
        ),
        None,
        None
    ),
    // The `autoSelfCast` CVar's own toggle, the reference's body verbatim — real since 1745
    // wired that CVar to the cast arm's `AutoSelfCast` knob, which had been welded to a Resource
    // default with nothing able to move it. Ships unbound, like the reference.
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
    // The friendly cone — the same scan with the reaction test flipped (1745). `CTRL-TAB` /
    // `CTRL-SHIFT-TAB`, byte-real; the reverse flag is the reference's own
    // `TargetNearestFriend(1)`, and its `Bindings.xml` comment says so out loud.
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
    // The self/party bodies are 1.12's own, 1:1 (Bindings.xml:460-509 — already-targeted
    // falls through to the pet).
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
    // The bag family, in the reference's own `Bindings.xml` order and with its own bodies and
    // defaults (1494). 0997 collapsed all of this onto ONE command — `OPENALLBAGS` wearing both
    // `B` and `SHIFT-B` over a `ToggleBackpack()` that opened every bag — because benilla had a
    // single all-bags knob to hang keys off. The reference has three knobs and the director
    // reported the difference: `B` opens the backpack ALONE, `SHIFT-B` opens the lot.
    //
    // Every default here is byte-real from the client's own `bindings-cache.wtf`, identical across
    // all three independent accounts (ONE, TWO, WINUSER) — which is what rules out a player rebind
    // (the `TOGGLEUI` trap, 0870).
    //
    // Each body is the bare GLOBAL, exactly the ref's own `Bindings.xml` — NOT a button's OnClick
    // handler: a handler carries the button's checked bookkeeping, and routing a key through it
    // drags that onto a path the reference keeps clean (with a bag addon holding `ToggleBackpack`,
    // the key press is the path that lights the backpack button on the real client — the addon's
    // OnShow write is the last word).
    spec!(
        "TOGGLEBACKPACK",
        INTERFACE,
        Kind::Edge("ToggleBackpack()"),
        Some("B"),
        Some("F12")
    ),
    // TOGGLEBAG**N** is bag **5-N**: the reference's numbering runs the bar right-to-left, so F8
    // opens the slot FARTHEST from the backpack. Quoted from ref Bindings.xml l.564-575.
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
    // The keyring's own toggle (1.12 `Bindings.xml`; ships UNBOUND, and the reference's page
    // shows it as such). 0765 landed the keyring plate and its `HasKey` gate; the row it was
    // owed has been missing since.
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
    // The PET book is the same window forked on `bookType` — 1050's law, and the reason this is
    // a second command rather than a second key on the first.
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
    // TOGGLECHARACTER**N** is the reference's PAGE number, not our tab index: 0 = PaperDoll,
    // 1 = Skill, 2 = Reputation, 3 = PetPaperDoll, 4 = Honor. The rows run 4, 3, 2, 1 in the
    // file, and the file's order is this table's order.
    spec!(
        "TOGGLECHARACTER4",
        INTERFACE,
        Kind::Edge(r#"ToggleCharacter("HonorFrame")"#),
        Some("H"),
        None
    ),
    // `SHIFT-P` is byte-real from the client's own `bindings-cache.wtf`, and identical in two
    // independent accounts (ONE and WINUSER) — which is what rules out a player rebind (the
    // `TOGGLEUI` trap, 0870). Decision 1057.
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
    // The battleground scoreboard and the battlefield minimap — the Interface block's last two
    // (Bindings.xml l.630-635), both Lua-bodied: `ToggleWorldStateScoreFrame` is the stock
    // WorldStateFrame.lua's since 1972 and `ToggleBattlefieldMinimap` UIParent.xml's (ref
    // UIParent.lua l.216-221). Defaults byte-real from `WTF\DefaultBindings.wtf`.
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
    // The sound toggles/steps flip the same CVars the ref's SoundOptionsFrame_* bodies do
    // (1.12 SoundOptionsFrame.lua: master enable = MasterSoundEffects, step 0.1).
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
    // ALT-Z, not the cache's CTRL-Z — settled by 0870: all three of the install's accounts
    // descend from one profile whose TOGGLEUI had been rebound; ALT-Z is the shipped default
    // (the one command whose default does NOT trust the cache file).
    spec!("TOGGLEUI", MISC, Kind::Host, Some("ALT-Z"), None),
    // Print screen (decision 1487). The body is the reference's own one-liner because our
    // `TakeScreenshot` has the same contract its does (ScreenshotStatus.xml) — hide the last
    // shot's confirmation, then ask the engine.
    //
    // `PRINTSCREEN` comes from the SHIPPED default, not a player's cache: `DefaultBindings.wtf`
    // lives inside `patch.MPQ` and its line 128 is `bind PRINTSCREEN SCREENSHOT` (the install's
    // account-ONE `bindings-cache.wtf` agrees). `Edge`, not `EdgeUpDown`, is also byte-real: the
    // `<Binding>` carries no `runOnUp`, and the reference's dispatcher returns on key-up unless
    // that flag is set (`0x4b7bea`).
    //
    // On a Mac keyboard the token arrives as F13, which is the reference's own Mac mapping rather
    // than an accommodation (`KEY_PRINTSCREEN_MAC = "F13"`); `super::chord` does the translation.
    // The stock WorldFrame.lua's framerate readout (decision 1983): `FramerateLabel`/`FramerateText`
    // toggled by `ToggleFramerate()`, refreshed off `GetFramerate()` every quarter second by
    // `WorldFrame_OnUpdate`. CTRL-R is the install's own DefaultBindings.wtf chord (1804).
    spec!(
        "TOGGLEFPS",
        MISC,
        Kind::Edge("ToggleFramerate();"),
        Some("CTRL-R"),
        None
    ),
    spec!(
        "SCREENSHOT",
        MISC,
        Kind::Edge("TakeScreenshot();"),
        Some("PRINTSCREEN"),
        None
    ),
    // ── Camera (BINDING_HEADER_CAMERA) ──────────────────────────────────────────────────
    // The five named camera views (decision 1745) — `player::camera_view`, whose defaults are the
    // reference's own `0x84f488` table. NEXTVIEW/PREVVIEW ship on END/HOME and **do not wrap**
    // (`0x50faa0`/`0x50fac0` are hard stops); Set/Save/Reset ship unbound, and 1.12 files no
    // SAVEVIEW1/RESETVIEW1 row even though its engine accepts view 1 — that is a `Bindings.xml`
    // decision, so the table follows the file.
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
    // The two bottom bars' buttons (stock MultiActionBars.xml renders exactly these; 1.12's right bars
    // and their MULTIACTIONBAR3/4 commands stay out — honest tree). Ref bodies are the
    // MultiActionButtonDown/Up runOnUp pair (Bindings.xml:799-966), defined in stock
    // MultiActionBars.lua; shipped UNBOUND like the ref (no MULTIACTIONBAR* line in any of the
    // install's bindings-cache.wtf files). 1.12 files bar 2 under a BLANK spacer-header;
    // both bars sit under the one MULTIACTIONBAR header here (1008, recorded).
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
    // ── The two VERTICAL bars (1.12 files them under its own `BLANK2`/`BLANK3` spacer
    // headers; they join bar 2 under MULTIACTIONBAR here for the same reason 1008 folded that
    // one — a spacer is a display device in the reference's flat list, and this page's sections
    // are named). Both bars are real since 1500; `MultiBarRight` is bar 3, `MultiBarLeft` bar 4,
    // and both ship UNBOUND exactly as the reference does.
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
    // 1.12 bodies 1:1 (SetRaidTargetIcon toggles when the unit already wears the icon; 0
    // clears — party.rs's registered semantics). All unbound by default, like the client.
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

/// One 1.12 binding command this client does **not** register — and the mechanism it waits on.
///
/// The honest tree (0997) says a command appears in [`SPECS`] only over a real engine action. That
/// rule is right and it stays; what it never had was a way to notice when the action *arrived*.
/// 0997's own residue promised "each returns the day its mechanism lands, one registry row" and
/// then five mechanisms landed without their rows: the keyring (0765), the pet book (1050), the
/// reputation and honor pages (1057-era), the six action-bar pages and the two vertical multibars
/// (1500). Nothing was wrong with any of those commits — nothing was *watching*.
///
/// So the absence is written down rather than merely true, and it is written down in a form that
/// goes stale loudly: [`needs`] names the Lua globals the reference's own binding body calls that
/// this client does not define, and
/// `the_absent_commands_are_still_absent` fails the day the last one lands.
///
/// [`needs`]: Absent::needs
pub(crate) struct Absent {
    /// The 1.12 command name, exactly as `Bindings.xml` spells it.
    pub name: &'static str,
    /// Lua globals the reference's body calls that this client does not define. **This is the
    /// falsifier**: while at least one is missing the absence is real; when they are all defined
    /// the mechanism has landed and the row belongs in [`SPECS`].
    ///
    /// Empty is allowed and means *there is no Lua-global signal* — the missing mechanism is
    /// host-side behind a global that already exists (self-cast lives behind `UseAction`'s third
    /// argument, which the Lua passes and the host drops). Those rows carry the whole weight in
    /// [`why`], and they are the ones to be suspicious of.
    ///
    /// [`why`]: Absent::why
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
    /// What would have to exist here — one line, mechanism-first.
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

/// The 1.12 commands benilla does not implement, in `Bindings.xml` order — the other half of the
/// registry, and the half that used to be invisible. `SPECS` ∪ `ABSENT` is exactly the client's
/// 228 live bindings, asserted by [`tests::every_1_12_command_is_registered_or_recorded_absent`].
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
    // ── Action bar ──────────────────────────────────────────────────────────────────────
    // ── Interface ───────────────────────────────────────────────────────────────────────
    // ── Misc ────────────────────────────────────────────────────────────────────────────
    // The nine `hidden="true" debug="true"` rows. Every one of them names an instrument benilla
    // really has (the tri counter, the collision display, the portal draw, the perf pill) — they
    // are absent because those instruments answer to the dev plane's chords (0702/1043), not to a
    // binding. Whether that is right is a real question and 1745 takes it; until then the rows
    // stay recorded rather than quietly missing.
    absent!(
        "TOGGLESTATS",
        ["ToggleStats"],
        "a dev-plane instrument (1043)"
    ),
    absent!(
        "TOGGLETRIS",
        ["ToggleTris"],
        "a dev-plane instrument (1043)"
    ),
    absent!(
        "TOGGLEPORTALS",
        ["TogglePortals"],
        "a dev-plane instrument (1043)"
    ),
    absent!(
        "TOGGLECOLLISION",
        ["ToggleCollision"],
        "a dev-plane instrument (1043)"
    ),
    absent!(
        "TOGGLECOLLISIONDISPLAY",
        ["ToggleCollisionDisplay"],
        "a dev-plane instrument (1043)"
    ),
    absent!(
        "TOGGLEPLAYERBOUNDS",
        ["TogglePlayerBounds"],
        "a dev-plane instrument (1043)"
    ),
    absent!(
        "TOGGLEPERFORMANCEDISPLAY",
        ["TogglePerformanceDisplay"],
        "a dev-plane instrument (1043)"
    ),
    absent!(
        "TOGGLEPERFORMANCEVALUES",
        ["TogglePerformanceValues"],
        "a dev-plane instrument (1043)"
    ),
    absent!(
        "RESETPERFORMANCEVALUES",
        ["ResetPerformanceValues"],
        "a dev-plane instrument (1043)"
    ),
    // ── The mouse's own three (hidden) ──────────────────────────────────────────────────
    // These are the reference's mouse-look bindings — BUTTON2 turn-or-action, BUTTON1
    // select-or-move, CTRL-BUTTON1 the sticky variant — and they are `hidden` because the window
    // never lists them, not because they are debug. benilla's mouse-look is hardwired in
    // `player/camera.rs` instead, which is exactly the shape 1043 spent a decision undoing for
    // the keyboard.
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
    // The one place 1.12's own binding file is OS-specific: five rows carrying `platform="mac"`,
    // the whole use of that attribute in the client. They remote the *system* music player, not
    // the game's, and benilla has nothing to remote.
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

    /// Every default chord in the table parses — a typo'd token would otherwise silently ship
    /// an unpressable default.
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

    /// No two commands ship the SAME default chord. One key, one command is the binding table's
    /// own law (the steal pass in [`super::store::resolve`] enforces it at load), so a duplicate
    /// here would ship a table that eats itself: registration order would decide the winner and
    /// the loser would come up silently unbound. Cheap tripwire for the one mistake a new block
    /// of rows can make — 1052's CTRL-1..CTRL-0 went in with nothing to catch a collision.
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

    /// Names are unique (the table is keyed by them everywhere: files, Lua, the window).
    #[test]
    fn names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for s in SPECS {
            assert!(seen.insert(s.name), "duplicate command {}", s.name);
        }
    }

    /// **The registry against the real client's own two files.** Two columns of this table are
    /// pure transcription from the 1.12 install, and both are load-bearing in a way nothing else
    /// here can see: `runOnUp` — which decides whether a wheel notch delivers a release half at
    /// all (1295), and which B265 turned entirely on — and every command's default chords. A
    /// hand-typed table drifts; this reads the source of truth instead of trusting the typing.
    ///
    /// `Interface\FrameXML\Bindings.xml` carries the flag, `WTF\DefaultBindings.wtf` the chords.
    /// Both are install content, read at runtime, never committed; the test skips (passes)
    /// without a client, the house pattern for a real-data test. Running Blizzard's own 234-row
    /// file through our [`benilla_ui::bindings_xml`] is a second gate riding along — that is the
    /// parser every addon's `Bindings.xml` lands in.
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

        // `bind <CHORD> <COMMAND>`, in the file's own order — which is the order the command's
        // Key 1 and Key 2 slots take.
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
                 whether a press of this command delivers a release half (1295)",
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

    /// **The coverage gate** (decision 1745): every one of the client's 228 live bindings is
    /// either in [`SPECS`] or in [`ABSENT`], and never both.
    ///
    /// This is the half [`the_registry_matches_the_installs_own_bindings`] could not see. That
    /// test walks OUR rows and checks them against the install, so a command we never wrote down
    /// is invisible to it — which is how the keyring, the pet book, the reputation and honor
    /// pages, the six action-bar pages and the two vertical multibars all shipped their mechanism
    /// and kept their key unbound, some of them for five hundred commits. Walking the INSTALL's
    /// rows instead makes an unwritten command a build failure the day it becomes reachable.
    ///
    /// `hidden` rides along: 1.12 marks twelve bindings `hidden="true"` (the nine debug toggles
    /// and the three mouse-look ones) and the window never lists them. This client registers none
    /// of them, and this asserts that stays true — a hidden row in `SPECS` would be a row the
    /// Keybindings page shows and the reference does not.
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

        // The table's order IS the Keybindings page's row order, and the module claims it is the
        // file's. It was not: TOGGLECHARACTER1/3 sat ahead of TOGGLESPELLBOOK until 1745.
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

    /// **The staleness half.** Each [`ABSENT`] row names the Lua globals the reference's own body
    /// calls that this client does not define; while at least one is still missing the absence is
    /// real. When they are all defined the mechanism has landed and the row is owed to [`SPECS`] —
    /// which is precisely the event nothing was watching for.
    ///
    /// "Defined" is read from source, not from a VM: a harness would have to load the whole of
    /// FrameXML to answer, and the question here is "has anyone written it yet", which the sources
    /// say directly. Same posture as `tests/world_api_wall.rs`.
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

    /// The absent table is well formed: unique names, and every row either carries a falsifier or
    /// is one of the host-side ones the header calls out. The second half is not a formality — a
    /// row with no `needs` can rot silently, so the count of them is pinned and moving it is a
    /// deliberate act.
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

    /// **The scanner's own falsifier.** [`the_absent_commands_are_still_absent`] is only a gate
    /// while `lua_globals_defined` really sees every kind of definition; a scanner that quietly
    /// returned nothing would pass every row forever, which is the exact failure it exists to
    /// end. So: one global of each kind that is certainly defined, and one that certainly is not.
    ///
    /// **There are THREE kinds since decision 1751, and this test found the third the hard way.**
    /// `ToggleKeyRing` sat in the FrameXML row below and started failing the day the bag windows
    /// migrated: it is defined in `ContainerFrame.lua`, which this client no longer ships — it
    /// EXECUTES it off the player's own patch chain. The honest repair was not to swap the name
    /// for one still in `assets/ui`; it was that the scan had gone blind to a whole store, and
    /// blind to it in exactly the direction that matters (1751 §4 makes "ship the stock files off
    /// the chain" the default for a NEW window, so the global that retires an ABSENT row —
    /// `ToggleBattlefieldMinimap`, say — will appear in neither Rust nor `assets/ui`). So
    /// `lua_globals_defined` reads the chain half of the manifest too, and `ToggleKeyRing` stays
    /// here as the witness for it.
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
        // Each is defined ONLY in `assets/ui` — the reference's own homes for them
        // (CharacterFrame.lua, ActionBarFrame.lua, UIParent.lua) are not among the files
        // `benilla.toc` sources, so the chain scan below cannot cover for a broken one.
        for shipped in ["ToggleCharacter", "ChangeActionBarPage", "ShowUIPanel"] {
            assert!(
                defined.contains(shipped),
                "the assets/ui scan missed `{shipped}`"
            );
        }
        // The chain half needs a client, like every other reader of install content — and the
        // two rows above still run without one, so the test keeps its teeth either way.
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

    /// The parser's own header, read against the file it describes. `benilla_ui::bindings_xml`
    /// quotes the shape of `Interface\FrameXML\Bindings.xml` — 228 live bindings, 94 `runOnUp`,
    /// 13 `header`, 12 `hidden`, the commented-out `MOVEVIEW*` family that a text search counts and
    /// a parser must not — and until 2331 the test that pinned those numbers lived beside the
    /// parser, keyed on a `BENILLA_BINDINGS_XML` path nothing ever set, so it had never run. It
    /// lives here because here is where the install is (`install_bindings`, off the player's own
    /// chain); a wrong count is either a wrong file or a header that drifted from the client.
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

    /// The install's own `Bindings.xml`, parsed — `None` (and a skipped test) without a client.
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

    /// Every Lua global this client defines, read out of its own sources — **all three stores**:
    /// the host registrations (`g.set("Name", …)` anywhere in the workspace), our own FrameXML
    /// port's function definitions (`function Name(` / `Name = function` inside the `<Script>`
    /// blocks of `assets/ui`), and the reference's own FrameXML that decision 1751 executes off
    /// the player's installed patch chain.
    ///
    /// The third store is not decoration. Until the bag windows migrated, "the FrameXML this
    /// client runs" and "the files in `assets/ui`" were the same set; they are not any more, and
    /// 1751 §4 makes the divergence grow — a NEW window ships as the stock files off the chain
    /// plus the engine verbs they call, so the global that would retire an [`ABSENT`] row can land
    /// without a line changing in either half this scan used to read. `benilla.toc` is the one
    /// ordered list of both stores (a manifest entry carrying a path is the reference's file), so
    /// it is what the chain half walks. No install ⇒ no chain half, the standard degradation.
    ///
    /// Deliberately syntactic and deliberately generous — a false "defined" fails the gate loudly
    /// (it asks for a row that turns out not to work), while a false "missing" would let a landed
    /// mechanism stay silent, which is the failure this whole gate exists to end. A reference
    /// file's `local function name()` is therefore counted as well; that is the generous direction
    /// on purpose.
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

        /// One Lua-carrying text's own definitions — a `<Script>` block of ours or a reference
        /// `.lua` off the chain, the same two shapes either way.
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

        /// The `.lua` files an XML document pulls in through `<Script file="…"/>`. Only a script
        /// reference ends in `.lua` — a `<Texture file=>` names art — so the extension is the
        /// whole test, and the paths come back relative to the document's own directory, which is
        /// how the loader resolves them (1186).
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

        // `crates/benilla-app/` → the workspace root.
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
            // `.set("Name"` — the host-registration idiom, over any amount of intervening
            // whitespace (rustfmt breaks the long ones onto their own line).
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

        // The chain half of the manifest — the reference's own files, read the way the client
        // reads them.
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
            // The manifest's own rule for which store an entry names
            // (`ui_script::reference_ui::is_chain_entry`): our tree is flat, so a separator is a
            // path and a path is the reference's. The bare names were walked above.
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
