//! The app half of `benilla_ui::script::macros`: the icon chooser's catalog, the files under
//! `benilla-config/macros/`, the runner, and the bound spell the action bar reads. 1.12 macros are
//! client state only: no opcode carries them, and the reference saves them to `macros-cache.txt`.

use bevy::prelude::*;

use benilla_ui::script::{MacroState, ScriptValue, UiScript};

use crate::char_select::InWorldGated;
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

/// Macro index to its bound spell, the reference's `[rec+0x564]` (read by `0x4e5ba0`), which an
/// action-bar macro slot's state reads through. A missing entry names no macro, which `0x4e5030`
/// greys, unlike a macro that casts nothing.
#[derive(Resource, Default)]
pub(crate) struct MacroBoundSpells(pub(crate) std::collections::HashMap<u32, BoundSpell>);

pub(crate) use run::BoundSpell;

pub(crate) struct UiMacroPlugin;

impl Plugin for UiMacroPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MacroFiles>()
            .init_resource::<MacroBoundSpells>()
            .add_systems(
                Update,
                (
                    // Once per VM, in `Update`: it needs both the patch chain and the VM, and each
                    // login builds a fresh VM.
                    load_icon_catalog.in_set(crate::ui_script::UiFeed),
                    // Before the action feeds read it (they run in `UnitFeed`), so a macro edited
                    // this frame reports its new spell's cooldown the same frame.
                    rebind_macro_spells
                        .in_set(crate::ui_script::UiFeed)
                        .before(crate::ui_unit::UnitFeed),
                    // In-world only: the per-character file needs the character.
                    load_macros
                        .in_set(crate::ui_script::UiFeed)
                        .in_set(InWorldGated),
                    // Every frame, in or out of world, so a `/logout` never strands an edit; after
                    // the script tick that dirtied the table.
                    save_dirty_macros.after(crate::ui_script::UiInput),
                ),
            );
    }
}

/// Build the icon chooser's list once per VM: the `Spell_` and `Ability_` files under
/// `Interface\Icons\` ([`benilla_formats::load_macro_icons`]). A failed load leaves it empty, and
/// `MacroPopupOkayButton_Update` then keeps OKAY disabled.
fn load_icon_catalog(
    script: Option<NonSendMut<UiScript>>,
    assets: Option<Res<WorldAssets>>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    let (Some(mut script), Some(assets)) = (script, assets) else {
        return;
    };
    if !seeded.claim(&script) {
        return;
    }
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_macro_icons(&mut chain)
    };
    match loaded {
        Ok(icons) => {
            info!("ui_macro: {} macro icons in the chooser", icons.len());
            script.set_macro_icons(icons);
        }
        Err(e) => error!("ui_macro: SpellIcon.dbc failed — the icon chooser is empty: {e:#}"),
    }
}

/// `(realm, character)` off the roster's login pick, keying the per-character files.
pub(crate) fn identity(roster: &crate::char_select::Roster) -> Option<(String, String)> {
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
    script: Option<NonSendMut<UiScript>>,
    roster: Res<crate::char_select::Roster>,
    mut files: ResMut<MacroFiles>,
) {
    let Some(mut script) = script else { return };
    let Some(id) = identity(&roster) else { return };
    if files.identity.get(&script).as_ref() == Some(&id) {
        return; // already loaded into this VM
    }
    let (realm, character) = (&id.0, &id.1);
    files.account = crate::local_state::macros_account_path();
    files.character = crate::local_state::macros_character_path(realm, character);
    *files.identity.get(&script) = Some(id.clone());

    let read = |path: &Option<std::path::PathBuf>| -> Vec<benilla_ui::script::MacroView> {
        let Some(path) = path else { return Vec::new() };
        match std::fs::read_to_string(path) {
            Ok(text) => store::parse(&text),
            // Absent is the first run, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                warn!("ui_macro: reading {}: {e}", path.display());
                Vec::new()
            }
        }
    };
    let state = MacroState {
        account: read(&files.account),
        character: read(&files.character),
    };
    info!(
        "ui_macro: {} account + {} character macros for {character} on {realm}",
        state.account.len(),
        state.character.len()
    );
    script.set_macros(state);
    // `UPDATE_MACROS` (string at `0x852460`): the macro frame redraws on it, as after an edit.
    script.fire_event("UPDATE_MACROS", vec![]);
}

