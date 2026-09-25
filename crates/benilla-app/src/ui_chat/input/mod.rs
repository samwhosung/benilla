//! The **submitted-line** side of the chat (decision 0288 P5) — what we DO about a line.
//! [`drain_chat_input`] routes each Entered line: a plain line sends as the box's CURRENT type (the
//! edit machine's law, [`super::edit`]); a `/`-line runs the Enter-path type switch, the reply arm,
//! then the action table. What a line *means* is [`parse`]'s ([`parse_line`] → [`ParsedChat`]);
//! which strings reach which arm is [`super::commands`]'s table.
//!
//! The emote arm is the client's `DoEmote` (`0x5ef560`) in full: the eligibility gate
//! ([`emote_send_eligible`]), the asleep gate, the stow, the **posture** branch that makes `/sit`
//! sit, then `CMSG_TEXT_EMOTE`. Opening keys + live parsing live in [`super::edit`]; the inbound
//! half is [`super::feed`]. Own sends are never echoed locally — the wire echoes back (vanilla
//! behavior).

use bevy::prelude::*;

mod parse;
#[cfg(test)]
pub(super) use parse::lua_quoted_string;
pub(super) use parse::{parse_line, ParsedChat};

use crate::creature_anim::{move_flags, MovementState};
use crate::net::{ClientCommand, NetCommands, SelfPlayer};
use crate::target::Selection;

/// The target half of the ref's `GetSlashCmdTarget` (ChatFrame.lua:650-658): a bare party
/// command falls back to the current selection iff it's a PLAYER; anything else is `None` (the
/// ref's silent no-op). The name is cache-resolved — a streamed player target is always cached.
fn target_player_name(
    selection: &Selection,
    names: &crate::names::NameCache,
    commands: &NetCommands,
) -> Option<String> {
    let guid = selection.guid?;
    if !benilla_protocol::guid::is_player(guid) {
        return None;
    }
    names.resolve(guid, commands).map(str::to_string)
}

/// The target guid a `CMSG_TEXT_EMOTE` carries — the current selection, **except that emoting at
/// yourself goes out untargeted**.
///
/// That exception is `DoEmote`'s last act before it builds the packet, VERIFIED at `0x5ef611`:
///
/// ```text
/// 5ef611: mov eax,[edi+8]     ; &ownGuid
/// 5ef614: mov ecx,[ebp+0xc]   ; the target guid low  ... cmp / jne past
/// 5ef61e: cmp edx,[eax+4]     ; ... and high         ... cmp / jne past
/// 5ef623: mov [ebp+0xc],ebx   ; ebx = 0 (xor at 0x5ef56b, never reloaded)
/// 5ef626: mov [ebp+0x10],ebx  ; -> the packet's target guid is ZERO
/// ```
///
/// **This is why vanilla has no self-emote sentence.** 1274 read the receive-side composer
/// correctly and then asserted an outcome — "You wave at ⟨YourName⟩." — from an input this gate
/// makes unreachable: the server never echoes your own name back as the target, so the untargeted
/// column is what you *and* everyone in range read ("You wave." / "Sam waves."). Without the gate
/// we would emit "Sam waves at Sam." to the whole zone.
///
/// Compared by **entity** rather than guid, which is the same predicate and costs no 17th system
/// param: `SelfPlayer` marks the entity whose guid *is* `SelfGuid` (`net.rs`), and [`Selection`]'s
/// only writer (`target::scan`) sets `target` and `guid` together from one streamed entity — so
/// "the selection is my entity" and "the selection is my guid" cannot disagree.
pub(super) fn emote_target(selection: &Selection, me: Option<Entity>) -> u64 {
    match selection.guid {
        Some(guid) if !(me.is_some() && selection.target == me) => guid,
        _ => 0,
    }
}

/// The client-local diagnostics' inputs, as one [`SystemParam`] — [`drain_chat_input`] is at the
/// 16-parameter ceiling, and a named struct beats a nested tuple nobody can read.
///
/// - `camera`/`clock` feed **`/shot`**, the framing instrument.
/// - `world` feeds **`/liquid`**, the swim diagnostic (decision 0634 follow-up): the interior
///   claim is what decides which liquid surfaces the swim query may see, and it arrives with the
///   query rather than beside it.
/// - `stores`/`self_store`/`factions`/`reputations`/`kinds` feed **`/reaction`**, the
///   attackability diagnostic: the exact inputs [`crate::target::ring_reaction`]
///   judges on, so "why is this unit not attackable" is one command instead of a guess — and
///   with them the V-plate CATEGORY, which turns on a different predicate entirely
///   ([`crate::target::ring::plate_is_friendly`]) and so cannot be read off the rank.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ChatProbes<'w, 's> {
    camera: Query<
        'w,
        's,
        &'static GlobalTransform,
        (With<benilla_world::view::WorldCamera>, Without<SelfPlayer>),
    >,
    clock: Option<Res<'w, benilla_world::lighting::GameClock>>,
    /// The `/liquid` instrument's whole world side — the claims, the verdict, the candidates.
    world: benilla_world::world_point::WorldPoint<'w, 's>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    self_store: Query<'w, 's, &'static crate::net::ObjectStore, With<SelfPlayer>>,
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Res<'w, crate::net::Reputations>,
    /// `/reaction <name>`'s resolve — so a scripted probe can ask about a player it has not
    /// clicked (the two-client duel run has no way to select the other side).
    guids: Res<'w, crate::net::GuidIndex>,
    /// The subject's [`benilla_protocol::EntityKind`] — the plate gate's own player test, so the
    /// diagnostic reports the branch the gate actually took rather than a second guess at it.
    kinds: Query<'w, 's, &'static crate::net::NetEntity>,
    /// **`/partytest raid`**: the synthetic raid seats US as its leader, and
    /// "leader" on this wire is a guid. Here rather than as a 17th drain parameter for the
    /// reason this struct exists at all — the drain is at Bevy's 16-param ceiling.
    self_guid: Res<'w, crate::net::SelfGuid>,
}

/// Everything the drain **queues into another subsystem's one setter** rather than applying itself,
/// bundled (the drain is at Bevy's 16-param ceiling).
///
/// - `stand`/`sheath` — the two setters `DoEmote` drives besides the packet: the **posture**
///   (`EmoteSpecProc == 1` → `SetStandState`) and the **stow** (`SetSheatheState(0, SNAP)`,
///   unconditional on every emote that passes the gates — site `0x5ef630`).
/// - `target`/`assist` — the by-name selection asks, answered by
///   [`crate::target`]'s shared resolver so they commit through the same SetSelection path a click
///   does. Chat never writes [`Selection`] itself.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ChatOut<'w, 's> {
    /// The console registry's lane: a `/console` line runs against the world at the next sync
    /// point ([`crate::console::execute`]).
    console: Commands<'w, 's>,
    stand: MessageWriter<'w, crate::player::StandStateRequest>,
    sheath: MessageWriter<'w, crate::creature_anim::SheathRequest>,
    target: MessageWriter<'w, crate::target::TargetByNameRequest>,
    assist: MessageWriter<'w, crate::target::AssistRequest>,
    follow: MessageWriter<'w, crate::player::FollowRequest>,
    /// **`/partytest ping`** — the minimap ping's setter. The LOCAL leg needs no
    /// instrument (click the map), but a *group member's* ping otherwise needs a second client
    /// logged in and standing somewhere else; this seats one as if Alice had sent it, through the
    /// same `seat` the wire arm calls, so the remote leg — the `partyN` token, the marker, the
    /// pin holding while you walk — is exercisable solo.
    ping: ResMut<'w, crate::minimap::MinimapPing>,
    /// The red UIErrorsFrame line by GlobalStrings key — the emote-while-moving refusal is its one
    /// tenant here. It rides this bundle rather than the system's own parameter
    /// list because that list is at Bevy's arity limit; checked first against every other param,
    /// since a resource reachable twice from one system is a `B0002` panic on the first live frame
    /// (1903). Nothing else in this system touches `UiErrorKeys`.
    ui_errors: ResMut<'w, crate::ui_action::UiErrorKeys>,
}

// One parameter per concern — the chat drain fans out to every command's consumer.
/// The chat verbs the stock `ChatFrame.lua` slash handlers call once *they* have parsed the
/// line — `DoEmote`, `RandomRoll`, `UninviteByName`, `ConsoleExec`, the channel verbs — drained
/// from the VM into the same [`ParsedChat`] values our own parser produces, so one executor
/// below serves both the reference's Lua and the lines it never sees.
fn engine_verbs(
    script: &mut benilla_ui::script::UiScript,
    emotes: Option<&crate::sound::EmoteSounds>,
) -> Vec<(String, ParsedChat)> {
    let mut out = Vec::new();
    for e in script.take_emote_requests() {
        // The token is the `EmotesText.dbc` NAME (`EMOTE<i>_TOKEN`, "WAVE"), which is how the
        // slash table resolved `/wave` before the Lua did the resolving.
        match emotes.and_then(|c| c.text_id(&e.token)) {
            Some(id) => out.push((String::new(), ParsedChat::TextEmote(id))),
            None => warn!(
                "chat: DoEmote({:?}): no EmotesText row for that token",
                e.token
            ),
        }
    }
    for (min, max) in script.take_roll_requests() {
        out.push((String::new(), ParsedChat::Random { min, max }));
    }
    for name in script.take_uninvite_requests() {
        out.push((String::new(), ParsedChat::Uninvite { name: Some(name) }));
    }
    for line in script.take_console_lines() {
        // `ConsoleExec` already wrote the valued CVar lines to the store; what reaches here is
        // a console COMMAND — a name the engine's own command table owns rather than
        // `CVar::Register`'s (`detailDoodadAlpha`: registrar `0x63f9e0`, so it never persists —
        // 2012) — or a bare CVar name for the registry to print (2303).
        out.push((line.clone(), ParsedChat::Console { line }));
    }
    for cmd in script.take_channel_commands() {
        out.push((String::new(), ParsedChat::Channel(cmd)));
    }
    out
}

