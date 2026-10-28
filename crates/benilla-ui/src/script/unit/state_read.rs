//! The state a getter reads for the unit its token resolves to, asked of the lent host at the call
//! ([`crate::script::live::ask`]): the reference's getters resolve the token (`0x515970`), then
//! look the guid up in the object manager (`0x468460`) and read that object and the caches it
//! keys as they stand when the getter runs (`UnitHealth`, `0x5174d0`; `GetRaidTargetIndex`,
//! `0x4bb4f1`), so a listener reads the unit its own packet left. With no host free to ask, the
//! snapshots ([`super::super::Model::snapshot_unit`]) answer.

use super::UnitState;

/// The question: the unit `guid` names, as of the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitStateQuery {
    /// The guid the token resolved to.
    pub guid: u64,
    /// The token is `"player"` itself, which names the active player as its own frames read it:
    /// no reaction toward itself, the raid mark, both faction-group spellings and the guild.
    pub player: bool,
}

/// The unit as the getters read it at the call.
#[derive(Clone, Debug, PartialEq)]
pub enum UnitStateAnswer {
    /// The object manager holds the guid as a unit (typemask 8): its descriptor, the name and
    /// creature caches it keys, its relations to the active player and its raid mark.
    Held(UnitState),
    /// The object manager holds nothing under the guid: what the client knows of it with no
    /// object, the name cache's record and the raid mark by guid (`0x5171eb`, `0x5182bc`,
    /// `0x4bb4f1`), the group's record of a member or a member's pet (`0x496400`, `0x496420`), and
    /// whether the roster names it (`0x491900`).
    Unheld(UnitState),
}

impl crate::script::live::LiveQuery for UnitStateQuery {
    /// Declined for a guid held as anything but a unit, a game object or an item, which the
    /// getters name apart ([`super::ObjectNameQuery`]).
    type Answer = UnitStateAnswer;
}

/// The thinner question the object, identity and record readers ask, which read the unit's own
/// fields through the host themselves: whether the object manager holds the guid as a unit, and
/// the two legs those readers take off the state beside the object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitHeldQuery(pub UnitStateQuery);

/// The thin answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitHeld {
    /// Held as a unit (typemask 8).
    Held {
        /// The guid is a player's.
        is_player: bool,
        /// [`UnitState::reaction`]: its reaction toward the active player plus one, 0 for
        /// `"player"` itself.
        reaction: u8,
    },
    /// The object manager holds nothing under the guid; [`UnitStateQuery`] says what the client
    /// knows of it.
    Unheld,
}

impl crate::script::live::LiveQuery for UnitHeldQuery {
    /// Declined as [`UnitStateQuery`] is.
    type Answer = UnitHeld;
}
