# Medicine Report Tracker

Offline desktop vault for paper medical records. Ingests photos and PDFs of lab
reports and prescriptions, files them under `<Patient>/<Year>/` with sortable
canonical filenames, tags them with categories, and — the point of the whole thing —
exports any filtered slice as **one merged, date-ordered PDF a doctor can scroll top
to bottom**.

Windows first. Web and Android later.

**Status: usable.** Ingest, review in bulk, filing, categories, search, notes,
editing, patient rename, merged-PDF export with saved filters, and backup to
Google Drive all work; recognition pre-fills the review screen. See
[docs/testing.md](docs/testing.md) for a walkthrough and
[docs/phase-0-findings.md](docs/phase-0-findings.md) for what was measured.

Still open: putting a document back from Trash without using Explorer, golden-file
export tests, accuracy measured on a larger sample, and code signing.

## Backing up to Google Drive

Two ways, and the app picks whichever is available.

**Signing in to a Google account** uploads over the Drive API and works on any
machine. It needs a one-time setup, because Google issues OAuth credentials per
application and this one is published as source: a client ID committed here would
be spent by strangers against the quota and would put someone else's name on the
consent screen. The panel walks through it — create a Cloud project, enable the
Drive API, create an **OAuth client ID of type Desktop app**, publish the consent
screen, paste the ID and secret.

Publishing the consent screen is not optional in practice. Left in **Testing**,
Google revokes the sign-in every seven days.

**Without an account**, it copies into the folder Google Drive for desktop already
mounts (`G:\My Drive` and friends), which needs no credentials at all.

Either way, what lands in Drive is the vault itself: ordinary folders, canonical
filenames, a `.meta.json` beside each document. Readable in a browser without this
app, which is the point. `Exports/` is left out — it rebuilds from the documents.

The scope requested is `drive.file`, which reaches only files this app created. It
cannot see the rest of your Drive. Because that scope is non-sensitive, publishing
the consent screen needs no paid security assessment.

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

cd src-tauri && cargo test   # Rust tests (naming, ingest, vault, export, search,
                             # reconciler, sync, OAuth, DPAPI, upright)
```

365 tests at present: 108 TypeScript, 257 Rust. The Rust suite drives real files
and a real SQLite database rather than mocks, so it takes about half a minute.

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

### The one thing that is encrypted

A Google refresh token is not the user's data — it is standing permission to reach
their Drive, and it stays valid until revoked. So it is sealed with **DPAPI**,
tied to the Windows account, and a copied database yields nothing. Disconnecting
revokes it with Google rather than merely forgetting it.

That is the exception. Everything else is plaintext on purpose, as above.

### Hardening already in place

- `app.security.csp` is a hand-written policy, verified against a release build:
  IPC under `connect-src`, `data:` under `img-src` for thumbnails, `'unsafe-inline'`
  under `style-src`, and `object-src 'none'`.
- A single-instance guard, so a second launch focuses the running window instead
  of opening a second copy to race it over the same files. A file passed to that
  second launch is handed over rather than dropped.
- The window is registered as a *viewer* for scans and PDFs, so it appears under
  "Open with" and takes over as nobody's default.

## Layout

```
src/lib/naming/     canonical filenames, path budget, collision suffixes
src/lib/ingest/     magic-byte type detection, EXIF orientation
src/lib/extract/    date candidate extraction and ranking
src/lib/pdf.ts      pdf.js: text already inside a PDF, read before OCR is asked
src-tauri/src/
  vault.rs          journalled moves: commit, edit, rename a patient, trash
  ingest.rs         staging, dedupe, page counts, locked-PDF detection
  export.rs         the merged PDF, and the loose-files escape hatch
  ocr.rs            Windows.Media.Ocr, off the database lock, cached
  sync.rs           BackupTarget, and the Drive-for-desktop folder copy
  google.rs         OAuth: PKCE, loopback listener, token refresh
  drive.rs          Drive API: folder mapping, resumable uploads, restore
  secret.rs         DPAPI, for the one secret worth protecting
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
