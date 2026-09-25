//! Battleground messages: queue status, the port answer, the scoreboard, the instance list and
//! teammate positions. The client keeps three queue slots (`0xb6e9d0`, stride `0x20`).

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_u32_le, read_u8};

/// `SMSG_BATTLEFIELD_STATUS`, one slot's update (handler `0x4aa850`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BattlefieldStatus {
    /// Which of the three slots, 0-based on the wire (1-based in Lua).
    pub slot: u32,
    /// The battleground's Map.dbc row id; zero clears the slot.
    pub map_id: u32,
    pub bracket: u8,
    /// The instance id (`+0x10`), `GetBattlefieldStatus`'s third value.
    pub instance_id: u32,
    pub status: u32,
    /// Status 2: the port deadline in ms from now (`+0x14`, `GetBattlefieldPortExpiration`).
    pub time_ms: Option<u32>,
    /// Status 3: the expiry delta (`[0xb6ebb8] = now + Δ₁`, `GetBattlefieldInstanceExpiration`)
    /// and the run time (`[0xb6ebbc] = now − Δ₂`, `GetBattlefieldInstanceRunTime`).
    pub in_progress: Option<(u32, u32)>,
    /// Status 1: the estimated wait in ms (`[slot+0x18]`, `GetBattlefieldEstimatedWaitTime`)
    /// and the time waited (`[slot+0x1c] = now − Δ`, `GetBattlefieldTimeWaited`).
    pub queued: Option<(u32, u32)>,
}

pub(super) fn read_battlefield_status(r: &mut impl Read) -> io::Result<BattlefieldStatus> {
    let slot = read_u32_le(r)?;
    let map_id = read_u32_le(r)?;
    if map_id == 0 {
        return Ok(BattlefieldStatus {
            slot,
            map_id,
            bracket: 0,
            instance_id: 0,
            status: 0,
            time_ms: None,
            in_progress: None,
            queued: None,
        });
    }
    let bracket = read_u8(r)?;
    let instance_id = read_u32_le(r)?;
    let status = read_u32_le(r)?;
    let time_ms = if status == 2 {
        Some(read_u32_le(r)?)
    } else {
        None
    };
    let in_progress = if status == 3 {
        Some((read_u32_le(r)?, read_u32_le(r)?))
    } else {
        None
    };
    let queued = if status == 1 {
        Some((read_u32_le(r)?, read_u32_le(r)?))
    } else {
        None
    };
    Ok(BattlefieldStatus {
        slot,
        map_id,
        bracket,
        instance_id,
        status,
        time_ms,
        in_progress,
        queued,
    })
}

/// One `MSG_PVP_LOG_DATA` scoreboard row (handler `0x4aab30`), a 0x40-byte block in the client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PvpLogRow {
    pub guid: u64,
    /// Wire field 2, before the kills; stored at `+0x1c`.
    pub rank: u32,
    pub killing_blows: u32,
    pub honorable_kills: u32,
    pub deaths: u32,
    pub honor_gained: u32,
    /// The extra-stat dwords; the client keeps eight and reads past the rest.
    pub stats: Vec<u32>,
}

/// `MSG_PVP_LOG_DATA` inbound: the whole scoreboard, rows in wire order.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PvpLogData {
    /// The battleground has ended; `LeaveBattlefield` sends nothing until it has.
    pub ended: bool,
    /// Present only when ended: 0 Horde, 1 Alliance (`GetBattlefieldWinner`).
    pub winner: Option<u8>,
    pub rows: Vec<PvpLogRow>,
}

/// Deviation: every row is kept, because the reference takes the count unclamped into its 80
/// row blocks and a larger one overruns them.
pub(super) fn read_pvp_log_data(r: &mut impl Read) -> io::Result<PvpLogData> {
    let ended = read_u8(r)? != 0;
    let winner = if ended { Some(read_u8(r)?) } else { None };
    let count = read_u32_le(r)?;
    let mut rows = Vec::with_capacity(capacity_hint(count, 80));
    for _ in 0..count {
        let guid = crate::wire::read_u64_le(r)?;
        let rank = read_u32_le(r)?;
        let killing_blows = read_u32_le(r)?;
        let honorable_kills = read_u32_le(r)?;
        let deaths = read_u32_le(r)?;
        let honor_gained = read_u32_le(r)?;
        let stat_count = read_u32_le(r)?;
        let mut stats = Vec::with_capacity(capacity_hint(stat_count, 8));
        for i in 0..stat_count {
            let v = read_u32_le(r)?;
            if i < 8 {
                stats.push(v);
            }
        }
        rows.push(PvpLogRow {
            guid,
            rank,
            killing_blows,
            honorable_kills,
            deaths,
            honor_gained,
            stats,
        });
    }
    Ok(PvpLogData {
        ended,
        winner,
        rows,
    })
}

