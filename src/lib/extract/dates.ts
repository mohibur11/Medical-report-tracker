/**
 * Date candidate extraction and ranking.
 *
 * Recognition is not the hard part — a lab report carries four to six dates and
 * the dominant failure is picking the WRONG one. Choosing the date of birth files
 * a 2026 report under 1978, where it sorts to the top of the folder forever and
 * never announces itself as an error.
 *
 * So this module does not "find the date". It produces a RANKED list of candidates
 * with the evidence for each, which the confirmation grid shows the user: the
 * winner prefilled, the runners-up one click away.
 *
 * Dates are South Asian convention: day first. '03/04/2026' is 3 April.
 */

export type DateSource = 'pdf_text' | 'ocr' | 'exif' | 'manual' | 'mtime';

export interface DateCandidate {
  /** Normalized to YYYY-MM-DD. */
  iso: string;
  /** Exactly as it appeared in the source text, for showing the user. */
  raw: string;
  /** Character offset in the source text — used to crop the image snippet. */
  index: number;
  /** The label found immediately before this date, if any. */
  anchor: string | null;
  /** How much that label was worth. A bare "date" is weak evidence; "sample
   *  collected" is strong. Kept so downstream rules can tell them apart. */
  anchorWeight: number;
  score: number;
  /** Human-readable justification, shown in the grid tooltip. */
  reason: string;
}

/**
 * Labels that mark the date a report is ABOUT. Ordered strongest first.
 *
 * Drawn from real documents rather than guessed: Bangladeshi and Thai hospital
 * reports, pathology reports, radiology reports, visit slips and receipts.
 */
const POSITIVE_ANCHORS: Array<[RegExp, number, string]> = [
  [/\b(sample\s*(collected|collection)|collected\s*(on|date)?|collection\s*date|drawn)\b/i, 100, 'sample collected'],
  [/\b(specimen|sampling)\s*(date|on)?\b/i, 90, 'specimen date'],
  // Radiology reports label the study itself this way, and it beats the date the
  // report was typed up or handed over.
  [/\b(exam|study|scan|imaging|procedure)\s*date\b/i, 85, 'exam date'],
  [/\b(date\s*of\s*(operation|procedure|surgery))\b/i, 80, 'operation date'],
  [/\b(received|recd|receiv(ed)?\s*date)\b/i, 60, 'received'],
  [/\b(report(ed)?\s*(on|date)?|result\s*date|date\s*of\s*report)\b/i, 45, 'reported'],
  [/\b(requested\s*date|date\s*requested|ordered\s*(on|date))\b/i, 40, 'requested'],
  [/\b(registered|registration|admitted|visit|consultation|en\s*date)\b/i, 35, 'registered/visit'],
  // The report was handed over after it was produced, so this loses to almost
  // anything else that identifies the study itself.
  [/\b(delivery\s*date|delivered)\b/i, 25, 'delivered'],
  [/\b(date)\b/i, 15, 'generic "date"'],
];

/** Labels that mark a date the report is NOT about. A wrong pick here is the
 *  single most damaging silent failure in the app. */
const NEGATIVE_ANCHORS: Array<[RegExp, number, string]> = [
  // "Birth Date" as well as "Date of Birth": real radiology sheets use the former,
  // and missing it filed a 2026 scan under 1994 in the accuracy run.
  [/\b(date\s*of\s*birth|birth\s*date|d\.?o\.?b\.?|born)\b/i, -400, 'date of birth'],
  [/\b(age)\b/i, -200, 'age field'],
  [/\b(printed?\s*(on|date)?|print\s*time)\b/i, -120, 'printed on'],
  [/\b(expiry|expires|valid\s*(till|until)|due)\b/i, -150, 'expiry'],
  [/\b(next\s*(visit|appointment|follow\s*up))\b/i, -150, 'next appointment'],
  // Form revision stamps, e.g. "F/M-CAS-012.1 Rev.0 (15 Dec 2017)" in the footer
  // of hospital receipts — a real date, and never the document's own.
  [/\b(rev\.?\s*\d*|revision|form\s*no|version)\b/i, -200, 'form revision'],
];

