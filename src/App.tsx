import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CpuCheck } from "./components/CpuCheck";
import { Home } from "./components/Home";
import { ProjectView } from "./components/ProjectView";
import { chooseVideo } from "./components/TopBar";
import { flushProject, openVideoPath, useProject } from "./store/project";
import { ToastRegion } from "./ui";

function App() {
  const hasProject = useProject((s) => s.project !== null);

  // ⌘O / Ctrl+O opens a video from any screen.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && e.key.toLowerCase() === "o") {
        e.preventDefault();
        void chooseVideo();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Save the last edit before the window closes (the autosave waits a moment).
  useEffect(() => {
    const unlisten = getCurrentWindow().onCloseRequested(() => flushProject());
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // Development aid: KREOKIT_DEV_OPEN=/path/video.mp4 opens it at startup.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    invoke<string | null>("dev_startup_video").then((path) => path && void openVideoPath(path));
  }, []);

  return (
    <>
      {hasProject ? <ProjectView /> : <Home />}
      <ToastRegion />
      <CpuCheck />
    </>
  );
}

export default App;
