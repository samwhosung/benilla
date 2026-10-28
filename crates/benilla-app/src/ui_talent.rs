//! The talent window: the class's pages from `Talent.dbc` and `TalentTab.dbc`, the known spells
//! and `PLAYER_CHARACTER_POINTS1/2`, which the getters read at the call ([`live`]), and the talent
//! verbs' engine half ([`TalentSteps`]). A talent's rank is its highest rank spell known itself or
//! through a later rank ([`Known`]; learning grants every lower rank, vmangos
//! `Player::LearnTalent`); an unlearned one wears rank 1's name and icon.
//! `CHARACTER_POINTS_CHANGED` is the points watch's ([`crate::field_watch`]).
//!
//! The red requirement lines are the three keys `0x52b390` emits, in its order:
//! `TOOLTIP_TALENT_PREREQ[_P1]` (`0x854a5c`), `TOOLTIP_TALENT_TIER_POINTS` (`0x854a40`) and
//! `ITEM_REQ_SKILL` (`0x84e338`); the snapshot carries their arguments ([`TalentReq`]) and the
//! tooltip reads the templates as it builds.

use std::collections::BTreeSet;

use bevy::prelude::*;

use benilla_formats::{Talent, TalentCatalog};
use benilla_ui::script::{
    TalentCall, TalentPrereqView, TalentReq, TalentTabView, TalentUiState, TalentView, UiScript,
};

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

mod live;
pub(crate) use live::TalentLive;

/// The talent catalog (`Talent.dbc`, `TalentTab.dbc`), absent when the client data failed to load.
#[derive(Resource)]
pub(crate) struct Talents {
    pub(crate) catalog: TalentCatalog,
}

pub(crate) struct UiTalentPlugin;

impl Plugin for UiTalentPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_talents.after(AssetSet::Open));
    }
}

fn load_talents(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_talent_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            info!("ui_talent: {} talent(s) in the catalog", catalog.len());
            commands.insert_resource(Talents { catalog });
        }
        Err(e) => warn!("ui_talent: Talent.dbc failed to load — no talent window: {e:#}"),
    }
}

/// The known spells a rank reads, with the ability rows whose supersede chain
/// `KnowsHigherRank` (`0x60c8d0`) walks; `None` rows know no higher rank.
#[derive(Clone, Copy)]
pub(crate) struct Known<'a> {
    pub(crate) spells: &'a BTreeSet<u32>,
    pub(crate) lines: Option<&'a benilla_formats::SkillLineCatalog>,
}

impl<'a> Known<'a> {
    /// The known spells alone, with no ability rows.
    #[cfg(test)]
    pub(crate) fn bare(spells: &'a BTreeSet<u32>) -> Self {
        Self {
            spells,
            lines: None,
        }
    }

    /// `KnowsSpell(id) || KnowsHigherRank(id)` (`0x4f32df`, `0x4f32f5`).
    fn has_rank(self, id: u32) -> bool {
        self.spells.contains(&id)
            || self
                .lines
                .is_some_and(|l| l.higher_rank_known(id, self.spells))
    }
}

/// A talent's rank: the highest rank whose spell, or a later rank of it, is known
/// (`0x4f32c8`-`0x4f330d`), so a talent spell a trainer later ranked up past (vmangos
/// `Player::learnSpell` supersedes the lower rank) still counts.
fn rank_of(t: &Talent, known: Known<'_>) -> u32 {
    t.ranks
        .iter()
        .enumerate()
        .filter(|(_, &s)| s != 0 && known.has_rank(s))
        .map(|(i, _)| i as u32 + 1)
        .max()
        .unwrap_or(0)
}

/// The talent pages of `race` and `class`, `0x4f2c00`'s tab list in `TalentTab.dbc` order with
/// each tab's talents in file order, read off the known spells and the points as they stand: the
/// no-host snapshot ([`benilla_ui::script::UiScript::set_talents`]), and what the live reads
/// answer a piece of ([`live::TalentLive`]).
pub(crate) fn build_pages(
    catalog: &TalentCatalog,
    known: Known<'_>,
    // `Spell.dbc` alone, for names and icons, so the addon-corpus survey can build the snapshot
    // without the other spell catalogs.
    spells: &benilla_formats::SpellCatalog,
    race: u8,
    class: u8,
    points: (u32, u32),
) -> TalentUiState {
    let mut tabs = Vec::new();
    let mut pages = Vec::new();
    for tab in catalog.tabs_for_class(race, class) {
        let spent = tab_spent(catalog, tab.id, known);
        pages.push(
            catalog
                .talents_in_tab(tab.id)
                .iter()
                .map(|t| talent_view(catalog, tab, t, spent, known, spells, points))
                .collect(),
        );
        tabs.push(tab_view(tab, spent));
    }
    TalentUiState {
        tabs,
        talents: pages,
        points,
    }
}

