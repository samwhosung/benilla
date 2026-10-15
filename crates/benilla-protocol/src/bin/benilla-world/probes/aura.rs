//! `--aura`: the aura wire. Applies a buff and a DoT with set durations, requires both in
//! `UNIT_FIELD_AURA` (half, cancelable bit, level byte, stack bias), and requires each one's
//! `SMSG_UPDATE_AURA_DURATION` to arrive before the descriptor delta that names it.

use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use benilla_protocol::{decode, ObjectFields, ServerPacket, SessionEvent};

use crate::probes::{Ctx, Probe};

// `.aura <spell> <seconds>` (`UnitCommands.cpp:997-1051`) targets the caster when nothing is
// selected (`Chat.cpp:2621-2622`).
/// Mark of the Wild: positive, without `SPELL_ATTR_NO_AURA_CANCEL`, so cancelable (slots 0–31).
const AURA_BUFF_SPELL: u32 = 1126;
/// Shadow Word: Pain, `SPELL_AURA_PERIODIC_DAMAGE`, negative however applied: slots 32–47, not
/// cancelable.
const AURA_DEBUFF_SPELL: u32 = 589;
const AURA_BUFF_SECONDS: u32 = 300;
/// Short, since it damages us and must expire even if the cleanup `.unaura` is missed.
const AURA_DEBUFF_SECONDS: u32 = 15;

pub(crate) struct Aura;

impl Probe for Aura {
    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let self_guid = cx.world.self_guid;
        let self_level = cx.world.self_level;
        // The probe searches the decoded slots for its spell ids, so a wrong field index fails as
        // "never appeared". Start from the login snapshot: deltas carry only changed fields, and
        // auras restored from `character_aura` at login would otherwise be invisible.
        let mut fields = cx.world.self_fields.clone().unwrap_or_default();
        let session = &mut *cx.session;
        let dump = |label: &str, f: &ObjectFields| {
            println!("\nUNIT_FIELD_AURA {label}:");
            for a in f.unit_auras() {
                println!(
                    "  slot {:>2} spell {:>5} flags {:#06b} level {:>2} stacks {} ({}{})",
                    a.slot,
                    a.spell_id,
                    a.flags,
                    a.level,
                    a.stacks,
                    if a.is_helpful() { "buff" } else { "debuff" },
                    if a.is_cancelable() {
                        ", cancelable"
                    } else {
                        ""
                    },
                );
            }
        };
        dump("at login (restored from character_aura)", &fields);

