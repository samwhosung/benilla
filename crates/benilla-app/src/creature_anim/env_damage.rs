//! Environmental-damage feedback — the fall-landing dust puff and pain grunt (decision 0412).
//! Two independent sources fire it, matching the reference's own double-fire:
//!
//! 1. **The wire arm** (`net/apply`): `SMSG_ENVIRONMENTALDAMAGELOG` → [`EnvDamageTable`] maps the
//!    damage type (0 exhausted · 1 drowning · 2 fall · 3 lava · 4 slime · 5 fire) to a
//!    `SpellVisualKit` id, played on the victim through the ordinary discrete kit play
//!    ([`super::KitPush`] → `PlaySpellVisualKit` `0x60edf0`). This is the *unconditional tail* of
//!    the reference's 0x1FC consequence method `0x624f30` (fires for any resolved unit; the
//!    floating damage number + self-only chat line are the method's other two legs, not modeled).
//!    The server sends no sound with it — `0x624f30` plays no vocal at all.
//!
//! 2. **The client-side landing predictor** (`0x602d00`, the movement tick's *other* caller of
//!    `0x624f30`): on a hard landing the client plays the unit's wound vocal (class 2 →
//!    CreatureSoundData) AND re-fires the *same* fall kit locally with damage 0 — without waiting
//!    for the server. So a damaging local fall shows the puff twice (predicted, then echoed an
//!    RTT later) and the grunt is heard immediately. The metric is **fall height in yards**
//!    (`0x7c60c0`), gated at [`HARD_LANDING_DESCENT`]. The vocal leg lives in [`crate::sound`]
//!    (it owns the voice catalog); the dust leg is [`hard_landing_dust`] here; both gate on the
//!    same descent.
//!
//! **Deliberate scope (decision 0412):** benilla drives the predictor from the *self* controller's
//! landing edge only — remote movers' landings aren't detected yet, though the reference fires it
//! for any mover (its call graph reaches `0x602d00` from the networked movement handler). And the
//! HARD gate's immunity modifier (feather-fall / safe-fall auras force SOFT in the `13 < h < 70`
//! band; `h ≥ 70` bypasses it) isn't modeled — those auras aren't tracked, so every fall past the
//! floor reads HARD, which is the common case.

use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};

/// The loaded 6-slot table (`None` until the startup load lands; absent = no environmental
/// feedback kits, like every optional DBC face).
#[derive(Resource)]
pub(crate) struct EnvDamageTable(pub(crate) benilla_formats::EnvironmentalDamageTable);

/// Load the table off the patch chain at startup (the [`super::blood`] pattern).
pub(super) fn load_env_damage_table(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_environmental_damage(&mut chain)
    };
    match loaded {
        Ok(table) => commands.insert_resource(EnvDamageTable(table)),
        Err(e) => {
            warn!("anim: EnvironmentalDamage.dbc failed to load (no fall-damage dust): {e:#}")
        }
    }
}

/// The landing predictor's HARD-landing gate — the client's `[0x80c414]` = **13.0** yd of fall
/// height (`0x602d00`'s `fcomp 13.0`, byte-verified). Below it the client plays only its
/// jump-end/land sound (SOFT — class 0xc, not modeled here); above it comes the wound grunt + the
/// dust puff. The sibling constant `[0x80c418]` = 70.0 yd is the *unconditional* tier that bypasses
/// the immunity check (feather/safe-fall) — moot for us until those auras are tracked, so every
/// fall past this floor reads HARD. Note the client grunts from 13.0 yd but the *server* only
/// damages from ≈14.57 yd, so a 13–14.57 yd fall grunts and puffs with zero damage and no packet —
/// faithful (decision 0412).
pub(crate) const HARD_LANDING_DESCENT: f32 = 13.0;

/// The controller's landing report, written on **every** landing (ungated — consumers apply
/// [`HARD_LANDING_DESCENT`], keeping the predictor's threshold law in this byte-cited module).
#[derive(Message, Clone, Copy)]
pub(crate) struct HardLanding {
    pub(crate) entity: Entity,
    /// Fall height in yd: the arc's launch height − the landing height. The reference's metric is
    /// apex−current (`0x7c60c0`), which equals this for a step-off (launch *is* the apex) and
    /// overcounts ours by at most the jump rise (~1.6 yd) for a jump *off* a ledge — negligible
    /// against the 13-yd floor, and only near the boundary.
    pub(crate) descent: f32,
}

/// The dust leg of the landing predictor: `0x602d00`'s tail re-fires `0x624f30(type 2, damage 0)`
/// — the same fall kit the wire arm plays — locally, at the landing frame.
pub(super) fn hard_landing_dust(
    mut landings: MessageReader<HardLanding>,
    table: Option<Res<EnvDamageTable>>,
    mut seq: ResMut<super::PlaySeq>,
    mut pushes: MessageWriter<super::KitPush>,
) {
    for l in landings.read() {
        if l.descent <= HARD_LANDING_DESCENT {
            continue;
        }
        // Damage type 2 = fall (the predictor is fall-only; the other five types are wire-only).
        if let Some(kit_id) = table.as_ref().and_then(|t| t.0.kit_id(2)) {
            debug!(
                "anim: hard landing ({:.1} yd) → predicted dust kit {kit_id}",
                l.descent
            );
            pushes.write(super::KitPush {
                entity: l.entity,
                kit_id,
                seq: seq.next(),
            });
        }
    }
}
