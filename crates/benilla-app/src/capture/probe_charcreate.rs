//! The character-create live probe (`WOW_PROBE_CHARCREATE="<name>[,race,class,gender[,skin,\
//! face,hair,haircolor,facial]]"`), inert without the env. At character select it presses Create
//! New Character, puts the body up on the stock create screen and presses Okay
//! ([`super::glue_press`]), logs the `SMSG_CHAR_CREATE` result byte, then deletes the new
//! character from the select screen and logs that result too, unless
//! `WOW_PROBE_CHARCREATE_KEEP=1`. Run it without `WOW_CHAR`, which would enter the world; the
//! switches are `docs/CONTRIBUTING.md`, "Running it unattended".

use benilla_protocol::{messages, CharAction, CharCreateReq};
use bevy::prelude::*;

use super::glue_press;
use crate::net::{CharActionResultMessage, CharListMessage};
use crate::ui_script::glue::StockGlueProbe;

pub(crate) struct ProbeCharCreatePlugin;

impl Plugin for ProbeCharCreatePlugin {
    fn build(&self, app: &mut App) {
        let Some(req) = std::env::var("WOW_PROBE_CHARCREATE")
            .ok()
            .as_deref()
            .and_then(parse_req)
        else {
            return; // inert without a valid env spec
        };
        let keep = std::env::var("WOW_PROBE_CHARCREATE_KEEP").as_deref() == Ok("1");
        app.insert_resource(CharCreateProbe {
            req,
            keep,
            state: ProbeState::AwaitingRoster,
            roster: Vec::new(),
            listed: false,
        })
        .add_systems(Update, drive_charcreate_probe);
    }
}

/// Parse `name[,race,class,gender[,skin,face,hair,haircolor,facial]]` into a request; the name is
/// required, everything else defaults (Human Warrior male, all appearance dials 0).
fn parse_req(spec: &str) -> Option<CharCreateReq> {
    let mut parts = spec.split(',').map(str::trim);
    let name = parts.next().filter(|n| !n.is_empty())?.to_string();
    let mut next_u8 = |default: u8| parts.next().and_then(|p| p.parse().ok()).unwrap_or(default);
    Some(CharCreateReq {
        name,
        race: next_u8(messages::RACE_HUMAN),
        class: next_u8(messages::CLASS_WARRIOR),
        gender: next_u8(messages::GENDER_MALE),
        skin: next_u8(0),
        face: next_u8(0),
        hair_style: next_u8(0),
        hair_color: next_u8(0),
        facial_hair: next_u8(0),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeState {
    /// Waiting for the select screen to hold the first roster.
    AwaitingRoster,
    /// Pressed Create New Character, waiting for the create screen.
    CreateScreen,
    /// Pressed the create screen's Okay, waiting for its result.
    Creating,
    /// Created, waiting for the select screen to hold the roster with the new row.
    AwaitingCreated,
    /// Pressed the delete, waiting for its result.
    Deleting,
    /// Nothing left to do.
    Done,
}

#[derive(Resource)]
struct CharCreateProbe {
    req: CharCreateReq,
    keep: bool,
    state: ProbeState,
    /// The freshest roster, where a successful create finds its row to clean up.
    roster: Vec<benilla_protocol::Character>,
    /// A roster has come since the probe last left the select screen.
    listed: bool,
}

/// Drive create, then delete, through the stock screens off the roster and result messages.
fn drive_charcreate_probe(
    mut probe: ResMut<CharCreateProbe>,
    stock: StockGlueProbe,
    mut lists: MessageReader<CharListMessage>,
    mut results: MessageReader<CharActionResultMessage>,
) {
    if let Some(list) = lists.read().last() {
        probe.roster = list.characters.clone();
        probe.listed = true;
    }
    let vm = Some(stock.vm()).filter(|vm| vm.is_up());

    match probe.state {
        ProbeState::AwaitingRoster => {
            let Some(vm) =
                vm.filter(|vm| probe.listed && glue_press::select_holds(*vm, &probe.roster))
            else {
                return;
            };
            info!(
                "probe-charcreate: at select — creating {:?} (race {} class {} gender {}; \
                 appearance {}/{}/{}/{}/{})",
                probe.req.name,
                probe.req.race,
                probe.req.class,
                probe.req.gender,
                probe.req.skin,
                probe.req.face,
                probe.req.hair_style,
                probe.req.hair_color,
                probe.req.facial_hair,
            );
            glue_press::press_create_screen(vm);
            probe.state = ProbeState::CreateScreen;
        }
        ProbeState::CreateScreen => {
            let Some(vm) = vm.filter(|vm| glue_press::create_up(*vm)) else {
                return;
            };
            match glue_press::press_body(vm, &probe.req) {
                Ok(()) => {
                    glue_press::press_okay(vm, &probe.req.name);
                    probe.state = ProbeState::Creating;
                }
                Err(why) => {
                    error!("probe-charcreate: the create screen refused the body ({why})");
                    probe.state = ProbeState::Done;
                }
            }
        }
        ProbeState::Creating => {
            let Some(r) = results.read().find(|r| r.action == CharAction::Create) else {
                return;
            };
            info!(
                "probe-charcreate: create result = {:#04x} ({})",
                r.code,
                result_label(CharAction::Create, r.code)
            );
            let created = r.code == messages::CHAR_CREATE_SUCCESS;
            if created && !probe.keep {
                // The screen goes back to select and asks for the roster with the new row.
                probe.listed = false;
                probe.state = ProbeState::AwaitingCreated;
            } else {
                if created {
                    info!("probe-charcreate: KEEP set — leaving the new character in place");
                }
                probe.state = ProbeState::Done;
            }
        }
        ProbeState::AwaitingCreated => {
            let Some(vm) =
                vm.filter(|vm| probe.listed && glue_press::select_holds(*vm, &probe.roster))
            else {
                return;
            };
            match probe
                .roster
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(&probe.req.name))
            {
                Some(row) => {
                    info!(
                        "probe-charcreate: cleaning up — deleting guid {}",
                        probe.roster[row].guid
                    );
                    glue_press::press_delete(vm, row);
                    probe.state = ProbeState::Deleting;
                }
                None => {
                    warn!("probe-charcreate: created char not in roster — cannot clean up");
                    probe.state = ProbeState::Done;
                }
            }
        }
        ProbeState::Deleting => {
            let Some(r) = results.read().find(|r| r.action == CharAction::Delete) else {
                return;
            };
            info!(
                "probe-charcreate: delete result = {:#04x} ({})",
                r.code,
                result_label(CharAction::Delete, r.code)
            );
            probe.state = ProbeState::Done;
        }
        ProbeState::Done => {}
    }
}

/// A name for the result bytes the probe checks; the create screen has the full table.
fn result_label(action: CharAction, code: u8) -> &'static str {
    match (action, code) {
        (CharAction::Create, messages::CHAR_CREATE_SUCCESS) => "CHAR_CREATE_SUCCESS",
        (CharAction::Create, messages::CHAR_CREATE_NAME_IN_USE) => "CHAR_NAME_ALREADY_IN_USE",
        (CharAction::Delete, messages::CHAR_DELETE_SUCCESS) => "CHAR_DELETE_SUCCESS",
        _ => "other",
    }
}
