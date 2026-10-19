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
//!   `GroupHandler.cpp:384-391`), from [`send_own_pings`].
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

use benilla_assets::coords::bevy_to_wow;
use benilla_ui::script::UiScript;

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
    script: Option<bevy::ecs::system::NonSendMut<UiScript>>,
    mut ping: ResMut<MinimapPing>,
    player: Res<Player>,
    group: Res<crate::ui_party::GroupState>,
    self_q: Query<&Guid, With<SelfPlayer>>,
    mut seated: Local<crate::ui_script::VmMemo<bool>>,
) {
    let Some(mut script) = script else { return };
    let wow = bevy_to_wow(player.pos);
    script.set_minimap_player(player.active.then_some((wow[0], wow[1])));
    let seated = seated.get(&script);
    if !*seated {
        script.set_minimap_ping_point(ping.world);
        *seated = true;
    }
    // The token (`0x4ee330`): ourselves, else the sender's party slot (`party%d`, `0x4ee412`),
    // else its raid row (`raid%d`, `0x4ee41b`), the party scan first; a sender in neither leaves
    // `player`.
    let self_guid = self_q.iter().next().map(|g| g.0);
    for (world, sender) in std::mem::take(&mut ping.inbound) {
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
}

/// No in-game UI: a member's ping has no player object to land on, so it applies nowhere.
pub(super) fn drop_inbound_pings(mut ping: ResMut<MinimapPing>) {
    ping.inbound.clear();
}

/// Our own `PingLocation` calls, after the tick that made them: each is the process's ping now,
/// and goes on the wire when grouped (`0x4eef15`, `0x4eef26`, `0x4eef72`).
pub(super) fn send_own_pings(
    script: Option<bevy::ecs::system::NonSendMut<UiScript>>,
    mut ping: ResMut<MinimapPing>,
    group: Res<crate::ui_party::GroupState>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else { return };
    for (x, y) in script.take_minimap_pings_sent() {
        ping.world = (x, y);
        if group.in_group {
            let _ = commands.0.send(ClientCommand::MinimapPing { x, y });
        }
    }
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

    /// The two ping systems over a VM with a Minimap and a `MINIMAP_PING` listener, the player at
    /// WoW (100, 200) and in a group.
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
            .add_systems(Update, (drive_minimap_ping, send_own_pings).chain());
        let script = UiScript::new().unwrap();
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
            group.own_raid_row = Some(ME);
            // Subgroup 0 (ours) holds 0xA1; 0xB1 and 0xB2 are in subgroup 1. The rows are the
            // wire's order, then us.
            group.members = vec![member(0xB1, 1), member(0xA1, 0), member(0xB2, 1)];
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
            app.update();
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
}
