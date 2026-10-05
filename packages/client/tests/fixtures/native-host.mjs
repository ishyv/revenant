import assert from 'node:assert/strict';
import { setImmediate } from 'node:timers/promises';

export function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

export async function within(promise, label, milliseconds = 2_000) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`Timed out: ${label}`)), milliseconds);
      }),
    ]);
  } finally { clearTimeout(timer); }
}

export const turn = () => setImmediate();
export const hasCode = code => error => error?.code === code;
export async function until(predicate, label) {
  await within((async () => { while (!predicate()) await turn(); })(), label);
}
export function nativeError(code, message = code, retryable = false) {
  return Object.assign(new Error(message), { code, retryable });
}

const definition = (id, input, output) => ({
  id, description: `Compiled fixture operation ${id}`,
  input: { name: 'Input', typescript: 'unknown', schema: input },
  output: { name: 'Output', typescript: 'unknown', schema: output },
  source: { file: 'native/fixture.rs', line: 12, module: 'fixture' },
});
export const manifest = {
  version: 3, protocol: 2, digest: `sha256:${'a'.repeat(64)}`,
  operations: [
    definition('records.echo', {
      type: 'object', additionalProperties: false,
      properties: { value: { type: 'string' } }, required: ['value'],
    }, { type: 'string' }),
    definition('files.describe', { type: 'object', required: ['handle', 'name'] }, { type: 'string' }),
  ],
};

// Protocol peer with independent native ownership: pages own capabilities;
// selections retain their own leases. A replacement releases prior page owners
// only on its matching ACK. No client state or private methods are inspected.
export class NativeHost {
  calls = [];
  events = [];
  scopes = new Set();
  folders = new Map();
  views = new Map();
  resources = new Map();
  selections = new Map();
  tasks = new Map();
  settings = new Map();
  previews = new Map();
  gates = new Map();
  allGates = new Set();
  names = ['alpha.txt', 'beta.txt', 'gamma.txt'];
  streamQueryBeforeReply = false;
  cancelledPicker = false;
  maxPageOwners = 0;
  nextId = 0;
  providers = [{
    id: 'fixture', description: 'Native fixture provider', dependencies: [],
    configurationSchema: {
      type: 'object', additionalProperties: false,
      properties: { enabled: { type: 'boolean' } }, required: ['enabled'],
    },
    config: { enabled: true }, generation: 1,
    activeTasks: '9007199254740993', resources: '1', mutable: true,
  }];

  id(prefix) { return `${prefix}-${++this.nextId}`; }
  blockNext(action, stage = 'before') {
    const gate = { entered: deferred(), release: deferred(), stage };
    const queue = this.gates.get(action) ?? [];
    queue.push(gate);
    this.gates.set(action, queue);
    this.allGates.add(gate);
    return gate;
  }
  unblockAll() { for (const gate of this.allGates) gate.release.resolve(); }
  requests(action) { return this.calls.filter(call => call.request.action === action); }

  async dispatch(request, updates) {
    request = structuredClone(request);
    this.calls.push({ request, updates });
    this.events.push({ phase: 'request', ...request });
    const gate = this.gates.get(request.action)?.shift();
    if (gate?.stage === 'before') {
      gate.entered.resolve({ request, updates });
      await gate.release.promise;
    }
    const reply = this.admit(request, updates);
    if (gate?.stage === 'reply') {
      gate.entered.resolve({ request, updates, reply: structuredClone(reply) });
      await gate.release.promise;
    }
    this.events.push({ phase: 'reply', ...request });
    return structuredClone(reply);
  }

