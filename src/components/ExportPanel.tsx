import { useEffect, useState } from 'react';

import {
  deleteExportPreset,
  exportPdf,
  exportToFolder,
  listExportPresets,
  revealInExplorer,
  saveExportPreset,
  useExportPreset,
  type Category,
  type ExportPreset,
  type FolderExport,
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
  const [presets, setPresets] = useState<ExportPreset[]>([]);
  const [applied, setApplied] = useState<string | null>(null);
  const [folder, setFolder] = useState<FolderExport | null>(null);

  const loadPresets = () => listExportPresets().then(setPresets, () => {});
  useEffect(() => void loadPresets(), []);

  /**
   * Apply a saved filter.
   *
   * Ids that no longer exist are dropped rather than carried: a preset naming a
   * patient or category that has since been removed should still open, minus
   * that part, instead of quietly filtering everything out.
   */
  function apply(p: ExportPreset) {
    setPatientIds(p.patientIds.filter((id) => patients.some((x) => x.id === id)));
    setSelectedYears(p.years.filter((y) => years.includes(y)));
    setCategoryIds(p.categoryIds.filter((id) => categories.some((c) => c.id === id)));
    setPreset(p.preset);
    setSplit(p.maxBytes !== null);
    setApplied(p.id);
    setResult(null);
    void useExportPreset(p.id).then(loadPresets, () => {});
  }

  async function saveCurrent() {
    const name = window.prompt('Save these filters as', applied
      ? (presets.find((p) => p.id === applied)?.name ?? scope)
      : scope);
    if (!name) return;
    try {
      const saved = await saveExportPreset(name, {
        id: '',
        name,
        patientIds,
        years: selectedYears,
        categoryIds,
        docTypes: [],
        preset,
        maxBytes: split ? EMAIL_LIMIT : null,
      });
      setApplied(saved.id);
      await loadPresets();
    } catch (e) {
      setError(String(e));
    }
  }

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

  /**
   * The escape hatch. Kept next to the main button rather than hidden behind a
   * failure, because "give me the files" is a legitimate thing to want on its own
   * — for a USB stick, or for someone who would rather print them individually.
   */
  async function runFolder() {
    setBusy(true);
    setError(null);
    setResult(null);
    setFolder(null);
    try {
      setFolder(
        await exportToFolder({
          patientIds,
          years: selectedYears,
          categoryIds,
          docTypes: [],
          preset,
          maxBytes: null,
          outDir: `${vaultPath}\Exports`,
          baseName: `${scope} — Files`,
        }),
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function run() {
    setBusy(true);
    setError(null);
    setResult(null);
    setFolder(null);
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

      {presets.length > 0 && (
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <span className="w-20 shrink-0 text-xs text-slate-500 dark:text-slate-400">Saved</span>
          {presets.map((p) => (
            <span key={p.id} className="inline-flex items-center">
              <button
                type="button"
                onClick={() => apply(p)}
                className={`rounded-l border border-r-0 px-2 py-0.5 text-xs ${
                  applied === p.id
                    ? 'border-sky-500 bg-sky-50 text-sky-800 dark:bg-sky-950 dark:text-sky-200'
                    : 'border-slate-300 text-slate-600 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-300 dark:hover:bg-slate-800'
                }`}
              >
                {p.name}
              </button>
              <button
                type="button"
                title={`Delete the preset ${p.name} — the documents are untouched`}
                onClick={() => {
                  if (!window.confirm(`Delete the preset '${p.name}'? The documents are not affected.`)) return;
                  deleteExportPreset(p.id).then(() => {
                    if (applied === p.id) setApplied(null);
                    return loadPresets();
                  }, (e: unknown) => setError(String(e)));
                }}
                className="rounded-r border border-slate-300 px-1.5 py-0.5 text-xs text-slate-400 hover:text-red-600 dark:border-slate-600"
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}

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
        {/* The same doctor asks for the same slice every time; typing it out
            again each visit is friction on the one action that matters. */}
        <button
          type="button"
          disabled={busy || documentCount === 0}
          onClick={() => void runFolder()}
          title="Copy the same documents out as separate numbered files instead of one PDF"
          className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50 disabled:opacity-40 dark:border-slate-600 dark:hover:bg-slate-800"
        >
          Export as files
        </button>
        <button
          type="button"
          onClick={() => void saveCurrent()}
          className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-800"
        >
          Save these filters
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

      {folder && (
        <div className="mt-3 rounded border border-emerald-300 bg-emerald-50 p-3 text-xs dark:border-emerald-900 dark:bg-emerald-950">
          <p className="font-medium text-emerald-900 dark:text-emerald-200">
            {folder.copied} file{folder.copied === 1 ? '' : 's'} copied, numbered in date order.
          </p>
          <button
            type="button"
            onClick={() => void revealInExplorer(folder.outDir)}
            className="selectable mt-1 font-mono underline decoration-dotted hover:decoration-solid"
          >
            {folder.outDir}
          </button>
          {folder.missing.length > 0 && (
            <ul className="mt-1.5 space-y-0.5 text-amber-700 dark:text-amber-300">
              {folder.missing.map((m) => (
                <li key={m}>{m}</li>
              ))}
            </ul>
          )}
        </div>
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
