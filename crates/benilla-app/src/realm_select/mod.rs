//! **Realm selection** — the glue screen between the login and character select, and the policy
//! that decides whether the player ever sees it.
//!
//! For the project's whole life the client took `logon.realms.first()`: whichever realm the auth
//! server happened to list first was the realm you played on, the reference's `Change Realm` button
//! was drawn deliberately disabled, and the character screen's realm banner had nothing to show
//! until the character list arrived carrying the realm as a passenger — which is why it read
//! `Connecting…`, a string the real client does not have. The banner was the symptom; this module
//! is the missing subsystem.
//!
//! **Not to be confused with [`crate::realmlist`]**, which is the *address of the logon server* —
//! the reference's `realmList` CVar and its `realmlist.wtf`. The collision is the reference's own
//! (`realmList` the CVar, `RealmList` the window) and is kept rather than invented away: one is
//! where you dial, this is what answers.
//!
//! ## Not a screen — a dialog
//!
//! `RealmList` is **not** one of the glue screens. `GlueParent.lua`'s `GlueScreenInfo` table has
//! entries for `login`, `charselect`, `realmwizard`, `charcreate`, `patchdownload`, `movie` and
//! `credits` — and none for the realm list; `RealmList.xml` declares a `frameStrata="DIALOG"`
//! frame that `RealmList_OnEvent` simply `Show()`s, and `RealmList_OnCancel` simply `Hide()`s.
//! Whatever screen it went up over is still there underneath and is what the player is returned to.
//!
//! This shipped as a `ClientState` instead, and every symptom the director reported came out of
//! that one error: Cancel had to pick a state to go to (it picked the login screen, so Change
//! Realm's Cancel threw you out of the session), the character screen was torn down behind it, and
//! — the bad one — a roster arriving while that state was up moved nothing, so Okay looked inert
//! while the IO thread walked on to the character park. The next Cancel then sent an answer no
//! park was listening for and every later login queued behind a thread two parks away: the client
//! hung on "Connecting" with nothing in the log. It is a `shown` flag now, like the AddOns panel
//! it shares a plate with, and the screen underneath is never touched.
//!
//! ## The parks
//!
//! The IO thread publishes the list from **two** parks (`crate::net::io`) and takes its answer on
//! one channel: the *login-side* realm park (logon → **which realm?** → world handshake), and the
//! *character* park, where Change Realm raises the same list over the select screen and the
//! session underneath stays live. At each park the thread publishes facts and blocks; the app owns
//! the policy. Here the policy is:
//!
//! 1. **`WOW_REALM`**, when set — the dev fast path past the screen, the realm-side twin of
//!    `WOW_CHAR`. Matched case-insensitively; a name that is not on the list falls through rather
//!    than failing, so a stale env var cannot strand a session.
//! 2. **The realm this session is on** — a logout or a dropped connection comes back through this
//!    park, and the answer is the realm you were already playing on, not a question.
//! 3. **The remembered realm** — the `realmName` CVar, which 1.12 registers as a *persisted* CVar
//!    for exactly this ("last realm connected to"). A launch that recognises its realm goes
//!    straight to character select, and `Change Realm` is how you get back here.
//! 4. **An unattended run takes the first realm that is up** — `crate::run_mode::unattended`
//!    (`WOW_UNATTENDED`/`WOW_CAPTURE`/`WOW_RIG`). A smoke run, a capture and a rig leg have nobody
//!    in the room to click OK, and a screen that waits forever is how a park becomes a hang. This
//!    is deliberately `unattended` and not `env_login` — 1769's distinction: credentials in the
//!    environment do not mean nobody is watching, but these three cannot be a person.
//! 5. **Otherwise the dialog**, and the player picks.
//!
//! ## Change Realm
//!
//! Costs a world dial, not a login. The SRP6 session key authenticates against any world server on
//! the account's realm list and the list itself is already in hand, so the character park serves
//! the list **in place** — refreshing it on the realmd connection the cycle still holds — and only
//! leaves when a realm is chosen. Cancel there does nothing at all, which is what makes it Cancel.
//!
//! That is the reference's shape, byte for byte:
//! `CharacterSelect_ChangeRealm()` is `PlaySound + RequestRealmList(1)` and **names no screen**;
//! `RequestRealmList` (`0x46ecf0` → `0x46b8d0`) sets `screenState = 4` and begins a
//! `COP_GET_REALMS` on the realmd socket that is still open (it is closed at *world entry*,
//! `0x46b70c`, not on the way to character select); and the world connection is **not** dropped by
//! the button — only `ConnectToRealm` (`0x46b210`) drops it, once a realm is confirmed, deferring
//! its dial to the disconnect handler. So the whole cost of opening the list is a list request,
//! and the whole cost of cancelling is nothing.

