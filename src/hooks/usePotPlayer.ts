import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { createPotPlayerSession } from "./potPlayerSession";

export function usePotPlayer(saveProgress: (videoId: string, position: number) => Promise<void>) {
  const [error, setError] = useState<string | null>(null);
  const controller = useMemo(() => createPotPlayerSession({
    launch: api.launchPotplayer,
    poll: api.potplayerStatus,
    save: saveProgress,
    report: setError,
    schedule: (callback) => setTimeout(callback, 10000),
    cancel: clearTimeout,
  }), [saveProgress]);

  useEffect(() => () => controller.stop(), [controller]);
  const clearError = useCallback(() => setError(null), []);
  return { launch: controller.launch, error, clearError };
}
