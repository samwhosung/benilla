//! The GM trouble-ticket flow — the Help window's wire, its events, and its answers.
//!
//! **The law worth stating first: the client holds no ticket state.** There is no local "I have a
//! ticket" bit that survives anything; every question is a round trip, and `SMSG_GMTICKET_GETTICKET`
//! is the only truth about whether a ticket exists, what it says, and how long the wait is. That is
//! why this module is almost entirely feed-and-drain with a single counter of substance, and why
//! the Help window re-polls rather than caching.
//!
//! ## Three events, and why each fires when it does
//!
//! - **`UPDATE_TICKET`** — fired for **every** `SMSG_GMTICKET_GETTICKET`, including the "you have
//!   no ticket" answer, and including the second identical answer in a row. It is an **answer
//!   ticket**, not a state diff ([`GmTicketState::answers`]): the shipped window polls
//!   `GetGMTicket()` every `GMTICKET_CHECK_INTERVAL` (600 s) from `TicketStatus_OnUpdate`, and a
//!   feed that suppressed an unchanged answer would make the poll look like a hang. The idiom is
//!   [`crate::ui_party`]'s saved-instance ticket, for exactly the same reason.
//! - **`UPDATE_GM_STATUS`** — fired for every `SMSG_GMTICKETSYSTEMSTATUS`. Sets the window's
//!   `PETITION_QUEUE_ACTIVE`, which gates the whole "page a GM" path.
//! - **`GMSURVEY_DISPLAY`** — **not fired, deliberately.** See "The survey" below.
//!
//! ## The argument order is NOT the wire order
//!
//! `HelpFrameOpenTicket_OnEvent` reads `UPDATE_TICKET` as `arg1` = category, `arg2` = description,
//! `arg3..5` = the three day-valued floats, `arg6` = assignedToGM, `arg7` = openedByGM. The packet
//! puts the **text first and the category second** (both emulators agree byte-for-byte; see
//! [`benilla_protocol::messages::gm_ticket`]). So the reorder happens here, in [`feed_gm_ticket`],
//! which is where the reference client's own handler does it. Do not "fix" the decoder to match
//! the event: that would desynchronise every field after the string.
//!
//! **The no-ticket answer is a single `arg1 = 0`**, because the window's test is
//! `if ( arg1 and arg1 ~= 0 )`. That branch is what resets Submit/Cancel back to SUBMIT/CANCEL and
//! empties the edit box, so it is load-bearing rather than a null case.
//!
//! ## What vmangos actually does, and what that costs us
//!
//! Verified against the vmangos source (`Handlers/GMTicketHandler.cpp`, `GMTicketMgr.cpp`,
//! `Commands/TicketCommands.cpp`) — four behaviours a client author has to design around:
//!
//! 1. **Silence is a legal answer.** Create returns with no packet at all when the queue is off,
//!    when the player is under `GMTickets.MinLevel`, or when the category is ≥ 11
//!    (`GMTicketHandler.cpp:91,106-113`); delete with no ticket likewise (`:73-86`). Nothing here
//!    may block on or retry an answer — and nothing does: every send is fire-and-forget, and the
//!    window's own re-poll is what eventually reconciles.
//! 2. **`SMSG_GMTICKET_GETTICKET` and `SMSG_GMTICKET_DELETETICKET` arrive UNSOLICITED** when a GM
//!    runs `.ticket viewid`/`escalate`/`complete`/`delete` (`GMTicketMgr.cpp:153-159`,
//!    `TicketCommands.cpp:100-103,265,443-452`). They are handled identically to a solicited one,
//!    which is the whole benefit of not correlating answers to asks.
//! 3. **The GM's reply arrives inside the ticket text.** 1.12 has no response channel, so vmangos
//!    appends `"…GM answer: <response>"` to the description itself (`GMTicketMgr.cpp:124-136`).
//!    The window renders it verbatim, which is correct — there is nowhere else for it to go.
//! 4. **Editing is rate-limited to 2 per world tick, and the sanction is a KICK**
//!    (`WorldSession.cpp:1316-1342`). One send per click, never a retry.
//!
//! ## The survey
//!
//! `Blizzard_GMSurveyUI` and the `GMSURVEY_DISPLAY` event are **deferred, not forgotten**. The
//! trigger does not exist on this server: `SMSG_GM_TICKET_STATUS_UPDATE` — the packet cmangos uses
//! to raise a survey — is registered in vmangos's opcode table but **never constructed anywhere in
//! its source**, so a vmangos client can never be asked to fill one in. Building the survey window
//! now would be shipping a panel nothing can open. The DBCs it would read are already on disk and
//! already understood (`GMSurveyCurrentSurvey` → `GMSurveySurveys` → `GMSurveyQuestions`, five
//! questions, ids 28-32), so the work is scoped whenever a server grows the trigger.

