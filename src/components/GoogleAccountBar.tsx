import { useState } from 'react';

import {
  connectGoogle,
  disconnectGoogle,
  setGoogleClient,
  type DriveStatus,
} from '../lib/ipc.ts';

/**
 * Which Google account the backup goes to.
 *
 * Three states, and the panel has to be honest about which one it is in: no
 * OAuth client configured yet, a client but no account, or signed in. The middle
 * one is a setup step nobody can skip — Google issues credentials per app, and
 * this app is published as source, so shipping a client ID in it would spend a
 * stranger's quota and put their name on the consent screen.
 */
export function GoogleAccountBar({
  status,
  onChanged,
}: {
  status: DriveStatus;
  onChanged: () => void;
}) {
  const [clientId, setClientId] = useState('');
  const [clientSecret, setClientSecret] = useState('');
  const [showSetup, setShowSetup] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const account = status.account;

  const input =
    'rounded border border-slate-300 bg-white px-2 py-1 text-xs outline-none ' +
    'focus:border-sky-500 dark:border-slate-600 dark:bg-slate-800';

  async function save() {
    setBusy(true);
    setError(null);
    try {
      await setGoogleClient(clientId, clientSecret);
      setClientId('');
      setClientSecret('');
      setShowSetup(false);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function connect() {
    setBusy(true);
    setError(null);
    try {
      await connectGoogle();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function disconnect() {
    if (
      !window.confirm(
        `Disconnect ${account.email ?? 'this Google account'}?\n\n` +
          'The backup already in Drive is left exactly as it is. This app just stops ' +
          'being able to reach it until an account is connected again.',
      )
    )
      return;
    setBusy(true);
    setError(null);
    try {
      await disconnectGoogle();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mt-1.5">
      <div className="flex flex-wrap items-center gap-2">
        {account.connected ? (
          <>
            <span className="rounded bg-emerald-100 px-2 py-0.5 text-xs font-medium text-emerald-900 dark:bg-emerald-950 dark:text-emerald-200">
              {account.email ?? 'Google account connected'}
            </span>
            <button
              type="button"
              disabled={busy}
              onClick={() => void disconnect()}
              className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-800"
            >
              Disconnect
            </button>
          </>
        ) : account.configured ? (
          <>
            <button
              type="button"
              disabled={busy}
              onClick={() => void connect()}
              className="rounded bg-sky-600 px-3 py-1 text-xs font-medium text-white hover:bg-sky-700 disabled:bg-slate-300 dark:disabled:bg-slate-700"
            >
              {busy ? 'Waiting for Google…' : 'Choose a Google account'}
            </button>
            <span className="text-xs text-slate-500 dark:text-slate-400">
              {busy
                ? 'Pick the account in your browser, then come back.'
                : 'Opens your browser to pick an account.'}
            </span>
            <button
              type="button"
              onClick={() => setShowSetup((v) => !v)}
              className="text-xs text-slate-500 underline decoration-dotted dark:text-slate-400"
            >
              Change client ID
            </button>
          </>
        ) : (
          <button
            type="button"
            onClick={() => setShowSetup(true)}
            className="rounded bg-sky-600 px-3 py-1 text-xs font-medium text-white hover:bg-sky-700"
          >
            Set up Google Drive
          </button>
        )}
      </div>

      {(showSetup || (!account.configured && showSetup)) && (
        <div className="mt-2 rounded border border-slate-200 bg-slate-50 p-2.5 dark:border-slate-700 dark:bg-slate-900">
          <p className="text-xs text-slate-600 dark:text-slate-300">
            Google issues credentials per application, so this app needs one of your own. It is
            free and takes a few minutes:
          </p>
          <ol className="mt-1.5 list-decimal space-y-0.5 pl-5 text-xs text-slate-600 dark:text-slate-300">
            <li>
              At{' '}
              <span className="selectable font-mono">console.cloud.google.com</span> create a
              project, then enable the <b>Google Drive API</b>.
            </li>
            <li>
              Under <b>Credentials</b>, create an <b>OAuth client ID</b> of type{' '}
              <b>Desktop app</b>.
            </li>
            <li>
              On the consent screen, add the scope <span className="selectable font-mono">drive.file</span>{' '}
              and publish it. Left in <b>Testing</b>, Google expires the sign-in every seven days.
            </li>
            <li>Paste the client ID and secret here.</li>
          </ol>

          <div className="mt-2 flex flex-wrap items-center gap-2">
            <input
              className={`${input} w-full font-mono sm:w-auto sm:min-w-72 sm:flex-1`}
              placeholder="000000-xxxx.apps.googleusercontent.com"
              value={clientId}
              onChange={(e) => setClientId(e.target.value)}
              aria-label="Google client ID"
            />
            <input
              className={`${input} w-full font-mono sm:w-auto sm:min-w-48`}
              type="password"
              placeholder="Client secret"
              value={clientSecret}
              onChange={(e) => setClientSecret(e.target.value)}
              aria-label="Google client secret"
            />
            <button
              type="button"
              disabled={busy || !clientId.trim()}
              onClick={() => void save()}
              className="rounded bg-sky-600 px-3 py-1 text-xs font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
            >
              Save
            </button>
            <button
              type="button"
              onClick={() => setShowSetup(false)}
              className="rounded border border-slate-300 px-2 py-1 text-xs dark:border-slate-600"
            >
              Cancel
            </button>
          </div>

          <p className="mt-1.5 text-[11px] text-slate-400 dark:text-slate-500">
            The app asks only for <span className="font-mono">drive.file</span> — it can reach the
            files it creates and nothing else in your Drive. The sign-in is stored encrypted to
            this Windows account.
          </p>
        </div>
      )}

      {error && <p className="mt-1.5 text-xs text-red-600 dark:text-red-400">{error}</p>}
    </div>
  );
}