/// A tab's points spent (`+4`): the ranks of its talents off the known spells.
fn tab_spent(catalog: &TalentCatalog, tab_id: u32, known: Known<'_>) -> u32 {
    catalog
        .talents_in_tab(tab_id)
        .iter()
        .map(|t| rank_of(t, known))
        .sum()
}

/// `GetTalentTabInfo`'s row of `tab`, `spent` its points.
fn tab_view(tab: &benilla_formats::TalentTabInfo, spent: u32) -> TalentTabView {
    TalentTabView {
        name: tab.name.clone(),
        background: tab.background.clone(),
        points_spent: spent,
        texture: tab.icon.clone(),
    }
}

/// One talent of `tab` as `GetTalentInfo`, `GetTalentPrereqs` and the tooltip read it, `spent` the
/// tab's points and `points` the player's two counters.
fn talent_view(
    catalog: &TalentCatalog,
    tab: &benilla_formats::TalentTabInfo,
    t: &Talent,
    spent: u32,
    known: Known<'_>,
    spells: &benilla_formats::SpellCatalog,
    points: (u32, u32),
) -> TalentView {
    let rank = rank_of(t, known);
    let max_rank = t.max_rank();
    let display_spell = t.ranks[rank.max(1) as usize - 1];
    let next_spell = if rank > 0 && rank < max_rank {
        t.ranks[rank as usize]
    } else {
        0
    };
    let d = spells.get(display_spell);
    // `meetsPrereq` is the `requiredSpell` check alone (`GetTalentInfo` `0x4f3200`).
    let meets_prereq = t.required_spell == 0 || known.spells.contains(&t.required_spell);
    // `0x52b390`'s lines, in its order, for an unlearned talent alone (`0x52e887`, `0x52b246`):
    // each prerequisite below its rank (`0x52b3f0`-`0x52b4e5`), the tab points its tier needs
    // (`0x52b4ee`-`0x52b591`), then an unknown required spell (`0x52b5af`-`0x52b636`). Each line
    // clears the all-met flag it returns; a learned talent keeps the builder's 1 (`0x52e75b`).
    let mut reqs = Vec::new();
    let mut all_met = true;
    let mut prereqs = Vec::new();
    // The three slots in order, an empty or unknown id skipped (`0x52b3f0`-`0x52b418`).
    for (&id, &need0) in t.prereq_talents.iter().zip(&t.prereq_ranks) {
        let Some(p) = (id != 0).then(|| catalog.talent(t.tab, id)).flatten() else {
            continue;
        };
        let need = need0 + 1;
        let learnable = rank_of(p, known) >= need;
        prereqs.push(TalentPrereqView {
            tier: p.row + 1,
            column: p.col + 1,
            learnable,
        });
        if !learnable && rank == 0 {
            all_met = false;
            // The name is the last rank spell the scan from the needed rank read
            // (`0x52b430`-`0x52b466`); none, or no record, adds no line (`0x52b484`).
            let scanned = p
                .ranks
                .get(need0 as usize..)
                .and_then(|r| r.iter().take_while(|&&s| s != 0).last());
            if let Some(d) = scanned.and_then(|&s| spells.get(s)) {
                reqs.push(TalentReq::Prereq {
                    rank: need,
                    name: d.name.clone(),
                });
            }
        }
    }
    // The tier gate reads the tab's own spent sum.
    if rank == 0 && t.row * 5 > spent {
        all_met = false;
        reqs.push(TalentReq::Tier {
            points: t.row * 5,
            tab: tab.name.clone(),
        });
    }
    // The item tooltip's `ITEM_REQ_SKILL`, not the `LOCKED_WITH_*` or `SPELL_FAILED_*` keys that
    // read the same in enUS. A spell with no record adds no line and leaves the flag (`0x52b5f1`).
    if rank == 0 && !meets_prereq {
        if let Some(req) = spells.get(t.required_spell) {
            all_met = false;
            reqs.push(TalentReq::Spell {
                name: req.name.clone(),
            });
        }
    }
    // The green hint needs the flag, points left and a rank below the maximum (`0x52b31b` in
    // `0x52b0a0`, `0x52f735` in `0x52e610`); the frame's gold, green and gray come from the stock
    // Lua.
    let learnable = all_met && rank < max_rank && points.0 > 0;
    TalentView {
        talent_id: t.id,
        name: d.map(|d| d.name.clone()).unwrap_or_default(),
        texture: d.and_then(|d| d.icon.clone()),
        tier: t.row + 1,
        column: t.col + 1,
        rank,
        max_rank,
        exceptional: t.exceptional,
        meets_prereq,
        prereqs,
        display_spell,
        next_spell,
        reqs,
        learnable,
    }
}

