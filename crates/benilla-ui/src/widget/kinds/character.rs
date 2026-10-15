//! A character pane's own state, the `CGCharacterModelBase` members over a `Model`'s: the subject
//! its copy is built from, the turn latch, and the copy the host builds.

/// The `CGCharacterModelBase` members a character pane adds to a `Model` (`0x505680` zeroes all of
/// them): the subject its copy is built from, the turn latch and deadline, and the copy itself.
/// The copy is a duplicate of the subject's model (`0x5059a0` through `0x707400`) that the host
/// builds and draws; the engine keeps what Lua and the paint decide about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharacterPane {
    /// `+0x3e0`/`+0x3e4`: the GUID `SetUnit` resolved and stored (`0x505b50`); 0 for none.
    pub guid: u64,
    /// `+0x3f0`: a creature display shown in place of a unit (`0x505cb0`, the stable's); 0 for
    /// none. Storing a GUID zeroes it (`0x505b70`), and storing it zeroes the GUID (`0x505cc1`).
    pub display: u32,
    /// `+0x3e8`: a `SetRotation` came since the last paint (`0x505c28`); every paint clears it
    /// (`0x505ca3`).
    pub turn_latch: bool,
    /// `+0x3ec`: the turn's deadline on the millisecond clock, `now + 100` per rotation
    /// (`0x505c37`), 0 after the paint's return to Stand (`0x505c9d`).
    pub turn_deadline_ms: u64,
    /// Builds asked of the host, one per call of the builders `0x5059a0`/`0x505a70` on a visible
    /// pane: `SetUnit` and `RefreshUnit` (`0x505b99`), `SetPetStablePaperdoll` (`0x505cd7`) and
    /// the show hook (`0x505d3c`, `0x505d56`).
    pub builds: u64,
    /// The last request the host answered or a later `SetModel`/`ClearModel` overtook: the
    /// reference builds synchronously, so a file or a clear after a `SetUnit` wins.
    pub answered: u64,
    /// The copy at `+0x318` and what its key bone plays: `None` until the host builds one, and
    /// again after the hide hook drops it (`0x505ce7`).
    pub copy: Option<PaneCopy>,
    /// Arms ever made on this pane's copies, so the host plays each once ([`PaneArm::serial`]).
    pub arms: u64,
    /// The last arm made while a copy is on its way: the reference builds inside `SetUnit` and the
    /// show hook, so an arm in the same chunk lands on the new copy, after its Stand.
    pub queued: Option<QueuedArm>,
}

/// An arm held for the copy on its way ([`CharacterPane::queued`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueuedArm {
    pub anim_id: u16,
    pub blend: bool,
    pub offset_ms: u32,
}

/// A character pane's model copy, as the host built it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneCopy {
    /// The [`CharacterPane::builds`] request it answers.
    pub build: u64,
    /// What key bone 0 plays (`[bone + 0xf8]`, read by `0x712090(−1)`).
    pub arm: PaneArm,
}

/// One `0x7121a0(−1, id, −1, 0, 1.0, blend, 1)` arm on a character copy: a whole-body arm with a
/// frequency-weighted variation roll, which the host plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneArm {
    /// The `AnimationData.dbc` id.
    pub anim_id: u16,
    /// Its place among the pane's arms; the host plays an arm whose serial it has not seen.
    pub serial: u64,
    /// Cross-faded from the pose it replaces: every arm the pane makes (`0x505a2f`, `0x505c23`,
    /// `0x505c98`, `0x5062e4`), but not `SetSequence`'s (`0x76cf60` pushes blend 0).
    pub blend: bool,
    /// Where the track starts, ms in: `SetSequenceTime`'s offset, 0 for every other arm.
    pub offset_ms: u32,
}

/// The ids the character panes arm, from the client's name table (`0x8686a8`).
pub const ANIM_STAND: u16 = 0;
pub const ANIM_SHUFFLE_LEFT: u16 = 11;
pub const ANIM_SHUFFLE_RIGHT: u16 = 12;

/// How long one `SetRotation` holds its shuffle: `[+0x3ec] = clock() + 100` ms (`0x505c32`).
pub const TURN_HOLD_MS: u64 = 100;

impl CharacterPane {
    /// The subject a build reads: a stored GUID, else a stored display; `None` builds nothing
    /// (`0x505d1d`, `0x505d51`).
    pub fn has_subject(&self) -> bool {
        self.guid != 0 || self.display != 0
    }

    /// Ask the host for a fresh copy of the subject, as a builder call on a visible pane does. A
    /// pane with no subject asks nothing: the lookup of GUID 0 finds no unit (`0x505b8d`), and the
    /// old copy stays.
    pub fn request_build(&mut self) {
        if self.has_subject() {
            self.builds += 1;
        }
    }

    /// A build is asked and not yet answered.
    pub fn pending(&self) -> bool {
        self.builds > self.answered
    }

    /// The host built the copy for request `build`: the copy goes to `+0x318` with Stand armed on
    /// it (`0x505a2f`), then whatever was armed while it was on its way. A stale answer, for a
    /// request a later one replaced or a file overtook, is dropped, and so is a second answer.
    pub fn copy_built(&mut self, build: u64) -> bool {
        if build != self.builds || build <= self.answered {
            return false;
        }
        self.answered = build;
        self.arms += 1;
        self.copy = Some(PaneCopy {
            build,
            arm: PaneArm {
                anim_id: ANIM_STAND,
                serial: self.arms,
                blend: true,
                offset_ms: 0,
            },
        });
        if let Some(q) = self.queued.take() {
            self.arm(q.anim_id, q.blend, q.offset_ms);
        }
        true
    }

