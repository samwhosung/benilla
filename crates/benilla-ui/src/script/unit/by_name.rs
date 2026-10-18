//! The by-name resolver (`CGGameUI::ClosestObjectMatch 0x493aa0`), shared by every caller: the app's
//! `/target`, `/assist` and `/follow` scans, `TargetByName`, which shows the pick to the rest of its
//! chunk (in the reference the binding `0x489d60` commits through `SetSelection 0x493540` before it
//! returns, so `UnitName("target")` on the next line already names the new unit), and
//! `SetLootMethod`'s master name (`0x4e9418`). A name matches whole-string first (`0x64a4c0` → CRT
//! `0x414310`), else by the longest common prefix (`0x493cb6`-`0x493cf6`), both case-insensitive.
//!
//! [`resolve`] runs the reference's three passes over the one matcher `0x493c60`: the four party
//! slots (`0x493b22`), then the raid roster (`0x493ba7`), each member offered only when no object is
//! held for it (`0x468460` NULL, `0x493b4d`, `0x493be5`), at the placeholder distance
//! `0x4f1502f9` (`0x493b74`, `0x493c0c`); then every held object of the typemask that the mode's
//! filter `0x493e40` passes, at its squared distance (`0x468380` with `0x493d30`). No pass is gated
//! on the typemask. The filter mode gates the roster instead: mode 1 skips both passes (`0x493b1a`)
//! and mode 3 only the raid pass (`0x493b93`); no caller passes either, so neither is modelled.
//! Without an active player object the whole resolver returns nothing (`0x493ae0`), roster passes
//! included ([`Search::active_player`]).
//!
//! Deviation: the reference's matcher takes a whole-string hit at once only when it is nearer than
//! the best so far (`0x493c88`-`0x493ca2`); an exact hit that is not nearer competes on prefix
//! length alone (`0x493cf8`-`0x493d09`), so a nearer "Bobby" walked first beats an exact "Bob", and
//! the pick follows the object list's order. We rank exact, then longer prefix, then nearest
//! ([`Rank::beats`]), because the namesake is the better pick, and a full tie between held objects
//! goes to the lower guid ([`Rank::beats_at`]), so the VM's walk and the app's scan pick the same
//! unit whatever order each walks in. The roster passes keep the reference's order: of equal ranks
//! the first member offered stays.

use std::cmp::Ordering;

use super::{Model, UnitState};

/// How well one candidate answered the query, ordered by [`Rank::beats`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rank {
    /// A whole-string, case-insensitive match.
    pub exact: bool,
    /// The folded common-prefix length.
    pub prefix: usize,
    /// 3D centre-to-centre squared distance from the active player.
    pub dist2: f32,
}

impl Rank {
    /// Strictly better than the incumbent: exact, then the longer prefix, then the strictly nearer.
    /// A tie or a NaN distance keeps the incumbent, as the reference's `fcomp`/`jp` compare does.
    pub fn beats(self, other: Rank) -> bool {
        match (self.exact, other.exact) {
            (true, false) => true,
            (false, true) => false,
            _ => match self.prefix.cmp(&other.prefix) {
                Ordering::Greater => true,
                Ordering::Less => false,
                Ordering::Equal => self.dist2 < other.dist2,
            },
        }
    }

    /// [`Rank::beats`] with the guid as the last key: of two equal ranks the lower guid wins. A
    /// NaN distance is equal to nothing, so it still keeps the incumbent.
    ///
    /// Deviation: a full tie goes to the lower guid, where the reference keeps the first unit its
    /// walk (`0x493aa0`) reaches, an order neither of ours has; a guid keeps the two picks equal.
    pub fn beats_at(self, guid: u64, other: Rank, other_guid: u64) -> bool {
        self.beats(other) || (self == other && guid < other_guid)
    }
}

/// The case-folded common-prefix length, the reference's lockstep walk (`0x493cc0`-`0x493cee`)
/// through the ASCII fold `0x41089b`.
pub fn common_prefix_len(query: &str, name: &str) -> usize {
    query
        .bytes()
        .zip(name.bytes())
        .take_while(|(q, n)| q.eq_ignore_ascii_case(n))
        .count()
}

/// Whether the prefix tier is live, the resolver's exact-only argument. The unit popup's Target and
/// Follow pass 1 (`UnitPopup.lua:557`, `UnitPopup.lua:623`), so a menu naming its unit never lands
/// on a bystander sharing a first letter; the slash commands pass nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Match {
    /// A whole-string hit, else the longest common prefix.
    PrefixOk,
    /// Whole-string only.
    ExactOnly,
}

