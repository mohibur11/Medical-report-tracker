import { useEffect, useRef, useState } from 'react';

import { RemoveFromQueue } from './RemoveFromQueue.tsx';
import { Thumb } from './Thumb.tsx';
import { detectConflicts, isBlocked, type Conflict } from '../lib/extract/conflicts.ts';
import { formatDmy, parseDmyInput, rankDateCandidates, type DateCandidate } from '../lib/extract/dates.ts';
import { describe as describeDocument, type Suggestion } from '../lib/extract/lexicon.ts';
import { buildName } from '../lib/naming/sanitize.ts';
import {
  commitItem,
  runOcr,
  stagedPdfTextSource,
  type IngestItem,
  type Patient,
} from '../lib/ipc.ts';
import { extractText } from '../lib/pdf.ts';
import { ocrQueue } from '../lib/queue.ts';

/**
 * One row of the review queue: confirm what this file is, then file it.
 *
 * The filename preview is computed locally rather than over IPC. This screen is
 * meant to be keyboard-first — if correcting a guess costs more than a few
 * seconds the whole app stops being worth using — and a round trip per keystroke
 * would make the preview lag the typing.
 */
/**
 * What the bulk bar is allowed to do to a row it does not own.
 *
 * Patient and type can be set for a whole batch; the date deliberately cannot.
 * A wrong date is the one failure that never announces itself — it produces a
 * subtly misordered PDF handed to a doctor — so it stays a per-row decision.
 */
export interface RowApi {
  ready: boolean;
  /** Why this row would be skipped by a bulk file, in words. */
  skipReason: string | null;
  setPatient: (id: string) => void;
  setDocType: (t: string) => void;
  /** Resolves to the new document id, or null if the row refused to file. */
  file: () => Promise<string | null>;
}

/** Read at call time, so the bulk bar never acts on a stale closure. */
export type RowHandle = { current: RowApi };

