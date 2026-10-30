//! The app half of `benilla_ui::script::macros`: the icon chooser's catalog, the files under
//! `benilla-config/macros/`, the runner, and the bound spell the action bar reads. The reference
//! saves them to `macros-cache.txt`, account-data sections 4 and 5, which the server keeps a copy
//! of ([`crate::account_data`]).

use bevy::prelude::*;

use benilla_protocol::messages::ACTION_KIND_MACRO;
use benilla_ui::script::{ScriptValue, UiScript, MAX_MACROS};

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
    /// The moves of a character file of the builds that numbered macros by position, read into
    /// this VM and waiting for this login's bar ([`renumber_buttons`]).
    renumber: crate::ui_script::VmMemo<Vec<(u32, u32)>>,
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
        use crate::lanes::LaneApp;
        // The first read is the interface load's ([`register_macro_readers`]).
        app.init_resource::<MacroFiles>()
            .add_systems(
                Update,
                // Every frame, in or out of world, so a `/logout` never strands an edit; after
                // the script tick that dirtied the table.
                save_dirty_macros.in_set(crate::ui_script::UiDrain),
            )
            .add_exclusive_lane(
                "macro arrivals",
                peek_arrived_macros,
                fire_arrived_macros,
                arrived_macros_reads,
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

/// The macro reader's registration for each scope's section (`0x4f0a70`: `0x5afe50` at
/// `0x4f0a7c` and `0x4f0a8d`), which the interface load makes once per VM (`0x4900d1`): a scope
/// is read now on a settled section (a `/reload`), else once its section settles
/// ([`fire_arrived_macros`]), and again at every later settling. No character (a capture) reads
/// nothing, as the per-character file needs one.
pub(crate) fn register_macro_readers(world: &mut World, script: &mut UiScript) {
    let Some(id) = world
        .get_resource::<crate::glue_session::roster::Roster>()
        .and_then(identity)
    else {
        return;
    };
    {
        let Some(mut files) = world.get_resource_mut::<MacroFiles>() else {
            return;
        };
        files.account = crate::local_state::macros_account_path();
        files.character = crate::local_state::macros_character_path(&id.0, &id.1);
        *files.identity.get(script) = Some(id);
    }
    for (scope, section) in crate::account_data::MACROS.into_iter().enumerate() {
        let runs = world
            .get_resource::<crate::account_data::AccountData>()
            .is_none_or(|data| data.register(section));
        // A held scope is read when it settles, and nothing is saved over its file meanwhile
        // ([`save_dirty_macros`]).
        world.resource_mut::<MacroFiles>().held.get(script)[scope] = !runs;
        if runs {
            read_macro_scope(world, script, scope == 1);
        }
    }
}

/// `LoadMacroFiles` (`0x4f0600`) for one scope: its file merged into the table as it stands, as
/// the reader never empties one, then the order rebuilt, which signals `UPDATE_MACROS` (string at
/// `0x852460`; `0x4f0a50` → `0x4f0d30`), and every record derived (`0x4f0a55` → `0x4f0ca0`), with
/// the host lent, so a listener's calls apply inside their verbs and the derivation reads the book
/// as it stands.
fn read_macro_scope(world: &mut World, script: &mut UiScript, per_character: bool) {
    let path = {
        let Some(mut files) = world.get_resource_mut::<MacroFiles>() else {
            return;
        };
        files.held.get(script)[usize::from(per_character)] = false;
        if per_character {
            files.character.clone()
        } else {
            files.account.clone()
        }
    };
    let records = match &path {
        None => Vec::new(),
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => store::parse(&text, per_character),
            // Absent is the first run, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                warn!("ui_macro: reading {}: {e}", path.display());
                Vec::new()
            }
        },
    };
    let moves = if per_character {
        store::legacy_character_words(&records)
    } else {
        Vec::new()
    };
    let mut state = script.macros();
    state.merge_scope(per_character, records);
    info!(
        "ui_macro: {} {} macros read, {} account + {} character in all",
        state.count(per_character),
        if per_character {
            "character"
        } else {
            "account"
        },
        state.count(false),
        state.count(true),
    );
    // The records the table held keep their bound spells (`[rec+0x564]`, `[rec+0x568]`), as the
    // merge only adds records; the new ones are derived below.
    let bound = script.macro_bindings();
    script.set_macros(state);
    script.set_macro_bindings(bound);
    // A file of the older numbering: its buttons move once the bar is loaded, now when it is.
    if !moves.is_empty() {
        if let Some(mut files) = world.get_resource_mut::<MacroFiles>() {
            *files.renumber.get(script) = moves;
        }
    }
    let moved = renumber_buttons(world, script);
    crate::script_calls::lent_by_world(world, script, |script| {
        for id in moved {
            script.fire_event(
                "ACTIONBAR_SLOT_CHANGED",
                vec![ScriptValue::Int(i64::from(id))],
            );
        }
        script.fire_event("UPDATE_MACROS", vec![]);
        script.derive_macro_bindings(|_, _| true);
    });
}