use benilla_ui::script::{GmTicketIntent, GmTicketWrite, ScriptValue, UiScript};
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;

use crate::net::{ClientCommand, NetCommands};
use crate::player::Player;
use crate::ui_script::{UiFeed, UiInput, VmMemo};

/// Spell 7355 "Stuck" — what `Stuck()` casts, and the Help window's "Auto-Unstuck".
///
/// Pinned from **both** sides rather than assumed: `Spell.dbc` row 7355 is named "Stuck" in the
/// shipped 5875 client, and it is the only spell in the server's whole `spell_template` table
/// carrying `SPELL_EFFECT_STUCK` (84), whose handler teleports the player to their last safe
/// position (`vmangos SpellEffects.cpp:4697`, gated by `CastUnstuck` which defaults on). So the
/// id is not a guess in either direction.
const SPELL_STUCK: u32 = 7355;

/// What the last `SMSG_GMTICKET_GETTICKET` said, and **how many** have arrived.
///
/// The count is the load-bearing half. A `bool`/value diff cannot express "the server answered
/// again with the same thing", and that is precisely the Help window's 10-minute poll — so the
/// feed fires on the counter moving, never on the value changing.
#[derive(Resource, Default)]
pub(crate) struct GmTicketState {
    /// The open ticket, or `None` for "you have no ticket" — the ordinary answer.
    ticket: Option<Box<benilla_protocol::messages::GmTicket>>,
    /// One per `SMSG_GMTICKET_GETTICKET`, wrapping. The answer ticket.
    answers: u32,
    /// Re-asks the ENGINE owes the server — see [`Self::note_write_landed`]. Drained into
    /// `CMSG_GMTICKET_GETTICKET` sends by [`drain_gm_ticket`], beside the ones Lua asked for.
    engine_reasks: u32,
    /// The last queue status the server reported, and its own answer counter — same idiom, because
    /// `GetGMStatus()` is asked on every window open and the answer is usually unchanged.
    ///
    /// **`i32`, not `u32`, and that is byte-exact rather than a preference**: the reference
    /// copies the field off the wire verbatim at `0x418e95` with no
    /// extension, pushes it unmodified at `0x5e467b`, and hands it to Lua through
    /// `0x704fa6 fild dword` — a **signed** load, with no `cmp`, `test`, clamp or mapping anywhere
    /// between the wire and the event. So `HelpFrame`'s `arg1 == -1` arm is reachable exactly when
    /// a server sends `0xFFFFFFFF`, and reading the field unsigned would turn that into
    /// 4294967295, silently costing the "the queue is down" dialog.
    queue_status: i32,
    queue_answers: u32,
}

impl GmTicketState {
    /// `SMSG_GMTICKET_GETTICKET` — replace the ticket wholesale and count the answer. **An empty
    /// answer still counts**: it is what "you abandoned it" and "you never had one" both look like,
    /// and the window has to hear it.
    pub(crate) fn answer(&mut self, ticket: Option<Box<benilla_protocol::messages::GmTicket>>) {
        self.ticket = ticket;
        self.answers = self.answers.wrapping_add(1);
    }

