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
        let (base, hops) = match parse_unit_token(token) {
            UnitTokenParse::Unknown => return Err(()),
            UnitTokenParse::Nobody => return Ok(None),
            UnitTokenParse::Unit { base, hops } => (base, hops),
        };
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
            UnitBase::Pet if self.player == 0 => return Ok(None),
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
            UnitBase::Mouseover if !self.held.contains_key(&self.mouseover) => return Ok(None),
            UnitBase::Mouseover => self.mouseover,
        };
        Ok(self.follow(guid, hops))
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

    /// Us targeting a mob that targets party1, who targets us; party1's pet and raid row 3 held,
    /// `FAR` a roster member out of range.
    fn guids() -> UnitGuids {
        UnitGuids {
            player: ME,
            pet: PET,
            target: MOB,
            mouseover: MOB,
            npc: MOB,
            party: [P1, FAR, 0, 0],
            party_pets: [P1_PET, 0, 0, 0],
            raid: vec![ME, P1, R3],
            raid_pets: vec![PET, P1_PET, 0],
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
        assert_eq!(at("raid3"), Some(R3));
        assert_eq!(at("raidpet2"), Some(P1_PET));
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
        assert_eq!(at("raid2target"), Some(ME));
        assert_eq!(at("raid2targettarget"), Some(MOB));
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
            "raidpet3",
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
        assert_eq!(at("raid03"), Some(R3));
        assert_eq!(at("raid03target"), None, "raid3 targets nobody");
        assert_eq!(
            index_of(b"4294967297"),
            (0, 10),
            "2^32 + 1 wraps to 1, index 0"
        );
        assert_eq!(index_of(b"x"), (u32::MAX, 0));
    }
}
