import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from 'react';

/**
 * One thing at a time, and the screen says which.
 *
 * A phone shows one screen, so work started on it has nowhere to go and be
 * visible. A restore that takes half a minute looked like a dead app, and tapping
 * around during it started a second job against the same database — which is how
 * one failure turned into every later action failing.
 *
 * So: while something is running, the whole app is covered, taps go to the cover
 * rather than to the buttons underneath, and the cover says what is happening.
 */
interface Busy {
  /** What is running, in words a person would use. Null when idle. */
  label: string | null;
  /** Run something with the app held. Returns whatever it returns. */
  run: <T>(label: string, work: () => Promise<T>) => Promise<T>;
}

const BusyContext = createContext<Busy>({
  label: null,
  run: (_label, work) => work(),
});

export function useBusy() {
  return useContext(BusyContext);
}

export function BusyProvider({ children }: { children: ReactNode }) {
  const [label, setLabel] = useState<string | null>(null);

  const run = useCallback(async <T,>(next: string, work: () => Promise<T>): Promise<T> => {
    setLabel(next);
    try {
      return await work();
    } finally {
      // Always. A job that fails must not leave the app covered for good.
      setLabel(null);
    }
  }, []);

  const value = useMemo(() => ({ label, run }), [label, run]);

  return (
    <BusyContext.Provider value={value}>
      {children}
      {label !== null && <BusyOverlay label={label} />}
    </BusyContext.Provider>
  );
}

function BusyOverlay({ label }: { label: string }) {
  return (
    <div
      // Above everything except the page preview, which is its own full screen.
      className="fixed inset-0 z-[900] flex flex-col items-center justify-center gap-4 bg-slate-950/70 backdrop-blur-sm"
      role="alertdialog"
      aria-busy="true"
      aria-label={label}
      // Swallows every tap underneath. That is the point of it.
      onPointerDown={(e) => e.preventDefault()}
      onTouchStart={(e) => e.preventDefault()}
    >
      <span
        role="progressbar"
        className="inline-block size-12 animate-spin rounded-full border-[3px] border-sky-400 border-t-transparent"
      />
      <p className="px-8 text-center text-base font-medium text-white">{label}</p>
      <p className="px-10 text-center text-xs text-white/60">
        Please wait — closing the app now would leave this half done.
      </p>
    </div>
  );
}
