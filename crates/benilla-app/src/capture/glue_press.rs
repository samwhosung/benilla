//! The stock character screens as an instrument presses them: the create screen's race, sex,
//! class and five customizations set through its own verbs, a name typed and Okay pressed, and a
//! roster row deleted through the delete dialog's verb. Each is what a player's clicks call, so the
//! glue manager's state, dialogs and roster asks run as they do for a player.

use benilla_protocol::{CharCreateReq, Character};

use crate::ui_script::glue::StockVm;

/// `ChrRaces.dbc`'s `ClientFileString` by id, the second value `GetAvailableRaces` gives per race.
const RACE_FILES: [(u8, &str); 8] = [
    (1, "HUMAN"),
    (2, "ORC"),
    (3, "DWARF"),
    (4, "NIGHTELF"),
    (5, "SCOURGE"),
    (6, "TAUREN"),
    (7, "GNOME"),
    (8, "TROLL"),
];

/// `ChrClasses.dbc`'s `filename` by id, the second value `GetClassesForRace` gives per class.
const CLASS_FILES: [(u8, &str); 9] = [
    (1, "WARRIOR"),
    (2, "PALADIN"),
    (3, "HUNTER"),
    (4, "ROGUE"),
    (5, "PRIEST"),
    (7, "SHAMAN"),
    (8, "MAGE"),
    (9, "WARLOCK"),
    (11, "DRUID"),
];

