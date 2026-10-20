//! The 1.12.1 guid bit layout (vmangos `ObjectGuid.h`): `[high:16][entry:24][counter:24]` for
//! the families that carry a template entry, `[high:16][counter:48]` (32 bits used) for the rest.

/// `HIGHGUID_PLAYER`.
pub const HIGH_PLAYER: u16 = 0x0000;
/// `HIGHGUID_ITEM`, which is also `HIGHGUID_CONTAINER`: a bag is an item.
pub const HIGH_ITEM: u16 = 0x4000;
/// `HIGHGUID_GAMEOBJECT`.
pub const HIGH_GAMEOBJECT: u16 = 0xF110;
/// `HIGHGUID_UNIT`: a spawned creature.
pub const HIGH_UNIT: u16 = 0xF130;
/// `HIGHGUID_PET`.
pub const HIGH_PET: u16 = 0xF140;
/// `HIGHGUID_MO_TRANSPORT` (`ObjectGuid.h:77`): a boat or zeppelin, gameobject type 15.
pub const HIGH_MO_TRANSPORT: u16 = 0x1FC0;
/// `HIGHGUID_TRANSPORT` (`ObjectGuid.h:72`): an elevator, gameobject type 11.
pub const HIGH_TRANSPORT: u16 = 0xF120;

/// The high 16 bits, the object-family tag (`GetHigh`).
pub fn high(guid: u64) -> u16 {
    ((guid >> 48) & 0xFFFF) as u16
}

/// A non-zero player-character guid.
pub fn is_player(guid: u64) -> bool {
    guid != 0 && high(guid) == HIGH_PLAYER
}

/// A creature or pet guid (`IsCreatureOrPet`); only a creature's carries a template entry.
pub fn is_creature_or_pet(guid: u64) -> bool {
    matches!(high(guid), HIGH_UNIT | HIGH_PET)
}

/// A pet guid (`IsPet`), whose middle field is a pet number, not a template entry.
pub fn is_pet(guid: u64) -> bool {
    high(guid) == HIGH_PET
}

/// The pet number in bits 24-47, fed in as `_Create`'s entry (`Objects/Pet.cpp:2250`): the key
/// of `CMSG_PET_NAME_QUERY`, answered only when it matches `CharmInfo::GetPetNumber()`.
pub fn pet_number(guid: u64) -> Option<u32> {
    is_pet(guid).then_some(((guid >> 24) & 0xFF_FFFF) as u32)
}

/// A non-zero item or container guid (`IsItem`); the entry is only in `OBJECT_FIELD_ENTRY`.
pub fn is_item(guid: u64) -> bool {
    guid != 0 && high(guid) == HIGH_ITEM
}

/// Either transport family: a boat or zeppelin, or an elevator.
pub fn is_transport(guid: u64) -> bool {
    matches!(high(guid), HIGH_MO_TRANSPORT | HIGH_TRANSPORT)
}

pub fn is_gameobject(guid: u64) -> bool {
    high(guid) == HIGH_GAMEOBJECT
}

/// Whether an update packet's first pass handles this guid: its top four bits are 6 or 8
/// (`0x4660d8`-`0x4660f1`, the out-of-range list's `0x465f10`-`0x465f28`). The header byte runs
/// that pass ahead of every other block (`0x46520e`); vmangos's high words (`ObjectGuid.h:68-77`)
/// never carry either nibble.
pub fn first_pass(guid: u64) -> bool {
    matches!(guid >> 60, 6 | 8)
}

/// The template entry: bits 24-47 for creatures, gameobjects and elevators (`HasEntry`,
/// `ObjectGuid.h:223-240`; `GameObject.cpp:207`), the full low 32 bits for boats and zeppelins,
/// whose template entry is passed as the counter (`Transport.cpp:65`, `ObjectGuid.h:123`). `None`
/// for pets although `HasEntry` is true: their slot holds the pet number.
pub fn entry(guid: u64) -> Option<u32> {
    match high(guid) {
        HIGH_MO_TRANSPORT => Some((guid & 0xFFFF_FFFF) as u32),
        HIGH_UNIT | HIGH_GAMEOBJECT | HIGH_TRANSPORT => Some(((guid >> 24) & 0xFF_FFFF) as u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Composes a guid as the vmangos ctor does (`ObjectGuid.h:123`).
    fn compose(high: u16, entry: u32, counter: u32) -> u64 {
        u64::from(counter) | (u64::from(entry) << 24) | (u64::from(high) << 48)
    }

    #[test]
    fn creature_guid_decodes_high_and_entry() {
        let g = compose(HIGH_UNIT, 69, 12345);
        assert_eq!(high(g), HIGH_UNIT);
        assert!(is_creature_or_pet(g));
        assert!(!is_player(g));
        assert_eq!(entry(g), Some(69));
    }

    #[test]
    fn player_guid_has_no_entry() {
        let g = compose(HIGH_PLAYER, 0, 7);
        assert!(is_player(g));
        assert!(!is_creature_or_pet(g));
        assert_eq!(entry(g), None);
    }

    #[test]
    fn zero_guid_is_nothing() {
        assert!(!is_player(0));
        assert!(!is_creature_or_pet(0));
        assert!(!is_item(0));
        assert_eq!(entry(0), None);
    }

    #[test]
    fn item_guid_is_item_and_has_no_entry() {
        let g = compose(HIGH_ITEM, 0, 42);
        assert!(is_item(g));
        assert!(!is_player(g));
        assert_eq!(
            entry(g),
            None,
            "an item's entry is a descriptor field, not guid bits"
        );
    }

    #[test]
    fn entry_masks_to_24_bits() {
        let g = compose(HIGH_UNIT, 0xFF_FFFF, 0xFF_FFFF);
        assert_eq!(entry(g), Some(0xFF_FFFF));
    }

    #[test]
    fn a_pet_carries_a_pet_number_not_a_template_entry() {
        let g = compose(HIGH_PET, 137, 4242);
        assert!(is_pet(g));
        assert!(is_creature_or_pet(g));
        assert_eq!(pet_number(g), Some(137));
        assert_eq!(entry(g), None, "a pet's slot is not a template entry");
        assert_eq!(pet_number(compose(HIGH_UNIT, 137, 4242)), None);
    }

    #[test]
    fn elevator_guid_decodes_high_and_entry() {
        let g = compose(HIGH_TRANSPORT, 900, 4242);
        assert_eq!(high(g), HIGH_TRANSPORT);
        assert!(is_transport(g));
        assert_eq!(entry(g), Some(900));
    }

    #[test]
    fn mo_transport_guid_decodes_high_and_entry() {
        // The Grom'Gol-Undercity zeppelin's template entry, which the ctor takes as the counter.
        let g = compose(HIGH_MO_TRANSPORT, 0, 176_495);
        assert_eq!(high(g), HIGH_MO_TRANSPORT);
        assert!(is_transport(g));
        assert_eq!(
            entry(g),
            Some(176_495),
            "the template entry rides the full low 32 bits, not bits 24-47"
        );
        // The 24-bit entry slot does not hold it: the two layouts really differ.
        assert_ne!(((g >> 24) & 0xFF_FFFF) as u32, 176_495);
    }

    #[test]
    fn is_transport_excludes_other_families() {
        assert!(!is_transport(compose(HIGH_UNIT, 69, 1)));
        assert!(!is_transport(compose(HIGH_GAMEOBJECT, 1, 1)));
        assert!(!is_transport(0));
    }
}
