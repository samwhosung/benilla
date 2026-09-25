//! The probe **rig** (`WOW_RIG="<spec>"`) — one command that puts this checkout's probe
//! account into a chosen **body** and hands the session a world that is ready to test.
//!
//! ## Why it exists
//!
//! Every probe account owns one level-1 character, and every session that needs anything else has
//! been hand-assembling it out of GM commands. Mined from 507 MB of this project's own session
//! transcripts, the standing tax is: `.go xyz` 750×, `.additem` 434×, `.modify` 345×, `.gm off`
//! 257×, `.revive` **121×**, `.learn <one spell id>` a few dozen times — while
//! `.character premade` (96 ready-made BiS/twink **gear** templates plus 53 **talent** ones sitting
//! in this deploy's world DB, every class at 19/29/39/49/60) has been used **zero** times, because
//! nobody knew it was there. That is the shape of the problem: the primitives exist and are undiscoverable, so
//! each session re-derives a worse version of them.
//!
//! The rig is the one verb that hides all of it: name a body, get a body.
//!
//! ```text
//! WOW_RIG="tauren druid 60 gear:heal-preraid-bis spec:heal-preraid-bis at:ThunderBluff"
//! WOW_RIG="gnome mage 39 gear:dps-39-twink gm:off"
//! WOW_RIG="60 gear:dps-preraid-bis"          # the account's own probe body, no new character
//! WOW_RIG="nightelf druid"                   # just a body of that shape, level 1
//! ```
//!
//! ## What it does
//!
//! **Race + class name a character, so the rig owns the pick.** vmangos has no class-change command
//! (`.character race` exists; there is no `.character class`), so a different class *is* a different
//! character — which makes creating one the honest primitive rather than a fallback. The name is
//! derived, never invented: `<Race3><Class3><word>[f]`, the word being this checkout's
//! (`run_mode::rig_suffix`), e.g. a Tauren Druid for `Probeone` → `Taudruone`. Deterministic means
//! the *next* session reuses the same body instead of littering a second one, and the pattern is
//! what makes eviction safe (below). Omit race+class and the rig configures whatever `WOW_CHAR`
//! already logs in as.
//!
//! **The roster is a cache with an eviction policy.** `CharactersPerRealm` is 10, and there are 40
//! valid race/class pairs, so a busy account fills. The rig does not hardcode the limit: it tries
//! the create, and only on `CHAR_CREATE_SERVER_LIMIT` evicts **the rig-named character of this checkout
//! that is cheapest to rebuild** — lowest level first, ties broken by oldest — then retries once.
//! Nothing outside the rig's own naming pattern is ever deleted, so `Probe<N>` and anything a human
//! made are untouchable.
//!
//! **Then it applies state, in an order that matters:** revive before anything (a ghost's commands
//! half-apply), level before gear (`ApplyPremadeGearTemplateToPlayer` only levels *up*, never
//! down), spells after level, teleport last (so a fixup lands where you asked, not where you were).
//! It always revives a dead or ghost body, whether or not you asked — that is the 121× command, and
//! there is no session that wants to keep testing on a corpse.
//!
//! ## The GM verbs behind it (all verified against the vmangos source, github.com/vmangos/core)
//!
//! | rig token | command | needs | note |
//! |---|---|---|---|
//! | (always, if dead) | `.revive` | SEC_GAMEMASTER 3 | |
//! | `<level>` | `.character level N` | SEC_DEVELOPER 5 | no name arg ⇒ self |
//! | (with a level) | `.learn all_myclass` | SEC_DEVELOPER 5 | = all class spells **+** all talents |
//! | `gear:<t>` | `.character premade gear <t>` | SEC_BASIC_ADMIN 4 | name or entry; levels up + equips |
//! | `spec:<t>` | `.character premade spec <t>` | SEC_BASIC_ADMIN 4 | resets talents, learns the tree |
//! | `at:<name>` | `.tele <name>` | SEC_TICKETMASTER 2 | 997 named rows in `game_tele` |
//! | `at:m,x,y,z[,o]` | `.go xyz x y z m` (`.go xyzo` with `o`) | SEC_TICKETMASTER 2 | pin the facing when a motion probe walks |
//! | `gm:on\|off` | `.gm on\|off` | SEC_GAMEMASTER 3 | see 0649 on why `off` matters |
//!
//! Probe accounts are gmlevel **6**, so every one of these lands. Pass `gear:?` (or `spec:?`) to
//! make the server *list* the templates its class has instead of applying one — the discovery path,
//! since the catalog lives in the world DB and not in any client data.
//!
//! **Two things about `gear:` that have each cost a session.** First, the templates are not all full
//! sets: of the 96 in this deploy, `pvp-r14-hunter-fx` holds 5 items and the priest/warrior
//! `heal-r14`/`tank-r14` hold 8, against a 16.8 mean — so a sparse body can be exactly what the
//! template says, and only a body under `GEAR_FLOOR` is evidence of a refusal. Second, applying gear
//! **strips before it dresses** (`ApplyPremadeGearTemplateToPlayer` unequips all 19 slots, then
//! `StoreNewItemInBestSlots` per item), so re-dressing a body leaves the old copy in its bags and
//! mails the overflow once they fill. Repeated rigs on one character accumulate; a fresh rig-named
//! character (give `WOW_RIG` a race+class) starts with empty bags and is the cheaper path.
//!
//! Non-combat throughout: the rig creates, configures, places and stops. It never fights, so the
//! unattended-combat ban (docs/METHOD.md) is untouched.