/// The macro section that settled first, its reader still owed, with its settling's stamp.
fn arrived_macros(world: &World) -> Option<(usize, crate::lanes::Stamp)> {
    let data = world.get_resource::<crate::account_data::AccountData>()?;
    crate::account_data::MACROS
        .into_iter()
        .filter_map(|section| Some((section, data.owed_at(section)?)))
        .min_by_key(|(_, at)| *at)
}

/// A macro section's settling, the head of its lane: the reader runs where the handler that
/// settled it ran (`0x5afaca` in the digests' check, `0x5afd19` in a download), in the packets'
/// order with the other lanes' events.
fn peek_arrived_macros(world: &World) -> Option<crate::lanes::Stamp> {
    arrived_macros(world).map(|(_, at)| at)
}

/// The settled section's scope read into the VM its reader registered with, as the firing of the
/// settling's stamp.
fn fire_arrived_macros(In(until): In<crate::lanes::Until>, world: &mut World) {
    let Some((section, at)) = arrived_macros(world).filter(|(_, at)| until.admits(*at)) else {
        return;
    };
    // No VM: the settling waits for one.
    let Some(mut script) = world.remove_non_send_resource::<UiScript>() else {
        return;
    };
    world
        .resource::<crate::account_data::AccountData>()
        .take_owed(section);
    // Registered in this VM, so its files are the ones the registration named.
    let registered = world
        .get_resource_mut::<MacroFiles>()
        .is_some_and(|mut files| files.identity.get(&script).is_some());
    if registered {
        let per_character = section == crate::account_data::MACROS[1];
        crate::lanes::firing(at, || read_macro_scope(world, &mut script, per_character));
    }
    world.insert_non_send_resource(script);
}

/// What the macros lane's firing touches beside the call host and its own lane: the files. The
/// move off the older numbering writes the bar through the host ([`renumber_buttons`]).
fn arrived_macros_reads(world: &mut World) -> bevy::ecs::query::FilteredAccessSet {
    fn read(_: Option<ResMut<MacroFiles>>) {}
    crate::lanes::system_access(world, read)
}

/// Every macro settling fired alone, in order, for a test that drives the lane without the
/// others.
#[cfg(test)]
pub(super) fn reread_arrived_macros(world: &mut World) {
    while let Some(head) = peek_arrived_macros(world) {
        world
            .run_system_cached_with(fire_arrived_macros, crate::lanes::Until::all())
            .expect("the macros lane fires");
        if peek_arrived_macros(world) == Some(head) {
            break;
        }
    }
}

