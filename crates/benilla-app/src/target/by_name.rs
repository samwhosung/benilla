//! Selection by name: the reference's one resolver (`0x493aa0`) behind `/target`, `/assist` and
//! `/follow`, parameterised per binding.
//!
//! | binding | typemask | filter mode | exact-only |
//! |---|---|---|---|
//! | `TargetByName 0x489d60` | 8, creatures and players | 0 | Lua arg 2 |
//! | `AssistByName 0x489c40` | 0x10, players only | 0 | 0 |
//! | `FollowByName 0x489ec0` | 0x10, players only | 2, `CanAssist` and alive | Lua arg 2 |
//!
//! The ranking is [`benilla_ui::script::by_name::resolve`], the one resolver `SetLootMethod`'s master
//! name and `TargetByName`'s view of the pick share; this module feeds it the held units, positioned
//! by [`GlobalTransform`] as [`crate::ui_unit`]'s candidate feed does, and breaks a full tie by
//! guid, so the chunk shows the unit the commit picks. The reference's walk follows the object
//! list's order (a tail append, `0x4646c3`-`0x4646d4`), the deviation documented there.
//!
//! Not built for these selecting callers: the roster passes (party `0x493b20`, raid `0x493b9d`),
//! because the selection holds an entity, not a guid, and `0x493540` stores a guid with no object
//! (`0x493605`). `DoEmote`'s [`PlayerLookup`] needs only a guid and runs them.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::guid;
use benilla_ui::script::by_name::{resolve, Filter, Held, Kinds, Match, RosterEntry, Search};

use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;

use super::{scan, Selection};

/// Which guid families a search accepts, the reference's typemask argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameSearch {
    /// Typemask 8, creatures and players (a player carries the UNIT bit): `/target`.
    AnyUnit,
    /// Typemask 0x10, players only: `/assist <name>` and `/follow <name>`.
    PlayerOnly,
}

/// What a by-name search makes of one unit, [`NameSearch::candidacy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Candidacy<'a> {
    /// Its guid family is outside the search's typemask.
    Outside,
    /// In the typemask, but its name is not cached; a sweep sends no query, so it cannot match.
    Nameless,
    /// A candidate under its cached name.
    Named(&'a str),
}

impl NameSearch {
    /// The resolver's typemask argument.
    fn kinds(self) -> Kinds {
        match self {
            Self::AnyUnit => Kinds::AnyUnit,
            Self::PlayerOnly => Kinds::PlayerOnly,
        }
    }

    fn accepts(self, guid_val: u64) -> bool {
        match self {
            Self::AnyUnit => guid::is_player(guid_val) || guid::is_creature_or_pet(guid_val),
            Self::PlayerOnly => guid::is_player(guid_val),
        }
    }

    /// Whether `guid_val` is a candidate of this search: the typemask, then a name already in the
    /// cache. The one test both [`scan_units`] and the VM's candidate feed
    /// (`ui_unit::held::feed_name_candidates`) apply, so the chunk's candidates are the commit's.
    pub(crate) fn candidacy<'a>(self, names: &'a NameCache, guid_val: u64) -> Candidacy<'a> {
        if !self.accepts(guid_val) {
            return Candidacy::Outside;
        }
        names
            .resolve_unit(guid_val, None, Ask::Read)
            .map_or(Candidacy::Nameless, Candidacy::Named)
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

