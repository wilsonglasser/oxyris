interface Props {
  name: string;
  size?: number;
}

export function ProjectBadge({ name, size = 20 }: Props) {
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
