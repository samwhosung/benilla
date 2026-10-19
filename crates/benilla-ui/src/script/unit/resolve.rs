//! The unit-token resolver `0x515970`, token to guid, over the state it reads, which the app pushes
//! as [`UnitGuids`]: a base (`player`, `pet`, `target`, `partyN`, `partypetN`, `raidN`,
//! `raidpetN`, `mouseover`, `npc`), then any number of `target` hops, each off the current unit's
//! `UNIT_FIELD_TARGET`. Every compare folds ASCII case (`SStrCmpI 0x64a4c0`, first at `0x5159b4`).

use std::collections::HashMap;

/// What the resolver reads: the guid globals, the group tables and, for each unit a token can
/// reach, whether it is held and whom it targets. A zero guid is nobody.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitGuids {
    /// The local player's guid; 0 without the player object, when `player` and `pet` answer nil
    /// (`0x5159c9`, `0x515a58`).
    pub player: u64,
    /// The pet: `UNIT_FIELD_CHARM`, else `UNIT_FIELD_SUMMON` (`0x515a64`).
    pub pet: u64,
    /// The target global `[0xb4e2d8]`.
    pub target: u64,
    /// The mouseover global `[0xb4e2c8]`, read only while it is a held unit (`0x515bd2`).
    pub mouseover: u64,
    /// The interaction NPC `[0xb4e2d0]`.
    pub npc: u64,
    /// `partyN`'s guid, index N-1 (`0x4e81a0`).
    pub party: [u64; 4],
    /// `partypetN`'s guid, index N-1 (`0x4e81d0`).
    pub party_pets: [u64; 4],
    /// `raidN`'s guid, one per roster row; a row past the end is nobody (`0x491940`).
    pub raid: Vec<u64>,
    /// `raidpetN`'s guid, row for row with [`Self::raid`] (`0x491960`).
    pub raid_pets: Vec<u64>,
    /// Each held unit a token can reach, to its `UNIT_FIELD_TARGET` (0 for none): the object
    /// manager's lookup with the unit typemask (`0x468460`, `ecx = 8`) and the descriptor read a
    /// `target` hop makes (`0x515a0f`-`0x515a25`). A guid missing here is not held.
    pub held: HashMap<u64, u64>,
}

/// A token's base, the resolver's compares in its order (`partypet` before `party`, `raidpet`
/// before `raid`); `npc` is tested apart, as its one full-string compare.
#[derive(Clone, Copy)]
enum Base {
    Player,
    Pet,
    Target,
    PartyPet,
    Party,
    RaidPet,
    Raid,
    Mouseover,
}

/// Each prefix and its base, in the resolver's order; a prefix test compares `strlen(literal)`
/// bytes, so `"playerfoo"` matches `player`.
const PREFIXES: [(&str, Base); 8] = [
    ("player", Base::Player),
    ("pet", Base::Pet),
    ("target", Base::Target),
    ("partypet", Base::PartyPet),
    ("party", Base::Party),
    ("raidpet", Base::RaidPet),
    ("raid", Base::Raid),
    ("mouseover", Base::Mouseover),
];

/// The unit a token starts from, before any `target` hop. A numbered base carries its 0-based
/// row, [`index_of`]'s: a table lookup that fails on the reserved `u32::MAX` of a missing or zero
/// number names nobody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitBase {
    Player,
    Pet,
    Target,
    Mouseover,
    /// The interaction NPC; an exact compare, so it takes no `target` hop (`0x515bec`).
    Npc,
    Party(u32),
    PartyPet(u32),
    Raid(u32),
    RaidPet(u32),
}

/// What `0x515970` makes of a token's text, before it reads any state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitTokenParse {
    /// None of the nine compares match: the reference raises `Unknown unit name` (`0x515c14`).
    Unknown,
    /// Recognised, and names nobody whatever the state: the empty token (`0x51599d`), or text
    /// after the base, and after each hop, that is not `target` (`0x515c2d`).
    Nobody,
    /// A base and the `target` hops after it, each off the current unit's `UNIT_FIELD_TARGET`
    /// (`0x515a1c`-`0x515a25`); the loop has no depth limit.
    Unit { base: UnitBase, hops: usize },
}

