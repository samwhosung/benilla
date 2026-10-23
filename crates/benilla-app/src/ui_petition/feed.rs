//! The charter VM feed: the snapshot the charter windows read, their four events and the packets'
//! composed lines; the verbs act inside their calls ([`super::PetitionSteps`]).
//!
//! `PETITION_SHOW` waits for the record and every signer name (`0x4f419b`-`0x4f41ad`), then fires
//! once (`0x4f4320` fires as the pending count reaches zero); the owner's name is not counted
//! (`0x4f4186`/`0x4f42e8`). The getters answer at any time, nil where nothing has landed.

use benilla_ui::script::{
    PetitionRecordView, PetitionState as VmPetition, PETITION_TYPE_CHARTER, PETITION_TYPE_PETITION,
};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::script_calls::VmWith;
use crate::ui_session::{CoreCloses, NpcWindow};

use super::{GuildRegistrarState, PetitionRecords, PetitionState};
use crate::query_cache::Ask;

/// What the feed last announced of the charter, so the events fire on edges rather than every
/// frame. It is the session's, not the VM's: every `PETITION_SHOW` the reference fires answers a
/// packet (`0x4f41b4`, `0x4f430c`, `0x4f433f`, `0x4f43ad`, `0x5ef29c`), so a `/reload` replays
/// none into the new VM; the session's end forgets it ([`PetitionState::forget_announced`]).
#[derive(Default)]
pub(super) struct FedPetition {
    /// The charter shown last frame; `None` while one is still resolving, so the wait is an edge.
    shown_charter: Option<u64>,
    /// The last pushed view and names, so a change while shown re-fires `PETITION_SHOW`.
    shown: Option<PetitionRecordView>,
    shown_signers: Vec<Option<String>>,
}

impl FedPetition {
    /// Nothing shown: `ClosePetition` signalled the close inside its call.
    pub(super) fn closed(&mut self) {
        *self = Self::default();
    }

    /// No signers announced, as the sweep's free (`0x4f3eb0`) leaves it with none to repaint.
    pub(super) fn forget_signers(&mut self) {
        self.shown_signers.clear();
    }
}

/// The leave-world sweep's petition leg (`0x490b11` → `0x4f3eb0`): the signer list is freed, its
/// count zeroed, with no event; the charter window stays.
pub(super) fn free_petition_signers_on_leave_world(mut petition: ResMut<PetitionState>) {
    petition.free_signers();
}

/// The petition window's edge a feed found, fired once the snapshot is in.
enum Edge {
    None,
    Show,
    Closed,
    /// Another charter while one is shown.
    Switch,
}

