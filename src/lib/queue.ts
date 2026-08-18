/**
 * A tiny concurrency gate.
 *
 * Every review row reads its own page on mount, and a backlog import mounts
 * hundreds of rows at once. Firing hundreds of recognitions together does not
 * make any of them finish sooner — the machine has a fixed number of cores — but
 * it does bury the one row the user is actually looking at behind all the others.
 * Two at a time keeps the top of the list responsive and the rest arriving
 * steadily.
 */

export interface Queue {
  run<T>(job: () => Promise<T>): Promise<T>;
  /** For tests and diagnostics. */
  readonly active: number;
  readonly waiting: number;
}

export function createQueue(limit: number): Queue {
  let active = 0;
  const waiting: Array<() => void> = [];

  const next = () => {
    // Shift, not pop: the first row asked for is the first row answered, so the
    // queue reads top to bottom the way the list does.
    const waiter = waiting.shift();
    if (waiter) {
      // The slot is handed straight over rather than released and re-taken. If it
      // were released first, a job starting in the gap would claim it and the
      // resumed waiter would take a second one, putting three recognitions in
      // flight under a limit of two.
      waiter();
    } else {
      active -= 1;
    }
  };

  return {
    get active() {
      return active;
    },
    get waiting() {
      return waiting.length;
    },
    async run<T>(job: () => Promise<T>): Promise<T> {
      if (active >= limit) {
        // Resuming means a slot was handed over, so it is already counted.
        await new Promise<void>((resolve) => waiting.push(resolve));
      } else {
        active += 1;
      }
      try {
        return await job();
      } finally {
        // A failed job must free its slot, or one bad page stalls the queue.
        next();
      }
    },
  };
}

/** Shared by every review row. */
export const ocrQueue = createQueue(2);
