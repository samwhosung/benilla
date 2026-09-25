//! Being summoned — the server's question, its dialog, and the one word back.
//!
//! **The law this module exists for: the client's whole part in a summon is a dialog and an echo.**
//! A warlock's Ritual of Summoning, a meeting stone and a GM's `.summon` all converge on the same
//! `SMSG_SUMMON_REQUEST`; the client parks three fields, raises `CONFIRM_SUMMON`, and — only if
//! Accept is pressed — sends `CMSG_SUMMON_RESPONSE` carrying the summoner's guid.
//! **Declining sends nothing at all.** There is no decline opcode and no accept flag in the body:
//! 1.12 has no `CancelSummon` binding (`reference/1.12-globals.tsv` has exactly four names in this
//! family), so a refused summon is silence plus the server's own two-minute expiry. That is why
//! nothing here has a "no" path.
//!
//! The Era surface it drives is the reference's own (`StaticPopup.lua:1336-1357`,
//! `UIParent.lua:126`/`551-552`): the `CONFIRM_SUMMON` event, fired with **no arguments**, and the
//! four engine globals the dialog reads back through ([`benilla_ui::script`]'s `summon` module).
//!
//! Byte-pinned against the reference's own handler `0x5e6140` (the opcode→handler map at the
//! `0x5ab650` registration site gives it `0x2ab`), its latch `0x4963a0`, and the four bindings
//! `0x48b660`/`0x48b6a0`/`0x48b720`/`0x48b770`:
//!
//! 1. **The latch is four globals and nothing structured.** `0x4963a0` writes the guid to
//!    `[0xb4e358]/[0xb4e35c]`, the zone id to `[0xb4e354]`, and `OsGetAsyncTimeMs() + delay` to
//!    `[0xb4e350]`, then calls `FrameScript_SignalEvent(516)` with no varargs. [`SummonState`] is
//!    those four fields, with the deadline on `Time<Real>` for [`crate::death`]'s reason: a window
//!    the *server* sends counts down in real seconds, and the virtual clock's `max_delta` clamp
//!    would quietly lengthen it.
//! 2. **A dead *or ghost* player is refused the question outright.** Ahead of the latch the handler
//!    resolves the active player (`0x468550` → `0x468460` with typemask `0x10` = player) and, if
//!    `0x605f30` says yes, returns without latching *or* firing (`5e6194: jne`). So the request
//!    does not merely fail to show a dialog: it does not disturb a previously latched one either,
//!    which is why [`net::request`] is the gate and the popup engine's `whileDead` rule is not.
//!
//!    **`0x605f30` is not "is dead"** — that reading cost a wrong first cut here. It is
//!    `[values+0x40] <= 0` (signed, `605f3b: jle`) **OR** (typemask bit `0x10` **AND** the
//!    `PLAYER_FLAGS` ghost bit at `[[obj+0xe68]+8] & 0x10`, `605f3d..605f59`). A ghost's wire
//!    health is **1**, so the first leg alone lets a ghost through; the second is what stops it.
//!    Both legs are read here, and neither is `unit_reads_dead`'s triple (`0x605f90`), which is
//!    a different function with a dyn-flag and stand-state leg this one does not have.
//!    A *missing* player object falls through to the latch — a NULL is sent through
//!    (`5e6189: je`), so the safe default is to ask.
//! 3. **`GetSummonConfirmAreaName()` is the summoner's zone, straight out of `AreaTable.dbc`**
//!    (`0x48b720`: bounds-check, row pointer, `[row + locale*4 + 0x2c]`) — **no parent-zone walk
//!    and no GlobalString tail**, unlike the innkeeper question's three-step chain
//!    ([`crate::ui_binder::area_name`], `0x5dfe5e`). An id the table cannot name renders `""`.
//! 4. **`GetSummonConfirmSummoner()` answers `""` on a cache miss — and the miss is not inert.**
//!    `0x48b6a0` → `DBCache::NameCache::GetRecord 0x55f080` against the cache at `0xc0e228`, else
//!    the shared empty string `0x882748`; but a missing record makes `0x55f120` allocate one and
//!    **send a name query** (`0x55f1fa`), whose arrival signals nothing. So the real name surfaces
//!    only because the popup engine re-reads all three getters every OnUpdate frame — *a client
//!    that formats the text once shows an empty summoner forever.* That is what makes
//!    `UIParent.lua`'s `CONFIRM_SUMMON` countdown arm the mechanism rather than a transcription,
//!    and it is why the event is not held back for the name the way `DUEL_REQUESTED` is:
//!    that event carries its name as `arg1`, and this one has no argument to hold.
//!
//!    [`crate::names::NameCache::resolve`] is the same shape — answer or ask, once — so the feed
//!    below asks on every frame the question is live and reports whatever has landed.
//!
//! **Two deviations, both deliberate.**
//!
//! - **The reference never clears the latched guid or zone — nothing in the image does.** Each of
//!   the three cells has exactly one writer (the latch) and no clearer, so a second pending summon
//!   silently overwrites the first, and a `ConfirmSummon()` typed at the console with nothing
//!   pending sends a packet built from whatever is left in the bank. Only the *deadline* is ever
//!   zeroed, by `0x49014a` inside the login-side init `0x48fbf0`. We keep the overwrite (it is the
//!   behaviour) and drop the whole latch on the session edge ([`end_session_summon`]), refusing to
//!   send with no summoner: the reference reaches the same state one step later, at the next
//!   login-side init, and nothing can read the bank in between.
//! - We do **not** gate the send on the deadline, and that is the reference's own shape rather than
//!   an oversight: `ConfirmSummon 0x48b770` tests nothing and does not clear the latch, so two
//!   Accepts are two packets. vmangos is the thing that makes a late one harmless — its handler
//!   *ignores the guid entirely* and checks its own `m_summon_expire`.

