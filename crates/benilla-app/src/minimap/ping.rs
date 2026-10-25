//! The minimap ping, engine-owned and pinned to a world point.
//!
//! The world `(x, y)` is the only stored position, in the VM's minimap statics: `PingLocation`
//! sets it and signals `MINIMAP_PING` inside the call, and `GetPingPosition()` recomputes its
//! offset from the player at every call (`benilla_ui`'s `MinimapStatics`), so the marker follows
//! the pan and the zoom with no second copy to fall out of step.
//!
//! - In: the stock `Minimap_OnClick` calls `Minimap:PingLocation(dx, dy)`; the VM turns it into a
//!   world point against the frame's own size and the zoom diameter. A group member's
//!   `MSG_MINIMAP_PING` (raw world floats) applies through [`drive_minimap_ping`].
//! - Out: our own ping goes on the wire when grouped (relayed to the rest of the group only,
//!   `GroupHandler.cpp:384-391`), inside its call ([`MinimapSteps`]).
//!
//! The reference keeps the world point in two statics nothing clears (`0xbc787c`/`0xbc7880`),
//! across a `ReloadUI` too, so [`MinimapPing`] keeps the process's copy for each new VM. The stock
//! `Minimap.lua` owns everything visible: it shows the `MiniMapPing` `<Model>`
//! (`Interface\MiniMap\Ping\MinimapPing.mdx`, rendered by `crate::ui_models`), holds it 5 s and
//! fades it 0.5 s (`MINIMAPPING_TIMER`, `MINIMAPPING_FADE_TIMER`). `WorldMapPing` is the same model
//! on the world map.
//!
//! Reaching a ping does not clear it: the reference's 10-yd `d² < 100` auto-clear belongs to the
//! `SMSG_GOSSIP_POI` marker (`0x6d99aa`–`0x6d9a4c`).

use bevy::prelude::*;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};

use crate::net::{ClientCommand, Guid, NetCommands, SelfPlayer};
use crate::player::Player;

/// The process's copy of the ping statics, and the members' pings not yet applied to a VM.
#[derive(Resource, Default)]
pub(crate) struct MinimapPing {
    /// The last ping's WoW `(x, y)`, which a new VM inherits; the statics start at zero.
    world: (f32, f32),
    /// Group members' pings since the last [`drive_minimap_ping`], with each sender's guid.
    inbound: Vec<((f32, f32), u64)>,
}

impl MinimapPing {
    /// A group member's ping, stored now and applied to the VM by the next [`drive_minimap_ping`].
    pub(crate) fn seat(&mut self, world: (f32, f32), sender: u64) {
        self.world = world;
        self.inbound.push((world, sender));
    }
}

/// Push the player's position, seat a new VM's stored ping, and apply the members' pings, each
/// signalling `MINIMAP_PING` with its sender's unit token. Runs before the script tick.
pub(super) fn drive_minimap_ping(
    // The ping, which the host's `PingLocation` writes too: taken before the VM runs.
    mut hosted: crate::script_calls::disjoint::Pair<
        ResMut<'static, MinimapPing>,
        crate::script_calls::HostedVm<'static, 'static>,
    >,
    player: Res<Player>,
    group: Res<crate::ui_party::GroupState>,
    self_q: Query<&Guid, With<SelfPlayer>>,
    mut seated: Local<crate::ui_script::VmMemo<bool>>,
) {
    let (stored, inbound) = {
        let mut ping = hosted.p0();
        (ping.world, std::mem::take(&mut ping.inbound))
    };
    hosted.p1().run(|script| {
        let Some(script) = script else { return };
        let wow = bevy_to_wow(player.pos);
        script.set_minimap_player(player.active.then_some((wow[0], wow[1])));
        let seated = seated.get(script);
        if !*seated {
            script.set_minimap_ping_point(stored);
            *seated = true;
        }
        // The token (`0x4ee330`): ourselves, else the sender's party slot (`party%d`, `0x4ee412`),
        // else its raid row (`raid%d`, `0x4ee41b`), the party scan first; a sender in neither
        // leaves `player`.
        let self_guid = self_q.iter().next().map(|g| g.0);
        for (world, sender) in inbound {
            let token = if Some(sender) == self_guid {
                "player".to_string()
            } else if let Some(i) = group.party_slots().position(|m| m.guid == sender) {
                format!("party{}", i + 1)
            } else if let Some(i) = group.raid_rows().position(|g| g == sender) {
                format!("raid{}", i + 1)
            } else {
                "player".to_string()
            };
            script.apply_minimap_ping(world, &token);
        }
    })
}

/// No in-game UI: a member's ping has no player object to land on, so it applies nowhere.
pub(super) fn drop_inbound_pings(mut ping: ResMut<MinimapPing>) {
    ping.inbound.clear();
}

