//! The click router: a clean left-click selects, a clean right-click takes the context action,
//! and a loot close's deselect clears the same [`super::Selection`].

use super::lock::GoLockInputs;
use super::*;
use crate::query_cache::Ask;

/// The right-click payload leg of the reference's click router (`0x481f60`, clean clicks only,
/// `0x514ae0`), for a payload still held at the release: the hook that clears one at the press
/// (`benilla_ui::script::UiScript::mouse_button`, `0x492b50`) has already run unless a pickup came
/// after it. Over terrain (`0x492c90`) or nothing (`0x492d30`) any payload clears, silently. Over
/// an object (a unit, a corpse, a GameObject: `0x492ce0`) only a preview one clears, the vendor
/// row or bar item that sets `[0xb4b41c]`. A charmed player's click is the nothing leg
/// (`0x481f98`). A nameplate's right-click does not come through here (`0x4949f0`).
pub(super) fn world_right_click_payload(
    mut right_clicks: MessageReader<WorldRightClick>,
    // The press pick: this leg and [`act_on_right_click`] must classify the same pick.
    press: Res<PressPick>,
    me: Query<&ObjectStore, With<SelfPlayer>>,
    mut hosted_vm: crate::script_calls::HostedVm,
) {
    hosted_vm.run(|script| {
        if right_clicks.read().last().is_none() {
            return;
        }
        let Some(script) = script else {
            return;
        };
        let charmed = me.iter().any(|me| me.0.unit_charmed_by().is_some());
        if press.over_object() && !charmed {
            script.clear_preview_payload();
        } else {
            script.clear_cursor_payload();
        }
    })
}

/// A right-click begun on a nameplate, replayed for [`act_on_interact`]: the plate's click slot
/// (`0x7cb910`) goes to `0x4949f0` and so to `InteractWithObjectByClick` (`0x492820`), not
/// through the world click router (`0x481f60`), whose charmed return and payload legs it skips.
/// It carries what the click's `SelectTarget` (`0x492870`) did, which ran at the click.
#[derive(Message, Clone, Copy)]
pub(super) struct PlateRightClick {
    pub(super) selected: scan::CommitOutcome,
}

/// A plate click is a click on its unit (the click slot `0x7cb910`). The left button carries the
/// plate's unit, as a scripted `plate:Click()` (pfUI, `nameplates.lua:1228`) has no cursor; the
/// right replays [`PlateRightClick`] only when the press pick names it, so a scripted one does
/// nothing yet. A physical right-click arrives only here: `0x7cb910` → `0x4949f0` → `0x492820`.
///
/// The left click is the body click's select (`0x4949f0` action 1 → `0x4925d0` → `SelectTarget`
/// `0x49280c`), so with the targeting cursor up the unit goes to the cast's bind and the target
/// stays ([`super::front`]). The right is the interact (`0x492820`): its `SelectTarget`
/// (`0x492870`) runs here and binds the same way, and the unit's interact (`0x492881`) follows
/// either way, in [`act_on_interact`], after the bind has emptied the cursor's word; the plate's
/// press never reached the world frame's right-press targeting cancel (`0x492c20`), so the cursor
/// stands up to here.
pub(super) fn select_on_plate_click(
    mut plate: ResMut<crate::vplates::PlateClicks>,
    press: Res<PressPick>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    mut front: super::front::SelectFront,
    self_q: Query<(&Guid, Has<Engaged>), With<SelfPlayer>>,
    mut greeting: MessageWriter<crate::sound::NpcGreetingRequest>,
    mut right_clicks: MessageWriter<PlateRightClick>,
    units: Query<(&Guid, Option<&ObjectStore>)>,
) {
    let (left, right) = (
        std::mem::take(&mut plate.left),
        std::mem::take(&mut plate.right),
    );
    let (self_guid, engaged) = self_q
        .single()
        .map(|(g, e)| (Some(g.0), e))
        .unwrap_or((None, false));
    for entity in left {
        let Ok((guid, store)) = units.get(entity) else {
            continue; // the unit left between the click and this frame
        };
        // The greeting fires on the select, plate or body, before SetTarget (`0x60c270`).
        greeting.write(crate::sound::NpcGreetingRequest { npc: entity });
        // `SelectTarget`'s returns: charmed, or the cursor's bind in the select's place.
        if front.takes(entity, guid.0, seam.slots.targeting()) {
            continue;
        }
        // Attack-classified only when the press was on this unit: a scripted click has no cursor.
        let attack = press.attack() && press.hovered.target == Some(entity);
        scan::commit(
            &mut selection,
            &mut seam,
            entity,
            guid.0,
            store,
            engaged,
            self_guid,
            attack,
        );
    }
    for entity in right {
        // The press pick must name it: a scripted click has no cursor to interact by.
        if press.hovered.target != Some(entity) {
            continue;
        }
        let Ok((guid, store)) = units.get(entity) else {
            continue;
        };
        let selected = if front.takes(entity, guid.0, seam.slots.targeting()) {
            scan::CommitOutcome::default()
        } else {
            scan::commit(
                &mut selection,
                &mut seam,
                entity,
                guid.0,
                store,
                engaged,
                self_guid,
                press.attack(),
            )
        };
        right_clicks.write(PlateRightClick { selected });
    }
}

/// On a [`WorldClick`], select the press pick's unit or deselect on empty world, except on nothing
/// (the sky) with a payload held (`0x492d30`); the terrain leg deselects regardless (`0x5e03bb`).
/// With the targeting cursor up, the object it picks binds instead and a terrain click its word
/// can bind binds, while the rest still deselect. The press pick, since the hover is empty
/// through a camera drag, as in the reference's freelook.
pub(super) fn select_on_click(
    mut clicks: MessageReader<WorldClick>,
    inspect: Res<InspectMode>,
    press: Res<PressPick>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    self_q: Query<(&Guid, Has<Engaged>), With<SelfPlayer>>,
    payload_held: Res<crate::ui_script::CursorPayloadHeld>,
    mut greeting: MessageWriter<crate::sound::NpcGreetingRequest>,
    click_cfg: Res<ClickConfig>,
    // Read at the commit, where the reference resolves it for `0x493540`'s `IsSelectable` gate.
    stores: Query<&ObjectStore>,
) {
    let (hovered, occlusion) = (press.hovered, press.occlusion);
    let clicked = clicks.read().last().is_some();
    if !clicked || inspect.enabled {
        return;
    }
    // The click router returns for a charmed player before anything (`0x481f98`), greeting included.
    if seam.player_is_charmed() {
        return;
    }
    // With the cursor up, an object the press picked takes the object leg (`0x4925d0` →
    // `SelectTarget 0x493540`), whose word test (`0x4935ca`) binds it in the select's place; the
    // commit systems run after this one. The greeter runs ahead of `SelectTarget` for every unit
    // the click names (`0x4927fd`) and nothing before it tests targeting, so the unit the cursor's
    // click binds is greeted as well, unless a nearer GameObject takes the click. The terrain leg
    // (`0x492580`) tries `BindLocation 0x6e60f0`, which binds a word with a location bit and
    // returns 1; for any other word it signals (`0x6e615d`) and returns 0, and the click falls to
    // `OnWorldClick 0x5e0320`'s deselect (`0x5e03cf`). The nothing leg deselects as ever
    // (`0x492e5d`): neither tests the word.
    let ground = seam.slots.targeting();
    if ground.active() {
        let object_click = ground.wants(crate::spell::TargetingWants::GameObject)
            && go_is_nearest(&hovered, &press.object);
        if object_click || hovered.corpse.is_some() || hovered.refused {
            return;
        }
        if let Some(entity) = hovered.target {
            greeting.write(crate::sound::NpcGreetingRequest { npc: entity });
            return;
        }
        if occlusion.point.is_some() {
            if ground.wants(crate::spell::TargetingWants::Location) {
                return;
            }
            seam.slots.core(&seam.net).reports.changed();
        }
    }
    let (self_guid, engaged) = self_q
        .single()
        .map(|(g, e)| (Some(g.0), e))
        .unwrap_or((None, false));
    match (hovered.target, hovered.guid) {
        (Some(entity), Some(guid)) => {
            // The greeter `0x60c270` runs before SetTarget, so on the left-click select only; every
            // click fires it, so a re-click steps the variation.
            greeting.write(crate::sound::NpcGreetingRequest { npc: entity });
            // The cursor's Attack classification is `0x5ecb70`'s new-target validation.
            scan::commit(
                &mut selection,
                &mut seam,
                entity,
                guid,
                stores.get(entity).ok(),
                engaged,
                self_guid,
                press.attack(),
            );
        }
        // Deselect, except on the sky with a payload held (reachable only over a GameObject, as a
        // payload's empty-world press is the drop's); `deselectOnClick` 0 keeps the target.
        _ => {
            // A corpse or a refused unit is an object hit, which never deselects: the down-edge
            // pick `0x481f00` still finds a `NOT_SELECTABLE` unit, and `0x493540` refuses it.
            if hovered.corpse.is_some() || hovered.refused {
                return;
            }
            if click_cfg.deselect_on_click && (!payload_held.0 || occlusion.distance.is_finite()) {
                deselect(&mut selection, &mut seam, engaged);
            }
        }
    }
}

/// The service arms that are not a bare packet, bundled for the 16-param ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ServiceArms<'w> {
    /// The last `SMSG_QUESTGIVER_STATUS` per guid (`[unit+0xcb8]`), read by bit 1's `0x5df490`.
    pub(crate) quest: Res<'w, crate::ui_quest::QuestGiver>,
    /// Bit 7: `0x5dfdc0` fires `CONFIRM_BINDER`; `CMSG_BINDER_ACTIVATE` is the dialog's Accept.
    pub(crate) binder: ResMut<'w, crate::ui_binder::BinderState>,
    pub(crate) death: ResMut<'w, crate::death::DeathNet>,
    /// Bit 6 sends through the cache the proximity poll writes, so the two agree on the healer.
    pub(crate) spirit: ResMut<'w, crate::ui_dialog_verbs::AreaSpiritHealer>,
    /// The NPC whose window is open (`[0xb4e2d0]`), checked before the ladder.
    pub(crate) interact: Res<'w, crate::ui_session::InteractNpc>,
}

/// The re-click gate (`0x5f0251`), before the ladder: a right-click on the NPC whose window is
/// open (`[0xb4e2d0]`) does nothing, no packet, no error, no gesture. The reference arms that NPC
/// only from window openers (`0x4930d0`, cleared at `0x493310`), never on the click, the set
/// [`crate::ui_session::feed_interact_npc`] publishes.
fn interaction_already_open_on(target: u64, interact: &crate::ui_session::InteractNpc) -> bool {
    interact.1 == Some(target)
}

/// The refusals and openers the dispatchers raise.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Feedback<'w, 's> {
    errors: Res<'w, crate::ui_action::UiErrorKeys>,
    // The mailbox's open, which registers on the interaction latch with the world in hand.
    windows: Commands<'w, 's>,
    item_text: ResMut<'w, crate::ui_item_text::ItemTextOpen>,
    // The cast queue: this system cannot also hold `CastLadder` (a second `Items` and
    // `CastReports` borrow), so the lock verdict and the skin go to
    // `ui_action::drain::drain_click_casts`.
    casts: ResMut<'w, crate::ui_action::ClickCasts>,
    // The stone's refusals need the roster, so they run in `drain_meeting_stone_joins`.
    stone_uses: MessageWriter<'w, crate::ui_dialog_verbs::MeetingStoneUse>,
}

impl Feedback<'_, '_> {
    /// Whether a walk started; the leash's refusal says `ERR_AUTOFOLLOW_TOO_FAR` (`0x61110c`).
    fn walked(&mut self, started: Result<(), crate::player::Refused>) -> bool {
        match started {
            Ok(()) => true,
            Err(crate::player::Refused::TooFar) => {
                self.errors
                    .push(crate::ui_action::UiError::key("ERR_AUTOFOLLOW_TOO_FAR"));
                false
            }
            Err(crate::player::Refused::Silent) => false,
        }
    }
}

enum MeleeApproach {
    Face(Vec3),
    Walk { at: Vec3, stop: f32 },
}

/// The dispatchers, which take the object (`0x5f0130`, `0x5df2a0`, `0x5df130`, `0x5f86b0`,
/// `0x5f05e0`): a right-click reaches them through [`act_on_right_click`] and a crate's
/// [`Interact`] through [`act_on_interact`], both with Click to Move's walk, an approach's arrival
/// through [`act_on_arrival`] without it.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Dispatch<'w, 's> {
    seam: crate::creature_anim::AttackSeam<'w, 's>,
    /// The returns ahead of the select both attack-or-click entries make (`SelectTarget`,
    /// [`super::front`]).
    front: super::front::SelectFront<'w, 's>,
    self_player: Query<'w, 's, (Entity, &'static Guid, Has<Engaged>), With<SelfPlayer>>,
    // Through `creature_anim::gesture`, the chat path's entry, as the reference's dispatcher does.
    gestures: ResMut<'w, crate::creature_anim::GestureQueue>,
    go_inputs: GoLockInputs<'w, 's>,
    player_actions: Res<'w, crate::ui_action::PlayerActions>,
    // The skin leg's spells (`[0xb700e4]`, `[0xb700e8]`), already gated by the classifier.
    learned: Res<'w, crate::ui_action::LearnedAbilities>,
    // The GameObject leg also reads the object's anim state (the Action gate).
    stores: Query<
        'w,
        's,
        (
            &'static ObjectStore,
            Option<&'static crate::go_anim::GoAnim>,
        ),
    >,
    places: Query<'w, 's, &'static Transform>,
    service: ServiceArms<'w>,
    feedback: Feedback<'w, 's>,
    /// The walks, whose stand-up goes through the seam's writer: a system holds one.
    auto: crate::player::AutoMove<'w, 's>,
    player: Res<'w, crate::player::Player>,
}

/// The Loot walk's stop radius: the arm (`0x611130`) takes the square root (`0x6111ab`) of what
/// the mode's stop function stores, and Loot's is the linear melee reach (`0x6112b6`), 5.0 for a
/// corpse (`0x611317`).
fn loot_stop(reach: f32) -> f32 {
    reach.sqrt()
}

impl Dispatch<'_, '_> {
    fn me(&self) -> Option<(Entity, u64, bool)> {
        self.self_player
            .single()
            .ok()
            .map(|(e, g, engaged)| (e, g.0, engaged))
    }

    fn self_store(&self) -> Option<&ObjectStore> {
        let (me, ..) = self.me()?;
        self.stores.get(me).ok().map(|(s, _)| s)
    }

    /// Any `0x20ff` bit in our last streamed movement word: a direction, turn, pitch or fall.
    fn moving(&self) -> bool {
        self.player.move_flags() & crate::creature_anim::move_flags::INTEGRATED != 0
    }

    /// The reference's one mounted predicate, the player's `UNIT_FIELD_MOUNTDISPLAYID`.
    fn mounted(&self) -> bool {
        self.self_store()
            .is_some_and(|s| s.0.unit_mount_display_id() != 0)
    }

    /// Centre to centre, as the cursor measures.
    fn dist_sq(&self, target: Entity) -> Option<f32> {
        let (me, ..) = self.me()?;
        let a = self.places.get(me).ok()?.translation;
        Some(a.distance_squared(self.places.get(target).ok()?.translation))
    }

    fn melee_reach(&self, target: Entity) -> f32 {
        match (self.stores.get(target).ok(), self.self_store()) {
            (Some((it, _)), Some(me)) => cursor_mode::melee_reach(it, me),
            _ => cursor_mode::MELEE_FLOOR,
        }
    }

    // The unit legs measure their own reach, as the reference's dispatchers do, never the
    // press's latched cursor: with a spell cursor up at the press that gray is the spell's
    // verdict (`spell::targeting::cursor`), not the interaction's.

    /// `0x5f0130`'s range test (`0x5f01b9`-`0x5f01fd`, `0x5f0238`): centre to centre against
    /// 5.5556 yd squared, boundary-inclusive, a NaN passing.
    fn in_service_reach(&self, target: Entity) -> bool {
        let d2 = self.dist_sq(target).unwrap_or(f32::INFINITY);
        d2.is_nan() || d2 <= SERVICE_RANGE_SQ
    }

    /// Inside the melee reach, which the skin cast's range rides (`0x6e3480`).
    fn in_melee_reach(&self, target: Entity) -> bool {
        let reach = self.melee_reach(target);
        self.dist_sq(target).is_some_and(|d2| d2 <= reach * reach)
    }

    /// `CanLootNow 0x5ec110`: the melee reach, skipped under `CanAutoInteract` (`0x5ec11b`).
    fn can_loot_unit(&self, target: Entity) -> bool {
        self.auto.can_auto_interact() || self.in_melee_reach(target)
    }

    fn approach(
        &mut self,
        verb: crate::player::ApproachVerb,
        target: Entity,
        guid: u64,
        subject: crate::player::Subject,
        stop: f32,
    ) -> bool {
        let Ok(at) = self.places.get(target).map(|t| t.translation) else {
            return false;
        };
        let started = self
            .auto
            .start(verb, guid, subject, at, stop, &mut self.seam.stand);
        self.feedback.walked(started)
    }

    /// `0x60fcc0`'s two arms for an attack: inside the melee reach `max(rA + rB + 1.3333, 5)` a
    /// turn to face the enemy (`0x6100a0`), beyond it a walk to where it stands, stopping at the
    /// square root (`0x6111ab`) of the reach less its offset (`0x61131d`), measured between the
    /// player and itself, as the arm names the player's own guid (`0x60fe5f`).
    fn melee_approach(&self, target: Entity) -> Option<MeleeApproach> {
        let d2 = self.dist_sq(target)?;
        let at = self.places.get(target).ok()?.translation;
        let reach = self.melee_reach(target);
        if d2 < reach * reach {
            return Some(MeleeApproach::Face(at));
        }
        let own = self.self_store().map_or(cursor_mode::MELEE_FLOOR, |me| {
            cursor_mode::melee_reach(me, me)
        });
        let stop = (own - cursor_mode::MELEE_OFFSET).sqrt();
        Some(MeleeApproach::Walk { at, stop })
    }

