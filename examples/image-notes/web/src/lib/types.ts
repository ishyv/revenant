import type { OperationOutput } from "@revenant/client";
import type { Application } from "./revenant";

// Inferred from the compiled Rust contract; no duplicate handwritten wire DTOs.
export type ImageOperations = Application["operations"]["images"];
export type AssetPage = OperationOutput<ImageOperations["query"]>;
export type Asset = AssetPage["items"][number];
export type Startup = OperationOutput<ImageOperations["bootstrap"]>;
export type Root = Startup["roots"][number];
export type SavedNote = OperationOutput<ImageOperations["annotate"]>;
export type ScanReport = OperationOutput<ImageOperations["scan"]>;
export type ImageRenderer = (
  id: string,
  large: boolean,
  signal?: AbortSignal,
) => Promise<string>;

/** Native query failures are records, while other calls may throw Error objects. */
export function message(failure: unknown): string {
  if (typeof failure === "object" && failure !== null && "message" in failure) {
    return String(failure.message);
  }
  return String(failure);
}