/// Our own `PingLocation` inside its call ([`benilla_ui::script::MinimapCall`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MinimapSteps<'w> {
    /// Absent in a harness without the minimap.
    ping: Option<ResMut<'w, MinimapPing>>,
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    commands: Res<'w, NetCommands>,
    /// The streamed terrain the ground cast's trace reads.
    terrain: benilla_world::world_point::TerrainHeights<'w>,
}

impl MinimapSteps<'_> {
    /// The ground cast's point under the world point `(x, y)`: the terrain-only trace down the
    /// column from z 100000 to -100000 (`0x672170` with flags `0x100`, `0x4eee91`), whose hit is
    /// the heightfield's height there, raised by `[0x807b10]` (`0x4eeee9`). `None` is a miss: no
    /// tile streamed there, or a hole.
    pub(crate) fn ground_under(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        let z = self.terrain.under(wow_to_bevy([x, y, 0.0]))?;
        Some(ground_cast_point(x, y, z))
    }

    /// The ping at the world point `(x, y)`: the process's ping now, and on the wire when grouped
    /// (`0x4eef15`, `0x4eef26`, `0x4eef72`), before the VM applies it (`0x4eefb5`).
    pub(crate) fn ping(&mut self, x: f32, y: f32) {
        if let Some(ping) = self.ping.as_deref_mut() {
            ping.world = (x, y);
        }
        if self.group.as_deref().is_some_and(|g| g.in_group) {
            let _ = self.commands.0.send(ClientCommand::MinimapPing { x, y });
        }
    }
}

/// `[0x807b10]`, the lift `PingLocation` puts on the trace's hit (`0x4eeee9`).
const GROUND_CAST_LIFT: f32 = 8.0;

/// The bound point of a ground cast whose trace hit the terrain at height `z`.
fn ground_cast_point(x: f32, y: f32, z: f32) -> [f32; 3] {
    [x, y, z + GROUND_CAST_LIFT]
}

/// Register the ping's packet handler.
pub(super) fn register(app: &mut App) {
    use crate::net::NetHandlerApp;
    app.net_handler(
        benilla_protocol::SessionEventKind::MinimapPing,
        on_minimap_ping,
    );
}