    /// A GameObject out of its use range (`0x5f3346`, error `0xe1`): the approach (`0x610300`) when
    /// `walk` and the object is no fishing bobber (`0x5f8705`), which stops short of this object's
    /// range. A walk that does not start, the leash's `ERR_AUTOFOLLOW_TOO_FAR` first, leaves
    /// `ERR_USE_TOO_FAR` (`0x5f874b`). Deviation: the reference's arm reads the last object's
    /// stop (`0x611336` before `0x610383` stores this one's), so its first walk has no stop.
    fn walk_to_object(&mut self, entity: Entity, guid: u64, go_type: i32, walk: bool) -> bool {
        let walked = walk
            && go_type != cursor_mode::GO_TYPE_FISHINGNODE
            && self.approach(
                crate::player::ApproachVerb::Use,
                entity,
                guid,
                crate::player::Subject::GameObject,
                crate::player::RANGE_STOP_FRACTION
                    * cursor_mode::go_interact_range_sq(go_type).sqrt(),
            );
        if !walked {
            self.feedback
                .errors
                .push(crate::ui_action::UiError::key("ERR_USE_TOO_FAR"));
        }
        walked
    }

    /// `0x5f86b0`, `OnUse` past its highlightable gate: `usable`'s player arms, its type's use
    /// handler, then its lock. A usable object out of `in_reach` goes to
    /// [`Self::walk_to_object`].
    fn use_gameobject(&mut self, entity: Entity, guid: u64, in_reach: bool, walk: bool) {
        let self_store = self
            .me()
            .and_then(|(e, ..)| self.stores.get(e).ok())
            .map(|(s, _)| s);
        let go = self
            .stores
            .get(entity)
            .ok()
            .map(|(s, anim)| (s, crate::go_anim::go_state(anim, s)));
        let lock_id = go
            .and_then(|(s, _)| self.go_inputs.templates.of(&s.0))
            .map_or(0, |t| t.lock_id);
        let has_lock_row = lock_id != 0
            && self
                .go_inputs
                .locks
                .as_deref()
                .is_some_and(|l| l.0.slots(lock_id).is_some());
        let go_type = go.map_or(-1, |(s, _)| s.0.gameobject_type_id());
        // The player arms refuse ahead of the range arm, so a dead player is never walked there,
        // and before the opener: no packet, no cast, no mail window.
        if let Some(refusal) =
            self_store.and_then(|me| cursor_mode::go_player_refusal(me, has_lock_row, go_type))
        {
            debug!("right-click gameobject {guid:#x}: refused, {refusal:?} (type {go_type})");
            if let Some(key) = refusal.error_key() {
                self.feedback
                    .errors
                    .push(crate::ui_action::UiError::key(key));
            }
            return;
        }
        // MAILBOX (type 19) opens locally: its use handler `0x5f6820` sends no
        // `CMSG_GAMEOBJ_USE`, and `MAIL_SHOW` → `CheckInbox` asks for the list.
        if go_type == cursor_mode::GO_TYPE_MAILBOX {
            if in_reach {
                debug!("right-click mailbox: open mail window {guid:#x}");
                self.feedback
                    .windows
                    .queue(move |world: &mut World| crate::ui_mail::open_mailbox(world, guid));
            } else {
                self.walk_to_object(entity, guid, go_type, walk);
            }
            return;
        }
        // TEXT (type 9) opens locally (`0x5f58c0` → `0x4e32e0(goGuid, 0)`; vmangos' `Use` has no
        // type-9 case); a re-click closes it, and the page is read at paint time.
        if go_type == cursor_mode::GO_TYPE_TEXT {
            if !in_reach {
                self.walk_to_object(entity, guid, go_type, walk);
            } else if self.feedback.item_text.read_pages(guid) {
                debug!("right-click text gameobject: re-click closes {guid:#x}");
            } else {
                debug!("right-click text gameobject: read {guid:#x}");
            }
            return;
        }
        // MEETINGSTONE (type 23) has its own use slot `0x5f69d0`: four client-side refusals, then
        // `CMSG 0x292 {u64 goGuid}` (`0x4c9ff0`), never `CMSG_GAMEOBJ_USE`, whose type-23 arm in
        // vmangos does nothing (`GameObject.cpp:1836`). Types 9, 19, 23 and 28 override the shared
        // sender `0x5f33e0`.
        if go_type == cursor_mode::GO_TYPE_MEETINGSTONE {
            if in_reach {
                debug!("right-click meeting stone: {guid:#x}");
                self.feedback
                    .stone_uses
                    .write(crate::ui_dialog_verbs::MeetingStoneUse { go_guid: guid });
            } else {
                self.walk_to_object(entity, guid, go_type, walk);
            }
            return;
        }
        // By lock: used, opened by its opener cast, or refused locally (`0x5f3427..`).
        match resolve_go_action(
            guid,
            &mut self.go_inputs,
            &self.player_actions.spells,
            go,
            self_store,
            &self.seam.net,
        ) {
            GoAction::Use | GoAction::OpenLock(_) | GoAction::OpenByKey(_) if !in_reach => {
                self.walk_to_object(entity, guid, go_type, walk);
            }
            GoAction::Use => {
                debug!("right-click gameobject use: {guid:#x}");
                if benilla_assets::trace::enabled_for("use") {
                    benilla_assets::trace::line(
                        "use",
                        &format!("SEND CMSG_GAMEOBJ_USE guid={guid:#x}"),
                    );
                }
                let _ = self.seam.net.0.send(ClientCommand::GameObjUse { guid });
            }
            // Both opener arms queue for the one cast path, as the reference reaches `TryCast
            // 0x6e4b60` from the use sender (`0x5f35c0`) like a button press.
            GoAction::OpenLock(spell_id) => {
                debug!("right-click gameobject open-lock: cast {spell_id} at {guid:#x}");
                self.feedback
                    .casts
                    .0
                    .push(crate::ui_action::ClickCast::AtObject {
                        spell_id,
                        go_guid: guid,
                    });
            }
            GoAction::OpenByKey(it) => {
                debug!(
                    "right-click gameobject open-by-key: use item ({},{}) blk {} at {guid:#x}",
                    it.bag_index, it.slot, it.spell_index
                );
                self.feedback
                    .casts
                    .0
                    .push(crate::ui_action::ClickCast::Key(it));
            }
            GoAction::Refuse(err) => {
                debug!("right-click gameobject {guid:#x}: locked, refused ({err:?})");
                if let Some(err) = err {
                    self.feedback.errors.push(err);
                }
            }
        }
    }

    /// `0x5df130`: `CMSG_LOOT` at a lootable corpse, never while [`Self::moving`] (`0x5df173`),
    /// walked to first when `walk` and beyond its 5 yd (`0x5df1c3`), stopping at [`loot_stop`] of
    /// 5. A walk that does not start falls through to the send.
    fn loot_corpse(&mut self, entity: Entity, guid: u64, walk: bool) {
        if self.moving() {
            return;
        }
        if walk
            && self
                .dist_sq(entity)
                .is_some_and(|d2| d2 > cursor_mode::CORPSE_INTERACT_RANGE_SQ)
            && self.approach(
                crate::player::ApproachVerb::Loot,
                entity,
                guid,
                crate::player::Subject::Corpse,
                loot_stop(cursor_mode::MELEE_FLOOR),
            )
        {
            return;
        }
        debug!("right-click corpse loot: {guid:#x}");
        let _ = self.seam.net.0.send(ClientCommand::Loot { guid });
        // Predicted, as for a unit corpse (`[player+0x1d28]` armed at the send).
        self.seam.loot_latch.0 = Some(guid);
    }

    /// `0x5df2a0`: `CMSG_LOOT` at a dead unit, which `can_loot` (`CanLootNow 0x5ec110`) must
    /// allow, never while [`Self::moving`] (`0x5df2e9`), walked to first when `walk` and beyond
    /// melee reach. A walk that does not start falls through to the send.
    fn loot_unit(&mut self, entity: Entity, guid: u64, can_loot: bool, walk: bool) {
        if !can_loot || self.moving() {
            return;
        }
        let reach = self.melee_reach(entity);
        if walk
            && self.dist_sq(entity).is_some_and(|d2| d2 > reach * reach)
            && self.approach(
                crate::player::ApproachVerb::Loot,
                entity,
                guid,
                crate::player::Subject::Unit { dead: true },
                loot_stop(reach),
            )
        {
            return;
        }
        debug!("right-click loot: {guid:#x}");
        let _ = self.seam.net.0.send(ClientCommand::Loot { guid });
        // Predicted at the send, as the reference's sender (`0x5df253`) arms `[player+0x1d28]` and
        // kneels before any reply; the anim driver reads the latch.
        self.seam.loot_latch.0 = Some(guid);
    }

    /// `InteractWithSpell 0x5f05e0`: the skin cast at a body. Out of reach it walks there when
    /// `walk` and casts nothing (`0x5f06a1`-`0x5f06c4`); otherwise, in reach or not, it casts
    /// through the attempt with the body's guid (`0x5f06dc` → `0x6e5a90` → `0x6e4b60`), whose
    /// rungs refuse it as they refuse a press: another spell in flight (`0x6e4d97`), the range
    /// test (`0x6e4f26`), the validator (`0x6e4f3b`), and a passing one is pushed in flight
    /// (`0x6e5026`) and sent (`0x6e54f0`). Queued for [`crate::ui_action::ClickCasts`]' drain.
    fn skin(
        &mut self,
        entity: Entity,
        guid: u64,
        spell_id: u32,
        subject: crate::player::Subject,
        in_reach: bool,
        walk: bool,
    ) {
        if !in_reach && walk {
            let reach = match subject {
                crate::player::Subject::Corpse => cursor_mode::MELEE_FLOOR,
                _ => self.melee_reach(entity),
            };
            let stop = crate::player::RANGE_STOP_FRACTION * reach;
            self.approach(
                crate::player::ApproachVerb::Skin,
                entity,
                guid,
                subject,
                stop,
            );
            return;
        }
        debug!("right-click skin: {guid:#x} (spell {spell_id})");
        self.feedback
            .casts
            .0
            .push(crate::ui_action::ClickCast::AtUnit {
                spell_id,
                entity,
                guid,
            });
    }

    /// Whether the player's form bars NPCs (`IsShapeShifted`, `0x60e9f0`, and the form row's flag
    /// `0x8`, `0x5f017f`).
    fn shapeshifted_from_npcs(&self) -> bool {
        let form = self.self_store().map_or(0, |s| s.0.unit_shapeshift_form());
        self.go_inputs
            .spells
            .as_deref()
            .is_some_and(|spells| crate::ui_session::form_bars_interaction(&spells.forms, form))
    }

    /// `0x5f0130`: the NPC-service ladder. It opens with the form test (`0x5f0150`): a player in a
    /// form that bars NPCs gets `ERR_CANT_INTERACT_SHAPESHIFTED` (`0x5f0185`, message 351) and
    /// nothing else, no walk and no gesture, in reach or not. Out of the 5.5556 yd reach it then
    /// walks there when `walk`, returning before the ladder and the gesture (`0x5f022f`), and
    /// otherwise sends nothing. `script` is the VM, for the vendor arm's sell fork (`0x5df5d7`) and
    /// the spirit guide's event; `None` when no VM is mounted.
    fn talk(
        &mut self,
        mut script: Option<&mut benilla_ui::script::UiScript>,
        entity: Entity,
        guid: u64,
        walk: bool,
    ) {
        if self.shapeshifted_from_npcs() {
            debug!("right-click interact: {guid:#x} — refused, shapeshifted");
            self.feedback.errors.push(crate::ui_action::UiError::key(
                "ERR_CANT_INTERACT_SHAPESHIFTED",
            ));
            return;
        }
        if !self.in_service_reach(entity) {
            if walk {
                self.approach(
                    crate::player::ApproachVerb::Talk,
                    entity,
                    guid,
                    crate::player::Subject::Unit { dead: false },
                    crate::player::TALK_STOP,
                );
            }
            return;
        }
        // The reference's own `UNIT_NPC_FLAGS` ladder (`0x5f0289`), not the cursor kind, which its
        // cursor ladder (`0x482336`) projects lossily; the re-click gate comes first.
        if interaction_already_open_on(guid, &self.service.interact) {
            debug!("right-click interact: {guid:#x} — its window is already open, nothing sent");
            return;
        }
        let npc_flags = self
            .stores
            .get(entity)
            .map(|(s, _)| s.0.unit_npc_flags())
            .unwrap_or(0);
        let me = self.me();
        let self_store = self.self_store();
        // Peeked, not taken: the cursor clears only once a sale goes out. The reference sends the
        // cursor's stored guid; we resolve the held `(bag, slot)`, which a pickup locks, and a slot
        // with no guid opens the list instead.
        let cursor_sale = script
            .as_deref()
            .and_then(|s| s.cursor_item())
            .and_then(|item| {
                let slot0 = u8::try_from(item.slot.saturating_sub(1)).unwrap_or(0);
                self_store.and_then(|s| {
                    crate::ui_items::slot_guid(&s.0, item.bag, slot0, &self.go_inputs.objects)
                })
            });
        let ghost = self_store.is_some_and(|s| s.0.player_is_ghost());
        let Some(arm) = service_arm(npc_flags, self.service.quest.status(guid)) else {
            // `0x5f05ca`: no consulted bit does nothing, gesture included.
            debug!("right-click interact: {guid:#x} matches no service bit — nothing sent");
            return;
        };
        match service_action(arm, guid, ghost, cursor_sale) {
            ServiceAction::Send(cmd) => {
                debug!("right-click interact: {guid:#x} ({arm:?})");
                let _ = self.seam.net.0.send(cmd);
            }
            ServiceAction::SellFromCursor(cmd) => {
                debug!("right-click interact: {guid:#x} (vendor — selling the held item)");
                let _ = self.seam.net.0.send(cmd);
                // The sell clear: the slot stays greyed until the server's update.
                if let Some(script) = script.as_deref_mut() {
                    script.take_cursor_item_for_sale();
                }
            }
            ServiceAction::AskBinder => {
                debug!("right-click interact: {guid:#x} (innkeeper — CONFIRM_BINDER, no packet)");
                self.service.binder.ask(guid);
            }
            ServiceAction::AskSpiritHealer => {
                debug!(
                    "right-click interact: {guid:#x} (spirit healer — CONFIRM_XP_LOSS, no packet)"
                );
                self.service.death.ask_spirit_healer(guid);
            }
            ServiceAction::AcquireSpiritGuide => {
                debug!("right-click interact: {guid:#x} (spirit guide — adopting, 0x2E2)");
                let outcome = self.service.spirit.click_guide(guid);
                if outcome.cancel_aura {
                    if let Some(script) = script {
                        script.fire_event("AREA_SPIRIT_HEALER_OUT_OF_RANGE", vec![]);
                    }
                    let _ = self.seam.net.0.send(ClientCommand::CancelAura {
                        spell_id: crate::ui_dialog_verbs::AREA_SPIRIT_HEALER_AURA,
                    });
                }
                if let Some(healer) = outcome.query {
                    let _ = self
                        .seam
                        .net
                        .0
                        .send(ClientCommand::AreaSpiritHealerQuery { healer });
                }
            }
            ServiceAction::Silent(why) => {
                debug!("right-click interact: {guid:#x} ({arm:?}) — silent: {why}");
            }
        }
        // Talk on every taken arm, silent ones too (each arm of `0x5f0130` ends `call
        // 0x60bb30(0)`); the anim's WeaponFlags `0x10` stows a drawn weapon for good.
        if let Some((_, my_guid, _)) = me {
            self.gestures
                .push(my_guid, crate::creature_anim::Gesture::Talk);
        }
    }
}

/// The world click router (`0x481f60`) on the press pick, for a right-click. A charmed player
/// takes the nothing leg with zero points (`0x481f98`, `0x48204e`), so nothing is selected,
/// interacted with or walked to; otherwise the ground and the sky walk (`0x5e0378`, `0x492d50`)
/// and an object takes the INTERACT leg `0x492820`: select it, then [`interact`]. A nameplate's
/// click does not come through here ([`PlateRightClick`]).
pub(super) fn act_on_right_click(
    mut clicks: MessageReader<WorldRightClick>,
    // The press pick: the reference picks once, on the down edge (`0x481f00`).
    press: Res<PressPick>,
    // The dispatchers and the selection, which the host's calls also reach: an engine turn.
    mut vm: crate::script_calls::VmWith<(Dispatch<'static, 'static>, ResMut<'static, Selection>)>,
    payload_held: Res<crate::ui_script::CursorPayloadHeld>,
) {
    if clicks.read().last().is_none() || vm.own().0.seam.player_is_charmed() {
        return;
    }
    vm.turn(|script, (mut dispatch, mut selection)| {
        // The router's other legs: the world hit walks there (`0x5e0378`), and nothing walks along
        // the ray when the cursor is empty (`0x492d50`).
        if !press.over_object() {
            match (press.occlusion.point, press.occlusion.ray) {
                (Some(at), _) => {
                    dispatch.auto.walk_to_ground(at, &mut dispatch.seam.stand);
                }
                (None, Some(ray)) if !payload_held.0 => {
                    dispatch
                        .auto
                        .walk_toward_sky(*ray, &mut dispatch.seam.stand);
                }
                _ => {}
            }
            return;
        }
        interact(&mut dispatch, script, &press, Select::First(&mut selection));
    })
}

/// Where [`interact`] selects its unit, if it does: the world click's entry (`0x492820`) selects
/// first, whatever leg follows; a nameplate's click made that select already, at the click
/// ([`select_on_plate_click`]), and the dispatcher runs after the cast it bound; a crate's
/// [`Interact`] runs the dispatcher alone. The attack leg selects again through StartAttack
/// (`0x5ecb70`, `SetSelection` at `0x5eccaa`), which finds the unit selected or, after a bind
/// emptied the cursor's word, selects it.
enum Select<'a, 'w> {
    First(&'a mut ResMut<'w, Selection>),
    Done(&'a mut ResMut<'w, Selection>, scan::CommitOutcome),
    OnAttack(&'a mut ResMut<'w, Selection>),
}

impl<'a, 'w> Select<'a, 'w> {
    fn selection(&mut self) -> &mut ResMut<'w, Selection> {
        match self {
            Select::First(selection) | Select::Done(selection, _) | Select::OnAttack(selection) => {
                selection
            }
        }
    }
}

