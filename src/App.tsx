import { useCallback, useEffect, useRef, useState } from 'react';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open } from '@tauri-apps/plugin-dialog';

import { CategoryManager, CategoryPicker } from './components/CategoryPicker.tsx';
import { EditRow } from './components/EditRow.tsx';
import { ExportPanel } from './components/ExportPanel.tsx';
import { IDLE_LOCK_MS, LockScreen, LockSettings } from './components/Lock.tsx';
import { ReviewRow } from './components/ReviewRow.tsx';
import { Thumb } from './components/Thumb.tsx';
import { VaultTools } from './components/VaultTools.tsx';
import { formatDmy } from './lib/extract/dates.ts';
import {
  createPatient,
  documentTags,
  health,
  importFiles,
  listCategories,
  listDocuments,
  listPatients,
  listYears,
  lockState,
  type LockState,
  setDocumentCategories,
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
    listDocuments().then((rows) => {
      setDocs(rows);
      setViewChosen((chosen) => {
        if (!chosen && rows.length > 0) setView('library');
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
      setItems((prev) => [...staged, ...prev]);
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
    refresh();
  }

  const pending = items.filter((i) => i.status === 'needs_date' || i.status === 'pending');
  const rejected = items.filter((i) => i.status === 'failed' || i.status === 'duplicate');

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
            {pending.length > 0 && (
              <ul className="divide-y divide-slate-100 dark:divide-slate-800">
                {pending.map((it) => (
                  <ReviewRow
                    key={it.id}
                    item={it}
                    patients={patients}
                    defaultPatientId={lastPatientId}
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
