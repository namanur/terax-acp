import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";

interface DebugMessage {
  timestamp: number;
  direction: "Sent" | "Received" | "Error";
  raw_content: string;
  byte_size: number;
  agent_id: string;
  session_id: string | null;
}

interface DebugBufferStats {
  message_count: number;
  byte_count: number;
  max_bytes: number;
  usage_percent: number;
}

export function AcpDebugPanel() {
  const [messages, setMessages] = useState<DebugMessage[]>([]);
  const [stats, setStats] = useState<DebugBufferStats | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key === "D") {
        setVisible((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const refresh = async () => {
    try {
      const msgs = await invoke<DebugMessage[]>("agent_debug_messages");
      setMessages(msgs);
      const st = await invoke<DebugBufferStats>("agent_debug_stats");
      setStats(st);
    } catch (e) {
      console.error(e);
    }
  };

  useEffect(() => {
    if (visible) {
      void refresh();
      const i = setInterval(() => void refresh(), 2000);
      return () => clearInterval(i);
    }
  }, [visible]);

  const handleEnable = async () => {
    await invoke("agent_debug_enable");
    setEnabled(true);
    await refresh();
  };

  const handleDisable = async () => {
    await invoke("agent_debug_disable");
    setEnabled(false);
    await refresh();
  };

  const handleClear = async () => {
    await invoke("agent_debug_clear");
    await refresh();
  };

  if (!visible) return null;

  return (
    <div className="fixed inset-4 z-50 rounded-xl border border-border bg-background shadow-2xl flex flex-col p-4">
      <div className="flex items-center justify-between mb-4">
        <h2 className="text-lg font-bold">ACP Debug Inspector</h2>
        <div className="flex items-center gap-2">
          {enabled ? (
            <Button size="sm" variant="destructive" onClick={handleDisable}>
              Disable
            </Button>
          ) : (
            <Button size="sm" onClick={handleEnable}>
              Enable
            </Button>
          )}
          <Button size="sm" variant="outline" onClick={handleClear}>
            Clear
          </Button>
          <Button size="sm" variant="outline" onClick={() => setVisible(false)}>
            Close
          </Button>
        </div>
      </div>
      
      {stats && (
        <div className="text-xs mb-4 text-muted-foreground">
          {stats.message_count} messages | {(stats.byte_count / 1024).toFixed(1)} KB / {(stats.max_bytes / 1024).toFixed(1)} KB ({stats.usage_percent.toFixed(1)}%)
        </div>
      )}

      <div className="flex-1 overflow-auto flex flex-col gap-2">
        {messages.map((m, i) => (
          <div key={i} className="rounded border bg-muted/30 p-2 text-xs font-mono">
            <div className="flex justify-between mb-1 opacity-70">
              <span>{new Date(m.timestamp).toLocaleTimeString()} - {m.direction} - {m.agent_id}</span>
              <span>{m.byte_size} bytes</span>
            </div>
            <pre className="whitespace-pre-wrap">{m.raw_content}</pre>
          </div>
        ))}
      </div>
    </div>
  );
}
