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

/// The gore level, the client's `violenceLevel` (0 none, 1 green, 2 true colors). The reference
/// defaults it to its region's maximum (`0x6c5aa0`, from the table at `0x86c3f8`, which the setter
/// `0x6c5af0` also clamps to): 2 for enUS, 1 only for koKR, whose blood turns green. Fixed here,
/// not a CVar row: `cvars::REGISTERED` holds only the knobs a settings page wires.
const VIOLENCE_LEVEL: usize = 2;

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
            .effect_id(blood_id, VIOLENCE_LEVEL, front, large)
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
        });
    }
}

fn swing_attacker<'a>(
    transforms: &'a Query<&Transform>,
    swing: &SwingMessage,
) -> Option<&'a Transform> {
    transforms.get(swing.attacker).ok()
}
