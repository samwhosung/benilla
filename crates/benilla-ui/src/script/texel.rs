//! A Texture region's texture slot (`+0xcc`) and the creation stamp its texture carries, which
//! stands in for the device-texture address a layer batch sorts by (`script::layer_batch`).
//!
//! The reference shares one texture object among the regions that show one texture: a file through
//! the cache by name (`0x449d90`), a solid fill through the cache by packed colour (`0x44aa20`), a
//! `SetPortraitToTexture` image through the portrait cache by name (`0xc0ce30`). Its device
//! texture is made when the object is, so its address follows allocation time: a solid when it is
//! set (`0x44a940`), a generated image when it is created (`0x449c52`), a file when its
//! asynchronous read completes (`0x44a500` → `0x44a560`, at `0x44a71c`) or a draw forces it first
//! (`0x44acf0` → `0x44ad50`). The object lives while a region holds it. So each texture here takes
//! a stamp from one counter: a solid or generated one when set, a file at the next extract (the
//! first draw after the set, by when the synchronous work of the pass that set it is done), and a
//! texture its last region lets go of takes a new stamp when it comes back.

use std::collections::HashMap;

use crate::color::Rgba8;

/// A region's texture slot: the file or solid fill it shows, and its texture's place in [`Texels`].
/// Only [`Texels::set`] and [`Texels::release`] write it, so every holder is counted. A clone (the
/// extract's working copy) carries the id uncounted, which is sound while the original holds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TexSlot {
    path: Option<String>,
    fill: Option<Rgba8>,
    id: Option<TexId>,
    /// The wrap flag the file loaded with (`0x770200`'s second argument, bits 3 and 4 of the
    /// texture's flags word): the image repeats on both axes, else clamps. Fixed at load, never
    /// by a later `SetTexCoord`.
    wrap: bool,
}

impl TexSlot {
    /// The texture path (`SetTexture("Interface\\...")`), or an engine image's.
    pub(crate) fn path(&self) -> &Option<String> {
        &self.path
    }

    /// The solid fill (`SetTexture(r, g, b, a)`, `<Color>` with no `file=`).
    pub(crate) fn fill(&self) -> Option<Rgba8> {
        self.fill
    }

    /// Whether the file loaded with the wrap flag, so it repeats past [0, 1] on both axes.
    pub(crate) fn wrap(&self) -> bool {
        self.wrap
    }

    /// The texture's entry, `None` for an empty slot.
    pub(crate) fn id(&self) -> Option<TexId> {
        self.id
    }

    /// The path, out of a working copy.
    pub(crate) fn into_path(self) -> Option<String> {
        self.path
    }
}

/// An entry of [`Texels`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TexId(u32);

/// What a slot is set to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TexSet {
    /// Empty: no file, no fill.
    None,
    /// A file loaded with the wrap flag off, so it clamps: every `0x770200` caller but the XML
    /// `<Texture>` loader and the backdrop's tiled background and sides. Shared by name and flag;
    /// one the install lacks is the cache's solid green (`0x449e7c` → `0x44a9c0` with
    /// `0xff00ff00` at `0x90582c`).
    File(String),
    /// [`Self::File`] loaded with the wrap flag on, so it repeats: an XML `<Texture>` whose
    /// `<TexCoords>` leave [0, 1] (`0x77012d`), a backdrop's sides and its tiled background
    /// (`0x77ea9e`..`0x77ec9c`, `0x77e9f1`).
    WrapFile(String),
    /// A solid fill, shared by its packed colour.
    Fill(Rgba8),
    /// A `SetPortraitToTexture` image, generated and shared by its exact name (the portrait cache
    /// compares case-sensitively, `0x64a480`).
    Portrait(String),
    /// An image made for this region alone: a live unit portrait (no path) or a tabard cell.
    Own(Option<String>),
}

/// How the reference's caches key a texture.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    /// The name with a four-byte `.xxx` extension dropped (`0x449590`), upper-cased: the lookup
    /// compares with `_strnicmp` (`0x449eb0` → `0x64a4c0`); and the wrap flag, part of the flags
    /// the lookup matches (`0x449eb0`, `+0x18`).
    File(Box<str>, bool),
    /// The packed colour, a byte a channel.
    Fill(u32),
    Portrait(Box<str>),
    Own,
}

#[derive(Clone, Debug)]
struct Entry {
    key: Key,
    /// The creation stamp, from 1; 0 while a file waits for its first draw.
    stamp: u64,
    /// The slots holding it; 0 is a free entry.
    refs: u32,
}

/// Every live texture, its stamp and its holders.
#[derive(Clone, Debug, Default)]
pub(crate) struct Texels {
    entries: Vec<Entry>,
    free: Vec<u32>,
    shared: HashMap<Key, u32>,
    /// Files waiting for their stamp, in the order set.
    pending: Vec<u32>,
    /// The last stamp handed out.
    clock: u64,
}

