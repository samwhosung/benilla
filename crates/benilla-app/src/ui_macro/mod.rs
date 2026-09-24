//! The **macros** feed (decision 0983) — the app half of `benilla_ui::script::macros`: the icon
//! chooser's catalog, persistence under `benilla-config/macros/`, and `UPDATE_MACROS`.
//!
//! Unlike every other window feed in this tree there is **no wire traffic here at all**. 1.12
//! macros are pure client state — no opcode carries them (vmangos has none), and the reference
//! client persists them to `WTF/…/macros-cache.txt` itself. So the engine owns the live table
//! (`benilla_ui::script::macros`' module doc says why) and this module owns its two ends:
//!
//! - **In** — the icon list (`SpellIcon.dbc`, filtered by the client's own `Spell_`/`Ability_`
//!   prefixes: [`benilla_formats::load_macro_icons`]) at startup, and the saved files at world
//!   entry.
//! - **Out** — a save whenever a script mutated the table ([`save_dirty_macros`]), plus the
//!   reference's own `UPDATE_MACROS` event on the same edge.
//!
//! The **runner** and the action-bar's **bound spell** live in [`run`]; the file format is
//! [`store`] (the reference's own, so a vanilla `macros-cache.txt` drops straight in).

use bevy::prelude::*;

use benilla_ui::script::{MacroState, ScriptValue, UiScript};

use crate::char_select::InWorldGated;
use benilla_assets::{LockRecover, WorldAssets};

pub(crate) mod run;
mod store;
#[cfg(test)]
mod tests;

/// Which files this session's macros live in — resolved once the character is known, since the
/// per-character tab is keyed by realm + name exactly as the reference's own folder tree is.
/// `None` on either path means "session-only": a hermetic capture, or no resolvable install
/// (`crate::local_state`'s law). Macros still work in memory; nothing is written.
#[derive(Resource, Default)]
pub(crate) struct MacroFiles {
    account: Option<std::path::PathBuf>,
    character: Option<std::path::PathBuf>,
    /// The `(realm, character)` the [`Self::character`] path was built for — the reload trigger
    /// when a `/logout` brings a different character back into the world. Session-keyed (1290),
    /// because the *same* character coming back still meets a fresh VM with an empty macro table.
    identity: crate::ui_script::VmMemo<Option<(String, String)>>,
}

/// Macro index → its bound spell — benilla's `[rec+0x564]` (read by the MACRO arm `0x4e5ba0`),
/// the field an action-bar MACRO slot's whole dynamic state reads through, in the
/// reference's own three values ([`BoundSpell`]). Stored, not derived at read time, for the
/// reference's own reason: it is a **field on the macro record**, recomputed when the macro (or
/// the book) changes, so the three per-frame action-bar systems pay a hash lookup instead of a
/// body re-parse each.
///
/// **One entry per macro that exists**, whatever it binds — absence means the index names no
/// macro, which is the usable compute's `0x4e5030` verdict and reads differently from a macro
/// that merely casts nothing (decision 1636: the former is grey, the latter is not).
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
                    // Once per **VM** (decision 1290), and in `Update` rather than `PostStartup`:
                    // this needs BOTH the patch chain and the VM, and a login builds a fresh VM
                    // whose icon chooser stays empty until it runs again. `PostStartup` was the
                    // old answer to the ordering half alone — after `AssetSet::Open` this ran
                    // before the VM existed and silently pushed no icon list at all — and a
                    // session-keyed claim answers both halves at once.
                    load_icon_catalog.in_set(crate::ui_script::UiFeed),
                    // Before the action feeds read it (they run in `UnitFeed`), so a macro edited
                    // this frame reports its new spell's cooldown the same frame.
                    rebind_macro_spells
                        .in_set(crate::ui_script::UiFeed)
                        .before(crate::ui_unit::UnitFeed),
                    // Load runs in-world only: the per-character file needs the character, and the
                    // roster only names it once a login is live. It self-gates on the identity, so
                    // a re-entry with a different character reloads and a re-entry with the same
                    // one is a no-op.
                    load_macros
                        .in_set(crate::ui_script::UiFeed)
                        .in_set(InWorldGated),
                    // The save edge is checked every frame, in or out of world: the macro window is
                    // `whileDead = 1` and reachable from the game menu, and a `/logout` must not
                    // strand an unsaved edit. After the tick that dirtied it — which also puts it
                    // after the load, in the feed phase.
                    save_dirty_macros.after(crate::ui_script::UiInput),
                ),
            );
    }
}

