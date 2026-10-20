//! The combat log's floating-number leg: the callers of `SubmitWorldText` (`0x6c7840`), one fn
//! per packet. Each classifies the damage source (self, owned by me, or anything else, which is
//! never drawn), resolves the recipient, applies Gate A (no damage text over self), picks the
//! category and colour and writes a [`CombatTextSpawn`]. XP is the exception: over self, no gates,
//! its own row colour. The law itself lives in [`crate::combat_text`].
//!
//! Each spell arm also feeds `UNIT_COMBAT` ([`UnitCombatFeedback`], the portrait hit indicator),
//! ungated and fired at packet receive; the melee arm's rides the swing impact keyframe instead
//! (`ui_unit::melee_unit_combat`).

use bevy::prelude::*;

use benilla_protocol::messages::{
    DamageShield, ExplorationXp, LevelUpInfo, PeriodicAuraLog, PeriodicTick, SpellDamageLog,
    SpellHealLog, SpellLogMiss, XpGain,
};

use crate::combat_text::{
    damage_color, miss_word, spell_text, CombatTextSpawn, DamageSource, DamageTextGates,
};
use crate::ui_chat::{combat, ChatLog};
use crate::ui_unit::UnitCombatFeedback;

use crate::net::{GuidIndex, ObjectStore, SelfGuid};

/// Gate A: the recipient's entity, unless the recipient is us (`0x607140` and `0x6128b0` compare
/// the anchor guid to the active player and return before submitting).
fn gated_anchor(guid: u64, index: &GuidIndex, self_guid: &SelfGuid) -> Option<Entity> {
    if self_guid.0 == Some(guid) {
        return None;
    }
    index.0.get(&guid).copied()
}

/// The source-ownership classifier `0x5efea0` (the colour law's `K`): `Player` for me, `Pet` for a
/// unit whose SummonedBy or CreatedBy is me (pet, guardian, totem), `None` for anything else,
/// whose damage is never drawn.
pub(crate) fn classify_source(
    guid: u64,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
) -> Option<DamageSource> {
    let me = self_guid.0?;
    if guid == me {
        return Some(DamageSource::Player);
    }
    let entity = index.0.get(&guid).copied()?;
    let store = stores.get(entity).ok()?;
    (store.0.unit_summoned_by() == Some(me) || store.0.unit_created_by() == Some(me))
        .then_some(DamageSource::Pet)
}

// The melee number and word (`SMSG_ATTACKERSTATEUPDATE`) spawn at the swing's impact keyframe
// (`crate::combat_text::melee_impact_text`), not here.

/// The colour law's `B` bit for a spell-packet emit, from the spell's record via
/// [`crate::combat_text::melee_styled`]: a ranged basic shot floats white, a spell gold.
fn melee_styled(spells: Option<&crate::ui_action::Spells>, spell_id: u32) -> bool {
    crate::combat_text::melee_styled(spells.and_then(|s| s.catalog.get(spell_id)))
}

/// `UNIT_COMBAT`'s descriptor, as its one emitter `0x494600` picks it from the packet's flags:
/// with an amount, `CRITICAL` (`0x80`), `GLANCING` (`0x4000`), `CRUSHING` (`0x8000`); without one,
/// `ABSORB` (`0x20`), `BLOCK` (`0x800`), `RESIST` (`0x40`); else empty, which the stock portrait
/// shows as "Miss" (`CombatFeedback.lua:57-58`).
pub(crate) fn wound_descriptor(amount: u32, flags: u32) -> &'static str {
    // The amount is tested signed (`0x49460a`, `jle`).
    let tests: [(u32, &str); 3] = if amount as i32 > 0 {
        [
            (0x80, "CRITICAL"),
            (0x4000, "GLANCING"),
            (0x8000, "CRUSHING"),
        ]
    } else {
        [(0x20, "ABSORB"), (0x800, "BLOCK"), (0x40, "RESIST")]
    };
    tests
        .into_iter()
        .find(|&(bit, _)| flags & bit != 0)
        .map_or("", |(_, word)| word)
}

/// The flags a spell hit hands `0x494600`: crit `0x80`, and each partial nonzero, absorbed `0x20`,
/// blocked `0x800`, resisted `0x40` (`0x5e87fa`-`0x5e8825`).
fn hit_flags(crit: bool, absorb: u32, resist: i32, blocked: u32) -> u32 {
    let bit = |set: bool, bit: u32| if set { bit } else { 0 };
    bit(crit, 0x80) | bit(absorb != 0, 0x20) | bit(blocked != 0, 0x800) | bit(resist != 0, 0x40)
}

