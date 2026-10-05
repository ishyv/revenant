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

/** A dispatcher request. Capability fields are documented in docs/WIRE.md. */
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
  /** Invoke the Tauri dispatcher with an optional direct snapshot Channel. */
  async dispatch<T, U = T>(
    request: NativeRequest,
    updates?: (snapshot: U) => void,
  ): Promise<T> {
    const channel = updates ? new Channel<U>(updates) : undefined;
    try {
      return await invoke<T>("revenant_dispatch", {
        request,
        ...(channel ? { updates: channel } : {}),
      });
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
