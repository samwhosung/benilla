//! The GM ticket flow behind the stock Help window: the events it hears and the packets it sends.
//! The client holds no ticket: `SMSG_GMTICKET_GETTICKET` is the only truth, and every answer fires
//! `UPDATE_TICKET`, an unchanged one too, as the window re-polls `GetGMTicket()` every 10 minutes.
//! 1.12 has no response channel, so vmangos appends a GM's reply to the ticket text
//! (`GMTicketMgr.cpp:124-136`).
//!
//! Every send is fire-and-forget and never retried: vmangos answers some refusals with silence
//! (`GMTicketHandler.cpp:91,106-113`), and a third `CMSG_GMTICKET_UPDATETEXT` in one world tick
//! trips its anti-flood kick (`WorldSession.cpp:1316-1342`).
//!
//! `SMSG_GM_TICKET_STATUS_UPDATE` is cmangos's; vmangos never sends it. Its status 2 fires the
//! no-ticket `UPDATE_TICKET` and status 3 `GMSURVEY_DISPLAY`, which `HelpFrame` hears
//! (`HelpFrame.xml:744`, `HelpFrame.lua:391`).

use benilla_ui::script::{GmTicketIntent, GmTicketWrite, ScriptValue, UnservedSend};
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;

use crate::net::{ClientCommand, NetCommands};
use crate::player::Player;
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_dialog_verbs::DialogLane;
use crate::ui_script::{UiFeed, VmMemo};

/// The engine's re-asks a landed write owes. The client holds no ticket: each answer signals at
/// its packet ([`ticket_answer`], [`status_answer`]).
#[derive(Resource, Default)]
pub(crate) struct GmTicketState {
    /// Re-asks the engine owes ([`Self::note_write_landed`]), sent by [`send_gm_reasks`].
    engine_reasks: u32,
}

impl GmTicketState {
    /// A write landed: re-ask for the ticket, as the reference engine does, once per push and
    /// unguarded, on create-ok (2) and update-ok (4) at `0x5e4479` and on status update 1 at
    /// `0x5e7932`. Without it a filed ticket stays unseen until the 10-minute poll.
    pub(crate) fn note_write_landed(&mut self) {
        self.engine_reasks = self.engine_reasks.saturating_add(1);
    }

    fn take_reasks(&mut self) -> u32 {
        std::mem::take(&mut self.engine_reasks)
    }

    /// Session end: a reconnect owes the previous character's ticket nothing.
    pub(crate) fn clear_session(&mut self) {
        *self = Self::default();
    }
}

/// Per VM ([`VmMemo`]): a `/reload`'s new VM is pushed the categories and the survey afresh.
#[derive(Resource, Default)]
struct GmTicketFeedState {
    vm: VmMemo<FedTicket>,
}

#[derive(Default)]
struct FedTicket {
    categories_pushed: bool,
    survey_pushed: bool,
}

/// Push the categories and the survey tables, `GMTicketCategory.dbc`'s and the survey's, once per
/// VM.
fn feed_gm_ticket(
    mut hosted_vm: crate::script_calls::HostedVm,
    categories: Option<Res<GmTicketCategories>>,
    survey: Option<Res<GmSurveyResource>>,
    mut feed: ResMut<GmTicketFeedState>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let fed = feed.vm.get(script);

        if !fed.categories_pushed {
            if let Some(categories) = categories.as_deref() {
                script.set_gm_ticket_categories(categories.0.clone());
                fed.categories_pushed = true;
            }
        }

        if !fed.survey_pushed {
            if let Some(survey) = survey.as_deref() {
                script.set_gm_survey_tables(survey.0.clone());
                fed.survey_pushed = true;
            }
        }
    })
}

