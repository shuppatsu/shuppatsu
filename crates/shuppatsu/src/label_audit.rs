//! Audit of a catalogue's regions against the original art (`shuppatsu label-audit`; checker in
//! [`shuppatsu_render::text_audit`], docs/localisation.md "Checking the regions").
//!
//! Every part of every sprite override is checked for original lettering left outside its
//! region, for a region reaching onto a border or off its panel, and for an ink box (what the
//! English is sized from) that includes more than the lettering. Findings that are intended
//! are listed in [`ALLOW`] with the reason; the data test keeps the rest at zero.

use crate::locale::{Catalogue, Key, SheetTables};
use anyhow::{Context, Result};
use shuppatsu_formats::cg::{Palette, TILE_SIZE};
use shuppatsu_render::sprites::SpriteBank;
use shuppatsu_render::text::{Fonts, SubRect, TextOverlay};
use shuppatsu_render::text_audit::{Finding, audit_part};
use std::fmt::Write as _;
use std::path::Path;

/// An intended finding: `class` of part `part` of the entries `keys` (as [`key_name`] spells
/// them) is how it should be.
pub struct Allow {
    pub keys: &'static [&'static str],
    pub part: usize,
    pub class: &'static str,
    pub why: &'static str,
}

/// The departure-board flaps and the remaining-time 秒 box, whose split-flap gap goes with the
/// lettering.
const GAPPED: &[&str] = &[
    "tbl 0x3c9",
    "tbl 0x3ca",
    "tbl 0x3cb",
    "tbl 0x3cc",
    "tbl 0x3cd",
    "tbl 0x3ce",
    "tbl 0x3cf",
    "tbl 0x3d0",
    "tbl 0x3d1",
    "tbl 0x3d2",
    "tbl 0x3d3",
    "tbl 0x3d4",
    "tbl 0x3d9",
    "tbl 0x3da",
    "tbl 0x3db",
    "tbl 0x3dc",
    "tbl 0x3dd",
    "tbl 0x3de",
    "tbl 0x3df",
    "tbl 0x3e0",
    "tbl 0x3e1",
    "tbl 0x3e2",
    "tbl 0x3e3",
    "tbl 0x3e4",
    "tbl 0x3e5",
    "tbl 0x3e6",
    "tbl 0x3e7",
    "tbl 0x3e8",
    "tbl 0x3e9",
    "tbl 0x3ea",
    "tbl 0x3eb",
    "tbl 0x3ec",
    "tbl 0x3ed",
    "tbl 0x3ee",
    "tbl 0x3ef",
    "tbl 0x3f0",
    "tbl 0x3f1",
    "tbl 0x40d",
    "tbl 0x40e",
];
/// Sign captions (hud.md §3.10): plain lettering joined to the sign's post.
const SIGNS: &[&str] = &[
    "tbl 0x45b",
    "tbl 0x45c",
    "tbl 0x45d",
    "tbl 0x45e",
    "tbl 0x45f",
    "tbl 0x460",
    "tbl 0x461",
    "tbl 0x462",
    "tbl 0x463",
    "tbl 0x464",
    "tbl 0x465",
    "tbl 0x466",
    "tbl 0x467",
    "tbl 0x468",
    "tbl 0x469",
    "tbl 0x46a",
    "tbl 0x46b",
    "tbl 0x46c",
    "tbl 0x46d",
    "tbl 0x46e",
    "tbl 0x46f",
    "tbl 0x48a",
    "tbl 0x48b",
    "tbl 0x48c",
    "tbl 0x48d",
    "tbl 0x48e",
    "tbl 0x48f",
];
/// Line-name plates of the line list, with the line's emblem beside the name.
const LINE_PLATES: &[&str] = &[
    "tbl 0x41f",
    "tbl 0x420",
    "tbl 0x421",
    "tbl 0x422",
    "tbl 0x423",
    "tbl 0x424",
    "tbl 0x425",
    "tbl 0x426",
    "tbl 0x427",
    "tbl 0x428",
    "tbl 0x429",
    "tbl 0x42a",
    "tbl 0x42b",
    "tbl 0x42c",
    "tbl 0x42d",
    "tbl 0x42e",
    "tbl 0x42f",
    "tbl 0x430",
    "tbl 0x431",
    "tbl 0x432",
];