/// Act on a pick by the cursor's classification of it: a GameObject, a corpse, an attack, loot,
/// skin, or the `UNIT_NPC_FLAGS` ladder ([`service_arm`]); only the click selects first
/// (`0x493540`), and the attack leg selects through StartAttack. The unit legs measure their own
/// reach ([`Dispatch::in_service_reach`]); a GameObject's and a corpse's still ride the range gray
/// the press latched (`unable`). Out of reach every send but attack is suppressed, the server
/// holds attack until in reach; with Click to Move on, each dispatcher walks there instead
/// (`CanAutoInteract` is their approach flag, `0x60c170`, `0x5d6c6f`).
fn interact(
    dispatch: &mut Dispatch,
    script: Option<&mut benilla_ui::script::UiScript>,
    press: &PressPick,
    mut select: Select,
) {
    let (hovered, hovered_object, cursor) = (&press.hovered, &press.object, &press.cursor);
    let walk = dispatch.auto.can_auto_interact();
    let self_mounted = dispatch.mounted();
    // A GameObject is used, never falling through to units: `OnUse 0x5f8660` gates on
    // highlightable (our `Point` cursor, silent), then usable (`0x5f3130`), whose lock arm toasts
    // first, even out of range.
    if go_is_nearest(hovered, hovered_object) {
        // Traced (tag `use`): a `Point` cursor refuses silently; the range gray raises
        // `ERR_USE_TOO_FAR`.
        if benilla_assets::trace::enabled_for("use") {
            let ty = hovered_object
                .target
                .and_then(|e| dispatch.stores.get(e).ok())
                .map_or(-1, |(s, _)| s.0.gameobject_type_id());
            benilla_assets::trace::line(
                "use",
                &format!(
                    "right-click go guid={:?} type={ty} mounted={self_mounted} cursor={:?} unable={}",
                    hovered_object.guid.map(|g| format!("{g:#x}")),
                    cursor.kind,
                    cursor.unable
                ),
            );
        }
        if cursor.kind != cursor_mode::CursorKind::Point {
            if let (Some(entity), Some(guid)) = (hovered_object.target, hovered_object.guid) {
                // Walking, the range arm (`0x5f330c`) is the object's own: the PickLock cursor
                // never grays, and the gray folds the lock in.
                let in_reach = if walk {
                    let go_type = dispatch
                        .stores
                        .get(entity)
                        .map_or(-1, |(s, _)| s.0.gameobject_type_id());
                    dispatch
                        .dist_sq(entity)
                        .is_some_and(|d2| d2 <= cursor_mode::go_interact_range_sq(go_type))
                } else {
                    !cursor.unable
                };
                dispatch.use_gameobject(entity, guid, in_reach, walk);
            }
        }
        return;
    }
    // ── The corpse leg, `CGCorpse_C`'s interact slot `0x5d6bf0` ──
    // Leg 1: not mounted and lootable (`CORPSE_FIELD_DYNAMIC_FLAGS` bit 0) → a stand-state check,
    // `SetAutoLoot` (`0x5df460`) and `CMSG_LOOT` (`0x5df130`). Leg 2: `CORPSE_FIELD_FLAGS` bit 5,
    // the `[0xb700e8]` latch (never set in 1.12.1) and an unfriendly corpse (`!0x6067d0`) → the
    // skin cast (`0x5f05e0`). Your own corpse takes neither: the resurrect prompt comes from the
    // 40 yd `CORPSE_IN_RANGE` poll (`0x492130`), never a click.
    if let (Some(entity), Some(guid)) = (hovered.corpse, hovered.corpse_guid) {
        let store = dispatch.stores.get(entity).ok().map(|(s, _)| s);
        if benilla_assets::trace::enabled_for("use") {
            benilla_assets::trace::line(
                "use",
                &format!(
                    "right-click corpse guid={guid:#x} bones={} lootable={} insignia={} mounted={self_mounted} cursor={:?} unable={}",
                    store.is_some_and(|s| s.0.corpse_is_bones()),
                    store.is_some_and(|s| s.0.corpse_lootable()),
                    store.is_some_and(|s| s.0.corpse_pvp_insignia()),
                    cursor.kind,
                    cursor.unable
                ),
            );
        }
        let (lootable, insignia) = (
            store.is_some_and(|s| s.0.corpse_lootable()),
            store.is_some_and(|s| s.0.corpse_pvp_insignia()),
        );
        // A rider fails leg 1 (`0x5d6c2a jg`) with no error and falls to leg 2, silently.
        if !self_mounted && lootable {
            // Not standing: the client-local red `ERR_LOOT_NOTSTANDING`, no packet (`0x5d6c3b` →
            // `GetStandState 0x5ed570`, non-zero → `0x496720(0x85)`).
            if dispatch
                .self_store()
                .is_some_and(|s| s.0.unit_stand_state() != 0)
            {
                debug!("right-click corpse loot: refused, not standing ({guid:#x})");
                dispatch
                    .feedback
                    .errors
                    .push(crate::ui_action::UiError::key("ERR_LOOT_NOTSTANDING"));
                return;
            }
            // Range rides the cursor's gray, so the pouch is never lit where the click refuses.
            if !cursor.unable {
                dispatch.loot_corpse(entity, guid, walk);
            }
        } else if insignia {
            // Leg 2. `skin_player_corpse` mirrors `[0xb700e8]`, `None` for every 1.12.1 player,
            // so this is inert as in the reference; the unfriendly test is not built.
            if let Some(spell_id) = dispatch.learned.skin_player_corpse {
                dispatch.skin(
                    entity,
                    guid,
                    spell_id,
                    crate::player::Subject::Corpse,
                    !cursor.unable,
                    walk,
                );
            }
        }
        return;
    }
    let (Some(entity), Some(guid)) = (hovered.target, hovered.guid) else {
        return;
    };
    // The dispatcher's attack arm (`0x60c18c`), not the sword: a dead, ghost or mounted player
    // still takes it and is refused inside (`0x60c1a1`, `0x60c1bc`), never reaching a service.
    let attack = press.attack_fork.0;
    let target = dispatch.stores.get(entity).ok().map(|(s, _)| s);
    // ── The dead-target fork of the unit dispatcher `0x60bea0` ──
    // Loot routes by classification (dead and `UNIT_DYNFLAG_LOOTABLE`), not the cursor kind, whose
    // Pickup(8) a live vendor shares. A rider skips the loot leg for the skin leg (`0x60bf98`),
    // silently. Step 0 of [`DeadUnitLeg`] is hoisted for the trace.
    let dead_fork = target.is_some_and(|s| s.0.unit_is_dead() && !s.0.unit_dynflag_dead());
    let leg = dead_unit_leg(
        self_mounted,
        dead_fork,
        target.is_some_and(|s| s.0.unit_lootable()),
        target.is_some_and(|s| s.0.unit_flags() & cursor_mode::UNIT_FLAG_SKINNABLE != 0),
        dispatch.learned.skinning.is_some(),
    );
    if benilla_assets::trace::enabled_for("use") {
        benilla_assets::trace::line(
            "use",
            &format!(
                "right-click unit guid={guid:#x} dead={} lootable={} skinnable={} mounted={self_mounted} leg={} cursor={:?} unable={}",
                target.is_some_and(|s| s.0.unit_is_dead()),
                target.is_some_and(|s| s.0.unit_lootable()),
                target.is_some_and(|s| s.0.unit_flags() & cursor_mode::UNIT_FLAG_SKINNABLE != 0),
                if attack {
                    "attack"
                } else if dead_fork {
                    leg.tag()
                } else {
                    // The alive branch (`0x60c162`), not mounted-gated: a rider talks to a
                    // flight master.
                    "service"
                },
                cursor.kind,
                cursor.unable
            ),
        );
    }
    let me = dispatch.me();
    // A mid-combat click on a vendor or corpse switches and stops, never swings (`0x5ecb70`); the
    // sword, not the fork, as the re-swing also needs the player's own legs.
    // Deref'd only where it commits: a leg that does not select leaves the change tick alone.
    let commit = |seam: &mut crate::creature_anim::AttackSeam,
                  front: &mut super::front::SelectFront,
                  selection: &mut ResMut<Selection>| {
        // Both selects are `SelectTarget`'s (`0x492870`, `0x5eccaa`): charmed, or a standing
        // cursor's bind in its place.
        if front.takes(entity, guid, seam.slots.targeting()) {
            return scan::CommitOutcome::default();
        }
        scan::commit(
            selection,
            seam,
            entity,
            guid,
            target,
            me.is_some_and(|(_, _, e)| e),
            me.map(|(_, g, _)| g),
            press.attack(),
        )
    };
    let mut outcome = match &mut select {
        Select::First(selection) => commit(&mut dispatch.seam, &mut dispatch.front, selection),
        Select::Done(_, done) => *done,
        Select::OnAttack(_) => scan::CommitOutcome::default(),
    };
    match unit_branch(attack, dead_fork, leg) {
        UnitBranch::Attack => {
            // `0x60c22b`, past the dead and mounted gates (`0x60c1a1`, `0x60c1bc`) and before
            // StartAttack, so a refused swing still walks.
            let refused = dispatch
                .self_store()
                .is_some_and(|s| s.0.is_dead_or_ghost() || s.0.unit_mount_display_id() > 0);
            match dispatch.melee_approach(entity).filter(|_| !refused) {
                Some(MeleeApproach::Face(at)) => {
                    dispatch.auto.face(at, &mut dispatch.seam.stand);
                }
                Some(MeleeApproach::Walk { at, stop }) => {
                    let started = dispatch
                        .auto
                        .walk_into_melee(at, stop, &mut dispatch.seam.stand);
                    dispatch.feedback.walked(started);
                }
                None => {}
            }
            // Silent after the select: the click's `0x60c247 call 0x5ecb70` has no `DisplayError`,
            // and the red `ERR_ATTACK_*` lines are `0x612df0`'s (the Attack action, pet attack,
            // TryCast), which this path never reaches. The arm's own returns are the dead and
            // mounted player (`0x60c1a1`, `0x60c1bc`), and StartAttack's the pacified flag and a
            // clear `[0xb4b3e4]` (`0x5ecbaa`, `0x5ecbc0`).
            let gate = dispatch.seam.start_gate();
            if refused || gate.refuses() {
                // refused: the selection stands, no swing, nothing said
            } else {
                // StartAttack's own select (`0x5eccaa`), past its gates and before the swing
                // (`0x5eccfd`): the click's commit, with its stop and re-swing mid-combat. A
                // unit already selected is its dedupe, and a cursor's bind has emptied the word
                // by now, so a plate's unit is selected here, ahead of the swing.
                let second = commit(&mut dispatch.seam, &mut dispatch.front, select.selection());
                outcome = scan::CommitOutcome {
                    changed: outcome.changed || second.changed,
                    swung: outcome.swung || second.swung,
                };
                debug!("right-click attack: {guid:#x}");
                // `0x5ecb70`'s body through the seam; `swung` means the commit already asked it
                // for its re-swing, which `0x5eccda` keeps from sending twice.
                let engaged = me.is_some_and(|(_, _, e)| e);
                dispatch.seam.start(guid, engaged || outcome.swung, false);
            }
        }
        UnitBranch::Dead(DeadUnitLeg::Loot) => {
            // The stand-state check (`0x60bfb7` → `0x60c007 push 0x85`) matches the corpse leg's.
            if dispatch
                .self_store()
                .is_some_and(|s| s.0.unit_stand_state() != 0)
            {
                debug!("right-click loot: refused, not standing ({guid:#x})");
                dispatch
                    .feedback
                    .errors
                    .push(crate::ui_action::UiError::key("ERR_LOOT_NOTSTANDING"));
            } else {
                let can_loot = dispatch.can_loot_unit(entity);
                dispatch.loot_unit(entity, guid, can_loot, walk);
            }
        }
        UnitBranch::Dead(DeadUnitLeg::Skin) => {
            // The skin leg (`0x60c01f`): the known Skinning spell (`[0xb700e4]`) at a corpse the
            // loot leg declined, lootable ones included while mounted.
            if let Some(spell_id) = dispatch.learned.skinning {
                let in_reach = dispatch.in_melee_reach(entity);
                dispatch.skin(
                    entity,
                    guid,
                    spell_id,
                    crate::player::Subject::Unit { dead: true },
                    in_reach,
                    walk,
                );
            }
        }
        // Terminal: a dead unit that took no leg does nothing; only the alive branch (`0x60c162`)
        // reaches the service dispatch.
        UnitBranch::Dead(DeadUnitLeg::Nothing) => {
            debug!("right-click unit {guid:#x}: dead fork took no leg — nothing sent");
        }
        UnitBranch::Service => dispatch.talk(script, entity, guid, walk),
    }
}

/// Interact with an object by running its own virtual dispatcher (slot `+0x60`): `0x60bea0` for a
/// unit or player, `0x5d6bf0` for a corpse, `0x5f8660` for a GameObject. It skips the world click's
/// select-first entry (`0x492820`); the attack leg selects through StartAttack (`0x5ecb70`), and
/// the service, loot, skin, corpse and GameObject legs select nothing.
#[derive(Message, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Interact(pub Entity);

/// Each nameplate right-click's unit interact (`0x492881`), then each [`Interact`], its object
/// classified as a press over it would be. Runs after the bind of a standing cursor's parked picks
/// ([`crate::script_calls::bind_parked_picks`]): a plate's `SelectTarget` gave its unit to the
/// cast, and the interact that follows finds the cursor's word empty, so its attack leg selects
/// the unit and its service leg talks, after the cast on the wire.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn act_on_interact(
    mut requests: MessageReader<Interact>,
    mut plate_clicks: MessageReader<PlateRightClick>,
    press: Res<PressPick>,
    kinds: Query<(&Guid, &crate::net::NetEntity)>,
    // The classifier, the dispatchers and the selection, which the host's calls also reach: an
    // engine turn.
    mut vm: crate::script_calls::VmWith<(
        ParamSet<
            'static,
            'static,
            (
                cursor_mode::CursorInputs<'static, 'static>,
                Dispatch<'static, 'static>,
            ),
        >,
        ResMut<'static, Selection>,
    )>,
) {
    use benilla_protocol::EntityKind;
    if requests.is_empty() && plate_clicks.is_empty() {
        return;
    }
    vm.turn(|mut script, (mut set, mut selection)| {
        for &PlateRightClick { selected } in plate_clicks.read() {
            interact(
                &mut set.p1(),
                script.as_deref_mut(),
                &press,
                Select::Done(&mut selection, selected),
            );
        }
        for &Interact(entity) in requests.read() {
            let Ok((guid, net)) = kinds.get(entity) else {
                continue;
            };
            let mut pick = PressPick::default();
            match net.kind {
                EntityKind::Unit | EntityKind::Player => {
                    (pick.hovered.target, pick.hovered.guid) = (Some(entity), Some(guid.0));
                }
                EntityKind::Corpse => {
                    (pick.hovered.corpse, pick.hovered.corpse_guid) = (Some(entity), Some(guid.0));
                }
                EntityKind::GameObject => {
                    (pick.object.target, pick.object.guid) = (Some(entity), Some(guid.0));
                }
                EntityKind::DynamicObject | EntityKind::Other => continue,
            }
            (pick.cursor, pick.attack_fork) =
                cursor_mode::classify(&set.p0(), &pick.hovered, &pick.object);
            interact(
                &mut set.p1(),
                script.as_deref_mut(),
                &pick,
                Select::OnAttack(&mut selection),
            );
        }
    })
}

/// The verb an approach owed, once it arrived (`0x60fa20`, run at the stop): the dispatcher alone,
/// with no select, no cursor and no walk but Use's. Each looks its object up by type (`0x468460`),
/// and the loot's asks for a unit (`0x60fa41`), so a corpse object walked to is not looted.
pub(super) fn act_on_arrival(
    // The dispatchers, which the host's calls also reach: an engine turn.
    mut vm: crate::script_calls::VmWith<Dispatch<'static, 'static>>,
    index: Res<crate::net::GuidIndex>,
    kinds: Query<&crate::net::NetEntity>,
) {
    vm.turn(|script, mut dispatch| act_on_arrival_with(&mut dispatch, script, &index, &kinds));
}