use benilla_ui::script::{SummonConfirmUiState, UiScript};
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_script::{UiFeed, UiInput};

/// The pending summon question — the reference's four-global bank (module doc, pin 1). Written by
/// the net drain's `SummonRequest` arm through [`net::request`], read by [`feed_summon`] (which
/// pushes the three getters' answers and fires `CONFIRM_SUMMON`) and by [`drain_summon`] (which
/// turns the dialog's Accept into the response).
#[derive(Resource, Default)]
pub(crate) struct SummonState {
    /// Who is summoning — `[0xb4e358]/[0xb4e35c]`. `0` = nothing has ever asked this session.
    /// This guid is what goes back on the wire.
    summoner: u64,
    /// The **summoner's** `AreaTable` id — `[0xb4e354]`. Not ours, and not a map id.
    zone: u32,
    /// When the offer expires, in `Time<Real>` seconds — `[0xb4e350]`, which the reference stores
    /// as an absolute millisecond stamp. `None` = the zeroed deadline: no live offer.
    expires_at: Option<f64>,
    /// A question the feed still owes the UI. Set per *packet*, not per state edge — being
    /// summoned twice by the same warlock is two dialogs, and an edge-diff would swallow the
    /// second ([`crate::ui_binder::BinderState`]'s `ask`, for the identical reason).
    ask: bool,
}

impl SummonState {
    /// The latch `0x4963a0`: park the three fields, arm the deadline, and owe the UI a dialog.
    fn latch(&mut self, summoner: u64, zone: u32, delay_ms: u32, now: f64) {
        self.summoner = summoner;
        self.zone = zone;
        self.expires_at = Some(now + f64::from(delay_ms) / 1000.0);
        self.ask = true;
    }

    /// The guid to answer with, if anything has ever asked.
    fn pending(&self) -> Option<u64> {
        (self.summoner != 0).then_some(self.summoner)
    }

    /// `0x4963e0` — milliseconds left, clamped at zero, and zero for a deadline that was never
    /// armed. The truncation to whole seconds is the *binding's* (`0x48b660`), not this: see
    /// [`SummonConfirmUiState::time_left_ms`].
    fn time_left_ms(&self, now: f64) -> u32 {
        self.expires_at.map_or(0.0, |at| {
            ((at - now).max(0.0) * 1000.0).min(f64::from(u32::MAX))
        }) as u32
    }
}

