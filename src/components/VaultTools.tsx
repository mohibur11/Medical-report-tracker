import { useEffect, useState } from 'react';

import { rescanVault, searchDocuments, type ReconcileReport, type SearchHit } from '../lib/ipc.ts';
import { formatDmy } from '../lib/extract/dates.ts';

/**
 * Search across the library, and repair it after the user has been in Explorer.
 *
 * Both belong together: the vault is browsable on purpose, so files get moved and
 * dropped by hand, and the two things you then want are "find it" and "sort it out".
 */
export function VaultTools({ onRepaired }: { onRepaired: () => void }) {
  const [query, setQuery] = useState('');
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [report, setReport] = useState<ReconcileReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits(null);
      return;
    }
    // Debounced so a query runs per pause, not per keystroke.
    const timer = setTimeout(() => {
      searchDocuments(q).then(setHits, (e: unknown) => setError(String(e)));
    }, 150);
    return () => clearTimeout(timer);
  }, [query]);

  async function rescan() {
    setBusy(true);
    setError(null);
    try {
      setReport(await rescanVault());
      onRepaired();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="border-b border-slate-200 px-6 py-3 dark:border-slate-800">
      <div className="flex flex-wrap items-center gap-2">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search titles, notes and scanned text…"
          className="w-80 rounded border border-slate-300 bg-white px-2 py-1 text-sm outline-none focus:border-sky-500 dark:border-slate-600 dark:bg-slate-800"
        />
        <button
          type="button"
          onClick={() => void rescan()}
          disabled={busy}
          className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:hover:bg-slate-800"
          title="Check the vault folder for files moved, dropped or deleted outside the app"
        >
          {busy ? 'Rescanning…' : 'Rescan vault'}
        </button>
        {hits && (
          <span className="text-xs text-slate-500 dark:text-slate-400">
            {hits.length} match{hits.length === 1 ? '' : 'es'}
          </span>
        )}
      </div>

      {error && <p className="mt-2 text-xs text-red-600 dark:text-red-400">{error}</p>}

      {hits && hits.length > 0 && (
        <ul className="mt-2 space-y-1">
          {hits.map((h) => (
            <li key={h.documentId} className="text-xs">
              <span className="font-mono text-slate-500 dark:text-slate-400">
                {formatDmy(h.docDate)}
              </span>
              <span className="ml-2 font-medium">{h.title}</span>
              <span className="ml-2 text-slate-500 dark:text-slate-400">{h.patient}</span>
              {h.snippet && (
                <span className="ml-2 text-slate-500 dark:text-slate-400">{h.snippet}</span>
              )}
            </li>
          ))}
        </ul>
      )}

      {hits && hits.length === 0 && (
        <p className="mt-2 text-xs text-slate-500 dark:text-slate-400">No matches.</p>
      )}

      {report && (
        <div className="mt-2 rounded border border-slate-200 bg-slate-50 p-2 text-xs dark:border-slate-700 dark:bg-slate-900">
          <p className="text-slate-600 dark:text-slate-300">
            {report.ok} in place
            {report.relinked.length > 0 && `, ${report.relinked.length} found again`}
            {report.adopted.length > 0 && `, ${report.adopted.length} adopted`}
            {report.recovered.length > 0 && `, ${report.recovered.length} recovered`}
            {report.missing.length > 0 && `, ${report.missing.length} missing`}
            {report.unknown.length > 0 && `, ${report.unknown.length} not recognised`}
          </p>
          <Detail label="Found again" items={report.relinked} />
          <Detail label="Adopted from the folder" items={report.adopted} />
          <Detail label="Missing — flagged, not deleted" items={report.missing} tone="amber" />
          <Detail label="Not ours, left untouched" items={report.unknown} />
        </div>
      )}
    </div>
  );
}

function Detail({
  label,
  items,
  tone,
}: {
  label: string;
  items: string[];
  tone?: 'amber';
}) {
  if (items.length === 0) return null;
  return (
    <div className={`mt-1 ${tone === 'amber' ? 'text-amber-700 dark:text-amber-400' : 'text-slate-500 dark:text-slate-400'}`}>
      <span className="font-medium">{label}:</span>
      <ul className="ml-4 list-disc">
        {items.slice(0, 8).map((i) => (
          <li key={i} className="selectable">{i}</li>
        ))}
        {items.length > 8 && <li>…and {items.length - 8} more</li>}
      </ul>
    </div>
  );
}
