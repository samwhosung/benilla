//! Selection by name: the reference's one resolver (`0x493aa0`) behind `/target`, `/assist` and
//! `/follow`, parameterised per binding.
//!
//! | binding | typemask | filter mode | exact-only |
//! |---|---|---|---|
//! | `TargetByName 0x489d60` | 8, creatures and players | 0 | Lua arg 2 |
//! | `AssistByName 0x489c40` | 0x10, players only | 0 | 0 |
//! | `FollowByName 0x489ec0` | 0x10, players only | 2, `CanAssist` and alive | Lua arg 2 |
//!
//! Filter mode 0 underflows `0x493e40`'s jump table into accept-all: no range, facing, reaction,
//! liveness, visibility or self test. A name matches whole-string first (`0x64a4c0` → CRT
//! `0x414310`), else by the longest common prefix (`0x493cb6`-`0x493cf6`), both case-insensitive.
//!
//! Deviation: the reference ends the walk at the first whole-string hit (`0x493ca2`), so its pick
//! follows the object list's order (a tail append, `0x4646c3`-`0x4646d4`); we rank exact, then
//! longer prefix, then nearest ([`Rank::beats`]), because the nearest namesake is the better pick.
//!
//! Not built: the reference's party (`0x493b20`) and raid (`0x493b9d`) pre-scans, which select an
//! unstreamed groupmate; [`Selection::target`] is an `Entity`.

use std::cmp::Ordering;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::guid;

use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, ObjectStore, SelfPlayer};

use super::{scan, Selection};

/// Which guid families a search accepts, the reference's typemask argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameSearch {
    /// Typemask 8, creatures and players (a player carries the UNIT bit): `/target`.
    AnyUnit,
    /// Typemask 0x10, players only: `/assist <name>` and `/follow <name>`.
    PlayerOnly,
}

impl NameSearch {
    fn accepts(self, guid_val: u64) -> bool {
        match self {
            Self::AnyUnit => guid::is_player(guid_val) || guid::is_creature_or_pet(guid_val),
            Self::PlayerOnly => guid::is_player(guid_val),
        }
    }
}

/// `/target <name>`: resolve and select (`TargetByName`).
#[derive(bevy::ecs::message::Message, Clone, Debug)]
pub(crate) struct TargetByNameRequest {
    pub(crate) name: String,
}

/// `/assist [name]`: select the basis unit's target (`AssistByName`, or bare
/// `AssistUnit("target")`, whose basis is the current selection).
#[derive(bevy::ecs::message::Message, Clone, Debug)]
pub(crate) struct AssistRequest {
    pub(crate) name: Option<String>,
}

/// How well one candidate answered the query, ordered by [`Rank::beats`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rank {
    /// A whole-string, case-insensitive match.
    exact: bool,
    /// The folded common-prefix length.
    prefix: usize,
    /// 3D centre-to-centre squared distance from the active player.
    dist2: f32,
}

