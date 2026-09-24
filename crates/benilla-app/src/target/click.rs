//! The click/USE router — the input half of targeting: a clean left-click selects the
//! [`super::Hovered`] unit, a clean right-click dispatches the context action (attack, NPC
//! interact, GameObject USE with the lock/refusal ladder), and the UI's selection asks
//! (`TargetUnit` / `AssistUnit` / `TargetLastEnemy`) + Esc drain into the same
//! [`super::Selection`]. Split from `mod.rs` (which keeps the state resources and the plugin)
//! along the state-vs-input seam; the systems here are registered by [`super::TargetPlugin`] in
//! the target chain after the hover picks and the cursor classifier.

use benilla_ui::script::SelectionRequest;

use super::lock::GoLockInputs;
use super::*;

/// The **right-click cursor-payload leg** — the reference's WorldFrame click router (`0x481f60`
/// → object leg `0x492ce0` / terrain leg `0x492c90` / nothing leg `0x492d30`; decision 0571 /
/// decision 0574), transcribed onto the camera arbiter's clean-click message (a drag/turn never
/// routes here, exactly the ref's click-not-drag gate `0x514ae0`):
///
/// - a **right-click over empty world** (terrain OR nothing): ANY payload clears silently — no
///   popup, no packet (both legs' action-4 arm: `ClearCursor(1,1)` unconditionally). This is
///   the "right-click dismisses the held item/spell" behavior.
/// - released over a **world object**: no payload change at all — `0x492ce0` clears only the
///   displayId-PREVIEW gate (`[0xb4b41c]`, an arm benilla doesn't carry) and INTERACT proceeds
///   normally in the systems that follow, item or spell still on the cursor.
///
/// The left-click legs live in the UI engine's world drop ([`benilla_ui::script`]'s
/// `world_drop_click`, routed by the app-fed pick — decisions 0218/0571/0574); that press is
/// consumed when it would drop, so no `WorldClick` fires for them. The put-down sound rides the
/// app's cursor-transition watcher (`crate::sound`), matching the ref's `ClearCursor` play.
pub(super) fn world_right_click_payload(
    mut right_clicks: MessageReader<WorldRightClick>,
    // The **press** pick, not the live hover (decision 2230) — this leg and [`act_on_right_click`]
    // are two halves of one click's routing, so they must classify the same pick or a right-click
    // can dismiss the cursor payload *and* interact with the thing it was held over.
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

/// **A plate click is a click on its unit** — the plate's own click slot (`0x7cb910`) ends in the
/// same `SetSelection` a click on the body does (decision 2148).
///
/// It **carries its own unit** rather than reading the press pick, and that is the whole design:
/// pfUI's click-through calls `plate:Click("LeftButton")` with the cursor wherever the player left
/// it (`nameplates.lua:1274`), so a plate click that depended on the hover would work under the
/// mouse and silently do nothing for the addon it exists to serve. The first cut of this system
/// replayed a [`WorldClick`] and was guarded on the press pick naming the same unit; the live probe
/// caught it — the scripted click selected nothing.
///
/// The RIGHT button is the other way round, deliberately. [`act_on_right_click`] acts on the
/// frame's [`PressPick`] across many legs (interact, attack, loot, GameObject), so feeding it a
/// unit that pick does not name could attack the wrong thing; it is replayed only when the press
/// pick already names the plate's unit. A scripted right-click on a plate does nothing yet, which
/// is a stated gap rather than a guess about which leg it should take.
///
/// Since 2233 the **physical** right-click on a plate comes through here, and this is the only path
/// it has: the press is the plate's (the camera no longer looks through one), so no look session
/// starts and the camera arbiter emits no [`WorldRightClick`] of its own. The plate's click slot
/// fires on the up edge and the replay below turns it into the interact leg — which is the
/// reference's own shape, `0x7cb910` → `0x4949f0(mask 4)` → `0x492820`, the same terminal the world
/// right-click's object leg `0x492ce0` reaches.
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
    // The ground-targeting cursor owns the click, exactly as it does for a world one (0792).
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
        // The NPC greets us on the SELECT gesture, plate or body — the byte-verified trigger fires
        // before SetTarget either way (`0x60c270`).
        greeting.write(crate::sound::NpcGreetingRequest { npc: entity });
        // The attack classification is the press pick's, and only when the press was actually on
        // this unit: a scripted click has no cursor behind it, so it is not an attack-cursor click.
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

/// On a [`WorldClick`], select the unit the **press** was over ([`PressPick`]) and inform the
/// server; a click on empty ground / a non-unit clears the target — except a click on NOTHING (sky
/// — no occlusion-ray hit) while a payload is held: the reference's nothing-leg deselect is
/// no-payload-gated (`0x492d30`'s local flag test), while the terrain leg deselects regardless of a
/// surviving spell/action payload (`0x5e03bb` — decisions 0571 + 0574). Skipped while the inspector
/// is armed (left-click is its copy affordance).
///
/// **The press pick, not the live hover** (decision 1122). A click may now arrive at the end of a
/// gesture that orbited the camera, and the live hover is empty by then — `update_hover` clears it
/// for the whole look session, as the reference suppresses its own hover during freelook. Reading
/// the live hover here would take the `_ =>` arm below and *clear* the player's target on every
/// drag. The reference picks once, on the down edge, and this consumes that same latch.
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
    // The clicked unit's descriptor — `0x493540`'s `IsSelectable` gate reads it inside
    // [`scan::commit`]. Read live rather than latched with the press pick: the reference resolves
    // the object at the *commit*, not at the down edge.
    stores: Query<&ObjectStore>,
) {
    let (hovered, occlusion) = (press.hovered, press.occlusion);
    // Drain the frame's clicks; act only if there was one and the inspector isn't holding left-click.
    let clicked = clicks.read().last().is_some();
    if !clicked || inspect.enabled {
        return;
    }
    // The ground-targeting cursor owns the click (decision 0792): the ref's terrain leg tries
    // the ground commit BEFORE its select/deselect half and skips it when the commit fires
    // (`0x492580`'s "otherwise"). The commit system runs after this one in the chain, so the
    // mode is still readable here — selection changes not at all, in range or out.
    if ground.active() {
        return;
    }
    let (self_guid, engaged) = self_q
        .single()
        .map(|(g, e)| (Some(g.0), e))
        .unwrap_or((None, false));
    match (hovered.target, hovered.guid) {
        (Some(entity), Some(guid)) => {
            // The NPC greets us on the SELECT gesture — the reference's trigger (the
            // variation-cycling greeter `0x60c270` fires before SetTarget,
            // i.e. on the left-click select, NOT the right-click interact — director-confirmed:
            // left-click greets and repeat left-clicks cycle, right-click does nothing). Fired on
            // EVERY select click on a unit (not gated on a selection change) so re-clicking the
            // same NPC steps the variation sequence; the sound crate holds the per-unit latch (a
            // re-click while the line still sounds is silent) and resolves non-NPCs to nothing.
            greeting.write(crate::sound::NpcGreetingRequest { npc: entity });
            // The one SetSelection law ([`scan::commit`]): dedup + selection + the engaged-switch
            // stop→select→re-swing. The cursor's Attack classification (alive + reaction ≤
            // neutral, hover-refreshed every frame) IS Attack `0x5ecb70`'s new-target validation.
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
        // Clicked nothing targetable → deselect (only sends the clear if we actually had a
        // target). The one exception is a payload held over NOTHING (sky): the ref's
        // nothing-leg deselect is no-payload-gated. Since 0843 ANY payload's empty-world left
        // press is consumed for the drop/dismiss (input.rs's `would_drop`), so a payload
        // normally never reaches this arm at all — the gate still holds the nothing-leg law
        // for the residual GameObject-hover case (`Object` pick, unit arm empty).
        // `deselectOnClick` (0961) gates the whole arm: "Sticky Targeting" checked = the CVar
        // at 0 = an empty-world click keeps the target (1.12's own inverted checkbox).
        _ => {
            // A **corpse** is an object hit, not empty world (decision 1723): the reference's
            // deselect lives in the terrain and nothing legs, and an object leg clears no
            // selection — so a left-click on a body must leave the target exactly where it was.
            // Without this the corpse would arrive here (its guid is in the other slot) and read
            // as "clicked nothing", dropping the player's target every time they clicked a body.
            //
            // **A REFUSED pick is the same kind of object hit** (decision 2060). The hover grader
            // `0x4828d0` takes the mouseover away from a `NOT_SELECTABLE` unit, but the *click*
            // never went through it: the down-edge pick `0x481f00` still found the object, so the
            // release takes the object leg `0x4925d0`, whose tail `0x49280c call 0x493540` hits
            // `SetSelection`'s own `IsSelectable` early-out and returns having changed nothing.
            // Selecting is refused; deselecting was never on the table.
            if hovered.corpse.is_some() || hovered.refused {
                return;
            }
            if click_cfg.deselect_on_click && (!payload_held.0 || occlusion.distance.is_finite()) {
                clear(&mut selection, &mut seam, engaged);
            }
        }
    }
}

/// The service arms that are **not** a bare packet, as one [`SystemParam`] (the 16-param ceiling).
///
/// Three of the reference's fourteen arms need something other than "send this opcode": bit 1
/// reads the target's cached questgiver status as its second conjunct, and bits 5 and 7 raise a
/// client-side CONFIRM dialog and send nothing at all (`0x5f0130`'s ladder).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ServiceArms<'w> {
    /// `[unit+0xcb8]`'s mirror — the last `SMSG_QUESTGIVER_STATUS` per guid, which the bit-1 arm's
    /// predicate `0x5df490` reads off the TARGET.
    pub(crate) quest: Res<'w, crate::ui_quest::QuestGiver>,
    /// The innkeeper's bind question (bit 7): `0x5dfdc0` caches the guid and fires `CONFIRM_BINDER`
    /// — `CMSG_BINDER_ACTIVATE` belongs to the dialog's Accept, not to the click.
    pub(crate) binder: ResMut<'w, crate::ui_binder::BinderState>,
    /// The spirit healer's XP-loss question (bit 5): `0x5df730` does the same, firing
    /// `CONFIRM_XP_LOSS`.
    pub(crate) death: ResMut<'w, crate::death::DeathNet>,
    /// The spirit GUIDE's arm (bit 6) — the battleground graveyard's resurrect wave. Unlike its
    /// two no-packet neighbours this one does send, and it sends through the cache the per-frame
    /// proximity poll also writes, so the click and the poll cannot disagree about which healer
    /// is current.
    pub(crate) spirit: ResMut<'w, crate::ui_dialog_verbs::AreaSpiritHealer>,
    /// `[0xb4e2d0]/[0xb4e2d4]`'s mirror — the NPC whose window is open, which the dispatcher
    /// compares the clicked guid against *before* the ladder ([`interaction_already_open_on`]).
    pub(crate) interact: Res<'w, crate::ui_session::InteractNpc>,
    /// The cursor, for the vendor arm's fork (`0x5df5d7`) — peeked to choose the arm's leg, then
    /// taken through the **sell** clear once the sale actually goes out. `Option` because a
    /// headless net-only build mounts no VM.
    pub(crate) script: Option<NonSendMut<'w, benilla_ui::script::UiScript>>,
}

