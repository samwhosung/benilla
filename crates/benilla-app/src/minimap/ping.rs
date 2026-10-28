//! The minimap ping, engine-owned and pinned to a world point.
//!
//! The world `(x, y)` is the only stored position, in the VM's minimap statics: `PingLocation`
//! sets it and signals `MINIMAP_PING` inside the call, and `GetPingPosition()` recomputes its
//! offset from the player at every call (`benilla_ui`'s `MinimapStatics`), so the marker follows
//! the pan and the zoom with no second copy to fall out of step.
//!
//! - In: the stock `Minimap_OnClick` calls `Minimap:PingLocation(dx, dy)`; the VM turns it into a
//!   world point against the frame's own size and the zoom diameter. A group member's
//!   `MSG_MINIMAP_PING` (raw world floats) applies at its packet ([`on_minimap_ping`]), its
//!   `MINIMAP_PING` on the notice lane.
//! - Out: our own ping goes on the wire when grouped (relayed to the rest of the group only,
//!   `GroupHandler.cpp:384-391`), inside its call ([`MinimapSteps`]).
//!
//! The reference keeps the world point in two statics nothing clears (`0xbc787c`/`0xbc7880`),
//! across a `ReloadUI` too, so [`MinimapPing`] keeps the process's copy, which a new VM is seeded
//! with as it is built. The stock `Minimap.lua` owns everything visible: it shows the
//! `MiniMapPing` `<Model>` (`Interface\MiniMap\Ping\MinimapPing.mdx`, rendered by
//! `crate::ui_models`), holds it 5 s and fades it 0.5 s (`MINIMAPPING_TIMER`,
//! `MINIMAPPING_FADE_TIMER`). `WorldMapPing` is the same model on the world map.
//!
//! Reaching a ping does not clear it: the reference's 10-yd `d² < 100` auto-clear belongs to the
//! `SMSG_GOSSIP_POI` marker (`0x6d99aa`–`0x6d9a4c`).

use bevy::prelude::*;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};

use crate::net::{ClientCommand, Guid, NetCommands, SelfPlayer};
use crate::player::Player;

/// The process's copy of the ping statics, which a new VM inherits.
#[derive(Resource, Default)]
pub(crate) struct MinimapPing {
    /// The last ping's WoW `(x, y)`; the statics start at zero.
    world: (f32, f32),
}

impl MinimapPing {
    /// The stored point, a new VM's seed.
    pub(crate) fn point(&self) -> (f32, f32) {
        self.world
    }

    /// `sender`'s ping at the world point `at` (`0x4ee330`): the point stored (`0x4ee366`,
    /// `0x4ee373`), and the signal that follows, its token resolved against `group` as it stands,
    /// `me` our own guid.
    pub(crate) fn member(
        &mut self,
        at: (f32, f32),
        sender: u64,
        me: Option<u64>,
        group: Option<&crate::ui_party::GroupState>,
    ) -> MemberPing {
        self.world = at;
        MemberPing {
            token: PingToken::of(sender, me, group),
            x: at.0.to_bits(),
            y: at.1.to_bits(),
        }
    }
}

/// The pinger's unit token (`0x4ee330`): ourselves, else the sender's party slot (`party%d`,
/// `0x4ee412`), else its raid row (`raid%d`, `0x4ee41b`), each 1-based, the party scan first; a
/// sender in neither leaves `player`, the buffer's first text (`0x4ee379`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PingToken {
    Player,
    Party(u8),
    Raid(u8),
}

impl PingToken {
    /// The token `sender` resolves to against `group` as it stands, `me` our own guid.
    fn of(sender: u64, me: Option<u64>, group: Option<&crate::ui_party::GroupState>) -> Self {
        if Some(sender) == me {
            return Self::Player;
        }
        let Some(group) = group else {
            return Self::Player;
        };
        if let Some(i) = group.party_slots().position(|m| m.guid == sender) {
            Self::Party(i as u8 + 1)
        } else if let Some(i) = group.raid_rows().position(|g| g == sender) {
            Self::Raid(i as u8 + 1)
        } else {
            Self::Player
        }
    }

    fn text(self) -> String {
        match self {
            Self::Player => "player".to_string(),
            Self::Party(n) => format!("party{n}"),
            Self::Raid(n) => format!("raid{n}"),
        }
    }
}

/// A group member's ping as the notice lane carries it: the token its packet resolved and the
/// world point, the floats by their bits so the notice compares exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MemberPing {
    token: PingToken,
    x: u32,
    y: u32,
}