/// `SMSG_GMTICKET_GETTICKET`'s `UPDATE_TICKET` (`0x5e4527`), fired for every answer, an unchanged
/// one too, as the window re-polls `GetGMTicket()` every 10 minutes. Status 0 shows
/// `ERR_TICKET_DB_ERROR` first (`0x5e44fb`-`0x5e4509`), then the no-ticket answer like any other
/// status but 6.
fn ticket_answer(
    lane: &mut DialogLane,
    errors: &UiErrorKeys,
    status: u32,
    ticket: Option<&benilla_protocol::messages::GmTicket>,
) {
    if status == 0 {
        errors.push(UiError::key("ERR_TICKET_DB_ERROR"));
    }
    lane.event("UPDATE_TICKET", update_ticket_args(ticket));
}

/// `SMSG_GMTICKET_DELETETICKET` (`0x5e4606`): result 9 fires `UPDATE_TICKET` with `%d%s`, 0 and
/// `""` (`0x5e461d`); any other result shows `ERR_TICKET_DB_ERROR` (`0x5e4635`).
fn deleted(lane: &mut DialogLane, errors: &UiErrorKeys, code: u32) {
    if code == 9 {
        lane.event(
            "UPDATE_TICKET",
            vec![ScriptValue::Int(0), ScriptValue::Str(String::new())],
        );
    } else {
        errors.push(UiError::key("ERR_TICKET_DB_ERROR"));
    }
}

/// `SMSG_GMTICKETSYSTEMSTATUS`'s `UPDATE_GM_STATUS` (`0x5e4689`): `arg1` the status as a number,
/// signed as the reference copies the wire dword (`0x418e95`) and loads it for Lua (`0x704fa6`), so
/// `HelpFrame`'s `arg1 == -1` arm fires on `0xFFFFFFFF`; vmangos sends only 0 or 1.
fn status_answer(lane: &mut DialogLane, status: i32) {
    lane.event(
        "UPDATE_GM_STATUS",
        vec![ScriptValue::Int(i64::from(status))],
    );
}

/// `UPDATE_TICKET`'s arguments: category, text, the three day figures, assigned, opened. The wire
/// has the text first and the reference's handler reorders as it fires, so the decoder keeps wire
/// order. `HelpFrameOpenTicket_OnEvent` tests `arg1 and arg1 ~= 0` (`HelpFrame.lua:403`), and its
/// else branch resets the form (`:468-475`).
fn update_ticket_args(ticket: Option<&benilla_protocol::messages::GmTicket>) -> Vec<ScriptValue> {
    let Some(t) = ticket else {
        return no_ticket_args();
    };
    vec![
        ScriptValue::Int(i64::from(t.category)),
        ScriptValue::Str(t.text.clone()),
        // The first figure, below 0 stored as 0, NaN and -0 passing (`0x5e4568`-`0x5e4578`: a
        // `fcomp` against 0.0, then `jp` over the store); it is `ticket_age`, which the window
        // subtracts from the wait estimate. The other two stay signed: a negative is the server's
        // "unknown", shown as `GM_TICKET_UNAVAILABLE` (`HelpFrame.lua:442`).
        ScriptValue::Number(f64::from(if t.ticket_age < 0.0 {
            0.0
        } else {
            t.ticket_age
        })),
        ScriptValue::Number(f64::from(t.oldest_ticket_age)),
        ScriptValue::Number(f64::from(t.update_time)),
        ScriptValue::Int(i64::from(t.assigned_to_gm)),
        ScriptValue::Int(i64::from(t.opened_by_gm)),
    ]
}

/// The no-ticket `UPDATE_TICKET` (`0x5e450c`, `0x5e7945`): seven dwords 0, `""`, -1, -1, -1, 0, 0
/// under the format `%d%s%f%f%f%d%d` (`0x8603e4`), which reads ten. A `%f` takes eight bytes
/// (`fld qword` at `0x704fe3`), so arg3 is `0xFFFFFFFFFFFFFFFF`, a NaN, and arg4
/// `0x00000000FFFFFFFF`, a denormal; the bits are kept, so `tostring` reads `-1.#QNAN`. Arg5's
/// high half and both `%d` are read from past the pushed arguments: on the fetch path (esp at
/// `0x5e450c`) the handler's saved edi, saved esi and its local `[ebp-0x9b8]`, on status 2
/// (`0x5e7945`) the saved ebp, the return address into the dispatcher and the handler's first
/// argument.
///
/// Deviation: those three are stack residue, so benilla sends 0 for each.
fn no_ticket_args() -> Vec<ScriptValue> {
    vec![
        ScriptValue::Int(0),
        ScriptValue::Str(String::new()),
        ScriptValue::Number(f64::from_bits(u64::MAX)),
        ScriptValue::Number(f64::from_bits(0xFFFF_FFFF)),
        ScriptValue::Number(0.0),
        ScriptValue::Int(0),
        ScriptValue::Int(0),
    ]
}