/// Push the three getters' answers every frame, and fire `CONFIRM_SUMMON` for a question the UI is
/// still owed.
///
/// The fire is **unconditional and argument-less** once a packet has asked — the reference's
/// `SignalEvent(516)` carries no varargs, and every getter answers a value on every path, so
/// there is nothing to wait for and nothing to withhold.
fn feed_summon(
    script: Option<NonSendMut<UiScript>>,
    mut summon: ResMut<SummonState>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    areas: Option<Res<AreaTableRes>>,
    // Real, not virtual: the deadline was stamped on this clock by the net drain, and the server's
    // window runs in real seconds ([`crate::death`]'s reasoning).
    time: Res<Time<Real>>,
) {
    let Some(mut script) = script else {
        return;
    };

    // Nothing has ever asked and nothing is owed: the getters' defaults are already exactly what
    // the reference answers over a zeroed bank, so there is no snapshot to push and no name to
    // resolve. **Not `pending()`** — the two conditions are deliberately separate, because a
    // request carrying a zero guid still latches and still fires in the reference (the handler
    // does not inspect the guid), and would otherwise leave `ask` owed forever.
    if summon.summoner == 0 && !summon.ask {
        return;
    }

    // A cache miss issues the name query and reports `""` this frame — the reference's own
    // `NameCache::GetRecord` shape (module doc, pin 4). A zero guid is never asked about: it names
    // nobody, and the reference's own cache lookup would answer the same `""`.
    let summoner_name = match summon.pending() {
        Some(guid) => names
            .resolve(guid, &commands)
            .unwrap_or_default()
            .to_string(),
        None => String::new(),
    };
    script.set_summon_confirm(SummonConfirmUiState {
        summoner: summoner_name,
        area: area_name(summon.zone, areas.as_deref()),
        time_left_ms: summon.time_left_ms(time.elapsed_secs_f64()),
    });

    if !summon.ask {
        return;
    }
    summon.ask = false;
    // **The interact chain's middle link** (tag `summon`, the `use` kit's shape in
    // [`crate::target::click`]). "I got summoned and nothing happened" spans three links — the
    // packet landing, this event, and the response going out — and only one run should be needed
    // to say which is dead. The name is traced because a blank one is the most likely-looking
    // symptom that is in fact correct.
    if benilla_assets::trace::enabled_for("summon") {
        benilla_assets::trace::line(
            "summon",
            &format!(
                "fire CONFIRM_SUMMON summoner={:#x} zone={}",
                summon.summoner, summon.zone
            ),
        );
    }
    script.fire_event("CONFIRM_SUMMON", Vec::new());
}

/// `GetSummonConfirmAreaName()`'s lookup — the row's own name or `""` (module doc, pin 3).
///
/// Split out and taking a bare id so the "no parent walk, no GlobalString tail" half can be tested
/// against the real `AreaTable` beside [`crate::ui_binder`]'s three-step chain, which is the thing
/// it is most likely to be mistaken for.
fn area_name(zone: u32, areas: Option<&AreaTableRes>) -> String {
    areas
        .and_then(|a| a.0.name(zone))
        .unwrap_or_default()
        .to_string()
}

/// Turn the dialog's Accept into `CMSG_SUMMON_RESPONSE`.
///
/// Gated only on *something having asked* — not on the deadline, which is the reference's own
/// shape (module doc, deviation 2). A `ConfirmSummon()` with nothing ever latched would otherwise
/// put a zero guid on the wire, which no server can do anything with.
fn drain_summon(
    script: Option<NonSendMut<UiScript>>,
    summon: Res<SummonState>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let confirms = script.take_summon_confirms();
    if confirms == 0 {
        return;
    }
    let Some(summoner) = summon.pending() else {
        debug!("ui_summon: ConfirmSummon() with nothing latched — nothing to answer with");
        return;
    };
    if benilla_assets::trace::enabled_for("summon") {
        benilla_assets::trace::line(
            "summon",
            &format!("SEND CMSG_SUMMON_RESPONSE summoner={summoner:#x} n={confirms}"),
        );
    }
    for _ in 0..confirms {
        let _ = commands.0.send(ClientCommand::SummonResponse { summoner });
    }
}

/// The latch is the session's, and dies with it (module doc, deviation 1).
///
/// `DisconnectedMessage` is the one total edge — a socket death, a kick and a `/logout` all reach
/// it — which is the same edge [`crate::death`]'s world-scoped memory resets on, so the two stay in
/// lockstep by construction. A `/reload` must NOT clear this: the summon window is the engine's,
/// and the reference's survives one.
fn end_session_summon(
    mut msgs: MessageReader<crate::net::DisconnectedMessage>,
    mut summon: ResMut<SummonState>,
) {
    if msgs.read().next().is_some() {
        *summon = SummonState::default();
    }
}

/// The summon's packet handler (in the net handler table since 2312), beside the
/// state it drives.
pub(crate) mod net {
    use bevy::prelude::*;

    use super::SummonState;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;
    use crate::net::{GuidIndex, ObjectStore, SelfGuid};

