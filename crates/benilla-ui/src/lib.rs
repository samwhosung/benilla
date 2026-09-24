//! The engine-free core of benilla's UI: the `.toc`, FrameXML and `Bindings.xml` parsers, the
//! layout resolver, the frame arena, draw order and the Lua host FrameXML runs in; no Bevy, no GPU.

pub mod bindings_xml;
pub mod civil;
pub mod framexml;
pub mod justify;
pub mod layout;
pub mod loader;
pub mod markup;
pub mod messages;
pub mod order;
pub mod script;
pub mod source;
pub mod strings;
pub mod toc;
pub mod widget;
