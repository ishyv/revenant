
/** Native built-in result types from this executable's compiled contract. */
type RuntimeApp = App<{{checksum_type}}, {{metadata_type}}>;

/** Application facade bound to this compiled desktop contract. */
export interface Application extends RuntimeApp {
  createScope(): Application;
  files: RuntimeApp['files'] & {
{{file_types}}
  };
  media: RuntimeApp['media'] & {
{{media_types}}
  };
  operations: {
{{operation_types}}
  };
}

const bound = new WeakSet<RuntimeApp>();

/** Preserve native scope ownership while attaching typed operations. */
export function bindApp(runtime: RuntimeApp): Application {
  if (bound.has(runtime)) return runtime as Application;
  const createScope = runtime.createScope.bind(runtime);
  bound.add(runtime);
  return Object.assign(runtime, {
    createScope: (): Application => bindApp(createScope()),
    files: Object.assign(runtime.files, {
{{files}}
    }),
    media: Object.assign(runtime.media, {
{{media}}
    }),
    operations: {
{{operations}}
    }
  });
}

/** Connect to the desktop host using the exact compiled manifest. */
export function createApp(options: Omit<Parameters<typeof createRuntime>[0], 'manifest'> = {}): Application {
  return bindApp(createRuntime<{{checksum_type}}, {{metadata_type}}>({ ...options, manifest: contract }));
}

/** Provide this typed application to descendants during component initialization. */
export function provideApp(app: Application): Application {
  provideRuntimeApp(app);
  return app;
}

/** Get the provided application, including its typed native operations. */
export function useApp(app?: Application): Application {
  return bindApp(useRuntimeApp<{{checksum_type}}, {{metadata_type}}>(app));
}

export default createApp;