/// **A manual join or leave of `GuildRecruitment` turns the auto-join option off** — the
/// reference's `0x49ed3d` (join) and `0x49ef8f` (leave), each `call 0x49ea70(0)` gated on the
/// matched `ChatChannels.dbc` row carrying `flags & 0x20000` and on the caller's own flag, which
/// the Lua bindings pass and the cascade's internal calls do not. The player has
/// taken manual control, and an option left checked would silently re-join or re-leave behind
/// them.
fn manual_join_or_leave(
    channels: &super::edit::ChannelState,
    script: &mut benilla_ui::script::UiScript,
    wire_name: &str,
) {
    let guild_row = channels
        .channels
        .row_for_name(wire_name)
        .is_some_and(|r| r.is_guild_recruitment());
    if guild_row && script.reset_guild_recruitment_mode() {
        info!("chat: manual {wire_name:?} — auto-join guild recruitment channel switched off");
    }
}

pub(super) fn drain_chat_input(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut chat_log: ResMut<super::feed::ChatLog>,
    // Mutable for one reason: an EXPLICIT leave clears the channel's `ZONECHANNELS` bit
    // (the reference's `0x49f10a` inside leave-by-name `0x49ee70`). The zone
    // walk's own LEAVE, one module over, deliberately does not.
    mut channels: ResMut<super::edit::ChannelState>,
    commands: Res<NetCommands>,
    emotes: Option<Res<crate::sound::EmoteSounds>>,
    selection: Res<Selection>,
    // The party slash commands: the roster for /promote's name→guid resolve, the
    // name cache for the bare-command player-target fallback (`GetSlashCmdTarget`).
    mut group: ResMut<crate::ui_party::GroupState>,
    mut names: ResMut<crate::names::NameCache>,
    // Our own live stand-state + move flags for the posture-eligibility gate (the pending-aware
    // component the player controller writes each frame); the entity doubles as `/castvis`'s
    // fallback subject.
    self_player: Query<(Entity, &MovementState, &GlobalTransform), With<SelfPlayer>>,
    mut cast_events: MessageWriter<crate::creature_anim::CastEvent>,
    mut play_seq: ResMut<crate::creature_anim::PlaySeq>,
    mut go_targets: MessageWriter<crate::creature_anim::SpellGoTargets>,
    // The command table — the reference's own aliases, resolved at boot.
    table: Res<super::commands::SlashCommands>,
    mut chat_out: ChatOut,
    probes: ChatProbes,
) {
    let ChatProbes {
        camera: world_camera,
        clock,
        world,
        stores,
        self_store,
        factions,
        reputations,
        guids,
        kinds,
        self_guid,
    } = &probes;
    let Some(mut script) = script else {
        return;
    };
    let mut queue = engine_verbs(&mut script, emotes.as_deref());
    // What `take_chat_input` carries now is benilla's own commands, handed over by the
    // `SlashCmdList` shim in ScriptLogFrame.xml once the reference's `ChatEdit_ParseText` has
    // found no built-in for the line — the history line, the type switch and the send are the
    // reference's Lua before the line ever reaches here (1948).
    for raw in script.take_chat_input() {
        let msg = raw.trim();
        if msg.is_empty() {
            continue;
        }
        // A line with no slash is SPEECH — which is also how a `.gm`-style server command
        // travels (the server reads the dot off a SAY). That is the seam's contract
        // (`UiScript::push_chat_input`): a typed line takes the stock `ChatEdit_SendText` →
        // `SendChatMessage` route since 1948, and a probe's line, which never touched the edit
        // box, must reach the wire the same way. The migration dropped this branch with the
        // old edit-state sender, and every `WOW_PROBE_CHAT` rig — the crowd raid's forty
        // `.partybot add`s, the probe shield's `.cheat god on` — went silent on the server.
        if !msg.starts_with('/') {
            let cmd = ClientCommand::Chat {
                kind: super::edit::SendType::Say.wire(),
                target: None,
                text: msg.to_string(),
            };
            if commands.0.send(cmd).is_err() {
                warn!("chat: not connected; line dropped");
            }
            continue;
        }
        queue.push((msg.to_string(), parse_line(&table, msg)));
    }
    for (msg, parsed) in queue {
        let msg = msg.as_str();
        match parsed {
            // `/r` is `SLASH_REPLY`, one of the reference's own built-ins (`ChatEdit_ParseText`'s
            // REPLY arm over the box's tell ring) — the line never reaches this queue.
            ParsedChat::Reply { .. } => {}
            // `/join` and `/leave` are stock built-ins (`SlashCmdList["JOIN"]`/`["LEAVE"]`, ref
            // `ChatFrame.lua` l.778/l.803), so a typed line never reaches here — only a probe's,
            // which skips the edit box. It takes the same handler, not a shortcut past it: the
            // command-table arm used to send the token verbatim, and a probe's `/join General`
            // created a CUSTOM channel called "General" on the server (decision 2144's live
            // run D). The handlers resolve through `JoinChannelByName`/`LeaveChannelByName`,
            // whose commands the `Channel` arm below drains next frame.
            ParsedChat::Join { name, password } => {
                let body = format!(
                    "SlashCmdList['JOIN']({:?})",
                    format!("{name} {password}").trim_end()
                );
                if let Err(e) = script.run(&body) {
                    warn!("ui_chat: {body}: {e}");
                }
            }
            ParsedChat::Leave { name } => {
                let body = format!("SlashCmdList['LEAVE']({name:?})");
                if let Err(e) = script.run(&body) {
                    warn!("ui_chat: {body}: {e}");
                }
            }
            ParsedChat::ChatList { name } => {
                let _ = commands.0.send(ClientCommand::ChannelList { name });
            }
            ParsedChat::Random { min, max } => {
                let _ = commands.0.send(ClientCommand::RandomRoll { min, max });
            }
            ParsedChat::Played => {
                let _ = commands.0.send(ClientCommand::PlayedTime);
            }
            ParsedChat::Shot => {
                // The framing instrument: the CURRENT camera pose, in the raw WoW
                // coords a capture `Scenario` takes, echoed to chat and appended to
                // `benilla-config/shots.txt` so a chosen spot survives the session.
                let Ok(cam) = world_camera.single() else {
                    continue;
                };
                let (_, rot, eye_bevy) = cam.to_scale_rotation_translation();
                let eye = benilla_assets::coords::bevy_to_wow(eye_bevy);
                let look = benilla_assets::coords::bevy_to_wow(eye_bevy + rot * Vec3::NEG_Z * 50.0);
                let minute = clock.as_deref().map(|c| c.minute).unwrap_or(720);
                let snippet = format!(
                    "eye: [{:.1}, {:.1}, {:.1}], look: [{:.1}, {:.1}, {:.1}], minute: {minute}",
                    eye[0], eye[1], eye[2], look[0], look[1], look[2]
                );
                chat_log.push_event(super::event::ChatEvent::text_only(
                    super::event::ChatEventKind::System,
                    format!("shot: {snippet}"),
                ));
                if let Some(path) = crate::local_state::shots_path() {
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let line = format!("{snippet}\n");
                    let appended = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
                    if appended.is_ok() {
                        chat_log.push_event(super::event::ChatEvent::text_only(
                            super::event::ChatEventKind::System,
                            format!("shot: appended to {}", path.display()),
                        ));
                    }
                }
            }
            ParsedChat::Liquid => {
                // What the swim query actually sees here, and why. Prints the interior claim, the
                // resolved verdict, and EVERY candidate footprint — a surface that should not be
                // claiming shows up beside the one that should.
                let Ok((_, feet)) = self_player.single().map(|(_, _, t)| ((), t.translation()))
                else {
                    chat_log.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::System,
                        "liquid: no player yet".into(),
                    ));
                    continue;
                };
                let wow = benilla_assets::coords::bevy_to_wow(feet);
                let claim = world.claim(benilla_world::world_point::Subject::Player);
                let eye = world.claim(benilla_world::world_point::Subject::Eye);
                let verdict = world.liquid_at(benilla_world::world_point::Subject::Player, wow);
                let mut lines = vec![
                    format!(
                        "liquid: feet [{:.2}, {:.2}, {:.2}] · claim {claim:?} ({})",
                        wow[0],
                        wow[1],
                        wow[2],
                        match world.interior() {
                            Some(k) => format!(
                                "wmo {} nameSet {} group {}",
                                k.wmo_id, k.name_set, k.group_area_id
                            ),
                            None => "no WMO interior claim".into(),
                        }
                    ),
                    // The camera EYE is a separate subject with its own claim (the reference's
                    // `[0xc7b748]`), and it is the one that decides the underwater filter. Printing
                    // only the player's is what let the two disagree unseen for a whole bug.
                    format!("liquid: EYE claim {eye:?}"),
                    match verdict {
                        Some(h) => format!(
                            "liquid: VERDICT {:?} surface z {:.2} ({:+.2} over feet)",
                            h.kind,
                            h.surface_z,
                            h.surface_z - wow[2]
                        ),
                        None => "liquid: VERDICT none — not in liquid".into(),
                    },
                ];
                let candidates =
                    world.describe_liquid_at(benilla_world::world_point::Subject::Player, wow);
                if candidates.is_empty() {
                    lines.push("liquid: no footprint covers this XY at all".into());
                }
                lines.extend(candidates.into_iter().map(|c| format!("liquid:   {c}")));
                for line in lines {
                    chat_log.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::System,
                        line,
                    ));
                }
            }
            ParsedChat::Reaction { name } => {
                // Everything the reaction ladder judges the subject on, in the order it judges:
                // the PvP rung's three duel gates first (naming the one that refused), then the
                // faction work, then the `can_attack` verdict the cast/attack paths use.
                let subject_guid = match &name {
                    Some(n) => crate::ui_duel::streamed_player_named(n, guids, &names),
                    None => selection.guid,
                };
                let subject_entity = subject_guid.and_then(|g| guids.0.get(&g).copied());
                let target_store = subject_entity.and_then(|e| stores.get(e).ok());
                let own_store = self_store.iter().next();
                // The plate gate's own player test, read the same way it reads it.
                let is_player = subject_entity
                    .and_then(|e| kinds.get(e).ok())
                    .is_some_and(|n| n.kind == benilla_protocol::EntityKind::Player);
                let mut lines = Vec::new();
                let describe = |label: &str, s: Option<&crate::net::ObjectStore>| match s {
                    Some(s) => format!(
                        "reaction: {label} flags 0x{:08x} (player-controlled {}) faction_tpl {:?} \
                         duel_team {} duel_arbiter 0x{:016x}",
                        s.0.unit_flags(),
                        s.0.unit_flags() & (1 << 3) != 0,
                        s.0.unit_faction_template(),
                        s.0.player_duel_team(),
                        s.0.player_duel_arbiter(),
                    ),
                    None => format!("reaction: {label} — no ObjectStore"),
                };
                lines.push(format!(
                    "reaction: subject {} guid {} · self store present {}",
                    name.as_deref().unwrap_or("<current target>"),
                    subject_guid.map_or("none".to_string(), |g| format!("0x{g:016x}")),
                    own_store.is_some(),
                ));
                lines.push(describe("target", target_store));
                lines.push(describe("self  ", own_store));
                match (target_store, own_store) {
                    (Some(t), Some(o)) => {
                        lines.push(format!(
                            "reaction: duel rung {:?}",
                            crate::target::duel_rung(&t.0, &o.0)
                        ));
                    }
                    _ => {
                        lines.push("reaction: duel rung not evaluated (a store is missing)".into())
                    }
                }
                let rank = crate::target::ring_reaction(
                    factions.as_deref(),
                    reputations,
                    target_store,
                    own_store,
                );
                lines.push(format!(
                    "reaction: RANK {rank} → can_attack {}",
                    crate::target::can_attack(
                        target_store,
                        factions.as_deref(),
                        reputations,
                        own_store
                    )
                ));
                // **The at-war bit, named**. It is the entire content of the
                // player→unit reaction's leg 3, it is what the world cursor, the plate category and
                // `UnitCanAttack` all turn on for a reputation-slot faction, and it is printed
                // nowhere else — so "why does this friendly NPC take the sword?" is unanswerable
                // from any other line here.
                let war = (|| {
                    let catalog = factions.as_deref()?.catalog();
                    let tpl = catalog.template(target_store?.0.unit_faction_template()?)?;
                    let at_war =
                        crate::target::ring::at_war_with(catalog, reputations, tpl.faction);
                    Some(match at_war {
                        Some(at_war) => format!(
                            "faction {} {:?} owns reputation slot {} → leg 3 answers with AT WAR \
                             = {at_war} (reaction {}); the standing is never read",
                            tpl.faction,
                            catalog.faction_name(tpl.faction).unwrap_or("<unnamed>"),
                            catalog
                                .reputation_faction(tpl.faction)
                                .map_or(-1, |i| i.rep_index),
                            if at_war { "hostile" } else { "friendly" },
                        ),
                        None => format!(
                            "faction {} {:?} has NO reputation slot → the template comparator \
                             answers; at-war does not apply",
                            tpl.faction,
                            catalog.faction_name(tpl.faction).unwrap_or("<unnamed>"),
                        ),
                    })
                })()
                .unwrap_or_else(|| "at-war not evaluated (no catalog, store or template)".into());
                lines.push(format!("reaction: {war}"));
                // The world cursor's two branch predicates, in the order `0x482200` runs them —
                // `CanInteract` picks the service ladder or the loot/skin/attack block, then
                // `CanAttack` decides the sword. Neither is a reaction threshold, so neither can
                // be read off RANK above.
                let interactable = crate::target::can_interact(
                    target_store,
                    factions.as_deref(),
                    reputations,
                    own_store,
                );
                let attackable = crate::target::can_attack(
                    target_store,
                    factions.as_deref(),
                    reputations,
                    own_store,
                );
                lines.push(format!(
                    "reaction: CURSOR npc_flags 0x{:04x} · can_interact {interactable} · \
                     can_attack {attackable} → {}",
                    target_store.map_or(0, |s| s.0.unit_npc_flags()),
                    if interactable {
                        "the NPC service ladder (a bit it consults → its cursor; none → Point)"
                    } else if attackable {
                        "the ATTACK sword (grayed past 10.45 yd)"
                    } else {
                        "nothing matched → Point"
                    },
                ));
                // The V-plate CATEGORY, which is a **different predicate** from the rank above
                // (`CanAttack` player→unit, plus `CanCooperate` for a player subject — decision
                // 1530) and so cannot be read off it. Both legs are printed, with the faction-group
                // masks they compare: a player in the wrong bucket is a mask disagreement roughly
                // every time, and the mask is invisible everywhere else in the client. A GM-mode
                // character is mask 0 on a vmangos realm (`.gm on` → faction template 35), which is
                // exactly how a friendly player ends up with an enemy plate.
                let mask = |s| {
                    crate::target::ring::faction_group_mask(factions.as_deref(), s)
                        .map_or("none".to_string(), |m| m.to_string())
                };
                let friendly = crate::target::ring::plate_is_friendly(
                    factions.as_deref(),
                    reputations,
                    target_store,
                    own_store,
                    is_player,
                );
                lines.push(format!(
                    "reaction: plate is_player {is_player} (OBJECT_FIELD_TYPE says {:?}; the two \
                     must agree — the predicates read the field, 1674) · faction-group mask self {} \
                     target {} · can_cooperate {} · can_attack(player→unit) {}",
                    target_store.and_then(|s| s.0.object_type()),
                    mask(own_store),
                    mask(target_store),
                    crate::target::ring::can_cooperate_with_player(
                        factions.as_deref(),
                        target_store,
                        own_store
                    ),
                    crate::target::ring::can_attack_from_player(
                        factions.as_deref(),
                        reputations,
                        target_store,
                        own_store,
                        is_player,
                    ),
                ));
                lines.push(format!(
                    "reaction: PLATE CATEGORY {} — this unit plates under {}",
                    if friendly { "FRIENDLY" } else { "ENEMY" },
                    if friendly { "SHIFT-V" } else { "V" },
                ));
                for line in lines {
                    // Also to the log: a scripted two-client run reads stdout, not the feed.
                    info!("{line}");
                    chat_log.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::System,
                        line,
                    ));
                }
            }
            ParsedChat::PartyTest { arg } => match arg.as_str() {
                "off" => group.clear_session(),
                // The raid grid's instrument — 25 synthetic rows, us leading.
                "raid" => {
                    // Onto the same by-key queue the wire arm uses (2045/2054), so the instrument
                    // shows the lines a real roster would — resolved from GlobalStrings, on the
                    // surface each catalog row names, with its sound. An instrument that composed
                    // its own text would be eyeballing something the game never prints.
                    chat_out.ui_errors.0.extend(crate::ui_party::synthetic_raid(
                        &mut group,
                        &mut names,
                        self_guid.0,
                    ));
                }
                "invite" => group.pending_invite = Some("Partner".to_string()),
                // A group member's ping, without the group member. 35 yd
                // north-east: off both axes so a mirrored sign is obvious, and inside every
                // outdoor view radius (the tightest is 66.7 yd) — indoors at the two tightest
                // zooms it is off the disc, which is itself worth seeing (the marker hides and
                // the ping survives, so walking back brings it into view).
                "ping" => {
                    let line = if let Ok((_, _, tf)) = self_player.single() {
                        let w = benilla_assets::coords::bevy_to_wow(tf.translation());
                        // +x is north, −y is east (0203's north-up mapping).
                        let at = (w[0] + 24.75, w[1] - 24.75);
                        chat_out.ping.seat(at, 0xF001);
                        format!(
                            "partytest: Alice pinged ({:.0}, {:.0}) — 35 yd NE, party1, 5 s",
                            at.0, at.1
                        )
                    } else {
                        "partytest: no player position — cannot place a ping".to_string()
                    };
                    chat_log.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::System,
                        line,
                    ));
                }
                // Serverless mark eyeball: skull the current target on the LOCAL board (the
                // real send round-trips through the server's echo, which /partytest lacks).
                "mark" => {
                    if let Some(guid) = selection.guid {
                        group.apply_raid_target(7, guid);
                    }
                }
                arg => {
                    // The live position seats the synthetic members' blips around us.
                    let player_xy = self_player.single().ok().map(|(_, _, tf)| {
                        let w = benilla_assets::coords::bevy_to_wow(tf.translation());
                        (w[0], w[1])
                    });
                    chat_out
                        .ui_errors
                        .0
                        .extend(crate::ui_party::synthetic_roster(&mut group, player_xy));
                    if arg == "lead" {
                        // The leader-view variant: an unmatched leader guid resolves to
                        // leader_index 0 in the feed — "we lead" — so the leader-only popup
                        // rows (promote/uninvite/the loot submenus) are eyeballable serverless.
                        group.leader = 0xF000;
                    }
                }
            },
            ParsedChat::Invite { name } => {
                if let Some(name) =
                    name.or_else(|| target_player_name(&selection, &names, &commands))
                {
                    let _ = commands.0.send(ClientCommand::GroupInvite { name });
                }
            }
            // Fire-and-forget: the server judges leader/group state and answers with either a
            // raid-typed SMSG_GROUP_LIST (the "joined a raid group" line rides the apply) or a
            // SMSG_PARTY_COMMAND_RESULT error, both already handled.
            ParsedChat::ConvertRaid => {
                let _ = commands.0.send(ClientCommand::GroupRaidConvert);
            }
            ParsedChat::Uninvite { name } => {
                if let Some(name) =
                    name.or_else(|| target_player_name(&selection, &names, &commands))
                {
                    let _ = commands.0.send(ClientCommand::GroupUninvite { name });
                }
            }
            ParsedChat::Promote { name } => {
                if let Some(name) =
                    name.or_else(|| target_player_name(&selection, &names, &commands))
                {
                    // The 1.12 wire form is a guid (CMSG_GROUP_SET_LEADER) — resolve against the
                    // roster; a miss answers with the server's own would-be error string
                    // (INTERIM: the ref's PromoteByName miss behavior is a 0434-dispatch item).
                    match group
                        .members
                        .iter()
                        .find(|m| m.name.eq_ignore_ascii_case(&name))
                    {
                        Some(m) => {
                            let _ = commands
                                .0
                                .send(ClientCommand::GroupSetLeader { guid: m.guid });
                        }
                        None => {
                            // ERR_TARGET_NOT_IN_GROUP_S (GlobalStrings:1861).
                            chat_log.push_event(super::event::ChatEvent::text_only(
                                super::event::ChatEventKind::System,
                                format!("{name} is not in your party."),
                            ));
                        }
                    }
                }
            }
            // The duel verbs enter the SAME intent queue the Era globals feed —
            // the reference's own slash handlers are one-liners over `StartDuel`/`CancelDuel`, so
            // routing through the queue keeps a single resolution path (spell lookup, streamed-
            // player gate, arbiter echo) instead of a second one here.
            ParsedChat::Duel { name } => {
                if let Some(name) =
                    name.or_else(|| target_player_name(&selection, &names, &commands))
                {
                    script.queue_duel_request(benilla_ui::script::DuelRequest::StartByName(name));
                }
            }
            ParsedChat::Forfeit => {
                script.queue_duel_request(benilla_ui::script::DuelRequest::Cancel);
            }
            // The by-name selection pair. Both hand the name to `crate::target`'s
            // shared resolver — the reference's own `0x493aa0`, parameterised per caller — so the
            // commit goes through the one SetSelection path a click, TAB and `TargetUnit` share.
            //
            // A bare `/target` reproduces `GetSlashCmdTarget`'s fallback (your current target's
            // name, iff it is a player) and is therefore a no-op re-select; a bare `/target` with a
            // creature selected resolves to nothing, exactly as the reference's `if
            // GetSlashCmdTarget(msg)` guard does.
            ParsedChat::Target { name } => {
                if let Some(name) =
                    name.or_else(|| target_player_name(&selection, &names, &commands))
                {
                    chat_out
                        .target
                        .write(crate::target::TargetByNameRequest { name });
                }
            }
            // A bare `/assist` is the ref's `AssistUnit("target")` — assist whoever is selected,
            // creature or player — so it passes `None` straight through rather than taking the
            // player-name fallback. The ref reaches the same unit by resolving its NAME first
            // (`AssistByName`), which can pick a different same-named player; going by the live
            // selection is the same intent without that ambiguity (recorded in 0886).
            ParsedChat::Assist { name } => {
                chat_out.assist.write(crate::target::AssistRequest { name });
            }
            // `/follow` — the subject is resolved by `crate::target` and the motion
            // is `crate::player`'s; chat only carries the ask. The two arms are the ref handler's
            // own two calls: bare is `FollowUnit("target")`, named is `FollowByName(name)` with no
            // second argument, i.e. prefix matching live.
            ParsedChat::Follow { name } => {
                chat_out.follow.write(match name {
                    Some(name) => crate::player::FollowRequest::Name { name, exact: false },
                    None => crate::player::FollowRequest::Unit("target".into()),
                });
            }
            // The ref's handler is `SlashCmdList["PVP"] = function() TogglePVP() end` — one line
            // over the same binding the popup row calls, so it enters the same intent queue
            // (the `/duel` reasoning above, verbatim).
            ParsedChat::Pvp => script.queue_pvp_toggle(),
            ParsedChat::Help => {
                // An honest benilla summary (the ref's HELP_TEXT_LINE pages are a settings-era
                // nicety; this stays useful and never stale-quotes them).
                for line in [
                    "Chat: /s /y /p /g /o /raid /rw /bg, /w <name>, /r, /e",
                    "Channels: /join <name> [pw], /leave <name>, /chatlist <name>",
                    "Party: /invite /uninvite /promote [name — bare uses your target]",
                    "Loot: /ffa /roundrobin /master <name>",
                    "Duel: /duel [name — bare uses your target], /forfeit (/concede /yield)",
                    "Social: /who [filter], /friends, /ignore, /trade, /inspect",
                    "Emotes: /sit /stand /sleep /kneel and every /wave-style emote",
                    "Spells: /cast <name> [(Rank N)]",
                    "Macros: /macro (/m) opens the window, /macrohelp explains them",
                    "Misc: /afk, /dnd, /random [min] [max], /played, /logout, /quit",
                    "Instruments: /shot, /liquid, /reaction, /castvis, /chattest, /partytest",
                ] {
                    chat_log.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::System,
                        line.to_string(),
                    ));
                }
            }
            // `ChatFrame_DisplayMacroHelpText` (ChatFrame.lua): the five shipped lines, read off
            // the VM's own GlobalStrings so they are the install's text, never a transcription
            // (the 0881 posture — the strings are data, the handler is ours).
            ParsedChat::MacroHelp => {
                for i in 1..=5 {
                    let key = format!("MACRO_HELP_TEXT_LINE{i}");
                    let Ok(line) = script.lua().globals().get::<String>(key.as_str()) else {
                        continue;
                    };
                    if !line.is_empty() {
                        chat_log.push_event(super::event::ChatEvent::text_only(
                            super::event::ChatEventKind::System,
                            line,
                        ));
                    }
                }
            }
            // `DoEmote` (`0x5ef560`) end to end. The gates in the client's own order, then the two
            // things it DOES: set a posture (the `/sit` family) and send the packet.
            ParsedChat::TextEmote(text_id) => {
                let (stand_state, flags) = self_player
                    .single()
                    .map_or((0, 0), |(_, m, _)| (m.stand_state, m.flags));
                let swimming = flags & move_flags::SWIMMING != 0;
                let emote_id = emotes.as_deref().and_then(|e| e.text_emote(text_id));
                // A chat-only text emote (no Emotes.dbc row) has no EmoteFlags to test and no
                // posture to set — it always sends.
                let posture = emote_id.and_then(|id| emotes.as_deref()?.posture_state(id));
                // GATE A (`CheckEmoteEligible` `0x47db40`): suppresses the anim AND the packet —
                // except its `0x4000` arm, which refuses out loud instead.
                let gate = match emotes.as_deref() {
                    Some(e) => emote_id
                        .and_then(|id| e.emote_flags(id))
                        .map_or(EmoteGate::Send, |f| {
                            emote_send_eligible(f, stand_state, swimming, flags)
                        }),
                    None => EmoteGate::Send,
                };
                // The `0x4000` arm's own half of the law, which lives in `DoEmote` (`0x5ef5d0`)
                // and not in the eligibility check: the red line fires only while the caster is
                // acting under its OWN control, so a feared or confused player emotes normally.
                if gate == EmoteGate::Moving {
                    let controlled = self_store
                        .single()
                        .is_ok_and(|s| crate::player::self_controlled(s.0.unit_flags()));
                    if controlled {
                        debug!("chat: emote {text_id} refused — moving (ERR_NOEMOTEWHILERUNNING)");
                        chat_out
                            .ui_errors
                            .0
                            .push(crate::ui_action::UiError::key("ERR_NOEMOTEWHILERUNNING"));
                        continue;
                    }
                }
                let eligible = gate != EmoteGate::Suppressed;
                // GATE B (`0x5ef5f3`): asleep, only a POSTURE emote gets through — which is how
                // `/stand` (and `/sit`) is the way out of `/sleep`, while a `/wave` in bed does
                // nothing at all.
                let awake_or_posture = stand_state != 3 || posture.is_some();
                if !eligible || !awake_or_posture {
                    // Byte-verified whole-send suppression: a seated /bow sends nothing (no packet,
                    // no anim) — the client's DoEmote returns before building the packet.
                    debug!(
                        "chat: emote {text_id} suppressed (eligible {eligible}, \
                         awake-or-posture {awake_or_posture}, stand {stand_state})"
                    );
                    continue;
                }
                // The stow (`0x5ef630`, VERIFIED unconditional at this point in the flow): every
                // emote that reaches here puts the weapons away, instantly — a `/wave` mid-fight
                // sheathes in the reference too. The anim layer's one setter owns the idempotency
                // refusal, so an already-stowed body costs nothing.
                if let Ok((entity, _, _)) = self_player.single() {
                    chat_out.sheath.write(crate::creature_anim::SheathRequest {
                        entity,
                        state: 0,
                        ceremony: false,
                    });
                }
                // The posture branch (`EmoteSpecProc == 1` → `SetStandState(param)`): the emote's
                // whole visible effect, since the SERVER deliberately does nothing for a STATE text
                // emote (vmangos `HandleTextEmoteOpcode` breaks out for SIT/SLEEP/KNEEL). Through
                // the one setter in `crate::player`, which sends `CMSG_STANDSTATECHANGE`, holds the
                // local commit until the echo lands, and runs the sit-stow rider.
                if let Some(state) = posture {
                    chat_out
                        .stand
                        .write(crate::player::StandStateRequest { state: state as u8 });
                }
                let target = emote_target(
                    &selection,
                    self_player.single().ok().map(|(entity, _, _)| entity),
                );
                match commands
                    .0
                    .send(ClientCommand::TextEmote { text_id, target })
                {
                    Ok(()) => info!("chat: sent {msg:?}"),
                    Err(_) => warn!("chat: not connected; dropped {msg:?}"),
                }
            }
            ParsedChat::CastVis {
                spell_id,
                kind,
                ground,
            } => {
                // The dev instrument: fire the synthesized cast edge at the selection, else self —
                // the same message the net bridge writes, so it exercises the whole resolve/hold/
                // release path (decision 0099's iteration loop).
                let me = self_player.single().ok().map(|(e, _, _)| e);
                let subject = selection.target.or(me);
                match subject {
                    Some(entity) => {
                        info!("castvis: spell {spell_id} {kind:?} ground={ground} on {entity}");
                        cast_events.write(crate::creature_anim::CastEvent {
                            entity,
                            spell_id,
                            kind,
                            seq: play_seq.next(),
                        });
                        // A selected caster's GO also fires its target list at *us* — the one
                        // live unit the dev loop always has — so a Speed>0 spell's missile and
                        // impact run end to end (0099 phase 4). A self-cast has no target: no
                        // missile, matching a hit-less GO. `ground` instead sends the pure DEST
                        // shape — empty lists, a point 15 yd ahead of the player at the player's
                        // own height (a dev stand-in for the terrain pick; flat enough to watch a
                        // Flare fly) — which is the only shape that reaches the location fallback.
                        if kind == crate::creature_anim::CastEventKind::Go {
                            let dest = ground.then(|| {
                                self_player.single().ok().map(|(_, _, tf)| {
                                    tf.translation() + tf.forward().as_vec3() * 15.0
                                })
                            });
                            if let Some(dest) = dest {
                                go_targets.write(crate::creature_anim::SpellGoTargets {
                                    caster: entity,
                                    spell_id,
                                    hits: Vec::new(),
                                    misses: Vec::new(),
                                    dest,
                                    ammo_display_id: None,
                                    seq: play_seq.next(),
                                });
                            } else if let Some(me) = me.filter(|&m| m != entity) {
                                go_targets.write(crate::creature_anim::SpellGoTargets {
                                    caster: entity,
                                    spell_id,
                                    hits: vec![me],
                                    misses: Vec::new(),
                                    dest: None,
                                    // Dev stand-in only: a real GO's ammo block carries the
                                    // caster's actual ammo; `/castvis` has none, so shot
                                    // spells (`75 go`, `2480 go`) fly the Rough Arrow
                                    // display (5996).
                                    ammo_display_id: Some(5996),
                                    seq: play_seq.next(),
                                });
                            }
                        }
                    }
                    None => warn!("castvis: no selection and no self avatar — dropped"),
                }
            }
            ParsedChat::ChatTest => chattest_battery(&mut chat_log, &|key: &str| {
                script
                    .lua()
                    .globals()
                    .get::<String>(key)
                    .ok()
                    .filter(|t| !t.is_empty())
            }),
            // `/logout` (and `/camp`) is the reference's own `SlashCmdList["LOGOUT"]` → `Logout()`,
            // so it takes the SAME route the game menu's Logout button does: the
            // request queues on the script seam, `crate::ui_logout` sends it and narrates the
            // server's answer with the CAMP countdown. It used to send `CMSG_LOGOUT_REQUEST`
            // straight from here, which meant a field logout looked like nothing happening for 20 s.
            ParsedChat::Logout => {
                script.queue_session_request(benilla_ui::script::SessionRequest::Logout)
            }
            // `/quit` `/exit` — the ref's `Quit()`, same queue as the game menu's Exit button, so
            // the field countdown and the confirmation are the ones already built.
            ParsedChat::Quit => {
                script.queue_session_request(benilla_ui::script::SessionRequest::Quit)
            }
            // `/reload` — the same deferred rebuild `ReloadUI()` queues, through
            // the same session seam as the exit verbs above.
            ParsedChat::ReloadUi => {
                script.queue_session_request(benilla_ui::script::SessionRequest::ReloadUi)
            }
            // A `/console` line for the command registry. Deferred to the world
            // because a command's body is `fn(&mut World, &str)` — the reference's handlers run
            // against the whole client too — and its lines land in the chat frame as system
            // text, the seam `/console` output has always used.
            ParsedChat::Console { line } => {
                chat_out.console.queue(move |world: &mut World| {
                    let lines = crate::console::execute(world, &line);
                    let mut log = world.resource_mut::<super::feed::ChatLog>();
                    for text in lines {
                        log.push_event(super::event::ChatEvent::text_only(
                            super::event::ChatEventKind::System,
                            text,
                        ));
                    }
                });
            }
            // The reference's own handler body, run in the VM (the 0668 posture) — `/trade`,
            // `/inspect`, the loot-method trio, and `/script`'s raw chunk.
            ParsedChat::Lua { body } => {
                if let Err(e) = script.run(&body) {
                    // `/script` hands the player's own typo back to them; a built-in body failing
                    // is ours to see in the log.
                    warn!("ui_chat: {body:?}: {e}");
                    chat_log.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::System,
                        format!("{e}"),
                    ));
                }
            }
            ParsedChat::Channel(cmd) => {
                use benilla_ui::script::ChannelCommand as C;
                let cmd = match cmd {
                    C::Join { name, password } => {
                        manual_join_or_leave(&channels, &mut script, &name);
                        ClientCommand::JoinChannel { name, password }
                    }
                    C::Leave { name } => {
                        // `LeaveChannelByName` (`0x4a0000` → `0x49ee70`): the VM composed a
                        // shortcut or passed a custom name; a number names a confirmed slot here
                        // or the call is a no-op. The mask clear is this path's and no other's,
                        // and so is the custom re-join list's.
                        let Some(name) = channels.leave_target(&name) else {
                            continue;
                        };
                        manual_join_or_leave(&channels, &mut script, &name);
                        channels.note_zone_channel_left(&name);
                        channels.note_custom_channel_left(&name);
                        ClientCommand::LeaveChannel { name }
                    }
                    C::List { name } => ClientCommand::ChannelList { name },
                    // `ListChannels()` — the joined roster, numbered the way `/N` addresses it.
                    C::ListAll => {
                        let roster: Vec<String> = channels
                            .joined
                            .iter()
                            .enumerate()
                            .filter_map(|(i, c)| {
                                c.as_ref().map(|c| format!("{}. {}", i + 1, c.name))
                            })
                            .collect();
                        let text = if roster.is_empty() {
                            "You are not in any channels.".to_string()
                        } else {
                            format!("Channels: {}", roster.join(", "))
                        };
                        chat_log.push_event(super::event::ChatEvent::text_only(
                            super::event::ChatEventKind::System,
                            text,
                        ));
                        continue;
                    }
                    C::DisplayOwner { name } => ClientCommand::ChannelOwner { name },
                    C::SetOwner { name, player } => ClientCommand::ChannelSetOwner { name, player },
                    C::SetPassword { name, password } => {
                        ClientCommand::ChannelPassword { name, password }
                    }
                    C::Ban { name, player } => ClientCommand::ChannelBan { name, player },
                    C::Invite { name, player } => ClientCommand::ChannelInvite { name, player },
                    C::Kick { name, player } => ClientCommand::ChannelKick { name, player },
                    C::Moderator { name, player } => {
                        ClientCommand::ChannelModerator { name, player }
                    }
                    C::Unmoderator { name, player } => {
                        ClientCommand::ChannelUnmoderator { name, player }
                    }
                    C::Mute { name, player } => ClientCommand::ChannelMute { name, player },
                    C::Unmute { name, player } => ClientCommand::ChannelUnmute { name, player },
                    C::Unban { name, player } => ClientCommand::ChannelUnban { name, player },
                    C::Moderate { name } => ClientCommand::ChannelModerate { name },
                    C::ToggleAnnouncements { name } => ClientCommand::ChannelAnnouncements { name },
                };
                let _ = commands.0.send(cmd);
            }
            ParsedChat::Unknown => {
                // Before the help line: an ADDON may claim it. The reference
                // resolves `SlashCmdList` in the same pass as its own commands; ours runs after
                // the boot table misses, which gives the same precedence — a shipped command can
                // never be shadowed — without moving our handlers into Lua.
                if let Some(rest) = msg.strip_prefix('/') {
                    let rest = rest.trim();
                    let (cmd, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    if script.run_slash_command(cmd, args.trim()) {
                        continue;
                    }
                }
                // HELP_TEXT_SIMPLE (the ref's unknown-command reply, ChatEdit_ParseText l.2203),
                // read off the player's own table rather than re-typed here. It is
                // no message-catalog row — the reference emits it from Lua straight into chat — so
                // there is no surface or sound to look up, only the wording.
                if let Some(text) = script
                    .lua()
                    .globals()
                    .get::<String>("HELP_TEXT_SIMPLE")
                    .ok()
                    .filter(|t| !t.is_empty())
                {
                    chat_log.push_event(super::event::ChatEvent::text_only(
                        super::event::ChatEventKind::System,
                        text,
                    ));
                }
            }
        }
    }
}