impl Rank {
    /// Strictly better than the incumbent: exact, then the longer prefix, then the strictly nearer.
    /// A tie or a NaN distance keeps the incumbent, as the reference's `fcomp`/`jp` compare does.
    fn beats(self, other: Rank) -> bool {
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
}

/// The case-folded common-prefix length, the reference's lockstep walk (`0x493cc0`-`0x493cee`)
/// through the ASCII fold `0x41089b`.
fn common_prefix_len(query: &str, name: &str) -> usize {
    query
        .bytes()
        .zip(name.bytes())
        .take_while(|(q, n)| q.eq_ignore_ascii_case(n))
        .count()
}

/// The follow start gate (`0x60fed0`'s mode-3 chain) as a pure function, its order pinned by test:
///
/// | check | site | failure |
/// |---|---|---|
/// | followee is a player (typemask `0x10`) | `0x60ff5f` | `0x128` `ERR_INVALID_FOLLOW_TARGET` |
/// | `CanCooperate` with the followee (`0x606ba0`) | `0x60ff6a` | `0x128`, the same line |
/// | we are alive | `0x60ff7c` | `0x7e` `ERR_PLAYER_DEAD` |
/// | we are not stunned (`UNIT_FIELD_FLAGS & 0x40000`) | `0x60ff95` | `0x191` `ERR_GENERIC_STUNNED` |
/// | we are not casting | `0x60ffb9` | `0x134` `ERR_TOOBUSYTOFOLLOW` |
///
/// There is no followee-alive check (the per-tick death test ends a follow) and no distance check:
/// follow's row in `0x860a58` has a `0.0` threshold, so `ERR_AUTOFOLLOW_TOO_FAR` (`0x6110a0`)
/// never fires (`0x6110c8`).
fn follow_refusal_key(
    followee_is_player: bool,
    can_assist_followee: bool,
    we_are_dead: bool,
    we_are_stunned: bool,
    we_are_casting: bool,
) -> Option<&'static str> {
    if !followee_is_player || !can_assist_followee {
        return Some("ERR_INVALID_FOLLOW_TARGET");
    }
    if we_are_dead {
        return Some("ERR_PLAYER_DEAD");
    }
    if we_are_stunned {
        return Some("ERR_GENERIC_STUNNED");
    }
    if we_are_casting {
        return Some("ERR_TOOBUSYTOFOLLOW");
    }
    None
}

/// Whether the prefix tier is live, the resolver's exact-only argument. The unit popup's Target and
/// Follow pass 1 (`UnitPopup.lua:557`, `UnitPopup.lua:623`), so a menu naming its unit never lands
/// on a bystander sharing a first letter; the slash commands pass nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Match {
    /// A whole-string hit, else the longest common prefix.
    PrefixOk,
    /// Whole-string only.
    ExactOnly,
}

/// Score one candidate against the query. The reference's best length seeds at 1, so a zero-length
/// overlap is no match.
fn rank(query: &str, name: &str, dist2: f32, mode: Match) -> Option<Rank> {
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

/// Everything a by-name search reads.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled param, the app's convention for big query sets
pub(crate) struct ByNameScan<'w, 's> {
    /// Every known unit, our own avatar included: the reference has no self-exclusion here.
    units: Query<
        'w,
        's,
        (
            Entity,
            &'static Guid,
            &'static Transform,
            Option<&'static ObjectStore>,
        ),
    >,
    self_q: Query<
        'w,
        's,
        (
            Entity,
            &'static Guid,
            &'static Transform,
            Option<&'static ObjectStore>,
        ),
        With<SelfPlayer>,
    >,
    /// Read-only: a sweep fires no name query, so an uncached name does not match, as in the
    /// reference.
    names: Res<'w, NameCache>,
    /// The reaction inputs behind `CanAssist`, for [`Filter::AssistableAlive`] and the follow gate.
    factions: Option<Res<'w, super::Factions>>,
    reputations: Res<'w, crate::net::Reputations>,
}

/// The per-candidate filter, the mode argument the reference hands `0x493e40`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    /// Mode 0, which underflows the jump table's bound into accept-all: `/target`, `/assist`.
    AcceptAll,
    /// Mode 2, `CanAssist` and `UNIT_FIELD_HEALTH > 0`: `/follow`.
    AssistableAlive,
}

impl ByNameScan<'_, '_> {
    /// The distance origin, the active player's position; without one the reference resolves
    /// nothing (`0x493ae0`).
    fn origin(&self) -> Option<Vec3> {
        self.self_q
            .single()
            .ok()
            .map(|(_, _, tf, _)| tf.translation)
    }

    /// The mode-2 arm (`0x493eca`): alive, and `CanAssist 0x6066f0` as a friendly reaction (4+).
    fn assistable_alive(&self, store: Option<&ObjectStore>) -> bool {
        if store.is_some_and(|s| s.0.unit_is_dead()) {
            return false;
        }
        let self_store = self.self_q.single().ok().and_then(|(_, _, _, s)| s);
        super::ring_reaction(
            self.factions.as_deref(),
            &self.reputations,
            store,
            self_store,
        ) >= 4
    }

