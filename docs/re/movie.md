# The Komachi movie (`DATA/komachi.dat`)

The bonus movie "LOVE特急こまち": an MPEG-1 program stream the original hands to DirectShow, and
the port decodes with its own MPEG-1 video and Layer II audio decoders (`shuppatsu_formats::mpeg`,
decision D2 in architecture.md). Confidence tags: **confirmed** (code, data bytes or a
measurement), **likely**, **guess**.

## 1. Where the original plays it

| Address | What | Confidence |
|---|---|---|
| 0x404e10 | attract item 3: `0x404cf0` (every stage of the five route-select groups has a clear mark) → the movie | confirmed |
| 0x42a1c0 case 5 | title menu result 5: BGM 0x19e off, the movie, BGM on, the menu again with the cursor on 5. Unreachable: the menu has four or five rows (menus.md §3) | confirmed |
| 0x429110 | `movie_play_komachi`: `CoInitialize`, open `..\data\Komachi.DAT`, play, release | confirmed |
| 0x428db0 | `CoCreateInstance(CLSID_AMMultiMediaStream)`, `Initialize(STREAMTYPE_READ)`, `AddMediaStream(pDD, MSPID_PrimaryVideo)`, `AddMediaStream(NULL, MSPID_PrimaryAudio, AMMSF_ADDDEFAULTRENDERER)`, `OpenFile` | confirmed |
| 0x428f00 | the loop: one DirectDraw stream sample (`CreateSample(NULL, NULL, 0)`, its surface and rect), `SetState(RUN)`, then per sample `Update(0, …)` → `Blt` to the back buffer → flip (0x4286b0) → message pump and input poll (0x42a130), while the confirm edge is clear | confirmed |

**The rectangle — confirmed (0x428f00).** The destination of the `Blt` is the sample's rect
(0, 0, 320, 144) doubled and offset by a per-resolution origin (`0x816af4`): mode 1 (640×480)
(0, 0x60), mode 2 (800×600) (0x50, 0x9c), mode 3 (1024×768) (0xc0, 0xf0). Every mode gives the
same **640×288 rectangle, centred horizontally, 96 px from the top on 640×480** (centred
vertically in the other modes), never scaled with the resolution. The picture is doubled in
both directions, so the stream's sample aspect is ignored: on screen it is **2.22:1**, against
the 2.03:1 it codes (§2).

**The blit — likely.** `Blt(&dst, surface, &src, DDBLT_WAIT, NULL)`: a plain stretch blit with
no FX. Whether it was filtered depends on the DirectDraw driver (the HEL point-samples; cards
of the time did either). The sample surface is created in the primary surface's format, so the
movie was shown in the game's 16-bit colour (**likely**; the port shows 24-bit).

**Audio — confirmed.** `AMMSF_ADDDEFAULTRENDERER` sends the soundtrack to DirectShow's default
audio renderer at full level, beside the game's DirectSound voices; the game's sound option
(`0x856356`) does not reach it.

