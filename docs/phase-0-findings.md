# Phase 0 — spike findings

Running log of what the de-risk spikes actually proved, including corrections to
the approved plan. Numbers here come from measurement, not estimation.

## Environment (verified 2026-08-17)

| Thing | State |
|---|---|
| Node | 24.16.0 |
| npm | 11.13.0 |
| git | 2.39.0.windows.2 |
| WebView2 runtime | **151.0.4129.86 already present** — Tauri ships no browser engine |
| `LongPathsEnabled` | **0** — confirms the 259-char ceiling assumed by the naming design |
| Rust / cargo | absent → installing |
| Visual Studio / Build Tools | absent → installing (VCTools + Windows SDK) |
| Go | absent, and **not needed** — pdfcpu ships as a prebuilt binary |
| Free disk | C: 170 GB, D: 148 GB |

## Spike 1 — pdfcpu export pipeline: **PASS** (synthetic fixtures)

pdfcpu **v0.15.0**, 14.9 MB single binary, built 2026-08-11 (six days before this
spike — actively maintained, which was the whole reason it displaced pdf-lib).

Pipeline proven end to end on 5 mixed-size images
(A4 portrait, landscape, 12 MP phone photo, Folio/F4, square):

```
import 'f:A4, pos:c, sc:1.0 rel'  →  merge --bookmarks  →  resize formsize:A4P  →  optimize
```

| Step | Time |
|---|---|
| image → A4 PDF (5 files) | 0.47 s |
| merge | 0.10 s |
| resize A4 | 0.10 s |
| optimize | 0.11 s |
| **total** | **0.78 s** |

Verified in the output:

- **Page uniformity holds.** `pageSizes` returns a *single* entry, `595x842` pt —
  every page is A4 portrait. Landscape and Folio sources were fitted, not left at
  their native size. This is the property that stops a doctor's viewer jumping and
  rezooming between records.
- **A4 fitting happens at `import`, not `resize`.** For image sources the resize
  step is a no-op; it earns its 0.1 s only on source PDFs, which pass through
  `merge` at their native page size. Keep it.
- **Bookmarks come free and correct.** `merge --bookmarks` names each outline entry
  after its input filename. Because staged files already carry the canonical
  `date_patient_test` name, the export outline is correct with no extra work.

### Corrections to the approved plan

1. **`linearize` does not exist in pdfcpu 0.15.0.** The plan listed it as a
   first-class command; it is absent from the command set, and `info` reports
   `linearized: False` after `optimize`.
   *Impact: low.* Linearization buys progressive rendering for PDFs streamed from a
   web server. These files are delivered as complete downloads over WhatsApp, email
   or USB, so the viewer has the whole file before it opens it. **Dropping it.**
   If it is ever wanted, `qpdf --linearize` (Apache-2.0) is the add-on — and qpdf is
   already the planned tool for the malformed-PDF repair path.
2. **`encrypt` does exist** — AES output password protection for an exported summary
   is confirmed available, as the plan claimed.
3. **`create` exists: "Create PDF content including forms via JSON".** Worth
   evaluating against the planned canvas → PNG → import route for the cover page and
   per-page header bands. The canvas route was justified largely by Bangla text
   shaping; since the user confirmed **no Bangla is needed in app-generated text**,
   the native JSON route may be simpler and avoids rasterizing text. Evaluate in
   Phase 1 before building the cover page.

### Still unmeasured — needs the user's real documents

Wall time, peak RSS and output size on synthetic 85 KB images prove the mechanics,
not the budget. The numbers that gate the design come from real 12 MP phone photos:

- output MB for ~45 real pages vs the 25 MB email cap
- whether the Standard preset lands in the predicted 16–27 MB band
- peak RSS on a large merge

Run: `npm run spike:merge -- "<folder>" --limit 45`

## Spike 2 — rusqlite / FTS5 / WAL / VACUUM INTO: **PASS**

`spikes/sqlite-fts5/`. Clean release build in 1m17s.

```
sqlite version : 3.50.2
ENABLE_FTS5    : true
create fts5    : OK
match query    : 1 hit(s) for 'creatinine'
snippet()      : ...14/03/2026 [TSH] 6.82 uIU/mL...
prefix query   : 1 hit(s) for 'chol*'
journal_mode   : wal
VACUUM INTO    : OK (8192 bytes)
```

All three load-bearing properties hold: FTS5 virtual tables create and match,
`snippet()` returns usable search context for the results UI, prefix queries work,
WAL engages, and `VACUUM INTO` is available for snapshot-on-close backup.

Building `libsqlite3-sys` also compiles the SQLite amalgamation from C, so this
doubles as proof that the freshly installed MSVC toolchain works end to end.

### Correction to the approved plan

**`rusqlite` 0.37 has no `fts5` feature.** The plan specified
`features = ["bundled", "fts5"]`; that does not compile:

```
package `spike-sqlite-fts5` depends on `rusqlite` with feature `fts5`
but `rusqlite` does not have that feature
```

The correct dependency is **`features = ["bundled"]` alone** — the bundled
amalgamation is compiled with `SQLITE_ENABLE_FTS5` already, confirmed by querying
`pragma_compile_options` at runtime rather than trusting the docs.

This is exactly the failure the spike existed to catch, and it surfaced at build
time rather than after the schema had committed to full-text search.

Toolchain versions: rustc 1.97.1, cargo 1.97.1, MSVC 14.44.35207,
Windows SDK 10.0.26100, VS BuildTools 17.14.37531.7.

