//! **The UI session's lifecycle** — the VM's birth, its identity, its death, and the reload.
//! Split from `mod.rs` when the reload verb landed and the file held
//! two concerns: this, and the per-frame extract/input bridge that stayed behind.
//!
//! The shape, end to end: `Startup` installs a **boot VM** (strings, emote tokens, fonts — the
//! state the glue screens run against); entering the world loads the in-game UI and every addon
//! onto it under the picked character's identity; leaving the world runs the reference's ordered
//! shutdown tail and installs a fresh boot VM; and `ReloadUI()` is the two edges back to back
//! without leaving the world. Every function here is an edge — nothing per-frame lives here.

use bevy::prelude::*;

use benilla_assets::LockRecover;
use benilla_ui::script::UiScript;

use super::manifest::silenced_ui_load;
use super::{
    load_font_registry, load_ingame_ui, CursorPayloadHeld, PlayerUiHover, UiClock,
    UiKeyboardCapture,
};
use crate::ui_script::addons;

pub(crate) fn setup_script(world: &mut World) {
    install_boot_vm(world);
}

/// Build a **boot VM** and install it: a Lua state carrying the strings, the emote tokens and the
/// font-object registry, and no frames at all.
///
/// This is the state the client sits at outside the world — the login and character screens. It is
/// installed at `Startup`, at every [`end_ui_session`], and **at every world entry**
/// ([`load_ingame_ui_on_world_entry`]): the in-game UI is never loaded onto the VM
/// the glue phase was using, so no login inherits a session id — and therefore a
/// [`super::VmMemo`] — from the character screen that preceded it.
///
/// ONLY the font-object registry at boot (1051). The glyph atlas bakes once, on the first `Update`,
/// from `script.font_objects()` — and our native glue screens share that one atlas, so the registry
/// has to exist before the login screen or in-game text loses its outlined variants and its
/// registry-declared sizes for the whole session. The other 55 files are in-game UI and load at
/// world entry ([`load_ingame_ui`]); the reference splits at exactly this seam, with GlueXML
/// carrying its own `GlueFonts.xml`.
///
/// **Since 1290 this runs per login, and that is load-bearing rather than incidental.** The
/// reference re-loads `Fonts.xml` on every rebuild and *must*: `0x48fbf0`'s toc loop has no
/// already-loaded gate, and both halves of a font object die with the old session — the Lua handle
/// with `_G`, and the native `CSimpleFont` registry with the frame-script owner (`0x7839c0`, torn
/// down at `0x490c97`). Because [`end_ui_session`] routes every login through here, every session's
/// VM gets the registry and every `inherits="GameFontNormal"` in FrameXML resolves. A design that
/// dropped the VM and skipped index 0 on the in-game load would fail exactly there (2277).
fn install_boot_vm(world: &mut World) {
    let mut script = match UiScript::new() {
        Ok(s) => s,
        Err(e) => {
            error!("ui_script: VM init failed: {e}");
            world.remove_non_send_resource::<UiScript>();
            return;
        }
    };
    seed_vm_clock(world, &mut script);
    install_addon_asset_resolvers(world, &mut script);
    load_global_strings(world, &script);
    load_emote_tokens(world, &script);
    if ui_wanted(world) {
        // Errors are already logged per file as they happen; the returned list is the test-side
        // assertion (`shipped_xml_tests`), not a second reporting channel.
        let _ = load_font_registry(&script);
    }
    world.insert_non_send_resource(script);
}

/// **`GetTime()` is the PROCESS's clock, not the VM's** — so a VM built now starts
/// it where the process already is, and [`UiClock`] is re-anchored to match in the same breath.
///
/// The reference's `GetTime` (`0x515ea0`) reads `KERNEL32!GetTickCount` and scales by 0.001 (the
/// thunk `0x42c010` → `0x42b790`): an OS clock, with no relationship
/// to the Lua VM, that cannot restart. Ours is a Lua global the VM owns, and since 1290/1291 the VM
/// is destroyed and rebuilt at every logout/login and every `ReloadUI` — so without this seed the
/// clock went back to zero on each of those, and every host value already converted onto it
/// (`CooldownInfo::ui_triple` and its kin, the aura feed's `expirationTime`) suddenly sat in the
/// past. Stock `Cooldown.lua`'s `CooldownFrame_SetTimer` gates on `start > 0` and takes the `else`
/// branch — `this:Hide()` — for anything else, so after a relog **every cooldown still running
/// drew nothing** while the store, and therefore the cast validator, still held it: no sweep on the
/// button and a "Spell is not ready yet" on the press.
///
/// `Time<Real>` is the right clock because it is the very one [`UiClock`] already anchors on:
/// `elapsed` is measured at `last_update()`, which is [`UiClock::anchor`], so the pair written here
/// is atomic in exactly the sense that resource's doc requires. A world with no `Time<Real>` (a
/// bare test world) starts at zero, which is what it did before this existed.
fn seed_vm_clock(world: &mut World, script: &mut UiScript) {
    let (anchor, elapsed) = world
        .get_resource::<Time<bevy::time::Real>>()
        .map_or_else(Default::default, |t| {
            (t.last_update(), t.elapsed_secs_f64())
        });
    script.set_now(elapsed);
    if let Some(mut clock) = world.get_resource_mut::<UiClock>() {
        *clock = UiClock {
            anchor: anchor.unwrap_or_else(std::time::Instant::now),
            ui_now: elapsed,
        };
    }
}

/// Wire up the halves of `Interface\AddOns\` **art and fonts**: the sprite
/// decoder's **loose-file root** (so addon-shipped BLP/TGA files render at all — the store's
/// [`benilla_assets::WorldAssets::set_loose_addon_root`]), the VM's **texture probe** (so the
/// path form of `SetTexture` can answer the reference's 1|nil load verdict — Atlas picks its map
/// art by that return), and the VM's **font probe** (the same 1|nil for `SetFont`, whose nil is a
/// load failure — `!OmniCC` uses it as a font-file validity check). All resolve the same one folder
/// ([`addons::root`], hermetic-`None` under `$WOW_CAPTURE`), and each probe walks exactly the
/// store its renderer reads — [`benilla_assets::sprite_candidates`] for art, the chain-then-folder
/// pair [`crate::ui_text`]'s face loader uses for fonts — so the verdict the Lua caller gets is the
/// verdict the screen shows.
///
/// The **size probe** beside it is the same oracle answering a different question — how many texels
/// wide and tall is the art — which is what lets a region that authored no size on an axis take
/// that span from its content, as the client's virtual size getters do (the fix for
/// B342's page-sized book crest). It goes through the decoder, so the number layout resolves with
/// is the number the screen shows, and memoises per texture key: the ask is per zero-size region
/// per resolve, and the answer cannot change for a key that already read.
///
/// No `WorldAssets` (no client data) means no backend: nothing to install, so `SetTexture`'s path
/// form keeps answering nil and `SetFont`'s keeps answering 1 — each store's own engine-less truth
/// (`Model::texture_probe` / `Model::font_probe` carry why the two defaults differ).
///
/// Both probes test **existence**, not decode success: a file that is there but will not decode
/// answers 1 and draws nothing. That is 1322's stated approximation and it is unchanged here; the
/// renderer's own `texture miss` / `font miss` WARN is what names such a file.
fn install_addon_asset_resolvers(world: &mut World, script: &mut UiScript) {
    let root = addons::root();
    let Some(mut assets) = world.get_resource_mut::<benilla_assets::WorldAssets>() else {
        return;
    };
    assets.set_loose_addon_root(root.clone());
    let chain = assets.chain.clone();
    let size_chain = chain.clone();
    let size_root = root.clone();
    let font_chain = chain.clone();
    let font_root = root.clone();
    script.set_texture_probe(Box::new(move |path| {
        benilla_assets::sprite_candidates(path).iter().any(|c| {
            chain.lock_recover().contains(c)
                || root
                    .as_deref()
                    .is_some_and(|r| benilla_assets::loose_addon_file(r, c).is_some())
        })
    }));
    // Keyed by the reference string exactly as the region carries it, so the hit path allocates
    // nothing: this is asked from inside the layout sweep, once per zero-size textured region per
    // pass. Two spellings of one file (`Foo` and `Foo.blp`) cost two entries holding the same
    // number, which is cheaper than normalising every ask to avoid it. Misses are cached too — a
    // path that resolves to nothing measures nothing however often we look.
    let sizes: std::cell::RefCell<std::collections::HashMap<String, Option<(u32, u32)>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    script.set_texture_size_probe(Box::new(move |path| {
        if let Some(cached) = sizes.borrow().get(path) {
            return *cached;
        }
        let measured = benilla_assets::sprite_dimensions(&size_chain, size_root.as_deref(), path);
        sizes.borrow_mut().insert(path.to_string(), measured);
        measured
    }));
    // The font oracle. Existence over the same two stores, in the same order, that
    // `ui_text::engine`'s face loader reads — a `SetFont` that answers 1 and then draws Friz
    // because the two disagreed would be worse than no probe at all. Memoised for the same reason
    // the size probe is: an addon can call `SetFont` per animated string per frame (MSBT does),
    // and a path's answer cannot change mid-session.
    let seen: std::cell::RefCell<std::collections::HashMap<String, bool>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    script.set_font_probe(Box::new(move |path| {
        if let Some(&cached) = seen.borrow().get(path) {
            return cached;
        }
        let found = font_chain.lock_recover().contains(path)
            || font_root.as_deref().is_some_and(|r| {
                benilla_assets::loose_addon_file(r, &benilla_assets::normalize_path(path)).is_some()
            });
        seen.borrow_mut().insert(path.to_string(), found);
        found
    }));
}