/// Score one candidate against the query. The reference's best length seeds at 1, so a zero-length
/// overlap is no match.
pub fn rank(query: &str, name: &str, dist2: f32, mode: Match) -> Option<Rank> {
    if query.eq_ignore_ascii_case(name) {
        return Some(Rank {
            exact: true,
            prefix: query.len(),
            dist2,
        });
    }
    if mode == Match::ExactOnly {
        return None;
    }
    let prefix = common_prefix_len(query, name);
    (prefix >= 1).then_some(Rank {
        exact: false,
        prefix,
        dist2,
    })
}

/// The distance the roster passes hand the matcher for a member with no object (`0x4f1502f9`,
/// pushed at `0x493b74` and `0x493c0c`), so any held object of an equal prefix is nearer.
pub const ROSTER_DISTANCE: f32 = f32::from_bits(0x4f15_02f9);

/// Which held objects a search admits, the typemask argument (`edx` of `0x493aa0`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kinds {
    /// Typemask 8, creatures, pets and players (a player carries the UNIT bit): `TargetByName`.
    AnyUnit,
    /// Typemask 0x10, players only: `AssistByName`, `FollowByName`, `SetLootMethod`.
    PlayerOnly,
}

/// The per-object filter, the mode argument `0x493d30` hands `0x493e40`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    /// Mode 0, which underflows the jump table's bound into accept-all: no range, facing, reaction,
    /// liveness, visibility or self test.
    AcceptAll,
    /// Mode 2 (`0x493eca`): `CanAssist 0x6066f0` and `UNIT_FIELD_HEALTH > 0`.
    AssistableAlive,
}

/// One by-name search: the arguments of `0x493aa0`.
#[derive(Clone, Copy, Debug)]
pub struct Search<'q> {
    /// The name as typed; the callers trim, the resolver does not.
    pub query: &'q str,
    pub kinds: Kinds,
    pub filter: Filter,
    /// The exact-only flag.
    pub exact: Match,
    /// An object is held for the active player; `0x493aa0` returns nothing without one (`0x493ae0`).
    pub active_player: bool,
}

/// A roster member as passes 1 and 2 see it.
#[derive(Clone, Copy, Debug)]
pub struct RosterEntry<'a> {
    /// The name cache's answer for the member's guid; empty when it has none, which the matcher
    /// skips (`0x493c68`).
    pub name: &'a str,
    pub guid: u64,
    /// The object manager holds an object for the guid, which the roster passes then leave to
    /// the object pass (`0x493b4d`, `0x493be5`).
    pub held: bool,
}

/// A held object as the object pass sees it; `id` is the caller's handle for it.
#[derive(Clone, Copy, Debug)]
pub struct Held<'a, T> {
    pub id: T,
    pub name: &'a str,
    pub guid: u64,
    /// Squared distance from the active player.
    pub dist2: f32,
    /// The guid is a player's, for [`Kinds::PlayerOnly`].
    pub is_player: bool,
    /// [`Filter::AssistableAlive`]'s test; [`Filter::AcceptAll`] never reads it.
    pub assistable_alive: bool,
}

/// What [`resolve`] picked.
#[derive(Clone, Copy, Debug)]
pub struct Pick<'a, T> {
    pub guid: u64,
    pub name: &'a str,
    pub rank: Rank,
    /// The held object's handle; `None` for a roster member with no object.
    pub object: Option<T>,
}

/// `CGGameUI::ClosestObjectMatch` (`0x493aa0`): the best guid for the search over the party
/// slots, the raid roster and the held objects, in that order, or `None`.
///
/// The roster passes run the matcher `0x493c60` at one distance, so there it is the longest
/// prefix with the first member offered keeping the place on a tie, and a whole-string hit ends the
/// search at once only while nothing has been picked (`0x493c88` wants a distance below the best,
/// which a roster pick has already lowered to [`ROSTER_DISTANCE`]); a later whole-string hit
/// competes by its length like any prefix. A roster pick is therefore never ranked `exact`. The
/// object pass ranks by [`Rank::beats_at`].
pub fn resolve<'a, T>(
    search: Search<'_>,
    party: impl IntoIterator<Item = RosterEntry<'a>>,
    raid: impl IntoIterator<Item = RosterEntry<'a>>,
    held: impl IntoIterator<Item = Held<'a, T>>,
) -> Option<Pick<'a, T>> {
    if !search.active_player {
        return None;
    }
    let mut best: Option<Pick<'a, T>> = None;
    for member in party.into_iter().chain(raid) {
        if member.held || member.guid == 0 || member.name.is_empty() {
            continue;
        }
        let Some(r) = rank(search.query, member.name, ROSTER_DISTANCE, search.exact) else {
            continue;
        };
        let pick = Pick {
            guid: member.guid,
            name: member.name,
            rank: Rank { exact: false, ..r },
            object: None,
        };
        if r.exact && best.is_none() {
            return Some(pick);
        }
        if best.as_ref().is_none_or(|b| pick.rank.beats(b.rank)) {
            best = Some(pick);
        }
    }
    for h in held {
        let admitted = match search.kinds {
            Kinds::AnyUnit => true,
            Kinds::PlayerOnly => h.is_player,
        } && match search.filter {
            Filter::AcceptAll => true,
            Filter::AssistableAlive => h.assistable_alive,
        };
        if !admitted {
            continue;
        }
        let Some(r) = rank(search.query, h.name, h.dist2, search.exact) else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|b| r.beats_at(h.guid, b.rank, b.guid))
        {
            best = Some(Pick {
                guid: h.guid,
                name: h.name,
                rank: r,
                object: Some(h.id),
            });
        }
    }
    best
}