fn file_of(table: &[(u8, &'static str)], id: u8) -> Option<&'static str> {
    table.iter().find(|(i, _)| *i == id).map(|(_, f)| *f)
}

/// A Lua expression for the 1-based position of `file` among the file strings a list verb
/// returns in pairs (`GetAvailableRaces`, `GetClassesForRace`), nil when it lists none.
fn list_index(verb: &str, file: &str) -> String {
    format!(
        "(function() local t = {{{verb}()}} local i = 2 while t[i] do \
         if strupper(t[i]) == {file:?} then return i / 2 end i = i + 2 end end)()"
    )
}

/// The login screen's account form stands, every agreement accepted.
pub(super) fn login_form_up(vm: StockVm) -> bool {
    vm.frame_visible("AccountLoginUI")
}

/// Accept every user agreement the login screen holds up, as its Accept button does for each in
/// turn (`TOSAccept`'s `OnClick`, then `AccountLogin_ShowUserAgreements`).
pub(super) fn press_accept_agreements(vm: StockVm) {
    vm.press(
        "AcceptEULA() AcceptTOS() AcceptScanning() AcceptContest() \
         AccountLogin_ShowUserAgreements()",
    );
}

/// A dialog stands over the screens: the glue manager's status dialog, or a failure's `OKAY`.
pub(super) fn dialog_up(vm: StockVm) -> bool {
    vm.frame_visible("GlueDialog")
}

/// The select screen stands with nothing over it and `chars` loaded into it, in order: the
/// roster load (`0x4724d0`) has run on the roster the instrument holds.
pub(super) fn select_holds(vm: StockVm, chars: &[Character]) -> bool {
    if !vm.frame_visible("CharacterSelect") || dialog_up(vm) {
        return false;
    }
    let shown = vm.number("return GetNumCharacters()").map(|n| n as usize);
    shown == Some(chars.len())
        && chars.iter().enumerate().all(|(row, c)| {
            vm.text(&format!("return (GetCharacterInfo({}))", row + 1))
                .as_deref()
                == Some(c.name.as_str())
        })
}

/// The create screen stands with nothing over it.
pub(super) fn create_up(vm: StockVm) -> bool {
    vm.frame_visible("CharacterCreate") && !dialog_up(vm)
}

/// Character select's Create New Character (`CharacterSelect.lua:336`).
pub(super) fn press_create_screen(vm: StockVm) {
    vm.press("SetGlueScreen(\"charcreate\")");
}

/// Put `req`'s body up on the create screen: its race button's click (`CharacterRace_OnClick`,
/// whose checked test only guards a click on the race already up), its sex button's
/// (`SetCharacterGender`), its class button's (`CharacterClass_OnClick`), then each customization's
/// arrow (`CycleCharCustomization`) pressed as many times as reaches `req`'s value, hair style
/// before the hair colour it counts by. Returns why it could not, a race or class the screen does
/// not list.
pub(super) fn press_body(vm: StockVm, req: &CharCreateReq) -> Result<(), String> {
    let race = file_of(&RACE_FILES, req.race).ok_or(format!("no race {}", req.race))?;
    let class = file_of(&CLASS_FILES, req.class).ok_or(format!("no class {}", req.class))?;
    let race_at = vm
        .number(&format!("return {}", list_index("GetAvailableRaces", race)))
        .ok_or(format!("the create screen lists no {race}"))?;
    vm.press(&format!(
        "SetSelectedRace({race_at}) SetCharacterRace({race_at}) \
         SetSelectedSex(GetSelectedSex()) SetCharacterCreateFacing(-15) \
         SetCharacterGender({})",
        u32::from(req.gender) + 1
    ));
    let class_at = vm
        .number(&format!(
            "return {}",
            list_index("GetClassesForRace", class)
        ))
        .ok_or(format!("{race} cannot be a {class}"))?;
    vm.press(&format!("CharacterClass_OnClick({class_at})"));
    let look = vm
        .create_look()
        .ok_or("the create screen built no character")?;
    let have = [
        look.skin,
        look.face,
        look.hair_style,
        look.hair_color,
        look.facial_hair,
    ];
    let want = [
        req.skin,
        req.face,
        req.hair_style,
        req.hair_color,
        req.facial_hair,
    ];
    let cycles: String = (0..5)
        .filter(|&i| have[i] != want[i])
        .map(|i| {
            format!(
                "CycleCharCustomization({}, {}) ",
                i + 1,
                i32::from(want[i]) - i32::from(have[i])
            )
        })
        .collect();
    if !cycles.is_empty() {
        vm.press(&cycles);
    }
    Ok(())
}

/// Type `name` into the create screen's name box and press Okay (`CharacterCreate_Okay`), whose
/// `CreateCharacter` reaches the glue manager's create (`0x46b290`).
pub(super) fn press_okay(vm: StockVm, name: &str) {
    vm.press(&format!(
        "CharacterCreateNameEdit:SetText({name:?}) CharacterCreate_Okay()"
    ));
}

/// The create screen's Back (`CharacterCreate_Back`): character select, which asks its roster.
pub(super) fn press_back(vm: StockVm) {
    vm.press("CharacterCreate_Back()");
}

/// The standing dialog's first button, as a click on it (`GlueDialog_OnClick(1)`).
pub(super) fn press_dialog_okay(vm: StockVm) {
    vm.press("GlueDialogButton1:Click()");
}

/// Delete `row` (0-based): the row selected, then the delete dialog's Okay, whose
/// `DeleteCharacter(CharacterSelect.selectedIndex)` (`CharacterSelect.xml:613`) reaches the glue
/// manager's delete (`0x46b2e0`).
pub(super) fn press_delete(vm: StockVm, row: usize) {
    let n = row + 1;
    vm.press(&format!(
        "CharacterSelect_SelectCharacter({n}) DeleteCharacter(CharacterSelect.selectedIndex)"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every playable race and class names its file string, and nothing else does.
    #[test]
    fn every_playable_race_and_class_has_its_file_string() {
        for race in 1..=8 {
            assert!(file_of(&RACE_FILES, race).is_some(), "race {race}");
        }
        for class in [1, 2, 3, 4, 5, 7, 8, 9, 11] {
            assert!(file_of(&CLASS_FILES, class).is_some(), "class {class}");
        }
        assert_eq!(file_of(&RACE_FILES, 9), None);
        assert_eq!(file_of(&CLASS_FILES, 6), None);
    }
}
