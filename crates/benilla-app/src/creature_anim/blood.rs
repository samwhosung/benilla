//! The melee blood spurt (`0x624530` → `0x625010`): a self-terminating particle model on the
//! victim, front or back by where the attacker stands, large on a crushing blow, colored by the
//! victim's blood type.

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::BloodCatalog;

use crate::net::NetEntity;
use benilla_assets::{LockRecover, WorldAssets};

use super::spell_visual::FxSlot;
use super::spell_visual::SpellVisuals;
use super::{SpellKitFx, SwingImpact, SwingMessage};

/// The locale caps of `violenceLevel` by locale index (`[0xc0e080]`), the table at `0x86c3f8`: 2
/// everywhere but koKR, which gets 1. The row's default is the cap (`0x6c5aa0`).
const VIOLENCE_CAPS: [i32; 8] = [2, 1, 2, 2, 2, 2, 2, 2];

/// The gore level the blood spurt indexes `UnitBloodLevels` by (`0x60a39d` → `0x6c5b50`, the word
/// `[0xce8534]`): 0 none, 1 green, 2 true colors. Initially the English cap.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ViolenceLevel(pub(crate) usize);

impl Default for ViolenceLevel {
    fn default() -> Self {
        Self(VIOLENCE_CAPS[0] as usize)
    }
}

/// `violenceLevel`'s callback (`0x6c5af0`): the integer of the text (`SStrToInt`, `0x64ac60`),
/// and a value below 0 or above the locale's cap (signed compares, `0x6c5b09`-`0x6c5b1b`) is
/// stored as the cap. The CVar's own text stays as typed.
pub(crate) fn on_cvar(
    ev: &crate::cvars::CvarChanged,
    mut level: bevy::prelude::Mut<'_, ViolenceLevel>,
    locale: usize,
) {
    if ev.is("violenceLevel") {
        *level = ViolenceLevel(clamp_violence(
            benilla_ui::script::s_str_to_int(&ev.new),
            locale,
        ));
    }
}

fn clamp_violence(value: i32, locale: usize) -> usize {
    let cap = VIOLENCE_CAPS[locale.min(VIOLENCE_CAPS.len() - 1)];
    if (0..=cap).contains(&value) {
        value as usize
    } else {
        cap as usize
    }
}

/// The victim attachments the spurt hangs on, `0xf` front and `0x10` back (`0x625010`).
const ATTACH_FRONT: u16 = 15;
const ATTACH_BACK: u16 = 16;

/// The `UnitBlood` and `UnitBloodLevels` tables; absent, no spurts.
#[derive(Resource)]
pub(super) struct BloodTables(pub(super) BloodCatalog);

/// Load the blood tables off the patch chain at startup.
pub(super) fn load_blood_tables(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_blood_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            let (levels, rows) = cat.len();
            info!("anim: {levels} UnitBloodLevels / {rows} UnitBlood rows");
            commands.insert_resource(BloodTables(cat));
        }
        Err(e) => warn!("anim: blood tables failed to load (no melee spurts): {e:#}"),
    }
}

