//! The UI error line's queues and resolvers, which `super::feed_actions` drains into
//! [`show_messages`]; cast failures resolve in [`super::cast_fail`].

use benilla_ui::messages::MessageRecord;
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::net::ObjectStore;
use crate::sound::MessageSounds;
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
use crate::ui_items::{count_of, InventoryScope};

/// Cast failures for the UI error line; the spell id lets the display key on the spell's record.
#[derive(Resource, Default)]
pub(crate) struct CastErrors(pub Vec<CastFail>);

/// One queued cast failure. `arg` fills the message's `%s` (the arm table `0x6e1d8e`); the
/// reference raises its local refusals with none, except the crowd-control `0x8d` (`0x6094f0`),
/// which names the blocking aura's `SpellMechanic.dbc` id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CastFail {
    pub spell_id: u32,
    pub reason: u8,
    pub arg: Option<u32>,
    /// Whose refusal this is, which picks the reference's message table ([`Caster`]).
    pub caster: Caster,
    /// A retry after the item template landed (the `0x78`/`0x5c` cache miss). The reference's
    /// first pass exits past both the line and the log (`0x6e1eab`, `0x6e1efc`); its retry, the
    /// cache callback `0x6e29b0`, calls `DisplayError` directly and never `0x62c360`, so a
    /// redisplay is not logged.
    pub redisplay: bool,
}

/// Who failed to cast, which picks the reference's handler: `SMSG_CAST_RESULT` lands in
/// `0x6e1a00` and `SMSG_PET_CAST_FAILED` in `0x6e8eb0`, each with its own reason-to-errorId map;
/// the rest of [`super::cast_fail::cast_fail_text`] is shared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Caster {
    /// Ours: `SMSG_CAST_RESULT` and every local refusal.
    #[default]
    Player,
    /// The pet's or the charm's: `SMSG_PET_CAST_FAILED` only.
    Pet,
}

impl CastFail {
    /// A client-local refusal, with no argument word; always the player's, since the server
    /// decides every pet refusal.
    pub(crate) const fn local(spell_id: u32, reason: u8) -> Self {
        Self {
            spell_id,
            reason,
            arg: None,
            caster: Caster::Player,
            redisplay: false,
        }
    }

    /// The same entry, marked as a [`Self::redisplay`].
    pub(crate) const fn requeued(self) -> Self {
        Self {
            redisplay: true,
            ..self
        }
    }
}

impl CastErrors {
    /// Queue a [`CastFail::local`] refusal.
    pub(crate) fn push_local(&mut self, spell_id: u32, reason: u8) {
        self.0.push(CastFail::local(spell_id, reason));
    }

    /// Queue a client-local refusal with an argument word, the crowd-control `0x8d`.
    pub(crate) fn push_local_arg(&mut self, spell_id: u32, reason: u8, arg: u32) {
        self.0.push(CastFail {
            spell_id,
            reason,
            arg: Some(arg),
            caster: Caster::Player,
            redisplay: false,
        });
    }

    /// Queue the pet's `SMSG_PET_CAST_FAILED` refusal, which carries no argument word
    /// (vmangos `Packets/Pet.cpp:127-132`).
    pub(crate) fn push_pet(&mut self, spell_id: u32, reason: u8) {
        self.0.push(CastFail {
            spell_id,
            reason,
            arg: None,
            caster: Caster::Pet,
            redisplay: false,
        });
    }
}

/// (Dis)mount refusals from `SMSG_MOUNTRESULT`/`SMSG_DISMOUNTRESULT` as `(mount, code)`, resolved
/// by [`mount_result_key`].
#[derive(Resource, Default)]
pub(crate) struct MountErrors(pub Vec<(bool, u32)>);

/// Taming refusals as the raw `SMSG_PET_TAME_FAILURE` reason. The reference (`0x6e6a20`) resolves
/// the reason's `PETTAME_*` string through the VM (`0x703bf0`) and passes it as `ERR_TAME_FAILED`'s
/// `%s` (`DisplayError(0xee)`), so both lookups wait for the VM at the drain.
#[derive(Resource, Default)]
pub(crate) struct PetTameFailures(pub Vec<u8>);

/// Where a message shows: the `kind` field (`+0x04`) of the reference's message record, read from
/// the catalog ([`benilla_ui::messages`]).
pub(crate) use benilla_ui::messages::MsgKind;

