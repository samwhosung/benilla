//! The reference's price discount `0x612b80(vendor, player)`, which the client takes off the two
//! prices it computes from its own DBCs: the repair cost (`0x4faf30`, caller `0x4faf8a`) and the
//! taxi map's fare (`0x4dc3d0`, callers `0x4dc45d` and `0x4dc4a2`). The merchant's goods and the
//! trainer's services are the server's prices, shown as sent.

use crate::net::{ObjectStore, Reputations};

use super::{ring_reaction, Factions};

/// `0x801620`, the f32 0.1 the reputation term loads (`0x612b9d`).
const REPUTATION_TERM: f32 = 0.1;
/// `0x8029c8`, the f32 0.05 each honor step adds (`0x612bcf`, `0x612bd7`).
const HONOR_STEP: f32 = 0.05;
/// `UNIT_FLAG_PVP`, bit 12 of the vendor's `UNIT_FIELD_FLAGS`, which the honor steps need
/// (`0x612bb4 shr ecx,0xc`).
const UNIT_FLAG_PVP: u32 = 0x1000;

/// `0x612b80`: 0.1 when the vendor's reaction to the player is at least 5, Honored (`0x612b98`,
/// signed), then, only at a vendor flagged `UNIT_FLAG_PVP`, 0.05 at an honor rank byte of 6 and
/// 0.05 more at 8 (`0x612bc9`, `0x612bcd`, unsigned), whatever the reaction. The f32 terms are
/// summed in f64 and never narrowed, so 0.15 is no f32.
fn price_discount(reaction: u8, vendor_unit_flags: u32, pvp_rank: u8) -> f64 {
    let mut d = if reaction >= 5 {
        f64::from(REPUTATION_TERM)
    } else {
        0.0
    };
    if vendor_unit_flags & UNIT_FLAG_PVP != 0 && pvp_rank >= 6 {
        d += f64::from(HONOR_STEP);
        if pvp_rank >= 8 {
            d += f64::from(HONOR_STEP);
        }
    }
    d
}

/// [`price_discount`] for `vendor` and the player: the vendor's reaction toward the player
/// ([`ring_reaction`], `vendor->UnitReaction(player)` at `0x612b93`), the vendor's own
/// `UNIT_FIELD_FLAGS` (`[[vendor+0x110]+0xa0]`) and the player's current honor rank,
/// `PLAYER_BYTES_3` byte 3 (`[[player+0xe68]+0x1f]`), not the highest rank held.
pub(crate) fn vendor_price_discount(
    factions: Option<&Factions>,
    reputations: &Reputations,
    vendor: &ObjectStore,
    player: &ObjectStore,
) -> f64 {
    price_discount(
        ring_reaction(factions, reputations, Some(vendor), Some(player)),
        vendor.0.unit_flags(),
        player.0.player_pvp_rank().unwrap_or(0),
    )
}

/// A fixture off the install's `Faction.dbc`: the catalog, a `FactionTemplate` of Stormwind
/// (faction 72, which has a reputation slot) and a reputation table putting a human warrior
/// (`UNIT_FIELD_BYTES_0` = [`HUMAN_WARRIOR`]) at `total` standing with it.
#[cfg(test)]
pub(crate) fn stormwind_fixture(
    chain: &mut benilla_formats::Chain,
    total: i32,
) -> (Factions, u32, Reputations) {
    const STORMWIND: u32 = 72;
    let catalog = benilla_formats::load_faction_catalog(chain).expect("Faction.dbc");
    let template = (1u32..4096)
        .find(|&id| catalog.template(id).is_some_and(|t| t.faction == STORMWIND))
        .expect("a Stormwind template");
    let info = catalog
        .reputation_faction(STORMWIND)
        .expect("Stormwind has a reputation slot");
    let slot = usize::try_from(info.rep_index).expect("a slot index");
    let mut reps = vec![(0u8, 0i32); slot + 1];
    reps[slot].1 = total - info.base_for(1, 1);
    (Factions::from_catalog(catalog), template, Reputations(reps))
}