/// Does this run want the player UI at all? Captures stay pristine — their baselines regression-test
/// the WORLD render — unless the UI is opted in, which happens two ways: `WOW_CAPTURE_UI=1` on any
/// scenario, or the scenario **declaring a `ui:` fixture**, which is the harness saying the window
/// is the subject — [`crate::run_mode::capture_ui_opted_in`] is the one predicate, and its doc says why all
/// three consumers of it must agree. A run with no `CaptureMode` is an ordinary client and always
/// wants its UI.
fn ui_wanted(world: &World) -> bool {
    !world.contains_resource::<crate::run_mode::CaptureMode>()
        || crate::run_mode::capture_ui_opted_in()
}

/// The world-entry UI load, armed at `OnEnter(InWorld)` and run by [`run_pending_entry_load`]
/// once the loading cover is actually ON THE GLASS.
///
/// This is 0962's frame accounting, second offender: `OnEnter(InWorld)` fires on exactly the
/// frame whose render would first present the cover (the raise frame still renders the glue
/// above it), so a synchronous burst there holds the *previous* present — the frozen character
/// screen — for its whole duration. 0962 taught the menagerie to wait for
/// `WARM_COVER_PRESENT_FRAMES`; 1051 then moved the FrameXML load onto the same unprotected
/// frame, and by 1290+addons the burst had grown to ~0.5 s (measured live: raise 48.317 →
/// reference sourcing + 55 files + addons + saved vars + `PLAYER_LOGIN` → 48.83) — the
/// director's "frozen char for 1 sec" at every Enter World.
///
/// The wait itself is [`crate::loading_screen::EntryCover`]'s — one count of covered+in-world
/// frames for the whole client. This used to keep its own, beside the warm pass's identical one,
/// and the duplication is not incidental: the third consumer of the same rule (the world camera,
/// which renders the whole new world on that same frame) had no counter at all until it was
/// measured. So the latch here is a *marker* — is the entry UI still owed? — and the timing
/// question is answered in one place. The loading screen folds this resource's existence into
/// its clear condition, so a reveal can never precede the UI it is supposed to reveal.
#[derive(Resource, Default)]
pub(crate) struct PendingEntryUiLoad;

/// **The boot VM, parked for the deferral window**. Between the world-entry edge
/// and the deferred entry load there is no VM in the world at all: the boot VM waits here, out of
/// every feed's reach, and the load takes it back. Every feed takes the VM as an `Option` and
/// already returns on `None` — the glue phase's shape — so a feed keyed on a per-VM memo cannot
/// push into a VM that has no interface yet and then hold the real push back (the chat plate
/// that stayed white, and 1348's whole class). `ingame_ui_pending` stays for the feeds that
/// name it; with the VM parked it is belt and braces.
pub(crate) struct ParkedBootVm(UiScript);

/// `OnEnter(InWorld)`: arm the deferred entry load and park the boot VM. The load itself runs a
/// few frames later — see [`PendingEntryUiLoad`] and [`ParkedBootVm`].
pub(crate) fn arm_entry_ui_load(world: &mut World) {
    world.insert_resource(PendingEntryUiLoad);
    if let Some(vm) = world.remove_non_send_resource::<UiScript>() {
        world.insert_non_send_resource(ParkedBootVm(vm));
    }
}

/// **Retire the glue phase's VM and build the one the entry load will run on** —
/// `0x48fe97`, the reset inside `UI_Init 0x48fbf0` itself, expressed against our own load.
///
/// The fold first, because it is the one thing that must outlive the VM being discarded. The glue
/// phase can write CVars — the character screen's AddOns panel is the live case, a *Load out of
/// date AddOns* click — and `sync_cvars` (1291) registers them onto whichever VM is in the world,
/// so this is the last moment that table exists. Folded into the persist state, the
/// `set_cvar_saved_base` + `seed_cvars` pair in the load below starts the new VM at the
/// player's value. That is exactly the route `/reload` has always taken through
/// [`end_ui_session`]; 2226 makes the first login take it too.
///
/// Nothing else has to survive: every other thing the glue VM was carrying is either re-seeded
/// onto the new VM a few lines later (the realm, the player seat, the addon array, the minimap
/// zoom, the text measurer, the strings, the emote tokens, the font registry) or is a
/// [`super::VmMemo`], which is *supposed* to reset here — that reset is the whole point.
fn mint_entry_vm(world: &mut World) {
    crate::cvars::fold_dying_vm_cvars(world);
    install_boot_vm(world);
}

/// The parked boot VM back into the world, if one is parked — the session end and the
/// left-before-the-load arm both want the VM where the tail expects it.
fn unpark_boot_vm(world: &mut World) {
    if let Some(ParkedBootVm(vm)) = world.remove_non_send_resource::<ParkedBootVm>() {
        world.insert_non_send_resource(vm);
    }
}

/// **Is the in-game UI up on the VM that is in the world?** The run condition every in-world feed
/// wants — nothing is pushed and nothing is fired until there is an interface to receive it.
///
/// The reason it exists (1348): the feeds fire the login **one-shots** —
/// `PLAYER_ENTERING_WORLD`, the first `PLAYER_XP_UPDATE`, the first `UPDATE_EXHAUSTION` — and
/// every one of them is latched by a [`super::VmMemo`] keyed on the VM's *session*, which the
/// entry load does not change (it loads files ONTO this VM). So an event fired against the boot
/// VM is delivered to nobody and then never fires again for the whole session: the frames built
/// moments later do their first paint with no first paint. That is not a hypothetical ordering —
/// it is a RACE against the wire, which is why it took some logins and not others, and why the
/// symptom moved between characters. The self descriptor arriving inside the deferral window is
/// all it takes.
///
/// **The reference has no such window — but not for the reason this doc used to give** (corrected
/// 2226). It said `UI_Init 0x48fbf0` fires the world-enter cascade from inside itself, so a UI-less
/// client never sees a unit event; that is 1348's reading and the bytes refute it, because the
/// cascade call at `0x490168` is gated on an active-player GUID that is still 0/0 on a fresh login
/// (see [`finish_ui_init`]). The conclusion survives, on four independent structural grounds:
///
/// - World entry is a **category-5** callee (`0x420d63`), dispatched after the same iteration's
///   **category-6** inbound drain (`0x420d55`) has already returned.
/// - A nested drain is impossible: the drain's container `0x420c00` is entered **once per
///   process**, so no unresolved indirect call inside the load can re-enter it.
/// - Reception is decoupled — the socket threads only **enqueue** (`0x537b7c`); handlers run only
///   from the main-thread drain. `SMSG_PONG` (`0x1dd`, inline at `0x537b56`) is the single bypass
///   in the image, and it carries no game state.
/// - The client does not even **send** the login request until a later frame (`0x46c272`), gated on
///   the async loads being idle — i.e. not until the UI is up.
///
/// So the real client cannot process world state without an interface, structurally, and our
/// deferred load is expressing that same property with a run condition. The first frame this
/// answers true on is a fresh, unlatched world, and the feed delivers the full set in order.
///
/// **TWO terms, because `not(ingame_ui_pending)` is only half of it**. The latch is armed
/// at `OnEnter(InWorld)`, and that edge trails the wire by a frame: `apply_net_updates` drains
/// `Connected` and the whole login burst behind it in one `try_iter`, `enter_on_connected` sets
/// `NextState` from that same drain, and the transition — with it the park and the latch — does
/// not run until the NEXT frame's `StateTransition`. So for exactly one frame the client has
/// in-world wire state and a live *boot* VM, and 1978's park does not cover it. `InWorld` is the
/// term that does: the state flips on the same edge that arms the latch, so the pair is closed at
/// both ends. B376's guild MOTD was fired into that one frame (observed live, 1 login in 3);
/// the unit feed and the cinematic's UI edge sat on the same hole, unobserved.
pub(crate) fn ingame_ui_up(
    pending: Option<Res<PendingEntryUiLoad>>,
    state: Option<Res<State<crate::char_select::ClientState>>>,
) -> bool {
    pending.is_none() && state.is_some_and(|s| *s.get() == crate::char_select::ClientState::InWorld)
}

/// `PreUpdate` (chained after [`run_pending_reload`] — same exclusive slot, and a reload must
/// not interleave an armed entry load): run the armed entry load once the cover has presented.
///
/// No cover at all — a capture booting straight `InWorld`, or the screen's assets missing —
/// means there is no glass to protect and nothing watching the previous present: load
/// immediately. Leaving the world first (an instant disconnect) drops the latch unrun;
/// [`end_ui_session`] treats that as "the in-game UI never existed" and skips the shutdown
/// writes, so a UI-less VM can never clobber the saved-variables files with its emptiness.
pub(crate) fn run_pending_entry_load(world: &mut World) {
    if world.get_resource::<PendingEntryUiLoad>().is_none() {
        return;
    }
    let in_world = *world
        .resource::<State<crate::char_select::ClientState>>()
        .get()
        == crate::char_select::ClientState::InWorld;
    if !in_world {
        // Left the world before the load ran — nothing to build a UI for.
        world.remove_resource::<PendingEntryUiLoad>();
        unpark_boot_vm(world);
        return;
    }
    let covering = world
        .get_resource::<crate::loading_screen::LoadingScreen>()
        .is_some_and(|s| s.covering());
    // The wait is [`crate::loading_screen::EntryCover`]'s, not a second count of the same frames
    // (1345 wrote its own beside the warm pass's, and the third consumer — the world camera —
    // then had none at all). `presented()` is already true with no cover up, which is the
    // "nothing watching the previous present, load now" arm.
    if !world
        .get_resource::<crate::loading_screen::EntryCover>()
        .is_none_or(|c| c.presented())
    {
        return;
    }
    world.remove_resource::<PendingEntryUiLoad>();
    let start = std::time::Instant::now();
    load_ingame_ui_on_world_entry(world);
    // The standing instrument for this burst: the one number that says whether the cover is
    // still hiding it, on every entry, in every log — and, since 2226, WHICH VM it came up on.
    // The session is the load-bearing half of that record: it is what every `VmMemo` keys on, so
    // a login whose session did not move from the character screen's is a login where the whole
    // one-shot class is already spent. Printing it means the next report of a missing login line
    // can be answered from the log the run already wrote.
    info!(
        "ui_script: in-game UI up in {:.0} ms (behind the cover: {}, vm session: {})",
        start.elapsed().as_secs_f32() * 1000.0,
        covering,
        world
            .get_non_send_resource::<UiScript>()
            .map_or(0, UiScript::session),
    );
}

