//! The character create's glue verbs (`CharacterCreate.cpp`, registrar `0x471a20`, table
//! `0x8380f8`, 24 entries) and the state they read: the race list (`[0xb42060]`, count
//! `[0xb4205c]`), the selected race's index into it (`[0xb42068]`), the class (`[0x8380f0]`, a
//! `ChrClasses` id, 1 in the image), the class list (`[0xb42084]`, count `[0xb42080]`), the preview
//! character (`[0xb42078]`: race `+0x18`, sex `+0x1c`, hair colour `+0x20`, skin `+0x24`, face
//! `+0x2c`, facial hair `+0x30`, hair style `+0x34`), the customize frame (`[0xb420f0]`), the
//! facing (`[0xb420f4]`, radians) and the name generator (`0xb420b8`).
//!
//! The glue build zeroes the frame, the facing and the race list and walks `ChrRaces` again
//! (`0x470430`, from `0x46a894`): here the state is the VM's own and starts there, and the host
//! seats the walk's inputs ([`GlueCreateCatalog`]). The class and the selected index the
//! reference keeps across builds are fresh per VM, which nothing sees: the screen's `OnShow` sets
//! both through `ResetCharCustomize` before it reads them. Before the first build the reference
//! reads the preview character through a null pointer; here its cells read 0.
//!
//! The draws are `(n · rand) >> 32` (`0x455c70`) of a 32-bit word, and a count of 0 or less
//! draws nothing (`0x4702d6`). The words are a clock-seeded generator's in place of the
//! reference's table-driven one (`0x4531e0` over `0x882664`, reseeded from the tick at startup);
//! only the range law and the order of the draws are the reference's.
//!
//! The 3-D side, the model build into the frame's scene (`0x470b70`), its dressing in the class's
//! `CharStartOutfit` (`0x470800`), the bake `UpdateCustomizationScene` runs each frame (`0x471300`)
//! and the facing's rotation (`0x470a50`), is the host's scene drive; what it reads is kept here
//! ([`super::UiScript::glue_create_look`], [`super::UiScript::glue_create_facing`]).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{number_arg, string_arg};
use super::glue_core::{call, glue_status_key, GlueCall};
use super::glue_select::{DEG_TO_RAD, RAD_TO_DEG};
use super::{check_char_name, Model, ScriptValue, CHAR_NAME_SUCCESS};

/// One race of the create screen's list (`0x470430`), its `ChrRaces` row as the verbs read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlueCreateRace {
    /// The row's id, which the list holds (`0x4705e9`).
    pub id: u8,
    /// `Name_Lang` (`+0x44`).
    pub name: String,
    /// The file string (`+0x3c`).
    pub file: String,
    /// The first `FactionGroup.dbc` row, in file order, with a nonzero bit index whose bit the
    /// race's template's group mask holds (`0x4714b0`-`0x4714cd`): its `Name_Lang` and
    /// `InternalName`. `None` without one, which `GetFactionForRace` answers nil, nil.
    pub faction: Option<(String, String)>,
    /// The three customization tokens, `+0x68`, `+0x6c` and `+0x70`: the facial hair's for sex 0
    /// and 1, then the hair's. `GetFacialHairCustomization` indexes them by sex
    /// (`[eax+ecx*4+0x68]`), `GetHairCustomization` reads the third.
    pub tokens: [String; 3],
}

/// One `ChrClasses` row as the create verbs read it: `Name_Lang` (`+0x14`) and the file string
/// (`+0x38`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlueCreateClass {
    pub name: String,
    pub file: String,
}

/// One `NameGen.dbc` row: race, sex, name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlueNameRow {
    pub race: u32,
    pub sex: u32,
    pub name: String,
}

/// What the create verbs read off the DBCs, which the host seats.
#[derive(Clone, Debug, Default)]
pub struct GlueCreateCatalog {
    /// The race list `0x470430` builds: `ChrRaces` in file order, rows flagged `0x1` skipped
    /// (`0x470483`), once for the races whose faction group's `InternalName` is `Alliance`
    /// (`0x47051f`) and then for `Horde` (`0x47057d`), compared case-folded.
    pub races: Vec<GlueCreateRace>,
    /// The class list `0x4706b0` builds for each race: `CharBaseInfo` in file order less the
    /// literal Dwarf-Mage pair.
    pub class_lists: HashMap<u8, Vec<u8>>,
    /// `ChrClasses` by id.
    pub classes: HashMap<u8, GlueCreateClass>,
    /// The customization counts by (race, sex), the grid's record `race·2 + sex` (`0x47a3e0` and
    /// its siblings).
    pub counts: HashMap<(u8, u8), GlueCreateCounts>,
    /// The (race, sex) pairs whose body has a model (`0x5ec480`'s record with a path): the build
    /// `0x470b70` goes ahead only for one (`0x470b80`-`0x470b8e`).
    pub models: HashSet<(u8, u8)>,
    /// `NameGen.dbc` in file order.
    pub names: Vec<GlueNameRow>,
}