use benilla_protocol::{messages, CharAction, CharCreateReq};
use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::char_select::{send_pick, Roster};
use crate::net::{
    CharActionResultMessage, CharListMessage, CharPick, CharRequest, EnteredWorldMessage,
    ObjectStore, SelfPlayer,
};

/// Grace after world entry before the first GM line — the descriptor and the UI VM both have to be
/// up, and a command sent into a half-built session is silently dropped.
const SETTLE_SECS: f32 = 3.0;

/// Spacing between GM lines. Each one is a server-side mutation whose result the next may depend on
/// (level → gear → spec), and two flips inside one net drain merge to a no-op (0441's lesson).
const STEP_SECS: f32 = 0.8;

/// Grace after the last GM line before reading the result back — the level-up, the equip sweep and
/// the teleport all land as descriptor deltas a frame or two later.
const VERIFY_SECS: f32 = 2.0;

/// The fewest items any real gear template in this deploy's world DB actually holds, so a body that
/// wears fewer than this after a `gear:` **cannot** be the template landing — something refused.
///
/// Derived, not invented: `SELECT COUNT(*) … player_premade_item GROUP BY entry` over the 96 rows of
/// `player_premade_item_template` runs 5…23 (mean 16.8). Only three templates are partial —
/// `pvp-r14-hunter-fx` (5), and the priest/warrior `heal-r14`/`tank-r14` (8 each) — so a **low but
/// legitimate** count is a real outcome and this floor deliberately sits under it: the warning fires
/// only for a body that is essentially naked. (Entry 910 has one item row and no template row at
/// all — an orphan; asking for it by entry errors server-side.)
const GEAR_FLOOR: usize = 5;

/// Did a `gear:` ask come back with a body that cannot be wearing the template? `gear:?` is the
/// discovery path — it lists and dresses nothing, so it never counts as a refusal.
fn gear_was_refused(gear: Option<&str>, equipped: usize) -> bool {
    gear.is_some_and(|g| g != "?") && equipped < GEAR_FLOOR
}

pub(crate) struct ProbeRigPlugin;

impl Plugin for ProbeRigPlugin {
    fn build(&self, app: &mut App) {
        let Some(spec) = std::env::var("WOW_RIG")
            .ok()
            .as_deref()
            .and_then(RigSpec::parse)
        else {
            return; // inert without a parseable spec (parse() has already said why)
        };
        info!("rig: {}", spec.describe());
        // Publish the claim on the character pick as an always-present run fact.
        // The roster reads it to know the pick is already spoken for; before 1174 it called
        // `rig_char_name_from_env` directly, which is gameplay naming the harness.
        if let Some(name) = rig_char_name(&spec) {
            app.insert_resource(crate::run_mode::RigCharacter(name));
        }
        app.insert_resource(Rig {
            spec,
            phase: RigPhase::AwaitRoster,
            roster: Vec::new(),
            evicted: false,
            steps: Vec::new(),
            sent: 0,
            next_at: 0.0,
        })
        .add_systems(Update, drive_rig);
    }
}

/// The parsed `WOW_RIG` spec. Every field is optional — the rig only touches what was asked for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct RigSpec {
    /// `(race id, class id, gender)` — present together or not at all; they name the character.
    body: Option<(u8, u8, u8)>,
    level: Option<u8>,
    gear: Option<String>,
    spec: Option<String>,
    /// `.tele <name>`, or `.go xyz` when the token parsed as `map,x,y,z`.
    at: Option<String>,
    gm: Option<bool>,
    /// Whether to `.learn all_myclass`. Defaults to "yes if a level was asked for" — a level-60
    /// body with a level-1 spellbook is not a level-60 body.
    learn: Option<bool>,
}