/// **The re-click gate** — `0x5f0130`'s own, at `0x5f0251`/`0x5f025c`, sitting between the range
/// test and the ladder:
///
/// ```text
/// 5f024c  8b 46 08            mov edx,[esi+8] → [eax]   ; the CLICKED unit's guid
/// 5f0251  3b 15 d0 e2 b4 00   cmp edx,[0xb4e2d0]        ; the interaction NPC, low
/// 5f0257  75 0f               jne 0x5f0268              ; different → on to the ladder
/// 5f025c  3b 05 d4 e2 b4 00   cmp eax,[0xb4e2d4]        ; high
/// 5f0262  0f 84 62 03 00 00   je  0x5f05ca              ; SAME → the silent return
/// ```
///
/// So **right-clicking the NPC whose window is already open does nothing at all**: no packet, no
/// error, and no talk gesture — the gesture lives at the tail of each arm, and no arm runs.
///
/// `[0xb4e2d0]` is the `"npc"` UnitID token, and the timing is what makes this safe to transcribe:
/// an image-wide census of the two write sites — the arm at `0x493114` inside
/// `SetInteractionNPC 0x4930d0`, and `0x49334b`'s clear inside `0x493310` — finds
/// **fourteen callers of the setter, none of them on the click path**. Every one
/// is a window *opener*, armed when the server's reply lands. The first click therefore always gets
/// through (nothing is open yet); only a re-click while the window stands is eaten. That is exactly
/// the set [`crate::ui_session::feed_interact_npc`] collapses, which is why this reads that
/// resource rather than growing a second latch — the mailbox and the item-text reader are the two
/// arming sites it leaves out, and neither is a unit a right-click can land on.
fn interaction_already_open_on(target: u64, interact: &crate::ui_session::InteractNpc) -> bool {
    interact.1 == Some(target)
}