/// Findings that are intended, each checked by eye on the crops of `shuppatsu label-audit`.
pub const ALLOW: &[Allow] = &[
    Allow {
        keys: GAPPED,
        part: 0,
        class: "border",
        why: "the split-flap gap across the face (rows 19–24 of a flap, 20–24 of the 秒 box) goes \
              with the lettering; drawn over the English it would cut the words in half",
    },
    Allow {
        keys: GAPPED,
        part: 0,
        class: "sizing",
        why: "the ink box spans the split-flap gap's ends, a pixel or two beyond the lettering, \
              which only widens the room the text may grow into",
    },
    Allow {
        keys: &["tbl 0x3e9"],
        part: 1,
        class: "border",
        why: "the split-flap gap across the name panel goes with はくたか",
    },
    Allow {
        keys: &["tbl 0x145"],
        part: 0,
        class: "border",
        why: "the hint's kanji start at x 5 on rows 26–40 and also reach rows 6 and 58, so a \
              rectangle over them takes in 4 pixels of the balloon's rounded corners; leaving \
              x 5–6 out would leave a stroke of 満 beside the English",
    },
    Allow {
        keys: &["tbl 0x13e"],
        part: 1,
        class: "border",
        why: "not a frame: the tops of 内, 信 and 号 (rows 4–6) touch the balloon's white inside \
              at row 3, the band having no top edge of its own, and pass for part of the frame",
    },
    Allow {
        keys: &["tbl 0x24e"],
        part: 0,
        class: "border",
        why: "not a frame: the vertical stroke of 事 runs 62 of the plate's 64 rows and passes \
              for a rule",
    },
    Allow {
        keys: &["tbl 0x24a", "tbl 0x250", "tbl 0x251"],
        part: 0,
        class: "leftover",
        why: "the outlines of the kanji touch CONTINUE / GAME OVER under them, which is Latin \
              art and stays",
    },
    Allow {
        keys: &["tbl 0x250", "tbl 0x251"],
        part: 0,
        class: "intrudes",
        why: "the GAME OVER art joined to the kanji outline: the few pixels of it inside the \
              region are the kanji's outline",
    },
    Allow {
        keys: &["sheet 0x544 #22", "sheet 0x544 #23"],
        part: 0,
        class: "intrudes",
        why: "the arrow's tip touches 右 at x 88; one pixel of the tip goes rather than a column \
              of the kanji staying",
    },
    Allow {
        keys: &[
            "sheet 0x544 #16",
            "sheet 0x544 #17",
            "sheet 0x544 #18",
            "sheet 0x544 #19",
            "sheet 0x544 #20",
            "sheet 0x544 #21",
        ],
        part: 0,
        class: "nearby",
        why: "the arrow icon left of the word stays",
    },
    Allow {
        keys: SIGNS,
        part: 0,
        class: "intrudes",
        why: "the sign's post runs down behind the caption and is joined to the kanji; under the \
              English it is erased with them, as the kanji covered most of it",
    },
    Allow {
        keys: SIGNS,
        part: 0,
        class: "sizing",
        why: "the caption kanji fill the band edge to edge, so the post adds nothing to the ink \
              box that the kanji do not",
    },
    Allow {
        keys: &[
            "tbl 0x453",
            "tbl 0x454",
            "tbl 0x455",
            "tbl 0x473",
            "tbl 0x474",
            "tbl 0x475",
            "tbl 0x476",
            "tbl 0x477",
            "tbl 0x478",
            "tbl 0x479",
            "tbl 0x47a",
            "tbl 0x47b",
        ],
        part: 0,
        class: "intrudes",
        why: "the bottom of the signal's black housing reaches into the solid black caption band \
              and is painted black again",
    },
    Allow {
        keys: &["tbl 0x473", "tbl 0x474", "tbl 0x475"],
        part: 0,
        class: "sizing",
        why: "one row of the housing above the caption: the ink box is one pixel taller",
    },
    Allow {
        keys: &["tbl 0x9", "tbl 0x12", "tbl 0x50d"],
        part: 0,
        class: "intrudes",
        why: "a plain label over the handle gauge: the label's box replaces the gauge art behind \
              the kanji, which is joined to them",
    },
    Allow {
        keys: &["tbl 0x9", "tbl 0x12", "tbl 0x50d"],
        part: 0,
        class: "sizing",
        why: "the kanji fill the label's box, so the gauge art adds nothing to the ink box",
    },
    Allow {
        keys: &["sheet 0x50b replay"],
        part: 0,
        class: "leftover",
        why: "the red frame of the replay mark is joined to 自動運転 and stays round the English",
    },
    Allow {
        keys: &["tbl 0x4fe"],
        part: 0,
        class: "leftover",
        why: "one pixel of 停 touches the dotted border at x 0; the region keeps to the panel \
              (x 1–150) like the other centimetre plates",
    },
    Allow {
        keys: &["tbl 0x548", "tbl 0x549"],
        part: 0,
        class: "leftover",
        why: "the tag's divider and side marks (rows 13–20) touch the name above them and stay",
    },
    Allow {
        keys: &["tbl 0x548", "tbl 0x549"],
        part: 1,
        class: "nearby",
        why: "the left side mark of the divider stays",
    },
    Allow {
        keys: &["tbl 0x548"],
        part: 1,
        class: "backdrop",
        why: "not a backdrop: 通過 is dense enough to fill most of its band",
    },
    Allow {
        keys: &["tbl 0x1a4", "tbl 0x1a5"],
        part: 1,
        class: "nearby",
        why: "the digit before 秒 is drawn by the game and stays",
    },
    Allow {
        keys: &["tbl 0x1cc"],
        part: 0,
        class: "nearby",
        why: "EXCELLENT! after 停止位置 is Latin art and stays",
    },
    Allow {
        keys: &["tbl 0x41d"],
        part: 0,
        class: "nearby",
        why: "the header's rivets at the corners stay",
    },
    Allow {
        keys: LINE_PLATES,
        part: 0,
        class: "nearby",
        why: "the line's emblem beside the name stays",
    },
    Allow {
        keys: &["tbl 0x545", "tbl 0x546"],
        part: 0,
        class: "nearby",
        why: "the key icon before the word stays",
    },
];

