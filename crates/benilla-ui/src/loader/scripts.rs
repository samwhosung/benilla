use mlua::{Function, ObjectLike, Table};

use crate::framexml::Element;

use super::Loader;

impl Loader<'_> {
    /// One `<Scripts>` arm of the frame's child loop (`0x769d18` → `0x769ef0`): `SetScript` for
    /// each handler. An `OnLoad` replaces `onload`, which the caller fires bottom-up.
    pub(super) fn apply_scripts(
        &mut self,
        scripts: &Element,
        wrapper: &Table,
        dbg: &str,
        onload: &mut Option<Function>,
    ) {
        // A handler's chunk name is `"<name>:<Handler>"`, the frame's own name or `<unnamed>`
        // (`0x7025fd`-`0x70263c`), never the loader's `dbg`.
        let owner = crate::script::binding_abi::name_of(self.lua, wrapper);
        for handler in &scripts.children {
            let name = handler.tag.clone();
            // A tag the frame's type has no slot for reports `Frame %s: Unknown script
            // element %s` (`0x878778`, `0x769f44`) and enables nothing; the name is looked up
            // first (`0x7025d5`) and the body compiled after (`0x70263a`).
            let known = wrapper
                .call_method::<mlua::Value>("HasScript", name.clone())
                .is_ok_and(|v| !v.is_nil());
            if !known {
                self.status_warning(format!("Frame {owner}: Unknown script element {name}"));
                continue;
            }
            // An empty body clears the handler to nil, but a whitespace one is an empty
            // function: `SetScript` (`0x7025c0`) tests only NULL and the first byte
            // (`0x7025f8`), and nothing trims the body (`0x6f29d0`). The stock blanks
            // (`BuffFrame.xml:101`) are whitespace, so their `GetScript` answers a function.
            let func = if handler.body.is_empty() {
                None
            } else {
                self.compile_handler(handler, &name, &owner, dbg)
            };
            if let Err(e) = wrapper.call_method::<()>("SetScript", (name.clone(), func.clone())) {
                self.report
                    .fault(format!("{dbg}: SetScript(\"{name}\"): {e}"));
                continue;
            }
            // After each `SetScript` the XML walker enables the handler's input kind
            // (`0x769ef0` → `0x76af00`): these five arm the mouse (`OnDragStop` and
            // `OnReceiveDrag` do not), `OnMouseWheel` the wheel (kind 3). The Lua `SetScript`
            // never does (`0x7748d0`).
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
            // The key handlers, the same rule, one bit each: `OnChar` the char bit,
            // `OnKeyDown` and `OnKeyUp` the key bit (`0x769f6b`, `0x769fa4`).
            if let Err(e) = crate::script::enable_for_script(self.lua, wrapper, &name) {
                self.report.fault(format!("{dbg}: {name}: {e}"));
            }
            if name.eq_ignore_ascii_case("OnLoad") {
                // A cleared `OnLoad` also unsets a template's, or the caller would fire it.
                *onload = func;
            }
        }
    }

    /// Compile a handler body as the chunk itself, as the reference loads it (`0x704c70`): it takes
    /// no arguments and reads its frame from `this`, so a `self` in it is an ordinary global. The
    /// loader reads no other attribute of the element (no `function=`). A body that fails to
    /// compile reports nothing (`0x704c87`-`0x704c9f`) and still fills the slot, replacing a
    /// template's, with a handler that calls nil each time it fires
    /// ([`crate::script::object::broken_handler`]); `0x7025c0` still answers success (`0x70264c`),
    /// so the enables run.
    pub(super) fn compile_handler(
        &mut self,
        handler: &Element,
        name: &str,
        owner: &str,
        dbg: &str,
    ) -> Option<Function> {
        // The raw body, never trimmed: a whitespace body is a real empty chunk (`apply_scripts`).
        let body = handler.body.as_str();
        if body.is_empty() {
            return None;
        }
        let compiled = self
            .lua()
            .load(body)
            .set_name(format!("{owner}:{name}"))
            .set_mode(mlua::ChunkMode::Text)
            .into_function();
        let f = match compiled {
            Ok(f) => f,
            Err(_) => match crate::script::object::broken_handler(self.lua()) {
                Ok(f) => return Some(f),
                Err(e) => {
                    self.report.fault(format!("{dbg}: <{name}>: {e}"));
                    return None;
                }
            },
        };
        // An insecure addon's handler is its code whoever fires it ([`crate::script::taint`]).
        crate::script::taint::tag_for_load(self.lua(), &f);
        Some(f)
    }

    /// Fire a captured `OnLoad` under the event path's convention, `this` set and restored
    /// (`0x704d50`); an error is recorded, never propagated.
    pub(super) fn fire_onload(&mut self, wrapper: &Table, func: &Function, dbg: &str) {
        if let Err(e) = self.invoke_handler(wrapper, func) {
            self.report.fault(format!("{dbg}: OnLoad: {e}"));
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
            self.report.fault(format!("{dbg}: {method}: {e}"));
        }
    }

    /// Install the texture a widget element's child (`<BarTexture>`, `<ThumbTexture>`, a ColorSelect
    /// texture) built as the widget's object, through the engine's own install, recording (not
    /// propagating) any error: the Lua setters take the object too, but the loader needs no Lua
    /// call to say it.
    pub(super) fn install_texture(
        &mut self,
        wrapper: &Table,
        tag: &str,
        texture: &Table,
        layer: Option<&str>,
        dbg: &str,
    ) {
        if let Err(e) =
            crate::script::install_loaded_texture(self.lua, wrapper, tag, texture, layer)
        {
            self.report.fault(format!("{dbg}: <{tag}>: {e}"));
        }
    }

    /// [`Self::call_region`] on a region wrapper, else [`Self::call`].
    pub(super) fn call_on(
        &mut self,
        wrapper: &Table,
        region: bool,
        method: &str,
        args: impl mlua::IntoLuaMulti,
        dbg: &str,
    ) {
        if region {
            self.call_region(wrapper, method, args, dbg);
        } else {
            self.call(wrapper, method, args, dbg);
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
            self.report.fault(format!("{dbg}: region {method}: {e}"));
        }
    }
}
