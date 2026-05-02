const IMAGE_EXT = /\.(png|jpe?g|webp|gif)$/i;

export interface ParsedUserMessage {
  /** Absolute file paths of attached images, extracted from `@path` tokens. */
  images: string[];
  /** User narrative text with `@image-path` tokens stripped and trimmed. */
  text: string;
}

/**
 * Pulls `@path` image attachment tokens out of a user turn body so the UI
 * can render real thumbnails in the bubble instead of a raw path. Non-image
 * `@path` tokens are left inline (they might be file/dir references meant
 * for Claude to read literally).
 */
export function parseUserMessage(raw: string): ParsedUserMessage {
  const images: string[] = [];
  const stripped = raw.replace(/@(\S+)/g, (match, path: string) => {
    if (IMAGE_EXT.test(path)) {
      images.push(path);
      return "";
    }
    return match;
  });
  return { images, text: stripped.trim() };
}