impl RigSpec {
    /// Parse the whitespace-separated, order-free, case-insensitive token soup. Returns `None` (with
    /// a `warn!` naming the offending token) rather than silently rigging the wrong thing.
    fn parse(spec: &str) -> Option<Self> {
        let mut out = Self::default();
        let (mut race, mut class, mut gender) = (None, None, None);
        for tok in spec.split_whitespace() {
            let lower = tok.to_ascii_lowercase();
            if let Some((key, _)) = lower.split_once(':') {
                // The KEY is matched case-insensitively, but the VALUE is taken from the original
                // token: a premade-template name and a `game_tele` name are both looked up verbatim
                // server-side, so lowercasing them would break the lookup.
                let val = tok.split_once(':').map_or("", |(_, v)| v).to_string();
                match key {
                    "gear" => out.gear = Some(val),
                    "spec" => out.spec = Some(val),
                    "at" => out.at = Some(val),
                    "gm" => out.gm = Some(matches!(val.to_ascii_lowercase().as_str(), "on" | "1")),
                    _ => {
                        warn!("rig: unknown token {tok:?} — expected gear:/spec:/at:/gm:");
                        return None;
                    }
                }
            } else if let Ok(level) = lower.parse::<u8>() {
                out.level = Some(level.clamp(1, 60));
            } else if let Some(id) = race_id(&lower) {
                race = Some(id);
            } else if let Some(id) = class_id(&lower) {
                class = Some(id);
            } else if lower == "male" || lower == "female" {
                gender = Some(u8::from(lower == "female"));
            } else if lower == "spells" || lower == "nospells" {
                out.learn = Some(lower == "spells");
            } else {
                warn!("rig: unknown token {tok:?} in WOW_RIG — nothing rigged");
                return None;
            }
        }
        match (race, class) {
            (Some(r), Some(c)) => out.body = Some((r, c, gender.unwrap_or(0))),
            (None, None) => {
                if gender.is_some() {
                    warn!(
                        "rig: a gender needs a race and a class to name a character — ignoring it"
                    );
                }
            }
            _ => {
                warn!("rig: give BOTH a race and a class (they name the character), or neither");
                return None;
            }
        }
        Some(out)
    }

    /// Whether the class's spellbook should be filled in (`learn:` if given, else "a level implies
    /// the spells that come with it").
    fn wants_spells(&self) -> bool {
        self.learn.unwrap_or(self.level.is_some_and(|l| l > 1))
    }