    /// **The client re-asks for its own ticket whenever a write lands** — the mechanism that makes
    /// the shipped UI's total silence on the three response opcodes correct rather than a gap.
    ///
    /// `CMSG_GMTICKET_GETTICKET` (`0x211`) has three callers, and two of them are *engine* legs
    /// reacting to a server push — the `0x206`/`0x208` arm on response codes 2 (create-ok) and 4
    /// (update-ok) at `0x5e4479`, and the `0x328` handler on body value 1 at `0x5e7932`. **Neither
    /// carries an idempotence guard**, so it is one resend per qualifying push, which is what this
    /// counter models.
    ///
    /// Without it the window would not learn its own ticket exists until the 10-minute poll came
    /// round: file a ticket, and the toast stays dark and the form stays a form for up to ten
    /// minutes. That is exactly the "goes stale" failure, and no FrameXML handler can fix it —
    /// there is none to fix.
    pub(crate) fn note_write_landed(&mut self) {
        self.engine_reasks = self.engine_reasks.saturating_add(1);
    }

    /// Drain the engine's owed re-asks.
    fn take_reasks(&mut self) -> u32 {
        std::mem::take(&mut self.engine_reasks)
    }

    /// `SMSG_GMTICKETSYSTEMSTATUS` — the petition queue's state, counted the same way.
    pub(crate) fn answer_queue(&mut self, status: i32) {
        self.queue_status = status;
        self.queue_answers = self.queue_answers.wrapping_add(1);
    }

    /// The socket died: a fresh session has not been answered, so both tickets go back to zero
    /// along with the state they described. Without this a reconnect would show the previous
    /// character's ticket until the first poll landed.
    pub(crate) fn clear_session(&mut self) {
        *self = Self::default();
    }
}

/// What the VM has already been told, per VM.
///
/// Behind a [`VmMemo`] (1290/1291) for the reason every feed's is: a `/reload` replaces the VM
/// without despawning the world, and a memory of what the *old* VM heard would leave the new one
/// with no categories, no ticket and no event ever coming.
#[derive(Resource, Default)]
struct GmTicketFeedState {
    vm: VmMemo<FedTicket>,
}

/// The per-VM change bases: the two answer counters already fired, and whether the static category
/// catalog has been handed over.
#[derive(Default)]
struct FedTicket {
    answers: u32,
    queue_answers: u32,
    categories_pushed: bool,
}

/// Fire `UPDATE_TICKET`/`UPDATE_GM_STATUS` for answers the UI has not heard yet, and push the
/// category catalog once.
fn feed_gm_ticket(
    script: Option<NonSendMut<UiScript>>,
    state: Res<GmTicketState>,
    categories: Option<Res<GmTicketCategories>>,
    mut feed: ResMut<GmTicketFeedState>,
) {
    let Some(mut script) = script else {
        return;
    };
    let fed = feed.vm.get(&script);

    // The catalog is static client data — pushed once per VM, before any window can ask for it.
    if !fed.categories_pushed {
        if let Some(categories) = categories.as_deref() {
            script.set_gm_ticket_categories(categories.0.clone());
            fed.categories_pushed = true;
        }
    }

    if fed.queue_answers != state.queue_answers {
        fed.queue_answers = state.queue_answers;
        // arg1 is the queue status as a NUMBER: the window tests `arg1 == 1` for "up" and
        // `arg1 == -1` for "down, and say so". vmangos only ever sends 0 or 1, so the -1 arm — the
        // one that pops HELP_TICKET_QUEUE_DISABLED unprompted — is unreachable against this
        // server. Passed through unchanged rather than remapped: inventing a -1 would put a dialog
        // on screen that the server never asked for.
        script.fire_event(
            "UPDATE_GM_STATUS",
            vec![ScriptValue::Int(i64::from(state.queue_status))],
        );
    }

    if fed.answers != state.answers {
        fed.answers = state.answers;
        script.fire_event("UPDATE_TICKET", update_ticket_args(state.ticket.as_deref()));
    }
}