export function ReviewRow({
  item,
  patients,
  defaultPatientId,
  selected,
  onToggle,
  onRegister,
  onCommitted,
  onRemoved,
}: {
  item: IngestItem;
  patients: Patient[];
  defaultPatientId: string | null;
  selected: boolean;
  onToggle: (shift: boolean) => void;
  /** null on unmount. */
  onRegister: (id: string, handle: RowHandle | null) => void;
  /** Reports the chosen patient so the next row can default to it — a backlog
   *  import is almost always one person at a time. */
  onCommitted: (id: string, fileName: string, patientId: string) => void;
  /** The row was taken out of the queue rather than filed. */
  onRemoved: (id: string, fileName: string) => void;
}) {
  const [date, setDate] = useState('');
  const [title, setTitle] = useState('');
  const [patientId, setPatientId] = useState(defaultPatientId ?? '');
  const [docType, setDocType] = useState('report');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reading, setReading] = useState(false);
  const [candidates, setCandidates] = useState<DateCandidate[]>([]);
  const [pageText, setPageText] = useState('');
  const [titleIdeas, setTitleIdeas] = useState<Suggestion[]>([]);
  /** True when the text came out of the PDF itself rather than the recognizer. */
  const [exactText, setExactText] = useState(false);
  /** Clockwise degrees the page was turned so its text reads upright. */
  const [turned, setTurned] = useState(item.textRotation ?? 0);
  /** Bumped whenever the staged file changed, so the thumbnail is fetched again. */
  const [fileVersion, setFileVersion] = useState(0);
  /** Bumped by a hand turn: the old read is gone and the page is read again. */
  const [readPass, setReadPass] = useState(0);

  function onTurned(degrees: number) {
    setTurned(degrees);
    setFileVersion((v) => v + 1);
    setReadPass((n) => n + 1);
  }

  /**
   * Read the page and pre-fill the date.
   *
   * Recognition is not the hard part — a lab report carries four to six dates and
   * the dominant failure is picking the wrong one. So every candidate is kept and
   * offered as a chip: the winner is a suggestion, not an answer, and correcting
   * it is one click rather than retyping.
   */
  useEffect(() => {
    if (item.status === 'failed' || item.status === 'duplicate') return;
    let alive = true;
    setReading(true);
    // Through the queue, not straight to the backend: a backlog import mounts
    // every row at once, and a row the user has already scrolled past should not
    // hold a recognition slot.
    ocrQueue
      .run(async () => {
        if (!alive) return null;

        // A PDF emailed straight from a lab carries its text exactly. Reading it
        // is faster than recognising pixels and cannot misread a digit, so it is
        // tried first; a scan carries no text and falls through to the recognizer.
        if (item.fileKind === 'pdf') {
          try {
            const b64 = await stagedPdfTextSource(item.id);
            if (b64) {
              const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
              const pages = await extractText(bytes);
              if (pages.length > 0) {
                return { exact: true, text: pages.map((p) => p.text).join('\n') };
              }
            }
          } catch {
            // Not worth reporting: recognition still works, and does the same job.
          }
        }

        if (!alive) return null;
        const read = await runOcr(item.id);
        return {
          exact: false,
          text: read.pages.map((p) => p.text).join('\n'),
          turned: read.textRotation,
        };
      })
      .then(
      (read) => {
        if (!alive || !read) return;
        setReading(false);
        setExactText(read.exact);

        // The first read of an image may have turned the file on disk, in
        // which case the thumbnail fetched on mount shows it the old way up.
        if (read.turned !== undefined && read.turned !== turned) {
          setTurned(read.turned);
          setFileVersion((v) => v + 1);
        }

        // Rank across the whole document. A twelve-page report carries its date
        // on the first page, but a covering letter or a lab slip can put it
        // anywhere, and pages are cheap to read together.
        const text = read.text;
        setPageText(text);
        const ranked = rankDateCandidates(text);
        setCandidates(ranked);

        // Only pre-fill untouched fields — never overwrite typing.
        const top = ranked[0];
        if (top) setDate((current) => current || formatDmy(top.iso));

        // Kind and title are decided together: a receipt that prints
        // "Prescription No." is still a receipt, and must not be titled after
        // the medicines or the lab tests it happens to list.
        const { docType: kind, titles: ideas } = describeDocument(text);
        setDocType((current) => (current === 'report' ? kind : current));
        setTitleIdeas(ideas);
        if (ideas[0]) setTitle((current) => current || ideas[0]!.title);
      },
      () => alive && setReading(false),
    );
    return () => {
      alive = false;
    };
  }, [item.id, item.status, readPass]);

  const iso = parseDmyInput(date);
  const patient = patients.find((p) => p.id === patientId);

  // Everything the app noticed but must not decide alone. Blocking ones refuse
  // the save; the rest are questions shown next to the answer.
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

  async function file(): Promise<string | null> {
    if (!patient || !title.trim()) return null;
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
      return doc.id;
    } catch (e) {
      setError(String(e));
      setBusy(false);
      return null;
    }
  }

  // Said in the words the row would use itself, so a bulk file can report
  // exactly what it left behind instead of a bare count.
  const skipReason = !patient
    ? 'no patient chosen'
    : !title.trim()
      ? 'no title'
      : !iso
        ? date.trim()
          ? 'date not understood'
          : 'no date'
        : blocked
          ? (conflicts.find((c) => c.severity === 'blocking')?.message ?? 'needs checking')
          : null;

  // A ref, not a value: the bulk bar reads this when it acts, by which point
  // several rows may have re-rendered.
  const handle = useRef<RowApi>({} as RowApi);
  handle.current = { ready, skipReason, setPatient: setPatientId, setDocType, file };

  useEffect(() => {
    onRegister(item.id, handle);
    return () => onRegister(item.id, null);
  }, [item.id, onRegister]);

  const input =
    'rounded border border-slate-300 bg-white px-2 py-1 text-sm outline-none ' +
    'focus:border-sky-500 dark:border-slate-600 dark:bg-slate-800';

  return (
    <li
      className={`flex items-start gap-2 px-3 py-3 sm:gap-4 sm:px-6 ${
        selected ? 'bg-sky-50 dark:bg-sky-950/40' : ''
      }`}
    >
      <input
        type="checkbox"
        checked={selected}
        // Shift-range needs the modifier, which `onChange` does not carry.
        onChange={() => {}}
        onClick={(e) => onToggle(e.shiftKey)}
        aria-label={`Select ${item.fileName}`}
        className="mt-1 size-4 shrink-0 accent-sky-600"
      />
      <Thumb id={item.id} kind={item.fileKind} version={fileVersion} onTurned={onTurned} />

      <div className="min-w-0 flex-1">
        <p className="selectable truncate text-xs text-slate-500 dark:text-slate-400">
          {item.fileName}
          {item.orientationBaked && (
            <span className="ml-2 text-sky-600 dark:text-sky-400">
              rotated upright (EXIF {item.exifOrientation})
            </span>
          )}
          {turned !== 0 && (
            <span className="ml-2 text-sky-600 dark:text-sky-400">
              turned {turned}° so the text reads upright
            </span>
          )}
        </p>

        <div className="mt-2 flex flex-wrap items-center gap-2">
          <input
            className={`${input} w-28 shrink-0 font-mono`}
            placeholder="dd/mm/yyyy"
            value={date}
            onChange={(e) => setDate(e.target.value)}
            aria-label="Date"
          />
          <input
            // Full width of its own line on a phone; shares the row on a desktop.
            className={`${input} w-full sm:w-auto sm:min-w-52 sm:flex-1`}
            placeholder="Test or report name"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && ready && void file()}
            aria-label="Title"
          />
          <select
            className={`${input} min-w-0 flex-1 sm:flex-none`}
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
            className={`${input} min-w-0 flex-1 sm:flex-none`}
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
          <RemoveFromQueue item={item} onRemoved={onRemoved} />
        </div>

        {(reading || candidates.length > 0) && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-[11px]">
            {reading && <span className="text-slate-400">reading the page…</span>}
            {exactText && !reading && (
              <span
                className="text-emerald-700 dark:text-emerald-400"
                title="This PDF carried its own text, so nothing had to be read from pixels"
              >
                exact text
              </span>
            )}
            {candidates.map((c) => {
              const chosen = iso === c.iso;
              return (
                <button
                  key={c.iso}
                  type="button"
                  onClick={() => setDate(formatDmy(c.iso))}
                  title={`read as "${c.raw}" — ${c.reason}`}
                  className={`rounded border px-1.5 py-0.5 ${
                    chosen
                      ? 'border-sky-500 bg-sky-50 text-sky-800 dark:bg-sky-950 dark:text-sky-200'
                      : 'border-slate-300 text-slate-600 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-300 dark:hover:bg-slate-800'
                  }`}
                >
                  {formatDmy(c.iso)}
                  {c.anchor && <span className="ml-1 opacity-60">{c.anchor}</span>}
                </button>
              );
            })}
            {!reading && candidates.length === 0 && (
              <span className="text-slate-400">no date found — type it</span>
            )}
          </div>
        )}

        {titleIdeas.length > 1 && (
          <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[11px]">
            <span className="text-slate-400">or:</span>
            {titleIdeas.slice(1).map((s) => (
              <button
                key={s.title}
                type="button"
                onClick={() => setTitle(s.title)}
                title={`matched "${s.matched}"`}
                className="rounded border border-slate-300 px-1.5 py-0.5 text-slate-600 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-300 dark:hover:bg-slate-800"
              >
                {s.title}
              </button>
            ))}
          </div>
        )}

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

        {conflicts.map((c) => (
          <div
            key={c.kind}
            className={`mt-1.5 rounded border px-2 py-1.5 text-xs ${
              c.severity === 'blocking'
                ? 'border-red-300 bg-red-50 text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200'
                : 'border-amber-300 bg-amber-50 text-amber-900 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-200'
            }`}
          >
            <span className="font-medium">
              {c.severity === 'blocking' ? 'Cannot file: ' : 'Please check: '}
            </span>
            {c.message}
            {c.options && (
              <span className="ml-1 inline-flex flex-wrap gap-1.5 align-middle">
                {c.options.map((o) => (
                  <button
                    key={o.value}
                    type="button"
                    onClick={() => setDate(formatDmy(o.value))}
                    className="rounded border border-current px-1.5 py-0.5 font-medium hover:bg-white/60 dark:hover:bg-black/20"
                  >
                    {o.label}
                  </button>
                ))}
              </span>
            )}
          </div>
        ))}

        {error && <p className="mt-1 text-xs text-red-600 dark:text-red-400">{error}</p>}
      </div>
    </li>
  );
}