    /// The follow start gate ([`follow_refusal_key`], entered at `0x60ff59`): the key of the first
    /// refusal, for `CGGameUI::DisplayError`. `FollowUnit` skips the resolver but not this gate.
    fn follow_refusal(
        &self,
        followee: u64,
        followee_store: Option<&ObjectStore>,
        casting: bool,
    ) -> Option<&'static str> {
        let self_store = self.self_q.single().ok().and_then(|(_, _, _, s)| s);
        follow_refusal_key(
            guid::is_player(followee),
            super::ring_reaction(
                self.factions.as_deref(),
                &self.reputations,
                followee_store,
                self_store,
            ) >= 4,
            self_store.is_some_and(|s| s.0.unit_is_dead()),
            self_store.is_some_and(|s| s.0.unit_flags() & crate::player::UNIT_FLAG_STUNNED != 0),
            casting,
        )
    }

    /// Resolve a name to a unit, logging one `by-name:` line per call, ungated since it runs per
    /// command: the candidates counted, the winner and on what.
    fn resolve(
        &self,
        query: &str,
        search: NameSearch,
        filter: Filter,
        mode: Match,
    ) -> Option<(Entity, u64, String)> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let Some(origin) = self.origin() else {
            info!("by-name: \"{query}\" — no active player object; nothing resolves");
            return None;
        };
        let mut considered = 0usize;
        let mut nameless = 0usize;
        let mut best: Option<(Entity, u64, Rank, String)> = None;
        for (entity, guid, tf, store) in &self.units {
            if !search.accepts(guid.0) {
                continue;
            }
            if filter == Filter::AssistableAlive && !self.assistable_alive(store) {
                continue;
            }
            let Some(name) = self.names.peek(guid.0) else {
                nameless += 1;
                continue;
            };
            considered += 1;
            let dist2 = tf.translation.distance_squared(origin);
            let Some(r) = rank(query, name, dist2, mode) else {
                continue;
            };
            if best.as_ref().is_none_or(|(_, _, b, _)| r.beats(*b)) {
                best = Some((entity, guid.0, r, name.to_string()));
            }
        }
        match &best {
            Some((_, guid, r, name)) => info!(
                "by-name: \"{query}\" ({search:?}) -> \"{name}\" guid {guid:#x} \
                 ({}, prefix {}, {:.1} yd) over {considered} named candidates ({nameless} unnamed)",
                if r.exact { "exact" } else { "prefix" },
                r.prefix,
                r.dist2.sqrt(),
            ),
            None => info!(
                "by-name: \"{query}\" ({search:?}) -> NO MATCH over {considered} named candidates \
                 ({nameless} unnamed, {mode:?}); target left untouched"
            ),
        }
        best.map(|(e, g, _, name)| (e, g, name))
    }
}

/// Drain `/target <name>` through [`scan::commit`], the stop, select, re-swing path a click takes.
/// A miss leaves the target alone, as neither failure edge in `0x489db4` calls `SetSelection`, and
/// prints nothing; the reference prints message `0x127` `ERR_UNIT_NOT_FOUND` there, or `0xb8`
/// `ERR_GENERIC_NO_TARGET` for an empty name.
pub(super) fn target_by_name_requests(
    mut requests: MessageReader<TargetByNameRequest>,
    scan_params: ByNameScan,
    mut commit: SelectCommit,
) {
    for request in requests.read() {
        let Some((entity, guid, _)) = scan_params.resolve(
            &request.name,
            NameSearch::AnyUnit,
            Filter::AcceptAll,
            Match::PrefixOk,
        ) else {
            continue;
        };
        commit.commit(entity, guid);
    }
}

/// Drain the Lua `TargetByName(name, exactMatch)` asks, the `/target` path plus the second
/// argument, which `0x489d8e` reads with default 0 as the resolver's exact-only flag (consumed at
/// `0x493cab`).
pub(super) fn script_target_by_name_requests(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    scan_params: ByNameScan,
    mut commit: SelectCommit,
) {
    let Some(mut script) = script else {
        return;
    };
    for (name, exact) in script.take_target_by_name_requests() {
        let Some((entity, guid, _)) = scan_params.resolve(
            &name,
            NameSearch::AnyUnit,
            Filter::AcceptAll,
            if exact {
                Match::ExactOnly
            } else {
                Match::PrefixOk
            },
        ) else {
            continue;
        };
        commit.commit(entity, guid);
    }
}

