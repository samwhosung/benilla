//! The red error line's queues and resolvers — every route into `UIErrorsFrame`'s top line
//! except the cast-fail two-layer display (its own module, [`super::cast_fail`]):
//!
//! - [`CastErrors`] — the wire [`CastFail`]s (`SMSG_CAST_RESULT` + local cast refusals), resolved
//!   by [`super::cast_fail`] at the drain.
//! - [`MountErrors`] — the `SMSG_MOUNTRESULT`/`SMSG_DISMOUNTRESULT` code pairs, resolved by
//!   [`mount_result_key`] (decision 0441 P2).
//! - [`UiErrorKeys`] — client-LOCAL refusals straight by GlobalStrings key, the
//!   `CGGameUI::DisplayError` route for errors with no wire code and no spell record:
//!   `ERR_ATTACK_MOUNTED` (decision 0481) and the GameObject lock-refusal toasts
//!   ("Requires Herbalism", decision 0545) and the guild lines (decision 2054) — the latter
//!   carry [`UiError`]'s ordered argText list, resolved by [`ui_error_text`].
//!
//! - [`UiErrorTexts`] — the same route for lines that arrive already resolved (the server's own
//!   `SMSG_NOTIFICATION` / `SMSG_AREA_TRIGGER_MESSAGE` text, the death durability notice), so
//!   there is no key to look up — only the arm to pick.
//!
//! All four drain in `super::feed_actions` into the one sink [`show_messages`], which puts each
//! resolved line on the surface its message record names — read from the catalog
//! ([`benilla_ui::messages`], decision 1770) rather than carried to the call site by hand.
//! Every string comes from the VM's own loaded `GlobalStrings.lua`, never hardcoded, so
//! localization rides for free. An absent key shows **nothing**, which is a deliberate
//! divergence and not the reference's behaviour — see [`ui_error_text`].

use benilla_ui::messages::MessageRecord;
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::net::ObjectStore;
use crate::sound::MessageSounds;
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
use crate::ui_items::{count_of, InventoryScope};

/// Cast failures queued for the UI error line — the wire triple from `SMSG_CAST_RESULT` and the
/// local refusals alike. The spell id rides along because the display layer keys several messages
/// on the failing spell's record ([`super::cast_fail`]: NO_POWER's power family, the 0x28/0x3c
/// cooldown families).
#[derive(Resource, Default)]
pub(crate) struct CastErrors(pub Vec<CastFail>);

/// One queued cast failure: the failing spell, the wire reason, and the reason-specific argument
/// word that fills the message's `%s` (the arm table at `0x6e1d8e` — a `SpellFocusObject.dbc` id
/// for 0x5e, an `AreaTable.dbc` id for 0x5d).
///
/// `arg` is `None` for **almost** every client-local refusal, and that is faithful rather than a
/// gap: the local refusals the reference raises itself go through `DisplayError` with no argText,
/// and the two client-generated argument messages it *does* build (the lock toasts, MIN_SKILL) are
/// [`UiError`]'s tenants, not this queue's.
///
/// The **one** exception is the crowd-control ladder's `0x8d` (decision 1948). That refusal is
/// local — `0x6094f0` bails before any packet — yet it carries an argument all the same, because
/// its own exemption scan produced one: the blocking aura's `SpellMechanic.dbc` id. It is the
/// only place a locally-generated word reaches this queue, which is why `push_local` still cannot
/// set one and [`CastErrors::push_local_arg`] exists beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CastFail {
    pub spell_id: u32,
    pub reason: u8,
    pub arg: Option<u32>,
    /// **Whose** refusal this is — which decides which of the reference's two message tables
    /// resolves it. See [`Caster`].
    pub caster: Caster,
    /// **This entry is a REDISPLAY** — it was queued once already, declined for want of an item
    /// template, and is being resolved now that the template landed (the `0x78`/`0x5c`/`0x84`
    /// cache-miss path, decisions 0545 + 0552).
    ///
    /// It exists because the reference's two attempts do not raise the same things (decision
    /// 2285). The first pass bails at `0x6e1eab`/`0x6e1efc` to the epilogue `0x6e224f`, which is
    /// past the red line AND past the combat-log formatter; the retry is the DBCACHECALLBACK
    /// `0x6e29b0`, which calls `CGGameUI::DisplayError 0x496720` **directly** and never reaches
    /// `0x62c360`. So a reagent refusal that had to wait for its item name shows the toast and is
    /// never written to the log — where one that resolved first try is written to both.
    pub redisplay: bool,
}