/// **The raster seam, at the load edge**: the VM's screen size and its font engine, both under the
/// CURRENT window and `uiScale` — the load-edge half of [`super::extract::tick_script`]'s own
/// per-frame pair.
///
/// **The screen size is the half that had been missing, and it is a real screen bug** (decision
/// 2242). A fresh `Model` starts at **1024×768** and `set_screen_size` is called from
/// `tick_script`, an `Update` system — so between 2226 (the entry mints its own VM) and this, every
/// `<OnLoad>` in FrameXML and every addon's file scope read `GetScreenWidth()/GetScreenHeight()` as
/// 1024×768 no matter what window the player had. Verified live with a probe addon: `SCREENPROBE
/// file-scope … 1024x768`, on both logins of a round trip.
///
/// Almost everything survives that, because almost everything is *anchored* and the first `Update`
/// resize re-solves it. What does not survive is a number a file **computed once** from those
/// getters — and the stock world map is exactly that: `WorldMapFrame_OnLoad` sizes `BlackoutWorld`,
/// the full-screen quad that hides the world behind the map, with `GetScreenWidth()`/
/// `GetScreenHeight()` and never touches it again. Sized 1024×768 UI units on a wider window, it
/// leaves the world showing along the right and bottom edges — the director's report.
///
/// No atlas means no measure to be had (it bakes on the first `Update`, from the patch chain and
/// the window's real `scale_factor`); the per-frame pass seats one on the first frame it appears,
/// exactly as before. The screen size goes in either way, and only when the window has a real size
/// — a bare test world has none, and `0` would be worse than the default it already carries.
fn seat_raster_seam_for_load(world: &mut World, script: &mut UiScript) {
    let ui_scale = world
        .get_resource::<super::UiScaleCvar>()
        .map_or(1.0, |c| c.0);
    let (w, h) = {
        let mut q = world.query_filtered::<&Window, With<bevy::window::PrimaryWindow>>();
        q.single(world)
            .map_or((0.0, 0.0), |win| (win.width(), win.height()))
    };
    let s = super::seam_scale(h, ui_scale);
    if w > 0.0 && h > 0.0 {
        // `tick_script`'s own line, with its own units: the VM lives in 768-tall virtual space,
        // so the window's logical size divided by the seam scale IS the screen
        // the getters answer. No `UIParent_ManageFramePositions` beside it, unlike there: nothing
        // is laid out yet, and the first tick will see no resize to re-run it for.
        script.set_screen_size(w / s, h / s);
    }
    let Some(atlas) = world.get_resource::<crate::ui_text::UiFontAtlas>() else {
        return;
    };
    super::extract::seat_text_measurer(script, atlas, s);
}