/// Build the snapshot, push it, show the packets' lines, and fire the four events on their edges.
#[allow(clippy::too_many_arguments)] // the feed's inputs, each read once
pub(super) fn feed_petition(
    // The registrar beside the charter: the host's close core clears its cell inside a call.
    mut vm: VmWith<(
        ResMut<'static, PetitionState>,
        Res<'static, GuildRegistrarState>,
    )>,
    records: Res<PetitionRecords>,
    names: Res<NameCache>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    self_guid: Res<SelfGuid>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
    // Apart from the charter's: the registrar's close core tells the VM, and leaves the charter
    // open.
    mut fed_registrar: crate::ui_script::WindowMemo<Option<u64>>,
    mut closes: CoreCloses,
) {
    // Shown before the snapshot, so a refusal lands the same frame as its state change; a key the
    // VM's `GlobalStrings.lua` lacks shows nothing, as in the reference.
    let composed = std::mem::take(&mut vm.own().0.lines);
    if !composed.is_empty() {
        vm.run(|script| {
            let Some(script) = script else {
                return;
            };
            let resolved: Vec<crate::ui_action::Shown> =
                composed.iter().filter_map(|e| shown(script, e)).collect();
            crate::ui_action::show_messages(script, &mut sink, "ui_petition", resolved);
        });
    }

    let mut registrar_shown = false;
    let (edge, deferred) = vm.turn(|script, (mut petition, registrar)| {
        let Some(script) = script else {
            return (Edge::None, None);
        };
        let fed_registrar = fed_registrar.get(script);
        if closes.since_last(script, NpcWindow::Registrar) {
            *fed_registrar = None;
        }

        // Rebuilt every frame: reading the caches issues their queries, so a landing sets no flag
        // and only a rebuild sees it.
        let open = petition.open.clone();
        let me = self_guid.0.unwrap_or(0);
        // The getters only read (`0x4f45a1`, `0x4f4459`); the show looked the signers up.
        let ask = Ask::at(std::mem::take(&mut petition.shown), &commands);

        let signers: Vec<Option<String>> = open
            .as_ref()
            .map(|o| {
                o.signers
                    .iter()
                    .map(|s| names.resolve_player(*s, ask).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let record = open.as_ref().and_then(|o| {
            let r = records.records.get(o.petition_id)?;
            Some(PetitionRecordView {
                petition_type: if r.is_charter {
                    PETITION_TYPE_CHARTER
                } else {
                    PETITION_TYPE_PETITION
                }
                .to_string(),
                title: r.name.clone(),
                body_text: r.body_text.clone(),
                max_signatures: r.required,
                // Nil until the name lands, and not waited for: only read (`0x4f4459`).
                originator: names.resolve_player(o.owner, Ask::Read).map(str::to_string),
                // The record's owner, which the binding compares (`0x4f447a`), not the packet's.
                is_originator: r.owner == me,
            })
        });

        // `CanSignPetition` (`0x4f45e0`). With no record the reference runs only the signer scan,
        // over the real signers: the close path zeroes the array, then `0x4f40b0` refills it
        // (`0x5eef29`).
        let in_guild = self_q
            .iter()
            .next()
            .is_some_and(|s| s.0.player_guild_id() != 0);
        let can_sign = match (open.as_ref(), record.as_ref()) {
            (Some(o), Some(r)) => {
                // The guild-member and full-list refusals apply only to a charter.
                let charter_refusal = r.petition_type == PETITION_TYPE_CHARTER
                    && (in_guild || o.signers.len() as i32 >= r.max_signatures);
                !charter_refusal && !r.is_originator && !o.signers.contains(&me)
            }
            // No record: only the signer scan runs.
            (Some(o), None) => !o.signers.contains(&me),
            (None, _) => true,
        };

        let now = VmPetition {
            charter_cost: registrar.cost(),
            signers: signers.clone(),
            record: record.clone(),
            can_sign,
        };

        // ── The registrar's open; its close is the core's ([`super::close_core`]), and a
        //    disconnect's clear tells the VM nothing.
        let now_registrar = registrar.npc();
        registrar_shown = now_registrar.is_some() && *fed_registrar != now_registrar;
        *fed_registrar = now_registrar;

        // ── The petition window's edges, once the record and every signer name have landed.
        let ready = record.is_some() && signers.iter().all(Option::is_some);
        let now_charter = open.as_ref().map(|o| o.item).filter(|_| ready);
        let fed = &mut petition.fed;
        let edge = match (fed.shown_charter, now_charter) {
            (Some(a), Some(b)) if a != b => Edge::Switch,
            // The wait's end: the record landed or the last name resolved; fires once.
            (None, Some(_)) => Edge::Show,
            (Some(_), None) => Edge::Closed,
            // A rename echo, or a name resolving after the window was already up.
            (Some(_), Some(_)) if fed.shown != record || fed.shown_signers != signers => Edge::Show,
            _ => Edge::None,
        };
        // A close signals over the old charter's record with its signers freed (`0x4f3ef0`): the
        // record goes, or the next one's lookup replaces it, only after the signal
        // (`0x4f402c`, then `0x4f4061` or `0x4f4078`). The new state is pushed after it.
        let deferred = if matches!(edge, Edge::Closed | Edge::Switch) {
            script.set_petition(VmPetition {
                charter_cost: registrar.cost(),
                signers: Vec::new(),
                record: fed.shown.clone(),
                can_sign: true,
            });
            Some(now)
        } else {
            script.set_petition(now);
            None
        };
        fed.shown_charter = now_charter;
        fed.shown = record;
        fed.shown_signers = signers;
        (edge, deferred)
    });

    // Each signal bare, with the host lent.
    if registrar_shown {
        vm.run(|s| {
            if let Some(s) = s {
                s.fire_event("GUILD_REGISTRAR_SHOW", vec![]);
            }
        });
    }
    // The close signals with the item's guid zeroed (`0x4f4020`-`0x4f402c`), so the frame's
    // `OnHide`, whose `ClosePetition()` applies in its call, finds nothing to close or decline;
    // the next charter, if any, is set after it (`0x4f403b`).
    if matches!(edge, Edge::Closed | Edge::Switch) {
        let held = vm.own().0.open.take();
        vm.run(|s| {
            if let Some(s) = s {
                s.fire_event("PETITION_CLOSED", vec![]);
            }
        });
        vm.own().0.open = held;
        vm.run(|s| {
            if let (Some(s), Some(now)) = (s, deferred) {
                s.set_petition(now);
            }
        });
    }
    if matches!(edge, Edge::Show | Edge::Switch) {
        vm.run(|s| {
            if let Some(s) = s {
                s.fire_event("PETITION_SHOW", vec![]);
            }
        });
    }
}

/// A composed line resolved against the VM's strings.
pub(super) fn shown(
    script: &benilla_ui::script::UiScript,
    e: &super::lines::Line,
) -> Option<crate::ui_action::Shown> {
    crate::ui_action::ui_error_text(e, &|key| script.lua().globals().get::<String>(key).ok())
        .map(|text| crate::ui_action::Shown::keyed(e.key, text))
}