/// Who failed to cast — the one input that picks between the reference's **two** cast-failure
/// message tables (decision 2033).
///
/// It is not a flag on one handler: `SMSG_CAST_FAILED` and `SMSG_PET_CAST_FAILED` land in two
/// separate functions, `0x6e1a00` and `0x6e8eb0`, each with its own reason -> errorId map. Ten of
/// the pet's reasons resolve to something the player's never says, six of them to the
/// `ERR_PET_SPELL_*` catalog rows that exist for no other purpose. Everything else about the two
/// — the first-layer `SPELL_FAILED_*` vocabulary, the argument arms, the strip fallback — is
/// literally the same code, which is why this rides the shared queue instead of forking it.
///
/// [`super::cast_fail::cast_fail_text`] is the reader; nothing downstream of it asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Caster {
    /// Ours — `SMSG_CAST_FAILED` and every client-local refusal we raise for ourselves.
    #[default]
    Player,
    /// The pet's or the charm's — `SMSG_PET_CAST_FAILED` only.
    Pet,
}

impl CastFail {
    /// A **client-local** refusal — no wire argument (see [`Self::arg`]).
    ///
    /// Always the player's: every local refusal benilla raises is raised on its own cast, and the
    /// reference has no client-side path that refuses on the pet's behalf — a pet's refusals are
    /// all decided server-side and arrive as `SMSG_PET_CAST_FAILED`.
    pub(crate) const fn local(spell_id: u32, reason: u8) -> Self {
        Self {
            spell_id,
            reason,
            arg: None,
            caster: Caster::Player,
            redisplay: false,
        }
    }

    /// The same entry, re-queued for the frame its item template lands on — see
    /// [`Self::redisplay`].
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

    /// Queue a client-local refusal that carries its own argument word — see [`CastFail::arg`]
    /// for why there is exactly one such tenant.
    pub(crate) fn push_local_arg(&mut self, spell_id: u32, reason: u8, arg: u32) {
        self.0.push(CastFail {
            spell_id,
            reason,
            arg: Some(arg),
            caster: Caster::Player,
            redisplay: false,
        });
    }

    /// Queue the **pet's** wire refusal (`SMSG_PET_CAST_FAILED`). Same queue, same drain, same
    /// sink — [`Caster::Pet`] is the whole difference, and it is read once, at the table pick.
    ///
    /// No argument word: `PetCastFailed::AppendBodyTo` writes none, so the reference's own
    /// argument arms on this path have nothing to read.
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

/// (Dis)mount refusals queued for the UI error line, as the wire pair `(mount, code)` from
/// `SMSG_MOUNTRESULT`/`SMSG_DISMOUNTRESULT` (decision 0441 P2) — resolved to text through the
/// VM's own GlobalStrings by key ([`mount_result_key`]), the [`CastErrors`] shape exactly.
#[derive(Resource, Default)]
pub(crate) struct MountErrors(pub Vec<(bool, u32)>);

/// Taming refusals queued for the UI error line, as the raw `SMSG_PET_TAME_FAILURE` reason byte
/// (decision 2039) — [`MountErrors`]' shape, and here for the same reason: it is a **wire code
/// family**, resolved by a code table, not a client-local key.
///
/// It cannot ride [`UiErrorKeys`] because its message is a *nested* lookup, which is exactly what
/// the reference does: `0x6e6a20` resolves the reason's `PETTAME_*` key through the script VM
/// (`0x703bf0`) and then passes that **string** as `DisplayError(0xee)`'s argText, filling
/// `ERR_TAME_FAILED`'s lone `%s`. [`UiError`]'s arguments are text the raise site already has;
/// here the raise site has only a byte, and the VM is only reachable at the drain.
#[derive(Resource, Default)]
pub(crate) struct PetTameFailures(pub Vec<u8>);

/// Where a client message is SHOWN is **not a decision this crate makes** — it is the `kind` field
/// (`+0x04`) of the reference's message record, and it now arrives from the catalog
/// ([`benilla_ui::messages`], decision 1770) instead of being hand-carried to every call site
/// beside the key. [`MsgKind`] is that field; [`UiError::kind`] is the whole lookup.
///
/// It reads as a property of the MESSAGE rather than of the window that raised it because that is
/// exactly what it is: the quest refusals (0669), the auction house (1523) and the party
/// quest-share (1733) all ask the same table the same question.
pub(crate) use benilla_ui::messages::MsgKind;

/// One argument in a message's **argText list**.
///
/// Heterogeneous and ordered because the template's specifiers are: the lock-refusal toast
/// `ERR_USE_LOCKED_WITH_SPELL_KNOWN_SI` is String-then-Integer (its arm `0x5f34a9`,
/// decision 0545), and a list of strings beside a separate number cannot express that.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FillArg {
    S(String),
    D(i64),
}