/// The follow start gate's refusals with lines (`0x60fed0`'s mode-3 chain, after its silent arms)
/// as a pure function, its order pinned by test; "we" are the active mover:
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
    we_cooperate: bool,
    we_are_dead: bool,
    we_are_stunned: bool,
    we_are_casting: bool,
) -> Option<&'static str> {
    if !followee_is_player || !we_cooperate {
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

/// Why a follow request starts nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FollowRefusal {
    /// The error line `CGGameUI::DisplayError` shows, by its GlobalStrings key.
    Line(&'static str),
    /// The start gate's silent return (`0x60ff8a`).
    Silent,
}

/// Everything a by-name search reads.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled param, the app's convention for big query sets
pub(crate) struct ByNameScan<'w, 's> {
    /// Every known unit, our own avatar included: the reference has no self-exclusion here.
    units: UnitQuery<'w, 's>,
    self_q: Query<
        'w,
        's,
        (
            Entity,
            &'static Guid,
            &'static GlobalTransform,
            Option<&'static ObjectStore>,
        ),
        With<SelfPlayer>,
    >,
    /// Read-only: a sweep fires no name query, so an uncached name does not match, as in the
    /// reference.
    names: Res<'w, NameCache>,
    /// The relation inputs behind `CanAssist`, for [`Filter::AssistableAlive`], and
    /// `CanCooperate`, for the follow gate.
    relations: super::RelationWorld<'w, 's>,
}

impl ByNameScan<'_, '_> {
    /// The distance origin, the active player's position; without one the reference resolves
    /// nothing (`0x493ae0`).
    fn origin(&self) -> Option<Vec3> {
        self.self_q
            .single()
            .ok()
            .map(|(_, _, tf, _)| tf.translation())
    }

    /// [`assistable_alive`] for a unit the scan walks.
    fn assistable_alive(&self, guid: u64, store: Option<&ObjectStore>) -> bool {
        let unit = store.map(|s| (self.relations.unit_of(guid, s), s));
        assistable_alive(
            &self.relations.get(),
            self.relations.me().as_ref(),
            unit.as_ref().map(|(u, s)| (u, *s)),
        )
    }

    /// Why a follow of `followee` by `mover`, each a guid and its unit descriptor, starts nothing,
    /// from the entry points' own lookups to the start gate `0x60fed0`'s mode-3 chain; `None`
    /// starts it. No followee object shows `missing`, the entry point's line (`0x489e8f` for
    /// `FollowUnit`; `0x489fa5`, else `0x489fbd`, for `FollowByName`); no active mover, which
    /// lost control zeroes (`0x5fa600`), shows `ERR_GENERIC_NO_TARGET` from both (`0x489e61`,
    /// `0x489f74`). The gate then returns silently when the followee is the mover itself
    /// (`0x60feeb`-`0x60fef1`), before [`follow_refusal_key`], whose `this` is the mover too.
    ///
    /// The gate's two other silent arms, a `this` that is not the active mover (`0x60fef7`) and a
    /// followee with no object (`0x60ff25`), never fire from these callers: both found those
    /// objects first.
    fn follow_refusal(
        &self,
        mover: Option<(u64, &ObjectStore)>,
        followee: Option<(u64, &ObjectStore)>,
        missing: &'static str,
        casting: bool,
    ) -> Option<FollowRefusal> {
        let Some((guid, store)) = followee else {
            return Some(FollowRefusal::Line(missing));
        };
        let Some((mover_guid, mover_store)) = mover else {
            return Some(FollowRefusal::Line("ERR_GENERIC_NO_TARGET"));
        };
        if guid == mover_guid {
            return Some(FollowRefusal::Silent);
        }
        let (mover, unit) = (
            self.relations.unit_of(mover_guid, mover_store),
            self.relations.unit_of(guid, store),
        );
        // `CanCooperate(this = mover, followee)` (`0x60ff6a`): the faction group, so a duel or
        // free-for-all opponent of our side is followed and a charmed player is not.
        follow_refusal_key(
            unit.is_player,
            self.relations.get().can_cooperate(&mover, &unit),
            mover_store.0.unit_is_dead(),
            mover_store.0.unit_flags() & crate::player::UNIT_FLAG_STUNNED != 0,
            casting,
        )
        .map(FollowRefusal::Line)
    }

    /// Resolve a name to a unit through [`scan_units`], the mode argument deciding the filter.
    fn resolve(
        &self,
        query: &str,
        search: NameSearch,
        filter: Filter,
        mode: Match,
    ) -> Option<(Entity, u64, String)> {
        scan_units(
            self.units
                .iter()
                .map(|(e, g, tf, store)| (e, g.0, tf.translation(), store)),
            self.origin(),
            &self.names,
            query,
            search,
            filter,
            mode,
            Roster::default(),
            |guid, store| self.assistable_alive(guid, store),
        )
        // With no roster every pick is a held unit.
        .and_then(|(e, g, n)| Some((e?, g, n)))
    }
}

/// The mode-2 filter (`0x493eca`): `CanAssist 0x6066f0` from the active player and
/// `UNIT_FIELD_HEALTH > 0` (`[unit+0x110]+0x40`), over the unit and its descriptor. The follow
/// search and the VM's by-name candidates share it; no player or no descriptor refuses.
pub(crate) fn assistable_alive(
    rel: &super::Relations,
    me: Option<&super::RelUnit>,
    unit: Option<(&super::RelUnit, &ObjectStore)>,
) -> bool {
    unit.zip(me).is_some_and(|((unit, store), me)| {
        rel.can_assist(me, unit) && store.0.unit_health().unwrap_or(0) > 0
    })
}

/// One unit query's candidates, positioned by [`GlobalTransform`] as the VM's candidate feed
/// (`ui_unit::held::feed_name_candidates`) does: nothing writes it during `Update`, so both walks
/// read one value in a frame, and it adds no undeclared order against the systems that move units.
type UnitQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Guid,
        &'static GlobalTransform,
        Option<&'static ObjectStore>,
    ),
