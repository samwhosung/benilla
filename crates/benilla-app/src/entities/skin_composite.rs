//! Composited body skins, one 256² atlas per look. An arriving body's first composite runs off the
//! main thread: the reference's world composite is unforced, so its section loads poll and never
//! block the frame (`0x44b430`), and it draws nothing of a unit whose first composite has not
//! finished, its ShouldRender answering `0x477860(cc, 0)`'s result (`0x607e7c`). Only the forced
//! callers wait on their loads (`0x44ad50`): the glue model (`0x470c59`, `0x471308`, `0x4731b6`),
//! the dressing room (`0x504485`), `PlayerModel`'s `SetUnit` (`0x5059be`) and world entry
//! (`0x49091e`). Here the forced lane ([`SkinComposites::force`]) composites on the calling thread.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use benilla_assets::{repeat_texture_authored, LockRecover, SpatialCache};
use benilla_formats::{blp_bytes_to_mip_chain, BlpMipChain, Chain, CharSections, CompositePlan};
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};

/// The `CharSections` skin lookup and the chain its textures read from; without it a player's body
/// skin stays untextured.
#[derive(Resource)]
pub(super) struct SkinSections {
    pub(super) tables: CharSections,
    chain: Arc<Mutex<Chain>>,
}

impl SkinSections {
    pub(super) fn new(tables: CharSections, chain: Arc<Mutex<Chain>>) -> Self {
        Self { tables, chain }
    }
}

/// What decides a composited body skin: race and sex pick the `CharSections` rows, the dials pick
/// the variations, and `equip` holds the worn armour display ids by body slot − 2.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) struct SkinKey {
    pub(super) race: u8,
    pub(super) sex: u8,
    pub(super) skin: u8,
    pub(super) face: u8,
    pub(super) facial_hair: u8,
    pub(super) hair_style: u8,
    pub(super) hair_color: u8,
    pub(super) equip: [u32; 8],
    /// The guild emblem: two guilds' members wear one tabard display but must not share an atlas.
    pub(super) emblem: Option<benilla_formats::GuildEmblem>,
    /// The tabard designer's preview: the emblem paints over an empty tabard slot.
    pub(super) tabard_preview: bool,
}

/// Where a body's atlas stands.
#[derive(Debug, PartialEq)]
pub(super) enum BodyAtlas {
    /// Ready to bind; `None` for a look with no atlas (no base skin row, or art that never read).
    Ready(Option<Handle<Image>>),
    /// Its composite is running.
    Pending,
}

/// A composite's whole work, run where it lands: reads, decodes and blits.
type Work = Box<dyn FnOnce() -> Option<BlpMipChain> + Send>;

/// Composited body skins by look, so every body wearing a look shares one atlas, and the
/// composites still running.
#[derive(Resource, Default)]
pub(super) struct SkinComposites {
    /// Finished atlases, swept by distance ([`benilla_world::art_scope`]); `None` for a look with
    /// no atlas, so it is never retried.
    pub(super) done: SpatialCache<SkinKey, Option<Handle<Image>>>,
    /// Composites on the async compute pool, one per look however many bodies wait on it.
    running: HashMap<SkinKey, Task<Option<BlpMipChain>>>,
}

impl SkinComposites {
    /// A world body's atlas: finished, or running, its composite started here on a miss. `plan` is
    /// read only then; `None` is a look with no atlas.
    pub(super) fn request(
        &mut self,
        key: SkinKey,
        sections: &SkinSections,
        plan: impl FnOnce() -> Option<CompositePlan>,
    ) -> BodyAtlas {
        self.request_with(key, || plan().map(|p| work(p, sections.chain.clone())))
    }

    /// [`Self::request`] over any work, so the lane is testable without the install.
    fn request_with(&mut self, key: SkinKey, work: impl FnOnce() -> Option<Work>) -> BodyAtlas {
        if let Some(done) = self.done.fetch(&key) {
            return BodyAtlas::Ready(done);
        }
        if self.running.contains_key(&key) {
            return BodyAtlas::Pending;
        }
        let Some(work) = work() else {
            self.done.insert(key, None);
            return BodyAtlas::Ready(None);
        };
        self.running.insert(
            key,
            AsyncComputeTaskPool::get().spawn(async move { work() }),
        );
        BodyAtlas::Pending
    }

