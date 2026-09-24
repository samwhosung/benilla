//! **The language gate** — everything that decides *whether* a chat line is garbled, and by how
//! much. The substitution itself is [`benilla_formats::garble`] (the reference's `0x49b560`); this
//! module is `0x49a870`'s share of the work: the exemptions, and the "how well does this character
//! know that language" answer the routine takes as its second argument.
//!
//! That answer is `0x5ec720`'s. The chain is entirely
//! client-side over server-synced skill data — **nothing is pushed as a "language list"**:
//!
//! ```text
//! language id ──(the spell whose Effect_1 == 39 declares it, over the KNOWN spells)──▶ spell id
//!             ──(that spell's SkillLineAbility row)──▶ SkillLine.dbc id
//!             ──(the player's PLAYER_SKILL_INFO block)──▶ skill value
//! ```
//!
//! Then `skill >= 300` is fluent and the line passes through untouched; below that every *word*
//! runs its own `hash % 300 < skill` test, so partial skill is graduated rather than a switch.
//!
//! **Why the first hop reads spell → language and not the reverse** is the shipped data, and it is
//! recorded on [`benilla_formats::SpellCatalog::declared_language`]: five spells declare Common
//! rather than their own language, so a whole-DBC map inverted the other way makes *Common* resolve
//! to Old Tongue and garbles everyone's own faction chat. Folding spell → language over the
//! character's own known-spell set is what the reference does (`0x4b2656` runs on spell *add*) and
//! it is the only shape that survives that data.

use std::collections::HashMap;

use benilla_formats::LanguageWords;
use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_spellbook::SkillLines;

/// `PLAYER_FLAGS_GM` — descriptor index 190, bit 3 (`0x49a9cc` reads `[[player+0xe68]+8] >> 3 & 1`).
const PLAYER_FLAGS_GM: u32 = 0x8;

/// The chat types that **force the language to 0** regardless of what the wire said
/// (`0x49a970`–`0x49a986`): EMOTE, SYSTEM, MONSTER_EMOTE and RAID_BOSS_EMOTE.
///
/// They are narration, not speech — "Thrall glares at you" is not spoken in Orcish — and forcing 0
/// suppresses both the garble and the `[Language]` header in one move, since the header keys off the
/// same field.
const ALWAYS_UNIVERSAL: [u8; 4] = [
    benilla_protocol::messages::CHAT_MSG_EMOTE,
    benilla_protocol::messages::CHAT_MSG_SYSTEM,
    benilla_protocol::messages::CHAT_MSG_MONSTER_EMOTE,
    benilla_protocol::messages::CHAT_MSG_RAID_BOSS_EMOTE,
];

/// The garble word pool plus the local character's fluency in each language — everything
/// [`effective_language`] and [`garble`] need, in one resource so the chat feed spends one system
/// param on the whole gate.
#[derive(Resource, Default)]
pub(crate) struct ChatLanguages {
    /// `LanguageWords.dbc`, loaded once. `None` while the chain has not been read (or if it could
    /// not be), in which case nothing garbles — a broken install renders chat plainly rather than
    /// blanking it.
    words: Option<LanguageWords>,
    /// language id → this character's skill in it. Absent = 0 = never understood.
    skill: HashMap<u32, u32>,
    /// `PLAYER_FLAGS & 0x8`. A GM sees **everything** plain, and with an empty `[Language]` header
    /// — the same bit also skips the profanity filter in the reference.
    gm: bool,
    /// Is there a local player object at all?
    ///
    /// `0x49b560`'s **second** early-out (`0x49b590` → `0x49b597 test eax,eax; je`) is exactly this:
    /// with no local player the line is copied verbatim. It matters because the alternative is
    /// worse than a divergence — with no body we would find no skill row, read 0, and garble
    /// *everything* rather than nothing. A line arriving before the self entity exists must render
    /// plainly, not as gibberish.
    have_player: bool,
}

impl ChatLanguages {
    /// This character's skill in `language` — the second argument of `0x49b560`.
    pub(crate) fn skill(&self, language: u32) -> u32 {
        self.skill.get(&language).copied().unwrap_or(0)
    }

    /// The language this line is **rendered** as, after the exemptions `0x49a870` applies before it
    /// ever reaches the garble routine. `0` means "render plain, and show no `[Language]` header" —
    /// which is one decision in the reference, not two, because the header reads the same field.
    ///
    /// The addon sentinel is *not* handled here: `language == -1` never reaches this path at all
    /// (it is dropped upstream as addon traffic, decision 1029), and it is a `u32` by the time we
    /// see it.
    /// **Is the viewer a GM?** — `PLAYER_FLAGS & 0x8`, read at the same `0x49a9cc` site the garble
    /// gate reads it at. The chat chokepoint's spam arm needs the identical bit (`0x49ab03`), and
    /// it is already here, so it is answered from here rather than re-derived from a second
    /// descriptor query (decision 2077).
    pub(crate) fn is_gm(&self) -> bool {
        self.gm
    }

