//! Every row's change callback, the reference's function handed to `CVar::Register` (`0x63db90`)
//! and called inside `CVar::Set` (`0x63df50`, at `0x63df7d`): one param holding each knob a
//! callback writes ([`CvarCallbacks`]), so a write runs its callback wherever it is made. A Lua
//! verb's write runs it inside the verb's call ([`CvarSteps`], as the call host's applier), and the
//! frame's sync runs it for the writes no call made; the boot load and a dying VM's fold trigger
//! [`CvarChanged`], which [`on_cvar_changed`] runs the same callbacks for. Each callback takes its
//! knob as a [`Mut`], so only the arm that writes it marks it changed.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::{CvarChanged, Cvars};

/// Each knob a row's change callback writes, one field per owner; a knob absent from the world (a
/// harness without its plugin) skips its callback.
#[derive(SystemParam)]
pub(crate) struct CvarCallbacks<'w> {
    approach: Option<ResMut<'w, crate::player::Approach>>,
    sound: Option<ResMut<'w, crate::sound::SoundConfig>>,
    ui_scale: Option<ResMut<'w, crate::ui_script::UiScaleCvar>>,
    video: Option<crate::video::VideoKnobs<'w>>,
    look: Option<ResMut<'w, crate::player::LookConfig>>,
    zoom: Option<ResMut<'w, crate::player::camera_zoom::ZoomLimit>>,
    follow: Option<ResMut<'w, crate::player::camera::FollowConfig>>,
    camera_options: Option<ResMut<'w, crate::player::camera_dynamics::CameraOptions>>,
    click: Option<ResMut<'w, crate::target::ClickConfig>>,
    assist: Option<ResMut<'w, crate::target::AssistAttack>>,
    self_cast: Option<ResMut<'w, crate::spell::AutoSelfCast>>,
    damage_text: Option<ResMut<'w, crate::combat_text::DamageTextGates>>,
    log_ranges: Option<ResMut<'w, crate::ui_chat::combat::CombatLogRanges>>,
    log_periodic: Option<ResMut<'w, crate::ui_chat::combat::LogPeriodicSpells>>,
    loot: Option<ResMut<'w, crate::ui_loot::LootConfig>>,
    names: Option<ResMut<'w, crate::nameplates::NameConfig>>,
    game_tip: Option<ResMut<'w, crate::game_tip::GameTipSetting>>,
    text_filter: Option<ResMut<'w, crate::text_filter::TextFilterSwitches>>,
    bubbles: Option<ResMut<'w, crate::chat_bubble::BubbleConfig>>,
    guild_notify: Option<ResMut<'w, crate::ui_guild::GuildMemberNotify>>,
    block_trades: Option<ResMut<'w, crate::ui_trade::BlockTrades>>,
    minimap_zoom: Option<ResMut<'w, crate::minimap::MinimapZoom>>,
    pane_rate: Option<ResMut<'w, crate::portrait::PaneRate>>,
    fps_journal: Option<ResMut<'w, crate::perf::FpsJournalSetting>>,
    realmlist: Option<ResMut<'w, crate::realmlist::Realmlist>>,
    gamma: Option<ResMut<'w, crate::ui_gamma::DisplayGamma>>,
    render_scale: Option<ResMut<'w, crate::world_backdrop::RenderScale>>,
}

