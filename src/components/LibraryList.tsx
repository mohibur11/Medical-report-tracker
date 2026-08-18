import { useRef, type RefObject } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';

import { CategoryPicker } from './CategoryPicker.tsx';
import { EditRow } from './EditRow.tsx';
import { formatDmy } from '../lib/extract/dates.ts';
import * as selection from '../lib/selection.ts';
import type { Category, DocumentRow, Patient } from '../lib/ipc.ts';

/**
 * The filed library.
 *
 * Virtualized because the archive this app is for is 200-1000 documents and
 * growing: rendering every row costs nothing at ten and makes scrolling
 * noticeably heavy by several hundred. Only what is on screen is in the DOM.
 *
 * The rows scroll inside the page rather than a box of their own — the export
 * panel and category editor sit above them and should scroll away — so the
 * virtualizer measures against the page's scroll container and is told how far
 * down the list starts.
 */
export function LibraryList({
  docs,
  patients,
  categories,
  tags,
  sel,
  onSel,
  editingId,
  onEdit,
  onSaved,
  onRetag,
  onTrash,
  scrollRef,
}: {
  docs: DocumentRow[];
  patients: Patient[];
  categories: Category[];
  tags: Record<string, string[]>;
  sel: selection.Selection;
  onSel: (next: selection.Selection) => void;
  editingId: string | null;
  onEdit: (id: string | null) => void;
  onSaved: () => void;
  onRetag: (documentId: string, categoryIds: string[]) => void;
  onTrash: (doc: DocumentRow) => void;
  scrollRef: RefObject<HTMLElement | null>;
}) {
  const listRef = useRef<HTMLDivElement | null>(null);
  const ids = docs.map((d) => d.id);

  const virtualizer = useVirtualizer({
    count: docs.length,
    getScrollElement: () => scrollRef.current,
    // A two-line row at rest. Rows that wrap, carry a note, or open for editing
    // are measured for real once rendered.
    estimateSize: () => 58,
    // Everything above the list is part of the same scroller, so without this the
    // rows would be positioned as if the list started at the top of the page.
    scrollMargin: listRef.current?.offsetTop ?? 0,
    overscan: 8,
    getItemKey: (index) => docs[index]?.id ?? index,
  });

  return (
    <div ref={listRef} className="relative" style={{ height: virtualizer.getTotalSize() }}>
      {virtualizer.getVirtualItems().map((row) => {
        const d = docs[row.index];
        if (!d) return null;
        const selected = sel.ids.has(d.id);

        return (
          <div
            key={row.key}
            data-index={row.index}
            ref={virtualizer.measureElement}
            className="absolute left-0 w-full border-b border-slate-100 dark:border-slate-800"
            style={{ transform: `translateY(${row.start - virtualizer.options.scrollMargin}px)` }}
          >
            {editingId === d.id ? (
              <ul>
                <EditRow
                  doc={d}
                  patients={patients}
                  onSaved={onSaved}
                  onCancel={() => onEdit(null)}
                />
              </ul>
            ) : (
              <div className={`px-6 py-2 ${selected ? 'bg-sky-50 dark:bg-sky-950/40' : ''}`}>
                <div className="flex items-baseline gap-3">
                  <input
                    type="checkbox"
                    checked={selected}
                    onChange={() => {}}
                    onClick={(e) => onSel(selection.click(sel, ids, d.id, e.shiftKey))}
                    aria-label={`Select ${d.title}`}
                    className="size-4 shrink-0 self-center accent-sky-600"
                  />
                  <span className="w-24 shrink-0 font-mono text-xs text-slate-500 dark:text-slate-400">
                    {formatDmy(d.docDate)}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-sm">{d.title}</span>
                </div>

                {/* Patient, type and tags share the second line. Several categories
                    per document is normal, and none of it should squeeze the title. */}
                <div className="mt-1 flex flex-wrap items-center gap-2 pl-32 text-xs text-slate-500 dark:text-slate-400">
                  <span>{d.patient}</span>
                  <span className="text-slate-400">
                    {d.fileKind.toUpperCase()}
                    {d.pageCount > 1 && ` · ${d.pageCount}p`}
                  </span>
                  <CategoryPicker
                    categories={categories}
                    selected={tags[d.id] ?? []}
                    onChange={(ids) => onRetag(d.id, ids)}
                  />
                  {d.notes && (
                    <span
                      className="max-w-72 truncate italic text-slate-500 dark:text-slate-400"
                      title={d.notes}
                    >
                      {d.notes}
                    </span>
                  )}
                  {d.missing && (
                    <span className="rounded bg-amber-100 px-1.5 py-0.5 text-[11px] font-medium text-amber-800 dark:bg-amber-900 dark:text-amber-200">
                      file missing
                    </span>
                  )}
                  <button
                    type="button"
                    title="Change the date, title, patient, type or notes"
                    onClick={() => onEdit(d.id)}
                    className="rounded border border-slate-300 px-1.5 py-0.5 hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-800"
                  >
                    Edit
                  </button>
                  <button
                    type="button"
                    title="Move to the vault's Trash folder — the file is not deleted"
                    onClick={() => onTrash(d)}
                    className="rounded border border-slate-300 px-1.5 py-0.5 text-red-600 hover:bg-red-50 dark:border-slate-600 dark:hover:bg-red-950"
                  >
                    Delete
                  </button>
                </div>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
