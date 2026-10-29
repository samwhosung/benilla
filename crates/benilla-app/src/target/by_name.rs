//! Selection by name: the reference's one resolver (`0x493aa0`) behind `/target`, `/assist` and
//! `/follow`, parameterised per binding.
//!
//! | binding | typemask | filter mode | exact-only |
//! |---|---|---|---|
//! | `TargetByName 0x489d60` | 8, creatures and players | 0 | Lua arg 2 |
//! | `AssistByName 0x489c40` | 0x10, players only | 0 | 0 |
//! | `FollowByName 0x489ec0` | 0x10, players only | 2, `CanAssist` and alive | Lua arg 2 |
//!
//! The ranking is [`benilla_ui::script::by_name::resolve`], the one resolver the VM's name verbs
//! (`SetLootMethod`'s master name, `StartDuel`, `PromoteByName`) share; this module feeds it the
//! held units, each positioned at the call, and breaks a full tie by guid, as the VM's walk
//! over the units the call host offers does ([`crate::ui_unit::live`]). The reference's walk follows the object
//! list's order (a tail append, `0x4646c3`-`0x4646d4`), the deviation documented there.
//!
//! Every caller runs the roster passes (party `0x493b22`, raid `0x493ba7`), which offer a member
//! the object manager does not hold: `TargetByName` selects it by guid (`0x493540` stores a guid
//! with no object, `0x493605`), and `/assist` and `/follow`, which then look the pick up as a unit
//! (`0x489c9b`, `0x489f46`), find none and say so.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::guid;
use benilla_ui::script::by_name::{resolve, Filter, Held, Kinds, Match, RosterEntry, Search};