/// `SMSG_SPELLNONMELEEDAMAGELOG` (`0x5e85e0`), after its line: for a spell with a record and no
/// `Attributes & 0x100`, `UNIT_COMBAT` `WOUND` over the target, and the spell-damage number or the
/// stopped hit's word over it; only my spells and my pet's (`PetSpellDamage`-gated) draw, number
/// and word coloured alike. Split damage flashes `WOUND` with no flags and floats nothing
/// (`0x5e872d`-`0x5e876a`).
pub(super) fn spell_damage_log(
    s: SpellDamageLog,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
) {
    let crit = s.hit_info & 0x2 != 0;
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "fct",
            &format!(
                "recv spelldmg atk={:#x} target={:#x} dmg={} crit={crit}",
                s.attacker, s.target, s.damage
            ),
        );
    }
    let Some(display) = combat::feedback_spell(spells, s.spell_id) else {
        return;
    };
    let split = s.hit_info & 0x8 != 0;
    // UNIT_COMBAT: any source, any recipient including self.
    if let Some(&unit) = index.0.get(&s.target) {
        let flags = if split {
            0
        } else {
            hit_flags(crit, s.absorb, s.resist, s.blocked)
        };
        feedback.write(UnitCombatFeedback {
            unit,
            action: "WOUND",
            flags: wound_descriptor(s.damage, flags),
            amount: s.damage,
            school: u32::from(s.school),
        });
    }
    if split {
        return;
    }
    let Some(source) = classify_source(s.attacker, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    // A ranged basic shot (Throw, Auto Shot: `AttributesEx3 & 0x8000`) floats white.
    let melee = crate::combat_text::melee_styled(Some(display));
    let Some(color) = damage_color(gates, source, melee) else {
        return; // the CombatDamage / PetSpellDamage gates
    };
    if let (Some(anchor), Some((category, body))) = (
        gated_anchor(s.target, index, self_guid),
        spell_text(s.damage, s.absorb, s.resist, s.blocked, crit),
    ) {
        text.write(CombatTextSpawn {
            anchor,
            text: body,
            category,
            color,
        });
    }
}

/// `SMSG_PERIODICAURALOG` (`0x626dd0`): per tick of a spell with a record and no
/// `Attributes & 0x100` (`0x626f1b`, `0x62700f`, `0x627095`), a damage tick floats like direct
/// damage, never as a crit, and flashes `WOUND` with its absorbed and resisted flags
/// (`0x626f87`-`0x626fb4`); a heal tick flashes `HEAL` (`0x627033`) and floats nothing, as heals
/// never float in the 1.12 client. Energize (`0x494770` is an empty stub) and leech ticks do
/// neither.
pub(super) fn periodic_aura_log(
    s: PeriodicAuraLog,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
) {
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "fct",
            &format!(
                "recv periodic caster={:#x} target={:#x} ticks={}",
                s.caster,
                s.target,
                s.ticks.len()
            ),
        );
    }
    let Some(display) = combat::feedback_spell(spells, s.spell_id) else {
        return;
    };
    if let Some(&unit) = index.0.get(&s.target) {
        for tick in &s.ticks {
            let (action, flags, amount, school) = match *tick {
                PeriodicTick::Damage {
                    amount,
                    school,
                    absorb,
                    resist,
                } => (
                    "WOUND",
                    wound_descriptor(amount, hit_flags(false, absorb, resist, 0)),
                    amount,
                    school,
                ),
                PeriodicTick::Heal { amount } => ("HEAL", "", amount, 0),
                PeriodicTick::Energize { .. } | PeriodicTick::ManaLeech { .. } => continue,
            };
            feedback.write(UnitCombatFeedback {
                unit,
                action,
                flags,
                amount,
                school,
            });
        }
    }
    let Some(source) = classify_source(s.caster, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    // `0x626dd0` pushes the spell's record to the same `0x6128b0` law as direct damage.
    let melee = crate::combat_text::melee_styled(Some(display));
    let Some(color) = damage_color(gates, source, melee) else {
        return;
    };
    let Some(anchor) = gated_anchor(s.target, index, self_guid) else {
        return;
    };
    for tick in &s.ticks {
        let PeriodicTick::Damage {
            amount,
            absorb,
            resist,
            ..
        } = *tick
        else {
            continue;
        };
        if let Some((category, body)) = spell_text(amount, absorb, resist, 0, false) {
            text.write(CombatTextSpawn {
                anchor,
                text: body,
                category,
                color,
            });
        }
    }
}

