//! Emote data: `EmotesText` (`ID, Name, EmoteID, EmoteText[16]`, the `/wave` command table)
//! joined to `EmotesTextSound` (`ID, EmotesTextID, RaceID, SexID, SoundID`, the voice kits)
//! and `Emotes`, the anim emotes:
//! `ID, SlashCommand, AnimID, EmoteFlags, EmoteSpecProc, EmoteSpecProcParam, EventSoundID`.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

/// The joined emote tables.
pub struct EmoteSoundCatalog {
    /// Uppercased `EmotesText.Name` ("WAVE") to text-emote id.
    by_name: HashMap<String, u32>,
    /// Text-emote id to the `Emotes.dbc` id it plays, 0 for chat-only.
    text_emote: HashMap<u32, u32>,
    /// `(text-emote id, race, sex)` to voice kit.
    voice: HashMap<(u32, u32, u32), u32>,
    event_sound: HashMap<u32, u32>,
    anim: HashMap<u32, u32>,
    emote_flags: HashMap<u32, u32>,
    /// `Emotes.dbc` id to its `EmoteSpecProc`; `2` is a looping state emote whose `EventSoundID`
    /// the `$ESD` anim event rings (`0x6239f0` tests `row[+0x10] == 2`).
    spec_proc: HashMap<u32, u32>,
    /// `Emotes.dbc` id to its `EmoteSpecProcParam`, the stand state when `EmoteSpecProc` is 1.
    spec_proc_param: HashMap<u32, u32>,
    /// The five gesture slots, in [`GESTURE_FLAG_BITS`] order.
    gesture: [Option<u32>; GESTURE_FLAG_BITS.len()],
}

/// The `EmoteFlags` bits the client scans `Emotes.dbc` for to build its gesture table
/// (`0x603a60`), in slot order: talk, question, exclamation, shout, laugh. It names no row id.
pub const GESTURE_FLAG_BITS: [u32; 5] = [0x08, 0x10, 0x20, 0x40, 0x100];

impl EmoteSoundCatalog {
    /// Resolve a `/command` name (case-insensitive) to its text-emote id.
    pub fn text_id(&self, name: &str) -> Option<u32> {
        self.by_name.get(&name.to_ascii_uppercase()).copied()
    }

    /// The voice kit for a performer of the given race/sex (`sex`: 0 male, 1 female).
    pub fn voice(&self, text_id: u32, race: u32, sex: u32) -> Option<u32> {
        self.voice.get(&(text_id, race, sex)).copied()
    }

    /// The anim emote a text emote plays.
    pub fn text_emote(&self, text_id: u32) -> Option<u32> {
        self.text_emote.get(&text_id).copied().filter(|&e| e != 0)
    }

    /// An anim emote's event-sound kit.
    pub fn event_sound(&self, emote_id: u32) -> Option<u32> {
        self.event_sound.get(&emote_id).copied().filter(|&k| k != 0)
    }

    /// The `AnimationData.dbc` id an `Emotes.dbc` id plays, for both `SMSG_EMOTE` one-shots and
    /// the looping `UNIT_NPC_EMOTESTATE` idle, which share the id space.
    pub fn anim(&self, emote_id: u32) -> Option<u32> {
        self.anim.get(&emote_id).copied().filter(|&a| a != 0)
    }

    /// The `Emotes.dbc` id in gesture `slot`, the `code` the client's gesture dispatcher
    /// `0x60bb30` takes; a slot no shipped row fills plays no gesture, as in the client.
    pub fn gesture(&self, slot: usize) -> Option<u32> {
        self.gesture.get(slot).copied().flatten()
    }

    /// An `Emotes.dbc` id's raw `EmoteFlags`, `0` included. Bit `0x0001` with a non-zero stand
    /// state aborts the whole emote before `CMSG_TEXT_EMOTE` is sent, so a seated `/bow` does
    /// nothing (`0x47db40`, called from `DoEmote` `0x5ef560`).
    pub fn emote_flags(&self, emote_id: u32) -> Option<u32> {
        self.emote_flags.get(&emote_id).copied()
    }