/** How far back to look for a label. Long enough to clear "Sample Collected : ",
 *  short enough not to borrow the previous field's label. */
const ANCHOR_WINDOW = 34;

const MONTHS: Record<string, number> = {
  jan: 1, feb: 2, mar: 3, apr: 4, may: 5, jun: 6,
  jul: 7, aug: 8, sep: 9, oct: 10, nov: 11, dec: 12,
};

/** Earliest plausible report date. Anything older is almost always a DOB or a
 *  misread year, not a medical record this user is filing. */
export const MIN_YEAR = 1990;

const PATTERNS = [
  // 14/03/2026, 14-03-26, 14.03.2026  — day first
  { re: /\b(\d{1,2})[/\-.](\d{1,2})[/\-.](\d{2,4})\b/g, kind: 'dmy' as const },
  // 2026-03-14  — ISO, unambiguous
  { re: /\b(\d{4})[/\-.](\d{1,2})[/\-.](\d{1,2})\b/g, kind: 'ymd' as const },
  // 14 Mar 2026 / 14-March-2026
  { re: /\b(\d{1,2})[\s\-]*([A-Za-z]{3,9})[\s\-,]*(\d{2,4})\b/g, kind: 'dMy' as const },
  // Mar 14, 2026
  { re: /\b([A-Za-z]{3,9})[\s\-]+(\d{1,2}),?[\s\-]+(\d{2,4})\b/g, kind: 'Mdy' as const },
];

/** Expand a 2-digit year. A medical archive looks backwards, so anything that
 *  would land in the future rolls back a century. */
function expandYear(y: number, today: Date): number {
  if (y >= 100) return y;
  const cc = Math.floor(today.getFullYear() / 100) * 100;
  const candidate = cc + y;
  return candidate > today.getFullYear() ? candidate - 100 : candidate;
}

function iso(y: number, m: number, d: number): string | null {
  if (m < 1 || m > 12 || d < 1) return null;
  const dim = new Date(Date.UTC(y, m, 0)).getUTCDate();
  if (d > dim) return null;
  return `${String(y).padStart(4, '0')}-${String(m).padStart(2, '0')}-${String(d).padStart(2, '0')}`;
}

/**
 * The label for a date is the text immediately before it — but only as far back
 * as the previous date. A label belongs to the value that follows it, so reading
 * past an earlier date would let "Birth Date" in "Birth Date 09-03-1992 Exam Date
 * 11-05-2026" condemn both of them.
 */
function anchorFor(
  text: string,
  index: number,
  previousEnd = 0,
): { label: string | null; weight: number; why: string } {
  const start = Math.max(previousEnd, index - ANCHOR_WINDOW);
  const window = text.slice(start, index);

  // Negatives win ties: a date labelled both "Date" and "of Birth" is a DOB.
  for (const [re, weight, why] of NEGATIVE_ANCHORS) {
    if (re.test(window)) return { label: why, weight, why };
  }
  for (const [re, weight, why] of POSITIVE_ANCHORS) {
    if (re.test(window)) return { label: why, weight, why };
  }
  return { label: null, weight: 0, why: 'no label nearby' };
}

export interface RankOptions {
  /** Hard-reject candidates equal to a known date of birth. The strongest
   *  available signal, and it costs one comparison. Accepts several because a
   *  document can name more than one person. */
  patientDob?: string | string[] | null;
  /** Injected so ranking is deterministic in tests. */
  today?: Date;
}

/**
 * A candidate this many years older than the newest date on the page, with no
 * label vouching for it, is treated as biographical rather than clinical.
 *
 * Real radiology sheets lay labels and values out in separate columns, which OCR
 * flattens into "Patient Name Birth Date Gender ... 09-03-1992 ... 11-05-2026".
 * Proximity cannot associate the label with its value there, but the age gap
 * still can: a report is filed near when it is created, so the decades-old date
 * beside recent content is a birth date.
 */
