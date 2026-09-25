//! The select screen's live refresh after [`super::screen`] spawns the tree: row texts, the
//! locked row highlight, the realm banner, the selected name, button states and the glue booth.

use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::glue::widgets::{GlueDisabled, Hilight, LockHighlight};
use crate::glue_strings::GlueStrings;
use crate::net::NetStatus;
use crate::portrait::{GlueLook, GluePreview, SelectLook};

use super::screen::{RealmBanner, RowText, SelectAction, SelectedName, MAX_ROWS};
use super::{class_name, Roster};

/// Refill the rows, the selected name and the realm banner when the roster changes or the screen
/// was just spawned (a return from the create screen finds the roster unchanged).
#[allow(clippy::type_complexity)]
pub(super) fn refresh_list(
    roster: Res<Roster>,
    areas: Option<Res<AreaTableRes>>,
    strings: Option<Res<GlueStrings>>,
    status: Res<NetStatus>,
    mut rows: Query<(&SelectAction, &mut Visibility), (With<Button>, Without<Hilight>)>,
    mut texts: Query<(&RowText, &mut Text), Without<SelectedName>>,
    mut name: Query<&mut Text, (With<SelectedName>, Without<RealmBanner>, Without<RowText>)>,
    mut banner: Query<&mut Text, (With<RealmBanner>, Without<SelectedName>, Without<RowText>)>,
    spawned: Query<Ref<SelectAction>>,
) {
    let fresh = spawned.iter().any(|r| r.is_added());
    if !roster.is_changed() && !fresh && !status.is_changed() {
        return;
    }
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);

    for (action, mut vis) in &mut rows {
        if let SelectAction::Row(i) = action {
            *vis = if *i < roster.chars.len() {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        }
    }
    // Info is `CHARACTER_SELECT_INFO` "Level %d %s" (class only; the ghost variant appends
    // "(Ghost)"); Location is the AreaTable zone name.
    for (text, mut t) in &mut texts {
        let (i, kind) = match text {
            RowText::Name(i) => (*i, 0),
            RowText::Info(i) => (*i, 1),
            RowText::Location(i) => (*i, 2),
        };
        let new = match roster.chars.get(i) {
            None => String::new(),
            Some(c) => match kind {
                0 => c.name.clone(),
                1 => {
                    let key = if c.flags & benilla_protocol::CHARACTER_FLAG_GHOST != 0 {
                        ("CHARACTER_SELECT_INFO_GHOST", "Level %d %s (Ghost)")
                    } else {
                        ("CHARACTER_SELECT_INFO", "Level %d %s")
                    };
                    strings
                        .text(key.0, key.1)
                        .replacen("%d", &c.level.to_string(), 1)
                        .replacen("%s", class_name(c.class), 1)
                }
                _ => areas
                    .as_deref()
                    .and_then(|a| a.0.name(c.zone))
                    .unwrap_or_default()
                    .to_string(),
            },
        };
        if t.0 != new {
            t.0 = new;
        }
    }
    if let Ok(mut t) = name.single_mut() {
        let new = roster
            .selected_char()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        if t.0 != new {
            t.0 = new;
        }
    }
    if let Ok(mut t) = banner.single_mut() {
        let new = match &roster.realm {
            Some(realm) => {
                // `GetServerName`'s `isPVP, isRP` pair. A normal realm has no suffix, unlike
                // the realm list's `Normal`: the reference leaves `serverType = ""`.
                let suffix = match crate::realm_select::pvp_rp(realm.realm_type) {
                    (true, true) => strings.text("RPPVP_PARENTHESES", "(RPPVP)"),
                    (false, true) => strings.text("RP_PARENTHESES", "(RP)"),
                    (true, false) => strings.text("PVP_PARENTHESES", "(PVP)"),
                    (false, false) => "",
                };
                let down = (status.last_reason.is_some() && roster.pending_pick.is_none())
                    .then(|| strings.text("SERVER_DOWN", "Server down"));
                realm_banner(&realm.name, suffix, down)
            }
            // With no server name the reference hides the banner (`CharSelectRealmName:Hide()`,
            // `CharacterSelect.lua:67`); empty text is that Hide.
            None => String::new(),
        };
        if t.0 != new {
            t.0 = new;
        }
    }
}

