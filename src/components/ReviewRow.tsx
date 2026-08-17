import { useState } from 'react';

import { Thumb } from './Thumb.tsx';
import { parseDmyInput } from '../lib/extract/dates.ts';
import { buildName } from '../lib/naming/sanitize.ts';
import { commitItem, type IngestItem, type Patient } from '../lib/ipc.ts';

/**
 * One row of the review queue: confirm what this file is, then file it.
 *
 * The filename preview is computed locally rather than over IPC. This screen is
 * meant to be keyboard-first — if correcting a guess costs more than a few
 * seconds the whole app stops being worth using — and a round trip per keystroke
 * would make the preview lag the typing.
 */
export function ReviewRow({
  item,
  patients,
  defaultPatientId,
  onCommitted,
}: {
  item: IngestItem;
  patients: Patient[];
  defaultPatientId: string | null;
  /** Reports the chosen patient so the next row can default to it — a backlog
   *  import is almost always one person at a time. */
  onCommitted: (id: string, fileName: string, patientId: string) => void;
}) {
  const [date, setDate] = useState('');
  const [title, setTitle] = useState('');
  const [patientId, setPatientId] = useState(defaultPatientId ?? '');
  const [docType, setDocType] = useState('report');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const iso = parseDmyInput(date);
  const patient = patients.find((p) => p.id === patientId);
  const ready = Boolean(iso && title.trim() && patient);

  // Undated files are filed under <Patient>/Undated rather than guessed into a
  // year, so an empty date is allowed but visibly different.
  const preview =
    patient && title.trim()
      ? buildName(
          {
            docDate: iso ?? '0000-00-00',
            patientName: patient.displayName,
            title,
            ext: item.fileKind === 'pdf' ? 'pdf' : 'jpg',
          },
          60,
        )
      : null;

  async function file() {
    if (!patient || !title.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const doc = await commitItem({
        ingestId: item.id,
        patientId: patient.id,
        docDate: iso ?? '0000-00-00',
        title: title.trim(),
        docType,
      });
      onCommitted(item.id, doc.fileName, patient.id);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  const input =
    'rounded border border-slate-300 bg-white px-2 py-1 text-sm outline-none ' +
    'focus:border-sky-500 dark:border-slate-600 dark:bg-slate-800';

  return (
    <li className="flex items-start gap-4 px-6 py-3">
      <Thumb id={item.id} kind={item.fileKind} />

      <div className="min-w-0 flex-1">
        <p className="selectable truncate text-xs text-slate-500 dark:text-slate-400">
          {item.fileName}
          {item.orientationBaked && (
            <span className="ml-2 text-sky-600 dark:text-sky-400">
              rotated upright (EXIF {item.exifOrientation})
            </span>
          )}
        </p>

        <div className="mt-2 flex flex-wrap items-center gap-2">
          <input
            className={`${input} w-28 font-mono`}
            placeholder="dd/mm/yyyy"
            value={date}
            onChange={(e) => setDate(e.target.value)}
            aria-label="Date"
          />
          <input
            className={`${input} min-w-52 flex-1`}
            placeholder="Test or report name"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && ready && void file()}
            aria-label="Title"
          />
          <select
            className={input}
            value={patientId}
            onChange={(e) => setPatientId(e.target.value)}
            aria-label="Patient"
          >
            <option value="">Which patient?</option>
            {patients.map((p) => (
              <option key={p.id} value={p.id}>
                {p.displayName}
              </option>
            ))}
          </select>
          <select
            className={input}
            value={docType}
            onChange={(e) => setDocType(e.target.value)}
            aria-label="Type"
          >
            <option value="report">Report</option>
            <option value="prescription">Prescription</option>
            <option value="invoice">Invoice</option>
            <option value="other">Other</option>
          </select>
          <button
            type="button"
            disabled={!ready || busy}
            onClick={() => void file()}
            className="rounded bg-sky-600 px-3 py-1 text-sm font-medium text-white disabled:cursor-not-allowed disabled:bg-slate-300 dark:disabled:bg-slate-700"
          >
            {busy ? 'Filing…' : 'File'}
          </button>
        </div>

        {preview && (
          <p className="selectable mt-1.5 truncate font-mono text-[11px] text-slate-500 dark:text-slate-400">
            {preview.relPath}
            {!iso && date.length > 0 && (
              <span className="ml-2 text-amber-600">date not understood</span>
            )}
            {preview.titleTruncated && (
              <span className="ml-2 text-amber-600">name shortened on disk</span>
            )}
          </p>
        )}

        {error && <p className="mt-1 text-xs text-red-600 dark:text-red-400">{error}</p>}
      </div>
    </li>
  );
}
