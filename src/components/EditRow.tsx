import { useState } from 'react';

import { detectConflicts, isBlocked } from '../lib/extract/conflicts.ts';
import { formatDmy, parseDmyInput } from '../lib/extract/dates.ts';
import { buildName } from '../lib/naming/sanitize.ts';
import { updateDocument, type DocumentRow, type Patient } from '../lib/ipc.ts';

/**
 * Editing a filed document.
 *
 * The date, patient and title are all part of the filename, and the patient and
 * year are its folders — so saving here can move the file across the vault. The
 * preview shows exactly where it will end up, because a rename that silently
 * relocates a medical record is not something to discover later.
 */
export function EditRow({
  doc,
  patients,
  onSaved,
  onCancel,
}: {
  doc: DocumentRow;
  patients: Patient[];
  onSaved: () => void;
  onCancel: () => void;
}) {
  const [date, setDate] = useState(doc.docDate.startsWith('0000') ? '' : formatDmy(doc.docDate));
  const [title, setTitle] = useState(doc.title);
  const [patientId, setPatientId] = useState(doc.patientId);
  const [docType, setDocType] = useState(doc.docType);
  const [notes, setNotes] = useState(doc.notes ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const iso = date.trim() ? parseDmyInput(date) : '0000-00-00';
  const patient = patients.find((p) => p.id === patientId);

  const conflicts = detectConflicts({
    chosen: iso,
    patientDob: patient?.dob ?? null,
    patientName: patient?.displayName,
  });
  const blocked = isBlocked(conflicts);
  const ready = Boolean(iso && title.trim() && patient) && !blocked;

  const preview =
    patient && title.trim()
      ? buildName(
          {
            docDate: iso ?? '0000-00-00',
            patientName: patient.displayName,
            title,
            ext: doc.relPath.split('.').pop() ?? 'jpg',
          },
          60,
        )
      : null;

  const moves = preview ? preview.relPath !== doc.relPath : false;

  async function save() {
    if (!patient || !iso) return;
    setBusy(true);
    setError(null);
    try {
      await updateDocument({
        documentId: doc.id,
        patientId: patient.id,
        docDate: iso,
        title: title.trim(),
        docType,
        notes,
      });
      onSaved();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  const input =
    'rounded border border-slate-300 bg-white px-2 py-1 text-sm outline-none ' +
    'focus:border-sky-500 dark:border-slate-600 dark:bg-slate-800';

  return (
    <li className="border-l-2 border-sky-500 bg-sky-50/40 px-3 py-3 sm:px-6 dark:bg-sky-950/20">
      <div className="flex flex-wrap items-center gap-2">
        <input
          className={`${input} w-28 font-mono`}
          placeholder="dd/mm/yyyy"
          value={date}
          onChange={(e) => setDate(e.target.value)}
          aria-label="Date"
        />
        <input
          className={`${input} w-full sm:w-auto sm:min-w-52 sm:flex-1`}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && ready) void save();
            if (e.key === 'Escape') onCancel();
          }}
          aria-label="Title"
        />
        <select className={input} value={patientId} onChange={(e) => setPatientId(e.target.value)} aria-label="Patient">
          {patients.map((p) => (
            <option key={p.id} value={p.id}>
              {p.displayName}
            </option>
          ))}
        </select>
        <select className={input} value={docType} onChange={(e) => setDocType(e.target.value)} aria-label="Type">
          <option value="report">Report</option>
          <option value="prescription">Prescription</option>
          <option value="invoice">Invoice</option>
          <option value="other">Other</option>
        </select>
        <button
          type="button"
          disabled={!ready || busy}
          onClick={() => void save()}
          className="rounded bg-sky-600 px-3 py-1 text-sm font-medium text-white disabled:cursor-not-allowed disabled:bg-slate-300 dark:disabled:bg-slate-700"
        >
          {busy ? 'Saving…' : 'Save'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-white dark:border-slate-600"
        >
          Cancel
        </button>
      </div>

      {/* Notes are not part of the filename, so they never move the file. They are
          what the scan itself cannot say — what was advised, what to repeat and
          when — and they are searchable and carried in the sidecar. */}
      <textarea
        className={`${input} mt-2 block w-full resize-y`}
        rows={2}
        placeholder="Notes — what the doctor said, what to repeat and when"
        value={notes}
        onChange={(e) => setNotes(e.target.value)}
        aria-label="Notes"
      />

      {preview && (
        <p className="selectable mt-1.5 truncate font-mono text-[11px] text-slate-500 dark:text-slate-400">
          {preview.relPath}
          {moves && <span className="ml-2 text-sky-700 dark:text-sky-400">file will move</span>}
          {!iso && date.length > 0 && <span className="ml-2 text-amber-600">date not understood</span>}
        </p>
      )}

      {conflicts.map((c) => (
        <p
          key={c.kind}
          className={`mt-1.5 rounded border px-2 py-1.5 text-xs ${
            c.severity === 'blocking'
              ? 'border-red-300 bg-red-50 text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200'
              : 'border-amber-300 bg-amber-50 text-amber-900 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-200'
          }`}
        >
          <span className="font-medium">{c.severity === 'blocking' ? 'Cannot save: ' : 'Please check: '}</span>
          {c.message}
        </p>
      ))}

      {error && <p className="mt-1 text-xs text-red-600 dark:text-red-400">{error}</p>}
    </li>
  );
}
