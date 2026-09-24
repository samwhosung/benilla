//! The `benilla-worldview` launcher: boots the `benilla-world` engine with no game attached, so an
//! engine system that needs a gameplay resource fails here, a coupling no compiler sees. Like
//! `benilla`, a shim that carries the build stamp, so a new commit recompiles only these lines.

use benilla_world::build_id::BuildId;

fn main() -> benilla_world::AppExit {
    benilla_world::worldview::run(BuildId {
        sha: env!("BENILLA_GIT_SHA"),
        short: env!("BENILLA_GIT_SHORT"),
        date: env!("BENILLA_GIT_DATE"),
        profile: env!("BENILLA_PROFILE"),
    })
}