/// Materialize the in-game UI for **this** session.
///
/// **Once per world entry, not once per process**. The reference builds the whole
/// in-game UI at `CGGameUI::Initialize 0x48fbf0` and destroys it again at `0x490bd0` on the way
/// out, so every login runs every addon's file scope afresh — and that file scope is where the
/// corpus reads the character it is looking at (`local currentPlayer = UnitName("player")`, the
/// idiom [`seat_from_roster`] already documents). While this was latched per process, a second
/// login kept the first login's captured name: the director's "the selected one is always
/// Onewarrior no matter what char I log into", and, worse, its saved variables went to the first
/// character's file. [`world_entry_tests`] holds both ends.
///
/// **The entry BUILDS its VM; it does not adopt the glue phase's**. That is the
/// reference's own shape — the next Lua state is born *inside* `UI_Init 0x48fbf0`, at `0x48fe97`,
/// before the bindings and the FrameXML walk — and it is what [`benilla_ui::script::UiScript::session`]'s contract has
/// claimed all along ("destroys its Lua state at logout and builds another at the next world
/// entry"). We were half-keeping it: [`install_boot_vm`] ran at `end_ui_session`, so a *second*
/// login was a genuine second load, but the boot VM that `Startup` (or that same logout) left
/// behind then carried its session **across the entry edge** — the glue phase and the world that
/// followed it were one VM with one session id.
///
/// That shared id is what made the login one-shot class (1348) so hard to close. A
/// [`super::VmMemo`] keys on the session, so an edge spent against the FRAMELESS glue VM — any
/// feed that reached it in the window between the wire turning in-world and this load — was spent
/// for the whole login: the event went to a VM with no frames to hear it, and the memo said
/// "already told" forever after. Gating the feeds (1978's park, B376's `ingame_ui_up`) closes
/// that window one feed at a time and silently misses the next one written. A new session closes
/// it for **every** memo at once, without any feed having to know this problem exists: whatever
/// was spent against the glue VM is re-spent here, against a VM that has an interface.
///
/// Safe on the state edge only because 1038 moved the initial transition after `PostStartup` — a
/// capture boots straight into `InWorld`, so before that this would have run ahead of
/// [`benilla_assets::AssetSet::Open`] and loaded against no patch chain.
pub(crate) fn load_ingame_ui_on_world_entry(world: &mut World) {
    // The VM the entry edge parked (1978); the live slot is the fallback for a caller that did
    // not go through the arm. It goes back in the world because the fold below reads it there.
    unpark_boot_vm(world);
    if !ui_wanted(world) {
        return;
    }
    mint_entry_vm(world);
    let Some(mut script) = world.remove_non_send_resource::<UiScript>() else {
        warn!("ui_script: entering the world with no VM — the in-game UI will not load");
        return;
    };
    // The character whose AddOn enable state applies. Resolved before the load because the
    // enable file gates which addons run at all.
    let identity = world
        .get_resource::<crate::char_select::Roster>()
        .and_then(crate::ui_macro::identity);
    // …and the realm's whole character list, which is the enable store's node set: the reference
    // populates one `ADDONSTATELIST` node per character at char-list time, and an addon this
    // character has never had an opinion about is resolved from what the others said.
    let roster: Vec<String> = world
        .get_resource::<crate::char_select::Roster>()
        .map(|r| r.chars.iter().map(|c| c.name.clone()).collect())
        .unwrap_or_default();
    // **The Lua index space, before a single addon file runs**. The reference has
    // the array in hand well before `UI_Init 0x48fbf0` reaches the addon walk — `SMSG_ADDON_INFO`
    // lands during the handshake — so an addon reading `GetNumAddOns()` at file scope sees a
    // populated one. Seated here rather than off a message for exactly that reason: a feed running
    // a frame later would seat it after every addon had already asked.
    //
    // A server that never answered leaves this `None`, and the array then stays empty for the
    // session — which is the reference's behaviour too, not a fallback we chose.
    if let Some(reply) = world
        .get_resource::<crate::net::AddonInfoReply>()
        .and_then(|r| r.0.clone())
    {
        script.note_addon_info_reply(&reply);
    }
    // **The CVar table goes in BEFORE the UI loads**, for the same reason and with a shipped-file
    // consumer rather than an addon one: the reference's own `UIOptionsFrame.xml`
    // — hidden, on the manifest for the addons that name it — reads `cameraSmoothStyle` and
    // `cameraSmoothTrackingStyle` inside its two camera dropdowns' `OnLoad`, and a nil there is a
    // concat error, not a default.
    //
    // The seed normally belongs to `cvars::sync_cvars`, a per-VM `Update` claim (1291), and on a
    // FIRST login that has run many frames earlier. **The window is a VM replacement**: a
    // `/reloadui` builds a fresh VM and loads the whole interface inside one call, before `Update`
    // gets a turn — so this edge would load a client with no CVar table at all. Nothing had ever
    // noticed, because until now no interface file read a CVar at load.
    //
    // The two lines are `sync_cvars`'s own seed, in its own order, and the order is
    // load-bearing: the file's unclaimed entries go in FIRST so an addon's `RegisterCVar` starts
    // its key at the player's value rather than the declared one (1291); then the registry's
    // whole table at its live values (2303) — the store that outlived the last VM. `sync_cvars`
    // still runs its own per-VM seed on the next `Update`; both are idempotent. A world with no
    // registry (a bare test app) gets the registered defaults.
    match world.get_resource::<crate::cvars::Cvars>() {
        Some(cvars) => {
            script.set_cvar_saved_base(cvars.orphans());
            script.seed_cvars(cvars.vm_seed());
        }
        None => script.register_cvars(crate::cvars::registered_pairs()),
    }
    // The realm name goes in BEFORE the UI loads, because `GetRealmName()` is read at addon file
    // scope — `MyAddonDB[GetRealmName()] = …` is the corpus idiom, and 24 addons stop on it.
    // The roster carries the auth realm-list entry this session connected to.
    let realm = world
        .get_resource::<crate::char_select::Roster>()
        .and_then(|r| r.realm.as_ref().map(|r| r.name.clone()))
        .unwrap_or_default();
    script.set_realm_name(&realm);
    // …and so does the PLAYER, for the same reason and with more riding on it — see
    // [`seat_from_roster`], which is where the why lives.
    //
    // **The player RECORD first, and separately**. The reference keeps the
    // local player's name, race, class and gender somewhere no cache, feed or object can reach — a
    // copy of the char-enum row written at the Enter World commit (`0x5abd9e`) and never cleared —
    // and `UnitName`/`UnitRace`/`UnitClass`/`UnitSex` read only that, unconditionally, even after
    // the descriptor exists. The snapshot below is the descriptor's stand-in until the real one
    // streams in and is replaced when it does; the record outlives every push. Seeded here because
    // our VM is rebuilt per login (1290) while the reference's record simply persists, so each new
    // VM has to be told once — before the addon walk, like the realm.
    if let Some(record) = world
        .get_resource::<crate::char_select::Roster>()
        .and_then(record_from_roster)
    {
        script.set_player_record(record);
    }
    if let Some(seat) = world
        .get_resource::<crate::char_select::Roster>()
        .and_then(seat_from_roster)
    {
        script.set_unit("player", Some(seat));
    }
    // The addon version gate: the live table when this VM has one, else the
    // persisted value. **Every entry is now the reload case** (2226) — the VM was born a few
    // lines up, so its table is whatever `register_cvars` just seeded off the persisted base, and
    // that base is current by construction because the fold runs in the same call. Before 2226
    // this read the glue phase's own live table, which is how a *Load out of date AddOns* click at
    // character select reached here inside the save debounce; the fold carries that click now, by
    // the route the reload path always used. Absent both (a bare test world) is the registrar
    // default: check ON.
    let version_check = world
        .get_resource::<crate::cvars::Cvars>()
        .is_none_or(crate::cvars::Cvars::addon_version_check);
    // **…and the rest of the same class**. Each of these was a per-VM claim in
    // `Update`, which answers *which* VM and not *when inside its life* — and every one of them
    // backs a Lua getter the load burst below reads:
    //
    // - the **zone-channel catalog**, whose empty state is not "no zone yet" to its three verbs
    //   but *"no such built-in channel"* — the leg that files `General` as a custom channel in the
    //   chat cache and puts a real `CMSG_JOIN_CHANNEL` on the wire;
    // - the **keybinding table**, without which stock `ActionButton_OnLoad` paints a blank hotkey
    //   corner and an addon's `SetBinding` on a stock command is a silent nil;
    // - the **default language**, which `ChatFrame_OnEvent`'s `PLAYER_ENTERING_WORLD` arm turns
    //   into the `[Language]` prefix gate for every chat line of the session.
    crate::ui_chat::seed_zone_channel_catalog(world, &mut script);
    crate::bindings::seed_bindings_for_vm(world, &mut script);
    crate::ui_unit::seed_default_language(world, &mut script);
    // **The world map's catalog, before the first addon file runs**. The continent
    // and zone lists behind `GetMapContinents`/`GetMapZones` are static DBC data, and the corpus
    // reads them at file scope: Astrolabe — the positioning library under Questie and Cartographer
    // — builds its entire continent → zone table inside `AceLibrary:Register`'s synchronous
    // `activate`. Pushed from an `Update` system it landed ~210 ms after this whole call returned,
    // so that table was built from two empty lists and every icon placement afterwards indexed a
    // nil zone. Same shape as the four seeds above, and the reference has no timing here at all.
    crate::ui_world_map::seed_world_map_catalog(world, &mut script);
    // **The raster seam — the screen size and the font engine — before the first `<OnLoad>` runs**.
    // The screen size is 2242's: a fresh VM starts at 1024×768 and the
    // feed that corrects it is an `Update` system, so every OnLoad that *computes* from
    // `GetScreenWidth()`/`GetScreenHeight()` baked the wrong number for the session — the stock
    // world map's full-screen blackout quad among them.
    //
    // The font engine is 2028's. Every file the
    // walk below loads may measure the text it just set — the era's own tab law is
    // `label:GetStringWidth() + 40` at OnLoad, and the addon corpus writes the same pair — and a
    // `GetStringWidth` with no measurer installed answers 0. Seated only from the per-frame pass
    // (`extract::tick_script`, an `Update` system), a VM that is BORN and LOADED inside one
    // exclusive `PreUpdate` slot never sees it: that is exactly `ReloadUI()`, which mints a fresh
    // boot VM in `end_ui_session` and calls straight into here, so every `/reload` measured 0
    // through its whole load edge and only converged a frame later off whatever poll the caller
    // had written to survive it.
    seat_raster_seam_for_load(world, &mut script);
    // The minimap zoom pair and the plate pair are read out of the world FIRST: the bracket's body
    // borrows `world` for the chat-cache restore, and two `Copy` pairs cost nothing next to
    // fighting that borrow. The plates are absent in a bare test world, where "both off" is also
    // the resource's own default.
    let zoom = {
        let z = world.resource::<crate::minimap::MinimapZoom>();
        (z.outdoor, z.inside)
    };
    let plates = world
        .get_resource::<crate::vplates::VPlateMode>()
        .copied()
        .unwrap_or_default();
    // **The whole load edge is silent** — the TOC walk, every addon, the saved variables and
    // `PLAYER_LOGIN`, the span `0x48fbf0` brackets itself across (`0x48fbfa` → `0x49016d`). Both
    // login and `ReloadUI` come through here; see [`silenced_ui_load`].
    silenced_ui_load(&mut script, |script| {
        let _ = load_ingame_ui(script, identity.as_ref(), &roster, version_check);
        // The Minimap widget was born a moment ago with `MinimapState::default()`; seed its two
        // live zoom indices from the persisted CVars now, before anything reads them — the
        // reference's own minimap reset path copying each CVar object's int into its live index.
        // Once only: from here the widget's index is the live truth and
        // `Minimap:SetZoom` writes the CVar back. Startup always precedes this state edge (1038),
        // so the knob is already loaded.
        script.set_minimap_zoom(zoom.0, zoom.1);
        // The saved-variables chunk runs HERE — after the XML assigned its file-scope defaults,
        // before any consumer reads them — then `VARIABLES_LOADED`. That is the reference's own
        // load order (`AddOn_Load 0x51f240` steps 2 → 4 → 6); reversing it means the
        // defaults always win and nothing can ever be remembered.
        //
        // **And the chat cache restores inside it, between `VARIABLES_LOADED` and `PLAYER_LOGIN`**
        // — the reference's own slot for the reader's `UPDATE_CHAT_WINDOWS` + `UPDATE_CHAT_COLOR`
        // burst (`0x4900d6`, after `0x4900b2` and before `0x490959`). It
        // is the sole firer of `UPDATE_CHAT_WINDOWS`, which is the only thing that registers a chat
        // frame for any `CHAT_MSG_*` (ref `ChatFrame.lua` l.1261-1273) — as an `Update` system it
        // landed after the session's first chat had already been routed, and the login MOTD went to
        // a window registered for nothing.
        finish_ui_load_with(
            script,
            // `NAMEPLATES_ON` / `FRIENDNAMEPLATES_ON` (2132). This seat, and not the `Update`
            // feed beside it, is what fixes the bug: `UIParent_OnEvent`'s first
            // `UpdateNameplates()` runs inside the `VARIABLES_LOADED` fired at the end of this
            // very call.
            |script| crate::vplates::push_plate_globals(script, plates),
            |script| {
                crate::ui_chat::restore_chat_looks(world, script);
            },
        );
    });
    // **Say it out loud when an addon didn't load**. Every failure the walk found
    // is retained now, but a log nobody knows to open does not fix silence — and silence is the
    // actual defect: addons that do not load, with nothing on screen to say which or why. Counted
    // off the retained log rather than the walk's
    // `failures` vec so the number matches what `/errors` will show: the log deduplicates, and one
    // broken addon that fails four files should not read as four broken addons. The VM is fresh
    // per world entry, so every `Load` row here is this load's.
    // Placed AFTER `finish_ui_load` so the count covers the WHOLE load edge — the saved-variables
    // chunks and `ADDON_LOADED`/`VARIABLES_LOADED` handlers it runs are part of loading, and an
    // addon that dies in one of them is as absent as one whose file was missing.
    let failed = script
        .diagnostics()
        .iter()
        .filter(|d| d.kind == benilla_ui::script::diagnostics::DiagnosticKind::Load)
        .count();
    if failed > 0 {
        if let Some(mut chat) = world.get_resource_mut::<crate::ui_chat::ChatLog>() {
            // Queued, not drawn: `ChatLog` is a pending buffer the chat feed drains once the UI is
            // up, which is what makes it safe to push from inside the world-entry load edge.
            chat.push_event(crate::ui_chat::ChatEvent::text_only(
                crate::ui_chat::ChatEventKind::System,
                format!(
                    "{failed} addon load {} — type /errors to see {}.",
                    if failed == 1 { "failure" } else { "failures" },
                    if failed == 1 { "it" } else { "them" }
                ),
            ));
        }
    }
    // **`DAMAGE_TEXT_FONT` binds HERE, at the end of the load edge, and once**:
    // `0x6c8470` runs from `0x401570 + 0x1620`, *after* the UI load `0x401602` — so after
    // FrameXML's `Fonts.xml` and after every non-LoadOnDemand addon's `ADDON_LOADED`, which is
    // where MikScrollingBattleText and pfUI assign it. The reference reads the global's value
    // eagerly, hands it to the font factory, and never looks again: one writer of `[0xce8820]`,
    // no invalidation, and a `/reloadui` does not re-run it.
    //
    // One step later than the reference, deliberately: it reads after `PLAYER_ENTERING_WORLD`
    // too, which here fires from [`crate::ui_unit`] when the self descriptor lands rather than
    // inside this call. Nothing in the corpus assigns a font that late, and moving the seat would
    // mean waiting on the wire for a value the whole load edge has already settled.
    world.insert_resource(crate::combat_text::read_damage_text_font(&script));
    // The load edge is over: disarm the instruction bound `load_ingame_ui` installed (decision
    // 1306). From here every OnUpdate and event handler runs unhooked — a session must not kill
    // a player's addon for being slow; only a load that never returns is fair game.
    script.clear_instruction_budget();
    world.insert_non_send_resource(script);
    world.insert_resource(AddOnIdentity(identity));
    // **The UI load's own arm of the world latch** (2239) — the reference's `0x490168 call
    // 0x4908c0`, `UI_Init`'s conditional leg into the world-enter cascade. It is what makes a
    // SECOND `/reload` fire `PLAYER_LEAVING_WORLD` again after the first spent the latch: no
    // entity is created or destroyed by a reload, so nothing else here would re-arm it.
    //
    // Unconditional, where the reference guards on the active-player GUID pair (`0x490166 je`)
    // and therefore *skips* this leg on a fresh login, arming from the player's create instead.
    // The outcome is the same byte either way — both roots reach "armed" — and reproducing which
    // of the two did it would buy nothing observable.
    if let Some(mut armed) = world.get_resource_mut::<LeavingWorldArmed>() {
        armed.arm();
    }
}