mod input;
mod load;
mod screen;
mod smoke;

pub(crate) use load::pvp_rp;

use bevy::prelude::*;

use crate::net::{RealmChoice, RealmListMessage, RealmRequest};
use benilla_protocol::RealmInfo;

/// The CVar naming the realm we are on — a **real 1.12 CVar** (`0x83f2d0`, persisted; the client
/// builds its SavedVariables path `0x8539a8` from it), already registered by [`crate::cvars`] and
/// pushed into the VM at world entry by `benilla_ui::script::UiScript::set_realm_name`.
///
/// Read here for the remembered-realm auto-pick. Its persisted value is the *previous* session's
/// realm, which is precisely the question this policy asks.
pub(crate) const CVAR_REALM_NAME: &str = "realmName";

/// How often the realm list is re-requested while the screen is up — `REALM_LIST_REFRESH_TIME`
/// (`GlueXML/RealmList.lua` l.3), the reference's own 5 seconds. `RealmList_OnUpdate` counts it
/// down and fires `RequestRealmList`, which is how a realm going offline or filling up shows up
/// without a re-login.
const REFRESH_SECS: f32 = 5.0;

/// The realm list, the selection on it, and the policy's memory.
#[derive(Resource, Default)]
pub(crate) struct Realms {
    /// Every realm the auth server advertised, in wire order.
    pub(super) realms: Vec<RealmInfo>,
    /// The highlighted row, **by realm name** — the list is re-published every few seconds and a
    /// row index would drift under the player when a realm appears or disappears.
    selected: Option<String>,
    /// The category tab in front (`RealmList.selectedCategory`), by the wire's category byte.
    pub(super) category: Option<u8>,
    /// First visible row within the selected category (`RealmList.offset`).
    pub(super) offset: usize,
    /// Which column the list is sorted on, and which way.
    pub(super) sort: Sort,
    /// **Is the dialog up?** The reference's `RealmList:IsVisible()`, and the whole of its
    /// screen-state: a frame that is shown over the current glue screen and hidden again.
    ///
    /// It is also the auto-answer's off switch. While the player is looking at the list, no
    /// remembered realm may answer it for them — without that, the five-second refresh would
    /// re-publish and the policy would enter the very realm they opened the dialog to leave.
    pub(super) shown: bool,
    /// The realm this client considers **current** — the reference's `GetRealmInfo` `currentRealm`
    /// return, which is the row `RealmListUpdate` highlights when nothing else is selected.
    ///
    /// **It is a string compare against the `realmName` CVar, and nothing else** (`0x46ef9e` reads
    /// that CVar's current value through `0x5ab7d0` — the handle registered at `0x63db90`, help
    /// `"Last realm connected to"` — and case-folds it against the realm record's
    /// inline name at `realm+9`, pushing `1.0` on a match and `nil` otherwise; a 15-site census of
    /// that getter found no competing mechanism anywhere in the image). It marks the realm you are
    /// *connected to* on the character screen only because `ConnectToRealm` (`0x46b217`) writes the
    /// CVar **before** it dials.
    ///
    /// Ours is that same fact tracked one step earlier: set when we answer a park with a realm,
    /// seeded from the persisted CVar before we have. benilla's own `realmName` write lands at
    /// world entry (`benilla_ui::script::UiScript::set_realm_name`), which is after the point the
    /// highlight needs it, and moving that write is a change to the SavedVariables path this
    /// screen has no business making.
    current: Option<String>,
    /// Seconds until the next refresh request.
    refresh_in: f32,
    /// `WOW_REALM`, read once.
    env_realm: Option<String>,
    env_read: bool,
}

/// One of the realm list's four sort columns — `SortRealms`' string argument, as a type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SortKey {
    /// `SortRealms("characters")`, comparator case 0 — the per-account character count.
    Characters,
    /// `SortRealms("load")`, case 1 — the computed load band, not the raw population.
    Load,
    /// `SortRealms("name")`, case 2 — a case-folding compare.
    Name,
    /// `SortRealms("mode")`, case 3 — the `(pvp, rp)` pair.
    Type,
}

