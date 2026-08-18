# Medicine Report Tracker

Offline desktop vault for paper medical records. Ingests photos and PDFs of lab
reports and prescriptions, files them under `<Patient>/<Year>/` with sortable
canonical filenames, tags them with categories, and — the point of the whole thing —
exports any filtered slice as **one merged, date-ordered PDF a doctor can scroll top
to bottom**.

Windows first. Web and Android later.

**Status: usable.** Ingest, review, filing, categories, search, backup, editing and
merged-PDF export all work; recognition pre-fills the review screen. See
[docs/testing.md](docs/testing.md) for a walkthrough and
[docs/phase-0-findings.md](docs/phase-0-findings.md) for what was measured.

Still open: patient rename, malformed-PDF repair, code signing.

## Signing the installer

The installer is unsigned, so Windows SmartScreen shows *"Windows protected your
PC"* on first run and the publisher reads as unknown. Click **More info → Run
anyway**. That is a nuisance for the person who built it and a genuine warning
sign for anyone else, so it only matters once this is handed to other people.

Signing is configuration, not code — everything except the certificate is already
in `tauri.conf.json`. Set `bundle.windows.certificateThumbprint` to the thumbprint
of a code-signing certificate in the Windows certificate store and rebuild.

What it costs: an OV certificate runs roughly $200-400 a year and still
accumulates SmartScreen reputation slowly; an EV certificate costs more, needs a
hardware token, and carries reputation immediately. A self-signed certificate
silences nothing for anyone but the machine that trusts it, so it is not worth
the trouble.

## Requirements

- Node 24+
- Rust (stable, MSVC toolchain) + Visual Studio Build Tools with the C++ workload
- WebView2 runtime — already present on Windows 11

### If `npm run dev` says `cargo metadata ... program not found`

Cargo is not on PATH in that terminal. Installing Rust updates the persisted user
PATH, but a terminal keeps whatever environment it started with — and VS Code's
integrated terminal inherits from the VS Code process, so opening a new tab in the
same window does not help either.

For the terminal you are in:

```powershell
$env:Path += ";$env:USERPROFILE\.cargo\bin"
```

Permanently: quit VS Code completely and reopen it.

## Commands

```sh
npm install
npm run fetch:sidecar # download the pdfcpu binary (~15 MB, not committed) — do this once
npm run dev          # Tauri dev: Vite + the Rust backend, hot reload on both
npm run build        # NSIS installer into src-tauri/target/release/bundle
npm test             # TypeScript unit tests
npm run typecheck    # tsc --noEmit

cd src-tauri && cargo test   # Rust tests (naming, ingest, vault, export, search, reconciler)
```

All PDF assembly runs through **pdfcpu**, bundled as a Tauri sidecar. It is fetched
rather than committed, because git keeps every version of a binary forever.

## Where things live

| Path | What |
|---|---|
| `%USERPROFILE%\Documents\MedicineReportTracker\` | the vault — browsable, plaintext, portable |
| `%LOCALAPPDATA%\com.mohibur.medicinereporttracker\app.db` | live SQLite, **never** in the vault |
| `%LOCALAPPDATA%\...\staging\` | imported but not yet committed to a patient |

The vault is deliberately readable in Explorer and copyable to a USB stick without
this app. The live database is deliberately *outside* it: Documents is routinely
OneDrive-synced, and a WAL sidecar synced mid-transaction is a documented corruption
path. Backup is a `VACUUM INTO` snapshot written into the vault, not the live file.

## Security posture — read this

The password is an **honest screen lock**, not encryption. Files and the metadata
database are plaintext on disk. Anyone with access to the Windows account can open
the vault folder and read every report.

That is a deliberate trade: encrypting the vault would destroy Explorer browsing,
thumbnails, Windows Search, USB portability and app-independent recovery — the
properties that make the archive outlive the app.

**Use BitLocker** for real protection at rest. It is free on Windows 11 Pro.

### Open security item

`app.security.csp` is currently `null`, so Tauri's permissive default applies. A
hand-written CSP was in place and was removed while debugging an unrelated rendering
fault — it turned out not to be the cause, and was not restored. Before release,
put a tested CSP back: it must allow the IPC origin (`ipc:` / `http://ipc.localhost`)
under `connect-src`, `data:` under `img-src` for thumbnails, and `'unsafe-inline'`
under `style-src` for Vite's injected styles. Tracked for Phase 4 hardening.

## Layout

```
src/lib/naming/     canonical filenames, path budget, collision suffixes
src/lib/ingest/     magic-byte type detection, EXIF orientation
src/lib/extract/    date candidate extraction and ranking
src-tauri/          Rust: SQLite, file ops, OCR, pdfcpu sidecar
db/migrations/      schema, applied by version and never edited after release
spikes/             Phase 0 de-risk harnesses, kept as measurement tools
```

## Conventions that are load-bearing

- **Dates are bare text `YYYY-MM-DD`.** Never an epoch, never a timezone. The UI
  displays `DD/MM/YYYY`; only disk and DB are ISO. Sentinels `YYYY-MM-00` and
  `0000-00-00` sort correctly and feed the undated queue.
- **Identity is a ULID.** Never a name, a path, or an email — all three change.
- **Filenames are a projection, not identity.** Correcting a title renames a file;
  it must not invalidate anything.
- **File type comes from magic bytes.** Scanner apps emit `.pdf` files containing
  JPEG bytes; trusting the extension corrupts the export.
- **EXIF orientation is baked into pixels at ingest.** No PDF library reads the tag,
  so an untreated photo lands sideways in every merged export.
