import { useState } from 'react';

import { archiveCategory, createCategory, renameCategory, type Category } from '../lib/ipc.ts';

/**
 * Tag editor for one document.
 *
 * Categories are the axis the folder tree cannot express, so this is the only
 * place a document gains one. Kept inline in the library row rather than behind a
 * dialog — tagging a backlog means touching many rows, and a dialog per row is
 * the same trap as asking "which patient?" 200 times.
 */
export function CategoryPicker({
  categories,
  selected,
  onChange,
}: {
  categories: Category[];
  selected: string[];
  onChange: (ids: string[]) => void;
}) {
  const [open, setOpen] = useState(false);

  const toggle = (id: string) =>
    onChange(selected.includes(id) ? selected.filter((s) => s !== id) : [...selected, id]);

  const chosen = categories.filter((c) => selected.includes(c.id));

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex max-w-full flex-wrap items-center gap-1 rounded border border-dashed border-slate-300 px-1.5 py-0.5 text-left text-[11px] text-slate-500 hover:border-slate-400 dark:border-slate-600 dark:text-slate-400"
      >
        {chosen.length === 0 ? (
          <span>+ tag</span>
        ) : (
          chosen.map((c) => (
            <span
              key={c.id}
              className="shrink-0 truncate rounded px-1.5 py-0.5 text-[11px] font-medium"
              style={{
                background: c.color ?? '#e2e8f0',
                color: c.color ? '#fff' : '#334155',
              }}
            >
              {c.name}
            </span>
          ))
        )}
      </button>

      {open && (
        <>
          {/* Click-away layer, so the popover closes without a global listener. */}
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} />
          <div className="absolute left-0 z-20 mt-1 w-52 rounded border border-slate-200 bg-white p-1.5 shadow-lg dark:border-slate-700 dark:bg-slate-900">
            {categories.length === 0 ? (
              <p className="px-1 py-2 text-xs text-slate-500">
                No categories yet. Add one from the Library header.
              </p>
            ) : (
              categories.map((c) => (
                <label
                  key={c.id}
                  className="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 text-xs hover:bg-slate-50 dark:hover:bg-slate-800"
                >
                  <input
                    type="checkbox"
                    checked={selected.includes(c.id)}
                    onChange={() => toggle(c.id)}
                  />
                  <span className="flex-1 truncate">{c.name}</span>
                  <span className="text-slate-400">{c.documentCount}</span>
                </label>
              ))
            )}
          </div>
        </>
      )}
    </div>
  );
}

/** Create, rename and archive — the management surface requirement 4 asks for. */
export function CategoryManager({
  categories,
  onChanged,
}: {
  categories: Category[];
  onChanged: () => void;
  }) {
  const [error, setError] = useState<string | null>(null);

  async function guard(fn: () => Promise<unknown>) {
    setError(null);
    try {
      await fn();
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <span className="w-16 shrink-0 text-xs text-slate-500 dark:text-slate-400">Categories</span>

      {categories.map((c) => (
        <span
          key={c.id}
          className="group flex items-center gap-1 rounded border border-slate-300 px-1.5 py-0.5 text-xs dark:border-slate-600"
        >
          <span>{c.name}</span>
          <span className="text-slate-400">{c.documentCount}</span>
          <button
            type="button"
            title="Rename"
            onClick={() => {
              const name = window.prompt('Rename category', c.name);
              if (name && name !== c.name) void guard(() => renameCategory(c.id, name));
            }}
            className="text-slate-400 hover:text-slate-700 dark:hover:text-slate-200"
          >
            ✎
          </button>
          <button
            type="button"
            title="Archive (keeps existing tags)"
            onClick={() => {
              if (window.confirm(`Archive '${c.name}'? Documents keep the tag, but it stops being offered.`))
                void guard(() => archiveCategory(c.id));
            }}
            className="text-slate-400 hover:text-red-600"
          >
            ×
          </button>
        </span>
      ))}

      <button
        type="button"
        onClick={() => {
          const name = window.prompt('New category, e.g. Thyroid');
          if (name) void guard(() => createCategory(name));
        }}
        className="rounded border border-dashed border-slate-300 px-1.5 py-0.5 text-xs text-slate-500 hover:border-slate-400 dark:border-slate-600"
      >
        + Category
      </button>

      {error && <span className="text-xs text-red-600">{error}</span>}
    </div>
  );
}
