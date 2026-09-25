//! Realm selection: the realm list dialog and the policy that decides whether the player sees it.
//! Not [`crate::realmlist`], which is the logon server's address (the `realmList` CVar).
//!
//! `RealmList` is a `frameStrata="DIALOG"` frame, not a glue screen (`GlueParent.lua`'s
//! `GlueScreenInfo` has no entry for it): it is shown over the current screen and hidden again,
//! and that screen is where the player returns.
//!
//! The IO thread publishes the list from two parks (`crate::net::io`), the login-side realm park
//! and the character park, and takes one answer. The policy, in order:
//!
//! 1. `WOW_REALM`, matched case-insensitively; a name not on the list falls through.
//! 2. The realm this session is on, so a logout or a dropped connection returns to it.
//! 3. The remembered realm, the persisted `realmName` CVar.
//! 4. An unattended run (`crate::run_mode::unattended`) takes the first realm that is up.
//! 5. Otherwise the dialog.
//!
//! Change Realm costs a world dial, not a login. `RequestRealmList` (`0x46ecf0` -> `0x46b8d0`)
//! begins a realm query on the realmd socket, which stays open until world entry (`0x46b70c`), and
//! names no screen; only `ConnectToRealm` (`0x46b210`) drops the world connection, once a realm is
//! chosen. So the character park serves the list in place and Cancel there does nothing.

mod input;
mod load;
mod screen;
mod smoke;

pub(crate) use load::pvp_rp;

use bevy::prelude::*;

use crate::net::{RealmChoice, RealmListMessage, RealmRequest};
use benilla_protocol::RealmInfo;

/// The persisted 1.12 CVar naming the last realm connected to (`0x83f2d0`); the SavedVariables
/// path (`0x8539a8`) is built from it.
pub(crate) const CVAR_REALM_NAME: &str = "realmName";

/// `REALM_LIST_REFRESH_TIME` (`RealmList.lua:3`): the re-request interval while the
/// dialog is up.
const REFRESH_SECS: f32 = 5.0;

/// The realm list, the selection on it, and the policy's memory.
#[derive(Resource, Default)]
pub(crate) struct Realms {
    /// Every realm the auth server advertised, in wire order.
    pub(super) realms: Vec<RealmInfo>,
    /// The highlighted row by realm name, so a refresh that adds or drops a realm cannot move it.
    selected: Option<String>,
    /// The category tab in front (`RealmList.selectedCategory`), by the wire's category byte.
    pub(super) category: Option<u8>,
    /// First visible row within the selected category (`RealmList.offset`).
    pub(super) offset: usize,
    /// Which column the list is sorted on, and which way.
    pub(super) sort: Sort,
    /// `RealmList:IsVisible()`. While set, a published list is a refresh and is never
    /// auto-answered.
    pub(super) shown: bool,
    /// `GetRealmInfo`'s `currentRealm`, the row highlighted when nothing else is. The reference
    /// case-folds the `realmName` CVar (`0x46ef9e`, via `0x5ab7d0` on the handle from `0x63db90`)
    /// against the realm name, and `ConnectToRealm` (`0x46b217`) writes the CVar before it dials.
    /// Ours is set when we answer a park and seeded from the persisted CVar, because benilla
    /// writes `realmName` only at world entry (`benilla_ui::script::UiScript::set_realm_name`).
    current: Option<String>,
    /// Seconds until the next refresh request.
    refresh_in: f32,
    /// `WOW_REALM`, read once.
    env_realm: Option<String>,
    env_read: bool,
}

/// One of the four sort columns, `SortRealms`' string argument.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SortKey {
    /// `SortRealms("characters")`, comparator case 0: the account's character count.
    Characters,
    /// `SortRealms("load")`, case 1: the computed load band, not the raw population.
    Load,
    /// `SortRealms("name")`, case 2: a case-folding compare.
    Name,
    /// `SortRealms("mode")`, case 3: the `(pvp, rp)` pair.
    Type,
}

/// The sort config (`0xb41f40`): four `{key, descending}` records. The comparator (`0x46e790`)
/// walks them in order and the first that does not tie decides, negated by its own `descending`.
/// The default, `[characters, load, name, mode]` all ascending, is written once per process
/// (`0x46e430`), so a click persists for the session. Ascending characters is higher count first.
#[derive(Clone, Copy)]
pub(super) struct Sort(pub(super) [(SortKey, bool); 4]);

impl Default for Sort {
    fn default() -> Self {
        Self([
            (SortKey::Characters, false),
            (SortKey::Load, false),
            (SortKey::Name, false),
            (SortKey::Type, false),
        ])
    }
}

