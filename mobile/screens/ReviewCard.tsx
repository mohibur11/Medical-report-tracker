import { useEffect, useState } from 'react';

import { Spinner } from './Capture.tsx';
import { detectConflicts, isBlocked, type Conflict } from '../../src/lib/extract/conflicts.ts';
import {
  formatDmy,
  parseDmyInput,
  rankDateCandidates,
  type DateCandidate,
} from '../../src/lib/extract/dates.ts';
import { describe as describeDocument, type Suggestion } from '../../src/lib/extract/lexicon.ts';
import {
  commitItem,
  runOcr,
  setDocumentCategories,
  stagedThumb,
  unlockPdf,
  type Category,
  type IngestItem,
  type Patient,
} from '../../src/lib/ipc.ts';
import { ocrQueue } from '../../src/lib/queue.ts';

/**
 * One report, waiting to be confirmed.
 *
 * The same decisions as the desktop review row — date, person, what it is — laid
 * out as a card that fits a thumb. The date is the only field that has to be
 * right; everything else can be fixed later, so everything else has a sensible
 * default and stays out of the way.
 */
export function ReviewCard({
  item,
  patients,
  categories,
  onDone,
  onError,
}: {
  item: IngestItem;
  patients: Patient[];
  categories: Category[];
  onDone: () => void;
  onError: (e: string) => void;
}) {
  const [thumb, setThumb] = useState<string | null>(null);
  const [date, setDate] = useState('');
  const [title, setTitle] = useState('');
  const [patientId, setPatientId] = useState(patients[0]?.id ?? '');
  const [docType, setDocType] = useState('report');
  const [categoryId, setCategoryId] = useState('');
  const [candidates, setCandidates] = useState<DateCandidate[]>([]);
  const [ideas, setIdeas] = useState<Suggestion[]>([]);
  const [pageText, setPageText] = useState('');
  const [reading, setReading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [password, setPassword] = useState('');

  useEffect(() => {
    let alive = true;
    stagedThumb(item.id).then((s) => alive && setThumb(s), () => {});
    return () => {
      alive = false;
    };
  }, [item.id]);

  useEffect(() => {
    if (item.status === 'locked') return;
    let alive = true;
    setReading(true);
    ocrQueue
      .run(() => (alive ? runOcr(item.id) : Promise.resolve(null)))
      .then(
        (pages) => {
          if (!alive || !pages) return;
          setReading(false);
          const text = pages.map((p) => p.text).join('\n');
          setPageText(text);

          const ranked = rankDateCandidates(text);
          setCandidates(ranked);
          if (ranked[0]) setDate((d) => d || formatDmy(ranked[0]!.iso));

          const { docType: kind, titles } = describeDocument(text);
          setDocType((t) => (t === 'report' ? kind : t));
          setIdeas(titles);
          if (titles[0]) setTitle((t) => t || titles[0]!.title);
        },
        () => alive && setReading(false),
      );
    return () => {
      alive = false;
    };
  }, [item.id, item.status]);

  const iso = parseDmyInput(date);
  const patient = patients.find((p) => p.id === patientId);
  const conflicts: Conflict[] = detectConflicts({
    chosen: iso,
    chosenRaw: candidates.find((c) => c.iso === iso)?.raw ?? null,
    text: pageText,
    candidates,
    patientDob: patient?.dob ?? null,
    patientName: patient?.displayName,
  });
  const blocked = isBlocked(conflicts);
  const ready = Boolean(iso && title.trim() && patient) && !blocked;

  async function save() {
    if (!patient || !iso) return;
    setBusy(true);
    try {
      const doc = await commitItem({
        ingestId: item.id,
        patientId: patient.id,
        docDate: iso,
        title: title.trim(),
        docType,
      });
      if (categoryId) await setDocumentCategories(doc.id, [categoryId]);
      onDone();
    } catch (e) {
      onError(String(e));
      setBusy(false);
    }
  }

  async function unlock() {
    setBusy(true);
    try {
      await unlockPdf(item.id, password);
      setPassword('');
      onDone();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const field =
    'h-12 w-full rounded-xl border border-slate-300 bg-white px-3 text-base outline-none ' +
    'focus:border-sky-500 dark:border-slate-700 dark:bg-slate-800';

  if (item.status === 'locked') {
    return (
      <div className="rounded-2xl border border-amber-300 bg-amber-50 p-4 dark:border-amber-900 dark:bg-amber-950/50">
        <p className="truncate text-sm font-medium">{item.fileName}</p>
        <p className="mt-1 text-xs text-amber-800 dark:text-amber-200">
          This PDF needs a password before it can be read.
        </p>
        <input
          type="password"
          className={`${field} mt-3`}
          placeholder="Password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <button
          type="button"
          disabled={!password || busy}
          onClick={() => void unlock()}
          className="mt-2 h-12 w-full rounded-xl bg-sky-600 text-sm font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
        >
          {busy ? 'Unlocking…' : 'Unlock'}
        </button>
      </div>
    );
  }

  return (
    <div className="overflow-hidden rounded-2xl border border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900">
      <div className="flex gap-3 p-3">
        <div className="h-24 w-18 shrink-0 overflow-hidden rounded-lg bg-slate-100 dark:bg-slate-800">
          {thumb ? (
            <img src={thumb} alt="" className="h-full w-full object-cover" />
          ) : (
            <div className="flex h-full items-center justify-center text-[10px] font-medium uppercase text-slate-400">
              {item.fileKind}
            </div>
          )}
        </div>
        <div className="min-w-0 flex-1">
          <p className="selectable truncate text-xs text-slate-500 dark:text-slate-400">
            {item.fileName}
          </p>
          <input
            className={`${field} mt-1 font-mono`}
            inputMode="numeric"
            placeholder="dd/mm/yyyy"
            value={date}
            onChange={(e) => setDate(e.target.value)}
            aria-label="Date"
          />
          {reading && (
            <p className="mt-1 flex items-center gap-2 text-xs text-slate-400">
              <Spinner small />
              reading the page…
            </p>
          )}
        </div>
      </div>

      {candidates.length > 0 && (
        <div className="flex gap-2 overflow-x-auto px-3 pb-2">
          {candidates.map((c) => (
            <button
              key={c.iso}
              type="button"
              onClick={() => setDate(formatDmy(c.iso))}
              className={`h-9 shrink-0 rounded-full border px-3 text-xs ${
                iso === c.iso
                  ? 'border-sky-500 bg-sky-50 text-sky-800 dark:bg-sky-950 dark:text-sky-200'
                  : 'border-slate-300 text-slate-600 dark:border-slate-700 dark:text-slate-300'
              }`}
            >
              {formatDmy(c.iso)}
              {c.anchor && <span className="ml-1 opacity-60">{c.anchor}</span>}
            </button>
          ))}
        </div>
      )}

      <div className="space-y-2 px-3 pb-3">
        <input
          className={field}
          placeholder="What is it? e.g. Thyroid Profile"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          aria-label="Title"
        />

        {ideas.length > 1 && (
          <div className="flex gap-2 overflow-x-auto">
            {ideas.slice(1).map((s) => (
              <button
                key={s.title}
                type="button"
                onClick={() => setTitle(s.title)}
                className="h-9 shrink-0 rounded-full border border-slate-300 px-3 text-xs text-slate-600 dark:border-slate-700 dark:text-slate-300"
              >
                {s.title}
              </button>
            ))}
          </div>
        )}

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

        <div className="flex gap-2">
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
          <select
            className={field}
            value={categoryId}
            onChange={(e) => setCategoryId(e.target.value)}
            aria-label="Category"
          >
            <option value="">No tag</option>
            {categories.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </div>

        {conflicts.map((c) => (
          <div
            key={c.kind}
            className={`rounded-xl px-3 py-2 text-xs ${
              c.severity === 'blocking'
                ? 'bg-red-50 text-red-800 dark:bg-red-950 dark:text-red-200'
                : 'bg-amber-50 text-amber-900 dark:bg-amber-950 dark:text-amber-200'
            }`}
          >
            {c.message}
            {c.options && (
              <div className="mt-2 flex gap-2">
                {c.options.map((o) => (
                  <button
                    key={o.value}
                    type="button"
                    onClick={() => setDate(formatDmy(o.value))}
                    className="h-9 rounded-full border border-current px-3 font-medium"
                  >
                    {o.label}
                  </button>
                ))}
              </div>
            )}
          </div>
        ))}

        <button
          type="button"
          disabled={!ready || busy}
          onClick={() => void save()}
          className="h-12 w-full rounded-xl bg-sky-600 text-base font-medium text-white active:bg-sky-700 disabled:bg-slate-300 dark:disabled:bg-slate-700"
        >
          {busy ? (
            <span className="flex items-center justify-center gap-2">
              <Spinner small />
              Saving…
            </span>
          ) : (
            'Save'
          )}
        </button>
      </div>
    </div>
  );
}
