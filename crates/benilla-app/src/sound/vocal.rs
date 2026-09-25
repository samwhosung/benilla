//! **Vocal UI error speech** — the lines your own character says out loud when the client refuses
//! something: *"I can't do that yet."*, *"Not enough mana."*, *"Out of range."*.
//!
//! One engine function owns this in the reference — `SndInterfacePlayVocalUISound` `0x458250`,
//! whose **sole caller** is the message dispatcher `CGGameUI::DisplayError 0x496720` — and one
//! builder fills the table it reads, `0x4580f0`, called once from the local player's world-entry
//! (`0x5dea50`: race from the descriptor's own byte, sex from `0x5ed5b0`). Both are transcribed
//! here; the byte-level decode of each, and the shipped-data facts behind it, are in.
//!
//! ## The table (`0x4580f0` → `[0xb06240]`)
//!
//! `2 × 0x44` twelve-byte slots indexed `sex * 0x44 + line`, filled for the player's **race** from
//! `VocalUISounds.dbc` ([`benilla_formats::VocalUiSound`]). Each slot carries the ordinary kit, the
//! annoyed kit, and the annoyed kit's variation count — the reference reads that count at build
//! time through `0x45cda0(kit) + 0x94`, which is the runtime kit record's populated-`File[i]`
//! counter, i.e. [`benilla_formats::SoundKit::files`]`.len()`.
//!
//! **benilla resolves BOTH kits at build time; the reference resolves only the annoyed one there.**
//! Same answer, and deliberately so: the reference's play core looks the ordinary id up at play
//! time and treats an unresolvable one as "did not play" (`0x45cda0`'s `cmp ecx,count; jae → NULL`),
//! whereas [`kit::play_kit_ext`] reports an unknown kit as an **error** — which for a table where
//! 281 of 1 066 ids are `0`/`-1` would be a warning per refusal rather than the silence the
//! reference produces. Resolving once, up front, keeps the observable identical and the log clean.
//!
//! ## The cycle (`0x458250`)
//!
//! Transcribed in [`VocalSpeech::speak`], and it is stranger than "play the line":
//!
//! 1. A **different** line than last time resets the escalation state (`0x4582ad`).
//! 2. If escalation is armed, play the **annoyed** kit at variation `n` — an *explicit* index that
//!    walks `0, 1, 2, …` (the ordinary line uses `-1`, the weighted-random pool). Advance `n`; if
//!    the kit is exhausted or the play was refused, disarm and reset.
//! 3. **Then attempt the ordinary line anyway** — there is no early return at `0x458316`, the
//!    annoyed arm falls straight through into `0x458324`. What stops you hearing both is the
//!    **bus-1 cap of 1** ([`kit::Bus::ERROR_SPEECH`]): the annoyed line is already holding the
//!    single slot, so the second attempt is refused at the gate and reports back that it did not
//!    play. That refusal is why [`kit::play_kit_ext`] returns a bool at all.
//! 4. On an ordinary line that *did* play, count it; at **four** consecutive plays of the same
//!    line, arm the escalation and reset the counter (`0x458359 cmp eax,4`).
//!
//! **On 5875's data the escalation is inaudible, and that is a fact about the data, not a gap.**
//! Every one of `VocalUISounds.dbc`'s 1 066 annoyed ids is absent from `SoundEntries.dbc`, so the
//! variation count is always 0, step 2 always disarms on its first attempt, and step 3 always
//! plays the ordinary line — pinned by `benilla_formats`'
//! `the_annoyed_column_resolves_to_nothing_in_this_build`. The cycle is transcribed anyway
//! because it *is* the mechanism, on the same footing as the dormant `0x800` volume-variation gate
//! in [`super::kit`].
//!
//! ## Gates
//!
//! `MasterSoundEffects` **and** `EnableErrorSpeech`, both read before anything else happens
//! (`0x458264`/`0x45827f`) — so a player with error speech off does not silently advance the
//! escalation counter either. The second is a real 1.12 CVar (`CVar::Register` at `0x457877`,
//! default `"1"`) wired to the stock Sound panel's fourth checkbox; [`super::SoundConfig`] carries
//! it and `crate::cvars` registers it.

use bevy::prelude::*;

