//! The cast journal: every recent cast edge, shown in the inspect overlay as click-to-copy rows.
//! Recording is always on so a spell seen before inspect was armed is already there; only the
//! drawing gates on [`InspectMode`].

use std::collections::VecDeque;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::creature_anim::{CastEvent, CastEventKind};
use crate::debug_panel::{overlay_text, OVERLAY_FILL, OVERLAY_TEXT_DIM};
use crate::net::{Guid, NetCommands, SelfGuid};
use crate::ui_action::Spells;

use crate::ui_script::InspectMode;

/// Records kept in the ring buffer.
const KEPT: usize = 24;
/// Rows drawn in the overlay, newest first.
const SHOWN: usize = 10;

/// Where a recorded cast stands on its wire edges.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CastState {
    /// `SMSG_SPELL_START` seen, no GO yet (or the row went stale on a missed edge).
    Casting,
    /// `SMSG_SPELL_GO`: the cast went off.
    Went,
    /// The cast died without a release (`SMSG_SPELL_FAILED_OTHER` or our failed `CAST_RESULT`).
    Failed,
}

/// One observed cast, its `Spell.dbc` display resolved at record time; the caster stays a guid
/// and resolves at draw time, so a name that fills late still shows.
struct CastRecord {
    caster: u64,
    spell_id: u32,
    name: Option<String>,
    visual: u32,
    speed: f32,
    /// `Time::elapsed_secs_f64` of the latest edge; a GO refreshes it.
    at: f64,
    state: CastState,
    /// Missile arrivals, matched by spell id to the newest launched row: an impact carries the
    /// target, not the caster.
    impacts: u32,
}

/// The ring buffer of recent casts, newest at the back.
#[derive(Resource, Default)]
pub(super) struct CastJournal {
    records: VecDeque<CastRecord>,
}

impl CastJournal {
    /// The newest `Casting` row for `caster` and `spell_id`, the one a GO or fail edge upgrades.
    fn open_cast(&mut self, caster: u64, spell_id: u32) -> Option<&mut CastRecord> {
        self.records
            .iter_mut()
            .rev()
            .find(|r| r.caster == caster && r.spell_id == spell_id && r.state == CastState::Casting)
    }