/// `UPDATE_TICKET`'s argument list — **the reorder** the module doc explains.
///
/// With a ticket: category, text, then the three day-floats, then the two status bytes. Without
/// one: a single `0`, because `HelpFrameOpenTicket_OnEvent` gates its whole "you have a ticket"
/// branch on `arg1 and arg1 ~= 0` and `TicketStatusFrame_OnEvent` on `arg1 ~= 0`.
fn update_ticket_args(ticket: Option<&benilla_protocol::messages::GmTicket>) -> Vec<ScriptValue> {
    let Some(t) = ticket else {
        return vec![ScriptValue::Int(0)];
    };
    vec![
        ScriptValue::Int(i64::from(t.category)),
        ScriptValue::Str(t.text.clone()),
        // Days, as f64 — and deliberately unclamped: a negative arg4/arg5 is the server saying
        // "I don't know", which the window renders as GM_TICKET_UNAVAILABLE. Clamping to 0 here
        // would turn "unavailable" into a confident wait estimate of zero.
        ScriptValue::Number(f64::from(t.ticket_age)),
        ScriptValue::Number(f64::from(t.oldest_ticket_age)),
        ScriptValue::Number(f64::from(t.update_time)),
        ScriptValue::Int(i64::from(t.assigned_to_gm)),
        ScriptValue::Int(i64::from(t.opened_by_gm)),
    ]
}

/// Turn the window's clicks into packets.
fn drain_gm_ticket(
    script: Option<NonSendMut<UiScript>>,
    mut reask: ResMut<GmTicketState>,
    commands: Res<NetCommands>,
    player: Option<Res<Player>>,
    map: Option<Res<benilla_world::world_map::CurrentMap>>,
) {
    let Some(mut script) = script else {
        return;
    };

    // The engine's own re-asks first: they answer a server push that arrived before this frame's
    // input ran, so they precede anything the player typed into it.
    for _ in 0..reask.take_reasks() {
        let _ = commands.0.send(ClientCommand::GmTicketGet);
    }

    for _ in 0..script.take_stuck_casts() {
        let _ = commands.0.send(ClientCommand::CastSpell {
            spell_id: SPELL_STUCK,
            target: None,
        });
    }

    let intents = script.take_gm_ticket_intents();
    if intents.is_empty() {
        return;
    }
    // The position is stamped at SEND time, not at window-open time: a ticket says where you are
    // when you file it, which is what makes `.ticket go` land on the problem. Raw WoW coords —
    // the wire never sees Bevy's basis. Resolved once for the whole batch: every intent in it was
    // raised in the same frame, so they cannot honestly disagree about where the player stands.
    let pos = player.map(|p| bevy_to_wow(p.pos)).unwrap_or([0.0; 3]);
    let map = map.map(|m| m.0).unwrap_or(0);
    // **In call order.** A per-verb drain would reorder `DeleteGMTicket(); GetGMTicket()` into
    // get-then-delete, and the get would answer with the pre-delete state.
    for intent in intents {
        let _ = commands.0.send(match intent {
            GmTicketIntent::Ask => ClientCommand::GmTicketGet,
            GmTicketIntent::AskStatus => ClientCommand::GmTicketSystemStatus,
            GmTicketIntent::Delete => ClientCommand::GmTicketDelete,
            GmTicketIntent::Write(write) => {
                let category = write.category;
                match client_command_for(write, map, pos) {
                    Some(cmd) => cmd,
                    None => {
                        warn!(
                            "gm ticket: refusing to file under category {category} — not a \
                             GMTicketCategory.dbc id (1..={GM_TICKET_CATEGORY_MAX})"
                        );
                        continue;
                    }
                }
            }
        });
    }
}

/// The category a `NewGMTicket`/`UpdateGMTicket` argument may put on the wire, or `None`.
///
/// **0 is legal and means "uncategorised"**. Our Help window has no category picker
/// — one click goes from Home to the text box — so it sends 0 deliberately. vmangos accepts it: its
/// whole validation is `if (packet.ticketType >= GMTICKET_MAX) return;` with `GMTICKET_MAX == 11`
/// (`GMTicketHandler.cpp:112`; the enum starts at `GMTICKET_STUCK = 1`, `SharedDefines.h:1776`),
/// and `GmTicket::GetTicketCategoryName` falls through its 1..10 switch to return **"Unknown"**
/// (`GMTicketMgr.cpp:205-232`). So the GM's notification and queue read "Unknown" — the honest
/// label, and far better than picking one of the ten real headings and mislabelling every ticket
/// as an Item or a Stuck.
///
/// **Above 10 is still refused**, and that is not tidiness: the server drops those *silently*, with
/// no response packet at all, so a bad argument would be indistinguishable from a filed ticket. The
/// range test is deliberately on the **`u32`, before the narrowing** — a category of 256 truncated
/// into a `u8` lands on 0 and would sail through as "uncategorised" when the caller meant something
/// else entirely.
fn category_for_wire(category: u32) -> Option<u8> {
    (category <= u32::from(GM_TICKET_CATEGORY_MAX)).then_some(category as u8)
}

