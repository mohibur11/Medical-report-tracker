import { useState } from 'react';

import { ReviewCard } from './ReviewCard.tsx';
import { useBusy } from '../busy.tsx';
import {
  discardStaged,
  pickAndImport,
  type Category,
  type IngestItem,
  type Patient,
} from '../../src/lib/ipc.ts';

/**
 * Adding a report, and confirming what it is.
 *
 * One card at a time rather than a grid of rows. On a phone the whole job is:
 * take the photo, glance at the date it read, tap Save. Anything that needs two
 * hands or a horizontal scroll does not belong here.
 */
export function Capture({
  items,
  patients,
  categories,
  onChanged,
  onError,
  onNeedPeople,
}: {
  items: IngestItem[];
  patients: Patient[];
  categories: Category[];
  onChanged: () => void;
  onError: (e: string) => void;
  onNeedPeople: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  /**
   * Files that were refused on the way in.
   *
   * Held here rather than read back from the queue: the review queue only lists
   * what can still be reviewed, so a HEIC or a duplicate used to vanish without
   * ever saying why — the button simply appeared to do nothing.
   */
  const [skipped, setSkipped] = useState<IngestItem[]>([]);
  const busyGate = useBusy();

  const waiting = items.filter(
    (i) => i.status === 'needs_date' || i.status === 'pending' || i.status === 'locked',
  );
  const rejected = skipped.filter((s) => !items.some((i) => i.id === s.id));

  async function add() {
    setBusy(true);
    setNote(null);
    try {
      // Goes through the phone's own picker rather than the desktop dialog: that
      // one hands back a content:// URI, which the backend cannot open, so the
      // button appeared to work and imported nothing.
      const staged = await busyGate.run('Adding your file…', pickAndImport);
      const bad = staged.filter((i) => i.status === 'failed' || i.status === 'duplicate');
      if (bad.length > 0) {
        setSkipped((prev) => [...bad, ...prev.filter((p) => !bad.some((b) => b.id === p.id))]);
      }
      if (staged.length > bad.length) onChanged();
      else if (staged.length === 0) {
        onError('Nothing was added. If you chose a file, tell me — that is a bug.');
      }
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function dismiss(item: IngestItem) {
    setSkipped((prev) => prev.filter((s) => s.id !== item.id));
    try {
      await discardStaged(item.id);
    } catch {
      // Nothing to say. The row is already off the screen, and what is left
      // behind is a database row that lists nowhere.
    }
  }

  if (patients.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center px-8 text-center">
        <p className="text-5xl">☺</p>
        <h2 className="mt-4 text-base font-medium">Who are these reports for?</h2>
        <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
          Add yourself or a family member first. Every report is filed under a person.
        </p>
        <button
          type="button"
          onClick={onNeedPeople}
          className="mt-6 h-12 rounded-xl bg-sky-600 px-6 text-sm font-medium text-white active:bg-sky-700"
        >
          Add a person
        </button>
      </div>
    );
  }

  return (
    <div className="px-4 py-4">
      <button
        type="button"
        disabled={busy}
        onClick={() => void add()}
        className="flex h-28 w-full flex-col items-center justify-center gap-1 rounded-2xl border-2 border-dashed border-sky-300 bg-sky-50 text-sky-800 active:bg-sky-100 disabled:opacity-60 dark:border-sky-800 dark:bg-sky-950/40 dark:text-sky-200"
      >
        {busy ? (
          <>
            <Spinner />
            <span className="text-sm font-medium">Adding…</span>
            <span className="text-xs opacity-70">
              Straightening and shrinking the photo — a large one takes a moment
            </span>
          </>
        ) : (
          <>
            <span className="text-3xl leading-none">＋</span>
            <span className="text-sm font-medium">Add a photo or PDF</span>
            <span className="text-xs opacity-70">Straightened and de-duplicated automatically</span>
          </>
        )}
      </button>

      {note && (
        <button
          type="button"
          onClick={() => setNote(null)}
          className="mt-3 w-full rounded-xl bg-slate-100 px-3 py-2 text-left text-xs text-slate-600 dark:bg-slate-800 dark:text-slate-300"
        >
          {note} <span className="opacity-60">Tap to dismiss.</span>
        </button>
      )}

      {waiting.length > 0 && (
        <>
          <h2 className="mt-6 text-xs font-semibold uppercase tracking-wide text-slate-500">
            Waiting for you
          </h2>
          <div className="mt-2 space-y-3">
            {waiting.map((item) => (
              <ReviewCard
                key={item.id}
                item={item}
                patients={patients}
                categories={categories}
                onDone={onChanged}
                onRemoved={(fileName) => {
                  setNote(`Removed ${fileName}.`);
                  onChanged();
                }}
                onError={onError}
              />
            ))}
          </div>
        </>
      )}

      {rejected.length > 0 && (
        <>
          <h2 className="mt-6 text-xs font-semibold uppercase tracking-wide text-slate-500">
            Skipped
          </h2>
          <ul className="mt-2 space-y-2">
            {rejected.map((item) => (
              <li
                key={item.id}
                className="flex items-start gap-2 rounded-xl border border-slate-200 bg-white p-3 dark:border-slate-800 dark:bg-slate-900"
              >
                <div className="min-w-0 flex-1">
                  <p className="selectable truncate text-sm">{item.fileName}</p>
                  <p className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
                    {item.status === 'duplicate' ? 'Already added' : item.error}
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => void dismiss(item)}
                  aria-label={`Dismiss ${item.fileName}`}
                  className="-m-1 size-9 shrink-0 rounded-lg text-slate-400 active:bg-slate-100 dark:active:bg-slate-800"
                >
                  ✕
                </button>
              </li>
            ))}
          </ul>
        </>
      )}

      {waiting.length === 0 && rejected.length === 0 && (
        <p className="mt-8 text-center text-sm text-slate-500 dark:text-slate-400">
          Nothing waiting. Everything you have added is filed.
        </p>
      )}
    </div>
  );
}

/**
 * Something turning, so a slow import does not look like a dead button.
 *
 * Pure CSS rather than an image: it has to appear the instant the work starts,
 * and the work is what would delay loading anything.
 */
export function Spinner({ small = false }: { small?: boolean }) {
  return (
    <span
      role="progressbar"
      aria-label="Working"
      className={`inline-block animate-spin rounded-full border-2 border-current border-t-transparent ${
        small ? 'size-4' : 'size-7'
      }`}
    />
  );
}