/// `SMSG_SPELLDAMAGESHIELD`: the number floats over the attacker, who struck the shield; the
/// source is the shield bearer (the victim field). The site pushes a NULL record, so it colours
/// melee-styled: my shield white, my pet's orange (`0x5e84e0`).
pub(super) fn damage_shield(
    s: DamageShield,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
) {
    if s.damage == 0 {
        return;
    }
    // UNIT_COMBAT: the attacker takes the damage, so striking thorns wounds your own portrait.
    if let Some(&unit) = index.0.get(&s.attacker) {
        feedback.write(UnitCombatFeedback {
            unit,
            action: "WOUND",
            flags: "",
            amount: s.damage,
            school: s.school,
        });
    }
    let Some(source) = classify_source(s.victim, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    let Some(color) = damage_color(gates, source, true) else {
        return;
    };
    if let Some(anchor) = gated_anchor(s.attacker, index, self_guid) {
        text.write(CombatTextSpawn {
            anchor,
            text: s.damage.to_string(),
            category: 0,
            color,
        });
    }
}

/// `SMSG_SPELLHEALLOG`: `UNIT_COMBAT` `HEAL` for any streamed recipient (`0x5e8a48`), with no
/// spell gate. No worldtext: heals never float in the 1.12 client.
pub(super) fn spell_heal_log(
    s: SpellHealLog,
    index: &GuidIndex,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
) {
    if let Some(&unit) = index.0.get(&s.target) {
        feedback.write(UnitCombatFeedback {
            unit,
            action: "HEAL",
            flags: if s.critical { "CRITICAL" } else { "" },
            amount: s.amount,
            school: 0,
        });
    }
}

/// Miss code (vmangos `SpellMissInfo`, 1 to 11) to the `UNIT_COMBAT` action word, the worldtext
/// words' table in the event's uppercase keys (`CombatFeedbackText`).
fn miss_action(code: u8) -> Option<&'static str> {
    Some(match code {
        1 => "MISS",
        2 => "RESIST",
        3 => "DODGE",
        4 => "PARRY",
        5 => "BLOCK",
        6 => "EVADE",
        7 | 8 => "IMMUNE",
        9 => "DEFLECT",
        10 => "ABSORB",
        11 => "REFLECT",
        _ => return None,
    })
}

