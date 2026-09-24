//! `spellvis`: dump a spell's visual chain — spell → SpellVisual stages → each kit's anim/sound/
//! camera shake/attach effects, plus the missile block — decision 0099's phase-2 instrument
//! (columns per decisions 0107 and 1849).

use anyhow::Result;
use benilla_formats::Chain;

/// Dump the spell's visual chain for `spell_id`.
pub fn run(chain: &mut Chain, spell_id: u32) -> Result<()> {
    let spells = benilla_formats::load_spell_catalog(chain)?;
    let visuals = benilla_formats::load_spell_visual_catalog(chain)?;
    let shakes = benilla_formats::load_camera_shakes(chain)?;
    let Some(spell) = spells.get(spell_id) else {
        anyhow::bail!("spell {spell_id} not in Spell.dbc");
    };
    println!(
        "spell {spell_id} \"{}\"  visual={}  speed={}",
        spell.name, spell.visual, spell.speed
    );
    let Some(stages) = visuals.stages(spell.visual) else {
        println!("  (no SpellVisual row — a silent cast)");
        return Ok(());
    };
    for (label, kit_id) in [
        ("precast", stages.precast),
        ("cast", stages.cast),
        ("impact", stages.impact),
        ("state", stages.state),
        ("channel", stages.channel),
    ] {
        if kit_id == 0 {
            println!("  {label:8} —");
        } else {
            match visuals.kit(kit_id) {
                Some(kit) => {
                    println!(
                        "  {label:8} kit {kit_id:<5} anim={:<12} sound={}",
                        kit.anim_id.map_or("—".into(), |a| a.to_string()),
                        kit.sound.map_or("—".into(), |s| s.to_string()),
                    );
                    // The kit's CAMERA SHAKE (field 14): a
                    // `SpellEffectCameraShakes` GROUP id, expanded to the presets it fires.
                    // `benilla-extract … shakecensus` is the whole-table view.
                    if let Some(group) = kit.shake {
                        match shakes.group(group) {
                            Some(g) => println!(
                                "           shake  group {group} -> presets {}",
                                g.shakes()
                                    .map(|id| id.to_string())
                                    .collect::<Vec<_>>()
                                    .join(" · ")
                            ),
                            None => println!(
                                "           shake  group {group} (NO SUCH SpellEffectCameraShakes ROW)"
                            ),
                        }
                    }
                    // The kit's CharProcs (fields 15-34): what it does to the BODY.
                    crate::charprocs::print_kit_procs(&visuals, kit_id, "           ");
                    // The kit's BEAM, if it draws one: the chain CharProc's
                    // decoded `SpellChainEffects` row. `benilla-extract … chaincensus` is the
                    // whole-table view.
                    if let Some(c) = kit.chain_proc() {
                        match visuals.chain_effect(c.effect_id) {
                            Some(e) => println!(
                                "           beam   chain {} x{} flag={} -> {} (segLen {} halfWidth {} noise {} scroll {}s hopLife {}ms hopStagger {}ms)",
                                c.effect_id,
                                c.beams,
                                u8::from(c.flag),
                                e.texture,
                                e.avg_seg_len,
                                e.half_width,
                                e.noise_scale,
                                e.scroll_period_s,
                                e.bolt_life_ms,
                                e.bolt_stagger_ms,
                            ),
                            None => println!(
                                "           beam   chain {} (NO SUCH SpellChainEffects ROW)",
                                c.effect_id
                            ),
                        }
                    }
                    // The kit's attach-point emitter slots (phase 3): tag + effect model.
                    for (tag, effect) in kit.effects() {
                        println!(
                            "           attach {tag:#04x} effect {effect:<5} -> {}",
                            visuals.effect_path(effect).unwrap_or("(MISSING PATH)"),
                        );
                    }
                }
                None => println!("  {label:8} kit {kit_id} (MISSING ROW)"),
            }
        }
    }
    // The missile block (phase 4): the projectile exists whenever Speed > 0; its model is
    // field 7's SpellVisualEffectName (else the ammo/ErrorCube fallbacks) and it homes to
    // field 9's dest-attach ordinal on a live target.
    if spell.speed > 0.0 {
        let tag = benilla_formats::MISSILE_ATTACH_TABLE
            .get(stages.missile_attach as usize)
            .copied();
        println!(
            "  missile  effect {:<5} -> {}   dest ordinal {} (attach {})",
            stages.missile_model,
            visuals
                .effect_path(stages.missile_model)
                .unwrap_or("(ammo/ErrorCube fallback)"),
            stages.missile_attach,
            tag.map_or("—".into(), |t| format!("{t:#04x}")),
        );
    }
    Ok(())
}
