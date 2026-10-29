//! The pet's packet handlers: `SMSG_PET_SPELLS` and `SMSG_PET_MODE` write [`PetBar`]; the
//! refusals, the tame failure and the pet's sounds go to their queues. The pet's refusals share
//! the player's red line but not its message tables: the reference maps them in their own handlers
//! (`0x4bdb70`, `0x6e8eb0`) to say "your pet" where the player's say "you".

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::{PetMode, PetSpells};
use benilla_protocol::{SessionEvent, SessionEventKind};

use benilla_assets::coords::wow_to_bevy;

use super::PetBar;
use crate::net::{GuidIndex, NetHandlerApp, PetDismissSoundMessage, PetTalkMessage};
use crate::ui_action::{FillArg, Spells, UiError, UiErrorKeys};
use crate::ui_cast::CastReports;

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::PetSpells, on_spells)
        .net_handler(K::PetMode, on_mode)
        .net_handler(K::PetActionFeedback, on_action_feedback)
        .net_handler(K::PetCastFailed, on_cast_failed)
        .net_handler(K::PetTameFailure, on_tame_failure)
        .net_handler(K::PetNameInvalid, on_refusal_line)
        .net_handler(K::PetBroken, on_refusal_line)
        .net_handler(K::PetActionSound, on_action_sound)
        .net_handler(K::PetDismissSound, on_dismiss_sound)
        .net_handler(K::Disconnected, on_session_end);
}

/// A lost socket sends no zero-guid `SMSG_PET_SPELLS`, so the session end resets the bar itself.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut bar: ResMut<PetBar>,
    mut latch: ResMut<super::PetAttackLatch>,
) {
    bar.clear();
    latch.0 = None;
}

fn on_spells(
    In(ev): In<SessionEvent>,
    catalog: Option<Res<Spells>>,
    mods: Option<Res<crate::spell::SpellModifiers>>,
    player: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    mut bar: ResMut<PetBar>,
    mut latch: ResMut<super::PetAttackLatch>,
    mut lane: ResMut<crate::field_watch::NoticeLane>,
) {
    if let SessionEvent::PetSpells(spells) = ev {
        // The pet list's category pad (`0x6e2b60` with the pet flag): op 11 alone.
        let pad = crate::spell::CategoryPad::for_list(true, player.iter().next());
        let changed = pet_spells(*spells, catalog.as_deref(), mods.as_deref(), pad, &mut bar);
        if changed {
            latch.0 = None;
        }
        // The pet book's sort on every packet, an empty list included (`0x4bdadb` →
        // `0x4b2fd0(0, 1)`), which signals `SPELLS_CHANGED` (`0x4b302a`) ahead of the macro
        // derivation (`0x4bdae0`) and the guid's store.
        lane.write(crate::field_watch::FieldNotice::Global {
            id: crate::field_watch::id::SPELLS_CHANGED,
        });
        // Then the macros that follow the pet's book derived again (`0x4bdae0` → `0x4f0ce0`).
        lane.write(crate::field_watch::FieldNotice::PetMacros);
        // `SetPetGuid`'s signals at the packet's place (`0x4bdaf0` → `0x4bc7e0`): the pet frame's
        // `UnitExists("pet")` reads our summon field as this packet left it.
        lane.write(crate::field_watch::FieldNotice::PetGuidSet {
            changed,
            guid: bar.spells.pet_guid,
        });
    }
}

fn on_mode(
    In(ev): In<SessionEvent>,
    mut bar: ResMut<PetBar>,
    mut lane: ResMut<crate::field_watch::NoticeLane>,
) {
    if let SessionEvent::PetMode(mode) = ev {
        if pet_mode(mode, &mut bar) {
            // `0x4bdb4f`: `PET_BAR_UPDATE` inside the handler, at the packet's place.
            lane.write(crate::field_watch::FieldNotice::Global {
                id: crate::field_watch::id::PET_BAR_UPDATE,
            });
        }
    }
}

fn on_action_feedback(In(ev): In<SessionEvent>, errors: Res<UiErrorKeys>) {
    if let SessionEvent::PetActionFeedback { reason } = ev {
        pet_action_feedback(reason, &errors);
    }
}

fn on_cast_failed(In(ev): In<SessionEvent>, mut reports: ResMut<CastReports>) {
    if let SessionEvent::PetCastFailed { spell_id, reason } = ev {
        pet_cast_failed(spell_id, reason, &mut reports);
    }
}

fn on_tame_failure(In(ev): In<SessionEvent>, errors: Res<UiErrorKeys>) {
    if let SessionEvent::PetTameFailure { reason } = ev {
        pet_tame_failure(reason, &errors);
    }
}

