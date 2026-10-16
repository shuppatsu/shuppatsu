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
  explicit `rect` cover sheets whose descriptors were not found (the screen headers, the
  legend word sheet).
* `[station_plate]`: the English name of each vertical station plate, keyed by the plate's
  tilemap id (`0x4e1 = "Yokohama"`). These are not sprite overrides: they feed the horizontal
  plate layout below.

Values are a string, or a table with `text` and options:

| Option | Meaning |
|---|---|
| `region = [x, y, w, h]` | Replace only these pixels (relative to the sprite); the rest of the art stays. Used where text shares a sprite with gauges, digits, icons or Latin art. |
| `parts = [{ region, text }, ...]` | Several regions in one sprite (e.g. a label and a unit). |
| `align` | `center` (default), `left`, `right`. |
| `style` | `auto` (default), `plain` (lettering on transparency: the old art is removed), `boxed` (a plate or box: the backdrop is kept, the old lettering painted over). `auto` picks `boxed` when more than 35 % of the region's edge is opaque. |
| `vertical` | Rotate 90° clockwise (reads top to bottom). |
| `outline` | Plain style: 1-px outline in the art's second colour (default on). |

`\n` breaks lines; the font size is fitted to the rectangle. Comments in `en.toml` give each
sprite's role, a short Japanese label for identification and the pixel size.

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
| Font file (TTF/OTF) | `DDG_FONT` | `--font <file>` |
| Extra catalogue directory | `DDG_LOCALE_DIR` | `--locale-dir <dir>` |

No font is shipped in the repo. Any TTF/OTF with Latin glyphs works; DejaVu Sans Bold or Noto
Sans Bold are good starting points (on NixOS: `fc-list | grep -i dejavu`). The catalogue uses a
few non-ASCII characters (macrons as in "Ōmagari", arrows "← →", the en dash), so the font
should cover Latin Extended-A and arrows. A language without a font keeps the original art and
prints a warning. Built-in catalogues (`en`) are compiled into the binary;
`<locale-dir>/<lang>.toml` overrides them, which is how a new language can be tried without a
rebuild.

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
  at a whole number of canvas pixels per art pixel, and text. `SpriteLayer::add_text(layer,
  rect, text, Paint { palette, index }, cap)` draws a string into a rectangle (original
  pixels; drawn at the canvas resolution of `ui_scale`) in a palette colour, with capitals
  `cap` pixels high or as large as fits, uniformly scaled. Widgets are drawn after the slots of
  their layer, are cleared with the rect layer every tick, and are empty in the arcade default.
  The text uses `crates/ddg-render/src/label.rs` (`LabelFont`: the language's font, installed
  by `install` with `SpriteBank::set_label_font`, masks cached per string and size).

## Station plates (enhancement `en_station_plates`)

The original's station plates (hud.md §3.7) are two 32×96 plates with vertical kanji in the
top right, "from" (slot 0x83 at (493, 8)) and "next stop" (slot 0x85 at (552, 8)), with the
arrow 0x4b8 (slot 0x84 at (530, 48)) between them. Vertical romaji is hard to read, so with a
translation active (`LocaleConfig::active`: a language other than `ja` and a font) the plates
are laid out horizontally in the same footprint (`crates/ddg-game/src/hud/plates_en.rs`).
640×480 coordinates:

| Element | Rectangle | Content |
|---|---|---|
| Backdrop | (490, 7) 98×100 | the plates' black |
| "From" box | (492, 10) 94×42 | #303030, the kanji name and the English name under it |
| Arrow | (533.5, 53) 11×9 | ▼ in the plates' yellow |
| "Next stop" box | (492, 62) 94×42 | as "from" |

