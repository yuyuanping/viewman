import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { PotPlayerStatus } from "../types";
import { createPotPlayerSession } from "./potPlayerSession";

export function usePotPlayer(saveProgress: (videoId: string, position: number) => Promise<void>) {
  const [error, setError] = useState<string | null>(null);
  const controller = useMemo(() => createPotPlayerSession({
    launch: (videoPath, seek) => invoke("launch_potplayer", { videoPath, seek }),
    poll: (videoPath) => invoke<PotPlayerStatus>("potplayer_status", { videoPath }),
    save: saveProgress,
    report: setError,
    schedule: (callback) => setTimeout(callback, 10000),
    cancel: clearTimeout,
  }), [saveProgress]);

  useEffect(() => () => controller.stop(), [controller]);
  const clearError = useCallback(() => setError(null), []);
  return { launch: controller.launch, error, clearError };
}
