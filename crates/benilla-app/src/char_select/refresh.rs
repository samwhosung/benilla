//! The select screen's live refresh after [`super::screen`] spawns the tree: row texts, the
//! locked row highlight, the realm banner, the selected name, button states and the glue booth.

use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::glue::widgets::{GlueDisabled, Hilight, LockHighlight};
use crate::glue_strings::GlueStrings;
use crate::net::NetStatus;
use crate::portrait::{GlueLook, GluePreview, GlueScene, SelectLook};

use super::screen::{RealmBanner, RowText, SelectAction, SelectedName, MAX_ROWS};
use super::{class_name, Roster};

/// Refill the rows, the selected name and the realm banner when the roster changes or the screen
/// was just spawned (a return from the create screen finds the roster unchanged).
#[allow(clippy::type_complexity)]
pub(super) fn refresh_list(
    roster: Res<Roster>,
    areas: Option<Res<AreaTableRes>>,
    realms: Option<Res<crate::realm_select::Realms>>,
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
                let pvp_rp = realms
                    .as_deref()
                    .map_or((false, false), |r| r.pvp_rp(realm.realm_type));
                let suffix = match pvp_rp {
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

/// The select scene's background as `CharacterSelect` holds it for the glue session: `race` is
/// `None` while only its OnLoad has set the scene (`Model:SetModel(UI_Orc)` under Orc's fog,
/// `CharacterSelect.lua:26-31`), else the race whose file string `SetBackgroundModel` last set
/// (`CharacterSelect.lua:341-344`), drawn unfogged (`0x472110` replaces the fog callback). The
/// call is keyed on the race's file string, not its stage, so a gnome after a dwarf loads the
/// same stage again, a new instance whose scene starts over (`0x76cce0`): `stage` counts them. An
/// emptied roster selects nothing and calls neither, so the scene stays. The glue rebuild at world
/// entry restores OnLoad's.
#[derive(Resource, Default)]
pub(crate) struct SelectBackground {
    pub(super) race: Option<u8>,
    pub(super) stage: u64,
}

/// The glue rebuild (`0x46a7b0`) runs `CharacterSelect`'s OnLoad again, a new instance.
pub(super) fn reset_select_background(mut background: ResMut<SelectBackground>) {
    background.race = None;
    background.stage = background.stage.wrapping_add(1);
}

/// Feed the glue booth: the scene is the selected character's stage (`SetBackgroundModel`),
/// unfogged; with no character the last stage stays, or the OnLoad Orc scene under its fog stands
/// ([`SelectBackground`]); the look is the selected character's geared enum record.
pub(super) fn feed_glue_preview(
    roster: Res<Roster>,
    mut background: ResMut<SelectBackground>,
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
    let look = roster.selected_char().map(|c| {
        if background.race != Some(c.race) {
            background.race = Some(c.race);
            background.stage = background.stage.wrapping_add(1);
        }
        GlueLook::Select(SelectLook::from(c))
    });
    let (scene, fog) = match background.race {
        Some(race) => (GlueScene::of_race(race), None),
        None => (GlueScene::Orc, Some(GlueScene::Orc.stock_fog())),
    };
    if preview.scene != Some(scene) {
        preview.scene = Some(scene);
    }
    if preview.stage != background.stage {
        preview.stage = background.stage;
    }
    if preview.fog != fog {
        preview.fog = fog;
    }
    if preview.look != look {
        preview.look = look;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #978: an empty roster's select scene is OnLoad's Orc under Orc's fog; a selected character
    /// sets its stage, unfogged, and that stage stays when the roster empties; the glue rebuild at
    /// world entry restores OnLoad's.
    fn tauren_again() -> benilla_protocol::Character {
        let mut c = crate::char_select::test_character(1, "Moo");
        c.race = 6;
        c
    }

    #[test]
    fn the_select_scene_fogs_until_a_background_is_set_and_then_stays() {
        let mut app = App::new();
        app.init_resource::<Roster>()
            .init_resource::<SelectBackground>()
            .init_resource::<GluePreview>()
            .add_systems(Update, feed_glue_preview);
        let shown = |app: &App| {
            let p = app.world().resource::<GluePreview>();
            (p.scene, p.fog.map(|f| f.far), p.look.is_some())
        };
        app.update();
        assert_eq!(shown(&app), (Some(GlueScene::Orc), Some(270.0), false));

        let mut tauren = crate::char_select::test_character(1, "Moo");
        tauren.race = 6;
        {
            let mut roster = app.world_mut().resource_mut::<Roster>();
            roster.chars = vec![tauren];
            roster.select(Some(0));
        }
        app.update();
        assert_eq!(shown(&app), (Some(GlueScene::Tauren), None, true));

        // A gnome after a dwarf: the same stage, set again, starts over (`0x76cce0`).
        let stage = |app: &App| app.world().resource::<GluePreview>().stage;
        let mut dwarf = crate::char_select::test_character(2, "Stone");
        dwarf.race = 3;
        let mut gnome = crate::char_select::test_character(3, "Cog");
        gnome.race = 7;
        let first = stage(&app);
        for (c, row) in [(dwarf, 0), (gnome, 1)] {
            let before = stage(&app);
            {
                let mut roster = app.world_mut().resource_mut::<Roster>();
                roster.chars.insert(row, c);
                roster.select(Some(row));
            }
            app.update();
            assert_eq!(shown(&app).0, Some(GlueScene::Dwarf));
            assert_ne!(
                stage(&app),
                before,
                "a new race's background is a new instance"
            );
        }
        // Back to the dwarf, then the same row again: only a race change makes one.
        {
            let mut roster = app.world_mut().resource_mut::<Roster>();
            roster.select(Some(0));
        }
        app.update();
        let dwarf_again = stage(&app);
        assert!(dwarf_again != first);
        {
            let mut roster = app.world_mut().resource_mut::<Roster>();
            roster.select(Some(0));
        }
        app.update();
        assert_eq!(stage(&app), dwarf_again);
        {
            let mut roster = app.world_mut().resource_mut::<Roster>();
            roster.chars = vec![tauren_again()];
            roster.select(Some(0));
        }
        app.update();
        assert_eq!(shown(&app), (Some(GlueScene::Tauren), None, true));

        // The last character deleted: nothing is selected, and no background call replaces it.
        {
            let mut roster = app.world_mut().resource_mut::<Roster>();
            roster.chars.clear();
            roster.select(None);
        }
        app.update();
        assert_eq!(shown(&app), (Some(GlueScene::Tauren), None, false));

        app.world_mut()
            .run_system_cached(reset_select_background)
            .unwrap();
        app.update();
        assert_eq!(shown(&app), (Some(GlueScene::Orc), Some(270.0), false));
    }

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
