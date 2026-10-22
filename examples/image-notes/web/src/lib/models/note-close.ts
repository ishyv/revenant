import { get } from "svelte/store";
import type { NoteEditor } from "./note-editor.ts";
import type { SaveChangesDecision } from "@revenant/client";

/** Reuse the editor's save drain and recheck its draft at final close admission. */
export function noteCloseGuard(editor: NoteEditor, choose: () => Promise<SaveChangesDecision>) {
  let admitted: { id?: string; draft: string; discard: boolean } | undefined;
  return {
    async decide(): Promise<boolean> {
      admitted = undefined;
      let snapshot = get(editor);
      let discard = false;
      if (snapshot.dirty || snapshot.status === "saving") {
        const decision = await choose();
        if (decision === "cancel") return false;
        discard = decision === "discard";
        if (!discard && !await editor.save()) return false;
        snapshot = get(editor);
      }
      if (!discard && (snapshot.dirty || snapshot.status === "saving")) return false;
      admitted = { id: snapshot.asset?.id, draft: snapshot.draft, discard };
      return true;
    },
    canClose(): boolean {
      const snapshot = get(editor);
      return !!admitted && admitted.id === snapshot.asset?.id && admitted.draft === snapshot.draft
        && (admitted.discard || (!snapshot.dirty && snapshot.status !== "saving"));
    },
  };
}
