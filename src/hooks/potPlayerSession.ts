interface Status {
  running: boolean;
  position: number | null;
  position_source: "live" | "remembered" | null;
  state: "running" | "unknown" | "stopped";
}

interface SessionVideo {
  id: string;
  path: string;
  duration: number | null;
}

interface Ports {
  launch: (path: string, seek: number | null) => Promise<unknown>;
  poll: (path: string) => Promise<Status>;
  save: (id: string, position: number) => Promise<void>;
  report: (error: string | null) => void;
  schedule: (callback: () => void) => ReturnType<typeof setTimeout>;
  cancel: (timer: ReturnType<typeof setTimeout>) => void;
}

export function createPotPlayerSession(ports: Ports) {
  let current: { video: SessionVideo; last: number | null; misses: number } | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let launches: Promise<unknown> = Promise.resolve();

  function stop() {
    current = null;
    if (timer !== null) ports.cancel(timer);
    timer = null;
  }

  async function launch(video: SessionVideo, seek: number | null) {
    stop();
    const session = { video, last: null as number | null, misses: 0 };
    current = session;
    ports.report(null);
    const active = () => current === session;

    async function poll() {
      timer = null;
      if (!active()) return;
      try {
        const status = await ports.poll(video.path);
        if (!active()) return;
        if (status.state === "stopped") {
          stop();
          return;
        }
        if (status.position_source === "live" && status.position !== null &&
            Number.isFinite(status.position) && status.position >= 0) {
          const position = video.duration !== null && video.duration > 0
            ? Math.min(status.position, video.duration) : status.position;
          if (position !== session.last) {
            await ports.save(video.id, position);
            if (!active()) return;
            session.last = position;
          }
          session.misses = 0;
        } else {
          session.misses++;
        }
      } catch (error) {
        if (!active()) return;
        session.misses++;
        ports.report(`无法同步外部播放器进度：${String(error)}`);
      }
      if (!active()) return;
      if (session.misses >= 6) {
        ports.report("无法确认 PotPlayer 的实时位置，已停止跟踪并保留最后保存的进度。请检查播放器标题栏时间显示设置。");
        stop();
      } else {
        timer = ports.schedule(() => { void poll(); });
      }
    }

    try {
      // Launches are ordered so a late process launch cannot replace a newer selection.
      const pending = launches.catch(() => {}).then(() => {
        if (active()) return ports.launch(video.path, seek);
      });
      launches = pending;
      await pending;
      if (!active()) return;
      // 用已知的真实进度（数据库里的上次位置）刷新播放记录；不造任何估算数据
      if (seek !== null && seek > 0) {
        try { await ports.save(video.id, seek); } catch { /* 首条实时进度由轮询写入 */ }
        if (!active()) return;
      }
      if (active()) timer = ports.schedule(() => { void poll(); });
    } catch (error) {
      if (!active()) return;
      stop();
      ports.report(`无法启动 PotPlayer：${String(error)}`);
    }
  }

  return { launch, stop };
}
