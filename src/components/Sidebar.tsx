import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";

interface SidebarProps {
  onScanDirectory: (dir: string) => Promise<void>;
  loading: boolean;
}

export function Sidebar({ onScanDirectory, loading }: SidebarProps) {
  const [ffprobeOk, setFfprobeOk] = useState<boolean | null>(null);

  const handleScan = async () => {
    const dir = await open({ directory: true, multiple: false, title: "选择视频目录" });
    if (dir) {
      await onScanDirectory(dir);
    }
  };

  const handleCheckFfprobe = async () => {
    const ok = await invoke<boolean>("check_ffprobe");
    setFfprobeOk(ok);
  };

  return (
    <aside className="w-60 h-full bg-gray-900 text-white flex flex-col p-4 gap-3">
      <h1 className="text-lg font-bold mb-2">ViewMan</h1>
      <button
        onClick={handleScan}
        disabled={loading}
        className="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 py-2 px-4 rounded"
      >
        {loading ? "扫描中..." : "扫描目录"}
      </button>
      <button
        onClick={handleCheckFfprobe}
        className="bg-gray-700 hover:bg-gray-600 py-2 px-4 rounded text-sm"
      >
        检测 ffprobe
      </button>
      {ffprobeOk === true && <span className="text-green-400 text-sm">✓ ffprobe 可用</span>}
      {ffprobeOk === false && <span className="text-red-400 text-sm">✗ ffprobe 未安装</span>}
    </aside>
  );
}