/// One argText argument; the list is ordered and mixed because a template's specifiers are:
/// `ERR_USE_LOCKED_WITH_SPELL_KNOWN_SI` takes a string then an integer (`0x5f34a9`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FillArg {
    S(String),
    D(i64),
}

/// One `DisplayError` (`0x496720`) message, whatever its surface: a GlobalStrings key and the
/// argText the reference pushes after the id, at most three strings (the guild event tail
/// `0x5e745f`). Extra arguments are ignored and a starved specifier shows verbatim, as
/// `SStrPrintf` does, so a short list is passed short.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct UiError {
    pub key: &'static str,
    pub args: Vec<FillArg>,
}

impl UiError {
    /// A message with no arguments.
    pub(crate) fn key(key: &'static str) -> Self {
        Self {
            key,
            args: Vec::new(),
        }
    }

    /// One string, the `_S` family.
    pub(crate) fn s(key: &'static str, s: impl Into<String>) -> Self {
        Self {
            key,
            args: vec![FillArg::S(s.into())],
        }
    }

    /// Several strings in push order, the `_SS`/`_SSS` family.
    pub(crate) fn strings(key: &'static str, args: &[&str]) -> Self {
        Self {
            key,
            args: args.iter().map(|s| FillArg::S((*s).to_string())).collect(),
        }
    }

    /// A mixed list, for a template whose specifiers are not all `%s`.
    pub(crate) fn args(key: &'static str, args: Vec<FillArg>) -> Self {
        Self { key, args }
    }

    /// The first string argument.
    #[cfg(test)]
    pub(crate) fn arg_s(&self) -> Option<&str> {
        self.args.iter().find_map(|a| match a {
            FillArg::S(s) => Some(s.as_str()),
            FillArg::D(_) => None,
        })
    }

    /// The first integer argument.
    #[cfg(test)]
    pub(crate) fn arg_d(&self) -> Option<i64> {
        self.args.iter().find_map(|a| match a {
            FillArg::D(d) => Some(*d),
            FillArg::S(_) => None,
        })
    }
}

/// Client-local refusals queued by GlobalStrings key: the `DisplayError` route for errors with no
/// wire code and no spell record.
#[derive(Resource, Default)]
pub(crate) struct UiErrorKeys(pub Vec<UiError>);

/// Lines that arrive already resolved, with no key and no catalog record: they reach the
/// reference's sink `0x4945b0(text, flag)` without `DisplayError`, flag 1 firing
/// `UI_ERROR_MESSAGE` and 0 `UI_INFO_MESSAGE`. `SMSG_NOTIFICATION` (`0x401800`) passes 1,
/// `SMSG_AREA_TRIGGER_MESSAGE` (`0x48f8ff`) passes 0.
#[derive(Resource, Default)]
pub(crate) struct UiErrorTexts(pub Vec<(String, MsgKind)>);

impl UiErrorTexts {
    /// The red `UI_ERROR_MESSAGE` arm (`0x4945b0(text, 1)`).
    pub(crate) fn error(&mut self, text: String) {
        self.0.push((text, MsgKind::Error));
    }

    /// The yellow `UI_INFO_MESSAGE` arm (`0x4945b0(text, 0)`).
    pub(crate) fn info(&mut self, text: String) {
        self.0.push((text, MsgKind::Info));
    }
}

/// Resolve a [`UiError`] to its text, the template filled in push order. Deviation: an absent or
/// empty result shows nothing, where the reference shows an empty line (no test of the string
/// between `0x4967d7` and `0x496842`), because a blank line is worse than none; only a chain
/// missing a stock key reaches it.
pub(crate) fn ui_error_text(e: &UiError, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let args: Vec<benilla_ui::strings::Arg<'_>> = e
        .args
        .iter()
        .map(|a| match a {
            FillArg::S(s) => benilla_ui::strings::Arg::S(s),
            FillArg::D(d) => benilla_ui::strings::Arg::D(*d),
        })
        .collect();
    let text = benilla_ui::strings::fill(&get(e.key)?, &args);
    (!text.is_empty()).then_some(text)
}

