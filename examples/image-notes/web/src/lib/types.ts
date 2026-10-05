import type { Application } from "$lib/revenant";

// Inferred from the compiled Rust contract; no duplicate handwritten wire DTOs.
export type AssetPage = Awaited<
  ReturnType<Application["operations"]["images"]["query"]["call"]>
>;
export type Asset = AssetPage["items"][number];
export type Startup = Awaited<
  ReturnType<Application["operations"]["images"]["bootstrap"]["call"]>
>;
export type Root = Startup["roots"][number];
export type ImageRenderer = (
  id: string,
  large: boolean,
  signal?: AbortSignal,
) => Promise<string>;
