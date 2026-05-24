import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Search } from "lucide-react";
import type { ProjectRow } from "~/ipc/commands.ts";
import { useProjectStore } from "~/stores/projectStore.ts";
import { Modal } from "~/components/Modal.tsx";
import { ProjectBadge } from "~/components/ProjectBadge.tsx";

function envLabel(p: ProjectRow): string {
  return p.environment.kind === "windows"
    ? "Windows"
    : `WSL · ${p.environment.distro}`;
}

/**
 * Project picker for "New chat". Because the app no longer pins everything to
 * a single active project, starting a thread first asks which project to scope
 * it to. Picking one fires `onPick(project)`; the caller decides what to do
 * (land on the empty composer, seed a Multi View pane, …).
 */
export function NewChatModal({
  open,
  onClose,
  onPick,
}: {
  open: boolean;
  onClose: () => void;
  onPick: (project: ProjectRow) => void;
}) {
  const { t } = useTranslation("common");
  const projects = useProjectStore((s) => s.projects);
  const [query, setQuery] = useState("");

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return projects;
    return projects.filter((p) => p.name.toLowerCase().includes(needle));
  }, [projects, query]);

  return (
    <Modal open={open} onClose={onClose} closeLabel={t("sidebar.close")}>
      <div className="p-3">
        <h2 className="pr-8 text-sm font-semibold text-neutral-100">
          {t("new_chat.title")}
        </h2>
        <p className="mt-0.5 text-[12px] text-neutral-400">
          {t("new_chat.subtitle")}
        </p>

        <div className="relative mt-3">
          <Search
            className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-neutral-500"
            strokeWidth={1.75}
          />
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && filtered[0]) {
                e.preventDefault();
                onPick(filtered[0]);
              }
            }}
            placeholder={t("new_chat.search_placeholder")}
            className="h-8 w-full rounded-lg border border-neutral-800 bg-neutral-900 pl-7 pr-2 text-[12px] text-neutral-200 placeholder:text-neutral-600 outline-none focus:border-neutral-700"
          />
        </div>

        <div className="mt-3 max-h-[50vh] overflow-y-auto">
          {filtered.length === 0 ? (
            <p className="px-1 py-6 text-center text-[12px] text-neutral-500">
              {t("new_chat.empty")}
            </p>
          ) : (
            <ul className="flex flex-col gap-0.5">
              {filtered.map((p) => (
                <li key={p.id}>
                  <button
                    type="button"
                    onClick={() => onPick(p)}
                    className="flex w-full items-center gap-2.5 rounded-lg px-2 py-2 text-left transition hover:bg-neutral-800/60"
                  >
                    <ProjectBadge
                      name={p.name}
                      projectId={p.id}
                      logoPath={p.logo_path}
                      size={28}
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13px] font-medium text-neutral-100">
                        {p.name}
                      </span>
                      <span className="block truncate text-[11px] text-neutral-500">
                        {envLabel(p)}
                      </span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </Modal>
  );
}
