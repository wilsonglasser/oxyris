import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type InstallSource = "managed" | "path";

export type InstallStatus =
  | { kind: "installed"; source: InstallSource; path: string }
  | { kind: "not_installed" }
  | { kind: "installing"; progress: number }
  | { kind: "failed"; message: string };

export type LanguagePack = {
  id: string;
  display_name: string;
  description: string;
  lsp_language: string;
  install_method: "github_release" | "npm_global" | "manual";
  status: InstallStatus;
};

export type PackProgressEvent =
  | { phase: "started"; id: string }
  | { phase: "progress"; id: string; bytes: number; total: number | null }
  | { phase: "done"; id: string; path: string }
  | { phase: "failed"; id: string; message: string };

// Backend serializes `InstallStatus` as a tagged enum with a literal `kind`.
// The "kind" field gets renamed by serde's `rename_all = "snake_case"` on
// the parent enum, but each variant is also flattened — so we receive
// `{installed: {...}}` patterns. Normalize to a flat shape for easier UI.
type RawStatus =
  | { installed: { source: InstallSource; path: string } }
  | "not_installed"
  | { installing: { progress: number } }
  | { failed: { message: string } };

type RawPack = Omit<LanguagePack, "status"> & { status: RawStatus };

function normalize(raw: RawPack): LanguagePack {
  const s = raw.status;
  let status: InstallStatus;
  if (s === "not_installed") {
    status = { kind: "not_installed" };
  } else if ("installed" in s) {
    status = { kind: "installed", ...s.installed };
  } else if ("installing" in s) {
    status = { kind: "installing", progress: s.installing.progress };
  } else {
    status = { kind: "failed", message: s.failed.message };
  }
  return { ...raw, status };
}

export async function languagePacksList(): Promise<LanguagePack[]> {
  const raw = await invoke<RawPack[]>("language_packs_list");
  return raw.map(normalize);
}

export async function languagePacksInstall(id: string): Promise<void> {
  await invoke("language_packs_install", { input: { id } });
}

export async function languagePacksUninstall(id: string): Promise<void> {
  await invoke("language_packs_uninstall", { input: { id } });
}

export async function languagePacksInstallInWsl(
  id: string,
  distro: string,
): Promise<string> {
  return invoke<string>("language_packs_install_in_wsl", {
    input: { id, distro },
  });
}

export async function wslDistros(): Promise<string[]> {
  return invoke<string[]>("wsl_distros");
}

export async function onLanguagePackStatus(
  cb: (event: PackProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<PackProgressEvent>("language_pack:status", (e) => cb(e.payload));
}