/// One audited part.
pub struct Audited {
    pub key: String,
    pub tbl: u16,
    /// The resolved source rectangle in tiles.
    pub rect: (u8, u8, u8, u8),
    /// The sprite's size in pixels.
    pub size: (usize, usize),
    pub part: usize,
    pub text: String,
    /// The region clipped to the sprite, `[x, y, w, h]`.
    pub region: [usize; 4],
    pub boxed: bool,
    /// The backdrop per parity class, as the renderer takes it.
    pub pattern: [u8; 4],
    /// The renderer's ink box `[x0, x1, y0, y1]` in region pixels (what the English is sized
    /// from).
    pub ink: Option<[usize; 4]>,
    pub findings: Vec<Finding>,
    /// The shadow or outline found in the original lettering (what the English repeats).
    pub effects: Option<shuppatsu_render::lettering::Effects>,
    /// The colours (even, odd columns) of a gradient in the original strokes, top to bottom
    /// (empty: none).
    pub gradient: Vec<[u8; 2]>,
}

impl Audited {
    /// The findings not in [`ALLOW`].
    pub fn unexplained(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| allowed(&self.key, self.part, f).is_none())
    }
}

/// The reason `f` of part `part` of entry `key` is intended, if it is.
pub fn allowed(key: &str, part: usize, f: &Finding) -> Option<&'static str> {
    ALLOW
        .iter()
        .find(|a| a.part == part && a.class == f.class() && a.keys.contains(&key))
        .map(|a| a.why)
}

