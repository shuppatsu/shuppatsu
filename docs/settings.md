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
  `true`/`false`, `on`/`off`; the two levels (`msaa`, `anisotropic`) take one of their steps
  (`DDG_MSAA=4`) or `off` (= 1).
* The command line: a switch is `--<key>` (on) / `--no-<key>` (off), with `-` for `_`
  (`--no-wide-weather`); the older names are kept as aliases (`--original-visibility`,
  `--original-weather`, `--original-traffic`, `--uncapped`). The last flag of a pair wins. A
  level is `--msaa 4` or `--msaa=4`, `--no-msaa` for off; in `config.toml` `msaa = 4`.
* Precedence: flag > environment > `config.toml` > default. The front end (`ddg-game`),
  `ddg-game play` and `ddg-game view` read the file; headless runs (`--shot`, the `shot`
  subcommand) never do, so checks stay reproducible with the defaults, the environment and
  the flags.
* A file that does not parse is reported and ignored; a bad value (`fog = 3`,
  `hud_layout = "wide"`) is reported and loses only that setting. Unknown keys are kept when
  the game writes the file (the first-run language screen writes `lang`, the settings screen
  any key it changes; comments, the other keys and a comment after the changed value stay;
  the file is replaced atomically).

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

## In the game: ポート設定 / Port settings

Title menu → ゲーム設定 (Game Settings) → **ポート設定 / Port settings**, a seventh row under
the original six. Decided as the least intrusive place: the title menu, its cursor positions
and the six options rows (positions, values, value pages, `Options_Set`) stay exactly the
original's; only moving the cursor below 距離メーター reaches the new row. With the analog
levers (control modes 5/6) the options cursor follows the power notch P0–P5 and cannot reach
a seventh row, so there the horn opens it (the original reads no horn on that page).
Code: `crates/ddg-game/src/front/port_settings.rs` (screen, items, values),
`App::apply_port_setting` in `app.rs`, `config::store` / `config::locks`.

* **Look**: the options page's header bar, menu backdrop and button legend (0x42a820 page 8)
  with the title replaced; rows of label and value in the label font (bundled M+ 2 Medium, or
  `font`), a blue cursor bar, gold group headings and ▲/▼ when rows are hidden; a panel under
  the list with the setting's one-line description, its status and the change legend for the
  control mode (keys, pad, levers). Strings: `[ui]` in `locales/en.toml` and `locales/ja.toml`
  (localisation.md); in Japanese the bundled font is installed for these rows only while the
  options screens are open.
* **Groups**: Language & text (`lang`, `one_line_captions`, `staff_kanji`, `landmark_names`, `board_names`,
  `subtitles`, `font`, `display_font`); Display (`hud_layout`, `ui_scaling`, `ui_scale`,
  `wide_menus`, `hud_upscale`, `interpolate`, `vsync`, `fullscreen`, `movie_scaling`); 3D & effects (`wide_visibility`, `square_pixels`,
  `stable_track_variants`, `texture_filtering`, `anisotropic`, `msaa`, `ground_variation`,
  `ground_detiling`, `texture_pack`, `fog`, `wide_weather`, `wide_coupling_band`, `far_traffic`, `max_lod`,
  `early_activation`, `draw_distance`, `distance_fade`, `fine_depth`); Sound (`audio_resampler`); Saves & controls (`save_on_change`,
  `save_timed_out_names`, `keyboard_with_controller`, `exit_no_to_menu`, `movie_in_menu`). All 41 keys of
  the table below.
* **Keys**: up/down move (no wrap, repeat on hold), left/right step the value (switches flip,
  `ui_scale` runs Auto, 1×…4×, the others wrap), Enter (confirm) steps it on, Esc (cancel)
  goes back to the options list on the new row. Pad: the same through its menu bindings.
  Levers: each notch the power handle is pushed on moves the cursor down, each notch the
  brake handle is pulled on moves it up; confirm changes the value.
* **Written at once**: every change goes to `config.toml` (`config::store`, atomic, keeping
  comments and unknown keys; `ui_scale` back to Auto removes its line). Headless runs
  (`--shot`) never write it, as they never read it.