/// One (race, sex)'s customization counts off the CharSections grid.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlueCreateCounts {
    /// `0x47a440`.
    pub skin: i32,
    /// `0x47a4a0`.
    pub face: i32,
    /// `[grid+0x18]`, `0x47a3e0`.
    pub hair_styles: i32,
    /// Each hair style's colour count, by style (`0x47a400`: `record[style].field0`).
    pub hair_colors: Vec<i32>,
    /// `0x47a4c0`.
    pub facial_hair: i32,
}

impl GlueCreateCatalog {
    fn race(&self, id: u32) -> Option<&GlueCreateRace> {
        self.races.iter().find(|r| u32::from(r.id) == id)
    }

    /// A customization's count for a race, sex and hair style. The grid's record is
    /// `race·2 + sex` (`0x47a3e0`), so a sex of 2 reads the next race's male record. A hair
    /// colour's is the style's own, 0 for a style past the record's (`0x47a41e`-`0x47a425`).
    fn count(&self, race: u32, sex: u32, which: usize, style: i32) -> i32 {
        let cell = race.wrapping_mul(2).wrapping_add(sex);
        let (Ok(race), Ok(sex)) = (u8::try_from(cell / 2), u8::try_from(cell % 2)) else {
            return 0;
        };
        let Some(c) = self.counts.get(&(race, sex)) else {
            return 0;
        };
        match which {
            SKIN => c.skin,
            FACE => c.face,
            HAIR_STYLE => c.hair_styles,
            HAIR_COLOR => usize::try_from(style)
                .ok()
                .and_then(|s| c.hair_colors.get(s))
                .copied()
                .unwrap_or(0),
            _ => c.facial_hair,
        }
    }
}

/// The preview character as the scene drive builds it: race, sex, class and the five
/// customizations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlueCreateLook {
    pub race: u8,
    pub sex: u8,
    pub class: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

/// What `CreateCharacter` hands the host past the name check (`0x471219`-`0x471264`): the name
/// and the nine bytes of `CMSG_CHAR_CREATE`'s body, the outfit byte 0 (`0x471260`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlueCreateRequest {
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

/// The customization indices, by customization id less one: skin, face, hair style, hair colour,
/// facial hair.
const SKIN: usize = 0;
const FACE: usize = 1;
const HAIR_STYLE: usize = 2;
const HAIR_COLOR: usize = 3;
const FACIAL_HAIR: usize = 4;

/// The build's input (`0x470900`'s local at `ebp-0x16c`, copied in at `0x476b90`): race, sex and
/// the five customizations, by customization id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Info {
    race: u32,
    sex: u32,
    dials: [i32; 5],
}

/// The words the draws take.
struct Words {
    state: u64,
    /// Words given ahead of the generator's, consumed first.
    scripted: std::collections::VecDeque<u32>,
}

impl Default for Words {
    fn default() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Words {
            state: nanos | 1,
            scripted: Default::default(),
        }
    }
}

impl Words {
    fn next(&mut self) -> u32 {
        if let Some(w) = self.scripted.pop_front() {
            return w;
        }
        let x = &mut self.state;
        *x ^= *x << 13;
        *x ^= *x >> 7;
        *x ^= *x << 17;
        (*x >> 32) as u32
    }

    /// `0x4702d0`: a draw in `[0, n)`, none for `n <= 0` (`0x4702db`).
    fn draw(&mut self, n: i32) -> i32 {
        if n <= 0 {
            return 0;
        }
        scale(self.next(), n as u32) as i32
    }
}

/// `0x455c70`: the high half of `r · n`.
fn scale(r: u32, n: u32) -> u32 {
    ((u64::from(r) * u64::from(n)) >> 32) as u32
}

/// The name generator (`0xb420b8`): a trigram table learned from one race's `NameGen` rows.
#[derive(Default)]
struct NameGen {
    /// Each key, one to three bytes of a lowered `_name`, and how often it was seen, sorted by key
    /// (`0x4753fb`, comparator `0x475420`).
    table: Vec<(Vec<u8>, u32)>,
}

/// `GetRandomName`'s buffer (`0x471990 mov edx,0xe`).
const NAME_BUFFER: usize = 14;

/// How many names [`GetRandomName`] tries before it answers nil: the reference tries until one
/// passes (`0x4719b3`-`0x4719bd`), forever with no table.
const NAME_TRIES: usize = 100_000;