/// The engine's re-asks ([`GmTicketState::note_write_landed`]): they answer a push that arrived
/// before this frame's input.
fn send_gm_reasks(mut reask: ResMut<GmTicketState>, commands: Res<NetCommands>) {
    for _ in 0..reask.take_reasks() {
        let _ = commands.0.send(ClientCommand::GmTicketGet);
    }
}

/// The Help window's ticket verbs and the unserved systems' sends inside their calls
/// ([`benilla_ui::script::GmTicketCall`], [`benilla_ui::script::ScriptCall::Unserved`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GmTicketSteps<'w> {
    commands: Res<'w, NetCommands>,
    player: Option<Res<'w, Player>>,
    map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
    /// The book, which holds `Stuck()`'s spell.
    actions: Option<Res<'w, crate::ui_action::PlayerActions>>,
}

impl GmTicketSteps<'_> {
    /// `[0xb70098]`, the spell `Stuck()` casts ([`crate::ui_action::PlayerActions::stuck_spell`]):
    /// 7355 "Stuck" in the shipped `Spell.dbc`, which moves the player to their last safe
    /// position (vmangos `SpellEffects.cpp:4697`). `None` is 0, no such spell learned.
    pub(crate) fn stuck_spell(&self) -> Option<u32> {
        self.actions.as_deref()?.stuck_spell
    }

    /// A ticket verb's packet, sent in its call, so call order is wire order:
    /// `DeleteGMTicket(); GetGMTicket()` never becomes get-then-delete.
    pub(crate) fn ticket(&self, intent: GmTicketIntent) {
        let _ = self.commands.0.send(match intent {
            GmTicketIntent::Ask => ClientCommand::GmTicketGet,
            GmTicketIntent::AskStatus => ClientCommand::GmTicketSystemStatus,
            GmTicketIntent::Delete => ClientCommand::GmTicketDelete,
            GmTicketIntent::Write(write) => {
                // Where the player stands at send time, in WoW coordinates: where `.ticket go`
                // takes the GM.
                let pos = self
                    .player
                    .as_deref()
                    .map(|p| bevy_to_wow(p.pos))
                    .unwrap_or([0.0; 3]);
                let map = self.map.as_deref().map(|m| m.0).unwrap_or(0);
                let category = write.category;
                match client_command_for(write, map, pos) {
                    Some(cmd) => cmd,
                    None => {
                        warn!(
                            "gm ticket: refusing to file under category {category} — not a \
                             GMTicketCategory.dbc id (1..={GM_TICKET_CATEGORY_MAX})"
                        );
                        return;
                    }
                }
            }
        });
    }

    /// A lottery or survey verb's send (the lottery has no module of its own, its window being
    /// unbuilt), or `BuySkillTier`'s.
    pub(crate) fn unserved(&self, send: UnservedSend) {
        let _ = self.commands.0.send(match send {
            UnservedSend::SkillBuyStep { skill_id } => ClientCommand::SkillBuyStep { skill_id },
            UnservedSend::LotteryQuery => ClientCommand::LotteryQuery,
            UnservedSend::BuyLotteryTicket { first, second } => {
                ClientCommand::BuyLotteryTicket { first, second }
            }
            UnservedSend::GmSurveySubmit {
                survey,
                answers,
                comment,
            } => ClientCommand::GmSurveySubmit {
                survey,
                answers: answers
                    .into_iter()
                    .map(|a| (a.question, a.rating, a.comment))
                    .collect(),
                comment,
            },
        });
    }
}

