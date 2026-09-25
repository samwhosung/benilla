//! The character-create live probe (`WOW_PROBE_CHARCREATE="<name>[,race,class,gender[,skin,face,\
//! hair,haircolor,facial]]"`) — the agent-side instrument that machine-verifies the char-create/delete
//! wire against the live server (phase 1), inert without the env. It also keeps the
//! `CharRequest::Create`/`Delete` verbs exercised until their UI lands (the create screen is phase 4,
//! delete's UI is deferred).
//!
//! While parked at character select it sends [`CharRequest::Create`] with the parsed request, logs
//! the `SMSG_CHAR_CREATE` result byte, and — unless `WOW_PROBE_CHARCREATE_KEEP=1` — deletes the
//! character it just made ([`CharRequest::Delete`]) so the account isn't littered, logging that
//! result too. Run it **without** `WOW_CHAR` (which would auto-enter the world and leave select),
//! against the checkout's own probe account (`.probe-identity`, or `WOW_USER`/`WOW_PASS` — the
//! `probe` skill); creating/deleting characters is a
//! non-combat operation, safe to run headlessly.

use benilla_protocol::{messages, CharAction, CharCreateReq};
use bevy::prelude::*;

use crate::net::{CharActionResultMessage, CharListMessage, CharPick, CharRequest};

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
    /// Waiting for the first roster (the socket parking at select).
    AwaitingRoster,
    /// Sent `Create`, waiting for its result.
    Creating,
    /// Sent `Delete`, waiting for its result.
    Deleting,
    /// Nothing left to do.
    Done,
}

#[derive(Resource)]
struct CharCreateProbe {
    req: CharCreateReq,
    keep: bool,
    state: ProbeState,
    /// The freshest roster seen (tracked so a successful create can find its guid to clean up).
    roster: Vec<benilla_protocol::Character>,
}

/// Drive the probe's create → (delete) → done sequence off the roster + result messages.
fn drive_charcreate_probe(
    mut probe: ResMut<CharCreateProbe>,
    pick: Res<CharPick>,
    mut lists: MessageReader<CharListMessage>,
    mut results: MessageReader<CharActionResultMessage>,
) {
    // Always track the freshest roster (the fresh list precedes each successful action's result).
    let got_roster = lists.read().fold(false, |_, m| {
        probe.roster = m.characters.clone();
        true
    });

    match probe.state {
        ProbeState::AwaitingRoster => {
            if got_roster {
                info!(
                    "probe-charcreate: parked at select — creating {:?} (race {} class {} gender {}; \
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
                let _ = pick.0.send(CharRequest::Create(probe.req.clone()));
                probe.state = ProbeState::Creating;
            }
        }
        ProbeState::Creating => {
            for r in results.read() {
                if r.action != CharAction::Create {
                    continue;
                }
                info!(
                    "probe-charcreate: create result = {:#04x} ({})",
                    r.code,
                    result_label(CharAction::Create, r.code)
                );
                let created = r.code == messages::CHAR_CREATE_SUCCESS;
                if created && !probe.keep {
                    match probe
                        .roster
                        .iter()
                        .find(|c| c.name.eq_ignore_ascii_case(&probe.req.name))
                    {
                        Some(c) => {
                            let guid = c.guid;
                            info!("probe-charcreate: cleaning up — deleting guid {guid}");
                            let _ = pick.0.send(CharRequest::Delete(guid));
                            probe.state = ProbeState::Deleting;
                        }
                        None => {
                            warn!("probe-charcreate: created char not in roster — cannot clean up");
                            probe.state = ProbeState::Done;
                        }
                    }
                } else {
                    if created {
                        info!("probe-charcreate: KEEP set — leaving the new character in place");
                    }
                    probe.state = ProbeState::Done;
                }
                break;
            }
        }
        ProbeState::Deleting => {
            for r in results.read() {
                if r.action != CharAction::Delete {
                    continue;
                }
                info!(
                    "probe-charcreate: delete result = {:#04x} ({})",
                    r.code,
                    result_label(CharAction::Delete, r.code)
                );
                probe.state = ProbeState::Done;
                break;
            }
        }
        ProbeState::Done => {}
    }
}

/// A minimal name for the result byte (the create screen has the full 1.12 GlueStrings table; the
/// probe only needs the anchors it verifies against).
fn result_label(action: CharAction, code: u8) -> &'static str {
    match (action, code) {
        (CharAction::Create, messages::CHAR_CREATE_SUCCESS) => "CHAR_CREATE_SUCCESS",
        (CharAction::Create, messages::CHAR_CREATE_NAME_IN_USE) => "CHAR_NAME_ALREADY_IN_USE",
        (CharAction::Delete, messages::CHAR_DELETE_SUCCESS) => "CHAR_DELETE_SUCCESS",
        _ => "other",
    }
}