/// One drained write → its packet, or `None` when [`category_for_wire`] refuses the category.
///
/// Split out so the verb choice is testable without a world: **the window's own `hasTicket` flag
/// picks the opcode**, and we must not re-derive it from whether we believe a ticket exists — our
/// belief can be stale by up to the 10-minute poll.
fn client_command_for(write: GmTicketWrite, map: u32, pos: [f32; 3]) -> Option<ClientCommand> {
    let category = category_for_wire(write.category)?;
    Some(if write.is_new {
        ClientCommand::GmTicketCreate {
            category,
            map,
            pos,
            text: write.text,
        }
    } else {
        ClientCommand::GmTicketUpdate {
            category,
            text: write.text,
        }
    })
}

/// The highest category the server will file — `GMTICKET_MAX - 1`, and also the last row of
/// `GMTicketCategory.dbc`. The two agree, which is why one constant serves both.
const GM_TICKET_CATEGORY_MAX: u8 = 10;

/// `GMTicketCategory.dbc`, loaded once — the ten rows `GetGMTicketCategories()` returns, already
/// flattened to the `(id, name)` pairs the binding hands Lua.
#[derive(Resource)]
pub(crate) struct GmTicketCategories(pub(crate) Vec<(u32, String)>);

/// Startup (after the MPQ chain opens): the ticket-category catalog.
///
/// Ordered `.after(AssetSet::Open)` for the reason `ui_items`' loader records — a bare `Startup`
/// slot races the chain open and, when it wins, silently leaves the resource absent for the whole
/// session. Absent here means the "page a GM" list paints no rows, which is a quiet and confusing
/// failure, so the ordering matters more than the warn line does.
fn load_gm_ticket_dbc(
    mut commands: Commands,
    world_assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match benilla_formats::load_gm_ticket_categories(&mut chain) {
        Ok(cat) => {
            info!(
                "gm ticket: GMTicketCategory.dbc loaded ({} categories)",
                cat.len()
            );
            commands.insert_resource(GmTicketCategories(
                cat.categories()
                    .iter()
                    .map(|c| (c.id, c.name.clone()))
                    .collect(),
            ));
        }
        Err(e) => warn!("gm ticket: GMTicketCategory.dbc failed to load: {e:#}"),
    }
}

