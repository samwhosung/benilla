//! The charter VM feed/drain — the systems half of [`super`]: resolve the wire's guids and ids into
//! the snapshot the two charter windows read, fire the four events on their edges, drain the
//! composed lines onto their two channels, and turn the Lua-side [`PetitionRequest`] intents into
//! their sends.
//!
//! **`PETITION_SHOW` is DEFERRED, and that is the load-bearing behaviour of this file.** The real
//! client fires it only when *no signer name is still resolving* **and** *the petition record has
//! arrived* (`0x4f419b`-`0x4f41ad`), and it fires exactly once however many names were outstanding
//! (`0x4f4320` decrements the pending counter and fires only on the transition to zero). A client
//! that fires `PETITION_SHOW` straight off the packet paints a window with blank signer names.
//! That is precisely what this file once did — it opened immediately and repainted as each lookup
//! landed, which is a visibly different window.
//!
//! Two things follow, and they are why the deferral is a simplification rather than a cost:
//!
//! - the window never paints a partial state, so `PetitionFrame`'s `format(GUILD_CHARTER_TEMPLATE,
//!   title)` can never see a nil title;
//! - the *getters* still answer honestly at any moment (an addon may call them whenever it likes),
//!   which is why `benilla_ui::script::petition` keeps its no-record leg and its nil names. The
//!   deferral is about the **event**, not about the data.
//!
//! **The owner's name is deliberately NOT part of the defer condition.** The pending counter is
//! incremented only for *signers* (`0x4f4186`/`0x4f42e8`), so a charter whose owner name has not
//! resolved still opens, with `GetPetitionInfo`'s fifth return reading nil. Folding the owner in
//! would look tidier and would hold the window shut in a case the client does not.
//!
//! **Lines go to two different channels**, and which one is not guessable from the key names — see
//! [`super::lines`]. The red `UI_ERROR_MESSAGE` needs the `script` handle, which is why they are
//! composed at apply time and fired here.

use benilla_ui::script::{
    PetitionRecordView, PetitionRequest, PetitionState as VmPetition, UiScript,
    PETITION_TYPE_CHARTER, PETITION_TYPE_PETITION,
};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfGuid, SelfPlayer};
use crate::ui_items::{find_item, ItemSearch};
use crate::ui_session::{npc_switched, NpcSession};

use super::{lines, GuildRegistrarState, PetitionState};

/// What the feed last announced, so the events fire on edges rather than every frame.
#[derive(Default)]
pub(super) struct FedPetition {
    /// The registrar's NPC, if the window was open last frame.
    registrar: Option<u64>,
    /// The charter item the petition window was **showing** last frame — set only once the charter
    /// became READY (module doc). A charter that is open but still resolving is `None` here, which
    /// is what makes the deferral a plain edge rather than a second flag.
    shown_charter: Option<u64>,
    /// The last pushed view, so a change while shown re-fires `PETITION_SHOW`.
    shown: Option<PetitionRecordView>,
    /// …and the signer names beside it, which change independently of the record.
    shown_signers: Vec<Option<String>>,
}

