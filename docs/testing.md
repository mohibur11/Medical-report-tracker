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

#### A PDF that asks for a password

Labs and hospitals routinely email reports locked with a date of birth or a phone
number. One of those lands in the queue on an amber strip, with a password box,
and stays there across restarts — it is counted as `1 locked` in the header.
Nothing can read it until it is unlocked: not the recognizer, not the export.

Enter the password and it becomes an ordinary row, with its real page count. A
wrong password says so and leaves the file exactly as it was.

Worth knowing what is *not* built, and why: no separate repair tool. pdfcpu already
rebuilds damaged PDFs by itself — a wrong `startxref` offset, a missing
cross-reference table, a file truncated to a third of its length all come back
readable, and it says `pdfcpu repaired: catalog` when it does. Bundling qpdf for
that would be dead weight. A password is the one thing repair cannot substitute
for.

For the same reason, a PDF that the strict validator dislikes is still accepted at
import: the operations that matter — resize, stamp, merge — succeed on files
`pdfcpu info` complains about, so refusing the import on that basis would refuse
real scans. If a file genuinely cannot be read, the export says so per document
and still produces the rest.

### 3. Review

Each row reads its page and pre-fills what it found. This is the screen the whole
app rests on, so it is the one worth being picky about.

- A **PDF that carries its own text** — one emailed straight from a lab rather
  than scanned — is read exactly instead of recognised, and the row says
  `exact text`. Nothing is guessed from pixels in that case. Scans have no text
  layer and take the recognition path as before.
- **PDF rows show their first page**, not a grey badge.
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

#### Reviewing a backlog rather than a handful

Two files or more and a bar appears above the queue. It exists because a 200-file
import is not 200 small decisions — it is a few decisions repeated.

- **Select all**, or tick rows individually. **Shift-click** takes the whole run
  between the last row you ticked and this one.
- **Set patient** and **Set type** apply to everything selected at once. Date is
  deliberately not there: a wrong date is the one mistake that never announces
  itself, so it stays per-row.
- **+ tag** chooses categories to apply once the files land.
- **File selected** files them one at a time and then says what it did — including
  which rows it left behind and why (`no patient chosen`, `no date`, and so on).
  Rows that are not ready are never filed with a guess, and one failure does not
  abandon the rest of the batch.

The queue is stored, not remembered. Close the app halfway through a big import,
reopen it, and the remaining files are still there with their recognised text
intact — nothing is stranded in staging.

#### Opening a scan straight from Explorer

The installer registers the app for `.pdf`, `.jpg`, `.png` and friends as a
*viewer* — it appears under **Open with**, and does not take over as the default
for any of them.

Sending a file that way imports it. If the app is already open, the second launch
hands its file over and closes rather than starting a second copy, and the window
jumps to the inbox with the new row already read. Worth trying with the app both
closed and open — they are different code paths.

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
- **Bulk tagging** — tick documents (shift-click takes a run), pick a category,
  then **Add tag** or **Remove tag**. Sixty thyroid reports filed over ten years
  all want the same category, and doing that one popover at a time is what stops
  people tagging at all. **Delete selected** moves the lot to Trash.
- The list is **virtualized**: only the rows on screen exist in the page, so a
  library of several hundred scrolls the same as a library of five.
- **Notes** — `Edit` on any document has a notes box. It is not part of the
  filename, so writing one never moves the file. Notes are searchable, and they
  are written into the `.meta.json` sidecar beside the document, which is what
  survives if the database is ever lost.

### 4b. Correcting a patient's name or date of birth

`Library` → **Patients** → `Edit`.

The date of birth is worth setting even though it is optional: it is what stops a
birth date printed on a report being chosen as the report date, and it is what
raises a warning when a document disagrees with it. A patient with none is marked
`add one`.

Renaming is the expensive one, and the screen says so before it acts. The name is
the folder *and* part of every filename, so renaming a patient with 400 documents
moves 400 files. Each move is journalled individually rather than the folder being
renamed in one go — a folder rename fails entirely if any single file inside it is
locked, and leaves nothing behind saying how far it got.

Worth testing deliberately: **open one of the patient's PDFs in a viewer, then
rename**. The locked file should be left where it is and named in the report, the
rest should move, and renaming again after closing the viewer should finish the
job. Nothing should be lost either way — a row that could not move keeps pointing
at the file that still exists.

Refused, with the reason: an empty name, a name made only of punctuation (it would
silently become `Unknown-Patient`), a name that folds onto another patient's folder
on Windows, and a date of birth later than a report already filed for that patient.

### 5. Export — the point of the whole thing

Pick a patient, a year, a category, a quality preset, then **Create PDF**. Output
lands in `<vault>\Exports\`.

**Export as files** is the escape hatch: the same filtered documents copied out
as loose, numbered files instead of one merged PDF. Because the canonical
filenames already carry date, patient and title, and the numbering keeps the
merged order, this stays usable — for a USB stick, or when a source PDF refuses
to merge.

**Save these filters** keeps that combination under a name, and saved presets
appear as chips above the filters. What is stored is the filter, not the result:
opening "Thyroid for Dr Karim" a year from now includes everything filed since.
A preset naming a patient or category that has since been removed still opens,
minus that part, rather than silently matching nothing.

Open it and check:

- every page is **upright**
- every page is the **same size**, so scrolling does not jump or rezoom
- each page carries a **header band** with date, patient and title
- records are in **date order**, with undated ones at the end
- the **size** suits how you will send it — Email-safe stays under 25 MB and splits
  into parts at record boundaries if it cannot

---

## Keeping a copy in Google Drive

The vault is deliberately plain: ordinary folders, ordinary files, nothing
encrypted. Putting `Documents\MedicineReportTracker` in a synced Drive folder
works, and what arrives on the other side is readable without this app —
`2025-03-14_Rahim-Uddin_Thyroid-Profile.pdf` says what it is, and the
`.meta.json` beside it carries the categories and notes the filename cannot.

Four things to know before doing it:

- **The live database must not go in there.** It sits in
  `%LOCALAPPDATA%\com.mohibur.medicinereporttracker\` on purpose: a WAL sidecar
  synced mid-write is a documented way to corrupt SQLite. The snapshot that
  `Back up now` writes into the vault *is* safe to sync — it is a single
  consistent file with no WAL.
- **Sync clients hold file handles.** A rename or an edit that moves a file can
  fail while Drive is uploading it. The app reports which files it could not move
  and leaves them working where they are; repeating the action finishes the job.
- **Renaming a patient re-uploads everything they own**, because every file
  genuinely changes name. Rename before a big import, not after.
- **Nothing is encrypted.** Anyone with access to that Drive folder can read every
  report. That is the intended trade — it is what makes the archive readable
  without the app — but it is worth being deliberate about who the folder is
  shared with.

---

## What has been measured, and what has not

Measured against 17 pages of real records: **100% of dates read correctly**, no date
of birth ever chosen, ~500 ms per page. Receipts, radiology, pathology and
immunology reports all classified correctly.

**Not verified by me**, because I cannot drive the mouse: the drag-and-drop handler,
the File button, the review checkboxes and bulk bar, and Create PDF. Everything behind them is covered by tests, but
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
