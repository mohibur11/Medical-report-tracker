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
  score: number;
  /** Human-readable justification, shown in the grid tooltip. */
  reason: string;
}

/** Labels that mark the date a report is ABOUT. Ordered strongest first. */
const POSITIVE_ANCHORS: Array<[RegExp, number, string]> = [
  [/\b(sample\s*(collected|collection)|collected\s*on|collection\s*date|drawn)\b/i, 100, 'sample collected'],
  [/\b(specimen|sampling)\s*(date|on)?\b/i, 90, 'specimen date'],
  [/\b(received|recd)\b/i, 60, 'received'],
  [/\b(report(ed)?\s*(on|date)?|result\s*date)\b/i, 45, 'reported'],
  [/\b(registered|registration|admitted|visit|consultation)\b/i, 35, 'registered/visit'],
  [/\b(date)\b/i, 15, 'generic "date"'],
];

/** Labels that mark a date the report is NOT about. A wrong pick here is the
 *  single most damaging silent failure in the app. */
const NEGATIVE_ANCHORS: Array<[RegExp, number, string]> = [
  [/\b(date\s*of\s*birth|d\.?o\.?b\.?|born)\b/i, -400, 'date of birth'],
  [/\b(age)\b/i, -200, 'age field'],
  [/\b(printed?\s*(on|date)?|print\s*time)\b/i, -120, 'printed on'],
  [/\b(expiry|expires|valid\s*(till|until)|due)\b/i, -150, 'expiry'],
  [/\b(next\s*(visit|appointment|follow\s*up))\b/i, -150, 'next appointment'],
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

function anchorFor(text: string, index: number): { label: string | null; weight: number; why: string } {
  const window = text.slice(Math.max(0, index - ANCHOR_WINDOW), index);

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
  /** Hard-reject any candidate equal to a known patient DOB. The strongest
   *  available signal, and it costs one comparison. */
  patientDob?: string | null;
  /** Injected so ranking is deterministic in tests. */
  today?: Date;
}

/**
 * Find every date-shaped string and rank it by how likely it is to be the date
 * the document is ABOUT. Returns highest score first; an empty array means the
 * user must type a date, which for handwritten prescriptions is the normal path.
 */
export function rankDateCandidates(text: string, opts: RankOptions = {}): DateCandidate[] {
  const today = opts.today ?? new Date();
  const todayIso = today.toISOString().slice(0, 10);
  const seen = new Map<string, DateCandidate>();

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
      if (opts.patientDob && value === opts.patientDob) continue;

      const { label, weight, why } = anchorFor(text, m.index);

      // A date appearing under several labels keeps its best evidence.
      const prev = seen.get(value);
      const score = weight + positionBonus(m.index, text.length);
      if (prev && prev.score >= score) continue;

      seen.set(value, {
        iso: value,
        raw: m[0],
        index: m.index,
        anchor: label,
        score,
        reason: why,
      });
    }
  }

  return [...seen.values()].sort((a, b) => b.score - a.score || a.index - b.index);
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