/// The one-time move of this character's macro buttons off the numbering of the builds that held a
/// macro's position in its slot, a move with no reference fact. They placed a character macro as
/// `0x40000000 | (18 + n)`, n its place in the character file in id order (19 to 36), where a slot
/// holds the macro's id now, and this build reads those words as account macros 19 to 36. The
/// reader parks the moves of such a build's file ([`MacroFiles::renumber`], from
/// `store::legacy_character_words`), and they run once this login's bar is loaded: at the read
/// when it already is, else at our player's arrival, after its load of the staged words
/// ([`renumber_parked_buttons`]), as the game entry empties the bar and `SMSG_ACTION_BUTTONS`
/// only stages it.
///
/// Each button goes out as a placement's does, through the host
/// ([`crate::script_calls::ScriptCallHost::engine_bar_writes`]). A button whose old place holds no
/// macro is cleared the same way: its word would read as an account macro of that id, and misbind
/// once one takes it. Only then is the file rewritten in this build's numbering, so the next load
/// finds no old ids and the buttons move once, and a move that never ran is tried again at the
/// next load. The 1-based slots written, in order.
fn renumber_buttons(world: &mut World, script: &mut UiScript) -> Vec<u32> {
    let (moves, path) = {
        let Some(mut files) = world.get_resource_mut::<MacroFiles>() else {
            return Vec::new();
        };
        let moves = std::mem::take(files.renumber.get(script));
        (moves, files.character.clone())
    };
    if moves.is_empty() {
        return Vec::new();
    }
    let mut host =
        bevy::ecs::system::SystemState::<Option<crate::script_calls::ScriptCallHost>>::new(world);
    let moved = host
        .get_mut(world)
        .as_mut()
        .and_then(|host| host.engine_bar_writes(script, |buttons| legacy_writes(buttons, &moves)));
    host.apply(world);
    let Some(moved) = moved else {
        // The bar is not loaded yet: the moves wait for the arrival's load.
        if let Some(mut files) = world.get_resource_mut::<MacroFiles>() {
            *files.renumber.get(script) = moves;
        }
        return Vec::new();
    };
    info!(
        "ui_macro: {} character macro button(s) moved to macro ids or cleared",
        moved.len()
    );
    if let Some(path) = path {
        let text = store::write(script.macros().saved(true).into_iter());
        if let Err(e) = crate::local_state::write_atomic(&path, &text) {
            warn!("ui_macro: renumbering {}: {e}", path.display());
        }
    }
    moved
}

/// The bar writes [`renumber_buttons`] makes, `(1-based id, word)`: each button holding an old word
/// onto its macro's, and each other word of the old numbering cleared.
fn legacy_writes(
    buttons: &[benilla_protocol::messages::ActionButton],
    moves: &[(u32, u32)],
) -> Vec<(u32, u32)> {
    buttons
        .iter()
        .filter_map(|b| {
            let id = u32::from(b.slot) + 1;
            let word = (u32::from(b.kind) << 24) | b.action;
            let old_numbering = b.kind == ACTION_KIND_MACRO
                && (MAX_MACROS as u32 + 1..=2 * MAX_MACROS as u32).contains(&b.action);
            match moves.iter().find(|(old, _)| *old == word) {
                Some((_, new)) => Some((id, *new)),
                None => old_numbering.then_some((id, 0)),
            }
        })
        .collect()
}

/// Our player's arrival's leg after its slot pass, which loaded this login's bar: the buttons the
/// macro reader parked moved ([`renumber_buttons`]), each signalled as a placement is.
pub(crate) fn renumber_parked_buttons(world: &mut World) {
    let Some(mut script) = world.remove_non_send_resource::<UiScript>() else {
        return;
    };
    let moved = renumber_buttons(world, &mut script);
    if !moved.is_empty() {
        crate::script_calls::lent_by_world(world, &mut script, |script| {
            for id in moved {
                script.fire_event(
                    "ACTIONBAR_SLOT_CHANGED",
                    vec![ScriptValue::Int(i64::from(id))],
                );
            }
        });
    }
    world.insert_non_send_resource(script);
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
        // Moves parked for the bar keep the character file in the older numbering: the moves
        // write the whole table once they run ([`renumber_buttons`]), this edit with it. An edit
        // made meanwhile, in a session that ends before the arrival, is lost, never the buttons.
        let parked = !files.renumber.get(script).is_empty();
        let state = script.macros();
        for (path, per_character) in [(&files.account, false), (&files.character, true)] {
            // A held scope's section is not written (`0x4f0c7f`, `0x4f0c90` → `0x5afee0`).
            if held[usize::from(per_character)] || (per_character && parked) {
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
/// (`0x5deacc`); each scope's read runs its own ([`read_macro_scope`]). A learned spell derives nothing, so a
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
    script.set_macros(benilla_ui::script::MacroState::from_scopes(
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
