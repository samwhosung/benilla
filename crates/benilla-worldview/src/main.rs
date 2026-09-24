//! The `benilla-worldview` launcher — the world viewer's shim, the `benilla` launcher's twin.
//!
//! **What this binary is for**: `benilla-app` is being split into `benilla-world`
//! (the engine: renders a WoW world, streams it, flies over it, clicks on it) and the game that
//! stands on it. A crate boundary alone cannot hold that line — in Bevy, code couples through
//! *resources at runtime* with no symbol crossing between the crates — so the enforcer is a second
//! program that boots the engine with **no game attached**. It is built by
//! `cargo clippy --workspace --all-targets`, which is already a gate, so wiring a game concept back
//! into the engine breaks it the same day.
//!
//! It is also where `benilla-editor` starts: a window that loads a map and lets you fly around is
//! the editor's first milestone.
//!
//! Like `benilla`, this is a shim so the every-commit git stamp dirties ~20 lines instead of the
//! app crate.

use benilla_world::build_id::BuildId;

fn main() -> benilla_world::AppExit {
    benilla_world::worldview::run(BuildId {
        sha: env!("BENILLA_GIT_SHA"),
        short: env!("BENILLA_GIT_SHORT"),
        date: env!("BENILLA_GIT_DATE"),
        profile: env!("BENILLA_PROFILE"),
    })
}
