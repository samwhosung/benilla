//! `extended_launcher`: the client with a plugin of its own on top, started through
//! [`benilla_app::run_with`] as a crate built on benilla starts it. A feature 1.12.1 does not have
//! lives in a crate like this one, never in benilla.
//!
//! ```text
//! cargo run -p benilla-app --example extended_launcher
//! ```
//!
//! The banner reads `benilla build … · extended`, and the plugin logs one line at `Startup`.
//!
//! A crate of its own copies the `benilla` launcher (`crates/benilla`): a build script that stamps
//! its commit, a `main` like this one that reads the stamp back as `crates/benilla/src/main.rs`
//! does, and a manifest that takes benilla as the workspace does:
//!
//! ```toml
//! [features]
//! default = ["dev"]
//! # The instruments; the player build is `--no-default-features`.
//! dev = ["benilla-app/dev"]
//!
//! [dependencies]
//! # `default-features = false`, or `--no-default-features` still builds benilla-app with `dev`.
//! benilla-app = { git = "https://github.com/samwhosung/benilla", default-features = false }
//! # benilla's version, adding no features: bevy's defaults bring its audio stack back.
//! bevy = { version = "0.18.1", default-features = false }
//!
//! [build-dependencies]
//! benilla-buildstamp = { git = "https://github.com/samwhosung/benilla" }
//!
//! # Cargo reads `[patch]` and `[profile]` from the root manifest only, so benilla's are repeated:
//! # without the patches the addon Lua and the mixer are not benilla's.
//! [patch.crates-io]
//! lua-src = { git = "https://github.com/samwhosung/benilla" }
//! kira = { git = "https://github.com/samwhosung/benilla" }
//!
//! [profile.dev]
//! opt-level = 1
//!
//! [profile.dev.package."*"]
//! opt-level = 3
//! ```
//!
//! ```text
//! // build.rs
//! fn main() {
//!     benilla_buildstamp::emit();
//! }
//! ```
//!
//! Its `Cargo.lock` starts as a copy of benilla's: cargo reads only the root's lockfile too, and
//! without it resolves every crate to its newest compatible version, not the one benilla is built
//! with. A dev build looks for the `WoW` link and `benilla-config/` at benilla's repo root, which
//! for a git dependency is cargo's checkout, so `WOW_DATA` and `BENILLA_HOME` name them instead.

use benilla_app::{AppExit, BuildId};
use bevy::prelude::*;

/// The crate's own plugin: one line at `Startup`, to show it runs inside the client.
struct HelloPlugin;

impl Plugin for HelloPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, || info!("a plugin on top of benilla is running"));
    }
}

fn main() -> AppExit {
    // A launcher of its own passes the stamp its build script made; an example has no build
    // script, so this names the version and profile alone.
    let build = BuildId {
        version: env!("CARGO_PKG_VERSION"),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        ..Default::default()
    };
    benilla_app::run_with(build, |app| {
        app.add_plugins(HelloPlugin);
    })
}
