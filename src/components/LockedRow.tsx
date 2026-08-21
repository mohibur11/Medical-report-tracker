import { useState } from 'react';

import { RemoveFromQueue } from './RemoveFromQueue.tsx';
import { Thumb } from './Thumb.tsx';
import { unlockPdf, type IngestItem } from '../lib/ipc.ts';

/**
 * A password-protected PDF waiting for its password.
 *
 * Kept in the queue rather than rejected: the file is perfectly good, it simply
 * cannot be read yet. Labs and hospitals routinely email reports locked with a
 * date of birth or a phone number, and silently dropping those from an export is
 * exactly the kind of quiet loss this app exists to prevent.
 */
export function LockedRow({
  item,
  onUnlocked,
  onRemoved,
}: {
  item: IngestItem;
  onUnlocked: () => void;
  /** A PDF whose password nobody has is exactly what somebody wants to drop. */
  onRemoved: (id: string, fileName: string) => void;
}) {
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function unlock() {
    if (!password) return;
    setBusy(true);
    setError(null);
    try {
      await unlockPdf(item.id, password);
      setPassword('');
      onUnlocked();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <li className="flex items-start gap-4 px-6 py-3">
      <Thumb id={item.id} kind={item.fileKind} />

      <div className="min-w-0 flex-1">
        <p className="selectable truncate text-xs text-slate-500 dark:text-slate-400">
          {item.fileName}
        </p>

        <p className="mt-1 text-sm text-amber-800 dark:text-amber-200">
          Password protected — nothing can read it until it is unlocked, including the export.
        </p>

        <div className="mt-2 flex flex-wrap items-center gap-2">
          <input
            type="password"
            className="rounded border border-slate-300 bg-white px-2 py-1 text-sm outline-none focus:border-sky-500 dark:border-slate-600 dark:bg-slate-800"
            placeholder="Password for this PDF"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && void unlock()}
            aria-label="PDF password"
          />
          <button
            type="button"
            disabled={!password || busy}
            onClick={() => void unlock()}
            className="rounded bg-sky-600 px-3 py-1 text-sm font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
          >
            {busy ? 'Unlocking…' : 'Unlock'}
          </button>
          <span className="text-xs text-slate-500 dark:text-slate-400">
            Often a date of birth or a phone number.
          </span>
          <RemoveFromQueue item={item} onRemoved={onRemoved} />
        </div>

        {error && <p className="mt-1 text-xs text-red-600 dark:text-red-400">{error}</p>}
      </div>
    </li>
  );
}
