/** Native preview resources. URLs are returned by the host and are used verbatim. */
import { failure } from "./contracts.js";
import type { FileEntry, Handle } from "./contracts.js";
import type { Scope } from "./scope.js";
import type { Operation } from "./operation.js";

/** Native preview renderer classification. */
export type PreviewKind = "text" | "image" | "audio" | "video" | "unsupported";
/** Wire projection of an owned preview, including bounded text or a native protocol URL. */
export interface PreviewDescriptor {
  /** Opaque native preview token released through media.dispose. */
  preview: string;
  /** Native renderer category inferred for this preview. */
  kind: PreviewKind;
  /** Trusted file display name returned by native preview admission. */
  name: string;
  /** Native MIME description for the admitted preview resource. */
  mime: string;
  /** Host-formatted media URL used verbatim by the renderer. */
  url?: string;
  /** Optional bounded native text content for a text preview. */
  text?: string;
  /** Whether native text preview content was truncated. */
  truncated?: boolean;
}
/** Scoped preview owner; disposal revokes the native preview token and underlying leases. */
export class Preview {
  private closed = false;
  private cleanup?: Promise<void>;
  /** Own an admitted native preview descriptor until explicit or scope disposal. */
  constructor(
    private owner: Scope,
    /** Admitted native preview metadata and its revocable token. */
    readonly descriptor: PreviewDescriptor,
  ) {
    owner.own(this);
  }
  /** Renderer classification supplied by native media capability. */
  get kind(): PreviewKind {
    return this.descriptor.kind;
  }
  /** Trusted display name returned by the native preview capability. */
  get name(): string {
    return this.descriptor.name;
  }
  /** Native MIME description. */
  get mime(): string {
    return this.descriptor.mime;
  }
  /** Host-formatted URL, including platform-specific custom protocol conversion. */
  get url(): string | undefined {
    return this.closed ? undefined : this.descriptor.url;
  }
  /** Bounded native text preview. */
  get text(): string | undefined {
    return this.closed ? undefined : this.descriptor.text;
  }
  /** Whether the host truncated text. */
  get truncated(): boolean | undefined {
    return this.descriptor.truncated;
  }
  /** Whether native preview ownership has been released. */
  get disposed(): boolean {
    return this.closed;
  }
  /** Revoke the native token. Idempotent and safe during parent scope teardown. */
  dispose(): Promise<void> {
    if (this.cleanup) return this.cleanup;
    this.closed = true;
    this.owner.release(this);
    this.cleanup = this.owner.ready.then((scope) =>
      this.owner.transport.dispatch<void>({
        action: "media.dispose",
        scope,
        preview: this.descriptor.preview,
      }),
    );
    return this.cleanup;
  }
}

/** Scoped native preview capability. */
export class Media<Metadata = unknown> {
  /** Bind native preview admission and the compiled metadata operation to a scope. */
  constructor(
    private owner: Scope,
    /** Manifest-backed metadata binding; generated facades supply the exact compiled output type. */
    readonly readMetadata: Operation<FileEntry, Metadata>,
  ) {}
  /** Preview a full file capability; host enforces ancestry, lease and size limits. */
  async preview(file: FileEntry | Handle): Promise<Preview> {
    const handle = "handle" in file ? file.handle : file;
    const descriptor = await this.owner.request<PreviewDescriptor>({
      action: "media.preview",
      handle,
    });
    if (this.owner.disposed) {
      await this.owner.transport.dispatch({
        action: "media.dispose",
        scope: await this.owner.ready,
        preview: descriptor.preview,
      });
      failure("scope_disposed", "Preview scope was disposed while loading");
    }
    return new Preview(this.owner, descriptor);
  }
}
