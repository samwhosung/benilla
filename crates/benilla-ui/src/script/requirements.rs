//! What an item is judged against, read as the getter runs: the active player's fields and lists
//! that the usable predicate (`0x5ea930`), the tooltip's red lines (`0x5eaae0`) and the unit
//! tooltip's skinnable line read. Each is a question of its own through the lent host
//! ([`ReqHeadQuery`] and its kin), so a getter reads only what it tests. With no active player
//! held the reference's callers each answer their own way: the item predicate's callers read
//! the item usable (`0x4fb293`, `0x4af764`), the item build hides (`0x52b699`). With no host lent
//! the VM's copy answers, which the app seats for its own reads inside a call's apply
//! ([`super::UiScript::set_player_req_state`]).

use mlua::Lua;

use super::live::{ask, LiveQuery};
use super::{Model, PlayerReqState};

/// What every requirement check reads off the active player's descriptor: `UNIT_FIELD_LEVEL`
/// (`+0x70`), the race and class bytes (`+0x78`, `+0x79`), the lifetime honor rank
/// (`PLAYER_FIELD_BYTES` byte 3) and `PLAYER_FIELD_PVP_MEDALS`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReqHead {
    pub level: u32,
    pub class_id: u32,
    pub race_id: u32,
    pub honor_rank: u8,
    pub pvp_medals: u32,
}

/// The question: [`ReqHead`] now, `None` with no active player held.
pub struct ReqHeadQuery;

impl LiveQuery for ReqHeadQuery {
    type Answer = Option<ReqHead>;
}

/// The question: the skill line's value plus its permanent bonus (`0x5eaae0`), `None` for a line
/// the player lacks.
pub struct ReqSkillQuery(pub u32);

impl LiveQuery for ReqSkillQuery {
    type Answer = Option<u32>;
}

/// The question: the `SMSG_SET_PROFICIENCY` subclass mask of an item class (`0xc4d4a0[class]`),
/// `None` for a class no packet named.
pub struct ProficiencyQuery(pub u32);

impl LiveQuery for ProficiencyQuery {
    type Answer = Option<u32>;
}

/// The question: the player's rank with a faction, 0 hated .. 7 exalted, an unknown faction 0.
pub struct RepRankQuery(pub u32);

impl LiveQuery for RepRankQuery {
    type Answer = u8;
}

/// The question: the dual-wield latch `[0xc4d770]` (`0x5eab70`).
pub struct DualWieldQuery;

impl LiveQuery for DualWieldQuery {
    type Answer = bool;
}

/// The question: the latched skinning spell's (`[0xb700e4]`) `SKINNING` points at the player's
/// skill (`0x529f4e`), `None` without the spell or at 0 or below (`0x529f58`).
pub struct SkinningQuery;

impl LiveQuery for SkinningQuery {
    type Answer = Option<i32>;
}

/// The question: whether the known-spell list holds the spell now (`0x60c740`).
pub struct KnowsSpellQuery(pub u32);

impl LiveQuery for KnowsSpellQuery {
    type Answer = bool;
}

/// The requirement reads, each field read as it is asked.
pub trait ReqRead {
    fn head(&self) -> ReqHead;
    fn skill(&self, id: u32) -> Option<u32>;
    fn proficiency(&self, item_class: u32) -> Option<u32>;
    fn rep_rank(&self, faction: u32) -> u8;
    fn can_dual_wield(&self) -> bool;
    fn skinning(&self) -> Option<i32>;
}

impl ReqRead for PlayerReqState {
    fn head(&self) -> ReqHead {
        ReqHead {
            level: self.level,
            class_id: self.class_id,
            race_id: self.race_id,
            honor_rank: self.honor_rank,
            pvp_medals: self.pvp_medals,
        }
    }

    fn skill(&self, id: u32) -> Option<u32> {
        self.skills.get(&id).copied()
    }

    fn proficiency(&self, item_class: u32) -> Option<u32> {
        self.proficiency.get(&item_class).copied()
    }

    fn rep_rank(&self, faction: u32) -> u8 {
        self.rep_ranks.get(&faction).copied().unwrap_or(0)
    }

    fn can_dual_wield(&self) -> bool {
        self.can_dual_wield
    }

    fn skinning(&self) -> Option<i32> {
        self.skinning
    }
}

/// The player as a getter reads it now.
pub(crate) enum ReqNow<'a> {
    /// The active player through the lent host, its head read once.
    Live(&'a Lua, ReqHead),
    /// No active player held.
    NoPlayer,
    /// No host lent: the VM's copy.
    Copy(PlayerReqState),
}

impl ReqRead for ReqNow<'_> {
    fn head(&self) -> ReqHead {
        match self {
            ReqNow::Live(_, head) => *head,
            ReqNow::NoPlayer => ReqHead::default(),
            ReqNow::Copy(c) => c.head(),
        }
    }

    fn skill(&self, id: u32) -> Option<u32> {
        match self {
            ReqNow::Live(lua, _) => ask(lua, &ReqSkillQuery(id)).flatten(),
            ReqNow::NoPlayer => None,
            ReqNow::Copy(c) => c.skill(id),
        }
    }

    fn proficiency(&self, item_class: u32) -> Option<u32> {
        match self {
            ReqNow::Live(lua, _) => ask(lua, &ProficiencyQuery(item_class)).flatten(),
            ReqNow::NoPlayer => None,
            ReqNow::Copy(c) => c.proficiency(item_class),
        }
    }

    fn rep_rank(&self, faction: u32) -> u8 {
        match self {
            ReqNow::Live(lua, _) => ask(lua, &RepRankQuery(faction)).unwrap_or(0),
            ReqNow::NoPlayer => 0,
            ReqNow::Copy(c) => c.rep_rank(faction),
        }
    }

    fn can_dual_wield(&self) -> bool {
        match self {
            ReqNow::Live(lua, _) => ask(lua, &DualWieldQuery).unwrap_or(false),
            ReqNow::NoPlayer => false,
            ReqNow::Copy(c) => c.can_dual_wield,
        }
    }

    fn skinning(&self) -> Option<i32> {
        match self {
            ReqNow::Live(lua, _) => ask(lua, &SkinningQuery).flatten(),
            ReqNow::NoPlayer => None,
            ReqNow::Copy(c) => c.skinning,
        }
    }
}

/// The player the requirement checks read now ([`ReqNow`]).
pub(crate) fn player_req<'a>(lua: &'a Lua, model: &Model) -> ReqNow<'a> {
    match ask(lua, &ReqHeadQuery) {
        Some(Some(head)) => ReqNow::Live(lua, head),
        Some(None) => ReqNow::NoPlayer,
        None => ReqNow::Copy(model.player_req.clone()),
    }
}

/// The active player's head now, `None` with none held; with no host lent, the VM's copy.
pub(crate) fn player_head(lua: &Lua, model: &Model) -> Option<ReqHead> {
    ask(lua, &ReqHeadQuery).unwrap_or_else(|| Some(model.player_req.head()))
}

/// Whether the lent host holds no active player, where the item build hides (`0x52b699`); `false`
/// with no host lent.
pub(crate) fn no_player(lua: &Lua) -> bool {
    ask(lua, &ReqHeadQuery) == Some(None)
}

/// Whether the player knows spell `id` now: the list through the host, else the VM's copy.
pub(crate) fn knows_spell(lua: &Lua, model: &Model, id: u32) -> bool {
    ask(lua, &KnowsSpellQuery(id)).unwrap_or_else(|| model.known_spells.contains(&id))
}
