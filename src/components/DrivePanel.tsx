import { useEffect, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';

import { GoogleAccountBar } from './GoogleAccountBar.tsx';

import {
  backupToDrive,
  driveStatus,
  restoreFromDrive,
  setDriveFolder,
  type DriveStatus,
  type SyncReport,
} from '../lib/ipc.ts';

/**
 * A second copy of the vault, in Google Drive.
 *
 * The most likely way to lose a decade of records is one dead disk, so this is
 * the panel that matters most and the one least likely to be visited.
 *
 * Two ways to get there. Signing in to a Google account uploads over the API and
 * works on any machine. Failing that — no account connected — it falls back to
 * copying into the folder Google Drive for desktop already mounts, which needs
 * no credentials at all. Either way what lands in Drive is the vault itself:
 * ordinary folders and files, readable in a browser without this app.
 */
export function DrivePanel({ onRestored }: { onRestored: () => void }) {
  const [status, setStatus] = useState<DriveStatus | null>(null);
  const [busy, setBusy] = useState<'backup' | 'restore' | null>(null);
  const [report, setReport] = useState<SyncReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = () => driveStatus().then(setStatus, (e: unknown) => setError(String(e)));
  useEffect(() => void refresh(), []);

  async function choose() {
    try {
      const picked = await open({
        directory: true,
        title: 'Which folder does Google Drive sync?',
        defaultPath: status?.folder ?? undefined,
      });
      if (typeof picked !== 'string') return;
      await setDriveFolder(picked);
      setError(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function run(which: 'backup' | 'restore') {
    setBusy(which);
    setError(null);
    setReport(null);
    try {
      setReport(which === 'backup' ? await backupToDrive() : await restoreFromDrive());
      if (which === 'restore') onRestored();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  const mb = (bytes: number) =>
    bytes >= 1024 * 1024
      ? `${(bytes / 1024 / 1024).toFixed(1)} MB`
      : `${Math.max(1, Math.round(bytes / 1024))} KB`;

  return (
    <div className="border-b border-slate-200 px-6 py-3 dark:border-slate-800">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-slate-500">
        Google Drive
      </h2>

      {status && <GoogleAccountBar status={status} onChanged={() => void refresh()} />}

      <div className="mt-1.5 flex flex-wrap items-center gap-2">
        <button
          type="button"
          disabled={busy !== null || !status?.available}
          onClick={() => void run('backup')}
          className="rounded bg-sky-600 px-3 py-1 text-xs font-medium text-white hover:bg-sky-700 disabled:cursor-not-allowed disabled:bg-slate-300 dark:disabled:bg-slate-700"
        >
          {busy === 'backup' ? 'Copying…' : 'Sync with Google Drive'}
        </button>

        <button
          type="button"
          disabled={busy !== null || !status?.hasBackup}
          title="Copy back anything missing or damaged on this computer"
          onClick={() => {
            if (
              !window.confirm(
                'Restore from Google Drive?\n\n' +
                  'Files missing or different on this computer are copied back from Drive. ' +
                  'Nothing in Drive is changed, and files that already match are left alone.',
              )
            )
              return;
            void run('restore');
          }}
          className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50 disabled:opacity-40 dark:border-slate-600 dark:hover:bg-slate-800"
        >
          {busy === 'restore' ? 'Restoring…' : 'Restore from Drive'}
        </button>

        {status?.method === 'folder' && (
          <button
            type="button"
            onClick={() => void choose()}
            className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-800"
          >
            Change folder…
          </button>
        )}

        {status?.lastSync && (
          <span className="text-xs text-slate-500 dark:text-slate-400">
            last synced {status.lastSync} UTC
          </span>
        )}
      </div>

      <p className="selectable mt-1 font-mono text-[11px] text-slate-500 dark:text-slate-400">
        {status?.backupPath ??
          'Connect a Google account, or choose the folder Drive for desktop syncs.'}
      </p>

      {status?.method === 'folder' && !status.available && status.folder && (
        <p className="mt-1 text-xs text-amber-600">
          That folder is not there right now. Google Drive for desktop may be signed out or
          stopped — or connect a Google account instead, which does not need it.
        </p>
      )}

      <p className="mt-1 text-[11px] text-slate-400 dark:text-slate-500">
        Documents, their notes and tags, and a database snapshot. Exports are left out — they
        rebuild from the documents. Nothing is encrypted, so anyone with access to that Drive
        folder can read every report.
      </p>

      {report && (
        <div className="mt-2 rounded border border-emerald-300 bg-emerald-50 px-2 py-1.5 text-xs dark:border-emerald-900 dark:bg-emerald-950">
          <p className="text-emerald-900 dark:text-emerald-200">
            {report.copied === 0
              ? `Already up to date — ${report.unchanged} file${report.unchanged === 1 ? '' : 's'} unchanged.`
              : `${report.copied} file${report.copied === 1 ? '' : 's'} copied (${mb(report.bytes)}), ${report.unchanged} already there.`}
          </p>
          {report.failed.length > 0 && (
            <div className="mt-1 text-amber-700 dark:text-amber-300">
              <p>
                {report.failed.length} could not be copied — something had them open. Try again
                once Drive has finished uploading.
              </p>
              <ul className="selectable mt-0.5 space-y-0.5 font-mono text-[11px]">
                {report.failed.slice(0, 3).map((f) => (
                  <li key={f}>{f}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {error && <p className="mt-1.5 text-xs text-red-600 dark:text-red-400">{error}</p>}
    </div>
  );
}
