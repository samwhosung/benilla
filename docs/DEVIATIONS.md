# benilla — generated deviations

> **GENERATED** by `scripts/genmap.sh` from the code — do not edit by hand; rerun the script
> when a deviation is added or reworded. No timestamp — identical code yields an identical file.

benilla is the 1.12.1 client reimplemented: what the reference does is what benilla does. Where it
differs on purpose, that is a deviation, the maintainer's call alone (`docs/METHOD.md`), and it is
written down where it lives: a setting's `Deviates` row in the CVar table, or a `Deviation:` comment
at the code, the Lua or the XML that differs. This file gathers them all, regenerated at every land,
so the list is read from the code and cannot drift from it. Test code is left out, and so are the
CVars that are benilla's own knobs with no 1.12.1 counterpart (`ours` rows).

## Settings that ship off the reference default

A setting boots at the reference's default unless it is listed here (`docs/METHOD.md`).

| CVar | 1.12.1 | benilla | Why |
| --- | --- | --- | --- |
| `frillDensity` | `16` | `32` | the reference's registered 16 is stop 0 and its post-`hwDetect` 24 is on no stop at all, so every stop diverges; Medium (32) is the nearest one no sparser than a fresh install, and erring sparse is the worse failure for ground cover |
| `gxApi` | `direct3d` | `""` | descriptive, not a selector — benilla renders through wgpu, which has no D3D9 backend and no chooser; the value is the live adapter's own and is never persisted |
| `gxColorBits` | `16` | `32` | these describe, they do not steer — the pair is our swapchain's own, and every format `MsaaFormats` publishes carries it |
| `gxDepthBits` | `16` | `32` | as `gxColorBits` — the depth half of the same descriptive pair |
| `gxResolution` | `640x480` | `1600x900` | narrowed to the WINDOWED size only — fullscreen is the monitor's own and we expose no mode list, and 640x480 is not a window anyone would ship a client at |
| `realmList` | `us.logon.worldofwarcraft.com:3724` | `localhost` | that host has not resolved since 2019, so shipping it makes every first launch a DNS failure; benilla dials the machine it is running on |
| `SoundReverb` | `1` | `0` | the reference's reverb is EAX-over-hardware, and that hardware has not existed since Vista, so "1" would ship audio the real client has never actually produced on any machine a player runs today |
| `weatherDensity` | `2` | `3` | every precipitation rate in `benilla-world`'s own precipitation module was derived and graded against the reference install's own apitrace captures, and that install runs `SET weatherDensity "3"` (K = 1.0) — so 3 is the value a benilla-vs-reference side-by-side is correct at, and the registered 2 would thin every rate to 0.66 against the only client we compare with. The slider is how a player takes it back down |

## Deviations in the code

Each entry is the first sentence of a `Deviation:` comment, linking to where it lives.

