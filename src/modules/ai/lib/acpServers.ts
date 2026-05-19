import { LazyStore } from "@tauri-apps/plugin-store";

export type AcpAgentServer = {
  id: string;
  name: string;
  description: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  builtIn: boolean;
};

export const BUILTIN_ACP_AGENTS: readonly AcpAgentServer[] = [
  {
    id: "builtin:codex",
    name: "Codex CLI",
    description: "OpenAI's terminal coding agent with ACP support.",
    command: "codex",
    args: [],
    env: {},
    builtIn: true,
  },
  {
    id: "builtin:claude",
    name: "Claude Code",
    description: "Anthropic's terminal coding agent.",
    command: "claude",
    args: [],
    env: {},
    builtIn: true,
  },
  {
    id: "builtin:gemini",
    name: "Gemini CLI",
    description: "Google's terminal agent. Use custom args if your ACP entrypoint differs.",
    command: "gemini",
    args: [],
    env: {},
    builtIn: true,
  },
  {
    id: "builtin:goose",
    name: "Goose",
    description: "Goose has native ACP support. The standard launch shape is `goose acp`.",
    command: "goose",
    args: ["acp"],
    env: {},
    builtIn: true,
  },
  {
    id: "builtin:pi",
    name: "Pi",
    description:
      "Pi uses an ACP adapter/bridge. This preset starts with the common `pi-acp` command; customize it if your adapter exposes a different binary or args.",
    command: "pi-acp",
    args: [],
    env: {},
    builtIn: true,
  },
] as const;

const STORE_PATH = "terax-ai-acp-agents.json";
const KEY_CUSTOM = "customAcpAgents";

const store = new LazyStore(STORE_PATH, { defaults: {}, autoSave: 200 });

export async function loadAcpAgents(): Promise<AcpAgentServer[]> {
  const entries = await store.entries();
  for (const [key, value] of entries) {
    if (key === KEY_CUSTOM) {
      return (value as AcpAgentServer[]) ?? [];
    }
  }
  return [];
}

export async function saveCustomAcpAgents(
  agents: AcpAgentServer[],
): Promise<void> {
  await store.set(KEY_CUSTOM, agents);
  await store.save();
}

export function newAcpAgentId(): string {
  return `acp-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
}
