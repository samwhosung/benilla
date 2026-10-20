//! The combat log's floating-number leg: the callers of `SubmitWorldText` (`0x6c7840`), one fn
//! per packet. Each classifies the damage source (self, owned by me, or anything else, which is
//! never drawn), resolves the recipient, applies Gate A (no damage text over self), picks the
//! category and colour and writes a [`CombatTextSpawn`]. XP is the exception: over self, no gates,
//! its own row colour. The law itself lives in [`crate::combat_text`]. A float reaches no Lua
//! handler, so where it falls against the packet's line and `UNIT_COMBAT` does not matter.

use bevy::prelude::*;

use benilla_protocol::messages::{
    DamageShield, ExplorationXp, LevelUpInfo, PeriodicAuraLog, PeriodicTick, SpellDamageLog,
    SpellLogMiss, XpGain,
};

use crate::combat_text::{
    damage_color, miss_word, spell_text, CombatTextSpawn, DamageSource, DamageTextGates, TextBody,
};
use crate::ui_chat::combat::watch::XpAwarded;
use crate::ui_chat::{combat, ChatLog};

use crate::net::{GuidIndex, ObjectStore, SelfGuid};

/// The float's anchor: the recipient as a unit the client holds, the handlers' `0x468460`
/// lookup with type mask 8 (`0x5e885b`, `0x5e856e`, `0x5e7f5a`, `0x6e7dbd`, `0x6e7e3a`; the
/// periodic handler's target, `0x626e8a`), and then Gate A, unless the recipient is us
/// (`0x607140` and `0x6128b0` compare the anchor guid to the active player and return before
/// submitting).
fn gated_anchor(
    guid: u64,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
) -> Option<Entity> {
    if self_guid.0 == Some(guid) || !combat::holds_unit(guid, index, stores) {
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

/// `SMSG_SPELLNONMELEEDAMAGELOG` (`0x5e85e0`): for a spell with a record and no
/// `Attributes & 0x100`, the spell-damage number or the stopped hit's word over the target; only
/// my spells and my pet's (`PetSpellDamage`-gated) draw, number and word coloured alike. Split
/// damage floats nothing (`0x5e872d`-`0x5e876a`).
pub(super) fn spell_damage_log(
    s: SpellDamageLog,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
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
    if s.hit_info & 0x8 != 0 {
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
        gated_anchor(s.target, index, self_guid, stores),
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

/// `SMSG_PERIODICAURALOG` (`0x626dd0`): per damage tick of a spell with a record and no
/// `Attributes & 0x100` (`0x626f1b`), the number floats like direct damage, never as a crit. Heal,
/// energize and leech ticks float nothing, as heals never float in the 1.12 client.
pub(super) fn periodic_aura_log(
    s: PeriodicAuraLog,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
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
    let Some(source) = classify_source(s.caster, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    // `0x626dd0` pushes the spell's record to the same `0x6128b0` law as direct damage.
    let melee = crate::combat_text::melee_styled(Some(display));
    let Some(color) = damage_color(gates, source, melee) else {
        return;
    };
    let Some(anchor) = gated_anchor(s.target, index, self_guid, stores) else {
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
) {
    if s.damage == 0 {
        return;
    }
    let Some(source) = classify_source(s.victim, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    let Some(color) = damage_color(gates, source, true) else {
        return;
    };
    if let Some(anchor) = gated_anchor(s.attacker, index, self_guid, stores) {
        text.write(CombatTextSpawn {
            anchor,
            text: TextBody::Literal(s.damage.to_string()),
            category: 0,
            color,
        });
    }
}

/// The colour a spell's miss words take, or `None` where `0x607140` draws nothing: the same
/// source class and three CVars as the number emitter, `CombatDamage` (`0x60718f`),
/// `PetSpellDamage` (`0x6071a6`) and `PetMeleeDamage` (`0x6071c5`), with the spell's record pushed,
/// so the words are spell gold.
fn miss_color(
    caster: u64,
    display: &benilla_formats::SpellDisplay,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    gates: DamageTextGates,
) -> Option<Option<u32>> {
    let source = classify_source(caster, index, self_guid, stores)?;
    damage_color(
        gates,
        source,
        crate::combat_text::melee_styled(Some(display)),
    )
}

/// `0x607140`'s miss word over `guid`, a unit the client holds and not you (Gate A).
fn miss_float(
    guid: u64,
    code: u8,
    color: Option<Option<u32>>,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    text: &mut MessageWriter<CombatTextSpawn>,
) {
    if let (Some(color), Some(anchor), Some((key, category))) = (
        color,
        gated_anchor(guid, index, self_guid, stores),
        miss_word(code),
    ) {
        text.write(CombatTextSpawn {
            anchor,
            text: TextBody::Word(key),
            category,
            color,
        });
    }
}

/// `SMSG_SPELLLOGMISS` (`0x5e7e00`), per missed target of a spell with a record and no
/// `Attributes & 0x100` (`0x5e7f0b`-`0x5e7f18`): the word over the target (`0x5e7f6e`).
pub(super) fn spell_log_miss(
    s: SpellLogMiss,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
) {
    let Some(display) = combat::feedback_spell(spells, s.spell_id) else {
        return;
    };
    let color = miss_color(s.caster, display, index, self_guid, stores, gates);
    for &(target, code) in &s.misses {
        miss_float(target, code, color, index, self_guid, stores, text);
    }
}

/// `SMSG_SPELL_GO`'s miss list (`0x6e7d65`-`0x6e7e71`), for a spell with a record and no
/// `Attributes & 0x100` (`0x6e7d9a`): per entry the word over the target (`0x6e7dd4`), and for a
/// reflect with an outcome that outcome's word over `reflected_to`, the packet's second guid
/// (`0x6e7e51`); `caster`, the list's, colours both.
pub(super) fn spell_go_misses(
    caster: u64,
    reflected_to: u64,
    spell_id: u32,
    misses: &[benilla_protocol::messages::SpellGoMiss],
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
) {
    let Some(display) = combat::feedback_spell(spells, spell_id) else {
        return;
    };
    let color = miss_color(caster, display, index, self_guid, stores, gates);
    for m in misses {
        miss_float(m.target, m.info, color, index, self_guid, stores, text);
        if m.info == SPELL_MISS_REFLECT && m.reflect_result != 0 {
            miss_float(
                reflected_to,
                m.reflect_result,
                color,
                index,
                self_guid,
                stores,
                text,
            );
        }
    }
}

/// vmangos `SPELL_MISS_REFLECT` (`SpellDefines.h:173`), the code the GO's reflect leg tests
/// (`0x6e7ded`).
pub(super) const SPELL_MISS_REFLECT: u8 = 11;

/// `SMSG_LOG_XPGAIN`, as `0x637ec0` dispatches it. Your own guid as the victim is an XP loss,
/// written with no float (`0x637f70`). A kill of a unit the client holds (type mask 8) is named:
/// `CGUnit_C::SetPendingXP` (`0x60a0c0`) marks the unit ([`XpAwarded`], `0x60a0e6`), stores the
/// gain and flushes it at once through `0x6251e0`, which writes and floats nothing for a zero
/// gain (`0x625202`). Anything else is the unnamed line (`0x637f90`, victim NULL). Both gains
/// float `"XP: %d"` over self, category 4, the one emitter that skips Gate A (`0x607260`).
pub(super) fn xp_gain(
    x: XpGain,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    text: &mut MessageWriter<CombatTextSpawn>,
    chat_log: &ChatLog,
    commands: &mut Commands,
) {
    // `0x468550`: the active player's guid, 0 with none.
    if x.victim == self_guid.0.unwrap_or(0) {
        chat_log.push_xp_loss(x.total);
        return;
    }
    let victim_held = index
        .0
        .get(&x.victim)
        .and_then(|&e| stores.get(e).ok())
        .is_some_and(|s| {
            use benilla_protocol::messages::ObjectType;
            matches!(
                s.0.object_type(),
                Some(ObjectType::Unit | ObjectType::Player)
            )
        });
    if x.kill && victim_held {
        // The mark goes on before the amount is looked at (`0x60a0e6` precedes `0x625202`).
        if let Some(&victim) = index.0.get(&x.victim) {
            commands.entity(victim).insert(XpAwarded);
        }
        if x.total == 0 {
            return;
        }
        chat_log.push_xp_gain(x.victim, x.total, x.base, x.group);
    } else {
        // The unnamed forms never see a float (`0x637f90` passes 1.0).
        chat_log.push_xp_gain(0, x.total, x.base, 1.0);
    }
    if let Some(&me) = self_guid.0.as_ref().and_then(|g| index.0.get(g)) {
        text.write(CombatTextSpawn {
            anchor: me,
            text: TextBody::Literal(format!("XP: {}", x.total)),
            category: 4,
            color: None, // row 4's own purple: XP skips the damage colour law
        });
    }
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
    chat_log: &ChatLog,
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
pub(super) fn level_up(l: LevelUpInfo, chat_log: &ChatLog) {
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

    /// A store the handlers' type-mask-8 lookup takes for a unit (`OBJECT_FIELD_TYPE` 0x9).
    fn unit() -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)]))
    }

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
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>();
        let self_e = app.world_mut().spawn((Guid(10), SelfPlayer, unit())).id();
        let victim_e = app.world_mut().spawn((Guid(20), unit())).id();
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
                      mut text: MessageWriter<CombatTextSpawn>| {
                    spell_log_miss(
                        SpellLogMiss {
                            spell_id: IMMOLATE,
                            caster: 10,
                            extended: false,
                            misses: vec![(20, 7)], // IMMUNE, an outcome vmangos sends here
                        },
                        &index,
                        &self_guid,
                        &stores,
                        Some(&spells),
                        DamageTextGates::default(),
                        &mut text,
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
            vec![(
                TextBody::Word("IMMUNE"),
                3,
                Some(COLOR_SPELL_GOLD),
                victim_e
            )],
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
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>();
            for guid in [ME, MOB] {
                let e = app.world_mut().spawn((Guid(guid), unit())).id();
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

        /// Run one packet through the leg; the floats' text, a word by its key.
        fn run(&mut self, packet: impl Packet) -> Vec<String> {
            let spells = self.spells.clone();
            let mut packet = Some(packet);
            self.app
                .world_mut()
                .run_system_once(
                    move |index: Res<GuidIndex>,
                          self_guid: Res<SelfGuid>,
                          stores: Query<&mut ObjectStore>,
                          mut text: MessageWriter<CombatTextSpawn>| {
                        if let Some(packet) = packet.take() {
                            packet.emit(&index, &self_guid, &stores, &spells, &mut text);
                        }
                    },
                )
                .unwrap();
            self.app
                .world_mut()
                .resource_mut::<Messages<CombatTextSpawn>>()
                .drain()
                .map(|t| match t.text {
                    TextBody::Literal(text) => text,
                    TextBody::Word(key) => key.to_string(),
                })
                .collect()
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
        ) {
            spell_damage_log(
                self,
                index,
                self_guid,
                stores,
                Some(spells),
                DamageTextGates::default(),
                text,
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
        ) {
            periodic_aura_log(
                self,
                index,
                self_guid,
                stores,
                Some(spells),
                DamageTextGates::default(),
                text,
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

    /// `0x5e85e0` floats a stopped hit's reason, the first of absorbed, blocked and resisted
    /// (`0x5e888a`-`0x5e88c5`), and nothing for a hit with no reason; a hit through floats its
    /// number.
    #[test]
    fn a_stopped_spell_hit_floats_its_reason() {
        let mut leg = Leg::new();
        assert_eq!(leg.run(hit(FIREBALL, 0, 0, 30, 40)), ["BLOCK"]);
        assert_eq!(leg.run(hit(FIREBALL, 0, 50, 30, 40)), ["ABSORB"]);
        assert!(leg.run(hit(FIREBALL, 0, 0, 0, 0)).is_empty());
        let crit = SpellDamageLog {
            hit_info: 0x2,
            ..hit(FIREBALL, 300, 50, 0, 0)
        };
        assert_eq!(leg.run(crit), ["300"]);
    }

    /// The float needs the spell's record without `Attributes & 0x100` (`0x5e87e3`-`0x5e87f4`);
    /// split damage floats nothing (`0x5e8744`-`0x5e876a`).
    #[test]
    fn a_hidden_or_unknown_spell_or_a_split_floats_nothing() {
        let mut leg = Leg::new();
        assert!(leg.run(hit(HIDDEN, 300, 0, 0, 0)).is_empty());
        assert!(leg.run(hit(24_000, 300, 0, 0, 0)).is_empty());
        let split = SpellDamageLog {
            hit_info: 0x8 | 0x2,
            ..hit(FIREBALL, 120, 0, 0, 0)
        };
        assert!(leg.run(split).is_empty());
    }

    /// A periodic damage tick floats its reason, a heal tick nothing, and a hidden spell's ticks
    /// nothing (`0x626f21`).
    #[test]
    fn a_periodic_tick_floats_for_a_logged_spell_only() {
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
        assert_eq!(leg.run(tick(FIREBALL)), ["RESIST"]);
        assert!(leg.run(tick(HIDDEN)).is_empty());
    }

    impl Packet for SpellLogMiss {
        fn emit(
            self,
            index: &GuidIndex,
            self_guid: &SelfGuid,
            stores: &Query<&mut ObjectStore>,
            spells: &crate::ui_action::Spells,
            text: &mut MessageWriter<CombatTextSpawn>,
        ) {
            spell_log_miss(
                self,
                index,
                self_guid,
                stores,
                Some(spells),
                DamageTextGates::default(),
                text,
            );
        }
    }

    /// One `SMSG_SPELL_GO` miss list: the caster, the spell, the entries.
    struct GoMisses(u64, u32, Vec<benilla_protocol::messages::SpellGoMiss>);

    impl Packet for GoMisses {
        fn emit(
            self,
            index: &GuidIndex,
            self_guid: &SelfGuid,
            stores: &Query<&mut ObjectStore>,
            spells: &crate::ui_action::Spells,
            text: &mut MessageWriter<CombatTextSpawn>,
        ) {
            spell_go_misses(
                self.0,
                self.0,
                self.1,
                &self.2,
                index,
                self_guid,
                stores,
                Some(spells),
                DamageTextGates::default(),
                text,
            );
        }
    }

    /// The GO's miss list floats each target's word (`0x6e7dd4`), Reflect over the target too, and
    /// a reflect's outcome over the caster (`0x6e7e51`), which over you is no float (Gate A); a
    /// hidden spell floats nothing (`0x6e7d9a`).
    #[test]
    fn a_go_miss_list_floats_each_entry_and_the_reflected_outcome() {
        let mut leg = Leg::new();
        let entry = |target, info, reflect_result| benilla_protocol::messages::SpellGoMiss {
            target,
            info,
            reflect_result,
        };
        assert_eq!(
            leg.run(GoMisses(ME, FIREBALL, vec![entry(MOB, 11, 2)])),
            ["REFLECT"]
        );
        assert_eq!(
            leg.run(GoMisses(ME, FIREBALL, vec![entry(MOB, 3, 0)])),
            ["DODGE"]
        );
        assert!(leg
            .run(GoMisses(ME, HIDDEN, vec![entry(MOB, 3, 0)]))
            .is_empty());
    }

    /// The GO's miss word over your target is spell gold, the record pushed (`0x6e7d73`,
    /// `0x6e7dcc`), and `0x607140`'s CVars gate it: `CombatDamage` 0 draws nothing (`0x60718f`).
    #[test]
    fn a_go_miss_word_is_spell_gold_and_cvar_gated() {
        let mut leg = Leg::new();
        let mut float = |combat_damage| {
            let spells = leg.spells.clone();
            leg.app
                .world_mut()
                .run_system_once(
                    move |index: Res<GuidIndex>,
                          self_guid: Res<SelfGuid>,
                          stores: Query<&mut ObjectStore>,
                          mut text: MessageWriter<CombatTextSpawn>| {
                        let miss = benilla_protocol::messages::SpellGoMiss {
                            target: MOB,
                            info: 1,
                            reflect_result: 0,
                        };
                        let gates = DamageTextGates {
                            combat_damage,
                            ..DamageTextGates::default()
                        };
                        spell_go_misses(
                            ME,
                            ME,
                            FIREBALL,
                            &[miss],
                            &index,
                            &self_guid,
                            &stores,
                            Some(&spells),
                            gates,
                            &mut text,
                        );
                    },
                )
                .unwrap();
            leg.app
                .world_mut()
                .resource_mut::<Messages<CombatTextSpawn>>()
                .drain()
                .map(|t| (t.text, t.category, t.color))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            float(true),
            [(TextBody::Word("MISS"), 3, Some(COLOR_SPELL_GOLD))]
        );
        assert!(float(false).is_empty());
    }

    /// A spell-log miss floats its word under the `Attributes & 0x100` gate (`0x5e7f15`).
    #[test]
    fn a_spell_log_miss_floats_for_a_logged_spell_only() {
        let mut leg = Leg::new();
        let miss = |spell_id, code| SpellLogMiss {
            spell_id,
            caster: ME,
            extended: false,
            misses: vec![(MOB, code)],
        };
        assert_eq!(leg.run(miss(FIREBALL, 7)), ["IMMUNE"]);
        assert!(leg.run(miss(HIDDEN, 7)).is_empty());
    }

    /// A float needs its recipient held as a unit (`0x5e885b`, mask 8): a hit on a game object the
    /// client holds floats nothing.
    #[test]
    fn a_float_needs_its_recipient_held_as_a_unit() {
        const TRAP: u64 = 30;
        let mut leg = Leg::new();
        let e = leg
            .app
            .world_mut()
            .spawn((
                Guid(TRAP),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x21)])),
            ))
            .id();
        leg.app
            .world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(TRAP, e);
        assert_eq!(leg.run(hit(FIREBALL, 300, 0, 0, 0)), ["300"]);
        let on_trap = SpellDamageLog {
            target: TRAP,
            ..hit(FIREBALL, 300, 0, 0, 0)
        };
        assert!(leg.run(on_trap).is_empty());
    }
}