fn on_refusal_line(In(ev): In<SessionEvent>, errors: Res<UiErrorKeys>) {
    match ev {
        SessionEvent::PetNameInvalid => pet_name_invalid(&errors),
        SessionEvent::PetBroken => pet_broken(&errors),
        _ => {}
    }
}

fn on_action_sound(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut talks: MessageWriter<PetTalkMessage>,
) {
    if let SessionEvent::PetActionSound { pet_guid, talk } = ev {
        pet_action_sound(pet_guid, talk, &index, &mut talks);
    }
}

fn on_dismiss_sound(In(ev): In<SessionEvent>, mut sounds: MessageWriter<PetDismissSoundMessage>) {
    if let SessionEvent::PetDismissSound { model_id, position } = ev {
        pet_dismiss_sound(model_id, position, &mut sounds);
    }
}

/// `SMSG_PET_SPELLS`: replace the whole bar and reseed the pet's cooldowns from its tail. A zero
/// guid is the teardown (`Player::RemovePetActionBar`), cooldowns included. Whether the pet guid
/// changed, which lowers the attack latch (`0x4bc8ce`); a re-send from the same pet keeps it.
fn pet_spells(
    spells: PetSpells,
    catalog: Option<&Spells>,
    mods: Option<&crate::spell::SpellModifiers>,
    pad: crate::spell::CategoryPad,
    bar: &mut PetBar,
) -> bool {
    let new_pet = bar.spells.pet_guid != spells.pet_guid;
    if spells.pet_guid == 0 {
        if bar.spells.pet_guid != 0 {
            debug!("net: pet bar torn down");
        }
        bar.clear();
        return new_pet;
    }
    debug!(
        "net: pet bar for {:#x} — react {} command {} {}, {} known spell(s), {} cooldown(s)",
        spells.pet_guid,
        spells.react_state(),
        spells.command_state(),
        if spells.bar_disabled() {
            "DISABLED"
        } else {
            "usable"
        },
        spells.spells.len(),
        spells.cooldowns.len(),
    );
    debug!(
        "net: pet bar slots — {}",
        spells
            .bar
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let what = match e.kind() {
                    benilla_protocol::messages::PET_ACT_COMMAND => format!("cmd {}", e.action()),
                    benilla_protocol::messages::PET_ACT_REACTION => format!("react {}", e.action()),
                    _ if e.is_empty() => "empty".to_string(),
                    _ => match catalog.and_then(|c| c.catalog.get(e.action())) {
                        Some(d) => format!("{} ({})", d.name, e.action()),
                        None => format!("spell {} NOT IN CATALOG", e.action()),
                    },
                };
                format!("{}:{what}/{:#04x}", i + 1, e.kind())
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    // vmangos sends the pet's passives here too; the book skips `DO_NOT_DISPLAY` (`0x4b2f90`).
    debug!(
        "net: pet spellbook — {}",
        spells
            .spells
            .iter()
            .map(|e| {
                match catalog.and_then(|c| c.catalog.get(e.action())) {
                    Some(d) if d.in_pet_book() => format!("{} ({})", d.name, e.action()),
                    Some(d) => format!("{} ({}) DO_NOT_DISPLAY", d.name, e.action()),
                    None => format!("{} NOT IN CATALOG", e.action()),
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    );
    let now = Instant::now();
    // `SetPet` (`0x4bc7e0`) re-arms the expiry on every packet, and a 0 duration clears it.
    bar.expires = (spells.duration_ms != 0)
        .then(|| now + std::time::Duration::from_millis(u64::from(spells.duration_ms)));
    bar.cooldowns.clear_silent();
    let no_mods = crate::spell::SpellModifiers::default();
    for cd in &spells.cooldowns {
        let display = catalog.and_then(|c| c.catalog.get(cd.spell_id));
        bar.cooldowns
            .seed_pet(cd, display, mods.unwrap_or(&no_mods), pad, now);
    }
    bar.spells = spells;
    new_pet
}

/// `SMSG_PET_MODE` (`0x4bdb10`): the state word alone, applied only when the packet's guid is the
/// bar's cached one (`0x4bdb33`, `0x4bdb3f`), a zero guid against no bar included; whether it was,
/// for the signal that follows the store (`0x4bdb4f`).
fn pet_mode(mode: PetMode, bar: &mut PetBar) -> bool {
    if bar.spells.pet_guid != mode.pet_guid {
        return false;
    }
    debug!("net: pet mode — state {:#010x}", mode.state);
    // The whole dword, as the client stores both packets' state (`0x4bc930`).
    bar.spells.state = mode.state;
    true
}

/// `SMSG_PET_ACTION_FEEDBACK`: a refused order's reason byte, onto the red line.
fn pet_action_feedback(reason: u8, errors: &UiErrorKeys) {
    debug!("net: pet action feedback {reason}");
    if let Some(key) = pet_feedback_key(reason) {
        errors.push(UiError::key(key));
    }
}

/// The reference's map (`0x4bdb70`, jump table `0x4bdbe8`): vmangos `FEEDBACK_PET_DEAD`,
/// `FEEDBACK_NOTHING_TO_ATT`, `FEEDBACK_CANT_ATT_TARGET` and `FEEDBACK_NO_PATH_TO` (1-4) raise
/// errorIds `0x150`, `0xa0`, `0xa1` and `0x151` through `0x496720`; other bytes show nothing.
/// `ERR_PET_SPELL_NOPATH` has no 1.12 string, so a no-path order is silent in the reference too.
fn pet_feedback_key(reason: u8) -> Option<&'static str> {
    Some(match reason {
        1 => "ERR_PET_SPELL_DEAD",
        2 => "ERR_NO_ATTACK_TARGET",
        3 => "ERR_INVALID_ATTACK_TARGET",
        4 => "ERR_PET_SPELL_NOPATH",
        _ => return None,
    })
}

/// `SMSG_PET_TAME_FAILURE` (`0x6e97e0` → `0x6e6a20`), the handler's one signal: the reason's
/// `PETTAME_*` string (the jump table `0x6e6ac0`, `PETTAME_UNKNOWNERROR` outside 1 to 11), read
/// through `FrameScript_GetText` (`0x6e6a91`, `""` when missing) as `ERR_TAME_FAILED`'s `%s`
/// (`DisplayError(0xee)`, `0x6e6ab4`). vmangos also sends it for a summon while a pet or a charmed
/// unit is out (`Spell.cpp:5467`).
fn pet_tame_failure(reason: u8, errors: &UiErrorKeys) {
    let key = benilla_protocol::messages::pet_tame_failure_key(reason);
    debug!("net: pet tame failure {reason} ({key})");
    errors.push(UiError::args(
        "ERR_TAME_FAILED",
        vec![FillArg::Global(key.to_string())],
    ));
}

/// `SMSG_PET_NAME_INVALID`, an empty body: the reference raises `ERR_INVALID_PETNAME` and reads
/// nothing (`0x5e3e33`). No local name was applied, so nothing rolls back.
fn pet_name_invalid(errors: &UiErrorKeys) {
    debug!("net: pet name refused");
    errors.push(UiError::key("ERR_INVALID_PETNAME"));
}

/// `SMSG_PET_BROKEN`, an empty body: loyalty hit zero; raise `ERR_PET_BROKEN` (`0x4bdc00`). The
/// zero-guid `SMSG_PET_SPELLS` of the unsummon right after it clears the bar (`Pet.cpp:822`).
fn pet_broken(errors: &UiErrorKeys) {
    debug!("net: pet ran away");
    errors.push(UiError::key("ERR_PET_BROKEN"));
}

/// `SMSG_PET_ACTION_SOUND`: dropped when the pet is not streamed (`0x604101`); the sound layer
/// checks the selector ([`crate::sound::creature`], `0x604106`/`0x60411c`).
fn pet_action_sound(
    pet_guid: u64,
    talk: u32,
    index: &GuidIndex,
    talks: &mut MessageWriter<PetTalkMessage>,
) {
    debug!("net: pet talk {talk} from {pet_guid:#x}");
    if let Some(&unit) = index.0.get(&pet_guid) {
        talks.write(PetTalkMessage { unit, talk });
    }
}

/// `SMSG_PET_DISMISS_SOUND`: sounded a yard above the wire point (`0x6041d0 fadd [0x7ff9d8]`),
/// in WoW space, so before the change to Bevy's.
fn pet_dismiss_sound(
    model_id: u32,
    position: [f32; 3],
    sounds: &mut MessageWriter<PetDismissSoundMessage>,
) {
    debug!("net: pet dismissed — model {model_id} at {position:?}");
    sounds.write(PetDismissSoundMessage {
        model_id,
        pos: wow_to_bevy([position[0], position[1], position[2] + 1.0]),
    });
}

/// `SMSG_PET_CAST_FAILED`: queued as the pet's, so the display uses the reference's pet table
/// (`0x6e8eb0`, not the player's `0x6e1a00`). Our cast state is untouched, and unlike the
/// player's it writes no combat-log line and plays no error sound (`0x62c360`, `0x458a50`).
fn pet_cast_failed(spell_id: u32, reason: Option<u8>, reports: &mut CastReports) {
    debug!("net: pet cast failed — spell {spell_id} reason {reason:?}");
    if let Some(reason) = reason {
        reports.pet_refused(spell_id, reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{PetActionEntry, PET_ACT_COMMAND, PET_COMMAND_ATTACK};

    fn a_bar(guid: u64) -> PetSpells {
        let mut s = PetSpells {
            pet_guid: guid,
            state: 0x0101, // react Defensive, command Follow
            ..Default::default()
        };
        s.bar[0] = PetActionEntry::from(PET_COMMAND_ATTACK | (u32::from(PET_ACT_COMMAND) << 24));
        s
    }

    #[test]
    fn a_zero_guid_tears_the_whole_bar_down() {
        let mut bar = PetBar::default();
        pet_spells(
            a_bar(0x2A),
            None,
            None,
            crate::spell::CategoryPad::NoPlayer,
            &mut bar,
        );
        assert_eq!(bar.spells.pet_guid, 0x2A);

        pet_spells(
            PetSpells::default(),
            None,
            None,
            crate::spell::CategoryPad::NoPlayer,
            &mut bar,
        );
        assert_eq!(bar.spells, PetSpells::default());
        assert!(!bar.has_bar());
    }

    /// `SetPet` (`0x4bc7e0`): each packet re-arms the expiry from its own duration, 0 clears it,
    /// and the teardown clears it too.
    #[test]
    fn each_pet_packet_rearms_or_clears_the_expiry() {
        let mut bar = PetBar::default();
        let mut charm = a_bar(0x2A);
        charm.duration_ms = 60_000;
        let before = Instant::now();
        pet_spells(
            charm.clone(),
            None,
            None,
            crate::spell::CategoryPad::NoPlayer,
            &mut bar,
        );
        let left = bar.expires.expect("armed").duration_since(before);
        assert!(
            left >= std::time::Duration::from_secs(60) && left < std::time::Duration::from_secs(61)
        );

        pet_spells(
            a_bar(0x2A),
            None,
            None,
            crate::spell::CategoryPad::NoPlayer,
            &mut bar,
        );
        assert_eq!(bar.expires, None, "a 0 duration clears it");
        pet_spells(
            charm,
            None,
            None,
            crate::spell::CategoryPad::NoPlayer,
            &mut bar,
        );
        assert!(bar.expires.is_some());
        pet_spells(
            PetSpells::default(),
            None,
            None,
            crate::spell::CategoryPad::NoPlayer,
            &mut bar,
        );
        assert_eq!(bar.expires, None, "the teardown clears it");
    }

    #[test]
    fn pet_mode_only_writes_its_own_bar() {
        let mut bar = PetBar::default();
        pet_spells(
            a_bar(0x2A),
            None,
            None,
            crate::spell::CategoryPad::NoPlayer,
            &mut bar,
        );

        pet_mode(
            PetMode {
                pet_guid: 0x99,
                state: 0x0202,
            },
            &mut bar,
        );
        assert_eq!(bar.spells.react_state(), 1, "a stranger's mode is ignored");

        pet_mode(
            PetMode {
                pet_guid: 0x2A,
                state: 0x0202,
            },
            &mut bar,
        );
        assert_eq!(bar.spells.react_state(), 2);
        assert_eq!(bar.spells.command_state(), 2);
        assert_eq!(bar.spells.bar[0].action(), PET_COMMAND_ATTACK);
    }

    /// The client's pet-guid writer clears the latch only on a change (`0x4bc8ce`).
    #[test]
    fn the_attack_latch_survives_a_resend_and_dies_on_a_new_pet() {
        let mut bar = PetBar::default();
        let mut spells = |guid| {
            pet_spells(
                a_bar(guid),
                None,
                None,
                crate::spell::CategoryPad::NoPlayer,
                &mut bar,
            )
        };
        assert!(spells(0x2A), "the first pet");
        assert!(!spells(0x2A), "the same pet re-sending keeps its attack");
        assert!(spells(0x99), "a different pet is not attacking");
    }

    #[test]
    fn the_feedback_map_is_the_jump_table_at_0x4bdbe8() {
        let errors = UiErrorKeys::default();
        for reason in [0u8, 1, 2, 3, 4, 5, 200] {
            pet_action_feedback(reason, &errors);
        }
        assert_eq!(
            errors.snapshot().iter().map(|e| e.key).collect::<Vec<_>>(),
            [
                "ERR_PET_SPELL_DEAD",
                "ERR_NO_ATTACK_TARGET",
                "ERR_INVALID_ATTACK_TARGET",
                "ERR_PET_SPELL_NOPATH",
            ]
        );
    }

    #[test]
    fn every_feedback_arm_is_a_catalog_row() {
        for reason in 1..=4u8 {
            let key = pet_feedback_key(reason).expect("an arm");
            let row = benilla_ui::messages::by_key(key)
                .unwrap_or_else(|| panic!("{key} is not a message-catalog row"));
            assert_eq!(
                row.kind,
                benilla_ui::messages::MsgKind::Error,
                "{key} is the red line"
            );
        }
    }
}