  admit(request, updates) {
    const { action, scope } = request;
    if (action === 'root.connect') {
      const root = this.id('scope'); this.scopes.add(root);
      return { scope: root, manifest };
    }
    if (action === 'scope.create') {
      assert.ok(this.scopes.has(request.parent));
      const child = this.id('scope'); this.scopes.add(child);
      return { scope: child };
    }
    assert.ok(this.scopes.has(scope), `Native scope must still exist for ${action}`);
    switch (action) {
      case 'folder.open':
      case 'folder.reopen': {
        if (this.cancelledPicker) return null;
        const folder = this.id('folder');
        const path = request.bookmark?.path ?? 'C:\\native-fixture';
        const descriptor = { folder, name: 'Native fixture', path, bookmark: { path } };
        this.folders.set(folder, { scope, descriptor });
        return descriptor;
      }
      case 'folder.query':
      case 'view.query': {
        const folder = action === 'view.query' ? this.views.get(request.view).folder : request.folder;
        assert.equal(this.folders.get(folder).scope, scope);
        const id = this.id('view');
        const view = {
          id, scope, folder, generation: request.generation, revision: 0,
          indexGeneration: 1, options: request.options, offset: request.offset,
          limit: request.limit, names: this.filteredNames(request.options),
          state: 'ready', current: [], prior: [], outstanding: false, updates,
        };
        this.views.set(id, view);
        const snapshot = this.offer(view);
        if (this.streamQueryBeforeReply) updates?.(structuredClone(snapshot));
        return { view: id, snapshot };
      }
      case 'view.window': {
        const view = this.views.get(request.view);
        assert.equal(view.scope, scope);
        if (view.outstanding) throw nativeError('window_pending', 'ACK required', true);
        assert.equal(request.generation, view.generation);
        Object.assign(view, { offset: request.offset, limit: request.limit });
        return this.offer(view);
      }
      case 'view.ack': {
        const view = this.views.get(request.view);
        assert.ok(view, 'Received page must be ACKed before native view disposal');
        if (request.revision !== view.revision) return null;
        for (const entry of view.prior) this.resources.delete(entry.handle.id);
        view.prior = [];
        view.outstanding = false;
        this.events.push({ phase: 'ack', view: view.id, revision: request.revision });
        if (view.dirty) {
          view.dirty = false;
          view.updates?.(structuredClone(this.offer(view)));
        }
        return null;
      }
      case 'view.dispose': {
        const view = this.views.get(request.view);
        if (view) for (const entry of [...view.current, ...view.prior]) this.resources.delete(entry.handle.id);
        this.views.delete(request.view);
        return null;
      }
      case 'folder.dispose': this.folders.delete(request.folder); return null;
      case 'selection.set': {
        assert.ok(request.handles.length <= 512);
          const previous = this.selections.get(request.selection);
          if (previous) assert.equal(previous.view, request.view, 'Selection belongs to its admitting view');
        const entries = request.handles.map(handle => {
          const entry = this.resources.get(handle.id) ?? previous?.entries.find(item => item.handle.id === handle.id);
          assert.ok(entry, `Selection must capture a live capability: ${handle.id}`);
          assert.deepEqual(entry.handle, handle);
          assert.equal(handle.scope, scope);
          return structuredClone(entry);
        });
        const id = request.selection ?? this.id('selection');
          this.selections.set(id, { scope, view: request.view, entries });
        this.events.push({ phase: 'captured', selection: id, handles: request.handles });
        return { selection: id, count: entries.length };
      }
      case 'selection.dispose': this.selections.delete(request.id); return null;
      case 'task.run':
      case 'task.batch': {
        let inputs;
        if (action === 'task.run') inputs = [request.input];
        else if ('inputs' in request.selection) {
          inputs = request.selection.inputs;
          assert.ok(inputs.length <= 512);
          assert.ok(Buffer.byteLength(JSON.stringify(inputs)) <= 1024 * 1024);
        } else if ('id' in request.selection) inputs = this.selections.get(request.selection.id).entries;
        else {
          const view = this.views.get(request.selection.view);
          if (request.selection.generation !== undefined && request.selection.generation !== view.generation)
            throw nativeError('stale_selection');
          inputs = view.names.map(name => ({ name }));
        }
        const id = this.id('task');
        const snapshot = {
          id, scope, state: 'queued', progress: { completed: '0' }, outcomes: [],
          summary: { completed: '0', succeeded: '0', failed: '0' }, cancelRequested: false,
        };
        this.tasks.set(id, { snapshot, updates, inputs: structuredClone(inputs), rows: [] });
        this.events.push({ phase: 'admitted', task: id, inputs: structuredClone(inputs) });
        return snapshot;
      }
      case 'task.snapshot': return this.tasks.get(request.task).snapshot;
      case 'task.cancel': {
        const task = this.tasks.get(request.task);
        task.snapshot = { ...task.snapshot, state: 'cancelling', cancelRequested: true };
        return task.snapshot;
      }
      case 'task.results': {
        const task = this.tasks.get(request.task);
        assert.ok(request.limit <= 512);
        return {
          counts: task.snapshot.summary, offset: request.offset, total: task.rows.length,
          items: task.rows.slice(request.offset, request.offset + request.limit),
        };
      }
      case 'task.dispose': this.tasks.delete(request.task); return null;
      case 'settings.load': return this.settings.get(request.key) ?? request.defaults;
      case 'settings.save': {
        this.settings.set(request.key, structuredClone(request.value));
        this.events.push({ phase: 'persisted', key: request.key, value: request.value });
        return null;
      }
      case 'media.preview': {
        const entry = this.resources.get(request.handle.id);
        assert.ok(entry);
        const preview = this.id('preview');
        const descriptor = { preview, name: entry.name, kind: 'image', mime: 'image/png', url: `http://revenant-preview.localhost/${preview}` };
        this.previews.set(preview, descriptor);
        return descriptor;
      }
      case 'media.dispose': this.previews.delete(request.preview); return null;
      case 'runtime.inspect': return this.providers;
      case 'runtime.configure': this.providers[0].config = request.config; this.providers[0].generation++; return null;
      case 'runtime.replace': this.providers[0].generation++; return null;
      case 'scope.dispose': {
        this.scopes.delete(scope);
        this.events.push({ phase: 'closed', scope });
        return null;
      }
      default: throw new Error(`Unhandled fixture action ${action}`);
    }
  }