impl Sort {
    /// A column header click (`0x46e9b0`): the front key toggles its direction; any other moves
    /// to the front keeping its direction, and the records it passes shift down.
    pub(super) fn click(&mut self, key: SortKey) {
        if self.0[0].0 == key {
            self.0[0].1 = !self.0[0].1;
            return;
        }
        let Some(at) = self.0.iter().position(|(k, _)| *k == key) else {
            return;
        };
        let record = self.0[at];
        self.0.copy_within(0..at, 1);
        self.0[0] = record;
    }
}

impl Realms {
    /// The rows the screen draws: indices into [`Self::realms`] for the selected category, sorted.
    pub(super) fn rows(&self) -> Vec<usize> {
        let category = self.category;
        let (mean, stddev) = self.stats();
        let mut rows: Vec<usize> = self
            .realms
            .iter()
            .enumerate()
            .filter(|(_, r)| category.is_none_or(|c| r.category == c))
            .map(|(i, _)| i)
            .collect();
        rows.sort_by(|&a, &b| {
            let (ra, rb) = (&self.realms[a], &self.realms[b]);
            for (key, descending) in self.sort.0 {
                let ord = match key {
                    // Higher count first.
                    SortKey::Characters => rb.characters.cmp(&ra.characters),
                    SortKey::Load => {
                        let la = load::realm_load_classify(ra.flags, ra.population, mean, stddev);
                        let lb = load::realm_load_classify(rb.flags, rb.population, mean, stddev);
                        la.total_cmp(&lb)
                    }
                    SortKey::Name => ra.name.to_lowercase().cmp(&rb.name.to_lowercase()),
                    SortKey::Type => load::pvp_rp(ra.realm_type).cmp(&load::pvp_rp(rb.realm_type)),
                };
                if ord != std::cmp::Ordering::Equal {
                    return if descending { ord.reverse() } else { ord };
                }
            }
            // The reference's characters case answers +1 both ways on equal nonzero counts (its
            // tie-break strcmp is clobbered), a defect not reproduced: all four tied is Equal.
            std::cmp::Ordering::Equal
        });
        rows
    }

    /// The category bytes present, ascending, one tab each; `RealmList_UpdateTabs` hides the strip
    /// when there is only one.
    pub(super) fn categories(&self) -> Vec<u8> {
        let mut cats: Vec<u8> = self.realms.iter().map(|r| r.category).collect();
        cats.sort_unstable();
        cats.dedup();
        cats
    }

    /// The load distribution, over every realm, not the selected category: `0x46e510` walks the
    /// flat all-categories array, so switching tabs moves no band.
    pub(super) fn stats(&self) -> (f32, f32) {
        let pops: Vec<f32> = self.realms.iter().map(|r| r.population).collect();
        load::realm_load_stats(&pops)
    }

    /// The highlighted realm, if it is still on the list.
    pub(super) fn selected(&self) -> Option<&RealmInfo> {
        let name = self.selected.as_deref()?;
        self.realms.iter().find(|r| r.name == name)
    }

    /// Highlight a realm by name.
    pub(super) fn select(&mut self, name: &str) {
        self.selected = Some(name.to_string());
    }

    /// Whether Okay is live: `RealmListUpdate` disables it and only a highlighted, online row
    /// re-enables it.
    pub(super) fn can_enter(&self) -> bool {
        self.selected().is_some_and(|r| !is_down(r))
    }

    /// `RealmList:Show()`: reset the scroll, highlight [`Self::current`], start the refresh timer.
    fn open(&mut self) {
        self.shown = true;
        self.offset = 0;
        self.selected = self
            .current
            .as_deref()
            .and_then(|want| {
                self.realms
                    .iter()
                    .find(|r| r.name.eq_ignore_ascii_case(want))
            })
            .map(|r| r.name.clone());
        self.refresh_in = REFRESH_SECS;
    }

    /// Answer a park with a realm and remember it as [`Self::current`].
    pub(super) fn enter(&mut self, choice: &RealmChoice, name: String) {
        self.current = Some(name.clone());
        let _ = choice.0.send(RealmRequest::Enter(name));
    }

    /// `RealmList:Hide()`, and nothing else.
    pub(super) fn hide(&mut self) {
        self.shown = false;
    }
}

/// `GetRealmInfo`'s `realmDown`: flag bit 0x02, greyed and not enterable; the reference's
/// auto-pick (`0x46ea20`) skips a realm on these low bits.
pub(crate) fn is_down(realm: &RealmInfo) -> bool {
    realm.flags & 0x02 != 0
}