/// **The sort config: four `{key, descending}` records, walked in order.**
///
/// Not one key with a name tie-break, which is what this was first written as. The reference keeps
/// all four (`0xb41f40`), and the comparator (`0x46e790`) walks them in order, stops at the first
/// that does not tie, and negates the result if *that record's own* `descending` is set.
///
/// **The default is `[characters, load, name, mode]`, all ascending** — written once per process at
/// `0x46e430`, so it is what a player sees on a fresh launch, and a column click persists for the
/// whole session rather than resetting when the list refreshes. It is emphatically not
/// name-ascending, which is the natural guess and the one this code shipped with.
///
/// "Ascending" for `Characters` means **higher count first**: the comparator returns `-1` iff
/// `B.chars < A.chars`. Realms you already play on float to the top, which is the point.
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
    /// A column header click — the reference's `realm_set_primary_key` (`0x46e9b0`), which is
    /// **move-to-front**: clicking the key already in record 0 toggles its own direction; clicking
    /// any other shifts the records above it down and moves it to the front *keeping* the
    /// direction it already had.
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
    /// The realms in the selected category, already ordered — the rows the screen draws, as
    /// indices into [`Self::realms`].
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
            // Walk the four records in order; the first that does not tie decides, negated by
            // *its own* direction rather than by a single global one.
            for (key, descending) in self.sort.0 {
                let ord = match key {
                    // Higher count first — see `Sort`.
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
            // All four tied. The reference returns 0 here too — but its `characters` case is
            // **not antisymmetric** (on equal nonzero counts it answers `+1` for both `(a,b)` and
            // `(b,a)`, because the strcmp meant to break that tie is clobbered one instruction
            // later — a real, reachable defect, byte-verified). We do not reproduce it: a
            // comparator that disagrees with itself is a logic error Rust's sort is entitled to
            // punish, and the visible cost of the divergence is that two realms with the same
            // character count keep a stable order instead of an arbitrary one.
            std::cmp::Ordering::Equal
        });
        rows
    }

    /// The category bytes present, ascending — one tab each. The reference hides the tab strip
    /// entirely when there is only one (`RealmList_UpdateTabs`: `if arg.n == 1 then tab:Hide()`),
    /// which is every ordinary private server.
    pub(super) fn categories(&self) -> Vec<u8> {
        let mut cats: Vec<u8> = self.realms.iter().map(|r| r.category).collect();
        cats.sort_unstable();
        cats.dedup();
        cats
    }

    /// The population mean and scaled std-dev — the distribution each row's load band is placed in.
    ///
    /// **Over EVERY realm the client knows, not over the selected category.** This looked like a
    /// per-category statistic and was written that way; `realm_load_stats` (`0x46e510`) is in fact
    /// the whole realm-list rebuild, and its accumulator loop walks the flat all-categories array
    /// unconditionally — the per-category bucketing in the same loop is a side effect, never a
    /// bound. So a realm's word is measured against every other realm on the account's list, and
    /// switching tabs does not move any row's band.
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

    /// Whether the OK button is live: a realm is highlighted and it is not offline.
    ///
    /// `RealmListUpdate` opens with `RealmListOkButton:Disable()` and only the highlighted-row
    /// branches re-enable it, so a list with nothing selected has a dead Okay — which is what a
    /// fresh login shows, since there is no current realm to preselect.
    pub(super) fn can_enter(&self) -> bool {
        self.selected().is_some_and(|r| !is_down(r))
    }

    /// Raise the dialog (`RealmList:Show()` → `RealmList_OnShow`): reset the scroll, seat the
    /// highlight on [`Self::current`], and start the refresh countdown.
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

    /// Answer a park with a realm — and remember it as [`Self::current`], the way the reference's
    /// `ConnectToRealm` writes `realmName` on its way to the dial.
    pub(super) fn enter(&mut self, choice: &RealmChoice, name: String) {
        self.current = Some(name.clone());
        let _ = choice.0.send(RealmRequest::Enter(name));
    }

    /// `RealmList:Hide()` — and nothing else. The screen underneath was never touched.
    pub(super) fn hide(&mut self) {
        self.shown = false;
    }
}

/// **Offline** — the realm is up in the list but not accepting connections (`realmDown` in
/// `GetRealmInfo`, the row the reference greys out and refuses to enter).
///
/// Bit 0x02 of the wire's realm-flags byte. The client's realm record keeps the same byte at
/// `[realm+0x08]` and `realm_select` (0x46ea20) skips a realm on its low bits when auto-picking.
pub(crate) fn is_down(realm: &RealmInfo) -> bool {
    realm.flags & 0x02 != 0
}