/// `UNIT_FIELD_BYTES_0` of a human (race 1) warrior (class 1), for [`stormwind_fixture`].
#[cfg(test)]
pub(crate) const HUMAN_WARRIOR: u32 = 1 | (1 << 8);

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::{FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS};
    use benilla_protocol::ObjectFields;

    /// The five values `0x612b80` can return, as the f64 bits its `st0` holds.
    const ZERO: u64 = 0;
    const D05: u64 = 0x3fa9_9999_a000_0000;
    const D10: u64 = 0x3fb9_9999_a000_0000;
    const D15: u64 = 0x3fc3_3333_3800_0000;
    const D20: u64 = 0x3fc9_9999_a000_0000;

    /// Every branch of `0x612b80`: the reputation term at reaction 5 only, the honor steps only at a
    /// PvP-flagged vendor, rank 6 and 8 the thresholds, and a Friendly (4) player still earning them.
    #[test]
    fn the_discount_is_the_references_sum_bit_for_bit() {
        for (reaction, flags, rank, want) in [
            (4u8, 0u32, 0u8, ZERO),
            (4, 0, 5, ZERO),
            (4, 0, 6, ZERO),
            (4, 0, 7, ZERO),
            (4, 0, 8, ZERO),
            (4, 0, 19, ZERO),
            (4, UNIT_FLAG_PVP, 0, ZERO),
            (4, UNIT_FLAG_PVP, 5, ZERO),
            (4, UNIT_FLAG_PVP, 6, D05),
            (4, UNIT_FLAG_PVP, 7, D05),
            (4, UNIT_FLAG_PVP, 8, D10),
            (4, UNIT_FLAG_PVP, 19, D10),
            (5, 0, 0, D10),
            (5, 0, 5, D10),
            (5, 0, 6, D10),
            (5, 0, 7, D10),
            (5, 0, 8, D10),
            (5, 0, 19, D10),
            (5, UNIT_FLAG_PVP, 0, D10),
            (5, UNIT_FLAG_PVP, 5, D10),
            (5, UNIT_FLAG_PVP, 6, D15),
            (5, UNIT_FLAG_PVP, 7, D15),
            (5, UNIT_FLAG_PVP, 8, D20),
            (5, UNIT_FLAG_PVP, 19, D20),
            // Revered and up pass the same signed `cmp eax,5`; Neutral does not.
            (7, UNIT_FLAG_PVP, 19, D20),
            (3, UNIT_FLAG_PVP, 19, D10),
            // Every other flag bit set but the PvP one: no honor step.
            (5, !UNIT_FLAG_PVP, 19, D10),
            // The rank compares are unsigned: a byte past 19 takes both steps.
            (4, UNIT_FLAG_PVP, 255, D10),
        ] {
            let got = price_discount(reaction, flags, rank);
            assert_eq!(
                got.to_bits(),
                want,
                "reaction {reaction}, flags {flags:#x}, rank {rank}: got {got:e}"
            );
        }
    }

    /// The 0.15 case is the f64 sum of the two f32 constants, which no f32 holds: narrowing it would
    /// move the taxi fare.
    #[test]
    fn the_fifteen_percent_discount_is_an_f64() {
        let d = price_discount(5, UNIT_FLAG_PVP, 6);
        assert_eq!(d.to_bits(), (0.1f32 as f64 + 0.05f32 as f64).to_bits());
        assert_ne!(f64::from(d as f32).to_bits(), d.to_bits());
    }

    /// `PLAYER_BYTES_3`, `PLAYER_FIELD_BYTES`: absolute descriptor indices.
    const PLAYER_BYTES_3: u16 = 195;
    const PLAYER_FIELD_BYTES: u16 = 1222;

    /// The flag is read off the vendor and the rank off the player's current-rank byte, never the
    /// highest-rank-held byte of `PLAYER_FIELD_BYTES`. With no faction catalog the reaction is
    /// Neutral, so only the honor steps can show.
    #[test]
    fn the_inputs_are_the_vendors_flag_and_the_players_current_rank() {
        let reps = Reputations::default();
        let store = |pairs: &[(u16, u32)]| ObjectStore(ObjectFields::from_pairs(pairs));
        let pvp_vendor = store(&[(FIELD_UNIT_FLAGS, UNIT_FLAG_PVP)]);
        let plain_vendor = store(&[(FIELD_UNIT_FLAGS, 0)]);
        let rank_8 = store(&[(PLAYER_BYTES_3, 8 << 24), (PLAYER_FIELD_BYTES, 0)]);
        let held_8 = store(&[(PLAYER_BYTES_3, 0), (PLAYER_FIELD_BYTES, 8 << 24)]);
        let flagged_player = store(&[(PLAYER_BYTES_3, 8 << 24), (FIELD_UNIT_FLAGS, UNIT_FLAG_PVP)]);
        let disc =
            |v: &ObjectStore, p: &ObjectStore| vendor_price_discount(None, &reps, v, p).to_bits();
        assert_eq!(disc(&pvp_vendor, &rank_8), D10, "two honor steps");
        assert_eq!(
            disc(&pvp_vendor, &held_8),
            ZERO,
            "the highest rank held is not the byte"
        );
        assert_eq!(
            disc(&plain_vendor, &flagged_player),
            ZERO,
            "the player's own PvP flag is not the vendor's"
        );
    }

    /// `UNIT_FIELD_BYTES_0`, absolute descriptor index.
    const BYTES_0: u16 = 36;

    /// The reputation term reads the vendor's reaction to the player, the standing rank: Friendly
    /// (8999) earns nothing, Honored (9000) the 0.1. The player's reaction to the vendor would be
    /// 4 at both (not at war), so the other direction never discounts.
    #[test]
    fn the_reputation_term_is_the_vendors_reaction_to_the_player() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        for (total, want) in [(8999, ZERO), (9000, D10), (42000, D10)] {
            let (factions, template, reps) = stormwind_fixture(&mut chain, total);
            let vendor = ObjectStore(ObjectFields::from_pairs(&[(
                FIELD_UNIT_FACTIONTEMPLATE,
                template,
            )]));
            let me = ObjectStore(ObjectFields::from_pairs(&[(BYTES_0, HUMAN_WARRIOR)]));
            let got = vendor_price_discount(Some(&factions), &reps, &vendor, &me);
            assert_eq!(got.to_bits(), want, "standing {total}: got {got:e}");
        }
    }
}