/// One unit `TargetByName` can pick (typemask 8: a creature, pet or player whose name is cached),
/// as the app's by-name scan sees it: the name it ranks, its distance, and the snapshot the
/// `"target"` token reads once it is picked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NameCandidate {
    /// The cached name the scan compares.
    pub name: String,
    /// Squared distance from the active player this frame.
    pub dist2: f32,
    /// The unit as a token naming it reads it.
    pub state: UnitState,
    /// `IsSelectable`, which `SetSelection` asks before it selects (`0x4935ee`): a pick that is not
    /// selectable leaves the selection and so the `"target"` view as it was.
    pub selectable: bool,
    /// [`Filter::AssistableAlive`]'s test (`0x493eca`): `CanAssist` toward the active player and
    /// `UNIT_FIELD_HEALTH > 0`.
    pub assistable_alive: bool,
}

impl super::super::UiScript {
    /// Push, or clear with `None`, a by-name candidate. Without an active player object the app
    /// pushes none, as the reference resolves nothing then (`0x493ae0`).
    pub fn set_name_candidate(&mut self, guid: u64, candidate: Option<NameCandidate>) {
        let mut model = self.model_mut();
        match candidate {
            Some(c) => {
                model.name_candidates.insert(guid, c);
            }
            None => {
                model.name_candidates.remove(&guid);
            }
        }
    }

    /// Move a pushed candidate's distance, which changes every frame its snapshot does not.
    pub fn set_name_candidate_dist2(&mut self, guid: u64, dist2: f32) {
        if let Some(c) = self.model_mut().name_candidates.get_mut(&guid) {
            c.dist2 = dist2;
        }
    }

    /// Whether a selection call moved the `"target"` view since the last ask; the app's unit feed
    /// then re-pushes `"target"` from the committed selection.
    pub fn take_target_view_moved(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().target_view_moved)
    }
}