/// `Stuck()` past its gate (`0x489370`): `CastSpellAsActivePlayer` (`0x6e5a90`) on `spell`, its
/// [`GmTicketSteps::stuck_spell`], with no item and no target (`0x489376`, `0x489378`), into the
/// cast attempt.
pub(crate) fn cast_stuck(cast: &mut crate::spell::ScriptCast, spell: u32) {
    let ctx = cast.targeting.context();
    cast.ladder
        .send(spell, &ctx, crate::spell::CastCommit::Spell);
}

/// The category a `NewGMTicket`/`UpdateGMTicket` call may put on the wire, 0 to 10. vmangos files
/// 0 as "Unknown" (`GMTicketMgr.cpp:205-232`) and drops 11 and up with no answer at all
/// (`GMTicketHandler.cpp:112`), so the test runs on the `u32`, where 256 cannot wrap to 0.
fn category_for_wire(category: u32) -> Option<u8> {
    (category <= u32::from(GM_TICKET_CATEGORY_MAX)).then_some(category as u8)
}

/// One drained write to its packet, or `None` for a refused category. The window's own
/// `hasTicket` picks create or update, never our belief, which can be 10 minutes stale.
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

/// The highest category vmangos files (`GMTICKET_MAX - 1`), also `GMTicketCategory.dbc`'s last row.
const GM_TICKET_CATEGORY_MAX: u8 = 10;

/// `GMTicketCategory.dbc` as the `(id, name)` pairs `GetGMTicketCategories()` returns.
#[derive(Resource)]
pub(crate) struct GmTicketCategories(pub(crate) Vec<(u32, String)>);

/// The survey tables ([`benilla_formats::load_gm_survey`]), for the VM.
#[derive(Resource)]
pub(crate) struct GmSurveyResource(benilla_ui::script::GmSurveyTables);

/// Startup, after the MPQ chain opens: a bare `Startup` races the open and can leave the category
/// list empty for the whole session.
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
    match benilla_formats::load_gm_survey(&mut chain) {
        Ok(survey) => {
            info!("gm survey: survey {} loaded", survey.id);
            commands.insert_resource(GmSurveyResource(benilla_ui::script::GmSurveyTables {
                id: survey.id,
                questions: survey.questions,
                texts: survey.texts,
            }));
        }
        Err(e) => warn!("gm survey: the survey tables failed to load: {e:#}"),
    }
}

