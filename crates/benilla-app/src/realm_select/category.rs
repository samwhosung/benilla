//! The realm categories behind the list's tabs.
//!
//! The category set is the client Region's `Cfg_Categories.dbc` rows, built once per process
//! (`0x46e300`). Each list rebuild (`0x46e510`) places every realm by its wire category byte: a 0
//! byte takes the first category's id (`0x46e5b8`), then the byte joins every category whose id
//! equals it (`0x46e60b`), and a byte no category carries leaves the realm on no tab at all.
//!
//! A category the Lua names crosses as a 1-based ordinal over the non-empty categories only; the
//! map to the stored index (`0x46edf0`) has no sentinel, so an ordinal past the last one reads
//! the first stored category, empty or not.

use bevy::prelude::*;

use benilla_formats::RealmCategory;
use benilla_protocol::RealmInfo;

use super::Realms;

/// `MAX_REALM_CATEGORY_TABS` (`RealmList.lua:4`).
pub(super) const MAX_TABS: usize = 8;

/// Every category's realms, as indices into the list in wire order. Before the category set is
/// read there is one category holding every realm.
fn place(categories: Option<&[RealmCategory]>, realms: &[RealmInfo]) -> Vec<Vec<usize>> {
    let Some(categories) = categories else {
        return vec![(0..realms.len()).collect()];
    };
    let mut buckets = vec![Vec::new(); categories.len()];
    for (i, realm) in realms.iter().enumerate() {
        let byte = match realm.category {
            0 => match categories.first() {
                Some(first) => first.id,
                None => continue,
            },
            b => u32::from(b),
        };
        // No break: a realm joins every category carrying its id.
        for (bucket, category) in buckets.iter_mut().zip(categories) {
            if category.id == byte {
                bucket.push(i);
            }
        }
    }
    buckets
}

/// `0x46edf0`: the stored index of the `visible`-th non-empty category (1-based), or 0.
fn stored_index(buckets: &[Vec<usize>], visible: usize) -> usize {
    buckets
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.is_empty())
        .nth(visible.wrapping_sub(1))
        .map_or(0, |(i, _)| i)
}

impl Realms {
    /// Every category's realms; see [`place`].
    fn buckets(&self) -> Vec<Vec<usize>> {
        place(self.categories.as_deref(), &self.realms)
    }

    /// The realms of the tab in front, unsorted: `GetNumRealms` and `GetRealmInfo` with
    /// `RealmList.selectedCategory`.
    pub(super) fn category_realms(&self) -> Vec<usize> {
        let mut buckets = self.buckets();
        let at = stored_index(&buckets, self.category());
        if at < buckets.len() {
            std::mem::take(&mut buckets[at])
        } else {
            Vec::new()
        }
    }

    /// `RealmList.selectedCategory`: nil until the first `RealmListUpdate` makes it 1.
    pub(super) fn category(&self) -> usize {
        self.category.unwrap_or(1)
    }

    /// The tab labels, `GetRealmCategories` (`0x46f1f0`): the names of the categories holding a
    /// realm. `RealmList_UpdateTabs` hides the strip unless there are two or more, so the one name
    /// the reference still pushes when none hold a realm (`0x46f255`) is left out.
    pub(super) fn tabs(&self) -> Vec<&str> {
        let Some(categories) = &self.categories else {
            return Vec::new();
        };
        self.buckets()
            .iter()
            .zip(categories)
            .filter(|(b, _)| !b.is_empty())
            .map(|(_, c)| c.name.as_str())
            .collect()
    }