  filteredNames(options = {}) {
    const names = this.names.filter(name => name.includes(options.search ?? '')).sort();
    return options.descending ? names.reverse() : names;
  }
  offer(view) {
    assert.equal(view.outstanding, false, 'Only one replacement may be outstanding');
    view.prior = view.current;
    view.current = view.names.slice(view.offset, view.offset + view.limit).map(name => {
      const entry = {
        handle: { id: this.id('file'), scope: view.scope, generation: 1, kind: 'file' },
        name, relativePath: name, size: '42', mime: 'text/plain', modified: '-1',
      };
      this.resources.set(entry.handle.id, entry);
      return entry;
    });
    view.outstanding = true;
    this.maxPageOwners = Math.max(this.maxPageOwners, view.current.length + view.prior.length);
    assert.ok(view.current.length <= 512 && view.prior.length <= 512);
    return {
      view: view.id, revision: ++view.revision, indexGeneration: view.indexGeneration,
      generation: view.generation, items: view.current, offset: view.offset,
      total: view.names.length, discovered: view.names.length, state: view.state,
      warnings: [], warningCount: 0,
    };
  }
  scan(viewId, names, state = 'loading') {
    const view = this.views.get(viewId);
    Object.assign(view, { names: [...names], state, indexGeneration: view.indexGeneration + 1 });
    if (view.outstanding) { view.dirty = true; return; }
    view.updates?.(structuredClone(this.offer(view)));
  }
  complete(taskId, { state = 'succeeded', result, rows = [] } = {}) {
    const task = this.tasks.get(taskId);
    task.rows = structuredClone(rows);
    task.snapshot = {
      ...task.snapshot, state, result,
      progress: { completed: String(rows.length), total: String(rows.length) },
      summary: {
        completed: String(rows.length), succeeded: String(rows.filter(row => !row.error).length),
        failed: String(rows.filter(row => row.error).length),
      },
    };
    task.updates?.(structuredClone(task.snapshot));
  }
}