## Spike 3 — Windows.Media.Ocr: **engine PASS**, accuracy needs real photos

Reached through the WinRT projection in Windows PowerShell 5.1 — the same engine the
`windows` crate will call from Rust, so this measures the real thing with no Rust
installed. `spikes/ocr.ps1`.

Measured on this machine:

| Property | Value |
|---|---|
| Engine | available, `TryCreateFromUserProfileLanguages()` succeeds |
| Recognizer language | English (United States), `en-US` |
| **Available languages** | **`en-US` only** |
| `MaxImageDimension` | 10000 px — a 12 MP phone photo (3024×4032) is comfortably under |
| Speed | **~350 ms/page** for a full A4 lab report at 1654×2339 |

On a synthetic but realistic Bangladeshi lab report (patient block, collection /
received / reported dates, DOB, thyroid panel with reference ranges, "Printed on"
footer) the engine returned **27 lines / 85 words** and recognised the text cleanly.

**The ranking problem reproduced on the first document.** It found three dates:

```
12/03/1978   <- Date of Birth      (the trap: files the report under 1978 forever)
14/03/2026   <- Sample Collected   (the correct answer)
15/03/2026   <- Reported On / Printed on
```

and the positive-anchor regex (`collect|sample|received|reported|...`) matched. This
is direct evidence for two plan decisions: recognition is not the bottleneck,
**ranking is** — and the anchor-proximity scoring with DOB weighted strongly negative
is both necessary and feasible, because the anchor words are present in the OCR text.

`en-US` being the only installed recognizer is fine given the user confirmed English
printed reports. If a Bangla pack is ever needed it is a Windows language-pack
install, not a code change.

Not yet answered — the real gate: **date recall on actual phone photos**, where
skew, shadow, curl and JPEG noise apply. Clean synthetic text proves the plumbing,
not the accuracy. Run: `powershell -File spikes\ocr.ps1 -Path "<folder>" -Limit 50`

## Spike 4 — EXIF orientation: harness ready, **needs real photos**

`src/lib/ingest/sniff.ts` implements magic-byte type detection and a direct EXIF
orientation reader (APP1 → TIFF → IFD0 tag 0x0112), both little- and big-endian.
30 unit tests pass, covering all 8 orientation values, absent EXIF, truncated
buffers, and the JPEG-masquerading-as-PDF case.

`spikes/survey.ts` reports, over a real folder: kind distribution by magic bytes,
how many extensions lie, the orientation histogram, digital-vs-scanned PDF split,
and the longest source path against the 259-char ceiling.

Run: `npm run spike:survey -- "<folder>"`

**This is the spike that decides how much risk #2 actually carries.** The plan
assumes 20–40% of phone photos are non-upright; that is a published range, not a
measurement of this user's camera and workflow.

## Date ranking — built early, because it is risk #3

Ranking was scheduled for Phase 3, but the OCR spike handed over real engine output
containing the exact failure the plan warns about, so it was worth building and
testing immediately. `src/lib/extract/dates.ts`, 16 tests.

The Phase 3 test fixture is the **verbatim** `Windows.Media.Ocr` output — flattened
table columns, `ulU/mL` I/l confusion and all — not cleaned-up text.

Ranking behaviour proven on it:

| Date in document | Label | Outcome |
|---|---|---|
| `12/03/1978` | Date of Birth | **hard-rejected** (pre-1990 floor + DOB anchor at −400) |
| `14/03/2026` | Sample Collected | **winner** |
| `14/03/2026` | Received | same value, weaker label — best evidence kept |
| `15/03/2026` | Reported On | runner-up, offered as a one-click chip |
| `15/03/2026` | Printed on | same value, keeps the better "reported" label |

Also covered: day-first convention, MM/DD swap when the second field exceeds 12,
two-digit years rolling backwards rather than into the future, textual months in both
orders, ISO dates from digital PDF text, invalid calendar dates discarded rather than
clamped, expiry/next-appointment labels losing, patient-DOB hard rejection, and the
handwritten-prescription case returning **nothing** to prefill — the normal path, not
an error.

Three layered defences against the silent-wrong-date failure, in order of strength:
hard rejection (pre-1990, future, equals stored patient DOB) → negative anchors
(DOB −400, age −200, expiry −150, printed −120) → positive anchors (collected +100
down to a bare "date" +15), with a small header-position bonus as tie-break.

`OcrResult.Text` **flattens layout** — the results table came back as
`Test TSH FT3 FT 4 Result 6.82 2.91 0.88`, column association destroyed. Anchor
proximity survives this because labels stay adjacent to their values in reading
order, but it confirms the plan's requirement to persist word boxes: any future
table or result extraction needs geometry, not the flat string.

## Naming layer — built and tested ahead of schedule

`src/lib/naming/sanitize.ts` is complete and covered by 20 tests: illegal
characters, C0 controls, reserved DOS device names (including as folder names),
trailing dots/spaces, invisible and bidi characters, NFC normalization,
grapheme-safe truncation, the 259-char path budget under a deep OneDrive-style
root, monotonic zero-padded collision suffixes that sort correctly, and the
`YYYY-MM-00` / `0000-00-00` date sentinels.

One design change surfaced by a failing test: `titleTruncated` now means *the
on-disk title is shorter than the stored title, for any reason* — originally it
only reported MAX_PATH-driven truncation and stayed false when the 60-char title
cap had already cut the name. The review grid needs the broader meaning so the
user is never surprised that the file on disk reads differently.
