import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronDown, Plus } from "lucide-react";
import { useProjectStore } from "~/stores/projectStore.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { ProjectBadge } from "~/components/ProjectBadge.tsx";

/**
 * Title-bar project switcher: the active project's logo + name as a trigger,
 * opening a dropdown to switch projects. The first item is "New chat" (defers
 * to the caller's project picker) — the rest switches the active project and
 * drops the active session, mirroring the sidebar's project click.
 */
export function ProjectSwitcher({ onNewChat }: { onNewChat: () => void }) {
  const { t } = useTranslation("common");
  const projects = useProjectStore((s) => s.projects);
  const activeId = useProjectStore((s) => s.activeId);
  const setActive = useProjectStore((s) => s.setActive);
  const setActiveSession = useSessionStore((s) => s.setActive);
  const active = projects.find((p) => p.id === activeId) ?? null;

  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  if (!active) return null;

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-label={t("project_switcher.label")}
        className="flex max-w-[40vw] items-center gap-1.5 rounded-md px-1.5 py-1 text-neutral-200 transition hover:bg-neutral-800"
      >
        <ProjectBadge
          name={active.name}
          projectId={active.id}
          logoPath={active.logo_path}
          size={16}
        />
        <span className="truncate text-[11px] font-medium">{active.name}</span>
        <ChevronDown className="size-3 shrink-0 text-neutral-500" strokeWidth={2} />
      </button>

      {open && (
        <div className="absolute left-1/2 top-full z-50 mt-1 max-h-[70vh] w-64 -translate-x-1/2 overflow-y-auto rounded-lg border border-neutral-800 bg-neutral-900 p-1 shadow-2xl shadow-black/50">
          <button
            type="button"
            onClick={() => {
              setOpen(false);
              onNewChat();
            }}
            className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12px] font-medium text-neutral-100 transition hover:bg-neutral-800"
          >
            <span className="flex size-5 items-center justify-center rounded bg-neutral-800 text-neutral-300">
              <Plus className="size-3.5" strokeWidth={2} />
            </span>
            {t("project_switcher.new_chat")}
          </button>
          <div className="my-1 h-px bg-neutral-800" />
          <ul className="flex flex-col gap-0.5">
            {projects.map((p) => {
              const isActive = p.id === activeId;
              return (
                <li key={p.id}>
                  <button
                    type="button"
                    onClick={() => {
                      setActive(p.id);
                      setActiveSession(null);
                      setOpen(false);
                    }}
                    className={`flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition ${
                      isActive ? "bg-neutral-800/70" : "hover:bg-neutral-800/50"
                    }`}
                  >
                    <ProjectBadge
                      name={p.name}
                      projectId={p.id}
                      logoPath={p.logo_path}
                      size={18}
                    />
                    <span className="min-w-0 flex-1 truncate text-[12px] text-neutral-200">
                      {p.name}
                    </span>
                    {isActive && (
                      <Check
                        className="size-3.5 shrink-0 text-emerald-400"
                        strokeWidth={2}
                      />
                    )}
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}
