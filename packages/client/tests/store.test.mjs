import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Signal } from '../src/store.ts';
import { derived, get } from 'svelte/store';

test('invalidation reentry preserves write order for every subscriber', () => {
  const signal = new Signal(0);
  const first = [], second = [];
  let nested = false;
  signal.subscribe(value => first.push(value), () => {
    if (!nested) { nested = true; signal.set(2); }
  });
  signal.subscribe(value => second.push(value));
  signal.set(1);
  assert.equal(signal.current, 2);
  assert.deepEqual(first, [0, 1, 2]);
  assert.deepEqual(second, [0, 1, 2]);
});

test('nested run callbacks across signals retain global publication order', () => {
  const a = new Signal(0), b = new Signal(0);
  const values = [];
  a.subscribe(value => { if (value) { values.push(`a:${value}`); b.set(value); } });
  b.subscribe(value => { if (value) { values.push(`b:${value}`); if (value === 1) a.set(2); } });
  a.set(1);
  assert.deepEqual(values, ['a:1', 'b:1', 'a:2', 'b:2']);
});

test('unsubscription and clearing during invalidation suppress pending delivery', () => {
  const signal = new Signal(0);
  const values = [];
  let remove;
  remove = signal.subscribe(value => values.push(value), () => remove());
  signal.set(1);
  assert.deepEqual(values, [0]);
  signal.subscribe(value => values.push(value), () => signal.clear());
  signal.set(2);
  assert.deepEqual(values, [0, 1]);
});

test('Svelte derived stores wait for synchronously invalidated nested writes', () => {
  const a = new Signal(0), b = new Signal(0);
  a.subscribe(value => { if (value) b.set(value); });
  const sum = derived([a, b], ([left, right]) => left + right);
  const values = [];
  const stop = sum.subscribe(value => values.push(value));
  a.set(1);
  a.set(2);
  assert.deepEqual(values, [0, 2, 4]);
  assert.equal(get(sum), 4);
  stop();
});
