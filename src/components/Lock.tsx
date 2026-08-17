import { useEffect, useState } from 'react';

import {
  changePassword,
  disablePassword,
  setPassword,
  unlock,
  type LockState,
} from '../lib/ipc.ts';

/** Lock after this long without keyboard or mouse activity. */
export const IDLE_LOCK_MS = 10 * 60 * 1000;

/**
 * The lock screen.
 *
 * It says plainly what it is. Implying that a password protects the scans would
 * be worse than having no password at all: the user would skip BitLocker, which
 * is the thing that actually protects them, believing they were already covered.
 */
export function LockScreen({ email, onUnlocked }: { email: string; onUnlocked: () => void }) {
  const [password, setPasswordInput] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (await unlock(password)) {
        setPasswordInput('');
        onUnlocked();
      } else {
        setError('Incorrect password.');
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex h-full items-center justify-center bg-white dark:bg-slate-950">
      <form onSubmit={(e) => void submit(e)} className="w-80">
        <h1 className="text-sm font-semibold tracking-tight text-slate-900 dark:text-slate-100">
          Medicine Report Tracker
        </h1>
        {email && <p className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">{email}</p>}

        <input
          autoFocus
          type="password"
          value={password}
          onChange={(e) => setPasswordInput(e.target.value)}
          placeholder="Password"
          className="mt-4 w-full rounded border border-slate-300 bg-white px-3 py-2 text-sm outline-none focus:border-sky-500 dark:border-slate-600 dark:bg-slate-800"
        />

        <button
          type="submit"
          disabled={busy || !password}
          className="mt-2 w-full rounded bg-sky-600 px-3 py-2 text-sm font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
        >
          {busy ? 'Checking…' : 'Unlock'}
        </button>

        {error && <p className="mt-2 text-xs text-red-600 dark:text-red-400">{error}</p>}

        <p className="mt-6 text-xs leading-relaxed text-slate-500 dark:text-slate-400">
          This is a screen lock, not encryption. Your scans stay as ordinary files in
          Documents so they outlive this app — anyone with access to this Windows
          account can open them directly. Turn on <span className="font-medium">BitLocker</span>{' '}
          for real protection if this machine is lost or shared.
        </p>
      </form>
    </div>
  );
}

/** Set, change or remove the lock. Lives in the library, not behind a menu. */
export function LockSettings({ state, onChanged }: { state: LockState; onChanged: () => void }) {
  const [open, setOpen] = useState(false);
  const [email, setEmail] = useState(state.email);
  const [current, setCurrent] = useState('');
  const [next, setNext] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  useEffect(() => setEmail(state.email), [state.email]);

  async function guard(fn: () => Promise<unknown>, message: string) {
    setError(null);
    setDone(null);
    try {
      await fn();
      setCurrent('');
      setNext('');
      setDone(message);
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  const input =
    'rounded border border-slate-300 bg-white px-2 py-1 text-sm outline-none ' +
    'focus:border-sky-500 dark:border-slate-600 dark:bg-slate-800';

  return (
    <div className="border-b border-slate-200 px-6 py-3 dark:border-slate-800">
      <div className="flex flex-wrap items-center gap-2">
        <span className="w-16 shrink-0 text-xs text-slate-500 dark:text-slate-400">Lock</span>
        <span className="text-xs text-slate-600 dark:text-slate-300">
          {state.enabled ? `On — ${state.email || 'no email set'}` : 'Off'}
        </span>
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          className="rounded border border-slate-300 px-2 py-0.5 text-xs hover:bg-slate-50 dark:border-slate-600 dark:hover:bg-slate-800"
        >
          {open ? 'Close' : state.enabled ? 'Change' : 'Set a password'}
        </button>
        {done && <span className="text-xs text-emerald-700 dark:text-emerald-400">{done}</span>}
      </div>

      {open && (
        <div className="mt-2 flex flex-wrap items-center gap-2">
          {!state.enabled && (
            <>
              <input
                className={`${input} w-56`}
                placeholder="Email (stored, not used to sign in)"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
              <input
                className={`${input} w-48`}
                type="password"
                placeholder="New password (8+ characters)"
                value={next}
                onChange={(e) => setNext(e.target.value)}
              />
              <button
                type="button"
                onClick={() => void guard(() => setPassword(email, next), 'Password set.')}
                className="rounded bg-sky-600 px-3 py-1 text-sm font-medium text-white"
              >
                Set
              </button>
            </>
          )}

          {state.enabled && (
            <>
              <input
                className={`${input} w-44`}
                type="password"
                placeholder="Current password"
                value={current}
                onChange={(e) => setCurrent(e.target.value)}
              />
              <input
                className={`${input} w-44`}
                type="password"
                placeholder="New password"
                value={next}
                onChange={(e) => setNext(e.target.value)}
              />
              <button
                type="button"
                disabled={!current || !next}
                onClick={() => void guard(() => changePassword(current, next), 'Password changed.')}
                className="rounded bg-sky-600 px-3 py-1 text-sm font-medium text-white disabled:bg-slate-300 dark:disabled:bg-slate-700"
              >
                Change
              </button>
              <button
                type="button"
                disabled={!current}
                onClick={() => void guard(() => disablePassword(current), 'Lock removed.')}
                className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50 disabled:opacity-50 dark:border-slate-600 dark:hover:bg-slate-800"
              >
                Remove lock
              </button>
            </>
          )}
        </div>
      )}

      {error && <p className="mt-1 text-xs text-red-600 dark:text-red-400">{error}</p>}

      {open && (
        <p className="mt-2 max-w-2xl text-xs leading-relaxed text-slate-500 dark:text-slate-400">
          A screen lock only. Your scans stay as ordinary readable files so the archive
          outlives this app, and the lock does not change that. Use BitLocker if the
          machine could be lost or shared. Locks itself after 10 minutes idle.
        </p>
      )}
    </div>
  );
}
