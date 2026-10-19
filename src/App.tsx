import { CpuCheck } from "./components/CpuCheck";
import { TopBar } from "./components/TopBar";
import { ToastRegion } from "./ui";

function App() {
  return (
    <>
      <div className="screen">
        <TopBar title={<span className="topbar-name">KreoKit Clips</span>} />
        <main />
      </div>
      <ToastRegion />
      <CpuCheck />
    </>
  );
}

export default App;
