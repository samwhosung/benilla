//! The app half of `benilla_ui::script::macros`: the icon chooser's catalog, the files under
//! `benilla-config/macros/`, the runner, and the bound spell the action bar reads. The reference
//! saves them to `macros-cache.txt`, account-data sections 4 and 5, which the server keeps a copy
//! of ([`crate::account_data`]).

use bevy::prelude::*;

use benilla_protocol::messages::ACTION_KIND_MACRO;
use benilla_ui::script::{MacroState, ScriptValue, UiScript, MAX_MACROS};

use crate::client_state::InWorldGated;
use benilla_assets::{LockRecover, WorldAssets};

pub(crate) mod run;
mod store;
#[cfg(test)]
mod tests;

/// The files this session's macros live in, the per-character one keyed by realm and name as the
/// reference's folders are. `None` is session-only (a capture, or no install): nothing is written.
#[derive(Resource, Default)]
pub(crate) struct MacroFiles {
    account: Option<std::path::PathBuf>,
    character: Option<std::path::PathBuf>,
    /// The `(realm, character)` the files were loaded for, per VM: a relog meets a fresh VM with an
    /// empty macro table, even as the same character.
    identity: crate::ui_script::VmMemo<Option<(String, String)>>,
}

/// Macro id to its bound spell, the reference's `[rec+0x564]` (read by `0x4e5ba0`), which an
/// action-bar macro slot's state reads through. A missing entry names no macro, which `0x4e5030`
/// greys, unlike a macro that casts nothing.
#[derive(Resource, Default)]
pub(crate) struct MacroBoundSpells(pub(crate) std::collections::HashMap<u32, Bound>);

pub(crate) use run::{Bound, BoundSpell};

/// The bound spells' derivation, which the action-state feed reads after ([`crate::ui_action`]).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct MacroBindings;

pub(crate) struct UiMacroPlugin;

impl Plugin for UiMacroPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MacroFiles>()
            .init_resource::<MacroBoundSpells>()
            .add_systems(
                Update,
                (
                    // After the book's feed, which a derivation reads; the action-state feed
                    // follows it ([`crate::ui_action`]).
                    rebind_macro_spells
                        .in_set(crate::ui_unit::UnitFeed)
                        .in_set(MacroBindings)
                        .after(crate::ui_spellbook::BookFeed)
                        .after(crate::ui_pet_book::feed_pet_book),
                    // In-world only: the per-character file needs the character. Ahead of the
                    // rank pass, which rewrites the bar's words too, as the one-time move of a
                    // character's old buttons does.
                    load_macros
                        .in_set(crate::ui_script::UiFeed)
                        .in_set(InWorldGated)
                        .before(crate::ui_action::normalize_action_ranks),
                    // Every frame, in or out of world, so a `/logout` never strands an edit; after
                    // the script tick that dirtied the table.
                    save_dirty_macros.in_set(crate::ui_script::UiDrain),
                ),
            );
    }
}

/// Give the world-entry VM what lists the icon chooser, over the patch chain and the install's
/// loose `Interface\Icons\` folders ([`benilla_formats::load_macro_icons`]). The VM runs it on first use: only the in-game state
/// registers the macro verbs (`RegisterMacroFunctions` `0x4f1b90`, from `0x490297`), so the glue
/// and boot VMs get none. Without the archives (no install) it lists the loose folders alone; an
/// empty list leaves `MacroPopupOkayButton_Update` keeping OKAY disabled.
pub(crate) fn install_icon_source(world: &World, script: &mut UiScript) {
    let chain = world
        .get_resource::<WorldAssets>()
        .map(|assets| assets.chain.clone());
    script.set_macro_icon_source(icon_source(chain, benilla_formats::wow_data()));
}

