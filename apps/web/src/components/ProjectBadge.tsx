import { useEffect, useState } from "react";
import { projectLogoBytes } from "~/ipc/commands.ts";

interface Props {
  name: string;
  size?: number;
  /**
   * Optional project id — when set, the badge tries to load the project's
   * custom logo via `project_logo_bytes`. Falls back to the colored letter
   * when no logo is set or fetch fails.
   */
  projectId?: string;
  /**
   * The stored logo_path, used as a cache-buster + mount trigger so the
   * badge re-fetches when the user changes the logo.
   */
  logoPath?: string | null;
}

export function ProjectBadge({ name, size = 20, projectId, logoPath }: Props) {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    if (!projectId || !logoPath) {
      setSrc(null);
      return;
    }
    let cancelled = false;
    void projectLogoBytes({ id: projectId })
      .then((res) => {
        if (cancelled || !res) return;
        setSrc(`data:${res.mime};base64,${res.bytes_b64}`);
      })
      .catch(() => {
        if (!cancelled) setSrc(null);
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, logoPath]);

  if (src) {
    return (
      <img
        src={src}
        alt={name}
        className="shrink-0 rounded-[4px] object-cover"
        style={{ width: size, height: size }}
      />
    );
  }

  const letter = (name.trim()[0] ?? "?").toUpperCase();
  const hash = Array.from(name).reduce((acc, c) => acc + c.charCodeAt(0), 0);
  const hue = hash % 360;
  return (
    <span
      className="flex shrink-0 items-center justify-center rounded-[4px] font-semibold text-neutral-100"
      style={{
        width: size,
        height: size,
        fontSize: Math.max(9, Math.round(size * 0.5)),
        background: `hsl(${hue} 40% 35%)`,
        border: `1px solid hsl(${hue} 40% 45%)`,
      }}
    >
      {letter}
    </span>
  );
}