/// `SMSG_SPELLLOGMISS`: one outcome word per missed target (`0x5e7e00`); vmangos sends only
/// impact-time outcomes (immune, evade) here. Coloured by the same law as a number: `0x5e7f66`
/// pushes the resolved spell record, not NULL, so the words are spell gold.
pub(super) fn spell_log_miss(
    s: SpellLogMiss,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
) {
    // UNIT_COMBAT for each missed target, any source.
    for &(target, code) in &s.misses {
        if let (Some(&unit), Some(action)) = (index.0.get(&target), miss_action(code)) {
            feedback.write(UnitCombatFeedback {
                unit,
                action,
                flags: "",
                amount: 0,
                school: 0,
            });
        }
    }
    // The word emitter `0x607140` checks the same three CVars as the number emitter `0x6128b0`:
    // `CombatDamage` (`0x60718f`), `PetSpellDamage` (`0x6071a6`), `PetMeleeDamage` (`0x6071c5`).
    let Some(source) = classify_source(s.caster, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    let Some(color) = damage_color(gates, source, melee_styled(spells, s.spell_id)) else {
        return; // the CombatDamage / Pet* gates
    };
    for &(target, code) in &s.misses {
        if let (Some(anchor), Some((word, category))) =
            (gated_anchor(target, index, self_guid), miss_word(code))
        {
            text.write(CombatTextSpawn {
                anchor,
                text: word.to_string(),
                category,
                color,
            });
        }
    }
}

/// `SMSG_LOG_XPGAIN`: `"XP: %d"` over self, category 4, the one emitter that skips Gate A
/// (`0x607260`), for kill and quest XP alike; the packet also writes the chat line.
pub(super) fn xp_gain(
    x: XpGain,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    text: &mut MessageWriter<CombatTextSpawn>,
    chat_log: &mut ChatLog,
) {
    if let Some(&me) = self_guid.0.as_ref().and_then(|g| index.0.get(g)) {
        text.write(CombatTextSpawn {
            anchor: me,
            text: format!("XP: {}", x.total),
            category: 4,
            color: None, // row 4's own purple: XP skips the damage colour law
        });
    }
    chat_log.push_xp_gain(&x);
}

/// `SMSG_EXPLORATION_EXPERIENCE`, as the reference's case body `[0x5e41d2, 0x5e42bb]`:
/// - the ERR_ZONE_EXPLORED toast and, when xp > 0, the ERR_ZONE_EXPLORED_XP chat line, via
///   [`ChatLog::push_exploration`]; an `AreaTable.dbc` id with no name shows no message;
/// - the race's discovery jingle (`ChrRaces.dbc` col 3 to SoundEntries), played even when the
///   area resolves to nothing, through the same 2D path as `SMSG_PLAY_SOUND`.
///
/// No floating text: the XP arrives separately as `SMSG_LOG_XPGAIN`.
pub(super) fn exploration_xp(
    x: ExplorationXp,
    area_table: Option<&crate::area::AreaTableRes>,
    sounds: Option<&crate::sound::ExplorationSounds>,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    sound_out: &mut MessageWriter<crate::net::ServerSoundMessage>,
    chat_log: &mut ChatLog,
) {
    let race = self_guid
        .0
        .as_ref()
        .and_then(|g| index.0.get(g))
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.unit_race());
    let kit = race.and_then(|r| sounds.and_then(|s| s.0.kit(u32::from(r))));
    if let Some(kit) = kit {
        sound_out.write(crate::net::ServerSoundMessage {
            kind: crate::net::ServerSoundKind::Sound2d,
            sound_id: kit,
            source: None,
        });
    }
    match area_table.and_then(|t| t.0.name(x.area_id)) {
        Some(name) => {
            info!(
                "exploration: discovered {name} (area {}, {} xp, jingle kit {})",
                x.area_id,
                x.xp,
                kit.unwrap_or(0)
            );
            chat_log.push_exploration(name, x.xp);
        }
        None => warn!(
            "exploration: discovered area {} has no AreaTable name — no message (sound only)",
            x.area_id
        ),
    }
}

