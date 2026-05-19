import { emit, listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import {
  BUILTIN_ACP_AGENTS,
  loadAcpAgents,
  saveCustomAcpAgents,
  type AcpAgentServer,
} from "../lib/acpServers";

const CHANGED_EVENT = "terax://acp-agents-changed";

type AcpAgentsState = {
  hydrated: boolean;
  customAgents: AcpAgentServer[];
  all: () => AcpAgentServer[];
  hydrate: () => Promise<void>;
  upsert: (agent: AcpAgentServer) => void;
  remove: (id: string) => void;
};

let initialized = false;

function broadcast(): void {
  void emit(CHANGED_EVENT);
}

export const useAcpAgentsStore = create<AcpAgentsState>((set, get) => ({
  hydrated: false,
  customAgents: [],
  all: () => [...BUILTIN_ACP_AGENTS, ...get().customAgents],
  hydrate: async () => {
    if (initialized) return;
    initialized = true;
    const customAgents = await loadAcpAgents();
    set({ customAgents, hydrated: true });

    void listen(CHANGED_EVENT, async () => {
      const fresh = await loadAcpAgents();
      set({ customAgents: fresh });
    });
  },
  upsert: (agent) => {
    if (agent.builtIn) return;
    const list = get().customAgents;
    const idx = list.findIndex((entry) => entry.id === agent.id);
    const next =
      idx === -1
        ? [...list, agent]
        : list.map((entry) => (entry.id === agent.id ? agent : entry));
    set({ customAgents: next });
    void saveCustomAcpAgents(next).then(broadcast);
  },
  remove: (id) => {
    const next = get().customAgents.filter((entry) => entry.id !== id);
    set({ customAgents: next });
    void saveCustomAcpAgents(next).then(broadcast);
  },
}));

