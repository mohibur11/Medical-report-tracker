/**
 * Download the pdfcpu sidecar into src-tauri/binaries/.
 *
 * The binary is ~15 MB, so it is fetched rather than committed — git keeps every
 * version of a binary forever, and this one changes with each pdfcpu release.
 *
 * Tauri requires the target triple in the filename (`pdfcpu-<triple>.exe`) and
 * copies it next to the built executable, both for `cargo build` and for the
 * installer.
 *
 * Usage: npm run fetch:sidecar
 */

import { execFileSync } from 'node:child_process';
import { createWriteStream } from 'node:fs';
import { mkdir, rm, stat } from 'node:fs/promises';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { join } from 'node:path';

const VERSION = '0.15.0';
const OUT_DIR = 'src-tauri/binaries';

const DEFAULT_TRIPLE = 'x86_64-pc-windows-msvc';

/** Ask rustc, but do not require it — someone may just be running the JS tests. */
function targetTriple() {
  for (const exe of ['rustc', join(process.env.USERPROFILE ?? '', '.cargo', 'bin', 'rustc.exe')]) {
    try {
      const out = execFileSync(exe, ['-vV'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
      const host = out.split('\n').find((l) => l.startsWith('host:'));
      if (host) return host.slice('host:'.length).trim();
    } catch {
      // try the next candidate
    }
  }
  console.warn(`rustc not found; assuming ${DEFAULT_TRIPLE}`);
  return DEFAULT_TRIPLE;
}

const triple = targetTriple();
if (!triple.includes('windows')) {
  console.error(`This project currently targets Windows; rustc reports ${triple}.`);
  process.exit(1);
}

const asset = `pdfcpu_${VERSION}_Windows_x86_64.zip`;
const url = `https://github.com/pdfcpu/pdfcpu/releases/download/v${VERSION}/${asset}`;
const dest = join(OUT_DIR, `pdfcpu-${triple}.exe`);

if (await stat(dest).catch(() => null)) {
  console.log(`already present: ${dest}`);
  process.exit(0);
}

await mkdir(OUT_DIR, { recursive: true });
const tmpZip = join(OUT_DIR, asset);

console.log(`downloading pdfcpu ${VERSION}…`);
const res = await fetch(url, { redirect: 'follow' });
if (!res.ok || !res.body) throw new Error(`download failed: ${res.status} ${res.statusText}`);
await pipeline(Readable.fromWeb(res.body), createWriteStream(tmpZip));

// PowerShell is always present on the target platform, so no unzip dependency.
const tmpDir = join(OUT_DIR, '_unzip');
execFileSync('powershell', [
  '-NoProfile',
  '-Command',
  `Expand-Archive -LiteralPath '${tmpZip}' -DestinationPath '${tmpDir}' -Force`,
]);

const found = execFileSync('powershell', [
  '-NoProfile',
  '-Command',
  `(Get-ChildItem -LiteralPath '${tmpDir}' -Recurse -Filter pdfcpu.exe | Select-Object -First 1).FullName`,
], { encoding: 'utf8' }).trim();

if (!found) throw new Error('pdfcpu.exe not found inside the archive');

execFileSync('powershell', [
  '-NoProfile',
  '-Command',
  `Copy-Item -LiteralPath '${found}' -Destination '${dest}' -Force`,
]);

await rm(tmpZip, { force: true });
await rm(tmpDir, { recursive: true, force: true });

const { size } = await stat(dest);
console.log(`wrote ${dest} (${(size / 1024 / 1024).toFixed(1)} MB)`);
