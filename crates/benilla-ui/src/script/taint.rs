//! The execution context word `[0xceeac0]` as the protected-action gate `0x494a50` reads it.
//!
//! The reference stamps every Lua value with the context that made it. Copying a value whose stamp
//! is nonzero makes that stamp the current context while `[0xceeac4] > 0`; copying one whose stamp
//! is zero stamps the copy with the current context instead (`0x6f3c52`-`0x6f3c78`), which is why
//! taint is sticky. Context 0 is the client's own code; a script run by `RunScript` carries
//! `[0x83de24]`, the empty string's address (`0x48b9bb`), and an insecure addon's files and
//! `Bindings.xml` its name (`0x51f387`). So a function defined by one stays that one's whoever
//! calls it, and what a tainted function calls is tainted with it.
//!
//! benilla has no stamp on its values. [`current`] reads the call stack instead: the innermost
//! Lua frame whose function carries a context decides, which is the reference's answer wherever
//! the taint came from a function that is still running. A function carries a context when it was
//! [`tag`]ged at compile (a `RunScript` chunk, an XML handler or a binding body an insecure addon
//! loaded), or when its chunk is a file or an inline `<Script>` block of a registered insecure
//! addon (`Interface\AddOns\<Folder>\…`).
//!
//! Not modelled, each a refusal the stack scan can miss or make:
//! - a tainted value that is data (a string, table or number read from an addon's global and used
//!   by stock code);
//! - a tainted function that has returned, since only running frames are read;
//! - a closure created inside a `RunScript` chunk, an XML handler or a `loadstring` chunk, which
//!   carries its creator's context in the reference and is the client's here once the creator
//!   is off the stack;
//! - `SetScript` stamping the slot with the caller's context (`0x774a05`);
//! - a value copied under an addon context being restamped with it (`0x6f3c73`);
//! - the reset a script slot's own run does to the context (`0x7026ad`), so a stock handler an
//!   addon's call fires in place still reads as tainted;
//! - the rule that a binding body starts clean (`0x4b7aee`) is kept by `execute_binding` running
//!   only from Rust at the top of the stack (`bindings.rs:632`, `:647`), which the scan relies on;
//! - an event the app fires after the input event that caused it has returned runs with no input
//!   event up, where the reference fires it inside the key's `ExecuteBinding` with the binding flag
//!   still raised ([`super::protected`]). A queued call is applied under the events it was made
//!   under, which covers a macro's lines and whatever the application signals in place; the
//!   selection change of a script call (`PLAYER_TARGET_CHANGED`) and the refusals a press raises
//!   (`UI_ERROR_MESSAGE`, `UI_INFO_MESSAGE`) are signalled a step later, by the unit and action
//!   feeds, under the events noted for them. Every other event those feeds raise from a state edge
//!   (auras, health, the action bar, `AUTOFOLLOW_*`) and the selection a world click makes natively
//!   rather than by a call are signalled with none up.

use mlua::{Function, Lua, Table, Value};

use super::Model;

/// The registry table of tagged functions, keyed weakly by the function.
const REG_FN_CONTEXT: &str = "__benilla_fn_context";

/// A tagged function's value for a `RunScript` chunk; an addon's is its name.
const MACRO_TAG: i64 = 1;

/// Whose code is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Context {
    /// Context 0: the client's own code, stock FrameXML and benilla's layer.
    Client,
    /// `[0x83de24]`: a script `RunScript` ran, so a `/script` line or a macro's.
    Macro,
    /// An insecure addon's name (`[rec+0x14]`).
    Addon(String),
}

/// Make the registry table that [`tag`] fills.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let tags = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__mode", "k")?;
    tags.set_metatable(Some(meta))?;
    lua.set_named_registry_value(REG_FN_CONTEXT, tags)
}

/// Stamp `f` with the context it was made under. [`Context::Client`] is the default and stamps
/// nothing.
pub(crate) fn tag(lua: &Lua, f: &Function, context: &Context) {
    let value = match context {
        Context::Client => return,
        Context::Macro => Value::Integer(MACRO_TAG),
        Context::Addon(name) => match lua.create_string(name) {
            Ok(s) => Value::String(s),
            Err(_) => return,
        },
    };
    if let Ok(tags) = lua.named_registry_value::<Table>(REG_FN_CONTEXT) {
        let _ = tags.raw_set(f.clone(), value);
    }
}

/// [`tag`] with the context a load is under now ([`Model::exec_context`]): an insecure addon's
/// name while its files load, else nothing.
pub(crate) fn tag_for_load(lua: &Lua, f: &Function) {
    let context = lua
        .app_data_ref::<Model>()
        .and_then(|model| model.exec_context.borrow().clone());
    if let Some(name) = context {
        tag(lua, f, &Context::Addon(name));
    }
}

/// The addon a chunk name belongs to: `Interface\AddOns\<Folder>\<file>`, a file chunk's with its
/// `@` ([`super::addon_chunk_name`]) or an XML document's inline `<Script>` block's without
/// (`<path>:<Scripts>`, `0x871074`), and only while that addon is registered and insecure, since a
/// secure one runs under context 0 (`0x51f37e`-`0x51f387`). The registry row's own spelling of the
/// name is the context word. No code runs from a folder the registry lacks, so one is not an
/// addon's, and benilla's own layer, which names its chunks that way, stays the client's.
fn addon_of_chunk(model: &Model, source: &str) -> Option<String> {
    let path = source.strip_prefix('@').unwrap_or(source);
    let rest = path.strip_prefix("Interface\\AddOns\\")?;
    let folder = rest.split_once('\\')?.0;
    let row = model
        .addons
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(folder))?;
    (row.security != super::AddonSecurity::Secure).then(|| row.name.clone())
}

/// The context word the running call carries: the innermost Lua frame's, [`Context::Client`] when
/// none is tagged.
pub(crate) fn current(lua: &Lua) -> Context {
    let tags: Option<Table> = lua.named_registry_value(REG_FN_CONTEXT).ok();
    let mut level = 0;
    loop {
        let frame = lua.inspect_stack(level, |debug| {
            if let Some(tags) = &tags {
                match tags.raw_get::<Value>(debug.function()) {
                    Ok(Value::Integer(_)) => return Some(Context::Macro),
                    Ok(Value::String(name)) => {
                        return Some(Context::Addon(name.to_string_lossy()));
                    }
                    _ => {}
                }
            }
            let source = debug.source().source;
            let model = lua.app_data_ref::<Model>()?;
            addon_of_chunk(&model, source.as_deref()?).map(Context::Addon)
        });
        match frame {
            None => return Context::Client,
            Some(Some(context)) => return context,
            Some(None) => level += 1,
        }
    }
}
