import { useCallback, useEffect, useState } from 'react';

import { Backup } from './screens/Backup.tsx';
import { Capture } from './screens/Capture.tsx';
import { Library } from './screens/Library.tsx';
import { People } from './screens/People.tsx';
import {
  documentTags,
  listCategories,
  listDocuments,
  listPatients,
  listStaged,
  takeShared,
  type Category,
  type DocumentRow,
  type IngestItem,
  type Patient,
} from '../src/lib/ipc.ts';

export type Tab = 'capture' | 'library' | 'people' | 'backup';

/**
 * The phone app.
 *
 * Same backend, same vault, same Drive account as the desktop app — a different
 * set of screens. A phone is held in one hand, used a metre from a doctor's desk,
 * and has a third of the width; the desktop review grid does not survive any of
 * that.
 *
 * Everything the system draws over — status bar, gesture bar — is accounted for
 * here rather than in each screen, because getting it wrong once hid the title
 * behind the clock.
 */
export default function App() {
  const [tab, setTab] = useState<Tab>('capture');
  const [patients, setPatients] = useState<Patient[]>([]);
  const [items, setItems] = useState<IngestItem[]>([]);
  const [docs, setDocs] = useState<DocumentRow[]>([]);
  const [categories, setCategories] = useState<Category[]>([]);
  const [tags, setTags] = useState<Record<string, string[]>>({});
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    const fail = (e: unknown) => setError(String(e));
    listPatients().then(setPatients, fail);
    listStaged().then(setItems, fail);
    listCategories().then(setCategories, fail);
    listDocuments().then((rows) => {
      setDocs(rows);
      documentTags(rows.map((r) => r.id)).then((pairs) => {
        const map: Record<string, string[]> = {};
        for (const [docId, catId] of pairs) (map[docId] ??= []).push(catId);
        setTags(map);
      }, fail);
    }, fail);
  }, []);

  useEffect(refresh, [refresh]);

  /**
   * Pick up anything shared to the app.
   *
   * On opening, and again whenever the app comes back to the foreground — which
   * is exactly the moment a share has just happened, because sharing brings this
   * app forward.
   */
  useEffect(() => {
    const collect = () => {
      takeShared().then(
        (staged) => {
          if (staged.length > 0) {
            setTab('capture');
            refresh();
          }
        },
        () => {},
      );
    };

    collect();
    const onVisible = () => document.visibilityState === 'visible' && collect();
    document.addEventListener('visibilitychange', onVisible);
    return () => document.removeEventListener('visibilitychange', onVisible);
  }, [refresh]);

  const waiting = items.filter(
    (i) => i.status === 'needs_date' || i.status === 'pending' || i.status === 'locked',
  ).length;

  return (
    <div className="flex h-full flex-col bg-slate-50 text-slate-900 dark:bg-slate-950 dark:text-slate-100">
      <header className="pad-top shrink-0 border-b border-slate-200 bg-white px-4 pb-3 dark:border-slate-800 dark:bg-slate-900">
        <h1 className="text-lg font-semibold tracking-tight">
          {tab === 'capture' && 'Add a report'}
          {tab === 'library' && 'Reports'}
          {tab === 'people' && 'People'}
          {tab === 'backup' && 'Backup'}
        </h1>
        <p className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
          {docs.length} filed
          {waiting > 0 && ` · ${waiting} waiting`}
          {patients.length > 0 && ` · ${patients.length} ${patients.length === 1 ? 'person' : 'people'}`}
        </p>
      </header>

      {error && (
        <button
          type="button"
          onClick={() => setError(null)}
          className="selectable shrink-0 border-b border-red-300 bg-red-50 px-4 py-2 text-left text-xs text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
        >
          {error}
          <span className="ml-2 opacity-60">tap to dismiss</span>
        </button>
      )}

      <main className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
        {tab === 'capture' && (
          <Capture
            items={items}
            patients={patients}
            categories={categories}
            onChanged={refresh}
            onError={setError}
            onNeedPeople={() => setTab('people')}
          />
        )}
        {tab === 'library' && (
          <Library
            docs={docs}
            patients={patients}
            categories={categories}
            tags={tags}
            onChanged={refresh}
            onError={setError}
          />
        )}
        {tab === 'people' && <People patients={patients} onChanged={refresh} onError={setError} />}
        {tab === 'backup' && <Backup onChanged={refresh} onError={setError} />}
      </main>

      <TabBar tab={tab} onTab={setTab} waiting={waiting} />
    </div>
  );
}

/**
 * Four destinations, thumb-height, at the bottom where a thumb reaches.
 */
function TabBar({
  tab,
  onTab,
  waiting,
}: {
  tab: Tab;
  onTab: (t: Tab) => void;
  waiting: number;
}) {
  const tabs: Array<{ id: Tab; label: string; icon: string; badge?: number }> = [
    { id: 'capture', label: 'Add', icon: '＋', badge: waiting },
    { id: 'library', label: 'Reports', icon: '▤' },
    { id: 'people', label: 'People', icon: '☺' },
    { id: 'backup', label: 'Backup', icon: '☁' },
  ];

  return (
    <nav className="pad-bottom shrink-0 border-t border-slate-200 bg-white px-2 pt-1 dark:border-slate-800 dark:bg-slate-900">
      <div className="flex">
        {tabs.map((t) => {
          const active = tab === t.id;
          return (
            <button
              key={t.id}
              type="button"
              onClick={() => onTab(t.id)}
              // 56px tall: below about 48 a thumb starts missing.
              className={`relative flex h-14 flex-1 flex-col items-center justify-center gap-0.5 rounded-xl text-[11px] ${
                active
                  ? 'bg-sky-50 font-medium text-sky-700 dark:bg-sky-950 dark:text-sky-300'
                  : 'text-slate-500 dark:text-slate-400'
              }`}
            >
              <span className="text-xl leading-none">{t.icon}</span>
              {t.label}
              {t.badge ? (
                <span className="absolute right-1/4 top-1 min-w-4 rounded-full bg-amber-500 px-1 text-[10px] font-semibold text-white">
                  {t.badge}
                </span>
              ) : null}
            </button>
          );
        })}
      </div>
    </nav>
  );
}