/// The icon chooser's lister over `chain` and the `Data` folder `data`.
fn icon_source(
    chain: Option<std::sync::Arc<std::sync::Mutex<benilla_formats::Chain>>>,
    data: Option<std::path::PathBuf>,
) -> Box<dyn Fn() -> Vec<String>> {
    Box::new(move || {
        let start = std::time::Instant::now();
        let icons = match &chain {
            Some(chain) => {
                benilla_formats::load_macro_icons(Some(&chain.lock_recover()), data.as_deref())
            }
            None => benilla_formats::load_macro_icons(None, data.as_deref()),
        };
        info!(
            "ui_macro: {} macro icons in the chooser, listed in {:.0} ms",
            icons.len(),
            start.elapsed().as_secs_f32() * 1000.0
        );
        icons
    })
}

/// `(realm, character)` off the roster's login pick, keying the per-character files.
pub(crate) fn identity(roster: &crate::glue_session::roster::Roster) -> Option<(String, String)> {
    let guid = roster.pending_pick?;
    let name = roster.chars.iter().find(|c| c.guid == guid)?.name.clone();
    let realm = roster
        .realm
        .as_ref()
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "Realm".into());
    Some((realm, name))
}

/// Seed the engine's macro table from disk, once per character per VM.
fn load_macros(
    // The bar's verbs, whose slots the call host's own bar verbs write too: an engine turn.
    mut vm: crate::script_calls::VmWith<crate::ui_action::ActionBarSteps<'static>>,
    roster: Res<crate::glue_session::roster::Roster>,
    mut files: ResMut<MacroFiles>,
    data: Option<Res<crate::account_data::AccountData>>,
) {
    vm.feed(|script, mut bar, fires| {
        let Some(script) = script else { return };
        let Some(id) = identity(&roster) else { return };
        // A macro section the server's file replaced is read again, as the reference's readers
        // (`0x4f0a70`) run once it lands (`0x5afd19`); the first load takes what came before it.
        let arrived = data.as_deref().is_some_and(|data| {
            crate::account_data::MACROS
                .iter()
                .fold(false, |any, &section| data.take_arrived(section) | any)
        });
        if !arrived && files.identity.get(script).as_ref() == Some(&id) {
            return; // already loaded into this VM
        }
        let (realm, character) = (&id.0, &id.1);
        files.account = crate::local_state::macros_account_path();
        files.character = crate::local_state::macros_character_path(realm, character);
        *files.identity.get(script) = Some(id.clone());

        let read = |path: &Option<std::path::PathBuf>,
                    per_character: bool|
         -> Vec<benilla_ui::script::MacroView> {
            let Some(path) = path else { return Vec::new() };
            match std::fs::read_to_string(path) {
                Ok(text) => store::parse(&text, per_character),
                // Absent is the first run, not a failure.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => {
                    warn!("ui_macro: reading {}: {e}", path.display());
                    Vec::new()
                }
            }
        };
        let character_records = read(&files.character, true);
        let moves = store::legacy_character_words(&character_records);
        let state = MacroState::from_scopes(read(&files.account, false), character_records);
        info!(
            "ui_macro: {} account + {} character macros for {character} on {realm}",
            state.count(false),
            state.count(true)
        );
        if !moves.is_empty() {
            migrate_character_bar(&mut bar, &state, &files, &moves);
        }
        script.set_macros(state);
        // `UPDATE_MACROS` (string at `0x852460`): the macro frame redraws on it, as after an edit.
        fires.post("UPDATE_MACROS", vec![]);
    })
}