    /// `GetSelectedCategory` (`0x46f400`): the ordinal of the first non-empty category at or after
    /// the stored one, else 1.
    pub(super) fn selected_category(&self) -> usize {
        let from = self.chosen_category.unwrap_or(0);
        self.buckets()
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.is_empty())
            .enumerate()
            .find(|(_, (i, _))| *i >= from)
            .map_or(1, |(ordinal, _)| ordinal + 1)
    }

    /// `RealmList_OnShow`: the tab `GetSelectedCategory` names comes to the front, when a tab
    /// button carries that ordinal.
    pub(super) fn front_selected_category(&mut self) {
        let ordinal = self.selected_category();
        if ordinal <= MAX_TABS {
            self.category = Some(ordinal);
        }
    }

    /// `RealmListTab_OnClick`: the tab comes to the front and the list redraws. The redraw has no
    /// clicked name to keep (`RealmListUpdate` clears `selectedName`), so the highlight falls back
    /// to the realm this session is on, when that realm is on the new tab.
    pub(super) fn click_tab(&mut self, ordinal: usize) {
        self.category = Some(ordinal);
        self.refresh_in = super::REFRESH_SECS;
        self.highlight_current();
        self.clamp_offset();
    }

    /// `ChangeRealm` stores the front tab's category (`0x46f188`), unconditionally, before it
    /// dials; `GetSelectedCategory` reads it when the list next opens.
    pub(super) fn store_front_category(&mut self) {
        self.chosen_category = Some(stored_index(&self.buckets(), self.category()));
    }

    /// The rebuild's tail (`0x46e6f0`): the category holding the realm this session names becomes
    /// the stored one; with no such realm, or one on no tab, it is left as it was.
    pub(super) fn store_current_category(&mut self) {
        let (Some(categories), Some(want)) = (&self.categories, &self.current) else {
            return;
        };
        // The walk visits every realm, so the last one of the name decides.
        let Some(realm) = self
            .realms
            .iter()
            .rfind(|r| r.name.eq_ignore_ascii_case(want))
        else {
            return;
        };
        let byte = match realm.category {
            0 => match categories.first() {
                Some(first) => first.id,
                None => return,
            },
            b => u32::from(b),
        };
        if let Some(at) = categories.iter().position(|c| c.id == byte) {
            self.chosen_category = Some(at);
        }
    }
}