/// The solid the cache falls back to for a file the install lacks.
const MISSING_FILL: Rgba8 = Rgba8::rgba(0, 255, 0, 255);

impl Texels {
    /// Point `slot` at `set`'s texture: the same texture keeps its entry and stamp, another one is
    /// found or made, and the old one loses a holder. `missing` answers whether the install lacks a
    /// file (the host's texture probe).
    pub(crate) fn set(&mut self, slot: &mut TexSlot, set: TexSet, missing: impl Fn(&str) -> bool) {
        let wrap = matches!(set, TexSet::WrapFile(_));
        if let TexSet::File(p) | TexSet::WrapFile(p) = &set {
            // The file the texture already shows answers 1 before any load (`0x770225`), so its
            // wrap flag stays as it loaded, whatever the new call passes.
            if self.holds_file(slot, p) {
                return;
            }
        }
        let (path, fill, key) = match set {
            TexSet::None => (None, None, None),
            TexSet::File(p) | TexSet::WrapFile(p) => {
                let key = if missing(&p) {
                    Key::Fill(pack(MISSING_FILL))
                } else {
                    Key::File(cache_name(&p), wrap)
                };
                (Some(p), None, Some(key))
            }
            TexSet::Fill(c) => (None, Some(c), Some(Key::Fill(pack(c)))),
            TexSet::Portrait(p) => {
                let key = Key::Portrait(p.as_str().into());
                (Some(p), None, Some(key))
            }
            TexSet::Own(p) => (p, None, Some(Key::Own)),
        };
        slot.path = path;
        slot.fill = fill;
        slot.wrap = matches!(key, Some(Key::File(_, true)));
        let same = match (slot.id, &key) {
            (Some(id), Some(k)) => *k != Key::Own && self.entries[id.0 as usize].key == *k,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        let new = key.map(|k| self.acquire(k));
        if let Some(old) = slot.id.take() {
            self.drop_ref(old);
        }
        slot.id = new;
    }

    /// Whether `slot` shows the file `path` names, by the cache's name and not its flags: the
    /// test `0x770200` makes first (`0x44ac70`, over the held texture's name).
    pub(crate) fn holds_file(&self, slot: &TexSlot, path: &str) -> bool {
        slot.id.is_some_and(|id| {
            matches!(&self.entries[id.0 as usize].key, Key::File(n, _) if **n == *cache_name(path))
        })
    }

    /// `slot`'s region is gone: its texture loses the holder.
    pub(crate) fn release(&mut self, slot: TexSlot) {
        if let Some(id) = slot.id {
            self.drop_ref(id);
        }
    }

    /// Stamp the files set since the last extract, in the order set: their first draw forces them.
    pub(crate) fn flush(&mut self) {
        for i in std::mem::take(&mut self.pending) {
            let e = &mut self.entries[i as usize];
            if e.refs > 0 && e.stamp == 0 {
                self.clock += 1;
                e.stamp = self.clock;
            }
        }
    }

    /// A texture's creation stamp; a file not yet flushed ranks after every stamped one.
    pub(crate) fn stamp(&self, id: TexId) -> u64 {
        match self.entries[id.0 as usize].stamp {
            0 => u64::MAX,
            s => s,
        }
    }

    fn acquire(&mut self, key: Key) -> TexId {
        if key != Key::Own {
            if let Some(&i) = self.shared.get(&key) {
                self.entries[i as usize].refs += 1;
                return TexId(i);
            }
        }
        let file = matches!(key, Key::File(..));
        let stamp = if file {
            0
        } else {
            self.clock += 1;
            self.clock
        };
        let entry = Entry {
            key: key.clone(),
            stamp,
            refs: 1,
        };
        let i = match self.free.pop() {
            Some(i) => {
                self.entries[i as usize] = entry;
                i
            }
            None => {
                self.entries.push(entry);
                u32::try_from(self.entries.len() - 1).expect("fewer than 2^32 textures")
            }
        };
        if file {
            self.pending.push(i);
        }
        if key != Key::Own {
            self.shared.insert(key, i);
        }
        TexId(i)
    }

    fn drop_ref(&mut self, id: TexId) {
        let e = &mut self.entries[id.0 as usize];
        e.refs -= 1;
        if e.refs == 0 {
            e.stamp = 0;
            let key = std::mem::replace(&mut e.key, Key::Own);
            if key != Key::Own {
                self.shared.remove(&key);
            }
            self.free.push(id.0);
        }
    }
}

/// A file name as the texture cache keys it.
fn cache_name(path: &str) -> Box<str> {
    let b = path.as_bytes();
    let stem = match b.len().checked_sub(4) {
        Some(dot) if b[dot] == b'.' => &path[..dot],
        _ => path,
    };
    stem.to_ascii_uppercase().into_boxed_str()
}

/// A colour as the solid cache keys it, its packed dword (`0x44aa20`).
fn pack(c: Rgba8) -> u32 {
    u32::from_be_bytes([c.a, c.r, c.g, c.b])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    fn set(t: &mut Texels, slot: &mut TexSlot, s: TexSet) -> u64 {
        t.set(slot, s, none);
        t.flush();
        slot.id().map_or(0, |id| t.stamp(id))
    }

    /// One texture, one stamp: a file across case and its extension, a fill by its bytes.
    #[test]
    fn regions_showing_one_texture_share_its_stamp() {
        let mut t = Texels::default();
        let (mut a, mut b, mut c, mut d) = Default::default();
        let x = set(
            &mut t,
            &mut a,
            TexSet::File("Interface\\Glues\\Border1".into()),
        );
        let y = set(
            &mut t,
            &mut b,
            TexSet::File("interface\\glues\\BORDER1.blp".into()),
        );
        assert_eq!(x, y);
        let f = set(
            &mut t,
            &mut c,
            TexSet::Fill(Rgba8::from_f32([0.0, 0.0, 0.0, 1.0])),
        );
        let g = set(
            &mut t,
            &mut d,
            TexSet::Fill(Rgba8::from_f32([0.0, 0.0, 0.001, 1.0])),
        );
        assert_eq!(f, g, "one byte apart is one colour");
        assert_ne!(x, f);
    }

    /// A solid is stamped when set; a file waits for the next flush, so a solid set after a file in
    /// one pass ranks before it, as its device texture is made first.
    #[test]
    fn a_file_is_stamped_after_the_solids_of_its_pass() {
        let mut t = Texels::default();
        let (mut file, mut solid) = (TexSlot::default(), TexSlot::default());
        t.set(&mut file, TexSet::File("Border1".into()), none);
        t.set(&mut solid, TexSet::Fill(Rgba8::default()), none);
        assert_eq!(t.stamp(file.id().unwrap()), u64::MAX, "pending");
        t.flush();
        assert!(t.stamp(solid.id().unwrap()) < t.stamp(file.id().unwrap()));
    }

    /// A texture its last region lets go of is made anew when it comes back, so it ranks newest.
    #[test]
    fn a_released_texture_comes_back_with_a_new_stamp() {
        let mut t = Texels::default();
        let (mut a, mut b, mut c) = Default::default();
        let first = set(&mut t, &mut a, TexSet::File("A".into()));
        set(&mut t, &mut b, TexSet::File("B".into()));
        set(&mut t, &mut c, TexSet::File("A".into()));
        set(&mut t, &mut a, TexSet::None);
        assert_eq!(t.stamp(c.id().unwrap()), first, "still held by c");
        set(&mut t, &mut c, TexSet::None);
        let again = set(&mut t, &mut a, TexSet::File("A".into()));
        assert!(again > t.stamp(b.id().unwrap()), "{again}");
    }

    /// A file the install lacks is the cache's solid green.
    #[test]
    fn a_missing_file_ranks_as_the_green_solid() {
        let mut t = Texels::default();
        let (mut green, mut lost) = (TexSlot::default(), TexSlot::default());
        t.set(&mut green, TexSet::Fill(Rgba8::rgba(0, 255, 0, 255)), none);
        t.set(&mut lost, TexSet::File("Nowhere".into()), |_| true);
        assert_eq!(green.id(), lost.id());
        assert_eq!(lost.path().as_deref(), Some("Nowhere"));
    }

    /// The cache matches the wrap flag with the name (`0x449eb0`): one file loaded both ways is
    /// two textures, and a call for the file a slot already shows leaves its flag alone.
    #[test]
    fn the_wrap_flag_is_part_of_a_texture_and_survives_a_same_file_call() {
        let mut t = Texels::default();
        let (mut clamped, mut wrapped) = (TexSlot::default(), TexSlot::default());
        let a = set(&mut t, &mut clamped, TexSet::File("Strip".into()));
        let b = set(&mut t, &mut wrapped, TexSet::WrapFile("strip.blp".into()));
        assert_ne!(clamped.id(), wrapped.id());
        assert_ne!(a, b);
        assert!(!clamped.wrap() && wrapped.wrap());
        assert!(t.holds_file(&wrapped, "STRIP"));
        assert!(!t.holds_file(&wrapped, "Other"));

        // A clamped call for the shown file is a no-op, in either direction.
        let id = wrapped.id();
        set(&mut t, &mut wrapped, TexSet::File("Strip".into()));
        assert!(wrapped.wrap() && wrapped.id() == id);
        set(&mut t, &mut clamped, TexSet::WrapFile("Strip".into()));
        assert!(!clamped.wrap());

        // Another file takes the call's flag; clearing drops it with the image.
        set(&mut t, &mut wrapped, TexSet::File("Other".into()));
        assert!(!wrapped.wrap());
        set(&mut t, &mut clamped, TexSet::None);
        assert!(!t.holds_file(&clamped, "Strip"));
    }
}