/// The GM ticket's packet handlers (in the net handler table since 2312), beside
/// the state they drive.
pub(crate) mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use crate::net::NetHandlerApp;

    use super::GmTicketState;

    /// Register the ticket's handlers — called from [`super::UiGmTicketPlugin`]. One per kind,
    /// plus the session-end listener.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::GmTicket, on_ticket)
            .net_handler(K::GmTicketSystemStatus, on_system_status)
            .net_handler(K::GmTicketStatusUpdate, on_status_update)
            .net_handler(K::GmTicketCreated, on_written)
            .net_handler(K::GmTicketUpdated, on_written)
            .net_handler(K::GmTicketDeleted, on_deleted)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// The GETTICKET answer — EVERY one, including `None` ("you have no ticket") and including
    /// an unsolicited one pushed by a GM's `.ticket view`/`escalate`/`complete`: they are
    /// indistinguishable on the wire and want identical handling.
    fn on_ticket(In(ev): In<SessionEvent>, mut ticket: ResMut<GmTicketState>) {
        if let SessionEvent::GmTicket { ticket: answer } = ev {
            ticket.answer(answer);
        }
    }

    fn on_system_status(In(ev): In<SessionEvent>, mut ticket: ResMut<GmTicketState>) {
        if let SessionEvent::GmTicketSystemStatus { status } = ev {
            ticket.answer_queue(status);
        }
    }

    /// A GM touched the ticket. Value 1 makes the reference re-ask (`0x5e7932`), the same leg
    /// the create/update success codes take; 2 (closed) and 3 (survey offered) are recorded and
    /// not acted on — 3 is the survey trigger and that window is deferred. vmangos never sends
    /// this packet at all, so on our server the handler is dead; cmangos makes it the whole
    /// notification model, which is why it is parsed rather than dropped.
    fn on_status_update(In(ev): In<SessionEvent>, mut ticket: ResMut<GmTicketState>) {
        if let SessionEvent::GmTicketStatusUpdate { status } = ev {
            status_update(status, &mut ticket);
        }
    }

    /// The three response codes have no consumer in the shipped 1.12 UI — no event, no handler.
    /// Logged so a refusal is visible in a session log rather than silent; the `ERR_TICKET_*`
    /// display path is still unpinned. Create-ok (2) and update-ok (4) make the ENGINE re-ask
    /// for the ticket — the reference's own `0x5e4479` arm, and the reason the shipped UI needs
    /// no handler for either opcode. Without it a filed ticket goes unseen until the 10-minute
    /// poll.
    fn on_written(In(ev): In<SessionEvent>, mut ticket: ResMut<GmTicketState>) {
        match ev {
            SessionEvent::GmTicketCreated { response } => {
                write_response("create", response, 2, &mut ticket)
            }
            SessionEvent::GmTicketUpdated { response } => {
                write_response("update", response, 4, &mut ticket)
            }
            _ => {}
        }
    }

    fn on_deleted(In(ev): In<SessionEvent>) {
        if let SessionEvent::GmTicketDeleted { response: code } = ev {
            response("delete", code);
        }
    }

    /// The GM ticket is login-scoped, and for a sharper reason than most: the
    /// ticket belongs to the CHARACTER, and the next login may be a different one. Its answer
    /// counters go with it, so the first `SMSG_GMTICKET_GETTICKET` of the new session re-fires
    /// `UPDATE_TICKET` rather than being diffed away against the old character's answer count.
    /// A listener on the session end (a second handler on the kind, after the bridge's own teardown).
    fn on_session_end(In(_): In<SessionEvent>, mut ticket: ResMut<GmTicketState>) {
        ticket.clear_session();
    }

    /// `SMSG_GMTICKET_CREATE` (2 = created) and `SMSG_GMTICKET_UPDATETEXT` (4 = saved) — the two
    /// codes the reference engine answers by **re-asking for the ticket**, at `0x5e4479`.
    ///
    /// The shipped FrameXML registers no handler for either opcode, and that is not a gap: the
    /// engine turns the success code straight into a `CMSG_GMTICKET_GETTICKET`, whose answer fires
    /// the `UPDATE_TICKET` the window *does* listen for. So this arm is the whole reason filing a
    /// ticket updates the screen at all.
    ///
    /// Every other code is logged and nothing else. The reference surfaces the `ERR_TICKET_*`
    /// GlobalStrings on the failures; which string rides which code, and through which display
    /// path, is not yet pinned, and a guess here would put wrong words on screen.
    pub(crate) fn write_response(
        what: &str,
        response: u32,
        success: u32,
        ticket: &mut super::GmTicketState,
    ) {
        if response == success {
            debug!("gm ticket: {what} landed ({response}) — re-asking for the ticket");
            ticket.note_write_landed();
        } else {
            debug!("gm ticket: {what} answered {response}");
        }
    }

    /// `SMSG_GM_TICKET_STATUS_UPDATE` — a GM touched the ticket.
    ///
    /// **1 (updated) re-asks**, which is the reference's `0x5e7932` leg and the third of the three
    /// `CMSG_GMTICKET_GETTICKET` callers. 2 (closed) and 3 (a survey is offered) are logged: the
    /// close will arrive as an ordinary `UPDATE_TICKET` on the next poll either way, and the survey
    /// window is deferred (module doc), so acting on 3 would mean promising a panel that does not
    /// exist.
    pub(crate) fn status_update(status: u32, ticket: &mut super::GmTicketState) {
        match status {
            1 => {
                debug!("gm ticket: a GM updated the ticket — re-asking");
                ticket.note_write_landed();
            }
            2 => debug!("gm ticket: a GM closed the ticket"),
            3 => {
                debug!("gm ticket: a GM survey was offered — the survey window is deferred (1673)")
            }
            other => debug!("gm ticket: unknown status update {other}"),
        }
    }

    /// `SMSG_GMTICKET_DELETETICKET` — logged only. The reference has no re-ask leg here (its
    /// `0x5e4479` arm covers create and update alone), and the abandon path's own `GetGMTicket()`
    /// is the Lua dialog's, not the engine's.
    pub(crate) fn response(what: &str, response: u32) {
        debug!("gm ticket: {what} answered {response}");
    }
}

