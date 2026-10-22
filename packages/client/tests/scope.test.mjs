import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Scope } from '../src/scope.ts';

test('cleanup keeps bounded recent failures while notifying every failure in teardown order', async () => {
  const requests = [];
  const scope = new Scope({ async dispatch(request) { requests.push(request); } }, Promise.resolve('root'));
  const observed = [];
  scope.cleanupErrors.subscribe(errors => { if (errors.length) observed.push(errors.at(-1).message); });
  for (let i = 0; i < 512; i++) scope.own({ dispose() { throw new Error(`cleanup-${i}`); } });
  await scope.disposeAsync();
  assert.equal(observed.length, 512);
  assert.equal(observed[0], 'cleanup-511');
  assert.equal(observed.at(-1), 'cleanup-0');
  assert.equal(scope.cleanupErrors.current.length, 64);
  assert.deepEqual(scope.cleanupErrors.current.map(error => error.message), observed.slice(-64));
  assert.equal(scope.cleanupErrors.current.every(error => error.code === 'cleanup_failed'), true);
  assert.deepEqual(requests, [{ action: 'scope.dispose', scope: 'root' }]);
  await scope.disposeAsync();
  assert.equal(observed.length, 512);
});