    /// The one-line echo of what was asked for — printed at startup so a mis-parse is obvious
    /// before the socket even opens.
    fn describe(&self) -> String {
        let body = self
            .body
            .map_or("the account's probe character".into(), |(r, c, g)| {
                format!(
                    "{} {} {}",
                    if g == 1 { "female" } else { "male" },
                    crate::ui_unit::race_names(r).map_or("?", |(d, _)| d),
                    crate::ui_unit::class_names(c).map_or("?", |(d, _)| d),
                )
            });
        let mut extras = Vec::new();
        if let Some(l) = self.level {
            extras.push(format!("level {l}"));
        }
        if self.wants_spells() {
            extras.push("all class spells + talents".into());
        }
        if let Some(g) = &self.gear {
            extras.push(format!("gear {g}"));
        }
        if let Some(s) = &self.spec {
            extras.push(format!("spec {s}"));
        }
        if let Some(a) = &self.at {
            extras.push(format!("at {a}"));
        }
        if let Some(gm) = self.gm {
            extras.push(format!("GM {}", if gm { "on" } else { "off" }));
        }
        if extras.is_empty() {
            body
        } else {
            format!("{body} — {}", extras.join(", "))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RigPhase {
    /// Parked at select, waiting for a roster to look our character up in.
    AwaitRoster,
    /// Sent `Create`, waiting for its result.
    Creating,
    /// Sent `Delete` (the roster was full), waiting for its result before retrying the create.
    Deleting,
    /// Sent `Enter`, waiting to be in the world.
    Entering,
    /// In the world; the GM batch starts at this `Time::elapsed_secs`.
    Settling(f32),
    /// Sending the batch, one line per [`STEP_SECS`].
    Commanding,
    /// The batch is out; re-read the descriptor at this `Time::elapsed_secs` and report what
    /// actually landed. A command the server refused is otherwise invisible — the send always
    /// "succeeds", and the preflight banner describes the body we *arrived* in, not the rigged one.
    Verifying(f32),
    Done,
}

#[derive(Resource)]
struct Rig {
    spec: RigSpec,
    phase: RigPhase,
    /// The freshest roster (each successful create/delete is preceded by a new enum).
    roster: Vec<benilla_protocol::Character>,
    /// A server-limit eviction has already been spent — one is a full roster, two is a bug.
    evicted: bool,
    steps: Vec<String>,
    sent: usize,
    next_at: f32,
}

/// The whole rig: find-or-create the body at select, enter as it, then apply the state batch.
fn drive_rig(
    mut rig: ResMut<Rig>,
    mut roster: ResMut<Roster>,
    pick: Res<CharPick>,
    time: ProbeClock,
    mut lists: MessageReader<CharListMessage>,
    mut results: MessageReader<CharActionResultMessage>,
    mut entered: MessageReader<EnteredWorldMessage>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
) {
    if let Some(list) = lists.read().last() {
        rig.roster = list.characters.clone();
    }
    let entered_world = entered.read().next().is_some();

    match rig.phase {
        RigPhase::AwaitRoster => {
            if rig.roster.is_empty() && rig.spec.body.is_none() {
                return; // no roster yet, and nothing to create — wait for the enum
            }
            let Some(want) = rig_char_name(&rig.spec) else {
                // No body asked for: let char_select's own WOW_CHAR/default pick stand, and just
                // configure whatever walks into the world.
                rig.phase = RigPhase::Entering;
                return;
            };
            match rig
                .roster
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&want))
            {
                Some(c) => {
                    let guid = c.guid;
                    info!("rig: reusing {want} (guid {guid})");
                    send_pick(&mut roster, &pick, guid);
                    rig.phase = RigPhase::Entering;
                }
                None if rig.roster.is_empty() => {} // the enum hasn't landed yet
                None => {
                    let (race, class, gender) = rig.spec.body.expect("name implies a body");
                    info!("rig: {want} does not exist — creating it");
                    let _ = pick.0.send(CharRequest::Create(CharCreateReq {
                        name: want,
                        race,
                        class,
                        gender,
                        skin: 0,
                        face: 0,
                        hair_style: 0,
                        hair_color: 0,
                        facial_hair: 0,
                    }));
                    rig.phase = RigPhase::Creating;
                }
            }
        }
        RigPhase::Creating => {
            let Some(r) = results.read().find(|r| r.action == CharAction::Create) else {
                return;
            };
            match r.code {
                messages::CHAR_CREATE_SUCCESS => rig.phase = RigPhase::AwaitRoster,
                messages::CHAR_CREATE_SERVER_LIMIT if !rig.evicted => {
                    rig.evicted = true;
                    match crate::run_mode::rig_suffix().and_then(|word| {
                        evictable(&rig.roster, rig_char_name(&rig.spec).as_deref(), &word)
                    }) {
                        Some((guid, name)) => {
                            warn!(
                                "rig: the account is full — evicting the cheapest rig character to rebuild, {name} \
                                 (guid {guid}). Only this checkout's rig-named characters are ever \
                                 deleted; Probe<N> and hand-made characters are never touched."
                            );
                            let _ = pick.0.send(CharRequest::Delete(guid));
                            rig.phase = RigPhase::Deleting;
                        }
                        None => {
                            error!(
                                "rig: the account is full and nothing on it is a rig character to \
                                 evict — delete one by hand, or rig on an existing body."
                            );
                            rig.phase = RigPhase::Done;
                        }
                    }
                }
                code => {
                    error!("rig: character create failed ({code:#04x}) — nothing rigged");
                    rig.phase = RigPhase::Done;
                }
            }
        }
        RigPhase::Deleting => {
            if results.read().any(|r| r.action == CharAction::Delete) {
                rig.phase = RigPhase::AwaitRoster;
            }
        }
        RigPhase::Entering => {
            if entered_world {
                rig.phase = RigPhase::Settling(time.elapsed_secs() + SETTLE_SECS);
            }
        }
        RigPhase::Settling(at) => {
            if time.elapsed_secs() < at {
                return;
            }
            let Ok(store) = self_q.single() else { return };
            let dead = store.0.unit_is_dead() || store.0.player_is_ghost();
            rig.steps = build_steps(&rig.spec, dead);
            if rig.steps.is_empty() {
                info!("rig: nothing to apply — the body is already what was asked for");
                rig.phase = RigPhase::Done;
                return;
            }
            rig.next_at = time.elapsed_secs();
            rig.phase = RigPhase::Commanding;
        }
        RigPhase::Commanding => {
            let Some(mut script) = script else { return };
            while rig.sent < rig.steps.len() && time.elapsed_secs() >= rig.next_at {
                let line = rig.steps[rig.sent].clone();
                info!("rig: {line}");
                script.push_chat_input(line);
                rig.sent += 1;
                rig.next_at = time.elapsed_secs() + STEP_SECS;
            }
            if rig.sent == rig.steps.len() {
                rig.phase = RigPhase::Verifying(time.elapsed_secs() + VERIFY_SECS);
            }
        }
        RigPhase::Verifying(at) => {
            if time.elapsed_secs() < at {
                return;
            }
            let Ok(store) = self_q.single() else { return };
            let equipped = (0..19)
                .filter(|&i| store.0.player_visible_item_entry(i).is_some())
                .count();
            info!(
                "rig: done — {sent} command(s), and the body now reads: level {level}, {hp}/{maxhp} hp, \
                 {equipped} item(s) equipped, faction template {faction}. Re-run the same WOW_RIG to \
                 get this body back.",
                sent = rig.sent,
                level = store.0.unit_level().unwrap_or(0),
                hp = store.0.unit_health().unwrap_or(0),
                maxhp = store.0.unit_max_health().unwrap_or(0),
                faction = store.0.unit_faction_template().unwrap_or(0),
            );
            // The one failure that reads as success: the batch went out, the server refused every
            // line, and the body is exactly as found. Level is the cheapest tell.
            if let Some(want) = rig.spec.level {
                let got = store.0.unit_level().unwrap_or(0);
                if got < u32::from(want) {
                    warn!(
                        "rig: asked for level {want} but the body is level {got} — the server \
                         refused the command. Check the account's GM level (`.character level` \
                         needs SEC_DEVELOPER 5): \
                         SELECT gmlevel FROM realmd.account_access WHERE id = <account>."
                    );
                }
            }
            // The other failure that reads as success: `.character premade gear` went out, the body
            // came back wearing almost nothing, and every later reading is of the wrong body. The
            // server unequips all 19 slots *before* it equips (`ApplyPremadeGearTemplateToPlayer` →
            // `AutoUnequipItemFromSlot` → `StoreNewItemInBestSlots`, vmangos `ObjectMgr.cpp`), so a
            // near-naked body means the equip half was refused while the strip half already ran —
            // the set is in the bags, or mailed if they were full. A session that misses this line
            // measures a probe in its underwear.
            if gear_was_refused(rig.spec.gear.as_deref(), equipped) {
                warn!(
                    "rig: asked for gear but the body wears only {equipped} item(s) — the leanest \
                     real template in this deploy holds {GEAR_FLOOR}, so the equip half was \
                     refused (the strip half runs first, so the set is in the bags or was mailed). \
                     Cheapest fix: rig a FRESH body — give WOW_RIG a race+class so it creates a \
                     new character with empty bags, rather than re-dressing this one."
                );
            }
            rig.phase = RigPhase::Done;
        }
        RigPhase::Done => {}
    }
}

/// The GM batch, in the one order that works: revive first (a ghost half-applies everything else),
/// level before gear (a premade template only levels *up*), spells after the level they belong to,
/// and the teleport last so a fixup lands where you asked rather than where you started.
fn build_steps(spec: &RigSpec, dead: bool) -> Vec<String> {
    let mut steps = Vec::new();
    if dead {
        steps.push(".revive".into());
    }
    if let Some(gm) = spec.gm {
        steps.push(format!(".gm {}", if gm { "on" } else { "off" }));
    }
    if let Some(level) = spec.level {
        steps.push(format!(".character level {level}"));
    }
    if spec.wants_spells() {
        steps.push(".learn all_myclass".into());
    }
    for (token, verb) in [(&spec.gear, "gear"), (&spec.spec, "spec")] {
        if let Some(t) = token {
            // `?` asks the server to LIST this class's templates instead of applying one — the
            // catalog lives in the world DB, so the server is the only thing that can enumerate it.
            let arg = if t == "?" { "" } else { t.as_str() };
            steps.push(format!(".character premade {verb} {arg}").trim_end().into());
        }
    }
    if let Some(at) = &spec.at {
        steps.push(match parse_point(at) {
            Some((map, x, y, z, Some(o))) => format!(".go xyzo {x} {y} {z} {o} {map}"),
            Some((map, x, y, z, None)) => format!(".go xyz {x} {y} {z} {map}"),
            None => format!(".tele {at}"),
        });
    }
    steps
}

/// `map,x,y,z[,o]` → `.go xyz x y z map`, or `.go xyzo x y z o map` when a facing is given
/// (VERIFIED vmangos `Chat.cpp:408`: `xyzo`, SEC_TICKETMASTER like `xyz`). Anything else is a
/// `game_tele` name. The facing is load-bearing exactly when a motion probe walks from the
/// pin: `W` follows the BODY's facing, and each probe character keeps whatever facing it last
/// had — an unpinned facing sent probe0 and probe4 down different routes from the same point,
/// which invalidated a cross-checkout A/B before 1462's sitting caught it.
fn parse_point(at: &str) -> Option<(i32, f32, f32, f32, Option<f32>)> {
    let mut parts = at.split(',').map(str::trim);
    let map = parts.next()?.parse().ok()?;
    let (x, y, z) = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    let o = match parts.next() {
        Some(o) => Some(o.parse().ok()?),
        None => None,
    };
    parts.next().is_none().then_some((map, x, y, z, o))
}

fn rig_char_name(spec: &RigSpec) -> Option<String> {
    let (race, class, gender) = spec.body?;
    let word = crate::run_mode::rig_suffix()?;
    let name = format!(
        "{}{}{word}{}",
        race_code(race)?,
        class_code(class)?,
        if gender == 1 { "f" } else { "" }
    );
    // vmangos caps player names at 12 (`MAX_PLAYER_NAME`); the longest this can build is
    // `Nel` + `Wlk` + `three` + `f` = 12. Normalized like the server does: leading capital, rest
    // lower.
    let mut chars = name.chars();
    Some(chars.next()?.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase())
}

/// The rig-named character of this checkout that costs least to lose: **lowest level first**, ties
/// broken by oldest (the enum is ordered by `create_time` — vmangos `HandleCharEnumOpcode` — so an
/// earlier index *is* older). Level is the proxy for invested setup: a level-1 body is 15 seconds to
/// rebuild, a geared 60 is a minute and a talent tree. Evicting by age alone would throw away the
/// most valuable body on the account first, which the live fill test made obvious.
///
/// Anything that is not a rig name for `word` — the probe body itself, a hand-made character,
/// another checkout's leftovers — is invisible here, and that is what makes automatic deletion safe.
///
/// `word` is a **parameter, not a `rig_suffix()` call inside**: reading the ambient word here made
/// the eviction test pass only in the checkout it was written in (whose rig names end `…one`) and
/// fail in every other — and in a checkout with no declaration, where `rig_suffix()` is `None` and
/// nothing matches at all. A unit test's answer must not depend on which directory the build
/// happened in.
fn evictable(
    roster: &[benilla_protocol::Character],
    want: Option<&str>,
    word: &str,
) -> Option<(u64, String)> {
    roster
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            !want.is_some_and(|w| c.name.eq_ignore_ascii_case(w)) && is_rig_name(&c.name, word)
        })
        .min_by_key(|(age, c)| (c.level, *age))
        .map(|(_, c)| (c.guid, c.name.clone()))
}