/// One resolved line: its text, its surface, and its catalog record, whose sound fields
/// (`+0x08`/`+0x0c`) `DisplayError` reads with the kind (`+0x04`).
pub(crate) struct Shown {
    record: Option<&'static MessageRecord>,
    kind: MsgKind,
    text: String,
}

impl Shown {
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// A catalog message by GlobalStrings key, the reference's `DisplayError(id)`: its record gives
    /// the surface and the sound. A key with no row cannot occur in the reference, where a message
    /// is an index, so it warns and falls back to a silent [`MsgKind::Error`]; the warning is the
    /// only check on keys the source walk for `ERR_` literals cannot see.
    pub(crate) fn keyed(key: &str, text: String) -> Self {
        let record = benilla_ui::messages::by_key(key);
        if record.is_none() {
            warn!("message {key:?} is not a catalog row — surface and sound are a guess");
        }
        Self {
            record,
            kind: record.map_or(MsgKind::Error, |r| r.kind),
            text,
        }
    }

    /// A line with no catalog row, the wire's own text ([`UiErrorTexts`]), which reaches the sink
    /// `0x4945b0` without a record and so makes no sound.
    pub(crate) fn unkeyed(kind: MsgKind, text: String) -> Self {
        Self {
            record: None,
            kind,
            text,
        }
    }
}

/// Resolve a message key against the VM's `GlobalStrings.lua` into a [`Shown`]; an absent or empty
/// string is `None`, [`ui_error_text`]'s deviation.
pub(crate) fn keyed_line(script: &UiScript, key: &'static str) -> Option<Shown> {
    let text = script.lua().globals().get::<String>(key).ok()?;
    (!text.is_empty()).then(|| Shown::keyed(key, text))
}

/// [`keyed_line`] for a `%s` template, each argument filling the next `%s`.
pub(crate) fn keyed_line_s(script: &UiScript, key: &'static str, args: &[&str]) -> Option<Shown> {
    let template = script.lua().globals().get::<String>(key).ok()?;
    let args: Vec<benilla_ui::strings::Arg<'_>> = args
        .iter()
        .copied()
        .map(benilla_ui::strings::Arg::S)
        .collect();
    let text = benilla_ui::strings::fill(&template, &args);
    (!text.is_empty()).then(|| Shown::keyed(key, text))
}

/// The one sink for a resolved line: show it on its [`MsgKind`]'s surface and queue its catalog
/// row's sound, which `crate::sound::message` plays after the input pass, the same frame. `who`
/// tags the debug line. [`MsgKind::Chat`] lines go out as `CHAT_MSG_SYSTEM`; the three rows that
/// ask for `CHAT_MSG_SKILL` ([`benilla_ui::messages::MessageRecord::chat_type`]) are not raised
/// yet.
pub(crate) fn show_messages(
    script: &mut UiScript,
    sink: &mut MessageSink,
    who: &str,
    lines: impl IntoIterator<Item = Shown>,
) {
    for Shown { record, kind, text } in lines {
        debug!("{who}: message ({kind:?}) {text:?}");
        // The reference sounds a message beside its text, so the sound shares the display's guards.
        if let Some(record) = record {
            sink.sounds.push(record);
        }
        match kind {
            MsgKind::Chat => {
                sink.chat
                    .push_event(ChatEvent::text_only(ChatEventKind::System, text));
            }
            MsgKind::Info => {
                script.fire_event("UI_INFO_MESSAGE", vec![ScriptValue::Str(text)]);
            }
            MsgKind::Error => {
                script.fire_event("UI_ERROR_MESSAGE", vec![ScriptValue::Str(text)]);
            }
        }
    }
}

/// Where a shown message lands besides the VM: the chat log and the sound queue, one parameter
/// because the feeds that raise messages are at Bevy's parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MessageSink<'w> {
    pub(crate) chat: ResMut<'w, ChatLog>,
    pub(crate) sounds: ResMut<'w, MessageSounds>,
}

/// The `UNIT_FIELD_FLAGS` crowd-control bits the attack-start validator refuses on, in its test
/// order (`0x612eec`). The reference can raise `ERR_ATTACK_PREVENTED_BY_MECHANIC_S` (`0xa9`) with a
/// mechanic name instead, by a choice not yet traced; only the plain form is raised here.
const ATTACK_FLAG_REFUSALS: [(u32, &str); 4] = [
    (0x0004_0000, "ERR_ATTACK_STUNNED"),
    (0x0002_0000, "ERR_ATTACK_PACIFIED"),
    (0x0080_0000, "ERR_ATTACK_FLEEING"),
    (0x0040_0000, "ERR_ATTACK_CONFUSED"),
];