/// The wire's gender byte (0 male, 1 female) on `UnitSex`'s own 2/3 scale — the one mapping, so
/// the record and the snapshot can never disagree about which is which.
fn roster_sex(gender: u8) -> u8 {
    match gender {
        0 => 2,
        1 => 3,
        _ => 0,
    }
}

/// **The local player record the UI loads under** — our copy of the char-enum row the reference
/// copies at the Enter World commit (the bytes are on
/// [`benilla_ui::script::PlayerRecord`]).
///
/// The same roster row [`seat_from_roster`] builds the `"player"` *snapshot* from, and
/// deliberately the same `race_names`/`class_names` lookups: the reference resolves the record's
/// race and class bytes through `ChrRaces`/`ChrClasses` at call time, so resolving them once here
/// is the same answer, and sharing the lookup is what stops the record and the snapshot drifting
/// into two opinions about what race 4 is.
///
/// **The level is not here, and that is byte-verified, not an omission** — the reference's record
/// carries a level at `+0x108` and ships an accessor for it (`0x5abe00`) that *nothing calls*, so
/// `UnitLevel("player")` reads the descriptor.
pub(crate) fn record_from_roster(
    roster: &crate::char_select::Roster,
) -> Option<benilla_ui::script::PlayerRecord> {
    let row = roster.pending_row()?;
    let race = crate::ui_unit::race_names(row.race);
    let class = crate::ui_unit::class_names(row.class);
    Some(benilla_ui::script::PlayerRecord {
        name: row.name.clone(),
        race: race.map(|(n, f)| (n.to_string(), f.to_string())),
        class: class.map(|(n, f)| (n.to_string(), f.to_string())),
        sex: roster_sex(row.gender),
    })
}

/// The `"player"` snapshot the UI loads **under**, built from the roster row of the pick in
/// flight — `None` when there is no pick (a capture, a scenario, a test world).
///
/// **The reference's invariant is that addon file scope always sees a real character**:
/// `AddOn_Load 0x51f240` runs from inside `UI_Init 0x48fbf0`, which is after the world is entered.
/// benilla's does not. `Connected` flips us `InWorld` a whole server round-trip before the self
/// descriptor streams in ([`crate::ui_unit`]'s own comment measures that gap in *seconds*), and
/// `feed_units` — the only writer of the `"player"` token — is gated on that descriptor existing.
/// So until this existed, every addon's file scope ran in a VM where `UnitName("player")` was
/// **nil**, which is a state a real session cannot present. It is the same argument, at the same
/// line, as the `set_realm_name` above it — and this is the more load-bearing half.
///
/// **The failure it fixes is silent, which is why it survived every instrument.** The director
/// installed Bagnon, opened their bags, and got a window with a title, a gold line and **no bag
/// slots at all**. `Bagnon_Core/core/Utility.lua:5` opens `local currentPlayer =
/// UnitName("player")`, and every one of Bagnon's "am I looking at a cached snapshot of some OTHER
/// character?" predicates is `currentPlayer ~= frame.player`. With `currentPlayer` nil, Bagnon
/// concluded the live player's own bags belonged to somebody else, took every bag size from
/// Bagnon_Forever's (empty) offline cache instead of `GetContainerNumSlots`, created zero item
/// buttons — and raised nothing, so `loaded`, `session` and the UI probe all scored it a pass.
/// Reproduced both ways in [`bagnon_render_tests`].
///
/// **What is filled is what the roster actually knows**: name, race, class, gender and level, all
/// a round-trip ahead of the descriptor (the same fact [`crate::char_select::Roster::pending_entry`]
/// already exploits for the streamers). Health and power are deliberately left at zero — those are
/// the descriptor's to say, they land within the second, and inventing them would be a different
/// lie from the one being fixed.
pub(crate) fn seat_from_roster(
    roster: &crate::char_select::Roster,
) -> Option<benilla_ui::script::UnitState> {
    let row = roster.pending_row()?;
    let race = crate::ui_unit::race_names(row.race);
    let class = crate::ui_unit::class_names(row.class);
    let sex = roster_sex(row.gender);
    Some(benilla_ui::script::UnitState {
        // **`exists` is FALSE, and the level is 0 — the reference's answers, byte-verified**.
        // `UnitExists("player")` `0x515fb0` has no fast path: it resolves the
        // token, and the resolver reads the GUID out of the OBJECT (`0x515994`), not out of
        // `[mgr+0xc0]` — so with no object it holds `0:0`. The roster fallback `0x491900` then
        // bails on a zero GUID at `0x4e80aa je` *before* it fetches the active player, so the
        // `0 == 0` that would otherwise answer "yes, that's me" is never reached: `0x516001
        // lua_pushnil`. `UnitLevel 0x517fc0` likewise carries no `"player"` compare at all and
        // reaches its total-miss arm `0x51813e push 0; push 0` — the **number 0**, not nil. The
        // two misses are deliberately asymmetric and a client that modelled "no data" uniformly
        // would be wrong on one of them.
        //
        // This seat is no longer what makes the player *answerable* — the record above is — so it
        // no longer has to claim a unit exists in order to deliver a name. What it still carries
        // is the handful of fields the corpus reads at file scope that are neither the record's
        // nor the descriptor's to say yet, the faction side below chief among them.
        exists: false,
        name: Some(row.name.clone()),
        race: race.map(|(n, _)| n.to_string()),
        race_file: race.map(|(_, f)| f.to_string()),
        class: class.map(|(n, _)| n.to_string()),
        class_file: class.map(|(_, f)| f.to_string()),
        // The wire's 0/1 on `UnitSex`'s 2/3 scale — `ui_unit::snapshot`'s own mapping.
        sex,
        is_player: true,
        player_controlled: true,
        // Nil here is not "no faction", it is a state a player character cannot be in, and
        // AceDB-2.0 concatenates it at file scope — see [`crate::ui_unit::race_faction_group`].
        faction_group: crate::ui_unit::race_faction_group(row.race).map(str::to_string),
        ..Default::default()
    })
}

/// The character the loaded AddOn enable state belongs to, remembered so the shutdown write goes
/// back to the file it came from — the roster's pick can be gone by then.
#[derive(Resource, Default)]
pub(crate) struct AddOnIdentity(pub(crate) Option<(String, String)>);

/// **The world latch — `[0xb4b424]`, ours**.
///
/// `PLAYER_LEAVING_WORLD` has one fire site (`0x490b4d`) and three callers (2238), and the
/// reference does not keep them apart by asking each one "is this your occasion?". It keeps a
/// single byte: **set at `0x4908ce` inside the world-enter cascade `0x4908c0`, cleared at
/// `0x490a8d` by the fire itself**, so whichever caller runs first in a world fires and the rest
/// are no-ops until the next world entry re-arms it.
///
/// We have two producers — [`shutdown_ui_state`]'s tail and
/// [`crate::ui_unit::fire_leaving_world_on_worldport`] — and 2238 gated the tail on a *predicate*
/// instead (was the client `InWorld`), which is the shape this replaces. The predicate was wrong
/// in the one window that matters: a cross-map worldport despawns our avatar and re-streams it
/// (`net::apply`'s `tag_self_player`) **without leaving `InWorld`**, so a quit on the loading
/// screen fired the event a second time where the reference fires none. Enumerating occasions
/// found three of four; the latch is the mechanism, and the fourth falls out of it.
///
/// Reading the enumeration back through the latch, every case is one rule:
///
/// | occasion | latch | fires |
/// |---|---|---|
/// | `/logout`, disconnect | armed since the login | yes — and our avatar is already despawned by then, which is why a "is there a player?" predicate would have broken this, the commonest root |
/// | cross-map worldport | armed | yes, and spends it |
/// | quit *during* that port's loading screen | spent, not yet re-armed | **no** — the port's fire was this departure's |
/// | quit at the character screen | spent by the logout that got there | **no** |
/// | in-world `/reload` | armed | yes; the rebuild re-arms, so a second `/reload` fires again |
#[derive(Resource, Default)]
pub(crate) struct LeavingWorldArmed(bool);

impl LeavingWorldArmed {
    /// Arm it — a world began. Idempotent, like the reference's `mov byte [0xb4b424],1`.
    pub(crate) fn arm(&mut self) {
        self.0 = true;
    }

    /// Take the one-shot: `true` at most once per world, `0x490a8d`'s clear folded in.
    pub(crate) fn spend(&mut self) -> bool {
        std::mem::take(&mut self.0)
    }

    /// Read without taking — tests only, so the arm and the spend can be asserted separately.
    #[cfg(test)]
    pub(crate) fn is_armed(&self) -> bool {
        self.0
    }
}

/// Arm the latch when the local player's entity appears — the reference's `0x5deb60` entry into
/// the world-enter cascade, which is the one a fresh login and a worldport's new-world create
/// block both take (`0x5deb49 call 0x468570` / `0x5deb50 jne 0x5deb6a`).
///
/// `Added<SelfPlayer>` rather than a hook inside `net::apply::tag_self_player`, so the net layer
/// keeps knowing nothing about the UI's event law; the edge is the same one, read from the other
/// side.
pub(crate) fn arm_leaving_world_on_self_create(
    created: Query<(), Added<crate::net::SelfPlayer>>,
    mut armed: ResMut<LeavingWorldArmed>,
) {
    if !created.is_empty() {
        armed.arm();
    }
}

