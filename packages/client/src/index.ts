/** Desktop-only capability projections. Generated authoring facades bind these public contracts. */
export { createApp, App } from "./app.js";
export type { AppOptions, AppStatus } from "./app.js";
export { Collection, Selection } from "./collection.js";
export type { CollectionSnapshot } from "./collection.js";
export { Task, BatchResult } from "./task.js";
export { Operation, operationFactory } from "./operation.js";
export type { NativeSelection, NativeSelectionMatching } from "./operation.js";
export { Runtime } from "./runtime.js";
export type { ProviderDescriptor, ProviderSnapshot } from "./runtime.js";
export {
  Files,
  Folder,
  FileCollection,
  FileSelection,
  AllMatchingSelection,
} from "./files.js";
export type {
  FolderDescriptor,
  FolderBookmark,
  FileQueryOptions,
  FileCollectionSnapshot,
  SelectionStatus,
} from "./files.js";
export { Media, Preview } from "./media.js";
export type { PreviewDescriptor, PreviewKind } from "./media.js";
export { Settings, SettingsFactory } from "./settings.js";
export type { SettingsOptions, PersistenceStatus } from "./settings.js";
export { Scope } from "./scope.js";
export type { Disposable } from "./scope.js";
export type { Readable, Unsubscribe } from "./store.js";
export { TauriTransport } from "./transport.js";
export type { NativeTransport, NativeRequest } from "./transport.js";
export type * from "./wire.js";
export { validator, validateManifest } from "./schema.js";
export { CapabilityError, errorOf, progressOf } from "./contracts.js";
export type * from "./contracts.js";
