# Trying it out

Two ways to run it. Use the installed build if you just want to use the app; use
dev mode if you want to see errors when something misbehaves.

```sh
npm install
npm run fetch:sidecar     # once — downloads pdfcpu (~15 MB), not committed
npm run dev               # dev mode, opens devtools, hot reload
```

Or install the packaged build from
`src-tauri/target/release/bundle/nsis/`. Windows will warn that the publisher is
unrecognised — the app is unsigned. Click **More info → Run anyway**. Code signing
costs a few hundred dollars a year and is only worth it if other people install it.

Everything lives in two places:

| | |
|---|---|
| `%USERPROFILE%\Documents\MedicineReportTracker\` | the vault — your files, browsable in Explorer |
| `%LOCALAPPDATA%\com.mohibur.medicinereporttracker\` | the database (deliberately not in the vault) |

To start completely fresh, close the app and delete both.

---

## A run through, in the order it is meant to be used

### 1. Add a patient — and give them a date of birth

`+ Patient`. **Enter the date of birth.** It is not decoration: it is what stops a
report being filed under a birth year, and it is what raises a warning when a
document disagrees with it.

Names that differ only by capitalisation or punctuation are refused, because
Windows would give them the same folder.

### 2. Drop files in

Drag scans or PDFs anywhere in the window. Try dropping a whole folder at once.

Watch for:

- **Photos that were sideways come out upright**, and say `rotated upright (EXIF 6)`.
- **The same file twice is rejected** as a duplicate — by content, so renaming it
  first makes no difference.
- **An iPhone HEIC is skipped** with an explanation, not a crash.
- **A multi-page PDF reports its real page count**, not 1.

`tests/fixtures/photos/` has files built to exercise exactly these.

### 3. Review

Each row reads its page and pre-fills what it found. This is the screen the whole
app rests on, so it is the one worth being picky about.

- The **date** is pre-filled from the page. The chips beside it are the other dates
  found, labelled with the evidence — `sample collected`, `reported`, `date of
  birth`. Clicking one replaces the date.
- The **title** and **type** are suggested from a lexicon. Receipts should read as
  invoices, not prescriptions.
- The **filename preview** shows exactly what will be written to disk.
- The **patient carries over** to the next row, since a backlog is usually one
  person at a time.

Things it should refuse to file, with an explanation rather than a shrug:

- a date in the future
- a date before the patient was born
- a date that is not a real date

Things it should ask about rather than decide:

- **an ambiguous date** — `04-07-2026` is 4 July or 7 April, and if nothing else on
  the page settles it you get both options
- **a date of birth that disagrees** with the patient profile
- **two dates it could barely separate**

### 4. Library

- **Search** titles, notes and recognised text. Try a word from inside a scan —
  matches show the surrounding text.
- **Categories** — create one, tag documents, rename it. Renaming must not lose
  tags. Archiving keeps them.
- **Rescan vault** — the interesting one. Go into Explorer, move a filed file to a
  different year folder, delete another, drop a stray photo into a patient folder,
  then rescan. It should follow the moved file by content, flag the deleted one
  without removing it, adopt anything with a canonical name, and leave anything it
  does not recognise strictly alone.
- **Back up now** — writes a database snapshot into the vault. Also happens
  automatically when you close the app.

### 5. Export — the point of the whole thing

Pick a patient, a year, a category, a quality preset, then **Create PDF**. Output
lands in `<vault>\Exports\`.

Open it and check:

- every page is **upright**
- every page is the **same size**, so scrolling does not jump or rezoom
- each page carries a **header band** with date, patient and title
- records are in **date order**, with undated ones at the end
- the **size** suits how you will send it — Email-safe stays under 25 MB and splits
  into parts at record boundaries if it cannot

---

## What has been measured, and what has not

Measured against 17 pages of real records: **100% of dates read correctly**, no date
of birth ever chosen, ~500 ms per page. Receipts, radiology, pathology and
immunology reports all classified correctly.

**Not verified by me**, because I cannot drive the mouse: the drag-and-drop handler,
the File button, and Create PDF. Everything behind them is covered by tests, but
those three clicks are exactly what to try first.

Also unverified: how the exported PDF actually *looks*. The tests assert page size,
uniformity, ordering and that the header band is present — none of that is the same
as your judgement of whether a doctor would take it seriously.

## Known rough edges

- **Patient rename is not implemented.** Renaming a patient means renaming every one
  of their files, which needs the crash-safe journal; it is deliberately deferred.
- **No bulk assign.** Files are reviewed one row at a time. Fine for a few, tedious
  for two hundred.
- **The content security policy is switched off.** It was removed while debugging an
  unrelated fault and not restored. It only matters for a packaged release.
- **The app is unsigned**, so Windows SmartScreen will warn on first run.
- **PDFs with a real text layer are not read directly** — their pages are recognised
  as images instead. Slower and less exact, but every PDF in this archive is a pure
  scan, so it makes no practical difference yet.
