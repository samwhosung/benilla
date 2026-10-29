//! The language gate, `0x49a870`'s share of garbling: the exemptions, and how well the character
//! knows the language, the second argument of the substitution [`benilla_formats::garble`]
//! (`0x49b560`). Fluency is `0x5ec720`'s client-side chain; the server sends no language list:
//! language → the known spell whose effect 39 declares it → its `SkillLineAbility` row → the
//! `PLAYER_SKILL_INFO` value. At 300 a line passes whole; below, each word passes when
//! `hash % 300 < skill`. The fold runs spell → language over known spells only (`0x4b2656`, on
//! spell add): five spells declare Common, so the inverse would map Common to Old Tongue.

use std::collections::HashMap;

use benilla_formats::LanguageWords;
use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_spellbook::SkillLines;

/// The GM bit of `PLAYER_FLAGS` (descriptor 190), as `0x49a9cc` reads it.
const PLAYER_FLAGS_GM: u32 = 0x8;

/// Chat types forced to language 0 whatever the wire says (`0x49a970`-`0x49a986`), which drops
/// both the garble and the `[Language]` header.
const ALWAYS_UNIVERSAL: [u8; 4] = [
    benilla_protocol::messages::CHAT_MSG_EMOTE,
    benilla_protocol::messages::CHAT_MSG_SYSTEM,
    benilla_protocol::messages::CHAT_MSG_MONSTER_EMOTE,
    benilla_protocol::messages::CHAT_MSG_RAID_BOSS_EMOTE,
];

/// The garble word pools, `LanguageWords.dbc`, loaded once; the character's side of the gate is
/// read where a line or a page is shown ([`LanguageReads`]).
#[derive(Resource, Default)]
pub(crate) struct ChatLanguages {
    /// `LanguageWords.dbc`; while `None`, nothing garbles.
    words: Option<LanguageWords>,
}

impl ChatLanguages {
    /// The word pools `words`, for a test.
    #[cfg(test)]
    pub(crate) fn for_test(words: LanguageWords) -> Self {
        Self { words: Some(words) }
    }
}

/// The language gate as the character stands where it is read: the word pools, this character's
/// skill per language, the GM bit and whether a local player is held at all.
pub(crate) struct Gate<'a> {
    words: Option<&'a LanguageWords>,
    /// language id → this character's skill in it. Absent = 0 = never understood.
    skill: HashMap<u32, u32>,
    /// `PLAYER_FLAGS & 0x8`: a GM reads every line plain, with no `[Language]` header.
    gm: bool,
    /// With no local player a line is copied verbatim (`0x49b597`), not garbled for want of skill.
    have_player: bool,
}

impl Gate<'_> {
    /// This character's skill in `language`, the second argument of `0x49b560`.
    pub(crate) fn skill(&self, language: u32) -> u32 {
        self.skill.get(&language).copied().unwrap_or(0)
    }

    /// Whether the viewer is a GM (`0x49a9cc`); the spam filter reads the same bit (`0x49ab03`).
    pub(crate) fn is_gm(&self) -> bool {
        self.gm
    }

    /// The language a line renders as after `0x49a870`'s exemptions; 0 renders plain, no header.
    pub(crate) fn effective_language(&self, chat_type: u8, language: u32) -> u32 {
        if self.gm || !self.have_player || ALWAYS_UNIVERSAL.contains(&chat_type) {
            return 0;
        }
        language
    }

    /// The line as this character hears it: unchanged for language 0, fluency, or no word pool.
    pub(crate) fn garble(&self, language: u32, text: &str) -> String {
        let Some(words) = self.words else {
            return text.to_string();
        };
        benilla_formats::garble_chat(words, language, self.skill(language), text)
    }

    /// A readable's page text as this character reads it (`0x4e3856` → `0x49b560`, `keepAngle`
    /// and `keepPunct` set, 0x1f40 bytes): the character's skill in `language`, or 0 when the show
    /// is not `translate`d (a reason-1 read reply, `0x4e37b3`/`0x4e3808`). Unchanged for language
    /// 0, full skill, and no local player (`0x49b597`).
    pub(crate) fn garble_item_text(&self, language: u32, translate: bool, text: &str) -> String {
        let Some(words) = self.words.filter(|_| self.have_player) else {
            return text.to_string();
        };
        let skill = if translate { self.skill(language) } else { 0 };
        benilla_formats::garble(
            words,
            language,
            skill,
            text,
            benilla_formats::Garble::ITEM_TEXT,
        )
    }
}

/// Load `LanguageWords.dbc` once at startup.
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
        // Not fatal: with no pool every line renders plain.
        Err(e) => warn!("ui_chat: language word pools unavailable — {e:#}"),
    }
}

