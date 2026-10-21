//! The engine-free core of benilla's UI: the `.toc`, FrameXML and `Bindings.xml` parsers, the
//! layout resolver, the frame arena, draw order and the Lua host FrameXML runs in; no Bevy, no GPU.

// Every `mlua::` path in this crate resolves to benilla's own interpreter, and dependents name
// the same API through `benilla_ui::mlua`.
pub extern crate benilla_mlua as mlua;

pub mod auction_sort;
pub mod bindings_xml;
pub mod civil;
pub mod color;
pub mod crt;
pub mod framexml;
pub mod justify;
pub mod layout;
pub mod loader;
pub mod markup;
pub mod messages;
pub mod order;
pub mod screencoord;
pub mod script;
pub mod source;
pub mod status;
pub mod strings;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod toc;
pub mod widget;
