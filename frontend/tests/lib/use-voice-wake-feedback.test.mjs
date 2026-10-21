// Node built-in tests for the idle-suspending wake-chime controller exported by
// src/hooks/useVoiceWake.ts. The hook module imports React/Tauri packages that
// are not installed in a bare checkout, so module hooks stub them out; Node
// strips the TypeScript types on import.
//
// Run: node --test tests/lib/use-voice-wake-feedback.test.mjs
import { registerHooks } from 'node:module';
import assert from 'node:assert/strict';
import test, { beforeEach } from 'node:test';

const stubs = new Map([
  ['react', [
    'export const useCallback = (fn) => fn;',
    'export const useEffect = () => {};',
    'export const useRef = (value) => ({ current: value });',
    'export const useState = (value) => [value, () => {}];',
  ].join('\n')],
  ['@tauri-apps/api/core', 'export const invoke = async () => { throw new Error("tauri unavailable"); };'],
  ['@tauri-apps/api/event', 'export const listen = async () => () => {};'],
  ['sonner', 'export const toast = { info() {}, error() {}, dismiss() {} };'],
]);

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (stubs.has(specifier)) return { url: `stub:${specifier}`, shortCircuit: true };
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url.startsWith('stub:')) {
      return { format: 'module', source: stubs.get(url.slice(5)), shortCircuit: true };
    }
    return nextLoad(url, context);
  },
});

const { createFeedbackController } = await import(
  new URL('../../src/hooks/useVoiceWake.ts', import.meta.url).href
);

const settle = () => new Promise((resolve) => setImmediate(resolve));

let instances = [];
let nextResumeImpl = null;
let failStop = false;

class FakeAudioContext {
  constructor() {
    instances.push(this);
    this.state = 'suspended';
    this.events = [];
    this.resumeCalls = 0;
    this.suspendCalls = 0;
    this.closeCalls = 0;
    this.oscillators = [];
    this.resumeImpl = nextResumeImpl;
    nextResumeImpl = null;
  }

  get currentTime() {
    return 0;
  }

  get destination() {
    return {};
  }

  createOscillator() {
    this.events.push('oscillator');
    const oscillator = {
      type: '',
      frequency: { setValueAtTime() {} },
      connect() {},
      disconnect() {
        oscillator.disconnected = true;
      },
      start() {
        oscillator.started = true;
      },
      stop() {
        if (failStop) throw new Error('stop failed');
        oscillator.stopped = true;
      },
      onended: null,
      started: false,
      stopped: false,
      disconnected: false,
    };
    this.oscillators.push(oscillator);
    return oscillator;
  }

  createGain() {
    return {
      gain: { setValueAtTime() {}, linearRampToValueAtTime() {} },
      connect() {},
      disconnect() {},
    };
  }

  async resume() {
    this.resumeCalls += 1;
    this.events.push('resume');
    // One-shot: injected failures model a transient block (e.g. a resume
    // rejected before any gesture); later resumes succeed.
    const impl = this.resumeImpl;
    this.resumeImpl = null;
    if (impl) await impl();
    if (this.state === 'closed') throw new Error('Cannot resume a closed context');
    this.state = 'running';
  }

  async suspend() {
    this.suspendCalls += 1;
    this.events.push('suspend');
    this.state = 'suspended';
  }

  async close() {
    this.closeCalls += 1;
    this.state = 'closed';
  }
}

function installWindow() {
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { AudioContext: FakeAudioContext },
  });
}

beforeEach(() => {
  instances = [];
  nextResumeImpl = null;
  failStop = false;
  installWindow();
});

test('creating the controller does not touch WebAudio', async () => {
  createFeedbackController();
  await settle();
  assert.equal(instances.length, 0);
});

test('gesture unlock resumes then suspends so an idle context is not left running', async () => {
  const controller = createFeedbackController();
  controller.unlock();
  await settle();

  const context = instances[0];
  assert.equal(context.resumeCalls, 1);
  assert.equal(context.suspendCalls, 1);
  assert.equal(context.state, 'suspended');
  assert.equal(context.oscillators.length, 0);
});

test('gesture unlock creates and resumes the context synchronously', async () => {
  const controller = createFeedbackController();
  controller.unlock();

  // No await yet: both must have happened inside the gesture turn.
  assert.equal(instances.length, 1);
  assert.equal(instances[0].resumeCalls, 1);
});

test('wake chime resumes a suspended context before playing', async () => {
  const controller = createFeedbackController();
  controller.play();
  await settle();

  const context = instances[0];
  assert.deepEqual(context.events, ['resume', 'oscillator']);
  assert.equal(context.state, 'running');
  const [oscillator] = context.oscillators;
  assert.equal(oscillator.started, true);
  assert.equal(oscillator.stopped, true);
  assert.equal(typeof oscillator.onended, 'function');
});