/// Whether a roster name was minted by [`rig_char_name`] for this checkout: a known race code, a
/// known class code, this checkout's word, and nothing but an optional `f` after it.
fn is_rig_name(name: &str, word: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(rest) = lower.get(3..).and_then(|r| r.get(3..)) else {
        return false;
    };
    let has_codes = RACE_CODES.iter().any(|(_, c)| lower.starts_with(c))
        && CLASS_CODES.iter().any(|(_, c)| lower[3..].starts_with(c));
    has_codes && (rest == word || rest == format!("{word}f"))
}

/// Race id → the 3-letter name code. `Nel`/`Und` rather than the DBC's own prefixes: unambiguous,
/// and readable in a roster line.
const RACE_CODES: [(u8, &str); 8] = [
    (1, "hum"),
    (2, "orc"),
    (3, "dwa"),
    (4, "nel"),
    (5, "und"),
    (6, "tau"),
    (7, "gno"),
    (8, "tro"),
];

/// Class id → the 3-letter name code (`wlk` for warlock — `war` is already the warrior's).
const CLASS_CODES: [(u8, &str); 9] = [
    (1, "war"),
    (2, "pal"),
    (3, "hun"),
    (4, "rog"),
    (5, "pri"),
    (7, "sha"),
    (8, "mag"),
    (9, "wlk"),
    (11, "dru"),
];