    pub(crate) fn effective_language(&self, chat_type: u8, language: u32) -> u32 {
        if self.gm || !self.have_player || ALWAYS_UNIVERSAL.contains(&chat_type) {
            return 0;
        }
        language
    }

    /// Rewrite a line as this character hears it. Returns `text` unchanged for language 0, for a
    /// fluent listener, and when the pool is unavailable.
    pub(crate) fn garble(&self, language: u32, text: &str) -> String {
        let Some(words) = self.words.as_ref() else {
            return text.to_string();
        };
        benilla_formats::garble_chat(words, language, self.skill(language), text)
    }
}

/// Load `LanguageWords.dbc` once at startup ([`crate::ui_unit`]'s `load_default_languages` shape).
pub(super) fn load_language_words(
    mut langs: ResMut<ChatLanguages>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    if langs.words.is_some() {
        return;
    }
    let Some(assets) = assets else { return };
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_language_words(&mut chain)
    };
    match loaded {
        Ok(words) => {
            info!("ui_chat: {} language word pools", words.len());
            langs.words = Some(words);
        }
        // Not fatal: with no pool every line renders plain, which is what benilla did before B262.
        Err(e) => warn!("ui_chat: language word pools unavailable — {e:#}"),
    }
}

/// Recompute the per-language skill map and the GM bit.
///
/// Cheap and unconditional: the walk is over the character's own language spells (at most a
/// handful) and one pass of the 128-slot skill block per language, and it only runs when the spell
/// book or the self descriptors actually changed. The reference maintains the same information
/// incrementally, at spell-add time; a rebuild is the same answer without a hook, and a language
/// cannot change often enough for the difference to matter.
pub(super) fn feed_language_skills(
    mut langs: ResMut<ChatLanguages>,
    actions: Option<Res<PlayerActions>>,
    spells: Option<Res<Spells>>,
    skill_lines: Option<Res<SkillLines>>,
    self_q: Query<Ref<ObjectStore>, With<SelfPlayer>>,
) {
    let Ok(store) = self_q.single() else {
        // No body — the reference's "no local player" edge, which copies verbatim.
        if !langs.skill.is_empty() || langs.gm || langs.have_player {
            langs.skill.clear();
            langs.gm = false;
            langs.have_player = false;
        }
        return;
    };
    // The gate the doc above promises: the map is a function of the spell book, the two
    // catalogs and our own descriptor, so a frame with all four still — and a player already
    // folded in — has nothing to rebuild.
    let inputs_moved = store.is_changed()
        || actions.as_ref().is_some_and(|a| a.is_changed())
        || spells.as_ref().is_some_and(|s| s.is_changed())
        || skill_lines.as_ref().is_some_and(|l| l.is_changed());
    if !inputs_moved && langs.have_player {
        return;
    }
    let gm = store.0.player_flags() & PLAYER_FLAGS_GM != 0;

    let mut skill = HashMap::new();
    if let (Some(actions), Some(spells), Some(lines)) = (actions, spells, skill_lines) {
        for &known in &actions.spells {
            let Some(language) = spells.catalog.declared_language(known) else {
                continue;
            };
            let Some(line) = lines.catalog.spell_to_line(known) else {
                // The reference's own dead end: 25674 declares Draconic and has no
                // `SkillLineAbility` row, so `0x6de040` finds nothing and the language stays
                // unknown. Draconic is never understood in 1.12.1.
                continue;
            };
            // Ascending spell id, and the later write wins — the reference's `[0xb700ac][lang] =
            // spellId` is a plain store, and the initial spell batch arrives sorted.
            skill.insert(language, skill_value(&store, line));
        }
    }

    if skill != langs.skill || gm != langs.gm || !langs.have_player {
        // The instrument for this whole surface, and it earns its line: the gate is invisible from
        // the outside — a wrong answer here looks like "chat renders fine", which is precisely the
        // bug B262 reported. On change only, so it is one line per login, not per frame.
        //
        // **`gm=true` means nothing will EVER garble**, and GM mode is on by default on `probeN`
        // accounts (0679), so a probe hunting this surface reads its own answer here rather than
        // concluding the fix did not land.
        let mut named: Vec<String> = skill.iter().map(|(l, v)| format!("{l}:{v}")).collect();
        named.sort();
        info!("ui_chat: language fluency {{{}}} gm={gm}", named.join(" "));
        langs.skill = skill;
        langs.gm = gm;
        langs.have_player = true;
    }
}

