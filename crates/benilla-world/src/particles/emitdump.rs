//! `$WOW_EMIT_DUMP=<label-substring>[,<period-seconds>]`: what the emission front end decided.
//! Every period (default 2 s), one line per live emitter whose model's
//! [`crate::interact::WorldObject`] label (a model path, or a creature's name) contains the
//! substring, case-insensitively: the resolved sequence slot, the tracks sampled at it, the count.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_formats::{ParamsNow, ParticleEmitterDef};

/// Parsed `$WOW_EMIT_DUMP`: (lowercased label substring, period seconds); `None` is off.
static FILTER: std::sync::LazyLock<Option<(String, f32)>> = std::sync::LazyLock::new(|| {
    let v = std::env::var("WOW_EMIT_DUMP").ok()?;
    // Only a trailing number is a period: a model path may contain a comma.
    let (label, period) = match v
        .rsplit_once(',')
        .map(|(l, p)| (l, p.trim().parse::<f32>()))
    {
        Some((l, Ok(p))) => (l, p),
        _ => (v.as_str(), 2.0),
    };
    Some((label.trim().to_ascii_lowercase(), period.max(0.1)))
});

/// The label lookup the instrument filters by and its period clock, as one [`SystemParam`].
#[derive(SystemParam)]
pub(super) struct EmitDump<'w, 's> {
    /// The model's inspector label, which rides its drawn parts, not the emitter's host entity.
    labels: Query<'w, 's, &'static crate::interact::WorldObject>,
    parts: Query<'w, 's, &'static Children>,
    last: Local<'s, f32>,
}

impl EmitDump<'_, '_> {
    /// Is this frame a dump tick (advancing the period clock)? Always false without the env.
    pub(super) fn due(&mut self, now: f32) -> bool {
        let Some((_, period)) = FILTER.as_ref() else {
            return false;
        };
        if now - *self.last < *period {
            return false;
        }
        *self.last = now;
        true
    }

    /// The owner's own label, else the first labelled drawn part under it.
    fn label(&self, owner: Option<Entity>) -> &str {
        let Some(e) = owner else { return "" };
        if let Ok(o) = self.labels.get(e) {
            return o.label.as_str();
        }
        self.parts
            .get(e)
            .ok()
            .and_then(|kids| kids.iter().find_map(|k| self.labels.get(k).ok()))
            .map_or("", |o| o.label.as_str())
    }

    /// Print one emitter's decision line, if its owning model matches the filter.
    pub(super) fn dump(&self, owner: Option<Entity>, d: &Decision<'_>) {
        let Some((want, _)) = FILTER.as_ref() else {
            return;
        };
        let label = self.label(owner);
        if !label.to_ascii_lowercase().contains(want.as_str()) {
            return;
        }
        let n = d.now;
        println!(
            "emit {label} bone {bone:<3} seq {seq} t={t:.3}s · rate {rate:.2}/s {gate} · live \
             {live:<4} · life {life:.2} speed {speed:.3}±{var:.2} lat {lat:.3} lon {lon:.3} \
             grav {grav:.2} area {al:.3}x{aw:.3} · size {s0:.3}/{s1:.3}/{s2:.3} · at \
             [{x:.2},{y:.2},{z:.2}]",
            bone = d.def.bone,
            seq = d.seq.map_or("-".to_string(), |s| s.to_string()),
            t = d.elapsed,
            rate = d.rate,
            gate = if d.emitting { "ON " } else { "off" },
            live = d.live,
            life = n.lifespan,
            speed = n.emission_speed,
            var = n.speed_variation,
            lat = n.vertical_range,
            lon = n.horizontal_range,
            grav = n.gravity,
            al = n.area_length,
            aw = n.area_width,
            s0 = d.def.over_life.scale[0],
            s1 = d.def.over_life.scale[1],
            s2 = d.def.over_life.scale[2],
            x = d.at.x,
            y = d.at.y,
            z = d.at.z,
        );
    }
}

/// One emitter's decision this frame: the sim's live values, not the authored ones.
pub(super) struct Decision<'a> {
    pub(super) def: &'a ParticleEmitterDef,
    /// The sequence file slot the rate, gate and params tracks resolved to; `None` degrades to 0.
    pub(super) seq: Option<usize>,
    /// Seconds into that slot's baked loop.
    pub(super) elapsed: f32,
    pub(super) rate: f32,
    pub(super) emitting: bool,
    pub(super) live: usize,
    pub(super) now: &'a ParamsNow,
    /// The emitter origin's live world position.
    pub(super) at: Vec3,
}