/// `SMSG_LEVELUP_INFO`: the ding's chat lines. The talent-point arg is not on the wire; the client
/// computes `(newLevel >= 10) ? 1 : 0` (`0x5e407c`). The ding's visual rides the
/// `UNIT_FIELD_LEVEL` change watcher, not this packet.
pub(super) fn level_up(l: LevelUpInfo, chat_log: &mut ChatLog) {
    let talent_points = u32::from(l.level >= 10);
    // Parked for `ui_unit`'s `PLAYER_LEVEL_UP`, whose args the stock `ChatFrame_OnEvent` prints;
    // nothing here prints the block itself, or it shows twice.
    chat_log.push_level_up_gains(&l, talent_points);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat_text::COLOR_SPELL_GOLD;
    use crate::net::{Guid, SelfPlayer};
    use bevy::ecs::system::RunSystemOnce;

    /// `SMSG_SPELLLOGMISS`'s word is spell gold: `0x5e7f66` pushes the resolved spell record, not
    /// the melee site's NULL, and unlike the GO's inline emit there is no speed test here.
    #[test]
    fn the_spell_log_miss_word_is_spell_gold() {
        const IMMOLATE: u32 = 348;

        let spells = crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(
                    IMMOLATE,
                    benilla_formats::SpellDisplay {
                        name: "Immolate".into(),
                        speed: 24.0, // travels, and still prints here
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };

        let mut app = App::new();
        app.add_message::<CombatTextSpawn>()
            .add_message::<UnitCombatFeedback>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>();
        let self_e = app
            .world_mut()
            .spawn((Guid(10), SelfPlayer, ObjectStore::default()))
            .id();
        let victim_e = app
            .world_mut()
            .spawn((Guid(20), ObjectStore::default()))
            .id();
        {
            let mut index = app.world_mut().resource_mut::<GuidIndex>();
            index.0.insert(10, self_e);
            index.0.insert(20, victim_e);
        }
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

        app.world_mut()
            .run_system_once(
                move |index: Res<GuidIndex>,
                      self_guid: Res<SelfGuid>,
                      stores: Query<&mut ObjectStore>,
                      mut text: MessageWriter<CombatTextSpawn>,
                      mut feedback: MessageWriter<UnitCombatFeedback>| {
                    spell_log_miss(
                        SpellLogMiss {
                            spell_id: IMMOLATE,
                            caster: 10,
                            misses: vec![(20, 7)], // IMMUNE, an outcome vmangos sends here
                        },
                        &index,
                        &self_guid,
                        &stores,
                        Some(&spells),
                        DamageTextGates::default(),
                        &mut text,
                        &mut feedback,
                    );
                },
            )
            .unwrap();

        let spawned: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<CombatTextSpawn>>()
            .drain()
            .map(|s| (s.text, s.category, s.color, s.anchor))
            .collect();
        assert_eq!(
            spawned,
            vec![("Immune".to_string(), 3, Some(COLOR_SPELL_GOLD), victim_e)],
            "gold, category 3, over the immune target"
        );
    }

    const ME: u64 = 10;
    const MOB: u64 = 20;
    const FIREBALL: u32 = 133;
    const HIDDEN: u32 = 7;

    /// You and a creature held by guid, and Fireball with a record, `HIDDEN` flagged `0x100`.
    struct Leg {
        app: App,
        spells: std::sync::Arc<crate::ui_action::Spells>,
    }

    impl Leg {
        fn new() -> Self {
            let display = |id, name: &str, attributes| {
                (
                    id,
                    benilla_formats::SpellDisplay {
                        id,
                        name: name.into(),
                        attributes,
                        ..Default::default()
                    },
                )
            };
            let spells = crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [
                        display(FIREBALL, "Fireball", 0),
                        display(HIDDEN, "Hidden", 0x100),
                    ]
                    .into_iter()
                    .collect(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            };
            let mut app = App::new();
            app.add_message::<CombatTextSpawn>()
                .add_message::<UnitCombatFeedback>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>();
            for guid in [ME, MOB] {
                let e = app
                    .world_mut()
                    .spawn((Guid(guid), ObjectStore::default()))
                    .id();
                app.world_mut()
                    .resource_mut::<GuidIndex>()
                    .0
                    .insert(guid, e);
            }
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
            Self {
                app,
                spells: std::sync::Arc::new(spells),
            }
        }

        /// Run one packet through the leg; the flashes as `(action, descriptor, amount)` and the
        /// floats' text.
        fn run(
            &mut self,
            packet: impl Packet,
        ) -> (Vec<(&'static str, &'static str, u32)>, Vec<String>) {
            let spells = self.spells.clone();
            let mut packet = Some(packet);
            self.app
                .world_mut()
                .run_system_once(
                    move |index: Res<GuidIndex>,
                          self_guid: Res<SelfGuid>,
                          stores: Query<&mut ObjectStore>,
                          mut text: MessageWriter<CombatTextSpawn>,
                          mut feedback: MessageWriter<UnitCombatFeedback>| {
                        if let Some(packet) = packet.take() {
                            packet.emit(
                                &index,
                                &self_guid,
                                &stores,
                                &spells,
                                &mut text,
                                &mut feedback,
                            );
                        }
                    },
                )
                .unwrap();
            let world = self.app.world_mut();
            let flashes = world
                .resource_mut::<Messages<UnitCombatFeedback>>()
                .drain()
                .map(|f| (f.action, f.flags, f.amount))
                .collect();
            let floats = world
                .resource_mut::<Messages<CombatTextSpawn>>()
                .drain()
                .map(|t| t.text)
                .collect();
            (flashes, floats)
        }
    }

    /// A packet the leg takes.
    trait Packet: Send + Sync + 'static {
        fn emit(
            self,
            index: &GuidIndex,
            self_guid: &SelfGuid,
            stores: &Query<&mut ObjectStore>,
            spells: &crate::ui_action::Spells,
            text: &mut MessageWriter<CombatTextSpawn>,
            feedback: &mut MessageWriter<UnitCombatFeedback>,
        );
    }

    impl Packet for SpellDamageLog {
        fn emit(
            self,
            index: &GuidIndex,
            self_guid: &SelfGuid,
            stores: &Query<&mut ObjectStore>,
            spells: &crate::ui_action::Spells,
            text: &mut MessageWriter<CombatTextSpawn>,
            feedback: &mut MessageWriter<UnitCombatFeedback>,
        ) {
            spell_damage_log(
                self,
                index,
                self_guid,
                stores,
                Some(spells),
                DamageTextGates::default(),
                text,
                feedback,
            );
        }
    }

    impl Packet for PeriodicAuraLog {
        fn emit(
            self,
            index: &GuidIndex,
            self_guid: &SelfGuid,
            stores: &Query<&mut ObjectStore>,
            spells: &crate::ui_action::Spells,
            text: &mut MessageWriter<CombatTextSpawn>,
            feedback: &mut MessageWriter<UnitCombatFeedback>,
        ) {
            periodic_aura_log(
                self,
                index,
                self_guid,
                stores,
                Some(spells),
                DamageTextGates::default(),
                text,
                feedback,
            );
        }
    }

    /// My spell on the creature.
    fn hit(spell_id: u32, damage: u32, absorb: u32, resist: i32, blocked: u32) -> SpellDamageLog {
        SpellDamageLog {
            target: MOB,
            attacker: ME,
            spell_id,
            damage,
            school: 2,
            absorb,
            resist,
            periodic: false,
            blocked,
            hit_info: 0,
        }
    }

    /// `0x5e85e0` flashes `WOUND` for any hit of a logged spell, its descriptor the first of
    /// `ABSORB`, `BLOCK` and `RESIST` when nothing got through (`0x49463b`-`0x494656`), and floats
    /// the same reason (`0x5e888a`-`0x5e88c5`); a hit with no reason at all flashes `WOUND` bare,
    /// the portrait's "Miss", and floats nothing.
    #[test]
    fn a_stopped_spell_hit_flashes_and_floats_its_reason() {
        let mut leg = Leg::new();
        assert_eq!(
            leg.run(hit(FIREBALL, 0, 0, 30, 40)),
            (vec![("WOUND", "BLOCK", 0)], vec!["Block".to_string()])
        );
        assert_eq!(
            leg.run(hit(FIREBALL, 0, 50, 30, 40)),
            (vec![("WOUND", "ABSORB", 0)], vec!["Absorb".to_string()])
        );
        assert_eq!(
            leg.run(hit(FIREBALL, 0, 0, 0, 0)),
            (vec![("WOUND", "", 0)], vec![])
        );
        // Through: the crit is the descriptor, and the partials are not.
        let crit = SpellDamageLog {
            hit_info: 0x2,
            ..hit(FIREBALL, 300, 50, 0, 0)
        };
        assert_eq!(
            leg.run(crit),
            (vec![("WOUND", "CRITICAL", 300)], vec!["300".to_string()])
        );
    }

    /// The flash and the float need the spell's record without `Attributes & 0x100`
    /// (`0x5e87e3`-`0x5e87f4`); split damage flashes `WOUND` with no flags and floats nothing
    /// (`0x5e8744`-`0x5e876a`).
    #[test]
    fn a_hidden_or_unknown_spell_flashes_nothing_and_a_split_floats_nothing() {
        let mut leg = Leg::new();
        assert_eq!(leg.run(hit(HIDDEN, 300, 0, 0, 0)), (vec![], vec![]));
        assert_eq!(leg.run(hit(24_000, 300, 0, 0, 0)), (vec![], vec![]));
        let split = SpellDamageLog {
            hit_info: 0x8 | 0x2,
            ..hit(FIREBALL, 120, 0, 0, 0)
        };
        assert_eq!(leg.run(split), (vec![("WOUND", "", 120)], vec![]));
    }

    /// A periodic tick flashes `WOUND` with its absorbed and resisted flags alone
    /// (`0x626f87`-`0x626fb4`), and a hidden spell's ticks flash and float nothing (`0x626f21`).
    #[test]
    fn a_periodic_tick_flashes_its_partials_for_a_logged_spell_only() {
        let mut leg = Leg::new();
        let tick = |spell_id| PeriodicAuraLog {
            target: MOB,
            caster: ME,
            spell_id,
            ticks: vec![
                PeriodicTick::Damage {
                    amount: 0,
                    school: 2,
                    absorb: 0,
                    resist: 12,
                },
                PeriodicTick::Heal { amount: 9 },
            ],
        };
        assert_eq!(
            leg.run(tick(FIREBALL)),
            (
                vec![("WOUND", "RESIST", 0), ("HEAL", "", 9)],
                vec!["Resist".to_string()]
            )
        );
        assert_eq!(leg.run(tick(HIDDEN)), (vec![], vec![]));
    }
}
