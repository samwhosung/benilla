//! Our own mover's sends: the `MSG_MOVE_*` stream and the acks the server demands of a player
//! mover. Every ack is mandatory: unacked, the server force-resolves the change on a timeout and
//! flags its anticheat, or completes a teleport about 20 s late.

use anyhow::Result;

use crate::messages::{self, opcode, JumpInfo, MoveMode, TransportPose};
use crate::world::movement::{client_uptime_ms, movement_info};

use super::WorldWriter;

impl WorldWriter {
    /// One `MSG_MOVE_*` packet, its opcode picked per movement transition as the reference does.
    /// `flags` may set only bits with a serialized tail: directional, turn and walk bits, and
    /// `JUMPING`, `SWIMMING` and `ON_TRANSPORT` with `jump`, `pitch` and `transport`.
    pub fn send_movement(
        &mut self,
        opcode: u16,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        pitch: f32,
        fall_time: u32,
        jump: Option<JumpInfo>,
        transport: Option<TransportPose>,
    ) -> Result<()> {
        let mut info = movement_info(pos, orientation, flags);
        // The serializer writes each tail only when its flag is set, so flag and value must agree.
        info.pitch = pitch;
        info.fall_time = fall_time;
        info.jump = jump;
        info.transport = transport;
        self.send(opcode, &messages::movement(&info))
    }

    /// `CMSG_SET_ACTIVE_MOVER`, full guid: the client's claim after `SMSG_CLIENT_CONTROL_UPDATE`.
    /// Until it lands, every `MSG_MOVE_*` for that unit is discarded
    /// (`MovementHandler.cpp:291-293`, `Player.cpp:20257-20272`); sent at login for our own body
    /// and again on possession.
    pub fn set_active_mover(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_SET_ACTIVE_MOVER, &messages::full_guid(guid))
    }

    /// `CMSG_FAR_SIGHT`, one `u8`: 1 when the view attaches, 0 when it releases (reference:
    /// `0x5ee290`). It names no object; the server reads its own `PLAYER_FARSIGHT`.
    pub fn far_sight(&mut self, engage: bool) -> Result<()> {
        self.send(opcode::CMSG_FAR_SIGHT, &[u8::from(engage)])
    }

    /// `CMSG_MOVE_NOT_ACTIVE_MOVER`: the released full guid, then its parting pose, which vmangos
    /// re-broadcasts to observers as a stop (`MovementHandler.cpp:955-964`).
    pub fn move_not_active_mover(
        &mut self,
        guid: u64,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        fall_time: u32,
    ) -> Result<()> {
        let mut info = movement_info(pos, orientation, flags);
        info.fall_time = fall_time;
        let mut body = messages::full_guid(guid);
        body.extend_from_slice(&messages::movement(&info));
        self.send(opcode::CMSG_MOVE_NOT_ACTIVE_MOVER, &body)
    }

    /// `CMSG_MOVE_TIME_SKIPPED`: the server adds `lag_ms` to its copy of our movement clock and,
    /// just after we board a transport, re-sends that transport's create update.
    pub fn move_time_skipped(&mut self, guid: u64, lag_ms: u32) -> Result<()> {
        self.send(
            opcode::CMSG_MOVE_TIME_SKIPPED,
            &messages::move_time_skipped(guid, lag_ms),
        )
    }

    /// `CMSG_MOVE_SPLINE_DONE`: owed, at rest, when an `SMSG_MONSTER_MOVE` on our own guid ends.
    /// The server checks `spline_id` against its newest, then relocates us and tells observers.
    pub fn move_spline_done(
        &mut self,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        spline_id: u32,
    ) -> Result<()> {
        let info = movement_info(pos, orientation, flags);
        self.send(
            opcode::CMSG_MOVE_SPLINE_DONE,
            &messages::move_spline_done(&info, spline_id),
        )
    }

    /// `MSG_MOVE_WORLDPORT_ACK`, empty: confirms `SMSG_NEW_WORLD` so the object stream resumes.
    pub fn worldport_ack(&mut self) -> Result<()> {
        self.send(opcode::MSG_MOVE_WORLDPORT_ACK, &[])
    }

    /// `CMSG_FORCE_*_SPEED_CHANGE_ACK`: echoes `counter` and the exact `speed` with our live pose,
    /// which the server relocates us to. Unacked, it force-resolves after ~4 s and flags anticheat.
    pub fn force_speed_change_ack(
        &mut self,
        kind: messages::SpeedKind,
        guid: u64,
        counter: u32,
        speed: f32,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        pitch: f32,
        fall_time: u32,
        jump: Option<JumpInfo>,
        transport: Option<TransportPose>,
    ) -> Result<()> {
        let mut info = movement_info(pos, orientation, flags);
        info.pitch = pitch;
        info.fall_time = fall_time;
        info.jump = jump;
        info.transport = transport;
        self.send(
            kind.ack_opcode(),
            &messages::force_speed_ack(guid, counter, &info, speed),
        )
    }

    /// `MSG_MOVE_TELEPORT_ACK`: completes a near-teleport at once; without it the server finishes
    /// on a ~20 s fallback. The guid is a full 8 bytes: a packed one overruns vmangos's reader.
    pub fn teleport_ack(&mut self, guid: u64, counter: u32) -> Result<()> {
        self.send(
            opcode::MSG_MOVE_TELEPORT_ACK,
            &messages::teleport_ack(guid, counter, client_uptime_ms()),
        )
    }

    /// Ack a granted root, water-walk, feather-fall or hover: full guid, echoed counter and our
    /// pose, plus a trailing `u32 apply` for all but root. Unacked, the server never applies it.
    /// `flags` must carry the mode's bit (a root apply-ack without it is a kick, vmangos
    /// `MovementHandler.cpp:715-722`) and no moving bit beside `MOVEFLAG_ROOT`; turn bits are fine.
    pub fn move_mode_ack(
        &mut self,
        guid: u64,
        counter: u32,
        mode: MoveMode,
        apply: bool,
        flags: u32,
        pose: ([f32; 3], f32),
    ) -> Result<()> {
        let info = movement_info(pose.0, pose.1, flags);
        let trailing = mode.ack_carries_apply().then_some(apply);
        self.send(
            mode.ack_opcode(apply),
            &messages::move_flag_ack(guid, counter, &info, trailing),
        )
    }

    /// `CMSG_MOVE_KNOCK_BACK_ACK`: our pose at launch, with the server's `launch` echoed as the
    /// jump tail. `flags` must carry `MOVEFLAG_JUMPING` and `launch` must match within 0.01, or
    /// vmangos rejects the ack and observers never see the knockback.
    pub fn knock_back_ack(
        &mut self,
        guid: u64,
        counter: u32,
        launch: JumpInfo,
        flags: u32,
        pose: ([f32; 3], f32),
        transport: Option<TransportPose>,
    ) -> Result<()> {
        let mut info = movement_info(pose.0, pose.1, flags);
        info.fall_time = 0;
        info.jump = Some(launch);
        info.transport = transport;
        self.send(
            opcode::CMSG_MOVE_KNOCK_BACK_ACK,
            &messages::knock_back_ack(guid, counter, &info),
        )
    }
}