/// `/chattest` (the 0288 instrument): one synthetic line of every renderable form through the
/// real event pipeline — kinds, flags, the language header, channel prefixes, notices, and both
/// link forms (item + player), so formats/colors/links verify in one screen.
fn chattest_battery(log: &mut super::feed::ChatLog, get: &dyn Fn(&str) -> Option<String>) {
    use super::event::{ChatEvent, ChatEventKind as K};
    let player = |kind: K, text: &str, sender: &str| {
        let mut e = ChatEvent::text_only(kind, text.into());
        e.sender = sender.into();
        e
    };
    let mut battery: Vec<ChatEvent> = vec![
        player(K::Say, "the quick brown fox — say white.", "Testa"),
        player(K::Yell, "yell red!", "Testa"),
        player(
            K::Whisper,
            "whisper pink (chime + flash if Combat Log is selected).",
            "Testa",
        ),
        player(K::WhisperInform, "the To-echo.", "Testa"),
        player(K::Emote, "dances — bare name, orange.", "Testa"),
        player(K::Party, "party blue.", "Testa"),
        player(K::Guild, "guild green.", "Testa"),
        player(K::Officer, "officer deep green.", "Testa"),
        player(K::RaidWarning, "raid warning salmon.", "Testa"),
        player(K::MonsterSay, "monster say pale yellow.", "Grunt"),
        player(K::MonsterYell, "monster yell red.", "Grunt"),
        player(K::MonsterWhisper, "monster whisper gray.", "Grunt"),
        player(K::MonsterEmote, "%s looks around — emote orange.", "Grunt"),
        ChatEvent::text_only(K::System, "system yellow.".into()),
        ChatEvent::text_only(
            K::Skill,
            "Your skill in Testing has increased to 300.".into(),
        ),
        ChatEvent::text_only(
            K::Loot,
            "You receive loot: |cff1eff00|Hitem:2000:0:0:0|h[Test Blade]|h|r — click me.".into(),
        ),
        ChatEvent::text_only(K::Money, "You loot 1 Gold, 23 Silver, 45 Copper.".into()),
    ];
    // A GM-flagged line, an Orcish header, a numbered channel line, and the join/kick notices.
    let mut gm = player(K::Say, "a GM-tagged line.", "Testa");
    gm.flag = "GM".into();
    battery.push(gm);
    let mut orc = player(K::Say, "an Orcish-headered line.", "Grunk");
    orc.language = "Orcish".into();
    battery.push(orc);
    let mut chan = player(K::Channel, "channel pink.", "Testa");
    chan.channel = "1. General - Elwynn Forest".into();
    battery.push(chan);
    let mut join = player(K::ChannelJoin, "", "Testa");
    join.channel = "1. General - Elwynn Forest".into();
    battery.push(join);
    let mut notice = ChatEvent::text_only(K::ChannelNotice, String::new());
    notice.channel = "General - Elwynn Forest".into();
    notice.notice = "2".into(); // YOU_JOINED
    battery.push(notice);
    for e in battery {
        log.push_event(e);
    }
    combat_log_battery(log, get);
    info!("chattest: battery queued");
}

