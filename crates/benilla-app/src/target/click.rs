//! The click router: a clean left-click selects, a clean right-click takes the context action,
//! and the UI's selection asks and Esc commit into the same [`super::Selection`].

use benilla_ui::script::SelectionRequest;

use super::lock::GoLockInputs;
use super::*;

/// The right-click payload leg of the reference's click router (`0x481f60`, clean clicks only,
/// `0x514ae0`): over terrain (`0x492c90`) or nothing (`0x492d30`) any payload clears, silently.
/// Over a world object this keeps every payload; the reference's object leg `0x492ce0` there
/// clears one that sets `[0xb4b41c]` (mode 5 vendor row, mode 7 bar item, mode 9 ammo).
pub(super) fn world_right_click_payload(
    mut right_clicks: MessageReader<WorldRightClick>,
    // The press pick: this leg and [`act_on_right_click`] must classify the same pick.
    press: Res<PressPick>,
    script: Option<NonSendMut<UiScript>>,
) {
    if right_clicks.read().last().is_none() {
        return;
    }
    let Some(mut script) = script else {
        return;
    };
    if press.hovered.target.is_some() || press.object.target.is_some() {
        return;
    }
    script.clear_cursor_payload();
}

/// A plate click is a click on its unit (the click slot `0x7cb910`). The left button carries the
/// plate's unit, as a scripted `plate:Click()` (pfUI, `nameplates.lua:1228`) has no cursor; the
/// right replays [`WorldRightClick`] only when the press pick names it, so a scripted one does
/// nothing yet. A physical right-click arrives only here: `0x7cb910` → `0x4949f0` → `0x492820`.
pub(super) fn select_on_plate_click(
    mut plate: ResMut<crate::vplates::PlateClicks>,
    press: Res<PressPick>,
    ground: Res<crate::spell::SpellTargeting>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    self_q: Query<(&Guid, Has<Engaged>), With<SelfPlayer>>,
    mut greeting: MessageWriter<crate::sound::NpcGreetingRequest>,
    mut right_clicks: MessageWriter<WorldRightClick>,
    units: Query<(&Guid, Option<&ObjectStore>)>,
) {
    let (left, right) = (
        std::mem::take(&mut plate.left),
        std::mem::take(&mut plate.right),
    );
    if ground.active() {
        return;
    }
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
    if right.into_iter().any(|e| press.hovered.target == Some(e)) {
        right_clicks.write(WorldRightClick);
    }
}