/// On a clean right-*click* (vanilla's context action — [`WorldRightClick`], never a turn-drag):
/// select the hovered unit, then act by the same classification the cursor used (the INTERACT
/// leg `0x492820`). Three branches (decision 0081):
/// - **Attack** (alive + reaction ≤ neutral): auto-draw and start melee auto-attack, exactly the
///   action-bar attack's path (decision 0073's verified attack-start: SETSHEATHED then ATTACKSWING).
/// - **Loot** (dead + `UNIT_DYNFLAG_LOOTABLE` — the state, not the Pickup cursor kind, which a
///   live vendor shares): open the corpse's loot (`CMSG_LOOT`), decision 0084.
/// - **Interact** on an in-range friendly service NPC: the reference's own first-match-wins ladder
///   over `UNIT_NPC_FLAGS` ([`service_arm`] → [`service_action`], decision 1861) — bit 0 GOSSIP
///   first, so a trainer or questgiver that also carries the bit still opens a menu and only a
///   flagless one opens its own window (1865). A click that lands on the NPC whose window is
///   already open is eaten before any of it ([`interaction_already_open_on`], 1905). On a taken
///   arm our avatar plays EmoteTalk (id 60), which stows the weapon via the anim→sheath reconcile.
///
/// The cursor's own gate grays a service beyond `SERVICE_RANGE` (`unable`); we don't send then (no
/// auto-approach yet) — the selection still lands. Attack, by contrast, is never range-gated
/// (`unable` only grays): the server holds the swing until we're in reach, as the real client does.
/// A right-click on empty ground was just a turn — it never deselects.
// The `ui_feedback` tuple is the 16-SystemParam ceiling's overflow bundle, commented at its site.
#[allow(clippy::type_complexity)]
pub(super) fn act_on_right_click(
    mut clicks: MessageReader<WorldRightClick>,
    // **The press pick, not the live hover** (decision 2230) — the same latch the left button has
    // read since 1122, and for the same reason: the reference picks exactly once, on the down edge
    // (`0x481f00`, one caller image-wide), into the WorldFrame's own slots, and nothing re-picks on
    // move or on release. Reading the live hover here only looked right because the release frame
    // re-picks at the restored cursor and, for a body click that moved nothing, lands back on the
    // same unit. A V-plate has no body under the cursor to land back on: the plate published the
    // mouseover at the press (`0x7cb850` → `0x492890`), freelook then took the plates' mouse away
    // (`0x60f830`), and the live hover at the release was empty — so a right-click on a plate
    // reached every branch below with nothing hovered and did nothing at all.
    press: Res<PressPick>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    self_player: Query<(Entity, &Guid, Has<Engaged>), With<SelfPlayer>>,
    // The interact talk gesture. The reference's NPC-interact dispatcher calls the SAME gesture
    // entry point the chat display path does, always with code 0 — so this goes through
    // `creature_anim::gesture` rather than writing a raw AnimID, and inherits its gate chain
    // (decision 1469; before that it played unconditionally, even asleep or mid-combat).
    mut gestures: ResMut<crate::creature_anim::GestureQueue>,
    // The GameObject lock-routing inputs (decisions 0239 / 0545 / 0752) as one [`GoLockInputs`]
    // (the 16-SystemParam ceiling).
    mut go_inputs: GoLockInputs,
    player_actions: Res<crate::ui_action::PlayerActions>,
    // `[0xb700e4]`'s mirror — the skin leg's spell, already gated by the classifier (0752).
    learned: Res<crate::ui_action::LearnedAbilities>,
    // The GO leg needs the object's stored state too (the Action gate, decision 0752); the unit
    // legs ignore the second member.
    stores: Query<(&ObjectStore, Option<&crate::go_anim::GoAnim>)>,
    // One tuple param (the 16-SystemParam ceiling): the red-error keys + the reason-coded cast
    // line (the opener cast's local totem refusal, decision 0552) + the loot-target latch
    // the loot branch arms (decision 0515), and the mailbox session the mailbox branch opens
    // (decision 0544).
    // The three non-packet service arms (decision 1861), bundled — see [`ServiceArms`].
    mut service: ServiceArms,
    ui_feedback: (
        ResMut<crate::ui_action::UiErrorKeys>,
        ResMut<crate::ui_action::CastErrors>,
        ResMut<crate::ui_loot::LootLatch>,
        ResMut<crate::ui_mail::MailOpen>,
        // The reader session the TEXT branch opens (decision 1105) — like the mailbox, a
        // client-side window with no packet behind it.
        ResMut<crate::ui_item_text::ItemTextOpen>,
        // The GameObject opener's one-frame queue (decision 2199): this system cannot hold
        // `CastLadder` (it would reach `Items`/`CastErrors` twice), so the lock chain's verdict
        // travels to `ui_action::drain::drain_go_openers` instead of becoming a packet here.
        ResMut<crate::ui_action::GoOpenerCasts>,
        // The meeting stone's use slot (decision 2283), here for the same reason: its four
        // refusals read the roster and the template cache, so the click hands the object to
        // `ui_dialog_verbs::drain_meeting_stone_joins` rather than validating and sending here.
        MessageWriter<crate::ui_dialog_verbs::MeetingStoneUse>,
    ),
) {
    let (
        mut ui_error_keys,
        mut cast_errors,
        mut loot_latch,
        mut mail,
        mut item_text,
        mut openers,
        mut stone_uses,
    ) = ui_feedback;
    if clicks.read().last().is_none() {
        return;
    }
    // The whole ladder below reads ONE pick — the gesture's own, frozen at the press.
    let (hovered, hovered_object, cursor) = (&press.hovered, &press.object, &press.cursor);
    // ── The interact family's ACTOR gate: am I in the saddle? ────────────────────────────────
    // One predicate, the reference's own — the PLAYER's `UNIT_FIELD_MOUNTDISPLAYID` (decision
    // 0481's "one mounted predicate": no aura, no taxi
    // distinction, this field and nothing else). 0481 built the gate for the two action families
    // the director reported then — casts (`0x6094f0`'s reason `0x39` "You are mounted") and
    // attack-start (`0x612df0`'s `ERR_ATTACK_MOUNTED`) — and wrote interaction explicitly out of
    // scope. This is that third family (decision 1851): the right-click interact dispatchers read
    // the very same field, in their own ladders, and until now we read it in none of them.
    let self_store = self_player
        .single()
        .ok()
        .and_then(|(e, _, _)| stores.get(e).ok())
        .map(|(s, _)| s);
    let self_mounted = self_store.is_some_and(|s| s.0.unit_mount_display_id() != 0);
    // A GameObject is the nearest thing under the cursor → use it (decision 0236), and never fall
    // through to unit handling: a GO is not selectable, and a right-click on it acts on the GO or
    // does nothing. The reference's `OnUse 0x5f8660` gates on the same two predicates the cursor
    // computes: **highlightable** first — false is a silent no-op, which for us is
    // `cursor.kind == Point` — then **usable**, whose failure toasts an error and sends nothing.
    //
    // `usable`'s two arms we model land in different places, so the routing is split accordingly
    // (decision 0752): the **lock** arm comes back out of [`resolve_go_action`] as
    // `GoAction::Refuse`, which carries the lock-fail toast routing (`0x5f3427..`); the **range**
    // arm is `cursor.unable`, which suppresses the send with no toast (the reference auto-walks
    // there instead — `0x610300`, also no packet). The lock arm runs first in `0x5f3130`, and it
    // does here too: a `Refuse` toasts even when we are also out of range.
    if go_is_nearest(hovered, hovered_object) {
        // **The interact chain's first link** (tag `use`). "I clicked it and nothing happened" spans
        // three systems — this decision, the `CMSG_GAMEOBJ_USE` it sends, and the `SMSG_SPELL_GO`
        // the server answers with — and until this line existed only the *taken* branches said
        // anything, at `debug!`, which the hardcoded log filter keeps off. Both silent refusals are
        // traced here: a `Point` cursor (the reference's highlightable no-op) and `unable` (the
        // range gray, which suppresses the send with no toast). Pairs with the `fx` kit lines, so
        // ONE run says which link is dead instead of one round-trip per link.
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
                // ── The GameObject leg's OWN mounted gate — not the cast validator's ────────
                // The strategy's usable predicate `0x5f3130` carries a mounted arm of its own at
                // `0x5f31a8`: errorId `0x19f` `ERR_NOT_WHILE_MOUNTED`, then `0x5f31d6 xor al,al`,
                // so `0x5f86b0` never reaches the opener it would invoke at `0x5f86eb`. No
                // `CMSG_GAMEOBJ_USE`, no cast, no state write — the gate returns before the
                // opener, not merely before the message.
                //
                // **It applies only to LOCK-LESS objects.** The gate sits behind
                // `0x5f3195 call 0x5f8180` / `0x5f319c jne 0x5f3231`, which resolves the object's
                // **Lock.dbc row** and skips the whole thing when one exists. So a mining node, a
                // herb or a locked chest bypasses this and is refused further down by its opener
                // CAST's own mounted block — loudly, "You are mounted" — while a spellcaster, a
                // chair, a `lockId 0` quest goober and a readable sign are refused HERE, in
                // silence. What buys the bypass is a **lock**, not a spell.
                //
                // The test is pure **row existence**: `0x5f819c` tests the row *pointer*, and
                // nothing reads `Type[]`/`Index[]`/`Skill[]` until `0x5f83d0`. So it is
                // deliberately NOT [`benilla_formats::LockCatalog::is_locked`], which also demands
                // a non-empty slot — an all-empty Lock row is "no lock" to our opener resolver and
                // "a lock" to this gate, and the reference lets that object be used from the
                // saddle. Collapsing the two diverges from the reference on exactly that object.
                //
                // **MAILBOX (type 19) is the only exemption**, and it lives inside the gate on the
                // mounted arm alone (`0x5f31bb` → `0x47cff0`: a sixteen-byte `type == 0x13`
                // equality, no table, no range). TEXT is NOT exempt — the reference refuses a sign
                // from horseback even though its opener is every bit as client-side as the
                // mailbox's, which is why this sits ABOVE both local-open branches below.
                //
                // Silent by Blizzard's omission, not our choice: `ERR_NOT_WHILE_MOUNTED` is one of
                // 1.12.1's orphan keys with no GlobalStrings value, and both of `DisplayError`'s
                // side-effect branches skip for this row (`[row+0xc] == 0x44` takes the no-sound
                // jump; `[row+0x8]` is the literal `"NONE"`), so nothing is shown, sounded or
                // fired. Whether that leaves any visible artifact at all is the one thing the
                // binary could not settle — it wants one mounted click on a sign.
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
                // Mailbox (GO type 19): open the mail window client-side (decision 0544), BEFORE the
                // lock fork (a mailbox is never locked). The MAILBOX use handler `0x5f6820`
                // overrides the shared use-sender to a LOCAL open — it sends NO packet (no
                // CMSG_GAMEOBJ_USE); the window's own MAIL_SHOW → CheckInbox drives the first
                // CMSG_GET_MAIL_LIST. Re-clicking just re-shows (the session is already set).
                if go.is_some_and(|(s, _)| s.0.gameobject_type_id() == cursor_mode::GO_TYPE_MAILBOX)
                {
                    if !cursor.unable {
                        debug!("right-click mailbox: open mail window {guid:#x}");
                        mail.click(guid);
                    }
                    return;
                }
                // TEXT (GO type 9): a book, plaque or sign — READ it, client-side, before the lock
                // fork (a readable is never locked). Its strategy overrides the use-slot the same
                // way the mailbox does — `0x5f58c0` calls the local page-text opener
                // `0x4e32e0(goGuid, 0)` and never the shared `CMSG_GAMEOBJ_USE` sender (decision
                // 1105). Sending USE
                // instead is what left every world book dead: vmangos' `GameObject::Use` has no
                // type-9 case at all, so the packet is answered with silence.
                //
                // `arg2 == 0` here means the reference's **toggle**: right-clicking the book whose
                // reader is already open closes it. The page id + material are NOT resolved here —
                // the reader asks the object's template for them as it paints, like the reference's
                // `vtbl+0x74` getter, so a click that beats the ask-once template query still opens.
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
                // MEETINGSTONE (GO type 23): **not** the shared use-sender. `[0x80bf40+0x1c]`
                // is `0x5f69d0`, this type's own validator — four client-side refusals and then
                // `CMSG 0x292 {u64 goGuid}` from `0x4c9ff0` (decision 2283). The shared sender
                // `0x5f33e0` is **unreachable** from here: it
                // has zero direct callers and 29 `.rdata` refs, every one at some vtable's
                // `+0x1c`, and `0x80bf5c` is not among them — so a meeting stone cannot emit
                // `CMSG_GAMEOBJ_USE` in the reference at all. Sending `0xB1` anyway is what left
                // every meeting stone dead here: vmangos' `GameObject::Use` has an explicit
                // type-23 arm that does nothing ("Should never be called for this type of
                // object", `GameObject.cpp:1836`), so the packet was answered with silence.
                //
                // Four types override `+0x1c`, not one — 9 TEXT, 19 MAILBOX, 23 and 28; every
                // other strategy's `+0x1c` is `0x5f33e0`.
                //
                // Unlike those two it is NOT a local open — it is a real send, and it therefore
                // sits *below* the mounted gate above (a stone is lock-less, and only MAILBOX is
                // exempted there) and behind the same `unable` range suppression every sending
                // arm takes. The refusals themselves need the roster, the ask-once template and
                // the wire, which this system cannot also hold, so the verdict travels as a
                // message to `ui_dialog_verbs::drain_meeting_stone_joins` — the 2199 shape.
                if go.is_some_and(|(s, _)| {
                    s.0.gameobject_type_id() == cursor_mode::GO_TYPE_MEETINGSTONE
                }) {
                    if !cursor.unable {
                        debug!("right-click meeting stone: {guid:#x}");
                        stone_uses.write(crate::ui_dialog_verbs::MeetingStoneUse { go_guid: guid });
                    }
                    return;
                }
                // Branch on the lock (decisions 0239 / 0545 / 0752): a lockless GameObject is USEd;
                // a lockable one casts the opener (a known OPEN_LOCK spell, or a carried key's own
                // ON_USE) at it; an unopenable lock shows the client-local red toast — "The door is
                // locked.", "Requires Herbalism", "Requires Mining 100", "Requires <key item>" —
                // and sends nothing (the ref's validate/error block `0x5f3427..` fires
                // `DisplayError` with no packet).
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
                    // **Both opener arms queue for the one cast path** (decision 2199) — they
                    // do not send. The reference reaches `TryCast 0x6e4b60` from the GameObject
                    // strategy's use-sender (`0x5f35c0 → 0x6e5a90 → 0x6e4b60`) exactly as it
                    // does from a button press, so an opener owes the whole ladder — the in-flight
                    // refusal above all, which is what keeps a mashed right-click from shipping a
                    // duplicate the server answers "Another action is in progress" while red-fading
                    // the bar of the cast that is still running. The local reagent/totem and
                    // mounted checks that used to stand here are ladder rungs; they ran here only
                    // because this arm could not reach the ladder at all.
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
                        // `None` is a case the ref is silent on too (a key-item record miss —
                        // its ask-once query is away — or the deferred key-in-hand open).
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
    // ── The corpse leg: `CGCorpse_C`'s own interact slot, `0x5d6bf0` (vtable `+0x60`) ───────────
    // A corpse is not a unit and never routes through the unit block below — the reference gives
    // its class a *different* function in the same slot the unit class fills with `0x60bea0`.
    // Transcribed from the disassembly at `0x5d6bf0..0x5d6cdd`, which has exactly two legs:
    //
    //   1. `[player descriptor + UNIT_FIELD_MOUNTDISPLAYID] <= 0` (**not mounted**, `0x5d6c2a jg`)
    //      AND `0x5d6e20(corpse)` (**lootable** — `CORPSE_FIELD_DYNAMIC_FLAGS` bit 0) → a player-
    //      state check on the vtable `+0xa4` slot, which on failure raises message `0x85` and
    //      sends nothing; else `SetAutoLoot` (`0x5df460`) and then `0x5df130` — **`CMSG_LOOT`**.
    //   2. else `CORPSE_FIELD_FLAGS` bit 5 (the PvP insignia) AND the `[0xb700e8]`
    //      SKIN_PLAYER_CORPSE learn latch AND `!0x6067d0(player, corpse)` → the skin cast
    //      (`0x5f05e0`). Unreachable without that latch, and nothing in 1.12.1 sets it.
    //
    // **Neither leg touches your own body**, which carries neither flag — the function runs and
    // does nothing at all. Recovering your corpse is a separate route (decision 0308's
    // `RECOVER_CORPSE` → `CMSG_RECLAIM_CORPSE`) whose click path is out for RE and is NOT
    // guessed at here; the director reports a right-click on their own body raising the resurrect
    // prompt, so a route exists and this function is not it.
    if let (Some(entity), Some(guid)) = (hovered.corpse, hovered.corpse_guid) {
        let store = stores.get(entity).ok().map(|(s, _)| s);
        // The same one-line answer the GameObject leg gives (tag `use`): a corpse's click is three
        // silent refusals deep — no flag, the range gray, the missing latch — and every one of them
        // otherwise looks identical from the outside. One run says which.
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
        // **Leg 1's first conjunct is `!mounted`** (`0x5d6c2a jg`, the player's MOUNTDISPLAYID read
        // one test before the lootable one). The block comment above has named this conjunct since
        // the leg was transcribed and the code never carried it — a rider could loot bones from the
        // saddle. A mounted click is **not** an error here: it falls through to leg 2 exactly as the
        // unit dispatcher falls through to skin, and since no 1.12.1 player has the insignia latch,
        // what the rider actually gets is silence.
        if !self_mounted && store.is_some_and(|s| s.0.corpse_lootable()) {
            // **`GetStandState() != 0` refuses before the send** (`0x5d6c3b call [playerVtbl+0xa4]`
            // = `0x5ed570`, non-zero → `0x496720(0x85)`): sitting, kneeling or asleep, the click
            // raises the client-local red **`ERR_LOOT_NOTSTANDING`** as a `UI_ERROR_MESSAGE` and
            // sends nothing. Client-local, so the line shows even where the server would also have
            // refused — and unlike the range gray it is loud, because the player can fix it.
            // No longer scoped to the corpse leg: 1851 read the unit dispatcher's own copy
            // (`0x60bfb7` → `0x60c007 push 0x85`) off the same fork and built it too, so the
            // "unverified here" caveat this comment carried since 1729 is retired.
            let standing = self_store.is_none_or(|s| s.0.unit_stand_state() == 0);
            if !standing {
                debug!("right-click corpse loot: refused, not standing ({guid:#x})");
                ui_error_keys
                    .0
                    .push(crate::ui_action::UiError::key("ERR_LOOT_NOTSTANDING"));
                return;
            }
            // Range rides the cursor's own gray, exactly as the unit loot branch does —
            // the classifier and the click ask literally the same question, so the pouch can never
            // be lit on something a click refuses.
            if !cursor.unable {
                debug!("right-click corpse loot: {guid:#x}");
                let _ = seam.net.0.send(ClientCommand::Loot { guid });
                // Same client-side prediction as the unit corpse (decision 0515): the reference's
                // `CMSG_LOOT` sender arms `[player+0x1d28]` and kneels before any reply.
                loot_latch.0 = Some(guid);
            }
        } else if store.is_some_and(|s| s.0.corpse_pvp_insignia()) {
            // Leg 2, with its real precondition. `skin_player_corpse` is the mirror of `[0xb700e8]`
            // and is `None` for every 1.12.1 player, so this is inert for the reference's own
            // reason rather than by omission.
            if let Some(spell_id) = learned.skin_player_corpse {
                if !cursor.unable {
                    // Same TryCast funnel as the unit skin leg, so the same mounted block — and
                    // this is the leg a mounted bones-click FALLS INTO, which makes the gate here
                    // the difference between "nothing happens" and a rider yanking insignias.
                    // The record comes off [`GoLockInputs`]'s catalog because that bundle is this
                    // system's only handle on `Spells` and the param list is at the 16 ceiling —
                    // the bundle is named for its first tenant, not scoped to it.
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
    // ── The dead-target fork of the reference's unit interact dispatcher `0x60bea0` ──────────
    // Loot routes by the same CLASSIFICATION the cursor used — dead + `UNIT_DYNFLAG_LOOTABLE` —
    // not by the cursor kind (the right-click routes the same hovered object by the same
    // classification; its dead-unit row sends CMSG_LOOT). The loot cursor's
    // base mode is Pickup(8), which a live vendor also shows, so the kind alone can't name loot.
    //
    // **The fork's first test is the rider**, and we never carried it (decision 1851): `0x60bf98`
    // reads `[ecx+0x1fc]` off the *player's* descriptor block — the one still in `ecx` from
    // `0x60bee5` — and `jg 0x60c01f` skips the whole loot leg for a mounted player, landing on the
    // skin leg. That is a **fall-through, not a refusal**: no packet, no red line, no "You are
    // mounted". The cast and attack families of this same gate (0481) each announce themselves;
    // this one is silent, which is exactly why it could sit here unbuilt without ever looking
    // broken from the inside.
    // Step 0 of the fork, hoisted because the trace prints it too: a corpse the dispatcher will
    // actually treat as one.
    let dead_fork = target.is_some_and(|s| s.0.unit_is_dead() && !s.0.unit_dynflag_dead());
    let leg = dead_unit_leg(
        self_mounted,
        dead_fork,
        target.is_some_and(|s| s.0.unit_lootable()),
        target.is_some_and(|s| s.0.unit_flags() & cursor_mode::UNIT_FLAG_SKINNABLE != 0),
        learned.skinning.is_some(),
    );
    // The unit twin of the GO/corpse legs' one-line answer (tag `use`). Every refusal on this
    // ladder is silent from the outside — the range gray, the mounted fall-through, a corpse
    // somebody else owns — and they are indistinguishable on screen. One run says which.
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
                    // The alive branch (`0x60c162`): `CanInteract` → the service send. Not this
                    // fork's to name, and NOT mounted-gated — a rider talks to a flight master.
                    "service"
                },
                cursor.kind,
                cursor.unable
            ),
        );
    }
    let me = self_player.single().ok();
    // The one SetSelection law ([`scan::commit`]): dedup + selection + the engaged-switch
    // stop→select→re-swing. The Attack cursor kind is Attack `0x5ecb70`'s new-target validation
    // (alive + reaction ≤ neutral) — a mid-combat click on a vendor/corpse switches and stops,
    // it never swings at them.
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
            // The actor-eligibility block — **silent here**, and that is 1851 correcting 0481. The
            // click still SELECTED (the commit above already ran, the ref's select-then-refuse order)
            // and the melee auto-draw and swing still never happen; what changed is that no red
            // `ERR_ATTACK_*` line shows any more.
            //
            // 0481 attached `0x612df0`'s Phase A ladder to this path. That validator has exactly
            // three callers image-wide — pet-attack `0x4bd40d`, the
            // Attack action/keybind `0x6131aa`, and TryCast `0x6e4efb` — and the world right-click is
            // none of them: it runs `0x60c247 call 0x5ecb70`, an extent containing no `DisplayError`
            // at all. So all eight of those red lines belong to the bar and the pet command, never to
            // the click, and the fix is not to delete six of them but to move where they are asked
            // for. [`attack_actor_blocked`] is the same ladder with the message removed; the bar keeps
            // [`attack_actor_refusal`]. (The predicate is still `0x612df0`'s and not `0x5ecb70`'s own
            // overlapping set — a separate slice — but the SILENCE is the verified law, and the swing
            // is suppressed either way.)
            if crate::ui_action::attack_actor_blocked(self_store, me.map(|(_, g, _)| g.0)).is_some()
            {
                // refused — selection stands, no swing, and nothing is said
            } else {
                debug!("right-click attack: {guid:#x}");
                // The right-click's own StartAttack, through the one seam (auto-draw + the swing +
                // the auto-repeat cancel — `0x5ecb70`'s whole body). The commit above may already
                // have re-pointed the swing at this guid, in which case the ref's `0x5eccda` is the
                // thing that suppresses a second send, so pass it the real lock state and let the
                // seam decide: `swung` means the re-swing already went out and the lock is ours, so
                // no stop is in flight any more.
                let engaged = me.is_some_and(|(_, _, e)| e);
                seam.start(guid, engaged || outcome.swung, false);
            }
        }
        UnitBranch::Dead(DeadUnitLeg::Loot) => {
            // A dead unit carrying UNIT_DYNFLAG_LOOTABLE (the Pickup loot cursor, decision 0084): open
            // its loot (`CMSG_LOOT`). Range-gated like the interact branch — the cursor grays a corpse
            // beyond the melee interact reach (`unable`), and we don't send then (no auto-approach yet).
            // No EmoteTalk: looting is not an NPC interaction, the corpse plays no talk.
            //
            // **The unit leg's own stand-state gate** (`0x60bfb7 call [playerVtbl+0xa4]`, non-zero →
            // `0x60c007 push 0x85; call DisplayError 0x496720`) — the exact instruction pair the corpse
            // leg carries at `0x5d6c3b`/`0x496720(0x85)`, in the same fork, one test after the lootable
            // one. The corpse leg's comment has said since 1729 that "the unit loot branch's own copy of
            // it is unverified here"; it is the same `push 0x85` and the same virtual, read off the same
            // dispatcher this session's `0x60bf98` mounted gate came out of. So the caveat retires and
            // the two legs say the same thing: sitting, kneeling or asleep, the click raises the
            // client-local red `ERR_LOOT_NOTSTANDING` and sends nothing.
            if self_store.is_some_and(|s| s.0.unit_stand_state() != 0) {
                debug!("right-click loot: refused, not standing ({guid:#x})");
                ui_error_keys
                    .0
                    .push(crate::ui_action::UiError::key("ERR_LOOT_NOTSTANDING"));
            } else if !cursor.unable {
                debug!("right-click loot: {guid:#x}");
                let _ = seam.net.0.send(ClientCommand::Loot { guid });
                // The kneel is client-predicted AT THE SEND: the real client's `CMSG_LOOT` sender
                // (`0x5df253`) sets the loot-target latch `[player+0x1d28]` and plays Loot 50 before
                // any server response (decision 0515). Arm the latch the anim driver's loot leg
                // reads for the self unit; the release/refusal drops it.
                loot_latch.0 = Some(guid);
            }
        }
        UnitBranch::Dead(DeadUnitLeg::Skin) => {
            // A dead SKINNABLE corpse the loot leg declined (`0x60c01f`): cast our known Skinning spell
            // at it — the unit-side mirror of the GO lock split (0239; decision 0437's gathering
            // finish). The spell comes from the reference's own learn-time latch
            // ([`crate::ui_action::LearnedAbilities`] = `[0xb700e4]`, decision 0752), which is the same
            // thing the classifier gated the Skin cursor on. Ordinarily that means an unlootable
            // corpse; **while mounted it also means a still-lootable one**, and the cast then meets the
            // cast family's own mounted gate (`0x6094f0`'s reason `0x39`) and says "You are mounted" —
            // the one leg of this dispatcher where the rider is told anything at all.
            // Range rides the cursor's melee-reach gray, like loot.
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
        // **The silent leg — and it is TERMINAL.** That is the whole of decision 1858. A dead
        // unit whose fork declined both legs (a rider over a lootable corpse; a corpse someone
        // else killed; a skinnable one without the skill) is DONE: the reference reaches its
        // NPC-service dispatch only down the ALIVE branch (`0x60c162`), never off the back of
        // this fork.
        //
        // benilla wrote the fork as an `if/else if` chain whose LAST arm was the service send, so
        // "the dead fork chose nothing" fell through into "this is a live service NPC" — and that
        // arm dispatches on the CURSOR kind, which over a lootable corpse is `Pickup`, the mode
        // the vendor pouch also uses ([`interact_command`]). The click therefore sent
        // `CMSG_LIST_INVENTORY` at a dead wolf, and vmangos answers a non-vendor with
        // `SELL_ERR_CANT_FIND_VENDOR` (`ItemHandler.cpp:701-710` → `GetNPCIfCanInteractWith` →
        // `CanInteractWithNPC`'s npc-flag and `IsAlive` tests), which this client renders as the
        // red **"That merchant doesn't like you."** on an animal corpse.
        UnitBranch::Dead(DeadUnitLeg::Nothing) => {
            debug!("right-click unit {guid:#x}: dead fork took no leg — nothing sent");
        }
        UnitBranch::Service if !cursor.unable => {
            // An in-range friendly service NPC (the cursor already gated friendly + service +
            // range): run **the reference's own ladder over `UNIT_NPC_FLAGS`** ([`service_arm`]).
            //
            // Not the cursor's kind, which is what this dispatched on until decision 1861. The
            // reference runs two structurally identical ladders — one for the cursor
            // (`0x482336`), one for the send (`0x5f0289`) — and the cursor MODE is a lossy
            // projection of the winning bit: eight arms collapse to Speak(6) and two to Buy(3),
            // so a kind-keyed dispatch cannot tell a banker from an auctioneer, nor a trainer
            // from an innkeeper from a spirit healer. It sent `CMSG_GOSSIP_HELLO` for all eight.
            // `0x5f0251` — the click that lands on the NPC we are already talking to is eaten
            // whole, before the ladder and before the gesture (decision 1905).
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
            // The vendor fork's input, resolved before the arm is known because it is cheap and
            // because `service_action` must stay pure. **Peeked, not taken**: the cursor is only
            // cleared once the sale is actually chosen, so a click on a trainer while holding an
            // item leaves the item where it is.
            //
            // The reference addresses the packet with the cursor's own stored guid; we carry
            // `(bag, slot)` and resolve it, exactly as the merchant window's sell route does. A
            // pickup LOCKS the source slot rather than emptying it, so the guid is there in every
            // ordinary case; if it ever is not, the fork simply does not arm and the click opens
            // the list instead — the benign side of a divergence we cannot avoid.
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
                // `0x5f05ca` — a unit that reaches the ladder and matches no consulted bit
                // (repair-only, or a questgiver with nothing on offer) does nothing at all, and
                // takes no gesture with it.
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
                    // Now the cursor goes, through the SELL clear: `CURSOR_UPDATE` fires and the
                    // source slot stays greyed until the server's inventory update takes it away.
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
            // Talk at the NPC — on **every taken arm**, including the ones that send nothing:
            // the reference calls the gesture after the arm's handler RETURNS, not after a send
            // (each arm of `0x5f0130` ends `call 0x60bb30(0)` then `ret 8`). The gesture's own
            // anim carries WeaponFlags `0x10`, so the per-animation sheath reconcile stows a drawn
            // weapon — a committed change that persists after the talk (decisions 0080/0081).
            if let Some((_, my_guid, _)) = me {
                gestures.push(my_guid.0, crate::creature_anim::Gesture::Talk);
            }
        }
        // The classifier's own range gray on a LIVE service NPC — outside the 5.5556 yd service
        // reach the click sends nothing (there is no auto-approach yet), exactly as the loot and
        // skin legs ride the same `unable`.
        UnitBranch::Service => {}
    }
}

