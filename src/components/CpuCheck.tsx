import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Dialog } from "../ui";

interface CpuSupport {
  supported: boolean;
  missing: string[];
}

/**
 * Transcription and face detection need AVX2, FMA, F16C and SSE4.2-class instructions
 * (src-tauri/src/cpu.rs). On an older CPU, explain that once at startup instead of
 * failing later.
 */
export function CpuCheck() {
  const [missing, setMissing] = useState<string[] | null>(null);

  useEffect(() => {
    invoke<CpuSupport>("cpu_support").then(
      (s) => !s.supported && setMissing(s.missing),
      () => {},
    );
  }, []);

  if (!missing) return null;
  return (
    <Dialog
      title="This CPU isn't supported"
      onClose={() => setMissing(null)}
      width={460}
      footer={
        <>
          <Button variant="secondary" onClick={() => setMissing(null)}>
            Continue anyway
          </Button>
          <Button variant="primary" onClick={() => invoke("quit_app")}>
            Quit
          </Button>
        </>
      }
    >
      <p className="cpu-check-text">
        Transcription and speaker tracking need a processor with {missing.join(", ")}, which almost all Intel and
        AMD processors from 2013 on have. This computer's processor doesn't, so KreoKit Clips can't transcribe or
        reframe videos here.
      </p>
      <p className="cpu-check-text">You can still open videos.</p>
    </Dialog>
  );
}
