//! `dump_widget_methods` — print benilla's widget METHOD surface, asked of a real VM.
//!
//! ```text
//! cargo run -q -p benilla-ui --example dump_widget_methods    # class<TAB>method, one per line
//! ```
//!
//! The sibling of [`dump_globals`](../dump_globals.rs). The measurement itself lives in
//! `benilla_ui::script::widget_method_census` — its module doc is the *why*, and the widget-surface
//! gate reads the same function, so the instrument you look at by hand and the gate
//! that fails the build can never disagree about what our surface is.
use benilla_ui::script::{widget_method_census, UiScript};

fn main() -> mlua::Result<()> {
    for (class, method) in widget_method_census(&UiScript::new()?)? {
        println!("{class}\t{method}");
    }
    Ok(())
}