/// What the language gate reads where a line or a page is shown: the reference reads the skill
/// live at the show (`0x4e3837` → `GetIndexedSpellSkill`) and at the garble (`0x5ec720`), off the
/// language table its spell add keeps (`0x4b2656`), so a show inside a packet reads the skill an
/// earlier packet of its frame wrote.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct LanguageReads<'w, 's> {
    /// Absent in a harness without chat, where nothing garbles.
    langs: Option<Res<'w, ChatLanguages>>,
    actions: Option<Res<'w, PlayerActions>>,
    spells: Option<Res<'w, Spells>>,
    skill_lines: Option<Res<'w, SkillLines>>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
}

impl LanguageReads<'_, '_> {
    /// The gate as the character stands now: the language table folded over the known spells,
    /// the later spell id winning, as the reference's plain store `[0xb700ac][lang] = spellId`
    /// over the sorted initial spell batch, and each language's skill off the skill line's
    /// `PLAYER_SKILL_INFO` value.
    pub(crate) fn gate(&self) -> Gate<'_> {
        let words = self.langs.as_deref().and_then(|l| l.words.as_ref());
        let Ok(store) = self.me.single() else {
            // No body: every line renders plain.
            return Gate {
                words,
                skill: HashMap::new(),
                gm: false,
                have_player: false,
            };
        };
        let mut skill = HashMap::new();
        if let (Some(actions), Some(spells), Some(lines)) = (
            self.actions.as_deref(),
            self.spells.as_deref(),
            self.skill_lines.as_deref(),
        ) {
            let (race, class) = store.race_class();
            for &known in &actions.spells {
                let Some(language) = spells.catalog.declared_language(known) else {
                    continue;
                };
                let Some(line) = lines.catalog.spell_to_line(known, race, class) else {
                    // Spell 25674 declares Draconic but has no `SkillLineAbility` row
                    // (`0x6de040`), so Draconic is never understood.
                    continue;
                };
                skill.insert(language, skill_value(store, line));
            }
        }
        Gate {
            words,
            skill,
            gm: store.0.player_flags() & PLAYER_FLAGS_GM != 0,
            have_player: true,
        }
    }
}

/// Keep `this.defaultLanguage` at the faction tongue `GetDefaultLanguage()` answers, which the
/// `[Language]` header test in [`super::frames::compose`] reads.
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
        .and_then(|(race, langs)| langs.0.name(u32::from(race)))
        .unwrap_or_default();
    if windows.default_language != name {
        windows.default_language = name.to_string();
    }
}

/// A skill's `PLAYER_SKILL_INFO` value as `0x5ec720` sums it: base, plus the permanent bonus only
/// when base is non-zero, plus the signed temporary bonus; 0 with no row.
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

/// A character who knows a language, for the tests that read the gate through the world.
#[cfg(test)]
pub(crate) mod test_speaker {
    use super::*;
    use benilla_formats::{SkillLineCatalog, SpellCatalog, SpellDisplay};

    /// Language: Orcish, which declares the language its caller names.
    pub(crate) const SPELL: u32 = 669;
    /// Its skill line, Language: Orcish.
    pub(crate) const LINE: u32 = 109;

    /// The tables of a character who knows `language` through [`SPELL`] on [`LINE`]: the spell's
    /// declaration, its line, and the spell known. The skill is the player's own field
    /// ([`skill_fields`]).
    pub(crate) fn know(world: &mut World, language: u32) {
        let mut d = SpellDisplay {
            id: SPELL,
            name: "Language".into(),
            ..Default::default()
        };
        d.effects[0] = 39;
        d.effect_misc_value[0] = language as i32;
        world.insert_resource(Spells {
            catalog: SpellCatalog::from_displays([(SPELL, d)].into_iter().collect()),
            ..Spells::empty_for_tests()
        });
        world.insert_resource(SkillLines {
            catalog: SkillLineCatalog::from_spell_lines([(SPELL, LINE)]),
        });
        world.init_resource::<PlayerActions>();
        world.resource_mut::<PlayerActions>().spells.insert(SPELL);
    }

    /// `PLAYER_SKILL_INFO`'s first slot holding [`LINE`] at `value`, as an update's fields.
    pub(crate) fn skill_fields(value: u16) -> [(u16, u32); 2] {
        let base = benilla_protocol::field::FIELD_PLAYER_SKILL_INFO_1_1;
        [(base, LINE), (base + 1, u32::from(value) | 300 << 16)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{
        CHAT_MSG_EMOTE, CHAT_MSG_MONSTER_EMOTE, CHAT_MSG_MONSTER_SAY, CHAT_MSG_RAID_BOSS_EMOTE,
        CHAT_MSG_SAY, CHAT_MSG_SYSTEM,
    };

    fn langs(skill: &[(u32, u32)], gm: bool) -> Gate<'static> {
        Gate {
            words: None,
            skill: skill.iter().copied().collect(),
            gm,
            have_player: true,
        }
    }

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

    #[test]
    fn a_missing_pool_renders_plainly_rather_than_blanking_chat() {
        let l = langs(&[], false);
        assert_eq!(l.garble(1, "hello there"), "hello there");
    }
}