/// `_strnicmp(s, lit, strlen(lit)) == 0`: ASCII fold only, bytes of 0x80 and up exact.
fn has_prefix(s: &[u8], lit: &str) -> bool {
    s.len() >= lit.len() && s[..lit.len()].eq_ignore_ascii_case(lit.as_bytes())
}

/// The first prefix the token matches, with its length.
fn base_of(token: &[u8]) -> Option<(Base, usize)> {
    PREFIXES
        .iter()
        .find(|(p, _)| has_prefix(token, p))
        .map(|&(p, b)| (b, p.len()))
}

/// Whether the resolver recognises the token, not whether it names a unit: `"party5"` solo is
/// recognised and answers nil, and only a token none of its nine compares match raises.
pub(crate) fn token_recognised(token: &str) -> bool {
    token.eq_ignore_ascii_case("npc") || base_of(token.as_bytes()).is_some()
}

/// The inline base-10 parse after `party`, `partypet`, `raid` and `raidpet`, then `dec` to a
/// 0-based index (`0x515ab1`-`0x515acc`): digits accumulate with 32-bit wrap, and a missing
/// number leaves 0, whose index `0xffffffff` every bound rejects.
fn index_of(s: &[u8]) -> (u32, usize) {
    let digits = s.iter().take_while(|b| b.is_ascii_digit()).count();
    let n = s[..digits].iter().fold(0u32, |acc, &b| {
        acc.wrapping_mul(10).wrapping_add(u32::from(b - b'0'))
    });
    (n.wrapping_sub(1), digits)
}

/// Read `token`'s text as `0x515970` does, before any state: the base by the compares in their
/// order, a numbered base's row, then `target` hops to the end of the text. `"party1target"` is
/// party1 and one hop, `"targettarget"` the target and one, and `"npctarget"` is none of the nine
/// compares, as `npc` is one full-string compare.
pub fn parse_unit_token(token: &str) -> UnitTokenParse {
    let bytes = token.as_bytes();
    if bytes.is_empty() {
        return UnitTokenParse::Nobody;
    }
    let Some((base, len)) = base_of(bytes) else {
        return if token.eq_ignore_ascii_case("npc") {
            UnitTokenParse::Unit {
                base: UnitBase::Npc,
                hops: 0,
            }
        } else {
            UnitTokenParse::Unknown
        };
    };
    let mut rest = &bytes[len..];
    let mut numbered = |make: fn(u32) -> UnitBase| {
        let (row, digits) = index_of(rest);
        rest = &rest[digits..];
        make(row)
    };
    let base = match base {
        Base::Player => UnitBase::Player,
        Base::Pet => UnitBase::Pet,
        Base::Target => UnitBase::Target,
        Base::Mouseover => UnitBase::Mouseover,
        Base::PartyPet => numbered(UnitBase::PartyPet),
        Base::Party => numbered(UnitBase::Party),
        Base::RaidPet => numbered(UnitBase::RaidPet),
        Base::Raid => numbered(UnitBase::Raid),
    };
    let mut hops = 0;
    while !rest.is_empty() {
        if !has_prefix(rest, "target") {
            return UnitTokenParse::Nobody;
        }
        rest = &rest[6..];
        hops += 1;
    }
    UnitTokenParse::Unit { base, hops }
}

impl UnitGuids {
    /// The guid `token` names, or `None` for nobody; `Err` for a token none of the nine compares
    /// match, where the reference raises `Unknown unit name: %s` (`0x515c14`). An empty token is
    /// nobody (`0x51599d`).
    pub(crate) fn resolve(&self, token: &str) -> Result<Option<u64>, ()> {
        match parse_unit_token(token) {
            UnitTokenParse::Unknown => Err(()),
            UnitTokenParse::Nobody => Ok(None),
            UnitTokenParse::Unit { base, hops } => Ok(self.resolve_unit(base, hops)),
        }
    }