    /// An `Emotes.dbc` id's `EmoteSpecProc`, `0` included; `2` is a looping state emote.
    pub fn spec_proc(&self, emote_id: u32) -> Option<u32> {
        self.spec_proc.get(&emote_id).copied()
    }

    /// The stand state a posture emote sets, `EmoteSpecProcParam` when `EmoteSpecProc` is 1, as
    /// `DoEmote` (`0x5ef560`) feeds the sit key's setter `0x5ed430`. `/sit` rests on this alone:
    /// the server does nothing for a state text emote (`ChatHandler.cpp` `HandleTextEmoteOpcode`).
    pub fn posture_state(&self, emote_id: u32) -> Option<u32> {
        (self.spec_proc.get(&emote_id).copied() == Some(1))
            .then(|| self.spec_proc_param.get(&emote_id).copied())
            .flatten()
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

fn schema(name: &str, n: usize, strings: &[usize]) -> Schema {
    let mut s = Schema::new(name);
    for i in 0..n {
        let ty = if strings.contains(&i) {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    s
}

/// Read the three tables off the patch chain.
pub fn load_emote_sound_catalog(chain: &mut Chain) -> Result<EmoteSoundCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\EmotesText.dbc")
        .context("reading EmotesText.dbc")?;
    let rs = parse(&bytes, schema("EmotesText", 19, &[1]), "EmotesText")?;
    let mut by_name = HashMap::with_capacity(rs.records().len());
    let mut text_emote = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(name) = str_at(&rs, r, 1).filter(|n| !n.is_empty()) {
            by_name.insert(name.to_ascii_uppercase(), id);
        }
        text_emote.insert(id, u32_at(r, 2).unwrap_or(0));
    }

    let bytes = chain
        .read_file("DBFilesClient\\EmotesTextSound.dbc")
        .context("reading EmotesTextSound.dbc")?;
    let rs = parse(&bytes, schema("EmotesTextSound", 5, &[]), "EmotesTextSound")?;
    let mut voice = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(text), Some(race), Some(sex), Some(kit)) =
            (u32_at(r, 1), u32_at(r, 2), u32_at(r, 3), u32_at(r, 4))
        else {
            continue;
        };
        voice.insert((text, race, sex), kit);
    }

