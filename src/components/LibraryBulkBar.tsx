import { useState } from 'react';

import type { Category } from '../lib/ipc.ts';

/**
 * Batch actions for documents that are already filed.
 *
 * Tagging is the one thing a backlog needs done in bulk after the fact: sixty
 * thyroid reports filed over ten years all want the same category, and doing that
 * one popover at a time is what stops people tagging at all. Removing a tag is
 * here for the same reason — a bulk tag applied to the wrong selection needs an
 * undo that is not sixty more clicks.
 */
export function LibraryBulkBar({
  selectedCount,
  allChecked,
  onToggleAll,
  categories,
  onTag,
  onUntag,
  onDelete,
  busy,
  status,
}: {
  selectedCount: number;
  allChecked: boolean;
  onToggleAll: () => void;
  categories: Category[];
  onTag: (categoryId: string) => void;
  onUntag: (categoryId: string) => void;
  onDelete: () => void;
  busy: boolean;
  status: string | null;
}) {
  const [categoryId, setCategoryId] = useState('');
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
            aria-label="Select all documents"
            className="size-4 accent-sky-600"
          />
          {none ? 'Select all' : `${selectedCount} selected`}
        </label>

        <span className="text-slate-300 dark:text-slate-700">|</span>

        <select
          className={control}
          value={categoryId}
          disabled={none || categories.length === 0}
          onChange={(e) => setCategoryId(e.target.value)}
          aria-label="Category to apply"
        >
          <option value="">Choose a category…</option>
          {categories.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>

        <button
          type="button"
          disabled={none || !categoryId || busy}
          onClick={() => onTag(categoryId)}
          className="rounded bg-sky-600 px-2.5 py-1 text-xs font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
        >
          Add tag
        </button>
        <button
          type="button"
          disabled={none || !categoryId || busy}
          onClick={() => onUntag(categoryId)}
          className={control}
        >
          Remove tag
        </button>

        <span className="text-slate-300 dark:text-slate-700">|</span>

        <button
          type="button"
          disabled={none || busy}
          onClick={onDelete}
          className="rounded border border-slate-300 px-2 py-1 text-xs text-red-600 hover:bg-red-50 disabled:opacity-40 dark:border-slate-600 dark:hover:bg-red-950"
        >
          Delete selected
        </button>

        {status && (
          <span className="selectable text-xs text-slate-600 dark:text-slate-300">{status}</span>
        )}
      </div>
    </div>
  );
}
