//! The melee **blood spurt** (decision 0137 phase 3) — the fourth element of the victim-feedback
//! set (`0x624530 → 0x625010`): a landed melee blow
//! hangs a small particle-emitter model (`Particles\BloodSpurts\*.mdx`) on the victim, front or
//! back by where the attacker stands, sized by the crushing bit, colored by the creature's blood
//! type. Independent of the wound flinch and the floating text; rides the 0122 kit-effect spawn
//! machinery ([`SpellKitFx::Begin`], self-terminating — the spurt runs its clip span and dies).

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::BloodCatalog;

use crate::net::NetEntity;
use benilla_assets::{LockRecover, WorldAssets};

use super::spell_visual::FxSlot;
use super::spell_visual::SpellVisuals;
use super::{SpellKitFx, SwingImpact, SwingMessage};

/// The gore level: the client's `violenceLevel` cvar (0 none · 1 censored green · 2 true colors).
///
/// **2 is the reference's own default, not merely ours** (1859; the rule is 1804). The
/// registration at `0x6c5aa0` *formats* its default out of a per-region **maximum** table at
/// `0x86c3f8` — `[2, 1, 2, 2, 2, 2, 2, 2]` — and the setter `0x6c5af0` clamps to that same entry,
/// so a stock client boots at the highest gore its region permits. The locale table at `0x8558a4`
/// is `{enUS, koKR, frFR, …}` and `[0xc0e080]` is BSS-zero, so **enUS defaults to 2**; the single
/// capped entry is koKR, which at 1 maps red → *green* through UnitBloodLevels' middle column —
/// the censored-locale green blood, which this chain produces without being fitted to it.
/// Hardcoded rather than carried as a CVar row because
/// `cvars::REGISTERED` takes one row per knob a settings page actually wires (0137 phase 3).
const VIOLENCE_LEVEL: usize = 2;

/// The victim-model M2 attachment ids the spurt hangs on (`0x625010`: CEffect at
/// attach tag 0xf front / 0x10 back — present on every character and creature model checked).
const ATTACH_FRONT: u16 = 15;
const ATTACH_BACK: u16 = 16;

/// The UnitBlood/UnitBloodLevels tables (`None` until the startup load lands; absent = no spurts,
/// like every optional DBC face).
#[derive(Resource)]
pub(super) struct BloodTables(pub(super) BloodCatalog);

/// Load the blood tables off the patch chain at startup (the [`super::spell_visual`] pattern).
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

/// Spawn the spurt per landed swing — at the swing clip's **impact keyframe** ([`SwingImpact`],
/// not the raw packet: the blood flies when the blow lands, ~300–600 ms into the swing). The
/// client's gate verbatim — `HitInfo & 0x2`, nonzero damage, victimState ∈ {1, 4} — then the DBC
/// chain (the victim display's two blood candidates → [`BloodCatalog::level_key`]'s three-tier
/// row resolve → the violence-leveled UnitBloodLevels row → UnitBlood's front/back × small/large
/// effect), attached self-terminating on the victim. A missing link anywhere (an unknown display,
/// a censored violence level) drops the spurt — like the client's NULL-record skips, but
/// *audibly*: every dropped damaging swing logs its reason at
/// `info` and every fired spurt at `debug`, so "I never see blood" localizes to a link in one
/// fight instead of a code audit (no drop lines at all ⇒ the break is upstream, in the
/// [`SwingImpact`] feed itself).
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
        for _ in swings.read() {} // tables not up yet — drain, don't backlog
        return;
    };
    for SwingImpact {
        swing, text_only, ..
    } in swings.read()
    {
        if *text_only {
            continue; // a supersede/stop flush carries only the floating text
        }
        // The client's spawn gate (`0x624530`): HitInfo&0x2 (damage landed), nonzero damage,
        // victimState 1 (normal) or 4 (block-ish partial) — dodges/parries/misses spurt nothing.
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
        // The reference's three-tier row resolve (`0x60afb0`), including the tier-3 records-base
        // fallback that 595 of the 10534 shipped displays land on — Quilboar, crocolisks, gnolls
        // and Stranglethorn trolls among them, which is why it cannot mean "no blood" (1859).
        let Some(blood_id) = blood.0.level_key(disp_blood, model_blood) else {
            info!("blood: dropped — UnitBloodLevels is empty");
            continue;
        };
        let blood_id = blood_id as i32;
        // Front or back: the client's `sign(victimForward · (attackerPos − victimPos))`, in WoW
        // space — a unit Transform's Y rotation *is* its WoW yaw (net/motion's pose convention),
        // so forward = (cos θ, sin θ) against the WoW-mapped position delta. An unresolvable
        // attacker (despawned mid-flight) defaults to front.
        let front = match (transforms.get(victim), swing_attacker(&transforms, swing)) {
            (Ok(vt), Some(at)) => {
                let yaw = vt.rotation.to_euler(EulerRot::YXZ).0;
                let (v, a) = (bevy_to_wow(vt.translation), bevy_to_wow(at.translation));
                yaw.cos() * (a[0] - v[0]) + yaw.sin() * (a[1] - v[1]) >= 0.0
            }
            _ => true,
        };
        let large = swing.hit_info & 0x2000 != 0; // HITINFO crushing — the Large row, not crit
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
            spell_id: 0, // no spell — a self-terminating effect is never reaped by id
            persistent: false,
            class: super::FxClass::Hold,
            // The blood spurt is `CEffect::AddEffect` off the melee path, not a kit stage — but it
            // is the same self-terminating shape (one pass, then gone).
            stage: super::FxStage::OneShot,
            // One spurt per (record, tag) on a body: five attackers hitting the same flank
            // REPLACE each other's spurt rather than stacking five copies of it — the
            // reference's `0x6208e0` walk, which `resolve_spell_fx` runs for every slot
            // (decision 2057). It is why a busy fight does not brighten without bound there.
            effects: vec![FxSlot {
                tag: if front { ATTACH_FRONT } else { ATTACH_BACK },
                effect,
                path: path.to_string(),
            }],
        });
    }
}

/// The attacker's transform, if the entity still exists.
fn swing_attacker<'a>(
    transforms: &'a Query<&Transform>,
    swing: &SwingMessage,
) -> Option<&'a Transform> {
    transforms.get(swing.attacker).ok()
}
