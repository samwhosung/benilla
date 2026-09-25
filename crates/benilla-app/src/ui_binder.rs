//! The innkeeper's bind question. Selecting the gossip line binds nothing: the server closes the
//! gossip and asks (`SMSG_BINDER_CONFIRM`, vmangos `Player::SetBindPoint`), and the bind happens
//! only on `CMSG_BINDER_ACTIVATE`, when the innkeeper casts spell 3286 and `SPELL_EFFECT_BIND`
//! sends `SMSG_BINDPOINTUPDATE` and `SMSG_PLAYERBOUND`.
//!
//! The stock surface (`StaticPopup.lua:1321-1335`, `UIParent.lua:125`, `547-548`) is the
//! `CONFIRM_BINDER` event, whose one argument fills the dialog's `%s`; `ConfirmBinder()`, the
//! Accept that sends the activate; and `CheckBinderDist()`, polled from `OnUpdate`, which hides
//! the dialog the frame it goes false. The reference's handler is `0x5dfdc0`, its
//! `SMSG_BINDER_CONFIRM` arm `0x5e4aa2`.
//!
//! `CheckBinderDist()` tests `d² <= [0xc4c28c]` (`5.55555534362793²`), the same constant and
//! inclusive bound as [`crate::target::SERVICE_RANGE_SQ`], so the question is an [`NpcSession`]
//! closed by [`crate::ui_session::close_npc_session_out_of_range`]; an unstreamed guid reads as
//! gone, so a disconnect needs no teardown.
//!
//! Deviation: the reference never clears its latched guid (`0xc4d7c0`), so a second
//! `ConfirmBinder()` in range re-sends the activate; this drops the guid on `SMSG_PLAYERBOUND`,
//! because an answered question is not a question and the reference's dialog is gone by then.

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

/// The innkeeper's pending bind question; everything the dialog reads rides the event's argument.
#[derive(Resource, Default)]
pub(crate) struct BinderState {
    /// The innkeeper who asked, sent back on the wire; vmangos binds only a live one in range.
    npc: Option<u64>,
    /// A dialog the feed still owes, set per packet: asking again after a decline is a second
    /// dialog, which a state diff would swallow.
    ask: bool,
}

impl BinderState {
    /// `SMSG_BINDER_CONFIRM`: parks the innkeeper's guid and owes the UI a dialog.
    pub(crate) fn ask(&mut self, npc: u64) {
        self.npc = Some(npc);
        self.ask = true;
    }

    /// The guid to answer with, if a question is live.
    fn pending(&self) -> Option<u64> {
        self.npc
    }

    /// Retracts the question: the range guard's close, and `SMSG_PLAYERBOUND`'s.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.ask = false;
    }
}

/// The range guard closes the question without a packet, as declining does, when the player
/// leaves service range or the innkeeper despawns; that close is what `CheckBinderDist()` reports.
impl NpcSession for BinderState {
    fn npc(&self) -> Option<u64> {
        self.npc
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Publishes `CheckBinderDist()` every frame and fires `CONFIRM_BINDER` for each packet that
/// asked, unconditionally, as the reference's handler always reaches its `SignalEvent2`.
fn feed_binder(
    script: Option<NonSendMut<UiScript>>,
    mut binder: ResMut<BinderState>,
    world: benilla_world::world_point::WorldPoint,
    areas: Option<Res<AreaTableRes>>,
) {
    let Some(mut script) = script else {
        return;
    };
    script.set_binder_pending(binder.pending().is_some());

    if !binder.ask {
        return;
    }
    binder.ask = false;
    let name = area_name(&world, areas.as_deref(), &|key: &str| {
        script
            .lua()
            .globals()
            .get::<String>(key)
            .ok()
            .filter(|t| !t.is_empty())
    });
    script.fire_event("CONFIRM_BINDER", vec![ScriptValue::Str(name)]);
}

/// The dialog's `%s`, an area and never the NPC's name (`0x5dfe5e`): the player's sub-area, else
/// its parent zone, else `HOME_INN`. The reference never withholds the question for want of one.
fn area_name(
    world: &benilla_world::world_point::WorldPoint,
    areas: Option<&AreaTableRes>,
    get: &dyn Fn(&str) -> Option<String>,
) -> String {
    area_name_of(world.area(), areas, get)
}

/// [`area_name`] over a bare leaf id. The tail is the install's `HOME_INN`, the same fallback as
/// `GetBindLocation` (`0x48dae0`); an install without it gives an empty name and still asks.
fn area_name_of(
    leaf: Option<u32>,
    areas: Option<&AreaTableRes>,
    get: &dyn Fn(&str) -> Option<String>,
) -> String {
    let named = |id: u32| {
        areas
            .and_then(|a| a.0.get(id))
            .filter(|row| !row.name.is_empty())
            .map(|row| row.name.clone())
    };
    // The parent zone is the leaf's single-hop `zone_id`; 0 means the leaf is itself a zone.
    let zone = leaf
        .and_then(|id| areas.and_then(|a| a.0.get(id)))
        .map(|row| row.zone_id)
        .filter(|z| *z != 0);
    leaf.and_then(named)
        .or_else(|| zone.and_then(named))
        .or_else(|| get("HOME_INN"))
        .unwrap_or_default()
}

/// Turns the dialog's Accept into `CMSG_BINDER_ACTIVATE`, only while a question is pending: with
/// no guid latched, the reference's `ConfirmBinder()` finds no unit and sends nothing (`0x5dfdfe`).
fn drain_binder(
    script: Option<NonSendMut<UiScript>>,
    binder: Res<BinderState>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let confirms = script.take_binder_confirms();
    if confirms == 0 {
        return;
    }
    let Some(npc) = binder.pending() else {
        return;
    };
    for _ in 0..confirms {
        let _ = commands
            .0
            .send(ClientCommand::BinderActivate { binder: npc });
    }
}

/// `SoundEntries.dbc` 1141, played on `SMSG_PLAYERBOUND` before the area is resolved (`0x5e3d6e`),
/// so an area the catalog cannot name gets the sound and no line.
const SOUND_PLAYERBOUND: u32 = 1141;

/// The innkeeper's packet handlers.
pub(crate) mod net {
    use super::*;

    use bevy::ecs::message::MessageWriter;

    use crate::net::{ServerSoundKind, ServerSoundMessage};
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::BinderConfirm, on_confirm)
            .net_handler(K::PlayerBound, on_bound);
    }