        // A clean slate, so a re-run measures a fresh apply. `.unaura` zeroes the slot, which
        // arrives as an explicit `0` in the next delta.
        session.send_chat(&format!(".unaura {AURA_BUFF_SPELL}"))?;
        session.send_chat(&format!(".unaura {AURA_DEBUFF_SPELL}"))?;
        let settle = Instant::now() + Duration::from_secs(3);
        while Instant::now() < settle {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::ObjectValues { guid, fields: d } = ev {
                    if guid == self_guid {
                        fields.merge(d);
                    }
                }
            }
        }
        dump("after .unaura of both probe spells", &fields);
        // Survivors are not ours (Battle Stance, 2457, owns slot 0) and must report no duration:
        // the server sends `SMSG_UPDATE_AURA_DURATION` only on apply or refresh, never for a
        // permanent aura, which the 1.12 client shows as "until cancelled".
        let untouched: Vec<u8> = fields.unit_auras().map(|a| a.slot).collect();

        println!("\nGM: .aura {AURA_BUFF_SPELL} {AURA_BUFF_SECONDS} (Mark of the Wild)");
        println!("GM: .aura {AURA_DEBUFF_SPELL} {AURA_DEBUFF_SECONDS} (Shadow Word: Pain)");
        session.send_chat(&format!(".aura {AURA_BUFF_SPELL} {AURA_BUFF_SECONDS}"))?;
        session.send_chat(&format!(".aura {AURA_DEBUFF_SPELL} {AURA_DEBUFF_SECONDS}"))?;

        // Index the first duration packet and the first delta naming the buff by packet arrival;
        // `SMSG_UPDATE_AURA_DURATION` is read off the `ServerPacket`, before decode.
        let mut durations: Vec<(u8, i32)> = Vec::new();
        let (mut seq, mut first_duration_at, mut buff_field_at) = (0usize, None, None);
        let drain_until = Instant::now() + Duration::from_secs(8);
        while Instant::now() < drain_until {
            let Ok(msg) = session.recv() else { continue };
            seq += 1;
            if let ServerPacket::UpdateAuraDuration { slot, remaining_ms } = &msg {
                let (slot, remaining_ms) = (*slot, *remaining_ms);
                println!("  SMSG_UPDATE_AURA_DURATION slot {slot} → {remaining_ms} ms");
                first_duration_at.get_or_insert(seq);
                durations.push((slot, remaining_ms));
                continue;
            }
            for ev in decode(msg) {
                if let SessionEvent::ObjectValues { guid, fields: d } = ev {
                    if guid == self_guid {
                        fields.merge(d);
                        if fields.unit_auras().any(|a| a.spell_id == AURA_BUFF_SPELL) {
                            buff_field_at.get_or_insert(seq);
                        }
                    }
                }
            }
        }

        let auras: Vec<_> = fields.unit_auras().collect();
        dump("after both .aura applies", &fields);

        let buff = auras
            .iter()
            .find(|a| a.spell_id == AURA_BUFF_SPELL)
            .copied()
            .context(
                "--aura: spell 1126 never appeared in UNIT_FIELD_AURA. Either the descriptor field \
                 index is wrong, or the GM `.aura` command was refused — it needs gmlevel >= 4 \
                 (vmangos `Chat/Chat.cpp:1229`: SEC_BASIC_ADMIN, which is 4 in \
                 `shared/Common.h:142`).",
            )?;
        let debuff = auras
            .iter()
            .find(|a| a.spell_id == AURA_DEBUFF_SPELL)
            .copied()
            .context("--aura: spell 589 never appeared in UNIT_FIELD_AURA")?;

        ensure!(
            buff.is_helpful() && buff.slot < 32,
            "buff landed in the debuff half (slot {})",
            buff.slot
        );
        ensure!(
            !debuff.is_helpful() && debuff.slot >= 32,
            "debuff landed in the buff half (slot {})",
            debuff.slot
        );
        ensure!(
            buff.is_cancelable(),
            "AFLAG_CANCELABLE clear on a positive, cancelable buff (flags {:#x})",
            buff.flags
        );
        ensure!(
            !debuff.is_cancelable(),
            "AFLAG_CANCELABLE set on a debuff (flags {:#x})",
            debuff.flags
        );
        ensure!(
            buff.level == self_level && debuff.level == self_level,
            "AURALEVELS byte should be the caster's level {}: got buff {} / debuff {}",
            self_level,
            buff.level,
            debuff.level
        );
        ensure!(
            buff.stacks == 1 && debuff.stacks == 1,
            "stack bias wrong (the wire byte is count-1): got buff {} / debuff {}",
            buff.stacks,
            debuff.stacks
        );

        // `.aura` sets an exact duration and the packet goes out at once: allow 2 s of decay.
        for (label, aura, asked) in [
            ("buff", buff, AURA_BUFF_SECONDS),
            ("debuff", debuff, AURA_DEBUFF_SECONDS),
        ] {
            let ms = durations
                .iter()
                .find(|(slot, _)| *slot == aura.slot)
                .map(|&(_, ms)| ms)
                .with_context(|| {
                    format!(
                        "--aura: no SMSG_UPDATE_AURA_DURATION for the {label}'s own slot {} \
                         (durations seen: {durations:?})",
                        aura.slot
                    )
                })?;
            let asked_ms = (asked * 1000) as i32;
            ensure!(
                ms <= asked_ms && ms + 2000 >= asked_ms,
                "{label} duration {ms} ms is not the {asked_ms} ms we asked for"
            );
            println!("✅ {label}: slot {} ⇒ {ms} ms", aura.slot);
        }

        // The ordering the aura model rests on: the timer reaches us before the slot is named.
        let (d, v) = (
            first_duration_at.context("--aura: no duration packet at all")?,
            buff_field_at.context("--aura: the buff never appeared in a values delta")?,
        );
        ensure!(
            d < v,
            "SMSG_UPDATE_AURA_DURATION arrived AFTER the descriptor delta (seq {d} vs {v}) — \
             the app's slot-keyed buffering is built on the opposite order"
        );
        println!("✅ duration packet precedes the descriptor delta (event {d} before {v})");

        // Durations are apply/refresh edges, not a stream; the client counts down locally.
        if let Some(&slot) = untouched
            .iter()
            .find(|s| durations.iter().any(|(d, _)| d == *s))
        {
            bail!(
                "--aura: slot {slot} reported a duration without being (re)applied — durations are \
                 not the apply/refresh edges the client-side countdown assumes"
            );
        }
        println!(
            "✅ no duration for the {} untouched slot(s) {untouched:?} — permanent auras are \
             'until cancelled', and durations are apply/refresh edges, not a stream",
            untouched.len()
        );

        // Clean up, then drain: `character_aura` persists across logout, so an unprocessed
        // `.unaura` would leave the probe's auras on the character.
        session.send_chat(&format!(".unaura {AURA_BUFF_SPELL}"))?;
        session.send_chat(&format!(".unaura {AURA_DEBUFF_SPELL}"))?;
        let settle = Instant::now() + Duration::from_secs(3);
        while Instant::now() < settle {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::ObjectValues { guid, fields: d } = ev {
                    if guid == self_guid {
                        fields.merge(d);
                    }
                }
            }
        }
        dump("after cleanup", &fields);
        ensure!(
            !fields
                .unit_auras()
                .any(|a| a.spell_id == AURA_BUFF_SPELL || a.spell_id == AURA_DEBUFF_SPELL),
            "--aura: cleanup left a probe aura on the character"
        );

        println!("\n✅ --aura PASS: the aura block decodes and the duration wire is slot-keyed.");
        Ok(())
    }
}