/// Build the snapshot, push it, drain the queued lines, and fire the four events on their edges.
pub(super) fn feed_petition(
    script: Option<NonSendMut<UiScript>>,
    registrar: Res<GuildRegistrarState>,
    mut petition: ResMut<PetitionState>,
    names: Res<NameCache>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    self_guid: Res<SelfGuid>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
    mut fed: Local<crate::ui_script::VmMemo<FedPetition>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let fed = fed.get(&script);

    // The composed lines, each resolved against the VM's own `GlobalStrings.lua` and put on the
    // surface its message record names. Drained before the snapshot so a refusal shows in the
    // same frame as the state change that caused it. A key the string table has no entry for
    // shows nothing — the reference's data-suppression face, and the reason every line upstream
    // carries a key rather than a sentence (decision 2045).
    let composed = std::mem::take(&mut petition.lines);
    let resolved: Vec<crate::ui_action::Shown> = composed
        .iter()
        .filter_map(|e| {
            crate::ui_action::ui_error_text(e, &|key| {
                script.lua().globals().get::<String>(key).ok()
            })
            .map(|text| crate::ui_action::Shown::keyed(e.key, text))
        })
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_petition", resolved);

    // **Rebuilt every frame, deliberately.** Half of what this window shows arrives from a CACHE,
    // not a packet, and reading that cache is what ISSUES its query (decision 0660's law). So a
    // name landing sets no flag anywhere — the only evidence is that rebuilding differs. This is
    // also what drives the deferral: `ready` below can only become true on a rebuild.
    // A price and at most nine names, so the honest version is the cheap one too.
    let open = petition.open.clone();
    let me = self_guid.0.unwrap_or(0);

    let signers: Vec<Option<String>> = open
        .as_ref()
        .map(|o| {
            o.signers
                .iter()
                .map(|s| names.resolve(*s, &commands).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let record = open.as_ref().and_then(|o| {
        let r = petition.records.get(o.petition_id)?;
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
            // Nil until the cache has it — and NOT part of the defer condition (module doc).
            originator: names.resolve(o.owner, &commands).map(str::to_string),
            // The RECORD's owner, which is what the binding compares (`0x4f447a`) — not the
            // packet's. Identical on any sane server; only one of them is the source.
            is_originator: r.owner == me,
        })
    });

    // `CanSignPetition`, the four refusals of `0x4f45e0` — and its no-record leg, which answers
    // **1**. That asymmetry is the reference's own: with no record cached it jumps past the three
    // record-dependent tests straight into the signer scan, over an array the close path has
    // already zeroed.
    let in_guild = self_q
        .iter()
        .next()
        .is_some_and(|s| s.0.player_guild_id() != 0);
    let can_sign = match (open.as_ref(), record.as_ref()) {
        (Some(o), Some(r)) => {
            // (a)/(b) are gated on the charter bit, so a non-charter petition is refused for
            // neither guild membership nor a full list.
            let charter_refusal = r.petition_type == PETITION_TYPE_CHARTER
                && (in_guild || o.signers.len() as i32 >= r.max_signatures);
            !charter_refusal && !r.is_originator && !o.signers.contains(&me)
        }
        // No record: only the signer scan runs, and it is over an empty array.
        (Some(o), None) => !o.signers.contains(&me),
        (None, _) => true,
    };

    script.set_petition(VmPetition {
        charter_cost: registrar.cost(),
        signers: signers.clone(),
        record: record.clone(),
        can_sign,
    });

    // ── The registrar's edges. A registrar→registrar switch is a real close+open in the
    //    reference, because `ShowUIPanel` early-returns on a visible frame and the open sound
    //    would otherwise never replay (`crate::ui_session::npc_switched`).
    let now_registrar = registrar.npc();
    if npc_switched(fed.registrar, now_registrar) {
        script.fire_event("GUILD_REGISTRAR_CLOSED", vec![]);
        script.fire_event("GUILD_REGISTRAR_SHOW", vec![]);
        // …then consume the close `OnHide` just queued, or the drain would apply it to the
        // registrar we have this instant re-opened (decision 0096).
        let _ = script.drop_petition_close_intents();
    } else {
        match (fed.registrar, now_registrar) {
            (None, Some(_)) => script.fire_event("GUILD_REGISTRAR_SHOW", vec![]),
            (Some(_), None) => script.fire_event("GUILD_REGISTRAR_CLOSED", vec![]),
            _ => {}
        }
    }
    fed.registrar = now_registrar;

    // ── The petition window's edges, **behind the deferral**.
    //
    // `ready` is the client's two conditions: the record has arrived, and no signer name is still
    // resolving. A charter that is open but not ready is invisible to every arm below — which is
    // exactly the point, and is why an unresolved name cannot reach the screen.
    let ready = record.is_some() && signers.iter().all(Option::is_some);
    let now_charter = open.as_ref().map(|o| o.item).filter(|_| ready);
    match (fed.shown_charter, now_charter) {
        (Some(a), Some(b)) if a != b => {
            // A charter arriving while another is shown — somebody offering us theirs while ours is
            // up. Close then open, then eat the close `OnHide` queued: without that last line the
            // drain clears the session this branch just opened, AND puts a `MSG_PETITION_DECLINE`
            // on the wire for a charter we did not decline.
            script.fire_event("PETITION_CLOSED", vec![]);
            script.fire_event("PETITION_SHOW", vec![]);
            let _ = script.drop_petition_close_intents();
        }
        // The deferral's own edge: the record landed, or the last name resolved. Fires ONCE.
        (None, Some(_)) => script.fire_event("PETITION_SHOW", vec![]),
        (Some(_), None) => script.fire_event("PETITION_CLOSED", vec![]),
        // A rename echo, or a name resolving after the window was already up.
        (Some(_), Some(_)) if fed.shown != record || fed.shown_signers != signers => {
            script.fire_event("PETITION_SHOW", vec![])
        }
        _ => {}
    }
    fed.shown_charter = now_charter;
    fed.shown = record;
    fed.shown_signers = signers;
}

/// Turn the Era API's charter intents into their sends.
///
/// Most need the app because Lua has no way to name what they act on: buying needs the latched
/// registrar's NPC, turning in needs the charter *in the bags*, offering needs the current target,
/// and the item verbs need the open charter's item guid.
///
/// **`ClosePetition` is not local-only, and that was this file's sharpest correction.** `0x4f3f60`'s
/// decline leg puts `MSG_PETITION_DECLINE` on the wire whenever a petition was open, no sign is in
/// flight, a record is cached, and we are **not** its owner. Closing somebody else's charter tells
/// them so. `CloseGuildRegistrar`, by contrast, really does send nothing — verified by a closure
/// walk that found no `CDataStore` build on its whole path.
pub(super) fn drain_petition(
    script: Option<NonSendMut<UiScript>>,
    mut registrar: ResMut<GuildRegistrarState>,
    mut petition: ResMut<PetitionState>,
    names: Res<NameCache>,
    objects: Objects,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    self_guid: Res<SelfGuid>,
    selection: Res<crate::target::Selection>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_petition_requests();
    if requests.is_empty() {
        return;
    }
    let me = self_guid.0.unwrap_or(0);
    for request in requests {
        match request {
            PetitionRequest::Buy(name) => {
                // The NPC is the petition module's own latched registrar (`[0xbdceb0]`), never the
                // current target and never `CGGameUI`'s interaction pair — the send site's only
                // caller passes that cell by address.
                let Some(npc) = registrar.npc() else {
                    debug!("ui_petition: BuyGuildCharter with no registrar open — dropped");
                    continue;
                };
                let _ = commands.0.send(ClientCommand::PetitionBuy { npc, name });
            }
            PetitionRequest::TurnIn => {
                // A **bag scan**, verified: `0x5ef2b0` walks the sixteen backpack slots then the
                // four bags on every call and latches nothing. Ours is the same walk through the
                // reference's own inventory search; `max_count = 1` means there is at most one.
                let charter = self_q.iter().next().and_then(|store| {
                    find_item(
                        &store.0,
                        &objects,
                        benilla_protocol::messages::CHARTER_ITEM_ENTRY,
                        ItemSearch::default(),
                    )
                });
                match charter {
                    Some((_, _, item)) => {
                        let _ = commands.0.send(ClientCommand::TurnInPetition { item });
                    }
                    // Nothing to address the send with, so none is built — and the refusal is a
                    // RED line, not a chat one (`0x5ef49a` emits id `0x7c`, kind 2).
                    None => petition.lines.push(lines::no_charter_line()),
                }
            }
            // Verified send-free.
            PetitionRequest::CloseRegistrar => registrar.close(),
            PetitionRequest::ClosePetition => {
                petition.decline_on_close(me, &commands);
                petition.close();
            }
            PetitionRequest::Sign(byte) => {
                let Some(item) = petition.open_item() else {
                    continue;
                };
                let _ = commands.0.send(ClientCommand::PetitionSign { item, byte });
                // The in-flight latch, whose one reader is the decline leg above: a close while our
                // own signature is outstanding must not read as a decline.
                petition.signing = true;
            }
            PetitionRequest::Offer => {
                let Some(item) = petition.open_item() else {
                    continue; // guard 1 — silent
                };
                // `OfferPetition()` carries no argument, and the guid it needs is `CGGameUI`'s
                // **selection** pair — the current target — not the interaction pair the registrar
                // latches. With no target the reference is silent too (guard 2).
                let Some(player) = selection.guid else {
                    debug!("ui_petition: OfferPetition with no target — dropped");
                    continue;
                };
                if player == me {
                    // Guard 6, and the only one of the eight that is both reachable here and has a
                    // message. Guards 3/4/5/7 (unit resolution, is-a-player, faction) are the
                    // server's to enforce and are not built; guard 8 (target already guilded) needs
                    // the target's descriptor and is likewise left to the refusal.
                    petition.lines.push(lines::self_offer_line());
                    continue;
                }
                let _ = commands
                    .0
                    .send(ClientCommand::OfferPetition { item, player });
                // Emitted **optimistically on the send**, with no server confirmation — the target
                // is the one the server answers, so this echo is the offerer's only feedback.
                let name = names
                    .resolve(player, &commands)
                    .unwrap_or_default()
                    .to_string();
                petition.lines.push(lines::offered_line(&name));
            }
            PetitionRequest::Rename(name) => {
                let Some(item) = petition.open_item() else {
                    continue;
                };
                let _ = commands
                    .0
                    .send(ClientCommand::PetitionRename { item, name });
            }
            // The client's own name validator refused it; no packet was ever built.
            PetitionRequest::NameRefused(key) => {
                petition.lines.push(lines::name_refused_line(key));
            }
        }
    }
}