/// `CMSG_LEAVE_BATTLEFIELD` (`0x4abe60`): the active slot's map id, or 0 with none active.
pub fn leave_battlefield(map_id: u32) -> Vec<u8> {
    map_id.to_le_bytes().to_vec()
}

/// `SMSG_BATTLEFIELD_LIST` (handler `0x4aa6c0`): the instance list a battlemaster opens.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldList {
    /// 0 when opened without an NPC; `JoinBattlefield` picks the join opcode by it.
    pub battlemaster: u64,
    pub map_id: u32,
    /// The level-bracket index; the client derives the bracket's min/max from it and the map row.
    pub bracket: u8,
    /// The instance ids in wire order; wire index 0 is instance 1 in Lua.
    pub instances: Vec<u32>,
}

pub(super) fn read_battlefield_list(r: &mut impl Read) -> io::Result<BattlefieldList> {
    let battlemaster = crate::wire::read_u64_le(r)?;
    let map_id = read_u32_le(r)?;
    let bracket = read_u8(r)?;
    let count = read_u32_le(r)?;
    let mut instances = Vec::with_capacity(capacity_hint(count, 64));
    for _ in 0..count {
        instances.push(read_u32_le(r)?);
    }
    Ok(BattlefieldList {
        battlemaster,
        map_id,
        bracket,
        instances,
    })
}

/// One teammate's position from `MSG_BATTLEGROUND_PLAYER_POSITIONS`, in raw world coordinates;
/// the client prefers a live object's own position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BattlefieldPosition {
    pub guid: u64,
    pub x: f32,
    pub y: f32,
}

/// `MSG_BATTLEGROUND_PLAYER_POSITIONS` (handler `0x4aad40`): teammates outside our group, then
/// the friendly flag carrier if any.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldPositions {
    pub players: Vec<BattlefieldPosition>,
    pub carrier: Option<BattlefieldPosition>,
}

/// The client's 40-entry store. Deviation: a larger count keeps the first 40, where the
/// reference's handler writes past the store.
pub const BATTLEFIELD_POSITIONS_MAX: usize = 40;

pub(super) fn read_battlefield_positions(r: &mut impl Read) -> io::Result<BattlefieldPositions> {
    let count = read_u32_le(r)?;
    let mut players = Vec::with_capacity(capacity_hint(count, BATTLEFIELD_POSITIONS_MAX));
    for i in 0..count {
        let guid = crate::wire::read_u64_le(r)?;
        let x = crate::wire::read_f32_le(r)?;
        let y = crate::wire::read_f32_le(r)?;
        if (i as usize) < BATTLEFIELD_POSITIONS_MAX {
            players.push(BattlefieldPosition { guid, x, y });
        }
    }
    let carrier = if read_u8(r)? != 0 {
        Some(BattlefieldPosition {
            guid: crate::wire::read_u64_le(r)?,
            x: crate::wire::read_f32_le(r)?,
            y: crate::wire::read_f32_le(r)?,
        })
    } else {
        None
    };
    Ok(BattlefieldPositions { players, carrier })
}

/// `CMSG_BATTLEFIELD_LIST` (`0x4ab8c0`): the queued slot's map id.
pub fn battlefield_list(map_id: u32) -> Vec<u8> {
    map_id.to_le_bytes().to_vec()
}

/// `CMSG_BATTLEMASTER_JOIN` (`0x4a9f60`, with a battlemaster); instance 0 means first available.
pub fn battlemaster_join(
    battlemaster: u64,
    map_id: u32,
    instance_id: u32,
    as_group: bool,
) -> Vec<u8> {
    let mut body = battlemaster.to_le_bytes().to_vec();
    body.extend_from_slice(&map_id.to_le_bytes());
    body.extend_from_slice(&instance_id.to_le_bytes());
    body.push(u8::from(as_group));
    body
}