/// Keep the chat frame's own `this.defaultLanguage` in step with the body — the **faction** tongue
/// `GetDefaultLanguage()` answers, which is what suppresses the `[Language]` header on your own
/// side's chat ([`super::frames::compose`]).
///
/// The reference resolves it per call from the live player object; we resolve it on change, which
/// is the same answer without the per-line churn — a race cannot change without a new world entry.
pub(super) fn feed_default_language(
    mut windows: ResMut<super::frames::ChatWindows>,
    langs: Option<Res<crate::ui_unit::DefaultLanguagesRes>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
) {
    let name = self_q
        .iter()
        .next()
        .and_then(|store| store.0.unit_race())
        .zip(langs.as_ref())
        .and_then(|(race, langs)| langs.0.name(u32::from(race), 0))
        .unwrap_or_default();
    if windows.default_language != name {
        windows.default_language = name.to_string();
    }
}

/// A skill line's effective value out of `PLAYER_SKILL_INFO`, exactly as `0x5ec720` sums it:
/// the base, plus the permanent bonus **only when the base is non-zero**, plus the signed temporary
/// bonus. `0` when the character has no row for the line.
fn skill_value(store: &ObjectStore, line: u32) -> u32 {
    for i in 0..benilla_protocol::messages::PLAYER_SKILL_SLOTS {
        let Some(s) = store.0.player_skill(i) else {
            continue;
        };
        if u32::from(s.skill_id) != line {
            continue;
        }
        let mut base = i32::from(s.value);
        if base != 0 {
            base += i32::from(s.perm_bonus);
        }
        return (base + i32::from(s.temp_bonus)).max(0) as u32;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{
        CHAT_MSG_EMOTE, CHAT_MSG_MONSTER_EMOTE, CHAT_MSG_MONSTER_SAY, CHAT_MSG_RAID_BOSS_EMOTE,
        CHAT_MSG_SAY, CHAT_MSG_SYSTEM,
    };

    fn langs(skill: &[(u32, u32)], gm: bool) -> ChatLanguages {
        ChatLanguages {
            words: None,
            skill: skill.iter().copied().collect(),
            gm,
            have_player: true,
        }
    }

    /// With no body, everything is Universal — the reference's second early-out. The failure this
    /// prevents is the loud one: no body means no skill rows, so a naive gate would read 0 for every
    /// language and garble the whole world instead of none of it.
    #[test]
    fn no_local_player_renders_every_line_plainly() {
        let mut l = langs(&[(7, 300)], false);
        l.have_player = false;
        assert_eq!(l.effective_language(CHAT_MSG_SAY, 1), 0);
        assert_eq!(l.effective_language(CHAT_MSG_MONSTER_SAY, 7), 0);
    }

    #[test]
    fn the_narration_types_are_always_universal() {
        let l = langs(&[], false);
        // Speech keeps its language...
        assert_eq!(l.effective_language(CHAT_MSG_SAY, 1), 1);
        assert_eq!(l.effective_language(CHAT_MSG_MONSTER_SAY, 1), 1);
        // ...narration does not, whatever the wire said.
        for t in [
            CHAT_MSG_EMOTE,
            CHAT_MSG_SYSTEM,
            CHAT_MSG_MONSTER_EMOTE,
            CHAT_MSG_RAID_BOSS_EMOTE,
        ] {
            assert_eq!(l.effective_language(t, 1), 0, "chat type {t:#x}");
        }
    }

    /// GM mode turns the whole thing off — the line renders plain **and** loses its header, because
    /// the reference expresses both by forcing the language field to 0.
    #[test]
    fn gm_mode_makes_every_line_universal() {
        let l = langs(&[], true);
        assert_eq!(l.effective_language(CHAT_MSG_SAY, 1), 0);
        assert_eq!(l.effective_language(CHAT_MSG_MONSTER_SAY, 7), 0);
    }

    #[test]
    fn an_unlisted_language_is_never_understood() {
        let l = langs(&[(7, 300)], false);
        assert_eq!(l.skill(7), 300);
        assert_eq!(l.skill(1), 0, "Orcish, which this character never learned");
        assert_eq!(l.skill(8), 0, "Demonic, which nobody can learn in 5875");
    }

    /// With no word pool nothing garbles — the pre-B262 behaviour, which is the right failure for a
    /// broken install (readable chat beats blank chat).
    #[test]
    fn a_missing_pool_renders_plainly_rather_than_blanking_chat() {
        let l = langs(&[], false);
        assert_eq!(l.garble(1, "hello there"), "hello there");
    }
}