/// `GetRealmInfo`'s `invalidRealm`: flag bit 0x01, painted red but still enterable.
pub(super) fn is_invalid(realm: &RealmInfo) -> bool {
    realm.flags & 0x01 != 0
}

/// The realm-selection subsystem: the park's policy plus the screen.
pub(crate) struct RealmSelectPlugin;

impl Plugin for RealmSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Realms>().add_systems(
            Update,
            (
                apply_realm_policy,
                // Ungated by state: the dialog stands over login and character select alike.
                screen::drive_screen,
                // Input before the row refresh, so a click shows on the frame it landed.
                smoke::debug_realm_smoke,
                (
                    input::clicks,
                    input::keys,
                    tick_refresh,
                    screen::refresh_rows,
                )
                    .chain()
                    .run_if(|realms: Res<Realms>| realms.shown),
            )
                .chain()
                .after(benilla_world::schedule::WorldStage::Net)
                .before(crate::glue::GlueVisuals),
        );
    }
}

/// Answer the realm park, or raise the dialog; the policy for both parks, in every state.
fn apply_realm_policy(
    mut msgs: MessageReader<RealmListMessage>,
    mut realms: ResMut<Realms>,
    choice: Res<RealmChoice>,
    cvars: Res<crate::cvars::Cvars>,
) {
    if !realms.env_read {
        realms.env_read = true;
        realms.env_realm = std::env::var("WOW_REALM").ok().filter(|s| !s.is_empty());
    }
    for msg in msgs.read() {
        realms.realms = msg.realms.clone();
        realms.refresh_in = REFRESH_SECS;
        // Keep the tab across a refresh while it still exists, else the first.
        let cats = realms.categories();
        if !realms.category.is_some_and(|c| cats.contains(&c)) {
            realms.category = cats.first().copied();
        }

        // A list published while the dialog is up is a refresh, never a question.
        if realms.shown {
            continue;
        }
        // The registered default is empty: a client that has never connected.
        let remembered = cvars
            .get(CVAR_REALM_NAME)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if realms.current.is_none() {
            realms.current = remembered.clone();
        }
        match auto_answer(
            &realms,
            remembered.as_deref(),
            crate::run_mode::unattended(),
        ) {
            Some(name) => {
                realms.select(&name);
                realms.enter(&choice, name);
            }
            None => realms.open(),
        }
    }
}

/// The realm that answers this list without the player, or `None` to show the dialog: the policy
/// ladder in the module doc. Every arm requires the realm to be up.
fn auto_answer(realms: &Realms, remembered: Option<&str>, unattended: bool) -> Option<String> {
    // Case-insensitive: vmangos does not normalise realm names. `current` outranks the persisted
    // CVar, which benilla writes only at world entry, so a logout before it still returns here.
    let named = [
        realms.env_realm.as_deref(),
        realms.current.as_deref(),
        remembered,
    ]
    .into_iter()
    .flatten()
    .find_map(|want| {
        realms
            .realms
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(want) && !is_down(r))
            .map(|r| r.name.clone())
    });
    named.or_else(|| {
        // Nobody to click Okay: the first realm up, in drawn order.
        unattended
            .then(|| {
                realms
                    .rows()
                    .into_iter()
                    .map(|i| &realms.realms[i])
                    .find(|r| !is_down(r))
                    .map(|r| r.name.clone())
            })
            .flatten()
    })
}

/// Change Realm: raise the dialog over character select and ask for a fresh list; the session is
/// untouched.
///
/// Deviation: the reference shows the frame when `OPEN_REALM_LIST` arrives; ours shows at once
/// against the list already in hand, so the click does not wait a round trip.
pub(crate) fn open_over_char_select(realms: &mut Realms, choice: &RealmChoice) {
    realms.open();
    let _ = choice.0.send(RealmRequest::Refresh);
}

