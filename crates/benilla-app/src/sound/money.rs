//! The money coin sound — `LOOTWINDOWCOINSOUND` on every change to the self-player's coinage.
//!
//! The real 1.12 client has **no dedicated money kit**: every coinage change plays
//! `LOOTWINDOWCOINSOUND` (SoundEntries kit 895) from a `CMirrorHandler` callback registered on
//! `PLAYER_FIELD_COINAGE` (`0x5ddf30`) — the
//! same watcher that also fires the `PLAYER_MONEY` UI event. So one rule reproduces the coin for
//! **buy** (spend), **sell** (gain), and **loot money** — all three move `PLAYER_FIELD_COINAGE` on
//! the wire — plus any other purse change (quest reward, mail, trade), exactly as the client does.
//! (Looting an *item* is the only acquire that plays a per-item sound instead; that lives in
//! [`super::ui`].)
//!
//! benilla plays it 2D on the SFX bucket when the mirrored coinage moves. The **first** observation
//! after login/reconnect is a seed, not a change — it never plays for the initial populate (whether
//! the reference's watcher fires on the first coinage set at login is inferred, unconfirmed in the
//! binary; suppressing it is the correct-feeling choice and avoids a coin on every zone-in). The
//! real client double-plays loot money (an optimistic play at the click plus this watcher); benilla
//! keeps the single watcher-driven play — one clean coin on the confirmed change, imperceptibly
//! latent on localhost.

use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use benilla_assets::WorldAssets;

use super::kit::{self, KitRef, SoundKits};
use super::{SoundConfig, SoundOutput};

/// The SoundEntries name the client plays on any coinage change (kit 895; `0x5ddf30`). Played by
/// name through the same registry as every interface sound.
const COIN_SOUND: &str = "LOOTWINDOWCOINSOUND";

/// Play the coin whenever the self-player's `PLAYER_FIELD_COINAGE` changes. The previous value is a
/// `Local` seeded on first sight (no play for the initial populate) and reset to `None` whenever no
/// self-player exists (pre-login / disconnect), so a reconnect re-seeds rather than replaying a
/// stale delta. Fires on both directions (a buy is a decrease, a sell/loot an increase) — the
/// client's watcher plays unconditionally on change.
fn play_coin_on_coinage_change(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut prev: Local<Option<u32>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
) {
    let Some(store) = self_q.iter().next() else {
        *prev = None;
        return;
    };
    let Some(money) = store.0.player_money() else {
        return;
    };
    // Advance the memory unconditionally (even without a catalog to play through) so a late-loading
    // catalog never replays this delta; only an actual change past the seeded value plays.
    let old = prev.replace(money);
    if !matches!(old, Some(p) if p != money) {
        return; // first sight or unchanged
    }
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    if let Err(e) = kit::play_kit(
        &mut kits,
        &assets,
        &mut out,
        &config,
        Vec3::ZERO,
        KitRef::Name(COIN_SOUND),
        None,
        kit::SoundCategory::Sfx,
    ) {
        debug!("sound(money): coin — {e:#}");
    }
}

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, play_coin_on_coinage_change);
}
