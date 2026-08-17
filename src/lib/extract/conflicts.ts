/**
 * Things the app noticed but must not decide on its own.
 *
 * Every rule here exists because guessing wrong would be invisible. A file saved
 * under the wrong date does not throw; it just sits in the wrong place in a
 * history handed to a doctor. So where the evidence is genuinely ambiguous, or
 * where the document contradicts what the app already believes, it stops and asks
 * rather than picking the more likely option and moving on.
 *
 * Two severities:
 *   - `ask`      the app has a usable answer but is not sure. Filing is allowed;
 *                the question is shown with the alternative one click away.
 *   - `blocking` the value cannot be right. Filing is refused until it is fixed,
 *                because saving it would create a record that is wrong on its face.
 */

import {
  formatDmy,
  inferDateOrder,
  isAmbiguousOrder,
  swapOrder,
  type DateCandidate,
} from './dates.ts';
import { isValidDocDate } from '../naming/sanitize.ts';

export type ConflictKind =
  | 'ambiguous-date-order'
  | 'dob-mismatch'
  | 'date-before-birth'
  | 'future-date'
  | 'invalid-date'
  | 'close-call';

export interface ConflictOption {
  label: string;
  /** ISO date to apply if the user picks this. */
  value: string;
}

export interface Conflict {
  kind: ConflictKind;
  severity: 'ask' | 'blocking';
  /** Written to be read by someone who is not a programmer. */
  message: string;
  options?: ConflictOption[];
}

export interface ConflictInput {
  /** The date currently in the field, ISO, or null if empty. */
  chosen: string | null;
  /** The raw string the chosen date was read from, if it came from the page. */
  chosenRaw?: string | null;
  /** Full recognised text of the page, used to settle date-order ambiguity. */
  text?: string;
  /** Ranked candidates, so a near-tie can be surfaced. */
  candidates?: DateCandidate[];
  /** The selected patient's date of birth, ISO, if known. */
  patientDob?: string | null;
  patientName?: string;
  today?: Date;
}

/** Scores this close mean the ranking did not really decide anything. */
const CLOSE_CALL_MARGIN = 20;

export function detectConflicts(input: ConflictInput): Conflict[] {
  const today = input.today ?? new Date();
  const todayIso = today.toISOString().slice(0, 10);
  const out: Conflict[] = [];
  const { chosen } = input;

  if (!chosen) return out;

  if (!isValidDocDate(chosen)) {
    out.push({
      kind: 'invalid-date',
      severity: 'blocking',
      message: 'That is not a real date. Please correct it before filing.',
    });
    return out; // nothing else can be judged against a date that is not one
  }

  if (chosen > todayIso) {
    out.push({
      kind: 'future-date',
      severity: 'blocking',
      message: `${formatDmy(chosen)} is in the future. Please correct the date before filing.`,
    });
  }

  if (input.patientDob && isValidDocDate(input.patientDob) && chosen < input.patientDob) {
    const who = input.patientName ? `${input.patientName}'s` : 'the patient’s';
    out.push({
      kind: 'date-before-birth',
      severity: 'blocking',
      message:
        `This report is dated ${formatDmy(chosen)}, before ${who} date of birth ` +
        `(${formatDmy(input.patientDob)}). One of the two is wrong — correct the date, ` +
        'or fix the date of birth on the patient, before filing.',
    });
  }

  // A numeric date that could be read either way round. Only ask when the page
  // itself does not already settle it.
  if (input.chosenRaw && isAmbiguousOrder(input.chosenRaw)) {
    const pageOrder = input.text ? inferDateOrder(input.text) : null;
    if (!pageOrder) {
      const other = swapOrder(input.chosenRaw, today);
      if (other && other !== chosen && isValidDocDate(other) && other <= todayIso) {
        out.push({
          kind: 'ambiguous-date-order',
          severity: 'ask',
          message:
            `“${input.chosenRaw}” could mean either date. Nothing else on the page says ` +
            'which way round this lab writes them. Which is right?',
          options: [
            { label: `${formatDmy(chosen)} (day first)`, value: chosen },
            { label: `${formatDmy(other)} (month first)`, value: other },
          ],
        });
      }
    }
  }

  // The page states a date of birth that disagrees with the patient profile.
  // Left alone this quietly disables the strongest protection against filing a
  // report under a birth date.
  const dobOnPage = input.candidates?.find((c) => c.anchor === 'date of birth');
  if (dobOnPage && input.patientDob && dobOnPage.iso !== input.patientDob) {
    out.push({
      kind: 'dob-mismatch',
      severity: 'ask',
      message:
        `This document gives a date of birth of ${formatDmy(dobOnPage.iso)}, but the patient ` +
        `is recorded as ${formatDmy(input.patientDob)}. One of them is wrong. Check the ` +
        'document, and correct the patient if needed — this date is used to stop reports ' +
        'being filed under a birth date.',
    });
  }

  // Two candidates the ranking could barely separate.
  const [first, second] = input.candidates ?? [];
  if (
    first &&
    second &&
    first.iso === chosen &&
    first.score - second.score <= CLOSE_CALL_MARGIN
  ) {
    out.push({
      kind: 'close-call',
      severity: 'ask',
      message:
        `${formatDmy(first.iso)} and ${formatDmy(second.iso)} were almost equally likely. ` +
        'Please check which one this report is actually dated.',
      options: [
        { label: `${formatDmy(first.iso)} (${first.anchor ?? 'no label'})`, value: first.iso },
        { label: `${formatDmy(second.iso)} (${second.anchor ?? 'no label'})`, value: second.iso },
      ],
    });
  }

  return out;
}

/** Filing is refused while any blocking conflict stands. */
export const isBlocked = (conflicts: Conflict[]): boolean =>
  conflicts.some((c) => c.severity === 'blocking');