>;

/// The roster passes' input (`0x493b22`, `0x493ba7`): the party slots and the raid members, by the
/// name the wire gave and whether the object manager holds them. Empty for the callers that
/// select ([`Selection::target`] is an entity); a pick from it has a guid and no entity.
#[derive(Default)]
pub(super) struct Roster<'a> {
    party: Vec<RosterEntry<'a>>,
    raid: Vec<RosterEntry<'a>>,
}

/// The resolver's walk (`0x493aa0`): a name to a unit, logging one `by-name:` line per call,
/// ungated since it runs per command: the candidates counted, the winner and on what. `filter` is
/// the mode argument and `assistable` its mode-2 test; `origin` is the active player's position,
/// without which the reference resolves nothing (`0x493ae0`).
#[allow(clippy::too_many_arguments)] // the resolver's own arguments, one call per binding
fn scan_units<'a>(
    units: impl Iterator<Item = (Entity, u64, Vec3, Option<&'a ObjectStore>)>,
    origin: Option<Vec3>,
    names: &NameCache,
    query: &str,
    search: NameSearch,
    filter: Filter,
    mode: Match,
    roster: Roster<'_>,
    assistable: impl Fn(u64, Option<&ObjectStore>) -> bool,
) -> Option<(Option<Entity>, u64, String)> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let Some(origin) = origin else {
        info!("by-name: \"{query}\" — no active player object; nothing resolves");
        return None;
    };
    let mut nameless = 0usize;
    let mut held = Vec::new();
    for (entity, guid, pos, store) in units {
        match search.candidacy(names, guid) {
            Candidacy::Outside => {}
            Candidacy::Nameless => nameless += 1,
            Candidacy::Named(name) => held.push(Held {
                id: entity,
                name,
                guid,
                dist2: pos.distance_squared(origin),
                is_player: guid::is_player(guid),
                assistable_alive: filter == Filter::AssistableAlive && assistable(guid, store),
            }),
        }
    }
    let considered = held.len();
    let best = resolve(
        Search {
            query,
            kinds: search.kinds(),
            filter,
            exact: mode,
            // The `origin` test above is `0x493ae0`.
            active_player: true,
        },
        roster.party,
        roster.raid,
        held,
    );
    match &best {
        Some(pick) => info!(
            "by-name: \"{query}\" ({search:?}) -> \"{}\" guid {:#x} \
             ({}, prefix {}, {:.1} yd) over {considered} named candidates ({nameless} unnamed)",
            pick.name,
            pick.guid,
            if pick.rank.exact { "exact" } else { "prefix" },
            pick.rank.prefix,
            pick.rank.dist2.sqrt(),
        ),
        None => info!(
            "by-name: \"{query}\" ({search:?}) -> NO MATCH over {considered} named candidates \
             ({nameless} unnamed, {mode:?}); target left untouched"
        ),
    }
    best.map(|p| (p.object, p.guid, p.name.to_string()))
}

/// The player-only name search for a caller that holds the [`NameCache`] mutably and so cannot
/// take [`ByNameScan`]: `DoEmote`'s target (`0x49fdb1`).
#[derive(SystemParam)]
pub(crate) struct PlayerLookup<'w, 's> {
    /// World positions, [`GlobalTransform`]: the chat drain already reads it, where `Transform`
    /// would put it in an undeclared order against every system that moves a unit.
    units: Query<'w, 's, (Entity, &'static Guid, &'static GlobalTransform)>,
    self_q: Query<'w, 's, &'static GlobalTransform, With<SelfPlayer>>,
}

impl PlayerLookup<'_, '_> {
    /// `0x493aa0` with typemask `0x10`, filter mode 0 and no exact-only flag, as `/assist <name>`
    /// calls it: the party slots and the raid members with no object held, then any player in the
    /// object list, whole name or longest prefix. Selects nothing, so the pick is only a guid.
    /// `group` and `index` are the roster and the object manager's guids, which the chat drain
    /// holds itself.
    pub(crate) fn find(
        &self,
        names: &NameCache,
        group: &crate::ui_party::GroupState,
        index: &GuidIndex,
        name: &str,
    ) -> Option<u64> {
        let origin = self.self_q.single().ok().map(|tf| tf.translation());
        fn entry<'a>(
            m: &'a benilla_protocol::messages::GroupMemberEntry,
            index: &GuidIndex,
        ) -> RosterEntry<'a> {
            RosterEntry {
                name: &m.name,
                guid: m.guid,
                held: index.0.contains_key(&m.guid),
            }
        }
        let in_raid = group.group_type == crate::ui_party::GROUPTYPE_RAID;
        let roster = Roster {
            party: group.party_slots().map(|m| entry(m, index)).collect(),
            raid: group
                .members
                .iter()
                .filter(|_| in_raid)
                .map(|m| entry(m, index))
                .collect(),
        };
        scan_units(
            self.units
                .iter()
                .map(|(e, g, tf)| (e, g.0, tf.translation(), None)),
            origin,
            names,
            name,
            NameSearch::PlayerOnly,
            Filter::AcceptAll,
            Match::PrefixOk,
            roster,
            |_, _| true,
        )
        .map(|(_, g, _)| g)
    }
}