/// Read the client Region's category set once, after the archive chain opens.
pub(super) fn load_categories(
    mut realms: ResMut<Realms>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else {
        return;
    };
    let mut chain = assets.chain.lock_recover();
    let region = benilla_formats::client_region(&chain);
    match benilla_formats::load_realm_categories(&mut chain, region) {
        Ok(categories) => {
            info!(
                "realm list: Region {region}, categories {:?}",
                categories
                    .iter()
                    .map(|c| (c.id, c.name.as_str()))
                    .collect::<Vec<_>>()
            );
            if categories.is_empty() {
                warn!(
                    "realm list: Region {region} names no Cfg_Categories.dbc row, so no realm \
                     has a tab to list on (the reference's Region is Wow.ini's [WoW Config] Region)"
                );
            }
            realms.categories = Some(categories);
        }
        Err(e) => warn!(
            "realm list: Cfg_Categories.dbc failed to load, so every realm lists on one untabbed \
             page: {e:#}"
        ),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn cat(id: u32, name: &str) -> RealmCategory {
        RealmCategory {
            id,
            name: name.into(),
        }
    }

    /// Region 1's rows, `Cfg_Categories.dbc` in file order.
    pub(in crate::realm_select) fn us() -> Vec<RealmCategory> {
        vec![cat(1, "United States"), cat(5, "Oceanic")]
    }

    fn realm(name: &str, category: u8) -> RealmInfo {
        RealmInfo {
            name: name.into(),
            address: "127.0.0.1:8085".into(),
            population: 1.0,
            characters: 0,
            realm_type: 0,
            flags: 0,
            category,
            id: 0,
        }
    }

    fn realms(list: &[(&str, u8)]) -> Realms {
        Realms {
            realms: list.iter().map(|&(n, c)| realm(n, c)).collect(),
            categories: Some(us()),
            ..Realms::default()
        }
    }

    fn names(r: &Realms) -> Vec<String> {
        r.rows().iter().map(|&i| r.realms[i].name.clone()).collect()
    }

    /// The US client reads its two categories from the install's own `Cfg_Categories.dbc`.
    #[test]
    fn the_us_categories_come_from_the_install() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let loaded = benilla_formats::load_realm_categories(&mut chain, 1).expect("DBC");
        assert_eq!(loaded, us());
    }

    /// A category byte is an id, matched for equality: 1 and 5 are the two US tabs.
    #[test]
    fn a_realm_lands_on_the_tab_whose_id_is_its_category_byte() {
        let mut r = realms(&[("Alpha", 1), ("Down Under", 5), ("Beta", 1)]);
        assert_eq!(r.tabs(), ["United States", "Oceanic"]);
        assert_eq!(names(&r), ["Alpha", "Beta"]);
        r.category = Some(2);
        assert_eq!(names(&r), ["Down Under"]);
    }

    /// Byte 0 (a vmangos development-zone realm) folds into the first category.
    #[test]
    fn a_zero_byte_joins_the_first_category() {
        let r = realms(&[("Dev", 0), ("Alpha", 1)]);
        assert_eq!(r.tabs(), ["United States"], "one tab: the strip hides");
        assert_eq!(names(&r), ["Alpha", "Dev"]);
    }

    /// A byte no category of the Region carries lists nowhere.
    #[test]
    fn a_byte_outside_the_region_is_on_no_tab() {
        let r = realms(&[("Alpha", 1), ("Berlin", 2)]);
        assert_eq!(r.tabs(), ["United States"]);
        assert_eq!(names(&r), ["Alpha"]);
        assert_eq!(r.category_realms(), [0]);
    }

    /// Only a category holding a realm gets a tab, and ordinals count those alone.
    #[test]
    fn an_empty_category_has_no_tab_and_no_ordinal() {
        let mut r = realms(&[("Down Under", 5)]);
        assert_eq!(r.tabs(), ["Oceanic"], "United States holds nothing");
        assert_eq!(
            names(&r),
            ["Down Under"],
            "ordinal 1 is the Oceanic category"
        );
        // Past the last ordinal the map reads stored category 0, the empty United States.
        r.category = Some(2);
        assert!(names(&r).is_empty());
    }

    /// Before the category set is read every realm lists, untabbed.
    #[test]
    fn with_no_category_set_every_realm_lists_untabbed() {
        let mut r = realms(&[("Alpha", 1), ("Berlin", 2), ("Dev", 0)]);
        r.categories = None;
        assert!(r.tabs().is_empty());
        assert_eq!(names(&r), ["Alpha", "Berlin", "Dev"]);
    }

    /// A tab click fronts its category, and the highlight moves to the session's realm if it is
    /// there, else nowhere, so Okay cannot enter a realm from another tab.
    #[test]
    fn a_tab_click_relists_and_resets_the_highlight() {
        let mut r = realms(&[("Alpha", 1), ("Down Under", 5)]);
        r.current = Some("Alpha".into());
        r.select("Alpha");
        r.click_tab(2);
        assert_eq!(names(&r), ["Down Under"]);
        assert!(r.selected().is_none() && !r.can_enter());
        r.click_tab(1);
        assert_eq!(names(&r), ["Alpha"]);
        assert_eq!(r.selected().map(|r| r.name.as_str()), Some("Alpha"));
    }

    /// A tab with fewer realms than the scroll offset shows from its top.
    #[test]
    fn a_tab_click_clamps_the_scroll_to_the_new_tab() {
        let mut list: Vec<(String, u8)> = (0..30).map(|i| (format!("R{i:02}"), 1)).collect();
        list.push(("Down Under".into(), 5));
        let refs: Vec<(&str, u8)> = list.iter().map(|(n, c)| (n.as_str(), *c)).collect();
        let mut r = realms(&refs);
        r.offset = 12;
        r.click_tab(2);
        assert_eq!(r.offset, 0);
    }

    /// The list opens on the tab of the realm this session is on, and a realm entered from a tab
    /// brings that tab back next time.
    #[test]
    fn the_list_opens_on_the_current_realms_tab() {
        let mut r = realms(&[("Alpha", 1), ("Down Under", 5)]);
        assert_eq!(r.selected_category(), 1, "nothing stored: the first tab");
        r.current = Some("down under".into());
        r.store_current_category();
        assert_eq!(r.selected_category(), 2);
        r.front_selected_category();
        assert_eq!(names(&r), ["Down Under"]);

        let mut r = realms(&[("Alpha", 1), ("Down Under", 5)]);
        r.category = Some(2);
        r.store_front_category();
        r.category = Some(1);
        r.front_selected_category();
        assert_eq!(r.category(), 2, "ChangeRealm's stored tab");
    }

    /// A stored category that has emptied gives the next non-empty one's ordinal.
    #[test]
    fn a_stored_category_that_emptied_reads_the_next_tab_on() {
        let mut r = realms(&[("Alpha", 1), ("Down Under", 5)]);
        r.chosen_category = Some(0);
        r.realms.remove(0);
        assert_eq!(r.selected_category(), 1, "Oceanic, now the only tab");
    }
}