const STALE_YEARS = 10;

/** The weakest anchor that counts as a real label rather than an incidental word. */
const STRONG_ANCHOR = 35;

/**
 * Find every date-shaped string and rank it by how likely it is to be the date
 * the document is ABOUT. Returns highest score first; an empty array means the
 * user must type a date, which for handwritten prescriptions is the normal path.
 */
export function rankDateCandidates(text: string, opts: RankOptions = {}): DateCandidate[] {
  const today = opts.today ?? new Date();
  const todayIso = today.toISOString().slice(0, 10);
  const dobs = typeof opts.patientDob === 'string'
    ? [opts.patientDob]
    : (opts.patientDob ?? []);
  /** Every date-shaped match with where it sits, before any scoring. */
  const raws: Array<{ iso: string; raw: string; index: number; end: number }> = [];

  for (const { re, kind } of PATTERNS) {
    re.lastIndex = 0;
    let m: RegExpExecArray | null;

    while ((m = re.exec(text)) !== null) {
      const [, a, b, c] = m;
      // Every pattern has exactly three capture groups; a zero-length match would
      // also spin the loop forever, so bail rather than trust the shape.
      if (a === undefined || b === undefined || c === undefined) continue;
      if (m[0].length === 0) { re.lastIndex++; continue; }

      const month = (name: string): number | undefined => MONTHS[name.slice(0, 3).toLowerCase()];

      let y: number, mo: number | undefined, d: number;

      if (kind === 'dmy') {
        d = +a; mo = +b; y = expandYear(+c, today);
        // Day-first is the convention, but >12 in the second slot can only be a day.
        if (mo > 12 && d <= 12) { const t = d; d = mo; mo = t; }
      } else if (kind === 'ymd') {
        y = +a; mo = +b; d = +c;
      } else if (kind === 'dMy') {
        d = +a; mo = month(b); y = expandYear(+c, today);
      } else {
        mo = month(a); d = +b; y = expandYear(+c, today);
      }

      if (!mo) continue;
      const value = iso(y, mo, d);
      if (!value) continue;

      // Hard rejections — these are never the answer, whatever the label says.
      if (y < MIN_YEAR) continue;
      if (value > todayIso) continue;
      if (dobs.includes(value)) continue;

      raws.push({ iso: value, raw: m[0], index: m.index, end: m.index + m[0].length });
    }
  }

  // Left to right, longest first where two patterns start together.
  raws.sort((a, b) => a.index - b.index || b.raw.length - a.raw.length);

  const seen = new Map<string, DateCandidate>();
  let previousEnd = 0;

  for (const r of raws) {
    // Two patterns can match the same region; the first one wins and the
    // overlapping remainder is not a separate date.
    if (r.index < previousEnd) continue;

    const { label, weight, why } = anchorFor(text, r.index, previousEnd);
    previousEnd = r.end;

    // A date appearing under several labels keeps its best evidence.
    const score = weight + positionBonus(r.index, text.length);
    const prev = seen.get(r.iso);
    if (prev && prev.score >= score) continue;

    seen.set(r.iso, {
      iso: r.iso,
      raw: r.raw,
      index: r.index,
      anchor: label,
      anchorWeight: weight,
      score,
      reason: why,
    });
  }

  const found = [...seen.values()];

  // Penalise a candidate that is much older than everything else on the page and
  // has no label vouching for it. This is what catches a date of birth when the
  // layout puts its label out of reach of proximity matching.
  const newestYear = found.reduce((max, c) => Math.max(max, Number(c.iso.slice(0, 4))), 0);
  for (const c of found) {
    const gap = newestYear - Number(c.iso.slice(0, 4));
    // A bare "date" nearby is not evidence that a decades-old value is the
    // document's own date — only a specific label like "collected" or "reported"
    // is. Without that, the age gap decides.
    if (gap >= STALE_YEARS && c.anchorWeight < STRONG_ANCHOR) {
      c.score -= 300;
      c.reason = `${c.reason}; ${gap} years older than the rest of the page`;
    }
  }

  return found.sort((a, b) => b.score - a.score || a.index - b.index);
}