impl MemberPing {
    /// `MINIMAP_PING` with the token and the point's offset from the player (`0x4ee48d`), and the
    /// VM's statics storing the point; nothing with no player position pushed (`0x4ee35e`).
    pub(crate) fn fire(self, script: &mut benilla_ui::script::UiScript) {
        let world = (f32::from_bits(self.x), f32::from_bits(self.y));
        script.apply_minimap_ping(world, &self.token.text());
    }
}

/// Push the active player's position the ping methods read at the call (`0x4eecdf`, `0x4ef021`):
/// the player object's own, which unit movement writes in frame time. Runs before the script tick.
pub(super) fn feed_minimap_player(
    mut hosted_vm: crate::script_calls::HostedVm,
    player: Res<Player>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else { return };
        let wow = bevy_to_wow(player.pos);
        script.set_minimap_player(player.active.then_some((wow[0], wow[1])));
    })
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

/// A group member's ping (`0x5e4185` → `0x5e41bd` → `0x4ee330`), with the active player held
/// (`0x4ee341`-`0x4ee35e`): the point stored (`0x4ee366`, `0x4ee373`), the pinger's token
/// resolved against the group as the packet finds it ([`MinimapPing::member`]), and `MINIMAP_PING`
/// on the notice lane at the packet's place. Absent in a harness without the notice lane.
fn on_minimap_ping(
    In(ev): In<benilla_protocol::SessionEvent>,
    mut ping: ResMut<MinimapPing>,
    me: Query<&Guid, With<SelfPlayer>>,
    group: Option<Res<crate::ui_party::GroupState>>,
    lane: Option<ResMut<crate::field_watch::NoticeLane>>,
) {
    let benilla_protocol::SessionEvent::MinimapPing { guid, x, y } = ev else {
        return;
    };
    let Some(me) = me.iter().next().map(|g| g.0) else {
        return;
    };
    let signal = ping.member((x, y), guid, Some(me), group.as_deref());
    if let Some(mut lane) = lane {
        lane.write(crate::field_watch::FieldNotice::MinimapPing(signal));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::UiScript;

    /// The player-position push over a VM with a Minimap and a `MINIMAP_PING` listener, the player
    /// at WoW (100, 200), in a group or not; the ping's handler and the notice lane beside it.
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
            .init_resource::<crate::field_watch::NoticeLane>()
            .add_systems(Update, feed_minimap_player);
        register(&mut app);
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

    /// `sender`'s ping through its handler, then each notice it wrote fired on the VM.
    fn pinged(app: &mut App, sender: u64, point: (f32, f32)) {
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::MinimapPing {
                guid: sender,
                x: point.0,
                y: point.1,
            }],
        );
        let notices: Vec<_> = app
            .world_mut()
            .resource_mut::<crate::field_watch::NoticeLane>()
            .drain()
            .collect();
        let mut s = script(app);
        for notice in notices {
            match notice {
                crate::field_watch::FieldNotice::MinimapPing(p) => p.fire(&mut s),
                other => panic!("{other:?}"),
            }
        }
    }

    /// A member's ping stores the point and signals with its token at its packet, an unknown
    /// sender's leaving `player` (`0x4ee330`); the getter answers it at once. With no player
    /// object the handler stores and signals nothing (`0x4ee35e`).
    #[test]
    fn a_members_ping_signals_with_its_token_and_seats_the_statics() {
        let (mut app, _rx) = ping_app(true);
        app.update();
        pinged(&mut app, 0xDEAD, (100.0, 200.0 - 50.0));
        assert_eq!(app.world().resource::<MinimapPing>().point(), (0.0, 0.0));
        assert!(script(&mut app)
            .eval::<Option<String>>("return seen[1]")
            .unwrap()
            .is_none());
        app.world_mut().spawn((Guid(0x10), SelfPlayer));
        pinged(&mut app, 0xDEAD, (100.0, 200.0 - 50.0));
        assert_eq!(
            app.world().resource::<MinimapPing>().point(),
            (100.0, 150.0)
        );
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
        app.update();
        for (sender, want) in [
            (0xA1, "party1"),
            (0xB2, "raid3"),
            (0xB1, "raid1"),
            (ME, "player"),
            (0xDEAD, "player"),
        ] {
            pinged(&mut app, sender, (100.0, 150.0));
            let s = script(&mut app);
            assert_eq!(
                s.eval::<String>("return seen[table.getn(seen)]").unwrap(),
                want,
                "{sender:#x}"
            );
        }
    }

    /// Our own ping goes on the wire when grouped and only then, and becomes the process's copy,
    /// which a new VM is seeded with as the reference's statics outlive a `ReloadUI`.
    #[test]
    fn our_ping_is_sent_when_grouped_and_becomes_the_process_copy() {
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
            let (x, y) = app.world().resource::<MinimapPing>().point();
            assert!((x - want.0).abs() < 1e-3 && (y - want.1).abs() < 1e-3);
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
