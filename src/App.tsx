import { useCallback, useEffect, useRef, useState } from 'react';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open } from '@tauri-apps/plugin-dialog';

import { BulkBar } from './components/BulkBar.tsx';
import { CategoryManager, CategoryPicker } from './components/CategoryPicker.tsx';
import { EditRow } from './components/EditRow.tsx';
import { ExportPanel } from './components/ExportPanel.tsx';
import { IDLE_LOCK_MS, LockScreen, LockSettings } from './components/Lock.tsx';
import { PatientsPanel } from './components/PatientsPanel.tsx';
import { ReviewRow, type RowHandle } from './components/ReviewRow.tsx';
import { Thumb } from './components/Thumb.tsx';
import { VaultTools } from './components/VaultTools.tsx';
import { fileEach, summarize } from './lib/bulk.ts';
import { formatDmy } from './lib/extract/dates.ts';
import * as selection from './lib/selection.ts';
import {
  createPatient,
  documentTags,
  health,
  importFiles,
  listCategories,
  listDocuments,
  listPatients,
  listStaged,
  listYears,
  lockState,
  type LockState,
  setDocumentCategories,
  tagDocuments,
  trashDocument,
  type Category,
  type DbHealth,
  type DocumentRow,
  type IngestItem,
  type Patient,
} from './lib/ipc.ts';

/**
 * Phase 1 inbox and review queue.
 *
 * Files are staged on drop — hashed, typed by magic bytes, rotated upright,
 * downscaled — and nothing reaches the vault until a patient and a date are
 * confirmed here, because the canonical filename contains both.
 */