/// The right-click action a hovered GameObject resolves to (decisions 0239 / 0545 / 0752) — chosen
/// by its lock, through the shared chain in [`super::lock`].
pub(crate) enum GoAction {
    /// No lock (or no lock data): `CMSG_GAMEOBJ_USE` — door / lever / quest object / mailbox /
    /// unlocked chest.
    Use,
    /// A lock we can open with a **known skill spell**: cast it at the object — a known
    /// `OPEN_LOCK` (chest / vein / herb / a picked lock). `CMSG_CAST_SPELL`.
    OpenLock(u32),
    /// A lock whose **KEY** slot we satisfy: *use the key at the object* — `CMSG_USE_ITEM` with the
    /// key's wire position and `TARGET_FLAG_GAMEOBJECT`, NOT a bare cast of the key's spell
    /// (decision 0769; the client never sends a bare CMSG_CAST_SPELL for a key lock: the cast
    /// sender `0x6e54f0` takes its item arm). The distinction is the whole ballgame:
    /// `Spell::CanOpenLock` honours a `Lock.dbc` KEY slot only when `m_CastItem` is set, which only
    /// USE_ITEM supplies.
    ///
    /// Carried as a whole [`crate::ui_items::ItemUse`] (decision 2199) because the reference's
    /// lock chain calls `CGItem::Use` with the lock's guid — the one item-use fork every surface
    /// takes — rather than building a packet of its own. `on_object` is that guid.
    OpenByKey(crate::ui_items::ItemUse),
    /// A lock present that we cannot open — the client-local refusal (`0x5f3427..`:
    /// `DisplayError`, **no packet**). `Some` = the red toast to queue; `None` = the ref is silent
    /// for this case too.
    Refuse(Option<crate::ui_action::UiError>),
}