/// The GM trouble-ticket flow: the answer feed and the window's sends.
pub(crate) struct UiGmTicketPlugin;

impl Plugin for UiGmTicketPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<GmTicketState>()
            .init_resource::<GmTicketFeedState>()
            .add_systems(
                Startup,
                load_gm_ticket_dbc.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    feed_gm_ticket.in_set(UiFeed),
                    drain_gm_ticket.after(UiInput),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::GmTicket;

    fn ticket() -> Box<GmTicket> {
        Box::new(GmTicket {
            text: "Stuck in a rock.".into(),
            category: 1,
            ticket_age: 0.25,
            oldest_ticket_age: 2.5,
            update_time: 0.01,
            assigned_to_gm: 2,
            opened_by_gm: 1,
        })
    }

    /// The reorder, pinned: the event leads with the CATEGORY, the packet leads with the TEXT.
    /// This test is the guard on someone "simplifying" the two into agreement.
    #[test]
    fn the_event_args_lead_with_the_category_not_the_text() {
        let t = ticket();
        let args = update_ticket_args(Some(&t));
        assert_eq!(args[0], ScriptValue::Int(1), "arg1 is the category");
        assert_eq!(
            args[1],
            ScriptValue::Str("Stuck in a rock.".into()),
            "arg2 is the description"
        );
        assert_eq!(args.len(), 7, "arg1..arg7, the shipped window's whole read");
        assert_eq!(args[5], ScriptValue::Int(2), "arg6 assignedToGM");
        assert_eq!(args[6], ScriptValue::Int(1), "arg7 openedByGM");
    }

    /// "No ticket" is ONE argument and it is zero — the window's entire else-branch hangs on
    /// `arg1 and arg1 ~= 0`, so an empty arg list or a nil here would leave the Submit button
    /// reading "Save Changes" forever.
    #[test]
    fn the_no_ticket_answer_is_a_single_zero() {
        assert_eq!(update_ticket_args(None), vec![ScriptValue::Int(0)]);
    }

    /// A negative wait figure survives to Lua unclamped: it is the server saying "unavailable",
    /// and the window has a branch for exactly that (`arg4 < 0 or arg5 < 0`).
    #[test]
    fn an_unknown_wait_time_reaches_lua_as_a_negative_not_a_zero() {
        let mut t = ticket();
        t.oldest_ticket_age = -1.0;
        t.update_time = -1.0;
        let args = update_ticket_args(Some(&t));
        assert_eq!(args[3], ScriptValue::Number(-1.0));
        assert_eq!(args[4], ScriptValue::Number(-1.0));
    }

    /// An unchanged answer is still an answer. This is the whole reason the state carries a
    /// counter: the window polls every 10 minutes and must be re-fed each time, so a second
    /// identical `SMSG_GMTICKET_GETTICKET` has to move the ticket that the feed diffs on.
    #[test]
    fn a_repeated_identical_answer_still_counts_as_an_answer() {
        let mut state = GmTicketState::default();
        state.answer(Some(ticket()));
        let first = state.answers;
        state.answer(Some(ticket()));
        assert_ne!(state.answers, first, "the poll must re-fire UPDATE_TICKET");

        // And so does a repeated *empty* one — "still no ticket" is the common case.
        state.answer(None);
        let empty = state.answers;
        state.answer(None);
        assert_ne!(state.answers, empty);
    }

    /// **The engine re-asks after a write lands** — the reference's `0x5e4479` arm, and the reason
    /// the shipped UI needs no handler for `SMSG_GMTICKET_CREATE`/`UPDATETEXT` at all. Without this
    /// a filed ticket is invisible until the 10-minute poll: the toast stays dark and the form
    /// stays a form. The failure codes must NOT re-ask — there is nothing new to fetch.
    #[test]
    fn a_landed_write_makes_the_engine_reask_and_a_refused_one_does_not() {
        let mut state = GmTicketState::default();
        net::write_response("create", 2, 2, &mut state);
        net::write_response("update", 4, 4, &mut state);
        assert_eq!(state.take_reasks(), 2, "one re-ask per landed write");
        assert_eq!(state.take_reasks(), 0, "drained means drained");

        net::write_response("create", 3, 2, &mut state); // CREATE_ERROR
        net::write_response("update", 5, 4, &mut state); // UPDATE_ERROR
        assert_eq!(state.take_reasks(), 0, "a refusal has nothing to fetch");
    }

    /// The queue status reaches Lua **signed**. `-1` is the value the window branches on to raise
    /// "GM Help Tickets are currently unavailable"; read unsigned it would arrive as 4294967295 and
    /// that arm could never fire. vmangos only sends 0 or 1, so no live run would catch this.
    #[test]
    fn a_minus_one_queue_status_survives_to_lua_as_minus_one() {
        let mut state = GmTicketState::default();
        state.answer_queue(-1);
        assert_eq!(state.queue_status, -1);
        assert_eq!(i64::from(state.queue_status), -1_i64);
    }

    /// The socket dying resets both counters and the ticket, so a reconnect cannot show the
    /// previous session's ticket while the first poll is in flight.
    #[test]
    fn a_dead_session_forgets_the_ticket_and_both_answer_counts() {
        let mut state = GmTicketState::default();
        state.answer(Some(ticket()));
        state.answer_queue(1);
        state.clear_session();
        assert!(state.ticket.is_none());
        assert_eq!(state.answers, 0);
        assert_eq!(state.queue_answers, 0);
    }

    /// The window's own verb choice picks the opcode — never our belief about whether a ticket
    /// exists, which can be up to ten minutes stale.
    #[test]
    fn the_windows_verb_picks_the_opcode() {
        let write = |category, is_new| GmTicketWrite {
            category,
            text: "gone".into(),
            is_new,
        };
        assert!(matches!(
            client_command_for(write(4, true), 1, [1.0, 2.0, 3.0]),
            Some(ClientCommand::GmTicketCreate {
                category: 4,
                map: 1,
                ..
            })
        ));
        assert!(matches!(
            client_command_for(write(4, false), 1, [1.0, 2.0, 3.0]),
            Some(ClientCommand::GmTicketUpdate { category: 4, .. })
        ));
    }

    /// **0 rides the wire; anything above 10 does not**.
    ///
    /// 0 is what our own window sends, because it has no category picker — vmangos renders it
    /// "Unknown" rather than refusing it. Above 10 the server drops the packet *silently*, so
    /// letting one through would be indistinguishable from a filed ticket.
    ///
    /// The `256` case is the one worth having: it truncates to 0 in a `u8`, so a check written
    /// after the narrowing would accept it as "uncategorised" when the caller meant category 256.
    #[test]
    fn zero_is_uncategorised_and_anything_above_ten_is_refused() {
        assert_eq!(category_for_wire(0), Some(0), "the uncategorised ticket");
        for good in 1..=10 {
            assert_eq!(category_for_wire(good), Some(good as u8));
        }
        for bad in [11, 255, 256, 99_999] {
            assert_eq!(
                category_for_wire(bad),
                None,
                "category {bad} must not be sent"
            );
        }

        // And the whole path, not just the helper: a refused category produces no packet.
        let write = |category| GmTicketWrite {
            category,
            text: "x".into(),
            is_new: true,
        };
        assert!(client_command_for(write(256), 1, [0.0; 3]).is_none());
        assert!(matches!(
            client_command_for(write(0), 1, [0.0; 3]),
            Some(ClientCommand::GmTicketCreate { category: 0, .. })
        ));
    }
}