/// Drain `/assist [name]`: the basis is a named player, or bare the current selection
/// (`AssistUnit("target")`, `ChatFrame.lua:744`), and [`SelectCommit::assist`] selects its target.
pub(super) fn assist_requests(
    mut requests: MessageReader<AssistRequest>,
    scan_params: ByNameScan,
    mut commit: SelectCommit,
) {
    for request in requests.read() {
        // The basis: a named player, or bare, whatever is selected.
        let basis = match &request.name {
            Some(name) => scan_params
                .resolve(
                    name,
                    NameSearch::PlayerOnly,
                    Filter::AcceptAll,
                    Match::PrefixOk,
                )
                .map(|(e, _, _)| e),
            None => commit.selection.target,
        };
        let Some(basis) = basis else {
            info!("assist (/assist): no basis unit; nothing to assist");
            continue;
        };
        // From here on this is `AssistUnit`'s tail too, one function as in the reference.
        commit.assist(basis, "/assist");
    }
}

/// Drain `/follow [name]` into [`crate::player`]; follow sends nothing on the wire. The followee's
/// name is latched here for `AUTOFOLLOW_BEGIN` ([`crate::ui_follow`]).
pub(super) fn follow_requests(
    mut requests: MessageReader<crate::player::FollowRequest>,
    scan_params: ByNameScan,
    selection: Res<Selection>,
    group: Res<crate::ui_party::GroupState>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    cast: Res<crate::spell::PendingCast>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    mut follow: ResMut<crate::player::FollowState>,
) {
    for request in requests.read() {
        let resolved = match request {
            crate::player::FollowRequest::Name { name, exact } => scan_params
                .resolve(
                    name,
                    NameSearch::PlayerOnly,
                    Filter::AssistableAlive,
                    if *exact {
                        Match::ExactOnly
                    } else {
                        Match::PrefixOk
                    },
                )
                .map(|(_, guid, name)| (guid, name)),
            // `"target"` skips `player_token_guid`, whose players-only filter would make a
            // creature a silent miss: the start gate must refuse it with its error line.
            crate::player::FollowRequest::Unit(token) => match token.as_str() {
                "target" => selection.guid,
                tok => crate::ui_unit::player_token_guid(tok, &selection, &group),
            }
            .map(|guid| {
                let name = scan_params.names.peek(guid).unwrap_or_default().to_string();
                (guid, name)
            }),
        };
        // The start gate, whichever way the subject was found.
        if let Some((guid, _)) = &resolved {
            let followee = index.0.get(guid).and_then(|e| stores.get(*e).ok());
            if let Some(key) = scan_params.follow_refusal(
                *guid,
                followee,
                cast.in_flight(std::time::Instant::now()),
            ) {
                info!("follow: refused — {key}");
                errors.0.push(crate::ui_action::UiError::key(key));
                continue;
            }
        }
        match resolved {
            Some((guid, name)) => {
                info!("follow: now following \"{name}\" guid {guid:#x}");
                follow.start(guid, name);
            }
            None => {
                // No subject by name or token: a follow in progress is left alone.
                info!("follow: nothing to follow");
            }
        }
    }
}

/// Everything [`scan::commit`], the one `SetSelection` path, needs.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled param, the app's convention for big query sets
pub(crate) struct SelectCommit<'w, 's> {
    pub(super) selection: ResMut<'w, Selection>,
    seam: crate::creature_anim::AttackSeam<'w, 's>,
    // Our own body: the guid, the store `can_attack` reads, and whether we are mid-swing.
    me: Query<
        'w,
        's,
        (
            &'static Guid,
            Option<&'static ObjectStore>,
            Has<crate::creature_anim::Engaged>,
        ),
        With<SelfPlayer>,
    >,
    pub(super) stores: Query<'w, 's, &'static ObjectStore>,
    /// The guid to entity map, `0x489a40`'s `0x468460` lookup.
    index: Res<'w, GuidIndex>,
    factions: Option<Res<'w, super::Factions>>,
    reputations: Res<'w, crate::net::Reputations>,
    /// `assistAttack`, the assist tail's second leg.
    assist_attack: Res<'w, super::AssistAttack>,
}

