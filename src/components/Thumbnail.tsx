import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Film } from "lucide-react";
import { projectThumbnail } from "../lib/projects";

/** The project's video frame (made by FFmpeg on first request), or a film icon. */
export function Thumbnail({ id, className }: { id: string; className?: string }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    projectThumbnail(id).then(
      (path) => live && setSrc(convertFileSrc(path)),
      () => live && setSrc(null),
    );
    return () => {
      live = false;
    };
  }, [id]);
  return (
    <span className={["thumb", className].filter(Boolean).join(" ")}>
      {src ? <img src={src} alt="" /> : <Film size={22} strokeWidth={1.75} aria-hidden />}
    </span>
  );
}