/// A catalogue key as the report and [`ALLOW`] spell it: `tbl 0x59`, `tbl 0x50b [6,0,6,2]`
/// (an explicit tile rectangle), `sheet 0x544 #16`, `sheet 0x54d main_menu`.
pub fn key_name(k: &Key) -> String {
    match k {
        Key::Tbl { tbl, rect } if *rect == SubRect::WHOLE => format!("tbl {tbl:#x}"),
        Key::Tbl { tbl, rect } => format!(
            "tbl {tbl:#x} [{},{},{},{}]",
            rect.col, rect.row, rect.w, rect.h
        ),
        Key::Sheet { tbl, index } => format!("sheet {tbl:#x} #{index}"),
        Key::Rect { tbl, name, .. } => format!("sheet {tbl:#x} {name}"),
    }
}

/// Audits every part of every sprite override of `cat`. Entries whose sprite does not exist
/// are skipped (the catalogue test reports those).
pub fn audit(cat: &Catalogue, sheets: &SheetTables, bank: &SpriteBank) -> Result<Vec<Audited>> {
    let mut out = Vec::new();
    for e in &cat.entries {
        let (tbl, rect) = e.key.resolve(sheets)?;
        let Some(map) = bank.tilemaps().get(usize::from(tbl)) else {
            continue;
        };
        let Some(r) = rect.resolve(map.width, map.height) else {
            continue;
        };
        let art = bank.render_rect(tbl, r);
        let (w, h) = (usize::from(r.2) * TILE_SIZE, usize::from(r.3) * TILE_SIZE);
        for (i, p) in e.text.parts.iter().enumerate() {
            let others: Vec<[u16; 4]> = e
                .text
                .parts
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, q)| q.region.unwrap_or([0, 0, w as u16, h as u16]))
                .collect();
            let Some(a) = audit_part(p, &others, &art, w, h) else {
                continue;
            };
            let effects = shuppatsu_render::lettering::for_part(p, &a.scan, &art, w, h);
            let gradient =
                shuppatsu_render::lettering::gradient_for(p, &a.scan, &art, w, h, effects)
                    .map(|mut g| {
                        g.dedup();
                        g
                    })
                    .unwrap_or_default();
            out.push(Audited {
                key: key_name(&e.key),
                tbl,
                rect: r,
                size: (w, h),
                part: i,
                text: p.text.clone(),
                region: a.scan.rect,
                boxed: a.scan.boxed,
                pattern: a.scan.pattern,
                ink: a.scan.ink,
                findings: a.findings,
                effects,
                gradient,
            });
        }
    }
    Ok(out)
}

/// One line describing a finding.
pub fn describe(f: &Finding) -> String {
    let b = |[x0, x1, y0, y1]: [usize; 4]| format!("x {x0}–{} y {y0}–{}", x1 - 1, y1 - 1);
    match f {
        Finding::Leftover { pixels, bounds } => {
            format!("{} lettering px outside, {}", pixels.len(), b(*bounds))
        }
        Finding::Nearby { pixels, bounds } => {
            format!("{} lettering px beside it, {}", pixels.len(), b(*bounds))
        }
        Finding::Intrudes { pixels, bounds } => {
            format!(
                "{} px of other art erased inside, {}",
                pixels.len(),
                b(*bounds)
            )
        }
        Finding::Border {
            pixels,
            bounds,
            sides,
        } => format!("{} frame px inside ({sides}), {}", pixels.len(), b(*bounds)),
        Finding::OffPanel { pixels, bounds } => {
            format!("{} px off the panel, {}", pixels.len(), b(*bounds))
        }
        Finding::BeyondArt { overhang } => format!("overhang l/t/r/b {overhang:?}"),
        Finding::Backdrop { face, pattern } => {
            format!("face {face} but backdrop pattern {pattern:?}")
        }
        Finding::Sizing {
            renderer,
            lettering,
        } => {
            let o = |v: &Option<[usize; 4]>| v.map(b).unwrap_or_else(|| "none".into());
            format!("ink box {} vs lettering {}", o(renderer), o(lettering))
        }
    }
}

// ---- The report ------------------------------------------------------------------------------

const USAGE: &str =
    "usage: shuppatsu label-audit [--out <dir>] [--catalogue <en.toml>] [--palette <n>]
    [--no-crops | --all-crops] [--only <key>]... [--dump <tbl>]";

