/**
 * Filing a whole selection, one row at a time.
 *
 * Kept out of the component so the part that matters — what gets filed, what
 * gets left behind, and what the user is told about it — can be tested without a
 * browser.
 */

export interface BulkTarget {
  ready: boolean;
  /** Why this row would be skipped, in words the user can act on. */
  skipReason: string | null;
  /** Resolves to the new document id, or null if the row refused. */
  file: () => Promise<string | null>;
}

export interface Skipped {
  name: string;
  reason: string;
}

export interface BulkOutcome {
  /** Document ids, in the order they were filed. */
  filed: string[];
  skipped: Skipped[];
}

/**
 * File each id in turn.
 *
 * Sequential on purpose: every commit reserves a collision suffix and moves a
 * file through the journal, so firing hundreds at once buys nothing but
 * contention. One row failing never stops the batch — in a 200-file import,
 * abandoning the remaining 180 because of one bad scan is the worst possible
 * response.
 */
export async function fileEach(
  ids: readonly string[],
  lookup: (id: string) => { name: string; target: BulkTarget } | null,
  onProgress?: (done: number, total: number) => void,
): Promise<BulkOutcome> {
  const filed: string[] = [];
  const skipped: Skipped[] = [];

  for (const [n, id] of ids.entries()) {
    const row = lookup(id);
    // Gone from the queue between selecting and filing — nothing to report.
    if (!row) continue;

    if (!row.target.ready) {
      skipped.push({ name: row.name, reason: row.target.skipReason ?? 'not ready' });
      continue;
    }

    onProgress?.(n + 1, ids.length);
    try {
      const documentId = await row.target.file();
      if (documentId) filed.push(documentId);
      else skipped.push({ name: row.name, reason: 'could not be filed' });
    } catch (e) {
      skipped.push({ name: row.name, reason: String(e) });
    }
  }

  return { filed, skipped };
}

/**
 * One line saying what happened.
 *
 * Names the first few that were left, because "filed 34, skipped 6" tells the
 * user nothing about which six or what to do next.
 */
export function summarize(outcome: BulkOutcome, shown = 3): string {
  const { filed, skipped } = outcome;
  const count = `Filed ${filed.length}`;
  if (skipped.length === 0) return `${count}.`;

  const detail = skipped
    .slice(0, shown)
    .map((s) => `${s.name} — ${s.reason}`)
    .join(' · ');
  const more = skipped.length > shown ? ` · and ${skipped.length - shown} more` : '';
  return `${count}, left ${skipped.length}: ${detail}${more}`;
}
