import { useState } from 'react';

import {
  exportPdf,
  revealInExplorer,
  type Category,
  type ExportResult,
  type Patient,
  type Preset,
} from '../lib/ipc.ts';

const EMAIL_LIMIT = 25 * 1024 * 1024;

const PRESETS: Array<{ id: Preset; label: string; hint: string }> = [
  { id: 'standard', label: 'Standard', hint: 'Good on a phone. Send as a document, not a photo.' },
  { id: 'emailSafe', label: 'Email-safe', hint: 'Grayscale, ~150 DPI. Printed reports stay legible.' },
  { id: 'original', label: 'Original', hint: 'No recompression. For printing.' },
];

/**
 * The export builder — the screen the whole app exists to serve.
 *
 * A doctor receives one file and scrolls it top to bottom, so the defaults matter
 * more than the options: every page is A4 portrait, every page carries a header
 * band, and records are ordered by date.
 */
export function ExportPanel({
  patients,
  years,
  categories,
  vaultPath,
  documentCount,
}: {
  patients: Patient[];
  years: string[];
  categories: Category[];
  vaultPath: string;
  documentCount: number;
}) {
  const [patientIds, setPatientIds] = useState<string[]>([]);
  const [selectedYears, setSelectedYears] = useState<string[]>([]);
  const [categoryIds, setCategoryIds] = useState<string[]>([]);
  const [preset, setPreset] = useState<Preset>('standard');
  const [split, setSplit] = useState(true);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ExportResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  function toggle(list: string[], value: string, set: (v: string[]) => void) {
    set(list.includes(value) ? list.filter((v) => v !== value) : [...list, value]);
  }

  const who =
    patientIds.length === 1
      ? (patients.find((p) => p.id === patientIds[0])?.displayName ?? 'Selected')
      : patientIds.length > 1
        ? `${patientIds.length} patients`
        : 'All patients';

  // Name the file after what it contains — "Rahim Uddin — Thyroid" is what makes
  // a file handed to a doctor self-explanatory before it is even opened.
  const what =
    categoryIds.length === 1
      ? (categories.find((c) => c.id === categoryIds[0])?.name ?? null)
      : categoryIds.length > 1
        ? `${categoryIds.length} categories`
        : null;

  const scope = what ? `${who} — ${what}` : who;

  async function run() {
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      setResult(
        await exportPdf({
          patientIds,
          years: selectedYears,
          categoryIds,
          docTypes: [],
          preset,
          maxBytes: split ? EMAIL_LIMIT : null,
          outDir: `${vaultPath}\\Exports`,
          baseName: `${scope} — Medical History`,
        }),
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const chip = (active: boolean) =>
    `rounded border px-2 py-0.5 text-xs ${
      active
        ? 'border-sky-500 bg-sky-50 text-sky-800 dark:bg-sky-950 dark:text-sky-200'
        : 'border-slate-300 text-slate-600 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-300 dark:hover:bg-slate-800'
    }`;

  return (
    <div className="border-b border-slate-200 px-6 py-4 dark:border-slate-800">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-slate-500">
        Export one PDF for a doctor
      </h2>

      <div className="mt-3 space-y-2">
        <Field label="Patients">
          <button type="button" className={chip(patientIds.length === 0)} onClick={() => setPatientIds([])}>
            Everyone
          </button>
          {patients.map((p) => (
            <button
              key={p.id}
              type="button"
              className={chip(patientIds.includes(p.id))}
              onClick={() => toggle(patientIds, p.id, setPatientIds)}
            >
              {p.displayName}
            </button>
          ))}
        </Field>

        {years.length > 0 && (
          <Field label="Years">
            <button type="button" className={chip(selectedYears.length === 0)} onClick={() => setSelectedYears([])}>
              All years
            </button>
            {years.map((y) => (
              <button
                key={y}
                type="button"
                className={chip(selectedYears.includes(y))}
                onClick={() => toggle(selectedYears, y, setSelectedYears)}
              >
                {y}
              </button>
            ))}
          </Field>
        )}

        {categories.length > 0 && (
          <Field label="Category">
            <button type="button" className={chip(categoryIds.length === 0)} onClick={() => setCategoryIds([])}>
              Any
            </button>
            {categories.map((c) => (
              <button
                key={c.id}
                type="button"
                className={chip(categoryIds.includes(c.id))}
                onClick={() => toggle(categoryIds, c.id, setCategoryIds)}
              >
                {c.name}
                <span className="ml-1 opacity-60">{c.documentCount}</span>
              </button>
            ))}
          </Field>
        )}

        <Field label="Quality">
          {PRESETS.map((p) => (
            <button key={p.id} type="button" className={chip(preset === p.id)} onClick={() => setPreset(p.id)}>
              {p.label}
            </button>
          ))}
        </Field>

        <p className="pl-20 text-xs text-slate-500 dark:text-slate-400">
          {PRESETS.find((p) => p.id === preset)?.hint}
        </p>

        <Field label="If too big">
          <label className="flex items-center gap-1.5 text-xs text-slate-600 dark:text-slate-300">
            <input type="checkbox" checked={split} onChange={(e) => setSplit(e.target.checked)} />
            Split into parts under 25 MB, never mid-report
          </label>
        </Field>
      </div>

      <div className="mt-3 flex items-center gap-3">
        <button
          type="button"
          disabled={busy || documentCount === 0}
          onClick={() => void run()}
          className="rounded bg-sky-600 px-4 py-1.5 text-sm font-medium text-white disabled:cursor-not-allowed disabled:bg-slate-300 dark:disabled:bg-slate-700"
        >
          {busy ? 'Building…' : 'Create PDF'}
        </button>
        {documentCount === 0 && (
          <span className="text-xs text-slate-500">File some documents first.</span>
        )}
      </div>

      {error && (
        <pre className="selectable mt-3 whitespace-pre-wrap rounded border border-red-300 bg-red-50 p-2 font-mono text-xs text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
          {error}
        </pre>
      )}

      {result && (
        <div className="mt-3 rounded border border-emerald-300 bg-emerald-50 p-3 text-xs dark:border-emerald-900 dark:bg-emerald-950">
          <p className="font-medium text-emerald-900 dark:text-emerald-200">
            {result.totalDocuments} document{result.totalDocuments === 1 ? '' : 's'} in{' '}
            {result.parts.length} file{result.parts.length === 1 ? '' : 's'}
          </p>
          <ul className="mt-1.5 space-y-1">
            {result.parts.map((p) => (
              <li key={p.path} className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => void revealInExplorer(p.path)}
                  className="underline decoration-dotted hover:decoration-solid"
                >
                  {p.path.split('\\').pop()}
                </button>
                <span className="text-emerald-800 dark:text-emerald-300">
                  {p.pages} pages · {(p.bytes / 1024 / 1024).toFixed(1)} MB
                  {p.bytes > EMAIL_LIMIT && (
                    <span className="ml-1 text-amber-700 dark:text-amber-400">too big to email</span>
                  )}
                </span>
              </li>
            ))}
          </ul>

          {result.undated > 0 && (
            <p className="mt-2 text-amber-700 dark:text-amber-400">
              {result.undated} document{result.undated === 1 ? ' has' : 's have'} no date and
              appear{result.undated === 1 ? 's' : ''} at the end.
            </p>
          )}
          {result.missing.length > 0 && (
            <div className="mt-2 text-amber-700 dark:text-amber-400">
              <p>Left out because the file could not be read:</p>
              <ul className="ml-4 list-disc">
                {result.missing.map((m) => (
                  <li key={m}>{m}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <span className="w-16 shrink-0 text-xs text-slate-500 dark:text-slate-400">{label}</span>
      {children}
    </div>
  );
}
