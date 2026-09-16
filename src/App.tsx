import { useState, useMemo, useCallback, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { Sidebar } from "./components/Sidebar";
import { VideoGrid } from "./components/VideoGrid";
import { PlayerView } from "./components/PlayerView";
import { SearchBar } from "./components/SearchBar";
import { useVideos } from "./hooks/useVideos";
import { usePlayer } from "./hooks/usePlayer";
import { usePotPlayer } from "./hooks/usePotPlayer";
import type { Video } from "./types";

const POTPLAYER_PREF_KEY = "viewman.usePotPlayer";

interface ScanProgressPayload {
  processed: number;
  total: number;
  done: boolean;
}

function App() {
  const { videos, progressMap, recentlyPlayed, loading, scanDirectory, saveProgress, loadVideos } = useVideos();
  const { currentVideo, initialPosition, openPlayer, closePlayer } = usePlayer();
  const { launch: launchInPotPlayer, error: potPlayerError, clearError: clearPotPlayerError } = usePotPlayer(saveProgress);

  const [searchQuery, setSearchQuery] = useState("");
  const [selectedDir, setSelectedDir] = useState<string | null>(null);
  const [useExternalPlayer, setUseExternalPlayer] = useState(() => localStorage.getItem(POTPLAYER_PREF_KEY) === "1");
  const [scanProgress, setScanProgress] = useState<{ processed: number; total: number } | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<ScanProgressPayload>("scan-progress", (event) => {
      const { processed, total, done } = event.payload;
      setScanProgress(done ? null : { processed, total });
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, []);

  const togglePotPlayer = useCallback(() => {
    setUseExternalPlayer((prev) => {
      const next = !prev;
      localStorage.setItem(POTPLAYER_PREF_KEY, next ? "1" : "0");
      return next;
    });
  }, []);

  const seekFor = useCallback((video: Video): number | null => {
    const pos = progressMap[video.id] ?? 0;
    return pos > 0 ? pos : null;
  }, [progressMap]);

  const handlePlayVideo = useCallback((video: Video) => {
    if (useExternalPlayer) {
      launchInPotPlayer(video, seekFor(video));
    } else {
      openPlayer(video, progressMap[video.id] ?? 0);
    }
  }, [useExternalPlayer, progressMap, openPlayer, launchInPotPlayer, seekFor]);

  const handlePlayById = useCallback((videoId: string, position: number) => {
    const video = videos.find((v) => v.id === videoId);
    if (!video) return;
    if (useExternalPlayer) {
      launchInPotPlayer(video, position > 0 ? position : null);
    } else {
      openPlayer(video, position);
    }
  }, [videos, useExternalPlayer, openPlayer, launchInPotPlayer]);

  const handleFallbackToPotPlayer = useCallback((video: Video) => {
    closePlayer();
    launchInPotPlayer(video, seekFor(video));
  }, [closePlayer, launchInPotPlayer, seekFor]);

  const filteredVideos = useMemo(() => {
    let list = videos;
    if (selectedDir !== null) {
      list = list.filter((v) => v.path.startsWith(selectedDir + "\\"));
    }
    if (searchQuery) {
      const q = searchQuery.toLowerCase();
      list = list.filter((v) => v.filename.toLowerCase().includes(q));
    }
    return list;
  }, [videos, selectedDir, searchQuery]);

  return (
    <div className="h-screen w-screen flex bg-gray-950 text-white overflow-hidden">
      <Sidebar
        videos={videos}
        recentlyPlayed={recentlyPlayed}
        selectedDir={selectedDir}
        onSelectDir={setSelectedDir}
        onScanDirectory={scanDirectory}
        onPlayVideo={handlePlayById}
        loading={loading}
        scanProgress={scanProgress}
        usePotPlayer={useExternalPlayer}
        onTogglePotPlayer={togglePotPlayer}
      />
      <main className="flex-1 flex flex-col p-4 gap-4 overflow-hidden">
        <SearchBar value={searchQuery} onChange={setSearchQuery} total={filteredVideos.length} />
        {potPlayerError && (
          <div className="bg-red-900/80 text-red-200 px-3 py-2 rounded text-sm flex justify-between items-center">
            <span>{potPlayerError}</span>
            <button onClick={clearPotPlayerError} className="text-red-300 hover:text-white ml-2">✕</button>
          </div>
        )}
        <VideoGrid videos={filteredVideos} progressMap={progressMap} onPlay={handlePlayVideo} onDeleted={loadVideos} />
      </main>
      {!useExternalPlayer && currentVideo && (
        <PlayerView
          video={currentVideo}
          initialPosition={initialPosition}
          onClose={closePlayer}
          onProgress={saveProgress}
          onFallback={handleFallbackToPotPlayer}
        />
      )}
    </div>
  );
}

export default App;
