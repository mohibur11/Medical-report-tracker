import { useState } from 'react';

import { checkForUpdate, openDownload, type UpdateCheck as Result } from '../lib/ipc.ts';

/**
 * The version, and a way to find out whether there is a newer one.
 *
 * Checking is a click, never automatic: the app's promise is that nothing
 * leaves the machine unless asked, and a launch-time call to GitHub would be
 * the first exception. The download opens in the browser rather than being
 * fetched here, so what arrives and where it lands are both in plain view.
 */
export function UpdateCheck({ version }: { version: string }) {
  const [state, setState] = useState<
    { kind: 'idle' } | { kind: 'checking' } | { kind: 'done'; result: Result } | { kind: 'failed'; why: string }
  >({ kind: 'idle' });

  async function check() {
    setState({ kind: 'checking' });
    try {
      setState({ kind: 'done', result: await checkForUpdate() });
    } catch (e) {
      setState({ kind: 'failed', why: String(e) });
    }
  }

  const link = 'underline decoration-dotted underline-offset-2 hover:text-slate-700 dark:hover:text-slate-200';

  return (
    <span className="ml-3 font-mono text-slate-400 dark:text-slate-500">
      v{version}
      {state.kind === 'idle' && (
        <button type="button" onClick={() => void check()} className={`ml-1.5 ${link}`}>
          check for updates
        </button>
      )}
      {state.kind === 'checking' && <span className="ml-1.5">checking…</span>}
      {state.kind === 'failed' && (
        <span className="ml-1.5 text-amber-600" title={state.why}>
          could not check
          <button type="button" onClick={() => void check()} className={`ml-1 ${link}`}>
            retry
          </button>
        </span>
      )}
      {state.kind === 'done' &&
        (state.result.newer ? (
          <span className="ml-1.5 font-sans font-medium text-sky-600 dark:text-sky-400">
            v{state.result.latest} available —{' '}
            <button
              type="button"
              onClick={() => void openDownload(state.result.downloadUrl)}
              className={link}
            >
              download
            </button>
          </span>
        ) : (
          <span className="ml-1.5">up to date</span>
        ))}
    </span>
  );
}
