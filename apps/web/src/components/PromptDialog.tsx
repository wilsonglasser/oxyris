import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

interface Props {
  /** Question shown above the field, e.g. "New file name:". */
  title: string;
  /** Pre-filled value. Selected on open so typing replaces it. */
  initial?: string;
  /** Extra hint under the field. */
  help?: string;
  onSubmit: (value: string) => void;
  onClose: () => void;
}

/**
 * Single-field modal that stands in for `window.prompt`, which is a no-op in
 * WebView2 (every flow that needs a string from the user has to render its
 * own input). Render it conditionally — it has no `open` prop, so mounting is
 * what opens it and the field state resets on every open.
 *
 * Enter submits, Escape / backdrop click cancels. Submitting an empty value is
 * blocked, so a caller only ever sees a non-empty (untrimmed) string.
 */
export function PromptDialog({
  title,
  initial = "",
  help,
  onSubmit,
  onClose,
}: Props) {
  const { t } = useTranslation("common");
  const [value, setValue] = useState(initial);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    inputRef.current?.select();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    // Capture phase: panels underneath (terminal, editor) also listen for
    // Escape, and the dialog must win.
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const submit = () => {
    if (!value.trim()) return;
    onSubmit(value);
  };

  return (
    <div
      className="fixed inset-0 z-[60] flex items-start justify-center bg-black/60 p-6 backdrop-blur-sm"
      onMouseDown={onClose}
    >
      <div
        onMouseDown={(e) => e.stopPropagation()}
        className="mt-32 w-full max-w-sm rounded-lg border border-neutral-800 bg-neutral-950 p-3 shadow-2xl"
      >
        <label className="mb-1.5 block text-[12px] text-neutral-300">
          {title}
        </label>
        <input
          ref={inputRef}
          type="text"
          autoFocus
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") submit();
          }}
          className="w-full rounded bg-neutral-900 px-2 py-1.5 text-[12px] text-neutral-100 outline-none ring-1 ring-neutral-700 focus:ring-emerald-700"
        />
        {help && <p className="mt-1.5 text-[10px] text-neutral-500">{help}</p>}
        <div className="mt-3 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded border border-neutral-700 px-2.5 py-1 text-[11px] text-neutral-300 hover:bg-neutral-800"
          >
            {t("prompt_dialog.cancel")}
          </button>
          <button
            type="button"
            onClick={submit}
            disabled={!value.trim()}
            className="rounded border border-emerald-800 bg-emerald-900/40 px-2.5 py-1 text-[11px] text-emerald-200 enabled:hover:bg-emerald-900/70 disabled:cursor-default disabled:opacity-40"
          >
            {t("prompt_dialog.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
