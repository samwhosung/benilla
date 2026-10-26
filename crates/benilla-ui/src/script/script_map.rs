//! The handler names a frame type accepts. `SetScript`, `GetScript` and `HasScript` ask the
//! frame's type through its vtable slot 0xc (`0x774754`, `0x774854`, `0x77499f`), which compares
//! the name ignoring case (`SStrCmpI`, `0x64a4c0`) against the base map, `OnEvent` (`0x702590`)
//! plus the sixteen of `0x76a0d0`, and then the names the type adds. A name the type lacks raises
//! `%s doesn't have a "%s" script` from `SetScript` and `GetScript` (`0x879678` at `0x7749d1`,
//! `0x774884`), and `HasScript` answers nil (`0x774772`).

use crate::widget::FrameKind;

/// `OnEvent` (`0x702590`), then `0x76a0d0`'s sixteen: a plain Frame's map, which `Frame`,
/// `MessageFrame` and the world frame and minimap use as they are.
const FRAME: [&str; 17] = [
    "OnEvent",
    "OnLoad",
    "OnSizeChanged",
    "OnUpdate",
    "OnShow",
    "OnHide",
    "OnEnter",
    "OnLeave",
    "OnMouseDown",
    "OnMouseUp",
    "OnMouseWheel",
    "OnDragStart",
    "OnDragStop",
    "OnReceiveDrag",
    "OnChar",
    "OnKeyDown",
    "OnKeyUp",
];

/// Button and CheckButton (`0x778c50`), and the loot button, which is one.
const BUTTON: [&str; 2] = ["OnClick", "OnDoubleClick"];

/// The EditBox's (`0x77a310`). It never fires `OnKeyDown`, and fires `OnChar` only from `Insert`
/// (`0x77c13c`), but its map holds both through the frame's.
const EDIT_BOX: [&str; 10] = [
    "OnEnterPressed",
    "OnEscapePressed",
    "OnSpacePressed",
    "OnTabPressed",
    "OnTextChanged",
    "OnTextSet",
    "OnCursorChanged",
    "OnInputLanguageChanged",
    "OnEditFocusGained",
    "OnEditFocusLost",
];

/// Every model kind's (`0x76cc80`): Model (a cooldown is one), PlayerModel, DressUpModel and
/// TabardModel.
const MODEL: [&str; 2] = ["OnUpdateModel", "OnAnimFinished"];

/// The ScrollFrame's (`0x786c40`).
const SCROLL_FRAME: [&str; 3] = [
    "OnHorizontalScroll",
    "OnVerticalScroll",
    "OnScrollRangeChanged",
];

/// SimpleHTML's (`0x7a3700`).
const HYPERLINKS: [&str; 3] = ["OnHyperlinkEnter", "OnHyperlinkLeave", "OnHyperlinkClick"];

/// The ScrollingMessageFrame's (`0x787d90`): `OnMessageScrollChanged` after SimpleHTML's three,
/// which it asks first (`0x787d9b`).
const SCROLLING_MESSAGE: [&str; 4] = [
    "OnHyperlinkEnter",
    "OnHyperlinkLeave",
    "OnHyperlinkClick",
    "OnMessageScrollChanged",
];

/// The Slider's (`0x789750`) and the StatusBar's (`0x7830d0`).
const VALUE: [&str; 1] = ["OnValueChanged"];

/// The ColorSelect's (`0x78b4f0`).
const COLOR_SELECT: [&str; 1] = ["OnColorSelect"];

/// The movie frame's (`0x78c2e0`).
const MOVIE: [&str; 3] = [
    "OnMovieFinished",
    "OnMovieShowSubtitle",
    "OnMovieHideSubtitle",
];

/// The GameTooltip's (`0x5295d0`).
const TOOLTIP: [&str; 3] = [
    "OnTooltipSetDefaultAnchor",
    "OnTooltipCleared",
    "OnTooltipAddMoney",
];

/// The names `kind` adds to the frame's.
fn added(kind: FrameKind) -> &'static [&'static str] {
    match kind {
        FrameKind::Button | FrameKind::LootButton | FrameKind::CheckButton => &BUTTON,
        FrameKind::EditBox => &EDIT_BOX,
        FrameKind::Model
        | FrameKind::PlayerModel
        | FrameKind::DressUpModel
        | FrameKind::TabardModel => &MODEL,
        FrameKind::ScrollFrame => &SCROLL_FRAME,
        FrameKind::SimpleHtml => &HYPERLINKS,
        FrameKind::ScrollingMessageFrame => &SCROLLING_MESSAGE,
        FrameKind::Slider | FrameKind::StatusBar => &VALUE,
        FrameKind::ColorSelect => &COLOR_SELECT,
        FrameKind::MovieFrame => &MOVIE,
        FrameKind::GameTooltip => &TOOLTIP,
        FrameKind::Frame | FrameKind::WorldFrame | FrameKind::MessageFrame | FrameKind::Minimap => {
            &[]
        }
    }
}

/// `name` as the type spells it, if the map of a `kind` frame has it, ignoring case.
pub(crate) fn script_name(kind: FrameKind, name: &str) -> Option<&'static str> {
    FRAME
        .iter()
        .chain(added(kind))
        .copied()
        .find(|k| k.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_type_has_the_resolver_the_reference_gives_it() {
        let has = |kind, name| script_name(kind, name).is_some();
        assert!(has(FrameKind::Frame, "onevent") && has(FrameKind::Frame, "OnKeyUp"));
        assert!(!has(FrameKind::Frame, "OnClick"));
        assert!(has(FrameKind::CheckButton, "ONDOUBLECLICK"));
        assert!(!has(FrameKind::StatusBar, "OnTextChanged"));
        assert!(has(FrameKind::DressUpModel, "OnAnimFinished"));
        assert!(has(FrameKind::ScrollingMessageFrame, "OnHyperlinkClick"));
        assert!(!has(FrameKind::MessageFrame, "OnHyperlinkClick"));
        assert!(!has(FrameKind::SimpleHtml, "OnMessageScrollChanged"));
        assert!(has(FrameKind::GameTooltip, "OnTooltipAddMoney"));
        assert_eq!(
            script_name(FrameKind::Slider, "onvaluechanged"),
            Some("OnValueChanged")
        );
    }
}