use crate::creature_anim::StartTarget;
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
    /// cache. The one test both [`scan_units`] and the VM's walk ([`crate::ui_unit::live`]) apply,
    /// so the two offer the same units.
    pub(crate) fn candidacy<'a>(self, names: &'a NameCache, guid_val: u64) -> Candidacy<'a> {
        if !self.accepts(guid_val) {
            return Candidacy::Outside;
        }
        names
            .resolve_unit(guid_val, None, Ask::Read)
            .map_or(Candidacy::Nameless, Candidacy::Named)
    }
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
/// | we are not channelling (`UNIT_CHANNEL_SPELL`, the unit block's `+0x228`) | `0x60ffb9` | `0x134` `ERR_TOOBUSYTOFOLLOW` |
///
/// There is no followee-alive check (the per-tick death test ends a follow) and no distance check:
/// follow's row in `0x860a58` has a `0.0` threshold, so `ERR_AUTOFOLLOW_TOO_FAR` (`0x6110a0`)
/// never fires (`0x6110c8`).
fn follow_refusal_key(
    followee_is_player: bool,
    we_cooperate: bool,
    we_are_dead: bool,
    we_are_stunned: bool,
    we_are_channelling: bool,
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
    if we_are_channelling {
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
    self_q: Query<'w, 's, Entity, With<SelfPlayer>>,
    /// The positions as of the call, each composed through its parents on demand, so a unit
    /// created or moved this frame reads where it stands (`0x468380` measures it then).
    transforms: bevy::transform::helper::TransformHelper<'w, 's>,
    /// Read-only: a sweep fires no name query, so an uncached name does not match, as in the
    /// reference.
    names: Res<'w, NameCache>,
    /// The relation inputs behind `CanAssist`, for [`Filter::AssistableAlive`], and
    /// `CanCooperate`, for the follow gate.
    relations: super::RelationWorld<'w, 's>,
    /// The roster passes' party slots and raid rows, absent in a harness without groups.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    /// Whether the object manager holds a member, which keeps it out of the roster passes.
    index: Option<Res<'w, GuidIndex>>,
}

impl ByNameScan<'_, '_> {
    /// The distance origin, the active player's position; without one the reference resolves
    /// nothing (`0x493ae0`).
    fn origin(&self) -> Option<Vec3> {
        let me = self.self_q.single().ok()?;
        position(&self.transforms, me)
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
    /// objects first. Its channel test reads the mover's own `UNIT_CHANNEL_SPELL` (`0x60ffb9`),
    /// not the cast slot: a hard cast in flight does not refuse, a channel does for as long as it
    /// lasts.
    fn follow_refusal(
        &self,
        mover: Option<(u64, &ObjectStore)>,
        followee: Option<(u64, &ObjectStore)>,
        missing: &'static str,
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
            mover_store.0.unit_channel_spell() != 0,
        )
        .map(FollowRefusal::Line)
    }

    /// Resolve a name to a guid through [`scan_units`], the mode argument deciding the filter: a
    /// held unit, or a party or raid member the object manager does not hold.
    fn resolve(&self, query: &str, search: NameSearch, filter: Filter, mode: Match) -> Option<u64> {
        let roster = match (self.group.as_deref(), self.index.as_deref()) {
            (Some(group), Some(index)) => Roster::of(group, index),
            _ => Roster::default(),
        };
        scan_units(
            self.units
                .iter()
                .filter_map(|(e, g, store)| Some((e, g.0, position(&self.transforms, e)?, store))),
            self.origin(),
            &self.names,
            query,
            search,
            filter,
            mode,
            roster,
            |guid, store| self.assistable_alive(guid, store),
        )
        .map(|(_, guid, _)| guid)
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

/// One unit query's candidates, the entities the scene places, each positioned at the call
/// ([`position`]).
type UnitQuery<'w, 's> =
    Query<'w, 's, (Entity, &'static Guid, Option<&'static ObjectStore>), With<GlobalTransform>>;

/// `entity`'s position in the world as of the call, composed from its `Transform` through its
/// parents, so a propagation still to run this frame does not leave it where it stood.
fn position(transforms: &bevy::transform::helper::TransformHelper, entity: Entity) -> Option<Vec3> {
    transforms
        .compute_global_transform(entity)
        .ok()
        .map(|g| g.translation())
}

/// The roster passes' input (`0x493b22`, `0x493ba7`): the party slots and the raid members, by the
/// name the wire gave and whether the object manager holds them. A pick from it has a guid and no
/// entity.
#[derive(Default)]
pub(super) struct Roster<'a> {
    party: Vec<RosterEntry<'a>>,
    raid: Vec<RosterEntry<'a>>,
}

impl<'a> Roster<'a> {
    /// The party slots, then the raid rows while a raid is held, each marked held when the object
    /// manager holds its object.
    fn of(group: &'a crate::ui_party::GroupState, index: &GuidIndex) -> Self {
        let entry = |m: &'a benilla_protocol::messages::GroupMemberEntry| RosterEntry {
            name: &m.name,
            guid: m.guid,
            held: index.0.contains_key(&m.guid),
        };
        Roster {
            party: group.party_slots().map(entry).collect(),
            // The stored rows below the count (`0x493b9d`-`0x493bb5`), ours unnamed and held.
            raid: group
                .raid
                .iter()
                .map(|r| RosterEntry {
                    name: &r.name,
                    guid: r.guid,
                    held: index.0.contains_key(&r.guid),
                })
                .collect(),
        }
    }
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
    units: Query<'w, 's, (Entity, &'static Guid), With<GlobalTransform>>,
    self_q: Query<'w, 's, Entity, With<SelfPlayer>>,
    /// The positions as of the call ([`position`]).
    transforms: bevy::transform::helper::TransformHelper<'w, 's>,
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
        let origin = self
            .self_q
            .single()
            .ok()
            .and_then(|me| position(&self.transforms, me));
        let roster = Roster::of(group, index);
        scan_units(
            self.units
                .iter()
                .filter_map(|(e, g)| Some((e, g.0, position(&self.transforms, e)?, None))),
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

/// `TargetByName(name, exactMatch)`, the stock `/target`'s, through [`scan::commit_owing`], the
/// stop, select, re-swing path a click takes. `exact` is the Lua second argument, which `0x489d8e`
/// reads with default 0 as the resolver's exact-only flag (consumed at `0x493cab`). The pick goes
/// to `SetSelection` as it is (`0x489daa`), so a party member out of range is selected by guid. A
/// miss leaves the target alone, as neither failure edge in `0x489db4` calls `SetSelection`, and
/// shows message `0x127` `ERR_UNIT_NOT_FOUND` for a name given (`0x489dbd`, tested on the raw
/// string's first byte, so blanks count) or `0xb8` `ERR_GENERIC_NO_TARGET` for an empty one
/// (`0x489dcf`).
pub(super) fn target_named(
    scan_params: &ByNameScan,
    commit: &mut SelectCommit,
    errors: &crate::ui_action::UiErrorKeys,
    name: &str,
    exact: bool,
) {
    let Some(guid) = scan_params.resolve(
        name,
        NameSearch::AnyUnit,
        Filter::AcceptAll,
        if exact {
            Match::ExactOnly
        } else {
            Match::PrefixOk
        },
    ) else {
        errors.push(crate::ui_action::UiError::key(missing_line(name)));
        return;
    };
    commit.set(guid);
}

/// The line a by-name verb shows when its pick names no unit: `0x127` `ERR_UNIT_NOT_FOUND` for a
/// name given, `0xb8` `ERR_GENERIC_NO_TARGET` for an empty one, each tested on the raw string's
/// first byte (`0x489db8`, `0x489d12`, `0x489fa0`).
fn missing_line(name: &str) -> &'static str {
    if name.is_empty() {
        "ERR_GENERIC_NO_TARGET"
    } else {
        "ERR_UNIT_NOT_FOUND"
    }
}

/// `AssistByName(name)` (`0x489c40`), the stock `/assist`'s: the basis is the player a nonempty name
/// resolves to (typemask 0x10, mode 0, `0x489c82`), else the selection (`0x489c89`), looked up as a
/// unit (`0x489c9b`, typemask 8); [`SelectCommit::assist`] then selects its target. A basis the
/// object manager does not hold, a party member out of range included, shows
/// [`missing_line`]'s line (`0x489d17`, `0x489d2b`). A bare `/assist` is `AssistUnit("target")`,
/// which resolves the selection's unit and shows the same `0xb8` (`0x489c0e`).
pub(super) fn assist_named(
    scan_params: &ByNameScan,
    commit: &mut SelectCommit,
    errors: &crate::ui_action::UiErrorKeys,
    name: &str,
    how: &str,
) {
    let guid = if name.is_empty() {
        commit.selection.guid
    } else {
        scan_params.resolve(
            name,
            NameSearch::PlayerOnly,
            Filter::AcceptAll,
            Match::PrefixOk,
        )
    };
    let Some(basis) = guid.and_then(|guid| commit.held_unit(guid)) else {
        info!("assist ({how}): no basis unit; nothing to assist");
        errors.push(crate::ui_action::UiError::key(missing_line(name)));
        return;
    };
    // From here on this is `AssistUnit`'s tail too, one function as in the reference.
    commit.assist(basis, how);
}

/// What a follow request comes to at the start gate (`0x60fed0`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FollowStart {
    /// The gate's line, by its GlobalStrings key, shown where the request was made.
    Refused(&'static str),
    /// The gate's silent return (`0x60ff8a`): the mover itself.
    Silent,
    /// Armed (`0x611130`): the followee's name, `AUTOFOLLOW_BEGIN`'s argument (`0x610061`), and
    /// whether the arm's stop (`0x61113e` → `0x60fb60`) ended a follow, which owes
    /// `AUTOFOLLOW_END` (`0x60fb84`) first.
    Started { name: String, replaced: bool },
}

/// The follow's start, from `FollowUnit` and `FollowByName`, the stock `/follow`'s, inside their
/// calls ([`crate::script_calls::ScriptCallHost`]); follow sends nothing on the wire.
#[derive(SystemParam)]
pub(crate) struct FollowSteps<'w, 's> {
    scan_params: ByNameScan<'w, 's>,
    selection: Res<'w, Selection>,
    tokens: crate::ui_unit::UnitTokens<'w, 's>,
    mover: Query<'w, 's, (&'static Guid, &'static ObjectStore), With<crate::net::ActiveMover>>,
    /// Absent in a harness without the mover, which drops the request.
    follow: Option<ResMut<'w, crate::player::FollowState>>,
    approach: Option<ResMut<'w, crate::player::Approach>>,
}

impl FollowSteps<'_, '_> {
    /// `request` through the start gate, and the follow armed when it passes.
    pub(crate) fn start(&mut self, request: &crate::player::FollowRequest) -> FollowStart {
        use crate::player::FollowRequest;
        let scan_params = &self.scan_params;
        // The followee each entry point finds, and the line it shows when that is no unit.
        let (guid, missing) = match request {
            // `FollowByName` (`0x489ec0`): a nonempty name through the mode-2 scan (`0x489f23`),
            // which finds nobody as `ERR_UNIT_NOT_FOUND` (`0x489fa5`); an empty one the
            // selection (`0x489f2a`), whose miss is `ERR_GENERIC_NO_TARGET` (`0x489fbd`).
            FollowRequest::Name { name, exact } if !name.is_empty() => (
                scan_params.resolve(
                    name,
                    NameSearch::PlayerOnly,
                    Filter::AssistableAlive,
                    if *exact {
                        Match::ExactOnly
                    } else {
                        Match::PrefixOk
                    },
                ),
                "ERR_UNIT_NOT_FOUND",
            ),
            FollowRequest::Name { .. } => (self.selection.guid, "ERR_GENERIC_NO_TARGET"),
            // `FollowUnit` (`0x489e00`): the unit the token names (`0x515940`), any token.
            FollowRequest::Unit(token) => (
                self.tokens
                    .resolve(token, &self.selection)
                    .map(|(_, guid)| guid),
                "ERR_GENERIC_NO_TARGET",
            ),
        };
        // Both look the followee up with the unit typemask (`0x489e5c`, `0x489f46`).
        let tokens = &self.tokens;
        let followee = guid.and_then(|guid| {
            let store = tokens.stores.get(tokens.held(guid)?.0).ok();
            Some((guid, store.filter(|s| s.is_unit())?))
        });
        let mover = self
            .mover
            .iter()
            .next()
            .map(|(guid, store)| (guid.0, store));
        match scan_params.follow_refusal(mover, followee, missing) {
            // A refusal leaves a follow in progress alone: the gate returns before `0x611130`.
            Some(FollowRefusal::Line(key)) => {
                info!("follow: refused — {key}");
                FollowStart::Refused(key)
            }
            Some(FollowRefusal::Silent) => {
                info!("follow: the mover itself — nothing");
                FollowStart::Silent
            }
            None => {
                let guid = followee.map_or(0, |(guid, _)| guid);
                let name = scan_params
                    .names
                    .resolve_unit(guid, None, Ask::Read)
                    .unwrap_or_default()
                    .to_string();
                info!("follow: now following \"{name}\" guid {guid:#x}");
                // The arm `0x611130` cancels what runs first: one auto-move cell.
                if let Some(approach) = self.approach.as_deref_mut() {
                    approach.stop();
                }
                let replaced = self.follow.as_deref_mut().is_some_and(|follow| {
                    let replaced = follow.stop();
                    follow.start(guid, name.clone());
                    replaced
                });
                FollowStart::Started { name, replaced }
            }
        }
    }
}

/// The cue `SetSelection` plays for a guid whose object it does not find (`0x843328`).
const UNHELD_SELECT_SOUND: &str = "igCharacterSelect";

/// A swing a selecting verb owes after `SetSelection`'s `PLAYER_TARGET_CHANGED` (`0x49389c`),
/// whose listeners run first: the re-swing (`0x4938c8`), the assist tail's StartAttack at the
/// unit it selected (`0x489bfc`-`0x489c02`, a copy of that guid), when `assistAttack` is on then
/// (`0x489bd0`), or the rest of a StartAttack past its own select (`0x5eccaf` on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OwedSwing {
    Again(scan::OwedSwing),
    Assist { guid: u64 },
    Tail { guid: u64 },
}

/// What a selecting verb owes after its signals, for its caller to pay once their listeners ran
/// ([`SelectCommit::owing`]): the send after the teardown's `PET_BAR_UPDATE`, then the swings after
/// `PLAYER_TARGET_CHANGED`, in the order owed.
#[must_use]
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SelectDebt {
    send: Option<scan::OwedSend>,
    swings: Vec<OwedSwing>,
}

impl From<scan::Owed> for SelectDebt {
    fn from(owed: scan::Owed) -> Self {
        Self {
            send: owed.send,
            swings: owed.swing.map(OwedSwing::Again).into_iter().collect(),
        }
    }
}

impl SelectDebt {
    /// The send owed after the teardown's signal.
    pub(crate) fn take_send(&mut self) -> Option<scan::OwedSend> {
        self.send.take()
    }

    /// Whether swings are owed after the selection's signal.
    pub(crate) fn owes_swings(&self) -> bool {
        !self.swings.is_empty()
    }

    /// The debt of StartAttack's own select (`0x5eccaa`), then the rest of StartAttack at `guid`
    /// past it (`0x5eccaf` on), after the select's signals and its re-swing.
    pub(crate) fn then_tail(mut self, guid: u64) -> Self {
        self.swings.push(OwedSwing::Tail { guid });
        self
    }

    /// Pay all of it now, for an engine writer whose signals have no listeners to run first:
    /// nothing ran since its commit, so a re-swing finds the guid it stored.
    pub(crate) fn pay(
        self,
        selection: &mut Selection,
        seam: &mut crate::creature_anim::AttackSeam,
    ) {
        if let Some(send) = self.send {
            send.pay(selection, seam);
        }
        for swing in self.swings {
            match swing {
                OwedSwing::Again(owed) => owed.pay(selection, seam, StartTarget::Unheld),
                OwedSwing::Tail { guid } => seam.start_tail(guid),
                // The assist tail is a call's alone ([`SelectCommit::assist`]).
                OwedSwing::Assist { .. } => {}
            }
        }
    }
}

/// Where [`SelectCommit`] keeps what its verb owes while [`SelectCommit::owing`] runs.
#[derive(Default)]
pub(super) struct Owing {
    on: bool,
    debt: SelectDebt,
}

/// Everything [`scan::commit_owing`], the one `SetSelection` path, needs.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled param, the app's convention for big query sets
pub(crate) struct SelectCommit<'w, 's> {
    pub(super) selection: ResMut<'w, Selection>,
    pub(crate) seam: crate::creature_anim::AttackSeam<'w, 's>,
    // Our own body: the guid and the store `can_attack` reads.
    me: Query<'w, 's, (&'static Guid, Option<&'static ObjectStore>), With<SelfPlayer>>,
    pub(super) stores: Query<'w, 's, &'static ObjectStore>,
    /// The guid to entity map, `0x489a40`'s `0x468460` lookup.
    index: Res<'w, GuidIndex>,
    relations: super::RelationWorld<'w, 's>,
    /// `assistAttack`, the assist tail's second leg.
    assist_attack: Res<'w, super::AssistAttack>,
    /// The returns ahead of every selection `SetSelection` makes: charmed, and the cursor's bind.
    pub(super) front: super::front::SelectFront<'w, 's>,
    /// The roster `IsRaidMemberOrPet` reads (`0x491900`), absent in a harness without groups.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    /// The 2D cues `PlaySoundByName` (`0x458030`) plays, absent in a harness without sound.
    sounds: Option<Res<'w, crate::sound::MessageSounds>>,
    /// What the verb owes while [`Self::owing`] runs.
    owing: Local<'s, Owing>,
}

impl SelectCommit<'_, '_> {
    /// Run `f`, keeping what its selections owe after their signals in place of sending it, for a
    /// caller that runs the signals' listeners first and then pays it ([`Self::pay_send`],
    /// [`Self::pay`]).
    pub(super) fn owing(&mut self, f: impl FnOnce(&mut Self)) -> SelectDebt {
        self.owing.on = true;
        f(self);
        self.owing.on = false;
        std::mem::take(&mut self.owing.debt)
    }

    /// Owe what a commit owes while [`Self::owing`] runs; pay it now otherwise.
    pub(super) fn owe_all(&mut self, owed: scan::Owed) {
        if let Some(send) = owed.send {
            self.owe_send(send);
        }
        if let Some(swing) = owed.swing {
            self.owe(OwedSwing::Again(swing));
        }
    }

    /// Owe `send` while [`Self::owing`] runs; send it now otherwise.
    fn owe_send(&mut self, send: scan::OwedSend) {
        if self.owing.on {
            debug_assert!(self.owing.debt.send.is_none(), "one selection per verb");
            self.owing.debt.send = Some(send);
        } else {
            self.pay_send(send);
        }
    }

    /// Owe `swing` while [`Self::owing`] runs; send it now otherwise.
    fn owe(&mut self, swing: OwedSwing) {
        if self.owing.on {
            self.owing.debt.swings.push(swing);
        } else {
            self.pay_one(swing);
        }
    }

    /// Store and send the selection the teardown's signal held back.
    pub(crate) fn pay_send(&mut self, send: scan::OwedSend) {
        send.pay(&mut self.selection, &self.seam);
    }

    /// Pay what `debt` still owes, in order, now that the signals' listeners have run, up to a
    /// payment whose own select moved the selection (StartAttack's, `0x5eccaa`): what that one
    /// owes, ahead of the rest, for the caller to run its signals' listeners first and pay again.
    /// A payment that moved nothing has nothing to signal, and what it owes is paid in place.
    pub(crate) fn pay(&mut self, debt: SelectDebt) -> SelectDebt {
        if let Some(send) = debt.send {
            self.pay_send(send);
        }
        let mut swings: std::collections::VecDeque<OwedSwing> = debt.swings.into();
        while let Some(swing) = swings.pop_front() {
            let before = self.selection.guid;
            let mut owed = self.owing(|commit| commit.pay_one(swing));
            if owed.send.is_some() || self.selection.guid != before {
                owed.swings.extend(swings);
                return owed;
            }
            for swing in owed.swings.into_iter().rev() {
                swings.push_front(swing);
            }
        }
        SelectDebt::default()
    }

    /// Pay all of `debt` now, for a test's writer whose signals have no listeners to run first.
    #[cfg(test)]
    pub(crate) fn pay_now(&mut self, mut debt: SelectDebt) {
        while debt != SelectDebt::default() {
            debt = self.pay(debt);
        }
    }

    fn pay_one(&mut self, swing: OwedSwing) {
        match swing {
            OwedSwing::Again(owed) => {
                let now = self
                    .selection
                    .guid
                    .map_or(StartTarget::Unheld, |guid| self.start_target(guid));
                owed.pay(&self.selection, &mut self.seam, now);
            }
            OwedSwing::Assist { guid } => {
                if self.assist_attack.0 {
                    info!("assist: assistAttack is on -> opening the swing at {guid:#x}");
                    self.start_attack(guid);
                }
            }
            OwedSwing::Tail { guid } => self.seam.start_tail(guid),
        }
    }

    /// What `StartAttack`'s target checks find of `guid` (`0x5ecbe6`-`0x5ecc35`): a unit the
    /// object manager holds (typemask 8), then [`Self::attackable`].
    fn start_target(&self, guid: u64) -> StartTarget {
        match self.held_unit(guid) {
            None => StartTarget::Unheld,
            Some(entity) if self.attackable(entity, guid) => StartTarget::Attackable,
            Some(_) => StartTarget::Refused,
        }
    }

    /// StartAttack (`0x5ecb70`) at `guid`, its own select (`0x5ecca0`-`0x5eccaa` → `0x493540`)
    /// between its checks and stand and its swing: [`Self::commit`], the setter's dedup for a guid
    /// selected already, else the whole `SetSelection`, whose re-swing reaches StartAttack again
    /// at the same guid and is deduped there. The rest is owed past that select's signals while
    /// [`Self::owing`] runs.
    pub(crate) fn start_attack(&mut self, guid: u64) {
        let verdict = self.start_target(guid);
        if !self.seam.start_head(verdict) {
            return;
        }
        if let Some(entity) = self.held_unit(guid) {
            self.commit(entity, guid);
        }
        self.owe(OwedSwing::Tail { guid });
    }

    /// `StartAttack`'s target legs (`0x5ecc16`, `0x5ecc2e`) on the held object `entity`, `guid`:
    /// alive, then `CanAttack`, so a body is switched to and never swung at.
    fn attackable(&self, entity: Entity, guid: u64) -> bool {
        self.stores.get(entity).ok().is_some_and(|store| {
            let unit = self.relations.unit_of(guid, store);
            let rel = self.relations.get();
            scan::attack_target_valid(&rel, self.relations.me().as_ref(), &unit, store)
        })
    }

    /// The shared assist tail (`0x489bb2`-`0x489c07`, byte-identical in `AssistByName` at
    /// `0x489cae`-`0x489d07`): select the basis unit's `UNIT_FIELD_TARGET` through `0x489a40`
    /// ([`Self::select`]), so a party member out of range is selected by guid. A basis targeting
    /// nothing is silent (`0x489bc6`), as is a guid `0x489a40` passes over. With `assistAttack`
    /// set, the tail then opens the swing (`0x489c02`/`0x489d02` call `0x5ecb70`).
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
        info!("assist ({how}) -> the basis unit's target, guid {guid:#x}");
        self.select(guid);
        // The second leg, owed past the selection's signal: `assistAttack` is read after it
        // (`0x489bd0`), and StartAttack judges the unit itself, so a friendly one is selected
        // and refused, which stops a running swing (`0x5ecc48`). A swing the select re-pointed
        // (`0x4938c8`) left `[+0xc54]` set, so this one goes out again (`0x5eccda`).
        self.owe(OwedSwing::Assist { guid });
    }

    /// Deselect (`0x493540(0,0)`), a no-op with nothing selected; see [`super::click::clear`].
    pub(super) fn clear(&mut self) {
        if let Some(send) = super::click::deselect_owing(&mut self.selection, &mut self.seam) {
            self.owe_send(send);
        }
    }

    /// `0x493910(guid, 1)`: nothing unless `guid` is still the selection (`0x493949`); else the
    /// teardown, the last-target pair left as it is.
    pub(super) fn tear_down(&mut self, guid: u64) {
        if self.selection.guid != Some(guid) {
            return;
        }
        if let Some(send) = super::click::clear_owing(&mut self.selection, &mut self.seam) {
            self.owe_send(send);
        }
    }

    /// [`Self::select_unit`] inside a packet's handler, which goes on after it: the store, the
    /// send and the re-swing in place, and its signals owed at this place to the packet's end
    /// ([`super::OwedSelect::Signals`]), whose listeners then run behind it.
    pub(crate) fn select_unit_in_packet(&mut self, guid: u64, owed: &mut super::OwedSelects) {
        let before = super::click::Standing::of(&self.selection, &self.seam);
        let was = self.selection.guid;
        self.select_unit(guid);
        if self.selection.guid != was {
            owed.push(super::OwedSelect::Signals(
                before.owing_debt(SelectDebt::default()),
            ));
        }
    }

    /// `0x48f3a0`'s select on a looted object, which tests `TYPEMASK_UNIT` first (`0x48f3af`).
    pub(crate) fn select_unit(&mut self, guid: u64) {
        if let Some(entity) = self.held_unit(guid) {
            self.commit(entity, guid);
        }
    }

    /// The object manager's unit `guid` names (`0x468460`, typemask 8).
    pub(super) fn held_unit(&self, guid: u64) -> Option<Entity> {
        let entity = *self.index.0.get(&guid)?;
        self.stores
            .get(entity)
            .is_ok_and(ObjectStore::is_unit)
            .then_some(entity)
    }

    /// `IsRaidMemberOrPet` (`0x491900`) on `guid`, which needs no object of it.
    fn member_or_pet(&self, guid: u64) -> bool {
        let Some(group) = self.group.as_deref() else {
            return false;
        };
        let fields = |g: u64| Some(&self.stores.get(*self.index.0.get(&g)?).ok()?.0);
        // The active player is matched by guid before any object (`0x4e80b0`).
        let me = self.relations.me_guid().map(|g| (g, fields(g)));
        group.member_or_pet(guid, me, fields)
    }

    /// `CGGameUI::Target` (`0x489a40`), which `TargetUnit`, the assist tail, `TargetLastTarget`
    /// and `TargetLastEnemy` share: `guid` goes to `SetSelection` ([`Self::set`], `0x489a74`) when
    /// the object manager holds its unit (`0x489a59`, typemask 8) or `IsRaidMemberOrPet` names it
    /// (`0x489a64`), so a party member out of range is selected by the guid alone; anything else,
    /// a zero guid included, is a bare `ret` (`0x489a79`), never a deselect. Whether it reached
    /// `SetSelection`.
    pub(super) fn select(&mut self, guid: u64) -> bool {
        if self.held_unit(guid).is_none() && !self.member_or_pet(guid) {
            return false;
        }
        self.set(guid);
        true
    }

    /// `SetSelection` on a guid as `TargetByName` hands it over (`0x489daa`): with its object when
    /// the object manager holds one (`0x4935bf`, typemask 1), else the guid alone (`0x493605`).
    pub(super) fn set(&mut self, guid: u64) {
        match self.index.0.get(&guid).copied() {
            Some(entity) => self.commit(entity, guid),
            None => self.commit_unheld(guid),
        }
    }

    /// `SetSelection` on a guid with no object: the charmed return (`0x49359c`) alone stands
    /// ahead of it, and the guid is stored with no light (`0x493605`). Its object lookup finds
    /// nothing, so it plays `igCharacterSelect` (`0x4936f7 je 0x493843`, `0x843328` through
    /// `0x458030`), past the dedup (`0x493617`).
    fn commit_unheld(&mut self, guid: u64) {
        if self.front.charmed() {
            return;
        }
        let me = self.me.single().ok();
        let old_latches = scan::latches_on(&self.relations, self.selection.guid);
        let (outcome, owed) = scan::commit_owing(
            &mut self.selection,
            &mut self.seam,
            None,
            guid,
            None,
            me.map(|(g, _)| g.0),
            false,
            old_latches,
        );
        self.owe_all(owed);
        if outcome.changed {
            if let Some(sounds) = &self.sounds {
                sounds.push_cue(UNHELD_SELECT_SOUND);
            }
        }
    }

    /// Select the held object `entity`, `guid`, through [`scan::commit_owing`], unless `SelectTarget`'s
    /// front returns first ([`super::front`]): with the targeting cursor up the unit is bound to the
    /// cast and the selection stays.
    pub(super) fn commit(&mut self, entity: Entity, guid: u64) {
        if self.front.takes(entity, guid, self.seam.slots.targeting()) {
            return;
        }
        let me = self.me.single().ok();
        // The re-swing's gate is `StartAttack`'s on the new target (`0x5ecc16`).
        let attackable = self.attackable(entity, guid);
        let old_latches = scan::latches_on(&self.relations, self.selection.guid);
        let (_, owed) = scan::commit_owing(
            &mut self.selection,
            &mut self.seam,
            Some(entity),
            guid,
            self.stores.get(entity).ok(),
            me.map(|(g, _)| g.0),
            attackable,
            old_latches,
        );
        self.owe_all(owed);
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
                Transform::default(),
                GlobalTransform::default(),
                alive(&me),
            ))
            .id();
        index.0.insert(me.guid, e);
        let t = world
            .spawn((
                Guid(them.guid),
                Transform::from_translation(Vec3::X),
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

    /// One request through the follow's start, as `FollowUnit`'s or `FollowByName`'s call makes
    /// it: the line its refusal shows, and whom we follow after.
    fn drain(
        world: &mut World,
        request: crate::player::FollowRequest,
    ) -> (Vec<&'static str>, Option<u64>) {
        use bevy::ecs::system::RunSystemOnce;
        let start = world
            .run_system_once(move |mut steps: FollowSteps| steps.start(&request))
            .expect("the start runs");
        let keys = match start {
            FollowStart::Refused(key) => vec![key],
            _ => Vec::new(),
        };
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

    /// The start gate refuses a follow with `ERR_TOOBUSYTOFOLLOW` while the mover's
    /// `UNIT_CHANNEL_SPELL` is set (`0x60ffb9`, the unit block's word at `+0x228`): a channel
    /// refuses it for as long as it lasts, a hard cast in flight does not.
    #[test]
    fn a_channel_refuses_a_follow_and_a_cast_in_flight_does_not() {
        use crate::target::relations::fixture::{player, HUMAN};
        /// `UNIT_CHANNEL_SPELL`.
        const CHANNEL_SPELL: u16 = 144;
        const DRAIN_LIFE: u32 = 689;

        // (channelling, a cast in flight, followed)
        for (channel, cast, followed) in [
            (false, false, true),
            (true, false, false),
            (false, true, true),
        ] {
            let (mut world, t) = follow_world(player(ME, HUMAN), player(THEM, HUMAN), false);
            select(&mut world, Some((t, THEM)));
            if channel {
                let me = world.resource::<GuidIndex>().0[&ME];
                world
                    .entity_mut(me)
                    .get_mut::<ObjectStore>()
                    .expect("our descriptor")
                    .0
                    .merge(benilla_protocol::ObjectFields::from_pairs(&[(
                        CHANNEL_SPELL,
                        DRAIN_LIFE,
                    )]));
            }
            if cast {
                world.resource_mut::<crate::spell::PendingCast>().push(
                    DRAIN_LIFE,
                    None,
                    std::time::Instant::now(),
                );
            }
            let want: (Vec<&str>, Option<u64>) = if followed {
                (Vec::new(), Some(THEM))
            } else {
                (vec!["ERR_TOOBUSYTOFOLLOW"], None)
            };
            assert_eq!(
                drain(&mut world, unit("target")),
                want,
                "channel {channel}, cast in flight {cast}"
            );
        }
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
            Transform::from_translation(Vec3::ZERO),
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
                Transform::from_translation(Vec3::new(9.0, 0.0, 0.0)),
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
        let bob = world
            .spawn((Guid(BOB), Transform::default(), GlobalTransform::default()))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(BOB, bob);
        assert_eq!(find(&mut world, "Bob"), Some(BOBBY));
    }

    /// The searches measure each unit where it stands at the call (`0x468380` with `0x493d30`):
    /// of two players of an equal prefix, the one a handler moved nearer this frame wins, though
    /// the scene's propagation, which runs after the frame's systems, still has it where it was.
    #[test]
    fn the_searches_measure_a_unit_moved_this_frame() {
        use bevy::ecs::system::RunSystemOnce;
        const ME: u64 = 0x10;
        const BOBA: u64 = 0x20;
        const BOBO: u64 = 0x30;
        let mut world = World::new();
        world.init_resource::<crate::ui_party::GroupState>();
        world.init_resource::<GuidIndex>();
        // The relation inputs the scan holds.
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<crate::spell::group_relation::GroupRoster>();
        world.insert_resource(crate::net::SelfGuid(Some(ME)));
        world.init_resource::<NameCache>();
        let at = |x: f32| {
            let v = Vec3::new(x, 0.0, 0.0);
            (
                Transform::from_translation(v),
                GlobalTransform::from_translation(v),
            )
        };
        world.spawn((SelfPlayer, Guid(ME), at(0.0)));
        let boba = world.spawn((Guid(BOBA), at(5.0))).id();
        let bobo = world.spawn((Guid(BOBO), at(9.0))).id();
        for (guid, entity, name) in [(BOBA, boba, "Boba"), (BOBO, bobo, "Bobo")] {
            world.resource_mut::<GuidIndex>().0.insert(guid, entity);
            world
                .resource_mut::<NameCache>()
                .insert_player(guid, name.into(), None);
        }
        let find = |world: &mut World| {
            world
                .run_system_once(
                    |lookup: PlayerLookup,
                     scan: ByNameScan,
                     names: Res<NameCache>,
                     group: Res<crate::ui_party::GroupState>,
                     index: Res<GuidIndex>| {
                        (
                            lookup.find(&names, &group, &index, "Bob"),
                            scan.resolve(
                                "Bob",
                                NameSearch::PlayerOnly,
                                Filter::AcceptAll,
                                Match::PrefixOk,
                            ),
                        )
                    },
                )
                .expect("the searches run as a one-shot system")
        };
        assert_eq!(find(&mut world), (Some(BOBA), Some(BOBA)), "the nearer");
        // A handler moves Bobo to 1 yard; the propagation has not run.
        world.get_mut::<Transform>(bobo).unwrap().translation = Vec3::new(1.0, 0.0, 0.0);
        assert_eq!(
            find(&mut world),
            (Some(BOBO), Some(BOBO)),
            "where it stands now"
        );
    }
}