/// The GM ticket's packet handlers.
pub(crate) mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use crate::net::NetHandlerApp;

    use super::{DialogLane, GmTicketState, UiError, UiErrorKeys};

    /// Register the ticket's handlers, called from [`super::UiGmTicketPlugin`].
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

    /// Every GETTICKET answer, asked for or pushed after a GM's `.ticket` command
    /// (`GMTicketMgr.cpp:153-159`): the wire does not tell them apart.
    fn on_ticket(In(ev): In<SessionEvent>, mut lane: ResMut<DialogLane>, errors: Res<UiErrorKeys>) {
        if let SessionEvent::GmTicket { status, ticket } = ev {
            super::ticket_answer(&mut lane, &errors, status, ticket.as_deref());
        }
    }

    fn on_system_status(In(ev): In<SessionEvent>, mut lane: ResMut<DialogLane>) {
        if let SessionEvent::GmTicketSystemStatus { status } = ev {
            super::status_answer(&mut lane, status);
        }
    }

    /// `SMSG_GM_TICKET_STATUS_UPDATE`, which vmangos never sends; cmangos does, so it is parsed.
    fn on_status_update(
        In(ev): In<SessionEvent>,
        mut ticket: ResMut<GmTicketState>,
        mut lane: ResMut<DialogLane>,
    ) {
        if let SessionEvent::GmTicketStatusUpdate { status } = ev {
            status_update(status, &mut ticket, &mut lane);
        }
    }

    /// The create and update responses, which no 1.12 FrameXML handler reads.
    fn on_written(
        In(ev): In<SessionEvent>,
        mut ticket: ResMut<GmTicketState>,
        errors: Res<UiErrorKeys>,
    ) {
        match ev {
            SessionEvent::GmTicketCreated { response }
            | SessionEvent::GmTicketUpdated { response } => {
                write_response(response, &mut ticket, &errors)
            }
            _ => {}
        }
    }

    /// `SMSG_GMTICKET_DELETETICKET` (`0x5e4606`): result 9, deleted, signals `UPDATE_TICKET`
    /// with `%d%s` over 0 and `""` (`0x5e461d`), the window's no-ticket answer; any other result
    /// shows `ERR_TICKET_DB_ERROR`.
    fn on_deleted(
        In(ev): In<SessionEvent>,
        mut lane: ResMut<DialogLane>,
        errors: Res<UiErrorKeys>,
    ) {
        if let SessionEvent::GmTicketDeleted { response: code } = ev {
            debug!("gm ticket: delete answered {code}");
            super::deleted(&mut lane, &errors, code);
        }
    }

    /// The ticket is the character's and the next login may be another: no re-ask carries over.
    fn on_session_end(In(_): In<SessionEvent>, mut ticket: ResMut<GmTicketState>) {
        ticket.clear_session();
    }

    /// `SMSG_GMTICKET_CREATE` and `SMSG_GMTICKET_UPDATETEXT` share one arm of `0x5e38c0`
    /// (`0x5e4432`, table `0x5e4d30`): 2 and 4 re-ask for the ticket (`0x5e4479`), 1 shows
    /// `ERR_TICKET_ALREADY_EXISTS` (`0x5e448e`), 3 `ERR_TICKET_CREATE_ERROR` (`0x5e44ab`), 5
    /// `ERR_TICKET_UPDATE_ERROR` (`0x5e44c8`), whichever opcode carried the code; others do nothing.
    pub(crate) fn write_response(
        response: u32,
        ticket: &mut super::GmTicketState,
        errors: &UiErrorKeys,
    ) {
        match response {
            2 | 4 => {
                debug!("gm ticket: write landed ({response}) — re-asking for the ticket");
                ticket.note_write_landed();
            }
            1 => errors.push(UiError::key("ERR_TICKET_ALREADY_EXISTS")),
            3 => errors.push(UiError::key("ERR_TICKET_CREATE_ERROR")),
            5 => errors.push(UiError::key("ERR_TICKET_UPDATE_ERROR")),
            other => debug!("gm ticket: write answered {other}"),
        }
    }

    /// `SMSG_GM_TICKET_STATUS_UPDATE` (`0x5e78f0`): 1 (updated) re-asks, `0x5e7932`; 2 (closed)
    /// fires the no-ticket `UPDATE_TICKET` at once, `0x5e7945`-`0x5e7960`; 3 (survey offered)
    /// fires `GMSURVEY_DISPLAY`, event 0x21a with no arguments (`0x5e7976`); others do nothing.
    pub(crate) fn status_update(
        status: u32,
        ticket: &mut super::GmTicketState,
        lane: &mut DialogLane,
    ) {
        match status {
            1 => {
                debug!("gm ticket: a GM updated the ticket — re-asking");
                ticket.note_write_landed();
            }
            2 => lane.event("UPDATE_TICKET", super::update_ticket_args(None)),
            3 => lane.event("GMSURVEY_DISPLAY", Vec::new()),
            other => debug!("gm ticket: unknown status update {other}"),
        }
    }
}

/// The GM trouble-ticket flow: the answer feed and the window's sends.
pub(crate) struct UiGmTicketPlugin;

