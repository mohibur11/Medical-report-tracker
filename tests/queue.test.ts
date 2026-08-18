import { test } from 'node:test';
import assert from 'node:assert/strict';

import { createQueue } from '../src/lib/queue.ts';

/** A job that finishes only when told to. */
function deferred() {
  let resolve!: (v: string) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<string>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

test('no more than the limit run at once', async () => {
  const q = createQueue(2);
  const jobs = [deferred(), deferred(), deferred()];
  let started = 0;

  const results = jobs.map((j) => q.run(() => { started += 1; return j.promise; }));
  await Promise.resolve();
  assert.equal(started, 2, 'the third waits');
  assert.equal(q.waiting, 1);

  jobs[0]!.resolve('a');
  await results[0];
  assert.equal(started, 3, 'a finished slot is handed straight to the next job');

  jobs[1]!.resolve('b');
  jobs[2]!.resolve('c');
  assert.deepEqual(await Promise.all(results), ['a', 'b', 'c']);
});

test('a failing job frees its slot rather than wedging the queue', async () => {
  const q = createQueue(1);
  const bad = q.run(() => Promise.reject(new Error('unreadable page')));
  await assert.rejects(bad, /unreadable page/);

  assert.equal(await q.run(() => Promise.resolve('next one still runs')), 'next one still runs');
  assert.equal(q.active, 0);
  assert.equal(q.waiting, 0);
});

test('waiting jobs start in the order they were asked for', async () => {
  const q = createQueue(1);
  const order: number[] = [];
  const first = deferred();

  const all = [
    q.run(() => { order.push(0); return first.promise; }),
    q.run(() => { order.push(1); return Promise.resolve('b'); }),
    q.run(() => { order.push(2); return Promise.resolve('c'); }),
  ];

  first.resolve('a');
  await Promise.all(all);
  assert.deepEqual(order, [0, 1, 2]);
});

test('under the limit nothing waits', async () => {
  const q = createQueue(4);
  assert.equal(await q.run(() => Promise.resolve('straight through')), 'straight through');
  assert.equal(q.active, 0);
});

test('a job arriving as another finishes cannot overtake the limit', async () => {
  const q = createQueue(2);
  let inFlight = 0;
  let peak = 0;

  const gates = [deferred(), deferred(), deferred(), deferred()];
  const start = (i: number) =>
    q.run(async () => {
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      const v = await gates[i]!.promise;
      inFlight -= 1;
      return v;
    });

  const running = [start(0), start(1), start(2)];
  // Finish one and immediately ask for another, landing in the gap between the
  // slot being freed and the waiting job resuming.
  gates[0]!.resolve('a');
  running.push(start(3));

  gates[1]!.resolve('b');
  gates[2]!.resolve('c');
  gates[3]!.resolve('d');
  await Promise.all(running);

  assert.equal(peak, 2, 'never more than the limit in flight');
  assert.equal(q.active, 0);
  assert.equal(q.waiting, 0);
});