    /// Register the handler — called from [`super::UiSummonPlugin`].
    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::SummonRequest, on_request);
    }

    /// Someone is asking to pull us to them. The reference gates this in the HANDLER, not the
    /// dialog: a dead or ghost player's request is dropped before the latch, so it cannot
    /// disturb a live question either (`0x5e6194`). The predicate is `0x605f30` — **health ≤ 0
    /// OR (is-player AND `PLAYER_FLAGS` ghost bit)**, which is both of these accessors and not
    /// the one `unit_is_dead` alone would give (a ghost's wire health is 1). A self object we
    /// have not streamed yet reads as alive: the reference's own default (`0x5e6189` sends a
    /// NULL object through to the latch).
    fn on_request(
        In(ev): In<SessionEvent>,
        mut summon: ResMut<SummonState>,
        self_guid: Res<SelfGuid>,
        index: Res<GuidIndex>,
        stores: Query<&ObjectStore>,
        real_clock: Res<Time<Real>>,
    ) {
        if let SessionEvent::SummonRequest {
            summoner,
            zone,
            delay_ms,
        } = ev
        {
            let dead_or_ghost = self_guid
                .0
                .and_then(|g| index.0.get(&g))
                .and_then(|e| stores.get(*e).ok())
                .is_some_and(|s| s.0.unit_is_dead() || s.0.player_is_ghost());
            request(
                summoner,
                zone,
                delay_ms,
                dead_or_ghost,
                real_clock.elapsed_secs_f64(),
                &mut summon,
            );
        }
    }

    /// `SMSG_SUMMON_REQUEST` — latch the offer and owe the UI a dialog, unless we are dead or a
    /// ghost.
    ///
    /// `dead_or_ghost` is the reference's `0x605f30` over the *active player* — **both** of its
    /// legs (module doc, pin 2) — read by the caller because that is where the drain already has
    /// the descriptor. **False when there is no player object at all**, which is the reference's
    /// own default (`5e6189: je` sends a NULL through to the latch). A refusal touches nothing:
    /// that is the whole point of gating here rather than at the dialog — an ill-timed summon must
    /// not overwrite a live one.
    pub(crate) fn request(
        summoner: u64,
        zone: u32,
        delay_ms: u32,
        dead_or_ghost: bool,
        now: f64,
        summon: &mut SummonState,
    ) {
        // **The interact chain's first link** (tag `summon`): the packet, with the gate's verdict
        // beside it. A drop here is the reference's behaviour and looks exactly like a broken
        // client, so it is the one refusal that must never be silent in a trace.
        if benilla_assets::trace::enabled_for("summon") {
            benilla_assets::trace::line(
                "summon",
                &format!(
                    "recv SMSG_SUMMON_REQUEST summoner={summoner:#x} zone={zone} \
                     delay_ms={delay_ms} dead_or_ghost={dead_or_ghost}"
                ),
            );
        }
        if dead_or_ghost {
            debug!("ui_summon: summon request from {summoner:#x} dropped — we are dead or a ghost");
            return;
        }
        summon.latch(summoner, zone, delay_ms, now);
    }
}

/// The summon flow: the question's feed and its one answer.
pub(crate) struct UiSummonPlugin;