/// Phase A of the attack-start validator `0x612df0`, the actor's own eligibility: the first
/// failing gate raises its `ERR_ATTACK_*` line and returns `true`, and no packet is sent. The
/// actor is the player for Attack (`0x6131aa`) and the pet for the pet bar (`0x4bd40d`). The
/// reference's dead test `0x605f30` also refuses a player with `PLAYER_FLAGS_GHOST`
/// (`[[obj+0xe68]+8]` bit 4); this tests health only.
pub(crate) fn attack_actor_refusal(
    actor: Option<&ObjectStore>,
    self_guid: Option<u64>,
    errors: &mut UiErrorKeys,
) -> bool {
    let Some(key) = attack_actor_blocked(actor, self_guid) else {
        return false;
    };
    debug!("attack refused locally by the actor's own state — {key}");
    errors.0.push(UiError::key(key));
    true
}

/// The same ladder without the message, in the reference's gate order. The world right-click
/// asks this: its path (`0x60bea0` to `0x5ecb70`) shows no error text and never reaches
/// `0x612df0`, whose callers are the pet bar, Attack and TryCast (`0x6e4efb`). That path's own
/// gate set overlaps this one but is not transcribed.
pub(crate) fn attack_actor_blocked(
    actor: Option<&ObjectStore>,
    self_guid: Option<u64>,
) -> Option<&'static str> {
    // An unresolved actor skips the chain, as in the reference (`0x4bd403`).
    let fields = &actor?.0;
    let key = if fields.unit_health().is_some_and(|h| h == 0) {
        "ERR_ATTACK_DEAD"
    } else if fields
        .unit_charmed_by()
        .is_some_and(|g| Some(g) != self_guid)
    {
        // Charmed by anyone but us; a unit we control may swing.
        "ERR_ATTACK_CHARMED"
    } else if let Some((_, key)) = ATTACK_FLAG_REFUSALS
        .iter()
        .find(|(bit, _)| fields.unit_flags() & bit != 0)
    {
        key
    } else if fields.unit_mount_display_id() > 0 {
        "ERR_ATTACK_MOUNTED"
    } else {
        return None;
    };
    Some(key)
}

/// The reference's pre-send check `0x6e4000`, run on every cast path before any packet: the first
/// missing totem (a presence test), then the first short reagent (a count), refuses locally as
/// `0x78` or `0x5c` and returns `true`. It is the only source of "Requires Mining Pick": vmangos
/// answers a pickless cast with `ITEM_GONE` (`Spell.cpp:7301`). A missing self store skips the
/// check, as the reference's active-player gate does.
pub(crate) fn reagent_totem_refusal(
    spell_id: u32,
    def: Option<&benilla_formats::SpellDisplay>,
    self_store: Option<&ObjectStore>,
    objects: &crate::net::Objects,
    errors: &mut CastErrors,
) -> bool {
    let (Some(d), Some(store)) = (def, self_store) else {
        return false;
    };
    // Totems before reagents, the reference's order.
    let reason = if first_missing_totem(d, store, objects).is_some() {
        0x78
    } else if first_short_reagent(d, store, objects).is_some() {
        0x5c
    } else {
        return false;
    };
    debug!("cast {spell_id} refused locally — missing totem/reagent ({reason:#04x})");
    errors.push_local(spell_id, reason);
    true
}

/// The first totem item absent from our bags: `0x6e4000`'s failing slot.
pub(super) fn first_missing_totem(
    d: &benilla_formats::SpellDisplay,
    store: &ObjectStore,
    objects: &crate::net::Objects,
) -> Option<u32> {
    d.totems
        .iter()
        .copied()
        .filter(|&t| t != 0)
        .find(|&t| count_of(&store.0, objects, t, InventoryScope::CARRIED) == 0)
}

/// The first reagent we hold too few of: `0x6e4000`'s failing slot.
pub(super) fn first_short_reagent(
    d: &benilla_formats::SpellDisplay,
    store: &ObjectStore,
    objects: &crate::net::Objects,
) -> Option<u32> {
    d.reagents
        .iter()
        .copied()
        .filter(|&(id, _)| id != 0)
        .find(|&(id, n)| count_of(&store.0, objects, id, InventoryScope::CARRIED) < n)
        .map(|(id, _)| id)
}

