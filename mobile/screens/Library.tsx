import { useMemo, useState } from 'react';

import { formatDmy } from '../../src/lib/extract/dates.ts';
import {
  searchDocuments,
  trashDocument,
  type Category,
  type DocumentRow,
} from '../../src/lib/ipc.ts';

/**
 * Everything filed, newest first.
 *
 * A phone list rather than a table: one line for what it is, one for who and
 * when. Filtering is a row of chips because a doctor's question is almost always
 * "the thyroid ones" or "this year".
 */
export function Library({
  docs,
  categories,
  tags,
  onChanged,
  onError,
}: {
  docs: DocumentRow[];
  categories: Category[];
  tags: Record<string, string[]>;
  onChanged: () => void;
  onError: (e: string) => void;
}) {
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState('');
  const [hits, setHits] = useState<string[] | null>(null);

  const shown = useMemo(() => {
    let rows = docs;
    if (category) rows = rows.filter((d) => (tags[d.id] ?? []).includes(category));
    if (hits) rows = rows.filter((d) => hits.includes(d.id));
    return rows;
  }, [docs, tags, category, hits]);

  async function runSearch(text: string) {
    setQuery(text);
    if (!text.trim()) {
      setHits(null);
      return;
    }
    try {
      const found = await searchDocuments(text.trim());
      setHits(found.map((h) => h.documentId));
    } catch (e) {
      onError(String(e));
    }
  }

  return (
    <div className="px-4 py-4">
      <input
        className="h-12 w-full rounded-xl border border-slate-300 bg-white px-4 text-base outline-none focus:border-sky-500 dark:border-slate-700 dark:bg-slate-800"
        placeholder="Search reports and scanned text"
        value={query}
        onChange={(e) => void runSearch(e.target.value)}
        aria-label="Search"
      />

      {categories.length > 0 && (
        <div className="mt-3 flex gap-2 overflow-x-auto pb-1">
          <Chip active={category === ''} onClick={() => setCategory('')}>
            All
          </Chip>
          {categories.map((c) => (
            <Chip key={c.id} active={category === c.id} onClick={() => setCategory(c.id)}>
              {c.name}
            </Chip>
          ))}
        </div>
      )}

      {shown.length === 0 ? (
        <p className="mt-10 text-center text-sm text-slate-500 dark:text-slate-400">
          {docs.length === 0 ? 'Nothing filed yet.' : 'Nothing matches.'}
        </p>
      ) : (
        <ul className="mt-3 space-y-2">
          {shown.map((d) => (
            <li
              key={d.id}
              className="rounded-2xl border border-slate-200 bg-white p-3 dark:border-slate-800 dark:bg-slate-900"
            >
              <div className="flex items-start gap-3">
                <span className="mt-0.5 flex size-10 shrink-0 items-center justify-center rounded-lg bg-slate-100 text-[10px] font-semibold uppercase text-slate-500 dark:bg-slate-800">
                  {d.fileKind === 'pdf' ? 'PDF' : 'IMG'}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-base">{d.title}</span>
                  <span className="block text-xs text-slate-500 dark:text-slate-400">
                    {formatDmy(d.docDate)} · {d.patient}
                    {d.pageCount > 1 && ` · ${d.pageCount} pages`}
                  </span>
                  {d.notes && (
                    <span className="mt-0.5 block truncate text-xs italic text-slate-500 dark:text-slate-400">
                      {d.notes}
                    </span>
                  )}
                </span>
                <button
                  type="button"
                  aria-label={`Delete ${d.title}`}
                  onClick={() => {
                    if (
                      !window.confirm(
                        `Move '${d.title}' to Trash?\n\nThe file is not deleted — it moves to the Trash folder in your vault.`,
                      )
                    )
                      return;
                    trashDocument(d.id).then(onChanged, (e: unknown) => onError(String(e)));
                  }}
                  className="size-10 shrink-0 rounded-lg text-lg text-slate-400 active:bg-red-50 active:text-red-600"
                >
                  🗑
                </button>
              </div>
              {d.missing && (
                <p className="mt-2 rounded-lg bg-amber-50 px-2 py-1 text-xs text-amber-800 dark:bg-amber-950 dark:text-amber-200">
                  The file is missing from the vault.
                </p>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function Chip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`h-9 shrink-0 rounded-full border px-4 text-sm ${
        active
          ? 'border-sky-500 bg-sky-50 font-medium text-sky-800 dark:bg-sky-950 dark:text-sky-200'
          : 'border-slate-300 text-slate-600 dark:border-slate-700 dark:text-slate-300'
      }`}
    >
      {children}
    </button>
  );
}