/// The combat-log half of `/chattest`: one synthetic line of every family, through the real
/// [`super::combat`] composer and the real drain — so the whole `COMBAT_*`/`SPELL_*` block's
/// wording, colours and window routing verify in one screen without a fight.
///
/// It exists because the alternative is a live combat probe, and unattended ones are out (the
/// director's standing rule). The lines are driven with **guid 0 on both endpoints and literal
/// names in the fills**, so the drain's name resolve is a no-op and nothing here touches the wire;
/// everything downstream of that — the family's template lookup, the slot fill, the chat type, the
/// route, the `CHAT_MSG_*` fire an addon sees — is the production path exactly.
///
/// The chat TYPES are picked to show the block's spread rather than one row: your own melee and
/// spells, your pet, a hostile player, and a creature hitting you (which is the one that is red).
fn combat_log_battery(log: &mut super::feed::ChatLog, get: &dyn Fn(&str) -> Option<String>) {
    use super::combat::{self, Family, Fills, PendingCombat, Variant};
    use super::event::ChatEventKind as K;

    let fills = |amount: i64, school: Option<u8>, power: Option<u32>| Fills {
        attacker: "Gnoll Brute".into(),
        victim: "Target Dummy".into(),
        spell: "Fireball".into(),
        school,
        power,
        amount,
        amount2: amount / 3,
        power2: power,
        named: "Copper Bar".into(),
        trailers: None,
    };
    let line = |kind: K, family: Family, variant: Variant, f: Fills| PendingCombat {
        kind,
        family,
        variant,
        subject: 0,
        object: 0,
        named: super::combat::Named::Ready,
        fills: f,
        tries: 0,
    };
    let battery = [
        // Your own melee, plain and crit, and the school form.
        line(
            K::CombatSelfHits,
            combat::COMBATHIT,
            Variant::SelfOther,
            fills(120, None, None),
        ),
        line(
            K::CombatSelfHits,
            combat::COMBATHITCRIT,
            Variant::SelfOther,
            fills(240, None, None),
        ),
        line(
            K::CombatSelfHits,
            combat::COMBATHITSCHOOL,
            Variant::SelfOther,
            fills(35, Some(2), None),
        ),
        line(
            K::CombatSelfMisses,
            combat::MISSED,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        // A creature hitting you — the red rows, and the ones a player notices first.
        line(
            K::CombatCreatureVsSelfHits,
            combat::COMBATHIT,
            Variant::OtherSelf,
            fills(87, None, None),
        ),
        line(
            K::CombatCreatureVsSelfMisses,
            combat::VSDODGE,
            Variant::OtherSelf,
            fills(0, None, None),
        ),
        line(
            K::CombatCreatureVsSelfMisses,
            combat::VSBLOCK,
            Variant::OtherSelf,
            fills(0, None, None),
        ),
        line(
            K::CombatCreatureVsSelfMisses,
            combat::VSPARRY,
            Variant::OtherSelf,
            fills(0, None, None),
        ),
        // Your own spells: the gold pair, and the outcomes that are not damage.
        line(
            K::SpellSelfDamage,
            combat::SPELLLOGSCHOOL,
            Variant::SelfOther,
            fills(412, Some(2), None),
        ),
        line(
            K::SpellSelfDamage,
            combat::SPELLLOGCRITSCHOOL,
            Variant::SelfOther,
            fills(830, Some(2), None),
        ),
        line(
            K::SpellSelfDamage,
            combat::SPELLMISS,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        line(
            K::SpellSelfDamage,
            combat::SPELLRESIST,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        line(
            K::SpellSelfBuff,
            combat::HEALED,
            Variant::SelfOther,
            fills(560, None, None),
        ),
        line(
            K::SpellSelfBuff,
            combat::POWERGAIN,
            Variant::SelfSelf,
            fills(90, None, Some(0)),
        ),
        // Your pet, a hostile player, and the periodic + shield rows.
        line(
            K::CombatPetHits,
            combat::COMBATHIT,
            Variant::OtherOther,
            fills(64, None, None),
        ),
        line(
            K::SpellHostilePlayerDamage,
            combat::SPELLLOGSCHOOL,
            Variant::OtherSelf,
            fills(305, Some(5), None),
        ),
        line(
            K::SpellPeriodicSelfDamage,
            combat::PERIODICAURADAMAGE,
            Variant::SelfOther,
            fills(48, Some(5), None),
        ),
        line(
            K::SpellPeriodicSelfBuffs,
            combat::PERIODICAURAHEAL,
            Variant::SelfSelf,
            fills(75, None, None),
        ),
        line(
            K::SpellDamageShieldsOnSelf,
            combat::DAMAGESHIELD,
            Variant::SelfOther,
            fills(22, Some(1), None),
        ),
        line(
            K::SpellSelfBuff,
            combat::SPELLPOWERLEECH,
            Variant::SelfOther,
            fills(150, None, Some(0)),
        ),
        // ── the completeness block (1703) ────────────────────────────────────────────────
        // One line per family that was absent until now, in the order a player meets them.
        // The `Named` slot's text is already in the fills, so these drive the same drain the
        // wire does with the same composer — only the resolve hops are short-circuited.
        line(
            K::CombatHostileDeath,
            combat::UNITDIES,
            Variant::OtherOther,
            fills(0, None, None),
        ),
        line(
            K::CombatHostileDeath,
            combat::SELFKILLOTHER,
            Variant::OtherOther,
            fills(0, None, None),
        ),
        line(
            K::CombatFriendlyDeath,
            combat::UNITDESTROYEDOTHER,
            Variant::OtherOther,
            fills(0, None, None),
        ),
        line(
            K::SpellPeriodicSelfDamage,
            combat::AURAADDED_HARMFUL,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        line(
            K::SpellPeriodicSelfBuffs,
            combat::AURAADDED_HELPFUL,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        line(
            K::SpellPeriodicSelfDamage,
            combat::AURAAPPLICATIONADDED_HARMFUL,
            Variant::SelfOther,
            fills(3, None, None),
        ),
        line(
            K::SpellAuraGoneSelf,
            combat::AURAREMOVED,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        line(
            K::SpellBreakAura,
            combat::AURADISPEL,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        line(
            K::SpellItemEnchantments,
            combat::ITEMENCHANTMENTADD,
            Variant::SelfSelf,
            fills(0, None, None),
        ),
        line(
            K::SpellTradeskills,
            combat::TRADESKILL_LOG,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        line(
            K::SpellSelfDamage,
            combat::SPELLINTERRUPT,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        line(
            K::SpellSelfDamage,
            combat::SPELLEXTRAATTACKS_SINGULAR,
            Variant::SelfOther,
            fills(1, None, None),
        ),
        line(
            K::SpellSelfDamage,
            combat::SPELLSPLITDAMAGE,
            Variant::SelfOther,
            fills(66, None, None),
        ),
        line(
            K::SpellSelfDamage,
            combat::PROCRESIST,
            Variant::SelfOther,
            fills(0, None, None),
        ),
        line(
            K::CombatCreatureVsSelfHits,
            combat::VSENV_FALLING,
            Variant::SelfOther,
            fills(94, None, None),
        ),
        line(
            K::CombatMiscInfo,
            combat::DURABILITYDAMAGE_DEATH,
            Variant::OtherOther,
            fills(0, None, None),
        ),
        // The two families whose `Named` slot is not an item name: a faction and a failure
        // reason. Overridden here so the battery reads as the sentences a player will see.
        line(
            K::CombatFactionChange,
            combat::FACTION_STANDING_INCREASED,
            Variant::OtherOther,
            Fills {
                named: "Stormwind".into(),
                ..fills(250, None, None)
            },
        ),
        // The failure reason is the ONE fill in this battery that is a reference string rather
        // than a synthetic name: production puts a resolved GlobalString in this slot
        // (`ui_action::feed`'s `CAST_FAIL_KEYS` lookup), so a battery that typed English here
        // would read as the only untranslated line on a localized install. `ERR_OUT_OF_MANA` and
        // not `OUT_OF_MANA` — the same enUS sentence, but the byte-verified NO_POWER pick table
        // `0x8118dc` names the former (`ui_action::cast_fail`).
        line(
            K::SpellFailedLocalPlayer,
            combat::SPELLFAILCAST,
            Variant::SelfOther,
            Fills {
                named: get("ERR_OUT_OF_MANA").unwrap_or_default(),
                ..fills(0, None, None)
            },
        ),
        // The trailer pass, on the one family that can show all six.
        line(
            K::CombatSelfHits,
            combat::COMBATHIT,
            Variant::SelfOther,
            Fills {
                trailers: Some(combat::Trailers {
                    absorbed: 12,
                    resisted: 7,
                    blocked: 30,
                    hit_info: 0x4000,
                }),
                ..fills(210, None, None)
            },
        ),
    ];
    for l in battery {
        log.push_combat(l);
    }
}

/// The send-side emote **posture-eligibility gate**: the real client's `CheckEmoteEligible`
/// (`0x47db40`), the *only* site that reads an `Emotes.dbc` `EmoteFlags` — called from `DoEmote`
/// (`0x5ef560`) *before* `CMSG_TEXT_EMOTE` is built, so a suppressed emote sends no packet and
/// plays no local anim at all (a seated `/bow` self-censors; the server round-trip never happens).
/// The predicate is exactly these four tests, in the client's own order. `true` = eligible (send +
/// play); `false` = suppress both.
///
/// # The fifth flag, `0x4000` — NOT built, and the reason recorded here was WRONG
///
/// What `CheckEmoteEligible 0x47db40` decides about one emote — three outcomes, not two, which is
/// why this replaced a `bool`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum EmoteGate {
    /// Send it: packet, posture, animation.
    Send,
    /// **Silently** dropped — no packet, no animation, and no line. A seated `/bow` is this.
    Suppressed,
    /// The `0x4000` "requires standing still" arm tripped. The reference does NOT suppress here:
    /// `CheckEmoteEligible` writes an out-param and `DoEmote` reads it at `0x5ef5d0`, raising
    /// `ERR_NOEMOTEWHILERUNNING` and aborting **only when `IsSelfControlled 0x5fa550` is
    /// non-zero** — so the toast fires for an ordinary moving player and a **feared** one emotes
    /// away happily. That caster-state test is the caller's, exactly as it is the reference's.
    Moving,
}

/// This doc used to say `0x4000` ("requires standing still") *"only sets an out param the client
/// acts on while fear/confuse-controlled, which benilla doesn't model"*. The polarity is the
/// **inverse** (2026-08-23). The bytes:
///
/// - `0x47dbab` tests `EmoteFlags & 0x4000`, and if set, `0x47dbb3` tests the live `CMovement`
///   word against **`0x20ff`** — the four direction bits, the two turn bits, the two pitch bits and
///   `FALLING` — writing `*out = 1`. Pointedly **not** `SWIMMING`.
/// - `DoEmote` then reads that out-param at `0x5ef5d0` and, when `0x5fa550` returns non-zero,
///   raises message `0x139` = **`ERR_NOEMOTEWHILERUNNING`** ("You can't do that while moving!") and
///   **aborts**: no emote packet, no `SetStandState`.
/// - `0x5fa550` is `IsSelfControlled`, not "is fear/confuse-controlled" — it returns **1** for an
///   ordinary player and **0** while `UNIT_FIELD_FLAGS & 0xc00004` (DISABLE_MOVE / CONFUSED /
///   FLEEING). So the toast fires for the **ordinary moving player** and is *suppressed* while
///   feared. That is the exact reversal of what this comment claimed.
///
/// It stays unbuilt — deliberately, and as a named gap rather than a settled reading: implementing
/// it puts a red error line on screen for every emote typed while moving, turning, pitching or
/// falling, which is a visible behaviour change for the director to weigh rather than a bug fix to
/// slip in. What it is **not** is a water gate: `0x20ff` carries no `SWIMMING`, so it has nothing
/// to do with B155 and could never have covered it (`super::super::tests::
/// the_posture_emotes_carry_no_swim_suppression_flag` is the data half of that same negative). The
/// posture path's water refusal lives in [`crate::player::state`]'s `stand_state_refused`.
pub(super) fn emote_send_eligible(
    emote_flags: u32,
    stand_state: u8,
    swimming: bool,
    move_flags: u32,
) -> EmoteGate {
    // `0x0400`: unconditional suppress (client `0x47db58`).
    if emote_flags & 0x0400 != 0 {
        return EmoteGate::Suppressed;
    }
    // `0x0001` + non-zero stand-state: "requires STAND" (client `0x47db65`..`0x47db74`).
    if emote_flags & 0x0001 != 0 && stand_state != 0 {
        return EmoteGate::Suppressed;
    }
    // `0x0080` while swimming (client `0x47db76`..`0x47db7d`).
    if emote_flags & 0x0080 != 0 && swimming {
        return EmoteGate::Suppressed;
    }
    // `0x0200` ABSENT at SLEEP(3)/DEAD(7): the bit means "allowed while asleep/dead" (client
    // `0x47db8e`..`0x47db9f`).
    if emote_flags & 0x0200 == 0 && matches!(stand_state, 3 | 7) {
        return EmoteGate::Suppressed;
    }
    // `0x4000` "requires standing still" (client `0x47dbab`), tested against the live CMovement
    // word at `0x47dbb3` — the reference's own `0x20ff`, which is exactly
    // [`crate::creature_anim::move_flags::INTEGRATED`]: the four direction bits, the two turn
    // bits, the two pitch bits and FALLING. Pointedly **not** SWIMMING.
    //
    // This arm does not suppress: it writes `*out = 1`, and DoEmote turns that into a red line
    // (or not) on a caster-state test the caller owns — see [`EmoteGate::Moving`].
    if emote_flags & 0x4000 != 0 && move_flags & crate::creature_anim::move_flags::INTEGRATED != 0 {
        return EmoteGate::Moving;
    }
    EmoteGate::Send
}

/// `PLAYER_FLAGS_DND` (`0x4`) off our own live descriptor — the DND arm's state test.
///
/// **Live, not mirrored, and that asymmetry is the reference's** (`0x49f3f0`): the AFK path keeps
/// an optimistic global and the DND path keeps nothing, so a second `/dnd` typed before the
/// server's `PLAYER_FLAGS` update lands still reads DND clear and re-marks, where a second `/afk`
/// in the same window clears. Do not "fix" this into a symmetric pair.
fn is_dnd(self_q: &Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>) -> bool {
    self_q
        .iter()
        .next()
        .is_some_and(|s| s.0.player_flags() & 0x4 != 0)
}

/// `autoClearAFK` — registered default `"1"` (`0x5e24d4 push 0x82e748`). Its reader tests
/// `[cvar+0x28]` for non-zero (`0x5eb84b`), and with the CVar OFF the clear is a **total** no-op:
/// no echo, no mirror write, no packet.
fn auto_clear_afk(cvars: &crate::cvars::Cvars) -> bool {
    cvars.flag("autoClearAFK").unwrap_or(true)
}

/// Turn an addon's `SendChatMessage` calls into sends.
///
/// Its own system rather than a branch inside [`drain_chat_input`], for the reason
/// `benilla_ui::script::chat_send`'s module doc gives: the box's drain runs the **slash grammar**
/// and this path must not. An addon announcing `"/dance"` is saying six characters.
///
/// An unknown chat-type token is **reported, not guessed**. `SendChatMessage(msg, "RAID_WARNING")`
/// silently going to /say is worse than not going: the addon believes it warned the raid.
pub(super) fn drain_addon_chat_sends(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    commands: Res<NetCommands>,
    mut chat_log: ResMut<super::feed::ChatLog>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
    // The optimistic AFK mirror (`[0xb6e5cc]`) the `/afk` toggle reads and writes — 2088.
    mut mirror: ResMut<super::away::AfkMirror>,
    // `autoClearAFK`, whose registered default is `"1"` — the gate on the implicit clear.
    cvars: Res<crate::cvars::Cvars>,
    // Our own descriptor, for the DND arm's LIVE `PLAYER_FLAGS & 0x4` read (DND has no mirror).
    self_q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
) {
    let Some(mut script) = script else {
        return;
    };
    for send in script.take_chat_sends() {
        let Some(kind) = super::edit::SendType::from_token(&send.chat_type) else {
            warn!(
                "chat: SendChatMessage with unknown type {:?}",
                send.chat_type
            );
            chat_log.push_event(super::event::ChatEvent::text_only(
                super::event::ChatEventKind::System,
                format!("Unknown chat type \"{}\".", send.chat_type),
            ));
            continue;
        };
        // `SendChatMessage`'s acknowledge (`0x49f5fc`, 1976): the twelve social wire types —
        // party, raid, guild, officer, whisper, channel, AFK/DND, raid leader/warning, the two
        // battleground types — say, yell and emote are not among them.
        if !matches!(
            kind,
            super::edit::SendType::Say | super::edit::SendType::Yell | super::edit::SendType::Emote
        ) {
            if let Some(t) = tutorials.as_mut() {
                t.write(crate::tutorial::TutorialEvent::Acknowledge {
                    id: crate::tutorial::id::CHATTING,
                });
            }
        }
        // ── The away commands, and the implicit clear every other send carries (2088) ────────
        //
        // `SendChatMessage 0x49f1e0` is not a uniform dispatcher: AFK and DND each carry their own
        // arm ahead of the generic send, and EVERY other type first clears a standing AFK. The
        // whole law — the four `CHAT_MSG_SYSTEM` lines, the client-side default substitution, the
        // optimistic mirror — is `super::away`, off the reference's truth table.
        let strings = |key: &str| crate::ui_chat::combat::global_string(&script, key);
        let wire = kind.wire();
        let text = match wire {
            crate::net::ChatKind::Afk => {
                let out = super::away::afk_line(&send.text, *mirror, &strings);
                if let Some(line) = out.line {
                    super::away::push_system(&mut chat_log, line);
                }
                if let Some(v) = out.mirror {
                    mirror.0 = v;
                }
                out.body
            }
            crate::net::ChatKind::Dnd => {
                // The live descriptor bit, not a mirror: DND has none (`0x49f3f0`), which is what
                // makes a repeated `/dnd` re-mark where a repeated `/afk` clears.
                let out = super::away::dnd_line(&send.text, is_dnd(&self_q), &strings);
                if let Some(line) = out.line {
                    super::away::push_system(&mut chat_log, line);
                }
                out.body
            }
            // Any other type: clear a standing AFK first (`0x49f3d6` skips only type `0x14`), then
            // send the line's own packet. `/dnd` is type `0x15`, so it takes this path too — which
            // is why the reference prints THREE lines for a typed `/afk` then `/dnd`.
            _ => {
                if let Some(line) =
                    super::away::auto_clear_line(*mirror, auto_clear_afk(&cvars), &strings)
                {
                    super::away::push_system(&mut chat_log, line);
                    mirror.0 = 0;
                    // The empty `0x14` that tells the server, alongside the message's own packet.
                    let _ = commands.0.send(ClientCommand::Chat {
                        kind: crate::net::ChatKind::Afk,
                        target: None,
                        text: String::new(),
                    });
                }
                send.text
            }
        };
        let cmd = ClientCommand::Chat {
            kind: wire,
            target: send.target,
            text,
        };
        if commands.0.send(cmd).is_err() {
            warn!("chat: not connected; addon line dropped");
        }
    }
}

/// Turn an addon's `SendAddonMessage` calls into sends — the addon-to-addon lane,
/// not speech.
///
/// Its own drain rather than a branch in [`drain_addon_chat_sends`] because the two queues carry
/// different things: a `SendChatMessage` line still needs a chat-type token resolved and may be
/// refused here, while a `SendAddonMessage` broadcast arrives **already validated** — the binding
/// owns the four-value whitelist, the `prefix` TAB `message` composition and the outside-a-raid
/// downgrade, exactly as the reference's `0x49f920` does, so nothing is left for this side to
/// decide. That is why there is no "unknown distribution" arm here and no way to add one: the
/// queue's own type is the enum.
///
/// **The lane is traced in both directions, under one tag.** An addon channel is invisible by
/// construction — it never reaches a chat frame, which is the entire point of `LANG_ADDON` — so
/// without a line here a broadcast that went out and one that never happened look identical from
/// our own logs, which is exactly how a silently-discarded wire body survives (docs/METHOD.md's rule
/// that new wire bodies are proved, not assumed). `ui_chat::net` already writes inbound addon
/// traffic to the `addon` trace tag; this writes the outbound half to the same tag, so
/// `WOW_MOVE_TRACE=<path> WOW_MOVE_TRACE_TAGS=addon` on a live run is the whole conversation in
/// one file, in order.
pub(super) fn drain_addon_message_sends(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for send in script.take_addon_sends() {
        debug!(
            "chat: addon broadcast on {} — {:?}",
            send.distribution.token(),
            send.text
        );
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "addon",
                &format!("-> {} {:?}", send.distribution.token(), send.text),
            );
        }
        let cmd = ClientCommand::AddonMessage {
            distribution: send.distribution,
            text: send.text,
        };
        if commands.0.send(cmd).is_err() {
            warn!("chat: not connected; addon broadcast dropped");
        }
    }
}
