//! The by-name resolver's ranking (`0x493aa0`), shared by the app's scan, which commits the pick,
//! and `TargetByName`, which shows it to the rest of its chunk: in the reference the binding
//! (`0x489d60`) commits through `SetSelection 0x493540` before it returns, so `UnitName("target")`
//! on the next line already names the new unit. A name matches whole-string first (`0x64a4c0` → CRT
//! `0x414310`), else by the longest common prefix (`0x493cb6`-`0x493cf6`), both case-insensitive.
//!
//! Deviation: the reference ends the walk at the first whole-string hit (`0x493ca2`), so its pick
//! follows the object list's order; we rank exact, then longer prefix, then nearest
//! ([`Rank::beats`]), because the nearest namesake is the better pick, and a full tie goes to the
//! lower guid ([`Rank::beats_at`]), so the VM's walk and the app's scan pick the same unit whatever
//! order each walks in.

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
        let mode = if exact {
            Match::ExactOnly
        } else {
            Match::PrefixOk
        };
        let mut best: Option<(Rank, u64, &NameCandidate)> = None;
        for (&guid, c) in &self.name_candidates {
            let Some(r) = rank(query, &c.name, c.dist2, mode) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|(b, g, _)| r.beats_at(guid, *b, *g))
            {
                best = Some((r, guid, c));
            }
        }
        best.map(|(_, _, c)| c)
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
