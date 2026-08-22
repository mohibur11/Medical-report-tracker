import { useEffect, useState } from 'react';

import { useBusy } from '../busy.tsx';
import { formatDmy } from '../../src/lib/extract/dates.ts';
import { listTrashed, restoreDocument, type TrashedDocument } from '../../src/lib/ipc.ts';

/**
 * What was deleted, and the way back.
 *
 * Delete on a phone is one tap behind one confirm, held in one hand, often while
 * somebody is talking. Without this the only way back was the computer, and only
 * if the phone had been backed up since.
 *
 * Nothing here has been erased: deleting moves the file into the vault's Trash
 * folder and keeps everything known about it.
 */
export function Trash({
  reload,
  onRestored,
  onError,
}: {
  /** Changes when the library changes, so the list re-reads itself. */
  reload: unknown;
  onRestored: () => void;
  onError: (e: string) => void;
}) {
  const busyGate = useBusy();
  const [items, setItems] = useState<TrashedDocument[]>([]);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    listTrashed().then(setItems, () => {});
  }, [reload]);

  if (items.length === 0) return null;

  async function restore(item: TrashedDocument) {
    try {
      await busyGate.run('Putting it back…', () => restoreDocument(item.id));
      onRestored();
    } catch (e) {
      onError(String(e));
    }
  }

  return (
    <div className="mt-3 overflow-hidden rounded-2xl border border-slate-200 dark:border-slate-800">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex h-12 w-full items-center justify-between px-4 text-sm text-slate-600 dark:text-slate-300"
      >
        <span>
          Trash <span className="text-slate-400">({items.length})</span>
        </span>
        <span aria-hidden>{open ? '▴' : '▾'}</span>
      </button>

      {open && (
        <ul className="space-y-2 border-t border-slate-200 p-3 dark:border-slate-800">
          {items.map((item) => (
            <li key={item.id} className="flex items-center gap-3">
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm">{item.title}</span>
                <span className="block text-xs text-slate-500 dark:text-slate-400">
                  {formatDmy(item.docDate)} · {item.patient}
                </span>
              </span>
              {item.recoverable ? (
                <button
                  type="button"
                  onClick={() => void restore(item)}
                  className="h-10 shrink-0 rounded-lg border border-slate-300 px-3 text-sm dark:border-slate-700"
                >
                  Put back
                </button>
              ) : (
                <span className="shrink-0 text-xs text-amber-600">file not in Trash</span>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
