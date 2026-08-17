import { useEffect, useState } from 'react';

import { Thumb } from './Thumb.tsx';
import { detectConflicts, isBlocked, type Conflict } from '../lib/extract/conflicts.ts';
import { formatDmy, parseDmyInput, rankDateCandidates, type DateCandidate } from '../lib/extract/dates.ts';
import { describe as describeDocument, type Suggestion } from '../lib/extract/lexicon.ts';
import { buildName } from '../lib/naming/sanitize.ts';
import { commitItem, runOcr, type IngestItem, type Patient } from '../lib/ipc.ts';

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
  const [reading, setReading] = useState(false);
  const [candidates, setCandidates] = useState<DateCandidate[]>([]);
  const [pageText, setPageText] = useState('');
  const [titleIdeas, setTitleIdeas] = useState<Suggestion[]>([]);

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
    runOcr(item.id).then(
      (pages) => {
        if (!alive) return;
        setReading(false);

        // Rank across the whole document. A twelve-page report carries its date
        // on the first page, but a covering letter or a lab slip can put it
        // anywhere, and pages are cheap to read together.
        const text = pages.map((p) => p.text).join('\n');
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
  }, [item.id, item.status]);

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

        {(reading || candidates.length > 0) && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-[11px]">
            {reading && <span className="text-slate-400">reading the page…</span>}
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