/// The talent verbs' engine half, which the call host applies inside each call
/// ([`crate::script_calls::ScriptCallHost`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TalentSteps<'w, 's> {
    net: Res<'w, NetCommands>,
    /// Absent in a world with no trainer flow, which drops the Accept.
    wipe: Option<ResMut<'w, crate::ui_talent_wipe::TalentWipeState>>,
    /// The latched trainer's range, which the Accept tests at the call.
    reach: crate::ui_dialog_verbs::DialogReach<'w, 's>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    sink: crate::ui_action::MessageSink<'w>,
}

impl TalentSteps<'_, '_> {
    /// Apply one talent call.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: TalentCall) {
        match call {
            // The packet (`0x4f379b`-`0x4f37e0`), the server learning up to the rank.
            TalentCall::Learn { talent_id, rank } => {
                debug!("ui_talent: learn talent {talent_id} rank {rank}");
                let _ = self
                    .net
                    .0
                    .send(ClientCommand::LearnTalent { talent_id, rank });
            }
            // `0x48dc40` finds the active player first (`0x48dc78`).
            TalentCall::ConfirmWipe => {
                let (Some(wipe), Ok(store)) = (self.wipe.as_deref_mut(), self.self_q.single())
                else {
                    return;
                };
                let money = store.0.player_money().unwrap_or(0);
                crate::ui_talent_wipe::accept(
                    wipe,
                    &self.reach,
                    money,
                    &self.net,
                    script,
                    &mut self.sink,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::MAX_TALENT_RANK;
    use benilla_ui::script::UiScript;

    fn talent(id: u32, tab: u32, row: u32, col: u32, ranks: &[u32]) -> Talent {
        let mut r = [0u32; MAX_TALENT_RANK];
        r[..ranks.len()].copy_from_slice(ranks);
        Talent {
            id,
            tab,
            row,
            col,
            ranks: r,
            prereq_talents: [0; 3],
            prereq_ranks: [0; 3],
            required_spell: 0,
            exceptional: false,
        }
    }

    #[test]
    fn rank_is_the_highest_known_rank_spell() {
        let t = talent(1, 81, 0, 0, &[100, 101, 102]);
        let known: BTreeSet<u32> = [100, 101].into_iter().collect();
        assert_eq!(rank_of(&t, Known::bare(&known)), 2);
        assert_eq!(rank_of(&t, Known::bare(&BTreeSet::new())), 0);
        // Learning up to a rank leaves no gap on the wire, but a gap still reads by the max.
        let holey: BTreeSet<u32> = [102].into_iter().collect();
        assert_eq!(rank_of(&t, Known::bare(&holey)), 3);
    }

    /// The rank reads the known spells alone, so a wipe shows once the `SMSG_REMOVED_SPELL` burst
    /// empties them; the points are a descriptor field and arrive on their own.
    #[test]
    fn an_emptied_book_reads_as_a_wiped_tree() {
        let t = talent(1, 201, 0, 0, &[14522, 14788, 14789]);
        let spent: BTreeSet<u32> = [14522, 14788, 14789].into_iter().collect();
        assert_eq!(rank_of(&t, Known::bare(&spent)), 3);
        assert_eq!(rank_of(&t, Known::bare(&BTreeSet::new())), 0);
    }

    /// A mage Fire tab of three talents: Alpha and Beta on tier 1, Gamma on tier 2 behind both.
    fn two_arrow_tree() -> (TalentCatalog, benilla_formats::SpellCatalog) {
        let tab = benilla_formats::TalentTabInfo {
            id: 41,
            name: "Fire".into(),
            race_mask: 511,
            class_mask: 1 << 7,
            background: "MageFire".into(),
            icon: None,
        };
        let mut gamma = talent(12, 41, 1, 0, &[1003, 1004]);
        gamma.prereq_talents = [10, 11, 0];
        let talents = vec![
            talent(10, 41, 0, 0, &[1001]),
            talent(11, 41, 0, 1, &[1002]),
            gamma,
        ];
        let spells = [
            (1001, "Alpha"),
            (1002, "Beta"),
            (1003, "Gamma"),
            (1004, "Gamma"),
        ]
        .into_iter()
        .map(|(id, name)| {
            let d = benilla_formats::SpellDisplay {
                name: name.into(),
                ..Default::default()
            };
            (id, d)
        })
        .collect();
        (
            TalentCatalog::from_rows(vec![tab], talents),
            benilla_formats::SpellCatalog::from_displays(spells),
        )
    }

    /// `0x52b390` walks all three prerequisite slots (`+0x34`/`+0x40`, `0x52b3e4`), a line for
    /// each unmet one; `GetTalentPrereqs` (`0x4f34d0`, `0x4f3589`) lists each populated slot.
    #[test]
    fn every_prerequisite_slot_is_read() {
        let (catalog, spells) = two_arrow_tree();
        // Alpha learned, Beta not: one point in the tab.
        let known: BTreeSet<u32> = [1001].into_iter().collect();
        let state = build_pages(&catalog, Known::bare(&known), &spells, 1, 8, (1, 0));
        let gamma = &state.talents[0][2];
        assert_eq!(gamma.prereqs.len(), 2, "both arrows");
        assert_eq!(
            gamma
                .prereqs
                .iter()
                .map(|p| p.learnable)
                .collect::<Vec<_>>(),
            [true, false]
        );
        assert_eq!(
            talent_tooltip_reqs(&state, 2),
            ["PREREQ 1 Beta", "TIER 5 Fire"],
            "the unmet second slot, then the tier"
        );
        assert!(!gamma.learnable);
    }

    /// The requirement lines' templates are `_G` reads at the tooltip's build (`0x703bf0` at
    /// `0x52b4a5`, `0x52b566`, `0x52b60b`), so an addon's rewording shows at the next hover.
    #[test]
    fn the_requirement_words_are_read_at_the_tooltip_call() {
        let (catalog, spells) = two_arrow_tree();
        let state = build_pages(
            &catalog,
            Known::bare(&BTreeSet::new()),
            &spells,
            1,
            8,
            (1, 0),
        );
        let mut script = UiScript::new().expect("VM");
        crate::ui_script::test_ui::load_tooltip_template(&script);
        script.set_talents(state);
        script.set_spell_tooltip(1003, gamma_view());
        let lines: Vec<String> = script
            .eval(
                r#"
                TOOLTIP_TALENT_PREREQ = "NEW %d %s"
                TOOLTIP_TALENT_PREREQ_P1 = "NEW %d %s"
                TOOLTIP_TALENT_TIER_POINTS = "NEWTIER %d %s"
                local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
                tt:SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")
                tt:SetTalent(1, 3)
                local t = {}
                for i = 1, tt:NumLines() do t[i] = getglobal("TTTextLeft" .. i):GetText() or "" end
                return t
                "#,
            )
            .expect("the tooltip");
        assert!(lines.iter().any(|l| l == "NEW 1 Alpha"), "{lines:?}");
        assert!(lines.iter().any(|l| l == "NEWTIER 5 Fire"), "{lines:?}");
    }

    /// Gamma's rank-1 spell view, which `SetTalent` waits on before its requirement lines.
    fn gamma_view() -> benilla_ui::script::SpellTooltipView {
        benilla_ui::script::SpellTooltipView {
            name: "Gamma".into(),
            description: "Burns.".into(),
            ..Default::default()
        }
    }

    /// The requirement lines a talent's tooltip shows, as `SetTalent` writes them.
    fn talent_tooltip_reqs(state: &TalentUiState, index: usize) -> Vec<String> {
        let mut script = UiScript::new().expect("VM");
        crate::ui_script::test_ui::load_tooltip_template(&script);
        script
            .run(r#"TOOLTIP_TALENT_PREREQ = "PREREQ %d %s" TOOLTIP_TALENT_TIER_POINTS = "TIER %d %s""#)
            .expect("the words");
        script.set_talents(state.clone());
        script.set_spell_tooltip(1003, gamma_view());
        let lines: Vec<String> = script
            .eval(&format!(
                r#"
                local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
                tt:SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")
                tt:SetTalent(1, {})
                local t = {{}}
                for i = 1, tt:NumLines() do t[i] = getglobal("TTTextLeft" .. i):GetText() or "" end
                return t
                "#,
                index + 1
            ))
            .expect("the tooltip");
        lines
            .into_iter()
            .filter(|l| l.starts_with("PREREQ ") || l.starts_with("TIER "))
            .collect()
    }

    /// `0x52b390` writes an unmet prerequisite's line (`0x52b3f0`-`0x52b4e5`) before the tier
    /// line (`0x52b561`), and returns 0 when any requirement is unmet, which hides
    /// `TOOLTIP_TALENT_LEARN` (`0x52b31b`, `0x52f735`). It runs only for an unlearned talent
    /// (`0x52e887`, `0x52b246`), whose flag otherwise stays 1 (`0x52e75b`).
    #[test]
    fn a_locked_talent_lists_its_prerequisite_before_its_tier_and_offers_no_learn() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let talents = benilla_formats::load_talent_catalog(&mut chain).expect("talent catalog");
        let spells = benilla_formats::load_spell_catalog(&mut chain).expect("spell catalog");
        // Human (1) mage (8), one unspent point, nothing learned.
        let (race, class) = (1, 8);
        let state = build_pages(
            &talents,
            Known::bare(&BTreeSet::new()),
            &spells,
            race,
            class,
            (1, 0),
        );
        let mut checked = 0;
        for (tab, views) in talents
            .tabs_for_class(race, class)
            .iter()
            .zip(&state.talents)
        {
            for (t, v) in talents.talents_in_tab(tab.id).iter().zip(views) {
                if t.prereq_talents[0] != 0 && t.row > 0 {
                    assert!(v.reqs.len() >= 2, "{}: {:?}", v.name, v.reqs);
                    assert!(
                        matches!(v.reqs[0], TalentReq::Prereq { .. }),
                        "{:?}",
                        v.reqs
                    );
                    assert!(matches!(v.reqs[1], TalentReq::Tier { .. }), "{:?}", v.reqs);
                    assert!(!v.learnable, "{}: a locked talent offers no learn", v.name);
                    checked += 1;
                } else if t.row == 0 && t.prereq_talents[0] == 0 && t.required_spell == 0 {
                    assert!(v.reqs.is_empty());
                    assert!(
                        v.learnable,
                        "{}: a free first-tier talent is learnable",
                        v.name
                    );
                }
            }
        }
        assert!(checked > 0, "the mage trees have prerequisite arrows");

        // A learned talent builds no requirement lines, whatever the tree says: the builder
        // skips `0x52b390` for a known rank.
        let (tab, t) = talents
            .tabs_for_class(race, class)
            .into_iter()
            .find_map(|tab| {
                talents
                    .talents_in_tab(tab.id)
                    .iter()
                    .find(|t| t.prereq_talents[0] != 0 && t.row > 0 && t.max_rank() > 1)
                    .map(|t| (tab.id, t.clone()))
            })
            .expect("a multi-rank talent behind an arrow");
        let known: BTreeSet<u32> = [t.ranks[0]].into_iter().collect();
        let state = build_pages(&talents, Known::bare(&known), &spells, race, class, (1, 0));
        let page = talents
            .tabs_for_class(race, class)
            .iter()
            .position(|x| x.id == tab)
            .expect("its page");
        let v = state.talents[page]
            .iter()
            .find(|v| v.talent_id == t.id)
            .expect("its view");
        assert!(v.reqs.is_empty(), "{:?}", v.reqs);
        assert!(v.learnable, "rank 1 of {}, a point to spend", t.max_rank());
    }

    /// A tab's icon (`TalentTab.dbc` `SpellIconID` through `SpellIcon.dbc`, `0x4f30d0`-`0x4f30f1`)
    /// is the second value `GetTalentTabInfo` answers, from the catalog to the Lua call.
    #[test]
    fn get_talent_tab_info_answers_the_tab_icon_from_the_catalog() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let talents = benilla_formats::load_talent_catalog(&mut chain).expect("talent catalog");
        let spells = benilla_formats::load_spell_catalog(&mut chain).expect("spell catalog");
        // Human (`ChrRaces.dbc` 1) shaman (`ChrClasses.dbc` 7).
        let state = build_pages(
            &talents,
            Known::bare(&BTreeSet::new()),
            &spells,
            1,
            7,
            (0, 0),
        );
        let mut script = UiScript::new().expect("script");
        script.set_talents(state);
        let icons: Vec<String> = script
            .eval(
                "local t = {} for i = 1, 3 do local _, icon = GetTalentTabInfo(i) t[i] = icon end \
                 return t",
            )
            .expect("tab icons");
        assert_eq!(
            icons,
            [
                "Interface\\Icons\\Spell_Fire_Volcano",
                "Interface\\Icons\\Spell_Nature_UnyeildingStamina",
                "Interface\\Icons\\Spell_Nature_HealingWaveGreater",
            ]
        );
    }
}

#[cfg(test)]
#[path = "ui_talent_call_tests.rs"]
mod call_tests;
