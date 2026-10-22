//! Flight-master messages (425-431, 786; vmangos `TaxiHandler.cpp`, `Server/Packets/Taxi.cpp`).
//! Every guid here, both ways, is a plain `u64`, never packed (`ObjectGuid.cpp:174-186`).

use std::io;

use crate::wire::{read_u32_le, read_u64_le, read_u8};

/// The known-node bitmask (vmangos `PlayerTaxi::AppendTaximaskTo`, `PlayerTaxi.cpp:53-65`). Node
/// ids are 1-based: node `id` is bit `(id-1) % 32` of word `(id-1) / 32` (`PlayerTaxi.h:37-49`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TaxiMask(pub [u32; 8]);

impl TaxiMask {
    /// Whether `node_id` is known; 0 and ids past 256 read unknown (1.12 ships 85 nodes).
    pub fn is_known(&self, node_id: u32) -> bool {
        node_id != 0
            && self
                .0
                .get(((node_id - 1) / 32) as usize)
                .is_some_and(|word| word & (1 << ((node_id - 1) % 32)) != 0)
    }
}

/// `SMSG_ACTIVATETAXIREPLY` codes, vmangos `TaxiError` (`Player.h:404-416`). vmangos never sends
/// `NO_VENDOR_NEARBY`, `NOT_VISITED`, `PLAYER_MOVING`, `SAME_NODE` or `NOT_STANDING`
/// (`Player::ActivateTaxiPathTo`, `Player.cpp:18008-18252`).
pub mod taxi_reply {
    pub const OK: u32 = 0;
    pub const UNSPECIFIED_SERVER_ERROR: u32 = 1;
    pub const NO_SUCH_PATH: u32 = 2;
    pub const NOT_ENOUGH_MONEY: u32 = 3;
    pub const TOO_FAR: u32 = 4;
    pub const NO_VENDOR_NEARBY: u32 = 5;
    pub const NOT_VISITED: u32 = 6;
    pub const BUSY: u32 = 7;
    pub const ALREADY_MOUNTED: u32 = 8;
    pub const SHAPESHIFTED: u32 = 9;
    pub const PLAYER_MOVING: u32 = 10;
    pub const SAME_NODE: u32 = 11;
    pub const NOT_STANDING: u32 = 12;
}

/// Body of `CMSG_TAXINODE_STATUS_QUERY`, answered by `SMSG_TAXINODE_STATUS`.
pub fn taxi_node_status_query(flightmaster_guid: u64) -> Vec<u8> {
    flightmaster_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_TAXIQUERYAVAILABLENODES`, sent only for a pure flight master: the 1.12 client's
/// interact order sends a gossip+taxi NPC to gossip. A known node answers `SMSG_SHOWTAXINODES`; an
/// unvisited one is learned, not opened (`SMSG_NEW_TAXI_PATH`, `SendLearnNewTaxiNode`).
pub fn taxi_query_available_nodes(flightmaster_guid: u64) -> Vec<u8> {
    flightmaster_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_ACTIVATETAXI`: a flight along one direct `TaxiPath` edge, `node1` to `node2`,
/// answered by `SMSG_ACTIVATETAXIREPLY`.
pub fn activate_taxi(flightmaster_guid: u64, node1: u32, node2: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&flightmaster_guid.to_le_bytes());
    body.extend_from_slice(&node1.to_le_bytes());
    body.extend_from_slice(&node2.to_le_bytes());
    body
}

/// Body of `CMSG_ACTIVATETAXIEXPRESS`: the whole multi-hop chain, sent when no direct `TaxiPath`
/// edge joins the two nodes (client `0x4dbad0`), however many stops the drawn route has.
pub fn activate_taxi_express(flightmaster_guid: u64, total_cost: u32, nodes: &[u32]) -> Vec<u8> {
    let mut body = Vec::with_capacity(16 + nodes.len() * 4);
    body.extend_from_slice(&flightmaster_guid.to_le_bytes());
    body.extend_from_slice(&total_cost.to_le_bytes());
    body.extend_from_slice(&(nodes.len() as u32).to_le_bytes());
    for &n in nodes {
        body.extend_from_slice(&n.to_le_bytes());
    }
    body
}

/// Read `SMSG_SHOWTAXINODES` (vmangos `TaxiHandler.cpp:82-96`): a `u32` mode, then, only when it
/// is nonzero, the flight master and the nearest node (client `0x5ece97`-`0x5ececa`), then the
/// [`TaxiMask`] either way (`0x5ecee0`-`0x5ecf2e`). vmangos always sends 1.
pub(super) fn read_show_taxi_nodes(r: &mut &[u8]) -> io::Result<(u32, u64, u32, TaxiMask)> {
    let window = read_u32_le(r)?;
    let (flightmaster, nearest_node) = if window == 0 {
        (0, 0)
    } else {
        (read_u64_le(r)?, read_u32_le(r)?)
    };
    let mut mask = [0u32; 8];
    for word in &mut mask {
        *word = read_u32_le(r)?;
    }
    Ok((window, flightmaster, nearest_node, TaxiMask(mask)))
}

/// Read `SMSG_TAXINODE_STATUS` (`Taxi.cpp:36-40`): `(guid, known)`, `known` a one-byte bool.
pub(super) fn read_taxi_node_status(r: &mut &[u8]) -> io::Result<(u64, u8)> {
    Ok((read_u64_le(r)?, read_u8(r)?))
}

/// Read `SMSG_ACTIVATETAXIREPLY` (`Taxi.cpp:46-49`): one [`taxi_reply`] code.
pub(super) fn read_activate_taxi_reply(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}
