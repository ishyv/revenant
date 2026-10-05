/** Svelte 5 context helpers. Generated facades configure roots; components own child scopes. */
import { getContext, onDestroy, setContext } from "svelte";
import { createApp as createRoot } from "./app.js";
import type { App, AppOptions } from "./app.js";
import { failure } from "./contracts.js";
const application = Symbol("revenant.application");

/** Configure a desktop root during parent initialization and dispose it on unmount. */
export function createApp<Checksum = unknown, Metadata = unknown>(
  options: AppOptions = {},
): App<Checksum, Metadata> {
  return setupApp(createRoot<Checksum, Metadata>(options));
}
/** Provide an existing root during initialization and own its cleanup on component teardown. */
export function setupApp<Checksum, Metadata>(
  app: App<Checksum, Metadata>,
): App<Checksum, Metadata> {
  setContext(application, app);
  onDestroy(() => app.dispose());
  return app;
}
/** Provide an existing generated root to descendants without transferring root ownership. */
export function provideApp<Checksum, Metadata>(
  app: App<Checksum, Metadata>,
): App<Checksum, Metadata> {
  setContext(application, app);
  return app;
}
/** Own a child of the nearest app context and provide it to descendants until unmount. */
export function useApp<Checksum = unknown, Metadata = unknown>(
  app?: App<Checksum, Metadata>,
): App<Checksum, Metadata> {
  const configured =
    app ?? getContext<App<Checksum, Metadata> | undefined>(application);
  if (!configured)
    failure(
      "app_not_configured",
      "Configure createApp or provideApp in a parent component",
    );
  const scoped = configured.createScope();
  setContext(application, scoped);
  onDestroy(() => scoped.dispose());
  return scoped;
}
