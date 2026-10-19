import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Bug, Ellipsis, FolderOpen, House } from "lucide-react";
import mark from "../assets/kreokit-mark.png";
import { VIDEO_EXTENSIONS } from "../lib/projects";
import { openVideoPath } from "../store/project";
import { Menu, MenuEntry, toast } from "../ui";

/** Shows the open-file dialog for one video. Resolves to the picked path, or null. */
export async function pickVideo(title = "Open video"): Promise<string | null> {
  const picked = await open({
    title,
    multiple: false,
    directory: false,
    filters: [{ name: "Video", extensions: VIDEO_EXTENSIONS }],
  });
  return typeof picked === "string" ? picked : null;
}

/** Picks a video and opens its project. */
export async function chooseVideo() {
  const path = await pickVideo();
  if (path) await openVideoPath(path);
}

/**
 * The 48 px app bar: logo mark, title, the ⋯ menu and screen actions (`children`).
 * Empty parts are window drag regions; macOS draws the window buttons inside it.
 */
export function TopBar({
  title,
  onHome,
  children,
}: {
  title: React.ReactNode;
  /** Adds "Back to Home" to the menu. */
  onHome?: () => void;
  children?: React.ReactNode;
}) {
  const items: MenuEntry[] = [
    { label: "Open video…", icon: FolderOpen, shortcut: "⌘O", onSelect: () => void chooseVideo() },
    ...(onHome ? [{ label: "Back to Home", icon: House, onSelect: onHome }] : []),
    "separator",
    {
      label: "Report a problem",
      icon: Bug,
      onSelect: () => void invoke("report_problem").catch((e) => toast(String(e), "error")),
    },
  ];
  return (
    <header className="topbar" data-tauri-drag-region>
      <img src={mark} alt="" className="topbar-mark" width={24} height={24} data-tauri-drag-region />
      <div className="topbar-title" data-tauri-drag-region>
        {title}
      </div>
      <div className="topbar-actions">
        <Menu buttonLabel="App menu" icon={Ellipsis} variant="ghost" align="end" items={items} />
        {children}
      </div>
    </header>
  );
}
