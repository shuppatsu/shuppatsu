# RecompLauncher listing

What to send when submitting Shuppatsu at https://recomplauncher.com/submit?platform=pc. Use the
submit form, not the developer sign-in at /dev: that one goes through a GitHub account and would
tie the listing to it. The fields follow the catalog (`https://recomplauncher.com/pc-catalog-web.json`);
values are in the catalog's own vocabulary, compared on 2026-11-26.

## Entry

| Field | Value |
|---|---|
| Name | Shuppatsu |
| Game | Densha de GO! 2 Kōsoku-hen 3000-bandai (電車でGO！２ 高速編 ３０００番台) |
| Origin | `pc` (Windows, 2000) |
| Repo | `shuppatsu/shuppatsu` |
| Systems | `windows`, `linux` |
| Genre | `other` (the catalog has no simulation genre; `racing` would mislead) |
| Tags | `Widescreen`, `Ultrawide`, `High FPS`, `Controller`, `HD Textures` |
| Handheld | Steam Deck: likely (1280×800 default, controller support), **not yet tested on one**; leave it out until it has been |

**Blurb** (one line, as the catalog's: "*X as a native PC game: …*"):

> Densha de GO! 2's high-speed lines as a native game for Windows and Linux: widescreen, high
> refresh rates, English and controllers, from your own disc.

**What it adds** (the `adds` field, if asked):

> A new engine, not an emulator: it reads the routes, art, sounds and the original's own tables
> from the player's disc and drives exactly like the original, checked against recorded runs.
> Widescreen and ultrawide without pop-in, interpolated high refresh rates over the original
> 30 Hz simulation, a full English translation with subtitles, twice the draw distance,
> anti-aliasing, texture packs, gamepads and the Densha de GO! controllers.

**What the player needs:** the 3000-bandai disc as an image (`.cue`/`.bin`, `.mdf`/`.mds` or
`.iso`). The game is no longer sold, so that means an original disc; say so on the page.

## What the listing gets from the release

`recomplauncher.json` in every release package (docs/building.md, "Continuous integration and
releases") gives the page Easy Setup (the disc input, checked by `DENGO3K.ICO` on the disc, and
the import as the setup step), saves, logs, six settings and the exit-code messages. There are no
`dumps`: Redump values could not be verified, and a three-track MDF would not match Redump's
per-track files anyway.

## Art

The catalog shows an icon, a cover, a hero image and a logo. Nothing derived from game data goes
into this repository (CLAUDE.md, rule 1), and screenshots of the running game are such data. Offer:

- **Icon and logo**: the project's own, `assets/logo/gen/icon-512.png` and the station-board
  header (`assets/logo/header-dark.svg` / `header-light.svg`). Original artwork, no game data.
- **Cover and hero**: ask RecompLauncher to use its own catalog art for the game, as it does for
  other entries, or send screenshots hosted outside the repository (README comment at the top).

## After it is listed

- Change the README badge's link from `https://recomplauncher.com` to the game's page,
  `https://recomplauncher.com/game/<id>` (the `id` is in the catalog JSON, matched by `repo`).
- Check the page's Launcher Support box: everything should come from our own file ("Full
  Support"), except Verified Copies (no `dumps`).