**End and skip — confirmed (0x428f00, 0x402700).** `Update` returning non-zero (end of stream)
leaves the loop. The loop test is `test [0x48c42c], 0x1000; jne loop`: the input words are
active low, so it loops while the confirm edge (Enter, input.md) is absent; a press skips and
sets `0x855ffc |= 0x10`. In attract, 0x404e10 then leaves the item's running bit set, and the
walk (0x402700) sees the same edge still in `0x48c42c` as START: one tick, the walk counter
+1, restart at item 2 with the opening skipped, i.e. **the title picture**. At its end the
movie lets the walk go on to item 4 (the demo route). The sprites are not cleared: the movie
draws over the back buffer, black in attract (the title task's destroy cleared the sprites).

## 2. The file — confirmed (decoded in full, `shuppatsu-tools movie info`, ffprobe)

* 39,001,368 bytes. MPEG-1 system stream: 12-byte MPEG-1 pack headers, a system header, PES
  packets of video 0xe0 and audio 0xc0 with MPEG-1 PES headers, and padding 0xbe. Three program
  end codes (`00 00 01 B9`) at 38,901,432, 38,918,551 and 39,001,364: the first two are
  mid-file, with the last 13 audio frames after them, so the demuxer carries on past
  an end code (ffmpeg does too).
* **Video**: MPEG-1, 320×144, `pel_aspect_ratio` 12 (0.9157 is code 8; code 12 = 1.0950 height
  / width, CCIR 601 525-line: sample aspect **200:219**, display aspect **2.03:1**), 29.97 Hz
  (code 4), 2048 kbit/s, the default intra matrix and a **downloaded non-intra matrix**. 4075
  pictures: I 278, P 1082, B 2715, no D; 135.97 s. First PTS 60,752 (0.675 s).
* **Audio**: MPEG-1 Layer II, 44.1 kHz, stereo (not joint), 192 kbit/s, no CRC: 5205 frames of
  626/627 bytes, allocation table B.2b (30 subbands), 135.97 s. First PTS 60,752, the same as
  the video's.

## 3. Decoder design (`crates/shuppatsu-formats/src/mpeg/`)

Pure: `&[u8]` in, frames out; written from ISO/IEC 11172-1/-2/-3.

* `bits.rs`: MSB-first bit reader; reads past the end give zeros and set an overrun flag that
  callers turn into `FormatError::Truncated`. Start-code search.
* `ps.rs`: pack (MPEG-1 and MPEG-2 forms), system header, PES (MPEG-1 stuffing / STD / PTS /
  DTS, or the MPEG-2 header), padding; `demux`, `demux_many`, `stream_ids`. Each stream's PTS
  are kept as `(offset in the elementary stream, pts)`; an access unit gets the stamp of a
  packet that began after the previous unit and at or before it.
* `vlc.rs`, `vtables.rs`: the Annex B tables as code strings, decoded through one flat lookup
  table per code (indexed by the next `max_len` bits). A test checks each table is prefix-free
  and covers the code space except the prefixes the standard leaves unused.
* `video.rs`: sequence / GOP / picture / slice / macroblock / block layers; I, P, B and D
  pictures; macroblock stuffing and escapes, skipped macroblocks (P: zero vector, predictor
  reset; B: the previous macroblock's motion); motion vectors with f_code residuals, wrap,
  full-pel; half-pel prediction with rounded averages, forward / backward / bidirectional;
  DC prediction; intra and non-intra dequantisation with oddification and saturation; the
  zigzag scan. Display order: a reference is shown when the next reference is decoded. B
  pictures without both references (an open GOP's leaders, after a broken link) are dropped.
  Malformed data never panics: a damaged slice ends at the next start code, the picture is
  completed (unreached macroblocks keep the forward reference, mid-grey in an I picture) and
  stays a reference, the error is returned once and decoding continues.
* `idct.rs`: the separable form of the defining formula in `f32` with a precomputed basis,
  rounded and saturated to −256..255 (Annex A); zero rows skipped, DC-only shortcut. A test
  compares it with the f64 double sum on IEEE 1180-style random blocks (peak error ≤ 1, under
  0.1 % mismatches).
* `mp2.rs`: header, table selection (B.2a–d by rate and bit rate per channel), allocation,
  scale factor selection information, scale factors, grouped (3/5/9 levels) and plain codes,
  requantisation `(2c + 1 − L) / L · scalefactor`, intensity stereo above the bound, and the
  synthesis filterbank of Figure A.2 (the 512-tap window of Table 3-B.3, stored as its
  257-entry half: the values are multiples of 2⁻¹⁶). Output is `f32`, not clipped (§4).
  `Mp2Stream` walks frames and resynchronises after damage.
* `Movie::open` demuxes the first video and audio streams.

Tests: synthetic streams written by a test-only encoder (I, P, B and D pictures whose pixels are
computed independently: flat DC blocks, an escaped AC coefficient through an f64 IDCT, motion
compensation from its definition, a downloaded non-intra matrix, display reordering;
Layer II frames giving a tone at a subband's centre at unit gain, independent channel scale
factors, joint stereo, grouped codes), truncation at every byte and random bit flips, and the
data test `tests/data_movie.rs` (the whole file: frame counts, types, PTS, durations; damaged
copies).

## 4. Validation against ffmpeg — confirmed (measured 2026-10-18)

`scripts/movie-compare.sh [idct]` decodes the file with ffmpeg (dev only, `nix shell
nixpkgs#ffmpeg`, never at runtime) into `out/movie/` and runs `shuppatsu-tools movie compare`.
Side-by-side PNGs (ours | ffmpeg | |difference| × 8): `out/explain/movie/vs_ffmpeg/`.

**Video**, all 4075 frames, PSNR over Y + Cb + Cr:

| Reference IDCT | Bit-exact frames | Mean PSNR (inexact) | Min PSNR | Max \|ΔY\| | Identical luma |
|---|---|---|---|---|---|
| ffmpeg default (`simple`, integer) | 43 | 64.5 dB | 61.7 dB (frame 2458, B) | 3 | 96.9 % |
| ffmpeg `faani` (float AAN) | 1435 | 85.1 dB | 72.2 dB | 1 | 99.97 % |

Per type against `simple`: I mean 67.6 dB (min 65.7), P 64.9 (62.1), B 64.6 (61.7). The syntax
decode is exact (identical frame and type sequence; against a float IDCT a third of the
frames are bit-exact); what is left is IDCT rounding and its drift through the P/B chains.

**Audio**, all 5205 frames: our float output quantised to 16 bits (as ffmpeg's fixed-point
`mp2` decoder outputs) against ffmpeg's: **SNR 86.8 dB, peak difference 1 LSB**, 75.1 % of the
samples identical. Unclipped, loud passages of the soundtrack exceed full scale (up to 1.33 in
frame 1285); ffmpeg clips them to 16 bits, and so does the port's mixer (and, presumably, the
original's renderer).

## 5. Decode speed — confirmed (`shuppatsu-tools movie info`, a current AMD laptop CPU, one thread)

| Build | Video | Audio | Read + demux |
|---|---|---|---|
| release | 0.38 ms / frame, 88× real time | 926× real time | 39 + 10 ms |
| debug | 5.55 ms / frame, 6.0× real time | 25.6× real time | — |

## 6. Playback in the port (`crates/shuppatsu/src/movie.rs`, `app.rs`)

* **Where it is played**: attract item 3 once every select stage is cleared
  (`komachi_unlocked`, as 0x404cf0), and, with setting `movie_in_menu` (on), the title menu's
  sixth label as its last row (menus.md §3); as the fifth row (without the Densha de GO!
  controller) its label reads 5, composed from the fifth label's number (menus.md §3). Skip
  and end as §1; from the menu the BGM comes back and the cursor sits on the movie row (case 5).
* **A/V sync**: the soundtrack is a `shuppatsu_audio::PcmStream` mixed at unity gain and resampled to
  the device rate by `audio_resampler`. The mixer counts the source frames played; picture *n*
  is due from *n* · 1001/30000 s of that clock on (plus the audio-minus-video start offset, 0
  here). A dry queue holds the sound and so the clock. Without a sound device the clock is
  the app's 33 ms ticks. Measured live: clock 11.189 s after 11.187 s of wall time at the skip
  (`movie:` log line).
* **Playback timing** (§6.1): the count is read smoothly, and each presented display frame
  picks its picture, on the display's refresh grid; a picture is uploaded once, when it changes.
* **On screen**: black around the picture (the sprites are cleared). `hud_layout original`:
  the original's rectangle (§1) on the presented 640×480 design, scaled with it; `enhanced`:
  the largest rectangle of the picture's shape in the window. Always the
  stream's 2.03:1 (584×288 on the design), not the original's 640×288 stretch (2.22:1): a
  deviation the user decided on, with no setting. `movie_scaling`:
  `smooth` (bilinear, default), `sharp` (sharp bilinear like the UI), `nearest`. Colour:
  BT.601 video range, chroma upsampled bilinearly from the MPEG-1 sample positions.
* **Shots**: `out/explain/movie/` (menus in English and Japanese, frames at 2560×1440 and 4:3
  in both layouts and aspects, the filters, a skip, the fifth-row menu label in both languages).

### 6.1 Playback timing — confirmed (measured 2026-10-19)

**The stutter (fixed).** Pictures were moved to the clock both by the 30 Hz app tick and by
each redraw, but only a picture the *redraw* moved to was uploaded to the GPU. A redraw runs
the ticks that are due first, so on every frame with a tick (half of them at 60 Hz) the tick
took the new picture and the redraw found nothing new: about half the pictures were never
shown, and while the tick and picture phases lined up the screen froze for up to 12 refreshes
(200 ms). Now the picture carries a "not yet uploaded" flag (`MoviePlayer::take_new_picture`),
and once a display draws the movie only the redraws move the pictures; the ticks keep the
sound fed.

**The clock between callbacks.** The played-frame count moves once per audio callback (470 or
941 source frames here, 10.7 / 21.3 ms, under PipeWire; other setups use larger buffers). At
the start of each callback the output records the count, the time and the device's output
latency (`cpal` callback → playback timestamps, capped at 250 ms;
`PcmStream::mark_callback`). The position heard at a time t is that count + (t − callback −
latency) · 44,100, clamped to 0 ..= the frames rendered (an underrun still holds it;
`PcmStream::position_at`). Against a straight line through wall time it deviates 9 µs RMS
(22 µs at most); it lags the raw count by 21–32 ms (median 27 ms), which the raw clock used
to show the pictures early by.

**The refresh grid.** With Fifo the redraw runs some time before the refresh that shows it,
±2 ms of jitter at 60 Hz (redraw intervals 15.8–18.2 ms). Sampled at the redraw, a picture
boundary near a refresh shows one picture 1 refresh and its neighbour 3. `FrameGrid` numbers
the redraws by refresh, fits a least-squares line through all of them (skipped refreshes are
steps; more than half a period off restarts the line) and gives each redraw its refresh's
time on the line; the picture is the one due at the clock read for that time
(`MoviePlayer::update_display`, `picture_at`). The clock is kept from going back.

**Cadence**, 4K window (3840×2160 fullscreen, the external display at 60 Hz, Fifo, release
build, ~26 s of the movie; `SHUPPATSU_MOVIE_TRACE=<file>` writes one line per presented
frame):

| Build | Pictures shown | Skipped | Display frames per picture | Redraw CPU (mean / max) |
|---|---|---|---|---|
| before | 352 | 335 | 1: 4, 2: 289, 3: 2, 4–12: 57 | 1.09 / 2.76 ms |
| after | 786 | 0 | 2: 784, 3: 2 | 1.06 / 3.17 ms |

No refresh was missed in either (present interval mean 16.67 ms, max 20 ms; the upload,
the bilinear scale to 4K and the present cost about 1 ms of CPU), so the GPU side was not the
cause. The two 3-refresh pictures are the 29.97-against-30 beat: at 60 Hz a picture lasts
2.002 refreshes, one more refresh every 500 pictures (17 s). Simulated with the measured jitter
(`movie::tests::display_cadence_is_even_on_the_grid`, 58 s after a 1 s warm-up): 60 Hz
{2: 1763, 3: 4}, 120 Hz {4: 1761, 5: 7}, 180 Hz {6: 1757, 7: 11}; sampled at the redraws
instead, 60 Hz {1: 69, 2: 1625, 3: 73}.

## 7. Open points

* The original's colour depth on screen (16-bit, **likely**) is not reproduced.
* The DirectDraw stretch filter of the original is driver-dependent and not measured under
  Wine.
* The renderer's level is taken as full scale (DirectShow's default renderer at volume 0 dB).
* The cadence at 120 and 180 Hz is simulated (§6.1), not measured in a window: the laptop
  panel runs at 180 Hz, the external display at 60 Hz.