    let bytes = chain
        .read_file("DBFilesClient\\Emotes.dbc")
        .context("reading Emotes.dbc")?;
    let rs = parse(&bytes, schema("Emotes", 7, &[1]), "Emotes")?;
    let mut event_sound = HashMap::with_capacity(rs.records().len());
    let mut anim = HashMap::with_capacity(rs.records().len());
    let mut emote_flags = HashMap::with_capacity(rs.records().len());
    let mut spec_proc = HashMap::with_capacity(rs.records().len());
    let mut spec_proc_param = HashMap::with_capacity(rs.records().len());
    // The client walks the rows from the last down with an else-if chain over the bits, so a row
    // lands only in its first matching slot and a shared bit goes to the lowest index; iterating
    // forward with the earlier row winning gives the same table.
    let mut gesture: [Option<u32>; GESTURE_FLAG_BITS.len()] = [None; GESTURE_FLAG_BITS.len()];
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(flags) = u32_at(r, 3) {
            if let Some(slot) = GESTURE_FLAG_BITS.iter().position(|b| flags & b != 0) {
                gesture[slot].get_or_insert(id);
            }
        }
        if let Some(kit) = u32_at(r, 6) {
            event_sound.insert(id, kit);
        }
        if let Some(anim_id) = u32_at(r, 2) {
            anim.insert(id, anim_id);
        }
        if let Some(flags) = u32_at(r, 3) {
            emote_flags.insert(id, flags);
        }
        if let Some(proc) = u32_at(r, 4) {
            spec_proc.insert(id, proc);
        }
        if let Some(param) = u32_at(r, 5) {
            spec_proc_param.insert(id, param);
        }
    }

    Ok(EmoteSoundCatalog {
        by_name,
        text_emote,
        voice,
        event_sound,
        anim,
        emote_flags,
        spec_proc,
        spec_proc_param,
        gesture,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The expected ids and anims are the table `0x603a60` builds from the shipped data.
    #[test]
    fn the_five_gesture_slots_scan_to_the_shipped_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_emote_sound_catalog(&mut chain).expect("load emote catalog");
        // slot, expected Emotes.dbc id, expected AnimationData id, name
        for (slot, emote_id, anim_id, what) in [
            (0usize, 1u32, 60u32, "talk"),
            (1, 6, 65, "question"),
            (2, 5, 64, "exclamation"),
            (3, 22, 81, "shout"),
            (4, 11, 70, "laugh"),
        ] {
            assert_eq!(cat.gesture(slot), Some(emote_id), "{what} slot -> emote id");
            assert_eq!(cat.anim(emote_id), Some(anim_id), "{what} emote -> AnimID");
        }
        // One shipped row per bit, so the walk order cannot matter.
        for bit in GESTURE_FLAG_BITS {
            let carriers = cat
                .emote_flags
                .iter()
                .filter(|(_, f)| *f & bit != 0)
                .count();
            assert_eq!(carriers, 1, "exactly one shipped row carries {bit:#x}");
        }
    }

    #[test]
    fn real_emote_chain_resolves() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_emote_sound_catalog(&mut chain).expect("load emote catalog");
        assert_eq!(cat.text_id("wave"), Some(101), "case-insensitive by name");
        assert_eq!(cat.text_emote(101), Some(3), "WAVE plays anim emote 3");
        assert_eq!(cat.anim(2), Some(66), "ONESHOT_BOW (id 2) plays AnimID 66");
        // The posture-gate rows (`0x47db40`), ids from vmangos `SharedDefines.h`'s `Emote` enum.
        assert_eq!(cat.emote_flags(2), Some(0x4801), "ONESHOT_BOW EmoteFlags");
        assert_eq!(cat.emote_flags(4), Some(0x0800), "ONESHOT_CHEER EmoteFlags");
        assert_eq!(
            cat.emote_flags(11),
            Some(0x0980),
            "ONESHOT_LAUGH EmoteFlags"
        );
        assert_eq!(cat.emote_flags(14), Some(0x0001), "ONESHOT_RUDE EmoteFlags");
        assert_eq!(
            cat.emote_flags(21),
            Some(0x0000),
            "ONESHOT_APPLAUD EmoteFlags"
        );
        assert_eq!(
            cat.emote_flags(66),
            Some(0x0800),
            "ONESHOT_SALUTE EmoteFlags"
        );
        // STATE_WORK_NOSHEATHE_MINING (233), a proc-2 state emote carrying the mining kit.
        assert_eq!(cat.spec_proc(233), Some(2), "mining state EmoteSpecProc");
        assert_eq!(
            cat.event_sound(233),
            Some(3782),
            "mining state EventSoundID (SoundEntries \"Mining\")"
        );
        assert_eq!(cat.anim(233), Some(136), "mining state plays anim 136");
        assert_eq!(cat.spec_proc(2), Some(0), "ONESHOT_BOW EmoteSpecProc");
        // Stand states are vmangos `UnitStandStateType` values (STAND 0, SIT 1, SLEEP 3, KNEEL 8).
        assert_eq!(cat.posture_state(13), Some(1), "STATE_SIT sets SIT");
        assert_eq!(cat.posture_state(12), Some(3), "STATE_SLEEP sets SLEEP");
        assert_eq!(cat.posture_state(68), Some(8), "STATE_KNEEL sets KNEEL");
        assert_eq!(cat.posture_state(26), Some(0), "STATE_STAND sets STAND");
        // The gate is the proc column, not the param's presence.
        assert_eq!(cat.posture_state(233), None, "mining state is proc 2");
        assert_eq!(cat.posture_state(2), None, "ONESHOT_BOW is proc 0");
        assert!(
            cat.voice
                .keys()
                .any(|&(_, race, sex)| race == 1 && sex == 0),
            "human male voice rows exist"
        );
        assert!(
            cat.voice.keys().any(|&(_, _, sex)| sex == 1),
            "female voice rows exist"
        );
    }
}