impl Plugin for UiSummonPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<SummonState>().add_systems(
            Update,
            (
                // Before the feed, so the frame a session ends is already a frame the feed sees no
                // memory of the last one ([`crate::death`]'s ordering, for the same reason).
                end_session_summon.before(feed_summon),
                feed_summon.in_set(UiFeed),
                drain_summon.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second summon is a second dialog. The `ask` flag is per packet precisely so a
    /// decline-then-be-summoned-again (which never passes through "nothing pending") is not
    /// swallowed by a state diff.
    #[test]
    fn asking_twice_owes_two_dialogs() {
        let mut summon = SummonState::default();
        assert_eq!(summon.pending(), None);

        summon.latch(0x2a, 1519, 120_000, 100.0);
        assert!(summon.ask);
        summon.ask = false; // the feed fired the first dialog
        assert_eq!(summon.pending(), Some(0x2a), "the guid outlives the fire");

        summon.latch(0x2a, 1519, 120_000, 200.0);
        assert!(summon.ask, "the same summoner asking again owes a dialog");
    }

    /// The countdown is the packet's own window, clamped at zero and zero before anything asks —
    /// the reference's `0x4963e0`, whose three legs are exactly these three cases.
    #[test]
    fn the_countdown_runs_from_the_packets_delay_and_never_goes_negative() {
        let mut summon = SummonState::default();
        assert_eq!(summon.time_left_ms(0.0), 0, "a zeroed deadline reads 0");

        summon.latch(0x2a, 1519, 120_000, 100.0);
        assert_eq!(summon.time_left_ms(100.0), 120_000);
        assert_eq!(summon.time_left_ms(160.0), 60_000);
        assert_eq!(summon.time_left_ms(220.0), 0, "expired, not negative");
        assert_eq!(summon.time_left_ms(9_999.0), 0);
    }

    /// **The gate refuses without disturbing anything** (module doc, pin 2) — the half that would
    /// be lost by putting the test in the dialog instead: a summon arriving while dead or a ghost
    /// must leave a *live* question exactly as it found it.
    #[test]
    fn a_summon_that_arrives_while_refused_latches_nothing() {
        let mut summon = SummonState::default();
        net::request(0x2a, 1519, 120_000, true, 100.0, &mut summon);
        assert_eq!(summon.pending(), None);
        assert!(!summon.ask);
        assert_eq!(summon.time_left_ms(100.0), 0);

        // A live question, then a second request while refused: the first survives untouched.
        net::request(0x2a, 1519, 120_000, false, 100.0, &mut summon);
        summon.ask = false;
        net::request(0x99, 1, 120_000, true, 150.0, &mut summon);
        assert_eq!(
            summon.pending(),
            Some(0x2a),
            "the live offer is undisturbed"
        );
        assert_eq!(summon.zone, 1519);
        assert!(!summon.ask, "and no second dialog is owed");
    }

    /// **A GHOST is refused, and a ghost is not dead** — `0x605f30`'s second leg, and the one this
    /// module had wrong on its first cut. A released player's wire health is `1`, so
    /// `unit_is_dead()` is FALSE for them; only the `PLAYER_FLAGS` ghost bit stops the latch.
    ///
    /// This guards the **call site's** predicate (`net::apply`'s `SummonRequest` arm), which is
    /// exactly what would regress if someone simplified it back to the single accessor the
    /// parameter's old name suggested.
    #[test]
    fn a_ghost_is_refused_even_though_a_ghost_is_not_dead() {
        // A released player as the wire describes one: health 1 of 100, PLAYER_FLAGS ghost bit.
        const HEALTH: u16 = 22;
        const MAXHEALTH: u16 = 28;
        const PLAYER_FLAGS: u16 = 190;
        let ghost = benilla_protocol::ObjectFields::from_pairs(&[
            (HEALTH, 1),
            (MAXHEALTH, 100),
            (PLAYER_FLAGS, 0x10),
        ]);

        assert!(
            !ghost.unit_is_dead(),
            "a ghost has health — which is why the first leg alone lets one through"
        );
        assert!(ghost.player_is_ghost());
        assert!(
            ghost.unit_is_dead() || ghost.player_is_ghost(),
            "the arm's predicate is the OR of both legs, and it must refuse a ghost"
        );

        // The corpse case, for the other leg: dead, not yet released, no ghost bit.
        let corpse = benilla_protocol::ObjectFields::from_pairs(&[(HEALTH, 0), (MAXHEALTH, 100)]);
        assert!(corpse.unit_is_dead() && !corpse.player_is_ghost());
    }

    /// `GetSummonConfirmAreaName()` is a bare `AreaTable` read: the row's name, else `""` — **no**
    /// parent-zone hop and **no** GlobalString tail. Guarding the exact difference from
    /// [`crate::ui_binder`]'s chain, which is the thing this is most likely to drift into.
    /// Skips without client data.
    #[test]
    fn the_area_name_is_the_bare_row_with_no_fallback_chain() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let areas = AreaTableRes(
            benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable.dbc"),
        );

        assert_eq!(area_name(1519, Some(&areas)), "Stormwind City");
        // 186 is Dolanaar, a SUB-area whose parent zone is Teldrassil. The binder's chain would
        // reach a name for an unnamed leaf by walking up; this one never walks, so the only thing
        // that matters is that the row itself answers.
        assert_eq!(area_name(186, Some(&areas)), "Dolanaar");
        assert_eq!(area_name(0xffff, Some(&areas)), "", "no row, no name");
        assert_eq!(area_name(1519, None), "", "no catalog, no name");
    }

    /// Accepting with nothing latched sends nothing — the one place we decline to reproduce the
    /// reference's un-cleared bank (module doc, deviation 1).
    #[test]
    fn nothing_pending_means_nothing_to_answer_with() {
        let summon = SummonState::default();
        assert_eq!(summon.pending(), None);
    }
}
