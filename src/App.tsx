import { useState, useMemo } from "react";
import { Sidebar } from "./components/Sidebar";
import { VideoGrid } from "./components/VideoGrid";
import { PlayerView } from "./components/PlayerView";
import { SearchBar } from "./components/SearchBar";
import { useVideos } from "./hooks/useVideos";
import { usePlayer } from "./hooks/usePlayer";
import type { Video } from "./types";

function App() {
  const { videos, progressMap, loading, scanDirectory, saveProgress } = useVideos();
  const { currentVideo, initialPosition, openPlayer, closePlayer } = usePlayer(saveProgress);
  const [searchQuery, setSearchQuery] = useState("");
  const [selectedDir, setSelectedDir] = useState<string | null>(null);

  const filteredVideos = useMemo(() => {
    let list = videos;
    if (selectedDir !== null) {
      list = list.filter(v => v.path.startsWith(selectedDir + "\\"));
    }
    if (searchQuery) {
      list = list.filter(v => v.filename.toLowerCase().includes(searchQuery.toLowerCase()));
    }
    return list;
  }, [videos, selectedDir, searchQuery]);

  const handlePlayVideo = (video: Video) => {
    const pos = progressMap[video.id] ?? 0;
    openPlayer(video, pos);
  };

  return (
    <div className="h-screen w-screen flex bg-gray-950 text-white overflow-hidden">
      <Sidebar
        videos={videos}
        selectedDir={selectedDir}
        onSelectDir={setSelectedDir}
        onScanDirectory={scanDirectory}
        loading={loading}
      />
      <main className="flex-1 flex flex-col p-4 gap-4 overflow-hidden">
        <SearchBar value={searchQuery} onChange={setSearchQuery} total={filteredVideos.length} />
        <VideoGrid videos={filteredVideos} progressMap={progressMap} onPlay={handlePlayVideo} />
      </main>
      {currentVideo && (
        <PlayerView
          video={currentVideo}
          initialPosition={initialPosition}
          onClose={closePlayer}
          onProgress={saveProgress}
        />
      )}
    </div>
  );
}

export default App;