- [benilla-app](#benilla-app): 229
- [benilla-assets](#benilla-assets): 6
- [benilla-blp](#benilla-blp): 1
- [benilla-formats](#benilla-formats): 17
- [benilla-lua](#benilla-lua): 11
- [benilla-protocol](#benilla-protocol): 22
- [benilla-srp](#benilla-srp): 2
- [benilla-ui](#benilla-ui): 118
- [benilla-world](#benilla-world): 49

### benilla-app

#### area

- The indoor dedup keys `(WMOID, MOGP uniqueID)`, not the client's raw per-WMO group index, because the raw index can alias across adjacent buildings. ([`src/area.rs:30`](../crates/benilla-app/src/area.rs#L30))

#### assets/ui

- `OpenBackpack` opens every equipped bag, not only the backpack, so the vendor and mailbox windows that call it (MerchantFrame.lua:35, MailFrame.lua:39) bring up every bag to sell or mail from. ([`assets/ui/ContainerFrameAdapters.xml:9`](../crates/benilla-app/assets/ui/ContainerFrameAdapters.xml#L9))
- The stock FCF\_ChatTabFadeFinished (FloatingChatFrame.lua:989-992) nils oldAlpha even under a live hover, so leaving and re-entering within CHAT\_FRAME\_FADE\_TIME (a flick to the scroll buttons, outside the hover box, does it) stops the plate fading in (l.873) for the rest of the session; this keeps oldAlpha while hover is set. ([`assets/ui/FrameXMLFixes.xml:6`](../crates/benilla-app/assets/ui/FrameXMLFixes.xml#L6))
- Re-runs UIParent\_ManageFramePositions after DurabilityFrame's own OnEvent. ([`assets/ui/FrameXMLFixes.xml:20`](../crates/benilla-app/assets/ui/FrameXMLFixes.xml#L20))
- Friendly plates the player turned on come back at every world entry. The stock UpdateNameplates (UIOptionsFrame.lua:768-780) shows them and hides them again at once (l.775-776), and FRIENDNAMEPLATES\_ON is never saved, since l.182 registers NAMEPLATES\_ON a second time. ([`assets/ui/FrameXMLFixes.xml:33`](../crates/benilla-app/assets/ui/FrameXMLFixes.xml#L33))
- The rungs are the 1.15 era client's ESC menu, because its settings screens replace 1.12's, which are much worse to use; the era's AddOns rung is dropped, since the character-select AddOns screen is the only addon UI. ([`assets/ui/GameMenuAdapters.xml:4`](../crates/benilla-app/assets/ui/GameMenuAdapters.xml#L4))
- The layout is the era Settings panel's Keybindings page (collapsible category sections), because 1.12's settings screens are much worse to use. ([`assets/ui/KeyBindingsPage.xml:5`](../crates/benilla-app/assets/ui/KeyBindingsPage.xml#L5))
- Every bind, unbind and reset saves at once, as the era panel commits, where 1.12 had Okay and Cancel; only Reset To Default and leaving the character set confirm. ([`assets/ui/KeyBindingsPage.xml:11`](../crates/benilla-app/assets/ui/KeyBindingsPage.xml#L11))
- The spacer headers BLANK, BLANK2 and BLANK3 over multibars 2-4 (Bindings.xml:883/967/1051), which 1.12 draws blank (BINDING\_HEADER\_BLANK is "  ", the other two are not defined), continue the section above, because this page's sections are named and collapsible. ([`assets/ui/KeyBindingsPage.xml:123`](../crates/benilla-app/assets/ui/KeyBindingsPage.xml#L123))
- The 1.15 era client's single settings window replaces the 1.12 four-panel family, which is much worse to use; the 1.12 frames still load, hidden, for addons. ([`assets/ui/OptionsFrame.xml:3`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L3))
- AUTO\_QUEST\_WATCH is the string "1". 1.12 assigns the number 1 (l.122) but every consumer compares against "1" (QuestLogFrame.lua:64/68); 1 ~= "1" in Lua 5.0. ([`assets/ui/OptionsFrame.xml:31`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L31))
- The bar toggles precede the lock, which 1.12's box leads with, so what is on screen reads first. ([`assets/ui/OptionsFrame.xml:498`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L498))
- A named ladder reads out its stop; 1.12's slider labels only its ends, and a bare index would mean nothing to a player. ([`assets/ui/OptionsFrame.xml:631`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L631))
- 1.12 returns early here; a mute row hides the last plate, so a neighbour's text cannot stand when its OnLeave was missed. ([`assets/ui/OptionsFrame.xml:746`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L746))
- Ships "1" where 1.12 registers "0": off, a friendly cast falls into the targeting cursor, which is not modelled. ([`assets/ui/OptionsFrame.xml:1611`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L1611))
- No partner write of cameraPitchSmoothSpeed, which has no reader: the follow here has one rate. ([`assets/ui/OptionsFrame.xml:1700`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L1700))
- Modern Classic's two-entry Display Mode, as there is no exclusive fullscreen; gxWindow "0" is the borderless window. ([`assets/ui/OptionsFrame.xml:1986`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L1986))
- Our own row (the reference draws to the backbuffer), and no Multisampling, which misses alpha-tested cutouts. ([`assets/ui/OptionsFrame.xml:2015`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L2015))
- 1.12 has no `/reload` (later clients added it; 1.12 reloads through `/console reloadui`); kept so a player can reload the interface after toggling addons. ([`assets/ui/ScriptLogFrame.xml:381`](../crates/benilla-app/assets/ui/ScriptLogFrame.xml#L381))
- A three-slice where the reference overlaps two quads, since two quads reach only 362 units and a page here is 563 tall. ([`assets/ui/ScrollTemplates.xml:188`](../crates/benilla-app/assets/ui/ScrollTemplates.xml#L188))
- The pet page's autocast widgets take the pet bar button's proportions, so the shine and the corner brackets sit on the button's edge and share its centre. ([`assets/ui/SpellBookAdapters.xml:3`](../crates/benilla-app/assets/ui/SpellBookAdapters.xml#L3))

#### aura_visual

- A non-zero rate is ignored, because the only one shipped (`8947848.0` on kit 1744, a packed grey in the rate column) is noise on the reference's integer-ms clock and has no faithful form on our f32-second one. ([`src/aura_visual.rs:212`](../crates/benilla-app/src/aura_visual.rs#L212))

#### bindings

- A press with Super (Cmd) held runs no binding, so a system chord such as Cmd+Tab or Cmd+Q never acts in game. ([`src/bindings.rs:18`](../crates/benilla-app/src/bindings.rs#L18))
- Latches die with the VM that made them, because a latch names a command of that VM's table and releasing it against a new one would run another load's up-half. ([`src/bindings.rs:447`](../crates/benilla-app/src/bindings.rs#L447))
- The Windows key table stops at `VK_F12` and drops later F-keys; they are named here so a stored `F13`-`F24` can be pressed. ([`src/bindings/chord.rs:165`](../crates/benilla-app/src/bindings/chord.rs#L165))
- On the Mac and Linux a key its platform reports no character for keeps its US name too, where the Mac reference drops it (`0x8a4aa`), and a Linux dead key not yet pressed bare stays bindable. ([`src/bindings/layout.rs:116`](../crates/benilla-app/src/bindings/layout.rs#L116))
- A character above U+00FF names the key by itself. The reference's `MapVirtualKeyA` returns an ANSI code-page byte, which `0x42da4b` drops past `0xff` and the namer's UTF-8 encoder (`0x41abb0`) otherwise reads as Latin-1, so such a key is mislabelled or dropped; named by what it types, it stays bindable and readable. ([`src/bindings/layout.rs:216`](../crates/benilla-app/src/bindings/layout.rs#L216))
- A layout that is not ASCII-capable (Hebrew, Greek, Russian, Arabic, Thai) names its keys through the ASCII-capable layout macOS pairs with it, as macOS names its own shortcuts. ([`src/bindings/layout.rs:290`](../crates/benilla-app/src/bindings/layout.rs#L290))
- A character above U+00FF (Czech `ě`, Polish `ł`, Turkish `ş`) names the key by itself. ([`src/bindings/layout.rs:294`](../crates/benilla-app/src/bindings/layout.rs#L294))
- The reference saves a full snapshot (`bindings-cache.wtf`); a diff is stored because a full snapshot would leave every command added later unbound for a saved file. ([`src/bindings/store.rs:13`](../crates/benilla-app/src/bindings/store.rs#L13))

#### blob_shadow

- The shadow is gated on `InheritedVisibility`, though the reference draws one under an undrawn body (`0x48161d`), because benilla's exterior-scene election hides units the reference never loads, and their shadows would stay on the floor below. ([`src/blob_shadow.rs:27`](../crates/benilla-app/src/blob_shadow.rs#L27))

#### camera_shake

- The shake spawns at the kit play's position. The reference passes `&node+0x48` to the shake (`0x620e0e`) before `0x62101d` fills it from the attachment, so a bone-attached first node (`0x61fdd0`, which never writes it) shakes at the world origin its constructor left (`0x61f4c1`) and is culled beyond 80 yards of it, silencing 26 of the 58 shipped shake kits; that is a read before the write, not a behaviour. ([`src/camera_shake.rs:290`](../crates/benilla-app/src/camera_shake.rs#L290))

#### chat_bubble

- Sizes ride the plates' damped diagonal basis (`plate_basis`), not the unbounded reference law, so bubble text and plate text keep one em at every window size. ([`src/chat_bubble.rs:23`](../crates/benilla-app/src/chat_bubble.rs#L23))

#### cinematic

- Bevy UI nodes computed per frame, not `CinematicFrame.xml`'s textures, so a window resized mid-cinematic stays letterboxed. `CinematicFrame` still shows and owns ESC. ([`src/cinematic.rs:118`](../crates/benilla-app/src/cinematic.rs#L118))

#### combat_text

- The reference stretches a raster of at most 32 px; each whole-pixel size is rasterized here, so the crit pop stays crisp. ([`src/combat_text/mod.rs:244`](../crates/benilla-app/src/combat_text/mod.rs#L244))

#### creature_anim

- A new puff inside the last one's clip is declined instead of replacing it (`0x6208e0`), because one-shot effects carry no reap key; either way two never overlap. ([`src/creature_anim/breath.rs:17`](../crates/benilla-app/src/creature_anim/breath.rs#L17))
- A mounted draw is refused at the setter, because our rider track never replays the reference's per-play force-stow (`0x5fdf80`); the result is the same. ([`src/creature_anim/driver.rs:715`](../crates/benilla-app/src/creature_anim/driver.rs#L715))
- The reference writes bone 0's rate blindly; we scope it to the swing's own node, because a moving attacker's gait would otherwise slow too. ([`src/creature_anim/driver.rs:1031`](../crates/benilla-app/src/creature_anim/driver.rs#L1031))
- A time-based rate, because the reference's per-frame step changes with the frame rate. ([`src/creature_anim/select.rs:134`](../crates/benilla-app/src/creature_anim/select.rs#L134))
- The class lookup that can also clear the ranged candidate (`0x5eb616`–`0x5eb640`, `0xc0def4`, meaning inferred) is skipped, because a class failing it cannot equip one. ([`src/creature_anim/sheath.rs:90`](../crates/benilla-app/src/creature_anim/sheath.rs#L90))
- A spell's state kit and body procs are reaped only once no slot holds the spell. ([`src/creature_anim/spell_visual.rs:1028`](../crates/benilla-app/src/creature_anim/spell_visual.rs#L1028))

#### cvars

- The reference's `SaveConfig` (`0x63d980`) writes only at world entry (`0x46b6f6`), at the leave-world tail (`0x490c35`-`0x490c64`) and at shutdown (`0x63daf0`), so a crash or a kill loses every change since; here a player's settings survive one. ([`src/cvars/mod.rs:964`](../crates/benilla-app/src/cvars/mod.rs#L964))
- The reference clips at full scale and has no headroom mechanism (its SFX-bus duck `0x457960` is a sidechain armed only by server-pushed voice lines); benilla limits because every SFX is mastered to full scale and overlapping kits clip. ([`src/cvars/table.rs:231`](../crates/benilla-app/src/cvars/table.rs#L231))
- Here the dial multiplies the camera's own rate, because benilla does not change the OS pointer speed, a system-wide setting. ([`src/cvars/table.rs:465`](../crates/benilla-app/src/cvars/table.rs#L465))
- "0" raises a borderless fullscreen window instead of mode-setting the display, because Wayland and macOS offer no mode-set and X11's leaves the desktop changed after a crash (`crate::video`). ([`src/cvars/table.rs:586`](../crates/benilla-app/src/cvars/table.rs#L586))
- Benilla, which has no exclusive mode, applies the same curve in the composite pass (`crate::ui_gamma::DisplayGamma`), because the skipped upload would make a slider that moves no pixel. ([`src/cvars/table.rs:695`](../crates/benilla-app/src/cvars/table.rs#L695))

#### death

- The minutes come from the server's table, not from spell 15007's `Spell.dbc` duration, because the aura the server applies is what the dialog must warn about. ([`src/death/mod.rs:403`](../crates/benilla-app/src/death/mod.rs#L403))

#### entities

- A cap on one hop's sub-segment count, which the reference lacks, so a vertex run is never sized off table data alone; shipped rows peak at 12 (30 yd at `avgSegLen` 2.78). ([`src/entities/chain_beam.rs:54`](../crates/benilla-app/src/entities/chain_beam.rs#L54))
- Not the reference's generator (`((rand & 0x7fffff) | 0x3f800000)` re-centred through `2.0`), because the look rests on the amplitude and the advection, both exact, never on the draw sequence. ([`src/entities/chain_beam.rs:110`](../crates/benilla-app/src/entities/chain_beam.rs#L110))
- The reference updates every unit every frame; here only units showing a flagged visual are tracked, and a parked rig skips its ray and re-seeds at the slope on waking, to bound the cost at one ray per flagged unit on screen. ([`src/entities/conform.rs:67`](../crates/benilla-app/src/entities/conform.rs#L67))
- The reference copies a finished model, ours stream in, so while false the pane keeps re-taking and a late weapon is baked. ([`src/entities/equipment/resolve.rs:179`](../crates/benilla-app/src/entities/equipment/resolve.rs#L179))
- It attaches at that clip's `$SHL`, where the reference attaches it at the draw's start, because it keys on the ranged slot's sheath so it arrives with the bow it feeds; the two are a few hundred ms apart in one clip and never seen apart. ([`src/entities/equipment/resolve.rs:548`](../crates/benilla-app/src/entities/equipment/resolve.rs#L548))
- `Hold` is one repeating play where the reference re-arms it each pass with variation -1, a weighted random pick, because at most one `Spells\` model authors several variations. ([`src/entities/spell_fx/lifecycle.rs:11`](../crates/benilla-app/src/entities/spell_fx/lifecycle.rs#L11))

#### game_tip

- The reference rasterizes the tip's font at most 32 px (`0x5ca096`/`0x5ca09b` in `0x5ca030`, reached through `0x44d040`) and magnifies it to the string's height; this rasterizes at the true size, as the engine does all big text (`crate::ui_text`'s engine). ([`src/game_tip.rs:191`](../crates/benilla-app/src/game_tip.rs#L191))

#### loading_screen

- The live window's aspect stands in for `gxResolution`'s, as in `portrait`'s `GxAspect` (fullscreen is the monitor's own size with no mode list, where the two agree). ([`src/loading_screen.rs:61`](../crates/benilla-app/src/loading_screen.rs#L61))
- The wheel is swallowed, where the reference still zooms the hidden camera, because nothing should act through the screen. ([`src/loading_screen/input.rs:12`](../crates/benilla-app/src/loading_screen/input.rs#L12))
- The world pick is cleared, where the reference's `0x481790` keeps picking at the frozen point, because ours is quieter and looks the same: the reference's pointer cannot move, so its pick changes nothing visible. ([`src/loading_screen/input.rs:14`](../crates/benilla-app/src/loading_screen/input.rs#L14))

#### local_state

- The reference writes `Screenshots\\` inside the install, which benilla never writes to; the folder keeps the reference's name. ([`src/local_state.rs:382`](../crates/benilla-app/src/local_state.rs#L382))

#### minimap

- Dots re-project every frame, where the reference draws coordinates about 1 s stale, because that staleness is a throttle quirk. ([`src/minimap/blips/mod.rs:25`](../crates/benilla-app/src/minimap/blips/mod.rs#L25))

#### name_persist

- The file is `benilla-config/cache/<realm>.tsv` through `crate::local_state`, not `WDB/` in the install, because the install is read-only; and it is per realm, because another server may hold a different template under the same entry. ([`src/name_persist.rs:14`](../crates/benilla-app/src/name_persist.rs#L14))

#### nameplates

- `AlphaMode::Blend` writes no depth where the reference does, because overlapping names resolving by sort order look the same. ([`src/nameplates.rs:4`](../crates/benilla-app/src/nameplates.rs#L4))
- A mounted plate keeps only this fraction of its seat's oscillation about a slow mean, because the raw seat, which the reference also rides, read more intense than the reference side by side. `1.0` is the raw seat; on-foot plates are untouched. ([`src/nameplates.rs:265`](../crates/benilla-app/src/nameplates.rs#L265))
- Larger than the reference's 32 px `UNIT_NAME_FONT` raster, which it magnifies (`0x6c749b`), because a larger source is crisper up close. ([`src/nameplates.rs:274`](../crates/benilla-app/src/nameplates.rs#L274))

#### names

- The combat-log namer keys an unstreamed creature by guid bits 32-47 (`0x6265c4`), which on the servers in use (entry at bits 24-47) names it wrongly. ([`src/names.rs:129`](../crates/benilla-app/src/names.rs#L129))

#### net

- The reference loads the world first and logs in after: the entry's first frame starts the load at the record's map and position (`0x401570`, called at `0x46c236`), and a later one, once its file reader is idle (`0x46c247`), sends the login (`0x46c272`). ([`src/net/io.rs:10`](../crates/benilla-app/src/net/io.rs#L10))
- The reference re-dials only from `RequestRealmList`, and only when the link is down (`[CLoginMgr+5]` clear, `0x5ab857` → `0x5b2200`), with the reconnect challenge (`0x5b2134`, `[Logon+0xc]`); the connect's query sends only on a link in state 6 (`0x5bc14c`) and otherwise nothing, leaving the screen on its connecting dialog for good. ([`src/net/io.rs:397`](../crates/benilla-app/src/net/io.rs#L397))
- The reference's guard at `0x537e85` sends 0 whenever the write index is 0, one ping in sixteen; we send 0 only before the first pong, because the rest is an artefact of its index arithmetic that only the server stores. ([`src/net/io.rs:1158`](../crates/benilla-app/src/net/io.rs#L1158))
- The reference places collision and culling by the raw facing, ours follow the display facing, because a capsule and a sphere barely change under rotation. ([`src/net/motion/facing.rs:9`](../crates/benilla-app/src/net/motion/facing.rs#L9))
- The reference paces a self move through the same replay chain as a remote's, which would only delay a correction to our own pose. ([`src/net/motion/remote.rs:84`](../crates/benilla-app/src/net/motion/remote.rs#L84))
- Its rate-limited hover climb (`0x636fa1`, 7 yd/s) is not reproduced, because the cache gate needs a pure function; it converges to this answer. ([`src/net/motion/spline.rs:568`](../crates/benilla-app/src/net/motion/spline.rs#L568))

#### no_install

- A boot with no install at all ends at a fatal error dialog, where the reference logs each missing archive and goes on (`0x403b00`), because nothing past this point can draw a word. ([`src/no_install.rs:7`](../crates/benilla-app/src/no_install.rs#L7))

#### player

- The reference re-projects the point's height onto the first walkable facet a zero-size box gather finds at the enemy (`0x60fe25`-`0x60fe51`); here it keeps the enemy's height. ([`src/player/approach.rs:263`](../crates/benilla-app/src/player/approach.rs#L263))
- The reference stores the world point unconverted and reads it back as deck-local (`0x60fe6c`, `0x610e95`), so on a transport it walks astray; here it rides the deck. ([`src/player/approach.rs:266`](../crates/benilla-app/src/player/approach.rs#L266))
- The reference converts back through the transport the player stands on each frame (`0x610e6b`), so stepping off mid-walk sends the point astray; here the deck is fixed at the arm. ([`src/player/approach.rs:286`](../crates/benilla-app/src/player/approach.rs#L286))
- The melee arm names the player's own guid (`0x60fe5f`), so the reference turns to face itself, a fixed heading; here the melee walk does not turn. ([`src/player/approach.rs:490`](../crates/benilla-app/src/player/approach.rs#L490))
- The shape is kept, its scale anchored to `LOOK_SENSITIVITY` on both axes at the default speeds, because the unit factor is a per-machine OS setting and this keeps the shipped feel: both axes turn alike, where the reference's yaw turns 1.5× faster than its pitch. ([`src/player/camera.rs:111`](../crates/benilla-app/src/player/camera.rs#L111))
- A delay holds the channel at `from`, because the reference's `0x512830` backs the start time up by the delay, so `s` runs negative and the even cosine jumps the value away and back, an unclamped divide rather than a behaviour. ([`src/player/camera_channel.rs:13`](../crates/benilla-app/src/player/camera_channel.rs#L13))
- A code table, not the reference's ninety `cameraTerrainTilt<Style><State>` CVars, because the matrix is a law, not a setting. ([`src/player/camera_dynamics.rs:282`](../crates/benilla-app/src/player/camera_dynamics.rs#L282))
- The reference sweeps the camera collision against the bobbed eye (`0x50ed47`); benilla adds the offset after the sweep, because it shares camera shake's accumulator there. ([`src/player/camera_dynamics.rs:447`](../crates/benilla-app/src/player/camera_dynamics.rs#L447))
- Both values clamp to the rig's range, because a hand-edited file should not land a pose play cannot reach; the reference stores them unbounded, the pitch until a mouse-look re-clamps it. ([`src/player/camera_saved.rs:64`](../crates/benilla-app/src/player/camera_saved.rs#L64))
- `ResetView` writes the CVars back to their defaults, because the CVar store is our persistence; the reference (`0x50fae0`) touches no CVar, so its reset reverts at next launch. ([`src/player/camera_view.rs:15`](../crates/benilla-app/src/player/camera_view.rs#L15))
- The starting zoom, `CAM_DIST_DEFAULT`, is 15 yd where the reference's `cameraDistance` is 5.55 (`0x84f488`), for a wider view. ([`src/player/camera_zoom.rs:9`](../crates/benilla-app/src/player/camera_zoom.rs#L9))
- No drunk field-of-view lane, because the reference's is invisible in play: its target `(179° − 90°)·fraction` (camera `+0x10c`, setter `0x511250`) eases at `cameraFoVSmoothSpeed`, 0.5°/s, and restarts on every drunk-byte change, so it creeps under 1°. ([`src/player/drunk.rs:46`](../crates/benilla-app/src/player/drunk.rs#L46))
- A settle hold is summed and sent once on release, without the push, because it stands in for the reference's blocking load (`0x694920`), not a refused substep, and lasts seconds. ([`src/player/movement_net.rs:303`](../crates/benilla-app/src/player/movement_net.rs#L303))
- The reference's blocking load (`0x694920`) waits out its reads before the frame goes on and has no such release; without one, a model that never loads would freeze our body for good. ([`src/player/state.rs:120`](../crates/benilla-app/src/player/state.rs#L120))
- Root is not merged, because a bare pose unrooting us while the server still roots us streams motion into `CHEAT_TYPE_ROOT_MOVE`; vmangos keeps a rooted mover's root bit (`MovementHandler.cpp:1064`), so the reference's merge of it is a no-op anyway. ([`src/player/state.rs:202`](../crates/benilla-app/src/player/state.rs#L202))
- Applied at arrival, because our own avatar has no motion to de-jitter; the reference routes it through the replay chain remotes use, which paces to the sender's cadence and can hold an early packet (up to 1000 ms in `net::motion`). ([`src/player/wire_in.rs:413`](../crates/benilla-app/src/player/wire_in.rs#L413))

#### portrait

- The auction house's `AuctionDressUpModel` draws the same one assembly, because the app's `TryOn`/`Dress` intents are not per-widget; the reference's two widgets each clone their own model. ([`src/portrait/dressup.rs:8`](../crates/benilla-app/src/portrait/dressup.rs#L8))
- A fixed neutral front-lit studio (`studio_light`) replaces the portrait bake's hard-coded rig (`0x525b10`: ambient 0.45 grey, white directional), because the studio is the look chosen for them. ([`src/portrait/light.rs:65`](../crates/benilla-app/src/portrait/light.rs#L65))
- The round portraits bake at 256², not 64², because the 64² still is a resolution limit, not a look; and under a fixed studio light, not the ambient state, because that neutral light is the chosen portrait look (`light::BoothLight`). ([`src/portrait/mod.rs:16`](../crates/benilla-app/src/portrait/mod.rs#L16))
- Only the four party slots hand over, capped at `Self::CAP`, instead of every portrait until `ClientDestroyGame` (`0x401ee0`), because a general cache would pay a texture per unit ever seen. ([`src/portrait/mod.rs:251`](../crates/benilla-app/src/portrait/mod.rs#L251))
- This reads the window's aspect, not `gxResolution` (here only the windowed size) under `widescreen` (not registered), because fullscreen is the monitor's own size with no mode list, where the two agree. ([`src/portrait/mod.rs:566`](../crates/benilla-app/src/portrait/mod.rs#L566))
- `boothHalfRate` is benilla's own CVar, because the reference draws its doll in the main pass and has no second view; a full-rate pass costs up to 7.6 ms a frame at 4K. ([`src/portrait/mod.rs:599`](../crates/benilla-app/src/portrait/mod.rs#L599))
- The reference widget draws only its model; a body pane clears to near-black, the chosen look where nothing sits behind the pane. ([`src/portrait/mod.rs:837`](../crates/benilla-app/src/portrait/mod.rs#L837))
- The reference probes a bake cache keyed by the record's display id (`0x525cb0`, `0xc0cea4`) before the stand-in, so a pet out of view keeps its face. benilla holds no such cache, and `PortraitBakes` is keyed by guid for the party slots alone: the stand-in shows. ([`src/portrait/mod.rs:1582`](../crates/benilla-app/src/portrait/mod.rs#L1582))

#### quest_markers

- A quest marker outranks the green, where the reference's two handlers race last-write-wins on the slot, because a fixed order is deterministic; the two meet only on a flight master with a live quest marker, invisible in practice. ([`src/quest_markers/mod.rs:124`](../crates/benilla-app/src/quest_markers/mod.rs#L124))
- A live V-plate (`VPlates`) raises the marker too, because the reference's marker sits low behind the plate: it arms anim 0 under a plate, whose name suppression nulls the `desc+0x8` handle `0x6c7950` tests. ([`src/quest_markers/mod.rs:397`](../crates/benilla-app/src/quest_markers/mod.rs#L397))
- We wait for propagation to settle the joint matrix, so the marker is always constant size. ([`src/quest_markers/mod.rs:464`](../crates/benilla-app/src/quest_markers/mod.rs#L464))

#### raid_marks

- No depth write, as for the names, because blended sorting looks the same. ([`src/raid_marks.rs:14`](../crates/benilla-app/src/raid_marks.rs#L14))

#### realmlist

- The value lives in `config.toml` with every other CVar and is edited from a login-screen control, because local state is one folder and one config file. ([`src/realmlist.rs:5`](../crates/benilla-app/src/realmlist.rs#L5))
- The local machine's server, not the reference's `us.logon.worldofwarcraft.com:3724`, because that host no longer resolves. ([`src/realmlist.rs:41`](../crates/benilla-app/src/realmlist.rs#L41))
- 64, where the reference's login boxes cap at 16 (`AccountLogin.xml`), because a hostname needs more. ([`src/realmlist.rs:45`](../crates/benilla-app/src/realmlist.rs#L45))

#### screenshot

- The file is a PNG where the reference writes a TGA (`0x4426f0`, the name `%s\WoWScrnShot_%s.tga` at `0x834c44`), because a 32-bit Targa of a modern window is about 8 MB and few tools open it. ([`src/screenshot.rs:13`](../crates/benilla-app/src/screenshot.rs#L13))
- A failed write answers `SCREENSHOT_FAILED` and logs the reason (`report_captures`), where the reference fires `SCREENSHOT_SUCCEEDED`, because its capture returns 1 whenever it had a buffer and discards the write's result (`0x442792`, `0x4427a2`); announcing a shot that was never saved is a defect. ([`src/screenshot.rs:17`](../crates/benilla-app/src/screenshot.rs#L17))
- UTC, not local time, because there is no timezone source (as for Lua `date()`, `benilla_ui::civil`). ([`src/screenshot.rs:71`](../crates/benilla-app/src/screenshot.rs#L71))

#### shaders

- Bilinear filtering runs on the decoded texel, not on the gamma byte the reference filters, because the booth bake stores linear bytes. ([`src/shaders/ui_quad.wgsl:8`](../crates/benilla-app/src/shaders/ui_quad.wgsl#L8))

#### shutdown

- The reference leaves it to the OS. Its key arm (`0x42d261`-`0x42d276`) hands Alt+F4 to `DefWindowProc`, whose `SC_CLOSE` becomes the kind-5 event (`0x42d287`, as `WM_CLOSE` posts at `0x42d292`) and runs the quit at once (`0x423a21` -> `0x423ca0` -> `0x41f5d0` -> `0x422030`), with no logout timer, at the login screen and in game alike. ([`src/shutdown.rs:50`](../crates/benilla-app/src/shutdown.rs#L50))

#### smart_rect

- The plate caller snaps the result to device pixels before `Self::claim`, so its border art blits 1:1. ([`src/smart_rect.rs:50`](../crates/benilla-app/src/smart_rect.rs#L50))

#### sound

- `damage > 0` without a parry or block stands in for `0x2`, because vmangos clears that bit only for immune and a zero-damage miss or absorb (`Unit.cpp:1366`/`1587`), so it would grunt on every parry. ([`src/sound/combat.rs:587`](../crates/benilla-app/src/sound/combat.rs#L587))
- The water column is chosen on liquid above the feet (`depth > 0`, the footstep splash's test), because the reference's test, the flag saying liquid was found at the unit's sampled position (`[node+0x90] & 0x20`, `0x670630`), has no depth and splashes a body on a lake's shore. ([`src/sound/death_thud.rs:28`](../crates/benilla-app/src/sound/death_thud.rs#L28))
- The reference marks an unresolvable kit by negating the id (`[+0xE00] = -id`), so the owner's next `$DSL` re-registers into a fresh entry every cycle; a flag keeps it in place without the churn. ([`src/sound/emitter_pool.rs:54`](../crates/benilla-app/src/sound/emitter_pool.rs#L54))
- The reference leaves those kits uncapped; sample-aligned copies of one file sum coherently (five are one sound +14 dB), so a mass buff distorts. ([`src/sound/kit.rs:234`](../crates/benilla-app/src/sound/kit.rs#L234))
- The reference never steals. Every WoW voice is an `FSOUND_Stream` at priority 256 (`fmod.dll 0x1002be47`) and FMOD's steal scan skips `>= 256` (`0x100268e9`), so its 13th sound is dropped. ([`src/sound/kit.rs:259`](../crates/benilla-app/src/sound/kit.rs#L259))
- The reference sums at full scale and clips (a close 1.0 kit reaches `FSOUND_SetVolume(255)` through `0x7a5dc0`, FMOD's MMX mixers saturate with `paddsw`, and no gain depends on the live voice count); this limits, because every WoW SFX is mastered to full scale, so overlapping kits clip into broadband distortion, as when a mass buff lands five sample-aligned copies of one 0 dBFS file. `SoundOutputLimiter 0` turns it off. ([`src/sound/limiter.rs:3`](../crates/benilla-app/src/sound/limiter.rs#L3))
- The mantissa-bit volume curve (`0x7a5d70`, `(v·255+512.5).bits >> 14 & 0xff`) and the `__ftol` truncations are dropped, because they only quantized to FMOD's integer levels and our backend takes float amplitudes. ([`src/sound/math.rs:4`](../crates/benilla-app/src/sound/math.rs#L4))
- The reference also refuses to open a new non-music stream while inactive (`0x7a52bf`); benilla starts everything and gates the output, because a refusal at kit start would make an unattended capture record silence. ([`src/sound/mod.rs:93`](../crates/benilla-app/src/sound/mod.rs#L93))
- Default off where the reference registers "1". Its reverb is FMOD 3's EAX API, which renders only with hardware 3D mixing, so off is what the reference is heard to produce (inferred: `fmod.dll`'s render side is untraced). ([`src/sound/mod.rs:101`](../crates/benilla-app/src/sound/mod.rs#L101))
- `SoundOutputLimiter`, not a 1.12 CVar, default on, so overlapping full-scale kits are limited rather than clipped. ([`src/sound/mod.rs:119`](../crates/benilla-app/src/sound/mod.rs#L119))
- With no character the reference leaves the listener where it was; benilla seats it on the camera, where the view is. ([`src/sound/mod.rs:470`](../crates/benilla-app/src/sound/mod.rs#L470))
- The reference also plays an optimistic coin at the loot-money click, so looted money sounds twice; benilla plays the watcher's coin alone, because one coin on the confirmed change reads cleanly and the round trip the early coin hides is imperceptible on a local server. ([`src/sound/money.rs:7`](../crates/benilla-app/src/sound/money.rs#L7))
- 1.12.1 sets FMOD up on driver 0 and, when that fails, runs silent for the session (`Sound::Initialize` `0x7a4330`, the driver −1 → 0 at `0x7a445f`); here another output stands in, because on Linux the default is often a stock `dmix` whose card a sound server holds. ([`src/sound/output/cpal.rs:132`](../crates/benilla-app/src/sound/output/cpal.rs#L132))
- A change of the default output device is followed (here and in `coreaudio.rs`), where 1.12.1 initialises its sound device once and never again (`Sound::Initialize` `0x7a4330` refuses a second call; the driver-enumeration functions `0x7a6320`-`0x7a6380` have no caller), because a headset plugged in mid-session should play at once. ([`src/sound/output/cpal.rs:483`](../crates/benilla-app/src/sound/output/cpal.rs#L483))

#### spell

- The reference's second Esc re-asserts the strike from the save it never clears (`0x6e4b2c`), where ours clears, because vmangos drops the melee slot on the first cancel whatever its id (`HandleCancelCastOpcode`) and both clients converge on that echo. ([`src/spell/inflight.rs:188`](../crates/benilla-app/src/spell/inflight.rs#L188))
- An out-of-range byte drops the packet, because the reference's unchecked write overruns the table (`(64, 19)` lands on the class family at `0xcecaac`). ([`src/spell/mods.rs:106`](../crates/benilla-app/src/spell/mods.rs#L106))
- The fallback is the packet's target guid, never the nearest-hostile acquire, because that would attack something the player never named. ([`src/spell/net.rs:972`](../crates/benilla-app/src/spell/net.rs#L972))
- A range failure leaves targeting armed and working. The reference's report (`0x6e6089`) also cancels the cast (`0x6e2218` → `0x6e4940`), clearing its caster, and then restores the pre-click word, so every later click fails the caster test (`0x6e5b93`) and the cursor is stranded until a cancel; that is a defect, and staying armed is what a player can use. ([`src/spell/targeting/mod.rs:480`](../crates/benilla-app/src/spell/targeting/mod.rs#L480))

#### target

- The reference's arm reads the last object's stop (`0x611336` before `0x610383` stores this one's), so its first walk has no stop. ([`src/target/click.rs:335`](../crates/benilla-app/src/target/click.rs#L335))
- It runs on the first frame with none, so an arrival that sends no stop (mid-air, rooted, under a held turn key) acts at once or on landing or release, where the reference waits for the next ground stop and can act long after, wherever the player then is. ([`src/target/click.rs:1036`](../crates/benilla-app/src/target/click.rs#L1036))
- The rank is held to 7, the top of the ring scale's eight entries (`0..=7`), where the reference keeps any value; a server rank past 7 would index outside our palette. ([`src/target/ring.rs:524`](../crates/benilla-app/src/target/ring.rs#L524))
- TAB uses Classic Era's screen-space priority, not 1.12's ±30° facing cone and snapshot cursor (`0x493f60`), because the cone skips a close mob in front of the camera, which sits behind the character. ([`src/target/scan.rs:3`](../crates/benilla-app/src/target/scan.rs#L3))
- Our own body is never a candidate. The reference's friendly scan picks the player at some facings (`CanAssist(P, P)` is true, `0x6061e0`; the cone test `0x47f220`), but our scoring always puts the player dead centre at distance 0, so CTRL-TAB would pick us on every press. ([`src/target/scan.rs:194`](../crates/benilla-app/src/target/scan.rs#L194))
- No scene-attach gate (the reference's `0x6704c0`), because the draw election hides every body outside the frustum, which would empty the off-screen tier. ([`src/target/scan.rs:378`](../crates/benilla-app/src/target/scan.rs#L378))
- Sampled each frame, not stamped in `SetSelection`, so every selection writer is covered; a selected unit that turns hostile is remembered, where the reference's is not. ([`src/target/scan.rs:779`](../crates/benilla-app/src/target/scan.rs#L779))

#### textinput

- macOS takes the Cocoa text-field chords for Cmd and Option, because editing follows each OS's own text fields. ([`src/textinput/keymap.rs:5`](../crates/benilla-app/src/textinput/keymap.rs#L5))

#### ui_action

- `0x5e` with no word falls back to the spell's own `RequiresSpellFocus`, which the reference never reads here, because it is the id vmangos sends (`Spell.cpp:4426`). `0x5d` has no such stand-in: vmangos takes its area from its own `spell_area` table (`Spell.cpp:4429`), which `Spell.dbc` does not carry. ([`src/ui_action/cast_fail.rs:228`](../crates/benilla-app/src/ui_action/cast_fail.rs#L228))
- A reason past the table prints its hex code, so an unknown reason is visible. ([`src/ui_action/cast_fail.rs:338`](../crates/benilla-app/src/ui_action/cast_fail.rs#L338))
- A declined arm's template loses its tokens ("Missing reagent: %s" reads "Missing reagent") where the reference shows the literal `%s`, because a raw token reads as broken. ([`src/ui_action/cast_fail.rs:496`](../crates/benilla-app/src/ui_action/cast_fail.rs#L496))
- An absent or empty result shows nothing, where the reference shows an empty line (no test of the string between `0x4967d7` and `0x496842`), because a blank line is worse than none; only a chain missing a stock key reaches it. ([`src/ui_action/errors.rs:211`](../crates/benilla-app/src/ui_action/errors.rs#L211))
- This runs on every book or bar change, where the reference re-points slots in memory only on `SMSG_SUPERCEDED_SPELL` (`0x4e5e60`), because vmangos never rewrites stored slots on a rank-up (`Player::AddSpell`), drops superseded ranks from the book (`Player.cpp:3463`) and sends no supersede while loading (`Player.cpp:3704`), so a stored slot can name a spell the character no longer knows. ([`src/ui_action/ranks.rs:3`](../crates/benilla-app/src/ui_action/ranks.rs#L3))

#### ui_aura

- On a catalog miss the reference keeps the record's stale value (`0x4e4533`, `0x4e4550` skip the write); this answers from the joined expiry, because a stale value is no mechanism to reproduce. ([`src/ui_aura.rs:350`](../crates/benilla-app/src/ui_aura.rs#L350))
- On other units' rows an id the catalog lacks stays visible, because every wire id is a real spell and failing closed would blank every aura on a catalog failure; the reference skips one there (`[0xc0d78c]`), though its player cache inserts one, as here. ([`src/ui_aura.rs:378`](../crates/benilla-app/src/ui_aura.rs#L378))
- The reference never clears its expiry array (`0xbc5f68`); this does, because vmangos sends no durations at login to overwrite a kept stamp (`SMSG_UPDATE_AURA_DURATION` is an empty placeholder in `Player::SendInitialPacketsBeforeAddToMap`, `Player.cpp:19248`). ([`src/ui_aura.rs:640`](../crates/benilla-app/src/ui_aura.rs#L640))

#### ui_binder

- The reference never clears its latched guid (`0xc4d7c0`), so a second `ConfirmBinder()` in range re-sends the activate; this drops the guid on `SMSG_PLAYERBOUND`, because an answered question is not a question and the reference's dialog is gone by then. ([`src/ui_binder.rs:17`](../crates/benilla-app/src/ui_binder.rs#L17))

#### ui_chat

- Not run on benilla's same-character reconnect, which the reference lacks (it returns to the login screen), so the window keeps its scrollback there. ([`src/ui_chat.rs:216`](../crates/benilla-app/src/ui_chat.rs#L216))
- A missing global prints nothing, where the reference prints an empty line (`0x703c02`), because benilla's other `DisplayError` routes do the same. ([`src/ui_chat/broadcast.rs:148`](../crates/benilla-app/src/ui_chat/broadcast.rs#L148))
- With a blank city word the reference composes `"Trade - "` (its capital arm checks the row pointer, not the string, `0x49a2d0`-`0x49a2f6`); we decline, because a wrong channel name is joined silently and for good. ([`src/ui_chat/channels.rs:150`](../crates/benilla-app/src/ui_chat/channels.rs#L150))
- A probe's bare `/assist` assists the live selection, where the stock handler (`ChatFrame.lua:740`) resolves a selected player's name through `AssistByName`, because a name resolve can land on a different player. ([`src/ui_chat/input/mod.rs:649`](../crates/benilla-app/src/ui_chat/input/mod.rs#L649))
- The files live in `benilla-config/Logs/`, not the install's `Logs`, because the install is read-only. ([`src/ui_chat/logging.rs:4`](../crates/benilla-app/src/ui_chat/logging.rs#L4))
- No re-run when a remote player streams in, because the reference's re-run only repeats the join, which vmangos swallows, or the leave, whose "Not on channel" notice nothing prints. ([`src/ui_chat/recruitment.rs:7`](../crates/benilla-app/src/ui_chat/recruitment.rs#L7))
- `set_chat_window_looks` drops an out-of-range window, where the reference's `WINDOW` bound is off by one (`0x498d1c`, `ja` where the array needs `jae`), because that write would land past the array. ([`src/ui_chat/settings.rs:31`](../crates/benilla-app/src/ui_chat/settings.rs#L31))
- The debounce also writes Lua's changes (`dirty`) after the quiet time, where the reference writes only at teardown, so a crash loses at most a second of changes. ([`src/ui_chat/settings.rs:562`](../crates/benilla-app/src/ui_chat/settings.rs#L562))

#### ui_dressup

- The base is the player's melee pair, held drawn, because the booth shares the select screen's assembly; the reference clones the live model with its attachments, drawn or sheathed (`0x5059a0`, `0x70ea00`). ([`src/ui_dressup.rs:29`](../crates/benilla-app/src/ui_dressup.rs#L29))

#### ui_duel

- The challenger's name comes from the `NameCache`, so a name still being queried delays the popup rather than dropping it; the reference reads it off the streamed player and fires nothing without one (`0x4d4a72`). ([`src/ui_duel.rs:7`](../crates/benilla-app/src/ui_duel.rs#L7))
- Zero arms nothing, where the reference prints 0 and re-arms forever on the `dec`'s wraparound, a bug vmangos never reaches (it sends 3000). ([`src/ui_duel.rs:95`](../crates/benilla-app/src/ui_duel.rs#L95))

#### ui_follow

- Every stop fires `END`, because the reference's three silent stops are a bug: two writers reset the follow mode cell `0xc4d888` directly (`0x5fb646`, `0x60396f`), leaving a stale guid at `0xc4d980`, and the canceller's early-out (`0x60fb70`) fires nothing. ([`src/ui_follow.rs:5`](../crates/benilla-app/src/ui_follow.rs#L5))

#### ui_gamma

- The reference skips that upload when windowed (`byte[dev+0x20b]` is `gxWindow`), and every benilla mode is windowed, so the same curve applies in the compositor, where the slider can move pixels: `UiGammaNode` raises the gamma byte the RAMDAC would have read to `gamma` before its decode, the continuous form of the 256-entry ramp. ([`src/ui_gamma.rs:56`](../crates/benilla-app/src/ui_gamma.rs#L56))
- The reference never clamps (`SetGamma` `0x4891f0` writes `-4.000000` for 5), but its ramp is a fullscreen OS call and ours is the image, where an extreme exponent blacks out the panel that would undo it. ([`src/ui_gamma.rs:66`](../crates/benilla-app/src/ui_gamma.rs#L66))

#### ui_gossip

- 14 (a NULL, pushed as `nil`) and 15 up (a read past the table's end) give `"gossip"`, because the reference's behaviour there is a missing bounds check. ([`src/ui_gossip/mod.rs:200`](../crates/benilla-app/src/ui_gossip/mod.rs#L200))

#### ui_guild

- A script's 16-letter name, which the reference's bindings pass (1 to 16 units, `0x4d20d0`, `0x4d2210`), is dropped rather than sent or truncated, because vmangos would kick the session. ([`src/ui_guild/feed.rs:412`](../crates/benilla-app/src/ui_guild/feed.rs#L412))
- Bounded at the ten slots, because the reference's read (`0x4c93e7`) runs past its array. ([`src/ui_guild/mod.rs:44`](../crates/benilla-app/src/ui_guild/mod.rs#L44))
- Clamped to ten, because the reference's unbounded loop (`0x4d0bb0`) overruns its ten-slot array on a hostile `rankCount >= 12`. ([`src/ui_guild/mod.rs:279`](../crates/benilla-app/src/ui_guild/mod.rs#L279))
- The identity cache goes too, where the reference keeps it in `guildcache.wdb`, because a renamed guild would otherwise show its old name. ([`src/ui_guild/mod.rs:584`](../crates/benilla-app/src/ui_guild/mod.rs#L584))
- An exact tie is `Equal` where the reference answers ±1, and a NaN has a place (`total_cmp`), because Rust's sort requires a total order. ([`src/ui_guild/sort.rs:72`](../crates/benilla-app/src/ui_guild/sort.rs#L72))
- The reference ties a miss with a hit (`0x4d0e27`, `0x4d0ea1`), no total order. ([`src/ui_guild/sort.rs:89`](../crates/benilla-app/src/ui_guild/sort.rs#L89))

#### ui_honor

- We clear it whenever the inspected token's guid moves, because our window re-resolves its token each frame and both tabs read it, so the reference's rule would let the two tabs show different players. `ClearInspectPlayer` drops the token, which clears the slot here too. ([`src/ui_honor.rs:175`](../crates/benilla-app/src/ui_honor.rs#L175))

#### ui_instance

- Walking out of a party dungeon ourselves also counts as holding a bind (`InstanceState::saw_own_dungeon`), because vmangos reports only permanent binds (`Player.cpp:16175`) and makes only raid binds permanent (`Map.cpp:3528`), so the server alone would never offer the reset after a 5-man. ([`src/ui_instance.rs:6`](../crates/benilla-app/src/ui_instance.rs#L6))
- The app's monotonic clock, not `time(0)`, because it cannot run backwards; they differ only across a suspend. ([`src/ui_instance.rs:38`](../crates/benilla-app/src/ui_instance.rs#L38))
- Reason 3 and up prints nothing, where the reference prints an uninitialized 2 KB stack buffer, because that is a bug and vmangos means 3 and up as silent (`MapPersistentStateMgr.h:275`). ([`src/ui_instance.rs:300`](../crates/benilla-app/src/ui_instance.rs#L300))
- Flag 2 and up prints nothing, where the reference prints an uninitialized 2 KB stack buffer, because that is a bug, not a mechanism. ([`src/ui_instance.rs:332`](../crates/benilla-app/src/ui_instance.rs#L332))

#### ui_layout

- A debounced autosave (`save_layout`) also writes a quiet second after a drag, because the reference has no crash-path write and a crash would lose the session's windows. ([`src/ui_layout.rs:13`](../crates/benilla-app/src/ui_layout.rs#L13))
- A record's fields start empty. The reference's parser clears X, Y, W and the level at each `Frame:` (`ebx`, `edi`, `esi`, `[ebp-0x4]` at `0x49045b`–`0x490469`) but not H (`[ebp-0xc]`), so a record with no `H:` takes the one before's; benilla writes `H:` whenever it writes `W:`, so only a hand-edited file reaches the slip, which would size a window from another window's row. ([`src/ui_layout.rs:86`](../crates/benilla-app/src/ui_layout.rs#L86))

#### ui_loot

- It is the display row, where the reference's is the item index plus one (`0x4c2885`), so an addon's `GetLootSlotInfo(arg1)` reads the row the dialog is for. ([`src/ui_loot/mod.rs:98`](../crates/benilla-app/src/ui_loot/mod.rs#L98))
- Our close paths clear it guid-matched, where `CloseInteraction` clears it unconditionally (`0x48f2c9`), so closing an old window keeps a newer `CMSG_LOOT`'s latch. ([`src/ui_loot/mod.rs:376`](../crates/benilla-app/src/ui_loot/mod.rs#L376))
- A negative answer releases the row, where the reference's callback `0x4c2ac0` returns on a failed query and the window never opens, because an entry the server cannot describe would otherwise keep the loot shut for good. ([`src/ui_loot/mod.rs:548`](../crates/benilla-app/src/ui_loot/mod.rs#L548))
- A push is dropped after `RECEIVE_MAX_TRIES` frames, where the reference waits on the item-cache callback with no timeout, because an entry the server never describes would otherwise stay queued. ([`src/ui_loot/mod.rs:660`](../crates/benilla-app/src/ui_loot/mod.rs#L660))

#### ui_loot_roll

- A negative answer, or none within `TEMPLATE_HOLD_MAX_MS`, opens the frame unresolved, because a frame that never opens cannot even be passed on; the reference's callback fires on success only. ([`src/ui_loot_roll.rs:8`](../crates/benilla-app/src/ui_loot_roll.rs#L8))
- The open count is the number of rolls announced and not closed. The reference keeps a counter that its server-close paths decrement for any roll whose closed byte is clear (`0x61b6f8`, `0x61bc17`), a deferred roll that never counted up included, so it under-counts and lets a fifth frame open; here a deferred roll's close changes nothing. ([`src/ui_loot_roll.rs:18`](../crates/benilla-app/src/ui_loot_roll.rs#L18))

#### ui_macro

- Each line is trimmed, because the reference sends an indented `" /say hi"` as plain chat, never as the command meant. ([`src/ui_macro/run.rs:42`](../crates/benilla-app/src/ui_macro/run.rs#L42))
- The `/cast` argument is trimmed, because the reference binds `/cast  Fireball` (two spaces) to a name that never resolves. ([`src/ui_macro/run.rs:52`](../crates/benilla-app/src/ui_macro/run.rs#L52))

#### ui_mail

- A wrong field count is no invoice, because the reference would read the missing fields uninitialised. ([`src/ui_mail/invoice.rs:76`](../crates/benilla-app/src/ui_mail/invoice.rs#L76))
- An open mailbox also re-lists at once, past `CheckInbox`'s 60 s throttle, because a mail announced while the player stands there should show; the reference waits for the close. ([`src/ui_mail/net.rs:232`](../crates/benilla-app/src/ui_mail/net.rs#L232))

#### ui_merchant

- The row is matched by item entry, where the reference matches its vendor slot word (`+0x00`), because vmangos writes the entry in that field (`Player.cpp:11718`). ([`src/ui_merchant/net.rs:81`](../crates/benilla-app/src/ui_merchant/net.rs#L81))

#### ui_mirror

- A type with no bar is dropped, because no bar appears either way: the reference fires it as `"UNKNOWN"` (`0x5e7ae0`) and `MirrorTimer_Show` errors on its nil colour before `Show()`. vmangos's `NUM_CLIENT_TIMERS` keeps its fourth timer off the wire. ([`src/ui_mirror.rs:71`](../crates/benilla-app/src/ui_mirror.rs#L71))

#### ui_models

- The reference's paint (`0x76d240`) reads the eye before it rebuilds the root, so a facing change draws one frame late; not reproduced, because it is an ordering quirk. ([`src/ui_models/mod.rs:1421`](../crates/benilla-app/src/ui_models/mod.rs#L1421))

#### ui_pet

- It counts characters where the reference counts bytes, because a byte cut can split a UTF-8 character. ([`src/ui_pet/menu.rs:108`](../crates/benilla-app/src/ui_pet/menu.rs#L108))

#### ui_pet_stats

- Fired when the bucket or its two tooltip figures change, not on every change of the happiness field as in the reference, because no stock consumer reads the raw value. ([`src/ui_pet_stats.rs:180`](../crates/benilla-app/src/ui_pet_stats.rs#L180))

#### ui_quest_share

- A member's name not yet cached holds its line for a bounded retry, where the reference reads its name cache (`0xc0e228` via `0x55f080`) with a null callback and drops the message on a miss, because benilla's cache may not hold a party member yet. ([`src/ui_quest_share.rs:10`](../crates/benilla-app/src/ui_quest_share.rs#L10))

#### ui_saved

- The write is atomic (`crate::local_state::write_atomic_bytes`), because the reference's `.bak` rotation (`MoveFileW` without `MOVEFILE_REPLACE_EXISTING`, its result discarded) protects only the first write, and nothing reads the `.bak`. ([`src/ui_saved.rs:9`](../crates/benilla-app/src/ui_saved.rs#L9))
- A header saying what the file is, because the folder is in plain view; the reference's file opens with a bare blank line. ([`src/ui_saved.rs:136`](../crates/benilla-app/src/ui_saved.rs#L136))

#### ui_script

- The reference reads the install's `Interface\AddOns\`; benilla reads an install and does not live in one, so its addons live in its own state folder, and only there. ([`src/ui_script/addons.rs:264`](../crates/benilla-app/src/ui_script/addons.rs#L264))
- That file is renamed to `<Name>.lua.bak`, where the reference deletes it, so an addon update that drops its toc line costs the player nothing they cannot get back. ([`src/ui_script/addons.rs:657`](../crates/benilla-app/src/ui_script/addons.rs#L657))
- The reference writes none (its file opens with a blank line); ours names the file, because players open this folder. ([`src/ui_script/addons.rs:715`](../crates/benilla-app/src/ui_script/addons.rs#L715))
- The reference then re-reads the aspect (`0x492f13`), and on a change of 0.001 or more calls `0x491380`, which sets the reload trigger (`[0xb4b3f4]`) when the script-context check `0x494a50(0xa)` allows it; here a resize re-lays out the live UI, the chat bubbles' window-derived `BubbleGeometry` with it, because the window resizes freely and a reload on each resize would tear down every addon's state mid-play. ([`src/ui_script/extract/mod.rs:523`](../crates/benilla-app/src/ui_script/extract/mod.rs#L523))
- The V-plate border is resampled sharp to the quad's physical size rather than GPU-magnified, so its 1 px bevel stays crisp; keyed by that size, so only a resize re-rasterises it. ([`src/ui_script/extract/mod.rs:1403`](../crates/benilla-app/src/ui_script/extract/mod.rs#L1403))
- No signature or digest check (`0x46a90b`, `0x46a9d3`, `0x401560(9)`), so the chain's toc always loads and `FRAMES_LOADED` follows, because the check verifies Blizzard's signature with a key carried in the client binary, which benilla does not ship, and a modified interface file is loaded rather than refused. ([`src/ui_script/glue.rs:14`](../crates/benilla-app/src/ui_script/glue.rs#L14))
- The reference's size callback only re-scales `GlueParent` and re-lays it out (`0x46ac51`, `0x46ac56`), as its root keeps `gxResolution`'s aspect until a rebuild (`0x41ad10` at `0x46a828`) and its window keeps that shape while dragged (`0x5989d7`); here the root is the window's, which takes any shape, so the box follows it on every resize, where a rebuild would reset the screen (the create screen's character and name, the typed password, the selection). ([`src/ui_script/glue.rs:296`](../crates/benilla-app/src/ui_script/glue.rs#L296))
- The reference's handler completes whichever operation is current; here only the character list's does, in state 3, and a reply of another cause (a roster nothing of the screen asked for) or of another link (before a realm change) completes nothing. ([`src/ui_script/glue_mgr.rs:1362`](../crates/benilla-app/src/ui_script/glue_mgr.rs#L1362))
- The character is the platform's Unicode text, so a player can type any language on any platform. ([`src/ui_script/input.rs:466`](../crates/benilla-app/src/ui_script/input.rs#L466))
- A capture run (`scenario_active`) keeps the CRT's default seed, 1, so its frames reproduce. ([`src/ui_script/lifecycle.rs:144`](../crates/benilla-app/src/ui_script/lifecycle.rs#L144))
- The files are under `benilla-config/Logs/`, since the install is read-only. ([`src/ui_script/load_log.rs:5`](../crates/benilla-app/src/ui_script/load_log.rs#L5))
- `FULLSCREEN_QUADS_RESEAT` re-sizes the full-screen quads, which the stock files size only in their OnLoads, because a resize here keeps the loaded UI and the blackout would stop short of the new screen. ([`src/ui_script/manifest.rs:64`](../crates/benilla-app/src/ui_script/manifest.rs#L64))
- No `FrameXML.toc` signature check, where `UI_Init` loads on a failed one and then quits (`0x401560(0xa)` at `0x48ff90`), because the check verifies Blizzard's signature with a key carried in the client binary, which benilla does not ship, and a modified interface file is loaded rather than refused. ([`src/ui_script/manifest.rs:161`](../crates/benilla-app/src/ui_script/manifest.rs#L161))
- A flat 0.9, because 1.0 reads oversized. With `useUiScale` off, its default (`0x8430c0`), the reference sets `max(768/H, 0.9)` above 768 px tall and 1.0 at or below (`0x492f70`, on a mode set and from `0x4908ad`), so the two agree from ~853 px up. benilla never reads `useUiScale`: this CVar always applies to `UIParent` alone, as with it on. ([`src/ui_script/mod.rs:238`](../crates/benilla-app/src/ui_script/mod.rs#L238))
- The body is read with 0x4000-byte reads filled to the end of the body, so what a read returns does not depend on how the network cut the packets; and the reading stops once the buffer is full, since no later byte can change it. ([`src/ui_script/server_alert.rs:23`](../crates/benilla-app/src/ui_script/server_alert.rs#L23))
- The connection is direct. The reference opens its session with access type 0 (`InternetOpenA`, `0x7a6d97`), the system's proxy settings, which benilla does not read. ([`src/ui_script/server_alert.rs:26`](../crates/benilla-app/src/ui_script/server_alert.rs#L26))
- On a worldport it fires with our descriptor present, so `UnitExists("player")` answers true where the reference's (`0x515970`) misses, because matching it breaks `UnitExists` for no stock reader. ([`src/ui_script/sweep.rs:150`](../crates/benilla-app/src/ui_script/sweep.rs#L150))

#### ui_summon

- The latch is dropped at session end and an Accept with nothing latched sends nothing, so no stale or zero guid reaches the server. ([`src/ui_summon.rs:16`](../crates/benilla-app/src/ui_summon.rs#L16))

#### ui_talent_wipe

- Vmangos answers a reset with nothing to reset by asking again with a zero guid (`SkillHandler.cpp:52`), which the 1.12 client takes as its Accept and answers, looping; this drops a zero-guid question, because it means there is nothing to ask. ([`src/ui_talent_wipe.rs:14`](../crates/benilla-app/src/ui_talent_wipe.rs#L14))
- The reference never clears its latch (`0xc4d7a0`); this does once the answer is sent, because the reference's dialog is gone by then and a second Accept would reset twice. ([`src/ui_talent_wipe.rs:147`](../crates/benilla-app/src/ui_talent_wipe.rs#L147))

#### ui_text

- Text the reference magnifies from a smaller raster is rasterized at its true size here: a `SetTextHeight` size (the bit cleared at `0x771600`), an em past 32 px, and 3-D unit names (`UNIT_NAME_FONT`, em 32, string flag `0x80`), because the 32 px cap is a raster-memory budget that blurs big text on a modern display (`super::FONTSTRING_EM_CAP`). ([`src/ui_text/engine/mod.rs:8`](../crates/benilla-app/src/ui_text/engine/mod.rs#L8))
- The reference flushes every glyph cache on a window resize (`0x5c2b50` → `0x5ca6f0`); this keeps the old cells, because nameplate meshes bake UVs and a flush would rebuild them every resize frame. ([`src/ui_text/engine/mod.rs:13`](../crates/benilla-app/src/ui_text/engine/mod.rs#L13))
- The reference's measure kernels (`0x5c6940`, `0x5c6b70`, `0x5c6c50`, `0x5c7300`, `0x5c7470`) rasterize each character, but the step law reads only the advance, so skipping the bitmap changes no width. ([`src/ui_text/engine/mod.rs:362`](../crates/benilla-app/src/ui_text/engine/mod.rs#L362))
- The reference evicts the least recently used glyph and repacks its hole (`0x5cad2b`, over `[CGxFont+0x64..0x6c]`); this drops every cell, because a shelf allocator cannot reclaim an interior cell and the sheet is far larger than the reference's 256×256 pages. ([`src/ui_text/engine/mod.rs:652`](../crates/benilla-app/src/ui_text/engine/mod.rs#L652))
- Every UI text block sits 1 px below the reference's row, because at that row UI text reads high. ([`src/ui_text/layout/mod.rs:115`](../crates/benilla-app/src/ui_text/layout/mod.rs#L115))
- The cap is 32 logical units, not 32 device px, because the reference's raster-memory ceiling draws all big text at the same 32 device px on a modern display; at the 768-tall design height the two agree. ([`src/ui_text/mod.rs:54`](../crates/benilla-app/src/ui_text/mod.rs#L54))
- `r × round(dpi)` passes, so the ring keeps its logical weight on a high-DPI raster. ([`src/ui_text/outline.rs:60`](../crates/benilla-app/src/ui_text/outline.rs#L60))
- `r × round(dpi)` passes as in `outlined_cell`, the first `round(dpi)` of them the opaque ring. ([`src/ui_text/outline.rs:167`](../crates/benilla-app/src/ui_text/outline.rs#L167))
- One sheet, where the reference keeps up to 8 pages per font (`[CGxFont+0x18c]`), because a nameplate bakes its glyphs into one mesh with one material, which a page would split. ([`src/ui_text/pack.rs:11`](../crates/benilla-app/src/ui_text/pack.rs#L11))

#### ui_trade

- An out-of-range slot is dropped, where the reference indexes its 7-slot mirror unchecked, because an out-of-bounds write is not a behaviour to reproduce. ([`src/ui_trade/mod.rs:312`](../crates/benilla-app/src/ui_trade/mod.rs#L312))

#### video

- `gxWindow "0"` is borderless fullscreen, not 1.12's exclusive mode-set at `gxResolution`, because Wayland has no client mode-setting (winit ignores `Fullscreen::Exclusive` there), X11's XRandR changes the desktop's mode and a crash leaves it changed, and macOS has no exclusive mode. ([`src/video.rs:4`](../crates/benilla-app/src/video.rs#L4))
- Sizes are logical px, because `gxResolution` is a logical inner size here. ([`src/video.rs:453`](../crates/benilla-app/src/video.rs#L453))

#### vplates

- The basis is damped past `PLATE_DIAG_KNEE`, because the reference's unbounded growth (`0x7705b0`) draws plates too large and soft at high resolutions. ([`src/vplates.rs:11`](../crates/benilla-app/src/vplates.rs#L11))
- The bar fill brightens by `LIT_BOOST` in place of the reference's ADD `Nameplate-Glow` rim, which reads as hard edge lines under benilla's linear blending. ([`src/vplates.rs:15`](../crates/benilla-app/src/vplates.rs#L15))
- `LEVEL_H` is 0.0086, one em under the reference's 0.009, because the level number read too large. ([`src/vplates.rs:107`](../crates/benilla-app/src/vplates.rs#L107))
- The reference dims non-target plates to `0x7F`; raised because that fades them too far to read. ([`src/vplates.rs:121`](../crates/benilla-app/src/vplates.rs#L121))
- The reference draws at fractional device coordinates; the snap keeps the magnified border crisp. ([`src/vplates.rs:170`](../crates/benilla-app/src/vplates.rs#L170))
- The reference's plates grow with the diagonal without limit (`0x7705b0`), which reads too big and soft; past this diagonal, where the frame is the border art's native 128x32 px, the basis grows at `PLATE_GROWTH_DAMP` of that rate. ([`src/vplates.rs:176`](../crates/benilla-app/src/vplates.rs#L176))
- A constant one-logical-pixel shadow, not the reference's 0.001 gx, which draws 2 px thick and too heavy at larger windows. ([`src/vplates.rs:626`](../crates/benilla-app/src/vplates.rs#L626))
- The reference magnifies the 128 × 32 `Interface\Tooltips\Nameplate-Border` art with `GL_LINEAR` (`0x770200`, filter `[0x878cf0]` = 1), smearing its 1 px bevel once the plate is larger than native; we resample the same pixels to the plate's physical size with sharp bilinear and draw it 1:1, because the chosen look is the same art with a crisp bevel. `super::drive_vplates` regenerates it when the physical size changes. ([`src/vplates/border.rs:3`](../crates/benilla-app/src/vplates/border.rs#L3))
### benilla-assets

#### anim_rng

- A capture run (`deterministic`) keeps the default seed, so its frames reproduce. ([`src/anim_rng.rs:39`](../crates/benilla-assets/src/anim_rng.rs#L39))

#### model

- A card authored facing away from the viewer is lit on the face it presents, because the reference shades it inverted and camera-dependent. ([`src/model.rs:132`](../crates/benilla-assets/src/model.rs#L132))

#### shaders

- This is the `specular`/`pixelShaders` = 1 leg the reference install runs; both CVars (`0x6886a0`/`0x688712`) default to 0, where water has no program, no specular, a plain ADD combine and no blend. ([`src/shaders/liquid.wgsl:18`](../crates/benilla-assets/src/shaders/liquid.wgsl#L18))
- `min(I, 1)`. The reference bakes I into the SH unclamped (`0x71c4e0`), so a lit entity commits ×2.5 there and ×1.0 here; lifting the cap takes sun-facing surfaces past 1.0. `entity_shade::LIT_T` aims the lit ramp at 1.0 because of it: lift the cap and set `LIT_T` back to 0.0 together. ([`src/shaders/wow_model.wgsl:668`](../crates/benilla-assets/src/shaders/wow_model.wgsl#L668))

#### wmo

- The down-ray tries them when the walking and portal legs miss and no terrain lies below the eye, because the reference reads outside there and blanks the building around a camera sealed in an all-DETAIL pocket. ([`src/wmo.rs:61`](../crates/benilla-assets/src/wmo.rs#L61))

#### world_assets

- The reference uploads the shipped frames as they are; this evens them because their means differ (`ocean_h` alpha 55.985..57.628 over its 30 frames, more down the DXT3 mips), and at distance, where mipping averages a level to one value, the far sheet brightens and dims once per 1.25 s loop. ([`src/world_assets/images.rs:71`](../crates/benilla-assets/src/world_assets/images.rs#L71))
### benilla-blp

#### (crate root)

- Past the file's end `pad_to` zero-fills, because the reference reads stale buffer bytes there. ([`src/lib.rs:236`](../crates/benilla-blp/src/lib.rs#L236))
### benilla-formats

#### chain

- An archive that fails to open is an error, where the reference logs `"Failed to open archive"` and goes on, because a skipped corrupt archive surfaces only as missing files far downstream. ([`src/chain.rs:67`](../crates/benilla-formats/src/chain.rs#L67))

#### characters

- Blends in 8-bit RGBA, not the client's RGB565 with 2-bit coverage, because that format only saves texture memory and loses precision. ([`src/characters/sections.rs:280`](../crates/benilla-formats/src/characters/sections.rs#L280))

#### creature_families

- A zero range (row 28 only) is `None`, the lookup-miss fallback, because the reference divides by zero there into a NaN scale. ([`src/creature_families.rs:66`](../crates/benilla-formats/src/creature_families.rs#L66))

#### emote_text

- A surplus `%s` stays verbatim, where the reference would print stack garbage; no shipped row has one. ([`src/emote_text.rs:113`](../crates/benilla-formats/src/emote_text.rs#L113))

#### garble

- A language with no words keeps the line, where the reference drops every word, because a missing pool is a broken install and blank chat would hide it. ([`src/garble.rs:150`](../crates/benilla-formats/src/garble.rs#L150))
- It stops at a codepoint boundary where the reference's byte copy can split one, because a `&str` must stay UTF-8. ([`src/garble.rs:257`](../crates/benilla-formats/src/garble.rs#L257))

#### loose

- A folder is listed the first time a key reaches it, where the reference lists every folder at the first open (`0x646ea0`), so a file added to a folder no key has reached is found before the index empties; the whole walk takes minutes and gigabytes where the install folder holds or links a large tree, as a Wine prefix's `z:` drive links `/`. ([`src/loose.rs:31`](../crates/benilla-formats/src/loose.rs#L31))

#### models

- The fraction is clamped to `[0, 1]`; the reference extrapolates, which for a window whose brackets miss the band can push a fix16 factor negative and cull the batch on a quirk. ([`src/models/key_anim.rs:130`](../crates/benilla-formats/src/models/key_anim.rs#L130))
- Owners past the ceiling (Maraudon's waterfalls, the glue screens) keep the interleave with their emitters, because a bias past a 200-yard owner would jump every transparent surface in the zone. ([`src/models/m2_batches.rs:682`](../crates/benilla-formats/src/models/m2_batches.rs#L682))
- The fx lane redraws them as projected decals (`ground_fx`) so they drape the terrain. ([`src/models/types.rs:383`](../crates/benilla-formats/src/models/types.rs#L383))
- Consumers light such a card off the side it presents, because the reference's shading of it is inverted and follows the camera. ([`src/models/types.rs:454`](../crates/benilla-formats/src/models/types.rs#L454))
- The UV comes from the model-space position, not each surface's own `(i, j)`, because the two agree on cell-aligned surfaces and the position never seams where they are not. ([`src/models/wmo/group.rs:19`](../crates/benilla-formats/src/models/wmo/group.rs#L19))

#### particles

- NaN floors to 0 where the reference reads the raw mantissa, because we do not reproduce its divide by zero on a `mid` of 0 or 1; no shipped emitter authors one. ([`src/particles.rs:73`](../crates/benilla-formats/src/particles.rs#L73))
- `mid` is clamped to \[1e-3, 1\] because a 0 divides by zero in the reference (`0x7b9cf0`, walking NaN into the sampler); no shipped emitter authors one. ([`src/particles.rs:114`](../crates/benilla-formats/src/particles.rs#L114))

#### server_messages

- A row with two `%s` fills only the first, because the reference's one-argument `snprintf` fills the second with stack garbage; no shipped row has two. ([`src/server_messages.rs:46`](../crates/benilla-formats/src/server_messages.rs#L46))

#### spells

- Every token keys the `$l` plural. The reference's `$a`, `$d`, `$t`, `$e`, `$c`, `$p`, `$f`, `$F` and `$z` arms never write `[0xbe0b84]`, so its `$l` keys on the number before them, and Blizzard's "$s1 … every $t1 $lsecond:seconds;" reads "every 1 seconds" in 1.12.1. ([`src/spells/tokens.rs:551`](../crates/benilla-formats/src/spells/tokens.rs#L551))

#### tga

- Grayscale types 3 and 11 decode where the reference refuses them, because no shipped or addon file is one and removing working code would buy nothing. ([`src/tga.rs:6`](../crates/benilla-formats/src/tga.rs#L6))
### benilla-lua

#### api

- The C build grows a string while `realloc` succeeds, and a failure is the catchable `not enough memory`; Rust's allocator aborts the process instead, and Linux grants far more than it can fill, so past this bound the libraries raise C's message. ([`src/api.rs:32`](../crates/benilla-lua/src/api.rs#L32))

#### heap

- C Lua has no such bound; past it the string is `not enough memory`, as C's would be when memory runs out. ([`src/heap.rs:659`](../crates/benilla-lua/src/heap.rs#L659))
- The client's `luaM_realloc` (`0x6fc980`) has no failure path to ask from; here a refused string asks for the collection that may free room. ([`src/heap.rs:697`](../crates/benilla-lua/src/heap.rs#L697))

#### lib_math

- The client's one process-wide stream is shared with its engine draws; here each VM holds its own, seeded from the wall clock when it is built, so a fork or a clone draws its own sequence. ([`src/lib_math.rs:191`](../crates/benilla-lua/src/lib_math.rs#L191))

#### lib_string

- The decoder reads a NUL as no bytes (`0x7fbeda`), so the client's loop never passes one and the call never returns; here the result ends there. ([`src/lib_string.rs:96`](../crates/benilla-lua/src/lib_string.rs#L96))
- The client writes 5.0 bytecode (`luaU_dump`, `0x6fb600`); benilla writes 5.1's header and the function's source name. ([`src/lib_string.rs:232`](../crates/benilla-lua/src/lib_string.rs#L232))
- 5.1's `match` recurses without a bound and overflows the C stack (near 32000 levels on 8 MB); this raises 5.2's `pattern too complex` instead. ([`src/lib_string.rs:522`](../crates/benilla-lua/src/lib_string.rs#L522))

#### lib_table

- The client's loops have no bound (`0x7fb728`, `0x7fb7e0`), but a remembered size is whatever an addon wrote, and the instruction-budget hook never sees a native's loop, so one over two billion slots would hang the client. ([`src/lib_table.rs:34`](../crates/benilla-lua/src/lib_table.rs#L34))

#### pools

- That sequence is not wholly the client's. A table's node vector is sized as 5.1 sizes it, neither the constant table each function's parse makes (`open_func`'s `luaH_new`, `0x6fcad0`) nor `lua_load`'s token buffer is counted, and a prototype's vectors are grown one after another where the client's parser grows them together; each moves a pool's chunk boundary, and so its free space, by what it changes. ([`src/pools.rs:8`](../crates/benilla-lua/src/pools.rs#L8))

#### vm

- These are benilla's own needs, past what the client's checks (`Vm::checkstack`) reserve: a frame's registers above `c_maxstack` (`Proto::maxstack`), and the host's and natives' pushes, which ask for no margin of `EXTRA_STACK` and one more. ([`src/vm.rs:952`](../crates/benilla-lua/src/vm.rs#L952))
- benilla's scratch registers above a frame's `c_maxstack` (`Proto::maxstack`) stay on the stack; the client has none, and halves. ([`src/vm.rs:2252`](../crates/benilla-lua/src/vm.rs#L2252))
### benilla-protocol

#### auth

- The patch state downloads no patch and starts no updater, because benilla runs no code a server sends. ([`src/auth.rs:52`](../crates/benilla-protocol/src/auth.rs#L52))
- A stored per-OS constant, not the reference's HMAC over its own executables (`0x5b1170`), because every mangos-family realmd issues this one salt. ([`src/auth.rs:208`](../crates/benilla-protocol/src/auth.rs#L208))
- The survey id is read and dropped, and no survey runs, where the reference keeps it on the link (`0x5bad53`) for the `Survey.dll` it downloads and calls (`GetSurveyId 0x5ab620`, passed at `0x47404f`). benilla runs no code a server sends (`AuthReject::reply` gives the rule). ([`src/auth.rs:272`](../crates/benilla-protocol/src/auth.rs#L272))

#### events

- `benilla::spell_mods` refuses an out-of-range pair, because the reference would overrun it. ([`src/events.rs:669`](../crates/benilla-protocol/src/events.rs#L669))

#### examples

- Benilla always sends the published constant instead, since an install without those binaries could never answer for itself. ([`examples/version_check_probe.rs:9`](../crates/benilla-protocol/examples/version_check_probe.rs#L9))

#### messages

- A fixed list, not read off the install, because no third-party addon sets `## Secure:`, so the two differ only where the `.pub` files were altered. ([`src/messages/addons.rs:93`](../crates/benilla-protocol/src/messages/addons.rs#L93))
- Every row is kept, because the reference takes the count unclamped into its 80 row blocks and a larger one overruns them. ([`src/messages/battlefield.rs:98`](../crates/benilla-protocol/src/messages/battlefield.rs#L98))
- A larger count keeps the first 40, where the reference's handler writes past the store. ([`src/messages/battlefield.rs:188`](../crates/benilla-protocol/src/messages/battlefield.rs#L188))
- Text without a NUL stops at `length`, because the reference reads on out of bounds. ([`src/messages/broadcast.rs:29`](../crates/benilla-protocol/src/messages/broadcast.rs#L29))
- The 1.12 client appends a zlib chat transcript to a category 2 ticket (`0x5ef936`); we omit it because vmangos never reads it and logs the leftover bytes. ([`src/messages/gm_ticket.rs:48`](../crates/benilla-protocol/src/messages/gm_ticket.rs#L48))
- Zone and position are kept, not set to the viewer's zone and the object's position, because nothing in benilla reads them. ([`src/messages/group.rs:255`](../crates/benilla-protocol/src/messages/group.rs#L255))
- String reads are unbounded. The reference `0x4191b0` caps each field, counting the NUL (roster name `0x30`, notes `0x80`, MOTD `0x200`, info `0x7d0`, guild name `0x60`, rank name `0x40`), and on overflow empties it and no-ops every later read in the packet. ([`src/messages/guild.rs:4`](../crates/benilla-protocol/src/messages/guild.rs#L4))
- A reason of 3 or more is silent, where the reference prints an uninitialized stack buffer, because 3 is the server's `INSTANCERESET_FAIL_SILENTLY`. ([`src/messages/instance.rs:70`](../crates/benilla-protocol/src/messages/instance.rs#L70))
- 2 or more prints nothing, because the reference prints an uninitialized buffer. ([`src/messages/opcode.rs:1003`](../crates/benilla-protocol/src/messages/opcode.rs#L1003))
- The consumer refuses 64 and up, because there the reference's store overruns its table. ([`src/messages/packet.rs:606`](../crates/benilla-protocol/src/messages/packet.rs#L606))
- A body too short for the position gives `None` where the client redisplays a stale global, because showing a stale queue position as current is a client bug. ([`src/messages/parse.rs:295`](../crates/benilla-protocol/src/messages/parse.rs#L295))
- A type outside 1-7 reads as nothing rather than the reference's default arm, which under-pushes its returns. ([`src/messages/pet.rs:75`](../crates/benilla-protocol/src/messages/pet.rs#L75))
- Both are read, told apart exactly by the tail length (`12n` vs `1 + 14n`), since the 1.12 client mis-parses vmangos's. ([`src/messages/pet.rs:195`](../crates/benilla-protocol/src/messages/pet.rs#L195))

#### world

- The reference waits forever; a bound tells a server that died mid-queue from a long wait. ([`src/world/session.rs:17`](../crates/benilla-protocol/src/world/session.rs#L17))
- Zeros until one does, where the reference's words are never initialised (the session is allocated without zero-fill, `ui/billing` in benilla-re). ([`src/world/session.rs:58`](../crates/benilla-protocol/src/world/session.rs#L58))
- Checked per queue packet where the reference checks each tick, so a cancel can lag one server update. ([`src/world/session.rs:94`](../crates/benilla-protocol/src/world/session.rs#L94))
- Benilla runs no Warden module and answers nothing, where the reference hands `SMSG_WARDEN_DATA` to the module its connect loads (`0x6ca2f0`, `0x6ca5c0`) and sends that module's answer as `CMSG_WARDEN_DATA` (`0x6ca6a0`). ([`src/world/session.rs:218`](../crates/benilla-protocol/src/world/session.rs#L218))
### benilla-srp

#### (crate root)

- `SrpClientChallenge::new` redraws `a` until every value it sends serializes the same both ways (`is_width_stable`), and `benilla-protocol`'s `logon` redials when the server's `B` lands ambiguous, because a client that picks either serialization fails about 1 handshake in 45 against vmangos. ([`src/lib.rs:13`](../crates/benilla-srp/src/lib.rs#L13))
- `a` is redrawn until `A`, `K` and `M1` have no high-order zero byte and `S` no low-order one, because the client and vmangos hash those values differently. ([`src/lib.rs:270`](../crates/benilla-srp/src/lib.rs#L270))
### benilla-ui

#### civil

- UTC where the reference's callers use local time, because this crate has no timezone source. ([`src/civil.rs:2`](../crates/benilla-ui/src/civil.rs#L2))

#### loader

- A `<Script file=>` value with a separator joins the including document's directory, like a bare name. ([`src/loader/mod.rs:429`](../crates/benilla-ui/src/loader/mod.rs#L429))
- Padded to the block's line, so a raise reports a line a reader can find in the file; the reference counts from the body's first line. ([`src/loader/mod.rs:455`](../crates/benilla-ui/src/loader/mod.rs#L455))
- A link that would close a loop is refused and reported; the reference's XML legs (`0x783ce6`, `0x783d22`) check nothing, and the next push down such a loop would never end. ([`src/loader/mod.rs:715`](../crates/benilla-ui/src/loader/mod.rs#L715))
- `<HighlightColor>` and `<DisabledColor>` colour their own fonts; 1.12.1 sends both to the normal colour setter (`0x778aa4`, `0x778ada` call `0x778e20`), so neither could reach its state. ([`src/loader/widgets.rs:251`](../crates/benilla-ui/src/loader/widgets.rs#L251))

#### markup

- The length keeps every bit, where the client masks it to 16 (`0x77baf3`), because a wrapped length would put the cursor inside an `|H` span of 64 KiB or more. ([`src/markup.rs:261`](../crates/benilla-ui/src/markup.rs#L261))
- A step that runs out of buffer while skipping lands on the far end, because the client spins forever there (its skip loop re-enters at `0x77bb60`, past the zero check at `0x77bb56`), so `SetText("|cffffffff")` and one RIGHT hang 1.12. ([`src/markup.rs:384`](../crates/benilla-ui/src/markup.rs#L384))

#### order

- A child that left the scroll draw is drawn once, by the flat pass, where the client's callback, which reads only the child's visibility, draws it a second time, clipped, at the ScrollFrame's place. ([`src/order.rs:530`](../crates/benilla-ui/src/order.rs#L530))

#### script

- An out-of-range slot answers nil, because the reference indexes `0xbc6980` with no bounds check and reads the memory beside its 120 slots. ([`src/script/action.rs:256`](../crates/benilla-ui/src/script/action.rs#L256))
- The files the handler writes beside the install (`<Name>.pub` with the key, `<Name>.url`; `0x51dce0`, `0x51dde0`) are not written, as the install is read-only; the key is never read either, as there is no signature to verify with it. ([`src/script/addon.rs:204`](../crates/benilla-ui/src/script/addon.rs#L204))
- The reference fails silently; we report it, as a player needs to know, and hold the file so the shutdown write keeps it. ([`src/script/addon.rs:782`](../crates/benilla-ui/src/script/addon.rs#L782))
- The reference drops a send that resolves to PARTY while party slot 0's guid (`0xbc6f48`) is zero (`0x49fa9f`-`0x49faab`); benilla sends it, because `PartyState` is empty until the first `SMSG_GROUP_LIST` and the gate would swallow a real broadcast. vmangos drops a group-less party line anyway (`ChatHandler.cpp:476`). ([`src/script/addon_message.rs:7`](../crates/benilla-ui/src/script/addon_message.rs#L7))
- Backed off to a char boundary, because a `String` must stay UTF-8; the cut is the same for ASCII. ([`src/script/addon_message.rs:146`](../crates/benilla-ui/src/script/addon_message.rs#L146))
- `GetPlayerBuffApplications` answers 1 for a cleared record where the reference reads its stale `+0x9`, the previous occupant's count, because no guarded caller reaches it. ([`src/script/aura/player_buff.rs:70`](../crates/benilla-ui/src/script/aura/player_buff.rs#L70))
- The reference's usage line names `GetPlayerBuffTimeLeft` (`0x84bcc0`, shared with `0x4e4930`); ours names this verb, because nothing can depend on the wrong name. ([`src/script/aura/player_buff.rs:148`](../crates/benilla-ui/src/script/aura/player_buff.rs#L148))
- `None` answers `nil` where the reference's `-1` faction leg pushes nothing (Lua reads below the tuple), because no playable race reaches that leg. ([`src/script/battlefield_queue.rs:38`](../crates/benilla-ui/src/script/battlefield_queue.rs#L38))
- The reference (`0x4abdc0`) admits 9 and reads one dword past its block; we answer 0 because that read is out of bounds. ([`src/script/battlefield_score.rs:292`](../crates/benilla-ui/src/script/battlefield_score.rs#L292))
- `pow(10, n)` is the platform's, where the reference's (`0x73f940`, `fyl2x`) rounds its extended-precision steps at 53 bits, so a power or a fraction term can differ in the last place. ([`src/script/binding_abi.rs:186`](../crates/benilla-ui/src/script/binding_abi.rs#L186))
- The disabled-checked leg (`0x785380`) tests and reloads the checked slot (`[+0x4e0]`, `0x785388`/`0x7853a5`), a copy of the checked leg's (`0x785270`), so in 1.12.1 a check box with a checked texture repaints that one and never gets its disabled-checked texture from a path. benilla tests and reloads the slot the method names. ([`src/script/button.rs:585`](../crates/benilla-ui/src/script/button.rs#L585))
- The slot is read before the handler, where the reference reads it after (`[esi+0x4dc]`), because reading first keeps the borrow simple and no stock handler re-slots its row. ([`src/script/button.rs:1171`](../crates/benilla-ui/src/script/button.rs#L1171))
- The reference composes the Shortcut with `GetRealZoneText` alone (`0x49efe8`), so in a capital `/leave Trade` sends `Trade - Stormwind City` and leaves nothing; ours uses the join's city-aware name, because the reference's form cannot leave the channel it joined. ([`src/script/channel.rs:480`](../crates/benilla-ui/src/script/channel.rs#L480))
- The frame's bottom-left origin is snapped onto the device grid so the border art blits 1:1; the reference ceils the anchor point itself onto it (`0x7666a0`, `0x766750`). ([`src/script/chat_bubble.rs:476`](../crates/benilla-ui/src/script/chat_bubble.rs#L476))
- No truncation at 255 characters (`0x49f604`), because vmangos refuses a longer line itself (`Chat.cpp:2177`): it is dropped where the reference sends its first 255. ([`src/script/chat_send.rs:23`](../crates/benilla-ui/src/script/chat_send.rs#L23))
- A value outside 0..1 clamps, where the reference keeps the low byte (alpha 2.0 stores 254, -1.0 stores 1), because no FrameXML or addon caller passes one. ([`src/script/chat_window.rs:17`](../crates/benilla-ui/src/script/chat_window.rs#L17))
- An id outside the seven built windows raises, where the reference serves ten records and silently ignores any other id (`0x4a0be1`), because benilla keeps only the seven and every caller loops `1, NUM_CHAT_WINDOWS`. ([`src/script/chat_window.rs:220`](../crates/benilla-ui/src/script/chat_window.rs#L220))
- One past `i32::MAX` clamps where the reference keeps the low 32 bits, for the colours' reason. ([`src/script/chat_window.rs:430`](../crates/benilla-ui/src/script/chat_window.rs#L430))
- The reference reads the clock live at each call (`0x42c010`); ours is the session clock the tick advances by the paced frame step, one value per frame, so `GetTime()` deltas and the `elapsed` handed to OnUpdate agree, as FrameXML (CastingBarFrame and kin) assumes. ([`src/script/clock.rs:53`](../crates/benilla-ui/src/script/clock.rs#L53))
- With no wheel it scales by the strip's, because the reference reads the missing wheel unguarded. ([`src/script/colorselect.rs:364`](../crates/benilla-ui/src/script/colorselect.rs#L364))
- The reference's `set` registers an unknown name as a new category-5 record (`0x63d569`-`0x63d582`); benilla refuses it, because nothing would read the row and `config.toml` keeps an unknown key anyway. ([`src/script/console_exec.rs:120`](../crates/benilla-ui/src/script/console_exec.rs#L120))
- A row that no longer resolves is refused, as `0x4c7300` does, because this path dereferences it unchecked (`0x4f9f35`), a latent null deref. ([`src/script/container.rs:666`](../crates/benilla-ui/src/script/container.rs#L666))
- A last `spell_id` key breaks the remaining ties (Charge's six ranks all have `spellLevel` 0), because the reference leaves them to its qsort over an array benilla does not reproduce; ascending id is ascending rank in every 1.12 pet-ability family. ([`src/script/craft.rs:104`](../crates/benilla-ui/src/script/craft.rs#L104))
- On `Terrain` the reference keeps a spell, action, macro or pet action on the left button; we clear them, because otherwise a spell has no left-click way off the cursor. ([`src/script/cursor.rs:523`](../crates/benilla-ui/src/script/cursor.rs#L523))
- Every lock lifts here, bag contents included. The reference's walk of a bag's contents indexes the bag by its own player-array index (`0x495528`, `0x495535`), at least 19, past a bag's slots in every case but the biggest bags, so it normally unlocks none of them; a lock it strands lasts until the item's seat or stack moves, which this does not copy. ([`src/script/cursor.rs:694`](../crates/benilla-ui/src/script/cursor.rs#L694))
- Returns nothing. The handler ends `mov eax,1` with nothing pushed (`0x4895bc`), so Lua 5.0 hands back the stack top, the last argument or the function itself: an accident of the handler, not an API, and copying a stack-top leak would copy a bug. ([`src/script/cursor.rs:908`](../crates/benilla-ui/src/script/cursor.rs#L908))
- An auto-store into a bank item or keyring slot sends nothing, because no container id names it and no stock caller passes one; the reference sends it and the server refuses. ([`src/script/cursor/bag_verbs.rs:25`](../crates/benilla-ui/src/script/cursor/bag_verbs.rs#L25))
- Slot 11 is refused, because the reference's gate (`cmp esi,0xa; jbe`) admits it and neither `0x4bce00` nor `0x4bc9a0` bounds-checks again, so it writes one dword past the array, and the autocast toggle (`0x4bcbb0`) reads that word (`[0xb71438+0x28]`) with no bounds check. ([`src/script/cursor/pet.rs:18`](../crates/benilla-ui/src/script/cursor/pet.rs#L18))
- The list is the window sizes this client can present, not the device's display modes, because benilla makes no exclusive mode-set. `"WxH"` is a contract: `OptionsFrame.lua:281-284` splits on the `x` and `CT_Viewport.lua:107` matches `"(%d+)x(%d+)"`, both failing silently. ([`src/script/cvars.rs:56`](../crates/benilla-ui/src/script/cvars.rs#L56))
- The tie-break is ours, because area alone with the reference's adjacent-only dedupe can leave a duplicate `"WxH"` when two modes share an area (1280x960, 1600x768). ([`src/script/cvars.rs:68`](../crates/benilla-ui/src/script/cvars.rs#L68))
- An id out of range is ignored, because the reference reads one entry past the last (`0x48c67c`) and a clamp would apply a format the player did not pick. ([`src/script/cvars.rs:558`](../crates/benilla-ui/src/script/cvars.rs#L558))
- The reference then runs the console command `gxRestart` (`0x842978` through `0x63ce00`); none is queued here, because `gxMultisample` applies at next launch anyway: the camera reads its sample count once, at spawn. ([`src/script/cvars.rs:572`](../crates/benilla-ui/src/script/cvars.rs#L572))
- The reference (`0x48dad0` into `0x639a20`) writes hardware-recommended values from its matched `VideoHardware.dbc` row and the `CGxFormat` preset it names (`[0xc4e6ac]`): twelve `gx*` CVars, a synchronous `RestartGx`, then sixteen more. benilla has no such matching, so its registered defaults are its recommended configuration. ([`src/script/cvars.rs:631`](../crates/benilla-ui/src/script/cvars.rs#L631))
- The index is of the live window size, not of the `gxResolution` CVar the reference parses (`0x48bf20`), because without an exclusive mode-set `gxResolution` is the windowed size, not the live one in borderless fullscreen, which `CT_Viewport` scales by. ([`src/script/cvars.rs:683`](../crates/benilla-ui/src/script/cvars.rs#L683))
- 0, a negative or an index past the count clamps to the nearer end, because the reference reads one element past the last there (`0x48c00d`), uninitialised memory. ([`src/script/cvars.rs:698`](../crates/benilla-ui/src/script/cvars.rs#L698))
- Always empty, because a refresh rate is only selectable through an exclusive mode-set and benilla has none. ([`src/script/cvars.rs:738`](../crates/benilla-ui/src/script/cvars.rs#L738))
- 1.12 has no such CVar; it exists because our Graphics page needs somewhere to keep the stop, and takes the API's name. ([`src/script/cvars.rs:866`](../crates/benilla-ui/src/script/cvars.rs#L866))
- The getter reads `CVAR_WORLD_DETAIL`, where the reference's reads only `smallCull` (`0x488d77`), because benilla's stop follows the knob that draws: a console `frillDensity` write moves the ground clutter and `WorldDetail` with it, and no renderer reads `smallCull` (its callback's `[0x868620]` has no reader). ([`src/script/cvars.rs:894`](../crates/benilla-ui/src/script/cvars.rs#L894))
- The promotion's IME legs (`0x77d417`–`0x77d431`) and the IME writer's `or 6` (`0x77acde`, in `0x77ac60`) are not modelled: benilla has no IME composition. ([`src/script/editbox/flush.rs:28`](../crates/benilla-ui/src/script/editbox/flush.rs#L28))
- The nameplate glow is shown but never drawn, because its additive rim reads as hard edge lines in our linear-blending pipeline; the lit plate brightens its bar instead. ([`src/script/extract.rs:230`](../crates/benilla-ui/src/script/extract.rs#L230))
- A host of 0x80 bytes or more is refused. The reference copies the host into a 0x80-byte buffer (`0x5abc7c`-`0x5abc9c`) and writes its terminator one byte past the end, and the cut host could end in a whitelisted suffix the real one does not; this takes the safe side. ([`src/script/glue_core.rs:364`](../crates/benilla-ui/src/script/glue_core.rs#L364))
- Benilla keeps that decode and then encodes it again, every byte outside the URL characters `URL_SAFE` as `%XY` (a decoded `%` too), because the opener on some platforms is a shell script (`xdg-open`) or takes the raw command tail (`rundll32`) and the string comes off the wire: no space, quote, backtick, `$`, `;`, `<`, `>`, `|`, `^`, `\`, brace or control byte reaches it. ([`src/script/glue_core.rs:408`](../crates/benilla-ui/src/script/glue_core.rs#L408))
- Past `NAME_TRIES` names the answer is nil, where the reference keeps trying, forever when the generator has learned nothing. ([`src/script/glue_create.rs:968`](../crates/benilla-ui/src/script/glue_create.rs#L968))
- With no Region rows the reference reads a NULL category (`0x46e5b8`, `0x46f26a`) and faults; benilla lists every realm under one category, so a client whose `Wow.ini` names another Region still reaches a realm. ([`src/script/glue_realm.rs:195`](../crates/benilla-ui/src/script/glue_realm.rs#L195))
- Equal nonzero counts tie here, where the reference answers +1 both ways (its name tie-break at `0x46e841` is clobbered by `0x46e846`), because a comparator that is not a total order has no defined sort. ([`src/script/glue_realm.rs:260`](../crates/benilla-ui/src/script/glue_realm.rs#L260))
- A span count that no longer matches the live set always rebuilds, so a state this model does not flag (a ring wiped by `SetMaxLines`, a hidden frame) leaves no stale button. ([`src/script/hyperlink.rs:161`](../crates/benilla-ui/src/script/hyperlink.rs#L161))
- `IsInInstance()` then answers `nil, "none"`, because the reference returns two results without pushing either (`0x48a7a9`), a stack bug. ([`src/script/instance.rs:21`](../crates/benilla-ui/src/script/instance.rs#L21))
- Bounds-checked where the reference indexes unguarded, so none reads past it. ([`src/script/item_stats.rs:379`](../crates/benilla-ui/src/script/item_stats.rs#L379))
- Nil for a display row with no icon, where the reference answers the unloadable `Interface\Icons\`, because `SetTexture` shows no icon for either. ([`src/script/item_stats.rs:482`](../crates/benilla-ui/src/script/item_stats.rs#L482))
- An addon file whose first binding has no header opens a `HEADER_<addon>` row, where the reference files those rows under the previous file's last header, because under a foreign header the player could not find them. ([`src/script/keybind.rs:292`](../crates/benilla-ui/src/script/keybind.rs#L292))
- The handle's value is the address of a `CGxTex` (`SMemAlloc(0x54)`, `0x594a96`), which benilla cannot reproduce; each texture ranks instead by a creation stamp, one global sequence number taken when its cache key is first created and taken anew once its last region has let go of it (`super::texel::Texels`). ([`src/script/layer_batch.rs:9`](../crates/benilla-ui/src/script/layer_batch.rs#L9))
- The client's Lua has no such bound, as nothing sets a hook (`lua_sethook`, `0x6fba40`, has no caller and no pointer to it), so a load that never ends hangs it. ([`src/script/mod.rs:790`](../crates/benilla-ui/src/script/mod.rs#L790))
- A path that does not resolve is stored, where the reference raises `Invalid model file: %s` (`0x878b44`), because the file loads asynchronously here and the call cannot know yet. ([`src/script/modelframe.rs:341`](../crates/benilla-ui/src/script/modelframe.rs#L341))
- Benilla plays no pre-rendered movie (#968), so `StartMovie` answers nil, the reference's "could not start" (`0x78d861`), which the stock `MovieFrame_PlayMovie` answers by hiding the frame and so by returning to the login screen. ([`src/script/movieframe.rs:4`](../crates/benilla-ui/src/script/movieframe.rs#L4))
- The reference's free list is FIFO; either keeps every index stable, all an addon can observe. ([`src/script/nameplate.rs:429`](../crates/benilla-ui/src/script/nameplate.rs#L429))
- The reference adds this rim over the bar; we brighten the bar (`LIT_BOOST`), because the rim reads as hard edge lines in our linear-blending pipeline. ([`src/script/nameplate.rs:879`](../crates/benilla-ui/src/script/nameplate.rs#L879))
- A wrapper that decodes to no live widget takes `default_parent`, because the reference, with no `IsA` check, reads a wild pointer. ([`src/script/object/anchor_args.rs:160`](../crates/benilla-ui/src/script/object/anchor_args.rs#L160))
- Real 1.12 kinds nothing fires yet raise too (`OnMessageScrollChanged`, `OnInputLanguageChanged`), because an accepted handler that never runs fails silently. ([`src/script/object/events_regions.rs:236`](../crates/benilla-ui/src/script/object/events_regions.rs#L236))
- An `OnSizeChanged` that resolve produces fires at the next `UiScript::resolve`, because a Lua handler must not run in the middle of a level compaction. ([`src/script/object/toplevel.rs:74`](../crates/benilla-ui/src/script/object/toplevel.rs#L74))
- A third `%s` gets an empty string, because the reference pushes two varargs (`0x6093f2 add esp,0x14`) and would read uninitialised stack. ([`src/script/pvp/mod.rs:261`](../crates/benilla-ui/src/script/pvp/mod.rs#L261))
- A token resolving to guid 0 answers nil too. The reference jumps from the guid test (`0x5172f6`) to `return 1` (`0x517343`) with nothing pushed, so the one value is its top argument, the token it read (`0x5172e6`); that is a stack quirk, not an answer. ([`src/script/pvp/mod.rs:508`](../crates/benilla-ui/src/script/pvp/mod.rs#L508))
- With `UNIT_PVP_NAME` or the title missing from `_G` (no install) this answers the plain name, where the reference would format an empty string, because the decoration is install data. ([`src/script/pvp/mod.rs:512`](../crates/benilla-ui/src/script/pvp/mod.rs#L512))
- An in-flight name reads `...`, where the reference's reads `" "` (`0x82ee00`), because a blank looks like a nameless objective. ([`src/script/quest_log.rs:56`](../crates/benilla-ui/src/script/quest_log.rs#L56))
- One signal per missed record, where the reference runs every callback of every miss as its record lands (creatures in `0x556e20` at `0x556f91`, items in `0x55bc70` via `0x55bdb0` from `0x555140`; `0x4e07ea`, `0x4e083a`), 2^k - 1 updates for a walking handler over k first-seen records. ([`src/script/quest_log.rs:962`](../crates/benilla-ui/src/script/quest_log.rs#L962))
- A local clock step mid-session (an NTP correction) jumps the reference's countdown but not ours, because the step changes no time the quest has left. ([`src/script/quest_log.rs:1019`](../crates/benilla-ui/src/script/quest_log.rs#L1019))
- Four set ids count 4, where the loop's exit `jmp 0x4e0a4c` (`0x4e0a42`) skips the store at `0x4e0a49` and answers 0, a client bug. ([`src/script/quest_log.rs:1311`](../crates/benilla-ui/src/script/quest_log.rs#L1311))
- Nesting stops at 32 where the reference recurses uncapped (its indent saturates at `0x80`), so a hostile structure cannot exhaust the stack. ([`src/script/saved.rs:21`](../crates/benilla-ui/src/script/saved.rs#L21))
- The reference rewrites it from the live defaults (`0x51f865`–`0x51f970`), which would cost the player the whole file for one typo in a hand edit. ([`src/script/saved.rs:27`](../crates/benilla-ui/src/script/saved.rs#L27))
- The writer's escapes are undone (`unescape`), where the reference sets the bytes between the quotes raw, so a string holding a quote or a backslash would grow every session. ([`src/script/saved.rs:197`](../crates/benilla-ui/src/script/saved.rs#L197))
- The block step is not pixel-snapped (`0x766750`), because layout here is in logical units; the two differ by under a pixel, and not at all at the default spacing of 0. ([`src/script/simplehtml/mod.rs:13`](../crates/benilla-ui/src/script/simplehtml/mod.rs#L13))
- A block of width 0, in a frame sized by anchors, gets its text's natural width where the reference keeps 0, because every width-less FontString here does. ([`src/script/simplehtml/mod.rs:15`](../crates/benilla-ui/src/script/simplehtml/mod.rs#L15))
- The reference prints these to the console; here they are host warnings, because a malformed page is bad content, not a script error. ([`src/script/simplehtml/mod.rs:181`](../crates/benilla-ui/src/script/simplehtml/mod.rs#L181))
- Only `%s` and `%%` are expanded and other specs copied, because the real `printf` would read the string pointers as other types. ([`src/script/simplehtml/parse.rs:244`](../crates/benilla-ui/src/script/simplehtml/parse.rs#L244))
- Skipped because it is no command; the reference raises. ([`src/script/slash.rs:64`](../crates/benilla-ui/src/script/slash.rs#L64))
- The app plays the queued file after the call, so the refusals (the bus-3 cap, a muted SFX channel, a missing file) come too late to answer, and a queued file answers 1. ([`src/script/sound.rs:68`](../crates/benilla-ui/src/script/sound.rs#L68))
- `BOOKTYPE_SPELL`/`BOOKTYPE_PET` are engine globals here, where the reference defines them in `SpellBookFrame.lua:5-6`, so this module's tests run without the stock file. ([`src/script/spellbook.rs:9`](../crates/benilla-ui/src/script/spellbook.rs#L9))
- Both read and format time as UTC where the reference uses local time, because the tree has no timezone database: the reference as it runs in a zone with no offset and no daylight saving. ([`src/script/stdlib.rs:240`](../crates/benilla-ui/src/script/stdlib.rs#L240))
- A format that ends in `%` reads past its terminator in the client (`0x4135c3`), into whatever memory follows the string; here the output ends there. ([`src/script/stdlib.rs:425`](../crates/benilla-ui/src/script/stdlib.rs#L425))
- The reference reads uninitialised fields before the seed; we answer the first legal design because that value is undefined. ([`src/script/tabard.rs:103`](../crates/benilla-ui/src/script/tabard.rs#L103))
- Returns nothing. The handler ends `mov eax,1` with nothing pushed (`0x4dcd7c`), so Lua 5.0 hands back the stack top, the node argument: an accident of the handler, not an API, and copying a stack-top leak would copy a bug. ([`src/script/taxi.rs:138`](../crates/benilla-ui/src/script/taxi.rs#L138))
- A handler that unregisters the saved next ends the dispatch, because the reference then reads a freed node. ([`src/script/tick.rs:30`](../crates/benilla-ui/src/script/tick.rs#L30))
- `0x530a52` lays it out at once; getters resolve first, so only OnSizeChanged fires later. ([`src/script/tooltip/size.rs:155`](../crates/benilla-ui/src/script/tooltip/size.rs#L155))
- The reference's builder hides and clears on a missing record (`0x52b72e`..`0x52b73f`), where a source that knows the item's name shows that, because a blank plate under an on-screen name reads as broken; the template's arrival then builds it whole. ([`src/script/tooltip_item/build.rs:53`](../crates/benilla-ui/src/script/tooltip_item/build.rs#L53))
- An object no longer held, or a source that keeps no guid (the player's own trade slot), rebuilds from the instance the build read, where the reference's lookup finds no object and builds from the record and the block alone; benilla's views carry no object to read once it is gone, and the kept instance is what the plate showed. ([`src/script/tooltip_item/build.rs:154`](../crates/benilla-ui/src/script/tooltip_item/build.rs#L154))
- The reference's padding repeats a mana stat once per zero entry; not emulated, since only the internal test ring 6674 has one. ([`src/script/tooltip_item/render.rs:416`](../crates/benilla-ui/src/script/tooltip_item/render.rs#L416))
- The unknown-skill "Requires %s" shares it, though its reference red is `0xffff0000`, because a 32/255 difference in G and B does not show. ([`src/script/tooltip_unit.rs:30`](../crates/benilla-ui/src/script/tooltip_unit.rs#L30))
- The selection clears to 0, because the header the reference re-selects (`0x4d7b42`, reading 1) has no spell id; the stock window's `> 1` test treats 0 and 1 alike. ([`src/script/trainer/mod.rs:197`](../crates/benilla-ui/src/script/trainer/mod.rs#L197))
- A header clears it too, because it has no spell id; the reference stores its `0xffffffff` (`0x4d7b05`) and reads back 1, which the stock window's `> 1` test treats the same as 0. ([`src/script/trainer/mod.rs:871`](../crates/benilla-ui/src/script/trainer/mod.rs#L871))
- Index 0 is a no-op, because one stray call would spend the purse; the reference buys every visible service for any index at or below 0 (`0x4da244`, `0x4d8a70`). ([`src/script/trainer/mod.rs:893`](../crates/benilla-ui/src/script/trainer/mod.rs#L893))
- Kept apart from `Self::vertex_color`, which the reference's `Texture:SetAlpha` (`0x79b580`) rewrites, because the drawn product differs only on a region that sets both, or a gradient and an alpha, which no stock code does, and a FontString's `SetAlpha` then keeps its font colour. ([`src/script/types.rs:381`](../crates/benilla-ui/src/script/types.rs#L381))
- UnitCanCooperate answers 1 for a friendly (5 and up) player, because the engine carries no faction templates; the reference resolves their cooperation masks. ([`src/script/unit/bindings.rs:612`](../crates/benilla-ui/src/script/unit/bindings.rs#L612))
- Among whole-name matches ours picks the nearest, because the reference's first-walked pick is order-dependent and reads as a bug. ([`src/script/unit/bindings.rs:1115`](../crates/benilla-ui/src/script/unit/bindings.rs#L1115))
- The reference ends the walk at the first whole-string hit (`0x493ca2`), so its pick follows the object list's order; we rank exact, then longer prefix, then nearest (`Rank::beats`), because the nearest namesake is the better pick, and a full tie goes to the lower guid (`Rank::beats_at`), so the VM's walk and the app's scan pick the same unit whatever order each walks in. ([`src/script/unit/by_name.rs:7`](../crates/benilla-ui/src/script/unit/by_name.rs#L7))
- A full tie goes to the lower guid, where the reference keeps the first unit its walk (`0x493aa0`) reaches, an order neither of ours has; a guid keeps the two picks equal. ([`src/script/unit/by_name.rs:46`](../crates/benilla-ui/src/script/unit/by_name.rs#L46))
- `0x4e81d0` has no bound and reads past its four-entry table, which is stale memory, not a mechanism; past it, nobody. ([`src/script/unit/resolve.rs:198`](../crates/benilla-ui/src/script/unit/resolve.rs#L198))
- The reference ties a miss against a hit (`0x5adab6`); no total order can, and `sort_by` may panic on one that is not, so the miss goes last. ([`src/script/who_sort.rs:161`](../crates/benilla-ui/src/script/who_sort.rs#L161))
- The engine's own pushes (the world-enter sync, the explored areas, the landmark sources, the catalog) queue `WORLD_MAP_UPDATE` for the next frame's tick and fire only on a change, where the reference fires in place from the handler that moved the data: the app's feeds run after the frame's tick. ([`src/script/worldmap.rs:13`](../crates/benilla-ui/src/script/worldmap.rs#L13))
- The reference fires it only after an init rebuild (`0x48fa12`) or a zone-defense channel flip (`0x49bdd6`); we fire on a value change too, because our rows are resolved values its Lua would re-read live. ([`src/script/worldstate.rs:35`](../crates/benilla-ui/src/script/worldstate.rs#L35))

#### widget

- The history browses as a shell's does, a small everyday gain: the first UP stashes the line being typed and DOWN past the newest line brings it back, UP holds at the oldest, DOWN does nothing when not browsing, typing or a focus gain ends the browse, and an empty line is not kept. ([`src/widget/kinds/editbox.rs:245`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L245))
- A plain Left or Right (or Ctrl+B or F) with a selection collapses it to that edge, as every OS's own text fields do; the reference clears the selection and steps once from the caret (`0x77c6b0` from `0x77c70b`). ([`src/widget/kinds/editbox.rs:814`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L814))
- Left reads the whole character before the caret, not the lone byte `caret - 1` the client reads (`0x77c8da`), which takes a multi-byte space for a non-space. ([`src/widget/kinds/editbox.rs:833`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L833))
- On the last row a walk may end at the text's end, because the end starts no row; the reference steps back from it as from a row's start (its line table ends with the text's length, `0x77d6ef`), so DOWN onto a short last line stops before its last letter. ([`src/widget/kinds/editbox.rs:884`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L884))
- The drag selects from its anchor, as every OS's own text fields do, where the reference's applies `0x77cd10` to each mouse move's whole delta (`0x77a8a1`). ([`src/widget/kinds/editbox.rs:997`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L997))
- A code point past U+FFFF is no space, where the client tests its low 16 bits, so U+10020 counts as a space there. ([`src/widget/kinds/editbox.rs:1127`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L1127))
- The reference has one field, the font's, but our font is read off the first `<FontString>` region (`message_frame_font`), which a `CreateFrame` frame lacks; so that FontString's own `GetJustifyH` does not see this. ([`src/widget/kinds/messageframe.rs:33`](../crates/benilla-ui/src/widget/kinds/messageframe.rs#L33))
- Kept apart from the font's, as `ScrollingMessageState::justify` is. ([`src/widget/kinds/messageframe.rs:532`](../crates/benilla-ui/src/widget/kinds/messageframe.rs#L532))
- `None` until then, not the ctor's 0, so that an unset row takes nothing rather than slot 0. ([`src/widget/kinds/mod.rs:862`](../crates/benilla-ui/src/widget/kinds/mod.rs#L862))
- On the thumb it is the grabbed point, where `CSimpleSlider` (`0x789ba0`/`0x789ca0`) always centers it, because that feels less surprising and a stock-sized thumb hides the difference. ([`src/widget/kinds/mod.rs:1220`](../crates/benilla-ui/src/widget/kinds/mod.rs#L1220))
- Levels pack contiguously, because the reference's resuming cursor (`0x764fd3`/`0x764fd8`) leaves a hole past a wide gap ({0, 4, 6} at bound 7 gives {0, 1, 3} at bound 4). ([`src/widget/propagation.rs:180`](../crates/benilla-ui/src/widget/propagation.rs#L180))
- The frame is still in the old parent's child list during `OnHide`, unlike the reference (`0x76ab34` unlinks first), because a handler that itself reparents must not find the lists half-spliced. ([`src/widget/propagation.rs:280`](../crates/benilla-ui/src/widget/propagation.rs#L280))
### benilla-world

#### clouds

- The reference starts this buffer at 0xFFFFFFFF; alpha 0 keeps a dome drawn before the first build from flashing white. ([`src/clouds/kernel.rs:94`](../crates/benilla-world/src/clouds/kernel.rs#L94))
- The index wraps where the reference reads past the tile, only at `u == 1.0`. ([`src/clouds/kernel.rs:308`](../crates/benilla-world/src/clouds/kernel.rs#L308))
- Clamped; the reference NaNs above about 70° elevation, where its bodies never go. ([`src/clouds/kernel.rs:338`](../crates/benilla-world/src/clouds/kernel.rs#L338))
- Seed 1, the CRT default, for determinism; the reference's is the process `srand` state, and any uniform table looks the same. ([`src/clouds/tables.rs:47`](../crates/benilla-world/src/clouds/tables.rs#L47))

#### clutter

- ×2, `frillDensity` 32 (the panel's Medium), because a fresh install's `hwDetect` value (24 from `VideoHardware.dbc`, 8 on the weakest parts) is on no panel stop, and Medium is the nearest stop no sparser. ([`src/clutter.rs:155`](../crates/benilla-world/src/clutter.rs#L155))

#### collision

- The reference's camera trace is a bare ray (`0x672170`); the sphere keeps the near plane out of a wall. ([`src/collision.rs:38`](../crates/benilla-world/src/collision.rs#L38))
- The slide, step and snap are avian's kinematic controller, with parry's edge-hit normals, not the reference's resolver, because pieces of that resolver grafted onto the controller left the mover in states the reference never reaches; only its face filters, the facing law and the backface band, are ported. ([`src/collision/one_sided.rs:12`](../crates/benilla-world/src/collision/one_sided.rs#L12))

#### doodad_anim

- Every instance scrolls in one phase, where the reference phases a band loop per arm and a gseq loop from each instance's attach (`CM2Model+0x68`), because one uniform serves the material and a seamless scroll looks the same at any phase. ([`src/doodad_anim/mat_anim.rs:186`](../crates/benilla-world/src/doodad_anim/mat_anim.rs#L186))
- This runs for every resident host, where the reference stops a doodad culled out of its scene list (`[CM2Scene+0x20]`) and re-arms it at once on re-link (`0x7074b0`, `0x683f80`), because gating it so would re-roll the whole field on the frame the camera turns back. ([`src/doodad_anim/mod.rs:256`](../crates/benilla-world/src/doodad_anim/mod.rs#L256))

#### entity_shade

- The lit outdoor target is intensity 1.0, not the reference's 2.5, because `wow_model.wgsl` caps the intensity at 1.0 (`min(I, 1)`) and a chase through the capped span would stall unseen. ([`src/entity_shade.rs:38`](../crates/benilla-world/src/entity_shade.rs#L38))

#### exterior_cull

- The reference tests one AABB per object, but a doodad placement has no root entity here, so its submeshes are tested one by one, and a submesh outside a doorway's window drops where the reference draws the doodad whole. ([`src/exterior_cull.rs:29`](../crates/benilla-world/src/exterior_cull.rs#L29))

#### ffx_glow

- Kept a float where every reference pack (`0x6cb0e2`/`0x6cb557`/`0x6cb9e1`) quantizes it to a byte, because the two differ by under a thousandth (38/255 against 0.15). ([`src/ffx_glow.rs:176`](../crates/benilla-world/src/ffx_glow.rs#L176))

#### ground_fx

- The 1.12 client draws these flat quads as ordinary depth-tested M2 batches, so an up-slope buries them and a down-slope leaves them floating; here they drape the terrain like the selection ring, because an effect meant for ground level should sit on it. ([`src/ground_fx.rs:4`](../crates/benilla-world/src/ground_fx.rs#L4))

#### interact

- The broad bound is the mesh `Aabb` grown by 1 unit, not the reference's sequence-bounds sphere, because it can only err permissive. ([`src/interact/pick.rs:174`](../crates/benilla-world/src/interact/pick.rs#L174))

#### lighting

- The round trip's 8-bit quantization is skipped, because it moves a channel by under 0.5 %. ([`src/lighting/global_light.rs:83`](../crates/benilla-world/src/lighting/global_light.rs#L83))
- No 8-bit repack (`×255 + 512 >> 14`), because this light lane is float end to end. ([`src/lighting/resolve.rs:165`](../crates/benilla-world/src/lighting/resolve.rs#L165))

#### liquid

- Mote `i` draws cell `i & 7` of its set, because the reference reads its cell index at the bottom of the draw loop, an off-by-one that gives each mote the previous one's cell and slot 0 a hardcoded cell 8. ([`src/liquid/drift.rs:10`](../crates/benilla-world/src/liquid/drift.rs#L10))
- The water gust moves `speed·dt·60` a frame where the reference (`0x68e4f0`, `mode <= 1`) moves `speed` with no `dt`, because the reference's drift doubles from 30 to 60 fps; 60 Hz holds the top of the era's range at any frame rate. ([`src/liquid/drift.rs:56`](../crates/benilla-world/src/liquid/drift.rs#L56))
- Cell 12 is completed, because the initialiser's last store (`0x68efa5`) leaves its bottom-right corner at zero and one magma mote in four would smear across the atlas diagonal. ([`src/liquid/drift.rs:70`](../crates/benilla-world/src/liquid/drift.rs#L70))
- Floored at the frustum, because the cone stops containing the view at about 21:9, and a wider window would lose motes in bands down its sides. ([`src/liquid/drift.rs:93`](../crates/benilla-world/src/liquid/drift.rs#L93))
- Water frames have their per-frame mean flattened (`flatten_frame_dc`), because mipping turns the shipped frames' drifting means into a flicker on distant water; magma and slime keep theirs, the body's intended pulse. ([`src/liquid/surface.rs:459`](../crates/benilla-world/src/liquid/surface.rs#L459))

#### modkeys

- The reference (`0x4b7990`) retries a binding miss once without the leftmost modifier, so `CTRL-SHIFT-P` would reach `SHIFT-P` (`TOGGLECHARACTER3`, the pet paper doll); in a run with dev affordances `bindings::BindingDispatch::resolve` skips that retry on this plane, because a dev chord must open nothing in game. ([`src/modkeys.rs:22`](../crates/benilla-world/src/modkeys.rs#L22))

#### particles

- `AlphaTest` maps to `Alpha`, because flat `Spells\` quads are blend batches and a cutout would bite a soft decal's fade; and the part draws unlit, as no lit ground-fx part is known. ([`src/particles/buffer.rs:57`](../crates/benilla-world/src/particles/buffer.rs#L57))
- Quads keep pool order within a cloud, without the reference's back-to-front sort (`0x7b3a10`), because only an alpha-blended cloud can show the order; between clouds, each draw record's anchor sorts it. ([`src/particles/quads.rs:154`](../crates/benilla-world/src/particles/quads.rs#L154))
- `w` is the frame dt clamped to 0.1 s, because we do not substep as the reference does at 0.1 s (`0x81d828`, `0x7b58e7`); they differ only on a hitch. ([`src/particles/sim.rs:162`](../crates/benilla-world/src/particles/sim.rs#L162))
- In model mode the reference adds the raw world vector to local coordinates; we fold it into the local frame, because our world axes are Bevy's, not WoW's. ([`src/particles/sim.rs:753`](../crates/benilla-world/src/particles/sim.rs#L753))

#### shaders

- The `min(I, 1)` cap, as in wow\_model.wgsl, since lifting it takes sun-facing surfaces past 1.0; Matte keeps its own bit so it stays 1.0 if the cap goes. ([`src/shaders/static_gx.wgsl:325`](../crates/benilla-world/src/shaders/static_gx.wgsl#L325))

#### sky_order

- One constant, not the reference's per-lane magnitudes (ring and shadow at one site, `0x6d7480`; footprints 10x, `0x69a54a`; foam 20x), because those suit its 24-bit fixed-point forward-Z buffer; in our reversed-Z float buffer the bias is ULP-relative, and one bake path sets one residual. ([`src/sky_order.rs:84`](../crates/benilla-world/src/sky_order.rs#L84))
- Only a few ULPs, a guard against drivers rounding coplanar arithmetic differently, because the reference's `1.29847e−3 · d²` yd pull paints the splash onto dry bank (0.3 to 1.0 yd at an 11.9-yd camera, measured). ([`src/sky_order.rs:113`](../crates/benilla-world/src/sky_order.rs#L113))

#### terrain_stream

- The debug free-fly eye holds nothing: it is benilla's own tool with no reference counterpart, and a stall there only slows the developer. ([`src/terrain_stream.rs:390`](../crates/benilla-world/src/terrain_stream.rs#L390))
- A tile is released only one chunk past the outer window (`StreamWindow::keeps`), where the reference releases by window membership (`0x6984f0`), because our reload is a re-decode and re-spawn, not a cheap async read. ([`src/terrain_stream.rs:719`](../crates/benilla-world/src/terrain_stream.rs#L719))
- A read still loading after `INNER_WAIT_CAP` is given up on with a warning, once per request, and its tile then streams on unwaited like the outer ring, where the reference waits for ever: a hung read costs one capped stall, not one every frame. ([`src/terrain_stream.rs:814`](../crates/benilla-world/src/terrain_stream.rs#L814))
- Benilla gates on terrain alone, which is more permissive; the settle hold covers a building or doodad arriving after a teleport. ([`src/terrain_stream/residency.rs:9`](../crates/benilla-world/src/terrain_stream/residency.rs#L9))
- Tagged per submesh, where the reference tests the whole object, because a placement has no root entity to carry the bound. ([`src/terrain_stream/spawn/mod.rs:229`](../crates/benilla-world/src/terrain_stream/spawn/mod.rs#L229))
- The box, where the reference tests its circumsphere (`0x683700` → `0x682ef0` → `0x686b80` on the `[rec+0x5c]` centre and `[rec+0x68]` radius × scale), because Bevy culls by box and the box is tighter. ([`src/terrain_stream/spawn/mod.rs:896`](../crates/benilla-world/src/terrain_stream/spawn/mod.rs#L896))
- Whole tiles, because benilla loads, spawns and releases ADT tiles, not chunks: a tile is wanted when any of its chunks lies in the outer window, and waited for when any lies in the inner one, so each edge is a tile line where the reference's is a chunk line. ([`src/terrain_stream/window.rs:7`](../crates/benilla-world/src/terrain_stream/window.rs#L7))
- A one-chunk keep band (`KEEP_BAND_CHUNKS`), because a tile reload re-decodes and re-spawns where the reference's is a cheap read: a tile releases only past `outer + 1`. ([`src/terrain_stream/window.rs:10`](../crates/benilla-world/src/terrain_stream/window.rs#L10))

#### view

- The reference refuses a value outside them, echoing "NearClip must be in range 0.01 - 0.33" (`0x869abc`); ours clamps, as every CVar consumer here clamps at its own edge. ([`src/view.rs:84`](../crates/benilla-world/src/view.rs#L84))
- The reference projects the detailed world to `farclip` and the horizon to `horizonfarclip` (default 2112, floored at `farclip + 528`); ours has one far plane well beyond `farclip`, because the WDL ring draws in the same projection, and the detailed world ends at `farclip` by the wall. ([`src/view.rs:242`](../crates/benilla-world/src/view.rs#L242))

#### water_fx

- The first draw is `size0`, where the reference first ages the record one `dt·growth` step, a difference too small to see at our frame rates. ([`src/water_fx/params.rs:86`](../crates/benilla-world/src/water_fx/params.rs#L86))

#### wdl

- 5, not the reference's ±3-tile far walk, because hills out to its `horizonfarclip` (2112 yd, about 4 tiles) still rise above the horizon as silhouettes. ([`src/wdl.rs:24`](../crates/benilla-world/src/wdl.rs#L24))

#### weather

- It defaults to 3 where the reference registers 2, because the precipitation rates were graded against a reference install running 3. ([`src/weather/mod.rs:103`](../crates/benilla-world/src/weather/mod.rs#L103))
- A spawn ceiling on live plus pending drops, which the reference lacks, because each flake here costs a 4-vertex quad. ([`src/weather/precip/mod.rs:47`](../crates/benilla-world/src/weather/precip/mod.rs#L47))
- The drop rate is scaled to a 30 fps client, the frame rate the reference's look was matched at, because the reference budgets `min(dt, 1/60)·rate` and drops the remainder (`0x67846f`–`0x678480`), so it emits half its nominal rate at 30 fps. 60 is byte-faithful. ([`src/weather/precip/mod.rs:78`](../crates/benilla-world/src/weather/precip/mod.rs#L78))
- Flake pixels are pixels of an 800-px-tall screen, the height the look was matched at, converted to an angle, because the reference's literal framebuffer pixels shrink with resolution (2.7× smaller at 4K scale 2). ([`src/weather/precip/mod.rs:140`](../crates/benilla-world/src/weather/precip/mod.rs#L140))
- xorshift32 to \[0, 1) instead of the reference's lagged-table generator, because the laws need only a uniform distribution, not its exact stream. ([`src/weather/precip/mod.rs:174`](../crates/benilla-world/src/weather/precip/mod.rs#L174))

#### wmo_portal

- When legs A and B both miss and no terrain lies at or below the eye, Leg C runs the same race over the camera-only faces (DETAIL without NOCAMCOLLIDE), because the 1.12 client blanks the whole building from a pocket floored only with DETAIL faces, as at the Deadmines entrance. ([`src/wmo_portal/seed.rs:21`](../crates/benilla-world/src/wmo_portal/seed.rs#L21))

#### zfill

- The reference twins every translucent command, authored-blend glass included; here twins exist only during an instance alpha episode, so every steady-state look is untouched. ([`src/zfill.rs:9`](../crates/benilla-world/src/zfill.rs#L9))
- Bevy sorts per entity, so twins lead by a fixed −8 yd sort bias (`crate::model_render::ZFILL_SORT_BIAS`) instead of by the instance's sort-key tie; transparent content within that window behind a fading body is depth-clipped where the body covers it, for the episode. ([`src/zfill.rs:12`](../crates/benilla-world/src/zfill.rs#L12))

