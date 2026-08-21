import { useState } from 'react';

import { formatDmy } from '../lib/extract/dates.ts';
import { restoreDocument, type TrashedDocument } from '../lib/ipc.ts';

/**
 * What has been deleted, and the way back.
 *
 * Delete never unlinks anything — it moves the file into the vault's Trash folder
 * and keeps the row. Without this panel the only way back was Explorer plus a
 * rescan, which is not a recovery path anybody finds when they need it.
 *
 * The list is handed in rather than fetched here. Fetching it on mount meant the
 * panel only reloaded when it was mounted afresh: deleting a document made it
 * vanish from the library and appear nowhere, until you left the view and came
 * back. One place now decides what is in the trash and what is in the library,
 * and both change together.
 */
export function TrashPanel({
  items,
  onChanged,
}: {
  items: TrashedDocument[];
  /** Something moved between the trash and the library; reload both. */
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (items.length === 0) return null;

  async function restore(item: TrashedDocument) {
    setBusy(item.id);
    setError(null);
    try {
      await restoreDocument(item.id);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="border-b border-slate-200 px-6 py-3 dark:border-slate-800">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-slate-500">
        Trash ({items.length})
      </h2>

      <ul className="mt-1.5 space-y-1">
        {items.map((item) => (
          <li key={item.id} className="flex flex-wrap items-center gap-2 text-sm">
            <span className="min-w-0 truncate">{item.title}</span>
            <span className="text-xs text-slate-500 dark:text-slate-400">
              {formatDmy(item.docDate)} · {item.patient} · deleted {item.trashedAt} UTC
            </span>
            {item.recoverable ? (
              <button
                type="button"
                disabled={busy !== null}
                onClick={() => void restore(item)}
                className="rounded border border-slate-300 px-1.5 py-0.5 text-xs hover:bg-slate-50 disabled:opacity-40 dark:border-slate-600 dark:hover:bg-slate-800"
              >
                {busy === item.id ? 'Putting back…' : 'Put back'}
              </button>
            ) : (
              <span
                className="text-xs text-amber-600"
                title="The file is no longer in the Trash folder — use Rescan vault if you moved it"
              >
                file not in Trash
              </span>
            )}
          </li>
        ))}
      </ul>

      <p className="mt-1.5 text-[11px] text-slate-400 dark:text-slate-500">
        Deleting moves a file into the vault's Trash folder and keeps its details. Nothing here has
        been erased.
      </p>

      {error && <p className="mt-1.5 text-xs text-red-600 dark:text-red-400">{error}</p>}
    </div>
  );
}