/// **Invalid** — the realm rejects this client build (`invalidRealm`, the row the reference paints
/// red but still lets you try). Bit 0x01 of the same byte.
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
                // Ungated by state on purpose: the dialog stands over the login screen and over
                // character select alike, and `shown` is the only thing that decides.
                screen::drive_screen,
                // Input first, then the row refresh — a click that moved the highlight shows on
                // the frame it landed, not the next one.
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

/// Answer the realm park — or raise the dialog and let the player answer it.
///
/// Runs in every state: the list is published from the login-side park *and* from the character
/// park, and this one system is the policy for both.
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
        // Keep the tab across a refresh when it still names something real; a first list (or one
        // that dropped what we had) falls back to the first.
        let cats = realms.categories();
        if !realms.category.is_some_and(|c| cats.contains(&c)) {
            realms.category = cats.first().copied();
        }

        // **A list published while the dialog is up is a refresh, never a question.** The player
        // is looking at it; answering over their shoulder with the realm they are standing on is
        // the button-that-does-nothing bug in its purest form.
        if realms.shown {
            continue;
        }
        // The registered default is the empty string — a client that has never connected.
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

/// **Which realm answers this list without the player** — or `None`, meaning show the screen.
///
/// The arms, in order: an explicit `WOW_REALM`, the realm remembered from last time, then (only
/// when nobody is in the room) the first realm the list draws. Pure, and lifted out of the system
/// around it, because this ladder *is* the policy — the rest is plumbing.
///
/// Every arm requires the realm to be **up**. Auto-entering a realm the server says is offline
/// would replace a screen that can explain itself with a dial that just fails.
fn auto_answer(realms: &Realms, remembered: Option<&str>, unattended: bool) -> Option<String> {
    // The named arms, matched case-insensitively — vmangos does not normalise realm names and a
    // config file is hand-editable.
    //
    // **`current` sits ahead of the persisted CVar**, and it is not decoration: a clean logout and
    // a lost session both come back through this park, and the realm to return to is the one this
    // session is standing on. benilla writes `realmName` at world entry, so a session that never
    // reached the world has nothing persisted — without this arm, a logout mid-way would answer
    // the relist by *raising the dialog* over character select, which is not a question anybody
    // asked.
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
        // Nobody to click OK: take the first realm that will have us, in the order the list is
        // drawn in, rather than parking on a screen no one will answer.
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

/// The character screen's **Change Realm**: raise the dialog over the select screen and ask for a
/// fresh list.
///
/// The reference's order is the engine's — `CHANGE_REALM` requests the list and the arriving
/// `OPEN_REALM_LIST` shows the frame. We show it immediately against the list already in hand and
/// let the refresh land underneath, because the list *is* already in hand and a dialog that waits
/// a round trip to appear reads as a dropped click.
///
/// Nothing here touches the session. The character park serves the refresh in place and stays
/// exactly where it is, which is what lets Cancel mean nothing at all.
pub(crate) fn open_over_char_select(realms: &mut Realms, choice: &RealmChoice) {
    realms.open();
    let _ = choice.0.send(RealmRequest::Refresh);
}

/// The reference's `RealmList_OnUpdate`: count down `REALM_LIST_REFRESH_TIME` and re-request the
/// list. Only while the dialog is up — `RealmList_OnHide` calls `CancelRealmListQuery`, and a park
/// is never refreshed behind the player's back.
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

    /// The two flag bits the row's look hangs on, and the fact they are independent.
    #[test]
    fn offline_and_invalid_are_separate_bits() {
        assert!(is_down(&realm("a", 1, 0, 0x02, 0, 1.0)));
        assert!(!is_invalid(&realm("a", 1, 0, 0x02, 0, 1.0)));
        assert!(is_invalid(&realm("a", 1, 0, 0x01, 0, 1.0)));
        assert!(!is_down(&realm("a", 1, 0, 0x01, 0, 1.0)));
        assert!(
            is_down(&realm("a", 1, 0, 0x03, 0, 1.0)) && is_invalid(&realm("a", 1, 0, 0x03, 0, 1.0))
        );
        // The load sentinels live in the high bits and must not read as either.
        let sentinel = realm("a", 1, 0, 0xE0, 0, 1.0);
        assert!(!is_down(&sentinel) && !is_invalid(&sentinel));
    }

    /// A category tab scopes the **rows** — and deliberately NOT the load distribution.
    ///
    /// This is the correction that matters most here, because the wrong version looks right: the
    /// mean and std-dev a row's word is measured against come from every realm the client knows,
    /// so switching tabs re-filters the list without relabelling a single band. Scoping the stats
    /// to the tab (which is what this code did first) makes the same realm read a different word
    /// depending on which tab you are standing on.
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
        // And it really is the global mean: (1 + 1 + 400) / 3, not the visible category's.
        assert!((mean_on_tab_one - 134.0).abs() < 1e-3, "{mean_on_tab_one}");
    }

    /// A single-category list draws no tabs — the reference hides the whole strip.
    #[test]
    fn one_category_is_the_ordinary_case_and_has_no_tabs() {
        let r = list(vec![
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Beta", 1, 0, 0, 0, 1.0),
        ]);
        assert_eq!(r.categories(), vec![1]);
    }

    /// **The default order is characters → load → name → mode, all ascending** — the config the
    /// binary writes once per process, and not the name-ascending this first shipped with. A realm
    /// the account has characters on floats to the top; that is what "ascending" means for case 0.
    #[test]
    fn the_default_sort_puts_realms_you_play_on_first() {
        let mut r = list(vec![
            realm("Zeta", 1, 0, 0, 0, 1.0),
            realm("Alpha", 1, 0, 0, 0, 1.0),
            realm("Mu", 1, 0, 0, 3, 1.0),
        ]);
        assert_eq!(names(&r), ["Mu", "Alpha", "Zeta"], "Mu has characters");
        // With the counts equal the first record ties and the walk falls through to load, then to
        // the name — so the remainder is alphabetical rather than arbitrary.
        r.realms[2].characters = 0;
        assert_eq!(names(&r), ["Alpha", "Mu", "Zeta"]);
    }

    /// A column click is **move-to-front**, and the clicked column keeps the direction it already
    /// had; only re-clicking the column already in front flips it. The records behind it survive,
    /// which is what makes the fallthrough meaningful.
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

    /// The sort direction is **per record**, not one global flag: reversing the primary column must
    /// not also reverse the tie-breaks behind it.
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

    /// The highlight is a NAME, so a realm appearing above it on a refresh does not move the
    /// selection onto a different realm — the bug an index would have.
    #[test]
    fn the_selection_survives_a_realm_appearing_above_it() {
        let mut r = list(vec![realm("Mu", 1, 0, 0, 0, 1.0)]);
        r.select("Mu");
        r.realms.insert(0, realm("Alpha", 1, 0, 0, 0, 1.0));
        assert_eq!(r.selected().map(|r| r.name.as_str()), Some("Mu"));
    }

    /// A realm that vanished from the list is not selected any more, and OK is dead.
    #[test]
    fn a_realm_that_left_the_list_is_no_longer_the_selection() {
        let mut r = list(vec![realm("Mu", 1, 0, 0, 0, 1.0)]);
        r.select("Mu");
        r.realms.clear();
        assert!(r.selected().is_none());
        assert!(!r.can_enter());
    }

    /// **Raising the dialog seats the highlight on the realm you are on** — the reference's
    /// `GetRealmInfo` `currentRealm`, which `RealmListUpdate` highlights when nothing else is
    /// selected. With no current realm nothing is selected and Okay is dead, which is exactly what
    /// `RealmListUpdate` does: it opens with `RealmListOkButton:Disable()` and only the
    /// highlighted-row branches turn it back on.
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

    /// **Cancel hides the dialog and touches nothing else** — no screen change, because the
    /// reference's `RealmList_OnCancel` is `Hide()` and the screen underneath was never left.
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

    /// The policy ladder: `WOW_REALM` first, then the remembered realm, then — only when nobody
    /// is in the room — the first row. Attended and with nothing remembered, the answer is the
    /// dialog, which is the whole point of having one.
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

    /// **A logout comes back to the realm it left**, without asking. The relist arrives through the
    /// same park a first login does, and the session's own realm has to outrank the persisted CVar
    /// — which benilla only writes at world entry, so a session that never got there has nothing
    /// persisted at all and would otherwise be asked to pick again.
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

    /// A remembered realm that has left the list, or gone offline, must not strand the session: it
    /// falls through to the screen, or — unattended — to a realm that is actually up.
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

    /// An offline realm can be highlighted but not entered — the reference disables its row AND
    /// its OK button.
    #[test]
    fn an_offline_realm_cannot_be_entered() {
        let mut r = list(vec![realm("Down", 1, 0, 0x02, 0, 1.0)]);
        r.select("Down");
        assert!(r.selected().is_some());
        assert!(!r.can_enter());
    }
}