impl SelectCommit<'_, '_> {
    /// The shared assist tail (`0x489bb2`-`0x489c07`, byte-identical in `AssistByName` at
    /// `0x489cae`-`0x489d07`): select the basis unit's `UNIT_FIELD_TARGET` if it is streamed.
    /// Every miss is silent and deselects nothing: no basis, a basis targeting nothing, or an
    /// unstreamed guid (`0x489a40`'s arm 3 is a bare `ret`). With `assistAttack` set, the tail then
    /// opens the swing (`0x489c02`/`0x489d02` call `0x5ecb70`).
    pub(super) fn assist(&mut self, basis: Entity, how: &str) {
        let Some(guid) = self
            .stores
            .get(basis)
            .ok()
            .and_then(|s| s.0.unit_target())
            .filter(|g| *g != 0)
        else {
            info!("assist ({how}): the basis unit is targeting nothing; silent no-op");
            return;
        };
        // Only a streamed unit can be selected; the reference's roster fallback for an unstreamed
        // guid is not built (see the module doc).
        let Some(entity) = self.index.0.get(&guid).copied() else {
            info!("assist ({how}): the basis is targeting guid {guid:#x}, which is not streamed");
            return;
        };
        info!("assist ({how}) -> the basis unit's target, guid {guid:#x}");
        // `engaged` and attackability are read before the selection: `SetSelection` may re-point
        // a swing we already had (`0x4938c8`), and `0x5ecb70` then skips the send but still runs
        // its tail, a sheath snap; not yet fighting, the second leg opens a fresh swing.
        let me = self.me.single().ok();
        let engaged = me.is_some_and(|(_, _, engaged)| engaged);
        let attackable = super::relations::can_attack(
            self.stores.get(entity).ok(),
            self.factions.as_deref(),
            &self.reputations,
            me.and_then(|(_, store, _)| store),
        );
        self.commit(entity, guid);
        // The second leg. `0x5ecb70`'s attackability test is the caller's here, so a friendly
        // target is selected, not swung at.
        if self.assist_attack.0 && attackable {
            info!("assist ({how}): assistAttack is on -> opening the swing");
            self.seam.start(guid, engaged, false);
        }
    }

