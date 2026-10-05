import type { NoteEditor } from "./note-editor";

/** Serialize navigation/refresh intent behind the editor's complete save drain. */
export function coordinator(editor: Pick<NoteEditor, "save">) {
  let alive = true;
  let tail: Promise<unknown> = Promise.resolve();
  return {
    run(
      action: () => void | Promise<void>,
      confirm?: () => boolean | Promise<boolean>,
    ): Promise<boolean> {
      const next = tail.then(async () => {
        // Desktop dialogs are asynchronous. Never save or mutate before approval;
        // a dialog can also outlive the component that requested it.
        if (!alive || (confirm && !(await confirm())) || !alive) return false;
        if (!alive || !(await editor.save()) || !alive) return false;
        await action();
        return alive;
      });
      // A failed action must not poison later retries or navigation.
      tail = next.catch(() => {});
      return next;
    },
    dispose() {
      alive = false;
    },
  };
}