    fn on_confirm(In(ev): In<SessionEvent>, mut binder: ResMut<BinderState>) {
        if let SessionEvent::BinderConfirm { binder: npc } = ev {
            binder.ask(npc);
        }
    }

    fn on_bound(
        In(ev): In<SessionEvent>,
        mut binder: ResMut<BinderState>,
        mut errors: ResMut<crate::ui_action::UiErrorKeys>,
        areas: Option<Res<AreaTableRes>>,
        mut sounds: MessageWriter<ServerSoundMessage>,
    ) {
        if let SessionEvent::PlayerBound { binder: npc, area } = ev {
            debug!("net: bound to area {area} by {npc:#x}");
            bound(
                area,
                &mut binder,
                &mut errors,
                areas.as_deref(),
                &mut sounds,
            );
        }
    }

    /// `SMSG_PLAYERBOUND`, in the handler's order (`0x5e3d3f`): retract the question, play the
    /// handler's own sound (the error row has none), then queue `ERR_DEATHBIND_SUCCESS_S`
    /// (`DisplayError(0x138)`, a system chat line) named by the area. The hearthstone moves on
    /// `SMSG_BINDPOINTUPDATE`, which this leaves alone, as the reference does.
    pub(crate) fn bound(
        area: u32,
        binder: &mut BinderState,
        errors: &mut crate::ui_action::UiErrorKeys,
        areas: Option<&AreaTableRes>,
        sounds: &mut MessageWriter<ServerSoundMessage>,
    ) {
        binder.clear();
        sounds.write(ServerSoundMessage {
            kind: ServerSoundKind::Sound2d,
            sound_id: SOUND_PLAYERBOUND,
            source: None,
        });
        let Some(name) = areas.and_then(|a| a.0.name(area)).filter(|n| !n.is_empty()) else {
            debug!("ui_binder: bound to area {area}, which AreaTable does not name — no line");
            return;
        };
        errors.0.push(crate::ui_action::UiError::s(
            "ERR_DEATHBIND_SUCCESS_S",
            name,
        ));
    }
}

/// The innkeeper bind flow: the range guard, the dialog's feed, and its answer.
pub(crate) struct UiBinderPlugin;

impl Plugin for UiBinderPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<BinderState>().add_systems(
            Update,
            (
                // Range-close before the feed, so walking away hides the dialog the same frame.
                close_npc_session_out_of_range::<BinderState>.before(feed_binder),
                feed_binder.in_set(UiFeed),
                drain_binder.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_twice_owes_two_dialogs() {
        let mut binder = BinderState::default();
        assert_eq!(binder.pending(), None);

        binder.ask(0x2a);
        assert!(binder.ask);
        binder.ask = false; // the feed fired the first dialog
        assert_eq!(binder.pending(), Some(0x2a), "the guid outlives the fire");

        binder.ask(0x2a);
        assert!(binder.ask, "the same innkeeper asking again owes a dialog");
    }

    /// Area 186 is Dolanaar, a leaf area with an innkeeper.
    #[test]
    fn the_dialogs_name_falls_back_sub_area_then_zone_then_home_inn() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let areas = AreaTableRes(
            benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable.dbc"),
        );
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let get = |key: &str| {
            vm.lua()
                .globals()
                .get::<String>(key)
                .ok()
                .filter(|t| !t.is_empty())
        };
        let inn = get("HOME_INN").expect("HOME_INN ships");

        assert_eq!(area_name_of(Some(186), Some(&areas), &get), "Dolanaar");
        // No row, no catalog, no id: all three reach the GlobalString.
        assert_eq!(area_name_of(Some(0xffff), Some(&areas), &get), inn);
        assert_eq!(area_name_of(Some(186), None, &get), inn);
        assert_eq!(area_name_of(None, Some(&areas), &get), inn);
        // An install without the key withholds the name, never the question.
        assert_eq!(area_name_of(None, None, &|_| None), "");
    }

    /// Resolved against the install's table: a mistyped key would silence the line, not garble it.
    #[test]
    fn the_bind_queues_the_deathbind_success_line_named_by_the_area() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| vm.lua().globals().get::<String>(key).ok();

        let msg = crate::ui_action::UiError::s("ERR_DEATHBIND_SUCCESS_S", "Dolanaar");
        assert_eq!(msg.key, "ERR_DEATHBIND_SUCCESS_S");
        assert_eq!(
            crate::ui_action::ui_error_text(&msg, &g).as_deref(),
            Some("Dolanaar is now your home.")
        );
    }

    #[test]
    fn closing_retracts_the_guid_and_the_unfired_dialog() {
        let mut binder = BinderState::default();
        binder.ask(0x2a);
        binder.close();
        assert_eq!(binder.pending(), None);
        assert!(!binder.ask);
    }
}