/// A group member's ping: raw world floats, stored now and announced by the next drive.
fn on_minimap_ping(In(ev): In<benilla_protocol::SessionEvent>, mut ping: ResMut<MinimapPing>) {
    if let benilla_protocol::SessionEvent::MinimapPing { guid, x, y } = ev {
        ping.seat((x, y), guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::UiScript;

    /// The drive over a VM with a Minimap and a `MINIMAP_PING` listener, the player at WoW
    /// (100, 200) and in a group.
    fn ping_app(in_group: bool) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let mut app = App::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        let group = crate::ui_party::GroupState {
            in_group,
            ..Default::default()
        };
        let mut player = Player::default();
        player.active = true;
        player.pos = benilla_assets::coords::wow_to_bevy([100.0, 200.0, 0.0]);
        app.insert_resource(NetCommands(tx))
            .insert_resource(group)
            .insert_resource(player)
            .init_resource::<MinimapPing>()
            .add_systems(Update, drive_minimap_ping);
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
        script
            .run(
                r#"m = CreateFrame("Minimap", "Minimap") m:SetWidth(140) m:SetHeight(140)
                   seen = {}
                   local f = CreateFrame("Frame")
                   f:RegisterEvent("MINIMAP_PING")
                   f:SetScript("OnEvent", function() table.insert(seen, arg1) end)"#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    fn script(app: &mut App) -> Mut<'_, UiScript> {
        app.world_mut().non_send_resource_mut::<UiScript>()
    }

    /// A member's ping applies to the VM with its token at the next drive, and an unknown
    /// sender's leaves `player` (`0x4ee330`); the getter answers it at once.
    #[test]
    fn a_members_ping_signals_with_its_token_and_seats_the_statics() {
        let (mut app, _rx) = ping_app(true);
        app.world_mut()
            .resource_mut::<MinimapPing>()
            .seat((100.0, 200.0 - 50.0), 0xDEAD);
        app.update();
        let s = script(&mut app);
        assert_eq!(s.eval::<String>("return seen[1]").unwrap(), "player");
        let (x, y) = s.eval::<(f64, f64)>("return m:GetPingPosition()").unwrap();
        let d = 2.0 * f64::from(benilla_ui::widget::minimap_radius_yd(3, 3, false));
        assert!((x - 50.0 / d).abs() < 1e-6 && y.abs() < 1e-9, "{x} {y}");
    }

    /// The token is the party scan's `party%d` first, then the raid scan's `raid%d` (`0x4ee412`,
    /// `0x4ee41b`), each 1-based; ourselves, and a sender in neither, leave `player`.
    #[test]
    fn a_pingers_token_is_its_party_slot_else_its_raid_row() {
        use benilla_protocol::messages::GroupMemberEntry;
        let (mut app, _rx) = ping_app(true);
        const ME: u64 = 0x10;
        app.world_mut().spawn((Guid(ME), SelfPlayer));
        let member = |guid: u64, flags: u8| GroupMemberEntry {
            name: format!("m{guid:x}"),
            guid,
            status: 1,
            flags,
        };
        {
            let mut group = app
                .world_mut()
                .resource_mut::<crate::ui_party::GroupState>();
            group.group_type = crate::ui_party::GROUPTYPE_RAID;
            group.own_flags = 0;
            // Subgroup 0 (ours) holds 0xA1; 0xB1 and 0xB2 are in subgroup 1. The rows are the
            // wire's order, then us.
            group.members = vec![member(0xB1, 1), member(0xA1, 0), member(0xB2, 1)];
            group.rebuild_raid(ME);
        }
        for (sender, want) in [
            (0xA1, "party1"),
            (0xB2, "raid3"),
            (0xB1, "raid1"),
            (ME, "player"),
            (0xDEAD, "player"),
        ] {
            app.world_mut()
                .resource_mut::<MinimapPing>()
                .seat((100.0, 150.0), sender);
            app.update();
            let s = script(&mut app);
            assert_eq!(
                s.eval::<String>("return seen[table.getn(seen)]").unwrap(),
                want,
                "{sender:#x}"
            );
        }
    }

    /// Our own ping goes on the wire when grouped and only then, and becomes the process's copy,
    /// which a new VM inherits as the reference's statics outlive a `ReloadUI`.
    #[test]
    fn our_ping_is_sent_when_grouped_and_outlives_the_vm() {
        for grouped in [true, false] {
            let (mut app, rx) = ping_app(grouped);
            app.update();
            script(&mut app).run("m:PingLocation(0, 70)").unwrap();
            // The call as the host applies it.
            let pings = script(&mut app).take_minimap_pings_sent();
            let mut steps = bevy::ecs::system::SystemState::<MinimapSteps>::new(app.world_mut());
            for (x, y) in pings {
                steps.get_mut(app.world_mut()).ping(x, y);
            }
            let sent: Vec<_> = rx.try_iter().collect();
            assert_eq!(sent.len(), usize::from(grouped), "grouped: {grouped}");
            let d = 2.0 * benilla_ui::widget::minimap_radius_yd(3, 3, false);
            let want = (100.0 + 0.5 * d, 200.0);
            if let Some(ClientCommand::MinimapPing { x, y }) = sent.first() {
                assert!((x - want.0).abs() < 1e-3 && (y - want.1).abs() < 1e-3);
            }

            // A fresh VM: the drive seats the stored point before its first tick.
            let fresh = UiScript::new().unwrap();
            fresh
                .run(r#"m = CreateFrame("Minimap", "Minimap")"#)
                .unwrap();
            app.insert_non_send_resource(fresh);
            app.update();
            let (x, _) = script(&mut app)
                .eval::<(f64, f64)>("return m:GetPingPosition()")
                .unwrap();
            assert!(x.abs() < 1e-6, "the inherited point, due north: {x}");
            let (_, y) = script(&mut app)
                .eval::<(f64, f64)>("return m:GetPingPosition()")
                .unwrap();
            assert!((y - 0.5).abs() < 1e-6, "{y}");
        }
    }

    /// A trace's hit binds 8 above the ground (`0x4eeee9`) as the waiting spell's DEST (`0x40`,
    /// `0x6e612c`), and the bind empties the word and sends (`0x6e6177`).
    #[test]
    fn a_ground_hit_binds_eight_above_the_terrain_as_the_spells_location() {
        const BLIZZARD: u32 = 10;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.world_mut()
            .resource_mut::<crate::spell::targeting::SpellTargeting>()
            .enter(BLIZZARD, crate::spell::CastCommit::Spell, 0x40);
        let mut cursor =
            bevy::ecs::system::SystemState::<crate::spell::ScriptCursor>::new(app.world_mut());
        cursor
            .get_mut(app.world_mut())
            .bind_location(ground_cast_point(100.0, 200.0, 30.5));
        cursor.apply(app.world_mut());
        let sent: Vec<_> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpellAtDest { spell_id, dest } => Some((spell_id, dest)),
                _ => None,
            })
            .collect();
        assert_eq!(sent, [(BLIZZARD, [100.0, 200.0, 38.5])]);
        assert_eq!(
            app.world()
                .resource::<crate::spell::targeting::SpellTargeting>()
                .spell(),
            None,
            "the bind ends the mode"
        );
    }
}
