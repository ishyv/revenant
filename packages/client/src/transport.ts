/** Native IPC boundary. JavaScript projects resources; the desktop host owns them. */
import { Channel, invoke } from "@tauri-apps/api/core";
import { CapabilityError, errorOf } from "./contracts.js";

/** Show a native Yes/No dialog; only an explicit Yes approves the action.
 * Await the answer before mutating state. A failed dialog rejects the promise.
 */
export async function confirm(message: string): Promise<boolean> {
  // Use the current message command: some plugin versions still install a
  // window.confirm wrapper that invokes the removed legacy confirm command.
  const answer = await invoke<string>("plugin:dialog|message", {
    message,
    buttons: "YesNo",
  });
  return answer === "Yes";
}

/** A dispatcher request. Ownership and admission are documented in docs/ARCHITECTURE.md. */
export interface NativeRequest {
  /** Native dispatcher action selecting the requested capability operation. */
  action: string;
  /** Native caller scope, omitted only for root and child-scope creation. */
  scope?: string;
  /** Action-specific wire fields validated by native capability admission. */
  [field: string]: unknown;
}

/** Injectable IPC boundary for host integration, never browser execution providers. */
export interface NativeTransport {
  /** Invoke the native dispatcher and optionally subscribe to direct snapshot updates. */
  dispatch<T, U = T>(
    request: NativeRequest,
    updates?: (snapshot: U) => void,
  ): Promise<T>;
}

/** Tauri 2 transport: one command and a direct snapshot Channel per subscription. */
export class TauriTransport implements NativeTransport {
  private developmentScope?: string;
  private reportCount = 0;
  private errorListener = (event: ErrorEvent): void => {
    this.report({ kind: "error", message: event.message.slice(0, 4096), file: event.filename.slice(0, 1024), line: event.lineno, column: event.colno });
  };
  private rejectionListener = (event: PromiseRejectionEvent): void => {
    const error = errorOf(event.reason, "unhandled_rejection");
    this.report({ kind: "unhandledRejection", code: error.code.slice(0, 256), message: error.message.slice(0, 4096) });
  };
  private report(snapshot: Record<string, unknown>): void {
    if (!this.developmentScope || this.reportCount++ >= 32) return;
    const context = typeof window === "undefined" ? {} : {
      route: window.location.pathname.slice(0, 1024),
      title: typeof document === "undefined" ? "" : document.title.slice(0, 256),
      visibility: typeof document === "undefined" ? "unknown" : document.visibilityState,
    };
    void this.dispatch({ action: "frontend.snapshot", scope: this.developmentScope,
      snapshot: { ...context, ...snapshot } }).catch(() => {});
  }
  private closeReports(): void {
    if (typeof window !== "undefined") {
      window.removeEventListener("error", this.errorListener);
      window.removeEventListener("unhandledrejection", this.rejectionListener);
    }
    this.developmentScope = undefined;
  }
  /** Invoke the Tauri dispatcher with an optional direct snapshot Channel. */
  async dispatch<T, U = T>(
    request: NativeRequest,
    updates?: (snapshot: U) => void,
  ): Promise<T> {
    const channel = updates ? new Channel<U>(updates) : undefined;
    try {
      const result = await invoke<T>("revenant_dispatch", {
        request: { ...request, requestId: crypto.randomUUID() },
        ...(channel ? { updates: channel } : {}),
      });
      if (request.action === "root.connect" && typeof window !== "undefined") {
        const connection = result as { scope?: string; development?: boolean };
        if (connection.development && connection.scope) {
          this.closeReports();
          this.developmentScope = connection.scope;
          this.reportCount = 0;
          window.addEventListener("error", this.errorListener);
          window.addEventListener("unhandledrejection", this.rejectionListener);
          this.report({ kind: "connected" });
        }
      }
      if (request.action === "scope.dispose" && request.scope === this.developmentScope)
        this.closeReports();
      return result;
    } catch (error) {
      const failed = errorOf(error, "native_dispatch_failed");
      throw new CapabilityError(
        failed.code,
        failed.message,
        failed.details,
        failed.retryable,
      );
    }
  }
}
