//! The submitted-line side of the chat: [`drain_chat_input`] executes what [`parse`] resolved and
//! the verbs the stock `ChatFrame.lua` handlers queue in the VM; the addon drains send
//! `SendChatMessage` and `SendAddonMessage`. The emote arm is `DoEmote` (`0x5ef560`): the
//! eligibility gate, the asleep gate, the stow, the posture, then `CMSG_TEXT_EMOTE`. Own sends are
//! never echoed locally; the server echoes them.

use bevy::prelude::*;

mod parse;
#[cfg(test)]
pub(super) use parse::lua_quoted_string;
pub(super) use parse::{parse_line, ParsedChat};

use crate::creature_anim::{move_flags, MovementState};
use crate::net::{ClientCommand, NetCommands, SelfPlayer};
use crate::target::Selection;

/// The target half of `GetSlashCmdTarget` (`ChatFrame.lua:650-658`): a bare command falls back
/// to the selection only when it is a player. A streamed player's name is always cached.
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

/// The target guid a `CMSG_TEXT_EMOTE` carries: the selection, except that an emote at yourself
/// goes out untargeted (`0x5ef611`), so 1.12 has no self-emote sentence. Compared by entity, which
/// agrees with the guid: [`Selection`]'s one writer sets both from one streamed entity.
pub(super) fn emote_target(selection: &Selection, me: Option<Entity>) -> u64 {
    match selection.guid {
        Some(guid) if !(me.is_some() && selection.target == me) => guid,
        _ => 0,
    }
}

/// The inputs of `/shot`, `/liquid` and `/reaction`, bundled because [`drain_chat_input`] is at
/// Bevy's 16-parameter ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ChatProbes<'w, 's> {
    camera: Query<
        'w,
        's,
        &'static GlobalTransform,
        (With<benilla_world::view::WorldCamera>, Without<SelfPlayer>),
    >,
    clock: Option<Res<'w, benilla_world::lighting::GameClock>>,
    world: benilla_world::world_point::WorldPoint<'w, 's>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    self_store: Query<'w, 's, &'static crate::net::ObjectStore, With<SelfPlayer>>,
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Res<'w, crate::net::Reputations>,
    /// `/reaction <name>`'s resolve, for a player a scripted probe cannot click.
    guids: Res<'w, crate::net::GuidIndex>,
    kinds: Query<'w, 's, &'static crate::net::NetEntity>,
    /// `/partytest raid` seats us as the leader, and the wire names a leader by guid.
    self_guid: Res<'w, crate::net::SelfGuid>,
}

/// What the drain hands to another subsystem's setter, bundled for the same ceiling. Chat never
/// writes [`Selection`]: [`crate::target`]'s resolver commits through the click's path.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ChatOut<'w, 's> {
    /// A `/console` line runs against the world at the next sync point.
    console: Commands<'w, 's>,
    stand: MessageWriter<'w, crate::player::StandStateRequest>,
    sheath: MessageWriter<'w, crate::creature_anim::SheathRequest>,
    target: MessageWriter<'w, crate::target::TargetByNameRequest>,
    assist: MessageWriter<'w, crate::target::AssistRequest>,
    follow: MessageWriter<'w, crate::player::FollowRequest>,
    /// `/partytest ping` seats a group member's ping through the wire arm's `seat`.
    ping: ResMut<'w, crate::minimap::MinimapPing>,
    /// The red error line by GlobalStrings key. The system's only `UiErrorKeys` access: a second
    /// one panics on the first frame.
    ui_errors: ResMut<'w, crate::ui_action::UiErrorKeys>,
}

/// The verbs the stock `ChatFrame.lua` handlers call once they have parsed a line (`DoEmote`,
/// `RandomRoll`, `UninviteByName`, `ConsoleExec`, the channel verbs), drained from the VM as
/// [`ParsedChat`] values, so one executor serves them and the lines the Lua never sees.
fn engine_verbs(
    script: &mut benilla_ui::script::UiScript,
    emotes: Option<&crate::sound::EmoteSounds>,
) -> Vec<(String, ParsedChat)> {
    let mut out = Vec::new();
    for e in script.take_emote_requests() {
        // The token is the `EmotesText.dbc` name (`EMOTE<i>_TOKEN`, "WAVE").
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
        // `ConsoleExec` already applied a valued CVar line; what reaches here is a console
        // command (`detailDoodadAlpha`, registered by `0x63f9e0`, never persists) or a bare CVar
        // name to print.
        out.push((line.clone(), ParsedChat::Console { line }));
    }
    for cmd in script.take_channel_commands() {
        out.push((String::new(), ParsedChat::Channel(cmd)));
    }
    out
}

