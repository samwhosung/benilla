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
    /// Whether this VM's account and character scopes wait for their sections (4 and 5).
    held: crate::ui_script::VmMemo<[bool; 2]>,
}

pub(crate) use run::{Bound, BoundSpell};

/// Macro id to its bound spell, the reference's `[rec+0x564]` and `[rec+0x568]` (read by
/// `0x4e5ba0`), as the VM's records hold it when read: what an action-bar macro slot's state reads
/// through. A missing entry names no macro, which `0x4e5030` greys, unlike a macro that casts
/// nothing.
#[derive(Default)]
pub(crate) struct MacroBoundSpells(pub(crate) std::collections::HashMap<u32, Bound>);

impl MacroBoundSpells {
    /// The VM's records of the macros `ids` name, as they stand.
    pub(crate) fn of(script: &UiScript, ids: impl IntoIterator<Item = u32>) -> Self {
        Self(
            ids.into_iter()
                .filter_map(|id| Some((id, script.macro_binding(id)?.into())))
                .collect(),
        )
    }
}

pub(crate) struct UiMacroPlugin;

impl Plugin for UiMacroPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MacroFiles>().add_systems(
            Update,
            (
                // In-world only: the per-character file needs the character.
                load_macros
                    .in_set(crate::ui_script::UiFeed)
                    .in_set(InWorldGated),
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
    let mut loaded = false;
    vm.feed(|script, mut bar, fires| {
        let Some(script) = script else { return };
        let Some(id) = identity(&roster) else { return };
        // The reader (`0x4f0600`) registers once per VM for each section through `0x5afe50`
        // (`0x4f0a7c`, `0x4f0a8d`): a scope is read now on a settled section, else when it
        // settles, and again at every later settling.
        let owed = data.as_deref().map_or([false; 2], |data| {
            crate::account_data::MACROS.map(|s| data.take_owed(s))
        });
        let first = files.identity.get(script).as_ref() != Some(&id);
        let reads = if first {
            let (realm, character) = (&id.0, &id.1);
            files.account = crate::local_state::macros_account_path();
            files.character = crate::local_state::macros_character_path(realm, character);
            *files.identity.get(script) = Some(id.clone());
            let runs = data.as_deref().map_or([true; 2], |data| {
                crate::account_data::MACROS.map(|s| data.register(s))
            });
            // A held scope is read when it settles, and nothing is saved over its file meanwhile
            // ([`save_dirty_macros`]).
            *files.held.get(script) = runs.map(|r| !r);
            runs
        } else {
            owed
        };
        if !reads.contains(&true) {
            return;
        }
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
        // Each scope's records merged into the table as it stands (`0x4f0600` never empties one).
        let mut state = script.macros();
        if reads[0] {
            state.merge_scope(false, read(&files.account, false));
            files.held.get(script)[0] = false;
        }
        if reads[1] {
            let character_records = read(&files.character, true);
            let moves = store::legacy_character_words(&character_records);
            state.merge_scope(true, character_records);
            files.held.get(script)[1] = false;
            if !moves.is_empty() {
                for id in migrate_character_bar(&mut bar, &state, &files, &moves) {
                    // The VM's copy of the moved slot, the bar's word with nothing resolved:
                    // its getters resolve it at the call.
                    let slot = bar.button(id).map(|b| benilla_ui::script::ActionSlot {
                        kind: b.kind,
                        action: b.action,
                        ..Default::default()
                    });
                    script.set_action(id, slot);
                    fires.post(
                        "ACTIONBAR_SLOT_CHANGED",
                        vec![benilla_ui::script::ScriptValue::Int(i64::from(id))],
                    );
                }
            }
        }
        info!(
            "ui_macro: {} account + {} character macros for {} on {}",
            state.count(false),
            state.count(true),
            id.1,
            id.0
        );
        script.set_macros(state);
        // `UPDATE_MACROS` (string at `0x852460`): the order's rebuild signals it (`0x4f0a50` →
        // `0x4f0d30`), as after an edit.
        fires.post("UPDATE_MACROS", vec![]);
        loaded = true;
    });
    // Then every record derived (`0x4f0a55` → `0x4f0ca0`), with the host lent, so the derivation
    // reads the book as it stands.
    if loaded {
        vm.run(|script| {
            if let Some(script) = script {
                script.derive_macro_bindings(|_, _| true);
            }
        });
    }
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
/// word would read as an account macro of that id, and misbind once one takes it. Returns the
/// 1-based slots it wrote, in order.
fn migrate_character_bar(
    bar: &mut crate::ui_action::ActionBarSteps,
    state: &MacroState,
    files: &MacroFiles,
    moves: &[(u32, u32)],
) -> Vec<u32> {
    let Some(path) = &files.character else {
        return Vec::new();
    };
    let text = store::write(state.saved(true).into_iter());
    if let Err(e) = crate::local_state::write_atomic(path, &text) {
        warn!("ui_macro: renumbering {}: {e}", path.display());
        return Vec::new();
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
    for &(id, packed) in &writes {
        bar.set(id, packed);
    }
    writes.into_iter().map(|(id, _)| id).collect()
}

/// Persist on the engine's dirty edge. The verbs signal `UPDATE_MACROS` themselves, inside their
/// calls. Left at the frame: the file is read only at the next load, so no script sees when it
/// is written.
fn save_dirty_macros(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut files: ResMut<MacroFiles>,
    data: Option<Res<crate::account_data::AccountData>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else { return };
        // Not registered in this VM: no file of the player's was read to write over.
        if files.identity.get(script).is_none() || !script.take_macros_dirty() {
            return;
        }
        let held = *files.held.get(script);
        let state = script.macros();
        for (path, per_character) in [(&files.account, false), (&files.character, true)] {
            // A held scope's section is not written (`0x4f0c7f`, `0x4f0c90` → `0x5afee0`).
            if held[usize::from(per_character)] {
                continue;
            }
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

/// `DeriveAllBoundSpells 0x4f0ca0`: every record derived, with the host lent, so a derivation
/// reads the book as it stands. Our player's arrival runs it after its initial spells are learned
/// (`0x5deacc`); the load runs its own ([`load_macros`]). A learned spell derives nothing, so a
/// `/cast` of a spell not yet known stays -1 until its macro is edited, loaded again or the player
/// arrives again.
pub(crate) fn derive_every(mut vm: crate::script_calls::HostedVm) {
    vm.run(|script| {
        if let Some(script) = script {
            script.derive_macro_bindings(|_, _| true);
        }
    });
}

/// `DeriveUnresolvedBoundSpells 0x4f0ce0`, which each pet spell list runs after its book's sort
/// (`0x4bdae0`), the same list again included: the records that follow the pet's book, a `/cast`
/// that did not resolve and any record whose selector is the pet's (`0x4f0cfb`-`0x4f0d11`). Run
/// with the host lent on the notice lane, at the list's place
/// ([`crate::field_watch::FieldNotice::PetMacros`]).
pub(crate) fn derive_pet_followers(script: &mut UiScript) {
    script.derive_macro_bindings(|_, b| b.follows_the_pet_book());
}

/// Seat `binds` in the VM's macro table: account macros 1 to the highest id named, each bound as
/// given and the rest to nothing, for a test of the bar's macro slots.
#[cfg(test)]
pub(crate) fn bind_for_test(script: &mut UiScript, binds: &[(u32, Bound)]) {
    let top = binds.iter().map(|&(id, _)| id).max().unwrap_or(0);
    script.set_macros(MacroState::from_scopes(
        (1..=top)
            .map(|i| benilla_ui::script::MacroView {
                name: format!("m{i}"),
                ..Default::default()
            })
            .collect(),
        Vec::new(),
    ));
    script.set_macro_bindings(
        binds
            .iter()
            .map(|&(id, b)| {
                let spell = match b.spell {
                    BoundSpell::None => 0,
                    BoundSpell::Unresolved => -1,
                    BoundSpell::Spell(s) => s as i32,
                };
                (
                    id,
                    benilla_ui::script::MacroBinding {
                        spell,
                        pet_book: b.pet_book,
                    },
                )
            })
            .collect(),
    );
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