/// One `DisplayError` message: a GlobalStrings key plus the **ordered argText list** the
/// reference pushes with it. Not red-line-only: this is the payload of the reference's ONE
/// `CGGameUI::DisplayError` (`0x496720`) whatever surface the message's record names — the quest
/// refusals carry their chat lines in it too (decision 0669).
///
/// **`0x496720` is variadic** — cdecl, the catalog id first and the argText after it, `add esp,4`
/// at a bare call site and `add esp,8` at a one-string one (22 of 22 in the guild command-result
/// handler `0x5e7520`). Three strings is the most any call site pushes: `SMSG_GUILD_EVENT`'s
/// shared emitter tail `0x5e745f` passes **1, 2 or 3** of them off the packet's `strCount`, feeding
/// `ERR_GUILD_PROMOTE_SSS`. This field was a single `fill_s` until decision 2054, which is why
/// the guild lines could not use this route at all and composed their own English instead.
///
/// A key whose string carries fewer specifiers than there are arguments ignores the rest; one
/// that carries more shows the starved specifier verbatim, which is `SStrPrintf`'s own behaviour
/// and the reason a short `strCount` is passed short rather than padded with empties.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct UiError {
    pub key: &'static str,
    pub args: Vec<FillArg>,
}

impl UiError {
    /// A fill-less message — the plain-key tenants (`ERR_ATTACK_MOUNTED`, the flag-locked
    /// strategy defaults). The reference's `push <id>; call 0x496720; add esp,4`.
    pub(crate) fn key(key: &'static str) -> Self {
        Self {
            key,
            args: Vec::new(),
        }
    }

    /// One string — the `_S` family, and the reference's `add esp,8`.
    pub(crate) fn s(key: &'static str, s: impl Into<String>) -> Self {
        Self {
            key,
            args: vec![FillArg::S(s.into())],
        }
    }

    /// Several strings, in the order they are pushed — the `_SS`/`_SSS` family.
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