fn act_on_arrival_with(
    dispatch: &mut Dispatch,
    script: Option<&mut benilla_ui::script::UiScript>,
    index: &crate::net::GuidIndex,
    kinds: &Query<&crate::net::NetEntity>,
) {
    use crate::player::{ApproachVerb, Subject};
    use benilla_protocol::EntityKind;
    // Owed while any `0x20ff` bit is set: the reference drains it from the movement emitter
    // `0x60e0a0` only at a stop, strafe stop, root or unroot ack that leaves none (`0x60e352`), never
    // at a landing. Deviation: it runs on the first frame with none, so an arrival that sends no stop
    // (mid-air, rooted, under a held turn key) acts at once or on landing or release, where the
    // reference waits for the next ground stop and can act long after, wherever the player then is.
    if dispatch.moving() {
        return;
    }
    let Some((verb, guid)) = dispatch.auto.approach.arrived.take() else {
        return;
    };
    let Some(&entity) = index.0.get(&guid) else {
        return;
    };
    let kind = kinds.get(entity).ok().map(|k| k.kind);
    let unit = matches!(kind, Some(EntityKind::Unit | EntityKind::Player));
    let d2 = dispatch.dist_sq(entity).unwrap_or(f32::INFINITY);
    debug!(
        "approach: arrived for {verb:?} at {guid:#x}, {:.2} yd",
        d2.sqrt()
    );
    match verb {
        ApproachVerb::Talk if unit => dispatch.talk(script, entity, guid, false),
        // `0x5df2a0`'s own lootable test (`0x6003a0`), which the click's dead fork made earlier.
        ApproachVerb::Loot if unit => {
            if dispatch
                .stores
                .get(entity)
                .is_ok_and(|(s, _)| s.0.unit_lootable())
            {
                let can_loot = dispatch.can_loot_unit(entity);
                dispatch.loot_unit(entity, guid, can_loot, false);
            }
        }
        ApproachVerb::Use if kind == Some(EntityKind::GameObject) => {
            let go_type = dispatch
                .stores
                .get(entity)
                .map_or(-1, |(s, _)| s.0.gameobject_type_id());
            // Still out of range, `0x5f86b0`'s range arm walks again (`0x610300`).
            let walk = dispatch.auto.can_auto_interact();
            dispatch.use_gameobject(
                entity,
                guid,
                d2 <= cursor_mode::go_interact_range_sq(go_type),
                walk,
            );
        }
        // The spell by the target: a corpse or a player's body takes `[0xb700e8]` (`0x5f0608`).
        ApproachVerb::Skin => {
            let spell = if matches!(kind, Some(EntityKind::Corpse | EntityKind::Player)) {
                dispatch.learned.skin_player_corpse
            } else {
                dispatch.learned.skinning
            };
            if let Some(spell_id) = spell {
                let subject = if kind == Some(EntityKind::Corpse) {
                    Subject::Corpse
                } else {
                    Subject::Unit { dead: true }
                };
                dispatch.skin(entity, guid, spell_id, subject, true, false);
            }
        }
        _ => {}
    }
}

/// The right-click action for a hovered GameObject, chosen by its lock ([`super::lock`]).
pub(crate) enum GoAction {
    /// No lock, or no lock data: `CMSG_GAMEOBJ_USE`.
    Use,
    /// A lock a known skill spell opens (`OPEN_LOCK`): cast it at the object.
    OpenLock(u32),
    /// A satisfied key slot: use the key at the object, `CMSG_USE_ITEM` targeting it (the cast
    /// sender `0x6e54f0` takes its item arm), since the server honours a key slot only with
    /// `m_CastItem` set (`Spell.cpp:7892`). `on_object` is the lock's guid.
    OpenByKey(crate::ui_items::ItemUse),
    /// An unopenable lock: the client-local refusal, `None` where the reference is silent too.
    Refuse(Option<crate::ui_action::UiError>),
}

/// A key-item slot's key, when routing a refusal ([`route_lock_refusal`]).
enum KeyFact {
    /// Not held, and the template names it ("Requires Shadowforge Key").
    Named(String),
    /// Not held and uncached: silent, like the reference's record miss (`0x5f3562`).
    Unknown,
}

/// A hovered GameObject's action from its lock, the reference's use sender `0x5f33e0` over the
/// resolver [`super::lock::resolve_lock`] (`0x5f83d0`) the cursor's `usable` also asks. An uncached
/// template or no `Lock.dbc` counts as lockless.
pub(crate) fn resolve_go_action(
    guid: u64,
    inputs: &mut GoLockInputs,
    known: &std::collections::BTreeSet<u32>,
    go: Option<(&ObjectStore, u32)>,
    me_store: Option<&ObjectStore>,
    net: &NetCommands,
) -> GoAction {
    let Some(tmpl) = inputs.templates.of_guid(&inputs.objects, guid) else {
        return GoAction::Use;
    };
    let Some(locks) = inputs.locks.as_deref() else {
        return GoAction::Use;
    };
    // A lockId with no row is no lock (`0x5f8180` returns null).
    let Some(slots) = locks.0.slots(tmpl.lock_id).filter(|_| tmpl.lock_id != 0) else {
        return GoAction::Use;
    };
    let facts = super::lock::go_facts(go);
    let mut matched = None;
    let outcome = super::lock::resolve_lock(
        slots,
        known,
        inputs.spells.as_deref(),
        inputs.skill_lines.as_ref().map(|s| &s.catalog),
        me_store,
        &inputs.objects,
        facts,
        &mut matched,
    );
    let key_entry = match outcome {
        super::lock::LockOutcome::Unlocked => return GoAction::Use,
        super::lock::LockOutcome::OpenBySpell(spell_id) => {
            debug!("target: lock {} → open by spell {spell_id}", tmpl.lock_id);
            return GoAction::OpenLock(spell_id);
        }
        super::lock::LockOutcome::OpenByKey(entry) => entry,
        super::lock::LockOutcome::Unmet => {
            // The toast routing keys off Lock.dbc slot 0, whichever slot the resolver walked.
            let slot0 = slots[0];
            let key = if slot0.key_type == benilla_formats::LOCK_KEY_ITEM {
                match inputs.items.template(slot0.index, 0, Ask::Event(net)) {
                    Some(info) => KeyFact::Named(info.name.clone()),
                    None => KeyFact::Unknown,
                }
            } else {
                KeyFact::Unknown
            };
            let lock_types = inputs.lock_types.as_deref();
            return GoAction::Refuse(route_lock_refusal(
                &slot0,
                matched.is_some(),
                facts.flag_locked,
                go.map_or(-1, |(s, _)| s.0.gameobject_type_id()),
                facts.level,
                lock_types.and_then(|lt| lt.0.name(slot0.index)),
                key,
            ));
        }
    };
    // A key we carry is used at the object: the sender `0x6e54f0` takes its item arm
    // (`0x6e57d8 push 0xab`), `CMSG_USE_ITEM {u8 bag, u8 slot, u8 spellSlot, targets}` with no
    // spell id, so the wire needs the key's position.
    let Some(store) = me_store else {
        return GoAction::Refuse(None);
    };
    let Some((bag_index, slot, key_guid)) = crate::ui_items::find_item(
        &store.0,
        &inputs.objects,
        key_entry,
        crate::ui_items::ItemSearch::default(),
    ) else {
        // Held when the resolver ran, gone now: nothing to send.
        return GoAction::Refuse(None);
    };
    // An uncached template queries and does nothing this click. `use_spell_index` is the spell
    // block ordinal, the packet's third byte.
    let Some(tmpl) = inputs.items.template(key_entry, 0, Ask::Event(net)) else {
        return GoAction::Refuse(None);
    };
    let Some(spell_index) = tmpl.use_spell_index() else {
        return GoAction::Refuse(None);
    };
    GoAction::OpenByKey(crate::ui_items::ItemUse {
        guid: Some(key_guid),
        start_quest: tmpl.start_quest,
        bag_index,
        slot,
        entry: key_entry,
        spell_index,
        use_spell: tmpl.use_spell.as_ref().map(|u| u.spell_id),
        on_object: Some(guid),
        is_charter: tmpl.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
    })
}

/// The client-local toast for an unopenable lock (`0x5f3427..`): `GO_FLAG_LOCKED` takes the
/// strategy default first; otherwise Lock.dbc slot 0 picks `0xde` (key item), `0xdf` (skill
/// unknown), `0xe0` (under rank: `Skill[0]`, else GO level × 5) or `0xda`. `"UNKNOWN"` is the
/// reference's missing-LockType fallback (`0x838044`). The chest-in-use check (`0xd9`,
/// `0x5f81d0`) is not built; `GO_FLAG_IN_USE` already fails the highlightable gate.
fn route_lock_refusal(
    slot0: &benilla_formats::LockSlot,
    opener_known: bool,
    flag_locked: bool,
    go_type: i32,
    go_level: u32,
    lock_type_name: Option<&str>,
    key: KeyFact,
) -> Option<crate::ui_action::UiError> {
    use crate::ui_action::{FillArg, UiError};
    if flag_locked {
        return Some(UiError::key(match go_type {
            0 => "ERR_DOOR_LOCKED",
            1 => "ERR_BUTTON_LOCKED",
            _ => "ERR_USE_LOCKED",
        }));
    }
    match slot0.key_type {
        benilla_formats::LOCK_KEY_ITEM => match key {
            KeyFact::Unknown => None,
            KeyFact::Named(name) => Some(UiError::s("ERR_USE_LOCKED_WITH_ITEM_S", name)),
        },
        benilla_formats::LOCK_KEY_SKILL => {
            let name = lock_type_name.unwrap_or("UNKNOWN").to_string();
            if opener_known {
                let required = super::lock::required_skill(slot0, go_level).max(0) as u32;
                // String-then-Integer, the template's own order.
                Some(UiError::args(
                    "ERR_USE_LOCKED_WITH_SPELL_KNOWN_SI",
                    vec![FillArg::S(name), FillArg::D(i64::from(required))],
                ))
            } else {
                Some(UiError::s("ERR_USE_LOCKED_WITH_SPELL_S", name))
            }
        }
        _ => Some(UiError::key("ERR_USE_CANT_OPEN")),
    }
}

/// The dead-target fork (`0x60bf75`) of the unit dispatcher `0x60bea0`, in the reference's order:
///
/// 0. a corpse: `HEALTH <= 0` and the feign-death bit `UNIT_DYNFLAG_DEAD` clear, else alive;
/// 1. the player mounted (`0x60bf98`): straight to the skin leg, silently;
/// 2. lootable (`0x6003a0`): [`DeadUnitLeg::Loot`];
/// 3. skinnable (`UNIT_FIELD_FLAGS` bit 26) with Skinning known (`[0xb700e4]`): the skin leg;
/// 4. otherwise nothing (`0x60c25f`).
///
/// Step 1 never reads the target: a lootable and skinnable corpse skins mounted, loots on foot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DeadUnitLeg {
    /// `CMSG_LOOT` (`0x60bff7` → `0x5df2a0`).
    Loot,
    /// The skin cast (`0x60c082` → `0x5f05e0`).
    Skin,
    /// The dispatcher returns: no packet, no message, no state write.
    Nothing,
}

impl DeadUnitLeg {
    /// The one-word tag the `use` trace prints.
    fn tag(self) -> &'static str {
        match self {
            Self::Loot => "loot",
            Self::Skin => "skin",
            Self::Nothing => "none",
        }
    }
}

fn dead_unit_leg(
    mounted: bool,
    dead: bool,
    lootable: bool,
    skinnable: bool,
    know_skinning: bool,
) -> DeadUnitLeg {
    if !dead {
        return DeadUnitLeg::Nothing;
    }
    if !mounted && lootable {
        return DeadUnitLeg::Loot;
    }
    if skinnable && know_skinning {
        return DeadUnitLeg::Skin;
    }
    DeadUnitLeg::Nothing
}
/// The branch of `0x60bea0` a unit right-click takes: the reference splits on death first and
/// never rejoins, so only the alive side (`0x60c162`) reaches the service send.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UnitBranch {
    /// The Attack cursor's leg, `0x60c247 call 0x5ecb70`: select, then swing.
    Attack,
    /// The dead fork (`0x60bf75`) and its chosen leg; terminal in every variant.
    Dead(DeadUnitLeg),
    /// The alive branch (`0x60c162`): `CanInteract`, then the NPC-service packet.
    Service,
}

/// [`UnitBranch`] from its two facts; `attack` first, which only a live hostile classifies.
fn unit_branch(attack: bool, dead_fork: bool, leg: DeadUnitLeg) -> UnitBranch {
    if attack {
        UnitBranch::Attack
    } else if dead_fork {
        UnitBranch::Dead(leg)
    } else {
        UnitBranch::Service
    }
}

/// The reference's NPC-service ladder, `0x5f0130`'s first-match-wins walk over `UNIT_NPC_FLAGS`,
/// low bit to high. The cursor classifier `0x482200` walks the same bits but projects them lossily
/// (Speak is bits 0, 1, 5, 6, 9, 10, 11 and 13; Buy is 8 and 12), so the send keys on the bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ServiceArm {
    /// Bit 0: `0x5f02a4` → `0x5df4d0`.
    Gossip,
    /// Bit 1, and the target's cached questgiver status ∉ {0, 1} (`0x5f02c0` → `0x5df490`).
    Questgiver,
    /// Bit 2: `0x5f0317` → `0x5df5d0`.
    Vendor,
    /// Bit 3: `0x5f034e` → `0x5ed020`.
    FlightMaster,
    /// Bit 4: `0x5f0385` → `0x5df680`.
    Trainer,
    /// Bit 5: `0x5f03bc` → `0x5df730`. Ghost-gated, and sends nothing.
    SpiritHealer,
    /// Bit 6: `0x5f03f3` → `0x5df950`. Ghost-gated.
    SpiritGuide,
    /// Bit 7: `0x5f042a` → `0x5dfdc0`. Sends nothing.
    Innkeeper,
    /// Bit 8: `0x5f0461` → `0x5dffe0`.
    Banker,
    /// Bit 9: `0x5f04e3` → `0x5e0060`.
    Petitioner,
    /// Bit 10: `0x5f051a` → `0x5e00e0`.
    TabardDesigner,
    /// Bit 11: `0x5f0551` → `0x5e01a0`.
    Battlemaster,
    /// Bit 12: `0x5f0588` → `0x5e0220`.
    Auctioneer,
    /// Bit 13: `0x5f05bc` → `0x5e02a0`.
    StableMaster,
}

/// The ladder; `None` when no consulted bit is set (`0x5f05ca`). REPAIR (bit 14) has no arm, and
/// the reference's redundant `bits 9 AND 10` test reaches bit 9's handler, so it is omitted.
pub(crate) fn service_arm(npc_flags: u32, quest_status: Option<u32>) -> Option<ServiceArm> {
    use cursor_mode::npc_flags as f;
    let bit = |m: u32| npc_flags & m != 0;
    Some(if bit(f::GOSSIP) {
        ServiceArm::Gossip
    } else if bit(f::QUESTGIVER) && cursor_mode::questgiver_has_quest(quest_status) {
        // The cursor ladder's own predicate (`0x482362`), shared so the two cannot disagree.
        ServiceArm::Questgiver
    } else if bit(f::VENDOR) {
        ServiceArm::Vendor
    } else if bit(f::FLIGHTMASTER) {
        ServiceArm::FlightMaster
    } else if bit(f::TRAINER) {
        ServiceArm::Trainer
    } else if bit(f::SPIRITHEALER) {
        ServiceArm::SpiritHealer
    } else if bit(f::SPIRITGUIDE) {
        ServiceArm::SpiritGuide
    } else if bit(f::INNKEEPER) {
        ServiceArm::Innkeeper
    } else if bit(f::BANKER) {
        ServiceArm::Banker
    } else if bit(f::PETITIONER) {
        ServiceArm::Petitioner
    } else if bit(f::TABARDDESIGNER) {
        ServiceArm::TabardDesigner
    } else if bit(f::BATTLEMASTER) {
        ServiceArm::Battlemaster
    } else if bit(f::AUCTIONEER) {
        ServiceArm::Auctioneer
    } else if bit(f::STABLEMASTER) {
        ServiceArm::StableMaster
    } else {
        return None;
    })
}

