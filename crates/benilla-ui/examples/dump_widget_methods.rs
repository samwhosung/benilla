//! `dump_widget_methods`: print benilla's widget method surface, asked of a real VM.
//!
//! ```text
//! cargo run -q -p benilla-ui --example dump_widget_methods    # class<TAB>method, one per line
//! ```
//!
//! The census is `widget_method_census`, the same function the widget-surface gate reads.
use benilla_ui::script::{widget_method_census, UiScript};

fn main() -> mlua::Result<()> {
    for (class, method) in widget_method_census(&UiScript::new()?)? {
        println!("{class}\t{method}");
    }
    Ok(())
}