    /// The first string argument — what a test asking "was the name filled in?" means.
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

/// Client-LOCAL refusals queued for the UI error line straight by GlobalStrings key — the
/// `CGGameUI::DisplayError` route for errors that carry no wire code and no spell record
/// (`ERR_ATTACK_MOUNTED` was the first tenant; the GameObject lock-refusal toasts of
/// decision 0545 are the formatted ones). The [`MountErrors`] shape without the code table.
#[derive(Resource, Default)]
pub(crate) struct UiErrorKeys(pub Vec<UiError>);

/// [`UiErrorKeys`]' twin for lines that arrive **already resolved** — text with no GlobalStrings
/// key to look up, because the server (or a fixed literal) already wrote it. Same frame, same
/// sink, same drain.
///
/// This is the one route that does **not** consult the catalog, and faithfully so: its tenants do
/// not go through `DisplayError` at all, so there is no record and no id to read a kind from. The
/// wire tenants are the reference's own: `SMSG_NOTIFICATION` (`0x1cb`, handler `0x401800` —
/// `mov edx,1; call 0x4945b0` → `UI_ERROR_MESSAGE`) and `SMSG_AREA_TRIGGER_MESSAGE` (`0x2b8`, the
/// shared handler `0x48f690`'s arm at `0x48f8ff` — `xor edx,edx; call 0x4945b0` →
/// `UI_INFO_MESSAGE`). `0x4945b0(text, flag)` is the whole sink: null/empty guard, then
/// `neg edx; sbb edx,edx; add edx,0xe1` = event `0xe1` when the flag is 0 and `0xe0` when it is 1.
/// The handler's own choice of flag is what the [`MsgKind`] here carries — the same two surfaces
/// the catalog names, reached by a different road.
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

/// Resolve one [`UiError`] to its displayed text — `GetText(key)` + the `%s`/`%d` argText
/// substitution ("Requires %s" + "Herbalism" → "Requires Herbalism").
/// `None` (the key resolves to nothing, or the filled text is empty) = show nothing — a
/// **NAMED DIVERGENCE**, corrected from a false citation this doc carried until 2246.
///
/// The claim was that this is the reference's own guard at `0x4967bd`/`0x4967c5`. It is not.
/// Those two test the **catalog row's key field** — `0x4967b6 mov ecx,[edx*4 + 0xb4b498]`, the
/// row's `+0x00`, null then first-byte-empty — i.e. whether the *record names a key at all*. In
/// benilla that field is a `&'static str` literal in a generated table, so the guard they model
/// can never fire, and the guard actually implemented here is one the reference does not have:
/// between the resolve (`0x4967d7 call 0x703bf0`) and the dispatch (`0x496842`) there is **no
/// test of the resolved string**. A key the player's `GlobalStrings.lua` does not define resolves
/// to the empty string and the reference emits an **empty** line — `0x49a870`'s own entry test
/// (`0x49a881 test esi,esi`) is on the buffer *pointer*, not its contents.
///
/// Kept because it only bites on a chain missing a stock key, where silence beats a blank line in
/// the log — but kept **named**, because the shape of the old comment (bytes cited for behaviour
/// those bytes do not justify) is the same shape as the `removed_spell` error 2246 corrects.
/// [`keyed_line`] and [`keyed_line_s`] carry the same divergence.
pub(crate) fn ui_error_text(e: &UiError, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    // Through the one shared filler (2045). This used `str::replace`, which fills EVERY `%s`
    // with the same argument — latent only because no message on this queue carried two yet.
    // The list's order is the reference's own push order, and a specifier with no argument left
    // is copied through rather than blanked, which is `SStrPrintf`'s behaviour and the whole
    // reason a short guild `strCount` is passed short (2054).
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

/// One resolved line on its way to the screen — the text, where it goes, and the **catalog row it
/// came from** when it has one.
///
/// The record rides along because a message's *sound* is one of its fields (`+0x08`/`+0x0c`) just
/// as its surface is (`+0x04`), and the reference reads all three in the same breath inside
/// `CGGameUI::DisplayError`. Carrying only the [`MsgKind`], as this did before 1815, threw the
/// other two away at the call site and left the catalog's sound columns unreadable by anything.
pub(crate) struct Shown {
    record: Option<&'static MessageRecord>,
    kind: MsgKind,
    text: String,
}

impl Shown {
    /// The resolved text — what the surface will show.
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// A **catalog** message, named by its GlobalStrings key — the reference's `DisplayError(id)`,
    /// which is nearly every line benilla shows. One lookup answers both "where does it go" and
    /// "what does it sound like".
    ///
    /// A key with no row falls back to [`MsgKind::Error`] and no sound. That cannot arise in the
    /// reference (a message is an *index*, so an unknown key is not expressible), so it only ever
    /// means benilla named a key the client does not have.
    ///
    /// `every_error_key_in_the_source_is_a_catalog_row` was supposed to keep that unreachable and
    /// **could not**: all 465 catalog keys begin `ERR_`, so that walk collects `"ERR_…"` literals
    /// — which makes it blind to exactly the mistake it is guarding against, a raise site that
    /// named something else. `PET_SPELL_NOPATH` and `SPELL_FAILED_OUT_OF_RANGE` sat on the pet's
    /// feedback line for months, took this fallback every time, and no gate could see them
    /// (decision 2033). Widening the walk is not possible from the text — `SPELL_FAILED_*` keys
    /// are legitimate GlobalStrings lookups elsewhere in this very module — so the tripwire moves
    /// to the funnel: this is where a non-row key becomes observable, so it says so.
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

    /// A line with **no catalog row** — the wire's own already-resolved text ([`UiErrorTexts`]),
    /// which reaches the reference's sink `0x4945b0` directly and never passes a record. Silent,
    /// faithfully: there is no `+0x08`/`+0x0c` to read.
    pub(crate) fn unkeyed(kind: MsgKind, text: String) -> Self {
        Self {
            record: None,
            kind,
            text,
        }
    }
}

/// Resolve a message key against the VM's own `GlobalStrings.lua` into a [`Shown`] — or `None`
/// when the key has no string there. Carrying a *key* rather than text at every raise site is the
/// reference's shape; suppressing on an **absent** one is [`ui_error_text`]'s named divergence,
/// not the `0x4967bd`/`0x4967c5` guard this line used to cite.
///
/// Lives here rather than in each window because every keyed raise site needs exactly this and
/// four of them had grown their own copy (decision 1821).
pub(crate) fn keyed_line(script: &UiScript, key: &'static str) -> Option<Shown> {
    let text = script.lua().globals().get::<String>(key).ok()?;
    (!text.is_empty()).then(|| Shown::keyed(key, text))
}

/// [`keyed_line`] for a `%s`-shaped string: each argument fills the next `%s`, in order — the
/// reference's `format`-with-pushed-arguments face of the same catalog rows (the battleground
/// join verdict's map name, the joined-or-left player's name, 1974).
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

/// **The one sink** for a resolved message line: put it on the surface its [`MsgKind`] names, and
/// queue the sound its catalog row names.
///
/// Every route into the client's message display ends here — the window queues that resolve a
/// [`UiError`] (the questgiver refusals 0669, the auction house 1523, the party quest-share 1733),
/// the client-local [`UiErrorKeys`], and the already-resolved [`UiErrorTexts`] off the wire. They
/// differ only in how they *build* a line and where the kind comes from; what happens to a built
/// one is this, once, so the three cannot drift apart.
///
/// The sound is **queued, not played** — this runs deep inside the UI feeds, which have no audio
/// resources and should not grow them; `crate::sound::message` drains the queue after the input
/// pass, on the same frame (decision 1815).
///
/// `who` is the caller's module tag for the debug line — which exists because the chat path keeps
/// no log of its own, so without it a live probe can count lines but never read one (0669's
/// in-app leg).
///
/// [`MsgKind::Chat`] lines go out as `CHAT_MSG_SYSTEM`. Three catalog rows ask for
/// `CHAT_MSG_SKILL` instead (the skill-up trio, [`benilla_ui::messages::MessageRecord::chat_type`])
/// and benilla raises none of them yet; when it does, this is the line that has to read the field.
pub(crate) fn show_messages(
    script: &mut UiScript,
    sink: &mut MessageSink,
    who: &str,
    lines: impl IntoIterator<Item = Shown>,
) {
    for Shown { record, kind, text } in lines {
        debug!("{who}: message ({kind:?}) {text:?}");
        // The reference sounds a message on the way past its text, not instead of it — so this
        // sits with the display and inherits its every guard (an empty line never gets here).
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

/// Everywhere a displayed message lands, besides the script VM: the chat window and the sound
/// queue. One [`SystemParam`] rather than two, because [`show_messages`] writes both on every
/// line and no caller wants one without the other — and because the feeds that raise messages are
/// already at Bevy's parameter ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MessageSink<'w> {
    pub(crate) chat: ResMut<'w, ChatLog>,
    pub(crate) sounds: ResMut<'w, MessageSounds>,
}

/// `UNIT_FIELD_FLAGS` bits the attack-start validator refuses on, paired with the message each
/// raises — read at `0x612eec`+ in the binary's own test order. All four are crowd control: the
/// actor is not refusing, it is unable.
///
/// Each has a second face in the reference — errorId `0xa9`
/// `ERR_ATTACK_PREVENTED_BY_MECHANIC_S`, which substitutes a resolved mechanic name — chosen by
/// five `0x6e9…` resolvers whose bodies are **not yet decoded**. We raise the plain form
/// only; the fill is a strictly better message for the same refusal, never a different one.
const ATTACK_FLAG_REFUSALS: [(u32, &str); 4] = [
    (0x0004_0000, "ERR_ATTACK_STUNNED"),
    (0x0002_0000, "ERR_ATTACK_PACIFIED"),
    (0x0080_0000, "ERR_ATTACK_FLEEING"),
    (0x0040_0000, "ERR_ATTACK_CONFUSED"),
];

/// Phase A of the shared attack-start validator `0x612df0` — **the actor's own eligibility**
/// (it supersedes the mounted-only fragment decision 0481 built from the one gate that was known
/// then).
///
/// `0x612df0(ecx = actor, &outGuid)` is ONE function with three call sites, and **the actor is
/// whoever the caller passes in `ecx`** — the player for the melee attack-start router `0x6131aa`,
/// and **the pet** for the pet bar's ATTACK arm (`0x4bd40d` passes `edi`, the pet object). Every
/// gate below reads the actor's own descriptor, so one function answers both; that is why this
/// takes an actor rather than reaching for the self store the way the fragment did.
///
/// A veto is total: `EAX = 0`, one of the ten consecutive `ERR_ATTACK_*` registry ids `0xa0`–`0xa9`
/// through `CGGameUI::DisplayError`, and **no packet at all** — on the pet arm the refusal's
/// `0x4bd414 je 0x4bd4c6` lands on the function epilogue, not on the shared send.
///
/// Order is the binary's, and it is observable: a dead **and** mounted actor says "Can't attack
/// while dead."
///
/// `dead` is `0x605f30(actor)`, which for any non-player actor — a pet never carries the player
/// typemask bit — degenerates to exactly `health <= 0`. Its further leg for a *player* actor
/// (`[[obj+0xe68]+8]` bit 4, reached only when health is positive, so plainly the ghost state) is
/// byte-read but its name is unconfirmed, so it is left to the caller: pass `dead` yourself when
/// you know more than the health field does.
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

/// The same ladder **without the message** — which condition blocks the swing, or `None`.
///
/// It exists because `0x612df0` is not on every attack-start path, and decision 1851 pinned which:
/// its three callers image-wide are the pet-attack command (`0x4bd40d`), the Attack
/// action/keybind (`0x6131aa`) and TryCast (`0x6e4efb`) — and **not** the world right-click.
/// That click runs `0x60bea0` → `0x60c247 call 0x5ecb70`, whose whole extent
/// `[0x5ecb70, 0x5ecda3)` contains no `call 0x496720` and no `push` of `0xa0`–`0xa9`: it has its
/// own short, silent conjunct set (`0x5ecc06` is its mounted bail) and every failure lands on
/// `0x5ecc37`, which bails or stops attacking. **A right-click on a hostile shows no error text
/// under any condition** — not mounted, dead, charmed, stunned, pacified, fleeing or confused.
///
/// So the click asks this, and the bar asks [`attack_actor_refusal`]. The *predicate* is still
/// `0x612df0`'s rather than `0x5ecb70`'s own — the two sets overlap but are not identical, and
/// transcribing `0x5ecb70`'s is its own slice — but the **silence** is now the verified law, and
/// the swing is still suppressed, which is the half that keeps us off the wire.
pub(crate) fn attack_actor_blocked(
    actor: Option<&ObjectStore>,
    self_guid: Option<u64>,
) -> Option<&'static str> {
    // No descriptor is no refusal. The reference resolves the actor object first and skips the
    // whole chain when it cannot (`0x4bd403`: no pet ⇒ send unmodified) — an un-streamed unit is
    // not an ineligible one.
    let fields = &actor?.0;
    let key = if fields.unit_health().is_some_and(|h| h == 0) {
        "ERR_ATTACK_DEAD"
    } else if fields
        .unit_charmed_by()
        .is_some_and(|g| Some(g) != self_guid)
    {
        // Charmed by somebody who is not us. Charmed BY US is not a refusal — a mind-controlled
        // unit is one you are allowed to swing with.
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

/// The ref's pre-send totem/reagent possession check — `CheckReagentsAndTotems 0x6e4000`
/// (decision 0552): TryCast runs it
/// for EVERY cast path (action bar, Lua, the GameObject-use opener) **before any packet is
/// built**. Totems first (2 slots, a bag **presence** test — the Mining Pick / Skinning Knife /
/// Thieves' Tools tools), then reagents (8 slots, a bag **count** test). The first failing slot
/// refuses the cast LOCALLY — reason `0x78`/`0x5c` into [`CastErrors`] (whose drain fills
/// "Requires <item>" / "Missing reagent: <item>") and **no send** — which is the only way
/// "Requires Mining Pick" can ever appear: vmangos answers a sent pickless cast with
/// `ITEM_GONE` ("Item is gone"), and its own source marks the totem reason "client-side only".
/// A missing self store skips the check, like the ref's `IsActivePlayer` gate (the client can
/// only see its own bags). Returns `true` when the cast must be refused.
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
    // Totems before reagents — the ref's in-function loop order.
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

/// The first totem (tool) slot whose item is absent from our bags — the `0x6e4000` totem loop's
/// failing slot, re-derived (a presence test: any owned count satisfies).
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

/// The first reagent slot whose owned count falls short — the `0x6e4000` reagent loop's failing
/// slot, re-derived.
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

/// The (dis)mount result code → its `ERR_MOUNT_*`/`ERR_DISMOUNT_*` GlobalStrings key. The code
/// tables are vmangos `UnitDefines.h` (`UnitMountResult`/`UnitDismountResult`); every key was
/// verified present in the shipped 1.12 `GlobalStrings.lua` (patch-2.MPQ, extracted 2026-07-17)
/// — including the deliberately-shipped `ERR_MOUNT_OTHER` = "UNKNOWN MOUNT ERROR" and the
/// INTERNAL-ERROR dismount strings. The success codes (10 mounting / 3 dismounting) are silent.
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
            _ => None, // 10 = OK; anything past the table stays the debug log's business
        }
    } else {
        match code {
            0 => Some("ERR_DISMOUNT_NOPET"),
            1 => Some("ERR_DISMOUNT_NOTMOUNTED"),
            2 => Some("ERR_DISMOUNT_NOTYOURPET"),
            _ => None, // 3 = OK
        }
    }
}
#[cfg(test)]
mod shown_tests {
    use super::{MessageSink, MessageSounds, MsgKind, Shown};
    use crate::ui_chat::ChatLog;
    use benilla_ui::script::UiScript;
    use bevy::prelude::*;