/// What a taken [`ServiceArm`] does.
pub(crate) enum ServiceAction {
    /// The arm's own opcode.
    Send(ClientCommand),
    /// The vendor arm's cursor fork (`0x5df5d7`): sell the held item, then clear the cursor.
    SellFromCursor(ClientCommand),
    /// Raise `CONFIRM_BINDER` locally and send nothing (`0x5dfdc0`).
    AskBinder,
    /// Raise `CONFIRM_XP_LOSS` locally and send nothing (`0x5df730`).
    AskSpiritHealer,
    /// Adopt this guide as the area's spirit healer; `0x5df950` → `0x4921c0` sends the query.
    AcquireSpiritGuide,
    /// Nothing goes out; the payload says why, for the debug line.
    Silent(&'static str),
}

/// A taken arm → what benilla does. `cursor_sale` is the guid of a mode-1 item on the cursor, as
/// the reference's `GetCursorItem 0x494c60`. The spirit arms are ghost-gated (`0x5df74a`,
/// `0x5df962`: `PLAYER_FLAGS` bit 4), silent for the living. The tabard arm's shapeshift refusal
/// (`ERR_EMBLEMERROR_NOTABARDGEOSET`) and same-vendor no-op are not built.
pub(crate) fn service_action(
    arm: ServiceArm,
    guid: u64,
    ghost: bool,
    cursor_sale: Option<u64>,
) -> ServiceAction {
    match arm {
        ServiceArm::Gossip => ServiceAction::Send(ClientCommand::GossipHello { guid }),
        ServiceArm::Questgiver => ServiceAction::Send(ClientCommand::QuestgiverHello { npc: guid }),
        // A held item sells (`CMSG_SELL_ITEM 0x1a0`) instead of listing, with no merchant-window
        // test; the re-click gate stops it while this vendor's window is open.
        ServiceArm::Vendor => match cursor_sale {
            Some(item_guid) => ServiceAction::SellFromCursor(ClientCommand::SellItem {
                vendor: guid,
                item_guid,
                // Always 0 (`xor ecx,ecx` at `0x5df5ee`): the whole stack
                // (`ItemHandler.cpp:495`).
                count: 0,
            }),
            None => ServiceAction::Send(ClientCommand::ListInventory { guid }),
        },
        ServiceArm::FlightMaster => ServiceAction::Send(ClientCommand::TaxiQueryNodes { guid }),
        ServiceArm::Trainer => ServiceAction::Send(ClientCommand::TrainerList { trainer: guid }),
        ServiceArm::SpiritHealer if ghost => ServiceAction::AskSpiritHealer,
        ServiceArm::SpiritHealer => ServiceAction::Silent("spirit healer, and we are alive"),
        // The query goes through the proximity poll's routine after a `(0,0)` cache bust, so a
        // click beside the current guide still re-asks for the wave.
        ServiceArm::SpiritGuide if ghost => ServiceAction::AcquireSpiritGuide,
        ServiceArm::SpiritGuide => ServiceAction::Silent("spirit guide, and we are alive"),
        ServiceArm::Innkeeper => ServiceAction::AskBinder,
        ServiceArm::Banker => ServiceAction::Send(ClientCommand::BankerActivate { guid }),
        ServiceArm::Petitioner => {
            ServiceAction::Send(ClientCommand::PetitionShowList { npc: guid })
        }
        ServiceArm::TabardDesigner => {
            ServiceAction::Send(ClientCommand::TabardVendorActivate { npc: guid })
        }
        ServiceArm::Battlemaster => {
            ServiceAction::Send(ClientCommand::BattlemasterHello { npc: guid })
        }
        ServiceArm::Auctioneer => {
            ServiceAction::Send(ClientCommand::AuctionHello { auctioneer: guid })
        }
        ServiceArm::StableMaster => {
            ServiceAction::Send(ClientCommand::ListStabledPets { npc: guid })
        }
    }
}

/// Drain the guid-scoped deselects ([`DeselectGuid`]). `ClearTarget()`, the last leg of the Esc
/// chain (`UIParent.lua:1492`), is a script call and lands in call order
/// ([`crate::script_calls`]).
pub(crate) fn clear_target_requests(
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
    mut guid_asks: MessageReader<DeselectGuid>,
) {
    if guid_asks.read().any(|ask| selection.guid == Some(ask.0)) {
        clear(&mut selection, &mut seam, !engaged.is_empty());
    }
}

/// The UI shutdown's own deselect (`0x490bdf` → `SetSelection(0, 0)`), ahead of its
/// active-player guard, so every shutdown root and a `/reload` drop the target: `true` when one
/// was selected.
pub(crate) fn deselect_on_ui_shutdown(
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
    me: Query<&crate::net::ObjectStore, With<SelfPlayer>>,
) -> bool {
    // A charmed player's `SetSelection` returns at once (`0x493589`-`0x49359c`).
    if me
        .iter()
        .next()
        .is_some_and(|s| s.0.unit_charmed_by().is_some())
    {
        return false;
    }
    let had = selection.target.is_some();
    deselect(&mut selection, &mut seam, !engaged.is_empty());
    had
}

/// The UI shutdown's `SetSelection(0, 0)` after the world's teardown, with no object manager:
/// the selection is stamped as the last target (`0x493622`) and cleared (`0x4939d3`), and
/// `0x4939df` returns before the swing stop, the send and the event.
pub(crate) fn clear_selection_on_world_teardown(mut selection: ResMut<Selection>) {
    if selection.guid.is_some() {
        selection.last = selection.guid;
    }
    selection.target = None;
    selection.guid = None;
}

/// A deselect that applies only if the selection is this guid (`0x493910(guid, 1)`), raised by
/// every loot close for a dead unit (`0x48f369`) and drained by [`clear_target_requests`].
#[derive(bevy::ecs::message::Message, Clone, Copy, Debug)]
pub(crate) struct DeselectGuid(pub(crate) u64);

/// `SetSelection(0,0)` (`0x493540` → `0x4938f3`): the deselect of Esc, a click off and a target's
/// death, which stamps the outgoing target into the last-target pair before [`clear`]. A no-op
/// with nothing selected, the setter's dedup.
pub(super) fn deselect(
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    engaged: bool,
) {
    // Charmed returns before the guid test (`0x49359c`), so a deselect is refused too.
    if seam.player_is_charmed() {
        return;
    }
    if selection.guid.is_some() {
        selection.last = selection.guid;
    }
    clear(selection, seam, engaged);
}

/// Drop the target and send `CMSG_SET_SELECTION` 0 (a no-op with none): the teardown `0x493910`
/// with its send, which a despawn and a dead unit's loot close reach without `SetSelection`, so the
/// last-target pair is left. When `engaged`, melee stops too. Weapons stay drawn.
pub(super) fn clear(
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    engaged: bool,
) {
    if selection.target.take().is_some() {
        if let Some(old) = selection.guid.take() {
            // The teardown `0x493910` closes the old target's loot before its own send.
            seam.close_loot_on(old);
        }
        let _ = seam.net.0.send(ClientCommand::SetSelection { guid: 0 });
        // `SetSelection 0x493540`'s own `0x493a08 call 0x5ecac0`, the real StopAttack, which
        // also un-queues a pending on-next-swing strike.
        seam.stop(engaged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `0x5f3427..` toast routing on real data: Peacebloom (lock 29: skill slot, LockType 2,
    /// Skill 0), a rank-155 vein (lock 42: LockType 3), a keyed door, a padlocked chest.
    #[test]
    fn lock_refusals_route_like_the_reference() {
        use benilla_formats::{LockSlot, LOCK_KEY_ITEM, LOCK_KEY_SKILL};
        let skill_slot = |index, skill| LockSlot {
            key_type: LOCK_KEY_SKILL,
            index,
            skill,
            action: 0,
        };
        // Herb, Herbalism unknown → 0xdf "Requires %s" filled with the LockType name.
        let e = route_lock_refusal(
            &skill_slot(2, 0),
            false,
            false,
            3,
            0,
            Some("Herbalism"),
            KeyFact::Unknown,
        )
        .unwrap();
        assert_eq!(
            (e.key, e.arg_s(), e.arg_d()),
            ("ERR_USE_LOCKED_WITH_SPELL_S", Some("Herbalism"), None)
        );
        // Vein, Mining known but rank < 155 → 0xe0 "Requires %s %d" with the slot's Skill[0].
        let e = route_lock_refusal(
            &skill_slot(3, 155),
            true,
            false,
            3,
            0,
            Some("Mining"),
            KeyFact::Unknown,
        )
        .unwrap();
        assert_eq!(
            (e.key, e.arg_s(), e.arg_d()),
            (
                "ERR_USE_LOCKED_WITH_SPELL_KNOWN_SI",
                Some("Mining"),
                Some(155)
            )
        );
        // Skill[0] == 0 → the required rank falls back to GO-level × 5 (`0x5f3490`).
        let e = route_lock_refusal(
            &skill_slot(3, 0),
            true,
            false,
            3,
            20,
            Some("Mining"),
            KeyFact::Unknown,
        )
        .unwrap();
        assert_eq!(e.arg_d(), Some(100));
        // A missing LockType row fills the reference's literal fallback (`0x838044`).
        let e = route_lock_refusal(
            &skill_slot(9999, 0),
            false,
            false,
            3,
            0,
            None,
            KeyFact::Unknown,
        )
        .unwrap();
        assert_eq!(e.arg_s(), Some("UNKNOWN"));
        // Key lock, key absent: 0xde "Requires %s" when named, silent when uncached.
        let key_slot = LockSlot {
            key_type: LOCK_KEY_ITEM,
            index: 11000,
            skill: 0,
            action: 1,
        };
        let e = route_lock_refusal(
            &key_slot,
            false,
            false,
            0,
            0,
            None,
            KeyFact::Named("Shadowforge Key".into()),
        )
        .unwrap();
        assert_eq!(
            (e.key, e.arg_s()),
            ("ERR_USE_LOCKED_WITH_ITEM_S", Some("Shadowforge Key"))
        );
        assert!(
            route_lock_refusal(&key_slot, false, false, 0, 0, None, KeyFact::Unknown).is_none()
        );
        // GO_FLAG_LOCKED: the strategy default (`0x5f32a6`), even on a skill lock: door 0xdc,
        // button 0xdd, else 0xdb.
        for (go_type, key) in [
            (0, "ERR_DOOR_LOCKED"),
            (1, "ERR_BUTTON_LOCKED"),
            (3, "ERR_USE_LOCKED"),
        ] {
            let e = route_lock_refusal(
                &skill_slot(1, 0),
                false,
                true,
                go_type,
                0,
                Some("Pick Lock"),
                KeyFact::Unknown,
            )
            .unwrap();
            assert_eq!(e.key, key);
            assert_eq!(e.arg_s(), None);
        }
        // Slot-0 type neither key nor skill → 0xda "You can't open that."
        let odd = LockSlot {
            key_type: 7,
            index: 0,
            skill: 0,
            action: 0,
        };
        let e = route_lock_refusal(&odd, false, false, 3, 0, None, KeyFact::Unknown).unwrap();
        assert_eq!(e.key, "ERR_USE_CANT_OPEN");
    }

    /// The dead fork's rider test (`0x60bf98`): a mounted player never takes the loot leg.
    #[test]
    fn a_mounted_player_never_takes_the_loot_leg() {
        // On foot, a lootable corpse loots.
        assert_eq!(
            dead_unit_leg(false, true, true, false, false),
            DeadUnitLeg::Loot
        );
        // Mounted, the same corpse, nothing to skin: nothing at all.
        assert_eq!(
            dead_unit_leg(true, true, true, false, false),
            DeadUnitLeg::Nothing
        );
        // Mounted over a lootable and skinnable corpse, the fall-through skins.
        assert_eq!(
            dead_unit_leg(true, true, true, true, true),
            DeadUnitLeg::Skin
        );
        // On foot the same body loots: step 1 never reads the target.
        assert_eq!(
            dead_unit_leg(false, true, true, true, true),
            DeadUnitLeg::Loot
        );
    }

    #[test]
    fn the_dead_fork_keeps_its_other_three_gates() {
        // A live unit takes the alive branch (`0x60bf75 jg`).
        assert_eq!(
            dead_unit_leg(false, false, true, true, true),
            DeadUnitLeg::Nothing
        );
        // Dead, unlootable, skinnable, and we know the trade → Skin (`0x60c01f`).
        assert_eq!(
            dead_unit_leg(false, true, false, true, true),
            DeadUnitLeg::Skin
        );
        // Without the learn-time latch `[0xb700e4]` the same corpse gives nothing.
        assert_eq!(
            dead_unit_leg(false, true, false, true, false),
            DeadUnitLeg::Nothing
        );
        // A plain looted corpse: nothing, mounted or not.
        for mounted in [false, true] {
            assert_eq!(
                dead_unit_leg(mounted, true, false, false, true),
                DeadUnitLeg::Nothing
            );
        }
    }
    /// No combination of the dead fork's four inputs reaches [`UnitBranch::Service`].
    #[test]
    fn the_dead_fork_never_reaches_the_service_dispatch() {
        for mounted in [false, true] {
            for lootable in [false, true] {
                for skinnable in [false, true] {
                    for know_skinning in [false, true] {
                        let leg = dead_unit_leg(mounted, true, lootable, skinnable, know_skinning);
                        assert_eq!(
                            unit_branch(false, true, leg),
                            UnitBranch::Dead(leg),
                            "a dead target escaped the fork (mounted={mounted} \
                             lootable={lootable} skinnable={skinnable} know={know_skinning})"
                        );
                    }
                }
            }
        }
        // Mounted, over a lootable corpse, no skinning.
        let leg = dead_unit_leg(true, true, true, false, false);
        assert_eq!(
            unit_branch(false, true, leg),
            UnitBranch::Dead(DeadUnitLeg::Nothing)
        );
        // A live unit still reaches the service dispatch.
        assert_eq!(
            unit_branch(false, false, DeadUnitLeg::Nothing),
            UnitBranch::Service
        );
    }

    /// Every arm of `0x5f0130`'s walk over `UNIT_NPC_FLAGS`, in the reference's order.
    #[test]
    fn the_service_ladder_walks_the_reference_bit_order() {
        use cursor_mode::npc_flags as f;
        let has = Some(benilla_protocol::messages::dialog_status::AVAILABLE);
        for (flags, arm) in [
            (f::GOSSIP, ServiceArm::Gossip),
            (f::VENDOR, ServiceArm::Vendor),
            (f::FLIGHTMASTER, ServiceArm::FlightMaster),
            (f::TRAINER, ServiceArm::Trainer),
            (f::SPIRITHEALER, ServiceArm::SpiritHealer),
            (f::SPIRITGUIDE, ServiceArm::SpiritGuide),
            (f::INNKEEPER, ServiceArm::Innkeeper),
            (f::BANKER, ServiceArm::Banker),
            (f::PETITIONER, ServiceArm::Petitioner),
            (f::TABARDDESIGNER, ServiceArm::TabardDesigner),
            (f::BATTLEMASTER, ServiceArm::Battlemaster),
            (f::AUCTIONEER, ServiceArm::Auctioneer),
            (f::STABLEMASTER, ServiceArm::StableMaster),
        ] {
            assert_eq!(service_arm(flags, None), Some(arm), "flags {flags:#x}");
        }
        // Bit 1 is the one arm with a second conjunct: the target's cached questgiver status.
        assert_eq!(
            service_arm(f::QUESTGIVER, has),
            Some(ServiceArm::Questgiver)
        );
        assert_eq!(service_arm(f::QUESTGIVER, None), None);
        // REPAIR (bit 14) has no arm, and an empty field matches nothing.
        assert_eq!(service_arm(0x4000, None), None);
        assert_eq!(service_arm(0, None), None);
    }

    #[test]
    fn the_service_ladder_is_first_match_wins() {
        use cursor_mode::npc_flags as f;
        // Anything gossip-flagged keeps its menu.
        for other in [
            f::VENDOR,
            f::TRAINER,
            f::INNKEEPER,
            f::STABLEMASTER,
            f::BANKER,
        ] {
            assert_eq!(
                service_arm(f::GOSSIP | other, None),
                Some(ServiceArm::Gossip)
            );
        }
        // Banker (bit 8) before auctioneer (bit 12), both Buy(3) to the cursor.
        assert_eq!(
            service_arm(f::BANKER | f::AUCTIONEER, None),
            Some(ServiceArm::Banker)
        );
        // Trainer (bit 4) before innkeeper (bit 7).
        assert_eq!(
            service_arm(f::TRAINER | f::INNKEEPER, None),
            Some(ServiceArm::Trainer)
        );
    }

    #[test]
    fn the_service_arms_send_what_the_reference_sends() {
        let sent = |arm, ghost| match service_action(arm, 0x42, ghost, None) {
            ServiceAction::Send(cmd) => format!("{cmd:?}"),
            ServiceAction::SellFromCursor(cmd) => format!("sell {cmd:?}"),
            ServiceAction::AskBinder => "ask-binder".to_string(),
            ServiceAction::AskSpiritHealer => "ask-spirit-healer".to_string(),
            ServiceAction::AcquireSpiritGuide => "acquire-spirit-guide".to_string(),
            ServiceAction::Silent(_) => "silent".to_string(),
        };
        assert!(matches!(
            service_action(ServiceArm::Questgiver, 0x42, false, None),
            ServiceAction::Send(ClientCommand::QuestgiverHello { npc: 0x42 })
        ));
        assert!(matches!(
            service_action(ServiceArm::Trainer, 0x42, false, None),
            ServiceAction::Send(ClientCommand::TrainerList { trainer: 0x42 })
        ));
        assert!(matches!(
            service_action(ServiceArm::Petitioner, 0x42, false, None),
            ServiceAction::Send(ClientCommand::PetitionShowList { npc: 0x42 })
        ));
        assert!(matches!(
            service_action(ServiceArm::Vendor, 0x42, false, None),
            ServiceAction::Send(ClientCommand::ListInventory { guid: 0x42 })
        ));
        assert!(matches!(
            service_action(ServiceArm::FlightMaster, 0x42, false, None),
            ServiceAction::Send(ClientCommand::TaxiQueryNodes { guid: 0x42 })
        ));
        assert!(matches!(
            service_action(ServiceArm::Banker, 0x42, false, None),
            ServiceAction::Send(ClientCommand::BankerActivate { guid: 0x42 })
        ));
        assert!(matches!(
            service_action(ServiceArm::Auctioneer, 0x42, false, None),
            ServiceAction::Send(ClientCommand::AuctionHello { auctioneer: 0x42 })
        ));
        assert!(matches!(
            service_action(ServiceArm::StableMaster, 0x42, false, None),
            ServiceAction::Send(ClientCommand::ListStabledPets { npc: 0x42 })
        ));
        // The innkeeper asks and sends nothing.
        assert_eq!(sent(ServiceArm::Innkeeper, false), "ask-binder");
        // The two ghost-gated arms give a living player nothing, as in the reference.
        assert_eq!(sent(ServiceArm::SpiritHealer, false), "silent");
        assert_eq!(sent(ServiceArm::SpiritGuide, false), "silent");
        assert_eq!(sent(ServiceArm::SpiritHealer, true), "ask-spirit-healer");
        assert!(matches!(
            service_action(ServiceArm::TabardDesigner, 0x42, false, None),
            ServiceAction::Send(ClientCommand::TabardVendorActivate { npc: 0x42 })
        ));
        assert!(matches!(
            service_action(ServiceArm::Battlemaster, 0x42, false, None),
            ServiceAction::Send(ClientCommand::BattlemasterHello { npc: 0x42 })
        ));
    }

    /// The sell fork lives in the vendor handler alone (`0x5df5d0`), so no other arm may change
    /// with the cursor.
    #[test]
    fn only_the_vendor_arm_reads_the_cursor() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        const ITEM: u64 = 0x4000_0000_0000_0099;

        // Empty cursor: the list opens.
        assert!(matches!(
            service_action(ServiceArm::Vendor, VENDOR, false, None),
            ServiceAction::Send(ClientCommand::ListInventory { guid: VENDOR })
        ));
        // An item held: the sale to the clicked NPC, count 0 (the whole stack).
        assert!(matches!(
            service_action(ServiceArm::Vendor, VENDOR, false, Some(ITEM)),
            ServiceAction::SellFromCursor(ClientCommand::SellItem {
                vendor: VENDOR,
                item_guid: ITEM,
                count: 0,
            })
        ));
        // Every other arm answers the same, held or not.
        for arm in [
            ServiceArm::Gossip,
            ServiceArm::Questgiver,
            ServiceArm::FlightMaster,
            ServiceArm::Trainer,
            ServiceArm::SpiritHealer,
            ServiceArm::SpiritGuide,
            ServiceArm::Innkeeper,
            ServiceArm::Banker,
            ServiceArm::Petitioner,
            ServiceArm::TabardDesigner,
            ServiceArm::Battlemaster,
            ServiceArm::Auctioneer,
            ServiceArm::StableMaster,
        ] {
            for ghost in [false, true] {
                let describe = |a: ServiceAction| match a {
                    ServiceAction::Send(cmd) => format!("{cmd:?}"),
                    ServiceAction::SellFromCursor(cmd) => format!("SELL {cmd:?}"),
                    ServiceAction::AskBinder => "ask-binder".into(),
                    ServiceAction::AskSpiritHealer => "ask-xp-loss".into(),
                    ServiceAction::AcquireSpiritGuide => "acquire-spirit-guide".into(),
                    ServiceAction::Silent(w) => format!("silent {w}"),
                };
                let empty = describe(service_action(arm, VENDOR, ghost, None));
                let held = describe(service_action(arm, VENDOR, ghost, Some(ITEM)));
                assert_eq!(
                    empty, held,
                    "{arm:?} (ghost={ghost}) changed its answer because an item was on the cursor"
                );
                assert!(
                    !held.starts_with("SELL"),
                    "{arm:?} (ghost={ghost}) reached the vendor arm's sale"
                );
            }
        }
    }

    /// The re-click gate through the real [`crate::ui_session::feed_interact_npc`]: nothing arms
    /// it before a window opens, a window arms its own NPC, and a close disarms it.
    #[test]
    fn the_reclick_gate_fires_only_while_that_npc_s_window_is_open() {
        use crate::ui_session::{feed_interact_npc, InteractNpc};
        use bevy::ecs::system::RunSystemOnce;

        const NPC: u64 = 0xF130_0000_0000_0042;
        const OTHER: u64 = 0xF130_0000_0000_0043;

        let armed = |seed: &dyn Fn(&mut World)| {
            let mut world = World::new();
            world.init_resource::<InteractNpc>();
            seed(&mut world);
            world.run_system_once(feed_interact_npc).unwrap();
            world.remove_resource::<InteractNpc>().unwrap()
        };

        // Nothing open: a first click on any NPC reaches the ladder.
        let idle = armed(&|_| {});
        assert!(!interaction_already_open_on(NPC, &idle));
        assert!(!interaction_already_open_on(OTHER, &idle));

        // One opener per family (a menu, a quest panel, a specialized window) arms its own NPC,
        // through the latch it registers on (`0x4930d0`).
        let with_gossip = armed(&|w| {
            w.init_resource::<crate::ui_gossip::GossipState>();
            crate::ui_session::register(w, NPC);
            w.resource_mut::<crate::ui_gossip::GossipState>().npc = Some(NPC);
        });
        let with_quest = armed(&|w| {
            w.init_resource::<crate::ui_quest::QuestGiver>();
            crate::ui_quest::open_panel(
                w,
                NPC,
                crate::ui_quest::QuestView::Greeting(benilla_protocol::messages::QuestGiverList {
                    npc: NPC,
                    greeting: String::new(),
                    emote_delay: 0,
                    emote: 0,
                    quests: Vec::new(),
                }),
            );
        });
        let with_trainer = armed(&|w| {
            w.init_resource::<crate::ui_trainer::TrainerOpen>();
            crate::ui_trainer::open_trainer(w, NPC, 0, Vec::new(), String::new());
        });
        for (what, armed) in [
            ("gossip", &with_gossip),
            ("quest", &with_quest),
            ("trainer", &with_trainer),
        ] {
            assert_eq!(armed.1, Some(NPC), "{what} did not arm the npc token");
            // A re-click on that NPC is eaten...
            assert!(
                interaction_already_open_on(NPC, armed),
                "{what}: the re-click was not suppressed"
            );
            // ...but a click on another NPC is not.
            assert!(
                !interaction_already_open_on(OTHER, armed),
                "{what}: the gate ate a click on a different NPC"
            );
        }

        // Closing the window disarms it, as the reference's `0x493310` zeroes the pair.
        let closed = armed(&|w| {
            w.init_resource::<crate::ui_trainer::TrainerOpen>();
            crate::ui_trainer::open_trainer(w, NPC, 0, Vec::new(), String::new());
            crate::ui_trainer::close_core(w);
        });
        assert_eq!(closed.1, None);
        assert!(!interaction_already_open_on(NPC, &closed));
    }

    /// A corpse and a refused `NOT_SELECTABLE` unit reach `select_on_click`'s `_` arm with no
    /// `target`, but are object hits, and only the terrain and nothing legs deselect.
    #[test]
    fn an_object_hit_never_deselects_but_empty_world_does() {
        use crate::net::NetCommands;
        use bevy::ecs::system::RunSystemOnce;

        const HELD: u64 = 0xF00D;

        // One click of the left button over `pick`, returning the selection it left behind.
        let click = |pick: Hovered| {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let mut world = World::new();
            world.insert_resource(NetCommands(tx));
            world.init_resource::<InspectMode>();
            world.init_resource::<crate::spell::QueuedMeleeSpell>();
            world.init_resource::<crate::spell::PendingCast>();
            world.init_resource::<crate::ui_cast::CastReports>();
            world.init_resource::<crate::spell::AutoRepeatActive>();
            world.init_resource::<crate::creature_anim::UnitEdits>();
            world.init_resource::<crate::ui_loot::LootState>();
            world.init_resource::<crate::ui_loot::LootLatch>();
            world.init_resource::<crate::ui_script::CursorPayloadHeld>();
            world.init_resource::<crate::spell::SpellTargeting>();
            world.init_resource::<ClickConfig>();
            world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
            world.init_resource::<Messages<crate::player::StandStateRequest>>();
            world.init_resource::<Messages<crate::sound::NpcGreetingRequest>>();
            world.init_resource::<Messages<WorldClick>>();
            world.insert_resource(Selection {
                target: Some(Entity::PLACEHOLDER),
                guid: Some(HELD),
                ..Default::default()
            });
            world.insert_resource(PressPick {
                hovered: pick,
                ..PressPick::default()
            });
            world.spawn(SelfPlayer);
            world
                .resource_mut::<Messages<WorldClick>>()
                .write(WorldClick);
            world
                .run_system_once(select_on_click)
                .expect("select_on_click runs as a one-shot system");
            world.resource::<Selection>().guid
        };

        assert_eq!(
            click(Hovered::default()),
            None,
            "clicking the sky deselects"
        );
        assert_eq!(
            click(Hovered {
                corpse: Some(Entity::PLACEHOLDER),
                corpse_guid: Some(0xB0DE),
                distance: 5.0,
                ..Hovered::default()
            }),
            Some(HELD),
            "a body is an object hit — the target must survive it"
        );
        assert_eq!(
            click(Hovered {
                refused: true,
                distance: 5.0,
                ..Hovered::default()
            }),
            Some(HELD),
            "so is a NOT_SELECTABLE unit the grader threw away"
        );
    }

    use crate::net::{ClientCommand, NetCommands, ObjectStore};
    use bevy::ecs::system::RunSystemOnce;

    const F_HEALTH: u16 = 22;
    const F_MAXHEALTH: u16 = 28;
    /// `GAMEOBJECT_TYPE_ID`, absolute field 21, which the GameObject arms fork on.
    const GO_TYPE_FIELD: u16 = 21;
    const BOAR: u64 = 0xB0A2;
    const ME: u64 = 0x5E1F;

    fn store(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs))
    }

    /// Everything [`act_on_right_click`] and the commit under it reach for.
    fn right_click_world() -> (World, Entity) {
        let (tx, _rx) = crossbeam_channel::unbounded::<ClientCommand>();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<Messages<WorldRightClick>>();
        world.init_resource::<Messages<PlateRightClick>>();
        world.init_resource::<PressPick>();
        world.init_resource::<Selection>();
        world.init_resource::<crate::spell::SpellTargeting>();
        world.init_resource::<crate::spell::CursorPicks>();
        world.init_resource::<crate::spell::targeting::EnchantConfirmItem>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<crate::creature_anim::UnitEdits>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<Messages<crate::player::StandStateRequest>>();
        world.init_resource::<crate::creature_anim::GestureQueue>();
        world.init_resource::<crate::go_templates::GameObjectTemplates>();
        world.init_resource::<crate::items::Items>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::ui_action::PlayerActions>();
        world.init_resource::<crate::ui_action::LearnedAbilities>();
        world.init_resource::<crate::ui_quest::QuestGiver>();
        world.init_resource::<crate::ui_binder::BinderState>();
        world.init_resource::<crate::death::DeathNet>();
        world.init_resource::<crate::ui_dialog_verbs::AreaSpiritHealer>();
        world.init_resource::<crate::ui_session::InteractNpc>();
        world.init_resource::<crate::ui_action::UiErrorKeys>();
        world.init_resource::<crate::ui_cast::CastReports>();
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::ui_loot::LootState>();
        world.init_resource::<crate::ui_loot::LootLatch>();
        world.init_resource::<crate::ui_mail::MailOpen>();
        world.init_resource::<crate::ui_item_text::ItemTextOpen>();
        world.init_resource::<crate::ui_action::ClickCasts>();
        world.init_resource::<Messages<crate::ui_dialog_verbs::MeetingStoneUse>>();
        world.init_resource::<crate::player::Approach>();
        world.init_resource::<crate::player::FollowState>();
        world.init_resource::<crate::player::Player>();
        world.init_resource::<crate::player::OwnControlLost>();
        world.init_resource::<crate::ui_script::CursorPayloadHeld>();
        world.init_resource::<Messages<Interact>>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<crate::ui_loot::LootConfig>();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.spawn((SelfPlayer, Guid(ME)));
        let boar = world
            .spawn((Guid(BOAR), store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100)])))
            .id();
        (world, boar)
    }

    /// `InteractWithSpell 0x5f05e0`: a body in reach, or out of reach with Click to Move off
    /// (`0x5f06a1`), is skinned through the attempt with its guid (`0x5f06dc` → `0x6e5a90`), which
    /// the click queues for the ladder, sending nothing itself; out of reach with it on, the click
    /// walks there and casts nothing (`0x5f06c4`).
    #[test]
    fn a_skin_click_casts_through_the_attempt_unless_it_walks() {
        const SKINNING: u32 = 8613;
        const F_UNIT_FLAGS: u16 = benilla_protocol::field::FIELD_UNIT_FLAGS;
        for (in_reach, walk, casts) in [
            (true, false, true),
            (true, true, true),
            (false, false, true),
            (false, true, false),
        ] {
            let label = format!("in reach {in_reach}, walk {walk}");
            let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
            let (mut world, boar) = right_click_world();
            world.insert_resource(NetCommands(tx));
            world.resource_mut::<crate::player::Approach>().enabled = walk;
            let me = world
                .query_filtered::<Entity, With<SelfPlayer>>()
                .single(&world)
                .unwrap();
            world.entity_mut(me).insert((
                store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100)]),
                Transform::default(),
            ));
            // The click measures the melee reach itself: 5 yd for these two.
            world.entity_mut(boar).insert((
                store(&[
                    (F_HEALTH, 0),
                    (F_MAXHEALTH, 100),
                    (F_UNIT_FLAGS, cursor_mode::UNIT_FLAG_SKINNABLE),
                ]),
                Transform::from_xyz(if in_reach { 2.0 } else { 20.0 }, 0.0, 0.0),
            ));
            world
                .resource_mut::<crate::ui_action::LearnedAbilities>()
                .skinning = Some(SKINNING);
            *world.resource_mut::<PressPick>() = PressPick {
                hovered: Hovered {
                    target: Some(boar),
                    guid: Some(BOAR),
                    distance: 2.0,
                    ..Hovered::default()
                },
                cursor: WorldCursor {
                    kind: cursor_mode::CursorKind::Skin,
                    unable: !in_reach,
                },
                ..PressPick::default()
            };
            world
                .resource_mut::<Messages<WorldRightClick>>()
                .write(WorldRightClick);
            world.run_system_once(act_on_right_click).unwrap();
            let queued = &world.resource::<crate::ui_action::ClickCasts>().0;
            assert_eq!(
                matches!(
                    queued[..],
                    [crate::ui_action::ClickCast::AtUnit {
                        spell_id: SKINNING,
                        entity,
                        guid: BOAR,
                    }] if entity == boar
                ),
                casts,
                "{label}: {queued:?}"
            );
            assert!(
                !rx.try_iter()
                    .any(|c| matches!(c, ClientCommand::CastSpell { .. })),
                "{label}: the click sends no cast itself"
            );
        }
    }

    /// The skin's attempt (`0x6e4b60`), with the body's guid passed in, runs every rung a press
    /// does, in order: another spell in flight refuses it first (`0x6e4d97`, 0x61), ahead of the
    /// validator's mounted rung (`0x6e4f3b` → `0x609c6c`, 0x39), and either refusal sends
    /// nothing. A passing skin is pushed in flight (`0x6e5026`) and sent at the body
    /// (`0x6e54f0`), which needs no selection: an [`Interact`] runs the dispatcher with none.
    #[test]
    fn a_skin_meets_the_attempts_rungs_and_holds_the_slot() {
        use crate::net::{GuidIndex, SelfGuid};
        const SKINNING: u32 = 8613;
        const FIREBALL: u32 = 133;
        const F_MOUNT: u16 = benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID;
        const F_UNIT_FLAGS: u16 = benilla_protocol::field::FIELD_UNIT_FLAGS;
        for (in_flight, mounted, refused) in [
            (false, false, None),
            (true, false, Some(0x61)),
            (false, true, Some(0x39)),
            (true, true, Some(0x61)),
        ] {
            let label = format!("in flight {in_flight}, mounted {mounted}");
            let mut app = crate::game_plugins::schedule_tests::headless_client();
            let (tx, rx) = crossbeam_channel::unbounded();
            // In world: the body we drive.
            app.insert_resource(NetCommands(tx))
                .init_resource::<crate::player::Player>();
            let world = app.world_mut();
            let mut mine = vec![(2, 0x19), (F_HEALTH, 100), (F_MAXHEALTH, 100)];
            if mounted {
                mine.push((F_MOUNT, 14_337));
            }
            let me = world
                .spawn((
                    SelfPlayer,
                    Guid(ME),
                    Transform::default(),
                    GlobalTransform::default(),
                    store(&mine),
                ))
                .id();
            let boar = world
                .spawn((
                    Guid(BOAR),
                    crate::net::NetEntity {
                        kind: benilla_protocol::EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                    Transform::from_xyz(1.0, 0.0, 0.0),
                    GlobalTransform::from_xyz(1.0, 0.0, 0.0),
                    store(&[
                        (F_HEALTH, 0),
                        (F_MAXHEALTH, 100),
                        (F_UNIT_FLAGS, cursor_mode::UNIT_FLAG_SKINNABLE),
                    ]),
                ))
                .id();
            world
                .resource_mut::<GuidIndex>()
                .0
                .extend([(ME, me), (BOAR, boar)]);
            world.resource_mut::<SelfGuid>().0 = Some(ME);
            world
                .resource_mut::<crate::ui_action::LearnedAbilities>()
                .skinning = Some(SKINNING);
            let mut spells = crate::ui_action::Spells::empty_for_tests();
            spells.catalog = benilla_formats::SpellCatalog::from_displays(
                [(
                    SKINNING,
                    benilla_formats::SpellDisplay {
                        targets: 0x402,
                        implicit_target_a1: 25,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            );
            world.insert_resource(spells);
            if in_flight {
                world.resource_mut::<crate::spell::PendingCast>().push(
                    FIREBALL,
                    None,
                    std::time::Instant::now(),
                );
            }
            world
                .resource_mut::<crate::ui_cast::CastReports>()
                .0
                .clear();
            world
                .resource_mut::<Messages<Interact>>()
                .write(Interact(boar));
            world.run_system_once(act_on_interact).unwrap();
            assert_eq!(
                world.resource::<crate::ui_action::ClickCasts>().0.len(),
                1,
                "{label}: the dispatcher queued the skin"
            );
            assert_eq!(world.resource::<Selection>().guid, None, "{label}");
            world
                .run_system_once(crate::ui_action::drain_click_casts)
                .unwrap();
            let casts: Vec<_> = rx
                .try_iter()
                .filter_map(|c| match c {
                    ClientCommand::CastSpell { spell_id, target } => Some((spell_id, target)),
                    _ => None,
                })
                .collect();
            let refusals: Vec<_> = world
                .resource::<crate::ui_cast::CastReports>()
                .refusals()
                .iter()
                .map(|f| f.reason)
                .collect();
            let pending = world.resource::<crate::spell::PendingCast>().current();
            match refused {
                None => {
                    assert_eq!(casts, [(SKINNING, Some(BOAR))], "{label}");
                    assert!(refusals.is_empty(), "{label}");
                    assert_eq!(pending, Some(SKINNING), "{label}: in flight");
                }
                Some(reason) => {
                    assert!(casts.is_empty(), "{label}");
                    assert_eq!(refusals, [reason], "{label}");
                    assert_ne!(pending, Some(SKINNING), "{label}");
                }
            }
        }
    }

    /// The dispatcher's attack arm (`0x60c18c`) is taken on the fork, not the sword: a ghost over
    /// a hostile NPC sees the pointer, and its right-click is refused silently inside the arm
    /// (`0x60c1a1`), with no swing and no service packet; a live player on the same fork swings.
    #[test]
    fn a_ghost_takes_the_attack_arm_and_is_refused_silently() {
        const F_PLAYER_FLAGS: u16 = 190;
        const F_NPC_FLAGS: u16 = 147;
        for (label, own, swings) in [
            (
                "ghost",
                &[(F_HEALTH, 1), (F_MAXHEALTH, 100), (F_PLAYER_FLAGS, 0x10)][..],
                false,
            ),
            ("live", &[(F_HEALTH, 100), (F_MAXHEALTH, 100)][..], true),
        ] {
            let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
            let (mut world, _boar) = right_click_world();
            world.insert_resource(NetCommands(tx));
            let me = world
                .query_filtered::<Entity, With<SelfPlayer>>()
                .single(&world)
                .unwrap();
            world.entity_mut(me).insert(store(own));
            // A hostile gossip NPC: its service bit must never be read on the attack arm.
            let npc = world
                .spawn((
                    Guid(BOAR + 1),
                    store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100), (F_NPC_FLAGS, 0x1)]),
                ))
                .id();
            *world.resource_mut::<PressPick>() = PressPick {
                hovered: Hovered {
                    target: Some(npc),
                    guid: Some(BOAR + 1),
                    distance: 3.0,
                    ..Hovered::default()
                },
                attack_fork: cursor_mode::AttackFork(true),
                ..PressPick::default()
            };
            world
                .resource_mut::<Messages<WorldRightClick>>()
                .write(WorldRightClick);
            world.run_system_once(act_on_right_click).unwrap();
            let sent: Vec<_> = rx.try_iter().collect();
            assert!(
                matches!(sent.first(), Some(ClientCommand::SetSelection { guid }) if *guid == BOAR + 1),
                "{label}: the select stands: {sent:?}"
            );
            assert!(
                !sent
                    .iter()
                    .any(|c| matches!(c, ClientCommand::GossipHello { .. })),
                "{label}: the attack arm never reaches the service leg: {sent:?}"
            );
            assert_eq!(
                sent.iter()
                    .any(|c| matches!(c, ClientCommand::AttackSwing { .. })),
                swings,
                "{label}: {sent:?}"
            );
            assert!(
                world.resource::<crate::ui_action::UiErrorKeys>().is_empty(),
                "{label}: the arm says nothing"
            );
        }
    }

    /// A meeting stone's own use slot (`0x5f69d0`) replaces the shared sender: the click hands it
    /// to the join validator and puts no `0xB1` on the wire, which vmangos' type-23 `Use` ignores.
    #[test]
    fn a_right_click_on_a_meeting_stone_joins_it_and_sends_no_gameobj_use() {
        const STONE: u64 = 0x5701;
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        let (mut world, _boar) = right_click_world();
        world.insert_resource(NetCommands(tx));
        let stone = world
            .spawn((Guid(STONE), store(&[(GO_TYPE_FIELD, 23)])))
            .id();
        *world.resource_mut::<PressPick>() = PressPick {
            object: HoveredObject {
                target: Some(stone),
                guid: Some(STONE),
                distance: 5.0,
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Interact,
                unable: false,
            },
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();

        let uses: Vec<_> = world
            .resource::<Messages<crate::ui_dialog_verbs::MeetingStoneUse>>()
            .iter_current_update_messages()
            .copied()
            .collect();
        assert_eq!(
            uses,
            vec![crate::ui_dialog_verbs::MeetingStoneUse { go_guid: STONE }],
            "the stone must reach the join validator"
        );
        assert!(
            !rx.try_iter()
                .any(|c| matches!(c, ClientCommand::GameObjUse { .. })),
            "a meeting stone must never send CMSG_GAMEOBJ_USE — the server drops it on the floor"
        );
    }

    #[test]
    fn a_right_click_on_a_gameobject_leaves_the_selection_untouched() {
        const STONE: u64 = 0x5703;
        let (mut world, _boar) = right_click_world();
        let stone = world
            .spawn((Guid(STONE), store(&[(GO_TYPE_FIELD, 23)])))
            .id();
        *world.resource_mut::<PressPick>() = PressPick {
            object: HoveredObject {
                target: Some(stone),
                guid: Some(STONE),
                distance: 5.0,
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Interact,
                unable: false,
            },
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.clear_trackers();
        world.run_system_once(act_on_right_click).unwrap();
        assert!(!world.is_resource_changed::<Selection>());
    }

    const VENDOR: u64 = 0x7E0D;

    /// Our body alive at the origin, Click to Move set as `walk`, and a vendor at `at`.
    fn walking_world(
        at: Vec3,
        walk: bool,
    ) -> (World, Entity, crossbeam_channel::Receiver<ClientCommand>) {
        const F_NPC_FLAGS: u16 = 147;
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        let (mut world, _boar) = right_click_world();
        world.insert_resource(NetCommands(tx));
        world.resource_mut::<crate::player::Approach>().enabled = walk;
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(&world)
            .unwrap();
        world.entity_mut(me).insert((
            store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100)]),
            Transform::default(),
        ));
        let vendor = world
            .spawn((
                Guid(VENDOR),
                store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100), (F_NPC_FLAGS, 0x4)]),
                Transform::from_translation(at),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(VENDOR, vendor);
        (world, vendor, rx)
    }

    fn click_vendor(world: &mut World, vendor: Entity) {
        *world.resource_mut::<PressPick>() = PressPick {
            hovered: Hovered {
                target: Some(vendor),
                guid: Some(VENDOR),
                distance: 14.0,
                ..Hovered::default()
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Buy,
                unable: true,
            },
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
    }

    /// `0x5f0130`: out of reach, `CanAutoInteract` starts the walk and nothing goes out; with the
    /// option off the click stays refused.
    #[test]
    fn a_far_vendor_click_walks_only_with_click_to_move_on() {
        for walk in [true, false] {
            let (mut world, vendor, rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), walk);
            click_vendor(&mut world, vendor);
            assert_eq!(
                world.resource::<crate::player::Approach>().active(),
                walk,
                "AutoInteract {walk}"
            );
            let sent: Vec<ClientCommand> = rx.try_iter().collect();
            assert!(
                !sent
                    .iter()
                    .any(|c| matches!(c, ClientCommand::ListInventory { .. })),
                "no list before the walk ends: {sent:?}"
            );
        }
    }

    /// `0x60fa20` on arrival: the ladder alone, with no select and no walk of its own.
    #[test]
    fn an_arrival_opens_the_vendor_it_walked_to() {
        let (mut world, _vendor, rx) = walking_world(Vec3::new(2.5, 0.0, 0.0), true);
        world.resource_mut::<crate::player::Approach>().arrived =
            Some((crate::player::ApproachVerb::Talk, VENDOR));
        world.run_system_once(act_on_arrival).unwrap();
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::ListInventory { guid: VENDOR }]
            ),
            "{sent:?}"
        );
        assert!(world
            .resource::<crate::player::Approach>()
            .arrived
            .is_none());
    }

    const CHEST: u64 = 0xC4E5;

    fn spawn_object(world: &mut World, go_type: u32, at: Vec3) -> Entity {
        let object = world
            .spawn((
                Guid(CHEST),
                store(&[(GO_TYPE_FIELD, go_type)]),
                Transform::from_translation(at),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::GameObject,
                    display_id: None,
                    scale: 1.0,
                },
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(CHEST, object);
        object
    }

    fn spawn_chest(world: &mut World, at: Vec3) -> Entity {
        spawn_object(world, 3, at)
    }

    /// Right-click the object out of reach (the cursor grayed, as for a far object), and the red
    /// error keys the click raised.
    fn click_far_object(world: &mut World, object: Entity, distance: f32) -> Vec<&'static str> {
        *world.resource_mut::<PressPick>() = PressPick {
            object: HoveredObject {
                target: Some(object),
                guid: Some(CHEST),
                distance,
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Interact,
                unable: true,
            },
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
        world
            .resource::<crate::ui_action::UiErrorKeys>()
            .snapshot()
            .iter()
            .map(|e| e.key)
            .collect()
    }

    /// A crate's [`Interact`] on `entity`, the selection's change tick cleared first.
    fn interact_with(world: &mut World, entity: Entity) {
        world.clear_trackers();
        world
            .resource_mut::<Messages<Interact>>()
            .write(Interact(entity));
        world.run_system_once(act_on_interact).unwrap();
    }

    /// The vendor leg (`0x5f0130`) runs the dispatcher alone: no select.
    #[test]
    fn an_interaction_opens_a_vendor_in_reach_and_leaves_the_selection() {
        let (mut world, vendor, rx) = walking_world(Vec3::new(2.5, 0.0, 0.0), false);
        interact_with(&mut world, vendor);
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::ListInventory { guid: VENDOR }]
            ),
            "{sent:?}"
        );
        assert_eq!(world.resource::<Selection>().guid, None);
        assert!(!world.is_resource_changed::<Selection>());
    }

    #[test]
    fn a_far_vendor_interaction_walks_only_with_click_to_move_on() {
        for walk in [true, false] {
            let (mut world, vendor, rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), walk);
            interact_with(&mut world, vendor);
            assert_eq!(
                world.resource::<crate::player::Approach>().active(),
                walk,
                "AutoInteract {walk}"
            );
            let sent: Vec<ClientCommand> = rx.try_iter().collect();
            assert!(sent.is_empty(), "{sent:?}");
        }
    }

    /// `0x5f8660` → `0x5f86b0`: in reach the object is used; out of reach, with no walk, the range
    /// arm raises `ERR_USE_TOO_FAR` (`0x5f874b`) and sends nothing. Neither selects.
    #[test]
    fn an_interaction_uses_a_gameobject_in_reach_and_refuses_one_out_of_it() {
        for (x, used) in [(2.0, true), (30.0, false)] {
            let (mut world, _vendor, rx) = walking_world(Vec3::ZERO, false);
            let chest = spawn_chest(&mut world, Vec3::new(x, 0.0, 0.0));
            interact_with(&mut world, chest);
            let sent: Vec<ClientCommand> = rx.try_iter().collect();
            assert_eq!(
                matches!(sent.as_slice(), [ClientCommand::GameObjUse { guid: CHEST }]),
                used,
                "{x} yd: {sent:?}"
            );
            let keys: Vec<&str> = world
                .resource::<crate::ui_action::UiErrorKeys>()
                .snapshot()
                .iter()
                .map(|e| e.key)
                .collect();
            assert_eq!(
                keys,
                if used {
                    vec![]
                } else {
                    vec!["ERR_USE_TOO_FAR"]
                },
                "{x} yd"
            );
            assert!(!world.is_resource_changed::<Selection>(), "{x} yd");
        }
    }

    /// `usable 0x5f3130`'s player arms: dead or a ghost (`0x5f315f`) grays every object's cursor and
    /// its use raises `ERR_PLAYER_DEAD` (0x7e) with nothing sent and no mail window; mounted
    /// (`0x5f31a8`) grays a lock-less chest silently, never a mailbox (`0x5f31bb`).
    #[test]
    fn a_dead_ghost_or_mounted_player_is_refused_an_object_in_reach() {
        const F_PLAYER_FLAGS: u16 = 190;
        const F_MOUNT: u16 = benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID;
        const DEAD: &[(u16, u32)] = &[(F_HEALTH, 0), (F_MAXHEALTH, 100)];
        const GHOST: &[(u16, u32)] = &[(F_HEALTH, 1), (F_MAXHEALTH, 100), (F_PLAYER_FLAGS, 0x10)];
        const MOUNTED: &[(u16, u32)] = &[(F_HEALTH, 100), (F_MAXHEALTH, 100), (F_MOUNT, 14_337)];
        const LIVE: &[(u16, u32)] = &[(F_HEALTH, 100), (F_MAXHEALTH, 100)];
        const REFUSED: Option<&str> = Some("ERR_PLAYER_DEAD");
        // (label, our fields, GameObject type, grayed, used, error key)
        let cases = [
            ("dead chest", DEAD, 3, true, false, REFUSED),
            ("dead mailbox", DEAD, 19, true, false, REFUSED),
            ("ghost chest", GHOST, 3, true, false, REFUSED),
            ("ghost mailbox", GHOST, 19, true, false, REFUSED),
            ("mounted chest", MOUNTED, 3, true, false, None),
            ("mounted mailbox", MOUNTED, 19, false, true, None),
            ("live chest", LIVE, 3, false, true, None),
            ("live mailbox", LIVE, 19, false, true, None),
        ];
        for (label, own, go_type, grayed, used, error) in cases {
            let (mut world, _vendor, rx) = walking_world(Vec3::ZERO, false);
            let me = world
                .query_filtered::<Entity, With<SelfPlayer>>()
                .single(&world)
                .unwrap();
            world.entity_mut(me).insert(store(own));
            let object = spawn_object(&mut world, go_type, Vec3::new(2.0, 0.0, 0.0));
            let hovered = HoveredObject {
                target: Some(object),
                guid: Some(CHEST),
                distance: 2.0,
            };
            let cursor = world
                .run_system_once(move |inputs: cursor_mode::CursorInputs| {
                    cursor_mode::classify(&inputs, &Hovered::default(), &hovered).0
                })
                .unwrap();
            assert_eq!(cursor.unable, grayed, "{label}: {cursor:?}");
            interact_with(&mut world, object);
            let sent_use = rx
                .try_iter()
                .any(|c| matches!(c, ClientCommand::GameObjUse { .. }));
            let mail_open = world
                .resource::<crate::ui_mail::MailOpen>()
                .mailbox
                .is_some();
            assert_eq!(sent_use || mail_open, used, "{label}");
            let keys: Vec<&str> = world
                .resource::<crate::ui_action::UiErrorKeys>()
                .snapshot()
                .iter()
                .map(|e| e.key)
                .collect();
            assert_eq!(keys, error.into_iter().collect::<Vec<_>>(), "{label}");
        }
    }

    const WOLF: u64 = 0xF130_0000_0000_0299;

    /// Our body hostile to nobody in particular and a wolf three yards off, the attack cursor on
    /// it.
    fn wolf_world() -> (World, Entity, crossbeam_channel::Receiver<ClientCommand>) {
        const F_UNIT_FLAGS: u16 = benilla_protocol::field::FIELD_UNIT_FLAGS;
        let (mut world, _vendor, rx) = walking_world(Vec3::ZERO, false);
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(&world)
            .unwrap();
        world.entity_mut(me).insert(store(&[
            (F_HEALTH, 100),
            (F_MAXHEALTH, 100),
            (F_UNIT_FLAGS, 0x8),
        ]));
        let wolf = world
            .spawn((
                Guid(WOLF),
                store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100)]),
                Transform::from_xyz(3.0, 0.0, 0.0),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
            ))
            .id();
        (world, wolf, rx)
    }

    /// The attack leg `0x60c247` calls StartAttack (`0x5ecb70`), which selects the target
    /// (`SetSelection`, `0x5eccaa`) before the swing (`0x5eccfd`), as the right-click does.
    #[test]
    fn an_interaction_attacks_an_attackable_unit_and_selects_it() {
        let (mut world, wolf, rx) = wolf_world();
        interact_with(&mut world, wolf);
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        let at = |want: fn(&ClientCommand) -> bool| sent.iter().position(want);
        let select = at(|c| matches!(c, ClientCommand::SetSelection { guid: WOLF }));
        let swing = at(|c| matches!(c, ClientCommand::AttackSwing { guid: WOLF }));
        assert!(
            select.is_some() && swing.is_some() && select < swing,
            "{sent:?}"
        );
        assert_eq!(world.resource::<Selection>().guid, Some(WOLF));
    }

    /// Mid-combat on another target the select is the click's: stop, select, swing.
    #[test]
    fn an_interaction_switching_targets_mid_combat_stops_selects_and_swings() {
        const OLD: u64 = 0xF130_0000_0000_0111;
        let (mut world, wolf, rx) = wolf_world();
        let old = world.spawn((Guid(OLD), store(&[(F_HEALTH, 100)]))).id();
        {
            let mut selection = world.resource_mut::<Selection>();
            selection.guid = Some(OLD);
            selection.target = Some(old);
        }
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(&world)
            .unwrap();
        world.entity_mut(me).insert(Engaged(OLD));
        interact_with(&mut world, wolf);
        let sent: Vec<&str> = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::AttackStop => "stop",
                ClientCommand::SetSelection { guid: WOLF } => "select",
                ClientCommand::AttackSwing { guid: WOLF } => "swing",
                _ => "other",
            })
            .collect();
        assert_eq!(sent, ["stop", "select", "swing"]);
        assert_eq!(world.resource::<Selection>().guid, Some(WOLF));
    }

    /// `0x5df2a0`: beyond melee reach the walk comes first and nothing is looted yet.
    #[test]
    fn a_far_body_to_loot_is_walked_to_before_the_loot() {
        let (mut world, body, corpse, rx) = loot_world(12.0, 0, true);
        right_click_loot(&mut world, loot_hover(false, body, corpse, 12.0));
        assert!(world.resource::<crate::player::Approach>().active());
        assert!(!rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::Loot { .. })));
    }

    /// `0x611130`: a Loot walk stops at the square root (`0x6111ab`) of the linear melee reach
    /// `0x6112b6` stores: `sqrt(5)` for a corpse (`0x611317`), `sqrt(max(rA + rB + 1.3333, 5))`
    /// for a unit.
    #[test]
    fn a_loot_walk_stops_at_the_square_root_of_the_melee_reach() {
        const F_DYNAMIC_FLAGS: u16 = 143;
        const F_COMBAT_REACH: u16 = 130;
        // A unit of reach 4 against our default 1.5: 4 + 1.5 + 1.3333 = 6.8333.
        let (mut world, _vendor, _rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
        let body = world
            .spawn((
                Guid(BODY),
                store(&[
                    (F_HEALTH, 0),
                    (F_MAXHEALTH, 100),
                    (F_DYNAMIC_FLAGS, 0x1),
                    (F_COMBAT_REACH, 4.0_f32.to_bits()),
                ]),
                Transform::from_xyz(0.0, 0.0, -12.0),
            ))
            .id();
        right_click_loot(
            &mut world,
            Hovered {
                target: Some(body),
                guid: Some(BODY),
                distance: 12.0,
                ..Hovered::default()
            },
        );
        let stop = world
            .resource::<crate::player::Approach>()
            .stop_distance()
            .expect("the unit walk starts");
        assert!(
            (stop - (4.0_f32 + 1.5 + 1.333_333_3).sqrt()).abs() < 1e-3,
            "{stop}"
        );
        // A corpse object has no unit to look up: reach 5.0, stop sqrt(5).
        let (mut world, _vendor, _rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
        let corpse = world
            .spawn((
                Guid(CORPSE),
                store(&[(benilla_protocol::field::FIELD_CORPSE_DYNAMIC_FLAGS, 0x1)]),
                Transform::from_xyz(0.0, 0.0, -12.0),
            ))
            .id();
        right_click_loot(
            &mut world,
            Hovered {
                corpse: Some(corpse),
                corpse_guid: Some(CORPSE),
                distance: 12.0,
                ..Hovered::default()
            },
        );
        let stop = world
            .resource::<crate::player::Approach>()
            .stop_distance()
            .expect("the corpse walk starts");
        assert!((stop - 5.0_f32.sqrt()).abs() < 1e-5, "{stop}");
    }

    /// `0x5f86b0` → `0x610300`: a far object's use walks first, its range read off the object.
    #[test]
    fn a_far_object_is_walked_to_before_its_use() {
        let (mut world, _vendor, rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
        let chest = spawn_chest(&mut world, Vec3::new(0.0, 0.0, -20.0));
        *world.resource_mut::<PressPick>() = PressPick {
            object: HoveredObject {
                target: Some(chest),
                guid: Some(CHEST),
                distance: 20.0,
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Interact,
                unable: true,
            },
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
        assert!(world.resource::<crate::player::Approach>().active());
        assert!(!rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::GameObjUse { .. })));
    }

    /// An arrival short of the object's range walks again, as `0x5f86b0`'s range arm does.
    #[test]
    fn an_object_still_out_of_range_on_arrival_is_walked_to_again() {
        let (mut world, _vendor, rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
        spawn_chest(&mut world, Vec3::new(0.0, 0.0, -20.0));
        world.resource_mut::<crate::player::Approach>().arrived =
            Some((crate::player::ApproachVerb::Use, CHEST));
        world.run_system_once(act_on_arrival).unwrap();
        assert!(world.resource::<crate::player::Approach>().active());
        assert!(!rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::GameObjUse { .. })));
    }

    /// The arm's leash (`0x6110a0`): at 80 yd "Target is too far away." and no walk.
    #[test]
    fn a_vendor_80_yards_off_is_too_far_to_walk_to() {
        let (mut world, vendor, _rx) = walking_world(Vec3::new(80.0, 0.0, 0.0), true);
        click_vendor(&mut world, vendor);
        assert!(!world.resource::<crate::player::Approach>().active());
        let keys: Vec<&str> = world
            .resource::<crate::ui_action::UiErrorKeys>()
            .snapshot()
            .iter()
            .map(|e| e.key)
            .collect();
        assert_eq!(keys, vec!["ERR_AUTOFOLLOW_TOO_FAR"]);
    }

    /// Out of interact range the click sends nothing and raises `ERR_USE_TOO_FAR` (`0x5f874b`),
    /// the stone's own use slot (`0x5f69d0`) never running.
    #[test]
    fn an_out_of_range_meeting_stone_click_sends_nothing() {
        const STONE: u64 = 0x5702;
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        let (mut world, _boar) = right_click_world();
        world.insert_resource(NetCommands(tx));
        let stone = world
            .spawn((Guid(STONE), store(&[(GO_TYPE_FIELD, 23)])))
            .id();
        *world.resource_mut::<PressPick>() = PressPick {
            object: HoveredObject {
                target: Some(stone),
                guid: Some(STONE),
                distance: 50.0,
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Interact,
                unable: true,
            },
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
        assert!(world
            .resource::<Messages<crate::ui_dialog_verbs::MeetingStoneUse>>()
            .iter_current_update_messages()
            .next()
            .is_none());
        assert!(rx.try_iter().next().is_none());
        let keys: Vec<&str> = world
            .resource::<crate::ui_action::UiErrorKeys>()
            .snapshot()
            .iter()
            .map(|e| e.key)
            .collect();
        assert_eq!(keys, vec!["ERR_USE_TOO_FAR"]);
    }

    /// `0x5f86b0`: the usable test (`0x5f3130`) refuses out of range with `0xe1`, and with the
    /// option off the auto-walk (`0x610300`) is false, so `0x5f874b` raises `ERR_USE_TOO_FAR`: for
    /// the shared arms and the mailbox, text and stone, whose slot `+0x18` is the same test.
    #[test]
    fn a_far_object_click_is_too_far_with_click_to_move_off() {
        for go_type in [3, 9, 19, 23] {
            let (mut world, _vendor, rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), false);
            let object = spawn_object(&mut world, go_type, Vec3::new(0.0, 0.0, -20.0));
            let keys = click_far_object(&mut world, object, 20.0);
            assert_eq!(keys, vec!["ERR_USE_TOO_FAR"], "type {go_type}");
            assert!(!world.resource::<crate::player::Approach>().active());
            assert!(rx.try_iter().next().is_none(), "type {go_type}");
        }
    }

    /// The arm's leash (`0x6110a0`) refuses the walk with `0x126`, and `0x610300` returns false, so
    /// `0xe1` follows.
    #[test]
    fn a_leashed_far_object_click_raises_both_errors_in_order() {
        let (mut world, _vendor, _rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
        let chest = spawn_chest(&mut world, Vec3::new(0.0, 0.0, -90.0));
        let keys = click_far_object(&mut world, chest, 90.0);
        assert_eq!(keys, vec!["ERR_AUTOFOLLOW_TOO_FAR", "ERR_USE_TOO_FAR"]);
        assert!(!world.resource::<crate::player::Approach>().active());
    }

    /// A walk that starts raises nothing.
    #[test]
    fn a_walkable_far_object_click_raises_no_error() {
        let (mut world, _vendor, _rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
        let chest = spawn_chest(&mut world, Vec3::new(0.0, 0.0, -20.0));
        let keys = click_far_object(&mut world, chest, 20.0);
        assert!(keys.is_empty(), "{keys:?}");
        assert!(world.resource::<crate::player::Approach>().active());
    }

    /// A fishing node, out of its 100 yd, never takes the walk (`0x5f8705`) and goes straight to
    /// `0x5f874b`.
    #[test]
    fn a_far_fishing_node_is_too_far_even_with_click_to_move_on() {
        let (mut world, _vendor, _rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
        let node = spawn_object(
            &mut world,
            cursor_mode::GO_TYPE_FISHINGNODE as u32,
            Vec3::new(0.0, 0.0, -120.0),
        );
        let keys = click_far_object(&mut world, node, 120.0);
        assert_eq!(keys, vec!["ERR_USE_TOO_FAR"]);
        assert!(!world.resource::<crate::player::Approach>().active());
    }

    fn right_click_world_at(world: &mut World, occlusion: PickOcclusion) {
        *world.resource_mut::<PressPick>() = PressPick {
            occlusion,
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
    }

    /// `0x5e0378`: a right-click on the ground walks there through `CanAutoInteract`, sending
    /// nothing and keeping the selection; a held payload is dropped (`0x492ca9`) and the walk
    /// still starts.
    #[test]
    fn a_ground_right_click_walks_there_only_with_click_to_move_on() {
        for (walk, held) in [(true, false), (true, true), (false, false)] {
            let (mut world, vendor, rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), walk);
            world
                .resource_mut::<crate::ui_script::CursorPayloadHeld>()
                .0 = held;
            // `0x5e0320` returns without deselecting: the vendor stays the target.
            let vendor_guid = world.get::<Guid>(vendor).unwrap().0;
            let mut selection = world.resource_mut::<Selection>();
            selection.target = Some(vendor);
            selection.guid = Some(vendor_guid);
            right_click_world_at(
                &mut world,
                PickOcclusion {
                    distance: 10.0,
                    point: Some(Vec3::new(0.0, 0.0, -10.0)),
                    ..PickOcclusion::default()
                },
            );
            let approach = world.resource::<crate::player::Approach>();
            assert_eq!(approach.active(), walk, "AutoInteract {walk}, held {held}");
            if walk {
                assert!((approach.stop_distance().unwrap() - 0.5).abs() < 1e-6);
            }
            assert!(rx.try_iter().next().is_none());
            assert_eq!(world.resource::<Selection>().guid, Some(vendor_guid));
        }
    }

    fn attack_wolf_at(at: Vec3, reach: f32) -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        const F_COMBAT_REACH: u16 = 130;
        let (mut world, _vendor, rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
        let wolf = world
            .spawn((
                Guid(WOLF),
                store(&[
                    (F_HEALTH, 100),
                    (F_MAXHEALTH, 100),
                    (F_COMBAT_REACH, reach.to_bits()),
                ]),
                Transform::from_translation(at),
            ))
            .id();
        *world.resource_mut::<PressPick>() = PressPick {
            hovered: Hovered {
                target: Some(wolf),
                guid: Some(WOLF),
                distance: at.length(),
                ..Hovered::default()
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Attack,
                unable: false,
            },
            attack_fork: cursor_mode::AttackFork(true),
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
        (world, rx)
    }

    /// `0x60c22b` → `0x60fcc0`: out of melee the walk arms before the swing, stopping at
    /// `sqrt(max(2·own reach + 1.3333, 5) − 1.3333)`, the enemy's own reach not counted
    /// (`0x60fe5f`, `0x6112b6`, `0x61131d`, `0x6111ab`).
    #[test]
    fn an_enemy_out_of_melee_is_walked_into_before_the_swing() {
        let (world, rx) = attack_wolf_at(Vec3::new(0.0, 0.0, -12.0), 4.0);
        let stop = world
            .resource::<crate::player::Approach>()
            .stop_distance()
            .expect("the melee walk starts");
        assert!(
            (stop - (5.0_f32 - 1.333_333_3).sqrt()).abs() < 1e-3,
            "{stop}"
        );
        assert!(rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::AttackSwing { guid: WOLF })));
    }

    /// Inside `max(rA + rB + 1.3333, 5)` the attack turns to face the enemy where it stands
    /// (`0x60fd80`) and swings.
    #[test]
    fn an_enemy_in_melee_is_faced_without_a_walk() {
        let (world, rx) = attack_wolf_at(Vec3::new(0.0, 0.0, -6.0), 4.0);
        assert!(world.resource::<crate::player::Approach>().facing());
        assert!(rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::AttackSwing { guid: WOLF })));
    }

    /// The mounted gate (`0x60c1bc`) skips the walk and the swing alike.
    #[test]
    fn a_mounted_attack_does_not_walk() {
        const F_MOUNT: u16 = benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID;
        let (mut world, _vendor, rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(&world)
            .unwrap();
        world.entity_mut(me).insert(store(&[
            (F_HEALTH, 100),
            (F_MAXHEALTH, 100),
            (F_MOUNT, 14_337),
        ]));
        let wolf = world
            .spawn((
                Guid(WOLF),
                store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100)]),
                Transform::from_xyz(0.0, 0.0, -12.0),
            ))
            .id();
        *world.resource_mut::<PressPick>() = PressPick {
            hovered: Hovered {
                target: Some(wolf),
                guid: Some(WOLF),
                distance: 12.0,
                ..Hovered::default()
            },
            attack_fork: cursor_mode::AttackFork(true),
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
        assert!(!world.resource::<crate::player::Approach>().active());
        assert!(
            !rx.try_iter()
                .any(|c| matches!(c, ClientCommand::AttackSwing { .. })),
            "the mounted gate skips StartAttack too"
        );
    }

    /// The melee row's 80 yd leash (`0x60e652`) refuses with `ERR_AUTOFOLLOW_TOO_FAR` (`0x61110c`);
    /// the swing still goes out (`0x60c247`).
    #[test]
    fn an_enemy_80_yards_off_is_too_far_to_walk_into() {
        let (world, rx) = attack_wolf_at(Vec3::new(0.0, 0.0, -85.0), 1.5);
        assert!(!world.resource::<crate::player::Approach>().active());
        assert_eq!(
            world.resource::<crate::ui_action::UiErrorKeys>().snapshot(),
            vec![crate::ui_action::UiError::key("ERR_AUTOFOLLOW_TOO_FAR")]
        );
        assert!(rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::AttackSwing { guid: WOLF })));
    }

    /// `0x492d30`: a right-click on nothing walks along the ray, but not with a payload held
    /// (`0x492d50`), which that click only drops.
    #[test]
    fn a_sky_right_click_walks_along_the_ray_with_an_empty_cursor() {
        for held in [false, true] {
            let (mut world, _vendor, _rx) = walking_world(Vec3::new(14.0, 0.0, 0.0), true);
            world
                .resource_mut::<crate::ui_script::CursorPayloadHeld>()
                .0 = held;
            right_click_world_at(
                &mut world,
                PickOcclusion {
                    ray: Dir3::new(Vec3::new(0.0, 0.3, -1.0)).ok(),
                    ..PickOcclusion::default()
                },
            );
            assert_eq!(
                world.resource::<crate::player::Approach>().active(),
                !held,
                "payload held {held}"
            );
        }
    }

    const BODY: u64 = 0xB0D7;
    const CORPSE: u64 = 0xC0D5;

    /// A dead lootable unit and a lootable corpse `at` yd off and the vendor in reach, with our last
    /// streamed movement word `flags` and Click to Move set as `walk`.
    fn loot_world(
        at: f32,
        flags: u32,
        walk: bool,
    ) -> (
        World,
        Entity,
        Entity,
        crossbeam_channel::Receiver<ClientCommand>,
    ) {
        const F_DYNAMIC_FLAGS: u16 = 143;
        let (mut world, _vendor, rx) = walking_world(Vec3::new(2.5, 0.0, 0.0), walk);
        world.insert_resource(crate::player::Player::with_move_flags(flags));
        let body = world
            .spawn((
                Guid(BODY),
                store(&[(F_HEALTH, 0), (F_MAXHEALTH, 100), (F_DYNAMIC_FLAGS, 0x1)]),
                Transform::from_xyz(0.0, 0.0, -at),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(BODY, body);
        let corpse = world
            .spawn((
                Guid(CORPSE),
                store(&[(benilla_protocol::field::FIELD_CORPSE_DYNAMIC_FLAGS, 0x1)]),
                Transform::from_xyz(0.0, 0.0, at),
            ))
            .id();
        (world, body, corpse, rx)
    }

    fn right_click_loot(world: &mut World, hovered: Hovered) {
        *world.resource_mut::<PressPick>() = PressPick {
            hovered,
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Pickup,
                unable: false,
            },
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
    }

    /// The press over the corpse when `on_corpse`, else over the dead unit.
    fn loot_hover(on_corpse: bool, body: Entity, corpse: Entity, at: f32) -> Hovered {
        let (unit, object) = if on_corpse {
            (None, Some(corpse))
        } else {
            (Some(body), None)
        };
        Hovered {
            target: unit,
            guid: unit.map(|_| BODY),
            corpse: object,
            corpse_guid: object.map(|_| CORPSE),
            distance: at,
            ..Hovered::default()
        }
    }

    /// `0x5df2e9` and `0x5df173`: both loot senders return silently on any `0x20ff` bit, a
    /// direction, turn, pitch or fall; swimming and walk mode are outside the mask.
    #[test]
    fn a_loot_click_while_moving_sends_nothing() {
        use crate::creature_anim::move_flags as f;
        for (flags, loots) in [
            (0, true),
            (f::FORWARD, false),
            (f::BACKWARD, false),
            (f::STRAFE_LEFT, false),
            (f::TURN_RIGHT, false),
            (f::FALLING, false),
            (f::FORWARD | f::FALLING, false),
            (f::SWIMMING, true),
            (f::WALK_MODE, true),
        ] {
            for on_corpse in [false, true] {
                let (mut world, body, corpse, rx) = loot_world(2.0, flags, false);
                right_click_loot(&mut world, loot_hover(on_corpse, body, corpse, 2.0));
                let looted = rx
                    .try_iter()
                    .any(|c| matches!(c, ClientCommand::Loot { .. }));
                let latched = world.resource::<crate::ui_loot::LootLatch>().0.is_some();
                assert_eq!(
                    (looted, latched),
                    (loots, loots),
                    "flags {flags:#x}, corpse {on_corpse}"
                );
            }
        }
    }

    /// The movement test comes before the walk (`0x5df2f6`, `0x5df180`): a far body or corpse
    /// clicked on the run starts no Click to Move walk.
    #[test]
    fn a_far_loot_click_while_moving_starts_no_walk() {
        use crate::creature_anim::move_flags as f;
        for (flags, walks) in [(0, true), (f::FORWARD, false), (f::FALLING, false)] {
            for on_corpse in [false, true] {
                let (mut world, body, corpse, rx) = loot_world(12.0, flags, true);
                right_click_loot(&mut world, loot_hover(on_corpse, body, corpse, 12.0));
                assert_eq!(
                    world.resource::<crate::player::Approach>().active(),
                    walks,
                    "flags {flags:#x}, corpse {on_corpse}"
                );
                assert!(!rx
                    .try_iter()
                    .any(|c| matches!(c, ClientCommand::Loot { .. })));
            }
        }
    }

    /// `0x60e352`: the owed verb (`0x60fa20`) runs only once no `0x20ff` bit is left, and the cell
    /// stays armed until then.
    #[test]
    fn a_verb_owed_on_arrival_waits_for_a_word_with_no_move_bit() {
        use crate::creature_anim::move_flags as f;
        use crate::player::ApproachVerb;
        for (verb, guid) in [(ApproachVerb::Loot, BODY), (ApproachVerb::Talk, VENDOR)] {
            let (mut world, _body, _corpse, rx) = loot_world(2.0, f::FALLING, true);
            world.resource_mut::<crate::player::Approach>().arrived = Some((verb, guid));
            world.run_system_once(act_on_arrival).unwrap();
            assert_eq!(rx.try_iter().count(), 0, "{verb:?} mid-air");
            assert_eq!(
                world.resource::<crate::player::Approach>().arrived,
                Some((verb, guid)),
                "{verb:?} stays owed"
            );
            world.insert_resource(crate::player::Player::with_move_flags(0));
            world.run_system_once(act_on_arrival).unwrap();
            let sent: Vec<ClientCommand> = rx.try_iter().collect();
            assert!(
                match verb {
                    ApproachVerb::Loot =>
                        matches!(sent.as_slice(), [ClientCommand::Loot { guid: BODY }]),
                    _ => matches!(
                        sent.as_slice(),
                        [ClientCommand::ListInventory { guid: VENDOR }]
                    ),
                },
                "{verb:?} landed: {sent:?}"
            );
            assert!(world
                .resource::<crate::player::Approach>()
                .arrived
                .is_none());
        }
    }
}
