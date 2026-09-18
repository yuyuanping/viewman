import test from 'node:test';
import assert from 'node:assert/strict';
import { createPotPlayerSession } from './potPlayerSession.ts';

const video = (id) => ({ id, path: id, duration: 100 });
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
};
const flush = () => new Promise(resolve => setImmediate(resolve));
function fixture(overrides = {}) {
  const timers = new Map(), saved = [], errors = [];
  let next = 0;
  const ports = {
    launch: async () => {},
    poll: async () => ({ running: false, state: 'unknown', position: 80, position_source: 'remembered' }),
    save: async (...args) => { saved.push(args); },
    report: error => errors.push(error),
    schedule: callback => { timers.set(++next, callback); return next; },
    cancel: id => timers.delete(id),
    ...overrides,
  };
  const controller = createPotPlayerSession(ports);
  const tick = async () => {
    const entry = timers.entries().next().value;
    assert.ok(entry, 'expected a scheduled poll');
    timers.delete(entry[0]); entry[1](); await flush();
  };
  return { controller, timers, saved, errors, tick };
}

test('late poll from A cannot stop B', async () => {
  const request = deferred();
  const f = fixture({ poll: () => request.promise });
  await f.controller.launch(video('a'), null);
  await f.tick();
  await f.controller.launch(video('b'), null);
  request.resolve({ running: false, state: 'stopped', position: null, position_source: null });
  await flush();
  assert.equal(f.timers.size, 1);
  assert.deepEqual(f.saved, []);
});

test('launch records known position; missing telemetry never invents new progress', async () => {
  const f = fixture();
  await f.controller.launch(video('a'), 12);
  for (let i = 0; i < 6; i++) await f.tick();
  assert.deepEqual(f.saved, [['a', 12]]);
  assert.equal(f.timers.size, 0);
  assert.ok(f.errors.at(-1));
});

test('polls do not overlap and only changed live positions are saved', async () => {
  const request = deferred();
  const f = fixture({ poll: () => request.promise });
  await f.controller.launch(video('a'), null);
  await f.tick();
  assert.equal(f.timers.size, 0);
  request.resolve({ running: true, state: 'running', position: 20, position_source: 'live' });
  await flush();
  await f.tick();
  assert.deepEqual(f.saved, [['a', 20]]);
  f.controller.stop();
  assert.equal(f.timers.size, 0);
});

test('late failed launch does not clear a newer launch', async () => {
  const request = deferred();
  const launched = [];
  const f = fixture({ launch: path => { launched.push(path); return path === 'a' ? request.promise : Promise.resolve(); } });
  const a = f.controller.launch(video('a'), null);
  await flush();
  const b = f.controller.launch(video('b'), null);
  request.reject(new Error('old launch failed'));
  await Promise.all([a, b]);
  assert.deepEqual(launched, ['a', 'b']);
  assert.equal(f.timers.size, 1);
  assert.deepEqual(f.errors.filter(Boolean), []);
});
