import { useEffect, useState } from 'react';

import { Spinner } from './Capture.tsx';
import { useBusy } from '../busy.tsx';
import {
  backupToDrive,
  cancelGoogleSignin,
  connectGoogle,
  disconnectGoogle,
  driveStatus,
  finishGoogleSignin,
  restoreFromDrive,
  setGoogleClient,
  type DriveStatus,
  type SyncReport,
} from '../../src/lib/ipc.ts';

/**
 * The same Google Drive backup as the desktop app — same account, same folder,
 * same files.
 *
 * It matters more here than there. The phone's vault lives in app-private
 * storage, which Android deletes when the app is uninstalled, so Drive is not a
 * second copy: it is the copy that survives.
 */
export function Backup({ onError }: { onError: (e: string) => void }) {
  const [status, setStatus] = useState<DriveStatus | null>(null);
  const [report, setReport] = useState<SyncReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [setup, setSetup] = useState(false);
  const [clientId, setClientId] = useState('');
  const [clientSecret, setClientSecret] = useState('');
  const [pasted, setPasted] = useState('');
  const busyGate = useBusy();

  const refresh = () => driveStatus().then(setStatus, (e: unknown) => onError(String(e)));
  useEffect(() => void refresh(), []);

  // Coming back from the browser is the moment the answer might have landed, and
  // it is also the moment nothing on this screen knows that. The listener may
  // have finished the sign-in while the app was away — or been stopped before it
  // could, which is what the paste box is for.
  useEffect(() => {
    const look = () => document.visibilityState === 'visible' && void refresh();
    document.addEventListener('visibilitychange', look);
    return () => document.removeEventListener('visibilitychange', look);
  }, []);

  /** What each job is called while the app is held for it. */
  const SAYING: Record<string, string> = {
    up: 'Backing up to Google Drive…',
    down: 'Restoring from Google Drive…',
    in: 'Opening Google…',
    out: 'Disconnecting…',
    paste: 'Finishing the sign-in…',
    save: 'Saving…',
    check: 'Checking…',
    cancel: 'Starting over…',
  };

  async function run(what: string, fn: () => Promise<unknown>) {
    setBusy(what);
    setReport(null);
    try {
      // Held over the whole app, not just this button. A backup is minutes of
      // uploading, and a second job started against the same database while it
      // runs is how one slow restore turned into every later action failing.
      const result = await busyGate.run(SAYING[what] ?? 'Working…', fn);
      if (result && typeof result === 'object' && 'copied' in result) {
        setReport(result as SyncReport);
      }
      await refresh();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(null);
    }
  }

  const field =
    'h-12 w-full rounded-xl border border-slate-300 bg-white px-3 text-base outline-none ' +
    'focus:border-sky-500 dark:border-slate-700 dark:bg-slate-800';
  const account = status?.account;

  return (
    <div className="space-y-4 px-4 py-4">
      <div className="rounded-2xl border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900">
        {account?.connected ? (
          <>
            <p className="text-xs uppercase tracking-wide text-slate-500">Signed in</p>
            <p className="mt-1 truncate text-base">{account.email ?? 'Google account'}</p>
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => void run('out', disconnectGoogle)}
              className="mt-3 h-11 rounded-xl border border-slate-300 px-4 text-sm dark:border-slate-700"
            >
              Disconnect
            </button>
          </>
        ) : account?.pending ? (
          <>
            <p className="text-base font-medium">Finish in the browser</p>
            <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
              Choose your account and allow access. Come back here when you are done.
            </p>

            <button
              type="button"
              disabled={busy !== null}
              onClick={() => void run('check', async () => {})}
              className="mt-3 h-12 w-full rounded-xl bg-sky-600 text-base font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
            >
              {busy === 'check' ? (
                <span className="flex items-center justify-center gap-2">
                  <Spinner small />
                  Checking…
                </span>
              ) : (
                'I have finished — check'
              )}
            </button>

            <div className="mt-4 rounded-xl bg-amber-50 p-3 dark:bg-amber-950/50">
              <p className="text-sm font-medium text-amber-900 dark:text-amber-200">
                Did the browser show an error page?
              </p>
              <p className="mt-1 text-xs text-amber-800 dark:text-amber-300">
                It still worked. Google put the answer in the address bar before the page failed —
                copy the whole address and paste it here.
              </p>
              <input
                className={`${field} mt-2 font-mono text-xs`}
                placeholder="http://127.0.0.1:…"
                value={pasted}
                onChange={(e) => setPasted(e.target.value)}
                aria-label="The address from the browser"
              />
              <button
                type="button"
                disabled={!pasted.trim() || busy !== null}
                onClick={() =>
                  void run('paste', async () => {
                    await finishGoogleSignin(pasted.trim());
                    setPasted('');
                  })
                }
                className="mt-2 h-12 w-full rounded-xl bg-amber-600 text-base font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
              >
                {busy === 'paste' ? 'Finishing…' : 'Finish sign-in'}
              </button>
            </div>

            <button
              type="button"
              disabled={busy !== null}
              onClick={() => void run('cancel', cancelGoogleSignin)}
              className="mt-2 h-11 w-full rounded-xl text-sm text-slate-500 dark:text-slate-400"
            >
              Start over
            </button>
          </>
        ) : account?.configured ? (
          <>
            <p className="text-base font-medium">Connect Google Drive</p>
            <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
              Your reports are copied to your own Drive. This app can only see the files it puts
              there.
            </p>
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => void run('in', connectGoogle)}
              className="mt-3 h-12 w-full rounded-xl bg-sky-600 text-base font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
            >
              {busy === 'in' ? (
                <span className="flex items-center justify-center gap-2">
                  <Spinner small />
                  Opening Google…
                </span>
              ) : (
                'Choose a Google account'
              )}
            </button>
          </>
        ) : (
          <>
            <p className="text-base font-medium">Set up Google Drive</p>
            <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
              Paste the same client ID and secret you used on the computer — the backup then lands
              in the same Drive folder.
            </p>
            {setup ? (
              <div className="mt-3 space-y-2">
                <input
                  className={`${field} font-mono text-sm`}
                  placeholder="Client ID"
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                  aria-label="Google client ID"
                />
                <input
                  className={`${field} font-mono text-sm`}
                  type="password"
                  placeholder="Client secret"
                  value={clientSecret}
                  onChange={(e) => setClientSecret(e.target.value)}
                  aria-label="Google client secret"
                />
                <button
                  type="button"
                  disabled={!clientId.trim() || busy !== null}
                  onClick={() =>
                    void run('save', async () => {
                      await setGoogleClient(clientId, clientSecret);
                      setSetup(false);
                      setClientId('');
                      setClientSecret('');
                    })
                  }
                  className="h-12 w-full rounded-xl bg-sky-600 text-base font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
                >
                  Save
                </button>
              </div>
            ) : (
              <button
                type="button"
                onClick={() => setSetup(true)}
                className="mt-3 h-12 w-full rounded-xl bg-sky-600 text-base font-medium text-white"
              >
                Enter credentials
              </button>
            )}
          </>
        )}
      </div>

      <div className="rounded-2xl border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900">
        <button
          type="button"
          disabled={busy !== null || !status?.available}
          onClick={() => void run('up', backupToDrive)}
          className="h-12 w-full rounded-xl bg-sky-600 text-base font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
        >
          {busy === 'up' ? (
            <span className="flex items-center justify-center gap-2">
              <Spinner small />
              Backing up…
            </span>
          ) : (
            'Back up now'
          )}
        </button>
        <button
          type="button"
          disabled={busy !== null || !status?.hasBackup}
          onClick={() => {
            if (!window.confirm('Copy back anything missing on this phone? Drive is not changed.'))
              return;
            void run('down', restoreFromDrive);
          }}
          className="mt-2 h-12 w-full rounded-xl border border-slate-300 text-base disabled:opacity-40 dark:border-slate-700"
        >
          {busy === 'down' ? 'Restoring…' : 'Restore from Drive'}
        </button>

        {status?.lastSync && (
          <p className="mt-3 text-xs text-slate-500 dark:text-slate-400">
            Last backup {status.lastSync} UTC
          </p>
        )}

        {report && (
          <p className="mt-2 rounded-xl bg-emerald-50 px-3 py-2 text-sm text-emerald-900 dark:bg-emerald-950 dark:text-emerald-200">
            {report.copied === 0
              ? `Already up to date — ${report.unchanged} file${report.unchanged === 1 ? '' : 's'}.`
              : `${report.copied} file${report.copied === 1 ? '' : 's'} copied.`}
            {report.removed > 0 &&
              ` ${report.removed} old file${report.removed === 1 ? '' : 's'} removed from Drive.`}
            {report.adopted > 0 &&
              ` ${report.adopted} report${report.adopted === 1 ? '' : 's'} added back to the library.`}
          </p>
        )}
      </div>

      <p className="px-1 text-xs text-slate-500 dark:text-slate-400">
        Uninstalling this app deletes the reports stored on the phone. The Drive copy is the one
        that survives, so back up before you change phones.
      </p>
    </div>
  );
}