impl Plugin for UiGmTicketPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::ui_dialog_verbs::add_dialog_lane(app);
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
                    send_gm_reasks.in_set(crate::ui_script::UiDrain),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_dialog_verbs::DialogSignal;
    use benilla_ui::script::UiScript;
    /// Each lottery and survey verb reaches the wire exactly once, with the command the bytes
    /// call for, and a refused call reaches it not at all.
    #[test]
    fn the_lottery_and_survey_verbs_send_once_each() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        app.insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        // Each chunk with the host lent, which sends inside the verb.
        let run = |app: &mut App, lua: &str| {
            crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
                let _ = s.expect("a VM").run(lua);
            });
            rx.try_iter().collect::<Vec<_>>()
        };
        let one = |sent: Vec<ClientCommand>| {
            assert_eq!(sent.len(), 1, "{sent:?}");
            sent.into_iter().next().unwrap()
        };
        assert!(matches!(
            one(run(&mut app, "GetLastLotteryNumbers()")),
            ClientCommand::LotteryQuery
        ));
        assert!(matches!(
            one(run(&mut app, "BuyRandomPicks(4)")),
            ClientCommand::BuyLotteryTicket {
                first: 4,
                second: 0
            }
        ));
        assert!(matches!(
            one(run(&mut app, "SubmitNumbers(1, 2, 3, 4)")),
            ClientCommand::BuyLotteryTicket {
                first: 0,
                second: 0b1111
            }
        ));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_gm_survey_tables(benilla_ui::script::GmSurveyTables {
                id: 6,
                questions: [28, 29, 0, 0, 0, 0, 0, 0, 0, 0],
                texts: vec![],
            });
        match one(run(
            &mut app,
            "GMSurveyAnswerSubmit(2, 4, 'ok') GMSurveyCommentSubmit('hi') GMSurveySubmit()",
        )) {
            ClientCommand::GmSurveySubmit {
                survey,
                answers,
                comment,
            } => {
                assert_eq!(survey, 6);
                assert_eq!(answers, [(28, 0, String::new()), (29, 4, "ok".to_string())]);
                assert_eq!(comment, "hi");
            }
            other => panic!("{other:?}"),
        }
        // `BuySkillTier` on a line its tier prices within the player's points: once.
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_skills(benilla_ui::script::SkillsState {
                entries: vec![benilla_ui::script::SkillEntry {
                    skill_id: 129,
                    name: "First Aid".into(),
                    value: 1,
                    max: 75,
                    temp_bonus: 0,
                    perm_bonus: 0,
                    min_level: 0,
                    cost_index: 0,
                    category_id: 2,
                    category_name: "Professions".into(),
                    category_order: 2,
                    description: String::new(),
                    abandonable: false,
                    mono: false,
                    step: 0,
                    flags: 0x8,
                    tier_id: 5,
                }],
            });
            script.set_skill_tiers([(5, [30; 16])]);
        }
        // `PLAYER_CHARACTER_POINTS2`, which the price is held to (`0x4d3faf`), on our player.
        app.world_mut().spawn((
            crate::net::SelfPlayer,
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(1103, 30)])),
        ));
        assert!(matches!(
            one(run(&mut app, "BuySkillTier(2)")),
            ClientCommand::SkillBuyStep { skill_id: 129 }
        ));
        // Refused, or a verb that sends nothing in the state this client holds.
        for call in [
            "BuyRandomPicks(0)",
            "SubmitNumbers(1, 1, 2, 3)",
            "AcceptSkillUps()",
            "BuySkillTier(1)",
            "BuySkillTier(3)",
            "LFGQuery(1, 0)",
        ] {
            assert!(run(&mut app, call).is_empty(), "{call}");
        }
    }

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

    /// The event leads with the category, the packet with the text.
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

    /// The window's form reset hangs on `arg1 and arg1 ~= 0`; an addon reads the reference's
    /// seven arguments, the floats as the format `%d%s%f%f%f%d%d` reads the pushed dwords.
    #[test]
    fn the_no_ticket_answer_carries_the_references_seven_arguments() {
        let args = update_ticket_args(None);
        assert_eq!(args.len(), 7);
        assert_eq!(args[0], ScriptValue::Int(0));
        assert_eq!(args[1], ScriptValue::Str(String::new()));
        let float = |i: usize| match args[i] {
            ScriptValue::Number(n) => n,
            ref other => panic!("arg{} is {other:?}", i + 1),
        };
        assert_eq!(float(2).to_bits(), u64::MAX, "(-1, -1) read as one double");
        assert_eq!(
            float(3).to_bits(),
            0xFFFF_FFFF,
            "(-1, 0) read as one double"
        );
    }

    /// `0x5e4568`: a negative first figure is stored as 0; NaN and -0 pass, the others untouched.
    #[test]
    fn a_negative_ticket_age_reaches_lua_as_zero() {
        let age = |v: f32| {
            let mut t = ticket();
            t.ticket_age = v;
            match update_ticket_args(Some(&t))[2] {
                ScriptValue::Number(n) => n,
                ref other => panic!("{other:?}"),
            }
        };
        assert_eq!(age(-3.5), 0.0);
        assert_eq!(age(-3.5).to_bits(), 0.0f64.to_bits(), "+0, not -0");
        assert!(age(f32::NAN).is_nan());
        assert_eq!(age(-0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(age(2.0), 2.0);
    }

    /// The window's `arg4 < 0 or arg5 < 0` test shows "unavailable".
    #[test]
    fn an_unknown_wait_time_reaches_lua_as_a_negative_not_a_zero() {
        let mut t = ticket();
        t.oldest_ticket_age = -1.0;
        t.update_time = -1.0;
        let args = update_ticket_args(Some(&t));
        assert_eq!(args[3], ScriptValue::Number(-1.0));
        assert_eq!(args[4], ScriptValue::Number(-1.0));
    }

    /// The window's 10-minute poll is answered at each packet, the same ticket or none.
    #[test]
    fn each_answer_signals_its_update_ticket() {
        let t = ticket();
        let mut lane = DialogLane::default();
        let errors = UiErrorKeys::default();
        for _ in 0..2 {
            ticket_answer(&mut lane, &errors, 6, Some(&t));
        }
        ticket_answer(&mut lane, &errors, 10, None);
        // The no-ticket answer carries a NaN, which never equals itself: compare its shape.
        let signals = lane.take();
        let [first, second, none] = signals.as_slice() else {
            panic!("{signals:?}");
        };
        let ticket = DialogSignal::Event("UPDATE_TICKET".into(), update_ticket_args(Some(&t)));
        assert_eq!((first, second), (&ticket, &ticket));
        let DialogSignal::Event(name, args) = none else {
            panic!("{none:?}");
        };
        assert_eq!((name.as_str(), args.len()), ("UPDATE_TICKET", 7));
    }

    /// The reference's `0x5e4479` re-ask; a refusal has nothing new to fetch.
    #[test]
    fn a_landed_write_makes_the_engine_reask_and_a_refused_one_does_not() {
        let mut state = GmTicketState::default();
        let errors = UiErrorKeys::default();
        // One arm serves both opcodes: 2 and 4 re-ask whichever carried them.
        net::write_response(2, &mut state, &errors);
        net::write_response(4, &mut state, &errors);
        assert_eq!(state.take_reasks(), 2, "one re-ask per landed write");
        assert_eq!(state.take_reasks(), 0, "drained means drained");
        assert!(errors.is_empty(), "a landed write shows no error");

        for refused in [1, 3, 5] {
            net::write_response(refused, &mut state, &errors);
        }
        assert_eq!(state.take_reasks(), 0, "a refusal has nothing to fetch");
    }

    fn keys(errors: &UiErrorKeys) -> Vec<&'static str> {
        errors.take().into_iter().map(|e| e.key).collect()
    }

    /// The shared arm `0x5e4432` shows 1, 3 and 5 as `ERR_TICKET_ALREADY_EXISTS`,
    /// `ERR_TICKET_CREATE_ERROR` and `ERR_TICKET_UPDATE_ERROR` (table `0x5e4d30`); every other
    /// code is silent.
    #[test]
    fn a_refused_ticket_write_shows_its_error() {
        let mut state = GmTicketState::default();
        let errors = UiErrorKeys::default();
        for code in [0, 1, 2, 3, 4, 5, 6, 9, 10] {
            net::write_response(code, &mut state, &errors);
        }
        assert_eq!(
            keys(&errors),
            [
                "ERR_TICKET_ALREADY_EXISTS",
                "ERR_TICKET_CREATE_ERROR",
                "ERR_TICKET_UPDATE_ERROR"
            ]
        );
    }

    /// `0x5e44fb`: status 0 shows `ERR_TICKET_DB_ERROR` before the no-ticket event; the other
    /// non-6 statuses fire the event alone.
    #[test]
    fn a_ticket_fetch_status_zero_shows_the_db_error() {
        let mut lane = DialogLane::default();
        let errors = UiErrorKeys::default();
        ticket_answer(&mut lane, &errors, 0, None);
        assert_eq!(keys(&errors), ["ERR_TICKET_DB_ERROR"]);
        assert_eq!(lane.take().len(), 1, "the no-ticket event follows");
        for status in [10, 1, 7] {
            ticket_answer(&mut lane, &errors, status, None);
        }
        assert!(errors.is_empty());
        let t = ticket();
        ticket_answer(&mut lane, &errors, 6, Some(&t));
        assert!(errors.is_empty());
    }

    /// vmangos sends only 0 or 1, so no live run reaches the window's -1 arm.
    #[test]
    fn a_minus_one_queue_status_survives_to_lua_as_minus_one() {
        let mut lane = DialogLane::default();
        status_answer(&mut lane, -1);
        assert_eq!(
            lane.take(),
            [DialogSignal::Event(
                "UPDATE_GM_STATUS".into(),
                vec![ScriptValue::Int(-1)]
            )]
        );
    }

    /// `0x5e4606`: only result 9 signals, `UPDATE_TICKET` over `%d%s` with 0 and `""`
    /// (`0x5e461d`).
    #[test]
    fn a_deleted_ticket_signals_the_no_ticket_answer() {
        let mut lane = DialogLane::default();
        let errors = UiErrorKeys::default();
        deleted(&mut lane, &errors, 8);
        // Any other result is the DB error and fires nothing (`0x5e4635`).
        assert_eq!(keys(&errors), ["ERR_TICKET_DB_ERROR"]);
        assert!(lane.take().is_empty());
        deleted(&mut lane, &errors, 9);
        assert!(errors.is_empty());
        assert_eq!(
            lane.take(),
            [DialogSignal::Event(
                "UPDATE_TICKET".into(),
                vec![ScriptValue::Int(0), ScriptValue::Str(String::new())]
            )]
        );
    }

    /// `0x5e78f0`: status 2 fires the no-ticket `UPDATE_TICKET` and 3 `GMSURVEY_DISPLAY`; 1
    /// re-asks and fires nothing; anything outside 1 to 3 is ignored.
    #[test]
    fn a_closed_ticket_and_an_offered_survey_fire_their_events() {
        let mut state = GmTicketState::default();
        let mut lane = DialogLane::default();
        for status in [0, 4, 1] {
            net::status_update(status, &mut state, &mut lane);
        }
        assert!(lane.take().is_empty(), "1 re-asks and the rest do nothing");
        assert_eq!(state.take_reasks(), 1);

        net::status_update(2, &mut state, &mut lane);
        net::status_update(3, &mut state, &mut lane);
        let signals = lane.take();
        let [closed, survey] = signals.as_slice() else {
            panic!("{signals:?}");
        };
        assert!(matches!(
            closed,
            DialogSignal::Event(name, args) if name == "UPDATE_TICKET" && args.len() == 7
        ));
        assert_eq!(
            survey,
            &DialogSignal::Event("GMSURVEY_DISPLAY".into(), Vec::new())
        );
        assert_eq!(state.take_reasks(), 0);
    }

    #[test]
    fn a_dead_session_forgets_the_owed_reasks() {
        let mut state = GmTicketState::default();
        state.note_write_landed();
        state.clear_session();
        assert_eq!(state.take_reasks(), 0);
    }

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

    /// vmangos files 0 as "Unknown" and drops 11 and up unanswered; 256 wraps to 0 in a `u8`, so
    /// the check must run before the narrowing.
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

        // Through the whole path: a refused category makes no packet.
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