/// On a [`WorldClick`], select the press pick's unit or deselect on empty world, except on nothing
/// (the sky) with a payload held (`0x492d30`); the terrain leg deselects regardless (`0x5e03bb`).
/// The press pick, since the hover is empty through a camera drag, as in the reference's freelook.
pub(super) fn select_on_click(
    mut clicks: MessageReader<WorldClick>,
    inspect: Res<InspectMode>,
    press: Res<PressPick>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    self_q: Query<(&Guid, Has<Engaged>), With<SelfPlayer>>,
    payload_held: Res<crate::ui_script::CursorPayloadHeld>,
    mut greeting: MessageWriter<crate::sound::NpcGreetingRequest>,
    ground: Res<crate::spell::SpellTargeting>,
    click_cfg: Res<ClickConfig>,
    // Read at the commit, where the reference resolves it for `0x493540`'s `IsSelectable` gate.
    stores: Query<&ObjectStore>,
) {
    let (hovered, occlusion) = (press.hovered, press.occlusion);
    let clicked = clicks.read().last().is_some();
    if !clicked || inspect.enabled {
        return;
    }
    // The ground cursor owns the click (`0x492580`); its commit system runs after this one.
    if ground.active() {
        return;
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
                clear(&mut selection, &mut seam, engaged);
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
    /// The cursor, for the vendor arm's sell fork (`0x5df5d7`); `None` when no VM is mounted.
    pub(crate) script: Option<NonSendMut<'w, benilla_ui::script::UiScript>>,
}

/// The re-click gate (`0x5f0251`), before the ladder: a right-click on the NPC whose window is
/// open (`[0xb4e2d0]`) does nothing, no packet, no error, no gesture. The reference arms that NPC
/// only from window openers (`0x4930d0`, cleared at `0x493310`), never on the click, the set
/// [`crate::ui_session::feed_interact_npc`] collapses (less the mailbox and text reader).
fn interaction_already_open_on(target: u64, interact: &crate::ui_session::InteractNpc) -> bool {
    interact.1 == Some(target)
}

/// On a clean right-click, select the press pick's unit and act by the cursor's classification
/// (the INTERACT leg `0x492820`): a GameObject, a corpse, an attack, loot, skin, or the
/// `UNIT_NPC_FLAGS` ladder ([`service_arm`]). The range gray (`unable`) suppresses every send but
/// attack, which the server holds until in reach; there is no auto-approach yet.
// `ui_feedback` is the overflow bundle past the 16-SystemParam ceiling.
#[allow(clippy::type_complexity)]
pub(super) fn act_on_right_click(
    mut clicks: MessageReader<WorldRightClick>,
    // The press pick: the reference picks once, on the down edge (`0x481f00`).
    press: Res<PressPick>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    self_player: Query<(Entity, &Guid, Has<Engaged>), With<SelfPlayer>>,
    // Through `creature_anim::gesture`, the chat path's entry, as the reference's dispatcher does.
    mut gestures: ResMut<crate::creature_anim::GestureQueue>,
    mut go_inputs: GoLockInputs,
    player_actions: Res<crate::ui_action::PlayerActions>,
    // The skin leg's spell (`[0xb700e4]`), already gated by the classifier.
    learned: Res<crate::ui_action::LearnedAbilities>,
    // The GameObject leg also reads the object's anim state (the Action gate).
    stores: Query<(&ObjectStore, Option<&crate::go_anim::GoAnim>)>,
    mut service: ServiceArms,
    ui_feedback: (
        ResMut<crate::ui_action::UiErrorKeys>,
        ResMut<crate::ui_action::CastErrors>,
        ResMut<crate::ui_mail::MailOpen>,
        ResMut<crate::ui_item_text::ItemTextOpen>,
        // The opener queue: this system cannot also hold `CastLadder` (a second `Items` and
        // `CastErrors` borrow), so the lock verdict goes to `ui_action::drain::drain_go_openers`.
        ResMut<crate::ui_action::GoOpenerCasts>,
        // The stone's refusals need the roster, so they run in `drain_meeting_stone_joins`.
        MessageWriter<crate::ui_dialog_verbs::MeetingStoneUse>,
    ),
) {
    let (mut ui_error_keys, mut cast_errors, mut mail, mut item_text, mut openers, mut stone_uses) =
        ui_feedback;
    if clicks.read().last().is_none() {
        return;
    }
    let (hovered, hovered_object, cursor) = (&press.hovered, &press.object, &press.cursor);
    // The reference's one mounted predicate, the player's `UNIT_FIELD_MOUNTDISPLAYID`.
    let self_store = self_player
        .single()
        .ok()
        .and_then(|(e, _, _)| stores.get(e).ok())
        .map(|(s, _)| s);
    let self_mounted = self_store.is_some_and(|s| s.0.unit_mount_display_id() != 0);
    // A GameObject is used, never falling through to units: `OnUse 0x5f8660` gates on
    // highlightable (our `Point` cursor, silent), then usable (`0x5f3130`), whose lock arm toasts
    // first, even out of range. Out of range (`unable`) this shows nothing; the reference shows
    // `ERR_USE_TOO_FAR` (`0x5f874b`) unless its auto-walk `0x610300` starts (`AutoInteract` on).
    if go_is_nearest(hovered, hovered_object) {
        // Traced (tag `use`): a `Point` cursor and the range gray both refuse silently here.
        if benilla_assets::trace::enabled_for("use") {
            let ty = hovered_object
                .target
                .and_then(|e| stores.get(e).ok())
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
            if let Some(guid) = hovered_object.guid {
                let go = hovered_object
                    .target
                    .and_then(|e| stores.get(e).ok())
                    .map(|(s, anim)| (s, crate::go_anim::go_state(anim, s)));
                // The GameObject's own mounted gate (`0x5f31a8`) returns before the opener: no
                // packet, no cast. It applies only without a Lock.dbc row (`0x5f8180`, a pointer
                // test, so an all-empty row counts, unlike `LockCatalog::is_locked`); a locked
                // object is refused by its opener cast. Only MAILBOX is exempt (`0x5f31bb`).
                // Silent: the key has no text or sound, and `0x4945b0` drops the empty string.
                let lock_id = go_inputs.templates.get(guid).map_or(0, |t| t.lock_id);
                let has_lock_row = lock_id != 0
                    && go_inputs
                        .locks
                        .as_deref()
                        .is_some_and(|l| l.0.slots(lock_id).is_some());
                let go_type = go.map_or(-1, |(s, _)| s.0.gameobject_type_id());
                if self_mounted && !has_lock_row && go_type != cursor_mode::GO_TYPE_MAILBOX {
                    debug!(
                        "right-click gameobject {guid:#x}: refused, mounted (lock-less type {go_type}, silent)"
                    );
                    return;
                }
                // MAILBOX (type 19) opens locally: its use handler `0x5f6820` sends no
                // `CMSG_GAMEOBJ_USE`, and `MAIL_SHOW` → `CheckInbox` asks for the list.
                if go.is_some_and(|(s, _)| s.0.gameobject_type_id() == cursor_mode::GO_TYPE_MAILBOX)
                {
                    if !cursor.unable {
                        debug!("right-click mailbox: open mail window {guid:#x}");
                        mail.click(guid);
                    }
                    return;
                }
                // TEXT (type 9) opens locally (`0x5f58c0` → `0x4e32e0(goGuid, 0)`; vmangos' `Use`
                // has no type-9 case); a re-click closes it, and the page is read at paint time.
                if go.is_some_and(|(s, _)| s.0.gameobject_type_id() == cursor_mode::GO_TYPE_TEXT) {
                    if !cursor.unable {
                        if item_text.toggle_closed(guid) {
                            debug!("right-click text gameobject: re-click closes {guid:#x}");
                        } else {
                            debug!("right-click text gameobject: read {guid:#x}");
                            item_text.open_pages(guid);
                        }
                    }
                    return;
                }
                // MEETINGSTONE (type 23) has its own use slot `0x5f69d0`: four client-side
                // refusals, then `CMSG 0x292 {u64 goGuid}` (`0x4c9ff0`), never `CMSG_GAMEOBJ_USE`,
                // whose type-23 arm in vmangos does nothing (`GameObject.cpp:1836`). Types 9, 19,
                // 23 and 28 override the shared sender `0x5f33e0`.
                if go.is_some_and(|(s, _)| {
                    s.0.gameobject_type_id() == cursor_mode::GO_TYPE_MEETINGSTONE
                }) {
                    if !cursor.unable {
                        debug!("right-click meeting stone: {guid:#x}");
                        stone_uses.write(crate::ui_dialog_verbs::MeetingStoneUse { go_guid: guid });
                    }
                    return;
                }
                // By lock: used, opened by its opener cast, or refused locally (`0x5f3427..`).
                match resolve_go_action(
                    guid,
                    &mut go_inputs,
                    &player_actions.spells,
                    go,
                    self_store,
                    &seam.net,
                ) {
                    GoAction::Use if cursor.unable => {}
                    GoAction::OpenLock(_) | GoAction::OpenByKey(_) if cursor.unable => {}
                    GoAction::Use => {
                        debug!("right-click gameobject use: {guid:#x}");
                        if benilla_assets::trace::enabled_for("use") {
                            benilla_assets::trace::line(
                                "use",
                                &format!("SEND CMSG_GAMEOBJ_USE guid={guid:#x}"),
                            );
                        }
                        let _ = seam.net.0.send(ClientCommand::GameObjUse { guid });
                    }
                    // Both opener arms queue for the one cast path, as the reference reaches
                    // `TryCast 0x6e4b60` from the use sender (`0x5f35c0`) like a button press.
                    GoAction::OpenLock(spell_id) => {
                        debug!("right-click gameobject open-lock: cast {spell_id} at {guid:#x}");
                        openers.0.push(crate::ui_action::GoOpener::Spell {
                            spell_id,
                            go_guid: guid,
                        });
                    }
                    GoAction::OpenByKey(it) => {
                        debug!(
                            "right-click gameobject open-by-key: use item ({},{}) blk {} at {guid:#x}",
                            it.bag_index, it.slot, it.spell_index
                        );
                        openers.0.push(crate::ui_action::GoOpener::Key(it));
                    }
                    GoAction::Refuse(err) => {
                        debug!("right-click gameobject {guid:#x}: locked, refused ({err:?})");
                        if let Some(err) = err {
                            ui_error_keys.0.push(err);
                        }
                    }
                }
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
        let store = stores.get(entity).ok().map(|(s, _)| s);
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
        // A rider fails leg 1 (`0x5d6c2a jg`) with no error and falls to leg 2, silently.
        if !self_mounted && store.is_some_and(|s| s.0.corpse_lootable()) {
            // Not standing: the client-local red `ERR_LOOT_NOTSTANDING`, no packet (`0x5d6c3b` →
            // `GetStandState 0x5ed570`, non-zero → `0x496720(0x85)`).
            let standing = self_store.is_none_or(|s| s.0.unit_stand_state() == 0);
            if !standing {
                debug!("right-click corpse loot: refused, not standing ({guid:#x})");
                ui_error_keys
                    .0
                    .push(crate::ui_action::UiError::key("ERR_LOOT_NOTSTANDING"));
                return;
            }
            // Range rides the cursor's gray, so the pouch is never lit where the click refuses.
            if !cursor.unable {
                debug!("right-click corpse loot: {guid:#x}");
                let _ = seam.net.0.send(ClientCommand::Loot { guid });
                // Predicted, as for a unit corpse (`[player+0x1d28]` armed at the send).
                seam.loot_latch.0 = Some(guid);
            }
        } else if store.is_some_and(|s| s.0.corpse_pvp_insignia()) {
            // Leg 2. `skin_player_corpse` mirrors `[0xb700e8]`, `None` for every 1.12.1 player,
            // so this is inert as in the reference; the unfriendly test is not built.
            if let Some(spell_id) = learned.skin_player_corpse {
                if !cursor.unable {
                    // The cast's mounted check, where a mounted click lands; the record comes off
                    // [`GoLockInputs`]' catalog, this system's only `Spells` handle.
                    let def = go_inputs
                        .spells
                        .as_ref()
                        .and_then(|s| s.catalog.get(spell_id));
                    if crate::spell::validator::cast_mounted_refusal(self_mounted, def) {
                        debug!("right-click corpse insignia: refused locally — mounted (0x39)");
                        cast_errors.push_local(spell_id, 0x39);
                    } else {
                        debug!("right-click corpse insignia: {guid:#x} (spell {spell_id})");
                        let _ = seam.net.0.send(ClientCommand::CastSpell {
                            spell_id,
                            target: Some(guid),
                        });
                    }
                }
            }
        }
        return;
    }
    let (Some(entity), Some(guid)) = (hovered.target, hovered.guid) else {
        return;
    };
    let attack = cursor.kind == cursor_mode::CursorKind::Attack;
    let target = stores.get(entity).ok().map(|(s, _)| s);
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
        learned.skinning.is_some(),
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
    let me = self_player.single().ok();
    // A mid-combat click on a vendor or corpse switches and stops, never swings (`0x5ecb70`).
    let outcome = scan::commit(
        &mut selection,
        &mut seam,
        entity,
        guid,
        target,
        me.is_some_and(|(_, _, e)| e),
        me.map(|(_, g, _)| g.0),
        attack,
    );
    match unit_branch(attack, dead_fork, leg) {
        UnitBranch::Attack => {
            // Silent after the select: the click's `0x60c247 call 0x5ecb70` has no `DisplayError`,
            // and the red `ERR_ATTACK_*` lines are `0x612df0`'s (the Attack action, pet attack,
            // TryCast). The predicate is `0x612df0`'s; `0x5ecb70`'s own set is not transcribed.
            if crate::ui_action::attack_actor_blocked(self_store, me.map(|(_, g, _)| g.0)).is_some()
            {
                // refused: the selection stands, no swing, nothing said
            } else {
                debug!("right-click attack: {guid:#x}");
                // `0x5ecb70`'s body through the seam; `swung` means the commit's re-swing already
                // went out, which `0x5eccda` keeps from sending twice.
                let engaged = me.is_some_and(|(_, _, e)| e);
                seam.start(guid, engaged || outcome.swung, false);
            }
        }
        UnitBranch::Dead(DeadUnitLeg::Loot) => {
            // `CMSG_LOOT`, no talk gesture; the stand-state check (`0x60bfb7` → `0x60c007 push
            // 0x85`) matches the corpse leg's.
            if self_store.is_some_and(|s| s.0.unit_stand_state() != 0) {
                debug!("right-click loot: refused, not standing ({guid:#x})");
                ui_error_keys
                    .0
                    .push(crate::ui_action::UiError::key("ERR_LOOT_NOTSTANDING"));
            } else if !cursor.unable {
                debug!("right-click loot: {guid:#x}");
                let _ = seam.net.0.send(ClientCommand::Loot { guid });
                // Predicted at the send, as the reference's sender (`0x5df253`) arms
                // `[player+0x1d28]` and kneels before any reply; the anim driver reads the latch.
                seam.loot_latch.0 = Some(guid);
            }
        }
        UnitBranch::Dead(DeadUnitLeg::Skin) => {
            // The skin leg (`0x60c01f`): the known Skinning spell (`[0xb700e4]`) at a corpse the
            // loot leg declined, lootable ones included while mounted. Its cast mounted check
            // (`0x6094f0`, reason `0x39`) is the one place a rider is told anything.
            if !cursor.unable {
                if let Some(spell_id) = learned.skinning {
                    let def = go_inputs
                        .spells
                        .as_ref()
                        .and_then(|s| s.catalog.get(spell_id));
                    if crate::spell::validator::cast_mounted_refusal(self_mounted, def) {
                        debug!("right-click skin: refused locally — mounted (0x39)");
                        cast_errors.push_local(spell_id, 0x39);
                    } else {
                        debug!("right-click skin: {guid:#x} (spell {spell_id})");
                        let _ = seam.net.0.send(ClientCommand::CastSpell {
                            spell_id,
                            target: Some(guid),
                        });
                    }
                }
            }
        }
        // Terminal: a dead unit that took no leg does nothing; only the alive branch (`0x60c162`)
        // reaches the service dispatch.
        UnitBranch::Dead(DeadUnitLeg::Nothing) => {
            debug!("right-click unit {guid:#x}: dead fork took no leg — nothing sent");
        }
        UnitBranch::Service if !cursor.unable => {
            // The reference's own `UNIT_NPC_FLAGS` ladder (`0x5f0289`), not the cursor kind, which
            // its cursor ladder (`0x482336`) projects lossily; the re-click gate comes first.
            if interaction_already_open_on(guid, &service.interact) {
                debug!(
                    "right-click interact: {guid:#x} — its window is already open, nothing sent"
                );
                return;
            }
            let npc_flags = stores
                .get(entity)
                .map(|(s, _)| s.0.unit_npc_flags())
                .unwrap_or(0);
            // Peeked, not taken: the cursor clears only once a sale goes out. The reference sends
            // the cursor's stored guid; we resolve the held `(bag, slot)`, which a pickup locks,
            // and a slot with no guid opens the list instead.
            let cursor_sale = service
                .script
                .as_deref()
                .and_then(|s| s.cursor_item())
                .and_then(|item| {
                    let slot0 = u8::try_from(item.slot.saturating_sub(1)).unwrap_or(0);
                    self_store.and_then(|s| {
                        crate::ui_items::slot_guid(&s.0, item.bag, slot0, &go_inputs.objects)
                    })
                });
            let Some(arm) = service_arm(npc_flags, service.quest.status(guid)) else {
                // `0x5f05ca`: no consulted bit does nothing, gesture included.
                debug!("right-click interact: {guid:#x} matches no service bit — nothing sent");
                return;
            };
            match service_action(
                arm,
                guid,
                self_store.is_some_and(|s| s.0.player_is_ghost()),
                cursor_sale,
            ) {
                ServiceAction::Send(cmd) => {
                    debug!("right-click interact: {guid:#x} ({arm:?})");
                    let _ = seam.net.0.send(cmd);
                }
                ServiceAction::SellFromCursor(cmd) => {
                    debug!("right-click interact: {guid:#x} (vendor — selling the held item)");
                    let _ = seam.net.0.send(cmd);
                    // The sell clear: the slot stays greyed until the server's update.
                    if let Some(script) = service.script.as_deref_mut() {
                        script.take_cursor_item_for_sale();
                    }
                }
                ServiceAction::AskBinder => {
                    debug!(
                        "right-click interact: {guid:#x} (innkeeper — CONFIRM_BINDER, no packet)"
                    );
                    service.binder.ask(guid);
                }
                ServiceAction::AskSpiritHealer => {
                    debug!("right-click interact: {guid:#x} (spirit healer — CONFIRM_XP_LOSS, no packet)");
                    service.death.ask_spirit_healer(guid);
                }
                ServiceAction::AcquireSpiritGuide => {
                    debug!("right-click interact: {guid:#x} (spirit guide — adopting, 0x2E2)");
                    let outcome = service.spirit.click_guide(guid);
                    if outcome.cancel_aura {
                        if let Some(script) = service.script.as_deref_mut() {
                            script.fire_event("AREA_SPIRIT_HEALER_OUT_OF_RANGE", vec![]);
                        }
                        let _ = seam.net.0.send(ClientCommand::CancelAura {
                            spell_id: crate::ui_dialog_verbs::AREA_SPIRIT_HEALER_AURA,
                        });
                    }
                    if let Some(healer) = outcome.query {
                        let _ = seam
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
                gestures.push(my_guid.0, crate::creature_anim::Gesture::Talk);
            }
        }
        // Beyond the 5.5556 yd service range nothing is sent (no auto-approach yet).
        UnitBranch::Service => {}
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
    let Some(tmpl) = inputs.templates.get(guid) else {
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
                match inputs.items.template(slot0.index, 0, net) {
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
    let Some(tmpl) = inputs.items.template(key_entry, 0, net) else {
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

/// Drain `ClearTarget()`, the last leg of the Esc chain (`UIParent.lua:1492`), so the target drops
/// only when no cast, window or focused edit box took the press first.
pub(super) fn clear_target_requests(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
    mut guid_asks: MessageReader<DeselectGuid>,
) {
    let asked = guid_asks.read().any(|ask| selection.guid == Some(ask.0));
    let Some(mut script) = script else {
        if asked {
            clear(&mut selection, &mut seam, !engaged.is_empty());
        }
        return;
    };
    // Both drained every frame: a `||` short-circuiting on `asked` would leave the VM's
    // ClearTarget flag armed and clear the next frame's target.
    let vm_clear = script.take_target_clear();
    if asked || vm_clear {
        clear(&mut selection, &mut seam, !engaged.is_empty());
    }
}

/// A deselect that applies only if the selection is this guid (`0x493910(guid, 1)`), raised by
/// every loot close for a dead unit (`0x48f369`) and drained by [`clear_target_requests`].
#[derive(bevy::ecs::message::Message, Clone, Copy, Debug)]
pub(crate) struct DeselectGuid(pub(crate) u64);

/// Drain the UI's selection asks in order through the shared SetSelection path. `TargetUnit`
/// (`0x4899d0`), `AssistUnit` (`0x489b80`) and `TargetLastEnemy` share the reference's helper
/// `0x489a40`: a resolved unit or a roster member is committed, anything else is a no-op, never a
/// deselect. Here a token must name a streamed unit, so an out-of-range roster member is a no-op
/// (the guid-only selection is not built).
pub(super) fn selection_requests(
    script: Option<NonSendMut<UiScript>>,
    // The one unit-token resolver, shared with the reach feed so `TargetUnit("target")` and
    // `CheckInteractDistance("target", …)` name the same unit.
    tokens: crate::ui_unit::UnitTokens,
    // `TargetLastEnemy`'s memory (`[0xb4e2e8]`), stamped by `scan::remember_last_enemy`.
    last_enemy: Res<scan::LastEnemy>,
    // The SetSelection tail shared with `/target` and `/assist`, classification included.
    mut commit: super::by_name::SelectCommit,
    mut assist_by_name: MessageWriter<super::by_name::AssistRequest>,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_selection_requests();
    if requests.is_empty() {
        return;
    }
    for request in requests {
        match request {
            SelectionRequest::Unit(token) => {
                if let Some((entity, guid)) = tokens.resolve(&token, &commit.selection) {
                    commit.commit(entity, guid);
                }
            }
            // `0x489ba9 call 0x515940(token)`, then the shared tail. An unresolvable token is
            // silent here; the reference shows message `0xb8` `ERR_GENERIC_NO_TARGET` (`0x489c0e`).
            SelectionRequest::Assist(token) => match tokens.resolve(&token, &commit.selection) {
                Some((basis, _)) => commit.assist(basis, "AssistUnit"),
                None => info!("assist (AssistUnit): \"{token}\" names nothing; silent no-op"),
            },
            // `0x489c40`, `/assist <name>`: a player name, prefix-matched by the app's name scan.
            SelectionRequest::AssistByName(name) => {
                assist_by_name.write(super::by_name::AssistRequest { name: Some(name) });
            }
            // `0x489b45` → `0x489a40`. Empty memory is a no-op (the shim `0x489b40` tests nothing),
            // unlike `TargetLastTarget 0x489b00`, which reaches `0x493540(0,0)` and deselects.
            SelectionRequest::LastEnemy => {
                let Some(guid) = last_enemy.0 else {
                    info!("TargetLastEnemy: nothing hostile has been targeted yet; no-op");
                    continue;
                };
                match tokens.held(guid) {
                    Some((entity, guid)) => commit.commit(entity, guid),
                    // Stale: a no-op, and the memory stays, as the reference never clears it.
                    None => info!(
                        "TargetLastEnemy: {guid:#x} is no longer streamed; target left untouched"
                    ),
                }
            }
        }
    }
}

/// Drop the target and send `CMSG_SET_SELECTION` 0 (a no-op with none); when `engaged`, melee
/// stops too, as on the reference's Esc, click-off or target death. Weapons stay drawn.
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

        // One opener per family (a menu, a quest panel, a specialized window) arms its own NPC.
        let with_gossip = armed(&|w| {
            let mut g = crate::ui_gossip::GossipState::default();
            g.npc = Some(NPC);
            w.insert_resource(g);
        });
        let with_quest = armed(&|w| {
            let mut q = crate::ui_quest::QuestGiver::default();
            q.npc = Some(NPC);
            w.insert_resource(q);
        });
        let with_trainer = armed(&|w| {
            let mut t = crate::ui_trainer::TrainerOpen::default();
            t.open(NPC, 0, Vec::new(), String::new());
            w.insert_resource(t);
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
            let mut t = crate::ui_trainer::TrainerOpen::default();
            t.open(NPC, 0, Vec::new(), String::new());
            t.clear();
            w.insert_resource(t);
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
            world.init_resource::<crate::spell::AutoRepeatActive>();
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

    /// The selection queue from Lua, through the binding bodies of ASSISTTARGET (`AssistUnit`) and
    /// TARGETLASTHOSTILE (`TargetLastEnemy`): an empty basis, a garbage token and a stale memory
    /// are each a no-op (`0x489a40`'s bare `ret`), never a deselect or a panic.
    #[test]
    fn the_selection_queue_runs_assist_and_last_enemy_without_ever_deselecting() {
        use crate::net::NetCommands;
        use benilla_ui::script::UiScript;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const BASIS: u64 = 0xB0A2;
        const VICTIM: u64 = 0xC0DE;
        const GONE: u64 = 0x6017;
        // `UNIT_FIELD_TARGET` is a 2-field guid at index 16; HEALTH/MAXHEALTH keep the units live.
        let store =
            |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<crate::ui_loot::LootState>();
        world.init_resource::<crate::ui_loot::LootLatch>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<crate::ui_party::GroupState>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<super::AssistAttack>();
        world.init_resource::<Selection>();
        world.init_resource::<scan::LastEnemy>();
        world.init_resource::<Messages<super::by_name::AssistRequest>>();
        world.insert_non_send_resource(UiScript::new().expect("a bare VM"));

        world.spawn((SelfPlayer, Guid(ME), store(&[(22, 100), (28, 100)])));
        // The basis is pointing at the victim; the victim points at nobody.
        let basis = world
            .spawn((
                Guid(BASIS),
                store(&[(22, 100), (28, 100), (16, VICTIM as u32)]),
            ))
            .id();
        let victim = world
            .spawn((Guid(VICTIM), store(&[(22, 100), (28, 100)])))
            .id();
        let index = &mut world.resource_mut::<crate::net::GuidIndex>().0;
        index.insert(BASIS, basis);
        index.insert(VICTIM, victim);

        let run = |world: &mut World, lua: &str| {
            world
                .non_send_resource_mut::<UiScript>()
                .eval::<()>(lua)
                .expect("the binding runs");
            world
                .run_system_once(selection_requests)
                .expect("the drain runs as a one-shot system");
            world.resource::<Selection>().guid
        };
        let set = |world: &mut World, target: Option<(Entity, u64)>| {
            let mut sel = world.resource_mut::<Selection>();
            sel.target = target.map(|(e, _)| e);
            sel.guid = target.map(|(_, g)| g);
        };

        // Assist the current target: one hop onto its target.
        set(&mut world, Some((basis, BASIS)));
        assert_eq!(run(&mut world, r#"AssistUnit("target")"#), Some(VICTIM));

        // The victim targets nobody: assisting it is a no-op, not a deselect.
        assert_eq!(run(&mut world, r#"AssistUnit("target")"#), Some(VICTIM));

        assert_eq!(run(&mut world, r#"AssistUnit("nosuchunit")"#), Some(VICTIM));
        assert_eq!(run(&mut world, r#"TargetUnit("nosuchunit")"#), Some(VICTIM));

        // TargetLastEnemy with nothing remembered: also a no-op.
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));

        world.resource_mut::<scan::LastEnemy>().0 = Some(VICTIM);
        set(&mut world, None);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));

        // A stale memory (the unit despawned, so its guid is in no index) leaves the target alone.
        world.resource_mut::<scan::LastEnemy>().0 = Some(GONE);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));
        set(&mut world, None);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), None);
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
        world.init_resource::<PressPick>();
        world.init_resource::<Selection>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
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
        world.init_resource::<crate::ui_action::CastErrors>();
        world.init_resource::<crate::ui_loot::LootState>();
        world.init_resource::<crate::ui_loot::LootLatch>();
        world.init_resource::<crate::ui_mail::MailOpen>();
        world.init_resource::<crate::ui_item_text::ItemTextOpen>();
        world.init_resource::<crate::ui_action::GoOpenerCasts>();
        world.init_resource::<Messages<crate::ui_dialog_verbs::MeetingStoneUse>>();
        world.spawn((SelfPlayer, Guid(ME)));
        let boar = world
            .spawn((Guid(BOAR), store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100)])))
            .id();
        (world, boar)
    }

    /// A right-click begun on a plate acts on the plate's unit: [`act_on_right_click`] reads the
    /// [`PressPick`], and a plate leaves no body under the cursor for a live hover to find.
    #[test]
    fn a_right_click_begun_on_a_plate_acts_on_the_plates_unit() {
        let (mut world, boar) = right_click_world();
        // The press latch as `latch_press_pick` writes it: the plate's unit, distance 0.0
        // (topmost UI), classified Attack.
        *world.resource_mut::<PressPick>() = PressPick {
            hovered: Hovered {
                target: Some(boar),
                guid: Some(BOAR),
                distance: 0.0,
                ..Hovered::default()
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Attack,
                unable: false,
            },
            ..PressPick::default()
        };
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
        assert_eq!(
            world.resource::<Selection>().guid,
            Some(BOAR),
            "the release must act on the unit whose plate the press was over"
        );
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

    /// Out of interact range the click sends nothing, like the shared arms; the reference shows
    /// `ERR_USE_TOO_FAR` unless its auto-walk (`0x610300`) starts.
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
    }
}