    /// The producer half of the message-sound path (decision 1815), driven through the real
    /// [`super::show_messages`] rather than around it: a **keyed** line queues its catalog row for
    /// `crate::sound::message` to sound, an **unkeyed** one queues nothing (the wire's own text has
    /// no record), and a key the client does not have queues nothing either — the same fallback
    /// that makes it a red line.
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
                    // A voiced refusal, a cue row, a chat row that still speaks, and one with
                    // neither — every arm of the record's two sound columns.
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
        // …and what each of them asks the drain for.
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
        // Off-table codes stay the debug log's business — no red line.
        assert_eq!(mount_result_key(true, 11), None);
        assert_eq!(mount_result_key(false, 4), None);
    }

    /// The RUNTIME leg on the real data (the `cast_fail` pattern): every key this table can
    /// emit resolves to a non-empty string in the shipped 1.12 `GlobalStrings.lua` — the guard
    /// against a typo'd key silently swallowing the red line. Skips without client data.
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

    /// The DisplayError argText substitution against a fake getter: `%s` then `%d`, key-absent
    /// and key-empty both silent (the GlobalStrings data-suppression face).
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

    /// The RUNTIME leg on the real data (the `cast_fail`/mount pattern): every GlobalStrings
    /// key the lock-refusal toasts (decision 0545, the USE sender `0x5f33e0`) and the totem
    /// fill can emit resolves in the shipped 1.12 `GlobalStrings.lua`, with the exact ref-quoted
    /// formats — the guard against a typo'd key silently swallowing the red line. Skips without
    /// client data.
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
        // The ENGINE's own by-key refusal (`benilla_ui`'s `place_action`, errorId `0x9e`). The
        // key is the load-bearing half: an unresolvable one makes `ui_error_text` answer None and
        // the refusal quietly goes back to being the divergence 0666 named.
        assert_eq!(
            g("ERR_PASSIVE_ABILITY").unwrap(),
            "You can't put a passive ability in the action bar."
        );
        // The wire-side totem fill's template (feed_actions' 0x78 arm): "Requires Mining Pick".
        assert_eq!(g("SPELL_FAILED_TOTEMS").unwrap(), "Requires %s");