/// **The UI shutdown, in the reference's own order** — `0x490bd0`, whose ordered tail is:
///
/// > `PLAYER_LEAVING_WORLD` (273) → **`PLAYER_LOGOUT`** (271, `0x490c2a`) → `layout-cache.txt` →
/// > **the flat saved file** (`0x490c7e`) → **the per-addon files** (`0x490c83`) → `AddOns.txt`
/// > (`0x490c88`) → destroy the frame-script owner (`0x490c97`) → nil all 216 C bindings out of
/// > `_G` (`0x490cba` → `0x490ce0`)
///
/// **The last step is not "destroy the Lua state"** (2277). `0x490c97` is the frame-script owner's
/// scalar-deleting destructor — the widget tree and the native virtual-font registry — and the Lua
/// state outlives it. The state is closed and re-opened at `0x703b80`, which reaches `InitLua` by a
/// **tail-`jmp`** (`0x703b8e`) rather than a call, which is exactly why a call-census misses it.
/// Its three callers are `UI_Init 0x48fbf0` (at its *head*), `ShutdownGame 0x491180`, and the glue
/// builder `0x46a7b0` — so a logout/login cycle runs through four distinct states, and the glue
/// screen gets its own.
///
/// **`PLAYER_LOGOUT` fires before any write, and that is the point**: it is an addon's last chance
/// to mutate a saved global, so a handler that stores "where I left off" runs while the write is
/// still ahead of it. Firing it after would make the event useless and the bug invisible.
///
/// One function, called from every root, because the steps are ordered *against each other* —
/// three independent Bevy systems on one state edge cannot express that, and until this landed the
/// flat write and the `AddOns.txt` write were exactly that.
///
/// **The layout cache is step three, and it was missing until B353.** The quote above has always
/// carried `layout-cache.txt`; the body skipped it, because [`crate::ui_layout`] had hung its own
/// saver off `OnExit(InWorld)` instead. Two things follow from being outside the tail, and the
/// bug report is both of them: a `/reload` never leaves `InWorld` ([`run_pending_reload`] calls
/// this function and the rebuild back to back), so that saver never ran on the root a player uses
/// most; and on the roots where it did run it was racing [`end_ui_session`]'s VM replacement on
/// the same unordered edge — measured (bevy 0.18) to move with nothing but registration
/// positions. In the tail it is neither: one call, ahead of the replacement, on every root.
/// (It is also the one step that keeps a writer *outside* this function: a debounced
/// crash-save, which the reference has not got and which the next paragraph is not about.)
///
/// **There is no autosave**, deliberately: the reference has none (and
/// `ds:0xb4b3f4` has three references image-wide). These are a handful of scalars a player toggles
/// a few times a session, and every file is written whole from the live globals.
pub(crate) fn shutdown_ui_state(
    script: &mut UiScript,
    identity: Option<&(String, String)>,
    leaving_world: bool,
) {
    // **`PLAYER_LEAVING_WORLD` is the one step of this tail that is conditional** (2238/2239).
    // The reference reaches the fire through `0x490c20 call 0x490a80` and gets there past two
    // independent tests: `0x490bd0`'s own active-player guard (`0x490bee call 0x468550` /
    // `0x490bf3 or eax,edx` / `0x490bf5 je 0x490c25`, whose taken side skips exactly that one
    // instruction and lands on the `PLAYER_LOGOUT` block) and then `0x490a80`'s world latch.
    // `PLAYER_LOGOUT` at `0x490c2a` is outside both and fires on every root.
    //
    // `leaving_world` is [`LeavingWorldArmed::spend`]'s answer, and the caller spends it rather
    // than this function so that the one-shot is visibly *taken* at the root — there are two
    // producers, and a latch read in the callee would let a second root take it again.
    if leaving_world {
        script.fire_event("PLAYER_LEAVING_WORLD", vec![]);
    }
    script.fire_event("PLAYER_LOGOUT", vec![]);
    crate::ui_layout::save_now(script, identity);
    crate::ui_saved::save(script);
    addons::save_addon_variables(script, identity);
    addons::save_enable_state(script, identity);
}

/// `OnExit(InWorld)`: a `/logout` back to the glue, or a disconnect — two of the reference's five
/// roots — and, with the writes done, **the end of this session's Lua state**.
///
/// The reference's own shutdown destroys the frames and nils the 216 bindings (`0x490bd0`, after
/// `AddOns.txt`) and then replaces the state a little later, in `ShutdownGame` (`0x491231`) —
/// **not in `0x490bd0`'s tail, as this doc used to say** (corrected 2226: nothing in `0x490bd0`'s
/// call graph touches `ds:0xceef74`). Ours ends by replacing it with a fresh boot VM
/// ([`install_boot_vm`]), which is the
/// same guarantee expressed the way our two-phase load wants it — the character screen still needs
/// a font-object registry for the shared glyph atlas, and the next login needs somewhere to load
/// onto. What matters is that **no frame, no global and no addon upvalue crosses this edge**: that
/// is what makes the next login a real login instead of a re-entry into the
/// previous character's UI.
///
/// Exclusive rather than a `NonSendMut` system because it both drops and installs a `NonSend`, and
/// because the shutdown writes must be ordered against each other — see [`shutdown_ui_state`].
pub(crate) fn end_ui_session(world: &mut World) {
    // A VM still parked (the load never ran) goes back into the world first, so the tail below
    // runs against the same slot it always did (1978).
    unpark_boot_vm(world);
    // An armed-but-unrun entry load ([`PendingEntryUiLoad`]) means this session never built an
    // in-game UI: there are no globals to save and no addon state to write, and running the
    // shutdown tail against the boot VM would overwrite the real files with that emptiness.
    let ui_never_loaded = world.remove_resource::<PendingEntryUiLoad>().is_some();
    let identity = world
        .get_resource::<AddOnIdentity>()
        .and_then(|id| id.0.clone());
    // Spent before the tail, at the root, because there are two producers (2239): this edge and
    // the worldport's. Whichever reaches a departure first owns it.
    let leaving_world = world
        .get_resource_mut::<LeavingWorldArmed>()
        .is_some_and(|mut l| l.spend());
    if !ui_never_loaded {
        if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
            shutdown_ui_state(&mut script, identity.as_ref(), leaving_world);
        }
    }
    // The CVar bridge: the dying VM's table folds into the persist state — after
    // the shutdown events above (a `PLAYER_LOGOUT` handler may `SetCVar`, and in the reference
    // that lands in an engine-side store that survives), before the VM is replaced. The next
    // VM's registration seeds from what this writes ([`crate::cvars`]'s saved base).
    crate::cvars::fold_dying_vm_cvars(world);
    // The chat cache, on the same terms and for the same reason: it composes the
    // player's file out of the DYING VM, and `/reload` never crosses the `OnExit(InWorld)` edge
    // its flush used to hang on — so a window moved in the last second before a reload was
    // written nowhere and re-read stale from disk.
    crate::ui_chat::settings::fold_dying_vm_chat_cache(world);
    world.insert_resource(AddOnIdentity(None));

    // **Everything the host is holding that came OUT of the dying VM goes with it.** A change memo
    // handles itself — it is keyed on [`benilla_ui::script::UiScript::session`] ([`VmMemo`]) — but
    // these are plain values other systems read through `Res<…>`, with no VM in hand to key
    // against, so the edge clears them. Each is a fact about a frame tree that is about to stop
    // existing: a hovered frame id, the minimap's extracted hole, and a payload the cursor is
    // carrying.
    //
    // **[`UiClock`] is NOT one of them, and used to be**. It reads like a fact
    // about the dying VM — it is the `GetTime` leg of the conversion pair — but `GetTime` is the
    // reference's OS tick count, not a per-VM clock, so zeroing it here restarted every cooldown
    // and aura conversion at the character screen. [`seed_vm_clock`] writes the pair for the VM
    // that replaces this one, moments below.
    //
    // The two input latches were cleared by `char_select`'s logout and disconnect handlers, one
    // copy each. They belong here: the reason they need clearing is that `feed_ui_input` stops
    // running outside `InWorld`, which is this edge and nothing to do with *why* we left it.
    if let Some(mut hover) = world.get_resource_mut::<PlayerUiHover>() {
        hover.0 = None;
    }
    if let Some(mut keys) = world.get_resource_mut::<UiKeyboardCapture>() {
        keys.typing = false;
        keys.arrows_fall_through = false;
        keys.consumed.clear();
    }
    if let Some(mut held) = world.get_resource_mut::<CursorPayloadHeld>() {
        *held = CursorPayloadHeld::default();
    }
    if let Some(mut minimap) = world.get_resource_mut::<crate::minimap::MinimapWidget>() {
        minimap.0 = None;
    }

    install_boot_vm(world);
}

/// A `ReloadUI()` waiting to run — set when [`crate::ui_logout`] drains
/// [`benilla_ui::script::SessionRequest::ReloadUi`], consumed by [`run_pending_reload`] at the
/// top of the next frame's `Update`.
///
/// A flag rather than an immediate call for the reference's own reason (`ds:0xb4b3f4`, its only
/// writer `0x491380` and its only reader the per-frame callback `0x495590`): the VM that queued
/// the request must not be mid-call when it is destroyed. Our drain already runs outside any VM
/// dispatch, but the flag keeps the whole rebuild at one point in the frame — before the input
/// pass — instead of wherever the drain happens to sit.
#[derive(Resource, Default)]
pub(crate) struct ReloadUiPending(pub(crate) bool);