/// The one-time move of this character's macro buttons off the numbering of the builds that held a
/// macro's position in its slot. They placed a character macro as `0x40000000 | (18 + n)`, n its
/// place in the character file in id order (19 to 36), where a slot holds the macro's id now, and
/// this build reads those words as account macros 19 to 36. `moves` pairs each such word with the
/// word of the same macro under its id (`store::legacy_character_words`), and runs only for a
/// character file of those builds, whose ids sit below the character scope's base.
///
/// The file goes first, rewritten in this build's numbering, so the next load finds no old ids and
/// the buttons move once: afterwards a word in 19 to 36 can be an account macro's. If the write
/// fails nothing moves and the next load tries again. Each button then goes out as a placement's
/// does ([`crate::ui_action::ActionBarSteps::set`]): `CMSG_SET_ACTION_BUTTON` sent and the slot
/// stored. A button whose old place holds no macro is cleared the same way: its
/// word would read as an account macro of that id, and misbind once one takes it.
fn migrate_character_bar(
    bar: &mut crate::ui_action::ActionBarSteps,
    state: &MacroState,
    files: &MacroFiles,
    moves: &[(u32, u32)],
) {
    let Some(path) = &files.character else { return };
    let text = store::write(state.saved(true).into_iter());
    if let Err(e) = crate::local_state::write_atomic(path, &text) {
        warn!("ui_macro: renumbering {}: {e}", path.display());
        return;
    }
    let mut writes: Vec<(u32, u32)> = bar
        .buttons()
        .filter_map(|b| {
            let slot = b.slot;
            let word = (u32::from(b.kind) << 24) | b.action;
            let old_numbering = b.kind == ACTION_KIND_MACRO
                && (MAX_MACROS as u32 + 1..=2 * MAX_MACROS as u32).contains(&b.action);
            match moves.iter().find(|(old, _)| *old == word) {
                Some((_, new)) => Some((u32::from(slot) + 1, *new)),
                None => old_numbering.then_some((u32::from(slot) + 1, 0)),
            }
        })
        .collect();
    writes.sort_unstable();
    info!(
        "ui_macro: {} character macro button(s) moved to macro ids or cleared",
        writes.len()
    );
    for (id, packed) in writes {
        bar.set(id, packed);
    }
}

/// Persist on the engine's dirty edge. The verbs signal `UPDATE_MACROS` themselves, inside their
/// calls.
fn save_dirty_macros(
    mut hosted_vm: crate::script_calls::HostedVm,
    files: Res<MacroFiles>,
    data: Option<Res<crate::account_data::AccountData>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else { return };
        if !script.take_macros_dirty() {
            return;
        }
        let state = script.macros();
        for (path, per_character) in [(&files.account, false), (&files.character, true)] {
            let Some(path) = path else { continue };
            let text = store::write(state.saved(per_character).into_iter());
            if let Err(e) = crate::local_state::write_atomic(path, &text) {
                warn!("ui_macro: saving {}: {e}", path.display());
            }
        }
        // Both sections owed the shutdown's write, as `0x4f0c60` writes both (`0x4f0c7f`,
        // `0x4f0c90`).
        if let Some(data) = &data {
            data.saved(&crate::account_data::MACROS);
        }
    })
}

/// What [`rebind_macro_spells`] last saw, per VM: a fresh VM restarts the table's counter at 0,
/// which a bare memo would gate off for good.
#[derive(Default)]
struct BindingsSeen {
    macros: Option<u64>,
    pet_book: Option<u32>,
}