    /// The guid a parsed token names: its base's guid, then each `target` hop. `None` is nobody.
    pub(crate) fn resolve_unit(&self, base: UnitBase, hops: usize) -> Option<u64> {
        // The indexed bases: the table's row for the parsed number.
        let at = |table: &[u64], row: u32| {
            usize::try_from(row)
                .ok()
                .and_then(|row| table.get(row))
                .copied()
                .unwrap_or(0)
        };
        let guid = match base {
            UnitBase::Player => self.player,
            UnitBase::Pet if self.player == 0 => return None,
            UnitBase::Pet => self.pet,
            UnitBase::Target => self.target,
            UnitBase::Npc => self.npc,
            // Deviation: `0x4e81d0` has no bound and reads past its four-entry table, which is
            // stale memory, not a mechanism; past it, nobody.
            UnitBase::PartyPet(row) => at(&self.party_pets, row),
            // Capped at 4 (`0x4e81b0`).
            UnitBase::Party(row) => at(&self.party, row),
            // Capped at the roster count (`0x491940`, `0x491960`).
            UnitBase::RaidPet(row) => at(&self.raid_pets, row),
            UnitBase::Raid(row) => at(&self.raid, row),
            UnitBase::Mouseover if !self.held.contains_key(&self.mouseover) => return None,
            UnitBase::Mouseover => self.mouseover,
        };
        self.follow(guid, hops)
    }

