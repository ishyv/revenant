/** Native window close admission; resource shutdown remains owned by the desktop host. */
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import type { Unsubscribe } from "./store.js";

/** The explicit decision returned by a Save / Discard / Cancel dialog. */
export type SaveChangesDecision = "save" | "discard" | "cancel";

/** Labels for the three native decision buttons. */
export interface SaveChangesLabels {
  /** Save pending edits before closing. */
  save: string;
  /** Close without saving the pending draft. */
  discard: string;
  /** Keep the window and draft open. */
  cancel: string;
}

/** Ask for an explicit native save decision; closing the dialog means Cancel. */
export async function askSaveChanges(
  message: string,
  labels: SaveChangesLabels = { save: "Save", discard: "Discard", cancel: "Cancel" },
): Promise<SaveChangesDecision> {
  const answer = await invoke<string>("plugin:dialog|message", {
    message,
    buttons: { YesNoCancelCustom: [labels.save, labels.discard, labels.cancel] },
  });
  if (answer === labels.save || answer === "Yes") return "save";
  if (answer === labels.discard || answer === "No") return "discard";
  return "cancel";
}

/** Narrow window boundary, injectable for lifecycle tests. */
export interface DesktopCloseWindow {
  /** Register a native close event and return its unsubscribe function. */
  onCloseRequested(handler: (event: {
    /** Retain the window until the application's close decision is admitted. */
    preventDefault(): void;
  }) => void | Promise<void>): Promise<Unsubscribe>;
  /** Destroy the admitted window; native shutdown still drains its resources. */
  destroy(): Promise<void>;
}

/** Final admission checks and error reporting for a desktop close handler. */
export interface DesktopCloseOptions {
  /** Synchronously recheck pending edits immediately before native destruction. */
  canClose?: () => boolean;
  /** Report save, dialog or native-close failures while leaving the window open. */
  onError?: (error: unknown) => void;
  /** Override the current Tauri window for host lifecycle tests. */
  window?: DesktopCloseWindow;
}

/** Serialize native close requests, preserve the window on refusal/failure, and unsubscribe on teardown. */
export async function onDesktopCloseRequested(
  decide: () => boolean | Promise<boolean>,
  options: DesktopCloseOptions = {},
): Promise<Unsubscribe> {
  const window = options.window ?? getCurrentWindow();
  let alive = true;
  let pending = false;
  const unlisten = await window.onCloseRequested(async (event) => {
    event.preventDefault();
    if (!alive || pending) return;
    pending = true;
    try {
      if (await decide()) {
        if (alive && (options.canClose?.() ?? true)) await window.destroy();
      }
    } catch (error) {
      options.onError?.(error);
    } finally {
      pending = false;
    }
  });
  return () => {
    alive = false;
    unlisten();
  };
}