/// `CMSG_BATTLEFIELD_JOIN` (`0x4a9f60`, no battlemaster); instance 0 means first available.
pub fn battlefield_join(map_id: u32, instance_id: u32, as_group: bool) -> Vec<u8> {
    let mut body = map_id.to_le_bytes().to_vec();
    body.extend_from_slice(&instance_id.to_le_bytes());
    body.push(u8::from(as_group));
    body
}

/// `CMSG_BATTLEFIELD_PORT` (`0x4ab3b0`): the map id, then a one-byte accept, 0 or 1.
pub fn battlefield_port(map_id: u32, accept: bool) -> Vec<u8> {
    let mut body = map_id.to_le_bytes().to_vec();
    body.push(u8::from(accept));
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Status 2, 3 and 1 each read their own tail and no further; a zero map ends the packet.
    #[test]
    fn status_reads_the_conditional_tails() {
        let mut body = vec![
            1u8, 0, 0, 0, 30, 0, 0, 0, 5, 7, 0, 0, 0, 2, 0, 0, 0, 100, 0, 0, 0,
        ];
        let s = read_battlefield_status(&mut body.as_slice()).unwrap();
        assert_eq!(
            (s.slot, s.map_id, s.bracket, s.instance_id, s.status),
            (1, 30, 5, 7, 2)
        );
        assert_eq!(s.time_ms, Some(100));
        assert_eq!((s.in_progress, s.queued), (None, None));

        // The same slot, map, bracket and instance under status 3, then status 1.
        let header = body[..13].to_vec();
        let mut b = header.clone();
        for v in [3u32, 120_000, 45_000] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(0xEE); // a byte past the tail
        let mut r = b.as_slice();
        let s = read_battlefield_status(&mut r).unwrap();
        assert_eq!(s.status, 3);
        assert_eq!(s.in_progress, Some((120_000, 45_000)));
        assert_eq!((s.time_ms, s.queued), (None, None));
        assert_eq!(r, [0xEE], "the status-3 tail is two u32s");

        let mut b = header;
        for v in [1u32, 30_000, 5_000] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(0xEE);
        let mut r = b.as_slice();
        let s = read_battlefield_status(&mut r).unwrap();
        assert_eq!(s.status, 1);
        assert_eq!(s.queued, Some((30_000, 5_000)));
        assert_eq!((s.time_ms, s.in_progress), (None, None));
        assert_eq!(r, [0xEE], "the status-1 tail is two u32s");

        body = vec![0u8, 0, 0, 0, 0, 0, 0, 0];
        let s = read_battlefield_status(&mut body.as_slice()).unwrap();
        assert_eq!(
            s.map_id, 0,
            "a zero map clears the slot and ends the packet"
        );
    }

    #[test]
    fn port_is_a_map_id_and_one_byte() {
        assert_eq!(battlefield_port(489, true), vec![0xE9, 1, 0, 0, 1]);
        assert_eq!(battlefield_port(489, false), vec![0xE9, 1, 0, 0, 0]);
    }
}

#[cfg(test)]
mod pvp_log_tests {
    use super::*;

    #[test]
    fn the_scoreboard_reads_its_conditional_winner_and_keeps_eight_stats() {
        let mut body = vec![1u8, 0u8, 1, 0, 0, 0];
        body.extend_from_slice(&7u64.to_le_bytes());
        for v in [5u32, 3, 9, 2, 120, 9] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        for v in 1u32..=9 {
            body.extend_from_slice(&v.to_le_bytes());
        }
        let d = read_pvp_log_data(&mut body.as_slice()).unwrap();
        assert!(d.ended);
        assert_eq!(d.winner, Some(0));
        assert_eq!(d.rows.len(), 1);
        let r = &d.rows[0];
        assert_eq!(
            (
                r.guid,
                r.rank,
                r.killing_blows,
                r.honorable_kills,
                r.deaths,
                r.honor_gained
            ),
            (7, 5, 3, 9, 2, 120)
        );
        assert_eq!(r.stats, vec![1, 2, 3, 4, 5, 6, 7, 8]);

        let body = vec![0u8, 0, 0, 0, 0];
        let d = read_pvp_log_data(&mut body.as_slice()).unwrap();
        assert!(!d.ended && d.winner.is_none() && d.rows.is_empty());
        assert_eq!(leave_battlefield(489), vec![0xE9, 1, 0, 0]);
    }