/// Persist on the engine's dirty edge, and fire `UPDATE_MACROS`.
fn save_dirty_macros(script: Option<NonSendMut<UiScript>>, files: Res<MacroFiles>) {
    let Some(mut script) = script else { return };
    if !script.take_macros_dirty() {
        return;
    }
    let state = script.macros();
    // Fire first: the redraw must not depend on the write landing.
    script.fire_event("UPDATE_MACROS", vec![]);
    for (path, macros) in [
        (&files.account, &state.account),
        (&files.character, &state.character),
    ] {
        let Some(path) = path else { continue };
        if let Err(e) = crate::local_state::write_atomic(path, &store::write(macros)) {
            warn!("ui_macro: saving {}: {e}", path.display());
        }
    }
}

/// Recompute every macro's bound spell when the macro table or the spell book changes: gated on
/// the engine's macro generation and on change detection over the action store.
fn rebind_macro_spells(
    script: Option<NonSendMut<UiScript>>,
    actions: Res<crate::ui_action::PlayerActions>,
    table: Option<Res<crate::ui_chat::commands::SlashCommands>>,
    mut bound: ResMut<MacroBoundSpells>,
    mut last_generation: Local<crate::ui_script::VmMemo<Option<u64>>>,
) {
    let (Some(script), Some(table)) = (script, table) else {
        return;
    };
    // Per VM: a fresh VM restarts its generation at 0, which a bare memo would gate off for good.
    let last_generation = last_generation.get(&script);
    let generation = script.macros_generation();
    if *last_generation == Some(generation) && !actions.is_changed() {
        return;
    }
    *last_generation = Some(generation);

    let (macros, book) = (script.macros(), script.spellbook());
    let mut fresh = std::collections::HashMap::new();
    for index in 1..=(benilla_ui::script::MAX_MACROS as u32 * 2) {
        let Some(m) = macros.get(index as usize) else {
            continue;
        };
        fresh.insert(index, run::bound_spell(&table, &m.body, &book));
    }
    if fresh != bound.0 {
        let spells = fresh
            .values()
            .filter(|b| matches!(b, BoundSpell::Spell(_)))
            .count();
        debug!(
            "ui_macro: {} macro(s), {spells} bound to a spell",
            fresh.len()
        );
        bound.0 = fresh;
    }
}

/// The event each macro line is fired as: id `0x188`, whose registry slot `0xbe17b8` is written
/// once, at `0x51b4ff`, with the string at `0x852470`.
const EXECUTE_CHAT_LINE: &str = "EXECUTE_CHAT_LINE";

/// Run a macro by its 1-based index; returns whether anything ran. Like the reference's runner
/// (`0x4f14e0`, reached only from `UseAction` at `0x4e6098`), it fires `EXECUTE_CHAT_LINE` per
/// non-empty line and nothing else: the stock `ChatFrame1` sends each through its edit box
/// (`ChatFrame.lua:1343`), and an addon registered for the event sees every macro line.
pub(crate) fn run_macro(script: &mut UiScript, index: u32) -> bool {
    let Some(body) = script.macros().get(index as usize).map(|m| m.body.clone()) else {
        return false;
    };
    let lines: Vec<String> = run::macro_lines(&body).map(str::to_string).collect();
    if lines.is_empty() {
        return false;
    }
    debug!("ui_macro: running macro {index} ({} line(s))", lines.len());
    for line in lines {
        script.fire_event(EXECUTE_CHAT_LINE, vec![ScriptValue::Str(line)]);
    }
    true
}
