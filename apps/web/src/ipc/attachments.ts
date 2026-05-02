import { invoke } from "@tauri-apps/api/core";

export type AttachmentInfo = {
  path: string;
  filename: string;
  mime: string;
  size: number;
};

export async function attachmentSave(input: {
  bucket_id: string;
  mime: string;
  data_base64: string;
  filename?: string;
}): Promise<AttachmentInfo> {
  return invoke("attachment_save", { input });
}

/** Read a `File` / `Blob` as raw base64 (no data-URL prefix). */
export async function blobToBase64(blob: Blob): Promise<string> {
  const buf = await blob.arrayBuffer();
  const bytes = new Uint8Array(buf);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 1) {
    binary += String.fromCharCode(bytes[i]!);
  }
  // `btoa` is safe here — we feed it bytes, not arbitrary unicode text.
  return window.btoa(binary);
}