    /// Select a resolved guid, `0x489a40`'s arm 1, through [`scan::commit`].
    pub(super) fn commit(&mut self, entity: Entity, guid: u64) {
        let me = self.me.single().ok();
        // `scan::commit` takes the new target's attackability from its caller: the same
        // `can_attack` the cursor and TAB pass.
        let attackable = super::relations::can_attack(
            self.stores.get(entity).ok(),
            self.factions.as_deref(),
            &self.reputations,
            me.and_then(|(_, store, _)| store),
        );
        scan::commit(
            &mut self.selection,
            &mut self.seam,
            entity,
            guid,
            self.stores.get(entity).ok(),
            me.is_some_and(|(_, _, engaged)| engaged),
            me.map(|(g, _, _)| g.0),
            attackable,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(dist2: f32) -> Rank {
        Rank {
            exact: true,
            prefix: 3,
            dist2,
        }
    }
    fn prefix(prefix: usize, dist2: f32) -> Rank {
        Rank {
            exact: false,
            prefix,
            dist2,
        }
    }

    #[test]
    fn prefix_length_folds_case_and_stops_at_the_shorter_string() {
        assert_eq!(common_prefix_len("rag", "Ragnaros"), 3);
        assert_eq!(common_prefix_len("RAG", "ragnaros"), 3);
        assert_eq!(
            common_prefix_len("Ragnarosx", "Ragnaros"),
            8,
            "capped by the candidate"
        );
        assert_eq!(
            common_prefix_len("zzz", "Ragnaros"),
            0,
            "no shared first letter"
        );
    }

    #[test]
    fn a_match_needs_one_folded_character() {
        // The reference seeds its best length at 1: zero overlap is no match.
        assert!(rank("zzz", "Ragnaros", 1.0, Match::PrefixOk).is_none());
        assert_eq!(
            rank("rag", "Ragnaros", 1.0, Match::PrefixOk).map(|r| r.prefix),
            Some(3)
        );
    }

    #[test]
    fn whole_string_match_is_case_insensitive_and_ranks_exact() {
        let r = rank("kobold vermin", "Kobold Vermin", 9.0, Match::PrefixOk).expect("matches");
        assert!(r.exact, "tier 1 is case-insensitive whole-string");
    }

    /// A dead player following a creature sees `ERR_INVALID_FOLLOW_TARGET`, not `ERR_PLAYER_DEAD`:
    /// the two followee checks come first and share message `0x128`.
    #[test]
    fn the_follow_gate_refuses_in_the_references_own_order() {
        // The clean case.
        assert_eq!(follow_refusal_key(true, true, false, false, false), None);
        // A creature refuses, however it was found.
        assert_eq!(
            follow_refusal_key(false, true, false, false, false),
            Some("ERR_INVALID_FOLLOW_TARGET")
        );
        // An enemy player fails CanAssist, and shares the creature's line.
        assert_eq!(
            follow_refusal_key(true, false, false, false, false),
            Some("ERR_INVALID_FOLLOW_TARGET")
        );
        // Our own three, each shadowed by the followee checks above it.
        assert_eq!(
            follow_refusal_key(true, true, true, false, false),
            Some("ERR_PLAYER_DEAD")
        );
        assert_eq!(
            follow_refusal_key(true, true, false, true, false),
            Some("ERR_GENERIC_STUNNED")
        );
        assert_eq!(
            follow_refusal_key(true, true, false, false, true),
            Some("ERR_TOOBUSYTOFOLLOW")
        );
        // Everything wrong at once: the followee's line wins.
        assert_eq!(
            follow_refusal_key(false, false, true, true, true),
            Some("ERR_INVALID_FOLLOW_TARGET")
        );
        // Dead and stunned: dead is checked first.
        assert_eq!(
            follow_refusal_key(true, true, true, true, true),
            Some("ERR_PLAYER_DEAD")
        );
    }

    #[test]
    fn exact_only_skips_the_prefix_tier_but_keeps_the_case_fold() {
        assert!(rank("rag", "Ragnaros", 1.0, Match::ExactOnly).is_none());
        assert!(
            rank("RAGNAROS", "Ragnaros", 1.0, Match::ExactOnly)
                .expect("whole-string still matches")
                .exact
        );
    }

    #[test]
    fn exact_beats_any_prefix_and_nearest_breaks_the_tie() {
        // The deviation: exact ranks first, whatever the reference's walk order.
        assert!(exact(100.0).beats(prefix(3, 1.0)));
        assert!(!prefix(3, 1.0).beats(exact(100.0)));
        // Among same-named exact matches, the nearest wins.
        assert!(exact(4.0).beats(exact(9.0)));
        assert!(!exact(9.0).beats(exact(4.0)));
    }

    #[test]
    fn longer_prefix_beats_nearer_shorter_one() {
        // The reference's `jg` accept: a longer prefix wins at any distance.
        assert!(prefix(5, 900.0).beats(prefix(3, 1.0)));
        assert!(!prefix(3, 1.0).beats(prefix(5, 900.0)));
        // Equal prefix length falls through to strictly-nearest.
        assert!(prefix(3, 4.0).beats(prefix(3, 9.0)));
    }

    #[test]
    fn ties_keep_the_incumbent_and_nan_never_displaces() {
        // The reference's `jp` reject edge makes the accept a strict `<`.
        assert!(!prefix(3, 4.0).beats(prefix(3, 4.0)));
        assert!(!exact(4.0).beats(exact(4.0)));
        assert!(!prefix(3, f32::NAN).beats(prefix(3, 4.0)));
    }
}