/// Run a pending `ReloadUI()`: the reference's teardown/rebuild pair (`0x495664 call 0x490bd0`,
/// `0x495669 call 0x48fbf0`), which for us is [`end_ui_session`] then
/// [`load_ingame_ui_on_world_entry`] — the same two functions the logout/login edges run, called
/// back to back without leaving the world.
///
/// Everything that makes a login correct makes the reload correct **by construction**: the
/// shutdown tail fires `PLAYER_LEAVING_WORLD`/`PLAYER_LOGOUT` and writes the four files (so a
/// `DisableAddOn` staged in the dying VM reaches `AddOns.txt` before the rebuild reads it), the
/// rebuild is a real login's load (fresh file scope, saved variables, `VARIABLES_LOADED`,
/// `PLAYER_LOGIN`), and every host memory keyed on the VM's identity ([`VmMemo`])
/// expires with the old session id. `PLAYER_ENTERING_WORLD` refires from [`crate::ui_unit`]'s
/// feed once it notices the new VM, with the self descriptor already present — the reference's
/// own ordering, where the event follows the rebuild.
///
/// **In-world only.** At the glue there is no in-game UI to rebuild and no identity to load
/// addons under; the reference's own gate (`0x494a50(0xa)`) refuses there too. Dropped with a log
/// line rather than deferred — a reload asked for at the character screen answers nothing.
pub(crate) fn run_pending_reload(world: &mut World) {
    if !std::mem::take(&mut world.resource_mut::<ReloadUiPending>().0) {
        return;
    }
    let in_world = *world
        .resource::<State<crate::char_select::ClientState>>()
        .get()
        == crate::char_select::ClientState::InWorld;
    if !in_world {
        info!("ui_script: ReloadUI outside the world — dropped");
        return;
    }
    info!("ui_script: ReloadUI — ending the UI session and building a new one");
    end_ui_session(world);
    load_ingame_ui_on_world_entry(world);
}

/// `AppExit`: quitting the client — the quit / application-exit roots. Reads the message rather
/// than a state edge because a quit from in-world never leaves `InWorld`.
///
/// **The root that can reach the tail with nothing to leave** — a quit from the character screen,
/// and a quit on a worldport's loading screen, where the port already fired. Both fall out of
/// [`LeavingWorldArmed`] rather than being tested for (2239): the first has never been re-armed,
/// the second was spent minutes ago by the port.
pub(crate) fn shutdown_on_exit(
    script: Option<NonSendMut<UiScript>>,
    id: Res<AddOnIdentity>,
    pending_entry: Option<Res<PendingEntryUiLoad>>,
    mut armed: ResMut<LeavingWorldArmed>,
    mut exits: MessageReader<AppExit>,
) {
    if exits.read().next().is_none() {
        return;
    }
    // Same guard as [`end_ui_session`]: a quit inside the entry-load window has no UI to save.
    if pending_entry.is_some() {
        return;
    }
    let leaving_world = armed.spend();
    if let Some(mut script) = script {
        shutdown_ui_state(&mut script, id.0.as_ref(), leaving_world);
    }
}

/// The UI-init sequence's ordered tail, once every file — ours and every addon's — has loaded:
/// the saved-variables chunk and `VARIABLES_LOADED`, then `PLAYER_LOGIN`.
///
/// **The order is the reference's**. Inside `UI_Init 0x48fbf0`, in straight-line address order:
///
/// | | |
/// |---|---|
/// | `0x4900a3` → `0x51f600` | load every non-LoadOnDemand addon — each fires its own **`ADDON_LOADED`** (429, `0x51f5ad`) |
/// | `0x4900b2` → `0x4913b0` | read the flat saved file, fire **`VARIABLES_LOADED`** (430) |
/// | `0x490168` → `0x4908c0` | the world-enter cascade: **`PLAYER_LOGIN`** (`0x49094b`, `0x10e`) then **`PLAYER_ENTERING_WORLD`** (`0x490965`, `0x110`) — **gated, see below** |
///
/// So every non-LoD addon's `ADDON_LOADED` precedes `VARIABLES_LOADED`, which precedes
/// `PLAYER_LOGIN`. It is one function rather than three inline calls because that sequence is the
/// mechanism — an addon restores state on `ADDON_LOADED` and expects the saved chunk to have run,
/// and a window that waits on `PLAYER_LOGIN` expects both — so it is worth being able to assert.
///
/// **The third row does not run on a fresh login, and this doc used to say it did** (corrected
/// 2226; the claim came from 1348 and was re-checked against the bytes). The call
/// at `0x490168` is gated three instructions earlier:
///
/// ```text
/// 49015f:  call 0x468550    ; the active player's GUID from [[0xb41414]+0xc0/+0xc4]
/// 490164:  or   eax,edx
/// 490166:  je   0x49016d    ; TAKEN on a fresh login — the pair is still 0/0
/// 490168:  call 0x4908c0
/// ```
///
/// The object-manager ctor zeroed that pair at `0x4015f3`, three calls earlier, so on a first
/// entry the branch is taken and `0x4908c0` is reached only later, from a data-dependent episode
/// once an object-update packet has populated the GUID. On a `/reload` — where the player is
/// already in the world and the pair is set — it does fire from here, which is the case 1348 was
/// looking at when it generalised.
///
/// **This is a fidelity question we have NOT settled, deliberately left open rather than quietly
/// resolved.** Our `finish_ui_init` fires the cascade inside the load, which matches the reload
/// case and not the fresh-login one; whether to split those is its own change with its own
/// measurements, and 2226 (which was about the VM's identity, not the cascade's timing) is not it.
/// Which of `0x4908c0`'s two data-dependent entries fires first on a real login is still open, so
/// the target shape is not yet known.
///
/// **`PLAYER_LOGIN` is the conditional one; `PLAYER_ENTERING_WORLD` is not.** The cascade fires
/// the former only when `[0xb4e260]` is set, and only the FrameXML-loader path sets it, clearing
/// it immediately after — so it means "the UI came up". That is once per **UI build**, which since
/// 1290 is once per world entry here too: this runs from
/// [`load_ingame_ui_on_world_entry`], on the same edge that built the tree.
/// `PLAYER_ENTERING_WORLD` keeps its own per-entry latch in [`crate::ui_unit`] and still lands
/// after this, since it waits on the self descriptor arriving over the wire.
/// Test-only since 2125: the production edge is [`finish_ui_load_with`], because the chat-cache
/// restore has to sit inside it. A test that only wants the tail keeps the plain shape.
#[cfg(test)]
pub(crate) fn finish_ui_load(script: &mut UiScript) {
    finish_ui_load_with(script, |_| {}, |_| {});
}

/// [`finish_ui_load`] with the one step that has to land **between** `VARIABLES_LOADED` and
/// `PLAYER_LOGIN`: the chat-cache restore's `UPDATE_CHAT_WINDOWS` + `UPDATE_CHAT_COLOR` burst
/// (correcting 2119's placement).
///
/// The reference's login is `FrameXML → addons + ADDON_LOADED (0x4900a3) → VARIABLES_LOADED
/// (0x4900b2) → the chat-cache reader's burst (0x4900d6, firing synchronously through the
/// register-or-fire-now trampoline 0x498a20) → PLAYER_LOGIN (0x490959) → PLAYER_ENTERING_WORLD
/// (0x49096a)`. 2119 put the restore ahead of `VARIABLES_LOADED`, which is one step too early:
/// an addon reading its chat colours out of a `VARIABLES_LOADED` handler would see the file's
/// values where the reference shows it the boot ones.
///
/// `host_settings` is the earlier of the two seams — **between the saved-variables chunk and
/// `VARIABLES_LOADED`** — where the settings benilla keeps in `config.toml` rather than in that
/// file are pushed into the VM (the rationale for the exact seat is on
/// [`crate::ui_saved::load_saved_variables`]).
///
/// Callbacks rather than a split trio because the budget re-arm and the two fires are one edge,
/// and a caller that forgets a middle step should not be able to compile.
pub(crate) fn finish_ui_load_with(
    script: &mut UiScript,
    host_settings: impl FnOnce(&mut UiScript),
    between: impl FnOnce(&mut UiScript),
) {
    // Still the load edge, so still bounded (1306) — re-armed because the walk's last addon left
    // an arbitrary amount on the counter, and the saved-variables chunk plus every PLAYER_LOGIN
    // handler deserve the full allowance. The entry edge disarms after this returns.
    script.set_instruction_budget(addons::LOAD_INSTRUCTION_BUDGET);
    crate::ui_saved::load_saved_variables(script, host_settings);
    between(script);
    script.fire_event("PLAYER_LOGIN", vec![]);
}

/// Execute the real `Interface\FrameXML\GlobalStrings.lua` off the patch chain into the VM —
/// the reference boots FrameXML with exactly this file FIRST, and it is the source of every
/// localized string global the UI reads (the cast-fail display's whole message set).
/// Loaded before our own `assets/ui` files, matching the reference order. Failures are LOUD:
/// a silently missing GlobalStrings once suppressed every red error line (the 0427 fold's
/// absent-key face is faithful data suppression — but only when the file actually loaded).
fn load_global_strings(world: &mut World, script: &UiScript) {
    let Some(assets) = world.get_resource::<benilla_assets::WorldAssets>() else {
        warn!("ui_script: no patch chain — GlobalStrings absent, error lines will be empty");
        return;
    };
    let bytes = {
        let mut chain = assets.chain.lock_recover();
        chain.read_file("Interface\\FrameXML\\GlobalStrings.lua")
    };
    let src = match bytes {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => {
            error!("ui_script: GlobalStrings.lua read failed — error lines will be empty: {e:#}");
            return;
        }
    };
    if let Err(e) = script.run(&src) {
        error!("ui_script: GlobalStrings.lua failed to run: {e}");
        return;
    }
    // The sentinel: the exact lookup the cast-fail drain performs. If this misses, every
    // message would silently vanish — turn that failure mode into a diagnosable line. Presence
    // only, not the enUS text: a non-enUS install is still a loaded GlobalStrings.
    let sentinel: Option<String> = script.lua().globals().get("SPELL_FAILED_NO_AMMO").ok();
    match sentinel {
        Some(s) if !s.is_empty() => info!("ui_script: GlobalStrings loaded"),
        other => {
            error!("ui_script: GlobalStrings sentinel missing ({other:?}) — error lines broken")
        }
    }
}