/// A (dis)mount result code to its GlobalStrings key, by vmangos `UnitMountResult` and
/// `UnitDismountResult` (`UnitDefines.h:842-863`); every key ships in 1.12, `ERR_MOUNT_OTHER`'s
/// "UNKNOWN MOUNT ERROR" included. The success codes (10, 3) show nothing.
pub(super) fn mount_result_key(mount: bool, code: u32) -> Option<&'static str> {
    if mount {
        match code {
            0 => Some("ERR_MOUNT_INVALIDMOUNTEE"),
            1 => Some("ERR_MOUNT_TOOFARAWAY"),
            2 => Some("ERR_MOUNT_ALREADYMOUNTED"),
            3 => Some("ERR_MOUNT_NOTMOUNTABLE"),
            4 => Some("ERR_MOUNT_NOTYOURPET"),
            5 => Some("ERR_MOUNT_OTHER"),
            6 => Some("ERR_MOUNT_LOOTING"),
            7 => Some("ERR_MOUNT_RACECANTMOUNT"),
            8 => Some("ERR_MOUNT_SHAPESHIFTED"),
            9 => Some("ERR_MOUNT_FORCEDDISMOUNT"),
            _ => None, // 10 is OK; an unknown code shows nothing
        }
    } else {
        match code {
            0 => Some("ERR_DISMOUNT_NOPET"),
            1 => Some("ERR_DISMOUNT_NOTMOUNTED"),
            2 => Some("ERR_DISMOUNT_NOTYOURPET"),
            _ => None, // 3 is OK
        }
    }
}
#[cfg(test)]
mod shown_tests {
    use super::{MessageSink, MessageSounds, MsgKind, Shown};
    use crate::ui_chat::ChatLog;
    use benilla_ui::script::UiScript;
    use bevy::prelude::*;

    /// Through [`super::show_messages`]: a keyed line queues its catalog row; an unkeyed line and
    /// an unknown key queue nothing.
    #[test]
    fn a_keyed_line_queues_its_row_and_an_unkeyed_one_queues_nothing() {
        let mut world = World::new();
        world.init_resource::<ChatLog>();
        world.init_resource::<MessageSounds>();
        let mut state = bevy::ecs::system::SystemState::<MessageSink>::new(&mut world);
        let mut script = UiScript::new().expect("VM");

        {
            let mut sink = state.get_mut(&mut world);
            super::show_messages(
                &mut script,
                &mut sink,
                "test",
                [
                    // A voiced refusal, a named cue, a chat row and a silent row.
                    Shown::keyed("ERR_OUT_OF_MANA", "Not enough mana".into()),
                    Shown::keyed("ERR_NEWTAXIPATH", "New flight path discovered!".into()),
                    Shown::keyed("ERR_ALREADY_IN_GROUP_S", "Bob is already in a group".into()),
                    Shown::keyed("ERR_CANT_STACK", "That item cannot stack".into()),
                    Shown::unkeyed(MsgKind::Error, "the server said so".into()),
                    Shown::keyed("ERR_NOT_A_REAL_MESSAGE", "invented".into()),
                ],
            );
        }
        state.apply(&mut world);

        let queued = world.resource::<MessageSounds>().queued().to_vec();
        let keys: Vec<&str> = queued.iter().map(|r| r.key).collect();
        assert_eq!(
            keys,
            [
                "ERR_OUT_OF_MANA",
                "ERR_NEWTAXIPATH",
                "ERR_ALREADY_IN_GROUP_S",
                "ERR_CANT_STACK",
            ],
            "the unkeyed line and the unknown key have no row to sound"
        );
        // What each asks the drain for.
        assert_eq!(queued[0].type_tag, 0x0f, "the voice line");
        assert_eq!(queued[1].sound, Some("TaxiNodeDiscovered"), "the named cue");
        assert_eq!(queued[2].kind, MsgKind::Chat, "a chat row sounds too");
        assert_eq!(
            (queued[3].type_tag, queued[3].sound),
            (0x44, None),
            "queued and silent — the drain's own no-op"
        );
    }
}