impl NameGen {
    /// `0x475480(race, sex)`: the rows from the first with this race and sex up to the next race
    /// (the run is tested on the race alone, `0x4754da`), learned by `0x4752e0`. With no such row
    /// the table stands as it was: the names are dropped (`0x475490`-`0x475497`) and `0x4752e0`,
    /// which rebuilds it, is not reached (`0x4754f3`).
    fn set(&mut self, rows: &[GlueNameRow], race: u32, sex: u32) {
        let Some(first) = rows.iter().position(|r| r.race == race && r.sex == sex) else {
            return;
        };
        let run = rows[first..].iter().take_while(|r| r.race == race);
        let mut table: Vec<(Vec<u8>, u32)> = Vec::new();
        for row in run {
            // `"_%s"` into 0x40 bytes (`0x475345`), lowered (`0x64b480`).
            let mut buf = Vec::with_capacity(row.name.len() + 1);
            buf.push(b'_');
            buf.extend_from_slice(row.name.as_bytes());
            buf.truncate(0x3f);
            buf.make_ascii_lowercase();
            // Every three-byte window up to the last two bytes (`0x475361`-`0x4753d8`).
            for i in 0..buf.len().saturating_sub(1) {
                let key = buf[i..(i + 3).min(buf.len())].to_vec();
                match table.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, n)) => *n += 1,
                    None => table.push((key, 1)),
                }
            }
        }
        table.sort_by(|a, b| a.0.cmp(&b.0));
        self.table = table;
    }

    /// `0x475130(buf, size)`: from a key opening with `_`, extend by the last two bytes until a
    /// two-byte key ends the name or it fills the buffer, each key drawn by its count; then the
    /// `_` is dropped and the first byte raised (`0x4752be`, `0x4752ca`).
    fn generate(&self, words: &mut Words, size: usize) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let matches = |key: &[u8]| -> bool {
                if buf.is_empty() {
                    key.first() == Some(&b'_')
                } else {
                    // `strncmp(key, tail, 2)`: the tail is the buffer's last two bytes.
                    let tail = &buf[buf.len().saturating_sub(2)..];
                    key.get(..2).unwrap_or(key) == tail
                }
            };
            let Some(lo) = self.table.iter().position(|(k, _)| matches(k)) else {
                break;
            };
            let hi = lo
                + self.table[lo..]
                    .iter()
                    .take_while(|(k, _)| matches(k))
                    .count();
            let total: u32 = self.table[lo..hi].iter().map(|(_, n)| n).sum();
            let mut r = scale(words.next(), total);
            let mut pick = lo;
            for (i, (_, n)) in self.table.iter().enumerate().take(hi).skip(lo) {
                if r < *n {
                    pick = i;
                    break;
                }
                r -= n;
            }
            let key = &self.table[pick].0;
            if buf.is_empty() {
                buf = key[..key.len().min(size - 1)].to_vec();
                continue;
            }
            let Some(&next) = key.get(2) else {
                break;
            };
            buf.push(next);
            if buf.len() >= size - 1 {
                break;
            }
        }
        if !buf.is_empty() {
            buf.remove(0);
        }
        if let Some(first) = buf.first_mut() {
            *first = first.to_ascii_uppercase();
        }
        buf
    }
}

/// The create screen's state.
pub(crate) struct GlueCreate {
    catalog: Rc<GlueCreateCatalog>,
    /// `[0xb420f0]`: the name `SetCharCustomizeFrame` found.
    frame: Option<String>,
    /// `[0xb420f4]`, radians.
    facing: f32,
    /// `[0xb42068]`: an index into the race list, -1 for none (`0x470a08`).
    selected: i32,
    /// `[0x8380f0]`: a `ChrClasses` id.
    class: u32,
    /// `[0xb42084]`: the class ids the last build listed.
    class_list: Vec<u8>,
    /// `[0xb42078]`: the preview character, none until the first build.
    obj: Option<Info>,
    /// The character stands on the scene: a reset detaches it (`0x470922`) and a build attaches
    /// the new one (`0x470c79`).
    standing: bool,
    names: NameGen,
    words: Words,
}

impl Default for GlueCreate {
    fn default() -> Self {
        GlueCreate {
            catalog: Rc::default(),
            frame: None,
            facing: 0.0,
            selected: 0,
            class: 1,
            class_list: Vec::new(),
            obj: None,
            standing: false,
            names: NameGen::default(),
            words: Words::default(),
        }
    }
}

impl GlueCreate {
    fn obj(&self) -> Info {
        self.obj.unwrap_or_default()
    }