* The kanji are the original horizontal station-name sprites (`HudTables::station_names`,
  `plain`, the captions' art), not a font. Their lettering index (0x1c, green in palette 4) is
  recoloured to the plates' yellow (palette 4 index 0xa) in a port-only palette, slot 0x7f,
  rebuilt every tick from palette 4; the box grey goes into an index the name sprites do not
  use. All indices are found in the art at load (the plates' commonest index is the backdrop,
  the second their lettering; the name sprites' commonest other than the backdrop is theirs).
* The sprite is drawn at a whole number of canvas pixels per art pixel, never resampled:
  1 at `ui_scale` 1 and 2 (at 2× it is half the original height, 14 px of 640×480, like the
  other HUD lettering), 2 at 3 and 4. A name wider than the box is centred and clipped at both
  ends. At `ui_scale` 2 every name fits (the widest, Hokuhoku-Ōshima, is 164 of 188 px). At
  `ui_scale` 1 eight are clipped: Yotsugoya 0x4ba (109 px), Uragawara 0x4ce (138),
  Hokuhoku-Ōshima 0x4cf (164), Matsudai 0x4d0 (107), Higashi-Kanagawa 0x4e2 (116),
  Kōshienguchi 0x4ef (105), Kōnan-Yamate 0x4f2 (110), Settsu-Motoyama 0x4f3 (117); the box
  is 94 px wide. At `ui_scale` 3 (2 canvas pixels per art pixel in a 282-pixel box) only
  Hokuhoku-Ōshima is clipped; at 4 none.
* The English name is `[station_plate]` text under the kanji, capitals 5 px high (smaller if
  a long name needs it), centred, in the plates' yellow.
* State: the plate slots are set and cleared exactly as in arcade mode but parked off screen;
  each tick the HUD draws the boxes from the slots' current images, so they appear and
  disappear with the original plates. With neither plate shown nothing is drawn.
* Japanese (and any run without a font) draws the original plates unchanged.

## Integration

Nothing calls this yet. Three edits turn it on for `ddg-game play`:

1. `main.rs`, `parse_play_args`: take the locale options out of the argument list before the
   option loop, and keep them in the options (`PlayOptions` gets a
   `pub locale: crate::locale::LocaleConfig` field):

   ```rust
   let mut args: Vec<String> = std::env::args().skip(2).collect();
   let mut locale = locale::LocaleConfig::from_env();
   locale.take_args(&mut args)?; // removes --lang/--font/--locale-dir
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

622 sprite entries (506 tilemaps and 116 string-sheet labels) and 65 station-plate names.

* HUD: time labels, remaining time, line/vehicle plates (15), distance-panel headers (m and
  cm), handle labels, ATS lamp, cab-signal lamp, all 15 time messages with their sign and unit
  words, all 34 navigator hints plus the two horn variants, every signal caption (3-lamp,
  4-lamp, repeater, emergency: 37), every sign caption (27), station captions/banners and
  departure words, 85 station names (plain and highlighted), 31
  landmarks, the progress strips, all-clear, emergency-brake, time-up and its 9 advice
  balloons, the coupling bonus game.
* Screens: station result rows, continue, run end (both grades), report card, lever check,
  departure board (header and all 37 flaps), route select (groups, difficulty, rank badges,
  Shizukuishi tags), records (header, 7 line and 13 series plates, legends), press START.
* Sheets: title items (5), options labels and values (2 × 31), pause/dialog texts (27), screen
  headers (13), name-entry legends (3), legend words and the replay mark (6).

Not catalogued, on purpose:

* Latin art that needs no translation: `great!` 0x1aa, `good` 0x1ab, `Professional` 0x2c7,
  `PRO` 0x2c8, `ATC` 0x3f, `888 km/h` 0x53, `EXCELLENT!` 0x1cd, `PAUSE` (0x544 #1),
  `ALL ROUND CLEAR`/`GAME OVER`/`CONTINUE` parts (kept by regions).
* Pictures and gauges with only numbers or symbols: brake B1–B8, power P1–P5, digits, stars,
  key/pad glyphs (0x4a6, 0x503), the name-entry symbol wheel (its katakana are input symbols),
  `〜` 0x30d.
* Text baked into pictures where replacing it would damage the art: the E3 coupling animation
  0x163–0x166, the EF63 shock meter 0x2da, the tiny 車内信号 label in hint picture 0x13e, and
  the vertical route diagrams of the pre-departure screen (0x31e–0x325, 0x329, 0x32a…: one
  station name per column; doable with one vertical part per station, not done yet).
* Arcade-only, unreachable on PC: insert-coin texts 0x253, 0x254.

Regions marked `(est.)` in `en.toml` were measured by eye and should be checked in the running
game.

## Open decision: redrawn art or a font

The user has not decided how English should look. The two options:

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
* Station names use Hepburn with macrons (Ōmagari, Jingūji) except Tokyo, Osaka and Kobe.
