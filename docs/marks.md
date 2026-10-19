# Issue marks

An issue mark saves the frame on screen, together with everything needed to get back to it.
Use one when you see something wrong while playing: a rendering glitch, a misplaced HUD
element or wrong text. It is a port debugging aid and is always available.

## Taking one

* **Keyboard:** press **F12**.
* **Gamepad:** hold **View + Y** together for about a second.

The game does not pause and nothing in it changes. A toast reading "Mark 3 saved" shows in the
window's top-right corner for two seconds, and the log prints the folder:

```
mark 3 saved: /home/you/.local/share/shuppatsu/marks/20261019-183012-3 (play, tick 4567)
```

Marks work everywhere: while driving, in the pause menu and result screens, in the menus and in
attract. The buttons of the pad combination also reach the game as usual. View is cancel, and Y
is the R key, which only the route select reads.

## Where they go

Each mark gets its own folder, `<YYYYMMDD-HHMMSS>-<n>` (UTC time, `n` counts the marks of this
session), in the user's data directory:

| System | Folder |
|---|---|
| Linux | `$XDG_DATA_HOME/shuppatsu/marks`, by default `~/.local/share/shuppatsu/marks` |
| Windows | `%APPDATA%\shuppatsu\marks` |

**Marks stay on your machine.** Nothing is uploaded or sent anywhere. To report an issue,
attach the folder, or the parts you want to share, yourself. The folder holds a picture of the
game and local paths: the data directory, the program and any font or texture-pack paths you set.

## What a mark holds

| File | |
|---|---|
| `screenshot.png` | the presented frame at the window's size, without the toast (uncompressed: about 4 bytes per pixel, 33 MB at 4K) |
| `mark.toml` | what the game knows at that frame (below) |
| `inputs.txt` | the input script up to the mark, for the default replay |
| `eeprom.dat` | the save that replay starts from |
| `session-inputs.txt`, `session-eeprom.dat` | for a mark in a run started from the menus: the whole session from start-up |
| `reproduce.sh` | the headless command that renders the marked tick again |

`mark.toml` has these sections:

* `[mark]`: number, time, version and git commit, the screen (`play`, `pause`,
  `station_result`, `title`, `route_select`, `attract_demo`, …) and the data directory.
* `[ticks]`: the play tick, the app tick (the session's ticks, runs included), the drive tick,
  the global tick counter, and `frame_alpha`, how far the shown frame was between the last two
  ticks (setting `interpolate`).
* `[run]`: stage, difficulty, route file, the start station, the current and next station with
  their names, the drive phase, paused or not, the 3D view, the time left and the continues.
* `[train]`: the position on the route (engine index and fraction, track units, metres),
  speed, notches, distance to the stop and gradient.
* `[camera]`: the cab camera's eye and target at the tick.
* `[input]`: the control mode, the device the hints follow and whether a pad is connected.
* `[window]`: size, full screen, the 2D canvas multiple and the canvas size.
* `[settings]`: every setting of [settings.md](settings.md) as `config.toml` would hold it
  (`(default)` for "follows the window" or "none"). `non_default` lists the ones changed
  from the defaults.
* `[reproduce]`: the replay commands, the same as in `reproduce.sh`.

## Replaying a mark

```sh
cd ~/.local/share/shuppatsu/marks/20261019-183012-3
./reproduce.sh            # writes replay.ppm
./reproduce.sh session    # marks in a run started from the menus: writes session.ppm
```

`reproduce.sh` runs the same program that took the mark. Set `SHUPPATSU=/path/to/shuppatsu`
to use another build. It clears every `SHUPPATSU_*` variable and passes every setting as a
flag, so the replay does not depend on your `config.toml` or environment. Headless runs never
read the config file anyway.

* **The run alone** (the default for a mark in a run):
  `shuppatsu play <stage> --difficulty … --keys <mode> --global-tick <n> --save eeprom.dat
  <settings> --size <window> --inputs inputs.txt --shot <play tick>:replay.ppm`. This starts
  the run the way the menus started it: with the save as it was then, the global tick counter
  (weather timing depends on it), the device the hints followed and the settings in effect.
  It only replays the run.
* **The whole session** (`./reproduce.sh session`, and the only replay for marks taken in the
  menus, attract or a stage intro): `shuppatsu --save session-eeprom.dat <settings at
  start-up> --size <window> --inputs session-inputs.txt --shot <app tick>:session.ppm`. This
  replays everything from start-up, menu navigation and settings changes included. It is
  exact, and slower: it replays the whole session, roughly a minute per hour played.

Both replays render the marked tick. The window may have shown a frame between two ticks
(`frame_alpha`), so camera motion can differ by up to one tick's travel. Everything else
matches pixel for pixel. The tests check this: `play::mark` drives a run with keys and a pad
and replays its recording tick by tick, and `app::mark` plays a session from start-up into a
run and replays both the session and the run alone. Both compare the drive state and the
HUD's pixels.

The flags the replays use are also available directly:

* `--save <eeprom.dat>`: start from this save, read only (it is never written).
* `--global-tick <n>` (`play`): the global tick counter at the start, as the menus leave it.
* `--inputs <script>`: the input script below.

### Limits

* With `hybrid_input` off, a controller in modes 4–6 is not recorded; such a replay has the
  keyboard only. The default (`hybrid_input` on) records the pad in every mode.
* The calibration screen's raw handle readings are not recorded.
* A run replayed alone starts with fresh hold counters. If a key or stick was held across the
  run's start, the first ticks can differ. The session replay has no such gap.
* A mark from `shuppatsu play --ending` / `--intro` (check aids) has no replay.

## Input scripts

`--inputs <file>` gives `shuppatsu` and `shuppatsu play` scripted inputs. Marks write them, and
they can be written by hand. There is one directive per line, and `#` starts a comment:

| Line | Meaning |
|---|---|
| `<tick> <notch> <brake>` | the oracle's notches, held from drive tick `tick` on (overrides the levers) |
| `key <tick> <name>` | the key is down for the poll at the end of tick `tick` |
| `key <from>-<to> <name>` | the key is held from `from` through `to` |
| `pad <tick> off` | from `tick` on, no gamepad |
| `pad <tick> <fields…>` | from `tick` on, the gamepad reads as given; tick 0 is its state at start-up |
| `hint <pad\|keyboard>` | the device the button hints follow at the start |

Key names: `esc enter space up down left right r a z lshift rshift 1 2 3 4 5 kp0`…`kp9
kpenter`. These are all the keys any keyboard mode reads.

The fields of a `pad` line are the Xbox-layout reading. Fields left out are at rest:

* the axes `lx= ly= rx= ry=` (sticks, −1…1, +y up) and `lt= rt=` (trigger travel, 0…1),
  written in full so a replay reads back the same values;
* the buttons `a b x y lb rb view menu up down left right` (the last four are the d-pad);
* `joy=<x>,<y>,<buttons hex>[,<power>,<brake>]`: the DirectInput state the pad gives the
  joystick decoder of modes 4–6 (`-` for a notch the device leaves alone).

A script with `pad` lines replaces the real gamepad for the whole run.

Ticks count polls. In `shuppatsu play` they are play ticks (pause, result and ending ticks
included). In `shuppatsu` they are app ticks from start-up, a run's ticks included. A mark's
script lists only changes: a key's press and release, and the pad's state whenever it
changes. A two-hour session stays small in memory and on disk.
