//! `cargo run -p benilla-formats --example where` — **where does benilla think the install is?**
//!
//! The resolver's own answer, printed. Two callers, and both are the reason this
//! is a program rather than a comment:
//!
//! - `scripts/gates.sh`, to decide whether the enforcer gate can run. It used to hand-roll half
//!   the rule (`[ -n "$WOW_DATA" ] || [ -d WoW/Data ]`), which is exactly the duplication 1175
//!   exists to end — a gate that disagrees with the client about where the install is will skip
//!   on a machine where the client would have run, or run on one where it cannot.
//! - a human whose client came up with no world. "It looked in these four places and none of them
//!   existed" is actionable; "the terrain just doesn't appear" is not.
//!
//! Exits **0** when an install was found and **1** when none was, so a shell can branch on it.

fn main() -> std::process::ExitCode {
    match benilla_formats::wow_data() {
        Some(data) => {
            println!("{}", data.display());
            std::process::ExitCode::SUCCESS
        }
        None => {
            eprintln!("no WoW install found. Looked in, in order:");
            for c in benilla_formats::candidates() {
                eprintln!("  {}", c.display());
            }
            eprintln!("Set $WOW_DATA, or put a WoW folder beside the binary.");
            std::process::ExitCode::FAILURE
        }
    }
}