use benilla_formats::{SoundKitCatalog, VocalUiSoundCatalog, VOCAL_UI_LINES};

use crate::net::{ObjectStore, SelfPlayer};
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::kit::{self, Bus, KitRef, PlayExtras, SoundCategory, SoundKits};
use super::{SoundConfig, SoundOutput};

/// The message catalog's "**no** error speech" `type_tag` — and, not by coincidence, the table's
/// width: the reference tests `cmp [row+0xc],0x44` for equality to take the named-cue branch
/// (`0x49673d`) and `cmp edi,0x44; jge` as a bound on the line id (`0x45829c`), and the shipped
/// `VocalUISounds.dbc` stops one short at 67. So "the sentinel" and "one past the last line" are
/// the same number, and this constant is both.
pub(crate) const NO_SPEECH_TAG: u8 = VOCAL_UI_LINES as u8;

/// How many consecutive plays of one line arm the annoyed escalation — `0x458359 cmp eax,4`.
const ESCALATE_AFTER: u32 = 4;

/// One `sex × line` slot of the reference's `[0xb06240]` table, with both kits already resolved
/// (module doc). A slot with no ordinary kit is silent for that race/sex/line — 281 of the file's
/// 1 066 ids are `0` or `-1`, and that is what the data says rather than something to fill in.
#[derive(Clone, Copy, Default)]
struct Slot {
    normal: Option<u32>,
    annoyed: Option<u32>,
    /// The annoyed kit's variation count, the bound step 2 walks to (`0x4582f0`).
    annoyed_variations: u32,
}

/// `VocalUISounds.dbc`, loaded once.
#[derive(Resource)]
pub(crate) struct VocalUiSounds(pub(crate) VocalUiSoundCatalog);

/// The built table plus the reference's three escalation globals — `[0x835a40]`
/// `s_lastPlayedVocalUISound`, `[0xb05ee4]` (armed) and `[0xb05f78]` (the variation counter).
/// Kept together because `0x4580f0` resets all three when it rebuilds, and so does this.
#[derive(Resource)]
pub(crate) struct VocalSpeech {
    /// The race the table was built for — `None` until the local player's descriptor names one.
    race: Option<u32>,
    /// `2 * VOCAL_UI_LINES` slots, `sex * VOCAL_UI_LINES + line`.
    table: Vec<Slot>,
    last_line: u8,
    armed: bool,
    variation: u32,
}

impl Default for VocalSpeech {
    fn default() -> Self {
        Self {
            race: None,
            table: vec![Slot::default(); 2 * VOCAL_UI_LINES],
            // The builder's own reset value: `0x458109 mov [0x835a40],0x44` — the sentinel, so the
            // first line spoken after a rebuild is always "different from last time".
            last_line: NO_SPEECH_TAG,
            armed: false,
            variation: 0,
        }
    }
}

impl VocalSpeech {
    /// `0x4580f0` — fill the table for one race and reset the escalation state.
    ///
    /// Walked **backwards** over the file, which is the reference's own direction (`0x458140`
    /// counts down): on a duplicate `(race, line)` the lowest-indexed row is the one left
    /// standing. 5875's file has no duplicate pair, so this is a rule with no live case — kept
    /// because it costs a `.rev()` and losing it would be a silent divergence if a patch ever
    /// added one.
    fn build(&mut self, race: u32, catalog: &VocalUiSoundCatalog, kits: &SoundKitCatalog) {
        self.table.clear();
        self.table.resize(2 * VOCAL_UI_LINES, Slot::default());
        self.last_line = NO_SPEECH_TAG;
        self.armed = false;
        self.variation = 0;
        for row in catalog.rows().iter().rev() {
            if row.race != race {
                continue;
            }
            for sex in 0..2u32 {
                let live = |id: Option<u32>| id.filter(|k| kits.get(*k).is_some());
                let annoyed = live(row.pissed_kit(sex));
                self.table[sex as usize * VOCAL_UI_LINES + row.line as usize] = Slot {
                    normal: live(row.normal_kit(sex)),
                    annoyed,
                    annoyed_variations: annoyed
                        .and_then(|k| kits.get(k))
                        .map_or(0, |k| k.files.len() as u32),
                };
            }
        }
        self.race = Some(race);
        debug!(
            "sound(vocal): error-speech table built for race {race} — {} of {} slots voiced",
            self.table.iter().filter(|s| s.normal.is_some()).count(),
            self.table.len()
        );
    }