    /// An atlas composited on this thread on a miss, or waited for if it is already running, as
    /// the reference's forced composite waits on its section loads (`0x44ad50`) and passes no
    /// admission test: the glue model's (`0x477860(cc, 1)` at `0x470c59`, `0x471308`, `0x4731b6`)
    /// and the dressing room's (`0x504485`, in `0x504470`). A re-dress of a standing body and a
    /// rig-heal rebuild come here too, so neither drops out for a frame.
    pub(super) fn force(
        &mut self,
        key: SkinKey,
        sections: &SkinSections,
        plan: impl FnOnce() -> Option<CompositePlan>,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        self.force_with(
            key,
            || plan().map(|p| work(p, sections.chain.clone())),
            images,
        )
    }

    /// [`Self::force`] over any work, so the lane is testable without the install.
    fn force_with(
        &mut self,
        key: SkinKey,
        work: impl FnOnce() -> Option<Work>,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        if let Some(done) = self.done.fetch(&key) {
            return done;
        }
        let atlas = match self.running.remove(&key) {
            Some(task) => block_on(task),
            None => work().and_then(|w| w()),
        };
        self.install(key, atlas, images)
    }

    /// Move every finished composite into [`Self::done`]; how many landed.
    pub(super) fn land(&mut self, images: &mut Assets<Image>) -> usize {
        let mut finished = Vec::new();
        self.running
            .retain(|key, task| match block_on(future::poll_once(task)) {
                Some(atlas) => {
                    finished.push((*key, atlas));
                    false
                }
                None => true,
            });
        let landed = finished.len();
        for (key, atlas) in finished {
            self.install(key, atlas, images);
        }
        landed
    }

    /// Upload one atlas and cache it by look.
    fn install(
        &mut self,
        key: SkinKey,
        atlas: Option<BlpMipChain>,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        // Through the upload gate like every texture: a no-op on this RGBA8 composite, but it
        // keeps the format and the bytes in agreement.
        let handle = atlas.map(|a| {
            images.add(repeat_texture_authored(
                benilla_assets::for_upload(a),
                (true, true),
            ))
        });
        self.done.insert(key, handle.clone());
        handle
    }

    /// How many composites are running.
    #[cfg(test)]
    pub(super) fn running(&self) -> usize {
        self.running.len()
    }

    /// Drop every atlas and every running composite: the map-change teardown.
    pub(super) fn clear(&mut self) {
        self.done.clear();
        self.running.clear();
    }
}

/// A plan's work over the shared chain, which each read locks for its archive lookup alone.
fn work(plan: CompositePlan, chain: Arc<Mutex<Chain>>) -> Work {
    Box::new(move || {
        plan.run(|path| read_texture(&chain, path))
            .inspect_err(|e| warn!("body skin composite failed: {e:#}"))
            .ok()
    })
}

/// Read and decode one BLP off `chain`, locked only to find its archive.
fn read_texture(chain: &Mutex<Chain>, path: &str) -> anyhow::Result<BlpMipChain> {
    let name = path.replace('/', "\\");
    let archive = chain.lock_recover().archive_for(&name)?;
    let bytes = archive
        .read_file(&name)
        .with_context(|| format!("reading texture '{name}'"))?;
    blp_bytes_to_mip_chain(&bytes).with_context(|| format!("decoding texture '{name}'"))
}

