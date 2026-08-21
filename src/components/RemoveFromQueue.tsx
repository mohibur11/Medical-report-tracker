import { useState } from 'react';

import { discardStaged, type IngestItem } from '../lib/ipc.ts';

/**
 * Take a file back out of the review queue.
 *
 * Adding the wrong photo is an ordinary mistake, and until this existed the only
 * way out of it was to file the thing and then trash it — which put a document
 * nobody wanted into the vault on the way past.
 *
 * It asks twice. The button sits beside File, and a queue that loses a row to a
 * mis-click would be worse than one that cannot be cleared at all. What goes is
 * the app's own staged copy; the file it was read from is left where it was.
 */
export function RemoveFromQueue({
  item,
  onRemoved,
}: {
  item: IngestItem;
  /** Reports the name it went under, so the queue can say what disappeared. */
  onRemoved: (id: string, fileName: string) => void;
}) {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function go() {
    setBusy(true);
    setError(null);
    try {
      onRemoved(item.id, await discardStaged(item.id));
    } catch (e) {
      setError(String(e));
      setBusy(false);
      setAsking(false);
    }
  }

  if (error) {
    return <span className="text-xs text-red-600 dark:text-red-400">{error}</span>;
  }

  if (!asking) {
    return (
      <button
        type="button"
        onClick={() => setAsking(true)}
        title={`Remove ${item.fileName} from the queue`}
        aria-label={`Remove ${item.fileName} from the queue`}
        className="rounded px-2 py-1 text-sm text-slate-400 hover:bg-red-50 hover:text-red-600 dark:hover:bg-red-950/50 dark:hover:text-red-400"
      >
        ✕
      </button>
    );
  }

  return (
    <span className="flex items-center gap-1.5 rounded bg-red-50 px-2 py-1 dark:bg-red-950/50">
      <span className="text-xs text-red-800 dark:text-red-200">Remove?</span>
      <button
        type="button"
        disabled={busy}
        onClick={() => void go()}
        className="rounded bg-red-600 px-2 py-0.5 text-xs font-medium text-white disabled:opacity-60"
      >
        Yes
      </button>
      <button
        type="button"
        onClick={() => setAsking(false)}
        className="rounded px-1.5 py-0.5 text-xs text-slate-600 dark:text-slate-300"
      >
        Keep
      </button>
    </span>
  );
}