/// `shuppatsu label-audit`: writes `report.md` and a magnified crop per flagged part to `--out`
/// (default `out/explain/label_audit`).
pub fn run(args: &[String]) -> Result<()> {
    let mut out = std::path::PathBuf::from("out/explain/label_audit");
    let mut catalogue = None;
    let mut palette = 4u8;
    let mut dump = None;
    let mut crops = true;
    let mut all_crops = false;
    let mut only: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .with_context(|| format!("{a} needs a value\n{USAGE}"))
        };
        match a.as_str() {
            "--out" => out = val()?.into(),
            "--catalogue" => catalogue = Some(std::path::PathBuf::from(val()?)),
            "--palette" => palette = val()?.parse()?,
            "--no-crops" => crops = false,
            "--all-crops" => all_crops = true,
            "--only" => only.push(val()?.clone()),
            "--dump" => {
                let v = val()?;
                dump = Some(u16::from_str_radix(v.trim_start_matches("0x"), 16)?);
            }
            other => anyhow::bail!("unknown option {other}\n{USAGE}"),
        }
    }
    let cat = match &catalogue {
        Some(p) => Catalogue::parse(&std::fs::read_to_string(p)?)?,
        None => Catalogue::load("en", Some(Path::new("locales")))?,
    };
    let data = crate::scene::GameData::from_env()?;
    let exe_bytes = data.exe()?;
    let exe = shuppatsu_formats::exe::Exe::parse(&exe_bytes)?;
    let sheets = SheetTables::read(&exe)?;
    let mut bank = SpriteBank::new(
        &shuppatsu_formats::cg::CgFile::parse(&data.read("cg.rom")?)?,
        shuppatsu_formats::cg::parse_tbl(&data.read("tbl.rom")?)?,
    );
    // The port's own cells (the movie label numbered 5, the options' extra rows), as the game
    // builds them.
    crate::front::add_port_art(
        &mut bank,
        &mut crate::front::tables::FrontTables::read(&exe)?,
    );
    let palettes: std::collections::HashMap<u8, Palette> =
        shuppatsu_formats::cg::init_palettes(&exe)?
            .into_iter()
            .collect();
    let pal = palettes
        .get(&palette)
        .with_context(|| format!("palette {palette} is not set at init"))?;
    if let Some(tbl) = dump {
        print!("{}", dump_map(&bank, tbl)?);
        return Ok(());
    }
    let font = crate::locale::bundled_font()?;
    let items = audit(&cat, &sheets, &bank)?;
    std::fs::create_dir_all(out.join("crops"))?;

    let mut md = String::new();
    writeln!(md, "# Label audit\n")?;
    writeln!(
        md,
        "Every part of every sprite override in the catalogue, checked against the original art \
         with the renderer's notion of backdrop and lettering (`shuppatsu_render::text_audit`). \
         Generated by `shuppatsu label-audit`; crops in `crops/` (original with the region \
         outlined in cyan, findings tinted; the English as drawn; ×{{scale}}). Palette {palette}.\n"
    )?;
    let classes = [
        "leftover",
        "nearby",
        "intrudes",
        "border",
        "off-panel",
        "beyond-art",
        "backdrop",
        "sizing",
    ];
    writeln!(
        md,
        "{} parts in {} entries.\n",
        items.len(),
        cat.entries.len()
    )?;
    writeln!(md, "| class | parts | of which allowed |\n|---|---|---|")?;
    for c in classes {
        let all = items
            .iter()
            .filter(|i| i.findings.iter().any(|f| f.class() == c))
            .count();
        let ok = items
            .iter()
            .filter(|i| {
                i.findings
                    .iter()
                    .any(|f| f.class() == c && allowed(&i.key, i.part, f).is_some())
            })
            .count();
        writeln!(md, "| {c} | {all} | {ok} |")?;
    }
    writeln!(md)?;
    for (title, want_allowed) in [("Unexplained", false), ("Allowed", true)] {
        writeln!(md, "## {title}\n")?;
        for i in &items {
            let fs: Vec<&Finding> = i
                .findings
                .iter()
                .filter(|f| allowed(&i.key, i.part, f).is_some() == want_allowed)
                .collect();
            if fs.is_empty() {
                continue;
            }
            let [x, y, w, h] = i.region;
            let crop = format!("crops/{}_{}.png", file_stem(&i.key), i.part);
            writeln!(
                md,
                "### {} part {} — {:?}\n\n{}×{} sprite, region [{x}, {y}, {w}, {h}], {} — [crop]({crop})\n",
                i.key,
                i.part,
                i.text,
                i.size.0,
                i.size.1,
                if i.boxed {
                    format!("boxed, backdrop {:?}", i.pattern)
                } else {
                    "plain".to_owned()
                },
            )?;
            for f in fs {
                match allowed(&i.key, i.part, f) {
                    Some(why) => writeln!(md, "- {}: {} — allowed: {why}", f.class(), describe(f))?,
                    None => writeln!(md, "- **{}**: {}", f.class(), describe(f))?,
                }
            }
            writeln!(md)?;
        }
    }
    std::fs::write(out.join("report.md"), md)?;
    // Every part with the ink box the English is sized from, for comparing catalogues.
    let mut tsv = String::from("key\tpart\tregion\tink\tstyle\teffects\n");
    for i in &items {
        let ink = i.ink.map_or("-".into(), |b| format!("{b:?}"));
        let mut fx = i.effects.map_or("-".into(), |e| effects_text(&e));
        if !i.gradient.is_empty() {
            write!(fx, ", gradient {:?}", i.gradient)?;
        }
        let style = if i.boxed { "boxed" } else { "plain" };
        writeln!(
            tsv,
            "{}\t{}\t{:?}\t{ink}\t{style}\t{fx}",
            i.key, i.part, i.region
        )?;
    }
    std::fs::write(out.join("parts.tsv"), tsv)?;

    // Crops of every flagged part.
    let mut done = std::collections::HashSet::new();
    let wanted = |i: &Audited| only.is_empty() || only.contains(&i.key);
    for i in items
        .iter()
        .filter(|i| crops && (all_crops || !i.findings.is_empty()) && wanted(i))
    {
        let name = format!("{}_{}.png", file_stem(&i.key), i.part);
        if !done.insert(name.clone()) {
            continue;
        }
        let entry = cat
            .entries
            .iter()
            .find(|e| key_name(&e.key) == i.key)
            .expect("audited entries come from the catalogue");
        let img = crop(&bank, i, &entry.text, pal, &font)?;
        std::fs::write(out.join("crops").join(name), img)?;
    }
    eprintln!(
        "{} parts, {} flagged, {} unexplained; report in {}",
        items.len(),
        items.iter().filter(|i| !i.findings.is_empty()).count(),
        items
            .iter()
            .filter(|i| i.unexplained().next().is_some())
            .count(),
        out.join("report.md").display()
    );
    Ok(())
}

