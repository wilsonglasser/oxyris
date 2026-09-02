import { useCallback, useRef, useState } from "react";

const carriesFiles = (e: React.DragEvent) =>
  Array.from(e.dataTransfer.types).includes("Files");

/**
 * OS file drag-and-drop for a composer surface.
 *
 * The Tauri window runs with `dragDropEnabled: false` (see
 * `apps/desktop/tauri.conf.json`), so the webview receives plain HTML5 drop
 * events instead of Tauri's native `tauri://drag-drop`. That keeps the app's
 * internal HTML5 drags (sidebar reorder, multi-view panes) working, at the cost
 * of the dropped file's real path: the webview only hands us the *bytes*. The
 * caller is expected to persist them (`attachmentSave`) and reference the copy.
 *
 * Only drags that actually carry files are intercepted — an internal reorder
 * drag carries `text/plain` and passes straight through to its own handlers.
 */
export function useFileDrop(
  onFiles: (files: File[]) => void,
  options: { disabled?: boolean } = {},
) {
  const { disabled = false } = options;
  const [dragging, setDragging] = useState(false);
  // dragenter/dragleave fire for every child element the pointer crosses, so a
  // boolean flips off while still inside the zone. Count the nesting instead.
  const depth = useRef(0);

  const onDragEnter = useCallback(
    (e: React.DragEvent) => {
      if (disabled || !carriesFiles(e)) return;
      e.preventDefault();
      depth.current += 1;
      setDragging(true);
    },
    [disabled],
  );

  const onDragOver = useCallback(
    (e: React.DragEvent) => {
      if (disabled || !carriesFiles(e)) return;
      // Without preventDefault the drop never fires and Chromium navigates the
      // webview to the dropped `file://` URL — which unmounts the whole app.
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
    },
    [disabled],
  );

  const onDragLeave = useCallback(
    (e: React.DragEvent) => {
      if (disabled || !carriesFiles(e)) return;
      depth.current = Math.max(0, depth.current - 1);
      if (depth.current === 0) setDragging(false);
    },
    [disabled],
  );

  const onDrop = useCallback(
    (e: React.DragEvent) => {
      if (disabled || !carriesFiles(e)) return;
      e.preventDefault();
      e.stopPropagation();
      depth.current = 0;
      setDragging(false);
      // A dropped *folder* arrives as a zero-typed, zero-sized File; the entry
      // API is the only way to tell it apart. Silently skipped — reading a tree
      // through the webview would mean uploading it file by file.
      const items = Array.from(e.dataTransfer.items ?? []);
      const isDir = (i: number) =>
        items[i]?.webkitGetAsEntry?.()?.isDirectory ?? false;
      const files = Array.from(e.dataTransfer.files).filter(
        (_, i) => !isDir(i),
      );
      if (files.length > 0) onFiles(files);
    },
    [disabled, onFiles],
  );

  return {
    dragging,
    dropProps: { onDragEnter, onDragOver, onDragLeave, onDrop },
  };
}