    /// The host found no unit for request `build`: the lookup at the call (`0x505b8d`,
    /// `0x505d30`) found none, so nothing was built, the old copy stays, and nothing builds until
    /// the next request.
    pub fn copy_missed(&mut self, build: u64) {
        if build == self.builds && build > self.answered {
            self.answered = build;
            self.queued = None;
        }
    }

    /// `+0x318` was released or taken by a file: the copy is gone, and the request in flight with
    /// it.
    pub(super) fn overtake(&mut self) {
        self.copy = None;
        self.answered = self.builds;
        self.queued = None;
    }

    /// What key bone 0 of the copy plays (`0x712090(−1)`): a copy on its way first, its queued arm
    /// else the Stand it arrives with; `None` with no copy.
    pub fn key_bone(&self) -> Option<u16> {
        if self.pending() {
            return Some(self.queued.map_or(ANIM_STAND, |q| q.anim_id));
        }
        self.copy.map(|c| c.arm.anim_id)
    }

    /// Arm `anim_id` on the copy's key bone (`0x7121a0`), or hold it for the copy on its way;
    /// nothing with neither (`0x76cf50` reads `+0x318`).
    pub fn arm(&mut self, anim_id: u16, blend: bool, offset_ms: u32) {
        if self.pending() {
            self.queued = Some(QueuedArm {
                anim_id,
                blend,
                offset_ms,
            });
            return;
        }
        let Some(copy) = &mut self.copy else {
            return;
        };
        self.arms += 1;
        copy.arm = PaneArm {
            anim_id,
            serial: self.arms,
            blend,
            offset_ms,
        };
    }

    /// The copy's completion callback (`0x5062a0`) for the arm `serial` of copy `build`: a
    /// whole-body track arms Stand again (`0x5062aa`, `0x5062e4`). An arm a later one replaced is
    /// not a completion: only natural ends call back.
    pub fn finished(&mut self, build: u64, serial: u64) {
        if self
            .copy
            .is_some_and(|c| c.build == build && c.arm.serial == serial)
            && !self.pending()
        {
            self.arm(ANIM_STAND, true, 0);
        }
    }
}

/// The turn a character pane makes on whatever `+0x318` holds: its copy, a copy on its way, or a
/// file `SetModel` put there (`0x505bb0` and `0x505c50` read the slot, not its kind).
impl super::ModelState {
    /// What key bone 0 of `+0x318` plays (`0x712090(−1)`); `None` with the slot empty.
    fn key_bone(&self) -> Option<i32> {
        let c = self.character.as_ref()?;
        if c.pending() || c.copy.is_some() {
            return c.key_bone().map(i32::from);
        }
        self.path.as_ref()?;
        self.armed.map(|a| a.anim_id)
    }

    /// Arm key bone 0 of whatever `+0x318` holds or is about to: a copy takes the blend flag, a
    /// file its own arm ([`Self::arm`]). An id past the sequence table arms nothing (`0x712484`).
    pub fn arm_key_bone(
        &mut self,
        id: i32,
        ms: i64,
        blend: bool,
        facts: Option<&super::ModelFileFacts>,
    ) {
        self.sequence = id;
        if let Some(c) = self
            .character
            .as_mut()
            .filter(|c| c.pending() || c.copy.is_some())
        {
            if let Ok(id) = u16::try_from(id) {
                c.arm(id, blend, u32::try_from(ms).unwrap_or(0));
            }
            return;
        }
        self.arm(id, ms, facts);
    }

    /// `SetRotation`'s turn (`0x505bb0`) toward `angle` from the current facing: ShuffleRight when
    /// the facing is below it, ShuffleLeft above, else Stand (equal or NaN), unless key bone 0
    /// already plays that id; then, on every path, the latch and a deadline `now + 100`.
    pub fn rotate(&mut self, angle: f32, now_ms: u64, facts: Option<&super::ModelFileFacts>) {
        if self.character.is_none() {
            return;
        }
        let id = if self.facing < angle {
            ANIM_SHUFFLE_RIGHT
        } else if self.facing > angle {
            ANIM_SHUFFLE_LEFT
        } else {
            ANIM_STAND
        };
        if self.key_bone().is_some_and(|k| k != i32::from(id)) {
            self.arm_key_bone(i32::from(id), 0, true, facts);
        }
        if let Some(c) = &mut self.character {
            c.turn_latch = true;
            c.turn_deadline_ms = now_ms + TURN_HOLD_MS;
        }
    }

    /// The turn timer each paint runs (`0x505c50`): with no rotation since the last paint, the
    /// deadline passed (strictly, `0x505c70`) and anything but Stand on key bone 0, back to Stand
    /// and the deadline to 0. The latch clears on every path.
    pub fn turn_paint(&mut self, now_ms: u64, facts: Option<&super::ModelFileFacts>) {
        let Some(c) = &self.character else {
            return;
        };
        let due = !c.turn_latch && now_ms > c.turn_deadline_ms;
        if due && self.key_bone().is_some_and(|k| k != i32::from(ANIM_STAND)) {
            self.arm_key_bone(i32::from(ANIM_STAND), 0, true, facts);
            if let Some(c) = &mut self.character {
                c.turn_deadline_ms = 0;
            }
        }
        if let Some(c) = &mut self.character {
            c.turn_latch = false;
        }
    }

    /// The hide hook (`0x505ce0`): slot 36 with null releases whatever `+0x318` holds, a file
    /// included (`0x505ce7`); the stored subject stays for the next show.
    pub fn release_on_hide(&mut self) {
        if self.character.is_some() {
            self.clear_file();
        }
    }
}