    /// `0x470ad0`: the race id at a list index, 0 off the list.
    fn race_at(&self, index: i32) -> u32 {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.catalog.races.get(i))
            .map_or(0, |r| u32::from(r.id))
    }

    /// `0x4703c0`: the five customizations drawn for `info`'s race and sex, in the order the
    /// reference draws them: skin, hair style, hair colour (counted for the style just drawn,
    /// `0x4703ed`), facial hair, face. A sex of 2 draws over the next race's male counts, as the
    /// grid's index does; its build then fails, so the draws are spent and nothing is kept.
    fn randomize(&mut self, info: &mut Info) {
        let (race, sex) = (info.race, info.sex);
        let count = |which: usize, style: i32| self.catalog.count(race, sex, which, style);
        let skin = count(SKIN, 0);
        info.dials[SKIN] = self.words.draw(skin);
        let styles = count(HAIR_STYLE, 0);
        info.dials[HAIR_STYLE] = self.words.draw(styles);
        let colors = count(HAIR_COLOR, info.dials[HAIR_STYLE]);
        info.dials[HAIR_COLOR] = self.words.draw(colors);
        let facial = count(FACIAL_HAIR, 0);
        info.dials[FACIAL_HAIR] = self.words.draw(facial);
        let faces = count(FACE, 0);
        info.dials[FACE] = self.words.draw(faces);
    }

    /// `0x470b70`: a body with a model replaces the character, which keeps the facing
    /// (`0x470c33`), and lists the race's classes (`0x470c47`); one without changes nothing.
    fn build(&mut self, info: Info) -> bool {
        let key = (u8::try_from(info.race), u8::try_from(info.sex));
        let (Ok(race), Ok(sex)) = key else {
            return false;
        };
        if !self.catalog.models.contains(&(race, sex)) {
            return false;
        }
        self.obj = Some(info);
        self.standing = true;
        self.class_list = self
            .catalog
            .class_lists
            .get(&race)
            .cloned()
            .unwrap_or_default();
        true
    }

    fn learn_names(&mut self, race: u32, sex: u32) {
        let catalog = Rc::clone(&self.catalog);
        self.names.set(&catalog.names, race, sex);
    }

    /// `ResetCharCustomize` (`0x470900`): nothing without a frame (`0x470913`); otherwise the
    /// character comes off the scene, a sex and a race are drawn (`0x4702a0`: the sex first, then
    /// an index into the list), the five customizations, the class goes to 1 (`0x4709ed`, the id
    /// and not an index), the build, the selected index found by the character's race or -1
    /// (`0x470a08`-`0x470a2c`), and the name generator learns the character's race and sex.
    fn reset(&mut self) {
        if self.frame.is_none() {
            return;
        }
        self.standing = false;
        let sex = self.words.draw(2) as u32;
        let count = i32::try_from(self.catalog.races.len()).unwrap_or(i32::MAX);
        let index = self.words.draw(count);
        let race = self.race_at(index);
        let mut info = Info {
            race,
            sex,
            dials: [0; 5],
        };
        self.randomize(&mut info);
        self.class = 1;
        self.build(info);
        let obj = self.obj;
        self.selected = obj
            .and_then(|o| {
                self.catalog
                    .races
                    .iter()
                    .position(|r| u32::from(r.id) == o.race)
            })
            .map_or(-1, |i| i as i32);
        let obj = self.obj();
        self.learn_names(obj.race, obj.sex);
    }

    /// `SetSelectedRace`'s body (`0x470cf0`), 0-based: an index off the list or the one already
    /// selected changes nothing (`0x470cff`, `0x470d0b`). Otherwise the index is stored, the race
    /// keeps the character's sex with five fresh customizations, the class is the first of the
    /// list the last build made (`0x470df4`, ahead of the build that lists the new race's), the
    /// build, and the name generator learns the race and the character's sex.
    fn select_race(&mut self, index: i32) {
        let in_list = u32::try_from(index)
            .ok()
            .is_some_and(|i| (i as usize) < self.catalog.races.len());
        if !in_list || index == self.selected {
            return;
        }
        self.selected = index;
        let mut info = Info {
            race: self.race_at(index),
            sex: self.obj().sex,
            dials: [0; 5],
        };
        self.randomize(&mut info);
        self.select_class(0);
        self.build(info);
        let (race, sex) = (self.race_at(self.selected), self.obj().sex);
        self.learn_names(race, sex);
    }

    /// `SetSelectedSex`'s body (`0x470e30`), 0-based: 3 or more (`0x470e3c`, unsigned) or the
    /// character's own changes nothing; otherwise the sex with the character's race and five
    /// fresh customizations is built, and the name generator learns the selected race and the
    /// sex asked for, built or not (`0x470f2f`).
    fn select_sex(&mut self, sex: i32) {
        let Ok(sex) = u32::try_from(sex) else {
            return;
        };
        if sex >= 3 || sex == self.obj().sex {
            return;
        }
        let mut info = Info {
            race: self.obj().race,
            sex,
            dials: [0; 5],
        };
        self.randomize(&mut info);
        self.build(info);
        if let Some(race) = usize::try_from(self.selected)
            .ok()
            .and_then(|i| self.catalog.races.get(i))
            .map(|r| u32::from(r.id))
        {
            self.learn_names(race, sex);
        }
    }

    /// `SetSelectedClass`'s body (`0x470f50`), 0-based: an index on the class list takes its id
    /// (`0x470f60`); the re-dress on a change (`0x470f6f`) is the scene drive's.
    fn select_class(&mut self, index: i32) {
        let Some(&id) = u32::try_from(index)
            .ok()
            .and_then(|i| self.class_list.get(i as usize))
        else {
            return;
        };
        // The list holds a null record for an id with no `ChrClasses` row (`0x470781`), which
        // the reference dereferences (`0x470f60`).
        if self.catalog.classes.contains_key(&id) {
            self.class = u32::from(id);
        }
    }

    /// `CycleCharCustomization`'s body (`0x470f80`), 0-based: a delta of 0 or an index past 4
    /// (unsigned) changes nothing; otherwise the customization moves by the delta, a sum below 0
    /// reads `count - 1` and one at or past the count reads 0 (`0x470fb9`-`0x470fc2`), so a step
    /// wraps and a long one lands on an end. The setter refuses a value with no `CharSections` row
    /// (`0x477c20`), which with no count is every value.
    fn cycle(&mut self, which: i32, delta: i32) {
        let Ok(which) = usize::try_from(which) else {
            return;
        };
        if delta == 0 || which > 4 {
            return;
        }
        let Some(obj) = self.obj.as_mut() else {
            return;
        };
        // A hair colour counts the character's current style's (`0x471019`).
        let count = self
            .catalog
            .count(obj.race, obj.sex, which, obj.dials[HAIR_STYLE]);
        if count <= 0 {
            return;
        }
        let mut v = obj.dials[which].wrapping_add(delta);
        if v < 0 {
            v = count - 1;
        }
        if v >= count {
            v = 0;
        }
        obj.dials[which] = v;
    }

    /// `RandomizeCharCustomization`'s body (`0x471080`): five fresh customizations for the
    /// character's race and sex, set in place with no build (`0x471162`-`0x4711ae`).
    fn randomize_in_place(&mut self) {
        let Some(obj) = self.obj else {
            return;
        };
        let mut info = obj;
        self.randomize(&mut info);
        self.obj = Some(Info {
            dials: info.dials,
            ..obj
        });
    }

    /// `HasCharCustomization`'s count (`0x471270`): the customization's for the character, 0 for
    /// an index past 4 (`0x471273`).
    fn customization_count(&self, which: i32) -> i32 {
        match usize::try_from(which) {
            Ok(w) if w <= 4 => {
                let obj = self.obj();
                self.catalog
                    .count(obj.race, obj.sex, w, obj.dials[HAIR_STYLE])
            }
            _ => 0,
        }
    }

    fn look(&self) -> Option<GlueCreateLook> {
        let obj = self.obj.filter(|_| self.standing)?;
        let byte = |v: i32| v as u8;
        Some(GlueCreateLook {
            race: obj.race as u8,
            sex: obj.sex as u8,
            class: self.class as u8,
            skin: byte(obj.dials[SKIN]),
            face: byte(obj.dials[FACE]),
            hair_style: byte(obj.dials[HAIR_STYLE]),
            hair_color: byte(obj.dials[HAIR_COLOR]),
            facial_hair: byte(obj.dials[FACIAL_HAIR]),
        })
    }
}