export default function App() {
  const [sys, setSys] = useState<DbHealth | null>(null);
  const [patients, setPatients] = useState<Patient[]>([]);
  const [items, setItems] = useState<IngestItem[]>([]);
  const [docs, setDocs] = useState<DocumentRow[]>([]);
  const [years, setYears] = useState<string[]>([]);
  const [categories, setCategories] = useState<Category[]>([]);
  /** documentId -> categoryIds, fetched in one call rather than per row. */
  const [tags, setTags] = useState<Record<string, string[]>>({});
  const [filed, setFiled] = useState<string[]>([]);
  const [view, setView] = useState<'inbox' | 'library'>('inbox');
  /** Opening with nothing to review should land on the library, not an empty
   *  inbox. Decided once on first load so it never yanks the view mid-session. */
  const [viewChosen, setViewChosen] = useState(false);
  const [lastPatientId, setLastPatientId] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lock, setLock] = useState<LockState>({ enabled: false, email: '' });
  const [locked, setLocked] = useState(false);
  /** Which library row is open for editing, if any. */
  const [editingId, setEditingId] = useState<string | null>(null);
  /** Review-queue multi-select, for the bulk bar. */
  const [sel, setSel] = useState(selection.EMPTY);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkStatus, setBulkStatus] = useState<string | null>(null);

  /** Live handles onto the review rows, so the bulk bar can drive them. */
  const rows = useRef(new Map<string, RowHandle>());
  const registerRow = useCallback((id: string, handle: RowHandle | null) => {
    if (handle) rows.current.set(id, handle);
    else rows.current.delete(id);
  }, []);
  /** Set while a bulk file runs, so 200 commits do not trigger 200 refreshes. */
  const bulkRunning = useRef(false);

  const refreshLock = useCallback(() => {
    lockState().then((s) => {
      setLock(s);
      // Only ever lock as a result of loading state on startup; enabling the lock
      // from settings should not throw the user out of the session they are in.
      setLocked((was) => was || (s.enabled && !hasUnlockedThisSession.current));
    }, () => {});
  }, []);

  const hasUnlockedThisSession = useRef(false);

  useEffect(refreshLock, [refreshLock]);

  // Idle auto-lock. Only meaningful when a password is actually set.
  useEffect(() => {
    if (!lock.enabled || locked) return;
    let timer: number;
    const reset = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        hasUnlockedThisSession.current = false;
        setLocked(true);
      }, IDLE_LOCK_MS);
    };
    const events = ['mousemove', 'keydown', 'mousedown', 'wheel'] as const;
    events.forEach((e) => window.addEventListener(e, reset, { passive: true }));
    reset();
    return () => {
      window.clearTimeout(timer);
      events.forEach((e) => window.removeEventListener(e, reset));
    };
  }, [lock.enabled, locked]);

  const refresh = useCallback(() => {
    const fail = (e: unknown) => setError(String(e));
    health().then(setSys, fail);
    listPatients().then(setPatients, fail);
    listYears().then(setYears, fail);
    listCategories().then(setCategories, fail);

    // Fetched together because the opening view depends on both: files waiting to
    // be reviewed outrank a library that is already filed.
    Promise.all([listDocuments(), listStaged()]).then(([rows, staged]) => {
      setDocs(rows);
      // The queue is reloaded rather than remembered, so committing a row cannot
      // leave the screen disagreeing with the vault. Rows that were rejected on
      // the way in are not stored as reviewable, so they are carried over here
      // instead of vanishing at the first refresh.
      setItems((prev) => [
        ...staged,
        ...prev.filter((i) => i.status === 'failed' || i.status === 'duplicate'),
      ]);
      setViewChosen((chosen) => {
        if (!chosen && staged.length === 0 && rows.length > 0) setView('library');
        return true;
      });
      documentTags(rows.map((r) => r.id)).then((pairs) => {
        const map: Record<string, string[]> = {};
        for (const [docId, catId] of pairs) (map[docId] ??= []).push(catId);
        setTags(map);
      }, fail);
    }, fail);
  }, []);

  async function retag(documentId: string, categoryIds: string[]) {
    // Optimistic: the popover stays responsive while the write lands.
    setTags((prev) => ({ ...prev, [documentId]: categoryIds }));
    try {
      await setDocumentCategories(documentId, categoryIds);
      listCategories().then(setCategories, () => {});
    } catch (e) {
      setError(String(e));
      refresh();
    }
  }

  useEffect(refresh, [refresh]);

  const ingest = useCallback(async (paths: string[]) => {
    if (paths.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      const staged = await importFiles(paths);
      setItems((prev) => [...staged, ...prev.filter((i) => !staged.some((s) => s.id === i.id))]);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    // Registering this can fail — Tauri denies core APIs unless a capability
    // grants them, and the failure is a rejected promise, not an exception. Left
    // unhandled it produces an app that looks fine and silently ignores every
    // dropped file, which is worse than an app that plainly does not start.
    const pending = getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === 'over') setDragging(true);
        else if (event.payload.type === 'leave') setDragging(false);
        else if (event.payload.type === 'drop') {
          setDragging(false);
          void ingest(event.payload.paths);
        }
      })
      .catch((e: unknown) => {
        setError(
          `Drag and drop is unavailable: ${String(e)}. Use the Add files button instead.`,
        );
        return () => {};
      });

    return () => void pending.then((un) => un());
  }, [ingest]);

  /** The door that always works, whatever drag-and-drop is doing. */
  async function pickFiles() {
    try {
      const picked = await open({
        multiple: true,
        title: 'Add scans or PDFs',
        filters: [{ name: 'Scans and PDFs', extensions: ['jpg', 'jpeg', 'png', 'pdf', 'tif', 'tiff', 'webp'] }],
      });
      if (!picked) return;
      await ingest(Array.isArray(picked) ? picked : [picked]);
    } catch (e) {
      setError(String(e));
    }
  }

  async function addPatient() {
    const name = window.prompt('Patient name');
    if (!name) return;
    try {
      const p = await createPatient(name);
      setPatients((prev) => [...prev, p].sort((a, b) => a.displayName.localeCompare(b.displayName)));
      setLastPatientId(p.id);
    } catch (e) {
      setError(String(e));
    }
  }

  function onCommitted(id: string, fileName: string, patientId: string) {
    setItems((prev) => prev.filter((i) => i.id !== id));
    setFiled((prev) => [fileName, ...prev]);
    // Carry the patient to the next row. Re-picking per file does not survive a
    // backlog import.
    setLastPatientId(patientId);
    if (!bulkRunning.current) refresh();
  }

  const pending = items.filter((i) => i.status === 'needs_date' || i.status === 'pending');
  const rejected = items.filter((i) => i.status === 'failed' || i.status === 'duplicate');
  const pendingIds = pending.map((i) => i.id);
  const pendingKey = pendingIds.join(',');

  // Filed rows leave the queue; a selection still holding them would make the
  // count lie and aim the next bulk action at nothing.
  useEffect(() => {
    setSel((prev) => selection.prune(prev, pendingKey ? pendingKey.split(',') : []));
  }, [pendingKey]);

  const applyToSelected = (fn: (api: RowHandle['current']) => void) => {
    for (const id of selection.ordered(sel, pendingIds)) {
      const handle = rows.current.get(id);
      if (handle) fn(handle.current);
    }
  };

  /**
   * File every selected row, one at a time.
   *
   * Sequential on purpose: each commit reserves a collision suffix and moves a
   * file through the journal, and firing three hundred of those at once buys
   * nothing but contention. Rows that are not ready are skipped and named, never
   * filed with a guess.
   */
  async function fileSelected(categoryIds: string[]) {
    const ids = selection.ordered(sel, pendingIds);
    const nameOf = new Map(pending.map((i) => [i.id, i.fileName]));
    setBulkBusy(true);
    setBulkStatus(null);
    setError(null);
    bulkRunning.current = true;

    let outcome = { filed: [] as string[], skipped: [] as { name: string; reason: string }[] };
    try {
      outcome = await fileEach(
        ids,
        (id) => {
          const handle = rows.current.get(id);
          return handle ? { name: nameOf.get(id) ?? id, target: handle.current } : null;
        },
        (done, total) => setBulkStatus(`filing ${done} of ${total}…`),
      );

      // Tagging waits until the documents exist; an ingest item has no row to tag.
      for (const categoryId of categoryIds) {
        if (outcome.filed.length > 0) await tagDocuments(outcome.filed, categoryId);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      bulkRunning.current = false;
      setBulkBusy(false);
      refresh();
    }

    setBulkStatus(summarize(outcome));
  }

  if (locked) {
    return (
      <LockScreen
        email={lock.email}
        onUnlocked={() => {
          hasUnlockedThisSession.current = true;
          setLocked(false);
          refresh();
        }}
      />
    );
  }

  return (
    <div className="flex h-full flex-col bg-white text-slate-900 dark:bg-slate-950 dark:text-slate-100">
      <header className="border-b border-slate-200 px-6 py-3 dark:border-slate-800">
        <div className="flex items-center gap-3">
          <h1 className="text-sm font-semibold tracking-tight">Medicine Report Tracker</h1>
          <nav className="flex gap-1">
            {(['inbox', 'library'] as const).map((v) => (
              <button
                key={v}
                type="button"
                onClick={() => setView(v)}
                className={`rounded px-2 py-0.5 text-xs capitalize ${
                  view === v
                    ? 'bg-slate-900 text-white dark:bg-slate-100 dark:text-slate-900'
                    : 'text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800'
                }`}
              >
                {v}
                {v === 'inbox' && pending.length > 0 && ` (${pending.length})`}
                {v === 'library' && docs.length > 0 && ` (${docs.length})`}
              </button>
            ))}
          </nav>
          <button
            type="button"
            onClick={() => void pickFiles()}
            className="rounded bg-sky-600 px-2 py-0.5 text-xs font-medium text-white hover:bg-sky-700"
          >
            Add files…
          </button>
          <button
            type="button"
            onClick={() => void addPatient()}
            className="rounded border border-slate-300 px-2 py-0.5 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-800"
          >
            + Patient
          </button>
        </div>
        <p className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
          {pending.length} to review · {docs.length} filed · {patients.length} patient
          {patients.length === 1 ? '' : 's'}
          <span className="ml-3 font-mono text-slate-400 dark:text-slate-500">
            {sys
              ? `SQLite ${sys.sqliteVersion} · schema ${sys.schemaVersion} · ${sys.journalMode}`
              : 'backend: not connected'}
          </span>
          {sys && sys.pendingJournalOps > 0 && (
            <span className="ml-2 font-medium text-amber-600">
              {sys.pendingJournalOps} pending file ops
            </span>
          )}
        </p>
      </header>

      {error && (
        <div className="border-b border-red-300 bg-red-50 px-6 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
          <span className="selectable font-mono">{error}</span>
        </div>
      )}

      <main className="relative flex-1 overflow-y-auto overflow-x-hidden">
        {view === 'library' ? (
          <>
            <LockSettings state={lock} onChanged={refreshLock} />
            <VaultTools onRepaired={refresh} />
            {patients.length > 0 && <PatientsPanel patients={patients} onChanged={refresh} />}
            <div className="border-b border-slate-200 px-6 py-3 dark:border-slate-800">
              <CategoryManager categories={categories} onChanged={refresh} />
            </div>
            <ExportPanel
              patients={patients}
              years={years}
              categories={categories}
              vaultPath={sys?.vaultPath ?? ''}
              documentCount={docs.length}
            />
            {docs.length === 0 ? (
              <p className="px-6 py-8 text-center text-sm text-slate-500 dark:text-slate-400">
                Nothing filed yet. Review the inbox first.
              </p>
            ) : (
              <ul className="divide-y divide-slate-100 dark:divide-slate-800">
                {docs.map((d) =>
                  editingId === d.id ? (
                    <EditRow
                      key={d.id}
                      doc={d}
                      patients={patients}
                      onSaved={() => {
                        setEditingId(null);
                        refresh();
                      }}
                      onCancel={() => setEditingId(null)}
                    />
                  ) : (
                  <li key={d.id} className="px-6 py-2">
                    <div className="flex items-baseline gap-3">
                      <span className="w-24 shrink-0 font-mono text-xs text-slate-500 dark:text-slate-400">
                        {formatDmy(d.docDate)}
                      </span>
                      <span className="min-w-0 flex-1 truncate text-sm">{d.title}</span>
                    </div>
                    {/* Patient, type and tags share the second line. Several
                        categories per document is normal, and none of it should
                        squeeze the title. */}
                    <div className="mt-1 flex flex-wrap items-center gap-2 pl-28 text-xs text-slate-500 dark:text-slate-400">
                      <span>{d.patient}</span>
                      <span className="text-slate-400">
                        {d.fileKind.toUpperCase()}
                        {d.pageCount > 1 && ` · ${d.pageCount}p`}
                      </span>
                      <CategoryPicker
                        categories={categories}
                        selected={tags[d.id] ?? []}
                        onChange={(ids) => void retag(d.id, ids)}
                      />
                      {d.missing && (
                        <span className="rounded bg-amber-100 px-1.5 py-0.5 text-[11px] font-medium text-amber-800 dark:bg-amber-900 dark:text-amber-200">
                          file missing
                        </span>
                      )}
                      <button
                        type="button"
                        title="Change the date, title, patient or type"
                        onClick={() => setEditingId(d.id)}
                        className="rounded border border-slate-300 px-1.5 py-0.5 hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-800"
                      >
                        Edit
                      </button>
                      <button
                        type="button"
                        title="Move to the vault's Trash folder — the file is not deleted"
                        onClick={() => {
                          if (!window.confirm(`Move '${d.title}' to Trash?

The file is not deleted — it moves to the Trash folder inside your vault, and you can put it back from there.`)) return;
                          trashDocument(d.id).then(refresh, (e: unknown) => setError(String(e)));
                        }}
                        className="rounded border border-slate-300 px-1.5 py-0.5 text-red-600 hover:bg-red-50 dark:border-slate-600 dark:hover:bg-red-950"
                      >
                        Delete
                      </button>
                    </div>
                  </li>
                  ),
                )}
              </ul>
            )}
          </>
        ) : items.length === 0 && filed.length === 0 ? (
          <div className="flex h-full items-center justify-center">
            <div className="text-center">
              <p className="text-sm text-slate-500 dark:text-slate-400">
                Drop scans or PDFs anywhere in this window, or use Add files.
              </p>
              <p className="mt-1 text-xs text-slate-400 dark:text-slate-500">
                Photos are straightened, de-duplicated and downscaled on the way in.
              </p>
              {patients.length === 0 && (
                <p className="mt-3 text-xs text-amber-600">Add a patient first.</p>
              )}
            </div>
          </div>
        ) : (
          <>
            {pending.length > 1 && (
              <BulkBar
                selectedCount={sel.ids.size}
                allChecked={selection.allSelected(sel, pendingIds)}
                onToggleAll={() => setSel((prev) => selection.toggleAll(prev, pendingIds))}
                patients={patients}
                categories={categories}
                onApplyPatient={(id) => applyToSelected((api) => api.setPatient(id))}
                onApplyType={(t) => applyToSelected((api) => api.setDocType(t))}
                onFile={(categoryIds) => void fileSelected(categoryIds)}
                busy={bulkBusy}
                status={bulkStatus}
              />
            )}

            {pending.length > 0 && (
              <ul className="divide-y divide-slate-100 dark:divide-slate-800">
                {pending.map((it) => (
                  <ReviewRow
                    key={it.id}
                    item={it}
                    patients={patients}
                    defaultPatientId={lastPatientId}
                    selected={sel.ids.has(it.id)}
                    onToggle={(shift) =>
                      setSel((prev) => selection.click(prev, pendingIds, it.id, shift))
                    }
                    onRegister={registerRow}
                    onCommitted={onCommitted}
                  />
                ))}
              </ul>
            )}

            {rejected.length > 0 && (
              <ul className="divide-y divide-slate-100 border-t border-slate-200 dark:divide-slate-800 dark:border-slate-800">
                {rejected.map((it) => (
                  <li key={it.id} className="flex items-center gap-4 px-6 py-3 opacity-70">
                    <Thumb id={it.id} kind={it.fileKind} />
                    <div className="min-w-0 flex-1">
                      <p className="selectable truncate text-sm">{it.fileName}</p>
                      <p className="mt-0.5 text-xs text-red-600 dark:text-red-400">{it.error}</p>
                    </div>
                    <span className="shrink-0 rounded bg-slate-100 px-2 py-0.5 text-[11px] font-medium text-slate-500 dark:bg-slate-800 dark:text-slate-400">
                      {it.status === 'duplicate' ? 'Duplicate' : 'Skipped'}
                    </span>
                  </li>
                ))}
              </ul>
            )}

            {filed.length > 0 && (
              <div className="border-t border-slate-200 px-6 py-3 dark:border-slate-800">
                <h2 className="text-xs font-semibold uppercase tracking-wide text-slate-500">
                  Filed
                </h2>
                <ul className="mt-1.5 space-y-0.5">
                  {filed.map((name) => (
                    <li
                      key={name}
                      className="selectable truncate font-mono text-[11px] text-emerald-700 dark:text-emerald-400"
                    >
                      {name}
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </>
        )}

        {(dragging || busy) && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-sky-50/85 dark:bg-sky-950/80">
            <p className="rounded-md border-2 border-dashed border-sky-400 px-8 py-5 text-sm font-medium text-sky-800 dark:text-sky-200">
              {busy ? 'Processing…' : 'Drop to import'}
            </p>
          </div>
        )}
      </main>
    </div>
  );
}