/// `RealmList_OnUpdate`: re-request the list every `REALM_LIST_REFRESH_TIME` while shown
/// (`RealmList_OnHide` cancels the query).
fn tick_refresh(time: Res<Time>, mut realms: ResMut<Realms>, choice: Res<RealmChoice>) {
    realms.refresh_in -= time.delta_secs();
    if realms.refresh_in <= 0.0 {
        realms.refresh_in = REFRESH_SECS;
        let _ = choice.0.send(RealmRequest::Refresh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn realm(
        name: &str,
        category: u8,
        realm_type: u32,
        flags: u8,
        chars: u8,
        pop: f32,
    ) -> RealmInfo {
        RealmInfo {
            name: name.into(),
            address: "127.0.0.1:8085".into(),
            population: pop,
            characters: chars,
            realm_type,
            flags,
            category,
            id: 0,
        }
    }

    /// The realm names in the order the screen would draw them.
    fn names(r: &Realms) -> Vec<String> {
        r.rows().iter().map(|&i| r.realms[i].name.clone()).collect()
    }

    fn list(realms: Vec<RealmInfo>) -> Realms {
        let category = realms.first().map(|r| r.category);
        Realms {
            realms,
            category,
            ..Realms::default()
        }
    }

    /// The offline and invalid bits are independent.
    #[test]
    fn offline_and_invalid_are_separate_bits() {
        assert!(is_down(&realm("a", 1, 0, 0x02, 0, 1.0)));
        assert!(!is_invalid(&realm("a", 1, 0, 0x02, 0, 1.0)));
        assert!(is_invalid(&realm("a", 1, 0, 0x01, 0, 1.0)));
        assert!(!is_down(&realm("a", 1, 0, 0x01, 0, 1.0)));
        assert!(
            is_down(&realm("a", 1, 0, 0x03, 0, 1.0)) && is_invalid(&realm("a", 1, 0, 0x03, 0, 1.0))
        );
        // The load sentinels live in the high bits.
        let sentinel = realm("a", 1, 0, 0xE0, 0, 1.0);
        assert!(!is_down(&sentinel) && !is_invalid(&sentinel));
    }

    /// The load distribution spans every realm, so switching tabs relabels no band.
    #[test]
    fn a_category_tab_scopes_the_rows_but_not_the_load_distribution() {
        let mut r = list(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Beta", 1, 0, 0, 0, 1.0),
            realm("Gamma", 2, 0, 0, 0, 400.0),
        ]);
        r.category = Some(1);
        assert_eq!(r.rows().len(), 2, "the tab scopes the rows");

        let (mean_on_tab_one, _) = r.stats();
        r.category = Some(2);
        assert_eq!(r.rows().len(), 1);
        let (mean_on_tab_two, _) = r.stats();
        assert_eq!(
            mean_on_tab_one, mean_on_tab_two,
            "the distribution is global — the tab must not move it"
        );
        // (1 + 1 + 400) / 3.
        assert!((mean_on_tab_one - 134.0).abs() < 1e-3, "{mean_on_tab_one}");
    }

    /// The reference hides the tab strip for a single category.
    #[test]
    fn one_category_is_the_ordinary_case_and_has_no_tabs() {
        let r = list(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Beta", 1, 0, 0, 0, 1.0),
        ]);
        assert_eq!(r.categories(), vec![1]);
    }

    /// The default order is characters, load, name, mode, all ascending; more characters first.
    #[test]
    fn the_default_sort_puts_realms_you_play_on_first() {
        let mut r = list(vec![
            realm("Zeta", 1, 0, 0, 0, 1.0),
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Mu", 1, 0, 0, 3, 1.0),
        ]);
        assert_eq!(names(&r), ["Mu", "Alpha", "Zeta"], "Mu has characters");
        // Equal counts fall through to load, then name.
        r.realms[2].characters = 0;
        assert_eq!(names(&r), ["Alpha", "Mu", "Zeta"]);
    }

    #[test]
    fn a_column_click_moves_it_to_the_front_and_only_a_re_click_flips_it() {
        let mut sort = Sort::default();
        assert_eq!(sort.0[0], (SortKey::Characters, false));

        sort.click(SortKey::Name);
        assert_eq!(sort.0[0], (SortKey::Name, false));
        assert_eq!(
            sort.0[1..],
            [
                (SortKey::Characters, false),
                (SortKey::Load, false),
                (SortKey::Type, false)
            ],
            "the records it passed shift down rather than being dropped"
        );

        sort.click(SortKey::Name);
        assert_eq!(sort.0[0], (SortKey::Name, true), "a re-click flips it");

        sort.click(SortKey::Load);
        assert_eq!(sort.0[0], (SortKey::Load, false));
        assert_eq!(
            sort.0[1],
            (SortKey::Name, true),
            "Name keeps the direction it was toggled to"
        );
    }

    /// Reversing the primary column does not reverse the tie-breaks behind it.
    #[test]
    fn each_sort_record_carries_its_own_direction() {
        let mut r = list(vec![
            realm("Alpha", 1, 0, 0, 1, 1.0),
            realm("Mu", 1, 0, 0, 1, 1.0),
            realm("Zeta", 1, 0, 0, 5, 1.0),
        ]);
        r.sort.click(SortKey::Characters); // already primary → descending
        assert_eq!(
            names(&r),
            ["Alpha", "Mu", "Zeta"],
            "counts reversed (fewest first), but the name tie-break stays ascending"
        );
    }

    #[test]
    fn the_selection_survives_a_realm_appearing_above_it() {
        let mut r = list(vec![realm("Mu", 1, 0, 0, 0, 1.0)]);
        r.select("Mu");
        r.realms.insert(0, realm("Alpha", 1, 0, 0, 0, 1.0));
        assert_eq!(r.selected().map(|r| r.name.as_str()), Some("Mu"));
    }

    /// A realm that left the list is not selected, and Okay is dead.
    #[test]
    fn a_realm_that_left_the_list_is_no_longer_the_selection() {
        let mut r = list(vec![realm("Mu", 1, 0, 0, 0, 1.0)]);
        r.select("Mu");
        r.realms.clear();
        assert!(r.selected().is_none());
        assert!(!r.can_enter());
    }

    /// With no current realm nothing is highlighted and Okay stays disabled, as in
    /// `RealmListUpdate`.
    #[test]
    fn opening_the_dialog_highlights_the_realm_you_are_on() {
        let mut r = list(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Mu", 1, 0, 0, 3, 1.0),
        ]);
        r.current = Some("Mu".into());
        r.open();
        assert!(r.shown);
        assert_eq!(r.selected().map(|r| r.name.as_str()), Some("Mu"));
        assert!(r.can_enter());

        let mut fresh = list(vec![realm("Alpha", 1, 0, 0, 0, 1.0)]);
        fresh.open();
        assert!(fresh.selected().is_none(), "nothing to preselect");
        assert!(
            !fresh.can_enter(),
            "and Okay is dead until a row is clicked"
        );

        // A remembered realm that is no longer listed cannot seat a highlight either.
        let mut gone = list(vec![realm("Alpha", 1, 0, 0, 0, 1.0)]);
        gone.current = Some("Elsewhere".into());
        gone.open();
        assert!(gone.selected().is_none());
    }

    /// `RealmList_OnCancel` is `Hide()`; no screen change.
    #[test]
    fn hide_is_the_whole_of_cancel() {
        let mut r = list(vec![realm("Alpha", 1, 0, 0, 0, 1.0)]);
        r.current = Some("Alpha".into());
        r.open();
        r.hide();
        assert!(!r.shown);
        assert_eq!(
            r.selected().map(|r| r.name.as_str()),
            Some("Alpha"),
            "and the list keeps its state for the next time it is raised"
        );
    }

    #[test]
    fn the_auto_answer_ladder_is_env_then_remembered_then_unattended() {
        let mut r = list(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Mu", 1, 0, 0, 0, 1.0),
        ]);
        assert_eq!(
            auto_answer(&r, None, false),
            None,
            "attended: show the dialog"
        );
        assert_eq!(auto_answer(&r, None, true).as_deref(), Some("Alpha"));
        assert_eq!(auto_answer(&r, Some("mu"), false).as_deref(), Some("Mu"));
        r.env_realm = Some("ALPHA".into());
        assert_eq!(
            auto_answer(&r, Some("Mu"), false).as_deref(),
            Some("Alpha"),
            "WOW_REALM outranks the remembered realm"
        );
    }

    /// The session's realm outranks the persisted `realmName`, which is the previous session's.
    #[test]
    fn a_relogin_returns_to_the_realm_this_session_is_on() {
        let mut r = list(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Mu", 1, 0, 0, 0, 1.0),
        ]);
        assert_eq!(auto_answer(&r, None, false), None, "nothing to go on: ask");
        r.current = Some("Mu".into());
        assert_eq!(auto_answer(&r, None, false).as_deref(), Some("Mu"));
        assert_eq!(
            auto_answer(&r, Some("Alpha"), false).as_deref(),
            Some("Mu"),
            "and it outranks the persisted realmName, which is the PREVIOUS session's"
        );
    }

    #[test]
    fn a_stale_or_offline_remembered_realm_falls_through() {
        let r = list(vec![
            realm("Down", 1, 0, 0x02, 0, 1.0),
            realm("Up", 1, 0, 0, 0, 1.0),
        ]);
        assert_eq!(auto_answer(&r, Some("Gone"), false), None);
        assert_eq!(auto_answer(&r, Some("Down"), false), None);
        assert_eq!(
            auto_answer(&r, Some("Down"), true).as_deref(),
            Some("Up"),
            "unattended skips the offline row rather than dialing it"
        );
    }

    /// The reference disables an offline realm's Okay.
    #[test]
    fn an_offline_realm_cannot_be_entered() {
        let mut r = list(vec![realm("Down", 1, 0, 0x02, 0, 1.0)]);
        r.select("Down");
        assert!(r.selected().is_some());
        assert!(!r.can_enter());
    }
}