/// Build the icon chooser's list once at startup — `SpellIcon.dbc` filtered by the client's own
/// two prefixes ([`benilla_formats::load_macro_icons`], where the byte citations live). A failed
/// load leaves the list empty: the popup then shows no icons and `MacroPopupOkayButton_Update`
/// keeps OKAY disabled, which is a visible, diagnosable failure rather than a silent one.
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

/// Who we are, for the per-character file: `(realm, character)` off the roster's own login pick.
/// `None` until the roster and the pick agree — the load simply waits a frame. Shared with the
/// bindings load ([`crate::bindings`]), whose per-character file is keyed the same way (0997).
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

/// Seed the engine's macro table from disk, once per character per world entry.
fn load_macros(
    script: Option<NonSendMut<UiScript>>,
    roster: Res<crate::char_select::Roster>,
    mut files: ResMut<MacroFiles>,
) {
    let Some(mut script) = script else { return };
    let Some(id) = identity(&roster) else { return };
    if files.identity.get(&script).as_ref() == Some(&id) {
        return; // already loaded for this character, into the VM that is live now
    }
    let (realm, character) = (&id.0, &id.1);
    files.account = crate::local_state::macros_account_path();
    files.character = crate::local_state::macros_character_path(realm, character);
    *files.identity.get(&script) = Some(id.clone());

    let read = |path: &Option<std::path::PathBuf>| -> Vec<benilla_ui::script::MacroView> {
        let Some(path) = path else { return Vec::new() };
        match std::fs::read_to_string(path) {
            Ok(text) => store::parse(&text),
            // Absent is the normal first-run case, not a failure; anything else is worth a line.
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
    // The reference's own event for "the macro table changed" (`UPDATE_MACROS`, byte-verified
    // string at `0x452460`) — the frame redraws off it exactly as it does after an edit.
    script.fire_event("UPDATE_MACROS", vec![]);
}

/// Persist on the engine's dirty edge, and fire `UPDATE_MACROS`.
///
/// Deliberately **not** coalesced behind a timer the way `config.toml`'s save is (decision 0954
/// §3): a macro edit is a discrete, deliberate act (an OKAY click, a tab switch, a window close),
/// not a slider drag, and the file is a few hundred bytes. The dirty flag is already the
/// coalescer — a whole `MacroFrame_SaveMacro` + `MacroFrame_Update` round raises it once.
fn save_dirty_macros(script: Option<NonSendMut<UiScript>>, files: Res<MacroFiles>) {
    let Some(mut script) = script else { return };
    if !script.take_macros_dirty() {
        return;
    }
    let state = script.macros();
    // Fire first: the event is the UI's redraw trigger and must not depend on the write landing
    // (a read-only `benilla/` folder still gets a live macro list for the session).
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

/// Recompute every macro's bound spell when the macro table or the spell book moves ([`run`]'s
/// module doc for what "bound" means and where it is byte-verified). Change-gated on the engine's
/// macro generation plus Bevy's own change detection over the action store (the book derives from
/// its known-spell set), so a steady frame does nothing at all.
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
    // Session-keyed (1290): a fresh VM restarts its macro generation at 0, so a bare memo could
    // hold a *higher* number than the live VM will ever reach and gate this off for the session.
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

/// The engine event a macro line is delivered as — `0x188` in the reference's runtime event
/// registry, resolved to its name inside the binary (`0xbe1198 + 4*0x188` is written exactly once,
/// at `0x51b4ff`, with `0x852470` = these bytes).
const EXECUTE_CHAT_LINE: &str = "EXECUTE_CHAT_LINE";

/// Run a macro by its 1-based index — the action bar's MACRO arm (`crate::ui_action::drain`) and
/// nothing else today, which is also the reference's shape (`0x4f14e0`'s only caller is
/// `UseAction`'s core at `0x4e6098`). Returns whether anything ran.
///
/// **Each body line is FIRED AS AN EVENT, not handed to the drain directly** (0996). The reference's
/// runner names no Lua function and walks no command table — per non-empty line it fires
/// `FrameScript_SignalEvent(EXECUTE_CHAT_LINE, "%s", line)` and the Lua side does everything else,
/// which is exactly why a scan of `WoW.exe` finds no FrameXML function name and no chat-frame name.
/// benilla's ChatFrame1 registers it (ChatFrame.xml) and calls `SubmitChatInput`, so the line lands
/// in the same queue a typed line does — but through the reference's own door, which means **an
/// addon that registers `EXECUTE_CHAT_LINE` sees macro lines**, as it would in 1.12.
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