/// `fill 10, shadow (-2,-1) in 1, outline 1 px in 3`, or `fill 10, flat`.
pub fn effects_text(e: &shuppatsu_render::lettering::Effects) -> String {
    let mut s = format!("fill {}", e.fill);
    if let Some(o) = e.outline {
        write!(s, ", outline {} px in {}", o.width, o.colour).expect("string");
    }
    if let Some(o) = e.outer {
        write!(s, ", outer {} px in {}", o.width, o.colour).expect("string");
    }
    if let Some(d) = e.shadow {
        let how = if d.extrude { "swept " } else { "" };
        write!(s, ", shadow {how}({},{}) in {}", d.dx, d.dy, d.colour).expect("string");
    }
    if !e.any() {
        s.push_str(", flat");
    }
    s
}

fn file_stem(key: &str) -> String {
    key.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .replace("__", "_")
}

/// Original (region outlined, findings tinted) and English side by side, magnified, as PNG.
fn crop(
    bank: &SpriteBank,
    i: &Audited,
    o: &shuppatsu_render::text::TextOverride,
    pal: &Palette,
    font: &shuppatsu_render::text::FontGlyphs,
) -> Result<Vec<u8>> {
    let (w, h) = i.size;
    let s = (768 / w).clamp(2, 8);
    let art = bank.render_rect(i.tbl, i.rect);
    // Art that the palette draws in fewer than half as many colours as it has indices (a menu
    // sprite whose palette is
    // loaded later, not at init) gets false colours per index, so the crop stays readable.
    let false_colour;
    let pal = {
        let mut used: Vec<u8> = art.iter().copied().filter(|&v| v != 0).collect();
        used.sort_unstable();
        used.dedup();
        let mut shown: Vec<[u8; 4]> = used.iter().map(|&v| pal.rgba(v)).collect();
        shown.sort_unstable();
        shown.dedup();
        if used.len() >= 3 && shown.len() * 2 < used.len() {
            let mut p = [0u32; 256];
            for (i, c) in p.iter_mut().enumerate() {
                // A spread of hues and brightness by index (B, R, G byte order as in the exe).
                let h = (i as u32).wrapping_mul(0x9e37_79b9);
                *c = (h >> 8) & 0x00ff_ffff | 0x0040_4040;
            }
            false_colour = Palette(p);
            &false_colour
        } else {
            pal
        }
    };
    let gap = 8;
    let (cw, ch) = (2 * w * s + gap, h * s);
    let mut rgba = vec![0u8; cw * ch * 4];
    let bg = |x: usize, y: usize| -> [u8; 4] {
        if ((x / 8) + (y / 8)).is_multiple_of(2) {
            [48, 48, 56, 255]
        } else {
            [72, 72, 80, 255]
        }
    };
    let mut put = |x: usize, y: usize, c: [u8; 4]| {
        let o = (y * cw + x) * 4;
        rgba[o..o + 4].copy_from_slice(&c);
    };
    // Both panels start as the art over a checker.
    let english = o.parts.get(i.part).map(|_| {
        let only = shuppatsu_render::text::TextOverride {
            parts: o.parts.clone(),
        };
        let sub = SubRect {
            col: i.rect.0 as i8,
            row: i.rect.1 as i8,
            w: i.rect.2 as i8,
            h: i.rect.3 as i8,
        };
        TextOverlay::build_with(bank, [(i.tbl, sub, &only)], Fonts::one(font), s)
    });
    let parts = english
        .as_ref()
        .and_then(|e| e.get(&(i.tbl, i.rect)))
        .unwrap_or(&[]);
    for y in 0..h * s {
        for x in 0..w * s {
            let v = art[(y / s) * w + x / s];
            let c = if v == 0 { bg(x, y) } else { pal.rgba(v) };
            put(x, y, c);
            // The English: the art outside every part, the part's base and text inside.
            let mut e = c;
            for p in parts {
                let [px, py, pw, ph] = p.region.map(usize::from);
                if (px..px + pw).contains(&x) && (py..py + ph).contains(&y) {
                    let k = (y - py) * pw + (x - px);
                    let under = if p.base[k] != 0 {
                        pal.rgba(p.base[k])
                    } else {
                        bg(x, y)
                    };
                    let fg = pal.rgba(p.fg_map.get(k).copied().unwrap_or(p.fg));
                    let t = u16::from(p.coverage[k]);
                    let mix =
                        |a: u8, b: u8| ((u16::from(a) * (255 - t) + u16::from(b) * t) / 255) as u8;
                    e = [
                        mix(under[0], fg[0]),
                        mix(under[1], fg[1]),
                        mix(under[2], fg[2]),
                        255,
                    ];
                }
            }
            put(w * s + gap + x, y, e);
        }
    }
    // Findings tinted on the original.
    for f in &i.findings {
        let tint: [u8; 4] = match f.class() {
            "leftover" => [255, 0, 0, 255],
            "nearby" => [255, 140, 0, 255],
            "intrudes" => [0, 160, 255, 255],
            "border" => [255, 255, 0, 255],
            "off-panel" => [255, 0, 255, 255],
            _ => continue,
        };
        for &(x, y) in f.pixels() {
            let (x, y) = (usize::from(x) * s, usize::from(y) * s);
            // A dot in the middle of the magnified pixel keeps the art readable.
            let d = (s / 3).max(1);
            for yy in y + d..y + s - d {
                for xx in x + d..x + s - d {
                    put(xx, yy, tint);
                }
            }
        }
    }
    // The region's outline on the original (the English stays clean for judging it).
    let [rx, ry, rw, rh] = i.region.map(|v| v * s);
    let cyan = [0, 255, 255, 255];
    {
        let off = 0;
        for x in rx..rx + rw {
            put(off + x, ry, cyan);
            put(off + x, ry + rh - 1, cyan);
        }
        for y in ry..ry + rh {
            put(off + rx, y, cyan);
            put(off + rx + rw - 1, y, cyan);
        }
    }
    Ok(crate::desktop::encode_png(
        &shuppatsu_render::rgba::RgbaImage {
            width: cw as u32,
            height: ch as u32,
            rgba,
        },
    ))
}

