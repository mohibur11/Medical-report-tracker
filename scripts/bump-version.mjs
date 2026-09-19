/**
 * Set the app's version everywhere it is written down.
 *
 * The version lives in package.json; tauri.conf.json points at it, so the
 * installer's name and the About line follow automatically. Cargo carries its
 * own copy because it has to, and the two lockfiles echo theirs. This keeps all
 * four in step so a release is one number, not a hunt.
 *
 * Usage: npm run version:bump -- 0.3.0
 * Then:  git commit -am "Release 0.3.0" && git tag v0.3.0 && git push --tags
 *
 * Pushing the tag is what builds the installer and publishes the release —
 * see .github/workflows/release.yml.
 */

import { readFileSync, writeFileSync } from 'node:fs';

const next = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(next ?? '')) {
  console.error('usage: npm run version:bump -- <major.minor.patch>');
  process.exit(1);
}

/**
 * Replace in place. The pattern's first group is what precedes the version and
 * its second is the version itself. A file already at the target is fine — a
 * half-finished bump can be run again.
 */
function rewrite(file, pattern) {
  const before = readFileSync(file, 'utf8');
  const match = before.match(pattern);
  if (!match) {
    console.error(`${file}: nothing matched — has the layout changed?`);
    process.exit(1);
  }
  const was = match[2];
  if (was === next) {
    console.log(`${file}: already ${next}`);
    return;
  }
  writeFileSync(file, before.replace(pattern, `$1"${next}"`));
  console.log(`${file}: ${was} -> ${next}`);
}

rewrite('package.json', /^(\s*"version":\s*)"([^"]+)"/m);
// The root package appears twice in the lockfile: at the top and under "".
rewrite('package-lock.json', /^(\s*"version":\s*)"([^"]+)"/m);
rewrite('package-lock.json', /("":\s*\{\s*"name":\s*"medicine-report-tracker",\s*"version":\s*)"([^"]+)"/);
rewrite('src-tauri/Cargo.toml', /^(version\s*=\s*)"([^"]+)"/m);
rewrite('src-tauri/Cargo.lock', /(name = "medicine-report-tracker"\r?\nversion = )"([^"]+)"/);

console.log(`\nNow:\n  git commit -am "Release ${next}"\n  git tag v${next}\n  git push && git push --tags`);
