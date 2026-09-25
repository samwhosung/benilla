//! The innkeeper bind: `SMSG_BINDER_CONFIRM` (0x2eb) asks, `CMSG_BINDER_ACTIVATE` (0x1b5)
//! answers, `SMSG_PLAYERBOUND` (0x158) confirms. The gossip line only asks (`Player.cpp:12417`);
//! the bind happens on the answer, when the innkeeper casts spell 3286 (`NPCHandler.cpp:479`).

use std::io;

use crate::wire::{read_u32_le, read_u64_le};

/// `SMSG_PLAYERBOUND`, sent once the bind has taken (`SpellEffects.cpp:5806`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerBound {
    /// The caster of spell 3286, the innkeeper.
    pub binder: u64,
    /// The `AreaTable` id now bound, the same one `SMSG_BINDPOINTUPDATE` carries beside it.
    pub area: u32,
}

/// `CMSG_BINDER_ACTIVATE`: the guid from [`read_binder_confirm`]. The server binds only if it is
/// an innkeeper in interact range, and says nothing otherwise.
pub fn binder_activate(binder_guid: u64) -> Vec<u8> {
    binder_guid.to_le_bytes().to_vec()
}

/// `SMSG_BINDER_CONFIRM` (vmangos `Npc.cpp`): the innkeeper's guid. `SMSG_GOSSIP_COMPLETE` comes
/// just before it, so the question outlives the gossip window.
pub(super) fn read_binder_confirm(r: &mut &[u8]) -> io::Result<u64> {
    read_u64_le(r)
}

/// `SMSG_PLAYERBOUND` (vmangos `Misc.cpp`).
pub(super) fn read_player_bound(r: &mut &[u8]) -> io::Result<PlayerBound> {
    Ok(PlayerBound {
        binder: read_u64_le(r)?,
        area: read_u32_le(r)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_activate_body_is_one_little_endian_guid() {
        assert_eq!(
            binder_activate(0x0000_0000_0000_2a01),
            vec![0x01, 0x2a, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn the_two_inbound_bodies_round_trip() {
        let guid: u64 = 0xF130_0000_0001_2345;
        let mut bytes = guid.to_le_bytes().to_vec();
        assert_eq!(read_binder_confirm(&mut &bytes[..]).unwrap(), guid);

        bytes.extend_from_slice(&141u32.to_le_bytes());
        assert_eq!(
            read_player_bound(&mut &bytes[..]).unwrap(),
            PlayerBound {
                binder: guid,
                area: 141
            }
        );
    }
}