impl super::UiScript {
    /// Seat what the create verbs read off the DBCs (the glue build's walk, `0x470430`).
    pub fn set_glue_create_catalog(&mut self, catalog: Rc<GlueCreateCatalog>) {
        self.model_mut().glue.create.catalog = catalog;
    }

    /// The character the create scene stands, while it stands.
    pub fn glue_create_look(&self) -> Option<GlueCreateLook> {
        self.model_ref().glue.create.look()
    }

    /// The create facing (`[0xb420f4]`), radians.
    pub fn glue_create_facing(&self) -> f32 {
        self.model_ref().glue.create.facing
    }

    /// Words the create draws take before the generator's, in order: a fixed sequence for a run
    /// that must be repeatable.
    pub fn script_glue_create_draws(&mut self, words: &[u32]) {
        self.model_mut()
            .glue
            .create
            .words
            .scripted
            .extend(words.iter().copied());
    }
}

/// `[0xb420f0]`'s frame, while the name `SetCharCustomizeFrame` took still names a model pane.
pub(super) fn create_frame_handle(model: &Model) -> Option<crate::widget::FrameHandle> {
    let h = model.arena.lookup(model.glue.create.frame.as_deref()?)?;
    matches!(
        model.arena.frame(h).map(|f| &f.kind_state),
        Some(crate::widget::KindState::Model(_))
    )
    .then_some(h)
}

fn model(lua: &Lua) -> mlua::AppDataRefMut<'_, Model> {
    lua.app_data_mut::<Model>().expect("model app_data")
}