#[cfg(test)]
mod mount_error_tests {
    use super::mount_result_key;

    #[test]
    fn success_codes_are_silent_and_failures_map() {
        assert_eq!(mount_result_key(true, 10), None); // MOUNTRESULT_OK
        assert_eq!(mount_result_key(false, 3), None); // DISMOUNTRESULT_OK
        assert_eq!(mount_result_key(true, 2), Some("ERR_MOUNT_ALREADYMOUNTED"));
        assert_eq!(mount_result_key(true, 8), Some("ERR_MOUNT_SHAPESHIFTED"));
        assert_eq!(mount_result_key(false, 1), Some("ERR_DISMOUNT_NOTMOUNTED"));
        // Off-table codes show nothing.
        assert_eq!(mount_result_key(true, 11), None);
        assert_eq!(mount_result_key(false, 4), None);
    }

    #[test]
    fn every_mount_key_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        for (mount, codes) in [(true, 0..=9), (false, 0..=2)] {
            for code in codes {
                let key = mount_result_key(mount, code).expect("failure code maps");
                let text = g(key).unwrap_or_default();
                assert!(!text.is_empty(), "{key} missing from GlobalStrings");
            }
        }
        assert_eq!(
            g("ERR_MOUNT_ALREADYMOUNTED").unwrap(),
            "You're already mounted!"
        );
        assert_eq!(g("ERR_DISMOUNT_NOTMOUNTED").unwrap(), "You're not mounted!");
    }
}

#[cfg(test)]
mod ui_error_tests {
    use super::{ui_error_text, FillArg, UiError};

    fn filled(key: &'static str, s: Option<&str>, d: Option<u32>) -> UiError {
        let mut args = Vec::new();
        args.extend(s.map(|s| FillArg::S(s.to_string())));
        args.extend(d.map(|d| FillArg::D(i64::from(d))));
        UiError::args(key, args)
    }

    /// `%s` then `%d`; an absent or empty key shows nothing.
    #[test]
    fn fills_substitute_and_absent_keys_are_silent() {
        let get = |key: &str| match key {
            "REQ_S" => Some("Requires %s".to_string()),
            "REQ_SI" => Some("Requires %s %d".to_string()),
            "PLAIN" => Some("Can't attack while mounted.".to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        };
        let t = |e: &UiError| ui_error_text(e, &get);
        assert_eq!(
            t(&filled("REQ_S", Some("Herbalism"), None)).as_deref(),
            Some("Requires Herbalism")
        );
        assert_eq!(
            t(&filled("REQ_SI", Some("Mining"), Some(100))).as_deref(),
            Some("Requires Mining 100")
        );
        assert_eq!(
            t(&UiError::key("PLAIN")).as_deref(),
            Some("Can't attack while mounted.")
        );
        assert_eq!(t(&UiError::key("EMPTY")), None);
        assert_eq!(t(&UiError::key("ABSENT")), None);
    }

    /// The lock-refusal keys (the use sender `0x5f33e0`) against the shipped file.
    #[test]
    fn every_lock_refusal_key_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        assert_eq!(g("ERR_USE_LOCKED_WITH_SPELL_S").unwrap(), "Requires %s");
        assert_eq!(
            g("ERR_USE_LOCKED_WITH_SPELL_KNOWN_SI").unwrap(),
            "Requires %s %d"
        );
        assert_eq!(g("ERR_USE_LOCKED_WITH_ITEM_S").unwrap(), "Requires %s");
        assert_eq!(g("ERR_USE_LOCKED").unwrap(), "Item is locked.");
        assert_eq!(g("ERR_DOOR_LOCKED").unwrap(), "The door is locked.");
        assert_eq!(
            g("ERR_BUTTON_LOCKED").unwrap(),
            "That has already been used."
        );
        assert_eq!(g("ERR_USE_CANT_OPEN").unwrap(), "You can't open that.");
        // The engine's own refusal (`place_action`, errorId `0x9e`).
        assert_eq!(
            g("ERR_PASSIVE_ABILITY").unwrap(),
            "You can't put a passive ability in the action bar."
        );
        // The drain's `0x78` template.
        assert_eq!(g("SPELL_FAILED_TOTEMS").unwrap(), "Requires %s");

        // Through the formatter: the herb and vein lines.
        let herb = filled("ERR_USE_LOCKED_WITH_SPELL_S", Some("Herbalism"), None);
        assert_eq!(
            ui_error_text(&herb, &g).as_deref(),
            Some("Requires Herbalism")
        );
        let vein = filled(
            "ERR_USE_LOCKED_WITH_SPELL_KNOWN_SI",
            Some("Mining"),
            Some(155),
        );
        assert_eq!(
            ui_error_text(&vein, &g).as_deref(),
            Some("Requires Mining 155")
        );
    }
}