/// `/target <name>` and the Lua `TargetByName(name, exactMatch)` through [`scan::commit`], the
/// stop, select, re-swing path a click takes. `exact` is the Lua second argument, which `0x489d8e`
/// reads with default 0 as the resolver's exact-only flag (consumed at `0x493cab`). A miss leaves
/// the target alone, as neither failure edge in `0x489db4` calls `SetSelection`, and prints
/// nothing; the reference prints message `0x127` `ERR_UNIT_NOT_FOUND` there, or `0xb8`
/// `ERR_GENERIC_NO_TARGET` for an empty name.
pub(super) fn target_named(
    scan_params: &ByNameScan,
    commit: &mut SelectCommit,
    name: &str,
    exact: bool,
) {
    let Some((entity, guid, _)) = scan_params.resolve(
        name,
        NameSearch::AnyUnit,
        Filter::AcceptAll,
        if exact {
            Match::ExactOnly
        } else {
            Match::PrefixOk
        },
    ) else {
        return;
    };
    commit.commit(entity, guid);
}

/// `/assist [name]` and the Lua `AssistByName`: the basis is a named player, or bare the current
/// selection (`AssistUnit("target")`, `ChatFrame.lua:744`), and [`SelectCommit::assist`] selects
/// its target.
pub(super) fn assist_named(
    scan_params: &ByNameScan,
    commit: &mut SelectCommit,
    name: Option<&str>,
    how: &str,
) {
    let basis = match name {
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
        info!("assist ({how}): no basis unit; nothing to assist");
        return;
    };
    // From here on this is `AssistUnit`'s tail too, one function as in the reference.
    commit.assist(basis, how);
}

/// Drain the chat layer's `/target <name>` asks.
pub(super) fn target_by_name_requests(
    mut requests: MessageReader<TargetByNameRequest>,
    scan_params: ByNameScan,
    mut commit: SelectCommit,
) {
    for request in requests.read() {
        target_named(&scan_params, &mut commit, &request.name, false);
    }
}

/// Drain the chat layer's `/assist [name]` asks.
pub(super) fn assist_requests(
    mut requests: MessageReader<AssistRequest>,
    scan_params: ByNameScan,
    mut commit: SelectCommit,
) {
    for request in requests.read() {
        assist_named(
            &scan_params,
            &mut commit,
            request.name.as_deref(),
            "/assist",
        );
    }
}

/// Drain `/follow [name]` into [`crate::player`]; follow sends nothing on the wire. The followee's
/// name is latched here for `AUTOFOLLOW_BEGIN` ([`crate::ui_follow`]).
#[allow(clippy::too_many_arguments)]
pub(super) fn follow_requests(
    mut requests: MessageReader<crate::player::FollowRequest>,
    scan_params: ByNameScan,
    selection: Res<Selection>,
    tokens: crate::ui_unit::UnitTokens,
    mover: Query<(&Guid, &ObjectStore), With<crate::net::ActiveMover>>,
    cast: Res<crate::spell::PendingCast>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    mut follow: ResMut<crate::player::FollowState>,
    mut approach: ResMut<crate::player::Approach>,
) {
    use crate::player::FollowRequest;
    for request in requests.read() {
        // The followee each entry point finds, and the line it shows when that is no unit.
        let (guid, missing) = match request {
            // `FollowByName` (`0x489ec0`): a nonempty name through the mode-2 scan (`0x489f23`),
            // which finds nobody as `ERR_UNIT_NOT_FOUND` (`0x489fa5`); an empty one the
            // selection (`0x489f2a`), whose miss is `ERR_GENERIC_NO_TARGET` (`0x489fbd`).
            FollowRequest::Name { name, exact } if !name.is_empty() => (
                scan_params
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
                    .map(|(_, guid, _)| guid),
                "ERR_UNIT_NOT_FOUND",
            ),
            FollowRequest::Name { .. } => (selection.guid, "ERR_GENERIC_NO_TARGET"),
            // `FollowUnit` (`0x489e00`): the unit the token names (`0x515940`), any token.
            FollowRequest::Unit(token) => (
                tokens.resolve(token, &selection).map(|(_, guid)| guid),
                "ERR_GENERIC_NO_TARGET",
            ),
        };
        // Both look the followee up with the unit typemask (`0x489e5c`, `0x489f46`).
        let followee = guid.and_then(|guid| {
            let store = tokens.stores.get(tokens.held(guid)?.0).ok();
            Some((guid, store.filter(|s| s.is_unit())?))
        });
        let mover = mover.iter().next().map(|(guid, store)| (guid.0, store));
        let casting = cast.in_flight(std::time::Instant::now());
        match scan_params.follow_refusal(mover, followee, missing, casting) {
            // A refusal leaves a follow in progress alone: the gate returns before `0x611130`.
            Some(FollowRefusal::Line(key)) => {
                info!("follow: refused — {key}");
                errors.0.push(crate::ui_action::UiError::key(key));
            }
            Some(FollowRefusal::Silent) => info!("follow: the mover itself — nothing"),
            None => {
                let guid = followee.map_or(0, |(guid, _)| guid);
                let name = scan_params
                    .names
                    .resolve_unit(guid, None, Ask::Read)
                    .unwrap_or_default()
                    .to_string();
                info!("follow: now following \"{name}\" guid {guid:#x}");
                // The arm `0x611130` cancels what runs first: one auto-move cell.
                approach.stop();
                follow.start(guid, name);
            }
        }
    }
}