    #[test]
    fn the_list_reads_its_guid_bracket_and_instances() {
        let mut body = 0x1234_5678_9abc_def0u64.to_le_bytes().to_vec();
        body.extend_from_slice(&489u32.to_le_bytes());
        body.push(2);
        body.extend_from_slice(&3u32.to_le_bytes());
        for id in [7u32, 3, 11] {
            body.extend_from_slice(&id.to_le_bytes());
        }
        let l = read_battlefield_list(&mut body.as_slice()).unwrap();
        assert_eq!(l.battlemaster, 0x1234_5678_9abc_def0);
        assert_eq!((l.map_id, l.bracket), (489, 2));
        assert_eq!(l.instances, vec![7, 3, 11], "wire order, nothing sorts it");
        let body = [0u8; 8]
            .iter()
            .chain(&30u32.to_le_bytes())
            .chain(&[0u8])
            .chain(&0u32.to_le_bytes())
            .copied()
            .collect::<Vec<_>>();
        let l = read_battlefield_list(&mut body.as_slice()).unwrap();
        assert_eq!(l.battlemaster, 0, "a list opened without an NPC");
        assert!(l.instances.is_empty());
    }

    #[test]
    fn the_positions_reply_reads_the_list_and_the_carrier() {
        let mut body = 2u32.to_le_bytes().to_vec();
        for (g, x, y) in [(0x10u64, 1.5f32, -2.0f32), (0x11, 3.0, 4.0)] {
            body.extend_from_slice(&g.to_le_bytes());
            body.extend_from_slice(&x.to_le_bytes());
            body.extend_from_slice(&y.to_le_bytes());
        }
        body.push(1);
        body.extend_from_slice(&0x20u64.to_le_bytes());
        body.extend_from_slice(&9.0f32.to_le_bytes());
        body.extend_from_slice(&8.0f32.to_le_bytes());
        let p = read_battlefield_positions(&mut body.as_slice()).unwrap();
        assert_eq!(p.players.len(), 2);
        assert_eq!(
            p.players[1],
            BattlefieldPosition {
                guid: 0x11,
                x: 3.0,
                y: 4.0
            }
        );
        assert_eq!(
            p.carrier,
            Some(BattlefieldPosition {
                guid: 0x20,
                x: 9.0,
                y: 8.0
            })
        );
        let body = [0u8, 0, 0, 0, 0];
        let p = read_battlefield_positions(&mut body.as_slice()).unwrap();
        assert!(p.players.is_empty() && p.carrier.is_none());
        let mut body = 41u32.to_le_bytes().to_vec();
        for i in 0..41u64 {
            body.extend_from_slice(&i.to_le_bytes());
            body.extend_from_slice(&[0u8; 8]);
        }
        body.push(0);
        let p = read_battlefield_positions(&mut body.as_slice()).unwrap();
        assert_eq!(
            p.players.len(),
            40,
            "the client's store, not the wire's count"
        );
    }

    #[test]
    fn the_join_bodies_and_the_list_request() {
        assert_eq!(
            battlefield_join(489, 0, true),
            vec![0xE9, 1, 0, 0, 0, 0, 0, 0, 1],
            "first available, as a group"
        );
        let mut want = 0x42u64.to_le_bytes().to_vec();
        want.extend_from_slice(&[0xE9, 1, 0, 0, 5, 0, 0, 0, 0]);
        assert_eq!(battlemaster_join(0x42, 489, 5, false), want);
        assert_eq!(battlefield_list(529), vec![0x11, 2, 0, 0]);
    }

    #[test]
    fn a_queued_status_carries_its_wait_pair() {
        let mut body = vec![0u8, 0, 0, 0];
        body.extend_from_slice(&489u32.to_le_bytes());
        body.push(3);
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&30000u32.to_le_bytes());
        body.extend_from_slice(&5000u32.to_le_bytes());
        let s = read_battlefield_status(&mut body.as_slice()).unwrap();
        assert_eq!(s.status, 1);
        assert_eq!(s.queued, Some((30000, 5000)));
        assert!(s.time_ms.is_none() && s.in_progress.is_none());
    }
}