/// What we know about a key-item lock slot's key when routing a refusal ([`route_lock_refusal`]).
enum KeyFact {
    /// Not held; the item template names it ("Requires Shadowforge Key").
    Named(String),
    /// Not held and the template isn't cached yet — the ref's `GetRecord` miss is silent (the
    /// `0xde` arm's `0x5f3562 je`); our ask-once query is away, so a later click names it.
    Unknown,
}

/// Resolve a hovered GameObject's right-click action from its lock — the reference's USE sender
/// `0x5f33e0` over the shared resolver (decisions 0239 / 0545 / **0752**).
///
/// Not-yet-queried or no `Lock.dbc` → treat as lockless (`Use`): the stream-in query makes "not
/// cached" a rare race, and `Use` is both the correct lockless action and a harmless no-op on a
/// chest whose template is still in flight.
///
/// The satisfaction decision itself lives in [`super::lock::resolve_lock`] so the cursor's `usable`
/// asks exactly the same question (the resolver `0x5f83d0` — the icon and the click agree by
/// construction). Here we only turn its answer into a packet, and the lock split has **three**
/// arms, not two: lockless → `CMSG_GAMEOBJ_USE`; a satisfied **skill** slot →
/// `CMSG_CAST_SPELL` of the matched opener; a satisfied **key** slot → `CMSG_USE_ITEM` carrying the
/// key's position and the GO as its cast target ([`GoAction::OpenByKey`]). An unmet lock takes
/// the toast routing of `0x5f3427..` and sends nothing.
///
/// The key arm sent a bare cast until decision 0769, which is why keys never opened anything: the
/// server honours a KEY slot only when the cast carries `m_CastItem` (`Spell::CanOpenLock`,
/// `Spell.cpp:7892`), and only `CMSG_USE_ITEM` supplies it. That was recorded here as the server's
/// gap; it was ours: the client never sends a bare CMSG_CAST_SPELL for a key lock (the cast sender
/// `0x6e54f0` takes its item arm).
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
    // A lockId whose row is missing is "no lock" — the ref resolver's `0x5f8180` null → FALSE with
    // spell 0 → `CMSG_GAMEOBJ_USE`.
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
            // The retest instrument for B247 (decision 1312): which of the player's openers this
            // lock resolved to, by id. The spell's Spell.dbc name is what the cast bar prints, so
            // reading the id off a probe run is how "the bar says Opening - No Text" becomes a
            // machine-checkable fact instead of a screenshot.
            debug!("target: lock {} → open by spell {spell_id}", tmpl.lock_id);
            return GoAction::OpenLock(spell_id);
        }
        super::lock::LockOutcome::OpenByKey(entry) => entry,
        super::lock::LockOutcome::Unmet => {
            // Unopenable — the toast routing of `0x5f3427..`, which keys off Lock.dbc **slot 0**
            // regardless of which slot the resolver walked.
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
    // A key we carry: **use it at the object**. The resolver `0x5f85aa` only hands the caller the
    // key's ON_USE spell id AND the item object itself; it is the sender `0x6e54f0` that then
    // discriminates on cast-item-vs-caster and takes the item arm — `0x6e57d8 push 0xab`,
    // CMSG_USE_ITEM carrying `u8 bag · u8 slot · u8 spellSlot · SpellCastTargets(GO)`, with no raw
    // spell id at all (the server re-resolves the Item* from bag+slot). Decision 0769.
    //
    // So what the wire needs is the key's POSITION, which the same walker that found it gives us.
    let Some(store) = me_store else {
        return GoAction::Refuse(None);
    };
    let Some((bag_index, slot, key_guid)) = crate::ui_items::find_item(
        &store.0,
        &inputs.objects,
        key_entry,
        crate::ui_items::ItemSearch::default(),
    ) else {
        // Held a moment ago (the resolver said so) and gone now — nothing to send.
        return GoAction::Refuse(None);
    };
    // The template is ask-once — a miss queries and does nothing this click, exactly like the
    // toast's own name miss. `use_spell_index` is the BLOCK ordinal, the packet's third byte.
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
        // The bound lock — `CGItem::Use`'s own target argument (decision 0769).
        on_object: Some(guid),
        is_charter: tmpl.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
    })
}

/// The client-local toast for an unopenable lock — the ref's routing (`0x5f3427..`), transcribed
/// (decision 0545). Two layers, exactly as the binary orders them:
///
/// 1. **`GO_FLAG_LOCKED` set** (a padlocked chest/door — gather nodes never set it): the `usable`
///    gate refuses with the strategy default `[strat+8]` before the rich routing ever runs —
///    DOOR "The door is locked." / BUTTON "That has already been used." / else "Item is locked."
/// 2. Flag clear: route by **Lock.dbc slot 0** — key item missing → "Requires <item>" (`0xde`),
///    skill spell unknown → "Requires <LockType.Name>" (`0xdf`, the "Requires Herbalism" case),
///    known but under-rank → "Requires <name> <rank>" (`0xe0`, rank = `Skill[0]`, else
///    GO-level×5), slot-0 type neither → "You can't open that." (`0xda`).
///
/// The `"UNKNOWN"` literal is the ref's own missing-LockType-row fallback (`0x838044`). The
/// `0xd9` chest-in-use pre-check (`0x5f81d0`) is unmodeled — its state is generally unreachable
/// through our highlightable gate (`GO_FLAG_IN_USE` already excludes the busy chest).
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

/// Which leg the reference's unit interact dispatcher `0x60bea0` takes on a **dead** target — its
/// `0x60bf75` fork, transcribed. Pure, because every conjunct in it is a fact about two units'
/// wire state and nothing else, and because the conjunct this project was missing is invisible
/// from the outside (see [`DeadUnitLeg::Nothing`]).
///
/// The reference's order, and ours:
///
/// 0. **`0x60bf75`/`0x60bf86` — is it a corpse at all?** `HEALTH <= 0` **and** the target's
///    `UNIT_DYNFLAG_DEAD` (`[+0x224]` bit 5, the feign-death bit — decision 1022) **clear**; either
///    failing routes to the alive branch `0x60c162` instead. The caller passes this as `dead`.
/// 1. **`0x60bf98` — am I mounted?** `mov eax,[ecx+0x1fc]` on the *player's* descriptor block
///    (`UNIT_FIELD_MOUNTDISPLAYID`, the one mounted signal — decision 0481), `jg 0x60c01f`. A
///    rider does not loot. It is a jump into the skin leg, not a refusal: nothing is sent and
///    nothing is said.
/// 2. **`0x6003a0` — is it lootable?** (`UNIT_DYNFLAG_LOOTABLE`, plus the decay deadline the server
///    owns.) Yes ⇒ [`DeadUnitLeg::Loot`] and `CMSG_LOOT`.
/// 3. **`0x60c01f` — is it skinnable?** `UNIT_FIELD_FLAGS` bit 26, on the TARGET, plus the
///    learn-time latch `[0xb700e4]` that says we know a Skinning spell (decision 0752). Yes ⇒
///    [`DeadUnitLeg::Skin`] and the skin cast — which, being a cast, meets the mounted gate's cast
///    face (`0x6094f0`, reason `0x39`) and is where a rider finally gets told something.
/// 4. Otherwise nothing at all (`0x60c25f`, the bare epilogue).
///
/// Note what step 1 does **not** do: it does not consult the target. A lootable-*and*-skinnable
/// corpse — an already-looted one, which is the common case for a skinner — routes to Skin while
/// mounted and to Loot on foot, off the same two units.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DeadUnitLeg {
    /// `CMSG_LOOT` (`0x60bff7` → `0x5df2a0`).
    Loot,
    /// The skin cast (`0x60c082` → `0x5f05e0`).
    Skin,
    /// **The silent leg.** No packet, no message, no state write — the dispatcher just returns.
    /// A mounted click on an ordinary lootable corpse lands here, and that silence is why the
    /// missing gate looked like working code: the loot window simply opened, as it would on foot.
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
/// **Which branch of `0x60bea0` a right-click on a unit takes** — the dispatcher's own top-level
/// fork, named so it cannot be fallen out of.
///
/// The reference splits on the target's death *first* and never rejoins: the dead side runs
/// [`dead_unit_leg`]'s three-way ladder and returns, and the NPC-service dispatch hangs off the
/// ALIVE side alone (`0x60c162`'s `CanInteract` → the service send). An `if/else if` chain that
/// ends in the service arm gets that wrong in one specific, invisible way — a dead target whose
/// ladder chose [`DeadUnitLeg::Nothing`] slides into the service arm — which is exactly the bug
/// decision 1858 was written for. This enum exists so the compiler, not a comment, is what keeps
/// the two sides apart: the call site matches it exhaustively, so a new [`DeadUnitLeg`] variant
/// cannot silently acquire a service send.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UnitBranch {
    /// The Attack cursor's own leg — `0x60c247 call 0x5ecb70` (select, then swing).
    Attack,
    /// The dead fork (`0x60bf75`), and its chosen leg. **Terminal in every variant.**
    Dead(DeadUnitLeg),
    /// The alive branch (`0x60c162`): `CanInteract`, then the NPC-service packet.
    Service,
}

