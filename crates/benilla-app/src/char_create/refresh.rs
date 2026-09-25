//! The create screen's live refresh: everything that follows the selection or the pointer once
//! [`super::screen`] has spawned the tree.

use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;

use crate::char_select::{class_name, race_name};
use crate::entities::CharCreate;
use crate::glue_strings::GlueStrings;

use super::parts::{
    CharCreateUi, DialRow, DynIcon, DynText, DynTint, InfoKind, InfoScroll, ScrollArrow,
    ScrollHides, ScrollThumb,
};
use super::{class_file, race_classes, CreateAction, CreateSelection, ALLIANCE};
use crate::glue::art::{
    class_tc, race_tc, tc_rect, GlueArt, ALLIANCE_FILL, BACKDROP_ALLIANCE, BACKDROP_HORDE, BTN_BG,
    BTN_HOVER, FALLBACK_ALPHA, HORDE_FILL,
};
use crate::glue::widgets::{FallbackFace, GlueDisabled, Hilight, HoverLabel, LockHighlight};

/// Refill icon rects, dial labels, info texts, faction tints and class slots on a selection
/// change or a fresh spawn.
#[allow(clippy::type_complexity)]
pub(super) fn refresh_dynamic(
    sel: Res<CreateSelection>,
    catalog: Option<Res<CharCreate>>,
    art: Res<GlueArt>,
    strings: Option<Res<GlueStrings>>,
    root: Query<Ref<CharCreateUi>>,
    mut icons: Query<(&DynIcon, &mut ImageNode)>,
    mut texts: Query<(&DynText, &mut Text)>,
    mut tints: Query<
        (
            &DynTint,
            Option<&mut BackgroundColor>,
            Option<&mut ImageNode>,
        ),
        Without<DynIcon>,
    >,
    mut dial_rows: Query<(&DialRow, &mut Node), Without<Button>>,
    mut slots: Query<(&CreateAction, &mut Node), With<Button>>,
) {
    let force = root.iter().any(|r| r.is_added());
    if !force && !sel.is_changed() {
        return;
    }
    let cat = catalog.as_deref();
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);
    let classes = race_classes(cat, sel.race);
    let alliance = ALLIANCE.contains(&sel.race);
    let file = cat
        .and_then(|c| c.0.race_file(sel.race))
        .unwrap_or("Human")
        .to_uppercase();
    let hair_tok = cat
        .and_then(|c| c.0.hair_customization(sel.race))
        .unwrap_or("NORMAL")
        .to_string();
    let facial_tok = cat
        .and_then(|c| c.0.facial_hair_customization(sel.race, sel.sex))
        .unwrap_or("NORMAL")
        .to_string();

    for (icon, mut node) in &mut icons {
        let tc = match icon {
            DynIcon::Race(r) => art
                .races
                .as_ref()
                .and_then(|(h, size)| Some((h.clone(), tc_rect(*size, race_tc(*r, sel.sex)?)))),
            DynIcon::ClassSlot(i) => {
                let class = classes.get(*i as usize).copied();
                art.classes
                    .as_ref()
                    .and_then(|(h, size)| Some((h.clone(), tc_rect(*size, class_tc(class?)?))))
            }
            DynIcon::Info(InfoKind::Faction) => art.factions.as_ref().map(|(h, size)| {
                let half = if alliance {
                    [0.0, 0.5, 0.0, 1.0]
                } else {
                    [0.5, 1.0, 0.0, 1.0]
                };
                (h.clone(), tc_rect(*size, half))
            }),
            DynIcon::Info(InfoKind::Race) => art.races.as_ref().and_then(|(h, size)| {
                Some((h.clone(), tc_rect(*size, race_tc(sel.race, sel.sex)?)))
            }),
            DynIcon::Info(InfoKind::Class) => art
                .classes
                .as_ref()
                .and_then(|(h, size)| Some((h.clone(), tc_rect(*size, class_tc(sel.class)?)))),
        };
        if let Some((image, rect)) = tc {
            node.image = image;
            node.rect = Some(rect);
            node.color = Color::WHITE; // spawned transparent until the art lands
        }
    }

    for (text, mut t) in &mut texts {
        let new = match text {
            DynText::DialLabel(d) => match d {
                0 => strings
                    .text("CHAR_CUSTOMIZATION1_DESC", "Skin Color")
                    .to_string(),
                1 => strings.text("CHAR_CUSTOMIZATION2_DESC", "Face").to_string(),
                2 => strings
                    .text(&format!("HAIR_{hair_tok}_STYLE"), "Hair Style")
                    .to_string(),
                3 => strings
                    .text(&format!("HAIR_{hair_tok}_COLOR"), "Hair Color")
                    .to_string(),
                _ => strings
                    .text(&format!("FACIAL_HAIR_{facial_tok}"), "Facial Hair")
                    .to_string(),
            },
            // Painted by `refresh_name_box`.
            DynText::Name => continue,
            DynText::InfoTitle(InfoKind::Faction) => strings
                .text(
                    if alliance { "ALLIANCE" } else { "HORDE" },
                    if alliance { "Alliance" } else { "Horde" },
                )
                .to_string(),
            DynText::InfoTitle(InfoKind::Race) => race_name(sel.race).to_string(),
            DynText::InfoTitle(InfoKind::Class) => class_name(sel.class).to_string(),
            // Never trimmed: each `FACTION_INFO_*`, `RACE_INFO_*` and `CLASS_*` string opens with
            // eight spaces, which is the reference's first-line clearance past the header icon.
            DynText::InfoBody(InfoKind::Faction) => strings
                .text(
                    if alliance {
                        "FACTION_INFO_ALLIANCE"
                    } else {
                        "FACTION_INFO_HORDE"
                    },
                    "",
                )
                .to_string(),
            DynText::InfoBody(InfoKind::Race) => {
                strings.text(&format!("RACE_INFO_{file}"), "").to_string()
            }
            DynText::InfoAbilities => {
                // `CharacterCreateEnumerateRaces` joins every `ABILITY_INFO_<FILE><n>` by newline.
                let mut lines = Vec::new();
                let mut n = 1;
                while let Some(a) = strings.get(&format!("ABILITY_INFO_{file}{n}")) {
                    lines.push(a);
                    n += 1;
                }
                lines.join("\n")
            }
            DynText::InfoBody(InfoKind::Class) => strings
                .text(&format!("CLASS_{}", class_file(sel.class)), "")
                .to_string(),
            DynText::ClassSlotLabel(i) => classes
                .get(*i as usize)
                .map(|&c| class_name(c).to_string())
                .unwrap_or_default(),
        };
        if t.0 != new {
            t.0 = new;
        }
    }

    // The reference's `SetBackdropColor` tints only the bg; the border's entry is commented out.
    for (tint, bg, node) in &mut tints {
        let fill = if alliance { ALLIANCE_FILL } else { HORDE_FILL };
        match tint {
            DynTint::Backdrop => {
                if let Some(mut bg) = bg {
                    bg.0 = if alliance {
                        BACKDROP_ALLIANCE
                    } else {
                        BACKDROP_HORDE
                    };
                }
            }
            DynTint::BoxFill => {
                if let Some(mut node) = node {
                    node.color = fill; // the texture keeps its own alpha
                } else if let Some(mut bg) = bg {
                    bg.0 = fill.with_alpha(FALLBACK_ALPHA);
                }
            }
        }
    }

    // The facial dial hides when the race's token is `NONE`, as in the reference (no 1.12 row is).
    for (row, mut node) in &mut dial_rows {
        if row.0 == 4 {
            node.display = if facial_tok == "NONE" {
                Display::None
            } else {
                Display::Flex
            };
        }
    }

    // The reference fills the valid classes into the first buttons and hides the rest.
    for (action, mut node) in &mut slots {
        if let CreateAction::ClassSlot(i) = action {
            node.display = if (*i as usize) < classes.len() {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
}

/// The create screen's own per-frame visuals: the chosen icon's `LockHighlight`, hover labels,
/// the no-art fallback shade and Create's disabled latch; the shared passes are [`crate::glue`]'s.
/// The lock write keeps its own query, so a button without a `LockHighlight` still gets hover.
#[allow(clippy::type_complexity)]
pub(super) fn refresh_hover(
    sel: Res<CreateSelection>,
    catalog: Option<Res<CharCreate>>,
    art: Res<GlueArt>,
    // Accept, Back and Randomize are wholly the shared visuals pass's.
    mut buttons: Query<
        (
            &CreateAction,
            &Interaction,
            &Children,
            &mut BackgroundColor,
            Has<FallbackFace>,
        ),
        (With<Button>, Without<crate::glue::widgets::GlueBtn>),
    >,
    mut locks: Query<(&CreateAction, &mut LockHighlight)>,
    mut labels: Query<&mut Visibility, (With<HoverLabel>, Without<Hilight>)>,
    mut disables: Query<(&CreateAction, &mut GlueDisabled)>,
) {
    let cat = catalog.as_deref();
    let classes = race_classes(cat, sel.race);
    let selected = |action: &CreateAction| match *action {
        CreateAction::Race(r) => sel.race == r,
        CreateAction::Gender(g) => sel.sex == g,
        CreateAction::ClassSlot(i) => classes.get(i as usize) == Some(&sel.class),
        _ => false,
    };

    // `SetCharacterRace`/`Class`/`Gender`: `LockHighlight()` the chosen one, unlock the rest;
    // `glue_hilights` draws the sheen.
    for (action, mut locked) in &mut locks {
        let is_sel = selected(action);
        if locked.0 != is_sel {
            locked.0 = is_sel;
        }
    }

    for (action, interaction, children, mut bg, fallback) in &mut buttons {
        let is_sel = selected(action);
        let hovered = *interaction != Interaction::None;
        let lit = is_sel || hovered;
        // Every node has a `BackgroundColor`; only a `FallbackFace`'s is ours to shade.
        if fallback {
            bg.0 = if lit { BTN_HOVER } else { BTN_BG };
        }
        for child in children {
            if let Ok(mut vis) = labels.get_mut(*child) {
                // Without icon art the label is the button face.
                let show = lit || art.races.is_none();
                *vis = if show {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
            }
        }
    }

    for (action, mut disabled) in &mut disables {
        if matches!(action, CreateAction::Create) {
            let want = sel.creating;
            if disabled.0 != want {
                disabled.0 = want;
            }
        }
    }
}

/// Wheel-scroll a hovered info panel, the reference's GlueScrollFrame `OnMouseWheel`.
pub(super) fn scroll_info(
    mut wheel: MessageReader<MouseWheel>,
    mut boxes: Query<(&Interaction, &ComputedNode, &mut ScrollPosition), With<InfoScroll>>,
) {
    for ev in wheel.read() {
        let dy = match ev.unit {
            MouseScrollUnit::Line => ev.y * 24.0,
            MouseScrollUnit::Pixel => ev.y,
        };
        for (interaction, node, mut pos) in &mut boxes {
            if *interaction != Interaction::None {
                pos.0.y = (pos.0.y - dy).clamp(0.0, max_scroll(node));
            }
        }
    }
}

/// A scroll frame's maximum offset, logical px (`ComputedNode` sizes are physical).
fn max_scroll(node: &ComputedNode) -> f32 {
    ((node.content_size.y - node.size.y) * node.inverse_scale_factor).max(0.0)
}

/// The scrollbar's inputs (`GlueScrollBarTemplate`): an arrow click steps half a track,
/// `SetValue(GetValue() ± GetHeight()/2)`, and the knob drags.
pub(super) fn scroll_drive(
    motion: Res<AccumulatedMouseMotion>,
    arrows: Query<(Entity, &ScrollArrow)>,
    clicks: Res<crate::glue::GlueClicks>,
    thumbs: Query<(&ScrollThumb, &Interaction)>,
    mut scrolls: Query<(&ComputedNode, &mut ScrollPosition), With<InfoScroll>>,
) {
    // The arrow is a `<Button>` with an `<OnClick>`, so it steps on the release; the knob drags.
    for (entity, arrow) in &arrows {
        if !clicks.hit(entity) {
            continue;
        }
        let Ok((node, mut pos)) = scrolls.get_mut(arrow.scroll) else {
            continue;
        };
        let step = if arrow.up { -arrow.step } else { arrow.step };
        pos.0.y = (pos.0.y + step).clamp(0.0, max_scroll(node));
    }
    for (thumb, interaction) in &thumbs {
        if *interaction != Interaction::Pressed || motion.delta.y == 0.0 || thumb.travel <= 0.0 {
            continue;
        }
        let Ok((node, mut pos)) = scrolls.get_mut(thumb.scroll) else {
            continue;
        };
        let max = max_scroll(node);
        pos.0.y = (pos.0.y + motion.delta.y * max / thumb.travel).clamp(0.0, max);
    }
}

/// The scrollbar's look: the knob follows the scroll fraction, the arrows swap up, down and
/// disabled art, and every [`ScrollHides`] piece hides while there is nothing to scroll.
#[allow(clippy::type_complexity)]
pub(super) fn scroll_visuals(
    art: Res<GlueArt>,
    scrolls: Query<(&ComputedNode, &ScrollPosition), With<InfoScroll>>,
    mut arrows: Query<(
        &ScrollArrow,
        &Interaction,
        &mut ImageNode,
        &mut GlueDisabled,
    )>,
    mut thumbs: Query<(&ScrollThumb, &mut Node)>,
    mut hides: Query<(&ScrollHides, &mut Visibility), Without<Hilight>>,
) {
    let Some(sc) = &art.scroll else {
        return;
    };
    for (arrow, interaction, mut img, mut off) in &mut arrows {
        let Ok((node, pos)) = scrolls.get(arrow.scroll) else {
            continue;
        };
        let disabled = if arrow.up {
            pos.0.y <= 0.0
        } else {
            pos.0.y >= max_scroll(node)
        };
        let set = if arrow.up { &sc.up_btn } else { &sc.down_btn };
        let face = if disabled {
            &set.dis
        } else if *interaction == Interaction::Pressed {
            &set.down
        } else {
            &set.up
        };
        if img.image != *face {
            img.image = face.clone();
        }
        // The shared sheen pass reads this.
        if off.0 != disabled {
            off.0 = disabled;
        }
    }
    for (thumb, mut node) in &mut thumbs {
        let Ok((cn, pos)) = scrolls.get(thumb.scroll) else {
            continue;
        };
        let max = max_scroll(cn);
        let frac = if max > 0.0 {
            (pos.0.y / max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        node.top = Val::Px(frac * thumb.travel);
    }
    for (hide, mut vis) in &mut hides {
        let Ok((cn, _)) = scrolls.get(hide.scroll) else {
            continue;
        };
        *vis = if max_scroll(cn) > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}