#[cfg(test)]
mod attack_actor_tests {
    use super::*;
    use benilla_protocol::ObjectFields;

    const HEALTH: u16 = 22;
    const FLAGS: u16 = 46;
    const CHARMEDBY: u16 = 10;
    const MOUNT: u16 = 133;

    fn actor(pairs: &[(u16, u32)]) -> ObjectStore {
        // A live, unowned, unmounted, unimpaired unit unless a case says otherwise.
        let mut all = vec![(HEALTH, 100)];
        all.extend_from_slice(pairs);
        ObjectStore(ObjectFields::from_pairs(&all))
    }

    fn refusal(store: Option<&ObjectStore>, self_guid: Option<u64>) -> Option<&'static str> {
        let mut errors = UiErrorKeys::default();
        let refused = attack_actor_refusal(store, self_guid, &mut errors);
        assert_eq!(
            refused,
            !errors.0.is_empty(),
            "a refusal and a message are the same event — the ref raises one per veto"
        );
        errors.0.first().map(|e| e.key)
    }

    /// Every gate of `0x612df0`'s Phase A, each on its own, against an otherwise-healthy actor.
    #[test]
    fn each_actor_gate_raises_its_own_error() {
        assert_eq!(
            refusal(Some(&actor(&[])), Some(7)),
            None,
            "a fit actor swings"
        );
        assert_eq!(
            refusal(Some(&actor(&[(HEALTH, 0)])), Some(7)),
            Some("ERR_ATTACK_DEAD")
        );
        assert_eq!(
            refusal(Some(&actor(&[(MOUNT, 1147)])), Some(7)),
            Some("ERR_ATTACK_MOUNTED")
        );
        for (bit, key) in ATTACK_FLAG_REFUSALS {
            assert_eq!(
                refusal(Some(&actor(&[(FLAGS, bit)])), Some(7)),
                Some(key),
                "unit flag {bit:#x}"
            );
        }
        // An unrelated flag bit is no refusal.
        assert_eq!(refusal(Some(&actor(&[(FLAGS, 0x1000)])), Some(7)), None);
    }

    /// `0x612e33` compares the charmer with the active player's guid.
    #[test]
    fn charmed_by_us_still_swings_and_charmed_away_does_not() {
        let mine = actor(&[(CHARMEDBY, 7), (CHARMEDBY + 1, 0)]);
        assert_eq!(refusal(Some(&mine), Some(7)), None);

        let theirs = actor(&[(CHARMEDBY, 9), (CHARMEDBY + 1, 0)]);
        assert_eq!(refusal(Some(&theirs), Some(7)), Some("ERR_ATTACK_CHARMED"));
        // An unknown own guid never matches another's charm.
        assert_eq!(refusal(Some(&theirs), None), Some("ERR_ATTACK_CHARMED"));
    }

    /// The reference's order: dead before the flags, the flags before mounted.
    #[test]
    fn the_first_failing_gate_names_the_message() {
        let both = actor(&[(HEALTH, 0), (MOUNT, 1147), (FLAGS, 0x0004_0000)]);
        assert_eq!(refusal(Some(&both), Some(7)), Some("ERR_ATTACK_DEAD"));
        let cc = actor(&[(MOUNT, 1147), (FLAGS, 0x0004_0000)]);
        assert_eq!(refusal(Some(&cc), Some(7)), Some("ERR_ATTACK_STUNNED"));
    }

    /// A descriptor with no health field yet is not a corpse.
    #[test]
    fn an_unresolved_actor_is_never_a_refusal() {
        assert_eq!(refusal(None, Some(7)), None);
        assert_eq!(
            refusal(Some(&ObjectStore(ObjectFields::default())), Some(7)),
            None
        );
    }

    #[test]
    fn every_attack_error_key_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        for key in [
            "ERR_NO_ATTACK_TARGET",               // 0xa0
            "ERR_INVALID_ATTACK_TARGET",          // 0xa1
            "ERR_ATTACK_STUNNED",                 // 0xa2
            "ERR_ATTACK_PACIFIED",                // 0xa3
            "ERR_ATTACK_MOUNTED",                 // 0xa4
            "ERR_ATTACK_FLEEING",                 // 0xa5
            "ERR_ATTACK_CONFUSED",                // 0xa6
            "ERR_ATTACK_CHARMED",                 // 0xa7
            "ERR_ATTACK_DEAD",                    // 0xa8
            "ERR_ATTACK_PREVENTED_BY_MECHANIC_S", // 0xa9
        ] {
            assert!(!g(key).unwrap_or_default().is_empty(), "{key} missing");
        }
        assert_eq!(g("ERR_ATTACK_DEAD").unwrap(), "Can't attack while dead.");
    }
}