/// [`UnitBranch`], from the two facts that pick it. `attack` first, matching the chain the
/// dispatcher's callers see: the Attack cursor kind is only ever classified for a LIVE hostile,
/// so the order is a formality on real data and a defined answer on impossible data.
fn unit_branch(attack: bool, dead_fork: bool, leg: DeadUnitLeg) -> UnitBranch {
    if attack {
        UnitBranch::Attack
    } else if dead_fork {
        UnitBranch::Dead(leg)
    } else {
        UnitBranch::Service
    }
}

/// **The reference's own NPC-service ladder** — `0x5f0130`'s first-match-wins walk over
/// `UNIT_NPC_FLAGS`, low bit to high. The winning bit, **not** a cursor kind.
///
/// The cursor classifier `0x482200` runs a second, structurally identical ladder over the same
/// field in the same order, which is why keying the send on the classified kind looked right for
/// so long. It is not: the projection is lossy. Speak(6) is bits 0, 1, 5, 6, 9, 10, 11 and 13 —
/// eight arms, two of which send nothing — and Buy(3) is bits 8 and 12. A kind-keyed dispatch
/// cannot express this ladder, and benilla's sent `CMSG_GOSSIP_HELLO` for all eight Speak arms
/// (decision 1861).
///
/// **First-match-wins is load-bearing**: a GOSSIP+VENDOR NPC sends `CMSG_GOSSIP_HELLO` only, and
/// a stable master with a menu keeps the menu — which is where 1677's hand-written
/// `STABLEMASTER && !GOSSIP` conjunct came from. The bit order gives it for free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ServiceArm {
    /// bit 0 — `0x5f02a4` → `0x5df4d0`.
    Gossip,
    /// bit 1, **and** the target's cached questgiver status ∉ {0, 1} (`0x5f02c0` → `0x5df490`).
    Questgiver,
    /// bit 2 — `0x5f0317` → `0x5df5d0`.
    Vendor,
    /// bit 3 — `0x5f034e` → `0x5ed020`.
    FlightMaster,
    /// bit 4 — `0x5f0385` → `0x5df680`.
    Trainer,
    /// bit 5 — `0x5f03bc` → `0x5df730`. **Ghost-gated, and sends nothing.**
    SpiritHealer,
    /// bit 6 — `0x5f03f3` → `0x5df950`. **Ghost-gated.**
    SpiritGuide,
    /// bit 7 — `0x5f042a` → `0x5dfdc0`. **Sends nothing.**
    Innkeeper,
    /// bit 8 — `0x5f0461` → `0x5dffe0`.
    Banker,
    /// bit 9 — `0x5f04e3` → `0x5e0060`.
    Petitioner,
    /// bit 10 — `0x5f051a` → `0x5e00e0`.
    TabardDesigner,
    /// bit 11 — `0x5f0551` → `0x5e01a0`.
    Battlemaster,
    /// bit 12 — `0x5f0588` → `0x5e0220`.
    Auctioneer,
    /// bit 13 — `0x5f05bc` → `0x5e02a0`.
    StableMaster,
}