fn race_code(id: u8) -> Option<&'static str> {
    RACE_CODES.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
}

fn class_code(id: u8) -> Option<&'static str> {
    CLASS_CODES.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
}

/// Spec word → `ChrRaces.dbc` id. Both the one-word and the two-word spellings, because a spec is
/// typed by hand and `nightelf`/`night-elf` are the same intent.
fn race_id(word: &str) -> Option<u8> {
    Some(match word {
        "human" => 1,
        "orc" => 2,
        "dwarf" => 3,
        "nightelf" | "night-elf" | "nelf" => 4,
        "undead" | "scourge" | "forsaken" => 5,
        "tauren" => 6,
        "gnome" => 7,
        "troll" => 8,
        _ => return None,
    })
}

/// Spec word → `ChrClasses.dbc` id (6 and 10 are unused in 1.12).
fn class_id(word: &str) -> Option<u8> {
    Some(match word {
        "warrior" => 1,
        "paladin" => 2,
        "hunter" => 3,
        "rogue" => 4,
        "priest" => 5,
        "shaman" => 7,
        "mage" => 8,
        "warlock" => 9,
        "druid" => 11,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_naked_body_after_a_gear_ask_is_a_refusal_but_a_lean_template_is_not() {
        // The three partial templates this deploy really ships (5 and 8 items) must stay quiet —
        // the floor exists to catch a stripped body, not to second-guess the world DB.
        assert!(!gear_was_refused(Some("pvp-r14-hunter-fx"), 5));
        assert!(!gear_was_refused(Some("tank-r14"), 8));
        assert!(
            gear_was_refused(Some("dps-preraid-bis"), 1),
            "a lone bow is the trap"
        );
        assert!(gear_was_refused(Some("dps-preraid-bis"), 0));
        assert!(!gear_was_refused(Some("?"), 0), "discovery dresses nothing");
        assert!(!gear_was_refused(None, 0), "no gear asked, no claim made");
    }

    #[test]
    fn a_body_spec_parses_in_any_order() {
        let a = RigSpec::parse("tauren druid 60 gear:heal-preraid-bis").unwrap();
        let b = RigSpec::parse("gear:heal-preraid-bis 60 druid tauren").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.body, Some((6, 11, 0)));
        assert_eq!(a.level, Some(60));
        assert_eq!(a.gear.as_deref(), Some("heal-preraid-bis"));
    }

    #[test]
    fn template_names_keep_their_case() {
        // The token soup is matched case-insensitively, but a premade template name and a
        // `game_tele` name are looked up VERBATIM server-side — lowercasing them would break both.
        let s = RigSpec::parse("at:ThunderBluff gear:DPS-PreRaid-BiS").unwrap();
        assert_eq!(s.at.as_deref(), Some("ThunderBluff"));
        assert_eq!(s.gear.as_deref(), Some("DPS-PreRaid-BiS"));
    }

    #[test]
    fn a_half_named_body_is_refused_rather_than_guessed() {
        // A race with no class (or the reverse) cannot name a character, and guessing the other
        // half would silently rig the wrong body.
        assert!(RigSpec::parse("tauren 60").is_none());
        assert!(RigSpec::parse("druid").is_none());
        assert!(RigSpec::parse("tauren druid").is_some());
        // No body at all is legal — that rigs whatever already logs in.
        assert_eq!(RigSpec::parse("60 gear:x").unwrap().body, None);
        // A typo is refused, never ignored.
        assert!(RigSpec::parse("taruen druid").is_none());
        assert!(RigSpec::parse("tauren druid lvl:60").is_none());
    }

    #[test]
    fn spells_follow_the_level_unless_asked_otherwise() {
        assert!(!RigSpec::parse("tauren druid").unwrap().wants_spells());
        assert!(RigSpec::parse("tauren druid 60").unwrap().wants_spells());
        assert!(!RigSpec::parse("tauren druid 60 nospells")
            .unwrap()
            .wants_spells());
        assert!(RigSpec::parse("tauren druid spells")
            .unwrap()
            .wants_spells());
    }

    #[test]
    fn the_batch_runs_in_the_order_that_works() {
        let spec = RigSpec::parse("tauren druid 60 gear:g spec:s at:ThunderBluff gm:off").unwrap();
        assert_eq!(
            build_steps(&spec, true),
            vec![
                ".revive",
                ".gm off",
                ".character level 60",
                ".learn all_myclass",
                ".character premade gear g",
                ".character premade spec s",
                ".tele ThunderBluff",
            ]
        );
    }

    #[test]
    fn a_dead_body_is_revived_even_when_nothing_was_asked_for() {
        let spec = RigSpec::parse("tauren druid").unwrap();
        assert_eq!(build_steps(&spec, true), vec![".revive"]);
        assert!(build_steps(&spec, false).is_empty());
    }

    #[test]
    fn a_point_goes_to_go_xyz_and_a_name_goes_to_tele() {
        let point = RigSpec::parse("at:1,-1277.5,124.0,131.2").unwrap();
        assert_eq!(
            build_steps(&point, false),
            vec![".go xyz -1277.5 124 131.2 1"]
        );
        let faced = RigSpec::parse("at:0,-9399.03,10.41,59.83,0.0").unwrap();
        assert_eq!(
            build_steps(&faced, false),
            vec![".go xyzo -9399.03 10.41 59.83 0 0"]
        );
        assert_eq!(parse_point("ThunderBluff"), None);
        assert_eq!(parse_point("1,2,3"), None); // three numbers is not a map + point
        assert_eq!(parse_point("1,2,3,4,5,6"), None); // and six is past even a faced point
    }

    #[test]
    fn the_template_list_is_reachable_through_the_spec() {
        let spec = RigSpec::parse("gear:?").unwrap();
        assert_eq!(build_steps(&spec, false), vec![".character premade gear"]);
    }

    #[test]
    fn the_derived_name_is_deterministic_and_fits_the_server_limit() {
        // `<Race3><Class3><word>[f]`, normalized the way vmangos normalizes a player name.
        let name = |spec: &str, slot: &str| {
            let s = RigSpec::parse(spec).unwrap();
            let (race, class, gender) = s.body.unwrap();
            format!(
                "{}{}{slot}{}",
                race_code(race).unwrap(),
                class_code(class).unwrap(),
                if gender == 1 { "f" } else { "" }
            )
        };
        assert_eq!(name("tauren druid", "one"), "taudruone");
        assert_eq!(name("nightelf warlock female", "three"), "nelwlkthreef");
        // The longest name this scheme can mint is exactly vmangos's MAX_PLAYER_NAME of 12 — a
        // 13th character would be refused by the server as an invalid name, and the rig would
        // recreate it every run.
        const MAX_PLAYER_NAME: usize = 12;
        let longest = RACE_CODES
            .iter()
            .flat_map(|(_, rc)| {
                CLASS_CODES.iter().flat_map(move |(_, cc)| {
                    [
                        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight",
                        "nine",
                    ]
                    .iter()
                    .map(move |slot| format!("{rc}{cc}{slot}f").len())
                })
            })
            .max()
            .unwrap();
        assert_eq!(longest, MAX_PLAYER_NAME);
    }

    /// A roster row with just the fields eviction reads.
    fn row(name: &str, level: u8, guid: u64) -> benilla_protocol::Character {
        benilla_protocol::Character {
            guid,
            name: name.into(),
            level,
            race: 0,
            class: 0,
            gender: 0,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
            zone: 0,
            map: 0,
            position: benilla_protocol::wire::Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            flags: 0,
            equipment: [Default::default(); 19],
            pet_display_id: 0,
            pet_level: 0,
            pet_family: 0,
        }
    }

    #[test]
    fn eviction_spends_the_body_that_is_cheapest_to_rebuild() {
        // The word is passed in, never read from the ambient checkout — these names are one
        // checkout's, and taking the word from `rig_suffix()` made this test pass there and fail
        // in every other checkout (and in one with no declaration, where it is `None`).
        let slot = "one";
        // Roster order IS create order (vmangos enumerates by `create_time`).
        let roster = [
            row("Probeone", 60, 1),  // the identity — never evictable
            row("Watcher", 60, 2),   // hand-made — never evictable
            row("Taudruone", 60, 3), // oldest rig body, but a geared 60
            row("Orcwarone", 1, 4),  // a level-1 filler: the cheapest to lose
            row("Undmagone", 1, 5),  // same level, but younger — the tie goes to the older
            row("Nelwlkonef", 40, 6),
        ];
        assert_eq!(
            evictable(&roster, Some("Tauwarone"), slot),
            Some((4, "Orcwarone".into())),
        );
        // The body we are about to create is never the one we delete to make room for it.
        assert_eq!(
            evictable(&roster, Some("Orcwarone"), slot),
            Some((5, "Undmagone".into())),
        );
        // Nothing rig-named on this slot ⇒ nothing to evict; the caller errors rather than guessing.
        assert_eq!(evictable(&roster[..2], None, slot), None);
        // And another checkout's leftovers are invisible: the same roster, read with another word,
        // evicts nothing. This is the assertion that would have caught the ambient read.
        assert_eq!(evictable(&roster, None, "three"), None);
    }

    #[test]
    fn eviction_only_ever_sees_rig_characters() {
        assert!(is_rig_name("Taudruone", "one"));
        assert!(is_rig_name("Nelwlkthreef", "three"));
        // The identity character, a human-made character, and another slot's rig: all invisible.
        assert!(!is_rig_name("Probeone", "one"));
        assert!(!is_rig_name("Watcher", "one"));
        assert!(!is_rig_name("Taudruthree", "one"));
        assert!(!is_rig_name("Taudru", "one"));
        assert!(!is_rig_name("Taudruoneextra", "one"));
    }
}