    /// The guids a `target` hop can end on: each held unit's target, when it names one. `held`
    /// holds the bases' units and each unit a hop reaches from them, so a token with a hop names
    /// one of these or nobody, and a per-guid feed over them covers the chains a script can spell.
    /// A guid repeats when two units share a target, and the last hop's unit need not be held: the
    /// lookup that finds its object is the reader's.
    pub(crate) fn chain_ends(&self) -> impl Iterator<Item = u64> + '_ {
        self.held.values().copied().filter(|&guid| guid != 0)
    }

    /// Every token that names a unit now, the base spellings `0x515c50` builds for a guid (it
    /// tests the guid against each global and table in turn and keeps every match, so a unit two
    /// tokens name is fired for twice), taken over all units at once: `player`, `pet`, `target`,
    /// `mouseover`, `npc`, then each `partyN`, `partypetN`, `raidN` and `raidpetN` with a guid.
    /// Hop tokens are not among them: the reference's array holds none.
    pub fn bound_tokens(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut name = |token: String, base: UnitBase| {
            if self.resolve_unit(base, 0).is_some() {
                out.push(token);
            }
        };
        name("player".into(), UnitBase::Player);
        name("pet".into(), UnitBase::Pet);
        name("target".into(), UnitBase::Target);
        name("mouseover".into(), UnitBase::Mouseover);
        name("npc".into(), UnitBase::Npc);
        for row in 0..self.party.len() {
            name(format!("party{}", row + 1), UnitBase::Party(row as u32));
        }
        for row in 0..self.party_pets.len() {
            name(
                format!("partypet{}", row + 1),
                UnitBase::PartyPet(row as u32),
            );
        }
        for row in 0..self.raid.len() {
            name(format!("raid{}", row + 1), UnitBase::Raid(row as u32));
        }
        for row in 0..self.raid_pets.len() {
            name(format!("raidpet{}", row + 1), UnitBase::RaidPet(row as u32));
        }
        out
    }

    /// The tokens naming `guid`, as `0x515c50` lists them for one unit: `player`, `pet`, `target`,
    /// the first of party rows 1 to 4 whose member or pet it is (`0x515d61` stops there), the
    /// first such of raid rows 1 to 40 (`0x515dc4`), then `npc` and `mouseover`, each compared
    /// with its global (`0x515de6`, `0x515e14`). Nothing removes duplicates; nobody and a missing
    /// player give none (`0x515c62`, `0x515c89`).
    pub fn tokens_of(&self, guid: u64) -> Vec<String> {
        let mut out = Vec::new();
        if guid == 0 || self.player == 0 {
            return out;
        }
        for (token, at) in [
            ("player", self.player),
            ("pet", self.pet),
            ("target", self.target),
        ] {
            if at == guid {
                out.push(token.to_string());
            }
        }
        if let Some(row) = (0..4).find(|&i| self.party[i] == guid || self.party_pets[i] == guid) {
            let base = if self.party[row] == guid {
                "party"
            } else {
                "partypet"
            };
            out.push(format!("{base}{}", row + 1));
        }
        let pet_of = |i: usize| self.raid_pets.get(i).copied().unwrap_or(0);
        if let Some(row) =
            (0..self.raid.len().min(40)).find(|&i| self.raid[i] == guid || pet_of(i) == guid)
        {
            let base = if self.raid[row] == guid {
                "raid"
            } else {
                "raidpet"
            };
            out.push(format!("{base}{}", row + 1));
        }
        for (token, at) in [("npc", self.npc), ("mouseover", self.mouseover)] {
            if at == guid {
                out.push(token.to_string());
            }
        }
        out
    }

    /// The base token of the first group row naming `guid`, as a member or its pet, or `None`
    /// off the roster: party rows 1 to 4, then raid rows. These are the units a getter answers
    /// from a record when it holds no object (the party record, then the raid's, `0x496400`, then
    /// the pet record, `0x496420`), so a chain ending on one reads what its own token reads.
    pub(crate) fn roster_token(&self, guid: u64) -> Option<String> {
        if guid == 0 {
            return None;
        }
        let row = |members: &[u64], pets: &[u64], member: &str, pet: &str| {
            members
                .iter()
                .zip(pets.iter().chain(std::iter::repeat(&0)))
                .enumerate()
                .find_map(|(i, (&m, &p))| {
                    if m == guid {
                        Some(format!("{member}{}", i + 1))
                    } else if p == guid {
                        Some(format!("{pet}{}", i + 1))
                    } else {
                        None
                    }
                })
        };
        row(&self.party, &self.party_pets, "party", "partypet")
            .or_else(|| row(&self.raid, &self.raid_pets, "raid", "raidpet"))
    }

    /// Whether `guid` carries the unit-field watchers: `0x51bbb0` registers them while the unit's
    /// token flags are nonzero (`0x60c4a0`), and `0x60c520` sets those for the active player,
    /// its pet, a party member or its pet, a raid member or its pet, and the target. `npc` and
    /// `mouseover` set none.
    pub fn has_field_watchers(&self, guid: u64) -> bool {
        guid != 0
            && (guid == self.player
                || (self.player != 0 && guid == self.pet)
                || guid == self.target
                || self.party.contains(&guid)
                || self.party_pets.contains(&guid)
                || self.raid.contains(&guid)
                || self.raid_pets.contains(&guid))
    }

    /// [`Self::resolve`] with the raise as the Lua error it is.
    pub(crate) fn guid_of(&self, token: &str) -> mlua::Result<Option<u64>> {
        self.resolve(token)
            .map_err(|()| mlua::Error::runtime(format!("Unknown unit name: {token}")))
    }

    /// The `target` loop (`0x5159d3`-`0x515a2c`): nobody stays nobody, and each hop goes to the held
    /// unit's `UNIT_FIELD_TARGET`; a hop off a unit not held is a quiet nobody (`0x515a16`).
    fn follow(&self, mut guid: u64, hops: usize) -> Option<u64> {
        for _ in 0..hops {
            if guid == 0 {
                return None;
            }
            guid = *self.held.get(&guid)?;
        }
        Some(guid).filter(|&g| g != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u64 = 0x10;
    const PET: u64 = 0xF140_0000_0000_0077;
    const MOB: u64 = 0xF130_0000_0000_0001;
    const P1: u64 = 0x21;
    const P1_PET: u64 = 0xF140_0000_0000_0021;
    const R3: u64 = 0x33;
    const FAR: u64 = 0x44;

    /// Us targeting a mob that targets party1, who targets us; party1's pet and raid row 2 held,
    /// `FAR` a roster member out of range. The raid rows are the list's members, then us
    /// (`0x5e6e73`).
    fn guids() -> UnitGuids {
        UnitGuids {
            player: ME,
            pet: PET,
            target: MOB,
            mouseover: MOB,
            npc: MOB,
            party: [P1, FAR, 0, 0],
            party_pets: [P1_PET, 0, 0, 0],
            raid: vec![P1, R3, ME],
            raid_pets: vec![P1_PET, 0, PET],
            held: HashMap::from([
                (ME, MOB),
                (PET, 0),
                (MOB, P1),
                (P1, ME),
                (P1_PET, MOB),
                (R3, 0),
            ]),
        }
    }

    fn at(token: &str) -> Option<u64> {
        guids().resolve(token).expect("recognised")
    }

    #[test]
    fn every_base_resolves_through_its_table() {
        assert_eq!(at("player"), Some(ME));
        assert_eq!(at("pet"), Some(PET));
        assert_eq!(at("target"), Some(MOB));
        assert_eq!(at("mouseover"), Some(MOB));
        assert_eq!(at("npc"), Some(MOB));
        assert_eq!(at("party1"), Some(P1));
        // Not held, but a roster member: the base needs no object.
        assert_eq!(at("party2"), Some(FAR));
        assert_eq!(at("partypet1"), Some(P1_PET));
        assert_eq!(at("raid2"), Some(R3));
        assert_eq!(at("raidpet1"), Some(P1_PET));
    }

    /// The tokens naming a unit, in the fan-out's order: the five globals that hold a guid, then
    /// the group tables row by row. A hop token and a nobody are left out.
    #[test]
    fn bound_tokens_lists_each_base_that_names_a_unit() {
        assert_eq!(
            guids().bound_tokens(),
            [
                "player",
                "pet",
                "target",
                "mouseover",
                "npc",
                "party1",
                "party2",
                "partypet1",
                "raid1",
                "raid2",
                "raid3",
                "raidpet1",
                "raidpet3",
            ]
        );
        // Every listed token resolves to a unit, and the omitted `raidpet2` to nobody.
        for token in guids().bound_tokens() {
            assert!(guids().resolve(&token).unwrap().is_some(), "{token}");
        }
        assert_eq!(guids().resolve("raidpet2"), Ok(None));
        // Nothing seated, nothing named; no mouseover held, no mouseover.
        assert!(UnitGuids::default().bound_tokens().is_empty());
        let unheld = UnitGuids {
            held: HashMap::new(),
            ..guids()
        };
        assert!(!unheld.bound_tokens().contains(&"mouseover".to_string()));
    }

    /// One unit's tokens in `0x515c50`'s order: the three globals, one party row, one raid row,
    /// then `npc` and `mouseover`; a unit two tokens name is listed twice.
    #[test]
    fn tokens_of_lists_one_units_names_in_the_fan_out_order() {
        // The mob is the target, the mouseover and the NPC: target first, npc before mouseover.
        assert_eq!(guids().tokens_of(MOB), ["target", "npc", "mouseover"]);
        // We are the last raid row and in no party table.
        assert_eq!(guids().tokens_of(ME), ["player", "raid3"]);
        assert_eq!(guids().tokens_of(PET), ["pet", "raidpet3"]);
        // Party row 1's pet is also raid row 1's pet: one party row, one raid row.
        assert_eq!(guids().tokens_of(P1_PET), ["partypet1", "raidpet1"]);
        assert_eq!(guids().tokens_of(P1), ["party1", "raid1"]);
        assert!(guids().tokens_of(0).is_empty());
        let no_player = UnitGuids {
            player: 0,
            ..guids()
        };
        assert!(
            no_player.tokens_of(MOB).is_empty(),
            "no active player, no list"
        );
    }

    /// The watchers follow the token flags, which `npc` and `mouseover` do not set.
    #[test]
    fn field_watchers_follow_the_token_flags() {
        for guid in [ME, PET, MOB, P1, FAR, P1_PET, R3] {
            assert!(guids().has_field_watchers(guid), "{guid:#x}");
        }
        let hovered = UnitGuids {
            target: 0,
            ..guids()
        };
        assert!(
            !hovered.has_field_watchers(MOB),
            "the mouseover and the NPC alone register nothing"
        );
        assert!(!guids().has_field_watchers(0));
        // Our pet in no group table carries them by the pet's own flag, and only with a player.
        let alone = UnitGuids {
            player: ME,
            pet: PET,
            ..Default::default()
        };
        assert!(alone.has_field_watchers(PET));
        assert!(!UnitGuids { player: 0, ..alone }.has_field_watchers(PET));
    }

    #[test]
    fn the_compares_fold_ascii_case() {
        assert_eq!(at("PLAYER"), Some(ME));
        assert_eq!(at("Player"), Some(ME));
        assert_eq!(at("PartyPet1"), Some(P1_PET));
        assert_eq!(at("MouseOver"), Some(MOB));
        assert_eq!(at("NPC"), Some(MOB));
        assert_eq!(at("TargetTarget"), Some(P1));
    }

    #[test]
    fn target_is_a_loop_on_every_base_but_npc() {
        assert_eq!(at("targettarget"), Some(P1));
        assert_eq!(at("targettargettarget"), Some(ME));
        assert_eq!(at("targettargettargettarget"), Some(MOB));
        assert_eq!(at("playertarget"), Some(MOB));
        assert_eq!(at("party1target"), Some(ME));
        assert_eq!(at("partypet1target"), Some(MOB));
        assert_eq!(at("raid1target"), Some(ME));
        assert_eq!(at("raid1targettarget"), Some(MOB));
        assert_eq!(at("mouseovertarget"), Some(P1));
        // A hop to nobody ends the chain (`0x515a2e`), as does a hop off a unit not held.
        assert_eq!(at("pettarget"), None);
        assert_eq!(at("party2target"), None);
        assert!(guids().resolve("npctarget").is_err());
    }

    /// The parse reads the text alone: the base by the resolver's compares, a numbered base's
    /// 0-based row, then `target` hops to the end, whatever the depth.
    #[test]
    fn a_token_parses_to_a_base_and_its_hops() {
        use UnitBase::*;
        let unit = |base, hops| UnitTokenParse::Unit { base, hops };
        for (token, want) in [
            ("player", unit(Player, 0)),
            ("pet", unit(Pet, 0)),
            ("target", unit(Target, 0)),
            ("mouseover", unit(Mouseover, 0)),
            ("npc", unit(Npc, 0)),
            ("party1", unit(Party(0), 0)),
            ("partypet4", unit(PartyPet(3), 0)),
            ("raid40", unit(Raid(39), 0)),
            ("raidpet2", unit(RaidPet(1), 0)),
            ("targettarget", unit(Target, 1)),
            ("targettargettargettarget", unit(Target, 3)),
            ("playertarget", unit(Player, 1)),
            ("pettarget", unit(Pet, 1)),
            ("mouseovertarget", unit(Mouseover, 1)),
            ("party1target", unit(Party(0), 1)),
            ("partypet1target", unit(PartyPet(0), 1)),
            ("raid12targettargettarget", unit(Raid(11), 3)),
            ("raidpet3target", unit(RaidPet(2), 1)),
            // A missing or zero number is the row every bound rejects.
            ("party", unit(Party(u32::MAX), 0)),
            ("raid0target", unit(Raid(u32::MAX), 1)),
            // The compares fold case, the hops too.
            ("PARTY1TARGET", unit(Party(0), 1)),
            ("PartyPet2TargetTarget", unit(PartyPet(1), 2)),
        ] {
            assert_eq!(parse_unit_token(token), want, "{token}");
        }
        let deep = format!("player{}", "target".repeat(1000));
        assert_eq!(
            parse_unit_token(&deep),
            unit(Player, 1000),
            "no depth limit"
        );
    }

    /// Text after the base, or after any hop, that is not `target` names nobody; a token none of
    /// the nine compares match is the raise, `npc` being an exact compare with no chain.
    #[test]
    fn a_token_that_is_not_base_then_hops_parses_to_nobody_or_the_raise() {
        for token in [
            "",
            "party1foo",
            "playerfoo",
            "player1",
            "pet1",
            "party1targetfoo",
            "party1targettarge",
            "party1 target",
            "targetfoo",
            "raid3x",
        ] {
            assert_eq!(parse_unit_token(token), UnitTokenParse::Nobody, "{token}");
        }
        for token in ["npctarget", "npc1", "bogus", "tar", "focus", "5", "targe"] {
            assert_eq!(parse_unit_token(token), UnitTokenParse::Unknown, "{token}");
        }
    }

    #[test]
    fn a_recognised_token_naming_nobody_is_quiet() {
        for t in [
            "",
            "party",
            "party0",
            "party5",
            "partyX",
            "raid0",
            "raid4",
            "raidpet2",
            "playerfoo",
            "targetfoo",
            "partypet5",
        ] {
            assert_eq!(guids().resolve(t), Ok(None), "{t}");
        }
        let solo = UnitGuids {
            player: 0,
            pet: PET,
            ..guids()
        };
        assert_eq!(solo.resolve("pet"), Ok(None), "no player object, no pet");
        let gone = UnitGuids {
            held: HashMap::new(),
            ..guids()
        };
        assert_eq!(gone.resolve("mouseover"), Ok(None), "a mouseover not held");
        assert_eq!(
            gone.resolve("target"),
            Ok(Some(MOB)),
            "the target global is not gated"
        );
    }

    #[test]
    fn an_unrecognised_token_is_the_raise() {
        for t in ["bogus", "focus", "none", "5", "npc1", "tar"] {
            assert_eq!(guids().resolve(t), Err(()), "{t}");
        }
    }

    #[test]
    fn the_index_parse_wraps_and_stops_at_the_first_non_digit() {
        assert_eq!(at("party01"), Some(P1));
        assert_eq!(at("raid02"), Some(R3));
        assert_eq!(at("raid02target"), None, "raid2 targets nobody");
        assert_eq!(
            index_of(b"4294967297"),
            (0, 10),
            "2^32 + 1 wraps to 1, index 0"
        );
        assert_eq!(index_of(b"x"), (u32::MAX, 0));
    }

    /// The group row a guid answers from without its object: a party row before a raid row, a
    /// member or its pet, and nothing off the roster.
    #[test]
    fn a_roster_guid_names_its_first_group_row() {
        let g = guids();
        assert_eq!(
            g.roster_token(P1).as_deref(),
            Some("party1"),
            "before raid1"
        );
        assert_eq!(g.roster_token(P1_PET).as_deref(), Some("partypet1"));
        assert_eq!(g.roster_token(FAR).as_deref(), Some("party2"));
        assert_eq!(g.roster_token(R3).as_deref(), Some("raid2"));
        assert_eq!(g.roster_token(PET).as_deref(), Some("raidpet3"));
        assert_eq!(g.roster_token(ME).as_deref(), Some("raid3"));
        assert_eq!(g.roster_token(MOB), None);
        assert_eq!(g.roster_token(0), None, "an empty row is nobody's");
    }
}