    fn push(&mut self, record: CastRecord) {
        if self.records.len() == KEPT {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }
}

/// Record every cast edge into the journal, resolving the spell's display row once.
pub(super) fn record_casts(
    mut casts: MessageReader<CastEvent>,
    mut journal: ResMut<CastJournal>,
    guids: Query<&Guid>,
    spells: Option<Res<Spells>>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for ev in casts.read() {
        match ev.kind {
            CastEventKind::Start | CastEventKind::Go | CastEventKind::Fail => {
                // A caster despawned the same frame drops.
                let Ok(&Guid(caster)) = guids.get(ev.entity) else {
                    continue;
                };
                let state = match ev.kind {
                    CastEventKind::Start => CastState::Casting,
                    CastEventKind::Go => CastState::Went,
                    _ => CastState::Failed,
                };
                // A GO or fail closes its START row; without one (a proc's bare GO, a failed
                // instant) it gets a fresh row.
                if state != CastState::Casting {
                    if let Some(open) = journal.open_cast(caster, ev.spell_id) {
                        open.state = state;
                        open.at = now;
                        continue;
                    }
                }
                let display = spells.as_ref().and_then(|s| s.catalog.get(ev.spell_id));
                journal.push(CastRecord {
                    caster,
                    spell_id: ev.spell_id,
                    name: display.map(|d| d.name.clone()),
                    visual: display.map(|d| d.visual).unwrap_or(0),
                    speed: display.map(|d| d.speed).unwrap_or(0.0),
                    at: now,
                    state,
                    impacts: 0,
                });
            }
            // Arrivals credit the newest launched row for the spell; a ground arrival counts, as
            // a pure dest cast has no other.
            CastEventKind::Impact { .. } | CastEventKind::GroundImpact { .. } => {
                if let Some(row) = journal
                    .records
                    .iter_mut()
                    .rev()
                    .find(|r| r.spell_id == ev.spell_id && r.state == CastState::Went)
                {
                    row.impacts += 1;
                }
            }
        }
    }
}

/// A record's age as a compact suffix (`now` / `8s` / `3m`).
fn age(now: f64, at: f64) -> String {
    let secs = (now - at).max(0.0);
    if secs < 1.5 {
        "now".into()
    } else if secs < 60.0 {
        format!("{}s", secs as u32)
    } else {
        format!("{}m", (secs / 60.0) as u32)
    }
}

/// The journal overlay: while inspect is armed, recent casts top-left, newest first; a click
/// copies a row.
pub(super) fn journal_ui(
    mut contexts: EguiContexts,
    inspect: Res<InspectMode>,
    journal: Res<CastJournal>,
    self_guid: Res<SelfGuid>,
    names: Res<crate::names::NameCache>,
    net_commands: Res<NetCommands>,
    time: Res<Time>,
    // The copied row's `at` (unique per row) and when it was copied, for the confirmation flash.
    mut copied: Local<Option<(f64, f32)>>,
) -> Result {
    if !inspect.enabled || journal.records.is_empty() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let now = time.elapsed_secs_f64();

    egui::Area::new(egui::Id::new("inspect_journal"))
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(8.0, 8.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(8, 6))
                .corner_radius(5.0)
                .fill(OVERLAY_FILL)
                .show(ui, |ui| {
                    overlay_text(ui);
                    ui.label(
                        egui::RichText::new("recent spells · click a row to copy")
                            .small()
                            .color(OVERLAY_TEXT_DIM),
                    );
                    for (i, r) in journal.records.iter().rev().take(SHOWN).enumerate() {
                        let caster = if self_guid.0 == Some(r.caster) {
                            "you".to_string()
                        } else {
                            names
                                .resolve(r.caster, &net_commands)
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("guid {:#x}", r.caster))
                        };
                        let name = r.name.as_deref().unwrap_or("?");
                        let state = match r.state {
                            CastState::Casting => "casting…",
                            CastState::Went => "went off",
                            CastState::Failed => "failed",
                        };
                        let flight = if r.speed > 0.0 {
                            format!("missile {:.0}/s", r.speed)
                        } else {
                            "instant".to_string()
                        };
                        let mut detail = format!("by {caster} · visual {} · {flight}", r.visual);
                        if r.impacts > 0 {
                            detail.push_str(&format!(" · {} impact(s)", r.impacts));
                        }
                        let name_color = match r.state {
                            CastState::Casting => egui::Color32::from_rgb(240, 205, 130),
                            CastState::Went => egui::Color32::from_rgb(200, 220, 240),
                            CastState::Failed => egui::Color32::from_rgb(230, 140, 140),
                        };
                        let response = ui
                            .push_id(i, |ui| {
                                egui::Frame::NONE
                                    .inner_margin(egui::Margin::symmetric(4, 3))
                                    .corner_radius(3.0)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.colored_label(
                                                name_color,
                                                egui::RichText::new(name).monospace(),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} · {} · {state}",
                                                    r.spell_id,
                                                    age(now, r.at)
                                                ))
                                                .small()
                                                .color(OVERLAY_TEXT_DIM),
                                            );
                                        });
                                        ui.label(
                                            egui::RichText::new(&detail)
                                                .small()
                                                .color(OVERLAY_TEXT_DIM),
                                        );
                                    })
                            })
                            .inner
                            .response
                            .interact(egui::Sense::click());
                        if response.hovered() {
                            ui.painter().rect_filled(
                                response.rect,
                                3.0,
                                egui::Color32::from_white_alpha(6),
                            );
                        }
                        if response.clicked() {
                            let when = match age(now, r.at).as_str() {
                                "now" => "just now".to_string(),
                                a => format!("{a} ago"),
                            };
                            ctx.copy_text(format!(
                                "spell {} \"{name}\" · by {caster} · visual {} · {flight} · \
                                 {state}{} · {when}",
                                r.spell_id,
                                r.visual,
                                if r.impacts > 0 {
                                    format!(", {} impact(s)", r.impacts)
                                } else {
                                    String::new()
                                },
                            ));
                            *copied = Some((r.at, time.elapsed_secs()));
                        }
                        if copied.is_some_and(|(at, t)| {
                            at == r.at && time.elapsed_secs() - t < super::inspect::COPY_FLASH_SECS
                        }) {
                            ui.label(
                                egui::RichText::new("copied to clipboard")
                                    .small()
                                    .color(egui::Color32::from_rgb(140, 220, 140)),
                            );
                        }
                    }
                });
        });
    Ok(())
}
