//! Units by role and a scene holding them, to ask the relation functions of.

use std::collections::HashMap;

use benilla_formats::{FactionCatalog, FactionTemplate};

use benilla_protocol::ObjectFields;

use super::{RelUnit, RelationSnapshot, Relations};
use crate::net::{ObjectStore, Reputations};
use crate::spell::group_relation::GroupRoster;
use crate::target::Factions;

/// `UNIT_FIELD_FLAGS`: player-controlled, immune to players, PvP, not selectable.
pub(crate) const PC: u32 = 0x8;
pub(crate) const IMMUNE_TO_PLAYER: u32 = 0x100;
pub(crate) const PVP: u32 = 0x1000;
pub(crate) const NOT_SELECTABLE: u32 = 1 << 25;
/// `PLAYER_FLAGS`: ghost, free-for-all, contested.
pub(crate) const GHOST: u32 = 0x10;
pub(crate) const FFA: u32 = 0x80;
pub(crate) const CONTESTED: u32 = 0x100;

/// Templates 1 (Human) and 2 (Orc), as `corpse_fixture` has them, and 3, a Human-side guard
/// flagged `0x1000`, which turns on a contested player.
pub(crate) const HUMAN: u32 = 1;
pub(crate) const ORC: u32 = 2;
pub(crate) const GUARD: u32 = 3;

/// The three templates' catalog: Human and the guard friendly to each other, both hostile to Orc.
pub(crate) fn factions() -> Factions {
    let tpl = |faction, flags, group_mask, friend_group_mask, enemy_group_mask| FactionTemplate {
        faction,
        flags,
        group_mask,
        friend_group_mask,
        enemy_group_mask,
        ..Default::default()
    };
    Factions::from_catalog(FactionCatalog::from_rows(
        HashMap::from([
            (HUMAN, tpl(1, 0, 3, 2, 12)),
            (ORC, tpl(2, 0, 5, 4, 10)),
            (GUARD, tpl(3, 0x1000, 3, 2, 12)),
        ]),
        HashMap::new(),
    ))
}

/// A player-controlled player of this template.
pub(crate) fn player(guid: u64, template: u32) -> RelUnit {
    RelUnit {
        guid,
        is_player: true,
        flags: PC,
        template: Some(template),
        ..Default::default()
    }
}

/// A creature of this template that nobody controls.
pub(crate) fn creature(guid: u64, template: u32) -> RelUnit {
    RelUnit {
        guid,
        template: Some(template),
        ..Default::default()
    }
}

/// A pet: a player-controlled creature that `owner` created, on its template.
pub(crate) fn pet_of(guid: u64, owner: &RelUnit) -> RelUnit {
    RelUnit {
        guid,
        flags: PC,
        template: owner.template,
        created_by: owner.guid,
        ..Default::default()
    }
}

/// `u` in the duel `arbiter` on `team`, its countdown over.
pub(crate) fn duelling(u: RelUnit, arbiter: u64, team: u32) -> RelUnit {
    RelUnit {
        duel_arbiter: arbiter,
        duel_team: team,
        ..u
    }
}

/// `u` with these `PLAYER_FLAGS` bits set.
pub(crate) fn flagged(u: RelUnit, player_flags: u32) -> RelUnit {
    RelUnit {
        player_flags: u.player_flags | player_flags,
        ..u
    }
}

/// `u` with these `UNIT_FIELD_FLAGS` bits set.
pub(crate) fn with_flags(u: RelUnit, flags: u32) -> RelUnit {
    RelUnit {
        flags: u.flags | flags,
        ..u
    }
}

/// A human warrior player of template 1, race and class as `UNIT_FIELD_BYTES_0` carries them.
pub(crate) fn human_warrior(guid: u64) -> RelUnit {
    RelUnit {
        race: 1,
        class: 1,
        ..player(guid, HUMAN)
    }
}

/// A descriptor carrying what [`RelUnit::of`] reads: `OBJECT_FIELD_TYPE` (2),
/// `UNIT_FIELD_CHARMEDBY` (10), `_CREATEDBY` (14), `_FACTIONTEMPLATE` (35), `_FLAGS` (46),
/// `PLAYER_DUEL_ARBITER` (188), `PLAYER_FLAGS` (190) and `PLAYER_DUEL_TEAM` (196).
pub(crate) fn store_of(u: &RelUnit) -> ObjectStore {
    let lo = |g: u64| g as u32;
    let hi = |g: u64| (g >> 32) as u32;
    let mut pairs = vec![
        (2, if u.is_player { 0x19 } else { 0x09 }),
        (10, lo(u.charmed_by)),
        (11, hi(u.charmed_by)),
        (14, lo(u.created_by)),
        (15, hi(u.created_by)),
        (46, u.flags),
    ];
    pairs.extend(u.template.map(|t| (35, t)));
    if u.is_player {
        pairs.extend([
            (188, lo(u.duel_arbiter)),
            (189, hi(u.duel_arbiter)),
            (190, u.player_flags),
            (196, u.duel_team),
        ]);
    }
    ObjectStore(ObjectFields::from_pairs(&pairs))
}

/// A new human warrior's reputation list as the server first sends it (vmangos
/// `ReputationMgr::Initialize`): every slot at standing 0 with its `ReputationFlags` seed, which
/// puts the other side's factions at war.
pub(crate) fn new_human_reputations(catalog: &FactionCatalog) -> Reputations {
    let mut slots = vec![(0u8, 0i32); 64];
    for (_, info) in catalog.reputation_factions() {
        if let Ok(i) = usize::try_from(info.rep_index) {
            slots[i].0 = info.default_flags_for(1, 1) as u8;
        }
    }
    Reputations::from_slots(slots)
}

/// The active player and whatever else a test holds, with the tables they are judged by.
pub(crate) struct Scene {
    pub(crate) units: HashMap<u64, RelUnit>,
    pub(crate) factions: Option<Factions>,
    pub(crate) reputations: Reputations,
    pub(crate) roster: GroupRoster,
    pub(crate) me: u64,
}

impl Scene {
    /// `me` held and active, judged by `factions`.
    pub(crate) fn new(me: RelUnit, factions: Option<Factions>) -> Self {
        let mut scene = Self {
            units: HashMap::new(),
            factions,
            reputations: Reputations::default(),
            roster: GroupRoster::default(),
            me: me.guid,
        };
        scene.hold(me);
        scene
    }

    /// Hold `u` in the object manager, and hand it back.
    pub(crate) fn hold(&mut self, u: RelUnit) -> RelUnit {
        self.units.insert(u.guid, u);
        u
    }

    /// Put these guids in the party slots.
    pub(crate) fn party(&mut self, members: &[u64]) {
        self.roster.party = members.to_vec();
    }

    /// The scene as the Lua API's relation functions: the copy `feed_unit_relations` pushes.
    pub(crate) fn snapshot(&self) -> RelationSnapshot {
        RelationSnapshot {
            units: self.units.clone(),
            factions: self.factions.clone(),
            reputations: self.reputations.clone(),
            me: Some(self.me),
            roster: self.roster.clone(),
        }
    }

    /// [`Self::snapshot`] as the VM holds it, for `set_unit_relations`.
    pub(crate) fn relations(&self) -> std::rc::Rc<dyn benilla_ui::script::UnitRelations> {
        std::rc::Rc::new(self.snapshot())
    }

    /// The relation functions over the scene.
    pub(crate) fn rel(&self) -> Relations<'_> {
        Relations {
            factions: self.factions.as_ref(),
            reputations: &self.reputations,
            me: Some(self.me),
            roster: Some(&self.roster),
            units: &self.units,
        }
    }
}