    /// Which race the table currently holds — the rebuild trigger, and what the tests read.
    fn built_for(&self) -> Option<u32> {
        self.race
    }

    /// `0x458250(sex, line)` — the whole cycle (module doc). `play` is the kit player, returning
    /// whether a channel actually opened; the bus-1 cap living inside it is what makes step 3
    /// silent while step 2 is sounding.
    fn speak(&mut self, sex: u32, line: u8, play: &mut dyn FnMut(u32, Option<usize>) -> bool) {
        // `0x458254`/`0x45825b`: anything but male/female leaves without touching a thing.
        if sex > 1 {
            return;
        }
        // `0x45829c cmp edi,0x44; jge` — a signed bound on the line, checked before the state is
        // touched, so a catalog row carrying the sentinel cannot disturb an escalation in flight.
        if usize::from(line) >= VOCAL_UI_LINES {
            return;
        }
        // `0x4582ad`: a different line resets the escalation. Note the reference stamps
        // `s_lastPlayedVocalUISound` *unconditionally* below, whether or not anything sounds.
        if line != self.last_line {
            self.armed = false;
            self.variation = 0;
        }
        self.last_line = line;

        let slot = self.table[sex as usize * VOCAL_UI_LINES + usize::from(line)];

        if self.armed {
            let played = self.variation < slot.annoyed_variations
                && slot
                    .annoyed
                    .is_some_and(|kit| play(kit, Some(self.variation as usize)));
            if played {
                self.variation += 1;
            } else {
                self.variation = 0;
                self.armed = false;
            }
        }

        // No early return above this line — `0x458316`'s `jmp` lands on the store, not on the
        // exit, so the ordinary attempt always happens (module doc step 3).
        if slot.normal.is_some_and(|kit| play(kit, None)) {
            self.variation += 1;
            if self.variation >= ESCALATE_AFTER {
                self.armed = true;
                self.variation = 0;
            }
        }
    }
}

fn load_vocal_ui_sounds(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_vocal_ui_sounds(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} vocal UI error-speech rows", cat.len());
            commands.insert_resource(VocalUiSounds(cat));
        }
        Err(e) => warn!("sound: vocal UI sounds failed to load: {e:#}"),
    }
}

/// Rebuild the table when the local player's race changes — the reference's world-entry call
/// (`0x5dea72`), reached here as a state watch rather than an event because benilla's descriptor
/// arrives incrementally and a character swap is just another race change. Cheap: it does nothing
/// at all until the race actually moves.
fn build_vocal_table(
    mut speech: ResMut<VocalSpeech>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    catalog: Option<Res<VocalUiSounds>>,
    kits: Option<Res<SoundKits>>,
) {
    let (Some(catalog), Some(kits)) = (catalog, kits) else {
        return;
    };
    let race = self_q.iter().next().and_then(|s| s.0.unit_race());
    let Some(race) = race.map(u32::from) else {
        return; // no descriptor yet — keep whatever table we have, like the reference does
    };
    if speech.built_for() == Some(race) {
        return;
    }
    speech.build(race, &catalog.0, kits.catalog());
}

