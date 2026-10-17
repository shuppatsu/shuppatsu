# Localisation

Status: infrastructure and an English catalogue are in place. Translated text is **off by
default**; nothing changes unless a language and a font are configured. Not yet wired into the
game loop (see "Integration" below).

## Why it works this way

The original has no text renderer. Every string on screen (HUD labels, messages, signal
captions, menus, station names) is pre-rendered tile art: a `tbl.rom` tilemap of `cg.rom`
tiles, drawn through a sprite slot (docs/re/hud.md §4, menus.md §1.2, audio-text.md §7). Menu
labels are sub-rectangles cut out of large "string sheets" by 6-byte descriptors in the exe.
There are no Japanese strings to translate, so the art itself is the source text, and it is read
from the user's disc at runtime like everything else.

A translation therefore replaces the art of a sprite with text drawn into the same pixel
rectangle. The layout (positions, sizes, timing, palettes) stays the original's.

## Catalogue: `locales/<lang>.toml`

Keyed by the sprite's identity in the original:

* `[tbl]`: a `tbl.rom` tilemap id (`0x206 = "TIME"`), as a whole, or one `rect` of it in
  tiles.
* `[sheet.<tbl>]`: a label on a string sheet. Integer keys are descriptor indices into the
  sheet's table in the exe; the tables are listed with their addresses in `SHEETS`
  (`crates/ddg-game/src/locale.rs`) and read from the user's exe at runtime. Named keys with an
  explicit `rect` cover labels without a known descriptor (most screen headers, the
  replay mark).
* `[station_plate]`: the English name of each vertical station plate, keyed by the plate's
  tilemap id (`0x4e1 = "Yokohama"`). These are not sprite overrides: they feed the horizontal
  plate layout below.
* `[landmark]`: the English name of each landmark caption, keyed by the landmark sprite's
  tilemap id (`0x235 = "Yodo River"`). Not sprite overrides either: the kanji stay and the
  name goes on a line under them ("Landmark captions" below).

Values are a string, or a table with `text` and options:

