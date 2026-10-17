# Port settings

Every enhancement and every decided deviation from the original has one named setting
(CLAUDE.md: "every enhancement is a config option with its own name"). The defaults are the
user's decisions; turning a setting back gives the original's behaviour for that one thing
and leaves the rest alone. `eeprom.dat` keeps the original's own options byte for byte; these
settings live only in the port's `config.toml`.

## Where a setting comes from

* `config.toml` in the user's config dir, next to `eeprom.dat`: `$XDG_CONFIG_HOME/ddg/`
  (default `~/.config/ddg/`), `%APPDATA%\ddg\` on Windows. Every key is optional.
* The environment: `DDG_<KEY>` in capitals (`DDG_WIDE_WEATHER=0`); switches take `1`/`0`,
  `true`/`false`, `on`/`off`.
* The command line: a switch is `--<key>` (on) / `--no-<key>` (off), with `-` for `_`
  (`--no-wide-weather`); the older names are kept as aliases (`--original-visibility`,
  `--original-weather`, `--original-traffic`, `--uncapped`). The last flag of a pair wins.
* Precedence: flag > environment > `config.toml` > default. The front end (`ddg-game`),
  `ddg-game play` and `ddg-game view` read the file; headless runs (`--shot`, the `shot`
  subcommand) never do, so checks stay reproducible with the defaults, the environment and
  the flags.
* A file that does not parse is reported and ignored; a bad value (`fog = 3`,
  `hud_layout = "wide"`) is reported and loses only that setting. Unknown keys are kept when
  the game writes the file (it writes only `lang`, from the first-run language screen).

```toml
lang = "en"
hud_layout = "enhanced"
wide_weather = false      # rain and snow only in the 4:3 area, as the original
save_on_change = false    # eeprom.dat written only at exit, as the original
```

Code: `crates/ddg-game/src/settings.rs` (`Settings`: the switches, their env variables and
flags), `crates/ddg-game/src/config.rs` (`PortConfig`, `layered`) and `LocaleConfig` in
`crates/ddg-game/src/locale.rs` (language, fonts, layout, scaling). `main.rs` builds both once
and passes them down in the options (`AppOptions`, `PlayOptions`, the viewer's `Options`; the
keyboard switch in `PadOptions`, the save switch on `Save`); there are no globals. Tests:
`config::tests` (every key from the file, the environment and both flags; the precedence; the
defaults; this table lists every key) and one test per switch below where it is marked.

## Settings

| Key | Default | Flags | Original | Port (the setting on) |
|---|---|---|---|---|
| `lang` | asked on the first start (`ja` until then) | `--lang <code>` | Japanese art only | `en`: translated sprites and text over the original art (localisation.md); `ja` is the original art |
| `hud_layout` | `original` | `--hud-layout <original\|enhanced>` | the 640×480 HUD and menus, centred | `enhanced`: the driving HUD on a canvas of the window's shape with its groups at the edges (`wide_hud`, hud.md §6.4) and, with a translation, the horizontal bilingual station plates with their sizes and gaps (`wide_plates`, localisation.md); the details are part of this setting, with no keys of their own |
| `ui_scaling` | `smooth` | `--ui-scaling <smooth\|integer\|nearest>` | 640×480 (DirectDraw) | how the 2D canvas reaches the window: sharp bilinear, whole multiples centred, or plain nearest neighbour (localisation.md, "UI scaling") |
| `ui_scale` | follows the window | `--ui-scale <1..4>` | 1 | a fixed canvas multiple; translated text is rasterised at it |
| `font` | bundled M+ 2 Medium | `--font <files>` | — (no text renderer) | TTF/OTF list for translated text |
| `display_font` | the `font` list | `--display-font <files>` | — | the list for headers, menu titles and banners |
| `wide_visibility` | on | `--[no-]wide-visibility`, `--original-visibility` | scenery and side tracks from 4:3 visibility lists: pop-in at the edges of a wider view | the lists widened for the window's Hor+ view (at least 21:9) (scene.md §5.2) |
| `square_pixels` | off | `--[no-]square-pixels` | the 3D view stretched 7.9 % horizontally (renderer.md §5) | square pixels (narrower field of view at the same height) |
| `stable_track_variants` | off | `--[no-]stable-track-variants` | side-track single pieces swap model each time the camera passes a point (flicker) (scene.md §2.6) | the single-piece phase anchored to the run's slot (`run.slots.start & 3`): about half the change across a point, no swap with the camera |
| `texture_filtering` | off | `--[no-]texture-filtering` | nearest sampling, no mipmaps (shimmer) | CPU-built mipmaps, trilinear and anisotropic filtering |
| `fog` | on | `--[no-]fog` | the snow fog only with DengoSetting's FOGENABLE, which a fresh install lacks: no fog | the snow fog while it snows (`snow_fog`, renderer.md §13.9). `view --fog` is fog everywhere, a separate viewer option (off) |
| `wide_weather` | on | `--[no-]wide-weather`, `--original-weather` | raindrops and snow in the 4:3 area only (x 0..496) | over the whole visible width; at 4:3 identical to the original (renderer.md §13.9). Test `weather::tests::wide_weather_off_is_the_original_area` |
| `far_traffic` | on | `--[no-]far-traffic`, `--original-traffic` | AI trains drawn up to 150 m ahead | up to the far plane (320 m), the farthest LOD beyond 150 m; spawning and sounds unchanged (traffic.md §4, renderer.md §13.9) |
| `interpolate` | on | `--[no-]interpolate` | everything moves at the 30 Hz tick | the camera along the track, AI trains, wiper, snow scroll and speedometer needle placed between the last two ticks at the display's rate (renderer.md §13.9); off: every frame shows the last tick |
| `vsync` | on | `--[no-]vsync`, `--uncapped` | — (DirectDraw flip) | presents with vsync (FIFO); off: uncapped |
| `wide_menus` | on | `--[no-]wide-menus` | the 640×480 menus centred over black | on a wide window the menu backdrops continue to the edges, never stretched; at 4:3 pixel-identical (menus.md §14) |
| `one_line_captions` | on | `--[no-]one-line-captions` | — (Japanese sprites) | with a translation: station captions and banners as one line of text instead of three sprites translated one by one (localisation.md) |
| `landmark_names` | on | `--[no-]landmark-names` | — | with a translation: the landmark's English name under its kanji plate (localisation.md) |
| `board_names` | on | `--[no-]board-names` | — | with a translation: romaji under the route board's kanji (localisation.md) |
| `save_on_change` | on | `--[no-]save-on-change` | `eeprom.dat` written only at exit (0x434f50) | also right after a ranking entry, a clear mark, an options change and a calibration, atomically (menus.md §14.1). Test `save::tests::without_save_on_change_only_the_exit_writes` |
| `save_timed_out_names` | on | `--[no-]save-timed-out-names` | a name entry that times out is shown but never packed (`Ranking_ToEeprom` 0x41e670 skipped): lost at exit | saved like a completed name (menus.md §14.1). Test `screens::ranking::tests::timeout_without_the_setting_is_not_committed` |
| `keyboard_with_controller` | on | `--[no-]keyboard-with-controller` | in controller modes 4–6 only Esc, Enter (menu confirm) and R reach the game | the keyboard's horn (Space) and A / ATS confirm (Enter) work too; the levers stay the controller's (input.md §10.3). Test `gamepad::tests::keyboard_buttons_off_is_the_original_mask` |
| `exit_no_to_menu` | on | `--[no-]exit-no-to-menu` | the exit dialog's いいえ restarts attract at the title picture | the title menu again, cursor on ゲーム終了 (menus.md §14.1). Test `app::tests::exit_no_without_the_setting_goes_to_attract` |

Not settings: the window size (`--size`), `--mute`, the controller files (`--joystick-dat`,
`--combo`) and the check aids (`--shot`, `--inputs`, `--speedometer`, `--ending`,
`--first-run`, `--debug-title`) are per-run options, not behaviours. The audio mixer's
options (`MixerConfig`: `master_volume`, `smooth_volume`, `interpolation`, audio-text.md §10)
are library options left at their defaults (the original's level, no ramps, linear
resampling, as the closest match to DirectSound is not known); they get keys with the audio
settings of the Phase 6 config UI.
