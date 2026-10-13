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

/// A region's texture slot: the file or solid fill it shows, and its texture's place in [`Texels`].
/// Only [`Texels::set`] and [`Texels::release`] write it, so every holder is counted. A clone (the
/// extract's working copy) carries the id uncounted, which is sound while the original holds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TexSlot {
    path: Option<String>,
    fill: Option<[f32; 4]>,
    id: Option<TexId>,
}

impl TexSlot {
    /// The texture path (`SetTexture("Interface\\...")`), or an engine image's.
    pub(crate) fn path(&self) -> &Option<String> {
        &self.path
    }

    /// The solid fill (`SetTexture(r, g, b, a)`, `<Color>` with no `file=`).
    pub(crate) fn fill(&self) -> Option<[f32; 4]> {
        self.fill
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
    /// A file, shared by name; one the install lacks is the cache's solid green (`0x449e7c` →
    /// `0x44a9c0` with `0xff00ff00` at `0x90582c`).
    File(String),
    /// A solid fill, shared by colour.
    Fill([f32; 4]),
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
    /// compares with `_strnicmp` (`0x449eb0` → `0x64a4c0`).
    File(Box<str>),
    /// The packed colour, a byte a channel.
    Fill(u32),
    Portrait(Box<str>),
    Own,
}

#[derive(Debug)]
struct Entry {
    key: Key,
    /// The creation stamp, from 1; 0 while a file waits for its first draw.
    stamp: u64,
    /// The slots holding it; 0 is a free entry.
    refs: u32,
}

/// Every live texture, its stamp and its holders.
#[derive(Debug, Default)]
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
const MISSING_FILL: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

impl Texels {
    /// Point `slot` at `set`'s texture: the same texture keeps its entry and stamp, another one is
    /// found or made, and the old one loses a holder. `missing` answers whether the install lacks a
    /// file (the host's texture probe).
    pub(crate) fn set(&mut self, slot: &mut TexSlot, set: TexSet, missing: impl Fn(&str) -> bool) {
        let (path, fill, key) = match set {
            TexSet::None => (None, None, None),
            TexSet::File(p) => {
                let key = if missing(&p) {
                    Key::Fill(pack(MISSING_FILL))
                } else {
                    Key::File(cache_name(&p))
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
        let file = matches!(key, Key::File(_));
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

/// A colour as the solid cache keys it, a byte a channel, `×255 + 0.5` truncated after a clamp.
fn pack(c: [f32; 4]) -> u32 {
    c.iter().enumerate().fold(0, |acc, (i, v)| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let byte = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        acc | byte << (8 * i)
    })
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
        let f = set(&mut t, &mut c, TexSet::Fill([0.0, 0.0, 0.0, 1.0]));
        let g = set(&mut t, &mut d, TexSet::Fill([0.0, 0.0, 0.001, 1.0]));
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
        t.set(&mut solid, TexSet::Fill([0.0; 4]), none);
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
        t.set(&mut green, TexSet::Fill([0.0, 1.0, 0.0, 1.0]), none);
        t.set(&mut lost, TexSet::File("Nowhere".into()), |_| true);
        assert_eq!(green.id(), lost.id());
        assert_eq!(lost.path().as_deref(), Some("Nowhere"));
    }
}