* **When it applies** (the status line): *Applies now*: `lang` (the locale is installed again:
  sprite text, header art and these strings switch at once), `ui_scaling`, `ui_scale`,
  `wide_menus` (the canvas is refitted), `hud_upscale` (the canvas is composed every frame), `fullscreen`, `save_on_change`, `keyboard_with_controller` (the
  input is rebuilt), `exit_no_to_menu`, `movie_in_menu` and `movie_scaling` (read when the movie
  starts), `audio_resampler` (sent to the audio thread, heard from the next
  device buffer). *Applies from the next stage*: everything a run reads
  when it starts (the HUD layout, interpolation, the 3D and caption switches,
  `save_timed_out_names`). *Applies after a restart*: `vsync` (the surface's present mode is
  chosen when the window opens).
* **Locked**: a setting given by a flag or a `DDG_*` variable (they override the file) shows
  its value dimmed, without the ◀ ▶, and *Locked: set by --no-fog on the command line* /
  *Locked: set by the environment variable DDG_FOG*; changing it only plays the cancel sound.
  `font`, `display_font` and `texture_pack` are paths: shown, changed only in `config.toml`.
* Tests: `front::port_settings::tests` (navigation, scrolling, levers, value cycling, locks,
  the `config.toml` round trip of every key, both catalogues complete, drawing),
  `front::options::tests::port_settings_row`, `app::tests::port_settings_apply_live`
  (needs `DDG_DATA_DIR`).

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
| `texture_filtering` | off | `--[no-]texture-filtering` | nearest sampling, no mipmaps (shimmer) | CPU-built mipmaps and trilinear filtering of the 3D textures (blurs magnified texels); with `anisotropic` also anisotropic (renderer.md §14.4) |
| `anisotropic` | 1 (off) | `--anisotropic <1\|2\|4\|8\|16>`, `--no-anisotropic` | nearest sampling, no mipmaps: distant ground and track shimmer and alias | CPU-built mipmaps and anisotropic filtering up to N×. Without `texture_filtering`, magnified texels stay sharp squares (antialiased edges) and only minified ones are filtered; with it, plain trilinear + anisotropic. Mip levels where a sub-rect of the atlas is under 4 texels are never used (renderer.md §14.4) |
| `msaa` | 1 (off) | `--msaa <1\|2\|4\|8>`, `--no-msaa` | aliased polygon edges and colour-keyed cut-outs (wires, fences, trees) | the 3D pass multisampled N× and resolved (the HUD and 2D layers are not); the colour-keyed cut-outs use alpha to coverage from a sharpened bilinear key, so their edges are smoothed too. A count the GPU lacks falls back to the next lower one (renderer.md §14.3) |
| `ground_variation` | off | `--[no-]ground-variation` | ground textures repeat with an even brightness | ground surfaces (fields, snow, gravel and verge strips, ground slabs; not track, platforms, buildings, trains or signs) get a soft world-space brightness (mostly ±6 %, at most ±15 %) and warm/cool tint (±4 %) variation over 5–25 m, procedural in the shader (renderer.md §14.1–14.2) |
| `ground_detiling` | off | `--[no-]ground-detiling` | ground textures tile in a visible regular grid | the same ground surfaces sampled with per-region offsets and mirrors (blended at the region edges, keyed on world position), only along the axes where the texture repeats and wraps seamlessly, always inside its atlas rect (renderer.md §14.1, §14.5) |
| `texture_pack` | unset | `--texture-pack <dir>` | the textures of `tex*.rom` | replacement 3D texture pages (area and train `tex*.rom`) from a pack directory, any whole multiple of the original size; a page the pack lacks keeps the original; sampling stays nearest unless `texture_filtering` (docs/texture-packs.md; packs made with `ddg-tools upscale-textures`) |
| `fog` | on | `--[no-]fog` | the snow fog only with DengoSetting's FOGENABLE, which a fresh install lacks: no fog | the snow fog while it snows (`snow_fog`, renderer.md §13.9). `view --fog` is fog everywhere, a separate viewer option (off) |
| `wide_weather` | on | `--[no-]wide-weather`, `--original-weather` | raindrops and snow in the 4:3 area only (x 0..496) | over the whole visible width; at 4:3 identical to the original (renderer.md §13.9). Test `weather::tests::wide_weather_off_is_the_original_area` |
| `wide_coupling_band` | on | `--[no-]wide-coupling-band` | the coupling minigame's black band under its panel (virtual rows 268..400) only across the 4:3 width of the original's screen; the view goes on beside it on a wide window | the band across the whole visible width, so the bottom of the screen is black edge to edge under the centred panel; at 4:3 identical to the original (hud.md §3.16) |
| `far_traffic` | on | `--[no-]far-traffic`, `--original-traffic` | AI trains drawn up to 150 m ahead | up to the far plane (320 m, or `draw_distance`'s), the farthest LOD beyond 150 m; spawning and sounds unchanged (traffic.md §4, renderer.md §13.9) |
| `max_lod` | on | `--[no-]max-lod` | track pieces, AI trains and result-replay cars change to coarser models with distance (rails in 2–32-point bands re-cut every point, ballast and track-side kinds P0/P3/P4, cars LOD 1–4): visible LOD pop | the nearest band's model at every distance; whether something is drawn is unchanged (scene.md §5.3) |
| `early_activation` | on | `--[no-]early-activation` | scenery appears 40–200 m ahead (activation lists), side tracks when their bucket lists them, the main line ends 160 m ahead: pop-in inside the 320 m view | each object activated from where its bounding circle comes inside the view and the far plane; main line and side-track runs built out to the far plane; removals unchanged (scene.md §5.3) |
| `draw_distance` | `2x` | `--draw-distance <original\|2x\|3x\|max>` | far plane 32 units (320 m) | 640 m, 960 m or 1280 m (the panorama's distance); scenery, track and `far_traffic` out to it; the 2D background stays as far in front of the far plane as in the original (scene.md §5.3) |
| `distance_fade` | on | `--[no-]distance-fade` | the world ends at a hard edge (the far plane, or the panorama's depth 7.6 units before it) | the last quarter before that edge dissolves into the background (ordered dither) (scene.md §5.3) |
| `fine_depth` | on | `--[no-]fine-depth` | the 16-bit Z buffer: depth steps grow with the square of the distance (3 m at 320 m, 12 m at 640 m with `draw_distance`), so distant surfaces close together flicker | depth in steps of a constant 0.07 % of the distance (22 cm at 320 m, 87 cm at 1280 m). Either way the steps are whole and ties go to the later part in the original's draw order, so coplanar track and ground layers (crossovers, grass over the ballast's edge) show the later one drawn without z-fighting (renderer.md §6.1) |
| `interpolate` | on | `--[no-]interpolate` | everything moves at the 30 Hz tick | the camera along the track, AI trains, wiper, snow scroll and speedometer needle placed between the last two ticks at the display's rate (renderer.md §13.9); off: every frame shows the last tick |
| `vsync` | on | `--[no-]vsync`, `--uncapped` | — (DirectDraw flip) | presents with vsync (FIFO); off: uncapped |
| `fullscreen` | off | `--[no-]fullscreen`; Alt+Enter / F11 at any time | full screen at 640×480 (DirectDraw exclusive; windowed only with `-debug`, renderer.md §3) | borderless full screen on the window's monitor at its own resolution; off: a window. Alt+Enter and F11 switch it in every window (menus, play, view) and write the new state to `config.toml`, unless a flag or `DDG_FULLSCREEN` fixed it for the run |
| `wide_menus` | on | `--[no-]wide-menus` | the 640×480 menus and attract title picture centred over black | on a wide window the menu backdrops continue to the edges (their tile pattern repeated) and so does the attract title picture tbl 0x410 (paper synthesised from its plain edges, blended in at the seams), never stretched; at 4:3 pixel-identical (menus.md §14) |
| `hud_upscale` | off | `--[no-]hud-upscale` | blocks of whole pixels: each art pixel of the HUD and menu sprites an N×N square at the canvas multiple | xBRZ-shaped edges: at canvas multiples 2–4 each tilemap is enlarged as one image with index-preserving xBRZ (no new colours, no seams at tile borders), still drawn through the palette, so palette swaps, the animated speed readout, the route strip and fades are intact; other multiples, the rect layer, paper fills and translated text unchanged (docs/texture-packs.md, "HUD sprites") |
| `one_line_captions` | on | `--[no-]one-line-captions` | — (Japanese sprites) | with a translation: station captions and banners as one line of text instead of three sprites translated one by one (localisation.md) |
| `staff_kanji` | on | `--[no-]staff-kanji` | — (the credits are kanji) | with a translation and the staff roll as text (`one_line_captions`): each person's name in kanji after the English, on its line at the role size in a dimmer white (`[caption]` `staff_kanji` sets the form, bare `{kanji}` in en.toml); the names come out 23 px instead of 24 so every screen still fits (localisation.md, "Staff roll") |
| `landmark_names` | on | `--[no-]landmark-names` | — | with a translation: the landmark's English name under its kanji plate (localisation.md) |
| `board_names` | on | `--[no-]board-names` | — | with a translation: romaji under the route board's kanji (localisation.md) |
| `subtitles` | on | `--[no-]subtitles` | — (Japanese voices only) | with a translation: English subtitles for the spoken Japanese (station, signal and speed-limit calls, announcements, navigator, time-up advice), one or two lines centred above the bottom of the 3D view; never in Japanese (localisation.md, "Subtitles") |
| `save_on_change` | on | `--[no-]save-on-change` | `eeprom.dat` written only at exit (0x434f50) | also right after a ranking entry, a clear mark, an options change and a calibration, atomically (menus.md §14.1). Test `save::tests::without_save_on_change_only_the_exit_writes` |
| `save_timed_out_names` | on | `--[no-]save-timed-out-names` | a name entry that times out is shown but never packed (`Ranking_ToEeprom` 0x41e670 skipped): lost at exit | saved like a completed name (menus.md §14.1). Test `screens::ranking::tests::timeout_without_the_setting_is_not_committed` |
| `keyboard_with_controller` | on | `--[no-]keyboard-with-controller` | in controller modes 4–6 only Esc, Enter (menu confirm) and R reach the game | the keyboard's horn (Space) and A / ATS confirm (Enter) work too; the levers stay the controller's (input.md §10.3). Test `gamepad::tests::keyboard_buttons_off_is_the_original_mask` |
| `exit_no_to_menu` | on | `--[no-]exit-no-to-menu` | the exit dialog's いいえ restarts attract at the title picture | the title menu again, cursor on ゲーム終了 (menus.md §14.1). Test `app::tests::exit_no_without_the_setting_goes_to_attract` |
| `movie_in_menu` | on | `--[no-]movie-in-menu` | the title menu's sixth label, LOVE特急こまち (the Komachi movie), has no descriptor and no cursor position: the movie plays only in attract, once every select stage has a clear mark (menus.md §2–§3) | the label (the sheet's cell under the fifth) as the menu's last row, after 専用コントローラー設定 in control mode 6; choosing it stops the BGM, plays the movie and comes back on that row with the BGM, as the original's unreachable case 5 of 0x42a1c0 would. The art's number stays "6", so without mode 6 the rows read 1–4, 6. English: "Komachi Movie" over the big title, LOVE特急こまち as the subtitle. Off: the original's rows exactly. Test `front::title::tests::the_movie_row_follows_the_setting` |
| `audio_resampler` | `sinc` | `--audio-resampler <linear\|nearest\|sinc>` | every voice (8-bit effects at 9–44.1 kHz, pitch-shifted live; 44.1 kHz music) resampled by the Windows 9x DirectSound software mixer into a 22.05 kHz 8-bit primary buffer, nearest or linear: not known (audio-text.md §1, open question 8) | how each voice reaches the device rate: `linear` interpolation or `nearest` (sample-and-hold), the two candidates for the original, neither imitating its 22.05 kHz primary; `sinc` (default, the user's decision) is band-limited: a 32-tap Kaiser-windowed sinc whose cutoff follows the pitch, reading across loop seams, with images and aliases below −70 dB (on the 10 kHz voice lines linear leaves images at 5–10 kHz about 45 dB down, nearest about 24 dB; linear also dulls the top of each sample's band, −5 dB at 80 % of its Nyquist frequency, where sinc stays flat to 74 %), about 1.5 % of a core for 10 voices at 48 kHz (audio-text.md §9) |
| `movie_scaling` | `smooth` | `--movie-scaling <smooth\|sharp\|nearest>` | a DirectDraw stretch blit (`Blt`, no FX flags) of the 320×144 sample to 640×288: point-sampled by the HEL, filtered or not by the card's driver: not known (movie.md §5) | how the movie reaches its rectangle: `smooth` (default) = plain bilinear, suited to the low-resolution live-action picture; `sharp` = sharp bilinear like `ui_scaling smooth` (square pixels, blended edges); `nearest` = nearest neighbour. The rectangle itself follows `hud_layout` (original: the original's on the 640×480 design; enhanced: the window filled), always at the stream's 2.03:1 |

Not settings: the window size (`--size`), `--mute`, the controller files (`--joystick-dat`,
`--combo`) and the check aids (`--shot`, `--inputs`, `--speedometer`, `--ending`,
`--first-run`, `--debug-title`) are per-run options, not behaviours. The audio mixer's
options (`MixerConfig`: `master_volume`, `smooth_volume`, `interpolation`, audio-text.md §10)
are library options left at their defaults (the original's level, no ramps, linear
resampling, as the closest match to DirectSound is not known); they get keys with the audio
settings of the Phase 6 config UI.