/// The ladder itself. `None` = no consulted bit set (`0x5f05ca`) — the click does nothing, and
/// takes no talk gesture with it. Bit 14 (REPAIR) has no arm in the binary and none here.
///
/// The reference re-reads the flags field for a redundant `bits 9 AND 10` block before the plain
/// bit-9 test; it routes to the same handler bit 9 alone reaches, so it is dead in effect and is
/// not transcribed (recording it would be a distinction with no outcome).
pub(crate) fn service_arm(npc_flags: u32, quest_status: Option<u32>) -> Option<ServiceArm> {
    use cursor_mode::npc_flags as f;
    let bit = |m: u32| npc_flags & m != 0;
    Some(if bit(f::GOSSIP) {
        ServiceArm::Gossip
    } else if bit(f::QUESTGIVER) && cursor_mode::questgiver_has_quest(quest_status) {
        // The SAME predicate the cursor ladder uses at `0x482362`, shared rather than copied so
        // the two can never disagree about which questgiver is worth talking to.
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

/// What a taken [`ServiceArm`] does. Three arms are not a packet at all.
pub(crate) enum ServiceAction {
    /// The arm's own opcode.
    Send(ClientCommand),
    /// The vendor arm's **cursor fork** (`0x5df5d7`): an item is on the cursor, so this click
    /// sells it rather than opening the list — send, then take the cursor through the sell clear
    /// ([`benilla_ui::script::UiScript::take_cursor_item_for_sale`]).
    SellFromCursor(ClientCommand),
    /// Raise `CONFIRM_BINDER` locally and send nothing (`0x5dfdc0`).
    AskBinder,
    /// Raise `CONFIRM_XP_LOSS` locally and send nothing (`0x5df730`).
    AskSpiritHealer,
    /// Adopt this spirit guide as the current-area spirit healer (`0x5df950` → two calls into
    /// `0x4921c0`, the second of which always transmits) — `CMSG_AREA_SPIRIT_HEALER_QUERY`.
    AcquireSpiritGuide,
    /// Nothing goes out. The payload is why, for the debug line.
    Silent(&'static str),
}

/// One taken arm → what benilla does about it.
///
/// `cursor_sale` is the vendor fork's input: the guid of the **mode-1 item on the cursor**, or
/// `None` for an empty cursor or any other payload — benilla's resolution of the reference's
/// `GetCursorItem 0x494c60`, which returns `[0xb4e248]/[0xb4e24c]` only while `[0xb4d900] == 1`.
///
/// **The two "spirit" arms are ghost-gated at entry** (`0x5df74a` / `0x5df962`, the byte-identical
/// `PLAYER_FLAGS` bit 4 test): a LIVING player who right-clicks a spirit healer or spirit guide
/// carrying no gossip bit gets nothing at all — no packet, no event, no error. That is not an
/// omission here, it is the reference's own answer.
///
/// The `TabardDesigner` and `Battlemaster` arms send their own opcodes (`0x5f051a`'s
/// `MSG_TABARDVENDOR_ACTIVATE`, `0x5f0551`'s `CMSG_BATTLEMASTER_HELLO`) since decision 1977 —
/// 1861 had them on the universal gossip greeting while neither window existed; the battleground
/// list (1974) and the tabard designer (1977) answer the replies now. The tabard arm's one gate
/// — the shapeshift refusal `ERR_EMBLEMERROR_NOTABARDGEOSET` when the player's displayed model is
/// not the native one, and the same-vendor no-op — is named, not built (1977's record).
pub(crate) fn service_action(
    arm: ServiceArm,
    guid: u64,
    ghost: bool,
    cursor_sale: Option<u64>,
) -> ServiceAction {
    match arm {
        ServiceArm::Gossip => ServiceAction::Send(ClientCommand::GossipHello { guid }),
        ServiceArm::Questgiver => ServiceAction::Send(ClientCommand::QuestgiverHello { npc: guid }),
        // **The vendor arm is a fork, not a send** (`0x5df5d0`). `0x5df5d7 call 0x494c60` is
        // `GetCursorItem` — the cursor's own payload guid, answered for **mode 1 only** — and a
        // non-zero answer takes the arm to `CMSG_SELL_ITEM 0x1a0` instead of the list. Nothing on
        // either leg tests the merchant window, so this sells to a vendor whose window was never
        // opened; the re-click gate above is what keeps it from firing when it IS open.
        ServiceArm::Vendor => match cursor_sale {
            Some(item_guid) => ServiceAction::SellFromCursor(ClientCommand::SellItem {
                vendor: guid,
                item_guid,
                // `xor ecx,ecx` at `0x5df5ee` — the body's `u8 count`, and the client only ever
                // writes the zero here. Whole stack (vmangos `HandleSellItemOpcode` corroborates
                // the meaning; the byte is the fact).
                count: 0,
            }),
            None => ServiceAction::Send(ClientCommand::ListInventory { guid }),
        },
        ServiceArm::FlightMaster => ServiceAction::Send(ClientCommand::TaxiQueryNodes { guid }),
        ServiceArm::Trainer => ServiceAction::Send(ClientCommand::TrainerList { trainer: guid }),
        ServiceArm::SpiritHealer if ghost => ServiceAction::AskSpiritHealer,
        ServiceArm::SpiritHealer => ServiceAction::Silent("spirit healer, and we are alive"),
        // Ghost-gated like its neighbour. The arm sends `CMSG_AREA_SPIRIT_HEALER_QUERY 0x2E2`
        // from one call deeper (`0x5df950` → `0x4921c0`), and it does it through the *same*
        // "set current area spirit healer" routine the per-frame proximity poll uses — with a
        // deliberate `(0,0)` cache-bust first, so a click on the guide you are already standing
        // next to re-asks for the wave clock instead of being swallowed by the routine's
        // unchanged-guid early return.
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

/// Drain the ESC chain's `ClearTarget()` (the ref's ESC ladder's last close/cancel leg (`ToggleGameMenu` — the ref's
/// `ToggleGameMenu` order, `UIParent.lua:1492`) and commit the deselect. The target drops ONLY
/// when nothing earlier in the chain ate the press: a mid-cast ESC cancels the cast instead, an
/// open window closes instead — the two-press behavior the raw-key clear this replaces couldn't
/// express (it ran beside the chain, so the first ESC both canceled the cast AND dropped the
/// target — the director's 0449 report). EditBox precedence rides the chain too: a focused box
/// consumes ESCAPE before `ToggleGameMenu` ever runs.
pub(super) fn clear_target_requests(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
    mut guid_asks: MessageReader<DeselectGuid>,
) {
    // The engine-side teardown ask (`0x493910(guid, 1)`): a no-op unless the selection IS that
    // guid — the loot window's move-start close is its one producer today (decision 2097).
    let asked = guid_asks.read().any(|ask| selection.guid == Some(ask.0));
    let Some(mut script) = script else {
        if asked {
            clear(&mut selection, &mut seam, !engaged.is_empty());
        }
        return;
    };
    // Both drained every frame: a `||` that short-circuited on `asked` would leave the VM's
    // ClearTarget flag armed for the next frame and clear whatever was targeted by then.
    let vm_clear = script.take_target_clear();
    if asked || vm_clear {
        clear(&mut selection, &mut seam, !engaged.is_empty());
    }
}

/// Ask for the selection teardown **if the selection is this guid** — the reference's
/// `CGGameUI::SelectionTeardown 0x493910(guid, ecx=1)`, which no-ops unless a selection exists
/// and equals the argument. The loot window's move-start close raises it for a dead corpse
/// (`0x48f369`, decision 2097); drained by [`clear_target_requests`], so the attack-stop and the
/// wire clear stay the teardown's.
#[derive(bevy::ecs::message::Message, Clone, Copy, Debug)]
pub(crate) struct DeselectGuid(pub(crate) u64);

/// Drain the UI's **selection asks** — `TargetUnit(token)`, `AssistUnit(token)` and
/// `TargetLastEnemy()` — and commit each through the shared SetSelection path ([`scan::commit`]).
///
/// One drain for the three because the reference has one function for them: `TargetUnit
/// 0x4899d0`, `AssistUnit 0x489b80`, `TargetLastEnemy` and `TargetLastTarget` all reach selection
/// through the "select if it resolves" helper `0x489a40`, whose three arms — resolves → commit;
/// doesn't resolve but is on the roster → commit anyway; **neither, including guid 0 → a bare
/// `ret`** — are the same for every caller. That third arm is the one worth naming: a token,
/// basis or remembered guid that does not resolve is a **no-op, not a deselect**. Draining them
/// in one ordered queue also keeps their relative order, which a macro can observe.
///
/// `TargetUnit` callers: the player frame's left-click (`TargetUnit("player")`), the party frames'
/// (`TargetUnit("partyN")`, decision 0434 phase 5), the TARGETSELF binding. Only tokens resolving
/// to a STREAMED unit act: `"player"` → our avatar; `"target"` → the current selection (a dedup
/// no-op); `"partyN"`/`"raidN"` → that roster slot when its entity is in range (an out-of-range
/// member needs the guid-only selection the phase-4 out-of-range slice owns — until then the click
/// no-ops, like the real client on a nonexistent unit); `"pet"` → the bar's cached pet guid
/// (decision 0990, the pet frame's left click); `"targettarget"` → the selection's own
/// `UNIT_FIELD_TARGET` (decision 1576, the ToT frame's). Everything else (mouseover/name) waits
/// for its wire.
///
/// `AssistUnit` resolves the same token and then runs the shared assist tail
/// ([`super::by_name::SelectCommit::assist`]) — the basis's own `UNIT_FIELD_TARGET`, which is the
/// hop `"targettarget"` already makes for the ToT frame. `TargetLastEnemy` skips the token
/// grammar entirely and reads [`scan::LastEnemy`].
pub(super) fn selection_requests(
    script: Option<NonSendMut<UiScript>>,
    // The one unit-token resolver (`crate::ui_unit::UnitTokens`) — the arms this drain used to
    // spell out inline. It is shared with the reach feed precisely so `TargetUnit("target")` and
    // `CheckInteractDistance("target", …)` can never mean two different units (B304).
    tokens: crate::ui_unit::UnitTokens,
    // `TargetLastEnemy`'s memory — `[0xb4e2e8]/[0xb4e2ec]`, stamped by `scan::remember_last_enemy`.
    last_enemy: Res<scan::LastEnemy>,
    // The one SetSelection tail, shared with `/target` and `/assist` (decision 1583). It carries
    // the classification too, which is why this drain no longer states one: hand-stating it here
    // is exactly how a `false` that the binary refutes got written down.
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
            // Resolved before the commit borrows the selection mutably. An unstreamed unit — an
            // out-of-range party member, a despawned pet — resolves to nothing and no-ops.
            SelectionRequest::Unit(token) => {
                if let Some((entity, guid)) = tokens.resolve(&token, &commit.selection) {
                    commit.commit(entity, guid);
                }
            }
            // `0x489ba9 call 0x515940(token)` — the same token grammar, then the shared tail. An
            // unresolvable token is silent here: the reference emits game-message `0xb8`, whose
            // id→string table is runtime-populated BSS with no statically recoverable text — the
            // known deviation `TargetByName` already carries on this path.
            SelectionRequest::Assist(token) => match tokens.resolve(&token, &commit.selection) {
                Some((basis, _)) => commit.assist(basis, "AssistUnit"),
                None => info!("assist (AssistUnit): \"{token}\" names nothing; silent no-op"),
            },
            // `0x489c40` — the `/assist Bob` handler's verb: a player NAME, resolved by the
            // app's name scan (prefix-ok, players only) the way the slash line always was.
            SelectionRequest::AssistByName(name) => {
                assist_by_name.write(super::by_name::AssistRequest { name: Some(name) });
            }
            // `0x489b45` reads the last-attackable pair and hands it to the same `0x489a40`.
            // **Empty memory is a no-op**: the shim `0x489b40` is thirteen bytes with no
            // emptiness test at all, where `TargetLastTarget 0x489b00` really does reach
            // `0x493540(0,0)` and deselect. The two shims differ exactly here.
            SelectionRequest::LastEnemy => {
                let Some(guid) = last_enemy.0 else {
                    info!("TargetLastEnemy: nothing hostile has been targeted yet; no-op");
                    continue;
                };
                match tokens.held(guid) {
                    Some((entity, guid)) => commit.commit(entity, guid),
                    // The stale-guid arm: the remembered unit has streamed out or despawned. The
                    // memory is deliberately kept (the reference never clears its globals either)
                    // and this is a bare no-op, never a deselect.
                    None => info!(
                        "TargetLastEnemy: {guid:#x} is no longer streamed; target left untouched"
                    ),
                }
            }
        }
    }
}

/// Drop the current target and tell the server (`CMSG_SET_SELECTION` guid 0). A no-op when nothing is
/// selected, so it never sends a redundant clear. Losing the target also ends melee auto-attack when
/// one is running (`engaged`, our server-echoed [`Engaged`]): `CMSG_ATTACKSTOP` — the ref stops
/// swinging and drops the attack stance on Esc/click-off/target-death alike (the stance itself falls
/// when the `SMSG_ATTACKSTOP` echo removes [`Engaged`]). Weapons *stay drawn* — combat never stows.
pub(super) fn clear(
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    engaged: bool,
) {
    if selection.target.take().is_some() {
        selection.guid = None;
        let _ = seam.net.0.send(ClientCommand::SetSelection { guid: 0 });
        // `SetSelection 0x493540`'s own `0x493a08 call 0x5ecac0` — the real StopAttack, so
        // losing the target also un-queues a pending on-next-swing strike. It was a bare
        // `CMSG_ATTACKSTOP` before the seam existed.
        seam.stop(engaged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `0x5f3427..` toast routing, case by case (decision 0545). The slot/flag/type
    /// combinations mirror real data: Peacebloom (lock 29: skill slot, LockType 2, Skill 0), a
    /// rank-155 vein (lock 42: LockType 3, Skill 155), a keyed door, a padlocked chest.
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
        // A missing LockType row fills the ref's literal fallback (`0x838044`).
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
        // Key lock, key absent + named → 0xde "Requires %s" with the item name; the template
        // miss is silent, like the ref (a key we DO hold never reaches the toast at all — the
        // resolver returns `OpenByKey` and the click casts it, decision 0752).
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
        // GO_FLAG_LOCKED set → the strategy default REPLACES the rich routing (`usable`'s lock
        // check, `0x5f32a6`): door 0xdc, button 0xdd, chest/else 0xdb — even on a skill lock.
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

    /// The dead-target fork of `0x60bea0`, and the conjunct that was missing: **the rider does not
    /// loot** (`0x60bf98`). The director found it from the saddle, over a Young Wolf, with the loot
    /// window open — the one shape of bug that never looks like one from inside the client.
    #[test]
    fn a_mounted_player_never_takes_the_loot_leg() {
        // On foot, a lootable corpse loots. This is the control: the fix must not touch it.
        assert_eq!(
            dead_unit_leg(false, true, true, false, false),
            DeadUnitLeg::Loot
        );
        // Mounted, the same corpse: the loot leg is skipped and, with nothing to skin, the
        // dispatcher returns having done nothing. Silent — no packet and no red line.
        assert_eq!(
            dead_unit_leg(true, true, true, false, false),
            DeadUnitLeg::Nothing
        );
        // Mounted over a corpse that is BOTH lootable and skinnable (an already-looted body a
        // skinner rides up to): the fall-through reaches the skin leg, which `cursor.kind` alone
        // could never have routed — the classifier names Skin only on an unlootable corpse.
        assert_eq!(
            dead_unit_leg(true, true, true, true, true),
            DeadUnitLeg::Skin
        );
        // ...and on foot the very same body loots instead. Step 1 never consults the target.
        assert_eq!(
            dead_unit_leg(false, true, true, true, true),
            DeadUnitLeg::Loot
        );
    }

    /// The rest of the fork's ladder, so the mounted conjunct above cannot be "fixed" by
    /// collapsing a leg it shares with the others.
    #[test]
    fn the_dead_fork_keeps_its_other_three_gates() {
        // A live unit is not this fork's business at all (`0x60bf75 jg` → the alive branch).
        assert_eq!(
            dead_unit_leg(false, false, true, true, true),
            DeadUnitLeg::Nothing
        );
        // Dead, unlootable, skinnable, and we know the trade → Skin (`0x60c01f`).
        assert_eq!(
            dead_unit_leg(false, true, false, true, true),
            DeadUnitLeg::Skin
        );
        // The learn-time latch `[0xb700e4]` is the leg's second precondition (0752): a
        // non-skinner gets nothing on the same corpse.
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
    /// **The dead fork is TERMINAL** — decision 1858, and the reason a mounted right-click on a
    /// Young Wolf answered *"That merchant doesn't like you."*
    ///
    /// [`dead_unit_leg`] was right; where its `Nothing` WENT was not. The dispatcher's arms were
    /// an `if/else if` chain ending in the NPC-service send, so a dead target that took no leg
    /// slid into the arm that dispatches on the CURSOR kind — and a lootable corpse's cursor is
    /// `Pickup`, the very mode a vendor shows. The click sent `CMSG_LIST_INVENTORY` at the
    /// corpse, and vmangos answers a non-vendor with `SELL_ERR_CANT_FIND_VENDOR`, which is
    /// `ERR_VENDOR_HATES_YOU`. Nothing about that was visible in [`dead_unit_leg`], which is why
    /// 1851's two tests passed over it: they asserted the LEG, and the bug was in the ROUTING.
    ///
    /// So this asserts the property rather than the instance — over a dead target, no combination
    /// of the fork's four inputs may produce [`UnitBranch::Service`].
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
        // The exact shape the director hit: mounted, over a lootable corpse with no skinning.
        let leg = dead_unit_leg(true, true, true, false, false);
        assert_eq!(
            unit_branch(false, true, leg),
            UnitBranch::Dead(DeadUnitLeg::Nothing)
        );
        // (What that used to fall into: the pouch over a corpse and the pouch over a vendor are
        // one cursor mode, so the old last arm read the corpse as a shop. The dispatch no longer
        // consults a cursor kind at all — 1861 — but the fork's terminality is the guarantee that
        // does not depend on that, so it is the one asserted here.)
        // A LIVE unit still reaches the service dispatch — the fix must not shut the door on the
        // branch that is supposed to send.
        assert_eq!(
            unit_branch(false, false, DeadUnitLeg::Nothing),
            UnitBranch::Service
        );
    }

    /// **The ladder, bit by bit** — decision 1861. Every arm of `0x5f0130`'s first-match-wins walk
    /// over `UNIT_NPC_FLAGS`, in the order the binary tests them.
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
        // No consulted bit — REPAIR (bit 14) has no arm in the binary, so a repair-only NPC does
        // nothing at all. Neither does an empty field.
        assert_eq!(service_arm(0x4000, None), None);
        assert_eq!(service_arm(0, None), None);
    }

    /// **First-match-wins, which is the half a kind-keyed dispatch could not express.** The two
    /// collisions that made the old proxy wrong are the two this pins.
    #[test]
    fn the_service_ladder_is_first_match_wins() {
        use cursor_mode::npc_flags as f;
        // A gossip-flagged anything keeps its menu — this is where 1677's hand-written
        // `STABLEMASTER && !GOSSIP` conjunct came from, now free.
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
        // Banker (bit 8) before auctioneer (bit 12) — both were Buy(3) to the cursor.
        assert_eq!(
            service_arm(f::BANKER | f::AUCTIONEER, None),
            Some(ServiceArm::Banker)
        );
        // Trainer (bit 4) before innkeeper (bit 7) — both were Speak/Interact to the cursor.
        assert_eq!(
            service_arm(f::TRAINER | f::INNKEEPER, None),
            Some(ServiceArm::Trainer)
        );
    }

    /// **The three arms that are not a packet**, and the packets the other eleven actually send.
    /// The eight arms that used to collapse into `CMSG_GOSSIP_HELLO` are the point of this test.
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
        // The innkeeper asks, mounted or not, alive or not — and sends nothing.
        assert_eq!(sent(ServiceArm::Innkeeper, false), "ask-binder");
        // The two ghost-gated arms: nothing at all for a living player, which is the reference's
        // own answer and not an omission.
        assert_eq!(sent(ServiceArm::SpiritHealer, false), "silent");
        assert_eq!(sent(ServiceArm::SpiritGuide, false), "silent");
        assert_eq!(sent(ServiceArm::SpiritHealer, true), "ask-spirit-healer");
        // The two arms 1861 had on the greeting send their own opcodes since 1977.
        assert!(matches!(
            service_action(ServiceArm::TabardDesigner, 0x42, false, None),
            ServiceAction::Send(ClientCommand::TabardVendorActivate { npc: 0x42 })
        ));
        assert!(matches!(
            service_action(ServiceArm::Battlemaster, 0x42, false, None),
            ServiceAction::Send(ClientCommand::BattlemasterHello { npc: 0x42 })
        ));
    }

    /// **The vendor arm is a fork, and only the vendor arm** — decision 1914.
    ///
    /// `0x5df5d0` asks `GetCursorItem 0x494c60` before anything else, and a mode-1 item on the
    /// cursor sends `CMSG_SELL_ITEM` instead of `CMSG_LIST_INVENTORY`. What this pins is the
    /// blast radius: the held item must change the **vendor** arm and **nothing else**, because
    /// the fork lives inside that one handler and every other arm is reached without ever
    /// consulting the cursor. A cursor read that leaked into the ladder would sell an item at a
    /// trainer.
    #[test]
    fn only_the_vendor_arm_reads_the_cursor() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        const ITEM: u64 = 0x4000_0000_0000_0099;

        // Empty cursor: the plain list open, exactly as before.
        assert!(matches!(
            service_action(ServiceArm::Vendor, VENDOR, false, None),
            ServiceAction::Send(ClientCommand::ListInventory { guid: VENDOR })
        ));
        // An item held: the sale, addressed to the CLICKED npc, count 0 = whole stack — and the
        // variant that carries the cursor clear with it.
        assert!(matches!(
            service_action(ServiceArm::Vendor, VENDOR, false, Some(ITEM)),
            ServiceAction::SellFromCursor(ClientCommand::SellItem {
                vendor: VENDOR,
                item_guid: ITEM,
                count: 0,
            })
        ));
        // Every other arm answers the same held or not. `SellFromCursor` may not appear anywhere
        // but the vendor arm — that is the property, asserted over the whole enum rather than
        // over the arms someone remembered to list.
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

    /// **The re-click gate, and the wiring it rests on** — decision 1905.
    ///
    /// The comparison itself is one `==`; what could actually be wrong is the *latch*. The
    /// reference arms `[0xb4e2d0]` from fourteen window openers and from nothing on the click
    /// path, so the gate is only ever "a window is open on this NPC" — and benilla reads that
    /// state off [`crate::ui_session::feed_interact_npc`]'s collapse rather than a latch of its
    /// own. So this drives the real feed and asserts the property that makes the transcription
    /// sound: **nothing arms it until a window opens, every NPC window arms it, and the arm names
    /// the window's own NPC.**
    ///
    /// The first click must always get through — that is the half a wrong latch would break, and
    /// it is asserted first.
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

        // Nothing open: the FIRST click on any NPC reaches the ladder.
        let idle = armed(&|_| {});
        assert!(!interaction_already_open_on(NPC, &idle));
        assert!(!interaction_already_open_on(OTHER, &idle));

        // Each NPC window arms the token with its own NPC — one representative per opener family
        // (a menu, a quest panel, a specialized window), which is the shape the gate depends on.
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
            // The re-click on THAT npc is eaten…
            assert!(
                interaction_already_open_on(NPC, armed),
                "{what}: the re-click was not suppressed"
            );
            // …and a click on the NPC beside him is not. A gate that swallowed this would make
            // every neighbouring vendor unclickable while a window stood.
            assert!(
                !interaction_already_open_on(OTHER, armed),
                "{what}: the gate ate a click on a different NPC"
            );
        }

        // Closing the window disarms it — the reference's `0x493310` zeroes the pair, and ours is
        // a collapse, so a cleared session must read through as "clickable again".
        let closed = armed(&|w| {
            let mut t = crate::ui_trainer::TrainerOpen::default();
            t.open(NPC, 0, Vec::new(), String::new());
            t.clear();
            w.insert_resource(t);
        });
        assert_eq!(closed.1, None);
        assert!(!interaction_already_open_on(NPC, &closed));
    }

    /// **The nothing-leg deselect, and the two object hits that are not it** — `select_on_click`'s
    /// `_` arm.
    ///
    /// The reference's deselect lives in the terrain and *nothing* legs of the world click; an
    /// **object** leg clears no selection, whatever it then decides to do with the object. Two
    /// picks reach this arm carrying no `target`: a **corpse** (its guid is in the other slot,
    /// decision 1723) and a **refused** one (a `NOT_SELECTABLE` unit the hover grader threw away,
    /// decision 2060 — the click's own down-edge pick `0x481f00` still found the object, so the
    /// release takes the object leg `0x4925d0` and its tail `0x493540` merely refuses to select).
    /// Reading either as "clicked the sky" would drop the player's target: click a body mid-fight,
    /// or click the flagged boss you are standing next to, and your target evaporates.
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
            // The clean left-click the player controller emits.
            world
                .resource_mut::<Messages<WorldClick>>()
                .write(WorldClick);
            world
                .run_system_once(select_on_click)
                .expect("select_on_click runs as a one-shot system");
            world.resource::<Selection>().guid
        };

        // Empty world: the deselect arm, which is the behaviour the guards are carved out of.
        assert_eq!(
            click(Hovered::default()),
            None,
            "clicking the sky deselects"
        );
        // A corpse.
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
        // A refused unit.
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

    /// **The selection queue, end to end** — Lua in, `Selection` out — over the four ways it is
    /// reachable from a player macro. Every one of them is a shipped binding body: `AssistUnit`
    /// is ASSISTTARGET's (default `F`), `TargetLastEnemy` is TARGETLASTHOSTILE's (default `G`).
    ///
    /// What it pins, claim by claim:
    /// * `AssistUnit("target")` on a basis that has a target selects **that unit** — the shared
    ///   assist tail's one hop off `UNIT_FIELD_TARGET`;
    /// * a basis with **no** target is a silent no-op — the reference's tail bails before any send;
    /// * a **garbage token** resolves to nothing and is a no-op, never a deselect (`0x489a40`'s
    ///   arm 3 is a bare `ret`) — and, being reachable from any macro, must not panic;
    /// * `TargetLastEnemy()` re-selects the remembered guid, and with a **stale** one — the unit
    ///   despawned or streamed out — leaves the target exactly where it was.
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

        // Assist the current target: one hop onto ITS target.
        set(&mut world, Some((basis, BASIS)));
        assert_eq!(run(&mut world, r#"AssistUnit("target")"#), Some(VICTIM));

        // The victim targets nobody — assisting it is a silent no-op, not a deselect.
        assert_eq!(run(&mut world, r#"AssistUnit("target")"#), Some(VICTIM));

        // A token that names nothing: a no-op, and above all not a panic.
        assert_eq!(run(&mut world, r#"AssistUnit("nosuchunit")"#), Some(VICTIM));
        assert_eq!(run(&mut world, r#"TargetUnit("nosuchunit")"#), Some(VICTIM));

        // TargetLastEnemy with nothing remembered: also a no-op.
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));

        // Remember the victim, drop the target, and press it: back onto the victim.
        world.resource_mut::<scan::LastEnemy>().0 = Some(VICTIM);
        set(&mut world, None);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));

        // A STALE memory — the unit despawned, so the guid is in no index. The target stays put;
        // the reference's helper returns without touching the selection.
        world.resource_mut::<scan::LastEnemy>().0 = Some(GONE);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));
        set(&mut world, None);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), None);
    }
    use crate::net::{ClientCommand, NetCommands, ObjectStore};
    use bevy::ecs::system::RunSystemOnce;

    const F_HEALTH: u16 = 22;
    const F_MAXHEALTH: u16 = 28;
    /// `GAMEOBJECT_TYPE_ID`, absolute field 21 — the store the GO arms fork on.
    const GO_TYPE_FIELD: u16 = 21;
    const BOAR: u64 = 0xB0A2;
    const ME: u64 = 0x5E1F;

    fn store(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs))
    }

    /// Everything [`act_on_right_click`] and the commit under it reach for, and nothing else.
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

    /// **A right-click that began on a V-plate acts on that plate's unit** — the bug the director
    /// reported as "right click on nameplates no longer works", and the reason
    /// [`act_on_right_click`] reads [`PressPick`] (decision 2230).
    ///
    /// The gesture it reconstructs is the whole of it: the plate published the mouseover at the
    /// press, the press engaged freelook (the camera looks *through* a plate — 2159), freelook
    /// handed the plates' mouse back (`0x60f830`) so the plate's own `OnClick` never fires and its
    /// hover goes dark, and `update_hover` blanks the live [`Hovered`] for the whole session. What
    /// survives all of that is the press latch — and a plate hover leaves **no body under the
    /// cursor** for the release frame's re-pick to land back on, which is exactly why the live
    /// hover carried the body case and dropped this one.
    #[test]
    fn a_right_click_begun_on_a_plate_acts_on_the_plates_unit() {
        let (mut world, boar) = right_click_world();
        // The press: the plate's unit is the mouseover (distance 0.0 — topmost UI), classified
        // Attack. Nothing here is live state; it is the latch, exactly as `latch_press_pick` wrote
        // it one frame before the look session began.
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
    /// **A meeting stone is JOINED, not USEd** (decision 2283). The GO type's own use slot
    /// (`0x5f69d0`) replaces the shared `CMSG_GAMEOBJ_USE` sender, so the click must hand the
    /// object to the join validator and put **no** `0xB1` on the wire — which is what left every
    /// stone dead, since vmangos' `GameObject::Use` has an explicit do-nothing arm for type 23.
    ///
    /// The falsification is the assertion that matters: with the arm removed this test sees a
    /// `GameObjUse` command and no `MeetingStoneUse` message.
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

    /// Out of interact range the click is suppressed with no toast and no packet, exactly as the
    /// shared arms are (the reference auto-walks instead; `0x610300`, no packet).
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