/// A manual join or leave of `GuildRecruitment` turns its auto-join option off: `0x49ed3d` (join)
/// and `0x49ef8f` (leave) call `0x49ea70(0)` when the `ChatChannels.dbc` row has `flags & 0x20000`
/// and the caller passes its flag, which the Lua bindings do and the cascade's own calls do not.
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
    // Mutable because an explicit leave clears the channel's `ZONECHANNELS` bit (`0x49f10a` in
    // leave-by-name `0x49ee70`); the zone walk's own leave does not.
    mut channels: ResMut<super::edit::ChannelState>,
    commands: Res<NetCommands>,
    emotes: Option<Res<crate::sound::EmoteSounds>>,
    selection: Res<Selection>,
    mut group: ResMut<crate::ui_party::GroupState>,
    mut names: ResMut<crate::names::NameCache>,
    // Our stand state and move flags for the emote gates; the entity is `/castvis`'s fallback.
    self_player: Query<(Entity, &MovementState, &GlobalTransform), With<SelfPlayer>>,
    mut cast_events: MessageWriter<crate::creature_anim::CastEvent>,
    mut play_seq: ResMut<crate::creature_anim::PlaySeq>,
    mut go_targets: MessageWriter<crate::creature_anim::SpellGoTargets>,
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
    // benilla's own commands, from the `SlashCmdList` entries in `ScriptLogFrame.xml` once the
    // stock `ChatEdit_ParseText` has found no built-in, and probe lines.
    for raw in script.take_chat_input() {
        let msg = raw.trim();
        if msg.is_empty() {
            continue;
        }
        // A probe's line with no slash is a SAY, which is also how a `.gm`-style server command
        // travels; a typed one goes out through the stock `ChatEdit_SendText` instead.
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
            // `/r` is the stock `ChatEdit_ParseText`'s REPLY arm; a typed one never gets here.
            ParsedChat::Reply { .. } => {}
            // Stock built-ins (`ChatFrame.lua:778`, `:803`), so only a probe's line gets here, and
            // it runs the same handler: sent verbatim, `/join General` would create a custom
            // channel of that name. The handlers queue their verbs for the `Channel` arm.
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
                // The camera pose in a capture `Scenario`'s raw WoW coords, to chat and a file.
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
                // Every candidate footprint, so a wrong surface shows beside the right one.
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
                    // The camera eye has its own claim (`[0xc7b748]`), and it decides the
                    // underwater filter.
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
                // In the ladder's order: the duel gates, the faction work, then `can_attack`.
                let subject_guid = match &name {
                    Some(n) => crate::ui_duel::streamed_player_named(n, guids, &names),
                    None => selection.guid,
                };
                let subject_entity = subject_guid.and_then(|g| guids.0.get(&g).copied());
                let target_store = subject_entity.and_then(|e| stores.get(e).ok());
                let own_store = self_store.iter().next();
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
                // The at-war bit: the whole of the player-to-unit reaction's leg 3 for a faction
                // with a reputation slot, which the cursor, the plate and `UnitCanAttack` turn on.
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
                // The world cursor's two predicates in `0x482200`'s order: `CanInteract` picks the
                // service ladder, then `CanAttack` the sword. Neither is a reaction threshold.
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
                // The V-plate category is its own predicate (`CanAttack` player-to-unit, plus
                // `CanCooperate` for a player), printed with the faction-group masks it compares.
                // A GM-mode character is mask 0 on vmangos (`.gm on` sets faction template 35),
                // so a friendly player can plate as an enemy.
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
                // 25 synthetic raid rows, us leading.
                "raid" => {
                    // The wire arm's by-key queue, so the lines resolve from GlobalStrings with
                    // each row's surface and sound.
                    chat_out.ui_errors.0.extend(crate::ui_party::synthetic_raid(
                        &mut group,
                        &mut names,
                        self_guid.0,
                    ));
                }
                "invite" => group.pending_invite = Some("Partner".to_string()),
                // A group member's ping 35 yd north-east: off both axes, so a mirrored sign
                // shows, and inside every outdoor view radius (the tightest is 66.7 yd).
                "ping" => {
                    let line = if let Ok((_, _, tf)) = self_player.single() {
                        let w = benilla_assets::coords::bevy_to_wow(tf.translation());
                        // +x is north, -y is east.
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
                // Skull the target on the local board; a real mark waits for the server's echo.
                "mark" => {
                    if let Some(guid) = selection.guid {
                        group.apply_raid_target(7, guid);
                    }
                }
                arg => {
                    let player_xy = self_player.single().ok().map(|(_, _, tf)| {
                        let w = benilla_assets::coords::bevy_to_wow(tf.translation());
                        (w[0], w[1])
                    });
                    chat_out
                        .ui_errors
                        .0
                        .extend(crate::ui_party::synthetic_roster(&mut group, player_xy));
                    if arg == "lead" {
                        // An unmatched leader guid reads as `leader_index` 0, "we lead", which
                        // shows the leader-only popup rows.
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
            // The server judges it: a raid-typed `SMSG_GROUP_LIST`, or an
            // `SMSG_PARTY_COMMAND_RESULT` error.
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
                    // `CMSG_GROUP_SET_LEADER` takes a guid, resolved against the roster. A miss
                    // prints the server's error wording; the reference's `PromoteByName` miss is
                    // untraced.
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
                            // `ERR_TARGET_NOT_IN_GROUP_S` (`GlobalStrings.lua:1861`).
                            chat_log.push_event(super::event::ChatEvent::text_only(
                                super::event::ChatEventKind::System,
                                format!("{name} is not in your party."),
                            ));
                        }
                    }
                }
            }
            // The duel verbs join the intent queue `StartDuel` and `CancelDuel` feed: the stock
            // handlers are one-line calls to them.
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
            // `crate::target`'s resolver (the reference's `0x493aa0`) commits through the one
            // SetSelection path. A bare `/target` takes `GetSlashCmdTarget`'s fallback: a no-op
            // re-select of a player target, nothing for a creature.
            ParsedChat::Target { name } => {
                if let Some(name) =
                    name.or_else(|| target_player_name(&selection, &names, &commands))
                {
                    chat_out
                        .target
                        .write(crate::target::TargetByNameRequest { name });
                }
            }
            // Deviation: a probe's bare `/assist` assists the live selection, where the stock
            // handler (`ChatFrame.lua:740`) resolves a selected player's name through
            // `AssistByName`, because a name resolve can land on a different player.
            ParsedChat::Assist { name } => {
                chat_out.assist.write(crate::target::AssistRequest { name });
            }
            // Bare is `FollowUnit("target")`, where the stock handler (`ChatFrame.lua:754`) first
            // tries a selected player's name; named is `FollowByName(name)` with no exact flag, so
            // prefix matching.
            ParsedChat::Follow { name } => {
                chat_out.follow.write(match name {
                    Some(name) => crate::player::FollowRequest::Name { name, exact: false },
                    None => crate::player::FollowRequest::Unit("target".into()),
                });
            }
            // `SlashCmdList["PVP"]` is `TogglePVP()`, the popup row's intent queue.
            ParsedChat::Pvp => script.queue_pvp_toggle(),
            ParsedChat::Help => {
                // benilla's own summary, for a line that skipped the edit box; a typed `/help`
                // runs the stock `ChatFrame_DisplayHelpText`.
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
            // `ChatFrame_DisplayMacroHelpText` (`ChatFrame.lua:1695`), off the VM's GlobalStrings.
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
            // `DoEmote` (`0x5ef560`): the gates in the client's order, then posture and packet.
            ParsedChat::TextEmote(text_id) => {
                let (stand_state, flags) = self_player
                    .single()
                    .map_or((0, 0), |(_, m, _)| (m.stand_state, m.flags));
                let swimming = flags & move_flags::SWIMMING != 0;
                let emote_id = emotes.as_deref().and_then(|e| e.text_emote(text_id));
                // A chat-only text emote (no Emotes.dbc row) has no flags or posture: it sends.
                let posture = emote_id.and_then(|id| emotes.as_deref()?.posture_state(id));
                // Gate A (`CheckEmoteEligible`, `0x47db40`) suppresses the anim and the packet,
                // except its `0x4000` arm, which refuses out loud.
                let gate = match emotes.as_deref() {
                    Some(e) => emote_id
                        .and_then(|id| e.emote_flags(id))
                        .map_or(EmoteGate::Send, |f| {
                            emote_send_eligible(f, stand_state, swimming, flags)
                        }),
                    None => EmoteGate::Send,
                };
                // `DoEmote`'s half of the `0x4000` arm (`0x5ef5d0`): the red line only while
                // self-controlled, so a feared or confused player emotes normally.
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
                // Gate B (`0x5ef5f3`): asleep, only a posture emote gets through, so `/stand` ends
                // `/sleep` and a `/wave` in bed does nothing.
                let awake_or_posture = stand_state != 3 || posture.is_some();
                if !eligible || !awake_or_posture {
                    // `DoEmote` returns before the packet: no send, no anim.
                    debug!(
                        "chat: emote {text_id} suppressed (eligible {eligible}, \
                         awake-or-posture {awake_or_posture}, stand {stand_state})"
                    );
                    continue;
                }
                // The stow (`0x5ef630`), unconditional here: every emote past the gates sheathes
                // instantly, mid-fight too.
                if let Ok((entity, _, _)) = self_player.single() {
                    chat_out.sheath.write(crate::creature_anim::SheathRequest {
                        entity,
                        state: 0,
                        ceremony: false,
                    });
                }
                // The posture branch (`EmoteSpecProc == 1` → `SetStandState`) is the whole
                // visible effect: the server plays nothing for SIT, SLEEP and KNEEL
                // (`ChatHandler.cpp:728`).
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
                // The net bridge's own message, on the selection or else self.
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
                        // A selected caster's GO hits us, so a missile flies end to end; a
                        // self-cast has no target. `ground` sends the pure dest shape instead, a
                        // point 15 yd ahead at the player's height.
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
                                    // A real GO carries the caster's ammo; this flies the
                                    // Rough Arrow, display id 5996.
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
            // `SlashCmdList["LOGOUT"]` is `Logout()`, the game menu's route: `crate::ui_logout`
            // sends the request and narrates the answer with the CAMP countdown.
            ParsedChat::Logout => {
                script.queue_session_request(benilla_ui::script::SessionRequest::Logout)
            }
            // `Quit()`, the game menu Exit button's queue, countdown and confirmation.
            ParsedChat::Quit => {
                script.queue_session_request(benilla_ui::script::SessionRequest::Quit)
            }
            // The deferred rebuild `ReloadUI()` queues.
            ParsedChat::ReloadUi => {
                script.queue_session_request(benilla_ui::script::SessionRequest::ReloadUi)
            }
            // Deferred to the world, since a command is `fn(&mut World, &str)`; its lines print
            // as system text.
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
            // The reference's handler body, run in the VM.
            ParsedChat::Lua { body } => {
                if let Err(e) = script.run(&body) {
                    // Logged and printed: the player sees a `/script` typo, the log a failing
                    // built-in.
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
                        // `LeaveChannelByName` (`0x4a0000` → `0x49ee70`): a number names a
                        // confirmed slot or the call does nothing. Only this path clears the zone
                        // mask and the custom re-join list.
                        let Some(name) = channels.leave_target(&name) else {
                            continue;
                        };
                        manual_join_or_leave(&channels, &mut script, &name);
                        channels.note_zone_channel_left(&name);
                        channels.note_custom_channel_left(&name);
                        ClientCommand::LeaveChannel { name }
                    }
                    C::List { name } => ClientCommand::ChannelList { name },
                    // `ListChannels()`: the joined roster, numbered as `/N` addresses it.
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
                // `SlashCmdList` gets the line before the help reply: an addon's command, or a
                // stock one with no arm here.
                if let Some(rest) = msg.strip_prefix('/') {
                    let rest = rest.trim();
                    let (cmd, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    if script.run_slash_command(cmd, args.trim()) {
                        continue;
                    }
                }
                // `HELP_TEXT_SIMPLE`, the reference's unknown-command reply (`ChatFrame.lua:2205`),
                // a plain chat line with no message-catalog surface or sound.
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

/// `/chattest`: one synthetic line of every renderable form (kinds, flags, the language header,
/// channel prefixes, notices, item and player links) through the real event pipeline.
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

/// The combat-log half of `/chattest`: one line of every family through the real
/// [`super::combat`] composer and drain. Guid 0 on both ends and literal names in the fills make
/// the name resolve a no-op; everything after it is the production path.
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
        // A creature hitting you: the red rows.
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
        // Your own spells, and the outcomes that are not damage.
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
        // ── The remaining families, in the order a player meets them ─────────────────────
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
        // The two families whose `Named` slot is not an item: a faction and a failure reason.
        line(
            K::CombatFactionChange,
            combat::FACTION_STANDING_INCREASED,
            Variant::OtherOther,
            Fills {
                named: "Stormwind".into(),
                ..fills(250, None, None)
            },
        ),
        // A resolved GlobalString, as production fills this slot: `ERR_OUT_OF_MANA`, the key
        // the NO_POWER pick table names (`0x8118dc`).
        line(
            K::SpellFailedLocalPlayer,
            combat::SPELLFAILCAST,
            Variant::SelfOther,
            Fills {
                named: get("ERR_OUT_OF_MANA").unwrap_or_default(),
                ..fills(0, None, None)
            },
        ),
        // The trailers, on the one family that can show all six.
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

/// What `CheckEmoteEligible` (`0x47db40`) decides about one emote.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum EmoteGate {
    /// Send it: packet, posture, animation.
    Send,
    /// Dropped silently: no packet, no animation, no line. A seated `/bow` is this.
    Suppressed,
    /// The `0x4000` "requires standing still" arm tripped. `DoEmote` (`0x5ef5d0`) raises
    /// `ERR_NOEMOTEWHILERUNNING` and aborts only when `IsSelfControlled` (`0x5fa550`), false while
    /// `UNIT_FIELD_FLAGS & 0xc00004`, so a feared player emotes; that test is the caller's.
    Moving,
}

/// `CheckEmoteEligible` (`0x47db40`), the only reader of `Emotes.dbc` `EmoteFlags`, in the
/// client's order; `DoEmote` (`0x5ef560`) calls it before building `CMSG_TEXT_EMOTE`. The `0x4000`
/// arm tests movement, not `SWIMMING`, so it is no water gate.
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
    // `0x4000` "requires standing still" (client `0x47dbab`), against the live movement word's
    // `0x20ff` at `0x47dbb3` (direction, turn and pitch bits and FALLING), which is
    // [`crate::creature_anim::move_flags::INTEGRATED`]. It only reports; `DoEmote` decides.
    if emote_flags & 0x4000 != 0 && move_flags & crate::creature_anim::move_flags::INTEGRATED != 0 {
        return EmoteGate::Moving;
    }
    EmoteGate::Send
}

/// `PLAYER_FLAGS_DND` (`0x4`) off our live descriptor. Live, not mirrored, as in the reference
/// (`0x49f3f0`): AFK keeps an optimistic global and DND nothing, so a second `/dnd` before the
/// server's update re-marks where a second `/afk` clears.
fn is_dnd(self_q: &Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>) -> bool {
    self_q
        .iter()
        .next()
        .is_some_and(|s| s.0.player_flags() & 0x4 != 0)
}

/// `autoClearAFK`, registered default `"1"` (`0x5e24d4`). Off, the clear is a total no-op: no
/// echo, no mirror write, no packet (`0x5eb84b`).
fn auto_clear_afk(cvars: &crate::cvars::Cvars) -> bool {
    cvars.flag("autoClearAFK").unwrap_or(true)
}

/// Turn `SendChatMessage` calls into sends. No slash grammar runs here: an addon sending
/// `"/dance"` says six characters. An unknown chat-type token prints a system line and sends
/// nothing.
pub(super) fn drain_addon_chat_sends(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    commands: Res<NetCommands>,
    mut chat_log: ResMut<super::feed::ChatLog>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
    // The optimistic AFK mirror (`[0xb6e5cc]`) the `/afk` toggle reads and writes.
    mut mirror: ResMut<super::away::AfkMirror>,
    cvars: Res<crate::cvars::Cvars>,
    // Our descriptor, for the DND arm's live `PLAYER_FLAGS & 0x4`.
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
        // `SendChatMessage`'s tutorial acknowledge (`0x49f5fc`) covers the twelve social types,
        // not say, yell or emote.
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
        // ── The away commands, and the AFK clear the other sends carry ───────────────────────
        //
        // `SendChatMessage` (`0x49f1e0`): AFK and DND each have an arm ahead of the generic send,
        // and every type but AFK (`0x14`) first clears a standing AFK. The system lines, the
        // default text and the mirror are `super::away`'s.
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
                // DND has no mirror (`0x49f3f0`): the live descriptor bit. The reference's DND
                // also clears a standing AFK first (`0x49f3d6`); this arm does not.
                let out = super::away::dnd_line(&send.text, is_dnd(&self_q), &strings);
                if let Some(line) = out.line {
                    super::away::push_system(&mut chat_log, line);
                }
                out.body
            }
            // Clear a standing AFK first (`0x49f3d6` skips only type `0x14`), then send the
            // line's own packet.
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

/// Turn `SendAddonMessage` calls into sends. The binding has already applied the reference's
/// rules (`0x49f920`: the four distributions, `prefix` TAB `message`, the downgrade outside a
/// raid), so nothing is refused here. This writes the outbound half of the `addon` trace tag and
/// `ui_chat::net` the inbound: `WOW_MOVE_TRACE=<path> WOW_MOVE_TRACE_TAGS=addon` logs both.
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