/// The realm banner as `CharacterSelect_OnShow` composes it (`CharacterSelect.lua:48-64`): the
/// down note is appended to the name, and the realm-type suffix follows the whole.
fn realm_banner(name: &str, suffix: &str, down: Option<&str>) -> String {
    let name = match down {
        Some(reason) => format!("{name}\n({reason})"),
        None => name.to_string(),
    };
    // The reference appends `" "..serverType` unconditionally; the trailing space is trimmed.
    format!("{name} {suffix}").trim_end().to_string()
}

/// Per-frame button states: `LockHighlight` on the selected row, Enter World and Delete disabled
/// on an empty list (`UpdateCharacterList`), Create hidden at the 10-cap or disconnected.
#[allow(clippy::type_complexity)]
pub(super) fn refresh_banner_and_buttons(
    roster: Res<Roster>,
    mut rows: Query<(&SelectAction, &mut LockHighlight), With<Button>>,
    mut disables: Query<(&SelectAction, &mut GlueDisabled)>,
    mut create_vis: Query<
        (&SelectAction, &mut Visibility),
        (With<crate::glue::widgets::GlueBtn>, Without<Hilight>),
    >,
) {
    // Only the lock: hover is `crate::glue::glue_hilights`'.
    for (action, mut locked) in &mut rows {
        let SelectAction::Row(i) = action else {
            continue;
        };
        let want = roster.selected() == Some(*i);
        if locked.0 != want {
            locked.0 = want;
        }
    }
    let have_chars = !roster.chars.is_empty();
    for (action, mut disabled) in &mut disables {
        let want = match action {
            SelectAction::EnterWorld | SelectAction::Delete => !have_chars,
            _ => false,
        };
        if disabled.0 != want {
            disabled.0 = want;
        }
    }
    // The reference hides Create disconnected or at `MAX_CHARACTERS_PER_REALM` (10).
    let show_create = roster.realm.is_some() && roster.chars.len() < MAX_ROWS;
    for (action, mut vis) in &mut create_vis {
        if matches!(action, SelectAction::CreateChar) {
            *vis = if show_create {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        }
    }
}

/// Feed the glue booth: the scene is the selected character's race (`SetBackgroundModel`; Orc
/// with no character, the reference's OnLoad default), the look its geared enum record.
pub(super) fn feed_glue_preview(
    roster: Res<Roster>,
    mut preview: ResMut<GluePreview>,
    mut showing: Local<Option<u64>>,
) {
    // Each select zeroes the facing, even of the same index (`SelectCharacter`, `0x472950`),
    // so this keys on the selection counter, not on who is shown. `GluePreview::yaw` is shared
    // by all three glue screens, so the reset lives here.
    if *showing != Some(roster.select_seq) {
        *showing = Some(roster.select_seq);
        preview.yaw = 0.0;
    }
    let (race, look) = match roster.selected_char() {
        Some(c) => (c.race, Some(GlueLook::Select(SelectLook::from(c)))),
        None => (2, None), // UI_Orc, the reference's empty-account scene
    };
    let scene = Some(crate::portrait::GlueScene::Race(race));
    if preview.scene != scene {
        preview.scene = scene;
    }
    if preview.look != look {
        preview.look = look;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The composition order of `CharacterSelect.lua:48-64`.
    #[test]
    fn the_down_note_hangs_off_the_name_and_the_type_suffix_follows_it() {
        assert_eq!(realm_banner("Kalimdor", "PVP", None), "Kalimdor PVP");
        assert_eq!(
            realm_banner("Kalimdor", "PVP", Some("Server down")),
            "Kalimdor\n(Server down) PVP",
            "the suffix follows the whole name+note, not the name alone"
        );
        assert_eq!(realm_banner("Kalimdor", "", None), "Kalimdor");
        assert_eq!(
            realm_banner("Kalimdor", "", Some("Server down")),
            "Kalimdor\n(Server down)"
        );
    }
}
