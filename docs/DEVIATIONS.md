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
| `gxResolution` | `640x480` | `1600x900` | the saved value is the WINDOWED size, as fullscreen is the borderless window over the monitor, whose size the row answers while it holds; and 640x480 is not a window anyone would ship a client at |
| `realmList` | `us.logon.worldofwarcraft.com:3724` | `localhost` | that host has not resolved since 2019, so shipping it makes every first launch a DNS failure; benilla dials the machine it is running on |
| `SoundReverb` | `1` | `0` | the reference's reverb is EAX-over-hardware, and that hardware has not existed since Vista, so "1" would ship audio the real client has never actually produced on any machine a player runs today |
| `weatherDensity` | `2` | `3` | every precipitation rate in `benilla-world`'s own precipitation module was derived and graded against the reference install's own apitrace captures, and that install runs `SET weatherDensity "3"` (K = 1.0) — so 3 is the value a benilla-vs-reference side-by-side is correct at, and the registered 2 would thin every rate to 0.66 against the only client we compare with. The slider is how a player takes it back down |

## Deviations in the code

Each entry is the first sentence of a `Deviation:` comment, linking to where it lives.

- [benilla-app](#benilla-app): 251
- [benilla-assets](#benilla-assets): 6
- [benilla-blp](#benilla-blp): 1
- [benilla-formats](#benilla-formats): 24
- [benilla-lua](#benilla-lua): 11
- [benilla-protocol](#benilla-protocol): 29
- [benilla-srp](#benilla-srp): 2
- [benilla-ui](#benilla-ui): 135
- [benilla-world](#benilla-world): 49

### benilla-app

#### area

- The indoor dedup keys `(WMOID, MOGP uniqueID)`, not the client's raw per-WMO group index, because the raw index can alias across adjacent buildings. ([`src/area.rs:30`](../crates/benilla-app/src/area.rs#L30))

#### assets/ui

- `OpenBackpack` opens every equipped bag, not only the backpack, so the vendor and mailbox windows that call it (MerchantFrame.lua:35, MailFrame.lua:39) bring up every bag to sell or mail from. ([`assets/ui/ContainerFrameAdapters.xml:9`](../crates/benilla-app/assets/ui/ContainerFrameAdapters.xml#L9))
- The stock FCF\_ChatTabFadeFinished (FloatingChatFrame.lua:989-992) nils oldAlpha even under a live hover, so leaving and re-entering within CHAT\_FRAME\_FADE\_TIME (a flick to the scroll buttons, outside the hover box, does it) stops the plate fading in (l.873) for the rest of the session; this keeps oldAlpha while hover is set. ([`assets/ui/FrameXMLFixes.xml:6`](../crates/benilla-app/assets/ui/FrameXMLFixes.xml#L6))
- Re-runs UIParent\_ManageFramePositions after DurabilityFrame's own OnEvent. ([`assets/ui/FrameXMLFixes.xml:20`](../crates/benilla-app/assets/ui/FrameXMLFixes.xml#L20))
- Friendly plates the player turned on come back at every world entry. The stock UpdateNameplates (UIOptionsFrame.lua:768-780) shows them and hides them again at once (l.775-776), and FRIENDNAMEPLATES\_ON is never saved, since l.182 registers NAMEPLATES\_ON a second time. ([`assets/ui/FrameXMLFixes.xml:33`](../crates/benilla-app/assets/ui/FrameXMLFixes.xml#L33))
- A button whose key moved away keeps the old key's label while it holds an action with no range (a stance, a self buff). ([`assets/ui/FrameXMLFixes.xml:51`](../crates/benilla-app/assets/ui/FrameXMLFixes.xml#L51))
- The rungs are the 1.15 era client's ESC menu, because its settings screens replace 1.12's, which are much worse to use; the era's AddOns rung is dropped, since the character-select AddOns screen is the only addon UI. ([`assets/ui/GameMenuAdapters.xml:4`](../crates/benilla-app/assets/ui/GameMenuAdapters.xml#L4))
- The layout is the era Settings panel's Keybindings page (collapsible category sections), because 1.12's settings screens are much worse to use. ([`assets/ui/KeyBindingsPage.xml:5`](../crates/benilla-app/assets/ui/KeyBindingsPage.xml#L5))
- Every bind, unbind and reset saves at once, as the era panel commits, where 1.12 had Okay and Cancel; only Reset To Default and leaving the character set confirm. ([`assets/ui/KeyBindingsPage.xml:11`](../crates/benilla-app/assets/ui/KeyBindingsPage.xml#L11))
- The spacer headers BLANK, BLANK2 and BLANK3 over multibars 2-4 (Bindings.xml:883/967/1051), which 1.12 draws blank (BINDING\_HEADER\_BLANK is "  ", the other two are not defined), continue the section above, because this page's sections are named and collapsible. ([`assets/ui/KeyBindingsPage.xml:123`](../crates/benilla-app/assets/ui/KeyBindingsPage.xml#L123))
- The 1.15 era client's single settings window replaces the 1.12 four-panel family, which is much worse to use; the 1.12 frames still load, hidden, for addons. ([`assets/ui/OptionsFrame.xml:3`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L3))
- AUTO\_QUEST\_WATCH is the string "1". 1.12 assigns the number 1 (l.122) but every consumer compares against "1" (QuestLogFrame.lua:64/68); 1 ~= "1" in Lua 5.0. ([`assets/ui/OptionsFrame.xml:31`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L31))
- The bar toggles precede the lock, which 1.12's box leads with, so what is on screen reads first. ([`assets/ui/OptionsFrame.xml:498`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L498))
- A named ladder reads out its stop; 1.12's slider labels only its ends, and a bare index would mean nothing to a player. ([`assets/ui/OptionsFrame.xml:631`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L631))
- 1.12 returns early here; a mute row hides the last plate, so a neighbour's text cannot stand when its OnLeave was missed. ([`assets/ui/OptionsFrame.xml:746`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L746))
- No partner write of cameraPitchSmoothSpeed, which has no reader: the follow here has one rate. ([`assets/ui/OptionsFrame.xml:1697`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L1697))
- Modern Classic's two-entry Display Mode, as there is no exclusive fullscreen; gxWindow "0" is the borderless window. ([`assets/ui/OptionsFrame.xml:1983`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L1983))
- Our own row (the reference draws to the backbuffer), and no Multisampling, which misses alpha-tested cutouts. ([`assets/ui/OptionsFrame.xml:2012`](../crates/benilla-app/assets/ui/OptionsFrame.xml#L2012))
- 1.12 has no `/reload` (later clients added it; 1.12 reloads through `/console reloadui`); kept so a player can reload the interface after toggling addons. ([`assets/ui/ScriptLogFrame.xml:381`](../crates/benilla-app/assets/ui/ScriptLogFrame.xml#L381))
- A three-slice where the reference overlaps two quads, since two quads reach only 362 units and a page here is 563 tall. ([`assets/ui/ScrollTemplates.xml:190`](../crates/benilla-app/assets/ui/ScrollTemplates.xml#L190))
- The pet page's autocast widgets take the pet bar button's proportions, so the shine and the corner brackets sit on the button's edge and share its centre. ([`assets/ui/SpellBookAdapters.xml:3`](../crates/benilla-app/assets/ui/SpellBookAdapters.xml#L3))

#### aura_visual

- A non-zero rate is ignored, because the only one shipped (`8947848.0` on kit 1744, a packed grey in the rate column) is noise on the reference's integer-ms clock and has no faithful form on our f32-second one. ([`src/aura_visual.rs:212`](../crates/benilla-app/src/aura_visual.rs#L212))

#### bindings

- A press with Super (Cmd) held runs no binding, so a system chord such as Cmd+Tab or Cmd+Q never acts in game. ([`src/bindings.rs:19`](../crates/benilla-app/src/bindings.rs#L19))
- A trackpad's `Pixel` deltas are carried until they make a line, and step at most once per event (`crate::ui_script::Record::Wheel`). ([`src/bindings.rs:637`](../crates/benilla-app/src/bindings.rs#L637))
- Latches die with the VM that made them, because a latch names a command of that VM's table and releasing it against a new one would run another load's up-half. ([`src/bindings.rs:682`](../crates/benilla-app/src/bindings.rs#L682))
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

- Sizes ride the plates' damped diagonal basis (`plate_basis`), not the unbounded reference law, so bubble text and plate text keep one em at every window size. ([`src/chat_bubble.rs:24`](../crates/benilla-app/src/chat_bubble.rs#L24))

#### cinematic

- Bevy UI nodes computed per frame, not `CinematicFrame.xml`'s textures, so a window resized mid-cinematic stays letterboxed. `CinematicFrame` still shows and owns ESC. ([`src/cinematic.rs:187`](../crates/benilla-app/src/cinematic.rs#L187))

#### combat_text

- The reference stretches a raster of at most 32 px; each whole-pixel size is rasterized here, so the crit pop stays crisp. ([`src/combat_text/mod.rs:273`](../crates/benilla-app/src/combat_text/mod.rs#L273))

#### console

- A line of eight names is not left with its trailing `, ` and a list of a multiple of eight names is not followed by `NONE`, two slips of the reference's loop (`0x63ffed`, `0x64004d`). ([`src/console.rs:467`](../crates/benilla-app/src/console.rs#L467))
- A CVar lists under its registration's category with its registration's help, even when `config.toml` held it at boot. ([`src/console.rs:472`](../crates/benilla-app/src/console.rs#L472))

#### creature_anim

- A new puff inside the last one's clip is declined instead of replacing it (`0x6208e0`), because one-shot effects carry no reap key; either way two never overlap. ([`src/creature_anim/breath.rs:17`](../crates/benilla-app/src/creature_anim/breath.rs#L17))
- A mounted draw is refused at the setter, because our rider track never replays the reference's per-play force-stow (`0x5fdf80`); the result is the same. ([`src/creature_anim/driver.rs:769`](../crates/benilla-app/src/creature_anim/driver.rs#L769))
- The reference writes bone 0's rate blindly; we scope it to the swing's own node, because a moving attacker's gait would otherwise slow too. ([`src/creature_anim/driver.rs:1088`](../crates/benilla-app/src/creature_anim/driver.rs#L1088))
- A time-based rate, because the reference's per-frame step changes with the frame rate. ([`src/creature_anim/select.rs:137`](../crates/benilla-app/src/creature_anim/select.rs#L137))
- A spell's state kit and body procs are reaped only once no slot holds the spell. ([`src/creature_anim/spell_visual.rs:1275`](../crates/benilla-app/src/creature_anim/spell_visual.rs#L1275))

#### cvars

- The reference's list runs in creation order, the records `Config.wtf` created first in file order (`0x63d8eb`), then each registration as start-up reaches it (`containers`), an order benilla-re has not traced. benilla lists in the table's order, which is register-site address order, then the rows the table lacks (benilla's own, an addon's) as they registered. ([`src/cvars/listing.rs:5`](../crates/benilla-app/src/cvars/listing.rs#L5))
- For a row that is not latched the change callback runs, inside the call that restored it, where the reference runs no callback here (`0x63dff0`, `0x63e010`), because a knob moves only through its callback (`callbacks::CvarCallbacks`), which also stands in for the reference's direct reads of the record; without it knob and value would disagree until the next write. ([`src/cvars/mod.rs:439`](../crates/benilla-app/src/cvars/mod.rs#L439))
- The reference's `SaveConfig` (`0x63d980`) writes only at world entry (`0x46b6f6`), at the leave-world tail (`0x490c35`-`0x490c64`) and at shutdown (`0x63daf0`), so a crash or a kill loses every change since; here a player's settings survive one. ([`src/cvars/mod.rs:1170`](../crates/benilla-app/src/cvars/mod.rs#L1170))
- The reference clips at full scale and has no headroom mechanism (its SFX-bus duck `0x457960` is a sidechain armed only by server-pushed voice lines); benilla limits because every SFX is mastered to full scale and overlapping kits clip. ([`src/cvars/table.rs:243`](../crates/benilla-app/src/cvars/table.rs#L243))
- Here the dial multiplies the camera's own rate, because benilla does not change the OS pointer speed, a system-wide setting. ([`src/cvars/table.rs:485`](../crates/benilla-app/src/cvars/table.rs#L485))
- "0" raises a borderless fullscreen window instead of mode-setting the display, because Wayland and macOS offer no mode-set and X11's leaves the desktop changed after a crash (`crate::video`). ([`src/cvars/table.rs:615`](../crates/benilla-app/src/cvars/table.rs#L615))
- Benilla, which has no exclusive mode, applies the same curve in the composite pass (`crate::ui_gamma::DisplayGamma`), because the skipped upload would make a slider that moves no pixel. `SetGamma 0x4891f0` writes `"%f"` ("1.000000"), which `SaveConfig` (`0x63da4a`) keeps as a diff against the default "1.0", as the reference does. ([`src/cvars/table.rs:729`](../crates/benilla-app/src/cvars/table.rs#L729))

#### death

- `0x492130` compares the positions whatever map we stand on, so a ghost on another map at the corpse's coordinates reads in range for a reclaim the server refuses; here only the corpse's display map counts. ([`src/death/mod.rs:308`](../crates/benilla-app/src/death/mod.rs#L308))

#### entities

- A cap on one hop's sub-segment count, which the reference lacks, so a vertex run is never sized off table data alone; shipped rows peak at 12 (30 yd at `avgSegLen` 2.78). ([`src/entities/chain_beam.rs:54`](../crates/benilla-app/src/entities/chain_beam.rs#L54))
- Not the reference's generator (`((rand & 0x7fffff) | 0x3f800000)` re-centred through `2.0`), because the look rests on the amplitude and the advection, both exact, never on the draw sequence. ([`src/entities/chain_beam.rs:110`](../crates/benilla-app/src/entities/chain_beam.rs#L110))
- The reference updates every unit every frame; here only units showing a flagged visual are tracked, and a parked rig skips its ray and re-seeds at the slope on waking, to bound the cost at one ray per flagged unit on screen. ([`src/entities/conform.rs:67`](../crates/benilla-app/src/entities/conform.rs#L67))
- The reference copies a finished model, ours stream in, so while false the pane keeps re-taking and a late weapon is baked. ([`src/entities/equipment/resolve.rs:181`](../crates/benilla-app/src/entities/equipment/resolve.rs#L181))
- It attaches at that clip's `$SHL`, where the reference attaches it at the draw's start, because it keys on the ranged slot's sheath so it arrives with the bow it feeds; the two are a few hundred ms apart in one clip and never seen apart. ([`src/entities/equipment/resolve.rs:560`](../crates/benilla-app/src/entities/equipment/resolve.rs#L560))
- `Hold` is one repeating play where the reference re-arms it each pass with variation -1, a weighted random pick, because at most one `Spells\` model authors several variations. ([`src/entities/spell_fx/lifecycle.rs:11`](../crates/benilla-app/src/entities/spell_fx/lifecycle.rs#L11))

#### game_tip

- The reference rasterizes the tip's font at most 32 px (`0x5ca096`/`0x5ca09b` in `0x5ca030`, reached through `0x44d040`) and magnifies it to the string's height; this rasterizes at the true size, as the engine does all big text (`crate::ui_text`'s engine). ([`src/game_tip.rs:191`](../crates/benilla-app/src/game_tip.rs#L191))

#### go_templates

- One per entry, where the reference keeps one per call, as the creature arm's waits are kept (`benilla_ui::script::quest_log`). ([`src/go_templates.rs:161`](../crates/benilla-app/src/go_templates.rs#L161))

#### lanes

- The reference has no such cap. A signal raised inside a listener runs nested in its call, bounded by `luaD_call`'s C-stack count of 200 (`0x6f65ae`), whose error ends the innermost listener alone, or by the stack of the thread that runs the loop, which the image's stack reserve of 0x100000 bounds, whichever comes first: a chain through the chat line function `0x49a870` takes an 8 KB frame each (`0x49a873`, `0x1f0c` bytes). ([`src/lanes.rs:28`](../crates/benilla-app/src/lanes.rs#L28))

#### loading_screen

- The live window's aspect stands in for `gxResolution`'s, as in `portrait`'s `GxAspect`, because the window here takes any shape a drag gives it, where the reference's keeps `gxResolution`'s (`0x5989d7`). ([`src/loading_screen.rs:66`](../crates/benilla-app/src/loading_screen.rs#L66))
- The wheel is swallowed, where the reference still zooms the hidden camera, because nothing should act through the screen. ([`src/loading_screen/input.rs:12`](../crates/benilla-app/src/loading_screen/input.rs#L12))
- The world pick is cleared, where the reference's freezes: the screen's layer blocks the frame pass (bit 0 clear, `0x406808` → `0x4083c0`), so `0x481790` does not run, except after a transport arrival (`0x5debb6`), when it picks at the frozen point. ([`src/loading_screen/input.rs:14`](../crates/benilla-app/src/loading_screen/input.rs#L14))

#### local_state

- The reference writes `Screenshots\\` inside the install, which benilla never writes to; the folder keeps the reference's name. ([`src/local_state.rs:433`](../crates/benilla-app/src/local_state.rs#L433))

#### minimap

- Dots re-project every frame, where the reference draws coordinates about 1 s stale, because that staleness is a throttle quirk. ([`src/minimap/blips/mod.rs:28`](../crates/benilla-app/src/minimap/blips/mod.rs#L28))

#### nameplates

- `AlphaMode::Blend` writes no depth where the reference does, because overlapping names resolving by sort order look the same. ([`src/nameplates.rs:4`](../crates/benilla-app/src/nameplates.rs#L4))
- A mounted plate keeps only this fraction of its seat's oscillation about a slow mean, because the raw seat, which the reference also rides, read more intense than the reference side by side. `1.0` is the raw seat; on-foot plates are untouched. ([`src/nameplates.rs:300`](../crates/benilla-app/src/nameplates.rs#L300))
- Larger than the reference's 32 px `UNIT_NAME_FONT` raster, which it magnifies (`0x6c749b`), because a larger source is crisper up close. ([`src/nameplates.rs:309`](../crates/benilla-app/src/nameplates.rs#L309))
- `ShouldShowName`'s first leg is `IsSelectable` (`0x60be60`), which passes our own unit; what hides the own name in first person in 1.12.1 is untraced, so the avatar's fade stands in. ([`src/nameplates.rs:541`](../crates/benilla-app/src/nameplates.rs#L541))

#### names

- For a guid it does not hold, the reference's combat-log namer (`0x6264e0`) asks the creature cache under guid bits 32-47 (`0x6265c4`-`0x6265e0`) for every guid whose top nibble is 0xf and whose bits 32-47 are nonzero, a game object's and a pet's included, and never the pet-name cache (only a held unit reaches it, `0x6092c7`). ([`src/names.rs:144`](../crates/benilla-app/src/names.rs#L144))

#### net

- UTC days, where `mktime` reads the host's local time zone, because the app has no time zone source (`benilla_ui::civil` has the same). ([`src/net/game_time.rs:354`](../crates/benilla-app/src/net/game_time.rs#L354))
- The reference loads the world first and logs in after: the entry's first frame starts the load at the record's map and position (`0x401570`, called at `0x46c236`), and a later one, once its file reader is idle (`0x46c247`), sends the login (`0x46c272`). ([`src/net/io.rs:10`](../crates/benilla-app/src/net/io.rs#L10))
- The reference re-dials only from `RequestRealmList`, and only when the link is down (`[CLoginMgr+5]` clear, `0x5ab857` → `0x5b2200`), with the reconnect challenge (`0x5b2134`, `[Logon+0xc]`); the connect's query sends only on a link in state 6 (`0x5bc14c`) and otherwise nothing, leaving the screen on its connecting dialog for good. ([`src/net/io.rs:407`](../crates/benilla-app/src/net/io.rs#L407))
- The reference's guard at `0x537e85` sends 0 whenever the write index is 0, one ping in sixteen; we send 0 only before the first pong, because the rest is an artefact of its index arithmetic that only the server stores. ([`src/net/io.rs:1300`](../crates/benilla-app/src/net/io.rs#L1300))
- The reference places collision and culling by the raw facing, ours follow the display facing, because a capsule and a sphere barely change under rotation. ([`src/net/motion/facing.rs:9`](../crates/benilla-app/src/net/motion/facing.rs#L9))
- The reference paces a self move through the same replay chain as a remote's, which would only delay a correction to our own pose. ([`src/net/motion/remote.rs:84`](../crates/benilla-app/src/net/motion/remote.rs#L84))
- Its rate-limited hover climb (`0x636fa1`, 7 yd/s) is not reproduced, because the cache gate needs a pure function; it converges to this answer. ([`src/net/motion/spline.rs:659`](../crates/benilla-app/src/net/motion/spline.rs#L659))
- A first-pass values block the clear header byte left unwritten re-fires, for a live object, its last write's edges against the watch copy (`0x465570`, `0x466686`); benilla compares against this packet's copies, which hold none for it, so a stale edge never fires twice. ([`src/net/objects/update.rs:1086`](../crates/benilla-app/src/net/objects/update.rs#L1086))
- The create notify uses its lookup's result untested (`0x465c97`, `0x465d79`). ([`src/net/objects/update.rs:1098`](../crates/benilla-app/src/net/objects/update.rs#L1098))
- On benilla's unattended reconnect, which the reference lacks, the world's VM survives the loss, so the clear signals its `PLAYER_TARGET_CHANGED` and usable refresh as a release's teardown does (`0x493a69`, `0x493a73`), still sending nothing, and the surviving target frame lets the lost unit go. ([`src/net/session.rs:310`](../crates/benilla-app/src/net/session.rs#L310))

#### no_install

- A boot with no install at all ends at a fatal error dialog, where the reference logs each missing archive and goes on (`0x403b00`), because nothing past this point can draw a word. ([`src/no_install.rs:7`](../crates/benilla-app/src/no_install.rs#L7))

#### player

- The reference re-projects the point's height onto the first walkable facet a zero-size box gather finds at the enemy (`0x60fe25`-`0x60fe51`); here it keeps the enemy's height. ([`src/player/approach.rs:417`](../crates/benilla-app/src/player/approach.rs#L417))
- The reference stores the world point unconverted and reads it back as deck-local (`0x60fe6c`, `0x610e95`), so on a transport it walks astray; here it rides the deck. ([`src/player/approach.rs:420`](../crates/benilla-app/src/player/approach.rs#L420))
- The reference converts back through the transport the player stands on each frame (`0x610e6b`), so stepping off mid-walk sends the point astray; here the deck is fixed at the arm. ([`src/player/approach.rs:449`](../crates/benilla-app/src/player/approach.rs#L449))
- The melee arm names the player's own guid (`0x60fe5f`), so the reference turns to face itself, a fixed heading; here the melee walk does not turn. ([`src/player/approach.rs:665`](../crates/benilla-app/src/player/approach.rs#L665))
- The shape is kept, its scale anchored to `LOOK_SENSITIVITY` on both axes at the default speeds, because the unit factor is a per-machine OS setting and this keeps the shipped feel: both axes turn alike, where the reference's yaw turns 1.5× faster than its pitch. ([`src/player/camera.rs:121`](../crates/benilla-app/src/player/camera.rs#L121))
- A delay holds the channel at `from`, because the reference's `0x512830` backs the start time up by the delay, so `s` runs negative and the even cosine jumps the value away and back, an unclamped divide rather than a behaviour. ([`src/player/camera_channel.rs:13`](../crates/benilla-app/src/player/camera_channel.rs#L13))
- A code table, not the reference's ninety `cameraTerrainTilt<Style><State>` CVars, because the matrix is a law, not a setting. ([`src/player/camera_dynamics.rs:285`](../crates/benilla-app/src/player/camera_dynamics.rs#L285))
- The reference sweeps the camera collision against the bobbed eye (`0x50ed47`); benilla adds the offset after the sweep, because it shares camera shake's accumulator there. ([`src/player/camera_dynamics.rs:450`](../crates/benilla-app/src/player/camera_dynamics.rs#L450))
- Both values clamp to the rig's range, because a hand-edited file should not land a pose play cannot reach; the reference stores them unbounded, the pitch until a mouse-look re-clamps it. ([`src/player/camera_saved.rs:64`](../crates/benilla-app/src/player/camera_saved.rs#L64))
- `ResetView` writes the CVars back to their defaults, because the CVar store is our persistence; the reference (`0x50fae0`) touches no CVar, so its reset reverts at next launch. ([`src/player/camera_view.rs:15`](../crates/benilla-app/src/player/camera_view.rs#L15))
- The starting zoom, `CAM_DIST_DEFAULT`, is 15 yd where the reference starts at 5.55, the third-person-A row of its default table `0x84f488` that `cameraView` 1 picks (`0x50c17a`, `0x50c18b`; the `cameraDistance` CVar itself registers "0.0"), for a wider view. ([`src/player/camera_zoom.rs:10`](../crates/benilla-app/src/player/camera_zoom.rs#L10))
- No drunk field-of-view lane, because the reference's is invisible in play: its target `(179° − 90°)·fraction` (camera `+0x10c`, setter `0x511250`) eases at `cameraFoVSmoothSpeed`, 0.5°/s, and restarts on every drunk-byte change, so it creeps under 1°. ([`src/player/drunk.rs:46`](../crates/benilla-app/src/player/drunk.rs#L46))
- Every end signals `END`, because the reference's three silent ends are a bug: two writers reset the follow mode cell `0xc4d888` directly (`0x5fb646`, `0x60396f`), leaving a stale guid at `0xc4d980`, and the canceller's early-out (`0x60fb70`) fires nothing. ([`src/player/follow.rs:24`](../crates/benilla-app/src/player/follow.rs#L24))
- A settle hold is summed and sent once on release, without the push, because it stands in for the reference's blocking load (`0x694920`), not a refused substep, and lasts seconds. ([`src/player/movement_net.rs:470`](../crates/benilla-app/src/player/movement_net.rs#L470))
- The reference's blocking load (`0x694920`) waits out its reads before the frame goes on and has no such release; without one, a model that never loads would freeze our body for good. ([`src/player/state.rs:120`](../crates/benilla-app/src/player/state.rs#L120))
- Root is not merged, because a bare pose unrooting us while the server still roots us streams motion into `CHEAT_TYPE_ROOT_MOVE`; vmangos keeps a rooted mover's root bit (`MovementHandler.cpp:1064`), so the reference's merge of it is a no-op anyway. ([`src/player/state.rs:202`](../crates/benilla-app/src/player/state.rs#L202))
- Applied at arrival, because our own avatar has no motion to de-jitter; the reference routes it through the replay chain remotes use, which paces to the sender's cadence and can hold an early packet (up to 1000 ms in `net::motion`). ([`src/player/wire_in.rs:503`](../crates/benilla-app/src/player/wire_in.rs#L503))

#### portrait

- The auction house's `AuctionDressUpModel` draws the same one assembly, because the app's `TryOn`/`Dress` intents are not per-widget; the reference's two widgets each clone their own model. ([`src/portrait/dressup.rs:8`](../crates/benilla-app/src/portrait/dressup.rs#L8))
- A fixed neutral front-lit studio (`studio_light`) replaces the portrait bake's hard-coded rig (`0x525b10`: ambient 0.45 grey, white directional), because the studio is the look chosen for them. ([`src/portrait/light.rs:65`](../crates/benilla-app/src/portrait/light.rs#L65))
- The round portraits bake at 256², not 64², because the 64² still is a resolution limit, not a look; and under benilla's studio light (`light::BoothLight`), the chosen portrait look, in place of the reference's own fixed rig (`0x525b10`, pushed at `0x52562e`: a 0.45 grey ambient and a white directional). ([`src/portrait/mod.rs:18`](../crates/benilla-app/src/portrait/mod.rs#L18))
- Only the four party slots hand over, capped at `Self::CAP`, instead of every portrait until `ClientDestroyGame` (`0x401ee0`), because a general cache would pay a texture per unit ever seen. ([`src/portrait/mod.rs:265`](../crates/benilla-app/src/portrait/mod.rs#L265))
- This reads the window's aspect, not `gxResolution`'s, because the window here takes any shape a drag gives it, where the reference's keeps `gxResolution`'s (`0x5989d7`). ([`src/portrait/mod.rs:694`](../crates/benilla-app/src/portrait/mod.rs#L694))
- `boothHalfRate` is benilla's own CVar, because the reference draws its doll in the main pass and has no second view; a full-rate pass costs up to 7.6 ms a frame at 4K. ([`src/portrait/mod.rs:733`](../crates/benilla-app/src/portrait/mod.rs#L733))
- The reference widget draws only its model; a body pane clears to near-black, the chosen look where nothing sits behind the pane. ([`src/portrait/mod.rs:990`](../crates/benilla-app/src/portrait/mod.rs#L990))

#### quest_markers

- A quest marker outranks the green, where the reference's two handlers race last-write-wins on the slot, because a fixed order is deterministic; the two meet only on a flight master with a live quest marker, invisible in practice. ([`src/quest_markers/mod.rs:124`](../crates/benilla-app/src/quest_markers/mod.rs#L124))
- A live V-plate (`VPlates`) raises the marker too, because the reference's marker sits low behind the plate: it arms anim 0 under a plate, whose name suppression nulls the `desc+0x8` handle `0x6c7950` tests. ([`src/quest_markers/mod.rs:397`](../crates/benilla-app/src/quest_markers/mod.rs#L397))
- We wait for propagation to settle the joint matrix, so the marker is always constant size. ([`src/quest_markers/mod.rs:464`](../crates/benilla-app/src/quest_markers/mod.rs#L464))

#### raid_marks

- No depth write, as for the names, because blended sorting looks the same. ([`src/raid_marks.rs:14`](../crates/benilla-app/src/raid_marks.rs#L14))

#### realmlist

- The value lives in `config.toml` with every other CVar and is edited from a login-screen control, because local state is one folder and one config file. ([`src/realmlist.rs:6`](../crates/benilla-app/src/realmlist.rs#L6))
- The local machine's server, not the reference's `us.logon.worldofwarcraft.com:3724`, because that host no longer resolves. ([`src/realmlist.rs:44`](../crates/benilla-app/src/realmlist.rs#L44))
- 64, where the reference's login boxes cap at 16 (`AccountLogin.xml`), because a hostname needs more. ([`src/realmlist.rs:48`](../crates/benilla-app/src/realmlist.rs#L48))
- A host name may carry a port, where the reference resolves a name whole (`0x43ba0e`) and takes a port only on a dotted address (`0x43f3d0`), because a server on a non-default logon port behind a host name is everyday use. ([`src/realmlist.rs:151`](../crates/benilla-app/src/realmlist.rs#L151))

#### screenshot

- The file is a PNG where the reference writes a TGA (`0x4426f0`, the name `%s\WoWScrnShot_%s.tga` at `0x834c44`), because a 32-bit Targa of a modern window is about 8 MB and few tools open it. ([`src/screenshot.rs:14`](../crates/benilla-app/src/screenshot.rs#L14))
- A failed write answers `SCREENSHOT_FAILED` and logs the reason (`report_captures`), where the reference fires `SCREENSHOT_SUCCEEDED`, because its capture returns 1 whenever it had a buffer and discards the write's result (`0x442792`, `0x4427a2`); announcing a shot that was never saved is a defect. ([`src/screenshot.rs:18`](../crates/benilla-app/src/screenshot.rs#L18))

#### shaders

- Bilinear filtering runs on the decoded texel, not on the gamma byte the reference filters, because the booth bake stores linear bytes. ([`src/shaders/ui_quad.wgsl:13`](../crates/benilla-app/src/shaders/ui_quad.wgsl#L13))

#### shutdown

- The reference leaves it to the OS. Its key arm (`0x42d261`-`0x42d276`) hands Alt+F4 to `DefWindowProc`, whose `SC_CLOSE` becomes the kind-5 event (`0x42d287`, as `WM_CLOSE` posts at `0x42d292`) and closes at once: the running client's callback `0x63c9a0` (installed from `0x402840`, asked at `0x423a21`) answers 0 and tears the context down (`0x41f9b0`), with no logout timer, at the login screen and in game alike. ([`src/shutdown.rs:50`](../crates/benilla-app/src/shutdown.rs#L50))

#### smart_rect

- The plate caller snaps the result to device pixels before `Self::claim`, so its border art blits 1:1. ([`src/smart_rect.rs:50`](../crates/benilla-app/src/smart_rect.rs#L50))

#### sound

- `damage > 0` without a parry or block stands in for `0x2`, because vmangos clears that bit only for immune and a zero-damage miss or absorb (`Unit.cpp:1366`/`1587`), so it would grunt on every parry. ([`src/sound/combat.rs:587`](../crates/benilla-app/src/sound/combat.rs#L587))
- The water column is chosen on liquid above the feet (`depth > 0`, the footstep splash's test), because the reference's test, the flag saying liquid was found at the unit's sampled position (`[node+0x90] & 0x20`, `0x670630`), has no depth and splashes a body on a lake's shore. ([`src/sound/death_thud.rs:28`](../crates/benilla-app/src/sound/death_thud.rs#L28))
- The reference marks an unresolvable kit by negating the id (`[+0xE00] = -id`), so the owner's next `$DSL` re-registers into a fresh entry every cycle; a flag keeps it in place without the churn. ([`src/sound/emitter_pool.rs:54`](../crates/benilla-app/src/sound/emitter_pool.rs#L54))
- The reference leaves those kits uncapped; sample-aligned copies of one file sum coherently (five are one sound +14 dB), so a mass buff distorts. ([`src/sound/kit.rs:240`](../crates/benilla-app/src/sound/kit.rs#L240))
- The reference never steals. Every WoW voice is an `FSOUND_Stream` at priority 256 (`fmod.dll 0x1002be47`) and FMOD's steal scan skips `>= 256` (`0x100268e9`), so its 13th sound is dropped. ([`src/sound/kit.rs:284`](../crates/benilla-app/src/sound/kit.rs#L284))
- The reference sums at full scale and clips (a close 1.0 kit reaches `FSOUND_SetVolume(255)` through `0x7a5dc0`, FMOD's MMX mixers saturate with `paddsw`, and no gain depends on the live voice count); this limits, because every WoW SFX is mastered to full scale, so overlapping kits clip into broadband distortion, as when a mass buff lands five sample-aligned copies of one 0 dBFS file. `SoundOutputLimiter 0` turns it off. ([`src/sound/limiter.rs:3`](../crates/benilla-app/src/sound/limiter.rs#L3))
- The mantissa-bit volume curve (`0x7a5d70`, `(v·255+512.5).bits >> 14 & 0xff`) and the `__ftol` truncations are dropped, because they only quantized to FMOD's integer levels and our backend takes float amplitudes. ([`src/sound/math.rs:4`](../crates/benilla-app/src/sound/math.rs#L4))
- The reference also refuses to open a new non-music stream while inactive (`0x7a52bf`); benilla starts everything and gates the output, because a refusal at kit start would make an unattended capture record silence. ([`src/sound/mod.rs:97`](../crates/benilla-app/src/sound/mod.rs#L97))
- Default off where the reference registers "1". Its reverb is FMOD 3's EAX API, which renders only with hardware 3D mixing, so off is what the reference is heard to produce (inferred: `fmod.dll`'s render side is untraced). ([`src/sound/mod.rs:105`](../crates/benilla-app/src/sound/mod.rs#L105))
- `SoundOutputLimiter`, not a 1.12 CVar, default on, so overlapping full-scale kits are limited rather than clipped. ([`src/sound/mod.rs:123`](../crates/benilla-app/src/sound/mod.rs#L123))
- With no character the reference leaves the listener where it was; benilla seats it on the camera, where the view is. ([`src/sound/mod.rs:488`](../crates/benilla-app/src/sound/mod.rs#L488))
- The reference also plays an optimistic coin at the loot-money click, and at the auto-loot sweep's money, once per loot source (`0x4c201b`-`0x4c2020`, the same `0x458030` call; `0xb71bc4` set at `0x4c221e`, reset at `0x4c1d95`), so looted money sounds twice; benilla plays the watcher's coin alone, because one coin on the confirmed change reads cleanly and the round trip the early coin hides is imperceptible on a local server. ([`src/sound/money.rs:7`](../crates/benilla-app/src/sound/money.rs#L7))
- 1.12.1 sets FMOD up on driver 0 and, when that fails, runs silent for the session (`Sound::Initialize` `0x7a4330`, the driver −1 → 0 at `0x7a445f`); here another output stands in, because on Linux the default is often a stock `dmix` whose card a sound server holds. ([`src/sound/output/cpal.rs:193`](../crates/benilla-app/src/sound/output/cpal.rs#L193))
- A change of the default output device is followed (here and in `coreaudio.rs`), where 1.12.1 initialises its sound device once and never again (`Sound::Initialize` `0x7a4330` refuses a second call; the driver-enumeration functions `0x7a6320`-`0x7a6380` have no caller), because a headset plugged in mid-session should play at once. ([`src/sound/output/cpal.rs:544`](../crates/benilla-app/src/sound/output/cpal.rs#L544))

#### spell

- An out-of-range byte drops the packet, because the reference's unchecked write overruns the table (`(64, 19)` lands on the class family at `0xcecaac`). ([`src/spell/mods.rs:131`](../crates/benilla-app/src/spell/mods.rs#L131))
- The fallback is the packet's target guid, started only when the validator keeps it (`on_spell_go`), and with none there is no start, never the nearest-hostile acquire, because that would attack something the player never named. ([`src/spell/net.rs:1479`](../crates/benilla-app/src/spell/net.rs#L1479))
- A range failure leaves targeting armed and working. The reference's report cancels the cast (`0x6e2218` → `0x6e4940`): its line, `SPELLCAST_FAILED` and the tradeskill disarm, which benilla keeps; with the word the bind emptied it also sends `CMSG_CANCEL_CAST` for the unsent spell (`0x6e4970`) and clears the caster, then restores the pre-click word (`0x6e6096`), so every later click fails the caster test (`0x6e5b93`) and the cursor is stranded until a cancel. ([`src/spell/targeting/mod.rs:661`](../crates/benilla-app/src/spell/targeting/mod.rs#L661))

#### target

- The reference's arm reads the last object's stop (`0x611336` before `0x610383` stores this one's), so its first walk has no stop. ([`src/target/click.rs:723`](../crates/benilla-app/src/target/click.rs#L723))
- It runs on the first frame with none, so an arrival that sends no stop (mid-air, rooted, under a held turn key) acts at once or on landing or release, where the reference waits for the next ground stop and can act long after, wherever the player then is. ([`src/target/click.rs:1670`](../crates/benilla-app/src/target/click.rs#L1670))
- The rank is held to 7, the top of the ring scale's eight entries (`0..=7`), where the reference keeps any value; a server rank past 7 would index outside our palette. ([`src/target/relations.rs:487`](../crates/benilla-app/src/target/relations.rs#L487))
- TAB uses Classic Era's screen-space priority, not 1.12's ±30° facing cone and snapshot cursor (`0x493f60`), because the cone skips a close mob in front of the camera, which sits behind the character. ([`src/target/scan.rs:3`](../crates/benilla-app/src/target/scan.rs#L3))
- Our own body is never a candidate. The reference's friendly scan picks the player at some facings (`CanAssist(P, P)` is true, `0x6061e0`; the cone test `0x47f220`), but our scoring always puts the player dead centre at distance 0, so CTRL-TAB would pick us on every press. ([`src/target/scan.rs:194`](../crates/benilla-app/src/target/scan.rs#L194))
- No drawn gate (the reference's IsEntityDrawn, `0x6704c0`), because the draw election hides every body outside the frustum, which would empty the off-screen tier. ([`src/target/scan.rs:374`](../crates/benilla-app/src/target/scan.rs#L374))
- Sampled each frame, not stamped in `SetSelection`, so every selection writer is covered; a selected unit that turns hostile is remembered, where the reference's is not. ([`src/target/scan.rs:1071`](../crates/benilla-app/src/target/scan.rs#L1071))

#### textinput

- macOS takes the Cocoa text-field chords for Cmd and Option, because editing follows each OS's own text fields. ([`src/textinput/keymap.rs:5`](../crates/benilla-app/src/textinput/keymap.rs#L5))

#### ui_action

- The player's `0x5e` with no word falls back to the spell's own `RequiresSpellFocus`, which the reference never reads here, because it is the id vmangos sends (`Spell.cpp:4426`). `0x5d` has no such stand-in: vmangos takes its area from its own `spell_area` table (`Spell.cpp:4429`), which `Spell.dbc` does not carry. ([`src/ui_action/cast_fail.rs:311`](../crates/benilla-app/src/ui_action/cast_fail.rs#L311))
- A reason past the table prints its hex code, so an unknown reason is visible. ([`src/ui_action/cast_fail.rs:415`](../crates/benilla-app/src/ui_action/cast_fail.rs#L415))
- A declined arm's template loses its tokens ("Missing reagent: %s" reads "Missing reagent") where the reference shows the literal `%s`, because a raw token reads as broken. ([`src/ui_action/cast_fail.rs:534`](../crates/benilla-app/src/ui_action/cast_fail.rs#L534))
- An empty chat line is dropped, where the reference adds a blank one (`0x49a870` refuses only a null text, `0x49a881`), because a blank line is worse than none; only a string an addon emptied, or a chain missing a stock key, reaches it. ([`src/ui_action/errors.rs:588`](../crates/benilla-app/src/ui_action/errors.rs#L588))
- The book's and the bar's login packets run this over every staged spell word, where the reference re-points slots only at `SMSG_SUPERCEDED_SPELL`, old rank to new (`0x4e5e60`, `super::spell_slots`), because vmangos never rewrites stored slots on a rank-up (`Player::AddSpell`), drops superseded ranks from the book (`Player.cpp:3463`) and sends no supersede while loading (`Player.cpp:3704`), so a stored slot can name a spell the character no longer knows. ([`src/ui_action/ranks.rs:3`](../crates/benilla-app/src/ui_action/ranks.rs#L3))

#### ui_aura

- On a catalog miss the reference keeps the record's stale value (`0x4e4533`, `0x4e4550` skip the write); this answers from the joined expiry, because a stale value is no mechanism to reproduce. `None` is that miss: the call reads the expiry (`AuraState::until_cancelled_unknown`). ([`src/ui_aura.rs:603`](../crates/benilla-app/src/ui_aura.rs#L603))
- On other units' rows an id the catalog lacks stays visible, because every wire id is a real spell and failing closed would blank every aura on a catalog failure; the reference skips one there (`[0xc0d78c]`), though its player cache inserts one, as here. ([`src/ui_aura.rs:631`](../crates/benilla-app/src/ui_aura.rs#L631))
- The reference clears its expiry array (`0xbc5f68`) at every game entry, in `ResetBuffCache` (`0x4e40c0`, called from `0x48f5a9`); benilla clears it at exit instead. ([`src/ui_aura.rs:1011`](../crates/benilla-app/src/ui_aura.rs#L1011))

#### ui_binder

- The reference never clears its latched guid (`0xc4d7c0`), so a second `ConfirmBinder()` in range re-sends the activate; this drops the guid on `SMSG_PLAYERBOUND`, because an answered question is not a question and the reference's dialog is gone by then. ([`src/ui_binder.rs:19`](../crates/benilla-app/src/ui_binder.rs#L19))

#### ui_chat

- Not run on benilla's same-character reconnect, which the reference lacks (it returns to the login screen), so the window keeps its scrollback there. ([`src/ui_chat.rs:266`](../crates/benilla-app/src/ui_chat.rs#L266))
- A missing global prints nothing, where the reference prints an empty line (`0x703c02`), because benilla's other `DisplayError` routes do the same. ([`src/ui_chat/broadcast.rs:167`](../crates/benilla-app/src/ui_chat/broadcast.rs#L167))
- In a capital with a blank city word the walk composes `"Trade - "` (it tests the row pointer, not the string, `0x49a2e4`-`0x49a2ed`); we decline, because a wrong channel name is joined silently and for good. ([`src/ui_chat/channels.rs:157`](../crates/benilla-app/src/ui_chat/channels.rs#L157))
- The reference holds every line that waits on its `$`-macro subject's name in the name cache's callback list (`0x49daea` → `0x55f080`) until the name answers, with no bound; here at most `MAX_HELD_SUBJECT_LINES` wait, the oldest dropped past that, as a subject no player owns never answers. ([`src/ui_chat/feed.rs:391`](../crates/benilla-app/src/ui_chat/feed.rs#L391))
- A probe's bare `/assist` assists the live selection, `AssistByName("")` (`0x489c89`), where the stock handler (`ChatFrame.lua:740`) resolves a selected player's name through `AssistByName`, because a name resolve can land on a different player. ([`src/ui_chat/input/mod.rs:594`](../crates/benilla-app/src/ui_chat/input/mod.rs#L594))
- A line of 256 bytes or more, which reaches here only through `chat_send`'s deviation for `0x49f604`, is slurred whole instead of cut to that buffer, so it reaches the server whole for the refusal that deviation leaves to it. ([`src/ui_chat/input/send.rs:305`](../crates/benilla-app/src/ui_chat/input/send.rs#L305))
- The files live in `benilla-config/Logs/`, not the install's `Logs`, because the install is read-only. ([`src/ui_chat/logging.rs:14`](../crates/benilla-app/src/ui_chat/logging.rs#L14))
- No re-run when a remote player streams in, because the reference's re-run only repeats the join, which vmangos swallows, or the leave, whose "Not on channel" notice nothing prints. ([`src/ui_chat/recruitment.rs:10`](../crates/benilla-app/src/ui_chat/recruitment.rs#L10))
- `set_chat_window_looks` drops an out-of-range window, where the reference's `WINDOW` bound is off by one (`0x498d17`, `ja` where the array needs `jae`), because that write would land past the array. ([`src/ui_chat/settings.rs:31`](../crates/benilla-app/src/ui_chat/settings.rs#L31))
- The debounce also writes Lua's changes (`dirty`) after the quiet time, where the reference writes only at teardown, so a crash loses at most a second of changes. ([`src/ui_chat/settings.rs:627`](../crates/benilla-app/src/ui_chat/settings.rs#L627))

#### ui_dressup

- The base is the player's melee pair, held drawn, because the booth shares the select screen's assembly; the reference clones the live model with its attachments, drawn or sheathed (`0x5059a0`, `0x70ea00`). ([`src/ui_dressup.rs:44`](../crates/benilla-app/src/ui_dressup.rs#L44))

#### ui_duel

- The challenger's name comes from the `NameCache`, so a name still being queried delays the popup to the name's answer rather than dropping it; the reference reads it off the streamed player and fires nothing without one (`0x4d4a72`). ([`src/ui_duel.rs:8`](../crates/benilla-app/src/ui_duel.rs#L8))
- Zero arms nothing, where the reference prints 0 and re-arms forever on the `dec`'s wraparound, a bug vmangos never reaches (it sends 3000). ([`src/ui_duel.rs:95`](../crates/benilla-app/src/ui_duel.rs#L95))

#### ui_gamma

- The reference skips that upload when windowed (`byte[dev+0x20b]` is `gxWindow`), and every benilla mode is windowed, so the same curve applies in the compositor, where the slider can move pixels: `UiGammaNode` raises the gamma byte the RAMDAC would have read to `gamma` before its decode, the continuous form of the 256-entry ramp. ([`src/ui_gamma.rs:56`](../crates/benilla-app/src/ui_gamma.rs#L56))
- The reference never clamps (`SetGamma` `0x4891f0` writes `-4.000000` for 5), but its ramp is a fullscreen OS call and ours is the image, where an extreme exponent blacks out the panel that would undo it. ([`src/ui_gamma.rs:66`](../crates/benilla-app/src/ui_gamma.rs#L66))

#### ui_gm_ticket

- Those three are stack residue, so benilla sends 0 for each. ([`src/ui_gm_ticket.rs:169`](../crates/benilla-app/src/ui_gm_ticket.rs#L169))

#### ui_gossip

- 14 (a NULL, pushed as `nil`) and 15 up (a read past the table's end) give `"gossip"`, because the reference's behaviour there is a missing bounds check. ([`src/ui_gossip/mod.rs:252`](../crates/benilla-app/src/ui_gossip/mod.rs#L252))

#### ui_guild

- A script's 16-letter name, which the reference's bindings pass (1 to 16 code points, `0x4d20d0`, `0x4d2210`), is dropped rather than sent or truncated, because vmangos would kick the session. ([`src/ui_guild/feed.rs:72`](../crates/benilla-app/src/ui_guild/feed.rs#L72))
- Bounded at the ten slots, because the reference's read (`0x4c93e7`) runs past its array. ([`src/ui_guild/mod.rs:87`](../crates/benilla-app/src/ui_guild/mod.rs#L87))
- Clamped to ten, because the reference's unbounded loop (`0x4d0bb0`) overruns its ten-slot array from a hostile `rankCount` of 11: index 10 is `0xb726f8`, a float its initializer writes (`0x4d279c`). ([`src/ui_guild/mod.rs:562`](../crates/benilla-app/src/ui_guild/mod.rs#L562))
- An exact tie is `Equal` where the reference answers ±1, and a NaN has a place (`total_cmp`), because Rust's sort requires a total order. ([`src/ui_guild/sort.rs:72`](../crates/benilla-app/src/ui_guild/sort.rs#L72))
- The reference ties a miss with a hit (`0x4d0e27`, `0x4d0ea1`), no total order. ([`src/ui_guild/sort.rs:89`](../crates/benilla-app/src/ui_guild/sort.rs#L89))

#### ui_honor

- A new target zeroes the figures too, and dropping a held reply signals `INSPECT_HONOR_UPDATE`, where `0x4c6f70` keeps the figures and clears only has-data (`0xb71fc8`) and pending (`0xb71fcc`): a server that never answers the ask, as vmangos often does not, would leave the last player's figures shown under the new one. ([`src/ui_honor.rs:38`](../crates/benilla-app/src/ui_honor.rs#L38))

#### ui_instance

- Walking out of a party dungeon ourselves also counts as holding a bind (`InstanceState::saw_own_dungeon`), because vmangos reports only permanent binds (`Player.cpp:16175`) and makes only raid binds permanent (`Map.cpp:3528`), so the server alone would never offer the reset after a 5-man. ([`src/ui_instance.rs:7`](../crates/benilla-app/src/ui_instance.rs#L7))
- The app's monotonic clock, not `time(0)`, because it cannot run backwards; they differ only across a suspend. ([`src/ui_instance.rs:35`](../crates/benilla-app/src/ui_instance.rs#L35))
- Reason 3 and up prints nothing, where the reference prints an uninitialized 2 KB stack buffer, because that is a bug and vmangos means 3 and up as silent (`MapPersistentStateMgr.h:275`). ([`src/ui_instance.rs:352`](../crates/benilla-app/src/ui_instance.rs#L352))
- Flag 2 and up prints nothing, where the reference prints an uninitialized 2 KB stack buffer, because that is a bug, not a mechanism. ([`src/ui_instance.rs:396`](../crates/benilla-app/src/ui_instance.rs#L396))

#### ui_item_text

- For a reader the latch already holds, the reference's open again re-registers it (`0x4e349a` → `0x4930d0` → `0x49310a` → `0x493310` → `0x49338e` `0x4e32e0(0, 0)`), which resets the reader and signals `ITEM_TEXT_CLOSED` (`0x4e339d`); the `ITEM_TEXT_BEGIN` after it never shows the frame again, and the show finds the cell cleared, so `ITEM_TEXT_READY` never fires (`0x4e35de`). ([`src/ui_item_text.rs:493`](../crates/benilla-app/src/ui_item_text.rs#L493))

#### ui_layout

- A debounced autosave (`save_layout`) also writes a quiet second after a drag, because the reference has no crash-path write and a crash would lose the session's windows. ([`src/ui_layout.rs:13`](../crates/benilla-app/src/ui_layout.rs#L13))
- A record's fields start empty. The reference's parser clears X, Y, W and the level at each `Frame:` (`ebx`, `edi`, `esi`, `[ebp-0x4]` at `0x49045b`–`0x490469`) but not H (`[ebp-0xc]`), so a record with no `H:` takes the one before's; benilla writes `H:` whenever it writes `W:`, so only a hand-edited file reaches the slip, which would size a window from another window's row. ([`src/ui_layout.rs:89`](../crates/benilla-app/src/ui_layout.rs#L89))

#### ui_loot

- A negative answer releases the row, where the reference's callback `0x4c2ac0` returns on a failed query and the window never opens, because an entry the server cannot describe would otherwise keep the loot shut for good. ([`src/ui_loot/mod.rs:243`](../crates/benilla-app/src/ui_loot/mod.rs#L243))
- It is the display row, where the reference's is the item index plus one (`0x4c2885`), so an addon's `GetLootSlotInfo(arg1)` reads the row the dialog is for. ([`src/ui_loot/mod.rs:330`](../crates/benilla-app/src/ui_loot/mod.rs#L330))
- A row cleared meanwhile stops counting, where `ClearLootRow` (`0x4c1f00`) cancels its callback (`0x4c1f52`) and never lowers the count (its writers are `0x4c1def`, `0x4c1e94` and `0x4c2af6` alone), so in 1.12.1 a group member's take of an uncached item before the answer leaves that corpse's window shut until the next loot response resets the count (`0x4c1def`), and ours opens. ([`src/ui_loot/mod.rs:842`](../crates/benilla-app/src/ui_loot/mod.rs#L842))
- `autoLootDefault` is benilla's own option, 1.12 auto-looting only on a shift-click, and off is 1.12's behaviour. ([`src/ui_loot/mod.rs:872`](../crates/benilla-app/src/ui_loot/mod.rs#L872))
- Our close paths clear it guid-matched, where `CloseInteraction` clears it unconditionally (`0x48f2c9`), so closing an old window keeps a newer `CMSG_LOOT`'s latch. ([`src/ui_loot/mod.rs:900`](../crates/benilla-app/src/ui_loot/mod.rs#L900))

#### ui_loot_roll

- A negative answer, or none within `TEMPLATE_HOLD_MAX_MS`, opens the frame unresolved, because a frame that never opens cannot even be passed on; the reference's callback fires on success only. ([`src/ui_loot_roll.rs:11`](../crates/benilla-app/src/ui_loot_roll.rs#L11))
- The open count is the number of rolls announced and not closed. The reference keeps a counter that its server-close paths decrement for any roll whose closed byte is clear (`0x61b6f8`, `0x61bc17`), a deferred roll that never counted up included, so it under-counts and lets a fifth frame open; here a deferred roll's close changes nothing. ([`src/ui_loot_roll.rs:22`](../crates/benilla-app/src/ui_loot_roll.rs#L22))
- A refused record or name closes the roll unspoken. In 1.12.1 the failure callbacks (`0x61bc80`, `0x61bd50`, `0x61b7d0`) free the record with no `CANCEL_LOOT_ROLL` and leave the open count unlowered, so its frame stays up, empty and dead, until a `/reload`, and holds one of the four frames, so a later fourth roll never shows; benilla closes the roll instead. ([`src/ui_loot_roll.rs:29`](../crates/benilla-app/src/ui_loot_roll.rs#L29))

#### ui_macro

- The reference's 255-byte cut (`0x4f0677`, `0x4f0b93`) can end inside a character and keep its stray bytes; a `String` must stay UTF-8, so the cut backs off to the character's start. ([`src/ui_macro/store.rs:21`](../crates/benilla-app/src/ui_macro/store.rs#L21))

#### ui_mail

- With no usable paper the reference reads the first record of an empty list through a null pointer (`0x4ad561`-`0x4ad56d`, `mov eax,[eax]` with `eax` 0) and faults; benilla keeps the row's wire id, so the inbox still opens. ([`src/ui_mail/net.rs:232`](../crates/benilla-app/src/ui_mail/net.rs#L232))
- An open mailbox also re-lists at once, past `CheckInbox`'s 60 s throttle, because a mail announced while the player stands there should show; the reference waits for the close. ([`src/ui_mail/net.rs:388`](../crates/benilla-app/src/ui_mail/net.rs#L388))

#### ui_merchant

- The row is matched by item entry, where the reference matches its vendor slot word (`+0x00`), because vmangos writes the entry in that field (`Player.cpp:11718`). ([`src/ui_merchant/net.rs:99`](../crates/benilla-app/src/ui_merchant/net.rs#L99))

#### ui_mirror

- A type with no bar is dropped, because no bar appears either way: the reference fires it as `"UNKNOWN"` (`0x5e7ae0`) and `MirrorTimer_Show` errors on its nil colour before `Show()`. vmangos's `NUM_CLIENT_TIMERS` keeps its fourth timer off the wire. ([`src/ui_mirror.rs:145`](../crates/benilla-app/src/ui_mirror.rs#L145))

#### ui_models

- The reference's paint (`0x76d240`) reads the eye before it rebuilds the root, so a facing change draws one frame late; not reproduced, because it is an ordering quirk. ([`src/ui_models/mod.rs:1468`](../crates/benilla-app/src/ui_models/mod.rs#L1468))

#### ui_pet

- It counts characters where the reference counts bytes, because a byte cut can split a UTF-8 character. ([`src/ui_pet/steps.rs:147`](../crates/benilla-app/src/ui_pet/steps.rs#L147))

#### ui_pet_stats

- Only when the bucket or its two tooltip figures change, not on every change of the field as in the reference, because no stock consumer reads the raw value. ([`src/ui_pet_stats.rs:58`](../crates/benilla-app/src/ui_pet_stats.rs#L58))

#### ui_quest

- The row one past a full pool is read from the cells after it (`0xbe07e0` and `0xbe0828` for the available pool, `0xbdf010` for the active one), which benilla does not copy, so that select sends nothing and only sets the latch the send would. ([`src/ui_quest/verbs.rs:115`](../crates/benilla-app/src/ui_quest/verbs.rs#L115))

#### ui_quest_share

- A member's name not yet cached holds its verdict line for a bounded retry, shown at the name's answer within `NAME_WAIT_FRAMES` and dropped after, and holds the escort confirm with no bound until the name answers or another confirm replaces it, where the reference reads its name cache (`0xc0e228` via `0x55f080`) with a null callback and drops the message on a miss, because benilla's cache may not hold a party member yet. ([`src/ui_quest_share.rs:11`](../crates/benilla-app/src/ui_quest_share.rs#L11))

#### ui_saved

- The write is atomic (`crate::local_state::write_atomic_bytes`), because the reference's `.bak` rotation (`MoveFileW` without `MOVEFILE_REPLACE_EXISTING`, its result discarded) protects only the first write, and nothing reads the `.bak`. ([`src/ui_saved.rs:9`](../crates/benilla-app/src/ui_saved.rs#L9))
- A header saying what the file is, because the folder is in plain view; the reference's file opens with a bare blank line. ([`src/ui_saved.rs:136`](../crates/benilla-app/src/ui_saved.rs#L136))

#### ui_script

- The reference reads the install's `Interface\AddOns\`; benilla reads an install and does not live in one, so its addons live in its own state folder, and only there. ([`src/ui_script/addons.rs:265`](../crates/benilla-app/src/ui_script/addons.rs#L265))
- That file is renamed to `<Name>.lua.bak`, where the reference deletes it, so an addon update that drops its toc line costs the player nothing they cannot get back. ([`src/ui_script/addons.rs:684`](../crates/benilla-app/src/ui_script/addons.rs#L684))
- The reference writes none (its file opens with a blank line); ours names the file, because players open this folder. ([`src/ui_script/addons.rs:742`](../crates/benilla-app/src/ui_script/addons.rs#L742))
- The reference then re-reads the aspect (`0x492f13`), and on a change of 0.001 or more calls `0x491380`, which sets the reload trigger (`[0xb4b3f4]`) when the script-context check `0x494a50(0xa)` allows it; here a resize re-lays out the live UI, the chat bubbles' window-derived `BubbleGeometry` with it, because the window resizes freely and a reload on each resize would tear down every addon's state mid-play. ([`src/ui_script/extract/mod.rs:543`](../crates/benilla-app/src/ui_script/extract/mod.rs#L543))
- The V-plate border is resampled sharp to the quad's physical size rather than GPU-magnified, so its 1 px bevel stays crisp; keyed by that size, so only a resize re-rasterises it. ([`src/ui_script/extract/mod.rs:1463`](../crates/benilla-app/src/ui_script/extract/mod.rs#L1463))
- No signature or digest check (`0x46a90b`, `0x46a9d3`, `0x401560(9)`), so the chain's toc always loads and `FRAMES_LOADED` follows, because the check verifies Blizzard's signature with a key carried in the client binary, which benilla does not ship, and a modified interface file is loaded rather than refused. ([`src/ui_script/glue.rs:14`](../crates/benilla-app/src/ui_script/glue.rs#L14))
- The reference's size callback only re-scales `GlueParent` and re-lays it out (`0x46ac51`, `0x46ac56`), as its root keeps `gxResolution`'s aspect until a rebuild (`0x41ad10` at `0x46a828`) and its window keeps that shape while dragged (`0x5989d7`); here the root is the window's, which takes any shape, so the box follows it on every resize, where a rebuild would reset the screen (the create screen's character and name, the typed password, the selection). ([`src/ui_script/glue.rs:346`](../crates/benilla-app/src/ui_script/glue.rs#L346))
- The reference's handler completes whichever operation is current; here only the character list's does, in state 3, and a reply of another cause (a roster nothing of the screen asked for) or of another link (before a realm change) completes nothing. ([`src/ui_script/glue_mgr.rs:1520`](../crates/benilla-app/src/ui_script/glue_mgr.rs#L1520))
- The character is the platform's Unicode text, so a player can type any language on any platform. ([`src/ui_script/input.rs:763`](../crates/benilla-app/src/ui_script/input.rs#L763))
- A capture run (`scenario_active`) keeps the CRT's default seed, 1, so its frames reproduce. ([`src/ui_script/lifecycle.rs:179`](../crates/benilla-app/src/ui_script/lifecycle.rs#L179))
- The files are under `benilla-config/Logs/`, since the install is read-only. ([`src/ui_script/load_log.rs:5`](../crates/benilla-app/src/ui_script/load_log.rs#L5))
- `FULLSCREEN_QUADS_RESEAT` re-sizes the full-screen quads, which the stock files size only in their OnLoads, because a resize here keeps the loaded UI and the blackout would stop short of the new screen. ([`src/ui_script/manifest.rs:64`](../crates/benilla-app/src/ui_script/manifest.rs#L64))
- No `FrameXML.toc` signature check, where `UI_Init` loads on a failed one and then quits (`0x401560(0xa)` at `0x48ff90`), because the check verifies Blizzard's signature with a key carried in the client binary, which benilla does not ship, and a modified interface file is loaded rather than refused. ([`src/ui_script/manifest.rs:161`](../crates/benilla-app/src/ui_script/manifest.rs#L161))
- A flat 0.9, because 1.0 reads oversized. With `useUiScale` off, its default (`0x8430c0`), the reference sets `max(768/H, 0.9)` above 768 px tall and 1.0 at or below (`0x492f70`, on a mode set and from `0x4908ad`), so the two agree from ~853 px up. benilla never reads `useUiScale`: this CVar always applies to `UIParent` alone, as with it on. ([`src/ui_script/mod.rs:275`](../crates/benilla-app/src/ui_script/mod.rs#L275))
- The body is read with 0x4000-byte reads filled to the end of the body, so what a read returns does not depend on how the network cut the packets; and the reading stops once the buffer is full, since no later byte can change it. ([`src/ui_script/server_alert.rs:23`](../crates/benilla-app/src/ui_script/server_alert.rs#L23))
- The connection is direct. The reference opens its session with access type 0 (`InternetOpenA`, `0x7a6d97`), the system's proxy settings, which benilla does not read. ([`src/ui_script/server_alert.rs:26`](../crates/benilla-app/src/ui_script/server_alert.rs#L26))
- On a worldport it fires with our descriptor present, so `UnitExists("player")` answers true where the reference's (`0x515970`) misses, because matching it breaks `UnitExists` for no stock reader. ([`src/ui_script/sweep.rs:154`](../crates/benilla-app/src/ui_script/sweep.rs#L154))

#### ui_social

- After a full 50 the reference reads the 160 bytes past the array (`0xc2a0f8`, the counts at `0xc2a120` among them); benilla answers the miss. ([`src/ui_social/live.rs:72`](../crates/benilla-app/src/ui_social/live.rs#L72))
- A filter of 127 bytes or more puts the copy's NUL in the name buffer's first byte (`0x5aec1a`), so a name an earlier tag set is scanned as more filter. benilla ends the copy at its own NUL: the run-on reads one stack buffer through another. ([`src/ui_social/query.rs:76`](../crates/benilla-app/src/ui_social/query.rs#L76))
- `C1_SPACE` from `GetStringTypeW` is the Windows system's own table, which no file of the client holds; benilla answers Unicode's `White_Space` for those units, and nothing for a lone surrogate. ([`src/ui_social/query.rs:164`](../crates/benilla-app/src/ui_social/query.rs#L164))

#### ui_summon

- The latch is dropped at session end and an Accept with nothing latched sends nothing, so no stale or zero guid reaches the server. ([`src/ui_summon.rs:18`](../crates/benilla-app/src/ui_summon.rs#L18))

#### ui_talent_wipe

- Vmangos answers a reset with nothing to reset by asking again with a zero guid (`SkillHandler.cpp:52`), which the 1.12 client takes as its Accept and answers, looping; this drops a zero-guid question, because it means there is nothing to ask. ([`src/ui_talent_wipe.rs:17`](../crates/benilla-app/src/ui_talent_wipe.rs#L17))
- The reference never clears its latch (`0xc4d7a0`); this does once the answer is sent, because the reference's dialog is gone by then and a second Accept would reset twice. ([`src/ui_talent_wipe.rs:103`](../crates/benilla-app/src/ui_talent_wipe.rs#L103))

#### ui_text

- Text the reference magnifies from a smaller raster is rasterized at its true size here: a `SetTextHeight` size (the bit cleared at `0x771600`), an em past 32 px, and 3-D unit names (`UNIT_NAME_FONT`, em 32, string flag `0x80`), because the 32 px cap is a raster-memory budget that blurs big text on a modern display (`super::FONTSTRING_EM_CAP`). ([`src/ui_text/engine/mod.rs:8`](../crates/benilla-app/src/ui_text/engine/mod.rs#L8))
- The reference flushes every glyph cache on a window resize (`0x5c2b50` → `0x5ca6f0`); this keeps the old cells, because nameplate meshes bake UVs and a flush would rebuild them every resize frame. ([`src/ui_text/engine/mod.rs:13`](../crates/benilla-app/src/ui_text/engine/mod.rs#L13))
- The reference's measure kernels (`0x5c6940`, `0x5c6b70`, `0x5c6c50`, `0x5c7300`, `0x5c7470`) rasterize each character, but the step law reads only the advance, so skipping the bitmap changes no width. ([`src/ui_text/engine/mod.rs:362`](../crates/benilla-app/src/ui_text/engine/mod.rs#L362))
- The reference evicts the least recently used glyph and repacks its hole (`0x5cad2b`, over `[CGxFont+0x64..0x6c]`); this drops every cell, because a shelf allocator cannot reclaim an interior cell and the sheet is far larger than the reference's 256×256 pages. ([`src/ui_text/engine/mod.rs:652`](../crates/benilla-app/src/ui_text/engine/mod.rs#L652))
- Every UI text block sits 1 device px below the reference's snapped row, because at that row UI text reads high. ([`src/ui_text/layout/mod.rs:119`](../crates/benilla-app/src/ui_text/layout/mod.rs#L119))
- The cap is 32 logical units, not 32 device px, because the reference's raster-memory ceiling draws all big text at the same 32 device px on a modern display; at the 768-tall design height the two agree. ([`src/ui_text/mod.rs:54`](../crates/benilla-app/src/ui_text/mod.rs#L54))
- `r × floor(dpi)` passes, so the ring keeps its logical weight on a whole-number high-DPI raster; a fractional scale such as 1.5 rounds down to the reference's one texel a pass. ([`src/ui_text/outline.rs:60`](../crates/benilla-app/src/ui_text/outline.rs#L60))
- `r × floor(dpi)` passes as in `outlined_cell`, the first `floor(dpi)` of them the opaque ring. ([`src/ui_text/outline.rs:168`](../crates/benilla-app/src/ui_text/outline.rs#L168))
- One sheet, where the reference keeps up to 8 pages per font (`[CGxFont+0x18c]`), because a nameplate bakes its glyphs into one mesh with one material, which a page would split. ([`src/ui_text/pack.rs:11`](../crates/benilla-app/src/ui_text/pack.rs#L11))

#### ui_tooltip

- A pet view with no pet unit costs 0, an empty cell, where `0x6e3233` returns -1 and the unsigned divide (`0x52e8dd`) prints it as "429496729 Rage", or "4294967 Happiness" (divisors 10 and 1000 at `0x86f978`); benilla shows no such garbage cost. ([`src/ui_tooltip/spell_view.rs:99`](../crates/benilla-app/src/ui_tooltip/spell_view.rs#L99))

#### ui_trade

- An out-of-range slot is dropped, where the reference indexes its 7-slot mirror unchecked, because an out-of-bounds write is not a behaviour to reproduce. ([`src/ui_trade/mod.rs:411`](../crates/benilla-app/src/ui_trade/mod.rs#L411))

#### ui_trainer

- Benilla hides every state from 3 up. The reference shows states 32-34, 64-66 … 224-226 and hides the gaps, because x86 `shl` uses its count's low 5 bits and only filter bits 0-2 are set (`0x4da390`); its group counter `[hdr+4+state*4]` (`0x4d7854`) runs past the header. ([`src/ui_trainer/law.rs:301`](../crates/benilla-app/src/ui_trainer/law.rs#L301))

#### video

- `gxWindow "0"` is borderless fullscreen, not 1.12's exclusive mode-set at `gxResolution`, because Wayland has no client mode-setting (winit ignores `Fullscreen::Exclusive` there), and X11's XRandR changes the desktop's mode and a crash leaves it changed. ([`src/video.rs:5`](../crates/benilla-app/src/video.rs#L5))
- On Windows, NVIDIA's Vulkan driver (610.88 seen) crashes inside `vkQueuePresentKHR` on the first present after any swapchain resize while the present mode is `Fifo`, `FifoRelaxed` or `AutoVsync` (a maximize is a resize). ([`src/video.rs:17`](../crates/benilla-app/src/video.rs#L17))
- Sizes are logical px, because the windowed `gxResolution` is a logical inner size here. ([`src/video.rs:619`](../crates/benilla-app/src/video.rs#L619))

#### vplates

- The basis is damped past `PLATE_DIAG_KNEE`, because the reference's unbounded growth (`0x7705b0`) draws plates too large and soft at high resolutions. ([`src/vplates.rs:14`](../crates/benilla-app/src/vplates.rs#L14))
- The bar fill brightens by `LIT_BOOST` in place of the reference's ADD `Nameplate-Glow` rim, which reads as hard edge lines under benilla's linear blending. ([`src/vplates.rs:18`](../crates/benilla-app/src/vplates.rs#L18))
- `LEVEL_H` is 0.0086, one em under the reference's 0.009, because the level number read too large. ([`src/vplates.rs:118`](../crates/benilla-app/src/vplates.rs#L118))
- The reference dims non-target plates to `0x7F`; raised because that fades them too far to read. ([`src/vplates.rs:134`](../crates/benilla-app/src/vplates.rs#L134))
- The reference draws at fractional device coordinates; the snap keeps the magnified border crisp. ([`src/vplates.rs:193`](../crates/benilla-app/src/vplates.rs#L193))
- The reference's plates grow with the diagonal without limit (`0x7705b0`), which reads too big and soft; past this diagonal, where the frame is the border art's native 128x32 px, the basis grows at `PLATE_GROWTH_DAMP` of that rate. ([`src/vplates.rs:199`](../crates/benilla-app/src/vplates.rs#L199))
- A constant one-logical-pixel shadow, not the reference's 0.001 gx, which draws 2 px thick and too heavy at larger windows. ([`src/vplates.rs:670`](../crates/benilla-app/src/vplates.rs#L670))
- The reference magnifies the 128 × 32 `Interface\Tooltips\Nameplate-Border` art with `GL_LINEAR` (`0x770200`, filter `[0x878cf0]` = 1), smearing its 1 px bevel once the plate is larger than native; we resample the same pixels to the plate's physical size with sharp bilinear and draw it 1:1, because the chosen look is the same art with a crisp bevel. `super::drive_vplates` regenerates it when the physical size changes. ([`src/vplates/border.rs:3`](../crates/benilla-app/src/vplates/border.rs#L3))

#### wdb

- The files are `benilla-config/cache/<realm>/<store>.bin` through `crate::local_state`, not `WDB/` in the install, which is read-only; they are per realm, because another server may hold another template under the same key; and a store is written ten seconds after an answer lands as well as at exit, so a crash costs only re-asking. ([`src/wdb.rs:38`](../crates/benilla-app/src/wdb.rs#L38))
- The reference keeps a saved store's ask "pending" for the rest of the process (`0x555740` clears only the stores built with persistence off), so a query a lost link dropped is never sent again until restart; benilla's reconnect asks it again. ([`src/wdb.rs:344`](../crates/benilla-app/src/wdb.rs#L344))

#### world_console

- A float the scan leaves unread is 0.0, where the reference reads whatever its uninitialised stack slot holds. ([`src/world_console.rs:217`](../crates/benilla-app/src/world_console.rs#L217))
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

- Past the file's end `pad_to` zero-fills, because the reference reads stale buffer bytes there. ([`src/lib.rs:243`](../crates/benilla-blp/src/lib.rs#L243))
### benilla-formats

#### chain

- An archive that fails to open is an error. In 1.12.1 a failed base archive is fatal (Error #134: `0x4038ea` → `0x64d0e0` → `0x64cfa0` → `0x64cc80`, `ExitProcess`) and only a failing patch archive is skipped, silently; benilla fails on that too, because a skipped corrupt archive surfaces only as missing files far downstream. ([`src/chain.rs:67`](../crates/benilla-formats/src/chain.rs#L67))
- A last line with no CR or LF after it is passed too, where `0x648fb0` ends at the buffer's end without a flush (`0x64907f`) and drops it, a bug that loses a listfile's last name when it lacks a newline. ([`src/chain.rs:234`](../crates/benilla-formats/src/chain.rs#L234))

#### characters

- Blends in 8-bit RGBA, not the client's RGB565 with 2-bit coverage, because that format only saves texture memory and loses precision. ([`src/characters/sections.rs:280`](../crates/benilla-formats/src/characters/sections.rs#L280))

#### creature_families

- A zero range (row 28 only) is `None`, the lookup-miss fallback, because the reference divides by zero there into a NaN scale. ([`src/creature_families.rs:71`](../crates/benilla-formats/src/creature_families.rs#L71))

#### elevators

- A path that cannot drive a cycle is dropped and its GO stays at its spawn point, where the reference faults or hangs: a last frame at 0 divides by zero (`0x5f62a4`), and a first frame past 0, one frame among them, leaves a time before it in no bracket, which `0x5f62bc`-`0x5f62e7` seek forever. ([`src/elevators.rs:62`](../crates/benilla-formats/src/elevators.rs#L62))

#### emote_text

- A surplus `%s` stays verbatim, where the reference would print stack garbage; no shipped row has one. ([`src/emote_text.rs:113`](../crates/benilla-formats/src/emote_text.rs#L113))

#### garble

- A language with no words keeps the line, where the reference drops every word, because a missing pool is a broken install and blank chat would hide it. ([`src/garble.rs:151`](../crates/benilla-formats/src/garble.rs#L151))
- It stops at a codepoint boundary where the reference's byte copy can split one, because a `&str` must stay UTF-8. ([`src/garble.rs:258`](../crates/benilla-formats/src/garble.rs#L258))

#### loose

- A folder is listed the first time a key reaches it, where the reference lists every folder at the first open (`0x646ea0`), so a file added to a folder no key has reached is found before the index empties; the whole walk takes minutes and gigabytes where the install folder holds or links a large tree, as a Wine prefix's `z:` drive links `/`. ([`src/loose.rs:31`](../crates/benilla-formats/src/loose.rs#L31))

#### macro_icons

- Of names equal but for case, benilla keeps the first in source order, where the reference's survivor is whichever its CRT `qsort` (`0x73f727`, not stable) leaves last, as the dedup frees the earlier of each equal pair (`0x4f0163`-`0x4f0171`). ([`src/macro_icons.rs:65`](../crates/benilla-formats/src/macro_icons.rs#L65))

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

- Every token keys the `$l` plural. The reference's `$a`, `$d`, `$t`, `$e`, `$c`, `$p`, `$f`, `$F` and `$z` arms never write `[0xbe0b84]`, so its `$l` keys on the number before them, and Blizzard's "$s1 … every $t1 $lsecond:seconds;" reads "every 1 seconds" in 1.12.1. ([`src/spells/tokens.rs:690`](../crates/benilla-formats/src/spells/tokens.rs#L690))

#### tga

- Grayscale types 3 and 11 decode where the reference refuses them, because no shipped or addon file is one and removing working code would buy nothing. ([`src/tga.rs:6`](../crates/benilla-formats/src/tga.rs#L6))
- Colour maps of 15, 16 and 32 bits decode at the entry width the file declares, where the reference has no defined result to copy for 15 and 16 bits and only a garble for 32. ([`src/tga.rs:9`](../crates/benilla-formats/src/tga.rs#L9))
- A colour-mapped image whose descriptor has alpha bits decodes as its picture, where the reference has no defined result to copy. `0x5a3820` sizes each pixel as 24 plus the alpha bits (`0x5a382b`-`0x5a383d`, stored at `0x5a3973`) but copies 3-byte entries (`0x5a38f3`), then `0x5a3b80` moves `(bpp + 7) >> 3` bytes a pixel (`0x5a3c00`-`0x5a3c13`): each pixel after the first takes its bytes from its neighbours, and the last read heap that was never written. ([`src/tga.rs:16`](../crates/benilla-formats/src/tga.rs#L16))
- A truecolour image whose depth less its alpha bits is 24, but whose depth is not 24 or 32, is refused where the reference crashes. ([`src/tga.rs:22`](../crates/benilla-formats/src/tga.rs#L22))

#### transports

- `None` too where the reference's build overruns its stack or reads memory it never wrote (`transports#path-faults`), so the ship stays at its spawn: more than 1024 nodes in a leg (`0x5f9530`), a leg of 3 nodes or fewer (`0x4532e3`) and a stop on a leg's last node (`0x5f91a1`). ([`src/transports/mod.rs:82`](../crates/benilla-formats/src/transports/mod.rs#L82))
### benilla-lua

#### api

- The C build grows a string while `realloc` succeeds, and a failure is the catchable `not enough memory`; Rust's allocator aborts the process instead, and Linux grants far more than it can fill, so past this bound the libraries raise C's message. ([`src/api.rs:32`](../crates/benilla-lua/src/api.rs#L32))

#### heap

- C Lua has no such bound; past it the string is `not enough memory`, as C's would be when memory runs out. ([`src/heap.rs:659`](../crates/benilla-lua/src/heap.rs#L659))
- The client's `luaM_realloc` (`0x6fc980`) has no failure path to ask from; here a refused string asks for the collection that may free room. ([`src/heap.rs:697`](../crates/benilla-lua/src/heap.rs#L697))

#### lib_math

- The client's one process-wide stream is shared with its engine draws; here each VM holds its own, seeded from the wall clock when it is built, so a fork or a clone draws its own sequence. ([`src/lib_math.rs:230`](../crates/benilla-lua/src/lib_math.rs#L230))

#### lib_string

- The decoder reads a NUL as no bytes (`0x7fbeda`), so the client's loop never passes one and the call never returns; here the result ends there. ([`src/lib_string.rs:98`](../crates/benilla-lua/src/lib_string.rs#L98))
- The client writes 5.0 bytecode (`luaU_dump`, `0x6fb600`); benilla writes 5.1's header and the function's source name. ([`src/lib_string.rs:234`](../crates/benilla-lua/src/lib_string.rs#L234))
- 5.1's `match` recurses without a bound and overflows the C stack (near 32000 levels on 8 MB); this raises 5.2's `pattern too complex` instead. ([`src/lib_string.rs:528`](../crates/benilla-lua/src/lib_string.rs#L528))

#### lib_table

- The client's loops have no bound (`0x7fb728`, `0x7fb7e0`), but a remembered size is whatever an addon wrote, and the instruction-budget hook never sees a native's loop, so one over two billion slots would hang the client. ([`src/lib_table.rs:34`](../crates/benilla-lua/src/lib_table.rs#L34))

#### pools

- That sequence is not wholly the client's. A table's node vector is sized as 5.1 sizes it, neither the constant table each function's parse makes (`open_func`'s `luaH_new`, `0x6fcad0`) nor `lua_load`'s token buffer is counted, and a prototype's vectors are grown one after another where the client's parser grows them together; each moves a pool's chunk boundary, and so its free space, by what it changes. ([`src/pools.rs:8`](../crates/benilla-lua/src/pools.rs#L8))

#### vm

- These are benilla's own needs, past what the client's checks (`Vm::checkstack`) reserve: a frame's registers above `c_maxstack` (`Proto::maxstack`), and the host's and natives' pushes, which ask for no margin of `EXTRA_STACK` and one more. ([`src/vm.rs:975`](../crates/benilla-lua/src/vm.rs#L975))
- benilla's scratch registers above a frame's `c_maxstack` (`Proto::maxstack`) stay on the stack; the client has none, and halves. ([`src/vm.rs:2322`](../crates/benilla-lua/src/vm.rs#L2322))
### benilla-protocol

#### (crate root)

- A named piece takes a `:port` here, where the reference hands a name to `gethostbyname` whole (`0x43ba0e`) and only a dotted piece takes a port (`0x43f3d0`), because a server on a non-default logon port behind a host name is everyday use. ([`src/lib.rs:174`](../crates/benilla-protocol/src/lib.rs#L174))

#### auth

- The patch state downloads no patch and starts no updater, because benilla runs no code a server sends. ([`src/auth.rs:120`](../crates/benilla-protocol/src/auth.rs#L120))
- A stored per-OS constant, not the reference's HMAC over its own executables (`0x5b1170`), because every mangos-family realmd issues this one salt. ([`src/auth.rs:302`](../crates/benilla-protocol/src/auth.rs#L302))
- The survey id is read and dropped, and no survey runs, where the reference keeps it on the link (`0x5bad53`) for the `Survey.dll` it downloads and calls (`GetSurveyId 0x5ab620`, called at `0x474064`). benilla runs no code a server sends (`AuthReject::reply` gives the rule). ([`src/auth.rs:376`](../crates/benilla-protocol/src/auth.rs#L376))

#### events

- `benilla::spell_mods` refuses an out-of-range pair, because the reference would overrun it. ([`src/events.rs:774`](../crates/benilla-protocol/src/events.rs#L774))
- An out-of-range list past the first block reads no body in the write passes (`0x466076`), which misread every block behind it, while the notify pass reads past it (`0x465fd0`) and stays in step; benilla reads the list and drops it, keeping the blocks behind it whole. ([`src/events/decode.rs:1148`](../crates/benilla-protocol/src/events/decode.rs#L1148))
- The flagged pass reads a movement block's guid as 8 raw bytes (`0x467140`, `0x46714d`) where the wire packs it, so unless the packed guid happens to be 8 bytes the reference misreads the rest of that pass; benilla reads the block once, packed, as pass 0 does (`0x467080`). ([`src/events/decode.rs:1165`](../crates/benilla-protocol/src/events/decode.rs#L1165))

#### examples

- Benilla always sends the published constant instead, since an install without those binaries could never answer for itself. ([`examples/version_check_probe.rs:9`](../crates/benilla-protocol/examples/version_check_probe.rs#L9))

#### messages

- A stream that fails to inflate is no file, where the reference writes its buffer unchecked (`0x5afcbc`), because those bytes are whatever its stack held. ([`src/messages/account_data.rs:48`](../crates/benilla-protocol/src/messages/account_data.rs#L48))
- The whole stream is sent, where the reference cuts it at the file's size (`0x5afb42`), because the server refuses a cut stream and the next login then downloads its old copy over the player's file. ([`src/messages/account_data.rs:85`](../crates/benilla-protocol/src/messages/account_data.rs#L85))
- A fixed list, not read off the install, because no third-party addon sets `## Secure:`, so the two differ only where the `.pub` files were altered. ([`src/messages/addons.rs:93`](../crates/benilla-protocol/src/messages/addons.rs#L93))
- Every row is kept, because the reference takes the count unclamped into its 80 row blocks and a larger one overruns them. ([`src/messages/battlefield.rs:98`](../crates/benilla-protocol/src/messages/battlefield.rs#L98))
- A larger count keeps the first 40, where the reference's handler writes past the store. ([`src/messages/battlefield.rs:188`](../crates/benilla-protocol/src/messages/battlefield.rs#L188))
- Text without a NUL stops at `length`, because the reference reads on out of bounds. ([`src/messages/broadcast.rs:29`](../crates/benilla-protocol/src/messages/broadcast.rs#L29))
- The 1.12 client appends a zlib chat transcript to a category 2 ticket (`0x5ef936`); we omit it because vmangos never reads it and logs the leftover bytes. ([`src/messages/gm_ticket.rs:48`](../crates/benilla-protocol/src/messages/gm_ticket.rs#L48))
- String reads are unbounded. The reference `0x4191b0` caps each field, counting the NUL (roster name `0x30`, notes `0x80`, MOTD `0x200`, info `0x7d0`, guild name `0x60`, rank name `0x40`), and on overflow empties it and no-ops every later read in the packet. ([`src/messages/guild.rs:4`](../crates/benilla-protocol/src/messages/guild.rs#L4))
- A reason of 3 or more is silent, where the reference prints an uninitialized stack buffer, because 3 is the server's `INSTANCERESET_FAIL_SILENTLY`. ([`src/messages/instance.rs:70`](../crates/benilla-protocol/src/messages/instance.rs#L70))
- 2 or more prints nothing, because the reference prints an uninitialized buffer. ([`src/messages/opcode.rs:1059`](../crates/benilla-protocol/src/messages/opcode.rs#L1059))
- The consumer refuses 64 and up, because there the reference's store overruns its table. ([`src/messages/packet.rs:654`](../crates/benilla-protocol/src/messages/packet.rs#L654))
- A body too short for the position gives `None` where the client redisplays a stale global, because showing a stale queue position as current is a client bug. ([`src/messages/parse.rs:285`](../crates/benilla-protocol/src/messages/parse.rs#L285))
- A type outside 1-7 reads as nothing rather than the reference's default arm, which under-pushes its returns. ([`src/messages/pet.rs:71`](../crates/benilla-protocol/src/messages/pet.rs#L71))
- Both are read, told apart exactly by the tail length (`12n` vs `1 + 14n`), since the 1.12 client mis-parses vmangos's. ([`src/messages/pet.rs:191`](../crates/benilla-protocol/src/messages/pet.rs#L191))
- The active player's gender is this byte too, where `CGPlayer_C::GetGender` (`0x5ed5b0`) reads `PLAYER_BYTES_3` byte 0 whole, because that byte also carries the drunk bits (`gender | drunk & 0xFFFE` on vmangos), so a drunk female reads as male. ([`src/messages/update_object/fields/player.rs:573`](../crates/benilla-protocol/src/messages/update_object/fields/player.rs#L573))

#### world

- A size past 23 bits is refused, where the reference sends it with bit 23 folded into the flag (`0x5b5712` `or dl,0x80` keeps one byte), because no client packet comes near 8 MB and a refusal beats a corrupt header. ([`src/world/mod.rs:139`](../crates/benilla-protocol/src/world/mod.rs#L139))
- 10 s of silence ends the wait, where the reference has no timer and waits for the reply, a disconnect or the player's Cancel (`0x46b720`; its connect reports only success or a socket error, `0x5b60c6`-`0x5b61ac`), because each wait here is a blocking read on the net thread, which a server that stays connected and never answers would hold for good. ([`src/world/session.rs:15`](../crates/benilla-protocol/src/world/session.rs#L15))
- The reference waits forever; a bound tells a server that died mid-queue from a long wait. ([`src/world/session.rs:22`](../crates/benilla-protocol/src/world/session.rs#L22))
- Zeros until one does, where the reference's words are never initialised (the session is allocated without zero-fill, `ui/billing` in benilla-re). ([`src/world/session.rs:65`](../crates/benilla-protocol/src/world/session.rs#L65))
- Checked per queue packet where the reference checks each tick, so a cancel can lag one server update. ([`src/world/session.rs:102`](../crates/benilla-protocol/src/world/session.rs#L102))
- Benilla runs no Warden module and answers nothing, where the reference hands `SMSG_WARDEN_DATA` to the module its connect loads (`0x6ca2f0`, `0x6ca5c0`) and sends that module's answer as `CMSG_WARDEN_DATA` (`0x6ca6a0`). ([`src/world/session.rs:244`](../crates/benilla-protocol/src/world/session.rs#L244))
### benilla-srp

#### (crate root)

- `SrpClientChallenge::new` redraws `a` until every value it sends serializes the same both ways (`is_width_stable`), and `benilla-protocol`'s `logon` redials when the server's `B` lands ambiguous, because a client that picks either serialization fails about 1 handshake in 45 against vmangos. ([`src/lib.rs:13`](../crates/benilla-srp/src/lib.rs#L13))
- `a` is redrawn until `A`, `K` and `M1` have no high-order zero byte and `S` no low-order one, because the client and vmangos hash those values differently. ([`src/lib.rs:270`](../crates/benilla-srp/src/lib.rs#L270))
### benilla-ui

#### civil

- A `_dstbias` that takes the time below 0 faults the client, which copies the second `localtime`'s null answer unchecked (`0x740842` → `0x740860`); here it is refused. ([`src/civil.rs:87`](../crates/benilla-ui/src/civil.rs#L87))
- The zone database's offset for the date, where the reference's CRT applies the rules it read for every date, so a date in a zone whose offset has since changed does not round-trip through `time{}`. ([`src/civil.rs:121`](../crates/benilla-ui/src/civil.rs#L121))

#### loader

- A `<Script file=>` value with a separator joins the including document's directory, like a bare name. ([`src/loader/mod.rs:413`](../crates/benilla-ui/src/loader/mod.rs#L413))
- Padded to the block's line, so a raise reports a line a reader can find in the file; the reference counts from the body's first line. ([`src/loader/mod.rs:438`](../crates/benilla-ui/src/loader/mod.rs#L438))
- A cycle (`A` inherits `B`, `B` inherits `A`) stops at the repeated name with a warning. ([`src/loader/mod.rs:546`](../crates/benilla-ui/src/loader/mod.rs#L546))
- A link that would close a loop is refused and reported; the reference's XML legs (`0x783ce6`, `0x783d22`) check nothing, and the next push down such a loop would never end. ([`src/loader/mod.rs:723`](../crates/benilla-ui/src/loader/mod.rs#L723))
- `<HighlightColor>` and `<DisabledColor>` colour their own fonts; 1.12.1 sends both to the normal colour setter (`0x778aa4`, `0x778ada` call `0x778e20`), so neither could reach its state. ([`src/loader/widgets.rs:215`](../crates/benilla-ui/src/loader/widgets.rs#L215))

#### markup

- The length keeps every bit, where the client masks it to 16 (`0x77baf3`), because a wrapped length would put the cursor inside an `|H` span of 64 KiB or more. ([`src/markup.rs:299`](../crates/benilla-ui/src/markup.rs#L299))
- A step that runs out of buffer while skipping lands on the far end, because the client spins forever there (its skip loop re-enters at `0x77bb60`, past the zero check at `0x77bb56`), so `SetText("|cffffffff")` and one RIGHT hang 1.12. ([`src/markup.rs:422`](../crates/benilla-ui/src/markup.rs#L422))

#### order

- A child that left the scroll draw is drawn once, by the flat pass, where the client's callback, which reads only the child's visibility, draws it a second time, clipped, at the ScrollFrame's place. ([`src/order.rs:544`](../crates/benilla-ui/src/order.rs#L544))

#### script

- An out-of-range slot answers nil, because the reference indexes `0xbc6980` with no bounds check and reads the memory beside its 120 slots. ([`src/script/action.rs:621`](../crates/benilla-ui/src/script/action.rs#L621))
- The files the handler writes beside the install (`<Name>.pub` with the key, `<Name>.url`; `0x51dce0`, `0x51dde0`) are not written, as the install is read-only; the key is never read either, as there is no signature to verify with it. ([`src/script/addon.rs:207`](../crates/benilla-ui/src/script/addon.rs#L207))
- Holding the file, so the shutdown write keeps it. ([`src/script/addon.rs:787`](../crates/benilla-ui/src/script/addon.rs#L787))
- The reference drops a send that resolves to PARTY while party slot 0's guid (`0xbc6f48`) is zero (`0x49fa9f`-`0x49faab`); benilla sends it, because `PartyState` is empty until the first `SMSG_GROUP_LIST` and the gate would swallow a real broadcast. vmangos drops a group-less party line anyway (`ChatHandler.cpp:476`). ([`src/script/addon_message.rs:7`](../crates/benilla-ui/src/script/addon_message.rs#L7))
- Backed off to a char boundary, because a `String` must stay UTF-8; the cut is the same for ASCII. ([`src/script/addon_message.rs:125`](../crates/benilla-ui/src/script/addon_message.rs#L125))
- `GetPlayerBuffApplications` answers 1 for a cleared record where the reference reads its stale `+0x9`, the previous occupant's count, because no guarded caller reaches it. ([`src/script/aura/player_buff.rs:98`](../crates/benilla-ui/src/script/aura/player_buff.rs#L98))
- The reference's usage line names `GetPlayerBuffTimeLeft` (`0x84bcc0`, shared with `0x4e4930`); ours names this verb, because nothing can depend on the wrong name. ([`src/script/aura/player_buff.rs:176`](../crates/benilla-ui/src/script/aura/player_buff.rs#L176))
- `None` answers `nil` where the reference's `-1` faction leg pushes nothing (Lua reads below the tuple), because no playable race reaches that leg. ([`src/script/battlefield_queue.rs:40`](../crates/benilla-ui/src/script/battlefield_queue.rs#L40))
- The reference (`0x4abdc0`) admits 9 and reads one dword past its block; we answer 0 because that read is out of bounds. ([`src/script/battlefield_score.rs:289`](../crates/benilla-ui/src/script/battlefield_score.rs#L289))
- `pow(10, n)` is the platform's, where the reference's (`0x73f940`, `fyl2x`) rounds its extended-precision steps at 53 bits, so a power or a fraction term can differ in the last place. ([`src/script/binding_abi.rs:281`](../crates/benilla-ui/src/script/binding_abi.rs#L281))
- The disabled-checked leg (`0x785380`) tests and reloads the checked slot (`[+0x4e0]`, `0x785388`/`0x7853a5`), a copy of the checked leg's (`0x785270`), so in 1.12.1 a check box with a checked texture repaints that one and never gets its disabled-checked texture from a path. benilla tests and reloads the slot the method names. ([`src/script/button.rs:621`](../crates/benilla-ui/src/script/button.rs#L621))
- The slot is read before the handler, where the reference reads it after (`[esi+0x4dc]`), because reading first keeps the borrow simple and no stock handler re-slots its row. ([`src/script/button.rs:1213`](../crates/benilla-ui/src/script/button.rs#L1213))
- The reference's only bound is `luaD_call`'s count of 200 nested calls (`0x6f65ae`), past which it raises `C stack overflow` (`0x6f65bf`); eight here, as each level keeps an apply's frame on the Rust stack. ([`src/script/calls.rs:731`](../crates/benilla-ui/src/script/calls.rs#L731))
- The reference composes the Shortcut with `GetRealZoneText` alone (`0x49efe8`), so in a capital `/leave Trade` sends `Trade - Stormwind City` and leaves nothing; ours uses the join's city-aware name, because the reference's form cannot leave the channel it joined. ([`src/script/channel.rs:535`](../crates/benilla-ui/src/script/channel.rs#L535))
- The frame's bottom-left origin is snapped onto the device grid so the border art blits 1:1; the reference ceils the anchor point itself onto it (`0x7666a0`, `0x766750`). ([`src/script/chat_bubble.rs:482`](../crates/benilla-ui/src/script/chat_bubble.rs#L482))
- No truncation at 255 characters (`0x49f604`), because vmangos refuses a longer line itself (`Chat.cpp:2177`): it is dropped where the reference sends its first 255. ([`src/script/chat_send.rs:26`](../crates/benilla-ui/src/script/chat_send.rs#L26))
- A value outside 0..1 clamps, where the reference keeps the low byte (alpha 2.0 stores 254, -1.0 stores 1), because no FrameXML or addon caller passes one. ([`src/script/chat_window.rs:17`](../crates/benilla-ui/src/script/chat_window.rs#L17))
- An id outside the seven built windows raises, where the reference serves ten records and silently ignores any other id (`0x4a0be1`), because benilla keeps only the seven and every caller loops `1, NUM_CHAT_WINDOWS`. ([`src/script/chat_window.rs:221`](../crates/benilla-ui/src/script/chat_window.rs#L221))
- One past `i32::MAX` clamps where the reference keeps the low 32 bits, for the colours' reason. ([`src/script/chat_window.rs:481`](../crates/benilla-ui/src/script/chat_window.rs#L481))
- The reference reads the clock live at each call (`0x42c010`); ours is the session clock the tick advances by the paced frame step, one value per frame, so `GetTime()` deltas and the `elapsed` handed to OnUpdate agree, as FrameXML (CastingBarFrame and kin) assumes. ([`src/script/clock.rs:64`](../crates/benilla-ui/src/script/clock.rs#L64))
- With no wheel it scales by the strip's, because the reference reads the missing wheel unguarded. ([`src/script/colorselect.rs:379`](../crates/benilla-ui/src/script/colorselect.rs#L379))
- The reference's `set` registers an unknown name as a new category-5 record (`0x63d569`-`0x63d582`); benilla refuses it, because nothing would read the row and `config.toml` keeps an unknown key anyway. ([`src/script/console_exec.rs:120`](../crates/benilla-ui/src/script/console_exec.rs#L120))
- A row the session owns (an env lever) is passed, because the env chose its value for this run and the file never holds it, so a reset would undo the lever and save nothing. ([`src/script/console_exec.rs:204`](../crates/benilla-ui/src/script/console_exec.rs#L204))
- A row index the list no longer holds buys nothing and drops, where this path reads through a null row (`0x4f9f35`). ([`src/script/container.rs:753`](../crates/benilla-ui/src/script/container.rs#L753))
- A last `spell_id` key breaks the remaining ties (Charge's six ranks all have `spellLevel` 0), because the reference leaves them to its qsort over an array benilla does not reproduce; ascending id is ascending rank in every 1.12 pet-ability family. ([`src/script/craft.rs:255`](../crates/benilla-ui/src/script/craft.rs#L255))
- The reference reads past the table unbounded (`0x4f6c90`), so type 5 would answer "CloseCraft"; no shipped opener carries a type past 3, and benilla answers nil rather than read past it. ([`src/script/craft.rs:926`](../crates/benilla-ui/src/script/craft.rs#L926))
- On `Terrain` the reference keeps a spell, action, macro or pet action on the left button; we clear them, because otherwise a spell has no left-click way off the cursor. ([`src/script/cursor.rs:744`](../crates/benilla-ui/src/script/cursor.rs#L744))
- Every lock lifts here, bag contents included. The reference's walk of a bag's contents indexes the bag by its own player-array index (`0x495528`, `0x495535`), at least 19, past a bag's slots in every case but the biggest bags, so it normally unlocks none of them; a lock it strands lasts until the item's seat or stack moves, which this does not copy. ([`src/script/cursor.rs:950`](../crates/benilla-ui/src/script/cursor.rs#L950))
- Returns nothing. The handler ends `mov eax,1` with nothing pushed (`0x4895bc`), so Lua 5.0 hands back the stack top, the last argument or the function itself: an accident of the handler, not an API, and copying a stack-top leak would copy a bug. ([`src/script/cursor.rs:1254`](../crates/benilla-ui/src/script/cursor.rs#L1254))
- An auto-store into a bank item or keyring slot sends nothing, because no container id names it and no stock caller passes one; the reference sends it and the server refuses. ([`src/script/cursor/bag_verbs.rs:26`](../crates/benilla-ui/src/script/cursor/bag_verbs.rs#L26))
- A row index the list no longer holds buys nothing and drops, where this site reads through a null row (`0x4c7e9a`, `0x4c7ea5`). ([`src/script/cursor/bag_verbs.rs:132`](../crates/benilla-ui/src/script/cursor/bag_verbs.rs#L132))
- Slot 11 is refused, because the reference's gate (`cmp esi,0xa; jbe`) admits it; `0x4bce00` checks its bound (`0x4bce14`) but `0x4bc9a0` does not, so it writes one dword past the array (`0x4bcad9`), and the autocast toggle (`0x4bcbb0`) reads that word (`[0xb71438+0x28]`) with no bounds check. ([`src/script/cursor/pet.rs:20`](../crates/benilla-ui/src/script/cursor/pet.rs#L20))
- The list is the window sizes this client can present, not the device's display modes, because benilla makes no exclusive mode-set. `"WxH"` is a contract: `OptionsFrame.lua:281-284` splits on the `x`, raising on a string without one, and `CT_Viewport.lua:107` matches `"(%d+)x(%d+)"`, failing silently. ([`src/script/cvars.rs:81`](../crates/benilla-ui/src/script/cvars.rs#L81))
- The tie-break is ours, because area alone with the reference's adjacent-only dedupe can leave a duplicate `"WxH"` when two modes share an area (1280x960, 1600x768). ([`src/script/cvars.rs:94`](../crates/benilla-ui/src/script/cvars.rs#L94))
- An id out of range is ignored, because the reference reads one entry past the last (`0x48c67c`) and a clamp would apply a format the player did not pick. ([`src/script/cvars.rs:779`](../crates/benilla-ui/src/script/cvars.rs#L779))
- The reference (`0x48dad0` into `0x639a20`) writes hardware-recommended values from its matched `VideoHardware.dbc` row and the `CGxFormat` preset it names (`[0xc4e6ac]`): twelve `gx*` CVars, a synchronous `RestartGx`, then sixteen more. benilla has no such matching, so its registered defaults are its recommended configuration. ([`src/script/cvars.rs:895`](../crates/benilla-ui/src/script/cvars.rs#L895))
- The index is of the live window size, not of the `gxResolution` CVar the reference parses (`0x48bf20`), because the window here can be dragged to any size, where the reference's backbuffer stays at `gxResolution`'s, and `CT_Viewport` scales by the live one. ([`src/script/cvars.rs:948`](../crates/benilla-ui/src/script/cvars.rs#L948))
- 0, a negative or an index past the count clamps to the nearer end, because the reference reads one element past the last there (`0x48c00d`), uninitialised memory. ([`src/script/cvars.rs:966`](../crates/benilla-ui/src/script/cvars.rs#L966))
- Always empty, because a refresh rate is only selectable through an exclusive mode-set and benilla has none. ([`src/script/cvars.rs:1010`](../crates/benilla-ui/src/script/cvars.rs#L1010))
- 1.12 has no such CVar; it exists because our Graphics page needs somewhere to keep the stop, and takes the API's name. ([`src/script/cvars.rs:1118`](../crates/benilla-ui/src/script/cvars.rs#L1118))
- The getter reads `CVAR_WORLD_DETAIL`, where the reference's reads only `smallCull` (`0x488d77`), because benilla's stop follows the knob that draws: a console `frillDensity` write moves the ground clutter and `WorldDetail` with it, and no renderer reads `smallCull` (its callback's `[0x868620]` has no reader). ([`src/script/cvars.rs:1146`](../crates/benilla-ui/src/script/cvars.rs#L1146))
- The minutes come from the server's table, not from spell 15007's `Spell.dbc` duration, because the aura the server applies is what the dialog must warn about. ([`src/script/death.rs:45`](../crates/benilla-ui/src/script/death.rs#L45))
- The promotion's IME legs (`0x77d417`–`0x77d431`) and the IME writer's `or 6` (`0x77acde`, in `0x77ac60`) are not modelled: benilla has no IME composition. ([`src/script/editbox/flush.rs:28`](../crates/benilla-ui/src/script/editbox/flush.rs#L28))
- The nameplate glow is shown but never drawn, because its additive rim reads as hard edge lines in our linear-blending pipeline; the lit plate brightens its bar instead. ([`src/script/extract.rs:223`](../crates/benilla-ui/src/script/extract.rs#L223))
- A host of 0x80 bytes or more is refused. The reference copies the host into a 0x80-byte buffer (`0x5abc7c`-`0x5abc9c`) and writes its terminator one byte past the end, and the cut host could end in a whitelisted suffix the real one does not; this takes the safe side. ([`src/script/glue_core.rs:424`](../crates/benilla-ui/src/script/glue_core.rs#L424))
- Benilla keeps that decode and then encodes it again, every byte outside the URL characters `URL_SAFE` as `%XY` (a decoded `%` too), because the opener on some platforms is a shell script (`xdg-open`) or takes the raw command tail (`rundll32`) and the string comes off the wire: no space, quote, backtick, `$`, `;`, `<`, `>`, `|`, `^`, `\`, brace or control byte reaches it. ([`src/script/glue_core.rs:469`](../crates/benilla-ui/src/script/glue_core.rs#L469))
- Past `NAME_TRIES` names the answer is nil, where the reference keeps trying, forever when the generator has learned nothing. ([`src/script/glue_create.rs:968`](../crates/benilla-ui/src/script/glue_create.rs#L968))
- With no Region rows the reference reads a NULL category (`0x46e5b8`, `0x46f26a`) and faults; benilla lists every realm under one category, so a client whose `Wow.ini` names another Region still reaches a realm. ([`src/script/glue_realm.rs:195`](../crates/benilla-ui/src/script/glue_realm.rs#L195))
- Equal nonzero counts tie here, where the reference answers +1 both ways (its name tie-break at `0x46e841` is clobbered by `0x46e846`), because a comparator that is not a total order has no defined sort. ([`src/script/glue_realm.rs:260`](../crates/benilla-ui/src/script/glue_realm.rs#L260))
- The backpack, the bank and the keyring are each bounded by their own slots, where `0x4f990f` bounds them only by the player list's count (0x71) from their bases (the bank at `+0x27`, `0x4f98ae`; the keyring at `+0x51`, `0x4f98c0`), a reference bug that lets their index read the next container's slots. ([`src/script/held.rs:74`](../crates/benilla-ui/src/script/held.rs#L74))
- A span count that no longer matches the live set always rebuilds, so a state this model does not flag (a ring wiped by `SetMaxLines`, a hidden frame, a frame not yet laid out, which the reference's rebuild skips at `0x78af7b`) leaves no stale button. ([`src/script/hyperlink.rs:167`](../crates/benilla-ui/src/script/hyperlink.rs#L167))
- `IsInInstance()` then answers `nil, "none"`, because the reference returns two results without pushing either (`0x48a7a9`), a stack bug. ([`src/script/instance.rs:24`](../crates/benilla-ui/src/script/instance.rs#L24))
- Bounds-checked where the reference indexes unguarded, so none reads past it. ([`src/script/item_stats.rs:677`](../crates/benilla-ui/src/script/item_stats.rs#L677))
- Nil for a display row with no icon, where the reference answers the unloadable `Interface\Icons\`, on which `SetTexture` keeps the previous image (`0x7702ac`), so a reused button would show its old icon. ([`src/script/item_stats.rs:829`](../crates/benilla-ui/src/script/item_stats.rs#L829))
- A key rebound to the command it already has keeps a duplicate index in 1.12.1 (`0x4b7786` lowers the other keys' indices before `0x4b77ac` returns on the same name), so both getters there answer one key too few; that client fault is not reproduced. ([`src/script/keybind.rs:223`](../crates/benilla-ui/src/script/keybind.rs#L223))
- An addon file whose first binding has no header opens a `HEADER_<addon>` row, where the reference files those rows under the previous file's last header, because under a foreign header the player could not find them. ([`src/script/keybind.rs:324`](../crates/benilla-ui/src/script/keybind.rs#L324))
- The handle's value is the address of a `CGxTex` (`SMemAlloc(0x54)`, `0x594a96`), which benilla cannot reproduce; each texture ranks instead by a creation stamp, one global sequence number taken when its cache key is first created and taken anew once its last region has let go of it (`super::texel::Texels`). ([`src/script/layer_batch.rs:9`](../crates/benilla-ui/src/script/layer_batch.rs#L9))
- The reference's byte copy can end inside a character and keep the stray bytes; a `String` must stay UTF-8, so the cut stops at the character boundary. ([`src/script/macros.rs:127`](../crates/benilla-ui/src/script/macros.rs#L127))
- A listing that finds no icon stays empty, to be listed again on the next ask, where the reference's dedup then reads below its null list (`0x4f0134`-`0x4f0145`) and faults. ([`src/script/macros.rs:551`](../crates/benilla-ui/src/script/macros.rs#L551))
- Each line is trimmed, because the reference sends an indented `" /say hi"` as plain chat, never as the command meant. ([`src/script/macros/bind.rs:30`](../crates/benilla-ui/src/script/macros/bind.rs#L30))
- The `/cast` argument is trimmed, because the reference binds `/cast  Fireball` (two spaces) to a name that never resolves. ([`src/script/macros/bind.rs:50`](../crates/benilla-ui/src/script/macros/bind.rs#L50))
- The client's Lua has no such bound, as nothing sets a hook (`lua_sethook`, `0x6fba40`, has no caller and no pointer to it), so a load that never ends hangs it. ([`src/script/mod.rs:953`](../crates/benilla-ui/src/script/mod.rs#L953))
- The reference sets the cells only when glue or the interface is built, `0x41ad10` from `0x46a828` (`0x46a7b0`) and `0x48fe2b` (`0x48fbf0`), and a resize reaches them only through the rebuild it asks for. ([`src/script/mod.rs:1041`](../crates/benilla-ui/src/script/mod.rs#L1041))
- A path that does not resolve is stored, where the reference raises `Invalid model file: %s` (`0x878b44`), because the file loads asynchronously here and the call cannot know yet. ([`src/script/modelframe.rs:333`](../crates/benilla-ui/src/script/modelframe.rs#L333))
- The reference's free list is FIFO (`0x6087f0`, `0x608a5a`); either keeps every index stable, all an addon can observe of the pool's order. ([`src/script/nameplate.rs:807`](../crates/benilla-ui/src/script/nameplate.rs#L807))
- The reference adds this rim over the bar; we brighten the bar (`LIT_BOOST`), because the rim reads as hard edge lines in our linear-blending pipeline. ([`src/script/nameplate.rs:1430`](../crates/benilla-ui/src/script/nameplate.rs#L1430))
- A wrapper that decodes to no live widget takes `default_parent`, because the reference, with no `IsA` check, reads a wild pointer. ([`src/script/object/anchor_args.rs:218`](../crates/benilla-ui/src/script/object/anchor_args.rs#L218))
- An `OnSizeChanged` that resolve produces fires at the next `UiScript::resolve`, because a Lua handler must not run in the middle of a level compaction. ([`src/script/object/toplevel.rs:77`](../crates/benilla-ui/src/script/object/toplevel.rs#L77))
- A third `%s` gets an empty string, because the reference pushes two varargs (`0x6093f2 add esp,0x14`) and would read uninitialised stack. ([`src/script/pvp/mod.rs:319`](../crates/benilla-ui/src/script/pvp/mod.rs#L319))
- A token resolving to guid 0 answers nil too. The reference jumps from the guid test (`0x5172f6`) to `return 1` (`0x517343`) with nothing pushed, so the one value is its top argument, the token it read (`0x5172e6`); that is a stack quirk, not an answer. ([`src/script/pvp/mod.rs:568`](../crates/benilla-ui/src/script/pvp/mod.rs#L568))
- With `UNIT_PVP_NAME` or the title missing from `_G` (no install) this answers the plain name, where the reference would format an empty string, because the decoration is install data. ([`src/script/pvp/mod.rs:572`](../crates/benilla-ui/src/script/pvp/mod.rs#L572))
- An in-flight name reads `...`, where the reference's reads `" "` (`0x82ee00`), because a blank looks like a nameless objective. ([`src/script/quest_log.rs:54`](../crates/benilla-ui/src/script/quest_log.rs#L54))
- One signal per missed record, where the reference runs every callback of every miss as its record lands (items at `0x55bea8` and `0x55bf39` in `0x55bdb0`, from `0x555140`; `0x4e083a`), 2^k - 1 updates for a walking handler over k first-seen records. ([`src/script/quest_log.rs:1212`](../crates/benilla-ui/src/script/quest_log.rs#L1212))
- A local clock step mid-session (an NTP correction) jumps the reference's countdown but not ours, because the step changes no time the quest has left. ([`src/script/quest_log.rs:1282`](../crates/benilla-ui/src/script/quest_log.rs#L1282))
- Four set ids count 4, where the loop's exit `jmp 0x4e0a4c` (`0x4e0a42`) skips the store at `0x4e0a49` and answers 0, a client bug. ([`src/script/quest_log.rs:1654`](../crates/benilla-ui/src/script/quest_log.rs#L1654))
- The reference bounds the index with `jbe` against the entry count (`0xb73760`), so an index one past the last entry reads the pool entry after it, which only an earlier, longer list wrote, or uninitialised heap (`0x4d5310`, the rebuild resetting the count without clearing, `0x4d547f`); that stale read answers 0 here, as an index further on does. ([`src/script/reputation.rs:563`](../crates/benilla-ui/src/script/reputation.rs#L563))
- The folds are keyed by header, not by the reference's mask positions, whose rebuild folds bit N-1 by registration index before the sort (`0x4d5d49`-`0x4d5d68`) and whose fold verbs search `0xb736c0` before `0x4d5c40` sorts it (`0x4d6400`), so Inactive opens at login whenever a childless header exists and inactivating a header's last child shows it expanded; that is a reference bug, and the folds follow the header the player clicked. ([`src/script/reputation.rs:631`](../crates/benilla-ui/src/script/reputation.rs#L631))
- Nesting stops at 32 where the reference recurses uncapped (its indent saturates at `0x80`), so a hostile structure cannot exhaust the stack. ([`src/script/saved.rs:21`](../crates/benilla-ui/src/script/saved.rs#L21))
- The reference rewrites it from the live defaults (`0x51f865`–`0x51f970`) after renaming the old file to `.bak` (`0x51f794`, `0x51f8eb`, `.bak` at `0x8431e4`), so a hand edit with one typo survives only until the next logout. ([`src/script/saved.rs:27`](../crates/benilla-ui/src/script/saved.rs#L27))
- The writer's escapes are undone (`unescape`), where the reference sets the bytes between the quotes raw, so a string holding a quote or a backslash would grow every session. ([`src/script/saved.rs:197`](../crates/benilla-ui/src/script/saved.rs#L197))
- The block step is not pixel-snapped (`0x766750`), because layout here is in logical units; the two differ by under a pixel, and not at all at the default spacing of 0. ([`src/script/simplehtml/mod.rs:13`](../crates/benilla-ui/src/script/simplehtml/mod.rs#L13))
- A block of width 0, in a frame sized by anchors, gets its text's natural width where the reference keeps 0, because every width-less FontString here does. ([`src/script/simplehtml/mod.rs:15`](../crates/benilla-ui/src/script/simplehtml/mod.rs#L15))
- The reference prints these to the console; here they are host warnings, because a malformed page is bad content, not a script error. ([`src/script/simplehtml/mod.rs:105`](../crates/benilla-ui/src/script/simplehtml/mod.rs#L105))
- Only `%s` and `%%` are expanded and other specs copied, because the real `printf` would read the string pointers as other types. ([`src/script/simplehtml/parse.rs:247`](../crates/benilla-ui/src/script/simplehtml/parse.rs#L247))
- Skipped as no command. The stock walk calls an entry only when one of its aliases matches (`ChatFrame.lua:2164-2178`), so only a matched one raises. ([`src/script/slash.rs:71`](../crates/benilla-ui/src/script/slash.rs#L71))
- The app plays the queued file after the call, so the refusals (the bus-3 cap, a muted SFX channel, a missing file) come too late to answer, and a queued file answers 1. ([`src/script/sound.rs:68`](../crates/benilla-ui/src/script/sound.rs#L68))
- `BOOKTYPE_SPELL`/`BOOKTYPE_PET` are engine globals here, where the reference defines them in `SpellBookFrame.lua:5-6`, so this module's tests run without the stock file. ([`src/script/spellbook.rs:10`](../crates/benilla-ui/src/script/spellbook.rs#L10))
- Ours answers milliseconds, since a cycle count of unknown rate is no use to an addon timing its code. ([`src/script/stdlib.rs:101`](../crates/benilla-ui/src/script/stdlib.rs#L101))
- A format that ends in `%` reads past its terminator in the client (`0x4135c3`), into whatever memory follows the string; here the output ends there. ([`src/script/stdlib.rs:637`](../crates/benilla-ui/src/script/stdlib.rs#L637))
- With `#` and one byte of room, `_store_number` writes no digit and its reversal swaps before it tests (`0x413ab3`), trading the last byte written for the stale byte after it in the caller's buffer, which then ends the client's answer; here the output is kept. ([`src/script/stdlib.rs:688`](../crates/benilla-ui/src/script/stdlib.rs#L688))
- The reference reads uninitialised fields before the seed; we answer the first legal design because that value is undefined. ([`src/script/tabard.rs:122`](../crates/benilla-ui/src/script/tabard.rs#L122))
- Returns nothing. The handler ends `mov eax,1` with nothing pushed (`0x4dcd7c`), so Lua 5.0 hands back the stack top, the node argument: an accident of the handler, not an API, and copying a stack-top leak would copy a bug. ([`src/script/taxi.rs:235`](../crates/benilla-ui/src/script/taxi.rs#L235))
- `0x530a52` lays it out at once; getters resolve first, so only OnSizeChanged fires later. ([`src/script/tooltip/size.rs:180`](../crates/benilla-ui/src/script/tooltip/size.rs#L180))
- The reference anchors a Font object through its +0x24 with no type test (`0x5313c9`), undefined; benilla raises the owner line. ([`src/script/tooltip/verbs.rs:51`](../crates/benilla-ui/src/script/tooltip/verbs.rs#L51))
- No time line follows, where the reference's rides the record's `+0xc` (`0x532bdf`), a byte an empty record keeps from its history. 0 after `ClearBuffRecord 0x4e4490` prints a lone "0 seconds remaining" (`0x4e447e`); else it is whatever the last occupant left, as `ResetBuffCache 0x4e40c0` and the compaction's tail reset (`0x4e4237`) call `0x4e44b0` with slot -1, which writes `+0x0` and `+0x4` alone (`0x4e44bc`-`0x4e44c0`). ([`src/script/tooltip_aura.rs:238`](../crates/benilla-ui/src/script/tooltip_aura.rs#L238))
- The reference's builder hides and clears on a missing record (`0x52b72e`..`0x52b73f`), where a source that knows the item's name shows that, because a blank plate under an on-screen name reads as broken; the template's arrival then builds it whole. ([`src/script/tooltip_item/build.rs:56`](../crates/benilla-ui/src/script/tooltip_item/build.rs#L56))
- An object no longer held rebuilds from the instance the build read, where the reference's lookup finds no object and builds from the record and the block alone; benilla's views carry no object to read once it is gone, and the kept instance is what the plate showed. ([`src/script/tooltip_item/build.rs:203`](../crates/benilla-ui/src/script/tooltip_item/build.rs#L203))
- The reference recurses without a bound (`0x52dbce` builds the product whatever the depth), so a product that teaches a recipe for itself would recurse until the stack overflows; benilla stops here, so a cyclic `Spell.dbc` chain cannot crash the client. ([`src/script/tooltip_item/render.rs:199`](../crates/benilla-ui/src/script/tooltip_item/render.rs#L199))
- The reference's padding repeats a mana stat once per zero entry; not emulated, since only the internal test ring 6674 has one. ([`src/script/tooltip_item/render.rs:574`](../crates/benilla-ui/src/script/tooltip_item/render.rs#L574))
- The unknown-skill "Requires %s" shares it, though its reference red is `0xffff0000`, because a 32/255 difference in G and B does not show. ([`src/script/tooltip_unit.rs:135`](../crates/benilla-ui/src/script/tooltip_unit.rs#L135))
- The selection clears to 0, because the header the reference re-selects (`0x4d7b42`, reading 1) has no spell id; the stock window's `> 1` test treats 0 and 1 alike. ([`src/script/trainer/mod.rs:332`](../crates/benilla-ui/src/script/trainer/mod.rs#L332))
- A header clears it too, because it has no spell id; the reference stores its `0xffffffff` (`0x4d7b05`) and reads back 1, which the stock window's `> 1` test treats the same as 0. ([`src/script/trainer/mod.rs:1310`](../crates/benilla-ui/src/script/trainer/mod.rs#L1310))
- Index 0 is a no-op, because one stray call would spend the purse; the reference buys every visible service for any index at or below 0 (`0x4da244`, `0x4d8a70`). ([`src/script/trainer/mod.rs:1334`](../crates/benilla-ui/src/script/trainer/mod.rs#L1334))
- Kept apart from `Self::vertex_color`, which the reference's `Texture:SetAlpha` (`0x79b580`) rewrites, because where stock sets both on one region (`PlayerFrame.lua:25-26`, `TargetFrame.lua:408` with `420-426`) the drawn product differs for about one frame, and a FontString's `SetAlpha` then keeps its font colour. ([`src/script/types.rs:428`](../crates/benilla-ui/src/script/types.rs#L428))
- Among whole-name matches ours picks the nearest, because the reference's first-walked pick is order-dependent and reads as a bug. ([`src/script/unit/bindings.rs:1077`](../crates/benilla-ui/src/script/unit/bindings.rs#L1077))
- The reference's matcher takes a whole-string hit at once only when it is nearer than the best so far (`0x493c88`-`0x493ca2`); an exact hit that is not nearer competes on prefix length alone (`0x493cf8`-`0x493d09`), so a nearer "Bobby" walked first beats an exact "Bob", and the pick follows the object list's order. ([`src/script/unit/by_name.rs:19`](../crates/benilla-ui/src/script/unit/by_name.rs#L19))
- A full tie goes to the lower guid, where the reference keeps the first unit its walk (`0x493aa0`) reaches, an order neither of ours has; a guid keeps the two picks equal. ([`src/script/unit/by_name.rs:61`](../crates/benilla-ui/src/script/unit/by_name.rs#L61))
- `0x4e81d0` has no bound and reads past its four-entry table, which is stale memory, not a mechanism; past it, nobody. ([`src/script/unit/resolve.rs:85`](../crates/benilla-ui/src/script/unit/resolve.rs#L85))
- The reference ties a miss against a hit (`0x5adab6`); no total order can, and `sort_by` may panic on one that is not, so the miss goes last. ([`src/script/who_sort.rs:159`](../crates/benilla-ui/src/script/who_sort.rs#L159))
- The engine's own push (the world-enter sync) queues `WORLD_MAP_UPDATE` for the next frame's tick and fires only on a change, where the reference fires in place from the handler that moved the data: the app's feeds run after the frame's tick. ([`src/script/worldmap.rs:15`](../crates/benilla-ui/src/script/worldmap.rs#L15))

#### status

- The cut at 255 bytes backs up to a character start, where the reference cuts at the byte, mid-character (`0x64a868`, `SStrCopy` `0x64a5e0`), because a Rust string holds whole characters. ([`src/status.rs:19`](../crates/benilla-ui/src/status.rs#L19))

#### strings

- A pass that changes nothing (`|01`, `|02`) ends the scan, where the reference loops on it for ever. ([`src/strings.rs:46`](../crates/benilla-ui/src/strings.rs#L46))

#### widget

- The history browses as a shell's does, a small everyday gain: the first UP stashes the line being typed and DOWN past the newest line brings it back, UP holds at the oldest, DOWN does nothing when not browsing, typing or a focus gain ends the browse, and an empty line is not kept. ([`src/widget/kinds/editbox.rs:247`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L247))
- A plain Left or Right (or Ctrl+B or F) with a selection collapses it to that edge, as every OS's own text fields do; the reference clears the selection and steps once from the caret (`0x77c6b0` from `0x77c70b`). ([`src/widget/kinds/editbox.rs:818`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L818))
- Left reads the whole character before the caret, not the lone byte `caret - 1` the client reads (`0x77c8da`), which takes a multi-byte space for a non-space. ([`src/widget/kinds/editbox.rs:837`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L837))
- On the last row a walk may end at the text's end, because the end starts no row; the reference steps back from it as from a row's start (its line table ends with the text's length, `0x77d6ef`), so DOWN onto a short last line stops before its last letter. ([`src/widget/kinds/editbox.rs:888`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L888))
- The drag selects from its anchor, as every OS's own text fields do, where the reference's applies `0x77cd10` to each mouse move's whole delta (`0x77a8a1`). ([`src/widget/kinds/editbox.rs:1001`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L1001))
- A code point past U+FFFF is no space, where the client tests its low 16 bits, so U+10020 counts as a space there. ([`src/widget/kinds/editbox.rs:1131`](../crates/benilla-ui/src/widget/kinds/editbox.rs#L1131))
- The cut backs up to a character start, where the reference cuts at the byte. ([`src/widget/kinds/messageframe.rs:120`](../crates/benilla-ui/src/widget/kinds/messageframe.rs#L120))
- N of -2 or less is an empty line too, where the reference asks `SMemAlloc` for about 4 GB and fails. ([`src/widget/kinds/messageframe.rs:121`](../crates/benilla-ui/src/widget/kinds/messageframe.rs#L121))
- An index past 5 reads the last entry, where 1.12.1 reads past both six-entry tables (`0x6da9d5`, `0x6da9c4`), a client fault a hand-edited CVar reaches. ([`src/widget/kinds/mod.rs:965`](../crates/benilla-ui/src/widget/kinds/mod.rs#L965))
- `None` until then, not the ctor's 0, so that an unset row takes nothing rather than slot 0. ([`src/widget/kinds/mod.rs:1067`](../crates/benilla-ui/src/widget/kinds/mod.rs#L1067))
- On the thumb it is the grabbed point, where `CSimpleSlider` (`0x789ba0`/`0x789ca0`) always centers it, because that feels less surprising and a stock-sized thumb hides the difference. ([`src/widget/kinds/mod.rs:1525`](../crates/benilla-ui/src/widget/kinds/mod.rs#L1525))
- Levels pack contiguously, because the reference's resuming cursor (`0x764fd3`/`0x764fd8`) leaves a hole past a wide gap ({0, 4, 6} at bound 7 gives {0, 1, 3} at bound 4). ([`src/widget/propagation.rs:189`](../crates/benilla-ui/src/widget/propagation.rs#L189))
- The frame is still in the old parent's child list during `OnHide`, unlike the reference (`0x76ab34` unlinks first), because a handler that itself reparents must not find the lists half-spliced. ([`src/widget/propagation.rs:312`](../crates/benilla-ui/src/widget/propagation.rs#L312))
### benilla-world

#### clouds

- The reference starts this buffer at 0xFFFFFFFF; alpha 0 keeps a dome drawn before the first build from flashing white. ([`src/clouds/kernel.rs:94`](../crates/benilla-world/src/clouds/kernel.rs#L94))
- The index wraps where the reference reads past the tile, only at `u == 1.0`. ([`src/clouds/kernel.rs:308`](../crates/benilla-world/src/clouds/kernel.rs#L308))
- Clamped; the reference NaNs above about 70° elevation, where its bodies never go. ([`src/clouds/kernel.rs:338`](../crates/benilla-world/src/clouds/kernel.rs#L338))
- Seed 1, the CRT default, for determinism; the reference's is the process `srand` state, and any uniform table looks the same. ([`src/clouds/tables.rs:47`](../crates/benilla-world/src/clouds/tables.rs#L47))

#### clutter

- ×2, `frillDensity` 32 (the panel's Medium), because a fresh install's `hwDetect` value (24 from `VideoHardware.dbc`, 8 on the weakest parts) is on no panel stop, and Medium is the nearest stop no sparser. ([`src/clutter.rs:158`](../crates/benilla-world/src/clutter.rs#L158))

#### collision

- The reference's camera trace is a bare ray (`0x672170`); the sphere keeps the near plane out of a wall. ([`src/collision.rs:56`](../crates/benilla-world/src/collision.rs#L56))
- The slide, step and snap are avian's kinematic controller, with parry's edge-hit normals, not the reference's resolver, because pieces of that resolver grafted onto the controller left the mover in states the reference never reaches; only its face filters, the facing law and the backface band, are ported. ([`src/collision/one_sided.rs:12`](../crates/benilla-world/src/collision/one_sided.rs#L12))
- The reference keeps the later record in file order on an exact tie (`0x6327e2`), and its box-and-cone volume meets a lip where our capsule does not, so a capsule hit on an edge two different front faces share is classified by its contact normal. ([`src/collision/one_sided.rs:54`](../crates/benilla-world/src/collision/one_sided.rs#L54))

#### doodad_anim

- Every instance scrolls in one phase, where the reference phases a band loop per arm and a gseq loop from each instance's attach (`CM2Model+0x68`), because one uniform serves the material and a seamless scroll looks the same at any phase. ([`src/doodad_anim/mat_anim.rs:186`](../crates/benilla-world/src/doodad_anim/mat_anim.rs#L186))
- This runs for every resident host, where the reference stops a doodad culled out of its scene list (`[CM2Scene+0x20]`) and re-arms it at once on re-link (`0x7074b0`, `0x683f80`), because gating it so would re-roll the whole field on the frame the camera turns back. ([`src/doodad_anim/mod.rs:339`](../crates/benilla-world/src/doodad_anim/mod.rs#L339))

#### entity_shade

- The lit outdoor target is intensity 1.0, not the reference's 2.5, because `wow_model.wgsl` caps the intensity at 1.0 (`min(I, 1)`) and a chase through the capped span would stall unseen. ([`src/entity_shade.rs:38`](../crates/benilla-world/src/entity_shade.rs#L38))

#### exterior_cull

- The reference tests one AABB per object, but a doodad placement has no root entity here, so its submeshes are tested one by one, and a submesh outside a doorway's window drops where the reference draws the doodad whole. ([`src/exterior_cull.rs:29`](../crates/benilla-world/src/exterior_cull.rs#L29))

#### ffx_glow

- Kept a float where every reference pack (`0x6cb0e2`/`0x6cb557`/`0x6cb9e1`) floors it to a byte (`x·255 + 512`, bits >> 14; `0x7ffe58`, `0x8029cc`), because the two differ by at most 0.002 (0.30 packs to 76, 0.2980). ([`src/ffx_glow.rs:176`](../crates/benilla-world/src/ffx_glow.rs#L176))

#### ground_fx

- The 1.12 client draws these flat quads as ordinary depth-tested M2 batches, so an up-slope buries them and a down-slope leaves them floating; here they drape the terrain like the selection ring, because an effect meant for ground level should sit on it. ([`src/ground_fx.rs:4`](../crates/benilla-world/src/ground_fx.rs#L4))

#### interact

- The broad bound is the mesh `Aabb` grown by 1 unit, not the reference's sequence-bounds sphere, because it can only err permissive. ([`src/interact/pick.rs:190`](../crates/benilla-world/src/interact/pick.rs#L190))

#### lighting

- The round trip's 8-bit quantization is skipped, because it moves a channel by under 0.5 %. ([`src/lighting/global_light.rs:83`](../crates/benilla-world/src/lighting/global_light.rs#L83))
- No 8-bit repack (`×255 + 512 >> 14`), because this light lane is float end to end. ([`src/lighting/resolve.rs:165`](../crates/benilla-world/src/lighting/resolve.rs#L165))

#### liquid

- Mote `i` draws cell `i & 7` of its set, because the reference reads its cell index at the bottom of the draw loop, an off-by-one that gives each mote the previous one's cell and slot 0 a hardcoded cell 8. ([`src/liquid/drift.rs:11`](../crates/benilla-world/src/liquid/drift.rs#L11))
- The water gust moves `speed·dt·60` a frame where the reference (`0x68e4f0`, `mode <= 1`) moves `speed` with no `dt`, because the reference's drift doubles from 30 to 60 fps; 60 Hz holds the top of the era's range at any frame rate. ([`src/liquid/drift.rs:58`](../crates/benilla-world/src/liquid/drift.rs#L58))
- Cell 12 is completed, because the initialiser's last store (`0x68efa5`) leaves its bottom-right corner at zero and one magma mote in four would smear across the atlas diagonal. ([`src/liquid/drift.rs:72`](../crates/benilla-world/src/liquid/drift.rs#L72))
- Floored at the frustum, because the cone stops containing the view at about 21:9, and a wider window would lose motes in bands down its sides. ([`src/liquid/drift.rs:95`](../crates/benilla-world/src/liquid/drift.rs#L95))
- Water frames have their per-frame mean flattened (`flatten_frame_dc`), because mipping turns the shipped frames' drifting means into a flicker on distant water; magma and slime keep theirs, the body's intended pulse. ([`src/liquid/surface.rs:459`](../crates/benilla-world/src/liquid/surface.rs#L459))

#### modkeys

- The reference (`0x4b7990`) retries a binding miss once without the leftmost modifier, so `CTRL-SHIFT-P` would reach `SHIFT-P` (`TOGGLECHARACTER3`, the pet paper doll); in a run with dev affordances `bindings::BindingDispatch::resolve` skips that retry on this plane, because a dev chord must open nothing in game. ([`src/modkeys.rs:22`](../crates/benilla-world/src/modkeys.rs#L22))

#### particles

- `AlphaTest` maps to `Alpha`, because flat `Spells\` quads are blend batches and a cutout would bite a soft decal's fade; and the part draws unlit, as no lit ground-fx part is known. ([`src/particles/buffer.rs:57`](../crates/benilla-world/src/particles/buffer.rs#L57))
- `w` is the frame dt clamped to 0.1 s, because we do not substep as the reference does at 0.1 s (`0x81d828`, `0x7b58e7`); they differ only on a hitch. ([`src/particles/sim.rs:162`](../crates/benilla-world/src/particles/sim.rs#L162))
- In model mode the reference adds the raw world vector to local coordinates; we fold it into the local frame, because our world axes are Bevy's, not WoW's. ([`src/particles/sim.rs:753`](../crates/benilla-world/src/particles/sim.rs#L753))

#### shaders

- The `min(I, 1)` cap, as in wow\_model.wgsl, since lifting it takes sun-facing surfaces past 1.0; Matte keeps its own bit so it stays 1.0 if the cap goes. ([`src/shaders/static_gx.wgsl:325`](../crates/benilla-world/src/shaders/static_gx.wgsl#L325))

#### sky_order

- One constant, not the reference's per-lane magnitudes (ring and shadow at one site, `0x6d7480`; footprints 10x, `0x69a54a`; foam 20x), because those suit its 24-bit fixed-point forward-Z buffer; in our reversed-Z float buffer the bias is ULP-relative, and one bake path sets one residual. ([`src/sky_order.rs:84`](../crates/benilla-world/src/sky_order.rs#L84))
- Only a few ULPs, a guard against drivers rounding coplanar arithmetic differently, because the reference's `1.29847e−3 · d²` yd pull paints the splash onto dry bank (0.3 to 1.0 yd at an 11.9-yd camera, measured). ([`src/sky_order.rs:113`](../crates/benilla-world/src/sky_order.rs#L113))

#### terrain_stream

- The debug free-fly eye holds nothing: it is benilla's own tool with no reference counterpart, and a stall there only slows the developer. ([`src/terrain_stream.rs:434`](../crates/benilla-world/src/terrain_stream.rs#L434))
- A tile is released only one chunk past the outer window (`StreamWindow::keeps`), where the reference releases by window membership (`0x6984f0`), because our reload is a re-decode and re-spawn, not a cheap async read. ([`src/terrain_stream.rs:775`](../crates/benilla-world/src/terrain_stream.rs#L775))
- A read still loading after `INNER_WAIT_CAP` is given up on with a warning, once per request, and its tile then streams on unwaited like the outer ring, where the reference waits for ever: a hung read costs one capped stall, not one every frame. ([`src/terrain_stream.rs:870`](../crates/benilla-world/src/terrain_stream.rs#L870))
- Benilla gates on terrain alone, which is more permissive; the settle hold covers a building or doodad arriving after a teleport. ([`src/terrain_stream/residency.rs:9`](../crates/benilla-world/src/terrain_stream/residency.rs#L9))
- Tagged per submesh, where the reference tests the whole object, because a placement has no root entity to carry the bound. ([`src/terrain_stream/spawn/mod.rs:229`](../crates/benilla-world/src/terrain_stream/spawn/mod.rs#L229))
- The box, where the reference tests its circumsphere (`0x683700` → `0x682ef0` → `0x686b80` on the `[rec+0x5c]` centre and `[rec+0x68]` radius × scale), because Bevy culls by box and the box is tighter. ([`src/terrain_stream/spawn/mod.rs:889`](../crates/benilla-world/src/terrain_stream/spawn/mod.rs#L889))
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
- A spawn ceiling on live plus pending drops, which the reference lacks, because each flake here costs a 4-vertex quad. ([`src/weather/precip/mod.rs:48`](../crates/benilla-world/src/weather/precip/mod.rs#L48))
- The drop rate is scaled to a 30 fps client, the frame rate the reference's look was matched at, because the reference budgets `min(dt, 1/60)·rate` and drops the remainder (`0x67846f`–`0x678480`), so it emits half its nominal rate at 30 fps. 60 is byte-faithful. ([`src/weather/precip/mod.rs:79`](../crates/benilla-world/src/weather/precip/mod.rs#L79))
- Flake pixels are pixels of an 800-px-tall screen, the height the look was matched at, converted to an angle, because the reference's literal framebuffer pixels shrink with resolution (2.7× smaller at 4K scale 2). ([`src/weather/precip/mod.rs:141`](../crates/benilla-world/src/weather/precip/mod.rs#L141))
- xorshift32 to \[0, 1) instead of the reference's lagged-table generator, because the laws need only a uniform distribution, not its exact stream. ([`src/weather/precip/mod.rs:175`](../crates/benilla-world/src/weather/precip/mod.rs#L175))

#### wmo_portal

- When legs A and B both miss and no terrain lies at or below the eye, Leg C runs the same race over the camera-only faces (DETAIL without NOCAMCOLLIDE), because the 1.12 client blanks the whole building from a pocket floored only with DETAIL faces, as at the Deadmines entrance. ([`src/wmo_portal/seed.rs:21`](../crates/benilla-world/src/wmo_portal/seed.rs#L21))

#### zfill

- The reference twins every translucent command, authored-blend glass included; here twins exist only during an instance alpha episode, so every steady-state look is untouched. ([`src/zfill.rs:9`](../crates/benilla-world/src/zfill.rs#L9))
- Bevy sorts per entity, so twins lead by a fixed −8 yd sort bias (`crate::model_render::ZFILL_SORT_BIAS`) instead of by the instance's sort-key tie; transparent content within that window behind a fading body is depth-clipped where the body covers it, for the episode. ([`src/zfill.rs:12`](../crates/benilla-world/src/zfill.rs#L12))