/// Everything [`scan::commit`], the one `SetSelection` path, needs.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled param, the app's convention for big query sets
pub(crate) struct SelectCommit<'w, 's> {
    pub(super) selection: ResMut<'w, Selection>,
    pub(crate) seam: crate::creature_anim::AttackSeam<'w, 's>,
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
    relations: super::RelationWorld<'w, 's>,
    /// `assistAttack`, the assist tail's second leg.
    assist_attack: Res<'w, super::AssistAttack>,
    /// The returns ahead of every selection `SetSelection` makes: charmed, and the cursor's bind.
    pub(super) front: super::front::SelectFront<'w, 's>,
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
        let attackable = (|| {
            let target = self.relations.unit_of(guid, self.stores.get(entity).ok()?);
            Some(
                self.relations
                    .get()
                    .can_attack(&self.relations.me()?, &target),
            )
        })() == Some(true);
        self.commit(entity, guid);
        // The second leg. `0x5ecb70`'s attackability test is the caller's here, so a friendly
        // target is selected, not swung at.
        if self.assist_attack.0 && attackable {
            info!("assist ({how}): assistAttack is on -> opening the swing");
            self.seam.start(guid, engaged, false);
        }
    }

    /// Whether we are mid-swing, which a deselect stops.
    pub(super) fn engaged(&self) -> bool {
        self.me.single().is_ok_and(|(_, _, engaged)| engaged)
    }

    /// Deselect (`0x493540(0,0)`), a no-op with nothing selected; see [`super::click::clear`].
    pub(super) fn clear(&mut self) {
        let engaged = self.engaged();
        super::click::deselect(&mut self.selection, &mut self.seam, engaged);
    }

    /// `0x48f3a0`'s select on a looted object, which tests `TYPEMASK_UNIT` first (`0x48f3af`).
    pub(crate) fn select_unit(&mut self, guid: u64) {
        let Some(&entity) = self.index.0.get(&guid) else {
            return;
        };
        if self.stores.get(entity).is_ok_and(ObjectStore::is_unit) {
            self.commit(entity, guid);
        }
    }

    /// Select a resolved guid, `0x489a40`'s arm 1, through [`scan::commit`], unless
    /// `SelectTarget`'s front returns first ([`super::front`]): with the targeting cursor up the unit
    /// is bound to the cast and the selection stays.
    pub(super) fn commit(&mut self, entity: Entity, guid: u64) {
        if self.front.takes(entity, guid) {
            return;
        }
        let me = self.me.single().ok();
        // The re-swing's gate is `StartAttack`'s on the new target (`0x5ecc16`): alive, then
        // `CanAttack`, so a body is switched to and never swung at.
        let attackable = self.stores.get(entity).ok().is_some_and(|store| {
            let unit = self.relations.unit_of(guid, store);
            let rel = self.relations.get();
            scan::attack_target_valid(&rel, self.relations.me().as_ref(), &unit, store)
        });
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
    use benilla_ui::script::by_name::{common_prefix_len, rank, Rank};

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
        // An enemy player fails CanCooperate, and shares the creature's line.
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

    /// Mode 2 (`0x493eca`) is `CanAssist` and health above 0: a dead friend and a living enemy are
    /// both refused, a living friend (PvP-flagged, as a creature must be for the player to assist
    /// it, `0x6067a7`) passes.
    #[test]
    fn the_follow_filter_wants_an_assistable_unit_with_health() {
        use crate::spell::targeting::corpse_fixture as fx;
        let factions = fx::factions();
        let reps = crate::net::Reputations::default();
        // UNIT_FIELD_FACTIONTEMPLATE, _FLAGS (player-controlled, PvP), _HEALTH and _MAXHEALTH.
        let unit = |template: u32, flags: u32, health: u32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (35, template),
                (46, flags),
                (22, health),
                (28, 50),
            ]))
        };
        let me = super::super::RelUnit::of(1, &unit(1, 0x8, 50));
        let rel = super::super::Relations {
            factions: Some(&factions),
            reputations: &reps,
            me: Some(1),
            roster: None,
            units: &super::super::relations::NoUnits,
        };
        let ask = |u: &ObjectStore| {
            let target = super::super::RelUnit::of(2, u);
            assistable_alive(&rel, Some(&me), Some((&target, u)))
        };
        assert!(ask(&unit(1, 0x1000, 50)));
        assert!(!ask(&unit(1, 0x1000, 0)), "dead");
        assert!(!ask(&unit(2, 0x1000, 50)), "an enemy");
        assert!(!assistable_alive(&rel, Some(&me), None));
    }

    const ME: u64 = 0x10;
    const THEM: u64 = 0x20;
    /// A party member the object manager does not hold.
    const AWAY: u64 = 0x30;
    const ARBITER: u64 = 0xF110_0000_0000_0001;

    /// Us, named "Me", the active mover, and one other living player, "Them", judged by the
    /// relation fixture's tables; "Away" is in our party slots with no object, and `party` puts
    /// "Them" there too. Nothing is selected. Returns his entity.
    fn follow_world(
        me: super::super::RelUnit,
        them: super::super::RelUnit,
        party: bool,
    ) -> (World, Entity) {
        use crate::target::relations::fixture::{factions, store_of};
        use benilla_protocol::messages::{member_status, GroupMemberEntry};
        // `UNIT_FIELD_HEALTH` and `_MAXHEALTH`.
        let alive = |u: &super::super::RelUnit| {
            let mut store = store_of(u);
            store.0.merge(benilla_protocol::ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
            ]));
            store
        };
        let mut world = World::new();
        world.insert_resource(factions());
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<Selection>();
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::ui_action::UiErrorKeys>();
        world.init_resource::<crate::player::FollowState>();
        world.init_resource::<crate::player::Approach>();
        world.init_resource::<Messages<crate::player::FollowRequest>>();
        let mut names = NameCache::default();
        names.insert_player(ME, "Me".into(), None);
        names.insert_player(them.guid, "Them".into(), None);
        names.insert_player(AWAY, "Away".into(), None);
        world.insert_resource(names);
        let mut slots = vec![AWAY];
        slots.extend(party.then_some(them.guid));
        world.insert_resource(crate::spell::group_relation::GroupRoster {
            party: slots.clone(),
            raid: Vec::new(),
        });
        let mut group = crate::ui_party::GroupState::default();
        let entry = |guid| GroupMemberEntry {
            name: String::new(),
            guid,
            status: member_status::ONLINE,
            flags: 0,
        };
        group.apply_list(
            0,
            0,
            slots.into_iter().map(entry).collect(),
            ME,
            None,
            Some(ME),
        );
        world.insert_resource(group);
        let mut index = GuidIndex::default();
        let e = world
            .spawn((
                SelfPlayer,
                crate::net::ActiveMover,
                Guid(me.guid),
                GlobalTransform::default(),
                alive(&me),
            ))
            .id();
        index.0.insert(me.guid, e);
        let t = world
            .spawn((
                Guid(them.guid),
                GlobalTransform::from_translation(Vec3::X),
                alive(&them),
            ))
            .id();
        index.0.insert(them.guid, t);
        world.insert_resource(index);
        (world, t)
    }

    /// Select `guid` at `entity`.
    fn select(world: &mut World, target: Option<(Entity, u64)>) {
        let mut selection = world.resource_mut::<Selection>();
        selection.target = target.map(|(e, _)| e);
        selection.guid = target.map(|(_, g)| g);
    }

    /// One request through the follow drain: the error lines it queued, and whom we follow after.
    fn drain(
        world: &mut World,
        request: crate::player::FollowRequest,
    ) -> (Vec<&'static str>, Option<u64>) {
        world.write_message(request);
        world
            .run_system_cached(follow_requests)
            .expect("the drain runs");
        let keys = std::mem::take(&mut world.resource_mut::<crate::ui_action::UiErrorKeys>().0)
            .into_iter()
            .map(|e| e.key)
            .collect();
        (keys, world.resource::<crate::player::FollowState>().guid)
    }

    fn unit(token: &str) -> crate::player::FollowRequest {
        crate::player::FollowRequest::Unit(token.into())
    }

    fn name(name: &str) -> crate::player::FollowRequest {
        crate::player::FollowRequest::Name {
            name: name.into(),
            exact: false,
        }
    }

    /// `/follow <name>` keeps only the candidates `CanAssist` passes (`0x493eca`), its
    /// player-controlled arm included (`0x60673e`-`0x60679d`): a Human in someone else's duel, or
    /// free-for-all while we are not, is no candidate, nor is our own duel opponent; a plain
    /// Human is.
    #[test]
    fn the_follow_search_refuses_a_duel_or_free_for_all_candidate() {
        use crate::target::relations::fixture::{duelling, flagged, player, FFA, HUMAN};
        use bevy::ecs::system::RunSystemOnce;

        let candidate = |me, them| {
            let (mut world, _) = follow_world(me, them, false);
            world
                .run_system_once(|scan: ByNameScan| {
                    scan.resolve(
                        "Them",
                        NameSearch::PlayerOnly,
                        Filter::AssistableAlive,
                        Match::ExactOnly,
                    )
                    .map(|(_, guid, _)| guid)
                })
                .expect("the scan runs as a one-shot system")
        };
        let (me, human) = (player(ME, HUMAN), player(THEM, HUMAN));
        assert_eq!(candidate(me, human), Some(THEM));
        assert_eq!(
            candidate(me, duelling(human, ARBITER, 1)),
            None,
            "in someone else's duel"
        );
        assert_eq!(
            candidate(duelling(me, ARBITER, 2), duelling(human, ARBITER, 1)),
            None,
            "our duel opponent"
        );
        assert_eq!(
            candidate(me, flagged(human, FFA)),
            None,
            "free-for-all while we are not"
        );
    }

    /// The follow start gate asks `CanCooperate(mover, followee)` (`0x60ff6a`), the faction group
    /// alone: our duel opponent and a Human free-for-all while we are one too are followed, though
    /// hostile and attackable; an Orc in our party is not, though friendly, nor a charmed Human
    /// (`0x606bab`).
    #[test]
    fn the_follow_gate_asks_cooperation_not_reaction() {
        use crate::target::relations::fixture::{duelling, flagged, player, FFA, HUMAN, ORC};

        let follow = |me, them, party| {
            let (mut world, t) = follow_world(me, them, party);
            select(&mut world, Some((t, THEM)));
            drain(&mut world, unit("target"))
        };
        const INVALID: (&[&str], Option<u64>) = (&["ERR_INVALID_FOLLOW_TARGET"], None);
        let followed: (&[&str], Option<u64>) = (&[], Some(THEM));
        let (me, human) = (player(ME, HUMAN), player(THEM, HUMAN));
        let check = |got: (Vec<&'static str>, Option<u64>), want: (&[&str], Option<u64>), why| {
            assert_eq!((got.0.as_slice(), got.1), want, "{why}");
        };
        check(follow(me, human, false), followed, "a Human");
        check(
            follow(duelling(me, ARBITER, 1), duelling(human, ARBITER, 2), false),
            followed,
            "our duel opponent",
        );
        check(
            follow(flagged(me, FFA), flagged(human, FFA), false),
            followed,
            "both free-for-all",
        );
        check(
            follow(me, player(THEM, ORC), true),
            INVALID,
            "an Orc in our party",
        );
        let charmed = super::super::RelUnit {
            charmed_by: 0x40,
            ..human
        };
        check(follow(me, charmed, false), INVALID, "a charmed Human");
    }

    /// The gate returns at once, with no line, when the followee is the mover itself
    /// (`0x60feeb`-`0x60fef1`), ahead of `CanCooperate`, which refuses one object (`0x606ba6`):
    /// `FollowUnit("player")`, `"target"` on yourself, `/follow` with your own name, and a
    /// free-for-all you, whose reaction toward himself stays friendly. A follow in progress is
    /// left alone.
    #[test]
    fn following_yourself_does_nothing_and_says_nothing() {
        use crate::target::relations::fixture::{flagged, player, FFA, HUMAN};

        let nothing: (Vec<&'static str>, Option<u64>) = (Vec::new(), None);
        for me in [player(ME, HUMAN), flagged(player(ME, HUMAN), FFA)] {
            let (mut world, _) = follow_world(me, player(THEM, HUMAN), false);
            assert_eq!(drain(&mut world, unit("player")), nothing);
            let me_entity = world.resource::<GuidIndex>().0[&ME];
            select(&mut world, Some((me_entity, ME)));
            assert_eq!(drain(&mut world, unit("target")), nothing);
            assert_eq!(drain(&mut world, name("Me")), nothing);
        }
        let (mut world, t) = follow_world(player(ME, HUMAN), player(THEM, HUMAN), false);
        select(&mut world, Some((t, THEM)));
        assert_eq!(drain(&mut world, unit("target")), (Vec::new(), Some(THEM)));
        assert_eq!(
            drain(&mut world, unit("player")),
            (Vec::new(), Some(THEM)),
            "still following"
        );
    }

    /// The entry points find the followee's unit and the mover before the gate, and a miss is
    /// their own line: `FollowUnit` on a party member with no object, or with no active mover,
    /// `ERR_GENERIC_NO_TARGET` (`0x489e61`-`0x489e8f`); `FollowByName` with a name that finds
    /// nobody `ERR_UNIT_NOT_FOUND` (`0x489fa5`), and with an empty name the selection
    /// (`0x489f2a`), nobody selected `ERR_GENERIC_NO_TARGET` (`0x489fbd`).
    #[test]
    fn a_followee_or_mover_with_no_unit_shows_the_entry_points_line() {
        use crate::target::relations::fixture::{player, HUMAN};

        let (mut world, t) = follow_world(player(ME, HUMAN), player(THEM, HUMAN), false);
        let no_target = (vec!["ERR_GENERIC_NO_TARGET"], None);
        assert_eq!(drain(&mut world, unit("party1")), no_target, "out of view");
        assert_eq!(
            drain(&mut world, name("Nobody")),
            (vec!["ERR_UNIT_NOT_FOUND"], None)
        );
        assert_eq!(drain(&mut world, name("")), no_target, "nobody selected");
        select(&mut world, Some((t, THEM)));
        assert_eq!(drain(&mut world, name("")), (Vec::new(), Some(THEM)));
        world.resource_mut::<crate::player::FollowState>().stop();
        let me_entity = world.resource::<GuidIndex>().0[&ME];
        world
            .entity_mut(me_entity)
            .remove::<crate::net::ActiveMover>();
        assert_eq!(drain(&mut world, unit("target")), no_target, "no mover");
    }

    /// `DoEmote`'s search (`0x49fde9`, typemask 0x10, mode 0) runs the roster passes: a groupmate
    /// with no object answers to a prefix, and a streamed player nearer than the roster's
    /// placeholder distance with an equal prefix answers instead.
    #[test]
    fn the_emote_search_finds_a_groupmate_with_no_object() {
        use benilla_protocol::messages::GroupMemberEntry;
        use bevy::ecs::system::RunSystemOnce;
        const ME: u64 = 0x10;
        const BOB: u64 = 0x20;
        const BOBBY: u64 = 0x30;
        let mut world = World::new();
        world.init_resource::<crate::ui_party::GroupState>();
        world.init_resource::<GuidIndex>();
        world.spawn((
            SelfPlayer,
            Guid(ME),
            GlobalTransform::from_translation(Vec3::ZERO),
        ));
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .members
            .push(GroupMemberEntry {
                name: "Bob".into(),
                guid: BOB,
                status: 1,
                flags: 0,
            });
        world.init_resource::<NameCache>();
        world
            .resource_mut::<NameCache>()
            .insert_player(ME, "Me".into(), None);
        let find = |world: &mut World, query: &'static str| {
            world
                .run_system_once(
                    move |lookup: PlayerLookup,
                          names: Res<NameCache>,
                          group: Res<crate::ui_party::GroupState>,
                          index: Res<GuidIndex>| {
                        lookup.find(&names, &group, &index, query)
                    },
                )
                .expect("the lookup runs as a one-shot system")
        };
        assert_eq!(find(&mut world, "Bo"), Some(BOB));
        assert_eq!(find(&mut world, "Zed"), None);
        // Bobby is streamed and named: nearer than the placeholder, an equal prefix.
        let bobby = world
            .spawn((
                Guid(BOBBY),
                GlobalTransform::from_translation(Vec3::new(9.0, 0.0, 0.0)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(BOBBY, bobby);
        world
            .resource_mut::<NameCache>()
            .insert_player(BOBBY, "Bobby".into(), None);
        assert_eq!(find(&mut world, "Bo"), Some(BOBBY));
        // Once Bob is streamed too, the roster leaves him to the object pass; unnamed, he is
        // nobody's candidate.
        let bob = world.spawn((Guid(BOB), GlobalTransform::default())).id();
        world.resource_mut::<GuidIndex>().0.insert(BOB, bob);
        assert_eq!(find(&mut world, "Bob"), Some(BOBBY));
    }
}
