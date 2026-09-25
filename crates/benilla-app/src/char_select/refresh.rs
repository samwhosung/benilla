//! The select screen's live refresh — everything that follows the roster or the pointer after
//! [`super::screen`] spawns the tree: the row texts + visibility, the row highlight (hover, and
//! LOCKED on the selected row — the ref's `LockHighlight`), the realm banner, the selected name
//! over the model, the buttons' enabled states, and the glue-booth feed (scene by the selected
//! character's race, the geared look).

use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::glue::widgets::{GlueDisabled, Hilight, LockHighlight};
use crate::glue_strings::GlueStrings;
use crate::net::NetStatus;
use crate::portrait::{GlueLook, GluePreview, SelectLook};

use super::screen::{RealmBanner, RowText, SelectAction, SelectedName, MAX_ROWS};
use super::{class_name, Roster};

/// Refill the row texts + visibility, the selected name, and the realm banner whenever the roster
/// changes (a fresh enum, a selection move) — or when the screen was just (re)spawned (returning
/// from the create screen finds an unchanged roster; the fresh, empty tree must still fill).
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

    // Row visibility: rows with a character show; the rest hide (the ref's enumerate-then-hide).
    for (action, mut vis) in &mut rows {
        if let SelectAction::Row(i) = action {
            *vis = if *i < roster.chars.len() {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        }
    }
    // Row texts: Name; Info = `CHARACTER_SELECT_INFO` "Level %d %s" (class only — the ghost
    // variant appends "(Ghost)"); Location = the zone name off AreaTable.
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
    // The selected character's name over the model (empty list → empty, the ref's
    // UPDATE_SELECTED_CHARACTER arg 0).
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
                // `GetServerName`'s own `isPVP, isRP` pair, from the one table that owns it.
                // The banner's normal-realm arm is EMPTY, unlike the realm list's `Normal` — the
                // reference leaves `serverType = ""` when neither flag is set.
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
            // **`CharSelectRealmName:Hide()`** (`CharacterSelect.lua` l.66): with no server name
            // the reference hides the FontString outright — it has no string for this case, and
            // ours used to invent one (`"Connecting…"`). Empty text is our Hide.
            //
            // It is also all but unreachable now. The realm is chosen *before* the world dial, so
            // by the time this screen exists the session has one; what is left here is the
            // no-network harness, which is exactly the case the reference draws nothing for.
            None => String::new(),
        };
        if t.0 != new {
            t.0 = new;
        }
    }
}

/// The realm banner, composed the way `CharacterSelect_OnShow` composes it (`CharacterSelect.lua`
/// l.48-62): the down note is appended to the **name**, and the realm-type suffix goes after that
/// whole thing.
///
/// ```text
/// serverName = serverName.."\n("..TEXT(SERVER_DOWN)..")";   -- only while disconnected
/// CharSelectRealmName:SetText(serverName.." "..serverType);
/// ```
///
/// A separate function because the order is exactly what drifted: ours put the suffix first and the
/// down note last, under a comment claiming the reference's shape. Nothing showed while the suffix
/// was `"(PVP)"` — parentheses read plausibly at either end. Decision 2052 made the suffix the enGB
/// patch's `"PVP"`, a bare word, and the order became legible. A comment could not fail; this can.
fn realm_banner(name: &str, suffix: &str, down: Option<&str>) -> String {
    let name = match down {
        Some(reason) => format!("{name}\n({reason})"),
        None => name.to_string(),
    };
    // The reference appends `" "..serverType` unconditionally, leaving a trailing space on a realm
    // with no type; we trim it, since ours is a laid-out node rather than a Lua string.
    format!("{name} {suffix}").trim_end().to_string()
}

/// Per-frame interaction visuals + button states: the row highlight (hover ∪ selected — the ref's
/// `LockHighlight` on the selected row), and the enabled states — Enter World / Delete disable on
/// an empty list (the ref's `UpdateCharacterList`), Create hides at the 10-cap or disconnected.
/// (Change Realm was drawn deliberately dead under 0465 §6, until 2056 gave it something to do and
/// 2072 made it work.)
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
    // `LockHighlight` on the chosen row and nothing about *visibility*: hovering is
    // `crate::glue::glue_hilights`' question, and the two used to be one expression here.
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
    // Create New Character shows while the realm answered and a slot is free (the ref hides it
    // disconnected or at MAX_CHARACTERS_PER_REALM = 10).
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

/// Feed the glue booth from the roster: the scene is the SELECTED character's
/// race's (the ref's `SetBackgroundModel` on the enum fileString — Orc before a list arrives or
/// with an empty account, the ref's OnLoad default), the look its geared enum record. Runs every
/// frame (cheap compares inside `GluePreview` writers — the builder keys on value change).
pub(super) fn feed_glue_preview(
    roster: Res<Roster>,
    mut preview: ResMut<GluePreview>,
    mut showing: Local<Option<u64>>,
) {
    // **The facing belongs to the displayed character, not to the screen** (1533): turning one
    // character must not hand its angle to the next one you click, and a fresh character faces the
    // stage's own forward until you turn it. `GluePreview::yaw` is one resource shared by all three
    // glue screens, so the reset lives at the selection edge rather than in the resource.
    //
    // Keyed on the selection **counter**, not on who is shown: the ref's `SelectCharacter` zeroes
    // the facing unconditionally (`0x472950`, above the already-built discriminator), so selecting
    // the same index again — what a roster refresh does — re-squares the character too. A *click*
    // on the selected row is gated out one level up and never gets here (`Roster::click_row`,
    // 2194). See `Roster::select_seq`.
    if *showing != Some(roster.select_seq) {
        *showing = Some(roster.select_seq);
        preview.yaw = 0.0;
    }
    let (race, look) = match roster.selected_char() {
        Some(c) => (c.race, Some(GlueLook::Select(SelectLook::from(c)))),
        None => (2, None), // UI_Orc — the ref's initial/empty-account scene
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

    /// The composition order, pinned against `CharacterSelect.lua` l.48-62.
    #[test]
    fn the_down_note_hangs_off_the_name_and_the_type_suffix_follows_it() {
        assert_eq!(realm_banner("Kalimdor", "PVP", None), "Kalimdor PVP");
        assert_eq!(
            realm_banner("Kalimdor", "PVP", Some("Server down")),
            "Kalimdor\n(Server down) PVP",
            "the suffix follows the whole name+note, not the name alone"
        );
        // No realm type: the reference's unconditional " " is all that would be left, and a banner
        // does not end in whitespace.
        assert_eq!(realm_banner("Kalimdor", "", None), "Kalimdor");
        assert_eq!(
            realm_banner("Kalimdor", "", Some("Server down")),
            "Kalimdor\n(Server down)"
        );
    }
}