impl CvarCallbacks<'_> {
    /// Run every callback on the move `ev`; a callback that writes the registry back (a second
    /// spelling's mirror, a console line) writes `cvars`.
    pub(crate) fn run(&mut self, ev: &CvarChanged, cvars: &mut Cvars) {
        if let Some(k) = self.approach.as_mut() {
            crate::player::approach_on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.sound.as_mut() {
            crate::sound::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.ui_scale.as_mut() {
            crate::ui_script::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.video.as_mut() {
            k.on_cvar(ev, cvars);
        }
        if let (Some(look), Some(zoom), Some(follow)) =
            (self.look.as_mut(), self.zoom.as_mut(), self.follow.as_mut())
        {
            crate::player::camera::on_cvar(ev, look.reborrow(), zoom.reborrow(), follow.reborrow());
        }
        if let Some(k) = self.camera_options.as_mut() {
            crate::player::camera_dynamics::on_cvar(ev, k.reborrow());
        }
        if let (Some(click), Some(assist)) = (self.click.as_mut(), self.assist.as_mut()) {
            crate::target::on_cvar(ev, click.reborrow(), assist.reborrow());
        }
        if let Some(k) = self.self_cast.as_mut() {
            crate::spell::cast_target::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.damage_text.as_mut() {
            crate::combat_text::on_cvar(ev, k.reborrow());
        }
        if let (Some(ranges), Some(periodic)) =
            (self.log_ranges.as_mut(), self.log_periodic.as_mut())
        {
            crate::ui_chat::combat::on_cvar(ev, ranges.reborrow(), periodic.reborrow());
        }
        if let Some(k) = self.loot.as_mut() {
            crate::ui_loot::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.names.as_mut() {
            crate::nameplates::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.game_tip.as_mut() {
            crate::game_tip::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.text_filter.as_mut() {
            crate::text_filter::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.bubbles.as_mut() {
            crate::chat_bubble::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.guild_notify.as_mut() {
            crate::ui_guild::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.block_trades.as_mut() {
            crate::ui_trade::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.minimap_zoom.as_mut() {
            crate::minimap::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.pane_rate.as_mut() {
            crate::portrait::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.fps_journal.as_mut() {
            crate::perf::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.realmlist.as_mut() {
            crate::realmlist::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.gamma.as_mut() {
            crate::ui_gamma::on_cvar(ev, k.reborrow());
        }
        if let Some(k) = self.render_scale.as_mut() {
            crate::world_backdrop::on_cvar(ev, k.reborrow());
        }
    }
}

/// The callbacks for a move triggered as an event: the boot load's, a dying VM's fold, a latch
/// commit's. With no registry nothing moved.
pub(crate) fn on_cvar_changed(
    ev: On<CvarChanged>,
    cvars: Option<ResMut<Cvars>>,
    mut callbacks: CvarCallbacks,
) {
    if let Some(mut cvars) = cvars {
        callbacks.run(&ev, &mut cvars);
    }
}

/// The registry and the callbacks, as the call host applies a write inside the call that made it
/// and as the frame's sync takes the rest ([`CvarSteps::sync`]).
#[derive(SystemParam)]
pub(crate) struct CvarSteps<'w> {
    cvars: Option<ResMut<'w, Cvars>>,
    callbacks: CvarCallbacks<'w>,
}

impl CvarSteps<'_> {
    /// The registry, absent in a harness without the plugin.
    pub(crate) fn cvars(&mut self) -> Option<&mut Cvars> {
        self.cvars.as_deref_mut()
    }

    /// Host writes an engine verb makes inside its call (`CVar::Set`, `0x63df50`), each through
    /// its row's check and callback, then the VM's mirror synced, before the verb returns.
    pub(crate) fn write(&mut self, script: &mut UiScript, writes: &[(&str, String)]) {
        let Some(cvars) = self.cvars.as_deref_mut() else {
            return;
        };
        for (name, value) in writes {
            cvars.set(name, value);
        }
        self.sync(Some(script));
    }

    /// Take the VM's queued writes into the registry, run each accepted move's callback, then hand
    /// the VM what the registry wrote (a host write, a callback's mirror of a second spelling).
    /// Read before taking, so a quiet sync never deref-muts the registry.
    pub(crate) fn sync(&mut self, script: Option<&mut UiScript>) {
        let Some(cvars) = self.cvars.as_deref_mut() else {
            return;
        };
        let script = match script {
            Some(script) => {
                if script.has_cvar_traffic() || script.cvar_lua_created() != cvars.lua_created {
                    cvars.drain_vm(script);
                    cvars.log_console_lines();
                }
                Some(script)
            }
            None => None,
        };
        if cvars.has_events() {
            for event in cvars.take_events() {
                self.callbacks.run(&event, cvars);
            }
        }
        if cvars.has_console_lines() {
            cvars.log_console_lines();
        }
        if cvars.has_outbox() {
            let outbox = cvars.take_outbox();
            // With no VM there is no mirror; a later VM is seeded from the table.
            if let Some(script) = script {
                for (name, value) in outbox {
                    script.set_cvar_host(&name, &value);
                }
            }
        }
    }
}