/// Say one error-speech line in the local player's own voice — the app-side entry point
/// [`super::message`] calls, and the only caller there should ever be (the reference has exactly
/// one too).
pub(super) fn speak_line(
    line: u8,
    speech: &mut VocalSpeech,
    sex: u32,
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
) {
    // `0x458264`/`0x45827f`: BOTH CVars, ahead of every state write. A player with error speech
    // off is not merely muted — the escalation counter does not advance for them either.
    if !config.enabled || !config.error_speech {
        return;
    }
    let mut play = |kit: u32, variant: Option<usize>| {
        match kit::play_kit_ext(
            kits,
            assets,
            out,
            config,
            listener,
            KitRef::Id(kit),
            // 2D: `0x45ce60` opens through `0x7a5450`, the two-dimensional wrapper. It is your
            // own voice — no position, no rolloff.
            None,
            SoundCategory::Sfx,
            PlayExtras {
                variant,
                bus: Bus::ERROR_SPEECH,
                ..default()
            },
        ) {
            Ok(played) => played,
            Err(e) => {
                warn!("sound(vocal): error-speech kit {kit}: {e:#}");
                false
            }
        }
    };
    speech.speak(sex, line, &mut play);
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<VocalSpeech>()
        .add_systems(Startup, load_vocal_ui_sounds.after(AssetSet::Open))
        .add_systems(Update, build_vocal_table.in_set(WorldStage::Stream));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorder standing in for the kit player, with the **bus-1 cap of 1** modelled the way it
    /// actually bites: at most one play per `speak` call is admitted, because the reference's cap
    /// counts a channel that is still sounding and error speech has the bus to itself.
    #[derive(Default)]
    struct Rec {
        plays: Vec<(u32, Option<usize>)>,
        this_call: usize,
    }

    impl Rec {
        fn play(&mut self, kit: u32, variant: Option<usize>) -> bool {
            if self.this_call > 0 {
                return false; // bus 1, cap 1 — the slot is taken
            }
            self.this_call += 1;
            self.plays.push((kit, variant));
            true
        }
    }

    fn speech_with(normal: Option<u32>, annoyed: Option<u32>, variations: u32) -> VocalSpeech {
        let mut s = VocalSpeech {
            race: Some(1),
            ..Default::default()
        };
        s.table.fill(Slot {
            normal,
            annoyed,
            annoyed_variations: variations,
        });
        s
    }

    fn say(s: &mut VocalSpeech, rec: &mut Rec, sex: u32, line: u8) -> Vec<(u32, Option<usize>)> {
        rec.this_call = 0;
        let start = rec.plays.len();
        let mut f = |kit: u32, v: Option<usize>| rec.play(kit, v);
        s.speak(sex, line, &mut f);
        rec.plays[start..].to_vec()
    }

    /// **The 5875 case**: no annoyed audio, so the ordinary line plays every single time and the
    /// escalation is invisible — the behaviour a player actually gets today. The counter still
    /// turns over underneath (it arms on the 4th and disarms on the 5th), which is exactly why
    /// this has to be asserted rather than assumed.
    #[test]
    fn with_no_annoyed_audio_the_ordinary_line_plays_every_time() {
        let mut s = speech_with(Some(1875), None, 0);
        let mut rec = Rec::default();
        for i in 0..12 {
            assert_eq!(
                say(&mut s, &mut rec, 0, 0),
                vec![(1875, None)],
                "call {i} went quiet"
            );
        }
    }

    /// The escalation itself, with audio present: four ordinary plays, then the annoyed variations
    /// **in file order** (an explicit index, not the random pool), then back to ordinary. The
    /// ordinary attempt still happens on every annoyed call — the cap is what silences it, and the
    /// recorder proves only one sound per call comes out.
    #[test]
    fn four_of_the_same_line_escalates_then_walks_the_annoyed_variations() {
        let mut s = speech_with(Some(10), Some(20), 2);
        let mut rec = Rec::default();
        for i in 0..4 {
            assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(10, None)], "call {i}");
        }
        // Armed: the annoyed kit, variation 0 then 1 — and nothing else, though the ordinary line
        // was attempted both times.
        assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(20, Some(0))]);
        assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(20, Some(1))]);
        // Exhausted (2 variations): disarm, and the ordinary line is heard again on that very call.
        assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(10, None)]);
        for _ in 0..3 {
            assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(10, None)]);
        }
        // …and the cycle comes round again.
        assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(20, Some(0))]);
    }

    /// A different line resets the escalation — three of one line then a fourth of another does
    /// not make anybody annoyed (`0x4582ad`).
    #[test]
    fn a_different_line_resets_the_escalation() {
        let mut s = speech_with(Some(10), Some(20), 2);
        let mut rec = Rec::default();
        for _ in 0..3 {
            say(&mut s, &mut rec, 0, 5);
        }
        assert_eq!(say(&mut s, &mut rec, 0, 6), vec![(10, None)]);
        // Back to line 5: the count restarts from zero, so it takes a fresh FOUR — the three
        // before the interruption bought nothing.
        for i in 0..4 {
            assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(10, None)], "call {i}");
        }
        assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(20, Some(0))]);
    }

    /// A silent slot plays nothing and **does not count** — the reference advances its counter
    /// only on a play that returned a channel (`0x458344 je`), so a line your race has no audio
    /// for can never arm the escalation.
    #[test]
    fn a_voiceless_line_neither_sounds_nor_counts() {
        let mut s = speech_with(None, Some(20), 2);
        let mut rec = Rec::default();
        for _ in 0..10 {
            assert!(say(&mut s, &mut rec, 0, 5).is_empty());
        }
        assert!(!s.armed);
    }

    /// The two front-door refusals, neither of which may disturb state: a sex past female
    /// (`0x45825b`) and a line at or past the sentinel (`0x45829c`).
    #[test]
    fn the_bounds_refuse_without_touching_the_escalation() {
        let mut s = speech_with(Some(10), Some(20), 2);
        let mut rec = Rec::default();
        for _ in 0..3 {
            say(&mut s, &mut rec, 0, 5);
        }
        assert!(
            say(&mut s, &mut rec, 2, 5).is_empty(),
            "sex 2 is not a voice"
        );
        assert!(
            say(&mut s, &mut rec, 0, NO_SPEECH_TAG).is_empty(),
            "the sentinel is not a line"
        );
        // The line-5 run is intact, so the fourth call still escalates.
        assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(10, None)]);
        assert_eq!(say(&mut s, &mut rec, 0, 5), vec![(20, Some(0))]);
    }

    /// Male and female read **different slots** of one table — the `sex * 0x44` stride, which is
    /// the whole reason the table is 2 × 0x44 rather than 0x44.
    #[test]
    fn the_sexes_index_different_slots() {
        let mut s = VocalSpeech {
            race: Some(1),
            ..Default::default()
        };
        s.table[0].normal = Some(1875); // male, line 0
        s.table[VOCAL_UI_LINES].normal = Some(1999); // female, line 0
        let mut rec = Rec::default();
        assert_eq!(say(&mut s, &mut rec, 0, 0), vec![(1875, None)]);
        assert_eq!(say(&mut s, &mut rec, 1, 0), vec![(1999, None)]);
    }

    /// The table build, on the real shipped DBCs: Human male line 0 is `HumanMale_InventoryFull`,
    /// the annoyed column comes back empty (1815's dormancy), and a rebuild for another race
    /// replaces the whole table rather than merging into it. Skips without client data.
    #[test]
    fn the_real_table_builds_per_race_and_replaces_on_change() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_vocal_ui_sounds(&mut chain).expect("VocalUISounds");
        let kits = benilla_formats::load_sound_kit_catalog(&mut chain).expect("SoundEntries");

        let mut s = VocalSpeech::default();
        s.build(1, &catalog, &kits);
        assert_eq!(s.built_for(), Some(1));
        let human_male = s.table[0];
        assert_eq!(
            kits.get(human_male.normal.expect("Human male line 0"))
                .map(|k| k.name.as_str()),
            Some("HumanMale_InventoryFull")
        );
        assert_eq!(
            kits.get(s.table[VOCAL_UI_LINES].normal.expect("Human female line 0"))
                .map(|k| k.name.as_str()),
            Some("HumanFemale_InventoryFull")
        );
        assert!(
            s.table.iter().all(|slot| slot.annoyed.is_none()),
            "an annoyed kit resolved — 1815's dormancy is over"
        );

        // A race change is a full rebuild: Tauren line 0, not Human's, and no Human residue.
        s.build(6, &catalog, &kits);
        assert_eq!(
            kits.get(s.table[0].normal.expect("Tauren male line 0"))
                .map(|k| k.name.as_str()),
            Some("TaurenMale_InventoryFull")
        );
        // A race with no row for some line leaves that slot silent rather than the last race's.
        s.build(1, &catalog, &kits);
        let voiced_human = s.table.iter().filter(|s| s.normal.is_some()).count();
        s.build(9, &catalog, &kits); // Goblin: one row in the whole file
        let voiced_goblin = s.table.iter().filter(|s| s.normal.is_some()).count();
        assert!(
            voiced_goblin < voiced_human,
            "the rebuild merged instead of replacing ({voiced_goblin} vs {voiced_human})"
        );
    }
}
