import { useState } from 'react';

import { formatDmy, parseDmyInput } from '../lib/extract/dates.ts';
import { renamePatient, type Patient, type RenameReport } from '../lib/ipc.ts';

/**
 * Patient details, and the one place a name can be corrected.
 *
 * Renaming is not a text edit: the name is the folder and part of every filename,
 * so it moves every document the patient owns. The screen says so before it
 * happens and reports what actually moved afterwards, because the operation can
 * half-succeed — a file open in a viewer, or held by a sync client, cannot be
 * moved out from under it.
 */
export function PatientsPanel({
  patients,
  onChanged,
}: {
  patients: Patient[];
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [dob, setDob] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<RenameReport | null>(null);

  const begin = (p: Patient) => {
    setEditing(p.id);
    setName(p.displayName);
    setDob(p.dob ? formatDmy(p.dob) : '');
    setError(null);
    setReport(null);
  };

  const input =
    'rounded border border-slate-300 bg-white px-2 py-1 text-sm outline-none ' +
    'focus:border-sky-500 dark:border-slate-600 dark:bg-slate-800';

  async function save(p: Patient) {
    const iso = dob.trim() ? parseDmyInput(dob) : null;
    if (dob.trim() && !iso) {
      setError('Date of birth must be dd/mm/yyyy.');
      return;
    }

    const renaming = name.trim() !== p.displayName;
    if (renaming && p.documentCount > 0) {
      const ok = window.confirm(
        `Rename ${p.displayName} to ${name.trim()}?\n\n` +
          `This moves ${p.documentCount} file${p.documentCount === 1 ? '' : 's'} — the name is ` +
          `part of the folder and of every filename. Close anything you have open from ` +
          `this patient's folder first.`,
      );
      if (!ok) return;
    }

    setBusy(true);
    setError(null);
    try {
      const result = await renamePatient(p.id, name.trim(), iso);
      setReport(result);
      setEditing(null);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="border-b border-slate-200 px-6 py-3 dark:border-slate-800">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-slate-500">Patients</h2>

      <ul className="mt-1.5 space-y-1">
        {patients.map((p) =>
          editing === p.id ? (
            <li key={p.id} className="flex flex-wrap items-center gap-2">
              <input
                className={`${input} min-w-52`}
                value={name}
                onChange={(e) => setName(e.target.value)}
                aria-label="Patient name"
                autoFocus
              />
              <input
                className={`${input} w-28 font-mono`}
                value={dob}
                onChange={(e) => setDob(e.target.value)}
                placeholder="dd/mm/yyyy"
                aria-label="Date of birth"
              />
              <button
                type="button"
                disabled={busy || !name.trim()}
                onClick={() => void save(p)}
                className="rounded bg-sky-600 px-3 py-1 text-xs font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
              >
                {busy ? 'Moving files…' : 'Save'}
              </button>
              <button
                type="button"
                onClick={() => setEditing(null)}
                className="rounded border border-slate-300 px-2 py-1 text-xs dark:border-slate-600"
              >
                Cancel
              </button>
              {p.documentCount > 0 && name.trim() !== p.displayName && (
                <span className="text-xs text-amber-600">
                  moves {p.documentCount} file{p.documentCount === 1 ? '' : 's'}
                </span>
              )}
            </li>
          ) : (
            <li key={p.id} className="flex flex-wrap items-center gap-2 text-sm">
              <span className="min-w-40">{p.displayName}</span>
              <span className="text-xs text-slate-500 dark:text-slate-400">
                {p.dob ? `born ${formatDmy(p.dob)}` : 'no date of birth'}
                {' · '}
                {p.documentCount} file{p.documentCount === 1 ? '' : 's'}
              </span>
              {!p.dob && (
                <span
                  className="text-xs text-amber-600"
                  title="Without it, nothing stops a birth date on a report being filed as the report date"
                >
                  add one
                </span>
              )}
              <button
                type="button"
                onClick={() => begin(p)}
                className="rounded border border-slate-300 px-1.5 py-0.5 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-800"
              >
                Edit
              </button>
            </li>
          ),
        )}
      </ul>

      {error && <p className="mt-1.5 text-xs text-red-600 dark:text-red-400">{error}</p>}

      {report && (
        <div className="mt-1.5 text-xs">
          <p className="text-slate-600 dark:text-slate-300">
            {report.moved > 0
              ? `Moved ${report.moved} file${report.moved === 1 ? '' : 's'} to ${report.folderSlug}.`
              : 'Nothing needed moving.'}
            {report.missing > 0 &&
              ` ${report.missing} row${report.missing === 1 ? '' : 's'} had no file on disk — run Rescan vault.`}
          </p>
          {report.leftBehind.length > 0 && (
            <div className="mt-1 rounded border border-amber-300 bg-amber-50 px-2 py-1.5 text-amber-900 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-200">
              <p className="font-medium">
                {report.leftBehind.length} file
                {report.leftBehind.length === 1 ? ' was' : 's were'} left where they are — something
                had them open. Close it and rename again to finish.
              </p>
              <ul className="selectable mt-1 space-y-0.5 font-mono text-[11px]">
                {report.leftBehind.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