/// Execute the reference's own **emote token table** into the VM (`EMOTE87_TOKEN = "SIT"`, …) —
/// the second half of the emote slash grammar. The *aliases* are in
/// `GlobalStrings.lua` above (`EMOTE87_CMD1 = "/sit"`), but the alias → `EmotesText.Name` mapping
/// lives in `ChatFrame.lua`: the reference's chat **code**, which benilla replaces in Rust. So we
/// take that file's **data** and none of its code — only whole lines matching
/// `EMOTE<digits>_TOKEN = "<UPPER>";` ([`is_emote_token_line`]) are executed, and the file's ~2400
/// lines of frame logic never run. Reading the shipped table beats transcribing 170 tokens into
/// Rust: a transcription can be wrong, and a hand-kept alias list is exactly what left 61 real
/// commands (`/lol`, `/hi`, `/ty`, …) unresolvable before 0881.
fn load_emote_tokens(world: &mut World, script: &UiScript) {
    let Some(assets) = world.get_resource::<benilla_assets::WorldAssets>() else {
        return; // already WARNed by load_global_strings
    };
    let bytes = {
        let mut chain = assets.chain.lock_recover();
        chain.read_file("Interface\\FrameXML\\ChatFrame.lua")
    };
    let src = match bytes {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => {
            error!("ui_script: ChatFrame.lua read failed — emote commands will be dead: {e:#}");
            return;
        }
    };
    let table: Vec<&str> = src
        .lines()
        .map(str::trim)
        .filter(|l| is_emote_token_line(l))
        .collect();
    let count = table.len();
    if let Err(e) = script.run(&table.join("\n")) {
        error!("ui_script: emote token table failed to run: {e}");
        return;
    }
    // The sentinel is the command this whole seam exists for: EMOTE87 is `/sit`.
    let sentinel: Option<String> = script.lua().globals().get("EMOTE87_TOKEN").ok();
    match sentinel.as_deref() {
        Some("SIT") => info!("ui_script: {count} emote tokens loaded"),
        other => error!(
            "ui_script: emote token sentinel is {other:?}, not \"SIT\" ({count} lines) — \
             emote slash commands are broken"
        ),
    }
}

/// Is this line one of `ChatFrame.lua`'s emote-token assignments — `EMOTE<digits>_TOKEN = "<NAME>";`
/// with `NAME` in `[A-Z0-9_]`? The whole-line shape is the filter that makes running the matched
/// lines equivalent to reading data (no calls, no expressions, no side effects).
pub(crate) fn is_emote_token_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("EMOTE") else {
        return false;
    };
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let Some(rest) = rest[digits..].strip_prefix("_TOKEN = \"") else {
        return false;
    };
    let Some(name) = rest.strip_suffix("\";") else {
        return false;
    };
    digits > 0
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The entry edge parks the boot VM (1978): between the arm and the load there is no VM in
    /// the world for any feed to push into, and the session end puts a never-loaded one back.
    #[test]
    fn the_entry_edge_parks_the_boot_vm_and_the_session_end_returns_it() {
        let mut world = World::new();
        let boot = UiScript::new().unwrap();
        let session = boot.session();
        world.insert_non_send_resource(boot);
        arm_entry_ui_load(&mut world);
        assert!(
            world.get_non_send_resource::<UiScript>().is_none(),
            "no VM in the deferral window"
        );
        assert!(world.get_resource::<PendingEntryUiLoad>().is_some());
        assert!(world.get_non_send_resource::<ParkedBootVm>().is_some());
        // Left the world before the load ran: the VM comes back, untouched.
        unpark_boot_vm(&mut world);
        let vm = world
            .get_non_send_resource::<UiScript>()
            .expect("the parked VM is back");
        assert_eq!(vm.session(), session, "the same VM, no session moved");
        assert!(world.get_non_send_resource::<ParkedBootVm>().is_none());
    }

    /// **The world entry builds its own VM** — the reference's reset at
    /// `0x48fe97`, inside `UI_Init` itself, and what
    /// [`benilla_ui::script::UiScript::session`]'s contract already claimed.
    ///
    /// Against the old shape this reads *"the in-game UI loaded onto the character screen's own
    /// Lua state, so the session id never moved across the entry edge"* — and a session that
    /// never moves is a [`super::VmMemo`] that never resets, which is how a login one-shot fired
    /// into the frameless glue VM (1348) stayed spent for the rest of the login.
    #[test]
    fn the_world_entry_builds_its_own_vm_rather_than_adopting_the_glue_phases() {
        let mut world = World::new();
        let glue = UiScript::new().unwrap();
        let glue_session = glue.session();
        world.insert_non_send_resource(glue);

        mint_entry_vm(&mut world);

        let entry = world
            .get_non_send_resource::<UiScript>()
            .expect("the entry load has a VM to run on");
        assert_ne!(
            entry.session(),
            glue_session,
            "the entry VM is a new session — every VmMemo resets here, so a one-shot spent \
             against the frameless glue VM is spent again against the one that has an interface"
        );
    }

    /// **The login one-shot class, at the mechanism** — the reason the session has
    /// to move, expressed without any particular feed in it.
    ///
    /// This is the shape of every one of them: a feed reaches the VM in the window between the
    /// wire turning in-world and the deferred entry load, fires its event at a state with no
    /// frames to hear it (`0x703f50` drops an event with no listener), and its [`super::VmMemo`]
    /// records "told". Before 2226 the entry load then ran onto that same VM, the memo still
    /// matched, and the telling never happened again for the rest of the login — 1348 on the unit
    /// feed, B376 on the guild MOTD, the cinematic and `feed_chat`.
    ///
    /// Against the old shape the third assertion reads *"still spent"*, and it does so for every
    /// feed at once, gated or not.
    #[test]
    fn a_one_shot_spent_against_the_character_screens_vm_is_spent_again_after_the_entry() {
        let mut world = World::new();
        world.insert_non_send_resource(UiScript::new().expect("a glue VM"));
        let mut told: super::super::VmMemo<bool> = Default::default();

        assert!(
            told.claim(world.non_send_resource::<UiScript>()),
            "the window: a feed fires its login one-shot at the frameless character-screen VM"
        );
        assert!(
            !told.claim(world.non_send_resource::<UiScript>()),
            "and the memo has it as told — which is correct for THAT VM"
        );

        mint_entry_vm(&mut world);

        assert!(
            told.claim(world.non_send_resource::<UiScript>()),
            "the VM the in-game UI loads onto has never been told, so the feed tells it again — \
             no run condition, no ordering edge, and nothing the feed had to know about"
        );
    }

    /// **A rebuilt VM inherits the running `GetTime()` clock, and so does the conversion pair**
    /// — the bug the director reported as *cooldowns are lost on relog*.
    ///
    /// The reference's `GetTime` is `KERNEL32!GetTickCount` × 0.001 (`0x515ea0` → `0x42c010` →
    /// `0x42b790`), an OS clock that cannot restart — which is why stock `Cooldown.lua` can gate
    /// on `start > 0`. Ours lives in the VM, and since 1290/1291 the VM dies at the character
    /// screen; before this, `end_ui_session` zeroed [`UiClock`] too, so on the way back in every
    /// cooldown that was already running converted to a NEGATIVE start,
    /// `CooldownFrame_SetTimer` took its `else` branch and hid the sweep — while the store, and
    /// therefore the cast validator, still held the cooldown.
    ///
    /// Against the old shape the two assertions read *"the new VM's GetTime clock restarted at 0
    /// instead of the process's 90 s"* and *"a cooldown armed before the rebuild derived start
    /// -30000 ms; stock Cooldown.lua hides anything but `start > 0`"*.
    #[test]
    fn a_rebuilt_vm_inherits_the_running_gettime_clock() {
        use std::time::{Duration, Instant};

        let mut world = World::new();
        world.init_resource::<UiClock>();
        // The process has been up 90 s — the clock `Time<Real>` keeps and the one the dying VM
        // is on, which are the same clock.
        let startup = Instant::now() - Duration::from_secs(90);
        let mut time = Time::<bevy::time::Real>::new(startup);
        // `Time<Real>` measures `elapsed` from its FIRST update, not from `startup` — so the app's
        // own clock is "since frame one", and the test's has to be seeded the same way.
        time.update_with_instant(startup);
        time.update_with_instant(startup + Duration::from_secs(90));
        world.insert_resource(time);
        let mut dying = UiScript::new().unwrap();
        dying.set_now(90.0);
        world.insert_non_send_resource(dying);
        // The session never loaded an in-game UI, so the shutdown tail (which writes the player's
        // four files) is skipped: this test is about the VM swap and nothing else.
        world.init_resource::<PendingEntryUiLoad>();

        end_ui_session(&mut world);

        let vm = world
            .get_non_send_resource::<UiScript>()
            .expect("the session end installs a fresh boot VM");
        assert_eq!(
            vm.now(),
            90.0,
            "the new VM's GetTime clock restarted at {} instead of the process's 90 s",
            vm.now()
        );
        // …and the pair every `Instant`→`GetTime` conversion runs through moved with it: a
        // 10-minute cooldown armed 30 s before the rebuild still derives its real start.
        let clock = world.resource::<UiClock>();
        let armed = crate::spell::cooldowns::CooldownInfo {
            start: startup + Duration::from_secs(60),
            remaining_ms: 570_000,
            duration_ms: 600_000,
            enabled: true,
        };
        let triple = armed
            .ui_triple(clock.anchor, clock.ui_now)
            .expect("a running cooldown pushes a triple");
        assert_eq!(
            triple,
            (60_000, 600_000, true),
            "a cooldown armed before the rebuild derived start {} ms; stock Cooldown.lua hides \
             anything but `start > 0`",
            triple.0
        );
    }
}