| Option | Meaning |
|---|---|
| `region = [x, y, w, h]` | Replace only these pixels (relative to the sprite); the rest of the art stays. Used where text shares a sprite with gauges, digits, icons or Latin art. |
| `parts = [{ region, text }, ...]` | Several regions in one sprite (e.g. a label and a unit). |
| `align` | `center` (default), `left`, `right`. |
| `style` | `auto` (default), `plain` (lettering on transparency: the old art is removed), `boxed` (a plate or box: the backdrop is kept, the old lettering painted over). `auto` picks `boxed` when more than 35 % of the region's edge is opaque. |
| `vertical` | Rotate 90° clockwise (reads top to bottom). |
| `outline` | Plain style: 1-px outline in the art's second colour (default on). |
| `backdrop` | Boxed style: `art` (default, the backdrop as drawn) or `solid` (the backdrop's opaque colour fills the transparent half of a dither too, so the text never sits on the dither; the signal captions). |
| `group` | Parts of one entry with the same group name are drawn at one size, the smallest any of them fits at (the words of one callout cell). |
| `shared_group` | The same across entries: every part with this name, in any entry of any sheet or tilemap, is drawn at the smallest size any of them fits at (the items of one menu list: the title items `title.items`, the options labels and values, the route select's line names). Names are global, so they carry a screen prefix. |
| `font` | `body` (default) or `display`: which font list draws the text ("Fonts" below). On an entry it is the default of its parts; a part can set its own. |
| `line_height` | Lines this many font sizes apart instead of the font's own line height (e.g. `1.1` for two lines in the signals' caption band). |

`\n` breaks lines; the font size is fitted to the rectangle. Comments in `en.toml` give each
sprite's role, a short Japanese label for identification and the pixel size (decided: these
labels stay, as a translator's reference, the same practice as docs/re).

Colours always come from the slot's palette: a part stores palette *indices* picked from the
original art (the dominant colour for plain text and the second for its outline; for boxes the
backdrop's 2×2 pattern and the lettering's colour), so the palette swaps the game uses for
colour variants (white/yellow/red digits and labels, highlighted menu items) recolour the text
too.

Rules: `ja` is the identity (no overrides). Any sprite that is not in the catalogue, or is shown
through a sub-rectangle the catalogue does not name, draws the original art. A catalogue that
does not parse (unknown fields, bad keys, a descriptor index out of range, two entries for one
sprite) is an error, not a silent fallback.

## Configuration

| Setting | Env | Argument |
|---|---|---|
| Language (default `ja`) | `DDG_LANG` | `--lang <code>` |
| Body font list (TTF/OTF, `:`-separated: primary, then fallbacks; default: the bundled M+ 2 Medium, which is also appended as the last fallback) | `DDG_FONT` | `--font <files>` |
| Display font list (same form; default: the body list) | `DDG_DISPLAY_FONT` | `--display-font <files>` |
| Extra catalogue directory | `DDG_LOCALE_DIR` | `--locale-dir <dir>` |
| HUD layout: `original` (default) or `enhanced` | `DDG_HUD_LAYOUT` | `--hud-layout <layout>` |
| Presentation: `smooth` (default), `integer`, `nearest` | `DDG_UI_SCALING` | `--ui-scaling <mode>` |
| Fixed canvas multiple 1..4 (default: follows the window) | `DDG_UI_SCALE` | `--ui-scale <n>` |

These and every other port setting (one on/off switch per enhancement: `wide_weather`,
`save_on_change`, `one_line_captions`, …) are listed in one table in [settings.md](settings.md),
with their defaults, flags and what the original does instead.

### Settings: `config.toml` and the first-run language screen

The port's own settings live in `config.toml` next to `eeprom.dat`: `$XDG_CONFIG_HOME/ddg/`
(default `~/.config/ddg/`), `%APPDATA%\ddg\` on Windows. `eeprom.dat` keeps the original's
options byte for byte; `config.toml` holds only what the port adds. Every key is optional; the
full list is [settings.md](settings.md):

```toml
lang = "en"               # en, or ja for the original art
hud_layout = "original"   # original or enhanced
ui_scaling = "smooth"     # smooth, integer or nearest
ui_scale = 2              # fixed canvas multiple 1..4 (omit: follows the window)
font = "/path/a.otf:/path/b.ttf"
display_font = "/path/c.otf"
fog = true                # the snow fog (renderer.md §13.9); false = as a fresh install
wide_weather = true       # and the other switches of settings.md
```

* Precedence: command-line flag > `DDG_*` environment variable > `config.toml` > built-in
  default. Play, the front end and `view` read the file.
* Headless runs (`--shot`, the `shot` subcommand) never read it, so checks stay reproducible:
  they use the defaults, the environment and the flags.
* Unknown keys are ignored when read and kept when the game writes the file. A file that does
  not parse is reported and ignored (the defaults apply); a bad value (`hud_layout = "wide"`,
  `ui_scale = 9`) loses only that setting; a `lang` with no catalogue is dropped. The game never
  stops over the file.
* First run: when no language comes from a flag, the environment or the file, `ddg-game`
  (the front end) opens with a language screen before attract: "Language / 言語" over
  **English** and **日本語**, the title menu's cursor frame, up/down (or the power lever) and
  Enter / pad confirm, the menu move and decide sounds. The choice is installed at once (`en`,
  or `ja` = the original art) and written to `config.toml` as `lang` (an existing file keeps its
  other lines and comments; a corrupt one is moved to `config.toml.bad`). It sets only the
  language: the HUD layout keeps its own setting (default `original`).
* Changing it later: `--lang en|ja` for one run, or edit `lang` in `config.toml` (removing the
  line brings the screen back). An options entry comes with the Phase 6 config UI.
* Checking: the hidden flag `--first-run` forces the screen, also in headless shots
  (`ddg-game --first-run --size 1280x800 --shot 12:out.ppm`); headless runs never write the file.
* Code: `crates/ddg-game/src/config.rs` (`PortConfig`, `layered`, `store_lang`),
  `crates/ddg-game/src/front/language.rs` (the screen, a pure state machine), `App::choose_language`.
* `fog` (decided enhancement `snow_fog`, on by default): the fog that the original draws only
  while it snows and only with DengoSetting's FOGENABLE. `--no-fog` / `--fog` override it for
  one run; headless runs use the default (on). It is one of the switches of
  [settings.md](settings.md), which all work this way (`DDG_<KEY>`, `--<key>` / `--no-<key>`).

No font is shipped in the repo. Any TTF/OTF with Latin glyphs works; DejaVu Sans Bold or Noto
Sans Bold are good starting points (on NixOS: `fc-list | grep -i dejavu`). The catalogue uses a
few non-ASCII characters (macrons as in "Ōmagari", arrows "← →", the en dash), so the font
should cover Latin Extended-A and arrows. A language without a font keeps the original art and
prints a warning. Built-in catalogues (`en`) are compiled into the binary;
`<locale-dir>/<lang>.toml` overrides them, which is how a new language can be tried without a
rebuild.

## Fonts

Two font roles, each a `PATH`-style list (the primary font, then fallbacks for the characters
it lacks, e.g. the Japanese menu subtitles):

* **body** (`--font`): everything not marked otherwise;
* **display** (`--display-font`): text marked `font = "display"` in the catalogue, and the
  station banners (`[caption]` `banner_font = "display"`).

Without a display font, display text is drawn in the body font, so a single `--font` looks
exactly as before. The display list needs its own fallbacks: a Latin-only display font
followed by the body font (`--display-font Michroma-Regular.ttf:MPLUS1p-Black.ttf`) keeps
macrons and kana drawable. Free-text widgets (captions, banners, plates, landmarks, route
board) take the role from their `TextStyle::font`; `SpriteBank::set_display_label_font`
installs the display label font next to the body one.

Size groups (`group`, `shared_group`) span roles: a group's parts share one **capital
height** (the ink height of `H`), not one font size, so a display word and a body word in one
group still read as the same size although the fonts' proportions differ. With a single font
this is the same as one font size.

What `en.toml` marks as display, and why. The original's own Latin lettering (the small
"Arcade Mode" subtitles of the main menu) is a wide, square, technical face in the
Microgramma family, used for **titling**; the rest of its text is heavy Japanese gothic
lettering. The display role follows that split: display for short headings that name a
screen or announce an event, body for anything read as information, at small sizes, or next
to digits.

| Display | Why |
|---|---|
| Screen headers, sheet 0x54d (Select Line, Select Train, Main Menu, Controls, Difficulty, … Brake Lever Setup) | One title per screen, large, on the header bar. |
| Main-menu item titles, sheet 0x513 (Arcade Mode, Game Settings, Rankings, Quit Game, Calibration) | The original pairs them with its Microgramma subtitles. The Japanese subtitles that replace those stay body (the display font has no kana; body has them). |
| Station banners (`[caption]` `banner_font`) | The big centred yellow-on-navy announcement. The small green captions stay body. |
| Time-message headlines: 0x1be, 0x1ac Bonus, 0x1af, 0x1b0, 0x1b1, 0x1b2, 0x1c5, 0x1c6, 0x1c7, 0x1c8, 0x1ce, 0x258, 0x259 On Time | The popup's headline, the event itself. |
| Result header 0x1a0 | The station-result screen's title. |

Kept on the body font: HUD labels (gauges, distance panel, lamps), navigator hints, the
small captions, signal and sign captions, station plates, landmark names, the route board,
legends, options labels and values, dialogs, the report card, and the words around digits in
the time messages (0x1ad "Penalty", 0x1bd "sec"), which sit in the digits' line and are read
with them. A wide display face would also be too wide for most of these boxes and get
shrunk well below its neighbours.

## UI scaling

Decided by the user (enhancements). The 2D layer (sprites, HUD, menus, translated text) is
composed on the CPU into a canvas of 640×480 × the *canvas multiple* (`SpriteLayer::scale`),
widened to the window's shape in the enhanced HUD while driving and, in every layout, on the
menus (`wide_menus`, menus.md §14). Three settings, independent of
each other (`crates/ddg-game/src/presentation.rs`, `crates/ddg-render/src/overlay.rs`):

* **Layout** (`hud_layout`). `original`: the original 4:3 layout, centred, at any resolution
  and multiple. `enhanced`: the driving HUD on a canvas of the window's shape with its groups at
  the edges (`wide_hud`, hud.md §6.4) and the wider station plates, horizontal and bilingual
  with a translation (`wide_plates`, below). Until this split, `ui_scale` 2 and up switched
  the enhanced layout on; it no longer does.
* **Canvas multiple.** Follows the window height unless `--ui-scale` fixes it
  (`overlay::auto_multiple`, capped at 4). With `smooth`/`nearest` it is `ceil(h / 480)`, so
  text is always rasterised at or above the window's resolution and the presentation only
  shrinks, by 1/2 to 1 (800 → 2, 1080 → 3, 1440 → 3, 1600 → 4; 2160 → 4, enlarged ×1.125 by the
  cap). With `integer` it is `floor(h / 480)`, the largest canvas that fits whole (800 → 1,
  1080 → 2, 1440 → 3, 2160 → 4). A resize recomputes it every frame (cheap); only when it
  changes are the translated text sprites rasterised again (`presentation::rescale_text`, via
  `locale::install`), the canvas re-made and recomposed and the overlay texture resized, so a
  drag rebuilds at most once per multiple crossed. Label masks are cached per size.
* **Presentation** (`ui_scaling`), one factor for x and y, never stretched:
  * `smooth` (default): sharp bilinear in the overlay shader, no extra pass. Per axis the
    canvas is (conceptually) enlarged with nearest neighbour to the smallest whole multiple
    `p = ceil(target / canvas)` at least the target, then sampled with one bilinear tap: inside
    a canvas pixel the sample is its centre, and within `0.5 / p` of a pixel edge it ramps
    across (`overlay::sharp_bilinear_coord`). When the canvas is larger than the target
    (`p` = 1) this is plain bilinear. Every canvas pixel gets the same size; only edge pixels
    blend. Texels keep the original blit's alpha test (alpha < 0.5 is transparent) and are
    filtered premultiplied, in linear light, so edges blend with the 3D view without fringes.
  * `integer`: the largest whole multiple `k` of the canvas that fits, centred, unfiltered.
    The rest of the window shows the 3D view while driving and black around menus. The
    enhanced HUD canvas is `floor(width / k)` wide, so its edge anchors sit at the edges of the
    integer-scaled area; the 3D view's top edge moves down with the canvas
    (`presentation::band_top`) so the HUD band and the 3D band still meet. A window under
    480 high (k = 0) is fitted like `smooth`.
  * `nearest`: the earlier presentation, nearest neighbour at the fitting factor (uneven pixel
    sizes, dropped detail); kept for comparisons.

Play, the front end, `view --hud` and the headless `--shot` paths all use the same settings
(shots take the multiple from `--size`).

## Code

* `crates/ddg-render/src/text.rs`: `TextOverride`/`TextPart` (what to draw), `rasterise`
  (fontdue, fitted into a rectangle, never writes outside it), `TextOverlay::build` (renders
  every override against the bank's art once, at load), `GlyphSource` (fontdue or a test stub).
* `crates/ddg-render/src/sprites.rs`: `SpriteBank::set_text` installs an overlay. While
  composing, a slot whose `(tbl, resolved sub-rect)` has an override skips the art pixels inside
  the replaced regions and draws the text instead. With no overlay, composing is unchanged.
* `crates/ddg-game/src/locale.rs`: catalogue parsing, sheet descriptors from the exe,
  `Locale::lookup(tbl, sub_rect)`, `LocaleConfig` (env and arguments) and `install`.
* Free text and widgets: `SpriteLayer::widgets` (`crates/ddg-render/src/sprites.rs`) hold
  port-only elements of enhancements: a fill, a down arrow, a tilemap clipped to a rectangle
  at any uniform scale (original pixels per art pixel; at a fractional number of canvas
  pixels per art pixel each canvas pixel is the area average of the art under it, the art
  enlarged with nearest neighbour and box-filtered, so only edge pixels blend, like the
  `smooth` presentation; at a whole number it is the plain pixel blit), and text.
  `SpriteLayer::add_text(layer, rect, text, Paint { palette, index }, cap)` draws a string
  into a rectangle (original pixels; drawn at the canvas resolution of `ui_scale`) in a
  palette colour, with capitals `cap` pixels high or as large as fits, uniformly scaled.
  Widgets are drawn after the slots of their layer, are cleared with the rect layer every
  tick, and are empty in the arcade default.
  The text uses `crates/ddg-render/src/label.rs` (`LabelFont`: the language's font, installed
  by `install` with `SpriteBank::set_label_font`, masks cached per string and size).
  `SpriteLayer::add_styled_text` takes a `TextStyle` with an alignment, an outline colour
  (one original pixel around the text) and a backdrop colour (a box behind the text), used by
  the one-line captions. Widgets and fans take an edge anchor (`SpriteLayer::anchored`) for the
  wide HUD (hud.md §6.4).

## Station plates by HUD layout (enhancement `wide_plates`)

The original's station plates (hud.md §3.7) are two 32×96 plates with vertical kanji in the
top right, "from" (slot 0x83 at (493, 8)) and "next stop" (slot 0x85 at (552, 8)), with the
arrow 0x4b8 (slot 0x84 at (530, 48)) between them.

Decided by the user (enhancement):

* **The `original` HUD layout**: the original vertical plates with the Japanese art only, in
  every language. No horizontal plates, no romaji; the whole HUD is the original 4:3 layout,
  centred. (Japanese at multiple 1 is pixel-identical to the arcade default.)
* **Japanese keeps the original vertical plates in every layout** (they are its own
  script; only the HUD canvas is larger and, with `wide_hud`, the groups keep to the edges).
* **A translation in the `enhanced` layout** (`LocaleConfig::active`: a language other than
  `ja` with a font): the plates are laid out horizontally
  (`crates/ddg-game/src/hud/plates_en.rs`), kanji with the translated name under them. The
  boxes grow on a wide canvas (enhancement `wide_hud`, hud.md §6.4). The vehicle picture's
  shift (below) belongs to this layout: with the vertical plates it stays where the layout
  table puts it.

640×480 coordinates, with `extra` = the widening (below):

| Element | Rectangle | Content |
|---|---|---|
| Backdrop | (490 − extra, 7) (98 + extra)×100 | the plates' black |
| "From" box | (492 − extra, 10) (94 + extra)×42 | #303030, the kanji name and the English name under it |
| Arrow | centred on the boxes, y 53, 11×9 | ▼ in the plates' yellow |
| "Next stop" box | (492 − extra, 62) (94 + extra)×42 | as "from" |

* **Width.** The plates are in the right group of the wide HUD. They grow to the left into the
  room that group gains over the centred 4:3 layout, up to 56 px: `extra = min(56,
  floor(side room))`, where the side room is half the canvas widening in 640×480 pixels
  (`SpriteLayer::side_room`). The vehicle picture (slot 0x1e, same group) moves left by the
  same amount, so nothing overlaps the centred remaining-time readout. At 16:10 the side room
  is 64 px, so the boxes are 150 px wide (1.6×); at 21:9 also 150. **At 4:3 there is no room**:
  the boxes stay 94 px wide.
* **Gap to the remaining time** (decided by the user): the vehicle picture keeps at least
  `MIN_GAP` = 4 px (640×480) clear of the remaining-time panel at every aspect from 4:3 up
  (`plates_en::RightGroup::fit`). Both edges are measured in the art at load: the leftmost ink
  of every vehicle picture of the cab table at the picture's 4:3 place, and the rightmost ink
  of 秒 (tilemaps 0x40d and 0x40e at the 秒 slot's x). Relative to the right group the panel is
  `side room` px further left, so the widening is capped at `floor(picture − panel − 4 + side
  room)`; when even the unwidened layout would come closer, the group (picture and plates
  together) is made uniformly smaller about the backdrop's top right corner (588, 7), just
  enough, the picture then drawn as a resampled sprite widget (its slot parked off screen like
  the plates'). Never stretched. With the data this never triggers: the pictures' ink starts
  at x 411 and 秒's ends at 405 (exclusive; the visible border at 403), 6 px at 4:3, and the
  gap only grows with the side room (exactly 6 + the fraction of the side room up to 16:10,
  14 at 16:10, more beyond), so every layout is unchanged. (An earlier version of this note
  said the picture touches 秒 at 4:3; measured, it does not.) A test checks the numbers
  against the data (`hud::tests::right_group_gap_from_the_data`).
* The kanji are the original horizontal station-name sprites (`HudTables::station_names`,
  `plain`, the captions' art), not a font. Their lettering index (0x1c, green in palette 4) is
  recoloured to the plates' yellow (palette 4 index 0xa) in a port-only palette, slot 0x7f,
  rebuilt every tick from palette 4; the box grey goes into an index the name sprites do not
  use. All indices are found in the art at load (the plates' commonest index is the backdrop,
  the second their lettering; the name sprites' commonest other than the backdrop is theirs).
* The sprite has a fixed size relative to the boxes at every canvas multiple (decided by the
  user): `KANJI_SCALE` = 0.5 original px per art pixel (half the original height, 14 px of
  640×480 for the common ink rows, a third of the 42-px box, like the other HUD lettering),
  the size it had at multiple 2. That is 1 canvas pixel per art pixel at multiple 2 and 2 at
  4 (plain pixel blits, unchanged), 1.5 at 3 and 0.5 at 1, drawn with the widget's area
  filter (previously 2 at multiple 3, a third larger, and 1 at multiple 1, twice as large).
  The palette recolouring is unchanged (the filter averages the recoloured palette colours).
  A name wider than the box is centred and clipped at both ends; since the size is relative,
  one list holds at every multiple: **no name is clipped**, even in a 94-px box (the widest,
  Hokuhoku-Ōshima, plate 0x4cf, is 164 art px = 82 of 94 px).
* The English name is `[station_plate]` text under the kanji, capitals 5 px high in a 94-px
  box and up to 7 px in a 150-px one (smaller if a long name needs it), centred, in the
  plates' yellow.
* State: the plate slots are set and cleared exactly as in arcade mode but parked off screen;
  each tick the HUD draws the boxes from the slots' current images, so they appear and
  disappear with the original plates. With neither plate shown nothing is drawn.

## Captions and banners (enhancement `one_line_captions`)

Setting `one_line_captions` (on; `--no-one-line-captions` keeps the translated sprites,
[settings.md](settings.md)).

The station captions (slots 0x36–0x38) and banners (0x39–0x3b) are each three sprites:
name + 駅 + word (通過 / 停車 / 出発進行 …), hud.md §3.13. Translated sprite by sprite, each
word is fitted to its own box, so the long name comes out small and "Stn" / "Stop" big.

Decided by the user (enhancement, translations only; Japanese is unchanged): with a
translation and a font, each caption or banner is **one line of text at one size** across the
group's area, e.g. "Higashi-Kanagawa · Stop" ("Stn" is dropped). `crates/ddg-game/src/
captions.rs` (`OneLine`):

* **Phrases** are `[caption]` in the catalogue, by kind: `depart_clear`, `depart_reduce`,
  `depart_caution`, `depart_restrict`, `pass`, `stop` and `name` (kind 6, the bare name).
  `{name}` is the station's name: the `[tbl]` text of the station's plain name sprite (the
  existing name table, e.g. `0xe6 = "Higashi-Kanagawa"`). A kind without a phrase, or a
  station without a name, keeps the three sprites.
* **Drawing.** The tasks, their timing (6 hidden updates, then distance or tick mode), the
  time-message hiding and the cancelling are the original's; only the drawing differs. The
  three slots stay empty and the task adds a text widget each update it would have drawn
  them. Captions are left-aligned from the caption's x (80) to the same margin on the right;
  banners are centred on the original's centre (x 320), at most 560 px wide.
* **Size**: capitals 0.75 × the original lettering height (`rasterise_sized`'s rule), measured
  from the 駅 sprite of the set; smaller only when the line does not fit.
* **Colours** come from palette 4 at draw time, so palette changes apply as on the art. The
  captions' 駅 art is plain: the line is its commonest colour (green) with a one-pixel outline
  in its second (dark). The banners' art is on a box (most of its edge is opaque): the line is
  the commonest colour that is not the box's (yellow) on a box of the box colour (navy), the
  sprite's full height and as wide as the text plus 4 px each side.
* **Fonts**: captions in the `[caption]` `font` role, banners in `banner_font` (`en.toml`:
  body and display, "Fonts" above).
* The landmark captions have their own enhancement (below); the time messages (word +
  digits) are not changed.

## Landmark captions (enhancement `landmark_names`)

Setting `landmark_names` (on; `--no-landmark-names` shows the plate alone,
[settings.md](settings.md)).

The landmark captions (course event 17, `Landmark_Show` 0x410a70 → task 0x410bd0, slot 0x3c,
hud.md §3.13) are 30 white plates naming bridges, rivers, roads and signal stations, one
sprite each (tbl 0x222–0x23f, widths in the exe table 0x466a20).

Decided by the user (enhancement, translations only; Japanese is unchanged): **Japanese on
top, English below.** With a translation and a font, the original kanji plate is drawn
exactly as in the original, and the `[landmark]` name is drawn on a small line directly under
it (`crates/ddg-game/src/captions.rs`, `LandmarkLines`):

* **Look.** The plate's colours are read from the art at load (`Look`: most of the sprite's
  edge is opaque, so its commonest edge colour is the plate, white; the commonest other
  colour is the lettering, black), from palette 4 at draw time. The plate is extended
  downwards by the line: a fill of the plate's own columns (its opaque columns in the art),
  starting at its bottom row, and the text on a box of the plate colour (text width + 4 px
  each side), so a name longer than the plate widens the extension around its centre.
* **Size.** Capitals 6.5 original pixels high (13 canvas pixels at `ui_scale` 2), but at
  least 8 canvas pixels (8 original pixels at `ui_scale` 1); the line is the capital height
  + 4.5 px. Drawn at the `ui_scale` resolution with the sprite layer's text widget
  (`SpriteLayer::add_styled_text`), centred under the plate, at most 300 px wide.
* **Timing.** The line belongs to the landmark task: it waits the same 6 updates, is drawn
  every update the sprite is, hides with it under a time message and ends with it (distance
  run out, cancel, arrival or time-up).
* **Anchor.** Drawn with slot 0x3c's anchor (centred; `hud::anchor_slots` leaves the
  captions in the middle on a wide canvas).
* A landmark without a `[landmark]` name, or Japanese, shows the sprite alone.

## Route board (enhancement `board_names`)

Setting `board_names` (on; `--no-board-names` keeps the kanji only, [settings.md](settings.md)).

Before departure the board of hud.md §3.7 / §5 shows the route diagram (slot 0x7b, one sprite
per stage and stations served, e.g. 0x32a 秋田…大曲, 0x31e 直江津…犀潟): a white board with the
line and its station dots, and under each dot the station's name in vertical kanji (black
for stops, grey for stations passed).

With a translation and a font (enhancement, translations only; Japanese is unchanged) the
kanji stay and each station's romaji is drawn under its column
(`crates/ddg-game/src/board_en.rs`):

* **Columns from the art.** At load every diagram the strip tables name is scanned: the board
  is its commonest colour, rows and columns opaque almost throughout are its frame, the first
  rows inked over half the width are the line, the names start at the first blank row below
  it. Ink columns closer than 6 px are one name. Each column keeps its ink extent, its last
  kanji row and its lettering colour (black or grey), which the romaji uses (palette 4).
* **Stations.** A diagram starts at the segment's start station: column `i` is station
  `station + i` of the stage (checked on stages 0, 5, 10, 12 and 19). The names are the
  station-name texts the one-line captions use (`[tbl]` of the plain name sprites), so a
  passed station without a plate (黒井 Kuroi) has one too.
* **Placement**, left to right: the first line under the column's own kanji, centred on the
  column, that touches no other column's kanji and no name placed before (on a crowded board
  such as 秋田–大曲 neighbouring names step down in turn); else broken at a hyphen or space onto
  two lines (Kita-/Ōmagari); a name whose kanji reach the bottom edge (羽後四ッ屋) is turned
  90° beside them, in the gap to the next column; failing all that it is drawn smaller (down
  to 70 %), and last of all narrowed into the room directly under its kanji.
* **Size.** Capitals 5.5 original pixels high (11 canvas pixels at `ui_scale` 2), at least 6
  canvas pixels; drawn at the `ui_scale` resolution with the sprite layer's text widget.
* **Timing.** Drawn every update while slot 0x7b shows a diagram, with its anchor, so the
  names come and go with the board (removed at departure tick `offset + 100`).

## Integration

Nothing calls this yet. Three edits turn it on for `ddg-game play`:

1. `main.rs`, `parse_play_args`: take the locale options out of the argument list before the
   option loop, and keep them in the options (`PlayOptions` gets a
   `pub locale: crate::locale::LocaleConfig` field):

   ```rust
   let mut args: Vec<String> = std::env::args().skip(2).collect();
   // config.toml (unless headless), then the environment, then the flags it removes
   let locale = config::locale(&mut args, headless)?;
   // ... PlayOptions { ..., locale, }
   ```

2. `play.rs`, `run`: make the bank mutable and install the overlay right after it is built
   (`exe` is already parsed there):

   ```rust
   let mut bank = SpriteBank::new(/* as now */);
   let n = crate::locale::install(&opts.locale, &exe, &mut bank)?;
   if n > 0 {
       eprintln!("{}: {n} sprites translated", opts.locale.lang);
   }
   ```

3. Optionally the same `install` call in `HudView::load` (`main.rs`, the `view --hud` test
   HUD), and the three options in `USAGE`.

`install` is a no-op for `ja` and for a language without a font, so the call can be
unconditional. Composing stays `layer.compose(&bank, &mut canvas)`. `locale.rs` carries a
module-level `#![allow(dead_code)]` until these calls exist; remove it then.

## Coverage of `en.toml`

599 sprite entries (483 tilemaps and 116 string-sheet labels), 65 station-plate names and 30
landmark names.

* HUD: time labels, remaining time, line/vehicle plates (15), distance-panel headers (m and
  cm), handle labels, ATS lamp, cab-signal lamp, all 15 time messages with their sign and unit
  words, all 34 navigator hints plus the two horn variants, every signal caption (3-lamp,
  4-lamp, repeater, emergency: 37), every sign caption (27), station captions/banners and
  departure words, 85 station names (plain and highlighted), the 30 landmarks (`[landmark]`,
  under the kanji) and 通過, the progress strips, all-clear, emergency-brake, time-up and its
  9 advice balloons, the coupling bonus game.
* Screens: station result rows, continue, run end (both grades), report card, lever check,
  departure board (header and all 37 flaps), route select (groups, difficulty, rank badges,
  Shizukuishi tags), records (header, 7 line and 13 series plates, legends), press START,
  the seven control-mode diagrams of Options → Controls (callouts only; the controller
  pictures 0x493–0x498 and 0x4a1 are not text).
* Sheets: title items (5), options labels and values (2 × 31), pause/dialog texts (27), screen
  headers (13), name-entry legends (3), legend words (5: 選択 / キャンセル / 決定 / おわり /
  モード切替 by descriptor, 0x47c538; the key pictures beside them stay) and the replay mark.

Not catalogued, on purpose:

* Latin art that needs no translation: `great!` 0x1aa, `good` 0x1ab, `Professional` 0x2c7,
  `PRO` 0x2c8, `ATC` 0x3f, `888 km/h` 0x53, `EXCELLENT!` 0x1cd, `PAUSE` (0x544 #1),
  `ALL ROUND CLEAR`/`GAME OVER`/`CONTINUE` parts (kept by regions).
* Pictures and gauges with only numbers or symbols: brake B1–B8, power P1–P5, digits, stars,
  key/pad glyphs (0x4a6, 0x503), the name-entry symbol wheel (its katakana are input symbols),
  `〜` 0x30d.
* Text baked into pictures where replacing it would damage the art: the E3 coupling animation
  0x163–0x166, the EF63 shock meter 0x2da and the tiny 車内信号 label in hint picture 0x13e.
  The vertical route diagrams of the pre-departure screen (0x31e–0x325, 0x329, 0x32a…) keep
  their kanji and get romaji under them ("Route board" above).
* Arcade-only, unreachable on PC: insert-coin texts 0x253, 0x254.

Regions marked `(est.)` in `en.toml` were measured by eye and should be checked in the running
game.

## Decided: a font, M+ 2 Medium

The user chose a font (option 1 below) after comparing 13 candidates on the enhanced HUD at
2560×1600 (M PLUS 1p, M+ 2, PixelMplus, Archivo Black, Unifont, Michroma, Noto Sans JP, Zen
Kaku Gothic New, BIZ UDPGothic, Inter, Source Sans 3; heavy and regular weights; a Michroma
display hybrid). **M+ 2 Medium** (OFL) is the body font: a humanist sans in the spirit of the
Sony handheld system fonts, compact enough for long station names, with its own Japanese, and
Medium holds up next to the bold 1999 lettering where Regular looked faint. No display font is
set (the display role falls back to the body font); `--display-font` stays available.

The options that were considered:

1. **A font** (what this infrastructure does): any TTF the user supplies, fitted into the
   original rectangles, coloured from the original palettes. Cheap, covers every language,
   but the look is a modern vector font next to 1999 pixel art, and small rectangles (station
   tags 48×32, distance words 24×24, 16-px labels) get very small text.
2. **Redrawn sprite art in the original style**: our own pixel lettering, drawn by hand or by a
   tool, shipped in the repo (it is not derived from game data). Faithful look, but a lot of art
   per language, and every change of wording means redrawing.

Nothing here closes off option 2: the catalogue keys (tbl id / sheet descriptor / region) and
the per-slot hook in `SpriteBank` are exactly what a redrawn-art path needs. It would add an
alternative to `rasterise` that blits a stored indexed bitmap into the same `RenderedPart`
(palette indices plus coverage), so palette swaps keep working. A middle ground is a bitmap
pixel font (our own, in the repo) rendered without antialiasing, which keeps the crisp look and
still covers any language.

Smaller open points:

* Wording style (decided): no all-caps text. Labels, headers, menu items, banners and signal
  captions use title case ("Time Left", "Main Menu"); messages, hints and exclamations use
  sentence case ("Use the emergency brake!"). Only acronyms and model names stay in capitals
  (ATS, ATC, OK, HK100). Railway terms follow JR English usage (Clear, Caution,
  Restricted, Reduce, Stop; home/starting/repeater signals; "series" for rolling stock).
* The time-up advice and hint texts paraphrase the Japanese to fit three short lines.
* Station names use Hepburn with macrons everywhere (decided): Ōmagari, Jingūji, Tōkyō, Ōsaka,
  Kōbe.

## Subtitles (setting `subtitles`)

Decided by the user: the spoken Japanese gets English subtitles, **on by default** with a
translation (`subtitles = true`, `--[no-]subtitles`, `DDG_SUBTITLES`; never in Japanese, where
`LocaleConfig::active` is false). Code: `crates/ddg-game/src/subtitles.rs`, the `[subtitle]`
section of `locales/en.toml`, and the voice log of `ddg_audio::SoundSystem`
(`set_voice_log`, `ddg_audio::voice`).

**How a line's meaning is found.** Not by listening or transcribing: the sound system records
every voice line it starts together with the call that built it (`Origin`: a single code, a
table sequence, the approach pair call 0x41ffe0, the speed call 0x420180, the signal call
0x420320, the announcer call 0x420040), and the meaning follows from the call's arguments and
the event that made it (audio-text.md §5, §10):

* station names: the pair table at 0x46c368 has one recording per station-name id (entries
  5..89 the driver's voice, 95..179 the announcer's), the same ids as the HUD's name sprites
  (`0x856d58`), so `{station}` is the name sprite's translation. The data test checks that
  every station of every stage resolves;
* the approach call (state 1 → 2 of 0x856d68) is pair `name` + pair 2 / 3, played with the
  banner "name 駅 通過 / 停車": 0xbf = 通過 (passing), 0xbd = 停車 (stopping);
* signal calls: 0x420320's word tables by index (kind 1 home, 2 starting; aspect words 0..6 =
  進行, 減速, 注意, 警戒, 停止, 高速進行, 制限 by the aspect the HUD shows with them). Kind 0
  (0xd1) is reached only from the repeater call of 0x40c010, so it is "repeater" (correcting
  audio-text.md §10.3's "閉塞"); the repeater's prefix 1 / 2 names the repeated signal;
* speed calls: 制限 + the number phrase indexed by km/h / 5, 0xc3 for 0 km/h ("lifted", the
  call made with the 制限解除 sign);
* numbered block calls: table sequences 5..0x10 = number phrase 0x9b + k + the aspect word,
  started for blocks 3..9 counted down to the home signal (block 3 is the first block signal
  outside it, 第1閉塞 by JR numbering: **likely**);
* in-train announcements: "next" (0x111 / 0x147 / 0x168, as 0x420040 builds it) + an
  announcer station code; the station is confirmed by where the course event fires
  (sequence 0x2a at the leg to Amagasaki, 0x2d to Kōbe, 0x13 / 0x16 the Hokuhoku stops).

**Display.** One line per voice line, white M+ 2 Medium (body font) with a dark outline, in a
port palette (0x7e), centred in the centre group, the newest line's box ending at y 342
(640×480 space), an older one directly above: clear of the speedometer (y ≥ 345), the
navigator's portrait and balloon (y ≥ 376), the distance panel (y ≥ 408), the time-up advice
(y ≥ 344) and the signal (x ≥ 528). Checked in both HUD layouts at 4:3, 16:10 and 21:9
(out/explain/subtitles/). A line comes up when its first spoken phrase starts (sequencer
timing, `step_offsets`) and stays for its phrases' length from `effect.rom` + 10 ticks, at
least 30 ticks + 2 per character; at most two lines, the oldest goes; a line already up is
only kept longer; a stop-all or the sequencer switched off (end of a stop, the results)
clears them. Headless and muted runs run a silent sound system for the log.

### Voice codes the game plays while driving

Confidence: **confirmed** = from the call structure and the HUD text shown with it;
**likely** = from structure and context; **description** = the event is known but not the
words, so the subtitle describes it in brackets; **not identified** = no subtitle, for the
user to review by listening.

| Code / call | Played by | Subtitle | Confidence |
|---|---|---|---|
| pair `name` + 0xbd / 0xbf | approach call at the banner (0x40c4e0) | "{station}: stopping." / "…passing through." | confirmed |
| 0x42..0x96 | the driver's station names (pairs 5..89) | part of the above | confirmed |
| 0x420040 modes 0 / 2 | course event 2, ~600 m before | "The next station is {station}." | likely ("next" by structure) |
| 0x420040 mode 1 | course event 2 at 100 m | "{station}. {station}." | confirmed (name twice) |
| 0xff..0x167 (pairs 95..179) | the announcer's station names | part of the above | confirmed |
| 0x111/0x112, 0x147/0x148, 0x168/0x169 | "next" / closing phrase of 0x420040 | part of the above | likely |
| 0x1a4, 0x1a7 | chimes before announcements | — (music) | confirmed |
| 0x420320 kind + aspect (0xc1 home, 0xc0 starting, 0xd1 repeater; 0xa7 進行, 0xa8 減速, 0xa9 注意, 0xaa 警戒, 0xd2 停止, 0xbe 高速進行, 0xc2 制限) | signal sightings and repeaters (0x40c010) | "{Home signal / Starting signal / Repeater / Home repeater / Starting repeater}: {aspect}." | confirmed (kinds, aspects); 0xd1 likely |
| 0xd4 | prefix 3 of 0x420320, sightings on passing routes when the station is passed | left out of the line | **not identified** |
| 0xa8, 0xa9, 0xaa, 0xd2 alone | block signal aspects (0x40c010) | "Signal: Reduce." etc. | confirmed |
| 0x97 | green at block 0, departure (aspect 0), recovery calls | "Starting signal: Clear." | confirmed (出発進行, banner 0x6b) |
| 0x99 | green at blocks 1–2 (home signal) | "Home signal: Clear." | likely |
| 0x98 | green from block 8 on | "Block signal: Clear." | likely |
| sequences 5..0xb (0x9b..0xa1 + 0xa7) | green at blocks 3..7 | "Block signal {n}: Clear." | likely (the number n) |
| 0xbe | green with the high-speed display variant (course event 15, Hokuhoku) | "Signal: High-speed clear." | likely |
| 0xce, 0xcf, 0xd0 | departure by aspect 1–3 (0x413370) | "Starting signal: Reduce / Caution / Restricted." | confirmed (captions 出発減速/注意/警戒) |
| 0xc2 + 0xae..0xbb, 0xc4..0xcd | speed-limit call 10..125 km/h | "Speed limit {kmh} km/h." | confirmed |
| 0xc2 + 0xc3 | limit lifted | "Speed limit lifted." | confirmed |
| 0xc2 alone | limit kind 4 on stages 0–2, or no number phrase | "Speed limit." | confirmed |
| 0xbc | on-time pass (time message 13 定通) | "On-time pass." | confirmed |
| 0xd6, 0xdc, 0xe2, 0xe6 | navigator reactions (EB, notch slam, overrun by stage set; the buffer-stop crash plays all four) | "[The navigator reacts]" | description |
| 0xe5 | navigator, EB (stages 5–6) | "[The navigator reacts to the emergency brake]" | description |
| 0xd8, sequence 0x12 (0xe4 0xe6 0xe8) | navigator, overrun | "[The navigator reacts to the overrun]" | description |
| sequences 0x11 (0xe7 ×3), 0x25 (0xdb 0xdd) | navigator, notch slam | "[The navigator reacts to the rough handling]" | description |
| 0xef, 0xf0 | time-up advice, cause 0 / 1 with balloon 0x3f6 | "Make use of coasting." | likely (balloon gist) |
| 0xf6 | time-up advice with balloon 0x3fe | "Aim for precise driving!" | likely |
| 0xf1 | balloon 0x3f9 | "Never release the brake before the door lamp lights up." | likely |
| 0xea, 0xfb | balloon 0x3fa | "Brake gently and smoothly." | likely |
| 0xfc | balloon 0x3f3 | "Your braking is too hard." | likely |
| 0xeb, 0xf3 | balloon 0x3f2 | "Don't skip your stops." | likely |
| 0xec | balloon 0x3f4 | "Obey the signals." | likely |
| 0xf5 | balloon 0x3f7 | "Never force a level crossing. Stop and look first!" | likely |
| 0xf2 | time-up advice, cause 5 (its balloon is the generic 0x3fe) | — | **not identified** |
| 0xd5 | departure task tick 0x30, stages 3–16 (the navigator's voice set, 17.9 kHz; during the departure melody, before the doors close) | — | **not identified** (heard in the lesson stage at tick ~48) |
| 0x16d, sequence 0x18 (0x16a..0x16d) | departure task tick 0x30 on the Shinkansen stages | — | **not identified** (platform/conductor announcement?) |
| sequences 0x13, 0x16, 0x2a, 0x2b, 0x2c, 0x2d | in-train announcements (Hokuhoku, Kōbe line) | only their "next {station}" part (0x2c names 0x15f = Ashiya) | confirmed for that part; the rest **not identified** |
| sequences 0x19..0x23, 0x2e (0x16e..0x18f) | Shinkansen in-train announcements | — | **not identified**; by position 0x177 Kakunodate, 0x17a Tazawako, 0x17c Shizukuishi and 0x173 "next" are likely, to confirm by listening |

Outside driving, not subtitled (menus and screens): the run-end voices 0xf7, 0xf8, 0xf9, 0xfd,
0xfe and sequences 0..4 (0xfa + one of those, report card by grade), the title call 0x1024
(0x190, 0x191), sequences 0x26 / 0x27 (navigator-set phrases, caller not traced). Non-voice
codes (bells, buzzers, joints, the passing train's sequence 0x28, the E3 coupling's 0x29) have
no subtitles.