test('chime end suspends the context once nothing is playing', async () => {
  const controller = createFeedbackController();
  controller.play();
  await settle();

  const context = instances[0];
  context.oscillators[0].onended();
  await settle();

  assert.equal(context.oscillators[0].disconnected, true);
  assert.equal(context.suspendCalls, 1);
  assert.equal(context.state, 'suspended');
});

test('overlapping chimes keep the context running until the last one ends', async () => {
  const controller = createFeedbackController();
  controller.play();
  controller.play();
  await settle();

  const context = instances[0];
  assert.equal(context.oscillators.length, 2);
  const [first, second] = context.oscillators;

  first.onended();
  await settle();
  assert.equal(context.suspendCalls, 0);
  assert.equal(context.state, 'running');

  second.onended();
  await settle();
  assert.equal(context.suspendCalls, 1);
  assert.equal(context.state, 'suspended');
});

test('a wake arriving as a chime ends never loses the new chime', async () => {
  const controller = createFeedbackController();
  controller.play();
  await settle();

  const context = instances[0];
  const first = context.oscillators[0];

  // Same tick: the new wake is queued, then the old chime ends and queues its
  // suspend check. The suspend must not land before the new chime plays.
  controller.play();
  first.onended();
  await settle();

  assert.equal(context.oscillators.length, 2);
  assert.equal(context.oscillators[1].started, true);
  assert.equal(context.suspendCalls, 0);
  assert.equal(context.state, 'running');

  context.oscillators[1].onended();
  await settle();
  assert.equal(context.suspendCalls, 1);
  assert.equal(context.state, 'suspended');
});

test('teardown during a pending resume plays nothing, closes, and stays closed', async () => {
  let releaseResume;
  const gate = new Promise((resolve) => {
    releaseResume = resolve;
  });
  nextResumeImpl = () => gate;

  const controller = createFeedbackController();
  controller.play();
  await settle();

  const context = instances[0];
  assert.equal(context.resumeCalls, 1);

  controller.dispose();
  releaseResume();
  await settle();

  assert.equal(context.oscillators.length, 0);
  assert.equal(context.closeCalls, 1);
  assert.equal(context.state, 'closed');

  controller.play();
  controller.unlock();
  await settle();
  assert.equal(instances.length, 1);
  assert.equal(context.oscillators.length, 0);
});

test('a chime whose stop fails is still counted out and suspends', async () => {
  const controller = createFeedbackController();
  failStop = true;
  controller.play();
  await settle();

  const context = instances[0];
  assert.equal(context.oscillators.length, 1);
  // The failed chime must not leave a leaked count holding the context open.
  assert.equal(context.suspendCalls, 1);
  assert.equal(context.state, 'suspended');

  // A subsequent healthy chime plays and suspends normally.
  failStop = false;
  controller.play();
  await settle();
  assert.equal(context.oscillators.length, 2);
  context.oscillators[1].onended();
  await settle();
  assert.equal(context.suspendCalls, 2);
  assert.equal(context.state, 'suspended');
});

test('a resume that settles after dispose does not revive the controller', async () => {
  let releaseResume;
  const gate = new Promise((resolve) => {
    releaseResume = resolve;
  });
  nextResumeImpl = () => gate;

  const controller = createFeedbackController();
  controller.unlock();
  await settle();

  const context = instances[0];
  assert.equal(context.resumeCalls, 1);

  controller.dispose();
  releaseResume();
  await settle();

  assert.equal(context.closeCalls, 1);
  assert.equal(context.oscillators.length, 0);

  controller.play();
  await settle();
  assert.equal(instances.length, 1);
  assert.equal(context.oscillators.length, 0);
});

test('a closed context is recreated before the next chime', async () => {
  const controller = createFeedbackController();
  controller.play();
  await settle();

  const first = instances[0];
  first.state = 'closed';

  controller.play();
  await settle();

  assert.equal(instances.length, 2);
  const second = instances[1];
  assert.equal(second.resumeCalls, 1);
  assert.equal(second.oscillators.length, 1);
  assert.equal(second.state, 'running');
});

test('a blocked resume does not throw and later chimes still play', async () => {
  nextResumeImpl = () => Promise.reject(new Error('gesture required'));

  const controller = createFeedbackController();
  controller.play();
  await settle();

  const context = instances[0];
  assert.equal(context.oscillators.length, 0);
  assert.equal(context.state, 'suspended');

  controller.play();
  await settle();
  assert.equal(context.oscillators.length, 1);
  assert.equal(context.state, 'running');
});
