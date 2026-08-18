import { useState } from 'react';

import { CategoryPicker } from './CategoryPicker.tsx';
import type { Category, Patient } from '../lib/ipc.ts';

/**
 * Batch actions for the review queue.
 *
 * A backlog is 200-1000 files, and almost always arrives in runs: one patient's
 * folder, one visit's papers, one lab's reports. Setting patient and type per row
 * is the same trap as asking "which patient?" 200 times, so the whole point of
 * this bar is that a run costs one click instead of two hundred.
 *
 * Date is missing from it on purpose — see RowApi.
 */
export function BulkBar({
  selectedCount,
  allChecked,
  onToggleAll,
  patients,
  categories,
  onApplyPatient,
  onApplyType,
  onFile,
  busy,
  status,
}: {
  selectedCount: number;
  allChecked: boolean;
  onToggleAll: () => void;
  patients: Patient[];
  categories: Category[];
  onApplyPatient: (id: string) => void;
  onApplyType: (t: string) => void;
  /** Tags are applied after filing — an ingest item has no document to tag yet. */
  onFile: (categoryIds: string[]) => void;
  busy: boolean;
  status: string | null;
}) {
  const [categoryIds, setCategoryIds] = useState<string[]>([]);
  const none = selectedCount === 0;

  const control =
    'rounded border border-slate-300 bg-white px-2 py-1 text-xs outline-none ' +
    'disabled:opacity-40 dark:border-slate-600 dark:bg-slate-800';

  return (
    <div className="sticky top-0 z-10 border-b border-slate-200 bg-slate-50/95 px-6 py-2 backdrop-blur dark:border-slate-800 dark:bg-slate-900/95">
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-2 text-xs text-slate-600 dark:text-slate-300">
          <input
            type="checkbox"
            checked={allChecked}
            onChange={onToggleAll}
            aria-label="Select all"
            className="size-4 accent-sky-600"
          />
          {none ? 'Select all' : `${selectedCount} selected`}
        </label>

        <span className="text-slate-300 dark:text-slate-700">|</span>

        <select
          className={control}
          value=""
          disabled={none}
          onChange={(e) => e.target.value && onApplyPatient(e.target.value)}
          aria-label="Set patient for selected"
        >
          <option value="">Set patient…</option>
          {patients.map((p) => (
            <option key={p.id} value={p.id}>
              {p.displayName}
            </option>
          ))}
        </select>

        <select
          className={control}
          value=""
          disabled={none}
          onChange={(e) => e.target.value && onApplyType(e.target.value)}
          aria-label="Set type for selected"
        >
          <option value="">Set type…</option>
          <option value="report">Report</option>
          <option value="prescription">Prescription</option>
          <option value="invoice">Invoice</option>
          <option value="other">Other</option>
        </select>

        <span className={none ? 'pointer-events-none opacity-40' : ''}>
          <CategoryPicker categories={categories} selected={categoryIds} onChange={setCategoryIds} />
        </span>

        <button
          type="button"
          disabled={none || busy}
          onClick={() => onFile(categoryIds)}
          className="rounded bg-sky-600 px-3 py-1 text-xs font-medium text-white hover:bg-sky-700 disabled:cursor-not-allowed disabled:bg-slate-300 dark:disabled:bg-slate-700"
        >
          {busy ? 'Filing…' : `File ${selectedCount || ''} selected`.replace('  ', ' ')}
        </button>

        {status && (
          <span className="selectable text-xs text-slate-600 dark:text-slate-300">{status}</span>
        )}
      </div>

      {categoryIds.length > 0 && !none && (
        <p className="mt-1 text-[11px] text-slate-500 dark:text-slate-400">
          Tags are applied once each file lands in the vault.
        </p>
      )}
    </div>
  );
}