/// One spurt per landed swing, at the clip's impact key ([`SwingImpact`]). A missing link in the
/// blood tables drops it, as the client skips a null record, and logs why at `info`.
pub(super) fn blood_spurts(
    mut swings: MessageReader<SwingImpact>,
    transforms: Query<&Transform>,
    net: Query<&NetEntity>,
    creatures: Option<Res<crate::entities::Creatures>>,
    blood: Option<Res<BloodTables>>,
    visuals: Option<Res<SpellVisuals>>,
    violence: Res<ViolenceLevel>,
    mut fx: MessageWriter<SpellKitFx>,
) {
    let (Some(creatures), Some(blood), Some(visuals)) = (creatures, blood, visuals) else {
        for _ in swings.read() {} // tables not loaded: drain, do not backlog
        return;
    };
    for SwingImpact {
        swing, text_only, ..
    } in swings.read()
    {
        if *text_only {
            continue; // a flush carries only the floating text
        }
        // The client's gate (`0x624530`): `HitInfo & 0x2`, damage, victim state 1 or 4 (interrupt).
        if swing.hit_info & 0x2 == 0 || swing.damage == 0 || !matches!(swing.victim_state, 1 | 4) {
            if swing.damage > 0 {
                info!(
                    "blood: gate dropped a damaging swing (hit_info {:#x}, victim_state {}, damage {})",
                    swing.hit_info, swing.victim_state, swing.damage
                );
            }
            continue;
        }
        let Some(victim) = swing.victim else {
            info!("blood: dropped — victim entity unresolved");
            continue;
        };
        let Some(display_id) = net.get(victim).ok().and_then(|n| n.display_id) else {
            info!("blood: dropped — victim carries no net display id");
            continue;
        };
        let Some((disp_blood, model_blood)) = creatures.blood_candidates(display_id) else {
            info!("blood: dropped — display {display_id} unknown to the creature catalog");
            continue;
        };
        // The three-tier row resolve (`0x60afb0`). Its tier-3 records-base fallback is real blood,
        // never "no blood": Quilboar, crocolisks and gnolls land there.
        let Some(blood_id) = blood.0.level_key(disp_blood, model_blood) else {
            info!("blood: dropped — UnitBloodLevels is empty");
            continue;
        };
        let blood_id = blood_id as i32;
        // Front or back: the sign of `victimForward · (attackerPos − victimPos)` in WoW space,
        // where a unit's Y rotation is its WoW yaw. An unresolved attacker counts as front.
        let front = match (transforms.get(victim), swing_attacker(&transforms, swing)) {
            (Ok(vt), Some(at)) => {
                let yaw = vt.rotation.to_euler(EulerRot::YXZ).0;
                let (v, a) = (bevy_to_wow(vt.translation), bevy_to_wow(at.translation));
                yaw.cos() * (a[0] - v[0]) + yaw.sin() * (a[1] - v[1]) >= 0.0
            }
            _ => true,
        };
        let large = swing.hit_info & 0x2000 != 0; // crushing picks the large row; a crit does not
        let Some((effect, path)) = blood
            .0
            .effect_id(blood_id, violence.0, front, large)
            .and_then(|id| visuals.0.effect_path(id).map(|path| (id, path)))
        else {
            info!("blood: dropped — no effect for blood {blood_id} (front {front}, large {large})");
            continue;
        };
        debug!("blood: spurt {path} (blood {blood_id}, front {front}, large {large})");
        fx.write(SpellKitFx::Begin {
            entity: victim,
            spell_id: 0, // no spell: a self-terminating effect is never reaped by id
            persistent: false,
            class: super::FxClass::Hold,
            // Not a kit stage (`CEffect::AddEffect` off the melee path), but it too plays once.
            stage: super::FxStage::OneShot,
            // One spurt per (record, tag) on a body: a new one replaces the old (`0x6208e0`, which
            // `resolve_spell_fx` runs for every slot), so a busy fight does not stack them.
            effects: vec![FxSlot {
                tag: if front { ATTACH_FRONT } else { ATTACH_BACK },
                effect,
                path: path.to_string(),
            }],
            // A swing's impact, presented in the frame after every packet.
            at: None,
        });
    }
}

fn swing_attacker<'a>(
    transforms: &'a Query<&Transform>,
    swing: &SwingMessage,
) -> Option<&'a Transform> {
    transforms.get(swing.attacker).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value below 0 or above the locale's cap becomes the cap, not 0 (`0x6c5b09`-`0x6c5b1b`);
    /// koKR's cap is 1.
    #[test]
    fn violence_level_clamps_to_the_locale_cap() {
        for (value, locale, want) in [
            (2, 0, 2),
            (1, 0, 1),
            (0, 0, 0),
            (3, 0, 2),
            (-1, 0, 2),
            (2, 1, 1),
            (0, 1, 0),
            (-5, 1, 1),
            (9, 7, 2),
        ] {
            assert_eq!(clamp_violence(value, locale), want, "{value} on {locale}");
        }
    }
}
