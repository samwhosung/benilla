//! The innkeeper's bind question. Selecting the gossip line binds nothing: the server closes the
//! gossip and asks (`SMSG_BINDER_CONFIRM`, vmangos `Player::SetBindPoint`), and the bind happens
//! only on `CMSG_BINDER_ACTIVATE`, when the innkeeper casts spell 3286 and `SPELL_EFFECT_BIND`
//! sends `SMSG_BINDPOINTUPDATE` and `SMSG_PLAYERBOUND`.
//!
//! The stock surface (`StaticPopup.lua:1321-1335`, `UIParent.lua:125`, `547-548`) is the
//! `CONFIRM_BINDER` event, whose one argument fills the dialog's `%s`; `ConfirmBinder()`, the
//! Accept that sends the activate; and `CheckBinderDist()`, polled from `OnUpdate`, which hides
//! the dialog the frame it goes false and reads the latch through the call host
//! ([`crate::ui_dialog_verbs::DialogLive`]). The reference's handler is `0x5dfdc0`, its
//! `SMSG_BINDER_CONFIRM` arm `0x5e4aa2`: an innkeeper in range (`0x5dfe3c`) is latched and the
//! event fires inside it, here on the dialogs' lane at the packet's place; out of range nothing.
//!
//! `CheckBinderDist()` tests the latched innkeeper's range at the call (`0x48d234` →
//! `0x5dff50`, `d² <= [0xc4c28c]`, [`crate::ui_dialog_verbs::DialogReach`]) and nothing closes
//! the question, so walking away hides the dialog through its own `OnUpdate`; an unstreamed guid
//! reads as gone, so a disconnect needs no teardown.
//!
//! Deviation: the reference never clears its latched guid (`0xc4d7c0`), so a second
//! `ConfirmBinder()` in range re-sends the activate; this drops the guid on `SMSG_PLAYERBOUND`,
//! because an answered question is not a question and the reference's dialog is gone by then.

use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::net::{ClientCommand, NetCommands};

/// The innkeeper's pending bind question; everything the dialog reads rides the event's argument.
#[derive(Resource, Default)]
pub(crate) struct BinderState {
    /// The innkeeper who asked, sent back on the wire; vmangos binds only a live one in range.
    npc: Option<u64>,
}

impl BinderState {
    /// `SMSG_BINDER_CONFIRM` in range: the latch (`0xc4d7c0`).
    pub(crate) fn ask(&mut self, npc: u64) {
        self.npc = Some(npc);
    }

    /// The guid to answer with, if a question is live.
    pub(crate) fn pending(&self) -> Option<u64> {
        self.npc
    }

    /// Retracts the question: `SMSG_PLAYERBOUND`'s.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
    }
}

/// The dialog's `%s`, an area and never the NPC's name (`0x5dfe5e`): the player's sub-area
/// (`0xb4e318`), else its parent zone (`0xb4e314`), else `HOME_INN`, the same fallback as
/// `GetBindLocation` (`0x48dae0`); an install without it gives an empty name and still asks.
pub(crate) fn area_name_of(
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

/// What `0x5dfdc0` with a guid reads and writes: the latch, the dialogs' lane, the area under the
/// player and the range gate.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BinderAsk<'w, 's> {
    binder: ResMut<'w, BinderState>,
    lane: ResMut<'w, crate::ui_dialog_verbs::DialogLane>,
    area: benilla_world::world_point::AreaUnderfoot<'w>,
    reach: crate::ui_dialog_verbs::DialogReach<'w, 's>,
}

/// `0x5dfdc0` with a guid, the packet's (`0x5e4aa2`) or the innkeeper's click's (bit 7 of the
/// service ladder): an innkeeper past the range gate (`0x5dfe3c`,
/// [`crate::ui_dialog_verbs::DialogReach`]) is latched, then `CONFIRM_BINDER` fires with the area
/// under the player (`0x5dfe5e`); one out of range, or no unit, drops the question.
pub(crate) fn ask_binder(In(npc): In<u64>, mut ask: BinderAsk) {
    if !ask.reach.holds(npc) {
        debug!("ui_binder: innkeeper {npc:#x} missing or out of range — the question drops");
        return;
    }
    ask.binder.ask(npc);
    let leaf = ask.area.area();
    ask.lane
        .push(crate::ui_dialog_verbs::DialogSignal::ConfirmBinder { leaf });
}

/// `ConfirmBinder()` inside its call ([`benilla_ui::script::BinderCall`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BinderSteps<'w, 's> {
    binder: Option<Res<'w, BinderState>>,
    reach: crate::ui_dialog_verbs::DialogReach<'w, 's>,
    net: Res<'w, NetCommands>,
}

impl BinderSteps<'_, '_> {
    /// `0x5dfdc0(0)`: `CMSG_BINDER_ACTIVATE` with the latched innkeeper while its unit is held in
    /// range at the call (`0x5dfdf5`, `0x5dfe4d`); with no guid latched, no unit or out of range
    /// the reference sends nothing (`0x5dfdfe`).
    pub(crate) fn confirm(&self) {
        let Some(npc) = self.binder.as_deref().and_then(BinderState::pending) else {
            return;
        };
        if !self.reach.holds(npc) {
            debug!("ui_binder: innkeeper {npc:#x} missing or out of range at the confirm");
            return;
        }
        let _ = self
            .net
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

    /// `SMSG_BINDER_CONFIRM`'s arm (`0x5e4aa2`): `0x5dfdc0` with the packet's guid
    /// ([`super::ask_binder`]). Asking again after a decline is a second dialog, each its packet's.
    fn on_confirm(In(ev): In<SessionEvent>, ask: super::BinderAsk) {
        if let SessionEvent::BinderConfirm { binder: npc } = ev {
            super::ask_binder(In(npc), ask);
        }
    }

    fn on_bound(
        In(ev): In<SessionEvent>,
        mut binder: ResMut<BinderState>,
        errors: Res<crate::ui_action::UiErrorKeys>,
        areas: Option<Res<AreaTableRes>>,
        mut sounds: MessageWriter<ServerSoundMessage>,
    ) {
        if let SessionEvent::PlayerBound { binder: npc, area } = ev {
            debug!("net: bound to area {area} by {npc:#x}");
            bound(area, &mut binder, &errors, areas.as_deref(), &mut sounds);
        }
    }

    /// `SMSG_PLAYERBOUND`, in the handler's order (`0x5e3d3f`): retract the question, play the
    /// handler's own sound (the error row has none), then queue `ERR_DEATHBIND_SUCCESS_S`
    /// (`DisplayError(0x138)`, a system chat line) named by the area. The hearthstone moves on
    /// `SMSG_BINDPOINTUPDATE`, which this leaves alone, as the reference does.
    pub(crate) fn bound(
        area: u32,
        binder: &mut BinderState,
        errors: &crate::ui_action::UiErrorKeys,
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
        errors.push(crate::ui_action::UiError::s(
            "ERR_DEATHBIND_SUCCESS_S",
            name,
        ));
    }
}

/// The innkeeper bind flow: the question's packet, the dialog's lane, and its answer.
pub(crate) struct UiBinderPlugin;

impl Plugin for UiBinderPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::ui_dialog_verbs::add_dialog_lane(app);
        app.init_resource::<BinderState>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::UiScript;

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
}