fn arg(args: &MultiValue, i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

/// A string, or nil for a null pointer (`0x6f3890` with 0 pushes nil, `0x6f3897`).
fn text(lua: &Lua, s: Option<&str>) -> mlua::Result<Value> {
    match s {
        Some(s) => lua.create_string(s).map(Value::String),
        None => Ok(Value::Nil),
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x471330`: a string or number (`0x6f3510`) or the usage error (`0x838428`); the global of
    // that name castable to the model type (`0x76c760` with `[0xcf0c5c]`, the select's type
    // cell) becomes the frame (`0x4707a0`), else the call changes nothing.
    g.set(
        "SetCharCustomizeFrame",
        lua.create_function(|lua, args: MultiValue| {
            let name = string_arg(
                lua,
                arg(&args, 0),
                "Usage: SetCharCustomizeFrame(\"frameName\")",
            )?;
            let mut m = model(lua);
            let is_model = m
                .arena
                .lookup(&name)
                .and_then(|h| m.arena.frame(h))
                .is_some_and(|f| f.kind == crate::widget::FrameKind::Model);
            if is_model {
                m.glue.create.frame = Some(name);
            }
            Ok(())
        })?,
    )?;

    // `0x4713a0`: the file (`0x6f3510`, else `0x838454`) into `0x4707b0`, which needs the frame
    // and a non-empty name and hands it to the frame's own `SetModel` (vtable `+0x94`,
    // `0x4707c8`). Unlike the select's, the scene keeps the fog fill callback `SetModel`
    // installs. A built character is attached to the new scene (`0x4707dd`-`0x4707ed`), a reset's
    // detached one included.
    g.set(
        "SetCharCustomizeBackground",
        lua.create_function(|lua, args: MultiValue| {
            let file = string_arg(
                lua,
                arg(&args, 0),
                "Usage: SetCharCustomizeBackground(\"filename\")",
            )?;
            let mut m = model(lua);
            let Some(h) = create_frame_handle(&m).filter(|_| !file.is_empty()) else {
                return Ok(());
            };
            m.set_pane_model(h, file);
            let create = &mut m.glue.create;
            create.standing |= create.obj.is_some();
            Ok(())
        })?,
    )?;

    // `0x4713e0` → `0x470900`; no values, arguments ignored.
    g.set(
        "ResetCharCustomize",
        lua.create_function(|lua, _: MultiValue| {
            model(lua).glue.create.reset();
            Ok(())
        })?,
    )?;

    // `0x4713f0`: the character's race row (`+0x18`), its `Name_Lang` and file string; nil, nil
    // without a row. Always two values.
    g.set(
        "GetNameForRace",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let row = c.catalog.race(c.obj().race);
            Ok(MultiValue::from_vec(vec![
                text(lua, row.map(|r| r.name.as_str()))?,
                text(lua, row.map(|r| r.file.as_str()))?,
            ]))
        })?,
    )?;

    // `0x471450`: the character's race's faction group, `Name_Lang` then `InternalName`; nil,
    // nil without one. Always two values.
    g.set(
        "GetFactionForRace",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let faction = c
                .catalog
                .race(c.obj().race)
                .and_then(|r| r.faction.as_ref());
            Ok(MultiValue::from_vec(vec![
                text(lua, faction.map(|f| f.0.as_str()))?,
                text(lua, faction.map(|f| f.1.as_str()))?,
            ]))
        })?,
    )?;

    // `0x471520`: name and file string for each race of the list (`0x470b20`, `0x470af0`), two
    // values a race and none for an empty list (`0x471558`).
    g.set(
        "GetAvailableRaces",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let mut out = Vec::new();
            for r in &m.glue.create.catalog.races {
                out.push(text(lua, Some(&r.name))?);
                out.push(text(lua, Some(&r.file))?);
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // `0x471560`: name (`+0x14`) and file string (`+0x38`) for each class of the list, nil, nil
    // for a listed id with no row (`0x4715a0`); no arguments read.
    g.set(
        "GetClassesForRace",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let mut out = Vec::new();
            for id in &c.class_list {
                let row = c.catalog.classes.get(id);
                out.push(text(lua, row.map(|r| r.name.as_str()))?);
                out.push(text(lua, row.map(|r| r.file.as_str()))?);
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // `0x4715d0`: the selected list race's hair token (`+0x70`), `"NORMAL"` (`0x838484`) with no
    // row; the race is the selected index's (`0x470ad0`), not the character's.
    g.set(
        "GetHairCustomization",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let row = c.catalog.race(c.race_at(c.selected));
            text(lua, Some(row.map_or("NORMAL", |r| r.tokens[2].as_str())))
        })?,
    )?;

    // `0x471620`: the selected list race's facial hair token for the character's sex
    // (`+0x68 + sex·4`), `"NORMAL"` with no row.
    g.set(
        "GetFacialHairCustomization",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let row = c.catalog.race(c.race_at(c.selected));
            let sex = c.obj().sex as usize;
            let token = row.and_then(|r| r.tokens.get(sex)).map(String::as_str);
            text(lua, Some(token.unwrap_or("NORMAL")))
        })?,
    )?;

    // `0x471680`: the selected index plus one, a number (0 with none).
    g.set(
        "GetSelectedRace",
        lua.create_function(|lua, _: MultiValue| {
            Ok(f64::from(model(lua).glue.create.selected) + 1.0)
        })?,
    )?;

    // `0x4716b0`: the character's sex plus one, a number: 1 male, 2 female.
    g.set(
        "GetSelectedSex",
        lua.create_function(|lua, _: MultiValue| {
            Ok(f64::from(model(lua).glue.create.obj().sex) + 1.0)
        })?,
    )?;

    // `0x4716e0`: the class's name and file string, nil, nil without a row. Always two values.
    g.set(
        "GetSelectedClass",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let row = u8::try_from(c.class)
                .ok()
                .and_then(|id| c.catalog.classes.get(&id));
            Ok(MultiValue::from_vec(vec![
                text(lua, row.map(|r| r.name.as_str()))?,
                text(lua, row.map(|r| r.file.as_str()))?,
            ]))
        })?,
    )?;

    // `0x471740`: one number or `Usage: SetSelectedRace(index)` (`0x83848c`), less one, into
    // `0x470cf0`; no values.
    g.set(
        "SetSelectedRace",
        lua.create_function(|lua, args: MultiValue| {
            let index = number_arg(lua, arg(&args, 0), "Usage: SetSelectedRace(index)")?;
            model(lua).glue.create.select_race(index.wrapping_sub(1));
            Ok(())
        })?,
    )?;

    // `0x471780`: one number or `Usage: SetSelectedSex(index)` (`0x8384ac`), less one, into
    // `0x470e30`; no values.
    g.set(
        "SetSelectedSex",
        lua.create_function(|lua, args: MultiValue| {
            let sex = number_arg(lua, arg(&args, 0), "Usage: SetSelectedSex(index)")?;
            model(lua).glue.create.select_sex(sex.wrapping_sub(1));
            Ok(())
        })?,
    )?;

    // `0x4717c0`: one number or `Usage: SetSelectedClass(index)` (`0x8384cc`), less one, into
    // `0x470f50`; no values.
    g.set(
        "SetSelectedClass",
        lua.create_function(|lua, args: MultiValue| {
            let index = number_arg(lua, arg(&args, 0), "Usage: SetSelectedClass(index)")?;
            model(lua).glue.create.select_class(index.wrapping_sub(1));
            Ok(())
        })?,
    )?;

    // `0x471800`: `0x470cf0` with the selected index itself, which its own gate turns away
    // (`0x470d0b`, or `0x470cff` for -1): a call that changes nothing. No values.
    g.set(
        "UpdateCustomizationBackground",
        lua.create_function(|lua, _: MultiValue| {
            let mut m = model(lua);
            let selected = m.glue.create.selected;
            m.glue.create.select_race(selected);
            Ok(())
        })?,
    )?;

    // `0x471810` → `0x471300`: the character bake (`0x477860`) and the scene's advance
    // (`0x710450`), once per `OnUpdateModel` (`CharacterCreate.lua:325`): the scene drive's.
    g.set(
        "UpdateCustomizationScene",
        lua.create_function(|_, _: MultiValue| Ok(()))?,
    )?;

    // `0x471820`: one number or `Usage: HasCharCustomization(index)` (`0x8384ec`); 1 when the
    // customization has more than one choice (`0x47185c`, unsigned), else nil. One value.
    g.set(
        "HasCharCustomization",
        lua.create_function(|lua, args: MultiValue| {
            let which = number_arg(lua, arg(&args, 0), "Usage: HasCharCustomization(index)")?;
            let count = model(lua)
                .glue
                .create
                .customization_count(which.wrapping_sub(1));
            Ok(if count as u32 > 1 {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // `0x471890`: two numbers or `Usage: CycleCharCustomization(index, delta)` (`0x838510`), the
    // index less one, into `0x470f80`; no values.
    g.set(
        "CycleCharCustomization",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: CycleCharCustomization(index, delta)";
            let which = number_arg(lua, arg(&args, 0), USAGE)?;
            let delta = number_arg(lua, arg(&args, 1), USAGE)?;
            model(lua).glue.create.cycle(which.wrapping_sub(1), delta);
            Ok(())
        })?,
    )?;

    // `0x471900` → `0x471080`; no values.
    g.set(
        "RandomizeCharCustomization",
        lua.create_function(|lua, _: MultiValue| {
            model(lua).glue.create.randomize_in_place();
            Ok(())
        })?,
    )?;

    // `0x471910`: the cell (a float) times 57.29578, pushed as a double.
    g.set(
        "GetCharacterCreateFacing",
        lua.create_function(|lua, _: MultiValue| {
            let radians = model(lua).glue.create.facing;
            Ok(f64::from(radians) * f64::from(RAD_TO_DEG))
        })?,
    )?;

    // `0x471930`: one number or `Usage: SetCharacterCreateFacing(degrees)` (`0x83853c`), times
    // 0.017453292 and stored as a float with no gate (`0x470a59`), wrap or clamp. It returns one
    // value (`0x47196e`) with nothing pushed since the arguments: the last argument.
    g.set(
        "SetCharacterCreateFacing",
        lua.create_function(|lua, args: MultiValue| {
            let degrees = match lua.coerce_number(arg(&args, 0))? {
                Some(n) => n,
                None => {
                    return Err(mlua::Error::RuntimeError(
                        "Usage: SetCharacterCreateFacing(degrees)".into(),
                    ))
                }
            };
            model(lua).glue.create.facing = (degrees * f64::from(DEG_TO_RAD)) as f32;
            Ok(args.back().cloned().unwrap_or(Value::Nil))
        })?,
    )?;

    // `0x471980`: names from the generator (`0x4711c0`, a 14-byte buffer) until one passes the
    // name check (`0x4719ab`) and runs 4 to 10 bytes (`0x4719b5`-`0x4719bd`); one value.
    // Deviation: past [`NAME_TRIES`] names the answer is nil, where the reference keeps trying,
    // forever when the generator has learned nothing.
    g.set(
        "GetRandomName",
        lua.create_function(|lua, _: MultiValue| {
            let lists = model(lua).glue.name_lists();
            let mut m = model(lua);
            let charsets = m.name_charsets();
            let c = &mut m.glue.create;
            for _ in 0..NAME_TRIES {
                let name = c.names.generate(&mut c.words, NAME_BUFFER);
                let name = String::from_utf8_lossy(&name).into_owned();
                if (4..=10).contains(&name.len())
                    && check_char_name(Some(&name), &*lists, charsets) == CHAR_NAME_SUCCESS
                {
                    return text(lua, Some(&name));
                }
            }
            Ok(Value::Nil)
        })?,
    )?;

    // `0x4719e0`: the name as `lua_tostring` reads it (`0x6f3690`): a string or a number, else a
    // null, which is `CHAR_NAME_NO_NAME` without the check (`0x4711db`). No usage error and no
    // values. `0x4711d0` checks the name (`0x4711e2`); a failure opens `OKAY` with the status's
    // text (`0x5abd20`, `0x703bf0`, event 3 at `0x47120c`) and sends nothing. A passing name with
    // the character's bytes is the host's (`0x46b290`: state 5, the dialog and the
    // `CMSG_CHAR_CREATE`).
    g.set(
        "CreateCharacter",
        lua.create_function(|lua, args: MultiValue| {
            let name = match arg(&args, 0) {
                v @ (Value::String(_) | Value::Integer(_) | Value::Number(_)) => {
                    lua.coerce_string(v)?.map(|s| s.to_string_lossy())
                }
                _ => None,
            };
            // A C string ends at its NUL.
            let name = name.map(|n| n.split('\0').next().unwrap_or_default().to_string());
            let (status, request) = {
                let m = model(lua);
                let status = match &name {
                    Some(n) => check_char_name(Some(n), &*m.glue.name_lists(), m.name_charsets()),
                    None => 0x45,
                };
                let c = &m.glue.create;
                let request = c.obj.map(|obj| GlueCreateRequest {
                    name: name.clone().unwrap_or_default(),
                    race: obj.race as u8,
                    class: c.class as u8,
                    sex: obj.sex as u8,
                    skin: obj.dials[SKIN] as u8,
                    face: obj.dials[FACE] as u8,
                    hair_style: obj.dials[HAIR_STYLE] as u8,
                    hair_color: obj.dials[HAIR_COLOR] as u8,
                    facial_hair: obj.dials[FACIAL_HAIR] as u8,
                });
                (status, request)
            };
            if status != CHAR_NAME_SUCCESS {
                let key = glue_status_key(i32::from(status));
                let text = super::glue_core::global_text(lua, key);
                super::tick::fire_event_into(
                    lua,
                    "OPEN_STATUS_DIALOG",
                    vec![ScriptValue::Str("OKAY".into()), ScriptValue::Str(text)],
                );
                return Ok(());
            }
            // The reference builds the body off the character with no test for one; with none
            // built yet nothing is sent here.
            if let Some(request) = request {
                call(lua, GlueCall::CreateCharacter(request));
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "glue_create_tests.rs"]
mod tests;