        // End to end through the formatter — the two gathering lines the director will see.
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
        // An unrelated flag bit is not a refusal — the mask is four specific bits, not "any flag".
        assert_eq!(refusal(Some(&actor(&[(FLAGS, 0x1000)])), Some(7)), None);
    }

    /// `0x612e33`'s charm test compares against the ACTIVE PLAYER's guid, so being charmed **by
    /// us** is not a refusal — that is the whole point of mind control, and reading the field as a
    /// plain "is charmed" boolean would make every controlled unit unable to swing.
    #[test]
    fn charmed_by_us_still_swings_and_charmed_away_does_not() {
        let mine = actor(&[(CHARMEDBY, 7), (CHARMEDBY + 1, 0)]);
        assert_eq!(refusal(Some(&mine), Some(7)), None);

        let theirs = actor(&[(CHARMEDBY, 9), (CHARMEDBY + 1, 0)]);
        assert_eq!(refusal(Some(&theirs), Some(7)), Some("ERR_ATTACK_CHARMED"));
        // Not knowing our own guid yet cannot make somebody else's charm look like ours.
        assert_eq!(refusal(Some(&theirs), None), Some("ERR_ATTACK_CHARMED"));
    }

    /// The order is the binary's, and it is observable: the FIRST gate to fail names the message,
    /// so a dead-and-mounted actor says "Can't attack while dead."
    #[test]
    fn the_first_failing_gate_names_the_message() {
        let both = actor(&[(HEALTH, 0), (MOUNT, 1147), (FLAGS, 0x0004_0000)]);
        assert_eq!(refusal(Some(&both), Some(7)), Some("ERR_ATTACK_DEAD"));
        // …and with the death removed, the flag block still precedes the mount check.
        let cc = actor(&[(MOUNT, 1147), (FLAGS, 0x0004_0000)]);
        assert_eq!(refusal(Some(&cc), Some(7)), Some("ERR_ATTACK_STUNNED"));
    }

    /// An un-streamed actor is not an ineligible one: the reference skips the whole chain when it
    /// cannot resolve the object (`0x4bd403` — no pet ⇒ send unmodified). A descriptor that simply
    /// has not sent a health field yet must not read as a corpse.
    #[test]
    fn an_unresolved_actor_is_never_a_refusal() {
        assert_eq!(refusal(None, Some(7)), None);
        assert_eq!(
            refusal(Some(&ObjectStore(ObjectFields::default())), Some(7)),
            None
        );
    }

    /// The RUNTIME leg (the mount/lock-refusal pattern): all ten of the consecutive `ERR_ATTACK_*`
    /// registry ids `0xa0`–`0xa9` resolve to non-empty strings in the shipped 1.12
    /// `GlobalStrings.lua`. This is the guard against a typo'd key turning a refusal into a silent
    /// one — a veto that shows nothing is indistinguishable from a click that did nothing.
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
        // One key's text quoted from the file, as a spot check that the ids line up with the keys.
        assert_eq!(g("ERR_ATTACK_DEAD").unwrap(), "Can't attack while dead.");
    }
}

#[cfg(test)]
mod totem_reagent_tests {
    use super::*;
    use benilla_formats::SpellDisplay;
    use benilla_protocol::ObjectFields;

    fn store() -> ObjectStore {
        // Empty bags: no pack fields streamed → every count reads 0, everything is "missing".
        ObjectStore(ObjectFields::default())
    }

    /// The object index the count walks read — nothing streamed, which is the empty-bags pole
    /// every case here is graded at (2334).
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

    /// The pre-send check's routing (`0x6e4000`, decision 0552) against empty bags: a totem
    /// spell refuses 0x78, a reagent spell 0x5c, totems win when both lack (the ref's loop
    /// order), a materials-free spell passes, and absent def/store skip the check (the ref's
    /// `IsActivePlayer` gate) — the cast then goes out for the server to judge.
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

    /// The failing-slot selection the fill re-derives: the first MISSING totem / first SHORT
    /// reagent (against empty bags, the first nonzero of each).
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