impl Model {
    /// The candidate `TargetByName(name, exactMatch)` ranks first, the scan's walk over this
    /// frame's candidates: the query trimmed, an empty one nothing. Selectable or not, as the
    /// resolver ranks before `SetSelection` refuses (`0x4935ee`).
    pub(crate) fn named_unit(&self, query: &str, exact: bool) -> Option<&NameCandidate> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let search = Search {
            query,
            kinds: Kinds::AnyUnit,
            filter: Filter::AcceptAll,
            exact: if exact {
                Match::ExactOnly
            } else {
                Match::PrefixOk
            },
            // The app pushes candidates only while it holds the active player.
            active_player: true,
        };
        // No roster: the app selects a held unit only (`Selection::target` is an entity).
        resolve(
            search,
            std::iter::empty(),
            std::iter::empty(),
            self.held_candidates(),
        )
        .and_then(|pick| pick.object)
    }

    /// The pushed candidates as the resolver's object pass sees them.
    pub(crate) fn held_candidates(&self) -> impl Iterator<Item = Held<'_, &NameCandidate>> {
        self.name_candidates.iter().map(|(&guid, c)| Held {
            id: c,
            name: &c.name,
            guid,
            dist2: c.dist2,
            is_player: c.state.is_player,
            assistable_alive: c.assistable_alive,
        })
    }

    /// The guid `0x493aa0` resolves for a player's name typed to a binding (typemask 0x10, no
    /// exact-only flag, `filter` the binding's mode): the party slots, then the raid roster, each
    /// member offered while no object is held for it, then the pushed candidates. Our own row is
    /// held, as the active player object always is, and is offered by the object pass at distance 0.
    pub(crate) fn closest_player(&self, name: &str, filter: Filter) -> Option<u64> {
        let party = &self.party;
        let held = |token: String| self.unit(&token).is_some_and(|u| u.has_object);
        let members = party
            .members
            .iter()
            .take(4)
            .enumerate()
            .map(|(i, m)| RosterEntry {
                name: &m.name,
                guid: m.guid,
                held: held(format!("party{}", i + 1)),
            });
        let raid = party.raid.iter().enumerate().map(|(i, m)| RosterEntry {
            name: &m.name,
            guid: m.guid,
            held: (party.own_guid != 0 && m.guid == party.own_guid)
                || held(format!("raid{}", i + 1)),
        });
        let search = Search {
            query: name,
            kinds: Kinds::PlayerOnly,
            filter,
            exact: Match::PrefixOk,
            active_player: held("player".to_string()),
        };
        resolve(search, members, raid, self.held_candidates()).map(|pick| pick.guid)
    }

    /// What `TargetUnit(token)` shows as `"target"`: the unit the token names when the object
    /// manager holds it, the app's own commit condition, and `SetSelection` accepts it
    /// (`0x4935ee`); its by-name snapshot when it has one, else the token's own push, whose
    /// selectability the VM cannot see.
    pub(crate) fn held_unit_view(&self, token: &str) -> Option<UnitState> {
        let guid = self.unit_guids.resolve(token).ok().flatten();
        match guid.and_then(|g| self.name_candidates.get(&g)) {
            Some(c) => c.selectable.then(|| c.state.clone()),
            None => self.unit(token).filter(|u| u.has_object).cloned(),
        }
    }

    /// Whether a selection call made now will not move the selection when the app applies it, as
    /// far as the VM can know: `SelectTarget` returns first for a charmed player (`0x49359c`), and
    /// binds the unit to the cast in the selection's place while the targeting cursor is up
    /// (`0x4935d5`). Both are the frame's last push. A cursor an earlier call of the same chunk arms
    /// (`CastSpellByName("Heal")` with nothing selected) is the app's to decide when it applies
    /// that call, so it is not seen here: the view is eager for the rest of such a chunk until the
    /// next frame's feed corrects it.
    pub(crate) fn selection_call_declined(&self) -> bool {
        self.spell_targeting || self.unit("player").is_some_and(|u| u.charmed)
    }

    /// Show a selection call's outcome to the rest of its chunk: the resolver's target guid and the
    /// `"target"` snapshot, `None` for nothing selected. The call itself stays queued, and the app
    /// commits it (`CMSG_SET_SELECTION`, the attack re-point, the loot close) when its turn comes;
    /// the next frame's feed replaces this view with the committed selection's.
    pub(crate) fn show_selection(&mut self, state: Option<UnitState>) {
        // Already shown: the feed's own push, which carries the guild leg, stays.
        if state.as_ref().is_some_and(|s| {
            s.guid != 0 && s.guid == self.unit_guids.target && self.pushed_unit("target").is_some()
        }) {
            return;
        }
        self.unit_guids.target = state.as_ref().map_or(0, |s| s.guid);
        match state {
            Some(s) => {
                self.units_by_lower.insert("target".to_string(), s);
            }
            None => {
                self.units_by_lower.remove("target");
            }
        }
        self.target_view_moved = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster(name: &str, guid: u64, held: bool) -> RosterEntry<'_> {
        RosterEntry { name, guid, held }
    }

    fn held(name: &str, guid: u64, dist2: f32) -> Held<'_, u64> {
        Held {
            id: guid,
            name,
            guid,
            dist2,
            is_player: true,
            assistable_alive: true,
        }
    }

    fn search(query: &str) -> Search<'_> {
        Search {
            query,
            kinds: Kinds::PlayerOnly,
            filter: Filter::AcceptAll,
            exact: Match::PrefixOk,
            active_player: true,
        }
    }

    fn guid_of<'a>(
        search: Search<'_>,
        party: Vec<RosterEntry<'a>>,
        raid: Vec<RosterEntry<'a>>,
        held: Vec<Held<'a, u64>>,
    ) -> Option<u64> {
        resolve(search, party, raid, held).map(|p| p.guid)
    }

    /// `0x493b4d` and `0x493be5` offer a member only when no object is held, and a name the cache
    /// does not have, or a zero guid, is no candidate (`0x493c68`).
    #[test]
    fn the_roster_passes_offer_only_members_with_no_object() {
        let party = vec![
            roster("Bob", 1, true),
            roster("", 2, false),
            roster("Bobby", 0, false),
        ];
        assert_eq!(guid_of(search("Bo"), party, vec![], vec![]), None);
        let party = vec![roster("Bob", 1, false)];
        assert_eq!(guid_of(search("Bo"), party, vec![], vec![]), Some(1));
    }

    /// The matcher keeps the first of equal prefixes (`0x493cf8`, a strict distance compare), the
    /// party slots before the raid; a whole-string hit ends the search only while nothing is
    /// picked, and later competes by its length (`0x493c88` against the lowered best).
    #[test]
    fn the_roster_passes_keep_the_first_of_equal_prefixes() {
        let both = |first: &'static str, second: &'static str| {
            guid_of(
                search("Bob"),
                vec![roster(first, 1, false)],
                vec![roster(second, 2, false)],
                vec![],
            )
        };
        assert_eq!(both("Bobby", "Bobbo"), Some(1), "party before raid");
        assert_eq!(
            both("Bobby", "Bob"),
            Some(1),
            "an exact name after a prefix of its length"
        );
        assert_eq!(both("Bo", "Bob"), Some(2), "a longer prefix");
        assert_eq!(
            both("Bob", "Bobby"),
            Some(1),
            "an exact name first ends the search"
        );
    }

    /// The object pass runs after the roster passes at real distances, below the placeholder: an
    /// equal prefix goes to the object, an exact name to the object over a roster prefix, and the
    /// roster's exact name, first, ends the search before any object.
    #[test]
    fn the_object_pass_follows_the_roster_passes() {
        let r = |name| vec![roster(name, 1, false)];
        assert_eq!(
            guid_of(search("Bo"), r("Bob"), vec![], vec![held("Bobby", 9, 1e9)]),
            Some(9)
        );
        assert_eq!(
            guid_of(search("Bob"), r("Bobby"), vec![], vec![held("Bob", 9, 1e9)]),
            Some(9)
        );
        assert_eq!(
            guid_of(search("Bob"), r("Bob"), vec![], vec![held("Bob", 9, 1.0)]),
            Some(1)
        );
        assert_eq!(
            guid_of(
                search("Bobb"),
                r("Bobby"),
                vec![],
                vec![held("Bobx", 9, 1.0)]
            ),
            Some(1),
            "a longer prefix wins at the placeholder distance"
        );
    }

    /// The typemask and the mode's filter bind the object pass alone; the roster passes ignore both.
    #[test]
    fn typemask_and_filter_bind_the_object_pass_only() {
        let mut creature = held("Bobcat", 7, 1.0);
        creature.is_player = false;
        let mut dead = held("Bobby", 8, 1.0);
        dead.assistable_alive = false;
        let objects = || vec![creature, dead];
        let mode2 = Search {
            filter: Filter::AssistableAlive,
            ..search("Bob")
        };
        assert_eq!(guid_of(mode2, vec![], vec![], objects()), None);
        let any = Search {
            kinds: Kinds::AnyUnit,
            ..search("Bobc")
        };
        assert_eq!(guid_of(any, vec![], vec![], objects()), Some(7));
        assert_eq!(guid_of(search("Bob"), vec![], vec![], objects()), Some(8));
        // An unheld dead groupmate is still offered: the client cannot know.
        assert_eq!(
            guid_of(mode2, vec![roster("Bobby", 3, false)], vec![], objects()),
            Some(3)
        );
    }

    /// Exact-only applies to every pass (`0x493cab`), and between held namesakes the nearer wins,
    /// a full tie going to the lower guid.
    #[test]
    fn exact_only_and_the_object_tiebreaks() {
        let exact = Search {
            exact: Match::ExactOnly,
            ..search("Bob")
        };
        assert_eq!(
            guid_of(
                exact,
                vec![roster("Bobby", 1, false)],
                vec![],
                vec![held("Bobbo", 2, 1.0)]
            ),
            None
        );
        let near_far = vec![held("Bob", 5, 9.0), held("Bob", 4, 4.0)];
        assert_eq!(guid_of(search("Bob"), vec![], vec![], near_far), Some(4));
        let tie = vec![held("Bob", 5, 4.0), held("Bob", 4, 4.0)];
        assert_eq!(guid_of(search("Bob"), vec![], vec![], tie), Some(4));
    }

    /// `0x493ae0`: without an active player object the resolver returns nothing, the roster passes
    /// included.
    #[test]
    fn no_active_player_resolves_nothing() {
        let none = Search {
            active_player: false,
            ..search("Bob")
        };
        let party = || vec![roster("Bob", 1, false)];
        assert_eq!(
            guid_of(none, party(), vec![], vec![held("Bob", 2, 1.0)]),
            None
        );
        assert_eq!(guid_of(search("Bob"), party(), vec![], vec![]), Some(1));
    }
}