/** Report metadata clusters in the header. A late date is more likely a footer
 *  timestamp or a follow-up instruction than the report's own date. */
function positionBonus(index: number, length: number): number {
  if (length === 0) return 0;
  const rel = index / length;
  return rel < 0.4 ? 10 : rel > 0.8 ? -10 : 0;
}

/** The single best guess, or null when the user must be asked. */
export function bestDate(text: string, opts: RankOptions = {}): DateCandidate | null {
  const ranked = rankDateCandidates(text, opts);
  const top = ranked[0];
  // An unlabelled date is a guess, not an answer — but still worth prefilling,
  // because correcting a prefilled field is faster than typing an empty one.
  return top ?? null;
}

/** Which way round a purely numeric date is written. */
export type DateOrder = 'dmy' | 'mdy';

/**
 * A numeric date whose first two parts are both 12 or less can be read either
 * way round, and the two readings are different days. `04-07-2026` is 4 July to
 * most of the world and 7 April to an American lab.
 *
 * The app cannot silently pick one. It guesses, then says it guessed.
 */
export function isAmbiguousOrder(raw: string): boolean {
  const m = /^(\d{1,2})[/\-.](\d{1,2})[/\-.](\d{2,4})$/.exec(raw.trim());
  if (!m) return false; // textual months and ISO are unambiguous by construction
  const [, a, b] = m;
  const first = Number(a);
  const second = Number(b);
  return first >= 1 && first <= 12 && second >= 1 && second <= 12 && first !== second;
}

/**
 * Work out which order a page uses, from the dates on it that cannot be read two
 * ways.
 *
 * A page that also contains `28/11/2024` has settled the question for every other
 * date on it: 28 cannot be a month. This resolves most ambiguity without asking
 * anyone anything, and it is why the whole page is inspected rather than each
 * date in isolation.
 */
export function inferDateOrder(text: string): DateOrder | null {
  let dmy = 0;
  let mdy = 0;

  const re = /\b(\d{1,2})[/\-.](\d{1,2})[/\-.](\d{2,4})\b/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    const first = Number(m[1]);
    const second = Number(m[2]);
    if (first > 12 && second <= 12) dmy++;
    else if (second > 12 && first <= 12) mdy++;
  }

  if (dmy === 0 && mdy === 0) return null;
  if (dmy === mdy) return null; // genuinely mixed; do not guess
  return dmy > mdy ? 'dmy' : 'mdy';
}

/** Read a raw numeric date the other way round, for offering as the alternative. */
export function swapOrder(raw: string, today = new Date()): string | null {
  const m = /^(\d{1,2})[/\-.](\d{1,2})[/\-.](\d{2,4})$/.exec(raw.trim());
  if (!m) return null;
  const [, a, b, c] = m;
  if (a === undefined || b === undefined || c === undefined) return null;
  // Deliberately reversed: month first, day second.
  return iso(expandYear(+c, today), +a, +b);
}

/** Display form. The UI always shows DD/MM/YYYY; only the disk and DB are ISO. */
export function formatDmy(isoDate: string): string {
  const [y, m, d] = isoDate.split('-');
  if (y === '0000') return 'unknown';
  if (d === '00') return `${m}/${y}`;
  return `${d}/${m}/${y}`;
}

/** Parse what the user types in the DMY-locked date field. Accepts the
 *  separators people actually use, and bare digits (14032026). */
export function parseDmyInput(input: string, today = new Date()): string | null {
  const s = input.trim();

  const m =
    /^(\d{2})(\d{2})(\d{4})$/.exec(s) ??
    /^(\d{2})(\d{2})(\d{2})$/.exec(s) ??
    /^(\d{1,2})[/\-. ](\d{1,2})[/\-. ](\d{2,4})$/.exec(s);
  if (!m) return null;

  const [, d, mo, y] = m;
  if (d === undefined || mo === undefined || y === undefined) return null;

  return iso(expandYear(+y, today), +mo, +d);
}