/// The whole map `tbl` as text, one character per pixel (`.` transparent, then `0-9a-zA-Z`
/// per palette index in order of first use), with x / y rulers and the legend.
fn dump_map(bank: &SpriteBank, tbl: u16) -> Result<String> {
    let map = bank
        .tilemaps()
        .get(usize::from(tbl))
        .with_context(|| format!("no tilemap {tbl:#x}"))?;
    let (w, h) = (
        usize::from(map.width) * TILE_SIZE,
        usize::from(map.height) * TILE_SIZE,
    );
    let art = bank.render_rect(tbl, (0, 0, map.width, map.height));
    const SYMS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut seen: Vec<u8> = Vec::new();
    let mut s = String::new();
    let ruler = |s: &mut String, div: usize| {
        s.push_str("     ");
        for x in 0..w {
            s.push(char::from(b'0' + ((x / div) % 10) as u8));
        }
        s.push('\n');
    };
    ruler(&mut s, 100);
    ruler(&mut s, 10);
    ruler(&mut s, 1);
    for y in 0..h {
        write!(s, "{y:4} ")?;
        for x in 0..w {
            let v = art[y * w + x];
            if v == 0 {
                s.push('.');
                continue;
            }
            let i = seen.iter().position(|&c| c == v).unwrap_or_else(|| {
                seen.push(v);
                seen.len() - 1
            });
            s.push(char::from(*SYMS.get(i).unwrap_or(&b'?')));
        }
        s.push('\n');
    }
    for (i, v) in seen.iter().enumerate() {
        write!(s, "{}={v} ", char::from(*SYMS.get(i).unwrap_or(&b'?')))?;
    }
    s.push('\n');
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the user's data: no region of the English catalogue leaves original lettering
    /// visible, reaches onto a border or is sized from more than the lettering, except the
    /// findings in [`ALLOW`].
    #[test]
    fn english_regions_fit_their_art() {
        if std::env::var_os("SHUPPATSU_DATA_DIR").is_none() {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return;
        }
        let data = crate::scene::GameData::from_env().unwrap();
        let exe_bytes = data.exe().unwrap();
        let exe = shuppatsu_formats::exe::Exe::parse(&exe_bytes).unwrap();
        let mut bank = SpriteBank::new(
            &shuppatsu_formats::cg::CgFile::parse(&data.read("cg.rom").unwrap()).unwrap(),
            shuppatsu_formats::cg::parse_tbl(&data.read("tbl.rom").unwrap()).unwrap(),
        );
        let mut front = crate::front::tables::FrontTables::read(&exe).unwrap();
        crate::front::add_port_art(&mut bank, &mut front);
        let cat = Catalogue::load("en", None).unwrap();
        let items = audit(&cat, &SheetTables::read(&exe).unwrap(), &bank).unwrap();
        assert!(items.len() > 300, "{} parts", items.len());
        let bad: Vec<String> = items
            .iter()
            .flat_map(|i| {
                i.unexplained()
                    .map(|f| format!("{} part {}: {}: {}", i.key, i.part, f.class(), describe(f)))
            })
            .collect();
        assert!(
            bad.is_empty(),
            "{} findings:\n{}",
            bad.len(),
            bad.join("\n")
        );
        // Every allowance is still needed.
        for a in ALLOW {
            for k in a.keys {
                assert!(
                    items.iter().any(|i| i.key == *k
                        && i.part == a.part
                        && i.findings.iter().any(|f| f.class() == a.class)),
                    "allowance {k} part {} {} is no longer needed",
                    a.part,
                    a.class
                );
            }
        }
    }
}
