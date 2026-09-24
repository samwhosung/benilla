use mlua::{Function, ObjectLike, Table, Value};

use crate::framexml::Element;

use super::{children_named, Loader};

impl Loader<'_> {
    /// `<Scripts>` → SetScript each handler (`0x769ef0`). Handler source comes from the element
    /// body (`<OnClick>lua…</OnClick>`) or a `function="Global"` attribute. Returns the compiled
    /// `OnLoad` handle (if any) so the caller can fire it bottom-up. A body that fails to *compile* is
    /// an error; an unsupported handler *name* (e.g. the keyboard-focus handlers `OnKeyDown`/`OnChar`,
    /// not yet modeled) is a warn-once gap.
    pub(super) fn apply_scripts(
        &mut self,
        el: &Element,
        wrapper: &Table,
        dbg: &str,
    ) -> Option<Function> {
        let mut onload = None;
        // **The chunk name of an XML handler body is `"<GetName()>:<Handler>"`, read off the
        // WRAPPER — not off the loader's diagnostic string.** `0x7025fd` calls the script object's
        // `GetName` through its own vtable (`[[esi]+4]`), falls back to `<unnamed>` (`0x84c7f0`)
        // at `0x702611` when that is NULL, formats `"%s:%s"` (`0x872a28`) at `0x70261b`, and hands
        // the result to `0x704c70` at `0x70263c` as the chunk name.
        //
        // Reading it here rather than plumbing `dbg` down is the fidelity: `dbg` is a *sentence*
        // on the `CreateFrame` path (`CreateFrame("Button", "Foo", inherits="Bar")`), which made
        // every template-inherited handler's errors and `debugstack` frames read as one, and an
        // unnamed element wrote `<Button>` where the image writes `<unnamed>`.
        for scripts in children_named(el, "Scripts") {
            let owner = wrapper
                .call_method::<Option<String>>("GetName", ())
                .ok()
                .flatten()
                .unwrap_or_else(|| "<unnamed>".to_string());
            for handler in &scripts.children {
                let name = handler.tag.clone();
                // **AN EMPTY BODY IS A CLEAR, AND WHITESPACE IS NOT EMPTY** — the two byte
                // tests at `SetScript 0x7025c0`, reproduced rather than approximated:
                //
                //     7025ec  call 0x702670      ; unref the PREVIOUS handler, unconditionally
                //     7025f4  test ebx,ebx
                //     7025f6  je 0x702655        ; text == NULL -> store 0 (reads back as nil)
                //     7025f8  cmp byte ptr [ebx],0
                //     7025fb  je 0x702655        ; text == ""   -> same
                //
                // The XMLTree chardata handler (`0x6f29d0`) appends every run with `len > 0` and
                // nothing trims, so `0x7025f8` tests the FIRST BYTE of the raw body.
                //
                // That distinction is the whole finding, and it is not academic. 1.12's own
                // FrameXML has **zero** `<OnX/>` and **zero** `<OnX></OnX>`; its seven blanking
                // sites (BuffFrame ×3, BankFrame, PaperDollFrame ×2, UIPanelTemplates) are all
                // `<OnLoad>`↵`</OnLoad>` — a body of `"\n\t\t\t"`, whose first byte is `0x0A`.
                // Both tests fail and it **compiles to a valid empty function**. So
                // `TempEnchant1:GetScript("OnLoad")` answers a FUNCTION in the real client, not
                // nil, and the inherited `BuffButton_OnLoad` is displaced rather than removed.
                // A `.trim()` here would produce the right window and the wrong answer to an addon.
                //
                // The clear path is still real (an empty body, or a `function=` element with no
                // body — 5875 never reads that attribute at all, so it takes the NULL leg), and it
                // matters that it is a stored nil: a `0` registry ref is skipped by the dispatch
                // guard entirely, where the compiled no-op is entered and returns.
                let cleared = handler.body.is_empty() && handler.attr("function").is_none();
                let func = if cleared {
                    None
                } else {
                    match self.compile_handler(handler, &name, &owner, dbg) {
                        Some(f) => Some(f),
                        None => continue,
                    }
                };
                // SetScript stores it; an unsupported name errors — surface as a gap, don't drop hard.
                if let Err(e) = wrapper.call_method::<()>("SetScript", (name.clone(), func.clone()))
                {
                    self.warn_once(
                        &format!("script:{name}"),
                        format!("{dbg}: SetScript(\"{name}\") unsupported in v1: {e}"),
                    );
                    continue;
                }
                // The real `<Scripts>` walker auto-enables the matching input kind after each
                // successful SetScript (`0x769ef0` → `0x76af00(kind,-1)` per handler name): the
                // five MOUSE-kind handlers arm the same enable as the XML `enableMouse` attribute —
                // `OnDragStart` is in the set, `OnDragStop`/`OnReceiveDrag` are NOT. This is
                // XML-load-time ONLY: the Lua SetScript binding (`0x7748d0`) never auto-enables, so
                // the law
                // lives here and not in SetScript itself (a runtime-created frame still needs an
                // explicit `EnableMouse(true)`, like the real client). The KEYBOARD kinds are
                // modelled and armed just below, and the WHEEL kind (`OnMouseWheel` = kind 3) is
                // armed the same way right after — it used to say "a separate index this engine
                // doesn't model yet", and the wheel dispatcher's own gate (`0x7664f0` →
                // `0x76c180`) is what made modelling it necessary:
                // the wheel plane is the ONE place the engine really does gate on a handler and
                // continue past a frame that has none. Until it was its own flag, our wheel sweep
                // had to accept any mouse-enabled frame as a stand-in, and the first such frame
                // swallowed the wheel.
                const MOUSE_KIND: [&str; 5] = [
                    "OnEnter",
                    "OnLeave",
                    "OnMouseDown",
                    "OnMouseUp",
                    "OnDragStart",
                ];
                if MOUSE_KIND.iter().any(|k| name.eq_ignore_ascii_case(k)) {
                    self.call(wrapper, "EnableMouse", true, dbg);
                }
                if name.eq_ignore_ascii_case("OnMouseWheel") {
                    self.call(wrapper, "EnableMouseWheel", true, dbg);
                }
                // The KEYBOARD kinds, the same walker rule one index over (`OnChar` = kind 0,
                // `OnKeyDown`/`OnKeyUp` = kind 1). Bucket membership is what the delivery walk
                // iterates ([`crate::script::keyboard`]) and it is the FLAG, never the presence of
                // a script — so an XML frame carrying a key handler but no `enableKeyboard`
                // attribute would never be reached by the dispatcher at all, and its handler could
                // not fire. XML-load-time only, like the mouse half.
                const KEY_KIND: [&str; 3] = ["OnChar", "OnKeyDown", "OnKeyUp"];
                if KEY_KIND.iter().any(|k| name.eq_ignore_ascii_case(k)) {
                    self.call(wrapper, "EnableKeyboard", true, dbg);
                }
                if name.eq_ignore_ascii_case("OnLoad") {
                    // A blanked `<OnLoad/>` leaves nothing to fire — and it must also UNSET a
                    // handle a template's own OnLoad put here, or the caller fires the very body
                    // this element exists to remove.
                    onload = func;
                }
            }
        }
        onload
    }

    /// Compile a handler element into `function(self, ...) <body> end`, or resolve its
    /// `function="Global"` reference. A syntax error in the body is recorded as an error (and the
    /// handler dropped), never a panic.
    ///
    /// ## `self` falls back to `this`, and that one clause is a whole class of addon bug
    ///
    /// The 1.12 contract for an XML script body is that it takes **no arguments**: the frame
    /// arrives as the `this` global, which [`crate::script::event::invoke_with_globals`] sets
    /// around every dispatch. This engine also passes the modern `(self, event, …)` positionals —
    /// convenient, and what Era-era addons expect — and our own FrameXML is written against
    /// *that* spelling (`<OnEnter>BenillaBagToggle_OnEnter(self)</OnEnter>`).
    ///
    /// The two spellings agree right up until **an addon captures a script and calls it back**,
    /// which is the standard 1.12 hook idiom and the reason `GetScript` exists:
    ///
    /// ```lua
    /// bMainBag_OnEnter = MainMenuBarBackpackButton:GetScript("OnEnter")  -- Bagnon.lua:61
    /// MainMenuBarBackpackButton:SetScript("OnEnter", BagnonBlizMainBag_OnEnter)
    /// function BagnonBlizMainBag_OnEnter()
    ///     …
    ///     bMainBag_OnEnter()      -- l.87: no arguments. The reference's contract.
    /// end
    /// ```
    ///
    /// The re-entry is perfectly legal, `this` is correctly set (Bagnon's own body reads
    /// `this:GetID()` one line above), and our compiled body still took `self` from argument 1 —
    /// which was nil. The director saw it as `bad argument #2: error converting Lua nil to table`
    /// out of `GameTooltip:SetOwner`, from a handler that works perfectly when the engine calls it.
    ///
    /// So the fallback lives **here**, once, rather than as `(self or this)` sprinkled through
    /// ~20 handler bodies: any XML body in any file — ours or an addon's — now works under both
    /// callers.
    ///
    /// ## The prologue shares the body's first line, and that is a line-number fix
    ///
    /// The reference compiles the handler body **as the chunk**: `0x704c70` takes the raw text and
    /// `luaL_loadbuffer`s it, so body line *n* is chunk line *n*. Our wrapper used to end with a
    /// newline, which pushed every body line down by one — a `<OnClick>` whose code sat on the
    /// element's second line reported `:3:` where the reference reports `:2:`. Keeping the
    /// prologue on the body's own first line restores the reference's numbering exactly; the
    /// closing `end` is the only line we add, and it lands *after* the body.
    ///
    /// `owner` is the frame's `GetName()` (or `<unnamed>`) — see [`Self::apply_scripts`] for the
    /// bytes; `dbg` names the load for the report and is deliberately not the same string.
    pub(super) fn compile_handler(
        &mut self,
        handler: &Element,
        name: &str,
        owner: &str,
        dbg: &str,
    ) -> Option<Function> {
        // **The raw body, NOT a trimmed one** — 1.12 hands `node->text` straight to
        // `luaL_loadbuffer` and tests only its first byte, so a whitespace-only body is a real
        // (empty) chunk there and must be one here. See `apply_scripts` for the bytes and for the
        // seven stock files that depend on it.
        let body = handler.body.as_str();
        if !body.is_empty() {
            let src = format!(
                "return function(self, ...) if self == nil then self = this end {body}\nend"
            );
            match self
                .lua()
                .load(&src)
                .set_name(format!("{owner}:{name}"))
                .set_mode(mlua::ChunkMode::Text)
                .eval::<Function>()
            {
                Ok(f) => return Some(f),
                Err(e) => {
                    self.report
                        .errors
                        .push(format!("{dbg}: compiling <{name}>: {e}"));
                    return None;
                }
            }
        }
        if let Some(global) = handler.attr("function") {
            match self.lua().globals().get::<Value>(global) {
                Ok(Value::Function(f)) => return Some(f),
                _ => {
                    self.warn_once(
                        &format!("fn:{global}"),
                        format!(
                            "{dbg}: <{name} function=\"{global}\">: no such global function (yet)"
                        ),
                    );
                    return None;
                }
            }
        }
        None
    }

    /// Fire a captured `OnLoad` with the frame wrapper as both the legacy `this` global
    /// (`0x704d50`, set-then-restored) and the modern `self` argument — the same dual convention
    /// the host's event path uses; we replicate only the `this` set/restore here because there is
    /// no public host API to
    /// fire `OnLoad` directly (it is not an event). Handler errors are recorded, never propagated.
    pub(super) fn fire_onload(&mut self, wrapper: &Table, func: &Function, dbg: &str) {
        // The `this`/`self` convention lives in one home (`UiScript::invoke_handler`); the
        // loader doesn't re-implement the set/restore, it just supplies the wrapper + captured func.
        if let Err(e) = self.invoke_handler(wrapper, func) {
            self.report.errors.push(format!("{dbg}: OnLoad: {e}"));
        }
    }

    /// Call a frame-wrapper method, recording (not propagating) any error.
    pub(super) fn call(
        &mut self,
        wrapper: &Table,
        method: &str,
        args: impl mlua::IntoLuaMulti,
        dbg: &str,
    ) {
        if let Err(e) = wrapper.call_method::<()>(method, args) {
            self.report.errors.push(format!("{dbg}: {method}: {e}"));
        }
    }

    /// Call a region-wrapper method, recording (not propagating) any error.
    pub(super) fn call_region(
        &mut self,
        region: &Table,
        method: &str,
        args: impl mlua::IntoLuaMulti,
        dbg: &str,
    ) {
        if let Err(e) = region.call_method::<()>(method, args) {
            self.report
                .errors
                .push(format!("{dbg}: region {method}: {e}"));
        }
    }
}
