import { queryGroup } from '../src/query-group.js';
import type { Scope } from '../src/scope.js';
import type { ReadOperationFunction, OperationFunction } from '../src/operation.js';

function check(owner: Scope, title: ReadOperationFunction<{ title: string }, string>,
  count: ReadOperationFunction<{ limit: number }, number>, write: OperationFunction<{ title: string }, string>) {
  const group = queryGroup(owner, { required: { title }, optional: { count } });
  void group.load('key', { title: { title: 'a' }, count: { limit: 1 } });
  void group.load('key', { title: { title: 'a' }, count: { limit: 1 } }, { skipOptional: ['count'] });
  // @ts-expect-error required slots cannot be skipped
  void group.load('key', { title: { title: 'a' }, count: { limit: 1 } }, { skipOptional: ['title'] });
  // @ts-expect-error only declared optional names are accepted
  void group.load('key', { title: { title: 'a' }, count: { limit: 1 } }, { skipOptional: ['missing'] });
  const data = group.snapshot.data;
  if (data) { const required: string = data.title; const optional: number | undefined = data.count; void required; void optional; }
  // @ts-expect-error mutations do not expose declared-read helpers
  queryGroup(owner, { required: { write } });
  // @ts-expect-error slot inputs use each operation's compiled input
  void group.load('key', { title: { title: 'a' }, count: { limit: 'wrong' } });
  // @ts-expect-error optional result slots still need explicit load inputs
  void group.load('key', { title: { title: 'a' } });
  // @ts-expect-error required output type is preserved
  const bad: number | undefined = group.snapshot.data?.title;
  void bad;
}
void check;
