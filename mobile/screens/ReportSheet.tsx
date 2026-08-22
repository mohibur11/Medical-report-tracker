import { useState } from 'react';

import { Spinner } from './Capture.tsx';
import { useBusy } from '../busy.tsx';
import { formatDmy, parseDmyInput } from '../../src/lib/extract/dates.ts';
import {
  createCategory,
  setDocumentCategories,
  trashDocument,
  updateDocument,
  type Category,
  type DocumentRow,
  type Patient,
} from '../../src/lib/ipc.ts';

/**
 * One filed report, and everything about it that can be wrong.
 *
 * Until this existed a report filed on the phone could only be corrected on the
 * computer. That is the wrong way round: the phone is where reports get filed in
 * a hurry, next to the doctor's desk, and a date read off a photograph is exactly
 * the thing that turns out wrong later.
 *
 * Changing the date, the person or the title renames and moves the file on disk,
 * which is why it is one Save rather than a field that commits as you type.
 */
export function ReportSheet({
  doc,
  patients,
  categories,
  tagIds,
  onDone,
  onClose,
  onError,
}: {
  doc: DocumentRow;
  patients: Patient[];
  categories: Category[];
  tagIds: string[];
  /** Saved or trashed: the library behind this is now out of date. */
  onDone: () => void;
  onClose: () => void;
  onError: (e: string) => void;
}) {
  const busyGate = useBusy();
  const [date, setDate] = useState(formatDmy(doc.docDate));
  const [title, setTitle] = useState(doc.title);
  const [patientId, setPatientId] = useState(doc.patientId);
  const [docType, setDocType] = useState(doc.docType);
  const [notes, setNotes] = useState(doc.notes ?? '');
  const [tags, setTags] = useState<string[]>(tagIds);
  const [confirming, setConfirming] = useState(false);
  /** A tag being typed. Categories can only be made on the computer otherwise,
   *  which makes tagging on a phone impossible on a fresh install. */
  const [newTag, setNewTag] = useState<string | null>(null);
  const [known, setKnown] = useState(categories);
  const [busy, setBusy] = useState(false);

  const iso = parseDmyInput(date);
  const ready = Boolean(iso && title.trim() && patientId);

  async function save() {
    if (!iso || !patientId) return;
    setBusy(true);
    try {
      await busyGate.run('Saving the changes…', async () => {
        await updateDocument({
          documentId: doc.id,
          patientId,
          docDate: iso,
          title: title.trim(),
          docType,
          notes: notes.trim(),
        });
        await setDocumentCategories(doc.id, tags);
      });
      onDone();
    } catch (e) {
      onError(String(e));
      setBusy(false);
    }
  }

  async function trash() {
    setBusy(true);
    try {
      await busyGate.run('Moving to Trash…', () => trashDocument(doc.id));
      onDone();
    } catch (e) {
      onError(String(e));
      setBusy(false);
    }
  }

  const field =
    'h-12 w-full rounded-xl border border-slate-300 bg-white px-3 text-base outline-none ' +
    'focus:border-sky-500 dark:border-slate-700 dark:bg-slate-800';

  return (
    <div className="fixed inset-0 z-[800] flex flex-col justify-end bg-black/50">
      {/* Tapping away closes, the way every other sheet on the phone does. */}
      <button
        type="button"
        aria-label="Close"
        onClick={onClose}
        className="min-h-16 flex-1 cursor-default"
      />

      <div
        className="max-h-[88%] overflow-y-auto rounded-t-3xl bg-white p-4 dark:bg-slate-900"
        style={{
          paddingBottom: 'calc(env(safe-area-inset-bottom, 0px) + 1rem)',
        }}
      >
        <div className="mx-auto mb-3 h-1 w-10 rounded-full bg-slate-300 dark:bg-slate-700" />

        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold">Edit report</h2>
          <button
            type="button"
            onClick={onClose}
            className="h-10 rounded-lg px-3 text-sm text-slate-500 dark:text-slate-400"
          >
            Close
          </button>
        </div>

        <div className="mt-3 space-y-2">
          <input
            className={`${field} font-mono`}
            inputMode="numeric"
            placeholder="dd/mm/yyyy"
            value={date}
            onChange={(e) => setDate(e.target.value)}
            aria-label="Date"
          />
          {date.trim() && !iso && (
            <p className="text-xs text-red-600 dark:text-red-400">
              That date is not understood. It should look like 14/03/2026.
            </p>
          )}

          <input
            className={field}
            placeholder="What is it?"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            aria-label="Title"
          />

          <select
            className={field}
            value={patientId}
            onChange={(e) => setPatientId(e.target.value)}
            aria-label="Person"
          >
            {patients.map((p) => (
              <option key={p.id} value={p.id}>
                {p.displayName}
              </option>
            ))}
          </select>

          <select
            className={field}
            value={docType}
            onChange={(e) => setDocType(e.target.value)}
            aria-label="Kind"
          >
            <option value="report">Report</option>
            <option value="prescription">Prescription</option>
            <option value="invoice">Receipt</option>
            <option value="other">Other</option>
          </select>

          <div className="flex flex-wrap gap-2 pt-1">
            {known.map((c) => {
              const on = tags.includes(c.id);
              return (
                <button
                  key={c.id}
                  type="button"
                  onClick={() =>
                    setTags((prev) => (on ? prev.filter((id) => id !== c.id) : [...prev, c.id]))
                  }
                  className={`h-10 rounded-full border px-4 text-sm ${
                    on
                      ? 'border-sky-500 bg-sky-50 text-sky-800 dark:bg-sky-950 dark:text-sky-200'
                      : 'border-slate-300 text-slate-600 dark:border-slate-700 dark:text-slate-300'
                  }`}
                >
                  {c.name}
                </button>
              );
            })}

            {newTag === null ? (
              <button
                type="button"
                onClick={() => setNewTag('')}
                className="h-10 rounded-full border border-dashed border-slate-400 px-4 text-sm text-slate-500 dark:text-slate-400"
              >
                + New tag
              </button>
            ) : (
              <span className="flex w-full gap-2">
                <input
                  autoFocus
                  className={`${field} flex-1`}
                  placeholder="Thyroid, Diabetes…"
                  value={newTag}
                  onChange={(e) => setNewTag(e.target.value)}
                  aria-label="New tag"
                />
                <button
                  type="button"
                  disabled={!newTag.trim()}
                  onClick={() => {
                    const name = newTag.trim();
                    createCategory(name).then(
                      (made) => {
                        setKnown((prev) => [...prev, made]);
                        setTags((prev) => [...prev, made.id]);
                        setNewTag(null);
                      },
                      (e: unknown) => onError(String(e)),
                    );
                  }}
                  className="h-12 shrink-0 rounded-xl bg-sky-600 px-4 text-sm font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
                >
                  Add
                </button>
              </span>
            )}
          </div>

          <textarea
            className="min-h-20 w-full rounded-xl border border-slate-300 bg-white p-3 text-base outline-none focus:border-sky-500 dark:border-slate-700 dark:bg-slate-800"
            placeholder="Notes — what the paper does not say"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            aria-label="Notes"
          />

          <p className="text-xs text-slate-500 dark:text-slate-400">
            Changing the date, the person or the title renames the file and moves it into the right
            folder.
          </p>

          <button
            type="button"
            disabled={!ready || busy}
            onClick={() => void save()}
            className="h-12 w-full rounded-xl bg-sky-600 text-base font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
          >
            {busy ? (
              <span className="flex items-center justify-center gap-2">
                <Spinner small />
                Saving…
              </span>
            ) : (
              'Save changes'
            )}
          </button>

          {confirming ? (
            <div className="flex items-center gap-2 rounded-xl bg-red-50 p-2 dark:bg-red-950/50">
              <p className="flex-1 pl-1 text-xs text-red-800 dark:text-red-200">
                Move to Trash? It can be put back.
              </p>
              <button
                type="button"
                onClick={() => setConfirming(false)}
                className="h-10 rounded-lg px-3 text-sm text-slate-600 dark:text-slate-300"
              >
                Keep
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => void trash()}
                className="h-10 rounded-lg bg-red-600 px-4 text-sm font-medium text-white disabled:opacity-60"
              >
                Move
              </button>
            </div>
          ) : (
            <button
              type="button"
              disabled={busy}
              onClick={() => setConfirming(true)}
              className="h-11 w-full rounded-xl text-sm text-slate-500 active:bg-slate-100 dark:text-slate-400 dark:active:bg-slate-800"
            >
              Move to Trash
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