/// Land every finished composite before this frame's bodies ask for theirs.
pub(super) fn land_skin_composites(
    mut composites: ResMut<SkinComposites>,
    mut images: ResMut<Assets<Image>>,
) {
    composites.land(&mut images);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn key(race: u8) -> SkinKey {
        SkinKey {
            race,
            sex: 0,
            skin: 0,
            face: 0,
            facial_hair: 0,
            hair_style: 0,
            hair_color: 0,
            equip: [0; 8],
            emblem: None,
            tabard_preview: false,
        }
    }

    /// A 1×1 atlas.
    fn atlas() -> BlpMipChain {
        BlpMipChain {
            width: 1,
            height: 1,
            texels: benilla_formats::BlpTexels::Rgba8Unorm,
            mips: vec![vec![255; 4]],
        }
    }

    /// Work that finishes when the test says so, and counts how many times it ran. It gives up
    /// after two seconds, so work run inline on the test's thread fails the test, never hangs it.
    fn gated(ran: &Arc<std::sync::atomic::AtomicUsize>) -> (mpsc::Sender<()>, Work) {
        let (tx, rx) = mpsc::channel::<()>();
        let ran = ran.clone();
        let work: Work = Box::new(move || {
            ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            rx.recv_timeout(std::time::Duration::from_secs(2)).ok()?;
            Some(atlas())
        });
        (tx, work)
    }

    /// Land until `key` is done, or give up after two seconds.
    fn land_until(c: &mut SkinComposites, images: &mut Assets<Image>, key: SkinKey) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while c.running.contains_key(&key) && std::time::Instant::now() < deadline {
            c.land(images);
            std::thread::yield_now();
        }
    }

    /// A look's composite runs once off the main thread however many bodies ask, and every body
    /// gets the one atlas once it lands.
    #[test]
    fn a_look_composites_once_off_the_main_thread() {
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut images = Assets::<Image>::default();
        let mut c = SkinComposites::default();
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (tx, w) = gated(&ran);
        let mut w = Some(w);

        assert_eq!(c.request_with(key(1), || w.take()), BodyAtlas::Pending);
        // A second body of the same look waits on the same composite; its work is never built.
        assert_eq!(
            c.request_with(key(1), || panic!(
                "a running look starts no second composite"
            )),
            BodyAtlas::Pending
        );
        assert_eq!(c.running(), 1);
        assert_eq!(
            c.land(&mut images),
            0,
            "nothing lands before the work finishes"
        );

        tx.send(()).unwrap();
        land_until(&mut c, &mut images, key(1));
        let BodyAtlas::Ready(Some(first)) = c.request_with(key(1), || None) else {
            panic!("the landed atlas is ready");
        };
        assert_eq!(
            c.request_with(key(1), || None),
            BodyAtlas::Ready(Some(first))
        );
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(c.running(), 0);
    }

    /// A forced composite of a look already running waits for that composite rather than starting
    /// a second, and the atlas is the one every later request gets.
    #[test]
    fn a_forced_composite_takes_over_the_running_one() {
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut images = Assets::<Image>::default();
        let mut c = SkinComposites::default();
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (tx, w) = gated(&ran);
        let mut w = Some(w);
        assert_eq!(c.request_with(key(3), || w.take()), BodyAtlas::Pending);
        tx.send(()).unwrap();
        let forced = c.force_with(
            key(3),
            || panic!("a running look starts no second composite"),
            &mut images,
        );
        assert!(forced.is_some(), "the running composite's atlas");
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(c.running(), 0);
        assert_eq!(c.request_with(key(3), || None), BodyAtlas::Ready(forced));
    }

    /// A look with no atlas is ready at once, as `None`, and never retried.
    #[test]
    fn a_look_without_an_atlas_never_waits() {
        let mut c = SkinComposites::default();
        assert_eq!(c.request_with(key(9), || None), BodyAtlas::Ready(None));
        assert_eq!(
            c.request_with(key(9), || panic!("a look with no atlas is never retried")),
            BodyAtlas::Ready(None)
        );
        assert_eq!(c.running(), 0);
    }

    /// The map-change teardown drops running composites with the cache: a body still waiting
    /// starts its composite again.
    #[test]
    fn a_teardown_drops_the_running_composites() {
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut c = SkinComposites::default();
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (_tx, w) = gated(&ran);
        let mut w = Some(w);
        assert_eq!(c.request_with(key(2), || w.take()), BodyAtlas::Pending);
        c.clear();
        assert_eq!(c.running(), 0);
        let (_tx2, w2) = gated(&ran);
        let mut w2 = Some(w2);
        assert_eq!(c.request_with(key(2), || w2.take()), BodyAtlas::Pending);
        assert!(
            w2.is_none(),
            "a body still waiting starts its composite again"
        );
    }
}
