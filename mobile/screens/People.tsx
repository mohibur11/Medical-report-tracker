import { useState } from 'react';

import { formatDmy, parseDmyInput } from '../../src/lib/extract/dates.ts';
import { createPatient, renamePatient, type Patient } from '../../src/lib/ipc.ts';

/**
 * The people reports belong to.
 *
 * Date of birth carries weight rather than being decoration: it is what stops a
 * birth date printed on a report being read as the report's own date, so the
 * screen asks for it and says why.
 */
export function People({
  patients,
  onChanged,
  onError,
}: {
  patients: Patient[];
  onChanged: () => void;
  onError: (e: string) => void;
}) {
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [dob, setDob] = useState('');
  const [busy, setBusy] = useState(false);

  const field =
    'h-12 w-full rounded-xl border border-slate-300 bg-white px-3 text-base outline-none ' +
    'focus:border-sky-500 dark:border-slate-700 dark:bg-slate-800';

  async function save(existing?: Patient) {
    const iso = dob.trim() ? parseDmyInput(dob) : null;
    if (dob.trim() && !iso) {
      onError('Date of birth should look like 14/12/1995.');
      return;
    }
    setBusy(true);
    try {
      if (existing) await renamePatient(existing.id, name.trim(), iso);
      else await createPatient(name.trim(), iso ?? undefined);
      setName('');
      setDob('');
      setAdding(false);
      setEditing(null);
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="px-4 py-4">
      {patients.map((p) =>
        editing === p.id ? (
          <div key={p.id} className="mb-3 space-y-2 rounded-2xl border border-sky-300 bg-white p-3 dark:border-sky-800 dark:bg-slate-900">
            <input className={field} value={name} onChange={(e) => setName(e.target.value)} aria-label="Name" />
            <input
              className={`${field} font-mono`}
              inputMode="numeric"
              placeholder="dd/mm/yyyy"
              value={dob}
              onChange={(e) => setDob(e.target.value)}
              aria-label="Date of birth"
            />
            {p.documentCount > 0 && name.trim() !== p.displayName && (
              <p className="text-xs text-amber-600">
                Renaming moves {p.documentCount} file{p.documentCount === 1 ? '' : 's'}.
              </p>
            )}
            <div className="flex gap-2">
              <button
                type="button"
                disabled={busy || !name.trim()}
                onClick={() => void save(p)}
                className="h-12 flex-1 rounded-xl bg-sky-600 text-sm font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
              >
                {busy ? 'Saving…' : 'Save'}
              </button>
              <button
                type="button"
                onClick={() => setEditing(null)}
                className="h-12 rounded-xl border border-slate-300 px-4 text-sm dark:border-slate-700"
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <button
            key={p.id}
            type="button"
            onClick={() => {
              setEditing(p.id);
              setName(p.displayName);
              setDob(p.dob ? formatDmy(p.dob) : '');
            }}
            className="mb-3 flex w-full items-center gap-3 rounded-2xl border border-slate-200 bg-white p-4 text-left active:bg-slate-50 dark:border-slate-800 dark:bg-slate-900"
          >
            <span className="flex size-11 shrink-0 items-center justify-center rounded-full bg-sky-100 text-lg font-semibold text-sky-800 dark:bg-sky-950 dark:text-sky-200">
              {p.displayName.trim().charAt(0).toUpperCase()}
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-base">{p.displayName}</span>
              <span className="block text-xs text-slate-500 dark:text-slate-400">
                {p.dob ? `Born ${formatDmy(p.dob)}` : 'No date of birth'} · {p.documentCount} report
                {p.documentCount === 1 ? '' : 's'}
              </span>
            </span>
            {!p.dob && <span className="shrink-0 text-xs text-amber-600">add DOB</span>}
          </button>
        ),
      )}

      {adding ? (
        <div className="space-y-2 rounded-2xl border border-sky-300 bg-white p-3 dark:border-sky-800 dark:bg-slate-900">
          <input
            className={field}
            placeholder="Full name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            aria-label="Name"
            autoFocus
          />
          <input
            className={`${field} font-mono`}
            inputMode="numeric"
            placeholder="Date of birth — dd/mm/yyyy"
            value={dob}
            onChange={(e) => setDob(e.target.value)}
            aria-label="Date of birth"
          />
          <p className="text-xs text-slate-500 dark:text-slate-400">
            The date of birth is what stops a birth date printed on a report being mistaken for the
            report's own date.
          </p>
          <div className="flex gap-2">
            <button
              type="button"
              disabled={busy || !name.trim()}
              onClick={() => void save()}
              className="h-12 flex-1 rounded-xl bg-sky-600 text-sm font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
            >
              {busy ? 'Adding…' : 'Add'}
            </button>
            <button
              type="button"
              onClick={() => setAdding(false)}
              className="h-12 rounded-xl border border-slate-300 px-4 text-sm dark:border-slate-700"
            >
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => {
            setAdding(true);
            setName('');
            setDob('');
          }}
          className="h-14 w-full rounded-2xl border-2 border-dashed border-slate-300 text-sm font-medium text-slate-600 active:bg-slate-100 dark:border-slate-700 dark:text-slate-300"
        >
          ＋ Add a person
        </button>
      )}
    </div>
  );
}