#[cfg(test)]
mod totem_reagent_tests {
    use super::*;
    use benilla_formats::SpellDisplay;
    use benilla_protocol::ObjectFields;

    fn store() -> ObjectStore {
        // Empty bags: every count reads 0.
        ObjectStore(ObjectFields::default())
    }

    /// The object index the count walks read, with nothing streamed.
    fn objects() -> crate::ui_items::TestObjects {
        crate::ui_items::TestObjects::new()
    }

    fn spell(totems: [u32; 2], reagents: [(u32, u32); 8]) -> SpellDisplay {
        SpellDisplay {
            totems,
            reagents,
            ..Default::default()
        }
    }

    /// Against empty bags: a totem spell refuses `0x78`, a reagent spell `0x5c`, totems first; a
    /// spell with neither passes, and no spell or no store skips the check.
    #[test]
    fn missing_materials_refuse_with_the_refs_reasons() {
        let mut objs = objects();
        let st = store();
        let mining = spell([2901, 0], [(0, 0); 8]);
        let mut errors = CastErrors::default();
        assert!(reagent_totem_refusal(
            2575,
            Some(&mining),
            Some(&st),
            &objs.get(),
            &mut errors
        ));
        assert_eq!(errors.0.as_slice(), &[CastFail::local(2575, 0x78)]);

        let mut reagents = [(0, 0); 8];
        reagents[0] = (17056, 1); // Slow Fall's Light Feather
        let slow_fall = spell([0, 0], reagents);
        let mut errors = CastErrors::default();
        assert!(reagent_totem_refusal(
            130,
            Some(&slow_fall),
            Some(&st),
            &objs.get(),
            &mut errors
        ));
        assert_eq!(errors.0.as_slice(), &[CastFail::local(130, 0x5c)]);

        let both = spell([2901, 0], reagents);
        let mut errors = CastErrors::default();
        assert!(reagent_totem_refusal(
            1,
            Some(&both),
            Some(&st),
            &objs.get(),
            &mut errors
        ));
        assert_eq!(errors.0.as_slice(), &[CastFail::local(1, 0x78)]);

        let plain = spell([0, 0], [(0, 0); 8]);
        let mut errors = CastErrors::default();
        assert!(!reagent_totem_refusal(
            133,
            Some(&plain),
            Some(&st),
            &objs.get(),
            &mut errors
        ));
        assert!(!reagent_totem_refusal(
            2575,
            None,
            Some(&st),
            &objs.get(),
            &mut errors
        ));
        assert!(!reagent_totem_refusal(
            2575,
            Some(&mining),
            None,
            &objs.get(),
            &mut errors
        ));
        assert!(errors.0.is_empty());
    }

    /// Against empty bags, the first nonzero totem and reagent.
    #[test]
    fn first_failing_slot_is_named() {
        let mut objs = objects();
        let st = store();
        let mut reagents = [(0, 0); 8];
        reagents[1] = (17056, 1);
        let d = spell([0, 7005], reagents);
        assert_eq!(first_missing_totem(&d, &st, &objs.get()), Some(7005));
        assert_eq!(first_short_reagent(&d, &st, &objs.get()), Some(17056));
        let none = spell([0, 0], [(0, 0); 8]);
        assert_eq!(first_missing_totem(&none, &st, &objs.get()), None);
        assert_eq!(first_short_reagent(&none, &st, &objs.get()), None);
    }
}