/// Derive the bound spells the engine's verbs do not (they derive the record a create or an edit
/// touches inside the call, `0x4f123c`, `0x4f1313`), where the reference's other callers of
/// `0x4efe00` do, and nowhere else:
/// - every record after the macro files load (`0x4f0a55` into `0x4f0ca0`) and after our player's
///   arrival learns its initial spells (`0x5deacc`);
/// - at each pet spell list (`0x4bdae0` into `0x4f0ce0`), the same list again included, the
///   records that follow the pet's book: a `/cast` that did not resolve and any record whose
///   selector is the pet's (`0x4f0cfb`-`0x4f0d11`). The frame's lists derive once, after the
///   last, off the book the pet book's feed pushed.
///
/// A learned spell derives nothing, so a `/cast` of a spell not yet known stays -1 until its macro
/// is edited, loaded again or the player arrives again. A superseded rank rewrites the cached
/// spell of each macro on the bar that holds the old one (`0x4e5e60`'s macro arm, `0x4e5ec4`),
/// derived from nothing. Runs after the book's feed, so a derivation reads the book as it stands,
/// and copies the engine's records out for the action bar.
#[allow(clippy::type_complexity)] // the bar, the records and the pet book's sorts, as one param
fn rebind_macro_spells(
    // The bar, whose supersede queue this drains, and the records, which the call host's presses
    // and usable refresh read, so a turn: no Lua runs here.
    mut vm: crate::script_calls::VmWith<(
        ResMut<'static, crate::ui_action::PlayerActions>,
        Res<'static, crate::ui_action::ActionSlots>,
        ResMut<'static, MacroBoundSpells>,
        Option<Res<'static, crate::ui_pet::PetBar>>,
    )>,
    arrivals: Query<(), Added<crate::net::SelfPlayer>>,
    mut owed: Local<(bool, crate::ui_script::VmMemo<BindingsSeen>)>,
) {
    let (arrived, seen) = &mut *owed;
    *arrived |= !arrivals.is_empty();
    vm.turn(|script, (mut actions, slots, mut bound, pet_bar)| {
        // With no VM the supersede pairs wait for the first pass that can apply them.
        let Some(script) = script else {
            return;
        };
        let seen = seen.get(script);
        let generation = script.macros_generation();
        let book_sorts = pet_bar.map_or(0, |b| b.book_sorts);
        let table_moved = seen.macros != Some(generation);
        let pet_moved = seen.pet_book != Some(book_sorts);
        if !table_moved && !pet_moved && !*arrived && actions.superseded.is_empty() {
            return;
        }
        seen.macros = Some(generation);
        seen.pet_book = Some(book_sorts);
        // Both taken, so neither stays owed to the next pass.
        let (arrival, load) = (std::mem::take(arrived), script.take_macro_derives().loaded);
        let every = arrival || load;

        // The queue empties without marking the bar changed: the supersede marked it already.
        let superseded = std::mem::take(&mut actions.bypass_change_detection().superseded);
        if !superseded.is_empty() {
            // The macros the bar holds are the ones a supersede reaches (`0x4e5e70`-`0x4e5ece`).
            let on_bar: std::collections::HashSet<u32> = slots
                .buttons
                .values()
                .filter_map(|b| benilla_ui::script::macro_slot_id(b.kind, b.action))
                .collect();
            let mut records = script.macro_bindings();
            for (_, b) in records.iter_mut().filter(|(id, _)| on_bar.contains(id)) {
                for &(old, new) in &superseded {
                    if b.spell == old as i32 {
                        b.spell = new as i32;
                    }
                }
            }
            script.set_macro_bindings(records);
        }
        script.derive_macro_bindings(|_, b| every || (pet_moved && b.follows_the_pet_book()));

        let records = script.macro_bindings();
        let fresh: std::collections::HashMap<u32, Bound> = script
            .macros()
            .views()
            .map(|m| (m.id, records.get(&m.id).copied().unwrap_or_default().into()))
            .collect();
        if fresh != bound.0 {
            let spells = fresh
                .values()
                .filter(|b| matches!(b.spell, BoundSpell::Spell(_)))
                .count();
            debug!(
                "ui_macro: {} macro(s), {spells} bound to a spell",
                fresh.len()
            );
            bound.0 = fresh;
        }
    })
}

/// The event each macro line is fired as: id `0x188`, whose registry slot `0xbe17b8` is written
/// once, at `0x51b4ff`, with the string at `0x852470`.
const EXECUTE_CHAT_LINE: &str = "EXECUTE_CHAT_LINE";

/// Run a macro by its id; returns whether anything ran. Like the reference's runner
/// (`0x4f1460`, reached only from `UseAction` at `0x4e6098`), it fires `EXECUTE_CHAT_LINE` per
/// non-empty line and nothing else: the stock `ChatFrame1` sends each through its edit box
/// (`ChatFrame.lua:1343`), and an addon registered for the event sees every macro line.
pub(crate) fn run_macro(script: &mut UiScript, id: u32) -> bool {
    let Some(body) = script.macros().by_id(id).map(|m| m.body.clone()) else {
        return false;
    };
    let lines: Vec<String> = run::macro_lines(&body).map(str::to_string).collect();
    if lines.is_empty() {
        return false;
    }
    debug!("ui_macro: running macro {id} ({} line(s))", lines.len());
    for line in lines {
        script.fire_event(EXECUTE_CHAT_LINE, vec![ScriptValue::Str(line)]);
    }
    true
}
